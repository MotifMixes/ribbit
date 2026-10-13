//! Memory-mapped timers in the 0x0180_D000 block.
//!
//! Evidence (docs/hardware-research.md §Timers):
//! * MAME `leapster.cpp`: three timers, registers count (+0), control (+4)
//!   and max (+8); timer 0 at 0x0180_D084, timer 1 at 0x0180_D400, timer 2 at
//!   0x0180_D800; 16 MHz; IRQs 0x19, 0x1A, 0x1E; control bit 0 enables the IRQ.
//! * The v1.5/v2.1 BIOS: busy-waits for timer 2 to count up from 0 to
//!   0x3E80/0x27100 with control = 0; the MQX BSP declares 100 ticks/s and
//!   0x27100 timer counts per tick (100 x 160,000 = 16,000,000 counts/s),
//!   installs its tick ISR on vector 0x19, and programs timer 0 as
//!   count = 0x1000000 - 0x27100, max = 0x1000000, control = 3.
//! * The register order and the control value 3 match ARC's own processor
//!   timers (COUNT/CONTROL/LIMIT; IE = bit 0, NH = bit 1). Ribbit therefore
//!   uses the ARC semantics where MAME is silent: the counter reaches LIMIT,
//!   restarts from 0 on the next count, raises a level interrupt if IE, and a
//!   write to CONTROL de-asserts it. HYPOTHESIS.

/// One memory-mapped timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioTimer {
    pub count: u32,
    pub control: u32,
    pub limit: u32,
    /// Interrupt output (level): set on reaching the limit with IE set,
    /// cleared by writing CONTROL.
    pub pending: bool,
    /// Times the counter wrapped (diagnostics only).
    pub wraps: u64,
}

impl MmioTimer {
    pub const IE: u32 = 1 << 0;
    /// "Not halted": count only while the CPU is running (ARC semantics).
    pub const NH: u32 = 1 << 1;

    /// Power-on state. MAME resets max to 0xFFFF_FFFF and the count and
    /// control to 0; the hardware reset value is UNKNOWN.
    pub fn reset() -> Self {
        Self { count: 0, control: 0, limit: u32::MAX, pending: false, wraps: 0 }
    }

    pub fn read(&self, reg: TimerReg) -> u32 {
        match reg {
            TimerReg::Count => self.count,
            TimerReg::Control => self.control,
            TimerReg::Limit => self.limit,
        }
    }

    pub fn write(&mut self, reg: TimerReg, value: u32) {
        match reg {
            TimerReg::Count => self.count = value,
            TimerReg::Control => {
                self.control = value;
                self.pending = false;
            }
            TimerReg::Limit => self.limit = value,
        }
    }

    /// Counts until the next wrap (the only time `pending` can change).
    pub fn ticks_to_wrap(&self) -> u64 {
        self.limit.wrapping_sub(self.count) as u64 + 1
    }

    /// Advances by `ticks` timer counts.
    pub fn advance(&mut self, mut ticks: u64, cpu_halted: bool) {
        if ticks == 0 || (cpu_halted && self.control & Self::NH != 0) {
            return;
        }
        // Counts until the counter equals LIMIT (wrapping through 2^32 if it
        // is already above it), then one more count returns it to 0.
        let to_limit = self.limit.wrapping_sub(self.count) as u64;
        if ticks <= to_limit {
            self.count = self.count.wrapping_add(ticks as u32);
            return;
        }
        ticks -= to_limit + 1;
        self.wrap();
        let period = self.limit as u64 + 1;
        if ticks >= period {
            self.wraps += ticks / period;
            ticks %= period;
        }
        self.count = ticks as u32;
    }

    fn wrap(&mut self) {
        self.wraps += 1;
        if self.control & Self::IE != 0 {
            self.pending = true;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimerReg {
    Count,
    Control,
    Limit,
}

/// Base address of each timer's COUNT register.
pub const TIMER_BASES: [u32; 3] = [0x0180_D084, 0x0180_D400, 0x0180_D800];

/// CPU interrupt line of each timer (MAME; timer 0 confirmed by the BIOS
/// installing its MQX tick handler on vector 0x19).
pub const TIMER_IRQS: [u8; 3] = [0x19, 0x1A, 0x1E];

/// Decodes a timer register address.
pub fn decode(addr: u32) -> Option<(usize, TimerReg)> {
    TIMER_BASES.iter().enumerate().find_map(|(n, &base)| {
        let reg = match addr.wrapping_sub(base) {
            0 => TimerReg::Count,
            4 => TimerReg::Control,
            8 => TimerReg::Limit,
            _ => return None,
        };
        Some((n, reg))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_the_three_timer_blocks() {
        assert_eq!(decode(0x0180_D084), Some((0, TimerReg::Count)));
        assert_eq!(decode(0x0180_D088), Some((0, TimerReg::Control)));
        assert_eq!(decode(0x0180_D08C), Some((0, TimerReg::Limit)));
        assert_eq!(decode(0x0180_D404), Some((1, TimerReg::Control)));
        assert_eq!(decode(0x0180_D808), Some((2, TimerReg::Limit)));
        assert_eq!(decode(0x0180_D080), None);
        assert_eq!(decode(0x0180_D80C), None);
        assert_eq!(decode(0x0180_D510), None);
    }

    #[test]
    fn counts_up_without_interrupt_enable() {
        // The BIOS delay loop: limit = 0xFFFF_FFFF, control = 0, count = 0.
        let mut t = MmioTimer::reset();
        t.write(TimerReg::Limit, u32::MAX);
        t.write(TimerReg::Control, 0);
        t.write(TimerReg::Count, 0);
        t.advance(159_999, false);
        assert_eq!(t.read(TimerReg::Count), 159_999);
        t.advance(1, false);
        assert_eq!(t.read(TimerReg::Count), 0x27100);
        assert!(!t.pending);
    }

    #[test]
    fn wraps_after_reaching_limit_and_raises_interrupt() {
        let mut t = MmioTimer::reset();
        t.write(TimerReg::Limit, 0xFF);
        t.write(TimerReg::Count, 0xFE);
        t.write(TimerReg::Control, MmioTimer::IE);
        t.advance(1, false);
        assert_eq!((t.count, t.pending), (0xFF, false), "reaching the limit is not yet a wrap");
        t.advance(1, false);
        assert_eq!((t.count, t.pending, t.wraps), (0, true, 1));
        t.advance(5, false);
        assert!(t.pending, "level interrupt stays until CONTROL is written");
        t.write(TimerReg::Control, MmioTimer::IE | MmioTimer::NH);
        assert!(!t.pending);
        assert_eq!(t.count, 5);
    }

    #[test]
    fn bios_tick_programming_gives_160000_count_period() {
        // count = 0x1000000 - 0x27100, limit = 0x1000000, control = 3.
        let mut t = MmioTimer::reset();
        t.write(TimerReg::Control, 0);
        t.write(TimerReg::Limit, 0x0100_0000);
        t.write(TimerReg::Count, 0x00FD_8F00);
        t.write(TimerReg::Control, 3);
        t.advance(160_000, false);
        assert!(!t.pending, "ARC semantics: count == limit for one tick");
        t.advance(1, false);
        assert!(t.pending);
        assert_eq!(t.count, 0);
    }

    #[test]
    fn large_advance_is_equivalent_to_single_steps() {
        let mut bulk = MmioTimer::reset();
        let mut single = MmioTimer::reset();
        for t in [&mut bulk, &mut single] {
            t.write(TimerReg::Limit, 999);
            t.write(TimerReg::Count, 990);
            t.write(TimerReg::Control, MmioTimer::IE);
        }
        bulk.advance(12_345, false);
        for _ in 0..12_345 {
            single.advance(1, false);
        }
        assert_eq!(bulk, single);
    }

    #[test]
    fn not_halted_mode_stops_while_cpu_halted() {
        let mut t = MmioTimer::reset();
        t.write(TimerReg::Control, MmioTimer::NH);
        t.advance(10, true);
        assert_eq!(t.count, 0);
        t.write(TimerReg::Control, 0);
        t.advance(10, true);
        assert_eq!(t.count, 10);
    }
}
