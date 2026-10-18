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

#[cfg(windows)]
mod sys {
    use std::io;
    use windows_sys::Win32::System::Diagnostics::Debug::FlushInstructionCache;
    use windows_sys::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_EXECUTE_READ, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        VirtualProtect,
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

    pub fn protect_rx(_: *mut u8, _: usize) -> io::Result<()> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "no executable memory on this platform"))
    }

    pub unsafe fn unmap(_: *mut u8, _: usize) {}
}
