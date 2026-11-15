//! The ClickStart's memory map and the SPG28x's peripherals (word
//! addresses): RAM 0x0000-0x27ff, video 0x2800-0x2fff, sound 0x3000-0x37ff,
//! I/O 0x3d00-0x3dff (ports, timers, interrupts, UART, ADC), system DMA
//! 0x3e00-0x3e03, and the 22-bit ROM space above: with a cartridge in,
//! 0x000000-0x1fffff reads the cartridge, the rest the system ROM (as in
//! MAME's clickstart driver). The I/O registers follow MAME's spg2xx_io
//! (BSD-3-Clause; see THIRD-PARTY-NOTICES.txt).
//!
//! Time is counted in CPU cycles (27 MHz). Devices that tick (scanlines,
//! timers, the UART) each have their next due time; `Bus::service` runs
//! those due when the CPU loop reaches `next_event`.

use crate::cpu;
use crate::eeprom::Eeprom;
use crate::sio_audio::SioAudio;
use crate::sound::{self, Sound};
use crate::video::{self, LINES, SCREEN_H, Video};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

pub const CLOCK: u64 = 27_000_000;
/// Cycles per frame (60 Hz).
pub const FRAME: u64 = CLOCK / 60;
/// Device times are in 1/SUB of a cycle.
const SUB: u64 = 1 << 16;

const RAM_WORDS: usize = 0x2800;

// I/O registers (offsets from 0x3d00).
const IOC_MASK: usize = 0x0f;
const TIMEBASE_SETUP: usize = 0x10;
const TIMEBASE_CLEAR: usize = 0x11;
const TIMERA_DATA: usize = 0x12;
const TIMERA_CTRL: usize = 0x13;
const TIMERA_IRQCLR: usize = 0x15;
const TIMERB_DATA: usize = 0x16;
const TIMERB_CTRL: usize = 0x17;
const TIMERB_ON: usize = 0x18;
const TIMERB_IRQCLR: usize = 0x19;
const VERT_LINE: usize = 0x1c;
/// System control: the system ROM sets bit 15 to turn the console off
/// (after its goodbye animation; 0x3f0370 then loops).
const SYSTEM_CTRL: usize = 0x20;
const INT_CTRL: usize = 0x21;
const INT_STATUS: usize = 0x22;
const EXT_MEMORY_CTRL: usize = 0x23;
const ADC_CTRL: usize = 0x25;
const ADC_DATA: usize = 0x27;
const NTSC_PAL: usize = 0x2b;
const PRNG1: usize = 0x2c;
const PRNG2: usize = 0x2d;
const FIQ_SEL: usize = 0x2e;
const DATA_SEGMENT: usize = 0x2f;
const UART_CTRL: usize = 0x30;
const UART_STATUS: usize = 0x31;
const UART_BAUD1: usize = 0x33;
const UART_TXBUF: usize = 0x35;
const UART_RXBUF: usize = 0x36;
const UART_RXFIFO: usize = 0x37;
const SIO_SETUP: usize = 0x50;
const SIO_STATUS: usize = 0x51;
const SIO_DATA: usize = 0x54;
const I2C_CMD: usize = 0x58;
const I2C_STATUS: usize = 0x59;

/// A periodic device clock: its next tick and period, in 1/SUB cycles
/// (u64::MAX: stopped).
#[derive(Clone, Copy, Serialize, Deserialize)]
struct Ticker {
    next: u64,
    period: u64,
}

impl Ticker {
    const OFF: Ticker = Ticker { next: u64::MAX, period: 0 };

    fn hz(now: u64, hz: u64) -> Ticker {
        let period = CLOCK * SUB / hz;
        Ticker { next: now * SUB + period, period }
    }

    /// One shot in `cycles`.
    fn once(now: u64, cycles: u64) -> Ticker {
        Ticker { next: (now + cycles) * SUB, period: 0 }
    }

    /// Whether it is due at `now`; if so moves it on.
    fn due(&mut self, now: u64) -> bool {
        if self.next > now * SUB {
            return false;
        }
        self.next = if self.period == 0 { u64::MAX } else { self.next + self.period };
        true
    }

    fn at(&self) -> u64 {
        if self.next == u64::MAX { u64::MAX } else { self.next.div_ceil(SUB) }
    }
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    #[serde(skip)]
    pub rom: Vec<u16>,
    #[serde(skip)]
    pub cart: Option<Vec<u16>>,
    pub ram: Vec<u16>,
    pub video: Video,
    pub sound: Sound,
    pub io: Vec<u16>,
    dma: [u16; 4],

    /// CPU cycles since power-on, and when `service` must next run.
    pub now: u64,
    pub next_event: u64,
    /// The scanline being drawn (0-261) and when the next one starts.
    pub line: u32,
    line_end: u64,
    /// Frames finished (the start of each vertical blank).
    pub frames: u64,
    /// The CPU's data segment (I/O register 0x2f reads and writes it): the
    /// machine copies it in before each instruction and out after.
    pub ds: u16,
    pub ds_written: bool,
    /// The video interrupt goes to the FIQ when FIQ_SEL picks the PPU (0).
    fiq_vector: u8,

    sys: Ticker,
    timer_a_src: Ticker,
    timer_b_src: Ticker,
    tmb1: Ticker,
    tmb2: Ticker,
    rng: Ticker,
    uart_tx: Ticker,
    uart_rx: Ticker,
    adc: Ticker,
    input_feed: Ticker,
    audio: Ticker,
    timer_a_preload: u16,
    timer_b_preload: u16,
    timer_ab_divisor: u32,
    timer_ab_rate: u32,
    div_2khz: u32,
    div_1khz: u32,
    div_4hz: u32,
    uart_baud: u64,
    uart_rx_fifo: VecDeque<u8>,
    uart_rx_available: bool,
    uart_rx_irq: bool,
    uart_tx_irq: bool,
    /// Bytes the keyboard and mouse have still to send (already inverted,
    /// as the line carries them), fed to the UART 320 times a second.
    pub input_bytes: VecDeque<u8>,
    portc_toggle: u16,
    /// The 24C08 on port C's bits 0 (SDA) and 1 (SCL).
    pub eeprom: Eeprom,
    /// The serial interface (SIO, port C bits 11-12): bits left in the
    /// transfer and its direction. Not the EEPROM: it drives a sound chip
    /// (`SioAudio`) that the system streams compressed sound to.
    sio_bits: u32,
    sio_writing: bool,
    pub sio_audio: SioAudio,
    /// The status word being clocked out on port C bit 11, and its bits left.
    sio_status: u16,
    sio_status_bits: u8,
    /// The system turned the console off (SYSTEM_CTRL bit 15); not kept in
    /// states.
    #[serde(skip)]
    pub powered_off: bool,
    /// What the UART sent (for research logs).
    #[serde(skip)]
    pub uart_out: Vec<u8>,
}

impl Bus {
    pub fn new(rom: Vec<u16>) -> Self {
        let mut b = Bus {
            rom,
            cart: None,
            ram: vec![0; RAM_WORDS],
            video: Video::new(),
            sound: Sound::new(),
            io: vec![0; 0x100],
            dma: [0; 4],
            now: 0,
            next_event: 0,
            line: 0,
            line_end: 0,
            frames: 0,
            ds: 0,
            ds_written: false,
            fiq_vector: 0xff,
            sys: Ticker::OFF,
            timer_a_src: Ticker::OFF,
            timer_b_src: Ticker::OFF,
            tmb1: Ticker::OFF,
            tmb2: Ticker::OFF,
            rng: Ticker::OFF,
            uart_tx: Ticker::OFF,
            uart_rx: Ticker::OFF,
            adc: Ticker::OFF,
            input_feed: Ticker::OFF,
            audio: Ticker::OFF,
            timer_a_preload: 0,
            timer_b_preload: 0,
            timer_ab_divisor: 0,
            timer_ab_rate: 0,
            div_2khz: 0,
            div_1khz: 0,
            div_4hz: 0,
            uart_baud: 9600,
            uart_rx_fifo: VecDeque::new(),
            uart_rx_available: false,
            uart_rx_irq: false,
            uart_tx_irq: false,
            input_bytes: VecDeque::new(),
            portc_toggle: 0,
            eeprom: Eeprom::new(),
            sio_bits: 0,
            sio_writing: false,
            sio_audio: SioAudio::default(),
            sio_status: 0,
            sio_status_bits: 0,
            powered_off: false,
            uart_out: Vec::new(),
        };
        b.reset_devices();
        b
    }

    fn reset_devices(&mut self) {
        self.io.iter_mut().for_each(|r| *r = 0);
        self.io[EXT_MEMORY_CTRL] = 0x0028;
        self.io[PRNG1] = 0x1418;
        self.io[PRNG2] = 0x1658;
        self.sys = Ticker::hz(self.now, 4096);
        self.rng = Ticker::hz(self.now, 1234);
        self.input_feed = Ticker::hz(self.now, 320);
        self.audio = Ticker { next: (self.now + sound::CYCLES_PER_SAMPLE) * SUB, period: sound::CYCLES_PER_SAMPLE * SUB };
        self.line = 0;
        self.line_end = self.now + self.line_cycles(0);
        self.update_next();
    }

    /// Cycles in scanline `line`, spreading FRAME over LINES lines.
    fn line_cycles(&self, line: u32) -> u64 {
        let l = line as u64;
        (l + 1) * FRAME / LINES as u64 - l * FRAME / LINES as u64
    }

    fn update_next(&mut self) {
        self.next_event = [
            self.line_end,
            self.sys.at(),
            self.timer_a_src.at(),
            self.timer_b_src.at(),
            self.tmb1.at(),
            self.tmb2.at(),
            self.rng.at(),
            self.uart_tx.at(),
            self.uart_rx.at(),
            self.adc.at(),
            self.input_feed.at(),
            self.audio.at(),
        ]
        .into_iter()
        .min()
        .unwrap();
    }

    // ----- Memory -----

    #[inline]
    fn rom_read(&self, addr: u32) -> u16 {
        let a = addr as usize & 0x3f_ffff;
        if a < 0x20_0000 {
            if let Some(cart) = &self.cart {
                return cart.get(a).copied().unwrap_or(0);
            }
        }
        self.rom.get(a).copied().unwrap_or(0)
    }

    /// Memory as the video unit and DMA see it (no side effects).
    pub fn peek(&self, addr: u32) -> u16 {
        peek_parts(&self.ram, &self.rom, &self.cart, addr)
    }

    pub fn read(&mut self, addr: u32) -> u16 {
        let a = addr & 0x3f_ffff;
        match a {
            0..0x2800 => self.ram[a as usize],
            0x2800..0x2900 => self.video.read_reg(a as usize & 0xff, self.line),
            0x2900..0x2a00 => self.video.scroll[a as usize & 0xff],
            0x2a00..0x2b00 => self.video.hcomp[a as usize & 0xff],
            0x2b00..0x2c00 => self.video.palette[a as usize & 0xff],
            0x2c00..0x3000 => self.video.sprites[a as usize & 0x3ff],
            0x3000..0x3800 => self.sound.read(a as usize - 0x3000),
            0x3d00..0x3e00 => self.io_read(a as usize & 0xff),
            0x3e00..0x3e04 => {
                if a == 0x3e02 { 0 } else { self.dma[a as usize & 3] }
            }
            0x3800..0x4000 => 0,
            _ => self.rom_read(a),
        }
    }

    pub fn write(&mut self, addr: u32, value: u16) {
        let a = addr & 0x3f_ffff;
        match a {
            0..0x2800 => self.ram[a as usize] = value,
            0x2800..0x2900 => {
                if let Some((src, dst, len)) = self.video.write_reg(a as usize & 0xff, value) {
                    for j in 0..len {
                        if dst + j < 0x400 {
                            self.video.sprites[dst + j] = self.peek(src + j as u32);
                        }
                    }
                    self.video.regs[0x72] = 0;
                    self.video.raise(video::IRQ_DMA);
                }
            }
            0x2900..0x2a00 => self.video.scroll[a as usize & 0xff] = value,
            0x2a00..0x2b00 => self.video.hcomp[a as usize & 0xff] = value,
            0x2b00..0x2c00 => self.video.palette[a as usize & 0xff] = value,
            0x2c00..0x3000 => self.video.sprites[a as usize & 0x3ff] = value,
            0x3000..0x3800 => self.sound.write(a as usize - 0x3000, value),
            0x3d00..0x3e00 => self.io_write(a as usize & 0xff, value),
            0x3e00..0x3e04 => self.dma_write(a as usize & 3, value),
            _ => {}
        }
    }

    fn dma_write(&mut self, reg: usize, value: u16) {
        if reg != 2 {
            self.dma[reg] = value;
            return;
        }
        if value & 0xc000 != 0 {
            return;
        }
        let len = value as u32;
        let mut src = ((self.dma[1] as u32 & 0x3f) << 16) | self.dma[0] as u32;
        let dst = self.dma[3] as u32 & 0x3fff;
        for j in 0..len {
            let v = self.read(src + j);
            self.write((dst + j) & 0x3fff, v);
        }
        src += len;
        self.dma[0] = src as u16;
        self.dma[1] = ((src >> 16) & 0x3f) as u16;
        self.dma[2] = 0;
        self.dma[3] = ((dst + len) & 0x3fff) as u16;
    }

    // ----- Interrupts -----

    /// The CPU's interrupt lines (bit 0 FIQ, bit 1 + n IRQ n).
    #[inline]
    pub fn lines(&self) -> u16 {
        let st = self.io[INT_STATUS] & self.io[INT_CTRL];
        let mut l = 0u16;
        if self.video.irq_pending() {
            l |= if self.fiq_vector == 0 { 1 } else { 1 << 1 };
        }
        if self.sound.channel_irq() {
            l |= 1;
        }
        if st & 0x0c00 != 0 {
            l |= 1 << 3; // IRQ2: timers A/B
        }
        if st & 0x6100 != 0 {
            l |= 1 << 4; // IRQ3: UART, SPI, ADC
        }
        if self.sound.irq() {
            l |= 1 << 5; // IRQ4
        }
        if st & 0x1200 != 0 {
            l |= 1 << 6; // IRQ5: external
        }
        if st & 0x0070 != 0 {
            l |= 1 << 7; // IRQ6: 4096/2048/1024 Hz
        }
        if st & 0x008b != 0 {
            l |= 1 << 8; // IRQ7: TMB1, TMB2, 4 Hz, key change
        }
        l
    }

    // ----- I/O -----

    fn port_in(&mut self, index: usize) -> u16 {
        match index {
            0 => 0x4000,
            1 => 0,
            _ => {
                let mut v = self.portc_toggle;
                self.portc_toggle ^= 0x0400;
                if self.sio_status_bits > 0 && self.sio_status & 0x8000 != 0 {
                    v |= 0x0800;
                }
                // SDA reads the line: high (pulled up) unless the master or
                // the EEPROM pulls it low.
                v | self.eeprom.sda(self.portc_level(0)) as u16
            }
        }
    }

    /// Updates port `index`'s data register from its buffer, direction and
    /// attribute registers and what drives the pins.
    fn gpio(&mut self, index: usize, write: bool) {
        let base = 5 * index;
        let buffer = self.io[base + 2];
        let dir = self.io[base + 3];
        let attr = self.io[base + 4];
        let special = self.io[base + 5];
        let (push, pull) = (dir, !dir);
        let mut what = buffer & (push | pull);
        what ^= dir & !attr;
        what &= !special;
        what &= !pull;
        if !write {
            what |= self.port_in(index) & pull;
        }
        self.io[base + 1] = what;
        if index == 2 {
            let (scl, sda) = (self.portc_level(1), self.portc_level(0));
            self.eeprom.update(scl, sda);
        }
    }

    /// The level port C drives on pin `bit`: its output if the pin is one,
    /// else high (the I2C lines' pull-ups).
    fn portc_level(&self, bit: u16) -> bool {
        let (buffer, dir, attr) = (self.io[12], self.io[13], self.io[14]);
        let m = 1 << bit;
        dir & m == 0 || (buffer ^ (dir & !attr)) & m != 0
    }

    fn clock_rng(&mut self, which: usize) -> u16 {
        let v = self.io[PRNG1 + which];
        self.io[PRNG1 + which] = ((v << 1) | (((v >> 14) ^ (v >> 13)) & 1)) & 0x7fff;
        v
    }

    fn io_read(&mut self, reg: usize) -> u16 {
        match reg {
            1 | 6 | 11 => {
                self.gpio((reg - 1) / 5, false);
                self.io[reg]
            }
            VERT_LINE => self.line as u16,
            NTSC_PAL => 0,
            PRNG1 => self.clock_rng(0),
            PRNG2 => self.clock_rng(1),
            DATA_SEGMENT => self.ds,
            UART_RXBUF => {
                if self.uart_rx_available {
                    self.io[UART_STATUS] &= !0x0081;
                    if let Some(b) = self.uart_rx_fifo.pop_front() {
                        self.io[UART_RXBUF] = b as u16;
                        if self.uart_rx_fifo.is_empty() {
                            self.uart_rx_available = false;
                        } else if self.uart_rx.next == u64::MAX {
                            self.start_uart_rx();
                        }
                    } else {
                        self.uart_rx_available = false;
                    }
                } else {
                    self.io[UART_RXFIFO] |= 0x2000;
                }
                self.io[UART_RXBUF]
            }
            SIO_DATA => {
                if self.io[SIO_STATUS] & 0x8000 != 0 && !self.sio_writing {
                    self.sio_bits = self.sio_bits.saturating_sub(1);
                    if self.sio_bits == 0 {
                        self.io[SIO_STATUS] &= !0x8000;
                    }
                }
                self.io[reg]
            }
            UART_RXFIFO => (self.io[reg] & !0x0070) | if self.uart_rx_available { 0x70 } else { 0 },
            _ => self.io[reg],
        }
    }

    fn uart_bits(&self) -> u64 {
        if self.io[UART_CTRL] & 0x20 != 0 { 11 } else { 10 }
    }

    fn start_uart_rx(&mut self) {
        self.uart_rx = Ticker::once(self.now, self.uart_bits() * CLOCK / self.uart_baud.max(1));
        self.update_next();
    }

    fn timer_rate(sel: u16) -> u64 {
        match sel & 7 {
            2 => 32768,
            3 => 8192,
            4 => 4096,
            _ => 0,
        }
    }

    fn update_timer_b(&mut self) {
        let hz = Self::timer_rate(self.io[TIMERB_CTRL]);
        self.timer_b_src = if hz == 0 { Ticker::OFF } else { Ticker::hz(self.now, hz) };
    }

    fn io_write(&mut self, reg: usize, data: u16) {
        match reg {
            1 | 6 | 11 => {
                self.io[reg + 1] = data;
                self.gpio((reg - 1) / 5, true);
            }
            2..=IOC_MASK => {
                let old = self.io[reg];
                // The sound chip's status, read by bit-banging port C
                // (0x3f0c90): latched when bit 11 turns to an input, a bit
                // out (the top first) at each rise of bit 12, the clock.
                if reg == 13 && old & 0x0800 != 0 && data & 0x0800 == 0 {
                    self.sio_status = self.sio_audio.status();
                    self.sio_status_bits = 16;
                } else if reg == 12 && old & 0x1000 == 0 && data & 0x1000 != 0 && self.sio_status_bits > 0 {
                    self.sio_status <<= 1;
                    self.sio_status_bits -= 1;
                }
                self.io[reg] = data;
                if (reg - 1) % 5 != 4 {
                    self.gpio((reg - 1) / 5, true);
                }
            }
            TIMEBASE_SETUP => {
                self.io[reg] = data;
                let hi = data & 0x10 != 0;
                let f1 = if hi { [12000, 24000, 40000, 40000] } else { [8, 16, 32, 64] }[(data & 3) as usize];
                let f2 = if hi { [105000, 210000, 420000, 840000] } else { [128, 256, 512, 1024] }[((data >> 2) & 3) as usize];
                self.tmb1 = Ticker::hz(self.now, f1);
                self.tmb2 = Ticker::hz(self.now, f2);
            }
            TIMEBASE_CLEAR => {
                self.div_2khz = 0;
                self.div_1khz = 0;
                self.div_4hz = 0;
            }
            TIMERA_DATA => {
                self.io[reg] = data;
                self.timer_a_preload = data;
            }
            TIMERA_CTRL => {
                self.io[reg] = data;
                let hz = Self::timer_rate(data);
                self.timer_a_src = if hz == 0 { Ticker::OFF } else { Ticker::hz(self.now, hz) };
                let hz = hz as u32;
                self.timer_ab_rate = match (data >> 3) & 7 {
                    0 => hz / 2048,
                    1 => hz / 1024,
                    2 => hz / 256,
                    4 => hz / 4,
                    5 => hz / 2,
                    6 => 1,
                    _ => 0,
                };
            }
            TIMERA_IRQCLR => self.io[INT_STATUS] &= !0x0800,
            TIMERB_DATA => {
                self.io[reg] = data;
                self.timer_b_preload = data;
            }
            TIMERB_CTRL => {
                self.io[reg] = data;
                if self.io[TIMERB_ON] == 1 {
                    self.update_timer_b();
                }
            }
            TIMERB_ON => {
                self.io[reg] = data & 1;
                if data & 1 != 0 {
                    self.update_timer_b();
                } else {
                    self.timer_b_src = Ticker::OFF;
                }
            }
            TIMERB_IRQCLR => self.io[INT_STATUS] &= !0x0400,
            INT_CTRL => self.io[reg] = data,
            INT_STATUS => {
                self.io[INT_STATUS] &= !data;
                if self.uart_rx_irq || self.uart_tx_irq {
                    self.io[INT_STATUS] |= 0x0100;
                }
            }
            ADC_CTRL => {
                let old = self.io[ADC_CTRL];
                self.io[ADC_CTRL] = data & !0x2000;
                if old & 0x2000 != 0 && data & 0x2000 != 0 {
                    self.io[INT_STATUS] &= !0x2000;
                }
                if self.io[ADC_CTRL] & 1 != 0 {
                    self.io[ADC_CTRL] |= 0x2000;
                    if old & 0x1000 == 0 && self.io[ADC_CTRL] & 0x1000 != 0 {
                        self.io[ADC_CTRL] &= !0x3000;
                        let clocks = 16u64 << ((self.io[ADC_CTRL] >> 2) & 3);
                        self.adc = Ticker::once(self.now, clocks);
                        self.io[ADC_DATA] &= !0x8000;
                    }
                    if data & 0x0400 != 0 {
                        self.io[ADC_DATA] &= !0x8000;
                        self.adc = Ticker::hz(self.now, 8000);
                    }
                } else {
                    self.adc = Ticker::OFF;
                }
            }
            PRNG1 | PRNG2 => self.io[reg] = data & 0x7fff,
            FIQ_SEL => {
                self.io[reg] = data;
                self.fiq_vector = (data & 7) as u8;
            }
            DATA_SEGMENT => {
                self.ds = data & 0x3f;
                self.ds_written = true;
            }
            UART_CTRL => {
                let changed = self.io[reg] ^ data;
                self.io[reg] = data;
                if data & 0x40 == 0 {
                    self.uart_rx_available = false;
                    self.io[UART_RXBUF] = 0;
                }
                if changed & 0x80 != 0 {
                    if data & 0x80 != 0 {
                        self.io[UART_STATUS] |= 0x0002;
                    } else {
                        self.io[UART_STATUS] &= !0x0042;
                        self.uart_tx = Ticker::OFF;
                    }
                }
            }
            UART_STATUS => {
                if data & 1 != 0 {
                    self.io[UART_STATUS] &= !1;
                    self.uart_rx_irq = false;
                }
                if data & 2 != 0 {
                    self.io[UART_STATUS] &= !2;
                    self.uart_tx_irq = false;
                }
                if !self.uart_rx_irq && !self.uart_tx_irq {
                    self.io[INT_STATUS] &= !0x0100;
                }
            }
            UART_BAUD1 => {
                // The SPG28x: one register, 27 MHz / (0x10000 - value).
                self.io[reg] = data;
                self.uart_baud = CLOCK / (0x10000 - data as u64);
            }
            UART_TXBUF => {
                self.io[reg] = data;
                if self.io[UART_CTRL] & 0x80 != 0 {
                    self.uart_tx = Ticker::once(self.now, self.uart_bits() * CLOCK / self.uart_baud.max(1));
                    self.io[UART_STATUS] &= !0x0002;
                    self.io[UART_STATUS] |= 0x0040;
                }
            }
            UART_RXBUF => {}
            UART_RXFIFO => {
                if data & 0x8000 != 0 {
                    self.uart_rx_available = false;
                    self.io[UART_RXBUF] = 0;
                }
                self.io[reg] &= !data & 0x6000;
                self.io[reg] = (self.io[reg] & !7) | (data & 7);
            }
            SIO_SETUP => {
                self.io[reg] = data;
                if data & 0x0400 != 0 {
                    self.sio_audio.command(self.io[0x52]);
                    self.io[SIO_STATUS] |= 0x8000;
                    self.sio_bits = if data & 0x80 != 0 { 16 } else { 8 };
                    self.sio_writing = data & 0x20 != 0;
                } else {
                    self.io[SIO_STATUS] &= !0x8000;
                }
            }
            SIO_DATA => {
                self.io[reg] = data;
                if self.io[SIO_SETUP] & 0x0400 != 0 {
                    self.sio_audio.data(data);
                }
                if self.io[SIO_SETUP] & 0x8000 != 0 && self.sio_writing {
                    self.sio_bits = self.sio_bits.saturating_sub(1);
                    if self.sio_bits == 0 {
                        self.io[SIO_STATUS] &= !0x8000;
                    }
                }
            }
            I2C_CMD => {
                // No EEPROM on the I2C unit: reads give 0; done at once.
                self.io[reg] = data;
                self.io[I2C_STATUS] |= 1;
            }
            I2C_STATUS => self.io[reg] &= !data,
            SYSTEM_CTRL => {
                self.io[reg] = data;
                self.powered_off |= data & 0x8000 != 0;
            }
            _ => self.io[reg] = data,
        }
        self.update_next();
    }

    /// A byte arriving on the UART's receive line.
    fn uart_receive(&mut self, byte: u8) {
        if self.io[UART_CTRL] & 0x40 == 0 {
            return;
        }
        self.uart_rx_fifo.push_back(byte);
        if self.uart_rx.next == u64::MAX {
            self.start_uart_rx();
        }
    }

    fn timer_a_inc(&mut self) {
        self.io[TIMERA_DATA] = self.io[TIMERA_DATA].wrapping_add(1);
        if self.io[TIMERA_DATA] == 0 {
            self.io[TIMERA_DATA] = self.timer_a_preload;
            self.io[INT_STATUS] |= 0x0800;
        }
    }

    // ----- Time -----

    /// Runs everything due by `now`. Returns true when a frame has
    /// finished (vertical blank started).
    pub fn service(&mut self) -> bool {
        let now = self.now;
        let mut frame_done = false;
        while self.line_end <= now {
            frame_done |= self.end_line();
        }
        while self.sys.due(now) {
            let st = &mut self.io[INT_STATUS];
            *st |= 0x0040;
            self.div_2khz += 1;
            if self.div_2khz == 2 {
                self.div_2khz = 0;
                *st |= 0x0020;
                self.div_1khz += 1;
                if self.div_1khz == 2 {
                    self.div_1khz = 0;
                    *st |= 0x0010;
                    self.div_4hz += 1;
                    if self.div_4hz == 256 {
                        self.div_4hz = 0;
                        *st |= 0x0008;
                    }
                }
            }
            self.sound.tick_4khz();
        }
        while self.timer_a_src.due(now) {
            if self.timer_ab_rate != 0 {
                self.timer_ab_divisor += 1;
                if self.timer_ab_divisor >= self.timer_ab_rate {
                    self.timer_ab_divisor = 0;
                    self.timer_a_inc();
                }
            }
        }
        while self.timer_b_src.due(now) {
            self.io[TIMERB_DATA] = self.io[TIMERB_DATA].wrapping_add(1);
            if self.io[TIMERB_DATA] == 0 {
                self.io[TIMERB_DATA] = self.timer_b_preload;
                self.io[INT_STATUS] |= 0x0400;
            }
        }
        while self.tmb1.due(now) {
            self.io[INT_STATUS] |= 1;
        }
        while self.tmb2.due(now) {
            self.io[INT_STATUS] |= 2;
        }
        while self.rng.due(now) {
            self.clock_rng(0);
            self.clock_rng(1);
        }
        if self.uart_tx.due(now) {
            self.uart_out.push(self.io[UART_TXBUF] as u8);
            self.io[UART_STATUS] |= 0x0002;
            self.io[UART_STATUS] &= !0x0040;
            if self.io[UART_CTRL] & 2 != 0 {
                self.io[INT_STATUS] |= 0x0100;
                self.uart_tx_irq = true;
            }
        }
        if self.uart_rx.due(now) {
            self.io[UART_STATUS] |= 0x81;
            self.uart_rx_available = true;
            if self.io[UART_CTRL] & 1 != 0 {
                self.uart_rx_irq = true;
                self.io[INT_STATUS] |= 0x0100;
            }
        }
        while self.adc.due(now) {
            self.io[ADC_DATA] = 0x0fff | 0x8000;
            self.io[ADC_CTRL] |= 0x2000;
            if self.io[ADC_CTRL] & 0x0200 != 0 {
                self.io[INT_STATUS] |= 0x2000;
            }
        }
        while self.audio.due(now) {
            let (ram, rom, cart) = (&self.ram, &self.rom, &self.cart);
            self.sound.tick(&|a| peek_parts(ram, rom, cart, a));
            // The SIO sound chip, mixed into both sides.
            let chip = self.sio_audio.tick();
            if chip != 0 {
                let n = self.sound.out.len();
                for s in &mut self.sound.out[n - 2..] {
                    *s = (*s as i32 + chip).clamp(-32768, 32767) as i16;
                }
            }
        }
        while self.input_feed.due(now) {
            if let Some(b) = self.input_bytes.pop_front() {
                self.uart_receive(b);
            }
        }
        self.update_next();
        frame_done
    }

    /// Finishes the current scanline (drawing it if visible) and starts the
    /// next. Returns true at the start of the vertical blank.
    fn end_line(&mut self) -> bool {
        let line = self.line;
        self.draw(line);
        self.line = (line + 1) % LINES;
        self.line_end += self.line_cycles(self.line);
        let mut frame = false;
        if self.line == SCREEN_H as u32 {
            self.video.raise(video::IRQ_VBLANK);
            self.frames += 1;
            frame = true;
        } else if self.line == 0 {
            self.video.regs[0x63] &= !video::IRQ_VBLANK;
        }
        let (v, h) = (self.video.regs[0x36] as u32, self.video.regs[0x37]);
        if v == self.line && h < 160 && v < 240 {
            self.video.raise(video::IRQ_POS);
        }
        frame
    }

    fn draw(&mut self, line: u32) {
        if line as usize >= SCREEN_H {
            return;
        }
        let (ram, rom, cart) = (&self.ram, &self.rom, &self.cart);
        self.video.draw_line(line, &|a| peek_parts(ram, rom, cart, a));
    }
}

/// Memory as the video unit and DMA see it (no side effects): RAM and ROM.
fn peek_parts(ram: &[u16], rom: &[u16], cart: &Option<Vec<u16>>, addr: u32) -> u16 {
    let a = addr as usize & 0x3f_ffff;
    match a {
        0..0x2800 => ram[a],
        0x2800..0x4000 => 0,
        _ => match cart {
            Some(c) if a < 0x20_0000 => c.get(a).copied().unwrap_or(0),
            _ => rom.get(a).copied().unwrap_or(0),
        },
    }
}

impl cpu::Bus for Bus {
    #[inline]
    fn read(&mut self, addr: u32) -> u16 {
        Bus::read(self, addr)
    }

    #[inline]
    fn write(&mut self, addr: u32, value: u16) {
        Bus::write(self, addr, value)
    }

    #[inline]
    fn lines(&self) -> u16 {
        Bus::lines(self)
    }
}
