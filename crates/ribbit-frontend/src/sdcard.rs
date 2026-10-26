//! The Leapster 2's SD card from this PC, without a card reader or USB.
//!
//! * The SD card folder (`Folder`), as the RockIt Twist's content folder: the
//!   card is made from a folder on this PC each time the Leapster 2 starts,
//!   and what the console writes to the card is written back to the folder.
//! * A card image loaded (a real card's raw image or a Ribbit card file), the
//!   card's files copied out, a freshly formatted card.
//!
//! The card is `io::sd::SdCard`'s sectors; `fatfs` reads and writes its FAT
//! volume as a PC would through a card reader.

use anyhow::{Context, Result, bail};
use ribbit_core::io::sd::{CARD_SECTORS, SECTOR, SdCard};
use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;

pub type Sectors = BTreeMap<u32, Vec<u8>>;

/// What to do to the card (Emulation › Leapster 2 SD card) when no SD card
/// folder is used.
#[derive(Debug, Clone, PartialEq)]
pub enum Job {
    /// Replace the card with an image.
    LoadImage(PathBuf),
    /// Copy the card's files into a folder.
    Export(PathBuf),
    /// Replace the card with a freshly formatted one.
    Empty,
}

/// Files a PC's file manager leaves in folders; not copied onto the card.
const SKIP: [&str; 3] = ["desktop.ini", "thumbs.db", ".ds_store"];

/// Sectors read, noted for `Folder` (which sectors hold which file).
type Log = Rc<RefCell<BTreeSet<u32>>>;

/// A FAT volume on sparse 512-byte sectors (absent sectors read as zeros),
/// starting `base` bytes into the card. Read only: writes are dropped.
struct Disk<'a> {
    sectors: &'a mut Sectors,
    base: u64,
    size: u64,
    pos: u64,
    read_only: bool,
    /// Notes the sectors read from `log_from` on (the data area).
    log: Option<(Log, u32)>,
}

impl Read for Disk<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut done = 0;
        let end = buf.len().min(self.size.saturating_sub(self.pos) as usize);
        while done < end {
            let p = self.base + self.pos;
            let (n, off) = ((p / SECTOR as u64) as u32, (p % SECTOR as u64) as usize);
            let len = (SECTOR - off).min(end - done);
            match self.sectors.get(&n) {
                Some(s) => buf[done..done + len].copy_from_slice(&s[off..off + len]),
                None => buf[done..done + len].fill(0),
            }
            if let Some((log, from)) = &self.log
                && n >= *from
            {
                log.borrow_mut().insert(n);
            }
            done += len;
            self.pos += len as u64;
        }
        Ok(done)
    }
}

impl Write for Disk<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let end = buf.len().min(self.size.saturating_sub(self.pos) as usize);
        if self.read_only {
            self.pos += end as u64;
            return Ok(end);
        }
        let mut done = 0;
        while done < end {
            let p = self.base + self.pos;
            let (n, off) = ((p / SECTOR as u64) as u32, (p % SECTOR as u64) as usize);
            let len = (SECTOR - off).min(end - done);
            let data = &buf[done..done + len];
            // Zeros over a sector that holds none need no storage.
            if self.sectors.contains_key(&n) || data.iter().any(|&b| b != 0) {
                self.sectors.entry(n).or_insert_with(|| vec![0; SECTOR])[off..off + len].copy_from_slice(data);
            }
            done += len;
            self.pos += len as u64;
        }
        Ok(done)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Seek for Disk<'_> {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        self.pos = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::End(d) => self.size.saturating_add_signed(d),
            SeekFrom::Current(d) => self.pos.saturating_add_signed(d),
        };
        Ok(self.pos)
    }
}

/// A freshly formatted card, as the emulated Leapster 2 starts with (io/fat.rs).
pub fn fresh() -> Sectors {
    ribbit_core::io::fat::format_fat32(CARD_SECTORS).into_iter().collect()
}

/// The first sector of the FAT volume: the first partition's if sector 0 is
/// a partition table (as Windows formats a card), else 0 (as the BIOS does).
fn volume_start(sectors: &Sectors) -> u32 {
    match sectors.get(&0) {
        Some(s) if s[510] == 0x55 && s[511] == 0xAA && s[0] != 0xEB && s[0] != 0xE9 && s[0x1BE + 4] != 0 => {
            u32::from_le_bytes([s[0x1C6], s[0x1C7], s[0x1C8], s[0x1C9]])
        }
        _ => 0,
    }
}

/// The volume's first data sector (after its reserved sectors, FATs and,
/// on FAT12/16, root directory), from its boot sector.
fn data_start(sectors: &Sectors) -> u32 {
    let start = volume_start(sectors);
    let Some(b) = sectors.get(&start) else {
        return start;
    };
    let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u32;
    let fat = match u16_at(22) {
        0 => u32::from_le_bytes([b[36], b[37], b[38], b[39]]),
        n => n,
    };
    start + u16_at(14) + b[16] as u32 * fat + (u16_at(17) * 32).div_ceil(SECTOR as u32)
}

/// Runs `f` on the card's FAT volume (`total` sectors on the card).
fn with_volume<R>(
    sectors: &mut Sectors,
    total: u64,
    read_only: bool,
    log: Option<(Log, u32)>,
    f: impl FnOnce(&fatfs::Dir<'_, &mut Disk<'_>>) -> Result<R>,
) -> Result<R> {
    let start = volume_start(sectors) as u64;
    let mut disk = Disk {
        sectors,
        base: start * SECTOR as u64,
        size: total.saturating_sub(start) * SECTOR as u64,
        pos: 0,
        read_only,
        log,
    };
    let fs = fatfs::FileSystem::new(&mut disk, fatfs::FsOptions::new())
        .context("the card has no FAT volume Ribbit can read (Empty the card makes a new one)")?;
    let out = f(&fs.root_dir())?;
    if !read_only {
        fs.unmount().context("finishing the card")?;
    }
    Ok(out)
}

/// The directory `name` in `dir`, made if it isn't there.
fn dir_in<'a, T: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<'a, T>, name: &str) -> Result<fatfs::Dir<'a, T>> {
    match dir.open_dir(name) {
        Ok(d) => Ok(d),
        Err(_) => dir.create_dir(name).with_context(|| format!("making the folder {name} on the card")),
    }
}

/// Writes `data` as `name` in `dir` on the card, replacing a file of that name.
fn put<T: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<'_, T>, name: &str, data: &[u8]) -> Result<()> {
    // A full card fails here: say how big it is.
    let full = |e: std::io::Error| anyhow::anyhow!("writing {name} onto the card: {e} (the card holds 2 GB)");
    let mut f = dir.create_file(name).map_err(full)?;
    f.truncate().map_err(full)?;
    f.write_all(data).map_err(full)
}

/// Copies `from`'s files and folders into `to` on the card, replacing files
/// with the same names; the number of files.
fn copy_in<T: fatfs::ReadWriteSeek>(from: &Path, to: &fatfs::Dir<'_, T>) -> Result<usize> {
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .with_context(|| format!("reading {}", from.display()))?
        .collect::<std::io::Result<_>>()
        .with_context(|| format!("reading {}", from.display()))?;
    entries.sort_by_key(|e| e.file_name());
    let mut files = 0;
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        if SKIP.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            files += copy_in(&path, &dir_in(to, &name)?)?;
        } else {
            put(to, &name, &std::fs::read(&path).with_context(|| format!("reading {}", path.display()))?)?;
            files += 1;
        }
    }
    Ok(files)
}

/// A file or folder on the card.
struct Entry {
    /// From the root, '/'-separated.
    path: String,
    is_dir: bool,
    len: u64,
    /// Its dates, to see it rewritten.
    stamp: String,
}

/// Every file and folder under `dir` on the card, parents first.
fn walk<T: fatfs::ReadWriteSeek>(dir: &fatfs::Dir<'_, T>, prefix: &str, out: &mut Vec<Entry>) -> Result<()> {
    for e in dir.iter() {
        let e = e.context("reading the card's folders")?;
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let path = format!("{prefix}{name}");
        out.push(Entry { path: path.clone(), is_dir: e.is_dir(), len: e.len(), stamp: format!("{:?} {:?}", e.created(), e.modified()) });
        if e.is_dir() {
            walk(&e.to_dir(), &format!("{path}/"), out)?;
        }
    }
    Ok(())
}

fn read_all<T: fatfs::ReadWriteSeek>(root: &fatfs::Dir<'_, T>, path: &str) -> Result<Vec<u8>> {
    let mut data = Vec::new();
    root.open_file(path)
        .and_then(|mut f| f.read_to_end(&mut data))
        .with_context(|| format!("reading {path} from the card"))?;
    Ok(data)
}

/// The PC path of a card path in `dir`.
fn on_pc(dir: &Path, path: &str) -> PathBuf {
    path.split('/').fold(dir.to_path_buf(), |p, part| p.join(part))
}

/// Copies the card's files into `dir`; the number of files.
pub fn export(sectors: &mut Sectors, dir: &Path) -> Result<usize> {
    with_volume(sectors, CARD_SECTORS as u64, true, None, |root| {
        let mut entries = Vec::new();
        walk(root, "", &mut entries)?;
        std::fs::create_dir_all(dir).with_context(|| format!("making {}", dir.display()))?;
        let mut files = 0;
        for e in entries {
            let to = on_pc(dir, &e.path);
            if e.is_dir {
                std::fs::create_dir_all(&to).with_context(|| format!("making {}", to.display()))?;
            } else {
                std::fs::write(&to, read_all(root, &e.path)?).with_context(|| format!("writing {}", to.display()))?;
                files += 1;
            }
        }
        Ok(files)
    })
}

/// Copies one card's directory into another's; the number of files.
fn copy_across<A: fatfs::ReadWriteSeek, B: fatfs::ReadWriteSeek>(from: &fatfs::Dir<'_, A>, to: &fatfs::Dir<'_, B>) -> Result<usize> {
    let mut files = 0;
    for e in from.iter() {
        let e = e.context("reading the image's folders")?;
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        if e.is_dir() {
            files += copy_across(&e.to_dir(), &dir_in(to, &name)?)?;
        } else {
            let mut data = Vec::with_capacity(e.len() as usize);
            e.to_file().read_to_end(&mut data).with_context(|| format!("reading {name} from the image"))?;
            put(to, &name, &data)?;
            files += 1;
        }
    }
    Ok(files)
}

/// A card from an image file: a Ribbit card file (`sdcard-*.img`), or the raw
/// image of a card (Win32 Disk Imager, `dd`). An image of a card up to 2 GB
/// becomes the card sector for sector; a larger card's files are copied onto
/// a fresh 2 GB card (the emulated card's size). Also says whether files were
/// copied.
pub fn load_image(path: &Path) -> Result<(Sectors, bool)> {
    let mut file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let len = file.metadata()?.len();
    let mut magic = [0u8; 8];
    if file.read_exact(&mut magic).is_ok() && &magic == b"RBSPARSE" {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let mut s = ribbit_core::io::sparse::decode(&bytes, SECTOR).context("the Ribbit card file is damaged")?;
        s.retain(|&n, _| n < CARD_SECTORS);
        return Ok((s, false));
    }
    if len == 0 || len % SECTOR as u64 != 0 {
        bail!("{} is not an SD card image (its size is not whole 512-byte sectors)", path.display());
    }
    // Only the sectors holding data are kept.
    file.seek(SeekFrom::Start(0))?;
    let mut sectors = Sectors::new();
    let mut chunk = vec![0u8; 1 << 20];
    let mut n = 0u32;
    loop {
        let got = read_full(&mut file, &mut chunk).with_context(|| format!("reading {}", path.display()))?;
        for s in chunk[..got].as_chunks::<SECTOR>().0 {
            if s.iter().any(|&b| b != 0) {
                sectors.insert(n, s.to_vec());
            }
            n += 1;
        }
        if got < chunk.len() {
            break;
        }
    }
    let total = len / SECTOR as u64;
    // A FAT volume must be there (this also rejects files that aren't cards).
    with_volume(&mut sectors, total, true, None, |_| Ok(()))
        .with_context(|| format!("{} is not an SD card image", path.display()))?;
    if total <= CARD_SECTORS as u64 {
        return Ok((sectors, false));
    }
    let mut card = fresh();
    with_volume(&mut sectors, total, true, None, |from| {
        with_volume(&mut card, CARD_SECTORS as u64, false, None, |to| copy_across(from, to))
    })?;
    Ok((card, true))
}

/// Reads until `buf` is full or the file ends; the bytes read.
fn read_full(file: &mut std::fs::File, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut got = 0;
    while got < buf.len() {
        match file.read(&mut buf[got..])? {
            0 => break,
            n => got += n,
        }
    }
    Ok(got)
}

/// The SD card folder: the card is made from it when the Leapster 2 starts,
/// and what the console writes to the card goes back into it (`sync`).
#[derive(Debug)]
pub struct Folder {
    pub dir: PathBuf,
    seen: Mutex<Seen>,
}

/// The card as the folder last had it.
#[derive(Debug, Default)]
struct Seen {
    files: BTreeMap<String, File>,
    dirs: BTreeSet<String>,
    /// Data sectors of the directories.
    dir_sectors: BTreeSet<u32>,
}

#[derive(Debug, Clone)]
struct File {
    len: u64,
    stamp: String,
    /// The data sectors it was read from.
    sectors: BTreeSet<u32>,
}

impl Folder {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir, seen: Mutex::default() }
    }

    /// Makes `card` a freshly formatted card holding the folder's files; the
    /// number of files.
    pub fn load(&self, card: &mut SdCard) -> Result<usize> {
        let mut sectors = fresh();
        let n = with_volume(&mut sectors, CARD_SECTORS as u64, false, None, |root| {
            let n = copy_in(&self.dir, root)?;
            // B5 formats a card without it at boot (0x400C_E8C4), as a real
            // one would, so the folder's files would be lost.
            dir_in(root, "Leapster")?;
            Ok(n)
        })
        .with_context(|| format!("the SD card folder {}", self.dir.display()))?;
        self.scan(&mut sectors, None, false)?;
        card.sectors = sectors;
        card.changed.clear();
        card.dirty = false;
        card.synced = true;
        Ok(n)
    }

    /// Writes what the console changed on the card into the folder: files
    /// written or added, and files and folders deleted. The number of files
    /// written or removed.
    pub fn sync(&self, card: &mut SdCard) -> Result<usize> {
        // A card from elsewhere (a save state), or one the console formatted
        // (its boot sector written): every file is compared, and nothing is
        // deleted, as the folder may hold files the card no longer has.
        let formatted = card.changed.contains(&volume_start(&card.sectors));
        let changed = (card.synced && !formatted).then_some(&card.changed);
        let n = self.scan(&mut card.sectors, changed, true)?;
        card.changed.clear();
        card.dirty = false;
        card.synced = true;
        Ok(n)
    }

    /// Notes what the card holds and, with `write`, puts it in the folder.
    /// `changed`: the sectors written since the last scan (None: unknown).
    fn scan(&self, sectors: &mut Sectors, changed: Option<&BTreeSet<u32>>, write: bool) -> Result<usize> {
        let mut seen = self.seen.lock().unwrap();
        let log: Log = Rc::default();
        let from = data_start(sectors);
        with_volume(sectors, CARD_SECTORS as u64, true, Some((log.clone(), from)), |root| {
            let mut entries = Vec::new();
            walk(root, "", &mut entries)?;
            let dir_sectors = log.take();
            // Data written where no known file or folder was: a new file, or
            // one written to new clusters at the same size and dates. Every
            // file is compared then.
            let all = changed.is_none_or(|c| {
                c.iter().any(|n| {
                    *n >= from
                        && !dir_sectors.contains(n)
                        && !seen.dir_sectors.contains(n)
                        && !seen.files.values().any(|f| f.sectors.contains(n))
                })
            });
            let mut now = Seen { dir_sectors, ..Default::default() };
            let mut done = 0;
            for e in &entries {
                let to = on_pc(&self.dir, &e.path);
                if e.is_dir {
                    if write {
                        std::fs::create_dir_all(&to).with_context(|| format!("making {}", to.display()))?;
                    }
                    now.dirs.insert(e.path.clone());
                    continue;
                }
                let old = seen.files.get(&e.path);
                let same = old.is_some_and(|o| {
                    o.len == e.len && o.stamp == e.stamp && !changed.is_some_and(|c| o.sectors.iter().any(|n| c.contains(n)))
                });
                if let (false, Some(old)) = (all, old.filter(|_| same)) {
                    now.files.insert(e.path.clone(), old.clone());
                    continue;
                }
                log.borrow_mut().clear();
                let data = read_all(root, &e.path)?;
                let sectors = log.take().into_iter().filter(|n| !now.dir_sectors.contains(n)).collect();
                if write && std::fs::read(&to).ok().as_deref() != Some(&data[..]) {
                    std::fs::write(&to, &data).with_context(|| format!("writing {}", to.display()))?;
                    done += 1;
                }
                now.files.insert(e.path.clone(), File { len: e.len, stamp: e.stamp.clone(), sectors });
            }
            if write && changed.is_some() {
                for path in seen.files.keys().filter(|p| !now.files.contains_key(*p)) {
                    let to = on_pc(&self.dir, path);
                    if to.is_file() {
                        std::fs::remove_file(&to).with_context(|| format!("deleting {}", to.display()))?;
                        done += 1;
                    }
                }
                // Deepest first; a folder still holding files of the PC's stays.
                for path in seen.dirs.iter().rev().filter(|p| !now.dirs.contains(*p)) {
                    let _ = std::fs::remove_dir(on_pc(&self.dir, path));
                }
            }
            *seen = now;
            Ok(done)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ribbit-sdcard-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Writes a file on the card as the console would, noting the sectors.
    fn console_writes(card: &mut SdCard, path: &str, data: Option<&[u8]>) {
        let before = card.sectors.clone();
        with_volume(&mut card.sectors, CARD_SECTORS as u64, false, None, |root| {
            let (dir, name) = path.rsplit_once('/').map_or((None, path), |(d, n)| (Some(d), n));
            let dir = match dir {
                Some(d) => d.split('/').try_fold(root.clone(), |p, part| dir_in(&p, part))?,
                None => root.clone(),
            };
            match data {
                Some(data) => put(&dir, name, data),
                None => dir.remove(name).map_err(Into::into),
            }
        })
        .unwrap();
        for (n, s) in &card.sectors {
            if before.get(n) != Some(s) {
                card.changed.insert(*n);
            }
        }
        card.dirty = true;
    }

    #[test]
    fn the_folder_becomes_the_card_and_gets_the_consoles_writes() {
        let dir = temp("folder");
        std::fs::create_dir_all(dir.join("Leapster/Games")).unwrap();
        std::fs::write(dir.join("Leapster/Games/GAME.BIN"), vec![0x5A; 70_000]).unwrap();
        std::fs::write(dir.join("A long file name.txt"), b"hello").unwrap();
        std::fs::write(dir.join("OLD.LOG"), b"old").unwrap();
        std::fs::write(dir.join("Thumbs.db"), b"skip me").unwrap();
        let folder = Folder::new(dir.clone());
        let mut card = SdCard::default();
        assert_eq!(folder.load(&mut card).unwrap(), 3);
        // The volume is still the one the BIOS formats (boot sector kept).
        assert_eq!(card.sectors[&0], fresh()[&0]);
        let game = std::fs::metadata(dir.join("Leapster/Games/GAME.BIN")).unwrap().modified().unwrap();

        // The console makes a profile, changes a file in place and deletes one.
        console_writes(&mut card, "Leapster/Profiles/USER0.DAT", Some(&[1; 3000]));
        console_writes(&mut card, "A long file name.txt", Some(b"HELLO"));
        console_writes(&mut card, "OLD.LOG", None);
        assert_eq!(folder.sync(&mut card).unwrap(), 3);
        assert_eq!(std::fs::read(dir.join("Leapster/Profiles/USER0.DAT")).unwrap(), vec![1; 3000]);
        assert_eq!(std::fs::read(dir.join("A long file name.txt")).unwrap(), b"HELLO");
        assert!(!dir.join("OLD.LOG").exists());
        assert!(dir.join("Thumbs.db").exists(), "the PC's own files stay");
        // Files the console didn't touch aren't rewritten.
        assert_eq!(std::fs::metadata(dir.join("Leapster/Games/GAME.BIN")).unwrap().modified().unwrap(), game);
        assert!(!card.dirty && card.changed.is_empty());

        // Rewritten at the same size into new clusters: still seen.
        console_writes(&mut card, "Leapster/Profiles/USER0.DAT", None);
        console_writes(&mut card, "Leapster/Profiles/USER0.DAT", Some(&[2; 3000]));
        folder.sync(&mut card).unwrap();
        assert_eq!(std::fs::read(dir.join("Leapster/Profiles/USER0.DAT")).unwrap(), vec![2; 3000]);

        // The next start brings it all back.
        let mut again = SdCard::default();
        Folder::new(dir.clone()).load(&mut again).unwrap();
        let out = temp("folder-out");
        export(&mut again.sectors, &out).unwrap();
        assert_eq!(std::fs::read(out.join("Leapster/Profiles/USER0.DAT")).unwrap(), vec![2; 3000]);
        assert_eq!(std::fs::read(out.join("Leapster/Games/GAME.BIN")).unwrap(), vec![0x5A; 70_000]);
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&out);
    }

    /// A real Leapster 2 BIOS (0.6 or Connected B5) starts from a card made
    /// from a folder and keeps its files (B5 formats a card without
    /// b:\Leapster). RIBBIT_L2_SD / RIBBIT_L2_FLASH: a saved card and flash.
    /// RIBBIT_L2_BIOS=path cargo test -p ribbit-frontend bios_uses -- --ignored
    #[test]
    #[ignore = "needs a Leapster 2 BIOS (RIBBIT_L2_BIOS)"]
    fn a_bios_uses_the_folder() {
        let bios = std::env::var("RIBBIT_L2_BIOS").expect("RIBBIT_L2_BIOS");
        let dir = temp("bios");
        std::fs::write(dir.join("MINE.TXT"), b"mine").unwrap();
        // The console's saved card and internal flash (read only), if given.
        if let Ok(p) = std::env::var("RIBBIT_L2_SD") {
            export(&mut load_image(Path::new(&p)).unwrap().0, &dir).unwrap();
        }
        let fw = ribbit_memory::Firmware::from_bytes(std::fs::read(bios).unwrap()).unwrap();
        let mut m = ribbit_core::Machine::with_straps(fw, None, 0x63FF_FFFF);
        if let Ok(p) = std::env::var("RIBBIT_L2_FLASH") {
            m.io.spi_flash.as_mut().unwrap().load(&std::fs::read(p).unwrap());
        }
        let folder = Folder::new(dir.clone());
        folder.load(m.io.sd.as_mut().unwrap().card.as_mut().unwrap()).unwrap();
        m.boot_leapster2_system();
        m.ensure_touch_calibration();
        for _ in 0..30 {
            m.run_cycles(96_000 * 1000, &Default::default());
        }
        let card = m.io.sd.as_mut().unwrap().card.as_mut().unwrap();
        // Not formatted (B5 formats a card without b:\Leapster).
        assert!(!card.changed.contains(&0), "the card was formatted");
        let n = folder.sync(card).unwrap();
        let mut found = Vec::new();
        fn list(d: &Path, out: &mut Vec<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                out.push(e.path().display().to_string());
                if e.path().is_dir() {
                    list(&e.path(), out);
                }
            }
        }
        list(&dir, &mut found);
        println!("{n} written: {found:#?}");
        assert!(dir.join("MINE.TXT").exists() && dir.join("Leapster").is_dir());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_card_from_a_save_state_deletes_nothing() {
        let dir = temp("state");
        std::fs::write(dir.join("KEEP.TXT"), b"keep").unwrap();
        let folder = Folder::new(dir.clone());
        let mut card = SdCard::default();
        folder.load(&mut card).unwrap();
        // A state from before KEEP.TXT was there.
        card.sectors = fresh();
        card.synced = false;
        card.dirty = true;
        folder.sync(&mut card).unwrap();
        assert!(dir.join("KEEP.TXT").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn images_load_raw_and_as_ribbit_card_files() {
        let dir = temp("image");
        let src = dir.join("in");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("SAVE.DAT"), vec![7u8; 1234]).unwrap();
        let mut card = SdCard::default();
        Folder::new(src).load(&mut card).unwrap();
        let card = card.sectors;
        // Ribbit's own card file.
        let ribbit = dir.join("sdcard-0.img");
        std::fs::write(&ribbit, ribbit_core::io::sparse::encode(&card, SECTOR)).unwrap();
        assert_eq!(load_image(&ribbit).unwrap(), (card.clone(), false));
        // A raw image of a smaller card (64 MB): taken sector for sector.
        let raw = dir.join("card.img");
        let mut bytes = vec![0u8; 64 << 20];
        for (n, s) in &card {
            if (*n as usize + 1) * SECTOR <= bytes.len() {
                bytes[*n as usize * SECTOR..][..SECTOR].copy_from_slice(s);
            }
        }
        std::fs::write(&raw, &bytes).unwrap();
        let (mut loaded, copied) = load_image(&raw).unwrap();
        assert!(!copied);
        let out = dir.join("out");
        export(&mut loaded, &out).unwrap();
        assert_eq!(std::fs::read(out.join("SAVE.DAT")).unwrap(), vec![7u8; 1234]);
        // Not a card.
        let junk = dir.join("junk.img");
        std::fs::write(&junk, vec![1u8; 4096]).unwrap();
        assert!(load_image(&junk).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
