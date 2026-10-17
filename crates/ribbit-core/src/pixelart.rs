//! Pixel art traced into smooth shapes, for HD's optional "traced" look of
//! art that has no vector original (bitmaps inside Flash movies, games
//! drawn by their own code). A fixed algorithm, no learned model: in the
//! manner of Kopf and Lischinski's "Depixelizing Pixel Art" (2011), much
//! simplified.
//!
//! 1. Pixels of one colour joined side by side are one region; two pixels
//!    of one colour that touch only at a corner join across it unless the
//!    other diagonal of that 2 x 2 block joins too (then the sparser
//!    colour, the one with fewer pixels around, keeps its diagonal, as
//!    foreground lines do in pixel art).
//! 2. Where a diagonal joins, the corners of the two other pixels are cut
//!    (a quarter pixel each way), so the joined pixels meet along a slope.
//! 3. The borders between regions become chains between junctions; each is
//!    smoothed (through the middles of its straight runs, then corner
//!    cutting), except at real corners: turns of 90 degrees or more between
//!    runs at least two pixels long stay sharp.
//! 4. The shapes are drawn anti-aliased at any scale.
//!
//! The original pixels are always the default; this is a switchable look.

use std::collections::HashMap;

/// An image as A R G B words (premultiplied, as HD keeps them), `width`
/// by `height`.
pub struct Traced {
    width: usize,
    height: usize,
    /// Each pixel's region.
    region: Vec<u32>,
    /// Each region's colour.
    colors: Vec<u32>,
    /// Smoothed border pieces: (from, to), the region on the side the
    /// normal (dy, -dx) points to, and on the other side.
    segs: Vec<((f32, f32), (f32, f32), u32, u32)>,
}

/// A point on the quarter-pixel lattice (x, y in quarter pixels).
type Q = (i32, i32);

impl Traced {
    /// Traces `pixels` (`width` x `height`, A R G B).
    pub fn new(width: usize, height: usize, pixels: &[u32]) -> Traced {
        let (w, h) = (width, height);
        let at = |x: usize, y: usize| pixels[y * w + x];
        // 1. Regions.
        let mut parent: Vec<u32> = (0..(w * h) as u32).collect();
        fn find(p: &mut [u32], mut i: u32) -> u32 {
            while p[i as usize] != i {
                p[i as usize] = p[p[i as usize] as usize];
                i = p[i as usize];
            }
            i
        }
        let union = |p: &mut Vec<u32>, a: usize, b: usize| {
            let (ra, rb) = (find(p, a as u32), find(p, b as u32));
            if ra != rb {
                p[ra.max(rb) as usize] = ra.min(rb);
            }
        };
        for y in 0..h {
            for x in 0..w {
                if x + 1 < w && at(x, y) == at(x + 1, y) {
                    union(&mut parent, y * w + x, y * w + x + 1);
                }
                if y + 1 < h && at(x, y) == at(x, y + 1) {
                    union(&mut parent, y * w + x, (y + 1) * w + x);
                }
            }
        }
        // Diagonals: (corner x, y, true for top-left to bottom-right).
        let mut cuts: Vec<(usize, usize, bool)> = vec![];
        // Pixels of colour `c` in the 8 x 8 window around a corner.
        let around = |cx: usize, cy: usize, c: u32| {
            let mut n = 0;
            for y in cy.saturating_sub(4)..(cy + 4).min(h) {
                for x in cx.saturating_sub(4)..(cx + 4).min(w) {
                    n += (at(x, y) == c) as u32;
                }
            }
            n
        };
        for y in 0..h.saturating_sub(1) {
            for x in 0..w.saturating_sub(1) {
                let (a, b, c, d) = (at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1));
                let d1 = a == d && a != b && a != c;
                let d2 = b == c && b != a && b != d;
                let keep = match (d1, d2) {
                    (true, true) => Some(around(x + 1, y + 1, a) <= around(x + 1, y + 1, b)),
                    (true, false) => Some(true),
                    (false, true) => Some(false),
                    _ => None,
                };
                if let Some(main) = keep {
                    if main {
                        union(&mut parent, y * w + x, (y + 1) * w + x + 1);
                    } else {
                        union(&mut parent, y * w + x + 1, (y + 1) * w + x);
                    }
                    cuts.push((x + 1, y + 1, main));
                }
            }
        }
        let mut ids: HashMap<u32, u32> = HashMap::new();
        let mut colors = vec![];
        let mut region = vec![0u32; w * h];
        for i in 0..w * h {
            let r = find(&mut parent, i as u32);
            let id = *ids.entry(r).or_insert_with(|| {
                colors.push(pixels[r as usize]);
                colors.len() as u32 - 1
            });
            region[i] = id;
        }
        let reg = |x: usize, y: usize| region[y * w + x];
        // 2. Borders on the quarter lattice: (from, to, the region on the
        // side the normal (dy, -dx) points to, the other).
        let mut border: Vec<(Q, Q, u32, u32)> = vec![];
        // Pixel edges that end a quarter short at a cut corner: (edge, the
        // corner end) -> the trimmed end.
        let mut trim: HashMap<(Q, Q, Q), Q> = HashMap::new();
        for &(cx, cy, main) in &cuts {
            let p = (cx as i32 * 4, cy as i32 * 4);
            let joined = if main { reg(cx - 1, cy - 1) } else { reg(cx, cy - 1) };
            // The cut pixels, by the direction of their square from p.
            let cut: [(i32, i32); 2] = if main { [(1, -1), (-1, 1)] } else { [(-1, -1), (1, 1)] };
            for (sx, sy) in cut {
                let (v, hz) = ((p.0, p.1 + 4 * sy), (p.0 + 4 * sx, p.1));
                let (ve, he) = ((p.0, p.1 + sy), (p.0 + sx, p.1));
                trim.insert((norm((v, p)).0, norm((v, p)).1, p), ve);
                trim.insert((norm((hz, p)).0, norm((hz, p)).1, p), he);
                let px = (cx as i32 + if sx > 0 { 0 } else { -1 }, cy as i32 + if sy > 0 { 0 } else { -1 });
                let q = reg(px.0 as usize, px.1 as usize);
                // The slope, with the cut pixel on the normal's side.
                let (dx, dy) = (he.0 - ve.0, he.1 - ve.1);
                if dy * sx - dx * sy > 0 {
                    border.push((ve, he, q, joined));
                } else {
                    border.push((he, ve, q, joined));
                }
            }
        }
        // An edge from a to b between regions: `neg` on the normal's side.
        let edge = |a: Q, b: Q, neg: u32, pos: u32, border: &mut Vec<(Q, Q, u32, u32)>| {
            let k = norm((a, b));
            let s = trim.get(&(k.0, k.1, a)).copied().unwrap_or(a);
            let e = trim.get(&(k.0, k.1, b)).copied().unwrap_or(b);
            if s != e {
                border.push((s, e, neg, pos));
            }
        };
        for y in 0..h {
            for x in 0..w {
                let r = reg(x, y);
                let (qx, qy) = (x as i32 * 4, y as i32 * 4);
                // Downward between left and right: the normal points right.
                if x + 1 < w && reg(x + 1, y) != r {
                    edge((qx + 4, qy), (qx + 4, qy + 4), reg(x + 1, y), r, &mut border);
                }
                // Rightward between top and bottom: the normal points up.
                if y + 1 < h && reg(x, y + 1) != r {
                    edge((qx, qy + 4), (qx + 4, qy + 4), r, reg(x, y + 1), &mut border);
                }
            }
        }
        let segs = smooth(&border, w as i32 * 4, h as i32 * 4);
        Traced { width, height, region, colors, segs }
    }

    /// Draws the traced shapes at `scale` canvas pixels per pixel into
    /// `out` (`width * scale` square rows, A R G B), only rows
    /// `rows.0..rows.1` and columns `cols.0..cols.1` (canvas pixels).
    pub fn draw(&self, scale: usize, out: &mut [u32], stride: usize, rows: (usize, usize), cols: (usize, usize)) {
        self.draw_rows(scale, out, stride, rows, cols, 0);
    }

    /// `draw`, with `out`'s first row being canvas row `first`.
    fn draw_rows(&self, scale: usize, out: &mut [u32], stride: usize, rows: (usize, usize), cols: (usize, usize), first: usize) {
        const SUB: usize = 4;
        let s = scale as f32;
        let (cw, ch) = (self.width * scale, self.height * scale);
        let (y0, y1) = (rows.0.min(ch), rows.1.min(ch));
        let (x0, x1) = (cols.0.min(cw), cols.1.min(cw));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        // Segments by canvas row.
        let mut by_row: Vec<Vec<u32>> = vec![vec![]; y1 - y0];
        for (i, sg) in self.segs.iter().enumerate() {
            let (lo, hi) = ((sg.0).1.min((sg.1).1) * s, (sg.0).1.max((sg.1).1) * s);
            let (a, b) = ((lo.floor() as i64).max(y0 as i64), (hi.ceil() as i64).min(y1 as i64));
            for y in a..b {
                by_row[y as usize - y0].push(i as u32);
            }
        }
        let rgba = |c: u32| [(c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF, c >> 24].map(|v| v as f32);
        let mut acc = vec![[0f32; 4]; x1 - x0];
        let mut xs: Vec<(f32, u32)> = vec![];
        for y in y0..y1 {
            acc.iter_mut().for_each(|a| *a = [0.0; 4]);
            for j in 0..SUB {
                let sy = y as f32 + (j as f32 + 0.5) / SUB as f32;
                xs.clear();
                for &i in &by_row[y - y0] {
                    let (a, b, neg, pos) = self.segs[i as usize];
                    let (a, b) = ((a.0 * s, a.1 * s), (b.0 * s, b.1 * s));
                    let (lo, hi) = if a.1 < b.1 { (a, b) } else { (b, a) };
                    if sy >= lo.1 && sy < hi.1 {
                        let x = lo.0 + (sy - lo.1) / (hi.1 - lo.1) * (hi.0 - lo.0);
                        // The region on the +x side: the normal (dy, -dx)
                        // points to +x when dy > 0.
                        xs.push((x, if b.1 > a.1 { neg } else { pos }));
                    }
                }
                xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                let py = ((sy / s) as usize).min(self.height - 1);
                let mut cur = self.region[py * self.width];
                let mut from = 0f32;
                let put = |a: f32, b: f32, r: u32, acc: &mut [[f32; 4]]| {
                    let c = rgba(self.colors[r as usize]);
                    let (lo, hi) = (a.max(x0 as f32), b.min(x1 as f32));
                    if hi <= lo {
                        return;
                    }
                    for x in lo.floor() as usize..(hi.ceil() as usize).min(x1) {
                        let cov = (hi.min(x as f32 + 1.0) - lo.max(x as f32)).max(0.0);
                        for k in 0..4 {
                            acc[x - x0][k] += c[k] * cov;
                        }
                    }
                };
                for &(x, r) in &xs {
                    put(from, x, cur, &mut acc);
                    cur = r;
                    from = from.max(x);
                }
                put(from, cw as f32, cur, &mut acc);
            }
            for (i, a) in acc.iter().enumerate() {
                let v = a.map(|c| (c / SUB as f32).round().clamp(0.0, 255.0) as u32);
                out[(y - first) * stride + x0 + i] = v[3] << 24 | v[0] << 16 | v[1] << 8 | v[2];
            }
        }
    }

    /// The whole image drawn at `scale` (A R G B, `width * scale` wide),
    /// in bands across the cores.
    pub fn render(&self, scale: usize) -> Vec<u32> {
        let (cw, ch) = (self.width * scale, self.height * scale);
        let mut out = vec![0u32; cw * ch];
        let threads = if cw * ch < 200_000 { 1 } else { std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16) };
        let band = ch.div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (b, chunk) in out.chunks_mut(band * cw).enumerate() {
                let y0 = b * band;
                let rows = chunk.len() / cw;
                scope.spawn(move || {
                    // Drawn into the band, as if its first row were row 0.
                    self.draw_band(scale, chunk, cw, y0, y0 + rows);
                });
            }
        });
        out
    }

    /// `draw` for rows `y0..y1` into `out`, whose first row is row `y0`.
    fn draw_band(&self, scale: usize, out: &mut [u32], stride: usize, y0: usize, y1: usize) {
        self.draw_rows(scale, out, stride, (y0, y1), (0, self.width * scale), y0);
    }
}

fn norm(e: (Q, Q)) -> (Q, Q) {
    if e.0 <= e.1 { e } else { (e.1, e.0) }
}

/// The borders joined into chains and smoothed; corners of real shapes and
/// the image's edge stay where they are.
fn smooth(border: &[(Q, Q, u32, u32)], qw: i32, qh: i32) -> Vec<((f32, f32), (f32, f32), u32, u32)> {
    // Undirected adjacency by point.
    let mut at: HashMap<Q, Vec<usize>> = HashMap::new();
    for (i, s) in border.iter().enumerate() {
        at.entry(s.0).or_default().push(i);
        at.entry(s.1).or_default().push(i);
    }
    let pair = |i: usize| {
        let s = border[i];
        (s.2.min(s.3), s.2.max(s.3))
    };
    // A chain passes through a point only if exactly two pieces of the same
    // pair of regions meet there and it is inside the image.
    let through = |p: Q| -> bool {
        let v = &at[&p];
        v.len() == 2 && pair(v[0]) == pair(v[1]) && p.0 > 0 && p.1 > 0 && p.0 < qw && p.1 < qh
    };
    let mut used = vec![false; border.len()];
    let mut out = vec![];
    let walk = |start: usize, from: Q, used: &mut Vec<bool>, out: &mut Vec<((f32, f32), (f32, f32), u32, u32)>| {
        // Points along the chain, oriented as the first piece goes.
        let first = border[start];
        let (neg, pos) = if first.0 == from { (first.2, first.3) } else { (first.3, first.2) };
        let mut pts = vec![from];
        let mut cur = start;
        let mut p = from;
        loop {
            used[cur] = true;
            let s = border[cur];
            let next = if s.0 == p { s.1 } else { s.0 };
            pts.push(next);
            p = next;
            if !through(p) {
                break;
            }
            let Some(&n) = at[&p].iter().find(|&&j| !used[j]) else { break };
            cur = n;
        }
        let closed = pts.first() == pts.last() && pts.len() > 2;
        let fp: Vec<(f32, f32)> = pts.iter().map(|&(x, y)| (x as f32 / 4.0, y as f32 / 4.0)).collect();
        let curve = smooth_chain(&fp, closed);
        for w in curve.windows(2) {
            out.push((w[0], w[1], neg, pos));
        }
    };
    // Chains from their ends first, then the closed loops left.
    let ends: Vec<Q> = at.keys().copied().filter(|&p| !through(p)).collect();
    for p in ends {
        for k in 0..at[&p].len() {
            let i = at[&p][k];
            if !used[i] {
                walk(i, p, &mut used, &mut out);
            }
        }
    }
    for i in 0..border.len() {
        if !used[i] {
            walk(i, border[i].0, &mut used, &mut out);
        }
    }
    out
}

/// Smooths a chain of lattice points (pixels): straight runs merged, real
/// corners kept, the rest through the middles of the runs, then three
/// rounds of corner cutting. The ends of an open chain stay.
fn smooth_chain(pts: &[(f32, f32)], closed: bool) -> Vec<(f32, f32)> {
    // Merge straight runs.
    let mut v: Vec<(f32, f32)> = vec![];
    for &p in pts {
        if v.len() >= 2 {
            let (a, b) = (v[v.len() - 2], v[v.len() - 1]);
            let cross = (b.0 - a.0) * (p.1 - b.1) - (b.1 - a.1) * (p.0 - b.0);
            let dot = (b.0 - a.0) * (p.0 - b.0) + (b.1 - a.1) * (p.1 - b.1);
            if cross.abs() < 1e-4 && dot > 0.0 {
                *v.last_mut().unwrap() = p;
                continue;
            }
        }
        v.push(p);
    }
    if closed && v.len() > 1 && v.first() == v.last() {
        v.pop();
    }
    let n = v.len();
    if n < 3 {
        let mut r = v.clone();
        if closed && !r.is_empty() {
            r.push(r[0]);
        }
        return r;
    }
    let len = |a: (f32, f32), b: (f32, f32)| ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt();
    // Real corners: a turn of 90 degrees or more between runs at least two
    // pixels long.
    let corner = |i: usize| -> bool {
        if !closed && (i == 0 || i == n - 1) {
            return true;
        }
        let (a, b, c) = (v[(i + n - 1) % n], v[i], v[(i + 1) % n]);
        let (u, w) = ((b.0 - a.0, b.1 - a.1), (c.0 - b.0, c.1 - b.1));
        let cos = (u.0 * w.0 + u.1 * w.1) / (len(a, b) * len(b, c)).max(1e-6);
        cos <= 0.01 && len(a, b) >= 1.99 && len(b, c) >= 1.99
    };
    let fixed: Vec<bool> = (0..n).map(corner).collect();
    // Control points: corners as they are, else the middles of the runs.
    let mut ctrl: Vec<((f32, f32), bool)> = vec![];
    let runs = if closed { n } else { n - 1 };
    for i in 0..runs {
        let (a, b) = (v[i], v[(i + 1) % n]);
        if fixed[i] {
            ctrl.push((a, true));
        }
        ctrl.push((((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), false));
    }
    if !closed {
        ctrl.push((v[n - 1], true));
    }
    // Corner cutting (Chaikin), keeping fixed points.
    for _ in 0..3 {
        let m = ctrl.len();
        if m < 3 {
            break;
        }
        let mut next: Vec<((f32, f32), bool)> = vec![];
        let segs = if closed { m } else { m - 1 };
        for i in 0..segs {
            let (a, b) = (ctrl[i], ctrl[(i + 1) % m]);
            let q = (a.0 .0 * 0.75 + b.0 .0 * 0.25, a.0 .1 * 0.75 + b.0 .1 * 0.25);
            let r = (a.0 .0 * 0.25 + b.0 .0 * 0.75, a.0 .1 * 0.25 + b.0 .1 * 0.75);
            if a.1 {
                next.push(a);
            } else {
                next.push((q, false));
            }
            if !b.1 {
                next.push((r, false));
            }
        }
        if !closed {
            next.push(ctrl[m - 1]);
        }
        ctrl = next;
    }
    let mut out: Vec<(f32, f32)> = ctrl.into_iter().map(|c| c.0).collect();
    if closed {
        out.push(out[0]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat image traces to its own colour everywhere.
    #[test]
    fn flat_image() {
        let t = Traced::new(4, 4, &[0xFF11_2233; 16]);
        assert!(t.render(4).iter().all(|&p| p == 0xFF11_2233));
    }

    /// A square stays a square (its corners are real), and a pixel's
    /// middle keeps its colour at any scale.
    #[test]
    fn square_keeps_corners_and_colours() {
        let (w, h) = (8, 8);
        let mut px = vec![0xFF00_0000u32; w * h];
        for y in 2..6 {
            for x in 2..6 {
                px[y * w + x] = 0xFFFF_FFFF;
            }
        }
        let t = Traced::new(w, h, &px);
        let out = t.render(8);
        let at = |x: usize, y: usize| out[y * 64 + x];
        // Inside the square, just inside its corner, and outside.
        assert_eq!(at(17, 17), 0xFFFF_FFFF);
        assert_eq!(at(40, 40), 0xFFFF_FFFF);
        assert_eq!(at(15, 15), 0xFF00_0000);
        assert_eq!(at(60, 4), 0xFF00_0000);
    }

    /// A staircase (a diagonal line one pixel thick) becomes one shape: no
    /// pixel of it is cut off from the next.
    #[test]
    fn diagonal_line_is_one_region() {
        let (w, h) = (6, 6);
        let mut px = vec![0xFFFF_FFFFu32; w * h];
        for i in 0..6 {
            px[i * w + i] = 0xFF00_0000;
        }
        let t = Traced::new(w, h, &px);
        let r = t.region[0];
        assert!((0..6).all(|i| t.region[i * w + i] == r));
        // The line's middle between two of its pixels is dark, the far
        // corners light.
        let out = t.render(8);
        let at = |x: usize, y: usize| out[y * 48 + x];
        assert!(at(16, 16) & 0xFF < 0x80);
        assert!(at(44, 4) & 0xFF > 0x80);
    }
}
