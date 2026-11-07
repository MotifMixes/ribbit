//! HD drawing: the 3D engine draws each triangle a second time at `scale`
//! times the resolution into a store of its own, beside the console's
//! 16-bit pixels in block memory (colour and depth alike). The store is
//! indexed like block memory (2048 pixels across, a line per 4 KB), each
//! pixel there standing for `scale` x `scale` HD pixels, in 32 x 32 tiles
//! made when first drawn into. Textures are still read from the console's
//! memory, so only edges and interpolation gain detail.
//!
//! Each pixel keeps the value the engine last left in the console's memory
//! there (`shadow`). Where memory holds something else (the CPU, a DMA or a
//! save state wrote it), the HD pixels are stale: they are refilled from
//! memory when the engine next uses them, and the picture shows memory's
//! pixel meanwhile.

use crate::mlc::{BLOCK_BASE, BLOCK_END};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU16, AtomicU32, Ordering};

const TILE: usize = 32;
const ACROSS: usize = 2048 / TILE;
const TILES: usize = ACROSS * ((BLOCK_END - BLOCK_BASE) as usize >> 12) / TILE;
/// A shadow never written: no 16-bit value matches it.
const NONE: u32 = u32::MAX;

struct Tile {
    shadow: Box<[AtomicU32]>,
    hd: Box<[AtomicU16]>,
}

pub struct Hd {
    /// HD pixels per console pixel, across and down (1: off).
    pub scale: usize,
    tiles: Vec<OnceLock<Box<Tile>>>,
}

impl Default for Hd {
    fn default() -> Self {
        Self { scale: 1, tiles: Vec::new() }
    }
}

/// A console pixel's place: its tile and the pixel in it.
fn locate(a: u32) -> Option<(usize, usize, usize)> {
    if !(BLOCK_BASE..BLOCK_END).contains(&a) {
        return None;
    }
    let (x, y) = ((a & 0xFFF) as usize / 2, (a - BLOCK_BASE) as usize >> 12);
    Some(((y / TILE) * ACROSS + x / TILE, x % TILE, y % TILE))
}

impl Hd {
    /// HD drawing at `scale` (1 turns it off); the store starts empty.
    pub fn set_scale(&mut self, scale: usize) {
        let scale = scale.clamp(1, 4);
        if scale != self.scale {
            self.scale = scale;
            self.tiles = if scale > 1 { (0..TILES).map(|_| OnceLock::new()).collect() } else { Vec::new() };
        }
    }

    pub fn on(&self) -> bool {
        self.scale > 1
    }

    fn tile(&self, t: usize) -> &Tile {
        let s = self.scale;
        self.tiles[t].get_or_init(|| {
            Box::new(Tile {
                shadow: (0..TILE * TILE).map(|_| AtomicU32::new(NONE)).collect(),
                hd: (0..TILE * TILE * s * s).map(|_| AtomicU16::new(0)).collect(),
            })
        })
    }

    /// The HD pixels of the tile's pixel (x, y) made current: refilled
    /// from `native` (memory's value) if that isn't what the engine left.
    fn sync(&self, tile: &Tile, x: usize, y: usize, native: u16) {
        let sh = &tile.shadow[y * TILE + x];
        if sh.load(Ordering::Relaxed) != native as u32 {
            self.block(tile, x, y, native);
            sh.store(native as u32, Ordering::Relaxed);
        }
    }

    fn block(&self, tile: &Tile, x: usize, y: usize, v: u16) {
        let s = self.scale;
        for k in 0..s {
            let row = (y * s + k) * TILE * s + x * s;
            for c in &tile.hd[row..row + s] {
                c.store(v, Ordering::Relaxed);
            }
        }
    }

    fn index(&self, x: usize, y: usize, j: usize, k: usize) -> usize {
        (y * self.scale + k) * TILE * self.scale + x * self.scale + j
    }

    /// The engine writes `new` over `old` at console address `a`.
    pub fn note(&self, a: u32, old: u16, new: u16) {
        if let Some((t, x, y)) = locate(a) {
            let tile = self.tile(t);
            self.sync(tile, x, y, old);
            tile.shadow[y * TILE + x].store(new as u32, Ordering::Relaxed);
        }
    }

    /// HD pixel (j, k) of the console pixel at `a`, which memory holds as
    /// `native`.
    pub fn get(&self, a: u32, j: usize, k: usize, native: u16) -> u16 {
        let Some((t, x, y)) = locate(a) else { return native };
        let tile = self.tile(t);
        self.sync(tile, x, y, native);
        tile.hd[self.index(x, y, j, k)].load(Ordering::Relaxed)
    }

    pub fn set(&self, a: u32, j: usize, k: usize, native: u16, v: u16) {
        if let Some((t, x, y)) = locate(a) {
            let tile = self.tile(t);
            self.sync(tile, x, y, native);
            tile.hd[self.index(x, y, j, k)].store(v, Ordering::Relaxed);
        }
    }

    /// A memory fill wrote `v` at `a`: only tiles already made (the others
    /// show memory anyway).
    pub fn fill(&self, a: u32, v: u16) {
        if let Some((t, x, y)) = locate(a)
            && let Some(tile) = self.tiles[t].get()
        {
            self.block(tile, x, y, v);
            tile.shadow[y * TILE + x].store(v as u32, Ordering::Relaxed);
        }
    }

    /// What the screen shows at HD pixel (j, k) of the console pixel at
    /// `a`: the HD pixel while memory still holds the engine's `native`.
    pub fn sample(&self, a: u32, j: usize, k: usize, native: u16) -> u16 {
        let Some((t, x, y)) = locate(a) else { return native };
        match self.tiles.get(t).and_then(|t| t.get()) {
            Some(tile) if tile.shadow[y * TILE + x].load(Ordering::Relaxed) == native as u32 => tile.hd[self.index(x, y, j, k)].load(Ordering::Relaxed),
            _ => native,
        }
    }

    /// The tile console address `a` is in (for `save`).
    pub fn tile_of(a: u32) -> Option<usize> {
        locate(a).map(|(t, _, _)| t)
    }

    /// Copies of tiles `ts` (None: not made yet), for `restore`.
    pub fn save(&self, ts: impl IntoIterator<Item = usize>) -> Vec<(usize, Option<(Vec<u32>, Vec<u16>)>)> {
        let load32 = |c: &[AtomicU32]| c.iter().map(|v| v.load(Ordering::Relaxed)).collect();
        let load16 = |c: &[AtomicU16]| c.iter().map(|v| v.load(Ordering::Relaxed)).collect();
        ts.into_iter().filter(|&t| t < self.tiles.len()).map(|t| (t, self.tiles[t].get().map(|tile| (load32(&tile.shadow), load16(&tile.hd))))).collect()
    }

    /// Puts tiles back as `save` copied them.
    pub fn restore(&mut self, saved: Vec<(usize, Option<(Vec<u32>, Vec<u16>)>)>) {
        for (t, copy) in saved {
            match copy {
                Some((shadow, hd)) => {
                    let tile = self.tile(t);
                    for (c, v) in tile.shadow.iter().zip(shadow) {
                        c.store(v, Ordering::Relaxed);
                    }
                    for (c, v) in tile.hd.iter().zip(hd) {
                        c.store(v, Ordering::Relaxed);
                    }
                }
                None => self.tiles[t] = OnceLock::new(),
            }
        }
    }
}
