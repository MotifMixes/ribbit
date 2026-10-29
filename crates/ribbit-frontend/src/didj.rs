//! Playing the LeapFrog Didj (`ribbit_didj`): its internal NAND is built from
//! the firmware packages (`DIDJ-0x...lfp`) in one folder, as LeapFrog Connect
//! installs them, and the machine runs on its own thread, a 60th of a second
//! of processor cycles per frame. What the system writes to the NAND (its
//! settings, players, progress) is kept in the save folder as the erase
//! blocks that differ from the freshly built image.
//!
//! A cartridge dump (the raw NAND with its spare areas) is played in a Didj
//! built from the packages of the Didj found in the library; the menu
//! starts the game on it. The cartridge is read-only (Brio mounts it so),
//! so nothing on it is saved.
//!
//! Save states hold the machine and the NAND's changes (loading one puts the
//! NAND back as it was then, which the system's file systems need), not the
//! cartridge: they are kept per game title.
//!
//! Controls: the D-pad, A and B as they are; the shoulder buttons L and R on
//! the Leapster's volume buttons (LB and RB on a gamepad); Home is the Didj's
//! Menu button, Hint and Pause its own, and Brightness Up its brightness
//! button.

use crate::audio_out::AudioSink;
use crate::zippity::Resampler;
use ribbit_didj::machine::{Didj, button};
use ribbit_didj::mlc::{HEIGHT, WIDTH};
use ribbit_didj::nand::Nand;
use ribbit_input::Button;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SCREEN_W: usize = WIDTH;
pub const SCREEN_H: usize = HEIGHT;

/// Ribbit's buttons as the Didj's.
pub const CONTROLS: [(Button, u32, &str); 11] = [
    (Button::Up, button::UP, "Up"),
    (Button::Down, button::DOWN, "Down"),
    (Button::Left, button::LEFT, "Left"),
    (Button::Right, button::RIGHT, "Right"),
    (Button::A, button::A, "A"),
    (Button::B, button::B, "B"),
    (Button::VolumeDown, button::L, "L (left shoulder)"),
    (Button::VolumeUp, button::R, "R (right shoulder)"),
    (Button::Home, button::MENU, "Menu"),
    (Button::Hint, button::HINT, "Hint"),
    (Button::Pause, button::PAUSE, "Pause"),
];

/// Frames a second.
const FPS: f64 = 60.0;

struct Shared {
    didj: Mutex<Didj>,
    /// The latest picture, 0x00RRGGBB, SCREEN_W x SCREEN_H.
    frame: Mutex<Vec<u32>>,
    running: AtomicBool,
    quit: AtomicBool,
    /// Buttons held (`button` bits).
    held: AtomicU32,
    /// Volume, as f32 bits.
    gain: AtomicU32,
    /// Speed over the last second, percent of real time (f32 bits).
    speed: AtomicU32,
    sink: Mutex<Option<AudioSink>>,
}

pub struct DidjSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub title: String,
    /// When play started (Unix seconds), for Discord's timer.
    pub started: u64,
    /// The NAND as built from the packages, and where the system's changes
    /// to it are kept.
    original: Nand,
    save_path: PathBuf,
}

impl DidjSession {
    /// Powers on a Didj built from the packages in `dir`.
    pub fn start(dir: &Path, save_dir: &Path, cart: Option<Vec<u8>>, sink: Option<AudioSink>) -> anyhow::Result<Self> {
        let cart = cart.map(ribbit_didj::cart::Cart::new).transpose().map_err(anyhow::Error::msg)?;
        let original = ribbit_didj::nand_from_packages(dir, true).map_err(anyhow::Error::msg)?;
        let save_path = save_file(dir, save_dir);
        let mut flash = original.clone();
        if let Ok(changes) = std::fs::read(&save_path)
            && let Err(e) = flash.apply_changes(&changes)
        {
            tracing::warn!(target: "ribbit::saves", "{}: {e}", save_path.display());
            flash = original.clone();
        }
        let mut didj = Didj::new(flash);
        didj.bus.cart = cart;
        let shared = Arc::new(Shared {
            didj: Mutex::new(didj),
            frame: Mutex::new(vec![0; SCREEN_W * SCREEN_H]),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            held: AtomicU32::new(0),
            gain: AtomicU32::new(1f32.to_bits()),
            speed: AtomicU32::new(100f32.to_bits()),
            sink: Mutex::new(sink),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-didj".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: "Didj".into(), started, original, save_path })
    }

    /// Copies the latest picture into `out` (SCREEN_W x SCREEN_H).
    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clone_from(&self.shared.frame.lock().unwrap());
    }

    /// Processor time run in the last second, as a percentage of real time.
    pub fn speed_percent(&self) -> f64 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed)) as f64
    }

    pub fn set_running(&self, running: bool) {
        self.shared.running.store(running, Ordering::Relaxed);
    }

    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    /// A Ribbit button, as the Didj button it stands for (`CONTROLS`).
    pub fn set_button(&self, b: Button, pressed: bool) {
        let bits = match CONTROLS.iter().find(|c| c.0 == b) {
            Some(&(_, bits, _)) => bits,
            None if b == Button::BrightnessUp => button::BRIGHTNESS,
            None => return,
        };
        if pressed {
            self.shared.held.fetch_or(bits, Ordering::Relaxed);
        } else {
            self.shared.held.fetch_and(!bits, Ordering::Relaxed);
        }
    }

    /// The console now, for a save slot (its NAND as changes from the built
    /// image; the cartridge left out).
    pub fn save_state(&self) -> Vec<u8> {
        self.shared.didj.lock().unwrap().save_state(&self.original)
    }

    /// Restores a state from `save_state`. The NAND goes back to what it
    /// held then, as the system's memory expects.
    pub fn load_state(&self, state: &[u8]) -> Result<(), String> {
        self.shared.didj.lock().unwrap().load_state(state, &self.original)
    }

    /// Restarts the console (keeping what it has saved).
    pub fn reset(&self) {
        let mut d = self.shared.didj.lock().unwrap();
        let (flash, cart) = (std::mem::take(&mut d.bus.nand), d.bus.cart.take());
        *d = Didj::new(flash);
        d.bus.cart = cart;
    }

    /// Keeps what the system has saved to its NAND.
    pub fn flush(&self) -> anyhow::Result<()> {
        let changes = self.shared.didj.lock().unwrap().bus.nand.changes_from(&self.original);
        if changes.is_empty() && !self.save_path.exists() {
            return Ok(());
        }
        if let Some(dir) = self.save_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = self.save_path.with_extension("tmp");
        std::fs::write(&tmp, &changes)?;
        std::fs::rename(&tmp, &self.save_path)?;
        Ok(())
    }

    /// Turns the console off (its power button, `power_off`) and keeps its
    /// saves.
    pub fn stop(mut self) {
        self.shared.quit.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Err(e) = self.flush() {
            tracing::warn!(target: "ribbit::saves", "saving the Didj's NAND: {e:#}");
        }
    }
}

/// Where the system's changes to the NAND built from `dir` are kept.
pub fn save_file(dir: &Path, save_dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or_else(|| "didj".into(), |s| s.to_string_lossy().into_owned());
    save_dir.join(format!("didj-{name}.nandblocks"))
}

/// The emulation thread: a frame every 1/60 s of wall time while running.
fn run(s: &Shared) {
    let mut next = Instant::now();
    let mut rate = 0;
    let mut resample = Resampler::new(32000.0);
    let (mut stereo, mut mono) = (Vec::new(), Vec::new());
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
        let picture = {
            let mut d = s.didj.lock().unwrap();
            d.bus.buttons = s.held.load(Ordering::Relaxed);
            let cycles = d.cpu_hz() / FPS as u64;
            d.run(cycles);
            if d.bus.dma.pcm_rate != rate {
                rate = d.bus.dma.pcm_rate;
                resample = Resampler::new(rate as f64);
            }
            stereo.clear();
            let pcm = &mut d.bus.dma.pcm;
            let bytes = pcm.len() & !3;
            stereo.extend(pcm.drain(..bytes).collect::<Vec<u8>>().as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)));
            d.bus.frame()
        };
        let (w, h, px) = picture;
        if w == SCREEN_W && h == SCREEN_H {
            s.frame.lock().unwrap().copy_from_slice(&px);
        }
        mono.clear();
        resample.push(&stereo, &mut mono);
        let gain = f32::from_bits(s.gain.load(Ordering::Relaxed));
        if let Some(sink) = s.sink.lock().unwrap().as_mut() {
            sink.push_32k(&mono, gain);
        }
    }
    power_off(s);
}

/// Turns the Didj off with its power button, as a player would: Linux then
/// unmounts its file systems before cutting the power. Stopping it without
/// this is pulling the batteries: a file being written is left half done,
/// the next start's fsck finds it, and the console refuses to mount /Didj
/// ("Download Failed"). Runs as fast as the host allows; gives up after
/// 15 s (say, before the system has started).
fn power_off(s: &Shared) {
    let mut d = s.didj.lock().unwrap();
    let hz = d.cpu_hz();
    let t = Instant::now();
    d.bus.buttons = button::POWER;
    d.run(hz / 10);
    d.bus.buttons = 0;
    while !d.bus.powered_off && t.elapsed() < Duration::from_secs(15) {
        d.run(hz / FPS as u64);
        d.bus.dma.pcm.clear();
    }
    if d.bus.powered_off {
        tracing::info!(target: "ribbit::saves", "Didj powered off in {:.1} s", t.elapsed().as_secs_f32());
    } else {
        tracing::warn!(target: "ribbit::saves", "the Didj did not power off in 15 s; saving its NAND as it is");
    }
}

/// What a Didj package (`DIDJ-0x...lfp`) is in the library: the Device
/// Firmware package stands for the console (its menu); an application
/// package (a game) is listed too. The other system packages are not
/// games; they are only needed beside these.
pub fn describe_package(path: &Path) -> Option<crate::library::Game> {
    use crate::library::{Art, Family, Game};
    let name = path.file_name()?.to_string_lossy().into_owned();
    let id = ribbit_didj::package::package_id(&name)?.to_string();
    let data = std::fs::read(path).ok()?;
    let meta = ribbit_core::zip::entries(&data)
        .ok()?
        .into_iter()
        .find(|e| e.name.to_ascii_lowercase().ends_with("meta.inf"))
        .and_then(|e| ribbit_core::zip::read_entry(&data, &e).ok())
        .map(|m| ribbit_didj::package::Meta::parse(&String::from_utf8_lossy(&m)))?;
    if !meta.is_didj() {
        return None;
    }
    let product = meta.get("ProductID").unwrap_or("");
    let (title, tag) = match meta.get("Type").map(str::trim) {
        // The firmware package carries the kernel and root file system.
        Some("System") if product.eq_ignore_ascii_case("0x000E0003") => ("Didj".to_string(), "System"),
        Some("Application") => (meta.get("Name").unwrap_or(&id).to_string(), "Game"),
        _ => return None,
    };
    let mut details = vec![("System", "Didj".to_string())];
    for (label, key) in [("Name", "Name"), ("Version", "Version"), ("Product", "ProductID")] {
        if let Some(v) = meta.get(key) {
            details.push((label, v.to_string()));
        }
    }
    details.push(("Status", "early emulation: starts at the Didj's menu, built from the packages in this folder".to_string()));
    details.push(("File", path.display().to_string()));
    let mb = (data.len() as u64).div_ceil(1 << 20);
    Some(Game {
        path: path.to_path_buf(),
        family: Family::Didj,
        title,
        subtitle: format!("{}  ·  {mb} MB", meta.get("Version").unwrap_or("")),
        details,
        tags: vec![tag.into()],
        key: format!("didj-{id}"),
        art: Art::Missing,
    })
}

/// A Didj cartridge dump in the library, titled from the meta.inf of the
/// game on it, its preview image as the art.
pub fn describe_cart(path: &Path, data: &[u8]) -> Option<crate::library::Game> {
    use crate::library::{Art, Family, Game};
    use ribbit_core::fatfs;
    let volume = ribbit_didj::cart::volume(data)?;
    let (folder, meta) = fatfs::list(&volume, "")?.into_iter().filter(|e| e.is_dir).find_map(|e| {
        let m = fatfs::read_file(&volume, &format!("{}/meta.inf", e.name))?;
        Some((e.name, ribbit_didj::package::Meta::parse(&String::from_utf8_lossy(&m))))
    })?;
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let title = meta.get("Name").map_or(stem, str::to_string);
    let art = ["PreviewImage", "Icon"]
        .iter()
        .filter_map(|k| meta.get(k))
        .find_map(|f| crate::manual::decode_png(&fatfs::read_file(&volume, &format!("{folder}/{f}"))?))
        .map_or(Art::Missing, Art::Image);
    let mut details = vec![("System", "Didj".to_string())];
    for (label, key) in [("Name", "Name"), ("Version", "Version"), ("Product", "ProductID"), ("Publisher", "Publisher"), ("Developer", "Developer")] {
        if let Some(v) = meta.get(key) {
            details.push((label, v.to_string()));
        }
    }
    details.push(("Status", "early emulation: starts at the Didj's menu with the cartridge in".to_string()));
    details.push(("File", path.display().to_string()));
    let id = meta.get("ProductID").unwrap_or("").to_string();
    Some(Game {
        path: path.to_path_buf(),
        family: Family::Didj,
        title,
        subtitle: format!("Cartridge  ·  {}", meta.get("Version").map_or(String::new(), |v| format!("version {v}"))),
        details,
        tags: vec!["Cartridge".into()],
        key: format!("didj-cart-{id}"),
        art,
    })
}

/// The dump at `path` if it is a Didj cartridge (zipped or not).
pub fn read_cart(path: &Path) -> Option<Vec<u8>> {
    let raw = path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip"));
    let len = std::fs::metadata(path).ok()?.len() as usize;
    if raw && len % (ribbit_didj::cart::PAGE_RAW * ribbit_didj::cart::PAGES_PER_BLOCK) != 0 {
        return None;
    }
    let data = ribbit_core::read_image(path).ok()?;
    ribbit_didj::cart::Cart::looks_like(&data).then_some(data)
}

/// Whether `path` is a Didj package (by its name), listed or not.
pub fn is_package(path: &Path) -> bool {
    path.file_name().is_some_and(|n| ribbit_didj::package::package_id(&n.to_string_lossy()).is_some())
}
