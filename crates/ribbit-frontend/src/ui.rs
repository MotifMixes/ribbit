//! Menus, dialogs and overlays (egui). The UI only reads state and emits
//! `Action`s; `app.rs` performs them.

use crate::cheat::{SearchOp, ValueType};
use crate::files::{Browser, Pick};
use crate::gpu::Display;
use crate::library::{Art, Family, Library};
use crate::settings::{self, RecordEncoder, BINDABLE, Settings};
use ribbit_input::Button;
use ribbit_memory::known::Hardware;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Pick(Pick),
    /// Show this folder in the system's file manager.
    OpenFolder(PathBuf),
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
    /// "Custom settings for this game" ticked or unticked.
    GameSettings(bool),
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
    /// Turn on the original LeapPad with no cartridge (its built-in book).
    StartLeapPad,
    /// Open this virtual book on the running LeapPad.
    OpenLeapPadBook(PathBuf),
    /// Use this folder as the RockIt Twist's storage content (None: none).
    SetTwistContent(Option<PathBuf>),
    SetTwistUsb(bool),
    /// Offer a Leapster 2 on the PC's USB, or stop.
    SetLeapster2Usb(bool),
    /// Unpack this zip of RockIt Twist packages and use it as the content.
    ImportTwistZip(PathBuf),
    /// Change the Leapster 2's SD card (Emulation › Leapster 2 SD card).
    SdCard(crate::sdcard::Job),
    /// Use this folder as the Leapster 2's SD card (None: the card file).
    SetSdFolder(Option<PathBuf>),
    /// The console view was turned on or off.
    ConsoleViewChanged,
    /// The update is installed: save, start the new version and quit.
    RestartForUpdate,
    /// Start or stop screen recording.
    ToggleRecord,
    /// Choose the recording output folder.
    PickRecordFolder,
    /// Set the recording output folder.
    SetRecordFolder(PathBuf),
    /// Recordings back to the default folder (Videos).
    ResetRecordFolder,
    /// Show the recordings folder in the file manager.
    OpenRecordFolder,
    /// First pass of the cheat / memory search.
    CheatFirstScan,
    /// Subsequent pass filtering the cheat candidates.
    CheatNextScan,
    /// Reset / clear the cheat search.
    CheatReset,
    /// The debugger's windows asked for this (debugger.rs).
    Debug(crate::debugger::Action),
    /// Freeze address at a value.
    CheatFreeze(u32, u32),
    /// Write a value to an address once.
    CheatSet(u32, u32),
    /// Unfreeze this address.
    CheatUnfreeze(u32),
    Quit,
    /// Open or close the side drawer (Android library header button).
    ToggleDrawer,
    /// Star or unstar a game.
    ToggleFavorite(PathBuf),
    /// Toggle visibility of "Internal" (built-in) games for this tab
    /// (None = "All games").
    ToggleBuiltinGames(Option<Family>),
    /// Create a new folder with these games (immediately shows rename dialog).
    CreateFolder(Vec<PathBuf>),
    /// Navigate into a library folder by index.
    OpenLibraryFolder(usize),
    /// Return to the main library from an open folder.
    CloseLibraryFolder,
    /// Rename the folder at this index.
    RenameFolder { idx: usize, name: String },
    /// Delete the folder at this index (games stay in the library).
    DeleteFolder(usize),
    /// Add a game to an existing folder.
    AddToFolder { folder: usize, game: PathBuf },
    /// Remove a game from a folder.
    RemoveFromFolder { folder: usize, game: PathBuf },
}

pub const STATE_SLOTS: u8 = 5;

/// What the UI shows (read-only snapshot).
pub struct Info {
    pub running: bool,
    /// A cartridge game is running, and whether its own settings are on.
    pub game_settings: Option<bool>,
    /// The original LeapPad is running.
    pub leappad: bool,
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
    /// A recording is in progress (duration so far in seconds).
    pub recording: Option<u64>,
    /// An encoding task is running in the background.
    pub encoding: bool,
    /// Where recordings are saved (the chosen folder, else Videos).
    pub record_dir: PathBuf,
    /// Cheat search candidates (addr, value), capped at 200 for display.
    pub cheat_candidates: Vec<(u32, u32)>,
    /// A cheat scan has been done (next-scan is available).
    pub cheat_has_scan: bool,
    /// Active freezes (addr, value, label).
    pub cheat_freezes: Vec<(u32, u32, String)>,
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
    /// Messages and when each stops showing.
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
    pub drawer: bool,
    /// Android: asking whether to save a state before exiting the game.
    confirm_back: bool,
    /// Help › User Guide.
    pub manual: crate::manual::ManualState,
    /// Whether the cheat / memory search panel is open.
    pub show_cheat: bool,
    /// Debug › Memory viewer and Debugger (debugger.rs), and what they show
    /// this frame (set by the app).
    pub debug: crate::debugger::DebugUi,
    pub debug_snapshot: crate::debugger::Snapshot,
    /// Cheat search: typed value text.
    pub cheat_value_text: String,
    /// The value Set and Freeze write (empty: Freeze keeps the current one).
    pub cheat_new_value: String,
    /// Cheat search: comparison operator.
    pub cheat_op: crate::cheat::SearchOp,
    /// Cheat search: value width.
    pub cheat_ty: crate::cheat::ValueType,
    /// The folder currently being renamed (index into settings.folders).
    pub renaming_folder: Option<usize>,
    /// Text in the rename-folder input.
    pub rename_text: String,
}

/// What the library shows.
#[derive(Default)]
pub struct LibraryView {
    /// The Books side (book readers: the LeapPad), not the Games side.
    pub books: bool,
    /// The page: None for all games (or all books), else the games of one
    /// family's console.
    pub tab: Option<Family>,
    /// Search text.
    pub search: String,
    /// Android: how far the list is pulled down at its top (points), to
    /// refresh it.
    pub pull: f32,
    /// The finger touched down with the list at its top (only then does a
    /// pull count).
    pub pull_armed: bool,
    /// Show only favorited games (the ★ Favorites tab).
    pub favorites_only: bool,
    /// A library folder is open: its index in settings.folders.
    pub open_folder: Option<usize>,
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
        self.toast_for(text, TOAST_TIME);
    }

    /// A message shown for `time` (longer than the usual few seconds).
    pub fn toast_for(&mut self, text: impl Into<String>, time: Duration) {
        self.toasts.push((text.into(), Instant::now() + time));
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
        egui::Panel::bottom("status").show(ui, |ui| status_bar(ui, info));
    }
    let was_running = std::mem::replace(&mut st.was_running, info.running);
    let mut area = egui::Rect::NOTHING;
    egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
        area = ui.max_rect();
        // The library once any console's system files are known (a player
        // with only a LeapPad, Zippity or RockIt Twist has no Leapster BIOS).
        let have_system = info.bios_name.is_some() || settings.leappad_bios.is_some() || library.has_system_files();
        if !info.running && have_system {
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
            Pick::RecordFolder => Action::SetRecordFolder(path),
            Pick::LeapPadBook => Action::OpenLeapPadBook(path),
            Pick::SdFolder => Action::SetSdFolder(Some(path)),
            Pick::SdImage => Action::SdCard(crate::sdcard::Job::LoadImage(path)),
            Pick::SdExport => Action::SdCard(crate::sdcard::Job::Export(path)),
        });
    }
    if phone {
        side_menu(&ctx, st, settings, info, actions);
    }
    if st.show_cheat {
        cheat_window(&ctx, st, info, actions);
    }
    if st.debug.open() && st.debug.in_app {
        let snap = std::mem::take(&mut st.debug_snapshot);
        let mut asked = Vec::new();
        crate::debugger::windows(&ctx, &mut st.debug, &snap, &mut asked);
        st.debug_snapshot = snap;
        actions.extend(asked.into_iter().map(Action::Debug));
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
    // Dropped down from the top, under the menu bar; the status bar keeps
    // showing the game and its speed (a message in its place hid them).
    toasts(&ctx, st, area.top());
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
    let back = ctx.input(|i| i.key_pressed(egui::Key::BrowserBack));
    if settings.touch_menu_button {
        // Floating ☰ button in the top-left corner opens/closes the menu;
        // swipe-from-left is disabled to avoid conflicts with the game.
        if info.running {
            let btn_size = egui::vec2(40.0, 40.0);
            let btn_rect = egui::Rect::from_min_size(screen.min + egui::vec2(8.0, 8.0), btn_size);
            egui::Area::new("side-menu-btn".into()).order(egui::Order::Foreground).fixed_pos(btn_rect.min).show(ctx, |ui| {
                let resp = ui.allocate_rect(btn_rect, egui::Sense::click());
                let alpha = if resp.hovered() { 180u8 } else { 110u8 };
                ui.painter().rect_filled(btn_rect, 8.0, egui::Color32::from_black_alpha(alpha));
                ui.painter().text(btn_rect.center(), egui::Align2::CENTER_CENTER, "☰", egui::FontId::proportional(22.0), egui::Color32::WHITE);
                if resp.clicked() {
                    st.drawer = !st.drawer;
                }
            });
        }
    } else {
        let swipe = ctx.input(|i| i.pointer.press_origin().zip(i.pointer.interact_pos()).map(|(o, p)| (o, p.x - o.x)));
        match swipe {
            Some((origin, dx)) if !st.drawer && origin.x <= SIDE_MENU_EDGE && dx > SWIPE => st.drawer = true,
            Some((origin, dx)) if st.drawer && origin.x <= width && dx < -SWIPE => st.drawer = false,
            _ => {}
        }
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
    let windows = open_windows(st);
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
    // Something chosen, or a window opened from a menu (the guide, the
    // controls...): the menu goes, so it is not left on top of it.
    if actions.len() > before || open_windows(st) > windows {
        st.drawer = false;
    }
}

/// How many of the UI's windows are open.
fn open_windows(st: &UiState) -> usize {
    [st.manual.open, st.show_updates, st.show_controls, st.show_about, st.show_bindings, st.show_status, st.browser.open.is_some()]
        .into_iter()
        .filter(|&o| o)
        .count()
}

/// Before exiting the game (back to the games): save a state first?
fn back_to_games_question(ctx: &egui::Context, st: &mut UiState, info: &Info, actions: &mut Vec<Action>) {
    let dialog_frame = egui::Frame::new()
        .fill(egui::Color32::from_rgb(22, 26, 24))
        .stroke(egui::Stroke::new(1.0, ACCENT.gamma_multiply(0.55)))
        .corner_radius(16)
        .inner_margin(egui::Margin::same(20))
        .shadow(egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(110) });
    let modal = egui::Modal::new(egui::Id::new("back-to-games")).frame(dialog_frame).show(ctx, |ui| {
        ui.set_width(ctx.content_rect().width().min(360.0) - 32.0);
        ui.label(egui::RichText::new("Exit the game?").size(18.0).strong().color(egui::Color32::WHITE));
        ui.add_space(4.0);
        let note = match info.slots.first().cloned().flatten() {
            Some(when) => format!("Save a state first to carry on from here later (it replaces the one from {when})."),
            None => "Save a state first to carry on from here later.".to_string(),
        };
        ui.add(egui::Label::new(egui::RichText::new(note).size(14.0).color(egui::Color32::from_gray(190))).wrap());
        ui.add_space(14.0);
        // Full-width buttons, their labels centred.
        ui.with_layout(egui::Layout::top_down_justified(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            let button = |text: &str, fill: egui::Color32, stroke: egui::Stroke| {
                let text = egui::RichText::new(text).size(16.0).strong().color(egui::Color32::WHITE);
                egui::Button::new(text).corner_radius(10).min_size(egui::vec2(0.0, 44.0)).fill(fill).stroke(stroke)
            };
            let none = egui::Stroke::NONE;
            if ui.add(button("Save and exit", ACCENT, none)).clicked() {
                actions.push(Action::SaveState(1));
                actions.push(Action::CloseCartridge);
                st.confirm_back = false;
            }
            if ui.add(button("Exit without saving", egui::Color32::from_gray(58), none)).clicked() {
                actions.push(Action::CloseCartridge);
                st.confirm_back = false;
            }
            let quiet = egui::Stroke::new(1.0, egui::Color32::from_gray(70));
            if ui.add(button("Cancel", egui::Color32::TRANSPARENT, quiet)).clicked() {
                st.confirm_back = false;
            }
        });
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
        let (rec_icon, rec_label, rec_detail) = match &info.recording {
            Some(secs) => {
                let m = secs / 60;
                let s = secs % 60;
                ("⏹", "Stop recording", format!("{m}:{s:02}"))
            }
            None => ("⏺", "Record", "".to_string()),
        };
        if !info.encoding && drawer_row(ui, rec_icon, rec_label, &rec_detail, None, true) {
            actions.push(Action::ToggleRecord);
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
    if drawer_row(ui, "☰", "Menu button", "Corner button instead of swipe", Some(settings.touch_menu_button), true) {
        settings.touch_menu_button = !settings.touch_menu_button;
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

/// The library's pages: all games, then each console that is here and has
/// games (all the Leapster models share one page; the Leapster 2's page has
/// the games for it alone); on the Books side the same for book readers.
/// The console is chosen in the header's menu.
#[allow(clippy::too_many_arguments)]
fn console_tabs(
    ui: &mut egui::Ui,
    settings: &Settings,
    library: &Library,
    books: bool,
    tab: &mut Option<Family>,
    favorites_only: &mut bool,
    open_folder: &mut Option<usize>,
    actions: &mut Vec<Action>,
) {
    let side = |g: &crate::library::Game| g.family.is_book() == books;
    let count = |f: Option<Family>| library.games.iter().filter(|g| side(g) && f.is_none_or(|f| f.lists(g.family))).count();
    // A page for each console that is here and has games.
    let families: Vec<Family> = Family::ALL
        .into_iter()
        .filter(|&f| f.is_book() == books && library.has_console(f.console()) && library.games.iter().any(|g| f.lists(g.family)))
        .collect();
    if tab.is_some_and(|t| !families.contains(&t)) {
        *tab = None;
    }
    let fav_count = settings.favorites.iter().filter(|p| library.games.iter().any(|g| &g.path == *p && side(g))).count();
    // Tabs: a label each over a shared rule; the open one in the accent colour.
    let top = ui.cursor().min.y;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(26.0, 6.0);
        // ★ Favorites tab first (only when there are any).
        if fav_count > 0 {
            let selected = *favorites_only;
            let font = egui::FontId::proportional(16.0);
            let name_g = ui.painter().layout_no_wrap("★ Favorites".to_string(), font, egui::Color32::WHITE);
            let count_g = ui.painter().layout_no_wrap(fav_count.to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
            let size = egui::vec2(name_g.size().x + 6.0 + count_g.size().x, 34.0);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let fg = if selected { STAR_YELLOW } else if resp.hovered() { egui::Color32::from_gray(240) } else { egui::Color32::from_gray(165) };
            let p = ui.painter();
            let y = rect.top() + 4.0;
            p.galley(egui::pos2(rect.left(), y), name_g.clone(), fg);
            let count_pos = egui::pos2(rect.left() + name_g.size().x + 6.0, y + name_g.size().y - count_g.size().y - 1.0);
            p.galley(count_pos, count_g, fg.gamma_multiply(0.7));
            if selected {
                let bar = egui::Rect::from_min_max(egui::pos2(rect.left(), rect.bottom() - 3.0), rect.right_bottom());
                p.rect_filled(bar, 2.0, STAR_YELLOW);
            }
            if resp.clicked() {
                *favorites_only = true;
                *tab = None;
                if open_folder.is_some() { actions.push(Action::CloseLibraryFolder); }
            }
        }
        // Regular family tabs (None = All games).
        for choice in std::iter::once(None).chain(families.into_iter().map(Some)) {
            let selected = !*favorites_only && *tab == choice;
            let name = choice.map_or(if books { "All books" } else { "All games" }, Family::name);
            let font = egui::FontId::proportional(16.0);
            let name_g = ui.painter().layout_no_wrap(name.to_string(), font.clone(), egui::Color32::WHITE);
            let count_g =
                ui.painter().layout_no_wrap(count(choice).to_string(), egui::FontId::proportional(12.0), egui::Color32::WHITE);
            let size = egui::vec2(name_g.size().x + 6.0 + count_g.size().x, 34.0);
            let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
            let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
            let fg = if selected { ACCENT } else if resp.hovered() { egui::Color32::from_gray(240) } else { egui::Color32::from_gray(165) };
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
                *favorites_only = false;
                *tab = choice;
                if open_folder.is_some() { actions.push(Action::CloseLibraryFolder); }
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
        sub_menu(ui, "Open Recent", |ui| {
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
        sub_menu(ui, "Games Folders", |ui| {
            for f in settings.rom_folders.clone() {
                let b = egui::Button::new(format!("x  {}", f.display()));
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
        if let Some(mut own) = info.game_settings
            && ui
                .checkbox(&mut own, "Custom settings for this game")
                .on_hover_text(
                    "While this is on, changes to the picture, sound, speed and controls are for this game only, \
                     and come back each time it starts. Other games keep your usual settings.",
                )
                .changed()
        {
            actions.push(Action::GameSettings(own));
        }
        ui.separator();
        ui.add_enabled_ui(info.running, |ui| {
            sub_menu(ui, "Save State", |ui| {
                for slot in 1..=STATE_SLOTS {
                    let when = info.slots.get(slot as usize - 1).cloned().flatten().unwrap_or_else(|| "empty".into());
                    let key = if slot == 1 { "F5" } else { "" };
                    if ui.add(egui::Button::new(format!("Slot {slot}  ({when})")).shortcut_text(key)).clicked() {
                        actions.push(Action::SaveState(slot));
                        ui.close();
                    }
                }
            });
            sub_menu(ui, "Load State", |ui| {
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
        sub_menu(ui, "Speed", |ui| {
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
            ui.indent("flash_indent", |ui| {
                if ui
                    .checkbox(&mut settings.flash_before_idle_off, "Flash taskbar before turning off")
                    .on_hover_text("Flash Ribbit's taskbar button in the last minute. On Windows this also shows an auto-hidden taskbar until you click Ribbit.")
                    .changed()
                {
                    actions.push(Action::SettingsChanged);
                }
            });
        });
        if ui
            .checkbox(&mut settings.pause_in_background, "Pause in background")
            .on_hover_text("Pause the game while another window is active (not a RockIt Twist plugged into this PC over USB).")
            .changed()
        {
            actions.push(Action::SettingsChanged);
        }
        if ui
            .checkbox(&mut settings.close_with_shutdown, "Animate game shutdown (slow, experimental)")
            .on_hover_text("When closing a game, press the console's power button first, so the game can play its own shutdown animation. Most games take longer than Ribbit waits (it closes after 3 s) and some ignore it, so closing often just pauses for 3 s. Not used if a save state was loaded this session, or while the game is paused. Off: back to the library at once.")
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
        let usb = settings.leapster2_usb;
        if ui
            .button(if usb {
                // Green while plugged in, red while not (as the RockIt Twist's).
                egui::RichText::new("🔌  Leapster 2: unplug from this PC (USB)").color(egui::Color32::from_rgb(80, 200, 110))
            } else {
                egui::RichText::new("🔌  Leapster 2: plug into this PC (USB)").color(egui::Color32::from_rgb(235, 90, 90))
            })
            .on_hover_text(
                "Experimental. Connects the emulated Leapster 2 to this PC as a USB cable would, so programs such as \
                 LeapLoader can use its two drives. Needs usbip-win2 (winget install usbip-win2). A running Leapster 2 \
                 restarts once when plugged in, as a real one does. Wait about 7 seconds, then run: \
                 usbip attach -r 127.0.0.1 -b 1-1",
            )
            .clicked()
        {
            actions.push(Action::SetLeapster2Usb(!usb));
            ui.close();
        }
        sub_menu(ui, "💾  Leapster 2 SD card", |ui| {
            let folder = settings.leapster2_sd_folder.clone();
            let name = folder.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            if ui
                .button(match &name {
                    Some(n) => format!("Folder: {n}…"),
                    None => "SD card folder…".to_string(),
                })
                .on_hover_text(
                    "A folder on this PC that is the Leapster 2's SD card, like the RockIt Twist's content folder: \
                     the card is made from it each time the Leapster 2 starts, and what the console writes to the \
                     card (profiles, saves, logs) goes back into it. Lay it out like the card's top level; for a \
                     LeapLoader card, the folder LeapLoader built. An empty folder first gets the card's files. \
                     The card holds 2 GB. Nothing to install.",
                )
                .clicked()
            {
                actions.push(Action::Pick(Pick::SdFolder));
                ui.close();
            }
            if let Some(dir) = &folder {
                if ui.button("Open the folder").clicked() {
                    actions.push(Action::OpenFolder(dir.clone()));
                    ui.close();
                }
                if ui.button("Stop using the folder").on_hover_text("Back to the card file in the save folder. The folder's files stay where they are.").clicked() {
                    actions.push(Action::SetSdFolder(None));
                    ui.close();
                }
            }
            ui.separator();
            if ui
                .button("Load a card image…")
                .on_hover_text(if folder.is_some() {
                    "Copies a card image's files into the SD card folder: a real card's image (.img from Win32 Disk \
                     Imager or dd) or a Ribbit card file (sdcard-….img)."
                } else {
                    "Replaces the card with an image: a real card's image (.img from Win32 Disk Imager or dd) \
                     or a Ribbit card file (sdcard-….img, e.g. from another BIOS). The old card is kept as .img.bak."
                })
                .clicked()
            {
                actions.push(Action::Pick(Pick::SdImage));
                ui.close();
            }
            if folder.is_none() {
                if ui.button("Copy the card's files to a folder…").on_hover_text("For the games' saves, or a backup.").clicked() {
                    actions.push(Action::Pick(Pick::SdExport));
                    ui.close();
                }
                if ui
                    .button("Empty the card")
                    .on_hover_text("A freshly formatted card, as a new Leapster 2's. The old card is kept as .img.bak.")
                    .clicked()
                {
                    actions.push(Action::SdCard(crate::sdcard::Job::Empty));
                    ui.close();
                }
            }
            ui.label(egui::RichText::new("A running Leapster 2 restarts with the new card.").weak());
        });
        if ui
            .checkbox(&mut settings.auto_calibrate, "Calibrate automatically")
            .on_hover_text("Ribbit presses the calibration targets for you when the calibration screen appears.")
            .changed()
        {
            actions.push(Action::SettingsChanged);
        }
        ui.separator();
        // The original LeapPad's own items.
        sub_menu(ui, "LeapPad (1999)", |ui| {
            if ui
                .button("Turn on (built-in book)")
                .on_hover_text("The pad with no cartridge. Needs its base ROM: open it with Open BIOS…")
                .clicked()
            {
                actions.push(Action::StartLeapPad);
                ui.close();
            }
            if ui
                .add_enabled(info.leappad, egui::Button::new("Open book…"))
                .on_hover_text("A virtual book file (docs/leappad-books.md)")
                .clicked()
            {
                actions.push(Action::Pick(Pick::LeapPadBook));
                ui.close();
            }
            ui.checkbox(&mut settings.leappad_words, "Show the books' words").on_hover_text(
                "Cartridges hold no text: Ribbit finds a book's printed words by listening to its spots on this PC (a speech model, downloaded once), in the background, the page you are on first. Off: the word boxes stay empty. From the next book opened.",
            );
            ui.checkbox(&mut settings.leappad_auto_go, "Touch GO automatically").on_hover_text(
                "On a real LeapPad the child touches the green GO circle after opening the book and after each page turn, so the pad knows the page. With this on, Ribbit's pen does it: on the first page once the pad has started, and whenever the page arrows turn the page.",
            );
            if ui
                .checkbox(&mut settings.leappad_classic, "Play LeapPad Classic cartridges")
                .on_hover_text(
                    "The first LeapPad books (1999-2001) bring their own software. The 2 MB Full Base ROM lets them \
                     in by itself. The 1 MB ILA2 base ROM's own list lets none in (\"LeapPad Classic not supported \
                     on this base\"); on it Ribbit lets them in. Off: refused, as by the ILA2 base (original). From \
                     the next start of the pad.",
                )
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
            ui.label(egui::RichText::new("Open a LeapPad cartridge with Open Cartridge…").weak());
        });
        // The RockIt Twist's own items, in a menu of their own.
        sub_menu(ui, "RockIt Twist", |ui| {
            if ui
                .add(egui::Button::new("Turn on (early emulation)"))
                .on_hover_text(ribbit_twist::STATUS)
                .clicked()
            {
                actions.push(Action::OpenTwist);
                ui.close();
            }
            if ui
                .button("Import games (.zip)…")
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
                    Some(n) => format!("Content: {n}…"),
                    None => "Content folder…".to_string(),
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
            if content.is_some() && ui.button("Remove content").clicked() {
                actions.push(Action::SetTwistContent(None));
                ui.close();
            }
            let usb = settings.twist_usb;
            if ui
                .button(if usb {
                    // Green while plugged in, red while not.
                    egui::RichText::new("🔌  Unplug from this PC (USB)").color(egui::Color32::from_rgb(80, 200, 110))
                } else {
                    egui::RichText::new("🔌  Plug into this PC (USB)").color(egui::Color32::from_rgb(235, 90, 90))
                })
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
        });
        ui.separator();
        sub_menu(ui, "Advanced (experimental)", |ui| {
            ui.label(egui::RichText::new("Unverified guesses. Apply at the next start.").weak());
            if ui
                .checkbox(&mut settings.mame_gpio_lines, "MAME power-line workaround")
                .on_hover_text(
                    "Reads every unidentified GPIO line as 1, as MAME does. The v1.5 (Universal) BIOS otherwise sees a shutdown request on line 11 and turns itself off. Not known to match hardware.",
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
            ui.separator();
            if ui
                .checkbox(&mut settings.jit_enabled, "JIT compiler")
                .on_hover_text("Translates Leapster code into this device's own machine code (x86-64 or 64-bit ARM) after it has run 32 times: much faster, same results. Turn it off if a game misbehaves only with it on. Applies when the next game starts.")
                .changed()
            {
                actions.push(Action::SettingsChanged);
            }
        });
    });
    top_menu(ui, rows, "⌨", "Input", |ui| {
        game_only_note(ui, info);
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
    top_menu(ui, rows, "🖥", "Video", |ui| {
        game_only_note(ui, info);
        if ui
            .checkbox(&mut settings.hd_vectors, "HD vectors (experimental)")
            .on_hover_text("Redraws the game's Flash art at your window's full resolution from the original vector data: shapes from the movies' own coordinates, text from the fonts' own outlines, bitmaps from their own pixels. Art with no vector original shows its pixels (see HD pixel art); never AI-upscaled. Replaces the scaling options below while on. Original Leapster and L-MAX BIOSes, and cartridges with their own Flash player.")
            .changed()
        {
            actions.push(Action::HdChanged);
        }
        // Art with no vector original: its pixels, or traced.
        ui.add_enabled_ui(settings.hd_vectors, |ui| {
            sub_menu(ui, "HD pixel art", |ui| {
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
        sub_menu(ui, "AI upscale settings", |ui| {
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
        sub_menu(ui, "Window size", |ui| {
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
        ui.separator();
        if ui.add_enabled(info.running, egui::Button::new("Screenshot").shortcut_text("F12")).clicked() {
            actions.push(Action::Screenshot);
            ui.close();
        }
        {
            let label = match &info.recording {
                Some(secs) => {
                    let m = secs / 60;
                    let s = secs % 60;
                    egui::RichText::new(format!("⏹  Stop recording  ({m}:{s:02})"))
                        .color(egui::Color32::from_rgb(220, 60, 60))
                }
                None => egui::RichText::new("⏺  Record…").into(),
            };
            if info.encoding {
                ui.add_enabled(false, egui::Button::new("⏳  Encoding…"));
            } else if ui.add_enabled(info.running || info.recording.is_some(), egui::Button::new(label).shortcut_text("F8")).clicked() {
                actions.push(Action::ToggleRecord);
                ui.close();
            }
            if ui.button("📂  Open recordings folder")
                .on_hover_text(info.record_dir.display().to_string())
                .clicked()
            {
                actions.push(Action::OpenRecordFolder);
                ui.close();
            }
            sub_menu(ui, "Recording settings", |ui| {
                ui.label(egui::RichText::new("Encoder").strong()).on_hover_text(
                    "These need FFmpeg on your PATH. Without it, Windows saves recordings as H.264 (.mp4) \
                     with its own encoder; other systems save uncompressed .avi.",
                );
                for enc in [RecordEncoder::H264, RecordEncoder::H265, RecordEncoder::Vp9, RecordEncoder::Lossless] {
                    if ui.radio_value(&mut settings.record_encoder, enc, enc.label()).clicked() {
                        actions.push(Action::SettingsChanged);
                    }
                }
                ui.separator();
                let default = if settings.record_folder.is_some() { "" } else { " (default)" };
                ui.label(egui::RichText::new(format!("Saved in{default}")).strong());
                ui.label(egui::RichText::new(info.record_dir.display().to_string()).weak());
                if ui.button("📂  Open folder").clicked() {
                    actions.push(Action::OpenRecordFolder);
                    ui.close();
                }
                if ui.button("Change folder…").clicked() {
                    actions.push(Action::PickRecordFolder);
                    ui.close();
                }
                if settings.record_folder.is_some() && ui.button("Use the Videos folder").clicked() {
                    actions.push(Action::ResetRecordFolder);
                    ui.close();
                }
            });
        }
    });
    top_menu(ui, rows, "🔊", "Audio", |ui| {
        game_only_note(ui, info);
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
    top_menu(ui, rows, "🔍", "Cheats", |ui| {
        if ui.add_enabled(info.running, egui::Button::new("Memory Search…")).clicked() {
            st.show_cheat = true;
            ui.close();
        }
    });
    top_menu(ui, rows, "🐞", "Debug", |ui| {
        if ui
            .add_enabled(info.running, egui::Button::new("Memory viewer…"))
            .on_hover_text("Scroll through the console's memory and change bytes, like a hex editor")
            .clicked()
        {
            st.debug.show_memory = true;
            ui.close();
        }
        if ui
            .add_enabled(info.running, egui::Button::new("Debugger…"))
            .on_hover_text("The CPU's registers and code; pause, step and breakpoints")
            .clicked()
        {
            st.debug.show_cpu = true;
            ui.close();
        }
    });
    top_menu(ui, rows, "❓", "Help", |ui| {
        if item(ui, "User Guide", "") {
            st.manual.open_at(None);
        }
        ui.separator();
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
        sub_menu(ui, "Patch notes", |ui| patch_notes_menu(ui, st));
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
/// Said at the top of the menus a game can have its own settings in.
fn game_only_note(ui: &mut egui::Ui, info: &Info) {
    if info.game_settings == Some(true) {
        ui.label(egui::RichText::new("For this game only").color(ACCENT).small());
        ui.separator();
    }
}

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
    // The side menu: the row folds its items open below it, inside the
    // menu's own scrolling list (a pop-up beside it could not be swiped or
    // scrolled, and could open off the screen).
    let open = fold_row(ui, &format!("{icon}    {label}"), 17.0, 48.0);
    if open {
        egui::Frame::NONE.inner_margin(egui::Margin { left: 18, right: 0, top: 0, bottom: 8 }).show(ui, |ui| {
            // Sized for fingers, like the rest of the side menu.
            let style = ui.style_mut();
            style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(17.0));
            style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(16.0));
            style.spacing.button_padding = egui::vec2(12.0, 9.0);
            style.spacing.interact_size.y = 40.0;
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(IN_SIDE_MENU), true));
            add_contents(ui);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new(IN_SIDE_MENU), false));
        });
    }
}

/// Set while the side menu draws a menu's items: its submenus fold too.
const IN_SIDE_MENU: &str = "drawing the side menu's items";

/// A full-width row that folds open (its state kept by its label); true
/// while open.
fn fold_row(ui: &mut egui::Ui, label: &str, size: f32, height: f32) -> bool {
    let id = ui.id().with(("fold", label));
    let mut open = ui.ctx().data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let text = egui::RichText::new(format!("{label}  {}", if open { "⏷" } else { "⏵" })).size(size).color(egui::Color32::from_gray(230));
    let button = egui::Button::new(text).frame(false).min_size(egui::vec2(ui.available_width(), height));
    if ui.add(button).clicked() {
        open = !open;
        ui.ctx().data_mut(|d| d.insert_temp(id, open));
    }
    open
}

/// A submenu: a pop-up from the menu bar, folded open in the side menu.
fn sub_menu(ui: &mut egui::Ui, label: &str, add_contents: impl FnOnce(&mut egui::Ui)) {
    if ui.ctx().data(|d| d.get_temp::<bool>(egui::Id::new(IN_SIDE_MENU))).unwrap_or(false) {
        if fold_row(ui, label, 16.0, 40.0) {
            egui::Frame::NONE.inner_margin(egui::Margin { left: 14, right: 0, top: 0, bottom: 4 }).show(ui, add_contents);
        }
    } else {
        ui.menu_button(label, add_contents);
    }
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
                                if let Ok(asset) = &package {
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
                            match &package {
                                Ok(_) => {
                                    let note = "Ribbit restarts by itself when the update is in. A game you're \
                                                playing closes first; its saves are kept.";
                                    ui.label(egui::RichText::new(note).weak());
                                    if install != Install::Idle && ui.link("Download it from GitHub instead").clicked() {
                                        ctx.open_url(egui::OpenUrl::new_tab(&r.url));
                                    }
                                }
                                // Said, so a player knows why it's the browser.
                                Err(why) => {
                                    ui.label(egui::RichText::new(format!("Ribbit can't update itself here: {why}.")).weak());
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

fn status_bar(ui: &mut egui::Ui, info: &Info) {
    ui.horizontal(|ui| {
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
        // On a phone, a quick start: the AI note on a page of its own first
        // (its card is the page's title: all of it and Continue fit a phone
        // held sideways), then the BIOS and games.
        if phone && !settings.ai_notice_seen {
            ai_notice(ui);
            ui.add_space(12.0);
            let text = egui::RichText::new("Continue").size(18.0).color(egui::Color32::WHITE);
            let button = egui::Button::new(text).fill(ACCENT).corner_radius(10).min_size(egui::vec2(240.0, 48.0));
            if ui.add(button).clicked() {
                settings.ai_notice_seen = true;
                actions.push(Action::SettingsChanged);
            }
            ui.add_space(12.0);
            return;
        }
        ui.heading(egui::RichText::new("🐸 Ribbit").size(34.0));
        ui.label("An emulator for LeapFrog's handhelds and TV consoles");
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
    // Wider on a phone held sideways: fewer lines, so Continue fits under it.
    let width = ui.available_width().min(if cfg!(target_os = "android") { 760.0 } else { 560.0 });
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
const STAR_YELLOW: egui::Color32 = egui::Color32::from_rgb(255, 210, 50);

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
    let LibraryView { books, tab, search, pull, pull_armed, favorites_only, open_folder } = view;
    // The Books side only when there are books; only it when there is
    // nothing else.
    let has_books = library.games.iter().any(|g| g.family.is_book());
    let has_games = library.games.iter().any(|g| !g.family.is_book());
    if !has_books {
        *books = false;
    } else if !has_games {
        *books = true;
    }
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
    let margin = if phone { egui::Margin::symmetric(14, 10) } else { egui::Margin { left: 28, right: 0, top: 20, bottom: 20 } };
    let (card_width, card_gap) = if phone { (150.0, 12.0) } else { (CARD_WIDTH, CARD_GAP) };
    egui::Frame::NONE.inner_margin(margin).show(ui, |ui| {
        // On a phone the header scrolls away with the games (it would take
        // half of a landscape screen).
        if (!phone || library.games.is_empty())
            && let Some(b) = library_header(ui, settings, library, *tab, (*books, has_books && has_games), actions)
        {
            switch_side(b, books, tab, favorites_only, open_folder, actions);
        }
        if library.games.is_empty() && !library.scanning {
            empty_library(ui, settings, actions);
            return;
        }
        ui.style_mut().spacing.scroll.floating = false;
        let out = egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .show(ui, |ui| {
            if phone {
                pull_to_refresh_hint(ui, *pull);
                if let Some(b) = library_header(ui, settings, library, *tab, (*books, has_books && has_games), actions) {
                    switch_side(b, books, tab, favorites_only, open_folder, actions);
                }
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
            let on_side = |g: &&crate::library::Game| g.family.is_book() == *books;
            if let Some(game) = last.and_then(|p| library.games.iter().find(|g| &g.path == p)).filter(on_side).filter(|_| !phone) {
                hero(ui, game, actions);
                ui.add_space(28.0);
            }
            console_tabs(ui, settings, library, *books, tab, favorites_only, open_folder, actions);
            ui.add_space(14.0);
            let query = search.trim().to_lowercase();
            let matches = |g: &crate::library::Game| {
                query.is_empty() || g.title.to_lowercase().contains(&query) || g.subtitle.to_lowercase().contains(&query)
            };
            let tab_name = tab.map_or("all", |f| f.name());
            let shown_games: Vec<&crate::library::Game> = library.games.iter().filter(|g| {
                if g.family.is_book() != *books { return false; }
                if !tab.is_none_or(|f| f.lists(g.family)) { return false; }
                if !matches(g) { return false; }
                if *favorites_only && !settings.favorites.contains(&g.path) { return false; }
                if g.has_tag("Internal") && settings.hide_builtin_tabs.contains(tab_name) { return false; }
                // Hide games that belong to a folder from the main list (show them inside the folder).
                if open_folder.is_none() && !*favorites_only && settings.folders.iter().any(|f| f.games.contains(&g.path)) { return false; }
                true
            }).collect();
            let heading = if *favorites_only {
                "FAVORITES".to_string()
            } else {
                match tab {
                    None if *books => "ALL BOOKS".to_string(),
                    None => "ALL GAMES".to_string(),
                    Some(f) => f.name().to_uppercase(),
                }
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
                        .hint_text(if *books { "🔍  Search books" } else { "🔍  Search games" })
                        .desired_width(if phone { 180.0 } else { 240.0 })
                        .margin(egui::vec2(10.0, 6.0));
                    let resp = ui.add(field);
                    if resp.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                        search.clear();
                    }
                    // Built-in toggle: only when the current tab has Internal games.
                    let tab_has_builtins = library.games.iter().any(|g| {
                        g.has_tag("Internal") && g.family.is_book() == *books && tab.is_none_or(|f| f.lists(g.family))
                    });
                    if tab_has_builtins && !*favorites_only {
                        let hidden = settings.hide_builtin_tabs.contains(tab_name);
                        let mut shown = !hidden;
                        if ui.checkbox(&mut shown, egui::RichText::new("Built-in").size(11.0)).changed() {
                            actions.push(Action::ToggleBuiltinGames(*tab));
                        }
                    }
                });
            });
            ui.add_space(12.0);
            // Folder cards (skip when inside an open folder or showing favorites).
            if open_folder.is_none() && !*favorites_only && !settings.folders.is_empty() {
                let avail = ui.available_width();
                let fcols = (((avail + card_gap) / (card_width + card_gap)).floor() as usize).max(1);
                let fc_w = ((avail - card_gap * (fcols - 1) as f32) / fcols as f32).floor().min(card_width * 1.4);
                // Only show folder cards that have at least one game visible on the current tab.
                let visible_folders: Vec<(usize, String, usize)> = settings.folders.iter().enumerate()
                    .filter_map(|(i, f)| {
                        let count = f.games.iter()
                            .filter(|p| {
                                library.games.iter().any(|g| &g.path == *p && g.family.is_book() == *books && tab.is_none_or(|t| t.lists(g.family)))
                            })
                            .count();
                        if count > 0 { Some((i, f.name.clone(), count)) } else { None }
                    })
                    .collect();
                for row in visible_folders.chunks(fcols) {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = card_gap;
                        for &(idx, ref name, game_count) in row {
                            let art_paths: Vec<std::path::PathBuf> = settings.folders[idx].games.iter().take(4).cloned().collect();
                            let art_games: Vec<&crate::library::Game> = art_paths.iter()
                                .filter_map(|p| library.games.iter().find(|g| &g.path == p))
                                .collect();
                            folder_card(ui, idx, name, game_count, &art_games, fc_w, actions);
                        }
                    });
                    ui.add_space(card_gap);
                }
                ui.add_space(4.0);
            }
            if shown_games.is_empty() && !query.is_empty() && open_folder.is_none() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(egui::RichText::new(format!("No games match \"{}\"", search.trim())).size(16.0).weak());
                });
            }
            let shown = since.elapsed().as_secs_f32();
            if let Some(folder_idx) = open_folder {
                if let Some(folder) = settings.folders.get(*folder_idx) {
                    let folder_games: Vec<&crate::library::Game> = shown_games.iter().copied()
                        .filter(|g| folder.games.contains(&g.path))
                        .collect();
                    let visible_count = folder_games.len();
                    egui::Frame::NONE
                        .fill(egui::Color32::from_rgb(15, 32, 18))
                        .stroke(egui::Stroke::new(1.5, ACCENT.gamma_multiply(0.5)))
                        .corner_radius(12)
                        .inner_margin(egui::Margin::same(14))
                        .show(ui, |ui| {
                            // Header row: back button + folder name.
                            ui.horizontal(|ui| {
                                let back = ui.add(
                                    egui::Button::new(
                                        egui::RichText::new("< Back")
                                            .size(13.0)
                                            .color(egui::Color32::WHITE)
                                            .strong()
                                    )
                                    .fill(ACCENT)
                                    .corner_radius(6.0)
                                ).on_hover_cursor(egui::CursorIcon::PointingHand);
                                if back.clicked() { actions.push(Action::CloseLibraryFolder); }
                                ui.label(
                                    egui::RichText::new(format!("{}  ({})", folder.name, visible_count))
                                        .size(14.0)
                                        .strong()
                                        .color(egui::Color32::WHITE)
                                );
                            });
                            ui.add_space(10.0);
                            if folder_games.is_empty() {
                                ui.vertical_centered(|ui| {
                                    ui.add_space(20.0);
                                    ui.label(egui::RichText::new("No games here yet").weak().size(15.0));
                                });
                            } else {
                                let avail = ui.available_width();
                                let cols = (((avail + card_gap) / (card_width + card_gap)).floor() as usize).max(1);
                                let card_w = ((avail - card_gap * (cols - 1) as f32) / cols as f32).floor().min(card_width * 1.4);
                                for (r, row) in folder_games.chunks(cols).enumerate() {
                                    ui.horizontal(|ui| {
                                        ui.spacing_mut().item_spacing.x = card_gap;
                                        for (c, game) in row.iter().enumerate() {
                                            let n = (r * cols + c).min(CARD_STAGGER_MAX);
                                            let t = ((shown - CARD_STAGGER * n as f32) / CARD_GROW).clamp(0.0, 1.0);
                                            if t < 1.0 { ui.ctx().request_repaint(); }
                                            let is_fav = settings.favorites.contains(&game.path);
                                            game_card(ui, game, card_w, 1.0 - (1.0 - t).powi(3), is_fav, true, &settings.folders, actions);
                                        }
                                    });
                                    ui.add_space(card_gap);
                                }
                            }
                        });
                }
            } else {
                let avail = ui.available_width();
                let cols = (((avail + card_gap) / (card_width + card_gap)).floor() as usize).max(1);
                let card_w = ((avail - card_gap * (cols - 1) as f32) / cols as f32).floor().min(card_width * 1.4);
                for (r, row) in shown_games.chunks(cols).enumerate() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = card_gap;
                        for (c, game) in row.iter().enumerate() {
                            let n = (r * cols + c).min(CARD_STAGGER_MAX);
                            let t = ((shown - CARD_STAGGER * n as f32) / CARD_GROW).clamp(0.0, 1.0);
                            if t < 1.0 { ui.ctx().request_repaint(); }
                            let is_fav = settings.favorites.contains(&game.path);
                            game_card(ui, game, card_w, 1.0 - (1.0 - t).powi(3), is_fav, false, &settings.folders, actions);
                        }
                    });
                    ui.add_space(card_gap);
                }
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

/// Turns to the library's Games or Books side, on its first page.
fn switch_side(to_books: bool, books: &mut bool, tab: &mut Option<Family>, favorites_only: &mut bool, open_folder: &Option<usize>, actions: &mut Vec<Action>) {
    *books = to_books;
    *tab = None;
    *favorites_only = false;
    if open_folder.is_some() {
        actions.push(Action::CloseLibraryFolder);
    }
}

/// The library's title: "Games", or the Games | Books switch when there
/// are both (`side`: on the Books side, whether to offer the switch).
/// Returns the side clicked.
fn shelf_title(ui: &mut egui::Ui, size: f32, side: (bool, bool)) -> Option<bool> {
    let (books, offer) = side;
    if !offer {
        ui.label(egui::RichText::new("Games").size(size).family(crate::fonts::title()).color(egui::Color32::WHITE));
        return None;
    }
    let mut clicked = None;
    for (name, is_books) in [("Games", false), ("Books", true)] {
        let on = books == is_books;
        // One size for both (only the colour says which is open): the
        // title keeps its width, and the buttons beside it stay put.
        let text = egui::RichText::new(name).size(size).family(crate::fonts::title());
        let r = ui
            .add(egui::Label::new(text.color(if on { egui::Color32::WHITE } else { egui::Color32::from_gray(120) })).sense(egui::Sense::click()))
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if r.clicked() && !on {
            clicked = Some(is_books);
        }
        if !is_books {
            ui.label(egui::RichText::new("|").size(size * 0.7).color(egui::Color32::from_gray(80)));
        }
    }
    clicked
}

fn library_header(
    ui: &mut egui::Ui,
    settings: &Settings,
    library: &Library,
    tab: Option<Family>,
    side: (bool, bool),
    actions: &mut Vec<Action>,
) -> Option<bool> {
    // All books: the header's buttons are the book reader's (the LeapPad,
    // the only one).
    let tab = tab.or(side.0.then_some(Family::LeapPad));
    let phone = cfg!(target_os = "android");
    if phone {
        return phone_library_header(ui, settings, library, tab, side, actions);
    }
    let mut switch_to = None;
    let header_id = egui::Id::new("library header controls width");
    let mut wrapped = true;
    ui.horizontal(|ui| {
        let frog = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("frog icon"))).unwrap_or_else(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied([256, 256], &crate::icon::frog_glyph_rgba(256));
            let tex = ui.ctx().load_texture("frog icon", img, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("frog icon"), tex.clone()));
            tex
        });
        ui.add(egui::Image::new((frog.id(), egui::vec2(42.0, 42.0))));
        ui.add_space(4.0);
        switch_to = shelf_title(ui, 34.0, side);
        // The buttons beside the title when they fit (their width from the
        // last frame), else on a row of their own under it.
        if ui.available_width() >= ui.ctx().data(|d| d.get_temp::<f32>(header_id).unwrap_or(0.0)) {
            header_controls(ui, settings, library, tab, actions, header_id);
            wrapped = false;
        }
    });
    if wrapped {
        ui.add_space(8.0);
        ui.horizontal(|ui| header_controls(ui, settings, library, tab, actions, header_id));
    }
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
    switch_to
}

/// The library header's buttons, right-aligned: the console and its BIOS,
/// power, Open Cartridge, Add games folder, rescan. Their width is kept
/// under `width_id` for the header to know whether they fit beside the title.
fn header_controls(ui: &mut egui::Ui, settings: &Settings, library: &Library, tab: Option<Family>, actions: &mut Vec<Action>, width_id: egui::Id) {
    let inner = ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        // Clear of the window's edge (the library has no right margin, for
        // its scroll bar), lined up with the cards' right side.
        ui.add_space(20.0);
        if ui
            .add(egui::Button::new(egui::RichText::new("⟳").size(16.0)).corner_radius(8).min_size(egui::vec2(36.0, 32.0)))
            .on_hover_text("Look for new games in your folders")
            .clicked()
        {
            actions.push(Action::RescanLibrary);
        }
        games_folder_button(ui, settings, actions);
        if ui.add(egui::Button::new("Open Cartridge…").corner_radius(8).min_size(egui::vec2(0.0, 32.0))).clicked() {
            actions.push(Action::Pick(Pick::Cartridge));
        }
        // Next to Open Cartridge: the console chosen on the left, then on
        // with a game or without.
        turn_on_button(ui, settings, library, tab, actions);
        // Zippity and RockIt Twist games play on their own console: no
        // choice to make on their pages.
        if !matches!(tab, Some(Family::Zippity | Family::RockItTwist | Family::LeapPad)) {
            // (Right to left: the BIOS button shows to the right of the
            // console menu.)
            bios_picker(ui, settings, library, actions);
            console_picker(ui, settings, library, tab, actions);
        }
        ui.min_rect().width()
    });
    // Room to spare from the title, too. The widest they have been: the
    // Books side has no console or BIOS menu, and its narrower buttons
    // fitting beside the title where the Games side's don't moved them
    // between rows when switching.
    let width = inner.inner + 24.0;
    ui.ctx().data_mut(|d| {
        let widest = d.get_temp::<f32>(width_id).unwrap_or(0.0).max(width);
        d.insert_temp(width_id, widest);
    });
}

/// The library's header on a phone: the title and progress, then the
/// console and BIOS on a row of their own (adding games is in the side menu).
fn phone_library_header(
    ui: &mut egui::Ui,
    settings: &Settings,
    library: &Library,
    tab: Option<Family>,
    side: (bool, bool),
    actions: &mut Vec<Action>,
) -> Option<bool> {
    let mut switch_to = None;
    ui.horizontal(|ui| {
        let frog = ui.ctx().data_mut(|d| d.get_temp::<egui::TextureHandle>(egui::Id::new("frog icon"))).unwrap_or_else(|| {
            let img = egui::ColorImage::from_rgba_unmultiplied([256, 256], &crate::icon::frog_glyph_rgba(256));
            let tex = ui.ctx().load_texture("frog icon", img, egui::TextureOptions::LINEAR);
            ui.ctx().data_mut(|d| d.insert_temp(egui::Id::new("frog icon"), tex.clone()));
            tex
        });
        ui.add(egui::Image::new((frog.id(), egui::vec2(30.0, 30.0))));
        switch_to = shelf_title(ui, 26.0, side);
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
            // Hamburger: opens the side menu.
            if ui
                .add(egui::Button::new(egui::RichText::new("☰").size(20.0)).corner_radius(8).min_size(egui::vec2(44.0, 40.0)))
                .clicked()
            {
                actions.push(Action::ToggleDrawer);
            }
        });
    });
    ui.add_space(6.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        if !matches!(tab, Some(Family::Zippity | Family::RockItTwist | Family::LeapPad)) {
            console_picker(ui, settings, library, tab, actions);
            bios_picker(ui, settings, library, actions);
        }
        turn_on_button(ui, settings, library, tab, actions);
    });
    ui.add_space(8.0);
    switch_to
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
            .selected_text(format!("💾  BIOS: {}", current.short_name()))
            .show_ui(ui, |ui| {
                ui.label(egui::RichText::new(format!("Select a {} BIOS", console.name())).weak());
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

/// "Add games folder" until there is one; then "Open games folder", which
/// shows it in the file manager (with several, a menu of them). More are
/// added from File › Games Folders.
fn games_folder_button(ui: &mut egui::Ui, settings: &Settings, actions: &mut Vec<Action>) {
    let folders = &settings.rom_folders;
    if folders.is_empty() {
        if primary_button(ui, "➕  Add games folder", egui::vec2(0.0, 32.0)).clicked() {
            actions.push(Action::Pick(Pick::RomFolder));
        }
        return;
    }
    let r = primary_button(ui, "📂  Open games folder", egui::vec2(0.0, 32.0));
    if let [only] = folders.as_slice() {
        if r.on_hover_text(format!("{}
(Add more in File › Games Folders)", only.display())).clicked() {
            actions.push(Action::OpenFolder(only.clone()));
        }
        return;
    }
    egui::Popup::menu(&r).show(|ui| {
        for f in folders {
            if ui.button(f.display().to_string()).clicked() {
                actions.push(Action::OpenFolder(f.clone()));
            }
        }
        ui.separator();
        if ui.button("Add games folder…").clicked() {
            actions.push(Action::Pick(Pick::RomFolder));
        }
    });
}

/// Turns on the page's console on its own, no game in: its start-up screens
/// and menu (the Leapster 2's main menu, the Zippity's built-in games, the
/// RockIt Twist's menu). On the Leapster page, the console chosen there.
fn turn_on_button(ui: &mut egui::Ui, settings: &Settings, library: &Library, tab: Option<Family>, actions: &mut Vec<Action>) {
    // The LeapPad's page: the pad alone runs its built-in book.
    if tab == Some(Family::LeapPad) {
        if settings.leappad_bios.is_some() && power_button(ui, "Turn on the LeapPad without a cartridge (its built-in book)") {
            actions.push(Action::StartLeapPad);
        }
        return;
    }
    let console = match tab {
        Some(Family::Zippity) => Hardware::Zippity,
        Some(Family::RockItTwist) => Hardware::RockItTwist,
        _ => library.selected_console(&settings.library_console),
    };
    // The RockIt Twist starts from its system packages (it says which are
    // missing); the others need their BIOS.
    let can = console == Hardware::RockItTwist || library.bios_for(console).is_some();
    if !can || !tab.is_none_or(|f| f.runs_on(console)) {
        return;
    }
    if power_button(ui, "Boot without cartridge") {
        actions.push(if console == Hardware::RockItTwist { Action::OpenTwist } else { Action::StartSystem(console) });
    }
}

/// A power button, the size of the rescan button; whether it was clicked.
fn power_button(ui: &mut egui::Ui, hover: &str) -> bool {
    // The symbol is drawn: the fonts may not have it.
    let r = ui.add(egui::Button::new("").corner_radius(8).min_size(egui::vec2(36.0, 32.0))).on_hover_text(hover);
    let color = ui.style().interact(&r).fg_stroke.color;
    let stroke = egui::Stroke::new(2.2, color);
    let c = r.rect.center() + egui::vec2(0.0, 1.0);
    let radius = 7.5;
    // A ring open at the top (clockwise from 1 o'clock round to 11, angles
    // from 12 o'clock), and the bar through the opening.
    let ring: Vec<egui::Pos2> = (0..=32)
        .map(|i| {
            let a = (40.0f32 + 280.0 * i as f32 / 32.0).to_radians();
            c + egui::vec2(a.sin(), -a.cos()) * radius
        })
        .collect();
    ui.painter().add(egui::Shape::line(ring, stroke));
    ui.painter().line_segment([c + egui::vec2(0.0, -radius - 2.0), c + egui::vec2(0.0, -2.0)], stroke);
    r.clicked()
}

/// A green button: at least `min`, else its text plus padding. Drawn here so
/// its size never depends on hover (egui's button grew or shrank with the
/// state's outline) and its hover colour comes from its own response (a
/// fixed-size one never lit up before).
fn primary_button(ui: &mut egui::Ui, text: &str, min: egui::Vec2) -> egui::Response {
    primary_button_text(ui, egui::RichText::new(text).strong().into(), min)
}

/// `primary_button` with text laid out by the caller (mixed sizes).
fn primary_button_text(ui: &mut egui::Ui, text: egui::WidgetText, min: egui::Vec2) -> egui::Response {
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let pad = ui.spacing().button_padding;
    let size = (galley.size() + 2.0 * pad).max(min);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() || response.has_focus() { ACCENT_HOVER } else { ACCENT };
        ui.painter().rect_filled(rect, 8, fill);
        let at = rect.center() - galley.size() / 2.0;
        ui.painter().galley(at, galley, egui::Color32::WHITE);
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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
        _ if game.family == crate::library::Family::LeapPad => paint_book_cover(p, rect, radius, &game.title),
        _ if game.family == crate::library::Family::RockItTwist => paint_twist_card(p, rect, radius, &game.title),
        other => {
            p.rect_filled(rect, radius, egui::Color32::from_rgb(24, 30, 27));
            let note = if matches!(other, Art::Pending) { "Getting title screen…" } else { "🎮" };
            let font = egui::FontId::proportional(if matches!(other, Art::Pending) { 14.0 } else { 40.0 });
            p.text(rect.center(), egui::Align2::CENTER_CENTER, note, font, egui::Color32::from_gray(120));
        }
    }
}

/// A LeapPad book's card (the cartridge has no picture of its own): a
/// cover in a colour of its own, a ring binding down the left, the series as
/// a tag, the book's name, and a pen-and-page mark.
fn paint_book_cover(p: &egui::Painter, rect: egui::Rect, radius: u8, title: &str) {
    const COVERS: [(u8, u8, u8); 8] =
        [(40, 150, 150), (220, 120, 70), (90, 140, 210), (150, 100, 190), (70, 160, 90), (210, 90, 120), (200, 160, 50), (60, 120, 160)];
    let (series, name) = crate::leappad::book_series(title);
    let hash = card_hash(title);
    let (r, g, b) = COVERS[hash as usize % COVERS.len()];
    let cover = egui::Color32::from_rgb(r, g, b);
    p.rect_filled(rect, radius, cover);
    // A lighter band at the top, as on the books' covers.
    let band = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), rect.height() * 0.22));
    p.rect_filled(band, egui::CornerRadius { nw: radius, ne: radius, sw: 0, se: 0 }, egui::Color32::from_white_alpha(40));
    // The binding.
    let spine = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width() * 0.09, rect.height()));
    p.rect_filled(spine, egui::CornerRadius { nw: radius, sw: radius, ne: 0, se: 0 }, egui::Color32::from_black_alpha(50));
    let rings = 7;
    for i in 0..rings {
        let y = rect.top() + rect.height() * (i as f32 + 0.5) / rings as f32;
        p.circle_stroke(egui::pos2(spine.right(), y), rect.width() * 0.025, egui::Stroke::new(1.5, egui::Color32::from_gray(225)));
    }
    let left = spine.right() + rect.width() * 0.07;
    let width = rect.right() - left - rect.width() * 0.06;
    let size = rect.height() / 10.0;
    if let Some(series) = series {
        let galley = p.layout(series.to_uppercase(), egui::FontId::proportional(size * 0.55), egui::Color32::WHITE, width);
        p.galley(egui::pos2(left, band.center().y - galley.size().y / 2.0), galley, egui::Color32::WHITE);
    }
    let name = name.rsplit_once(" (").map_or(name.as_str(), |(n, _)| n).to_string();
    let galley = p.layout(name, egui::FontId::new(size, crate::fonts::title()), egui::Color32::WHITE, width);
    let top = band.bottom() + rect.height() * 0.08;
    p.galley(egui::pos2(left, top), galley, egui::Color32::WHITE);
    // The pen and page, bottom right.
    let page = egui::Rect::from_center_size(rect.right_bottom() - egui::vec2(rect.width() * 0.2, rect.height() * 0.15), egui::vec2(rect.width() * 0.18, rect.height() * 0.14));
    p.rect_filled(page, 3.0, egui::Color32::from_rgb(255, 250, 236));
    for k in 0..3 {
        let y = page.top() + page.height() * (0.3 + 0.2 * k as f32);
        p.line_segment([egui::pos2(page.left() + page.width() * 0.15, y), egui::pos2(page.right() - page.width() * 0.15, y)], egui::Stroke::new(1.5, cover));
    }
    let tip = page.left_top() + egui::vec2(page.width() * 0.35, page.height() * 0.55);
    let end = tip + egui::vec2(page.width() * 0.75, -page.height() * 0.95);
    p.line_segment([tip, end], egui::Stroke::new(rect.width() * 0.035, egui::Color32::from_rgb(40, 90, 200)));
    p.circle_filled(tip, rect.width() * 0.012, egui::Color32::from_gray(40));
}

/// A name's hash, to pick a card's colour.
fn card_hash(name: &str) -> u32 {
    let mut h = name.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193));
    // A finishing mix, as FNV's low bits alone repeat across similar names.
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^ (h >> 16)
}

/// A RockIt Twist package's card (game packs are encrypted, so there is no
/// picture to take): a colour of its own, a drawing of a twistable handheld,
/// and the game's name.
fn paint_twist_card(p: &egui::Painter, rect: egui::Rect, radius: u8, title: &str) {
    const COLOURS: [(u8, u8, u8); 6] = [(225, 70, 110), (60, 170, 220), (240, 160, 40), (120, 90, 210), (40, 180, 120), (230, 100, 50)];
    let hash = card_hash(title);
    let (r, g, b) = COLOURS[hash as usize % COLOURS.len()];
    let colour = egui::Color32::from_rgb(r, g, b);
    p.rect_filled(rect, radius, egui::Color32::from_rgb(28, 30, 40));
    let glow = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, rect.height() * 0.08), rect.size() * 0.8);
    p.rect_filled(glow, rect.width() * 0.4, colour.gamma_multiply(0.25));
    // The handheld: a rounded body with a screen in its middle and two
    // ridged grips that twist.
    let body = egui::Rect::from_center_size(rect.center() - egui::vec2(0.0, rect.height() * 0.1), egui::vec2(rect.width() * 0.72, rect.height() * 0.3));
    p.rect_filled(body, body.height() * 0.5, colour);
    let screen = egui::Rect::from_center_size(body.center(), egui::vec2(body.width() * 0.42, body.height() * 0.62));
    p.rect_filled(screen, 4.0, egui::Color32::from_rgb(20, 24, 30));
    p.rect_filled(screen.shrink(screen.height() * 0.12), 2.0, colour.gamma_multiply(0.45));
    for side in [-1.0f32, 1.0] {
        let x = body.center().x + side * body.width() * 0.36;
        for k in 0..4 {
            let dx = side * k as f32 * body.width() * 0.025;
            p.line_segment(
                [egui::pos2(x + dx, body.top() + body.height() * 0.2), egui::pos2(x + dx, body.bottom() - body.height() * 0.2)],
                egui::Stroke::new(2.0, egui::Color32::from_black_alpha(70)),
            );
        }
    }
    let width = rect.width() * 0.84;
    let tag = p.layout("ROCKIT TWIST".into(), egui::FontId::proportional(rect.height() * 0.05), colour, width);
    p.galley(egui::pos2(rect.center().x - tag.size().x / 2.0, rect.top() + rect.height() * 0.07), tag, colour);
    let name = title.rsplit_once(" (").map_or(title, |(n, _)| n).to_string();
    let galley = p.layout(name, egui::FontId::new(rect.height() / 11.0, crate::fonts::title()), egui::Color32::WHITE, width);
    let top = body.bottom() + rect.height() * 0.08;
    p.galley(egui::pos2(rect.center().x - galley.size().x / 2.0, top), galley, egui::Color32::WHITE);
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
                // The ▶ larger than the word (the user's request).
                let mut play = egui::text::LayoutJob::default();
                let style = egui::TextStyle::Button.resolve(ui.style());
                let format = |size: f32| egui::TextFormat {
                    font_id: egui::FontId::new(size, style.family.clone()),
                    color: egui::Color32::WHITE,
                    valign: egui::Align::Center,
                    ..Default::default()
                };
                play.append("▶", 0.0, format(style.size * 1.7));
                play.append("  Play", 0.0, egui::TextFormat { font_id: egui::FontId::new(style.size, style.family.clone()), ..format(0.0) });
                if primary_button_text(ui, play.into(), egui::vec2(180.0, 46.0)).clicked() {
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

/// A folder tile: 2×2 art thumbnails, folder name, open/delete.
fn folder_card(ui: &mut egui::Ui, idx: usize, name: &str, game_count: usize, art_games: &[&crate::library::Game], w: f32, actions: &mut Vec<Action>) {
    let pad = 10.0;
    let inner_w = w - 2.0 * pad;
    let label_h = 32.0;
    let h = pad + inner_w + 8.0 + label_h + pad;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click());
    let drag_over = response.dnd_hover_payload::<std::path::PathBuf>().is_some();
    if let Some(payload) = response.dnd_release_payload::<std::path::PathBuf>() {
        actions.push(Action::AddToFolder { folder: idx, game: (*payload).clone() });
    }
    let resp = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    let lift = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || drag_over, 0.12);
    let card = rect.translate(egui::vec2(0.0, -4.0 * lift));
    let p = ui.painter();
    let shadow = egui::epaint::Shadow {
        offset: [0, (4.0 + 6.0 * lift) as i8],
        blur: (12.0 + 14.0 * lift) as u8,
        spread: 0,
        color: egui::Color32::from_black_alpha(80 + (50.0 * lift) as u8),
    };
    p.add(shadow.as_shape(card, 14));
    p.rect_filled(card, 14, egui::Color32::from_rgb(28, 34, 28).lerp_to_gamma(egui::Color32::from_rgb(36, 44, 36), lift));
    let border_color = if drag_over {
        ACCENT.gamma_multiply(0.9)
    } else {
        egui::Color32::from_white_alpha(18 + (20.0 * lift) as u8)
    };
    p.rect_stroke(card, 14, egui::Stroke::new(if drag_over { 2.0 } else { 1.0 }, border_color), egui::StrokeKind::Inside);
    // 2×2 thumbnail grid inside the art area.
    let art_area = egui::Rect::from_min_size(card.min + egui::vec2(pad, pad), egui::vec2(inner_w, inner_w));
    let half = (inner_w - 4.0) / 2.0;
    let quads = [
        egui::Rect::from_min_size(art_area.min, egui::vec2(half, half)),
        egui::Rect::from_min_size(egui::pos2(art_area.min.x + half + 4.0, art_area.min.y), egui::vec2(half, half)),
        egui::Rect::from_min_size(egui::pos2(art_area.min.x, art_area.min.y + half + 4.0), egui::vec2(half, half)),
        egui::Rect::from_min_size(egui::pos2(art_area.min.x + half + 4.0, art_area.min.y + half + 4.0), egui::vec2(half, half)),
    ];
    for (i, quad) in quads.iter().enumerate() {
        if let Some(game) = art_games.get(i) {
            paint_art(ui, game, *quad, 8);
        } else {
            ui.painter().rect_filled(*quad, 8, egui::Color32::from_white_alpha(8));
        }
    }
    // Folder name + game count.
    let name_pos = egui::pos2(card.min.x + pad, art_area.max.y + 8.0);
    let label = format!("{name} ({game_count})");
    let mut job = egui::text::LayoutJob::simple(label, egui::FontId::proportional(14.0), egui::Color32::WHITE, inner_w - 24.0);
    job.wrap.max_rows = 1;
    let name_g = ui.painter().layout_job(job);
    ui.painter().galley(name_pos, name_g, egui::Color32::WHITE);
    // Delete button (×) at the right end.
    let del_rect = egui::Rect::from_min_size(
        egui::pos2(card.max.x - pad - 18.0, art_area.max.y + 6.0),
        egui::vec2(18.0, 20.0),
    );
    let del_id = resp.id.with("del");
    let del_resp = ui.interact(del_rect, del_id, egui::Sense::click());
    let del_color = if del_resp.hovered() { egui::Color32::from_rgb(235, 87, 80) } else { egui::Color32::from_gray(110) };
    ui.painter().text(del_rect.center(), egui::Align2::CENTER_CENTER, "×", egui::FontId::proportional(16.0), del_color);
    if del_resp.clicked() {
        actions.push(Action::DeleteFolder(idx));
    } else if resp.clicked() {
        actions.push(Action::OpenLibraryFolder(idx));
    }
    resp.context_menu(|ui| {
        if ui.button("Open folder").clicked() {
            actions.push(Action::OpenLibraryFolder(idx));
            ui.close();
        }
        if ui.button("Rename folder…").clicked() {
            actions.push(Action::RenameFolder { idx, name: String::new() });
            ui.close();
        }
        if ui.button(egui::RichText::new("Delete folder").color(egui::Color32::from_rgb(235, 87, 80))).clicked() {
            actions.push(Action::DeleteFolder(idx));
            ui.close();
        }
    });
}

/// `appear`: 0 to 1 as the card grows in (smaller and transparent at 0).
fn game_card(ui: &mut egui::Ui, game: &crate::library::Game, w: f32, appear: f32, is_fav: bool, in_folder: bool, folders: &[crate::settings::LibraryFolder], actions: &mut Vec<Action>) {
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
    let (rect, response) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::click_and_drag());
    response.dnd_set_drag_payload(game.path.clone());
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
    // Star button — top-right corner of the art area. Always shown as ☆
    // (dim) or ★ (accent); check the cursor/click position against the star
    // rect to avoid using ui.interact on a sub-rect of the card.
    let star_size = 28.0;
    let star_rect = egui::Rect::from_min_size(
        egui::pos2(art.max.x - star_size - 4.0, art.min.y + 4.0),
        egui::vec2(star_size, star_size),
    );
    let star_hovered = response.hovered() && response.hover_pos().map_or(false, |p| star_rect.contains(p));
    let star_clicked = response.clicked() && response.interact_pointer_pos().map_or(false, |p| star_rect.contains(p));
    let star_char = if is_fav { "★" } else { "☆" };
    let star_color = if is_fav {
        STAR_YELLOW
    } else if star_hovered {
        egui::Color32::from_white_alpha(210)
    } else {
        egui::Color32::from_white_alpha(55)
    };
    // Dark outline for cartoony look: draw slightly larger in near-black first.
    let star_font = egui::FontId::proportional(22.0);
    let outline_color = egui::Color32::from_black_alpha(160);
    for dx in [-1.0_f32, 1.0] {
        for dy in [-1.0_f32, 1.0] {
            ui.painter().text(
                star_rect.center() + egui::vec2(dx, dy),
                egui::Align2::CENTER_CENTER,
                star_char,
                star_font.clone(),
                outline_color,
            );
        }
    }
    if is_fav {
        // Gradient: bright yellow top half, warm amber bottom half.
        let top_color = if star_hovered { egui::Color32::from_rgb(255, 248, 160) } else { egui::Color32::from_rgb(255, 235, 100) };
        let bot_color = if star_hovered { egui::Color32::from_rgb(255, 175, 30) } else { egui::Color32::from_rgb(220, 140, 10) };
        let mid_y = star_rect.center().y;
        let top_clip = egui::Rect::from_min_max(star_rect.min - egui::vec2(4.0, 4.0), egui::pos2(star_rect.max.x + 4.0, mid_y));
        let bot_clip = egui::Rect::from_min_max(egui::pos2(star_rect.min.x - 4.0, mid_y), star_rect.max + egui::vec2(4.0, 4.0));
        ui.painter().with_clip_rect(top_clip).text(star_rect.center(), egui::Align2::CENTER_CENTER, star_char, star_font.clone(), top_color);
        ui.painter().with_clip_rect(bot_clip).text(star_rect.center(), egui::Align2::CENTER_CENTER, star_char, star_font, bot_color);
    } else {
        ui.painter().text(star_rect.center(), egui::Align2::CENTER_CENTER, star_char, star_font, star_color);
    }
    if star_clicked {
        actions.push(Action::ToggleFavorite(game.path.clone()));
    }
    let p = ui.painter();
    let at = egui::pos2(card.min.x + pad, art.max.y + 12.0);
    p.galley(at, title, egui::Color32::WHITE);
    let mut x = at.x;
    let y = at.y + two_lines + 6.0;
    for r in region.into_iter().chain(game.tags.iter().map(|t| t.as_ref())) {
        let g = p.layout_no_wrap(r.to_string(), egui::FontId::proportional(11.0), egui::Color32::from_gray(200));
        let b = egui::Rect::from_min_size(egui::pos2(x, y), g.size() + egui::vec2(12.0, 4.0));
        p.rect_filled(b, 5, egui::Color32::from_white_alpha(14));
        p.galley(b.min + egui::vec2(6.0, 2.0), g, egui::Color32::from_gray(200));
        x = b.max.x + 8.0;
    }
    // The rest of the line, cut short with … where it would run past the
    // card's edge.
    let room = card.right() - pad - x;
    if room > 12.0 {
        let mut job = egui::text::LayoutJob::simple_singleline(
            game.subtitle.clone(),
            egui::FontId::proportional(12.0),
            egui::Color32::from_gray(140),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        p.galley(egui::pos2(x, y + 1.0), p.layout_job(job), egui::Color32::from_gray(140));
    }
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
    let game_clicked = response.clicked() && !response.interact_pointer_pos().map_or(false, |p| star_rect.contains(p));
    if game_clicked {
        actions.push(Action::PlayFromLibrary(game.path.clone()));
    }
    let game_path = game.path.clone();
    response.context_menu(|ui| {
        if ui.button("Play").clicked() {
            actions.push(Action::PlayFromLibrary(game_path.clone()));
            ui.close();
        }
        if ui.button("Get title screen again").clicked() {
            actions.push(Action::RemakeArt(game_path.clone()));
            ui.close();
        }
        ui.separator();
        let fav_label = if is_fav { "☆ Remove from favorites" } else { "★ Add to favorites" };
        if ui.button(fav_label).clicked() {
            actions.push(Action::ToggleFavorite(game_path.clone()));
            ui.close();
        }
        if in_folder {
            if ui.button("Remove from folder").clicked() {
                actions.push(Action::RemoveFromFolder { folder: usize::MAX, game: game_path.clone() });
                ui.close();
            }
        }
        ui.separator();
        if ui.button("New folder").clicked() {
            actions.push(Action::CreateFolder(vec![game_path.clone()]));
            ui.close();
        }
        if !folders.is_empty() {
            sub_menu(ui, "Add to folder", |ui| {
                for (i, f) in folders.iter().enumerate() {
                    if f.games.contains(&game_path) {
                        ui.add_enabled(false, egui::Button::new(format!("+ {}", f.name)));
                    } else if ui.button(&f.name).clicked() {
                        actions.push(Action::AddToFolder { folder: i, game: game_path.clone() });
                        ui.close();
                    }
                }
            });
        }
        ui.separator();
        // Leapster games keep their progress on their own cartridge chip; a
        // Zippity's or RockIt Twist's belongs to the whole console.
        let own_save = !matches!(game.family, Family::Zippity | Family::RockItTwist | Family::LeapPad);
        let delete = ui
            .add_enabled(
                own_save,
                egui::Button::new(egui::RichText::new("Delete save data…").color(egui::Color32::from_rgb(235, 87, 80))),
            )
            .on_hover_text("Removes this game's progress, profiles and save states")
            .on_disabled_hover_text("This console's saves belong to the whole console, not one game");
        if delete.clicked() {
            actions.push(Action::AskDeleteSave(game_path.clone(), game.title.clone()));
            ui.close();
        }
    });
}

/// "Delete save data for <title>?" Delete / Cancel.
/// Rename folder dialog. Returns Some(Some(action)) for commit, Some(None) for cancel, None while open.
fn rename_folder_dialog(ctx: &egui::Context, idx: usize, text: &mut String) -> Option<Option<Action>> {
    let screen = ctx.content_rect();
    ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("rename_folder_dim")))
        .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(150));
    let mut answer: Option<Option<Action>> = None;
    egui::Area::new(egui::Id::new("rename_folder"))
        .order(egui::Order::Foreground)
        .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
        .show(ctx, |ui| {
            egui::Frame::NONE
                .fill(egui::Color32::from_rgb(24, 27, 30))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_white_alpha(30)))
                .corner_radius(14)
                .inner_margin(egui::Margin::symmetric(26, 22))
                .shadow(egui::Shadow { offset: [0, 8], blur: 28, spread: 0, color: egui::Color32::from_black_alpha(140) })
                .show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical_centered(|ui| {
                        ui.label(egui::RichText::new("Rename folder").size(18.0).strong().color(egui::Color32::WHITE));
                    });
                    ui.add_space(14.0);
                    let field = ui.add(
                        egui::TextEdit::singleline(text)
                            .desired_width(f32::INFINITY)
                            .margin(egui::vec2(10.0, 6.0)),
                    );
                    field.request_focus();
                    let enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
                    let escape = ui.input(|i| i.key_pressed(egui::Key::Escape));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let ok = ui.add_sized([120.0, 34.0], egui::Button::new("OK")).clicked() || enter;
                        let cancel = ui.add_sized([100.0, 34.0], egui::Button::new("Cancel").frame(false)).clicked() || escape;
                        if ok && !text.trim().is_empty() {
                            answer = Some(Some(Action::RenameFolder { idx, name: text.trim().to_string() }));
                        } else if cancel {
                            answer = Some(None);
                        }
                    });
                });
        });
    answer
}

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
    crate::manual::window(ctx, &mut st.manual);
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

    if let Some(idx) = st.renaming_folder {
        match rename_folder_dialog(ctx, idx, &mut st.rename_text) {
            Some(Some(action)) => {
                st.renaming_folder = None;
                actions.push(action);
            }
            Some(None) => {
                st.renaming_folder = None;
            }
            None => {}
        }
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

fn toasts(ctx: &egui::Context, st: &mut UiState, top: f32) {
    let now = Instant::now();
    st.toasts.retain(|(_, until)| now < *until);
    if st.toasts.is_empty() {
        return;
    }
    // Top centre, like a phone's notifications: each comes out from under
    // the menu bar and slides back up into it when it ends (cut off at the
    // bar's edge), the newest on top. Solid, so nothing shows through.
    const SLIDE: f32 = 0.3;
    let mut y = top + 8.0;
    for (text, until) in st.toasts.iter().rev() {
        let id = egui::Id::new(("toast", text, until));
        let shown = ctx.animate_bool_with_time(id, true, SLIDE);
        let leaving = (until.saturating_duration_since(now).as_secs_f32() / SLIDE).min(1.0);
        let t = shown.min(leaving);
        // Its height from the last frame (a first guess before that).
        let height = ctx.memory(|m| m.area_rect(id)).map_or(40.0, |r| r.height());
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        egui::Area::new(id)
            .anchor(egui::Align2::CENTER_TOP, [0.0, y - (1.0 - ease) * (y - top + height + 20.0)])
            .order(egui::Order::Tooltip)
            .interactable(false)
            .constrain(false)
            .show(ctx, |ui| {
                let below_bar = egui::Rect::from_min_max(egui::pos2(f32::NEG_INFINITY, top), egui::pos2(f32::INFINITY, f32::INFINITY));
                ui.set_clip_rect(ui.clip_rect().intersect(below_bar));
                egui::Frame::NONE
                    .fill(egui::Color32::from_rgb(28, 32, 30))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgba_unmultiplied(46, 170, 82, 120)))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::symmetric(14, 9))
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: egui::Color32::from_black_alpha(120) })
                    .show(ui, |ui| {
                        ui.set_max_width(560.0);
                        ui.add(egui::Label::new(egui::RichText::new(text).color(ACCENT).strong()).wrap());
                    });
            });
        y += (height + 6.0) * ease;
    }
    ctx.request_repaint();
}

fn cheat_window(ctx: &egui::Context, st: &mut UiState, info: &Info, actions: &mut Vec<Action>) {
    let mut open = st.show_cheat;
    egui::Window::new("Cheat / Memory Search")
        .open(&mut open)
        .resizable(true)
        .default_width(380.0)
        .show(ctx, |ui| {
            // Search parameters.
            ui.horizontal(|ui| {
                ui.label("Width:");
                ui.radio_value(&mut st.cheat_ty, ValueType::U8, ValueType::U8.label());
                ui.radio_value(&mut st.cheat_ty, ValueType::U16, ValueType::U16.label());
                ui.radio_value(&mut st.cheat_ty, ValueType::U32, ValueType::U32.label());
            });
            ui.horizontal(|ui| {
                ui.label("Compare:");
                for op in [
                    SearchOp::Equal,
                    SearchOp::NotEqual,
                    SearchOp::Less,
                    SearchOp::Greater,
                    SearchOp::Decreased,
                    SearchOp::Increased,
                    SearchOp::Unchanged,
                ] {
                    ui.radio_value(&mut st.cheat_op, op, op.label());
                }
            });
            ui.horizontal(|ui| {
                ui.add_enabled_ui(st.cheat_op.needs_value(), |ui| {
                    ui.label("Value:");
                    ui.add(egui::TextEdit::singleline(&mut st.cheat_value_text).desired_width(100.0));
                });
            });
            ui.horizontal(|ui| {
                if ui.button(if info.cheat_has_scan { "First scan" } else { "First scan" }).clicked() {
                    actions.push(Action::CheatFirstScan);
                }
                if ui.add_enabled(info.cheat_has_scan, egui::Button::new("Next scan")).clicked() {
                    actions.push(Action::CheatNextScan);
                }
                if ui.add_enabled(info.cheat_has_scan, egui::Button::new("Reset")).clicked() {
                    actions.push(Action::CheatReset);
                }
            });
            let n = info.cheat_candidates.len();
            ui.label(format!("{n} candidate{}", if n == 1 { "" } else { "s" }));
            // The value Set writes once and Freeze holds (empty: Freeze
            // holds the value the address has now).
            let new_value = crate::cheat::parse_number(&st.cheat_new_value);
            let typed = !st.cheat_new_value.trim().is_empty();
            // Negative numbers count down from the top (-1 is 255 in a byte).
            let negative = st.cheat_new_value.trim().starts_with('-');
            let fits = new_value.is_some_and(|v| {
                v <= st.cheat_ty.max() || (negative && (v as i32 as i64) >= -((st.cheat_ty.max() as i64 + 1) / 2))
            });
            ui.horizontal(|ui| {
                ui.label("New value:");
                ui.add(egui::TextEdit::singleline(&mut st.cheat_new_value).desired_width(100.0).hint_text("e.g. 99 or 0x63"));
                if typed && !fits {
                    let why = if new_value.is_none() { "Not a number".to_string() } else { format!("Too big for {} (largest {})", st.cheat_ty.label(), st.cheat_ty.max()) };
                    ui.label(egui::RichText::new(why).color(egui::Color32::from_rgb(235, 120, 100)));
                }
            });
            let mask = st.cheat_ty.max();
            // Candidate list.
            egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                for &(addr, val) in &info.cheat_candidates {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{addr:#010x}")).monospace());
                        ui.label(format!("= {val}  (0x{val:x})"));
                        if ui
                            .add_enabled(fits, egui::Button::new("Set").small())
                            .on_hover_text("Write the new value here once; the game carries on from it")
                            .on_disabled_hover_text("Type a new value above first")
                            .clicked()
                            && let Some(v) = new_value
                        {
                            actions.push(Action::CheatSet(addr, v & mask));
                        }
                        let hold = if fits { new_value.map_or(val, |v| v & mask) } else { val };
                        if ui
                            .add_enabled(!typed || fits, egui::Button::new("Freeze").small())
                            .on_hover_text(if typed { "Keep the new value here" } else { "Keep the value it has now" })
                            .clicked()
                        {
                            actions.push(Action::CheatFreeze(addr, hold));
                        }
                    });
                }
                if n > info.cheat_candidates.len() {
                    ui.label(egui::RichText::new(format!("… and {} more (narrow the search)", n - info.cheat_candidates.len())).weak());
                }
            });
            // Active freezes.
            if !info.cheat_freezes.is_empty() {
                ui.separator();
                ui.label(egui::RichText::new("Frozen values").strong());
                let mut unfreeze = None;
                for (addr, val, label) in &info.cheat_freezes {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(format!("{addr:#010x}")).monospace());
                        ui.label(format!("= {val}  ({label})"));
                        if ui.small_button("Unfreeze").clicked() {
                            unfreeze = Some(*addr);
                        }
                    });
                }
                if let Some(addr) = unfreeze {
                    actions.push(Action::CheatUnfreeze(addr));
                }
            }
        });
    st.show_cheat = open;
}
