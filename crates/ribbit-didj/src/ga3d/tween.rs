//! In-between frames (Enhanced (new), phase 5): each game frame's drawing is
//! recorded as the engine does it (the clipper's triangles in clip space,
//! the TSE's screen-space triangles and rectangles, memory fills, each with
//! the registers it was drawn with), and a picture between two frames is
//! the newer frame drawn again with its positions moved back towards the
//! older one's. Colours and texture coordinates are the newer frame's.
//!
//! The newer frame is drawn into its own buffers (render target and depth),
//! saved first and put back after the picture is made (`Ga3d::untween`),
//! so the game never sees it; with HD drawing on, so are the HD store's
//! tiles under them.

use super::*;

/// The registers an operation is drawn with: the pixel pipeline's, the
/// memory fill's and the viewport.
const REGS: usize = VPORTBOT + 4;

#[derive(Clone, Copy)]
enum Kind {
    /// The clipper's input: position, colour, texture coordinates of each
    /// vertex; inverse culling.
    Clip([[[f32; 4]; 3]; 3], bool),
    Triangle([Vertex; 3], bool),
    Rectangle(Vertex, Vertex),
    Fill,
    /// A LUT fill (the palette is part of what later operations draw with).
    Lut,
}

#[derive(Clone, Copy)]
struct Op {
    kind: Kind,
    /// Index into the frame's register snapshots.
    regs: usize,
}

/// One game frame's drawing, in order.
#[derive(Default)]
pub struct Frame {
    ops: Vec<Op>,
    regs: Vec<[u32; REGS]>,
}

impl Frame {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    fn push(&mut self, kind: Kind, regs: &[u32]) {
        let r: [u32; REGS] = regs[..REGS].try_into().unwrap();
        if self.regs.last() != Some(&r) {
            self.regs.push(r);
        }
        self.ops.push(Op { kind, regs: self.regs.len() - 1 });
    }

    /// What must match for two operations to be the same thing drawn:
    /// the kind, render state and textures (not the render target: frames
    /// take turns with two buffers).
    fn key(&self, k: usize) -> (u8, u32, u32, u32, u32) {
        let op = &self.ops[k];
        let r = &self.regs[op.regs];
        let kind = match op.kind {
            Kind::Clip(..) => 0,
            Kind::Triangle(..) => 1,
            Kind::Rectangle(..) => 2,
            Kind::Fill => 3,
            Kind::Lut => 4,
        };
        (kind, r[RENDERSTATE], r[TEXINFO[0]], r[TEXINFO[0] + 1], r[TEXINFO[1]])
    }

    /// The texture coordinates, hashed: the same for the same letter or
    /// sprite frame (an animated sprite changes them, so they don't have
    /// to match).
    fn uvs(&self, k: usize) -> u64 {
        let uv = |h: u64, c: f32| (h ^ c.to_bits() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23);
        match &self.ops[k].kind {
            Kind::Clip(out, _) => out.iter().flat_map(|v| v[2]).fold(0, uv),
            Kind::Triangle(v, _) => v.iter().flat_map(|v| v[8..].iter().copied()).fold(0, uv),
            Kind::Rectangle(a, b) => [a, b].iter().flat_map(|v| v[8..].iter().copied()).fold(0, uv),
            Kind::Fill | Kind::Lut => 0,
        }
    }
}

/// Recording and the last two frames.
#[derive(Default)]
pub struct Tween {
    rec: Frame,
    pub older: Frame,
    pub newer: Frame,
}

/// What `tween` overwrote: (memory offset, bytes).
pub struct Saved {
    mem: Vec<(usize, Vec<u8>)>,
    hd: Vec<(usize, Option<(Vec<u32>, Vec<u16>)>)>,
}

/// The farthest a vertex moves between frames and is still blended, in
/// console pixels (beyond it, a different thing drawn in the same place
/// in the list, or a jump).
const MAX_MOVE: f32 = 48.0;
/// How far ahead the two lists are searched to line them up again after
/// something appears or goes.
const RESYNC: usize = 64;

/// For each operation of `b`, the operation of `a` it is (by key, in order).
/// Two with the same key but other texture coordinates pair only when no
/// exact match is near: otherwise text that changes pairs each letter with
/// another one of the same font and blends it across.
fn line_up(a: &Frame, b: &Frame) -> Vec<Option<usize>> {
    let mut pairs = vec![None; b.ops.len()];
    let (mut i, mut j) = (0, 0);
    let exact = |i: usize, j: usize| a.key(i) == b.key(j) && a.uvs(i) == b.uvs(j);
    while i < a.ops.len() && j < b.ops.len() {
        if a.key(i) == b.key(j) {
            if a.uvs(i) != b.uvs(j) {
                let ahead_a = (1..RESYNC).find(|&d| i + d < a.ops.len() && exact(i + d, j));
                let ahead_b = (1..RESYNC).find(|&d| j + d < b.ops.len() && exact(i, j + d));
                match (ahead_a, ahead_b) {
                    (Some(da), Some(db)) if da <= db => i += da,
                    (Some(da), None) => i += da,
                    (_, Some(db)) => j += db,
                    (None, None) => {}
                }
                if ahead_a.is_some() || ahead_b.is_some() {
                    continue;
                }
            }
            pairs[j] = Some(i);
            (i, j) = (i + 1, j + 1);
            continue;
        }
        // The nearest match ahead in either list.
        let ahead_a = (1..RESYNC).find(|&d| i + d < a.ops.len() && a.key(i + d) == b.key(j));
        let ahead_b = (1..RESYNC).find(|&d| j + d < b.ops.len() && b.key(j + d) == a.key(i));
        match (ahead_a, ahead_b) {
            (Some(da), Some(db)) if da <= db => i += da,
            (Some(da), None) => i += da,
            (_, Some(db)) => j += db,
            (None, None) => (i, j) = (i + 1, j + 1),
        }
    }
    pairs
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

impl Ga3d {
    /// Starts (or stops) recording frames for in-between pictures.
    pub fn set_tween(&mut self, on: bool) {
        self.tween = on.then(Box::default);
    }

    pub fn tween(&self) -> Option<&Tween> {
        self.tween.as_deref()
    }

    fn record(&mut self, kind: Kind) {
        if let Some(t) = &mut self.tween {
            t.rec.push(kind, &self.regs);
        }
    }

    pub(super) fn record_clip(&mut self, invcull: bool) {
        let out = self.out;
        self.record(Kind::Clip(out, invcull));
    }

    pub(super) fn record_triangle(&mut self, v: &[Vertex; 3], invcull: bool) {
        self.record(Kind::Triangle(*v, invcull));
    }

    pub(super) fn record_rectangle(&mut self, a: &Vertex, b: &Vertex) {
        self.record(Kind::Rectangle(*a, *b));
    }

    pub(super) fn record_fill(&mut self) {
        self.record(Kind::Fill);
    }

    pub(super) fn record_lut(&mut self) {
        self.record(Kind::Lut);
    }

    /// A frame was shown: what was drawn since the last one is the newest
    /// frame. True when there was any.
    pub fn end_frame(&mut self) -> bool {
        let Some(t) = &mut self.tween else { return false };
        if t.rec.is_empty() {
            return false;
        }
        t.older = std::mem::replace(&mut t.newer, std::mem::take(&mut t.rec));
        true
    }

    /// Draws the picture `t` (0 the older frame, 1 the newer) of the way
    /// between the last two frames into the newer frame's buffers. None
    /// when there's nothing to blend; otherwise put memory back with
    /// `untween` once the picture is made.
    pub fn tween_draw(&mut self, t: f32, mem: &mut Mem) -> Option<Saved> {
        let tw = self.tween.take()?;
        let saved = (!tw.older.is_empty() && !tw.newer.is_empty()).then(|| self.redraw(&tw.older, &tw.newer, t, mem));
        self.tween = Some(tw);
        saved
    }

    pub fn untween(&mut self, saved: Saved, mem: &mut Mem) {
        for (o, bytes) in saved.mem {
            mem.sdram[o..o + bytes.len()].copy_from_slice(&bytes);
        }
        self.hd.restore(saved.hd);
    }

    fn redraw(&mut self, a: &Frame, b: &Frame, t: f32, mem: &mut Mem) -> Saved {
        self.flush(mem);
        let (mem_saved, tiles) = save(b, mem);
        let saved = Saved { mem: mem_saved, hd: if self.hd.on() { self.hd.save(tiles) } else { Vec::new() } };
        let pairs = line_up(a, b);
        // Everything the drawing touches that the game would see.
        let keep = (self.regs.clone(), self.lut, self.out, self.cost, self.pixels, self.triangles, self.trace.len(), self.skip);
        self.skip = false;
        for (k, op) in b.ops.iter().enumerate() {
            self.regs[..REGS].copy_from_slice(&b.regs[op.regs]);
            let from = pairs[k].map(|i| a.ops[i].kind);
            match op.kind {
                Kind::Clip(mut out, invcull) => {
                    if let Some(Kind::Clip(old, _)) = from {
                        out = blend_clip(&old, &out, t, &self.regs);
                    }
                    self.out = out;
                    self.clip_and_draw(invcull, mem);
                }
                Kind::Triangle(mut v, invcull) => {
                    if let Some(Kind::Triangle(old, _)) = from
                        && near(&old, &v)
                    {
                        for (n, o) in v.iter_mut().zip(&old) {
                            n[0] = lerp(o[0], n[0], t);
                            n[1] = lerp(o[1], n[1], t);
                        }
                    }
                    self.triangle(&v, invcull, mem);
                }
                Kind::Rectangle(mut p, mut q) => {
                    if let Some(Kind::Rectangle(op, oq)) = from
                        && near(&[op, oq], &[p, q])
                    {
                        for (n, o) in [(&mut p, op), (&mut q, oq)] {
                            n[0] = lerp(o[0], n[0], t);
                            n[1] = lerp(o[1], n[1], t);
                        }
                    }
                    self.rectangle(&p, &q, mem);
                }
                Kind::Fill => self.memory_fill(mem),
                Kind::Lut => self.lut_fill(mem),
            }
        }
        self.flush(mem);
        let (regs, lut, out, cost, pixels, triangles, trace, skip) = keep;
        (self.regs, self.lut, self.out, self.cost, self.pixels, self.triangles, self.skip) = (regs, lut, out, cost, pixels, triangles, skip);
        self.trace.truncate(trace);
        self.epoch += 1;
        saved
    }
}

/// Whether no vertex moved more than `MAX_MOVE` (screen space).
fn near<const N: usize>(a: &[Vertex; N], b: &[Vertex; N]) -> bool {
    a.iter().zip(b).all(|(a, b)| (a[0] - b[0]).abs() <= MAX_MOVE && (a[1] - b[1]).abs() <= MAX_MOVE)
}

/// The clipper's triangle with its positions blended, when each vertex is
/// in front (w > 0) in both and moved no more than `MAX_MOVE` on screen;
/// the newer one otherwise.
fn blend_clip(old: &[[[f32; 4]; 3]; 3], new: &[[[f32; 4]; 3]; 3], t: f32, regs: &[u32]) -> [[[f32; 4]; 3]; 3] {
    let (height, width) = (f24(regs[VPORTBOT + 1]).abs(), f24(regs[VPORTBOT + 3]).abs());
    let screen = |p: &[f32; 4]| [width * p[0] / p[3], height * p[1] / p[3]];
    let ok = old.iter().zip(new).all(|(o, n)| {
        let (o, n) = (&o[0], &n[0]);
        if o[3] <= 0.0 || n[3] <= 0.0 {
            return false;
        }
        let (so, sn) = (screen(o), screen(n));
        (so[0] - sn[0]).abs() <= MAX_MOVE && (so[1] - sn[1]).abs() <= MAX_MOVE
    });
    if !ok {
        return *new;
    }
    let mut v = *new;
    for (n, o) in v.iter_mut().zip(old) {
        n[0] = std::array::from_fn(|c| lerp(o[0][c], n[0][c], t));
    }
    v
}

/// Saves the memory frame `b` draws into: each render target and depth
/// buffer inside its clipping rectangle, and each fill's rectangle; and
/// lists the HD store's tiles under them.
fn save(b: &Frame, mem: &Mem) -> (Vec<(usize, Vec<u8>)>, std::collections::BTreeSet<usize>) {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut tiles = std::collections::BTreeSet::new();
    let mut rect = |sub: u32, texture: bool, addr: &dyn Fn(u32, u32) -> u32, [x0, y0, x1, y1]: [u32; 4]| {
        if !seen.insert((sub, texture, x0, y0, x1, y1)) {
            return;
        }
        for y in y0..=y1 {
            // A line's pixels are together in memory (block or linear), in
            // runs that break only at the block's 64-byte cells.
            let mut run: Option<(usize, usize)> = None;
            for x in x0..=x1 {
                let a = addr(x, y);
                tiles.extend(Hd::tile_of(a));
                let o = Mem::offset(a, texture);
                match &mut run {
                    Some((start, end)) if *end == o => *end = o + 2,
                    _ => {
                        if let Some((s, e)) = run.take()
                            && e <= mem.sdram.len()
                        {
                            out.push((s, mem.sdram[s..e].to_vec()));
                        }
                        run = Some((o, o + 2));
                    }
                }
            }
            if let Some((s, e)) = run
                && e <= mem.sdram.len()
            {
                out.push((s, mem.sdram[s..e].to_vec()));
            }
        }
    };
    for op in &b.ops {
        let r = &b.regs[op.regs];
        if let Kind::Lut = op.kind {
            continue;
        }
        if let Kind::Fill = op.kind {
            let (p0, p1, p2) = (r[MFILLPARAM0], r[MFILLPARAM0 + 1], r[MFILLPARAM0 + 2]);
            let sub = p0 >> 16 & 0x7FF;
            rect(sub, p0 >> 27 & 1 != 0, &|x, y| sub_addr(sub, 2 * x, y), [p1 & 0x3FF, p1 >> 16 & 0x3FF, p2 & 0x3FF, p2 >> 16 & 0x3FF]);
            continue;
        }
        let (rt, zb, rt1, rt2) = (r[RENDTRG0], r[ZBINFO], r[RENDTRG0 + 1], r[RENDTRG0 + 2]);
        let clip = [rt1 & 0x7FF, rt1 >> 16 & 0x3FF, rt2 & 0x7FF, rt2 >> 16 & 0x3FF];
        if clip[0] > clip[2] || clip[1] > clip[3] {
            continue;
        }
        for b in [rt, zb] {
            rect(b, b >> 27 & 1 != 0, &|x, y| pixel_addr(b >> 16 & 0x7FF, b & 0xFF, b >> 8 & 0xFF, x, y), clip);
        }
    }
    (out, tiles)
}
