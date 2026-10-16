//! The system NAND flash (Micron MT29F2G08AACWP: pages of 0x800 bytes plus
//! 0x40 spare, ID 2C DA 80 15) and its controller at 0xD0900000, modelled
//! from the bootloader's driver (UNVERIFIED beyond what the driver does):
//!
//! | Offset | Use |
//! |---|---|
//! | +0x000 | DMA control: bit 0 start (never busy here), bit 1 memory to NAND |
//! | +0x004 / +0x008 | DMA memory address / length in bytes |
//! | +0x140 | Status: bit 15 ready |
//! | +0x144 | Command byte (FF reset, 90 ID, 00/30 read, 80/10 program, 60/D0 erase, 70 status) |
//! | +0x148 / +0x14C | Column / row (for an erase, row bits 15-0 / 31-16) |
//! | +0x150 | Data port, a byte per access |
//! | +0x154-0x164 | Chip select? and the ECC engine (reads 0: no errors) |

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A page with its spare bytes, and its data part.
pub const PAGE: usize = 0x840;
pub const PAGE_DATA: usize = 0x800;
const PAGES_PER_BLOCK: usize = 64;
const NAND_ID: [u8; 5] = [0x2C, 0xDA, 0x80, 0x15, 0x50];

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
enum Port {
    Idle,
    Id,
    Status,
    Page,
    Program,
}

/// The flash (kept out of save states: it is saved as its own file) and the
/// controller's state.
#[derive(Serialize, Deserialize)]
pub struct Nand {
    #[serde(skip)]
    pub flash: Vec<u8>,
    /// Set when the kernel programs or erases the flash.
    #[serde(skip)]
    pub dirty: bool,
    port: Port,
    column: u32,
    row: u32,
    /// The page register (read: loaded from flash; program: filled by writes).
    buf: Vec<u8>,
    cursor: usize,
    pub(crate) dma_addr: u32,
    pub(crate) dma_len: u32,
    regs: BTreeMap<u32, u32>,
    /// Research log of operations (a few thousand at most).
    #[serde(skip)]
    pub log: Vec<String>,
    #[serde(skip)]
    pub logging: bool,
}

impl Nand {
    pub fn new(flash: Vec<u8>) -> Self {
        Self {
            flash,
            dirty: false,
            port: Port::Idle,
            column: 0,
            row: 0,
            buf: vec![0xFF; PAGE],
            cursor: 0,
            dma_addr: 0,
            dma_len: 0,
            regs: BTreeMap::new(),
            log: Vec::new(),
            logging: false,
        }
    }

    pub(crate) fn note(&mut self, line: impl FnOnce() -> String) {
        if self.logging && self.log.len() < 3000 {
            self.log.push(line());
        }
    }

    fn load(&mut self) {
        let at = self.row as usize * PAGE;
        self.buf = self.flash.get(at..at + PAGE).map_or_else(|| vec![0xFF; PAGE], <[u8]>::to_vec);
        self.cursor = self.column as usize;
        self.port = Port::Page;
        let (row, column) = (self.row, self.column);
        self.note(|| format!("read page {row:#07x} col {column:#x}"));
    }

    fn command(&mut self, c: u32) {
        match c {
            0xFF => self.port = Port::Idle,
            0x90 => {
                self.port = Port::Id;
                self.cursor = 0;
            }
            0x70 => self.port = Port::Status,
            0x00 | 0x50 => self.port = Port::Idle,
            0x30 => self.load(),
            0x80 => {
                self.buf = vec![0xFF; PAGE];
                self.cursor = 0;
                self.port = Port::Program;
            }
            0x10 => {
                let at = self.row as usize * PAGE;
                for (i, b) in self.buf.iter().enumerate() {
                    if let Some(f) = self.flash.get_mut(at + i) {
                        *f &= b;
                    }
                }
                self.dirty = true;
                let row = self.row;
                self.note(|| format!("program page {row:#07x}"));
                self.port = Port::Idle;
            }
            0x60 => {}
            0xD0 => {
                let row = ((self.row << 16) | self.column) as usize;
                let start = row / PAGES_PER_BLOCK * PAGES_PER_BLOCK * PAGE;
                if let Some(b) = self.flash.get_mut(start..start + PAGES_PER_BLOCK * PAGE) {
                    b.fill(0xFF);
                }
                self.dirty = true;
                self.note(|| format!("erase block of row {row:#07x}"));
            }
            _ => self.note(|| format!("NAND command {c:#04x}?")),
        }
    }

    pub(crate) fn read_data(&mut self) -> u8 {
        match self.port {
            Port::Id => {
                let v = NAND_ID.get(self.cursor).copied().unwrap_or(0);
                self.cursor += 1;
                v
            }
            // Ready, not write protected, no failure.
            Port::Status => 0xE0,
            Port::Page => {
                let v = self.buf.get(self.cursor).copied().unwrap_or(0xFF);
                self.cursor += 1;
                v
            }
            _ => 0xFF,
        }
    }

    pub(crate) fn write_data(&mut self, v: u8) {
        if self.port == Port::Program {
            let at = self.column as usize + self.cursor;
            if let Some(b) = self.buf.get_mut(at) {
                *b = v;
            }
            self.cursor += 1;
        }
    }

    /// The controller's registers other than DMA start (which needs memory:
    /// see `Bus::nand_io`). Returns the value read.
    pub(crate) fn reg(&mut self, off: u32, write: Option<u32>) -> u32 {
        match (off, write) {
            (0x004, Some(v)) => self.dma_addr = v,
            (0x008, Some(v)) => self.dma_len = v,
            (0x140, None) => return 0x8000,
            (0x144, Some(v)) => self.command(v & 0xFF),
            (0x148, Some(v)) => self.column = v,
            (0x14C, Some(v)) => self.row = v,
            (0x150, None) => return self.read_data() as u32,
            (0x150, Some(v)) => self.write_data(v as u8),
            (o, Some(v)) => {
                self.regs.insert(o, v);
            }
            (0x158 | 0x15C, None) => return self.regs.get(&off).copied().unwrap_or(0),
            (_, None) => {}
        }
        0
    }
}
