//! The Ribbit application: one window, the emulated Leapster, egui menus.

use crate::audio_out::AudioOut;
use crate::files::Pick;
use crate::gpu::Gpu;
use crate::saves::{AutoSave, SavePaths};
use crate::settings::Settings;
use crate::ui::{self, Action, UiState};
use anyhow::{Context, Result};
use ribbit_core::{Machine, StopReason};
use ribbit_debug::Breakpoints;
use ribbit_video::{HEIGHT, WIDTH};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{ElementState, KeyEvent, MouseButton, TouchPhase, WindowEvent};
use winit::event_loop::{ActiveEventLoop, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

/// Never emulate more than this per frame; drop lag beyond `MAX_LAG`.
const MAX_SLICE: Duration = Duration::from_millis(100);
const MAX_LAG: Duration = Duration::from_millis(250);

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
    machine: Machine,
    saves: SavePaths,
    autosave: AutoSave,
    title: String,
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
    modifiers: ModifiersState,
    pace: Option<(Instant, u64)>,
    speed: (Instant, u64, f64),
    audio: Option<AudioOut>,
    audio_buf: Vec<i16>,
    cursor: Option<(f64, f64)>,
    mouse_down: bool,
    finger: Option<(u64, (f64, f64))>,
    lcd_px: (f32, f32, f32, f32),
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
            modifiers: ModifiersState::empty(),
            pace: None,
            speed: (Instant::now(), 0, 100.0),
            audio: AudioOut::open(),
            audio_buf: Vec::new(),
            cursor: None,
            mouse_down: false,
            finger: None,
            lcd_px: (0.0, 0.0, 0.0, 0.0),
            fatal: None,
            quit: false,
        };
        let cart = app.opts.cartridge.clone();
        if (cart.is_some() || app.settings.bios.is_some())
            && let Err(e) = app.boot(cart)
        {
            app.ui.error = Some(format!("{e:#}"));
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
        let default_straps = if self.settings.skip_boot_calibration {
            0x63FF_FFFF
        } else {
            ribbit_core::io::straps::MAME_LEAPSTER2_MEASURED
        };
        let mut machine = Machine::with_straps(firmware, cartridge, self.opts.straps.unwrap_or(default_straps));
        if let Some(mhz) = self.opts.cpu_mhz {
            machine.clock = ribbit_core::Clock::new(mhz * 1_000_000, ribbit_core::Clock::PERIPHERAL_HZ);
        }
        if self.opts.experimental_touch.unwrap_or(self.settings.experimental_touch) {
            machine.enable_experimental_mame_touch(Default::default());
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
        self.session = Some(Session { machine, saves, autosave: AutoSave::default(), title });
        self.paused = false;
        self.pace = None;
        if let Some(a) = &mut self.audio {
            a.clear();
        }
        self.update_title();
        Ok(())
    }

    fn end_session(&mut self) {
        if let Some(mut s) = self.session.take()
            && let Err(e) = s.saves.flush(&mut s.machine)
        {
            tracing::warn!(target: "ribbit::saves", "saving: {e:#}");
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
                if self.session.is_none()
                    && let Err(e) = self.boot(None)
                {
                    self.ui.error = Some(format!("{e:#}"));
                }
            }
            Err(e) => self.ui.error = Some(format!("Not a usable BIOS: {e:#}")),
        }
    }

    fn open_dropped(&mut self, path: PathBuf) {
        // A BIOS is recognised by checksum; anything else is tried as a cartridge.
        let is_bios = std::fs::read(&path)
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
        };
        #[cfg(not(target_os = "android"))]
        {
            if let Some(path) = crate::files::native_pick(what, start.as_deref()) {
                match what {
                    Pick::Bios => self.set_bios(path),
                    Pick::Cartridge => self.open_cart(path),
                }
            }
            self.pace = None; // the dialog blocked; don't race to catch up
        }
        #[cfg(target_os = "android")]
        self.ui.browser.start(what, start.as_deref());
    }

    fn update_title(&self) {
        if let Some(g) = &self.gpu {
            let t = match &self.session {
                Some(s) if self.paused => format!("Ribbit - {} (paused)", s.title),
                Some(s) => format!("Ribbit - {}", s.title),
                None => "Ribbit".to_string(),
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
            }
            Action::Reset => self.reset(),
            Action::TogglePause => self.toggle_pause(),
            Action::Screenshot => self.screenshot(),
            Action::ToggleFullscreen => self.toggle_fullscreen(),
            Action::SetWindowScale(s) => {
                self.settings.window_scale = s;
                self.save_settings();
                if let Some(g) = &self.gpu {
                    let _ = g.window.request_inner_size(Self::window_size(s));
                }
            }
            Action::SettingsChanged => self.save_settings(),
            Action::Quit => {
                self.quit = true;
                event_loop.exit();
            }
        }
    }

    fn window_size(scale: u32) -> PhysicalSize<u32> {
        // Room for the menu and status bars.
        PhysicalSize::new(160 * scale, 160 * scale + 56)
    }

    fn reset(&mut self) {
        if let Some(s) = &mut self.session {
            let _ = s.saves.flush(&mut s.machine);
            s.machine.reset();
            self.pace = None;
            self.ui.toast("Reset");
        }
    }

    fn toggle_pause(&mut self) {
        if self.session.is_some() {
            self.paused = !self.paused;
            self.pace = None;
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

    /// Emulates as much as real time (times the speed setting) allows.
    fn emulate(&mut self) {
        let Some(s) = &mut self.session else {
            return;
        };
        if self.paused {
            return;
        }
        let m = &mut s.machine;
        let hz = m.clock.cpu_hz as f64;
        let speed = if self.fast_forward || self.settings.speed_percent == 0 {
            f64::INFINITY
        } else {
            self.settings.speed_percent as f64 / 100.0
        };
        let now = Instant::now();
        let steps = if speed.is_finite() {
            let (origin, base) = *self.pace.get_or_insert((now, m.cpu.cycles));
            let target = base + (now.duration_since(origin).as_secs_f64() * hz * speed) as u64;
            let behind = target.saturating_sub(m.cpu.cycles);
            let max_lag = (MAX_LAG.as_secs_f64() * hz * speed) as u64;
            if behind > max_lag {
                self.pace = Some((now, m.cpu.cycles));
            }
            behind.min(max_lag).min((MAX_SLICE.as_secs_f64() * hz * speed) as u64)
        } else {
            self.pace = None;
            (hz * 0.25) as u64 // fast-forward: a big slice per frame
        };
        let r = m.run(steps, &Breakpoints::default());
        if r.stop != StopReason::StepLimit {
            self.paused = true;
            self.ui.error = Some(format!("Emulation stopped: {:?}", r.stop));
            tracing::warn!(target: "ribbit::boot", ?r, "emulation stopped");
        }
        // Sound.
        let out = &mut m.io.voices.output;
        self.audio_buf.resize(out.len(), 0);
        out.pop_into(&mut self.audio_buf);
        if let Some(a) = &mut self.audio {
            if speed.is_finite() && speed == 1.0 {
                let gain = if self.settings.muted { 0.0 } else { self.settings.volume };
                a.push(&self.audio_buf, gain);
            } else {
                a.clear(); // no pitch-shifted audio at other speeds
            }
        }
        // Saves.
        if let Err(e) = s.autosave.tick(&s.saves, m, now) {
            tracing::warn!(target: "ribbit::saves", "autosave failed: {e:#}");
        }
        // Speed meter.
        let elapsed = now.duration_since(self.speed.0).as_secs_f64();
        if elapsed >= 1.0 {
            let done = m.cpu.cycles.saturating_sub(self.speed.1) as f64;
            self.speed = (now, m.cpu.cycles, 100.0 * done / (elapsed * hz));
        }
    }

    /// EXPERIMENTAL touch: the stylus at a window position (None = up).
    fn set_stylus(&mut self, pos: Option<(f64, f64)>) {
        let Some(s) = &mut self.session else {
            return;
        };
        let Some(touch) = s.machine.io.touch.as_mut() else {
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
                KeyCode::KeyO if ctrl => return self.act(Action::Pick(Pick::Cartridge), event_loop),
                KeyCode::KeyR if ctrl => return self.reset(),
                _ => {}
            }
        }
        if code == KeyCode::Tab {
            self.fast_forward = pressed;
            self.pace = None;
            return;
        }
        if let (Some(b), Some(s)) = (self.settings.button_for(code), &mut self.session) {
            s.machine.buttons.set(b, pressed);
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
        if let Some(s) = &mut self.session {
            s.machine.buttons.set(button, pressed);
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        self.emulate();
        if let Some(s) = &self.session {
            s.machine.render_frame(&mut self.frame);
        }
        let save_dir = self.save_dir().display().to_string();
        let (Some(gpu), Some(state)) = (&mut self.gpu, &mut self.egui_state) else {
            return;
        };
        gpu.upload(&self.frame);
        let raw = state.take_egui_input(&gpu.window);
        let info =
            ui::Info {
                running: self.session.is_some(),
                paused: self.paused,
                title: self.session.as_ref().map(|s| s.title.clone()).unwrap_or_default(),
                speed: self.speed.2,
                bios_name: self.settings.bios.as_ref().map(|p| {
                    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
                }),
                fullscreen: gpu.window.fullscreen().is_some(),
                experimental_touch: self.session.as_ref().is_some_and(|s| s.machine.experimental_touch_enabled()),
                save_dir,
            };
        let mut actions = Vec::new();
        let mut area = egui::Rect::NOTHING;
        let (ui_state, settings) = (&mut self.ui, &mut self.settings);
        let out = self.egui_ctx.run_ui(raw, |ui| {
            area = ui::draw(ui, ui_state, settings, &info, &mut actions);
        });
        state.handle_platform_output(&gpu.window, out.platform_output);
        let ppp = out.pixels_per_point;
        let display = self.settings.display;
        self.lcd_px =
            Gpu::lcd_rect((area.min.x * ppp, area.min.y * ppp, area.width() * ppp, area.height() * ppp), display);
        let jobs = self.egui_ctx.tessellate(out.shapes, ppp);
        let lcd = self.session.is_some().then_some((self.lcd_px, display));
        if let Err(e) = gpu.render(lcd, &jobs, &out.textures_delta, ppp) {
            self.fatal = Some(e);
            event_loop.exit();
            return;
        }
        for a in actions {
            self.act(a, event_loop);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gpu.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Ribbit")
            .with_inner_size(Self::window_size(self.settings.window_scale.clamp(1, 8)))
            .with_min_inner_size(PhysicalSize::new(320, 240))
            .with_window_icon(crate::icon::window_icon());
        #[cfg(windows)]
        let attrs = {
            use winit::platform::windows::WindowAttributesExtWindows as _;
            attrs.with_taskbar_icon(crate::icon::taskbar_icon())
        };
        match event_loop.create_window(attrs).context("creating window").and_then(|w| Gpu::new(Arc::new(w))) {
            Ok(gpu) => {
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
            let _ = s.saves.flush(&mut s.machine);
        }
        self.save_settings();
        self.gpu = None;
        self.egui_state = None;
        self.pace = None;
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let consumed = match (&mut self.egui_state, &self.gpu) {
            (Some(st), Some(g)) => st.on_window_event(&g.window, &event).consumed,
            _ => false,
        };
        let over_ui = self.egui_ctx.is_pointer_over_egui() || self.egui_ctx.egui_wants_pointer_input();
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
                if down && over_ui {
                    return;
                }
                self.mouse_down = down;
                self.set_stylus(if down { self.cursor } else { None });
            }
            WindowEvent::Touch(t) => {
                let pos = (t.location.x, t.location.y);
                match t.phase {
                    TouchPhase::Started if self.finger.is_none() && !over_ui => {
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

/// "Game (USA, alt) [500-11002 / 500-01168] (MAME: x)" -> "Game (USA, alt)".
fn short_name(full: &str) -> String {
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
