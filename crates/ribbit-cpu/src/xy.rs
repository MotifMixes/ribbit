//! XY memory / DSP extension: core registers r32-r55 and aux 0x80-0x9F.
//!
//! Evidence (docs/isa-arcompact.md §XY extension):
//! * GNU binutils names r32-r47 `x0_u0, x0_u1, x1_u0, ... y3_u1`, r48-r55
//!   `x0_nu ... y3_nu`, and aux 0x80-0x87 `ax0-ax3, ay0-ay3`, 0x88-0x97
//!   `mx00 ... my31`, 0x98 `xyconfig`, 0x99-0x9E burst/base registers. So the
//!   core registers are access ports through eight address generators (four
//!   into X memory, four into Y memory), each with two post-modifiers (u0/u1)
//!   or none (nu).
//! * The Leapster v1.5 BIOS saves and restores r32-r59 in its MQX context
//!   switch (0x4007_3CA0 / 0x4007_3D74), so the ports exist on this CPU.
//! * The access semantics below are MAME's (`arcompact.cpp`, BSD-3,
//!   2026-10-07): a port reads/writes the word at its AX/AY address, then a
//!   u0/u1 access adds the signed low 16 bits of MXnm to the address, modulo
//!   the length in bits 27:16 (0 = linear); bit 29 selects 16-bit data in the
//!   top half of the register. Memory size (2 banks x 2 x 1024 words, bank
//!   selected by XYCONFIG bit 0) is also MAME's. SOURCED, not verified.
//! * Bursts (MAME): writing aux 0x9B copies (low 16 bits + 1) bytes, a word
//!   at a time, between system memory at aux 0x99 and X (bit 29 clear) or Y
//!   memory from word aux 0x9A; bit 30 set copies into XY memory, clear copies
//!   out of it. Synchronous, so XYCONFIG's busy bit reads clear. SOURCED.
//! * Halfword order (Ribbit, not MAME): XY memory holds the lower-addressed
//!   halfword in the HIGH half of a word. A burst moves the halfword at
//!   system address a+0 to bits 31:16 and a+2 to bits 15:0 (and back), and
//!   in 16-bit mode an even halfword address selects the high half. MAME
//!   copies words as they are and maps even addresses to the low half.
//!   HYPOTHESIS, from the Leapster TV and Leapster 2 BIOSes (docs/
//!   hardware-research.md, "XY memory halfword order"): their PCM high-pass
//!   (0.6 0x4003_E098) filters each word's high half before its low half,
//!   and only this order makes it, the resampler (0x4003_E36C, 16-bit
//!   ports) and the bursts of address-ordered sample buffers agree; with
//!   MAME's order the filter scrambles every sample pair (an image of the
//!   sound mirrored about 16 kHz). The one routine that implies MAME's order
//!   (0.6 0x4003_E7A8, a 4x upsampler) is never called by any BIOS. Pipelines
//!   that treat both halves alike (original Leapster 2.1, L-MAX) give the
//!   same output either way.

/// Aux register block: 0x80 + offset.
pub const AUX_BASE: u32 = 0x80;
pub const AUX_COUNT: u32 = 0x20;
const XYCONFIG: usize = 0x18;
pub const BURST_SIZE: usize = 0x1B;
const BURST_SYSTEM: usize = 0x19;
const BURST_XY: usize = 0x1A;
const WORDS: usize = 0x400;

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct XyExtension {
    /// aux 0x80-0x9F: AX0-3, AY0-3, MX00-MY31, XYCONFIG, burst registers.
    pub aux: [u32; AUX_COUNT as usize],
    /// [bank][x=0 / y=1][word]
    #[serde(with = "xy_mem")]
    pub mem: Box<[[[u32; WORDS]; 2]; 2]>,
}

/// XY memory as one flat list of words (save states).
mod xy_mem {
    use super::WORDS;
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    type Mem = [[[u32; WORDS]; 2]; 2];

    pub fn serialize<S: Serializer>(m: &Mem, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(m.iter().flatten().flatten())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Box<Mem>, D::Error> {
        let v = Vec::<u32>::deserialize(d)?;
        if v.len() != 4 * WORDS {
            return Err(D::Error::custom("XY memory size"));
        }
        let mut m = Box::new([[[0; WORDS]; 2]; 2]);
        for (i, w) in v.into_iter().enumerate() {
            m[i / (2 * WORDS)][(i / WORDS) % 2][i % WORDS] = w;
        }
        Ok(m)
    }
}

impl Default for XyExtension {
    fn default() -> Self {
        Self { aux: [0; AUX_COUNT as usize], mem: Box::new([[[0; WORDS]; 2]; 2]) }
    }
}

impl std::fmt::Debug for XyExtension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("XyExtension").field("aux", &self.aux).finish_non_exhaustive()
    }
}

/// r32-r55 are ports; returns (address generator 0-7, modifier 0/1, updates).
fn port(reg: u8) -> (usize, usize, bool) {
    if reg < 48 {
        let n = (reg - 32) as usize;
        (n / 2, n % 2, true)
    } else {
        ((reg - 48) as usize, 0, false)
    }
}

/// 16-bit mode: the bit position of halfword `address` within its word (an
/// even address is the high half; module notes, "Halfword order").
fn half_shift(address: u32) -> u32 {
    if address & 1 == 0 { 16 } else { 0 }
}

pub fn is_port(reg: u8) -> bool {
    (32..56).contains(&reg)
}

impl XyExtension {
    fn update(&mut self, index: usize, modifier: u32) {
        let increment = modifier as u16 as i16 as i32;
        let length = (modifier >> 16) & 0x0FFF;
        let address = self.aux[index];
        self.aux[index] = if length != 0 {
            let base = address & !(length.next_power_of_two() - 1);
            let offset = ((address.wrapping_sub(base)) as i32 + increment).rem_euclid(length as i32);
            base + offset as u32
        } else {
            address.wrapping_add(increment as u32)
        };
    }

    fn location(&self, index: usize) -> (usize, usize) {
        ((self.aux[XYCONFIG] & 1) as usize, index / 4)
    }

    pub fn read(&mut self, reg: u8) -> u32 {
        let (index, which, updates) = port(reg);
        let modifier = self.aux[8 + index * 2 + which];
        let address = self.aux[index];
        let (bank, xy) = self.location(index);
        let memory = &self.mem[bank][xy];
        let value = if modifier & (1 << 29) != 0 {
            ((memory[(address as usize / 2) % WORDS] >> half_shift(address)) & 0xFFFF) << 16
        } else {
            memory[address as usize % WORDS]
        };
        if updates {
            self.update(index, modifier);
        }
        value
    }

    pub fn write(&mut self, reg: u8, data: u32) {
        let (index, which, updates) = port(reg);
        let modifier = self.aux[8 + index * 2 + which];
        let address = self.aux[index];
        let (bank, xy) = self.location(index);
        let memory = &mut self.mem[bank][xy];
        if modifier & (1 << 29) != 0 {
            let shift = half_shift(address);
            let word = &mut memory[(address as usize / 2) % WORDS];
            *word = (*word & !(0xFFFF << shift)) | ((data >> 16) << shift);
        } else {
            memory[address as usize % WORDS] = data;
        }
        if updates {
            self.update(index, modifier);
        }
    }

    /// LR from aux 0x80-0x9F. XYCONFIG's bit 4 (burst busy) reads 0 because
    /// bursts complete at once.
    pub fn aux_read(&self, n: u32) -> u32 {
        let off = (n - AUX_BASE) as usize;
        if off == XYCONFIG { self.aux[off] & !0x10 } else { self.aux[off] }
    }

    /// SR to aux 0x80-0x9F. Writing the burst size register (0x9B) copies
    /// between XY and system memory (see the module notes).
    pub fn aux_write(&mut self, n: u32, value: u32, bus: &mut impl ribbit_memory::Bus) {
        let off = (n - AUX_BASE) as usize;
        self.aux[off] = value;
        if off == BURST_SIZE {
            self.burst(value, bus);
        }
    }

    fn burst(&mut self, value: u32, bus: &mut impl ribbit_memory::Bus) {
        let (system, xy) = (self.aux[BURST_SYSTEM], self.aux[BURST_XY] as usize);
        let size = (value & 0xFFFF) + 1;
        let bank = (self.aux[XYCONFIG] & 1) as usize;
        let memory = &mut self.mem[bank][((value >> 29) & 1) as usize];
        for i in (0..size).step_by(4) {
            let word = &mut memory[(xy + i as usize / 4) % WORDS];
            // The halfword at the lower system address is the high half
            // (module notes, "Halfword order").
            if value & (1 << 30) != 0 {
                *word = bus.read32(system.wrapping_add(i)).rotate_left(16);
            } else {
                bus.write32(system.wrapping_add(i), word.rotate_left(16));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbit_memory::Bus;

    #[test]
    fn ports_follow_binutils_numbering() {
        assert_eq!(port(32), (0, 0, true)); // x0_u0
        assert_eq!(port(33), (0, 1, true)); // x0_u1
        assert_eq!(port(34), (1, 0, true)); // x1_u0
        assert_eq!(port(40), (4, 0, true)); // y0_u0
        assert_eq!(port(48), (0, 0, false)); // x0_nu
        assert_eq!(port(55), (7, 0, false)); // y3_nu
    }

    #[test]
    fn post_modify_linear_and_no_update() {
        let mut xy = XyExtension::default();
        xy.aux[0] = 10; // AX0
        xy.aux[8] = 2; // MX00: +2
        xy.write(32, 0xAAAA_0001); // x0_u0 -> word 10, AX0 = 12
        xy.write(32, 0xAAAA_0002); // word 12, AX0 = 14
        assert_eq!(xy.aux[0], 14);
        xy.aux[0] = 10;
        assert_eq!(xy.read(48), 0xAAAA_0001, "x0_nu reads without moving");
        assert_eq!(xy.aux[0], 10);
        assert_eq!(xy.read(32), 0xAAAA_0001);
        assert_eq!(xy.read(32), 0xAAAA_0002);
    }

    #[test]
    fn x_and_y_memories_are_separate() {
        let mut xy = XyExtension::default();
        xy.write(48, 1); // x0_nu at AX0 = 0
        xy.write(52, 2); // y0_nu at AY0 = 0
        assert_eq!((xy.read(48), xy.read(52)), (1, 2));
    }

    #[test]
    fn modulo_addressing_wraps_within_ring() {
        let mut xy = XyExtension::default();
        xy.aux[0] = 0x13; // AX0 inside the ring at 0x10..0x14
        xy.aux[8] = (4 << 16) | 1; // length 4, +1
        xy.read(32);
        assert_eq!(xy.aux[0], 0x10);
        xy.aux[8] = (4 << 16) | 0xFFFF; // length 4, -1
        xy.read(32);
        assert_eq!(xy.aux[0], 0x13);
    }

    #[test]
    fn sixteen_bit_format_uses_upper_half() {
        let mut xy = XyExtension::default();
        xy.aux[8] = 1 << 29;
        xy.aux[0] = 3; // odd halfword of word 1: its low half
        xy.write(32, 0x1234_0000);
        assert_eq!(xy.mem[0][0][1], 0x0000_1234);
        xy.aux[0] = 4; // halfword 4: word 2's high half
        xy.write(32, 0x5678_0000);
        assert_eq!(xy.mem[0][0][2], 0x5678_0000);
        xy.aux[0] = 3;
        assert_eq!(xy.read(48), 0x1234_0000, "x0_nu shares MX00's format bit");
    }

    /// System memory for burst tests: 64 KiB of words.
    struct Ram(Vec<u32>);
    impl ribbit_memory::Bus for Ram {
        fn read8(&mut self, a: u32) -> u8 {
            (self.0[a as usize / 4] >> ((a & 3) * 8)) as u8
        }
        fn read16(&mut self, a: u32) -> u16 {
            (self.0[a as usize / 4] >> ((a & 2) * 8)) as u16
        }
        fn read32(&mut self, a: u32) -> u32 {
            self.0[a as usize / 4]
        }
        fn write8(&mut self, a: u32, v: u8) {
            let w = &mut self.0[a as usize / 4];
            *w = (*w & !(0xFF << ((a & 3) * 8))) | (v as u32) << ((a & 3) * 8);
        }
        fn write16(&mut self, a: u32, v: u16) {
            let w = &mut self.0[a as usize / 4];
            *w = (*w & !(0xFFFF << ((a & 2) * 8))) | (v as u32) << ((a & 2) * 8);
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.0[a as usize / 4] = v;
        }
    }

    #[test]
    fn burst_then_sixteen_bit_ports_keep_address_order() {
        // Samples s0, s1, s2, s3 at ascending system addresses come back
        // through a 16-bit port in that order, and a filter reading 32-bit
        // words sees each pair's earlier sample in the high half.
        let mut xy = XyExtension::default();
        let mut ram = Ram(vec![0; 0x100]);
        ram.write16(0x40, 10);
        ram.write16(0x42, 11);
        ram.write16(0x44, 12);
        ram.write16(0x46, 13);
        xy.aux_write(0x99, 0x40, &mut ram);
        xy.aux_write(0x9A, 0, &mut ram);
        xy.aux_write(0x9B, (1 << 30) | 7, &mut ram); // 8 bytes into X
        assert_eq!(xy.mem[0][0][0], (10 << 16) | 11);
        xy.aux[8] = (1 << 29) | 1; // 16-bit, +1
        let read: Vec<u32> = (0..4).map(|_| xy.read(32) >> 16).collect();
        assert_eq!(read, [10, 11, 12, 13]);
        xy.aux_write(0x99, 0x80, &mut ram);
        xy.aux_write(0x9B, 7, &mut ram); // and back out
        assert_eq!((ram.read16(0x80), ram.read16(0x82), ram.read16(0x86)), (10, 11, 13));
    }

    #[test]
    fn burst_copies_into_and_out_of_xy_memory() {
        let mut xy = XyExtension::default();
        let mut ram = Ram((0..0x4000).collect());
        xy.aux_write(0x99, 0x100, &mut ram); // system address (word 0x40)
        xy.aux_write(0x9A, 5, &mut ram); // XY word 5
        xy.aux_write(0x9B, (1 << 30) | (1 << 29) | 11, &mut ram); // 12 bytes into Y
        assert_eq!(&xy.mem[0][1][5..8], &[0x40 << 16, 0x41 << 16, 0x42 << 16], "lower halfword high");
        assert_eq!(xy.mem[0][1][8], 0, "only the burst's size");
        assert_eq!(xy.mem[0][0][5], 0, "X untouched");
        xy.mem[0][0][2] = 0xDEAD_BEEF;
        xy.aux_write(0x9A, 2, &mut ram);
        xy.aux_write(0x99, 0x200, &mut ram);
        xy.aux_write(0x9B, 3, &mut ram); // 4 bytes out of X
        assert_eq!(ram.0[0x80], 0xBEEF_DEAD);
        xy.aux_write(0x98, 0x11, &mut ram);
        assert_eq!(xy.aux_read(0x98), 0x01, "busy bit reads clear");
    }
}
