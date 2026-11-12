//! Carrying a player's files over to another NAND image: profiles, game
//! progress, settings and whatever LeapFrog Connect sent (`/Data/0/...`,
//! `/Base/settings.cfg`), when the image the save was made on is not the
//! one built now (Ribbit built it differently, other packages, a save from
//! another PC).
//!
//! A save holds only the erase blocks the console changed (`Nand::changes_from`),
//! and the image it was made against can't be built again. Those blocks are
//! enough: the Brio volume's LEBs the console wrote are among them (UBI
//! writes a LEB anew elsewhere, with a higher sequence number), and the
//! console wrote every FAT sector, folder and cluster of a file it made.
//! So the files read whole from those LEBs alone are the console's, and
//! those the new image doesn't have are carried over (VERIFIED 2026-10-31
//! on Ribbit's own saves: profiles 0-2, their game data and backups,
//! settings.cfg, the game icons).

use crate::nand::{self, Nand};
use crate::{brio, ubi};
use ribbit_core::fatfs;

/// The files in a save's changes (`Nand::changes_from` output) to carry
/// onto `base`, a newly built image: the volume's files read whole from
/// the LEBs the save holds, less those at paths `base` has (the packages'
/// files stay as built).
pub fn user_files(changes: &[u8], base: &Nand) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut saved = Nand::erased();
    saved.apply_changes(changes)?;
    let brio = nand::partition("Brio").unwrap();
    let len = brio::volume_sectors(brio.size / nand::BLOCK) * 512;
    let base_volume = ubi::read_volume(base, brio, len);
    let lebs = ubi::lebs(&saved, brio, len.div_ceil(ubi::LEB));
    // The new image's volume with the save's LEBs over it: its boot sector
    // (the same layout in every Ribbit) when the save has none.
    let mut volume = base_volume.clone();
    for (leb, block) in lebs.iter().enumerate() {
        if let Some(b) = block {
            let at = leb * ubi::LEB;
            let n = (len - at).min(ubi::LEB);
            volume[at..at + n].copy_from_slice(&saved.read(b * nand::BLOCK + ubi::DATA_OFFSET, n));
        }
    }
    let known = |r: std::ops::Range<usize>| {
        r.end <= 512 || (r.start / ubi::LEB..r.end.div_ceil(ubi::LEB)).all(|l| lebs.get(l).is_some_and(Option::is_some))
    };
    let files = fatfs::known_files(&volume, &known);
    let built: std::collections::HashSet<String> =
        fatfs::known_files(&base_volume, &|_| true).into_iter().map(|(p, _)| p.to_ascii_lowercase()).collect();
    Ok(files.into_iter().filter(|(p, _)| !built.contains(&p.to_ascii_lowercase())).collect())
}

/// The files on `flash`'s Brio volume at paths `base` (the image it was
/// built as) doesn't have: the player's, read from a whole image (the save
/// applied to `base`), so every block is known, unlike in `user_files`.
pub fn files_beyond(flash: &Nand, base: &Nand) -> Vec<(String, Vec<u8>)> {
    let brio = nand::partition("Brio").unwrap();
    let len = brio::volume_sectors(brio.size / nand::BLOCK) * 512;
    let built: std::collections::HashSet<String> =
        fatfs::known_files(&ubi::read_volume(base, brio, len), &|_| true).into_iter().map(|(p, _)| p.to_ascii_lowercase()).collect();
    fatfs::known_files(&ubi::read_volume(flash, brio, len), &|_| true).into_iter().filter(|(p, _)| !built.contains(&p.to_ascii_lowercase())).collect()
}

/// The player's files (`files_beyond`) kept beside a save (`.files`), for
/// the day the save lands on another image: `user_files` can only read
/// back files whose folders the save's blocks hold too, and a save the PC
/// wrote over USB (Windows leaves the folders above a file as they were)
/// or Enhanced (new) wrote (a volume built afresh) may hold no such
/// folders: adding micromods then lost every profile (VERIFIED 2026-11-12:
/// `user_files` gave nothing for an Enhanced (new) save). `save` is the
/// save's contents: the list is only taken back with that save.
pub fn pack(files: &[(String, Vec<u8>)], save: &[u8]) -> Vec<u8> {
    let mut out = b"RIBBIT-DIDJ-FILES1".to_vec();
    out.extend(crc32fast::hash(save).to_le_bytes());
    for (p, d) in files {
        out.extend((p.len() as u32).to_le_bytes());
        out.extend(p.as_bytes());
        out.extend((d.len() as u32).to_le_bytes());
        out.extend(d);
    }
    out
}

/// The files `pack` kept with `save`; None if they were kept with another
/// save or are damaged.
pub fn unpack(packed: &[u8], save: &[u8]) -> Option<Vec<(String, Vec<u8>)>> {
    let mut rest = packed.strip_prefix(b"RIBBIT-DIDJ-FILES1".as_slice())?;
    let mut take = |n: usize| {
        let (a, b) = rest.split_at_checked(n)?;
        rest = b;
        Some(a)
    };
    let word = |b: &[u8]| u32::from_le_bytes(b.try_into().unwrap()) as usize;
    if word(take(4)?) as u32 != crc32fast::hash(save) {
        return None;
    }
    let mut files = Vec::new();
    while let Some(n) = take(4) {
        let path = String::from_utf8(take(word(n))?.to_vec()).ok()?;
        let n = word(take(4)?);
        files.push((path, take(n)?.to_vec()));
    }
    Some(files)
}

/// Whether an avatar on `flash`'s volume (/Didj/Data/Avatars/<folder>/meta.inf)
/// wants `package::fix_avatar_meta`: Didjis LeapFrog Connect sent to the
/// console over USB. `fix_avatars_in_place` then fixes them.
pub fn avatars_to_fix(flash: &Nand) -> bool {
    let brio = nand::partition("Brio").unwrap();
    let volume = ubi::read_volume(flash, brio, brio::volume_sectors(brio.size / nand::BLOCK) * 512);
    fatfs::list(&volume, "Data/Avatars").unwrap_or_default().iter().filter(|e| e.is_dir).any(|e| {
        fatfs::read_file(&volume, &format!("Data/Avatars/{}/meta.inf", e.name)).is_some_and(|m| crate::package::fix_avatar_meta(&m).is_some())
    })
}

/// The Leapster Explorer's Bulk partition on `flash` (to its end).
fn explorer_bulk(flash: &Nand) -> nand::Partition {
    let offset = crate::explorer::PARTITIONS[4].1;
    nand::Partition { name: "Bulk", offset, size: flash.size() - offset }
}

/// The Explorer's Bulk volume (UBIFS) read from `flash`: with `all`, every
/// LEB is trusted; else only those mapped on `flash` (a save's changes
/// applied to an erased NAND).
fn explorer_bulk_files(flash: &Nand, all: bool) -> crate::ubifs::Tree {
    let part = explorer_bulk(flash);
    let lebs = ubi::lebs(flash, part, ubi::available_blocks(part.size / nand::BLOCK));
    let data: Vec<Vec<u8>> = lebs.iter().map(|b| b.map_or_else(Vec::new, |b| flash.read(b * nand::BLOCK + ubi::DATA_OFFSET, ubi::LEB))).collect();
    let slices: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    crate::ubifs_read::read(&slices, &|l| all || lebs.get(l).is_some_and(Option::is_some))
}

/// Whether two tree entries hold the same thing (files by contents).
fn same(a: &crate::ubifs::Entry, b: &crate::ubifs::Entry) -> bool {
    use crate::ubifs::Kind;
    match (&a.kind, &b.kind) {
        (Kind::Dir, Kind::Dir) => true,
        (Kind::File(x), Kind::File(y)) => x == y,
        (Kind::Symlink(x), Kind::Symlink(y)) => x == y,
        _ => false,
    }
}

/// The Leapster Explorer's `user_files`: the files on the Bulk volume
/// (UBIFS; paths under /LF/Bulk) the console wrote, read whole from the
/// LEBs a save made on another image holds, less those at paths `base`
/// (the newly built image) has, so the packages' files stay as built.
pub fn explorer_user_files(changes: &[u8], base: &Nand) -> Result<crate::ubifs::Tree, String> {
    let mut saved = Nand::with_blocks(crate::explorer::NAND_BLOCKS_1GB.max(base.blocks()));
    saved.apply_changes(changes)?;
    let files = explorer_bulk_files(&saved, false);
    let built = explorer_bulk_files(base, true);
    Ok(files.into_iter().filter(|(p, _)| !built.contains_key(p)).collect())
}

/// The Leapster Explorer's `files_beyond`, for a save whose image can be
/// built again (`old`, e.g. the 1 GB NAND): the files on `flash`'s Bulk
/// volume that `old` doesn't have or has otherwise, so a file the console
/// changed comes along too.
pub fn explorer_files_beyond(flash: &Nand, old: &Nand) -> crate::ubifs::Tree {
    let built = explorer_bulk_files(old, true);
    explorer_bulk_files(flash, true).into_iter().filter(|(p, e)| built.get(p).is_none_or(|b| !same(b, e))).collect()
}

/// Fixes the Didjis LeapFrog Connect sent (`avatars_to_fix`) where they
/// are: each meta.inf rewritten on the volume and the LEBs that changed
/// written anew (`ubi::write_leb`), everything else on the console left
/// as it was. Returns how many were fixed.
///
/// Until 2026-11-12 the image was built again from the packages with the
/// player's files (`user_files`) instead, which keeps only files at paths
/// the packages don't have: a player's profile after a sync didn't
/// persist (GUESS that this was the cause; a file LeapFrog Connect
/// changed where the packages have one was lost that way).
pub fn fix_avatars_in_place(flash: &mut Nand) -> Result<usize, String> {
    let brio = nand::partition("Brio").unwrap();
    let len = brio::volume_sectors(brio.size / nand::BLOCK) * 512;
    let before = ubi::read_volume(flash, brio, len);
    let mut volume = before.clone();
    let mut fixed = 0;
    {
        let mut w = fatfs::Writer::new(&mut volume).map_err(|e| e.to_string())?;
        for e in fatfs::list(&before, "Data/Avatars").unwrap_or_default().iter().filter(|e| e.is_dir) {
            let path = format!("Data/Avatars/{}/meta.inf", e.name);
            if let Some(meta) = fatfs::read_file(&before, &path).and_then(|m| crate::package::fix_avatar_meta(&m)) {
                w.file(&path, &meta).map_err(|e| format!("{path}: {e}"))?;
                fixed += 1;
            }
        }
        w.finish();
    }
    for (lnum, (was, now)) in before.chunks(ubi::LEB).zip(volume.chunks(ubi::LEB)).enumerate() {
        if was != now {
            ubi::write_leb(flash, brio, lnum, now)?;
        }
    }
    Ok(fixed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Didji synced over USB (no Device line) wants fixing; LeapFrog's
    /// own avatar doesn't.
    #[test]
    fn synced_didjis_found() {
        let mut flash = Nand::erased();
        brio::install(&mut flash, &[("Data/Avatars/Avatar1/meta.inf".to_string(), b"Device=\"Didj\"
".to_vec())]).unwrap();
        assert!(!avatars_to_fix(&flash));
        brio::install(&mut flash, &[("Data/Avatars/D/meta.inf".to_string(), b"DeviceTypes=\"DIDJ\"
".to_vec())]).unwrap();
        assert!(avatars_to_fix(&flash));
    }

    /// The fix rewrites the synced Didji's meta.inf where it is: the
    /// player's other files stay, including one at a path the packages
    /// have.
    #[test]
    fn synced_didjis_fixed_in_place() {
        let mut flash = Nand::erased();
        let files = [
            ("Data/Avatars/Avatar1/meta.inf".to_string(), b"Device=\"Didj\"
".to_vec()),
            ("Data/Avatars/D/meta.inf".to_string(), b"DeviceTypes=\"DIDJ\"
".to_vec()),
            ("Data/0/profile.dsc".to_string(), b"Ann".to_vec()),
            ("Base/settings.cfg".to_string(), vec![7u8; 200_000]),
        ];
        brio::install(&mut flash, &files).unwrap();
        assert_eq!(fix_avatars_in_place(&mut flash), Ok(1));
        assert!(!avatars_to_fix(&flash));
        let brio = nand::partition("Brio").unwrap();
        let volume = ubi::read_volume(&flash, brio, brio::volume_sectors(brio.size / nand::BLOCK) * 512);
        assert_eq!(fatfs::read_file(&volume, "Data/Avatars/D/meta.inf").unwrap(), b"DeviceTypes=\"DIDJ\"
Device=\"Didj\"
");
        assert_eq!(fatfs::read_file(&volume, "Data/0/profile.dsc").unwrap(), b"Ann");
        assert_eq!(fatfs::read_file(&volume, "Base/settings.cfg").unwrap(), vec![7u8; 200_000]);
        assert_eq!(fix_avatars_in_place(&mut flash), Ok(0));
    }

    #[test]
    fn packed_files_come_back_with_their_save_only() {
        let files = vec![("/Data/0/profile.dsc".to_string(), b"Ann".to_vec()), ("/Data/0/empty".to_string(), Vec::new())];
        let packed = pack(&files, b"save");
        assert_eq!(unpack(&packed, b"save"), Some(files));
        assert_eq!(unpack(&packed, b"another save"), None);
        assert_eq!(unpack(&packed[..packed.len() - 1], b"save"), None);
    }

    /// A file the console wrote is carried over from a save made on
    /// another image; the packages' files are not.
    #[test]
    fn carries_the_consoles_files_only() {
        let package = vec![("Base/lib/a.so".to_string(), vec![1u8; 70_000])];
        let mut old = Nand::erased();
        brio::install(&mut old, &package).unwrap();
        // The console's writes: the old image with a profile added.
        let mut played = Nand::erased();
        let profile = ("Data/0/profile.dsc".to_string(), b"Ann".to_vec());
        brio::install(&mut played, &[package.clone(), vec![profile]].concat()).unwrap();
        let changes = played.changes_from(&old);
        // The new image: a newer package.
        let mut new = Nand::erased();
        brio::install(&mut new, &[("Base/lib/a.so".to_string(), vec![2u8; 70_000])]).unwrap();
        let files = user_files(&changes, &new).unwrap();
        assert_eq!(files, [("/Data/0/profile.dsc".to_string(), b"Ann".to_vec())]);
    }

    /// An Explorer save made on another image: the console's files on
    /// Bulk come over, the packages' don't. And a save on the 1 GB NAND
    /// (built again) moves to the 512 MB one with a file the console
    /// changed.
    #[test]
    fn explorer_files_carried_over() {
        use crate::explorer::{write_bulk, NAND_BLOCKS, NAND_BLOCKS_1GB};
        use crate::ubifs::{Entry, Kind, Tree};
        let file = |d: &[u8]| Entry { kind: Kind::File(d.to_vec()), mode: 0o644, mtime: 1 };
        let mut package = Tree::new();
        package.insert("ProgramFiles/Game/a.bin".into(), file(&vec![1u8; 70_000]));
        package.insert("settings.cfg".into(), file(b"lang=en"));
        let mut old = Nand::with_blocks(NAND_BLOCKS_1GB);
        write_bulk(&mut old, &package).unwrap();
        // The console's writes: a profile added, settings changed.
        let mut played_tree = package.clone();
        played_tree.insert("Data/Local/0/profile".into(), file(b"Ann"));
        played_tree.insert("settings.cfg".into(), file(b"lang=fr"));
        let mut played = old.clone();
        write_bulk(&mut played, &played_tree).unwrap();
        let changes = played.changes_from(&old);

        // Another package set on the 512 MB NAND: the profile only.
        let mut new_tree = Tree::new();
        new_tree.insert("ProgramFiles/Game/a.bin".into(), file(&vec![2u8; 70_000]));
        new_tree.insert("settings.cfg".into(), file(b"lang=en"));
        let mut new = Nand::with_blocks(NAND_BLOCKS);
        write_bulk(&mut new, &new_tree).unwrap();
        let files = explorer_user_files(&changes, &new).unwrap();
        let paths: Vec<&str> = files.keys().map(String::as_str).collect();
        assert_eq!(paths, ["Data", "Data/Local", "Data/Local/0", "Data/Local/0/profile"]);

        // The 1 GB image built again: the changed settings come too.
        let beyond = explorer_files_beyond(&played, &old);
        let paths: Vec<&str> = beyond.keys().map(String::as_str).collect();
        assert_eq!(paths, ["Data", "Data/Local", "Data/Local/0", "Data/Local/0/profile", "settings.cfg"]);
        let mut moved = Nand::with_blocks(NAND_BLOCKS);
        let mut tree = package.clone();
        tree.extend(beyond);
        write_bulk(&mut moved, &tree).unwrap();
        let read = explorer_bulk_files(&moved, true);
        assert!(matches!(&read["settings.cfg"].kind, Kind::File(d) if d == b"lang=fr"));
        assert!(matches!(&read["Data/Local/0/profile"].kind, Kind::File(d) if d == b"Ann"));
    }

    /// A real save: RIBBIT_EXPLORER_DIR (the firmware set) and
    /// RIBBIT_EXPLORER_SAVE (a .nandblocks made on the 1 GB NAND). Lists
    /// what would be carried over to the 512 MB NAND.
    #[test]
    #[ignore]
    fn explorer_real_save() {
        let (Ok(dir), Ok(save)) = (std::env::var("RIBBIT_EXPLORER_DIR"), std::env::var("RIBBIT_EXPLORER_SAVE")) else { return };
        let changes = std::fs::read(save).unwrap();
        let new = crate::explorer::nand_from_packages(std::path::Path::new(&dir)).unwrap();
        // RIBBIT_EXPLORER_PARTIAL=1: a save made on the 512 MB NAND, read
        // from its blocks alone, as if made on another image.
        if std::env::var("RIBBIT_EXPLORER_PARTIAL").is_ok() {
            let mut flash = new.clone();
            flash.apply_changes(&changes).unwrap();
            let whole: Vec<String> = explorer_files_beyond(&flash, &new).into_keys().collect();
            let partial: Vec<String> = explorer_user_files(&changes, &crate::explorer::nand_from_packages_blocks(std::path::Path::new(&dir), 2048 + 1024).unwrap_or(new)).unwrap().into_keys().collect();
            println!("whole {whole:?}
partial {partial:?}");
            return;
        }
        let old = crate::explorer::nand_from_packages_blocks(std::path::Path::new(&dir), crate::explorer::NAND_BLOCKS_1GB).unwrap();
        let mut flash = old.clone();
        flash.apply_changes(&changes).unwrap();
        let mut c = changes.as_slice();
        let mut per: std::collections::BTreeMap<&str, usize> = Default::default();
        while c.len() >= 5 {
            let n = u32::from_le_bytes(c[..4].try_into().unwrap()) as usize * nand::BLOCK;
            let part = crate::explorer::PARTITIONS.iter().rev().find(|p| p.1 <= n).unwrap().0;
            *per.entry(part).or_default() += 1;
            c = if c[4] == 1 { &c[5 + nand::PAGE_RAW * nand::PAGES_PER_BLOCK..] } else { &c[5..] };
        }
        println!("changed blocks: {per:?}");
        for (p, e) in explorer_files_beyond(&flash, &old) {
            let len = match &e.kind { crate::ubifs::Kind::File(d) => d.len() as i64, _ => -1 };
            println!("beyond {p} {len} {:o}", e.mode);
        }
        for p in explorer_user_files(&changes, &new).unwrap().keys() {
            println!("user {p}");
        }
    }
}
