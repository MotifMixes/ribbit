//! The Didj as a virtual console on the desktop: the games installed on it
//! listed in the library, games installed and uninstalled as LeapFrog
//! Connect does (`ribbit_didj::connect`), and the player's files copied to
//! and from a real Didj's layout.
//!
//! The console is "plugged in" meanwhile, as a real one is to Connect: the
//! app turns a running Didj off first, and the work is done on its save (the
//! changed blocks, `didj::save_file`) with the image built from its
//! packages. All three modes (Original, Original+, Enhanced (new)) start
//! from that save, so what is done here shows in each.
use ribbit_didj::connect;
use ribbit_didj::nand::Nand;
use std::path::{Path, PathBuf};

/// The library's paths for games installed on the console (no file of
/// their own): this, then the game's folder in ProgramFiles.
pub const INSTALLED_PREFIX: &str = "didj-installed:";

/// A game's folder on the console, if `path` stands for an installed game.
pub fn installed_folder(path: &Path) -> Option<&str> {
    path.to_str()?.strip_prefix(INSTALLED_PREFIX)
}

/// Files of the player's: paths on /Didj, contents.
pub type Files = Vec<(String, Vec<u8>)>;

/// The Didj built from the packages in `dir`, and its save.
pub struct Virtual {
    dir: PathBuf,
    save: PathBuf,
    original: Nand,
    image: String,
}

impl Virtual {
    /// Builds the image (seconds: run it off the window's thread).
    pub fn open(dir: &Path, save_dir: &Path) -> anyhow::Result<Self> {
        if ribbit_didj::nand_dump_in(dir).map_err(anyhow::Error::msg)?.is_some() {
            anyhow::bail!(
                "this Didj starts from a real console's NAND dump (didj.nand or mtd0..mtd7 in {}); Ribbit leaves a dump as it is and installs only on a Didj built from its packages",
                dir.display()
            );
        }
        let (original, _) = ribbit_didj::nand_and_extras_missing(dir, true).map_err(anyhow::Error::msg)?;
        let image = format!("{:08x}", original.fingerprint());
        Ok(Self { dir: dir.to_path_buf(), save: crate::didj::save_file(dir, save_dir), original, image })
    }

    /// The player's files on the console: from the save applied to the
    /// image, or carried from one made on another image (`carry`).
    pub fn files(&self) -> anyhow::Result<Files> {
        let Ok(changes) = std::fs::read(&self.save) else { return Ok(Vec::new()) };
        let same = std::fs::read_to_string(self.save.with_extension("nandimage")).ok().as_deref() == Some(self.image.as_str());
        if same {
            let mut flash = self.original.clone();
            flash.apply_changes(&changes).map_err(anyhow::Error::msg)?;
            Ok(ribbit_didj::carry::files_beyond(&flash, &self.original))
        } else {
            ribbit_didj::carry::user_files(&changes, &self.original).map_err(anyhow::Error::msg)
        }
    }

    /// The image with `files` on it.
    fn build(&self, files: &[(String, Vec<u8>)]) -> anyhow::Result<Nand> {
        Ok(ribbit_didj::nand_with_files(&self.dir, true, files).map_err(anyhow::Error::msg)?.0)
    }

    /// Makes `files` the player's files: the image built with them, its
    /// changes the save. The save it had is kept as
    /// `.nandblocks.before-library` (the last one only).
    pub fn write(&self, files: &[(String, Vec<u8>)]) -> anyhow::Result<()> {
        let flash = self.build(files)?;
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

    /// The paths the packages put on /Didj (they stay as built).
    pub fn built(&self) -> std::collections::HashSet<String> {
        connect::built_paths(&self.original)
    }

    /// The whole console's flash with `files`, as a NAND dump in the real
    /// layout: the 256 MB data area, mtd0 to mtd7 end to end (what Ribbit
    /// boots as `didj.nand`), with the factory data a console has.
    pub fn dump(&self, files: &[(String, Vec<u8>)]) -> anyhow::Result<Vec<u8>> {
        let mut flash = self.build(files)?;
        flash.add_factory_data();
        Ok(flash.read(0, ribbit_didj::nand::SIZE))
    }
}

/// A game installed on the console, for the library.
pub struct Listed {
    pub game: crate::library::Game,
    /// What its panel in the Didj's menu shows (`AutoStart`).
    pub menu_art: Option<ribbit_didj::autostart::Art>,
}

/// The games installed on the console (not those built in: they are its
/// packages, listed from their files).
pub fn list(dir: &Path, save_dir: &Path) -> anyhow::Result<Vec<Listed>> {
    let v = Virtual::open(dir, save_dir)?;
    let files = v.files()?;
    Ok(listed(&files))
}

fn listed(files: &[(String, Vec<u8>)]) -> Vec<Listed> {
    use crate::library::{Family, Game};
    connect::installed(files)
        .into_iter()
        .map(|(folder, meta)| {
            let read = |f: &str| connect::game_file(files, &folder, f).map(<[u8]>::to_vec);
            let title = connect::display_name(&meta, &folder);
            let mut details = vec![("System", "Didj".to_string()), ("Where", format!("installed on the Didj, /Didj/ProgramFiles/{folder}")), ("Folder", folder.clone())];
            for (label, key) in [("Name", "Name"), ("Version", "Version"), ("Product", "ProductID"), ("Publisher", "Publisher")] {
                if let Some(v) = meta.get(key) {
                    details.push((label, v.to_string()));
                }
            }
            let saves = connect::game_saves(files, &folder).len();
            details.push(("Saves", if saves == 0 { "none yet".to_string() } else { format!("{saves} files") }));
            details.push(("Status", "early emulation: Ribbit starts it from the Didj's menu (a console set up with a profile), in the mode chosen in Emulation > Didj".to_string()));
            let game = Game {
                path: PathBuf::from(format!("{INSTALLED_PREFIX}{folder}")),
                family: Family::Didj,
                title,
                subtitle: meta.get("Version").map_or(String::new(), |v| format!("version {v}")),
                details,
                tags: vec!["Installed".into()],
                key: format!("{}{}", crate::library::INSTALLED_KEY, folder.to_ascii_lowercase()),
                art: crate::didj::card_art(&meta, read),
            };
            Listed { menu_art: crate::didj::menu_art_from(&meta, read), game }
        })
        .collect()
}

/// The game in the file at `path`: an application package (.lfp), or a
/// cartridge dump (raw NAND, its FAT volume, zipped or not).
pub fn read_game(path: &Path) -> anyhow::Result<connect::Game> {
    let data = std::fs::read(path)?;
    connect::game_from_file(&data).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))
}

/// Installs the game at `path` on the console in `dir`. Its name, folder
/// and menu art.
pub fn install(dir: &Path, save_dir: &Path, path: &Path) -> anyhow::Result<(String, String, Option<ribbit_didj::autostart::Art>)> {
    let game = read_game(path)?;
    let v = Virtual::open(dir, save_dir)?;
    if v.built().contains(&format!("programfiles/{}/meta.inf", game.folder.to_ascii_lowercase())) {
        anyhow::bail!("{} is built into this Didj already (its package is in {})", game.name(), dir.display());
    }
    let mut files = v.files()?;
    connect::install(&mut files, &game);
    v.write(&files)?;
    let read = |f: &str| connect::game_file(&files, &game.folder, f).map(<[u8]>::to_vec);
    let art = crate::didj::menu_art_from(&game.meta, read);
    Ok((game.name(), game.folder, art))
}

/// Uninstalls the game in ProgramFiles/`folder`, its saves too if asked.
pub fn uninstall(dir: &Path, save_dir: &Path, folder: &str, delete_saves: bool) -> anyhow::Result<usize> {
    let v = Virtual::open(dir, save_dir)?;
    let mut files = v.files()?;
    let n = connect::uninstall(&mut files, folder, delete_saves);
    if n == 0 {
        anyhow::bail!("no game is installed in /Didj/ProgramFiles/{folder}");
    }
    v.write(&files)?;
    Ok(n)
}

/// What to tell a player about copying an exported folder to a real Didj.
const EXPORT_README: &str = "\
These are the files of Ribbit's Didj in a real Didj's layout (its /Didj drive):
  Data/<0,1,2>/        profiles (profile.dsc, profile.log) and each game's progress
  Data/Avatars/        Didjis
  Data/GameIcons/      the menu's game icons
  ProgramFiles/<game>  games installed in Ribbit (not those built into the Didj)
The Didj's system (Base/) is left out on purpose: never copy system files to a real Didj.

UNTESTED ON HARDWARE. Copying to a real Didj (see Ribbit's manual, \"Didj: library, installing, real Didjs\"):
 1. Back up the real Didj first: copy everything on its drive to your PC.
 2. Plug the Didj into the PC with LeapFrog Connect installed and running; the Didj
    shows its USB screen and Connect opens its drive (Windows: a drive letter, or the
    Didj's folder under Connect's Mnt folder).
 3. Copy the folders here (Data, ProgramFiles) onto the Didj's drive, replacing files.
    A profile replaces the real Didj's profile with the same number.
 4. Close Connect (or eject the drive) before unplugging the Didj.
";

/// Copies the player's files to `out`/Didj in a real Didj's layout (and a
/// read-me on copying them to one). How many files.
pub fn export(dir: &Path, save_dir: &Path, out: &Path) -> anyhow::Result<(usize, PathBuf)> {
    let v = Virtual::open(dir, save_dir)?;
    let files = v.files()?;
    let to = out.join("Didj");
    let n = connect::write_folder(&to, &files).map_err(anyhow::Error::msg)?;
    std::fs::write(out.join("READ ME - copying to a real Didj.txt"), EXPORT_README.replace('\n', "\r\n"))?;
    Ok((n, to))
}

/// Writes the console (packages and the player's files) to `out`/didj.nand,
/// a NAND dump in the real layout.
pub fn export_nand(dir: &Path, save_dir: &Path, out: &Path) -> anyhow::Result<PathBuf> {
    let v = Virtual::open(dir, save_dir)?;
    let files = v.files()?;
    let to = out.join("didj.nand");
    std::fs::write(&to, v.dump(&files)?)?;
    Ok(to)
}

/// Merges a real Didj's files into the console: a folder copied off its
/// drive, or a NAND dump of it (didj.nand, 256 MB, with or without spare
/// areas). Profiles, Didjis, game saves and installed games come in; the
/// system's files don't. How many files came in.
pub fn import(dir: &Path, save_dir: &Path, from: &Path) -> anyhow::Result<usize> {
    let v = Virtual::open(dir, save_dir)?;
    let incoming = if from.is_dir() {
        connect::read_folder(from).map_err(anyhow::Error::msg)?
    } else {
        let dump = Nand::from_dump(&std::fs::read(from)?).map_err(|e| anyhow::anyhow!("{}: {e}", from.display()))?;
        ribbit_didj::carry::files_beyond(&dump, &Nand::erased())
    };
    let mut files = v.files()?;
    let n = connect::merge(&mut files, incoming, &v.built());
    if n == 0 {
        anyhow::bail!("{} holds no profiles, Didjis, saves or games to bring in", from.display());
    }
    v.write(&files)?;
    Ok(n)
}

/// Work on the console's save, run on a worker thread (`run`).
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// The games installed on it, for the library.
    List,
    /// Install the game in this file; start it after (`play`).
    Install { path: PathBuf, play: bool },
    Uninstall { folder: String, title: String, delete_saves: bool },
    /// The player's files to this folder, in a real Didj's layout.
    Export(PathBuf),
    /// The console as a NAND dump in this folder.
    ExportNand(PathBuf),
    /// A real Didj's files from this folder (a copy of its drive, or a
    /// folder holding its didj.nand).
    Import(PathBuf),
}

impl Job {
    /// Whether it changes the save (the Didj is turned off first).
    pub fn writes(&self) -> bool {
        matches!(self, Job::Install { .. } | Job::Uninstall { .. } | Job::Import(_))
    }
}

/// What a job came to.
pub enum Done {
    /// The games installed on the console in `dir` (the Explorer's with
    /// `explorer`).
    Listed { dir: PathBuf, explorer: bool, games: Vec<Listed> },
    Installed { name: String, folder: String, art: Option<ribbit_didj::autostart::Art>, play: bool, explorer: bool },
    Uninstalled { title: String, delete_saves: bool },
    Exported { files: usize, to: PathBuf },
    ExportedNand(PathBuf),
    Imported(usize),
    Failed { title: &'static str, explain: String, error: String },
}

/// Runs `job` on the console built from the packages in `dir`.
pub fn run(job: Job, dir: &Path, save_dir: &Path) -> Done {
    let failed = |title: &'static str, explain: &str, e: anyhow::Error| Done::Failed { title, explain: explain.to_string(), error: format!("{e:#}") };
    match job {
        Job::List => match list(dir, save_dir) {
            Ok(games) => Done::Listed { dir: dir.to_path_buf(), explorer: false, games },
            Err(e) => {
                tracing::warn!(target: "ribbit::didj", "listing the games installed on the Didj: {e:#}");
                Done::Listed { dir: dir.to_path_buf(), explorer: false, games: Vec::new() }
            }
        },
        Job::Install { path, play } => match install(dir, save_dir, &path) {
            Ok((name, folder, art)) => Done::Installed { name, folder, art, play, explorer: false },
            Err(e) => failed(
                "Couldn't install the game",
                "Ribbit installs a Didj game from its application package (.lfp, as LeapFrog Connect downloads it) or a cartridge dump, onto the Didj built from the packages in its folder.",
                e,
            ),
        },
        Job::Uninstall { folder, title, delete_saves } => match uninstall(dir, save_dir, &folder, delete_saves) {
            Ok(_) => Done::Uninstalled { title, delete_saves },
            Err(e) => failed("Couldn't uninstall the game", "", e),
        },
        Job::Export(out) => match export(dir, save_dir, &out) {
            Ok((files, to)) => Done::Exported { files, to },
            Err(e) => failed("Couldn't copy the Didj's files", "", e),
        },
        Job::ExportNand(out) => match export_nand(dir, save_dir, &out) {
            Ok(to) => Done::ExportedNand(to),
            Err(e) => failed("Couldn't write the NAND image", "", e),
        },
        Job::Import(from) => {
            // A folder holding a NAND dump (didj.nand) and no copy of a drive.
            let dump = from.join("didj.nand");
            let drive = |d: &Path| ["Data", "ProgramFiles"].iter().any(|f| d.join(f).is_dir());
            let from = if !drive(&from) && !drive(&from.join("Didj")) && dump.is_file() { dump } else { from };
            match import(dir, save_dir, &from) {
                Ok(n) => Done::Imported(n),
                Err(e) => failed(
                    "Couldn't import from the Didj",
                    "Choose the folder you copied a real Didj's drive into (it holds Data and ProgramFiles, or a Didj folder holding them), or a folder holding its NAND dump as didj.nand.",
                    e,
                ),
            }
        }
    }
}
