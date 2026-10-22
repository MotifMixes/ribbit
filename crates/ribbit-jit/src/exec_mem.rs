//! Executable memory for generated code, W^X: pages are mapped read-write,
//! the code is copied in, then the pages become read-execute. They are never
//! writable and executable at once (Windows CFG/ACG, macOS hardened runtime,
//! SELinux `execmem` policies all forbid or penalise RWX).
//!
//! * Windows: `VirtualAlloc` (RW), `VirtualProtect` (RX), `FlushInstructionCache`.
//! * Unix: `mmap` (RW, anonymous), `mprotect` (RX). x86-64 keeps its
//!   instruction cache coherent, so no flush is needed there. macOS on ARM64
//!   will need `MAP_JIT` and `pthread_jit_write_protect_np` (phase 3,
//!   docs/jit-design.md); x86-64 macOS works with `mprotect` as long as the
//!   app is not signed with the hardened runtime without the JIT entitlement.

use std::io;

/// One mapping of read-execute code. Freed on drop.
pub struct ExecMemory {
    ptr: *mut u8,
    /// Mapped size (whole pages).
    size: usize,
    /// Bytes of code.
    len: usize,
}

// The mapping is immutable once built (read-execute), so sharing is safe.
unsafe impl Send for ExecMemory {}
unsafe impl Sync for ExecMemory {}

impl ExecMemory {
    /// Maps `code` as read-execute memory.
    pub fn new(code: &[u8]) -> io::Result<ExecMemory> {
        let page = page_size();
        let size = code.len().max(1).div_ceil(page) * page;
        let ptr = sys::map_rw(size)?;
        // SAFETY: `ptr` is a fresh read-write mapping of `size >= code.len()` bytes.
        unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), ptr, code.len()) };
        if let Err(e) = sys::protect_rx(ptr, size) {
            // SAFETY: unmapping the mapping made above.
            unsafe { sys::unmap(ptr, size) };
            return Err(e);
        }
        Ok(ExecMemory { ptr, size, len: code.len() })
    }

    /// Start of the code.
    pub fn as_ptr(&self) -> *const u8 {
        self.ptr
    }

    /// Bytes of code (for disassembly dumps).
    pub fn code(&self) -> &[u8] {
        // SAFETY: the mapping is readable and holds `len` bytes of code.
        unsafe { std::slice::from_raw_parts(self.ptr, self.len) }
    }
}

impl Drop for ExecMemory {
    fn drop(&mut self) {
        // SAFETY: `ptr`/`size` describe the mapping made in `new`.
        unsafe { sys::unmap(self.ptr, self.size) };
    }
}

fn page_size() -> usize {
    sys::page_size()
}

/// Code for many blocks in shared mappings ("chunks"), still never writable
/// and executable at once. Where the system allows it, a chunk is mapped
/// twice, a read-write view the code is copied into and a read-execute view
/// it runs from, so adding code needs no system call (Windows: a section
/// mapped twice; Linux: a memfd mapped twice). Elsewhere the pages the new
/// code lands on are made read-write, written, and made read-execute again
/// (the caller runs no code meanwhile: one thread). Everything is freed on
/// drop.
#[derive(Default)]
pub struct Arena {
    chunks: Vec<Chunk>,
}

struct Chunk {
    /// Where the code runs.
    exec: *mut u8,
    /// Where it is written: a second view of the same memory, or `exec`
    /// (protection flipped around each write).
    write: *mut u8,
    size: usize,
    used: usize,
}

// As `ExecMemory`: the chunks are only written by `add`, which takes `&mut`.
unsafe impl Send for Arena {}

/// Chunk size (a multiple of every page size in use).
const CHUNK: usize = 1 << 20;

impl Arena {
    /// Copies `code` into a chunk and returns its address (16-byte aligned).
    pub fn add(&mut self, code: &[u8]) -> io::Result<*const u8> {
        let need = code.len().next_multiple_of(16);
        if self.chunks.last().is_none_or(|c| c.used + need > c.size) {
            let size = need.max(CHUNK).div_ceil(page_size()) * page_size();
            let chunk = match sys::map_dual(size) {
                Some((exec, write)) => Chunk { exec, write, size, used: 0 },
                None => {
                    let ptr = sys::map_rw(size)?;
                    if let Err(e) = sys::protect_rx(ptr, size) {
                        // SAFETY: the mapping made above.
                        unsafe { sys::unmap(ptr, size) };
                        return Err(e);
                    }
                    Chunk { exec: ptr, write: ptr, size, used: 0 }
                }
            };
            self.chunks.push(chunk);
        }
        let c = self.chunks.last_mut().expect("a chunk");
        // SAFETY: both views hold room for `code` at `used` (checked above).
        let (at, dst) = unsafe { (c.exec.add(c.used), c.write.add(c.used)) };
        if c.write != c.exec {
            // SAFETY: the write view is read-write.
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len()) };
            sys::flush(at, code.len());
        } else {
            let page = page_size();
            let (from, to) = (c.used / page * page, (c.used + need).div_ceil(page) * page);
            // SAFETY: within the chunk (`to` <= its size, a multiple of the page size).
            let pages = unsafe { c.exec.add(from) };
            sys::protect_rw(pages, to - from)?;
            // SAFETY: those pages are read-write now.
            unsafe { std::ptr::copy_nonoverlapping(code.as_ptr(), dst, code.len()) };
            sys::protect_rx(pages, to - from)?;
            sys::flush(at, code.len());
        }
        c.used += need;
        Ok(at)
    }

    /// Rewrites `bytes` of code at `at` (an address `add` returned, or within
    /// such code), through the write view or by flipping the page's
    /// protection; false if `at` is not code held here. No code may be running
    /// meanwhile (one thread).
    pub fn patch(&mut self, at: *const u8, bytes: &[u8]) -> bool {
        let Some(c) = self.chunks.iter_mut().find(|c| {
            let start = c.exec as usize;
            (start..start + c.used).contains(&(at as usize)) && at as usize + bytes.len() <= start + c.used
        }) else {
            return false;
        };
        let off = at as usize - c.exec as usize;
        if c.write != c.exec {
            // SAFETY: within the chunk's write view (checked above).
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), c.write.add(off), bytes.len()) };
        } else {
            let page = page_size();
            let (from, to) = (off / page * page, (off + bytes.len()).div_ceil(page) * page);
            // SAFETY: within the chunk (its size is a multiple of the page size).
            let pages = unsafe { c.exec.add(from) };
            if sys::protect_rw(pages, to - from).is_err() {
                return false;
            }
            // SAFETY: those pages are read-write now.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), c.exec.add(off), bytes.len()) };
            if sys::protect_rx(pages, to - from).is_err() {
                return false;
            }
        }
        sys::flush(at, bytes.len());
        true
    }

    /// Bytes of code held.
    pub fn used(&self) -> usize {
        self.chunks.iter().map(|c| c.used).sum()
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        for c in &self.chunks {
            // SAFETY: each chunk's views were made by `add`.
            unsafe {
                if c.write != c.exec {
                    sys::unmap_dual(c.exec, c.write, c.size);
                } else {
                    sys::unmap(c.exec, c.size);
                }
            }
        }
    }
}

#[cfg(windows)]
mod sys {
    use std::io;
    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        CreateFileMappingW, FILE_MAP_EXECUTE, FILE_MAP_READ, FILE_MAP_WRITE, MEM_COMMIT, MEM_RELEASE, MEM_RESERVE,
        MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_EXECUTE_READ, PAGE_EXECUTE_READWRITE, PAGE_READWRITE,
        UnmapViewOfFile, VirtualAlloc, VirtualFree, VirtualProtect,
    };
    use windows_sys::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub fn page_size() -> usize {
        // SAFETY: GetSystemInfo fills the struct it is given.
        unsafe {
            let mut info: SYSTEM_INFO = std::mem::zeroed();
            GetSystemInfo(&mut info);
            info.dwPageSize as usize
        }
    }

    pub fn map_rw(size: usize) -> io::Result<*mut u8> {
        // SAFETY: a fresh anonymous allocation.
        let p = unsafe { VirtualAlloc(std::ptr::null(), size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) };
        if p.is_null() { Err(io::Error::last_os_error()) } else { Ok(p as *mut u8) }
    }

    pub fn protect_rw(ptr: *mut u8, size: usize) -> io::Result<()> {
        let mut old = 0;
        // SAFETY: `ptr`/`size` is a mapping made by `map_rw`.
        let ok = unsafe { VirtualProtect(ptr as _, size, PAGE_READWRITE, &mut old) };
        if ok == 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    pub fn protect_rx(ptr: *mut u8, size: usize) -> io::Result<()> {
        let mut old = 0;
        // SAFETY: `ptr`/`size` is a mapping made by `map_rw`.
        let ok = unsafe { VirtualProtect(ptr as _, size, PAGE_EXECUTE_READ, &mut old) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: as above; the current process's own pages.
        unsafe { FlushInstructionCache(GetCurrentProcess(), ptr as _, size) };
        Ok(())
    }

    /// # Safety
    /// `ptr` must come from `map_rw` and not be used afterwards.
    pub unsafe fn unmap(ptr: *mut u8, _size: usize) {
        // SAFETY: the caller's contract.
        unsafe { VirtualFree(ptr as _, 0, MEM_RELEASE) };
    }

    /// A pagefile-backed section of `size` bytes mapped twice: (read-execute
    /// view, read-write view). None if the system refuses (then pages are
    /// flipped instead).
    pub fn map_dual(size: usize) -> Option<(*mut u8, *mut u8)> {
        // SAFETY: a fresh section; each view is checked, and the handle is
        // closed once mapped (the views keep the section).
        unsafe {
            let h = CreateFileMappingW(
                INVALID_HANDLE_VALUE,
                std::ptr::null(),
                PAGE_EXECUTE_READWRITE,
                (size as u64 >> 32) as u32,
                size as u32,
                std::ptr::null(),
            );
            if h.is_null() {
                return None;
            }
            let w = MapViewOfFile(h, FILE_MAP_WRITE, 0, 0, size);
            let x = MapViewOfFile(h, FILE_MAP_READ | FILE_MAP_EXECUTE, 0, 0, size);
            CloseHandle(h);
            if w.Value.is_null() || x.Value.is_null() {
                if !w.Value.is_null() {
                    UnmapViewOfFile(w);
                }
                if !x.Value.is_null() {
                    UnmapViewOfFile(x);
                }
                return None;
            }
            Some((x.Value as *mut u8, w.Value as *mut u8))
        }
    }

    /// # Safety
    /// The views must come from `map_dual` and not be used afterwards.
    pub unsafe fn unmap_dual(exec: *mut u8, write: *mut u8, _size: usize) {
        // SAFETY: the caller's contract.
        unsafe {
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: exec as _ });
            UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: write as _ });
        }
    }

    /// x86-64 keeps instruction fetches coherent with stores, also through
    /// another view of the same memory (it snoops by physical address), so
    /// the system call `FlushInstructionCache` would only cost time here.
    pub fn flush(_: *const u8, _: usize) {}
}

#[cfg(unix)]
mod sys {
    use std::io;

    pub fn page_size() -> usize {
        // SAFETY: sysconf has no preconditions.
        let n = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if n > 0 { n as usize } else { 4096 }
    }

    pub fn map_rw(size: usize) -> io::Result<*mut u8> {
        // SAFETY: a fresh anonymous private mapping.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                size,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED { Err(io::Error::last_os_error()) } else { Ok(p as *mut u8) }
    }

    pub fn protect_rw(ptr: *mut u8, size: usize) -> io::Result<()> {
        // SAFETY: `ptr`/`size` is a mapping made by `map_rw`.
        let r = unsafe { libc::mprotect(ptr as _, size, libc::PROT_READ | libc::PROT_WRITE) };
        if r != 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    pub fn protect_rx(ptr: *mut u8, size: usize) -> io::Result<()> {
        // SAFETY: `ptr`/`size` is a mapping made by `map_rw`.
        let r = unsafe { libc::mprotect(ptr as _, size, libc::PROT_READ | libc::PROT_EXEC) };
        if r != 0 { Err(io::Error::last_os_error()) } else { Ok(()) }
    }

    /// # Safety
    /// `ptr` must come from `map_rw` and not be used afterwards.
    pub unsafe fn unmap(ptr: *mut u8, size: usize) {
        // SAFETY: the caller's contract.
        unsafe { libc::munmap(ptr as _, size) };
    }

    /// Linux: a memfd of `size` bytes mapped twice: (read-execute view,
    /// read-write view). None elsewhere or if refused.
    #[cfg(target_os = "linux")]
    pub fn map_dual(size: usize) -> Option<(*mut u8, *mut u8)> {
        // Profiling: anonymous mappings, which perf names from its map file.
        if crate::full::perf_map_on() {
            return None;
        }
        // SAFETY: a fresh memfd; each mapping is checked; the descriptor is
        // closed once mapped (the mappings keep the memory).
        unsafe {
            let fd = libc::memfd_create(c"ribbit-jit".as_ptr(), libc::MFD_CLOEXEC);
            if fd < 0 {
                return None;
            }
            if libc::ftruncate(fd, size as libc::off_t) != 0 {
                libc::close(fd);
                return None;
            }
            let w = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, fd, 0);
            let x = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_EXEC, libc::MAP_SHARED, fd, 0);
            libc::close(fd);
            if w == libc::MAP_FAILED || x == libc::MAP_FAILED {
                if w != libc::MAP_FAILED {
                    libc::munmap(w, size);
                }
                if x != libc::MAP_FAILED {
                    libc::munmap(x, size);
                }
                return None;
            }
            Some((x as *mut u8, w as *mut u8))
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub fn map_dual(_: usize) -> Option<(*mut u8, *mut u8)> {
        None
    }

    /// # Safety
    /// The views must come from `map_dual` and not be used afterwards.
    pub unsafe fn unmap_dual(exec: *mut u8, write: *mut u8, size: usize) {
        // SAFETY: the caller's contract.
        unsafe {
            libc::munmap(exec as _, size);
            libc::munmap(write as _, size);
        }
    }

    /// x86-64 keeps instruction fetches coherent with stores.
    #[cfg(target_arch = "x86_64")]
    pub fn flush(_: *const u8, _: usize) {}

    /// AArch64 requires explicit icache invalidation after writing code.
    #[cfg(target_arch = "aarch64")]
    pub fn flush(ptr: *const u8, len: usize) {
        // SAFETY: ptr..ptr+len is executable memory we just wrote.
        unsafe { dynasmrt::cache_control::synchronize_icache(std::slice::from_raw_parts(ptr, len)) }
    }
}

#[cfg(not(any(windows, unix)))]
mod sys {
    use std::io;

    pub fn page_size() -> usize {
        4096
    }

    pub fn map_rw(_: usize) -> io::Result<*mut u8> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no executable memory on this platform"))
    }

    pub fn protect_rw(_: *mut u8, _: usize) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no executable memory on this platform"))
    }

    pub fn protect_rx(_: *mut u8, _: usize) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no executable memory on this platform"))
    }

    pub unsafe fn unmap(_: *mut u8, _: usize) {}

    pub fn map_dual(_: usize) -> Option<(*mut u8, *mut u8)> {
        None
    }

    pub unsafe fn unmap_dual(_: *mut u8, _: *mut u8, _: usize) {}

    pub fn flush(_: *const u8, _: usize) {}
}
