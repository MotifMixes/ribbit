//! The Didj as far as booting goes (milestone 2): the ARM926EJ-S, SDRAM,
//! the NAND controller and the UARTs. Every other register reads back what
//! was last written to it, and the first access to each one is logged, so a
//! run shows which hardware the software needs next.
//!
//! Sources: the kernel headers include/asm-arm/arch-lf1000/*.h and
//! lightning-boot nand.c/debug.c (opendidj). What is a guess is marked
//! GUESS; see docs/didj.md.

use std::collections::{BTreeMap, HashMap};

use ribbit_arm::{Arm, ArmBus};

use crate::nand::{self, Nand};
use crate::timer::{self, Timers};

pub const SDRAM_SIZE: usize = 32 << 20;

/// How much of the NAND the chip copies to SDRAM 0 before running it.
/// GUESS: the boot ROM's amount is UNKNOWN; lightning-boot.bin (7386 bytes)
/// must fit, so one whole erase block is copied.
pub const BOOT_COPY: usize = nand::BLOCK;

/// NAND data/command/address ports (static bus 7 and 11 in shadow mode; the
/// kernel's nand.h picks one per board).
const NAND_WINDOWS: [u32; 2] = [0x9C00_0000, 0xAC00_0000];
const NFDATA: u32 = 0x00;
const NFCMD: u32 = 0x10;
const NFADDR: u32 = 0x18;

/// Static memory controller (NAND control and ECC registers).
const MCU_S: u32 = 0xC001_5800;
const NFCONTROL: u32 = MCU_S + 0x74;
const NFECCSTATUS: u32 = MCU_S + 0x8C;
const RNB: u32 = 1 << 9;
const NFECCDECDONE: u32 = 1 << 1;

/// Clock and power: PWRMODE's CHGPLL bit asks for a PLL change and reads 1
/// until it is done (clkpwr.h). Done at once here (GUESS: the real time is
/// UNKNOWN).
const PWRMODE: u32 = 0xC000_F07C;
const CHGPLL: u32 = 1 << 15;

/// ADC (adc.h): writing ADEN in ADCCON starts a conversion of channel ASEL;
/// ADEN reads 1 until ADCDAT holds the result. Done at once here.
const ADCCON: u32 = 0xC000_5000;
const ADCDAT: u32 = 0xC000_5004;
const ADEN: u32 = 1 << 0;
/// A finished conversion sets the ADC's pending bit; ADCINTCLR clears it,
/// ADCINTENB lets it through to the interrupt controller (kernel adc.c).
const ADCINTENB: u32 = 0xC000_5008;
const ADCINTCLR: u32 = 0xC000_500C;
const ADC_IRQ: u32 = 25;
/// Readings per channel (10 bits). Channel 2 is the battery: lightning-boot
/// takes it as 1957 * r / 256 + 190 mV and won't boot under 4400 mV; 760 is
/// 6.0 V (four fresh AA cells). The others (0 LED voltage, 1 volume) are
/// GUESSES.
const ADC_READINGS: [u32; 8] = [512, 512, 760, 0, 0, 0, 0, 0];

/// SPI0 (drivers/lf1000/spi): 16-bit registers. Every word written to
/// SSPSPIDATA is sent and one word received (GUESS: 0; what the LCD panel
/// answers is UNKNOWN), which sets IRQE (transfer done) and IRQR (receive
/// FIFO has data); writing those bits to SSPSPISTATE clears them.
const SPI0: u32 = 0xC000_7800;
const SPI_IRQ: u32 = 12;

#[derive(Default)]
struct Spi {
    rx: std::collections::VecDeque<u16>,
    /// SSPSPISTATE bits 13-15 (interrupt enables).
    enables: u16,
    /// IRQE (bit 6), IRQW (bit 5), IRQR (bit 4).
    pending: u16,
}

impl Spi {
    fn read(&mut self, off: u32) -> Option<u32> {
        match off {
            0x04 => Some(self.rx.pop_front().unwrap_or(0) as u32),
            // Transmit FIFO always empty.
            0x06 => Some((self.enables | self.pending | 1 << 2 | self.rx.is_empty() as u16) as u32),
            _ => None,
        }
    }

    fn write(&mut self, off: u32, v: u32) -> bool {
        match off {
            0x04 => {
                self.rx.push_back(0);
                self.pending |= 1 << 6 | 1 << 4;
            }
            0x06 => {
                self.enables = v as u16 & 0xE000;
                self.pending &= !(v as u16 & 0x70);
            }
            _ => return false,
        }
        true
    }

    fn irq(&self) -> bool {
        let e = self.enables;
        (e & 1 << 15 != 0 && self.pending & 1 << 6 != 0) || (e & 1 << 13 != 0 && self.pending & 1 << 4 != 0)
    }
}

/// MLC (multi-layer controller): software sets a "dirty" bit to commit a
/// layer's new settings, and the hardware clears it when it takes them at
/// the next frame (kernel drivers/lf1000/mlc: MLCCONTROLT bit 3, layer
/// control bit 4 at 0x24, 0x58, 0x7C).
const MLC: u32 = 0xC000_4000;
const MLC_DIRTY: [(u32, u32); 4] = [(MLC, 1 << 3), (MLC + 0x24, 1 << 4), (MLC + 0x58, 1 << 4), (MLC + 0x7C, 1 << 4)];
/// Frames per second (GUESS: 60; the real rate follows the DPC timing,
/// not modelled yet).
const FRAME_HZ: u64 = 60;

/// GPIO port A's input levels (gpio_hal.h GPIOAPAD). Pin 18 is the
/// cartridge detect, low when a cartridge is in (nand.h
/// NAND_CART_DETECT_LEVEL; launch_main reads it with gpio-control): high
/// here, an empty slot. The other pins read low (UNKNOWN).
const GPIOAPAD: u32 = 0xC000_A018;
const CART_DETECT: u32 = 1 << 18;

/// UART 0-5 (uart.h); the transmit holding register and the status.
const UARTS: [u32; 6] = [0xC001_6000, 0xC001_6080, 0xC001_6800, 0xC001_6880, 0xC001_7000, 0xC001_7080];
const TRSTATUS: u32 = 0x08;
/// FIFO status: receive FIFO count in bits 0-3, transmit count in 4-7.
const FSR: u32 = 0x0C;
const THB: u32 = 0x10;
/// The receive buffer (read as a byte by 8250-lf1000.c).
const RHB: u32 = 0x12;
/// Transmit buffer empty, transmitter empty; nothing received.
const TRSTATUS_IDLE: u32 = 0b110;
const TRSTATUS_DR: u32 = 1 << 0;
/// INTSTATUSREG (serial_reg-lf1000.h): pending flags in bits 0-3 (bit 0
/// transmit holding register empty), their enables in bits 4-7. Bytes go
/// out at once here, so "transmit empty" is always pending; Linux's console
/// waits for that interrupt after each write.
const INTSTATUS: u32 = 0x18;
const THRI_PENDING: u32 = 1 << 0;
const THRI_ENABLE: u32 = 1 << 4;
/// Received data waiting (UART_IIR_RDI) and its enable (UART_IER_RDI).
const RDI_PENDING: u32 = 1 << 1;
const RDI_ENABLE: u32 = 1 << 5;
/// UART0's interrupt (the others are UNKNOWN use on the Didj).
const UART0_IRQ: u32 = 10;

#[derive(Default)]
struct NandChip {
    cmd: u8,
    addr: Vec<u8>,
    /// Bytes being read out (page data from the column on, then spare).
    out: Vec<u8>,
    pos: usize,
    status: u8,
}

pub struct Bus {
    pub sdram: Vec<u8>,
    pub nand: Nand,
    chip: NandChip,
    pub timers: Timers,
    spi: Spi,
    pub dma: crate::dma::Dma,
    pub ga3d: crate::ga3d::Ga3d,
    adc_pending: bool,
    /// Registers with no model: the last value written.
    regs: HashMap<u32, u32>,
    /// Unmodelled addresses in order of first access: (address, first PC,
    /// read count, write count).
    pub touched: BTreeMap<u32, (u32, u64, u64)>,
    /// The last unmodelled accesses, oldest first: (address, PC, write).
    pub recent: std::collections::VecDeque<(u32, u32, bool)>,
    pub uart: Vec<String>,
    /// Bytes waiting to be received on UART0 (the console's serial port).
    pub uart_rx: std::collections::VecDeque<u8>,
    /// The PC of the instruction being run (for the access log).
    pub pc: u32,
    /// Cycles since the last frame.
    frame_cycles: u64,
}

impl Bus {
    /// An unmodelled register's last written value (0 if never written).
    pub fn reg(&self, addr: u32) -> u32 {
        self.regs.get(&addr).copied().unwrap_or(0)
    }

    /// The picture on the screen: (width, height, 0x00RRGGBB pixels).
    pub fn frame(&self) -> (usize, usize, Vec<u32>) {
        let mem = |a: u32, n: u32| {
            let a = if (crate::mlc::BLOCK_BASE..crate::mlc::BLOCK_END).contains(&a) { crate::mlc::block_sdram(a, false) as u32 } else { a };
            (0..n).fold(0, |v, i| v | (self.sdram.get((a + i) as usize).copied().unwrap_or(0) as u32) << (8 * i))
        };
        crate::mlc::compose(|a| self.reg(a), mem)
    }

    fn note(&mut self, addr: u32, write: bool) {
        if self.recent.len() == 64 {
            self.recent.pop_front();
        }
        self.recent.push_back((addr, self.pc, write));
        let e = self.touched.entry(addr).or_insert((self.pc, 0, 0));
        if write {
            e.2 += 1;
        } else {
            e.1 += 1;
        }
    }

    fn nand_port(addr: u32) -> Option<u32> {
        NAND_WINDOWS.iter().find(|&&w| addr.wrapping_sub(w) < 0x20).map(|&w| addr - w)
    }

    fn nand_command(&mut self, cmd: u8) {
        let c = &mut self.chip;
        match cmd {
            // 0x00 starts a read's address, and also switches the data port
            // back to the page register after 0x30 (lightning-boot sends it
            // before reading out); the page register stays either way.
            0x00 | 0x80 | 0x60 | 0x90 => {
                if cmd != 0x00 {
                    c.out.clear();
                }
                c.pos = if cmd == 0x90 { 0 } else { c.pos };
                c.addr.clear();
                c.cmd = cmd;
            }
            0x30 => {
                // Read: column (2 bytes), page (3 bytes).
                let col = c.addr.first().copied().unwrap_or(0) as usize | (c.addr.get(1).copied().unwrap_or(0) as usize) << 8;
                let page = row(&c.addr[2.min(c.addr.len())..]);
                c.out = self.nand.page(page % (nand::SIZE / nand::PAGE)).to_vec();
                c.pos = col.min(nand::PAGE_RAW);
                c.cmd = 0x30;
            }
            0x10 => {
                // Program: the bytes written since 0x80 go at the column.
                let col = c.addr.first().copied().unwrap_or(0) as usize | (c.addr.get(1).copied().unwrap_or(0) as usize) << 8;
                let page = row(&c.addr[2.min(c.addr.len())..]) % (nand::SIZE / nand::PAGE);
                let data = std::mem::take(&mut c.out);
                let p = self.nand.page_mut(page);
                for (i, b) in data.iter().enumerate() {
                    if let Some(x) = p.get_mut(col + i) {
                        *x &= b;
                    }
                }
                c.cmd = 0;
            }
            0xD0 => {
                let block = row(&c.addr) / nand::PAGES_PER_BLOCK;
                if block < nand::BLOCKS {
                    self.nand.erase(block);
                }
                c.cmd = 0;
            }
            0x70 => {
                c.status = 0xC0; // ready, not write protected, no error
                c.cmd = 0x70;
            }
            0xFF => *c = NandChip::default(),
            _ => c.cmd = cmd,
        }
    }

    fn nand_read_byte(&mut self) -> u8 {
        let c = &mut self.chip;
        match c.cmd {
            0x70 => c.status,
            0x90 => {
                // GUESS: Samsung K9F2G08 (256 MB, 2 KiB pages); the real
                // part is UNKNOWN.
                let id = [0xEC, 0xDA, 0x10, 0x95, 0x44];
                let b = id.get(c.pos).copied().unwrap_or(0);
                c.pos += 1;
                b
            }
            _ => {
                let b = c.out.get(c.pos).copied().unwrap_or(0xFF);
                c.pos += 1;
                b
            }
        }
    }

    fn nand_write_byte(&mut self, b: u8) {
        let c = &mut self.chip;
        if c.cmd == 0x80 {
            c.out.push(b);
        }
    }

    fn io_read(&mut self, addr: u32) -> u32 {
        if let Some(port) = Self::nand_port(addr) {
            return if port == NFDATA { self.nand_read_byte() as u32 } else { 0 };
        }
        match addr {
            NFCONTROL => return self.regs.get(&addr).copied().unwrap_or(0) | RNB,
            NFECCSTATUS => return NFECCDECDONE,
            GPIOAPAD => return CART_DETECT,
            _ => {}
        }
        if (timer::TIMER_BASE..timer::TIMER_END).contains(&addr) || (timer::IC_BASE..timer::IC_END).contains(&addr) {
            return self.timers.read(addr);
        }
        if let Some(&u) = UARTS.iter().find(|&&u| addr.wrapping_sub(u) < 0x80) {
            let rx = u == UARTS[0] && !self.uart_rx.is_empty();
            match addr - u {
                TRSTATUS => return TRSTATUS_IDLE | if rx { TRSTATUS_DR } else { 0 },
                FSR => return if u == UARTS[0] { self.uart_rx.len().min(15) as u32 } else { 0 },
                THB => return 0,
                INTSTATUS => {
                    return self.regs.get(&addr).copied().unwrap_or(0) | THRI_PENDING | if rx { RDI_PENDING } else { 0 };
                }
                _ => {}
            }
        }
        self.note(addr, false);
        self.regs.get(&addr).copied().unwrap_or(0)
    }

    fn io_write(&mut self, addr: u32, v: u32, size: u32) {
        if let Some(port) = Self::nand_port(addr) {
            match port {
                NFCMD => self.nand_command(v as u8),
                NFADDR => self.chip.addr.push(v as u8),
                NFDATA => {
                    for i in 0..size {
                        self.nand_write_byte((v >> (8 * i)) as u8);
                    }
                }
                _ => {}
            }
            return;
        }
        if (timer::TIMER_BASE..timer::TIMER_END).contains(&addr) || (timer::IC_BASE..timer::IC_END).contains(&addr) {
            return self.timers.write(addr, v);
        }
        if let Some(&u) = UARTS.iter().find(|&&u| addr.wrapping_sub(u) < 0x80) {
            if addr - u == THB {
                let n = UARTS.iter().position(|&x| x == u).unwrap();
                if self.uart.len() <= n {
                    self.uart.resize(n + 1, String::new());
                }
                self.uart[n].push(v as u8 as char);
                return;
            }
            if addr - u == INTSTATUS {
                // Pending flags clear by writing 1; only the enables stay.
                self.regs.insert(addr, v & !0xF);
                return;
            }
        }
        self.note(addr, true);
        let v = match addr {
            PWRMODE => v & !CHGPLL,
            ADCCON if v & ADEN != 0 => {
                self.regs.insert(ADCDAT, ADC_READINGS[(v >> 3 & 7) as usize]);
                self.adc_pending = true;
                v & !ADEN
            }
            ADCINTCLR => {
                if v & 1 != 0 {
                    self.adc_pending = false;
                }
                0
            }
            _ => v,
        };
        self.regs.insert(addr, v);
    }

    fn read_n(&mut self, addr: u32, size: u32) -> u32 {
        // Block (2D) addresses reach SDRAM through the display-mode layout.
        let a = if (crate::mlc::BLOCK_BASE..crate::mlc::BLOCK_END).contains(&addr) { crate::mlc::block_sdram(addr, false) } else { addr as usize };
        if a + size as usize <= SDRAM_SIZE {
            let mut v = 0;
            for i in 0..size as usize {
                v |= (self.sdram[a + i] as u32) << (8 * i);
            }
            return v;
        }
        if Self::nand_port(addr) == Some(NFDATA) {
            let mut v = 0;
            for i in 0..size {
                v |= (self.nand_read_byte() as u32) << (8 * i);
            }
            return v;
        }
        if let Some(v) = addr.checked_sub(SPI0).filter(|&o| o < 8).and_then(|o| self.spi.read(o)) {
            return v;
        }
        if (crate::ga3d::BASE..crate::ga3d::END).contains(&addr) {
            let shift = 8 * (addr & 3);
            let mask = if size == 4 { u32::MAX } else { (1 << (8 * size)) - 1 };
            return (self.ga3d.read(addr & !3) >> shift) & mask;
        }
        if (crate::dma::BASE..crate::dma::END).contains(&addr) {
            let shift = 8 * (addr & 3);
            let mask = if size == 4 { u32::MAX } else { (1 << (8 * size)) - 1 };
            return (self.dma.read(addr & !3) >> shift) & mask;
        }
        if addr == UARTS[0] + RHB {
            return self.uart_rx.pop_front().unwrap_or(0) as u32;
        }
        let shift = 8 * (addr & 3);
        let mask = if size == 4 { u32::MAX } else { (1 << (8 * size)) - 1 };
        (self.io_read(addr & !3) >> shift) & mask
    }

    fn write_n(&mut self, addr: u32, v: u32, size: u32) {
        // Block (2D) addresses reach SDRAM through the display-mode layout.
        let a = if (crate::mlc::BLOCK_BASE..crate::mlc::BLOCK_END).contains(&addr) { crate::mlc::block_sdram(addr, false) } else { addr as usize };
        if a + size as usize <= SDRAM_SIZE {
            for i in 0..size as usize {
                self.sdram[a + i] = (v >> (8 * i)) as u8;
            }
            return;
        }
        if Self::nand_port(addr).is_some() {
            return self.io_write(addr, v, size);
        }
        if addr.checked_sub(SPI0).is_some_and(|o| o < 8 && self.spi.write(o, v)) {
            return;
        }
        // Narrow writes merge into the register's 32-bit value.
        let base = addr & !3;
        if (crate::ga3d::BASE..crate::ga3d::END).contains(&addr) {
            let shift = 8 * (addr & 3);
            let mask = if size == 4 { u32::MAX } else { ((1 << (8 * size)) - 1) << shift };
            let v = (self.ga3d.read(base) & !mask) | ((v << shift) & mask);
            let mut mem = crate::ga3d::Mem { sdram: &mut self.sdram };
            return self.ga3d.write(base, v, &mut mem);
        }
        if (crate::dma::BASE..crate::dma::END).contains(&addr) {
            let shift = 8 * (addr & 3);
            let mask = if size == 4 { u32::MAX } else { ((1 << (8 * size)) - 1) << shift };
            let old = self.dma.read(base);
            return self.dma.write(base, (old & !mask) | ((v << shift) & mask), &self.sdram);
        }
        let shift = 8 * (addr & 3);
        let mask = if size == 4 { u32::MAX } else { ((1 << (8 * size)) - 1) << shift };
        let old = self.regs.get(&base).copied().unwrap_or(0);
        self.io_write(base, (old & !mask) | ((v << shift) & mask), size);
    }
}

/// A row (page) address from up to 3 address bytes.
fn row(bytes: &[u8]) -> usize {
    bytes.iter().take(3).enumerate().fold(0, |acc, (i, &b)| acc | (b as usize) << (8 * i))
}

impl ArmBus for Bus {
    fn read8(&mut self, addr: u32) -> u8 {
        self.read_n(addr, 1) as u8
    }
    fn read16(&mut self, addr: u32) -> u16 {
        self.read_n(addr, 2) as u16
    }
    fn read32(&mut self, addr: u32) -> u32 {
        self.read_n(addr, 4)
    }
    fn write8(&mut self, addr: u32, value: u8) {
        self.write_n(addr, value as u32, 1)
    }
    fn write16(&mut self, addr: u32, value: u16) {
        self.write_n(addr, value as u32, 2)
    }
    fn write32(&mut self, addr: u32, value: u32) {
        self.write_n(addr, value, 4)
    }
}

pub struct Didj {
    pub cpu: Arm,
    pub bus: Bus,
}

impl Didj {
    /// Powers on: the start of the NAND is copied to SDRAM 0 and run.
    pub fn new(flash: Nand) -> Self {
        let mut sdram = vec![0; SDRAM_SIZE];
        sdram[..BOOT_COPY].copy_from_slice(&flash.read(0, BOOT_COPY));
        let bus = Bus {
            sdram,
            nand: flash,
            chip: NandChip::default(),
            timers: Timers::new(),
            spi: Spi::default(),
            dma: Default::default(),
            ga3d: Default::default(),
            adc_pending: false,
            regs: HashMap::new(),
            touched: BTreeMap::new(),
            recent: Default::default(),
            uart: Vec::new(),
            uart_rx: Default::default(),
            pc: 0,
            frame_cycles: 0,
        };
        Self { cpu: Arm::new(), bus }
    }

    /// The processor clock and the PLL outputs, from the clock registers.
    /// Before they are set: 393 MHz (GUESS: the boot ROM's setting is
    /// UNKNOWN).
    fn clocks(&self) -> (u64, [Option<u64>; 2]) {
        let reg = |a: u32| self.bus.regs.get(&a).copied().unwrap_or(0);
        let plls = [timer::pll_hz(reg(timer::PLLSETREG0)), timer::pll_hz(reg(timer::PLLSETREG0 + 4))];
        let mode = reg(timer::CLKMODEREG);
        let cpu = plls.get((mode >> 4 & 3) as usize).copied().flatten().map(|p| p / ((mode & 0xF) as u64 + 1));
        (cpu.unwrap_or(393_000_000), plls)
    }

    /// Runs `n` processor cycles (one per instruction for now), the timers
    /// and interrupts with them. Returns the instructions run.
    pub fn run(&mut self, n: u64) -> u64 {
        const SLICE: u64 = 64;
        let start = self.cpu.instructions;
        let end = self.cpu.cycles + n;
        while self.cpu.cycles < end {
            let before = self.cpu.cycles;
            for _ in 0..SLICE {
                self.bus.pc = self.cpu.r[15];
                if !self.cpu.step(&mut self.bus) {
                    // Waiting for an interrupt: skip to the next slice.
                    self.cpu.cycles += SLICE - 1;
                    break;
                }
            }
            let (cpu_hz, plls) = self.clocks();
            self.bus.timers.tick(self.cpu.cycles - before, cpu_hz, plls);
            self.bus.dma.tick(self.cpu.cycles - before, cpu_hz, &self.bus.sdram);
            self.bus.frame_cycles += self.cpu.cycles - before;
            if self.bus.frame_cycles >= cpu_hz / FRAME_HZ {
                self.bus.frame_cycles = 0;
                for (reg, bit) in MLC_DIRTY {
                    if let Some(v) = self.bus.regs.get_mut(&reg) {
                        *v &= !bit;
                    }
                }
            }
            if self.bus.adc_pending && self.bus.regs.get(&ADCINTENB).is_some_and(|v| v & 1 != 0) {
                self.bus.timers.raise(ADC_IRQ);
            }
            let rx = !self.bus.uart_rx.is_empty();
            if self.bus.regs.get(&(UARTS[0] + INTSTATUS)).is_some_and(|v| v & THRI_ENABLE != 0 || rx && v & RDI_ENABLE != 0) {
                self.bus.timers.raise(UART0_IRQ);
            }
            if self.bus.dma.irq() {
                self.bus.timers.raise(crate::dma::IRQ);
            }
            if self.bus.spi.irq() {
                self.bus.timers.raise(SPI_IRQ);
            }
            (self.cpu.irq_line, self.cpu.fiq_line) = self.bus.timers.lines();
        }
        self.cpu.instructions - start
    }
}
