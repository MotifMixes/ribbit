//! The sound unit (0xD0400000), which plays the menu's and the games'
//! sounds by itself: the System's Audio_* routines only set its registers
//! (no samples pass through the processor; the DAC at 0xC00D0000 carries
//! only the start-up logo's decoded MP3). The same unit as the Zippity's
//! (ribbit-zippity sound.rs, after MAME's spg2xx_audio): 32 channels,
//! channel n's registers at +0x1000 + 0x40 n, its phase registers at
//! +0x1800 + 0x40 n, the control registers at +0xE00 (channels 0-15) and
//! +0xE80 (16-31).
//!
//! What differs, VERIFIED from the System's own routine that sets a
//! channel (0x11460): the registers hold 16 bits, as on the 16-bit
//! GeneralPlus chips, so the addresses (in halfwords, 28 bits) are split:
//! the wave's bits 15-0 in register 0, 21-16 in mode bits 5-0, 27-22 in
//! register 14 bits 5-0; the loop's in register 2, mode bits 11-6 and
//! register 14 bits 11-6; the envelope's in registers 8, 7 and 15. Mode
//! bit 15 ADPCM, 14 16-bit samples, 13-12 the tone mode (1 one shot, 2
//! loop: Audio_isWaveLoopEnable reads bit 13), as on the Zippity.

use crate::machine::{Bus, CLOCK, DAC_HZ};

#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub struct Voice {
    pub(crate) playing: bool,
    /// The next word's address, in halfwords.
    addr: u32,
    /// Nibbles (or bytes) of the word used so far, times 4 (or 8).
    shift: u32,
    signal: i32,
    step: i32,
    /// ADPCM36: the sample before `signal`, the frame's header, its data
    /// words still to come, and where the last of them ends (`shift`; 0:
    /// a whole word).
    signal2: i32,
    header: u32,
    frame_left: u32,
    #[serde(default)]
    last_shift: u32,
    accum: u64,
    sample: i32,
    /// The sample before, for interpolating between the two.
    prev: i32,
    /// The envelope's next address (halfwords), and ticks until the next
    /// envelope and ramp-down steps.
    env_addr: u32,
    env_frame: u32,
    pub(crate) ramp_frame: u32,
    /// What the channel put out last (left, right), faded out when it
    /// stops (`spu_release`).
    #[serde(skip)]
    last_out: (i32, i32),
}

/// The sound unit's base clock: a channel plays phase * SPU_CLOCK / 2^19
/// samples a second (VERIFIED: the System sets a wave's phase to its rate
/// times 14913 / 8192, 0x11E5C, which is 2^19 / 288 kHz; 16 kHz waves get
/// 0x71C7), and output samples come at SPU_CLOCK / 4 (as on the Zippity,
/// UNVERIFIED here).
pub const SPU_CLOCK: u64 = 288_000;
/// The output rate, SPU_CLOCK / 4.
pub const OUTPUT_RATE: f64 = SPU_CLOCK as f64 / 4.0;
/// The DAC's level against the sound unit's: a quarter, so a full-scale MP3
/// comes out near full scale after the app raises the sound unit's quiet mix
/// four times (ribbit-frontend twist.rs `LOUDNESS`). The real balance is
/// UNVERIFIED.
const DAC_LEVEL: f32 = 0.25;

/// ADPCM36's predictors, (c0, c1) in 64ths of the last two samples, by
/// header bits 8-4: the first five are Sony XA's; all fitted from the
/// music packs held both as ADPCM36 and as 16-bit samples (VERIFIED by
/// decoding, see `adpcm36_fetch`; 30 and 31 never seen).
const ADPCM36_FILTERS: [(i32, i32); 32] = [
    (0, 0), (60, 0), (115, -52), (98, -55), (122, -60), (127, -64), (63, 1), (-64, -1),
    (-74, -35), (62, -5), (6, 19), (53, -11), (-24, -32), (72, -20), (-41, -14), (64, -2),
    (23, -7), (74, -13), (10, -30), (84, -33), (64, -32), (96, -46), (-9, -12), (85, -25),
    (39, 15), (79, -47), (41, -27), (97, -37), (38, -50), (110, -51), (0, 0), (0, 0),
];

/// IMA ADPCM, low nibble first, as on the Zippity.
const IMA_STEPS: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130,
    143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282,
    1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630,
    9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794, 32767,
];
const IMA_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Ticks between envelope steps for each envelope clock setting, and
/// between ramp-down steps for each ramp-down clock (MAME's tables).
const ENV_FRAMES: [u32; 16] = [4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 8192, 8192, 8192, 8192];
const RAMP_FRAMES: [u32; 8] = [13 * 4, 13 * 16, 13 * 64, 13 * 256, 13 * 1024, 13 * 4096, 13 * 8192, 13 * 8192];

const CTL: u32 = 0xD040_0E00;

/// A stopped channel's output fades by this much each output sample (to a
/// third in about 1.5 ms).
const TAIL_FADE: f32 = 0.991;

impl Bus {
    fn spu_reg(&self, ch: usize, r: u32) -> u32 {
        self.latch(0xD040_1000 + 0x40 * ch as u32 + 4 * r) & 0xFFFF
    }

    fn spu_set(&mut self, ch: usize, r: u32, v: u32) {
        self.set_latch(0xD040_1000 + 0x40 * ch as u32 + 4 * r, v);
    }

    fn spu_ctl(&self, bank: usize, off: u32) -> u32 {
        self.latch(CTL + 0x80 * bank as u32 + off)
    }

    fn spu_word(&self, halfword: u32) -> u32 {
        let a = (halfword as usize) * 2;
        self.ram.get(a..a + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as u32)
    }

    /// A channel's wave address and loop address, in halfwords.
    fn spu_addresses(&self, ch: usize) -> (u32, u32) {
        let (mode, high) = (self.spu_reg(ch, 1), self.spu_reg(ch, 14));
        (
            (high & 0x3F) << 22 | (mode & 0x3F) << 16 | self.spu_reg(ch, 0),
            (high >> 6 & 0x3F) << 22 | (mode >> 6 & 0x3F) << 16 | self.spu_reg(ch, 2),
        )
    }

    /// A channel's envelope table address, in halfwords.
    fn spu_env_base(&self, ch: usize) -> u32 {
        (self.spu_reg(ch, 15) & 0x3F) << 22 | (self.spu_reg(ch, 7) & 0x3F) << 16 | self.spu_reg(ch, 8)
    }

    /// The channels playing, a bit each, of one bank of 16.
    pub(crate) fn spu_busy(&self, bank: usize) -> u32 {
        (0..16).filter(|&i| self.voices[bank * 16 + i].playing).fold(0, |m, i| m | 1 << i)
    }

    /// Channel enable writes: a channel whose bit goes from 0 to 1 starts,
    /// one cleared stops. A channel that has ended keeps its bit (see
    /// `spu_stop`), so it starts again only when the System clears and
    /// sets it, as it does for each new wave (0x11EE0).
    pub(crate) fn spu_enable(&mut self, bank: usize, v: u32, old: u32) {
        for i in 0..16 {
            let ch = bank * 16 + i;
            let on = v & (1 << i) != 0;
            if on && old & (1 << i) == 0 {
                let env_addr = self.spu_env_base(ch);
                self.voices[ch] = Voice {
                    playing: true,
                    addr: self.spu_addresses(ch).0,
                    env_addr,
                    env_frame: 4,
                    ..Default::default()
                };
                // The envelope count starts at its load value.
                let r5 = self.spu_reg(ch, 5);
                self.spu_set(ch, 5, (self.spu_reg(ch, 6) & 0xFF) << 8 | (r5 & 0x7F));
            } else if !on {
                if self.voices[ch].playing {
                    self.spu_release(ch);
                }
                self.voices[ch].playing = false;
            }
        }
    }

    /// Ramp-down writes: channels newly set (and playing) start fading.
    pub(crate) fn spu_ramp_down(&mut self, bank: usize, v: u32, old: u32) {
        let new = v & 0xFFFF & self.spu_busy(bank);
        for i in 0..16 {
            if (new & !old) & (1 << i) != 0 {
                let ch = bank * 16 + i;
                let clock = self.latch(0xD040_1800 + 0x40 * ch as u32 + 0xC) & 7;
                self.voices[ch].ramp_frame = RAMP_FRAMES[clock as usize];
            }
        }
        self.set_latch(CTL + 0x80 * bank as u32 + 0x28, new);
    }

    /// The next sample of a channel, or None at the end of a one-shot.
    fn spu_fetch(&mut self, ch: usize) -> Option<i32> {
        let mode = self.spu_reg(ch, 1);
        let tone = (mode >> 12) & 3;
        // Tone mode 0: the processor writes the samples (register 0xB).
        if tone == 0 {
            return Some((self.spu_reg(ch, 0xB) as u16 as i32) - 0x8000);
        }
        if mode & 0x8000 != 0 && self.spu_reg(ch, 0xD) & 0x8000 != 0 {
            return self.adpcm36_fetch(ch, tone);
        }
        let v = self.voices[ch];
        let word = self.spu_word(v.addr);
        if word == 0xFFFF {
            // The ADPCM bit clears at a loop (as in MAME's model and on the
            // Zippity).
            if tone == 2 && mode & 0x8000 != 0 {
                self.spu_set(ch, 1, mode & !0x8000);
            }
            return self.spu_wave_end(ch, tone);
        }
        let vo = &mut self.voices[ch];
        if mode & 0x8000 != 0 {
            let n = ((word >> vo.shift) & 15) as i32;
            let step = IMA_STEPS[vo.step as usize];
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
            vo.signal = (vo.signal + if n & 8 != 0 { -diff } else { diff }).clamp(-32768, 32767);
            vo.step = (vo.step + IMA_INDEX[(n & 7) as usize]).clamp(0, 88);
            vo.shift += 4;
            if vo.shift == 16 {
                vo.shift = 0;
                vo.addr += 1;
            }
            Some(vo.signal)
        } else if mode & 0x4000 != 0 {
            vo.addr += 1;
            Some(word as i32 - 0x8000)
        } else {
            let b = (word >> vo.shift) & 0xFF;
            vo.shift += 8;
            if vo.shift == 16 {
                vo.shift = 0;
                vo.addr += 1;
            }
            Some(((b as i32) << 8) - 0x8000)
        }
    }

    /// The end mark (0xFFFF): a one shot ends; a loop goes on from the
    /// loop address.
    fn spu_wave_end(&mut self, ch: usize, tone: u32) -> Option<i32> {
        if tone == 1 {
            return None;
        }
        let loop_addr = self.spu_addresses(ch).1;
        let vo = &mut self.voices[ch];
        vo.addr = loop_addr;
        vo.shift = 0;
        vo.frame_left = 0;
        vo.header = 0;
        Some(vo.sample)
    }

    /// ADPCM36 (channel register 13 bit 15; the System sets 0xBE00 for its
    /// "SPF2" waves of formats 4-6): frames of a header word and up to 8
    /// data words, 4-bit residuals, low nibble first. Header bits 3-0 shift
    /// the residual (n << 12 >> shift), bits 8-4 pick the predictor
    /// (`ADPCM36_FILTERS`), bits 13-9 are the frame's samples less one and
    /// bit 14 marks the wave's last frame (VERIFIED over every wave in the
    /// packs: 15.6 million frames read 0xBExx-0xBFxx, 32 samples, and each
    /// wave's last frame alone has bit 14, its count matching the data
    /// before the "ffff ffff" that follows). So the wave ends after that
    /// frame: 0xFFFF data words are only samples (the menu's music has 24,
    /// the first 0.6 s in). VERIFIED against the games' own music, which
    /// some packs hold both this way and as 16-bit samples (29 dB).
    fn adpcm36_fetch(&mut self, ch: usize, tone: u32) -> Option<i32> {
        if self.voices[ch].frame_left == 0 {
            if self.voices[ch].header & 0x4000 != 0 {
                return self.spu_wave_end(ch, tone);
            }
            let header = self.spu_word(self.voices[ch].addr);
            if header == 0xFFFF {
                return self.spu_wave_end(ch, tone);
            }
            let samples = (header >> 9 & 31) + 1;
            let vo = &mut self.voices[ch];
            vo.header = header;
            vo.addr += 1;
            vo.frame_left = samples.div_ceil(4);
            vo.last_shift = samples % 4 * 4;
            vo.shift = 0;
        }
        let word = self.spu_word(self.voices[ch].addr);
        let vo = &mut self.voices[ch];
        let n = (((word >> vo.shift) & 15) as i32) << 28 >> 28;
        let (c0, c1) = ADPCM36_FILTERS[(vo.header >> 4 & 31) as usize];
        let s = ((n << 12) >> (vo.header & 15)) + ((vo.signal * c0 + vo.signal2 * c1 + 32) >> 6);
        let s = s.clamp(-32768, 32767);
        (vo.signal2, vo.signal) = (vo.signal, s);
        vo.shift += 4;
        if vo.shift == 16 || (vo.frame_left == 1 && vo.shift == vo.last_shift) {
            vo.shift = 0;
            vo.addr += 1;
            vo.frame_left -= 1;
        }
        Some(s)
    }

    /// Stops a channel at its end: busy clears, the stop flag sets, and any
    /// ramp-down ends. Its enable bit stays set (unlike the Zippity's):
    /// the System's Audio_getWaveStatus (0x145AC) calls a wave stopped
    /// when its channel is enabled and not busy, and paused when it is not
    /// enabled; Nom-Nom waits for its sounds to be stopped before starting
    /// a level (EVIDENCE: with the bit cleared it waits forever).
    fn spu_stop(&mut self, ch: usize) {
        let (bank, bit) = (ch / 16, 1u32 << (ch % 16));
        self.spu_release(ch);
        self.voices[ch].playing = false;
        self.spu_stopped[bank] |= bit;
        let a = CTL + 0x80 * bank as u32 + 0x28;
        let v = self.latch(a);
        self.set_latch(a, v & !bit);
    }

    /// A channel stops: what it was last putting out fades away over a
    /// couple of milliseconds instead of dropping to nothing at once (a
    /// click; the console's speaker and filter soften it too).
    fn spu_release(&mut self, ch: usize) {
        let (l, r) = std::mem::take(&mut self.voices[ch].last_out);
        self.spu_tail.0 += l as f32;
        self.spu_tail.1 += r as f32;
    }

    /// One step of a channel's envelope (as on the Zippity, after MAME).
    fn spu_envelope(&mut self, ch: usize) {
        let (r4, r5, r6) = (self.spu_reg(ch, 4), self.spu_reg(ch, 5), self.spu_reg(ch, 6));
        let (mut count, edd) = ((r5 >> 8) & 0xFF, (r5 & 0x7F) as i32);
        if count > 0 {
            count -= 1;
            self.spu_set(ch, 5, count << 8 | edd as u32);
        }
        if count != 0 {
            return;
        }
        let (target, inc) = (((r4 >> 8) & 0x7F) as i32, (r4 & 0x7F) as i32);
        let mut new = edd;
        if new != target {
            if r4 & 0x80 != 0 {
                new = (edd - inc).max(0);
                if new != 0 && new < target {
                    new = target;
                }
                if new == 0 {
                    self.spu_set(ch, 5, r5 & 0xFF00);
                    self.spu_stop(ch);
                    return;
                }
            } else {
                new = (edd + inc).min(target);
            }
        }
        let mut r6 = r6;
        if new == target {
            if r6 & 0x100 != 0 {
                let repeat = (r6 >> 9).wrapping_sub(1) & 0x7F;
                if repeat == 0 {
                    let ea = self.voices[ch].env_addr;
                    let (e4, e6, e10) = (self.spu_word(ea), self.spu_word(ea + 1), self.spu_word(ea + 2));
                    self.spu_set(ch, 4, e4);
                    self.spu_set(ch, 6, e6);
                    self.spu_set(ch, 10, e10);
                    r6 = e6;
                    self.voices[ch].env_addr = self.spu_env_base(ch) + (e10 & 0x1FF);
                } else {
                    r6 = (r6 & 0x1FF) | repeat << 9;
                    self.spu_set(ch, 6, r6);
                }
            } else {
                let ea = self.voices[ch].env_addr;
                let (e4, e6) = (self.spu_word(ea), self.spu_word(ea + 1));
                self.spu_set(ch, 4, e4);
                self.spu_set(ch, 6, e6);
                r6 = e6;
                self.voices[ch].env_addr = ea + 2;
            }
        }
        self.spu_set(ch, 5, (r6 & 0xFF) << 8 | new as u32);
    }

    /// One ramp-down step: the level falls by register 10 bits 15-9; at 0
    /// the channel stops.
    fn spu_rampdown(&mut self, ch: usize) {
        let r5 = self.spu_reg(ch, 5);
        let new = (r5 & 0x7F) as i32 - ((self.spu_reg(ch, 10) >> 9) & 0x7F) as i32;
        if new > 0 {
            self.spu_set(ch, 5, (r5 & 0xFF00) | new as u32);
            let clock = self.latch(0xD040_1800 + 0x40 * ch as u32 + 0xC) & 7;
            self.voices[ch].ramp_frame = RAMP_FRAMES[clock as usize];
        } else {
            self.spu_set(ch, 5, r5 & 0xFF00);
            self.spu_stop(ch);
        }
    }

    /// The beat counter, one tick: the base count (+0xE10) runs down; at 0
    /// it reloads and the beat count (+0xE14 bits 13-0) drops by one; at 0,
    /// with bit 15 set, the beat flag (bit 14) rises. The System takes no
    /// fast interrupts (its table at 0x84A14 stays empty), so the flag is
    /// only read.
    fn beat_tick(&mut self) {
        if self.beat_base > 0 {
            self.beat_base -= 1;
        }
        if self.beat_base == 0 {
            self.beat_base = self.latch(CTL + 0x10) & 0xFFFF;
            let reg = self.latch(CTL + 0x14);
            let mut count = reg & 0x3FFF;
            if count > 0 {
                count -= 1;
                self.set_latch(CTL + 0x14, (reg & !0x3FFF) | count);
            }
            if count == 0 && reg & 0x8000 != 0 {
                self.beat_flag = true;
            }
        }
    }

    /// Mixes one output sample (left, right) at SPU_CLOCK / 4, as the
    /// Zippity's: each channel's sample interpolated, times its envelope
    /// level, volume and pan; the sum scaled by the volume select and the
    /// main volume.
    fn spu_mix(&mut self) -> (i32, i32) {
        let (mut l, mut r) = (0i32, 0i32);
        for ch in 0..32 {
            if !self.voices[ch].playing {
                continue;
            }
            let phase = self.latch(0xD040_1800 + 0x40 * ch as u32 + 0x10) & 0x7_FFFF;
            self.voices[ch].accum += phase as u64;
            while self.voices[ch].accum >= 1 << 17 {
                self.voices[ch].accum -= 1 << 17;
                match self.spu_fetch(ch) {
                    Some(s) => {
                        self.voices[ch].prev = self.voices[ch].sample;
                        self.voices[ch].sample = s;
                    }
                    None => {
                        self.spu_stop(ch);
                        break;
                    }
                }
            }
            if !self.voices[ch].playing {
                continue;
            }
            let (bank, bit) = (ch / 16, 1u32 << (ch % 16));
            let v = self.voices[ch];
            let mut s = v.sample;
            if self.spu_ctl(0, 0x34) & 0x200 == 0 {
                let f = ((v.accum * 256) >> 17) as i32;
                s = ((v.prev * (256 - f)) >> 8) + ((v.sample * f) >> 8);
            }
            let s = (s * (self.spu_reg(ch, 5) & 0x7F) as i32) >> 7;
            let pv = self.spu_reg(ch, 3);
            let (vol, pan) = ((pv & 0x7F) as i32, ((pv >> 8) & 0x7F) as i32);
            let (pl, pr) = if pan < 0x40 { (0x7F * vol, pan * 2 * vol) } else { ((0x7F - pan) * 2 * vol, 0x7F * vol) };
            let out = ((s * pl) >> 14, (s * pr) >> 14);
            self.voices[ch].last_out = out;
            l += out.0;
            r += out.1;
            if self.spu_ctl(bank, 0x28) & bit != 0 {
                if self.voices[ch].ramp_frame > 0 {
                    self.voices[ch].ramp_frame -= 1;
                }
                if self.voices[ch].ramp_frame == 0 {
                    self.spu_rampdown(ch);
                }
            } else if self.spu_ctl(bank, 0x54) & bit == 0 {
                if self.voices[ch].env_frame > 0 {
                    self.voices[ch].env_frame -= 1;
                }
                if self.voices[ch].env_frame == 0 {
                    self.spu_envelope(ch);
                    let i = ch % 16;
                    let clock = (self.spu_ctl(bank, 0x18 + 4 * (i as u32 / 4)) >> (4 * (i % 4))) & 15;
                    self.voices[ch].env_frame = ENV_FRAMES[clock as usize];
                }
            }
        }
        l += self.spu_tail.0 as i32;
        r += self.spu_tail.1 as i32;
        self.spu_tail.0 *= TAIL_FADE;
        self.spu_tail.1 *= TAIL_FADE;
        let shift = if (self.spu_ctl(0, 0x34) >> 6) & 3 == 0 { 4 } else { 2 };
        let main = (self.spu_ctl(0, 0x04) & 0x7F) as i32;
        (((l >> shift) * main) >> 7, ((r >> shift) * main) >> 7)
    }

    /// Research: the channels playing, one line each (registers 0-15 and
    /// where the channel has got to).
    pub fn spu_state(&self) -> Vec<String> {
        (0..32)
            .filter(|&ch| self.voices[ch].playing)
            .map(|ch| {
                let regs: Vec<String> = (0..16).map(|r| format!("{:04x}", self.spu_reg(ch, r))).collect();
                let (wave, lp) = self.spu_addresses(ch);
                format!("ch {ch:2}: {} wave {wave:07x} loop {lp:07x} at {:07x}", regs.join(" "), self.voices[ch].addr)
            })
            .collect()
    }

    /// Runs the sound unit up to now: one output sample (into `sound`) each
    /// CLOCK / OUTPUT_RATE cycles.
    /// The DAC's output at time `at`, at the sound unit's rate: its last two
    /// samples interpolated (a sample late), at `DAC_LEVEL`.
    fn dac_sample(&self, at: u64) -> (i32, i32) {
        let period = (CLOCK / DAC_HZ) as f32;
        let f = (at.saturating_sub(self.dac_at) as f32 / period).min(1.0);
        let mix = |a: i16, b: i16| ((a as f32 + (b as f32 - a as f32) * f) * DAC_LEVEL) as i32;
        (mix(self.dac_prev.0, self.dac_hold.0), mix(self.dac_prev.1, self.dac_hold.1))
    }

    pub(crate) fn tick_sound(&mut self) {
        // Called every few hundred cycles, a sample every few thousand.
        if self.now < self.sound_next {
            return;
        }
        let due = (self.now as u128 * (SPU_CLOCK / 4) as u128 / CLOCK as u128) as u64;
        self.sound_next = ((due as u128 + 1) * CLOCK as u128).div_ceil((SPU_CLOCK / 4) as u128) as u64;
        while self.sound_made < due {
            self.sound_made += 1;
            self.beat_tick();
            let (mut l, mut r) = self.spu_mix();
            // The DAC (MP3s: the Music station, the start-up logo), mixed in
            // while it plays.
            // This output sample's own time (`now` is the tick's).
            let at = (self.sound_made as u128 * CLOCK as u128 / (SPU_CLOCK / 4) as u128) as u64;
            if at < self.dac_hold_until {
                let (dl, dr) = self.dac_sample(at);
                l += dl;
                r += dr;
            }
            if self.sound.len() < 1 << 24 {
                self.sound.extend_from_slice(&[l.clamp(-32768, 32767) as i16, r.clamp(-32768, 32767) as i16]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::machine::{CLOCK, Twist};

    /// The DAC's samples (MP3s) reach the speaker, mixed with the sound
    /// unit. Ribbit 1.4.2 decoded MP3s but left the DAC out of the mix.
    #[test]
    fn dac_samples_reach_the_speaker() {
        let mut t = Twist::new(&[]);
        // 64 stereo samples of 16000 (left) and -16000 (right) at 0x1000.
        for i in 0..64 {
            t.bus.ram[0x1000 + 4 * i..0x1000 + 4 * i + 2].copy_from_slice(&16000i16.to_le_bytes());
            t.bus.ram[0x1000 + 4 * i + 2..0x1000 + 4 * i + 4].copy_from_slice(&(-16000i16).to_le_bytes());
        }
        // DMA channel 0 to the DAC: 128 16-bit units.
        t.bus.write(0xD030_0004, 0x1000, 4);
        t.bus.write(0xD030_0008, 0xC00D_0004, 4);
        t.bus.write(0xD030_000C, 128, 4);
        t.bus.write(0xD030_0000, 0x1001, 4);
        while t.bus.now < CLOCK / 1000 {
            t.bus.now += 256;
            t.bus.tick();
        }
        let sound = std::mem::take(&mut t.bus.sound);
        let (l, r): (Vec<i16>, Vec<i16>) = sound.chunks(2).map(|p| (p[0], p[1])).unzip();
        assert!(l.iter().any(|&v| v == 4000), "left at a quarter: {:?}", &l[..20]);
        assert!(r.iter().any(|&v| v == -4000), "right at a quarter");
    }

    #[test]
    fn an_adpcm_one_shot_plays_from_its_16_bit_address_and_stops() {
        let mut t = Twist::new(&[]);
        // A wave at halfword 0x1_0010 (mode bits 5-0 = 1, register 0 =
        // 0x10): a few ADPCM words, then the end mark.
        let at = 0x1_0010 * 2;
        for i in 0..8 {
            t.bus.ram[at + 2 * i..at + 2 * i + 2].copy_from_slice(&0x7777u16.to_le_bytes());
        }
        t.bus.ram[at + 16..at + 18].copy_from_slice(&0xFFFFu16.to_le_bytes());
        let ch = 0xD040_1000 + 0x40 * 3;
        t.bus.write(ch, 0x0010, 4); // wave address
        t.bus.write(ch + 4, 0x9041, 4); // ADPCM, one shot, address bits 1
        t.bus.write(ch + 0xC, 0x407F, 4); // centre, full volume
        t.bus.write(ch + 0x14, 0x7F, 4); // envelope level
        t.bus.write(0xD040_1800 + 0x40 * 3 + 0x10, 0x8000, 4); // phase: 1/4 sample a tick
        t.bus.write(0xD040_0E54, 0x8, 4); // manual envelope
        t.bus.write(0xD040_0E04, 0x7F, 4); // main volume
        t.bus.write(0xD040_0E00, 0x8, 4);
        assert_eq!(t.bus.read(0xD040_0E3C, 4) & 8, 8, "busy");
        t.bus.now = CLOCK / 200; // 5 ms: time for all 32 samples
        t.bus.tick_sound();
        let sound = std::mem::take(&mut t.bus.sound);
        assert!(sound.iter().any(|&s| s > 1000), "rising: ADPCM 7s add");
        assert_eq!(t.bus.read(0xD040_0E3C, 4) & 8, 0, "ended at the mark");
        assert_eq!(t.bus.read(0xD040_0E2C, 4) & 8, 8, "its stop flag");
        assert_eq!(t.bus.read(0xD040_0E00, 4) & 8, 8, "still enabled: stopped, not paused");
        t.bus.write(0xD040_0E2C, 8, 4);
        assert_eq!(t.bus.read(0xD040_0E2C, 4) & 8, 0, "cleared by writing 1");
    }

    #[test]
    fn an_adpcm36_loop_wave_loops_to_its_loop_address() {
        let mut t = Twist::new(&[]);
        // Wave at halfword 0x01_0010: one frame of negative samples
        // (residuals 0xE = -2, s = -8192 with filter 0), then the end mark.
        let wave_at = 0x01_0010usize * 2;
        // Header: the last frame, 32 samples, filter 0.
        t.bus.ram[wave_at..wave_at + 2].copy_from_slice(&0xFE00u16.to_le_bytes());
        for i in 1..=8 {
            t.bus.ram[wave_at + 2 * i..wave_at + 2 * i + 2].copy_from_slice(&0xEEEEu16.to_le_bytes());
        }
        t.bus.ram[wave_at + 18..wave_at + 20].copy_from_slice(&0xFFFFu16.to_le_bytes()); // end mark
        // Loop body at halfword 0x01_2000: one frame of large positive samples
        // (residuals 0x7 = +7, s = +28672 with filter 0).
        let loop_at = 0x01_2000usize * 2;
        t.bus.ram[loop_at..loop_at + 2].copy_from_slice(&0xFE00u16.to_le_bytes());
        for i in 1..=8 {
            t.bus.ram[loop_at + 2 * i..loop_at + 2 * i + 2].copy_from_slice(&0x7777u16.to_le_bytes());
        }
        let ch = 0xD040_1000 + 0x40 * 3;
        // Wave address bits 21-16 = 1 → mode bits 5-0 = 1.
        // Loop address bits 21-16 = 1 → mode bits 11-6 = 1.
        // mode 0xA041: ADPCM (bit 15), loop/tone=2 (bits 13-12), mid bits = 1 each.
        t.bus.write(ch, 0x0010, 4);     // reg 0: wave addr bits 15-0
        t.bus.write(ch + 4, 0xA041, 4); // reg 1: ADPCM, loop, addr mid bits
        t.bus.write(ch + 8, 0x2000, 4); // reg 2: loop addr bits 15-0
        t.bus.write(ch + 0xC, 0x407F, 4); // reg 3: centre, full volume
        t.bus.write(ch + 0x14, 0x7F, 4); // reg 5: envelope level
        t.bus.write(ch + 0x34, 0xBE00, 4); // reg 0xD: ADPCM36
        t.bus.write(0xD040_1800 + 0x40 * 3 + 0x10, 0x8000, 4); // phase: 4 ticks per fetch
        t.bus.write(0xD040_0E54, 0x8, 4); // manual envelope, ch 3
        t.bus.write(0xD040_0E04, 0x7F, 4); // main volume
        t.bus.write(0xD040_0E00, 0x8, 4); // enable ch 3
        // 200 ms: 112x longer than the ~1.8 ms wave body.
        t.bus.now = CLOCK / 5;
        t.bus.tick_sound();
        let sound = std::mem::take(&mut t.bus.sound);
        // A tone=2 loop wave never stops.
        assert_eq!(t.bus.read(0xD040_0E3C, 4) & 8, 8, "still busy: loop never stops");
        assert_eq!(t.bus.read(0xD040_0E2C, 4) & 8, 0, "not stopped");
        // Wave body produced negative samples; loop body produced large positive ones.
        assert!(sound.iter().any(|&s| s < -100), "wave body: negative samples before the loop");
        assert!(sound.iter().any(|&s| s > 100), "loop body: positive samples after the loop");
    }

    /// Plays ADPCM36 words from halfword 0x1_0010 as a one-shot and
    /// returns the output and whether the channel is still busy after 5 ms.
    fn play_adpcm36(words: &[u16]) -> (Vec<i16>, bool) {
        let mut t = Twist::new(&[]);
        let at = 0x1_0010 * 2;
        for (i, w) in words.iter().enumerate() {
            t.bus.ram[at + 2 * i..at + 2 * i + 2].copy_from_slice(&w.to_le_bytes());
        }
        let ch = 0xD040_1000 + 0x40 * 3;
        t.bus.write(ch, 0x0010, 4);
        t.bus.write(ch + 4, 0x9041, 4); // ADPCM, one shot, address bits 1
        t.bus.write(ch + 0xC, 0x407F, 4);
        t.bus.write(ch + 0x14, 0x7F, 4);
        t.bus.write(ch + 0x34, 0xBE00, 4); // ADPCM36
        t.bus.write(0xD040_1800 + 0x40 * 3 + 0x10, 0x8000, 4);
        t.bus.write(0xD040_0E54, 0x8, 4);
        t.bus.write(0xD040_0E04, 0x7F, 4);
        t.bus.write(0xD040_0E00, 0x8, 4);
        t.bus.now = CLOCK / 200;
        t.bus.tick_sound();
        let busy = t.bus.read(0xD040_0E3C, 4) & 8 != 0;
        (std::mem::take(&mut t.bus.sound), busy)
    }

    #[test]
    fn an_adpcm36_wave_ends_after_its_last_frame() {
        // As the packs' waves end: a last frame (bit 14) of 8 samples, two
        // data words, "ffff ffff", then another wave's loud words, which
        // must not play.
        let mut words = vec![0xCE00u16, 0x1111, 0x1111, 0xFFFF, 0xFFFF];
        words.extend([0x7777; 14]);
        let (sound, busy) = play_adpcm36(&words);
        assert!(!busy, "ended after the last frame");
        let peak = sound.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 0 && peak < 1000, "only the quiet first wave plays (peak {peak})");
    }

    #[test]
    fn an_adpcm36_data_word_of_ffff_is_only_samples() {
        // The menu's music has 0xFFFF data words from 0.6 s in; the wave
        // goes on past them to its last frame (loud 7s), and ends there.
        let mut words = vec![0xBE00u16, 0x0000, 0xFFFF, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000, 0x0000];
        words.extend([0xFE00, 0x7777, 0x7777, 0x7777, 0x7777, 0x7777, 0x7777, 0x7777, 0x7777, 0xFFFF, 0xFFFF]);
        let (sound, busy) = play_adpcm36(&words);
        assert!(!busy, "ended after the last frame");
        assert!(sound.iter().any(|&s| s > 1000), "played on into the second frame");
    }
}
