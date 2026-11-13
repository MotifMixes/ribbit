//! The video chip (0xD0500000), drawn from its registers and RAM. It is a
//! 32-bit relative of the PPU in GeneralPlus's 16-bit GPL162xx chips:
//! register n of their block at 0x7000 sits at 0xD0500000 + 4n. Only what
//! the system menu and Mickey Mouse Clubhouse use is decoded
//! (docs/zippity.md); all of it is UNVERIFIED on hardware, checked against
//! recordings of real units.

use crate::bus::Bus;

/// The picture's size (VGA mode, PPU-enable bit 4). Sprite coordinates are
/// centred: x to the right and y up from the middle of the screen.
pub const SCREEN_W: usize = 640;
pub const SCREEN_H: usize = 480;
/// A sprite's top line is this minus its y (y up). 255, not 240 (half the
/// screen): the system menus' top band starts at y 255 (line 0), Mickey
/// Mouse Clubhouse's mat picture ends exactly at the bottom line (its last
/// row of sprites holds 9 lines of picture), Mickey stands on his pedestal
/// and the windows' sky meets the hills, as in recordings of real units. At
/// 240 these were 15 lines high (a black band across the windows, the mat's
/// straight bottom edge showing). The game sets 0xD0500358 to 0x400 on one
/// way into it and not on another; it was taken for a 16-line shift before.
const SPRITE_Y0: i32 = 255;

/// A text layer's registers (offsets from 0xD0500000): scroll x and y at
/// +scroll/+scroll+4; attributes at +attr, then control, tile map and
/// attribute map; the picture base at +base; the zoom block, if any.
struct TextLayer {
    scroll: u32,
    attr: u32,
    base: u32,
    zoom: Option<u32>,
}

/// Text layers 1 and 2 (the 16-bit chip's 0x7010 and 0x7016 blocks), then
/// 3 and 4 (0x7000 and 0x7008: two more words between the scroll and the
/// attributes, written 0 so far). Layer 4's picture base is +0x90 (Little
/// Einsteins' Rocket's Musical Mission: its water on layer 4, the base
/// written with layers 1 and 2's); layer 3's is taken to be +0x8C, between
/// the sprites' (+0x88) and layer 4's. No zoom block is known for 3 and 4.
const TEXT_LAYERS: [TextLayer; 4] = [
    TextLayer { scroll: 0x40, attr: 0x48, base: 0x80, zoom: Some(0x340) },
    TextLayer { scroll: 0x58, attr: 0x60, base: 0x84, zoom: Some(0x350) },
    TextLayer { scroll: 0x00, attr: 0x10, base: 0x8C, zoom: None },
    TextLayer { scroll: 0x20, attr: 0x30, base: 0x90, zoom: None },
];

/// A 15-bit colour (bit 15 ignored) as 0x00RRGGBB.
fn rgb555(c: u32) -> u32 {
    let f = |v: u32| ((v & 31) << 3) | ((v & 31) >> 2);
    f(c >> 10) << 16 | f(c >> 5) << 8 | f(c)
}

fn rgb565(c: u32) -> u32 {
    let f = |v: u32| ((v & 31) << 3) | ((v & 31) >> 2);
    let g = (c >> 5) & 63;
    f(c >> 11) << 16 | ((g << 2) | (g >> 4)) << 8 | f(c)
}

/// BT.601 YUV (full range) to 0x00RRGGBB.
fn yuv(y: i32, u: i32, v: i32) -> u32 {
    let c = |x: i32| x.clamp(0, 255) as u32;
    c(y + 1402 * v / 1000) << 16 | c(y - (344 * u + 714 * v) / 1000) << 8 | c(y + 1772 * u / 1000)
}

fn blend(over: u32, under: u32, alpha: u32, full: u32) -> u32 {
    let ch = |s: u32| {
        let (a, b) = ((over >> s) & 0xFF, (under >> s) & 0xFF);
        ((a * alpha + b * (full - alpha)) / full) << s
    };
    ch(16) | ch(8) | ch(0)
}

impl Bus {
    /// The maths unit's matrix product (0xF6004000; its arithmetic is
    /// `Bus::arith`): a row vector times a matrix, 16.16
    /// fixed point. The matrix's rows are at +0x00, +0x10, +0x20 and +0x30
    /// (four words each), the vector at +0x40, the result at +0x60. Writing
    /// 0xF6800004 multiplies by the top-left 3x3: the kernels' 2D
    /// transforms, (x, y, 1) times a matrix whose third row is the
    /// translation, giving a sprite corner's x and y. 0xF6800000 uses the
    /// whole 4x4 (UNVERIFIED: not seen used). With the unit unmodelled the
    /// results read 0, and every sprite placed through it (the game-select
    /// ring's bubbles and titles) went to the corner converter as one point
    /// at the top-left corner.
    pub(crate) fn geometry(&mut self, start: u32) {
        let n = if start == 0xF680_0000 { 4 } else { 3 };
        for j in 0..n {
            let sum: i64 = (0..n)
                .map(|i| self.latch(0xF600_4040 + i * 4) as i32 as i64 * self.latch(0xF600_4000 + i * 16 + j * 4) as i32 as i64)
                .sum();
            self.set_latch(0xF600_4060 + j * 4, (sum >> 16) as u32);
        }
    }

    /// `n` bytes of RAM, SRAM or on-chip RAM from `a` (None when they are
    /// not all in one of them: callers then go byte by byte).
    fn bytes(&self, a: u32, n: usize) -> Option<&[u8]> {
        let a = a as usize;
        if a + n <= crate::bus::RAM_SIZE {
            return Some(&self.ram[a..a + n]);
        }
        let s = a.wrapping_sub(crate::bus::SRAM_BASE as usize);
        if s.checked_add(n).is_some_and(|e| e <= crate::bus::SRAM_SIZE) {
            return Some(&self.sram[s..s + n]);
        }
        let i = a.wrapping_sub(crate::bus::IRAM_BASE as usize);
        if i.checked_add(n).is_some_and(|e| e <= crate::bus::IRAM_SIZE) {
            return Some(&self.iram[i..i + n]);
        }
        None
    }

    /// Pixel `i` of a picture of `bpp` bits per pixel at `addr`: the bits
    /// are one stream, most significant bit first (6-bit sprites show this).
    fn pixel(&self, addr: u32, bpp: u32, i: u32) -> u32 {
        let bit = i * bpp;
        let a = addr + bit / 8;
        let w = (self.peek8(a) as u32) << 16 | (self.peek8(a + 1) as u32) << 8 | self.peek8(a + 2) as u32;
        (w >> (24 - bpp - bit % 8)) & ((1 << bpp) - 1)
    }

    /// Draws a text layer (registers at +reg: +0/+4 scroll, +8 attributes,
    /// +0xC control (bit 3 on), +0x10 tile map (32-bit entries: the tile's
    /// byte offset from the picture base, 0 for none), pages of 1024x512
    /// pixels). Attributes: bits 1-0 depth (only 8 bits decoded), 5-4 and
    /// 7-6 tile width and height (8 << n), 13-12 priority, 23-22 the
    /// palette's 256-colour bank. Control bit 7: 16-bit pixels instead,
    /// RGB 5:6:5 (depth 1) or YUV 4:2:2 (depth 3).
    ///
    /// Each layer also has a zoom (registers at +xf: +0/+4 a centre on
    /// screen, +8 the step in the picture per screen pixel, 0x400 = 1;
    /// +0xC UNKNOWN, 0 so far, perhaps a rotation). The screen pixel
    /// (x, y) shows the picture's pixel (scroll x + (x - centre x) * step,
    /// scroll y + (y - centre y) * step). Games write them with the
    /// layer's other registers, in the order scroll, centre, +0xC, step;
    /// the system and most games leave centre 0 and step 0x400 (or 0, taken
    /// as 0x400). Cinderella's Dress Dash zooms its wall slowly out (step
    /// 0x437 to 0x470 about the centre (392, 240), scroll (493, 288)): drawn
    /// with the scroll alone it showed the empty rows under the wall; this
    /// way it matches a recording (the pillar at x 400, the wall's foot at
    /// y 233). UNVERIFIED beyond that.
    fn draw_text(&self, img: &mut [u32], pal: &[u32], rgb: &[u32], layer: &TextLayer) {
        let TextLayer { scroll: reg, attr: attr_reg, base: base_reg, zoom } = *layer;
        let attr = self.latch(0xD050_0000 + attr_reg);
        let control = self.latch(0xD050_0004 + attr_reg);
        if control & 8 == 0 || (attr & 3 != 3 && control & 0x80 == 0) {
            return;
        }
        let (map, base) = (self.latch(0xD050_0008 + attr_reg), self.latch(0xD050_0000 + base_reg));
        let (tw, th) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
        let bank = ((attr >> 22) & 3) * 0x100;
        let (sx, sy) = (self.latch(0xD050_0000 + reg) as i32, self.latch(0xD050_0004 + reg) as i32);
        let zoom = |k: u32| zoom.map_or(0, |xf| self.latch(0xD050_0000 + xf + k));
        let (cx, cy) = (zoom(0) as i16 as i32, zoom(4) as i16 as i32);
        let step = match zoom(8) & 0xFFFF {
            0 => 0x400,
            s => s as i32,
        };
        let at = |screen: i32, scroll: i32, centre: i32, size: i32| (scroll + ((screen - centre) * step >> 10)).rem_euclid(size) as u32;
        let wide = control & 0x80 != 0;
        for y in 0..SCREEN_H as u32 {
            let py = at(y as i32, sy, cy, 512);
            let row = map + (py / th) * (1024 / tw) * 4;
            let line = &mut img[y as usize * SCREEN_W..(y as usize + 1) * SCREEN_W];
            // A tile's run of pixels at a time (tiles never straddle the
            // 1024-pixel wrap: 1024 is a multiple of their width).
            let mut x = 0;
            while x < SCREEN_W as u32 {
                let px = at(x as i32, sx, cx, 1024);
                // Zoomed: a pixel at a time.
                let span = if step == 0x400 { (tw - px % tw).min(SCREEN_W as u32 - x) } else { 1 };
                let entry = self.peek32(row + px / tw * 4);
                if entry != 0 && wide {
                    // Control bit 7: 16-bit pixels, the colour itself (a
                    // tile entry of 0 is none). Depth 1: RGB 5:6:5, which
                    // matches a real unit's Disney Princess menu (blue at
                    // the top, pink and lavender below; 5:5:5 and YUV do
                    // not), 0 transparent as in 16-bit sprites: Cinderella's
                    // Dress Dash puts its floor on layer 2 over the wall on
                    // layer 1, the floor's upper rows all 0 (drawn black
                    // they hid the wall a recording shows). Depth 3: YUV 4:2:2, a
                    // pixel pair in 4 bytes V Y0 U Y1, as The Princess and
                    // the Frog's menu (as RGB its picture is noise). The
                    // order of U and V is UNVERIFIED: this one gives a night
                    // sky (the other a red sunset); no recording of a real
                    // unit found.
                    let p0 = (py % th) * tw + px % tw;
                    let src = base + entry + p0 * 2;
                    for i in 0..span {
                        if attr & 3 == 3 {
                            let pair = base + entry + ((p0 + i) & !1) * 2;
                            let y = self.peek8(pair + 1 + 2 * ((p0 + i) & 1)) as i32;
                            let (u, v) = (self.peek8(pair + 2) as i32 - 128, self.peek8(pair) as i32 - 128);
                            line[(x + i) as usize] = yuv(y, u, v);
                        } else {
                            let c = self.peek16(src + i * 2);
                            if c != 0 {
                                line[(x + i) as usize] = rgb565(c);
                            }
                        }
                    }
                } else if entry != 0 {
                    let src = base + entry + (py % th) * tw + px % tw;
                    let put = |line: &mut [u32], i: u32, v: u8| {
                        let k = (bank + v as u32) as usize & 2047;
                        if pal[k] & 0x8000 == 0 {
                            line[(x + i) as usize] = rgb[k];
                        }
                    };
                    match self.bytes(src, span as usize) {
                        Some(run) => (0..span).for_each(|i| put(line, i, run[i as usize])),
                        None => (0..span).for_each(|i| put(line, i, self.peek8(src + i))),
                    }
                }
                x += span;
            }
        }
    }

    /// Draws the frame's picture now (see the video DMA in bus.rs).
    pub(crate) fn redraw(&mut self) {
        let mut img = std::mem::take(&mut self.picture);
        img.resize(SCREEN_W * SCREEN_H, 0);
        self.draw(&mut img);
        self.picture = img;
        self.picture_at = Some(self.now);
    }

    /// The picture, as 0x00RRGGBB pixels, SCREEN_W x SCREEN_H.
    pub(crate) fn render(&self, img: &mut [u32]) {
        self.draw(img);
        self.apply_fade(img);
    }

    /// Darkens a picture by the fade (0xD05000C0 bits 7-0, 0xFF black).
    /// It is applied as the picture goes out, not when it is drawn: the
    /// kernels step it each field between screens, often without sending
    /// a new sprite list.
    pub(crate) fn apply_fade(&self, img: &mut [u32]) {
        let fade = self.fade();
        if fade != 0 {
            let keep = 255 - fade;
            for p in img.iter_mut() {
                let ch = |s: u32| ((*p >> s) & 0xFF) * keep / 255 << s;
                *p = ch(16) | ch(8) | ch(0);
            }
        }
    }

    /// The picture without the fade.
    fn draw(&self, img: &mut [u32]) {
        img.fill(0);
        // Palette RAM (0xD0501000): 16-bit colours (0RRRRRGGGGGBBBBB, bit 15
        // transparent), two per word.
        let pal: Vec<u32> = (0..2048u32).map(|i| (self.latch(0xD050_1000 + i / 2 * 4) >> (16 * (i & 1))) & 0xFFFF).collect();
        let rgb: Vec<u32> = pal.iter().map(|&c| rgb555(c)).collect();

        // Sprites (control +0x108: bit 0 on, bits 15-8 how many in fours):
        // entries of 32 bytes at 0xD0502000. Halfword 0 and byte 8 bits 6-0:
        // the picture's address in halfwords; halfwords 1 and 2: x and y;
        // halfword 3: attributes (bits 1-0 depth 2/4/6/8 bits, 2/3 flips,
        // 5-4 width and 7-6 height 8 << n, 11-8 palette, 13-12 priority, 15
        // palette bank bit 1); byte 8 bit 7: palette bank bit 0. Colours
        // come from 0x100 x bank.
        let control = self.latch(0xD050_0108);
        let count = if control & 1 != 0 { ((control >> 8) & 0xFF) * 4 } else { 0 };
        let half = |n: u32, k: u32| (self.latch(0xD050_2000 + n * 32 + k / 2 * 4) >> (16 * (k & 1))) & 0xFFFF;
        // Picture number 0 (halfword 0) is no sprite, whatever the address
        // bits in byte 8 (as in the GPL162xx; left-over entries keep them
        // and drew pink boxes on the controller screen).
        let mut order: Vec<(u32, u32)> = (0..count.min(512))
            .filter(|&n| half(n, 0) != 0)
            .map(|n| ((half(n, 3) >> 12) & 3, n))
            .collect();
        order.sort_unstable();
        // Layers are drawn by priority, 0 first: at each, the text layers
        // and then the sprites. Text layers 1 and 2 have registers at +0x40
        // and +0x58 and picture bases at +0x80 and +0x84.
        for d in 0..4 {
            for layer in &TEXT_LAYERS {
                if (self.latch(0xD050_0000 + layer.attr) >> 12) & 3 == d && !self.probe.hide_text {
                    self.draw_text(img, &pal, &rgb, layer);
                }
            }
            for &(_, n) in order.iter().filter(|o| o.0 == d && !self.probe.hide_sprites) {
                self.draw_sprite(img, &pal, &rgb, |k| half(n, k));
            }
        }
    }

    /// The fade shown (0xD05000C0 bits 7-0, 0xFF black): the register's
    /// value when the last frame came up (see `Bus::frame`). The kernels
    /// step the register by 0x10 from the frame interrupt, which comes only
    /// while a frame is on its way, so a fade started while the kernel loads
    /// waits, at black, for the new screen's first frame and then runs over
    /// it (the start-up warning, a mat picture), and a fade written while no
    /// frames come (LOADING during a program load) leaves the picture alone.
    pub(crate) fn fade(&self) -> u32 {
        self.shown_fade
    }

    /// Research: the sprites in use, one line each (number, the eight
    /// halfwords of the entry's first 16 bytes, where it lands on screen).
    pub(crate) fn sprite_list(&self) -> Vec<String> {
        let control = self.latch(0xD050_0108);
        let count = if control & 1 != 0 { ((control >> 8) & 0xFF) * 4 } else { 0 };
        let half = |n: u32, k: u32| (self.latch(0xD050_2000 + n * 32 + k / 2 * 4) >> (16 * (k & 1))) & 0xFFFF;
        (0..count.min(512))
            .filter(|&n| half(n, 0) != 0)
            .map(|n| {
                let h: Vec<String> = (0..16).map(|k| format!("{:04x}", half(n, k))).collect();
                let attr = half(n, 3);
                let x0 = SCREEN_W as i32 / 2 + half(n, 1) as i16 as i32;
                let y0 = SPRITE_Y0 - half(n, 2) as i16 as i32;
                let (w, hh) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
                format!("{n:3} {}  at ({x0},{y0}) {w}x{hh} depth {} pal {} prio {}", h.join(" "), attr & 3, (attr >> 8) & 15, (attr >> 12) & 3)
            })
            .collect()
    }

    fn draw_sprite(&self, img: &mut [u32], pal: &[u32], rgb: &[u32], half: impl Fn(u32) -> u32) {
        let attr = half(3);
        let addr = (half(0) | (half(4) & 0x7F) << 16) * 2;
        let x0 = SCREEN_W as i32 / 2 + half(1) as i16 as i32;
        let y0 = SPRITE_Y0 - half(2) as i16 as i32;
        let (w, h) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
        // The size on screen: the corner converter's third corner (this
        // crate's own encoding, see the converter in bus.rs), or the
        // picture's.
        let (sw, sh) = if half(7) == 0x8000 {
            let x2 = SCREEN_W as i32 / 2 + half(5) as i16 as i32;
            let y2 = SPRITE_Y0 - half(6) as i16 as i32;
            ((x2 - x0 + 1).max(0) as u32, (y2 - y0 + 1).max(0) as u32)
        } else {
            (w, h)
        };
        // Depth 8 with palette bit 0 set: 16 bits per pixel. With palette
        // bit 1 also set (0x23F3, 0x33F3) each pixel is a colour, RGB 5:6:5,
        // 0 transparent: the game-select ring's bubbles, the menus' buttons
        // and the games' pause panels (smooth blue gradients this way; as
        // 5:5:5 or as palette colours they came out banded and in false
        // colours). Otherwise (0x29F3, 0x99F3) a pixel has bit 14 set and
        // is a colour from the bank (bits 7-0) with an opacity (bits 13-8,
        // 0-63): the menus' background art, and the ring's reflections,
        // which fade from 0x19 to 0 downwards (a recording shows them
        // faint under each bubble).
        let wide = attr & 3 == 3 && attr & 0x100 != 0;
        let rgb16 = wide && attr & 0x200 != 0;
        let bpp = if wide { 16 } else { 2 * ((attr & 3) + 1) };
        // Attribute bit 14: the whole sprite is blended over what is below
        // at the level in byte 9's bits 5-0 (0-63; the kernel's sprite
        // routine at 0x2F2B8 sets it, 0x3F for most sprites). The games'
        // pause screens dim the picture with a 64x64 sprite of one dark
        // colour stretched over the whole screen at level 0x2A, as
        // recordings show (the scene still visible, darker); drawn solid it
        // hid the picture.
        let level = if attr & 0x4000 != 0 { (half(4) >> 8) & 63 } else { 63 };
        // Words 4-7 (bytes 16-31): bit 31 set in every sprite seen but one,
        // a full-screen 64x64 sprite of picture address 1 in Little
        // Einsteins' Rocket's Musical Mission, whose four words are the
        // colours 0x0890F8, 0x08508A, 0x083862, 0x083862 (0x00RRGGBB: blues
        // from light to dark). A recording shows that scene's water there,
        // light blue fading darker downwards; drawn as a picture it was the
        // kernel's code as noise. Such a sprite is taken as filled with its
        // corners' colours (in the corner converter's order: top left, top
        // right, bottom right, bottom left), shaded between them. UNVERIFIED
        // beyond that sprite (whether a picture would be tinted is UNKNOWN).
        let corner = |k: u32| half(8 + 2 * k) | half(9 + 2 * k) << 16;
        if corner(0) & 0x8000_0000 == 0 {
            let c: Vec<[i32; 3]> = (0..4).map(|k| { let v = corner(k); [(v >> 16 & 255) as i32, (v >> 8 & 255) as i32, (v & 255) as i32] }).collect();
            let (fw, fh) = ((sw.max(2) - 1) as i32, (sh.max(2) - 1) as i32);
            for sy in 0..sh as i32 {
                let y = y0 + sy;
                if !(0..SCREEN_H as i32).contains(&y) {
                    continue;
                }
                for sx in 0..sw as i32 {
                    let x = x0 + sx;
                    if !(0..SCREEN_W as i32).contains(&x) {
                        continue;
                    }
                    let mix = |i: usize| {
                        let top = c[0][i] * (fw - sx) + c[1][i] * sx;
                        let bottom = c[3][i] * (fw - sx) + c[2][i] * sx;
                        ((top * (fh - sy) + bottom * sy) / (fw * fh)) as u32
                    };
                    let colour = mix(0) << 16 | mix(1) << 8 | mix(2);
                    let o = y as usize * SCREEN_W + x as usize;
                    img[o] = if level >= 63 { colour } else { blend(colour, img[o], level, 63) };
                }
            }
            return;
        }
        let bank = ((attr >> 15) << 1 | ((half(4) >> 7) & 1)) * 0x100;
        let pal_base = bank + ((((attr >> 8) & 15) << 4) >> bpp << bpp);
        for sy in 0..sh {
            let y = y0 + if attr & 8 != 0 { (sh - 1 - sy) as i32 } else { sy as i32 };
            if y < 0 || y >= SCREEN_H as i32 {
                continue;
            }
            let py = sy * h / sh;
            // The picture's row, when it is all in one memory (8 and 16 bits
            // per pixel: a byte or halfword a pixel).
            let row = match bpp {
                8 => self.bytes(addr + py * w, w as usize),
                16 => self.bytes(addr + py * w * 2, w as usize * 2),
                _ => None,
            };
            for sx in 0..sw {
                let x = x0 + if attr & 4 != 0 { (sw - 1 - sx) as i32 } else { sx as i32 };
                if x < 0 || x >= SCREEN_W as i32 {
                    continue;
                }
                let px = sx * w / sw;
                let o = y as usize * SCREEN_W + x as usize;
                // The pixel's colour and opacity (0-63).
                let (colour, alpha) = if !wide {
                    let v = match row {
                        Some(r) => r[px as usize] as u32,
                        None => self.pixel(addr, bpp, py * w + px),
                    };
                    let k = (pal_base + v) as usize & 2047;
                    if pal[k] & 0x8000 != 0 {
                        continue;
                    }
                    (rgb[k], 63)
                } else {
                    let v = match row {
                        Some(r) => u16::from_le_bytes([r[px as usize * 2], r[px as usize * 2 + 1]]) as u32,
                        None => self.peek16(addr + (py * w + px) * 2),
                    };
                    if v == 0 && (rgb16 || v & 0x4000 == 0) {
                        continue;
                    }
                    if rgb16 {
                        (rgb565(v), 63)
                    } else if v & 0x4000 == 0 {
                        // Not seen in palette-and-opacity sprites (UNKNOWN):
                        // taken as a colour, RGB 5:5:5.
                        (rgb555(v), 63)
                    } else {
                        let raw = pal[(bank + (v & 0xFF)) as usize];
                        if raw & 0x8000 != 0 {
                            continue;
                        }
                        (rgb555(raw), (v >> 8) & 63)
                    }
                };
                let alpha = alpha * level / 63;
                img[o] = if alpha >= 63 { colour } else { blend(colour, img[o], alpha, 63) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPRITES: u32 = 0xD050_2000;

    /// A bus with sprites on (`count` entries) and nothing else drawn.
    fn bus(count: u32) -> Bus {
        let mut b = Bus::new(Vec::new());
        b.set_latch(0xD050_0108, 1 | (count.div_ceil(4)) << 8);
        b
    }

    /// Sprite `n`: picture at `addr` (bytes), top-left corner at (x, y) on
    /// screen, attributes, byte 9.
    fn sprite(b: &mut Bus, n: u32, addr: u32, x: i32, y: i32, attr: u32, byte9: u32) {
        let half = addr / 2;
        let e = SPRITES + n * 32;
        b.set_latch(e, (half & 0xFFFF) | (((x - SCREEN_W as i32 / 2) as u32 & 0xFFFF) << 16));
        b.set_latch(e + 4, ((SPRITE_Y0 - y) as u32 & 0xFFFF) | attr << 16);
        b.set_latch(e + 8, (half >> 16) & 0x7F | byte9 << 8);
        // Byte 19 bit 7, set in every sprite the kernels send (clear: a
        // colour-filled sprite).
        b.set_latch(e + 16, 0x8000_0000);
    }

    fn palette(b: &mut Bus, i: u32, c: u32) {
        let a = 0xD050_1000 + i / 2 * 4;
        let shift = 16 * (i & 1);
        b.set_latch(a, b.latch(a) & !(0xFFFF << shift) | c << shift);
    }

    fn fill16(b: &mut Bus, addr: u32, n: usize, v: u16) {
        for i in 0..n {
            b.ram[addr as usize + 2 * i..][..2].copy_from_slice(&v.to_le_bytes());
        }
    }

    fn draw(b: &Bus) -> Vec<u32> {
        let mut img = vec![0; SCREEN_W * SCREEN_H];
        b.render(&mut img);
        img
    }

    /// 16-bit sprites with palette bits 1 and 0 set are RGB 5:6:5, 0
    /// transparent (the pause panels' and the ring bubbles' format).
    #[test]
    fn rgb565_sprite() {
        let mut b = bus(1);
        fill16(&mut b, 0x1000, 8 * 8, 0x067F);
        b.ram[0x1000..0x1002].copy_from_slice(&0u16.to_le_bytes());
        sprite(&mut b, 0, 0x1000, 100, 50, 0x0303, 0x3F);
        let img = draw(&b);
        assert_eq!(img[50 * SCREEN_W + 100], 0, "colour 0 is transparent");
        assert_eq!(img[50 * SCREEN_W + 101], rgb565(0x067F));
        assert_eq!(img[50 * SCREEN_W + 101], 0x00CF_FF);
    }

    /// Other 16-bit sprites: a palette colour with an opacity in bits
    /// 13-8, in every bank; transparent palette colours stay transparent.
    #[test]
    fn palette_opacity_sprite() {
        let mut b = bus(1);
        palette(&mut b, 0x05, 0x7FFF);
        palette(&mut b, 0x06, 0xFC1F);
        fill16(&mut b, 0x1000, 8 * 8, 0x4000 | 0x3F << 8 | 0x05);
        b.ram[0x1002..0x1004].copy_from_slice(&(0x4000u16 | 0x1F << 8 | 0x05).to_le_bytes());
        b.ram[0x1004..0x1006].copy_from_slice(&(0x4000u16 | 0x3F << 8 | 0x06).to_le_bytes());
        sprite(&mut b, 0, 0x1000, 10, 10, 0x0103, 0x3F);
        let img = draw(&b);
        assert_eq!(img[10 * SCREEN_W + 10], 0xFF_FFFF);
        assert_eq!(img[10 * SCREEN_W + 11], blend(0xFF_FFFF, 0, 0x1F, 63));
        assert_eq!(img[10 * SCREEN_W + 12], 0, "a transparent palette colour");
    }

    /// Attribute bit 14: the sprite is blended at the level in byte 9 (the
    /// games' pause screens dim the picture this way).
    #[test]
    fn sprite_blend_level() {
        let mut b = bus(2);
        palette(&mut b, 1, 0x7C00);
        b.ram[0x1000..0x1040].fill(1);
        fill16(&mut b, 0x2000, 64, 0x0861);
        sprite(&mut b, 0, 0x1000, 0, 0, 0x0003, 0x3F);
        sprite(&mut b, 1, 0x2000, 0, 0, 0x4303, 0x2A);
        let img = draw(&b);
        assert_eq!(img[0], blend(rgb565(0x0861), 0xFF_0000, 0x2A, 63));
    }

    /// The geometry unit: (x, y, 1) times a 3x3 matrix whose third row is
    /// the translation, in 16.16.
    #[test]
    fn geometry_unit() {
        let mut b = Bus::new(Vec::new());
        let m = [0x8000, 0, 0, 0, 0, 0x8000, 0, 0, 0xF0_0000, 0x3E_0000, 0x1_0000];
        for (i, v) in m.iter().enumerate() {
            b.set_latch(0xF600_4000 + 4 * i as u32, *v);
        }
        for (i, v) in [0x40_0000u32, 0x20_0000, 0x1_0000].iter().enumerate() {
            b.set_latch(0xF600_4040 + 4 * i as u32, *v);
        }
        b.io(0xF680_0004, Some(1), 32);
        assert_eq!(b.io(0xF600_4060, None, 32), (0x20 + 0xF0) << 16);
        assert_eq!(b.io(0xF600_4064, None, 32), (0x10 + 0x3E) << 16);
        assert_eq!(b.io(0xF600_4068, None, 32), 0x1_0000);
    }

    /// A layer's zoom: screen pixel x shows the picture's pixel scroll +
    /// (x - centre) * step / 0x400.
    #[test]
    fn layer_zoom() {
        let mut b = Bus::new(Vec::new());
        // Layer 1: RGB 5:6:5 tiles of 8x8, the map at 0x100, the tile at
        // base 0x10000 + 0x800, its pixels numbered 1 to 64.
        b.set_latch(0xD050_0048, 0x0001);
        b.set_latch(0xD050_004C, 0x88);
        b.set_latch(0xD050_0050, 0x100);
        b.set_latch(0xD050_0080, 0x10000);
        b.ram[0x100..0x104].copy_from_slice(&0x800u32.to_le_bytes());
        for i in 0..64u16 {
            b.ram[0x10800 + 2 * i as usize..][..2].copy_from_slice(&(i + 1).to_le_bytes());
        }
        let px = |b: &Bus, x: usize, y: usize| draw(b)[y * SCREEN_W + x];
        assert_eq!(px(&b, 3, 2), rgb565(2 * 8 + 3 + 1), "no zoom: step 0 counts as 0x400");
        // Twice the step about (2, 1), scrolled by (1, 0).
        b.set_latch(0xD050_0040, 1);
        b.set_latch(0xD050_0340, 2);
        b.set_latch(0xD050_0344, 1);
        b.set_latch(0xD050_0348, 0x800);
        assert_eq!(px(&b, 3, 2), rgb565(2 * 8 + 3 + 1));
        assert_eq!(px(&b, 4, 3), rgb565(4 * 8 + 5 + 1));
    }

    /// Text layer 4: scroll at +0x20, attributes, control and tile map at
    /// +0x30/+0x34/+0x38, picture base at +0x90 (Rocket's Musical Mission's
    /// seabed).
    #[test]
    fn text_layer_4() {
        let mut b = Bus::new(Vec::new());
        b.set_latch(0xD050_0030, 0x0001 | 1 << 12);
        b.set_latch(0xD050_0034, 0x88);
        b.set_latch(0xD050_0038, 0x100);
        b.set_latch(0xD050_0090, 0x10000);
        b.set_latch(0xD050_0020, 1);
        b.ram[0x100..0x104].copy_from_slice(&0x800u32.to_le_bytes());
        for i in 0..64u16 {
            b.ram[0x10800 + 2 * i as usize..][..2].copy_from_slice(&(i + 1).to_le_bytes());
        }
        assert_eq!(draw(&b)[2 * SCREEN_W + 3], rgb565(2 * 8 + 4 + 1));
    }

    /// A sprite whose word 4 has bit 31 clear is filled with the colours in
    /// words 4-7 (top left, top right, bottom right, bottom left), shaded
    /// across the rectangle the corner converter gave it.
    #[test]
    fn colour_filled_sprite() {
        let mut b = bus(1);
        sprite(&mut b, 0, 2, 0, 0, 0x00F3, 0);
        let e = SPRITES;
        // The converter's third corner: (99, 49) on screen.
        b.set_latch(e + 8, b.latch(e + 8) | ((99 - SCREEN_W as i32 / 2) as u32 & 0xFFFF) << 16);
        b.set_latch(e + 12, (SPRITE_Y0 - 49) as u32 & 0xFFFF | 0x8000 << 16);
        for (k, c) in [0x0000FF, 0x0000FF, 0xFF0000, 0xFF0000u32].iter().enumerate() {
            b.set_latch(e + 16 + 4 * k as u32, *c);
        }
        let img = draw(&b);
        assert_eq!(img[0], 0x0000FF);
        assert_eq!(img[49 * SCREEN_W + 99], 0xFF0000);
        let mid = img[24 * SCREEN_W + 50];
        assert!((0x70..0x90).contains(&(mid >> 16)) && (0x70..0x90).contains(&(mid & 0xFF)), "{mid:06x}");
        assert_eq!(img[50 * SCREEN_W + 50], 0, "nothing below the rectangle");
    }
}
