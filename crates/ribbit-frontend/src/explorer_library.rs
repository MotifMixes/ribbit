//! The Leapster Explorer as a virtual console: its games in the library,
//! installed and uninstalled as LeapFrog Connect would (`connect::lx`), and
//! the player's files to and from a real Explorer's layout. As
//! `didj_library`, it works on the save (`didj::explorer_save_file`) and the
//! image built from the packages: the Bulk volume's file tree, whatever mode
//! runs it (Original now; an Enhanced (new) that reads the same save's tree
//! would see the same games).
use crate::didj_library::{Done, Job, Listed};
use ribbit_didj::connect::lx;
use ribbit_didj::nand::Nand;
use ribbit_didj::ubifs::Tree;
use std::path::{Path, PathBuf};

/// The library's paths for games installed on the Explorer.
pub const INSTALLED_PREFIX: &str = "explorer-installed:";

/// A game's folder on the Explorer, if `path` stands for an installed game.
pub fn installed_folder(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix(INSTALLED_PREFIX)
}

/// The Explorer built from the packages in `dir`, and its save.
pub struct Virtual {
    dir: PathBuf,
    save: PathBuf,
    original: Nand,
    image: String,
}

impl Virtual {
    /// Builds the image (seconds: off the window's thread).
    pub fn open(dir: &Path, save_dir: &Path) -> anyhow::Result<Self> {
        let original = ribbit_didj::explorer::nand_from_packages(dir).map_err(anyhow::Error::msg)?;
        let image = format!("{:08x}", original.fingerprint());
        Ok(Self { dir: dir.to_path_buf(), save: crate::didj::explorer_save_file(dir, save_dir), original, image })
    }

    /// The player's Bulk files: the save applied (what differs from the
    /// image), or carried from a save made on another image.
    pub fn files(&self) -> anyhow::Result<Tree> {
        let Ok(changes) = std::fs::read(&self.save) else { return Ok(Tree::new()) };
        let same = std::fs::read_to_string(self.save.with_extension("nandimage")).ok().as_deref() == Some(self.image.as_str());
        if same {
            let mut flash = self.original.clone();
            flash.apply_changes(&changes).map_err(anyhow::Error::msg)?;
            Ok(ribbit_didj::carry::explorer_files_beyond(&flash, &self.original))
        } else {
            ribbit_didj::carry::explorer_user_files(&changes, &self.original).map_err(anyhow::Error::msg)
        }
    }

    /// Makes `files` the player's files (the image built with them, its
    /// changes the save; the save before kept as `.nandblocks.before-library`).
    /// The NOR flash (`.nor`) is the console's own and stays.
    pub fn write(&self, files: &Tree) -> anyhow::Result<()> {
        let flash = ribbit_didj::explorer::nand_with_files(&self.dir, ribbit_didj::explorer::NAND_BLOCKS, files).map_err(anyhow::Error::msg)?;
        if let Some(d) = self.save.parent() {
            std::fs::create_dir_all(d)?;
        }
        if self.save.exists() {
            std::fs::copy(&self.save, self.save.with_extension("nandblocks.before-library"))?;
        }
        let tmp = self.save.with_extension("tmp");
        std::fs::write(&tmp, flash.changes_from(&self.original))?;
        std::fs::rename(&tmp, &self.save)?;
        std::fs::write(self.save.with_extension("nandimage"), &self.image)?;
        Ok(())
    }

    /// The image's own Bulk files (the packages').
    pub fn built(&self) -> Tree {
        ribbit_didj::carry::explorer_files_beyond(&self.original, &Nand::with_blocks(self.original.blocks()))
    }
}

/// An Explorer game's library card from its meta.inf and files.
fn card(folder: &str, meta: &ribbit_didj::package::Meta, read: impl Fn(&str) -> Option<Vec<u8>>, path: PathBuf, tag: &'static str, key: String) -> crate::library::Game {
    use crate::library::{Family, Game};
    let mut details = vec![("System", "Leapster Explorer".to_string()), ("Folder", folder.to_string())];
    for (label, k) in [("Name", "Name"), ("Version", "Version"), ("Product", "ProductID"), ("Publisher", "Publisher"), ("Category", "Category")] {
        if let Some(v) = meta.get(k) {
            details.push((label, v.to_string()));
        }
    }
    details.push(("Status", "early emulation: starts the Explorer, where the game is in its menu (Ribbit doesn't pick it there yet)".to_string()));
    Game {
        title: ribbit_didj::connect::display_name(meta, folder).trim().to_string(),
        subtitle: meta.get("Version").map_or(String::new(), |v| format!("version {v}")),
        details,
        tags: vec![tag.into()],
        key,
        art: crate::didj::card_art(meta, read),
        path,
        family: Family::Explorer,
    }
}

/// An Explorer game package (.lf2 or .lfp; not firmware) in the library:
/// "Built-in" beside the Explorer's Firmware Base, else "Not installed".
pub fn describe_package(path: &Path) -> Option<crate::library::Game> {
    let ext = path.extension()?.to_string_lossy().to_ascii_lowercase();
    if ext != "lf2" && ext != "lfp" {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let game = lx::game_from_package(&data).ok()?;
    // The firmware's own apps (Hidden=1: the Connect movie, the movie
    // player, Pet Pad, the accessory teaser) and anything not a game stay
    // in the Explorer's menu only.
    if !is_game(&game.meta) {
        return None;
    }
    let built_in = path.parent().is_some_and(|d| crate::didj::find_explorer_firmware(d).is_some());
    let read = |f: &str| lx::game_file(&game.tree, &game.folder, f).map(<[u8]>::to_vec);
    let mut g = card(&game.folder, &game.meta, read, path.to_path_buf(), if built_in { "Built-in" } else { "Not installed" }, format!("explorer-pkg-{}", game.folder.to_ascii_lowercase()));
    g.details.push(("File", path.display().to_string()));
    Some(g)
}

/// Whether an application is a game for the library: not hidden from
/// the player's view (meta.inf `Hidden=1`, as the firmware's own apps are)
/// and, if it says, of Category "Game".
pub fn is_game(meta: &ribbit_didj::package::Meta) -> bool {
    meta.get("Hidden") != Some("1") && meta.get("DeviceHidden") != Some("1") && meta.get("Category").is_none_or(|c| c.eq_ignore_ascii_case("Game"))
}

/// Whether `path` is a package for the Leapster Explorer (.lf2, or an .lfp
/// whose meta.inf says so).
pub fn is_explorer_package(path: &Path) -> bool {
    let Some(ext) = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()) else { return false };
    match ext.as_str() {
        "lf2" => true,
        "lfp" => std::fs::read(path).ok().and_then(|d| ribbit_didj::package::meta_of(&d)).is_some_and(|m| m.get("Device") == Some("LeapsterExplorer")),
        _ => false,
    }
}

/// The games installed on the Explorer (beyond its packages).
pub fn list(dir: &Path, save_dir: &Path) -> anyhow::Result<Vec<Listed>> {
    let v = Virtual::open(dir, save_dir)?;
    let files = v.files()?;
    Ok(lx::installed(&files)
        .into_iter()
        .filter(|(_, meta)| is_game(meta))
        .map(|(folder, meta)| {
            let read = |f: &str| lx::game_file(&files, &folder, f).map(<[u8]>::to_vec);
            let mut game = card(&folder, &meta, read, PathBuf::from(format!("{INSTALLED_PREFIX}{folder}")), "Installed", format!("{}lx-{}", crate::library::INSTALLED_KEY, folder.to_ascii_lowercase()));
            let saves = lx::game_saves(&files, &folder).len();
            game.details.push(("Saves", if saves == 0 { "none yet".to_string() } else { format!("{saves} files") }));
            Listed { game, menu_art: None }
        })
        .collect())
}

/// Installs the game package at `path`. Its name and folder.
pub fn install(dir: &Path, save_dir: &Path, path: &Path) -> anyhow::Result<(String, String)> {
    let game = lx::game_from_package(&std::fs::read(path)?).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
    let v = Virtual::open(dir, save_dir)?;
    if v.built().contains_key(&format!("ProgramFiles/{}/meta.inf", game.folder)) {
        anyhow::bail!("{} is built into this Explorer already (its package is in {})", game.name(), dir.display());
    }
    let mut files = v.files()?;
    lx::install(&mut files, &game);
    v.write(&files)?;
    Ok((game.name(), game.folder))
}

/// What to tell a player about an export.
const EXPORT_README: &str = "\
These are the files of Ribbit's Leapster Explorer in a real Explorer's layout (/LF/Bulk):
  Data/           profiles and progress
  ProgramFiles/   games installed in Ribbit (not those built into the Explorer)
  Downloads/      downloads installed in Ribbit
The console's own settings.cfg and language packs are left out on purpose.

UNTESTED ON HARDWARE. A real Explorer offers no drive to copy files to: over USB it
speaks only LeapFrog Connect's own protocol. Ribbit knows no safe way to put these
files on a real Explorer, so keep them as a backup in the real layout.
";

/// Runs `job` on the Explorer built from the packages in `dir`.
pub fn run(job: Job, dir: &Path, save_dir: &Path) -> Done {
    let failed = |title: &'static str, explain: &str, e: anyhow::Error| Done::Failed { title, explain: explain.to_string(), error: format!("{e:#}") };
    match job {
        Job::List => Done::Listed { dir: dir.to_path_buf(), explorer: true, games: list(dir, save_dir).unwrap_or_else(|e| {
            tracing::warn!(target: "ribbit::didj", "listing the games installed on the Explorer: {e:#}");
            Vec::new()
        }) },
        Job::Install { path, play } => match install(dir, save_dir, &path) {
            Ok((name, folder)) => Done::Installed { name, folder, art: None, play, explorer: true },
            Err(e) => failed("Couldn't install the game", "Ribbit installs a Leapster Explorer game from its package (.lf2 or .lfp, as LeapFrog Connect downloads it), onto the Explorer built from the packages in its folder.", e),
        },
        Job::Uninstall { folder, title, delete_saves } => {
            let done = Virtual::open(dir, save_dir).and_then(|v| {
                let mut files = v.files()?;
                if lx::uninstall(&mut files, &folder, delete_saves) == 0 {
                    anyhow::bail!("no game is installed in /LF/Bulk/ProgramFiles/{folder}");
                }
                v.write(&files)
            });
            match done {
                Ok(()) => Done::Uninstalled { title, delete_saves },
                Err(e) => failed("Couldn't uninstall the game", "", e),
            }
        }
        Job::Export(out) => {
            let done = Virtual::open(dir, save_dir).and_then(|v| {
                let to = out.join("LF").join("Bulk");
                let n = lx::write_folder(&to, &v.files()?).map_err(anyhow::Error::msg)?;
                std::fs::write(out.join("READ ME - Leapster Explorer files.txt"), EXPORT_README.replace('\n', "\r\n"))?;
                Ok((n, to))
            });
            match done {
                Ok((files, to)) => Done::Exported { files, to },
                Err(e) => failed("Couldn't copy the Explorer's files", "", e),
            }
        }
        Job::ExportNand(out) => {
            let done = Virtual::open(dir, save_dir).and_then(|v| {
                let mut flash = ribbit_didj::explorer::nand_with_files(&v.dir, ribbit_didj::explorer::NAND_BLOCKS, &v.files()?).map_err(anyhow::Error::msg)?;
                flash.add_factory_data();
                let to = out.join("explorer.nand");
                std::fs::write(&to, flash.read(0, flash.size()))?;
                Ok(to)
            });
            match done {
                Ok(to) => Done::ExportedNand(to),
                Err(e) => failed("Couldn't write the NAND image", "", e),
            }
        }
        Job::Import(from) => {
            let done = Virtual::open(dir, save_dir).and_then(|v| {
                let incoming = match lx::read_folder(&from) {
                    Ok(t) => t,
                    // A folder holding a NAND dump of a real Explorer
                    // (512 MB data area, explorer.nand).
                    Err(e) => {
                        let dump = from.join("explorer.nand");
                        if !dump.is_file() {
                            anyhow::bail!("{e}");
                        }
                        let data = std::fs::read(&dump)?;
                        let mut flash = Nand::with_blocks(data.len() / ribbit_didj::nand::BLOCK);
                        for (b, block) in data.chunks(ribbit_didj::nand::BLOCK).enumerate() {
                            if block.iter().any(|&x| x != 0xFF) {
                                flash.write(b * ribbit_didj::nand::BLOCK, block);
                            }
                        }
                        ribbit_didj::carry::explorer_files_beyond(&flash, &Nand::with_blocks(flash.blocks()))
                    }
                };
                let mut files = v.files()?;
                let n = lx::merge(&mut files, incoming, &v.built());
                if n == 0 {
                    anyhow::bail!("{} holds no profiles, saves or games to bring in", from.display());
                }
                v.write(&files)?;
                Ok(n)
            });
            match done {
                Ok(n) => Done::Imported(n),
                Err(e) => failed("Couldn't import from the Explorer", "Choose the folder you copied a real Explorer's /LF/Bulk into (it holds Data and ProgramFiles), or a folder holding its NAND dump as explorer.nand.", e),
            }
        }
    }
}
