//! ribbit-gui.exe: the windowed app (no console window on Windows).
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::path::Path;

fn main() {
    let data_dir = ribbit_frontend::default_data_dir();
    // Nobody sees this program's stderr: the log goes to a file as well, for
    // players to send when something goes wrong.
    let log = data_dir.join("ribbit.log");
    ribbit_debug::init_logging_to_file("info", &log);
    tracing::info!("Ribbit {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    {
        let log = log.clone();
        std::panic::set_hook(Box::new(move |info| {
            let backtrace = std::backtrace::Backtrace::force_capture();
            tracing::error!("panic: {info}\n{backtrace}");
            // A panic on the window's thread ends Ribbit (others are reported
            // where they are caught).
            if std::thread::current().name() == Some("main") {
                failed(&format!("Ribbit stopped because of a bug:\n\n{info}"), &log);
            }
        }));
    }
    #[cfg(windows)]
    log_native_crashes(log.clone());
    remove_old_programs();
    // WSL's Wayland server drops the window's connection after a moment
    // (its X11 server works): use X11 there unless RIBBIT_WAYLAND is set.
    #[cfg(target_os = "linux")]
    if std::env::var_os("RIBBIT_WAYLAND").is_none()
        && std::env::var_os("DISPLAY").is_some()
        && std::fs::read_to_string("/proc/sys/kernel/osrelease").is_ok_and(|r| r.to_lowercase().contains("microsoft"))
    {
        // SAFETY: nothing else runs yet (no other threads read the environment).
        unsafe { std::env::remove_var("WAYLAND_DISPLAY") };
    }
    // A file passed on the command line (e.g. "Open with Ribbit") is a cartridge.
    let cartridge = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let opts = ribbit_frontend::StartOptions { data_dir, cartridge, ..Default::default() };
    if let Err(e) = ribbit_frontend::run(opts) {
        tracing::error!("Ribbit could not start: {e:#}");
        failed(&format!("Ribbit could not start:\n\n{e:#}"), &log);
    }
}

/// Up to 1.2.1 a release held two programs, ribbit-gui (the app) and ribbit
/// (the command-line tool); now it holds the app alone, as Ribbit.exe or
/// ribbit. Unpacked over an older release, the app replaces the old ribbit
/// (Windows names ignore case) and removes the old ribbit-gui. A build from
/// source keeps the name ribbit-gui, so it never removes itself.
fn remove_old_programs() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let old = format!("ribbit-gui{}", std::env::consts::EXE_SUFFIX);
    if exe.file_name().is_none_or(|n| n.eq_ignore_ascii_case(&old)) {
        return;
    }
    let Some(old) = exe.parent().map(|d| d.join(&old)).filter(|p| p.is_file()) else {
        return;
    };
    match std::fs::remove_file(&old) {
        Ok(()) => tracing::info!("removed the old {}", old.display()),
        Err(e) => tracing::warn!("could not remove the old {}: {e}", old.display()),
    }
}

/// Says why Ribbit stopped: stderr, `last-error.txt` next to the log, and on
/// Windows a message box (the window may never have drawn anything).
fn failed(msg: &str, log: &Path) {
    eprintln!("{msg}");
    let msg = format!("{msg}\n\nThe log, to send with a bug report:\n{}", log.display());
    if let Some(dir) = log.parent() {
        let _ = std::fs::write(dir.join("last-error.txt"), &msg);
    }
    #[cfg(windows)]
    message_box(&msg);
}

/// A crash outside Rust (an access violation in a graphics driver, say) ends
/// the process without a panic, so the log just stops (a player's 1.4.3 log
/// ended after "audio output open"). This logs the exception and the DLL it
/// happened in, then lets Windows end the process as before.
#[cfg(windows)]
fn log_native_crashes(log: std::path::PathBuf) {
    use std::ffi::c_void;
    #[repr(C)]
    struct ExceptionRecord {
        code: u32,
        flags: u32,
        next: *mut ExceptionRecord,
        address: *mut c_void,
    }
    #[repr(C)]
    struct ExceptionPointers {
        record: *mut ExceptionRecord,
        context: *mut c_void,
    }
    type Filter = unsafe extern "system" fn(*const ExceptionPointers) -> i32;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetUnhandledExceptionFilter(filter: Option<Filter>) -> Option<Filter>;
        fn GetModuleHandleExW(flags: u32, address: *const c_void, module: *mut *mut c_void) -> i32;
        fn GetModuleFileNameW(module: *mut c_void, name: *mut u16, size: u32) -> u32;
    }
    const FROM_ADDRESS: u32 = 0x4;
    const UNCHANGED_REFCOUNT: u32 = 0x2;
    const CONTINUE_SEARCH: i32 = 0;
    static LOG: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

    unsafe extern "system" fn filter(info: *const ExceptionPointers) -> i32 {
        // SAFETY: Windows passes valid exception pointers to the filter.
        let Some(record) = (unsafe { info.as_ref() }).and_then(|i| unsafe { i.record.as_ref() }) else {
            return CONTINUE_SEARCH;
        };
        let mut module = std::ptr::null_mut();
        let mut name = [0u16; 260];
        // SAFETY: `module` and `name` are valid for the calls' writes.
        let len = unsafe {
            if GetModuleHandleExW(FROM_ADDRESS | UNCHANGED_REFCOUNT, record.address, &mut module) == 0 {
                0
            } else {
                GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32)
            }
        };
        let place = match len {
            0 => "an unknown module".to_string(),
            n => format!(
                "{} + {:#x}",
                String::from_utf16_lossy(&name[..n as usize]),
                record.address as usize - module as usize
            ),
        };
        let msg = format!(
            "Ribbit crashed: exception {:#010x} at {:#x} in {place} (thread {:?})",
            record.code,
            record.address as usize,
            std::thread::current().name().unwrap_or("unnamed"),
        );
        tracing::error!("{msg}");
        if let Some(dir) = LOG.get().and_then(|l| l.parent()) {
            let _ = std::fs::write(dir.join("last-error.txt"), &msg);
        }
        CONTINUE_SEARCH
    }

    let _ = LOG.set(log);
    // SAFETY: `filter` has the signature Windows expects and lives forever.
    unsafe { SetUnhandledExceptionFilter(Some(filter)) };
}

#[cfg(windows)]
fn message_box(msg: &str) {
    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(hwnd: *mut std::ffi::c_void, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }
    const MB_ICONERROR: u32 = 0x10;
    let wide = |s: &str| s.encode_utf16().chain([0]).collect::<Vec<u16>>();
    let (text, caption) = (wide(msg), wide("Ribbit"));
    // SAFETY: both strings are NUL-terminated and outlive the call.
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), MB_ICONERROR) };
}
