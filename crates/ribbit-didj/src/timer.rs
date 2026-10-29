//! The LF1000's five timers and its interrupt controller.
//!
//! Sources: the kernel's include/asm-arm/arch-lf1000/{timer,ic,clkpwr}.h,
//! arch/arm/mach-lf1000/{core,lf1000_core_func}.c and
//! arch/arm/common/ic-lf1000.c (opendidj). A timer counts up and goes back
//! to 0 when it reaches its match value, setting INTPEND ("The LF1000 timer
//! is an up counter, but zeros automaticly when the match value is
//! reached"); TMRCOUNT reads the count latched by writing LDCNT.

pub const TIMER_BASE: u32 = 0xC000_1800;
pub const TIMER_END: u32 = TIMER_BASE + 5 * 0x80;
const TMRCOUNT: u32 = 0x00;
const TMRMATCH: u32 = 0x04;
const TMRCONTROL: u32 = 0x08;
const TMRCLKENB: u32 = 0x40;
const TMRCLKGEN: u32 = 0x44;
const LDCNT: u32 = 1 << 6;
const INTPEND: u32 = 1 << 5;
const INTENB: u32 = 1 << 4;
const RUN: u32 = 1 << 3;
const TIMER_IRQS: [u32; 5] = [4, 11, 15, 21, 43];

pub const IC_BASE: u32 = 0xC000_0800;
pub const IC_END: u32 = IC_BASE + 0x28;
const INTMODEL: u32 = 0x08;
const INTMODEH: u32 = 0x0C;
const INTMASKL: u32 = 0x10;
const INTMASKH: u32 = 0x14;
const INTPENDL: u32 = 0x20;
const INTPENDH: u32 = 0x24;

/// Clock and power registers the timer rate depends on (clkpwr.h).
pub const CLKMODEREG: u32 = 0xC000_F000;
pub const PLLSETREG0: u32 = 0xC000_F004;
const CRYSTAL_HZ: u64 = 27_000_000;

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
struct Timer {
    count: u32,
    latched: u32,
    matchv: u32,
    control: u32,
    clkenb: u32,
    clkgen: u32,
    /// Timer ticks owed, in units of 1/cpu_hz.
    acc: u64,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Timers {
    t: [Timer; 5],
    /// Interrupt controller: pending (1 = pending, write 1 to clear), mask
    /// (1 = masked), mode (1 = FIQ) for sources 0-63.
    pend: u64,
    mask: u64,
    fiq_mode: u64,
}

/// A PLL's output from its PLLSETREG value: 27 MHz / P / 2^S * M (LF1000
/// field layout). None when the register isn't set yet.
pub fn pll_hz(reg: u32) -> Option<u64> {
    let p = (reg >> 18 & 0x3F) as u64;
    let m = (reg >> 8 & 0x3FF) as u64;
    let s = reg & 0xFF;
    (p != 0 && m != 0 && s < 16).then(|| CRYSTAL_HZ / p / (1 << s) * m)
}

impl Timers {
    pub fn new() -> Self {
        Self { mask: !0, ..Default::default() }
    }

    pub fn read(&mut self, addr: u32) -> u32 {
        if (IC_BASE..IC_END).contains(&addr) {
            let lo = |v: u64| v as u32;
            let hi = |v: u64| (v >> 32) as u32;
            // GUESS: INTPEND reads only the unmasked sources (the kernel's
            // entry macro calls it "masked status").
            let seen = self.pend & !self.mask;
            return match addr - IC_BASE {
                INTMODEL => lo(self.fiq_mode),
                INTMODEH => hi(self.fiq_mode),
                INTMASKL => lo(self.mask),
                INTMASKH => hi(self.mask),
                INTPENDL => lo(seen),
                INTPENDH => hi(seen),
                _ => 0,
            };
        }
        let t = &self.t[((addr - TIMER_BASE) / 0x80) as usize];
        match (addr - TIMER_BASE) % 0x80 {
            TMRCOUNT => t.latched,
            TMRMATCH => t.matchv,
            TMRCONTROL => t.control,
            TMRCLKENB => t.clkenb,
            TMRCLKGEN => t.clkgen,
            _ => 0,
        }
    }

    pub fn write(&mut self, addr: u32, v: u32) {
        if (IC_BASE..IC_END).contains(&addr) {
            let set_lo = |x: &mut u64, v: u32| *x = (*x & !0xFFFF_FFFF) | v as u64;
            let set_hi = |x: &mut u64, v: u32| *x = (*x & 0xFFFF_FFFF) | (v as u64) << 32;
            match addr - IC_BASE {
                INTMODEL => set_lo(&mut self.fiq_mode, v),
                INTMODEH => set_hi(&mut self.fiq_mode, v),
                INTMASKL => set_lo(&mut self.mask, v),
                INTMASKH => set_hi(&mut self.mask, v),
                INTPENDL => self.pend &= !(v as u64),
                INTPENDH => self.pend &= !((v as u64) << 32),
                _ => {}
            }
            return;
        }
        let t = &mut self.t[((addr - TIMER_BASE) / 0x80) as usize];
        match (addr - TIMER_BASE) % 0x80 {
            TMRCOUNT => t.count = v,
            TMRMATCH => t.matchv = v,
            TMRCONTROL => {
                if v & LDCNT != 0 {
                    t.latched = t.count;
                }
                let pend = if v & INTPEND != 0 { 0 } else { t.control & INTPEND };
                t.control = (v & !(LDCNT | INTPEND)) | pend;
            }
            TMRCLKENB => t.clkenb = v,
            TMRCLKGEN => t.clkgen = v,
            _ => {}
        }
    }

    /// Advances the timers by `cycles` processor cycles. `cpu_hz` is the
    /// processor clock and `plls` the PLL outputs, as the clock registers
    /// set them.
    pub fn tick(&mut self, cycles: u64, cpu_hz: u64, plls: [Option<u64>; 2]) {
        for (i, t) in self.t.iter_mut().enumerate() {
            if t.control & RUN == 0 {
                continue;
            }
            // TCLK = PLL (TMRCLKGEN bits 1-3) / (bits 4-11 + 1), then the
            // TMRCONTROL SETCLK divider: 1 << ((SETCLK + 1) & 3).
            let Some(Some(pll)) = plls.get((t.clkgen >> 1 & 7) as usize) else { continue };
            let div = (t.clkgen >> 4 & 0xFF) as u64 + 1;
            let setclk = 1u64 << ((t.control + 1) & 3);
            t.acc += cycles * (pll / div / setclk);
            let ticks = t.acc / cpu_hz;
            t.acc %= cpu_hz;
            if ticks > 0 {
                let next = t.count as u64 + ticks;
                let m = t.matchv as u64;
                if m != 0 && next >= m {
                    t.count = (next % m) as u32;
                    t.control |= INTPEND;
                } else {
                    t.count = next as u32;
                }
            }
            if t.control & (INTPEND | INTENB) == INTPEND | INTENB {
                self.pend |= 1 << TIMER_IRQS[i];
            }
        }
    }

    /// Raises interrupt source `irq` (a level that is still high).
    pub fn raise(&mut self, irq: u32) {
        self.pend |= 1 << irq;
    }

    /// The processor's IRQ and FIQ inputs.
    pub fn lines(&self) -> (bool, bool) {
        let active = self.pend & !self.mask;
        (active & !self.fiq_mode != 0, active & self.fiq_mode != 0)
    }
}
