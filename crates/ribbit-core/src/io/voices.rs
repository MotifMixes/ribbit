//! Audio voice engine: the eight legacy voices, their status register and the
//! LFC speech decoder (voice 7).
//!
//! SOURCED from MAME `src/mame/leapfrog/leapster_a.cpp` (master da56c5ac,
//! BSD-3; behaviour reimplemented, no code copied), UNVERIFIED on hardware.
//! MAME presents this as researched rather than guessed (its LFC decoder is
//! reported to decode 93,721 streams and align sample-exactly with source
//! recordings); "fixed-point arithmetic, saturation and exact hardware timing
//! remain unverified". VERIFIED-FW: the register addresses the BIOSes write
//! (e.g. voice 6 start 0x0180_40F4/F8, end 0x0180_4134/38, volume 0x0180_4154)
//! fall exactly where MAME's map puts them.
//!
//! | Item | Model |
//! |---|---|
//! | Voices 0-4 | pitched A-law; address = (start & 0xFFFF_0000) \| u16(position); position starts at start's low half and advances by pitch / 0x8000 per sample; at end (end >> 16) it loops to the loop point (end & 0xFFFF) or stops if end - loop < 2 |
//! | Voices 5-6 | raw A-law at 8 kHz from start to end (absolute addresses) |
//! | Voice 7 | LFC speech: 16-bit mode word (0-2 = Level4/6/8), records of control + optional reflection-coefficient word + excitation of the previous record, 192 samples each; 256 start-up samples discarded; control 0x0FC0 ends the stream |
//! | 0x0180_2078 | playing voices: voice 0 in bit 7, voices 1-7 in bits 0-6 ("not sure why it's out of place" - MAME) |
//! | Sample clock | 8 kHz (MAME's legacy mix rate) |
//! | A-law | standard G.711 (`ribbit_audio::alaw_to_linear`); MAME's table is the same curve at half amplitude and opposite sign |
//! | Mix | sum of voice * 0.125 * volume / 0x4000, clamped to 16 bits (MAME) |
//!
//! Not modelled: PCM DMA (0x0180_3000; no BIOS or SpongeBob access seen), the
//! 32 kHz interpolation, analog output, the other 0x0180_4000 registers.

use ribbit_memory::Bus;

pub const BASE: u32 = 0x0180_4000;
pub const STATUS_ADDR: u32 = 0x0180_2078;
pub const CODEBOOK_LO: u32 = 0x0180_20E0;
pub const CODEBOOK_HI: u32 = 0x0180_20E4;
pub const SAMPLE_HZ: u64 = 8_000;
pub const START_COMMAND: u8 = 0x10;

const LFC_FRAME: usize = 192;
const LFC_SKIP: u32 = 256;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Lfc {
    pub pointer: u32,
    pub mode: u16,
    flags: u16,
    position: usize,
    skip: u32,
    end: u8,
    coeff: [[f64; 10]; 2],
    history: [f64; 10],
    #[serde(with = "ribbit_memory::serde_array")]
    samples: [f64; LFC_FRAME],
    pub frames: u64,
}

impl Default for Lfc {
    fn default() -> Self {
        Self {
            pointer: 0,
            mode: 0,
            flags: 0,
            position: LFC_FRAME,
            skip: LFC_SKIP,
            end: 0,
            coeff: [[0.0; 10]; 2],
            history: [0.0; 10],
            samples: [0.0; LFC_FRAME],
            frames: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VoiceEngine {
    pub start: [u32; 8],
    pub end: [u32; 8],
    pub volume: [u16; 8],
    pub pitch: [u16; 5],
    pub triggered: [bool; 8],
    dx: [f32; 8],
    index: [f32; 8],
    pub codebook_page: u32,
    pub lfc: Lfc,
    /// A start command for voice 7 waits for memory access (done by the
    /// machine right after the instruction).
    pub lfc_start_pending: bool,
    /// Fractional sample-clock state, in peripheral ticks x SAMPLE_HZ.
    phase: u64,
    pub samples_generated: u64,
    /// Mixed output at 8 kHz, for the host audio backend (not saved).
    #[serde(skip, default = "default_output")]
    pub output: ribbit_audio::SampleRing,
}

fn default_output() -> ribbit_audio::SampleRing {
    ribbit_audio::SampleRing::new(8_000)
}

impl Default for VoiceEngine {
    fn default() -> Self {
        Self {
            start: [0; 8],
            end: [0; 8],
            volume: [0; 8],
            pitch: [0; 5],
            triggered: [false; 8],
            dx: [0.0; 8],
            index: [0.0; 8],
            codebook_page: 0,
            lfc: Lfc::default(),
            lfc_start_pending: false,
            phase: 0,
            samples_generated: 0,
            output: default_output(),
        }
    }
}

impl VoiceEngine {
    /// 0x0180_2078: playing voices (voice 0 in bit 7, voices 1-7 in bits 0-6).
    pub fn status(&self) -> u32 {
        let mut s = (self.triggered[0] as u32) << 7;
        for v in 1..8 {
            s |= (self.triggered[v] as u32) << (v - 1);
        }
        s
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        (addr == STATUS_ADDR).then(|| self.status())
    }

    /// Register writes (16-bit values; the BIOS uses `sth`).
    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        let v = value as u16;
        match addr {
            CODEBOOK_LO => self.codebook_page = (self.codebook_page & 0xFFFF_0000) | v as u32,
            CODEBOOK_HI => self.codebook_page = (self.codebook_page & 0x0000_FFFF) | (v as u32) << 16,
            _ => {
                let Some(off) = addr.checked_sub(BASE).filter(|&o| o < 0x1000) else {
                    return false;
                };
                match off {
                    0x0C4..=0x13B => {
                        let (base, regs) = if off < 0x104 { (0x0C4, &mut self.start) } else { (0x104, &mut self.end) };
                        let ch = ((off - base) / 8) as usize;
                        match ((off - base) / 2) & 3 {
                            0 => regs[ch] = (regs[ch] & 0x0000_FFFF) | (v as u32) << 16,
                            2 => regs[ch] = (regs[ch] & 0xFFFF_0000) | v as u32,
                            _ => {}
                        }
                    }
                    0x13C..=0x15B if ((off - 0x13C) / 2) & 1 == 0 => self.volume[((off - 0x13C) / 4) as usize] = v,
                    0x15C..=0x16F if ((off - 0x15C) / 2) & 1 == 0 => self.pitch[((off - 0x15C) / 4) as usize] = v,
                    _ => return false,
                }
            }
        }
        true
    }

    /// A voice command (0x0180_2070): command = byte >> 3, voice = byte & 7.
    pub fn command(&mut self, command: u8, voice: u8) {
        let v = voice as usize & 7;
        if command != START_COMMAND {
            self.triggered[v] = false;
            if v == 7 {
                self.lfc_start_pending = false;
            }
            return;
        }
        match v {
            0..=4 => {
                self.dx[v] = self.pitch[v] as f32 / 32768.0;
                self.index[v] = (self.start[v] & 0xFFFF) as f32;
                self.triggered[v] = true;
            }
            5 | 6 => {
                self.dx[v] = 1.0;
                self.index[v] = 0.0;
                self.triggered[v] = true;
            }
            _ => self.lfc_start_pending = true,
        }
    }

    fn lfc_word(&mut self, bus: &mut impl Bus) -> u16 {
        let w = bus.read16(self.lfc.pointer);
        self.lfc.pointer = self.lfc.pointer.wrapping_add(2);
        w
    }

    fn lfc_table(&self, bus: &mut impl Bus, offset: u32) -> f64 {
        bus.read16(((self.codebook_page << 15) | 0x900).wrapping_add(offset)) as i16 as f64 / 32768.0
    }

    /// Starts voice 7 (needs the stream's mode word from memory).
    pub fn start_lfc(&mut self, bus: &mut impl Bus) {
        self.lfc_start_pending = false;
        self.lfc = Lfc { pointer: self.start[7], ..Default::default() };
        self.lfc.mode = self.lfc_word(bus);
        self.triggered[7] = self.lfc.mode <= 2 && self.codebook_page != 0;
    }

    fn lfc_excitation(&mut self, bus: &mut impl Bus, at: usize, voiced: bool) {
        let mut code = self.lfc_word(bus) as u64;
        if !voiced {
            let gain = 32768.0 * self.lfc_table(bus, 0x840 + 2 * ((code >> 8) & 0x1F) as u32);
            let sign = if code & (1 << 13) != 0 { -1.0 } else { 1.0 };
            let base = 0xF00 + 64 * (code & 0xFF) as u32;
            for i in 0..32 {
                self.lfc.samples[at + i] = gain * sign * self.lfc_table(bus, base + 2 * i as u32);
            }
            return;
        }
        let mode = self.lfc.mode as usize;
        for i in 1..2 + mode {
            code |= (self.lfc_word(bus) as u64) << (16 * i);
        }
        const POSITIONS: [&[u32]; 3] =
            [&[12, 20, 24, 28], &[11, 22, 27, 32, 37, 42], &[11, 22, 27, 32, 37, 42, 48, 53]];
        const SIGNS: [&[u32]; 3] = [&[16, 17, 18, 19], &[16, 17, 18, 19, 20, 21], &[16, 17, 18, 19, 20, 21, 47, 63]];
        const SHAPES: [u32; 3] = [0x000, 0x180, 0x400];
        let count = 4 + 2 * mode;
        let gain = 32768.0 * self.lfc_table(bus, 0x780 + 0x40 * mode as u32 + 2 * (code & 0x1F) as u32);
        let shape = SHAPES[mode] + 2 * (count as u32 - 1) * ((code >> 5) & 0x3F) as u32;
        let width = if mode == 0 { 4 } else { 5 };
        for i in 0..count {
            let field = (code >> POSITIONS[mode][i]) & ((1 << width) - 1);
            let position = field as usize * if mode == 0 { 2 } else { 1 };
            let amplitude = gain * if i == 0 { 1.0 } else { self.lfc_table(bus, shape + 2 * (i as u32 - 1)) };
            let negative = (code >> SIGNS[mode][i]) & 1 != 0;
            if let Some(s) = self.lfc.samples.get_mut(at + position) {
                *s += if negative { -amplitude } else { amplitude };
            }
        }
    }

    fn lfc_synth(&mut self) {
        // Step-up recursion: reflection coefficients to predictor taps.
        let mut a = [0.0f64; 10];
        for order in 0..10 {
            let k = self.lfc.coeff[0][order];
            let old = a;
            for i in 0..order {
                a[i] = old[i] - k * old[order - 1 - i];
            }
            a[order] = k;
        }
        for i in 0..LFC_FRAME {
            let mut value = self.lfc.samples[i];
            for (j, tap) in a.iter().enumerate() {
                value += tap * self.lfc.history[j];
            }
            self.lfc.history.copy_within(0..9, 1);
            self.lfc.history[0] = value;
            self.lfc.samples[i] = value.clamp(-32768.0, 32767.0);
        }
    }

    /// Decodes the next 192-sample frame; false when the stream has ended.
    fn lfc_frame(&mut self, bus: &mut impl Bus) -> bool {
        if self.lfc.end == 2 {
            return false;
        }
        self.lfc.samples = [0.0; LFC_FRAME];
        self.lfc.coeff[0] = self.lfc.coeff[1];
        if self.lfc.end == 1 {
            self.lfc.end = 2; // drain the filter after the terminal record
        } else {
            let flags = self.lfc_word(bus);
            if flags & 0xF000 != 0 {
                return false;
            }
            if flags & 0x3F != 0 {
                let mut code = self.lfc_word(bus) as u32;
                code |= (self.lfc_word(bus) as u32) << 16;
                const BITS: [u32; 10] = [5, 4, 4, 3, 3, 3, 3, 3, 2, 2];
                let mut table = 0x880;
                for (i, &b) in BITS.iter().enumerate() {
                    self.lfc.coeff[1][i] = self.lfc_table(bus, table + 2 * (code & ((1 << b) - 1)));
                    code >>= b;
                    table += 2 << b;
                }
            }
            // The previous record's flags describe the excitation stored now.
            let prev = self.lfc.flags;
            for i in 0..6 {
                if prev & (1 << (5 - i)) != 0 {
                    self.lfc_excitation(bus, 32 * i, prev & (1 << (11 - i)) != 0);
                }
            }
            self.lfc.flags = flags;
            if flags == 0x0FC0 {
                self.lfc.end = 1;
            }
        }
        self.lfc_synth();
        self.lfc.position = 0;
        self.lfc.frames += 1;
        true
    }

    /// One 8 kHz output sample (MAME's legacy mix).
    fn sample(&mut self, bus: &mut impl Bus) -> i16 {
        let mut mix = 0.0f64;
        for v in 0..7 {
            if !self.triggered[v] {
                continue;
            }
            let start = self.start[v];
            let pos = self.index[v];
            let addr =
                if v < 5 { (start & 0xFFFF_0000) | pos as u32 as u16 as u32 } else { start.wrapping_add(pos as u32) };
            let s = ribbit_audio::alaw_to_linear(bus.read8(addr)) as f64;
            mix += s * 0.125 * self.volume[v] as f64 / 16384.0;
            self.index[v] += self.dx[v];
            if v < 5 {
                let (end, lp) = ((self.end[v] >> 16) as u16, self.end[v] as u16);
                if self.index[v] >= end as f32 {
                    if (end as i32) - (lp as i32) < 2 {
                        self.triggered[v] = false;
                    } else {
                        self.index[v] = self.index[v] - end as f32 + lp as f32;
                    }
                }
            } else if start.wrapping_add(self.index[v] as u32) >= self.end[v] {
                self.triggered[v] = false;
            }
        }
        while self.triggered[7] {
            if self.lfc.position == LFC_FRAME && !self.lfc_frame(bus) {
                self.triggered[7] = false;
                break;
            }
            let s = self.lfc.samples[self.lfc.position];
            self.lfc.position += 1;
            if self.lfc.skip > 0 {
                self.lfc.skip -= 1;
            } else {
                mix += s * 0.125 * self.volume[7] as f64 / 16384.0;
                break;
            }
        }
        mix.clamp(-32768.0, 32767.0) as i16
    }

    pub fn active(&self) -> bool {
        self.lfc_start_pending || self.triggered.iter().any(|&t| t)
    }

    /// Advances by `ticks` peripheral ticks at `peripheral_hz`, producing
    /// one sample per 1/8000 s. Exact over any split of `ticks`.
    pub fn advance(&mut self, ticks: u64, peripheral_hz: u64, bus: &mut impl Bus) {
        self.phase += ticks * SAMPLE_HZ;
        if !self.active() {
            // Idle: keep the clock phase; avoid a division on every step.
            if self.phase >= peripheral_hz {
                self.phase %= peripheral_hz;
            }
            return;
        }
        while self.phase >= peripheral_hz {
            self.phase -= peripheral_hz;
            if !self.active() {
                self.phase %= peripheral_hz;
                break;
            }
            let s = self.sample(bus);
            self.output.push(s);
            self.samples_generated += 1;
        }
    }

    /// Peripheral ticks until the sample clock next fires (for schedulers).
    pub fn ticks_to_next_sample(&self, peripheral_hz: u64) -> u64 {
        if !self.active() {
            return u64::MAX;
        }
        (peripheral_hz - self.phase).div_ceil(SAMPLE_HZ)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat test memory (not firmware).
    struct Mem(Vec<u8>);
    impl Bus for Mem {
        fn read8(&mut self, a: u32) -> u8 {
            self.0.get(a as usize).copied().unwrap_or(0)
        }
        fn read16(&mut self, a: u32) -> u16 {
            u16::from_le_bytes([self.read8(a), self.read8(a + 1)])
        }
        fn read32(&mut self, a: u32) -> u32 {
            self.read16(a) as u32 | (self.read16(a + 2) as u32) << 16
        }
        fn write8(&mut self, _: u32, _: u8) {}
        fn write16(&mut self, _: u32, _: u16) {}
        fn write32(&mut self, _: u32, _: u32) {}
    }

    const HZ: u64 = 16_000_000;

    #[test]
    fn register_map_matches_the_bios_writes() {
        let mut e = VoiceEngine::default();
        // v2.1 boot: voice 6 start 0x0180_40F4/F8, end 0x0180_4134/38, volume 0x0180_4154.
        assert!(e.write(0x0180_40F4, 0x0001));
        assert!(e.write(0x0180_40F8, 0x2345));
        assert!(e.write(0x0180_4134, 0x0001));
        assert!(e.write(0x0180_4138, 0x2400));
        assert!(e.write(0x0180_4154, 0x3CCD));
        assert_eq!((e.start[6], e.end[6], e.volume[6]), (0x0001_2345, 0x0001_2400, 0x3CCD));
        assert!(e.write(0x0180_415C, 0x8000));
        assert_eq!(e.pitch[0], 0x8000);
        assert!(!e.write(0x0180_419C, 0x7FFF), "outside MAME's map: left unmodelled");
    }

    #[test]
    fn raw_voice_plays_end_minus_start_samples_at_8khz() {
        let mut e = VoiceEngine::default();
        let mut mem = Mem(vec![0xD5; 0x2000]);
        e.start[5] = 0x1000;
        e.end[5] = 0x1000 + 80; // 10 ms at 8 kHz
        e.volume[5] = 0x4000;
        e.command(START_COMMAND, 5);
        assert_eq!(e.status(), 1 << 4, "voice 5 in bit 4");
        e.advance(HZ / 100 - 1, HZ, &mut mem);
        assert!(e.triggered[5], "still playing just before 10 ms");
        e.advance(HZ / 100, HZ, &mut mem);
        assert!(!e.triggered[5]);
        assert_eq!(e.samples_generated, 80);
        assert_eq!(e.status(), 0);
    }

    #[test]
    fn pitched_voice_loops_and_stop_command_ends_it() {
        let mut e = VoiceEngine::default();
        let mut mem = Mem(vec![0x55; 0x2_0000]);
        e.start[0] = 0x0001_0000;
        e.end[0] = 0x0100 << 16; // end 0x100, loop point 0
        e.pitch[0] = 0x8000; // 1 byte per sample
        e.command(START_COMMAND, 0);
        assert_eq!(e.status(), 1 << 7, "voice 0 in bit 7");
        e.advance(HZ, HZ, &mut mem); // 1 s: loops, keeps playing
        assert!(e.triggered[0]);
        e.command(0x01, 0);
        assert_eq!(e.status(), 0);
    }

    #[test]
    fn lfc_stream_plays_until_its_terminator() {
        // Codebook page 1 (tables at 0x8900, all zero: silence), stream at
        // 0x100: mode 0, three empty records, then the 0x0FC0 terminator.
        let mut mem = vec![0u8; 0x1_0000];
        let words: [u16; 5] = [0, 0, 0, 0, 0x0FC0];
        for (i, w) in words.iter().enumerate() {
            mem[0x100 + 2 * i..0x102 + 2 * i].copy_from_slice(&w.to_le_bytes());
        }
        let mut mem = Mem(mem);
        let mut e = VoiceEngine { codebook_page: 1, ..Default::default() };
        e.start[7] = 0x100;
        e.command(START_COMMAND, 7);
        assert!(e.lfc_start_pending && e.status() == 0);
        e.start_lfc(&mut mem);
        assert_eq!(e.status(), 1 << 6, "voice 7 in bit 6");
        e.advance(HZ, HZ, &mut mem);
        assert!(!e.triggered[7], "ended");
        // 4 records + drain = 5 frames x 192 = 960 samples, the first 256
        // discarded: playback lasts 960 - 256 = 704 samples (+1 tick to notice).
        assert_eq!(e.lfc.frames, 5);
        assert!((704..=706).contains(&e.samples_generated), "{}", e.samples_generated);
    }

    #[test]
    fn lfc_needs_a_codebook_and_a_valid_mode() {
        let mut mem = Mem(vec![0u8; 0x200]);
        let mut e = VoiceEngine::default();
        e.command(START_COMMAND, 7);
        e.start_lfc(&mut mem);
        assert!(!e.triggered[7], "no codebook page programmed");
    }
}
