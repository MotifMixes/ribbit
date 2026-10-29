//! LeapFrog Didj (2008, codename "Lightning"): an LF1000 (MagicEyes Pollux
//! VR3520F, ARM926EJ-S) running Linux 2.6.20 and LeapFrog's Brio libraries.
//! See docs/didj.md.
//!
//! Boots to Linux user space (no screen yet). This crate holds:
//!
//! * `package`: its software packages (`.lfp` zip archives) and their
//!   `meta.inf`, and the firmware files they carry.
//! * `tfs`: the "table file system" of `kernel.bin` (the boot splash and the
//!   kernel uImage, laid out by erase block).
//! * `nand`: the internal 256 MB NAND: its geometry, partitions, and an
//!   image built from the firmware packages, as LeapFrog Connect installs
//!   them.
//! * `ubi`, `brio`: the Brio partition: UBI holding a FAT16 volume with
//!   the packages that are not raw firmware.
//! * `machine`: the boot machine (ARM926EJ-S, SDRAM, NAND controller,
//!   UARTs, ADC, SPI0); every other register is logged.
//! * `timer`: the timers and the interrupt controller.

pub mod brio;
pub mod machine;
pub mod nand;
pub mod package;
pub mod tfs;
pub mod timer;
pub mod ubi;
