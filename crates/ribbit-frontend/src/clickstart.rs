//! Playing the LeapFrog ClickStart (`ribbit_clickstart`): the machine runs
//! on its own thread at 60 frames a second, and the UI takes its picture and
//! sends it the keyboard and the mouse. The PC's letter, digit, space,
//! Enter, Backspace, Caps Lock, '.' and '?' keys are the ClickStart
//! keyboard's; Home is its system key, End its Hint key and the arrow keys
//! its four unidentified keys (see `key_code`). The PC mouse over the
//! picture moves the ClickStart mouse (its movement, scaled so that the
//! console's pointer follows it pixel for pixel: the console has no
//! absolute position) and the left button is its button.
//!
//! The console keeps its players in a 24C08 EEPROM: one for all its games,
//! kept as `clickstart.eeprom` in the save folder (read at start, written
//! within a second of the system changing it, and when play stops).
//!
//! EXPERIMENTAL.

use crate::audio_out::AudioSink;
use crate::zippity::Resampler;
use ribbit_clickstart::{ClickStart, SCREEN_H, SCREEN_W};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const FPS: f64 = 60.0;
/// ClickStart mouse units per picture pixel the PC mouse moves: 32/12, from
/// the system ROM's pointer code (no acceleration), so the pointer moves
/// with the PC mouse 1:1 until it meets the picture's edge.
const MOUSE_SCALE: f32 = ribbit_clickstart::MOUSE_UNITS_PER_PIXEL;

#[derive(Default)]
struct Input {
    /// Key events (code, pressed) not yet sent; ARROW | an ARROW_ bit for
    /// an arrow key.
    keys: Vec<(u8, bool)>,
    /// Mouse movement not yet sent (units), and the button.
    dx: f32,
    dy: f32,
    button: bool,
    button_sent: bool,
}

struct Shared {
    cs: Mutex<ClickStart>,
    frame: Mutex<Vec<u32>>,
    input: Mutex<Input>,
    running: AtomicBool,
    quit: AtomicBool,
    /// The console turned itself off.
    off: AtomicBool,
    /// Keep it on however long it is left alone ("Turn off when idle" off):
    /// `ClickStart::keep_awake`.
    keep_awake: AtomicBool,
    gain: AtomicU32,
    speed: AtomicU32,
    sink: Mutex<Option<AudioSink>>,
    /// Where the EEPROM is kept.
    eeprom_path: PathBuf,
}

pub struct CsSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub title: String,
    /// When play started (Unix seconds), for Discord's timer.
    pub started: u64,
    bios: Vec<u8>,
    cart: Option<Vec<u8>>,
}

impl CsSession {
    /// Starts the console from its system ROM, with a cartridge (its ROM
    /// dump, raw or zipped) in the slot if given.
    pub fn start(bios: &Path, cart: Option<&Path>, save_dir: &Path, sink: Option<AudioSink>) -> anyhow::Result<Self> {
        let bios = ribbit_core::read_image(bios)?;
        anyhow::ensure!(bios.len() == ribbit_clickstart::BIOS_SIZE, "not a ClickStart system ROM (8 MB)");
        let cart = match cart {
            Some(p) => {
                let data = ribbit_core::read_image(p)?;
                anyhow::ensure!(data.len() == ribbit_clickstart::CART_SIZE, "not a ClickStart cartridge dump (4 MB)");
                Some(data)
            }
            None => None,
        };
        let eeprom_path = save_dir.join("clickstart.eeprom");
        let mut cs = Self::machine(&bios, cart.as_deref());
        // With no file yet the chip is blank, and the system formats it.
        if let Ok(data) = std::fs::read(&eeprom_path) {
            cs.set_eeprom(&data);
        }
        let shared = Arc::new(Shared {
            cs: Mutex::new(cs),
            frame: Mutex::new(vec![0; SCREEN_W * SCREEN_H]),
            input: Mutex::new(Input::default()),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            off: AtomicBool::new(false),
            keep_awake: AtomicBool::new(true),
            gain: AtomicU32::new(1f32.to_bits()),
            speed: AtomicU32::new(100f32.to_bits()),
            sink: Mutex::new(sink),
            eeprom_path,
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-clickstart".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: "ClickStart".into(), started, bios, cart })
    }

    fn machine(bios: &[u8], cart: Option<&[u8]>) -> ClickStart {
        let mut cs = ClickStart::new(bios);
        if let Some(cart) = cart {
            cs.insert_cartridge(cart);
            cs.reset();
        }
        cs
    }

    /// Copies the latest picture into `out` (SCREEN_W x SCREEN_H).
    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clone_from(&self.shared.frame.lock().unwrap());
    }

    /// Frames run in the last second, as a percentage of real time.
    /// Whether the console turned itself off (closed as a Leapster that did).
    pub fn powered_off(&self) -> bool {
        self.shared.off.load(Ordering::Relaxed)
    }

    /// "Turn off when idle": on, the home screen left alone for 150 s waves
    /// goodbye and turns the console off, as a real one; off, it stays on
    /// (the developer mode's shutdown still turns it off).
    pub fn set_turn_off_when_idle(&self, on: bool) {
        self.shared.keep_awake.store(!on, Ordering::Relaxed);
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

    /// A PC key, as the ClickStart key it stands for (if any). Returns
    /// whether it was one.
    pub fn key(&self, code: winit::keyboard::KeyCode, pressed: bool) -> bool {
        let Some(k) = key_code(code) else { return false };
        self.shared.input.lock().unwrap().keys.push((k, pressed));
        true
    }

    /// The PC mouse moved by (dx, dy) picture pixels.
    pub fn mouse_move(&self, dx: f32, dy: f32) {
        let mut i = self.shared.input.lock().unwrap();
        i.dx += dx * MOUSE_SCALE;
        i.dy += dy * MOUSE_SCALE;
    }

    pub fn mouse_button(&self, down: bool) {
        self.shared.input.lock().unwrap().button = down;
    }

    /// Restarts the console (the same cartridge in).
    pub fn reset(&self) {
        let mut cs = self.shared.cs.lock().unwrap();
        // The players stay: written first if they changed, then carried over.
        flush_eeprom(&mut cs, &self.shared.eeprom_path);
        let mut fresh = Self::machine(&self.bios, self.cart.as_deref());
        fresh.set_eeprom(cs.eeprom());
        *cs = fresh;
    }

    pub fn save_state(&self) -> Vec<u8> {
        self.shared.cs.lock().unwrap().save_state()
    }

    /// Loads a state; the players in the EEPROM stay as they are.
    pub fn load_state(&self, state: &[u8]) -> Result<(), String> {
        self.shared.cs.lock().unwrap().load_state(state)
    }

    /// Stops the machine (after keeping its EEPROM).
    pub fn stop(mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        flush_eeprom(&mut self.shared.cs.lock().unwrap(), &self.shared.eeprom_path);
    }
}

/// Writes the EEPROM to `path` if the system has changed it.
fn flush_eeprom(cs: &mut ClickStart, path: &Path) {
    if !cs.eeprom_dirty() {
        return;
    }
    let result = (|| -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, cs.eeprom())?;
        std::fs::rename(&tmp, path)
    })();
    match result {
        Ok(()) => cs.eeprom_saved(),
        Err(e) => tracing::warn!(target: "ribbit::saves", "saving the ClickStart's EEPROM: {e}"),
    }
}

/// Marks an arrow key in `Input::keys` (key codes are below 0x40).
const ARROW: u8 = 0x80;

/// The ClickStart keyboard's code for a PC key: letters, digits, space,
/// '.' and '?' (the '/' key), Enter, Backspace and Caps Lock; Home or Pause
/// is the Pause key, End the Hint key, the arrow keys the arrows (ARROW |
/// the bit), and Page Up, Page Down, Insert and Delete the four yellow
/// picture keys (smiley, note, boat, cat; which is which is UNVERIFIED, see
/// ribbit_clickstart::KEY_SPECIAL). Esc, Tab, ` and the F keys stay the
/// app's.
fn key_code(code: winit::keyboard::KeyCode) -> Option<u8> {
    use ribbit_clickstart as cs;
    use winit::keyboard::KeyCode as K;
    let c = match code {
        K::Enter | K::NumpadEnter => return Some(cs::KEY_ENTER),
        K::Backspace => return Some(cs::KEY_BACKSPACE),
        K::CapsLock => return Some(cs::KEY_CAPS),
        K::Home | K::Pause => return Some(cs::KEY_SYSTEM),
        K::End => return Some(cs::KEY_HINT),
        K::ArrowUp => return Some(ARROW | cs::ARROW_UP),
        K::ArrowDown => return Some(ARROW | cs::ARROW_DOWN),
        K::ArrowLeft => return Some(ARROW | cs::ARROW_LEFT),
        K::ArrowRight => return Some(ARROW | cs::ARROW_RIGHT),
        K::PageUp => return Some(cs::KEY_SPECIAL[0]),
        K::PageDown => return Some(cs::KEY_SPECIAL[1]),
        K::Insert => return Some(cs::KEY_SPECIAL[2]),
        K::Delete => return Some(cs::KEY_SPECIAL[3]),
        K::Period | K::NumpadDecimal => '.',
        K::Slash => '?',
        K::KeyA => 'a',
        K::KeyB => 'b',
        K::KeyC => 'c',
        K::KeyD => 'd',
        K::KeyE => 'e',
        K::KeyF => 'f',
        K::KeyG => 'g',
        K::KeyH => 'h',
        K::KeyI => 'i',
        K::KeyJ => 'j',
        K::KeyK => 'k',
        K::KeyL => 'l',
        K::KeyM => 'm',
        K::KeyN => 'n',
        K::KeyO => 'o',
        K::KeyP => 'p',
        K::KeyQ => 'q',
        K::KeyR => 'r',
        K::KeyS => 's',
        K::KeyT => 't',
        K::KeyU => 'u',
        K::KeyV => 'v',
        K::KeyW => 'w',
        K::KeyX => 'x',
        K::KeyY => 'y',
        K::KeyZ => 'z',
        K::Digit0 | K::Numpad0 => '0',
        K::Digit1 | K::Numpad1 => '1',
        K::Digit2 | K::Numpad2 => '2',
        K::Digit3 | K::Numpad3 => '3',
        K::Digit4 | K::Numpad4 => '4',
        K::Digit5 | K::Numpad5 => '5',
        K::Digit6 | K::Numpad6 => '6',
        K::Digit7 | K::Numpad7 => '7',
        K::Digit8 | K::Numpad8 => '8',
        K::Digit9 | K::Numpad9 => '9',
        K::Space => ' ',
        _ => return None,
    };
    ribbit_clickstart::key_code(c)
}

/// Sends what the keyboard and mouse have to say, a packet at a time: the
/// line carries about 45 packets a second, so a packet goes only once the
/// last has been delivered.
fn feed_input(cs: &mut ClickStart, input: &Mutex<Input>) {
    if !cs.input_idle() {
        return;
    }
    let mut i = input.lock().unwrap();
    if !i.keys.is_empty() {
        let (k, down) = i.keys.remove(0);
        if k & ARROW != 0 {
            cs.arrow(k & !ARROW, down);
        } else {
            cs.key(k, down);
        }
        return;
    }
    let (dx, dy) = (i.dx.round(), i.dy.round());
    if dx != 0.0 || dy != 0.0 || i.button != i.button_sent {
        let (sx, sy) = (dx.clamp(-255.0, 255.0), dy.clamp(-255.0, 255.0));
        i.dx -= sx;
        i.dy -= sy;
        i.button_sent = i.button;
        cs.mouse(sx as i32, sy as i32, i.button);
    }
}

fn run(s: &Shared) {
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    let mut next = Instant::now();
    let mut resample = Resampler::new(ribbit_clickstart::OUTPUT_RATE);
    let mut mono = Vec::new();
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
            // What the system saved, kept once a second.
            flush_eeprom(&mut s.cs.lock().unwrap(), &s.eeprom_path);
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
        let audio = {
            let mut cs = s.cs.lock().unwrap();
            feed_input(&mut cs, &s.input);
            cs.keep_awake = s.keep_awake.load(Ordering::Relaxed);
            cs.run_frame();
            cs.render(&mut img);
            if cs.powered_off() {
                s.off.store(true, Ordering::Relaxed);
            }
            cs.take_audio()
        };
        s.frame.lock().unwrap().copy_from_slice(&img);
        mono.clear();
        resample.push(&audio, &mut mono);
        let gain = f32::from_bits(s.gain.load(Ordering::Relaxed));
        if let Some(sink) = s.sink.lock().unwrap().as_mut() {
            sink.push_32k(&mono, gain);
        }
    }
}
