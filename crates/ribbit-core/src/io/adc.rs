//! ADC (touchscreen, battery) at 0x0180_0090-0x0180_00AC.
//!
//! VERIFIED-FW: every BIOS (v1.5 0x4007_CC6A, US v2.1 0x4008_EB78, L-MAX
//! v2.2 0x4009_28A0, Leapster 2 0.6 0x400B_3854-0x400B_3BAC) runs the same
//! driver, and the Leapster 2 0.6 copy was read in full:
//!
//! | Register | Use by the driver |
//! |---|---|
//! | 0x0180_0090 + 4n, n = 0-3 | slot n: bit 15 = running (start 0x400B_3B40 sets it, stop 0x400B_3B78 clears it); bits 16-17 = a mode set at registration (0x400B_3810); bits 0-2 = the analog input (mux, 0x400B_3A5C) |
//! | 0x0180_00A0 | FIFO data: `slot << 16 \| value << 5` (the ISR 0x400B_3854 takes bits 17:16 as the slot and calls that slot's callback with bits 15:5) |
//! | 0x0180_00A8 | status: bit 8 = FIFO empty (the ISR reads data until it is set) |
//! | 0x0180_00A4 | configuration: written 0, then 0xFFFF_FF81 at init (0x400B_3976); the setter 0x400B_3AAC writes 12-bit fields at bits 8-19 and 20-31 |
//! | 0x0180_00AC | written 0 at init |
//!
//! Each sample raises interrupt flag bit 8 (CPU line 0x10); the ISR
//! acknowledges it after draining the FIFO.
//!
//! Slots at boot (every BIOS): slot 2 = input 6, mode 1, a battery reading
//! (the callback 0x400E_2B6C averages samples 2-9 and stops the slot); slot 3 =
//! input 0, mode 1, registered but not started; slot 0 = the touchscreen,
//! mode 0, input 7 for 16 samples (callback 0x400B_56E8 averages samples
//! 9-16), then its state machine (touch.rs), which changes the input and the
//! panel's drive pins after every sample and never stops the slot.
//!
//! Model: while any slot runs, the converter samples the running slots in
//! turn, one conversion every `period()` peripheral ticks. HYPOTHESIS: the
//! period is A4 bits 8-19 plus one (4096 ticks, 256 us, with the BIOS's
//! 0xFFFF_FF81); the real conversion clock, the meaning of the low byte and
//! of the second field, and of the modes are UNKNOWN (the BIOSes give slots
//! of either mode the same treatment). A sample is the selected input's
//! level at the end of the conversion, from `AnalogInputs`.

use super::intc::{FLAG_ADC, IntFlags};
use std::collections::VecDeque;

pub const CONTROL_BASE: u32 = 0x0180_0090;
pub const CONTROL_END: u32 = 0x0180_009C;
pub const DATA_ADDR: u32 = 0x0180_00A0;
pub const CONFIG_ADDR: u32 = 0x0180_00A4;
pub const STATUS_ADDR: u32 = 0x0180_00A8;
pub const AC_ADDR: u32 = 0x0180_00AC;
pub const STATUS_EMPTY: u32 = 1 << 8;
/// Slot control bit 15: the slot converts.
pub const RUNNING: u32 = 1 << 15;
pub const MUX_MASK: u32 = 7;
/// Full scale: an 11-bit result (bits 15:5 of a FIFO word).
pub const FULL_SCALE: u32 = 0x7FF;
/// FIFO depth: UNKNOWN. Samples beyond it are dropped and counted.
pub const FIFO_CAPACITY: usize = 64;

/// The level of each analog input, 0..=FULL_SCALE.
pub trait AnalogInputs {
    fn level(&self, input: u32) -> u32;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Adc {
    /// Packed samples, oldest first.
    pub fifo: VecDeque<u32>,
    /// Reads of the data register while the FIFO was empty.
    pub empty_reads: u64,
    pub controls: [u32; 4],
    pub config: u32,
    pub ac: u32,
    /// Peripheral ticks until the current conversion completes (0 = idle).
    pub busy: u64,
    /// The slot converted last (round-robin).
    pub last_slot: usize,
    pub conversions: u64,
    pub dropped: u64,
}

impl Adc {
    /// Device side: queue a converted sample.
    pub fn push(&mut self, slot: u32, value: u32) {
        if self.fifo.len() >= FIFO_CAPACITY {
            self.dropped += 1;
        } else {
            self.fifo.push_back((slot << 16) | (value.min(FULL_SCALE) << 5));
        }
    }

    /// Peripheral ticks per conversion (HYPOTHESIS, see the module notes).
    pub fn period(&self) -> u64 {
        ((self.config >> 8) & 0xFFF) as u64 + 1
    }

    fn running(&self) -> bool {
        self.controls.iter().any(|c| c & RUNNING != 0)
    }

    pub fn read(&mut self, addr: u32) -> Option<u32> {
        match addr {
            DATA_ADDR => Some(self.fifo.pop_front().unwrap_or_else(|| {
                // MAME returns 0 here; the hardware value is UNKNOWN. The BIOS
                // drain loop discards it.
                self.empty_reads += 1;
                0
            })),
            STATUS_ADDR => Some(if self.fifo.is_empty() { STATUS_EMPTY } else { 0 }),
            CONFIG_ADDR => Some(self.config),
            AC_ADDR => Some(self.ac),
            _ => slot(addr).map(|n| self.controls[n]),
        }
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            CONFIG_ADDR => self.config = value,
            AC_ADDR => self.ac = value,
            _ => match slot(addr) {
                Some(n) => {
                    let was = self.running();
                    self.controls[n] = value;
                    if !was && self.running() {
                        // A conversion starts when the converter goes from idle
                        // to running (UNVERIFIED).
                        self.busy = self.period();
                    }
                }
                None => return false,
            },
        }
        true
    }

    /// Ticks until the next sample (u64::MAX when idle).
    pub fn ticks_to_next(&self) -> u64 {
        if self.running() && self.busy > 0 { self.busy } else { u64::MAX }
    }

    pub fn advance(&mut self, mut ticks: u64, inputs: &impl AnalogInputs, intc: &mut IntFlags) {
        while self.running() && ticks > 0 {
            if self.busy == 0 {
                self.busy = self.period();
            }
            let step = ticks.min(self.busy);
            self.busy -= step;
            ticks -= step;
            if self.busy == 0 {
                self.convert(inputs, intc);
            }
        }
    }

    fn convert(&mut self, inputs: &impl AnalogInputs, intc: &mut IntFlags) {
        let Some(n) = (1..=4).map(|k| (self.last_slot + k) % 4).find(|&n| self.controls[n] & RUNNING != 0) else {
            return;
        };
        self.last_slot = n;
        let value = inputs.level(self.controls[n] & MUX_MASK);
        self.push(n as u32, value);
        self.conversions += 1;
        intc.raise(FLAG_ADC);
    }
}

fn slot(addr: u32) -> Option<usize> {
    ((CONTROL_BASE..=CONTROL_END).contains(&addr) && addr.is_multiple_of(4)).then(|| ((addr - CONTROL_BASE) / 4) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;
    impl AnalogInputs for Fixed {
        fn level(&self, input: u32) -> u32 {
            input * 0x100
        }
    }

    fn drain(adc: &mut Adc) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        while adc.read(STATUS_ADDR).unwrap() & STATUS_EMPTY == 0 {
            let w = adc.read(DATA_ADDR).unwrap();
            out.push(((w >> 16) & 3, (w & 0xFFFF) >> 5));
        }
        out
    }

    fn adc() -> Adc {
        let mut a = Adc::default();
        a.write(CONFIG_ADDR, 0xFFFF_FF81);
        a
    }

    #[test]
    fn empty_fifo_reports_empty() {
        let mut adc = Adc::default();
        assert_eq!(adc.read(STATUS_ADDR), Some(STATUS_EMPTY));
        assert_eq!(adc.read(DATA_ADDR), Some(0));
        assert_eq!(adc.empty_reads, 1);
    }

    #[test]
    fn registers_read_back() {
        let mut a = adc();
        a.write(0x0180_0098, 0x0001_8006);
        assert_eq!(a.read(0x0180_0098), Some(0x0001_8006));
        assert_eq!(a.read(CONFIG_ADDR), Some(0xFFFF_FF81));
        assert_eq!(a.period(), 4096);
        assert!(!a.write(0x0180_00B0, 0));
    }

    #[test]
    fn a_running_slot_converts_its_input_once_per_period() {
        let (mut a, mut intc) = (adc(), IntFlags::default());
        a.advance(100_000, &Fixed, &mut intc);
        assert!(a.fifo.is_empty(), "nothing while every slot is stopped");
        a.write(CONTROL_BASE, RUNNING | 3);
        a.advance(4095, &Fixed, &mut intc);
        assert!(a.fifo.is_empty());
        assert_eq!(a.ticks_to_next(), 1);
        a.advance(1, &Fixed, &mut intc);
        assert_eq!(drain(&mut a), [(0, 0x300)]);
        assert_eq!(intc.flags, FLAG_ADC);
        // The input can change between samples (the touch driver does).
        a.write(CONTROL_BASE, RUNNING | 1);
        a.advance(4096 * 2, &Fixed, &mut intc);
        assert_eq!(drain(&mut a), [(0, 0x100), (0, 0x100)]);
        a.write(CONTROL_BASE, 1);
        a.advance(4096 * 4, &Fixed, &mut intc);
        assert!(a.fifo.is_empty(), "stopped");
    }

    #[test]
    fn running_slots_take_turns() {
        let (mut a, mut intc) = (adc(), IntFlags::default());
        a.write(CONTROL_BASE, RUNNING | 3);
        a.write(0x0180_0098, 0x0001_0000 | RUNNING | 6);
        a.advance(4096 * 4, &Fixed, &mut intc);
        assert_eq!(drain(&mut a), [(2, 0x600), (0, 0x300), (2, 0x600), (0, 0x300)]);
    }

    #[test]
    fn bios_drain_loop_terminates() {
        // Reads data then status until status bit 8 is set (0x4007_CCBA).
        let mut adc = Adc::default();
        for n in 0..5 {
            adc.push(0, n);
        }
        let mut iterations = 0;
        loop {
            adc.read(DATA_ADDR);
            iterations += 1;
            if adc.read(STATUS_ADDR).unwrap() & STATUS_EMPTY != 0 {
                break;
            }
        }
        assert_eq!(iterations, 5);
    }
}
