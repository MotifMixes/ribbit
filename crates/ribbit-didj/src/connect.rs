//! What LeapFrog Connect does with a Didj, done on Ribbit's own: games
//! installed and uninstalled, and the player's files (profiles, Didjis,
//! game saves, installed games) copied to and from a folder laid out like a
//! real Didj's /Didj.
//!
//! Everything here works on the player's files: the files on the Brio
//! volume (/Didj) that the image built from the packages doesn't have
//! (`carry::files_beyond`, `carry::user_files`), paths from /Didj. The app
//! builds the image again with them (`nand_with_files`) and keeps the
//! changed blocks as the save, as `carry` does; all three modes (Original,
//! Original+, Enhanced (new)) read the save that way.
//!
//! Layout (VERIFIED: the menu's package paths in brio.rs, the console's own
//! writes in carry.rs, games' meta.inf):
//!
//! * A game is an application package in `ProgramFiles/<its folder>`
//!   (meta.inf `Type="Application"`); the menu lists what it finds there.
//!   A cartridge's volume holds the same package (`JDI/`, `MAC/`...).
//! * Its progress: the folders its meta.inf `PersistentData` names
//!   (`"/Data/0/MAC","/Data/1/MAC","/Data/2/MAC","/Data/GameWide/MAC"`:
//!   Foster's), else `Data/<profile>/<its folder>`; the menu's copy of its
//!   icon is `Data/GameIcons/<its folder>.png`.
//! * Profiles: `Data/<n>/profile.dsc` and `profile.log` (and `Backup/`);
//!   Didjis: `Data/Avatars/<folder>`; micromods `Data/MDL`.
//!
//! What goes to a real Didj: `Data/` and `ProgramFiles/` only. `Base/` (the
//! system: settings.cfg sits there beside the Brio libraries) and `brd.rev`
//! are the console's own and never copied to or from one.
use crate::package::Meta;
use ribbit_core::fatfs;

/// A game ready to install: its package's folder name, meta.inf, and files
/// with their paths on /Didj (`ProgramFiles/<folder>/...`; folders end in
/// "/").
pub struct Game {
    pub folder: String,
    pub meta: Meta,
    pub files: Vec<(String, Vec<u8>)>,
}

impl Game {
    /// What the menu and the library call it.
    pub fn name(&self) -> String {
        display_name(&self.meta, &self.folder)
    }
}

/// A game's name from its meta.inf (Foster's says only its code name).
pub fn display_name(meta: &Meta, folder: &str) -> String {
    match meta.get("Name") {
        Some("MAC") => "Foster's Home for Imaginary Friends".to_string(),
        Some(n) if !n.is_empty() => n.to_string(),
        _ => folder.to_string(),
    }
}

fn norm(path: &str) -> String {
    path.trim_start_matches('/').to_ascii_lowercase()
}

/// The game in an application package (`.lfp`), as LeapFrog Connect
/// installs a download: its top folder under ProgramFiles.
pub fn game_from_package(data: &[u8]) -> Result<Game, String> {
    let entries = ribbit_core::zip::entries(data).map_err(|e| format!("not a Didj package (.lfp): {e}"))?;
    let mut files = Vec::with_capacity(entries.len());
    for e in &entries {
        files.push((e.name.clone(), ribbit_core::zip::read_entry(data, e).map_err(|err| format!("{}: {err}", e.name))?));
    }
    game_from_files(files, "package")
}

/// The game on a cartridge dump (the raw NAND or its FAT volume alone).
pub fn game_from_cart(raw: &[u8]) -> Result<Game, String> {
    let files = crate::cart::files(raw).ok_or("not a Didj cartridge dump")?;
    game_from_files(files, "cartridge")
}

/// A package or a cartridge dump, whichever `data` is.
pub fn game_from_file(data: &[u8]) -> Result<Game, String> {
    if data.starts_with(b"PK") {
        // A zipped cartridge dump, or a package.
        if let Ok(g) = game_from_package(data) {
            return Ok(g);
        }
        let entries = ribbit_core::zip::entries(data).map_err(|e| e.to_string())?;
        if let Some(e) = entries.iter().find(|e| !e.name.ends_with('/')) {
            let inner = ribbit_core::zip::read_entry(data, e).map_err(|err| err.to_string())?;
            if crate::cart::Cart::looks_like(&inner) || crate::cart::is_volume(&inner) {
                return game_from_cart(&inner);
            }
        }
        return Err("not a Didj game: neither an application package (.lfp) nor a cartridge dump".into());
    }
    game_from_cart(data).map_err(|_| "not a Didj game: neither an application package (.lfp) nor a cartridge dump".into())
}

/// `files` (paths inside a package or on a cartridge, one top folder) as a
/// game: its meta.inf must say a Didj application.
fn game_from_files(files: Vec<(String, Vec<u8>)>, what: &str) -> Result<Game, String> {
    let (folder, meta) = files
        .iter()
        .find_map(|(n, d)| {
            let folder = n.strip_suffix("/meta.inf").filter(|f| !f.is_empty() && !f.contains('/'))?;
            Some((folder.to_string(), Meta::parse(&String::from_utf8_lossy(d))))
        })
        .ok_or(format!("this {what} has no meta.inf"))?;
    if !meta.is_didj() {
        return Err(format!("this {what} is not for the Didj (its meta.inf names another device)"));
    }
    if meta.get("Type").map(str::trim) != Some("Application") {
        return Err(format!("this {what} is not a game (its meta.inf says Type=\"{}\")", meta.get("Type").unwrap_or("")));
    }
    let prefix = format!("{folder}/");
    let files = files
        .into_iter()
        .filter(|(n, _)| n.starts_with(&prefix))
        .map(|(n, d)| (format!("ProgramFiles/{n}"), d))
        .collect();
    Ok(Game { folder, meta, files })
}

/// The games installed in `files` (the player's files): each folder in
/// ProgramFiles with a meta.inf, and that meta.inf.
pub fn installed(files: &[(String, Vec<u8>)]) -> Vec<(String, Meta)> {
    let mut out: Vec<(String, Meta)> = files
        .iter()
        .filter_map(|(p, d)| {
            let p = p.trim_start_matches('/');
            let rest = p.get(..13).filter(|s| s.eq_ignore_ascii_case("ProgramFiles/")).map(|_| &p[13..])?;
            let folder = rest.strip_suffix("/meta.inf").filter(|f| !f.contains('/'))?;
            Some((folder.to_string(), Meta::parse(&String::from_utf8_lossy(d))))
        })
        .collect();
    out.sort_by(|a, b| a.0.to_ascii_lowercase().cmp(&b.0.to_ascii_lowercase()));
    out
}

/// A game's files in `files` (ProgramFiles/<folder>), read back for the
/// library's art: path inside its folder -> contents.
pub fn game_file<'a>(files: &'a [(String, Vec<u8>)], folder: &str, name: &str) -> Option<&'a [u8]> {
    let want = norm(&format!("ProgramFiles/{folder}/{name}"));
    files.iter().find(|(p, _)| norm(p) == want).map(|(_, d)| d.as_slice())
}

fn under(path: &str, folder: &str) -> bool {
    let (p, f) = (norm(path), norm(folder));
    let f = f.trim_end_matches('/');
    p == f || p.starts_with(&format!("{f}/"))
}

/// Installs `game` among `files`, replacing an older copy of it (the same
/// folder). Its saves stay.
pub fn install(files: &mut Vec<(String, Vec<u8>)>, game: &Game) {
    let folder = format!("ProgramFiles/{}", game.folder);
    files.retain(|(p, _)| !under(p, &folder));
    files.extend(game.files.iter().filter(|(p, _)| !p.ends_with('/')).cloned());
}

/// Where a game keeps its progress: the folders its meta.inf
/// `PersistentData` names, else (GUESS, as JetPack Heroes' POW)
/// `Data/<profile>/<folder>` for the three profiles and
/// `Data/GameWide/<folder>`; and the menu's copy of its icon.
pub fn save_folders(meta: Option<&Meta>, folder: &str) -> Vec<String> {
    let mut out: Vec<String> = meta
        .and_then(|m| m.get("PersistentData"))
        .map(|v| v.split(',').map(|s| s.trim().trim_matches('"').trim_start_matches('/').to_string()).filter(|s| s.len() > 5).collect())
        .unwrap_or_default();
    if out.is_empty() {
        out = ["0", "1", "2", "GameWide"].iter().map(|p| format!("Data/{p}/{folder}")).collect();
    }
    out.push(format!("Data/GameIcons/{folder}.png"));
    out
}

/// The files of `folder`'s saves among `files`.
pub fn game_saves(files: &[(String, Vec<u8>)], folder: &str) -> Vec<String> {
    let meta = installed(files).into_iter().find(|(f, _)| f.eq_ignore_ascii_case(folder)).map(|(_, m)| m);
    let saves = save_folders(meta.as_ref(), folder);
    files.iter().filter(|(p, _)| saves.iter().any(|s| under(p, s))).map(|(p, _)| p.clone()).collect()
}

/// Takes the game in ProgramFiles/`folder` out of `files`, and with
/// `delete_saves` its progress in every profile too. How many files went.
pub fn uninstall(files: &mut Vec<(String, Vec<u8>)>, folder: &str, delete_saves: bool) -> usize {
    let saves = if delete_saves { game_saves(files, folder) } else { Vec::new() };
    let program = format!("ProgramFiles/{folder}");
    let before = files.len();
    files.retain(|(p, _)| !under(p, &program) && !saves.contains(p));
    before - files.len()
}

/// Whether a file of the player's belongs on a real Didj: under Data/ or
/// ProgramFiles/ (profiles, Didjis, saves, games), never the system's
/// Base/ or brd.rev.
pub fn transferable(path: &str) -> bool {
    let p = norm(path);
    (p.starts_with("data/") || p.starts_with("programfiles/")) && !p.contains("../") && !p.ends_with('/')
}

/// The paths `base`'s Brio volume has (lower case, from /Didj, no leading
/// "/"): the packages' files, which stay as built.
pub fn built_paths(base: &crate::nand::Nand) -> std::collections::HashSet<String> {
    let brio = crate::nand::partition("Brio").unwrap();
    let len = crate::brio::volume_sectors(brio.size / crate::nand::BLOCK) * 512;
    fatfs::known_files(&crate::ubi::read_volume(base, brio, len), &|_| true).into_iter().map(|(p, _)| norm(&p)).collect()
}

/// `incoming` (files from a real Didj, or another Ribbit's) merged into
/// `files`: the transferable ones, a file at the same path replaced, those
/// the image has (`built`, from `built_paths`) left out. How many came in.
pub fn merge(files: &mut Vec<(String, Vec<u8>)>, incoming: Vec<(String, Vec<u8>)>, built: &std::collections::HashSet<String>) -> usize {
    let mut n = 0;
    for (path, data) in incoming {
        let path = path.trim_start_matches('/').replace('\\', "/");
        if !transferable(&path) || built.contains(&norm(&path)) {
            continue;
        }
        let key = norm(&path);
        files.retain(|(p, _)| norm(p) != key);
        files.push((format!("/{path}"), data));
        n += 1;
    }
    n
}

/// The files under `dir` on this PC, paths from it with "/". `dir` may be
/// a copy of a real Didj's drive: the folder holding Data/ and
/// ProgramFiles/, or one with a `Didj` folder that holds them.
pub fn read_folder(dir: &std::path::Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let has = |d: &std::path::Path| ["Data", "ProgramFiles"].iter().any(|f| d.join(f).is_dir());
    let root = if !has(dir) && has(&dir.join("Didj")) { dir.join("Didj") } else { dir.to_path_buf() };
    if !has(&root) {
        return Err(format!("{} holds no Data or ProgramFiles folder: choose the copy of a Didj's drive", dir.display()));
    }
    let mut out = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                out.push((rel, std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?));
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Writes the transferable files among `files` into `dir`, laid out as a
/// real Didj's drive (`dir`/Data/..., `dir`/ProgramFiles/...). How many.
pub fn write_folder(dir: &std::path::Path, files: &[(String, Vec<u8>)]) -> Result<usize, String> {
    let mut n = 0;
    for (path, data) in files.iter().filter(|(p, _)| transferable(p)) {
        let rel = path.trim_start_matches('/');
        if rel.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
            continue;
        }
        let to = dir.join(rel);
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        std::fs::write(&to, data).map_err(|e| format!("{}: {e}", to.display()))?;
        n += 1;
    }
    Ok(n)
}

/// The profiles among `files`: (number, name) for each Data/<n>/profile.dsc
/// with a Name (an empty profile.dsc is an unused slot).
pub fn profiles(files: &[(String, Vec<u8>)]) -> Vec<(String, String)> {
    let mut out: Vec<(String, String)> = files
        .iter()
        .filter_map(|(p, d)| {
            let p = p.trim_start_matches('/');
            let n = p.strip_prefix("Data/")?.strip_suffix("/profile.dsc").filter(|n| !n.contains('/'))?;
            let meta = Meta::parse(&String::from_utf8_lossy(d));
            Some((n.to_string(), meta.get("Name")?.to_string()))
        })
        .collect();
    out.sort();
    out
}

/// The same for the Leapster Explorer: its games live on the Bulk volume
/// (UBIFS, /LF/Bulk), as `ubifs::Tree` paths from Bulk's root.
///
/// VERIFIED from the console's own installer (/usr/bin/lfpkg in firmware
/// 1.4.11.2128's root file system): an Application package goes to
/// `ProgramFiles/<its folder>`, Download/MicroDownload/... to `Downloads`;
/// there is no registry beside the files (lfpkg's "remove" is "unsupported:
/// packages must be manually removed", and installing only unpacks and runs
/// a `postinstall` script if the package has one; none of the firmware's
/// do). The menu reads the folders' meta.inf. Ribbit runs no `postinstall`
/// (what a game's would do is UNKNOWN).
/// The player's own files: `Data/` (VERIFIED: `Data/Uploads/<n>/profile.log`
/// after setup, Ribbit 2026-11-12) and `settings.cfg` at the top (the
/// console's own, not copied to or from a real one).
pub mod lx {
    use crate::package::Meta;
    use crate::ubifs::{Entry, Kind, Tree};

    /// A game ready to install: its folder in ProgramFiles, meta.inf and
    /// files (Bulk paths).
    pub struct Game {
        pub folder: String,
        pub meta: Meta,
        pub tree: Tree,
    }

    impl Game {
        pub fn name(&self) -> String {
            super::display_name(&self.meta, &self.folder)
        }
    }

    fn folder_entry() -> Entry {
        Entry { kind: Kind::Dir, mode: 0o755, mtime: 0 }
    }

    fn file(d: Vec<u8>) -> Entry {
        Entry { kind: Kind::File(d), mode: 0o644, mtime: 0 }
    }

    /// An Explorer application package: a `.lf2` (bzip2 tar, unpacked as
    /// lfpkg does, `explorer::install_lf2`) or a `.lfp` (zip, one folder
    /// with meta.inf, unpacked under ProgramFiles as lfpkg's type_to_dir).
    pub fn game_from_package(data: &[u8]) -> Result<Game, String> {
        let mut tree = Tree::new();
        if data.starts_with(b"BZh") {
            crate::explorer::install_lf2(&mut tree, data)?;
        } else {
            let entries = ribbit_core::zip::entries(data).map_err(|e| format!("not an Explorer package (.lf2 or .lfp): {e}"))?;
            for e in &entries {
                let name = e.name.trim_end_matches('/');
                if name.is_empty() {
                    continue;
                }
                let entry = if e.name.ends_with('/') { folder_entry() } else { file(ribbit_core::zip::read_entry(data, e).map_err(|err| format!("{}: {err}", e.name))?) };
                tree.insert(format!("ProgramFiles/{name}"), entry);
            }
        }
        let (folder, meta) = installed(&tree).into_iter().next().ok_or("this package holds no game (no application meta.inf)")?;
        if meta.get("Device") != Some("LeapsterExplorer") {
            return Err("this package is not for the Leapster Explorer (its meta.inf names another device)".into());
        }
        if meta.get("Type") != Some("Application") {
            return Err(format!("this package is not a game (its meta.inf says Type=\"{}\")", meta.get("Type").unwrap_or("")));
        }
        let prefix = format!("ProgramFiles/{folder}");
        tree.retain(|p, _| under(p, &prefix));
        Ok(Game { folder, meta, tree })
    }

    /// The games in ProgramFiles in `tree`: folder and meta.inf.
    pub fn installed(tree: &Tree) -> Vec<(String, Meta)> {
        tree.iter()
            .filter_map(|(p, e)| {
                let folder = p.strip_prefix("ProgramFiles/")?.strip_suffix("/meta.inf").filter(|f| !f.contains('/'))?;
                let Kind::File(d) = &e.kind else { return None };
                Some((folder.to_string(), Meta::parse(&String::from_utf8_lossy(d))))
            })
            .collect()
    }

    /// A file of the game in ProgramFiles/`folder`.
    pub fn game_file<'a>(tree: &'a Tree, folder: &str, name: &str) -> Option<&'a [u8]> {
        match &tree.get(&format!("ProgramFiles/{folder}/{name}"))?.kind {
            Kind::File(d) => Some(d),
            _ => None,
        }
    }

    fn under(path: &str, folder: &str) -> bool {
        path == folder || path.starts_with(&format!("{folder}/"))
    }

    /// Installs `game` into `tree`, replacing an older copy of it.
    pub fn install(tree: &mut Tree, game: &Game) {
        let folder = format!("ProgramFiles/{}", game.folder);
        tree.retain(|p, _| !under(p, &folder));
        tree.extend(game.tree.iter().map(|(p, e)| (p.clone(), e.clone())));
    }

    /// Where a game keeps its progress: its meta.inf `PersistentData`, else
    /// (GUESS, as on the Didj) `Data/<profile>/<folder>`.
    pub fn save_folders(meta: Option<&Meta>, folder: &str) -> Vec<String> {
        let out: Vec<String> = meta
            .and_then(|m| m.get("PersistentData"))
            .map(|v| v.split(',').map(|s| s.trim().trim_matches('"').trim_start_matches('/').trim_start_matches("LF/Bulk/").to_string()).filter(|s| s.len() > 5).collect())
            .unwrap_or_default();
        if !out.is_empty() {
            return out;
        }
        ["0", "1", "2", "GameWide"].iter().map(|p| format!("Data/{p}/{folder}")).collect()
    }

    /// The paths of `folder`'s saves in `tree`.
    pub fn game_saves(tree: &Tree, folder: &str) -> Vec<String> {
        let meta = installed(tree).into_iter().find(|(f, _)| f == folder).map(|(_, m)| m);
        let saves = save_folders(meta.as_ref(), folder);
        tree.iter().filter(|(p, e)| matches!(e.kind, Kind::File(_)) && saves.iter().any(|s| under(p, s))).map(|(p, _)| p.clone()).collect()
    }

    /// Takes the game out of `tree`, and its progress with `delete_saves`.
    /// How many entries went.
    pub fn uninstall(tree: &mut Tree, folder: &str, delete_saves: bool) -> usize {
        let meta = installed(tree).into_iter().find(|(f, _)| f == folder).map(|(_, m)| m);
        let mut gone = vec![format!("ProgramFiles/{folder}")];
        if delete_saves {
            gone.extend(save_folders(meta.as_ref(), folder));
        }
        let before = tree.len();
        tree.retain(|p, _| !gone.iter().any(|g| under(p, g)));
        before - tree.len()
    }

    /// Whether a Bulk path belongs on a real Explorer: the player's Data,
    /// installed games and downloads; never the console's settings.cfg or
    /// language packs.
    pub fn transferable(path: &str) -> bool {
        let p = path.trim_start_matches('/');
        (p.starts_with("Data/") || p.starts_with("ProgramFiles/") || p.starts_with("Downloads/")) && !p.split('/').any(|c| c == "..")
    }

    /// `incoming` merged into `tree`: the transferable entries, those the
    /// image has the same (`built`) left out. How many files came in.
    pub fn merge(tree: &mut Tree, incoming: Tree, built: &Tree) -> usize {
        let mut n = 0;
        for (p, e) in incoming {
            if !transferable(&p) {
                continue;
            }
            let same = match (&e.kind, built.get(&p).map(|b| &b.kind)) {
                (Kind::File(a), Some(Kind::File(b))) => a == b,
                (Kind::Dir, Some(Kind::Dir)) => true,
                _ => false,
            };
            if same {
                continue;
            }
            n += usize::from(matches!(e.kind, Kind::File(_)));
            tree.insert(p, e);
        }
        n
    }

    /// The files under `dir` on this PC as a tree: a copy of a real
    /// Explorer's Bulk (the folder holding Data and ProgramFiles, or a
    /// `Bulk` or `LF/Bulk` folder inside it).
    pub fn read_folder(dir: &std::path::Path) -> Result<Tree, String> {
        let has = |d: &std::path::Path| ["Data", "ProgramFiles"].iter().any(|f| d.join(f).is_dir());
        let root = [dir.to_path_buf(), dir.join("Bulk"), dir.join("LF").join("Bulk")]
            .into_iter()
            .find(|d| has(d))
            .ok_or_else(|| format!("{} holds no Data or ProgramFiles folder: choose the copy of an Explorer's /LF/Bulk", dir.display()))?;
        let mut tree = Tree::new();
        let mut stack = vec![root.clone()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?.flatten() {
                let p = e.path();
                let rel = p.strip_prefix(&root).unwrap_or(&p).to_string_lossy().replace('\\', "/");
                if p.is_dir() {
                    tree.insert(rel, folder_entry());
                    stack.push(p);
                } else {
                    tree.insert(rel, file(std::fs::read(&p).map_err(|e| format!("{}: {e}", p.display()))?));
                }
            }
        }
        Ok(tree)
    }

    /// Writes the transferable files of `tree` under `out`, laid out as the
    /// Explorer's /LF/Bulk. How many files.
    pub fn write_folder(out: &std::path::Path, tree: &Tree) -> Result<usize, String> {
        let mut n = 0;
        for (p, e) in tree.iter().filter(|(p, _)| transferable(p)) {
            let to = out.join(p);
            match &e.kind {
                Kind::Dir => std::fs::create_dir_all(&to).map_err(|err| format!("{}: {err}", to.display()))?,
                Kind::File(d) => {
                    if let Some(parent) = to.parent() {
                        std::fs::create_dir_all(parent).map_err(|err| format!("{}: {err}", parent.display()))?;
                    }
                    std::fs::write(&to, d).map_err(|err| format!("{}: {err}", to.display()))?;
                    n += 1;
                }
                Kind::Symlink(_) | Kind::Special(..) => {}
            }
        }
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A zip of stored (uncompressed) files.
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let crc = !data.iter().fold(!0u32, |c, &b| (0..8).fold(c ^ b as u32, |c, _| if c & 1 != 0 { c >> 1 ^ 0xEDB8_8320 } else { c >> 1 }));
            let local = out.len() as u32;
            let sizes = [crc, data.len() as u32, data.len() as u32];
            out.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            sizes.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            sizes.iter().for_each(|v| dir.extend_from_slice(&v.to_le_bytes()));
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&local.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    const META: &[u8] = b"Device=\"Didj\"\nType=\"Application\"\nName=\"MAC\"\nPersistentData=\"/Data/0/MAC\",\"/Data/1/MAC\",\"/Data/2/MAC\",\"/Data/GameWide/MAC\"\n";

    #[test]
    fn a_package_installs_into_program_files_and_uninstalls() {
        let lfp = zip(&[("MAC/", b""), ("MAC/meta.inf", META), ("MAC/App.so", b"elf"), ("MAC/Data/x.ogg", b"ogg")]);
        let g = game_from_package(&lfp).unwrap();
        assert_eq!((g.folder.as_str(), g.name().as_str()), ("MAC", "Foster's Home for Imaginary Friends"));
        let mut files = vec![
            ("/Data/0/profile.dsc".to_string(), b"Name=\"ANN\"\n".to_vec()),
            ("/Data/0/MAC/save.dat".to_string(), b"lvl3".to_vec()),
            ("/Data/0/POW/GameData.bin".to_string(), b"jp".to_vec()),
            ("/Data/GameIcons/MAC.png".to_string(), b"png".to_vec()),
            ("/ProgramFiles/MAC/App.so".to_string(), b"old".to_vec()),
        ];
        install(&mut files, &g);
        assert_eq!(game_file(&files, "MAC", "App.so"), Some(&b"elf"[..]), "the older copy replaced");
        assert_eq!(installed(&files).len(), 1);
        assert_eq!(game_saves(&files, "mac"), ["/Data/0/MAC/save.dat", "/Data/GameIcons/MAC.png"]);
        let mut kept = files.clone();
        assert_eq!(uninstall(&mut kept, "MAC", false), 3);
        assert!(kept.iter().any(|(p, _)| p == "/Data/0/MAC/save.dat"), "saves stay unless asked");
        assert_eq!(uninstall(&mut files, "MAC", true), 5);
        let left: Vec<&str> = files.iter().map(|(p, _)| p.as_str()).collect();
        assert_eq!(left, ["/Data/0/profile.dsc", "/Data/0/POW/GameData.bin"]);
        assert_eq!(profiles(&files), [("0".to_string(), "ANN".to_string())]);
    }

    #[test]
    fn not_a_game() {
        let system = zip(&[("Base_bin/meta.inf", b"Device=\"Didj\"\nType=\"System\"\n")]);
        assert!(game_from_package(&system).err().unwrap().contains("not a game"));
        let twist = zip(&[("X/meta.inf", b"Device=\"RockitTwist\"\nType=\"Application\"\n")]);
        assert!(game_from_package(&twist).err().unwrap().contains("not for the Didj"));
        assert!(game_from_file(b"nothing").is_err());
    }

    #[test]
    fn only_data_and_games_cross_to_a_real_didj() {
        assert!(transferable("/Data/0/profile.dsc") && transferable("ProgramFiles/MAC/App.so") && transferable("Data/Avatars/D/meta.inf"));
        assert!(!transferable("/Base/settings.cfg") && !transferable("brd.rev") && !transferable("Data/../Base/x"));
        let built: std::collections::HashSet<String> = ["programfiles/pow/app.so".to_string()].into();
        let mut files = vec![("/Data/0/profile.dsc".to_string(), b"old".to_vec())];
        let incoming = vec![
            ("Data/0/profile.dsc".to_string(), b"new".to_vec()),
            ("Base/settings.cfg".to_string(), b"x".to_vec()),
            ("ProgramFiles/POW/App.so".to_string(), b"built in".to_vec()),
            ("Data\\Avatars\\D\\meta.inf".to_string(), b"m".to_vec()),
        ];
        assert_eq!(merge(&mut files, incoming, &built), 2);
        assert_eq!(files, [("/Data/0/profile.dsc".to_string(), b"new".to_vec()), ("/Data/Avatars/D/meta.inf".to_string(), b"m".to_vec())]);
    }

    #[test]
    fn a_folder_round_trips() {
        let dir = std::env::temp_dir().join(format!("ribbit-connect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let files = vec![
            ("/Data/0/profile.dsc".to_string(), b"Name=\"ANN\"".to_vec()),
            ("/Base/settings.cfg".to_string(), b"no".to_vec()),
            ("/ProgramFiles/MAC/meta.inf".to_string(), META.to_vec()),
        ];
        assert_eq!(write_folder(&dir.join("Didj"), &files).unwrap(), 2);
        assert!(!dir.join("Didj/Base").exists());
        let back = read_folder(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(back, [("Data/0/profile.dsc".to_string(), b"Name=\"ANN\"".to_vec()), ("ProgramFiles/MAC/meta.inf".to_string(), META.to_vec())]);
    }

    #[test]
    fn an_explorer_game_installs_and_uninstalls() {
        use super::lx;
        use crate::ubifs::{Entry, Kind, Tree};
        let meta = b"Device=\"LeapsterExplorer\"\nType=\"Application\"\nName=\"Pet Pad\"\n";
        let lfp = zip(&[("PetPad/meta.inf", meta), ("PetPad/App.so", b"elf")]);
        let g = lx::game_from_package(&lfp).ok().unwrap();
        assert_eq!((g.folder.as_str(), g.name().as_str()), ("PetPad", "Pet Pad"));
        let f = |d: &[u8]| Entry { kind: Kind::File(d.to_vec()), mode: 0o644, mtime: 0 };
        let mut tree = Tree::new();
        tree.insert("Data/0/PetPad/save".into(), f(b"s"));
        tree.insert("settings.cfg".into(), f(b"c"));
        lx::install(&mut tree, &g);
        assert_eq!(lx::game_file(&tree, "PetPad", "App.so"), Some(&b"elf"[..]));
        assert_eq!(lx::installed(&tree).len(), 1);
        assert_eq!(lx::game_saves(&tree, "PetPad"), ["Data/0/PetPad/save"]);
        // To a real Explorer's layout and back (not settings.cfg).
        let dir = std::env::temp_dir().join(format!("ribbit-lx-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(lx::write_folder(&dir.join("LF").join("Bulk"), &tree).unwrap(), 3);
        let back = lx::read_folder(&dir).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let mut merged = Tree::new();
        assert_eq!(lx::merge(&mut merged, back, &Tree::new()), 3);
        assert_eq!(lx::installed(&merged).len(), 1);
        assert!(lx::uninstall(&mut tree, "PetPad", false) >= 2);
        assert!(tree.contains_key("Data/0/PetPad/save"), "saves stay unless asked");
        lx::install(&mut tree, &g);
        lx::uninstall(&mut tree, "PetPad", true);
        assert!(!tree.contains_key("Data/0/PetPad/save") && tree.contains_key("settings.cfg"));
        let didj = zip(&[("X/meta.inf", b"Device=\"Didj\"\nType=\"Application\"\n")]);
        assert!(lx::game_from_package(&didj).is_err());
    }
}
