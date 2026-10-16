//! Audio hardware model.
//!
//! Sample-format primitives shared by the emulator and the host output: A-law,
//! the emulator-to-host sample ring and the 8 to 32 kHz output interpolator.
//! The voice engine itself lives in `ribbit-core` (io/voices.rs).

/// Output rate of the DAC path observed by MAME (control value 0x7f01).
/// UNVERIFIED on hardware.
pub const DAC_RATE_HZ: u32 = 32_000;

/// Decodes one ITU-T G.711 A-law byte to 16-bit linear PCM (13-bit
/// magnitude scaled by 8, the reference `alaw2linear` output).
pub fn alaw_to_linear(a: u8) -> i16 {
    let a = a ^ 0x55;
    let mut t = ((a & 0x0F) as i16) << 4;
    let seg = (a & 0x70) >> 4;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= seg - 1;
        }
    }
    if a & 0x80 != 0 { t } else { -t }
}

/// MAME's 4x polyphase interpolator for the 8 kHz legacy mix (64-tap
/// Kaiser-windowed sinc, beta 6, 3.9 kHz cutoff, each phase normalised to
/// unity DC gain). Coefficients from MAME `leapster_a.cpp` (master da56c5ac,
/// BSD-3-Clause). MAME: "Whether the hardware interpolates or simply repeats
/// samples is unverified" - UNVERIFIED; used for host output only.
const INTERP: [[f64; 16]; 4] = [
    [
        -0.0005094, 0.0025431, -0.0068394, 0.0143336, -0.0260942, 0.0440515, -0.0745725, 0.1565483, 0.9505791,
        -0.0801511, 0.0273811, -0.0095530, 0.0023707, 0.0001665, -0.0006333, 0.0003789,
    ],
    [
        -0.0010008, 0.0042238, -0.0112934, 0.0245213, -0.0474114, 0.0872235, -0.1680786, 0.4713854, 0.7696265,
        -0.1867429, 0.0869947, -0.0441397, 0.0216283, -0.0095494, 0.0035005, -0.0008878,
    ],
    [
        -0.0008878, 0.0035005, -0.0095494, 0.0216283, -0.0441397, 0.0869947, -0.1867429, 0.7696265, 0.4713854,
        -0.1680786, 0.0872235, -0.0474114, 0.0245213, -0.0112934, 0.0042238, -0.0010008,
    ],
    [
        0.0003789, -0.0006333, 0.0001665, 0.0023707, -0.0095530, 0.0273811, -0.0801511, 0.9505791, 0.1565483,
        -0.0745725, 0.0440515, -0.0260942, 0.0143336, -0.0068394, 0.0025431, -0.0005094,
    ],
];

/// One step of `Upsample4x` on caller-held state (most recent input first),
/// for emulated state that has to be saved.
pub fn upsample4x(history: &mut [f64; 16], sample: i16) -> [i16; 4] {
    history.copy_within(0..15, 1);
    history[0] = sample as f64;
    INTERP.map(|taps| {
        let v: f64 = taps.iter().zip(history.iter()).map(|(t, x)| t * x).sum();
        v.round().clamp(-32768.0, 32767.0) as i16
    })
}

/// Raises 8 kHz samples to 32 kHz (four outputs per input) as MAME does.
#[derive(Debug, Clone, Default)]
pub struct Upsample4x {
    /// Most recent input first.
    history: [f64; 16],
}

impl Upsample4x {
    pub fn reset(&mut self) {
        self.history = [0.0; 16];
    }

    /// Takes one 8 kHz sample and returns the next four 32 kHz samples.
    pub fn push(&mut self, sample: i16) -> [i16; 4] {
        upsample4x(&mut self.history, sample)
    }
}

/// A host-side low-pass filter for 32 kHz sound: a 95-tap windowed-sinc FIR
/// (Blackman window) with its cutoff at 8.5 kHz, flat to about 7.5 kHz and
/// below -70 dB from about 9.5 kHz. Delay: 47 samples (1.5 ms).
///
/// Why: the Leapster 2's 32 kHz PCM stream carried a mirror image of its
/// content (10-16 kHz, about 2% of the energy). That turned out to be an
/// emulation fault (XY memory's halfword order, ribbit-cpu xy.rs), now
/// fixed, so the emulated stream has almost nothing above 8 kHz and this
/// filter changes little; the frontend still offers it (default) or the
/// stream exactly as emulated. Not part of the emulation.
#[derive(Debug, Clone)]
pub struct LowPass32k {
    taps: Vec<f64>,
    /// Ring of the last `taps.len()` inputs; `next` is the oldest.
    history: Vec<f64>,
    next: usize,
}

impl Default for LowPass32k {
    fn default() -> Self {
        Self::new()
    }
}

impl LowPass32k {
    pub const TAPS: usize = 95;
    pub const CUTOFF_HZ: f64 = 8_500.0;

    pub fn new() -> Self {
        let n = Self::TAPS;
        let m = (n - 1) as f64;
        let fc = Self::CUTOFF_HZ / DAC_RATE_HZ as f64;
        let mut taps: Vec<f64> = (0..n)
            .map(|i| {
                let t = i as f64 - m / 2.0;
                let sinc = if t == 0.0 { 2.0 * fc } else { (2.0 * std::f64::consts::PI * fc * t).sin() / (std::f64::consts::PI * t) };
                let w = 0.42 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / m).cos() + 0.08 * (4.0 * std::f64::consts::PI * i as f64 / m).cos();
                sinc * w
            })
            .collect();
        let sum: f64 = taps.iter().sum();
        taps.iter_mut().for_each(|t| *t /= sum);
        Self { taps, history: vec![0.0; n], next: 0 }
    }

    pub fn reset(&mut self) {
        self.history.fill(0.0);
        self.next = 0;
    }

    /// Filters one sample.
    pub fn push(&mut self, sample: i16) -> i16 {
        let n = self.taps.len();
        self.history[self.next] = sample as f64;
        self.next = (self.next + 1) % n;
        // history[next] is now the oldest input; taps are symmetric.
        let mut acc = 0.0;
        for (k, t) in self.taps.iter().enumerate() {
            acc += t * self.history[(self.next + k) % n];
        }
        acc.round().clamp(-32768.0, 32767.0) as i16
    }
}

/// Fixed-capacity ring buffer between the emulation thread and a host audio
/// callback. Overflow drops the oldest samples and is counted, never hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleRing {
    buf: Vec<i16>,
    read: usize,
    len: usize,
    pub overruns: u64,
    pub underruns: u64,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self { buf: vec![0; capacity], read: 0, len: 0, overruns: 0, underruns: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, sample: i16) {
        let cap = self.buf.len();
        if self.len == cap {
            self.read = (self.read + 1) % cap;
            self.len -= 1;
            self.overruns += 1;
        }
        self.buf[(self.read + self.len) % cap] = sample;
        self.len += 1;
    }

    /// Fills `out`; missing samples are silence and counted as underruns.
    pub fn pop_into(&mut self, out: &mut [i16]) {
        for slot in out {
            if self.len == 0 {
                *slot = 0;
                self.underruns += 1;
            } else {
                *slot = self.buf[self.read];
                self.read = (self.read + 1) % self.buf.len();
                self.len -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Steady-state gain of the low-pass filter for a sine at `hz`.
    fn lowpass_gain(hz: f64) -> f64 {
        let mut f = LowPass32k::new();
        let amp = 10_000.0;
        let mut peak: f64 = 0.0;
        for i in 0..4000 {
            let x = (amp * (2.0 * std::f64::consts::PI * hz * i as f64 / 32_000.0).sin()).round() as i16;
            let y = f.push(x) as f64;
            if i > 1000 {
                peak = peak.max(y.abs());
            }
        }
        peak / amp
    }

    #[test]
    fn lowpass_keeps_the_band_and_removes_the_mirror() {
        for hz in [100.0, 1_000.0, 5_000.0, 7_000.0] {
            let g = lowpass_gain(hz);
            assert!((g - 1.0).abs() < 0.01, "{hz} Hz passes: {g}");
        }
        for hz in [10_000.0, 12_000.0, 15_000.0] {
            let g = lowpass_gain(hz);
            assert!(g < 0.001, "{hz} Hz is removed: {g}");
        }
    }

    #[test]
    fn alaw_matches_g711_reference_points() {
        assert_eq!(alaw_to_linear(0xD5), 8);
        assert_eq!(alaw_to_linear(0x55), -8);
        assert_eq!(alaw_to_linear(0xAA), 32256);
        assert_eq!(alaw_to_linear(0x2A), -32256);
        assert_eq!(alaw_to_linear(0x80), 5504);
        assert_eq!(alaw_to_linear(0x00), -5504);
    }

    #[test]
    fn alaw_is_monotonic_within_each_sign() {
        // Positive codes ordered by magnitude: XOR 0xD5 gives 0..=0x7F.
        let mut prev = i16::MIN;
        for m in 0u8..=0x7F {
            let v = alaw_to_linear(m ^ 0xD5);
            assert!(v > prev, "code {m:#x}");
            prev = v;
        }
    }

    #[test]
    fn upsampler_passes_dc_and_delays_by_about_1ms() {
        let mut u = Upsample4x::default();
        let mut out = Vec::new();
        for _ in 0..16 {
            out.extend(u.push(1000));
        }
        // Each phase has unity DC gain: a settled constant comes out unchanged.
        assert!(out[60..].iter().all(|&s| (s - 1000).abs() <= 1), "{:?}", &out[60..]);
        // An impulse peaks 8 input samples (1 ms) later.
        u.reset();
        let mut imp = Vec::new();
        for i in 0..16 {
            imp.extend(u.push(if i == 0 { 10000 } else { 0 }));
        }
        let peak = (0..imp.len()).max_by_key(|&i| imp[i]).unwrap();
        assert_eq!(peak / 4, 8, "{imp:?}");
    }

    #[test]
    fn ring_counts_overruns_and_underruns() {
        let mut ring = SampleRing::new(3);
        for s in 1..=4 {
            ring.push(s);
        }
        assert_eq!(ring.overruns, 1);
        let mut out = [0i16; 4];
        ring.pop_into(&mut out);
        assert_eq!(out, [2, 3, 4, 0]);
        assert_eq!(ring.underruns, 1);
    }
}
