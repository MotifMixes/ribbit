//! Choosing BIOS and cartridge files: native dialogs on desktop, an in-app
//! browser on Android (and as a fallback).

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pick {
    Bios,
    Cartridge,
}

impl Pick {
    pub fn title(self) -> &'static str {
        match self {
            Pick::Bios => "Open Leapster BIOS",
            Pick::Cartridge => "Open Leapster cartridge",
        }
    }
}

/// Native file dialog (blocks until the user chooses). Desktop only.
#[cfg(not(target_os = "android"))]
pub fn native_pick(what: Pick, start: Option<&Path>) -> Option<PathBuf> {
    let mut d =
        rfd::FileDialog::new().set_title(what.title()).add_filter("Leapster images", &["bin", "rom", "u3", "lst"]);
    d = d.add_filter("All files", &["*"]);
    if let Some(dir) = start.and_then(|p| if p.is_dir() { Some(p) } else { p.parent() }) {
        d = d.set_directory(dir);
    }
    d.pick_file()
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
                if let Some(e) = &self.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, e);
                    #[cfg(target_os = "android")]
                    ui.label("Ribbit needs \"All files access\" to read your BIOS and games: Android Settings → Apps → Ribbit → Permissions.");
                }
                ui.separator();
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    for (name, path, is_dir) in &self.entries {
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
