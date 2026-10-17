//! The video chip's sprites (0xD0500000), drawn into the frame buffer the
//! panel is then sent. The chip is of the family of the Zippity's
//! (ribbit-zippity video.rs, and GeneralPlus's GPL162xx PPU before it); what
//! differs on the RockIt Twist is EVIDENCE from its menu's own sprite lists
//! (docs/rockit-twist.md, "Video"):
//!
//! * The frame is the panel's own shape, 240 x 320 (registers 0x120 and
//!   0x124 hold 240 and 320; 0x36C 0x00F00140), drawn into the buffer at
//!   0xD05001E8 as RGB 5:6:5, 240 pixels a row.
//! * Sprite entries are 16 bytes (the Zippity's are 32): halfword 0 the
//!   picture's address (bits 6-0 of halfword 4 above it) in 16-byte units
//!   from the sprite segment at 0xD0500088 (VERIFIED: the System's routine
//!   0xF6CC computes it so), halfwords 1 and 2 x and y (10-bit, signed),
//!   halfword 3 the attributes.
//! * x and y are the sprite's centre: x to the right from column 121, y up
//!   from line 255. With that, the menu's background (64-pixel tiles, then
//!   a 32- and a 16-wide column) covers the frame exactly, edge to edge.
//! * Palette RAM (0xD0501000) holds one 15-bit colour per word.
//!
//! Attributes as on the Zippity: bits 1-0 depth (2, 4, 6, 8 bits), 3-2
//! flips (with V3D on: 3 = an ordinary sprite, anything else a quad; see
//! `draw_sprite`), 5-4 and 7-6 width and height (8 << n), 11-8 palette,
//! 13-12 priority, 14 blending; but depth 3 with palette bit 0 is 32 bits
//! a pixel here (alpha, red, green, blue), not 16. Halfword 1 bits 15-10
//! the rotation, halfword 2 bits 15-10 the zoom; halfword 4 bits 6-0 the
//! picture address's top, 9-8 the window, 15-10 the blend level;
//! halfwords 5-7 a quad's other corners. All read from the System's PPU_*
//! routines, which build the list; UNVERIFIED on hardware.

use crate::machine::Bus;

pub const FRAME_W: usize = 240;
pub const FRAME_H: usize = 320;
/// The column x = 0 is centred on, and the line y = 0 is centred on (y up).
const CENTRE_X: i32 = 121;
const CENTRE_Y: i32 = 255;

fn rgb555_to_565(c: u32) -> u16 {
    let (r, g, b) = ((c >> 10) & 31, (c >> 5) & 31, c & 31);
    (r << 11 | (g << 1 | g >> 4) << 5 | b) as u16
}

fn blend565(over: u16, under: u16, alpha: u32, full: u32) -> u16 {
    let ch = |shift: u32, mask: u32| {
        let (a, b) = ((over as u32 >> shift) & mask, (under as u32 >> shift) & mask);
        ((a * alpha + b * (full - alpha)) / full) << shift
    };
    (ch(11, 31) | ch(5, 63) | ch(0, 31)) as u16
}

/// Each sprite's extended attributes, a word each (sprite n at +4n):
/// VERIFIED, the System's frame update (0xE614) copies its table of them
/// (512 words) here, after the palette to 0xD0501000 and a table to
/// 0xD0500400.
const SPRITE_EXT: u32 = 0xD050_6000;
/// Extended attribute bit 18: a large sprite, sized 32 << n instead of
/// 8 << n (the System's PPU_enableLargeSprite sets it; its size tables at
/// 0x74C50 hold 8, 16, 32, 64 and 32, 64, 128, 256).
const EXT_LARGE: u32 = 0x4_0000;

/// A sprite's width and height from its attributes (bits 5-4 and 7-6).
fn size(attr: u32, ext: u32) -> (u32, u32) {
    let unit = if ext & EXT_LARGE != 0 { 32 } else { 8 };
    (unit << ((attr >> 4) & 3), unit << ((attr >> 6) & 3))
}

/// A zoom index (PPU_setSprite2DZoomFix 0xFCAC, from a 4.12 scale): 1-32
/// in 32nds, then 33-63 in quarters on from 1 (63: 8.75). 0 is what the
/// System stores for a scale below 0 or above 8.75: drawn as nothing
/// (UNVERIFIED).
fn zoom(id: u32) -> f32 {
    match id {
        0 => 0.0,
        1..=32 => id as f32 / 32.0,
        _ => 1.0 + (id - 32) as f32 / 4.0,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Palette,
    Argb4444,
    Argb8888,
}

/// A signed 10-bit coordinate.
fn coord(h: u32) -> i32 {
    ((h & 0x3FF) as i32) << 22 >> 22
}

/// The text planes' registers and their pictures' base registers, planes
/// 0-3 (the System's frame update, 0xE2A0). Plane 4 (0x70-0x78) has no map
/// and is not drawn.
const TEXT_PLANES: [(u32, u32); 4] = [(0x40, 0x80), (0x58, 0x84), (0x00, 0x8C), (0x20, 0x90)];

/// A plane's width and height from its size number (PPU_getTextPlaneSize
/// 0xD420: attribute bits 15-14, and bit 16 above them). 0, 512 x 256, is
/// EVIDENCE (Nom-Nom's map); the rest follow the GPL162xx's doubling,
/// UNVERIFIED.
fn plane_size(n: u32) -> (u32, u32) {
    [(512, 256), (512, 512), (1024, 512), (1024, 1024), (2048, 1024), (2048, 2048), (4096, 2048), (4096, 4096)]
        [n as usize & 7]
}

/// The frame pixel shown at (x, y) on the screen (320 x 240): the frame is
/// the panel's, turned a quarter (its last column is the screen's top
/// line), as the System's panel set-up (36 40) shows it.
fn screen_to_frame(x: u32, y: u32) -> (i32, i32) {
    (FRAME_W as i32 - 1 - y as i32, x as i32)
}

/// A texel: colour (RGB 5:6:5) and alpha (0-255).
type Texel = (u16, u32);

fn plot(out: &mut [u16], x: i32, y: i32, (c, alpha): Texel) {
    if (0..FRAME_W as i32).contains(&x) && (0..FRAME_H as i32).contains(&y) {
        let o = y as usize * FRAME_W + x as usize;
        out[o] = if alpha >= 255 { c } else { blend565(c, out[o], alpha, 255) };
    }
}

impl Bus {
    fn peek8(&self, a: u32) -> u32 {
        self.ram.get(a as usize).copied().unwrap_or(0) as u32
    }

    /// Pixel `i` of a picture of `bpp` bits per pixel at `addr` (one bit
    /// stream, most significant bit first, as on the Zippity).
    fn sprite_pixel(&self, addr: u32, bpp: u32, i: u32) -> u32 {
        let bit = i * bpp;
        let a = addr + bit / 8;
        let w = self.peek8(a) << 16 | self.peek8(a + 1) << 8 | self.peek8(a + 2);
        (w >> (24 - bpp - bit % 8)) & ((1 << bpp) - 1)
    }

    /// Research: the sprites in the list, as (number, left, top, width,
    /// height, the entry's 8 halfwords) in the frame's coordinates.
    pub fn sprite_rects(&self) -> Vec<(u32, i32, i32, u32, u32, [u32; 8])> {
        let control = self.latch(0xD050_0108);
        let count = if control & 1 != 0 { ((control >> 8) & 0xFF) * 4 } else { 0 };
        let half = |n: u32, k: u32| (self.latch(0xD050_2000 + n * 16 + k / 2 * 4) >> (16 * (k & 1))) & 0xFFFF;
        (0..count.min(1024))
            .filter(|&n| half(n, 0) != 0)
            .map(|n| {
                let attr = half(n, 3);
                let (w, h) = size(attr, self.latch(SPRITE_EXT + 4 * n));
                let x0 = CENTRE_X + coord(half(n, 1)) - w as i32 / 2;
                let y0 = CENTRE_Y - coord(half(n, 2)) - h as i32 / 2;
                (n, x0, y0, w, h, std::array::from_fn(|k| half(n, k as u32)))
            })
            .collect()
    }

    /// Draws the frame from the sprite list into `out` (FRAME_W x FRAME_H,
    /// RGB 5:6:5).
    pub fn draw_frame(&self, out: &mut [u16]) {
        out.fill(0);
        let pal: Vec<u32> = (0..1024u32).map(|i| self.latch(0xD050_1000 + 4 * i) & 0xFFFF).collect();
        let control = self.latch(0xD050_0108);
        let count = if control & 1 != 0 { ((control >> 8) & 0xFF) * 4 } else { 0 };
        let half = |n: u32, k: u32| (self.latch(0xD050_2000 + n * 16 + k / 2 * 4) >> (16 * (k & 1))) & 0xFFFF;
        // Picture address 0 is no sprite (as on the Zippity).
        let mut order: Vec<(u32, u32)> =
            (0..count.min(1024)).filter(|&n| half(n, 0) != 0).map(|n| ((half(n, 3) >> 12) & 3, n)).collect();
        order.sort_unstable();
        // By priority, 0 first: at each, the text planes and then the
        // sprites (as on the Zippity).
        for d in 0..4 {
            for &(reg, base_reg) in &TEXT_PLANES {
                if (self.latch(0xD050_0008 + reg) >> 12) & 3 == d {
                    self.draw_text_plane(out, &pal, reg, base_reg);
                }
            }
            for &(_, n) in order.iter().filter(|o| o.0 == d) {
                let ext = self.latch(SPRITE_EXT + 4 * n);
                self.draw_sprite(out, &pal, ext, |k| half(n, k));
            }
        }
    }

    /// A text plane (registers at +reg: +0 x position, +4 y position, +8
    /// attributes, +0xC control, +0x10 the tile map; the pictures' base at
    /// +base_reg). VERIFIED as the System's frame update (0xE2A0) copies
    /// its five planes' settings in (planes 0-3 at 0x40, 0x58, 0x00, 0x20;
    /// bases 0x80, 0x84, 0x8C, 0x90); the bits from its PPU_setTextPlane*
    /// routines:
    ///
    /// * Control: bit 3 on (PPU_enableTextPlane), 1 "register mode" (the
    ///   attributes from the register for every tile), 0 bitmap mode, 4
    ///   horizontal offsets, 7 direct colour, 9-8 blending, 15-10 the
    ///   blend level.
    /// * Attributes: bits 1-0 depth, 3-2 flips, 5-4 and 7-6 tile width and
    ///   height (8 << n), 11-8 palette, 13-12 priority, 15-14 and 16 the
    ///   plane's size.
    /// * Colour modes (PPU_setTextPlaneColorMode 0xDB34, numbered as the
    ///   sprites'): without bit 7 the palette at 2/4/6/8 bits; with it,
    ///   depth 0 16 bits (palette 9 or 0xB: ARGB 4:4:4:4), 1 RGB 5:6:5, 2
    ///   32 bits ARGB 8:8:8:8, 3 YUV 4:2:2.
    ///
    /// EVIDENCE from Nom-Nom's level menu: plane 0 on (control 0xAA,
    /// attributes 0x30A2, y position 240), its map 32-bit entries, each the
    /// byte offset of a 32 x 32 tile from the base (0 for none), 16 a row,
    /// holding the game's title picture, 320 x 240, upright in ARGB 8:8:8:8
    /// with the plane's (x, y) where the player sees them on the screen
    /// (the frame turned: `screen_to_frame`). Only register mode with the
    /// plane neither turned nor zoomed is drawn yet.
    fn draw_text_plane(&self, out: &mut [u16], pal: &[u32], reg: u32, base_reg: u32) {
        let control = self.latch(0xD050_000C + reg);
        if control & 8 == 0 || control & 3 != 2 {
            return;
        }
        let attr = self.latch(0xD050_0008 + reg);
        let (map, base) = (self.latch(0xD050_0010 + reg), self.latch(0xD050_0000 + base_reg));
        let (tw, th) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
        let (pw, ph) = plane_size((attr >> 14) & 3 | (attr >> 16 & 1) << 2);
        let (xpos, ypos) = (self.latch(0xD050_0000 + reg) & 0xFFF, self.latch(0xD050_0004 + reg) & 0xFFF);
        let depth = attr & 3;
        let nibble = (attr >> 8) & 15;
        let direct = control & 0x80 != 0;
        let bpp = if direct { [16, 16, 32, 16][depth as usize] } else { 2 * (depth + 1) };
        let pal_base = (nibble << 4) >> bpp << bpp;
        let level = (control & 0x300 != 0).then_some(((control >> 10) & 63).min(64));
        for y in 0..crate::SCREEN_H as u32 {
            // The plane's lines from y position 240 down (UNVERIFIED for
            // other positions: only 240 seen).
            let py = (y + 240).wrapping_sub(ypos) % ph;
            let row = map + (py / th) * (pw / tw) * 4;
            for x in 0..crate::SCREEN_W as u32 {
                let px = (x + xpos) % pw;
                let entry = self.read_ram32(row + px / tw * 4);
                if entry == 0 {
                    continue;
                }
                let i = (py % th) * tw + px % tw;
                let at = base.wrapping_add(entry);
                let texel: Option<Texel> = if !direct {
                    let k = (pal_base + self.sprite_pixel(at, bpp, i)) as usize & 1023;
                    (pal[k] & 0x8000 == 0).then(|| (rgb555_to_565(pal[k]), 255))
                } else {
                    match depth {
                        2 => {
                            let p = at + i * 4;
                            let (r, g, b) = (self.peek8(p + 1), self.peek8(p + 2), self.peek8(p + 3));
                            Some((((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16, self.peek8(p)))
                        }
                        1 => Some(((self.peek8(at + i * 2) | self.peek8(at + i * 2 + 1) << 8) as u16, 255)),
                        0 if nibble == 9 || nibble == 0xB => {
                            let v = self.peek8(at + i * 2) | self.peek8(at + i * 2 + 1) << 8;
                            let n = |s: u32| ((v >> s) & 15) * 17;
                            Some((((n(8) >> 3) << 11 | (n(4) >> 2) << 5 | n(0) >> 3) as u16, (v >> 12) * 17))
                        }
                        // ARGB 1:5:5:5 and YUV: not seen yet (UNVERIFIED).
                        0 => {
                            let v = self.peek8(at + i * 2) | self.peek8(at + i * 2 + 1) << 8;
                            Some((rgb555_to_565(v & 0x7FFF), if v & 0x8000 != 0 { 255 } else { 0 }))
                        }
                        _ => None,
                    }
                };
                if let Some((c, alpha)) = texel {
                    let alpha = level.map_or(alpha, |l| alpha * l / 64);
                    if alpha != 0 {
                        let (fx, fy) = screen_to_frame(x, y);
                        plot(out, fx, fy, (c, alpha));
                    }
                }
            }
        }
    }

    fn read_ram32(&self, a: u32) -> u32 {
        self.peek8(a) | self.peek8(a + 1) << 8 | self.peek8(a + 2) << 16 | self.peek8(a + 3) << 24
    }

    fn draw_sprite(&self, out: &mut [u16], pal: &[u32], ext: u32, half: impl Fn(u32) -> u32) {
        let attr = half(3);
        // The picture: 16-byte units from the sprite segment (0x88), as the
        // System's sprite routine 0xF6CC stores it (address minus segment,
        // shifted right 4; bits 22-16 into halfword 4).
        let addr = self.latch(0xD050_0088).wrapping_add((half(0) | (half(4) & 0x7F) << 16) * 16);
        let (w, h) = size(attr, ext);
        // Depth 3: the palette nibble picks a colour mode, as the System's
        // PPU_getSpriteColorMode (0x10608) reads it: with bit 0 clear, 8
        // bits through the palette; 9, 0xB, 0xD, 0xF: 16 bits ARGB 4:4:4:4
        // (its mode 4); 5: 32 bits ARGB 8:8:8:8 (mode 9, with 0x1F8 bit 3
        // on). Modes 5-8 (nibbles 1, 3, 7) are not decoded yet: drawn as 32
        // bits.
        let nibble = (attr >> 8) & 15;
        let mode = if attr & 3 != 3 || nibble & 1 == 0 {
            Mode::Palette
        } else if nibble & 8 != 0 {
            Mode::Argb4444
        } else {
            Mode::Argb8888
        };
        let bpp = match mode {
            Mode::Palette => 2 * ((attr & 3) + 1),
            Mode::Argb4444 => 16,
            Mode::Argb8888 => 32,
        };
        let pal_base = (((attr >> 8) & 15) << 4) >> bpp << bpp;
        // Blending (attribute bit 14, PPU_enableSpriteBlend 0x10088): the
        // level in halfword 4 bits 15-10, 0-63 (PPU_setSpriteBlendLevel
        // 0x100E4; with 0x1FC bit 13 on only bits 13-10, a quarter of it,
        // bits 15-14 being the mosaic then). Taken as the sprite's opacity
        // in 64ths (UNVERIFIED).
        let blend = (attr & 0x4000 != 0).then(|| {
            if self.latch(0xD050_01FC) & 0x2000 != 0 { ((half(4) >> 10) & 15) * 4 } else { half(4) >> 10 }
        });
        // Texel (sx, sy) of the picture, or None where it is transparent.
        let texel = |sx: u32, sy: u32| -> Option<Texel> {
            let i = sy * w + sx;
            let (c, alpha) = match mode {
                Mode::Palette => {
                    let k = (pal_base + self.sprite_pixel(addr, bpp, i)) as usize & 1023;
                    if pal[k] & 0x8000 != 0 {
                        return None;
                    }
                    (rgb555_to_565(pal[k]), 255)
                }
                Mode::Argb4444 => {
                    // Alpha in the top 4 bits (EVIDENCE: the menu's Yes and
                    // No buttons, 0x0FFF around them and purple 0xF74C).
                    let v = self.peek8(addr + i * 2) | self.peek8(addr + i * 2 + 1) << 8;
                    let n = |s: u32| ((v >> s) & 15) * 17;
                    (((n(8) >> 3) << 11 | (n(4) >> 2) << 5 | n(0) >> 3) as u16, (v >> 12) * 17)
                }
                Mode::Argb8888 => {
                    // Bytes alpha, red, green, blue (EVIDENCE: the menu's
                    // icons are opaque blue shapes with soft edges on
                    // transparent white this way).
                    let p = addr + i * 4;
                    let (r, g, b) = (self.peek8(p + 1), self.peek8(p + 2), self.peek8(p + 3));
                    (((r >> 3) << 11 | (g >> 2) << 5 | b >> 3) as u16, self.peek8(p))
                }
            };
            let alpha = blend.map_or(alpha, |level| alpha * level.min(64) / 64);
            (alpha != 0).then_some((c, alpha))
        };
        // Pseudo-3D ("V3D"): with 0x1FC bit 9 on, a sprite whose bits 3-2
        // are not both set is a quad given by its four corners
        // (PPU_isSpriteV3DEnable 0xF660; PPU_setSpriteV3DRect packs them,
        // 0xF774). Both set: an ordinary sprite, not flipped.
        let (mut flip_x, mut flip_y) = (false, false);
        if self.latch(0xD050_01FC) & 0x200 != 0 {
            if attr & 0xC != 0xC {
                self.draw_quad(out, w, h, &half, &texel);
                return;
            }
        } else {
            // Without V3D, flips as on the Zippity, bit 2 across and bit 3
            // up and down (UNVERIFIED here: the System and the games seen
            // so far keep V3D on).
            (flip_x, flip_y) = (attr & 4 != 0, attr & 8 != 0);
        }
        // Rotation (halfword 1 bits 15-10, 64ths of a turn: PPU_setSprite2D
        // RotationAng 0xFBD4) and zoom (halfword 2 bits 15-10: `zoom`), on
        // for every sprite with 0x108 bits 6 and 7, about the centre.
        let control = self.latch(0xD050_0108);
        let rot = if control & 0x40 != 0 { half(1) >> 10 } else { 0 };
        let scale = if control & 0x80 != 0 { zoom(half(2) >> 10) } else { 1.0 };
        let (cx, cy) = (CENTRE_X + coord(half(1)), CENTRE_Y - coord(half(2)));
        if rot == 0 && scale == 1.0 {
            let (x0, y0) = (cx - w as i32 / 2, cy - h as i32 / 2);
            for sy in 0..h {
                for sx in 0..w {
                    let t = texel(if flip_x { w - 1 - sx } else { sx }, if flip_y { h - 1 - sy } else { sy });
                    if let Some(t) = t {
                        plot(out, x0 + sx as i32, y0 + sy as i32, t);
                    }
                }
            }
            return;
        }
        if scale <= 0.0 {
            return;
        }
        // Each screen pixel takes the texel it falls on, turned back and
        // shrunk. Turned anticlockwise as seen (UNVERIFIED).
        let (sin, cos) = (rot as f32 * std::f32::consts::TAU / 64.0).sin_cos();
        let reach = ((w * w + h * h) as f32).sqrt() * scale / 2.0 + 1.0;
        let (fx, fy) = (cx as f32, cy as f32);
        for y in (fy - reach) as i32..=(fy + reach) as i32 {
            for x in (fx - reach) as i32..=(fx + reach) as i32 {
                // From the centre, y up.
                let (dx, dy) = (x as f32 + 0.5 - fx, fy - (y as f32 + 0.5));
                let (u, v) = ((dx * cos + dy * sin) / scale, (dy * cos - dx * sin) / scale);
                let (sx, sy) = (u + w as f32 / 2.0, h as f32 / 2.0 - v);
                if sx < 0.0 || sy < 0.0 || sx as u32 >= w || sy as u32 >= h {
                    continue;
                }
                let (sx, sy) = (sx as u32, sy as u32);
                if let Some(t) = texel(if flip_x { w - 1 - sx } else { sx }, if flip_y { h - 1 - sy } else { sy }) {
                    plot(out, x, y, t);
                }
            }
        }
    }

    /// A V3D sprite: the picture stretched over the quad of its corners,
    /// top left, top right, bottom right, bottom left (VERIFIED order: a
    /// 32 x 32 sprite of Nom-Nom's has its corners 31 apart in that order,
    /// y up). The x coordinates are the low 10 bits of halfwords 1, 5, 6
    /// and 7; the y coordinates the low 10 bits of halfword 2 and the bits
    /// packed above the others (PPU_fastCalculate 0xF774).
    fn draw_quad(
        &self,
        out: &mut [u16],
        w: u32,
        h: u32,
        half: &impl Fn(u32) -> u32,
        texel: &impl Fn(u32, u32) -> Option<Texel>,
    ) {
        let (h1, h2, h5, h6, h7) = (half(1), half(2), half(5), half(6), half(7));
        let xs = [h1, h5, h6, h7].map(coord);
        let y1 = (h1 >> 10) | ((h6 >> 10) & 15) << 6;
        let y2 = (h2 >> 10) | ((h7 >> 10) & 15) << 6;
        let y3 = (h5 >> 10) | (h6 >> 14) << 6 | (h7 >> 14) << 8;
        let ys = [h2, y1, y2, y3].map(coord);
        let p: [(f32, f32); 4] = std::array::from_fn(|i| ((CENTRE_X + xs[i]) as f32, (CENTRE_Y - ys[i]) as f32));
        let (min_x, max_x) = p.iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.0), b.max(q.0)));
        let (min_y, max_y) = p.iter().fold((f32::MAX, f32::MIN), |(a, b), q| (a.min(q.1), b.max(q.1)));
        let cross = |a: (f32, f32), b: (f32, f32)| a.0 * b.1 - a.1 * b.0;
        let sub = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0, a.1 - b.1);
        let (e, f) = (sub(p[1], p[0]), sub(p[3], p[0]));
        let g = (p[0].0 - p[1].0 + p[2].0 - p[3].0, p[0].1 - p[1].1 + p[2].1 - p[3].1);
        let k2 = cross(g, f);
        let inside = |t: f32| (-0.001..=1.001).contains(&t);
        for y in (min_y.floor() as i32).max(0)..=(max_y.ceil() as i32).min(FRAME_H as i32 - 1) {
            for x in (min_x.floor() as i32).max(0)..=(max_x.ceil() as i32).min(FRAME_W as i32 - 1) {
                // Inverse bilinear: where (u, v), each 0-1, puts this pixel.
                let q = sub((x as f32, y as f32), p[0]);
                let k1 = cross(e, f) + cross(q, g);
                let k0 = cross(q, e);
                let v = if k2.abs() < 1e-4 {
                    if k1.abs() < 1e-6 {
                        continue;
                    }
                    -k0 / k1
                } else {
                    let disc = k1 * k1 - 4.0 * k0 * k2;
                    if disc < 0.0 {
                        continue;
                    }
                    let r = disc.sqrt();
                    let a = (-k1 - r) / (2.0 * k2);
                    if inside(a) { a } else { (-k1 + r) / (2.0 * k2) }
                };
                let den = (e.0 + g.0 * v, e.1 + g.1 * v);
                let u = if den.0.abs() > den.1.abs() { (q.0 - f.0 * v) / den.0 } else { (q.1 - f.1 * v) / den.1 };
                if !inside(u) || !inside(v) {
                    continue;
                }
                // The corners are pixels, inclusive: u = 1 is the last column.
                let sx = (u * (w - 1) as f32).round().clamp(0.0, (w - 1) as f32) as u32;
                let sy = (v * (h - 1) as f32).round().clamp(0.0, (h - 1) as f32) as u32;
                if let Some(t) = texel(sx, sy) {
                    plot(out, x, y, t);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Twist;

    /// An 8 x 8, 8-bit sprite at 0x1000: pixel value 1 everywhere but the
    /// first (0, transparent through colour 0's bit 15); colour 1 red.
    fn red_square() -> Twist {
        let mut t = Twist::new(&[]);
        for i in 0..64 {
            t.bus.ram[0x1000 + i] = if i == 0 { 0 } else { 1 };
        }
        t.bus.write(0xD050_1000, 0x8000, 4); // colour 0: transparent
        t.bus.write(0xD050_1004, 0x7C00, 4); // colour 1: red
        t.bus.write(0xD050_01FC, 0x200, 4); // V3D on, as the System keeps it
        t
    }

    #[test]
    fn a_sprite_lands_by_its_centre_in_palette_colours() {
        let mut t = red_square();
        // Entry 0: picture 0x1000 (segment 0, 16-byte units: 0x100), x 0,
        // y 0, depth 3 (8 bits), 8 x 8, bits 3-2 set (an ordinary sprite).
        t.bus.write(0xD050_2000, 0x0000_0100, 4);
        t.bus.write(0xD050_2004, 0x000F_0000, 4);
        t.bus.write(0xD050_0108, 0x0000_0101, 4);
        let mut out = vec![0u16; FRAME_W * FRAME_H];
        t.bus.draw_frame(&mut out);
        let at = |x: usize, y: usize| out[y * FRAME_W + x];
        // Centred on (121, 255): the 8 x 8 square spans 117-124, 251-258.
        assert_eq!(at(117, 251), 0, "the transparent pixel");
        assert_eq!(at(118, 251), 0xF800, "red, as 5:6:5");
        assert_eq!(at(124, 258), 0xF800);
        assert_eq!((at(125, 258), at(124, 259), at(116, 252)), (0, 0, 0));
    }

    #[test]
    fn a_quad_sprite_covers_its_corners_and_zoom_doubles() {
        let mut t = red_square();
        // A quad (bits 3-2 = 0) from (-10, 20) to (5, 5): 16 x 16 pixels,
        // the 8 x 8 picture stretched twice.
        let (x0, y0, x1, y1, x2, y2, x3, y3) = (-10i32, 20i32, 5i32, 20i32, 5i32, 5i32, -10i32, 5i32);
        let m = |v: i32| (v as u32) & 0x3FF;
        let h1 = m(x0) | (m(y1) & 0x3F) << 10;
        let h2 = m(y0) | (m(y2) & 0x3F) << 10;
        let h5 = m(x1) | (m(y3) & 0x3F) << 10;
        let h6 = m(x2) | (m(y3) >> 6 & 3) << 14 | (m(y1) >> 6) << 10;
        let h7 = m(x3) | (m(y3) >> 8) << 14 | (m(y2) >> 6) << 10;
        t.bus.write(0xD050_2000, 0x100 | h1 << 16, 4);
        t.bus.write(0xD050_2004, h2 | 0x0003 << 16, 4);
        t.bus.write(0xD050_2008, h5 << 16, 4);
        t.bus.write(0xD050_200C, h6 | h7 << 16, 4);
        t.bus.write(0xD050_0108, 0x0000_0101, 4);
        let mut out = vec![0u16; FRAME_W * FRAME_H];
        t.bus.draw_frame(&mut out);
        let at = |x: i32, y: i32| out[(255 - y) as usize * FRAME_W + (121 + x) as usize];
        assert_eq!((at(-10, 20), at(-9, 20)), (0, 0), "the transparent texel, stretched");
        assert_eq!((at(-8, 20), at(5, 5), at(-10, 5), at(5, 20)), (0xF800, 0xF800, 0xF800, 0xF800));
        assert_eq!((at(6, 5), at(5, 4), at(-11, 10)), (0, 0, 0));

        // The same picture as an ordinary sprite at (0, 0), zoomed 2x
        // (index 36) with zoom on: 16 x 16 around the centre.
        t.bus.write(0xD050_2000, 0x0000_0100, 4);
        t.bus.write(0xD050_2004, (36 << 10) | 0x000F_0000, 4);
        t.bus.write(0xD050_0108, 0x0000_0181, 4);
        t.bus.draw_frame(&mut out);
        let at = |x: usize, y: usize| out[y * FRAME_W + x];
        assert_eq!((at(113, 247), at(114, 248)), (0, 0), "the transparent texel, 2 x 2");
        assert_eq!((at(115, 247), at(128, 262)), (0xF800, 0xF800));
        assert_eq!((at(112, 250), at(129, 250), at(120, 263)), (0, 0, 0));
    }
}
