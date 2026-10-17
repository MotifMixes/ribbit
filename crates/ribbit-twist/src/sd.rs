//! The eMMC chip (and later the SD card) behind the SD/MMC hosts, as the
//! MultiMediaCard protocol defines it (JEDEC eMMC: CMD1 power-up, CID/CSD,
//! relative address, select, EXT_CSD, block reads and writes). The host's
//! side (registers, status bits) is in `machine.rs`.
//!
//! Storage is sparse: sectors never written read as the backing image, or
//! as zeros past its end, so a 4 GB chip costs only what is used.

use std::collections::{HashMap, VecDeque};

pub const SECTOR: usize = 512;

/// What a command answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Response {
    /// No card answered (time-out).
    None,
    /// A 32-bit response (R1, R3...).
    Short(u32),
    /// A 128-bit response (R2: CID or CSD), most significant word first.
    Long([u32; 4]),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Idle,
    Ready,
    Ident,
    Stby,
    Tran,
}

pub struct Card {
    /// Sectors as the chip holds them: the backing image, then writes.
    image: Vec<u8>,
    written: HashMap<u64, Box<[u8; SECTOR]>>,
    /// Size in sectors.
    pub sectors: u64,
    state: State,
    rca: u16,
    /// Data waiting to be read by the host, as 32-bit words.
    pub fifo: VecDeque<u32>,
    /// The next sector of an open-ended multiple-block read.
    reading: Option<u64>,
    /// A write in progress: the sector, the bytes so far, and whether more
    /// sectors follow (CMD25).
    writing: Option<(u64, Vec<u8>, bool)>,
    /// Sectors read and commands received (research log, kept only with
    /// `logging` on: it grows with every command).
    pub log: Vec<String>,
    pub logging: bool,
    /// The machine's time, set by the host before each command (for the log).
    pub now: u64,
}

/// A JEDEC-style CID: manufacturer 0x15, "RIBBIT" product name, serial 1.
const CID: [u32; 4] = [0x1501_0052, 0x4942_4249, 0x5410_0000, 0x0001_0001];

impl Card {
    /// An eMMC chip of `sectors` sectors holding `image` from sector 0.
    pub fn emmc(image: Vec<u8>, sectors: u64) -> Self {
        Self {
            image,
            written: HashMap::new(),
            sectors,
            state: State::Idle,
            rca: 0,
            fifo: VecDeque::new(),
            reading: None,
            writing: None,
            log: Vec::new(),
            logging: false,
            now: 0,
        }
    }

    pub fn sector(&self, n: u64) -> [u8; SECTOR] {
        if let Some(s) = self.written.get(&n) {
            return **s;
        }
        let mut out = [0; SECTOR];
        let at = n as usize * SECTOR;
        if at < self.image.len() {
            let end = (at + SECTOR).min(self.image.len());
            out[..end - at].copy_from_slice(&self.image[at..end]);
        }
        out
    }

    /// The chip's contents now: the image with every write applied (grown
    /// to the last sector written).
    pub fn contents(&self) -> Vec<u8> {
        let mut out = self.image.clone();
        for (&n, data) in &self.written {
            let at = n as usize * SECTOR;
            if out.len() < at + SECTOR {
                out.resize(at + SECTOR, 0);
            }
            out[at..at + SECTOR].copy_from_slice(&data[..]);
        }
        out
    }

    /// Every sector written since power-on (for saving what the console
    /// stored), sorted.
    pub fn written(&self) -> Vec<(u64, [u8; SECTOR])> {
        let mut v: Vec<_> = self.written.iter().map(|(&n, d)| (n, **d)).collect();
        v.sort_by_key(|e| e.0);
        v
    }

    pub fn write_sector(&mut self, n: u64, data: &[u8; SECTOR]) {
        if std::env::var("RIBBIT_DROP_WRITES").is_ok() {
            return;
        }
        self.written.insert(n, Box::new(*data));
    }

    fn queue(&mut self, bytes: &[u8]) {
        for w in bytes.chunks(4) {
            self.fifo.push_back(u32::from_le_bytes([w[0], w[1], w[2], w[3]]));
        }
    }

    /// The card status word (R1): current state in bits 12-9, ready for
    /// data (bit 8).
    fn status(&self) -> u32 {
        let state = match self.state {
            State::Idle => 0,
            State::Ready => 1,
            State::Ident => 2,
            State::Stby => 3,
            State::Tran => 4,
        };
        state << 9 | 1 << 8
    }

    /// The 512-byte EXT_CSD: sector count, eMMC 4.5 revision, bus widths.
    fn ext_csd(&self) -> [u8; SECTOR] {
        let mut e = [0u8; SECTOR];
        e[192] = 6; // EXT_CSD_REV: 4.5
        e[196] = 0x03; // CARD_TYPE: 26/52 MHz
        e[212..216].copy_from_slice(&(self.sectors as u32).to_le_bytes()); // SEC_COUNT
        e[226] = 0x08; // BOOT_SIZE_MULT
        e[504] = 0x01; // S_CMD_SET
        e
    }

    /// A CSD for a high-capacity chip (the size is in EXT_CSD).
    fn csd(&self) -> [u32; 4] {
        // CSD_STRUCTURE 3 (version in EXT_CSD), SPEC_VERS 4, TRAN_SPEED
        // 0x32 (26 MHz), READ_BL_LEN 9, C_SIZE 0xFFF (size in EXT_CSD).
        [0xD04F_0132, 0x0F59_03FF, 0xFFFF_FFFF, 0x8A40_4000]
    }

    /// Runs command `index` with `arg`.
    pub fn command(&mut self, index: u32, arg: u32) -> Response {
        if self.logging {
            self.log.push(format!("CMD{index} {arg:08X} ({:?}) t={}", self.state, self.now));
        }
        match index {
            0 => {
                self.state = State::Idle;
                self.fifo.clear();
                self.reading = None;
                Response::None
            }
            // SEND_OP_COND: ready at once, sector addressing, 2.7-3.6 V.
            1 => {
                if self.state == State::Idle || self.state == State::Ready {
                    self.state = State::Ready;
                }
                Response::Short(0xC0FF_8080)
            }
            2 if self.state == State::Ready => {
                self.state = State::Ident;
                Response::Long(CID)
            }
            3 => {
                self.rca = (arg >> 16) as u16;
                self.state = State::Stby;
                Response::Short(self.status())
            }
            // SWITCH (bus width, timing): accepted.
            6 => Response::Short(self.status()),
            7 => {
                self.state = if (arg >> 16) as u16 == self.rca { State::Tran } else { State::Stby };
                Response::Short(self.status())
            }
            // SEND_EXT_CSD (eMMC; SD cards answer CMD8 differently).
            8 if self.state == State::Tran => {
                let e = self.ext_csd();
                self.queue(&e);
                Response::Short(self.status())
            }
            9 => Response::Long(self.csd()),
            10 => Response::Long(CID),
            12 => {
                self.reading = None;
                self.writing = None;
                self.fifo.clear();
                Response::Short(self.status())
            }
            24 | 25 => {
                if self.logging {
                    self.log.push(format!("  write sector {arg}"));
                }
                self.writing = Some((arg as u64, Vec::with_capacity(SECTOR), index == 25));
                Response::Short(self.status())
            }
            13 => Response::Short(self.status()),
            16 => Response::Short(self.status()),
            17 | 18 => {
                let n = arg as u64;
                let s = self.sector(n);
                if self.logging {
                    self.log.push(format!("  read sector {n}"));
                }
                self.queue(&s);
                self.reading = (index == 18).then_some(n + 1);
                Response::Short(self.status())
            }
            // Anything else (SD-only commands such as CMD8 before select,
            // CMD55): no answer, as an eMMC chip gives none.
            _ => Response::None,
        }
    }

    /// A word written to the data port during a write; true when a sector
    /// is complete.
    pub fn data_in(&mut self, w: u32) -> bool {
        let Some((n, buf, more)) = &mut self.writing else {
            return false;
        };
        buf.extend_from_slice(&w.to_le_bytes());
        if buf.len() < SECTOR {
            return false;
        }
        let (sector, multi) = (*n, *more);
        let data: [u8; SECTOR] = buf[..SECTOR].try_into().unwrap();
        self.write_sector(sector, &data);
        self.writing = multi.then(|| (sector + 1, Vec::with_capacity(SECTOR), true));
        if multi {
            if self.logging {
                self.log.push(format!("  write sector {}", sector + 1));
            }
        }
        true
    }

    /// The host emptied the FIFO during a multiple-block read: the next
    /// sector follows.
    pub fn next_block(&mut self) {
        if let Some(n) = self.reading {
            let s = self.sector(n);
            if self.logging {
                self.log.push(format!("  read sector {n}"));
            }
            self.queue(&s);
            self.reading = Some(n + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_emmc_chip_powers_up_and_reads() {
        let mut image = vec![0u8; SECTOR * 2];
        image[SECTOR..SECTOR + 4].copy_from_slice(b"FAT!");
        let mut c = Card::emmc(image, 1 << 23);
        assert_eq!(c.command(0, 0), Response::None);
        assert_eq!(c.command(8, 0x1AA), Response::None, "not an SD card");
        assert_eq!(c.command(55, 0), Response::None);
        assert_eq!(c.command(1, 0x40FF_8000), Response::Short(0xC0FF_8080));
        assert!(matches!(c.command(2, 0), Response::Long(_)));
        c.command(3, 0x0001_0000);
        c.command(7, 0x0001_0000);
        assert_eq!(c.command(8, 0), Response::Short(4 << 9 | 1 << 8), "EXT_CSD once selected");
        let ext: Vec<u32> = c.fifo.drain(..).collect();
        assert_eq!(ext.len(), 128);
        assert_eq!(ext[212 / 4], 1 << 23, "sector count");
        c.command(17, 1);
        assert_eq!(c.fifo.front(), Some(&u32::from_le_bytes(*b"FAT!")));
        // Writes stay; past the image is zeros.
        c.write_sector(5, &[7; SECTOR]);
        assert_eq!(c.sector(5)[0], 7);
        assert_eq!(c.sector(100), [0; SECTOR]);
    }
}
