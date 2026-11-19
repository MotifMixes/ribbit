//! Memory-system timing: the CPU's instruction and data caches and what a
//! miss, a write-back and a device access cost, charged as stall cycles to
//! the instruction that caused them (`Bus::take_stall`).
//!
//! Evidence (docs/hardware-research.md, "CPU timing, caches and clocks"):
//! * Geometry, VERIFIED-FW: the BIOS's cache-locking routines (US v2.1
//!   0x4003_8DF8, 0x4003_8EC8) give each cache 4 ways x 512 sets of 16-byte
//!   lines (32 KB); the BIOS enables both (IC_CTRL 2, DC_CTRL 0x8A). The
//!   data cache has flush operations, so it is taken as write-back with
//!   write-allocate (HYPOTHESIS).
//! * Replacement: UNKNOWN. Round-robin per set, advanced on a fill only, so
//!   hits never change cache state (an idle loop leaves it unchanged, which
//!   keeps idle fast-forwarding exact).
//! * Costs: CALIBRATED, not measured. The magnitudes follow Nathan Farlow's
//!   leapster-emu (S10: cartridge-bus line fills of about 200 cycles, SDRAM
//!   about 80, device registers about 40), and are set so that Sonic X
//!   gameplay on Ribbit's Leapster TV model matches a real Leapster TV
//!   capture (17.7 frames/s). Not modelled: line prefetch, bus contention
//!   with DMA, SDRAM rows, critical-word-first.
//!
//! Line contents (`contents`, the Leapster 2; docs/hardware-research.md,
//! "Cache contents"): the caches then also hold data, as the console's do
//! (measured with LeapLoader's LCD Timing app, v5). The CPU's view of a
//! cached line is memory itself (so compiled code reads and writes it in
//! place); each data line keeps what RAM holds there (`ram_copy`), taken
//! when the line is filled. RAM is then that copy while the line is cached:
//! `.di` loads and stores, devices (DMA, the LCD) and the LCD's picture use
//! it, and a cached load doesn't see an `st.di` until the line leaves the
//! cache. A dirty line leaving the cache is written back (memory already
//! holds it: RAM's copy is dropped, also over an `st.di`); a clean one gives
//! memory RAM's copy back. Code lines keep the bytes they were filled with
//! (`code_bytes`): code written as data runs only once the data cache has
//! written it back and the code cache was invalidated (IC_IVIC) or lost the
//! line. Cache controls: `aux_read`/`aux_write`.

/// Peripheral-clock-independent CPU cycles (at 96 MHz).
pub mod cost {
    /// Line fill from the BIOS ROM or a cartridge (16-bit parallel ROM).
    pub const FILL_ROM: u64 = 200;
    /// Line fill from SDRAM (0x3C00_0000).
    pub const FILL_RAM: u64 = 80;
    /// Line fill from the on-chip RAM (0x0300_0000).
    pub const FILL_SRAM: u64 = 40;
    /// A device-register access (always uncached).
    pub const DEVICE: u64 = 40;
    /// Writing back an evicted dirty line.
    pub const WRITE_BACK: u64 = 42;
    /// An uncached (`.di`) load from memory. Console (v1-v5): VRAM ~55;
    /// SDRAM UNMEASURED, taken as the same.
    pub const UNCACHED_LOAD: u64 = 55;
    /// Posted `.di` stores (console v3-v5): the memory bus takes a word per
    /// period, in tenths of a cycle: VRAM 24.4; SDRAM ~31 (st.di of a whole
    /// frame to RAM cost as much as cached stores, 18.5% of a frame).
    pub const POSTED_VRAM: u64 = 244;
    pub const POSTED_RAM: u64 = 310;
    /// Words the store queue holds (console: at least 8).
    pub const POSTED_DEPTH: u64 = 8;
    /// An `st.di` issued within `POSTED_GAP` cycles of the previous one
    /// costs this much more (console: ~4.6 cycles back to back, ~1 after 5-6
    /// cycles of other work).
    pub const POSTED_ISSUE: u64 = 4;
    pub const POSTED_GAP: u64 = 6;
}

const SETS: usize = 512;
const WAYS: usize = 4;
const VALID: u32 = 1 << 31;
const DIRTY: u32 = 1 << 30;
/// A line's address bits in a tag (address >> 4).
const LINE: u32 = (1 << 28) - 1;
/// DC_CTRL / IC_CTRL bit 0: the cache is off (accesses go to memory).
const CTRL_DISABLE: u32 = 1;
/// DC_CTRL bit 6, IM: invalidating writes dirty lines back first (ARC PRM
/// layout; GUESS for this core, the BIOS's 0x8A leaves it clear).
const CTRL_IM: u32 = 1 << 6;

/// Memory as whole 16-byte lines, for the line contents (`contents`):
/// `get` and `put` without side effects other than a code watch.
pub trait LineMem {
    fn get(&mut self, line: u32) -> [u8; 16];
    fn put(&mut self, line: u32, bytes: &[u8; 16]);
}

/// Memory whose lines can differ from what the cache holds: SDRAM, VRAM and
/// the on-chip RAM (ROM lines are the same everywhere).
pub fn writable(addr: u32) -> bool {
    matches!(addr >> 24, 0x03 | 0x04 | 0x3C..=0x3F)
}

/// A data-cache fill (`MemTiming::data`): the slot, and the tag it replaced.
#[derive(Debug, Clone, Copy)]
pub struct Filled {
    pub slot: usize,
    pub old: u32,
    pub line: u32,
}

/// One 4-way cache: per line its address >> 4, with VALID and DIRTY bits.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Cache {
    tags: Vec<u32>,
    next: Vec<u8>,
}

impl Default for Cache {
    fn default() -> Self {
        Self { tags: vec![0; SETS * WAYS], next: vec![0; SETS] }
    }
}

impl Cache {
    /// The slot holding the line of `addr`, if present.
    #[inline(always)]
    fn find(&self, addr: u32) -> Option<usize> {
        let (tag, set) = (addr >> 4 | VALID, (addr >> 4) as usize % SETS);
        let ways: &[u32; WAYS] = self.tags[set * WAYS..set * WAYS + WAYS].try_into().unwrap();
        let hit = |w: usize| (ways[w] & !DIRTY == tag) as u32;
        // Which way, without a branch per way (a line is in one way at most).
        let m = hit(0) | hit(1) << 1 | hit(2) << 2 | hit(3) << 3;
        if m == 0 { None } else { Some(set * WAYS + m.trailing_zeros() as usize) }
    }

    /// Brings the line of `addr` in; returns its slot and the tag it
    /// replaced.
    fn fill(&mut self, addr: u32) -> (usize, u32) {
        let set = (addr >> 4) as usize % SETS;
        let way = self.next[set] as usize;
        self.next[set] = ((way + 1) % WAYS) as u8;
        let i = set * WAYS + way;
        let old = self.tags[i];
        self.tags[i] = addr >> 4 | VALID;
        (i, old)
    }
}

/// Cost of filling a line at `addr` from its memory.
fn fill_cost(addr: u32) -> u64 {
    match addr >> 24 {
        0x3C..=0x3F => cost::FILL_RAM,
        0x03 => cost::FILL_SRAM,
        // Leapster 2 on-chip RAM (USB buffers): UNVERIFIED, costed as VRAM.
        0x04 => cost::FILL_SRAM,
        _ => cost::FILL_ROM,
    }
}

/// The line contents' state (`MemTiming::lines`).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Lines {
    /// The caches hold data.
    pub contents: bool,
    /// Per data-cache slot: RAM's copy of its line.
    ram_copy: Vec<[u8; 16]>,
    /// Per code-cache slot: the bytes it was filled with.
    code_bytes: Vec<[u8; 16]>,
    /// RAM code lines as last fetched into the code cache: a fill bringing
    /// other bytes makes code compiled from the old ones stale.
    fetched: std::collections::HashMap<u32, [u8; 16]>,
    /// DC_CTRL (aux 0x48) and IC_CTRL (aux 0x11) as written.
    dc_ctrl: u32,
    ic_ctrl: u32,
    /// The store queue (`cost::POSTED_*`): when its last word is through,
    /// in tenths of a cycle, and when the last `st.di` was issued.
    posted_done: u64,
    posted_last: u64,
}

thread_local! {
    /// The save state being read is older than version 9
    /// (`crate::STATE_VERSION`): it has no `Lines`.
    pub(crate) static READING_PRE9: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn lines_any_version<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Lines, D::Error> {
    use serde::Deserialize;
    if READING_PRE9.with(|v| v.get()) { Ok(Lines::default()) } else { Lines::deserialize(d) }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MemTiming {
    pub enabled: bool,
    code: Cache,
    data: Cache,
    /// Between `instruction` and `take_stall`: accesses belong to the
    /// executing instruction (instruction decoding reads are not timed).
    #[serde(skip)]
    armed: bool,
    #[serde(skip)]
    stall: u64,
    /// Misses so far: instruction, data, write-backs (diagnostics, and the
    /// idle fast-forward: an iteration that missed is not repeatable).
    #[serde(skip)]
    pub misses: [u64; 3],
    /// The line contents (see the module notes); the Leapster 2. Read as
    /// off from save states before version 9.
    #[serde(deserialize_with = "lines_any_version")]
    pub lines: Lines,
    /// The access in progress is `.di` (`Bus::set_uncached`).
    #[serde(skip)]
    pub uncached: bool,
    /// Code slots `fetch` filled whose bytes `code_line`/`code_fills` take.
    #[serde(skip)]
    code_pending: Vec<usize>,
}

impl Default for MemTiming {
    fn default() -> Self {
        Self {
            enabled: true,
            code: Cache::default(),
            data: Cache::default(),
            armed: false,
            stall: 0,
            misses: [0; 3],
            lines: Lines::default(),
            uncached: false,
            code_pending: Vec::new(),
        }
    }
}

impl MemTiming {
    /// The instruction at `pc` (`len` bytes) is about to execute.
    #[inline(always)]
    pub fn instruction(&mut self, pc: u32, len: u32) {
        if !self.enabled {
            return;
        }
        self.armed = true;
        let last = pc.wrapping_add(len - 1);
        self.fetch(pc);
        if (last ^ pc) >> 4 != 0 {
            self.fetch(last);
        }
    }

    /// `instruction` for an instruction on the line the one just executed
    /// ended on, not crossing out of it: that line is in the code cache (it
    /// was just fetched; only code fills evict code lines, and a fetch that
    /// crossed lines filled the two adjacent lines, which are in different
    /// sets), so the fetch hits and changes nothing. What `instruction`
    /// would do then.
    #[inline(always)]
    pub fn instruction_same_line(&mut self) {
        if self.enabled {
            self.armed = true;
        }
    }

    /// Empties the code cache (IC_IVIC, "invalidate instruction cache").
    pub fn invalidate_code(&mut self) {
        self.code = Cache::default();
    }

    #[inline(always)]
    fn fetch(&mut self, addr: u32) {
        if self.code.find(addr).is_none() {
            self.stall += fill_cost(addr);
            self.misses[0] += 1;
            let (slot, _) = self.code.fill(addr);
            if self.lines.contents {
                // Filled without its bytes: `code_line` takes them.
                self.code_bytes_stale(slot);
            }
        }
    }

    /// A code line `fetch` filled (timing has no memory) gets its bytes in
    /// `code_fills`.
    #[cold]
    fn code_bytes_stale(&mut self, slot: usize) {
        self.code_pending.push(slot);
    }

    /// A memory (not device) access by the executing instruction. With
    /// `contents`, a fill is returned for `fill_line`.
    #[inline(always)]
    pub fn data(&mut self, addr: u32, write: bool) -> Option<Filled> {
        if !self.armed {
            return None;
        }
        let mut filled = None;
        let slot = match self.data.find(addr) {
            Some(i) => i,
            None => {
                let (i, old) = self.data.fill(addr);
                self.stall += fill_cost(addr);
                self.misses[1] += 1;
                if old & (VALID | DIRTY) == VALID | DIRTY {
                    self.stall += cost::WRITE_BACK;
                    self.misses[2] += 1;
                }
                if self.lines.contents {
                    filled = Some(Filled { slot: i, old, line: addr & !15 });
                }
                i
            }
        };
        if write {
            self.data.tags[slot] |= DIRTY;
        }
        filled
    }

    // ---- Line contents (`contents`) ----

    /// Turns the line contents on or off. Off, memory is the CPU's view
    /// (dirty data stays, RAM's copies of lines are dropped).
    pub fn set_contents(&mut self, on: bool) {
        self.lines.contents = on;
        self.lines.ram_copy = if on { vec![[0; 16]; SETS * WAYS] } else { Vec::new() };
        self.lines.code_bytes = if on { vec![[0; 16]; SETS * WAYS] } else { Vec::new() };
        self.lines.fetched.clear();
        self.code_pending.clear();
        // The caches start empty so every line's copy is taken at its fill.
        self.data = Cache::default();
        self.code = Cache::default();
    }

    /// Whether data accesses go past the cache: `.di`, or the cache is off.
    #[inline(always)]
    pub fn bypass(&self) -> bool {
        self.uncached || self.lines.dc_ctrl & CTRL_DISABLE != 0
    }

    /// After `data` filled a line: the line it replaced leaves (a clean one
    /// gives memory RAM's copy back; a dirty one is written back, which
    /// memory already holds), and RAM's copy of the new line is taken.
    pub fn fill_line(&mut self, f: Filled, mem: &mut impl LineMem) {
        self.leave(f.slot, f.old, false, mem);
        if writable(f.line) {
            self.lines.ram_copy[f.slot] = mem.get(f.line);
        }
    }

    /// Slot `slot` (tag `tag`) leaves the cache: `discard` drops a dirty
    /// line's data instead of writing it back.
    fn leave(&mut self, slot: usize, tag: u32, discard: bool, mem: &mut impl LineMem) {
        let line = (tag & LINE) << 4;
        if tag & VALID == 0 || !writable(line) {
            return;
        }
        if tag & DIRTY == 0 || discard {
            mem.put(line, &self.lines.ram_copy[slot]);
        }
    }

    /// The data-cache slot holding memory line `addr`, if it differs from
    /// RAM there (a writable line in the cache).
    #[inline]
    fn ram_slot(&self, addr: u32) -> Option<usize> {
        if !self.lines.contents || !writable(addr) {
            return None;
        }
        self.data.find(addr)
    }

    /// What RAM holds at `addr` (`width` bytes, little-endian): for devices,
    /// `.di` loads and the LCD.
    pub fn ram_read(&self, addr: u32, width: u32, mem: &mut impl FnMut(u32) -> u32) -> u32 {
        match self.ram_slot(addr) {
            Some(slot) => {
                let line = &self.lines.ram_copy[slot];
                (0..width).fold(0, |v, k| v | (line[((addr + k) & 15) as usize] as u32) << (8 * k))
            }
            None => mem(addr),
        }
    }

    /// Writes RAM at `addr` (`width` bytes): into RAM's copy when the line
    /// is cached (the cache keeps its own data); false if memory itself is
    /// to be written.
    pub fn ram_write(&mut self, addr: u32, width: u32, value: u32) -> bool {
        let Some(slot) = self.ram_slot(addr) else {
            return false;
        };
        let line = &mut self.lines.ram_copy[slot];
        for k in 0..width {
            line[((addr + k) & 15) as usize] = (value >> (8 * k)) as u8;
        }
        true
    }

    /// RAM's picture of memory `base..base + bytes.len()` (`bytes` holds
    /// memory there, the CPU's view): cached lines take RAM's copies.
    pub fn ram_view(&self, base: u32, bytes: &mut [u8]) {
        if !self.lines.contents {
            return;
        }
        let end = base.wrapping_add(bytes.len() as u32);
        for (slot, &tag) in self.data.tags.iter().enumerate() {
            let line = (tag & LINE) << 4;
            if tag & VALID != 0 && line >= base && line < end && writable(line) {
                let o = (line - base) as usize;
                let n = 16.min(bytes.len() - o);
                bytes[o..o + n].copy_from_slice(&self.lines.ram_copy[slot][..n]);
            }
        }
    }

    /// Timing of an uncached access to memory at cycle `now`.
    pub fn uncached_access(&mut self, addr: u32, write: bool, now: u64) {
        if !self.armed {
            return;
        }
        let now10 = now * 10;
        if write {
            let period = if addr >> 24 == 0x03 { cost::POSTED_VRAM } else { cost::POSTED_RAM };
            if now.saturating_sub(self.lines.posted_last) < cost::POSTED_GAP {
                self.stall += cost::POSTED_ISSUE;
            }
            // A full queue holds the store until a word is through.
            let full_until = self.lines.posted_done.saturating_sub((cost::POSTED_DEPTH - 1) * period);
            if full_until > now10 {
                self.stall += (full_until - now10).div_ceil(10);
            }
            self.lines.posted_done = self.lines.posted_done.max(now10) + period;
            self.lines.posted_last = now;
        } else {
            // A load waits for the queued stores (ASSUMED: same bus).
            if self.lines.posted_done > now10 {
                self.stall += (self.lines.posted_done - now10).div_ceil(10);
            }
            self.stall += cost::UNCACHED_LOAD;
        }
    }

    /// The code-cache line holding `addr` (memory that can change), filled
    /// with RAM's bytes if missing; returns the line's bytes and whether a
    /// fill brought other bytes than were last fetched there (code compiled
    /// from them is stale).
    pub fn code_line(&mut self, addr: u32, mem: &mut impl LineMem) -> ([u8; 16], bool) {
        let line = addr & !15;
        if self.lines.ic_ctrl & CTRL_DISABLE != 0 {
            let bytes = self.ram_line(line, mem);
            return (bytes, self.note_fetched(line, bytes));
        }
        let mut changed = self.code_fills(mem);
        let slot = match self.code.find(line) {
            Some(slot) => slot,
            None => {
                self.stall += fill_cost(line);
                self.misses[0] += 1;
                let (slot, _) = self.code.fill(line);
                let bytes = self.ram_line(line, mem);
                self.lines.code_bytes[slot] = bytes;
                changed |= self.note_fetched(line, bytes);
                slot
            }
        };
        (self.lines.code_bytes[slot], changed)
    }

    /// Code-cache fills `fetch` made without bytes get them now; true if
    /// any brought other bytes than were last fetched.
    pub fn code_fills(&mut self, mem: &mut impl LineMem) -> bool {
        let mut changed = false;
        while let Some(slot) = self.code_pending.pop() {
            let l = (self.code.tags[slot] & LINE) << 4;
            if self.code.tags[slot] & VALID != 0 && writable(l) {
                let bytes = self.ram_line(l, mem);
                self.lines.code_bytes[slot] = bytes;
                changed |= self.note_fetched(l, bytes);
            }
        }
        changed
    }

    /// Whether `fetch` left code fills for `code_fills`.
    #[inline(always)]
    pub fn code_fills_pending(&self) -> bool {
        !self.code_pending.is_empty()
    }

    fn note_fetched(&mut self, line: u32, bytes: [u8; 16]) -> bool {
        self.lines.fetched.insert(line, bytes).is_some_and(|old| old != bytes)
    }

    /// RAM's line at `line`: its copy if the data cache holds it.
    fn ram_line(&self, line: u32, mem: &mut impl LineMem) -> [u8; 16] {
        match self.ram_slot(line) {
            Some(slot) => self.lines.ram_copy[slot],
            None => mem.get(line),
        }
    }

    /// Every valid data line leaves: written back first (`write_back`) or
    /// its data dropped.
    fn invalidate_data(&mut self, write_back: bool, mem: &mut impl LineMem) {
        for slot in 0..SETS * WAYS {
            let tag = self.data.tags[slot];
            if tag & (VALID | DIRTY) == VALID | DIRTY && write_back {
                // Written back: memory holds it, RAM's copy is gone.
            } else {
                self.leave(slot, tag, true, mem);
            }
            self.data.tags[slot] = 0;
        }
    }

    /// The cache-control auxiliary registers, with `contents` (otherwise
    /// they stay unmodelled, as before). DC_FLDL (0x4C, flush one line) did
    /// nothing on the console (LeapLoader, 2026-11-01 and llcache.h), so it
    /// does nothing here. DC_FLSH, DC_IVDL and DC_IVDC follow the ARC PRM
    /// (UNVERIFIED on the console). Locking (IC_LIL 0x13, DC_LDL 0x49) and
    /// the cache-RAM registers stay unmodelled.
    pub fn aux_read(&self, reg: u32) -> Option<u32> {
        if !self.lines.contents {
            return None;
        }
        match reg {
            0x11 => Some(self.lines.ic_ctrl),
            // FS (bit 8, flush in progress) never reads set: operations are
            // instant.
            0x48 => Some(self.lines.dc_ctrl & !0x100),
            _ => None,
        }
    }

    pub fn aux_write(&mut self, reg: u32, value: u32, mem: &mut impl LineMem) -> bool {
        if !self.lines.contents {
            return false;
        }
        match reg {
            // IC_IVIC: the code cache is emptied.
            0x10 => {
                self.code = Cache::default();
                self.code_pending.clear();
            }
            0x11 => self.lines.ic_ctrl = value,
            // IC_IVIL: one code line.
            0x19 => {
                if let Some(slot) = self.code.find(value) {
                    self.code.tags[slot] = 0;
                }
            }
            // DC_IVDC: invalidate all (IM: dirty lines written back first).
            0x47 => self.invalidate_data(self.lines.dc_ctrl & CTRL_IM != 0, mem),
            0x48 => self.lines.dc_ctrl = value,
            // DC_IVDL: one line, as DC_IVDC.
            0x4A => {
                if let Some(slot) = self.data.find(value) {
                    let tag = self.data.tags[slot];
                    let write_back = self.lines.dc_ctrl & CTRL_IM != 0 && tag & DIRTY != 0;
                    if !write_back {
                        self.leave(slot, tag, true, mem);
                    }
                    self.data.tags[slot] = 0;
                }
            }
            // DC_FLSH: dirty lines are written back and stay, clean.
            0x4B => {
                for slot in 0..SETS * WAYS {
                    let tag = self.data.tags[slot];
                    if tag & (VALID | DIRTY) == VALID | DIRTY {
                        let line = (tag & LINE) << 4;
                        if writable(line) {
                            self.lines.ram_copy[slot] = mem.get(line);
                        }
                        self.data.tags[slot] = tag & !DIRTY;
                    }
                }
            }
            // DC_FLDL: no effect on the console (see above).
            0x4C => {}
            _ => return false,
        }
        true
    }

    /// A device-register access by the executing instruction.
    #[inline(always)]
    pub fn device(&mut self) {
        if self.armed {
            self.stall += cost::DEVICE;
        }
    }

    /// For a JIT (`ribbit_memory::JitView`): whether it times anything,
    /// `armed`, `stall`, the code and the data caches' tags.
    pub fn jit_parts(&mut self) -> (bool, *mut bool, *mut u64, *const u32, *mut u32) {
        (self.enabled, &mut self.armed, &mut self.stall, self.code.tags.as_ptr(), self.data.tags.as_mut_ptr())
    }

    /// Stall cycles of the instruction just executed.
    #[inline(always)]
    pub fn take_stall(&mut self) -> u64 {
        self.armed = false;
        std::mem::take(&mut self.stall)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn misses_cost_a_fill_hits_are_free_and_state_is_stable_on_hits() {
        let mut m = MemTiming::default();
        m.instruction(0x8000_0000, 4);
        assert_eq!(m.take_stall(), cost::FILL_ROM, "cold cartridge line");
        m.instruction(0x8000_0004, 2);
        assert_eq!(m.take_stall(), 0, "same line");
        m.instruction(0x8000_000E, 4);
        assert_eq!(m.take_stall(), cost::FILL_ROM, "straddles into the next line");
        m.instruction(0x8000_0000, 2);
        m.data(0x3C00_0010, false);
        assert_eq!(m.take_stall(), cost::FILL_RAM);
        let before = m.clone();
        m.instruction(0x8000_0000, 2);
        m.data(0x3C00_0014, false);
        assert_eq!(m.take_stall(), 0);
        assert_eq!(m, before, "hits leave the caches unchanged");
        m.data(0x3C00_0000, false);
        assert_eq!(m.take_stall(), 0, "not armed: decoding reads are free");
    }

    #[test]
    fn dirty_lines_are_written_back_when_replaced() {
        let mut m = MemTiming::default();
        m.instruction(0x4000_0000, 2);
        m.data(0x3C00_0000, true);
        m.take_stall();
        // Four more lines in the same set (8 KB apart) replace it round-robin.
        let mut total = 0;
        for k in 1..=4 {
            m.instruction(0x4000_0000, 2);
            m.data(0x3C00_0000 + k * 0x2000, false);
            total += m.take_stall();
        }
        assert_eq!(total, 4 * cost::FILL_RAM + cost::WRITE_BACK);
        m.instruction(0x4000_0000, 2);
        m.device();
        assert_eq!(m.take_stall(), cost::DEVICE);
    }
}
