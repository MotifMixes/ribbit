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
//! | Voices 0-4 | pitched A-law; address = (start & 0xFFFF_0000) \| u16(position); position starts at start's low half and advances by pitch / 0x8000 per sample; at end (end >> 16) it loops to the loop point (end & 0xFFFF) or stops if end - loop < 2; between samples, linear interpolation (EVIDENCE: a tester's comparison with real hardware; MAME takes the earlier sample) |
//! | Voices 5-6 | raw A-law at 8 kHz from start to end (absolute addresses) |
//! | Voice 7 | LFC speech: 16-bit mode word (0-2 = Level4/6/8), records of control + optional reflection-coefficient word + excitation of the previous record, 192 samples each; 256 start-up samples discarded; control 0x0FC0 ends the stream |
//! | 0x0180_2078 | playing voices: voice 0 in bit 7, voices 1-7 in bits 0-6 ("not sure why it's out of place" - MAME) |
//! | Sample clock | 8 kHz (MAME's legacy mix rate) |
//! | A-law | standard G.711 (`ribbit_audio::alaw_to_linear`); MAME's table is the same curve at half amplitude and opposite sign |
//! | Mix | sum of voice * 0.125 * volume / 0x4000, clamped to 16 bits (MAME) |
//!
//! PCM DMA (0x0180_3000-300C), SOURCED from MAME (same source, UNVERIFIED;
//! MAME: "the observed 0x7f01 configuration runs at 32 kHz, with an interrupt
//! after each half-buffer. Other control settings ... need further hardware
//! verification"). VERIFIED-FW that it is used: the Leapster 2 BIOS 0.6
//! programs both channels for The Talking Words Factory (its title waits for
//! the stream). Two circular channels, n = (control bits 8-14) + 1 samples
//! per half-buffer, bit 0 = on:
//!
//! | Register | Use |
//! |---|---|
//! | 0x3000 / 0x3008 | channel 0 / 1 control (read back) |
//! | 0x3004 / 0x300C | channel 0 / 1 base; reads return base + position (MAME) |
//!
//! While a channel is on, a 32 kHz DAC clock runs: every fourth tick takes an
//! 8 kHz voice-mix sample, raised to 32 kHz with MAME's interpolator
//! (`ribbit_audio::upsample4x`). Channel 0 writes that 16-bit mix to memory
//! (for software to mix with its own audio); channel 1 reads the DAC's signed
//! 16-bit sample from memory. A channel interrupts (flag bit 20 + channel, CPU
//! line 0x13 if enabled) each time its position reaches a half-buffer
//! boundary (MAME uses a timer of the same period from the control write).
//! Host output: while channel 1 plays, the 32 kHz DAC stream goes to
//! `dac_output` as it is, and to the 8 kHz host ring averaged over four
//! samples (for hosts that only take 8 kHz); otherwise the 8 kHz mix as
//! before, which the host raises to 32 kHz with the same interpolator.
//!
//! Not modelled: analog output, the other 0x0180_4000 registers.

use ribbit_memory::Bus;

pub const BASE: u32 = 0x0180_4000;
pub const STATUS_ADDR: u32 = 0x0180_2078;
pub const CODEBOOK_LO: u32 = 0x0180_20E0;
pub const CODEBOOK_HI: u32 = 0x0180_20E4;
pub const SAMPLE_HZ: u64 = 8_000;
pub const PCM_BASE: u32 = 0x0180_3000;
pub const PCM_HZ: u64 = 32_000;
/// Interrupt flag bit of PCM channel 0 (channel 1 is the next bit).
pub const PCM_FLAG_SHIFT: u32 = 20;
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
    #[serde(default)]
    pub pcm: Pcm,
    /// Mixed output at 8 kHz, for the host audio backend (not saved).
    #[serde(skip, default = "default_output")]
    pub output: ribbit_audio::SampleRing,
    /// The 32 kHz DAC stream while PCM channel 1 plays (not saved).
    #[serde(skip, default = "default_dac_output")]
    pub dac_output: ribbit_audio::SampleRing,
}

fn default_dac_output() -> ribbit_audio::SampleRing {
    ribbit_audio::SampleRing::new(PCM_HZ as usize)
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
            pcm: Pcm::default(),
            output: default_output(),
            dac_output: default_dac_output(),
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
        if let Some((ch, base)) = pcm_register(addr) {
            let p = &self.pcm;
            return Some(if base { p.base[ch].wrapping_add(p.position[ch]) } else { p.control[ch] });
        }
        (addr == STATUS_ADDR).then(|| self.status())
    }

    /// Register writes (16-bit values; the BIOS uses `sth`).
    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        if let Some((ch, base)) = pcm_register(addr) {
            let p = &mut self.pcm;
            if base {
                p.base[ch] = value;
            } else {
                p.control[ch] = value;
            }
            p.position[ch] = 0;
            return true;
        }
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
                    0x15C..=0x16F if ((off - 0x15C) / 2) & 1 == 0 => {
                        // Takes effect at once, also on a playing voice: the
                        // BIOS's sound driver bends notes by rewriting pitch
                        // every ~16 ms without restarting them (sequence
                        // command 0x8A; SetPitch at US v2.1 0x4003_06E0).
                        // VERIFIED-FW; MAME reads pitch only at note start.
                        let ch = ((off - 0x15C) / 4) as usize;
                        self.pitch[ch] = v;
                        if self.triggered[ch] {
                            self.dx[ch] = v as f32 / 32768.0;
                        }
                    }
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
            let s = if v < 5 {
                // Pitched voices step through the sample by fractions:
                // linearly interpolated between the two samples around the
                // position (the next one wraps to the loop point at the
                // end). EVIDENCE: a tester comparing with real hardware hears
                // linear interpolation; MAME takes the earlier sample alone.
                let at = |i: u32| (start & 0xFFFF_0000) | (i as u16 as u32);
                let (end, lp) = ((self.end[v] >> 16) as u16, self.end[v] as u16);
                let i = pos as u32;
                let looping = (end as i32) - (lp as i32) >= 2;
                let next = if i + 1 < end as u32 { Some(i + 1) } else if looping { Some(lp as u32) } else { None };
                let s0 = ribbit_audio::alaw_to_linear(bus.read8(at(i))) as f64;
                match next {
                    Some(n) => {
                        let s1 = ribbit_audio::alaw_to_linear(bus.read8(at(n))) as f64;
                        let frac = (pos - i as f32) as f64;
                        s0 + (s1 - s0) * frac
                    }
                    None => s0,
                }
            } else {
                ribbit_audio::alaw_to_linear(bus.read8(start.wrapping_add(pos as u32))) as f64
            };
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
        // `phase` stays below `peripheral_hz` between calls, so no ticks
        // means no sample and no change (most CPU steps at 96:16 MHz).
        if ticks == 0 {
            return;
        }
        if self.pcm.running() {
            return self.advance_dac(ticks, peripheral_hz, bus);
        }
        self.phase += ticks * SAMPLE_HZ;
        if !self.active() {
            // Idle: keep the clock phase; avoid a division on every step.
            if self.phase >= peripheral_hz {
                self.output_silence(self.phase / peripheral_hz);
                self.phase %= peripheral_hz;
            }
            return;
        }
        while self.phase >= peripheral_hz {
            self.phase -= peripheral_hz;
            if !self.active() {
                self.output_silence(1 + self.phase / peripheral_hz);
                self.phase %= peripheral_hz;
                break;
            }
            let s = self.sample(bus);
            self.output.push(s);
            self.samples_generated += 1;
        }
    }

    /// The DAC keeps running while no voice plays: `n` silent samples for
    /// the host output (not emulated state; the host plays a continuous
    /// stream instead of bursts with holes between sounds).
    fn output_silence(&mut self, n: u64) {
        for _ in 0..n.min(self.output.capacity() as u64) {
            self.output.push(0);
        }
    }

    /// The 32 kHz DAC clock while a PCM channel is on.
    fn advance_dac(&mut self, ticks: u64, peripheral_hz: u64, bus: &mut impl Bus) {
        self.pcm.phase += ticks * PCM_HZ;
        while self.pcm.phase >= peripheral_hz {
            self.pcm.phase -= peripheral_hz;
            if self.pcm.sub == 0 {
                let legacy = if self.active() {
                    self.samples_generated += 1;
                    self.sample(bus)
                } else {
                    0
                };
                self.pcm.upsampled = ribbit_audio::upsample4x(&mut self.pcm.history, legacy);
            }
            let mut dac = self.pcm.upsampled[self.pcm.sub as usize];
            self.pcm.sub = (self.pcm.sub + 1) & 3;
            let p = &mut self.pcm;
            if p.control[0] & 1 != 0 {
                bus.write16(p.base[0].wrapping_add(p.position[0]), dac as u16);
            }
            if p.control[1] & 1 != 0 {
                dac = bus.read16(p.base[1].wrapping_add(p.position[1])) as i16;
            }
            for ch in 0..2 {
                if p.control[ch] & 1 != 0 {
                    let half = p.half_bytes(ch);
                    p.position[ch] = (p.position[ch] + 2) % (2 * half);
                    if p.position[ch].is_multiple_of(half) {
                        p.irq |= 1 << (PCM_FLAG_SHIFT + ch as u32);
                    }
                }
            }
            if p.control[1] & 1 != 0 {
                self.dac_output.push(dac);
                let p = &mut self.pcm;
                p.host_sum += dac as i32;
                if self.pcm.sub == 0 {
                    let avg = self.pcm.host_sum / 4;
                    self.pcm.host_sum = 0;
                    self.output.push(avg as i16);
                }
            } else if self.pcm.sub == 0 {
                let s = self.pcm.upsampled[0];
                self.output.push(s);
            }
        }
    }

    /// Interrupt flag bits raised by PCM DMA since the last call.
    pub fn take_pcm_irq(&mut self) -> u32 {
        std::mem::take(&mut self.pcm.irq)
    }

    /// Peripheral ticks until the sample clock next fires (for schedulers).
    pub fn ticks_to_next_sample(&self, peripheral_hz: u64) -> u64 {
        if self.pcm.running() {
            return (peripheral_hz - self.pcm.phase).div_ceil(PCM_HZ);
        }
        if !self.active() {
            return u64::MAX;
        }
        (peripheral_hz - self.phase).div_ceil(SAMPLE_HZ)
    }
}

/// PCM DMA channel state (see the module notes).
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Pcm {
    pub control: [u32; 2],
    pub base: [u32; 2],
    /// Byte offset of the next sample in each channel's buffer.
    pub position: [u32; 2],
    /// Fractional DAC clock, in peripheral ticks x PCM_HZ.
    phase: u64,
    /// DAC tick within the current 8 kHz sample (0-3).
    sub: u8,
    upsampled: [i16; 4],
    history: [f64; 16],
    host_sum: i32,
    /// Interrupt flag bits not yet delivered.
    irq: u32,
}

impl Pcm {
    pub fn running(&self) -> bool {
        self.control.iter().any(|c| c & 1 != 0)
    }

    /// Bytes per half-buffer of channel `ch`.
    fn half_bytes(&self, ch: usize) -> u32 {
        (((self.control[ch] >> 8) & 0x7F) + 1) * 2
    }
}

/// (channel, is the base register) for 0x0180_3000-300C.
fn pcm_register(addr: u32) -> Option<(usize, bool)> {
    let off = addr.checked_sub(PCM_BASE).filter(|&o| o < 0x10 && o % 4 == 0)?;
    Some(((off / 8) as usize, off % 8 == 4))
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
        fn write8(&mut self, a: u32, v: u8) {
            if let Some(b) = self.0.get_mut(a as usize) {
                *b = v;
            }
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.write8(a, v as u8);
            self.write8(a + 1, (v >> 8) as u8);
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.write16(a, v as u16);
            self.write16(a + 2, (v >> 16) as u16);
        }
    }

    const HZ: u64 = 16_000_000;
    /// Peripheral ticks per 32 kHz DAC sample.
    const DAC_TICKS: u64 = HZ / PCM_HZ;

    #[test]
    fn pitched_voices_interpolate_between_samples_and_across_the_loop() {
        // Voice 0 at half speed over a 3-sample loop at 0x100..0x103.
        let codes = [0xD5u8, 0x2A, 0x80];
        let lin = codes.map(|c| ribbit_audio::alaw_to_linear(c) as f64);
        let mut mem = Mem(vec![0; 0x200]);
        mem.0[0x100..0x103].copy_from_slice(&codes);
        let mut e = VoiceEngine::default();
        e.start[0] = 0x100;
        e.end[0] = (0x103 << 16) | 0x100;
        e.pitch[0] = 0x4000;
        e.volume[0] = 0x4000;
        e.command(START_COMMAND, 0);
        let got: Vec<i16> = (0..7).map(|_| e.sample(&mut mem)).collect();
        let want = |x: f64| (x * 0.125).clamp(-32768.0, 32767.0) as i16;
        let mid = |a: f64, b: f64| want((a + b) / 2.0);
        assert_eq!(
            got,
            [
                want(lin[0]),
                mid(lin[0], lin[1]),
                want(lin[1]),
                mid(lin[1], lin[2]),
                want(lin[2]),
                mid(lin[2], lin[0]), // the last sample blends into the loop point
                want(lin[0]),
            ]
        );
    }

    #[test]
    fn pcm_registers_read_back_and_base_reads_include_the_position() {
        let mut e = VoiceEngine::default();
        assert!(e.write(0x0180_300C, 0x1000));
        assert!(e.write(0x0180_3008, 0x7F01));
        assert_eq!(e.read(0x0180_3008), Some(0x7F01));
        let mut mem = Mem(vec![0; 0x2000]);
        e.advance(DAC_TICKS * 3, HZ, &mut mem);
        assert_eq!(e.read(0x0180_300C), Some(0x1006), "three 16-bit samples played");
        assert!(!e.write(0x0180_3010, 0));
    }

    #[test]
    fn pcm_channel_1_plays_memory_and_interrupts_each_half_buffer() {
        let mut e = VoiceEngine::default();
        let mut mem = Mem(vec![0; 0x2000]);
        for i in 0..256u32 {
            mem.write16(0x1000 + 2 * i, 4000);
        }
        e.write(0x0180_300C, 0x1000);
        e.write(0x0180_3008, 0x7F01); // 2 x 128 samples
        e.advance(DAC_TICKS * 127, HZ, &mut mem);
        assert_eq!(e.take_pcm_irq(), 0);
        assert_eq!(e.ticks_to_next_sample(HZ), DAC_TICKS);
        e.advance(DAC_TICKS, HZ, &mut mem);
        assert_eq!(e.take_pcm_irq(), 1 << 21, "first half played");
        e.advance(DAC_TICKS * 128, HZ, &mut mem);
        assert_eq!(e.take_pcm_irq(), 1 << 21, "second half played");
        assert_eq!(e.pcm.position[1], 0, "wrapped");
        let mut out = vec![0i16; 64];
        e.output.pop_into(&mut out);
        assert_eq!(out[10], 4000, "host output: the DAC stream at 8 kHz");
        assert_eq!(e.dac_output.len(), 256, "and at 32 kHz");
    }

    #[test]
    fn pcm_channel_0_captures_the_voice_mix() {
        let mut e = VoiceEngine::default();
        let mut mem = Mem(vec![0x55; 0x2000]);
        e.write(0x0180_3004, 0x1000);
        e.write(0x0180_3000, 0x0301); // 2 x 4 samples
        e.advance(DAC_TICKS * 4, HZ, &mut mem);
        assert_eq!(e.take_pcm_irq(), 1 << 20);
        assert_eq!(mem.read32(0x1000), 0, "silence captured");
        assert_eq!(mem.read16(0x1008), 0x5555, "not yet reached");
    }

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
    fn pitch_writes_bend_a_playing_note() {
        let mut e = VoiceEngine::default();
        e.start[1] = 0x0001_0000;
        e.end[1] = 0x8000 << 16;
        assert!(e.write(BASE + 0x160, 0x8000)); // voice 1: 1 byte per sample
        e.command(START_COMMAND, 1);
        assert_eq!(e.dx[1], 1.0);
        assert!(e.write(BASE + 0x160, 0xC000)); // bend up mid-note
        assert_eq!(e.dx[1], 1.5, "applies to the playing note at once");
        assert!(e.write(BASE + 0x15C, 0x4000)); // voice 0 is idle
        assert_eq!((e.pitch[0], e.dx[0]), (0x4000, 0.0), "kept for its next start");
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
