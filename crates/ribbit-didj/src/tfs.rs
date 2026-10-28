//! The "table file system" of `kernel.bin` (lightning-boot README.tfs and
//! tfs.c, opendidj). The first good erase block holds a summary:
//! `0x12345678`, the file count, then per file a 64-byte NUL-padded name and
//! its length in erase blocks (u32, little-endian). The files' blocks follow
//! in order; bad blocks are skipped and not counted. VERIFIED on firmware
//! 1.35.2.4222: bootsplash.rgb (2 blocks), uImage (7 blocks).

use crate::nand::BLOCK;

pub const MAGIC: u32 = 0x1234_5678;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct File {
    pub name: String,
    /// Offset in the image (kernel.bin), in bytes.
    pub offset: usize,
    pub blocks: usize,
}

/// The files in an image with no bad blocks (as packaged).
pub fn files(image: &[u8]) -> Option<Vec<File>> {
    let word = |o: usize| Some(u32::from_le_bytes(image.get(o..o + 4)?.try_into().ok()?));
    if word(0)? != MAGIC {
        return None;
    }
    let count = word(4)? as usize;
    let mut offset = BLOCK;
    let mut files = Vec::new();
    for i in 0..count {
        let e = 8 + i * 68;
        let name = image.get(e..e + 64)?;
        let name = String::from_utf8_lossy(&name[..name.iter().position(|&b| b == 0).unwrap_or(64)]).into_owned();
        let blocks = word(e + 64)? as usize;
        files.push(File { name, offset, blocks });
        offset += blocks * BLOCK;
    }
    Some(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary() {
        let mut img = vec![0xFF; BLOCK * 4];
        img[..8].copy_from_slice(&[0x78, 0x56, 0x34, 0x12, 2, 0, 0, 0]);
        img[8..16].copy_from_slice(b"splash\0\0");
        img[8 + 64..8 + 68].copy_from_slice(&1u32.to_le_bytes());
        img[76..81].copy_from_slice(b"uImg\0");
        img[76 + 64..76 + 68].copy_from_slice(&2u32.to_le_bytes());
        let f = files(&img).unwrap();
        assert_eq!(f[0], File { name: "splash".into(), offset: BLOCK, blocks: 1 });
        assert_eq!(f[1], File { name: "uImg".into(), offset: 2 * BLOCK, blocks: 2 });
        assert!(files(&[0; 8]).is_none());
    }
}
