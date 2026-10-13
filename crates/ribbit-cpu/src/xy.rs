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

/// Aux register block: 0x80 + offset.
pub const AUX_BASE: u32 = 0x80;
pub const AUX_COUNT: u32 = 0x20;
const XYCONFIG: usize = 0x18;
const BURST_SIZE: usize = 0x1B;
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
            ((memory[(address as usize / 2) % WORDS] >> ((address & 1) * 16)) & 0xFFFF) << 16
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
            let shift = (address & 1) * 16;
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
    /// bursts are not modelled as taking time (and are rejected, see below).
    pub fn aux_read(&self, n: u32) -> u32 {
        let off = (n - AUX_BASE) as usize;
        if off == XYCONFIG { self.aux[off] & !0x10 } else { self.aux[off] }
    }

    /// SR to aux 0x80-0x9F. Writing the burst size register starts a DMA
    /// between XY and system memory (MAME); not modelled yet.
    pub fn aux_write(&mut self, n: u32, value: u32) -> Result<(), &'static str> {
        let off = (n - AUX_BASE) as usize;
        if off == BURST_SIZE {
            return Err("XY burst transfer (aux 0x9B) is not modelled");
        }
        self.aux[off] = value;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        xy.aux[0] = 3; // odd halfword of word 1
        xy.write(32, 0x1234_0000);
        assert_eq!(xy.mem[0][0][1], 0x1234_0000);
        xy.aux[0] = 3;
        assert_eq!(xy.read(48), 0x1234_0000, "x0_nu shares MX00's format bit");
    }

    #[test]
    fn burst_is_reported() {
        let mut xy = XyExtension::default();
        assert!(xy.aux_write(0x9B, 0x10).is_err());
        assert!(xy.aux_write(0x98, 0x11).is_ok());
        assert_eq!(xy.aux_read(0x98), 0x01, "busy bit reads clear");
    }
}
