//! System firmware ("BaseROM") image. The user supplies their own dump.

use crate::image::ImageId;
use crate::known::{self, Hardware, KnownImage};
use crate::map;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum FirmwareError {
    #[error("firmware image is empty")]
    Empty,
    #[error("firmware image is {size:#x} bytes; it must fit the {max:#x}-byte BIOS window")]
    TooLarge { size: usize, max: usize },
    #[error("firmware size {0:#x} is not a multiple of 4 bytes")]
    Misaligned(usize),
}

#[derive(Debug, Clone)]
pub struct Firmware {
    data: Vec<u8>,
    id: ImageId,
    known: Option<&'static KnownImage>,
}

impl Firmware {
    /// Validates a firmware image. Unknown dumps are accepted (there may be
    /// undumped revisions) but reported as unverified by `known()`. Besides
    /// the original Leapster, the Leapster TV, L-MAX and Leapster 2 BIOSes
    /// are accepted (EXPERIMENTAL: `is_experimental_model`).
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FirmwareError> {
        Self::from_bytes_any_model(data)
    }

    /// A known BIOS of a model other than the original Leapster, whose
    /// emulation is EXPERIMENTAL.
    pub fn is_experimental_model(&self) -> bool {
        self.known.is_some_and(|k| k.hardware != Hardware::Leapster)
    }

    /// Any model's BIOS (now the same as `from_bytes`; kept for the research
    /// tools that name it).
    pub fn from_bytes_any_model(data: Vec<u8>) -> Result<Self, FirmwareError> {
        if data.is_empty() {
            return Err(FirmwareError::Empty);
        }
        if data.len() > map::BIOS_WINDOW as usize {
            return Err(FirmwareError::TooLarge { size: data.len(), max: map::BIOS_WINDOW as usize });
        }
        if !data.len().is_multiple_of(4) {
            return Err(FirmwareError::Misaligned(data.len()));
        }
        let id = ImageId::of(&data);
        let known = known::lookup(known::FIRMWARE, &id);
        Ok(Self { data, id, known })
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn id(&self) -> &ImageId {
        &self.id
    }

    pub fn known(&self) -> Option<&'static KnownImage> {
        self.known
    }

    /// SDRAM size for this firmware's model (`map::RAM_SIZE`; 16 MiB for a
    /// known Leapster 2 BIOS). Unknown dumps get the original's 2 MiB.
    pub fn ram_size(&self) -> u32 {
        match self.known.map(|k| k.hardware) {
            Some(Hardware::Leapster2) => map::RAM_SIZE_LEAPSTER2,
            _ => map::RAM_SIZE,
        }
    }

    /// On-chip RAM at `map::SRAM_BASE` for this firmware's model (8 KiB on
    /// a known Leapster 2; none elsewhere).
    pub fn sram_size(&self) -> u32 {
        match self.known.map(|k| k.hardware) {
            Some(Hardware::Leapster2) => map::SRAM_SIZE_LEAPSTER2,
            _ => 0,
        }
    }

    /// A Leapster 2 "CIP3" board BIOS (m9, `152-12659`): its board has a NAND
    /// flash at 0xE000_0000 and no SD card slot. VERIFIED-FW: m9 carries a
    /// NAND driver and no SD driver; 0.6 and Connected B5 the reverse.
    pub fn leapster2_nand(&self) -> bool {
        self.known.is_some_and(|k| k.sha1 == "4b5eaac1e40bc605eb4cf6d4ad212343334762fd")
    }

    /// Leapster 2: where in its internal SPI flash the BIOS keeps the touch
    /// calibration. VERIFIED by experiment (2026-10-16): 0.6 and m9 write
    /// the record at 0x1000, Connected B5 the identical record at 0x0000
    /// (each BIOS's storage-region table differs). Unknown dumps: 0x1000.
    pub fn touch_calibration_offset(&self) -> usize {
        match self.known.map(|k| k.sha1) {
            Some("bdc10ad70aa7641716e16fbea16bd0ef35f6e85e") => 0x0000,
            _ => 0x1000,
        }
    }

    /// A known Leapster TV BIOS: the machine needs the TV video chip and
    /// controller link (EXPERIMENTAL).
    pub fn is_leapster_tv(&self) -> bool {
        self.known.is_some_and(|k| k.hardware == Hardware::LeapsterTv)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_empty_oversized_and_misaligned() {
        assert!(matches!(Firmware::from_bytes(vec![]), Err(FirmwareError::Empty)));
        assert!(matches!(
            Firmware::from_bytes(vec![0; map::BIOS_WINDOW as usize + 4]),
            Err(FirmwareError::TooLarge { .. })
        ));
        assert!(matches!(Firmware::from_bytes(vec![0; 6]), Err(FirmwareError::Misaligned(6))));
    }

    #[test]
    fn unknown_firmware_is_accepted_but_unverified() {
        let fw = Firmware::from_bytes(vec![0xAA; 0x1000]).unwrap();
        assert!(fw.known().is_none());
        assert_eq!(fw.id().size, 0x1000);
    }
}
