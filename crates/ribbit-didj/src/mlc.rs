//! The MLC (multi-layer controller): composes the picture from up to three
//! layers over a background colour (kernel drivers/lf1000/mlc: mlc_hal.h,
//! mlc.c; include/asm-arm/arch-lf1000/mlc.h for the formats).
//!
//! Registers from 0xC000_4000: CONTROLT 0x00 (MLCENB bit 1, the video
//! layer's priority bits 8-9), SCREENSIZE 0x04 (height bits 16+, width
//! bits 0-15), BGCOLOR 0x08. The two RGB layers, 0x34 apart from 0x0C:
//! LEFTRIGHT +0x00 (left bits 16+, right 0-11), TOPBOTTOM +0x04 (top,
//! bottom), CONTROL +0x18 (format bits 16-31, LAYERENB bit 5, BLENDENB 2,
//! TPENB 0), HSTRIDE +0x1C, VSTRIDE +0x20, TPCOLOR +0x24 (layer alpha bits
//! 28-31, key colour 0-23), ADDRESS +0x2C. Layer 2 is the video layer:
//! YUV 4:2:0 in three planes, LEFTRIGHT 0x74, TOPBOTTOM 0x78, CONTROL 0x7C,
//! the Y plane's stride 0x80 and address 0x8C, Cb 0x90 and Cr 0x94 with
//! their strides at 0x98 and 0x9C, and the scale (source per output
//! pixel, 2048 = 1:1, bits 0-22) across at 0xA0 and down at 0xA4.
//!
//! Layer 0 is drawn over layer 1 ("Cursor > Window" in the priority
//! enum); the video layer goes over both, under layer 0 or under both by
//! CONTROLT bits 8-9 (0, 1, 2). Addresses are physical: SDRAM from 0, and
//! "block" addresses (bit 29 set) for 2D surfaces 4096 bytes across
//! (`block_sdram`).

pub const BASE: u32 = 0xC000_4000;

/// Block (2D) addresses: bit 29 set, the segment (16 MB of SDRAM) in bits
/// 24-28, then V (the row, bits 12-23) and U (the byte, bits 0-11) of a
/// 4096 x 4096-byte square. The 3D engine's and the video's surfaces;
/// software maps and writes them at these addresses too.
pub const BLOCK_BASE: u32 = 0x2000_0000;
/// Segments 0 and 1 are the 32 MB of SDRAM.
pub const BLOCK_END: u32 = 0x2200_0000;

/// The SDRAM byte behind block address `addr` (POLLUX data book 22.2.4).
/// The square is stored in blocks of 64 x 32 bytes (2 KiB), 64 to a row
/// of blocks: display mode (Table 22-2) keeps each block's rows whole,
/// texture mode (Table 22-3) stores it as 4 x 2-byte cells. The CPU, the
/// MLC and the 3D engine's render target use display mode; textures and
/// palettes are read in texture mode.
pub fn block_sdram(addr: u32, texture: bool) -> usize {
    let (seg, v, u) = (addr >> 24 & 0x1F, addr >> 12 & 0xFFF, addr & 0xFFF);
    let inner = if texture { (v >> 1 & 15) << 7 | (u >> 2 & 15) << 3 | (v & 1) << 2 | (u & 3) } else { (v & 31) << 6 | (u & 63) };
    (seg << 24 | (v >> 5) << 17 | (u >> 6) << 11 | inner) as usize
}

const CONTROLT: u32 = 0x00;
const SCREENSIZE: u32 = 0x04;
const BGCOLOR: u32 = 0x08;
const LAYER: [u32; 2] = [0x0C, 0x40];
const LEFTRIGHT: u32 = 0x00;
const TOPBOTTOM: u32 = 0x04;
const CONTROL: u32 = 0x18;
const HSTRIDE: u32 = 0x1C;
const VSTRIDE: u32 = 0x20;
const TPCOLOR: u32 = 0x24;
const ADDRESS: u32 = 0x2C;

const VIDEO_LEFTRIGHT: u32 = 0x74;
const VIDEO_TOPBOTTOM: u32 = 0x78;
const VIDEO_CONTROL: u32 = 0x7C;
const VIDEO_STRIDE: u32 = 0x80;
const VIDEO_Y: u32 = 0x8C;
const VIDEO_CB: u32 = 0x90;
const VIDEO_CR: u32 = 0x94;
const STRIDE_CB: u32 = 0x98;
const STRIDE_CR: u32 = 0x9C;
const HSCALE: u32 = 0xA0;
const VSCALE: u32 = 0xA4;

const MLCENB: u32 = 1 << 1;
const LAYERENB: u32 = 1 << 5;
const BLENDENB: u32 = 1 << 2;
const TPENB: u32 = 1 << 0;

/// The LCD's size, used while SCREENSIZE is unset.
pub const WIDTH: usize = 320;
pub const HEIGHT: usize = 240;

/// A pixel in format `fmt` (the CONTROL register's top half) as
/// (0x00RRGGBB, alpha 0-255). Bit 15 of the format swaps red and blue.
/// Formats not listed read as black.
fn decode(fmt: u32, v: u32, bytes: u32) -> (u32, u32) {
    let field = |shift: u32, bits: u32| {
        let x = (v >> shift) & ((1 << bits) - 1);
        // Widened to 8 bits by repeating the top bits.
        (x << (8 - bits)) | (x >> (2 * bits).saturating_sub(8).min(bits))
    };
    let (r, g, b, a) = match fmt & 0x7FFF {
        0x4432 => (field(11, 5), field(5, 6), field(0, 5), 255),
        0x4342 => (field(10, 5), field(5, 5), field(0, 5), 255),
        0x3342 => (field(10, 5), field(5, 5), field(0, 5), if v & 0x8000 != 0 { 255 } else { 0 }),
        0x4211 => (field(8, 4), field(4, 4), field(0, 4), 255),
        0x2211 => (field(8, 4), field(4, 4), field(0, 4), field(12, 4)),
        // 24 or 32 bits a pixel (the same code: the stride tells them apart).
        0x4653 => (v >> 16 & 0xFF, v >> 8 & 0xFF, v & 0xFF, 255),
        0x0653 if bytes == 4 => (v >> 16 & 0xFF, v >> 8 & 0xFF, v & 0xFF, v >> 24),
        _ => (0, 0, 0, 255),
    };
    let (r, b) = if fmt & 0x8000 != 0 { (b, r) } else { (r, b) };
    (r << 16 | g << 8 | b, a)
}

fn blend(under: u32, over: u32, a: u32) -> u32 {
    let mix = |s: u32| ((over >> s & 0xFF) * a + (under >> s & 0xFF) * (255 - a)) / 255;
    mix(16) << 16 | mix(8) << 8 | mix(0)
}

/// BT.601 YCbCr (studio range) to 0x00RRGGBB.
fn yuv(y: u8, cb: u8, cr: u8) -> u32 {
    let (y, cb, cr) = (1.164 * (y as f32 - 16.0), cb as f32 - 128.0, cr as f32 - 128.0);
    let c = |v: f32| v.round().clamp(0.0, 255.0) as u32;
    c(y + 1.596 * cr) << 16 | c(y - 0.392 * cb - 0.813 * cr) << 8 | c(y + 2.017 * cb)
}

/// Draws the video layer into `out` (`w` across).
fn video(reg: &impl Fn(u32) -> u32, mem: &impl Fn(u32, u32) -> u32, out: &mut [u32], w: usize, h: usize) {
    let r = |off: u32| reg(BASE + off);
    if r(VIDEO_CONTROL) & LAYERENB == 0 {
        return;
    }
    let (lr, tb) = (r(VIDEO_LEFTRIGHT), r(VIDEO_TOPBOTTOM));
    let (left, right) = ((lr >> 16 & 0xFFF) as usize, (lr & 0xFFF) as usize);
    let (top, bottom) = ((tb >> 16 & 0xFFF) as usize, (tb & 0xFFF) as usize);
    // The Y plane's address loses its block bit in the register (the
    // driver notes it goes in the memory controller's MEMCONTROL instead):
    // GUESS it is a block address when Cb's is.
    let block = r(VIDEO_CB) & BLOCK_BASE;
    let (ya, cba, cra) = (r(VIDEO_Y) | block, r(VIDEO_CB), r(VIDEO_CR));
    let (ys, cbs, crs) = (r(VIDEO_STRIDE), r(STRIDE_CB), r(STRIDE_CR));
    let scale = |v: u32| match v & 0x7F_FFFF {
        0 => 2048,
        s => s,
    };
    let (hs, vs) = (scale(r(HSCALE)), scale(r(VSCALE)));
    for y in top..=bottom.min(h - 1) {
        let sy = ((y - top) as u32 * vs) >> 11;
        for x in left..=right.min(w - 1) {
            let sx = ((x - left) as u32 * hs) >> 11;
            let luma = mem(ya + sy * ys + sx, 1) as u8;
            let cb = mem(cba + (sy / 2) * cbs + sx / 2, 1) as u8;
            let cr = mem(cra + (sy / 2) * crs + sx / 2, 1) as u8;
            out[y * w + x] = yuv(luma, cb, cr);
        }
    }
}

/// The picture the MLC shows: (width, height, 0x00RRGGBB pixels). `mem`
/// reads `size` bytes at a physical address (little-endian).
/// Whether `addr` is a layer's ADDRESS register (a write that changes it
/// shows another buffer: a game's frame is done).
pub fn is_layer_address(addr: u32) -> bool {
    LAYER.iter().any(|&l| addr == BASE + l + ADDRESS)
}

pub fn compose(reg: impl Fn(u32) -> u32, mem: impl Fn(u32, u32) -> u32) -> (usize, usize, Vec<u32>) {
    let size = reg(BASE + SCREENSIZE);
    let (w, h) = match ((size & 0xFFF) as usize + 1, (size >> 16 & 0xFFF) as usize + 1) {
        (1, 1) => (WIDTH, HEIGHT),
        // GUESS: the register holds the size less one, as the positions
        // hold the last column and row.
        wh => wh,
    };
    let mut out = vec![reg(BASE + BGCOLOR) & 0xFF_FFFF; w * h];
    if reg(BASE + CONTROLT) & MLCENB == 0 {
        return (w, h, out);
    }
    // The video layer's place among the others (CONTROLT bits 8-9).
    let video_under = (reg(BASE + CONTROLT) >> 8 & 3).min(2) as usize;
    if video_under == 2 {
        video(&reg, &mem, &mut out, w, h);
    }
    for (i, &l) in LAYER.iter().enumerate().rev() {
        if i == 0 && video_under == 1 {
            video(&reg, &mem, &mut out, w, h);
        }
        let r = |off: u32| reg(BASE + l + off);
        let control = r(CONTROL);
        if control & LAYERENB == 0 {
            continue;
        }
        let fmt = control >> 16;
        let (lr, tb) = (r(LEFTRIGHT), r(TOPBOTTOM));
        let (left, right) = ((lr >> 16 & 0xFFF) as usize, (lr & 0xFFF) as usize);
        let (top, bottom) = ((tb >> 16 & 0xFFF) as usize, (tb & 0xFFF) as usize);
        let (hs, vs) = (r(HSTRIDE), r(VSTRIDE));
        let bytes = hs.clamp(1, 4);
        let tp = r(TPCOLOR);
        let layer_alpha = if control & BLENDENB != 0 { (tp >> 28) * 17 } else { 255 };
        let base = r(ADDRESS);
        for y in top..=bottom.min(h - 1) {
            for x in left..=right.min(w - 1) {
                let a = base.wrapping_add(((y - top) as u32).wrapping_mul(vs)).wrapping_add((x - left) as u32 * hs);
                let v = mem(a, bytes);
                let (rgb, pa) = decode(fmt, v, bytes);
                if control & TPENB != 0 && rgb == tp & 0xFF_FFFF {
                    continue;
                }
                let alpha = pa * layer_alpha / 255;
                let at = &mut out[y * w + x];
                *at = if alpha >= 255 { rgb } else { blend(*at, rgb, alpha) };
            }
        }
    }
    if video_under == 0 {
        video(&reg, &mem, &mut out, w, h);
    }
    (w, h, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_widens_to_full_white_and_black() {
        assert_eq!(decode(0x4432, 0xFFFF, 2).0, 0xFF_FFFF);
        assert_eq!(decode(0x4432, 0x0000, 2).0, 0);
        // Pure red, and blue with bit 15 of the format set.
        assert_eq!(decode(0x4432, 0xF800, 2).0, 0xFF_0000);
        assert_eq!(decode(0xC432, 0xF800, 2).0, 0x00_00FF);
    }

    #[test]
    fn an_enabled_layer_is_drawn_where_placed() {
        let mut sdram = vec![0u8; 0x1000];
        // A 2 x 1 RGB565 layer at (1, 1): red, green.
        sdram[0x100..0x104].copy_from_slice(&[0x00, 0xF8, 0xE0, 0x07]);
        let reg = |a: u32| match a - BASE {
            CONTROLT => MLCENB,
            SCREENSIZE => (3 << 16) | 3,
            0x0C => (1 << 16) | 2,
            0x10 => (1 << 16) | 1,
            0x24 => (0x4432 << 16) | LAYERENB,
            0x28 => 2,
            0x2C => 4,
            0x38 => 0x100,
            _ => 0,
        };
        let mem = |a: u32, n: u32| (0..n).fold(0, |v, i| v | (sdram[(a + i) as usize] as u32) << (8 * i));
        let (w, h, px) = compose(reg, mem);
        assert_eq!((w, h), (4, 4));
        assert_eq!(px[w + 1], 0xFF_0000);
        assert_eq!(px[w + 2], 0x00_FF00);
        assert_eq!(px[0], 0);
    }

    #[test]
    fn video_grey_is_grey() {
        assert_eq!(yuv(235, 128, 128), 0xFF_FFFF);
        assert_eq!(yuv(16, 128, 128), 0);
    }
}
