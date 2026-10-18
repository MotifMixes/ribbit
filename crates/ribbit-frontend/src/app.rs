//! The Ribbit application: one window, the emulated Leapster, egui menus.

use crate::audio_out::AudioOut;
use crate::console_view::{ConsoleView, Event as ViewEvent, Target};
use crate::emu::{Control, Emu};
use crate::files::Pick;
use crate::gpu::Gpu;
use crate::held::{Held, Source};
use crate::library::{ArtSource, Library};
use crate::saves::{AutoSave, SavePaths};
use crate::settings::Settings;
use crate::ui::{self, Action, UiState};
use anyhow::{Context, Result};
use ribbit_core::Machine;
use ribbit_memory::known::Hardware;
use ribbit_video::{HEIGHT, WIDTH};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

/// Rewind: a snapshot every half second of emulated time, 30 s kept.
const REWIND_EVERY_CYCLES: u64 = 48_000_000;
/// The window title: name and version.
const APP_NAME: &str = concat!("Ribbit v", env!("CARGO_PKG_VERSION"));
/// Frames spent sizing the window around the screen (the first frames can
/// still be settling the menu bar's height).
const FIT_TRIES: u8 = 6;
/// The smallest window: wide enough for the whole menu bar (and an update
/// button) at any display scaling, so logical, not physical, pixels.
const MIN_WINDOW: winit::dpi::LogicalSize<f64> = winit::dpi::LogicalSize::new(460.0, 360.0);
const REWIND_KEEP: usize = 60;

/// How Ribbit was started (command line / config file overrides).
#[derive(Debug, Clone, Default)]
pub struct StartOptions {
    /// Settings, saves and screenshots live here.
    pub data_dir: PathBuf,
    pub firmware: Option<PathBuf>,
    pub cartridge: Option<PathBuf>,
    pub system_eeprom: Option<PathBuf>,
    pub cart_save: Option<PathBuf>,
    pub straps: Option<u32>,
    pub experimental_touch: Option<bool>,
    pub cpu_mhz: Option<u64>,
    /// Android: true once the activity is destroyed (swiped away from the
    /// recent apps). Ribbit then saves and stops: winit 0.30 does not pass
    /// that on, and Android waits for Ribbit to stop before it can start the
    /// app again.
    pub destroyed: Option<fn() -> bool>,
    /// Android: opens the system's file picker (true: for a folder); false
    /// if it can't (Ribbit's own browser is used then).
    pub pick_start: Option<fn(bool) -> bool>,
    /// Android: the picker's answer once it is in: the path, None if
    /// cancelled, or why the choice can't be used.
    pub pick_poll: Option<fn() -> Option<Result<Option<PathBuf>, String>>>,
}

struct Session {
    /// Debug: RIBBIT_IDLE_TEST_SECS (see `check_idle_power_off`).
    idle_test_secs: Option<u32>,
    /// A save state from just before an idle power-off (for Resume).
    idle_snapshot: Option<Vec<u8>>,
    /// When the BIOS began its power-off animation (it fades the screen to
    /// black in about 0.75 s); the game closes as soon as it is black.
    closing: Option<Instant>,
    /// The cartridge played.
    cart_path: Option<PathBuf>,
    /// EXPERIMENTAL HD picture, when HD vectors are on and this BIOS supports it.
    hd: Option<ribbit_core::hd::HdCanvas>,
    /// The machine, running on its own thread.
    emu: Emu,
    experimental_touch: bool,
    saves: SavePaths,
    autosave: AutoSave,
    title: String,
    /// The console model (its BIOS), as players call it.
    system: &'static str,
    hardware: Hardware,
    /// When play started (Unix seconds), for Discord's timer.
    started: u64,
    /// Prefix for this game's save-state files.
    state_key: String,
    rewind: std::collections::VecDeque<Vec<u8>>,
    /// Snapshots are taken under the machine lock (~1 ms) and compressed on
    /// this worker (~4 ms), so taking one does not stall the picture.
    compress: Compressor,
    last_snapshot: u64,
}

pub struct App {
    opts: StartOptions,
    settings: Settings,
    settings_path: PathBuf,
    session: Option<Session>,
    /// A Zippity being played (instead of `session`'s Leapster).
    zippity: Option<crate::zippity::ZipSession>,
    /// Its latest picture.
    tv_frame: Vec<u32>,
    /// Removing the saved players from a Zippity dump (the cleaned dump).
    zippity_cleaning: Option<std::thread::JoinHandle<anyhow::Result<PathBuf>>>,
    /// A zip of RockIt Twist packages being unpacked.
    twist_import: Option<std::thread::JoinHandle<anyhow::Result<crate::twist::Imported>>>,
    /// A RockIt Twist session (early emulation), instead of the others.
    twist: Option<crate::twist::TwistSession>,
    /// The console view's pointer state (controls held by the mouse).
    console_view: ConsoleView,
    /// Leapster/Zippity buttons held by each input source.
    held: Held,
    /// Mouse-wheel scrolling (pixels) not yet a whole notch (RockIt Twist
    /// Slider without the console view).
    wheel_px: f64,
    gpu: Option<Gpu>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    ui: UiState,
    frame: Box<[u32; WIDTH * HEIGHT]>,
    paused: bool,
    /// The BIOS for the next boot instead of the chosen one (a console page
    /// in the library); used once.
    next_bios: Option<PathBuf>,
    /// The window is not the active one (paused if `pause_in_background`).
    background: bool,
    /// The taskbar button is flashing for an upcoming idle power-off.
    flashing: bool,
    fast_forward: bool,
    rewinding: bool,
    modifiers: ModifiersState,
    speed_percent: f64,
    /// For the periodic audio-gap log line.
    audio_log: (Instant, u64),
    audio: Option<AudioOut>,
    #[cfg(not(target_os = "android"))]
    pads: Option<crate::gamepad::Gamepads>,
    cursor: Option<(f64, f64)>,
    mouse_down: bool,
    finger: Option<(u64, (f64, f64))>,
    /// The on-screen touch controls, and the game area they were laid out
    /// in last frame (points).
    touch_pad: crate::touch_pad::TouchPad,
    touch_area: egui::Rect,
    lcd_px: (f32, f32, f32, f32),
    /// Seconds until the idle power-off, in its last minute (shown).
    idle_left: Option<u32>,
    library: Library,
    discord: crate::discord::Discord,
    /// Frames left to size the window around the screen (0 = done).
    fit_window: u8,
    /// macOS: the open panel showing as a sheet, until it is answered.
    #[cfg(target_os = "macos")]
    sheet: Option<crate::files::SheetPick>,
    /// Android: what the system's file picker, open now, is choosing.
    native_pick: Option<Pick>,
    /// Android: the side menu was open last frame, and whether opening it
    /// paused the game (closing it then lets it go on).
    side_menu: (bool, bool),
    fatal: Option<anyhow::Error>,
    quit: bool,
    /// Everything is saved for good: nothing more is written (after an
    /// update's restart the new version owns the settings and saves).
    shut_down: bool,
    /// The game closed for inactivity, its snapshot, and where its BIOS
    /// keeps the idle timer (for Resume).
    idle_resume: Option<(PathBuf, Vec<u8>, Option<ribbit_core::idle_timer::IdleTimer>)>,
    /// The library has opened on the chosen console's page (once, when its
    /// first scan is in).
    tab_shown: bool,
}

fn hw_zippity() -> ribbit_memory::known::Hardware {
    ribbit_memory::known::Hardware::Zippity
}

/// Runs the desktop app until the window closes.
pub fn run(opts: StartOptions) -> Result<()> {
    let event_loop = EventLoop::new().context("creating event loop")?;
    run_with(event_loop, opts)
}

/// Runs the app on an existing event loop (Android builds its own).
pub fn run_with(event_loop: EventLoop<()>, opts: StartOptions) -> Result<()> {
    crate::perf::prefer_performance_cores();
    let mut app = App::new(opts);
    event_loop.run_app(&mut app)?;
    app.shutdown();
    match app.fatal {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

impl App {
    pub fn new(opts: StartOptions) -> Self {
        let settings_path = opts.data_dir.join("settings.toml");
        let opts_data_dir = opts.data_dir.clone();
        let mut settings = Settings::load(&settings_path);
        // A different Ribbit version (an update, or an older one) copies the
        // saves and settings aside before anything can change them.
        if settings.last_version != crate::backup::VERSION {
            let save_dir = settings.save_dir.clone().unwrap_or_else(|| opts.data_dir.join("saves"));
            crate::backup::before_update(&opts.data_dir, &save_dir, &settings.last_version);
            settings.last_version = crate::backup::VERSION.to_string();
            if let Err(e) = settings.save(&settings_path) {
                tracing::warn!(target: "ribbit::settings", "saving settings: {e:#}");
            }
        }
        // What an in-app update moved aside.
        crate::install::remove_leftovers();
        crate::twist::set_bindings(&settings.twist_keys);
        if let Some(f) = &opts.firmware {
            settings.bios = Some(f.clone());
        }
        let mut app = Self {
            opts,
            settings,
            settings_path,
            session: None,
            zippity: None,
            tv_frame: Vec::new(),
            zippity_cleaning: None,
            twist_import: None,
            twist: None,
            console_view: ConsoleView::default(),
            held: Held::default(),
            wheel_px: 0.0,
            gpu: None,
            egui_ctx: {
                let ctx = egui::Context::default();
                crate::fonts::install(&ctx);
                ctx
            },
            egui_state: None,
            ui: UiState::new(),
            frame: Box::new([0; WIDTH * HEIGHT]),
            paused: false,
            next_bios: None,
            background: false,
            flashing: false,
            fast_forward: false,
            rewinding: false,
            modifiers: ModifiersState::empty(),
            speed_percent: 100.0,
            audio_log: (Instant::now(), 0),
            audio: AudioOut::open(),
            #[cfg(not(target_os = "android"))]
            pads: crate::gamepad::Gamepads::open(),
            cursor: None,
            mouse_down: false,
            finger: None,
            touch_pad: crate::touch_pad::TouchPad::new(),
            touch_area: egui::Rect::NOTHING,
            lcd_px: (0.0, 0.0, 0.0, 0.0),
            idle_left: None,
            fit_window: 0,
            #[cfg(target_os = "macos")]
            sheet: None,
            native_pick: None,
            side_menu: (false, false),
            library: Library::new(&opts_data_dir),
            discord: crate::discord::Discord::start(),
            fatal: None,
            quit: false,
            shut_down: false,
            idle_resume: None,
            tab_shown: false,
        };
        // A game given on the command line starts at once; otherwise the
        // game library shows.
        match app.opts.cartridge.clone() {
            Some(cart) => {
                // The library loads meanwhile, ready for when the game closes.
                app.rescan_library();
                app.open_cart(cart)
            }
            None => app.rescan_library(),
        }
        app
    }

    fn save_dir(&self) -> PathBuf {
        self.settings.save_dir.clone().unwrap_or_else(|| self.opts.data_dir.join("saves"))
    }

    fn save_settings(&self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            tracing::warn!(target: "ribbit::settings", "saving settings: {e:#}");
        }
    }

    /// Starts (or restarts) the machine with the BIOS and `cart`.
    fn boot(&mut self, cart: Option<PathBuf>) -> Result<()> {
        self.end_session();
        let bios = match self.next_bios.take() {
            Some(b) => b,
            None => self.settings.bios.clone().context("Choose a BIOS first (File → Open BIOS…).")?,
        };
        let firmware = ribbit_core::load_firmware(&bios)?;
        if firmware.known().is_none() {
            self.ui.toast("Unknown BIOS dump: it may not work");
        }
        let cartridge = cart.as_deref().map(ribbit_core::load_cartridge).transpose()?;
        let default_straps = if firmware.is_leapster_tv() {
            ribbit_core::io::straps::LEAPSTER_TV
        } else if self.settings.skip_boot_calibration {
            0x63FF_FFFF
        } else {
            ribbit_core::io::straps::MAME_LEAPSTER2_MEASURED
        };
        let mut machine = Machine::with_straps(firmware, cartridge, self.opts.straps.unwrap_or(default_straps));
        // Playing, not debugging: skip the per-instruction trace history.
        machine.record_trace_tail = false;
        let mhz = self.opts.cpu_mhz.unwrap_or(self.settings.cpu_mhz).max(1);
        machine.clock = ribbit_core::Clock::new(mhz * 1_000_000, ribbit_core::Clock::PERIPHERAL_HZ);
        if self.settings.mame_gpio_lines {
            tracing::warn!(target: "ribbit::boot", "EXPERIMENTAL: unresolved GPIO lines read 1 (MAME's workaround)");
            machine.io.gpio.unresolved_lines = !ribbit_input::BUTTON_BITS;
        }
        if self.opts.experimental_touch.unwrap_or(self.settings.experimental_touch) {
            machine.enable_experimental_mame_touch(Default::default());
        }
        let hd = self.settings.hd_vectors.then(|| Self::start_hd(&mut machine, self.hd_scale())).flatten();
        if self.settings.hd_vectors && hd.is_none() {
            self.ui.toast("HD vectors are not available with this BIOS");
        }
        machine.keep_awake = !self.settings.turn_off_when_idle;
        let mut saves = SavePaths::for_machine(&self.save_dir(), &machine);
        if let Some(p) = &self.opts.system_eeprom {
            saves.system = Some(p.clone());
        }
        if let Some(p) = &self.opts.cart_save
            && saves.cart.is_some()
        {
            saves.cart = Some(p.clone());
        }
        saves.load(&mut machine)?;
        // Leapster 2: it cannot finish calibrating at boot (its Flash player
        // asserts about 2 s in and the BIOS restarts the console), so it
        // boots with the stored calibration, a new console getting the
        // factory one, as a retail unit ships (core: spiflash.rs).
        if machine.ensure_touch_calibration() {
            tracing::info!(target: "ribbit::boot", "new Leapster 2: factory touch calibration stored");
        }
        // Leapster 2: through its start-up screens and main menu, as a real
        // one starts, unless the player chose to go straight to the game.
        if !self.settings.leapster2_skip_menu {
            machine.boot_leapster2_system();
        }
        if let Some(straps) = machine.straps()
            && straps & ribbit_core::io::straps::SKIP_CALIBRATION == 0
            && machine.io.spi_flash.is_some()
        {
            if self.opts.straps.is_none() {
                machine.set_straps(straps | ribbit_core::io::straps::SKIP_CALIBRATION);
            } else {
                // Calibration asked for on the command line: after the
                // BIOS's restart, boot normally.
                machine.restart_straps = Some(straps | ribbit_core::io::straps::SKIP_CALIBRATION);
            }
        }
        let title = match (machine.cartridge(), &cart) {
            (Some(c), Some(path)) => c.known().map(|k| short_name(k.name)).unwrap_or_else(|| {
                path.file_stem().map_or_else(|| "Cartridge".into(), |s| s.to_string_lossy().into_owned())
            }),
            _ if machine.io.leapster2_power.is_some() => "Leapster 2".to_string(),
            _ => "Leapster (no cartridge)".to_string(),
        };
        if let Some(path) = &cart {
            self.settings.add_recent(path);
            self.save_settings();
        }
        let state_key = match machine.cartridge() {
            Some(c) => format!("cart-{:08x}", c.id().crc32),
            None => format!("bios-{:08x}", machine.firmware().id().crc32),
        };
        let system = machine.firmware().known().map_or("Leapster", |k| k.hardware.name());
        let hardware = machine.firmware().known().map_or(Hardware::Leapster, |k| k.hardware);
        self.session = Some(Session {
            system,
            hardware,
            idle_test_secs: std::env::var("RIBBIT_IDLE_TEST_SECS").ok().and_then(|v| v.parse().ok()),
            idle_snapshot: None,
            closing: None,
            cart_path: cart.clone(),
            hd,
            experimental_touch: machine.experimental_touch_enabled(),
            emu: {
                let emu = Emu::start(machine);
                emu.set_audio_sink(self.audio.as_ref().map(|a| a.sink()));
                emu
            },
            saves,
            autosave: AutoSave::default(),
            title,
            started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            state_key,
            rewind: Default::default(),
            compress: Compressor::start(),
            last_snapshot: 0,
        });
        self.paused = false;
        if let Some(a) = &mut self.audio {
            a.clear();
        }
        self.update_title();
        Ok(())
    }

    fn end_session(&mut self) {
        // Nothing stays held into the next game.
        self.console_view.release_all();
        self.held.clear();
        if self.twist.take().is_some() {
            self.fit_window = FIT_TRIES;
        }
        if let Some(z) = self.zippity.take() {
            z.stop();
            // Back to the Leapster's square screen.
            self.fit_window = FIT_TRIES;
        }
        if let Some(s) = self.session.take() {
            let mut machine = s.emu.stop();
            if let Err(e) = s.saves.flush(&mut machine) {
                tracing::warn!(target: "ribbit::saves", "saving: {e:#}");
            }
        }
    }

    /// Saves everything, once: after an update's restart the new version is
    /// already running, and a second save here would overwrite its settings.
    fn shutdown(&mut self) {
        if std::mem::replace(&mut self.shut_down, true) {
            return;
        }
        self.end_session();
        self.save_settings();
    }

    fn set_bios(&mut self, path: PathBuf) {
        match ribbit_core::load_firmware(&path) {
            Ok(fw) => {
                let name = fw.known().map_or("unknown dump".to_string(), |k| k.name.to_string());
                // Kept with every other BIOS opened, newest first.
                self.settings.opened_bios.retain(|p| p != &path);
                self.settings.opened_bios.insert(0, path.clone());
                self.settings.bios = Some(path);
                self.save_settings();
                self.ui.toast(format!("BIOS: {name}"));
                self.rescan_library();
            }
            Err(e) => {
                let e = anyhow::Error::from(e);
                let (title, explain) = explain_load_error(&e);
                self.ui.problem(title, explain, format!("{e:#}"));
            }
        }
    }

    /// Rescans the ROM folders (and recent games); art is made with the
    /// BIOS, if there is one.
    fn rescan_library(&mut self) {
        let art = self.settings.bios.clone().map(|bios| ArtSource { bios });
        // Every BIOS opened with File > Open BIOS that is still there, the
        // one in use first.
        let mut opened: Vec<PathBuf> = Vec::new();
        for p in self.settings.bios.iter().chain(&self.settings.opened_bios) {
            if !opened.contains(p) && p.is_file() {
                opened.push(p.clone());
            }
        }
        self.library.set_opened_bios(&opened);
        self.library.set_picked_bios(&self.settings.bios_picked);
        // Recent games, and the BIOSes opened (their built-in games are
        // listed wherever they are).
        let extra: Vec<PathBuf> = self.settings.recent_carts.iter().chain(&opened).cloned().collect();
        self.library.scan(&self.settings.rom_folders, &extra, self.settings.twist_content.clone(), art);
        // Outline fonts for HD device text, from the same folders.
        crate::device_fonts::load(self.settings.rom_folders.clone());
    }

    fn add_rom_folder(&mut self, path: PathBuf) {
        if !self.settings.rom_folders.contains(&path) {
            self.settings.rom_folders.push(path);
            self.save_settings();
        }
        self.rescan_library();
    }

    fn open_dropped(&mut self, path: PathBuf) {
        // A BIOS is recognised by checksum; anything else is tried as a cartridge.
        let is_bios = ribbit_core::read_image(&path)
            .ok()
            .and_then(|d| ribbit_memory::Firmware::from_bytes(d).ok())
            .is_some_and(|fw| fw.known().is_some());
        if is_bios {
            self.set_bios(path);
        } else {
            self.open_cart(path);
        }
    }

    /// Starts a console's own system (its BIOS from the library), no
    /// cartridge.
    fn start_system(&mut self, hw: Hardware) {
        let Some(bios) = self.library.bios_for(hw).map(Path::to_path_buf) else {
            return self.ui.problem(
                "No BIOS for this console",
                "Add the folder that holds its BIOS dump with \"Add games folder\".",
                "",
            );
        };
        self.next_bios = Some(bios);
        if let Err(e) = self.boot(None) {
            let (title, explain) = explain_load_error(&e);
            self.ui.problem(title, explain, format!("{e:#}"));
        }
    }

    /// Plays a game on the console chosen in the library's console menu
    /// (unless a BIOS was given on the command line).
    fn open_cart(&mut self, path: PathBuf) {
        // A console's built-in games: the Zippity's NAND, or a Leapster
        // BIOS's own games (that BIOS with no cartridge in).
        let internal = self.library.games.iter().find(|g| g.path == path && g.tags.contains(&"Internal"));
        if internal.is_some_and(|g| matches!(g.family, crate::library::Family::Leapster | crate::library::Family::Leapster2)) {
            self.next_bios = Some(path);
            if let Err(e) = self.boot(None) {
                let (title, explain) = explain_load_error(&e);
                self.ui.problem(title, explain, format!("{e:#}"));
            }
            return;
        }
        if internal.is_some() || crate::library::is_internal(&path) {
            return self.start_zippity(path.clone(), path);
        }
        // A Zippity cartridge runs only on a Zippity: its NAND dump, whatever
        // console the library has chosen.
        let family = self.library.games.iter().find(|g| g.path == path).map(|g| g.family).or_else(|| crate::library::family_of(&path));
        // A RockIt Twist package (or one unpacked into a folder): the
        // RockIt Twist, started from its system packages.
        if family == Some(crate::library::Family::RockItTwist) || crate::twist::is_package_folder(&path) {
            return self.start_twist(Some(path));
        }
        if family == Some(crate::library::Family::Zippity) {
            // The library may still be scanning (a cartridge given on the
            // command line): then the folders are searched directly.
            let nand = self.library.bios_for(hw_zippity()).map(Path::to_path_buf).or_else(|| crate::library::find_zippity_nand(&self.settings.rom_folders));
            match nand {
                Some(nand) => return self.start_zippity(nand, path),
                None => {
                    return self.ui.problem(
                        "No Zippity dump found",
                        "Zippity cartridges need the console's own memory (its NAND dump) in one of your games \
                         folders. Add the folder that holds it with \"Add games folder\".",
                        "",
                    );
                }
            }
        }
        if self.opts.firmware.is_none() {
            // A game the chosen console can't play switches to its own
            // console, when that one is available.
            let family = family.unwrap_or(crate::library::Family::Leapster);
            let console = self.library.selected_console(&self.settings.library_console);
            if !family.runs_on(console) && self.library.consoles().contains(&family.console()) {
                self.settings.library_console = family.console().name().to_string();
                self.save_settings();
                self.ui.library.tab = Some(self.library.tab_for(family.console()));
            }
            match self.console_bios(&path) {
                Ok(Some(nand)) if self.library.selected_console(&self.settings.library_console) == ribbit_memory::known::Hardware::Zippity => {
                    return self.start_zippity(nand, path);
                }
                Ok(bios) => self.next_bios = bios,
                Err((title, explain)) => {
                    self.ui.problem(title, explain, "");
                    return;
                }
            }
        }
        if let Err(e) = self.boot(Some(path)) {
            let (title, explain) = explain_load_error(&e);
            self.ui.problem(title, explain, format!("{e:#}"));
        }
    }

    /// The BIOS to play `path` on the chosen console with (None: the chosen
    /// BIOS, for the original Leapster), or why it can't be played there
    /// (a title and an explanation).
    fn console_bios(&self, path: &Path) -> std::result::Result<Option<PathBuf>, (String, String)> {
        use crate::library::Family;
        use ribbit_memory::known::Hardware;
        let console = self.library.selected_console(&self.settings.library_console);
        let game = self.library.games.iter().find(|g| g.path == path);
        // Files that are not recognised are tried as Leapster cartridges.
        let family = game.map(|g| g.family).or_else(|| crate::library::family_of(path)).unwrap_or(Family::Leapster);
        let title = game.map_or_else(
            || path.file_stem().map_or_else(|| "This game".to_string(), |s| s.to_string_lossy().into_owned()),
            |g| g.title.clone(),
        );
        if !family.runs_on(console) {
            let fits: Vec<&str> = self.library.consoles().into_iter().filter(|&h| family.runs_on(h)).map(Hardware::name).collect();
            let choose = match fits.as_slice() {
                [] => format!("It needs a {} BIOS in your games folder.", family.console().name()),
                names => format!("Choose {} in the console menu next to Open Cartridge.", names.join(" or ")),
            };
            return Err((
                "Wrong console for this game".to_string(),
                format!("{title} is a {} game: it can't run on the {}. {choose}", family.name(), console.name()),
            ));
        }
        match console {
            Hardware::Zippity => self.library.bios_for(hw_zippity()).map(|p| Some(p.to_path_buf())).ok_or_else(|| {
                (
                    "No Zippity dump found".to_string(),
                    "Zippity games need the console's own memory (its NAND dump) in one of your games folders. \
                     Add the folder that holds it with \"Add games folder\"."
                        .to_string(),
                )
            }),
            // The BIOS chosen in File > Open BIOS, if it is an original
            // Leapster's; else one from the games folders (a Leapster 2 BIOS
            // chosen there must not stand in for the Leapster).
            Hardware::Leapster => {
                let chosen = self.settings.bios.as_deref().and_then(|p| ribbit_core::load_firmware(p).ok());
                if chosen.is_none_or(|f| f.known().is_none_or(|k| k.hardware == Hardware::Leapster)) {
                    return Ok(None);
                }
                self.library.bios_for(Hardware::Leapster).map(|p| Some(p.to_path_buf())).ok_or_else(|| {
                    (
                        "No Leapster BIOS found".to_string(),
                        "The BIOS chosen in File > Open BIOS belongs to another console. Add the folder that holds \
                         an original Leapster BIOS with \"Add games folder\", or open one with File > Open BIOS."
                            .to_string(),
                    )
                })
            }
            hw => self.library.bios_for(hw).map(|p| Some(p.to_path_buf())).ok_or_else(|| {
                (
                    format!("No {} BIOS found", hw.name()),
                    format!(
                        "Games on the {0} need a dump of the {0}'s own BIOS in one of your games folders. \
                         Add the folder that holds it with \"Add games folder\", or choose another console.",
                        hw.name()
                    ),
                )
            }),
        }
    }

    /// Plays a Zippity: its system (from the NAND dump `nand`) with its
    /// built-in games. Cartridges cannot be read yet: opening one starts the
    /// system and says so.
    fn start_zippity(&mut self, nand: PathBuf, game: PathBuf) {
        self.end_session();
        let sink = self.audio.as_ref().map(|a| a.sink());
        // A cartridge goes in the slot; the system starts its game after
        // the player signs in.
        let cart = (game != nand).then_some(game.as_path());
        match crate::zippity::ZipSession::start(&nand, cart, &self.save_dir(), sink) {
            Ok(mut z) => {
                // The TV's 4:3 picture.
                self.fit_window = FIT_TRIES;
                z.title = match cart {
                    // Until the library has listed it, the file's name.
                    Some(c) => self.library.games.iter().find(|g| g.path == c).map_or_else(
                        || c.file_stem().map_or_else(|| "Zippity".to_string(), |s| s.to_string_lossy().into_owned()),
                        |g| g.title.clone(),
                    ),
                    None => self
                        .library
                        .games
                        .iter()
                        .find(|g| g.path == nand && g.tags.contains(&"Internal"))
                        .map_or_else(|| "Zippity Built-in Games".to_string(), |g| g.title.clone()),
                };
                z.set_on_home(self.settings.zippity_on_home);
                self.zippity = Some(z);
                self.paused = false;
                if let Some(a) = &mut self.audio {
                    a.clear();
                }
                self.update_title();
            }
            Err(e) => self.ui.problem(
                "Couldn't start the Zippity",
                match cart {
                    Some(c) => format!(
                        "Ribbit couldn't load the Zippity with the cartridge \"{}\" (and the memory dump \"{}\"). Either may be damaged or not a full dump.",
                        file_name(c),
                        file_name(&nand)
                    ),
                    None => format!("Ribbit couldn't load the Zippity's memory dump \"{}\". It may be damaged or not a full dump.", file_name(&nand)),
                },
                format!("{e:#}"),
            ),
        }
    }

    /// Opens a RockIt Twist session (`package`: the package chosen in the
    /// library, if any): the console starts from its system packages when
    /// they are found, otherwise only its controls work.
    fn start_twist(&mut self, package: Option<PathBuf>) {
        self.end_session();
        let session = crate::twist::TwistSession::start(
            package,
            &self.twist_known(),
            self.settings.twist_content.clone(),
            self.audio.as_ref(),
            Some(self.save_dir().join("rockit-twist.emmc")),
            self.settings.twist_usb,
        );
        let running = session.machine.is_some();
        self.twist = Some(session);
        self.paused = false;
        self.fit_window = FIT_TRIES;
        if let Some(a) = &mut self.audio {
            a.clear();
        }
        self.update_title();
        self.ui.toast(if running {
            "RockIt Twist: early emulation"
        } else {
            "RockIt Twist: system packages not found, controls only"
        });
    }

    /// The RockIt Twist files the system packages are looked for among: the
    /// library's, and those in Ribbit's own system folder.
    fn twist_known(&self) -> Vec<PathBuf> {
        let mut known: Vec<PathBuf> = self
            .library
            .games
            .iter()
            .filter(|g| g.family == crate::library::Family::RockItTwist)
            .map(|g| g.path.clone())
            .collect();
        known.extend(crate::twist::packages_in(&crate::twist::system_dir(&self.opts.data_dir)));
        known
    }

    /// Unpacks a zip of RockIt Twist packages into Ribbit's data folder (in
    /// the background), then uses it as the console's content.
    fn import_twist_zip(&mut self, zip: PathBuf) {
        if self.twist_import.is_some() {
            self.ui.toast("Already importing RockIt Twist games");
            return;
        }
        let prefer = crate::twist::SystemPackages::find(None, &self.twist_known()).and_then(|p| crate::twist::system_locale(&p));
        let into = self.opts.data_dir.join("rockit-twist").join("content");
        self.ui.toast("Importing RockIt Twist games…");
        self.twist_import = Some(std::thread::spawn(move || crate::twist::import_zip(&zip, &into, prefer.as_deref())));
    }

    /// Chooses (or forgets) the RockIt Twist's storage content; a running
    /// RockIt Twist restarts with it.
    fn set_twist_content(&mut self, path: Option<PathBuf>) {
        self.settings.twist_content = path;
        self.save_settings();
        // Its installed games are listed in the library's RockIt Twist tab.
        self.rescan_library();
        if let Some(t) = &mut self.twist {
            t.content = self.settings.twist_content.clone();
            t.restart(self.audio.as_ref());
            self.ui.toast(match &t.content {
                Some(_) => "RockIt Twist restarted with the content folder",
                None => "RockIt Twist restarted without content",
            });
        }
    }

    /// The console running (None: none).
    fn current_hardware(&self) -> Option<Hardware> {
        if self.twist.is_some() {
            Some(Hardware::RockItTwist)
        } else if self.zippity.is_some() {
            Some(Hardware::Zippity)
        } else {
            self.session.as_ref().map(|s| s.hardware)
        }
    }

    /// The console view's layout for the running console, when it is on.
    fn view_layout(&self) -> Option<crate::console_view::Layout> {
        let hw = self.current_hardware()?;
        self.view_setting(hw).then(|| crate::console_view::layout_for(hw)).flatten()
    }

    /// Whether the console view is chosen for `hw` (each console has its
    /// own choice: on for the RockIt Twist, off for the Leapsters, unless
    /// changed).
    fn view_setting(&self, hw: Hardware) -> bool {
        if hw == Hardware::RockItTwist { self.settings.console_view_twist } else { self.settings.console_view }
    }

    /// A Leapster or Zippity button pressed or let go by one source (the
    /// machine sees it down while any source holds it).
    fn press(&mut self, source: Source, b: ribbit_input::Button, down: bool) {
        let Some(state) = self.held.set(source, b, down) else {
            return;
        };
        if let Some(s) = &self.session {
            s.emu.lock().buttons.set(b, state);
        }
        if let Some(z) = &self.zippity {
            z.set_button(b, state);
        }
    }

    /// The touch controls for the running console, when they show (not with
    /// the console view, whose own controls take touches).
    fn touch_pads(&self) -> Option<Vec<crate::touch_pad::Pad>> {
        if !self.settings.touch_controls || !self.touch_pad.visible || self.view_layout().is_some() {
            return None;
        }
        if !self.touch_area.is_positive() {
            return None;
        }
        Some(crate::touch_pad::layout(self.current_hardware()?, self.touch_area))
    }

    /// A touch control pressed or let go.
    fn touch_target(&mut self, target: Target, down: bool) {
        match target {
            Target::Leapster(b) => self.press(Source::Touch, b, down),
            Target::Twist(b) => {
                if let Some(t) = &mut self.twist {
                    t.controls.set(ribbit_twist::input::Source::Pointer, b, down);
                }
            }
        }
    }

    /// A key or controller button was used: the touch controls go away
    /// (until the screen is touched again), letting go of what they held.
    fn hide_touch_pad(&mut self) {
        if !self.touch_pad.visible {
            return;
        }
        self.touch_pad.visible = false;
        for t in self.touch_pad.release_all() {
            self.touch_target(t, false);
        }
    }

    /// A finger on the window: the touch controls' when it is on them
    /// (true), otherwise the stylus's.
    fn touch_controls_event(&mut self, t: &winit::event::Touch) -> bool {
        if self.settings.touch_controls && t.phase == TouchPhase::Started {
            self.touch_pad.visible = true;
        }
        let Some(pads) = self.touch_pads() else {
            return false;
        };
        let ppp = self.egui_ctx.pixels_per_point();
        let p = egui::pos2(t.location.x as f32 / ppp, t.location.y as f32 / ppp);
        match t.phase {
            TouchPhase::Started if !self.ui_at((t.location.x, t.location.y)) => {
                let (taken, target) = self.touch_pad.down(&pads, t.id, p);
                if let Some(target) = target {
                    self.touch_target(target, true);
                }
                taken
            }
            TouchPhase::Moved => match self.touch_pad.moved(&pads, t.id, p) {
                Some((off, on)) => {
                    if let Some(off) = off {
                        self.touch_target(off, false);
                    }
                    if let Some(on) = on {
                        self.touch_target(on, true);
                    }
                    true
                }
                None => false,
            },
            TouchPhase::Ended | TouchPhase::Cancelled => match self.touch_pad.up(t.id) {
                Some(held) => {
                    if let Some(held) = held {
                        self.touch_target(held, false);
                    }
                    true
                }
                None => false,
            },
            _ => false,
        }
    }

    /// What the console view's controls did.
    fn apply_view_events(&mut self, events: Vec<ViewEvent>) {
        use ribbit_twist::input::Source as TwistSource;
        for e in events {
            match e {
                ViewEvent::Press(Target::Leapster(b), down) => self.press(Source::Pointer, b, down),
                ViewEvent::Press(Target::Twist(b), down) => {
                    if let Some(t) = &mut self.twist {
                        t.controls.set(TwistSource::Pointer, b, down);
                    }
                }
                ViewEvent::Turn(r, steps) => {
                    if let Some(t) = &mut self.twist {
                        t.controls.turn(r, steps);
                    }
                }
                ViewEvent::Strip(pos) => {
                    if let Some(t) = &mut self.twist {
                        t.controls.touch_slider(pos);
                    }
                }
                ViewEvent::Swipe(up) => {
                    if let Some(t) = &mut self.twist {
                        t.controls.swipe(up);
                    }
                }
            }
        }
    }

    /// Lets go of every control the mouse holds on the console view.
    fn release_view(&mut self) {
        let events = self.console_view.release_all();
        self.apply_view_events(events);
        for b in self.held.release(Source::Pointer) {
            if let Some(s) = &self.session {
                s.emu.lock().buttons.set(b, false);
            }
            if let Some(z) = &self.zippity {
                z.set_button(b, false);
            }
        }
        if let Some(t) = &mut self.twist {
            t.controls.release(ribbit_twist::input::Source::Pointer);
        }
    }

    /// F7: the running console's choice (the Leapster's when none runs).
    fn toggle_console_view(&mut self) {
        let hw = self.current_hardware().unwrap_or(Hardware::Leapster);
        let on = !self.view_setting(hw);
        if hw == Hardware::RockItTwist {
            self.settings.console_view_twist = on;
        } else {
            self.settings.console_view = on;
        }
        self.console_view_changed();
        self.ui.toast(if on { "Console view on" } else { "Console view off" });
    }

    /// Saved at once; turning it off lets go of what the mouse held. The
    /// game carries on (nothing in the emulation changes).
    fn console_view_changed(&mut self) {
        self.save_settings();
        if self.view_layout().is_none() {
            self.release_view();
        }
    }

    fn pick(&mut self, what: Pick) {
        let start = match what {
            Pick::Bios => self.settings.bios.clone(),
            Pick::Cartridge => self.settings.recent_carts.first().cloned(),
            Pick::RomFolder => self.settings.rom_folders.last().cloned(),
            Pick::TwistContent => self.settings.twist_content.clone(),
            Pick::TwistZip => None,
        };
        #[cfg(target_os = "macos")]
        if self.sheet.is_none()
            && let Some(g) = &self.gpu
        {
            self.sheet = Some(crate::files::SheetPick::start(what, start.as_deref(), &g.window));
        }
        #[cfg(not(any(target_os = "android", target_os = "macos")))]
        if let Some(path) = crate::files::native_pick(what, start.as_deref()) {
            self.picked(what, path);
        }
        // Android: the system's own picker; Ribbit's browser if it can't open.
        #[cfg(target_os = "android")]
        if self.opts.pick_start.is_some_and(|open| open(what.is_folder())) {
            self.native_pick = Some(what);
        } else {
            self.ui.browser.start(what, start.as_deref());
        }
    }

    /// Android: acts on the system picker's answer once it is in.
    fn poll_native_pick(&mut self) {
        let (Some(what), Some(poll)) = (self.native_pick, self.opts.pick_poll) else {
            return;
        };
        let Some(answer) = poll() else {
            return;
        };
        self.native_pick = None;
        match answer {
            Ok(Some(path)) => self.picked(what, path),
            Ok(None) => {}
            Err(why) => self.ui.problem("Ribbit can't use that", why, String::new()),
        }
    }

    fn picked(&mut self, what: Pick, path: PathBuf) {
        match what {
            Pick::Bios => self.set_bios(path),
            Pick::Cartridge => self.open_cart(path),
            Pick::RomFolder => self.add_rom_folder(path),
            Pick::TwistContent => self.set_twist_content(Some(path)),
            Pick::TwistZip => self.import_twist_zip(path),
        }
    }

    /// macOS: acts on the sheet's answer once it is in.
    #[cfg(target_os = "macos")]
    fn poll_sheet(&mut self) {
        let Some(answer) = self.sheet.as_mut().and_then(|s| s.poll()) else {
            return;
        };
        let what = self.sheet.take().map(|s| s.what);
        if let (Some(what), Some(path)) = (what, answer) {
            self.picked(what, path);
        }
    }

    fn update_title(&self) {
        if let Some(g) = &self.gpu {
            let title = self
                .session
                .as_ref()
                .map(|s| &s.title)
                .or(self.zippity.as_ref().map(|z| &z.title))
                .or(self.twist.as_ref().map(|t| &t.title));
            let t = match title {
                Some(t) if self.paused => format!("{APP_NAME} - {t} (paused)"),
                Some(t) => format!("{APP_NAME} - {t}"),
                None => APP_NAME.to_string(),
            };
            g.window.set_title(&t);
        }
    }

    fn act(&mut self, a: Action, event_loop: &ActiveEventLoop) {
        match a {
            Action::Pick(w) => self.pick(w),
            Action::OpenCartridge(p) => self.open_cart(p),
            Action::PlayFromLibrary(p) => self.open_cart(p),
            Action::SetLibraryConsole(hw) => {
                self.settings.library_console = hw.name().to_string();
                self.save_settings();
                // The page with the games this console plays.
                self.ui.library.tab = Some(self.library.tab_for(hw));
            }
            Action::SetBios(p) => self.set_bios(p),
            Action::StartSystem(hw) => self.start_system(hw),
            Action::CloseCartridge => {
                self.end_session();
                self.update_title();
                // Recently played games join the library.
                self.rescan_library();
            }
            Action::AddRomFolder(p) => self.add_rom_folder(p),
            Action::RemoveRomFolder(p) => {
                self.settings.rom_folders.retain(|f| *f != p);
                self.save_settings();
                self.rescan_library();
            }
            Action::RescanLibrary => self.rescan_library(),
            Action::RemakeArt(p) => {
                self.library.forget_art(&p);
                self.rescan_library();
            }
            Action::AskDeleteSave(path, title) => self.ui.ask_delete_save(path, title),
            Action::PickBios(hw, path) => self.pick_bios(hw, path),
            Action::DeleteSaveData(path) => self.delete_save_data(&path),
            Action::CleanZippityDump(path) => {
                self.end_session();
                // Our saves were made on top of the old players: drop them.
                let _ = std::fs::remove_file(crate::zippity::save_file(&path, &self.save_dir()));
                self.ui.toast("Removing the saved players from the Zippity dump...");
                self.zippity_cleaning = Some(std::thread::spawn(move || crate::zippity::clean_dump(&path)));
            }
            Action::KeepZippitySaves(path) => {
                self.settings.zippity_keep_saves.push(path);
                self.save_settings();
            }
            Action::OpenTwist => self.start_twist(None),
            Action::SetTwistContent(path) => self.set_twist_content(path),
            Action::SetTwistUsb(on) => {
                self.settings.twist_usb = on;
                self.save_settings();
                if let Some(t) = &mut self.twist {
                    t.usb = on;
                    t.restart(self.audio.as_ref());
                }
                self.ui.toast(if on {
                    "RockIt Twist on USB: run  usbip attach -r 127.0.0.1 -b 1-1  (usbip-win2) to plug it in"
                } else {
                    "RockIt Twist unplugged from USB"
                });
            }
            Action::ImportTwistZip(path) => self.import_twist_zip(path),
            Action::ConsoleViewChanged => self.console_view_changed(),
            Action::Reset => self.reset(),
            Action::TogglePause => self.toggle_pause(),
            Action::Screenshot => self.screenshot(),
            Action::ToggleFullscreen => self.toggle_fullscreen(),
            Action::SetWindowScale(s) => {
                self.settings.window_scale = s;
                self.save_settings();
                self.fit_window = FIT_TRIES;
            }
            Action::SettingsChanged => {
                self.save_settings();
                if let Some(z) = &self.zippity {
                    z.set_on_home(self.settings.zippity_on_home);
                }
                if let Some(s) = &self.session {
                    s.emu.lock().keep_awake = !self.settings.turn_off_when_idle;
                }
            }
            Action::ResumeAfterIdle => self.resume_after_idle(),
            Action::DismissIdleBanner => {
                self.ui.idle_closed = None;
                self.idle_resume = None;
            }
            Action::HdChanged => {
                self.save_settings();
                self.set_hd(self.settings.hd_vectors);
            }
            Action::SaveState(slot) => self.save_state(slot),
            Action::LoadState(slot) => self.load_state(slot),
            Action::RestartForUpdate => {
                // Everything saved before the new version starts and reads it.
                self.shutdown();
                match crate::install::restart() {
                    Ok(()) => {
                        self.quit = true;
                        event_loop.exit();
                    }
                    Err(e) => {
                        // Still running: closing it saves again.
                        self.shut_down = false;
                        self.ui.problem(
                            "The update is installed",
                            "Ribbit couldn't restart by itself: close it and open it again to use the new version.",
                            e.to_string(),
                        )
                    }
                }
            }
            Action::Quit => {
                self.quit = true;
                event_loop.exit();
            }
        }
    }

    /// With "Turn off when idle" on: when the BIOS has turned the console off
    /// for inactivity, close the game and say so. (Debug: RIBBIT_IDLE_TEST_SECS
    /// keeps the BIOS's idle count at most that many seconds, to try this
    /// without waiting.)
    fn check_idle_power_off(&mut self) {
        self.idle_left = None;
        let Some(s) = &mut self.session else {
            return;
        };
        let mut m = s.emu.lock();
        if let Some(secs) = s.idle_test_secs {
            m.cap_idle_count(secs);
        }
        if !self.settings.turn_off_when_idle {
            return;
        }
        let left = m.idle_seconds_left();
        self.idle_left = left.filter(|&l| l > 0 && l <= 60);
        let flash = self.idle_left.is_some() && self.settings.flash_before_idle_off && self.background;
        if flash != self.flashing
            && let Some(g) = &self.gpu
        {
            self.flashing = flash;
            g.window.request_user_attention(flash.then_some(winit::window::UserAttentionType::Critical));
        }
        // Keep a snapshot from just before the power-off, for Resume.
        match left {
            Some(left) if left <= 3 && left > 0 => {
                if s.idle_snapshot.is_none() {
                    s.idle_snapshot = Some(m.save_state());
                }
            }
            Some(left) if left > 3 => s.idle_snapshot = None,
            _ => {}
        }
        let off = m.idle_powered_off();
        let console = m.firmware().known().map_or("Leapster", |k| k.hardware.name());
        let timer = m.idle_timer();
        drop(m);
        if !off {
            return;
        }
        // The BIOS's own power-off animation is the fade-out: close the
        // moment the screen is black, so the library follows straight on.
        let since = *s.closing.get_or_insert_with(Instant::now);
        let light = self.frame.iter().map(|p| ((p >> 16) & 0xFF).max((p >> 8) & 0xFF).max(p & 0xFF) as u64).sum::<u64>();
        let black = light < 4 * self.frame.len() as u64;
        if !black && since.elapsed().as_secs_f32() < 1.5 {
            return;
        }
        tracing::info!(target: "ribbit::boot", "the console turned itself off after inactivity: closing the game");
        self.idle_resume = s.cart_path.clone().zip(s.idle_snapshot.take()).map(|(p, st)| (p, st, timer));
        self.ui.idle_closed = Some((s.title.clone(), console));
        self.end_session();
        self.update_title();
        self.rescan_library();
    }

    /// Reopens the game closed for inactivity where it was (or from its
    /// start if there is no snapshot), with a fresh idle timer.
    fn resume_after_idle(&mut self) {
        self.ui.idle_closed = None;
        let Some((path, state, timer)) = self.idle_resume.take() else {
            return;
        };
        self.open_cart(path);
        if let Some(s) = &self.session {
            {
                let mut m = s.emu.lock();
                if let Err(e) = m.load_state(&state) {
                    tracing::warn!(target: "ribbit::saves", "resume after inactivity: {e:#}");
                }
                // The snapshot may be in game code, where this fresh machine
                // cannot find the timer itself: give it the old one's.
                m.set_idle_timer(timer);
                m.reset_idle_count();
            }
            s.emu.resync();
            if let Some(a) = &mut self.audio {
                a.clear();
            }
        }
    }

    /// Turns HD capture on in `machine`; the canvas to draw into, or None
    /// if this BIOS's rasterizer is not recognised.
    fn start_hd(machine: &mut Machine, scale: usize) -> Option<ribbit_core::hd::HdCanvas> {
        machine.enable_hd_capture().then(|| ribbit_core::hd::HdCanvas::with_scale(machine.hd.as_ref().unwrap().hooks, scale))
    }

    /// HD canvas pixels per LCD pixel for the game's size on screen (one
    /// canvas pixel per screen pixel, 4x to 16x).
    fn hd_scale(&self) -> usize {
        let (_, _, w, h) = self.lcd_px;
        ((w.min(h) / 160.0).ceil() as usize).clamp(4, 16)
    }

    /// HD vectors on or off for the running game.
    fn set_hd(&mut self, on: bool) {
        let scale = self.hd_scale();
        let Some(s) = &mut self.session else {
            return;
        };
        let mut m = s.emu.lock();
        s.hd = if on { Self::start_hd(&mut m, scale) } else { None };
        if !on {
            m.disable_hd_capture();
        }
        drop(m);
        if on && s.hd.is_none() {
            self.ui.toast("HD vectors are not available with this BIOS");
        }
    }

    /// `scale`, reduced until the window fits on `screen` (16x is 2560
    /// pixels, bigger than most screens).
    fn fitting_scale(scale: u32, screen: Option<PhysicalSize<u32>>) -> u32 {
        let mut scale = if scale == 0 { 16 } else { scale };
        if let Some(screen) = screen {
            // Leave room for the title bar and the taskbar.
            while scale > 1 && (160 * scale > screen.width || 160 * scale + 56 + 120 > screen.height) {
                scale -= 1;
            }
        }
        scale
    }

    /// Sizes the window so the screen area (`area`, physical pixels) is
    /// exactly a multiple of `unit` (160 x 160 for the Leapster's screen,
    /// 160 x 120 for a TV's 4:3): no bars around the picture, whatever the
    /// menu and status bars measure at this display scaling. `scale` 0 =
    /// the largest that fits the monitor, as a normal window (never
    /// maximized), centred. Returns true once the area fits.
    fn fit_window(window: &Window, scale: u32, area: (f32, f32), unit: (u32, u32)) -> bool {
        if window.fullscreen().is_some() || window.is_maximized() || area.0 < 1.0 || area.1 < 1.0 {
            return true;
        }
        let (inner, outer) = (window.inner_size(), window.outer_size());
        // Rounded up: at fractional display scaling the bars can measure
        // a fraction of a pixel, which would leave the area just short.
        let chrome_w = inner.width.saturating_sub(area.0.floor() as u32);
        let chrome_h = inner.height.saturating_sub(area.1.floor() as u32);
        let frame = (outer.width.saturating_sub(inner.width), outer.height.saturating_sub(inner.height));
        let Some(monitor) = window.current_monitor() else {
            return true;
        };
        let (mon, at) = (monitor.size(), monitor.position());
        // The taskbar (not reported by winit): 48 points plus a margin.
        let taskbar = (56.0 * window.scale_factor()) as u32;
        let room_w = mon.width.saturating_sub(frame.0 + chrome_w);
        let room_h = mon.height.saturating_sub(frame.1 + chrome_h + taskbar);
        // Fit screen leaves some breathing room (about a tenth of the height).
        let room_h = if scale == 0 { room_h * 9 / 10 } else { room_h };
        let most = (room_w / unit.0).min(room_h / unit.1).max(1);
        let times = if scale == 0 { most } else { scale.min(most) };
        let want = PhysicalSize::new(unit.0 * times + chrome_w, unit.1 * times + chrome_h);
        // Never smaller than the window's minimum (the menu bar must fit).
        let min: PhysicalSize<u32> = MIN_WINDOW.to_physical(window.scale_factor());
        let want = PhysicalSize::new(want.width.max(min.width), want.height.max(min.height));
        tracing::debug!(target: "ribbit::window", ?inner, ?area, chrome_w, chrome_h, times, "fitting the window");
        if want == inner {
            return true;
        }
        let _ = window.request_inner_size(want);
        if scale == 0 {
            let (w, h) = ((want.width + frame.0) as i32, (want.height + frame.1) as i32);
            let x = at.x + (mon.width as i32 - w) / 2;
            let y = at.y + ((mon.height - taskbar) as i32 - h).max(0) / 2;
            window.set_outer_position(winit::dpi::PhysicalPosition::new(x, y));
        }
        false
    }

    fn window_size(scale: u32) -> PhysicalSize<u32> {
        // Room for the menu and status bars.
        PhysicalSize::new(160 * scale, 160 * scale + 56)
    }

    /// The library's BIOS picker: `path` becomes the BIOS `hw` plays on. For
    /// the original Leapster that is the File > Open BIOS choice itself.
    fn pick_bios(&mut self, hw: ribbit_memory::known::Hardware, path: PathBuf) {
        if hw == ribbit_memory::known::Hardware::Leapster {
            return self.set_bios(path);
        }
        let name = self.library.bios_list(hw).into_iter().find(|b| b.path == path).map(|b| b.short_name().to_string());
        self.settings.bios_picked.insert(hw.name().to_string(), path);
        self.save_settings();
        self.library.set_picked_bios(&self.settings.bios_picked);
        self.ui.toast(format!("{} BIOS: {}", hw.name(), name.unwrap_or_default()));
    }

    /// Deletes a Leapster game's save data: its cartridge save and its save
    /// states, moved into the backups folder. A running copy of the game is
    /// closed first (it would write its save back).
    fn delete_save_data(&mut self, game: &Path) {
        let Some(crc) = ribbit_core::read_image(game).ok().and_then(|d| ribbit_memory::Cartridge::from_bytes(d).ok()).map(|c| c.id().crc32)
        else {
            return self.ui.problem("Couldn't delete the save data", "The game file couldn't be read.", game.display().to_string());
        };
        let key = format!("cart-{crc:08x}");
        if self.session.as_ref().is_some_and(|s| s.state_key == key) {
            self.end_session();
        }
        let mut files = vec![self.save_dir().join(format!("{key}.sav"))];
        if let Ok(dir) = std::fs::read_dir(self.opts.data_dir.join("states")) {
            files.extend(
                dir.filter_map(|e| e.ok().map(|e| e.path()))
                    .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&format!("{key}-slot")))),
            );
        }
        let title = self.library.games.iter().find(|g| g.path == game).map_or_else(|| key.clone(), |g| g.title.clone());
        match crate::backup::move_to_deleted(&self.opts.data_dir, &files) {
            Ok((0, _)) => self.ui.toast(format!("{title} has no save data")),
            Ok((n, dest)) => {
                tracing::info!(target: "ribbit::saves", kept = %dest.display(), "deleted {n} save file(s) of {title}");
                self.ui.toast(format!("Save data of {title} deleted (a copy is in the backups folder)"));
            }
            Err(e) => self.ui.problem("Couldn't delete the save data", "A save file couldn't be moved.", format!("{e}")),
        }
    }

    fn state_path(&self, slot: u8) -> Option<PathBuf> {
        let key = match (&self.session, &self.zippity, &self.twist) {
            (Some(s), _, _) => s.state_key.clone(),
            (None, Some(_), _) => "zippity".to_string(),
            (None, None, Some(t)) if t.machine.is_some() => "rockit-twist".to_string(),
            _ => return None,
        };
        Some(self.opts.data_dir.join("states").join(format!("{key}-slot{slot}.state")))
    }

    /// When each slot was saved, for the menus.
    fn slot_times(&self) -> Vec<Option<String>> {
        (1..=ui::STATE_SLOTS)
            .map(|slot| {
                let modified = std::fs::metadata(self.state_path(slot)?).ok()?.modified().ok()?;
                let secs = modified.elapsed().ok()?.as_secs();
                Some(match secs {
                    0..=59 => "just now".to_string(),
                    60..=3599 => format!("{} min ago", secs / 60),
                    3600..=86_399 => format!("{} h ago", secs / 3600),
                    _ => format!("{} days ago", secs / 86_400),
                })
            })
            .collect()
    }

    fn save_state(&mut self, slot: u8) {
        let Some(path) = self.state_path(slot) else {
            return;
        };
        let bytes = match (&self.session, &self.zippity, &self.twist) {
            (Some(s), _, _) => s.emu.lock().save_state(),
            (None, Some(z), _) => z.save_state(),
            (None, None, Some(t)) => match t.machine.as_ref().map(|m| m.save_state()) {
                Some(Ok(b)) => b,
                Some(Err(e)) => return self.ui.problem("Couldn't save the state", "The RockIt Twist didn't answer.", e),
                None => return,
            },
            (None, None, None) => return,
        };
        let result = (|| -> Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.ui.toast(format!("State saved to slot {slot}")),
            Err(e) => self.ui.problem(
                "Couldn't save the state",
                format!("Slot {slot} wasn't saved. Check that the save folder still exists and the disk has room."),
                format!("{e:#}"),
            ),
        }
    }

    fn load_state(&mut self, slot: u8) {
        let Some(path) = self.state_path(slot) else {
            return;
        };
        let result = std::fs::read(&path).map_err(anyhow::Error::from).and_then(|bytes| match &self.session {
            Some(s) => s.emu.lock().load_state(&bytes).map_err(anyhow::Error::from),
            None => match (&self.zippity, self.twist.as_ref().and_then(|t| t.machine.as_ref())) {
                (Some(z), _) => z.load_state(&bytes).map_err(anyhow::Error::msg),
                (None, Some(m)) => m.load_state(bytes).map_err(anyhow::Error::msg),
                (None, None) => Ok(()),
            },
        });
        match result {
            Ok(()) => {
                if let Some(s) = &self.session {
                    s.emu.resync();
                }
                if let Some(a) = &mut self.audio {
                    a.clear();
                }
                self.ui.toast(format!("State loaded from slot {slot}"));
            }
            Err(e) => self.ui.problem(
                "Couldn't load the state",
                format!("Slot {slot} couldn't be loaded. It may be empty, damaged, or saved from another game. The game carries on where it was."),
                format!("{e:#}"),
            ),
        }
    }

    fn reset(&mut self) {
        if let Some(t) = &mut self.twist {
            t.restart(self.audio.as_ref());
            self.ui.toast("Reset");
        }
        if let Some(z) = &self.zippity {
            z.reset();
            self.ui.toast("Reset");
        }
        if let Some(s) = &mut self.session {
            {
                let mut m = s.emu.lock();
                let _ = s.saves.flush(&mut m);
                m.reset();
            }
            s.emu.resync();
            self.ui.toast("Reset");
        }
    }

    fn toggle_pause(&mut self) {
        if self.zippity.is_some() || self.twist.is_some() {
            self.paused = !self.paused;
            if let Some(a) = &mut self.audio {
                a.clear();
            }
            self.update_title();
        }
        if let Some(s) = &self.session {
            self.paused = !self.paused;
            s.emu.resync();
            if let Some(a) = &mut self.audio {
                a.clear();
            }
            self.update_title();
        }
    }

    /// Paused by the user, or by being in the background.
    fn halted(&self) -> bool {
        self.paused || (self.background && self.settings.pause_in_background)
    }

    /// The window became active (or not).
    fn focus_changed(&mut self, focused: bool) {
        let was = self.halted();
        self.background = !focused;
        if !focused {
            // Releases may never arrive while another window has the mouse
            // or the keys: nothing on the console view stays held.
            self.release_view();
            if let Some(t) = &mut self.twist {
                t.release_keys();
            }
        }
        if focused && self.flashing {
            self.flashing = false;
            if let Some(g) = &self.gpu {
                g.window.request_user_attention(None);
            }
        }
        if was != self.halted()
            && let Some(s) = &self.session
        {
            s.emu.resync();
            if let Some(a) = &mut self.audio {
                a.clear();
            }
        }
    }

    fn toggle_fullscreen(&mut self) {
        if let Some(g) = &self.gpu {
            let fs = if g.window.fullscreen().is_some() { None } else { Some(Fullscreen::Borderless(None)) };
            g.window.set_fullscreen(fs);
        }
    }

    fn screenshot(&mut self) {
        let dir = self.opts.data_dir.join("screenshots");
        let name = format!(
            "ribbit-{}.png",
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
        );
        let path = dir.join(name);
        let result = (|| -> Result<()> {
            std::fs::create_dir_all(&dir)?;
            let file = std::io::BufWriter::new(std::fs::File::create(&path)?);
            let scale = 4;
            let mut enc = png::Encoder::new(file, (WIDTH * scale) as u32, (HEIGHT * scale) as u32);
            enc.set_color(png::ColorType::Rgb);
            let mut w = enc.write_header()?;
            let mut data = Vec::with_capacity(WIDTH * HEIGHT * scale * scale * 3);
            for y in 0..HEIGHT * scale {
                for x in 0..WIDTH * scale {
                    let p = self.frame[(y / scale) * WIDTH + x / scale];
                    data.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
                }
            }
            w.write_image_data(&data)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.ui.toast(format!("Screenshot saved: {}", path.display())),
            Err(e) => self.ui.problem(
                "Couldn't save the screenshot",
                format!("Ribbit couldn't write {}. Check that the folder exists and the disk has room.", path.display()),
                format!("{e:#}"),
            ),
        }
    }

    /// Logs (every 5 s) how often the sound ran out.
    fn log_underruns(audio: Option<&AudioOut>, log: &mut (Instant, u64), speed_percent: f64) {
        let now = Instant::now();
        let Some(a) = audio else {
            return;
        };
        if now.duration_since(log.0).as_secs() >= 5 {
            let n = a.underruns();
            if n > log.1 {
                tracing::info!(target: "ribbit::audio", gaps = n - log.1, speed = speed_percent, "audio ran dry (last 5 s)");
            }
            let (queued, playing) = a.queue_state();
            tracing::debug!(target: "ribbit::audio", queued, playing, "audio queue");
            *log = (now, n);
        }
    }

    /// Per frame: tells the emulation thread whether and how fast to run,
    /// and does the UI-side work (rewind, sound, autosave, speed meter).
    fn emulate(&mut self) {
        let halted = self.halted();
        if self.zippity_cleaning.as_ref().is_some_and(|h| h.is_finished()) {
            match self.zippity_cleaning.take().unwrap().join() {
                Ok(Ok(dump)) => {
                    self.ui.toast("Saved players removed: the Zippity starts like a new one");
                    self.rescan_library();
                    self.start_zippity(dump.clone(), dump);
                }
                Ok(Err(e)) => self.ui.problem("Couldn't clear the old players", CLEAN_FAILED, format!("{e:#}")),
                Err(_) => self.ui.problem("Couldn't clear the old players", CLEAN_FAILED, "The cleaning thread crashed."),
            }
        }
        if self.twist_import.as_ref().is_some_and(|h| h.is_finished()) {
            match self.twist_import.take().unwrap().join() {
                Ok(Ok(done)) => {
                    let n = done.packs.len();
                    self.ui.toast(format!(
                        "Imported {n} RockIt Twist package{} ({}, {} files)",
                        if n == 1 { "" } else { "s" },
                        done.locale,
                        done.files
                    ));
                    self.set_twist_content(Some(done.dir));
                }
                Ok(Err(e)) => self.ui.problem("Couldn't import the RockIt Twist games", TWIST_IMPORT_FAILED, format!("{e:#}")),
                Err(_) => self.ui.problem("Couldn't import the RockIt Twist games", TWIST_IMPORT_FAILED, "The import thread crashed."),
            }
        }
        if let Some(t) = &mut self.twist {
            if !halted {
                t.tick();
            }
            if let Some(m) = &t.machine {
                m.set_running(!halted);
                m.set_gain(if self.settings.muted { 0.0 } else { self.settings.volume });
                match m.take_usb_news() {
                    Some(crate::twist::UsbNews::Attached) => self.ui.toast("RockIt Twist plugged into this PC"),
                    Some(crate::twist::UsbNews::Detached) => self.ui.toast("RockIt Twist unplugged from this PC"),
                    Some(crate::twist::UsbNews::Failed(e)) => {
                        self.ui.toast(format!("RockIt Twist USB: can't listen on port 3240 ({e})"))
                    }
                    None => {}
                }
            }
            self.speed_percent = t.machine.as_ref().map_or(0.0, |m| m.speed_percent());
            Self::log_underruns(self.audio.as_ref(), &mut self.audio_log, self.speed_percent);
            return;
        }
        if let Some(z) = &self.zippity {
            if z.take_user_data() && !self.settings.zippity_keep_saves.contains(&z.nand) {
                self.ui.zippity_clean = Some(z.nand.clone());
            }
            z.set_running(!halted);
            z.set_gain(if self.settings.muted { 0.0 } else { self.settings.volume });
            self.speed_percent = z.speed_percent();
            Self::log_underruns(self.audio.as_ref(), &mut self.audio_log, self.speed_percent);
            return;
        }
        let Some(s) = &mut self.session else {
            return;
        };
        let speed = if self.fast_forward || self.settings.speed_percent == 0 {
            f64::INFINITY
        } else {
            self.settings.speed_percent as f64 / 100.0
        };
        let gain = if self.settings.muted { 0.0 } else { self.settings.volume };
        if let Some(a) = &self.audio {
            a.set_enhance(self.settings.audio_enhance);
        }
        s.emu.set_control(Control {
            running: !halted && !self.rewinding,
            speed,
            gain,
            auto_calibrate: self.settings.auto_calibrate,
            pcm_32k: self.settings.pcm_rate == crate::audio_out::PcmRate::Original32k,
            pcm_clean: self.settings.pcm_clean == crate::audio_out::PcmClean::Clean,
        });
        if halted {
            return;
        }
        if self.rewinding {
            // Step back one snapshot per frame.
            if let Some(state) = s.rewind.pop_back() {
                let mut m = s.emu.lock();
                let _ = m.load_state(&state);
                s.last_snapshot = m.cpu.cycles;
            }
            s.emu.resync();
            if let Some(a) = &mut self.audio {
                a.clear();
            }
            return;
        }
        let now = Instant::now();
        while let Ok(state) = s.compress.done.try_recv() {
            s.rewind.push_back(state);
            if s.rewind.len() > REWIND_KEEP {
                s.rewind.pop_front();
            }
        }
        {
            let mut m = s.emu.lock();
            if m.cpu.cycles >= s.last_snapshot + REWIND_EVERY_CYCLES {
                let _ = s.compress.todo.send(m.snapshot());
                s.last_snapshot = m.cpu.cycles;
            }
            if let Err(e) = s.autosave.tick(&s.saves, &mut m, now) {
                tracing::warn!(target: "ribbit::saves", "autosave failed: {e:#}");
            }
        }
        // Sound goes from the emulation thread straight to the output (real-
        // time speed only); at other speeds drop what is queued.
        if speed != 1.0
            && let Some(a) = &mut self.audio
        {
            a.clear();
        }
        let stats = s.emu.stats();
        self.speed_percent = stats.speed_percent;
        Self::log_underruns(self.audio.as_ref(), &mut self.audio_log, self.speed_percent);
        if let Some(stop) = stats.stopped {
            s.emu.clear_stopped();
            self.paused = true;
            self.ui.problem(
                "The game stopped",
                "The emulated console did something Ribbit can't handle yet, so the game is paused. \
                 Reset (Emulation menu) or open another game. If it happens again at the same spot, \
                 the details below help track it down.",
                format!("{stop}"),
            );
        }
    }

    /// Whether a press at window position `p` (physical pixels) belongs to
    /// the UI. The game screen lives in egui's background panel, so only a
    /// menu, dialog or popup above it (or a widget being dragged) counts;
    /// `is_pointer_over_egui` alone is true over the whole game screen and
    /// swallowed every stylus click.
    fn ui_at(&self, p: (f64, f64)) -> bool {
        if self.egui_ctx.egui_is_using_pointer() {
            return true;
        }
        let ppp = self.egui_ctx.pixels_per_point();
        let pos = egui::pos2(p.0 as f32 / ppp, p.1 as f32 / ppp);
        if cfg!(target_os = "android") && pos.x <= crate::ui::SIDE_MENU_EDGE {
            return true;
        }
        self.egui_ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background)
    }

    /// The mouse wheel swipes the RockIt Twist's Slider (with the console
    /// view on, the view handles the wheel itself).
    fn wheel(&mut self, delta: winit::event::MouseScrollDelta) {
        if self.twist.is_none() || self.view_layout().is_some() || self.cursor.is_some_and(|p| self.ui_at(p)) {
            return;
        }
        const NOTCH: f64 = 40.0;
        self.wheel_px += match delta {
            winit::event::MouseScrollDelta::LineDelta(_, y) => y as f64 * NOTCH,
            winit::event::MouseScrollDelta::PixelDelta(p) => p.y,
        };
        while self.wheel_px.abs() >= NOTCH {
            let up = self.wheel_px > 0.0;
            self.wheel_px -= NOTCH.copysign(self.wheel_px);
            if let Some(t) = &mut self.twist {
                t.controls.swipe(up);
            }
        }
    }

    /// EXPERIMENTAL touch: the stylus at a window position (None = up).
    fn set_stylus(&mut self, pos: Option<(f64, f64)>) {
        let Some(s) = &mut self.session else {
            return;
        };
        let mut m = s.emu.lock();
        let Some(touch) = m.io.touch.as_mut() else {
            return;
        };
        let (x, y, w, h) = self.lcd_px;
        touch.live = pos.and_then(|(cx, cy)| {
            let px = ((cx as f32 - x) / w * WIDTH as f32).floor();
            let py = ((cy as f32 - y) / h * HEIGHT as f32).floor();
            let inside = (0.0..WIDTH as f32).contains(&px) && (0.0..HEIGHT as f32).contains(&py);
            // MAME's crosshair mapping (0..=0x7F0 across 160 pixels); the
            // BIOS's own calibration maps it back to the screen.
            let raw = |p: f32| ((p as u32 * 0x7F0 + 79) / 159).min(0x7F0);
            inside.then(|| (raw(px), raw(py)))
        });
    }

    fn key(&mut self, code: KeyCode, pressed: bool, event_loop: &ActiveEventLoop) {
        if pressed {
            self.hide_touch_pad();
        }
        if let Some(b) = self.ui.capturing {
            if pressed {
                if code != KeyCode::Escape {
                    self.settings.bind(b, code);
                    self.save_settings();
                }
                self.ui.capturing = None;
            }
            return;
        }
        if let Some(t) = self.ui.capturing_twist {
            if pressed {
                if code != KeyCode::Escape {
                    let name = crate::settings::key_name(code);
                    // A key does one thing: taken from any other action.
                    self.settings.twist_keys.retain(|_, k| *k != name);
                    self.settings.twist_keys.insert(crate::twist::action_name(t), name);
                    crate::twist::set_bindings(&self.settings.twist_keys);
                    self.save_settings();
                }
                self.ui.capturing_twist = None;
            }
            return;
        }
        let ctrl = self.modifiers.control_key();
        if pressed {
            match code {
                KeyCode::Escape => return self.toggle_pause(),
                KeyCode::F1 => return self.ui.show_controls = true,
                KeyCode::F10 => return self.ui.menu_visible = !self.ui.menu_visible,
                KeyCode::F11 => return self.toggle_fullscreen(),
                KeyCode::F12 => return self.screenshot(),
                KeyCode::F7 => return self.toggle_console_view(),
                KeyCode::F5 => return self.save_state(1),
                KeyCode::F9 => return self.load_state(1),
                KeyCode::KeyO if ctrl => return self.act(Action::Pick(Pick::Cartridge), event_loop),
                KeyCode::KeyR if ctrl => return self.reset(),
                _ => {}
            }
        }
        // The RockIt Twist has its own keys (twist::KEYS), Tab and Shift
        // included, while its session is open.
        if let Some(t) = &mut self.twist {
            t.key(code, pressed);
            return;
        }
        if code == KeyCode::Backquote {
            self.rewinding = pressed && self.session.is_some();
            return;
        }
        if code == KeyCode::Tab {
            self.fast_forward = pressed;
            if let Some(s) = &self.session {
                s.emu.resync();
            }
            return;
        }
        // Shift: stand on (or step off) the Zippity mat's home spot, unless
        // Shift is bound to a button.
        if matches!(code, KeyCode::ShiftLeft | KeyCode::ShiftRight)
            && self.settings.button_for(code).is_none()
            && let Some(z) = &self.zippity
        {
            if pressed {
                self.settings.zippity_on_home = !self.settings.zippity_on_home;
                z.set_on_home(self.settings.zippity_on_home);
                self.save_settings();
                self.ui.toast(if self.settings.zippity_on_home { "Standing on the home spot" } else { "Stepped off the home spot" });
            }
            return;
        }
        if let Some(b) = self.settings.button_for(code) {
            self.press(Source::Keyboard, b, pressed);
        }
    }

    /// Android: the game waits while the side menu is open, as in other
    /// emulators, and goes on when it closes (unless it was paused before).
    fn pause_for_side_menu(&mut self) {
        let open = self.ui.side_menu_open();
        let (was_open, paused_by_menu) = self.side_menu;
        if open && !was_open {
            let running = self.session.is_some() || self.zippity.is_some() || self.twist.is_some();
            let pause = running && !self.paused;
            if pause {
                self.toggle_pause();
            }
            self.side_menu = (true, pause);
        } else if !open && was_open {
            if paused_by_menu && self.paused {
                self.toggle_pause();
            }
            self.side_menu = (false, false);
        }
    }

    /// Android gamepad buttons (winit reports them as raw key codes; the
    /// D-pad usually arrives as arrow keys, its own codes are taken too).
    /// The layout matches the desktop's (`gamepad`, `twist::pad_button`).
    fn android_key(&mut self, code: u32, pressed: bool) {
        use ribbit_input::Button;
        // Back opens and closes the side menu (egui sees it as BrowserBack).
        const BACK: u32 = 4;
        if code == BACK {
            return;
        }
        if pressed {
            self.hide_touch_pad();
        }
        if let Some(t) = &mut self.twist {
            use crate::twist::TwistKey;
            use ribbit_twist::input::{Button as T, Rotary, Source as TwistSource};
            let key = match code {
                19 => TwistKey::Press(T::DpadUp),
                20 => TwistKey::Press(T::DpadDown),
                21 => TwistKey::Press(T::DpadLeft),
                22 => TwistKey::Press(T::DpadRight),
                96 | 23 => TwistKey::Press(T::Check), // BUTTON_A, DPAD_CENTER
                97 => TwistKey::Press(T::Hint),       // BUTTON_B
                100 => TwistKey::Press(T::Home),      // BUTTON_Y
                99 => TwistKey::Press(T::Grid(4)),    // BUTTON_X
                102 => TwistKey::Press(T::SwitchLeft),
                103 => TwistKey::Press(T::SwitchRight),
                104 => TwistKey::Turn(Rotary::Spinner, -1), // BUTTON_L2
                105 => TwistKey::Turn(Rotary::Spinner, 1),  // BUTTON_R2
                109 => TwistKey::Turn(Rotary::RedDial, -1), // BUTTON_SELECT
                108 => TwistKey::Turn(Rotary::RedDial, 1),  // BUTTON_START
                _ => return,
            };
            match key {
                TwistKey::Press(b) => t.controls.set(TwistSource::Gamepad, b, pressed),
                TwistKey::Turn(r, s) => t.controls.hold_turn(TwistSource::Gamepad, r, pressed.then_some(s)),
                TwistKey::Swipe(_) => {}
            }
            return;
        }
        let button = match code {
            19 => Button::Up,          // DPAD_UP
            20 => Button::Down,        // DPAD_DOWN
            21 => Button::Left,        // DPAD_LEFT
            22 => Button::Right,       // DPAD_RIGHT
            96 | 23 => Button::A,      // BUTTON_A, DPAD_CENTER
            97 => Button::B,           // BUTTON_B
            99 | 100 => Button::Hint,  // BUTTON_X / BUTTON_Y
            108 => Button::Pause,      // BUTTON_START
            109 => Button::Home,       // BUTTON_SELECT
            102 => Button::VolumeDown, // BUTTON_L1
            103 => Button::VolumeUp,   // BUTTON_R1
            _ => return,
        };
        self.press(Source::Gamepad, button, pressed);
    }

    /// Applies controller input (desktop; Android pads arrive as keys).
    #[cfg(not(target_os = "android"))]
    fn poll_pads(&mut self) {
        use crate::gamepad::PadEvent;
        let Some(pads) = &mut self.pads else {
            return;
        };
        let mut events = Vec::new();
        pads.poll(|e| events.push(e));
        if events.iter().any(|e| match e {
            PadEvent::Raw(_, down) | PadEvent::Button(_, down) => *down,
            PadEvent::RightStick(x, y) => *x != 0 || *y != 0,
            PadEvent::Menu => true,
            PadEvent::Connected(_) | PadEvent::Disconnected(_) => false,
        }) {
            self.hide_touch_pad();
        }
        for e in events {
            if let Some(t) = &mut self.twist {
                // The RockIt Twist maps the controller itself (twist::pad_button).
                use ribbit_twist::input::{Rotary, Source as TwistSource};
                match e {
                    PadEvent::Raw(b, pressed) => match crate::twist::pad_button(b) {
                        Some(crate::twist::TwistKey::Press(button)) => t.controls.set(TwistSource::Gamepad, button, pressed),
                        Some(crate::twist::TwistKey::Turn(r, s)) => {
                            t.controls.hold_turn(TwistSource::Gamepad, r, pressed.then_some(s))
                        }
                        _ => {}
                    },
                    PadEvent::RightStick(x, y) => {
                        t.controls.hold_turn(TwistSource::Gamepad, Rotary::YellowDial, (y != 0).then_some(y));
                        t.controls.hold_turn(TwistSource::Gamepad, Rotary::GreenDial, (x != 0).then_some(x));
                    }
                    PadEvent::Button(..) => {}
                    PadEvent::Menu => {
                        self.ui.menu_visible = true;
                        if !self.paused {
                            self.toggle_pause();
                        }
                    }
                    PadEvent::Connected(name) => self.ui.toast(format!("Controller connected: {name}")),
                    PadEvent::Disconnected(name) => self.ui.toast(format!("Controller disconnected: {name}")),
                }
                continue;
            }
            match e {
                PadEvent::Raw(..) | PadEvent::RightStick(..) => {}
                PadEvent::Button(b, pressed) => self.press(Source::Gamepad, b, pressed),
                PadEvent::Menu => {
                    self.ui.menu_visible = true;
                    if (self.session.is_some() || self.zippity.is_some() || self.twist.is_some()) && !self.paused {
                        self.toggle_pause();
                    }
                }
                PadEvent::Connected(name) => self.ui.toast(format!("Controller connected: {name}")),
                PadEvent::Disconnected(name) => self.ui.toast(format!("Controller disconnected: {name}")),
            }
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let frame_start = Instant::now();
        #[cfg(not(target_os = "android"))]
        self.poll_pads();
        self.emulate();
        self.check_idle_power_off();
        let (mut hd_frame, mut hd_dirty) = (false, false);
        if let Some(s) = &mut self.session {
            let passes = {
                let mut m = s.emu.lock();
                m.render_frame(&mut self.frame);
                m.hd.as_mut().map(|h| h.take()).unwrap_or_default()
            };
            // Automatic calibration is hidden: black instead of its targets.
            let hidden = self.settings.auto_calibrate && crate::calibrate::target(&self.frame).is_some();
            if hidden {
                self.frame.fill(0);
            }
            // EXPERIMENTAL HD: this frame's renders at the game's size on
            // screen (a new canvas when that changes; the fallback shows
            // until renders refill it).
            let scale = ((self.lcd_px.2.min(self.lcd_px.3) / 160.0).ceil() as usize).clamp(4, 16);
            if let Some(canvas) = &mut s.hd
                && canvas.scale != scale
                && let Some(hooks) = s.emu.lock().hd.as_ref().map(|h| h.hooks)
            {
                *canvas = ribbit_core::hd::HdCanvas::with_scale(hooks, scale);
                hd_dirty = true;
            }
            if let Some(canvas) = &mut s.hd
                && !hidden
            {
                if canvas.fonts.is_none() {
                    canvas.fonts = crate::device_fonts::get();
                }
                canvas.trace_pixels = self.settings.hd_trace_pixel_art;
                for p in &passes {
                    canvas.apply(p);
                }
                hd_dirty |= !passes.is_empty();
                // Traced pixel art goes into the canvas where the LCD would show.
                let from_lcd = canvas.update_mask(&self.frame[..]);
                hd_dirty |= canvas.trace_pixels && from_lcd > 0;
                hd_frame = true;
            }
        }
        if let Some(z) = &self.zippity {
            z.frame(&mut self.tv_frame);
        }
        if let Some(t) = &self.twist {
            t.frame(&mut self.tv_frame);
        }
        let halted = self.halted();
        let save_dir = self.save_dir().display().to_string();
        let slots = self.slot_times();
        let view_layout = self.view_layout();
        let view_available = self.current_hardware().map(|h| crate::console_view::layout_for(h).is_some());
        // A picture other than the Leapster's 160 x 160 LCD.
        let tv_size = if self.twist.is_some() {
            Some((ribbit_twist::SCREEN_W, ribbit_twist::SCREEN_H))
        } else if self.zippity.is_some() {
            Some((ribbit_zippity::SCREEN_W, ribbit_zippity::SCREEN_H))
        } else {
            None
        };
        let touch_pads = self.touch_pads();
        let (Some(gpu), Some(state)) = (&mut self.gpu, &mut self.egui_state) else {
            return;
        };
        gpu.upload(&self.frame);
        gpu.show_hd = hd_frame;
        gpu.show_tv = tv_size.is_some();
        if let Some(size) = tv_size {
            gpu.upload_tv(&self.tv_frame, size);
        }
        if let Some(c) = self.session.as_ref().and_then(|s| s.hd.as_ref()).filter(|_| hd_frame) {
            if hd_dirty {
                gpu.upload_hd(c.view(), c.size);
            }
            gpu.upload_hd_mask(&c.mask);
        }
        let raw = state.take_egui_input(&gpu.window);
        let info =
            ui::Info {
                running: self.session.is_some() || self.zippity.is_some() || self.twist.is_some(),
                paused: halted,
                background: !self.paused && halted,
                idle_left: self.idle_left,
                zippity: self.zippity.is_some(),
                zippity_on_home: self.settings.zippity_on_home,
                title: self
                    .session
                    .as_ref()
                    .map(|s| s.title.clone())
                    .or(self.zippity.as_ref().map(|z| z.title.clone()))
                    .or(self.twist.as_ref().map(|t| t.title.clone()))
                    .unwrap_or_default(),
                speed: self.speed_percent,
                bios_name: self.settings.bios.as_ref().map(|p| {
                    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
                }),
                fullscreen: gpu.window.fullscreen().is_some(),
                experimental_touch: self.session.as_ref().is_some_and(|s| s.experimental_touch),
                save_dir,
                slots,
                rewinding: self.rewinding,
                console_view_available: view_available,
            };
        let mut actions = Vec::new();
        let mut area = egui::Rect::NOTHING;
        self.library.poll();
        // At start the library opens on the page open last time (once the
        // first scan is in, so the page has its games); after that the open
        // page is remembered.
        if !self.tab_shown {
            if !self.library.scanning && !self.library.games.is_empty() {
                self.tab_shown = true;
                let last = crate::library::Family::ALL.into_iter().find(|f| f.name() == self.settings.library_tab);
                if self.ui.library.tab.is_none() {
                    self.ui.library.tab = last;
                }
            }
        } else {
            let open = self.ui.library.tab.map_or("", crate::library::Family::name);
            if open != self.settings.library_tab {
                self.settings.library_tab = open.to_string();
                if let Err(e) = self.settings.save(&self.settings_path) {
                    tracing::warn!(target: "ribbit::settings", "saving settings: {e:#}");
                }
            }
        }
        self.discord.set(match &self.session {
            _ if !self.settings.discord_presence => crate::discord::Presence::Off,
            Some(s) => crate::discord::Presence::playing(&s.title, s.system, s.started),
            None => match (&self.zippity, &self.twist) {
                (Some(z), _) => crate::discord::Presence::playing(&z.title, "Zippity", z.started),
                (None, Some(t)) => crate::discord::Presence::playing(&t.title, "RockIt Twist", t.started),
                (None, None) => crate::discord::Presence::Browsing,
            },
        });
        let (ui_state, settings, library) = (&mut self.ui, &mut self.settings, &mut self.library);
        let touch_pad = &self.touch_pad;
        let (view, held, twist) = (&mut self.console_view, &self.held, self.twist.as_ref());
        // Where the screen is inside the console view (None: the plain view).
        let mut screen = None;
        let mut view_events = Vec::new();
        let out = self.egui_ctx.run_ui(raw, |ui| {
            // The console is painted into this slot, under the menus and
            // messages drawn next.
            let under = ui.painter().add(egui::Shape::Noop);
            area = ui::draw(ui, ui_state, settings, library, &info, &mut actions);
            if let (true, Some(layout)) = (info.running, &view_layout) {
                let pressed = |t: Target| match t {
                    Target::Leapster(b) => held.pressed(b),
                    Target::Twist(b) => twist.is_some_and(|tw| tw.controls.pressed(b)),
                };
                let turned = |r| twist.map_or(0, |tw| tw.controls.turned(r));
                let turning = |r| twist.and_then(|tw| tw.controls.turning(r));
                let keys = |c: &crate::console_view::Control| control_keys(c, settings);
                let cap = |what| cap_label(what, settings);
                let lit = crate::console_view::Lit {
                    pressed: &pressed,
                    turned: &turned,
                    turning: &turning,
                    strip: twist.and_then(|t| t.controls.slider()),
                    swiping: twist.and_then(|t| t.controls.swiping()),
                    cap: &cap,
                    show_caps: settings.console_view_keys,
                    keys: &keys,
                };
                let (at, events) = view.show(ui, under, layout, area, &lit);
                screen = Some(at);
                view_events = events;
            }
            if let Some(tw) = twist {
                let at = screen.unwrap_or_else(|| {
                    let size = (ribbit_twist::SCREEN_W, ribbit_twist::SCREEN_H);
                    let (x, y, w, h) = Gpu::tv_rect((area.min.x, area.min.y, area.width(), area.height()), size);
                    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(w, h))
                });
                crate::twist::overlay(ui.painter(), at, tw);
            }
            if let (true, Some(pads)) = (info.running, &touch_pads) {
                crate::touch_pad::paint(ui.painter(), pads, touch_pad);
            }
        });
        self.touch_area = area;
        state.handle_platform_output(&gpu.window, out.platform_output);
        let ppp = out.pixels_per_point;
        if self.fit_window > 0 {
            self.fit_window -= 1;
            let unit = match &view_layout {
                // The whole console: sized so its screen gets the scale.
                Some(l) => {
                    let h = 160.0 / l.screen_aspect / l.screen.height() * 1.04;
                    ((h * l.aspect).round() as u32, h.round() as u32)
                }
                None if tv_size.is_some() => (160, 120),
                None => (160, 160),
            };
            if Self::fit_window(&gpu.window, self.settings.window_scale, (area.width() * ppp, area.height() * ppp), unit) {
                self.fit_window = 0;
            }
        }
        let display = self.settings.display;
        let at = screen.unwrap_or(area);
        let px_area = (at.min.x * ppp, at.min.y * ppp, at.width() * ppp, at.height() * ppp);
        self.lcd_px = match tv_size {
            Some(size) => Gpu::tv_rect(px_area, size),
            None => Gpu::lcd_rect(px_area, display),
        };
        let jobs = self.egui_ctx.tessellate(out.shapes, ppp);
        let lcd = (self.session.is_some() || self.zippity.is_some() || self.twist.is_some())
            .then_some((self.lcd_px, display, self.settings.ai));
        let mut deltas = out.textures_delta;
        let rendered = gpu.render(lcd, &jobs, &deltas, ppp);
        // Handled by `render` (debug builds of egui panic when an unhandled
        // delta is dropped).
        deltas.clear();
        if let Err(e) = rendered {
            self.fatal = Some(e);
            event_loop.exit();
            return;
        }
        self.apply_view_events(view_events);
        for a in actions {
            self.act(a, event_loop);
        }
        self.pause_for_side_menu();
        let took = frame_start.elapsed();
        if took.as_millis() >= 12 {
            tracing::debug!(target: "ribbit::frame", ms = took.as_secs_f64() * 1e3, "slow frame (excluding vsync wait)");
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // Back from the background (Android): a new window for the GPU kept
        // from before, with every texture still on it.
        if let Some(gpu) = &mut self.gpu {
            if self.egui_state.is_some() {
                return;
            }
            let reattached = event_loop
                .create_window(Window::default_attributes().with_title(APP_NAME))
                .context("creating window")
                .and_then(|w| gpu.reattach(Arc::new(w)));
            match reattached {
                Ok(()) => {
                    self.egui_state = Some(egui_winit::State::new(
                        self.egui_ctx.clone(),
                        egui::ViewportId::ROOT,
                        &gpu.window,
                        Some(gpu.window.scale_factor() as f32),
                        None,
                        None,
                    ));
                    gpu.window.request_redraw();
                }
                Err(e) => {
                    self.fatal = Some(e);
                    event_loop.exit();
                }
            }
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(APP_NAME)
            // Never maximized or full screen at start: a normal window, the
            // chosen size or the largest that fits the screen.
            .with_inner_size(Self::window_size(Self::fitting_scale(
                self.settings.window_scale,
                event_loop.primary_monitor().map(|m| m.size()),
            )))
            .with_min_inner_size(MIN_WINDOW)
            .with_window_icon(crate::icon::window_icon());
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows as _;
            attrs.with_taskbar_icon(crate::icon::taskbar_icon())
        };
        match event_loop.create_window(attrs).context("creating window").and_then(|w| Gpu::new(Arc::new(w))) {
            Ok(gpu) => {
                self.fit_window = FIT_TRIES;
                let state = egui_winit::State::new(
                    self.egui_ctx.clone(),
                    egui::ViewportId::ROOT,
                    &gpu.window,
                    Some(gpu.window.scale_factor() as f32),
                    None,
                    None,
                );
                self.egui_state = Some(state);
                self.gpu = Some(gpu);
                self.update_title();
            }
            Err(e) => {
                self.fatal = Some(e);
                event_loop.exit();
            }
        }
    }

    fn suspended(&mut self, _event_loop: &ActiveEventLoop) {
        // Android: the surface goes away when the app is backgrounded.
        if let Some(s) = &mut self.session {
            let _ = s.saves.flush(&mut s.emu.lock());
            s.emu.resync();
        }
        self.save_settings();
        // Only the surface goes (the window is being taken away); the GPU
        // and its textures (the UI's fonts, the game art) stay for when the
        // app comes back, which used to find them gone and draw nothing.
        if let Some(g) = &mut self.gpu {
            g.detach();
        }
        self.egui_state = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // (`ui_at` decides whether a press belongs to the UI or the stylus.)
        let consumed = match (&mut self.egui_state, &self.gpu) {
            (Some(st), Some(g)) => st.on_window_event(&g.window, &event).consumed,
            _ => false,
        };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            WindowEvent::Focused(f) => self.focus_changed(f),
            WindowEvent::DroppedFile(path) => self.open_dropped(path),
            WindowEvent::KeyboardInput { event: KeyEvent { physical_key, state, repeat: false, .. }, .. } => {
                if consumed && self.egui_ctx.egui_wants_keyboard_input() {
                    return;
                }
                let pressed = state == ElementState::Pressed;
                match physical_key {
                    PhysicalKey::Code(code) => self.key(code, pressed, event_loop),
                    PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Android(code)) => {
                        self.android_key(code, pressed)
                    }
                    _ => {}
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if self.mouse_down {
                    self.set_stylus(self.cursor);
                }
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                let down = state == ElementState::Pressed;
                if down && self.cursor.is_some_and(|p| self.ui_at(p)) {
                    return;
                }
                self.mouse_down = down;
                self.set_stylus(if down { self.cursor } else { None });
            }
            WindowEvent::Touch(t) => {
                if self.touch_controls_event(&t) {
                    return;
                }
                let pos = (t.location.x, t.location.y);
                match t.phase {
                    TouchPhase::Started if self.finger.is_none() && !self.ui_at(pos) => {
                        self.finger = Some((t.id, pos));
                        self.set_stylus(Some(pos));
                    }
                    TouchPhase::Moved if self.finger.is_some_and(|(id, _)| id == t.id) => {
                        self.finger = Some((t.id, pos));
                        self.set_stylus(Some(pos));
                    }
                    TouchPhase::Ended | TouchPhase::Cancelled if self.finger.is_some_and(|(id, _)| id == t.id) => {
                        self.finger = None;
                        self.set_stylus(None);
                    }
                    _ => {}
                }
            }
            WindowEvent::MouseWheel { delta, .. } => self.wheel(delta),
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        // Android: only asked in the background, as the activity is only
        // destroyed after its window has gone.
        if self.egui_state.is_none() && self.opts.destroyed.is_some_and(|destroyed| destroyed()) {
            event_loop.exit();
            return;
        }
        #[cfg(target_os = "macos")]
        self.poll_sheet();
        self.poll_native_pick();
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // macOS's Quit (⌘Q, the app menu) ends the process right after this,
        // without returning from run_app: save here, not only after it.
        self.shutdown();
    }
}

/// A worker thread that compresses rewind snapshots.
struct Compressor {
    todo: std::sync::mpsc::Sender<ribbit_core::RawState>,
    done: std::sync::mpsc::Receiver<Vec<u8>>,
}

impl Compressor {
    fn start() -> Self {
        let (todo, jobs) = std::sync::mpsc::channel::<ribbit_core::RawState>();
        let (results, done) = std::sync::mpsc::channel();
        std::thread::Builder::new()
            .name("ribbit-rewind".into())
            .spawn(move || {
                for raw in jobs {
                    if results.send(raw.compress()).is_err() {
                        break;
                    }
                }
            })
            .expect("spawning the rewind worker");
        Self { todo, done }
    }
}

/// The key a console-view keycap shows (None: no key).
fn cap_label(what: crate::console_view::CapFor, settings: &Settings) -> Option<String> {
    use crate::console_view::CapFor;
    use crate::twist::{TwistKey, cap_for};
    match what {
        CapFor::Press(Target::Leapster(b)) => settings.key_for(b).map(crate::settings::cap_text),
        CapFor::Press(Target::Twist(b)) => cap_for(TwistKey::Press(b)),
        CapFor::Turn(r, s) => cap_for(TwistKey::Turn(r, s)),
        CapFor::Swipe(up) => cap_for(TwistKey::Swipe(up)),
    }
}

/// The keys for a console-view control, for its tooltip.
fn control_keys(c: &crate::console_view::Control, settings: &Settings) -> String {
    use crate::console_view::Action as A;
    use crate::twist::{TwistKey, keys_for};
    let leapster = |b| settings.key_for(b).map(crate::settings::key_label).unwrap_or_default();
    let twist = |t: Target| match t {
        Target::Twist(b) => keys_for(TwistKey::Press(b)),
        Target::Leapster(b) => leapster(b),
    };
    let join = |v: Vec<String>| v.into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" / ");
    match c.action {
        A::Press(t) => twist(t),
        A::Dpad(ts) => join(ts.into_iter().map(twist).collect()),
        A::Rocker(ts) => join(ts.into_iter().map(twist).collect()),
        A::Wheel { rotary, .. } => {
            let keys = join(vec![keys_for(TwistKey::Turn(rotary, 1)), keys_for(TwistKey::Turn(rotary, -1))]);
            format!("{keys}; drag, click an end or scroll")
        }
        A::Strip => "Page Up / Page Down; drag or scroll".to_string(),
    }
}

/// "Game (USA, alt) [500-11002 / 500-01168] (MAME: x)" -> "Game (USA, alt)".
pub(crate) fn short_name(full: &str) -> String {
    full.split(" [").next().unwrap_or(full).trim().to_string()
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_names_drop_catalogue_details() {
        assert_eq!(
            super::short_name(
                "SpongeBob SquarePants - Saves the Day (USA, alt) [500-11002 / 500-01168] (MAME: spongeba)"
            ),
            "SpongeBob SquarePants - Saves the Day (USA, alt)"
        );
        assert_eq!(super::short_name("Plain"), "Plain");
    }
}

/// What the Zippity cleaning failure means for the user's file.
const TWIST_IMPORT_FAILED: &str = "Nothing changed on the emulated RockIt Twist. The zip should hold RockIt Twist      package folders, each with its meta.inf, as they are installed on the console.";

const CLEAN_FAILED: &str = "The players are still there. Nothing was lost: the dump is either unchanged or \
                            kept beside it as a .with-saves.bak file.";

/// A path's file name, for messages.
fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// A title and a plain explanation for a game or BIOS that didn't load.
fn explain_load_error(e: &anyhow::Error) -> (String, String) {
    use ribbit_core::LoadError;
    use ribbit_memory::CartridgeError;
    let Some(load) = e.downcast_ref::<LoadError>() else {
        // Messages written for people already (such as "Choose a BIOS first").
        return ("Couldn't start the game".to_string(), e.to_string());
    };
    match load {
        LoadError::Cartridge { path, source } => {
            let name = file_name(path);
            let explain = match source {
                CartridgeError::TooLarge { size, max } => format!(
                    "\"{name}\" is {} MB, but a Leapster cartridge holds at most {} MB, so it isn't a Leapster game. \
                     It may be a dump of something else, such as a console's own memory. \
                     Games Ribbit recognises are listed in the library.",
                    size.div_ceil(1 << 20),
                    max >> 20
                ),
                CartridgeError::Empty => format!("\"{name}\" is empty. It may not have finished copying or downloading."),
                CartridgeError::OddSize(_) => format!(
                    "\"{name}\" has an odd number of bytes, so it can't be a whole cartridge dump. It may be cut short or damaged."
                ),
            };
            ("This file isn't a Leapster game".to_string(), explain)
        }
        LoadError::Firmware { path, .. } => (
            "This BIOS can't be used".to_string(),
            format!(
                "\"{}\" isn't a console BIOS dump Ribbit can use. Choose another one with File → Open BIOS….",
                file_name(path)
            ),
        ),
        LoadError::Io { path, .. } => (
            "Couldn't read the file".to_string(),
            format!(
                "\"{}\" couldn't be opened. It may have been moved, renamed or deleted, or another program may be using it.",
                file_name(path)
            ),
        ),
        LoadError::Zip { path, .. } => (
            "Couldn't unpack the zip file".to_string(),
            format!("\"{}\" is a .zip archive Ribbit couldn't unpack. It may be damaged or hold no game.", file_name(path)),
        ),
    }
}
