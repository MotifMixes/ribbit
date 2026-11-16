//! The sound the OpenSL ES players make: each clip (WAV or Ogg Vorbis from an asset descriptor)
//! decoded once to mono at `RATE`, a voice per player with its position, loop and volume, and
//! the voices playing mixed in step with the clock (`Runtime::mix`).

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

/// The mix's rate: the app's output stream (mono, 32 kHz).
pub const RATE: u32 = 32_000;
/// The most mixed at once: after a long stall the rest is skipped, not played late.
const MAX_BURST: u64 = RATE as u64 / 4;

#[derive(Default)]
pub struct Mixer {
    /// Decoded clips by a hash of their bytes.
    cache: HashMap<u64, Arc<Vec<f32>>>,
    /// A voice per audio player object.
    pub voices: BTreeMap<u32, Voice>,
    /// Samples mixed since the start, and the mix not yet taken.
    made: u64,
    pub out: Vec<i16>,
    /// Players whose clip ran out since the last `end_players`.
    pub ended: Vec<u32>,
}

pub struct Voice {
    pub pcm: Arc<Vec<f32>>,
    pub pos: usize,
    pub playing: bool,
    /// Loop between these samples (`SLSeekItf::SetLoop`).
    pub looping: Option<(usize, usize)>,
    pub gain: f32,
    pub muted: bool,
}

impl Voice {
    pub fn length(&self) -> Duration {
        Duration::from_secs_f64(self.pcm.len() as f64 / RATE as f64)
    }

    pub fn position(&self) -> Duration {
        Duration::from_secs_f64(self.pos as f64 / RATE as f64)
    }
}

/// A position in milliseconds, as a sample index.
pub fn sample_at(ms: u32) -> usize {
    (ms as u64 * RATE as u64 / 1000) as usize
}

impl Mixer {
    /// A voice for `player` from a clip's bytes. A clip that can't be decoded plays a second
    /// of silence (GUESS), so the game still hears it end.
    pub fn add(&mut self, player: u32, clip: &[u8]) {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        clip.hash(&mut h);
        let pcm = self.cache.entry(h.finish()).or_insert_with(|| Arc::new(decode(clip).unwrap_or_else(|| vec![0.0; RATE as usize]))).clone();
        self.voices.insert(player, Voice { pcm, pos: 0, playing: false, looping: None, gain: 1.0, muted: false });
    }

    /// Mixes the voices up to `now` on the game's clock.
    pub fn mix(&mut self, now: Duration) {
        let due = (now.as_secs_f64() * RATE as f64) as u64;
        if due <= self.made {
            return;
        }
        let n = (due - self.made).min(MAX_BURST) as usize;
        self.made = due;
        let mut acc = vec![0f32; n];
        for (&player, v) in &mut self.voices {
            if !v.playing {
                continue;
            }
            let gain = if v.muted { 0.0 } else { v.gain };
            for a in acc.iter_mut() {
                if let Some((start, end)) = v.looping
                    && v.pos >= end.min(v.pcm.len())
                {
                    v.pos = start.min(end);
                }
                let Some(&s) = v.pcm.get(v.pos) else {
                    v.playing = false;
                    self.ended.push(player);
                    break;
                };
                *a += s * gain;
                v.pos += 1;
            }
        }
        self.out.extend(acc.iter().map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16));
        // Nobody taking it (no host output): keep only the last half second.
        if self.out.len() > RATE as usize {
            self.out.drain(..self.out.len() - RATE as usize / 2);
        }
    }
}

/// A WAV (8- or 16-bit PCM) or Ogg Vorbis clip, as mono at `RATE`.
pub fn decode(d: &[u8]) -> Option<Vec<f32>> {
    let (rate, mono) = if d.starts_with(b"RIFF") { wav(d)? } else if d.starts_with(b"OggS") { ogg(d)? } else { return None };
    Some(resample(&mono, rate))
}

fn wav(d: &[u8]) -> Option<(u32, Vec<f32>)> {
    let u16_at = |i: usize| d.get(i..i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32_at = |i: usize| d.get(i..i + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    let (mut i, mut fmt, mut data) = (12, None, None);
    while let (Some(id), Some(n)) = (d.get(i..i + 4), u32_at(i + 4)) {
        let body = i + 8..(i + 8 + n as usize).min(d.len());
        match id {
            b"fmt " => fmt = Some((u16_at(i + 8)?, u16_at(i + 10)?, u32_at(i + 12)?, u16_at(i + 22)?)),
            b"data" => data = Some(&d[body]),
            _ => {}
        }
        i += 8 + n as usize + (n as usize & 1);
    }
    let ((format, channels, rate, bits), data) = (fmt?, data?);
    let ch = channels.max(1) as usize;
    // 1 PCM, 0xFFFE extensible (PCM assumed)
    if !matches!(format, 1 | 0xFFFE) {
        return None;
    }
    let frames: Vec<f32> = match bits {
        8 => data.chunks_exact(ch).map(|f| f.iter().map(|&b| (b as f32 - 128.0) / 128.0).sum::<f32>() / ch as f32).collect(),
        16 => data.chunks_exact(2 * ch).map(|f| f.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0).sum::<f32>() / ch as f32).collect(),
        _ => return None,
    };
    Some((rate, frames))
}

fn ogg(d: &[u8]) -> Option<(u32, Vec<f32>)> {
    let mut r = lewton::inside_ogg::OggStreamReader::new(std::io::Cursor::new(d)).ok()?;
    let (rate, ch) = (r.ident_hdr.audio_sample_rate, r.ident_hdr.audio_channels.max(1) as usize);
    let mut out = Vec::new();
    // A damaged page ends the clip where it is.
    while let Ok(Some(p)) = r.read_dec_packet_itl() {
        out.extend(p.chunks_exact(ch).map(|f| f.iter().map(|&s| s as f32).sum::<f32>() / (ch as f32 * 32768.0)));
    }
    Some((rate, out))
}

/// To `RATE`: a windowed-sinc low-pass below the lower Nyquist frequency, read at the output's
/// sample times (linear between input samples).
fn resample(x: &[f32], from: u32) -> Vec<f32> {
    if from == RATE || from == 0 {
        return x.to_vec();
    }
    let filtered;
    let x = if from > RATE {
        // Cut-off at 0.45 of the output rate, 31 taps, Blackman window.
        let fc = 0.45 * RATE as f32 / from as f32;
        const N: usize = 31;
        let taps: Vec<f32> = (0..N)
            .map(|i| {
                let t = i as f32 - (N / 2) as f32;
                let sinc = if t == 0.0 { 2.0 * fc } else { (2.0 * std::f32::consts::PI * fc * t).sin() / (std::f32::consts::PI * t) };
                let w = 0.42 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos() + 0.08 * (4.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos();
                sinc * w
            })
            .collect();
        filtered = (0..x.len())
            .map(|n| taps.iter().enumerate().map(|(k, &c)| c * (n + k).checked_sub(N / 2).and_then(|j| x.get(j)).copied().unwrap_or(0.0)).sum())
            .collect::<Vec<f32>>();
        &filtered
    } else {
        x
    };
    let step = from as f64 / RATE as f64;
    let n = (x.len() as f64 / step) as usize;
    (0..n)
        .map(|i| {
            let p = i as f64 * step;
            let (j, f) = (p as usize, (p.fract()) as f32);
            let a = x.get(j).copied().unwrap_or(0.0);
            let b = x.get(j + 1).copied().unwrap_or(a);
            a + (b - a) * f
        })
        .collect()
}
