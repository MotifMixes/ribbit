//! OpenGL ES 1.1 and EGL for the programs (the LeapPad 2's Lightning draws
//! with them on the LF2000's Nexell VR5 GPU, which Ribbit doesn't model).
//! The libraries' exported functions are replaced in the file system with
//! `svc #0xF0nnnn; bx lr` (`patch_libraries`), so a call traps here with its
//! arguments in place; `ribbit_gles` does the work and draws in software,
//! and eglSwapBuffers shows the picture in the frame buffer. Their own
//! constructors and internal functions stay as they are.

use super::Process;
use super::vfs::{Data, Vfs};

/// The SWI numbers: 0xF00000 + the function's index in `NAMES_FILE`.
pub(super) const SWI_BASE: u32 = 0xF0_0000;
/// The index table, one name a line (in the file tree, so save states and
/// every process have the same).
const NAMES_FILE: &str = "/proc/ribbit-gles-hooks";
/// Where the strings glGetString/eglQueryString return live, a page of
/// each process's own memory.
const STRINGS: u32 = 0xB000_0000;

/// The libraries whose exported functions become hooks.
const LIBRARIES: &[&str] = &["/usr/lib/libEGL.so.1.4", "/usr/lib/libGLESv1_CM.so.1.1", "/usr/lib/libGLES.so", "/usr/lib/libvr5.so", "/usr/lib/libGLESv2.so.2.0"];

/// The GL state of the running system (not in save states: a state loads
/// with a fresh context; GUESS: good enough while a game re-uploads its
/// textures, to revisit).
pub struct Host {
    pub gl: ribbit_gles::Gl,
    pub egl: ribbit_gles::Egl,
    names: Vec<String>,
    /// Calls by name (diagnostics: EXPLORER_HLE_GLES_LOG=1 prints them).
    pub counts: std::collections::BTreeMap<String, u64>,
    log: bool,
    /// Swaps done.
    pub swaps: u64,
}

/// The exported functions (name, file offset) of an ARM ELF shared library.
fn exported_functions(elf: &[u8]) -> Vec<(String, usize)> {
    let u16_at = |o: usize| elf.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]])) as usize;
    let u32_at = |o: usize| elf.get(o..o + 4).map_or(0, |b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])) as usize;
    if elf.get(..4) != Some(b"\x7fELF") {
        return Vec::new();
    }
    let (phoff, phnum) = (u32_at(28), u16_at(44));
    let (shoff, shentsize, shnum) = (u32_at(32), u16_at(46), u16_at(48));
    // vaddr -> file offset through the PT_LOAD segments.
    let loads: Vec<(usize, usize, usize)> =
        (0..phnum).map(|i| phoff + i * 32).filter(|&p| u32_at(p) == 1).map(|p| (u32_at(p + 8), u32_at(p + 4), u32_at(p + 16))).collect();
    let to_file = |va: usize| loads.iter().find(|&&(v, _, sz)| va >= v && va < v + sz).map(|&(v, off, _)| va - v + off);
    let mut out = Vec::new();
    for i in 0..shnum {
        let sh = shoff + i * shentsize;
        if u32_at(sh + 4) != 11 {
            continue; // SHT_DYNSYM
        }
        let (off, size, link) = (u32_at(sh + 16), u32_at(sh + 20), u32_at(sh + 24));
        let strtab = u32_at(shoff + link * shentsize + 16);
        for s in (off..off + size).step_by(16) {
            let (name, value, sz, info, shndx) = (u32_at(s), u32_at(s + 4), u32_at(s + 8), elf.get(s + 12).copied().unwrap_or(0), u16_at(s + 14));
            // STT_FUNC, global or weak, defined, ARM state, room for two words.
            if info & 0xF != 2 || info >> 4 == 0 || shndx == 0 || value & 1 != 0 || sz < 8 {
                continue;
            }
            let start = strtab + name;
            let end = elf[start..].iter().position(|&b| b == 0).map_or(start, |e| start + e);
            let n = String::from_utf8_lossy(&elf[start..end]).into_owned();
            if let Some(fo) = to_file(value) {
                out.push((n, fo));
            }
        }
    }
    out
}

/// Replaces the GL/EGL libraries' exported functions with hooks and
/// writes the index table. Returns how many functions became hooks.
pub fn patch_libraries(vfs: &mut Vfs) -> usize {
    let mut names: Vec<String> = Vec::new();
    for path in LIBRARIES {
        let Some(Data::File(data)) = vfs.get(path).map(|n| n.data.clone()) else { continue };
        let mut data = data;
        for (name, off) in exported_functions(&data) {
            // Names starting with an underscore are the libraries' own
            // (C++ helpers, _init/_fini): left alone except vr5's API.
            if name.starts_with('_') && !name.starts_with("__vr5") {
                continue;
            }
            let idx = match names.iter().position(|n| *n == name) {
                Some(i) => i,
                None => {
                    names.push(name.clone());
                    names.len() - 1
                }
            };
            if idx > 0xFFFF || off + 8 > data.len() {
                continue;
            }
            let svc = 0xEF00_0000 | (SWI_BASE + idx as u32);
            data[off..off + 4].copy_from_slice(&svc.to_le_bytes());
            data[off + 4..off + 8].copy_from_slice(&0xE12F_FF1Eu32.to_le_bytes()); // bx lr
        }
        let mode = vfs.get(path).map_or(0o100755, |n| n.mode);
        vfs.insert(path, mode, Data::File(data));
    }
    vfs.add_file(NAMES_FILE, names.join("\n").into_bytes());
    names.len()
}

/// The arguments and memory of the calling thread.
struct Guest<'a> {
    p: &'a mut Process,
    sp: u32,
}

impl ribbit_gles::Memory for Guest<'_> {
    fn read(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        self.p.mem().read_bytes(addr, len)
    }
    fn write(&mut self, addr: u32, data: &[u8]) -> bool {
        self.p.mem().write_bytes(addr, data)
    }
}

impl ribbit_gles::abi::Guest for Guest<'_> {
    fn arg(&mut self, i: usize) -> u32 {
        if i < 4 { self.p.cpu.r[i] } else { self.p.mem().get32(self.sp.wrapping_add(4 * (i as u32 - 4))).unwrap_or(0) }
    }
    fn static_string(&mut self, s: &str) -> u32 {
        // Each string at its own slot of the page, by a hash of its text.
        let slot = (s.bytes().fold(5381u32, |h, b| h.wrapping_mul(33) ^ b as u32) % 8) * 512;
        let mut mem = self.p.mem();
        if !mem.mapped(STRINGS) {
            mem.map(STRINGS, 4096);
        }
        let mut bytes = s.as_bytes()[..s.len().min(510)].to_vec();
        bytes.push(0);
        mem.write_bytes(STRINGS + slot, &bytes);
        STRINGS + slot
    }
    fn window_size(&mut self, _native: u32) -> (u32, u32) {
        self.p.screen()
    }
}

impl Process {
    /// A call to hooked function `idx` (SWI 0xF00000 + idx): its result in r0.
    pub(super) fn gles_call(&mut self, idx: u32) {
        if self.gles.is_none() {
            let names = match self.vfs.get(NAMES_FILE).map(|n| &n.data) {
                Some(Data::File(d)) => String::from_utf8_lossy(d).lines().map(str::to_string).collect(),
                _ => Vec::new(),
            };
            let (w, h) = self.screen();
            self.gles = Some(Box::new(Host {
                gl: ribbit_gles::Gl::new(w, h),
                egl: Default::default(),
                names,
                counts: Default::default(),
                log: std::env::var_os("EXPLORER_HLE_GLES_LOG").is_some(),
                swaps: 0,
            }));
        }
        let mut host = self.gles.take().unwrap();
        let name = host.names.get(idx as usize).cloned().unwrap_or_default();
        *host.counts.entry(name.clone()).or_default() += 1;
        if host.log && host.counts[&name] <= 3 {
            eprintln!("[gles {}] {name}({:08X?})", self.tid, &self.cpu.r[..4]);
        }
        let sp = self.cpu.r[13];
        let outcome = {
            let mut g = Guest { p: self, sp };
            if name.starts_with("egl") {
                ribbit_gles::abi::egl_call(&mut host.egl, &mut host.gl, &name, &mut g)
            } else if name.starts_with("gl") {
                ribbit_gles::abi::gl_call(&mut host.gl, &name, &mut g)
            } else {
                ribbit_gles::abi::Outcome::Unknown
            }
        };
        let r0 = match outcome {
            ribbit_gles::abi::Outcome::Value(v) => v,
            ribbit_gles::abi::Outcome::Swap(_) => {
                host.swaps += 1;
                if host.log && host.swaps.is_power_of_two() {
                    eprintln!("[gles] swap {} at {:.1} s, {} draws since start", host.swaps, self.since_start().as_secs_f64(), host.gl.draws);
                }
                self.gles_present(&host.gl.surface);
                1
            }
            ribbit_gles::abi::Outcome::Unknown => {
                self.log_once(format!("GL/EGL/VR5 function {name} not handled (returns 0)"));
                0
            }
        };
        for u in std::mem::take(&mut host.gl.unsupported) {
            self.log_once(format!("GL: {u} not drawn"));
        }
        self.cpu.r[0] = r0;
        self.gles = Some(host);
    }

    /// eglSwapBuffers: the GL picture into the frame buffer that layer 0
    /// shows (the RGB layer DisplayMPI gives the GL display; GUESS until
    /// its UpdateOpenGL path is followed), as 32-bit pixels.
    fn gles_present(&mut self, s: &ribbit_gles::Surface) {
        let (addr, stride) = self.fb_layer_memory(0);
        let (w, h) = self.screen();
        let img = s.to_xrgb();
        for y in 0..h.min(s.height) as usize {
            let row = &img[y * s.width as usize..y * s.width as usize + w.min(s.width) as usize];
            let bytes: Vec<u8> = row.iter().flat_map(|p| (0xFF00_0000 | p).to_le_bytes()).collect();
            let at = addr as usize + y * stride as usize;
            if let Some(dst) = self.m.bus.sdram.get_mut(at..at + bytes.len()) {
                dst.copy_from_slice(&bytes);
            }
        }
    }
}
