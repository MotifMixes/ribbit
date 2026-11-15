//! The SPG2xx video unit (word addresses 0x2800-0x2fff): two tile layers
//! ("pages"), 64 sprites on the SPG28x, a 256-colour RGB555 palette (bit 15
//! set: transparent) and four priority levels, drawn back to front as page
//! 1, page 2, sprites for each level. The registers and the drawing follow
//! MAME's spg2xx_video / spg_renderer (BSD-3-Clause; see
//! THIRD-PARTY-NOTICES.txt), drawn here a scanline at a time.

use serde::{Deserialize, Serialize};

pub const SCREEN_W: usize = 320;
pub const SCREEN_H: usize = 240;
/// Lines in an NTSC frame.
pub const LINES: u32 = 262;
pub const SPRITES: usize = 64;

/// Video interrupt bits (status 0x63, enable 0x62): vertical blank, the
/// set screen position, sprite DMA done.
pub const IRQ_VBLANK: u16 = 1;
pub const IRQ_POS: u16 = 2;
pub const IRQ_DMA: u16 = 4;

const BLEND_LEVELS: [u32; 4] = [0x08, 0x10, 0x18, 0x20];

#[derive(Clone, Serialize, Deserialize)]
pub struct Video {
    pub regs: Vec<u16>,
    pub scroll: Vec<u16>,
    pub hcomp: Vec<u16>,
    pub palette: Vec<u16>,
    pub sprites: Vec<u16>,
    /// The picture being drawn, a line at a time (0x00RRGGBB).
    #[serde(skip, default = "blank_frame")]
    pub frame: Vec<u32>,
    #[serde(skip, default = "blank_lut")]
    lut: Vec<u32>,
    #[serde(skip, default = "dirty")]
    lut_dirty: bool,
}

fn blank_frame() -> Vec<u32> {
    vec![0; SCREEN_W * SCREEN_H]
}

fn blank_lut() -> Vec<u32> {
    vec![0; 0x8000]
}

fn dirty() -> bool {
    true
}

impl Video {
    pub fn new() -> Self {
        let mut v = Video {
            regs: vec![0; 0x100],
            scroll: vec![0; 0x100],
            hcomp: vec![0; 0x100],
            palette: vec![0; 0x100],
            sprites: vec![0; 0x400],
            frame: blank_frame(),
            lut: blank_lut(),
            lut_dirty: true,
        };
        v.regs[0x36] = 0xffff;
        v.regs[0x37] = 0xffff;
        v.regs[0x3c] = 0x0020;
        v.regs[0x42] = 0x0001;
        v
    }

    /// Reads a register; `line` is the current scanline. Interrupt state
    /// is the caller's (it owns the lines).
    pub fn read_reg(&self, reg: usize, line: u32) -> u16 {
        match reg {
            0x38 => line as u16,
            _ => self.regs[reg],
        }
    }

    /// Writes a register. Returns a sprite DMA's (source, dest, length) to
    /// do, as that reads main memory.
    pub fn write_reg(&mut self, reg: usize, data: u16) -> Option<(u32, usize, usize)> {
        match reg {
            0x10 | 0x16 => self.regs[reg] = data & 0x01ff,
            0x11 | 0x17 => self.regs[reg] = data & 0x00ff,
            0x2a => self.regs[reg] = data & 3,
            0x30 => {
                self.regs[reg] = data & 0xff;
                self.lut_dirty = true;
            }
            0x3c => {
                self.regs[reg] = data;
                self.lut_dirty = true;
            }
            0x36 | 0x37 => self.regs[reg] = data & 0x01ff,
            0x39 => self.regs[reg] = data & 1,
            0x3d => self.regs[reg] = data & 0xf,
            0x3e | 0x3f => {}
            0x62 => self.regs[reg] = data & 7,
            0x63 => self.regs[reg] &= !data,
            0x70 => self.regs[reg] = data & 0x3fff,
            0x71 => self.regs[reg] = data & 0x03ff,
            0x72 => {
                let len = (data & 0x3ff) as usize;
                return Some((self.regs[0x70] as u32, self.regs[0x71] as usize, if len == 0 { 0x400 } else { len }));
            }
            _ => self.regs[reg] = data,
        }
        None
    }

    /// Raises interrupt `bits` if enabled.
    pub fn raise(&mut self, bits: u16) {
        self.regs[0x63] |= bits & self.regs[0x62];
    }

    pub fn irq_pending(&self) -> bool {
        self.regs[0x62] & self.regs[0x63] != 0
    }

    fn update_lut(&mut self) {
        let sat = self.regs[0x3c] & 0xff;
        let fade = self.regs[0x30] as i32;
        let sat_adjust = (0xff - sat as i32) as f32 / (0xff - 0x20) as f32;
        for i in 0..0x8000u32 {
            let c5 = |v: u32| ((v << 3) | (v >> 2)) as i32;
            let (mut r, mut g, mut b) = (c5((i >> 10) & 31), c5((i >> 5) & 31), c5(i & 31));
            if sat != 0x20 {
                let (fr, fg, fb) = (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0);
                let luma = fr * 0.299 + fg * 0.587 + fb * 0.114;
                let adj = |c: f32| (((luma + (c - luma) * sat_adjust) * 255.0).floor() as i32).clamp(0, 255);
                (r, g, b) = (adj(fr), adj(fg), adj(fb));
            }
            if fade != 0 {
                r = (r - fade).max(0);
                g = (g - fade).max(0);
                b = (b - fade).max(0);
            }
            self.lut[i as usize] = (r as u32) << 16 | (g as u32) << 8 | b as u32;
        }
        self.lut_dirty = false;
    }

    /// Draws scanline `line` (0-239) into `frame`; `mem` reads main memory
    /// (22-bit word addresses).
    pub fn draw_line(&mut self, line: u32, mem: &dyn Fn(u32) -> u16) {
        if self.lut_dirty {
            self.update_lut();
        }
        let mut buf = [0x8000u16; SCREEN_W];
        let page1 = self.regs[0x20];
        let page2 = self.regs[0x21];
        let sprite_base = 0x40 * self.regs[0x22] as u32;
        for prio in 0..4 {
            self.draw_page(&mut buf, line, prio, page1, 0x10, mem);
            self.draw_page(&mut buf, line, prio, page2, 0x16, mem);
            self.draw_sprites(&mut buf, line, prio, sprite_base, mem);
        }
        let out = &mut self.frame[line as usize * SCREEN_W..][..SCREEN_W];
        for (o, &px) in out.iter_mut().zip(buf.iter()) {
            *o = self.lut[if px & 0x8000 != 0 { 0 } else { px as usize }];
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_strip(
        &self, buf: &mut [u16; SCREEN_W], width_mask: i32, blend: bool, flip_x: bool, tile_h: u32, tile_w: u32,
        gfx: u32, tile: u32, tile_line: u32, drawx: i32, flip_y: bool, pal_offset: u32, bpp: u32, mem: &dyn Fn(u32) -> u16,
    ) {
        let bits_per_row = bpp * tile_w / 16;
        let words_per_tile = bits_per_row * tile_h;
        let yflip = if flip_y { tile_h - 1 } else { 0 };
        let mut m = gfx.wrapping_add(words_per_tile.wrapping_mul(tile)).wrapping_add(bits_per_row * (tile_line ^ yflip));
        let level = BLEND_LEVELS[(self.regs[0x2a] & 3) as usize];
        let (mut bits, mut nbits) = (0u32, 0u32);
        for i in 0..tile_w as i32 {
            let x = if flip_x { tile_w as i32 - 1 - i } else { i };
            let pos = (drawx + x) & width_mask;
            bits <<= bpp;
            if nbits < bpp {
                let w = mem(m & 0x3f_ffff).swap_bytes() as u32;
                m = m.wrapping_add(1);
                bits |= w << (bpp - nbits);
                nbits += 16;
            }
            nbits -= bpp;
            let pal = pal_offset + (bits >> 16);
            bits &= 0xffff;
            if (0..SCREEN_W as i32).contains(&pos) {
                let rgb = self.palette[(pal & 0xff) as usize];
                if rgb & 0x8000 == 0 {
                    let p = &mut buf[pos as usize];
                    if blend && *p & 0x8000 == 0 {
                        let mix = |sh: u32| {
                            let (bot, top) = ((*p as u32 >> sh) & 31, (rgb as u32 >> sh) & 31);
                            (((0x20 - level) * bot + level * top) >> 5) << sh
                        };
                        *p = (mix(10) | mix(5) | mix(0)) as u16;
                    } else {
                        *p = rgb;
                    }
                }
            }
        }
    }

    fn draw_page(&self, buf: &mut [u16; SCREEN_W], line: u32, prio: u16, seg: u16, base: usize, mem: &dyn Fn(u32) -> u16) {
        let xscroll = self.regs[base] as i32;
        let yscroll = self.regs[base + 1] as u32;
        let attr = self.regs[base + 2] as u32;
        let ctrl = self.regs[base + 3] as u32;
        let tilemap = self.regs[base + 4] as u32;
        let attrmap = self.regs[base + 5] as u32;
        if ctrl & 8 == 0 || ((attr >> 12) & 3) as u16 != prio {
            return;
        }
        let gfx = seg as u32 * 0x40;
        if ctrl & 1 != 0 {
            self.draw_linemap(buf, line, gfx, yscroll, attr, ctrl, tilemap, attrmap, mem);
            return;
        }
        let mut logical = line;
        if ctrl & 0x40 != 0 {
            match self.vcomp_line(line) {
                Some(l) => logical = l,
                None => return,
            }
        }
        let tile_wbits = (attr >> 4) & 3;
        let tile_h = 8 << ((attr >> 6) & 3);
        let tile_w = 8 << tile_wbits;
        let tiles_x = 512 / tile_w;
        let y = (logical + yscroll) & 0xff;
        let y0 = y / tile_h;
        let tile_line = y % tile_h;
        let bpp = ((attr & 3) + 1) * 2;
        let mut sx = xscroll;
        if ctrl & 0x10 != 0 {
            sx += self.scroll[((logical + yscroll) & 0xff) as usize] as i16 as i32;
        }
        let upper = sx >> (tile_wbits + 3);
        let end = (SCREEN_W as u32 + tile_w) / tile_w;
        for x0 in 0..end {
            let rx = ((x0 as i32 + upper) as u32) & (tiles_x - 1);
            let at = rx + tiles_x * y0;
            let tile = mem(if ctrl & 4 != 0 { tilemap } else { tilemap + at }) as u32;
            if tile == 0 {
                continue;
            }
            let (mut tattr, mut tctrl) = (attr, ctrl);
            if tctrl & 2 == 0 {
                let mut ex = mem(if ctrl & 4 != 0 { attrmap } else { attrmap + at / 2 }) as u32;
                ex = if rx & 1 != 0 { ex >> 8 } else { ex & 0xff };
                tattr = (tattr & !0x000c) | ((ex >> 2) & 0x000c);
                tattr = (tattr & !0x0f00) | ((ex << 8) & 0x0f00);
                tctrl = (tctrl & !0x0100) | ((ex << 2) & 0x0100);
            }
            let pal_offset = (((tattr & 0x0f00) >> 4) >> bpp) << bpp;
            let drawx = (x0 * tile_w) as i32 - (sx & (tile_w as i32 - 1));
            self.draw_strip(
                buf, 511, tctrl & 0x100 != 0, tattr & 4 != 0, tile_h, tile_w, gfx, tile, tile_line, drawx,
                tattr & 8 != 0, pal_offset, bpp, mem,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_linemap(
        &self, buf: &mut [u16; SCREEN_W], line: u32, _gfx: u32, yscroll: u32, attr: u32, ctrl: u32, tilemap: u32,
        attrmap: u32, mem: &dyn Fn(u32) -> u16,
    ) {
        let real = (line + yscroll) & 0xff;
        let tile = mem(tilemap + real) as u32;
        let mut pal = mem(attrmap + real / 2) as u32;
        pal = if line & 1 != 0 { pal >> 8 } else { pal & 0xff };
        let mut src = tile | (pal << 16);
        if ctrl & 0x80 != 0 {
            for (i, p) in buf.iter_mut().enumerate() {
                let d = mem((src + i as u32) & 0x3f_ffff);
                if d & 0x8000 == 0 {
                    *p = d & 0x7fff;
                }
            }
            return;
        }
        let bpp = ((attr & 3) + 1) * 2;
        let pal_offset = (((attr & 0x0f00) >> 4) >> bpp) << bpp;
        let (mut bits, mut nbits) = (0u32, 0u32);
        for p in buf.iter_mut() {
            bits <<= bpp;
            if nbits < bpp {
                let w = mem(src & 0x3f_ffff).swap_bytes() as u32;
                src = src.wrapping_add(1);
                bits |= w << (bpp - nbits);
                nbits += 16;
            }
            nbits -= bpp;
            let idx = pal_offset + (bits >> 16);
            bits &= 0xffff;
            let rgb = self.palette[(idx & 0xff) as usize];
            if rgb & 0x8000 == 0 {
                *p = rgb;
            }
        }
    }

    /// Vertical compression (registers 0x1c-0x1e): the source line shown on
    /// screen line `line`, or none.
    fn vcomp_line(&self, line: u32) -> Option<u32> {
        let start = self.regs[0x1d] as u32;
        if line < start {
            return None;
        }
        let mut step = (self.regs[0x1e] & 0xff) as i32;
        if step & 0x80 != 0 {
            step -= 0x100;
        }
        let mut inc = (self.regs[0x1c] as i32) << 4;
        let (mut cur, mut counter) = (0i32, 0i32);
        for _ in start..line {
            counter += inc;
            while counter >= 0x200 {
                cur += 1;
                inc += step;
                counter -= 0x200;
            }
        }
        (0..256).contains(&cur).then_some(cur as u32)
    }

    fn draw_sprites(&self, buf: &mut [u16; SCREEN_W], line: u32, prio: u16, gfx: u32, mem: &dyn Fn(u32) -> u16) {
        let ctrl = self.regs[0x42];
        if ctrl & 1 == 0 {
            return;
        }
        for n in 0..SPRITES {
            let s = &self.sprites[4 * n..4 * n + 4];
            let tile = s[0] as u32;
            let attr = s[3] as u32;
            if tile == 0 || ((attr >> 12) & 3) as u16 != prio {
                continue;
            }
            let tile_h = 8 << ((attr >> 6) & 3);
            let tile_w = 8 << ((attr >> 4) & 3);
            let (mut x, mut y) = (s[1] as i16 as i32, s[2] as i16 as i32);
            if ctrl & 2 == 0 {
                // Centre-based coordinates, y up.
                x = (160 + x) - tile_w / 2;
                y = (128 - y) - tile_h / 2;
            }
            x &= 0x1ff;
            y &= 0x1ff;
            let first = y;
            let last = (y + tile_h - 1) & 0x1ff;
            let bpp = ((attr & 3) + 1) * 2;
            let pal_offset = (((attr & 0x0f00) >> 4) >> bpp) << bpp;
            let line = line as i32;
            let mut draw = |tile_line: i32| {
                self.draw_strip(
                    buf, 0x1ff, attr & 0x4000 != 0, attr & 4 != 0, tile_h as u32, tile_w as u32, gfx, tile,
                    tile_line as u32, x, attr & 8 != 0, pal_offset, bpp, mem,
                );
            };
            if first < last {
                if line >= first && line <= last {
                    draw(line - first);
                }
            } else {
                let f = first - 512;
                if line - f >= 0 && line <= last {
                    draw(line - f);
                }
                if line - first >= 0 && line <= last + 512 {
                    draw(line - first);
                }
            }
        }
    }
}

impl Default for Video {
    fn default() -> Self {
        Video::new()
    }
}
