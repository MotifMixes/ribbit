//! Outline fonts for HD device text (ribbit_core::swf_font): the BIOS draws
//! text in its device fonts ("Leapster Mono", "Leapster Prop") from pixel
//! glyphs; some cartridges carry the same fonts with their outlines. The
//! games folders are searched once in the background; HD canvases take the
//! fonts when they are ready.

use ribbit_core::swf_font::OutlineFont;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub type Fonts = Arc<HashMap<String, OutlineFont>>;

static FONTS: Mutex<Option<Fonts>> = Mutex::new(None);
static LOADING: AtomicBool = AtomicBool::new(false);

/// The fonts found so far (None until the first search ends).
pub fn get() -> Option<Fonts> {
    FONTS.lock().unwrap().clone()
}

/// Searches `folders` (and their subfolders) in the background for Flash
/// movies with font outlines; a search already running is left to finish.
pub fn load(folders: Vec<PathBuf>) {
    if LOADING.swap(true, Ordering::AcqRel) {
        return;
    }
    let _ = std::thread::Builder::new().name("ribbit-fonts".into()).spawn(move || {
        let mut fonts: HashMap<String, OutlineFont> = HashMap::new();
        let mut files = vec![];
        for f in &folders {
            collect(f, &mut files, 0);
        }
        for path in files {
            let Ok(data) = ribbit_core::read_image(&path) else { continue };
            for (name, font) in ribbit_core::swf_font::find_fonts(&data) {
                if fonts.get(&name).is_none_or(|f| f.glyphs.len() < font.glyphs.len()) {
                    fonts.insert(name, font);
                }
            }
        }
        tracing::info!(target: "ribbit::hd", fonts = ?fonts.iter().map(|(n, f)| (n.as_str(), f.glyphs.len())).collect::<Vec<_>>(), "outline fonts for HD text");
        *FONTS.lock().unwrap() = Some(Arc::new(fonts));
        LOADING.store(false, Ordering::Release);
    });
}

/// Cartridge-sized .bin and .zip files under `dir`.
fn collect(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            if depth < 4 {
                collect(&p, out, depth + 1);
            }
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("zip") || x.eq_ignore_ascii_case("bin"))
            && e.metadata().is_ok_and(|m| (64 * 1024..=64 * 1024 * 1024).contains(&m.len()))
        {
            out.push(p);
        }
    }
}
