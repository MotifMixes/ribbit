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
//! The Leapster TV (BIOS 152-11594) uses the same chip; its picture format
//! is documented at `ycbcr565_to_rgb` and `FRAME`.
//!
//! Model: the ID register answers 0x0300_0000; everything else is plain
//! memory that keeps what is written, and the machine shows the picture
//! from it. Present only on a Leapster TV (`Machine::enable_leapster_tv`).

pub const BASE: u32 = 0x2400_0000;
pub const SIZE: u32 = 0x4_0000;
const ID_REG: u32 = 0x0C;
const ID: u32 = 0x0300_0000;
/// The TV frame: 200x160 pixels, 2 bytes each, rows packed.
pub const FRAME: u32 = 0x2_0000;
pub const TV_WIDTH: usize = 200;
pub const TV_HEIGHT: usize = 160;
/// The game's 160x160 picture starts 20 pixels in (the BIOS fills the
/// 20-pixel side borders with the colour in register +0x08).
pub const GAME_X: usize = 20;
pub const GAME_SIZE: usize = 160;
/// `FrameStatus::Decoded` format number reported for TV pictures (not an
/// LCD format register value).
pub const FORMAT: u32 = 0x565;

/// TV pixel: YCbCr 5:6:5, Cb in bits 15-11, Y in 10-5, Cr in 4-0, studio
/// range (black Y=16, neutral chroma=128). VERIFIED-FW against the Leapster
/// TV BIOS's RGB-to-TV table at 0x400D_94A4 (black 0x8090, white 0x8750,
/// red 0x5A9D, green 0x3484 are BT.601). How the chip widens the fields to
/// 8 bits is UNKNOWN; zero-filled low bits keep neutral chroma at exactly 128.
pub fn ycbcr565_to_rgb(v: u16) -> u32 {
    let v = v as i32;
    let cb = ((v >> 11) & 0x1F) << 3;
    let y = ((v >> 5) & 0x3F) << 2;
    let cr = (v & 0x1F) << 3;
    // BT.601 studio range to full-range RGB, 16.16 fixed point.
    let (c, d, e) = (y - 16, cb - 128, cr - 128);
    let clamp = |x: i32| ((x + 0x8000) >> 16).clamp(0, 255) as u32;
    let r = clamp(76_284 * c + 104_595 * e);
    let g = clamp(76_284 * c - 25_690 * d - 53_281 * e);
    let b = clamp(76_284 * c + 132_186 * d);
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LmaxTv {
    pub mem: Vec<u8>,
    pub reads: u64,
    pub writes: u64,
    /// Research: accesses per instruction address (not saved).
    #[serde(skip)]
    pub pcs: std::collections::BTreeMap<u32, u64>,
}

impl Default for LmaxTv {
    fn default() -> Self {
        Self { mem: vec![0; SIZE as usize], reads: 0, writes: 0, pcs: Default::default() }
    }
}

pub fn contains(addr: u32) -> bool {
    addr.wrapping_sub(BASE) < SIZE
}

impl LmaxTv {
    pub fn read(&mut self, addr: u32, width: u32, pc: u32) -> u32 {
        self.reads += 1;
        *self.pcs.entry(pc).or_default() += 1;
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

    /// The TV frame pixel at (x, y), 0 <= x < TV_WIDTH, 0 <= y < TV_HEIGHT.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let o = FRAME as usize + 2 * (y * TV_WIDTH + x);
        u16::from_le_bytes([self.mem[o], self.mem[o + 1]])
    }

    /// The 160x160 game picture (the TV frame minus its side borders), as
    /// 0xAARRGGBB, in the LCD's size so every frontend can show it.
    pub fn render_game(&self, out: &mut [u32]) {
        for y in 0..TV_HEIGHT {
            for x in 0..GAME_SIZE {
                out[y * GAME_SIZE + x] = ycbcr565_to_rgb(self.pixel(GAME_X + x, y));
            }
        }
    }

    pub fn write(&mut self, addr: u32, value: u32, width: u32, pc: u32) {
        self.writes += 1;
        *self.pcs.entry(pc).or_default() += 1;
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
        tv.write(BASE + 0x1C, 0x3D4C, 4, 0);
        assert_eq!(tv.read(BASE + 0x0C, 4, 0) & 0xFFFF_0000, 0x0300_0000);
        tv.write(BASE + 0x2_0000, 0xABCD, 2, 0);
        assert_eq!(tv.read(BASE + 0x2_0000, 2, 0), 0xABCD);
        assert!(contains(BASE + SIZE - 1) && !contains(BASE + SIZE));
    }

    #[test]
    fn tv_pixels_decode_the_bios_colour_table() {
        // Entries of the Leapster TV BIOS's RGB444 -> TV table (0x400D_94A4).
        let rgb = |v: u16| ycbcr565_to_rgb(v) & 0xFF_FFFF;
        assert_eq!(rgb(0x8090), 0x00_0000, "black (index 000)");
        assert!(rgb(0x8750) & 0xFF >= 0xF8, "white (fff) is near full");
        // Within 16 per channel: 5-bit chroma is coarse (red's Cr is 232, not 240).
        let close = |got: u32, want: u32| (0..3).all(|i| ((got >> (8 * i)) as u8).abs_diff((want >> (8 * i)) as u8) <= 16);
        assert!(close(rgb(0x5A9D), 0xFF_0000), "red (f00): {:06x}", rgb(0x5A9D));
        assert!(close(rgb(0x3484), 0x00_FF00), "green (0f0): {:06x}", rgb(0x3484));
        assert!(close(rgb(0x1692), 0xFF_FF00), "yellow (ff0): {:06x}", rgb(0x1692));
    }
}
