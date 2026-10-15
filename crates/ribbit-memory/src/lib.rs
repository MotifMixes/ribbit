//! Memory map, firmware and cartridge images, and the system bus for the
//! original (2003) LeapFrog Leapster.
//!
//! Every address and size here is sourced in `docs/hardware-research.md`.
//! Values marked HYPOTHESIS are unverified against real hardware.

pub mod bus;
pub mod cartridge;
pub mod firmware;
pub mod image;
pub mod known;
mod known_carts;
pub mod map;
pub mod serde_array;

pub use bus::{Bus, SystemBus, UnmappedAccess};
pub use cartridge::{CartHeader, Cartridge, CartridgeError, Nvram};
pub use firmware::{Firmware, FirmwareError};
pub use image::ImageId;
