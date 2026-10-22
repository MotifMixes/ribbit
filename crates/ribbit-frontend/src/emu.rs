//! The emulator on its own thread, so emulation gets a whole core and the
//! window stays at the display's frame rate even when emulation runs slower
//! than real time. The thread runs the machine in short chunks, paced to real
//! time (times the speed setting); the UI thread locks the machine briefly
//! for input, the picture, save states and saves.

use crate::audio_out::AudioSink;
use ribbit_core::{Machine, StopReason};
use ribbit_debug::Breakpoints;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Emulated time per `run` call: short, so the UI never waits long for the
/// lock and pacing stays fine-grained.
const CHUNK: Duration = Duration::from_millis(2);
/// Drop lag beyond this (the game then runs slower than real time rather
/// than catching up in a burst).
const MAX_LAG: Duration = Duration::from_millis(250);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Control {
    pub running: bool,
    /// 1.0 = real time; infinity = as fast as possible.
    pub speed: f64,
    /// Volume (0 = muted).
    pub gain: f32,
    /// Press the touch-calibration targets automatically.
    pub auto_calibrate: bool,
    /// Play the Leapster 2's PCM sound at its 32 kHz (else via 8 kHz).
    pub pcm_32k: bool,
    /// Low-pass that 32 kHz sound (`audio_out::PcmClean`).
    pub pcm_clean: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Stats {
    /// Emulation speed over the last second, percent of real time.
    pub speed_percent: f64,
    /// Set when the machine stopped on its own (CPU error or halt).
    pub stopped: Option<String>,
}

struct Shared {
    machine: Mutex<Machine>,
    control: Mutex<(Control, bool)>, // (control, resync pacing)
    wake: Condvar,
    /// Sound goes straight from this thread to the output queue, so it does
    /// not depend on the window being redrawn.
    sink: Mutex<Option<AudioSink>>,
    stats: Mutex<Stats>,
    quit: AtomicBool,
    /// UI-thread lock requests in progress. std's mutex is not fair: the
    /// emulation thread, re-locking right after each chunk, could otherwise
    /// keep the UI waiting for tens of milliseconds (measured up to 43 ms:
    /// dropped clicks, stuttering sound).
    ui_waiting: AtomicUsize,
    /// Screen recording: i16 audio samples buffered here while recording
    /// (Some), drained by the UI thread each frame. None = not recording.
    record_audio: Mutex<Option<Vec<i16>>>,
}

pub struct Emu {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Emu {
    pub fn start(machine: Machine) -> Self {
        let shared = Arc::new(Shared {
            machine: Mutex::new(machine),
            control: Mutex::new((Control { running: false, speed: 1.0, gain: 1.0, auto_calibrate: false, pcm_32k: true, pcm_clean: true }, true)),
            wake: Condvar::new(),
            sink: Mutex::new(None),
            stats: Mutex::new(Stats::default()),
            quit: AtomicBool::new(false),
            ui_waiting: AtomicUsize::new(0),
            record_audio: Mutex::new(None),
        });
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("ribbit-emu".into())
            .spawn(move || {
                crate::perf::raise_thread_priority();
                run(&worker)
            })
            .expect("spawning the emulation thread");
        Self { shared, thread: Some(thread) }
    }

    /// The machine, for the UI thread (held briefly: the emulation thread
    /// waits meanwhile).
    pub fn lock(&self) -> MutexGuard<'_, Machine> {
        self.shared.ui_waiting.fetch_add(1, Ordering::SeqCst);
        let t = Instant::now();
        let g = self.shared.machine.lock().unwrap_or_else(|e| e.into_inner());
        self.shared.ui_waiting.fetch_sub(1, Ordering::SeqCst);
        let waited = t.elapsed();
        if waited > Duration::from_millis(8) {
            tracing::debug!(target: "ribbit::emu", waited_ms = waited.as_secs_f64() * 1e3, "UI waited for the machine");
        }
        g
    }

    pub fn set_control(&self, c: Control) {
        let mut g = self.shared.control.lock().unwrap();
        if g.0 != c {
            // Reset pacing when speed changes so the machine does not stall
            // waiting for real time to catch up to where it was at the old
            // speed (symptom: audio stays silent after going back to 1×).
            if g.0.speed != c.speed {
                g.1 = true;
            }
            g.0 = c;
            self.shared.wake.notify_all();
        }
    }

    /// Restart real-time pacing (after a reset, a loaded state or a pause).
    pub fn resync(&self) {
        self.shared.control.lock().unwrap().1 = true;
        self.shared.wake.notify_all();
    }

    pub fn set_audio_sink(&self, sink: Option<AudioSink>) {
        *self.shared.sink.lock().unwrap() = sink;
    }

    /// Begin buffering audio for screen recording.
    pub fn start_record_audio(&self) {
        *self.shared.record_audio.lock().unwrap() = Some(Vec::new());
    }

    /// Stop buffering and drain any remaining samples.
    pub fn stop_record_audio(&self, out: &mut Vec<i16>) {
        if let Some(buf) = self.shared.record_audio.lock().unwrap().take() {
            out.extend_from_slice(&buf);
        }
    }

    /// Drain buffered recording audio into `out` (called each UI frame).
    pub fn drain_record_audio(&self, out: &mut Vec<i16>) {
        let mut g = self.shared.record_audio.lock().unwrap();
        if let Some(buf) = g.as_mut() {
            out.extend_from_slice(buf);
            buf.clear();
        }
    }

    pub fn stats(&self) -> Stats {
        self.shared.stats.lock().unwrap().clone()
    }

    pub fn clear_stopped(&self) {
        self.shared.stats.lock().unwrap().stopped = None;
    }

    /// Stops the thread and returns the machine.
    pub fn stop(mut self) -> Machine {
        self.join();
        let shared = self.shared.clone();
        drop(self);
        match Arc::try_unwrap(shared) {
            Ok(s) => s.machine.into_inner().unwrap_or_else(|e| e.into_inner()),
            Err(_) => unreachable!("the emulation thread has exited"),
        }
    }

    fn join(&mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        self.shared.wake.notify_all();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Emu {
    fn drop(&mut self) {
        self.join();
    }
}

fn run(s: &Shared) {
    let mut pace: Option<(Instant, u64)> = None;
    let mut meter: Option<(Instant, u64)> = None;
    let none = Breakpoints::default();
    let mut audio = Vec::new();
    let mut dac_audio = Vec::new();
    let mut clean = ribbit_audio::LowPass32k::new();
    let mut last_chunk = Instant::now();
    let mut calibrate = crate::calibrate::AutoCalibrate::new();
    while !s.quit.load(Ordering::Relaxed) {
        let control = {
            let mut g = s.control.lock().unwrap();
            if std::mem::take(&mut g.1) {
                pace = None;
            }
            if !g.0.running {
                pace = None;
                meter = None;
                let _ = s.wake.wait_timeout(g, Duration::from_millis(50)).unwrap();
                continue;
            }
            g.0
        };
        // Let a waiting UI thread go first.
        while s.ui_waiting.load(Ordering::SeqCst) > 0 {
            std::thread::yield_now();
        }
        let mut m = s.machine.lock().unwrap_or_else(|e| e.into_inner());
        let hz = m.clock.cpu_hz as f64;
        let now = Instant::now();
        let chunk = (CHUNK.as_secs_f64() * hz) as u64;
        // Calibrating automatically: hidden, so as fast as possible.
        let calibrating = control.auto_calibrate && calibrate.busy(&m);
        let cycles = if control.speed.is_finite() && !calibrating {
            let (origin, base) = *pace.get_or_insert((now, m.cpu.cycles));
            let target = base + (now.duration_since(origin).as_secs_f64() * hz * control.speed) as u64;
            let behind = target.saturating_sub(m.cpu.cycles);
            if behind as f64 > MAX_LAG.as_secs_f64() * hz * control.speed {
                pace = Some((now, m.cpu.cycles));
            }
            behind.min(chunk)
        } else {
            pace = None;
            chunk
        };
        if cycles < chunk / 4 {
            // Ahead of real time: wait for the next chunk to be due.
            drop(m);
            std::thread::sleep(Duration::from_micros(500));
            continue;
        }
        let gap = now.duration_since(last_chunk);
        if gap > Duration::from_millis(12) {
            tracing::debug!(target: "ribbit::emu", gap_ms = gap.as_secs_f64() * 1e3, "emulation thread stalled");
        }
        let r = m.run_cycles(cycles, &none);
        last_chunk = Instant::now();
        if r.stop != StopReason::StepLimit {
            tracing::warn!(target: "ribbit::boot", ?r, "emulation stopped");
            s.stats.lock().unwrap().stopped = Some(format!("{:?}", r.stop));
            s.control.lock().unwrap().0.running = false;
        }
        if control.auto_calibrate {
            calibrate.tick(&mut m);
        }
        // Sound: push at any speed, subsampling to stay real-time at fast-forward
        // (pitches the audio up instead of going silent).
        let out = &mut m.io.voices.output;
        audio.resize(out.len(), 0);
        out.pop_into(&mut audio);
        // The PCM DAC stream (Leapster 2), when it plays: the same sound at
        // its 32 kHz, used instead of its 8 kHz copy if so chosen.
        let dac = &mut m.io.voices.dac_output;
        dac_audio.resize(dac.len(), 0);
        dac.pop_into(&mut dac_audio);
        if !calibrating
            && let Some(sink) = s.sink.lock().unwrap().as_mut()
        {
            let use_dac = control.pcm_32k && !dac_audio.is_empty();
            if control.speed <= 1.0 + f64::EPSILON {
                // Real-time or slower: push every sample.
                if use_dac {
                    if control.pcm_clean {
                        dac_audio.iter_mut().for_each(|s| *s = clean.push(*s));
                    }
                    sink.push_32k(&dac_audio, control.gain);
                } else {
                    sink.push(&audio, control.gain);
                }
            } else {
                // Fast-forward: subsample to real-time rate so audio pitches up.
                let step = control.speed.round() as usize;
                let src: &[_] = if use_dac { &dac_audio } else { &audio };
                let sub: Vec<_> = src.iter().copied().step_by(step.max(2)).collect();
                if use_dac {
                    sink.push_32k(&sub, control.gain);
                } else {
                    sink.push(&sub, control.gain);
                }
            }
        }
        // Screen recording: duplicate the same samples into the recording
        // buffer (drained by the UI thread each frame).
        if control.speed == 1.0 && !calibrating {
            let mut rec = s.record_audio.lock().unwrap();
            if let Some(buf) = rec.as_mut() {
                if control.pcm_32k && !dac_audio.is_empty() {
                    buf.extend_from_slice(&dac_audio);
                } else {
                    buf.extend_from_slice(&audio);
                }
            }
        }
        // Speed meter.
        let cycles = m.cpu.cycles;
        drop(m);
        let (t0, c0) = *meter.get_or_insert((now, cycles));
        let elapsed = now.duration_since(t0).as_secs_f64();
        if elapsed >= 1.0 {
            s.stats.lock().unwrap().speed_percent = 100.0 * cycles.saturating_sub(c0) as f64 / (elapsed * hz);
            meter = Some((now, cycles));
        }
    }
}
