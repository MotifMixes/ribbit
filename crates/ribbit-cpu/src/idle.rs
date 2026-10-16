//! Support for exact idle-loop fast-forwarding (used by the machine).
//!
//! The machine runs one loop iteration while watching it; if the CPU ends the
//! iteration in exactly the state it started in (`IdleSnapshot`, everything
//! architectural except time), and memory and devices are unchanged, every
//! further iteration is identical until the next event, so time can be
//! advanced in one go with `skip_idle`.

use crate::Cpu;

/// All architectural CPU state except time (cycle counts, timer counts).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct IdleSnapshot {
    regs: [u32; crate::NUM_CORE_REGS],
    pc: u32,
    status: [u32; 3],
    lp: [u32; 2],
    irq: [u32; 5],
    other: [u32; 3],
    mac_acc: [i64; 2],
    timers: [(u32, u32, bool); 2],
    delay_target: Option<u32>,
    sleeping: bool,
    xy: Option<crate::xy::XyExtension>,
}

impl Cpu {
    pub fn idle_snapshot(&self) -> IdleSnapshot {
        IdleSnapshot {
            regs: self.regs,
            pc: self.pc,
            status: [self.status32, self.status32_l1, self.status32_l2],
            lp: [self.lp_start, self.lp_end],
            irq: [self.int_vector_base, self.aux_irq_lv12, self.aux_irq_lev, self.aux_irq_hint, self.irq_lines],
            other: [self.semaphore, self.debug, self.macmode],
            mac_acc: self.mac_acc,
            timers: [0, 1].map(|n| (self.timers[n].control, self.timers[n].limit, self.timers[n].pending)),
            delay_target: self.delay_target,
            sleeping: self.sleeping,
            xy: self.xy.clone(),
        }
    }

    /// Cycles that can pass before a processor timer reaches LIMIT (the
    /// only time-driven CPU state change).
    pub fn cycles_to_timer_event(&self) -> u64 {
        self.timers.iter().map(|t| t.limit.wrapping_sub(t.count) as u64).min().unwrap_or(u64::MAX)
    }

    /// Advances time by `cycles` (at most `cycles_to_timer_event`) while the
    /// CPU sleeps: exactly what that many sleeping steps (one cycle each,
    /// `step_with`) do. Loads in flight complete as they would.
    pub fn skip_sleep(&mut self, cycles: u64) {
        debug_assert!(self.sleeping && cycles <= self.cycles_to_timer_event());
        self.cycles += cycles;
        for t in &mut self.timers {
            t.count = t.count.wrapping_add(cycles as u32);
        }
    }

    /// Advances time by `cycles` (at most `cycles_to_timer_event`) as if
    /// `instructions` idle instructions had executed.
    pub fn skip_idle(&mut self, cycles: u64, instructions: u64) {
        debug_assert!(cycles <= self.cycles_to_timer_event());
        // A load still in flight arrives as many cycles later as are skipped
        // (an executed run would have reissued it in the last iteration).
        if self.ready_max > self.cycles {
            for r in &mut self.ready {
                if *r > self.cycles {
                    *r += cycles;
                }
            }
            self.ready_max += cycles;
        }
        self.cycles += cycles;
        self.instructions_retired += instructions;
        for t in &mut self.timers {
            t.count = t.count.wrapping_add(cycles as u32);
        }
    }
}
