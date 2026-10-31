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
use ribbit_didj::autostart::{Art, AutoStart};
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
/// Emulation time a frame may take before the faster processor slows down
/// (of the 16.7 ms a frame lasts; the rest is for drawing and the PC).
const BUSY: f64 = 0.012;

struct Shared {
    didj: Mutex<Didj>,
    /// The latest picture (0x00RRGGBB): width, height, pixels.
    frame: Mutex<(usize, usize, Vec<u32>)>,
    running: AtomicBool,
    quit: AtomicBool,
    /// Buttons held (`button` bits).
    held: AtomicU32,
    /// Volume, as f32 bits.
    gain: AtomicU32,
    /// Speed over the last second, percent of real time (f32 bits).
    speed: AtomicU32,
    /// The fastest the processor may run, in thousandths of its clock
    /// (`Mode::cpu_scale`); `run` uses what the PC has time for.
    cpu_max: AtomicU32,
    /// Frame skipping while the PC can't keep up (`Mode::frame_skip`).
    frame_skip: AtomicBool,
    /// HD drawing scale (1 off).
    hd: AtomicU32,
    sink: Mutex<Option<AudioSink>>,
    /// The Didj kept turning itself off right after starting, so it was
    /// left off (`run`); the app tells the player once.
    gave_up: AtomicBool,
    /// On the PC's USB: the USB/IP server whose client is the cable
    /// (`pump_usb`), and news of the link for the app.
    usb: Mutex<Option<crate::emu::UsbServer>>,
    usb_news: Mutex<Option<crate::twist::UsbNews>>,
    /// A USB/IP client has the device imported: the cable is in.
    usb_host: AtomicBool,
    /// UART0, the Linux console: to the app's window and terminals.
    serial: Arc<crate::serial::Serial>,
}

/// How the console runs: Enhanced mode (`ribbit_didj::enhanced`: the
/// firmware's faults fixed, 50 MB for Linux) or Original, and the
/// processor's clock multiplier; frame skipping (a setting that applies at
/// once, through `DidjSession::set_frame_skip`).
#[derive(Clone, Copy)]
pub struct Mode {
    pub enhanced: bool,
    pub cpu_scale: u32,
    pub frame_skip: bool,
    /// HD drawing: the 3D engine at this many times the resolution (1 off).
    pub hd: u32,
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
    /// `original`'s fingerprint, kept beside the save.
    image: String,
    /// The packages it came with that it was built without
    /// (`package::EXTRAS`: JetPack Heroes, an avatar), for the player.
    pub extras_missing: Vec<String>,
    /// The save was made on another image: this many of the player's files
    /// were carried over from it (`ribbit_didj::carry`).
    pub carried: Option<usize>,
}

impl DidjSession {
    /// Powers on a Didj built from the packages in `dir`.
    /// With `autostart`, the game with that art is then started from the
    /// menu (`ribbit_didj::autostart`). `mode`: Enhanced or Original.
    pub fn start(dir: &Path, save_dir: &Path, cart: Option<Vec<u8>>, sink: Option<AudioSink>, autostart: Option<Art>, mode: Mode, serial: Arc<crate::serial::Serial>) -> anyhow::Result<Self> {
        let cart = cart.map(ribbit_didj::cart::Cart::new).transpose().map_err(anyhow::Error::msg)?;
        let (original, extras_missing) = ribbit_didj::nand_and_extras_missing(dir, true).map_err(anyhow::Error::msg)?;
        let save_path = save_file(dir, save_dir);
        // The save holds blocks changed from the built image: on another
        // image (new packages, Ribbit building it differently, a save from
        // another PC or imported) they would mix two file system layouts.
        // The player's files in it are carried over to this image instead
        // (`carry`), and the old save is kept beside it.
        let image = format!("{:08x}", original.fingerprint());
        let image_path = save_path.with_extension("nandimage");
        let mut flash = original.clone();
        let mut carried = None;
        if save_path.exists() && std::fs::read_to_string(&image_path).ok().as_deref() != Some(image.as_str()) {
            let aside = set_aside(&save_path);
            let files = std::fs::read(&aside).map_err(|e| e.to_string()).and_then(|c| ribbit_didj::carry::user_files(&c, &original));
            match files {
                Ok(files) if !files.is_empty() => match ribbit_didj::nand_with_files(dir, true, &files) {
                    Ok((with, _)) => {
                        tracing::info!(target: "ribbit::saves", "{} was made on another NAND image: {} files carried over, the old save kept as {}", save_path.display(), files.len(), aside.display());
                        flash = with;
                        carried = Some(files.len());
                    }
                    Err(e) => tracing::warn!(target: "ribbit::saves", "carrying {}'s files over: {e}", aside.display()),
                },
                Ok(_) => tracing::warn!(target: "ribbit::saves", "{} was made on another NAND image and holds no files to carry over; kept as {}", save_path.display(), aside.display()),
                Err(e) => tracing::warn!(target: "ribbit::saves", "{}: {e}", aside.display()),
            }
        } else if let Ok(changes) = std::fs::read(&save_path) {
            if let Err(e) = flash.apply_changes(&changes) {
                tracing::warn!(target: "ribbit::saves", "{}: {e}", save_path.display());
                flash = original.clone();
            } else if ribbit_didj::carry::avatars_to_fix(&flash) {
                // Didjis synced over USB: the menu takes them once their
                // meta.inf says Device="Didj" (package::fix_avatar_meta).
                let built = ribbit_didj::carry::user_files(&changes, &original).and_then(|files| ribbit_didj::nand_with_files(dir, true, &files));
                match built {
                    Ok((with, _)) => {
                        tracing::info!(target: "ribbit::saves", "Didjis synced to the console made ready for its menu");
                        flash = with;
                        carried = Some(0);
                    }
                    Err(e) => tracing::warn!(target: "ribbit::saves", "making synced Didjis ready: {e}"),
                }
            }
        }
        set_mode(&mut flash, &original, mode.enhanced);
        if carried.is_some() {
            // Written at once: the old save has been set aside.
            if let Some(d) = save_path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&save_path, flash.changes_from(&original));
            let _ = std::fs::write(&image_path, &image);
        }
        let mut didj = Didj::with_sdram(flash, ribbit_didj::enhanced::sdram_size(mode.enhanced));
        didj.set_cpu_scale(mode.cpu_scale);
        didj.bus.cart = cart;
        let shared = Arc::new(Shared {
            didj: Mutex::new(didj),
            frame: Mutex::new((SCREEN_W, SCREEN_H, vec![0; SCREEN_W * SCREEN_H])),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            held: AtomicU32::new(0),
            gain: AtomicU32::new(1f32.to_bits()),
            speed: AtomicU32::new(100f32.to_bits()),
            cpu_max: AtomicU32::new(mode.cpu_scale.max(1) * 1000),
            frame_skip: AtomicBool::new(mode.frame_skip),
            hd: AtomicU32::new(mode.hd),
            sink: Mutex::new(sink),
            gave_up: AtomicBool::new(false),
            usb: Mutex::new(None),
            usb_news: Mutex::new(None),
            usb_host: AtomicBool::new(false),
            serial,
        });
        shared.serial.note("--- Ribbit: the Didj powers on ---
");
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-didj".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s, autostart)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: "Didj".into(), started, original, save_path, image, extras_missing, carried })
    }

    /// Copies the latest picture into `out`; its size (SCREEN_W x SCREEN_H,
    /// times the HD scale).
    pub fn frame(&self, out: &mut Vec<u32>) -> (usize, usize) {
        let f = self.shared.frame.lock().unwrap();
        out.clone_from(&f.2);
        (f.0, f.1)
    }

    /// Processor time run in the last second, as a percentage of real time.
    pub fn speed_percent(&self) -> f64 {
        f32::from_bits(self.shared.speed.load(Ordering::Relaxed)) as f64
    }

    /// True once after the Didj was left off because it kept turning
    /// itself off right after starting (see `run`).
    pub fn take_gave_up(&self) -> bool {
        self.shared.gave_up.swap(false, Ordering::Relaxed)
    }

    /// Offers the Didj on the PC's USB through `server`, or stops (the
    /// cable comes out). The cable goes in when a USB/IP client imports it.
    pub fn set_usb(&self, server: Option<crate::emu::UsbServer>) {
        if let Some(srv) = &server {
            srv.lock().unwrap().set_info(usb_device());
        }
        let off = server.is_none();
        *self.shared.usb.lock().unwrap() = server;
        if off {
            self.shared.usb_host.store(false, Ordering::Relaxed);
            self.shared.didj.lock().unwrap().bus.udc.attach(false);
        }
    }

    /// News of the USB link since last asked.
    pub fn take_usb_news(&self) -> Option<crate::twist::UsbNews> {
        self.shared.usb_news.lock().unwrap().take()
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

    /// Restarts the console (keeping what it has saved), in `mode`.
    pub fn reset(&self, mode: Mode) {
        let mut d = self.shared.didj.lock().unwrap();
        let (mut flash, cart) = (std::mem::take(&mut d.bus.nand), d.bus.cart.take());
        set_mode(&mut flash, &self.original, mode.enhanced);
        *d = Didj::with_sdram(flash, ribbit_didj::enhanced::sdram_size(mode.enhanced));
        d.set_cpu_scale(mode.cpu_scale);
        d.bus.cart = cart;
        self.shared.cpu_max.store(mode.cpu_scale.max(1) * 1000, Ordering::Relaxed);
        self.set_frame_skip(mode.frame_skip);
        self.set_hd(mode.hd);
    }

    /// HD drawing at `scale` times the resolution (1 off), from the next
    /// frame.
    pub fn set_hd(&self, scale: u32) {
        self.shared.hd.store(scale, Ordering::Relaxed);
    }

    /// Frame skipping on or off (from the game's next frame).
    pub fn set_frame_skip(&self, on: bool) {
        self.shared.frame_skip.store(on, Ordering::Relaxed);
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
        std::fs::write(self.save_path.with_extension("nandimage"), &self.image)?;
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
/// Gives the NAND its factory serial if it has none, and writes Enhanced
/// mode's fixes to it or erases them (Original),
/// before a power-on. A state load keeps the NAND as it was saved: its kernel
/// has read the root file system already.
fn set_mode(flash: &mut Nand, original: &Nand, enhanced: bool) {
    flash.add_factory_data();
    let fixed = ribbit_didj::enhanced::apply(flash, original, enhanced);
    if enhanced && fixed == 0 {
        tracing::warn!(target: "ribbit::didj", "Enhanced mode: this firmware has none of the scripts it fixes");
    }
}

/// Moves a save made on another image aside (`.nandblocks.other-image`,
/// numbered if there is one already) and its `.nandimage` away; where it
/// went.
fn set_aside(save: &Path) -> PathBuf {
    let mut aside = save.with_extension("nandblocks.other-image");
    let mut n = 2;
    while aside.exists() {
        aside = save.with_extension(format!("nandblocks.other-image-{n}"));
        n += 1;
    }
    let _ = std::fs::rename(save, &aside);
    let _ = std::fs::remove_file(save.with_extension("nandimage"));
    aside
}

/// Makes `from` (a Didj save: a `.nandblocks` file, perhaps renamed, from
/// another PC or set aside) the save of the Didj in `dir`; the one there is
/// kept as `.nandblocks.before-import`. Its files are carried over at the
/// next power-on (no `.nandimage`: made on another image).
pub fn import_save(from: &Path, dir: &Path, save_dir: &Path) -> anyhow::Result<PathBuf> {
    let data = std::fs::read(from)?;
    let mut check = ribbit_didj::nand::Nand::erased();
    check.apply_changes(&data).map_err(|e| anyhow::anyhow!("{} is not a Didj save: {e}", from.display()))?;
    let save = save_file(dir, save_dir);
    std::fs::create_dir_all(save_dir)?;
    if save.exists() {
        std::fs::copy(&save, save.with_extension("nandblocks.before-import"))?;
    }
    std::fs::write(&save, &data)?;
    let _ = std::fs::remove_file(save.with_extension("nandimage"));
    Ok(save)
}

pub fn save_file(dir: &Path, save_dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or_else(|| "didj".into(), |s| s.to_string_lossy().into_owned());
    save_dir.join(format!("didj-{name}.nandblocks"))
}

/// The emulation thread: a frame every 1/60 s of wall time while running.
fn run(s: &Shared, art: Option<Art>) {
    let mut auto = art.clone().map(AutoStart::new);
    let mut auto_buttons = 0;
    let mut next = Instant::now();
    let mut rate = 0;
    let mut resample = Resampler::new(32000.0);
    let (mut stereo, mut mono) = (Vec::new(), Vec::new());
    let (mut counted_since, mut frames) = (Instant::now(), 0u32);
    // Frame skipping: the game's buffer flips seen, and frames since one.
    let (mut flips, mut since_flip) = (0u64, 0u32);
    // Power-offs in a row that came within a minute of starting.
    let mut quick_offs = 0;
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
        // Behind: this frame starts late.
        let behind = Instant::now() > next + Duration::from_millis(2);
        if Instant::now() > next + Duration::from_millis(100) {
            next = Instant::now();
        }
        next += Duration::from_secs_f64(1.0 / FPS);
        let picture = {
            let mut d = s.didj.lock().unwrap();
            d.bus.buttons = s.held.load(Ordering::Relaxed) | auto_buttons;
            // Frame skipping (a setting; not while the autostart reads the
            // screen): asked for while behind, it starts at the game's next
            // frame. A game that stops flipping buffers (a still screen drawn
            // once) is drawn again after a few frames.
            d.bus.skip_frames = auto.is_none() && behind && s.frame_skip.load(Ordering::Relaxed);
            (flips, since_flip) = if d.bus.flips != flips { (d.bus.flips, 0) } else { (flips, since_flip + 1) };
            if since_flip > 3 {
                d.bus.ga3d.skip = false;
                d.bus.shown_skipped = false;
            }
            // HD drawing (also after a power-on, a new console).
            let hd = s.hd.load(Ordering::Relaxed) as usize;
            if d.bus.ga3d.hd.scale != hd.clamp(1, 4) {
                d.bus.ga3d.hd.set_scale(hd);
            }
            let cycles = d.cpu_hz() / FPS as u64;
            let typed = s.serial.take_input();
            d.bus.uart_rx.extend(typed);
            let t = Instant::now();
            // On USB: in 16 pieces (~1 ms), passing transfers between, so
            // the PC's next one need not wait for the next frame.
            if s.usb.lock().unwrap().is_some() {
                for _ in 0..16 {
                    pump_usb(s, &mut d);
                    d.run(cycles / 16);
                }
                pump_usb(s, &mut d);
            } else {
                d.run(cycles);
            }
            // UART0's output (each byte kept as a char); the other UARTs
            // carry nothing.
            if let Some(out) = d.bus.uart.first_mut().map(std::mem::take) {
                s.serial.output(&out.chars().map(|c| c as u8).collect::<Vec<u8>>());
            }
            d.bus.uart.iter_mut().for_each(String::clear);
            // The system turned itself off (after installing a cartridge's
            // update, as a real Didj does): on again, as its owner would, and
            // the game started again. Not a third time in a row within a
            // minute of starting: in Original mode the Didj also turns off
            // when the game crashes, and starting it again looped for good
            // (a player: health and safety, shutting down, again).
            if d.bus.powered_off && !s.gave_up.load(Ordering::Relaxed) {
                quick_offs = if d.cpu.cycles < 60 * d.cpu_hz() { quick_offs + 1 } else { 0 };
                if quick_offs >= 3 {
                    tracing::warn!(target: "ribbit::didj", "the Didj turned itself off 3 times in a row soon after starting; leaving it off");
                    s.gave_up.store(true, Ordering::Relaxed);
                }
            }
            if d.bus.powered_off && !s.gave_up.load(Ordering::Relaxed) {
                tracing::info!(target: "ribbit::didj", "the Didj turned itself off; turning it on again");
                s.serial.note("--- Ribbit: the Didj turned itself off; on again ---
");
                let (flash, cart, sdram, milli) = (std::mem::take(&mut d.bus.nand), d.bus.cart.take(), d.bus.sdram.len(), d.cpu_milli());
                *d = Didj::with_sdram(flash, sdram);
                d.set_cpu_milli(milli);
                d.bus.cart = cart;
                auto = art.clone().map(AutoStart::new);
            }
            // A faster processor (Enhanced) only as far as the PC keeps up:
            // slower when a frame took too long, never below the Didj's own
            // speed, back up while there is time to spare.
            let (took, cur) = (t.elapsed().as_secs_f64(), d.cpu_milli());
            let want = if took > BUSY {
                (cur as f64 * BUSY / took) as u32
            } else if took < BUSY * 0.75 && !d.bus.skip_frames {
                // Slowly (1x to 2x in about a second), so a busy frame after
                // quiet ones doesn't find it too fast.
                cur + cur / 64
            } else {
                cur
            };
            let want = want.clamp(1000, s.cpu_max.load(Ordering::Relaxed).max(1000));
            if want.abs_diff(cur) >= 10 {
                d.set_cpu_milli(want);
            }
            if d.bus.dma.pcm_rate != rate {
                rate = d.bus.dma.pcm_rate;
                resample = Resampler::new(rate as f64);
            }
            stereo.clear();
            let pcm = &mut d.bus.dma.pcm;
            let bytes = pcm.len() & !3;
            stereo.extend(pcm.drain(..bytes).collect::<Vec<u8>>().as_chunks::<2>().0.iter().map(|b| i16::from_le_bytes(*b)));
            // A buffer whose drawing was skipped: the last picture stays.
            // The autostart reads the console's own picture.
            (!d.bus.shown_skipped).then(|| (d.bus.frame_hd(), auto.is_some().then(|| d.bus.frame())))
        };
        if let Some(((w, h, px), console)) = picture {
            if let Some(a) = &mut auto {
                let (cw, ch, cpx) = console.as_ref().unwrap();
                let screen = if (*cw, *ch) == (SCREEN_W, SCREEN_H) { &cpx[..] } else { &[] };
                auto_buttons = a.frame(screen, s.held.load(Ordering::Relaxed));
                // Its last press ends a frame later (done() is true on the
                // press's last frame): dropped then, A stayed held for good,
                // and the game never saw another press of it.
                if a.done() && auto_buttons == 0 {
                    auto = None;
                }
            }
            if w % SCREEN_W == 0 && h % SCREEN_H == 0 && w * h == px.len() {
                *s.frame.lock().unwrap() = (w, h, px);
            }
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

/// Passes the PC's transfers (USB/IP) to the Didj's USB controller
/// (ribbit_didj::udc) and its answers back. The cable is in while a client
/// has the device imported, also across a power-on (a new console).
fn pump_usb(s: &Shared, d: &mut Didj) {
    use ribbit_didj::udc::Transfer;
    use ribbit_twist::usbip::Event;
    let Some(srv) = s.usb.lock().unwrap().clone() else { return };
    let srv = srv.lock().unwrap();
    while let Some(e) = srv.next_event() {
        match e {
            Event::Attached => {
                s.usb_host.store(true, Ordering::Relaxed);
                *s.usb_news.lock().unwrap() = Some(crate::twist::UsbNews::Attached);
            }
            Event::Detached => {
                s.usb_host.store(false, Ordering::Relaxed);
                d.bus.udc.attach(false);
                *s.usb_news.lock().unwrap() = Some(crate::twist::UsbNews::Detached);
            }
            Event::Submit(u) => {
                if !d.bus.udc.attached && s.usb_host.load(Ordering::Relaxed) {
                    d.bus.udc.attach(true);
                }
                d.bus.udc.submit(Transfer { seq: u.seq, endpoint: u.endpoint, dir_in: u.dir_in, setup: u.setup, data: u.data, length: u.length });
            }
            Event::Unlink(seq) => {
                d.bus.udc.unlink(seq);
            }
        }
    }
    if s.usb_host.load(Ordering::Relaxed) && !d.bus.udc.attached {
        d.bus.udc.attach(true);
    }
    for (seq, status, data, length) in d.bus.udc.take_done() {
        srv.complete(seq, status, data, length);
    }
}

/// The Didj as USB/IP lists it before a client imports it (its device
/// descriptor, VERIFIED with examples/didjusb.rs: vendor 0x0F63, LeapFrog;
/// product 0x0800; release 3.18; high speed; one mass-storage interface,
/// SCSI, Bulk-Only).
pub fn usb_device() -> ribbit_twist::usbip::DeviceInfo {
    ribbit_twist::usbip::DeviceInfo {
        busid: "1-1".into(),
        vendor: 0x0F63,
        product: 0x0800,
        bcd_device: 0x0318,
        class: (0, 0, 0),
        configuration: 1,
        num_configurations: 1,
        interfaces: vec![(8, 6, 0x50)],
        speed: 3,
    }
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
    if ribbit_didj::package::is_bundle(&name) {
        return describe_bundle(path);
    }
    let data = std::fs::read(path).ok()?;
    let id = ribbit_didj::package::identify(&name, &data)?;
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
    let status = if tag == "Game" {
        "early emulation: Ribbit starts it from the Didj's menu (a console set up with a profile), built from the packages in this folder"
    } else {
        "early emulation: starts at the Didj's menu, built from the packages in this folder"
    };
    details.push(("Status", status.to_string()));
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

/// A bundle of the Didj's packages (`bios.didj`) in the library: the
/// console, when it holds the Device Firmware package.
fn describe_bundle(path: &Path) -> Option<crate::library::Game> {
    use crate::library::{Art, Family, Game};
    let data = std::fs::read(path).ok()?;
    let names: Vec<String> = ribbit_core::zip::entries(&data).ok()?.into_iter().map(|e| e.name).collect();
    let ids: Vec<&str> = names.iter().filter_map(|n| ribbit_didj::package::package_id(n)).collect();
    if !ids.iter().any(|id| ribbit_didj::package::product(id).eq_ignore_ascii_case("0x000E0003")) {
        return None;
    }
    let mut details = vec![("System", "Didj".to_string()), ("Packages", ids.len().to_string())];
    let missing = ribbit_didj::package::missing(names.iter().map(String::as_str));
    if !missing.is_empty() {
        details.push(("Missing", missing.join(", ")));
    }
    let extras = ribbit_didj::package::missing_extras(names.iter().map(String::as_str));
    if !extras.is_empty() {
        details.push(("Not included (optional)", extras.join(", ")));
    }
    details.push(("Status", "early emulation: starts at the Didj's menu, built from the packages in this bundle".to_string()));
    details.push(("File", path.display().to_string()));
    let mb = (data.len() as u64).div_ceil(1 << 20);
    Some(Game {
        path: path.to_path_buf(),
        family: Family::Didj,
        title: "Didj".to_string(),
        subtitle: format!("{} packages  ·  {mb} MB", ids.len()),
        details,
        tags: vec!["System".into()],
        key: "didj-bundle".to_string(),
        art: Art::Missing,
    })
}

/// The art the menu shows for the game in an application package, or on
/// the cartridge `cart` (meta.inf `LargeIcon` when it is panel-sized, else
/// `PreviewImage`: Hannah Montana's LargeIcon is a 64x64 icon and its
/// panel shows the PreviewImage), for `AutoStart`. None for the system
/// packages.
pub fn menu_art(path: &Path, cart: Option<&[u8]>) -> Option<Art> {
    use ribbit_didj::package::Meta;
    let image = if let Some(data) = cart {
        let volume = ribbit_didj::cart::volume(data)?;
        ribbit_core::fatfs::list(&volume, "")?.into_iter().filter(|e| e.is_dir).find_map(|e| {
            let meta = Meta::parse(&String::from_utf8_lossy(&ribbit_core::fatfs::read_file(&volume, &format!("{}/meta.inf", e.name))?));
            panel_art(&meta, |f| ribbit_core::fatfs::read_file(&volume, &format!("{}/{f}", e.name)))
        })?
    } else {
        let data = std::fs::read(path).ok()?;
        let entries = ribbit_core::zip::entries(&data).ok()?;
        let (dir, meta) = entries.iter().find_map(|e| {
            let dir = e.name.strip_suffix("/meta.inf").filter(|d| !d.contains('/'))?;
            Some((dir.to_string(), Meta::parse(&String::from_utf8_lossy(&ribbit_core::zip::read_entry(&data, e).ok()?))))
        })?;
        if meta.get("Type").map(str::trim) != Some("Application") {
            return None;
        }
        panel_art(&meta, |f| ribbit_core::zip::read_entry(&data, entries.iter().find(|e| e.name == format!("{dir}/{f}"))?).ok())?
    };
    let pixels = image.pixels.iter().map(|c| u32::from_be_bytes([c.a(), c.r(), c.g(), c.b()])).collect();
    Some(Art { width: image.size[0], pixels })
}

/// The picture a game's panel shows (`menu_art`), read with `read`.
fn panel_art(meta: &ribbit_didj::package::Meta, read: impl Fn(&str) -> Option<Vec<u8>>) -> Option<egui::ColorImage> {
    let mut found = ["LargeIcon", "PreviewImage"].into_iter().filter_map(|k| crate::manual::decode_png(&read(meta.get(k)?)?));
    let first = found.next()?;
    if first.size[0] >= 256 { Some(first) } else { found.next().or(Some(first)) }
}

/// A Didj cartridge dump in the library, titled from the meta.inf of the
/// game on it, its preview image as the art.
pub fn describe_cart(path: &Path, data: &[u8]) -> Option<crate::library::Game> {
    use crate::library::{Art, Family, Game};
    use ribbit_core::fatfs;
    // A raw dump, or an image of its FAT volume alone.
    let volume = if ribbit_didj::cart::is_volume(data) { std::borrow::Cow::Borrowed(data) } else { std::borrow::Cow::Owned(ribbit_didj::cart::volume(data)?) };
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
    details.push(("Status", "early emulation: Ribbit starts it from the Didj's menu (a console set up with a profile)".to_string()));
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

/// The dump at `path` if it is a Didj cartridge (zipped or not): the raw
/// flash, or an image of its FAT volume alone, made into the raw flash.
pub fn read_cart(path: &Path) -> Option<Vec<u8>> {
    use ribbit_didj::cart;
    let raw = path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip"));
    let len = std::fs::metadata(path).ok()?.len() as usize;
    if raw && len % (cart::PAGE_RAW * cart::PAGES_PER_BLOCK) != 0 && len % 15360 != 0 {
        return None;
    }
    let data = ribbit_core::read_image(path).ok()?;
    if cart::Cart::looks_like(&data) {
        return Some(data);
    }
    cart::is_volume(&data).then(|| cart::from_volume(&data).ok()).flatten()
}

/// Whether `path` is a Didj package or a bundle of them (`bios.didj`), by
/// its name, listed or not.
pub fn is_package(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        let n = n.to_string_lossy();
        ribbit_didj::package::package_id(&n).is_some() || ribbit_didj::package::is_bundle(&n) || renamed_package(path).is_some()
    })
}

/// The package ID of a Didj `.lfp` not named by it (players rename them:
/// "JetPack Heroes.lfp"), from its meta.inf. RockIt Twist packages are
/// `.lfp` files too; they say another device.
fn renamed_package(path: &Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    // Another console's package by its own name (LSTR-0x..., PCKT-0x...)
    // is not read.
    let other = name.get(4..7) == Some("-0x");
    if other || !name.to_ascii_lowercase().ends_with(".lfp") || std::fs::metadata(path).ok()?.len() > 256 << 20 {
        return None;
    }
    ribbit_didj::package::identify(&name, &std::fs::read(path).ok()?)
}

/// The Didj's system package (the one the library lists as the console) in
/// `dir` or up to `depth` folders below it, found by its name.
pub fn find_package(dir: &Path, depth: u32) -> Option<PathBuf> {
    let entries: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    let system = |p: &&PathBuf| {
        is_package(p) && p.file_name().is_some_and(|n| n.to_string_lossy().to_ascii_uppercase().contains("0X000E0003") || ribbit_didj::package::is_bundle(&n.to_string_lossy()))
    };
    entries.iter().find(system).cloned().or_else(|| {
        (depth > 0).then(|| entries.iter().filter(|p| p.is_dir()).find_map(|d| find_package(d, depth - 1))).flatten()
    })
}
