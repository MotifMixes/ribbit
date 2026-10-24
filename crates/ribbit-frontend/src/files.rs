//! Choosing BIOS and cartridge files: native dialogs on desktop, an in-app
//! browser on Android (and as a fallback).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Bios,
    Cartridge,
    /// A folder of games for the library.
    RomFolder,
    /// Content for the RockIt Twist's internal storage: a folder laid out
    /// like the console's F: drive.
    TwistContent,
    /// A zip of RockIt Twist packages to import.
    TwistZip,
    /// Output folder for screen recordings.
    RecordFolder,
    /// A virtual LeapPad book (.toml).
    LeapPadBook,
}

impl Pick {
    pub fn title(self) -> &'static str {
        match self {
            Pick::Bios => "Open Leapster BIOS",
            Pick::Cartridge => "Open Leapster cartridge",
            Pick::RomFolder => "Choose your games folder",
            Pick::TwistContent => "Choose RockIt Twist content (a folder laid out like the console's F: drive)",
            Pick::TwistZip => "Import RockIt Twist games (.zip)",
            Pick::RecordFolder => "Choose recording output folder",
            Pick::LeapPadBook => "Open a LeapPad book (.toml)",
        }
    }

    /// Whether a folder is chosen (not a file).
    pub fn is_folder(self) -> bool {
        matches!(self, Pick::RomFolder | Pick::TwistContent | Pick::RecordFolder)
    }
}

/// Native file dialog (blocks until the user chooses). Windows and Linux;
/// macOS uses `SheetPick`.
#[cfg(not(any(target_os = "android", target_os = "macos")))]
pub fn native_pick(what: Pick, start: Option<&Path>) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_title(what.title());
    if let Some(dir) = start.and_then(|p| if p.is_dir() { Some(p) } else { p.parent() }) {
        d = d.set_directory(dir);
    }
    // The games folder: a game in it is picked, and its folder taken. The
    // folder dialogs of Windows and GTK hide every file, so players could
    // not see their games while looking for the folder.
    if what == Pick::RomFolder {
        let game = d
            .set_title("Choose any game in your games folder (its folder is added)")
            .add_filter("Games", &["bin", "rom", "u3", "lst", "zip", "lfp", "lf3", "img"])
            .add_filter("All files", &["*"])
            .pick_file()?;
        return game.parent().map(Path::to_path_buf);
    }
    if what.is_folder() {
        return d.pick_folder();
    }
    if what == Pick::TwistZip {
        return d.add_filter("Zip archives", &["zip"]).add_filter("All files", &["*"]).pick_file();
    }
    if what == Pick::LeapPadBook {
        return d.add_filter("LeapPad books", &["toml"]).add_filter("All files", &["*"]).pick_file();
    }
    d.add_filter("Leapster images", &["bin", "rom", "u3", "lst", "zip"]).add_filter("All files", &["*"]).pick_file()
}

/// macOS: the open panel as a sheet on the window, shown at once and
/// answered later while the event loop and drawing carry on (`poll` it each
/// frame). A modal panel run from inside winit's event handler holds up
/// every event until it closes: the window stops drawing, and on some Macs
/// the whole desktop stops responding.
#[cfg(target_os = "macos")]
pub struct SheetPick {
    pub what: Pick,
    answer: std::pin::Pin<Box<dyn std::future::Future<Output = Option<rfd::FileHandle>> + Send>>,
}

#[cfg(target_os = "macos")]
impl SheetPick {
    pub fn start(what: Pick, start: Option<&Path>, window: &winit::window::Window) -> Self {
        let mut d = rfd::AsyncFileDialog::new().set_title(what.title()).set_parent(window);
        if let Some(dir) = start.and_then(|p| if p.is_dir() { Some(p) } else { p.parent() }) {
            d = d.set_directory(dir);
        }
        let answer: std::pin::Pin<Box<dyn std::future::Future<Output = _> + Send>> = if what.is_folder() {
            Box::pin(d.pick_folder())
        } else if what == Pick::TwistZip {
            Box::pin(d.add_filter("Zip archives", &["zip"]).add_filter("All files", &["*"]).pick_file())
        } else if what == Pick::LeapPadBook {
            Box::pin(d.add_filter("LeapPad books", &["toml"]).add_filter("All files", &["*"]).pick_file())
        } else {
            Box::pin(
                d.add_filter("Leapster images", &["bin", "rom", "u3", "lst", "zip"])
                    .add_filter("All files", &["*"])
                    .pick_file(),
            )
        };
        Self { what, answer }
    }

    /// `Some` once the panel has closed: the chosen path, or `None` if
    /// cancelled.
    pub fn poll(&mut self) -> Option<Option<PathBuf>> {
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match self.answer.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(file) => Some(file.map(|f| f.path().to_path_buf())),
            std::task::Poll::Pending => None,
        }
    }
}

/// In-app file browser (egui).
#[derive(Default)]
pub struct Browser {
    pub open: Option<Pick>,
    dir: PathBuf,
    entries: Vec<(String, PathBuf, bool)>,
    error: Option<String>,
}

impl Browser {
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    pub fn start(&mut self, what: Pick, start: Option<&Path>) {
        let dir = start
            .and_then(|p| if p.is_dir() { Some(p.to_path_buf()) } else { p.parent().map(Path::to_path_buf) })
            .unwrap_or_else(default_browse_dir);
        self.open = Some(what);
        self.go(dir);
    }

    fn go(&mut self, dir: PathBuf) {
        self.entries.clear();
        self.error = None;
        match std::fs::read_dir(&dir) {
            Ok(rd) => {
                for e in rd.flatten() {
                    let path = e.path();
                    let is_dir = path.is_dir();
                    let name = e.file_name().to_string_lossy().into_owned();
                    if name.starts_with('.') {
                        continue;
                    }
                    self.entries.push((name, path, is_dir));
                }
                self.entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));
            }
            Err(e) => self.error = Some(format!("Can't open this folder: {e}")),
        }
        self.dir = dir;
    }

    /// Shows the browser; returns the chosen file.
    pub fn ui(&mut self, ctx: &egui::Context) -> Option<(Pick, PathBuf)> {
        let what = self.open?;
        let mut chosen = None;
        let mut keep_open = true;
        let mut go_to = None;
        egui::Window::new(what.title()).open(&mut keep_open).collapsible(false).resizable(true).default_size([520.0, 420.0]).show(
            ctx,
            |ui| {
                ui.horizontal(|ui| {
                    if ui.button("⬆ Up").clicked()
                        && let Some(p) = self.dir.parent()
                    {
                        go_to = Some(p.to_path_buf());
                    }
                    for (label, dir) in shortcut_dirs() {
                        if ui.button(label).clicked() {
                            go_to = Some(dir);
                        }
                    }
                });
                ui.label(egui::RichText::new(self.dir.display().to_string()).weak());
                if what.is_folder() && ui.button("✔ Use this folder").clicked() {
                    chosen = Some((what, self.dir.clone()));
                }
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                    #[cfg(target_os = "android")]
                    ui.label("Ribbit needs \"All files access\" to read your BIOS and games: Android Settings → Apps → Ribbit → Permissions.");
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (name, path, is_dir) in &self.entries {
                        if what.is_folder() && !*is_dir {
                            continue;
                        }
                        let label = if *is_dir { format!("📁 {name}") } else { format!("🎮 {name}") };
                        if ui.selectable_label(false, label).clicked() {
                            if *is_dir {
                                go_to = Some(path.clone());
                            } else {
                                chosen = Some((what, path.clone()));
                            }
                        }
                    }
                });
            },
        );
        if let Some(d) = go_to {
            self.go(d);
        }
        if chosen.is_some() || !keep_open {
            self.open = None;
        }
        chosen
    }
}

fn default_browse_dir() -> PathBuf {
    #[cfg(target_os = "android")]
    {
        PathBuf::from("/storage/emulated/0")
    }
    #[cfg(not(target_os = "android"))]
    {
        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
    }
}

fn shortcut_dirs() -> Vec<(&'static str, PathBuf)> {
    #[cfg(target_os = "android")]
    {
        vec![
            ("Internal", PathBuf::from("/storage/emulated/0")),
            ("Downloads", PathBuf::from("/storage/emulated/0/Download")),
            ("SD cards", PathBuf::from("/storage")),
        ]
    }
    #[cfg(not(target_os = "android"))]
    {
        let home = default_browse_dir();
        vec![("Home", home.clone()), ("Desktop", home.join("Desktop")), ("Downloads", home.join("Downloads"))]
    }
}
