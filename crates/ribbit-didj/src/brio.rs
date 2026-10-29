//! The Brio partition's file system, mounted at /Didj: a FAT16 volume
//! inside the UBI volume "ubi_Brio" (ubi.rs), holding the Brio libraries,
//! the menu, its art and sounds, and the built-in game.
//!
//! The format copies what the console's own `mkfs.vfat -n Didj -S 512`
//! writes (VERIFIED: Ribbit boot, rcS formatting an empty Brio): FAT16,
//! 512-byte sectors, 8 per cluster, 1 reserved sector, 2 FATs, 512 root
//! entries, the label "Didj" in the boot sector and as the root's first
//! entry. Its serial number is Ribbit's own choice.
//!
//! Where packages go: system packages (meta.inf `Type="System"`) under
//! `/Didj/Base/<their folder>` (VERIFIED: the PATH and library path in
//! launch_main, `/Didj/Base/UniversalAudio/FaceOff.ogg` in libAudio.so,
//! `/Didj/Base/Brio/Module/` in libModule.so). The avatar goes there too
//! (GUESS: libLightningJSON.so names `Avatar1/` next to `Fonts/` and
//! `Audio/`). Applications go to `/Didj/<their folder>` (GUESS, like a
//! cartridge's `/JDI` at its root).

use crate::nand::{self, Nand};
use crate::package::Meta;
use crate::ubi;
use ribbit_core::fatfs;

const SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: usize = 8;
const RESERVED: usize = 1;
const FATS: usize = 2;
const ROOT_ENTRIES: usize = 512;
const LABEL: &[u8; 11] = b"Didj       ";
const SERIAL: u32 = 0x2008_0601;

/// The sectors of the volume on a partition of `blocks` erase blocks: all
/// the LEBs the UBI volume reserves (442512 on Brio, as mkfs.vfat saw).
pub fn volume_sectors(blocks: usize) -> usize {
    ubi::available_blocks(blocks) * ubi::LEB / SECTOR
}

/// The sectors in each FAT, as mkdosfs (dosfstools 2.x) works it out.
fn fat_sectors(total: usize) -> usize {
    let root = ROOT_ENTRIES * 32 / SECTOR;
    let data = total - RESERVED - root;
    let clusters = (data * SECTOR + FATS * 4) / (SECTORS_PER_CLUSTER * SECTOR + FATS * 2);
    ((clusters + 2) * 2).div_ceil(SECTOR)
}

/// A freshly formatted volume of `total` sectors, up to the end of its
/// root directory (the rest is free space; fatfs::Writer grows it).
pub fn format(total: usize) -> Vec<u8> {
    let fat = fat_sectors(total);
    let root = RESERVED + FATS * fat;
    let mut d = vec![0u8; (root + ROOT_ENTRIES * 32 / SECTOR) * SECTOR];
    let b = &mut d[..SECTOR];
    b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"mkdosfs\0");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = SECTORS_PER_CLUSTER as u8;
    b[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    b[16] = FATS as u8;
    b[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    b[21] = 0xF8; // media: fixed disk
    b[22..24].copy_from_slice(&(fat as u16).to_le_bytes());
    b[24..26].copy_from_slice(&63u16.to_le_bytes()); // sectors per track
    b[26..28].copy_from_slice(&255u16.to_le_bytes()); // heads
    b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    b[38] = 0x29; // extended boot signature
    b[39..43].copy_from_slice(&SERIAL.to_le_bytes());
    b[43..54].copy_from_slice(LABEL);
    b[54..62].copy_from_slice(b"FAT16   ");
    b[510..512].copy_from_slice(&[0x55, 0xAA]);
    for f in 0..FATS {
        let at = (RESERVED + f * fat) * SECTOR;
        d[at..at + 4].copy_from_slice(&[0xF8, 0xFF, 0xFF, 0xFF]);
    }
    let at = root * SECTOR;
    d[at..at + 11].copy_from_slice(LABEL);
    d[at + 11] = 0x08; // volume label
    d
}

/// Where a package's files go on the volume, from its meta.inf: the
/// folder that holds the package's own top folder.
pub fn install_folder(meta: &Meta) -> &'static str {
    match meta.get("Type") {
        Some("Application") => "",
        _ => "Base",
    }
}

/// A package's files (zip entry names and contents; folders end in "/")
/// with the paths they take on the volume. Every file is kept, meta.inf
/// and the .md5 lists too (what LeapFrog Connect copies is UNKNOWN).
pub fn package_files(entries: Vec<(String, Vec<u8>)>) -> Vec<(String, Vec<u8>)> {
    let meta = entries
        .iter()
        .find(|(n, _)| n.ends_with("/meta.inf") && n.matches('/').count() == 1)
        .map(|(_, d)| Meta::parse(&String::from_utf8_lossy(d)));
    let folder = meta.as_ref().map_or("Base", install_folder);
    entries
        .into_iter()
        .map(|(n, d)| (if folder.is_empty() { n } else { format!("{folder}/{n}") }, d))
        .collect()
}

/// The Brio volume with `files` (paths from its root, contents) on it.
pub fn build(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>, String> {
    let brio = nand::partition("Brio").unwrap();
    let mut volume = format(volume_sectors(brio.size / nand::BLOCK));
    let mut w = fatfs::Writer::new(&mut volume).map_err(|e| e.to_string())?;
    for (path, data) in files {
        if path.ends_with('/') {
            w.folder(path).map_err(|e| format!("{path}: {e}"))?;
        } else {
            w.file(path, data).map_err(|e| format!("{path}: {e}"))?;
        }
    }
    w.finish();
    Ok(volume)
}

/// Writes the Brio partition: UBI holding the volume with `files`.
pub fn install(flash: &mut Nand, files: &[(String, Vec<u8>)]) -> Result<(), String> {
    let volume = build(files)?;
    ubi::write_volume(flash, nand::partition("Brio").unwrap(), "ubi_Brio", &volume)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_format_matches_mkfs_vfat() {
        let total = volume_sectors(nand::partition("Brio").unwrap().size / nand::BLOCK);
        assert_eq!(total, 442512);
        assert_eq!(fat_sectors(total), 216);
        let d = format(total);
        // The boot sector's fields as the console's mkfs.vfat wrote them.
        assert_eq!(d[11..36], [0, 2, 8, 1, 0, 2, 0, 2, 0, 0, 0xF8, 216, 0, 63, 0, 255, 0, 0, 0, 0, 0, 0x90, 0xC0, 6, 0]);
        let v = fatfs::Volume::parse(&d).unwrap();
        assert!(v.fat16);
        assert_eq!((v.root, v.data), (433 * 512, 465 * 512));
        assert_eq!(d.len(), v.data);
    }

    #[test]
    fn files_land_on_the_volume() {
        let files = vec![
            ("Base/bin/AppManager".to_string(), vec![7u8; 5000]),
            ("Base/Brio/lib/libKernelMPI.so".to_string(), b"elf".to_vec()),
            ("POW/".to_string(), Vec::new()),
        ];
        let d = build(&files).unwrap();
        assert_eq!(fatfs::read_file(&d, "/Base/bin/AppManager").unwrap(), vec![7u8; 5000]);
        assert_eq!(fatfs::read_file(&d, "Base/Brio/lib/libKernelMPI.so").as_deref(), Some(&b"elf"[..]));
        let root: Vec<String> = fatfs::list(&d, "").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(root, ["Base", "POW"], "the label is not listed");
        // FAT16 entries: AppManager takes 2 clusters, chained.
        let v = fatfs::Volume::parse(&d).unwrap();
        let fat: Vec<u16> = d[v.fat..v.fat + 16].chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        assert_eq!(fat[..2], [0xFFF8, 0xFFFF]);
        assert!(fat[2..].iter().any(|&e| e == 0xFFFF));
        assert_eq!(d[v.fat..v.fat + v.fat_bytes], d[v.fat + v.fat_bytes..v.fat + 2 * v.fat_bytes]);
    }

    #[test]
    fn installed_on_the_nand() {
        let mut flash = Nand::erased();
        install(&mut flash, &[("Base/x.txt".to_string(), b"hi".to_vec())]).unwrap();
        let d = ubi::read_volume(&flash, nand::partition("Brio").unwrap(), 300 * 1024);
        assert_eq!(fatfs::read_file(&d, "Base/x.txt").as_deref(), Some(&b"hi"[..]));
    }
}
