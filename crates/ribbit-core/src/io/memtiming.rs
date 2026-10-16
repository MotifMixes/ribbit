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
//!   with DMA, SDRAM rows, critical-word-first, `.di` (uncached) accesses to
//!   memory (the bus cannot see the flag; they are timed as cached).

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
}

const SETS: usize = 512;
const WAYS: usize = 4;
const VALID: u32 = 1 << 31;
const DIRTY: u32 = 1 << 30;

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
        let hit = |w: usize| ways[w] & !DIRTY == tag;
        // Unrolled (the iterator form was a loop with a bounds check per way).
        if hit(0) {
            Some(set * WAYS)
        } else if hit(1) {
            Some(set * WAYS + 1)
        } else if hit(2) {
            Some(set * WAYS + 2)
        } else if hit(3) {
            Some(set * WAYS + 3)
        } else {
            None
        }
    }

    /// `find`, answered from `last` when it is the same line.
    #[inline(always)]
    fn find_after(&self, addr: u32, last: LastLine) -> Option<usize> {
        if addr >> 4 == last.line { Some(last.slot) } else { self.find(addr) }
    }

    /// Brings the line of `addr` in; returns its slot and whether the line
    /// it replaced was dirty.
    fn fill(&mut self, addr: u32) -> (usize, bool) {
        let set = (addr >> 4) as usize % SETS;
        let way = self.next[set] as usize;
        self.next[set] = ((way + 1) % WAYS) as u8;
        let i = set * WAYS + way;
        let evicted_dirty = self.tags[i] & (VALID | DIRTY) == VALID | DIRTY;
        self.tags[i] = addr >> 4 | VALID;
        (i, evicted_dirty)
    }
}

/// The last line looked up in one cache and its slot (speed only): a hit
/// changes no cache state and only that cache's own fills evict, so until
/// its next fill the line is still at that slot. Not state: compares equal,
/// not saved.
#[derive(Debug, Clone, Copy)]
struct LastLine {
    /// Address >> 4 (u32::MAX: none).
    line: u32,
    slot: usize,
}

impl Default for LastLine {
    fn default() -> Self {
        Self { line: u32::MAX, slot: 0 }
    }
}

impl PartialEq for LastLine {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for LastLine {}

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
    #[serde(skip)]
    last_code: LastLine,
    #[serde(skip)]
    last_data: LastLine,
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
            last_code: LastLine::default(),
            last_data: LastLine::default(),
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

    #[inline(always)]
    fn fetch(&mut self, addr: u32) {
        if addr >> 4 == self.last_code.line {
            return;
        }
        let slot = match self.code.find(addr) {
            Some(i) => i,
            None => {
                self.stall += fill_cost(addr);
                self.misses[0] += 1;
                self.code.fill(addr).0
            }
        };
        self.last_code = LastLine { line: addr >> 4, slot };
    }

    /// A memory (not device) access by the executing instruction.
    #[inline(always)]
    pub fn data(&mut self, addr: u32, write: bool) {
        if !self.armed {
            return;
        }
        let slot = match self.data.find_after(addr, self.last_data) {
            Some(i) => i,
            None => {
                let (i, dirty) = self.data.fill(addr);
                self.stall += fill_cost(addr);
                self.misses[1] += 1;
                if dirty {
                    self.stall += cost::WRITE_BACK;
                    self.misses[2] += 1;
                }
                i
            }
        };
        self.last_data = LastLine { line: addr >> 4, slot };
        if write {
            self.data.tags[slot] |= DIRTY;
        }
    }

    /// A device-register access by the executing instruction.
    #[inline(always)]
    pub fn device(&mut self) {
        if self.armed {
            self.stall += cost::DEVICE;
        }
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
