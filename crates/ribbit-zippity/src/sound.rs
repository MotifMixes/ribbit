//! The sound unit (0xD0400000): 32 channels like the 16-bit GeneralPlus
//! chips' (MAME's spg2xx_audio, used for facts), registers four bytes apart.
//! Channel n's registers are at +0x1000 + 0x40 n (0: wave address in
//! halfwords, 1: mode, 2: loop address, 3: pan and volume, 4-8 and 10:
//! envelope, 0xB: wave data), its phase registers at +0x1800 + 0x40 n (3:
//! ramp-down clock, 4: the phase, 19-bit), and the control registers at
//! +0xE00 for channels 0-15 and +0xE80 for 16-31 (+0x00 enable, +0x04 main
//! volume, +0x10/+0x14 beat counter, +0x18-0x24 envelope clocks, +0x28 ramp
//! down, +0x2C stop flags, +0x34 control, +0x3C busy, +0x54 manual envelope).

use crate::bus::Bus;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Default, Serialize, Deserialize)]
pub struct Voice {
    pub(crate) playing: bool,
    /// The next word's address, in halfwords.
    addr: u32,
    /// Nibbles (or bytes) of the word used so far, times 4 (or 8).
    shift: u32,
    signal: i32,
    step: i32,
    accum: u64,
    sample: i32,
    /// The sample before, for interpolating between the two.
    prev: i32,
    /// The envelope's next address (halfwords), and ticks until the next
    /// envelope and ramp-down steps.
    env_addr: u32,
    env_frame: u32,
    pub(crate) ramp_frame: u32,
}

/// The sound unit's base clock: a channel plays phase * SPU_CLOCK / 2^19
/// samples a second, and output samples come at SPU_CLOCK / 4 (as MAME's
/// model has it; UNVERIFIED on the Zippity), so each output sample a
/// channel moves on phase / 2^17 samples.
pub const SPU_CLOCK: u64 = 281_250;
/// The output rate, SPU_CLOCK / 4.
pub const OUTPUT_RATE: f64 = SPU_CLOCK as f64 / 4.0;

/// OKI ADPCM: the step sizes and how each nibble moves the step index.
fn oki_step(i: i32) -> i32 {
    (16.0 * 1.1f64.powi(i)) as i32
}
const OKI_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

/// Ticks between envelope steps for each envelope clock setting, and
/// between ramp-down steps for each ramp-down clock (MAME's tables).
const ENV_FRAMES: [u32; 16] = [4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096, 8192, 8192, 8192, 8192, 8192];
pub(crate) const RAMP_FRAMES: [u32; 8] = [13 * 4, 13 * 16, 13 * 64, 13 * 256, 13 * 1024, 13 * 4096, 13 * 8192, 13 * 8192];

impl Bus {
    fn spu_reg(&self, ch: usize, r: u32) -> u32 {
        self.latch(0xD040_1000 + 0x40 * ch as u32 + 4 * r)
    }

    fn spu_set(&mut self, ch: usize, r: u32, v: u32) {
        self.set_latch(0xD040_1000 + 0x40 * ch as u32 + 4 * r, v);
    }

    fn spu_ctl(&self, bank: usize, off: u32) -> u32 {
        self.latch(0xD040_0E00 + 0x80 * bank as u32 + off)
    }

    fn spu_word(&self, halfword: u32) -> u32 {
        self.peek16(halfword * 2)
    }

    /// Channel enable writes: channels newly enabled start, ones cleared stop.
    pub(crate) fn spu_enable(&mut self, bank: usize, v: u32) {
        for i in 0..16 {
            let ch = bank * 16 + i;
            let on = v & (1 << i) != 0;
            if on && !self.voices[ch].playing {
                if self.probe.spu_log {
                    let phase = self.latch(0xD040_1800 + 0x40 * ch as u32 + 0x10);
                    println!(
                        "spu {:>10} ch {ch} mode {:04x} addr {:06x} loop {:06x} phase {phase:05x} vol {:04x}",
                        self.now,
                        self.spu_reg(ch, 1),
                        self.spu_reg(ch, 0),
                        self.spu_reg(ch, 2),
                        self.spu_reg(ch, 3)
                    );
                }
                let env_addr = (self.spu_reg(ch, 7) & 0x3F) << 16 | (self.spu_reg(ch, 8) & 0xFFFF);
                self.voices[ch] = Voice {
                    playing: true,
                    addr: self.spu_reg(ch, 0) & 0x3F_FFFF,
                    env_addr,
                    env_frame: 4,
                    ..Default::default()
                };
                // The envelope count starts at its load value.
                let r5 = self.spu_reg(ch, 5);
                self.spu_set(ch, 5, (self.spu_reg(ch, 6) & 0xFF) << 8 | (r5 & 0x7F));
            } else if !on {
                self.voices[ch].playing = false;
            }
        }
    }

    /// The next sample of a channel, or None at the end of a one-shot.
    fn spu_fetch(&mut self, ch: usize) -> Option<i32> {
        let mode = self.spu_reg(ch, 1);
        let tone = (mode >> 12) & 3;
        // Tone mode 0: the CPU writes the samples (register 0xB, wave data).
        if tone == 0 {
            return Some((self.spu_reg(ch, 0xB) as u16 as i32) - 0x8000);
        }
        let v = self.voices[ch];
        let word = self.spu_word(v.addr);
        if word == 0xFFFF {
            if tone == 1 {
                return None;
            }
            // Loop: from the loop address (register 2).
            self.voices[ch].addr = self.spu_reg(ch, 2) & 0x3F_FFFF;
            self.voices[ch].shift = 0;
            return Some(v.sample);
        }
        let vo = &mut self.voices[ch];
        if mode & 0x8000 != 0 {
            // 4-bit OKI ADPCM, high nibble first (low first, as in MAME's
            // 16-bit chips, decodes to noise).
            let n = ((word >> (12 - vo.shift)) & 15) as i32;
            let step = oki_step(vo.step);
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
            vo.signal = (vo.signal + if n & 8 != 0 { -diff } else { diff }).clamp(-2048, 2047);
            vo.step = (vo.step + OKI_INDEX[(n & 7) as usize]).clamp(0, 48);
            vo.shift += 4;
            if vo.shift == 16 {
                vo.shift = 0;
                vo.addr += 1;
            }
            // MAME keeps OKI ADPCM to 12 bits; scaled to 16 here.
            Some(vo.signal << 4)
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

    /// Stops a channel: busy and enable clear, the stop flag sets, and any
    /// ramp-down ends.
    fn spu_stop(&mut self, ch: usize) {
        let (bank, bit) = (ch / 16, 1u32 << (ch % 16));
        self.voices[ch].playing = false;
        self.spu_stopped[bank] |= bit;
        for off in [0x00, 0x28] {
            let a = 0xD040_0E00 + 0x80 * bank as u32 + off;
            let v = self.latch(a);
            self.set_latch(a, v & !bit);
        }
    }

    /// One step of a channel's envelope (MAME's spg2xx_audio, as facts):
    /// the level (register 5 bits 6-0) moves by the increment (register 4
    /// bits 6-0, down if bit 7) towards the target (bits 14-8) every
    /// "load" ticks (register 6 bits 7-0); at the target the next envelope
    /// pair is read from memory (address in registers 7 and 8), repeating
    /// a stretch if register 6 bit 8 asks. Falling to 0 stops the channel.
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
                    let base = (self.spu_reg(ch, 7) & 0x3F) << 16 | (self.spu_reg(ch, 8) & 0xFFFF);
                    self.voices[ch].env_addr = base + (e10 & 0x1FF);
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

    /// The sound unit's beat counter, one tick (4 cycles of its clock): the
    /// base count (+0xE10) runs down; at 0 it reloads and the beat count
    /// (+0xE14 bits 13-0) drops by one; at 0, with bit 15 set, the beat flag
    /// rises (fast interrupt line 2: the kernel's music sequencer).
    pub(crate) fn beat_tick(&mut self) {
        if self.beat_base > 0 {
            self.beat_base -= 1;
        }
        if self.beat_base == 0 {
            self.beat_base = self.latch(0xD040_0E10) & 0xFFFF;
            let reg = self.latch(0xD040_0E14);
            let mut count = reg & 0x3FFF;
            if count > 0 {
                count -= 1;
                self.set_latch(0xD040_0E14, (reg & !0x3FFF) | count);
            }
            if count == 0 && reg & 0x8000 != 0 {
                self.beat_flag = true;
            }
        }
    }

    /// Mixes one output sample (left, right) at SPU_CLOCK / 4, as MAME's
    /// model does: each channel's sample interpolated, times its envelope
    /// level, volume and pan; the sum scaled by the control register's
    /// volume select and the main volume.
    pub(crate) fn spu_mix(&mut self) -> (i32, i32) {
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
            l += (s * pl) >> 14;
            r += (s * pr) >> 14;
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
        let shift = if (self.spu_ctl(0, 0x34) >> 6) & 3 == 0 { 4 } else { 2 };
        let main = (self.spu_ctl(0, 0x04) & 0x7F) as i32;
        (((l >> shift) * main) >> 7, ((r >> shift) * main) >> 7)
    }
}
