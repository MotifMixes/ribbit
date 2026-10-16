//! The NAND flash chips and their controller at 0xD0900000, modelled from the
//! bootloader's driver (UNVERIFIED beyond what the driver does):
//!
//! | Offset | Use |
//! |---|---|
//! | +0x000 | DMA control: bit 0 start (never busy here), bit 1 memory to NAND |
//! | +0x004 / +0x008 | DMA memory address / length in bytes |
//! | +0x140 | Status: bit 15 ready |
//! | +0x144 | Command byte (FF reset, 90 ID, 00/01/50/30 read, 80/10 program, 60/D0 erase, 70 status) |
//! | +0x148 / +0x14C | Column / row (for an erase, row bits 15-0 / 31-16) |
//! | +0x150 | Data port, a byte per access |
//! | +0x154-0x164 | Saved around Read ID, and the ECC engine (reads 0: no errors) |
//!
//! Two chips share the controller; GPIO register 0xC0000108 bits 5-4 choose
//! one. The bootloader and kernel set them to 1 and 2 to look for a
//! cartridge, then back to 0; once the kernel has found a SanDisk chip on 1
//! it reads it with 3 (both bits: why is UNKNOWN; perhaps bit 4 is the
//! cartridge's chip enable and bit 5 something else).
//!
//! - 0: the system NAND, Micron MT29F2G08AACWP (pages of 0x800 bytes plus
//!   0x40 spare, ID 2C DA 80 15).
//! - 1 and 3: the cartridge's, a 64 MB small-page chip (pages of 0x200
//!   bytes plus 0x10 spare, 32 to a block; dumps are 0x420_0000 bytes). The
//!   kernel's chip check (0x31C14) wants an ID starting 45 76 (SanDisk, 64
//!   MB) and treats the chip as a cartridge only then; the bytes after are
//!   UNKNOWN (not checked). The controller sends +0x148 as two address
//!   bytes and +0x14C after them, so on a small-page chip (one column byte)
//!   +0x148 bits 15-8 are the page number's low byte: the kernel reads
//!   sector n with +0x148 = n << 8, +0x14C = 0, then DMAs 0x200 bytes.
//!   Small-page chips take reads as 0x00
//!   (bytes 0-255), 0x01 (256-511) or 0x50 (the spare bytes) and start on
//!   the address, without the 0x30 of large-page chips.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A system NAND page with its spare bytes, and its data part.
pub const PAGE: usize = 0x840;
pub const PAGE_DATA: usize = 0x800;
pub const PAGES_PER_BLOCK: usize = 64;
const NAND_ID: [u8; 5] = [0x2C, 0xDA, 0x80, 0x15, 0x50];

/// A cartridge page with its spare bytes, and the cartridge's size.
pub const CART_PAGE: usize = 0x210;
pub const CART_SIZE: usize = 0x420_0000;
const CART_PAGES_PER_BLOCK: usize = 32;
const CART_ID: [u8; 5] = [0x45, 0x76, 0xFF, 0xFF, 0xFF];
/// Whether the cartridge answers on a chip select value (0xC0000108 bits
/// 5-4): 1 (where it is found) and 3 (what the kernel reads it with).
fn cart_selected(select: u32) -> bool {
    select & 1 != 0
}

#[derive(PartialEq, Clone, Copy, Serialize, Deserialize)]
enum Port {
    Idle,
    Id,
    Status,
    Page,
    Program,
}

/// The flashes (kept out of save states: they are saved as their own
/// files) and the controller's state.
#[derive(Serialize, Deserialize)]
pub struct Nand {
    /// The system NAND.
    #[serde(skip)]
    pub flash: Vec<u8>,
    /// The cartridge's NAND (empty: no cartridge).
    #[serde(skip)]
    pub cart: Vec<u8>,
    /// Set when the kernel programs or erases the system NAND.
    #[serde(skip)]
    pub dirty: bool,
    /// The system NAND blocks programmed or erased since power-on.
    #[serde(skip)]
    pub touched: std::collections::BTreeSet<usize>,
    /// The chip selected (0xC0000108 bits 5-4).
    #[serde(default)]
    pub(crate) select: u32,
    port: Port,
    column: u32,
    row: u32,
    /// The column a small-page read starts at (0x00: 0, 0x01: 0x100, 0x50:
    /// the spare bytes at 0x200).
    #[serde(default)]
    half: u32,
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

/// A chip's layout.
#[derive(Clone, Copy)]
struct Chip {
    page: usize,
    pages_per_block: usize,
    id: [u8; 5],
    small: bool,
}

const SYSTEM_CHIP: Chip = Chip { page: PAGE, pages_per_block: PAGES_PER_BLOCK, id: NAND_ID, small: false };
const CART_CHIP: Chip = Chip { page: CART_PAGE, pages_per_block: CART_PAGES_PER_BLOCK, id: CART_ID, small: true };

impl Nand {
    pub fn new(flash: Vec<u8>) -> Self {
        Self {
            flash,
            cart: Vec::new(),
            dirty: false,
            touched: Default::default(),
            select: 0,
            port: Port::Idle,
            column: 0,
            row: 0,
            half: 0,
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
        // The system NAND fills the first 3000; cartridge events get room after.
        if self.logging && (self.log.len() < 3000 || self.select != 0 && self.log.len() < 9000) {
            self.log.push(line());
        }
    }

    /// The selected chip's layout, or None when no chip answers.
    fn chip(&self) -> Option<Chip> {
        match self.select {
            0 => Some(SYSTEM_CHIP),
            s if cart_selected(s) && !self.cart.is_empty() => Some(CART_CHIP),
            _ => None,
        }
    }

    /// The page and byte the address registers point at on a chip.
    fn address(&self, chip: Chip) -> (usize, usize) {
        if chip.small {
            (((self.column >> 8) | (self.row << 8)) as usize, (self.column & 0xFF) as usize)
        } else {
            (self.row as usize, self.column as usize)
        }
    }

    fn flash_mut(&mut self) -> &mut Vec<u8> {
        if self.select == 0 { &mut self.flash } else { &mut self.cart }
    }

    fn load(&mut self) {
        let Some(chip) = self.chip() else {
            self.port = Port::Idle;
            return;
        };
        let (page, column) = self.address(chip);
        let at = page * chip.page;
        let flash = if self.select == 0 { &self.flash } else { &self.cart };
        self.buf = flash.get(at..at + chip.page).map_or_else(|| vec![0xFF; chip.page], <[u8]>::to_vec);
        self.cursor = column + self.half as usize;
        self.port = Port::Page;
        let (sel, column) = (self.select, self.cursor);
        self.note(|| format!("chip {sel} read page {page:#07x} col {column:#x}"));
    }

    fn command(&mut self, c: u32) {
        let small = self.chip().is_some_and(|c| c.small);
        match c {
            0xFF => self.port = Port::Idle,
            0x90 => {
                self.port = Port::Id;
                self.cursor = 0;
            }
            0x70 => self.port = Port::Status,
            // Small-page reads start on the address (see `reg`).
            0x00 | 0x01 | 0x50 if small => {
                self.half = match c {
                    0x01 => 0x100,
                    0x50 => 0x200,
                    _ => 0,
                };
                self.port = Port::Idle;
            }
            0x00 | 0x50 => {
                self.half = 0;
                self.port = Port::Idle;
            }
            0x30 => self.load(),
            0x80 => {
                let page = self.chip().map_or(PAGE, |c| c.page);
                self.buf = vec![0xFF; page];
                self.cursor = 0;
                self.port = Port::Program;
            }
            0x10 => {
                let Some(chip) = self.chip() else { return };
                let at = self.address(chip).0 * chip.page;
                let buf = std::mem::take(&mut self.buf);
                let flash = self.flash_mut();
                for (i, b) in buf.iter().enumerate() {
                    if let Some(f) = flash.get_mut(at + i) {
                        *f &= b;
                    }
                }
                self.buf = buf;
                if self.select == 0 {
                    self.dirty = true;
                    self.touched.insert(self.row as usize / PAGES_PER_BLOCK);
                }
                let (sel, page) = (self.select, self.address(chip).0);
                self.note(|| format!("chip {sel} program page {page:#07x}"));
                self.port = Port::Idle;
            }
            0x60 => {}
            0xD0 => {
                let Some(chip) = self.chip() else { return };
                let row = ((self.row << 16) | self.column) as usize;
                let size = chip.pages_per_block * chip.page;
                let start = row / chip.pages_per_block * size;
                if let Some(b) = self.flash_mut().get_mut(start..start + size) {
                    b.fill(0xFF);
                }
                if self.select == 0 {
                    self.dirty = true;
                    self.touched.insert(row / PAGES_PER_BLOCK);
                }
                let sel = self.select;
                self.note(|| format!("chip {sel} erase block of row {row:#07x}"));
            }
            _ => self.note(|| format!("NAND command {c:#04x}?")),
        }
    }

    pub(crate) fn read_data(&mut self) -> u8 {
        let Some(chip) = self.chip() else { return 0xFF };
        match self.port {
            Port::Id => {
                let v = chip.id.get(self.cursor).copied().unwrap_or(0);
                self.cursor += 1;
                v
            }
            // Ready, not write protected, no failure.
            Port::Status => 0xE0,
            Port::Page => {
                // Small-page chips read on into the next page.
                if chip.small && self.cursor >= chip.page {
                    let next = self.address(chip).0 + 1;
                    self.column = ((next & 0xFF) << 8) as u32;
                    self.row = (next >> 8) as u32;
                    self.half = 0;
                    self.load();
                }
                let v = self.buf.get(self.cursor).copied().unwrap_or(0xFF);
                self.cursor += 1;
                v
            }
            _ => 0xFF,
        }
    }

    pub(crate) fn write_data(&mut self, v: u8) {
        if self.port == Port::Program {
            let column = self.chip().map_or(self.column as usize, |c| self.address(c).1);
            let at = column + self.cursor;
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
            (0x14C, Some(v)) => {
                self.row = v;
                // A small-page chip starts a read on the address.
                if self.port == Port::Idle && self.chip().is_some_and(|c| c.small) {
                    self.load();
                }
            }
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
