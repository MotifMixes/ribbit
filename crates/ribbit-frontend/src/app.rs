//! The Ribbit application: one window, the emulated Leapster, egui menus.

use crate::audio_out::AudioOut;
use crate::emu::{Control, Emu};
use crate::files::Pick;
use crate::gpu::Gpu;
use crate::library::{ArtSource, Library};
use crate::saves::{AutoSave, SavePaths};
use crate::settings::Settings;
use crate::ui::{self, Action, UiState};
use anyhow::{Context, Result};
use ribbit_core::Machine;
use ribbit_video::{HEIGHT, WIDTH};
use std::path::PathBuf;
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
}

struct Session {
    /// EXPERIMENTAL HD picture, when HD vectors are on and this BIOS supports it.
    hd: Option<ribbit_core::hd::HdCanvas>,
    /// The machine, running on its own thread.
    emu: Emu,
    experimental_touch: bool,
    saves: SavePaths,
    autosave: AutoSave,
    title: String,
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
    gpu: Option<Gpu>,
    egui_ctx: egui::Context,
    egui_state: Option<egui_winit::State>,
    ui: UiState,
    frame: Box<[u32; WIDTH * HEIGHT]>,
    paused: bool,
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
    lcd_px: (f32, f32, f32, f32),
    library: Library,
    discord: crate::discord::Discord,
    /// Frames left to size the window around the screen (0 = done).
    fit_window: u8,
    fatal: Option<anyhow::Error>,
    quit: bool,
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
        if let Some(f) = &opts.firmware {
            settings.bios = Some(f.clone());
        }
        let mut app = Self {
            opts,
            settings,
            settings_path,
            session: None,
            gpu: None,
            egui_ctx: egui::Context::default(),
            egui_state: None,
            ui: UiState::new(),
            frame: Box::new([0; WIDTH * HEIGHT]),
            paused: false,
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
            lcd_px: (0.0, 0.0, 0.0, 0.0),
            fit_window: 0,
            library: Library::new(&opts_data_dir),
            discord: crate::discord::Discord::start(),
            fatal: None,
            quit: false,
        };
        // A game given on the command line starts at once; otherwise the
        // game library shows.
        match app.opts.cartridge.clone() {
            Some(cart) => app.open_cart(cart),
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
        let bios = self.settings.bios.clone().context("Choose a BIOS first (File → Open BIOS…).")?;
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
        let hd = self.settings.hd_vectors.then(|| Self::start_hd(&mut machine)).flatten();
        if self.settings.hd_vectors && hd.is_none() {
            self.ui.toast("HD vectors are not available with this BIOS");
        }
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
        let title = match (machine.cartridge(), &cart) {
            (Some(c), Some(path)) => c.known().map(|k| short_name(k.name)).unwrap_or_else(|| {
                path.file_stem().map_or_else(|| "Cartridge".into(), |s| s.to_string_lossy().into_owned())
            }),
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
        self.session = Some(Session {
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
        if let Some(s) = self.session.take() {
            let mut machine = s.emu.stop();
            if let Err(e) = s.saves.flush(&mut machine) {
                tracing::warn!(target: "ribbit::saves", "saving: {e:#}");
            }
        }
    }

    fn shutdown(&mut self) {
        self.end_session();
        self.save_settings();
    }

    fn set_bios(&mut self, path: PathBuf) {
        match ribbit_core::load_firmware(&path) {
            Ok(fw) => {
                let name = fw.known().map_or("unknown dump".to_string(), |k| k.name.to_string());
                self.settings.bios = Some(path);
                self.save_settings();
                self.ui.toast(format!("BIOS: {name}"));
                self.rescan_library();
            }
            Err(e) => self.ui.error = Some(format!("Not a usable BIOS: {e:#}")),
        }
    }

    /// Rescans the ROM folders (and recent games); art is made with the
    /// BIOS, if there is one.
    fn rescan_library(&mut self) {
        let art = self.settings.bios.clone().map(|bios| ArtSource { bios });
        self.library.scan(&self.settings.rom_folders, &self.settings.recent_carts, art);
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

    fn open_cart(&mut self, path: PathBuf) {
        if let Err(e) = self.boot(Some(path)) {
            self.ui.error = Some(format!("{e:#}"));
        }
    }

    fn pick(&mut self, what: Pick) {
        let start = match what {
            Pick::Bios => self.settings.bios.clone(),
            Pick::Cartridge => self.settings.recent_carts.first().cloned(),
            Pick::RomFolder => self.settings.rom_folders.last().cloned(),
        };
        #[cfg(not(target_os = "android"))]
        {
            if let Some(path) = crate::files::native_pick(what, start.as_deref()) {
                match what {
                    Pick::Bios => self.set_bios(path),
                    Pick::Cartridge => self.open_cart(path),
                    Pick::RomFolder => self.add_rom_folder(path),
                }
            }
        }
        #[cfg(target_os = "android")]
        self.ui.browser.start(what, start.as_deref());
    }

    fn update_title(&self) {
        if let Some(g) = &self.gpu {
            let t = match &self.session {
                Some(s) if self.paused => format!("{APP_NAME} - {} (paused)", s.title),
                Some(s) => format!("{APP_NAME} - {}", s.title),
                None => APP_NAME.to_string(),
            };
            g.window.set_title(&t);
        }
    }

    fn act(&mut self, a: Action, event_loop: &ActiveEventLoop) {
        match a {
            Action::Pick(w) => self.pick(w),
            Action::OpenCartridge(p) => self.open_cart(p),
            Action::SetBios(p) => self.set_bios(p),
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
            Action::Reset => self.reset(),
            Action::TogglePause => self.toggle_pause(),
            Action::Screenshot => self.screenshot(),
            Action::ToggleFullscreen => self.toggle_fullscreen(),
            Action::SetWindowScale(s) => {
                self.settings.window_scale = s;
                self.save_settings();
                self.fit_window = FIT_TRIES;
            }
            Action::SettingsChanged => self.save_settings(),
            Action::HdChanged => {
                self.save_settings();
                self.set_hd(self.settings.hd_vectors);
            }
            Action::SaveState(slot) => self.save_state(slot),
            Action::LoadState(slot) => self.load_state(slot),
            Action::Quit => {
                self.quit = true;
                event_loop.exit();
            }
        }
    }

    /// Turns HD capture on in `machine`; the canvas to draw into, or None
    /// if this BIOS's rasterizer is not recognised.
    fn start_hd(machine: &mut Machine) -> Option<ribbit_core::hd::HdCanvas> {
        machine.enable_hd_capture().then(|| ribbit_core::hd::HdCanvas::new(machine.hd.as_ref().unwrap().hooks))
    }

    /// HD vectors on or off for the running game.
    fn set_hd(&mut self, on: bool) {
        let Some(s) = &mut self.session else {
            return;
        };
        let mut m = s.emu.lock();
        s.hd = if on { Self::start_hd(&mut m) } else { None };
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
    /// exactly a multiple of 160 pixels square: no bars around the picture,
    /// whatever the menu and status bars measure at this display scaling.
    /// `scale` 0 = the largest that fits the monitor, as a normal window
    /// (never maximized), centred. Returns true once the area fits.
    fn fit_window(window: &Window, scale: u32, area: (f32, f32)) -> bool {
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
        let most = (room_w.min(room_h) / 160).max(1);
        let side = 160 * if scale == 0 { most } else { scale.min(most) };
        let want = PhysicalSize::new(side + chrome_w, side + chrome_h);
        tracing::debug!(target: "ribbit::window", ?inner, ?area, chrome_w, chrome_h, side, "fitting the window");
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

    fn state_path(&self, slot: u8) -> Option<PathBuf> {
        let s = self.session.as_ref()?;
        Some(self.opts.data_dir.join("states").join(format!("{}-slot{slot}.state", s.state_key)))
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
        let (Some(path), Some(s)) = (self.state_path(slot), &self.session) else {
            return;
        };
        let bytes = s.emu.lock().save_state();
        let result = (|| -> Result<()> {
            std::fs::create_dir_all(path.parent().unwrap())?;
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &path)?;
            Ok(())
        })();
        match result {
            Ok(()) => self.ui.toast(format!("State saved to slot {slot}")),
            Err(e) => self.ui.error = Some(format!("Saving state failed: {e:#}")),
        }
    }

    fn load_state(&mut self, slot: u8) {
        let Some(path) = self.state_path(slot) else {
            return;
        };
        let result = std::fs::read(&path).map_err(anyhow::Error::from).and_then(|bytes| {
            self.session.as_ref().unwrap().emu.lock().load_state(&bytes).map_err(anyhow::Error::from)
        });
        match result {
            Ok(()) => {
                let emu = &self.session.as_ref().unwrap().emu;
                emu.resync();
                if let Some(a) = &mut self.audio {
                    a.clear();
                }
                self.ui.toast(format!("State loaded from slot {slot}"));
            }
            Err(e) => self.ui.error = Some(format!("Loading state failed: {e:#}")),
        }
    }

    fn reset(&mut self) {
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
        if let Some(s) = &self.session {
            self.paused = !self.paused;
            s.emu.resync();
            if let Some(a) = &mut self.audio {
                a.clear();
            }
            self.update_title();
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
            Err(e) => self.ui.error = Some(format!("Screenshot failed: {e:#}")),
        }
    }

    /// Per frame: tells the emulation thread whether and how fast to run,
    /// and does the UI-side work (rewind, sound, autosave, speed meter).
    fn emulate(&mut self) {
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
            running: !self.paused && !self.rewinding,
            speed,
            gain,
            auto_calibrate: self.settings.auto_calibrate,
        });
        if self.paused {
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
        if let Some(a) = &self.audio
            && now.duration_since(self.audio_log.0).as_secs() >= 5
        {
            let n = a.underruns();
            if n > self.audio_log.1 {
                tracing::info!(target: "ribbit::audio", gaps = n - self.audio_log.1, speed = self.speed_percent, "audio ran dry (last 5 s)");
            }
            self.audio_log = (now, n);
        }
        if let Some(stop) = stats.stopped {
            s.emu.clear_stopped();
            self.paused = true;
            self.ui.error = Some(format!("Emulation stopped: {stop}"));
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
        self.egui_ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background)
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
        let ctrl = self.modifiers.control_key();
        if pressed {
            match code {
                KeyCode::Escape => return self.toggle_pause(),
                KeyCode::F1 => return self.ui.show_controls = true,
                KeyCode::F10 => return self.ui.menu_visible = !self.ui.menu_visible,
                KeyCode::F11 => return self.toggle_fullscreen(),
                KeyCode::F12 => return self.screenshot(),
                KeyCode::F5 => return self.save_state(1),
                KeyCode::F9 => return self.load_state(1),
                KeyCode::KeyO if ctrl => return self.act(Action::Pick(Pick::Cartridge), event_loop),
                KeyCode::KeyR if ctrl => return self.reset(),
                _ => {}
            }
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
        if let (Some(b), Some(s)) = (self.settings.button_for(code), &self.session) {
            s.emu.lock().buttons.set(b, pressed);
        }
    }

    /// Android gamepad buttons (winit reports them as raw key codes; the
    /// D-pad already arrives as arrow keys).
    fn android_key(&mut self, code: u32, pressed: bool) {
        use ribbit_input::Button;
        const BACK: u32 = 4;
        if code == BACK {
            if pressed {
                self.ui.menu_visible = true;
                if !self.paused {
                    self.toggle_pause();
                }
            }
            return;
        }
        let button = match code {
            96 => Button::A,           // BUTTON_A
            97 => Button::B,           // BUTTON_B
            99 | 100 => Button::Hint,  // BUTTON_X / BUTTON_Y
            108 => Button::Pause,      // BUTTON_START
            109 => Button::Home,       // BUTTON_SELECT
            102 => Button::VolumeDown, // BUTTON_L1
            103 => Button::VolumeUp,   // BUTTON_R1
            _ => return,
        };
        if let Some(s) = &self.session {
            s.emu.lock().buttons.set(button, pressed);
        }
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
        for e in events {
            match e {
                PadEvent::Button(b, pressed) => {
                    if let Some(s) = &self.session {
                        s.emu.lock().buttons.set(b, pressed);
                    }
                }
                PadEvent::Menu => {
                    self.ui.menu_visible = true;
                    if self.session.is_some() && !self.paused {
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
        let mut hd_frame = false;
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
            // EXPERIMENTAL HD: this frame's renders at 4x, kept true to the LCD.
            if let Some(canvas) = &mut s.hd
                && !hidden
            {
                for p in &passes {
                    canvas.apply(p, &self.frame[..]);
                }
                canvas.reconcile(&self.frame[..]);
                hd_frame = true;
            }
        }
        let save_dir = self.save_dir().display().to_string();
        let slots = self.slot_times();
        let (Some(gpu), Some(state)) = (&mut self.gpu, &mut self.egui_state) else {
            return;
        };
        gpu.upload(&self.frame);
        gpu.show_hd = hd_frame;
        if let Some(c) = self.session.as_ref().and_then(|s| s.hd.as_ref()).filter(|_| hd_frame) {
            gpu.upload_hd(&c.pixels);
        }
        let raw = state.take_egui_input(&gpu.window);
        let info =
            ui::Info {
                running: self.session.is_some(),
                paused: self.paused,
                title: self.session.as_ref().map(|s| s.title.clone()).unwrap_or_default(),
                speed: self.speed_percent,
                bios_name: self.settings.bios.as_ref().map(|p| {
                    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
                }),
                fullscreen: gpu.window.fullscreen().is_some(),
                experimental_touch: self.session.as_ref().is_some_and(|s| s.experimental_touch),
                save_dir,
                slots,
                rewinding: self.rewinding,
            };
        let mut actions = Vec::new();
        let mut area = egui::Rect::NOTHING;
        self.library.poll();
        self.discord.set(match &self.session {
            _ if !self.settings.discord_presence => crate::discord::Presence::Off,
            Some(s) => crate::discord::Presence::playing(&s.title, s.started),
            None => crate::discord::Presence::Browsing,
        });
        let (ui_state, settings, library) = (&mut self.ui, &mut self.settings, &mut self.library);
        let out = self.egui_ctx.run_ui(raw, |ui| {
            area = ui::draw(ui, ui_state, settings, library, &info, &mut actions);
        });
        state.handle_platform_output(&gpu.window, out.platform_output);
        let ppp = out.pixels_per_point;
        if self.fit_window > 0 {
            self.fit_window -= 1;
            if Self::fit_window(&gpu.window, self.settings.window_scale, (area.width() * ppp, area.height() * ppp)) {
                self.fit_window = 0;
            }
        }
        let display = self.settings.display;
        self.lcd_px =
            Gpu::lcd_rect((area.min.x * ppp, area.min.y * ppp, area.width() * ppp, area.height() * ppp), display);
        let jobs = self.egui_ctx.tessellate(out.shapes, ppp);
        let lcd = self.session.is_some().then_some((self.lcd_px, display, self.settings.ai));
        if let Err(e) = gpu.render(lcd, &jobs, &out.textures_delta, ppp) {
            self.fatal = Some(e);
            event_loop.exit();
            return;
        }
        for a in actions {
            self.act(a, event_loop);
        }
        let took = frame_start.elapsed();
        if took.as_millis() >= 12 {
            tracing::debug!(target: "ribbit::frame", ms = took.as_secs_f64() * 1e3, "slow frame (excluding vsync wait)");
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
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
            .with_min_inner_size(PhysicalSize::new(320, 240))
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
        self.gpu = None;
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
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(g) = &self.gpu {
            g.window.request_redraw();
        }
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
