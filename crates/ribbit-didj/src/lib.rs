//! LeapFrog Didj (2008, codename "Lightning"): an LF1000 (MagicEyes Pollux
//! VR3520F, ARM926EJ-S) running Linux 2.6.20 and LeapFrog's Brio libraries.
//! See docs/didj.md.
//!
//! NOT EMULATED YET. This crate holds what has been verified about the
//! console so far:
//!
//! * `package`: its software packages (`.lfp` zip archives) and their
//!   `meta.inf`, and the firmware files they carry.
//! * `tfs`: the "table file system" of `kernel.bin` (the boot splash and the
//!   kernel uImage, laid out by erase block).
//! * `nand`: the internal 256 MB NAND: its geometry, partitions, and an
//!   image built from the firmware packages, as LeapFrog Connect installs
//!   them.

pub mod nand;
pub mod package;
pub mod tfs;
