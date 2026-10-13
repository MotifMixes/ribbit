//! Host audio output: plays the voice engine's 8 kHz mix through the default
//! output device (cpal). Frontend only: nothing here affects emulation.
//!
//! The emulator pushes 8 kHz samples after every frame; the device callback
//! resamples them to the device rate with linear interpolation (MAME uses a
//! 64-tap interpolator; whether the hardware interpolates at all is UNKNOWN)
//! and plays silence when the queue runs dry. The queue is kept short so
//! sound stays in sync with the picture.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// Source rate of the voice engine.
const SOURCE_HZ: f64 = 8_000.0;
/// Queue limit (seconds of audio) before old samples are dropped.
const MAX_QUEUE: usize = 8_000 / 4;

pub struct AudioOut {
    queue: Arc<Mutex<VecDeque<i16>>>,
    _stream: cpal::Stream,
    pub dropped: u64,
}

impl AudioOut {
    /// Drops queued audio (pause, reset).
    pub fn clear(&mut self) {
        self.queue.lock().unwrap().clear();
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
        let queue = Arc::new(Mutex::new(VecDeque::<i16>::with_capacity(MAX_QUEUE)));
        let stream = match format {
            cpal::SampleFormat::F32 => build::<f32>(&device, &config, queue.clone())?,
            cpal::SampleFormat::I16 => build::<i16>(&device, &config, queue.clone())?,
            cpal::SampleFormat::U16 => build::<u16>(&device, &config, queue.clone())?,
            cpal::SampleFormat::I32 => build::<i32>(&device, &config, queue.clone())?,
            other => anyhow::bail!("unsupported device sample format {other}"),
        };
        stream.play()?;
        tracing::info!(target: "ribbit::audio", device_hz, channels = config.channels, "audio output open");
        Ok(Self { queue, _stream: stream, dropped: 0 })
    }

    /// Queues new 8 kHz samples from the emulator at `gain` (0..=1).
    pub fn push(&mut self, samples: &[i16], gain: f32) {
        let mut q = self.queue.lock().unwrap();
        q.extend(samples.iter().map(|&s| (s as f32 * gain) as i16));
        while q.len() > MAX_QUEUE {
            q.pop_front();
            self.dropped += 1;
        }
    }
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    queue: Arc<Mutex<VecDeque<i16>>>,
) -> anyhow::Result<cpal::Stream>
where
    T: cpal::SizedSample + cpal::FromSample<f32>,
{
    let channels = config.channels as usize;
    let step = SOURCE_HZ / config.sample_rate as f64;
    // Resampler state: the two source samples being interpolated and the
    // position between them.
    let (mut prev, mut next, mut frac) = (0.0f32, 0.0f32, 0.0f64);
    let stream = device.build_output_stream(
        *config,
        move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            let mut q = queue.lock().unwrap();
            for frame in data.chunks_mut(channels) {
                frac += step;
                while frac >= 1.0 {
                    frac -= 1.0;
                    prev = next;
                    next = q.pop_front().map_or(0.0, |s| s as f32 / 32768.0);
                }
                let v = prev + (next - prev) * frac as f32;
                let out = T::from_sample(v);
                frame.iter_mut().for_each(|s| *s = out);
            }
        },
        |err| tracing::warn!(target: "ribbit::audio", "audio stream error: {err}"),
        None,
    )?;
    Ok(stream)
}
