//! Research: runs the Zippity bootloader on the ARM9 model and logs the
//! unknown hardware it touches.
//!
//! zipboot <nand.bin> [instructions]
//!
//! The NAND image is the raw dump with spare bytes (pages of 0x800 + 0x40,
//! Micron MT29F2G08AACWP, ID 2C DA 80 15). As MAME's gpl32612 stub does, the
//! boot ROM is replaced by a copy of data 0xA0000..0x1F0000 (spare bytes
//! stripped) to address 0 and a jump to 0.
//!
//! The NAND controller at 0xD0900000 is modelled from its driver in the
//! bootloader (UNVERIFIED beyond what the driver does):
//!   +0x000 DMA control: bit 0 start/busy, bit 1 memory to NAND
//!   +0x004 DMA memory address, +0x008 DMA length (bytes)
//!   +0x140 status: bit 15 ready
//!   +0x144 command (a NAND command byte: FF reset, 90 ID, 00/30 read,
//!          80/10 program, 60/D0 erase, 70 status)
//!   +0x148 column (or, for erase, row bits 15-0)
//!   +0x14C row (or, for erase, row bits 31-16)
//!   +0x150 data port (a byte per access)
//!   +0x154 saved and restored around Read ID (chip select? UNKNOWN)
//!   +0x158..0x164 ECC engine (control, sector size, error bits, count);
//!          reads 0: no errors

use ribbit_arm::{Arm, ArmBus};
use std::collections::BTreeMap;

const RAM_SIZE: usize = 64 << 20;
const SRAM_BASE: u32 = 0x2000_0000;
const SRAM_SIZE: usize = 0x20_0000;
/// The bootloader copies code to 0xF8000000 and runs it: on-chip RAM
/// (size UNKNOWN; 64 KB assumed).
const IRAM_BASE: u32 = 0xF800_0000;
const IRAM_SIZE: usize = 0x1_0000;

const PAGE: usize = 0x840;
const PAGE_DATA: usize = 0x800;
const PAGES_PER_BLOCK: usize = 64;
const NAND_ID: [u8; 5] = [0x2C, 0xDA, 0x80, 0x15, 0x50];

#[derive(PartialEq)]
enum Port {
    Idle,
    Id,
    Status,
    Page,
    Program,
}

struct Nand {
    flash: Vec<u8>,
    port: Port,
    column: u32,
    row: u32,
    /// The page register (read: loaded from flash; program: filled by writes).
    buf: Vec<u8>,
    cursor: usize,
    dma_addr: u32,
    dma_len: u32,
    regs: BTreeMap<u32, u32>,
    log: Vec<String>,
}

impl Nand {
    fn note(&mut self, line: String) {
        if self.log.len() < 3000 {
            self.log.push(line);
        }
    }

    fn load(&mut self) {
        let at = self.row as usize * PAGE;
        self.buf = self.flash.get(at..at + PAGE).map_or_else(|| vec![0xFF; PAGE], <[u8]>::to_vec);
        self.cursor = self.column as usize;
        self.port = Port::Page;
        self.note(format!("read page {:#07x} col {:#x}", self.row, self.column));
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
                self.note(format!("program page {:#07x}", self.row));
                self.port = Port::Idle;
            }
            0x60 => {}
            0xD0 => {
                let row = ((self.row << 16) | self.column) as usize;
                let start = row / PAGES_PER_BLOCK * PAGES_PER_BLOCK * PAGE;
                if let Some(b) = self.flash.get_mut(start..start + PAGES_PER_BLOCK * PAGE) {
                    b.fill(0xFF);
                }
                self.note(format!("erase block of row {row:#07x}"));
            }
            _ => self.note(format!("NAND command {c:#04x}?")),
        }
    }

    fn read_data(&mut self) -> u8 {
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

    fn write_data(&mut self, v: u8) {
        if self.port == Port::Program {
            let at = self.column as usize + self.cursor;
            if let Some(b) = self.buf.get_mut(at) {
                *b = v;
            }
            self.cursor += 1;
        }
    }
}

#[derive(Default)]
struct Access {
    reads: u64,
    writes: u64,
    last_write: u32,
    first_pc: u32,
}

struct Bus {
    ram: Vec<u8>,
    sram: Vec<u8>,
    iram: Vec<u8>,
    pc: u32,
    /// Instructions run (a stand-in for time).
    now: u64,
    io: BTreeMap<u32, Access>,
    log: Vec<String>,
    /// Text written to the serial port (0xC0060000).
    uart: String,
    nand: Nand,
}

impl Bus {
    fn mem(&mut self, a: u32, n: usize) -> Option<&mut [u8]> {
        let a = a as usize;
        if a + n <= RAM_SIZE {
            return Some(&mut self.ram[a..a + n]);
        }
        let s = a.wrapping_sub(SRAM_BASE as usize);
        if s + n <= SRAM_SIZE {
            return Some(&mut self.sram[s..s + n]);
        }
        let i = a.wrapping_sub(IRAM_BASE as usize);
        if i + n <= IRAM_SIZE {
            return Some(&mut self.iram[i..i + n]);
        }
        None
    }

    /// The NAND controller (see the top of the file); None: not its register.
    fn nand_io(&mut self, a: u32, write: Option<u32>) -> Option<u32> {
        let off = a.checked_sub(0xD090_0000).filter(|&o| o < 0x200)?;
        let n = &mut self.nand;
        let v = match (off, write) {
            (0x000, Some(v)) => {
                if v & 1 != 0 {
                    let (addr, len) = (n.dma_addr, n.dma_len);
                    let to_nand = v & 2 != 0;
                    for i in 0..len {
                        let at = addr.wrapping_add(i);
                        if to_nand {
                            let b = self.mem(at, 1).map_or(0xFF, |m| m[0]);
                            self.nand.write_data(b);
                        } else {
                            let b = self.nand.read_data();
                            if let Some(m) = self.mem(at, 1) {
                                m[0] = b;
                            }
                        }
                    }
                    let dir = if to_nand { "to NAND" } else { "from NAND" };
                    self.nand.note(format!("DMA {dir} {len:#x} bytes at {addr:08x}"));
                }
                0
            }
            (0x000, None) => 0, // DMA done at once: never busy
            (0x004, Some(v)) => {
                n.dma_addr = v;
                0
            }
            (0x008, Some(v)) => {
                n.dma_len = v;
                0
            }
            (0x140, None) => 0x8000,
            (0x144, Some(v)) => {
                n.command(v & 0xFF);
                0
            }
            (0x148, Some(v)) => {
                n.column = v;
                0
            }
            (0x14C, Some(v)) => {
                n.row = v;
                0
            }
            (0x150, None) => n.read_data() as u32,
            (0x150, Some(v)) => {
                n.write_data(v as u8);
                0
            }
            (o, Some(v)) => {
                n.regs.insert(o, v);
                0
            }
            (0x158 | 0x15C, None) => n.regs.get(&off).copied().unwrap_or(0),
            (_, None) => 0,
        };
        Some(v)
    }

    fn io(&mut self, a: u32, write: Option<u32>, size: u32) -> u32 {
        if let Some(v) = self.nand_io(a, write) {
            return v;
        }
        let pc = self.pc;
        let e = self.io.entry(a).or_insert_with(|| Access { first_pc: pc, ..Default::default() });
        let line = match write {
            Some(v) => {
                e.writes += 1;
                e.last_write = v;
                format!("{pc:08x}  W{size} {a:08x} = {v:08x}")
            }
            None => {
                e.reads += 1;
                format!("{pc:08x}  R{size} {a:08x}")
            }
        };
        // The first few accesses to each register.
        if e.reads + e.writes <= 6 && self.log.len() < 2000 {
            self.log.push(line);
        }
        match (a, write) {
            (0xC006_0000, Some(v)) => {
                self.uart.push(v as u8 as char);
                0
            }
            _ => 0,
        }
    }
}

impl ArmBus for Bus {
    fn read8(&mut self, a: u32) -> u8 {
        match self.mem(a, 1) {
            Some(m) => m[0],
            None => self.io(a, None, 8) as u8,
        }
    }
    fn read16(&mut self, a: u32) -> u16 {
        match self.mem(a, 2) {
            Some(m) => u16::from_le_bytes([m[0], m[1]]),
            None => self.io(a, None, 16) as u16,
        }
    }
    fn read32(&mut self, a: u32) -> u32 {
        match self.mem(a, 4) {
            Some(m) => u32::from_le_bytes(m[..4].try_into().unwrap()),
            None => self.io(a, None, 32),
        }
    }
    fn write8(&mut self, a: u32, v: u8) {
        match self.mem(a, 1) {
            Some(m) => m[0] = v,
            None => {
                self.io(a, Some(v as u32), 8);
            }
        }
    }
    fn write16(&mut self, a: u32, v: u16) {
        match self.mem(a, 2) {
            Some(m) => m.copy_from_slice(&v.to_le_bytes()),
            None => {
                self.io(a, Some(v as u32), 16);
            }
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        match self.mem(a, 4) {
            Some(m) => m.copy_from_slice(&v.to_le_bytes()),
            None => {
                self.io(a, Some(v), 32);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flash = std::fs::read(&args[1]).expect("read the NAND image");
    let steps: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
    // The boot ROM's copy: data 0xA0000..0x1F0000, spare bytes stripped.
    let data: Vec<u8> = flash.chunks(PAGE).take(0x1F_0000 / PAGE_DATA).flat_map(|p| p[..PAGE_DATA].to_vec()).collect();
    let (start, len) = (0xA_0000, 0x15_0000);
    let nand = Nand {
        flash,
        port: Port::Idle,
        column: 0,
        row: 0,
        buf: vec![0xFF; PAGE],
        cursor: 0,
        dma_addr: 0,
        dma_len: 0,
        regs: BTreeMap::new(),
        log: Vec::new(),
    };

    let mut bus = Bus { ram: vec![0; RAM_SIZE], sram: vec![0; SRAM_SIZE], iram: vec![0; IRAM_SIZE], pc: 0, now: 0, io: BTreeMap::new(), log: Vec::new(), uart: String::new(), nand };
    bus.ram[..len].copy_from_slice(&data[start..start + len]);
    let mut cpu = Arm::new();
    cpu.reset();

    let mut hist: BTreeMap<u32, u64> = BTreeMap::new();
    let mut ran = 0u64;
    while ran < steps {
        bus.pc = cpu.r[15];
        bus.now = ran;
        if !cpu.step(&mut bus) {
            println!("waiting for an interrupt at {:08x} after {ran} instructions", cpu.r[15]);
            break;
        }
        ran += 1;
        if ran > steps.saturating_sub(200_000) {
            *hist.entry(cpu.current).or_default() += 1;
        }
        for (pc, w) in cpu.undefined_log.drain(..) {
            println!("undefined {w:08x} at {pc:08x}");
        }
    }

    println!("--- first accesses ---");
    for l in &bus.log {
        println!("{l}");
    }
    println!("--- NAND ({} events) ---", bus.nand.log.len());
    for l in bus.nand.log.iter().take(200) {
        println!("{l}");
    }
    println!("--- serial output ---
{}
--- end of serial output ---", bus.uart);
    println!("--- registers touched ({}) ---", bus.io.len());
    for (a, e) in &bus.io {
        println!("{a:08x}  reads {:>8}  writes {:>8}  last {:08x}  first pc {:08x}", e.reads, e.writes, e.last_write, e.first_pc);
    }
    println!("--- state after {ran} instructions ---");
    println!("pc {:08x} cpsr {:08x} mode {:02x} thumb {}", cpu.r[15], cpu.cpsr, cpu.mode(), cpu.thumb());
    for i in 0..16 {
        print!("r{i}={:08x} ", cpu.r[i]);
        if i % 4 == 3 {
            println!();
        }
    }
    println!("cp15 control {:08x} ttb {:08x}", cpu.cp15.control, cpu.cp15.ttb);
    if let Ok(dir) = std::env::var("ZIPBOOT_DUMP") {
        std::fs::write(format!("{dir}/ram.bin"), &bus.ram[..0x100_0000]).unwrap();
        std::fs::write(format!("{dir}/iram.bin"), &bus.iram).unwrap();
    }
    let mut hot: Vec<_> = hist.into_iter().collect();
    hot.sort_by_key(|h| std::cmp::Reverse(h.1));
    println!("--- hottest pcs (last 200k) ---");
    for (pc, n) in hot.iter().take(16) {
        println!("{pc:08x} {n}");
    }
}
