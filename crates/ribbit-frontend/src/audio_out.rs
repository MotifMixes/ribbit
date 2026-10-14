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
use std::sync::{Arc, Mutex};

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
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, queue.clone())?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, queue.clone())?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, queue.clone())?,
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, queue.clone())?,
            other => anyhow::bail!("unsupported device sample format {other}"),
        };
        stream.play()?;
        tracing::info!(target: "ribbit::audio", device_hz, channels = config.channels, "audio output open");
        Ok(Self { queue, _stream: stream })
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<Mutex<Queue>>,
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
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
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
                let out = T::from_sample(v);
                frame.iter_mut().for_each(|s| *s = out);
            }
        },
        |err| tracing::warn!(target: "ribbit::audio", "audio stream error: {err}"),
        None,
    )?;
    Ok(stream)
}
