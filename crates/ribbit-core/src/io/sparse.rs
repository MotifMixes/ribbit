//! A save-file format for large storage that is mostly blank (the Leapster 2's
//! SD card and NAND): only the blocks that hold data, by index.
//!
//! Layout (little-endian, Ribbit's own format): the 8-byte magic
//! `RBSPARSE`, block size (u32), block count (u32), then per block its index
//! (u32) and `block size` bytes. A block may appear more than once (`append`);
//! the last copy is its contents.

use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};

const MAGIC: &[u8; 8] = b"RBSPARSE";

/// Encodes the blocks (each `size` bytes).
pub fn encode(blocks: &BTreeMap<u32, Vec<u8>>, size: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(16 + blocks.len() * (4 + size));
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    for (&n, b) in blocks {
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&b[..size]);
    }
    out
}

/// Adds blocks to the end of a file written by `encode` with the same block
/// size, so saving a few changed blocks of a large card does not rewrite all
/// of it. The count is written last: an append cut short leaves the file as
/// it was (`decode` stops at the count, and the next append overwrites the
/// partial tail). Returns the file's block count, copies included.
pub fn append<'a, F: Read + Write + Seek>(
    file: &mut F,
    size: usize,
    blocks: impl IntoIterator<Item = (u32, &'a [u8])>,
) -> std::io::Result<u32> {
    let mut header = [0u8; 16];
    file.seek(SeekFrom::Start(0))?;
    file.read_exact(&mut header)?;
    let u32_at = |o: usize| u32::from_le_bytes(header[o..o + 4].try_into().unwrap());
    if &header[..8] != MAGIC || u32_at(8) as usize != size {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not a sparse image with this block size"));
    }
    let mut count = u32_at(12);
    let mut out = Vec::new();
    for (n, b) in blocks {
        out.extend_from_slice(&n.to_le_bytes());
        out.extend_from_slice(&b[..size]);
        count += 1;
    }
    file.seek(SeekFrom::Start(16 + u32_at(12) as u64 * (4 + size as u64)))?;
    file.write_all(&out)?;
    file.flush()?;
    file.seek(SeekFrom::Start(12))?;
    file.write_all(&count.to_le_bytes())?;
    file.flush()?;
    Ok(count)
}

/// Decodes a file written by `encode` with the same block size; None if it is
/// not one (wrong magic or size, truncated).
pub fn decode(bytes: &[u8], size: usize) -> Option<BTreeMap<u32, Vec<u8>>> {
    let u32_at = |o: usize| bytes.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if bytes.get(..8)? != MAGIC || u32_at(8)? as usize != size {
        return None;
    }
    let count = u32_at(12)? as usize;
    let mut blocks = BTreeMap::new();
    let mut o = 16;
    for _ in 0..count {
        let n = u32_at(o)?;
        blocks.insert(n, bytes.get(o + 4..o + 4 + size)?.to_vec());
        o += 4 + size;
    }
    Some(blocks)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_and_rejects() {
        let mut b = BTreeMap::new();
        b.insert(7, vec![1u8; 4]);
        b.insert(0x10000, vec![2u8; 4]);
        let e = encode(&b, 4);
        assert_eq!(decode(&e, 4), Some(b));
        assert_eq!(decode(&e, 8), None, "other block size");
        assert_eq!(decode(&e[..e.len() - 1], 4), None, "truncated");
        assert_eq!(decode(b"junk", 4), None);
    }

    #[test]
    fn appended_blocks_replace_earlier_copies_and_a_cut_append_is_ignored() {
        let mut b = BTreeMap::new();
        b.insert(7, vec![1u8; 4]);
        let mut f = std::io::Cursor::new(encode(&b, 4));
        assert_eq!(append(&mut f, 4, [(7, &[9u8; 4][..]), (8, &[3u8; 4][..])]).unwrap(), 3);
        b.insert(7, vec![9; 4]);
        b.insert(8, vec![3; 4]);
        assert_eq!(decode(f.get_ref(), 4), Some(b.clone()));
        // A tail left by an append that never got to its count.
        f.get_mut().extend_from_slice(&[0xEE; 5]);
        assert_eq!(decode(f.get_ref(), 4), Some(b.clone()), "partial tail ignored");
        append(&mut f, 4, [(2, &[5u8; 4][..])]).unwrap();
        b.insert(2, vec![5; 4]);
        assert_eq!(decode(f.get_ref(), 4), Some(b));
        assert!(append(&mut std::io::Cursor::new(encode(&BTreeMap::new(), 8)), 4, []).is_err(), "other block size");
    }
}
