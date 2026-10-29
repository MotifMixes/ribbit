//! LeapFrog RockIt Twist (2019, model 6060, firmware codename "PocketPlay").
//!
//! EARLY EMULATION: the machine boots the console's own firmware to its
//! "Device Error" screen; no game runs. This crate holds what has been
//! verified about the console so far, in code that the frontend and
//! research tools share:
//!
//! * `input`: the console's physical controls, as named by its own factory
//!   test firmware (MfgTest) and its Parent's Guide, and their state.
//! * `image`: the header of its firmware images (Boot, System, MfgTest...).
//! * `package`: its software packages (`.lfp` system packages, encrypted
//!   `.lf3` game packs) and their `meta.inf`.
//! * `machine`: the console being built up device by device (runs the base
//!   firmware to its "Device Error" screen; see docs/rockit-twist.md).
//! * `sd`, `fat`: its eMMC chip and its FAT32 volume.
//! * `probe`: runs a firmware image on the ARM9 model (`ribbit-arm`) and
//!   records the hardware it touches, to map the system-on-chip.
//!
//! See docs/rockit-twist.md for the evidence and the open questions.

pub mod content;
pub mod disk;
pub use ribbit_core::fatfs as fat;
pub mod image;
pub mod input;
pub mod machine;
pub mod mp3acc;
pub mod mpeg4;
mod mpeg4_tables;
pub mod package;
pub mod probe;
pub mod sd;
pub mod sound;
pub mod udc;
pub mod usbip;
pub mod video;

/// Where the emulation stands, for the UI and documentation.
pub const STATUS: &str = "Early emulation: the console's own firmware starts and shows its Device Error screen      (the menu LeapFrog Connect installs is missing). No game runs.";

/// The screen in landscape, in pixels. HYPOTHESIS: the factory test names
/// the panel's controller as an ST7789V or an ILI9340X, both 240 x 320
/// controllers, and the Parent's Guide shows the screen wider than tall.
/// Not confirmed from the firmware's video setup yet.
pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 240;
