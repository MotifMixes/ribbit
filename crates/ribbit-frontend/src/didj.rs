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
    /// The 3D engine dithers where games ask for it (`Ga3d::dither`).
    dither: AtomicBool,
    sink: Mutex<Option<AudioSink>>,
    /// The Didj kept turning itself off right after starting, so it was
    /// left off (`run`); the app tells the player once.
    gave_up: AtomicBool,
    /// "Turn off when idle" off: AppManager's idle timer is kept at 0.
    keep_awake: AtomicBool,
    /// It shut down for idling with that setting on: it stays off and the
    /// app closes it.
    idle_off: AtomicBool,
    /// On the PC's USB: the USB/IP server whose client is the cable
    /// (`pump_usb`), and news of the link for the app.
    usb: Mutex<Option<crate::emu::UsbServer>>,
    usb_news: Mutex<Option<crate::twist::UsbNews>>,
    /// A USB/IP client has the device imported: the cable is in.
    usb_host: AtomicBool,
    /// UART0, the Linux console: to the app's window and terminals.
    serial: Arc<crate::serial::Serial>,
    /// A Leapster Explorer (`ribbit_didj::explorer`), not a Didj.
    explorer: bool,
    /// The Explorer's stylus on the screen (pixels), None while lifted.
    touch: Mutex<Option<(f32, f32)>>,
    /// Enhanced (new): the Didj's programs on the HLE, without its kernel
    /// (`Hle`). `didj` then only keeps the NAND the saves are written to.
    hle: Mutex<Option<Hle>>,
    /// A Didj's save (`SaveTo`), written by `run` too whenever the
    /// console has turned itself off cleanly (None: the Explorer's, which
    /// `flush` alone writes).
    save_to: Option<SaveTo>,
}

/// Where a Didj's NAND changes are kept: the image they are changes from,
/// the save file and the image's fingerprint (`.nandimage`).
struct SaveTo {
    original: Arc<Nand>,
    path: PathBuf,
    image: String,
}

impl SaveTo {
    /// Writes `flash`'s changes (through a temporary file, so a save is
    /// never left half written).
    fn write(&self, flash: &Nand) -> std::io::Result<()> {
        let changes = flash.changes_from(&self.original);
        write_save(&self.path, &self.image, &changes, Some(&ribbit_didj::carry::files_beyond(flash, &self.original)))
    }
}

/// Writes a save's changes and the fingerprint of the image they are
/// changes from, and a Didj's player's files beside them (`.files`,
/// `carry::pack`: what is carried over if the image changes).
fn write_save(path: &Path, image: &str, changes: &[u8], files: Option<&[(String, Vec<u8>)]>) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, changes)?;
    std::fs::rename(&tmp, path)?;
    if let Some(files) = files {
        let tmp = path.with_extension("files.tmp");
        std::fs::write(&tmp, ribbit_didj::carry::pack(files, changes))?;
        std::fs::rename(&tmp, path.with_extension("files"))?;
    }
    std::fs::write(path.with_extension("nandimage"), image)
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
    /// Enhanced (new): the programs without the kernel (`Hle`); `enhanced`
    /// is then left alone (the NAND isn't run).
    pub hle: bool,
    /// Enhanced (new): in-between frames, so motion is smooth at 60 fps
    /// whatever the game's own rate (at once, `DidjSession::set_smooth`).
    pub smooth: bool,
}

/// A Leapster Explorer ready to power on (`DidjSession::build_explorer`).
pub struct ExplorerBuilt {
    lx: Didj,
    original: Nand,
    save_path: PathBuf,
    image: String,
    dir: PathBuf,
    /// The player's files carried over from a save made on another image.
    carried: Option<usize>,
}

pub struct DidjSession {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub title: String,
    /// When play started (Unix seconds), for Discord's timer.
    pub started: u64,
    /// The NAND as built from the packages, and where the system's changes
    /// to it are kept.
    original: Arc<Nand>,
    save_path: PathBuf,
    /// `original`'s fingerprint, kept beside the save.
    image: String,
    /// The packages it came with that it was built without
    /// (`package::EXTRAS`: JetPack Heroes, an avatar), for the player.
    pub extras_missing: Vec<String>,
    /// The save was made on another image: this many of the player's files
    /// were carried over from it (`ribbit_didj::carry`).
    pub carried: Option<usize>,
    /// A Leapster Explorer (`start_explorer`).
    pub explorer: bool,
    /// The packages' folder and the cartridge's dump, for building the
    /// HLE again (`reset`).
    dir: PathBuf,
    cart: Option<Vec<u8>>,
}

impl DidjSession {
    /// Powers on a Didj built from the packages in `dir`.
    /// With `autostart`, the game with that art is then started from the
    /// menu (`ribbit_didj::autostart`). `mode`: Enhanced or Original.
    pub fn start(dir: &Path, save_dir: &Path, cart: Option<Vec<u8>>, sink: Option<AudioSink>, autostart: Option<Art>, mode: Mode, serial: Arc<crate::serial::Serial>) -> anyhow::Result<Self> {
        let cart_dump = cart.clone();
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
            // The player's files kept beside it (`.files`) when they were
            // kept with this save, else what its blocks give back.
            let kept = std::fs::read(save_path.with_extension("files")).ok();
            let aside = set_aside(&save_path);
            let files = std::fs::read(&aside).map_err(|e| e.to_string()).and_then(|c| match kept.as_deref().and_then(|k| ribbit_didj::carry::unpack(k, &c)) {
                Some(files) => Ok(files),
                None => ribbit_didj::carry::user_files(&c, &original),
            });
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
                // Fixed where they are, the rest of the console untouched:
                // building the image again kept only files at paths the
                // packages don't have (a player's profile didn't persist).
                let mut fixed = flash.clone();
                match ribbit_didj::carry::fix_avatars_in_place(&mut fixed) {
                    Ok(n) => {
                        tracing::info!(target: "ribbit::saves", "Didjis synced to the console made ready for its menu ({n})");
                        flash = fixed;
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
            let changes = flash.changes_from(&original);
            let _ = write_save(&save_path, &image, &changes, Some(&ribbit_didj::carry::files_beyond(&flash, &original)));
        }
        let hle = match mode.hle {
            true => {
                let files = ribbit_didj::carry::files_beyond(&flash, &original);
                Some(Hle::new(dir, cart_dump.as_deref(), files, mode)?)
            }
            false => None,
        };
        let mut didj = Didj::with_sdram(flash, ribbit_didj::enhanced::sdram_size(mode.enhanced && !mode.hle));
        didj.set_cpu_scale(mode.cpu_scale);
        didj.bus.cart = cart;
        let original = Arc::new(original);
        let save_to = SaveTo { original: original.clone(), path: save_path.clone(), image: image.clone() };
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
            dither: AtomicBool::new(false),
            sink: Mutex::new(sink),
            gave_up: AtomicBool::new(false),
            keep_awake: AtomicBool::new(true),
            idle_off: AtomicBool::new(false),
            usb: Mutex::new(None),
            usb_news: Mutex::new(None),
            usb_host: AtomicBool::new(false),
            serial,
            explorer: false,
            touch: Mutex::new(None),
            hle: Mutex::new(hle),
            save_to: Some(save_to),
        });
        shared.serial.note("--- Ribbit: the Didj powers on ---
");
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-didj".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s, autostart)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: "Didj".into(), started, original, save_path, image, extras_missing, carried, explorer: false, dir: dir.to_path_buf(), cart: cart_dump })
    }

    /// Builds a Leapster Explorer from the packages in `dir`
    /// (`ribbit_didj::explorer::nand_from_packages`), with what it saved to
    /// its NAND and NOR flash before. Takes seconds: the app runs it on a
    /// worker thread, then `start_explorer`.
    pub fn build_explorer(dir: &Path, save_dir: &Path) -> anyhow::Result<ExplorerBuilt> {
        use ribbit_didj::explorer::{nand_from_packages_blocks, nand_with_files, NAND_BLOCKS, NAND_BLOCKS_1GB};
        let original = ribbit_didj::explorer::nand_from_packages(dir).map_err(anyhow::Error::msg)?;
        let save_path = explorer_save_file(dir, save_dir);
        let image = format!("{:08x}", original.fingerprint());
        let image_path = save_path.with_extension("nandimage");
        let mut flash = original.clone();
        let mut carried = None;
        // A save made on another image would mix two file system layouts:
        // the player's files on Bulk (profiles, progress, settings.cfg) are
        // carried over to this image instead (`carry`), and the old save is
        // kept beside it. A save made on the 1 GB NAND Ribbit had before
        // 2026-11-08 moves to the 512 MB one: that image is built again,
        // so every file the console wrote or changed comes along.
        let saved_image = std::fs::read_to_string(&image_path).ok();
        if save_path.exists() && saved_image.as_deref() != Some(image.as_str()) {
            let aside = set_aside(&save_path);
            let files = std::fs::read(&aside).map_err(|e| e.to_string()).and_then(|changes| {
                if let Some(old) = saved_image.and_then(|saved| nand_from_packages_blocks(dir, NAND_BLOCKS_1GB).ok().filter(|old| saved == format!("{:08x}", old.fingerprint()))) {
                    let mut was = old.clone();
                    was.apply_changes(&changes)?;
                    Ok(ribbit_didj::carry::explorer_files_beyond(&was, &old))
                } else {
                    ribbit_didj::carry::explorer_user_files(&changes, &original)
                }
            });
            match files {
                Ok(files) if !files.is_empty() => match nand_with_files(dir, NAND_BLOCKS, &files) {
                    Ok(with) => {
                        tracing::info!(target: "ribbit::saves", "{} was made on another NAND image: {} files carried over, the old save kept as {}", save_path.display(), files.len(), aside.display());
                        flash = with;
                        carried = Some(files.values().filter(|e| !matches!(e.kind, ribbit_didj::ubifs::Kind::Dir)).count());
                    }
                    Err(e) => tracing::warn!(target: "ribbit::saves", "carrying {}'s files over: {e}", aside.display()),
                },
                Ok(_) => tracing::warn!(target: "ribbit::saves", "{} was made on another NAND image and holds no files to carry over; kept as {}", save_path.display(), aside.display()),
                Err(e) => tracing::warn!(target: "ribbit::saves", "{}: {e}", aside.display()),
            }
        } else if let Ok(changes) = std::fs::read(&save_path)
            && let Err(e) = flash.apply_changes(&changes)
        {
            tracing::warn!(target: "ribbit::saves", "{}: {e}", save_path.display());
            flash = original.clone();
        }
        if carried.is_some() {
            // Written at once: the old save has been set aside.
            if let Some(d) = save_path.parent() {
                let _ = std::fs::create_dir_all(d);
            }
            let _ = std::fs::write(&save_path, flash.changes_from(&original));
            let _ = std::fs::write(&image_path, &image);
        }
        let mut lx = ribbit_didj::explorer::boot(flash, ribbit_didj::explorer::CMDLINE).map_err(anyhow::Error::msg)?;
        if let Ok(nor) = std::fs::read(save_path.with_extension("nor")) {
            lx.bus.nor = Some(ribbit_didj::nor::Nor::new(nor));
        }
        Ok(ExplorerBuilt { lx, original, save_path, image, carried, dir: dir.to_path_buf() })
    }

    /// Powers on the Leapster Explorer `build_explorer` made.
    /// With `hle`, Enhanced (new): its programs run on `Hle` from the file
    /// systems in its NAND, which `lx` then only keeps for the saves.
    pub fn start_explorer(built: ExplorerBuilt, sink: Option<AudioSink>, serial: Arc<crate::serial::Serial>, hle: bool) -> anyhow::Result<Self> {
        let ExplorerBuilt { lx, original, save_path, image, carried, dir } = built;
        let hle = match hle {
            true => Some(Hle::new_explorer(&dir, &lx.bus.nand, lx.bus.nor.as_ref().map(|n| &n.data[..]), None)?),
            false => None,
        };
        let original = Arc::new(original);
        let shared = Arc::new(Shared {
            didj: Mutex::new(lx),
            frame: Mutex::new((SCREEN_W, SCREEN_H, vec![0; SCREEN_W * SCREEN_H])),
            running: AtomicBool::new(true),
            quit: AtomicBool::new(false),
            held: AtomicU32::new(0),
            gain: AtomicU32::new(1f32.to_bits()),
            speed: AtomicU32::new(100f32.to_bits()),
            cpu_max: AtomicU32::new(1000),
            frame_skip: AtomicBool::new(false),
            hd: AtomicU32::new(1),
            dither: AtomicBool::new(false),
            sink: Mutex::new(sink),
            gave_up: AtomicBool::new(false),
            keep_awake: AtomicBool::new(true),
            idle_off: AtomicBool::new(false),
            usb: Mutex::new(None),
            usb_news: Mutex::new(None),
            usb_host: AtomicBool::new(false),
            serial,
            explorer: true,
            touch: Mutex::new(None),
            hle: Mutex::new(hle),
            save_to: None,
        });
        shared.serial.note("--- Ribbit: the Leapster Explorer powers on ---\n");
        let s = shared.clone();
        let thread = std::thread::Builder::new().name("ribbit-explorer".into()).spawn(move || {
            crate::perf::raise_thread_priority();
            run(&s, None)
        })?;
        let started = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
        Ok(Self { shared, thread: Some(thread), title: "Leapster Explorer".into(), started, original, save_path, image, extras_missing: Vec::new(), carried, explorer: true, dir, cart: None })
    }

    /// Puts a cartridge dump (raw, as `read_cart` gives it) in the
    /// Explorer's slot, or takes the cartridge out (None), while it runs:
    /// the kernel sees the detect pin change and the menu shows the game.
    pub fn set_cart(&self, raw: Option<Vec<u8>>) -> Result<(), String> {
        // Enhanced (new): the HLE's slot (its kernel files and /LF/Cart).
        if let Some(h) = self.shared.hle.lock().unwrap().as_mut().filter(|h| h.explorer) {
            h.p.explorer_cart(raw.as_deref())?;
            h.cart = raw.clone();
        }
        let cart = raw.map(ribbit_didj::cart::Cart::new).transpose()?;
        self.shared.didj.lock().unwrap().bus.cart = cart;
        Ok(())
    }

    /// A cartridge is in the slot.
    pub fn has_cart(&self) -> bool {
        self.shared.didj.lock().unwrap().bus.cart.is_some()
    }

    /// The Explorer's stylus at `pos` on the screen (pixels; None lifts it).
    pub fn set_touch(&self, pos: Option<(f32, f32)>) {
        *self.shared.touch.lock().unwrap() = pos;
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

    /// Whether the Didj may shut itself down for idling ("Turn off when
    /// idle"; a real one does after 7 minutes).
    pub fn set_turn_off_when_idle(&self, on: bool) {
        self.shared.keep_awake.store(!on, Ordering::Relaxed);
    }

    /// True once after it shut down for idling (see `idle_off`).
    pub fn take_idle_off(&self) -> bool {
        self.shared.idle_off.swap(false, Ordering::Relaxed)
    }

    /// Offers the Didj on the PC's USB through `server`, or stops (the
    /// cable comes out). The cable goes in when a USB/IP client imports it.
    pub fn set_usb(&self, server: Option<crate::emu::UsbServer>) {
        if let Some(srv) = &server {
            srv.lock().unwrap().set_info(if self.explorer { explorer_usb_device() } else { usb_device() });
        }
        let off = server.is_none();
        *self.shared.usb.lock().unwrap() = server;
        if off {
            self.shared.usb_host.store(false, Ordering::Relaxed);
            self.shared.didj.lock().unwrap().bus.udc.attach(false);
        }
    }

    /// Why USB isn't offered now (Enhanced (new) has no kernel to drive it).
    pub fn usb_unavailable(&self) -> Option<&'static str> {
        self.is_hle().then_some("USB and LeapFrog Connect need the Didj's own system: switch to Original+ or Original (Emulation > Didj)")
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

    /// The Leapster Explorer's volume up or down button.
    pub fn set_explorer_volume(&self, up: bool, pressed: bool) {
        let bits = if up { button::VOLUME_UP } else { button::VOLUME_DOWN };
        if pressed {
            self.shared.held.fetch_or(bits, Ordering::Relaxed);
        } else {
            self.shared.held.fetch_and(!bits, Ordering::Relaxed);
        }
    }

    /// The console now, for a save slot (its NAND as changes from the built
    /// image; the cartridge left out). In Enhanced (new), the programs and
    /// their files (as changes from the built system).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        if let Some(h) = self.shared.hle.lock().unwrap().as_mut() {
            return h.save_state().map_err(|e| tracing::warn!(target: "ribbit::didj", "Enhanced (new): no state: {e}")).ok();
        }
        Some(self.shared.didj.lock().unwrap().save_state(&self.original))
    }

    /// Restores a state from `save_state`. The NAND goes back to what it
    /// held then, as the system's memory expects.
    pub fn load_state(&self, state: &[u8]) -> Result<(), String> {
        if let Some(h) = self.shared.hle.lock().unwrap().as_mut() {
            return h.load_state(state);
        }
        self.shared.didj.lock().unwrap().load_state(state, &self.original)
    }

    /// Running Enhanced (new): no kernel, so no USB and no save states.
    pub fn is_hle(&self) -> bool {
        self.shared.hle.lock().unwrap().is_some()
    }

    /// Enhanced (new): in-between frames on or off, at once.
    pub fn set_smooth(&self, on: bool) {
        if let Some(h) = self.shared.hle.lock().unwrap().as_mut() {
            h.smooth = on;
        }
    }

    /// Enhanced (new): the player's files written into the NAND kept in
    /// `didj`, as the other modes keep them (`Hle::user_files`).
    fn hle_to_nand(&self) -> anyhow::Result<()> {
        let mut hle = self.shared.hle.lock().unwrap();
        let Some(h) = hle.as_mut() else { return Ok(()) };
        if h.explorer {
            let mut d = self.shared.didj.lock().unwrap();
            return h.explorer_to_nand(&mut d);
        }
        let files = h.user_files();
        if files == h.written {
            return Ok(());
        }
        let (nand, _) = ribbit_didj::nand_with_files(&self.dir, true, &files).map_err(anyhow::Error::msg)?;
        self.shared.didj.lock().unwrap().bus.nand = nand;
        h.written = files;
        Ok(())
    }

    /// Restarts the console (keeping what it has saved), in `mode`.
    pub fn reset(&self, mode: Mode) {
        if let Err(e) = self.hle_to_nand() {
            tracing::warn!(target: "ribbit::saves", "keeping the Didj's files: {e:#}");
        }
        if mode.hle && self.explorer {
            let cart = self.shared.hle.lock().unwrap().as_ref().and_then(|h| h.cart.clone());
            let built = {
                let d = self.shared.didj.lock().unwrap();
                Hle::new_explorer(&self.dir, &d.bus.nand, d.bus.nor.as_ref().map(|n| &n.data[..]), cart)
            };
            match built {
                Ok(h) => *self.shared.hle.lock().unwrap() = Some(h),
                Err(e) => tracing::error!(target: "ribbit::didj", "starting the Explorer's Enhanced (new): {e:#}"),
            }
            return;
        }
        if mode.hle && !self.explorer {
            let files = ribbit_didj::carry::files_beyond(&self.shared.didj.lock().unwrap().bus.nand, &self.original);
            let built = Hle::new(&self.dir, self.cart.as_deref(), files, mode);
            match built {
                Ok(h) => *self.shared.hle.lock().unwrap() = Some(h),
                Err(e) => tracing::error!(target: "ribbit::didj", "starting Enhanced (new): {e:#}"),
            }
            self.set_hd(mode.hd);
            return;
        }
        *self.shared.hle.lock().unwrap() = None;
        // Off the bus while it starts again (as in `run`).
        unplug(&self.shared);
        let mut d = self.shared.didj.lock().unwrap();
        if self.explorer {
            power_on_again(&mut d, true);
            return;
        }
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

    /// 3D dithering as on the real engine, from the next frame.
    pub fn set_dither(&self, on: bool) {
        self.shared.dither.store(on, Ordering::Relaxed);
    }

    /// Frame skipping on or off (from the game's next frame).
    pub fn set_frame_skip(&self, on: bool) {
        self.shared.frame_skip.store(on, Ordering::Relaxed);
    }

    /// Keeps what the system has saved to its NAND.
    pub fn flush(&self) -> anyhow::Result<()> {
        self.hle_to_nand()?;
        let changes = self.shared.didj.lock().unwrap().bus.nand.changes_from(&self.original);
        if changes.is_empty() && !self.save_path.exists() {
            return Ok(());
        }
        let files = (!self.explorer).then(|| ribbit_didj::carry::files_beyond(&self.shared.didj.lock().unwrap().bus.nand, &self.original));
        write_save(&self.save_path, &self.image, &changes, files.as_deref())?;
        // The Explorer's NOR flash holds its factory data (Ribbit's touch
        // calibration among it): kept whole once the system has written it.
        let nor = self.shared.didj.lock().unwrap().bus.nor.as_ref().filter(|n| n.dirty).map(|n| n.data.clone());
        if let Some(nor) = nor {
            std::fs::write(self.save_path.with_extension("nor"), nor)?;
        }
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

/// What a Didj save folder import brought (`import_didj_folder`).
pub struct DidjFolderImport {
    pub files: usize,
    pub profiles: usize,
    /// Games installed from their packages (copied beside this Didj's).
    pub packages: usize,
    /// The Didj's save before, if it had one.
    pub backup: Option<PathBuf>,
}

/// The player's files in this Didj's save (none without one): read whole
/// when it was made on `original`, else as `start` carries them.
fn players_files(save: &Path, original: &Nand) -> Vec<(String, Vec<u8>)> {
    let Ok(changes) = std::fs::read(save) else { return Vec::new() };
    let image = format!("{:08x}", original.fingerprint());
    if std::fs::read_to_string(save.with_extension("nandimage")).ok().as_deref() == Some(image.as_str()) {
        let mut flash = original.clone();
        if flash.apply_changes(&changes).is_ok() {
            return ribbit_didj::carry::files_beyond(&flash, original);
        }
    }
    let kept = std::fs::read(save.with_extension("files")).ok();
    kept.as_deref().and_then(|k| ribbit_didj::carry::unpack(k, &changes)).or_else(|| ribbit_didj::carry::user_files(&changes, original).ok()).unwrap_or_default()
}

/// Brings a Didj save folder (`ribbit_didj::didj_import`: a copy of the
/// Didj's /Didj tree, or a folder with it as `didj` or `assets/didj`)
/// into this Didj's save: profiles,
/// Didjis, themes, game progress and installed games, merged with the
/// player's files the save has. Games it installed that this Didj lacks
/// are installed from their packages (copied into an "Imported" folder
/// beside this Didj's packages) when it kept them, else copied as files.
/// The save before is kept as `.before-folder-import`. The Didj must be off.
pub fn import_didj_folder(from: &Path, dir: &Path, save_dir: &Path) -> anyhow::Result<DidjFolderImport> {
    let (original, _) = ribbit_didj::nand_and_extras_missing(dir, true).map_err(anyhow::Error::msg)?;
    let import = ribbit_didj::didj_import::read(from, &original).map_err(anyhow::Error::msg)?;
    if import.files.is_empty() && import.packages.is_empty() {
        anyhow::bail!("{}: this Didj folder holds nothing of a player's (no profiles, Didjis or game progress)", from.display());
    }
    let save = save_file(dir, save_dir);
    let ours = players_files(&save, &original);
    std::fs::create_dir_all(save_dir)?;
    let backup = save.with_extension("nandblocks.before-folder-import");
    let had = save.exists();
    if had {
        std::fs::copy(&save, &backup)?;
        let _ = std::fs::copy(save.with_extension("nandimage"), backup.with_extension("nandimage"));
    }
    let mut original = original;
    if !import.packages.is_empty() {
        let to = dir.join("Imported");
        std::fs::create_dir_all(&to)?;
        for p in &import.packages {
            std::fs::copy(p, to.join(p.file_name().unwrap_or_default()))?;
        }
        original = ribbit_didj::nand_and_extras_missing(dir, true).map_err(anyhow::Error::msg)?.0;
    }
    let files = ribbit_didj::didj_import::merge(ours, &import);
    let (flash, _) = ribbit_didj::nand_with_files(dir, true, &files).map_err(anyhow::Error::msg)?;
    let changes = flash.changes_from(&original);
    write_save(&save, &format!("{:08x}", original.fingerprint()), &changes, Some(&ribbit_didj::carry::files_beyond(&flash, &original)))?;
    tracing::info!(target: "ribbit::saves", "Didj save folder {} imported: {} files, {} profiles, {} packages", from.display(), import.files.len(), import.profiles.len(), import.packages.len());
    Ok(DidjFolderImport { files: import.files.len(), profiles: import.profiles.len(), packages: import.packages.len(), backup: had.then_some(backup) })
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

/// Where a Leapster Explorer built from `dir` keeps its NAND's changes
/// (and its NOR flash beside, `.nor`).
pub fn explorer_save_file(dir: &Path, save_dir: &Path) -> PathBuf {
    let name = dir.file_name().map_or_else(|| "explorer".into(), |s| s.to_string_lossy().into_owned());
    save_dir.join(format!("explorer-{name}.nandblocks"))
}

/// Powers the console on again with the NAND (and the cartridge, and the
/// Explorer's NOR flash) it has.
fn power_on_again(d: &mut Didj, explorer: bool) {
    let (flash, cart, sdram, milli) = (std::mem::take(&mut d.bus.nand), d.bus.cart.take(), d.bus.sdram.len(), d.cpu_milli());
    if explorer {
        let nor = d.bus.nor.take();
        match ribbit_didj::explorer::boot(flash, ribbit_didj::explorer::CMDLINE) {
            Ok(lx) => *d = lx,
            // The NAND held a kernel when it started; this can't happen.
            Err(e) => tracing::error!(target: "ribbit::didj", "powering the Explorer on again: {e}"),
        }
        if nor.is_some() {
            d.bus.nor = nor;
        }
    } else {
        *d = Didj::with_sdram(flash, sdram);
        d.set_cpu_milli(milli);
    }
    d.bus.cart = cart;
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
    // AppManager's idle timer, looked at about once a second.
    let (mut idle, mut idle_frames) = (ribbit_didj::idle::IdleTimer::default(), 0u32);
    while !s.quit.load(Ordering::Relaxed) {
        if !s.running.load(Ordering::Relaxed) {
            if let Some(h) = s.hle.lock().unwrap().as_mut() {
                h.p.set_paused(true);
            }
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
        let picture = if let Some(h) = s.hle.lock().unwrap().as_mut() {
            h.frame(s, next, auto.as_ref().map_or(0, |_| auto_buttons), &mut rate, &mut resample, &mut stereo, auto.is_some())
        } else {
            let mut d = s.didj.lock().unwrap();
            d.bus.buttons = s.held.load(Ordering::Relaxed) | auto_buttons;
            if s.explorer {
                d.bus.touch = *s.touch.lock().unwrap();
            }
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
            d.bus.ga3d.dither = s.dither.load(Ordering::Relaxed);
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
            idle_frames += 1;
            if idle_frames as f64 >= FPS && !s.explorer && !d.bus.powered_off {
                idle_frames = 0;
                let ttb = d.cpu.cp15.ttb;
                idle.tick(&mut d.bus.sdram, ttb, s.keep_awake.load(Ordering::Relaxed));
            }
            // It shut down for idling, which the player asked for: it stays
            // off.
            if d.bus.powered_off && idle.near_shutdown() && !s.gave_up.load(Ordering::Relaxed) {
                tracing::info!(target: "ribbit::didj", "the Didj shut down for idling");
                s.serial.note("--- Ribbit: the Didj shut down for idling ---\n");
                // In this order: the app looks at idle_off first.
                s.idle_off.store(true, Ordering::Relaxed);
                s.gave_up.store(true, Ordering::Relaxed);
            }
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
            // Off cleanly (its file systems unmounted): the save written
            // now, so what it has (a profile LeapFrog Connect synced, which
            // ends with the cable pulled and the Didj shutting down) is kept
            // even if Ribbit is then closed in a way that can't save.
            if d.bus.powered_off
                && let Some(to) = &s.save_to
            {
                match to.write(&d.bus.nand) {
                    Ok(()) => tracing::info!(target: "ribbit::saves", "the Didj turned itself off: its NAND saved"),
                    Err(e) => tracing::warn!(target: "ribbit::saves", "saving the Didj's NAND: {e}"),
                }
            }
            if d.bus.powered_off && !s.gave_up.load(Ordering::Relaxed) {
                tracing::info!(target: "ribbit::didj", "the Didj turned itself off; turning it on again");
                s.serial.note("--- Ribbit: the Didj turned itself off; on again ---
");
                // Off the bus while it starts again: the PC sees the cable
                // pulled (its client detaches), not a device that stopped
                // answering ("USB device not recognized").
                unplug(s);
                power_on_again(&mut d, s.explorer);
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
            // The Explorer's codec: its volume (the volume buttons) and mutes.
            if d.bus.explorer {
                let (l, r) = d.bus.i2c.gain();
                for (k, v) in stereo.iter_mut().enumerate() {
                    *v = (*v as f32 * if k % 2 == 0 { l } else { r }).clamp(-32768.0, 32767.0) as i16;
                }
            }
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
    if s.hle.lock().unwrap().is_none() {
        power_off(s);
    }
}


/// The Didj's buttons (`button` bits) as the keypad's Linux key codes.
/// The Explorer's volume buttons (lf1000-keypad: KEY_VOLUMEUP,
/// KEY_VOLUMEDOWN, in its /proc/bus/input/devices bitmap).
const EXPLORER_KEYS: [(u32, u16); 2] = [(button::VOLUME_UP, 115), (button::VOLUME_DOWN, 114)];

const HLE_KEYS: [(u32, u16); 12] = {
    use ribbit_didj::hle::key;
    [
        (button::UP, key::UP),
        (button::DOWN, key::DOWN),
        (button::LEFT, key::LEFT),
        (button::RIGHT, key::RIGHT),
        (button::A, key::A),
        (button::B, key::B),
        (button::L, key::L),
        (button::R, key::R),
        (button::MENU, key::MENU),
        (button::HINT, key::HINT),
        (button::PAUSE, key::PAUSE),
        (button::BRIGHTNESS, key::BRIGHTNESS),
    ]
};

/// Enhanced (new): the Didj's programs (AppManager, the menu, the games) on
/// `ribbit_didj::hle`, Ribbit answering their Linux system calls. The
/// player's files on /Didj go to the same save as the other modes'.
struct Hle {
    p: ribbit_didj::hle::Process,
    /// AppManager's idle timer and when it was last looked at.
    idle: ribbit_didj::idle::IdleTimer,
    idle_at: Instant,
    dir: PathBuf,
    cart: Option<Vec<u8>>,
    mode: Mode,
    /// The paths the packages put on /Didj (lower case): the other files
    /// there are the player's.
    built: std::collections::HashSet<String>,
    /// The player's files as last written to the NAND (or read from it).
    written: Vec<(String, Vec<u8>)>,
    /// Buttons down on the keypad (`button` bits).
    keys: u32,
    /// In-between frames (`Mode::smooth`).
    smooth: bool,
    /// The game's frames seen, and when the last two came.
    flips: (u64, Instant, Instant),
    started: Instant,
    /// Stops in a row that came within a minute of starting.
    quick_stops: u32,
    /// Left off: it kept stopping soon after starting.
    off: bool,
    /// The file tree as built (system and cartridge), that save states
    /// keep the changes from: built at the first state.
    base: Option<ribbit_didj::hle::vfs::Vfs>,
    /// A Leapster Explorer (`new_explorer`): its root file system and Bulk
    /// go to the NAND (`explorer_to_nand`), their fingerprints as last written.
    explorer: bool,
    written_fp: [u32; 2],
}

impl Hle {
    fn new(dir: &Path, cart: Option<&[u8]>, files: Vec<(String, Vec<u8>)>, mode: Mode) -> anyhow::Result<Self> {
        use ribbit_didj::hle;
        let mut vfs = hle::system_from_packages(dir).map_err(anyhow::Error::msg)?;
        let built = vfs.files_under("/Didj").into_iter().map(|(p, _)| p.to_ascii_lowercase()).collect();
        for (name, data) in files {
            vfs.add_file(&format!("/Didj/{}", name.trim_start_matches('/')), data);
        }
        let mut p = hle::Process::new(vfs);
        // The boot splash for the boot loader's and kernel's time, as Original.
        match hle::boot_splash_from_packages(dir) {
            Ok(rgb) => p.boot_splash(&rgb),
            Err(e) => tracing::warn!(target: "ribbit::didj", "Enhanced (new): no boot splash: {e}"),
        }
        p.m.bus.ga3d.hd.set_scale(mode.hd as usize);
        if let Some(raw) = cart {
            p.insert_cart(raw).map_err(anyhow::Error::msg)?;
        }
        let envp = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/", "TERM=vt100"];
        p.exec("/bin/sh", &["/bin/sh", "-c", ". /etc/profile; exec AppManager"], &envp).map_err(anyhow::Error::msg)?;
        let now = Instant::now();
        let mut h = Self {
            p,
            dir: dir.to_path_buf(),
            cart: cart.map(<[u8]>::to_vec),
            mode,
            built,
            written: Vec::new(),
            keys: 0,
            smooth: mode.smooth,
            flips: (0, now, now),
            started: now,
            quick_stops: 0,
            idle: Default::default(),
            idle_at: Instant::now(),
            off: false,
            base: None,
            explorer: false,
            written_fp: [0; 2],
        };
        h.written = h.user_files();
        Ok(h)
    }

    /// Enhanced (new) for the Leapster Explorer: its file systems out of
    /// its NAND (`flash`, with the save applied) and NOR, started from
    /// /sbin/init as its kernel would (rcS, then AppManager).
    fn new_explorer(dir: &Path, flash: &Nand, nor: Option<&[u8]>, cart: Option<Vec<u8>>) -> anyhow::Result<Self> {
        use ribbit_didj::hle;
        let vfs = hle::explorer::system_from_nand(flash, nor).map_err(anyhow::Error::msg)?;
        let written_fp = hle::explorer::fingerprints(&vfs);
        let mut p = hle::Process::new_explorer(vfs);
        if let Some(rle) = hle::explorer::splash_from_packages(dir) {
            p.explorer_splash(&rle);
        }
        if let Some(raw) = &cart {
            p.explorer_cart(Some(raw)).map_err(anyhow::Error::msg)?;
        }
        let envp = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/", "TERM=vt100"];
        p.exec("/sbin/init", &["/sbin/init"], &envp).map_err(anyhow::Error::msg)?;
        let now = Instant::now();
        Ok(Self {
            p,
            dir: dir.to_path_buf(),
            cart,
            mode: Mode { enhanced: true, cpu_scale: 1, frame_skip: false, hd: 1, hle: true, smooth: false },
            built: Default::default(),
            written: Vec::new(),
            keys: 0,
            smooth: false,
            flips: (0, now, now),
            started: now,
            quick_stops: 0,
            idle: Default::default(),
            idle_at: now,
            off: false,
            base: None,
            explorer: true,
            written_fp,
        })
    }

    /// The Explorer's root file system and Bulk into the NAND `d` keeps for
    /// the save, and its NOR, when they changed.
    fn explorer_to_nand(&mut self, d: &mut Didj) -> anyhow::Result<()> {
        use ribbit_didj::hle::explorer;
        explorer::write_changed_to_nand(&self.p.vfs, &mut d.bus.nand, &mut self.written_fp).map_err(anyhow::Error::msg)?;
        let nor = explorer::nor(&self.p.vfs);
        if d.bus.nor.as_ref().is_none_or(|n| n.data != nor) {
            let mut n = ribbit_didj::nor::Nor::new(nor);
            n.dirty = true;
            d.bus.nor = Some(n);
        }
        Ok(())
    }

    /// The file tree as built, without the player's files (they go in the
    /// states as changes).
    fn base(&mut self) -> Result<&ribbit_didj::hle::vfs::Vfs, String> {
        if self.base.is_none() && self.explorer {
            self.base = Some(ribbit_didj::hle::explorer::system_from_packages(&self.dir)?);
        }
        if self.base.is_none() {
            let mut vfs = ribbit_didj::hle::system_from_packages(&self.dir)?;
            if let Some(raw) = &self.cart {
                ribbit_didj::hle::cart_files(&mut vfs, raw)?;
            }
            self.base = Some(vfs);
        }
        Ok(self.base.as_ref().unwrap())
    }

    /// The programs' clock stands still meanwhile, so the game doesn't
    /// jump on by the time a state takes.
    fn save_state(&mut self) -> Result<Vec<u8>, String> {
        let paused = self.p.is_paused();
        self.p.set_paused(true);
        let state = self.base().map(|_| ()).map(|_| self.p.save_state(self.base.as_ref().unwrap()));
        self.p.set_paused(paused);
        state
    }

    fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let paused = self.p.is_paused();
        self.p.set_paused(true);
        let loaded = self.base().map(|_| ()).and_then(|_| self.p.load_state(state, self.base.as_ref().unwrap()));
        self.p.set_paused(paused);
        loaded?;
        let now = Instant::now();
        self.flips = (self.p.m.bus.flips, now, now);
        Ok(())
    }

    /// The player's files on /Didj (paths from /Didj), sorted.
    fn user_files(&self) -> Vec<(String, Vec<u8>)> {
        let mut files: Vec<_> = self
            .p
            .vfs
            .files_under("/Didj")
            .into_iter()
            .filter(|(p, _)| !self.built.contains(&p.to_ascii_lowercase()))
            .map(|(p, d)| (p, d.to_vec()))
            .collect();
        files.sort();
        files
    }

    /// Runs the programs until `until` (host time); the picture (with the
    /// console-size one for the autostart) and the sound, as `run` takes
    /// them from the console.
    #[allow(clippy::too_many_arguments)]
    fn frame(&mut self, s: &Shared, until: Instant, auto: u32, rate: &mut u64, resample: &mut Resampler, stereo: &mut Vec<i16>, console: bool) -> Option<((usize, usize, Vec<u32>), Option<(usize, usize, Vec<u32>)>)> {
        use ribbit_didj::hle::Stop;
        self.p.set_paused(false);
        let held = s.held.load(Ordering::Relaxed) | auto;
        for &(bit, code) in HLE_KEYS.iter().chain(if self.explorer { &EXPLORER_KEYS[..] } else { &[] }) {
            if (held ^ self.keys) & bit != 0 {
                self.p.key(code, held & bit != 0);
            }
        }
        self.keys = held;
        // The Explorer's stylus (screen pixels).
        if self.explorer {
            let at = *s.touch.lock().unwrap();
            self.p.stylus(at.map(|(x, y)| (x.clamp(0.0, 319.0) as i32, y.clamp(0.0, 239.0) as i32)));
        }
        let hd = s.hd.load(Ordering::Relaxed) as usize;
        if self.p.m.bus.ga3d.hd.scale != hd.clamp(1, 4) {
            self.p.m.bus.ga3d.hd.set_scale(hd);
        }
        self.p.m.bus.ga3d.dither = s.dither.load(Ordering::Relaxed);
        if self.idle_at.elapsed() >= Duration::from_secs(1) {
            self.idle_at = Instant::now();
            self.p.idle_tick(&mut self.idle, s.keep_awake.load(Ordering::Relaxed));
        }
        if self.p.m.bus.ga3d.tween().is_some() != self.smooth {
            self.p.m.bus.ga3d.set_tween(self.smooth);
        }
        // The Explorer's programs mostly wait in short polls (2 ms), so a
        // run of a million instructions spans tens of ms of their paced
        // clock: smaller steps keep the frames on time.
        let step = if self.explorer { 100_000 } else { 1_000_000 };
        while !self.off && Instant::now() < until {
            match self.p.run(step) {
                Stop::Budget => {}
                // Every thread waits for something only the player can do.
                Stop::Blocked => std::thread::sleep(until.saturating_duration_since(Instant::now()).min(Duration::from_millis(2))),
                stop => {
                    self.stopped(s, stop);
                    break;
                }
            }
            if !self.p.console.is_empty() {
                s.serial.output(&std::mem::take(&mut self.p.console));
            }
        }
        let (mut sound, r, channels) = self.p.take_sound();
        // The Explorer's codec: its volume (the volume buttons) and mutes,
        // as Original applies them.
        if self.explorer {
            let (l, rg) = self.p.m.bus.i2c.gain();
            for (k, v) in sound.iter_mut().enumerate() {
                *v = (*v as f32 * if k % 2 == 0 { l } else { rg }).clamp(-32768.0, 32767.0) as i16;
            }
        }
        stereo.clear();
        if r > 0 {
            if r as u64 != *rate {
                *rate = r as u64;
                *resample = Resampler::new(r as f64);
            }
            match channels {
                1 => stereo.extend(sound.iter().flat_map(|&v| [v, v])),
                _ => stereo.extend(sound),
            }
        }
        let flips = self.p.m.bus.flips;
        if flips != self.flips.0 {
            self.flips = (flips, self.flips.2, Instant::now());
        }
        // In-between frames: the picture between the last two frames, as far
        // along as the time since the last one, so motion is even (a game
        // frame behind). Not across a pause of the game's drawing.
        let (_, t0, t1) = self.flips;
        let span = t1.saturating_duration_since(t0);
        let between = (self.smooth && span > Duration::ZERO && span < Duration::from_millis(150))
            .then(|| t1.elapsed().as_secs_f32() / span.as_secs_f32())
            .filter(|&f| f < 1.0)
            .and_then(|f| self.p.m.bus.frame_between(f));
        let picture = between.unwrap_or_else(|| self.p.m.bus.frame_hd());
        Some((picture, console.then(|| self.p.m.bus.frame())))
    }

    /// AppManager ended or a program crashed. As a Didj that turned itself
    /// off: on again, with the player's files, but not a third time in a
    /// row within a minute of starting (as `run` does).
    fn stopped(&mut self, s: &Shared, stop: ribbit_didj::hle::Stop) {
        tracing::warn!(target: "ribbit::didj", "Enhanced (new) stopped: {stop:?}");
        // It shut down for idling, which the player asked for: it stays off.
        if self.idle.near_shutdown() {
            s.serial.note("--- Ribbit: the Didj shut down for idling ---\n");
            s.idle_off.store(true, Ordering::Relaxed);
            self.off = true;
            return;
        }
        let quick_stops = if self.started.elapsed() < Duration::from_secs(60) { self.quick_stops + 1 } else { 0 };
        if quick_stops >= 3 {
            s.serial.note(&format!("--- Ribbit: Enhanced (new) stopped ({stop:?}); left off ---\n"));
            s.gave_up.store(true, Ordering::Relaxed);
            self.off = true;
            return;
        }
        s.serial.note(&format!("--- Ribbit: Enhanced (new) stopped ({stop:?}); on again ---\n"));
        if self.explorer {
            let mut d = s.didj.lock().unwrap();
            if let Err(e) = self.explorer_to_nand(&mut d) {
                tracing::warn!(target: "ribbit::saves", "keeping the Explorer's files: {e:#}");
            }
            match Self::new_explorer(&self.dir, &d.bus.nand, d.bus.nor.as_ref().map(|n| &n.data[..]), self.cart.clone()) {
                Ok(mut h) => {
                    h.quick_stops = quick_stops;
                    *self = h;
                }
                Err(e) => {
                    tracing::error!(target: "ribbit::didj", "starting Enhanced (new) again: {e:#}");
                    self.off = true;
                }
            }
            return;
        }
        match Self::new(&self.dir, self.cart.as_deref(), self.user_files(), self.mode) {
            Ok(mut h) => {
                h.smooth = self.smooth;
                h.written = std::mem::take(&mut self.written);
                h.quick_stops = quick_stops;
                *self = h;
            }
            Err(e) => {
                tracing::error!(target: "ribbit::didj", "starting Enhanced (new) again: {e:#}");
                self.off = true;
            }
        }
    }
}

/// Passes the PC's transfers (USB/IP) to the Didj's USB controller
/// (ribbit_didj::udc) and its answers back. The cable is in while a client
/// has the device imported, also across a power-on (a new console).
fn pump_usb(s: &Shared, d: &mut Didj) {
    use ribbit_didj::udc::Transfer;
    use ribbit_twist::usbip::Event;
    let Some(srv) = s.usb.lock().unwrap().clone() else { return };
    let srv = srv.lock().unwrap();
    srv.set_ready(d.bus.udc.ready());
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

/// Pulls the cable out if a PC has the console attached: its client is
/// disconnected, so the PC sees the device go (not one that stopped
/// answering while the console starts again: "USB device not recognized").
fn unplug(s: &Shared) {
    if s.usb_host.swap(false, Ordering::Relaxed)
        && let Some(srv) = s.usb.lock().unwrap().as_ref()
    {
        srv.lock().unwrap().disconnect();
        *s.usb_news.lock().unwrap() = Some(crate::twist::UsbNews::Detached);
    }
}

/// The Leapster Explorer as USB/IP lists it (its device descriptor,
/// VERIFIED with `EXPLORER=1 didjusb`: vendor 0x0F63, LeapFrog; product
/// 0x0020; release 3.25; high speed; one mass-storage interface, SCSI,
/// Bulk-Only: g_file_storage over /LF/fuse/dftp, /etc/init.d/dftp2).
pub fn explorer_usb_device() -> ribbit_twist::usbip::DeviceInfo {
    ribbit_twist::usbip::DeviceInfo { product: 0x0020, bcd_device: 0x0325, ..usb_device() }
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

/// Turns the Didj off as a player would (`Didj::shut_down`: the USB cable
/// pulled if it is in, then its power button): Linux then unmounts its
/// file systems before cutting the power. Stopping it without
/// this is pulling the batteries: a file being written is left half done,
/// the next start's fsck finds it, and the console refuses to mount /Didj
/// ("Download Failed"). Runs as fast as the host allows; gives up after
/// `POWER_OFF_LIMIT` (say, before the system has started).
fn power_off(s: &Shared) {
    let mut d = s.didj.lock().unwrap();
    let t = Instant::now();
    if d.bus.powered_off {
        return;
    }
    // With the cable in, it is pulled first (`Didj::shut_down`); the PC
    // sees it go.
    if s.usb_host.swap(false, Ordering::Relaxed)
        && let Some(srv) = s.usb.lock().unwrap().as_ref()
    {
        srv.lock().unwrap().disconnect();
    }
    let usb = d.bus.udc.attached;
    // Nothing to see meanwhile: the 3D engine's drawing is skipped.
    d.bus.skip_frames = true;
    // A game's shutdown takes 5 s and more of the console's time in
    // Original mode (the cable's, 10 s); a slow PC needs longer than that.
    let off = d.shut_down(&mut |d, cycles| {
        d.run(cycles);
        d.bus.dma.pcm.clear();
        t.elapsed() < POWER_OFF_LIMIT
    });
    // The shutdown's messages, for the serial console.
    if let Some(out) = d.bus.uart.first_mut().map(std::mem::take) {
        s.serial.output(&out.chars().map(|c| c as u8).collect::<Vec<u8>>());
    }
    if off {
        tracing::info!(target: "ribbit::saves", usb, "Didj powered off in {:.1} s", t.elapsed().as_secs_f32());
    } else {
        tracing::warn!(target: "ribbit::saves", usb, "the Didj did not power off in {} s; saving its NAND as it is", POWER_OFF_LIMIT.as_secs());
    }
}

/// How long closing waits for the console to power off (`power_off`).
const POWER_OFF_LIMIT: Duration = Duration::from_secs(40);

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
    let entries = ribbit_core::zip::entries(&data).ok()?;
    let meta_entry = entries.iter().find(|e| e.name.to_ascii_lowercase().ends_with("meta.inf"))?;
    let meta = ribbit_didj::package::Meta::parse(&String::from_utf8_lossy(&ribbit_core::zip::read_entry(&data, meta_entry).ok()?));
    let dir = meta_entry.name[..meta_entry.name.len() - "meta.inf".len()].to_string();
    if !meta.is_didj() {
        return None;
    }
    let product = meta.get("ProductID").unwrap_or("");
    let (title, tag) = match meta.get("Type").map(str::trim) {
        // The firmware package carries the kernel and root file system.
        Some("System") if product.eq_ignore_ascii_case("0x000E0003") => ("Didj".to_string(), "System"),
        Some("Application") => (ribbit_didj::connect::display_name(&meta, &id), "Game"),
        _ => return None,
    };
    let mut details = vec![("System", "Didj".to_string())];
    for (label, key) in [("Name", "Name"), ("Version", "Version"), ("Product", "ProductID")] {
        if let Some(v) = meta.get(key) {
            details.push((label, v.to_string()));
        }
    }
    // Its folder on the console (ProgramFiles/<folder>): matched with an
    // installed copy, so a game is one card (`library::merge_installed`).
    details.push(("Folder", dir.trim_end_matches('/').to_string()));
    let status = if tag == "Game" {
        "early emulation: Ribbit starts it from the Didj's menu (a console set up with a profile), built from the packages in this folder"
    } else {
        "early emulation: starts at the Didj's menu, built from the packages in this folder"
    };
    details.push(("Status", status.to_string()));
    // A game beside the Didj's own packages is built into it; one elsewhere
    // is installed on it when played (`didj_library`).
    let shown = match tag {
        "Game" if path.parent().and_then(|d| find_package(d, 0)).is_some() => "Built-in",
        "Game" => {
            details.push(("Install", "playing it installs it on the Didj first, as LeapFrog Connect would".to_string()));
            "Not installed"
        }
        t => t,
    };
    details.push(("File", path.display().to_string()));
    let mb = (data.len() as u64).div_ceil(1 << 20);
    Some(Game {
        path: path.to_path_buf(),
        family: Family::Didj,
        title,
        subtitle: format!("{}  ·  {mb} MB", meta.get("Version").unwrap_or("")),
        details,
        tags: vec![shown.into()],
        key: format!("didj-{id}"),
        art: if tag == "Game" {
            card_art(&meta, |f| ribbit_core::zip::read_entry(&data, entries.iter().find(|e| e.name == format!("{dir}{f}"))?).ok())
        } else {
            // The console: its icon (`console_icons`).
            Art::Missing
        },
    })
}

/// Whether `path` may be the Leapster Explorer's Firmware Base package, by
/// its name (LeapFrog Connect's "Firmware Base.lfp", or its package ID
/// LST3-0x00170029-...), before it is read.
fn may_be_explorer_firmware(path: &Path) -> bool {
    path.file_name().is_some_and(|n| {
        let n = n.to_string_lossy().to_ascii_lowercase();
        n.ends_with(".lfp") && (n.starts_with("lst3-") || n.contains("firmware base") || n.contains("firmware-base"))
    })
}

/// The Leapster Explorer's Firmware Base package (its meta.inf: Device
/// "LeapsterExplorer", Name "Firmware-Base") stands for the console in the
/// library; the other packages are found beside it when it starts.
pub fn describe_explorer(path: &Path) -> Option<crate::library::Game> {
    use crate::library::{Art, Family, Game};
    if !may_be_explorer_firmware(path) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let meta = ribbit_core::zip::entries(&data)
        .ok()?
        .into_iter()
        .find(|e| e.name.to_ascii_lowercase().ends_with("meta.inf"))
        .and_then(|e| ribbit_core::zip::read_entry(&data, &e).ok())
        .map(|m| ribbit_didj::package::Meta::parse(&String::from_utf8_lossy(&m)))?;
    if meta.get("Device") != Some("LeapsterExplorer") || meta.get("Name") != Some("Firmware-Base") {
        return None;
    }
    let mut details = vec![("System", "Leapster Explorer".to_string())];
    for (label, key) in [("Version", "Version"), ("Locale", "Locale")] {
        if let Some(v) = meta.get(key) {
            details.push((label, v.to_string()));
        }
    }
    details.push(("Status", "early emulation: starts Linux and the Explorer's own setup and menu, built from the packages in this folder; Didj cartridges play in its slot".to_string()));
    details.push(("File", path.display().to_string()));
    let mb = (data.len() as u64).div_ceil(1 << 20);
    Some(Game {
        path: path.to_path_buf(),
        family: Family::Explorer,
        title: "Leapster Explorer".to_string(),
        subtitle: format!("{}  ·  {mb} MB", meta.get("Version").unwrap_or("")),
        details,
        tags: vec!["System".into()],
        key: "explorer-firmware".to_string(),
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

/// `menu_art` for a game whose files `read` gives (one installed on the
/// console: `didj_library`).
pub(crate) fn menu_art_from(meta: &ribbit_didj::package::Meta, read: impl Fn(&str) -> Option<Vec<u8>>) -> Option<Art> {
    let image = panel_art(meta, read)?;
    let pixels = image.pixels.iter().map(|c| u32::from_be_bytes([c.a(), c.r(), c.g(), c.b()])).collect();
    Some(Art { width: image.size[0], pixels })
}

/// The picture a game's panel shows (`menu_art`), read with `read`.
fn panel_art(meta: &ribbit_didj::package::Meta, read: impl Fn(&str) -> Option<Vec<u8>>) -> Option<egui::ColorImage> {
    let mut found = ["LargeIcon", "PreviewImage"].into_iter().filter_map(|k| crate::manual::decode_png(&read(meta.get(k)?)?));
    let first = found.next()?;
    if first.size[0] >= 256 { Some(first) } else { found.next().or(Some(first)) }
}

/// A game's library picture: what its panel shows (Tinker Bell and NeoPets
/// have only a `LargeIcon`), else its `Icon`.
pub(crate) fn card_art(meta: &ribbit_didj::package::Meta, read: impl Fn(&str) -> Option<Vec<u8>>) -> crate::library::Art {
    panel_art(meta, &read)
        .or_else(|| crate::manual::decode_png(&read(meta.get("Icon")?)?))
        .map_or(crate::library::Art::Missing, |art| crate::library::Art::Image(card_picture(&art)))
}

/// What a square library card shows of a Didj picture. The picture fills only
/// part of its texture (a 256x128 one, with empty space right and below):
/// that part, whole and in its own shape, over a blurred dark copy of itself
/// so the card has no empty space.
fn card_picture(art: &egui::ColorImage) -> egui::ColorImage {
    const SIZE: usize = 512;
    const BLUR: usize = 16;
    let [w, h] = art.size;
    let solid = |x: usize, y: usize| art.pixels[y * w + x].a() >= 128;
    let columns: Vec<usize> = (0..w).filter(|&x| (0..h).any(|y| solid(x, y))).collect();
    let rows: Vec<usize> = (0..h).filter(|&y| (0..w).any(|x| solid(x, y))).collect();
    let (Some(&left), Some(&right), Some(&top), Some(&bottom)) = (columns.first(), columns.last(), rows.first(), rows.last()) else {
        return art.clone();
    };
    let (cw, ch) = ((right - left + 1) as f32, (bottom - top + 1) as f32);
    // Premultiplied red, green, blue and alpha at (x, y) in the picture.
    let at = |x: f32, y: f32| -> [f32; 4] {
        let (x, y) = ((x - 0.5).clamp(0.0, cw - 1.0) + left as f32, (y - 0.5).clamp(0.0, ch - 1.0) + top as f32);
        let (x0, y0) = (x as usize, y as usize);
        let (x1, y1) = ((x0 + 1).min(right), (y0 + 1).min(bottom));
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let px = |x: usize, y: usize| art.pixels[y * w + x].to_array().map(f32::from);
        let (a, b, c, d) = (px(x0, y0), px(x1, y0), px(x0, y1), px(x1, y1));
        std::array::from_fn(|i| (a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy)
    };
    // The background: the picture cut to a square, in few pixels and darker.
    let side = cw.min(ch);
    let small: Vec<[f32; 4]> = (0..BLUR * BLUR)
        .map(|i| {
            let (u, v) = (((i % BLUR) as f32 + 0.5) / BLUR as f32, ((i / BLUR) as f32 + 0.5) / BLUR as f32);
            at((cw - side) / 2.0 + u * side, (ch - side) / 2.0 + v * side)
        })
        .collect();
    let back = |x: usize, y: usize| -> [f32; 4] {
        let grid = |p: usize| ((p as f32 + 0.5) / SIZE as f32 * BLUR as f32 - 0.5).clamp(0.0, (BLUR - 1) as f32);
        let (gx, gy) = (grid(x), grid(y));
        let (x0, y0) = (gx as usize, gy as usize);
        let (x1, y1) = ((x0 + 1).min(BLUR - 1), (y0 + 1).min(BLUR - 1));
        let (fx, fy) = (gx - x0 as f32, gy - y0 as f32);
        let (a, b, c, d) = (small[y0 * BLUR + x0], small[y0 * BLUR + x1], small[y1 * BLUR + x0], small[y1 * BLUR + x1]);
        std::array::from_fn(|i| ((a[i] * (1.0 - fx) + b[i] * fx) * (1.0 - fy) + (c[i] * (1.0 - fx) + d[i] * fx) * fy) * 0.4)
    };
    let scale = SIZE as f32 / cw.max(ch);
    let (fw, fh) = (cw * scale, ch * scale);
    let (ox, oy) = ((SIZE as f32 - fw) / 2.0, (SIZE as f32 - fh) / 2.0);
    let mut rgba = Vec::with_capacity(SIZE * SIZE * 4);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let (px, py) = (x as f32 + 0.5 - ox, y as f32 + 0.5 - oy);
            let under = back(x, y);
            let over = if (0.0..fw).contains(&px) && (0.0..fh).contains(&py) { at(px / scale, py / scale) } else { [0.0; 4] };
            let keep = 1.0 - over[3] / 255.0;
            rgba.extend((0..3).map(|i| (over[i] + under[i] * keep) as u8));
            rgba.push(255);
        }
    }
    egui::ColorImage::from_rgba_unmultiplied([SIZE, SIZE], &rgba)
}

/// A Didj cartridge dump in the library, titled from the meta.inf of the
/// game on it, its preview image as the art.
pub fn describe_cart(path: &Path, data: &[u8]) -> Option<crate::library::Game> {
    use crate::library::{Family, Game};
    use ribbit_core::fatfs;
    // A raw dump, or an image of its FAT volume alone (an Explorer
    // cartridge's dump is one).
    let volume = if ribbit_didj::cart::is_volume(data) || ribbit_didj::cart::is_explorer_image(data) { std::borrow::Cow::Borrowed(data) } else { std::borrow::Cow::Owned(ribbit_didj::cart::volume(data)?) };
    let (folder, meta) = fatfs::list(&volume, "")?.into_iter().filter(|e| e.is_dir).find_map(|e| {
        let m = fatfs::read_file(&volume, &format!("{}/meta.inf", e.name))?;
        Some((e.name, ribbit_didj::package::Meta::parse(&String::from_utf8_lossy(&m))))
    })?;
    let stem = path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let title = match meta.get("Name") {
        // Foster's meta.inf has only its internal code name.
        Some("MAC") => "Foster's Home for Imaginary Friends".to_string(),
        name => name.map_or(stem, str::to_string),
    };
    let art = card_art(&meta, |f| fatfs::read_file(&volume, &format!("{folder}/{f}")));
    // An Explorer cartridge (Device="LeapsterExplorer", PackageID LST3-...)
    // is the same FAT volume; it plays in the Explorer's slot alone. A
    // LeapPad's (Device="LeapPadExplorer") fits the slot too.
    let leappad = meta.get("Device") == Some("LeapPadExplorer");
    let explorer = leappad || meta.get("Device") == Some("LeapsterExplorer");
    let system = if leappad {
        "Leapster Explorer (a LeapPad cartridge)"
    } else if explorer {
        "Leapster Explorer"
    } else {
        "Didj"
    };
    let mut details = vec![("System", system.to_string())];
    for (label, key) in [("Name", "Name"), ("Version", "Version"), ("Product", "ProductID"), ("Publisher", "Publisher"), ("Developer", "Developer")] {
        if let Some(v) = meta.get(key) {
            details.push((label, v.to_string()));
        }
    }
    details.push((
        "Status",
        if leappad {
            "early emulation: in the Explorer's slot, its menu shows the game; most LeapPad games then ask to be played on a LeapPad"
        } else if explorer {
            "early emulation: in the Explorer's slot, its menu shows the game; take it out from the Emulation menu"
        } else {
            "early emulation: Ribbit starts it from the Didj's menu (a console set up with a profile)"
        }
        .to_string(),
    ));
    details.push(("File", path.display().to_string()));
    let id = meta.get("ProductID").unwrap_or("").to_string();
    Some(Game {
        path: path.to_path_buf(),
        family: if explorer { Family::Explorer } else { Family::Didj },
        title,
        subtitle: meta.get("Version").map_or(String::new(), |v| format!("version {v}")),
        details,
        tags: vec![if explorer { EXPLORER_OWN_CART_TAG } else { "Cartridge" }.into()],
        key: format!("{}-cart-{id}", if explorer { "explorer" } else { "didj" }),
        art,
    })
}

/// A Didj cartridge's entry on the Explorer's page: the Explorer plays
/// them too (its firmware lists the Didj titles and patches some).
pub fn on_explorer(didj: &crate::library::Game) -> crate::library::Game {
    use crate::library::{Art, Family, Game};
    let mut details = didj.details.clone();
    for d in &mut details {
        match d.0 {
            "System" => d.1 = "Leapster Explorer (a Didj cartridge)".to_string(),
            "Status" => d.1 = "early emulation: in the Explorer's slot, its menu shows the game; take it out from the Emulation menu".to_string(),
            _ => {}
        }
    }
    Game {
        path: didj.path.clone(),
        family: Family::Explorer,
        title: didj.title.clone(),
        subtitle: didj.subtitle.clone(),
        details,
        tags: vec![EXPLORER_CART_TAG.into()],
        key: format!("explorer-{}", didj.key),
        art: match &didj.art {
            Art::Image(i) => Art::Image(i.clone()),
            _ => Art::Missing,
        },
    }
}

/// The tag of `on_explorer` entries.
pub const EXPLORER_CART_TAG: &str = "Didj cartridge";

/// The tag of an Explorer cartridge's own entry (`describe_cart`).
pub const EXPLORER_OWN_CART_TAG: &str = "Cartridge";

/// The dump at `path` if it is a Didj cartridge (zipped or not): the raw
/// flash, or an image of its FAT volume alone, made into the raw flash.
pub fn read_cart(path: &Path) -> Option<Vec<u8>> {
    use ribbit_didj::cart;
    let raw = path.extension().is_none_or(|e| !e.eq_ignore_ascii_case("zip"));
    let len = std::fs::metadata(path).ok()?.len() as usize;
    if raw && len % (cart::PAGE_RAW * cart::PAGES_PER_BLOCK) != 0 && len % 15360 != 0 && len % (cart::PAGE * cart::PAGES_PER_BLOCK) != 0 {
        return None;
    }
    let data = ribbit_core::read_image(path).ok()?;
    if cart::Cart::looks_like(&data) {
        return Some(data);
    }
    // A Leapster Explorer cartridge: FAT from page 0, no UBI.
    if cart::is_explorer_image(&data) {
        return Some(cart::from_explorer_image(&data));
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

/// The Leapster Explorer's Firmware Base package in `dir`, by its name.
pub fn find_explorer_firmware(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| may_be_explorer_firmware(p))
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

#[cfg(test)]
#[path = "didj_library_tests.rs"]
mod library_tests;

#[cfg(test)]
mod tests {
    use super::*;

    /// The process's working set and private bytes in MB (Windows; zeros
    /// elsewhere), for the Explorer's memory checks.
    fn memory_mb() -> (f64, f64) {
        #[cfg(windows)]
        {
            #[repr(C)]
            #[derive(Default)]
            struct Counters {
                cb: u32,
                faults: u32,
                peak_ws: usize,
                ws: usize,
                quota: [usize; 4],
                pagefile: usize,
                peak_pagefile: usize,
                private: usize,
            }
            unsafe extern "system" {
                fn GetCurrentProcess() -> isize;
                fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
            }
            let mut c = Counters { cb: std::mem::size_of::<Counters>() as u32, ..Default::default() };
            // SAFETY: a plain query into a struct of the size given.
            unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
            (c.ws as f64 / 1048576.0, c.private as f64 / 1048576.0)
        }
        #[cfg(not(windows))]
        (0.0, 0.0)
    }

    /// Enhanced (new) in the app's session: the menu draws, and a file the
    /// programs write to /Didj reaches the save, where the other modes find
    /// it (carry::user_files). RIBBIT_DIDJ_DIR: a folder of Didj packages;
    /// RIBBIT_DIDJ_SHOT=<file.png> saves the picture.
    /// Every Leapster Explorer cartridge dump (zipped or not) in
    /// RIBBIT_EXPLORER_CARTS is an Explorer game that goes into its slot,
    /// titled from its meta.inf, and reads as a cartridge with the Emerald type.
    #[test]
    #[ignore]
    fn explorer_cartridges_are_explorer_games() {
        let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_CARTS").expect("RIBBIT_EXPLORER_CARTS"));
        let mut n = 0;
        for e in std::fs::read_dir(&dir).unwrap() {
            let path = e.unwrap().path();
            let raw = read_cart(&path).unwrap_or_else(|| panic!("{}: not read as a cartridge", path.display()));
            let data = ribbit_core::read_image(&path).unwrap();
            let g = describe_cart(&path, &data).unwrap_or_else(|| panic!("{}: no meta.inf", path.display()));
            assert!(g.family == crate::library::Family::Explorer && g.plays_in_explorer_slot() && !g.is_explorer_cart(), "{}", path.display());
            assert!(!g.title.is_empty());
            assert_eq!(ribbit_didj::cart::Cart::new(raw).unwrap().cart_type(), ribbit_didj::cart::EMERALD);
            n += 1;
        }
        assert!(n > 0);
        println!("{n} Explorer cartridges");
    }

    /// "Turn off when idle" on all three modes, in real time (9 minutes):
    /// off (the default), the Didj is still on past its 7 minutes; on, it
    /// shuts down, stays off and the app is told. RIBBIT_DIDJ_DIR as above.
    #[test]
    #[ignore]
    fn didj_idle() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let modes = [("original", false, false), ("enhanced", true, false), ("new", true, true)];
        let runs: Vec<_> = modes
            .into_iter()
            .flat_map(|m| [(m, false), (m, true)])
            .map(|((name, enhanced, hle), turn_off)| {
                let dir = dir.clone();
                std::thread::spawn(move || {
                    let saves = std::env::temp_dir().join(format!("ribbit-idle-{name}-{turn_off}"));
                    let _ = std::fs::remove_dir_all(&saves);
                    let mode = Mode { enhanced, cpu_scale: 1, frame_skip: false, hd: 1, hle, smooth: false };
                    let didj = DidjSession::start(&dir, &saves, None, None, None, mode, Arc::new(crate::serial::Serial::default())).unwrap();
                    didj.set_turn_off_when_idle(turn_off);
                    let t = Instant::now();
                    let mut off_at = None;
                    while t.elapsed() < Duration::from_secs(540) && off_at.is_none() {
                        std::thread::sleep(Duration::from_millis(200));
                        if didj.take_idle_off() {
                            off_at = Some(t.elapsed().as_secs());
                        }
                    }
                    // Still off a while later (not started again), or still on.
                    std::thread::sleep(Duration::from_secs(10));
                    let powered_off = didj.shared.didj.lock().unwrap().bus.powered_off;
                    let hle_off = didj.shared.hle.lock().unwrap().as_ref().map(|h| h.off);
                    let gave_up = didj.shared.gave_up.load(Ordering::Relaxed);
                    eprintln!("{name} turn_off={turn_off}: idle off at {off_at:?} s, powered_off {powered_off}, hle off {hle_off:?}, gave_up {gave_up}, speed {:.0}%", didj.speed_percent());
                    didj.stop();
                    let _ = std::fs::remove_dir_all(&saves);
                    (name, turn_off, off_at, hle_off.unwrap_or(powered_off))
                })
            })
            .collect();
        for r in runs {
            let (name, turn_off, off_at, off) = r.join().unwrap();
            assert_eq!(off_at.is_some(), turn_off, "{name} turn_off={turn_off}: idle off at {off_at:?}");
            assert_eq!(off, turn_off, "{name} turn_off={turn_off}: off {off}");
        }
    }

    #[test]
    fn a_card_shows_the_whole_picture_without_stretching_it() {
        // A red picture 180x109 in the corner of a 256x128 texture.
        let mut rgba = vec![0u8; 256 * 128 * 4];
        for y in 0..109 {
            for x in 0..180 {
                rgba[(y * 256 + x) * 4..][..4].copy_from_slice(&[255, 0, 0, 255]);
            }
        }
        let card = card_picture(&egui::ColorImage::from_rgba_unmultiplied([256, 128], &rgba));
        assert_eq!(card.size, [512, 512]);
        let red = |x: usize, y: usize| card.pixels[y * 512 + x].r();
        // Full width, centred; 512 * 109 / 180 = 310 high, so 101 above and below.
        assert_eq!((red(0, 256), red(511, 256)), (255, 255));
        assert_eq!((red(256, 103), red(256, 408)), (255, 255));
        // The background is the same picture, darker, not empty.
        assert_eq!(red(256, 90), (255.0 * 0.4) as u8);
        assert!(card.pixels.iter().all(|c| c.a() == 255));
    }

    #[test]
    #[ignore]
    fn didj_hle_session() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let saves = std::env::temp_dir().join("ribbit-hle-test");
        let _ = std::fs::remove_dir_all(&saves);
        let mode = Mode { enhanced: true, cpu_scale: 1, frame_skip: false, hd: 1, hle: true, smooth: true };
        let didj = DidjSession::start(&dir, &saves, None, None, None, mode, Arc::new(crate::serial::Serial::default())).unwrap();
        assert!(didj.is_hle());
        std::thread::sleep(Duration::from_secs(10));
        let mut px = Vec::new();
        let (w, h) = didj.frame(&mut px);
        let lit = px.iter().filter(|&&p| p != 0).count();
        eprintln!("picture {w}x{h}, {lit} pixels not black, speed {:.0}%", didj.speed_percent());
        if let Ok(path) = std::env::var("RIBBIT_DIDJ_SHOT") {
            let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
        assert!(lit > w * h / 4);
        didj.shared.hle.lock().unwrap().as_mut().unwrap().p.vfs.add_file("/Didj/Data/ribbit-test.txt", b"hello".to_vec());
        eprintln!("player's files: {:?}", didj.shared.hle.lock().unwrap().as_ref().unwrap().user_files().iter().map(|f| f.0.clone()).collect::<Vec<_>>());
        let original = didj.original.clone();
        let save = didj.save_path.clone();
        let t = Instant::now();
        didj.stop();
        eprintln!("stopped and saved in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        let bytes = std::fs::read(&save).unwrap();
        eprintln!("save {} bytes", bytes.len());
        let mut flash = (*original).clone();
        flash.apply_changes(&bytes).unwrap();
        let files = ribbit_didj::carry::files_beyond(&flash, &original);
        eprintln!("save holds {:?}", files.iter().map(|f| &f.0).collect::<Vec<_>>());
        assert!(files.iter().any(|(p, d)| p.to_ascii_lowercase().ends_with("data/ribbit-test.txt") && d == b"hello"));
    }

    /// Enhanced (new) save states: one made at the menu, the menu run on
    /// with a file added, the state loaded (the file gone again) and the
    /// programs going on. RIBBIT_DIDJ_DIR as above; RIBBIT_DIDJ_CART a
    /// cartridge dump, RIBBIT_DIDJ_SAVE a set-up save, RIBBIT_DIDJ_KEYS buttons to
    /// press (below) and RIBBIT_DIDJ_WAIT the seconds before the state (10).
    #[test]
    #[ignore]
    fn didj_hle_states() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let saves = std::env::temp_dir().join("ribbit-hle-states");
        let _ = std::fs::remove_dir_all(&saves);
        let mode = Mode { enhanced: true, cpu_scale: 1, frame_skip: false, hd: 1, hle: true, smooth: true };
        let cart = std::env::var("RIBBIT_DIDJ_CART").ok().map(|p| ribbit_core::read_image(Path::new(&p)).unwrap());
        let wait = std::env::var("RIBBIT_DIDJ_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(10);
        // RIBBIT_DIDJ_SAVE: a set-up console's .nandblocks (copied), past the first-run questions.
        if let Ok(save) = std::env::var("RIBBIT_DIDJ_SAVE") {
            let save = PathBuf::from(save);
            std::fs::create_dir_all(&saves).unwrap();
            std::fs::copy(&save, save_file(&dir, &saves)).unwrap();
            std::fs::copy(save.with_extension("nandimage"), save_file(&dir, &saves).with_extension("nandimage")).unwrap();
        }
        let didj = DidjSession::start(&dir, &saves, cart, None, None, mode, Arc::new(crate::serial::Serial::default())).unwrap();
        let flips = || didj.shared.hle.lock().unwrap().as_ref().unwrap().p.m.bus.flips;
        let lit = || {
            let mut px = Vec::new();
            let (w, h) = didj.frame(&mut px);
            (px.iter().filter(|&&p| p != 0).count(), w * h)
        };
        // Pictures at saving and just after loading, to RIBBIT_DIDJ_SHOT_DIR.
        let shot = |name: &str| {
            let Ok(shots) = std::env::var("RIBBIT_DIDJ_SHOT_DIR") else { return };
            let mut px = Vec::new();
            let (w, h) = didj.frame(&mut px);
            let mut enc = png::Encoder::new(std::fs::File::create(Path::new(&shots).join(name)).unwrap(), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        };
        // RIBBIT_DIDJ_KEYS: <seconds from the start>:<button> taps, comma separated
        // (a, b, up, down, left, right); a picture before each.
        let started = Instant::now();
        for tap in std::env::var("RIBBIT_DIDJ_KEYS").unwrap_or_default().split(',').filter(|t| !t.is_empty()) {
            let (at, key) = tap.split_once(':').expect("<seconds>:<button>");
            let b = match key {
                "a" => Button::A,
                "b" => Button::B,
                "up" => Button::Up,
                "down" => Button::Down,
                "left" => Button::Left,
                "right" => Button::Right,
                _ => panic!("button {key}"),
            };
            std::thread::sleep(Duration::from_secs(at.parse().unwrap()).saturating_sub(started.elapsed()));
            shot(&format!("state-key-{at}.png"));
            didj.set_button(b, true);
            std::thread::sleep(Duration::from_millis(150));
            didj.set_button(b, false);
        }
        std::thread::sleep(Duration::from_secs(wait).saturating_sub(started.elapsed()));
        let t = Instant::now();
        let state = didj.save_state().expect("a state");
        shot("state-saved.png");
        eprintln!("state {} bytes in {:.0} ms", state.len(), t.elapsed().as_secs_f64() * 1000.0);
        didj.shared.hle.lock().unwrap().as_mut().unwrap().p.vfs.add_file("/Didj/Data/ribbit-state.txt", b"later".to_vec());
        std::thread::sleep(Duration::from_secs(3));
        let t = Instant::now();
        didj.load_state(&state).unwrap();
        eprintln!("loaded in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        std::thread::sleep(Duration::from_millis(100));
        shot("state-loaded.png");
        assert!(didj.shared.hle.lock().unwrap().as_ref().unwrap().p.vfs.get("/Didj/Data/ribbit-state.txt").is_none());
        let before = flips();
        std::thread::sleep(Duration::from_secs(5));
        let (on, all) = lit();
        let hle = didj.shared.hle.lock().unwrap();
        let h = hle.as_ref().unwrap();
        eprintln!("after loading: {} flips in 5 s, {on}/{all} pixels lit, exit {:?}, speed {:.0}%", h.p.m.bus.flips - before, h.p.exit, didj.speed_percent());
        assert!(h.p.m.bus.flips > before + 20 && h.p.exit.is_none() && on > all / 4);
        drop(hle);
        // A second state from the loaded one loads too.
        let again = didj.save_state().expect("a state");
        didj.load_state(&again).unwrap();
        didj.stop();
    }

    /// Enhanced (new) starting a library game from the menu (the autostart):
    /// RIBBIT_DIDJ_DIR as above, RIBBIT_DIDJ_SAVE a set-up console's
    /// .nandblocks (copied, not changed), RIBBIT_DIDJ_GAME the game's
    /// package; pictures every 5 s to RIBBIT_DIDJ_SHOT_DIR.
    #[test]
    #[ignore]
    fn didj_hle_autostart() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let game = PathBuf::from(std::env::var("RIBBIT_DIDJ_GAME").expect("RIBBIT_DIDJ_GAME"));
        let shots = PathBuf::from(std::env::var("RIBBIT_DIDJ_SHOT_DIR").expect("RIBBIT_DIDJ_SHOT_DIR"));
        let saves = std::env::temp_dir().join("ribbit-hle-autostart");
        let _ = std::fs::remove_dir_all(&saves);
        std::fs::create_dir_all(&saves).unwrap();
        let save = PathBuf::from(std::env::var("RIBBIT_DIDJ_SAVE").expect("RIBBIT_DIDJ_SAVE"));
        std::fs::copy(&save, save_file(&dir, &saves)).unwrap();
        std::fs::copy(save.with_extension("nandimage"), save_file(&dir, &saves).with_extension("nandimage")).unwrap();
        let art = menu_art(&game, None).expect("the game's menu art");
        let mode = Mode { enhanced: true, cpu_scale: 1, frame_skip: false, hd: 1, hle: true, smooth: true };
        let didj = DidjSession::start(&dir, &saves, None, None, Some(art), mode, Arc::new(crate::serial::Serial::default())).unwrap();
        let mut px = Vec::new();
        for k in 1..=12 {
            std::thread::sleep(Duration::from_secs(5));
            let (w, h) = didj.frame(&mut px);
            let mut enc = png::Encoder::new(std::fs::File::create(shots.join(format!("auto-{:02}.png", k * 5))).unwrap(), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
        didj.stop();
    }

    /// Leapster Explorer Enhanced (new) in a session as the app runs it:
    /// starts from the packages (RIBBIT_EXPLORER_DIR), shows its menu, keeps
    /// a file put on Bulk and one on the root file system in the usual save
    /// (.nandblocks), and reads them back from it.
    #[test]
    #[ignore]
    fn explorer_hle_session() {
        let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_DIR").expect("RIBBIT_EXPLORER_DIR"));
        let saves = std::env::temp_dir().join("ribbit-explorer-hle-test");
        let _ = std::fs::remove_dir_all(&saves);
        let built = DidjSession::build_explorer(&dir, &saves).unwrap();
        let lx = DidjSession::start_explorer(built, None, Arc::new(crate::serial::Serial::default()), true).unwrap();
        assert!(lx.is_hle());
        std::thread::sleep(Duration::from_secs(12));
        let mut px = Vec::new();
        let (w, h) = lx.frame(&mut px);
        let lit = px.iter().filter(|&&p| p != 0).count();
        let (ws, private) = memory_mb();
        eprintln!("picture {w}x{h}, {lit} pixels not black, speed {:.0}%, memory {ws:.0} MB working set, {private:.0} MB private", lx.speed_percent());
        if let Ok(path) = std::env::var("RIBBIT_EXPLORER_SHOT") {
            let mut enc = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        }
        assert!(lit > w * h / 4);
        {
            let mut hle = lx.shared.hle.lock().unwrap();
            let vfs = &mut hle.as_mut().unwrap().p.vfs;
            vfs.add_file("/LF/Bulk/Data/ribbit-test.txt", b"hello".to_vec());
            vfs.add_file("/flags/ribbit-test", b"flag".to_vec());
        }
        let (original, save) = (lx.original.clone(), lx.save_path.clone());
        let t = Instant::now();
        lx.stop();
        eprintln!("stopped and saved in {:.0} ms, the save {} bytes", t.elapsed().as_secs_f64() * 1000.0, std::fs::metadata(&save).map_or(0, |m| m.len()));
        let mut flash = (*original).clone();
        flash.apply_changes(&std::fs::read(&save).unwrap()).unwrap();
        let (root, bulk) = ribbit_didj::hle::explorer::files_in_nand(&flash).unwrap();
        assert!(bulk.iter().any(|f| f.path == "/Data/ribbit-test.txt" && f.data == b"hello"));
        assert!(root.iter().any(|f| f.path == "/flags/ribbit-test" && f.data == b"flag"));
        // Started again from that save, the files are there.
        let built = DidjSession::build_explorer(&dir, &saves).unwrap();
        let lx = DidjSession::start_explorer(built, None, Arc::new(crate::serial::Serial::default()), true).unwrap();
        let has = lx.shared.hle.lock().unwrap().as_ref().unwrap().p.vfs.get("/LF/Bulk/Data/ribbit-test.txt").is_some();
        lx.stop();
        assert!(has);
    }

    /// Original (the Explorer's kernel) started from `flash`: `commands`
    /// typed at its serial console's shell, one per prompt, then the
    /// console's output and the NAND as the kernel left it.
    fn explorer_console(flash: Nand, commands: &[&str]) -> (String, Nand) {
        let mut lx = ribbit_didj::explorer::boot(flash, ribbit_didj::explorer::CMDLINE).unwrap();
        let mut commands = commands.iter().map(|c| format!("{c}\n")).collect::<Vec<_>>().into_iter().chain(["echo RIBBIT-END\n".to_string()]);
        let (mut prompts, mut quiet, mut seen_len) = (0, 0, 0);
        for _ in 0..8000 {
            lx.run(1_000_000);
            let out = lx.bus.uart.first().map_or("", |s| s.as_str());
            quiet = if out.len() > seen_len { 0 } else { quiet + 1 };
            seen_len = out.len();
            if out.contains("\nRIBBIT-END") {
                break;
            }
            let seen = out.matches("# ").count();
            if seen > prompts && quiet >= 50 {
                prompts = seen;
                if let Some(c) = commands.next() {
                    lx.bus.uart_rx.extend(c.bytes());
                }
            }
        }
        let out = lx.bus.uart.first().cloned().unwrap_or_default();
        (out, std::mem::take(&mut lx.bus.nand))
    }

    /// Enhanced (new)'s saves are journal writes, as the Explorer's kernel
    /// makes them: a file written in a session reaches a small save, quickly;
    /// Original's kernel mounts it (no UBIFS error) and reads the file, and
    /// writes its own; the next session in Enhanced (new) finds both, changes
    /// one and removes another into the save Original's kernel left, and
    /// Original reads that too. RIBBIT_EXPLORER_DIR as above.
    #[test]
    #[ignore]
    fn explorer_hle_save_journal() {
        let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_DIR").expect("RIBBIT_EXPLORER_DIR"));
        let saves = std::env::temp_dir().join("ribbit-explorer-hle-journal");
        let _ = std::fs::remove_dir_all(&saves);
        let first: Vec<u8> = (0..20_000u32).map(|i| b"ribbit journal "[i as usize % 15]).collect();
        let (bulk_file, root_file, lle_file) = ("/LF/Bulk/Data/ribbit-journal.txt", "/flags/ribbit-journal", "/LF/Bulk/Data/ribbit-lle.txt");
        let session = |change: &dyn Fn(&mut ribbit_didj::hle::vfs::Vfs)| {
            let built = DidjSession::build_explorer(&dir, &saves).unwrap();
            let lx = DidjSession::start_explorer(built, None, Arc::new(crate::serial::Serial::default()), true).unwrap();
            std::thread::sleep(Duration::from_secs(10));
            change(&mut lx.shared.hle.lock().unwrap().as_mut().unwrap().p.vfs);
            let (original, save) = (lx.original.clone(), lx.save_path.clone());
            let t = Instant::now();
            lx.stop();
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let changes = std::fs::read(&save).unwrap();
            eprintln!("closed in {ms:.0} ms, the save {} bytes", changes.len());
            let mut flash = (*original).clone();
            flash.apply_changes(&changes).unwrap();
            (flash, original, save, changes.len())
        };
        let file = |files: &[ribbit_didj::ubifs_read::File], p: &str| files.iter().find(|f| f.path == p).map(|f| f.data.clone());

        // Enhanced (new): two files written.
        let (flash, original, save, size) = session(&|vfs| {
            vfs.add_file(bulk_file, first.clone());
            vfs.add_file(root_file, b"flag".to_vec());
        });
        assert!(size < 2 << 20, "a small save: {size} bytes");
        let (root, bulk) = ribbit_didj::hle::explorer::files_in_nand(&flash).unwrap();
        assert_eq!(file(&bulk, "/Data/ribbit-journal.txt"), Some(first.clone()));
        assert_eq!(file(&root, "/flags/ribbit-journal"), Some(b"flag".to_vec()));
        // Carried over to another image from the save's blocks alone (the
        // journal's: no master node, no index).
        let carried = ribbit_didj::carry::explorer_user_files(&std::fs::read(&save).unwrap(), &original).unwrap();
        assert!(matches!(carried.get("Data/ribbit-journal.txt").map(|e| &e.kind), Some(ribbit_didj::ubifs::Kind::File(d)) if *d == first), "carried: {:?}", carried.keys().collect::<Vec<_>>());

        // Original reads them, and writes a file of its own.
        let (out, flash) = explorer_console(flash, &[&format!("wc -c {bulk_file}"), &format!("tail -c 30 {bulk_file}"), &format!("cat {root_file}"), &format!("echo from-original > {lle_file}"), "sync", "dmesg | grep -c 'UBIFS error'"]);
        assert!(out.contains(&format!("20000 {bulk_file}")), "Original reads the Bulk file:\n{out}");
        assert!(out.contains(std::str::from_utf8(&first[first.len() - 30..]).unwrap()), "Original reads the Bulk file:\n{out}");
        assert!(out.contains("\nflag"), "Original reads the root file:\n{out}");
        assert!(out.contains("UBIFS error'\r\n0") || out.contains("UBIFS error'\n0"), "no UBIFS error:\n{out}");
        // (Original's first start formats the root file system's blocks
        // past LeapFrog's image: its save is big already.)
        let lle_save = flash.changes_from(&original);
        eprintln!("Original's save {} bytes", lle_save.len());
        std::fs::write(&save, &lle_save).unwrap();

        // Enhanced (new) from Original's save: both there; one changed
        // (shorter), the other removed.
        let second = b"second version".to_vec();
        let (flash, _, _, size) = session(&|vfs| {
            assert_eq!(vfs.get(lle_file).map(|n| n.size()), Some(14), "Original's file");
            assert_eq!(vfs.get(bulk_file).map(|n| n.size()), Some(first.len() as u64));
            vfs.add_file(bulk_file, second.clone());
            vfs.remove(root_file);
        });
        assert!(size < lle_save.len() + (2 << 20), "a small change to the save: {size} bytes");
        let (root, bulk) = ribbit_didj::hle::explorer::files_in_nand(&flash).unwrap();
        assert_eq!(file(&bulk, "/Data/ribbit-journal.txt"), Some(second.clone()));
        assert_eq!(file(&bulk, "/Data/ribbit-lle.txt"), Some(b"from-original\n".to_vec()));
        assert_eq!(file(&root, "/flags/ribbit-journal"), None);

        // And Original once more.
        let (out, _) = explorer_console(flash, &[&format!("cat {bulk_file}"), &format!("ls {root_file}"), "dmesg | grep -c 'UBIFS error'"]);
        assert!(out.contains("\nsecond version"), "Original reads the change:\n{out}");
        assert!(out.contains("No such file"), "the root file is gone:\n{out}");
        assert!(out.contains("UBIFS error'\r\n0") || out.contains("UBIFS error'\n0"), "no UBIFS error:\n{out}");
    }

    /// The Explorer's memory in a session: Original, or Enhanced (new) with
    /// RIBBIT_EXPLORER_HLE=1; RIBBIT_EXPLORER_DIR as above. Prints the
    /// working set and private bytes at the menu.
    #[test]
    #[ignore]
    fn explorer_memory() {
        let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_DIR").expect("RIBBIT_EXPLORER_DIR"));
        let hle = std::env::var("RIBBIT_EXPLORER_HLE").is_ok_and(|v| v == "1");
        let saves = std::env::temp_dir().join("ribbit-explorer-memory");
        let _ = std::fs::remove_dir_all(&saves);
        let built = DidjSession::build_explorer(&dir, &saves).unwrap();
        let (ws, private) = memory_mb();
        eprintln!("built: {ws:.0} MB working set, {private:.0} MB private");
        let lx = DidjSession::start_explorer(built, None, Arc::new(crate::serial::Serial::default()), hle).unwrap();
        std::thread::sleep(Duration::from_secs(15));
        let (ws, private) = memory_mb();
        eprintln!("{} running: {ws:.0} MB working set, {private:.0} MB private", if hle { "Enhanced (new)" } else { "Original" });
        lx.stop();
    }

    /// Explorer Enhanced (new) save states: one made at the menu, a file
    /// added, the state loaded (the file gone) and the programs going on.
    #[test]
    #[ignore]
    fn explorer_hle_states() {
        let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_DIR").expect("RIBBIT_EXPLORER_DIR"));
        let saves = std::env::temp_dir().join("ribbit-explorer-hle-states");
        let _ = std::fs::remove_dir_all(&saves);
        let built = DidjSession::build_explorer(&dir, &saves).unwrap();
        let lx = DidjSession::start_explorer(built, None, Arc::new(crate::serial::Serial::default()), true).unwrap();
        std::thread::sleep(Duration::from_secs(12));
        let mut before = Vec::new();
        lx.frame(&mut before);
        let t = Instant::now();
        let state = lx.save_state().expect("a state");
        eprintln!("state {} bytes in {:.0} ms", state.len(), t.elapsed().as_secs_f64() * 1000.0);
        lx.shared.hle.lock().unwrap().as_mut().unwrap().p.vfs.add_file("/LF/Bulk/ribbit-state-test", b"x".to_vec());
        std::thread::sleep(Duration::from_secs(2));
        let t = Instant::now();
        lx.load_state(&state).unwrap();
        eprintln!("loaded in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
        let gone = lx.shared.hle.lock().unwrap().as_ref().unwrap().p.vfs.get("/LF/Bulk/ribbit-state-test").is_none();
        std::thread::sleep(Duration::from_secs(3));
        let mut after = Vec::new();
        lx.frame(&mut after);
        let lit = after.iter().filter(|&&p| p != 0).count();
        lx.stop();
        assert!(gone);
        assert!(lit > after.len() / 4);
        assert_eq!(before, after, "the menu stands still: the same picture after the load");
    }

    /// A Didj cartridge in the Explorer's slot in Enhanced (new), put in at
    /// the menu: CartManager mounts it at /LF/Cart through the HLE, and it
    /// goes when taken out. RIBBIT_EXPLORER_DIR as above, RIBBIT_EXPLORER_CART
    /// a Didj cartridge dump.
    #[test]
    #[ignore]
    fn explorer_hle_cart() {
        let dir = PathBuf::from(std::env::var("RIBBIT_EXPLORER_DIR").expect("RIBBIT_EXPLORER_DIR"));
        let cart = read_cart(Path::new(&std::env::var("RIBBIT_EXPLORER_CART").expect("RIBBIT_EXPLORER_CART"))).expect("a cartridge dump");
        let saves = std::env::temp_dir().join("ribbit-explorer-hle-cart");
        let _ = std::fs::remove_dir_all(&saves);
        let built = DidjSession::build_explorer(&dir, &saves).unwrap();
        let lx = DidjSession::start_explorer(built, None, Arc::new(crate::serial::Serial::default()), true).unwrap();
        std::thread::sleep(Duration::from_secs(12));
        lx.set_cart(Some(cart)).unwrap();
        let mounted = || lx.shared.hle.lock().unwrap().as_ref().unwrap().p.vfs.list("/LF/Cart").len();
        let t = Instant::now();
        while mounted() == 0 && t.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(200));
        }
        let files = mounted();
        eprintln!("/LF/Cart has {files} entries {:.1} s after the cartridge went in", t.elapsed().as_secs_f64());
        lx.set_cart(None).unwrap();
        std::thread::sleep(Duration::from_secs(2));
        let after = mounted();
        lx.stop();
        assert!(files > 0);
        assert_eq!(after, 0);
    }

    /// Save states and closing, in every mode: Original, Enhanced and
    /// Enhanced (new), at the menu or (RIBBIT_DIDJ_CART, a cartridge dump)
    /// in its game. A state made there loads and the console runs on (twice
    /// over); then the session is closed as the app closes it, and the save
    /// it left starts a console (Original mode) whose start finds its file
    /// systems clean (no file check, no repair: no tune-up screen).
    /// RIBBIT_DIDJ_SAVE: a set-up console's .nandblocks (its .nandimage
    /// beside it); RIBBIT_DIDJ_WAIT: seconds before the state (25).
    #[test]
    #[ignore]
    fn didj_states_and_close() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let save = std::env::var("RIBBIT_DIDJ_SAVE").ok().map(PathBuf::from);
        let cart = std::env::var("RIBBIT_DIDJ_CART").ok().map(PathBuf::from);
        let wait = std::env::var("RIBBIT_DIDJ_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(25);
        let modes = [("original", false, false), ("enhanced", true, false), ("new", true, true)];
        let runs: Vec<_> = modes
            .into_iter()
            .map(|(name, enhanced, hle)| {
                let (dir, save, cart) = (dir.clone(), save.clone(), cart.clone());
                std::thread::spawn(move || {
                    let saves = std::env::temp_dir().join(format!("ribbit-states-{name}"));
                    let _ = std::fs::remove_dir_all(&saves);
                    std::fs::create_dir_all(&saves).unwrap();
                    if let Some(save) = &save {
                        std::fs::copy(save, save_file(&dir, &saves)).unwrap();
                        std::fs::copy(save.with_extension("nandimage"), save_file(&dir, &saves).with_extension("nandimage")).unwrap();
                    }
                    let dump = cart.as_deref().map(|p| read_cart(p).expect("a cartridge"));
                    let art = cart.as_deref().and_then(|p| menu_art(p, dump.as_deref()));
                    let mode = Mode { enhanced, cpu_scale: if enhanced { 2 } else { 1 }, frame_skip: false, hd: 1, hle, smooth: false };
                    let didj = DidjSession::start(&dir, &saves, dump.clone(), None, art, mode, Arc::new(crate::serial::Serial::default())).unwrap();
                    let flips = || match didj.shared.hle.lock().unwrap().as_ref() {
                        Some(h) => h.p.m.bus.flips,
                        None => didj.shared.didj.lock().unwrap().bus.flips,
                    };
                    let lit = || {
                        let mut px = Vec::new();
                        let (w, h) = didj.frame(&mut px);
                        px.iter().filter(|&&p| p != 0).count() * 4 > w * h
                    };
                    std::thread::sleep(Duration::from_secs(wait));
                    let mut ok = Vec::new();
                    for round in 0..2 {
                        let state = didj.save_state().expect("a state");
                        std::thread::sleep(Duration::from_secs(3));
                        let loaded = didj.load_state(&state);
                        let before = flips();
                        std::thread::sleep(Duration::from_secs(5));
                        let (after, shown) = (flips(), lit());
                        eprintln!("{name} round {round}: state {} bytes, loaded {loaded:?}, {} flips in 5 s, picture {shown}, speed {:.0}%", state.len(), after - before, didj.speed_percent());
                        ok.push(loaded.is_ok() && after > before + 10 && shown);
                    }
                    let t = Instant::now();
                    didj.stop();
                    let closed = t.elapsed().as_secs_f32();
                    // The next start, in Original mode (the one that shows the
                    // tune-up screen), from the save the close left.
                    let serial = Arc::new(crate::serial::Serial::default());
                    let original = Mode { enhanced: false, cpu_scale: 1, frame_skip: false, hd: 1, hle: false, smooth: false };
                    let again = DidjSession::start(&dir, &saves, dump, None, None, original, serial.clone()).unwrap();
                    std::thread::sleep(Duration::from_secs(20));
                    let text = serial.text();
                    let checked = text.contains("fsck check on");
                    let refused = text.contains("needs_repair");
                    eprintln!("{name}: closed in {closed:.1} s; next start: file check {checked}, needs repair {refused}");
                    again.stop();
                    let _ = std::fs::remove_dir_all(&saves);
                    (name, ok, checked || refused)
                })
            })
            .collect();
        for r in runs {
            let (name, ok, unclean) = r.join().unwrap();
            assert!(ok.iter().all(|&o| o), "{name}: state round trips {ok:?}");
            assert!(!unclean, "{name}: the next start found the file systems unclean");
        }
    }

    /// A Didj save folder, imported (`import_didj_folder`). It is made
    /// up: Ribbit's own /Didj written out as a host
    /// folder (assets/didj, its Data also as assets/factory/Data), with a
    /// set-up console's player files over it (RIBBIT_DIDJ_SAVE, its
    /// .nandimage beside it), profile 0 renamed IMPORTED, and a Didji as
    /// Connect sends them (no Device line). Imported into a new Didj, the
    /// profile, the Didji and the game progress are on the console in every
    /// mode: Original and Enhanced (the console's shell lists them) and
    /// Enhanced (new) (its files). RIBBIT_DIDJ_IMPORT_OUT: keep the save made
    /// there (didj-<folder>.nandblocks) for didjboot.
    #[test]
    #[ignore]
    fn didj_folder_import() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let setup = PathBuf::from(std::env::var("RIBBIT_DIDJ_SAVE").expect("RIBBIT_DIDJ_SAVE"));
        let root = std::env::temp_dir().join("ribbit-didj-folder-import");
        let _ = std::fs::remove_dir_all(&root);
        let didj = root.join("Backup").join("assets").join("didj");
        let put = |base: &Path, p: &str, d: &[u8]| {
            let f = base.join(p.trim_start_matches('/'));
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, d).unwrap();
        };
        let (original, _) = ribbit_didj::nand_and_extras_missing(&dir, true).unwrap();
        let brio = ribbit_didj::nand::partition("Brio").unwrap();
        let volume = ribbit_didj::ubi::read_volume(&original, brio, ribbit_didj::brio::volume_sectors(brio.size / ribbit_didj::nand::BLOCK) * 512);
        for (p, d) in ribbit_core::fatfs::known_files(&volume, &|_| true) {
            put(&didj, &p, &d);
            if let Some(rest) = p.strip_prefix("/Data/") {
                put(&root.join("Backup").join("assets").join("factory").join("Data"), rest, &d);
            }
        }
        let mut flash = original.clone();
        flash.apply_changes(&std::fs::read(&setup).unwrap()).unwrap();
        let players = ribbit_didj::carry::files_beyond(&flash, &original);
        let mut profile = None;
        for (p, d) in &players {
            let mut d = d.clone();
            if p.eq_ignore_ascii_case("/Data/0/profile.dsc") {
                let text = String::from_utf8_lossy(&d).into_owned();
                let name = text.lines().find_map(|l| l.strip_prefix("Name=")).unwrap_or("").trim().to_string();
                d = text.replacen(&format!("Name={name}"), "Name=IMPORTED", 1).into_bytes();
                profile = Some(d.clone());
            }
            put(&didj, p, &d);
        }
        let profile = profile.expect("the set-up save has profile 0");
        let didji = didj.join("Data").join("Avatars").join("Avatar7");
        std::fs::create_dir_all(&didji).unwrap();
        for e in std::fs::read_dir(didj.join("Data").join("Avatars").join("Avatar1")).unwrap().flatten().filter(|e| e.path().is_file()) {
            std::fs::copy(e.path(), didji.join(e.file_name())).unwrap();
        }
        std::fs::write(didji.join("meta.inf"), b"Name=\"Zed\"
DeviceTypes=\"DIDJ\"
ProfileAccess=\"0\"
").unwrap();
        let game = players.iter().find(|(p, _)| p.to_ascii_lowercase().starts_with("/data/0/") && !p.to_ascii_lowercase().ends_with(".dsc") && !p.to_ascii_lowercase().ends_with(".log") && !p.to_ascii_lowercase().contains("backup")).expect("game progress").0.clone();
        let saves = std::env::var("RIBBIT_DIDJ_IMPORT_OUT").map(PathBuf::from).unwrap_or_else(|_| root.join("saves"));
        let _ = std::fs::remove_file(save_file(&dir, &saves));
        let r = import_didj_folder(&root.join("Backup"), &dir, &saves).unwrap();
        eprintln!("imported {} files, {} profiles, {} packages; game progress {game}", r.files, r.profiles, r.packages);
        assert_eq!(r.profiles, 1);
        let modes = [("original", false, false), ("enhanced", true, false), ("new", true, true)];
        for (name, enhanced, hle) in modes {
            let run = root.join(format!("saves-{name}"));
            std::fs::create_dir_all(&run).unwrap();
            for ext in ["nandblocks", "nandimage", "files"] {
                std::fs::copy(save_file(&dir, &saves).with_extension(ext), save_file(&dir, &run).with_extension(ext)).unwrap();
            }
            let serial = Arc::new(crate::serial::Serial::default());
            let mode = Mode { enhanced, cpu_scale: 1, frame_skip: false, hd: 1, hle, smooth: false };
            let session = DidjSession::start(&dir, &run, None, None, None, mode, serial.clone()).unwrap();
            let (has_profile, has_didji, has_game) = if hle {
                std::thread::sleep(Duration::from_secs(5));
                let h = session.shared.hle.lock().unwrap();
                let files = h.as_ref().unwrap().p.vfs.files_under("/Didj");
                let file = |path: &str| files.iter().find(|(p, _)| p.eq_ignore_ascii_case(path)).map(|(_, d)| d.to_vec());
                let meta = file("/Data/Avatars/Avatar7/meta.inf").unwrap_or_default();
                (file("/Data/0/profile.dsc") == Some(profile.clone()), String::from_utf8_lossy(&meta).contains("Device=\"Didj\""), file(&game).is_some())
            } else {
                std::thread::sleep(Duration::from_secs(25));
                serial.type_bytes(format!("
cat /Didj/Data/0/profile.dsc; cat /Didj/Data/Avatars/Avatar7/meta.inf; ls -l /Didj{game}
").as_bytes());
                std::thread::sleep(Duration::from_secs(5));
                let text = serial.text();
                (text.contains("Name=IMPORTED"), text.contains("Device=\"Didj\""), !text.contains("No such file"))
            };
            eprintln!("{name}: profile {has_profile}, Didji {has_didji}, game progress {has_game}");
            session.stop();
            assert!(has_profile && has_didji && has_game, "{name}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A real Didj save folder (RIBBIT_DIDJ_IMPORT_SAVE: a copied /Didj
    /// tree, read only; RIBBIT_DIDJ_IMPORT_NAME: profile 0's name),
    /// imported into a new Didj: in Original, Original+ (Enhanced) and
    /// Enhanced (new) the profile is on the console, and still there after
    /// closing and starting again. Pictures of Sign In go to
    /// RIBBIT_DIDJ_SHOT_DIR (import-<mode>-<n>.png).
    #[test]
    #[ignore]
    fn didj_folder_real_save() {
        let dir = PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let from = PathBuf::from(std::env::var("RIBBIT_DIDJ_IMPORT_SAVE").expect("RIBBIT_DIDJ_IMPORT_SAVE"));
        let name = std::env::var("RIBBIT_DIDJ_IMPORT_NAME").unwrap_or_else(|_| "IMPORTED".into());
        let saves = std::env::temp_dir().join("ribbit-didj-folder-real");
        let _ = std::fs::remove_dir_all(&saves);
        let r = import_didj_folder(&from, &dir, &saves).unwrap();
        eprintln!("imported {} files, {} profiles, {} packages", r.files, r.profiles, r.packages);
        let shot = |didj: &DidjSession, file: &str| {
            let Ok(shots) = std::env::var("RIBBIT_DIDJ_SHOT_DIR") else { return };
            let mut px = Vec::new();
            let (w, h) = didj.frame(&mut px);
            let mut enc = png::Encoder::new(std::fs::File::create(Path::new(&shots).join(file)).unwrap(), w as u32, h as u32);
            enc.set_color(png::ColorType::Rgb);
            let rgb: Vec<u8> = px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
            enc.write_header().unwrap().write_image_data(&rgb).unwrap();
        };
        for (mode_name, enhanced, hle) in [("original", false, false), ("originalplus", true, false), ("new", true, true)] {
            let run = saves.join(mode_name);
            std::fs::create_dir_all(&run).unwrap();
            for ext in ["nandblocks", "nandimage", "files"] {
                std::fs::copy(save_file(&dir, &saves).with_extension(ext), save_file(&dir, &run).with_extension(ext)).unwrap();
            }
            for n in 0..2 {
                let serial = Arc::new(crate::serial::Serial::default());
                let mode = Mode { enhanced, cpu_scale: 1, frame_skip: false, hd: 1, hle, smooth: false };
                let session = DidjSession::start(&dir, &run, None, None, None, mode, serial.clone()).unwrap();
                std::thread::sleep(Duration::from_secs(30));
                shot(&session, &format!("import-{mode_name}-{n}.png"));
                let has = if hle {
                    let h = session.shared.hle.lock().unwrap();
                    let files = h.as_ref().unwrap().p.vfs.files_under("/Didj");
                    files.iter().any(|(p, d)| p.eq_ignore_ascii_case("/Data/0/profile.dsc") && String::from_utf8_lossy(d).contains(&format!("Name={name}")))
                } else {
                    serial.type_bytes(b"
cat /Didj/Data/0/profile.dsc /Didj/Base/settings.cfg
");
                    std::thread::sleep(Duration::from_secs(3));
                    serial.text().contains(&format!("Name={name}"))
                };
                eprintln!("{mode_name} start {n}: profile {name} {has}");
                session.stop();
                assert!(has, "{mode_name} start {n}");
            }
        }
    }
}
