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

#[cfg(test)]
mod tests {
    use super::*;

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
}
