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
use std::collections::BTreeMap;
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

/// The player's own keys (`Settings::twist_keys`): action name (`action_name`)
/// to key name. An action given a key here uses only that key; the key no
/// longer does what it did by default.
static BINDINGS: std::sync::RwLock<BTreeMap<String, String>> = std::sync::RwLock::new(BTreeMap::new());

/// Sets the player's keys (from the settings).
pub fn set_bindings(b: &BTreeMap<String, String>) {
    *BINDINGS.write().unwrap() = b.clone();
}

/// An action's name in the settings file ("Press(DpadUp)", "Turn(YellowDial, 1)").
pub fn action_name(t: TwistKey) -> String {
    format!("{t:?}")
}

/// The actions keys can be given, in the bindings dialog's order.
pub fn actions() -> Vec<TwistKey> {
    let mut v: Vec<TwistKey> = Vec::new();
    for &(_, t) in KEYS {
        if !v.contains(&t) {
            v.push(t);
        }
    }
    v
}

/// What an action is called in the bindings dialog.
pub fn action_label(t: TwistKey) -> String {
    match t {
        TwistKey::Press(b) => match b {
            Button::Grid(n) => format!("Grid button {}", n + 1),
            Button::DpadUp => "D-pad up".into(),
            Button::DpadDown => "D-pad down".into(),
            Button::DpadLeft => "D-pad left".into(),
            Button::DpadRight => "D-pad right".into(),
            Button::SwitchLeft => "Switch left".into(),
            Button::SwitchRight => "Switch right".into(),
            Button::Check => "Check mark".into(),
            Button::VolumeUp => "Volume up".into(),
            Button::VolumeDown => "Volume down".into(),
            b => format!("{b:?}"),
        },
        TwistKey::Turn(r, s) => match r {
            Rotary::Spinner => format!("Spinner {}", if s > 0 { "right" } else { "left" }),
            r => format!("{} {}", r.label(), if s > 0 { "up" } else { "down" }),
        },
        TwistKey::Swipe(up) => format!("Slider swipe {}", if up { "up" } else { "down" }),
    }
}

/// Every (key name, action), the player's keys first.
fn keymap() -> Vec<(String, TwistKey)> {
    let b = BINDINGS.read().unwrap();
    let mut out: Vec<(String, TwistKey)> = Vec::new();
    for t in actions() {
        if let Some(k) = b.get(&action_name(t)) {
            out.push((k.clone(), t));
        }
    }
    for &(k, t) in KEYS {
        let name = crate::settings::key_name(k);
        if !b.contains_key(&action_name(t)) && !out.iter().any(|(c, _)| *c == name) {
            out.push((name, t));
        }
    }
    out
}

pub fn key(code: KeyCode) -> Option<TwistKey> {
    let name = crate::settings::key_name(code);
    keymap().into_iter().find(|(k, _)| *k == name).map(|(_, t)| t)
}

/// The first key for an action, as its keycap shows it.
pub fn cap_for(what: TwistKey) -> Option<String> {
    keymap().into_iter().find(|(_, t)| *t == what).map(|(k, _)| crate::settings::cap_text(&k))
}

/// The keys for a control, for tooltips ("W / Up").
pub fn keys_for(what: TwistKey) -> String {
    let names: Vec<String> = keymap().into_iter().filter(|(_, t)| *t == what).map(|(k, _)| key_label(&k)).collect();
    names.join(" / ")
}

/// A game controller on the RockIt Twist (the app's `gamepad` events):
/// D-pad or left stick = D-pad, A = check mark, B = Hint, Y = Home, X = grid
/// button 5 (the middle), LB / RB = Switch left / right, LT / RT = Spinner
/// left / right, right stick up / down = yellow dial, left / right = green
/// dial, Back / Start = red dial down / up.
#[cfg(not(target_os = "android"))]
pub fn pad_button(b: gilrs::Button) -> Option<TwistKey> {
    use gilrs::Button as P;
    Some(match b {
        P::DPadUp => TwistKey::Press(Button::DpadUp),
        P::DPadDown => TwistKey::Press(Button::DpadDown),
        P::DPadLeft => TwistKey::Press(Button::DpadLeft),
        P::DPadRight => TwistKey::Press(Button::DpadRight),
        P::South => TwistKey::Press(Button::Check),
        P::East => TwistKey::Press(Button::Hint),
        P::North => TwistKey::Press(Button::Home),
        P::West => TwistKey::Press(Button::Grid(4)),
        P::LeftTrigger => TwistKey::Press(Button::SwitchLeft),
        P::RightTrigger => TwistKey::Press(Button::SwitchRight),
        P::LeftTrigger2 => TwistKey::Turn(Rotary::Spinner, -1),
        P::RightTrigger2 => TwistKey::Turn(Rotary::Spinner, 1),
        P::Select => TwistKey::Turn(Rotary::RedDial, -1),
        P::Start => TwistKey::Turn(Rotary::RedDial, 1),
        _ => return None,
    })
}

fn key_label(n: &str) -> String {
    match n {
        "NumpadMultiply" => "Num *".into(),
        "NumpadSubtract" => "Num -".into(),
        "NumpadEnter" => "Num Enter".into(),
        "Equal" => "=".into(),
        "Minus" => "-".into(),
        n => match n.strip_prefix("Numpad") {
            Some(d) => format!("Num {d}"),
            None => crate::settings::key_label(n),
        },
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
    /// USB news for the UI.
    usb_news: Mutex<Option<UsbNews>>,
    /// Requests from the UI, taken by the console's thread.
    ops: Mutex<std::sync::mpsc::Receiver<Op>>,
    gain: AtomicU32,
}

/// The console running on its own thread, in real time.
pub struct Machine {
    shared: Arc<Shared>,
    /// Requests for the console's thread (save states).
    ops: std::sync::mpsc::Sender<Op>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// For the speed shown: when it was last measured, the console's time
    /// then, and the figure (tenths of a percent).
    speed_from: Mutex<(Instant, u64)>,
    speed_shown: AtomicU64,
}

impl Machine {
    /// `content`: a folder written onto the storage (the volume's root).
    /// `save`: the file holding what the console stored (profiles,
    /// progress), loaded at power-on and written as it changes.
    /// `usb`: offer the console on the PC's USB (`run_usb`).
    pub fn start(
        packages: SystemPackages,
        content: Option<PathBuf>,
        sink: Option<AudioSink>,
        save: Option<PathBuf>,
        usb: bool,
    ) -> Self {
        let (ops, ops_rx) = std::sync::mpsc::channel();
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
            usb_news: Mutex::new(None),
            ops: Mutex::new(ops_rx),
            gain: AtomicU32::new(1f32.to_bits()),
        });
        let s = shared.clone();
        let thread = std::thread::Builder::new()
            .name("rockit-twist".into())
            .spawn(move || {
                crate::perf::raise_thread_priority();
                if let Err(e) = Self::run(&packages, content.as_deref(), save.as_deref(), usb, &s) {
                    *s.error.lock().unwrap() = Some(format!("{e:#}"));
                }
            })
            .ok();
        Self { shared, ops, thread, speed_from: Mutex::new((Instant::now(), 0)), speed_shown: AtomicU64::new(0) }
    }

    fn run(p: &SystemPackages, content: Option<&Path>, save: Option<&Path>, usb: bool, s: &Shared) -> anyhow::Result<()> {
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
        let key = volume_key(&volume);
        let mut t = machine::Twist::boot(image::code(&boot), image::code(&base), &test, volume);
        // What the console stored last time, if the storage is built the
        // same way (the same packages and content).
        // A save for storage built differently (other content, or a Ribbit
        // that lays the storage out differently) can't be applied; it is
        // kept aside rather than written over by the new one.
        let mut saved = match save {
            Some(p) if p.exists() => load_save(p, key).unwrap_or_else(|| {
                crate::backup::set_aside(p, "other-storage");
                Vec::new()
            }),
            _ => Vec::new(),
        };
        if let Some(card) = &mut t.bus.emmc {
            for (n, d) in &saved {
                card.write_sector(*n, d);
            }
        }
        let mut save_check = Instant::now();
        let server = usb.then(|| match ribbit_twist::usbip::Server::start(usb_device(), ribbit_twist::usbip::PORT) {
            Ok(s) => Some(s),
            Err(e) => {
                *s.usb_news.lock().unwrap() = Some(UsbNews::Failed(e.to_string()));
                None
            }
        });
        let server = server.flatten();
        let mut trace = UsbTrace::default();
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
                tracing::info!(target: "ribbit::twist", "the console fell 0.25 s behind real time; it goes on from now");
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
            if let Some(srv) = &server {
                Self::run_usb(srv, &mut t, s, &mut trace);
            }
            while let Ok(op) = s.ops.lock().unwrap().try_recv() {
                match op {
                    Op::Save(reply) => {
                        let mut state = key.to_le_bytes().to_vec();
                        state.extend(t.save_state());
                        let _ = reply.send(state);
                    }
                    Op::Load(state, reply) => {
                        let result = if state.get(..8).is_some_and(|k| k == key.to_le_bytes()) {
                            t.load_state(&state[8..])
                        } else {
                            Err("this state is from a RockIt Twist with other content".into())
                        };
                        if result.is_ok() {
                            // Its clock starts from the state's time.
                            origin = Instant::now();
                            origin_now = t.bus.now;
                            shown = u64::MAX;
                        }
                        let _ = reply.send(result);
                    }
                }
            }
            let ran = Instant::now();
            t.run(slice);
            // Too heavy to run at full speed: the processor slows, not the
            // whole console, so the sound plays on without gaps.
            t.stall_for(slice, ran.elapsed().as_secs_f64());
            if let Some(srv) = &server {
                Self::run_usb(srv, &mut t, s, &mut trace);
            }
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
            // A comfortable level: the sound unit's mix comes out quiet (its
            // loudness on a real unit is UNVERIFIED), so it is raised, peaks
            // eased in rather than clipped.
            for v in &mut mono {
                *v = (32767.0 * (*v as f32 * LOUDNESS / 32767.0).tanh()) as i16;
            }
            let gain = f32::from_bits(s.gain.load(Ordering::Relaxed));
            if let Some(sink) = s.sink.lock().unwrap().as_mut() {
                sink.push_32k(&mono, gain);
            }
            if save_check.elapsed() > Duration::from_secs(30) {
                save_check = Instant::now();
                store_save(save, key, &t, &mut saved);
            }
        }
        store_save(save, key, &t, &mut saved);
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

    /// Passes the PC's transfers to the console's USB controller and its
    /// answers back.
    fn run_usb(srv: &ribbit_twist::usbip::Server, t: &mut ribbit_twist::machine::Twist, s: &Shared, trace: &mut UsbTrace) {
        use ribbit_twist::usbip::Event;
        while let Some(e) = srv.next_event() {
            trace.event(&e, t.bus.now);
            match e {
                Event::Attached => {
                    t.bus.udc_attach(true);
                    *s.usb_news.lock().unwrap() = Some(UsbNews::Attached);
                }
                Event::Detached => {
                    if t.bus.udc.attached {
                        *s.usb_news.lock().unwrap() = Some(UsbNews::Detached);
                    }
                    t.bus.udc_attach(false);
                }
                Event::Submit(u) => t.bus.udc_submit(u),
                Event::Unlink(seq) => {
                    t.bus.udc_unlink(seq);
                }
            }
        }
        for (seq, status, data, length) in t.bus.udc_take_done() {
            trace.done(seq, status, &data, t.bus.now);
            srv.complete(seq, status, data, length);
        }
    }

    /// News of the USB link since last asked.
    pub fn take_usb_news(&self) -> Option<UsbNews> {
        self.shared.usb_news.lock().unwrap().take()
    }

    /// The console's state now, for a save slot.
    pub fn save_state(&self) -> Result<Vec<u8>, String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.ops.send(Op::Save(tx)).map_err(|_| "the console has stopped".to_string())?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the console didn't answer".to_string())
    }

    /// Restores a state from `save_state`.
    pub fn load_state(&self, state: Vec<u8>) -> Result<(), String> {
        let (tx, rx) = std::sync::mpsc::channel();
        self.ops.send(Op::Load(state, tx)).map_err(|_| "the console has stopped".to_string())?;
        rx.recv_timeout(Duration::from_secs(10)).map_err(|_| "the console didn't answer".to_string())?
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

/// A request for the console's thread.
enum Op {
    Save(std::sync::mpsc::Sender<Vec<u8>>),
    Load(Vec<u8>, std::sync::mpsc::Sender<Result<(), String>>),
}

/// How much the sound unit's mix is raised before the app's own volume.
const LOUDNESS: f32 = 4.0;

/// What the UI hears about the USB link.
pub enum UsbNews {
    Attached,
    Detached,
    Failed(String),
}

/// Writes what LeapFrog Connect and the console say to each other over USB
/// to the log (target `ribbit::twist::usb`): control requests, SCSI
/// commands, and LeapFrog's file protocol, whose commands are a WRITE of
/// block 0x20 and whose replies a READ of it (file data moves at block
/// 0x40 and up, logged only at debug level).
#[derive(Default)]
struct UsbTrace {
    /// The last command block's operation and first block.
    scsi: Option<(u8, u32)>,
    /// Control transfers waiting for their answer, by sequence number.
    control: BTreeMap<u32, [u8; 8]>,
}

impl UsbTrace {
    fn event(&mut self, e: &ribbit_twist::usbip::Event, now: u64) {
        use ribbit_twist::usbip::Event;
        let at = now as f64 / ribbit_twist::machine::CLOCK as f64;
        match e {
            Event::Attached => tracing::info!(target: "ribbit::twist::usb", "{at:.3} attached"),
            Event::Detached => tracing::info!(target: "ribbit::twist::usb", "{at:.3} detached"),
            Event::Unlink(seq) => tracing::info!(target: "ribbit::twist::usb", "{at:.3} unlink {seq}"),
            Event::Submit(u) if u.endpoint & 0x0F == 0 => {
                tracing::info!(target: "ribbit::twist::usb", "{at:.3} control {:02X?} out {:02X?}", u.setup, &u.data[..u.data.len().min(32)]);
                self.control.insert(u.seq, u.setup);
            }
            Event::Submit(u) if !u.dir_in => {
                let d = &u.data;
                if d.len() == 31 && d.starts_with(b"USBC") {
                    let cdb = &d[15..15 + (d[14] as usize).min(16)];
                    let lba = cdb.get(2..6).map_or(0, |b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]));
                    let blocks = cdb.get(7..9).map_or(0, |b| u16::from_be_bytes([b[0], b[1]]));
                    self.scsi = Some((cdb[0], lba));
                    match cdb[0] {
                        0x28 | 0x2A if lba >= 0x40 => tracing::debug!(target: "ribbit::twist::usb", "{at:.3} {} block {lba:#X} x{blocks}", if cdb[0] == 0x28 { "READ" } else { "WRITE" }),
                        // The file protocol's own blocks: the text says it.
                        0x28 | 0x2A if lba == 0x20 => {}
                        _ => tracing::info!(target: "ribbit::twist::usb", "{at:.3} SCSI {cdb:02X?}"),
                    }
                } else if self.scsi == Some((0x2A, 0x20)) {
                    tracing::info!(target: "ribbit::twist::usb", "{at:.3} -> {:?}", text(d));
                }
            }
            Event::Submit(_) => {}
        }
    }

    fn done(&mut self, seq: u32, status: i32, data: &[u8], now: u64) {
        let at = now as f64 / ribbit_twist::machine::CLOCK as f64;
        if let Some(setup) = self.control.remove(&seq) {
            tracing::info!(target: "ribbit::twist::usb", "{at:.3} control {:02X?} status {status}, in {:02X?}", setup, &data[..data.len().min(32)]);
        } else if data.len() == 13 && data.starts_with(b"USBS") {
            // Command status: 0 passed, 1 failed, 2 phase error.
            if data[12] != 0 || status != 0 {
                tracing::info!(target: "ribbit::twist::usb", "{at:.3} SCSI status {} (urb {status})", data[12]);
            }
        } else if self.scsi == Some((0x28, 0x20)) && !data.is_empty() {
            let reply = text(data);
            // "102 BUSY": asked again until the console has an answer.
            if !reply.starts_with("102") {
                tracing::info!(target: "ribbit::twist::usb", "{at:.3} <- {reply:?}");
            }
        } else if status != 0 {
            tracing::info!(target: "ribbit::twist::usb", "{at:.3} urb {seq} status {status}");
        }
    }
}

/// A file-protocol block as text (up to its first NUL).
fn text(d: &[u8]) -> String {
    let end = d.iter().position(|&b| b == 0).unwrap_or(d.len());
    String::from_utf8_lossy(&d[..end]).into_owned()
}

/// The device as the console's firmware describes itself over USB
/// (VERIFIED with examples/usbhost.rs: vendor 0x0F63, LeapFrog; product
/// 0x0047; one mass-storage interface, SCSI, Bulk-Only; high speed).
fn usb_device() -> ribbit_twist::usbip::DeviceInfo {
    ribbit_twist::usbip::DeviceInfo {
        busid: "1-1".into(),
        vendor: 0x0F63,
        product: 0x0047,
        bcd_device: 0x0100,
        class: (0, 0, 0),
        configuration: 1,
        num_configurations: 1,
        interfaces: vec![(8, 6, 0x50)],
        speed: 3,
    }
}

/// The console's stored sectors: "RTWS", the storage's key (`volume_key`),
/// the count, then each sector's number and 512 bytes.
fn load_save(path: &Path, key: u64) -> Option<Vec<(u64, [u8; 512])>> {
    let d = std::fs::read(path).ok()?;
    if d.get(..4)? != b"RTWS" || u64::from_le_bytes(d.get(4..12)?.try_into().ok()?) != key {
        return None;
    }
    let n = u32::from_le_bytes(d.get(12..16)?.try_into().ok()?) as usize;
    (0..n)
        .map(|i| {
            let at = 16 + i * 520;
            let num = u64::from_le_bytes(d.get(at..at + 8)?.try_into().ok()?);
            Some((num, d.get(at + 8..at + 520)?.try_into().ok()?))
        })
        .collect()
}

/// Writes the save when the console has stored anything new.
fn store_save(path: Option<&Path>, key: u64, t: &ribbit_twist::machine::Twist, saved: &mut Vec<(u64, [u8; 512])>) {
    let (Some(path), Some(card)) = (path, &t.bus.emmc) else {
        return;
    };
    let now = card.written();
    if now == *saved {
        return;
    }
    let mut d = Vec::with_capacity(16 + now.len() * 520);
    d.extend_from_slice(b"RTWS");
    d.extend_from_slice(&key.to_le_bytes());
    d.extend_from_slice(&(now.len() as u32).to_le_bytes());
    for (n, s) in &now {
        d.extend_from_slice(&n.to_le_bytes());
        d.extend_from_slice(s);
    }
    let tmp = path.with_extension("tmp");
    let ok = path.parent().is_none_or(|p| std::fs::create_dir_all(p).is_ok())
        && std::fs::write(&tmp, &d).is_ok()
        && std::fs::rename(&tmp, path).is_ok();
    if ok {
        *saved = now;
    }
}

/// A key for the storage as built (its size, and samples through it): a
/// save applies only to storage laid out the same.
fn volume_key(v: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325 ^ v.len() as u64;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h = (h ^ x as u64).wrapping_mul(0x100_0000_01b3);
        }
    };
    // The start (boot area, FATs, root folder), then every MB.
    eat(&v[..v.len().min(1 << 22)]);
    for at in (0..v.len()).step_by(1 << 20) {
        eat(&v[at..(at + 512).min(v.len())]);
    }
    h
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
    /// Where the console's stored data is kept.
    save: Option<PathBuf>,
    /// Offer the console on the PC's USB.
    pub usb: bool,
}

impl TwistSession {
    /// A session; `known` lists the library's RockIt Twist files, among
    /// which (and next to `package`) the system packages are looked for.
    pub fn start(
        package: Option<PathBuf>,
        known: &[PathBuf],
        content: Option<PathBuf>,
        audio: Option<&AudioOut>,
        save: Option<PathBuf>,
        usb: bool,
    ) -> Self {
        let system = SystemPackages::find(package.as_deref(), known);
        let machine = system.clone().map(|s| Machine::start(s, content.clone(), audio.map(|a| a.sink()), save.clone(), usb));
        let kind = package.as_deref().and_then(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            package::kind(&name, &head(p)?)
        });
        let title = match &package {
            Some(p) => describe_package(p).or_else(|| describe_installed(p)).map_or_else(|| "RockIt Twist".to_string(), |g| g.title),
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
            save,
            usb,
        }
    }

    /// Powers the console off and on again.
    /// Buttons still held stay held: held at power-on, some start other
    /// modes (the factory test, see docs/rockit-twist.md).
    pub fn restart(&mut self, audio: Option<&AudioOut>) {
        self.machine = None;
        self.machine = self
            .system
            .clone()
            .map(|s| Machine::start(s, self.content.clone(), audio.map(|a| a.sink()), self.save.clone(), self.usb));
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
            "To start the console, Ribbit needs its Bootloader, Base and Firmware-Bulk packages (.lfp) in a games folder. Until then, only the controls work here."
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
/// The games installed in a content folder (`Applications\<package>`, each
/// with its `meta.inf`), as library entries: the built-in bundle tagged
/// "Internal", the packs "Installed". Playing one starts the console, whose
/// own menu then offers it.
pub fn installed_games(content: &Path) -> Vec<Game> {
    let Ok(dirs) = std::fs::read_dir(content.join("Applications")) else {
        return Vec::new();
    };
    let mut games: Vec<Game> = dirs.flatten().filter_map(|d| describe_installed(&d.path())).collect();
    games.sort_by(|a, b| a.title.cmp(&b.title));
    games
}

/// An installed package folder (`meta.inf` of type Application).
pub fn describe_installed(dir: &Path) -> Option<Game> {
    let meta = package::Meta::parse(&std::fs::read_to_string(dir.join("meta.inf")).ok()?);
    if meta.get("Type") != Some("Application") {
        return None;
    }
    let id = meta.get("PackageID").map_or_else(|| dir.file_name().unwrap_or_default().to_string_lossy().into_owned(), str::to_string);
    let bundle = dir.file_name().is_some_and(|n| n == "Bundle");
    // The bundle's games are its fapp\*.bin (Nomnom, Balloondart...).
    let mut built_in: Vec<String> = std::fs::read_dir(dir.join("fapp"))
        .map(|r| {
            r.flatten()
                .filter_map(|e| {
                    let n = e.file_name().to_string_lossy().into_owned();
                    n.strip_suffix(".bin").map(|s| s.replace('_', " "))
                })
                .filter(|n| !matches!(n.as_str(), "PetMenu" | "Usb"))
                .collect()
        })
        .unwrap_or_default();
    built_in.sort();
    let title = if bundle {
        "RockIt Twist Built-in Games".to_string()
    } else {
        meta.get("Name").unwrap_or(&id).trim_end_matches(" Game Pack").to_string()
    };
    let mut details = vec![("System", "RockIt Twist".to_string())];
    if let Some(n) = meta.get("Name") {
        details.push(("Name", n.to_string()));
    }
    if !built_in.is_empty() {
        details.push(("Games", built_in.join(", ")));
    }
    for (label, key) in [("Version", "Version"), ("Locale", "Locale"), ("Developer", "Developer")] {
        if let Some(v) = meta.get(key) {
            details.push((label, v.to_string()));
        }
    }
    details.push(("Package", id.clone()));
    details.push((
        "Status",
        "installed in the content folder: the console starts at its menu, which offers this game".to_string(),
    ));
    details.push(("Folder", dir.display().to_string()));
    let subtitle = if bundle {
        format!("{} games built into the console", built_in.len())
    } else {
        format!("Game pack  ·  {}", meta.get("Version").unwrap_or("installed"))
    };
    Some(Game {
        path: dir.to_path_buf(),
        family: Family::RockItTwist,
        title,
        subtitle,
        details,
        tags: vec![(if bundle { "Internal" } else { "Installed" }).into()],
        key: format!("twist-installed-{id}"),
        art: Art::Missing,
    })
}

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
        "early emulation: the console starts; game packs run once unpacked and imported (content folder)".to_string(),
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
        tags: vec![tag.into()],
        key,
        art: Art::Missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_console_storage_is_saved_and_only_restored_onto_the_same_layout() {
        let path = std::env::temp_dir().join(format!("ribbit-twist-save-{}.emmc", std::process::id()));
        let mut t = ribbit_twist::machine::Twist::new(&[]);
        let mut card = ribbit_twist::sd::Card::emmc(vec![0; 4096], 64);
        card.write_sector(5, &[7; 512]);
        t.bus.emmc = Some(card);
        let mut saved = Vec::new();
        store_save(Some(&path), 42, &t, &mut saved);
        assert_eq!(saved.len(), 1);
        let back = load_save(&path, 42).unwrap();
        assert_eq!((back[0].0, back[0].1[0]), (5, 7));
        assert!(load_save(&path, 43).is_none(), "another layout: not applied");
        std::fs::remove_file(&path).ok();
        assert_ne!(volume_key(&[0; 4096]), volume_key(&[0; 8192]));
    }


    #[test]
    fn installed_games_are_listed_from_the_content_folder() {
        let dir = std::env::temp_dir().join(format!("ribbit-twist-installed-{}", std::process::id()));
        let apps = dir.join("Applications");
        std::fs::create_dir_all(apps.join("Bundle").join("fapp")).unwrap();
        std::fs::create_dir_all(apps.join("PCKT-0x0050000D-000000")).unwrap();
        std::fs::create_dir_all(apps.join("Leftover")).unwrap();
        std::fs::write(
            apps.join("Bundle").join("meta.inf"),
            "Type=\"Application\"
PackageID=\"PCKT-0x00500001-000000\"
Name=\"Included App Bundle\"
",
        )
        .unwrap();
        for g in ["Nomnom.bin", "Pixel_Art.bin", "Usb.bin"] {
            std::fs::write(apps.join("Bundle").join("fapp").join(g), b"").unwrap();
        }
        std::fs::write(
            apps.join("PCKT-0x0050000D-000000").join("meta.inf"),
            "Type=\"Application\"
PackageID=\"PCKT-0x0050000D-000000\"
Name=\"Banzai Beans: Showdown Game Pack\"
Version=\"1.0.3.16\"
",
        )
        .unwrap();
        let games = installed_games(&dir);
        std::fs::remove_dir_all(&dir).ok();
        let titles: Vec<&str> = games.iter().map(|g| g.title.as_str()).collect();
        assert_eq!(titles, ["Banzai Beans: Showdown", "RockIt Twist Built-in Games"]);
        assert!(games.iter().all(|g| g.family == Family::RockItTwist));
        assert_eq!(games[1].tags, ["Internal"]);
        assert_eq!(games[1].subtitle, "2 games built into the console", "the USB screen is not a game");
        assert_eq!(games[0].tags, ["Installed"]);
    }
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
        let mut s = TwistSession::start(None, &[], None, None, None, false);
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
