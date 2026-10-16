//! A research probe, not a machine model: runs a firmware image on the
//! ARM9 model with nothing but memory around it and records what the code
//! reaches for, to map the system-on-chip.
//!
//! Everything outside the loaded code is plain memory that remembers what
//! was written (so the code runs on as far as it can), and every access
//! outside RAM is counted by 64 KB page with the first instruction that
//! made it. Writes to 0xC0060000 (the GPL32300A's UART data register, from
//! the Zippity) are also collected as text in case the chip shares it.
//!
//! What is RAM is a guess (`RAM_SIZE` from 0, as on the Zippity): the
//! probe's report is evidence to read, never a description of the console.

use ribbit_arm::{Arm, ArmBus};
use std::collections::{BTreeMap, HashMap};

/// Memory taken as RAM, from address 0 (GUESS).
pub const RAM_SIZE: usize = 64 << 20;
/// The Zippity's UART transmit register (GPL32300A).
pub const UART_TX: u32 = 0xC006_0000;

/// What one 64 KB page outside RAM saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageUse {
    pub reads: u64,
    pub writes: u64,
    /// The first access: (pc, address, written value or None).
    pub first: (u32, u32, Option<u32>),
}

pub struct ProbeBus {
    pub ram: Vec<u8>,
    /// Memory outside RAM, word by word, as last written.
    other: HashMap<u32, u32>,
    /// Use of each page outside RAM (key: address >> 16).
    pub pages: BTreeMap<u32, PageUse>,
    /// Bytes written to `UART_TX`.
    pub uart: Vec<u8>,
    /// Research stubs: reads of these words return these values (labelled
    /// in every report that uses them; never hardware facts).
    pub answers: HashMap<u32, u32>,
    /// The instruction being run (set by `Probe::run`).
    pc: u32,
}

impl ProbeBus {
    fn note(&mut self, a: u32, write: Option<u32>) {
        let pc = self.pc;
        let p = self.pages.entry(a >> 16).or_insert(PageUse { first: (pc, a, write), ..Default::default() });
        match write {
            Some(_) => p.writes += 1,
            None => p.reads += 1,
        }
    }

    fn read_other(&mut self, a: u32) -> u32 {
        self.note(a, None);
        if let Some(&v) = self.answers.get(&(a & !3)) {
            return v;
        }
        self.other.get(&(a & !3)).copied().unwrap_or(0)
    }

    fn write_other(&mut self, a: u32, v: u32, mask: u32) {
        self.note(a, Some(v));
        if a & !3 == UART_TX {
            self.uart.push(v as u8);
        }
        let shift = 8 * (a & 3);
        let w = self.other.entry(a & !3).or_insert(0);
        *w = (*w & !(mask << shift)) | ((v & mask) << shift);
    }

    fn in_ram(&self, a: u32, n: usize) -> bool {
        (a as usize).checked_add(n).is_some_and(|end| end <= self.ram.len())
    }
}

impl ArmBus for ProbeBus {
    fn read8(&mut self, a: u32) -> u8 {
        if self.in_ram(a, 1) { self.ram[a as usize] } else { (self.read_other(a) >> (8 * (a & 3))) as u8 }
    }
    fn read16(&mut self, a: u32) -> u16 {
        if self.in_ram(a, 2) {
            u16::from_le_bytes([self.ram[a as usize], self.ram[a as usize + 1]])
        } else {
            (self.read_other(a) >> (8 * (a & 2))) as u16
        }
    }
    fn read32(&mut self, a: u32) -> u32 {
        if self.in_ram(a, 4) {
            let i = a as usize;
            u32::from_le_bytes([self.ram[i], self.ram[i + 1], self.ram[i + 2], self.ram[i + 3]])
        } else {
            self.read_other(a)
        }
    }
    fn write8(&mut self, a: u32, v: u8) {
        if self.in_ram(a, 1) {
            self.ram[a as usize] = v;
        } else {
            self.write_other(a, v as u32, 0xFF);
        }
    }
    fn write16(&mut self, a: u32, v: u16) {
        if self.in_ram(a, 2) {
            self.ram[a as usize..a as usize + 2].copy_from_slice(&v.to_le_bytes());
        } else {
            self.write_other(a, v as u32, 0xFFFF);
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        if self.in_ram(a, 4) {
            self.ram[a as usize..a as usize + 4].copy_from_slice(&v.to_le_bytes());
        } else {
            self.write_other(a, v, 0xFFFF_FFFF);
        }
    }
}

/// Why a probe run ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum End {
    /// Ran the whole budget.
    Budget,
    /// Waiting for an interrupt that nothing will raise.
    WaitingForInterrupt,
}

pub struct Probe {
    pub cpu: Arm,
    pub bus: ProbeBus,
    /// How often each instruction address ran in the last `HOT_WINDOW`
    /// instructions (the loops it ended in).
    pub hot: HashMap<u32, u64>,
}

/// Instructions at the end of a run counted in `Probe::hot`.
pub const HOT_WINDOW: u64 = 200_000;

impl Probe {
    /// The code of an image (see `image::code`) loaded at address 0, the
    /// processor reset to run it.
    pub fn new(code: &[u8]) -> Self {
        let mut ram = vec![0; RAM_SIZE];
        let n = code.len().min(RAM_SIZE);
        ram[..n].copy_from_slice(&code[..n]);
        let mut cpu = Arm::new();
        cpu.reset();
        Self {
            cpu,
            bus: ProbeBus {
                ram,
                other: HashMap::new(),
                pages: BTreeMap::new(),
                uart: Vec::new(),
                answers: HashMap::new(),
                pc: 0,
            },
            hot: HashMap::new(),
        }
    }

    /// Runs up to `budget` instructions.
    pub fn run(&mut self, budget: u64) -> End {
        let hot_from = budget.saturating_sub(HOT_WINDOW);
        for i in 0..budget {
            self.bus.pc = self.cpu.r[15];
            if !self.cpu.step(&mut self.bus) {
                return End::WaitingForInterrupt;
            }
            if i >= hot_from {
                *self.hot.entry(self.cpu.current).or_default() += 1;
            }
        }
        End::Budget
    }

    /// The `n` addresses run most often at the end.
    pub fn hottest(&self, n: usize) -> Vec<(u32, u64)> {
        let mut v: Vec<_> = self.hot.iter().map(|(&a, &c)| (a, c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    #[test]
    fn records_peripheral_accesses_and_uart_text() {
        // mov r0, #0xC0000000; orr r0, r0, #0x60000; mov r1, #'H';
        // str r1, [r0]; mov r1, #'i'; str r1, [r0]; ldr r2, [r0, #0x10]; b .
        let code = words(&[
            0xE3A0_0103,
            0xE380_0806,
            0xE3A0_1048,
            0xE580_1000,
            0xE3A0_1069,
            0xE580_1000,
            0xE590_2010,
            0xEAFF_FFFE,
        ]);
        let mut p = Probe::new(&code);
        assert_eq!(p.run(100), End::Budget);
        assert_eq!(p.bus.uart, b"Hi");
        let page = p.bus.pages[&0xC006];
        assert_eq!((page.reads, page.writes), (1, 2));
        assert_eq!(page.first, (0xC, UART_TX, Some(b'H' as u32)));
        assert_eq!(p.hottest(1)[0].0, 0x1C, "ends in the branch-to-self");
    }
}
