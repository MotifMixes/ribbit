//! The pen sensor ("NearTouch"): a sequencer on the FS80A363 that takes a
//! drive state from a queue, measures the pen antenna's signal in that
//! state and puts a 16-bit count in a FIFO, raising INT0 for each one.
//!
//! The protocol is read from the kernel's INT0 handler (base ROM kernel
//! 0x0410-0x0D9C; docs/leappad.md, "Pen sensor"): EVIDENCE.
//!
//! * `FC17` (write): queue a drive code. `FC2A` bit 2: the queue has room
//!   (the kernel keeps at most two codes outstanding).
//! * `FC2A` bit 1: a result is waiting. `FC18` then `FC19`: its high and
//!   low bytes (reading FC19 takes it off the FIFO: HYPOTHESIS).
//! * `FC16` bit 4: written 0 then 1 before every new cycle (cart kernel
//!   0x59DE): taken here as a reset of the queue, FIFO and conversion
//!   (EVIDENCE: without it codes left from start-up fill the queue for
//!   good and the kernel never gets room for a second code).
//! * The kernel's cycle is codes 01 11 69 09 19 01: full drive, then a
//!   reference and a gradient for each axis, then full again. It subtracts
//!   an offset of 0x2000 from every count, takes a touch as both full
//!   readings above a threshold (1000-7000 chosen by an EEPROM byte) and
//!   computes each axis as gradient * 0x4000 / reference, a signed ratio
//!   it accepts within +-15000.
//!
//! What the counts are on real hardware (their scale and how they vary
//! across the page) is UNKNOWN: the readings here are the simplest ones the
//! kernel's arithmetic turns into the wanted ratios.

use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// The count the kernel treats as zero signal.
pub const OFFSET: u16 = 0x2000;
/// The signal returned for full drive and the references with the pen
/// down (above the highest touch threshold, 7000).
pub const SIGNAL: i32 = 0x3000;
/// Machine cycles per conversion: HYPOTHESIS (2 ms at the stand-in clock;
/// the patents give about 4 ms per state for the older design).
pub const CONVERSION_CYCLES: u32 = crate::MACHINE_HZ / 500;

/// Where the pen is, as the two signed axis ratios the kernel computes
/// (-1.0..1.0), or `None` when it is away from the page.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PenInput {
    pub position: Option<(f32, f32)>,
}

#[derive(Default, Serialize, Deserialize)]
pub struct PenSensor {
    pub input: PenInput,
    queue: VecDeque<u8>,
    fifo: VecDeque<u16>,
    /// Cycles left in the conversion at the head of the queue.
    busy: u32,
    /// Set when a conversion finished: the machine pulses INT0.
    pub irq: bool,
    /// Every drive code queued (for research tools; off unless set).
    #[serde(skip)]
    pub log: Option<Vec<u8>>,
}

impl PenSensor {
    /// The sequencer's state, for research logs.
    pub fn describe(&self) -> String {
        format!("queue {:02X?} fifo {:04X?} busy {}", self.queue, self.fifo, self.busy)
    }

    /// FC16 written: bit 4 low holds the sequencer in reset.
    pub fn control(&mut self, v: u8) {
        if v & 0x10 == 0 {
            self.queue.clear();
            self.fifo.clear();
            self.busy = 0;
        }
    }

    pub fn queue_code(&mut self, code: u8) {
        if let Some(l) = &mut self.log {
            l.push(code);
        }
        if self.queue.is_empty() {
            self.busy = CONVERSION_CYCLES;
        }
        self.queue.push_back(code);
    }

    /// FC2A bits 1 and 2.
    pub fn status(&self) -> u8 {
        (if self.fifo.is_empty() { 0 } else { 0x02 }) | (if self.queue.len() < 2 { 0x04 } else { 0 })
    }

    pub fn result_high(&self) -> u8 {
        (self.fifo.front().copied().unwrap_or(OFFSET) >> 8) as u8
    }

    pub fn result_low(&mut self) -> u8 {
        self.fifo.pop_front().unwrap_or(OFFSET) as u8
    }

    pub fn tick(&mut self, cycles: u32) {
        if self.queue.is_empty() {
            return;
        }
        if self.busy > cycles {
            self.busy -= cycles;
            return;
        }
        let code = self.queue.pop_front().unwrap_or(0);
        let v = self.reading(code);
        self.fifo.push_back(v);
        self.irq = true;
        self.busy = CONVERSION_CYCLES;
    }

    /// The count for drive code `code` with the pen where it is.
    fn reading(&self, code: u8) -> u16 {
        let Some((u, v)) = self.input.position else { return OFFSET };
        let s = match code {
            0x01 | 0x11 | 0x09 => SIGNAL,
            0x69 => (SIGNAL as f32 * v.clamp(-1.0, 1.0)) as i32,
            0x19 => (SIGNAL as f32 * u.clamp(-1.0, 1.0)) as i32,
            _ => 0,
        };
        (OFFSET as i32 + s) as u16
    }
}
