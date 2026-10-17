//! The LeapFrog RockIt Twist in the frontend: its packages in the library,
//! its keyboard layout, and the session. When the system packages are
//! found (Bootloader, Base, Firmware-Bulk), a session starts the console
//! from its bootloader on `ribbit_twist::machine`, which is far from
//! complete: it reaches the base firmware's own screens. Otherwise the
//! session shows the controls working and says so.

use crate::audio_out::{AudioOut, AudioSink};
use crate::library::{Art, Family, Game};
use ribbit_twist::input::{Button, Controls, Rotary, Source, Step};
use ribbit_twist::package::{self, Kind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use winit::keyboard::KeyCode;

/// What a key does on the RockIt Twist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwistKey {
    Press(Button),
    /// Turns a control one way while held (repeating).
    Turn(Rotary, Step),
    /// A swipe along the Slider, up (true) or down.
    Swipe(bool),
}

/// The keyboard layout (from the layout picture the project owner drew):
/// WASD or the arrows for the D-pad, Q and E for the Switch, the number pad
/// for the button grid (laid out as it is: 7 8 9 on top), * and - on the
/// number pad for the Spinner, U/J, I/K and O/L for the yellow, green and
/// red Dials, Page Up/Down (or the mouse wheel) for the Slider, Shift or Tab
/// for Hint, Home, and Enter for the check mark. Volume is - and =, as on
/// the Leapster. These replace the Leapster's keys (and Tab's fast forward)
/// only while a RockIt Twist session is open.
pub const KEYS: &[(KeyCode, TwistKey)] = &[
    (KeyCode::KeyW, TwistKey::Press(Button::DpadUp)),
    (KeyCode::ArrowUp, TwistKey::Press(Button::DpadUp)),
    (KeyCode::KeyS, TwistKey::Press(Button::DpadDown)),
    (KeyCode::ArrowDown, TwistKey::Press(Button::DpadDown)),
    (KeyCode::KeyA, TwistKey::Press(Button::DpadLeft)),
    (KeyCode::ArrowLeft, TwistKey::Press(Button::DpadLeft)),
    (KeyCode::KeyD, TwistKey::Press(Button::DpadRight)),
    (KeyCode::ArrowRight, TwistKey::Press(Button::DpadRight)),
    (KeyCode::KeyQ, TwistKey::Press(Button::SwitchLeft)),
    (KeyCode::KeyE, TwistKey::Press(Button::SwitchRight)),
    (KeyCode::Numpad7, TwistKey::Press(Button::Grid(0))),
    (KeyCode::Numpad8, TwistKey::Press(Button::Grid(1))),
    (KeyCode::Numpad9, TwistKey::Press(Button::Grid(2))),
    (KeyCode::Numpad4, TwistKey::Press(Button::Grid(3))),
    (KeyCode::Numpad5, TwistKey::Press(Button::Grid(4))),
    (KeyCode::Numpad6, TwistKey::Press(Button::Grid(5))),
    (KeyCode::Numpad1, TwistKey::Press(Button::Grid(6))),
    (KeyCode::Numpad2, TwistKey::Press(Button::Grid(7))),
    (KeyCode::Numpad3, TwistKey::Press(Button::Grid(8))),
    (KeyCode::NumpadMultiply, TwistKey::Turn(Rotary::Spinner, -1)),
    (KeyCode::NumpadSubtract, TwistKey::Turn(Rotary::Spinner, 1)),
    (KeyCode::KeyU, TwistKey::Turn(Rotary::YellowDial, 1)),
    (KeyCode::KeyJ, TwistKey::Turn(Rotary::YellowDial, -1)),
    (KeyCode::KeyI, TwistKey::Turn(Rotary::GreenDial, 1)),
    (KeyCode::KeyK, TwistKey::Turn(Rotary::GreenDial, -1)),
    (KeyCode::KeyO, TwistKey::Turn(Rotary::RedDial, 1)),
    (KeyCode::KeyL, TwistKey::Turn(Rotary::RedDial, -1)),
    (KeyCode::PageUp, TwistKey::Swipe(true)),
    (KeyCode::PageDown, TwistKey::Swipe(false)),
    (KeyCode::ShiftLeft, TwistKey::Press(Button::Hint)),
    (KeyCode::ShiftRight, TwistKey::Press(Button::Hint)),
    (KeyCode::Tab, TwistKey::Press(Button::Hint)),
    (KeyCode::Home, TwistKey::Press(Button::Home)),
    (KeyCode::Enter, TwistKey::Press(Button::Check)),
    (KeyCode::NumpadEnter, TwistKey::Press(Button::Check)),
    (KeyCode::Equal, TwistKey::Press(Button::VolumeUp)),
    (KeyCode::Minus, TwistKey::Press(Button::VolumeDown)),
];

pub fn key(code: KeyCode) -> Option<TwistKey> {
    KEYS.iter().find(|(k, _)| *k == code).map(|(_, t)| *t)
}

/// The first key for an action, as its keycap shows it.
pub fn cap_for(what: TwistKey) -> Option<String> {
    KEYS.iter().find(|(_, t)| *t == what).map(|(k, _)| crate::settings::cap_text(&format!("{k:?}")))
}

/// The keys for a control, for tooltips ("W / Up").
pub fn keys_for(what: TwistKey) -> String {
    let names: Vec<String> = KEYS.iter().filter(|(_, t)| *t == what).map(|(k, _)| key_label(*k)).collect();
    names.join(" / ")
}

fn key_label(k: KeyCode) -> String {
    match k {
        KeyCode::NumpadMultiply => "Num *".into(),
        KeyCode::NumpadSubtract => "Num -".into(),
        KeyCode::NumpadEnter => "Num Enter".into(),
        KeyCode::Equal => "=".into(),
        KeyCode::Minus => "-".into(),
        k => {
            let n = format!("{k:?}");
            if let Some(d) = n.strip_prefix("Numpad") {
                return format!("Num {d}");
            }
            crate::settings::key_label(&n)
        }
    }
}

/// The packages the console starts from, by package ID (VERIFIED from
/// their `meta.inf` names): Bootloader, Base (the System firmware),
/// Firmware-Bulk (the FAT volume, `FileSys`) and MfgTest (the factory
/// test, kept in its partition; optional).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemPackages {
    pub bootloader: PathBuf,
    pub base: PathBuf,
    pub bulk: PathBuf,
    pub factory_test: Option<PathBuf>,
}

const BOOTLOADER_ID: &str = "PCKT-0x004F0001-000000";
const BASE_ID: &str = "PCKT-0x004F0002-000000";
const MFGTEST_ID: &str = "PCKT-0x004F000A-000000";
const BULK_ID: &str = "PCKT-0x004F000B-000000";

/// Whether `p` is a system package unpacked: a folder named by its package
/// ID holding `Firmware/meta.inf` (what a `.lfp` zip holds).
pub fn is_package_folder(p: &Path) -> bool {
    p.join("Firmware").join("meta.inf").is_file()
}

/// `.lfp` files and unpacked package folders under `dir`, at most `depth`
/// folders down.
fn lfp_files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if is_package_folder(&p) {
                out.push(p);
            } else if depth > 0 {
                lfp_files(&p, depth - 1, out);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lfp")) {
            out.push(p);
        }
    }
}

/// Where Ribbit keeps RockIt Twist system packages of its own (in its data
/// folder), looked in as well as the games folders.
pub fn system_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("rockit-twist").join("system")
}

/// The system packages (`.lfp` files or unpacked package folders) in `dir`.
pub fn packages_in(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    lfp_files(dir, 3, &mut out);
    out
}

/// The locale folder a package sits in (en-us/<ID>/<version>/...).
fn locale_of(path: &Path) -> Option<String> {
    path.ancestors()
        .filter_map(|a| a.file_name()?.to_str())
        .find(|n| n.len() == 5 && n.as_bytes()[2] == b'-')
        .map(str::to_ascii_lowercase)
}

impl SystemPackages {
    /// The system packages among `known` (the library's RockIt Twist
    /// files) and those near `near` (the package opened: its locale
    /// folder, as the archive lays packages out). Same locale as `near`
    /// first, then the newest version (the highest path).
    pub fn find(near: Option<&Path>, known: &[PathBuf]) -> Option<Self> {
        let mut all: Vec<PathBuf> = known.to_vec();
        // <locale>/<ID>/<version>/<file>: three folders up.
        if let Some(locale_dir) = near.and_then(|n| n.ancestors().nth(3)) {
            lfp_files(locale_dir, 2, &mut all);
        }
        let want = near.and_then(locale_of);
        all.sort_by(|a, b| {
            let same = |p: &PathBuf| want.is_some() && locale_of(p) == want;
            same(b).cmp(&same(a)).then(b.cmp(a))
        });
        let pick = |id: &str| {
            all.iter()
                .find(|p| p.file_name().and_then(|n| n.to_str()).and_then(package::package_id) == Some(id))
                .cloned()
        };
        Some(Self {
            bootloader: pick(BOOTLOADER_ID)?,
            base: pick(BASE_ID)?,
            bulk: pick(BULK_ID)?,
            factory_test: pick(MFGTEST_ID),
        })
    }
}

/// A file from a system package (a `.lfp` zip, or the package unpacked
/// into a folder): `name`, or the firmware image its `meta.inf` names as
/// `BinFile`.
fn package_file(path: &Path, name: Option<&str>) -> anyhow::Result<Vec<u8>> {
    use anyhow::Context;
    if is_package_folder(path) {
        let firmware = path.join("Firmware");
        let entry = match name {
            Some(n) => n.to_string(),
            None => {
                let meta = std::fs::read(firmware.join("meta.inf")).context("reading meta.inf")?;
                let meta = package::Meta::parse(&String::from_utf8_lossy(&meta));
                meta.get("BinFile").context("meta.inf names no BinFile")?.to_string()
            }
        };
        let file = firmware.join(&entry);
        return std::fs::read(&file).with_context(|| format!("reading {}", file.display()));
    }
    let data = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let entry = match name {
        Some(n) => n.to_string(),
        None => {
            let meta = ribbit_core::zip::extract_named(&data, "Firmware/meta.inf").context("no Firmware/meta.inf")?;
            let meta = package::Meta::parse(&String::from_utf8_lossy(&meta));
            meta.get("BinFile").context("meta.inf names no BinFile")?.to_string()
        }
    };
    ribbit_core::zip::extract_named(&data, &format!("Firmware/{entry}"))
        .with_context(|| format!("{} has no Firmware/{entry}", path.display()))
}

/// The result of importing a zip of installed content.
#[derive(Debug)]
pub struct Imported {
    /// The folder now holding it, laid out like the console's F: drive.
    pub dir: PathBuf,
    pub locale: String,
    /// The packages' names.
    pub packs: Vec<String>,
    pub files: usize,
}

/// Unpacks a zip of RockIt Twist content (package folders, each with its
/// `meta.inf`) into `into`, laid out like the console's F: drive
/// (`ribbit_twist::content`): the packages of one region, `prefer` if the
/// zip has it, else en-us, else the first. `into` is emptied first.
pub fn import_zip(zip: &Path, into: &Path, prefer: Option<&str>) -> anyhow::Result<Imported> {
    use anyhow::Context;
    use ribbit_twist::content;
    let data = std::fs::read(zip).with_context(|| format!("reading {}", zip.display()))?;
    let entries = ribbit_core::zip::entries(&data).context("reading the zip's list of files")?;
    let mut packs = Vec::new();
    for e in
        entries.iter().filter(|e| e.name.rsplit(['/', '\\']).next().is_some_and(|n| n.eq_ignore_ascii_case("meta.inf")))
    {
        let text = ribbit_core::zip::read_entry(&data, e).with_context(|| format!("reading {}", e.name))?;
        packs.extend(content::pack(&e.name, &String::from_utf8_lossy(&text)));
    }
    let locales = content::locales(&packs);
    let locale = [prefer.unwrap_or("en-us"), "en-us"]
        .into_iter()
        .find(|l| locales.iter().any(|x| x.eq_ignore_ascii_case(l)))
        .map(str::to_string)
        .or_else(|| locales.first().cloned())
        .context(
            "the zip holds no RockIt Twist packages (no meta.inf with a Type of Application, Music, Video or Trailer)",
        )?;
    let chosen = content::for_locale(&packs, &locale);
    if into.exists() {
        std::fs::remove_dir_all(into).with_context(|| format!("emptying {}", into.display()))?;
    }
    let mut files = 0;
    for e in &entries {
        let Some(dest) = content::destination_of(&chosen, &e.name) else {
            continue;
        };
        let out = into.join(dest.replace('/', std::path::MAIN_SEPARATOR_STR));
        if let Some(dir) = out.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
        }
        let bytes = ribbit_core::zip::read_entry(&data, e).with_context(|| format!("unpacking {}", e.name))?;
        std::fs::write(&out, bytes).with_context(|| format!("writing {}", out.display()))?;
        files += 1;
    }
    Ok(Imported { dir: into.to_path_buf(), locale, packs: chosen.into_iter().map(|p| p.name).collect(), files })
}

/// The region of the system packages (their archive folder, "en-us"...).
pub fn system_locale(p: &SystemPackages) -> Option<String> {
    locale_of(&p.base)
}

/// What the console's thread shares with the window.
struct Shared {
    /// The screen, 320 x 240 (0x00RRGGBB).
    screen: Mutex<Vec<u32>>,
    running: AtomicBool,
    stop: AtomicBool,
    /// The console's time (clock cycles since power-on).
    now: AtomicU64,
    /// The buttons held (`machine::key` bits).
    keys: AtomicU32,
    /// Detents turned so far by each rotary control (`input::Rotary`
    /// order, wrapping), and the Slider pads touched (`encoder::pads`).
    turned: [AtomicI32; 4],
    slider_pads: AtomicU32,
    /// Why it could not start.
    error: Mutex<Option<String>>,
    /// Where the sound goes, and its volume (an f32's bits).
    sink: Mutex<Option<AudioSink>>,
    gain: AtomicU32,
}

/// The console running on its own thread, in real time.
pub struct Machine {
    shared: Arc<Shared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// For the speed shown: when it was last measured, the console's time
    /// then, and the figure (tenths of a percent).
    speed_from: Mutex<(Instant, u64)>,
    speed_shown: AtomicU64,
}

impl Machine {
    /// `content`: a folder written onto the storage (the volume's root).
    pub fn start(packages: SystemPackages, content: Option<PathBuf>, sink: Option<AudioSink>) -> Self {
        let shared = Arc::new(Shared {
            screen: Mutex::new(vec![0; ribbit_twist::SCREEN_W * ribbit_twist::SCREEN_H]),
            running: AtomicBool::new(true),
            stop: AtomicBool::new(false),
            now: AtomicU64::new(0),
            keys: AtomicU32::new(0),
            turned: Default::default(),
            slider_pads: AtomicU32::new(0),
            error: Mutex::new(None),
            sink: Mutex::new(sink),
            gain: AtomicU32::new(1f32.to_bits()),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("rockit-twist".into())
            .spawn(move || {
                if let Err(e) = Self::run(&packages, content.as_deref(), &s) {
                    *s.error.lock().unwrap() = Some(format!("{e:#}"));
                }
            })
            .ok();
        Self { shared, thread, speed_from: Mutex::new((Instant::now(), 0)), speed_shown: AtomicU64::new(0) }
    }

    fn run(p: &SystemPackages, content: Option<&Path>, s: &Shared) -> anyhow::Result<()> {
        use ribbit_twist::{image, machine};
        let boot = package_file(&p.bootloader, None)?;
        let base = package_file(&p.base, None)?;
        let test = match &p.factory_test {
            Some(t) => image::code(&package_file(t, None)?).to_vec(),
            None => vec![0; 512],
        };
        let mut volume = package_file(&p.bulk, Some("FileSys"))?;
        if let Some(dir) = content {
            ribbit_twist::fat::add_host_folder(&mut volume, dir)
                .map_err(|e| anyhow::anyhow!("adding the content folder {}: {e}", dir.display()))?;
        }
        let mut t = machine::Twist::boot(image::code(&boot), image::code(&base), &test, volume);
        let slice = machine::CLOCK / 120;
        let mut pixels = vec![0u32; ribbit_twist::SCREEN_W * ribbit_twist::SCREEN_H];
        // The console's time follows the clock from `origin` (moved on
        // while paused, or when the machine falls too far behind).
        let mut origin = Instant::now();
        let mut origin_now = 0u64;
        let mut shown = u64::MAX;
        let mut turned = [0i32; 4];
        let mut resample = crate::zippity::Resampler::new(ribbit_twist::sound::OUTPUT_RATE);
        let mut mono = Vec::new();
        while !s.stop.load(Ordering::Relaxed) {
            if !s.running.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                origin = Instant::now();
                origin_now = t.bus.now;
                continue;
            }
            let due = origin_now + (origin.elapsed().as_secs_f64() * machine::CLOCK as f64) as u64;
            if t.bus.now >= due {
                std::thread::sleep(Duration::from_millis(2));
                continue;
            }
            if due - t.bus.now > machine::CLOCK / 4 {
                origin = Instant::now();
                origin_now = t.bus.now;
            }
            t.bus.keys = s.keys.load(Ordering::Relaxed);
            for (r, (sent, total)) in turned.iter_mut().zip(&s.turned).enumerate() {
                let total = total.load(Ordering::Relaxed);
                t.bus.turn(r, total.wrapping_sub(*sent));
                *sent = total;
            }
            t.bus.slider_pads = s.slider_pads.load(Ordering::Relaxed) as u8;
            t.run(slice);
            // The picture changes only when the panel is sent a frame.
            if t.bus.panel_frames != shown {
                shown = t.bus.panel_frames;
                t.bus.screen(&mut pixels);
                s.screen.lock().unwrap().copy_from_slice(&pixels);
            }
            s.now.store(t.bus.now, Ordering::Relaxed);
            // The speaker: the sound unit with the DAC (`ribbit_twist::sound`).
            mono.clear();
            resample.push(&std::mem::take(&mut t.bus.sound), &mut mono);
            let gain = f32::from_bits(s.gain.load(Ordering::Relaxed));
            if let Some(sink) = s.sink.lock().unwrap().as_mut() {
                sink.push_32k(&mono, gain);
            }
        }
        Ok(())
    }

    /// The buttons held, as `machine::key` bits.
    pub fn set_keys(&self, keys: u32) {
        self.shared.keys.store(keys, Ordering::Relaxed);
    }

    /// How fast the console runs against real time (100: as the real
    /// one), measured since this was last asked.
    pub fn speed_percent(&self) -> f64 {
        let now = self.shared.now.load(Ordering::Relaxed);
        let mut last = self.speed_from.lock().unwrap();
        let (at, then) = *last;
        let real = at.elapsed().as_secs_f64();
        if real < 0.5 {
            return self.speed_shown.load(Ordering::Relaxed) as f64 / 10.0;
        }
        *last = (Instant::now(), now);
        let pct = (now - then) as f64 / ribbit_twist::machine::CLOCK as f64 / real * 100.0;
        self.speed_shown.store((pct * 10.0) as u64, Ordering::Relaxed);
        pct
    }

    /// The rotary controls' detents so far (`input::Rotary` order) and
    /// where a finger is on the Slider (0 = top, 1 = bottom).
    pub fn set_turns_and_slider(&self, turned: [i32; 4], slider: Option<f32>) {
        for (a, n) in self.shared.turned.iter().zip(turned) {
            a.store(n, Ordering::Relaxed);
        }
        let pads = ribbit_twist::machine::encoder::pads(slider);
        self.shared.slider_pads.store(pads.into(), Ordering::Relaxed);
    }

    /// The volume (0 mutes).
    pub fn set_gain(&self, gain: f32) {
        self.shared.gain.store(gain.to_bits(), Ordering::Relaxed);
    }

    pub fn set_running(&self, on: bool) {
        self.shared.running.store(on, Ordering::Relaxed);
    }

    pub fn error(&self) -> Option<String> {
        self.shared.error.lock().unwrap().clone()
    }

    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clone_from(&self.shared.screen.lock().unwrap());
    }
}

impl Drop for Machine {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// A RockIt Twist session: the controls, and the console running when its
/// system packages were found.
pub struct TwistSession {
    pub title: String,
    /// The package opened (None: the console on its own).
    pub package: Option<PathBuf>,
    pub kind: Option<Kind>,
    pub controls: Controls,
    pub started: u64,
    last_tick: Instant,
    /// Keys held down, oldest first (letting go of one hands its control
    /// back to another key still held for it).
    keys_down: Vec<KeyCode>,
    /// The system packages it starts from, and the console (None: they
    /// were not found).
    pub system: Option<SystemPackages>,
    pub machine: Option<Machine>,
    /// A folder written onto the storage at power-on (`Settings::twist_content`).
    pub content: Option<PathBuf>,
}

impl TwistSession {
    /// A session; `known` lists the library's RockIt Twist files, among
    /// which (and next to `package`) the system packages are looked for.
    pub fn start(package: Option<PathBuf>, known: &[PathBuf], content: Option<PathBuf>, audio: Option<&AudioOut>) -> Self {
        let system = SystemPackages::find(package.as_deref(), known);
        let machine = system.clone().map(|s| Machine::start(s, content.clone(), audio.map(|a| a.sink())));
        let kind = package.as_deref().and_then(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            package::kind(&name, &head(p)?)
        });
        let title = match &package {
            Some(p) => describe_package(p).map_or_else(|| "RockIt Twist".to_string(), |g| g.title),
            None => "RockIt Twist".to_string(),
        };
        Self {
            title,
            package,
            kind,
            controls: Controls::new(),
            started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            last_tick: Instant::now(),
            keys_down: Vec::new(),
            system,
            machine,
            content,
        }
    }

    /// Powers the console off and on again.
    /// Buttons still held stay held: held at power-on, some start other
    /// modes (the factory test, see docs/rockit-twist.md).
    pub fn restart(&mut self, audio: Option<&AudioOut>) {
        self.machine = None;
        self.machine = self.system.clone().map(|s| Machine::start(s, self.content.clone(), audio.map(|a| a.sink())));
        if let Some(m) = &self.machine {
            m.set_keys(self.keys());
        }
    }

    /// The buttons held, as the console reads them (`machine::key` bits).
    fn keys(&self) -> u32 {
        self.controls.held_buttons().into_iter().map(ribbit_twist::machine::key::of).fold(0, |k, b| k | b)
    }

    /// Advances held turns and swipes (once per frame).
    pub fn tick(&mut self) {
        let now = Instant::now();
        // Capped: a stalled frame must not spin a held dial far ahead.
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        self.controls.tick(dt);
        if let Some(m) = &self.machine {
            // As wired: the buttons (`machine::key`), the rotary encoders
            // and the Slider's pads (`machine::encoder`).
            m.set_keys(self.keys());
            m.set_turns_and_slider(Rotary::ALL.map(|r| self.controls.turned(r)), self.controls.slider());
        }
    }

    /// A key went down or up; false if the RockIt Twist does not use it.
    pub fn key(&mut self, code: KeyCode, pressed: bool) -> bool {
        let Some(k) = key(code) else {
            return false;
        };
        self.keys_down.retain(|&c| c != code);
        if pressed {
            self.keys_down.push(code);
        }
        // The newest key still held for the same control, if any.
        let other =
            |same: &dyn Fn(TwistKey) -> bool| self.keys_down.iter().rev().filter_map(|&c| key(c)).find(|&t| same(t));
        match k {
            TwistKey::Press(b) => {
                let still = other(&|t| t == TwistKey::Press(b)).is_some();
                self.controls.set(Source::Keyboard, b, pressed || still);
            }
            TwistKey::Turn(r, s) => {
                // Held: this way at once. Let go: back to another key held
                // for it (U let go while J is held turns down again).
                let step = if pressed {
                    Some(s)
                } else {
                    match other(&|t| matches!(t, TwistKey::Turn(r2, _) if r2 == r)) {
                        Some(TwistKey::Turn(_, s2)) => Some(s2),
                        _ => None,
                    }
                };
                self.controls.hold_turn(Source::Keyboard, r, step);
            }
            TwistKey::Swipe(up) => {
                if pressed {
                    self.controls.swipe(up);
                }
            }
        }
        true
    }

    /// Lets go of every key (the window lost focus).
    pub fn release_keys(&mut self) {
        self.keys_down.clear();
        self.controls.release(Source::Keyboard);
    }

    /// The picture: the console's screen, or dark when it is not running.
    pub fn frame(&self, out: &mut Vec<u32>) {
        match &self.machine {
            Some(m) => m.frame(out),
            None => {
                out.clear();
                out.resize(ribbit_twist::SCREEN_W * ribbit_twist::SCREEN_H, 0);
            }
        }
    }
}

fn head(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut b = [0u8; 4];
    std::fs::File::open(p).ok()?.read_exact(&mut b).ok()?;
    Some(b.to_vec())
}

/// What the screen says during a session (drawn over the dark screen; it
/// is Ribbit's own text, not the console's).
pub fn overlay(painter: &egui::Painter, screen: egui::Rect, s: &TwistSession) {
    let c = &s.controls;
    let size = (screen.height() / 16.0).clamp(9.0, 18.0);
    let font = egui::FontId::proportional(size);
    let small = egui::FontId::proportional(size * 0.8);
    if let Some(m) = &s.machine {
        // The console's own picture; words only if it could not start.
        let Some(e) = m.error() else {
            return;
        };
        let p = painter.with_clip_rect(screen);
        let mut y = screen.top() + size * 0.8;
        for (text, f, color) in [
            ("The RockIt Twist couldn't start".to_string(), font, egui::Color32::from_rgb(250, 190, 90)),
            (e, small, egui::Color32::from_gray(210)),
        ] {
            let g = p.layout(text, f, color, screen.width() - size * 1.6);
            p.galley(egui::pos2(screen.left() + size * 0.8, y), g.clone(), color);
            y += g.size().y + size * 0.25;
        }
        return;
    }
    let mut lines: Vec<(String, egui::FontId, egui::Color32)> = vec![
        ("RockIt Twist: system packages not found".into(), font.clone(), egui::Color32::from_rgb(140, 230, 90)),
        (
            "To start the console, Ribbit needs its Bootloader, Base and Firmware-Bulk packages (.lfp) in a games              folder. Until then, only the controls work here."
                .into(),
            small.clone(),
            egui::Color32::from_gray(200),
        ),
    ];
    match s.kind {
        Some(Kind::EncryptedPack) => lines.push((
            "This game pack is encrypted; Ribbit cannot read it.".into(),
            small.clone(),
            egui::Color32::from_rgb(250, 190, 90),
        )),
        Some(Kind::System) => {
            lines.push(("System package: firmware research only.".into(), small.clone(), egui::Color32::from_gray(200)))
        }
        None => {}
    }
    if let Some(name) = s.package.as_deref().and_then(Path::file_name) {
        lines.push((format!("File: {}", name.to_string_lossy()), small.clone(), egui::Color32::from_gray(160)));
    }
    lines.push((String::new(), small.clone(), egui::Color32::WHITE));
    let held: Vec<String> = c.held_buttons().iter().map(|b| b.label()).collect();
    lines.push((
        format!("Held: {}", if held.is_empty() { "nothing".to_string() } else { held.join(", ") }),
        small.clone(),
        egui::Color32::WHITE,
    ));
    let turns: Vec<String> = Rotary::ALL.iter().map(|&r| format!("{} {:+}", r.label(), c.turned(r))).collect();
    lines.push((format!("Turned: {}", turns.join("  ")), small.clone(), egui::Color32::WHITE));
    lines.push((
        format!(
            "Slider: {}",
            c.slider().map_or("not touched".to_string(), |p| format!("finger at {:.0}% from the top", p * 100.0))
        ),
        small,
        egui::Color32::WHITE,
    ));
    let p = painter.with_clip_rect(screen);
    let mut y = screen.top() + size * 0.8;
    for (text, f, color) in lines {
        let g = p.layout(text, f, color, screen.width() - size * 1.6);
        p.galley(egui::pos2(screen.left() + size * 0.8, y), g.clone(), color);
        y += g.size().y + size * 0.25;
    }
}

/// The parsed `package-catalog.txt` (ID -> name) that archives of these
/// packages come with, searched for in the package's folder and the three
/// above it.
fn catalog_name(path: &Path, id: &str) -> Option<String> {
    let mut dir = path.parent();
    for _ in 0..4 {
        let d = dir?;
        if let Ok(text) = std::fs::read_to_string(d.join("package-catalog.txt")) {
            let mut name: Option<&str> = None;
            for line in text.lines() {
                let t = line.trim();
                if let Some(found) = t.strip_prefix("ID: ") {
                    if found == id {
                        return name.map(|n| {
                            n.replace("RockIt Twist™ Game Pack: ", "").replace("Rockit Twist™ Game Pack: ", "")
                        });
                    }
                } else if !t.is_empty() && !t.starts_with('-') && !t.starts_with('=') {
                    name = Some(t);
                }
            }
            return None;
        }
        dir = d.parent();
    }
    None
}

/// A library entry for a RockIt Twist package, or None if `path` is not one.
pub fn describe_package(path: &Path) -> Option<Game> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".lfp") && !lower.ends_with(".lf3") {
        return None;
    }
    let kind = package::kind(&name, &head(path)?)?;
    let size = std::fs::metadata(path).ok()?.len();
    let id = package::package_id(&name).map(str::to_string);
    // The locale is the folder name in the archive's layout (en-us/<ID>/<version>/).
    let locale = path
        .ancestors()
        .filter_map(|a| a.file_name()?.to_str())
        .find(|n| n.len() == 5 && n.as_bytes()[2] == b'-')
        .map(str::to_string);
    let mut details = vec![("System", "RockIt Twist".to_string())];
    let (title, what, tag) = match kind {
        Kind::System => {
            let meta = std::fs::read(path)
                .ok()
                .and_then(|d| ribbit_core::zip::extract_named(&d, "Firmware/meta.inf").ok())
                .map(|m| package::Meta::parse(&String::from_utf8_lossy(&m)))
                .unwrap_or_default();
            for (label, key) in [
                ("Name", "Name"),
                ("Version", "Version"),
                ("Type", "Type"),
                ("Binary", "BinFile"),
                ("Built", "BuildDate"),
            ] {
                if let Some(v) = meta.get(key) {
                    details.push((label, v.to_string()));
                }
            }
            let title = format!("RockIt Twist system: {}", meta.get("Name").unwrap_or("package"));
            (title, "System package (firmware)", "System")
        }
        Kind::EncryptedPack => {
            let title = id.as_deref().and_then(|i| catalog_name(path, i)).unwrap_or_else(|| name.clone());
            details.push(("Contents", "encrypted (LF3): cannot be read".to_string()));
            (title, "Encrypted game pack", "Encrypted")
        }
    };
    if let Some(i) = &id {
        details.push(("Package", i.clone()));
    }
    if let Some(l) = &locale {
        details.push(("Locale", l.clone()));
    }
    let mb = size.div_ceil(1 << 20);
    details.push(("Size", format!("{mb} MB")));
    details.push((
        "Status",
        "early emulation: the console starts and shows its own screens; games cannot run yet".to_string(),
    ));
    details.push(("File", path.display().to_string()));
    let subtitle = [Some(what.to_string()), locale.clone(), Some(format!("{mb} MB"))]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ");
    let key = format!("twist-{}-{}-{size}", id.as_deref().unwrap_or(&name), locale.as_deref().unwrap_or(""));
    Some(Game {
        path: path.to_path_buf(),
        family: Family::RockItTwist,
        title,
        subtitle,
        details,
        tags: vec![tag],
        key,
        art: Art::Missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_has_a_key_and_no_key_does_two_things() {
        for (i, (k, _)) in KEYS.iter().enumerate() {
            assert!(KEYS[i + 1..].iter().all(|(o, _)| o != k), "{k:?} is bound twice");
        }
        for b in Button::ALL.into_iter().filter(|&b| b != Button::Power) {
            assert!(KEYS.iter().any(|(_, t)| *t == TwistKey::Press(b)), "{b:?} has no key");
        }
        for r in Rotary::ALL {
            for s in [-1, 1] {
                assert!(KEYS.iter().any(|(_, t)| *t == TwistKey::Turn(r, s)), "{r:?} {s} has no key");
            }
        }
        assert_eq!(keys_for(TwistKey::Press(Button::DpadUp)), "W / Up");
        assert_eq!(keys_for(TwistKey::Turn(Rotary::Spinner, -1)), "Num *");
        assert_eq!(keys_for(TwistKey::Press(Button::Grid(0))), "Num 7");
        assert_eq!(cap_for(TwistKey::Press(Button::Grid(0))).as_deref(), Some("7"));
        assert_eq!(cap_for(TwistKey::Turn(Rotary::RedDial, -1)).as_deref(), Some("L"));
        assert_eq!(cap_for(TwistKey::Swipe(true)).as_deref(), Some("PgUp"));
        assert_eq!(cap_for(TwistKey::Press(Button::Power)), None);
    }

    #[test]
    fn keys_press_and_release_and_turn() {
        let mut s = TwistSession::start(None, &[], None, None);
        assert!(s.machine.is_none(), "no system packages: controls only");
        assert!(s.key(KeyCode::KeyE, true));
        assert!(s.controls.pressed(Button::SwitchRight));
        assert!(s.key(KeyCode::KeyE, false));
        assert!(!s.controls.pressed(Button::SwitchRight));
        assert!(s.key(KeyCode::KeyU, true));
        assert_eq!(s.controls.turned(Rotary::YellowDial), 1);
        s.key(KeyCode::KeyU, false);
        assert!(s.key(KeyCode::PageDown, true));
        assert!(s.controls.slider().is_some());
        assert!(!s.key(KeyCode::F12, true), "not a RockIt Twist key");
        // Switching quickly: J pressed before U is let go keeps turning
        // down, with no pause.
        s.key(KeyCode::KeyU, true);
        s.key(KeyCode::KeyJ, true);
        assert_eq!(s.controls.turning(Rotary::YellowDial), Some(-1));
        s.key(KeyCode::KeyU, false);
        assert_eq!(s.controls.turning(Rotary::YellowDial), Some(-1), "J still held");
        s.key(KeyCode::KeyJ, false);
        assert_eq!(s.controls.turning(Rotary::YellowDial), None);
        // Two keys for one button: letting go of one keeps it held.
        s.key(KeyCode::KeyW, true);
        s.key(KeyCode::ArrowUp, true);
        s.key(KeyCode::KeyW, false);
        assert!(s.controls.pressed(Button::DpadUp));
        s.key(KeyCode::ArrowUp, false);
        assert!(!s.controls.pressed(Button::DpadUp));
    }

    #[test]
    fn the_system_packages_are_found_next_to_a_game_pack_in_its_locale() {
        let dir = std::env::temp_dir().join(format!("ribbit-twist-sys-{}", std::process::id()));
        let put = |locale: &str, id: &str, version: &str, ext: &str| {
            let d = dir.join(locale).join(id).join(version);
            std::fs::create_dir_all(&d).unwrap();
            let f = d.join(format!("{id}.{ext}"));
            std::fs::write(&f, b"PK").unwrap();
            f
        };
        let pack = put("en-us", "PCKT-0x0050000E-000000", "1.1.3.21", "lf3");
        let boot = put("en-us", BOOTLOADER_ID, "1.0.48.0", "lfp");
        let base_old = put("en-us", BASE_ID, "1.0.49.0", "lfp");
        let base = put("en-us", BASE_ID, "1.0.50.0", "lfp");
        let bulk = put("en-us", BULK_ID, "1.0.44.0", "lfp");
        let other_bulk = put("fr-fr", BULK_ID, "1.0.99.0", "lfp");
        let found = SystemPackages::find(Some(&pack), &[other_bulk.clone(), base_old]).unwrap();
        assert_eq!(found, SystemPackages { bootloader: boot, base, bulk, factory_test: None });
        // Without a pack to go by, the library's list is used.
        assert_eq!(SystemPackages::find(None, &[other_bulk]), None, "no bootloader listed");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unpacked_system_packages_work_like_lfp_files() {
        let dir = std::env::temp_dir().join(format!("ribbit-twist-unpacked-{}", std::process::id()));
        for (id, bin) in [(BOOTLOADER_ID, "Boot"), (BASE_ID, "System"), (BULK_ID, "FileSys")] {
            let fw = dir.join("system").join(id).join("Firmware");
            std::fs::create_dir_all(&fw).unwrap();
            std::fs::write(fw.join("meta.inf"), format!("BinFile=\"{bin}\"\r\n")).unwrap();
            std::fs::write(fw.join(bin), bin.as_bytes()).unwrap();
        }
        let known = packages_in(&dir.join("system"));
        let found = SystemPackages::find(None, &known).unwrap();
        assert!(found.base.ends_with(BASE_ID));
        assert_eq!(package_file(&found.base, None).unwrap(), b"System");
        assert_eq!(package_file(&found.bulk, Some("FileSys")).unwrap(), b"FileSys");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A zip of stored (uncompressed) files.
    fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            // CRC-32 (the zip's), bit by bit.
            let crc = !data.iter().fold(!0u32, |c, &b| {
                (0..8).fold(c ^ b as u32, |c, _| if c & 1 != 0 { c >> 1 ^ 0xEDB8_8320 } else { c >> 1 })
            });
            let local = out.len() as u32;
            let sizes = [crc, data.len() as u32, data.len() as u32];
            out.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            sizes.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            sizes.iter().for_each(|v| dir.extend_from_slice(&v.to_le_bytes()));
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&local.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn a_zip_of_packages_is_unpacked_for_one_region() {
        let meta = |t: &str, l: &str| {
            format!(
                "Type=\"{t}\"
Locale=\"{l}\"
Name=\"{t} {l}\"
"
            )
        };
        let (app_us, music_us, app_gb, game) = (
            meta("Application", "en-us"),
            meta("Music", "en-us"),
            meta("Application", "en-gb"),
            meta("Application", "en-us"),
        );
        let zip = stored_zip(&[
            (r"Games\A\Bundle\meta.inf", app_us.as_bytes()),
            (r"Games\A\Bundle\main.bin", b"MENU"),
            (r"Games\A\Bundle\fapp\Bunny.bin", b"BUNNY"),
            ("Games/M/Bundle/meta.inf", music_us.as_bytes()),
            ("Games/M/Bundle/song.mp3", b"SONG"),
            ("Games/G/Bundle/meta.inf", app_gb.as_bytes()),
            ("Games/G/Bundle/main.bin", b"UK MENU"),
            ("Games/PCKT-0x0050000E-000000/PCKT-0x0050000E-000000/meta.inf", game.as_bytes()),
            ("Games/PCKT-0x0050000E-000000/PCKT-0x0050000E-000000/x.bin", b"DINO"),
        ]);
        let dir = std::env::temp_dir().join(format!("ribbit-twist-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("games.zip");
        std::fs::write(&file, zip).unwrap();
        let into = dir.join("content");
        std::fs::create_dir_all(into.join("stale")).unwrap();
        let done = import_zip(&file, &into, None).unwrap();
        assert_eq!((done.locale.as_str(), done.files, done.packs.len()), ("en-us", 7, 3));
        let read = |p: &str| std::fs::read(into.join(p)).ok();
        assert_eq!(read("Applications/Bundle/main.bin").as_deref(), Some(&b"MENU"[..]), "en-us, not the UK menu");
        assert_eq!(read("Applications/Bundle/fapp/Bunny.bin").as_deref(), Some(&b"BUNNY"[..]));
        assert_eq!(read("Music/Bundle/song.mp3").as_deref(), Some(&b"SONG"[..]));
        assert_eq!(read("Applications/PCKT-0x0050000E-000000/x.bin").as_deref(), Some(&b"DINO"[..]));
        assert!(!into.join("stale").exists(), "emptied first");
        assert_eq!(import_zip(&file, &into, Some("en-gb")).unwrap().locale, "en-gb");
        assert_eq!(read("Applications/Bundle/main.bin").as_deref(), Some(&b"UK MENU"[..]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Boots as a session does, from Ribbit's data folder (system packages
    /// and content), for RIBBIT_TWIST_SECONDS emulated seconds, and prints
    /// where it got to: RIBBIT_TWIST_DATA=<data folder> cargo test
    /// --release -p ribbit-frontend boot_like_a_session -- --ignored
    #[test]
    #[ignore]
    fn boot_like_a_session() {
        use ribbit_twist::{image, machine};
        let data = PathBuf::from(std::env::var("RIBBIT_TWIST_DATA").expect("RIBBIT_TWIST_DATA"));
        let secs: u64 = std::env::var("RIBBIT_TWIST_SECONDS").ok().and_then(|s| s.parse().ok()).unwrap_or(25);
        let p = SystemPackages::find(None, &packages_in(&system_dir(&data))).expect("system packages");
        let boot = package_file(&p.bootloader, None).unwrap();
        let base = package_file(&p.base, None).unwrap();
        let test = image::code(&package_file(p.factory_test.as_ref().unwrap(), None).unwrap()).to_vec();
        let mut volume = package_file(&p.bulk, Some("FileSys")).unwrap();
        let started = Instant::now();
        let n = ribbit_twist::fat::add_host_folder(&mut volume, &data.join("rockit-twist").join("content")).unwrap();
        println!("content: {n} files in {:?}", started.elapsed());
        let mut t = machine::Twist::boot(image::code(&boot), image::code(&base), &test, volume);
        println!("clock functions {:X?}", t.clock_fns);
        let started = Instant::now();
        for _ in 0..secs * 120 {
            t.run(machine::CLOCK / 120);
        }
        println!(
            "{secs} s emulated in {:?}; pc {:08X}; {} frames drawn, {} sent; screen source {:X?}",
            started.elapsed(),
            t.cpu.r[15],
            t.bus.frames_drawn,
            t.bus.panel_frames,
            t.bus.panel_source
        );
    }

    /// Imports a real zip: RIBBIT_TWIST_ZIP=<zip> RIBBIT_TWIST_INTO=<folder>
    /// cargo test -p ribbit-frontend import_a_real_zip -- --ignored
    #[test]
    #[ignore]
    fn import_a_real_zip() {
        let zip = std::env::var("RIBBIT_TWIST_ZIP").expect("RIBBIT_TWIST_ZIP");
        let into = std::env::var("RIBBIT_TWIST_INTO").expect("RIBBIT_TWIST_INTO");
        let done = import_zip(Path::new(&zip), Path::new(&into), None).unwrap();
        println!("{} files, {}: {:?}", done.files, done.locale, done.packs);
    }

    #[test]
    fn packages_are_listed_by_their_catalog_names() {
        let dir = std::env::temp_dir().join(format!("ribbit-twist-{}", std::process::id()));
        let pack = dir.join("en-us").join("PCKT-0x0050000E-000000").join("1.1.3.21");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            dir.join("package-catalog.txt"),
            "Package Catalog\n===\n\nen-us\n-----\n\nRockIt Twist™ Game Pack: Dinosaur Discoveries™\n  ID: PCKT-0x0050000E-000000\n",
        )
        .unwrap();
        let file = pack.join("PCKT-0x0050000E-000000.lf3");
        std::fs::write(&file, [0x31, 0x0A, 0x7F, 0x9C, 1, 2, 3]).unwrap();
        let g = describe_package(&file).unwrap();
        assert_eq!(g.title, "Dinosaur Discoveries™");
        assert_eq!(g.family, Family::RockItTwist);
        assert_eq!(g.tags, ["Encrypted"]);
        assert!(g.subtitle.contains("en-us"));
        assert!(describe_package(&dir.join("package-catalog.txt")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
