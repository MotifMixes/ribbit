//! Content identification for user-supplied images.

use sha1::{Digest, Sha1};
use std::fmt;

/// Size and checksums of an image. Computed locally; never transmitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageId {
    pub size: usize,
    pub crc32: u32,
    pub sha1: [u8; 20],
}

impl ImageId {
    pub fn of(data: &[u8]) -> Self {
        let sha1: [u8; 20] = Sha1::digest(data).into();
        Self { size: data.len(), crc32: crc32fast::hash(data), sha1 }
    }

    pub fn sha1_hex(&self) -> String {
        self.sha1.iter().map(|b| format!("{b:02x}")).collect()
    }
}

impl fmt::Display for ImageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "size={:#x} crc32={:08x} sha1={}", self.size, self.crc32, self.sha1_hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_vectors() {
        // CRC-32/IEEE and SHA-1 of "abc".
        let id = ImageId::of(b"abc");
        assert_eq!(id.size, 3);
        assert_eq!(id.crc32, 0x3524_41c2);
        assert_eq!(id.sha1_hex(), "a9993e364706816aba3e25717850c26c9cd0d89d");
    }
}
