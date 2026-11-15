//! The 24C08 on the ClickStart's board: a 1 KB I2C EEPROM that keeps the
//! players. The system ROM bit-bangs it on port C (bit 0 SDA, bit 1 SCL;
//! routines at 0x3f0a01-0x3f0c55), not through the SPG's SIO or I2C units.
//!
//! The chip: a device byte 1010 A2 P1 P0 R/W (P1 P0 the 256-byte block,
//! A2 its pin, taken as tied low), then for a write the word address and up
//! to 16 bytes, kept in a page buffer and written at STOP (wrapping within
//! the 16-byte page); a read goes on from the current address. Writes take
//! no time here (the real chip is busy for up to 5 ms and NACKs meanwhile;
//! the BIOS polls for the ACK).

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 0x400;
const PAGE: usize = 16;

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
enum State {
    /// Waiting for a START.
    Idle,
    Device,
    Address,
    Write,
    /// A read's device byte, being acknowledged.
    ReadAck,
    Read,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Eeprom {
    /// The contents: kept out of save states (the console's saves stay as
    /// they are when a state is loaded).
    #[serde(skip, default = "blank")]
    pub data: Vec<u8>,
    /// Written since the last `saved`.
    #[serde(skip)]
    pub dirty: bool,
    scl: bool,
    sda: bool,
    state: State,
    /// Bits clocked in or out of `byte`; 8 then 9 around the ACK.
    bit: u8,
    byte: u8,
    /// Whether the chip leaves SDA alone (true) or pulls it low.
    out: bool,
    /// The master acknowledged the last byte read.
    acked: bool,
    addr: u16,
    pending: Vec<(u16, u8)>,
}

/// A blank chip. UNVERIFIED: 0xFF as erased EEPROM cells usually read; no
/// new ClickStart's chip has been read.
fn blank() -> Vec<u8> {
    vec![0xff; SIZE]
}

impl Eeprom {
    pub fn new() -> Self {
        Eeprom {
            data: blank(),
            dirty: false,
            scl: true,
            sda: true,
            state: State::Idle,
            bit: 0,
            byte: 0,
            out: true,
            acked: false,
            addr: 0,
            pending: Vec::new(),
        }
    }

    /// The SDA line as the master sees it: its own level ANDed with the chip's.
    pub fn sda(&self, master: bool) -> bool {
        master && self.out
    }

    /// The master's SCL and SDA levels (released lines read high).
    pub fn update(&mut self, scl: bool, sda: bool) {
        if scl && self.scl && sda != self.sda {
            if sda {
                self.stop();
            } else {
                // START (or a repeated one): an unfinished write is dropped.
                self.pending.clear();
                self.state = State::Device;
                self.bit = 0;
                self.out = true;
            }
        } else if scl && !self.scl {
            self.rise(sda);
        } else if !scl && self.scl {
            self.fall();
        }
        self.scl = scl;
        self.sda = sda;
    }

    fn stop(&mut self) {
        for &(a, v) in &self.pending {
            self.data[a as usize] = v;
            self.dirty = true;
        }
        self.pending.clear();
        self.state = State::Idle;
        self.out = true;
    }

    fn rise(&mut self, sda: bool) {
        match self.state {
            State::Idle => {}
            State::Read => {
                if self.bit == 9 {
                    self.acked = !sda;
                }
            }
            _ => {
                if self.bit < 8 {
                    self.byte = self.byte << 1 | sda as u8;
                    self.bit += 1;
                }
            }
        }
    }

    fn fall(&mut self) {
        match self.state {
            State::Idle => {}
            State::Read => match self.bit {
                0..=7 => {
                    self.out = self.byte >> (7 - self.bit) & 1 != 0;
                    self.bit += 1;
                }
                8 => {
                    // Released for the master's ACK.
                    self.out = true;
                    self.bit = 9;
                }
                _ => {
                    if self.acked {
                        self.addr = (self.addr + 1) % SIZE as u16;
                        self.load();
                    } else {
                        self.state = State::Idle;
                        self.out = true;
                    }
                }
            },
            _ => match self.bit {
                8 => {
                    self.out = !self.receive();
                    self.bit = 9;
                }
                9 => {
                    self.out = true;
                    self.bit = 0;
                    if self.state == State::ReadAck {
                        self.state = State::Read;
                        self.load();
                    }
                }
                _ => {}
            },
        }
    }

    /// Takes a received byte; returns whether it is acknowledged.
    fn receive(&mut self) -> bool {
        let b = self.byte;
        match self.state {
            State::Device => {
                if b & 0xf8 != 0xa0 {
                    self.state = State::Idle;
                    return false;
                }
                self.addr = (self.addr & 0xff) | ((b as u16 >> 1) & 3) << 8;
                self.state = if b & 1 != 0 { State::ReadAck } else { State::Address };
            }
            State::Address => {
                self.addr = (self.addr & 0x300) | b as u16;
                self.state = State::Write;
            }
            State::Write => {
                if let Some(p) = self.pending.iter_mut().find(|p| p.0 == self.addr) {
                    p.1 = b;
                } else if self.pending.len() < PAGE {
                    self.pending.push((self.addr, b));
                }
                let page = self.addr & !(PAGE as u16 - 1);
                self.addr = page | (self.addr + 1) & (PAGE as u16 - 1);
            }
            _ => {}
        }
        true
    }

    /// Starts sending the byte at the current address: its top bit now.
    fn load(&mut self) {
        self.byte = self.data[self.addr as usize];
        self.out = self.byte & 0x80 != 0;
        self.bit = 1;
    }
}
