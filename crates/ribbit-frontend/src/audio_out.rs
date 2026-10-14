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

/// Rate of the queued (interpolated) audio.
const SOURCE_HZ: f64 = ribbit_audio::DAC_RATE_HZ as f64;
/// Queue limit (a quarter second) before old samples are dropped.
const MAX_QUEUE: usize = ribbit_audio::DAC_RATE_HZ as usize / 4;
/// Queued audio needed before playback (re)starts: 120 ms, enough to ride
/// out short emulation stalls (e.g. other programs loading the CPU).
const START: usize = ribbit_audio::DAC_RATE_HZ as usize * 120 / 1000;
/// Fade length (device frames) when the queue runs dry.
const FADE: f32 = 64.0;

#[derive(Default)]
struct Queue {
    samples: VecDeque<i16>,
    playing: bool,
    /// Times the queue ran dry while playing.
    underruns: u64,
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
        while q.samples.len() > MAX_QUEUE {
            q.samples.pop_front();
        }
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

    /// Times playback ran out of samples (each is an audible gap).
    pub fn underruns(&self) -> u64 {
        self.queue.lock().unwrap().underruns
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
    let mut remaster = Remaster::new(config.sample_rate as f32);
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mode = enhance.load(Ordering::Relaxed);
            let mut q = queue.lock().unwrap();
            if !q.playing && q.samples.len() >= START {
                q.playing = true;
            }
            for frame in data.chunks_mut(channels) {
                if q.playing {
                    frac += step;
                    while frac >= 1.0 {
                        frac -= 1.0;
                        prev = next;
                        match q.samples.pop_front() {
                            Some(s) => next = s as f32 / 32768.0,
                            None => {
                                // Dry: hold the last value, fade out, refill.
                                q.playing = false;
                                q.underruns += 1;
                                break;
                            }
                        }
                    }
                }
                level = if q.playing { (level + 1.0 / FADE).min(1.0) } else { (level - 1.0 / FADE).max(0.0) };
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
