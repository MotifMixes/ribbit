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
    #[error("'{name}' is {hardware:?} firmware; Ribbit only targets the original Leapster")]
    WrongHardware { name: &'static str, hardware: Hardware },
}

#[derive(Debug, Clone)]
pub struct Firmware {
    data: Vec<u8>,
    id: ImageId,
    known: Option<&'static KnownImage>,
}

impl Firmware {
    /// Validates a firmware image. Unknown dumps are accepted (there may be
    /// undumped revisions) but reported as unverified by `known()`.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FirmwareError> {
        let fw = Self::from_bytes_any_model(data)?;
        if let Some(k) = fw.known
            && k.hardware != Hardware::Leapster
        {
            return Err(FirmwareError::WrongHardware { name: k.name, hardware: k.hardware });
        }
        Ok(fw)
    }

    /// EXPERIMENTAL: also accepts the BIOS of the Leapster TV, L-MAX and
    /// Leapster 2, which run on the original's emulated hardware (as in MAME,
    /// which marks them all not working). For research; not for players.
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
