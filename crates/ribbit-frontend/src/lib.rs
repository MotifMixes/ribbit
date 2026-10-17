//! Ribbit's user interface, shared by the desktop app and the Android app:
//! the window, the emulated screen, menus and dialogs (egui), input, sound
//! output, settings and save files. Nothing here changes emulation.

pub mod app;
mod audio_out;
mod calibrate;
mod console_view;
mod device_fonts;
mod discord;
mod emu;
mod files;
#[cfg(not(target_os = "android"))]
mod gamepad;
mod fonts;
mod gpu;
mod held;
mod icon;
mod library;
mod perf;
pub mod saves;
pub mod settings;
mod ui;
mod touch_pad;
mod twist;
mod updates;
pub mod upscale;
mod zippity;

pub use app::{StartOptions, run, run_with};
pub use settings::default_data_dir;
