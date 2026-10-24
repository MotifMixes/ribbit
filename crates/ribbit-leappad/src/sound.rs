//! The LeapPad's sound processor: the same voice engine as the Leapster's
//! (ribbit-core `io::voices`), driven through shared on-chip RAM and FC1C.
//!
//! EVIDENCE that it is that engine (cart kernel 500-00406; docs/leappad.md,
//! "Audio"):
//!
//! * FC1E reads the playing voices with voice 0 in bit 7 and voices 1-7 in
//!   bits 0-6 (kernel 0x5A4D, 0x5CD3), the Leapster's odd layout.
//! * A command byte is the Leapster's: command << 3 | voice, start = 0x80 | n.
//! * The kernel lays out a voice's start (32 bits), end and loop (16-bit
//!   halves for voices 0-4, an absolute end for voice 5) and pitch as the
//!   Leapster's registers hold them, and the sounds it starts are A-law at
//!   those physical addresses (base ROM music at 0xDEA7C, 0x88A58 ...).
//! * The base ROM holds the Leapster's LFC codebook at 0x900, byte for byte,
//!   and cartridge speech is LFC streams.
//!
//! Shared RAM (VERIFIED-FW, 2026-10-24: the cartridge kernel computes these
//! with the same code as the Leapster BIOS's sound driver, whose register
//! writes say what each is; docs/leappad.md, "Music"):
//!
//! | XDATA | What | Leapster register |
//! |---|---|---|
//! | F462 + 4n | start: two little-endian words, high word first | 0x0180_40C4 |
//! | F482 + 4n | voices 0-4: end index, loop index; voices 5-6: absolute end | 0x0180_4104 |
//! | F49E + 2n | volume of the note, voices 0-5 (velocity curve: the table at base/cartridge 0x10300 = BIOS 0x4010_0F5C) | 0x0180_413C |
//! | F4AE + 2n | pitch, voices 0-4 (0x8000 = one sample a step; rewritten for bends) | 0x0180_415C |
//! | F4B8 + 2n | release rate, per instrument (the record's last field; voice 5's goes to F4CC) | 0x0180_4170 |
//! | F4C2 + 2n | 0x7FFF; for voices 6-7 their volume | 0x0180_4184 |
//! | F4D2 + 2n | current volume, voices 0-5 (rewritten about every 16 ms for fades) | 0x0180_41A4 |
//!
//! Before this, Ribbit read F49E as the pitch and F4C2 as the volume, so every
//! note played at a pitch made from its loudness: the "random noises".
//!
//! The processor reads the shared RAM by itself: the playing voices follow
//! F4AE and F4D2 as the kernel changes them (`follow`; EVIDENCE: otherwise
//! notes never fade). Commands: 0x80 | n starts a voice; 0x40 | n stops it;
//! 0x10 | n is a key off (the voice rings out to its sample's end); 0x08 | n
//! (sent after copying F4F4 + 2n to F4D2 + 2n) takes the current levels.

use crate::bus::PHYS_SIZE;
use ribbit_core::io::voices::{VoiceEngine, START_COMMAND};

/// Where the LFC decoder looks for the codebook, in 32 KB pages: 0x400 is
/// physical 0x2000000, which wraps to 0 in the 32 MB space, so the codebook
/// is read from the base ROM's 0x900 (where it is) whatever the base's size.
/// (Page 0 means "no codebook" to the shared engine. The old 0x20, physical
/// 0x100000, mirrored 0x900 only in the 1 MB base: in the 2 MB Full Base it
/// is other data, and every speech clip came out silent.) HYPOTHESIS: the
/// LeapPad has no codebook register that the kernel writes (none found yet).
const CODEBOOK_PAGE: u32 = PHYS_SIZE >> 15;

/// Physical memory as the sound processor sees it (ROMs only).
pub struct Rom<'a> {
    pub bios: &'a [u8],
    pub cart: Option<&'a [u8]>,
}

impl Rom<'_> {
    pub fn byte(&self, addr: u32) -> u8 {
        let addr = addr % crate::bus::PHYS_SIZE;
        let image = if addr < crate::bus::CART_BASE {
            Some((self.bios, addr))
        } else if addr < crate::bus::CART_BASE + 0x40_0000 {
            self.cart.map(|c| (c, addr - crate::bus::CART_BASE))
        } else {
            None
        };
        match image {
            Some((d, a)) if !d.is_empty() => d[a as usize % d.len()],
            _ => 0xFF,
        }
    }
}

impl ribbit_memory::Bus for Rom<'_> {
    fn read8(&mut self, addr: u32) -> u8 {
        self.byte(addr)
    }
    fn read16(&mut self, addr: u32) -> u16 {
        u16::from_le_bytes([self.byte(addr), self.byte(addr.wrapping_add(1))])
    }
    fn read32(&mut self, addr: u32) -> u32 {
        self.read16(addr) as u32 | (self.read16(addr.wrapping_add(2)) as u32) << 16
    }
    fn write8(&mut self, _: u32, _: u8) {}
    fn write16(&mut self, _: u32, _: u16) {}
    fn write32(&mut self, _: u32, _: u32) {}
}

/// A new engine as the LeapPad's sound processor uses it.
pub fn engine() -> VoiceEngine {
    let mut e = VoiceEngine::default();
    e.codebook_page = CODEBOOK_PAGE;
    e
}

/// A 32-bit value stored as two little-endian words, high word first.
fn words(ram: &[u8], at: usize) -> u32 {
    (u16::from_le_bytes([ram[at], ram[at + 1]]) as u32) << 16 | u16::from_le_bytes([ram[at + 2], ram[at + 3]]) as u32
}

fn word(ram: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([ram[at], ram[at + 1]])
}

/// The voices' release after a key off: the volume falls from where it was
/// to silence at each instrument's rate (F4B8 + 2n, the instrument record's
/// last field; the Leapster's 0x0180_4170).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Release {
    /// The share of the volume left (1.0 until the key off).
    level: [f32; 5],
    /// How much of it goes each output sample (0: not releasing).
    rate: [f32; 5],
}

/// How much of a voice's volume a unit of release rate takes each output
/// sample: F4B8 / 32768 of it every 25 samples (3.1 ms). Fitted to a
/// recording of the power-on tune, whose last chord (rate 0x00C8) fades out
/// over ~0.5 s: 1/(32768 x 25) gives an error of 0.18 (log RMS per 50 ms),
/// against 0.32 for 1e-6 and 0.20 for 1.3e-6.
const RELEASE_PER_SAMPLE: f32 = 1.0 / (32768.0 * 25.0);

/// A command written to FC1C. `xram` is on-chip RAM from 0xF000.
pub fn command(e: &mut VoiceEngine, r: &mut Release, v: u8, xram: &[u8], rom: &mut Rom) {
    let n = (v & 7) as usize;
    match v & 0xF8 {
        0x80 => {
            e.start[n] = words(xram, 0x462 + 4 * n);
            if n < 7 {
                e.end[n] = words(xram, 0x482 + 4 * n);
            }
            levels(e, n, xram);
            // A sound outside the 32 MB space, or one that ends before it
            // starts, is not in any ROM: older Classic cartridges ask for base
            // sounds from a table that only their own base had (RIB +18h =
            // 0x353000; docs/leappad.md, "Base sounds"), and this base gives
            // them nonsense. Ribbit leaves the voice silent, so the sound is
            // done at once. What the chip does with such addresses: UNKNOWN.
            let absolute_end = n == 5 || n == 6;
            if e.start[n] >= crate::bus::PHYS_SIZE
                || (absolute_end && (e.end[n] >= crate::bus::PHYS_SIZE || e.end[n] < e.start[n]))
            {
                return;
            }
            if n < 5 {
                r.level[n] = 1.0;
                r.rate[n] = 0.0;
            }
            e.command(START_COMMAND, n as u8);
            if n == 7 {
                e.start_lfc(rom);
            }
        }
        0x40 => e.command(0, n as u8),
        // Key off (0x10 | n): the voice goes on, fading out at its release
        // rate. VERIFIED against a recording of the power-on tune: its
        // notes ring until the next (stopping them cut each to ~70 ms), and
        // its last chord fades over ~0.5 s (playing out the sample instead
        // cut it off). 0x20 | 5, voice 5's: its sample just plays to its end.
        0x10 if n < 5 => r.rate[n] = word(xram, 0x4B8 + 2 * n).max(1) as f32 * RELEASE_PER_SAMPLE,
        // Other commands: the voice takes its current pitch and volume (the
        // kernel rewrites both while a note plays: bends and fades).
        _ => {
            levels(e, n, xram);
            if n < 5 && e.triggered[n] {
                e.write_pitch(n, e.pitch[n]);
            }
        }
    }
}

/// A voice's pitch and volume from shared RAM (VERIFIED-FW, 2026-10-24: the
/// cartridge kernel writes them where the Leapster BIOS's sound driver
/// writes its registers; see the module docs).
fn levels(e: &mut VoiceEngine, n: usize, xram: &[u8]) {
    if n < 5 {
        e.pitch[n] = word(xram, 0x4AE + 2 * n);
    }
    e.volume[n] = if n < 6 { word(xram, 0x4D2 + 2 * n) } else { word(xram, 0x4C2 + 2 * n) };
}

/// The playing voices follow their pitch and volume in shared RAM, as the
/// kernel changes them without a command (fades: F4D2 + 2n about every 16 ms;
/// bends: F4AE + 2n). EVIDENCE: notes otherwise never end, while on a real
/// pad (YouTube MrrGFKw4-44) they fade.
/// `mute`: voices silenced for research tools (bit n: voice n).
/// `samples`: output samples since the last call (for the releases).
pub fn follow(e: &mut VoiceEngine, r: &mut Release, xram: &[u8], mute: u8, samples: f32) {
    for n in 0..8 {
        if mute >> n & 1 != 0 {
            e.volume[n] = 0;
        }
    }
    for n in 0..5 {
        if e.triggered[n] && r.rate[n] > 0.0 {
            r.level[n] -= r.rate[n] * samples;
            if r.level[n] <= 0.0 {
                r.rate[n] = 0.0;
                e.command(0, n as u8);
            }
        }
    }
    for n in (0..6).filter(|n| mute >> n & 1 == 0) {
        if e.triggered[n] {
            let level = if n < 5 { r.level[n] } else { 1.0 };
            e.volume[n] = (word(xram, 0x4D2 + 2 * n) as f32 * level) as u16;
            if n < 5 {
                let p = word(xram, 0x4AE + 2 * n);
                if p != e.pitch[n] {
                    e.write_pitch(n, p);
                }
            }
        }
    }
}

/// FC1E: the voices playing (voice 0 in bit 7, voices 1-7 in bits 0-6).
pub fn busy(e: &VoiceEngine) -> u8 {
    e.status() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The codebook is the base ROM's 0x900 for every base size (the 2 MB
    /// Full Base once read other data and all speech was silent).
    #[test]
    fn the_codebook_is_the_base_roms_0x900_whatever_its_size() {
        for size in [1 << 20, 2 << 20, 4 << 20] {
            let mut bios = vec![0u8; size];
            bios[0x900] = 0x5A;
            let rom = Rom { bios: &bios, cart: None };
            assert_eq!(rom.byte((CODEBOOK_PAGE << 15) | 0x900), 0x5A, "{} MB base", size >> 20);
        }
    }
}
