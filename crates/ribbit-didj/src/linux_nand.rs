//! Two of Linux's NAND functions (2.6.20) run natively; together most of
//! the busiest seconds of the Didj's start-up, which reads ~14 MB of NAND.
//! `Didj::run` finds them in the kernel by their code and, when the CPU
//! calls one, does its work, leaves the registers as its code does and
//! counts its instructions, so the console runs the same (VERIFIED with
//! didjboot DIDJ_NAND_CHECK=1 against the kernel's own code).
//!
//! * `nand_calculate_ecc` (drivers/mtd/nand/nand_ecc.c): 3 ECC bytes per
//!   256 bytes of every page read or written (HWECC is off in
//!   lf1000_defconfig), 3379 instructions each.
//! * `nand_read_buf` (nand_base.c): reads `len` bytes from the controller's
//!   data port, 6 instructions each. The bytes still come through the bus
//!   (the controller sees every read) and go into memory through it.

/// `nand_calculate_ecc` up to the end of its loop, as in firmware
/// 1.35.2.4222's kernel (word `ECC_LITERAL`, the table literal's offset, is
/// not compared).
const ECC_CODE: [u32; 23] = [
    0xE1A0C00D, 0xE92DD9F0, 0xE24CB004, 0xE3A05000, 0xE59F70E8, 0xE1A0E001, 0xE1A08002, 0xE1A04005, 0xE1A06005, 0xE1A00005, 0xE4DE3001, 0xE200C0FF,
    0xE7D72003, 0xE1E0100C, 0xE3120040, 0xE2800001, 0xE0211004, 0xE202203F, 0x1026600C, 0x120140FF, 0xE3500C01, 0xE0225005, 0x1AFFFFF2,
];
/// `ldr r7, [pc, #n]`: the table's address.
const ECC_LITERAL: usize = 4;

/// Instructions `nand_calculate_ecc` runs: 10 before the loop, 13 per
/// byte, 41 after.
pub const ECC_INSTRUCTIONS: u64 = 10 + 13 * 256 + 41;

/// `nand_read_buf`, whole.
const READ_BUF_CODE: [u32; 14] =
    [0xE1A0C00D, 0xE92DD800, 0xE24CB004, 0xE252E000, 0xE590C0AC, 0xD89DA800, 0xE3A00000, 0xE59C3000, 0xE5D32000, 0xE7C12000, 0xE2800001, 0xE15E0000, 0x1AFFFFF9, 0xE89DA800];

/// Instructions `nand_read_buf` runs for `len` bytes: 6 when `len` is 0 or
/// less, else 8 and 6 per byte.
pub fn read_buf_instructions(len: i32) -> u64 {
    if len <= 0 { 6 } else { 8 + 6 * len as u64 }
}

/// Kernel virtual addresses are physical + this (PAGE_OFFSET, SDRAM at 0).
const PAGE_OFFSET: u32 = 0xC000_0000;

/// The functions found in the kernel.
pub struct Found {
    /// `nand_calculate_ecc` and its table.
    pub ecc: Option<(u32, [u8; 256])>,
    pub read_buf: Option<u32>,
}

impl Found {
    /// Where the JIT must stop (`ArmJit::set_stops`).
    pub fn stops(&self) -> [u32; 2] {
        [self.ecc.map_or(u32::MAX, |e| e.0), self.read_buf.unwrap_or(u32::MAX)]
    }
}

/// Looks for the functions in the kernel's text in `sdram` (physical 0).
pub fn find(sdram: &[u8]) -> Found {
    let word = |at: usize| sdram.get(at..at + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let end = sdram.len().min(0x40_0000);
    let find_code = |code: &[u32], skip: usize| {
        (0..end.saturating_sub(4 * code.len())).step_by(4).find(|&at| {
            code.iter().enumerate().all(|(i, &w)| {
                let mask = if i == skip { !0xFFF } else { !0 };
                word(at + 4 * i).is_some_and(|v| v & mask == w & mask)
            })
        })
    };
    let ecc = find_code(&ECC_CODE, ECC_LITERAL).and_then(|at| {
        let ldr = at + 4 * ECC_LITERAL;
        let literal = ldr + 8 + (word(ldr)? & 0xFFF) as usize;
        let table_pa = word(literal)?.wrapping_sub(PAGE_OFFSET) as usize;
        Some((PAGE_OFFSET + at as u32, sdram.get(table_pa..table_pa + 256)?.try_into().ok()?))
    });
    let read_buf = find_code(&READ_BUF_CODE, usize::MAX).map(|at| PAGE_OFFSET + at as u32);
    Found { ecc, read_buf }
}

/// What `nand_calculate_ecc` leaves: the 3 ECC bytes and r1, r2, r3, ip.
pub fn calculate_ecc(data: &[u8], table: &[u8; 256]) -> ([u8; 3], [u32; 4]) {
    let (mut reg1, mut reg2, mut reg3) = (0u32, 0u32, 0u32);
    for (i, &b) in data.iter().enumerate() {
        let idx = table[b as usize] as u32;
        reg1 ^= idx & 0x3F;
        if idx & 0x40 != 0 {
            reg3 ^= i as u32;
            reg2 ^= !(i as u32) & 0xFF;
        }
    }
    let bit = |r: u32, b: u32, to: u32| ((r >> b) & 1) << to;
    let tmp1 = bit(reg3, 7, 7) | bit(reg2, 7, 6) | bit(reg3, 6, 5) | bit(reg2, 6, 4) | bit(reg3, 5, 3) | bit(reg2, 5, 2) | bit(reg3, 4, 1) | bit(reg2, 4, 0);
    // r2 as the code leaves it: tmp2 without reg2's bit 0 (ORed into r0).
    let r2 = bit(reg3, 3, 7) | bit(reg2, 3, 6) | bit(reg3, 2, 5) | bit(reg2, 2, 4) | bit(reg3, 1, 3) | bit(reg2, 1, 2) | bit(reg3, 0, 1);
    let tmp2 = r2 | bit(reg2, 0, 0);
    let r1 = (!reg1) << 2 | 3;
    ([!tmp1 as u8, !tmp2 as u8, r1 as u8], [r1, r2, !tmp1, reg2 & 0x10])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// nand_ecc.c's table, built as its comment describes: bits 0-5 the
    /// column parities, bit 6 the byte's parity.
    fn table() -> [u8; 256] {
        std::array::from_fn(|b| {
            let p = |mask: u32| (b as u32 & mask).count_ones() as u8 & 1;
            let cp = [p(0x55), p(0xAA), p(0x33), p(0xCC), p(0x0F), p(0xF0)];
            cp.iter().enumerate().fold(p(0xFF) << 6, |t, (i, &v)| t | v << i)
        })
    }

    #[test]
    fn erased_and_one_bit() {
        // An erased block's ECC is all ones (so erased pages pass).
        assert_eq!(calculate_ecc(&[0xFF; 256], &table()).0, [0xFF, 0xFF, 0xFF]);
        assert_eq!(calculate_ecc(&[0; 256], &table()).0, [0xFF, 0xFF, 0xFF]);
        let mut d = [0u8; 256];
        d[5] = 1;
        assert_ne!(calculate_ecc(&d, &table()).0, [0xFF, 0xFF, 0xFF]);
    }

    #[test]
    fn finds_both_functions() {
        let mut sdram = vec![0u8; 0x20_0000];
        let put = |s: &mut Vec<u8>, at: usize, words: &[u32]| words.iter().enumerate().for_each(|(i, w)| s[at + 4 * i..at + 4 * i + 4].copy_from_slice(&w.to_le_bytes()));
        put(&mut sdram, 0x1000, &ECC_CODE);
        // The literal 0xE8 past the ldr (+8) holds the table's address.
        put(&mut sdram, 0x1000 + 4 * ECC_LITERAL + 8 + 0xE8, &[PAGE_OFFSET + 0x8000]);
        sdram[0x8000] = 0x55;
        put(&mut sdram, 0x3000, &READ_BUF_CODE);
        let f = find(&sdram);
        let (va, table) = f.ecc.unwrap();
        assert_eq!((va, table[0]), (PAGE_OFFSET + 0x1000, 0x55));
        assert_eq!(f.read_buf, Some(PAGE_OFFSET + 0x3000));
        assert_eq!(read_buf_instructions(512), 8 + 6 * 512);
    }
}
