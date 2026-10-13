//! The emulator on its own thread, so emulation gets a whole core and the
//! window stays at the display's frame rate even when emulation runs slower
//! than real time. The thread runs the machine in short chunks, paced to real
//! time (times the speed setting); the UI thread locks the machine briefly
//! for input, the picture, save states and saves.

use ribbit_core::{Machine, StopReason};
use ribbit_debug::Breakpoints;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Emulated time per `run` call: short, so the UI never waits long for the
/// lock and pacing stays fine-grained.
const CHUNK: Duration = Duration::from_millis(2);
/// Drop lag beyond this (the game then runs slower than real time rather
/// than catching up in a burst).
const MAX_LAG: Duration = Duration::from_millis(250);
/// Most sound kept for the UI thread to collect (a second at 8 kHz).
const AUDIO_KEEP: usize = 8_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Control {
    pub running: bool,
    /// 1.0 = real time; infinity = as fast as possible.
    pub speed: f64,
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
    audio: Mutex<Vec<i16>>,
    stats: Mutex<Stats>,
    quit: AtomicBool,
}

pub struct Emu {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Emu {
    pub fn start(machine: Machine) -> Self {
        let shared = Arc::new(Shared {
            machine: Mutex::new(machine),
            control: Mutex::new((Control { running: false, speed: 1.0 }, true)),
            wake: Condvar::new(),
            audio: Mutex::new(Vec::new()),
            stats: Mutex::new(Stats::default()),
            quit: AtomicBool::new(false),
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
        self.shared.machine.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_control(&self, c: Control) {
        let mut g = self.shared.control.lock().unwrap();
        if g.0 != c {
            g.0 = c;
            self.shared.wake.notify_all();
        }
    }

    /// Restart real-time pacing (after a reset, a loaded state or a pause).
    pub fn resync(&self) {
        self.shared.control.lock().unwrap().1 = true;
        self.shared.wake.notify_all();
    }

    /// Sound produced since the last call (8 kHz).
    pub fn take_audio(&self, out: &mut Vec<i16>) {
        let mut a = self.shared.audio.lock().unwrap();
        out.clear();
        out.append(&mut a);
    }

    pub fn clear_audio(&self) {
        self.shared.audio.lock().unwrap().clear();
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
        let mut m = s.machine.lock().unwrap_or_else(|e| e.into_inner());
        let hz = m.clock.cpu_hz as f64;
        let now = Instant::now();
        let chunk = (CHUNK.as_secs_f64() * hz) as u64;
        let steps = if control.speed.is_finite() {
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
        if steps < chunk / 4 {
            // Ahead of real time: wait for the next chunk to be due.
            drop(m);
            std::thread::sleep(Duration::from_micros(500));
            continue;
        }
        let r = m.run(steps, &none);
        if r.stop != StopReason::StepLimit {
            tracing::warn!(target: "ribbit::boot", ?r, "emulation stopped");
            s.stats.lock().unwrap().stopped = Some(format!("{:?}", r.stop));
            s.control.lock().unwrap().0.running = false;
        }
        // Sound, at real-time speed only (no pitch-shifted audio).
        let out = &mut m.io.voices.output;
        let n = out.len();
        if control.speed == 1.0 {
            let mut a = s.audio.lock().unwrap();
            let start = a.len();
            a.resize(start + n, 0);
            out.pop_into(&mut a[start..]);
            if a.len() > AUDIO_KEEP {
                let excess = a.len() - AUDIO_KEEP;
                a.drain(..excess);
            }
        } else {
            let mut sink = vec![0; n];
            out.pop_into(&mut sink);
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
