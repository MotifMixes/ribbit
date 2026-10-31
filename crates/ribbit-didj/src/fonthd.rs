//! HD text: Didj games draw text with LeapFrog's font module (Brio's
//! libFont.so over FreeType, the system's TrueType fonts in Base_Fonts).
//! Per character, `CFontModule::DrawGlyph` turns the glyph's outline into a
//! bitmap (`FT_Glyph_To_Bitmap`) and a converter writes that bitmap into the
//! game's surface at (x, y) (`ConvertGraymapToRGB4444` and the like). The
//! processor stops at both (`ArmJit::stops`, found in the game's memory by
//! their code): the outline is read before it becomes pixels, and where its
//! bitmap goes, so the HD picture can draw it again from the outline.

use crate::machine::Bus;
use crate::texup::UP;
use ribbit_arm::Arm;

/// A function looked for: its library's anchor (the library is found by
/// it), offset in the library and first instructions.
struct Func {
    lib: Lib,
    offset: u32,
    code: [u32; 8],
}

#[derive(Clone, Copy, PartialEq)]
enum Lib {
    Font,
    FreeType,
}

/// libFont.so's anti-aliased converters and FreeType's FT_Glyph_To_Bitmap
/// (the system firmware's, package 0x000E0004). The converters' order is
/// `Format`'s.
const FUNCS: [Func; 5] = [
    Func { lib: Lib::FreeType, offset: 0xEE8C, code: [0xE92D4FF0, 0xE59FA27C, 0xE24DD0EC, 0xE3500000, 0xE3A04000, 0xE08FA00A, 0xE58D000C, 0xE58D1008] },
    Func { lib: Lib::Font, offset: 0x5A68, code: [0xE92D4FF0, 0xE590C058, 0xE24DD034, 0xE59D7058, 0xE1A05001, 0xE597E000, 0xE20C1C0F, 0xE20C00F0] },
    Func { lib: Lib::Font, offset: 0x5834, code: [0xE92D4FF0, 0xE590C058, 0xE24DD02C, 0xE59D7050, 0xE1A04001, 0xE5970000, 0xE20C1E7E, 0xE1A0E002] },
    Func { lib: Lib::Font, offset: 0x6370, code: [0xE92D4FF0, 0xE590C058, 0xE24DD01C, 0xE59DE040, 0xE1A04003, 0xE59E0000, 0xE59D3040, 0xE1A09001] },
    Func { lib: Lib::Font, offset: 0x5F78, code: [0xE92D4FF0, 0xE590C058, 0xE24DD02C, 0xE59DE050, 0xE1A04001, 0xE59E0000, 0xE20C1CFF, 0xE1A05003] },
];

/// Processor cycles between looks for the libraries while not found.
const SCAN_EVERY: u64 = 200_000_000;

/// The libraries in a process: its translation table and their bases.
#[derive(Clone, Copy)]
struct Found {
    ttb: u32,
    font: u32,
    freetype: u32,
}

/// A glyph's outline (FreeType's FT_Outline, 26.6 pixels, y up).
#[derive(Clone, Debug, Default)]
pub struct Outline {
    pub points: Vec<(i32, i32)>,
    pub tags: Vec<u8>,
    /// The last point of each contour.
    pub contours: Vec<u16>,
}

#[derive(Default)]
pub struct Fonts {
    /// Looking for the libraries and stopping there (HD text on).
    pub on: bool,
    /// didjboot: prints each glyph drawn.
    pub log: bool,
    found: Option<Found>,
    scanned_at: Option<u64>,
    /// The outline `FT_Glyph_To_Bitmap` was last given.
    outline: Option<Outline>,
    pub glyphs: u64,
}

impl Fonts {
    /// The stops for the libraries found (u32::MAX for none).
    pub fn stops(&self) -> [u32; 5] {
        match self.found {
            Some(f) => std::array::from_fn(|i| if FUNCS[i].lib == Lib::Font { f.font } else { f.freetype } + FUNCS[i].offset),
            None => [u32::MAX; 5],
        }
    }

    /// Looks for the libraries in the running process now and then while
    /// they aren't found; true when what was found changed.
    pub fn scan(&mut self, cpu: &Arm, bus: &Bus) -> bool {
        if !self.on || self.found.is_some_and(|f| f.ttb == cpu.cp15.ttb) {
            return false;
        }
        if self.scanned_at.is_some_and(|t| cpu.cycles < t + SCAN_EVERY) {
            return false;
        }
        self.scanned_at = Some(cpu.cycles);
        let ttb = cpu.cp15.ttb & !0x3FFF;
        let mut found = [None, None];
        // User space above 1 GB (where ld.so maps libraries), small pages.
        for i in 0x400..0xC00u32 {
            let l1 = word(bus, ttb + 4 * i).unwrap_or(0);
            if l1 & 3 != 1 {
                continue;
            }
            for j in 0..256u32 {
                let l2 = word(bus, (l1 & !0x3FF) + 4 * j).unwrap_or(0);
                if l2 & 3 == 0 {
                    continue;
                }
                let (va, pa) = (i << 20 | j << 12, l2 & !0xFFF);
                for (k, lib) in [Lib::FreeType, Lib::Font].into_iter().enumerate() {
                    // The library's first function looked for is its anchor.
                    let f = FUNCS.iter().find(|f| f.lib == lib).unwrap();
                    if found[k].is_none() && (0..8).all(|n| word(bus, pa + (f.offset & 0xFFF) + 4 * n) == Some(f.code[n as usize])) {
                        found[k] = Some(va - (f.offset & !0xFFF));
                    }
                }
            }
        }
        let [Some(freetype), Some(font)] = found else { return false };
        // Each function where expected.
        let f = Found { ttb: cpu.cp15.ttb, font, freetype };
        self.found = Some(f);
        if self.log {
            println!("fonts: libFont.so at {font:#010x}, libfreetype at {freetype:#010x} (ttb {:#x})", cpu.cp15.ttb);
        }
        true
    }

    /// The processor is at a stop (`stops`): reads what's drawn. The code
    /// then runs as usual.
    pub fn hook(&mut self, cpu: &Arm, bus: &mut Bus) {
        let Some(found) = self.found.filter(|f| self.on && f.ttb == cpu.cp15.ttb) else { return };
        let pc = cpu.r[15];
        let Some(i) = self.stops().iter().position(|&s| s == pc) else { return };
        let mem = User { bus: &*bus, ttb: found.ttb };
        if i == 0 {
            // FT_Glyph_To_Bitmap(FT_Glyph *the_glyph, mode, origin, destroy):
            // an outline glyph (FT_OutlineGlyphRec) has its outline at 0x14.
            self.outline = mem.word(cpu.r[0]).and_then(|g| mem.outline(g));
            return;
        }
        // Convert...(this, FT_Bitmap *bitmap, x, y, tFontSurf *surf): only
        // 32-bit surfaces (A R G B) so far, the ones games were seen using.
        let outline = self.outline.take();
        if i != 4 {
            return;
        }
        let (this, bitmap, x, y) = (cpu.r[0], cpu.r[1], cpu.r[2] as i32, cpu.r[3] as i32);
        let Some(surf) = mem.word(cpu.r[13]) else { return };
        // tFontSurf: width, height, pitch, buffer, format; FT_Bitmap: rows,
        // width, pitch, buffer.
        let Some([w, h, pitch, buf, _]) = mem.words(surf, 5).and_then(|v| <[u32; 5]>::try_from(v).ok()) else { return };
        let Some([rows, width, bpitch, bbuf]) = mem.words(bitmap, 4).and_then(|v| <[u32; 4]>::try_from(v).ok()) else { return };
        let Some(colour) = mem.word(this + 0x58) else { return };
        // A space draws nothing.
        if rows == 0 || width == 0 || w == 0 || h == 0 || w > 1024 || h > 1024 || pitch < 4 * w || rows > 512 || width > 512 {
            return;
        }
        let current: Option<Vec<u32>> = (0..h).map(|r| mem.words(buf + r * pitch, w as usize)).collect::<Option<Vec<_>>>().map(|v| v.concat());
        let cover: Option<Vec<u8>> = (0..rows).map(|r| mem.bytes(bbuf.wrapping_add(r.wrapping_mul(bpitch)), width as usize)).collect::<Option<Vec<_>>>().map(|v| v.concat());
        let (Some(current), Some(cover)) = (current, cover) else { return };
        // The bitmap is the outline's box, grid-fitted (FT_Glyph_To_Bitmap):
        // its left and top pixel edges.
        let outline = outline.filter(|o| !o.points.is_empty());
        let edges = outline.as_ref().map(|o| {
            let (x0, x1) = o.points.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.0), b.max(p.0)));
            let (y0, y1) = o.points.iter().fold((i32::MAX, i32::MIN), |(a, b), p| (a.min(p.1), b.max(p.1)));
            (x0.div_euclid(64), (y1 + 63).div_euclid(64), (x1 + 63).div_euclid(64) - x0.div_euclid(64), (y1 + 63).div_euclid(64) - y0.div_euclid(64))
        });
        // An outline that doesn't fit the bitmap isn't this glyph's.
        let (outline, left, top) = match edges {
            Some((left, top, bw, bh)) if (bw, bh) == (width as i32, rows as i32) => (outline, left, top),
            _ => (None, 0, 0),
        };
        self.glyphs += 1;
        if self.log {
            println!("glyph {}: ({x},{y}) {width}x{rows} colour {colour:#010x} into {w}x{h} at {buf:#x}, outline {}", self.glyphs, outline.is_some());
        }
        let glyph = Glyph { outline, x, y, left, top, colour, size: (width as i32, rows as i32) };
        bus.ga3d.text.draw((found.ttb, buf), (w as usize, h as usize), current, glyph, &cover);
    }
}

/// A word of physical memory (SDRAM only).
fn word(bus: &Bus, pa: u32) -> Option<u32> {
    let o = pa.wrapping_sub(bus.ram_base) as usize;
    bus.sdram.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()))
}

/// A process's memory, by walking its tables (read only: no TLB, no
/// faults).
struct User<'a> {
    bus: &'a Bus,
    ttb: u32,
}

impl User<'_> {
    fn pa(&self, va: u32) -> Option<u32> {
        let l1 = word(self.bus, (self.ttb & !0x3FFF) | (va >> 20) << 2)?;
        match l1 & 3 {
            2 => Some(l1 & 0xFFF0_0000 | va & 0xF_FFFF),
            1 => {
                let l2 = word(self.bus, (l1 & !0x3FF) | (va >> 12 & 0xFF) << 2)?;
                match l2 & 3 {
                    1 => Some(l2 & 0xFFFF_0000 | va & 0xFFFF),
                    2 | 3 => Some(l2 & !0xFFF | va & 0xFFF),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn word(&self, va: u32) -> Option<u32> {
        if va & 3 != 0 {
            return None;
        }
        word(self.bus, self.pa(va)?)
    }

    /// `len` bytes from `va`, a page at a time.
    fn bytes(&self, va: u32, len: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut at = va;
        while out.len() < len {
            let n = (len - out.len()).min(0x1000 - (at & 0xFFF) as usize);
            let o = self.pa(at)?.wrapping_sub(self.bus.ram_base) as usize;
            out.extend_from_slice(self.bus.sdram.get(o..o + n)?);
            at = at.wrapping_add(n as u32);
        }
        Some(out)
    }

    fn words(&self, va: u32, n: usize) -> Option<Vec<u32>> {
        Some(self.bytes(va, 4 * n)?.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
    }

    /// FT_OutlineGlyphRec at `glyph`: format 'outl', then the FT_Outline
    /// (n_contours, n_points as shorts; points, tags, contours).
    fn outline(&self, glyph: u32) -> Option<Outline> {
        if self.word(glyph + 8)? != u32::from_be_bytes(*b"outl") {
            return None;
        }
        let counts = self.word(glyph + 0x14)?;
        let (nc, np) = ((counts & 0xFFFF) as usize, (counts >> 16) as usize);
        if nc > 256 || np > 4096 {
            return None;
        }
        let (points, tags, contours) = (self.word(glyph + 0x18)?, self.word(glyph + 0x1C)?, self.word(glyph + 0x20)?);
        let p = self.bytes(points, 8 * np)?;
        let points = p.chunks_exact(8).map(|c| (i32::from_le_bytes(c[..4].try_into().unwrap()), i32::from_le_bytes(c[4..].try_into().unwrap()))).collect();
        let tags = self.bytes(tags, np)?;
        let c = self.bytes(contours, 2 * nc)?;
        let contours = c.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(Outline { points, tags, contours })
    }
}

/// A glyph drawn into a text surface: its outline, where its bitmap's
/// top-left went (x, y) and that bitmap's origin in the outline (left,
/// top: whole pixels, y up), its colour (A R G B) and the bitmap's size.
#[derive(Clone)]
pub struct Glyph {
    /// None when it wasn't read: the surface isn't drawn in HD.
    outline: Option<Outline>,
    x: i32,
    y: i32,
    left: i32,
    top: i32,
    colour: u32,
    size: (i32, i32),
}

/// A surface the font module drew into (A R G B, 32 bits): what it held
/// before the first glyph, the glyphs since, and what it holds now as
/// worked out from them (the converter done again).
pub struct Surface {
    /// The process's translation table and the buffer's address in it.
    at: (u32, u32),
    w: usize,
    h: usize,
    background: Vec<u32>,
    glyphs: Vec<Glyph>,
    now: Vec<u32>,
}

/// The text surfaces drawn lately (`Fonts`), for the HD textures.
#[derive(Default)]
pub struct TextStore {
    surfaces: Vec<Surface>,
    /// Textures drawn from outlines (didjboot's summary).
    pub matched: u64,
}

/// Surfaces kept.
const SURFACES: usize = 64;

/// The converter's blend (`ConvertGraymapToRGB32`): each of A R G B as
/// (colour * c + dst * (255 - c)) / 255, rounded down.
fn blend(colour: u32, dst: u32, c: u32) -> u32 {
    if c == 0 {
        return dst;
    }
    (0..4).fold(0, |v, k| {
        let s = 8 * k;
        v | ((colour >> s & 255) * c + (dst >> s & 255) * (255 - c)) / 255 << s
    })
}

impl TextStore {
    /// A glyph's `bitmap` (coverage, `bw` x `bh`) drawn at (x, y) into the
    /// surface `at` (`w` x `h`), which holds `current` (A R G B) before it.
    fn draw(&mut self, at: (u32, u32), (w, h): (usize, usize), current: Vec<u32>, glyph: Glyph, bitmap: &[u8]) {
        let i = match self.surfaces.iter().position(|s| s.at == at && (s.w, s.h) == (w, h) && s.now == current) {
            Some(i) => i,
            None => {
                // Cleared, or drawn into otherwise: text starts again.
                self.surfaces.retain(|s| s.at != at);
                if self.surfaces.len() >= SURFACES {
                    self.surfaces.remove(0);
                }
                self.surfaces.push(Surface { at, w, h, background: current.clone(), glyphs: Vec::new(), now: current });
                self.surfaces.len() - 1
            }
        };
        let s = &mut self.surfaces[i];
        let (bw, bh) = glyph.size;
        for by in 0..bh {
            for bx in 0..bw {
                let (sx, sy) = (glyph.x + bx, glyph.y + by);
                if sx >= 0 && sy >= 0 && (sx as usize) < w && (sy as usize) < h {
                    let p = &mut s.now[sy as usize * w + sx as usize];
                    *p = blend(glyph.colour, *p, bitmap[(by * bw + bx) as usize] as u32);
                }
            }
        }
        s.glyphs.push(glyph);
    }

    /// The texture (`w` x `h`, `texels` A B G R as decoded) drawn at `UP`
    /// times from a text surface's outlines, when its texels are such a
    /// surface's (as uploaded: fewer bits, maybe upside down): A B G R per
    /// pixel, or None. `base` gives the texture's own upscaled pixels
    /// outside the surface.
    pub fn render(&mut self, w: usize, h: usize, texels: &[u32], base: impl Fn(usize, usize) -> u32) -> Option<Vec<u32>> {
        let opaque = texels.iter().all(|&c| c >> 24 == 255);
        let close = |abgr: u32, argb: u32| {
            let argb = if opaque { argb | 0xFF00_0000 } else { argb };
            let (a, r, g, b) = (argb >> 24, argb >> 16 & 255, argb >> 8 & 255, argb & 255);
            let want = a << 24 | b << 16 | g << 8 | r;
            (0..4).all(|k| ((abgr >> (8 * k) & 255) as i32 - (want >> (8 * k) & 255) as i32).abs() <= 18)
        };
        let (s, flip) = self.surfaces.iter().rev().filter(|s| !s.glyphs.is_empty() && s.w <= w && s.h <= h).find_map(|s| {
            [false, true].into_iter().find(|&flip| {
                (0..s.h).all(|y| {
                    let ty = if flip { s.h - 1 - y } else { y };
                    (0..s.w).all(|x| close(texels[ty * w + x], s.now[y * s.w + x]))
                })
            }).map(|flip| (s, flip))
        })?;
        let hd = s.render()?;
        self.matched += 1;
        if let Ok(dir) = std::env::var("DIDJ_TEXT_DUMP") {
            dump(&format!("{dir}/text{:03}.ppm", self.matched), s.w, s.h, &s.now, &hd);
        }
        let (n, sw) = (UP, s.w * UP);
        let mut out = vec![0u32; w * h * n * n];
        for y in 0..h * n {
            for x in 0..w * n {
                out[y * w * n + x] = if x < sw && y < s.h * n {
                    let sy = if flip { s.h * n - 1 - y } else { y };
                    let argb = hd[sy * sw + x];
                    let argb = if opaque { argb | 0xFF00_0000 } else { argb };
                    argb & 0xFF00_FF00 | (argb >> 16 & 255) | (argb & 255) << 16
                } else {
                    base(x, y)
                };
            }
        }
        Some(out)
    }
}

impl Surface {
    /// The surface at `UP` times (A R G B): its background, each pixel
    /// repeated, and the glyphs from their outlines, blended as the
    /// converter does.
    fn render(&self) -> Option<Vec<u32>> {
        let n = UP as i32;
        let (hw, hh) = (self.w * UP, self.h * UP);
        let mut hd: Vec<u32> = (0..hw * hh).map(|i| self.background[(i / hw / UP) * self.w + i % hw / UP]).collect();
        for g in &self.glyphs {
            // The bitmap's box at UP times, a pixel's margin around it.
            let (bw, bh) = ((g.size.0 + 2) * n, (g.size.1 + 2) * n);
            let (ox, oy) = ((g.x - 1) * n, (g.y - 1) * n);
            // Outline (26.6, y up) to the box: x - left, top - y, plus the
            // margin.
            let to = |(px, py): (f32, f32)| ((px / 64.0 - g.left as f32 + 1.0) * n as f32, (g.top as f32 - py / 64.0 + 1.0) * n as f32);
            let cover = rasterize(g.outline.as_ref()?, bw as usize, bh as usize, to);
            for y in 0..bh {
                for x in 0..bw {
                    let (hx, hy) = (ox + x, oy + y);
                    if hx < 0 || hy < 0 || hx as usize >= hw || hy as usize >= hh {
                        continue;
                    }
                    let c = (cover[(y * bw + x) as usize] * 255.0 + 0.5) as u32;
                    let p = &mut hd[hy as usize * hw + hx as usize];
                    *p = blend(g.colour, *p, c.min(255));
                }
            }
        }
        Some(hd)
    }
}

/// didjboot (DIDJ_TEXT_DUMP): a surface as drawn (`now`, each pixel
/// repeated) beside its HD drawing, over grey.
fn dump(path: &str, w: usize, h: usize, now: &[u32], hd: &[u32]) {
    let (hw, hh) = (w * UP, h * UP);
    let over = |argb: u32| {
        let a = (argb >> 24) as f32 / 255.0;
        [16, 8, 0].map(|k| ((argb >> k & 255) as f32 * a + 96.0 * (1.0 - a)) as u8)
    };
    let mut f = format!("P6
{} {hh}
255
", 2 * hw).into_bytes();
    for y in 0..hh {
        for x in 0..2 * hw {
            f.extend(if x < hw { over(now[(y / UP) * w + x / UP]) } else { over(hd[y * hw + x - hw]) });
        }
    }
    let _ = std::fs::write(path, f);
}

/// Coverage (0..1) of `outline` in a `w` x `h` box, its points placed by
/// `to` (TrueType's quadratic curves; cubic ones as their control polygon).
/// Signed-area accumulation, as font-rs does it.
fn rasterize(outline: &Outline, w: usize, h: usize, to: impl Fn((f32, f32)) -> (f32, f32)) -> Vec<f32> {
    let mut acc = vec![0.0f32; w * h + 2];
    let mut line = |(x0, y0): (f32, f32), (x1, y1): (f32, f32)| {
        let clampx = |x: f32| x.clamp(0.0, (w - 1) as f32);
        let (x0, x1) = (clampx(x0), clampx(x1));
        if (y0 - y1).abs() <= 1e-6 {
            return;
        }
        let (dir, (x0, y0), (x1, y1)) = if y0 < y1 { (1.0, (x0, y0), (x1, y1)) } else { (-1.0, (x1, y1), (x0, y0)) };
        let dxdy = (x1 - x0) / (y1 - y0);
        let mut x = x0;
        if y0 < 0.0 {
            x -= y0 * dxdy;
        }
        for y in (y0.max(0.0) as usize)..h.min(y1.ceil() as usize) {
            let row = y * w;
            let dy = ((y + 1) as f32).min(y1) - (y as f32).max(y0);
            let xnext = x + dxdy * dy;
            let d = dy * dir;
            let (a, b) = if x < xnext { (x, xnext) } else { (xnext, x) };
            let (af, bc) = (a.floor(), b.ceil());
            let (ai, bi) = (af as usize, bc as usize);
            if bi <= ai + 1 {
                let xmf = 0.5 * (x + xnext) - af;
                acc[row + ai] += d - d * xmf;
                acc[row + ai + 1] += d * xmf;
            } else {
                let s = (b - a).recip();
                let a0f = a - af;
                let a0 = 0.5 * s * (1.0 - a0f) * (1.0 - a0f);
                let b1f = b - bc + 1.0;
                let am = 0.5 * s * b1f * b1f;
                acc[row + ai] += d * a0;
                if bi == ai + 2 {
                    acc[row + ai + 1] += d * (1.0 - a0 - am);
                } else {
                    let a1 = s * (1.5 - a0f);
                    acc[row + ai + 1] += d * (a1 - a0);
                    for xi in ai + 2..bi - 1 {
                        acc[row + xi] += d * s;
                    }
                    let a2 = a1 + (bi - ai - 3) as f32 * s;
                    acc[row + bi - 1] += d * (1.0 - a2 - am);
                }
                acc[row + bi] += d * am;
            }
            x = xnext;
        }
    };
    let mut start = 0usize;
    for &end in &outline.contours {
        let end = end as usize;
        if end < start || end >= outline.points.len() {
            break;
        }
        let pts: Vec<((f32, f32), bool)> = (start..=end).map(|i| (to((outline.points[i].0 as f32, outline.points[i].1 as f32)), outline.tags[i] & 1 != 0)).collect();
        start = end + 1;
        // On-curve points with the quadratic controls between them (two
        // controls in a row have an on-curve point midway).
        let mid = |a: (f32, f32), b: (f32, f32)| ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0);
        let first = match pts.iter().position(|p| p.1) {
            Some(i) => i,
            None => {
                // All controls: start midway between the first two.
                let m = mid(pts[0].0, pts[1 % pts.len()].0);
                let mut prev = m;
                let k = pts.len();
                for j in 0..k {
                    let (c, next) = (pts[(j + 1) % k].0, mid(pts[(j + 1) % k].0, pts[(j + 2) % k].0));
                    quad(&mut line, prev, c, next);
                    prev = next;
                }
                continue;
            }
        };
        let k = pts.len();
        let mut prev = pts[first].0;
        let mut control: Option<(f32, f32)> = None;
        for j in 1..=k {
            let (p, on) = pts[(first + j) % k];
            match (on, control) {
                (true, None) => {
                    line(prev, p);
                    prev = p;
                }
                (true, Some(c)) => {
                    quad(&mut line, prev, c, p);
                    prev = p;
                    control = None;
                }
                (false, None) => control = Some(p),
                (false, Some(c)) => {
                    let m = mid(c, p);
                    quad(&mut line, prev, c, m);
                    prev = m;
                    control = Some(p);
                }
            }
        }
    }
    let mut sum = 0.0;
    acc[..w * h]
        .iter()
        .map(|&a| {
            sum += a;
            sum.abs().min(1.0)
        })
        .collect()
}

/// A quadratic curve as lines.
fn quad(line: &mut impl FnMut((f32, f32), (f32, f32)), a: (f32, f32), c: (f32, f32), b: (f32, f32)) {
    let len = (c.0 - a.0).hypot(c.1 - a.1) + (b.0 - c.0).hypot(b.1 - c.1);
    let n = (len / 2.0).ceil().clamp(1.0, 32.0) as usize;
    let mut prev = a;
    for i in 1..=n {
        let t = i as f32 / n as f32;
        let u = 1.0 - t;
        let p = (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1);
        line(prev, p);
        prev = p;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn square_is_covered() {
        // A 2 x 2 pixel square (26.6, y up), in a 4 x 4 box from (1, 1).
        let o = Outline { points: vec![(64, 64), (64, 192), (192, 192), (192, 64)], tags: vec![1; 4], contours: vec![3] };
        let c = rasterize(&o, 4, 4, |(x, y)| (x / 64.0, 4.0 - y / 64.0));
        let at = |x: usize, y: usize| c[y * 4 + x];
        assert!((at(1, 1) - 1.0).abs() < 1e-4 && (at(2, 2) - 1.0).abs() < 1e-4, "{c:?}");
        assert!(at(0, 0) < 1e-4 && at(3, 3) < 1e-4, "{c:?}");
    }

    #[test]
    fn converter_blend() {
        assert_eq!(blend(0xFFFF_FFFF, 0, 255), 0xFFFF_FFFF);
        assert_eq!(blend(0xFFFF_FFFF, 0, 128), 0x8080_8080);
        assert_eq!(blend(0xFFFF_FFFF, 0x1234_5678, 0), 0x1234_5678);
    }
}
