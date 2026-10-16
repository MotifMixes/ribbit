//! Leapster 2 CIP3: the NAND flash at 0xE000_0000.
//!
//! VERIFIED-FW (BIOS "Leapster 2x CIP3 Baserom m9", `152-12659`; driver
//! 0x400D_BE90-0x400D_C4F0, file system strings "FFS_Flash_Format",
//! "eb %d, page %d"). The earlier Leapster 2 BIOSes (0.6, Connected B5) have no
//! NAND driver; they have an SD card driver instead.
//!
//! Interface: one 32-bit port at 0xE000_0000. A write with bit 24 set latches
//! a command byte, bit 25 an address byte, neither a data byte (the driver's
//! bus width is 8: byte loops 0x400D_C0F4 / 0x400D_C32E). A read returns the
//! next data byte. Commands used by the driver:
//!
//! | Command | Use |
//! |---|---|
//! | 0x90, address 0x00 | read ID: 4 bytes read, the first two checked (0x400D_BF98): maker 0x98 and device 0xD1, or maker 0x20 and device 0xF1 |
//! | 0x00, 4 addresses, 0x30 | read a page (column low, column high (4 bits), row low, row high) |
//! | 0x80, 4 addresses, data, 0x10 | program a page |
//! | 0x60, 2 addresses, 0xD0 | erase a block |
//! | 0x70 | status: bit 0 = 1 on failure |
//! | 0xFF | reset |
//!
//! Geometry the driver sets for either ID (0x400D_C002): 2048-byte pages plus
//! 64 spare bytes, 64 pages per block, 1024 blocks (1 Gbit). After a program,
//! erase or reset it waits on 0x0180_0018 (writes 0x84, polls bit 7 clear) and
//! then on 0x0180_00EC bit 1 (the chip's ready/busy line, 1 = ready).
//!
//! Model: an ST-style 1 Gbit part, ID 0x20 0xF1 (UNVERIFIED: which chip the
//! CIP3 uses is unknown; the driver accepts both IDs; the 3rd and 4th ID bytes,
//! 0x00 0x1D here, are not checked). Every operation completes at once (the
//! busy time is not modelled: ready/busy always reads ready). A new part
//! is fully erased (0xFF) with no factory bad blocks. Programming ANDs data
//! into the page, as on NAND. Pages are stored only once written.

use std::collections::BTreeMap;

pub const BASE: u32 = 0xE000_0000;
pub const PAGE_DATA: usize = 2048;
pub const PAGE_SPARE: usize = 64;
pub const PAGE_SIZE: usize = PAGE_DATA + PAGE_SPARE;
pub const PAGES_PER_BLOCK: u32 = 64;
pub const BLOCKS: u32 = 1024;
pub const PAGES: u32 = PAGES_PER_BLOCK * BLOCKS;
pub const ID: [u8; 4] = [0x20, 0xF1, 0x00, 0x1D];

const LATCH_COMMAND: u32 = 1 << 24;
const LATCH_ADDRESS: u32 = 1 << 25;

pub fn contains(addr: u32) -> bool {
    addr & !3 == BASE
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
enum Mode {
    #[default]
    Idle,
    ReadId,
    ReadSetup,
    Read,
    Program,
    Erase,
    Status,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Nand {
    /// Written pages (row -> 2112 bytes); missing pages are erased (0xFF).
    pages: BTreeMap<u32, Vec<u8>>,
    mode: Mode,
    address: Vec<u8>,
    /// Page register (read result or program data) and the column within it.
    buffer: Vec<u8>,
    column: usize,
    /// Status bit 0 after the last program/erase (always 0: no failures).
    failed: bool,
    /// Accesses outside the command set above, for research.
    pub unknown_commands: u64,
    /// Set when a page was programmed or a block erased (for saving; not saved).
    #[serde(skip)]
    pub dirty: bool,
}

impl Nand {
    fn page(&self, row: u32) -> Vec<u8> {
        self.pages.get(&row).cloned().unwrap_or_else(|| vec![0xFF; PAGE_SIZE])
    }

    fn column_row(&self) -> (usize, u32) {
        let a = |i: usize| self.address.get(i).copied().unwrap_or(0) as u32;
        let column = (a(0) | (a(1) & 0x0F) << 8) as usize;
        (column.min(PAGE_SIZE), (a(2) | a(3) << 8) % PAGES)
    }

    fn command(&mut self, c: u8) {
        match c {
            0xFF => {
                self.mode = Mode::Idle;
                self.address.clear();
            }
            0x90 => {
                self.mode = Mode::ReadId;
                self.column = 0;
            }
            0x00 => {
                self.mode = Mode::ReadSetup;
                self.address.clear();
            }
            0x30 if self.mode == Mode::ReadSetup => {
                let (column, row) = self.column_row();
                self.buffer = self.page(row);
                self.column = column;
                self.mode = Mode::Read;
            }
            0x80 => {
                self.mode = Mode::Program;
                self.address.clear();
                self.buffer = vec![0xFF; PAGE_SIZE];
                self.column = 0;
            }
            0x10 if self.mode == Mode::Program => {
                let (_, row) = self.column_row();
                let mut page = self.page(row);
                for (p, b) in page.iter_mut().zip(&self.buffer) {
                    *p &= *b;
                }
                if page.iter().all(|&b| b == 0xFF) {
                    self.pages.remove(&row);
                } else {
                    self.pages.insert(row, page);
                }
                self.failed = false;
                self.dirty = true;
                self.mode = Mode::Idle;
            }
            0x60 => {
                self.mode = Mode::Erase;
                self.address.clear();
            }
            0xD0 if self.mode == Mode::Erase => {
                let a = |i: usize| self.address.get(i).copied().unwrap_or(0) as u32;
                let first = ((a(0) | a(1) << 8) % PAGES) & !(PAGES_PER_BLOCK - 1);
                let keys: Vec<u32> = self.pages.range(first..first + PAGES_PER_BLOCK).map(|(&k, _)| k).collect();
                for k in keys {
                    self.pages.remove(&k);
                }
                self.failed = false;
                self.dirty = true;
                self.mode = Mode::Idle;
            }
            0x70 => self.mode = Mode::Status,
            _ => {
                self.unknown_commands += 1;
                self.mode = Mode::Idle;
            }
        }
    }

    pub fn write(&mut self, value: u32) {
        let byte = value as u8;
        if value & LATCH_COMMAND != 0 {
            self.command(byte);
        } else if value & LATCH_ADDRESS != 0 {
            self.address.push(byte);
            if self.mode == Mode::Program && self.address.len() == 2 {
                self.column = self.column_row().0;
            }
        } else if self.mode == Mode::Program {
            if let Some(b) = self.buffer.get_mut(self.column) {
                *b = byte;
            }
            self.column += 1;
        }
    }

    pub fn read(&mut self) -> u32 {
        match self.mode {
            Mode::ReadId => {
                let b = ID.get(self.column).copied().unwrap_or(0);
                self.column += 1;
                b as u32
            }
            Mode::Read => {
                let b = self.buffer.get(self.column).copied().unwrap_or(0xFF);
                self.column += 1;
                b as u32
            }
            // Ready (bit 6), not write-protected (bit 7), bit 0 = failure.
            Mode::Status => 0xC0 | self.failed as u32,
            _ => 0xFF,
        }
    }

    /// The programmed pages as a save file (sparse.rs).
    pub fn image(&self) -> Vec<u8> {
        super::sparse::encode(&self.pages, PAGE_SIZE)
    }

    /// Replaces the contents with a save file from `image`; false (unchanged)
    /// if the file is not one.
    pub fn load_image(&mut self, bytes: &[u8]) -> bool {
        match super::sparse::decode(bytes, PAGE_SIZE) {
            Some(mut p) => {
                p.retain(|&n, _| n < PAGES);
                self.pages = p;
                true
            }
            None => false,
        }
    }

    /// Pages that hold data (for tests and research).
    pub fn written_pages(&self) -> usize {
        self.pages.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(n: &mut Nand, c: u8) {
        n.write(LATCH_COMMAND | c as u32);
    }

    fn addr(n: &mut Nand, bytes: &[u8]) {
        for &b in bytes {
            n.write(LATCH_ADDRESS | b as u32);
        }
    }

    fn read_page(n: &mut Nand, column: u16, row: u16, len: usize) -> Vec<u8> {
        cmd(n, 0x00);
        addr(n, &[column as u8, (column >> 8) as u8, row as u8, (row >> 8) as u8]);
        cmd(n, 0x30);
        (0..len).map(|_| n.read() as u8).collect()
    }

    #[test]
    fn id_matches_what_the_driver_accepts() {
        let mut n = Nand::default();
        cmd(&mut n, 0x90);
        addr(&mut n, &[0]);
        let id: Vec<u32> = (0..4).map(|_| n.read()).collect();
        assert!(matches!((id[0], id[1]), (0x98, 0xD1) | (0x20, 0xF1)));
    }

    #[test]
    fn program_read_and_erase() {
        let mut n = Nand::default();
        assert_eq!(read_page(&mut n, 0, 70, 4), [0xFF; 4], "new part is erased");
        cmd(&mut n, 0x80);
        addr(&mut n, &[0x00, 0x08, 70, 0]); // column 0x800: the spare area
        for b in [1u8, 2, 3] {
            n.write(b as u32);
        }
        cmd(&mut n, 0x10);
        cmd(&mut n, 0x70);
        assert_eq!(n.read() & 1, 0, "status: no failure");
        assert_eq!(read_page(&mut n, 0x800, 70, 4), [1, 2, 3, 0xFF]);
        assert_eq!(read_page(&mut n, 0, 70, 1), [0xFF], "data area untouched");
        // Programming only clears bits.
        cmd(&mut n, 0x80);
        addr(&mut n, &[0x00, 0x08, 70, 0]);
        n.write(0xFE);
        cmd(&mut n, 0x10);
        assert_eq!(read_page(&mut n, 0x800, 70, 1), [0]);
        // Erasing block 1 (pages 64-127) clears page 70.
        cmd(&mut n, 0x60);
        addr(&mut n, &[64, 0]);
        cmd(&mut n, 0xD0);
        assert_eq!(read_page(&mut n, 0x800, 70, 1), [0xFF]);
        assert_eq!(n.written_pages(), 0);
    }
}
