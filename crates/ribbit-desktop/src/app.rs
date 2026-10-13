//! ribbit-gui.exe: the windowed app (no console window on Windows).
#![cfg_attr(windows, windows_subsystem = "windows")]

fn main() {
    ribbit_debug::init_logging("info");
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
