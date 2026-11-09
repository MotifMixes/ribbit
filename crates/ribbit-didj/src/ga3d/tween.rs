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

use std::collections::HashMap;
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
/// The farthest a new animation frame moves and is still blended, and how
/// much its corners may differ in that (console pixels).
const ANIMATED_MOVE: f32 = 12.0;
const RIGID: f32 = 1.5;
/// How far ahead the two lists are searched to line them up again after
/// something appears or goes.
const RESYNC: usize = 64;
/// Closer than this (console pixels) is the same place.
const STILL: f32 = 0.5;
/// Closer than this counts as about the same place, for things that may
/// have changed picture (a selected tile sits a little apart).
const SAME_PLACE: f32 = 6.0;

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
    nearest_alike(a, b, &mut pairs);
    follow_layers(a, b, &mut pairs);
    pairs
}

/// How far an operation moved from `from` to `to`, in console pixels.
fn moved(from: &Kind, to: &Kind) -> Option<(f32, f32)> {
    let (from, to) = (place(from)?, place(to)?);
    Some((to.0 - from.0, to.1 - from.1))
}

/// A scrolling layer moves as one, whatever order it is drawn in. Every
/// picture votes for each way it could have moved (to each place that same
/// picture was), and each then pairs with the place the most votes agree on:
/// lists that shift lose nothing, and a tile of a repeating wall doesn't
/// take its identical neighbour, a tile's width away, which opened gaps.
fn follow_layers(a: &Frame, b: &Frame, pairs: &mut [Option<usize>]) {
    let eighths = |m: (f32, f32)| ((m.0 * 8.0).round() as i32, (m.1 * 8.0).round() as i32);
    let mut same_look: HashMap<_, Vec<usize>> = HashMap::new();
    for i in 0..a.ops.len() {
        if place(&a.ops[i].kind).is_some() {
            same_look.entry((a.key(i), a.uvs(i))).or_default().push(i);
        }
    }
    let candidates = |j: usize| -> Vec<(usize, (f32, f32))> {
        same_look.get(&(b.key(j), b.uvs(j))).map_or(Vec::new(), |list| {
            let moves = list.iter().filter_map(|&i| Some((i, moved(&a.ops[i].kind, &b.ops[j].kind)?)));
            moves.filter(|(_, m)| m.0.abs() <= MAX_MOVE && m.1.abs() <= MAX_MOVE).collect()
        })
    };
    let mut votes: HashMap<(i32, i32), u32> = HashMap::new();
    for j in 0..b.ops.len() {
        for (_, m) in candidates(j) {
            *votes.entry(eighths(m)).or_default() += 1;
        }
    }
    for (j, pair) in pairs.iter_mut().enumerate() {
        // Most votes; a move only it has (3D, each triangle its own): the
        // one in the list's order; then the shortest.
        let score = |&(i, m): &(usize, (f32, f32))| {
            let count = votes[&eighths(m)];
            (count, count == 1 && *pair == Some(i), -(m.0.abs() + m.1.abs()))
        };
        // A move only one picture could make is no layer's: one that changed
        // picture in its place keeps its pair, not one that looks like it now.
        if let Some(&(i, m)) = candidates(j).iter().max_by(|x, y| score(x).partial_cmp(&score(y)).unwrap())
            && (votes[&eighths(m)] > 1 || pair.is_none())
        {
            *pair = Some(i);
        }
    }
}

/// How far to take back an operation that has no earlier self to blend from
/// (a tile that changed picture, one half of a quad) so that it stays with
/// what is drawn around it: as its neighbour in the list moved. Only for one
/// or two in a row: more is a sprite, which has its own motion.
fn with_neighbours(a: &Frame, b: &Frame, pairs: &[Option<usize>]) -> Vec<Option<(f32, f32)>> {
    let same = |j: usize| pairs[j].is_some_and(|i| a.key(i) == b.key(j) && a.uvs(i) == b.uvs(j));
    let drawn = |j: usize| place(&b.ops[j].kind).is_some();
    let mut shifts = vec![None; b.ops.len()];
    let mut j = 0;
    while j < b.ops.len() {
        if !drawn(j) || same(j) {
            j += 1;
            continue;
        }
        let start = j;
        while j < b.ops.len() && drawn(j) && !same(j) {
            j += 1;
        }
        if j - start > 2 {
            continue;
        }
        let beside = |n: usize| drawn(n) && b.key(n) == b.key(start);
        let neighbour = (start > 0 && beside(start - 1)).then(|| start - 1).or((j < b.ops.len() && beside(j)).then_some(j));
        let Some(n) = neighbour else { continue };
        let Some(m) = pairs[n].and_then(|i| moved(&a.ops[i].kind, &b.ops[n].kind)) else { continue };
        shifts[start..j].fill(Some(m));
    }
    shifts
}

/// Where an operation is on screen, in console pixels (about, for the
/// clipper's: its centre with x and y from -1 to 1); None when it has no
/// place or is behind the eye.
fn place(kind: &Kind) -> Option<(f32, f32)> {
    match kind {
        Kind::Clip(out, _) => {
            let w = out.iter().map(|v| v[0][3]).fold(f32::MAX, f32::min);
            if w <= 0.0 {
                return None;
            }
            let at = |c: usize| out.iter().map(|v| v[0][c] / v[0][3]).sum::<f32>() / 3.0;
            Some((at(0) * 160.0, at(1) * 120.0))
        }
        Kind::Triangle(v, _) => Some((v.iter().map(|v| v[0]).sum::<f32>() / 3.0, v.iter().map(|v| v[1]).sum::<f32>() / 3.0)),
        Kind::Rectangle(p, q) => Some(((p[0] + q[0]) / 2.0, (p[1] + q[1]) / 2.0)),
        Kind::Fill | Kind::Lut => None,
    }
}

/// Things that look the same (a row of letter tiles) can be drawn in another
/// order from one frame to the next (Didji Racing draws the row around its
/// blinking cursor): by order alone each pairs with its neighbour and the
/// whole row slides half a tile in the picture between. Each pairs instead
/// with the one of its kind still in its place, so what didn't move doesn't.
fn nearest_alike(a: &Frame, b: &Frame, pairs: &mut [Option<usize>]) {
    let far = |i: usize, at: (f32, f32)| place(&a.ops[i].kind).map_or(f32::MAX, |p| (p.0 - at.0).abs() + (p.1 - at.1).abs());
    let uvs_a: Vec<u64> = (0..a.ops.len()).map(|i| a.uvs(i)).collect();
    for (j, pair) in pairs.iter_mut().enumerate() {
        let (Some(i), Some(at)) = (*pair, place(&b.ops[j].kind)) else { continue };
        let uvs = b.uvs(j);
        // Only one about in the same place: scrolling rows of the same tile
        // must keep their order, or a fast scroll goes backwards in the
        // picture between. Nearest first and, as near, one that shows the
        // same picture (a tile, not the blinking cursor over it); a tile
        // that changes picture where it is (the selected one) is still
        // that tile, not the next one along that looks like it.
        let score = |c: usize| far(c, at) + if uvs_a[c] == uvs { 0.0 } else { 0.25 };
        if far(i, at) < STILL && uvs_a[i] == uvs {
            continue;
        }
        let key = b.key(j);
        let mut best = (score(i), i);
        for c in i.saturating_sub(RESYNC)..(i + RESYNC).min(a.ops.len()) {
            if far(c, at) < SAME_PLACE && a.key(c) == key {
                let d = score(c);
                if d < best.0 {
                    best = (d, c);
                }
            }
        }
        *pair = Some(best.1);
    }
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
        let shifts = with_neighbours(a, b, &pairs);
        // TEMPTW: TWEEN_DUMP=1 prints, per in-between picture, one line per
        // drawn op (kind, where, how big, which op it paired with, its move
        // and back-shift). The timer-end glitch is the last picture before
        // the Tune-Up screen changes; grab the tail of stderr.
        if std::env::var_os("TWEEN_DUMP").is_some() {
            use std::sync::atomic::{AtomicU64, Ordering};
            static PIC: AtomicU64 = AtomicU64::new(0);
            let n = PIC.fetch_add(1, Ordering::Relaxed);
            let span = |kind: &Kind| -> (f32, f32) {
                match kind {
                    Kind::Rectangle(p, q) => ((q[0] - p[0]).abs(), (q[1] - p[1]).abs()),
                    Kind::Triangle(v, _) => {
                        let ext = |c: usize| v.iter().map(|x| x[c]).fold(f32::MIN, f32::max) - v.iter().map(|x| x[c]).fold(f32::MAX, f32::min);
                        (ext(0), ext(1))
                    }
                    _ => (f32::NAN, f32::NAN),
                }
            };
            eprintln!("[pic {n}] t={t:.3} ops={}", b.ops.len());
            for (k, op) in b.ops.iter().enumerate() {
                let Some(at) = place(&op.kind) else { continue };
                let tag = match op.kind {
                    Kind::Clip(..) => "clip",
                    Kind::Triangle(..) => "tri",
                    Kind::Rectangle(..) => "rect",
                    Kind::Fill => "fill",
                    Kind::Lut => "lut",
                };
                let (w, h) = span(&op.kind);
                let (pair, mv, look) =
                    pairs[k].map_or((-1i64, (f32::NAN, f32::NAN), false), |i| (i as i64, moved(&a.ops[i].kind, &op.kind).unwrap_or((f32::NAN, f32::NAN)), a.uvs(i) == b.uvs(k)));
                let sh = shifts[k].map_or(f32::NAN, |s| s.0);
                eprintln!(
                    "  [op {k}] {tag} at {:.1},{:.1} size {:.1}x{:.1} pair {pair} mv {:.2},{:.2} look {} shift {:.2} uv {:x}",
                    at.0, at.1, w, h, mv.0, mv.1, look as u8, sh, b.uvs(k)
                );
            }
        }
        // Everything the drawing touches that the game would see.
        let keep = (self.regs.clone(), self.lut, self.out, self.cost, self.pixels, self.triangles, self.trace.len(), self.skip);
        self.skip = false;
        for (k, op) in b.ops.iter().enumerate() {
            self.regs[..REGS].copy_from_slice(&b.regs[op.regs]);
            let from = if shifts[k].is_some() { None } else { pairs[k].map(|i| a.ops[i].kind) };
            let back = shifts[k].map(|m| (m.0 * (1.0 - t), m.1 * (1.0 - t)));
            let same_look = pairs[k].is_some_and(|i| a.uvs(i) == b.uvs(k));
            match op.kind {
                Kind::Clip(mut out, invcull) => {
                    if let Some(Kind::Clip(old, _)) = from {
                        out = blend_clip(&old, &out, t, &self.regs, same_look);
                    }
                    if let Some((x, y)) = back {
                        for v in &mut out {
                            v[0][0] -= x * v[0][3] / 160.0;
                            v[0][1] -= y * v[0][3] / 120.0;
                        }
                    }
                    self.out = out;
                    self.clip_and_draw(invcull, mem);
                }
                Kind::Triangle(mut v, invcull) => {
                    if let Some(Kind::Triangle(old, _)) = from
                        && blends(&old, &v, same_look)
                    {
                        for (n, o) in v.iter_mut().zip(&old) {
                            n[0] = lerp(o[0], n[0], t);
                            n[1] = lerp(o[1], n[1], t);
                        }
                    }
                    if let Some((x, y)) = back {
                        for n in &mut v {
                            n[0] -= x;
                            n[1] -= y;
                        }
                    }
                    self.triangle(&v, invcull, mem);
                }
                Kind::Rectangle(mut p, mut q) => {
                    if let Some(Kind::Rectangle(op, oq)) = from
                        && blends(&[op, oq], &[p, q], same_look)
                    {
                        for (n, o) in [(&mut p, op), (&mut q, oq)] {
                            n[0] = lerp(o[0], n[0], t);
                            n[1] = lerp(o[1], n[1], t);
                        }
                    }
                    if let Some((x, y)) = back {
                        for n in [&mut p, &mut q] {
                            n[0] -= x;
                            n[1] -= y;
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

/// Whether a screen-space operation blends from `a` to `b`. One showing
/// another part of a texture (`same_look` false) is a new animation frame:
/// a sprite in one piece moves as a whole and not far; the pieces of one
/// cut up (Didji Racing's swinging hose) would each fly to another piece's
/// place, shards in the picture between, so they are drawn where they end.
fn blends<const N: usize>(a: &[Vertex; N], b: &[Vertex; N], same_look: bool) -> bool {
    if same_look {
        return near(a, b);
    }
    let moved = |i: usize| (b[i][0] - a[i][0], b[i][1] - a[i][1]);
    let (dx, dy) = moved(0);
    dx.abs() <= ANIMATED_MOVE
        && dy.abs() <= ANIMATED_MOVE
        && (1..N).all(|i| {
            let (x, y) = moved(i);
            (x - dx).abs() <= RIGID && (y - dy).abs() <= RIGID
        })
}

/// The clipper's triangle with its positions blended, when each vertex is
/// in front (w > 0) in both and moved no more than `MAX_MOVE` on screen
/// (a new animation frame: as `blends`); the newer one otherwise.
fn blend_clip(old: &[[[f32; 4]; 3]; 3], new: &[[[f32; 4]; 3]; 3], t: f32, regs: &[u32], same_look: bool) -> [[[f32; 4]; 3]; 3] {
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
    if !same_look {
        let moved = |i: usize| {
            let (so, sn) = (screen(&old[i][0]), screen(&new[i][0]));
            (sn[0] - so[0], sn[1] - so[1])
        };
        let (dx, dy) = moved(0);
        let whole = dx.abs() <= ANIMATED_MOVE
            && dy.abs() <= ANIMATED_MOVE
            && (1..3).all(|i| {
                let (x, y) = moved(i);
                (x - dx).abs() <= RIGID && (y - dy).abs() <= RIGID
            });
        if !whole {
            return *new;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A sprite at `x`, with texture coordinate `u` (what it shows).
    fn sprite(x: f32, u: f32) -> Kind {
        let mut p: Vertex = [0.0; 12];
        p[0] = x;
        p[8] = u;
        let mut q = p;
        q[0] = x + 20.0;
        Kind::Rectangle(p, q)
    }

    fn frame(ops: &[Kind]) -> Frame {
        let mut f = Frame::default();
        for &k in ops {
            f.push(k, &[0; REGS]);
        }
        f
    }

    #[test]
    fn a_row_drawn_in_another_order_stays_put() {
        let a = frame(&[sprite(0.0, 1.0), sprite(40.0, 1.0), sprite(80.0, 1.0)]);
        let b = frame(&[sprite(40.0, 1.0), sprite(80.0, 1.0), sprite(0.0, 1.0)]);
        assert_eq!(line_up(&a, &b), vec![Some(1), Some(2), Some(0)]);
    }

    fn at(x: f32, y: f32) -> Vertex {
        let mut v: Vertex = [0.0; 12];
        v[0] = x;
        v[1] = y;
        v
    }

    #[test]
    fn a_new_animation_frame_blends_only_as_a_whole() {
        let old = [at(0.0, 0.0), at(16.0, 16.0)];
        // A walking sprite: the same size, a little further on.
        assert!(blends(&old, &[at(4.0, 0.0), at(20.0, 16.0)], false));
        // A piece taking another piece's place.
        assert!(!blends(&old, &[at(30.0, 5.0), at(46.0, 21.0)], false));
        // Another shape.
        assert!(!blends(&old, &[at(0.0, 0.0), at(24.0, 8.0)], false));
        // The same picture may go further.
        assert!(blends(&old, &[at(30.0, 5.0), at(46.0, 21.0)], true));
    }

    #[test]
    fn a_blinking_cursor_does_not_take_the_tile_under_it() {
        // Older: tile, cursor over it, tile. Newer: the two tiles.
        let a = frame(&[sprite(0.0, 2.0), sprite(40.0, 9.0), sprite(40.0, 1.0)]);
        let b = frame(&[sprite(0.0, 2.0), sprite(40.0, 1.0)]);
        assert_eq!(line_up(&a, &b), vec![Some(0), Some(2)]);
    }

    #[test]
    fn a_tile_that_changes_picture_stays_where_it_is() {
        // The selected tile (picture 2) becomes a normal one (picture 1); the
        // next normal one is 40 pixels along.
        let a = frame(&[sprite(0.0, 2.0), sprite(40.0, 1.0)]);
        let b = frame(&[sprite(0.0, 1.0), sprite(40.0, 1.0)]);
        // (The other, left without a partner, is simply drawn where it is.)
        assert_eq!(line_up(&a, &b)[0], Some(0));
    }

    #[test]
    fn a_repeating_wall_scrolls_as_one() {
        // Tiles of one picture 10 apart and a marked one, all moving -2, the
        // list starting one tile later: by nearness or order some would take
        // the tile beside them.
        let row = |at: f32, n: usize, marked: f32| -> Vec<Kind> { (0..n).map(|k| sprite(at + 10.0 * k as f32, 0.0)).chain([sprite(marked, 0.5)]).collect() };
        let (a, b) = (frame(&row(0.0, 8, 200.0)), frame(&row(8.0, 7, 198.0)));
        let pairs = line_up(&a, &b);
        for (j, pair) in pairs.iter().enumerate() {
            assert_eq!(moved(&a.ops[pair.unwrap()].kind, &b.ops[j].kind), Some((-2.0, 0.0)), "op {j}");
        }
    }

    #[test]
    fn a_tile_with_a_new_picture_moves_with_its_neighbours() {
        let a = frame(&[sprite(0.0, 0.0), sprite(10.0, 0.25), sprite(20.0, 0.5)]);
        let b = frame(&[sprite(-2.0, 0.0), sprite(8.0, 0.75), sprite(18.0, 0.5)]);
        let pairs = line_up(&a, &b);
        assert_eq!(with_neighbours(&a, &b, &pairs), vec![None, Some((-2.0, 0.0)), None]);
    }

    #[test]
    fn a_moving_sprite_still_pairs_with_itself() {
        let a = frame(&[sprite(10.0, 1.0), sprite(100.0, 2.0)]);
        let b = frame(&[sprite(14.0, 1.0), sprite(104.0, 2.0)]);
        assert_eq!(line_up(&a, &b), vec![Some(0), Some(1)]);
    }
}
