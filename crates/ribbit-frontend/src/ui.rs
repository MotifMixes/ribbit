//! Menus, dialogs and overlays (egui). The UI only reads state and emits
//! `Action`s; `app.rs` performs them.

use crate::files::{Browser, Pick};
use crate::gpu::Display;
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
    SaveState(u8),
    LoadState(u8),
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
    let mut area = egui::Rect::NOTHING;
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        area = ui.max_rect();
        if !info.running {
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
            if ui.add_enabled(info.running, egui::Button::new("Close Cartridge")).clicked() {
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
                for s in [2, 3, 4, 16] {
                    if ui.radio(settings.window_scale == s, format!("{s}x ({}x{})", 160 * s, 160 * s)).clicked() {
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
