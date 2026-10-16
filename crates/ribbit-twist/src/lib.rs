//! LeapFrog RockIt Twist (2019, model 6060, firmware codename "PocketPlay").
//!
//! NOT EMULATED YET. This crate holds what has been verified about the
//! console so far, in code that the frontend and research tools share:
//!
//! * `input`: the console's physical controls, as named by its own factory
//!   test firmware (MfgTest) and its Parent's Guide, and their state.
//! * `image`: the header of its firmware images (Boot, System, MfgTest...).
//! * `package`: its software packages (`.lfp` system packages, encrypted
//!   `.lf3` game packs) and their `meta.inf`.
//! * `probe`: runs a firmware image on the ARM9 model (`ribbit-arm`) and
//!   records the hardware it touches, to map the system-on-chip.
//!
//! See docs/rockit-twist.md for the evidence and the open questions.

pub mod image;
pub mod input;
pub mod package;
pub mod probe;

/// Where the emulation stands, for the UI and documentation.
pub const STATUS: &str = "Research preview: controls and file formats only. The console is not emulated yet.";

/// The screen in landscape, in pixels. HYPOTHESIS: the factory test names
/// the panel's controller as an ST7789V or an ILI9340X, both 240 x 320
/// controllers, and the Parent's Guide shows the screen wider than tall.
/// Not confirmed from the firmware's video setup yet.
pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 240;
