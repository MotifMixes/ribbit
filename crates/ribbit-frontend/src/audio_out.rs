//! Host audio output: plays the voice engine's 8 kHz mix through the default
//! output device (cpal). Frontend only: nothing here affects emulation.
//!
//! The emulator's samples are raised to 32 kHz with MAME's 64-tap
//! interpolator (`ribbit_audio::Upsample4x`; whether the hardware
//! interpolates at all is UNVERIFIED - repeating samples instead sounds
//! harsh), queued, and resampled linearly to the device rate in the device
//! callback.
//!
//! Samples arrive in bursts (emulation chunks, UI frames), so the queue is a
//! jitter buffer: playback starts once `START` is queued, and if the queue
//! runs dry it fades out and waits to refill instead of playing a stream of
//! one-sample gaps (which is what crackles).
//!
//! The emulator is paced by the system clock and the device plays by its
//! own crystal; the two differ slightly (typically 0.01-0.1%), so at a fixed
//! ratio the queue would slowly drain (a gap every few minutes) or overfill
//! (dropped samples). `RateControl` nudges the resampling ratio, by at most
//! `MAX_RATE_ADJUST` (inaudible), to hold the queue at `TARGET`.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

/// Optional host-side "remaster" of the sound. The Leapster has one
/// speaker and an 8 kHz mix: there is no stereo or treble to recover, so
/// these are effects, like the picture upscaler, and change nothing
/// emulated.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum Enhance {
    /// As the hardware: mono, 8 kHz bandwidth.
    Off,
    /// Harmonic exciter: overtones above the 4 kHz the source can carry.
    Crisp,
    /// Crisp plus mono-compatible stereo widening.
    #[default]
    CrispStereo,
}

impl Enhance {
    pub const ALL: [Enhance; 3] = [Enhance::Off, Enhance::Crisp, Enhance::CrispStereo];

    pub fn label(self) -> &'static str {
        match self {
            Enhance::Off => "Original (mono)",
            Enhance::Crisp => "Crisp",
            Enhance::CrispStereo => "Crisp + wide stereo",
        }
    }
}

/// How the Leapster 2's PCM sound (its 32 kHz DAC stream, ribbit-core
/// voices.rs) reaches the speakers. Other sound is 8 kHz either way.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum PcmRate {
    /// The DAC's own 32 kHz samples, as the hardware plays them.
    #[default]
    Original32k,
    /// Averaged down to 8 kHz, then interpolated back up like other sound.
    Low8k,
}

impl PcmRate {
    pub const ALL: [PcmRate; 2] = [PcmRate::Original32k, PcmRate::Low8k];

    pub fn label(self) -> &'static str {
        match self {
            PcmRate::Original32k => "32 kHz (original)",
            PcmRate::Low8k => "8 kHz",
        }
    }
}

/// Whether the Leapster 2's 32 kHz PCM sound is low-passed on the way out
/// (`ribbit_audio::LowPass32k`, above 8.5 kHz). The emulated stream no
/// longer has the 10-16 kHz image this was added for (an emulation fault,
/// fixed), so the difference is small; "Unfiltered" plays the stream exactly
/// as emulated.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, serde::Serialize, serde::Deserialize)]
pub enum PcmClean {
    #[default]
    Clean,
    Unfiltered,
}

impl PcmClean {
    pub const ALL: [PcmClean; 2] = [PcmClean::Clean, PcmClean::Unfiltered];

    pub fn label(self) -> &'static str {
        match self {
            PcmClean::Clean => "Clean audio",
            PcmClean::Unfiltered => "Unfiltered (original)",
        }
    }
}

/// Rate of the queued (interpolated) audio.
const SOURCE_HZ: f64 = ribbit_audio::DAC_RATE_HZ as f64;
/// Queue limit (a quarter second) before old samples are dropped.
const MAX_QUEUE: usize = ribbit_audio::DAC_RATE_HZ as usize / 4;
/// Queued audio needed before playback (re)starts: 100 ms, enough to ride
/// out short emulation stalls (e.g. other programs loading the CPU).
const START: usize = ribbit_audio::DAC_RATE_HZ as usize * 100 / 1000;
/// Queue level the rate control holds while playing.
const TARGET: f64 = START as f64;
/// Largest resampling-ratio correction (0.5%: about 9 cents, below what
/// listeners notice on program material).
const MAX_RATE_ADJUST: f64 = 0.005;

/// Keeps the queue near `TARGET` by adjusting the resampling ratio in
/// proportion to the (smoothed) distance from it.
#[derive(Debug, Clone, Copy)]
struct RateControl {
    /// Queue level, low-pass filtered (callbacks are bursty).
    level: f64,
}

impl RateControl {
    fn new() -> Self {
        Self { level: TARGET }
    }

    /// Restart from the current level (playback (re)started).
    fn reset(&mut self, queued: usize) {
        self.level = queued as f64;
    }

    /// The ratio multiplier for one device callback, given the queue level
    /// and the callback length in seconds.
    fn factor(&mut self, queued: usize, seconds: f64) -> f64 {
        // About a 0.5 s time constant.
        let a = (seconds / 0.5).min(1.0);
        self.level += (queued as f64 - self.level) * a;
        // Full correction when the queue is 50% off target.
        let error = (self.level - TARGET) / TARGET;
        1.0 + (error * 2.0 * MAX_RATE_ADJUST).clamp(-MAX_RATE_ADJUST, MAX_RATE_ADJUST)
    }
}
/// Fade length (device frames) when the queue runs dry.
const FADE: f32 = 64.0;

#[derive(Default)]
struct Queue {
    samples: VecDeque<i16>,
    playing: bool,
    /// Times the queue ran dry while playing.
    underruns: u64,
    /// Times samples were dropped because the queue was full (the sound
    /// skips: the emulator ran ahead of the device, e.g. catching up).
    overflows: u64,
    /// The longest time between two device callbacks since last asked (a
    /// starved output thread shows here).
    longest_gap: std::time::Duration,
}

impl Queue {
    /// Drops the oldest samples beyond `MAX_QUEUE`.
    fn trim(&mut self) {
        if self.samples.len() > MAX_QUEUE {
            self.overflows += 1;
            let extra = self.samples.len() - MAX_QUEUE;
            self.samples.drain(..extra);
        }
    }
}

pub struct AudioOut {
    queue: Arc<Mutex<Queue>>,
    enhance: Arc<AtomicU8>,
    _stream: cpal::Stream,
}

/// The producer end of the output queue, for the emulation thread (the
/// device stream itself stays on the UI thread).
pub struct AudioSink {
    queue: Arc<Mutex<Queue>>,
    upsample: ribbit_audio::Upsample4x,
}

impl AudioSink {
    /// Queues new 8 kHz samples from the emulator at `gain` (0..=1).
    pub fn push(&mut self, samples: &[i16], gain: f32) {
        let mut q = self.queue.lock().unwrap();
        for &s in samples {
            q.samples.extend(self.upsample.push(s).map(|v| (v as f32 * gain) as i16));
        }
        q.trim();
    }

    /// Queues 32 kHz samples as they are (the PCM DAC stream).
    pub fn push_32k(&mut self, samples: &[i16], gain: f32) {
        let mut q = self.queue.lock().unwrap();
        q.samples.extend(samples.iter().map(|&s| (s as f32 * gain) as i16));
        q.trim();
    }
}

impl AudioOut {
    /// Drops queued audio (pause, reset).
    pub fn clear(&mut self) {
        let mut q = self.queue.lock().unwrap();
        q.samples.clear();
        q.playing = false;
    }

    pub fn set_enhance(&self, e: Enhance) {
        self.enhance.store(e as u8, Ordering::Relaxed);
    }

    /// A producer for this output (one per emulation thread).
    pub fn sink(&self) -> AudioSink {
        AudioSink { queue: self.queue.clone(), upsample: Default::default() }
    }

    /// Times playback ran out of samples (each is an audible gap), and
    /// times queued samples were dropped (each an audible skip).
    pub fn glitches(&self) -> (u64, u64) {
        let q = self.queue.lock().unwrap();
        (q.underruns, q.overflows)
    }

    /// The longest time between device callbacks since the last call.
    pub fn take_longest_gap(&self) -> std::time::Duration {
        std::mem::take(&mut self.queue.lock().unwrap().longest_gap)
    }

    /// Samples waiting and whether the device is playing them (for the log).
    pub fn queue_state(&self) -> (usize, bool) {
        let q = self.queue.lock().unwrap();
        (q.samples.len(), q.playing)
    }

    /// Opens the default output device; None (with a log line) if there is
    /// no usable device, in which case the emulator simply runs silently.
    pub fn open() -> Option<Self> {
        match Self::try_open() {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::warn!(target: "ribbit::audio", "no audio output: {e:#}");
                None
            }
        }
    }

    fn try_open() -> anyhow::Result<Self> {
        let host = cpal::default_host();
        let device = host.default_output_device().ok_or_else(|| anyhow::anyhow!("no default output device"))?;
        let supported = device.default_output_config()?;
        let format = supported.sample_format();
        let config: cpal::StreamConfig = supported.into();
        let device_hz = config.sample_rate;
        let queue = Arc::new(Mutex::new(Queue::default()));
        let enhance = Arc::new(AtomicU8::new(Enhance::default() as u8));
        let (q, e) = (queue.clone(), enhance.clone());
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, q, e)?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, q, e)?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, q, e)?,
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, q, e)?,
            other => anyhow::bail!("unsupported device sample format {other}"),
        };
        stream.play()?;
        tracing::info!(target: "ribbit::audio", device_hz, channels = config.channels, "audio output open");
        Ok(Self { queue, enhance, _stream: stream })
    }
}

/// First-order high-pass filter.
struct HighPass {
    a: f32,
    x: f32,
    y: f32,
}

impl HighPass {
    fn new(cutoff_hz: f32, rate_hz: f32) -> Self {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        Self { a: rc / (rc + 1.0 / rate_hz), x: 0.0, y: 0.0 }
    }

    fn run(&mut self, x: f32) -> f32 {
        self.y = self.a * (self.y + x - self.x);
        self.x = x;
        self.y
    }
}

/// The `Enhance` effects at the device rate: mono in, (left, right) out.
struct Remaster {
    band: HighPass,
    air: HighPass,
    delay: Vec<f32>,
    at: usize,
}

impl Remaster {
    fn new(rate_hz: f32) -> Self {
        Self {
            band: HighPass::new(2_000.0, rate_hz),
            air: HighPass::new(4_000.0, rate_hz),
            // 12 ms: wide without an audible echo.
            delay: vec![0.0; ((rate_hz * 0.012) as usize).max(1)],
            at: 0,
        }
    }

    fn run(&mut self, v: f32, mode: u8) -> (f32, f32) {
        if mode == Enhance::Off as u8 {
            return (v, v);
        }
        // Exciter: saturate the upper band, keep only the new overtones
        // above 4 kHz, mix them in.
        let band = self.band.run(v);
        let sat = (band * 4.0).tanh() * 0.25;
        let crisp = v + 0.8 * self.air.run(sat);
        if mode != Enhance::CrispStereo as u8 {
            return (crisp, crisp);
        }
        // Complementary combs: left + right stays the mono signal.
        let d = self.delay[self.at];
        self.delay[self.at] = crisp;
        self.at = (self.at + 1) % self.delay.len();
        ((crisp + 0.35 * d) / 1.35, (crisp - 0.35 * d) / 1.35)
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<Mutex<Queue>>,
    enhance: Arc<AtomicU8>,
) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let step = SOURCE_HZ / config.sample_rate as f64;
    // Resampler state: the two source samples being interpolated and the
    // position between them; `level` fades in/out around gaps.
    let (mut prev, mut next, mut frac) = (0.0f32, 0.0f32, 0.0f64);
    let mut level = 0.0f32;
    let mut rate = RateControl::new();
    let mut remaster = Remaster::new(config.sample_rate as f32);
    // This callback's source samples, taken out of the queue at its start.
    let mut taken: Vec<i16> = Vec::new();
    let mut last_callback: Option<std::time::Instant> = None;
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mode = enhance.load(Ordering::Relaxed);
            let frames = data.len() / channels.max(1);
            // The queue is locked only to take this callback's samples:
            // the emulation thread pushes to it, and a callback preempted
            // while holding it (a busy system) would hold that thread up.
            let now = std::time::Instant::now();
            let gap = last_callback.map(|t| now - t);
            last_callback = Some(now);
            let playing = {
                let mut q = queue.lock().unwrap();
                if let Some(g) = gap {
                    q.longest_gap = q.longest_gap.max(g);
                }
                if !q.playing && q.samples.len() >= START {
                    q.playing = true;
                    rate.reset(q.samples.len());
                }
                let step = step * rate.factor(q.samples.len(), frames as f64 * step / SOURCE_HZ);
                taken.clear();
                if q.playing {
                    // How many the resampler will step over in `frames`.
                    let (mut f, mut need) = (frac, 0usize);
                    for _ in 0..frames {
                        f += step;
                        need += f as usize;
                        f -= f.floor();
                    }
                    let n = need.min(q.samples.len());
                    taken.extend(q.samples.drain(..n));
                    if n < need {
                        // Dry: hold the last value, fade out, refill.
                        q.playing = false;
                        q.underruns += 1;
                    }
                }
                (q.playing || !taken.is_empty(), step)
            };
            let (mut playing, step) = playing;
            let mut source = taken.iter();
            for frame in data.chunks_mut(channels) {
                if playing {
                    frac += step;
                    while frac >= 1.0 {
                        frac -= 1.0;
                        prev = next;
                        match source.next() {
                            Some(&s) => next = s as f32 / 32768.0,
                            None => {
                                playing = false;
                                break;
                            }
                        }
                    }
                }
                level = if playing { (level + 1.0 / FADE).min(1.0) } else { (level - 1.0 / FADE).max(0.0) };
                let v = (prev + (next - prev) * frac as f32) * level;
                let (l, r) = remaster.run(v, mode);
                let (l, r) = (l.clamp(-1.0, 1.0), r.clamp(-1.0, 1.0));
                match frame {
                    [left, right, rest @ ..] => {
                        *left = T::from_sample(l);
                        *right = T::from_sample(r);
                        rest.iter_mut().for_each(|s| *s = T::from_sample(0.5 * (l + r)));
                    }
                    [only] => *only = T::from_sample(0.5 * (l + r)),
                    [] => {}
                }
            }
        },
        |err| tracing::warn!(target: "ribbit::audio", "audio stream error: {err}"),
        None,
    )?;
    Ok(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A producer and a device whose clocks differ by 0.2%, with bursty
    /// delivery (2 ms chunks in, 10 ms callbacks out): the queue settles
    /// near the target and never runs dry or overflows.
    #[test]
    fn rate_control_absorbs_clock_drift() {
        for drift in [-0.002, 0.0, 0.002] {
            let device_hz = 48_000.0;
            let base = SOURCE_HZ / device_hz;
            let mut rate = RateControl::new();
            let mut queued = START as f64;
            let (mut lo, mut hi) = (f64::MAX, 0.0f64);
            // 10 minutes of 10 ms callbacks.
            for n in 0..60_000 {
                // Five 2 ms producer chunks per callback, at the drifted rate.
                queued += SOURCE_HZ * (1.0 + drift) * 0.010;
                let step = base * rate.factor(queued as usize, 480.0 * base / SOURCE_HZ);
                queued -= 480.0 * step;
                if n > 6_000 {
                    lo = lo.min(queued);
                    hi = hi.max(queued);
                }
            }
            assert!(lo > 0.5 * TARGET && hi < 1.5 * TARGET && hi < MAX_QUEUE as f64, "drift {drift}: {lo}..{hi}");
        }
    }

    #[test]
    fn remaster_is_bounded_and_mono_compatible() {
        let (mut crisp, mut wide) = (Remaster::new(48_000.0), Remaster::new(48_000.0));
        for n in 0..48_000 {
            // A loud 1 kHz tone with a square-ish edge: worst case for the exciter.
            let v = (n as f32 * 2.0 * std::f32::consts::PI * 1_000.0 / 48_000.0).sin().signum() * 0.9;
            let (l, r) = crisp.run(v, Enhance::Crisp as u8);
            assert!(l.is_finite() && l == r && l.abs() < 1.5, "{l}");
            let (wl, wr) = wide.run(v, Enhance::CrispStereo as u8);
            assert!(wl.is_finite() && wr.is_finite());
            // L + R is the (scaled) crisp mono signal: nothing cancels in mono.
            assert!(((wl + wr) * 1.35 / 2.0 - l).abs() < 1e-4, "{wl} {wr} {l}");
        }
        let mut off = Remaster::new(48_000.0);
        assert_eq!(off.run(0.25, Enhance::Off as u8), (0.25, 0.25));
    }
}
