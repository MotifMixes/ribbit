//! AppManager's idle timer. CAppManager::UpdateInactivity (in
//! libLightningBase.so) adds the time since its last call to a static
//! `inactivityTimer` and zeroes it when a button was pressed or the screen
//! touched, then:
//!
//! * Didj (/Didj/Base/lib; disassembled, 2026-11-08): past 120 s it dims the
//!   screen, past 420 s (600 s paused) it shuts the Didj down.
//! * Leapster Explorer (/LF/Base/lib, Firmware Base; disassembled,
//!   2026-11-15, UpdateInactivity at 0x36148): the same states with other
//!   limits, compared at 0x362d4 (119 999 ms: dim), 0x3632c (59 999 ms),
//!   0x3641c (239 999 ms: "System shutting down due to inactivity after %d
//!   [ms] !") and 0x3622c (599 999 ms, paused). So it dims after 2 minutes
//!   and shuts down after 4 (10 paused). The timer is reached pc-relative,
//!   with no relocation (`ldr r3,[pc,#0x428]` = 0x30964: 0x66AD8 + 0x314 =
//!   0x66DEC, in .bss).
//!
//! "Turn off when idle" off (Ribbit's default): the timer is kept at 0 from
//! outside, as the Leapster's BIOS count is refilled, on every mode. Found
//! through the running process's page tables; a library that isn't one of
//! these (other firmware) is left alone.

use std::ops::Range;

/// One firmware's libLightningBase.so: offsets of UpdateInactivity and its
/// first instructions, a literal of the function that must read as given,
/// and the timer.
struct Library {
    code: u32,
    prologue: [u32; 3],
    literal: u32,
    literal_value: Literal,
    timer: u32,
    /// Timer readings from here on mean the shutdown was for idling
    /// (readings come about a second apart).
    near_shutdown_ms: u32,
}

#[derive(Clone, Copy)]
enum Literal {
    /// The timer's address, relocated at load: base + timer.
    TimerAddress,
    /// A pc-relative offset, as in the file.
    Word(u32),
}

const KNOWN: [Library; 2] = [
    // Didj: shuts down at 420 s.
    Library {
        code: 0x2_8768,
        prologue: [0xE92D_4FF0, 0xE59F_55A4, 0xE280_4014],
        literal: 0x2_8D1C,
        literal_value: Literal::TimerAddress,
        timer: 0x4_8458,
        near_shutdown_ms: 400_000,
    },
    // Leapster Explorer: shuts down at 240 s.
    Library {
        code: 0x3_6148,
        prologue: [0xE92D_45F0, 0xE59F_643C, 0xE280_7014],
        literal: 0x3_6594,
        literal_value: Literal::Word(0x3_0964),
        timer: 0x6_6DEC,
        near_shutdown_ms: 220_000,
    },
];
/// Where Linux and the HLE load libraries (TASK_UNMAPPED_BASE on; the
/// Explorer's AppManager maps more than the Didj's).
const LIBRARIES: Range<u32> = 0x4000_0000..0x4800_0000;
#[cfg(test)]
const ELF_MAGIC: u32 = 0x464C_457F;

/// A virtual address's place in SDRAM, by the ARM page tables at `ttb`.
/// SDRAM's physical address bit: the Didj's SDRAM is at 0, the Explorer's
/// at 0x8000_0000 (its page tables hold 0x8xxx_xxxx addresses).
const SDRAM_HIGH: u32 = 0x8000_0000;

fn translate(sdram: &[u8], ttb: u32, va: u32) -> Option<usize> {
    let word = |a: u32| {
        let a = (a & !SDRAM_HIGH) as usize;
        Some(u32::from_le_bytes(sdram.get(a..a + 4)?.try_into().unwrap()))
    };
    let l1 = word((ttb & !0x3FFF) + (va >> 20) * 4)?;
    let pa = match l1 & 3 {
        1 => {
            let l2 = word((l1 & !0x3FF) + ((va >> 12) & 0xFF) * 4)?;
            match l2 & 3 {
                1 => (l2 & !0xFFFF) | (va & 0xFFFF),
                2 => (l2 & !0xFFF) | (va & 0xFFF),
                _ => return None,
            }
        }
        2 => (l1 & !0xF_FFFF) | (va & 0xF_FFFF),
        _ => return None,
    };
    let pa = (pa & !SDRAM_HIGH) as usize;
    (pa + 4 <= sdram.len()).then_some(pa)
}

fn read(sdram: &[u8], ttb: u32, va: u32) -> Option<u32> {
    let pa = translate(sdram, ttb, va)?;
    Some(u32::from_le_bytes(sdram[pa..pa + 4].try_into().unwrap()))
}

/// Which known library (an index in KNOWN), if any, is at `base`. Its
/// first page (the ELF header) need not be mapped: a process maps a
/// library's pages as it first touches them, and the Explorer's AppManager
/// never touches that one. The function and its literal share a page,
/// which runs every frame.
fn library_at(sdram: &[u8], ttb: u32, base: u32) -> Option<usize> {
    KNOWN.iter().position(|l| {
        (0..3).all(|i| read(sdram, ttb, base + l.code + 4 * i as u32) == Some(l.prologue[i]))
            && read(sdram, ttb, base + l.literal)
                == Some(match l.literal_value {
                    Literal::TimerAddress => base + l.timer,
                    Literal::Word(w) => w,
                })
    })
}

/// Research: where the known UpdateInactivity code is in SDRAM (physical
/// pages), and the virtual addresses the page tables at `ttb` map to them.
#[doc(hidden)]
pub fn locate(sdram: &[u8], ttb: u32) -> Vec<(usize, Vec<u32>)> {
    let word = |a: usize| sdram.get(a..a + 4).map_or(0, |w| u32::from_le_bytes(w.try_into().unwrap()));
    let mut out = Vec::new();
    for l in &KNOWN {
        let off = (l.code & 0xFFF) as usize;
        for page in (0..sdram.len()).step_by(0x1000) {
            if (0..3).all(|i| word(page + off + 4 * i) == l.prologue[i]) {
                let vas = (0..0xC000_0000u32)
                    .step_by(0x1000)
                    .filter(|&va| translate(sdram, ttb, va).is_some_and(|pa| pa & !0xFFF == page))
                    .collect();
                out.push((page, vas));
            }
        }
    }
    out
}

/// One console's view of the timer; `tick` about once a second.
#[derive(Default)]
pub struct IdleTimer {
    /// Where the library is, and which one (an index in KNOWN).
    base: Option<(u32, usize)>,
    /// The timer's last reading, milliseconds.
    pub last_ms: u32,
}

impl IdleTimer {
    /// Looks at the timer in the address space at `ttb` (the running
    /// process's: another process's has no such library) and, with
    /// `keep_awake`, puts it back to 0.
    pub fn tick(&mut self, sdram: &mut [u8], ttb: u32, keep_awake: bool) {
        let found = match self.base {
            Some((base, lib)) if library_at(sdram, ttb, base) == Some(lib) => Some((base, lib)),
            _ => LIBRARIES.step_by(0x1000).find_map(|b| library_at(sdram, ttb, b).map(|lib| (b, lib))),
        };
        self.base = found.or(self.base);
        let Some(pa) = found.and_then(|(b, lib)| translate(sdram, ttb, b + KNOWN[lib].timer)) else { return };
        self.last_ms = u32::from_le_bytes(sdram[pa..pa + 4].try_into().unwrap());
        // Only once it has counted: then the page is the process's own, not
        // the file's.
        if keep_awake && self.last_ms != 0 {
            sdram[pa..pa + 4].fill(0);
            self.last_ms = 0;
        }
    }

    /// Whether a power-off now is the idle shutdown (not a cartridge's
    /// update or a crash).
    pub fn near_shutdown(&self) -> bool {
        self.base.is_some_and(|(_, lib)| self.last_ms >= KNOWN[lib].near_shutdown_ms)
    }

    /// The timer's last reading (milliseconds) and the reading that marks
    /// an idle shutdown, once the library is found.
    pub fn reading(&self) -> Option<(u32, u32)> {
        self.base.map(|(_, lib)| (self.last_ms, KNOWN[lib].near_shutdown_ms))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CODE: u32 = KNOWN[0].code;
    const TIMER: u32 = KNOWN[0].timer;

    /// A library at 0x4000_0000 in small pages mapped one to one above 1 MB,
    /// with a coarse table at 0x4000 and the first-level one at 0: the
    /// Didj's (`lib` 0) or the Explorer's (1).
    fn console_with(lib: usize) -> (Vec<u8>, u32) {
        let mut m = vec![0u8; 0x20_0000];
        let put = |m: &mut Vec<u8>, a: u32, v: u32| m[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
        let base = 0x4000_0000u32;
        put(&mut m, (base >> 20) * 4, 0x4000 | 1);
        for p in 0..0x70 {
            put(&mut m, 0x4000 + p * 4, (0x10_0000 + p * 0x1000) | 2);
        }
        let at = |off: u32| 0x10_0000 + off;
        put(&mut m, at(0), ELF_MAGIC);
        let l = &KNOWN[lib];
        for (i, w) in l.prologue.iter().enumerate() {
            put(&mut m, at(l.code) + 4 * i as u32, *w);
        }
        let literal = match l.literal_value {
            Literal::TimerAddress => base + l.timer,
            Literal::Word(w) => w,
        };
        put(&mut m, at(l.literal), literal);
        put(&mut m, at(l.timer), 12_345);
        (m, 0)
    }

    fn console() -> (Vec<u8>, u32) {
        console_with(0)
    }

    #[test]
    fn finds_the_explorers_timer() {
        let (mut m, ttb) = console_with(1);
        let mut t = IdleTimer::default();
        t.tick(&mut m, ttb, false);
        assert_eq!((t.base, t.last_ms), (Some((0x4000_0000, 1)), 12_345));
        t.last_ms = 230_000;
        assert!(t.near_shutdown(), "past 220 s on the Explorer");
        t.tick(&mut m, ttb, true);
        assert_eq!(read(&m, ttb, 0x4000_0000 + KNOWN[1].timer), Some(0));
    }

    #[test]
    fn keeps_the_timer_at_zero() {
        let (mut m, ttb) = console();
        let mut t = IdleTimer::default();
        t.tick(&mut m, ttb, true);
        assert_eq!(read(&m, ttb, 0x4000_0000 + TIMER), Some(0));
        assert_eq!(t.base, Some((0x4000_0000, 0)));
    }

    #[test]
    fn only_reads_when_turning_off_is_on() {
        let (mut m, ttb) = console();
        let mut t = IdleTimer::default();
        t.tick(&mut m, ttb, false);
        assert_eq!(t.last_ms, 12_345);
        assert!(!t.near_shutdown());
        assert_eq!(read(&m, ttb, 0x4000_0000 + TIMER), Some(12_345));
    }

    #[test]
    fn other_firmware_is_left_alone() {
        let (mut m, ttb) = console();
        let code = 0x10_0000 + CODE as usize;
        m[code] ^= 1;
        let mut t = IdleTimer::default();
        t.tick(&mut m, ttb, true);
        assert_eq!(t.base, None);
        assert_eq!(read(&m, ttb, 0x4000_0000 + TIMER), Some(12_345));
    }
}
