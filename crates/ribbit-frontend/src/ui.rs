//! Menus, dialogs and overlays (egui). The UI only reads state and emits
//! `Action`s; `app.rs` performs them.

use crate::files::{Browser, Pick};
use crate::gpu::Display;
use crate::library::{Art, Library};
use crate::settings::{self, BINDABLE, Settings};
use ribbit_input::Button;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Pick(Pick),
    OpenCartridge(PathBuf),
    SetBios(PathBuf),
    CloseCartridge,
    Reset,
    TogglePause,
    Screenshot,
    ToggleFullscreen,
    SetWindowScale(u32),
    SettingsChanged,
    /// Back into the game closed for inactivity, where it was.
    ResumeAfterIdle,
    /// Hide the "closed after inactivity" banner.
    DismissIdleBanner,
    /// The HD vectors setting changed (applies to the running game at once).
    HdChanged,
    SaveState(u8),
    LoadState(u8),
    AddRomFolder(PathBuf),
    RemoveRomFolder(PathBuf),
    RescanLibrary,
    /// Forget a game's art and make it again.
    RemakeArt(PathBuf),
    Quit,
}

pub const STATE_SLOTS: u8 = 5;

/// What the UI shows (read-only snapshot).
pub struct Info {
    pub running: bool,
    pub paused: bool,
    pub title: String,
    pub speed: f64,
    pub bios_name: Option<String>,
    pub fullscreen: bool,
    pub experimental_touch: bool,
    pub save_dir: String,
    /// When each save-state slot was last written (None = empty).
    pub slots: Vec<Option<String>>,
    pub rewinding: bool,
}

#[derive(Default)]
pub struct UiState {
    pub menu_visible: bool,
    pub show_bindings: bool,
    pub show_controls: bool,
    pub show_about: bool,
    pub show_status: bool,
    /// Waiting for a key to bind to this button.
    pub capturing: Option<Button>,
    pub browser: Browser,
    toasts: Vec<(String, Instant)>,
    pub error: Option<String>,
    /// The game (title) that was closed because the Leapster turned itself
    /// off; shown above the library until dismissed.
    pub idle_closed: Option<String>,
    /// When the library last appeared (its cards grow in from then).
    library_since: Option<Instant>,
    was_running: bool,
}

const TOAST_TIME: Duration = Duration::from_millis(2500);

impl UiState {
    pub fn new() -> Self {
        Self { menu_visible: true, ..Default::default() }
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.toasts.push((text.into(), Instant::now()));
        if self.toasts.len() > 4 {
            self.toasts.remove(0);
        }
    }
}

/// Draws the whole UI; returns the area left for the game screen (points).
pub fn draw(
    ui: &mut egui::Ui,
    st: &mut UiState,
    settings: &mut Settings,
    library: &mut Library,
    info: &Info,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    let ctx = ui.ctx().clone();
    if st.menu_visible && !info.fullscreen {
        egui::Panel::top("menu").show(ui, |ui| menu_bar(ui, st, settings, info, actions));
    }
    if settings.show_status_bar && !info.fullscreen {
        egui::Panel::bottom("status").show(ui, |ui| status_bar(ui, info));
    }
    let was_running = std::mem::replace(&mut st.was_running, info.running);
    let mut area = egui::Rect::NOTHING;
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        area = ui.max_rect();
        if !info.running && info.bios_name.is_some() {
            if was_running || st.library_since.is_none() {
                st.library_since = Some(Instant::now());
            }
            let since = st.library_since.unwrap_or_else(Instant::now);
            library_screen(ui, settings, library, st.idle_closed.as_deref(), since, actions);
        } else if !info.running {
            welcome(ui, settings, info, actions);
        } else if info.rewinding {
            ui.painter().text(
                area.left_top() + egui::vec2(12.0, 10.0),
                egui::Align2::LEFT_TOP,
                "⏪ Rewinding",
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
            );
        } else if info.paused {
            let p = ui.painter();
            p.rect_filled(area, 0.0, egui::Color32::from_black_alpha(140));
            p.text(
                area.center(),
                egui::Align2::CENTER_CENTER,
                "⏸ Paused",
                egui::FontId::proportional(28.0),
                egui::Color32::WHITE,
            );
        }
    });
    dialogs(&ctx, st, settings, info, actions);
    if let Some((pick, path)) = st.browser.ui(&ctx) {
        actions.push(match pick {
            Pick::Bios => Action::SetBios(path),
            Pick::Cartridge => Action::OpenCartridge(path),
            Pick::RomFolder => Action::AddRomFolder(path),
        });
    }
    toasts(&ctx, st);
    area
}

fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui.add(egui::Button::new(label).shortcut_text(shortcut)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn menu_bar(ui: &mut egui::Ui, st: &mut UiState, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>) {
    egui::MenuBar::new().ui(ui, |ui| {
        ui.menu_button("File", |ui| {
            if item(ui, "Open Cartridge…", "Ctrl+O") {
                actions.push(Action::Pick(Pick::Cartridge));
            }
            ui.menu_button("Open Recent", |ui| {
                if settings.recent_carts.is_empty() {
                    ui.label(egui::RichText::new("(none yet)").weak());
                }
                for path in settings.recent_carts.clone() {
                    let name = path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                    if ui.button(name).on_hover_text(path.display().to_string()).clicked() {
                        actions.push(Action::OpenCartridge(path));
                        ui.close();
                    }
                }
            });
            ui.menu_button("Games Folders", |ui| {
                for f in settings.rom_folders.clone() {
                    let b = egui::Button::new(format!("✕  {}", f.display()));
                    if ui.add(b).on_hover_text("Stop looking in this folder").clicked() {
                        actions.push(Action::RemoveRomFolder(f));
                        ui.close();
                    }
                }
                if !settings.rom_folders.is_empty() {
                    ui.separator();
                }
                if item(ui, "Add Games Folder…", "") {
                    actions.push(Action::Pick(Pick::RomFolder));
                }
            });
            if ui.add_enabled(info.running, egui::Button::new("Close Cartridge (back to games)")).clicked() {
                actions.push(Action::CloseCartridge);
                ui.close();
            }
            ui.separator();
            if item(ui, "Open BIOS…", "") {
                actions.push(Action::Pick(Pick::Bios));
            }
            ui.separator();
            if ui.add_enabled(info.running, egui::Button::new("Screenshot").shortcut_text("F12")).clicked() {
                actions.push(Action::Screenshot);
                ui.close();
            }
            if ui.checkbox(&mut settings.discord_presence, "Show my game on Discord").changed() {
                actions.push(Action::SettingsChanged);
            }
            ui.separator();
            if item(ui, "Exit", "Alt+F4") {
                actions.push(Action::Quit);
            }
        });
        ui.menu_button("Emulation", |ui| {
            let pause = if info.paused { "Resume" } else { "Pause" };
            if ui.add_enabled(info.running, egui::Button::new(pause).shortcut_text("Esc")).clicked() {
                actions.push(Action::TogglePause);
                ui.close();
            }
            if ui.add_enabled(info.running, egui::Button::new("Reset").shortcut_text("Ctrl+R")).clicked() {
                actions.push(Action::Reset);
                ui.close();
            }
            ui.separator();
            ui.add_enabled_ui(info.running, |ui| {
                ui.menu_button("Save State", |ui| {
                    for slot in 1..=STATE_SLOTS {
                        let when = info.slots.get(slot as usize - 1).cloned().flatten().unwrap_or_else(|| "empty".into());
                        let key = if slot == 1 { "F5" } else { "" };
                        if ui.add(egui::Button::new(format!("Slot {slot}  ({when})")).shortcut_text(key)).clicked() {
                            actions.push(Action::SaveState(slot));
                            ui.close();
                        }
                    }
                });
                ui.menu_button("Load State", |ui| {
                    for slot in 1..=STATE_SLOTS {
                        let when = info.slots.get(slot as usize - 1).cloned().flatten();
                        let key = if slot == 1 { "F9" } else { "" };
                        let label = format!("Slot {slot}  ({})", when.as_deref().unwrap_or("empty"));
                        if ui.add_enabled(when.is_some(), egui::Button::new(label).shortcut_text(key)).clicked() {
                            actions.push(Action::LoadState(slot));
                            ui.close();
                        }
                    }
                });
                ui.label(egui::RichText::new("Hold ` (left of 1) to rewind").weak());
            });
            ui.separator();
            ui.menu_button("Speed", |ui| {
                for (p, label) in [(50, "50%"), (100, "100% (normal)"), (200, "200%"), (400, "400%"), (0, "Unlimited")] {
                    if ui.radio_value(&mut settings.speed_percent, p, label).clicked() {
                        actions.push(Action::SettingsChanged);
                        ui.close();
                    }
                }
                ui.label(egui::RichText::new("Hold Tab to fast-forward").weak());
            });
            ui.separator();
            // Power.
            if ui
                .checkbox(&mut settings.turn_off_when_idle, "Turn off when idle")
                .on_hover_text("Like the real Leapster, turn off after a while with nothing pressed (about 15 minutes; 5 on the Leapster TV). Ribbit then closes the game. Off: Ribbit keeps it on.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
            ui.separator();
            // Touch, then how its calibration is handled.
            if ui
                .checkbox(&mut settings.experimental_touch, "Touchscreen (experimental)")
                .on_hover_text("MAME-derived, unverified touch model. Needed for calibration and touch games. Applies at the next start.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
            if ui
                .checkbox(&mut settings.skip_boot_calibration, "Skip calibration at boot")
                .on_hover_text("Sets board straps bit 14 (MAME's default) so the BIOS uses the saved calibration. Applies at the next start.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
            if ui
                .checkbox(&mut settings.auto_calibrate, "Calibrate automatically")
                .on_hover_text("Ribbit presses the calibration targets for you when the calibration screen appears.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
            ui.separator();
            ui.menu_button("Advanced (experimental)", |ui| {
                ui.label(egui::RichText::new("Unverified guesses. Apply at the next start.").weak());
                if ui
                    .checkbox(&mut settings.mame_gpio_lines, "MAME power-line workaround")
                    .on_hover_text(
                        "Reads every unidentified GPIO line as 1, as MAME does. The v1.5 (Universal) BIOS otherwise                          sees a shutdown request on line 11 and turns itself off. Not known to match hardware.",
                    )
                    .changed()
                {
                    actions.push(Action::SettingsChanged);
                }
                ui.label("CPU clock:");
                for (mhz, label) in [(96, "96 MHz (nominal)"), (48, "48 MHz (MAME)")] {
                    if ui.radio_value(&mut settings.cpu_mhz, mhz, label).clicked() {
                        actions.push(Action::SettingsChanged);
                    }
                }
            });
        });
        ui.menu_button("Video", |ui| {
            if ui
                .checkbox(&mut settings.hd_vectors, "HD vectors, 640x640 (experimental)")
                .on_hover_text("Redraws the game's vector art at 4x from the BIOS's own shape data. Bitmaps and anything not yet understood show the normal picture. Replaces the scaling options below while on.")
                .changed()
            {
                actions.push(Action::HdChanged);
            }
            ui.separator();
            let before = settings.display;
            ui.radio_value(&mut settings.display, Display::Ai, "AI upscale (HD)");
            ui.radio_value(&mut settings.display, Display::Sharp, "Sharp pixels");
            ui.radio_value(&mut settings.display, Display::Lcd, "LCD grid");
            ui.radio_value(&mut settings.display, Display::Pixel, "Pixel perfect (whole-number scale)");
            ui.radio_value(&mut settings.display, Display::Smooth, "Smooth (bicubic)");
            if settings.display != before {
                actions.push(Action::SettingsChanged);
            }
            ui.menu_button("AI upscale settings", |ui| {
                use crate::upscale::{Model, Resolution, Style};
                let before = settings.ai;
                ui.label(egui::RichText::new("Style").strong());
                ui.radio_value(&mut settings.ai.style, Style::Clean, "Clean (ArtCNN)")
                    .on_hover_text("Crisp lines and flat fills. Best for Leapster games, whose art is antialiased vector graphics.");
                ui.radio_value(&mut settings.ai.style, Style::Soft, "Soft (Anime4K)")
                    .on_hover_text("A softer neural upscaler.");
                ui.radio_value(&mut settings.ai.style, Style::Cartoon, "Pixel-art edges (ScaleFX + ArtCNN)")
                    .on_hover_text("For flat pixel art. Blotchy on antialiased art.");
                ui.separator();
                ui.label(egui::RichText::new("Quality").strong());
                for m in Model::ALL {
                    ui.radio_value(&mut settings.ai.model, m, m.label());
                }
                ui.label(egui::RichText::new("Bigger networks are sharper and use more GPU.").weak());
                ui.separator();
                ui.label(egui::RichText::new("Sharpness").strong());
                for sh in crate::upscale::Sharpness::ALL {
                    ui.radio_value(&mut settings.ai.sharpness, sh, sh.label());
                }
                ui.separator();
                ui.label(egui::RichText::new("Resolution").strong());
                for r in Resolution::ALL {
                    ui.radio_value(&mut settings.ai.resolution, r, r.label());
                }
                if settings.ai != before {
                    settings.display = Display::Ai;
                    actions.push(Action::SettingsChanged);
                }
            });
            ui.separator();
            ui.menu_button("Window size", |ui| {
                for s in [0, 2, 3, 4, 16] {
                    let label = if s == 0 { "Fit screen".to_string() } else { format!("{s}x ({}x{})", 160 * s, 160 * s) };
                    if ui.radio(settings.window_scale == s, label).clicked() {
                        actions.push(Action::SetWindowScale(s));
                        ui.close();
                    }
                }
            });
            if item(ui, if info.fullscreen { "Exit fullscreen" } else { "Fullscreen" }, "F11") {
                actions.push(Action::ToggleFullscreen);
            }
            if ui.checkbox(&mut settings.show_status_bar, "Status bar").changed() {
                actions.push(Action::SettingsChanged);
            }
        });
        ui.menu_button("Audio", |ui| {
            if ui.checkbox(&mut settings.muted, "Mute").changed() {
                actions.push(Action::SettingsChanged);
            }
            if ui.add(egui::Slider::new(&mut settings.volume, 0.0..=1.0).text("Volume").show_value(false)).changed() {
                actions.push(Action::SettingsChanged);
            }
            ui.separator();
            ui.label(egui::RichText::new("Sound").strong());
            for e in crate::audio_out::Enhance::ALL {
                if ui.radio_value(&mut settings.audio_enhance, e, e.label()).clicked() {
                    actions.push(Action::SettingsChanged);
                }
            }
            ui.label(egui::RichText::new("The Leapster itself is mono with 8 kHz sound; these are effects.").weak());
        });
        ui.menu_button("Input", |ui| {
            if item(ui, "Key bindings…", "") {
                st.show_bindings = true;
            }
        });
        ui.menu_button("Help", |ui| {
            if item(ui, "Controls", "F1") {
                st.show_controls = true;
            }
            if item(ui, "Emulation status", "") {
                st.show_status = true;
            }
            ui.separator();
            if item(ui, "About Ribbit", "") {
                st.show_about = true;
            }
        });
    });
}

fn status_bar(ui: &mut egui::Ui, info: &Info) {
    ui.horizontal(|ui| {
        ui.label(if info.running { info.title.as_str() } else { "No game running" });
        if info.experimental_touch && info.running {
            ui.label(egui::RichText::new("· touch: experimental").weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if info.running {
                let text = if info.paused { "paused".to_string() } else { format!("{:.0}% speed", info.speed) };
                ui.label(text);
            }
        });
    });
}

fn welcome(ui: &mut egui::Ui, settings: &Settings, info: &Info, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.18).max(12.0));
        ui.heading(egui::RichText::new("🐸 Ribbit").size(34.0));
        ui.label("An emulator for the original LeapFrog Leapster (2003)");
        ui.add_space(18.0);
        match &info.bios_name {
            Some(name) => ui.label(format!("BIOS: {name}")),
            None => ui.label("Step 1: choose the BIOS dumped from your own Leapster."),
        };
        if ui.add_sized([220.0, 34.0], egui::Button::new("Open BIOS…")).clicked() {
            actions.push(Action::Pick(Pick::Bios));
        }
        ui.add_space(8.0);
        let has_bios = info.bios_name.is_some();
        if ui.add_enabled(has_bios, egui::Button::new("Open Cartridge…").min_size([220.0, 34.0].into())).clicked() {
            actions.push(Action::Pick(Pick::Cartridge));
        }
        if has_bios && !settings.recent_carts.is_empty() {
            ui.add_space(14.0);
            ui.label(egui::RichText::new("Recent").strong());
            for path in settings.recent_carts.iter().take(5) {
                let name =
                    path.file_stem().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned());
                if ui.link(name).clicked() {
                    actions.push(Action::OpenCartridge(path.clone()));
                }
            }
        }
        ui.add_space(14.0);
        ui.label(egui::RichText::new("You can also drop a BIOS or cartridge file onto this window.").weak());
        ui.label(egui::RichText::new("No firmware or games are included: use dumps of hardware you own.").weak());
    });
}

/// Card width the grid aims for (points).
const CARD_WIDTH: f32 = 220.0;
const CARD_GAP: f32 = 22.0;
/// Ribbit's green (the frog), for play buttons and highlights.
const ACCENT: egui::Color32 = egui::Color32::from_rgb(46, 170, 82);
const ACCENT_HOVER: egui::Color32 = egui::Color32::from_rgb(64, 196, 104);

/// The game library: art and info for every game in the ROM folders;
/// clicking a game plays it.
fn library_screen(
    ui: &mut egui::Ui,
    settings: &Settings,
    library: &mut Library,
    idle_closed: Option<&str>,
    since: Instant,
    actions: &mut Vec<Action>,
) {
    let ctx = ui.ctx().clone();
    for g in &mut library.games {
        if let Art::Image(img) = &g.art {
            let tex = ctx.load_texture(g.path.display().to_string(), img.clone(), egui::TextureOptions::LINEAR);
            g.art = Art::Texture(tex);
        }
    }
    backdrop(ui);
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(28, 20)).show(ui, |ui| {
        library_header(ui, library, actions);
        if library.games.is_empty() && !library.scanning {
            empty_library(ui, settings, actions);
            return;
        }
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if settings.rom_folders.is_empty() {
                // Only recent games so far: ask for the folder with the rest.
                notice(ui, "These are your recent games. Choose your games folder to see all of them.", || {
                    actions.push(Action::Pick(Pick::RomFolder));
                });
                ui.add_space(18.0);
            }
            if let Some(title) = idle_closed {
                idle_banner(ui, title, actions);
                ui.add_space(14.0);
            }
            let last = settings.recent_carts.first();
            if let Some(game) = last.and_then(|p| library.games.iter().find(|g| &g.path == p)) {
                hero(ui, game, actions);
                ui.add_space(28.0);
            }
            ui.label(egui::RichText::new(format!("ALL GAMES  ·  {}", library.games.len())).size(13.0).strong().color(ACCENT));
            ui.add_space(12.0);
            let avail = ui.available_width();
            let cols = (((avail + CARD_GAP) / (CARD_WIDTH + CARD_GAP)).floor() as usize).max(1);
            let card_w = ((avail - CARD_GAP * (cols - 1) as f32) / cols as f32).floor().min(CARD_WIDTH * 1.4);
            let shown = since.elapsed().as_secs_f32();
            for (r, row) in library.games.chunks(cols).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = CARD_GAP;
                    for (c, game) in row.iter().enumerate() {
                        // Each card grows in a moment after the one before.
                        let n = (r * cols + c).min(CARD_STAGGER_MAX);
                        let t = ((shown - CARD_STAGGER * n as f32) / CARD_GROW).clamp(0.0, 1.0);
                        if t < 1.0 {
                            ui.ctx().request_repaint();
                        }
                        game_card(ui, game, card_w, 1.0 - (1.0 - t).powi(3), actions);
                    }
                });
                ui.add_space(CARD_GAP);
            }
        });
    });
}

/// A soft green glow at the top fading into the window background.
fn backdrop(ui: &egui::Ui) {
    let r = ui.max_rect();
    let top = egui::Color32::from_rgb(18, 46, 30);
    let bottom = egui::Color32::from_rgb(11, 12, 14);
    let mid = egui::pos2(r.max.x, r.min.y + r.height() * 0.55);
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(r.left_top(), top);
    mesh.colored_vertex(r.right_top(), top);
    mesh.colored_vertex(egui::pos2(r.min.x, mid.y), bottom);
    mesh.colored_vertex(mid, bottom);
    mesh.colored_vertex(r.left_bottom(), bottom);
    mesh.colored_vertex(r.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(1, 3, 2);
    mesh.add_triangle(2, 3, 4);
    mesh.add_triangle(3, 5, 4);
    ui.painter().add(mesh);
}

fn library_header(ui: &mut egui::Ui, library: &Library, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let frog = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("frog icon"))).unwrap_or_else(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied([256, 256], &crate::icon::frog_glyph_rgba(256));
            let tex = ui.ctx().load_texture("frog icon", img, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("frog icon"), tex.clone()));
            tex
        });
        ui.add(egui::Image::new((frog.id(), egui::vec2(42.0, 42.0))));
        ui.add_space(4.0);
        ui.label(egui::RichText::new("Games").size(34.0).family(crate::fonts::title()).color(egui::Color32::WHITE));
        // Progress, next to the heading.
        let pending = library.games.iter().filter(|g| matches!(g.art, Art::Pending)).count();
        if library.scanning {
            ui.add_space(10.0);
            ui.spinner();
            ui.label(egui::RichText::new("Looking for games…").weak());
        } else if library.making_art && pending > 0 {
            ui.add_space(10.0);
            ui.spinner();
            ui.label(egui::RichText::new(format!("Getting title screens… {pending} to go")).weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(egui::Button::new(egui::RichText::new("⟳").size(16.0)).corner_radius(8).min_size(egui::vec2(36.0, 32.0)))
                .on_hover_text("Look for new games in your folders")
                .clicked()
            {
                actions.push(Action::RescanLibrary);
            }
            if ui.add(egui::Button::new("Open a file…").corner_radius(8).min_size(egui::vec2(0.0, 32.0))).clicked() {
                actions.push(Action::Pick(Pick::Cartridge));
            }
            if primary_button(ui, "➕  Add games folder", egui::vec2(0.0, 32.0)).clicked() {
                actions.push(Action::Pick(Pick::RomFolder));
            }
        });
    });
    ui.add_space(10.0);
}

fn primary_button(ui: &mut egui::Ui, text: &str, min: egui::Vec2) -> egui::Response {
    let id = ui.next_auto_id();
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    let b = egui::Button::new(egui::RichText::new(text).strong().color(egui::Color32::WHITE))
        .fill(if hovered { ACCENT_HOVER } else { ACCENT })
        .stroke(egui::Stroke::NONE)
        .corner_radius(8)
        .min_size(min);
    if min.x > 0.0 { ui.add_sized(min, b) } else { ui.add(b) }
}

/// Red banner: the last game was closed because the Leapster turned itself
/// off after inactivity.
fn idle_banner(ui: &mut egui::Ui, title: &str, actions: &mut Vec<Action>) {
    const RED: egui::Color32 = egui::Color32::from_rgb(220, 70, 70);
    const RED_HOVER: egui::Color32 = egui::Color32::from_rgb(238, 92, 92);
    // A rounded panel filled with a red fade, like the library's green one.
    let bg = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(18, 14)).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal(|ui| {
            let (name, _) = split_region(title);
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(egui::RichText::new(name).size(17.0).strong().color(egui::Color32::from_rgb(255, 214, 120)));
                    ui.label(egui::RichText::new(" was closed").size(17.0).color(egui::Color32::WHITE));
                });
                ui.label(
                    egui::RichText::new("Nothing was pressed for a while, so the Leapster switched itself off.")
                        .color(egui::Color32::from_rgb(240, 200, 200)),
                );
                ui.label(
                    egui::RichText::new("You can change this in Emulation > Turn off when idle.")
                        .size(12.0)
                        .color(egui::Color32::from_rgb(200, 150, 150)),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let close = egui::Button::new(egui::RichText::new("×").size(20.0).color(egui::Color32::from_rgb(240, 200, 200)))
                    .frame(false)
                    .min_size(egui::vec2(28.0, 28.0));
                if ui.add(close).on_hover_text("Dismiss").clicked() {
                    actions.push(Action::DismissIdleBanner);
                }
                let id = ui.next_auto_id();
                let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
                let resume = egui::Button::new(egui::RichText::new("▶  Resume").size(15.0).strong().color(egui::Color32::WHITE))
                    .fill(if hovered { RED_HOVER } else { RED })
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(10)
                    .min_size(egui::vec2(120.0, 36.0));
                if ui.add(resume).on_hover_text("Back to where you were").clicked() {
                    actions.push(Action::ResumeAfterIdle);
                }
            });
        });
    });
    let rect = inner.response.rect;
    let fade = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("red fade"))).unwrap_or_else(|| {
        // 2x2 colours, bilinearly stretched: bright red top left to dark bottom right.
        let px = [
            egui::Color32::from_rgb(112, 30, 32),
            egui::Color32::from_rgb(74, 20, 24),
            egui::Color32::from_rgb(66, 18, 22),
            egui::Color32::from_rgb(28, 12, 14),
        ];
        let img = egui::ColorImage { size: [2, 2], pixels: px.to_vec(), source_size: egui::vec2(2.0, 2.0) };
        let tex = ui.ctx().load_texture("red fade", img, egui::TextureOptions::LINEAR);
        ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("red fade"), tex.clone()));
        tex
    });
    let uv = egui::Rect::from_min_max(egui::pos2(0.25, 0.25), egui::pos2(0.75, 0.75));
    ui.painter().set(
        bg,
        egui::epaint::RectShape::new(
            rect,
            14,
            egui::Color32::WHITE,
            egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(230, 90, 90, 120)),
            egui::StrokeKind::Inside,
        )
        .with_texture(fade.id(), uv),
    );
}

fn notice(ui: &mut egui::Ui, text: &str, mut on_click: impl FnMut()) {
    egui::Frame::NONE
        .fill(egui::Color32::from_rgba_unmultiplied(46, 170, 82, 28))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(46, 170, 82, 90)))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(14, 10))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(text).color(egui::Color32::from_gray(225)));
                ui.add_space(8.0);
                if primary_button(ui, "📁  Choose games folder", egui::vec2(0.0, 28.0)).clicked() {
                    on_click();
                }
            });
        });
}

fn empty_library(ui: &mut egui::Ui, settings: &Settings, actions: &mut Vec<Action>) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.16).max(12.0));
        ui.label(egui::RichText::new("🎮").size(64.0));
        ui.add_space(8.0);
        let (head, body) = if settings.rom_folders.is_empty() {
            (
                "Where are your Leapster games?",
                "Choose the folder with your cartridge dumps. Ribbit looks there and in its subfolders.",
            )
        } else {
            ("No Leapster games found yet", "Ribbit looked for cartridge dumps (.bin, .rom, .u3, .lst, or zipped) in the folders above.")
        };
        ui.label(egui::RichText::new(head).size(24.0).strong().color(egui::Color32::WHITE));
        ui.add_space(6.0);
        ui.label(egui::RichText::new(body).weak());
        ui.add_space(22.0);
        if primary_button(ui, "📁  Choose games folder", egui::vec2(280.0, 46.0)).clicked() {
            actions.push(Action::Pick(Pick::RomFolder));
        }
        ui.add_space(10.0);
        if ui.add(egui::Button::new("Open a single file…").corner_radius(8).min_size(egui::vec2(280.0, 36.0))).clicked() {
            actions.push(Action::Pick(Pick::Cartridge));
        }
        ui.add_space(18.0);
        ui.label(egui::RichText::new("No games are included: use dumps of cartridges you own.").size(12.5).weak());
    });
}

/// "Name (USA, alt)" -> ("Name", Some("USA, alt")).
fn split_region(title: &str) -> (&str, Option<&str>) {
    match title.strip_suffix(')').and_then(|t| t.rsplit_once(" (")) {
        Some((name, region)) => (name, Some(region)),
        None => (title, None),
    }
}

fn badge(ui: &mut egui::Ui, text: &str) {
    egui::Frame::NONE
        .fill(egui::Color32::from_white_alpha(14))
        .corner_radius(6)
        .inner_margin(egui::Margin::symmetric(7, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(text).size(11.5).color(egui::Color32::from_gray(200)));
        });
}

/// Rounded art with a drop shadow (or a placeholder while it is made).
fn paint_art(ui: &egui::Ui, game: &crate::library::Game, rect: egui::Rect, radius: u8) {
    let p = ui.painter();
    let shadow = egui::epaint::Shadow { offset: [0, 6], blur: 18, spread: 0, color: egui::Color32::from_black_alpha(120) };
    p.add(shadow.as_shape(rect, radius));
    match &game.art {
        Art::Texture(t) => {
            egui::Image::new((t.id(), rect.size())).corner_radius(radius).paint_at(ui, rect);
        }
        other => {
            p.rect_filled(rect, radius, egui::Color32::from_rgb(24, 30, 27));
            let note = if matches!(other, Art::Pending) { "Getting title screen…" } else { "🎮" };
            let font = egui::FontId::proportional(if matches!(other, Art::Pending) { 14.0 } else { 40.0 });
            p.text(rect.center(), egui::Align2::CENTER_CENTER, note, font, egui::Color32::from_gray(120));
        }
    }
}

/// A round green play button.
fn paint_play(p: &egui::Painter, c: egui::Pos2, r: f32, alpha: f32) {
    p.circle_filled(c, r, ACCENT.gamma_multiply(alpha));
    let s = r * 0.42;
    let tri = vec![c + egui::vec2(-s * 0.7, -s), c + egui::vec2(-s * 0.7, s), c + egui::vec2(s * 1.05, 0.0)];
    p.add(egui::Shape::convex_polygon(tri, egui::Color32::WHITE.gamma_multiply(alpha), egui::Stroke::NONE));
}

/// The last game played, big, with its details and a Play button.
fn hero(ui: &mut egui::Ui, game: &crate::library::Game, actions: &mut Vec<Action>) {
    let frame = egui::Frame::NONE
        .fill(egui::Color32::from_white_alpha(6))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(14)))
        .corner_radius(16)
        .inner_margin(20);
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.horizontal_top(|ui| {
            let side = (ui.available_width() * 0.32).clamp(180.0, 320.0);
            let (rect, art) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
            paint_art(ui, game, rect, 12);
            if art.hovered() {
                paint_play(ui.painter(), rect.center(), 40.0, 1.0);
            }
            if art.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                actions.push(Action::OpenCartridge(game.path.clone()));
            }
            ui.add_space(24.0);
            ui.vertical(|ui| {
                ui.add_space(6.0);
                ui.label(egui::RichText::new("CONTINUE PLAYING").size(13.0).strong().color(ACCENT));
                ui.add_space(6.0);
                let (name, region) = split_region(&game.title);
                ui.label(egui::RichText::new(name).size(28.0).strong().color(egui::Color32::WHITE));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if let Some(r) = region {
                        badge(ui, r);
                    }
                    ui.label(egui::RichText::new(&game.subtitle).weak());
                });
                ui.add_space(14.0);
                egui::Grid::new("hero details").num_columns(2).spacing([16.0, 5.0]).show(ui, |ui| {
                    for (k, v) in game.details.iter().filter(|(k, _)| matches!(*k, "Built" | "Catalogue" | "Dump")) {
                        ui.label(egui::RichText::new(*k).weak());
                        ui.label(egui::RichText::new(v).color(egui::Color32::from_gray(210)));
                        ui.end_row();
                    }
                });
                ui.add_space(20.0);
                if primary_button(ui, "▶   Play", egui::vec2(180.0, 46.0)).clicked() {
                    actions.push(Action::OpenCartridge(game.path.clone()));
                }
            });
        });
    });
}

/// One game: its art, title, region and a line of info. Click to play.
/// Library cards appear one after another (seconds between cards, and
/// each card's grow-in).
const CARD_STAGGER: f32 = 0.02;
const CARD_GROW: f32 = 0.28;
/// Cards after this many start together (the whole library is in place
/// within about half a second).
const CARD_STAGGER_MAX: usize = 12;

/// `appear`: 0 to 1 as the card grows in (smaller and transparent at 0).
fn game_card(ui: &mut egui::Ui, game: &crate::library::Game, w: f32, appear: f32, actions: &mut Vec<Action>) {
    let pad = 10.0;
    let inner_w = w - 2.0 * pad;
    let (name, region) = split_region(&game.title);
    // Every card the same height: two title lines, longer titles cut short.
    let title_font = egui::FontId::proportional(15.5);
    let mut job = egui::text::LayoutJob::simple(name.to_string(), title_font.clone(), egui::Color32::WHITE, inner_w);
    job.wrap.max_rows = 2;
    let title = ui.painter().layout_job(job);
    let two_lines = ui.painter().layout_no_wrap("A
A".into(), title_font, egui::Color32::WHITE).size().y;
    let sub = ui.painter().layout_no_wrap(
        game.subtitle.clone(),
        egui::FontId::proportional(12.0),
        egui::Color32::from_gray(140),
    );
    let sub_h = sub.size().y;
    let h = pad + inner_w + 12.0 + two_lines + 6.0 + sub_h.max(18.0) + pad;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let lift = ui.ctx().animate_bool_with_time(response.id, response.hovered(), 0.12);
    let opacity = ui.opacity();
    ui.multiply_opacity(appear);
    let grow = 0.86 + 0.14 * appear;
    let card = rect.translate(egui::vec2(0.0, -4.0 * lift));
    let card = egui::Rect::from_center_size(card.center(), card.size() * grow);
    let inner_w = inner_w * grow;
    let p = ui.painter();
    let shadow = egui::epaint::Shadow {
        offset: [0, (4.0 + 6.0 * lift) as i8],
        blur: (12.0 + 14.0 * lift) as u8,
        spread: 0,
        color: egui::Color32::from_black_alpha(90 + (60.0 * lift) as u8),
    };
    p.add(shadow.as_shape(card, 14));
    p.rect_filled(card, 14, egui::Color32::from_rgb(20, 23, 22).lerp_to_gamma(egui::Color32::from_rgb(28, 34, 31), lift));
    let border = egui::Color32::from_white_alpha(12).lerp_to_gamma(ACCENT, lift);
    p.rect_stroke(card, 14, egui::Stroke::new(1.0 + lift, border), egui::StrokeKind::Inside);
    let art = egui::Rect::from_min_size(card.min + egui::vec2(pad, pad), egui::vec2(inner_w, inner_w));
    paint_art(ui, game, art, 10);
    if lift > 0.0 {
        ui.painter().rect_filled(art, 10, egui::Color32::from_black_alpha((70.0 * lift) as u8));
        paint_play(ui.painter(), art.center(), 30.0, lift);
    }
    let p = ui.painter();
    let at = egui::pos2(card.min.x + pad, art.max.y + 12.0);
    p.galley(at, title, egui::Color32::WHITE);
    let mut x = at.x;
    let y = at.y + two_lines + 6.0;
    if let Some(r) = region {
        let g = p.layout_no_wrap(r.to_string(), egui::FontId::proportional(11.0), egui::Color32::from_gray(200));
        let b = egui::Rect::from_min_size(egui::pos2(x, y), g.size() + egui::vec2(12.0, 4.0));
        p.rect_filled(b, 5, egui::Color32::from_white_alpha(14));
        p.galley(b.min + egui::vec2(6.0, 2.0), g, egui::Color32::from_gray(200));
        x = b.max.x + 8.0;
    }
    p.galley(egui::pos2(x, y + 1.0), sub, egui::Color32::from_gray(140));
    ui.set_opacity(opacity);
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand).on_hover_ui(|ui| {
        ui.label(egui::RichText::new(&game.title).strong());
        egui::Grid::new("game details").num_columns(2).spacing([14.0, 3.0]).show(ui, |ui| {
            for (k, v) in &game.details {
                ui.label(egui::RichText::new(*k).weak());
                ui.label(v);
                ui.end_row();
            }
        });
        ui.label(egui::RichText::new("Click to play · right-click for more").weak());
    });
    if response.clicked() {
        actions.push(Action::OpenCartridge(game.path.clone()));
    }
    response.context_menu(|ui| {
        if ui.button("▶ Play").clicked() {
            actions.push(Action::OpenCartridge(game.path.clone()));
            ui.close();
        }
        if ui.button("Get title screen again").clicked() {
            actions.push(Action::RemakeArt(game.path.clone()));
            ui.close();
        }
    });
}

fn dialogs(ctx: &egui::Context, st: &mut UiState, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>) {
    let mut open = st.show_bindings;
    egui::Window::new("Key bindings").open(&mut open).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.label("Click a key, then press the new key on your keyboard.");
        egui::Grid::new("bindings").num_columns(2).spacing([24.0, 6.0]).show(ui, |ui| {
            for b in BINDABLE {
                ui.label(format!("{b:?}"));
                let label = if st.capturing == Some(b) {
                    "press a key…".to_string()
                } else {
                    settings.key_for(b).map_or("—".to_string(), settings::key_label)
                };
                if ui.add(egui::Button::new(label).min_size([110.0, 0.0].into())).clicked() {
                    st.capturing = Some(b);
                }
                ui.end_row();
            }
        });
        ui.separator();
        if ui.button("Restore defaults").clicked() {
            settings.keys = settings::default_keys();
            st.capturing = None;
            actions.push(Action::SettingsChanged);
        }
    });
    st.show_bindings = open;
    if !open {
        st.capturing = None;
    }

    egui::Window::new("Controls").open(&mut st.show_controls).collapsible(false).resizable(false).show(ctx, |ui| {
        egui::Grid::new("controls").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
            for b in BINDABLE {
                if let Some(k) = settings.key_for(b) {
                    ui.label(format!("{b:?}"));
                    ui.label(settings::key_label(k));
                    ui.end_row();
                }
            }
            for (what, key) in [
                ("Stylus", "Left mouse button / touch"),
                ("Pause", "Esc"),
                ("Fast-forward", "Hold Tab"),
                ("Rewind", "Hold ` (left of 1)"),
                ("Quick save / load", "F5 / F9"),
                ("Reset", "Ctrl+R"),
                ("Open cartridge", "Ctrl+O"),
                ("Fullscreen", "F11"),
                ("Screenshot", "F12"),
                ("Show/hide menu", "F10"),
            ] {
                ui.label(what);
                ui.label(key);
                ui.end_row();
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("Controller").strong());
        egui::Grid::new("pad").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
            for (what, button) in [
                ("Arrows", "D-pad / left stick"),
                ("A / B", "A / B"),
                ("Hint", "X or Y"),
                ("Pause", "Start"),
                ("Home", "Back / Select"),
                ("Volume - / +", "LB / RB"),
                ("Menu", "Guide"),
            ] {
                ui.label(what);
                ui.label(button);
                ui.end_row();
            }
        });
    });

    egui::Window::new("About Ribbit").open(&mut st.show_about).collapsible(false).resizable(false).show(ctx, |ui| {
        ui.heading("🐸 Ribbit");
        ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
        ui.label("An accuracy-first emulator for the original 2003 LeapFrog Leapster.");
        ui.add_space(6.0);
        ui.label("Ribbit includes no firmware or games. Use dumps of hardware you own.");
        ui.label("Parts of the audio and touchscreen behaviour follow MAME's research (BSD-3-Clause).");
        ui.label("AI upscaling: ArtCNN (Joao Chrisostomo, Kacper Michajlow), Anime4K (bloc97), ScaleFX (Sp00kyFox); all MIT.");
        ui.add_space(6.0);
        ui.label(egui::RichText::new(format!("Saves: {}", info.save_dir)).weak());
    });

    egui::Window::new("Emulation status").open(&mut st.show_status).collapsible(false).resizable(true).show(
        ctx,
        |ui| {
            ui.label("What works and how sure we are:");
            egui::Grid::new("status").num_columns(2).spacing([16.0, 4.0]).striped(true).show(ui, |ui| {
                for (part, state) in [
                    ("CPU (ARCompact A5)", "Verified against GNU binutils; timing is a placeholder"),
                    ("Display, framebuffer DMA", "From BIOS behaviour and MAME"),
                    ("Buttons", "MAME's layout"),
                    ("Touchscreen", "EXPERIMENTAL (MAME's guessed model)"),
                    ("Sound voices + speech", "MAME's model, unverified on hardware"),
                    ("Saves (EEPROMs)", "From BIOS behaviour; sizes unverified"),
                    ("US BIOS v2.1", "Boots; SpongeBob reaches gameplay"),
                    ("Universal BIOS v1.5", "Powers itself off after ~4 s (unknown input line)"),
                ] {
                    ui.label(part);
                    ui.label(state);
                    ui.end_row();
                }
            });
        },
    );

    if let Some(err) = st.error.clone() {
        let mut close = false;
        egui::Window::new("Problem")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.label(err);
                if ui.button("OK").clicked() {
                    close = true;
                }
            });
        if close {
            st.error = None;
        }
    }
}

fn toasts(ctx: &egui::Context, st: &mut UiState) {
    st.toasts.retain(|(_, t)| t.elapsed() < TOAST_TIME);
    if st.toasts.is_empty() {
        return;
    }
    egui::Area::new("toasts".into()).anchor(egui::Align2::RIGHT_TOP, [-12.0, 40.0]).interactable(false).show(
        ctx,
        |ui| {
            for (text, _) in &st.toasts {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.label(text);
                });
            }
        },
    );
    ctx.request_repaint();
}
