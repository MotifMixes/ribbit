//! EXPERIMENTAL: HD capture from the BIOS's Flash rasterizer.
//!
//! The BIOS rasterizes Flash shapes from edge records in quarter-pixel
//! device coordinates (a 640x640 grid) and averages the result down to the
//! 160x160 LCD (docs/hardware-research.md, "The BIOS's Flash rasterizer").
//! This module records, for each render pass, every edge record the BIOS
//! builds, the fill styles they use and the pass's redraw rectangle, so a
//! renderer can draw the same picture at high resolution. It observes only:
//! the emulated machine runs exactly as without it.
//!
//! Hooks (found by code signature, so BIOS revisions with the same player
//! build work): the edge constructor (r1 = edge record) and a point the
//! scanline driver passes once per scanline (r14 = renderer, whose +0x28,
//! +0x2C, +0x30, +0x34 are the redraw rectangle x0 x1 y0 y1 in quarter
//! pixels). A pass ends at the framebuffer transfer to the LCD.

use std::collections::HashMap;

/// First 44 bytes of the edge constructor (US v2.1 0x4004_E0D8; no absolute
/// addresses inside, so the same compiled code matches in other builds).
const EDGE_CTOR_SIG: [u8; 44] = [
    0xe1, 0xc5, 0xe1, 0xc6, 0xe1, 0xc7, 0xfc, 0x1c, 0x08, 0xb4, 0xfc, 0x1c, 0x48, 0xb4, 0xfc, 0x1c, 0x88, 0xb4, 0xfc,
    0x1c, 0xc8, 0xb4, 0xfc, 0x1c, 0x08, 0xb5, 0xfc, 0x1c, 0x48, 0xb5, 0xf1, 0xc0, 0xa4, 0xc1, 0x08, 0x75, 0x1c, 0x89,
    0x28, 0x76, 0x1a, 0x72, 0x24, 0xe8,
];
/// The scanline driver at US v2.1 0x4005_0514 (20 bytes).
const SCANLINE_SIG: [u8; 20] =
    [0x1c, 0x86, 0x80, 0x20, 0x1f, 0x0f, 0xac, 0x10, 0x00, 0x80, 0x84, 0xe0, 0x1c, 0x01, 0x02, 0x00, 0xd4, 0x16, 0x03, 0x10];

/// The solid-colour span painter at US v2.1 0x4005_544C (24 bytes): fills
/// painted by it are plain colours; others (gradients, bitmaps) are not.
const SOLID_PAINTER_SIG: [u8; 24] = [
    0xe1, 0xc5, 0xe1, 0xc6, 0xfc, 0x1c, 0x08, 0xb4, 0xf1, 0xc0, 0x08, 0x76, 0x02, 0x22, 0x4d, 0x00, 0x8c, 0x25, 0x04, 0x90, 0x1a,
    0x73, 0xd0, 0xf6,
];

/// Where the hooks are in one BIOS image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hooks {
    pub edge_ctor: u32,
    pub scanline: u32,
    pub solid_painter: u32,
}

impl Hooks {
    /// Finds both hooks in a BIOS image mapped at `base`; None unless each
    /// signature occurs (the first match is used).
    pub fn find(bios: &[u8], base: u32) -> Option<Hooks> {
        let find = |sig: &[u8]| {
            bios.windows(sig.len()).enumerate().step_by(2).find(|(_, w)| *w == sig).map(|(i, _)| base + i as u32)
        };
        Some(Hooks {
            edge_ctor: find(&EDGE_CTOR_SIG)?,
            scanline: find(&SCANLINE_SIG)?,
            solid_painter: find(&SOLID_PAINTER_SIG)?,
        })
    }
}

/// An edge in the pass's units (`Pass::scale` per pixel): start, control and end (a straight edge has
/// its control point on the line), and its two fill styles (0 = none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub p: [(i16, i16); 3],
    pub line: bool,
    pub fills: [u32; 2],
}

/// A fill style, keyed by its address in the pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    /// Stacking order (higher draws on top): +0x0C, depth in the high half.
    pub order: u32,
    /// +0x24: colour (stored B, G, R, A as u16).
    pub rgba: [u8; 4],
    /// The fill's span painter (+0x20): tells solid fills from others.
    pub painter: u32,
}

/// Everything one render pass drew.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pass {
    /// Redraw rectangle x0, x1, y0, y1 in the pass's units (end exclusive).
    pub rect: Option<[i32; 4]>,
    /// Units per LCD pixel (renderer +0x00): 4 at the BIOS's highest
    /// quality, 2 at medium (games choose, e.g. LF_SetMovieQuality).
    pub scale: i32,
    pub edges: Vec<Edge>,
    pub fills: HashMap<u32, Fill>,
}

/// Recorder state, owned by the machine while HD capture is on.
#[derive(Debug, Clone)]
pub struct HdCapture {
    pub hooks: Hooks,
    current: Pass,
    /// Finished passes, oldest first, for the renderer to take.
    pub done: Vec<Pass>,
    /// Passes dropped because no renderer took them (the HD picture may then
    /// be stale until the next full redraw).
    pub dropped: u64,
    /// Scanline of the last edge built (a lower one starts a new render).
    last_y: i32,
}

/// More passes than this unconsumed means nobody is rendering; stop growing.
const MAX_PENDING: usize = 64;

impl HdCapture {
    pub fn new(hooks: Hooks) -> Self {
        Self { hooks, current: Pass::default(), done: vec![], dropped: 0, last_y: 0 }
    }

    /// The edge constructor was executed: `rec` is its edge record, `y` the
    /// scanline being drawn. A scanline lower than the last edge's means a
    /// new render (the BIOS may draw several regions before one transfer).
    pub fn edge(&mut self, rec: u32, y: i32, peek: impl Fn(u32) -> u8) {
        if y < self.last_y && !self.current.edges.is_empty() {
            self.end_pass();
        }
        self.last_y = y;
        let b = |o: u32| peek(rec + o);
        let h = |o: u32| i16::from_le_bytes([b(o), b(o + 1)]);
        let w = |a: u32| u32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
        let e = Edge {
            p: [(h(0x10), h(0x12)), (h(0x14), h(0x16)), (h(0x18), h(0x1A))],
            line: b(0x1C) != 0,
            fills: [w(rec + 0x08), w(rec + 0x0C)],
        };
        for f in e.fills {
            if f != 0 {
                self.current.fills.entry(f).or_insert_with(|| {
                    let c = |o: u32| u16::from_le_bytes([peek(f + o), peek(f + o + 1)]).min(255) as u8;
                    Fill { order: w(f + 0x0C), rgba: [c(0x28), c(0x26), c(0x24), c(0x2A)], painter: w(f + 0x20) }
                });
            }
        }
        self.current.edges.push(e);
    }

    /// The scanline driver ran with renderer `r`: keep its redraw rectangle
    /// and scale; a different one from the pass's means a new render.
    pub fn scanline(&mut self, r: u32, peek: impl Fn(u32) -> u8) {
        let w = |a: u32| i32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
        let (rect, scale) = (Some([w(r + 0x28), w(r + 0x2C), w(r + 0x30), w(r + 0x34)]), w(r));
        if self.current.rect.is_some() && (self.current.rect, self.current.scale) != (rect, scale) {
            self.end_pass();
        }
        self.current.rect = rect;
        self.current.scale = scale;
    }

    /// A render is complete (the frame went to the LCD, or another render
    /// began).
    pub fn end_pass(&mut self) {
        if self.current.edges.is_empty() && self.current.rect.is_none() {
            return;
        }
        if self.done.len() >= MAX_PENDING {
            self.done.remove(0);
            self.dropped += 1;
        }
        self.done.push(std::mem::take(&mut self.current));
    }

    /// Takes the finished passes, oldest first.
    pub fn take(&mut self) -> Vec<Pass> {
        std::mem::take(&mut self.done)
    }
}

/// Side of the HD picture: the BIOS's own quarter-pixel grid.
pub const HD_SIZE: usize = 640;
/// Sub-rows sampled per HD row (horizontal coverage is exact).
const SUB: usize = 4;
/// Quarter pixels per LCD pixel.
const Q: usize = HD_SIZE / 160;

/// How a fill is drawn in HD.
#[derive(Clone, Copy)]
enum Paint {
    /// Plain colour, straight alpha.
    Solid([f32; 4]),
    /// Gradient or bitmap: the BIOS's own (LCD-resolution) pixels.
    Lcd,
}

/// One flattened piece of an edge, in HD pixels, with its fills (indices
/// into the pass's fill list, -1 = none).
#[derive(Clone, Copy)]
struct Seg {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    fills: [i32; 2],
}

/// The HD picture, kept across passes like the BIOS keeps its framebuffer
/// (a pass redraws only its rectangle).
#[derive(Clone)]
pub struct HdCanvas {
    /// 0xAARRGGBB, HD_SIZE x HD_SIZE.
    pub pixels: Vec<u32>,
    solid_painter: u32,
    pub passes: u64,
}

impl HdCanvas {
    pub fn new(hooks: Hooks) -> Self {
        Self { pixels: vec![0xFF00_0000; HD_SIZE * HD_SIZE], solid_painter: hooks.solid_painter, passes: 0 }
    }

    /// Draws `pass` inside its redraw rectangle. `lcd` (160x160, 0xAARRGGBB)
    /// is the BIOS's frame: it supplies fills that are not plain colours and
    /// places no fill covers.
    pub fn apply(&mut self, pass: &Pass, lcd: &[u32]) {
        self.passes += 1;
        // Pass units to quarter pixels (a pass with no rectangle seen is
        // taken as a full-quality full redraw).
        let k = match pass.scale {
            1 | 2 | 4 => (Q as i32) / pass.scale,
            _ => 1,
        };
        let [x0, x1, y0, y1] = pass.rect.map_or([0, HD_SIZE as i32, 0, HD_SIZE as i32], |r| r.map(|v| v * k));
        let clamp = |v: i32| v.clamp(0, HD_SIZE as i32) as usize;
        let (x0, x1, y0, y1) = (clamp(x0), clamp(x1), clamp(y0), clamp(y1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        // Fills bottom to top.
        let mut order: Vec<(&u32, &Fill)> = pass.fills.iter().collect();
        order.sort_by_key(|(a, f)| (f.order, **a));
        let index: HashMap<u32, i32> = order.iter().enumerate().map(|(i, (a, _))| (**a, i as i32)).collect();
        let paints: Vec<Paint> = order
            .iter()
            .map(|(_, f)| match f.painter == self.solid_painter {
                true => Paint::Solid(f.rgba.map(|c| c as f32 / 255.0)),
                false => Paint::Lcd,
            })
            .collect();
        // Edges flattened; each segment listed under the rows it crosses.
        let mut segs: Vec<Seg> = vec![];
        for e in &pass.edges {
            let fills = e.fills.map(|f| if f == 0 { -1 } else { index.get(&f).copied().unwrap_or(-1) });
            if fills == [-1, -1] {
                continue;
            }
            let p = e.p.map(|(x, y)| (x as f32 * k as f32, y as f32 * k as f32));
            let n = if e.line {
                1
            } else {
                let d = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() + (a.1 - b.1).abs();
                ((d(p[0], p[1]) + d(p[1], p[2])) / 6.0).ceil().clamp(1.0, 32.0) as usize
            };
            let at = |t: f32| {
                let u = 1.0 - t;
                let (a, b, c) = (u * u, 2.0 * u * t, t * t);
                (a * p[0].0 + b * p[1].0 + c * p[2].0, a * p[0].1 + b * p[1].1 + c * p[2].1)
            };
            let mut prev = p[0];
            for i in 1..=n {
                let q = if i == n { p[2] } else { at(i as f32 / n as f32) };
                if prev.1 != q.1 {
                    segs.push(Seg { x0: prev.0, y0: prev.1, x1: q.0, y1: q.1, fills });
                }
                prev = q;
            }
        }
        let rows = y1 - y0;
        let mut by_row: Vec<Vec<u32>> = vec![vec![]; rows];
        for (i, s) in segs.iter().enumerate() {
            let (lo, hi) = (s.y0.min(s.y1), s.y0.max(s.y1));
            let (a, b) = ((lo.floor() as i64).max(y0 as i64), (hi.ceil() as i64).min(y1 as i64));
            for y in a..b {
                by_row[(y as usize) - y0].push(i as u32);
            }
        }
        let w = x1 - x0;
        let mut acc = vec![[0f32; 3]; w];
        let mut lcd_w = vec![0f32; w];
        let mut xs: Vec<(f32, [i32; 2])> = vec![];
        let mut active = vec![false; paints.len()];
        for (r, list) in by_row.iter().enumerate() {
            let y = y0 + r;
            acc.iter_mut().for_each(|a| *a = [0.0; 3]);
            lcd_w.iter_mut().for_each(|a| *a = 0.0);
            for j in 0..SUB {
                let sy = y as f32 + (j as f32 + 0.5) / SUB as f32;
                xs.clear();
                for &i in list {
                    let s = segs[i as usize];
                    let (lo, hi) = if s.y0 < s.y1 { ((s.x0, s.y0), (s.x1, s.y1)) } else { ((s.x1, s.y1), (s.x0, s.y0)) };
                    if sy >= lo.1 && sy < hi.1 {
                        xs.push((lo.0 + (sy - lo.1) / (hi.1 - lo.1) * (hi.0 - lo.0), s.fills));
                    }
                }
                xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                active.iter_mut().for_each(|a| *a = false);
                let mut from = x0 as f32;
                let mut look = composite(&active, &paints);
                for &(x, fills) in &xs {
                    let to = x.min(x1 as f32);
                    if to > from {
                        span(&mut acc, &mut lcd_w, x0, from, to, look);
                    }
                    for f in fills {
                        if f >= 0 {
                            active[f as usize] ^= true;
                        }
                    }
                    look = composite(&active, &paints);
                    from = from.max(x);
                }
                if (x1 as f32) > from {
                    span(&mut acc, &mut lcd_w, x0, from, x1 as f32, look);
                }
            }
            let row = &mut self.pixels[y * HD_SIZE..(y + 1) * HD_SIZE];
            for i in 0..w {
                let x = x0 + i;
                let l = lcd[(y / Q) * 160 + x / Q];
                let lc = [(l >> 16) & 0xFF, (l >> 8) & 0xFF, l & 0xFF].map(|c| c as f32);
                let c = |k: usize| ((acc[i][k] * 255.0 + lcd_w[i] * lc[k]) / SUB as f32).round().clamp(0.0, 255.0) as u32;
                row[x] = 0xFF00_0000 | c(0) << 16 | c(1) << 8 | c(2);
            }
        }
    }
}

/// Largest per-channel difference between an HD block's average and its
/// LCD pixel that still counts as the same picture (the LCD keeps 4 bits per
/// channel, and the BIOS's anti-aliasing differs slightly from ours).
const AGREE: i32 = 40;

impl HdCanvas {
    /// Makes the HD picture agree with the BIOS's frame `lcd` (160x160): each
    /// 4x4 block whose average differs from its LCD pixel by more than
    /// `AGREE` becomes that pixel. Covers whatever the edges did not
    /// (bitmaps drawn another way, undecoded fills, stale areas), so the HD
    /// picture is never wrong where the LCD is right. Returns the number of
    /// blocks replaced.
    pub fn reconcile(&mut self, lcd: &[u32]) -> usize {
        let mut replaced = 0;
        for by in 0..160 {
            for bx in 0..160 {
                let l = lcd[by * 160 + bx];
                let mut sum = [0i32; 3];
                for y in by * Q..(by + 1) * Q {
                    for &p in &self.pixels[y * HD_SIZE + bx * Q..y * HD_SIZE + (bx + 1) * Q] {
                        sum[0] += ((p >> 16) & 0xFF) as i32;
                        sum[1] += ((p >> 8) & 0xFF) as i32;
                        sum[2] += (p & 0xFF) as i32;
                    }
                }
                let n = (Q * Q) as i32;
                // Accepted range per channel: the 3x3 LCD neighbourhood
                // (an edge block blends its neighbours), widened by AGREE.
                let (mut lo, mut hi) = ([255i32; 3], [0i32; 3]);
                for ny in by.saturating_sub(1)..(by + 2).min(160) {
                    for nx in bx.saturating_sub(1)..(bx + 2).min(160) {
                        let q = lcd[ny * 160 + nx];
                        for (k, c) in [(q >> 16) & 0xFF, (q >> 8) & 0xFF, q & 0xFF].into_iter().enumerate() {
                            lo[k] = lo[k].min(c as i32);
                            hi[k] = hi[k].max(c as i32);
                        }
                    }
                }
                if (0..3).any(|k| sum[k] / n < lo[k] - AGREE || sum[k] / n > hi[k] + AGREE) {
                    replaced += 1;
                    for y in by * Q..(by + 1) * Q {
                        self.pixels[y * HD_SIZE + bx * Q..y * HD_SIZE + (bx + 1) * Q].fill(0xFF00_0000 | l);
                    }
                }
            }
        }
        replaced
    }
}

/// The look where the fills in `active` cover: (premultiplied colour of the
/// plain fills, weight of the BIOS pixel showing beneath or in their place).
fn composite(active: &[bool], paints: &[Paint]) -> ([f32; 3], f32) {
    let (mut c, mut rest) = ([0f32; 3], 1f32);
    for i in (0..paints.len()).rev() {
        if !active[i] {
            continue;
        }
        match paints[i] {
            Paint::Lcd => return (c, rest),
            Paint::Solid(p) => {
                for k in 0..3 {
                    c[k] += rest * p[3] * p[k];
                }
                rest *= 1.0 - p[3];
                if rest <= 0.0 {
                    return (c, 0.0);
                }
            }
        }
    }
    (c, rest)
}

/// Adds one sub-row's span [a, b) (HD pixels) of `look` to the row.
fn span(acc: &mut [[f32; 3]], lcd_w: &mut [f32], x0: usize, a: f32, b: f32, look: ([f32; 3], f32)) {
    let (first, last) = (a.floor() as usize, (b.ceil() as usize).min(x0 + acc.len()));
    for x in first.max(x0)..last {
        let cov = (b.min(x as f32 + 1.0) - a.max(x as f32)).max(0.0);
        let i = x - x0;
        for (a, c) in acc[i].iter_mut().zip(look.0) {
            *a += c * cov;
        }
        lcd_w[i] += look.1 * cov;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_hooks_by_signature_at_any_even_offset() {
        let mut bios = vec![0u8; 0x400];
        bios[0x100..0x100 + 44].copy_from_slice(&EDGE_CTOR_SIG);
        bios[0x202..0x202 + 20].copy_from_slice(&SCANLINE_SIG);
        bios[0x300..0x300 + 24].copy_from_slice(&SOLID_PAINTER_SIG);
        assert_eq!(
            Hooks::find(&bios, 0x4000_0000),
            Some(Hooks { edge_ctor: 0x4000_0100, scanline: 0x4000_0202, solid_painter: 0x4000_0300 })
        );
        assert_eq!(Hooks::find(&bios[..0x200], 0x4000_0000), None);
    }

    #[test]
    fn records_edges_fills_and_the_first_rectangle_per_pass() {
        let mut mem = vec![0u8; 0x200];
        let put = |m: &mut Vec<u8>, a: usize, v: &[u8]| m[a..a + v.len()].copy_from_slice(v);
        // Edge record at 0x00 with fill 0x80; fill colour B=0x10 G=0x20 R=0x30 A=0xFF.
        put(&mut mem, 0x08, &0x80u32.to_le_bytes());
        put(&mut mem, 0x10, &[4, 0, 8, 0, 12, 0, 16, 0, 20, 0, 24, 0, 1]);
        put(&mut mem, 0x80 + 0x0C, &0x000A_0002u32.to_le_bytes());
        put(&mut mem, 0x80 + 0x24, &[0x10, 0, 0x20, 0, 0x30, 0, 0xFF, 0]);
        // Renderer at 0x100 with rectangle 40..106 x 208..256.
        put(&mut mem, 0x100, &2i32.to_le_bytes());
        for (i, v) in [40i32, 106, 208, 256].iter().enumerate() {
            put(&mut mem, 0x128 + 4 * i, &v.to_le_bytes());
        }
        let peek = |a: u32| mem[a as usize];
        let mut c = HdCapture::new(Hooks { edge_ctor: 0, scanline: 0, solid_painter: 0 });
        c.scanline(0x100, peek);
        c.edge(0, 0, peek);
        c.scanline(0x100, peek); // later scanlines of the same render
        c.end_pass();
        c.end_pass(); // nothing drawn: no empty pass
        let passes = c.take();
        assert_eq!(passes.len(), 1);
        let p = &passes[0];
        assert_eq!((p.rect, p.scale), (Some([40, 106, 208, 256]), 2));
        assert_eq!(p.edges[0].p, [(4, 8), (12, 16), (20, 24)]);
        assert!(p.edges[0].line);
        assert_eq!(p.fills[&0x80], Fill { order: 0x000A_0002, rgba: [0x30, 0x20, 0x10, 0xFF], painter: 0 });
    }
}
