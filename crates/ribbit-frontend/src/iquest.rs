//! Playing the LeapFrog iQuest and Turbo Twist Extreme (`ribbit_iquest`):
//! the machine runs on its own thread at 60 frames a second, and the UI takes
//! its LCD picture and sends it keys.
//!
//! iQuest keys: the PC's letters, digits, space and '.' are the keyboard's;
//! Enter is Enter, Backspace and Delete are Del, the arrows the D-pad, Home
//! is Menu, End is Repeat, '-' and '=' the volume.
//!
//! Turbo Twist Extreme keys: the arrows are the D-pad; W and S twist the
//! handle up and down, E and D twist it fast; Space or Enter is Slam, Home
//! or M is Menu, End or R is Repeat, '-' and '=' the volume.
//!
//! Pause is the ON/OFF button on both.
//!
//! What the machine keeps across power-offs (the iQuest's flash, the Turbo
//! Twist Extreme's E2) is `iquest.flash` / `turbotwist.e2` in the save
//! folder, written within a second of changing and when play stops.
//!
//! EXPERIMENTAL.

use crate::audio_out::AudioSink;
use ribbit_iquest::{Machine, Model};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FPS: f64 = ribbit_iquest::FRAME_HZ as f64;

struct Shared {
    m: Mutex<Machine>,
    frame: Mutex<Vec<u32>>,
    /// Key events (name, pressed) not yet sent.
    keys: Mutex<Vec<(&'static str, bool)>>,
    power: AtomicBool,
    running: AtomicBool,
    quit: AtomicBool,
    off: AtomicBool,
    gain: AtomicU32,
    speed: AtomicU32,
    keep_awake: AtomicBool,
    sink: Mutex<Option<AudioSink>>,
    save_path: PathBuf,
}

pub struct IqSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub title: String,
    pub started: u64,
    pub model: Model,
    bios: Vec<u8>,
    cart: Option<Vec<u8>>,
}

impl IqSession {
    /// Starts the machine from its base ROM, with a cartridge in the slot
    /// if given. `keep_awake`: the "Turn off when idle" setting is off.
    pub fn start(bios: &Path, cart: Option<&Path>, save_dir: &Path, sink: Option<AudioSink>, keep_awake: bool) -> anyhow::Result<Self> {
        let bios = ribbit_core::read_image(bios)?;
        let model = ribbit_iquest::identify(&bios).ok_or_else(|| anyhow::anyhow!("not an iQuest or Turbo Twist Extreme base ROM"))?;
        let cart = match cart {
            Some(p) => Some(ribbit_core::read_image(p)?),
            None => None,
        };
        let save_path = save_dir.join(match model {
            Model::IQuest => "iquest.flash",
            Model::TurboTwistExtreme => "turbotwist.e2",
        });
        let mut m = Self::machine(&bios, cart.as_deref());
        if let Ok(data) = std::fs::read(&save_path) {
            m.load_save(&data);
        }
        m.keep_awake = keep_awake;
        let (w, h) = model.screen();
        let shared = Arc::new(Shared {
            m: Mutex::new(m),
            frame: Mutex::new(vec![ribbit_iquest::GLASS; w * h]),
            keys: Mutex::new(Vec::new()),
            power: AtomicBool::new(false),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            off: AtomicBool::new(false),
            gain: AtomicU32::new(1f32.to_bits()),
            speed: AtomicU32::new(100f32.to_bits()),
            keep_awake: AtomicBool::new(keep_awake),
            sink: Mutex::new(sink),
            save_path,
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-iquest".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: model.name().into(), started, model, bios, cart })
    }

    fn machine(bios: &[u8], cart: Option<&[u8]>) -> Machine {
        let mut m = Machine::new(bios.to_vec());
        if let Some(c) = cart {
            m.insert_cartridge(c.to_vec());
        }
        m
    }

    /// The LCD's size.
    pub fn size(&self) -> (usize, usize) {
        self.model.screen()
    }

    /// Copies the latest picture into `out` (`size()`).
    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clone_from(&self.shared.frame.lock().unwrap());
    }

    /// Whether the machine turned itself off.
    pub fn powered_off(&self) -> bool {
        self.shared.off.load(Ordering::Relaxed)
    }

    pub fn speed_percent(&self) -> f64 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed)) as f64
    }

    pub fn set_running(&self, running: bool) {
        self.shared.running.store(running, Ordering::Relaxed);
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// The "Turn off when idle" setting changed (off: never idles off).
    pub fn set_turn_off_when_idle(&self, on: bool) {
        self.shared.keep_awake.store(!on, Ordering::Relaxed);
    }

    /// A PC key, as the machine's key it stands for (if any). Returns
    /// whether it was one.
    pub fn key(&self, code: winit::keyboard::KeyCode, pressed: bool) -> bool {
        if code == winit::keyboard::KeyCode::Pause {
            if pressed {
                self.shared.power.store(true, Ordering::Relaxed);
            }
            return true;
        }
        let Some(k) = key_name(self.model, code) else { return false };
        self.shared.keys.lock().unwrap().push((k, pressed));
        true
    }

    /// Restarts the machine (the same cartridge in, its save kept).
    pub fn reset(&self) {
        let mut m = self.shared.m.lock().unwrap();
        flush_save(&mut m, &self.shared.save_path);
        let mut fresh = Self::machine(&self.bios, self.cart.as_deref());
        fresh.load_save(m.save_data());
        fresh.keep_awake = m.keep_awake;
        *m = fresh;
    }

    /// Stops the machine (after keeping its save).
    pub fn stop(mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        flush_save(&mut self.shared.m.lock().unwrap(), &self.shared.save_path);
    }
}

/// Writes the save to `path` if the machine changed it.
fn flush_save(m: &mut Machine, path: &Path) {
    if !m.save_dirty() {
        return;
    }
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, m.save_data())?;
        std::fs::rename(&tmp, path)
    })();
    match result {
        Ok(()) => m.save_kept(),
        Err(e) => tracing::warn!(target: "ribbit::saves", "saving the {}'s memory: {e}", m.model.name()),
    }
}

/// The machine's key name (ribbit_iquest::keys) for a PC key.
fn key_name(model: Model, code: winit::keyboard::KeyCode) -> Option<&'static str> {
    use winit::keyboard::KeyCode as K;
    let shared = match code {
        K::ArrowUp => Some("up"),
        K::ArrowDown => Some("down"),
        K::ArrowLeft => Some("left"),
        K::ArrowRight => Some("right"),
        K::Minus | K::NumpadSubtract => Some("vol-"),
        K::Equal | K::NumpadAdd => Some("vol+"),
        K::Home => Some("menu"),
        K::End => Some("repeat"),
        _ => None,
    };
    if shared.is_some() {
        return shared;
    }
    match model {
        Model::TurboTwistExtreme => match code {
            K::KeyW => Some("twist-up"),
            K::KeyS => Some("twist-down"),
            K::KeyE => Some("twist-up-fast"),
            K::KeyD => Some("twist-down-fast"),
            K::Space | K::Enter | K::NumpadEnter => Some("slam"),
            K::KeyM => Some("menu"),
            K::KeyR => Some("repeat"),
            _ => None,
        },
        Model::IQuest => Some(match code {
            K::Enter | K::NumpadEnter => "enter",
            K::Backspace | K::Delete => "del",
            K::Space => "space",
            K::Period | K::NumpadDecimal => ".",
            K::KeyA => "A",
            K::KeyB => "B",
            K::KeyC => "C",
            K::KeyD => "D",
            K::KeyE => "E",
            K::KeyF => "F",
            K::KeyG => "G",
            K::KeyH => "H",
            K::KeyI => "I",
            K::KeyJ => "J",
            K::KeyK => "K",
            K::KeyL => "L",
            K::KeyM => "M",
            K::KeyN => "N",
            K::KeyO => "O",
            K::KeyP => "P",
            K::KeyQ => "Q",
            K::KeyR => "R",
            K::KeyS => "S",
            K::KeyT => "T",
            K::KeyU => "U",
            K::KeyV => "V",
            K::KeyW => "W",
            K::KeyX => "X",
            K::KeyY => "Y",
            K::KeyZ => "Z",
            K::Digit0 | K::Numpad0 => "0",
            K::Digit1 | K::Numpad1 => "1",
            K::Digit2 | K::Numpad2 => "2",
            K::Digit3 | K::Numpad3 => "3",
            K::Digit4 | K::Numpad4 => "4",
            K::Digit5 | K::Numpad5 => "5",
            K::Digit6 | K::Numpad6 => "6",
            K::Digit7 | K::Numpad7 => "7",
            K::Digit8 | K::Numpad8 => "8",
            K::Digit9 | K::Numpad9 => "9",
            _ => return None,
        }),
    }
}

fn run(s: &Shared) {
    let mut img = Vec::new();
    let mut next = Instant::now();
    let mut audio = Vec::new();
    let (mut counted_since, mut frames) = (Instant::now(), 0u32);
    while !s.quit.load(Ordering::Relaxed) {
        if !s.running.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
            next = Instant::now();
            (counted_since, frames) = (next, 0);
            continue;
        }
        let elapsed = counted_since.elapsed().as_secs_f64();
        if elapsed >= 1.0 {
            s.speed.store(((frames as f64 / elapsed / FPS * 100.0) as f32).to_bits(), Ordering::Relaxed);
            (counted_since, frames) = (Instant::now(), 0);
            flush_save(&mut s.m.lock().unwrap(), &s.save_path);
        }
        frames += 1;
        let now = Instant::now();
        if now < next {
            std::thread::sleep(next - now);
        }
        if Instant::now() > next + Duration::from_millis(100) {
            next = Instant::now();
        }
        next += Duration::from_secs_f64(1.0 / FPS);
        {
            let mut m = s.m.lock().unwrap();
            m.keep_awake = s.keep_awake.load(Ordering::Relaxed);
            for (k, down) in s.keys.lock().unwrap().drain(..) {
                m.set_key(k, down);
            }
            if s.power.swap(false, Ordering::Relaxed) {
                m.press_power();
            }
            m.run_frame();
            m.render(&mut img);
            if m.is_off() {
                s.off.store(true, Ordering::Relaxed);
            }
            m.take_audio(&mut audio);
        }
        s.frame.lock().unwrap().clone_from(&img);
        let gain = f32::from_bits(s.gain.load(Ordering::Relaxed));
        if let Some(sink) = s.sink.lock().unwrap().as_mut() {
            sink.push(&audio, gain);
        }
    }
}
