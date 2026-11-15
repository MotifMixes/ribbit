//! What the 8051 core sees on the iQuest and the Turbo Twist Extreme. Their
//! chip is the LeapPad's (the same boot stub, bank windows and register page
//! FC00-FCFF; docs/leappad.md, "Memory map"), so the windows and most
//! registers follow ribbit-leappad's bus. What differs is in docs/iquest.md.

use serde::{Deserialize, Serialize};

pub const PHYS_SIZE: u32 = 1 << 25;
/// Chip select 5: the cartridge (as on the LeapPad). iQuest cartridges are
/// 8 MB and go on through chip select 6 (their pointer tables address
/// 0x14xxxxx-0x17xxxxx: EVIDENCE).
pub const CART_BASE: u32 = 0x0140_0000;
pub const CART_SPAN: u32 = 0x80_0000;
/// Chip select 7: on-chip RAM. Window 4 is pointed here by the kernel, and
/// the firmware clears C268-C4FF through it (EVIDENCE).
pub const RAM_BASE: u32 = 0x01C0_0000;
pub const RAM_SIZE: usize = 0x4000;
/// Chip select 4: the iQuest's own flash (flash.rs).
pub const FLASH_BASE: u32 = 0x0100_0000;

/// The view a base ROM starts in when its memory was lost: the shop demo
/// ("Try Me"), which on both models plays, says goodbye and powers off, and
/// starts again at the next power-on until the user's state has been
/// saved, which only the menu's apps do (iQuest: Options writes "User state"
/// to flash; EVIDENCE from the logs, 2026-11-15). How a real unit leaves it
/// is UNKNOWN (docs/iquest.md, open questions); Ribbit can read the base's
/// default view as its menu instead, as the LeapPad's Classic list is
/// opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TryMe {
    /// Physical address of the default view's byte.
    pub at: u32,
    /// The menu's view number, read there instead.
    pub menu: u8,
}

impl TryMe {
    /// VERIFIED-FW: found by tracing the view look-up after "AppRAMInit"
    /// (iQuest base 152-10663: the base resource at 0x130000, +21h = 0Fh
    /// "Try Me", +24h = 08h "Menu"; TTExtreme 152-10696: 0x7000E = 08h
    /// "TryMe", view 1 is "Menu", checked by patching it to every value).
    pub fn of(bios: &[u8]) -> Option<TryMe> {
        let at = |a: usize| bios.get(a).copied();
        if at(0x13_0021) == Some(0x0F) && at(0x13_0024) == Some(0x08) && crate::identify(bios) == Some(crate::Model::IQuest) {
            return Some(TryMe { at: 0x13_0021, menu: 0x08 });
        }
        if at(0x7_000E) == Some(0x08) && crate::identify(bios) == Some(crate::Model::TurboTwistExtreme) {
            return Some(TryMe { at: 0x7_000E, menu: 0x01 });
        }
        None
    }
}

/// A memory or register access the model does not understand yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Unknown {
    pub pc: u16,
    pub what: String,
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    #[serde(skip)]
    pub bios: Vec<u8>,
    #[serde(skip)]
    pub cart: Option<Vec<u8>>,
    pub banks: [[u8; 2]; 5],
    pub io: Vec<u8>,
    /// On-chip RAM, physical RAM_BASE; XDATA F000-FFFF is its last 4 KB.
    pub ram: Vec<u8>,
    pub pc: u16,
    #[serde(skip)]
    pub uart: Vec<u8>,
    #[serde(skip)]
    pub unknown: Vec<Unknown>,
    #[serde(skip)]
    seen: std::collections::HashSet<String>,
    #[serde(skip)]
    pub trace_io: bool,
    #[serde(skip)]
    pub io_trace: Vec<(u16, u16, bool, u8)>,
    pub delay: u8,
    pub status: u8,
    pub counter: u32,
    counter_phase: u64,
    pub eeprom: ribbit_leappad::eeprom::Eeprom,
    pub e2: crate::e2::FastE2,
    /// Start in the menu instead of the shop demo (see `TryMe`). On by
    /// default.
    pub skip_try_me: bool,
    #[serde(skip)]
    pub try_me: Option<TryMe>,
    /// The keys held down (gpio.rs).
    #[serde(skip)]
    pub pressed: Vec<crate::gpio::Wire>,
    /// The iQuest's flash at chip select 4 (none on the Turbo Twist Extreme).
    pub flash: Option<crate::flash::Flash>,
    /// The sound processor: the LeapPad's (ribbit-leappad sound.rs), the
    /// Leapster's voice engine driven through on-chip RAM and FC1C.
    pub sound: ribbit_core::io::voices::VoiceEngine,
    pub release: ribbit_leappad::sound::Release,
    dsp_busy: u32,
    /// The sound processor took a command: the machine pulses INT1.
    pub dsp_irq: bool,
    #[serde(skip)]
    pub mute: u8,
    /// Research: data reads (MOVX, MOVC) of physical addresses in this
    /// range are logged as (pc, address, value).
    #[serde(skip)]
    pub pwatch: Option<(u32, u32)>,
    #[serde(skip)]
    pub pwatch_log: Vec<(u16, u32, u8)>,
}

/// Machine cycles the sound processor takes to answer a command (the
/// LeapPad's HYPOTHESIS).
pub const DSP_LATENCY: u32 = 100;

impl Bus {
    pub fn new(bios: Vec<u8>) -> Self {
        let try_me = TryMe::of(&bios);
        Bus {
            try_me,
            bios,
            cart: None,
            banks: [[0; 2]; 5],
            io: vec![0; 0x100],
            ram: vec![0; RAM_SIZE],
            pc: 0,
            uart: Vec::new(),
            unknown: Vec::new(),
            seen: Default::default(),
            trace_io: false,
            io_trace: Vec::new(),
            delay: 0,
            status: 0,
            counter: 0,
            counter_phase: 0,
            eeprom: Default::default(),
            e2: Default::default(),
            flash: None,
            pressed: Vec::new(),
            skip_try_me: true,
            sound: ribbit_leappad::sound::engine(),
            release: Default::default(),
            dsp_busy: 0,
            dsp_irq: false,
            mute: 0,
            pwatch: None,
            pwatch_log: Vec::new(),
        }
    }

    pub fn tick(&mut self, cycles: u32, hz: u32) {
        self.delay = self.delay.saturating_sub(cycles.min(255) as u8);
        self.counter_phase += cycles as u64;
        if self.counter_phase >= hz as u64 {
            self.counter_phase -= hz as u64;
            self.counter = self.counter.wrapping_add(1);
        }
        if self.dsp_busy > 0 {
            self.dsp_busy = self.dsp_busy.saturating_sub(cycles);
            if self.dsp_busy == 0 {
                self.status |= 0x10;
                self.dsp_irq = true;
            }
        }
        let xram = &self.ram[RAM_SIZE - 0x1000..];
        ribbit_leappad::sound::follow(&mut self.sound, &mut self.release, xram, self.mute, cycles as f32 * 8000.0 / hz as f32);
        let mut rom = ribbit_leappad::sound::Rom { bios: &self.bios, cart: self.cart.as_deref() };
        self.sound.advance(cycles as u64, hz as u64, &mut rom);
    }

    pub fn note(&mut self, what: String) {
        if self.seen.insert(what.clone()) {
            self.unknown.push(Unknown { pc: self.pc, what });
        }
    }

    pub fn window_phys(&self, w: usize, off: u16) -> u32 {
        let bank = u16::from_be_bytes(self.banks[w]) as u32;
        match w {
            0..=2 => ((bank & 0x3FF) << 15) | (off as u32 & 0x7FFF),
            _ => ((bank & 0x7FF) << 14) | (off as u32 & 0x3FFF),
        }
    }

    pub fn peek_phys(&self, addr: u32) -> Option<u8> {
        let addr = addr % PHYS_SIZE;
        if addr < 0x40_0000 {
            return (!self.bios.is_empty()).then(|| self.bios[addr as usize % self.bios.len()]);
        }
        if (CART_BASE..CART_BASE + CART_SPAN).contains(&addr) {
            return match &self.cart {
                Some(c) if !c.is_empty() => Some(c[(addr - CART_BASE) as usize % c.len()]),
                _ => None,
            };
        }
        if (RAM_BASE..RAM_BASE + RAM_SIZE as u32).contains(&addr) {
            return Some(self.ram[(addr - RAM_BASE) as usize]);
        }
        if let Some(f) = &self.flash
            && (FLASH_BASE..CART_BASE).contains(&addr)
        {
            return Some(f.read(addr - FLASH_BASE));
        }
        None
    }

    pub fn phys_read(&mut self, addr: u32) -> u8 {
        let addr = addr % PHYS_SIZE;
        if self.skip_try_me
            && let Some(t) = self.try_me
            && addr == t.at
        {
            return t.menu;
        }
        match self.peek_phys(addr) {
            Some(v) => v,
            None => {
                if !(CART_BASE..CART_BASE + CART_SPAN).contains(&addr) {
                    self.note(format!("read phys {addr:07X}"));
                }
                0xFF
            }
        }
    }

    pub fn phys_write(&mut self, addr: u32, v: u8) {
        let addr = addr % PHYS_SIZE;
        if (RAM_BASE..RAM_BASE + RAM_SIZE as u32).contains(&addr) {
            self.ram[(addr - RAM_BASE) as usize] = v;
            return;
        }
        if let Some(f) = &mut self.flash
            && (FLASH_BASE..CART_BASE).contains(&addr)
        {
            f.write(addr - FLASH_BASE, v);
            return;
        }
        self.note(format!("write phys {addr:07X} = {v:02X}"));
    }

    pub fn code_peek(&self, addr: u16) -> u8 {
        self.peek_phys(self.window_phys((addr >> 15) as usize, addr)).unwrap_or(0xFF)
    }

    fn eeprom_lines(&self) -> (bool, bool) {
        let (dir, latch) = (self.io[0x33], self.io[0x1F]);
        let line = |bit: u8| dir & bit == 0 || latch & bit != 0;
        (line(0x04), line(0x08))
    }

    fn io_read(&mut self, reg: u8) -> u8 {
        if let Some(v) = self.e2.read(reg, &mut self.status) {
            return v;
        }
        let v = match reg {
            0x06 => (self.banks[0][0] & 3) | ((self.banks[1][0] & 3) << 2),
            0x0A => (self.banks[2][0] & 3) | ((self.banks[3][0] & 7) << 2) | ((self.banks[4][0] & 7) << 5),
            0x07..=0x0F => self.banks[(reg as usize - 6) / 2][(reg as usize) & 1],
            0xE5 => self.delay,
            0x00 => self.status,
            0x01..=0x04 => self.counter.to_be_bytes()[reg as usize - 1],
            0x2A => (self.io[0x2A] & !0x40) | if self.cart.is_some() { 0x40 } else { 0 },
            0x2F => (self.io[0x2F] & !1) | self.cart.is_some() as u8,
            0x1E => ribbit_leappad::sound::busy(&self.sound),
            // Port A, with the serial EEPROM's data line on bit 3 (the
            // LeapPad's wiring; the Turbo Twist Extreme's kernel still
            // drives it).
            0x1F => crate::gpio::read(&self.io, crate::gpio::Port::A, &self.pressed) & !(((!self.eeprom.sda_released()) as u8) << 3),
            0x48 | 0x49 => crate::gpio::read(&self.io, crate::gpio::Port::of_data_reg(reg).unwrap(), &self.pressed),
            _ => {
                self.note(format!("read FC{reg:02X}"));
                self.io[reg as usize]
            }
        };
        if self.trace_io {
            self.io_trace.push((self.pc, 0xFC00 | reg as u16, false, v));
        }
        v
    }

    fn io_write(&mut self, reg: u8, v: u8) {
        if self.trace_io {
            self.io_trace.push((self.pc, 0xFC00 | reg as u16, true, v));
        }
        if self.e2.write(reg, v, &mut self.status) {
            self.io[reg as usize] = v;
            return;
        }
        match reg {
            0x06..=0x0F => self.banks[(reg as usize - 6) / 2][(reg as usize) & 1] = v,
            0xE5 => self.delay = v,
            0x00 => self.status &= !v,
            0x1C => {
                self.dsp_busy = DSP_LATENCY;
                let xram = &self.ram[RAM_SIZE - 0x1000..];
                let mut rom = ribbit_leappad::sound::Rom { bios: &self.bios, cart: self.cart.as_deref() };
                ribbit_leappad::sound::command(&mut self.sound, &mut self.release, v, xram, &mut rom);
            }
            0x01..=0x04 => {
                let mut b = self.counter.to_be_bytes();
                b[reg as usize - 1] = v;
                self.counter = u32::from_be_bytes(b);
            }
            // The LCD controller (lcd.rs).
            0xE1..=0xE4 => {}
            // Ports B and C and their directions (gpio.rs).
            0x48..=0x4B => {}
            0x1F | 0x33 => {
                self.io[reg as usize] = v;
                let (scl, sda) = self.eeprom_lines();
                self.eeprom.lines(scl, sda);
            }
            _ => self.note(format!("write FC{reg:02X} = {v:02X}")),
        }
        self.io[reg as usize] = v;
    }
}

impl ribbit_leappad::mcs51::Bus for Bus {
    fn code(&mut self, addr: u16) -> u8 {
        let p = self.window_phys((addr >> 15) as usize, addr);
        let v = self.phys_read(p);
        if addr.wrapping_sub(self.pc) > 3 {
            self.watch(p, v);
        }
        v
    }

    fn xread(&mut self, addr: u16) -> u8 {
        match addr {
            0xFC00..=0xFCFF => self.io_read(addr as u8),
            0xF000..=0xFFFF => self.ram[(addr as usize - 0xF000) + RAM_SIZE - 0x1000],
            _ => {
                let p = self.window_phys(xwindow(addr), addr);
                let v = self.phys_read(p);
                self.watch(p, v);
                v
            }
        }
    }

    fn xwrite(&mut self, addr: u16, v: u8) {
        match addr {
            0xFC00..=0xFCFF => self.io_write(addr as u8, v),
            0xF000..=0xFFFF => self.ram[(addr as usize - 0xF000) + RAM_SIZE - 0x1000] = v,
            _ => {
                let p = self.window_phys(xwindow(addr), addr);
                self.phys_write(p, v)
            }
        }
    }

    fn sfr_read(&mut self, addr: u8) -> u8 {
        self.note(format!("read SFR {addr:02X}"));
        0
    }

    fn sfr_write(&mut self, addr: u8, v: u8) {
        self.note(format!("write SFR {addr:02X} = {v:02X}"));
    }

    fn serial_tx(&mut self, byte: u8) {
        self.uart.push(byte);
    }
}

impl Bus {
    fn watch(&mut self, p: u32, v: u8) {
        if let Some((lo, hi)) = self.pwatch
            && (lo..=hi).contains(&p)
        {
            self.pwatch_log.push((self.pc, p, v));
        }
    }
}

fn xwindow(addr: u16) -> usize {
    match addr {
        0x0000..=0x7FFF => 2,
        0x8000..=0xBFFF => 3,
        _ => 4,
    }
}
