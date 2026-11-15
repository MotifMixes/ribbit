//! The second sound chip, on the SIO: the system ROM's driver (0x00fa90-
//! 0x00fbd8; every cartridge has a copy) streams it compressed sound in
//! Sunplus's SACM formats. 7000|n plays one (n = 2: SACM-A1600, the jingles
//! and most speech; n = 3: SACM-S480/S720, the rest of the speech), command
//! 0 with each 15 words of it after, 6020 before a sound and 6040 to stop.
//! The status, read back on port C: 0x10 playing, 0x40 wants the next 15
//! words, 0x01 finished (the driver then marks the sound done, which
//! activities wait for before going on). See docs/clickstart.md.
//!
//! The A1600 decoder is Ribbit's own, written from a study of Sunplus's PC
//! codec (A1600.DLL, for research only: its frame unpacking's layout and
//! tables, and its output for the ROMs' sounds, which a plain floating-point
//! IMDCT matches within the DLL's fixed-point rounding, 0.3% rms). The
//! S480/S720 decoder is Ribbit's own too: Federal Standard 1016's line
//! spectral pairs and its LPC synthesis, with the frame layout, tables and
//! excitation found by studying S4872.DLL (see `S4872`).

use crate::sound::OUTPUT_RATE;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::OnceLock;

/// The chip's A1600 rate: measured against a real console's recording
/// (the "Click" jingle and the built-in activities' speech match its
/// spectra at 0.768 x 16 kHz, within 0.2%).
pub const A1600_RATE: f64 = 12288.0;
/// The S480/S720 rate: the codec's own 8 kHz (see the doc).
pub const S4872_RATE: f64 = 8000.0;
/// The chip's level in the mix: the "Click" jingle's loudness against the
/// system music's in the recording.
const LEVEL: i32 = 16; // /32

/// Words the chip takes ahead of playing.
const FIFO_WORDS: usize = 64;

#[derive(Default, Serialize, Deserialize)]
enum Codec {
    /// No header yet.
    #[default]
    None,
    A1600(A1600),
    /// S480/S720 (a Federal Standard 1016 style CELP): 9 words a frame,
    /// 240 samples (S480) or 160 (S720).
    S4872(S4872),
    /// Unknown: 10 words for each 10 ms, in silence.
    Other,
}

#[derive(Default, Serialize, Deserialize)]
pub struct SioAudio {
    command: u16,
    /// The play command's n (7000|n): the format.
    format: u16,
    playing: bool,
    finished: bool,
    /// The words received (the length's two included), and the sound's
    /// length in words once known.
    received: u32,
    length: Option<u32>,
    /// Received words not decoded yet (the length taken off).
    fifo: VecDeque<u16>,
    codec: Codec,
    /// Decoded samples not played yet, the chip's rate, and the position
    /// between the last two samples played (for the output rate).
    pcm: VecDeque<i16>,
    rate: f64,
    phase: f64,
    prev: i16,
    cur: i16,
}

impl SioAudio {
    pub fn command(&mut self, cmd: u16) {
        self.command = cmd;
        match cmd {
            0x7000..=0x77ff => {
                *self = SioAudio { command: cmd, format: cmd & 0x7ff, playing: true, ..SioAudio::default() };
            }
            0x6020 | 0x6040 => {
                *self = SioAudio { command: cmd, ..SioAudio::default() };
            }
            _ => {}
        }
    }

    pub fn data(&mut self, word: u16) {
        if self.command != 0 || !self.playing || self.received >= self.length.unwrap_or(u32::MAX) {
            return;
        }
        match self.received {
            0 => self.length = Some(word as u32),
            // The length is in bytes, its own four included.
            1 => self.length = self.length.map(|lo| ((word as u32) << 16 | lo).div_ceil(2)),
            _ => self.fifo.push_back(word),
        }
        self.received += 1;
    }

    pub fn status(&mut self) -> u16 {
        if !self.playing {
            return if std::mem::take(&mut self.finished) { 0x01 } else { 0 };
        }
        let length = self.length.unwrap_or(u32::MAX);
        if self.received < length && self.fifo.len() + 15 <= FIFO_WORDS {
            0x50
        } else {
            0x10
        }
    }

    /// One output sample's worth (OUTPUT_RATE): the chip's sound, scaled
    /// for the mix.
    pub fn tick(&mut self) -> i32 {
        if !self.playing {
            return 0;
        }
        if self.rate == 0.0 && !self.start_codec() {
            return 0;
        }
        self.phase += self.rate / OUTPUT_RATE;
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.prev = self.cur;
            self.cur = match self.next_sample() {
                Some(s) => s,
                None => {
                    // Played out, or waiting for data.
                    if self.received >= self.length.unwrap_or(u32::MAX) {
                        self.playing = false;
                        self.finished = true;
                        return 0;
                    }
                    self.phase = 0.0;
                    self.prev
                }
            };
        }
        let s = self.prev as f64 + (self.cur as f64 - self.prev as f64) * self.phase;
        (s as i32 * LEVEL) >> 5
    }

    /// Picks the decoder once the header is in.
    fn start_codec(&mut self) -> bool {
        let Some(&first) = self.fifo.front() else { return false };
        self.codec = match (self.format, first) {
            (2, 0x8004..=0x8009) => {
                self.fifo.pop_front();
                Codec::A1600(A1600::new((first & 0xf) as u8))
            }
            (3, _) => Codec::S4872(S4872::new()),
            _ => Codec::Other,
        };
        self.rate = match self.codec {
            Codec::A1600(_) => A1600_RATE,
            Codec::S4872(_) => S4872_RATE,
            _ => 12800.0,
        };
        true
    }

    fn next_sample(&mut self) -> Option<i16> {
        if self.pcm.is_empty() {
            self.decode_frame();
        }
        self.pcm.pop_front()
    }

    fn decode_frame(&mut self) {
        let need = match &self.codec {
            Codec::A1600(a) => a.frame_words(),
            Codec::S4872(_) => 9,
            Codec::Other => 10,
            Codec::None => return,
        };
        if self.fifo.len() < need {
            return;
        }
        let words: Vec<u16> = self.fifo.drain(..need).collect();
        match &mut self.codec {
            Codec::A1600(a) => a.decode(&words, &mut self.pcm),
            Codec::S4872(c) => c.decode(&words, &mut self.pcm),
            _ => self.pcm.extend(std::iter::repeat_n(0, 128)),
        }
    }
}

// ----- SACM-A1600 -----
//
// A frame is 128 samples: an MDCT's 128 coefficients in 16 bands of 8. Its
// bits are read from the low end of each word up: 16 4-bit band scales,
// then each band in turn. The bands get their bit allocation by scale, the
// loudest first, from the mode's list; an allocation picks a table that,
// for the band's peak position (3 bits), gives each other coefficient's
// bits. The peak is the scale's gain with a sign bit; the others a level
// (a fraction of the gain, in 64ths) whose low bit is the sign.

const A1600_GAIN: [i32; 16] = [1, 3, 6, 11, 20, 36, 61, 103, 172, 288, 479, 797, 1324, 2200, 3653, 6066];
/// Each mode's allocations (mode 4-9 = header 0x8004-0x8009), loudest band
/// first.
const A1600_ALLOC: [[u8; 16]; 6] = [
    [4, 4, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [6, 6, 6, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [8, 8, 4, 4, 4, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [8, 8, 8, 6, 6, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1],
    [8, 8, 8, 8, 8, 6, 4, 2, 2, 2, 2, 2, 1, 1, 1, 1],
    [8, 8, 8, 8, 8, 8, 6, 6, 4, 4, 2, 2, 2, 2, 2, 2],
];
/// Each mode's frame in words.
const A1600_FRAME_WORDS: [usize; 6] = [10, 12, 14, 16, 20, 24];
/// The levels for a 4-, 3-, 2- and 1-bit coefficient (index = code >> 1).
const A1600_LEVEL4: [i32; 8] = [1, 5, 9, 15, 21, 27, 35, 47];
const A1600_LEVEL3: [i32; 4] = [3, 12, 24, 41];
const A1600_LEVEL2: [i32; 2] = [3, 24];
const A1600_LEVEL1: [i32; 1] = [5];

/// The bits of coefficient `i` in a band of allocation `alloc` whose peak
/// is at `peak` (the peak's own entry is unused: it takes a sign bit).
/// Allocation 1 gives the others none, 8 gives them all 4.
fn a1600_bits(alloc: u8, peak: usize, i: usize) -> u32 {
    // One row per peak position.
    const T2: [[u8; 8]; 8] = [
        [4, 2, 2, 0, 0, 0, 0, 0],
        [2, 4, 2, 0, 0, 0, 0, 0],
        [0, 2, 4, 2, 0, 0, 0, 0],
        [0, 0, 2, 4, 2, 0, 0, 0],
        [0, 0, 0, 2, 4, 2, 0, 0],
        [0, 0, 0, 0, 2, 4, 2, 0],
        [0, 0, 0, 0, 0, 2, 4, 2],
        [0, 0, 0, 0, 0, 2, 2, 4],
    ];
    const T3: [[u8; 8]; 8] = [
        [4, 2, 2, 2, 2, 0, 0, 0],
        [2, 4, 2, 2, 2, 0, 0, 0],
        [2, 2, 4, 2, 2, 0, 0, 0],
        [0, 2, 2, 4, 2, 2, 0, 0],
        [0, 0, 2, 2, 4, 2, 2, 0],
        [0, 0, 0, 2, 2, 4, 2, 2],
        [0, 0, 0, 2, 2, 2, 4, 2],
        [0, 0, 0, 2, 2, 2, 2, 4],
    ];
    const T4: [[u8; 8]; 8] = [
        [4, 2, 2, 2, 2, 2, 2, 0],
        [2, 4, 2, 2, 2, 2, 2, 0],
        [2, 2, 4, 2, 2, 2, 2, 0],
        [2, 2, 2, 4, 2, 2, 2, 0],
        [0, 2, 2, 2, 4, 2, 2, 2],
        [0, 2, 2, 2, 2, 4, 2, 2],
        [0, 2, 2, 2, 2, 2, 4, 2],
        [0, 2, 2, 2, 2, 2, 2, 4],
    ];
    const T5: [[u8; 8]; 8] = [
        [4, 4, 2, 2, 2, 2, 2, 2],
        [2, 4, 4, 2, 2, 2, 2, 2],
        [2, 2, 4, 4, 2, 2, 2, 2],
        [2, 2, 2, 4, 4, 2, 2, 2],
        [2, 2, 2, 2, 4, 4, 2, 2],
        [2, 2, 2, 2, 2, 4, 4, 2],
        [2, 2, 2, 2, 2, 2, 4, 4],
        [2, 2, 2, 2, 2, 2, 4, 4],
    ];
    const T6: [[u8; 8]; 8] = [
        [4, 4, 4, 4, 2, 2, 2, 2],
        [4, 4, 4, 4, 2, 2, 2, 2],
        [2, 4, 4, 4, 4, 2, 2, 2],
        [2, 2, 4, 4, 4, 4, 2, 2],
        [2, 2, 2, 4, 4, 4, 4, 2],
        [2, 2, 2, 2, 4, 4, 4, 4],
        [2, 2, 2, 2, 4, 4, 4, 4],
        [2, 2, 2, 2, 4, 4, 4, 4],
    ];
    let t = match alloc {
        1 => return 0,
        2 => &T2,
        3 => &T3,
        4 => &T4,
        5 => &T5,
        6 => &T6,
        _ => return 4,
    };
    t[peak][i] as u32
}

/// cos(pi/128 (m + 1/2)(k + 1/2)): the 128-point DCT-IV.
fn dct4_table() -> &'static [f32] {
    static TABLE: OnceLock<Vec<f32>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = Vec::with_capacity(128 * 128);
        for m in 0..128 {
            for k in 0..128 {
                t.push((std::f64::consts::PI / 128.0 * (m as f64 + 0.5) * (k as f64 + 0.5)).cos() as f32);
            }
        }
        t
    })
}

struct Bits<'a> {
    words: &'a [u16],
    pos: usize,
}

impl Bits<'_> {
    /// The next `n` bits, the first read the lowest.
    fn read(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for i in 0..n {
            let w = self.words.get(self.pos >> 4).copied().unwrap_or(0);
            v |= (((w >> (self.pos & 15)) & 1) as u32) << i;
            self.pos += 1;
        }
        v
    }
}

#[derive(Serialize, Deserialize)]
pub struct A1600 {
    mode: u8,
    /// The previous frame's second half, windowed.
    overlap: Vec<f32>,
    /// The first frame's block isn't output (as Sunplus's decoder does).
    started: bool,
}

impl A1600 {
    pub fn new(mode: u8) -> Self {
        A1600 { mode: mode.clamp(4, 9), overlap: vec![0.0; 128], started: false }
    }

    pub fn frame_words(&self) -> usize {
        A1600_FRAME_WORDS[self.mode as usize - 4]
    }

    /// The frame's 128 coefficients.
    fn unpack(&self, words: &[u16]) -> [i32; 128] {
        let mut bits = Bits { words, pos: 0 };
        let mut scale = [0u8; 16];
        for s in &mut scale {
            *s = bits.read(4) as u8;
        }
        // Allocations, the loudest band first (the lower band on a tie).
        let mut alloc = [0u8; 16];
        let mut left = [true; 16];
        for &a in &A1600_ALLOC[self.mode as usize - 4] {
            let mut best = 0;
            let mut found = false;
            for b in 0..16 {
                if left[b] && (!found || scale[b] > scale[best]) {
                    best = b;
                    found = true;
                }
            }
            alloc[best] = a;
            left[best] = false;
        }
        let mut x = [0i32; 128];
        for band in 0..16 {
            let a = alloc[band];
            if !matches!(a, 1..=6 | 8) {
                continue;
            }
            let gain = A1600_GAIN[scale[band] as usize];
            let peak = bits.read(3) as usize;
            for i in 0..8 {
                x[band * 8 + i] = if i == peak {
                    if bits.read(1) != 0 { -gain } else { gain }
                } else {
                    let n = a1600_bits(a, peak, i);
                    if n == 0 {
                        0
                    } else {
                        let code = bits.read(n);
                        let level = match n {
                            4 => A1600_LEVEL4[(code >> 1) as usize],
                            3 => A1600_LEVEL3[(code >> 1) as usize],
                            2 => A1600_LEVEL2[(code >> 1) as usize],
                            _ => A1600_LEVEL1[0],
                        };
                        let v = (gain * level) >> 6;
                        if code & 1 != 0 { -v } else { v }
                    }
                };
            }
        }
        x
    }

    /// Decodes a frame, adding its 128 samples to `out` (none for the
    /// first).
    pub fn decode(&mut self, words: &[u16], out: &mut VecDeque<i16>) {
        let x = self.unpack(words);
        let table = dct4_table();
        let mut u = [0f32; 128];
        for (m, um) in u.iter_mut().enumerate() {
            let row = &table[m * 128..m * 128 + 128];
            *um = row.iter().zip(&x).map(|(c, &v)| c * v as f32).sum();
        }
        // The IMDCT's 256 outputs from the DCT-IV's 128, sine-windowed and
        // overlapped with the previous frame's; Sunplus's scale is -2.
        let y = |n: usize| -> f32 {
            let m = n + 64;
            if m < 128 {
                u[m]
            } else if m < 256 {
                -u[255 - m]
            } else {
                -u[m - 256]
            }
        };
        let win = |n: usize| ((std::f64::consts::PI / 256.0) * (n as f64 + 0.5)).sin() as f32;
        if self.started {
            for n in 0..128 {
                let s = -2.0 * (self.overlap[n] + y(n) * win(n));
                out.push_back(s.round().clamp(-32768.0, 32767.0) as i16);
            }
        }
        self.started = true;
        for n in 0..128 {
            self.overlap[n] = y(n + 128) * win(n + 128);
        }
    }
}

// ----- SACM-S480/S720 -----
//
// A CELP coder built on Federal Standard 1016: its ten line spectral pairs
// (the standard's scalar tables, 34 bits) and a 10th-order LPC synthesis
// filter, but its own excitation: an adaptive codebook (a pitch delay in
// thirds of a sample) and, in place of the standard's stochastic codebook,
// two pulses on a grid of every other sample. A frame is 144 bits in 4
// subframes of 40 samples (S720) or 60 (S480), read from the low bit of
// each byte up (bits 4-7 of the first byte, the mode, skipped):
//
//   10 LSP indices (3, 4, 4, 4, 4, 3, 3, 3, 3, 3 bits), then per subframe:
//   pitch delay (8 bits; 5 in subframes 2 and 4, a step from the one
//   before, +-16), pulse pair (9 bits), grid (1, odd or even samples),
//   pitch gain (4), pulse gain (4), the second pulse's sign (1), the
//   first's (1).
//
// Found 2026-11-15 by studying Sunplus's PC codec (S4872.DLL, research
// only): its frame unpacking, tables and fixed-point steps; the decoder
// below follows them except the LSP-to-LPC conversion, done here in
// floating point (the DLL's own fixed-point conversion is a little less
// exact). Against the DLL's output for the ROMs' sounds: see the doc.

/// FS-1016's LSP tables in Hz.
const S4872_LSP_HZ: [u16; 112] = [
    100, 170, 225, 250, 280, 340, 420, 500, // 1
    210, 235, 265, 295, 325, 360, 400, 440, 480, 520, 560, 610, 670, 740, 810, 880, // 2
    420, 460, 500, 540, 585, 640, 705, 775, 850, 950, 1050, 1150, 1250, 1350, 1450, 1550, // 3
    620, 660, 720, 795, 880, 970, 1080, 1170, 1270, 1370, 1470, 1570, 1670, 1770, 1870, 1970, // 4
    1000, 1050, 1130, 1210, 1285, 1350, 1430, 1510, 1590, 1670, 1750, 1850, 1950, 2050, 2150, 2250, // 5
    1470, 1570, 1690, 1830, 2000, 2200, 2400, 2600, // 6
    1800, 1880, 1960, 2100, 2300, 2480, 2700, 2900, // 7
    2225, 2400, 2525, 2650, 2800, 2950, 3150, 3350, // 8
    2760, 2880, 3000, 3100, 3200, 3310, 3430, 3550, // 9
    3190, 3270, 3350, 3420, 3490, 3590, 3710, 3830, // 10
];
const S4872_LSP_BITS: [u32; 10] = [3, 4, 4, 4, 4, 3, 3, 3, 3, 3];
/// The pulses' amplitude for each pulse gain code.
const S4872_PULSE_GAIN: [i32; 16] = [0, 1, 3, 13, 35, 64, 98, 136, 178, 224, 278, 340, 418, 520, 660, 870];
/// The adaptive codebook's interpolation, a delay of 1/3 and 2/3 of a
/// sample (8 taps each, Q15).
const S4872_INTERP: [[i32; 8]; 2] = [
    [-235, 1050, -3571, 12714, 26674, -5216, 1629, -383],
    [-383, 1629, -5216, 26674, 12714, -3571, 1050, -235],
];
/// Past excitation kept, in samples (the longest delay, 140, and 4 taps).
const S4872_HISTORY: usize = 148;

/// cos(pi k / 128) in Q14, truncated, as the codec's table has it.
fn s4872_cos_table() -> &'static [i32; 129] {
    static TABLE: OnceLock<[i32; 129]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0; 129];
        for (k, v) in t.iter_mut().enumerate().take(128) {
            *v = (16384.0 * (std::f64::consts::PI * k as f64 / 128.0).cos()) as i32;
        }
        t
    })
}

/// The pulse pair for a 9-bit code: the codes 0-434 are the pairs a < b of
/// 0-29 in order (0,1), (0,2) ... (28,29); the rest put no pulses.
fn s4872_pulse_pair(code: usize) -> Option<(usize, usize)> {
    let mut left = code;
    for a in 0..29 {
        let row = 29 - a;
        if left < row {
            return Some((a, a + 1 + left));
        }
        left -= row;
    }
    None
}

/// LSP frequencies (Q15 of pi) to LPC coefficients a1-a10 in Q11, for the
/// filter 1 / (1 + a1 z^-1 + ... + a10 z^-10).
fn s4872_lpc(lsp: &[i32; 10]) -> [i32; 10] {
    let cos = s4872_cos_table();
    let c: Vec<f64> = lsp
        .iter()
        .map(|&x| {
            let x = x.clamp(0, 32767);
            let (i, f) = ((x >> 8) as usize, (x & 255) << 7);
            (cos[i] + (((cos[i + 1] - cos[i]) * f * 2) >> 16)) as f64 / 16384.0
        })
        .collect();
    // P from the 1st, 3rd ... pairs times (1 + z^-1), Q from the others
    // times (1 - z^-1); A = (P + Q) / 2.
    let poly = |cs: &mut dyn Iterator<Item = f64>| -> [f64; 11] {
        let mut p = [0.0; 11];
        p[0] = 1.0;
        let mut len = 1;
        for cc in cs {
            for i in (0..len + 2).rev() {
                let mut v = p[i];
                if i >= 1 {
                    v -= 2.0 * cc * p[i - 1];
                }
                if i >= 2 {
                    v += p[i - 2];
                }
                p[i] = v;
            }
            len += 2;
        }
        p
    };
    let p = poly(&mut c.iter().copied().step_by(2));
    let q = poly(&mut c.iter().copied().skip(1).step_by(2));
    let mut a = [0; 10];
    for (k, ak) in a.iter_mut().enumerate() {
        let i = k + 1;
        let v = (p[i] + p[i - 1] + q[i] - q[i - 1]) / 2.0;
        *ak = ((v * 2048.0).round() as i32).clamp(-32768, 32767);
    }
    a
}

#[derive(Serialize, Deserialize)]
pub struct S4872 {
    /// Samples a subframe: 40 (S720) or 60 (S480), by each frame's mode.
    subframe: usize,
    /// The last frame's LSPs (Q15 of pi), none before the first.
    old_lsp: Option<[i32; 10]>,
    /// The pitch delay code of the last odd subframe (1 or 3), which the
    /// next one's step is from.
    delay_code: i32,
    /// Past excitation, the latest first.
    history: Vec<i16>,
    /// The synthesis filter's past outputs, the latest first.
    mem: [i16; 10],
}

impl Default for S4872 {
    fn default() -> Self {
        Self::new()
    }
}

impl S4872 {
    pub fn new() -> Self {
        S4872 { subframe: 40, old_lsp: None, delay_code: 0, history: vec![0; S4872_HISTORY], mem: [0; 10] }
    }

    /// Decodes a frame (9 words), adding its 160 or 240 samples to `out`.
    pub fn decode(&mut self, words: &[u16], out: &mut VecDeque<i16>) {
        let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        match bytes[0] & 0xf0 {
            0x10 => self.subframe = 60,
            0x20 => self.subframe = 40,
            _ => {}
        }
        let n = self.subframe;
        let mut pos = 0;
        let mut read = |bits: u32| -> i32 {
            let mut v = 0;
            for i in 0..bits {
                if pos == 4 {
                    pos = 8;
                }
                v |= (((bytes.get(pos >> 3).copied().unwrap_or(0) >> (pos & 7)) & 1) as i32) << i;
                pos += 1;
            }
            v
        };
        let mut lsp = [0; 10];
        let mut base = 0;
        for (i, l) in lsp.iter_mut().enumerate() {
            let hz = S4872_LSP_HZ[base + read(S4872_LSP_BITS[i]) as usize] as i32;
            *l = hz * 32768 / 4000;
            base += 1 << S4872_LSP_BITS[i];
        }
        let old = self.old_lsp.unwrap_or(lsp);
        let mut v = vec![0i32; n];
        for sf in 0..4 {
            let mut code = read(if sf & 1 != 0 { 5 } else { 8 });
            let pair = read(9) as usize;
            let grid = read(1) as usize;
            let pitch_gain = read(4) * 6144 / 5; // 0.075 steps, Q14
            let pulse = S4872_PULSE_GAIN[read(4) as usize];
            let sign2 = read(1) != 0;
            let sign1 = read(1) != 0;
            if sf & 1 != 0 {
                code = (code + self.delay_code - 16).clamp(0, 255);
            } else {
                self.delay_code = code;
            }
            // 13 to 76 2/3 samples in thirds, then 77 to 140 whole.
            let (delay, frac) = if code <= 191 { (code / 3 + 13, code % 3) } else { (code - 115, 0) };
            let delay = delay as usize;
            // The LSPs move from the last frame's to this one's: 1/4, 1/2,
            // 3/4 and all of the way.
            let (wo, wn) = [(24576, 8192), (16384, 16384), (8192, 24576), (0, 32767)][sf];
            let mut l = [0; 10];
            for i in 0..10 {
                l[i] = ((wo * old[i] * 2) >> 16) + ((wn * lsp[i] * 2) >> 16);
            }
            let a = s4872_lpc(&l);
            // The adaptive codebook: the excitation `delay` samples back,
            // repeating the vector itself where that is in this subframe.
            let h = &self.history;
            for i in 0..n {
                let past = |j: isize, v: &[i32]| -> i32 {
                    if j >= 0 { h.get(j as usize).copied().unwrap_or(0) as i32 } else { v[(-j - 1) as usize] }
                };
                v[i] = if frac == 0 {
                    past(delay as isize - 1 - i as isize, &v)
                } else {
                    let taps = &S4872_INTERP[frac as usize - 1];
                    (0..8)
                        .map(|k| (past(delay as isize + 3 - i as isize - k as isize, &v) * taps[k] * 2) >> 16)
                        .sum()
                };
            }
            // Two pulses, repeated every `delay` samples.
            let (mut p1, mut p2) = match s4872_pulse_pair(pair) {
                Some((a, b)) => (grid + 2 * a, grid + 2 * b),
                None => (usize::MAX, usize::MAX),
            };
            let g1 = if sign1 { -pulse } else { pulse };
            let g2 = if sign2 { -pulse } else { pulse };
            self.history.copy_within(0..S4872_HISTORY - n, n);
            for (i, &vi) in v.iter().enumerate() {
                let mut e = ((vi * pitch_gain * 2) >> 15) as i16;
                if i == p1 {
                    e = e.wrapping_add(g1 as i16);
                    p1 += delay;
                }
                if i == p2 {
                    e = e.wrapping_add(g2 as i16);
                    p2 += delay;
                }
                self.history[n - 1 - i] = e;
                let acc = (0..10).fold(0i32, |s, k| s.wrapping_add(2 * a[k] * self.mem[k] as i32));
                let y = (e as i32).wrapping_sub(acc.wrapping_add(2048) >> 12) as i16;
                self.mem.copy_within(0..9, 1);
                self.mem[0] = y;
                out.push_back((y.min(2047) as i32 * 16).max(-32768) as i16);
            }
        }
        self.old_lsp = Some(lsp);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1600_silence_and_length() {
        // A mode-5 frame of zeros: every band at scale 0 (gain 1), peak 0.
        let mut a = A1600::new(5);
        let mut out = VecDeque::new();
        for _ in 0..3 {
            a.decode(&[0; 12], &mut out);
        }
        assert_eq!(out.len(), 256);
        assert!(out.iter().all(|&s| s.abs() < 64));
    }

    #[test]
    fn s4872_frames() {
        // The silent frame many sounds start with: 160 quiet samples as
        // S720, 240 with the mode nibble set to S480.
        let mut words = [0xe727u16, 0xb6dd, 0x35b5, 0x0d00, 0x0003, 0x061a, 0xa000, 0x3861, 0x5769];
        let mut out = VecDeque::new();
        S4872::new().decode(&words, &mut out);
        assert_eq!(out.len(), 160);
        assert!(out.iter().all(|&s| s.abs() <= 1100));
        words[0] = (words[0] & 0xff0f) | 0x10;
        out.clear();
        S4872::new().decode(&words, &mut out);
        assert_eq!(out.len(), 240);
        assert_eq!(s4872_pulse_pair(0), Some((0, 1)));
        assert_eq!(s4872_pulse_pair(29), Some((1, 2)));
        assert_eq!(s4872_pulse_pair(434), Some((28, 29)));
        assert_eq!(s4872_pulse_pair(435), None);
    }
}

