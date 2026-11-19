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

/// "Turn off when idle" on: the tablet goes to sleep after this long
/// untouched (no finger, no button), and the app closes the title. GUESS:
/// the LeapPad Academy's own sleep time is unknown (its Android settings and
/// LeapFrog's parent settings aren't run here); this is the LeapPad 2's and
/// Explorer's firmware shutdown time (ribbit_didj::idle, 240 s).
const IDLE_OFF_SECS: u32 = 240;

/// Frames since the last finger or button, for "Turn off when idle".
#[derive(Default)]
struct IdleCount {
    frames: u32,
}

impl IdleCount {
    /// One frame of the title. `active`: a finger is down or a button held
    /// or changed. `keep_awake`: the setting is off (never idles off).
    /// True when the tablet goes to sleep.
    fn tick(&mut self, active: bool, keep_awake: bool) -> bool {
        if active || keep_awake {
            self.frames = 0;
            return false;
        }
        self.frames += 1;
        self.frames as f64 >= IDLE_OFF_SECS as f64 * FPS
    }
}

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
    /// "Turn off when idle" off: never sleeps.
    keep_awake: AtomicBool,
    /// It went to sleep for idling ("Turn off when idle" on).
    idle_off: AtomicBool,
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
            keep_awake: AtomicBool::new(true),
            idle_off: AtomicBool::new(false),
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

    /// "Turn off when idle": on, the tablet sleeps after `IDLE_OFF_SECS`
    /// untouched (a GUESS at the real one) and `idle_powered_off` says so;
    /// off, it never does.
    pub fn set_turn_off_when_idle(&self, on: bool) {
        self.shared.keep_awake.store(!on, Ordering::Relaxed);
    }

    /// It went to sleep for idling: the app closes it.
    pub fn idle_powered_off(&self) -> bool {
        self.shared.idle_off.load(Ordering::Relaxed)
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

    /// A Leapster-style button (controller, touch pads), as the tablet's
    /// own: the d-pad, A, B, Hint, Pause and Home.
    pub fn set_button(&self, b: ribbit_input::Button, down: bool) {
        use ribbit_input::Button as B;
        let bit = match b {
            B::Up => button::UP,
            B::Down => button::DOWN,
            B::Left => button::LEFT,
            B::Right => button::RIGHT,
            B::A => button::A,
            B::B => button::B,
            B::Hint => button::HINT,
            B::Pause => button::PAUSE,
            B::Home => button::HOME,
            _ => return,
        };
        let mut i = self.shared.input.lock().unwrap();
        if down {
            i.buttons |= bit;
        } else {
            i.buttons &= !bit;
        }
    }

    /// Starts the title again from its APK.
    pub fn restart(self) -> anyhow::Result<Self> {
        let (apk, data) = (self.apk.clone(), self.data.clone());
        let sink = self.shared.sink.lock().unwrap().take();
        let keep_awake = self.shared.keep_awake.load(Ordering::Relaxed);
        self.stop();
        let t = Self::start(&apk, &data, sink)?;
        t.shared.keep_awake.store(keep_awake, Ordering::Relaxed);
        Ok(t)
    }

    pub fn stop(mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Names of the titles whose label is only in their resources (by the end
/// of the package name, after `app.x<product id>.`).
const TITLES: &[(&str, &str)] = &[("AP_DINO", "T-Rex Rush"), ("AP_SM2", "Stretchy Monkey 2: Big City Hijinks"), ("AP_SM2_NoSO", "Stretchy Monkey 2: Big City Hijinks")];

/// A LeapPad Academy / LeapFrog Epic title for the library: an APK with
/// its Brio native library inside (`ribbit_lfandroid::peek`, reading the
/// manifest alone). System apps (no native activity) and the Academy's
/// "NoSO" built-ins (their library is the firmware's, which File > Open
/// doesn't load either) are not listed.
pub fn describe_apk(path: &Path) -> Option<crate::library::Game> {
    let p = ribbit_lfandroid::peek(path).ok()?;
    if !p.has_library || !p.package.starts_with("com.leapfrog.") {
        return None;
    }
    let product = p.package.rsplit('.').next().unwrap_or_default();
    let title = TITLES.iter().find(|(k, _)| *k == product).map_or_else(
        || product.strip_prefix("AP_").unwrap_or(product).replace('_', " "),
        |(_, t)| t.to_string(),
    );
    // AND4 builds are the Academy's, ANDS the Epic's (docs/leappad-academy.md).
    let system = if p.package.contains(".ANDS.") { "LeapFrog Epic" } else { "LeapPad Academy" };
    let mut details = vec![("System", system.to_string()), ("Package", p.package.clone())];
    if !p.version.is_empty() {
        details.push(("Version", p.version.clone()));
    }
    if let Some(kind) = &p.content_type {
        details.push(("Kind", kind.clone()));
    }
    details.push(("Screen", if p.portrait { "Portrait" } else { "Landscape" }.to_string()));
    if let Ok(m) = std::fs::metadata(path) {
        details.push(("Size", format!("{} MB", m.len().div_ceil(1 << 20))));
    }
    let subtitle = if p.version.is_empty() { system.to_string() } else { format!("{system} {}", p.version) };
    Some(crate::library::Game {
        path: path.to_path_buf(),
        family: crate::library::Family::LeapPadAcademy,
        title,
        subtitle,
        details,
        tags: vec!["Experimental".into()],
        key: format!("apk_{}_{}", p.package.replace('.', "_"), p.version.replace('.', "_")),
        art: crate::library::Art::Missing,
    })
}

fn run(mut t: Tablet, s: &Shared) {
    let period = Duration::from_secs_f64(1.0 / FPS);
    let mut next = Instant::now();
    let (mut count, mut since) = (0u32, Instant::now());
    let (mut touch, mut buttons) = (None, 0);
    let mut audio = Vec::new();
    let mut idle = IdleCount::default();
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
        let active = want_touch.is_some() || want_buttons != 0 || want_buttons != buttons;
        if idle.tick(active, s.keep_awake.load(Ordering::Relaxed)) {
            tracing::info!(target: "ribbit::lfa", "untouched for {IDLE_OFF_SECS} s: the tablet went to sleep");
            s.idle_off.store(true, Ordering::Relaxed);
            return;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// "Turn off when idle": on, asleep after IDLE_OFF_SECS of frames with
    /// no finger or button (a touch starts the count again); off, never.
    #[test]
    fn idle_sleep_follows_the_setting() {
        let limit = (IDLE_OFF_SECS as f64 * FPS) as u32;
        let mut on = IdleCount::default();
        assert!(!(1..limit).any(|_| on.tick(false, false)));
        assert!(!on.tick(true, false), "a touch keeps it awake");
        assert!(!(1..limit).any(|_| on.tick(false, false)));
        assert!(on.tick(false, false), "asleep after {IDLE_OFF_SECS} s untouched");
        let mut off = IdleCount::default();
        assert!(!(0..limit * 3).any(|_| off.tick(false, true)), "off: never sleeps");
    }

    /// Every .apk under RIBBIT_LFA_DIR (folders below too): the titles with
    /// their native library are listed, system apps and NoSO built-ins not.
    #[test]
    #[ignore]
    fn academy_titles_are_listed() {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, out);
                } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("apk")) {
                    out.push(p);
                }
            }
        }
        let mut apks = Vec::new();
        walk(Path::new(&std::env::var("RIBBIT_LFA_DIR").expect("RIBBIT_LFA_DIR")), &mut apks);
        let mut listed = 0;
        for p in &apks {
            let t = Instant::now();
            let g = describe_apk(p);
            eprintln!("{}: {:?} ({:.0} ms)", p.display(), g.as_ref().map(|g| (&g.title, &g.subtitle)), t.elapsed().as_secs_f64() * 1e3);
            if let Some(g) = g {
                assert!(!g.title.is_empty());
                listed += 1;
            }
        }
        assert!(listed > 0 && listed < apks.len());
    }
}
