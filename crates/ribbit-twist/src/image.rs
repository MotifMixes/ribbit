//! Firmware images (the `BinFile` of a system package: Boot, System,
//! MfgTest, Surgeon). VERIFIED on every image in the US 2019 packages:
//!
//! ```text
//! 0x00  u32 magic 0x9ABCDEF0
//! 0x04  u32 1
//! 0x08  u32 0, u32 0
//! 0x10  u32 code size
//! 0x14  u32 check word (meaning UNKNOWN: not a sum, CRC32 or size)
//! 0x18  code, `code size` bytes: ARM code linked at address 0 (it starts
//!       with an ARM vector table of LDR PC entries)
//! ...   4 more bytes (meaning UNKNOWN)
//! ```
//!
//! Paramedic's `NandUsbBin` has no such header: it is bare ARM code.

pub const MAGIC: u32 = 0x9ABC_DEF0;
pub const HEADER_LEN: usize = 0x18;
const TRAILER_LEN: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub version: u32,
    pub code_size: u32,
    pub check: u32,
    pub trailer: u32,
}

fn word(d: &[u8], at: usize) -> Option<u32> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// The header of a firmware image, or None if `data` is not one (wrong
/// magic, or a size that does not match the file).
pub fn parse(data: &[u8]) -> Option<Header> {
    if word(data, 0)? != MAGIC {
        return None;
    }
    let code_size = word(data, 0x10)?;
    if HEADER_LEN + code_size as usize + TRAILER_LEN != data.len() {
        return None;
    }
    Some(Header {
        version: word(data, 4)?,
        code_size,
        check: word(data, 0x14)?,
        trailer: word(data, data.len() - TRAILER_LEN)?,
    })
}

/// The code of a firmware image (loaded at address 0), or the data itself
/// if it has no header (bare code, such as `NandUsbBin`).
pub fn code(data: &[u8]) -> &[u8] {
    match parse(data) {
        Some(h) => &data[HEADER_LEN..HEADER_LEN + h.code_size as usize],
        None => data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(code: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        for w in [MAGIC, 1, 0, 0, code.len() as u32, 0x1234_5678] {
            d.extend_from_slice(&w.to_le_bytes());
        }
        d.extend_from_slice(code);
        d.extend_from_slice(&0xCAFE_F00Du32.to_le_bytes());
        d
    }

    #[test]
    fn headers_are_read_and_checked_against_the_file_size() {
        let code = [0x18, 0xF0, 0x9F, 0xE5, 0, 0, 0, 0];
        let d = image(&code);
        let h = parse(&d).unwrap();
        assert_eq!((h.version, h.code_size, h.check, h.trailer), (1, 8, 0x1234_5678, 0xCAFE_F00D));
        assert_eq!(super::code(&d), code);
        // Cut short: not an image (the code is taken as it is).
        assert_eq!(parse(&d[..d.len() - 1]), None);
        assert_eq!(super::code(&code), code);
    }
}
