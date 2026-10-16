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
    fn draw_text(&self, img: &mut [u32], pal: &[u32], rgb: &[u32], reg: u32, base_reg: u32) {
        let attr = self.latch(0xD050_0008 + reg);
        let control = self.latch(0xD050_000C + reg);
        if control & 8 == 0 || (attr & 3 != 3 && control & 0x80 == 0) {
            return;
        }
        let (map, base) = (self.latch(0xD050_0010 + reg), self.latch(0xD050_0000 + base_reg));
        let (tw, th) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
        let bank = ((attr >> 22) & 3) * 0x100;
        let (sx, sy) = (self.latch(0xD050_0000 + reg), self.latch(0xD050_0004 + reg));
        let wide = control & 0x80 != 0;
        for y in 0..SCREEN_H as u32 {
            let py = (y + sy) % 512;
            let row = map + (py / th) * (1024 / tw) * 4;
            let line = &mut img[y as usize * SCREEN_W..(y as usize + 1) * SCREEN_W];
            // A tile's run of pixels at a time (tiles never straddle the
            // 1024-pixel wrap: 1024 is a multiple of their width).
            let mut x = 0;
            while x < SCREEN_W as u32 {
                let px = (x + sx) % 1024;
                let span = (tw - px % tw).min(SCREEN_W as u32 - x);
                let entry = self.peek32(row + px / tw * 4);
                if entry != 0 && wide {
                    // Control bit 7: 16-bit pixels, the colour itself (no
                    // transparent colour; a tile entry of 0 is none). Depth
                    // 1: RGB 5:6:5, which matches a real unit's Disney
                    // Princess menu (blue at the top, pink and lavender
                    // below; 5:5:5 and YUV do not). Depth 3: YUV 4:2:2, a
                    // pixel pair in 4 bytes V Y0 U Y1, as The Princess and
                    // the Frog's menu (as RGB its picture is noise). The
                    // order of U and V is UNVERIFIED: this one gives a night
                    // sky (the other a red sunset); no recording of a real
                    // unit found.
                    let p0 = (py % th) * tw + px % tw;
                    let src = base + entry + p0 * 2;
                    for i in 0..span {
                        line[(x + i) as usize] = if attr & 3 == 3 {
                            let pair = base + entry + ((p0 + i) & !1) * 2;
                            let y = self.peek8(pair + 1 + 2 * ((p0 + i) & 1)) as i32;
                            let (u, v) = (self.peek8(pair + 2) as i32 - 128, self.peek8(pair) as i32 - 128);
                            yuv(y, u, v)
                        } else {
                            rgb565(self.peek16(src + i * 2))
                        };
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

    /// The picture, as 0x00RRGGBB pixels, SCREEN_W x SCREEN_H.
    pub(crate) fn render(&self, img: &mut [u32]) {
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
            for (reg, base_reg) in [(0x40, 0x80), (0x58, 0x84)] {
                if (self.latch(0xD050_0008 + reg) >> 12) & 3 == d && !self.probe.hide_text {
                    self.draw_text(img, &pal, &rgb, reg, base_reg);
                }
            }
            for &(_, n) in order.iter().filter(|o| o.0 == d && !self.probe.hide_sprites) {
                self.draw_sprite(img, &pal, &rgb, |k| half(n, k));
            }
        }
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
                let y0 = SCREEN_H as i32 / 2 - half(n, 2) as i16 as i32;
                let (w, hh) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
                format!("{n:3} {}  at ({x0},{y0}) {w}x{hh} depth {} pal {} prio {}", h.join(" "), attr & 3, (attr >> 8) & 15, (attr >> 12) & 3)
            })
            .collect()
    }

    fn draw_sprite(&self, img: &mut [u32], pal: &[u32], rgb: &[u32], half: impl Fn(u32) -> u32) {
        let attr = half(3);
        let addr = (half(0) | (half(4) & 0x7F) << 16) * 2;
        let x0 = SCREEN_W as i32 / 2 + half(1) as i16 as i32;
        let y0 = SCREEN_H as i32 / 2 - half(2) as i16 as i32;
        let (w, h) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
        // The size on screen: the corner converter's third corner (this
        // crate's own encoding, see the converter in bus.rs), or the
        // picture's.
        // 0xD0500358 moves sprites down by its value / 64 lines (0x400: 16).
        // Mickey Mouse Clubhouse sets 0x400 and the system menus 0; with it,
        // Mickey stands on his pedestal and the windows' sky meets the hills,
        // as in recordings, where without it they float 16 lines high and a
        // black band crosses the windows. Its real meaning is UNVERIFIED.
        let dy = (self.latch(0xD050_0358) as i32) >> 6;
        let y0 = y0 + dy;
        let (sw, sh) = if half(7) == 0x8000 {
            let x2 = SCREEN_W as i32 / 2 + half(5) as i16 as i32;
            let y2 = SCREEN_H as i32 / 2 - half(6) as i16 as i32 + dy;
            ((x2 - x0 + 1).max(0) as u32, (y2 - y0 + 1).max(0) as u32)
        } else {
            (w, h)
        };
        // Depth 8 with palette bit 0 set: 16 bits per pixel. A pixel with
        // bit 14 set is a colour (bits 7-0, from the bank) and an opacity;
        // otherwise it is the colour itself, RGB 5:5:5, 0 transparent.
        let wide = attr & 3 == 3 && attr & 0x100 != 0;
        let bpp = if wide { 16 } else { 2 * ((attr & 3) + 1) };
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
                if !wide {
                    let v = match row {
                        Some(r) => r[px as usize] as u32,
                        None => self.pixel(addr, bpp, py * w + px),
                    };
                    let k = (pal_base + v) as usize & 2047;
                    if pal[k] & 0x8000 == 0 {
                        img[o] = rgb[k];
                    }
                    continue;
                }
                let v = match row {
                    Some(r) => u16::from_le_bytes([r[px as usize * 2], r[px as usize * 2 + 1]]) as u32,
                    None => self.peek16(addr + (py * w + px) * 2),
                };
                if v & 0x4000 == 0 {
                    // Direct colour (bit 15 is set in some light pixels; its
                    // meaning is UNKNOWN).
                    if v != 0 {
                        img[o] = rgb555(v);
                    }
                    continue;
                }
                // The opacity's encoding depends on the bank: in bank 2,
                // bits 13-8 (0-63; the menus' background art, which fades
                // through it); in the others, opaque unless bit 13 is set,
                // then bits 12-8 (0-31; game pictures are solid in
                // recordings with bit 13 clear). UNVERIFIED.
                let raw = pal[(bank + (v & 0xFF)) as usize];
                let (alpha, full) = if bank == 0x200 {
                    ((v >> 8) & 63, 63)
                } else if raw & 0x8000 != 0 {
                    continue;
                } else if v & 0x2000 != 0 {
                    ((v >> 8) & 31, 31)
                } else {
                    (1, 1)
                };
                img[o] = blend(rgb555(raw), img[o], alpha, full);
            }
        }
    }
}
