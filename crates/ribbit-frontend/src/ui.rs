//! Menus, dialogs and overlays (egui). The UI only reads state and emits
//! `Action`s; `app.rs` performs them.

use crate::files::{Browser, Pick};
use crate::gpu::Display;
use crate::library::{Art, Family, Library};
use crate::settings::{self, BINDABLE, Settings};
use ribbit_input::Button;
use ribbit_memory::known::Hardware;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Pick(Pick),
    OpenCartridge(PathBuf),
    /// A game chosen in the library (on the console chosen in its menu).
    PlayFromLibrary(PathBuf),
    /// The console the library plays Leapster games on.
    SetLibraryConsole(ribbit_memory::known::Hardware),
    /// Start a console's own system (its BIOS), no cartridge needed.
    StartSystem(ribbit_memory::known::Hardware),
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
    /// The library's BIOS picker: play this console on this BIOS.
    PickBios(ribbit_memory::known::Hardware, PathBuf),
    /// Ask before deleting a game's save data (path, title).
    AskDeleteSave(PathBuf, String),
    /// Delete a Leapster game's save data (moved into the backups folder).
    DeleteSaveData(PathBuf),
    /// Remove the saved players from this Zippity dump file.
    CleanZippityDump(PathBuf),
    /// Keep them (and don't ask again).
    KeepZippitySaves(PathBuf),
    /// Open the RockIt Twist research preview (no package).
    OpenTwist,
    /// Use this folder as the RockIt Twist's storage content (None: none).
    SetTwistContent(Option<PathBuf>),
    SetTwistUsb(bool),
    /// Unpack this zip of RockIt Twist packages and use it as the content.
    ImportTwistZip(PathBuf),
    /// The console view was turned on or off.
    ConsoleViewChanged,
    /// The update is installed: save, start the new version and quit.
    RestartForUpdate,
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
    /// Paused because the window is in the background (not by the user).
    pub background: bool,
    /// Seconds until the console turns itself off for inactivity, in its
    /// last minute.
    pub idle_left: Option<u32>,
    /// A Zippity is running, and whether the player stands on its home spot.
    pub zippity: bool,
    pub zippity_on_home: bool,
    /// The running console has a console view (None: nothing running).
    pub console_view_available: Option<bool>,
}

/// Something that went wrong, as the error card shows it.
#[derive(Clone)]
pub struct Problem {
    pub title: String,
    pub explain: String,
    /// The technical message (empty: no "Details").
    pub details: String,
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
    /// A RockIt Twist action waiting for its new key.
    pub capturing_twist: Option<crate::twist::TwistKey>,
    pub browser: Browser,
    toasts: Vec<(String, Instant)>,
    pub error: Option<Problem>,
    /// A Zippity dump holding a previous owner's players: asks whether to
    /// remove them from the file.
    pub zippity_clean: Option<PathBuf>,
    /// A game whose save data the player asked to delete (path, title):
    /// asks to confirm.
    delete_save: Option<(PathBuf, String)>,
    /// The library's page and search.
    pub library: LibraryView,
    /// The game (title) that was closed because the console (name) turned
    /// itself off; shown above the library until dismissed.
    pub idle_closed: Option<(String, &'static str)>,
    /// When the library last appeared (its cards grow in from then).
    library_since: Option<Instant>,
    was_running: bool,
    /// The update check and the releases it found.
    pub updates: crate::updates::Updates,
    /// The updates and patch notes window.
    pub show_updates: bool,
    /// The release (tag) the patch notes window opens on.
    notes_open: Option<String>,
    /// A newer release was announced this session (it pops up once).
    update_announced: bool,
    /// The installed update's restart has been asked for.
    restarting: bool,
    /// Android: the side menu is open.
    drawer: bool,
    /// Android: asking whether to save a state before exiting the game.
    confirm_back: bool,
}

/// What the library shows.
#[derive(Default)]
pub struct LibraryView {
    /// The page: None for all games, else the games of one family's console.
    pub tab: Option<Family>,
    /// Search text.
    pub search: String,
    /// Android: how far the list is pulled down at its top (points), to
    /// refresh it.
    pub pull: f32,
    /// The finger touched down with the list at its top (only then does a
    /// pull count).
    pub pull_armed: bool,
}

const TOAST_TIME: Duration = Duration::from_millis(4000);

impl UiState {
    pub fn new() -> Self {
        Self { menu_visible: true, ..Default::default() }
    }

    /// Shows an error card: a short title, what it means in plain words,
    /// and the technical message folded away under "Details".
    pub fn problem(&mut self, title: impl Into<String>, explain: impl Into<String>, details: impl Into<String>) {
        self.error = Some(Problem { title: title.into(), explain: explain.into(), details: details.into() });
    }

    /// Asks whether to delete `title`'s save data (the game at `path`).
    pub fn ask_delete_save(&mut self, path: PathBuf, title: String) {
        self.delete_save = Some((path, title));
    }

    /// Android: whether the side menu is open.
    pub fn side_menu_open(&self) -> bool {
        self.drawer
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
    if settings.check_for_updates && !st.updates.checked() {
        st.updates.check(&ctx);
    }
    // A newer release found at start-up pops up once (not for a skipped one).
    if !st.update_announced
        && let Some(r) = st.updates.newer()
    {
        st.update_announced = true;
        if r.tag != settings.skipped_update {
            st.show_updates = true;
            st.notes_open = None;
        }
    }
    // A phone has no bars: the side menu (swiped in from the left) has it all.
    let phone = cfg!(target_os = "android");
    if st.menu_visible && !info.fullscreen && !phone {
        egui::Panel::top("menu").show(ui, |ui| menu_bar(ui, st, settings, info, actions));
    }
    if settings.show_status_bar && !info.fullscreen && !phone {
        // The newest message shows in the status bar (nothing to cover).
        st.toasts.retain(|(_, t)| t.elapsed() < TOAST_TIME);
        let message = st.toasts.last().map(|(m, _)| m.clone());
        egui::Panel::bottom("status").show(ui, |ui| status_bar(ui, info, message.as_deref()));
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
            library_screen(ui, settings, library, &mut st.library, st.idle_closed.as_ref().map(|(t, c)| (t.as_str(), *c)), since, actions);
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
            let text = if info.background { "⏸ Paused while in the background" } else { "⏸ Paused" };
            p.text(area.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(28.0), egui::Color32::WHITE);
        } else if let Some(left) = info.idle_left {
            idle_countdown(ui, area, left);
        }
    });
    dialogs(&ctx, st, settings, info, actions);
    if let Some((pick, path)) = st.browser.ui(&ctx) {
        actions.push(match pick {
            Pick::Bios => Action::SetBios(path),
            Pick::Cartridge => Action::OpenCartridge(path),
            Pick::RomFolder => Action::AddRomFolder(path),
            Pick::TwistContent => Action::SetTwistContent(Some(path)),
            Pick::TwistZip => Action::ImportTwistZip(path),
        });
    }
    if phone {
        side_menu(&ctx, st, settings, info, actions);
    }
    // First start (an update from an older version too): the AI note as a
    // pop-up, read once. Android's quick start shows it as its first page.
    let quick_start = phone && !info.running && info.bios_name.is_none();
    if !settings.ai_notice_seen && !quick_start {
        egui::Modal::new(egui::Id::new("ai-notice")).frame(ai_card_frame()).show(&ctx, |ui| {
            ui.set_width(ctx.content_rect().width().min(560.0) - 80.0);
            ai_notice_text(ui);
            ui.add_space(16.0);
            ui.vertical_centered(|ui| {
                let text = egui::RichText::new("Continue").size(17.0).color(egui::Color32::WHITE);
                let button = egui::Button::new(text).fill(ACCENT).corner_radius(10).min_size(egui::vec2(220.0, 44.0));
                if ui.add(button).clicked() {
                    settings.ai_notice_seen = true;
                    actions.push(Action::SettingsChanged);
                }
            });
        });
    }
    // Over the screen only when there is no status bar to show them in.
    if !settings.show_status_bar || info.fullscreen || phone {
        toasts(&ctx, st);
    }
    area
}

/// Android: a swipe starting this close to the left edge (points) opens the
/// side menu; touches there are the menu's, not the game's.
pub const SIDE_MENU_EDGE: f32 = 48.0;

/// Android's side menu, swiped in from the left edge (or opened with Back),
/// laid out like other emulators' on a phone: a header, the game's quick
/// controls as big rows, then every setting.
fn side_menu(ctx: &egui::Context, st: &mut UiState, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>) {
    const SWIPE: f32 = 60.0;
    let screen = ctx.content_rect();
    let width = 280.0f32.min(screen.width() * 0.6);
    let (swipe, back) = ctx.input(|i| {
        let swipe = i.pointer.press_origin().zip(i.pointer.interact_pos()).map(|(o, p)| (o, p.x - o.x));
        (swipe, i.key_pressed(egui::Key::BrowserBack))
    });
    match swipe {
        Some((origin, dx)) if !st.drawer && origin.x <= SIDE_MENU_EDGE && dx > SWIPE => st.drawer = true,
        Some((origin, dx)) if st.drawer && origin.x <= width && dx < -SWIPE => st.drawer = false,
        _ => {}
    }
    if back {
        st.drawer = !st.drawer;
    }
    // Slides in and out.
    let shown = ctx.animate_bool_with_time(egui::Id::new("side-menu-open"), st.drawer, 0.16);
    if shown <= 0.0 {
        return;
    }
    // The rest dimmed; a tap there closes the menu.
    egui::Area::new("side-menu-shade".into()).order(egui::Order::Middle).fixed_pos(screen.min).show(ctx, |ui| {
        let resp = ui.allocate_rect(screen, egui::Sense::click());
        ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha((160.0 * shown) as u8));
        if resp.clicked() {
            st.drawer = false;
        }
    });
    let before = actions.len();
    let at = screen.min + egui::vec2(-width * (1.0 - shown), 0.0);
    egui::Area::new("side-menu".into()).order(egui::Order::Foreground).fixed_pos(at).interactable(st.drawer).show(ctx, |ui| {
        let panel = egui::Rect::from_min_size(at, egui::vec2(width, screen.height()));
        ui.painter().rect_filled(panel, 0.0, egui::Color32::from_gray(22));
        ui.painter().rect_filled(
            egui::Rect::from_min_max(panel.right_top(), panel.right_bottom() + egui::vec2(1.0, 0.0)),
            0.0,
            egui::Color32::from_white_alpha(20),
        );
        ui.set_width(width);
        ui.set_height(screen.height());
        drawer_header(ui, info, width);
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                side_menu_items(ui, st, settings, info, actions);
            });
        });
    });
    if st.confirm_back {
        back_to_games_question(ctx, st, info, actions);
    }
    // Something chosen: back to the game.
    if actions.len() > before {
        st.drawer = false;
    }
}

/// Before exiting the game (back to the games): save a state first?
fn back_to_games_question(ctx: &egui::Context, st: &mut UiState, info: &Info, actions: &mut Vec<Action>) {
    let modal = egui::Modal::new(egui::Id::new("back-to-games")).frame(ai_card_frame()).show(ctx, |ui| {
        ui.set_width(ctx.content_rect().width().min(440.0) - 64.0);
        ui.label(egui::RichText::new("Exit the game?").size(20.0).strong().color(egui::Color32::WHITE));
        ui.add_space(6.0);
        let note = match info.slots.first().cloned().flatten() {
            Some(when) => format!("Save a state first to carry on from here later (it replaces the one from {when})."),
            None => "Save a state first to carry on from here later.".to_string(),
        };
        ui.add(egui::Label::new(egui::RichText::new(note).size(15.0).color(egui::Color32::from_gray(205))).wrap());
        ui.add_space(14.0);
        let full = ui.available_width();
        let button = |text: &str, fill: Option<egui::Color32>| {
            let text = egui::RichText::new(text).size(16.0).color(egui::Color32::WHITE);
            let b = egui::Button::new(text).corner_radius(10).min_size(egui::vec2(full, 44.0));
            match fill {
                Some(f) => b.fill(f),
                None => b,
            }
        };
        if ui.add(button("💾  Save and exit", Some(ACCENT))).clicked() {
            actions.push(Action::SaveState(1));
            actions.push(Action::CloseCartridge);
            st.confirm_back = false;
        }
        if ui.add(button("Exit without saving", None)).clicked() {
            actions.push(Action::CloseCartridge);
            st.confirm_back = false;
        }
        if ui.add(button("Cancel", None).frame(false)).clicked() {
            st.confirm_back = false;
        }
    });
    if modal.should_close() {
        st.confirm_back = false;
    }
}

/// The side menu's header: Ribbit and what is running, on the frog's green.
fn drawer_header(ui: &mut egui::Ui, info: &Info, width: f32) {
    // The game's name under Ribbit, two lines at most; the header grows to
    // fit it (a long name used to run into its bottom edge).
    let sub = if info.running { info.title.clone() } else { "No game running".to_string() };
    let mut job = egui::text::LayoutJob::simple(sub, egui::FontId::proportional(14.0), egui::Color32::from_gray(225), width - 36.0);
    job.wrap.max_rows = 2;
    let sub = ui.painter().layout_job(job);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 52.0 + sub.size().y + 14.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 0.0, egui::Color32::from_rgb(30, 120, 58));
    let left = rect.left() + 18.0;
    p.text(egui::pos2(left, rect.top() + 30.0), egui::Align2::LEFT_CENTER, "🐸 Ribbit", egui::FontId::proportional(24.0), egui::Color32::WHITE);
    p.galley(egui::pos2(left, rect.top() + 52.0), sub, egui::Color32::from_gray(225));
}

/// A section's small heading.
fn drawer_section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(10.0);
    ui.label(egui::RichText::new(title.to_uppercase()).size(12.5).color(egui::Color32::from_gray(140)).strong());
    ui.add_space(2.0);
}

/// One of the side menu's rows: an icon, a label and, on the right, a value
/// or a switch (`Some(on)`).
fn drawer_row(ui: &mut egui::Ui, icon: &str, label: &str, value: &str, switch: Option<bool>, enabled: bool) -> bool {
    let sense = if enabled { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 50.0), sense);
    let p = ui.painter();
    if enabled && (resp.is_pointer_button_down_on() || resp.hovered()) {
        p.rect_filled(rect, 10.0, egui::Color32::from_white_alpha(16));
    }
    let fg = if enabled { egui::Color32::from_gray(232) } else { egui::Color32::from_gray(105) };
    let y = rect.center().y;
    p.text(egui::pos2(rect.left() + 26.0, y), egui::Align2::CENTER_CENTER, icon, egui::FontId::proportional(20.0), fg);
    p.text(egui::pos2(rect.left() + 52.0, y), egui::Align2::LEFT_CENTER, label, egui::FontId::proportional(17.0), fg);
    let right = rect.right() - 12.0;
    if let Some(on) = switch {
        let track = egui::Rect::from_center_size(egui::pos2(right - 22.0, y), egui::vec2(44.0, 24.0));
        p.rect_filled(track, 12.0, if on { ACCENT } else { egui::Color32::from_gray(70) });
        let knob = if on { track.right() - 12.0 } else { track.left() + 12.0 };
        p.circle_filled(egui::pos2(knob, y), 9.0, egui::Color32::WHITE);
    } else if !value.is_empty() {
        p.text(egui::pos2(right, y), egui::Align2::RIGHT_CENTER, value, egui::FontId::proportional(15.0), egui::Color32::from_gray(150));
    }
    resp.clicked()
}

fn side_menu_items(ui: &mut egui::Ui, st: &mut UiState, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>) {
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 2.0);
    if info.running {
        drawer_section(ui, "Game");
        // (Asks about a save state first: the menu stays open meanwhile.)
        if drawer_row(ui, "⏏", "Exit game", "", None, true) {
            st.confirm_back = true;
        }
        let (icon, label) = if info.paused { ("▶", "Resume") } else { ("⏸", "Pause") };
        if drawer_row(ui, icon, label, "", None, true) {
            actions.push(Action::TogglePause);
        }
        let saved = info.slots.first().cloned().flatten();
        if drawer_row(ui, "💾", "Save state", "", None, true) {
            actions.push(Action::SaveState(1));
        }
        if drawer_row(ui, "📂", "Load state", saved.as_deref().unwrap_or("empty"), None, saved.is_some()) {
            actions.push(Action::LoadState(1));
        }
        if drawer_row(ui, "⟲", "Reset", "", None, true) {
            actions.push(Action::Reset);
        }
        if drawer_row(ui, "📷", "Screenshot", "", None, true) {
            actions.push(Action::Screenshot);
        }
        drawer_section(ui, &format!("Speed · {:.0}% now", info.speed));
        speed_picker(ui, settings, actions);
    } else {
        drawer_section(ui, "Games");
        if drawer_row(ui, "🎮", "Open Cartridge…", "", None, info.bios_name.is_some()) {
            actions.push(Action::Pick(Pick::Cartridge));
        }
        if drawer_row(ui, "📁", "Add Games Folder…", "", None, true) {
            actions.push(Action::Pick(Pick::RomFolder));
        }
        if drawer_row(ui, "💾", "Open BIOS…", "", None, true) {
            actions.push(Action::Pick(Pick::Bios));
        }
    }
    drawer_section(ui, "Controls");
    if drawer_row(ui, "☝", "Touch controls", "", Some(settings.touch_controls), true) {
        settings.touch_controls = !settings.touch_controls;
        actions.push(Action::SettingsChanged);
    }
    drawer_section(ui, "All settings");
    ui.spacing_mut().item_spacing = egui::vec2(6.0, 0.0);
    ui.vertical(|ui| menus(ui, st, settings, info, actions, true));
    ui.add_space(14.0);
}

/// Speed as a segmented control: normal, double, four times, unlimited.
fn speed_picker(ui: &mut egui::Ui, settings: &mut Settings, actions: &mut Vec<Action>) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 46.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 12.0, egui::Color32::from_gray(38));
    let choices = [(100, "1×"), (200, "2×"), (400, "4×"), (0, "Max")];
    let w = rect.width() / choices.len() as f32;
    for (i, (pct, label)) in choices.into_iter().enumerate() {
        let seg = egui::Rect::from_min_size(rect.min + egui::vec2(w * i as f32, 0.0), egui::vec2(w, rect.height())).shrink(4.0);
        let resp = ui.interact(seg, ui.id().with(("speed", pct)), egui::Sense::click());
        let on = settings.speed_percent == pct;
        if on {
            ui.painter().rect_filled(seg, 9.0, ACCENT);
        } else if resp.is_pointer_button_down_on() {
            ui.painter().rect_filled(seg, 9.0, egui::Color32::from_white_alpha(18));
        }
        let fg = if on { egui::Color32::WHITE } else { egui::Color32::from_gray(200) };
        ui.painter().text(seg.center(), egui::Align2::CENTER_CENTER, label, egui::FontId::proportional(17.0), fg);
        if resp.clicked() && !on {
            settings.speed_percent = pct;
            actions.push(Action::SettingsChanged);
        }
    }
}

/// The last minute before an idle power-off: a small notice at the top.
fn idle_countdown(ui: &egui::Ui, area: egui::Rect, left: u32) {
    let p = ui.painter();
    let title = format!("Inactive: turning off in {}:{:02}", left / 60, left % 60);
    let font = egui::FontId::proportional(17.0);
    let small = egui::FontId::proportional(13.0);
    let g1 = p.layout_no_wrap(title, font, egui::Color32::WHITE);
    let g2 = p.layout_no_wrap("Press any button to keep playing".into(), small, egui::Color32::from_gray(215));
    let w = g1.size().x.max(g2.size().x) + 28.0;
    let h = g1.size().y + g2.size().y + 18.0;
    let rect = egui::Rect::from_min_size(egui::pos2(area.center().x - w / 2.0, area.top() + 12.0), egui::vec2(w, h));
    p.rect_filled(rect, 10.0, egui::Color32::from_rgba_unmultiplied(150, 24, 24, 225));
    p.galley(egui::pos2(rect.center().x - g1.size().x / 2.0, rect.top() + 7.0), g1, egui::Color32::WHITE);
    let y = rect.bottom() - 7.0 - g2.size().y;
    p.galley(egui::pos2(rect.center().x - g2.size().x / 2.0, y), g2, egui::Color32::WHITE);
}

/// The library's pages: all games, then each family that has games (all
/// the Leapster models share one page; the Leapster 2 has its own when there
/// are games for it alone). The console is chosen in the header's menu.
fn console_tabs(ui: &mut egui::Ui, settings: &Settings, library: &Library, tab: &mut Option<Family>, actions: &mut Vec<Action>) {
    let count = |f: Option<Family>| library.games.iter().filter(|g| f.is_none_or(|f| f.lists(g.family))).count();
    // A page for each family that has games of its own.
    let families: Vec<Family> =
        Family::ALL.into_iter().filter(|&f| library.games.iter().any(|g| g.family == f)).collect();
    if tab.is_some_and(|t| !families.contains(&t)) {
        *tab = None;
    }
    // Tabs: a label each over a shared rule; the open one in the accent
    // colour with a bar under it.
    let top = ui.cursor().min.y;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(26.0, 6.0);
        for choice in std::iter::once(None).chain(families.into_iter().map(Some)) {
            let selected = *tab == choice;
            let name = choice.map_or("All games", Family::name);
            let font = egui::FontId::proportional(16.0);
            let name_g = ui.painter().layout_no_wrap(name.to_string(), font.clone(), egui::Color32::WHITE);
            let count_g =
                ui.painter().layout_no_wrap(count(choice).to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
            let size = egui::vec2(name_g.size().x + 6.0 + count_g.size().x, 34.0);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let fg = if selected {
                ACCENT
            } else if resp.hovered() {
                egui::Color32::from_gray(240)
            } else {
                egui::Color32::from_gray(165)
            };
            let p = ui.painter();
            let y = rect.top() + 4.0;
            p.galley(egui::pos2(rect.left(), y), name_g.clone(), fg);
            let count_pos = egui::pos2(rect.left() + name_g.size().x + 6.0, y + name_g.size().y - count_g.size().y - 1.0);
            p.galley(count_pos, count_g, fg.gamma_multiply(0.7));
            if selected {
                let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 3.0), rect.right_bottom());
                p.rect_filled(bar, 2.0, ACCENT);
            }
            if resp.clicked() {
                *tab = choice;
                // A page whose games the chosen console can't play switches
                // to the page's console.
                let console = library.selected_console(&settings.library_console);
                if let Some(f) = choice
                    && !f.runs_on(console)
                    && library.consoles().contains(&f.console())
                {
                    actions.push(Action::SetLibraryConsole(f.console()));
                }
            }
        }
    });
    let bottom = ui.min_rect().bottom().max(top);
    let rule = egui::Rect::from_min_max(egui::pos2(ui.max_rect().left(), bottom), egui::pos2(ui.max_rect().right(), bottom + 1.0));
    ui.painter().rect_filled(rule, 0.0, egui::Color32::from_white_alpha(28));
}

fn item(ui: &mut egui::Ui, label: &str, shortcut: &str) -> bool {
    let clicked = ui.add(egui::Button::new(label).shortcut_text(shortcut)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

fn menu_bar(ui: &mut egui::Ui, st: &mut UiState, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>) {
    egui::MenuBar::new().ui(ui, |ui| menus(ui, st, settings, info, actions, false));
}

/// The menus, in the menu bar or (Android) down the side menu.
fn menus(ui: &mut egui::Ui, st: &mut UiState, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>, rows: bool) {
    top_menu(ui, rows, "📂", "File", |ui| {
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
        if ui
            .add(egui::Button::new("RockIt Twist (early emulation)"))
            .on_hover_text(ribbit_twist::STATUS)
            .clicked()
        {
            actions.push(Action::OpenTwist);
            ui.close();
        }
        if ui
            .button("Import RockIt Twist games (.zip)…")
            .on_hover_text(
                "Unpacks a zip of RockIt Twist package folders (each with its meta.inf) for one region and \
                 installs them on the emulated console, where its own firmware finds them.",
            )
            .clicked()
        {
            actions.push(Action::Pick(Pick::TwistZip));
            ui.close();
        }
        let content = settings.twist_content.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
        if ui
            .button(match &content {
                Some(n) => format!("RockIt Twist content: {n}…"),
                None => "RockIt Twist content folder…".to_string(),
            })
            .on_hover_text(
                "A folder written onto the RockIt Twist's internal storage when it starts, laid out like the \
                 console's F: drive (Applications, Music, Trailers...). For your own programs, or content \
                 copied from your own console.",
            )
            .clicked()
        {
            actions.push(Action::Pick(Pick::TwistContent));
            ui.close();
        }
        if content.is_some() && ui.button("Remove RockIt Twist content").clicked() {
            actions.push(Action::SetTwistContent(None));
            ui.close();
        }
        let usb = settings.twist_usb;
        if ui
            .button(format!("{}Plug the RockIt Twist into this PC (USB)", if usb { "✓ " } else { "" }))
            .on_hover_text(
                "Offers the emulated RockIt Twist as a USB device to this PC through USB/IP (port 3240), \
                 so LeapFrog Connect can talk to it. Needs usbip-win2 (winget install usbip-win2); then: \
                 usbip attach -r 127.0.0.1 -b 1-1. Restarts a running RockIt Twist.",
            )
            .clicked()
        {
            actions.push(Action::SetTwistUsb(!usb));
            ui.close();
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
    top_menu(ui, rows, "🎮", "Emulation", |ui| {
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
        ui.add_enabled_ui(settings.turn_off_when_idle, |ui| {
            if ui
                .checkbox(&mut settings.flash_before_idle_off, "    Flash taskbar before turning off")
                .on_hover_text("Flash Ribbit's taskbar button in the last minute. On Windows this also shows an auto-hidden taskbar until you click Ribbit.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
        });
        if ui
            .checkbox(&mut settings.pause_in_background, "Pause in background")
            .on_hover_text("Pause the game while another window is active.")
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
            .checkbox(&mut settings.leapster2_skip_menu, "Leapster 2: start games directly")
            .on_hover_text("Skip the Leapster 2's start-up screens and main menu and go straight into the cartridge. Off: games start from the cartridge icon in the menu, as on a real Leapster 2. Applies at the next start.")
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
    top_menu(ui, rows, "🖥", "Video", |ui| {
        if ui
            .checkbox(&mut settings.hd_vectors, "HD vectors (experimental)")
            .on_hover_text("Redraws the game's Flash art at your window's full resolution from the original vector data: shapes from the movies' own coordinates, text from the fonts' own outlines, bitmaps from their own pixels. Art with no vector original shows its pixels (see HD pixel art); never AI-upscaled. Replaces the scaling options below while on. Original Leapster and L-MAX BIOSes, and cartridges with their own Flash player.")
            .changed()
        {
            actions.push(Action::HdChanged);
        }
        // Art with no vector original: its pixels, or traced.
        ui.add_enabled_ui(settings.hd_vectors, |ui| {
            ui.menu_button("HD pixel art", |ui| {
                let before = settings.hd_trace_pixel_art;
                ui.radio_value(&mut settings.hd_trace_pixel_art, false, "Pixels (original)")
                    .on_hover_text("Bitmaps in the games' Flash movies, and games drawn by their own code, show their own pixels, sharp");
                ui.radio_value(&mut settings.hd_trace_pixel_art, true, "Traced into vectors")
                    .on_hover_text("The same pixels traced into smooth shapes by a fixed algorithm (depixelizing, no AI): curves where the pixels step, corners where the art has corners");
                if settings.hd_trace_pixel_art != before {
                    actions.push(Action::SettingsChanged);
                }
            });
        });
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
        ui.separator();
        ui.label(egui::RichText::new("Console view (F7: the running console's)").strong());
        let tip = "Shows the screen inside a drawing of the handheld, with its controls around it. Click, drag or scroll the controls to use them.";
        if ui.checkbox(&mut settings.console_view_twist, "RockIt Twist").on_hover_text(tip).changed() {
            actions.push(Action::ConsoleViewChanged);
        }
        let leapster_tip = match info.console_view_available {
            Some(false) => "TV consoles (Leapster TV, Zippity) have no handheld to draw: they show the plain screen.",
            _ => "Leapster, L-MAX and Leapster 2 (one drawing for all three, after the original Leapster).",
        };
        if ui.checkbox(&mut settings.console_view, "Leapster handhelds").on_hover_text(leapster_tip).changed() {
            actions.push(Action::ConsoleViewChanged);
        }
        ui.add_enabled_ui(settings.console_view || settings.console_view_twist, |ui| {
            if ui
                .checkbox(&mut settings.console_view_keys, "Show keys on the console")
                .on_hover_text("Each control shows its keyboard key; the key lights up while pressed.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
        });
    });
    top_menu(ui, rows, "🔊", "Audio", |ui| {
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
        ui.separator();
        ui.label(egui::RichText::new("Leapster 2 PCM sound").strong());
        for r in crate::audio_out::PcmRate::ALL {
            if ui.radio_value(&mut settings.pcm_rate, r, r.label()).clicked() {
                actions.push(Action::SettingsChanged);
            }
        }
        ui.label(egui::RichText::new("Games that stream sound through the Leapster 2's 32 kHz DAC.").weak());
        ui.add_enabled_ui(settings.pcm_rate == crate::audio_out::PcmRate::Original32k, |ui| {
            for c in crate::audio_out::PcmClean::ALL {
                if ui.radio_value(&mut settings.pcm_clean, c, c.label()).clicked() {
                    actions.push(Action::SettingsChanged);
                }
            }
        });
        ui.label(
            egui::RichText::new("Clean audio filters the 32 kHz sound above 8.5 kHz; unfiltered plays it exactly as emulated.")
                .weak(),
        );
    });
    top_menu(ui, rows, "⌨", "Input", |ui| {
        if item(ui, "Key bindings…", "") {
            st.show_bindings = true;
        }
        if ui
            .checkbox(&mut settings.touch_controls, "Touch controls")
            .on_hover_text("On-screen buttons, shown when the screen is touched; a key or controller button hides them.")
            .changed()
        {
            actions.push(Action::SettingsChanged);
        }
        ui.separator();
        if ui
            .checkbox(&mut settings.zippity_on_home, "Zippity: stand on the home spot (Shift)")
            .on_hover_text("Both footprints of the mat's orange home spot stay pressed. Mickey Says and other games wait for the player to stand there.")
            .changed()
        {
            actions.push(Action::SettingsChanged);
        }
    });
    top_menu(ui, rows, "❓", "Help", |ui| {
        if item(ui, "Controls", "F1") {
            st.show_controls = true;
        }
        if item(ui, "Emulation status", "") {
            st.show_status = true;
        }
        ui.separator();
        if item(ui, "Check for updates…", "") {
            st.updates.check(ui.ctx());
            st.show_updates = true;
            st.notes_open = None;
        }
        ui.menu_button("Patch notes", |ui| patch_notes_menu(ui, st));
        ui.separator();
        if item(ui, "About Ribbit", "") {
            st.show_about = true;
        }
    });
    // Right after the menus (never over them, however narrow the window).
    if let Some(r) = st.updates.newer() {
        ui.add_space(8.0);
        // The full label when it fits, else just the version.
        let full = format!("⬆ Update to {}", r.version_text());
        let font = egui::TextStyle::Button.resolve(ui.style());
        let need = ui.painter().layout_no_wrap(full.clone(), font, egui::Color32::WHITE).size().x
            + 2.0 * ui.spacing().button_padding.x;
        let label = if ui.available_width() >= need { full } else { format!("⬆ {}", r.version_text()) };
        let text = egui::RichText::new(label).color(egui::Color32::WHITE);
        if ui.add(egui::Button::new(text).fill(ACCENT)).on_hover_text("See what's new and download it").clicked() {
            st.show_updates = true;
            st.notes_open = Some(r.tag);
        }
    }
}

/// A top-level menu: a menu-bar button, or (the side menu) a full-width row
/// whose menu opens beside it.
fn top_menu(ui: &mut egui::Ui, rows: bool, icon: &str, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    if !rows {
        let response = ui.menu_button(label, add_contents).response;
        // With another of these menus open, pointing at this one opens it
        // instead, as menu bars have done since Windows 95 (egui's opens
        // only on a click).
        let ctx = ui.ctx().clone();
        let id = egui::Popup::default_response_id(&response);
        let all = egui::Id::new("top-level menus");
        let mut ids: Vec<egui::Id> = ctx.data(|d| d.get_temp(all)).unwrap_or_default();
        if !ids.contains(&id) {
            ids.push(id);
            ctx.data_mut(|d| d.insert_temp(all, ids.clone()));
        }
        if response.hovered()
            && !egui::Popup::is_id_open(&ctx, id)
            && ids.iter().any(|&other| other != id && egui::Popup::is_id_open(&ctx, other))
        {
            egui::Popup::open_id(&ctx, id);
            ctx.request_repaint();
        }
        return;
    }
    let text = egui::RichText::new(format!("{icon}    {label}")).size(17.0).color(egui::Color32::from_gray(230));
    let button = egui::Button::new(text).frame(false).min_size(egui::vec2(ui.available_width(), 48.0));
    egui::containers::menu::MenuButton::from_button(button).ui(ui, add_contents);
}

/// Help → Patch notes: every release, newest first; one opens its notes.
fn patch_notes_menu(ui: &mut egui::Ui, st: &mut UiState) {
    use crate::updates::{CURRENT, Status, parse_version};
    match st.updates.status() {
        Status::Done(releases) if !releases.is_empty() => {
            let current = parse_version(CURRENT);
            for r in releases {
                let this = r.version.is_some() && r.version == current;
                let label = format!(
                    "{}{}",
                    r.name,
                    if this { "  (this version)" } else if r.prerelease { "  (pre-release)" } else { "" }
                );
                if item(ui, &label, &r.date) {
                    st.show_updates = true;
                    st.notes_open = Some(r.tag);
                }
            }
        }
        Status::Done(_) => {
            ui.label("No releases published yet.");
        }
        Status::Checking => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading…");
            });
        }
        Status::NotChecked | Status::Failed(_) => {
            if item(ui, "Load patch notes from GitHub", "") {
                st.updates.check(ui.ctx());
                st.show_updates = true;
                st.notes_open = None;
            }
        }
    }
}

/// The updates window: whether a newer release is out, and the patch notes
/// of every release.
fn updates_window(ctx: &egui::Context, st: &mut UiState, settings: &mut Settings, actions: &mut Vec<Action>) {
    use crate::updates::{CURRENT, REPO, Status, long_date, notes_ui, parse_version};
    let mut open = st.show_updates;
    let mut close = false;
    egui::Window::new("Updates and patch notes")
        .open(&mut open)
        .collapsible(false)
        .resizable(true)
        .default_width(520.0)
        // In the middle of Ribbit's window (it can still be moved).
        .pivot(egui::Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .show(ctx, |ui| {
            let status = st.updates.status();
            let newer = st.updates.newer();
            match (&status, &newer) {
                (Status::Checking, _) => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Checking for updates…");
                    });
                }
                (Status::Failed(why), _) => {
                    ui.label(format!("Couldn't check for updates: {why}."));
                    if ui.button("Try again").clicked() {
                        st.updates.check(ctx);
                    }
                }
                (Status::Done(_), Some(r)) => {
                    use crate::install::State as Install;
                    ui.heading(format!("Ribbit {} is available", r.version_text()));
                    ui.label(format!("You have version {CURRENT}. Released {}.", long_date(&r.date)));
                    ui.add_space(4.0);
                    let package = crate::install::installable(r);
                    let install = st.updates.installer.state();
                    match &install {
                        Install::Downloading { done, total } => {
                            let mb = |b: u64| b as f32 / 1_048_576.0;
                            let part = if *total > 0 { *done as f32 / *total as f32 } else { 0.0 };
                            let text = format!("Downloading… {:.1} of {:.1} MB", mb(*done), mb(*total));
                            ui.add(egui::ProgressBar::new(part).text(text));
                        }
                        Install::Installing | Install::Ready => {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(if install == Install::Ready { "Restarting Ribbit…" } else { "Installing…" });
                            });
                        }
                        Install::Idle | Install::Failed(_) => {
                            if let Install::Failed(why) = &install {
                                ui.colored_label(egui::Color32::LIGHT_RED, format!("The update didn't install: {why}."));
                            }
                            ui.horizontal(|ui| {
                                if let Some(asset) = &package {
                                    let text = egui::RichText::new("Update now").color(egui::Color32::WHITE);
                                    if ui
                                        .add(egui::Button::new(text).fill(ACCENT))
                                        .on_hover_text("Downloads and installs it, then restarts Ribbit")
                                        .clicked()
                                    {
                                        st.updates.installer.start(asset.clone(), ctx);
                                    }
                                } else {
                                    let download = egui::RichText::new("Download").color(egui::Color32::WHITE);
                                    if ui.add(egui::Button::new(download).fill(ACCENT)).clicked() {
                                        ctx.open_url(egui::OpenUrl::new_tab(&r.url));
                                    }
                                }
                                if ui.button("Skip this version").on_hover_text("Don't announce it at start-up again").clicked() {
                                    settings.skipped_update = r.tag.clone();
                                    actions.push(Action::SettingsChanged);
                                    close = true;
                                }
                                if ui.button("Later").clicked() {
                                    close = true;
                                }
                            });
                            if package.is_some() {
                                let note = "Ribbit restarts by itself when the update is in. A game you're \
                                            playing closes first; its saves are kept.";
                                ui.label(egui::RichText::new(note).weak());
                                if install != Install::Idle && ui.link("Download it from GitHub instead").clicked() {
                                    ctx.open_url(egui::OpenUrl::new_tab(&r.url));
                                }
                            }
                        }
                    }
                }
                (Status::Done(_), None) => {
                    let text = format!("✔ You're on the latest version ({CURRENT}).");
                    ui.label(egui::RichText::new(text).color(ACCENT).strong());
                }
                (Status::NotChecked, _) => {
                    if ui.button("Check for updates").clicked() {
                        st.updates.check(ctx);
                    }
                }
            }
            if ui.checkbox(&mut settings.check_for_updates, "Check for updates when Ribbit starts").changed() {
                actions.push(Action::SettingsChanged);
            }
            let Status::Done(releases) = status else { return };
            ui.separator();
            ui.label(egui::RichText::new("Patch notes").strong().size(16.0));
            let current = parse_version(CURRENT);
            let focus = st.notes_open.take();
            egui::ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
                for r in &releases {
                    let this = r.version.is_some() && r.version == current;
                    let is_new = r.version.zip(current).is_some_and(|(v, c)| v > c);
                    let mut title = format!("{}  ·  {}", r.name, long_date(&r.date));
                    if this {
                        title.push_str("  (this version)");
                    } else if is_new {
                        title.push_str("  (new)");
                    } else if r.prerelease {
                        title.push_str("  (pre-release)");
                    }
                    let mut header = egui::CollapsingHeader::new(egui::RichText::new(title).strong())
                        .id_salt(("release", &r.tag))
                        .default_open(is_new || (this && newer.is_none()));
                    if let Some(f) = &focus {
                        header = header.open(Some(*f == r.tag));
                    }
                    let resp = header.show(ui, |ui| {
                        notes_ui(ui, &r.notes);
                        if ui.link("Open on GitHub").clicked() {
                            ctx.open_url(egui::OpenUrl::new_tab(&r.url));
                        }
                    });
                    if focus.as_ref() == Some(&r.tag) {
                        resp.header_response.scroll_to_me(Some(egui::Align::TOP));
                    }
                }
            });
            ui.add_space(4.0);
            if ui.link(format!("All releases: github.com/{REPO}/releases")).clicked() {
                ctx.open_url(egui::OpenUrl::new_tab(format!("https://github.com/{REPO}/releases")));
            }
        });
    st.show_updates = open && !close;
}

fn status_bar(ui: &mut egui::Ui, info: &Info, message: Option<&str>) {
    ui.horizontal(|ui| {
        if let Some(m) = message {
            // A message (BIOS chosen, save deleted...) in place of the
            // status text while it shows.
            ui.label(egui::RichText::new(m).color(ACCENT).strong());
            ui.ctx().request_repaint();
            return;
        }
        ui.label(if info.running { info.title.as_str() } else { "No game running" });
        if info.experimental_touch && info.running {
            ui.label(egui::RichText::new("· touch: experimental").weak());
        }
        if info.zippity && info.zippity_on_home {
            ui.label(egui::RichText::new("· on the home spot").weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if info.running {
                let text = if info.paused { "Paused".to_string() } else { format!("{:.0}% speed", info.speed) };
                ui.label(text);
            }
        });
    });
}

fn welcome(ui: &mut egui::Ui, settings: &mut Settings, info: &Info, actions: &mut Vec<Action>) {
    let phone = cfg!(target_os = "android");
    // Scrolls when it doesn't fit (a phone held sideways, a small window).
    let gap = (ui.available_height() * 0.18).max(12.0);
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| ui.vertical_centered(|ui| {
        ui.add_space(if phone { 12.0 } else { gap });
        ui.heading(egui::RichText::new("🐸 Ribbit").size(34.0));
        ui.label("An emulator for LeapFrog's handhelds and TV consoles");
        ui.add_space(18.0);
        // On a phone, a quick start: the AI note on a page of its own first,
        // then the BIOS and games.
        if phone && !settings.ai_notice_seen {
            ai_notice(ui);
            ui.add_space(16.0);
            let text = egui::RichText::new("Continue").size(18.0).color(egui::Color32::WHITE);
            let button = egui::Button::new(text).fill(ACCENT).corner_radius(10).min_size(egui::vec2(240.0, 48.0));
            if ui.add(button).clicked() {
                settings.ai_notice_seen = true;
                actions.push(Action::SettingsChanged);
            }
            ui.add_space(12.0);
            return;
        }
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
        if !phone {
            ui.add_space(14.0);
            ui.label(egui::RichText::new("You can also drop a BIOS or cartridge file onto this window.").weak());
        }
        if phone {
            ui.add_space(10.0);
            ui.label(egui::RichText::new("Swipe in from the left edge for the menu.").weak());
        }
        ui.add_space(12.0);
    }));
}

/// The README's "This project was made with AI", shortened, on the first
/// screen: players should know before they start.
fn ai_notice(ui: &mut egui::Ui) {
    let width = ui.available_width().min(560.0);
    ai_card_frame().show(ui, |ui| {
        ui.set_width(width - 48.0);
        ai_notice_text(ui);
    });
}

/// The AI note's card: dark, rounded, with a soft green edge.
fn ai_card_frame() -> egui::Frame {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(22, 26, 24))
        .stroke(egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.55)))
        .corner_radius(16)
        .inner_margin(egui::Margin::symmetric(24, 20))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(110) })
}

/// The README's "This project was made with AI", shortened: a title, the
/// note in short paragraphs, and who it's not from.
fn ai_notice_text(ui: &mut egui::Ui) {
    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
        ui.label(egui::RichText::new("This project was made with AI").size(21.0).strong().color(egui::Color32::WHITE));
        ui.add_space(2.0);
        ui.label(egui::RichText::new("Please read this before you start").size(13.5).color(ACCENT));
        ui.add_space(12.0);
        for text in [
            "Ribbit is written almost entirely by an AI coding assistant (Anthropic's Claude), working under my \
             direction: I choose what to work on, test it, supply the hardware references and decide what goes in.",
            "Many people don't want to use AI-written software, for reasons I respect: licensing and where training \
             data comes from, code quality, and the work of people who reverse-engineer hardware by hand. If that's \
             you, no hard feelings.",
            "Otherwise, please judge Ribbit by its results: every hardware claim in its notes says how sure it is \
             and where it comes from.",
        ] {
            ui.add(egui::Label::new(egui::RichText::new(text).size(15.0).color(egui::Color32::from_gray(205))).wrap());
            ui.add_space(9.0);
        }
        ui.add_space(2.0);
        ui.label(egui::RichText::new("Ribbit is not affiliated with or endorsed by LeapFrog.").size(12.5).color(egui::Color32::from_gray(130)));
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
    view: &mut LibraryView,
    idle_closed: Option<(&str, &str)>,
    since: Instant,
    actions: &mut Vec<Action>,
) {
    let LibraryView { tab, search, pull, pull_armed } = view;
    let ctx = ui.ctx().clone();
    for g in &mut library.games {
        if let Art::Image(img) = &g.art {
            let tex = ctx.load_texture(g.path.display().to_string(), img.clone(), egui::TextureOptions::LINEAR);
            g.art = Art::Texture(tex);
        }
    }
    backdrop(ui);
    // A phone: tighter margins, smaller cards, no "last played" banner.
    let phone = cfg!(target_os = "android");
    let margin = if phone { egui::Margin::symmetric(14, 10) } else { egui::Margin::symmetric(28, 20) };
    let (card_width, card_gap) = if phone { (150.0, 12.0) } else { (CARD_WIDTH, CARD_GAP) };
    egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
        // On a phone the header scrolls away with the games (it would take
        // half of a landscape screen).
        if !phone || library.games.is_empty() {
            library_header(ui, settings, library, *tab, actions);
        }
        if library.games.is_empty() && !library.scanning {
            empty_library(ui, settings, actions);
            return;
        }
        let out = egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if phone {
                pull_to_refresh_hint(ui, *pull);
                library_header(ui, settings, library, *tab, actions);
            }
            if settings.rom_folders.is_empty() {
                // Only recent games so far: ask for the folder with the rest.
                notice(ui, "These are your recent games. Choose your games folder to see all of them.", || {
                    actions.push(Action::Pick(Pick::RomFolder));
                });
                ui.add_space(18.0);
            }
            if let Some((title, console)) = idle_closed {
                idle_banner(ui, title, console, actions);
                ui.add_space(14.0);
            }
            let last = settings.recent_carts.first().filter(|_| search.is_empty());
            if let Some(game) = last.and_then(|p| library.games.iter().find(|g| &g.path == p)).filter(|_| !phone) {
                hero(ui, game, actions);
                ui.add_space(28.0);
            }
            console_tabs(ui, settings, library, tab, actions);
            ui.add_space(14.0);
            let query = search.trim().to_lowercase();
            let matches = |g: &crate::library::Game| {
                query.is_empty() || g.title.to_lowercase().contains(&query) || g.subtitle.to_lowercase().contains(&query)
            };
            let shown_games: Vec<&crate::library::Game> =
                library.games.iter().filter(|g| tab.is_none_or(|f| f.lists(g.family)) && matches(g)).collect();
            let heading = match tab {
                None => "ALL GAMES".to_string(),
                Some(f) => f.name().to_uppercase(),
            };
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{heading}  ·  {}", shown_games.len())).size(13.0).strong().color(ACCENT));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !search.is_empty()
                        && ui.add(egui::Button::new("×").frame(false)).on_hover_text("Clear the search").clicked()
                    {
                        search.clear();
                    }
                    let field = egui::TextEdit::singleline(search)
                        .hint_text("🔍  Search games")
                        .desired_width(if phone { 180.0 } else { 240.0 })
                        .margin(egui::vec2(10.0, 6.0));
                    let resp = ui.add(field);
                    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        search.clear();
                    }
                });
            });
            ui.add_space(12.0);
            if shown_games.is_empty() && !query.is_empty() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(format!("No games match \"{}\"", search.trim())).size(16.0).weak());
                });
            }
            let avail = ui.available_width();
            let cols = (((avail + card_gap) / (card_width + card_gap)).floor() as usize).max(1);
            let card_w = ((avail - card_gap * (cols - 1) as f32) / cols as f32).floor().min(card_width * 1.4);
            let shown = since.elapsed().as_secs_f32();
            for (r, row) in shown_games.chunks(cols).enumerate() {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = card_gap;
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
                ui.add_space(card_gap);
            }
        });
        if phone {
            pull_to_refresh(ui, pull, pull_armed, out.state.offset.y, actions);
        }
    });
}

/// How far to pull the list down at its top before letting go refreshes it.
const PULL_TO_REFRESH: f32 = 90.0;

/// Android: pulling the list down at its top, then letting go, looks for new
/// games, as in other apps' lists.
fn pull_to_refresh(ui: &egui::Ui, pull: &mut f32, armed: &mut bool, offset: f32, actions: &mut Vec<Action>) {
    let (drag, pressed, released) = ui.input(|i| {
        let drag = i.pointer.press_origin().zip(i.pointer.interact_pos()).map(|(o, p)| p.y - o.y);
        (if i.pointer.primary_down() { drag } else { None }, i.pointer.any_pressed(), i.pointer.any_released())
    });
    if pressed {
        *armed = offset <= 0.5;
    }
    match drag {
        // Only a pull whose finger came down with the list at its top counts.
        Some(dy) if *armed && offset <= 0.5 => *pull = dy.max(0.0),
        _ => {
            if released && *pull >= PULL_TO_REFRESH {
                actions.push(Action::RescanLibrary);
            }
            *pull = 0.0;
        }
    }
    if *pull > 0.0 {
        ui.ctx().request_repaint();
    }
}

/// The note above the list while it is pulled down.
fn pull_to_refresh_hint(ui: &mut egui::Ui, pull: f32) {
    if pull <= 0.0 {
        return;
    }
    let h = (pull * 0.5).min(56.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
    let ready = pull >= PULL_TO_REFRESH;
    let text = if ready { "⟳  Release to refresh" } else { "↓  Pull to refresh" };
    let color = if ready { ACCENT } else { egui::Color32::from_gray(170) };
    ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, text, egui::FontId::proportional(16.0), color);
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

fn library_header(ui: &mut egui::Ui, settings: &Settings, library: &Library, tab: Option<Family>, actions: &mut Vec<Action>) {
    let phone = cfg!(target_os = "android");
    if phone {
        return phone_library_header(ui, settings, library, tab, actions);
    }
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
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            if ui
                .add(egui::Button::new(egui::RichText::new("⟳").size(16.0)).corner_radius(8).min_size(egui::vec2(36.0, 32.0)))
                .on_hover_text("Look for new games in your folders")
                .clicked()
            {
                actions.push(Action::RescanLibrary);
            }
            if ui.add(egui::Button::new("Open Cartridge…").corner_radius(8).min_size(egui::vec2(0.0, 32.0))).clicked() {
                actions.push(Action::Pick(Pick::Cartridge));
            }
            if primary_button(ui, "➕  Add games folder", egui::vec2(0.0, 32.0)).clicked() {
                actions.push(Action::Pick(Pick::RomFolder));
            }
            // Zippity and RockIt Twist games play on their own console: no
            // choice to make on their pages.
            if !matches!(tab, Some(Family::Zippity | Family::RockItTwist)) {
                // (Right to left: the BIOS button shows to the right of the
                // console menu.)
                bios_picker(ui, settings, library, actions);
                console_picker(ui, settings, library, tab, actions);
            }
            // The Leapster 2 chosen: its own system (main menu, drawing,
            // games from the cartridge icon), with or without a cartridge.
            let console = library.selected_console(&settings.library_console);
            if console == Hardware::Leapster2
                && library.bios_for(console).is_some()
                && tab.is_none_or(|f| f.runs_on(console))
                && system_button(ui, "Leapster 2 Main Menu")
                    .on_hover_text("Turn on the Leapster 2 without a game: its start-up screens and main menu")
                    .clicked()
            {
                actions.push(Action::StartSystem(console));
            }
        });
    });
    // Progress on a line of its own under the heading (beside it, it ran
    // under the buttons in a narrow window).
    let pending = library.games.iter().filter(|g| matches!(g.art, Art::Pending)).count();
    if library.scanning || (library.making_art && pending > 0) {
        ui.horizontal(|ui| {
            ui.add_space(50.0);
            ui.spinner();
            let text = if library.scanning { "Looking for games…".to_string() } else { format!("Getting title screens… {pending} to go") };
            ui.label(egui::RichText::new(text).weak());
        });
    }
    ui.add_space(10.0);
}

/// The library's header on a phone: the title and progress, then the
/// console and BIOS on a row of their own (adding games is in the side menu).
fn phone_library_header(ui: &mut egui::Ui, settings: &Settings, library: &Library, tab: Option<Family>, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        let frog = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("frog icon"))).unwrap_or_else(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied([256, 256], &crate::icon::frog_glyph_rgba(256));
            let tex = ui.ctx().load_texture("frog icon", img, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("frog icon"), tex.clone()));
            tex
        });
        ui.add(egui::Image::new((frog.id(), egui::vec2(30.0, 30.0))));
        ui.label(egui::RichText::new("Games").size(26.0).family(crate::fonts::title()).color(egui::Color32::WHITE));
        let pending = library.games.iter().filter(|g| matches!(g.art, Art::Pending)).count();
        if library.scanning {
            ui.spinner();
            ui.label(egui::RichText::new("Looking for games…").weak());
        } else if library.making_art && pending > 0 {
            ui.spinner();
            ui.label(egui::RichText::new(format!("{pending} to go")).weak());
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui
                .add(egui::Button::new(egui::RichText::new("⟳").size(18.0)).corner_radius(8).min_size(egui::vec2(44.0, 40.0)))
                .clicked()
            {
                actions.push(Action::RescanLibrary);
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        if !matches!(tab, Some(Family::Zippity | Family::RockItTwist)) {
            console_picker(ui, settings, library, tab, actions);
            bios_picker(ui, settings, library, actions);
        }
        let console = library.selected_console(&settings.library_console);
        if console == Hardware::Leapster2
            && library.bios_for(console).is_some()
            && tab.is_none_or(|f| f.runs_on(console))
            && system_button(ui, "Leapster 2 Main Menu").clicked()
        {
            actions.push(Action::StartSystem(console));
        }
    });
    ui.add_space(8.0);
}

/// The console Leapster games play on (from the library and Open Cartridge): only
/// consoles that run the open page's games (all of them on "All games").
/// Choosing one opens the page with its games.
fn console_picker(ui: &mut egui::Ui, settings: &Settings, library: &Library, tab: Option<Family>, actions: &mut Vec<Action>) {
    let current = library.selected_console(&settings.library_console);
    let options: Vec<_> = library.consoles().into_iter().filter(|&hw| tab.is_none_or(|f| f.runs_on(hw))).collect();
    let usable = options.contains(&current);
    let label = if usable {
        current.name().to_string()
    } else if let [only] = options.as_slice() {
        only.name().to_string()
    } else {
        format!("{} (no BIOS)", tab.map_or(Hardware::Leapster, Family::console).name())
    };
    let hint = match tab {
        Some(Family::Leapster2) if options.is_empty() => "Add a Leapster 2 BIOS to your games folder to play these games",
        _ => "Add a Leapster TV, L-MAX or Leapster 2 BIOS to your games folder to play on those",
    };
    ui.add_enabled_ui(options.len() > 1 || (!usable && !options.is_empty()), |ui| {
        ui.spacing_mut().interact_size.y = 32.0;
        ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
        egui::ComboBox::from_id_salt("library console")
            .selected_text(format!("🎮  {label}"))
            .show_ui(ui, |ui| {
                for hw in options {
                    if ui.selectable_label(hw == current, hw.name()).clicked() {
                        actions.push(Action::SetLibraryConsole(hw));
                    }
                }
            })
            .response
            .on_hover_text("The console your games play on")
            .on_disabled_hover_text(hint);
    });
}

/// Which BIOS the chosen console plays on, when there is more than one
/// (US, UK and Universal Leapster BaseROMs, the Leapster 2's 0.6, m9, B5...):
/// every one found in the games folders or opened with File > Open BIOS.
fn bios_picker(ui: &mut egui::Ui, settings: &Settings, library: &Library, actions: &mut Vec<Action>) {
    let console = library.selected_console(&settings.library_console);
    let list = library.bios_list(console);
    let Some(current) = library.bios_file(console) else {
        return;
    };
    ui.scope(|ui| {
        ui.spacing_mut().interact_size.y = 32.0;
        ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
        egui::ComboBox::from_id_salt("library bios")
            .selected_text(format!("💾  {}", current.short_name()))
            .show_ui(ui, |ui| {
                for b in &list {
                    let file = b.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    let label = egui::RichText::new(&b.name);
                    if ui
                        .selectable_label(b.path == current.path, label)
                        .on_hover_text(format!("{file}\n{}", b.path.display()))
                        .clicked()
                    {
                        actions.push(Action::PickBios(console, b.path.clone()));
                    }
                }
                ui.separator();
                if ui.button("Open another BIOS…").clicked() {
                    actions.push(Action::Pick(Pick::Bios));
                }
            })
            .response
            .on_hover_text(format!("The {} BIOS your games play on", console.name()));
    });
}

/// A console's own system (no game), in the Leapster 2's purple.
fn system_button(ui: &mut egui::Ui, text: &str) -> egui::Response {
    const PURPLE: egui::Color32 = egui::Color32::from_rgb(108, 72, 190);
    const PURPLE_HOVER: egui::Color32 = egui::Color32::from_rgb(128, 92, 212);
    let id = ui.next_auto_id();
    let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
    ui.add(
        egui::Button::new(egui::RichText::new(text).strong().color(egui::Color32::WHITE))
            .fill(if hovered { PURPLE_HOVER } else { PURPLE })
            .stroke(egui::Stroke::NONE)
            .corner_radius(8)
            .min_size(egui::vec2(0.0, 32.0)),
    )
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

/// Red banner: the last game was closed because the console (`console`, as
/// players know it) turned itself off after inactivity.
fn idle_banner(ui: &mut egui::Ui, title: &str, console: &str, actions: &mut Vec<Action>) {
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
                    egui::RichText::new(format!("Nothing was pressed for a while, so the {console} switched itself off."))
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
                actions.push(Action::PlayFromLibrary(game.path.clone()));
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
                    for t in &game.tags {
                        badge(ui, t);
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
                    actions.push(Action::PlayFromLibrary(game.path.clone()));
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
    for r in region.into_iter().chain(game.tags.iter().copied()) {
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
        actions.push(Action::PlayFromLibrary(game.path.clone()));
    }
    response.context_menu(|ui| {
        if ui.button("▶ Play").clicked() {
            actions.push(Action::PlayFromLibrary(game.path.clone()));
            ui.close();
        }
        if ui.button("Get title screen again").clicked() {
            actions.push(Action::RemakeArt(game.path.clone()));
            ui.close();
        }
        ui.separator();
        // Leapster games keep their progress on their own cartridge chip; a
        // Zippity's or RockIt Twist's belongs to the whole console.
        let own_save = !matches!(game.family, Family::Zippity | Family::RockItTwist);
        let delete = ui
            .add_enabled(
                own_save,
                egui::Button::new(egui::RichText::new("Delete save data…").color(egui::Color32::from_rgb(235, 87, 80))),
            )
            .on_hover_text("Removes this game's progress, profiles and save states")
            .on_disabled_hover_text("This console's saves belong to the whole console, not one game");
        if delete.clicked() {
            actions.push(Action::AskDeleteSave(game.path.clone(), game.title.clone()));
            ui.close();
        }
    });
}

/// "Delete save data for <title>?" Delete / Cancel.
fn delete_save_prompt(ctx: &egui::Context, path: &std::path::Path, title: &str) -> Option<Option<Action>> {
    let screen = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("delete_save_dim")))
        .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
    let mut answer = None;
    egui::Area::new(egui::Id::new("delete_save"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(24, 27, 30))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(214, 64, 58, 110)))
                .corner_radius(14)
                .inner_margin(egui::Margin::symmetric(26, 22))
                .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_width(320.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Delete save data?").size(20.0).strong().color(egui::Color32::WHITE));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new(title).size(15.0).color(egui::Color32::from_gray(220)));
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(
                                "Its progress, profiles and save states are removed. A copy goes to the backups \
                                 folder in Ribbit's data folder, in case you change your mind.",
                            )
                            .size(13.0)
                            .color(egui::Color32::from_gray(170)),
                        );
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            let (del, cancel) = (egui::vec2(120.0, 38.0), egui::vec2(100.0, 38.0));
                            ui.add_space((ui.available_width() - del.x - cancel.x - ui.spacing().item_spacing.x) / 2.0);
                            let red = egui::Button::new(egui::RichText::new("Delete").strong().color(egui::Color32::WHITE))
                                .fill(egui::Color32::from_rgb(190, 52, 46))
                                .corner_radius(8)
                                .min_size(del);
                            if ui.add_sized(del, red).clicked() {
                                answer = Some(Some(Action::DeleteSaveData(path.to_path_buf())));
                            }
                            let cancel_button =
                                egui::Button::new(egui::RichText::new("Cancel").color(egui::Color32::from_gray(200)))
                                    .fill(egui::Color32::TRANSPARENT)
                                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(70)))
                                    .corner_radius(8)
                                    .min_size(cancel);
                            if ui.add_sized(cancel, cancel_button).clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                                answer = Some(None);
                            }
                        });
                    });
                });
        });
    answer
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
        ui.separator();
        egui::CollapsingHeader::new("RockIt Twist keys").show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                egui::Grid::new("twist-bindings").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
                    for t in crate::twist::actions() {
                        ui.label(crate::twist::action_label(t));
                        let label = if st.capturing_twist == Some(t) {
                            "press a key…".to_string()
                        } else {
                            crate::twist::keys_for(t)
                        };
                        if ui.add(egui::Button::new(label).min_size([110.0, 0.0].into())).clicked() {
                            st.capturing_twist = Some(t);
                        }
                        ui.end_row();
                    }
                });
            });
            if ui.button("Restore RockIt Twist defaults").clicked() {
                settings.twist_keys.clear();
                crate::twist::set_bindings(&settings.twist_keys);
                st.capturing_twist = None;
                actions.push(Action::SettingsChanged);
            }
        });
    });
    st.show_bindings = open;
    if !open {
        st.capturing = None;
        st.capturing_twist = None;
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
                ("Console view on/off", "F7"),
            ] {
                ui.label(what);
                ui.label(key);
                ui.end_row();
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("On the Zippity").strong());
        egui::Grid::new("zippity").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
            for (b, _, what) in crate::zippity::CONTROLS {
                ui.label(what);
                ui.label(settings.key_for(b).map_or_else(|| format!("{b:?}"), |k| format!("{} ({b:?})", settings::key_label(k))));
                ui.end_row();
            }
        });
        ui.separator();
        ui.label(egui::RichText::new("On the RockIt Twist (research preview)").strong());
        egui::Grid::new("twist").num_columns(2).spacing([24.0, 4.0]).show(ui, |ui| {
            use crate::twist::{TwistKey, keys_for};
            use ribbit_twist::input::{Button as T, Rotary as R};
            let both = |a: TwistKey, b: TwistKey| format!("{} / {}", keys_for(a), keys_for(b));
            for (what, keys) in [
                ("D-pad", "W A S D or the arrows".to_string()),
                ("Switch left / right", both(TwistKey::Press(T::SwitchLeft), TwistKey::Press(T::SwitchRight))),
                ("Button grid", "Number pad 1-9 (as laid out)".to_string()),
                ("Spinner left / right", both(TwistKey::Turn(R::Spinner, -1), TwistKey::Turn(R::Spinner, 1))),
                ("Yellow dial up / down", both(TwistKey::Turn(R::YellowDial, 1), TwistKey::Turn(R::YellowDial, -1))),
                ("Green dial up / down", both(TwistKey::Turn(R::GreenDial, 1), TwistKey::Turn(R::GreenDial, -1))),
                ("Red dial up / down", both(TwistKey::Turn(R::RedDial, 1), TwistKey::Turn(R::RedDial, -1))),
                ("Slider swipe up / down", "Page Up / Page Down, mouse wheel".to_string()),
                ("Hint", keys_for(TwistKey::Press(T::Hint))),
                ("Home", keys_for(TwistKey::Press(T::Home))),
                ("Check mark", keys_for(TwistKey::Press(T::Check))),
                ("Volume up / down", both(TwistKey::Press(T::VolumeUp), TwistKey::Press(T::VolumeDown))),
            ] {
                ui.label(what);
                ui.label(keys);
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

    updates_window(ctx, st, settings, actions);
    // Installed (even with the window closed since): restart into it, once.
    if st.updates.installer.state() == crate::install::State::Ready && !st.restarting {
        st.restarting = true;
        actions.push(Action::RestartForUpdate);
    }

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
                    ("RockIt Twist", "NOT EMULATED: research preview (controls, packages); game packs are encrypted"),
                ] {
                    ui.label(part);
                    ui.label(state);
                    ui.end_row();
                }
            });
        },
    );

    if let Some(path) = st.zippity_clean.clone()
        && let Some(a) = zippity_clean_prompt(ctx, &path)
    {
        st.zippity_clean = None;
        actions.push(a);
    }

    if let Some((path, title)) = st.delete_save.clone()
        && let Some(answer) = delete_save_prompt(ctx, &path, &title)
    {
        st.delete_save = None;
        actions.extend(answer);
    }

    if let Some(problem) = st.error.clone()
        && problem_card(ctx, &problem)
    {
        st.error = None;
    }
}

/// The error card over the dimmed screen; true when dismissed (OK, Enter
/// or Escape).
fn problem_card(ctx: &egui::Context, p: &Problem) -> bool {
    let accent = egui::Color32::from_rgb(232, 98, 82);
    let screen = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("problem_dim")))
        .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
    let mut close = ctx.input(|i| i.key_pressed(egui::Key::Escape) || i.key_pressed(egui::Key::Enter));
    egui::Area::new(egui::Id::new("problem"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(24, 27, 30))
                .stroke(egui::Stroke::new(1.0, accent.gamma_multiply(0.45)))
                .corner_radius(14)
                .inner_margin(egui::Margin::symmetric(26, 22))
                .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    let width = (screen.width() - 80.0).clamp(240.0, 400.0);
                    ui.set_width(width);
                    ui.horizontal(|ui| {
                        // A round "!" badge beside the title.
                        let (rect, _) = ui.allocate_exact_size(egui::vec2(30.0, 30.0), egui::Sense::hover());
                        ui.painter().circle_filled(rect.center(), 15.0, accent.gamma_multiply(0.2));
                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER, "!", egui::FontId::proportional(19.0), accent);
                        ui.add_space(6.0);
                        ui.add(egui::Label::new(egui::RichText::new(&p.title).size(19.0).strong().color(egui::Color32::WHITE)).wrap());
                    });
                    ui.add_space(10.0);
                    ui.add(egui::Label::new(egui::RichText::new(&p.explain).size(14.0).color(egui::Color32::from_gray(195))).wrap());
                    if !p.details.is_empty() {
                        ui.add_space(10.0);
                        egui::CollapsingHeader::new(egui::RichText::new("Details").size(13.0).color(egui::Color32::from_gray(150)))
                            .id_salt("problem_details")
                            .show(ui, |ui| {
                                egui::Frame::NONE
                                    .fill(egui::Color32::from_rgb(16, 18, 20))
                                    .corner_radius(8)
                                    .inner_margin(egui::Margin::same(10))
                                    .show(ui, |ui| {
                                        ui.set_width(ui.available_width());
                                        egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                                            ui.add(egui::Label::new(egui::RichText::new(&p.details).monospace().size(12.0).color(egui::Color32::from_gray(170))).wrap());
                                        });
                                    });
                            });
                    }
                    ui.add_space(16.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if primary_button(ui, "OK", egui::vec2(90.0, 36.0)).clicked() {
                            close = true;
                        }
                        if !p.details.is_empty() {
                            let copy = egui::Button::new(egui::RichText::new("Copy details").color(egui::Color32::from_gray(200)))
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(70)))
                                .corner_radius(8)
                                .min_size(egui::vec2(0.0, 36.0));
                            if ui.add(copy).on_hover_text("Copies the title and the technical message, for a bug report").clicked() {
                                ui.ctx().copy_text(format!("{}\n{}\n\n{}", p.title, p.explain, p.details));
                            }
                        }
                    });
                });
        });
    close
}

/// Asks whether to clear a previous owner's players from a Zippity dump:
/// a small card over the dimmed screen.
fn zippity_clean_prompt(ctx: &egui::Context, path: &std::path::Path) -> Option<Action> {
    let screen = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("zippity_clean_dim")))
        .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
    let mut answer = None;
    egui::Area::new(egui::Id::new("zippity_clean"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(24, 27, 30))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(46, 170, 82, 90)))
                .corner_radius(14)
                .inner_margin(egui::Margin::symmetric(26, 22))
                .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Start fresh?").size(20.0).strong().color(egui::Color32::WHITE));
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Old players are still saved on this Zippity.").size(14.0).color(egui::Color32::from_gray(185)));
                        ui.add_space(18.0);
                        ui.horizontal(|ui| {
                            // Centre the two buttons.
                            let (clear, keep) = (egui::vec2(150.0, 38.0), egui::vec2(90.0, 38.0));
                            ui.add_space((ui.available_width() - clear.x - keep.x - ui.spacing().item_spacing.x) / 2.0);
                            if primary_button(ui, "Clear them", clear).on_hover_text("The original file is kept as a .bak").clicked() {
                                answer = Some(Action::CleanZippityDump(path.to_path_buf()));
                            }
                            let keep_button = egui::Button::new(egui::RichText::new("Keep").color(egui::Color32::from_gray(200)))
                                .fill(egui::Color32::TRANSPARENT)
                                .stroke(egui::Stroke::new(1.0, egui::Color32::from_gray(70)))
                                .corner_radius(8)
                                .min_size(keep);
                            if ui.add_sized(keep, keep_button).clicked() {
                                answer = Some(Action::KeepZippitySaves(path.to_path_buf()));
                            }
                        });
                    });
                });
        });
    answer
}

fn toasts(ctx: &egui::Context, st: &mut UiState) {
    st.toasts.retain(|(_, t)| t.elapsed() < TOAST_TIME);
    if st.toasts.is_empty() {
        return;
    }
    // Bottom centre, above the status bar: clear of the menus, the library's
    // header and the game's own text. Solid, so nothing shows through.
    egui::Area::new("toasts".into())
        .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -44.0])
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            ui.spacing_mut().item_spacing.y = 6.0;
            for (text, _) in &st.toasts {
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(28, 32, 30))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(46, 170, 82, 120)))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: egui::Color32::from_black_alpha(120) })
                    .show(ui, |ui| {
                        ui.set_max_width(560.0);
                        ui.add(egui::Label::new(egui::RichText::new(text).color(egui::Color32::from_gray(235))).wrap());
                    });
            }
        });
    ctx.request_repaint();
}
