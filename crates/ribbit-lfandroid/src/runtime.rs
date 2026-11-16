//! A user-mode runner for a title's native library: Ribbit's ARM core (ARMv7 + VFP) over a flat
//! 32-bit memory, the library's imports trapped at their thunk slots and answered in Rust
//! (`imports.rs`), and a stand-in Java VM for its JNI calls (`jni.rs`). No operating system
//! runs: this is the "Enhanced (new)" path (b) of docs/leappad-academy.md.

use crate::elf::Loaded;
use crate::{DATA_BASE, GAME_BASE, THUNK_BASE};
use ribbit_arm::{Arm, ArmBus, V7};
use std::collections::BTreeMap;

const PAGE: usize = 0x1000;

/// Where `call` returns to: reaching it ends the call.
pub const RETURN: u32 = 0xEFFF_F000;
/// The main thread's stack (grows down from here).
pub const STACK_TOP: u32 = 0xE000_0000;
pub const STACK_SIZE: u32 = 0x0100_0000;
/// The heap (`malloc`).
pub const HEAP: u32 = 0x1000_0000;
pub const HEAP_END: u32 = 0x3000_0000;
/// The JNI function tables' slots (`jni.rs`): JNIEnv's, then JavaVM's.
pub const JNI_ENV_FUNCS: u32 = 0xF800_0000;
pub const JNI_VM_FUNCS: u32 = 0xF900_0000;
/// Functions Ribbit hands the game (OpenSL ES interfaces and the like): `host_fn`.
pub const HOST_FUNCS: u32 = 0xFA00_0000;

/// Flat memory, allocated by page as it is written.
pub struct Mem {
    pages: Vec<Option<Box<[u8; PAGE]>>>,
    /// Reads of memory never written, the first few addresses (diagnostics).
    pub unmapped: Vec<u32>,
    /// The latest such read (LFA_UNMAPPED).
    pub last_unmapped: Option<u32>,
}

impl Default for Mem {
    fn default() -> Self {
        Self { pages: (0..1usize << 20).map(|_| None).collect(), unmapped: Vec::new(), last_unmapped: None }
    }
}

impl Mem {
    fn page(&mut self, a: u32) -> &mut [u8; PAGE] {
        self.pages[(a >> 12) as usize].get_or_insert_with(|| Box::new([0; PAGE]))
    }
    pub fn r8(&mut self, a: u32) -> u8 {
        match &self.pages[(a >> 12) as usize] {
            Some(p) => p[a as usize & (PAGE - 1)],
            None => {
                self.last_unmapped = Some(a);
                if self.unmapped.len() < 16 {
                    self.unmapped.push(a);
                }
                0
            }
        }
    }
    pub fn w8(&mut self, a: u32, v: u8) {
        self.page(a)[a as usize & (PAGE - 1)] = v;
    }
    pub fn r16(&mut self, a: u32) -> u16 {
        self.r8(a) as u16 | (self.r8(a.wrapping_add(1)) as u16) << 8
    }
    pub fn r32(&mut self, a: u32) -> u32 {
        if a & 3 == 0 && let Some(p) = &self.pages[(a >> 12) as usize] {
            let o = a as usize & (PAGE - 1);
            return u32::from_le_bytes(p[o..o + 4].try_into().unwrap());
        }
        self.r16(a) as u32 | (self.r16(a.wrapping_add(2)) as u32) << 16
    }
    pub fn w16(&mut self, a: u32, v: u16) {
        self.w8(a, v as u8);
        self.w8(a.wrapping_add(1), (v >> 8) as u8);
    }
    pub fn w32(&mut self, a: u32, v: u32) {
        if a & 3 == 0 {
            let o = a as usize & (PAGE - 1);
            self.page(a)[o..o + 4].copy_from_slice(&v.to_le_bytes());
            return;
        }
        self.w16(a, v as u16);
        self.w16(a.wrapping_add(2), (v >> 16) as u16);
    }
    pub fn write(&mut self, a: u32, data: &[u8]) {
        for (i, &b) in data.iter().enumerate() {
            self.w8(a.wrapping_add(i as u32), b);
        }
    }
    pub fn read(&mut self, a: u32, n: usize) -> Vec<u8> {
        (0..n).map(|i| self.r8(a.wrapping_add(i as u32))).collect()
    }
    /// A NUL-terminated string (at most 64 KB).
    pub fn cstr(&mut self, a: u32) -> String {
        let mut v = Vec::new();
        for i in 0..0x1_0000u32 {
            let b = self.r8(a.wrapping_add(i));
            if b == 0 {
                break;
            }
            v.push(b);
        }
        String::from_utf8_lossy(&v).into_owned()
    }
}

impl ArmBus for Mem {
    fn read8(&mut self, a: u32) -> u8 {
        self.r8(a)
    }
    fn read16(&mut self, a: u32) -> u16 {
        self.r16(a)
    }
    fn read32(&mut self, a: u32) -> u32 {
        self.r32(a)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.w8(a, v)
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.w16(a, v)
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.w32(a, v)
    }
}

/// Why a run stopped short.
#[derive(Debug)]
pub enum Stop {
    /// An import Ribbit doesn't answer yet (its name, and where it was called from).
    Import(String, u32),
    /// A JNI function not answered yet (table, index, caller).
    Jni(&'static str, usize, u32),
    /// The instruction budget ran out (pc).
    Budget(u32),
    /// The game asked to stop (abort, exit, an undefined instruction...).
    Halt(String),
}

impl std::fmt::Display for Stop {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            Stop::Import(n, lr) => write!(f, "import {n} not answered (called from {lr:#010x})"),
            Stop::Jni(t, i, lr) => write!(f, "{t} function {i} not answered (called from {lr:#010x})"),
            Stop::Budget(pc) => write!(f, "instruction budget spent at {pc:#010x}"),
            Stop::Halt(s) => write!(f, "halted: {s}"),
        }
    }
}

/// A native the library registered with `RegisterNatives`.
#[derive(Debug, Clone)]
pub struct Native {
    pub class: String,
    pub signature: String,
    pub addr: u32,
}

pub struct Runtime {
    pub cpu: Arm,
    pub mem: Mem,
    /// Import names by thunk slot.
    pub thunks: Vec<String>,
    pub exports: BTreeMap<String, u32>,
    pub init: Vec<u32>,
    heap_next: u32,
    /// Allocation sizes by address (free, realloc).
    pub allocs: BTreeMap<u32, u32>,
    pub jni: crate::jni::Jni,
    pub natives: BTreeMap<String, Native>,
    /// OpenSL ES / OpenMAX AL state (`sles.rs`).
    pub sles: crate::sles::Sles,
    /// The APK's assets (`assets.rs`).
    pub assets: crate::assets::Assets,
    /// The writable files (`files.rs`): LFA_DATA, or the folder the host gives.
    pub files: crate::files::Files,
    /// The GL context (`gles.rs`), made on the first GL call.
    pub gles: Option<Box<crate::gles::Gles>>,
    /// glGetString's answers, by contents.
    pub(crate) gl_strings: BTreeMap<String, u32>,
    /// FILEs made by funopen (`stdio.rs`), by FILE*.
    pub(crate) streams: BTreeMap<u32, crate::stdio::Stream>,
    /// The game's threads (`threads.rs`).
    pub threads: crate::threads::Threads,
    /// Calls from Rust into the game in progress (`call` nests).
    pub(crate) depth: u32,
    /// LFA_TRACE: where the last jumps went (and the thread), newest last; each entry's address
    /// moves on with the instructions run after it.
    pub trace: Option<std::collections::VecDeque<(u32, u32)>>,
    /// LFA_TRACE: the last imports called (name, r0-r2, lr).
    pub import_trace: std::collections::VecDeque<(String, [u32; 3], u32)>,
    /// The window's size: the screen, turned for a portrait title.
    pub screen: (u32, u32),
    /// LFA_BREAK: library offsets (hex, comma-separated) where the registers are printed.
    pub breaks: Vec<u32>,
    /// LFA_UNMAPPED=<hex prefix>: stop at the first read of unwritten memory starting with it.
    pub unmapped_stop: Option<String>,
    /// What the game logged (`__android_log_print`, stdio).
    pub log: Vec<String>,
    /// Instructions left before `Stop::Budget`.
    pub budget: u64,
    /// Imports called, by name (first-pass survey).
    pub calls: BTreeMap<String, u64>,
    /// Errors raised inside a nested call, carried out.
    pub(crate) pending: Option<Stop>,
    /// pthread keys' values (one thread so far).
    pub(crate) tls: BTreeMap<u32, u32>,
    pub(crate) next_key: u32,
    /// Host functions handed out (`host_fn`), by slot.
    pub(crate) host: Vec<String>,
    /// Things the import layer keeps between calls.
    pub(crate) state: crate::imports::State,
    /// Touch and buttons sent so far (`input.rs`).
    pub input: crate::input::Input,
}

impl Runtime {
    /// Puts the loaded library in memory and sets up the stack, the JNI tables and the
    /// imports' data slots.
    pub fn new(lib: &Loaded) -> Self {
        let mut mem = Mem::default();
        mem.write(lib.base, &lib.image);
        let mut thunks = Vec::new();
        for (name, &a) in &lib.imports {
            if (THUNK_BASE..DATA_BASE).contains(&a) {
                let i = ((a - THUNK_BASE) / 8) as usize;
                if i >= thunks.len() {
                    thunks.resize(i + 1, String::new());
                }
                thunks[i] = name.clone();
            }
        }
        let mut cpu = Arm::new();
        cpu.v7 = Some(Box::<V7>::default());
        cpu.set_mode(ribbit_arm::mode::USR);
        cpu.r[13] = STACK_TOP;
        let mut rt = Runtime {
            cpu,
            mem,
            thunks,
            exports: lib.exports.clone(),
            init: lib.init.clone(),
            heap_next: HEAP,
            allocs: BTreeMap::new(),
            jni: Default::default(),
            natives: BTreeMap::new(),
            sles: Default::default(),
            assets: Default::default(),
            files: crate::files::Files::new(std::env::var_os("LFA_DATA").map(Into::into)),
            gles: None,
            gl_strings: BTreeMap::new(),
            threads: Default::default(),
            streams: BTreeMap::new(),
            depth: 0,
            trace: std::env::var_os("LFA_TRACE").map(|_| Default::default()),
            import_trace: Default::default(),
            screen: crate::gles::SCREEN,
            unmapped_stop: std::env::var("LFA_UNMAPPED").ok().map(|v| v.to_lowercase()),
            breaks: std::env::var("LFA_BREAK").map_or(Vec::new(), |v| {
                v.split(',').filter_map(|a| u32::from_str_radix(a.trim().trim_start_matches("0x"), 16).ok()).map(|a| a + GAME_BASE).collect()
            }),
            log: Vec::new(),
            budget: 2_000_000_000,
            calls: BTreeMap::new(),
            pending: None,
            tls: BTreeMap::new(),
            next_key: 1,
            host: Vec::new(),
            state: Default::default(),
            input: Default::default(),
        };
        // Data imports (__stack_chk_guard, __sF, _ctype_, SL_IID_*): the code reads them
        // where they were bound.
        for (name, &a) in &lib.imports {
            if (DATA_BASE..JNI_ENV_FUNCS).contains(&a) {
                crate::imports::init_data(&mut rt, name, a);
            }
        }
        crate::jni::install(&mut rt);
        rt
    }

    pub fn malloc(&mut self, n: u32) -> u32 {
        let n = n.max(1).next_multiple_of(16);
        let a = self.heap_next;
        self.heap_next += n;
        assert!(self.heap_next < HEAP_END, "lfandroid: heap full");
        self.allocs.insert(a, n);
        a
    }

    pub fn free(&mut self, a: u32) {
        self.allocs.remove(&a);
    }

    /// A NUL-terminated copy of `s` on the heap.
    pub fn alloc_str(&mut self, s: &str) -> u32 {
        let a = self.malloc(s.len() as u32 + 1);
        self.mem.write(a, s.as_bytes());
        self.mem.w8(a + s.len() as u32, 0);
        a
    }

    /// A function the game can call that Ribbit answers (`imports::host_call`), by name.
    pub fn host_fn(&mut self, name: &str) -> u32 {
        let i = match self.host.iter().position(|h| h == name) {
            Some(i) => i,
            None => {
                self.host.push(name.to_string());
                self.host.len() - 1
            }
        };
        HOST_FUNCS + 8 * i as u32
    }

    /// The `i`th argument of the function being answered (AAPCS: r0-r3, then the stack).
    pub fn arg(&mut self, i: usize) -> u32 {
        if i < 4 { self.cpu.r[i] } else { self.mem.r32(self.cpu.r[13] + 4 * (i as u32 - 4)) }
    }

    /// Calls the game's function at `addr` (bit 0: Thumb) with `args`, until it returns.
    pub fn call(&mut self, addr: u32, args: &[u32]) -> Result<u32, Stop> {
        let saved = (self.cpu.r, self.cpu.cpsr);
        let mut sp = self.cpu.r[13];
        if args.len() > 4 {
            let extra = &args[4..];
            sp -= (4 * extra.len() as u32).next_multiple_of(8);
            for (i, &v) in extra.iter().enumerate() {
                self.mem.w32(sp + 4 * i as u32, v);
            }
        }
        sp &= !7;
        for (i, &v) in args.iter().take(4).enumerate() {
            self.cpu.r[i] = v;
        }
        self.cpu.r[13] = sp;
        self.cpu.r[14] = RETURN;
        self.jump(addr);
        self.depth += 1;
        let out = self.run();
        self.depth -= 1;
        let r0 = self.cpu.r[0];
        self.cpu.r = saved.0;
        self.cpu.cpsr = saved.1;
        out.map(|()| r0)
    }

    fn jump(&mut self, addr: u32) {
        let t = ribbit_arm::psr::T;
        if addr & 1 != 0 {
            self.cpu.cpsr |= t;
        } else {
            self.cpu.cpsr &= !t;
        }
        self.cpu.r[15] = addr & !1;
    }

    /// Returns from the function being answered with `v` in r0.
    pub fn ret(&mut self, v: u32) {
        self.cpu.r[0] = v;
        let lr = self.cpu.r[14];
        self.jump(lr);
    }

    /// Returns a 64-bit value (r0, r1).
    pub fn ret64(&mut self, v: u64) {
        self.cpu.r[1] = (v >> 32) as u32;
        self.ret(v as u32);
    }

    fn run(&mut self) -> Result<(), Stop> {
        loop {
            let pc = self.cpu.r[15];
            if pc == RETURN {
                return Ok(());
            }
            if pc == crate::threads::THREAD_EXIT {
                let v = self.cpu.r[0];
                self.thread_exit(v)?;
                continue;
            }
            if pc >= THUNK_BASE && !(DATA_BASE..JNI_ENV_FUNCS).contains(&pc) {
                let lr = self.cpu.r[14];
                let r = if pc >= HOST_FUNCS {
                    let i = ((pc - HOST_FUNCS) / 8) as usize;
                    let name = self.host.get(i).cloned().unwrap_or_default();
                    crate::imports::host_call(self, &name)
                } else if pc >= JNI_VM_FUNCS {
                    crate::jni::vm_call(self, ((pc - JNI_VM_FUNCS) / 8) as usize)
                } else if pc >= JNI_ENV_FUNCS {
                    crate::jni::env_call(self, ((pc - JNI_ENV_FUNCS) / 8) as usize)
                } else {
                    let i = ((pc - THUNK_BASE) / 8) as usize;
                    let name = self.thunks.get(i).cloned().unwrap_or_default();
                    *self.calls.entry(name.clone()).or_default() += 1;
                    if self.trace.is_some() {
                        if self.import_trace.len() == 100 {
                            self.import_trace.pop_front();
                        }
                        self.import_trace.push_back((name.clone(), [self.cpu.r[0], self.cpu.r[1], self.cpu.r[2]], lr));
                    }
                    crate::imports::call(self, &name)
                };
                if let Some(e) = self.pending.take() {
                    return Err(e);
                }
                r?;
                // Another thread runs now (this one waits at the slot, or returned).
                if std::mem::take(&mut self.threads.switched) {
                    continue;
                }
                // Still at the slot: the answer forgot to return.
                if self.cpu.r[15] == pc {
                    let _ = lr;
                    return Err(Stop::Halt(format!("import at {pc:#x} didn't return")));
                }
                continue;
            }
            if self.budget == 0 {
                return Err(Stop::Budget(pc));
            }
            // The games have no code outside their library.
            if !in_library(pc) {
                let r = self.cpu.r;
                let bt = self.backtrace();
                return Err(Stop::Halt(format!("jumped out of the library to {pc:#010x} (lr {:#010x}; r0-r7 {:08x?}); backtrace {bt:08x?}", r[14], &r[..8])));
            }
            self.budget -= 1;
            if self.breaks.contains(&pc) {
                let r = self.cpu.r;
                eprintln!("break {:#x}: r0-r12 {:08x?} sp {:08x} lr {:08x}", pc - GAME_BASE, &r[..13], r[13], r[14]);
                // Arguments that point at text, or at a pointer to text (a std::string).
                for (i, &a) in r[..4].iter().enumerate() {
                    for (deref, a) in [(false, a), (true, if self.mem.pages[(a >> 12) as usize].is_some() { self.mem.r32(a) } else { 0 })] {
                        if self.mem.pages[(a >> 12) as usize].is_some() {
                            let s = self.mem.cstr(a);
                            if s.len() >= 3 && s.len() < 200 && s.bytes().all(|b| (0x20..0x7F).contains(&b)) {
                                eprintln!("  {}r{i} = {s:?}", if deref { "*" } else { "" });
                            }
                        }
                    }
                }
            }
            self.tick()?;
            if std::mem::take(&mut self.threads.switched) {
                continue;
            }
            if let Some(t) = self.trace.as_mut() {
                // Jumps only: where this isn't the instruction after the last one.
                let tid = self.threads.current_id();
                if t.back().is_none_or(|&(last, lt)| pc.wrapping_sub(last) > 4 || lt != tid) {
                    if t.len() == 256 {
                        t.pop_front();
                    }
                    t.push_back((pc, tid));
                } else if let Some(b) = t.back_mut() {
                    b.0 = pc;
                }
            }
            self.mem.last_unmapped = None;
            self.cpu.step(&mut self.mem);
            if let Some(p) = &self.unmapped_stop
                && let Some(a) = self.mem.last_unmapped.take()
                && format!("{a:08x}").starts_with(p.as_str())
            {
                let r = self.cpu.r;
                let bt = self.backtrace();
                return Err(Stop::Halt(format!("LFA_UNMAPPED: read {a:#010x} at {pc:#010x} (lib {:#x}); r0-r12 {:08x?} sp {:08x} lr {:08x}; backtrace {bt:08x?}", pc.wrapping_sub(GAME_BASE), &r[..13], r[13], r[14])));
            }
            if let Some(&(at, w)) = self.cpu.undefined_log.last() {
                self.cpu.undefined_log.clear();
                return Err(Stop::Halt(format!("undefined instruction {w:#010x} at {at:#010x}")));
            }
            if self.cpu.mode() != ribbit_arm::mode::USR {
                return Err(Stop::Halt(format!("exception into mode {:#x} from {:#010x}", self.cpu.mode(), self.cpu.current)));
            }
        }
    }

    /// Runs the constructors (DT_INIT, then DT_INIT_ARRAY in order), as the dynamic linker
    /// does before `JNI_OnLoad`.
    pub fn run_constructors(&mut self) -> Result<(), Stop> {
        for f in self.init.clone() {
            if f == 0 || f == u32::MAX {
                continue;
            }
            self.call(f, &[])?;
        }
        Ok(())
    }

    /// The library's `JNI_OnLoad(vm, NULL)`: it registers its natives.
    pub fn jni_onload(&mut self) -> Result<u32, Stop> {
        let f = *self.exports.get("JNI_OnLoad").ok_or_else(|| Stop::Halt("no JNI_OnLoad".into()))?;
        let vm = self.jni.vm;
        self.call(f, &[vm, 0])
    }
}

impl Runtime {
    /// Return addresses found on the stack (words just after a BL/BLX in the library), the
    /// innermost first: a rough backtrace for reports.
    pub fn backtrace(&mut self) -> Vec<u32> {
        let sp = self.cpu.r[13];
        let mut out = Vec::new();
        for i in 0..2048 {
            let w = self.mem.r32(sp.wrapping_add(4 * i));
            if !in_library(w) {
                continue;
            }
            let call = if w & 1 == 0 {
                let insn = self.mem.r32(w - 4);
                insn & 0x0F00_0000 == 0x0B00_0000 || insn & 0xFE00_0000 == 0xFA00_0000 || insn & 0x0FFF_FFF0 == 0x012F_FF30
            } else {
                // Thumb: a 32-bit BL/BLX (F000 xxxx, D000/C000 suffix) or BLX register.
                let a = w & !1;
                let (h1, h2) = (self.mem.r16(a - 4), self.mem.r16(a - 2));
                (h1 & 0xF800 == 0xF000 && h2 & 0xC000 == 0xC000) || h2 & 0xFF87 == 0x4780
            };
            if call {
                out.push(w);
                if out.len() == 24 {
                    break;
                }
            }
        }
        out
    }

    /// Calls a registered native as the Java shell would: `(env, this, args...)`, with `this`
    /// an object of the native's class.
    pub fn call_native(&mut self, name: &str, args: &[u32]) -> Result<u32, Stop> {
        let n = self.natives.get(name).ok_or_else(|| Stop::Halt(format!("no native {name}")))?;
        let (addr, class) = (n.addr, n.class.clone());
        let this = self.jni.new_handle(crate::jni::Obj::Object(class));
        let mut all = vec![self.jni.env, this];
        all.extend_from_slice(args);
        self.call(addr, &all)
    }
}

/// Where the library sits (for messages).
pub fn in_library(a: u32) -> bool {
    (GAME_BASE..GAME_BASE + 0x0100_0000).contains(&a)
}
