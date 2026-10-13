//! ADC sample FIFO (touchscreen/battery converter) at 0x0180_00A0/00A8.
//!
//! Evidence:
//! * MAME `leapster.cpp`: 0x0180_00A0 pops the FIFO (`channel << 16 |
//!   data << 5`); bit 8 of 0x0180_00A8 is set when the FIFO is empty.
//! * Both BIOSes (v1.5 at 0x4007_CC6A, v2.1 at 0x4008_EB78): clear the ADC
//!   bit in the interrupt enable register, acknowledge it in the flag
//!   register, zero all four channel controls (0x0180_0090-009C), then read
//!   0x0180_00A0 / 0x0180_00A8 until bit 8 is set, i.e. drain the FIFO.
//!
//! Only the FIFO is modelled. By default no sample source exists:
//! conversions, channel controls (0x0180_0090-009C) and 0x0180_00A4 are
//! unmodelled and remain visible in the unmapped-access report, so a BIOS
//! waiting for samples is seen waiting rather than fed invented data. The
//! opt-in EXPERIMENTAL touch model (touch.rs) is the only thing that pushes.

use std::collections::VecDeque;

pub const DATA_ADDR: u32 = 0x0180_00A0;
pub const STATUS_ADDR: u32 = 0x0180_00A8;
pub const STATUS_EMPTY: u32 = 1 << 8;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Adc {
    /// Packed samples, oldest first (MAME's format: channel << 16 | data << 5).
    pub fifo: VecDeque<u32>,
    /// Reads of the data register while the FIFO was empty.
    pub empty_reads: u64,
}

impl Adc {
    /// Device side: queue a converted sample.
    pub fn push(&mut self, channel: u32, data: u32) {
        self.fifo.push_back((channel << 16) | (data << 5));
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
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_fifo_reports_empty() {
        let mut adc = Adc::default();
        assert_eq!(adc.read(STATUS_ADDR), Some(STATUS_EMPTY));
        assert_eq!(adc.read(DATA_ADDR), Some(0));
        assert_eq!(adc.empty_reads, 1);
    }

    #[test]
    fn reads_pop_in_order_until_empty() {
        let mut adc = Adc::default();
        adc.push(0, 0x3DF);
        adc.push(3, 0x7FF);
        assert_eq!(adc.read(STATUS_ADDR), Some(0));
        assert_eq!(adc.read(DATA_ADDR), Some(0x3DF << 5));
        assert_eq!(adc.read(DATA_ADDR), Some((3 << 16) | (0x7FF << 5)));
        assert_eq!(adc.read(STATUS_ADDR), Some(STATUS_EMPTY));
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
        assert!(adc.read(0x0180_0090).is_none(), "channel controls are not modelled");
    }
}
