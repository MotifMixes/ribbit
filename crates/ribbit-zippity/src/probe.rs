//! Research hooks (used by `crates/ribbit-zippity/examples/zipboot.rs`): logs of
//! I/O and memory accesses and scripted inputs. All off by default, and
//! cheap when off.

use std::collections::BTreeMap;

/// What one I/O register saw.
#[derive(Default, Clone)]
pub struct Access {
    pub reads: u64,
    pub writes: u64,
    pub last_write: u32,
    pub first_pc: u32,
}

#[derive(Default)]
pub struct Probe {
    /// Count the accesses to every I/O register (`stats`) and keep the first
    /// few of each (`first`).
    pub enabled: bool,
    pub stats: BTreeMap<u32, Access>,
    pub first: Vec<String>,
    /// I/O accesses in this range (inclusive), from `watch_from` on.
    pub watch: Option<(u32, u32)>,
    pub watch_from: u64,
    pub watched: Vec<String>,
    /// CPU writes to memory in this range.
    pub rwatch: Option<(u32, u32)>,
    pub rwatched: Vec<String>,
    /// GPIO input bits flipped from their idle level: (port address, mask,
    /// from, to), times in instructions.
    pub presses: Vec<(u32, u32, u64, u64)>,
    /// The controllers' state over time: (state, from, to); outside these
    /// windows the state set with `Zippity::set_pad` is used.
    pub pad_script: Vec<(u16, u64, u64)>,
    /// Print each sound channel start.
    pub spu_log: bool,
    /// Operations started on the 0xD0800000 engine.
    pub gfx_ops: u64,
    /// Hold the power button this many instructions from power-on instead
    /// of the usual (long enough: the kernel enters its test mode).
    pub power_hold: Option<u64>,
    /// Run idle loops instruction by instruction (to check that skipping
    /// them changes nothing).
    pub no_idle_skip: bool,
    /// Research: leave the text layers or the sprites out of the picture.
    pub hide_text: bool,
    pub hide_sprites: bool,
}

impl Probe {
    pub(crate) fn io(&mut self, now: u64, pc: u32, a: u32, write: Option<u32>, size: u32) {
        if self.enabled {
            let e = self.stats.entry(a).or_insert_with(|| Access { first_pc: pc, ..Default::default() });
            match write {
                Some(v) => {
                    e.writes += 1;
                    e.last_write = v;
                }
                None => e.reads += 1,
            }
            if e.reads + e.writes <= 6 && self.first.len() < 2000 {
                self.first.push(line(pc, a, write, size));
            }
        }
        if let Some((lo, hi)) = self.watch
            && now >= self.watch_from
            && (lo..=hi).contains(&a)
            && self.watched.len() < 400_000
        {
            self.watched.push(format!("{now:>10} {}", line(pc, a, write, size)));
        }
    }

    pub(crate) fn mem_write(&mut self, now: u64, pc: u32, a: u32, v: u32, size: u32) {
        if let Some((lo, hi)) = self.rwatch
            && (lo..=hi).contains(&a)
            && self.rwatched.len() < 400_000
        {
            self.rwatched.push(format!("{now:>10} {pc:08x}  W{size} {a:08x} = {v:08x}"));
        }
    }
}

fn line(pc: u32, a: u32, write: Option<u32>, size: u32) -> String {
    match write {
        Some(v) => format!("{pc:08x}  W{size} {a:08x} = {v:08x}"),
        None => format!("{pc:08x}  R{size} {a:08x}"),
    }
}
