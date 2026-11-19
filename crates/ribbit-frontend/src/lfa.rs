//! Playing a LeapPad Academy / LeapFrog Epic title (`ribbit_lfandroid`): its APK's native
//! library runs on its own thread, a frame per vertical blank (60 a second at most), and the
//! UI takes its picture (1024x600, or 600x1024 for a portrait title) and sends it the mouse as
//! the finger and the keyboard as the tablet's buttons: the arrow keys the d-pad, Z and X its
//! A and B, Backspace Back, H Hint, P Pause and Home the home button (GUESS: what the Academy's
//! own buttons are; see docs/leappad-academy.md).
//!
//! Its OpenSL ES players' sound (`ribbit_lfandroid::mixer`, mono 32 kHz) goes to the output as
//! it is made. No saves yet: a title that needs a writable file (PPAD2's SAVE.DAT) quits.
//! EXPERIMENTAL.

use crate::audio_out::AudioSink;
use ribbit_lfandroid::input::button;
use ribbit_lfandroid::tablet::Tablet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FPS: f64 = 60.0;

#[derive(Default)]
struct Input {
    touch: Option<(f32, f32)>,
    buttons: u32,
}

struct Shared {
    frame: Mutex<Vec<u32>>,
    input: Mutex<Input>,
    running: AtomicBool,
    quit: AtomicBool,
    /// Why the title stopped, if it did.
    stopped: Mutex<Option<String>>,
    speed: AtomicU32,
    gain: AtomicU32,
    sink: Mutex<Option<AudioSink>>,
}

pub struct LfaSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub title: String,
    /// When play started (Unix seconds), for Discord's timer.
    pub started: u64,
    size: (usize, usize),
    apk: PathBuf,
    data: PathBuf,
}

impl LfaSession {
    /// Opens the APK and starts the title up to its first frame (on the title's thread, which
    /// it then stays on). `data` is the tablet's storage, where saves go.
    pub fn start(apk: &Path, data: &Path, sink: Option<AudioSink>) -> anyhow::Result<Self> {
        let shared = Arc::new(Shared {
            frame: Mutex::new(Vec::new()),
            input: Mutex::new(Input::default()),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            stopped: Mutex::new(None),
            speed: AtomicU32::new(100f32.to_bits()),
            gain: AtomicU32::new(1f32.to_bits()),
            sink: Mutex::new(sink),
        });
        let (tx, rx) = std::sync::mpsc::channel();
        let (s, path, dir) = (shared.clone(), apk.to_path_buf(), data.to_path_buf());
        let thread = std::thread::Builder::new().name("ribbit-lfa".into()).stack_size(16 << 20).spawn(move || {
            crate::perf::raise_thread_priority();
            match Tablet::open(&path, None, Some(&dir)) {
                Ok(t) => {
                    let (w, h) = t.screen();
                    *s.frame.lock().unwrap() = vec![0; (w * h) as usize];
                    let _ = tx.send(Ok(((w as usize, h as usize), t.package.clone())));
                    run(t, &s);
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                }
            }
        })?;
        let (size, package) = match rx.recv() {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                let _ = thread.join();
                anyhow::bail!("{e}");
            }
            Err(_) => {
                let _ = thread.join();
                anyhow::bail!("the title stopped while starting");
            }
        };
        let title = apk.file_stem().map_or(package, |s| s.to_string_lossy().into_owned());
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title, started, size, apk: apk.to_path_buf(), data: data.to_path_buf() })
    }

    /// The picture's size.
    pub fn size(&self) -> (usize, usize) {
        self.size
    }

    /// Copies the latest picture into `out` (`size()`).
    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clone_from(&self.shared.frame.lock().unwrap());
    }

    /// Why the title stopped (it quit or hit something not stood in for yet), once it has.
    pub fn stopped(&self) -> Option<String> {
        self.shared.stopped.lock().unwrap().clone()
    }

    pub fn speed_percent(&self) -> f64 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed)) as f64
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn set_running(&self, running: bool) {
        self.shared.running.store(running, Ordering::Relaxed);
    }

    /// The finger at a picture position, or lifted.
    pub fn set_touch(&self, at: Option<(f32, f32)>) {
        self.shared.input.lock().unwrap().touch = at;
    }

    /// A PC key, as the tablet button it stands for (if any). Returns whether it was one.
    pub fn key(&self, code: winit::keyboard::KeyCode, pressed: bool) -> bool {
        use winit::keyboard::KeyCode as K;
        let b = match code {
            K::ArrowUp => button::UP,
            K::ArrowDown => button::DOWN,
            K::ArrowLeft => button::LEFT,
            K::ArrowRight => button::RIGHT,
            K::KeyZ => button::A,
            K::KeyX => button::B,
            K::Backspace => button::BACK,
            K::KeyH => button::HINT,
            K::KeyP => button::PAUSE,
            K::Home => button::HOME,
            _ => return false,
        };
        let mut i = self.shared.input.lock().unwrap();
        if pressed {
            i.buttons |= b;
        } else {
            i.buttons &= !b;
        }
        true
    }

    /// Starts the title again from its APK.
    pub fn restart(self) -> anyhow::Result<Self> {
        let (apk, data) = (self.apk.clone(), self.data.clone());
        let sink = self.shared.sink.lock().unwrap().take();
        self.stop();
        Self::start(&apk, &data, sink)
    }

    pub fn stop(mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn run(mut t: Tablet, s: &Shared) {
    let period = Duration::from_secs_f64(1.0 / FPS);
    let mut next = Instant::now();
    let (mut count, mut since) = (0u32, Instant::now());
    let (mut touch, mut buttons) = (None, 0);
    let mut audio = Vec::new();
    while !s.quit.load(Ordering::Relaxed) {
        if !s.running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            next = Instant::now();
            since = next;
            count = 0;
            continue;
        }
        let (want_touch, want_buttons) = {
            let i = s.input.lock().unwrap();
            (i.touch, i.buttons)
        };
        let mut r = Ok(());
        if want_touch != touch {
            touch = want_touch;
            r = t.touch(touch);
        }
        if r.is_ok() && want_buttons != buttons {
            buttons = want_buttons;
            r = t.set_buttons(buttons);
        }
        let r = r.and_then(|()| t.frame());
        if let Err(e) = r {
            tracing::warn!(target: "ribbit::lfa", "{e}");
            *s.stopped.lock().unwrap() = Some(e);
            return;
        }
        t.take_audio(&mut audio);
        if let Some(sink) = s.sink.lock().unwrap().as_mut() {
            sink.push_32k(&audio, f32::from_bits(s.gain.load(Ordering::Relaxed)));
        }
        audio.clear();
        if let Some((w, h, px)) = t.picture() {
            let mut f = s.frame.lock().unwrap();
            if f.len() == (w * h) as usize && (w, h) == t.screen() {
                *f = px;
            }
        }
        count += 1;
        let span = since.elapsed();
        if span >= Duration::from_secs(1) {
            let pct = count as f64 / span.as_secs_f64() / FPS * 100.0;
            s.speed.store((pct as f32).to_bits(), Ordering::Relaxed);
            (count, since) = (0, Instant::now());
        }
        next += period;
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        } else {
            next = now;
        }
    }
}
