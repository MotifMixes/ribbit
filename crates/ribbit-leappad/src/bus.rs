//! What the LeapPad's 8051 core sees: the bank windows into the physical
//! address space, the on-chip registers at XDATA 0xFC00-0xFCFF, on-chip RAM
//! and the devices behind them.
//!
//! The banking was read from the base ROM's own address helpers (155-00450
//! kernel at ROM 0x10000; docs/leappad.md, "Memory map"):
//!
//! | window | written as   | unit  | CPU addresses        |
//! |--------|--------------|-------|----------------------|
//! | 0      | FC06:FC07    | 32 KB | code 0x0000-0x7FFF   |
//! | 1      | FC08:FC09    | 32 KB | code 0x8000-0xFFFF   |
//! | 2      | FC0A:FC0B    | 32 KB | XDATA 0x0000-0x7FFF  |
//! | 3      | FC0C:FC0D    | 16 KB | XDATA 0x8000-0xBFFF  |
//! | 4      | FC0E:FC0F    | 16 KB | XDATA 0xC000-0xFFFF  |
//!
//! Reading FC06 gives the high bits of windows 0 and 1 packed (bits 0-1,
//! 2-3) and FC0A those of windows 2-4 (bits 0-1, 2-4, 5-7); the low bytes
//! read back where they were written. EVIDENCE (the ROM's read-back code).

use serde::{Deserialize, Serialize};

/// The rate of the FC01-FC04 counter: 1 Hz, a seconds counter. EVIDENCE:
/// the cartridge's idle power-off timeout, which the kernel adds to the count
/// (cart kernel 0x5A82, value from 0x548D), is 180 (00B4h), a round 3
/// minutes in seconds. (At 8 kHz the app powered itself off 0.8 s after
/// starting, at 8 Hz after 22 s.)
pub const COUNTER_HZ: u32 = 1;

/// Machine cycles the sound processor takes to answer a command
/// (HYPOTHESIS: 25 us at the stand-in clock).
pub const DSP_LATENCY: u32 = 100;

/// Where on-chip registers start in XDATA.
pub const IO_BASE: u16 = 0xFC00;

/// A memory or register access the model does not understand yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Unknown {
    pub pc: u16,
    pub what: String,
}

/// The size of the physical address space reached through the windows
/// (11-bit 16 KB banks).
pub const PHYS_SIZE: u32 = 1 << 25;

/// Physical address of the cartridge slot: chip select 5 (CS n at n * 4 MB).
/// EVIDENCE: the kernel reads the cartridge info block at 0x1410102.
pub const CART_BASE: u32 = 0x0140_0000;

#[derive(Serialize, Deserialize)]
pub struct Bus {
    /// The base ROM (chip select 0, physical 0).
    #[serde(skip)]
    pub bios: Vec<u8>,
    /// The cartridge ROM, if one is in.
    #[serde(skip)]
    pub cart: Option<Vec<u8>>,
    /// The five bank windows, as written (high byte, low byte).
    pub banks: [[u8; 2]; 5],
    /// The register page FC00-FCFF as last written (latches only).
    pub io: Vec<u8>,
    /// On-chip XDATA RAM, 0xF000-0xFFFF less the register page.
    pub xram: Vec<u8>,
    /// External RAM, if the physical map has any (UNKNOWN; sized and
    /// placed when the RAM check finds it).
    pub sram: Vec<u8>,
    /// The CPU's PC at the current instruction (for logs).
    pub pc: u16,
    /// Bytes the firmware sent on the serial port (its debug printf).
    #[serde(skip)]
    pub uart: Vec<u8>,
    /// Accesses not understood, first occurrence of each.
    #[serde(skip)]
    pub unknown: Vec<Unknown>,
    #[serde(skip)]
    seen: std::collections::HashSet<String>,
    /// Every register access, for research tools (off unless set).
    #[serde(skip)]
    pub trace_io: bool,
    #[serde(skip)]
    pub io_trace: Vec<(u16, u16, bool, u8)>,
    /// Every cartridge read, as (serial output so far, offset), for research
    /// tools (off unless set).
    #[serde(skip)]
    pub cart_reads: Option<Vec<(usize, u32)>>,
    /// Voices silenced for research tools (bit n: voice n).
    #[serde(skip)]
    pub mute: u8,
    /// FCE5: a down-counter the kernel's delay loop writes and polls until
    /// zero. EVIDENCE for the behaviour; the rate is UNKNOWN (one count per
    /// machine cycle here).
    pub delay: u8,
    pub pen: crate::pen::PenSensor,
    /// FC00: status bits the firmware clears by writing 1s (EVIDENCE: it
    /// writes 01, 08, 10 to acknowledge them). Bit 0: the FC10-FC15 unit
    /// finished; bit 3: the ON/OFF switch was pressed (the app then reports
    /// "Off Switch"); bit 4: the sound processor took a command (FC1C).
    pub status: u8,
    /// Machine cycles until the sound processor takes the last command.
    dsp_busy: u32,
    /// Set when it has: the machine pulses INT1.
    pub dsp_irq: bool,
    /// Sound processor commands written to FC1C (research log).
    #[serde(skip)]
    /// Each entry: (pc, command, the channel's parameters: start F462+4n,
    /// end/loop F482+4n, pitch F49E+2n, F4B8+2n, volume F4C2+2n, F4D2+2n).
    pub dsp_log: Option<Vec<(u16, u8, [u8; 16])>>,
    /// FC01-FC04: a 32-bit counter (FC01 most significant) that the kernel
    /// and app read as their clock, in units of the count >> 3 (both shift by
    /// 3: kernel 0x5B9A, app 0x8EE7). Writes load the byte written.
    /// EVIDENCE for a free-running counter at COUNTER_HZ (1 Hz), so the
    /// firmware's unit is 8 s.
    pub counter: u32,
    /// Counter phase, in units of 1 / MACHINE_HZ.
    counter_phase: u64,
    pub eeprom: crate::eeprom::Eeprom,
    /// The sound processor's voices (sound.rs); `sound.output` holds its
    /// 8 kHz mix.
    pub sound: ribbit_core::io::voices::VoiceEngine,
    /// The voices' releases after a key off (sound.rs).
    #[serde(default)]
    pub release: crate::sound::Release,
    /// Research: on-chip RAM writes inside these ranges are logged as
    /// (pc, code window 1 bank, address, value).
    #[serde(skip)]
    pub xwatch: Vec<(u16, u16)>,
    #[serde(skip)]
    pub xwatch_log: Vec<(u16, u16, u16, u8)>,
    /// Research: data reads (MOVX, MOVC; not instruction fetches) of
    /// physical addresses inside these ranges are logged as (pc, address,
    /// value).
    #[serde(skip)]
    pub pwatch: Vec<(u32, u32)>,
    #[serde(skip)]
    pub pwatch_log: Vec<(u16, u32, u8)>,
    /// Play LeapPad Classic cartridges although the base ROM's own list
    /// allows none (`classic_list`). On by default; off is the ILA2 base's
    /// own behaviour ("LeapPad Classic not supported on this base").
    pub classic: bool,
    /// Where the base ROM keeps its empty list of the Classic cartridges it
    /// plays (see `empty_classic_list`).
    #[serde(skip)]
    classic_list: Option<u32>,
}

/// The base ROM's list of the LeapPad Classic cartridges it accepts, when
/// it is empty. VERIFIED-FW (kernel 0xA1B6, ILA2 Universal base): the word at
/// +7Eh of the base's ROM Info Block (0x10100) points, in that bank, to a
/// table whose word at +9 points to the list, a run of words ended by FFFFh;
/// an entry 0001h accepts every Classic cartridge ("Cart OK", status 6),
/// none prints "LeapPad Classic not supported on this base" (status 5). The
/// ILA2 Universal base's list is empty, though the Classic cartridges carry
/// their own kernel and app and run on it once accepted (docs/leappad.md,
/// "LeapPad Classic cartridges").
fn empty_classic_list(bios: &[u8]) -> Option<u32> {
    let word = |a: u32| Some(u16::from_be_bytes([*bios.get(a as usize)?, *bios.get(a as usize + 1)?]));
    let in_bank = |p: u16| 0x1_0000 | (p as u32 & 0x7FFF);
    let table = in_bank(word(0x1_017E)?);
    let list = in_bank(word(table + 9)?);
    (word(list)? == 0xFFFF).then_some(list)
}

impl Bus {
    pub fn new(bios: Vec<u8>) -> Self {
        Bus {
            classic: true,
            classic_list: empty_classic_list(&bios),
            bios,
            cart: None,
            banks: [[0; 2]; 5],
            io: vec![0; 0x100],
            xram: vec![0; 0x1000],
            sram: vec![0; 0x2_0000],
            pc: 0,
            uart: Vec::new(),
            unknown: Vec::new(),
            seen: Default::default(),
            trace_io: false,
            io_trace: Vec::new(),
            cart_reads: None,
            mute: 0,
            delay: 0,
            pen: Default::default(),
            status: 0,
            dsp_busy: 0,
            dsp_irq: false,
            dsp_log: None,
            counter: 0,
            counter_phase: 0,
            eeprom: Default::default(),
            sound: crate::sound::engine(),
            release: Default::default(),
            xwatch: Vec::new(),
            xwatch_log: Vec::new(),
            pwatch: Vec::new(),
            pwatch_log: Vec::new(),
        }
    }

    /// Advances the on-chip timers by `cycles` machine cycles.
    pub fn tick(&mut self, cycles: u32) {
        self.delay = self.delay.saturating_sub(cycles.min(255) as u8);
        self.pen.tick(cycles);
        if self.dsp_busy > 0 {
            self.dsp_busy = self.dsp_busy.saturating_sub(cycles);
            if self.dsp_busy == 0 {
                self.status |= 0x10;
                self.dsp_irq = true;
            }
        }
        crate::sound::follow(&mut self.sound, &mut self.release, &self.xram, self.mute, cycles as f32 * 8000.0 / crate::MACHINE_HZ as f32);
        let mut rom = crate::sound::Rom { bios: &self.bios, cart: self.cart.as_deref() };
        self.sound.advance(cycles as u64, crate::MACHINE_HZ as u64, &mut rom);
        self.counter_phase += cycles as u64 * COUNTER_HZ as u64;
        let n = self.counter_phase / crate::MACHINE_HZ as u64;
        if n > 0 {
            self.counter_phase %= crate::MACHINE_HZ as u64;
            self.counter = self.counter.wrapping_add(n as u32);
        }
    }

    fn note(&mut self, what: String) {
        if self.seen.insert(what.clone()) {
            self.unknown.push(Unknown { pc: self.pc, what });
        }
    }

    /// The physical address window `w` maps CPU offset `off` to.
    pub fn window_phys(&self, w: usize, off: u16) -> u32 {
        let [hi, lo] = self.banks[w];
        let bank = u16::from_be_bytes([hi, lo]) as u32;
        match w {
            0 | 1 => ((bank & 0x3FF) << 15) | (off as u32 & 0x7FFF),
            2 => ((bank & 0x3FF) << 15) | (off as u32 & 0x7FFF),
            _ => ((bank & 0x7FF) << 14) | (off as u32 & 0x3FFF),
        }
    }

    pub fn phys_read(&mut self, addr: u32) -> u8 {
        let addr = addr % PHYS_SIZE;
        if addr < CART_BASE {
            if self.bios.is_empty() {
                return 0xFF;
            }
            // The list then holds the one entry 0001h.
            if self.classic
                && let Some(l) = self.classic_list
                && (l..=l + 1).contains(&addr)
            {
                return (addr - l) as u8;
            }
            return self.bios[addr as usize % self.bios.len()];
        }
        if (CART_BASE..CART_BASE + 0x40_0000).contains(&addr) {
            if let Some(r) = &mut self.cart_reads {
                r.push((self.uart.len(), addr - CART_BASE));
            }
            return match &self.cart {
                Some(c) if !c.is_empty() => c[(addr - CART_BASE) as usize % c.len()],
                _ => 0xFF,
            };
        }
        self.note(format!("read phys {addr:07X}"));
        0xFF
    }

    /// A byte of physical memory as the debugger shows it: the ROMs, without
    /// logging (None outside them).
    pub fn peek_phys(&self, addr: u32) -> Option<u8> {
        let addr = addr % PHYS_SIZE;
        if addr < CART_BASE {
            return (!self.bios.is_empty()).then(|| self.bios[addr as usize % self.bios.len()]);
        }
        match &self.cart {
            Some(c) if !c.is_empty() && addr < CART_BASE + 0x40_0000 => Some(c[(addr - CART_BASE) as usize % c.len()]),
            _ => None,
        }
    }

    /// A code byte through the bank windows, for the debugger's listing.
    pub fn code_peek(&self, addr: u16) -> u8 {
        self.peek_phys(self.window_phys((addr >> 15) as usize, addr)).unwrap_or(0xFF)
    }

    pub fn phys_write(&mut self, addr: u32, v: u8) {
        self.note(format!("write phys {:07X} = {v:02X}", addr % PHYS_SIZE));
    }

    /// The EEPROM bus lines as the port drives them: an output bit (FC33)
    /// gives its latch (FC1F), an input is pulled up.
    fn eeprom_lines(&self) -> (bool, bool) {
        let (dir, latch) = (self.io[0x33], self.io[0x1F]);
        let line = |bit: u8| dir & bit == 0 || latch & bit != 0;
        (line(0x04), line(0x08))
    }

    fn io_read(&mut self, reg: u8) -> u8 {
        let v = match reg {
            0x06 => (self.banks[0][0] & 3) | ((self.banks[1][0] & 3) << 2),
            0x0A => (self.banks[2][0] & 3) | ((self.banks[3][0] & 7) << 2) | ((self.banks[4][0] & 7) << 5),
            0x07..=0x0F => self.banks[(reg as usize - 6) / 2][(reg as usize) & 1],
            0xE5 => self.delay,
            0x00 => self.status,
            0x01..=0x04 => self.counter.to_be_bytes()[reg as usize - 1],
            // Bit 6: a cartridge is in (the kernel's cartridge probe). EVIDENCE.
            // Bits 1 and 2: pen sensor result waiting / queue has room.
            0x2A => (self.io[0x2A] & !0x46) | if self.cart.is_some() { 0x40 } else { 0 } | self.pen.status(),
            // Bit 0: a cartridge is in, as an input pin of the port whose
            // latch is FC3F. The 2003 Canada Full Base's probe (kernel 0x6020)
            // reports none without it; the ILA2 base only uses FC2A. EVIDENCE.
            0x2F => (self.io[0x2F] & !1) | self.cart.is_some() as u8,
            0x18 => self.pen.result_high(),
            // Bits 2 and 3: the EEPROM bus clock and data line levels.
            0x1F => {
                let (scl, sda) = self.eeprom_lines();
                (self.io[0x1F] & !0x0C) | (scl as u8) << 2 | ((sda && self.eeprom.sda_released()) as u8) << 3
            }
            0x19 => self.pen.result_low(),
            0x1E => crate::sound::busy(&self.sound),
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
        match reg {
            0x06..=0x0F => self.banks[(reg as usize - 6) / 2][(reg as usize) & 1] = v,
            0xE5 => self.delay = v,
            0x00 => self.status &= !v,
            0x1C => {
                // A sound processor command (sound.rs). It is taken a little later
                // (latency HYPOTHESIS), which sets FC00 bit 4 and raises
                // INT1: EVIDENCE, as the kernel's T1 tick stops raising EX1
                // in software while a command is pending (cart kernel
                // 0x5D2C) and only the EX1 handler (0x5E62) clears it.
                if let Some(l) = &mut self.dsp_log {
                    let n = v as usize & 7;
                    let mut a = [0; 16];
                    for (i, (base, len)) in [(0x462, 4), (0x482, 4), (0x49E, 2), (0x4B8, 2), (0x4C2, 2), (0x4D2, 2)].into_iter().enumerate() {
                        let at = base + len * n;
                        let o = [0, 4, 8, 10, 12, 14][i];
                        a[o..o + len].copy_from_slice(&self.xram[at..at + len]);
                    }
                    l.push((self.pc, v, a));
                }
                self.dsp_busy = DSP_LATENCY;
                let mut rom = crate::sound::Rom { bios: &self.bios, cart: self.cart.as_deref() };
                crate::sound::command(&mut self.sound, &mut self.release, v, &self.xram, &mut rom);
            }
            0x01..=0x04 => {
                let mut b = self.counter.to_be_bytes();
                b[reg as usize - 1] = v;
                self.counter = u32::from_be_bytes(b);
            }
            0x17 => self.pen.queue_code(v),
            0x16 => self.pen.control(v),
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

impl Bus {
    fn watch_read(&mut self, p: u32, v: u8) {
        if self.pwatch.iter().any(|&(lo, hi)| (lo..=hi).contains(&p)) {
            self.pwatch_log.push((self.pc, p, v));
        }
    }
}

impl crate::mcs51::Bus for Bus {
    fn code(&mut self, addr: u16) -> u8 {
        let p = self.window_phys((addr >> 15) as usize, addr);
        let v = self.phys_read(p);
        // An instruction's own bytes are fetches; anything else is MOVC.
        if !self.pwatch.is_empty() && addr.wrapping_sub(self.pc) > 3 {
            self.watch_read(p, v);
        }
        v
    }

    fn xread(&mut self, addr: u16) -> u8 {
        match addr {
            0xFC00..=0xFCFF => self.io_read(addr as u8),
            0xF000..=0xFFFF => self.xram[(addr - 0xF000) as usize],
            _ => {
                let p = self.window_phys(xwindow(addr), addr);
                let v = self.phys_read(p);
                self.watch_read(p, v);
                v
            }
        }
    }

    fn xwrite(&mut self, addr: u16, v: u8) {
        match addr {
            0xFC00..=0xFCFF => self.io_write(addr as u8, v),
            0xF000..=0xFFFF => {
                if self.xwatch.iter().any(|&(lo, hi)| (lo..=hi).contains(&addr)) {
                    let bank = u16::from_be_bytes(self.banks[1]);
                    self.xwatch_log.push((self.pc, bank, addr, v));
                }
                self.xram[(addr - 0xF000) as usize] = v
            }
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

/// The data window an XDATA address below 0xF000 falls in.
fn xwindow(addr: u16) -> usize {
    match addr {
        0x0000..=0x7FFF => 2,
        0x8000..=0xBFFF => 3,
        _ => 4,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A base ROM whose Classic list (at 0x17FFE) is `list`.
    fn base(list: [u8; 2]) -> Vec<u8> {
        let mut b = vec![0xFF; 0x2_0000];
        b[0x1_017E..0x1_0180].copy_from_slice(&[0x7F, 0x55]);
        b[0x1_7F5E..0x1_7F60].copy_from_slice(&[0x7F, 0xFE]);
        b[0x1_7FFE..0x1_8000].copy_from_slice(&list);
        b
    }

    #[test]
    fn an_empty_classic_list_accepts_classic_cartridges_unless_turned_off() {
        let mut bus = Bus::new(base([0xFF, 0xFF]));
        assert_eq!([bus.phys_read(0x1_7FFE), bus.phys_read(0x1_7FFF)], [0x00, 0x01]);
        bus.classic = false;
        assert_eq!([bus.phys_read(0x1_7FFE), bus.phys_read(0x1_7FFF)], [0xFF, 0xFF], "the base's own list");
        // A base that lists its own Classic cartridges is left as it is.
        let mut bus = Bus::new(base([0x00, 0x07]));
        assert_eq!([bus.phys_read(0x1_7FFE), bus.phys_read(0x1_7FFF)], [0x00, 0x07]);
    }
}
