//! ribbit-gui.exe: the windowed app (no console window on Windows).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    ribbit_debug::init_logging("info");
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
    let data_dir = ribbit_frontend::default_data_dir();
    // A file passed on the command line (e.g. "Open with Ribbit") is a cartridge.
    let cartridge = std::env::args_os().nth(1).map(std::path::PathBuf::from);
    let opts = ribbit_frontend::StartOptions { data_dir, cartridge, ..Default::default() };
    if let Err(e) = ribbit_frontend::run(opts) {
        let msg = format!(
            "Ribbit could not start:

{e:#}"
        );
        eprintln!("{msg}");
        #[cfg(windows)]
        let _ = rfd_message(&msg);
    }
}

#[cfg(windows)]
fn rfd_message(msg: &str) -> Option<()> {
    std::fs::write(ribbit_frontend::default_data_dir().join("last-error.txt"), msg).ok()
}
