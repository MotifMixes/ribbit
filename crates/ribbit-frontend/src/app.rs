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
use std::time::{Duration, Instant};
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
/// In the data folder: how far the last starts got setting up the graphics card (see `Gpu::connect`).
const GPU_GUARD: &str = "gpu-setup-crashed.txt";

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
    /// Android: keeps the screen from sleeping (true) or lets it (false).
    pub keep_screen_on: Option<fn(bool)>,
}

struct Session {
    /// Debug: RIBBIT_IDLE_TEST_SECS (see `check_idle_power_off`).
    idle_test_secs: Option<u32>,
    /// A save state from just before an idle power-off (for Resume).
    idle_snapshot: Option<Vec<u8>>,
    /// When the BIOS began its power-off animation (it fades the screen to
    /// black in about 0.75 s); the game closes as soon as it is black.
    closing: Option<Instant>,
    /// A save state was successfully loaded this session: skip the shutdown
    /// animation on close (the game may be mid-state, not at a clean exit).
    loaded_save_state: bool,
    /// The game has no save yet (when it started): `first_start_notice`
    /// says so once the game begins writing one.
    first_start: Option<Instant>,
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
    /// A Didj being played.
    didj: Option<crate::didj::DidjSession>,
    /// Its latest picture.
    tv_frame: Vec<u32>,
    /// Removing the saved players from a Zippity dump (the cleaned dump).
    zippity_cleaning: Option<std::thread::JoinHandle<anyhow::Result<PathBuf>>>,
    /// A zip of RockIt Twist packages being unpacked.
    twist_import: Option<std::thread::JoinHandle<anyhow::Result<crate::twist::Imported>>>,
    /// The Leapster 2's USB/IP server, once started (its port stays bound
    /// for the rest of the program).
    leapster2_usb: Option<crate::emu::UsbServer>,
    /// A RockIt Twist session (early emulation), instead of the others.
    twist: Option<crate::twist::TwistSession>,
    /// An original LeapPad being played.
    leappad: Option<crate::leappad::LeapPadSession>,
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
    /// The debugger's breakpoints (code addresses), for whichever console runs.
    debug_breakpoints: Vec<u32>,
    /// The debugger's own windows (debugger.rs).
    popouts: crate::debugger::Popouts,
    /// Ribbit's window has the keyboard. With the debugger's windows', it
    /// decides whether the game is in the background (`about_to_wait`).
    main_focused: bool,
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
    audio_log: (Instant, (u64, u64)),
    /// Window frame statistics for the log (`FrameDiag`).
    frame_diag: FrameDiag,
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
    /// When the window was last drawn, and when egui asked to draw it
    /// again (None: not until something happens); `woken`: an event came
    /// in since (see `next_redraw`).
    drawn_at: Instant,
    repaint_at: Option<Instant>,
    woken: bool,
    /// The status line's period: since when, and frames drawn (`log_status`).
    status: (Instant, u32),
    /// The running game's own settings are on: its name and the player's
    /// usual settings (see game_settings.rs).
    game_own: Option<(String, Settings)>,
    /// macOS: the open panel showing as a sheet, until it is answered.
    #[cfg(target_os = "macos")]
    sheet: Option<crate::files::SheetPick>,
    /// Android: what the system's file picker, open now, is choosing.
    native_pick: Option<Pick>,
    /// Android: the side menu was open last frame, and whether opening it
    /// paused the game (closing it then lets it go on).
    side_menu: (bool, bool),
    /// Android: the screen is being kept on (a game is playing).
    screen_on: bool,
    fatal: Option<anyhow::Error>,
    quit: bool,
    /// Everything is saved for good: nothing more is written (after an
    /// update's restart the new version owns the settings and saves).
    shut_down: bool,
    /// Waiting for the console's power-off animation to finish before closing
    /// the session (triggered by CloseCartridge with close_with_shutdown on).
    closing_animation: bool,
    /// The game closed for inactivity, its snapshot, and where its BIOS
    /// keeps the idle timer (for Resume).
    idle_resume: Option<(PathBuf, Vec<u8>, Option<ribbit_core::idle_timer::IdleTimer>)>,
    /// The library has opened on the chosen console's page (once, when its
    /// first scan is in).
    tab_shown: bool,
    /// Active screen recording, or None.
    recording: Option<crate::record::Recorder>,
    /// Background thread encoding the last finished recording, or None.
    encoding: Option<std::thread::JoinHandle<anyhow::Result<crate::record::Saved>>>,
    /// The default recordings folder (Videos), looked up once.
    record_default: PathBuf,
    /// Scratch buffer for audio samples drained each frame while recording.
    rec_audio: Vec<i16>,
    /// Cheat / memory search state.
    cheat: crate::cheat::CheatSearch,
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
            // "Animate game shutdown" was on by default before 1.4 and kept
            // in settings: most games take 6-15 s to answer the power button
            // (or never do), so closing froze for its 3 s limit. Off once,
            // on the update to 1.5.1.
            if older_than(&settings.last_version, (1, 5, 1)) && settings.close_with_shutdown {
                tracing::info!(target: "ribbit::settings", "Animate game shutdown turned off (closing waited 3 s)");
                settings.close_with_shutdown = false;
            }
            settings.last_version = crate::backup::VERSION.to_string();
            if let Err(e) = settings.save(&settings_path) {
                tracing::warn!(target: "ribbit::settings", "saving settings: {e:#}");
            }
        }
        // What an in-app update moved aside.
        crate::install::remove_leftovers();
        crate::twist::set_bindings(&settings.twist_keys);
        if let Some(f) = &opts.firmware {
            // A LeapPad base ROM is the LeapPad's; the Leapster's BIOS stays.
            if std::fs::read(f).is_ok_and(|d| ribbit_leappad::identify(&d) == Some(ribbit_leappad::Image::BaseRom)) {
                settings.leappad_bios = Some(f.clone());
            } else {
                settings.bios = Some(f.clone());
            }
        }
        let mut app = Self {
            opts,
            settings,
            settings_path,
            session: None,
            zippity: None,
            didj: None,
            tv_frame: Vec::new(),
            zippity_cleaning: None,
            twist_import: None,
            leapster2_usb: None,
            twist: None,
            leappad: None,
            console_view: ConsoleView::default(),
            held: Held::default(),
            wheel_px: 0.0,
            gpu: None,
            egui_ctx: egui_context(),
            egui_state: None,
            ui: UiState::new(),
            frame: Box::new([0; WIDTH * HEIGHT]),
            paused: false,
            debug_breakpoints: Vec::new(),
            popouts: Default::default(),
            main_focused: true,
            next_bios: None,
            background: false,
            flashing: false,
            fast_forward: false,
            rewinding: false,
            modifiers: ModifiersState::empty(),
            speed_percent: 100.0,
            audio_log: (Instant::now(), (0, 0)),
            frame_diag: FrameDiag::default(),
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
            drawn_at: Instant::now(),
            repaint_at: None,
            woken: true,
            status: (Instant::now(), 0),
            game_own: None,
            #[cfg(target_os = "macos")]
            sheet: None,
            native_pick: None,
            side_menu: (false, false),
            screen_on: false,
            library: Library::new(&opts_data_dir),
            discord: crate::discord::Discord::start(),
            fatal: None,
            quit: false,
            shut_down: false,
            closing_animation: false,
            idle_resume: None,
            tab_shown: false,
            recording: None,
            encoding: None,
            record_default: crate::record::default_folder(&opts_data_dir),
            rec_audio: Vec::new(),
            cheat: crate::cheat::CheatSearch::default(),
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
        // RIBBIT_DEBUG_WINDOWS: the debugger's windows open at start (for
        // checking them).
        // Android's app has one window: the debugger is drawn inside it.
        app.ui.debug.in_app = cfg!(target_os = "android");
        if std::env::var_os("RIBBIT_DEBUG_WINDOWS").is_some() {
            app.ui.debug.show_memory = true;
            app.ui.debug.show_cpu = true;
        }
        app
    }

    fn save_dir(&self) -> PathBuf {
        self.settings.save_dir.clone().unwrap_or_else(|| self.opts.data_dir.join("saves"))
    }

    fn save_settings(&mut self) {
        if let Err(e) = self.usual_settings().save(&self.settings_path) {
            tracing::warn!(target: "ribbit::settings", "saving settings: {e:#}");
        }
    }

    /// The settings as kept in settings.toml: the player's usual ones, with
    /// the running game's own (see game_settings.rs) under its name.
    fn usual_settings(&mut self) -> Settings {
        Self::usual_of(&mut self.game_own, &mut self.settings)
    }

    /// `usual_settings` from the two fields (for when the window is borrowed).
    fn usual_of(game_own: &mut Option<(String, Settings)>, settings: &mut Settings) -> Settings {
        let Some((key, usual)) = game_own else {
            return settings.clone();
        };
        let (mut back, own) = crate::game_settings::split(usual, settings);
        back.game_settings.insert(key.clone(), own);
        settings.game_settings = back.game_settings.clone();
        *usual = back.clone();
        back
    }

    /// A game with its own settings starts: they go on top of the usual ones.
    fn enter_game_settings(&mut self, cart: Option<&Path>) {
        let Some(key) = cart.and_then(crate::game_settings::key) else {
            return;
        };
        let Some(own) = self.settings.game_settings.get(&key) else {
            return;
        };
        tracing::info!(target: "ribbit::settings", "{key}: its own settings ({} differ)", own.len());
        let playing = crate::game_settings::apply(&self.settings, own);
        let usual = std::mem::replace(&mut self.settings, playing);
        if usual.window_scale != self.settings.window_scale {
            self.fit_window = FIT_TRIES;
        }
        self.game_own = Some((key, usual));
    }

    /// The game closes: the usual settings are back (its own are kept).
    fn leave_game_settings(&mut self) {
        if self.game_own.is_none() {
            return;
        }
        let usual = self.usual_settings();
        self.game_own = None;
        if usual.window_scale != self.settings.window_scale {
            self.fit_window = FIT_TRIES;
        }
        self.settings = usual;
        self.save_settings();
    }

    /// "Custom settings for this game" ticked or unticked.
    fn set_game_settings(&mut self, on: bool) {
        let Some(key) = self.session.as_ref().and_then(|s| s.cart_path.as_deref()).and_then(crate::game_settings::key) else {
            return;
        };
        if on {
            if self.game_own.is_none() {
                self.settings.game_settings.entry(key.clone()).or_default();
                self.game_own = Some((key, self.settings.clone()));
                self.ui.toast("Changes to the picture, sound, speed and controls are now for this game only");
            }
        } else if self.game_own.is_some() {
            let hd = self.settings.hd_vectors;
            self.leave_game_settings();
            self.settings.game_settings.remove(&key);
            if hd != self.settings.hd_vectors {
                self.set_hd(self.settings.hd_vectors);
            }
            self.ui.toast("This game uses your usual settings again");
        }
        self.save_settings();
    }

    /// Starts (or restarts) the machine with the BIOS and `cart`.
    fn boot(&mut self, cart: Option<PathBuf>) -> Result<()> {
        let started = self.boot_game(cart);
        // Not started: the usual settings again.
        if started.is_err() {
            self.leave_game_settings();
        }
        started
    }

    fn boot_game(&mut self, cart: Option<PathBuf>) -> Result<()> {
        self.end_session();
        self.enter_game_settings(cart.as_deref());
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
        // Disable the JIT if the setting says so (for debugging; takes effect
        // at this boot, not requiring a full restart of Ribbit).
        if !self.settings.jit_enabled {
            machine.jit = None;
        }
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
        if saves.sd.is_some() {
            saves.sd_folder = self.settings.leapster2_sd_folder.clone().map(|d| Arc::new(crate::sdcard::Folder::new(d)));
        }
        if let Some(p) = &self.opts.system_eeprom {
            saves.system = Some(p.clone());
        }
        if let Some(p) = &self.opts.cart_save
            && saves.cart.is_some()
        {
            saves.cart = Some(p.clone());
        }
        // No save yet: a game may set one up on its first start, which can
        // take a while behind a black screen (Pet Pals: about 20 s, as on
        // the console; docs/compatibility.md).
        let first_start = saves.cart.as_ref().is_some_and(|p| !p.exists());
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
            loaded_save_state: false,
            first_start: first_start.then(Instant::now),
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
        self.apply_leapster2_usb();
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
        if let Some(lp) = self.leappad.take() {
            lp.stop();
        }
        if let Some(z) = self.zippity.take() {
            z.stop();
            // Back to the Leapster's square screen.
            self.fit_window = FIT_TRIES;
        }
        if let Some(d) = self.didj.take() {
            d.stop();
            self.fit_window = FIT_TRIES;
        }
        if let Some(s) = self.session.take() {
            // How long closing takes, for "the library takes a while to
            // come back" reports.
            let t = Instant::now();
            let mut machine = s.emu.stop();
            let stopped = t.elapsed();
            if let Err(e) = s.saves.flush(&mut machine) {
                tracing::warn!(target: "ribbit::saves", "saving: {e:#}");
            }
            tracing::info!(target: "ribbit", stop_ms = stopped.as_millis() as u64, saves_ms = (t.elapsed() - stopped).as_millis() as u64, "game closed");
        }
        self.leave_game_settings();
        self.update_title();
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
        // A LeapPad base ROM: kept as the LeapPad's; the Leapster's BIOS stays.
        if std::fs::read(&path).is_ok_and(|d| ribbit_leappad::identify(&d) == Some(ribbit_leappad::Image::BaseRom)) {
            self.settings.leappad_bios = Some(path);
            self.save_settings();
            self.ui.toast("BIOS: LeapPad base ROM");
            return;
        }
        // A Zippity's system NAND (as dropping it on the window takes it):
        // kept with the BIOSes opened, as the Zippity's; the Leapster's BIOS
        // stays as it is.
        if crate::library::is_internal(&path) {
            self.settings.opened_bios.retain(|p| p != &path);
            self.settings.opened_bios.insert(0, path);
            self.save_settings();
            self.ui.toast("BIOS: Zippity");
            self.rescan_library();
            return;
        }
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
        // The Zippity with no cartridge: its built-in games.
        if hw == Hardware::Zippity {
            return self.start_zippity(bios.clone(), bios);
        }
        self.next_bios = Some(bios);
        if let Err(e) = self.boot(None) {
            let (title, explain) = explain_load_error(&e);
            self.ui.problem(title, explain, format!("{e:#}"));
        }
    }

    /// Plays a game on the console chosen in the library's console menu
    /// (unless a BIOS was given on the command line).
    fn open_cart(&mut self, path: PathBuf) {
        // A Didj cartridge: the Didj from the library, with it in the slot.
        if let Some(cart) = crate::didj::read_cart(&path) {
            // Before the library is scanned: packages beside the cartridge.
            let beside = || std::fs::read_dir(path.parent()?).ok()?.flatten().map(|e| e.path()).find(|p| crate::didj::is_package(p));
            let system = self.library.games.iter().find(|g| g.family == crate::library::Family::Didj && g.has_tag("System")).map(|g| g.path.clone());
            let Some(package) = system.or_else(beside) else {
                return self.ui.problem(
                    "No Didj in the library",
                    "A Didj cartridge plays in a Didj built from its firmware packages (DIDJ-0x….lfp files). Add the folder that holds them with \"Add games folder\".",
                    "",
                );
            };
            self.start_didj(package, Some(cart));
            if let (Some(d), Some(g)) = (&mut self.didj, self.library.games.iter().find(|g| g.path == path)) {
                d.title.clone_from(&g.title);
                self.update_title();
            }
            return;
        }
        // A LeapPad cartridge (or its base ROM): the LeapPad.
        match std::fs::read(&path).ok().and_then(|d| ribbit_leappad::identify(&d)) {
            Some(ribbit_leappad::Image::Cartridge) => return self.start_leappad(Some(path)),
            Some(ribbit_leappad::Image::BaseRom) => {
                self.set_bios(path);
                return self.start_leappad(None);
            }
            None => {}
        }
        // A Didj package: the Didj, built from the packages beside it.
        if crate::didj::is_package(&path) {
            return self.start_didj(path, None);
        }
        // A console's built-in games: the Zippity's NAND, or a Leapster
        // BIOS's own games (that BIOS with no cartridge in).
        let internal = self.library.games.iter().find(|g| g.path == path && g.has_tag("Internal"));
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
                // The page stays when it lists the game (the Leapster 2's).
                if !self.ui.library.tab.is_none_or(|t| t.lists(family)) {
                    self.ui.library.tab = Some(self.library.tab_for(family.console()));
                }
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
                        .find(|g| g.path == nand && g.has_tag("Internal"))
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

    /// Plays a Didj built from the packages in the folder of `package` (the
    /// one chosen in the library), with `cart` in its slot. It starts at its
    /// menu, whichever package was chosen.
    fn start_didj(&mut self, package: PathBuf, cart: Option<Vec<u8>>) {
        self.end_session();
        let dir = package.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let sink = self.audio.as_ref().map(|a| a.sink());
        match crate::didj::DidjSession::start(&dir, &self.save_dir(), cart, sink) {
            Ok(mut d) => {
                self.fit_window = FIT_TRIES;
                if let Some(g) = self.library.games.iter().find(|g| g.path == package) {
                    d.title.clone_from(&g.title);
                }
                self.didj = Some(d);
                self.paused = false;
                if let Some(a) = &mut self.audio {
                    a.clear();
                }
                self.update_title();
            }
            Err(e) => self.ui.problem(
                "Couldn't start the Didj",
                format!(
                    "Ribbit builds the Didj's memory from its packages (DIDJ-0x….lfp files: Bootstrap, Device Firmware, Brio and the Base_ ones), all in one folder: \"{}\". One may be missing or damaged.",
                    dir.display()
                ),
                format!("{e:#}"),
            ),
        }
    }

    /// Opens a RockIt Twist session (`package`: the package chosen in the
    /// library, if any): the console starts from its system packages when
    /// they are found, otherwise only its controls work.
    /// Plays the original LeapPad: its base ROM (`leappad_bios`) with a
    /// cartridge, or none (its built-in book).
    fn start_leappad(&mut self, cart: Option<PathBuf>) {
        let Some(bios) = self.settings.leappad_bios.clone() else {
            return self.ui.problem(
                "No LeapPad base ROM",
                "The LeapPad needs the system software from your own pad (a dump of its base ROM). The 2 MB \
                 \"Canada Full Base ROM\" plays every book; the 1 MB \"ILA2 Universal Base ROM\" lacks sounds some \
                 older books use. Open it with File > Open BIOS…, then the cartridge again.",
                "",
            );
        };
        self.end_session();
        let sink = self.audio.as_ref().map(|a| a.sink());
        match crate::leappad::LeapPadSession::start(&bios, cart.as_deref(), &self.save_dir(), sink, self.settings.leappad_classic) {
            Ok(mut lp) => {
                if let Some(c) = &cart {
                    self.settings.recent_carts.retain(|p| p != c);
                    self.settings.recent_carts.insert(0, c.clone());
                    self.settings.recent_carts.truncate(crate::settings::MAX_RECENT);
                }
                // The book opened with this cartridge last time, or one next to it.
                let key = lp.book_key();
                let near = cart.as_ref().map(|c| c.with_extension("toml")).filter(|p| p.is_file());
                if let Some(b) = self.settings.leappad_books.get(&key).cloned().filter(|p| p.is_file()).or(near) {
                    let problems = lp.open_book(ribbit_leappad::book::Book::load(&b));
                    if problems.iter().any(|p| p.error) {
                        self.ui.toast(format!("The book {} has errors", b.display()));
                    }
                }
                // Otherwise the book drawn from the cartridge's own layout.
                if lp.book.is_none() {
                    lp.open_cartridge_book();
                }
                lp.auto_go = self.settings.leappad_auto_go;
                if lp.auto_go {
                    lp.go_first_page();
                }
                // Its pages' words: kept from before, or made on this PC.
                lp.start_words(&self.opts.data_dir, self.settings.leappad_words);
                self.ui.toast(format!("LeapPad: {}", lp.title));
                if lp.repaired {
                    self.ui.toast("This cartridge file is damaged (its checksum is wrong), so a real LeapPad would refuse it. Ribbit plays it anyway; a few sounds may be wrong.");
                }
                self.leappad = Some(lp);
                self.update_title();
                self.save_settings();
            }
            Err(e) => self.ui.problem(
                "Couldn't start the LeapPad",
                "The base ROM or the cartridge could not be used.",
                format!("{e:#}"),
            ),
        }
    }

    /// Opens a virtual book on the running LeapPad (None: Ribbit's test book).
    fn open_leappad_book(&mut self, path: Option<PathBuf>) {
        let Some(lp) = &mut self.leappad else { return };
        let loaded = match &path {
            Some(p) => ribbit_leappad::book::Book::load(p),
            None => ribbit_leappad::book::Book::parse(crate::leappad::TEST_BOOK, Path::new(".")),
        };
        let problems = lp.open_book(loaded);
        let errors: Vec<String> = problems.iter().filter(|p| p.error).map(|p| p.message.clone()).collect();
        if !errors.is_empty() {
            return self.ui.problem("This book can't be opened", "The book file has mistakes:", errors.join("\n"));
        }
        if let Some(p) = path {
            self.settings.leappad_books.insert(lp.book_key(), p);
            self.save_settings();
        }
        let notes = problems.len();
        self.ui.toast(if notes > 0 { format!("Book opened ({notes} notes in the debug panel)") } else { "Book opened".to_string() });
    }

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

    /// Emulation › Leapster 2 SD card: changes the card of the running
    /// Leapster 2 (which then restarts, as if the card had been swapped with
    /// the power off), or else the card file of the Leapster 2 BIOS picked in
    /// the library, for its next start.
    fn sd_card(&mut self, job: crate::sdcard::Job) {
        match self.sd_card_job(&job) {
            Ok(done) => self.ui.toast_for(done, Duration::from_secs(6)),
            Err(e) => {
                let title = if matches!(job, crate::sdcard::Job::Export(_)) {
                    "The SD card's files weren't copied"
                } else {
                    "The SD card wasn't changed"
                };
                self.ui.problem(title, format!("{e:#}"), String::new());
            }
        }
    }

    fn sd_card_job(&mut self, job: &crate::sdcard::Job) -> Result<String> {
        use crate::sdcard::{self, Job};
        // The new contents (None: unchanged) and what to say.
        let change = |sectors: &mut sdcard::Sectors| -> Result<(Option<sdcard::Sectors>, String)> {
            Ok(match job {
                Job::LoadImage(path) => {
                    let (s, copied) = sdcard::load_image(path)?;
                    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let how = if copied { " (its files: the card was larger than 2 GB)" } else { "" };
                    (Some(s), format!("SD card loaded from {name}{how}"))
                }
                Job::Export(dir) => {
                    let n = sdcard::export(sectors, dir)?;
                    (None, format!("{n} files copied from the SD card to {}", dir.display()))
                }
                Job::Empty => (Some(sdcard::fresh()), "The SD card is empty (freshly formatted)".to_string()),
            })
        };
        // The running Leapster 2's card.
        if let Some(s) = &mut self.session
            && let Some(path) = s.saves.sd.clone()
        {
            let mut m = s.emu.lock();
            s.saves.flush(&mut m)?;
            let card = crate::saves::sd_card_mut(&mut m).context("this Leapster 2 has no SD card")?;
            let (new, mut done) = change(&mut card.sectors)?;
            let Some(new) = new else {
                return Ok(done);
            };
            backup_card(&path, job)?;
            card.sectors = new;
            card.changed.clear();
            card.synced = false;
            card.dirty = true;
            s.saves.flush(&mut m)?;
            // With an SD card folder the image's files went into it: the
            // card is the whole folder again.
            if let (Some(folder), Some(card)) = (s.saves.sd_folder.clone(), crate::saves::sd_card_mut(&mut m)) {
                folder.load(card)?;
            }
            m.reset();
            drop(m);
            s.emu.resync();
            done.push_str("; the Leapster 2 restarted");
            return Ok(done);
        }
        // Not running: the card file of the Leapster 2 BIOS picked in the library.
        let bios = self
            .settings
            .bios_picked
            .get(Hardware::Leapster2.name())
            .cloned()
            .or_else(|| self.library.bios_list(Hardware::Leapster2).into_iter().next().map(|b| b.path.clone()))
            .context("no Leapster 2 BIOS yet: add one in the library (Leapster 2 tab), then try again")?;
        let fw = ribbit_memory::Firmware::from_bytes(std::fs::read(&bios).with_context(|| format!("reading {}", bios.display()))?)
            .map_err(|e| anyhow::anyhow!("{}: {e}", bios.display()))?;
        if fw.leapster2_nand() {
            anyhow::bail!(
                "the Leapster 2 BIOS picked in the library ({}) is a CIP3 board's, which has no SD card slot. Pick 0.6 or Connected B5.",
                bios.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
            );
        }
        if let Some(dir) = &self.settings.leapster2_sd_folder {
            let Job::LoadImage(image) = job else {
                anyhow::bail!("the SD card is the folder {} now: change the files there", dir.display());
            };
            let n = sdcard::export(&mut sdcard::load_image(image)?.0, dir)?;
            return Ok(format!("{n} files from the image copied into the SD card folder"));
        }
        let path = self.save_dir().join(format!("sdcard-{:08x}.img", fw.id().crc32));
        let mut sectors = if path.exists() { sdcard::load_image(&path)?.0 } else { sdcard::fresh() };
        let (new, done) = change(&mut sectors)?;
        if let Some(new) = new {
            backup_card(&path, job)?;
            crate::saves::write_atomic(&path, &ribbit_core::io::sparse::encode(&new, ribbit_core::io::sd::SECTOR))?;
        }
        Ok(done)
    }

    /// Chooses (or stops using) the Leapster 2's SD card folder; a running
    /// Leapster 2 restarts with it. An empty folder first gets the card's
    /// files, so the profiles and games on the card carry on.
    fn set_sd_folder(&mut self, path: Option<PathBuf>) {
        let mut seeded = None;
        if let Some(dir) = &path
            && std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none())
            && (self.settings.leapster2_sd_folder.is_none() || self.session.as_ref().is_some_and(|s| s.saves.sd.is_some()))
        {
            match self.sd_card_job(&crate::sdcard::Job::Export(dir.clone())) {
                Ok(_) => seeded = Some(()),
                // No Leapster 2 BIOS yet: the folder starts empty.
                Err(e) => tracing::info!(target: "ribbit::saves", "SD card folder not filled from the card: {e:#}"),
            }
        }
        self.settings.leapster2_sd_folder = path.clone();
        self.save_settings();
        let mut restarted = false;
        if let Some(s) = &mut self.session
            && s.saves.sd.is_some()
        {
            let mut m = s.emu.lock();
            if let Err(e) = s.saves.flush(&mut m) {
                tracing::warn!(target: "ribbit::saves", "saving: {e:#}");
            }
            s.saves.sd_folder = path.clone().map(|d| Arc::new(crate::sdcard::Folder::new(d)));
            let loaded = match (&s.saves.sd_folder, crate::saves::sd_card_mut(&mut m)) {
                (Some(folder), Some(card)) => folder.load(card).map(|_| ()),
                // Back to the card file (or a new card).
                (None, Some(card)) => {
                    let file = s.saves.sd.clone().filter(|p| p.exists());
                    match file.map(|p| crate::sdcard::load_image(&p)).transpose() {
                        Ok(sectors) => {
                            card.sectors = sectors.map_or_else(crate::sdcard::fresh, |(s, _)| s);
                            card.changed.clear();
                            card.dirty = false;
                            card.synced = s.saves.sd.as_ref().is_some_and(|p| p.exists());
                            Ok(())
                        }
                        Err(e) => Err(e),
                    }
                }
                _ => Ok(()),
            };
            m.reset();
            drop(m);
            s.emu.resync();
            if let Err(e) = loaded {
                self.ui.problem("The SD card folder couldn't be used", format!("{e:#}"), String::new());
                return;
            }
            restarted = true;
        }
        let what = match (&path, seeded) {
            (Some(_), Some(())) => "SD card folder chosen: the card's files were copied into it",
            (Some(_), None) => "SD card folder chosen",
            (None, _) => "The SD card is the card file again",
        };
        self.ui.toast_for(if restarted { format!("{what}; the Leapster 2 restarted") } else { what.to_string() }, Duration::from_secs(6));
    }

    /// When to draw the main window next. Every frame while something
    /// runs or moves (a game, the LeapPad, a recording, the debugger's
    /// windows, a file dialog); else after input, when egui asks (an
    /// animation, a message), and now and then for work done in the
    /// background (the library scan, the update check). Ribbit used to draw
    /// every frame with nothing changing, keeping the GPU busy all the time,
    /// even in the background; a player's Mac froze now and then while
    /// Ribbit sat at the library (2026-10-26).
    fn next_redraw(&self, now: Instant) -> Instant {
        #[cfg(target_os = "macos")]
        let picking = self.sheet.is_some() || self.native_pick.is_some();
        #[cfg(not(target_os = "macos"))]
        let picking = self.native_pick.is_some();
        let busy = self.session.is_some()
            || self.zippity.is_some()
            || self.didj.is_some()
            || self.twist.is_some()
            || self.leappad.is_some()
            || self.recording.is_some()
            || self.fit_window > 0
            || self.closing_animation
            || self.popouts.any()
            || picking;
        if busy || self.woken {
            return now;
        }
        let focused = self.main_focused || self.popouts.focused;
        let every = if focused { IDLE_REDRAW } else { BACKGROUND_REDRAW };
        let due = self.drawn_at + every;
        self.repaint_at.map_or(due, |at| at.min(due))
    }

    /// Every 30 s, a line in the log (frames drawn, a game running, in the
    /// background), written to disk at once: the last lines before a
    /// system freeze then survive the forced restart.
    fn log_status(&mut self, now: Instant) {
        let span = now.duration_since(self.status.0);
        if span < STATUS_EVERY {
            return;
        }
        let running = self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some() || self.leappad.is_some();
        tracing::info!(target: "ribbit::perf",
            fps = (self.status.1 as f64 / span.as_secs_f64()).round(),
            running,
            background = !(self.main_focused || self.popouts.focused),
            "status (last 30 s)");
        self.status = (now, 0);
        if let Ok(f) = std::fs::OpenOptions::new().append(true).open(self.opts.data_dir.join("ribbit.log")) {
            let _ = f.sync_data();
        }
    }

    /// The console running (None: none).
    fn current_hardware(&self) -> Option<Hardware> {
        if self.twist.is_some() {
            Some(Hardware::RockItTwist)
        } else if self.zippity.is_some() {
            Some(Hardware::Zippity)
        } else if self.didj.is_some() {
            Some(Hardware::Didj)
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
        if let Some(d) = &self.didj {
            d.set_button(b, state);
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
            if let Some(d) = &self.didj {
                d.set_button(b, false);
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
            Pick::RecordFolder => self.settings.record_folder.clone(),
            Pick::LeapPadBook => self.leappad.as_ref().and_then(|l| l.cart.clone()),
            Pick::SdFolder => self.settings.leapster2_sd_folder.clone(),
            Pick::SdImage | Pick::SdExport => None,
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
            Pick::RecordFolder => {
                self.settings.record_folder = Some(path);
                self.save_settings();
            }
            Pick::LeapPadBook => self.open_leappad_book(Some(path)),
            Pick::SdFolder => self.set_sd_folder(Some(path)),
            Pick::SdImage => self.sd_card(crate::sdcard::Job::LoadImage(path)),
            Pick::SdExport => self.sd_card(crate::sdcard::Job::Export(path)),
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

    /// The debugger (debugger.rs), each frame: a breakpoint reached pauses
    /// the game and shows the code; the open windows get what they show.
    fn poll_debugger(&mut self) {
        let hit = self
            .session
            .as_ref()
            .and_then(|s| s.emu.take_breakpoint())
            .or_else(|| self.leappad.as_ref().and_then(|l| l.take_breakpoint().map(u32::from)));
        if let Some(pc) = hit {
            self.paused = true;
            self.ui.debug.code_at = None;
            self.ui.debug.show_cpu = true;
            self.ui.toast(format!("Breakpoint at {pc:08X}"));
        }
        if !self.ui.debug.open() {
            return;
        }
        self.sync_breakpoints(false);
        let (paused, bps) = (self.paused, self.debug_breakpoints.clone());
        self.ui.debug_snapshot = if let Some(s) = &self.session {
            let m = s.emu.lock();
            crate::debugger::leapster_snapshot(&m, &mut self.ui.debug, &bps, paused)
        } else if let Some(l) = &self.leappad {
            l.debug_snapshot(&mut self.ui.debug, &bps, paused)
        } else {
            let system = if self.zippity.is_some() {
                "Zippity"
            } else if self.didj.is_some() {
                "Didj"
            } else if self.twist.is_some() { "RockIt Twist" } else { "no game" };
            crate::debugger::Snapshot { system: system.into(), ..Default::default() }
        };
    }

    /// Gives the running console the debugger's breakpoints (`resume`: run
    /// past the one it stopped at).
    fn sync_breakpoints(&self, resume: bool) {
        if let Some(s) = &self.session {
            let mut b = ribbit_debug::Breakpoints::default();
            self.debug_breakpoints.iter().for_each(|&a| b.add(a));
            s.emu.set_breakpoints(b);
        } else if let Some(l) = &self.leappad {
            l.set_breakpoints(&self.debug_breakpoints, resume);
        }
    }

    /// An event for one of the debugger's own windows: drawn from the
    /// snapshot the main window's frame took (`poll_debugger`).
    fn popout_event(&mut self, w: crate::debugger::Which, event: WindowEvent) {
        let Some(gpu) = &self.gpu else { return };
        let mut actions = Vec::new();
        if matches!(event, WindowEvent::RedrawRequested) {
            let snap = std::mem::take(&mut self.ui.debug_snapshot);
            if let Err(e) = self.popouts.draw(w, gpu, &mut self.ui.debug, &snap, &mut actions) {
                tracing::warn!(target: "ribbit", "drawing the debugger: {e:#}");
            }
            self.ui.debug_snapshot = snap;
        } else {
            self.popouts.event(w, &event, gpu, &mut self.ui.debug);
        }
        for a in actions {
            self.apply_debug(a);
        }
    }

    fn apply_debug(&mut self, a: crate::debugger::Action) {
        use crate::debugger::Action as D;
        match a {
            D::Poke(addr, v) => {
                if let Some(s) = &self.session {
                    s.emu.lock().bus.poke8(addr, v);
                } else if let Some(l) = &self.leappad {
                    l.debug_poke(addr, v);
                }
            }
            D::Pause => self.paused = true,
            D::Continue => {
                self.sync_breakpoints(true);
                self.paused = false;
            }
            D::Step => {
                if let Some(s) = &self.session {
                    s.emu.lock().run(1, &ribbit_debug::Breakpoints::default());
                } else if let Some(l) = &self.leappad {
                    l.debug_step();
                }
                self.ui.debug.code_at = None;
            }
            D::AddBreakpoint(addr) => {
                if !self.debug_breakpoints.contains(&addr) {
                    self.debug_breakpoints.push(addr);
                    self.debug_breakpoints.sort_unstable();
                }
                self.sync_breakpoints(false);
            }
            D::RemoveBreakpoint(addr) => {
                self.debug_breakpoints.retain(|&b| b != addr);
                self.sync_breakpoints(false);
            }
        }
        self.update_title();
    }

    fn update_title(&self) {
        if let Some(g) = &self.gpu {
            let title = self
                .session
                .as_ref()
                .map(|s| &s.title)
                .or(self.zippity.as_ref().map(|z| &z.title))
                .or(self.didj.as_ref().map(|d| &d.title))
                .or(self.leappad.as_ref().map(|l| &l.title))
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
                // Not while paused: the animation can't play, and closing
                // waited out its full 3 s for nothing.
                let can_animate = self.settings.close_with_shutdown
                    && !self.closing_animation
                    && !self.halted()
                    && self.session.as_ref().is_some_and(|s| !s.loaded_save_state);
                if can_animate {
                    self.start_closing_animation();
                } else if !self.closing_animation {
                    self.end_session();
                    self.update_title();
                    // Recently played games join the library.
                    self.rescan_library();
                }
            }
            Action::AddRomFolder(p) => self.add_rom_folder(p),
            Action::RemoveRomFolder(p) => {
                self.settings.rom_folders.retain(|f| *f != p);
                self.save_settings();
                self.rescan_library();
            }
            Action::RescanLibrary => self.rescan_library(),
            Action::OpenFolder(path) => open_folder(&path),
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
            Action::StartLeapPad => self.start_leappad(None),
            Action::OpenLeapPadBook(path) => self.open_leappad_book(Some(path)),
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
            Action::SetLeapster2Usb(on) => {
                self.settings.leapster2_usb = on;
                self.save_settings();
                self.apply_leapster2_usb();
                if on {
                    self.ui.toast(
                        "Leapster 2 plugged in. Wait about 7 seconds, then run: usbip attach -r 127.0.0.1 -b 1-1",
                    );
                } else {
                    self.ui.toast("Leapster 2 unplugged from USB");
                }
            }
            Action::ImportTwistZip(path) => self.import_twist_zip(path),
            Action::SdCard(job) => self.sd_card(job),
            Action::SetSdFolder(path) => self.set_sd_folder(path),
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
            Action::GameSettings(on) => self.set_game_settings(on),
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
            Action::ToggleDrawer => self.ui.drawer = !self.ui.drawer,
            Action::ToggleRecord => self.toggle_record(),
            Action::PickRecordFolder => self.pick(Pick::RecordFolder),
            Action::SetRecordFolder(path) => {
                self.settings.record_folder = Some(path);
                self.save_settings();
            }
            Action::ResetRecordFolder => {
                self.settings.record_folder = None;
                self.save_settings();
            }
            Action::OpenRecordFolder => {
                // Made now if nothing was recorded yet (the file manager
                // would open somewhere else).
                let dir = self.record_dir();
                let _ = std::fs::create_dir_all(&dir);
                open_folder(&dir);
            }
            Action::CheatFirstScan => {
                self.sync_cheat_from_ui();
                let Some(s) = &self.session else { return };
                let m = s.emu.lock();
                self.cheat.first_scan(&m);
            }
            Action::CheatNextScan => {
                self.sync_cheat_from_ui();
                let Some(s) = &self.session else { return };
                let m = s.emu.lock();
                self.cheat.next_scan(&m);
            }
            Action::CheatReset => self.cheat.reset(),
            Action::Debug(a) => self.apply_debug(a),
            Action::CheatFreeze(addr, val) => {
                self.sync_cheat_from_ui();
                let label = format!("0x{addr:08x}");
                self.cheat.add_freeze(addr, val, label);
            }
            Action::CheatUnfreeze(addr) => self.cheat.remove_freeze(addr),
            Action::CheatSet(addr, val) => {
                self.sync_cheat_from_ui();
                if let Some(s) = &self.session {
                    self.cheat.set_once(&mut s.emu.lock(), addr, val);
                }
            }
            Action::ToggleFavorite(path) => {
                if let Some(i) = self.settings.favorites.iter().position(|p| p == &path) {
                    self.settings.favorites.remove(i);
                } else {
                    self.settings.favorites.push(path);
                }
                self.save_settings();
            }
            Action::ToggleBuiltinGames(tab) => {
                let name = tab.map_or_else(|| "all".to_string(), |f| f.name().to_string());
                if !self.settings.hide_builtin_tabs.remove(&name) {
                    self.settings.hide_builtin_tabs.insert(name);
                }
                self.save_settings();
            }
            Action::CreateFolder(games) => {
                self.settings.folders.push(crate::settings::LibraryFolder {
                    name: "New Folder".to_string(),
                    games,
                });
                self.save_settings();
                let idx = self.settings.folders.len() - 1;
                self.ui.renaming_folder = Some(idx);
                self.ui.rename_text = "New Folder".to_string();
            }
            Action::OpenLibraryFolder(i) => {
                self.ui.library.open_folder = Some(i);
            }
            Action::CloseLibraryFolder => {
                self.ui.library.open_folder = None;
            }
            Action::RenameFolder { idx, name } => {
                if name.is_empty() {
                    // Empty name = open the rename dialog for this folder.
                    let current = self.settings.folders.get(idx).map_or("", |f| f.name.as_str()).to_string();
                    self.ui.renaming_folder = Some(idx);
                    self.ui.rename_text = current;
                } else {
                    if let Some(f) = self.settings.folders.get_mut(idx) {
                        f.name = name;
                    }
                    self.save_settings();
                    self.ui.renaming_folder = None;
                }
            }
            Action::DeleteFolder(idx) => {
                if idx < self.settings.folders.len() {
                    self.settings.folders.remove(idx);
                }
                self.save_settings();
                if self.ui.library.open_folder == Some(idx) {
                    self.ui.library.open_folder = None;
                }
            }
            Action::AddToFolder { folder, game } => {
                if let Some(f) = self.settings.folders.get_mut(folder) {
                    if !f.games.contains(&game) {
                        f.games.push(game);
                    }
                }
                self.save_settings();
            }
            Action::RemoveFromFolder { folder, game } => {
                if folder == usize::MAX {
                    // Sentinel: remove from whichever folder contains the game.
                    for f in &mut self.settings.folders {
                        f.games.retain(|g| *g != game);
                    }
                } else if let Some(f) = self.settings.folders.get_mut(folder) {
                    f.games.retain(|g| *g != game);
                }
                self.save_settings();
            }
        }
    }

    /// A game's first start: says it may take a while once the game starts
    /// writing its save (Pet Pals then shows a black screen for about 20 s),
    /// not at power-on over the BIOS's own screen. The first write comes
    /// after the LeapFrog logo (about 8 s in for Pet Pals, Cars and Sonic X).
    fn first_start_notice(&mut self) {
        let Some(s) = &mut self.session else {
            return;
        };
        let Some(since) = s.first_start else {
            return;
        };
        // A write long after start-up is the player saving, not set-up.
        if since.elapsed() > std::time::Duration::from_secs(30) {
            s.first_start = None;
            return;
        }
        if s.emu.lock().io.eeprom.cart_dirty {
            s.first_start = None;
            self.ui.toast_for("First start of this game: it may take a while to set up its save", std::time::Duration::from_secs(25));
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

    /// Begin the close-with-animation path: press the virtual power button so
    /// the BIOS plays its per-game shutdown animation, then wait for the screen
    /// to go black before ending the session.
    fn start_closing_animation(&mut self) {
        let Some(s) = &mut self.session else { return; };
        s.closing = None;
        {
            let mut m = s.emu.lock();
            // Clear the NO_POWER_OFF_REQUEST GPIO line (bit 11 low = power
            // button held). The BIOS polls this and plays the game's own
            // shutdown animation, which is different from (and more elaborate
            // than) the idle-timeout fade.
            m.io.gpio.board_lines &= !ribbit_core::io::gpio::NO_POWER_OFF_REQUEST;
        }
        self.closing_animation = true;
    }

    /// Called each frame while `closing_animation` is true: waits for the
    /// power-button animation to finish (screen goes black), then closes.
    fn check_closing_animation(&mut self) {
        if !self.closing_animation {
            return;
        }
        let Some(s) = &mut self.session else {
            self.closing_animation = false;
            return;
        };
        let since = *s.closing.get_or_insert_with(Instant::now);
        // Wait for the screen to go black. The per-game animation takes up to
        // ~2 s; fall back to a direct close after 3 s if it never goes black.
        let light = self.frame.iter().map(|p| ((p >> 16) & 0xFF).max((p >> 8) & 0xFF).max(p & 0xFF) as u64).sum::<u64>();
        let black = light < 4 * self.frame.len() as u64;
        if !black && since.elapsed().as_secs_f32() < 3.0 {
            return;
        }
        if !black {
            tracing::info!(target: "ribbit::boot", "shutdown animation did not complete; closing directly");
        } else {
            tracing::info!(target: "ribbit::boot", "shutdown animation complete; closing game");
        }
        self.closing_animation = false;
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
                if let Some(s) = &mut self.session {
                    s.emu.resync();
                    s.loaded_save_state = true;
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
        if let Some(d) = &self.didj {
            d.reset();
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
        if self.zippity.is_some() || self.didj.is_some() || self.twist.is_some() {
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

    /// Paused by the user, or by being in the background. A RockIt Twist
    /// plugged into the PC keeps running in the background: the program
    /// talking to it over USB (LeapFrog Connect) is the active window then.
    /// Offers the running Leapster 2 on USB if so set (starting the USB/IP
    /// server the first time), else unplugs it.
    fn apply_leapster2_usb(&mut self) {
        let Some(s) = &self.session else { return };
        if s.hardware != Hardware::Leapster2 {
            return;
        }
        if !self.settings.leapster2_usb {
            s.emu.set_usb(None);
            return;
        }
        if self.leapster2_usb.is_none() {
            match ribbit_twist::usbip::Server::start(leapster2_usb_device(), ribbit_twist::usbip::PORT) {
                Ok(srv) => self.leapster2_usb = Some(std::sync::Arc::new(std::sync::Mutex::new(srv))),
                Err(e) => {
                    self.ui.toast(format!("Leapster 2 USB: can't listen on port 3240 ({e})"));
                    return;
                }
            }
        }
        s.emu.set_usb(self.leapster2_usb.clone());
    }

    fn halted(&self) -> bool {
        // A console plugged into the PC keeps running in the background.
        let on_usb = (self.twist.is_some() && self.settings.twist_usb)
            || (self.settings.leapster2_usb && self.session.as_ref().is_some_and(|s| s.hardware == Hardware::Leapster2));
        self.paused || (self.background && self.settings.pause_in_background && !on_usb)
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

    /// Where recordings go: the chosen folder, else the player's Videos folder.
    fn record_dir(&self) -> PathBuf {
        self.settings.record_folder.clone().unwrap_or_else(|| self.record_default.clone())
    }

    /// Says where a finished recording went (or why it didn't).
    fn poll_encoding(&mut self) {
        if !self.encoding.as_ref().is_some_and(|t| t.is_finished()) {
            return;
        }
        let Some(t) = self.encoding.take() else { return };
        match t.join().unwrap_or_else(|_| Err(anyhow::anyhow!("encoding thread panicked"))) {
            Ok(saved) => {
                let name = saved.path.file_name().map_or_else(|| saved.path.display().to_string(), |n| n.to_string_lossy().into_owned());
                let folder = crate::record::folder_of(&saved.path).display();
                let parts = if saved.parts > 1 { format!(" (in {} parts)", saved.parts) } else { String::new() };
                let note = saved.note.map(|n| format!("
{n}")).unwrap_or_default();
                self.ui.toast_for(format!("Recording saved: {name}{parts} in {folder}{note}"), std::time::Duration::from_secs(10));
            }
            Err(e) => self.ui.problem("The recording couldn't be saved", format!("{e:#}"), ""),
        }
    }

    fn toggle_record(&mut self) {
        self.poll_encoding();
        if let Some(mut rec) = self.recording.take() {
            // Stop: flush remaining buffered audio and start encoding.
            if let Some(s) = &self.session {
                let mut remaining = Vec::new();
                s.emu.stop_record_audio(&mut remaining);
                let _ = rec.add_audio(&remaining);
                for _ in 0..rec.frames_due() {
                    let _ = rec.add_frame(&self.frame[..]);
                }
            }
            let encoder = self.settings.record_encoder;
            match rec.finish(encoder) {
                Ok(task) => {
                    let folder = crate::record::folder_of(&task.output_path).display();
                    self.ui.toast(format!("Recording stopped: saving it in {folder}…"));
                    self.encoding = Some(std::thread::spawn(move || task.run()));
                }
                Err(e) => self.ui.problem("Recording failed", format!("{e:#}"), ""),
            }
        } else {
            // Start.
            let Some(s) = &self.session else { return };
            let dir = self.record_dir();
            // The raw frames wait in the data folder, not the Videos folder.
            let temp = self.opts.data_dir.join("recording-temp");
            match crate::record::Recorder::start(&dir, &temp, self.settings.record_encoder) {
                Ok(rec) => {
                    self.ui.toast(format!("Recording… (F8 to stop). It will be saved in {}", dir.display()));
                    s.emu.start_record_audio();
                    self.recording = Some(rec);
                }
                Err(e) => self.ui.problem("Could not start recording", format!("{e:#}"), ""),
            }
        }
    }

    fn sync_cheat_from_ui(&mut self) {
        self.cheat.op = self.ui.cheat_op;
        self.cheat.ty = self.ui.cheat_ty;
        self.cheat.value_text = self.ui.cheat_value_text.clone();
    }

    /// Logs (every 5 s) how often the sound ran out.
    fn log_underruns(audio: Option<&AudioOut>, log: &mut (Instant, (u64, u64)), speed_percent: f64) {
        let now = Instant::now();
        let Some(a) = audio else {
            return;
        };
        if now.duration_since(log.0).as_secs() >= 5 {
            let n = a.glitches();
            // The longest wait between the device's callbacks.
            let callback_ms = a.take_longest_gap().as_millis() as u64;
            if n.0 > log.1.0 {
                tracing::info!(target: "ribbit::audio", gaps = n.0 - log.1.0, speed = speed_percent, callback_ms, "audio ran dry (last 5 s)");
            }
            if n.1 > log.1.1 {
                tracing::info!(target: "ribbit::audio", skips = n.1 - log.1.1, speed = speed_percent, callback_ms, "audio queue overflowed, sound skipped (last 5 s)");
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
        if let Some(lp) = &self.leappad {
            lp.set_gain(if self.settings.muted { 0.0 } else { self.settings.volume });
            self.speed_percent = lp.speed() as f64;
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
        if let Some(d) = &self.didj {
            d.set_running(!halted);
            d.set_gain(if self.settings.muted { 0.0 } else { self.settings.volume });
            self.speed_percent = d.speed_percent();
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
                let _ = m.load_rewind_state(&state);
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
                let _ = s.compress.todo.send(m.rewind_snapshot());
                s.last_snapshot = m.cpu.cycles;
            }
            if let Err(e) = s.autosave.tick(&s.saves, &mut m, now) {
                tracing::warn!(target: "ribbit::saves", "autosave failed: {e:#}");
            }
            // Apply cheat freezes: write frozen values to memory each frame.
            self.cheat.apply_freezes(&mut m);
        }
        // Sound goes from the emulation thread straight to the output (real-
        // time speed only); at other speeds drop what is queued.
        if speed != 1.0
            && let Some(a) = &mut self.audio
        {
            a.clear();
        }
        match s.emu.take_usb_news() {
            Some(crate::twist::UsbNews::Attached) => self.ui.toast("Leapster 2: the PC (USB/IP) connected"),
            Some(crate::twist::UsbNews::Detached) => self.ui.toast("Leapster 2: the PC (USB/IP) disconnected"),
            Some(crate::twist::UsbNews::Failed(e)) => self.ui.toast(format!("Leapster 2 USB: {e}")),
            None => {}
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
                KeyCode::F8 => return self.toggle_record(),
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
            let running = self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some();
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

    /// Android: the screen stays on while a game plays (as on the console,
    /// it may be played with a controller, or watched); paused, or in the
    /// library, the phone sleeps as usual.
    fn keep_screen_on(&mut self) {
        let Some(set) = self.opts.keep_screen_on else {
            return;
        };
        let playing = (self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some()) && !self.paused;
        if playing != self.screen_on {
            set(playing);
            self.screen_on = playing;
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
                    if (self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some()) && !self.paused {
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
        (self.drawn_at, self.woken) = (frame_start, false);
        self.status.1 += 1;
        #[cfg(not(target_os = "android"))]
        self.poll_pads();
        self.emulate();
        if self.closing_animation {
            self.check_closing_animation();
        } else {
            self.check_idle_power_off();
            self.first_start_notice();
        }
        self.poll_encoding();
        let (mut hd_frame, mut hd_dirty) = (false, false);
        if let Some(s) = &mut self.session {
            let passes = {
                let mut m = s.emu.lock();
                m.render_frame(&mut self.frame);
                m.hd.as_mut().map(|h| h.take()).unwrap_or_default()
            };
            // Screen recording: capture this frame and any buffered audio.
            if let Some(rec) = &mut self.recording {
                s.emu.drain_record_audio(&mut self.rec_audio);
                let _ = rec.add_audio(&self.rec_audio);
                self.rec_audio.clear();
                for _ in 0..rec.frames_due() {
                    let _ = rec.add_frame(&self.frame[..]);
                }
                // Apply cheat freezes while recording so they show in the video.
            }
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
        if let Some(d) = &self.didj {
            d.frame(&mut self.tv_frame);
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
        } else if self.didj.is_some() {
            Some((crate::didj::SCREEN_W, crate::didj::SCREEN_H))
        } else {
            None
        };
        let touch_pads = self.touch_pads();
        self.poll_debugger();
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
                running: self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some() || self.leappad.is_some(),
                game_settings: self.session.as_ref().and_then(|s| s.cart_path.as_ref()).map(|_| self.game_own.is_some()),
                leappad: self.leappad.is_some(),
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
                    .or(self.didj.as_ref().map(|d| d.title.clone()))
                    .or(self.twist.as_ref().map(|t| t.title.clone()))
                    .or(self.leappad.as_ref().map(|l| l.title.clone()))
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
                recording: self.recording.as_ref().map(|r| r.started.elapsed().as_secs()),
                encoding: self.encoding.as_ref().is_some_and(|t| !t.is_finished()),
                record_dir: self.settings.record_folder.clone().unwrap_or_else(|| self.record_default.clone()),
                cheat_candidates: {
                    // The values now (while the window is open), so a Set,
                    // or the game changing one, shows at once; the scan's
                    // values otherwise.
                    let all = &self.cheat.candidates;
                    let m = self.ui.show_cheat.then(|| self.session.as_ref().map(|s| s.emu.lock())).flatten();
                    all.iter()
                        .take(200)
                        .map(|c| (c.addr, m.as_ref().and_then(|m| self.cheat.current(m, c.addr)).unwrap_or(c.prev)))
                        .collect()
                },
                cheat_has_scan: self.cheat.has_scan,
                cheat_freezes: self.cheat.freezes.iter()
                    .map(|f| (f.addr, f.value, f.label.clone()))
                    .collect(),
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
                    // ("Books": all books.)
                    self.ui.library.books = last.map_or(self.settings.library_tab == "Books", |f| f.is_book());
                }
            }
        } else {
            let open = match self.ui.library.tab {
                Some(f) => f.name(),
                None if self.ui.library.books => "Books",
                None => "",
            }
            .to_string();
            if open != self.settings.library_tab {
                self.settings.library_tab = open;
                if let Err(e) = Self::usual_of(&mut self.game_own, &mut self.settings).save(&self.settings_path) {
                    tracing::warn!(target: "ribbit::settings", "saving settings: {e:#}");
                }
            }
        }
        self.discord.set(match &self.session {
            _ if !self.settings.discord_presence => crate::discord::Presence::Off,
            Some(s) => crate::discord::Presence::playing(&s.title, s.system, s.started),
            None => match (&self.zippity, &self.twist, &self.leappad) {
                (Some(z), _, _) => crate::discord::Presence::playing(&z.title, "Zippity", z.started),
                (None, Some(t), _) => crate::discord::Presence::playing(&t.title, "RockIt Twist", t.started),
                (None, None, Some(l)) => crate::discord::Presence::playing(&l.title, "LeapPad", l.started).with_series(l.series.as_deref()),
                (None, None, None) => match &self.didj {
                    Some(d) => crate::discord::Presence::playing(&d.title, "Didj", d.started),
                    None => crate::discord::Presence::Browsing,
                },
            },
        });
        let (ui_state, settings, library) = (&mut self.ui, &mut self.settings, &mut self.library);
        let touch_pad = &self.touch_pad;
        let (view, held, twist) = (&mut self.console_view, &self.held, self.twist.as_ref());
        let leappad = &mut self.leappad;
        let mut leappad_events = Vec::new();
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
            if let Some(lp) = leappad.as_mut() {
                lp.set_paused(info.paused);
                lp.auto_go = settings.leappad_auto_go;
                leappad_events = crate::leappad::show(ui, area, lp, info.paused, &mut settings.leappad_book_only, &mut settings.leappad_debug);
            }
        });
        self.touch_area = area;
        // Duration::MAX (nothing to animate) overflows: no time asked.
        self.repaint_at = out.viewport_output.get(&egui::ViewportId::ROOT).and_then(|v| frame_start.checked_add(v.repaint_delay));
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
        let lcd = (self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some())
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
        for e in leappad_events {
            match e {
                crate::leappad::Event::PickBook => self.pick(Pick::LeapPadBook),
                crate::leappad::Event::TestBook => self.open_leappad_book(None),
                crate::leappad::Event::CartridgeBook => {
                    if let Some(lp) = &mut self.leappad {
                        if lp.open_cartridge_book() {
                            lp.start_words(&self.opts.data_dir, self.settings.leappad_words);
                            self.settings.leappad_books.remove(&lp.book_key());
                            self.save_settings();
                            self.ui.toast("Book drawn from the cartridge's layout");
                        } else {
                            let note = lp.layout_note.clone().unwrap_or_default();
                            self.ui.toast(format!("Couldn't draw this cartridge's book: {note}"));
                        }
                    }
                }
                crate::leappad::Event::CloseBook => {
                    if let Some(lp) = &mut self.leappad {
                        lp.close_book();
                        self.settings.leappad_books.remove(&lp.book_key());
                        self.save_settings();
                    }
                }
            }
        }
        for a in actions {
            self.act(a, event_loop);
        }
        self.pause_for_side_menu();
        self.keep_screen_on();
        let took = frame_start.elapsed();
        if took.as_millis() >= 12 {
            tracing::debug!(target: "ribbit::frame", ms = took.as_secs_f64() * 1e3, "slow frame (excluding vsync wait)");
        }
        let running = self.session.is_some() || self.zippity.is_some() || self.didj.is_some() || self.twist.is_some();
        self.frame_diag.frame(frame_start, took, running && !self.halted());
    }
}

/// With nothing running or moving, the window is drawn at least this
/// often: in front, and in the background (`next_redraw`).
const IDLE_REDRAW: Duration = Duration::from_millis(250);
const BACKGROUND_REDRAW: Duration = Duration::from_secs(1);
/// How often `log_status` writes its line.
const STATUS_EVERY: Duration = Duration::from_secs(30);

/// The window's frame rate, its longest gap between frames and its own
/// work per frame, logged every 10 s while a game runs (for reports of
/// slowness: is the window, or the emulation, falling behind?).
#[derive(Default)]
struct FrameDiag {
    since: Option<Instant>,
    last: Option<Instant>,
    frames: u32,
    longest_gap: Duration,
    work: Duration,
    longest_work: Duration,
}

impl FrameDiag {
    fn frame(&mut self, start: Instant, work: Duration, running: bool) {
        if !running {
            *self = FrameDiag::default();
            return;
        }
        let since = *self.since.get_or_insert(start);
        if let Some(last) = self.last {
            self.longest_gap = self.longest_gap.max(start - last);
        }
        self.last = Some(start);
        self.frames += 1;
        self.work += work;
        self.longest_work = self.longest_work.max(work);
        let span = start - since;
        if span >= Duration::from_secs(10) {
            let ms = |d: Duration| (d.as_secs_f64() * 1e4).round() / 10.0;
            tracing::info!(target: "ribbit::perf",
                fps = (self.frames as f64 / span.as_secs_f64()).round(),
                longest_gap_ms = ms(self.longest_gap),
                work_ms = ms(self.work / self.frames.max(1)),
                longest_work_ms = ms(self.longest_work),
                "window (last 10 s)");
            *self = FrameDiag { since: Some(start), last: Some(start), ..Default::default() };
        }
    }
}

/// Whether `version` ("1.4.4"; empty: none recorded) is older than `than`.
fn older_than(version: &str, than: (u32, u32, u32)) -> bool {
    let mut n = version.split('.').map(|p| p.trim().parse::<u32>().unwrap_or(0));
    let v = (n.next().unwrap_or(0), n.next().unwrap_or(0), n.next().unwrap_or(0));
    !version.trim().is_empty() && v < than
}

/// An egui context with Ribbit's fonts and look (the main window's, and
/// the debugger's own windows').
pub(crate) fn egui_context() -> egui::Context {
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    // Buttons keep their size when pointed at or pressed (egui grows them a
    // pixel, so their text jumped).
    ctx.all_styles_mut(|s| {
        let w = &mut s.visuals.widgets;
        for state in [&mut w.hovered, &mut w.active, &mut w.open] {
            state.expansion = 0.0;
        }
    });
    ctx
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
        match event_loop.create_window(attrs).context("creating window").and_then(|w| Gpu::new(Arc::new(w), Some(&self.opts.data_dir.join(GPU_GUARD)))) {
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

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if let Some(w) = self.popouts.which(id) {
            self.popout_event(w, event);
            return;
        }
        // (`ui_at` decides whether a press belongs to the UI or the stylus.)
        let consumed = match (&mut self.egui_state, &self.gpu) {
            (Some(st), Some(g)) => st.on_window_event(&g.window, &event).consumed,
            _ => false,
        };
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.woken = true;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(size) => {
                if let Some(g) = &mut self.gpu {
                    g.resize(size);
                }
            }
            WindowEvent::ModifiersChanged(m) => self.modifiers = m.state(),
            // Settled in `about_to_wait`: focus moving to the debugger's
            // window is not going to the background.
            WindowEvent::Focused(f) => self.main_focused = f,
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
        // No BIOS chosen yet but one in the games folders (a player who only
        // added a folder): it is the BIOS, and the scan runs again to make
        // the title screens with it.
        if self.settings.bios.is_none()
            && !self.library.scanning
            && let Some(bios) = self.library.bios_for(ribbit_memory::known::Hardware::Leapster).map(Path::to_path_buf)
        {
            tracing::info!(target: "ribbit::library", bios = %bios.display(), "using the BIOS found in the games folders");
            self.settings.bios = Some(bios);
            self.save_settings();
            self.rescan_library();
        }
        let focused = self.main_focused || self.popouts.focused;
        if focused == self.background {
            self.focus_changed(focused);
        }
        let now = Instant::now();
        let next = self.next_redraw(now);
        if let Some(g) = &self.gpu {
            if next <= now {
                g.window.request_redraw();
            }
            self.popouts.sync(event_loop, g, &mut self.ui.debug, &self.ui.debug_snapshot.system);
            self.popouts.request_redraws();
        }
        event_loop.set_control_flow(if next <= now {
            winit::event_loop::ControlFlow::Wait
        } else {
            winit::event_loop::ControlFlow::WaitUntil(next)
        });
        self.log_status(now);
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

/// Before the SD card is replaced (an image, an empty card): its old file is
/// kept beside it as `.img.bak`.
fn backup_card(path: &Path, job: &crate::sdcard::Job) -> Result<()> {
    if matches!(job, crate::sdcard::Job::LoadImage(_) | crate::sdcard::Job::Empty) && path.exists() {
        std::fs::copy(path, path.with_extension("img.bak")).with_context(|| format!("backing up {}", path.display()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn versions_compare() {
        assert!(super::older_than("1.4.4", (1, 5, 1)));
        assert!(super::older_than("1.5.0", (1, 5, 1)));
        assert!(!super::older_than("1.5.1", (1, 5, 1)));
        assert!(!super::older_than("1.10.0", (1, 5, 1)));
        assert!(!super::older_than("", (1, 5, 1)));
    }

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
const TWIST_IMPORT_FAILED: &str = "Nothing changed on the emulated RockIt Twist. The zip should hold RockIt Twist package folders, each with its meta.inf, as they are installed on the console.";

const CLEAN_FAILED: &str = "The players are still there. Nothing was lost: the dump is either unchanged or \
                            kept beside it as a .with-saves.bak file.";

/// A path's file name, for messages.
/// The Leapster 2 as USB/IP lists it before a client imports it (BIOS 0.6's
/// device descriptor, VERIFIED with examples/l2usb.rs: vendor 0x0F63,
/// LeapFrog; product 0x1100; USB 1.1, full speed; one mass-storage
/// interface, SCSI, Bulk-Only).
fn leapster2_usb_device() -> ribbit_twist::usbip::DeviceInfo {
    ribbit_twist::usbip::DeviceInfo {
        busid: "1-1".into(),
        vendor: 0x0F63,
        product: 0x1100,
        bcd_device: 0x0001,
        class: (0, 0, 0),
        configuration: 1,
        num_configurations: 1,
        interfaces: vec![(8, 6, 0x50)],
        speed: 2,
    }
}

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
        LoadError::BiosIo { path, .. } => (
            "Couldn't read the BIOS".to_string(),
            format!(
                "The BIOS this console uses, \"{}\", couldn't be opened. It may have been moved, renamed or deleted. Choose it again with File → Open BIOS…, or pick another console.",
                path.display()
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

/// Shows a folder in the system's file manager.
fn open_folder(path: &Path) {
    #[cfg(windows)]
    let program = "explorer";
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(any(windows, target_os = "macos")))]
    let program = "xdg-open";
    if let Err(e) = std::process::Command::new(program).arg(path).spawn() {
        tracing::warn!(target: "ribbit::ui", %e, path = %path.display(), "could not open the folder");
    }
}
