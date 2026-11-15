//! The iQuest's "secondary (local) flash" at chip select 4 (physical
//! 0x1000000), where it keeps the user's name, settings and notes.
//!
//! The kernel identifies the part from a table in the ROM (iQuest base
//! 152-10663: the word at ROM Info Block +7Ch points to it; four access
//! methods, each with a list of manufacturer/device IDs: Intel-style 0x90
//! at address 0, then JEDEC unlock sequences at 5555/2AAA (byte mode) and
//! AAAA/5555 (word mode); docs/iquest.md, "Flash"). Which part a real iQuest
//! has is UNKNOWN; Ribbit fits one from the byte-mode list, the SST
//! 39SF040 (ID BF D7, 512 KB, 4 KB sectors), which the kernel accepts.

use serde::{Deserialize, Serialize};

pub const SIZE: usize = 0x8_0000;
const SECTOR: usize = 0x1000;
pub const MANUFACTURER: u8 = 0xBF;
pub const DEVICE: u8 = 0xD7;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Mode {
    Read,
    /// Unlock cycles seen (1 or 2) and whether an erase was set up.
    Unlock(u8, bool),
    Id,
    Program,
}

#[derive(Serialize, Deserialize)]
pub struct Flash {
    #[serde(skip)]
    pub data: Vec<u8>,
    #[serde(skip)]
    pub dirty: bool,
    mode: Mode,
}

impl Default for Flash {
    fn default() -> Self {
        Flash { data: vec![0xFF; SIZE], dirty: false, mode: Mode::Read }
    }
}

impl Flash {
    pub fn load(&mut self, data: &[u8]) {
        let n = data.len().min(SIZE);
        self.data[..n].copy_from_slice(&data[..n]);
    }

    pub fn read(&self, off: u32) -> u8 {
        match self.mode {
            Mode::Id => match off & 1 {
                0 => MANUFACTURER,
                _ => DEVICE,
            },
            _ => self.data[off as usize % SIZE],
        }
    }

    pub fn write(&mut self, off: u32, v: u8) {
        let a = off & 0x7FFF;
        self.mode = match (self.mode, a, v) {
            (_, _, 0xF0) => Mode::Read,
            (Mode::Program, _, _) => {
                let at = off as usize % SIZE;
                let n = self.data[at] & v;
                self.dirty |= n != self.data[at];
                self.data[at] = n;
                Mode::Read
            }
            (Mode::Read | Mode::Id, 0x5555, 0xAA) => Mode::Unlock(1, false),
            (Mode::Unlock(1, e), 0x2AAA, 0x55) => Mode::Unlock(2, e),
            (Mode::Unlock(2, false), 0x5555, 0x90) => Mode::Id,
            (Mode::Unlock(2, false), 0x5555, 0xA0) => Mode::Program,
            (Mode::Unlock(2, false), 0x5555, 0x80) => Mode::Unlock(0, true),
            (Mode::Unlock(0, true), 0x5555, 0xAA) => Mode::Unlock(1, true),
            (Mode::Unlock(2, true), 0x5555, 0x10) => {
                self.data.fill(0xFF);
                self.dirty = true;
                Mode::Read
            }
            (Mode::Unlock(2, true), _, 0x30 | 0x50) => {
                let s = off as usize % SIZE / SECTOR * SECTOR;
                self.data[s..s + SECTOR].fill(0xFF);
                self.dirty = true;
                Mode::Read
            }
            (Mode::Id, _, _) => Mode::Id,
            _ => Mode::Read,
        };
    }
}
