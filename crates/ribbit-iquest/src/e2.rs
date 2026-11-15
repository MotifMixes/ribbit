//! The chip's EEPROM transfer unit (FC10-FC15), which the Turbo Twist
//! Extreme's kernel uses for its "E2" (high scores, settings). The LeapPad's
//! kernels touch FC10 too but drive their EEPROM by hand (ribbit-leappad's
//! eeprom.rs).
//!
//! EVIDENCE (TTExtreme base 152-10696, kernel read routine at ROM 0x24E53 /
//! CPU CE53, write routine 0x24765 / C765):
//!
//! * `FC14:FC15`: the 16-bit EEPROM address (high byte first).
//! * `FC10` bit 2: sequential mode. 0 = one byte; 1 = a run (reads stream,
//!   writes fill an 8-byte page). Bits 0-1 are set at reset (`FC10 |= 3`),
//!   meaning UNKNOWN.
//! * Writing `FC11` writes that byte at the address (and moves on in
//!   sequential mode). Reading `FC11` starts a read; in sequential mode it
//!   returns the first byte.
//! * `FC13`: the byte read. In sequential mode each read of it takes the
//!   next byte.
//! * `FC00` bit 0: the transfer finished (cleared by writing 1); bit 6: a
//!   streamed byte is waiting; bit 7: busy (the page write loop waits for it
//!   to drop).
//!
//! Here every transfer finishes at once. The size is a HYPOTHESIS (8 KB, a
//! 24C64-type part: the addresses are 16 bits).

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 0x2000;

#[derive(Serialize, Deserialize)]
pub struct FastE2 {
    pub data: Vec<u8>,
    pub dirty: bool,
    addr: u16,
    mode: u8,
    out: u8,
}

impl Default for FastE2 {
    fn default() -> Self {
        FastE2 { data: vec![0xFF; SIZE], dirty: false, addr: 0, mode: 0, out: 0xFF }
    }
}

impl FastE2 {
    pub fn load(&mut self, data: &[u8]) {
        let n = data.len().min(SIZE);
        self.data[..n].copy_from_slice(&data[..n]);
    }

    fn sequential(&self) -> bool {
        self.mode & 4 != 0
    }

    fn at(&self) -> usize {
        self.addr as usize % SIZE
    }

    /// A register read; `status` is FC00. Returns None for registers that
    /// are not this unit's.
    pub fn read(&mut self, reg: u8, status: &mut u8) -> Option<u8> {
        Some(match reg {
            0x10 => self.mode,
            // In sequential mode the read gives the first byte itself (the
            // kernel keeps it: CEB8) and each FC13 read the next.
            0x11 => {
                let v = self.data[self.at()];
                self.out = v;
                if self.sequential() {
                    self.addr = self.addr.wrapping_add(1);
                    self.out = self.data[self.at()];
                    *status |= 0x40;
                }
                *status |= 0x01;
                v
            }
            0x13 => {
                let v = self.out;
                if self.sequential() {
                    self.addr = self.addr.wrapping_add(1);
                    self.out = self.data[self.at()];
                    *status |= 0x41;
                }
                v
            }
            0x14 => (self.addr >> 8) as u8,
            0x15 => self.addr as u8,
            _ => return None,
        })
    }

    pub fn write(&mut self, reg: u8, v: u8, status: &mut u8) -> bool {
        match reg {
            0x10 => self.mode = v,
            0x11 => {
                let at = self.at();
                if self.data[at] != v {
                    self.data[at] = v;
                    self.dirty = true;
                }
                if self.sequential() {
                    // A page write wraps inside its 8 bytes.
                    self.addr = (self.addr & !7) | (self.addr.wrapping_add(1) & 7);
                }
                *status |= 0x01;
            }
            0x14 => self.addr = (self.addr & 0xFF) | (v as u16) << 8,
            0x15 => self.addr = (self.addr & 0xFF00) | v as u16,
            _ => return false,
        }
        true
    }
}
