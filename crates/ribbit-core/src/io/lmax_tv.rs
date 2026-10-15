//! EXPERIMENTAL: the Leapster L-MAX's TV-output chip, at 0x2400_0000.
//!
//! What is known (all VERIFIED-FW, L-MAX BaseROM v2.2 152-11476; no hardware
//! documentation, no MAME support):
//!
//! | Behaviour | Evidence |
//! |---|---|
//! | The chip sits on a chip select at 0x2400_0000; the BIOS sets bus timing in 0x0180_102C first | 0x4009_BC7E |
//! | Probe: write 0x3D4C to +0x1C, read +0x0C; present if bits 31:16 = 0x0300 (10 tries, 10 ms apart), else "TV HW not found in select chip" | 0x4009_B204 |
//! | Registers +0x00-+0x20; +0x18 is a 16-bit port the BIOS reads and writes thousands of times (likely the serial link to the ADV7179 encoder) | access log |
//! | +0x2_0000-+0x2_F9FF: 64,000 bytes the BIOS fills (the TV picture, presumably) | access log |
//! | The video encoder is an Analog Devices ADV7179 (NTSC/PAL composite); NTSC mode registers 0x20/0x02/0x00, PAL 0x26/0x02 or 0x06/0x00 | BIOS debug strings at 0x4013_9C54 |
//!
//! Model: the ID register answers 0x0300_0000; everything else is plain
//! memory that keeps what is written, so the picture can be inspected.
//! Off unless asked for (Ribbit's machine is the original Leapster).

pub const BASE: u32 = 0x2400_0000;
pub const SIZE: u32 = 0x4_0000;
const ID_REG: u32 = 0x0C;
const ID: u32 = 0x0300_0000;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LmaxTv {
    pub mem: Vec<u8>,
    pub reads: u64,
    pub writes: u64,
}

impl Default for LmaxTv {
    fn default() -> Self {
        Self { mem: vec![0; SIZE as usize], reads: 0, writes: 0 }
    }
}

pub fn contains(addr: u32) -> bool {
    addr.wrapping_sub(BASE) < SIZE
}

impl LmaxTv {
    pub fn read(&mut self, addr: u32, width: u32) -> u32 {
        self.reads += 1;
        let o = (addr - BASE) as usize;
        let mut v = 0u32;
        for i in 0..width as usize {
            v |= (self.mem[(o + i) % SIZE as usize] as u32) << (8 * i);
        }
        if addr & !3 == BASE + ID_REG {
            v |= ID >> (8 * (addr & 3));
        }
        v
    }

    pub fn write(&mut self, addr: u32, value: u32, width: u32) {
        self.writes += 1;
        let o = (addr - BASE) as usize;
        for i in 0..width as usize {
            self.mem[(o + i) % SIZE as usize] = (value >> (8 * i)) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_the_bios_probe_and_keeps_writes() {
        let mut tv = LmaxTv::default();
        tv.write(BASE + 0x1C, 0x3D4C, 4);
        assert_eq!(tv.read(BASE + 0x0C, 4) & 0xFFFF_0000, 0x0300_0000);
        tv.write(BASE + 0x2_0000, 0xABCD, 2);
        assert_eq!(tv.read(BASE + 0x2_0000, 2), 0xABCD);
        assert!(contains(BASE + SIZE - 1) && !contains(BASE + SIZE));
    }
}
