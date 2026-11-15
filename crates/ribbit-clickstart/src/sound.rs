//! The SPG2xx sound unit (word addresses 0x3000-0x37ff): 16 channels
//! playing 8-bit, 16-bit or 4-bit ADPCM samples from memory, each with an
//! envelope, volume and pan, mixed at 281.25 kHz / 4. Channel registers are
//! at 0x3000 + 16 n, phase (pitch) registers at 0x3200 + 16 n, control
//! registers at 0x3400. Follows MAME's spg2xx_audio (BSD-3-Clause; see
//! THIRD-PARTY-NOTICES.txt), except that ADPCM is scaled to the full 16-bit
//! range (MAME mixes the 12-bit decoder output as is). Every other format
//! fills the 16-bit wave data register (8-bit as b<<8|b, ADPCM36 as a
//! nibble at the top of a word), and the BIOS and games play everything,
//! speech included, on OKI ADPCM channels: unscaled, the BIOS music would
//! peak at -46 dBFS (scaled: -22). See docs/clickstart.md.

use serde::{Deserialize, Serialize};

/// Output samples a second (stereo).
pub const OUTPUT_RATE: f64 = 281_250.0 / 4.0;
/// CPU cycles per output sample (27 MHz / 70312.5 Hz).
pub const CYCLES_PER_SAMPLE: u64 = 384;

// Channel registers.
const WAVE_ADDR: usize = 0x0;
const MODE: usize = 0x1;
const LOOP_ADDR: usize = 0x2;
const PAN_VOL: usize = 0x3;
const ENVELOPE0: usize = 0x4;
const ENVELOPE_DATA: usize = 0x5;
const ENVELOPE1: usize = 0x6;
const ENVELOPE_ADDR_HIGH: usize = 0x7;
const ENVELOPE_ADDR: usize = 0x8;
const WAVE_DATA_PREV: usize = 0x9;
const ENVELOPE_LOOP_CTRL: usize = 0xa;
const WAVE_DATA: usize = 0xb;
const ADPCM_SEL: usize = 0xd;

// Phase registers.
const PHASE_HIGH: usize = 0x0;
const RAMP_DOWN_CLOCK: usize = 0x3;
const PHASE: usize = 0x4;

// Control registers.
const CHANNEL_ENABLE: usize = 0x00;
const MAIN_VOLUME: usize = 0x01;
const FIQ_ENABLE: usize = 0x02;
const FIQ_STATUS: usize = 0x03;
const BEAT_BASE_COUNT: usize = 0x04;
const BEAT_COUNT: usize = 0x05;
const ENVCLK0: usize = 0x06;
const ENVCLK0_HIGH: usize = 0x07;
const ENVCLK1: usize = 0x08;
const ENVCLK1_HIGH: usize = 0x09;
const ENV_RAMP_DOWN: usize = 0x0a;
const CHANNEL_STOP: usize = 0x0b;
const CONTROL: usize = 0x0d;
const CHANNEL_STATUS: usize = 0x0f;
const WAVE_IN_L: usize = 0x10;
const WAVE_IN_R: usize = 0x11;
const CHANNEL_REPEAT: usize = 0x14;
const CHANNEL_ENV_MODE: usize = 0x15;
const CHANNEL_TONE_RELEASE: usize = 0x16;
const CHANNEL_ENV_IRQ: usize = 0x17;

const BIS: u16 = 0x4000;
const BIE: u16 = 0x8000;

const ENV_FRAMES: [u32; 16] = [4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 8192, 8192, 8192, 8192];
const RAMP_FRAMES: [u32; 8] = [13 * 4, 13 * 16, 13 * 64, 13 * 256, 13 * 1024, 13 * 4096, 13 * 8192, 13 * 8192];

/// OKI (Dialogic) ADPCM step sizes and index moves.
const OKI_STEPS: [i32; 49] = [
    16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209,
    230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552,
];
const OKI_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
struct Channel {
    /// Output-rate accumulator (samples to move on) and the channel's rate
    /// in Hz.
    accum: f64,
    rate: f64,
    shift: u32,
    env_addr: u32,
    env_frame: u32,
    ramp_frame: u32,
    /// OKI ADPCM state.
    signal: i32,
    step: i32,
    /// ADPCM36: header, samples left in the block, the last two samples.
    a36_header: u16,
    a36_left: u32,
    a36_prev: i32,
    /// The channel's FIQ clock (when its FIQ is enabled at start).
    irq_on: bool,
    irq_accum: f64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Sound {
    regs: Vec<u16>,
    phase: Vec<u16>,
    ctrl: Vec<u16>,
    ch: [Channel; 16],
    beat_base: u16,
    /// Output, stereo interleaved, at OUTPUT_RATE.
    #[serde(skip)]
    pub out: Vec<i16>,
    /// Research: channel starts are logged here when on.
    #[serde(skip)]
    pub trace: Option<Vec<String>>,
}

impl Sound {
    pub fn new() -> Self {
        let mut s = Sound {
            regs: vec![0; 0x200],
            phase: vec![0; 0x200],
            ctrl: vec![0; 0x400],
            ch: [Channel::default(); 16],
            beat_base: 0,
            out: Vec::new(),
            trace: None,
        };
        s.ctrl[CHANNEL_REPEAT] = 0x3f;
        s.ctrl[CHANNEL_ENV_MODE] = 0x3f;
        for c in &mut s.ch {
            c.env_frame = 4;
        }
        s
    }

    #[inline]
    fn reg(&self, ch: usize, r: usize) -> u16 {
        self.regs[ch << 4 | r]
    }

    #[inline]
    fn set_reg(&mut self, ch: usize, r: usize, v: u16) {
        self.regs[ch << 4 | r] = v;
    }

    fn wave_addr(&self, ch: usize) -> u32 {
        ((self.reg(ch, MODE) & 0x3f) as u32) << 16 | self.reg(ch, WAVE_ADDR) as u32
    }

    fn set_wave_addr(&mut self, ch: usize, a: u32) {
        self.set_reg(ch, WAVE_ADDR, a as u16);
        let m = self.reg(ch, MODE);
        self.set_reg(ch, MODE, (m & !0x3f) | ((a >> 16) & 0x3f) as u16);
    }

    fn loop_addr(&self, ch: usize) -> u32 {
        (((self.reg(ch, MODE) >> 6) & 0x3f) as u32) << 16 | self.reg(ch, LOOP_ADDR) as u32
    }

    fn envelope_addr(&self, ch: usize) -> u32 {
        ((self.reg(ch, ENVELOPE_ADDR_HIGH) & 0x3f) as u32) << 16 | self.reg(ch, ENVELOPE_ADDR) as u32
    }

    fn rate(&self, ch: usize) -> f64 {
        let phase = ((self.phase[ch << 4 | PHASE_HIGH] & 7) as u32) << 16 | self.phase[ch << 4 | PHASE] as u32;
        phase as f64 * 140625.0 * 2.0 / (1 << 19) as f64
    }

    fn adpcm(&self, ch: usize) -> bool {
        self.reg(ch, MODE) & 0x8000 != 0
    }

    fn adpcm36(&self, ch: usize) -> bool {
        self.reg(ch, ADPCM_SEL) & 0x8000 != 0
    }

    fn edd(&self, ch: usize) -> u16 {
        self.reg(ch, ENVELOPE_DATA) & 0x7f
    }

    fn set_edd(&mut self, ch: usize, edd: u16) {
        let v = self.reg(ch, ENVELOPE_DATA);
        self.set_reg(ch, ENVELOPE_DATA, (v & !0x7f) | (edd & 0x7f));
    }

    fn envelope_count(&self, ch: usize) -> u16 {
        self.reg(ch, ENVELOPE_DATA) >> 8
    }

    fn set_envelope_count(&mut self, ch: usize, count: u16) {
        let edd = self.edd(ch);
        self.set_reg(ch, ENVELOPE_DATA, edd | (count << 8));
    }

    fn envelope_load(&self, ch: usize) -> u16 {
        self.reg(ch, ENVELOPE1) & 0xff
    }

    fn env_frames(&self, ch: usize) -> u32 {
        let (reg, sh) = match ch {
            0..4 => (ENVCLK0, ch),
            4..8 => (ENVCLK0_HIGH, ch - 4),
            8..12 => (ENVCLK1, ch - 8),
            _ => (ENVCLK1_HIGH, ch - 12),
        };
        ENV_FRAMES[((self.ctrl[reg] >> (sh * 4)) & 0xf) as usize]
    }

    fn ramp_frames(&self, ch: usize) -> u32 {
        RAMP_FRAMES[(self.phase[ch << 4 | RAMP_DOWN_CLOCK] & 7) as usize]
    }

    pub fn read(&mut self, offset: usize) -> u16 {
        match offset {
            0x000..0x200 => self.regs[offset],
            0x200..0x400 => self.phase[offset - 0x200],
            _ => self.ctrl[offset - 0x400],
        }
    }

    pub fn write(&mut self, offset: usize, data: u16) {
        match offset {
            0x000..0x200 => {
                self.regs[offset] = match offset & 0xf {
                    PAN_VOL => data & 0x7f7f,
                    ENVELOPE_DATA => data & 0xff7f,
                    ADPCM_SEL => data & 0xfe00,
                    _ => data,
                }
            }
            0x200..0x400 => {
                let o = offset - 0x200;
                let ch = (o >> 4) & 0xf;
                self.phase[o] = match o & 0xf {
                    0..4 => data & 7,
                    _ => data,
                };
                if matches!(o & 0xf, PHASE_HIGH | PHASE) && o < 0x100 {
                    self.ch[ch].rate = self.rate(ch);
                    self.ch[ch].accum = 0.0;
                }
            }
            _ => self.ctrl_write(offset - 0x400, data),
        }
    }

    fn ctrl_write(&mut self, reg: usize, data: u16) {
        match reg {
            CHANNEL_ENABLE => {
                let changed = self.ctrl[reg] ^ data;
                self.ctrl[reg] = data;
                for ch in 0..16 {
                    let mask = 1 << ch;
                    if changed & mask == 0 {
                        continue;
                    }
                    if data & mask != 0 {
                        if self.ctrl[CHANNEL_STOP] & mask == 0 && self.ctrl[CHANNEL_STATUS] & mask == 0 {
                            self.start(ch);
                        }
                    } else if self.ctrl[CHANNEL_STATUS] & mask != 0 {
                        self.stop(ch);
                    }
                }
            }
            MAIN_VOLUME => self.ctrl[reg] = data & 0x7f,
            FIQ_STATUS => self.ctrl[reg] &= !data,
            BEAT_BASE_COUNT => {
                self.ctrl[reg] = data & 0x07ff;
                self.beat_base = self.ctrl[reg];
            }
            BEAT_COUNT => {
                let v = (self.ctrl[reg] & !(data & BIS)) & BIS;
                self.ctrl[reg] = v | (data & !BIS);
            }
            ENVCLK0 | ENVCLK0_HIGH | ENVCLK1 | ENVCLK1_HIGH => {
                let changed = self.ctrl[reg] ^ data;
                self.ctrl[reg] = data;
                let first = match reg {
                    ENVCLK0 => 0,
                    ENVCLK0_HIGH => 4,
                    ENVCLK1 => 8,
                    _ => 12,
                };
                for i in 0..4 {
                    if changed & (0xf << (4 * i)) != 0 {
                        // MAME looks the new count up for channel i of the
                        // low register even for the high ones; kept.
                        let lookup = if reg == ENVCLK0_HIGH || reg == ENVCLK1_HIGH { first - 4 + i } else { first + i };
                        self.ch[first + i].env_frame = self.env_frames(lookup);
                    }
                }
            }
            ENV_RAMP_DOWN => {
                let old = self.ctrl[reg];
                self.ctrl[reg] = data & self.ctrl[CHANNEL_STATUS];
                let changed = old ^ self.ctrl[reg];
                for ch in 0..16 {
                    let mask = 1 << ch;
                    if changed & mask != 0 && data & mask != 0 {
                        self.ch[ch].ramp_frame = self.ramp_frames(ch);
                    }
                }
            }
            CHANNEL_STOP => {
                let old = self.ctrl[reg];
                self.ctrl[reg] &= !data;
                let changed = old ^ self.ctrl[reg];
                for ch in 0..16 {
                    let mask = 1 << ch;
                    if changed & mask != 0
                        && self.ctrl[CHANNEL_ENABLE] & mask != 0
                        && self.ctrl[CHANNEL_STATUS] & mask == 0
                    {
                        self.start(ch);
                    }
                }
            }
            CONTROL => self.ctrl[reg] = data & 0x9fe8,
            CHANNEL_STATUS => {}
            CHANNEL_ENV_IRQ => self.ctrl[reg] &= !data,
            _ => self.ctrl[reg] = data,
        }
    }

    fn start(&mut self, ch: usize) {
        let rate = self.rate(ch);
        if self.trace.is_some() {
            let line = format!(
                "ch {ch:2} mode {:04x} addr {:06x} loop {:06x} vol {:04x} rate {rate:.0} fiq {} adpcm36 {} ctrl {:04x} main {:02x}",
                self.reg(ch, MODE), self.wave_addr(ch), self.loop_addr(ch), self.reg(ch, PAN_VOL),
                self.ctrl[FIQ_ENABLE] >> ch & 1, self.adpcm36(ch) as u8, self.ctrl[CONTROL], self.ctrl[MAIN_VOLUME]
            );
            self.trace.as_mut().unwrap().push(line);
        }
        self.ctrl[CHANNEL_STATUS] |= 1 << ch;
        let env_addr = self.envelope_addr(ch);
        let load = self.envelope_load(ch);
        self.set_envelope_count(ch, load);
        let c = &mut self.ch[ch];
        c.rate = rate;
        c.irq_on = self.ctrl[FIQ_ENABLE] & (1 << ch) != 0;
        c.irq_accum = 0.0;
        c.env_addr = env_addr;
        c.signal = 0;
        c.step = 0;
        c.shift = 0;
        c.a36_header = 0;
        c.a36_left = 0;
        c.a36_prev = 0;
    }

    fn stop(&mut self, ch: usize) {
        let mask = !(1u16 << ch);
        self.ctrl[CHANNEL_STATUS] &= mask;
        let m = self.reg(ch, MODE);
        self.set_reg(ch, MODE, m & !0x8000);
        self.ctrl[CHANNEL_TONE_RELEASE] &= mask;
        self.ctrl[ENV_RAMP_DOWN] &= mask;
        self.ch[ch].irq_on = false;
    }

    /// The FIQ from channels (any channel FIQ status bit).
    pub fn channel_irq(&self) -> bool {
        self.ctrl[FIQ_STATUS] != 0
    }

    /// The beat interrupt (IRQ4).
    pub fn irq(&self) -> bool {
        self.ctrl[BEAT_COUNT] & (BIS | BIE) == BIS | BIE
    }

    /// The system timer's 4096 Hz tick (nothing here).
    pub fn tick_4khz(&mut self) {}

    fn oki(&mut self, ch: usize, n: u16) -> i32 {
        let c = &mut self.ch[ch];
        let n = n as i32 & 15;
        let step = OKI_STEPS[c.step as usize];
        let mut diff = step >> 3;
        if n & 1 != 0 {
            diff += step >> 2;
        }
        if n & 2 != 0 {
            diff += step >> 1;
        }
        if n & 4 != 0 {
            diff += step;
        }
        c.signal = (c.signal + if n & 8 != 0 { -diff } else { diff }).clamp(-2048, 2047);
        c.step = (c.step + OKI_INDEX[(n & 7) as usize]).clamp(0, 48);
        c.signal << 4
    }

    fn adpcm36_nibble(&mut self, ch: usize, n: u16) -> i32 {
        let c = &mut self.ch[ch];
        let shift = (c.a36_header & 0xf) as i32;
        let filter = ((c.a36_header & 0x3f0) >> 4) as i16;
        let f0 = (if filter & 0x20 != 0 { filter | !0x3f } else { filter }) as i32;
        let s = ((n << 12) as i16 as i32 >> shift) + ((c.a36_prev * f0 + 32) >> 12);
        let s = s as i16 as i32;
        c.a36_prev = s;
        s
    }

    /// Fetches a channel's next sample into its wave data register.
    /// Returns false when a one-shot ends.
    fn fetch(&mut self, ch: usize, mem: &dyn Fn(u32) -> u16) -> bool {
        let prev = self.reg(ch, WAVE_DATA);
        self.set_reg(ch, WAVE_DATA_PREV, prev);
        let tone = (self.reg(ch, MODE) >> 12) & 3;
        if self.adpcm36(ch) && tone != 0 && self.ch[ch].a36_left == 0 {
            self.ch[ch].a36_header = mem(self.wave_addr(ch));
            self.ch[ch].a36_left = 8;
            let a = self.wave_addr(ch) + 1;
            self.set_wave_addr(ch, a);
        }
        let raw = if tone != 0 { mem(self.wave_addr(ch)) } else { self.reg(ch, WAVE_DATA) };
        if self.adpcm(ch) || self.adpcm36(ch) {
            if tone != 0 && raw == 0xffff {
                if tone == 1 {
                    self.ctrl[CHANNEL_STOP] |= 1 << ch;
                    self.stop(ch);
                    return false;
                }
                self.do_loop(ch);
                let m = self.reg(ch, MODE);
                self.set_reg(ch, MODE, m & !0x8000);
            } else {
                let nib = (raw >> self.ch[ch].shift) & 0xf;
                let s = if self.adpcm36(ch) { self.adpcm36_nibble(ch, nib) } else { self.oki(ch, nib) };
                self.set_reg(ch, WAVE_DATA, (s as u16) ^ 0x8000);
            }
        } else if self.reg(ch, MODE) & 0x4000 != 0 {
            if tone != 0 && raw == 0xffff {
                if tone == 1 {
                    self.stop(ch);
                    return false;
                }
                self.do_loop(ch);
            } else {
                self.set_reg(ch, WAVE_DATA, raw);
            }
        } else if tone != 0 {
            let mut r = if self.ch[ch].shift != 0 { raw & 0xff00 } else { raw << 8 };
            r |= r >> 8;
            if r == 0xffff {
                if tone == 1 {
                    self.ctrl[CHANNEL_STOP] |= 1 << ch;
                    self.stop(ch);
                    return false;
                }
                self.do_loop(ch);
            } else {
                self.set_reg(ch, WAVE_DATA, r);
            }
        }
        true
    }

    fn do_loop(&mut self, ch: usize) {
        let a = self.loop_addr(ch);
        self.set_wave_addr(ch, a);
        self.ch[ch].shift = 0;
    }

    fn advance(&mut self, ch: usize, mem: &dyn Fn(u32) -> u16) -> bool {
        self.ch[ch].accum += self.ch[ch].rate;
        let mut n = 0;
        while self.ch[ch].accum >= OUTPUT_RATE {
            self.ch[ch].accum -= OUTPUT_RATE;
            n += 1;
        }
        for _ in 0..n {
            if !self.fetch(ch, mem) {
                return false;
            }
            if self.adpcm(ch) || self.adpcm36(ch) {
                self.ch[ch].shift += 4;
                if self.ch[ch].shift >= 16 {
                    self.ch[ch].shift = 0;
                    let a = self.wave_addr(ch) + 1;
                    self.set_wave_addr(ch, a);
                    if self.adpcm36(ch) {
                        self.ch[ch].a36_left = self.ch[ch].a36_left.saturating_sub(1);
                    }
                }
            } else if self.reg(ch, MODE) & 0x4000 != 0 {
                let a = self.wave_addr(ch) + 1;
                self.set_wave_addr(ch, a);
            } else {
                self.ch[ch].shift += 8;
                if self.ch[ch].shift >= 16 {
                    self.ch[ch].shift = 0;
                    let a = self.wave_addr(ch) + 1;
                    self.set_wave_addr(ch, a);
                }
            }
        }
        true
    }

    fn rampdown(&mut self, ch: usize) {
        let old = self.edd(ch);
        let off = self.reg(ch, ENVELOPE_LOOP_CTRL) >> 9;
        let new = old.saturating_sub(off);
        if new != 0 {
            self.set_edd(ch, new);
            self.ch[ch].ramp_frame = self.ramp_frames(ch);
        } else {
            self.ctrl[CHANNEL_STOP] |= 1 << ch;
            self.stop(ch);
        }
    }

    fn envelope(&mut self, ch: usize, mem: &dyn Fn(u32) -> u16) {
        let mut count = self.envelope_count(ch);
        let cur = self.edd(ch);
        if count > 0 {
            count -= 1;
            self.set_envelope_count(ch, count);
        }
        if count != 0 {
            return;
        }
        let e0 = self.reg(ch, ENVELOPE0);
        let target = (e0 >> 8) & 0x7f;
        let inc = e0 & 0x7f;
        let mut new = cur;
        if new != target {
            if e0 & 0x80 != 0 {
                new = if inc > cur { 0 } else { cur - inc };
                if new != 0 && new < target {
                    new = target;
                }
                if new == 0 {
                    self.ctrl[CHANNEL_STOP] |= 1 << ch;
                    self.stop(ch);
                    return;
                }
            } else {
                new = (new + inc).min(target);
            }
        }
        if new == target {
            let e1 = self.reg(ch, ENVELOPE1);
            if e1 & 0x100 != 0 {
                let repeat = (e1 >> 9).wrapping_sub(1) & 0x7f;
                if repeat == 0 {
                    let ea = self.ch[ch].env_addr;
                    let (a, b, c) = (mem(ea), mem(ea + 1), mem(ea + 2));
                    self.set_reg(ch, ENVELOPE0, a);
                    self.set_reg(ch, ENVELOPE1, b);
                    self.set_reg(ch, ENVELOPE_LOOP_CTRL, c);
                    self.ch[ch].env_addr = self.envelope_addr(ch) + (c & 0x1ff) as u32;
                } else {
                    self.set_reg(ch, ENVELOPE1, (e1 & 0x1ff) | repeat << 9);
                }
            } else {
                let ea = self.ch[ch].env_addr;
                let (a, b) = (mem(ea), mem(ea + 1));
                self.set_reg(ch, ENVELOPE0, a);
                self.set_reg(ch, ENVELOPE1, b);
                self.ch[ch].env_addr = ea + 2;
            }
        }
        let load = self.envelope_load(ch);
        self.set_envelope_count(ch, load);
        self.set_edd(ch, new);
    }

    /// One output sample (and a beat counter tick).
    pub fn tick(&mut self, mem: &dyn Fn(u32) -> u16) {
        self.beat_tick();
        let (mut l, mut r) = (0i32, 0i32);
        let interpolate = self.ctrl[CONTROL] & 0x0200 == 0;
        for ch in 0..16 {
            let mask = 1u16 << ch;
            if self.ctrl[CHANNEL_STATUS] & mask == 0 {
                continue;
            }
            if self.ch[ch].irq_on && self.ch[ch].rate > 0.0 {
                self.ch[ch].irq_accum += self.ch[ch].rate;
                if self.ch[ch].irq_accum >= OUTPUT_RATE {
                    self.ch[ch].irq_accum %= OUTPUT_RATE;
                    self.ctrl[FIQ_STATUS] |= mask;
                }
            }
            if !self.advance(ch, mem) {
                continue;
            }
            let mut s = (self.reg(ch, WAVE_DATA) ^ 0x8000) as i16 as i32;
            if interpolate {
                let p = (self.reg(ch, WAVE_DATA_PREV) ^ 0x8000) as i16 as i32;
                let f = (self.ch[ch].accum / OUTPUT_RATE * 256.0) as i32;
                s = ((p * (0x100 - f)) >> 8) + ((s * f) >> 8);
            }
            s = (s * self.edd(ch) as i32) >> 7;
            let pv = self.reg(ch, PAN_VOL);
            let (vol, pan) = ((pv & 0x7f) as i32, ((pv >> 8) & 0x7f) as i32);
            let (pl, pr) = if pan < 0x40 { (0x7f * vol, pan * 2 * vol) } else { ((0x7f - pan) * 2 * vol, 0x7f * vol) };
            l += (s as i16 as i32 * pl as i16 as i32) >> 14;
            r += (s as i16 as i32 * pr as i16 as i32) >> 14;
            if self.ctrl[ENV_RAMP_DOWN] & mask != 0 {
                if self.ch[ch].ramp_frame > 0 {
                    self.ch[ch].ramp_frame -= 1;
                }
                if self.ch[ch].ramp_frame == 0 {
                    self.rampdown(ch);
                }
            } else if self.ctrl[CHANNEL_ENV_MODE] & mask == 0 {
                if self.ch[ch].env_frame > 0 {
                    self.ch[ch].env_frame -= 1;
                }
                if self.ch[ch].env_frame == 0 {
                    self.envelope(ch, mem);
                    self.ch[ch].env_frame = self.env_frames(ch);
                }
            }
        }
        if self.ctrl[WAVE_IN_L] != 0 {
            l += self.ctrl[WAVE_IN_L] as i32 - 0x8000;
        }
        if self.ctrl[WAVE_IN_R] != 0 {
            r += self.ctrl[WAVE_IN_R] as i32 - 0x8000;
        }
        let shift = if (self.ctrl[CONTROL] >> 6) & 3 == 0 { 4 } else { 2 };
        let main = self.ctrl[MAIN_VOLUME] as i32;
        let lf = ((l >> shift) * main) >> 7;
        let rf = ((r >> shift) * main) >> 7;
        self.out.push(lf.clamp(-32768, 32767) as i16);
        self.out.push(rf.clamp(-32768, 32767) as i16);
    }

    fn beat_tick(&mut self) {
        if self.beat_base > 0 {
            self.beat_base -= 1;
        }
        if self.beat_base == 0 {
            self.beat_base = self.ctrl[BEAT_BASE_COUNT];
            let mut count = self.ctrl[BEAT_COUNT] & 0x3fff;
            if count > 0 {
                count -= 1;
                self.ctrl[BEAT_COUNT] = (self.ctrl[BEAT_COUNT] & !0x3fff) | count;
            }
            if count == 0 && self.ctrl[BEAT_COUNT] & BIE != 0 {
                self.ctrl[BEAT_COUNT] |= BIS;
            }
        }
    }
}

impl Default for Sound {
    fn default() -> Self {
        Sound::new()
    }
}
