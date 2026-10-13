//! Ribbit's user interface, shared by the desktop app and the Android app:
//! the window, the emulated screen, menus and dialogs (egui), input, sound
//! output, settings and save files. Nothing here changes emulation.

pub mod app;
mod audio_out;
mod files;
#[cfg(not(target_os = "android"))]
mod gamepad;
mod gpu;
mod icon;
pub mod saves;
pub mod settings;
mod ui;

pub use app::{StartOptions, run, run_with};
pub use settings::default_data_dir;
