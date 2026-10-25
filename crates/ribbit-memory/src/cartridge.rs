//! Cartridge ROM images and the cartridge header directory.
//!
//! Header layout is INFERRED from one retail image (SpongeBob, 500-11002)
//! and is self-consistent there; it has not been checked against other
//! titles or any LeapFrog documentation. See docs/hardware-research.md
//! §Cartridge image layout.
//!
//! ```text
//! 0x00  "LEAP"                 magic
//! 0x04  u16  (0x0100 seen)     unknown, possibly a format version
//! 0x06  u16                    number of top-level directory entries
//! 0x20  [tag:u16 count:u16 ptr:u32] x N   top-level entries
//!       ptr -> count x [id:u16 flags:u16 value:u32]   sub-entries
//! ```
//! All multi-byte fields are little-endian; pointers are CPU addresses in
//! the 0x8000_0000 cartridge window.
//!
//! Most carts start with code instead and keep the header elsewhere, its
//! address at 0x140 (0x8000_0144 on Wall-E, Sonic X and Counting on Zero:
//! "LEAP" there, then the same layout; a player's sweep of 272 dumps,
//! 2026-10-25).

use crate::image::ImageId;
use crate::known::{self, KnownImage};
use crate::map;
use thiserror::Error;

pub const CART_MAGIC: &[u8; 4] = b"LEAP";
const DIRECTORY_OFFSET: usize = 0x20;
/// Where carts that do not start with the header keep its address.
const HEADER_POINTER_OFFSET: usize = 0x140;

#[derive(Debug, Error)]
pub enum CartridgeError {
    #[error("cartridge image is empty")]
    Empty,
    #[error("cartridge image is {size:#x} bytes; the cartridge window is {max:#x} bytes")]
    TooLarge { size: usize, max: usize },
    #[error("cartridge size {0:#x} is odd; the cartridge bus is 16 bits wide")]
    OddSize(usize),
}

#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum HeaderError {
    #[error("missing \"LEAP\" magic at offset 0")]
    BadMagic,
    #[error("header field at {offset:#x} lies outside the image")]
    Truncated { offset: usize },
    #[error("directory pointer {ptr:#010x} does not point into the cartridge image")]
    BadPointer { ptr: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubEntry {
    pub id: u16,
    pub flags: u16,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DirEntry {
    pub tag: u16,
    pub ptr: u32,
    pub entries: Vec<SubEntry>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CartHeader {
    pub field_04: u16,
    pub directory: Vec<DirEntry>,
}

/// An uncompressed Flash movie ("FWS" signature) found inside the image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwfMovie {
    pub offset: usize,
    pub version: u8,
    pub length: u32,
}

#[derive(Debug, Clone)]
pub struct Cartridge {
    rom: Vec<u8>,
    id: ImageId,
    known: Option<&'static KnownImage>,
    header: Result<CartHeader, HeaderError>,
}

impl Cartridge {
    pub fn from_bytes(rom: Vec<u8>) -> Result<Self, CartridgeError> {
        if rom.is_empty() {
            return Err(CartridgeError::Empty);
        }
        if rom.len() > map::CART_WINDOW as usize {
            return Err(CartridgeError::TooLarge { size: rom.len(), max: map::CART_WINDOW as usize });
        }
        if !rom.len().is_multiple_of(2) {
            return Err(CartridgeError::OddSize(rom.len()));
        }
        let id = ImageId::of(&rom);
        let known = known::lookup(known::CARTRIDGES, &id);
        let header = parse_header(&rom);
        Ok(Self { rom, id, known, header })
    }

    pub fn rom(&self) -> &[u8] {
        &self.rom
    }

    pub fn id(&self) -> &ImageId {
        &self.id
    }

    pub fn known(&self) -> Option<&'static KnownImage> {
        self.known
    }

    /// The header is parsed best-effort: a bad header does not prevent
    /// loading, because the firmware, not Ribbit, decides what is valid.
    pub fn header(&self) -> Result<&CartHeader, &HeaderError> {
        self.header.as_ref()
    }

    /// A text field of the header's 0x1003 directory, if present and
    /// printable. Seen on SpongeBob: 0x0a part number ("152-01508"), 0x0b
    /// title ("SpongeBob SquarePants"), 0x0f build date ("Oct 09 2003 18:23:53").
    /// INFERRED from one title.
    pub fn header_text(&self, id: u16) -> Option<String> {
        let dir = self.header.as_ref().ok()?.directory.iter().find(|d| d.tag == 0x1003)?;
        let ptr = dir.entries.iter().find(|e| e.id == id)?.value;
        let at = cart_offset(&self.rom, ptr).ok()?;
        let bytes = &self.rom[at..self.rom.len().min(at + 128)];
        let text = &bytes[..bytes.iter().position(|&b| b == 0)?];
        let ok = !text.is_empty() && text.iter().all(|&b| (0x20..0x7F).contains(&b));
        ok.then(|| String::from_utf8_lossy(text).trim().to_string())
    }
}

fn u16_at(data: &[u8], offset: usize) -> Result<u16, HeaderError> {
    data.get(offset..offset + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(HeaderError::Truncated { offset })
}

fn u32_at(data: &[u8], offset: usize) -> Result<u32, HeaderError> {
    data.get(offset..offset + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .ok_or(HeaderError::Truncated { offset })
}

fn cart_offset(data: &[u8], ptr: u32) -> Result<usize, HeaderError> {
    let offset = ptr.wrapping_sub(map::CART_BASE) as usize;
    if ptr < map::CART_BASE || offset >= data.len() {
        return Err(HeaderError::BadPointer { ptr });
    }
    Ok(offset)
}

pub fn parse_header(data: &[u8]) -> Result<CartHeader, HeaderError> {
    let magic_at = |o: usize| data.get(o..o + 4) == Some(CART_MAGIC.as_slice());
    let base = if magic_at(0) {
        0
    } else {
        u32_at(data, HEADER_POINTER_OFFSET)
            .ok()
            .and_then(|ptr| cart_offset(data, ptr).ok())
            .filter(|&o| magic_at(o))
            .ok_or(HeaderError::BadMagic)?
    };
    let field_04 = u16_at(data, base + 4)?;
    let count = u16_at(data, base + 6)? as usize;
    let mut directory = Vec::with_capacity(count);
    for i in 0..count {
        let at = base + DIRECTORY_OFFSET + 8 * i;
        let tag = u16_at(data, at)?;
        let sub_count = u16_at(data, at + 2)? as usize;
        let ptr = u32_at(data, at + 4)?;
        let base = cart_offset(data, ptr)?;
        let entries = (0..sub_count)
            .map(|j| {
                let at = base + 8 * j;
                Ok(SubEntry { id: u16_at(data, at)?, flags: u16_at(data, at + 2)?, value: u32_at(data, at + 4)? })
            })
            .collect::<Result<_, HeaderError>>()?;
        directory.push(DirEntry { tag, ptr, entries });
    }
    Ok(CartHeader { field_04, directory })
}

/// Finds plausible uncompressed SWF headers ("FWS", version 1-10, length
/// that fits in the image). Analysis aid only; the emulator never uses it.
pub fn scan_swf(data: &[u8]) -> Vec<SwfMovie> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 8 <= data.len() {
        if &data[i..i + 3] == b"FWS" {
            let version = data[i + 3];
            let length = u32::from_le_bytes([data[i + 4], data[i + 5], data[i + 6], data[i + 7]]);
            if (1..=10).contains(&version) && length > 21 && (length as usize) <= data.len() - i {
                found.push(SwfMovie { offset: i, version, length });
                i += length as usize;
                continue;
            }
        }
        i += 1;
    }
    found
}

/// Cartridge-side non-volatile storage. The cart PCB carries a 24LC02B
/// (256 bytes) footprint while MAME allocates 0x800 bytes; the true size and
/// access protocol are OPEN QUESTIONS, so the size is a parameter.
#[derive(Debug, Clone)]
pub struct Nvram {
    data: Vec<u8>,
    dirty: bool,
}

impl Nvram {
    /// Erased EEPROM reads as 0xFF on real 24LCxx parts. HYPOTHESIS for
    /// Leapster carts, which may ship pre-initialised.
    pub fn blank(size: usize) -> Self {
        Self { data: vec![0xFF; size], dirty: false }
    }

    pub fn from_bytes(data: Vec<u8>) -> Self {
        Self { data, dirty: false }
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }

    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    pub fn read(&self, addr: usize) -> u8 {
        self.data[addr % self.data.len()]
    }

    pub fn write(&mut self, addr: usize, value: u8) {
        let len = self.data.len();
        self.data[addr % len] = value;
        self.dirty = true;
    }

    pub fn bytes(&self) -> &[u8] {
        &self.data
    }

    pub fn take_dirty(&mut self) -> bool {
        std::mem::take(&mut self.dirty)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal synthetic image with the inferred header layout.
    fn synthetic_cart() -> Vec<u8> {
        let mut rom = vec![0u8; 0x1000];
        rom[0..4].copy_from_slice(CART_MAGIC);
        rom[4..6].copy_from_slice(&0x0100u16.to_le_bytes());
        rom[6..8].copy_from_slice(&1u16.to_le_bytes());
        // One top-level entry: tag 0x1003, 2 sub-entries at 0x8000_0040.
        rom[0x20..0x22].copy_from_slice(&0x1003u16.to_le_bytes());
        rom[0x22..0x24].copy_from_slice(&2u16.to_le_bytes());
        rom[0x24..0x28].copy_from_slice(&0x8000_0040u32.to_le_bytes());
        rom[0x40..0x48].copy_from_slice(&[1, 0, 0, 1, 0x00, 0x08, 0x00, 0x80]);
        rom[0x48..0x50].copy_from_slice(&[2, 0, 0, 1, 0x10, 0x00, 0x00, 0x00]);
        rom
    }

    #[test]
    fn header_found_through_the_pointer_at_0x140() {
        // The same header moved to 0x200, its address at 0x140.
        let head = synthetic_cart();
        let mut rom = vec![0u8; 0x1000];
        rom[0x200..0x228].copy_from_slice(&head[..0x28]);
        rom[0x40..0x50].copy_from_slice(&head[0x40..0x50]);
        rom[0x140..0x144].copy_from_slice(&0x8000_0200u32.to_le_bytes());
        let cart = Cartridge::from_bytes(rom.clone()).unwrap();
        assert_eq!(cart.header().unwrap().directory[0].entries[1], SubEntry { id: 2, flags: 0x100, value: 0x10 });
        // A pointer to something else is no header.
        rom[0x200] = b'X';
        assert_eq!(Cartridge::from_bytes(rom).unwrap().header().unwrap_err(), &HeaderError::BadMagic);
    }

    #[test]
    fn parses_inferred_header_layout() {
        let cart = Cartridge::from_bytes(synthetic_cart()).unwrap();
        let header = cart.header().unwrap();
        assert_eq!(header.field_04, 0x0100);
        assert_eq!(header.directory.len(), 1);
        let dir = &header.directory[0];
        assert_eq!(dir.tag, 0x1003);
        assert_eq!(dir.entries[0], SubEntry { id: 1, flags: 0x100, value: 0x8000_0800 });
        assert_eq!(dir.entries[1], SubEntry { id: 2, flags: 0x100, value: 0x10 });
    }

    #[test]
    fn header_text_reads_strings() {
        let mut rom = synthetic_cart();
        rom[0x800..0x806].copy_from_slice(b"Title\0");
        let cart = Cartridge::from_bytes(rom.clone()).unwrap();
        assert_eq!(cart.header_text(1).as_deref(), Some("Title"));
        // Not a pointer into the image, or no such id.
        assert_eq!(cart.header_text(2), None);
        assert_eq!(cart.header_text(3), None);
        // Unprintable bytes are not text.
        rom[0x800] = 0x01;
        assert_eq!(Cartridge::from_bytes(rom).unwrap().header_text(1), None);
    }

    #[test]
    fn bad_header_does_not_block_loading() {
        let cart = Cartridge::from_bytes(vec![0u8; 0x100]).unwrap();
        assert_eq!(cart.header().unwrap_err(), &HeaderError::BadMagic);

        let mut rom = synthetic_cart();
        rom[0x24..0x28].copy_from_slice(&0x9000_0000u32.to_le_bytes());
        let cart = Cartridge::from_bytes(rom).unwrap();
        assert_eq!(cart.header().unwrap_err(), &HeaderError::BadPointer { ptr: 0x9000_0000 });
    }

    #[test]
    fn rejects_invalid_sizes() {
        assert!(matches!(Cartridge::from_bytes(vec![]), Err(CartridgeError::Empty)));
        assert!(matches!(Cartridge::from_bytes(vec![0; 3]), Err(CartridgeError::OddSize(3))));
        assert!(matches!(
            Cartridge::from_bytes(vec![0; map::CART_WINDOW as usize + 2]),
            Err(CartridgeError::TooLarge { .. })
        ));
    }

    #[test]
    fn swf_scan_finds_movies_and_skips_noise() {
        let mut data = vec![0u8; 256];
        data[10..13].copy_from_slice(b"FWS"); // version 0: rejected
        data[40..43].copy_from_slice(b"FWS");
        data[43] = 5;
        data[44..48].copy_from_slice(&100u32.to_le_bytes());
        assert_eq!(scan_swf(&data), vec![SwfMovie { offset: 40, version: 5, length: 100 }]);
    }

    #[test]
    fn nvram_tracks_writes() {
        let mut nv = Nvram::blank(256);
        assert_eq!(nv.read(5), 0xFF);
        assert!(!nv.take_dirty());
        nv.write(5, 0x12);
        assert_eq!(nv.read(5), 0x12);
        assert!(nv.take_dirty());
        assert!(!nv.take_dirty());
    }
}
