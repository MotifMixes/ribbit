//! LCD controller state and framebuffer decoding.
//!
//! The panel is a 160x160 CSTN LCD. Register meanings and pixel formats are
//! taken from MAME `leapster.cpp` (`screen_update_leapster`, LCD registers
//! at 0x0180_8084..0x0180_809B) and are UNVERIFIED on hardware:
//!
//! * format register bit 31 = display enable; bits 0..29 = pixel format
//! * format 4: 12 bpp, two pixels packed into three bytes
//! * format 3: 8 bpp RGB332 (used by touchscreen calibration)
//!
//! The decoder reads the same VRAM bytes the emulated CPU writes. It never
//! fabricates an image: an unknown or disabled format yields `Blank`.

pub const WIDTH: usize = 160;
pub const HEIGHT: usize = 160;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LcdRegs {
    /// Byte offset of the framebuffer within VRAM.
    pub fb_base: u32,
    /// Bytes per scanline.
    pub stride: u32,
    pub format: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameStatus {
    /// Pixels were decoded from VRAM.
    Decoded { format: u32 },
    /// Display disabled or format not understood; output is black.
    Blank { format_reg: u32 },
}

impl LcdRegs {
    pub fn enabled(&self) -> bool {
        self.format & (1 << 31) != 0
    }

    pub fn pixel_format(&self) -> u32 {
        self.format & 0x3FFF_FFFF
    }
}

fn expand4(v: u32) -> u32 {
    (v & 0xF) * 0x11
}

fn expand3(v: u32) -> u32 {
    let v = v & 7;
    (v << 5) | (v << 2) | (v >> 1)
}

fn rgb(r: u32, g: u32, b: u32) -> u32 {
    0xFF00_0000 | (r << 16) | (g << 8) | b
}

/// Decodes one frame from VRAM into 0xAARRGGBB pixels. Reads outside VRAM
/// return 0, matching the bus's unmapped-read value.
pub fn decode_frame(vram: &[u8], regs: &LcdRegs, out: &mut [u32; WIDTH * HEIGHT]) -> FrameStatus {
    let byte = |offset: usize| vram.get(offset).copied().unwrap_or(0) as u32;
    let format = regs.pixel_format();
    if !regs.enabled() || !(format == 3 || format == 4) {
        out.fill(0xFF00_0000);
        return FrameStatus::Blank { format_reg: regs.format };
    }
    for y in 0..HEIGHT {
        let line = regs.fb_base as usize + y * regs.stride as usize;
        for x in 0..WIDTH {
            out[y * WIDTH + x] = if format == 4 {
                let pair = line + (x >> 1) * 3;
                let b1 = byte(pair + 1);
                if x & 1 == 0 {
                    let b0 = byte(pair);
                    rgb(expand4(b1 >> 4), expand4(b0 >> 4), expand4(b0))
                } else {
                    let b2 = byte(pair + 2);
                    rgb(expand4(b1), expand4(b2 >> 4), expand4(b2))
                }
            } else {
                let p = byte(line + x);
                rgb(expand3(p >> 5), expand3(p >> 2), (p & 3) * 0x55)
            };
        }
    }
    FrameStatus::Decoded { format }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame() -> Box<[u32; WIDTH * HEIGHT]> {
        Box::new([0; WIDTH * HEIGHT])
    }

    #[test]
    fn disabled_display_is_blank() {
        let mut out = frame();
        let regs = LcdRegs { fb_base: 0, stride: 240, format: 4 };
        assert_eq!(decode_frame(&[0xFF; 0x10000], &regs, &mut out), FrameStatus::Blank { format_reg: 4 });
        assert!(out.iter().all(|&p| p == 0xFF00_0000));
    }

    #[test]
    fn unknown_format_is_blank() {
        let mut out = frame();
        let regs = LcdRegs { fb_base: 0, stride: 160, format: 0x8000_0007 };
        assert!(matches!(decode_frame(&[0; 0x10000], &regs, &mut out), FrameStatus::Blank { .. }));
    }

    #[test]
    fn format4_unpacks_two_12bit_pixels_from_three_bytes() {
        let mut vram = vec![0u8; 0x10000];
        // Pixel 0 = r:0xA g:0x1 b:0x2, pixel 1 = r:0xB g:0x3 b:0x4
        // byte0 = g0<<4|b0, byte1 = r0<<4|r1, byte2 = g1<<4|b1
        let base = 0x100;
        let stride = 240;
        vram[base + stride..base + stride + 3].copy_from_slice(&[0x12, 0xAB, 0x34]);
        let regs = LcdRegs { fb_base: base as u32, stride: stride as u32, format: 0x8000_0004 };
        let mut out = frame();
        assert_eq!(decode_frame(&vram, &regs, &mut out), FrameStatus::Decoded { format: 4 });
        assert_eq!(out[WIDTH], 0xFFAA_1122);
        assert_eq!(out[WIDTH + 1], 0xFFBB_3344);
        assert_eq!(out[0], 0xFF00_0000);
    }

    #[test]
    #[allow(clippy::unusual_byte_groupings)] // grouped as R:3 G:3 B:2
    fn format3_is_rgb332() {
        let mut vram = vec![0u8; 0x10000];
        vram[0] = 0b111_000_00;
        vram[1] = 0b000_111_00;
        vram[2] = 0b000_000_11;
        let regs = LcdRegs { fb_base: 0, stride: 160, format: 0x8000_0003 };
        let mut out = frame();
        decode_frame(&vram, &regs, &mut out);
        assert_eq!(&out[0..3], &[0xFFFF_0000, 0xFF00_FF00, 0xFF00_00FF]);
    }
}
