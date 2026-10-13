//! Decoded-instruction cache for read-only code.
//!
//! Decoding is a pure function of the bytes at the PC, so for memory that
//! cannot change (the machine declares ROM ranges: BIOS, cartridge) a decoded
//! instruction can be reused. Anything outside the declared ranges (RAM,
//! VRAM) is decoded on every fetch. The cache is not architectural state: it
//! never changes behaviour, only speed, and compares equal regardless of
//! contents.

use crate::isa::Instr;

const SETS: usize = 1 << 16;
/// Longest instruction (32-bit word + long immediate).
const MAX_LEN: u32 = 8;

#[derive(Clone, Default)]
pub struct ICache {
    /// Half-open address ranges whose contents never change.
    ranges: Vec<(u32, u32)>,
    /// Per 64 KiB page: every instruction starting in it lies inside one range.
    pages: Vec<bool>,
    entries: Vec<Option<Instr>>,
    pub hits: u64,
    pub misses: u64,
}

impl PartialEq for ICache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for ICache {}

impl std::fmt::Debug for ICache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ICache {{ ranges: {:x?}, hits: {}, misses: {} }}", self.ranges, self.hits, self.misses)
    }
}

impl ICache {
    /// Declares read-only ranges (start, end exclusive) and clears the cache.
    pub fn set_rom_ranges(&mut self, ranges: &[(u32, u32)]) {
        self.ranges = ranges.to_vec();
        self.entries = if ranges.is_empty() { Vec::new() } else { vec![None; SETS] };
        self.pages = (0..1u32 << 16)
            .map(|p| {
                let (start, last) = (p << 16, (p << 16) | 0xFFFF);
                self.ranges.iter().any(|&(s, e)| start >= s && last.checked_add(MAX_LEN).is_some_and(|end| end <= e))
            })
            .collect();
    }

    pub fn clear(&mut self) {
        self.entries.iter_mut().for_each(|e| *e = None);
    }

    fn cacheable(&self, pc: u32) -> bool {
        if self.pages[(pc >> 16) as usize] {
            return true;
        }
        self.ranges.iter().any(|&(s, e)| pc >= s && pc.checked_add(MAX_LEN).is_some_and(|end| end <= e))
    }

    /// The cached decode of `pc`, decoding with `decode` on a miss.
    #[inline]
    pub fn get(&mut self, pc: u32, decode: impl FnOnce() -> Instr) -> Instr {
        if self.entries.is_empty() || !self.cacheable(pc) {
            return decode();
        }
        let slot = &mut self.entries[(pc as usize >> 1) & (SETS - 1)];
        match slot {
            Some(i) if i.addr == pc => {
                self.hits += 1;
                *i
            }
            _ => {
                self.misses += 1;
                let i = decode();
                *slot = Some(i);
                i
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_with;

    fn rom(addr: u32) -> u16 {
        // mov_s r0,0x1 repeated (0xd801), any even address.
        let _ = addr;
        0xd801
    }

    #[test]
    fn caches_only_inside_rom_ranges() {
        let mut c = ICache::default();
        c.set_rom_ranges(&[(0x4000_0000, 0x4000_1000)]);
        let mut decodes = 0;
        for _ in 0..3 {
            c.get(0x4000_0010, || {
                decodes += 1;
                decode_with(0x4000_0010, rom)
            });
        }
        assert_eq!((decodes, c.hits, c.misses), (1, 2, 1));
        for _ in 0..2 {
            c.get(0x3C00_0000, || {
                decodes += 1;
                decode_with(0x3C00_0000, rom)
            });
        }
        assert_eq!(decodes, 3, "RAM is decoded every time");
        // The last 8 bytes of a range are not cached (the instruction could
        // extend past it).
        c.get(0x4000_0FFC, || {
            decodes += 1;
            decode_with(0x4000_0FFC, rom)
        });
        c.get(0x4000_0FFC, || {
            decodes += 1;
            decode_with(0x4000_0FFC, rom)
        });
        assert_eq!(decodes, 5);
    }

    #[test]
    fn colliding_addresses_are_told_apart() {
        let mut c = ICache::default();
        c.set_rom_ranges(&[(0, 0x100_0000)]);
        let a = c.get(0x10, || decode_with(0x10, rom));
        let b = c.get(0x10 + (SETS as u32) * 2, || decode_with(0x10 + (SETS as u32) * 2, rom));
        assert_eq!((a.addr, b.addr), (0x10, 0x10 + (SETS as u32) * 2));
        assert_eq!(c.hits, 0);
    }
}
