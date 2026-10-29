//! Ribbit's user interface, shared by the desktop app and the Android app:
//! the window, the emulated screen, menus and dialogs (egui), input, sound
//! output, settings and save files. Nothing here changes emulation.

pub mod app;
mod audio_out;
mod backup;
mod calibrate;
mod debugger;
mod cheat;
mod record;
mod console_view;
mod device_fonts;
mod didj;
mod discord;
mod emu;
mod files;
#[cfg(not(target_os = "android"))]
mod gamepad;
mod game_settings;
mod fonts;
mod gpu;
mod held;
mod icon;
mod install;
mod leappad;
#[cfg(not(target_os = "android"))]
mod leappad_words;
/// The LeapPad words' helper process (leappad_words.rs).
#[cfg(not(target_os = "android"))]
pub use leappad_words::{WORKER_ARG, worker_main};
mod library;
mod manual;
mod perf;
pub mod saves;
pub mod sdcard;
pub mod settings;
mod ui;
mod touch_pad;
mod twist;
mod updates;
pub mod upscale;
mod zippity;

pub use app::{StartOptions, run, run_with};
pub use settings::default_data_dir;
