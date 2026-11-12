//! A Didj save as a folder: a copy of the Didj's /Didj tree (from a
//! backup or another emulator), to carry into Ribbit's Didj (`read`).
//!
//! The folder is the /Didj tree itself, or a folder with it as `didj` or
//! `assets/didj`; a pristine copy of `Data` may be beside it as
//! `assets/factory/Data`. The player's own files there:
//! `Data/<0|1|2>/` (profile.dsc, key=value lines: Name, Grade, Points,
//! AvatarPath=/Didj/Data/Avatars/<folder>/, ColorArt, LastSBShown; game
//! progress), `Data/Avatars/<folder>/` (Didjis, with meta.inf),
//! `Base/settings.cfg` (CurrProfile=0x<n>, Locale), colour themes in
//! `Base/BLT/ColorArt/<theme>`, and games installed from packages in
//! `ProgramFiles/<folder>` (their .lfp files may be in `assets/lfp`). Save
//! states kept beside it can't be read.

use crate::nand::{self, Nand};
use crate::{brio, ubi};
use ribbit_core::fatfs;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What a Didj save folder gives the Didj.
#[derive(Debug, Default)]
pub struct Import {
    /// Files for /Didj (paths from it, "/Data/0/profile.dsc").
    pub files: Vec<(String, Vec<u8>)>,
    /// Packages (.lfp) of games installed there that the packages `read`
    /// was given don't have: installed from these instead of copied.
    pub packages: Vec<PathBuf>,
    /// The profile slots it has (`Data/<n>/profile.dsc` with a name).
    pub profiles: Vec<u32>,
}

/// The /Didj folder in `path`: `path` itself, or its `didj` or
/// `assets/didj`.
pub fn didj_folder(path: &Path) -> Option<PathBuf> {
    let looks = |p: &Path| p.join("Data").is_dir() || p.join("Base").is_dir();
    [path.join("assets").join("didj"), path.join("didj"), path.to_path_buf()].into_iter().find(|p| looks(p))
}

/// Whether `path` is one of the player's files on /Didj, not the system's.
fn players(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    p.starts_with("/data/") || p == "/base/settings.cfg" || p.starts_with("/base/blt/colorart/") || p.starts_with("/programfiles/")
}

/// Every file under `dir` (symbolic links left out), paths from it with
/// `prefix` before them.
fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, PathBuf)>) -> Result<(), String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(kind) = e.file_type() else { continue };
        let path = format!("{prefix}/{}", e.file_name().to_string_lossy());
        if kind.is_dir() {
            walk(&e.path(), &path, out)?;
        } else if kind.is_file() {
            out.push((path, e.path()));
        }
    }
    Ok(())
}

/// The folder a package installs into its install folder (its zip's
/// top-level folder).
fn package_folder(lfp: &Path) -> Option<String> {
    let data = std::fs::read(lfp).ok()?;
    let entries = ribbit_core::zip::entries(&data).ok()?;
    let first = entries.iter().find_map(|e| e.name.replace('\\', "/").split_once('/').map(|(top, _)| top.to_string()))?;
    Some(first)
}

/// What the Didj save folder at `path` (`didj_folder`) holds for a Didj
/// built as `base`: the player's files that `base` doesn't have as they
/// are, and those its factory copy doesn't have either (a stock file of
/// another firmware version is left out). A game installed there that
/// `base` lacks comes from its .lfp when the folder has one, else as
/// its files.
pub fn read(path: &Path, base: &Nand) -> Result<Import, String> {
    let root = didj_folder(path).ok_or_else(|| format!("{}: no Didj folder (Data or Base) there", path.display()))?;
    let assets = root.parent().map(Path::to_path_buf).unwrap_or_default();
    let brio = nand::partition("Brio").unwrap();
    let volume = ubi::read_volume(base, brio, brio::volume_sectors(brio.size / nand::BLOCK) * 512);
    let built: HashMap<String, Vec<u8>> = fatfs::known_files(&volume, &|_| true).into_iter().map(|(p, d)| (p.to_ascii_lowercase(), d)).collect();
    let mut factory = Vec::new();
    let factory_dir = assets.join("factory").join("Data");
    if factory_dir.is_dir() {
        walk(&factory_dir, "/Data", &mut factory)?;
    }
    let factory: HashMap<String, PathBuf> = factory.into_iter().map(|(p, f)| (p.to_ascii_lowercase(), f)).collect();
    let mut found = Vec::new();
    walk(&root, "", &mut found)?;
    let mut import = Import::default();
    for (p, file) in found.into_iter().filter(|(p, _)| players(p)) {
        let data = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let key = p.to_ascii_lowercase();
        if built.get(&key) == Some(&data) || factory.get(&key).and_then(|f| std::fs::read(f).ok()).as_ref() == Some(&data) {
            continue;
        }
        import.files.push((p, data));
    }
    // Games installed there: from their package when it is there.
    let installed = |top: &str| built.keys().any(|k| k.starts_with(&format!("/programfiles/{}/", top.to_ascii_lowercase())));
    let mut lfps = Vec::new();
    if let Ok(rd) = std::fs::read_dir(assets.join("lfp")) {
        lfps = rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lfp"))).collect();
        lfps.sort();
    }
    let tops: std::collections::BTreeSet<String> = import
        .files
        .iter()
        .filter_map(|(p, _)| p.strip_prefix("/ProgramFiles/").or_else(|| p.strip_prefix("/programfiles/")).and_then(|r| r.split_once('/')).map(|(t, _)| t.to_string()))
        .collect();
    for top in tops.into_iter().filter(|t| !installed(t)) {
        if let Some(lfp) = lfps.iter().find(|l| package_folder(l).is_some_and(|f| f.eq_ignore_ascii_case(&top))) {
            let under = format!("/programfiles/{}/", top.to_ascii_lowercase());
            import.files.retain(|(p, _)| !p.to_ascii_lowercase().starts_with(&under));
            import.packages.push(lfp.clone());
        }
    }
    for n in 0..3 {
        let named = std::fs::read(root.join("Data").join(n.to_string()).join("profile.dsc"))
            .is_ok_and(|d| String::from_utf8_lossy(&d).lines().any(|l| l.strip_prefix("Name=").is_some_and(|v| !v.trim().is_empty())));
        if named {
            import.profiles.push(n);
        }
    }
    Ok(import)
}

/// `ours` (the player's files Ribbit's Didj has) with `import`'s over them:
/// a profile slot the import has replaces Ribbit's whole (its game
/// progress with it), and any other file at the same path is the
/// import's.
pub fn merge(ours: Vec<(String, Vec<u8>)>, import: &Import) -> Vec<(String, Vec<u8>)> {
    let theirs: std::collections::HashSet<String> = import.files.iter().map(|(p, _)| p.to_ascii_lowercase()).collect();
    let slots: Vec<String> = import.profiles.iter().map(|n| format!("/data/{n}/")).collect();
    let mut out: Vec<_> = ours
        .into_iter()
        .filter(|(p, _)| {
            let k = format!("/{}", p.trim_start_matches('/')).to_ascii_lowercase();
            !theirs.contains(&k) && !slots.iter().any(|s| k.starts_with(s))
        })
        .collect();
    out.extend(import.files.iter().cloned());
    // A Didj that was never set up, given profiles without the settings
    // file (a save of Data/ alone): the file as the Didj
    // writes it once set up (29 bytes, as in Ribbit's own saves), so it
    // starts at Sign In instead of its first-time questions.
    if let Some(n) = import.profiles.first()
        && !out.iter().any(|(p, _)| p.trim_start_matches('/').eq_ignore_ascii_case("Base/settings.cfg"))
    {
        out.push(("/Base/settings.cfg".to_string(), format!("CurrProfile=0x{n:x}\nLocale=en-us\n").into_bytes()));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip holding one file, stored.
    fn stored_zip(name: &str, data: &[u8]) -> Vec<u8> {
        let crc = crc32fast::hash(data);
        let fields = |out: &mut Vec<u8>| {
            out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
            out.extend(crc.to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((data.len() as u32).to_le_bytes());
            out.extend((name.len() as u16).to_le_bytes());
            out.extend([0, 0]);
        };
        let mut z = b"PK".to_vec();
        fields(&mut z);
        z.extend(name.as_bytes());
        z.extend(data);
        let cd = z.len() as u32;
        z.extend(b"PK");
        z.extend([20, 0]);
        fields(&mut z);
        z.extend([0; 14]);
        z.extend(name.as_bytes());
        let cd_len = z.len() as u32 - cd;
        z.extend(b"PK");
        z.extend([0, 0, 0, 0, 1, 0, 1, 0]);
        z.extend(cd_len.to_le_bytes());
        z.extend(cd.to_le_bytes());
        z.extend([0, 0]);
        z
    }

    /// A synthetic Didj save folder: a
    /// stock file, a profile, a Didji, a game save, settings and a game
    /// installed from a package it kept.
    #[test]
    fn reads_the_players_files() {
        let dir = std::env::temp_dir().join(format!("ribbit-didj-import-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let didj = dir.join("assets").join("didj");
        let put = |p: &str, d: &[u8]| {
            let f = didj.join(p);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, d).unwrap();
        };
        put("Base/Brio/lib/libBrio.so", b"system");
        put("Base/settings.cfg", b"CurrProfile=0x0\nLocale=en-us\n");
        put("Data/0/profile.dsc", b"Name=ANN\nGrade=0x2\nPoints=0x64\nAvatarPath=/Didj/Data/Avatars/Avatar2/\n");
        put("Data/0/pow/GameData.bin", &[1, 2, 3]);
        put("Data/Avatars/Avatar1/meta.inf", b"Device=\"Didj\"\n");
        put("Data/Avatars/Avatar2/meta.inf", b"Name=\"Zed\"\nDeviceTypes=\"DIDJ\"\n");
        put("ProgramFiles/GAME/game.so", b"code");
        put("ProgramFiles/OTHER/other.so", b"more code");
        let factory = dir.join("assets").join("factory").join("Data").join("Avatars").join("Avatar1");
        std::fs::create_dir_all(&factory).unwrap();
        std::fs::write(factory.join("meta.inf"), b"Device=\"Didj\"\n").unwrap();
        // GAME's package, kept in assets/lfp.
        std::fs::create_dir_all(dir.join("assets").join("lfp")).unwrap();
        let zip = stored_zip("GAME/game.so", b"code");
        std::fs::write(dir.join("assets").join("lfp").join("game.lfp"), &zip).unwrap();
        // Ribbit's Didj: the system file as the folder's.
        let mut base = Nand::erased();
        brio::install(&mut base, &[("Base/Brio/lib/libBrio.so".to_string(), b"system".to_vec())]).unwrap();
        for p in [dir.clone(), dir.join("assets"), didj.clone()] {
            assert_eq!(didj_folder(&p), Some(didj.clone()));
        }
        let import = read(&dir, &base).unwrap();
        let paths: Vec<&str> = import.files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(paths, ["/Base/settings.cfg", "/Data/0/pow/GameData.bin", "/Data/0/profile.dsc", "/Data/Avatars/Avatar2/meta.inf", "/ProgramFiles/OTHER/other.so"]);
        assert_eq!(import.packages, [dir.join("assets").join("lfp").join("game.lfp")]);
        assert_eq!(import.profiles, [0]);
        // Merged: Ribbit's profile 0 gives way, its profile 1 stays.
        let ours = vec![("/Data/0/profile.dsc".to_string(), b"Name=BOB".to_vec()), ("/Data/0/jdi/Profile.ini".to_string(), vec![9]), ("/Data/1/profile.dsc".to_string(), b"Name=CY".to_vec())];
        let merged = merge(ours, &import);
        assert!(merged.iter().any(|(p, d)| p == "/Data/1/profile.dsc" && d == b"Name=CY"));
        assert!(!merged.iter().any(|(p, _)| p == "/Data/0/jdi/Profile.ini"));
        assert!(merged.iter().any(|(p, d)| p == "/Data/0/profile.dsc" && d.starts_with(b"Name=ANN")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
