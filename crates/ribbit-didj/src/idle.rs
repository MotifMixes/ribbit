//! AppManager's idle timer. CAppManager::UpdateInactivity (in
//! /Didj/Base/lib/libLightningBase.so; disassembled, 2026-11-08) adds the time
//! since its last call to the static `inactivityTimer` and zeroes it when a
//! button was pressed: past 120 s it dims the screen, past 420 s (600 s
//! paused) it shuts the Didj down.
//!
//! "Turn off when idle" off (Ribbit's default): the timer is kept at 0 from
//! outside, as the Leapster's BIOS count is refilled, on every Didj mode.
//! Found through the running process's page tables; a library that isn't
//! this one (other firmware) is left alone.

use std::ops::Range;

/// Offsets in libLightningBase.so: UpdateInactivity, its first instructions
/// and the literal holding the timer's address (relocated at load), and
/// the timer.
const CODE: u32 = 0x2_8768;
const PROLOGUE: [u32; 3] = [0xE92D_4FF0, 0xE59F_55A4, 0xE280_4014];
const TIMER_LITERAL: u32 = 0x2_8D1C;
const TIMER: u32 = 0x4_8458;
/// Where Linux and the HLE load libraries (TASK_UNMAPPED_BASE on).
const LIBRARIES: Range<u32> = 0x4000_0000..0x4200_0000;
const ELF_MAGIC: u32 = 0x464C_457F;
/// Timer readings from here on mean the shutdown was for idling: it goes at
/// 420 s, and readings come about a second apart.
const NEAR_SHUTDOWN_MS: u32 = 400_000;

/// A virtual address's place in SDRAM, by the ARM page tables at `ttb`.
fn translate(sdram: &[u8], ttb: u32, va: u32) -> Option<usize> {
    let word = |a: u32| Some(u32::from_le_bytes(sdram.get(a as usize..a as usize + 4)?.try_into().unwrap()));
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
    ((pa as usize) + 4 <= sdram.len()).then_some(pa as usize)
}

fn read(sdram: &[u8], ttb: u32, va: u32) -> Option<u32> {
    let pa = translate(sdram, ttb, va)?;
    Some(u32::from_le_bytes(sdram[pa..pa + 4].try_into().unwrap()))
}

/// Whether this library, at `base`, is the one the offsets are for.
fn is_library(sdram: &[u8], ttb: u32, base: u32) -> bool {
    read(sdram, ttb, base) == Some(ELF_MAGIC)
        && (0..3).all(|i| read(sdram, ttb, base + CODE + 4 * i as u32) == Some(PROLOGUE[i]))
        && read(sdram, ttb, base + TIMER_LITERAL) == Some(base + TIMER)
}

/// One console's view of the timer; `tick` about once a second.
#[derive(Default)]
pub struct IdleTimer {
    base: Option<u32>,
    /// The timer's last reading, milliseconds.
    pub last_ms: u32,
}

impl IdleTimer {
    /// Looks at the timer in the address space at `ttb` (the running
    /// process's: another process's has no such library) and, with
    /// `keep_awake`, puts it back to 0.
    pub fn tick(&mut self, sdram: &mut [u8], ttb: u32, keep_awake: bool) {
        let found = match self.base {
            Some(base) if is_library(sdram, ttb, base) => Some(base),
            _ => LIBRARIES.step_by(0x1000).find(|&b| is_library(sdram, ttb, b)),
        };
        self.base = found.or(self.base);
        let Some(pa) = found.and_then(|b| translate(sdram, ttb, b + TIMER)) else { return };
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
        self.last_ms >= NEAR_SHUTDOWN_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A library at 0x4000_0000 in small pages mapped one to one above 1 MB,
    /// with a coarse table at 0x4000 and the first-level one at 0.
    fn console() -> (Vec<u8>, u32) {
        let mut m = vec![0u8; 0x20_0000];
        let put = |m: &mut Vec<u8>, a: u32, v: u32| m[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
        let base = 0x4000_0000u32;
        put(&mut m, (base >> 20) * 4, 0x4000 | 1);
        for p in 0..0x50 {
            put(&mut m, 0x4000 + p * 4, (0x10_0000 + p * 0x1000) | 2);
        }
        let at = |off: u32| 0x10_0000 + off;
        put(&mut m, at(0), ELF_MAGIC);
        for (i, w) in PROLOGUE.iter().enumerate() {
            put(&mut m, at(CODE) + 4 * i as u32, *w);
        }
        put(&mut m, at(TIMER_LITERAL), base + TIMER);
        put(&mut m, at(TIMER), 12_345);
        (m, 0)
    }

    #[test]
    fn keeps_the_timer_at_zero() {
        let (mut m, ttb) = console();
        let mut t = IdleTimer::default();
        t.tick(&mut m, ttb, true);
        assert_eq!(read(&m, ttb, 0x4000_0000 + TIMER), Some(0));
        assert_eq!(t.base, Some(0x4000_0000));
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
