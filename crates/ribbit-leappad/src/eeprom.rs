//! The serial EEPROM ("E2", "Slow EEPROM" in the kernel's log) on a
//! two-wire bus the kernel drives by hand through a port of the FS80A363:
//!
//! * `FC1F`: port data. Bit 2 is the clock (SCL), bit 3 the data line (SDA):
//!   the kernel's byte reader samples `FC1F & 08` with the clock high.
//! * `FC33`: port direction. Bit 3 set drives SDA low (the kernel writes a
//!   0 data bit as 08 and a 1 as 00, the latch staying 0); bit 2 drives SCL.
//!
//! EVIDENCE: cart kernel 0x46CB (byte write, MSB first) and 0x4E6B (byte
//! read). The bus and addressing are those of a 24C01/24C02-type I2C EEPROM
//! (control byte 1010xxxR, word address, sequential reads, page writes),
//! which the transactions logged in `log` confirm or not. The size is
//! 128 bytes: the kernel keeps a checksum at 0x7E-0x7F over 0x00-0x7D.

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 128;
/// Bytes written per page before the address wraps (24C01/02: 8).
const PAGE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum State {
    Idle,
    /// Receiving the control byte.
    Control,
    /// Receiving the word address.
    Address,
    /// Receiving data to write.
    Write,
    /// Sending data.
    Read,
}

#[derive(Serialize, Deserialize)]
pub struct Eeprom {
    pub data: Vec<u8>,
    /// The memory changed since the frontend last saved it.
    pub dirty: bool,
    state: State,
    addr: usize,
    /// Bits of the byte being shifted (in or out) and how many.
    shift: u8,
    bits: u8,
    /// The 9th clock (acknowledge) of the current byte.
    ack_phase: bool,
    /// The EEPROM is pulling SDA low.
    sda_out_low: bool,
    scl: bool,
    sda: bool,
    /// The master did not acknowledge the last byte read (end of read).
    nack: bool,
    /// Transactions (research): "S", "P", bytes in/out.
    #[serde(skip)]
    pub log: Option<Vec<String>>,
}

impl Default for Eeprom {
    fn default() -> Self {
        Self::new(vec![0xFF; SIZE])
    }
}

impl Eeprom {
    pub fn new(mut data: Vec<u8>) -> Self {
        data.resize(SIZE, 0xFF);
        Eeprom {
            data,
            dirty: false,
            state: State::Idle,
            addr: 0,
            shift: 0,
            bits: 0,
            ack_phase: false,
            sda_out_low: false,
            scl: true,
            sda: true,
            nack: false,
            log: None,
        }
    }

    fn note(&mut self, s: String) {
        if let Some(l) = &mut self.log {
            l.push(s);
        }
    }

    /// The level the EEPROM leaves on SDA (false = pulling it low).
    pub fn sda_released(&self) -> bool {
        !self.sda_out_low
    }

    /// The bus lines as the master drives them (true = released/high); the
    /// line levels are these ANDed with the EEPROM's own SDA.
    pub fn lines(&mut self, scl: bool, sda_master: bool) {
        let sda = sda_master && !self.sda_out_low;
        if scl && self.scl && sda != self.sda {
            // SDA changed with the clock high: start or stop.
            if sda {
                self.note("P".into());
                self.state = State::Idle;
            } else {
                self.note("S".into());
                self.state = State::Control;
                self.bits = 0;
                self.shift = 0;
                self.ack_phase = false;
            }
            self.sda_out_low = false;
            self.sda = sda;
            self.scl = scl;
            return;
        }
        if scl && !self.scl {
            self.rising(sda);
        } else if !scl && self.scl {
            self.falling();
        }
        self.sda = sda;
        self.scl = scl;
    }

    fn rising(&mut self, sda: bool) {
        if self.state == State::Idle {
            return;
        }
        if self.ack_phase {
            if self.state == State::Read {
                self.nack = sda;
            }
            return;
        }
        if self.state != State::Read {
            self.shift = (self.shift << 1) | sda as u8;
        }
        self.bits += 1;
    }

    fn falling(&mut self) {
        if self.state == State::Idle {
            self.sda_out_low = false;
            return;
        }
        if self.ack_phase {
            // End of the acknowledge clock.
            self.ack_phase = false;
            self.sda_out_low = false;
            self.bits = 0;
            if self.state == State::Read {
                if self.nack {
                    self.state = State::Idle;
                } else {
                    self.load_read_byte();
                }
            }
            return;
        }
        if self.state == State::Read {
            if self.bits >= 8 {
                // The master acknowledges (or not) next.
                self.ack_phase = true;
                self.sda_out_low = false;
            } else {
                self.sda_out_low = (self.shift >> (7 - self.bits)) & 1 == 0;
            }
            return;
        }
        if self.bits < 8 {
            return;
        }
        let byte = self.shift;
        self.shift = 0;
        self.ack_phase = true;
        self.sda_out_low = true; // acknowledge
        match self.state {
            State::Control => {
                self.note(format!("ctl {byte:02X}"));
                if byte & 0xF0 == 0xA0 {
                    if byte & 1 == 1 {
                        self.state = State::Read;
                        self.after_ack_read();
                    } else {
                        self.state = State::Address;
                    }
                } else {
                    // X24C01-style: the first byte is (address << 1) | R/W.
                    self.addr = (byte >> 1) as usize % SIZE;
                    if byte & 1 == 1 {
                        self.state = State::Read;
                        self.after_ack_read();
                    } else {
                        self.state = State::Write;
                    }
                }
            }
            State::Address => {
                self.note(format!("addr {byte:02X}"));
                self.addr = byte as usize % SIZE;
                self.state = State::Write;
            }
            State::Write => {
                self.note(format!("w[{:02X}]={byte:02X}", self.addr));
                self.data[self.addr] = byte;
                self.dirty = true;
                let page = self.addr & !(PAGE - 1);
                self.addr = page | ((self.addr + 1) & (PAGE - 1));
            }
            _ => {}
        }
    }

    /// After the acknowledge of a read command, the first data byte goes out.
    fn after_ack_read(&mut self) {
        self.nack = false;
    }

    fn load_read_byte(&mut self) {
        let b = self.data[self.addr];
        self.note(format!("r[{:02X}]={b:02X}", self.addr));
        self.addr = (self.addr + 1) % SIZE;
        self.shift = b;
        self.bits = 0;
        // The first bit goes out now, while the clock is low.
        self.sda_out_low = b & 0x80 == 0;
    }
}
