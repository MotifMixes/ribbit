//! The chip's LCD controller: it shows a buffer in on-chip RAM, one byte
//! per 8-pixel column of a page (bit 7 at the top), pages one after another.
//!
//! EVIDENCE (the kernels' LCD set-up, iQuest kernel 0x0086-0x00EE, TTExtreme
//! 0x494D-0x49B5; the pictures come out whole):
//!
//! | Reg | iQuest | TT Extreme | What |
//! |---|---|---|---|
//! | FCE3 | 59h | 5Fh | columns - 1 (90, 96) |
//! | FCE4 | A6h | D0h | buffer address / 8 in on-chip RAM (XDATA C530, C680) |
//! | FCE2 | 87h / C7h | 83h / C3h | bit 7 on (HYPOTHESIS), bits 0-2 pages - 1 (8, 4); bit 6 set when FC2E bit 4 is: UNKNOWN |
//! | FCE1 | 12h | 12h | UNKNOWN (timing?) |
//!
//! The Turbo Twist Extreme's firmware draws at C500 and copies each page
//! column-reversed to C680, the buffer shown, so its glass shows column 0
//! on the right (EVIDENCE: C680 read left to right is mirrored text). The
//! iQuest's copy (C260 to C530) is straight.

/// The picture the LCD shows, one byte per pixel (1 = dark).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Frame {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.pixels[y * self.width + x] != 0
    }
}

/// Reads the shown buffer out of on-chip RAM (`ram` from XDATA C000).
/// `mirror` reverses the columns (the Turbo Twist Extreme's glass).
pub fn frame(io: &[u8], ram: &[u8], mirror: bool) -> Frame {
    let width = io[0xE3] as usize + 1;
    let pages = (io[0xE2] & 7) as usize + 1;
    let start = io[0xE4] as usize * 8;
    let mut f = Frame { width, height: pages * 8, pixels: vec![0; width * pages * 8] };
    if io[0xE2] & 0x80 == 0 {
        return f;
    }
    for p in 0..pages {
        for x in 0..width {
            let b = ram.get(start + p * width + x).copied().unwrap_or(0);
            let col = if mirror { width - 1 - x } else { x };
            for y in 0..8 {
                f.pixels[(p * 8 + y) * width + col] = (b >> (7 - y)) & 1;
            }
        }
    }
    f
}
