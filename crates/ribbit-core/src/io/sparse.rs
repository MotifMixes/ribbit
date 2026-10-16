//! A save-file format for large storage that is mostly blank (the Leapster 2's
//! SD card and NAND): only the blocks that hold data, by index.
//!
//! Layout (little-endian, Ribbit's own format): the 8-byte magic
//! `RBSPARSE`, block size (u32), block count (u32), then per block its index
//! (u32) and `block size` bytes.

use std::collections::BTreeMap;

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
}
