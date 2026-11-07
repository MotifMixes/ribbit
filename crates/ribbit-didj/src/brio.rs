//! The Brio partition's file system, mounted at /Didj: a FAT32 volume
//! inside the UBI volume "ubi_Brio" (ubi.rs), holding the Brio libraries,
//! the menu, its art and sounds, and the built-in game.
//!
//! The format copies a real console's (VERIFIED: `fsck.vfat -nv
//! /dev/mtdblock9` on a real Didj): FAT32,
//! "MSDOS5.0", 512-byte sectors, 4 per cluster, 38 reserved sectors, 2 FATs
//! of 861 sectors, root at cluster 2, 63 sectors per track, 255 heads, no
//! hidden sectors, 442512 sectors. The FSInfo sector (1) and the backup
//! boot sector (6) are where Windows puts them (GUESS). The label "DIDJ"
//! (GUESS: the community's `mkfs.vfat -F 32 -s 4 -S 512 -n DIDJ` for that
//! geometry). The console's own `mkfs.vfat -n Didj -S 512` (rcS, on an
//! empty Brio) makes FAT16 instead; Ribbit made that before 2026-11-07, and
//! saves from then keep it. Its serial number is Ribbit's own choice.
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
const SECTORS_PER_CLUSTER: usize = 4;
const RESERVED: usize = 38;
const FATS: usize = 2;
const FSINFO: usize = 1;
const BACKUP_BOOT: usize = 6;
const ROOT_CLUSTER: u32 = 2;
const LABEL: &[u8; 11] = b"DIDJ       ";
const SERIAL: u32 = 0x2008_0601;

/// The sectors of the volume on a partition of `blocks` erase blocks: all
/// the LEBs the UBI volume reserves (442512 on Brio, as mkfs.vfat saw).
pub fn volume_sectors(blocks: usize) -> usize {
    ubi::available_blocks(blocks) * ubi::LEB / SECTOR
}

/// The sectors in each FAT (861 for Brio, as on the real console).
fn fat_sectors(total: usize) -> usize {
    let data = total - RESERVED;
    let clusters = (data * SECTOR + FATS * 8) / (SECTORS_PER_CLUSTER * SECTOR + FATS * 4);
    ((clusters + 2) * 4).div_ceil(SECTOR)
}

/// A freshly formatted volume of `total` sectors, up to the end of its
/// root directory's cluster (the rest is free space; fatfs::Writer grows
/// it).
pub fn format(total: usize) -> Vec<u8> {
    let fat = fat_sectors(total);
    let data = RESERVED + FATS * fat;
    let clusters = (total - data) / SECTORS_PER_CLUSTER;
    let mut d = vec![0u8; (data + SECTORS_PER_CLUSTER) * SECTOR];
    let b = &mut d[..SECTOR];
    b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"MSDOS5.0");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = SECTORS_PER_CLUSTER as u8;
    b[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    b[16] = FATS as u8;
    b[21] = 0xF8; // media: fixed disk
    b[24..26].copy_from_slice(&63u16.to_le_bytes()); // sectors per track
    b[26..28].copy_from_slice(&255u16.to_le_bytes()); // heads
    b[32..36].copy_from_slice(&(total as u32).to_le_bytes());
    b[36..40].copy_from_slice(&(fat as u32).to_le_bytes());
    b[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    b[48..50].copy_from_slice(&(FSINFO as u16).to_le_bytes());
    b[50..52].copy_from_slice(&(BACKUP_BOOT as u16).to_le_bytes());
    b[64] = 0x80; // drive number
    b[66] = 0x29; // extended boot signature
    b[67..71].copy_from_slice(&SERIAL.to_le_bytes());
    b[71..82].copy_from_slice(LABEL);
    b[82..90].copy_from_slice(b"FAT32   ");
    b[510..512].copy_from_slice(&[0x55, 0xAA]);
    // FSInfo: free clusters (all but the root's) and where to look next.
    let mut info = [0u8; SECTOR];
    info[0..4].copy_from_slice(b"RRaA");
    info[484..488].copy_from_slice(b"rrAa");
    info[488..492].copy_from_slice(&(clusters as u32 - 1).to_le_bytes());
    info[492..496].copy_from_slice(&(ROOT_CLUSTER + 1).to_le_bytes());
    info[508..512].copy_from_slice(&[0, 0, 0x55, 0xAA]);
    let boot: [u8; SECTOR] = d[..SECTOR].try_into().unwrap();
    for (sector, s) in [(FSINFO, &info), (BACKUP_BOOT, &boot), (BACKUP_BOOT + 1, &info)] {
        d[sector * SECTOR..(sector + 1) * SECTOR].copy_from_slice(s);
    }
    for f in 0..FATS {
        let at = (RESERVED + f * fat) * SECTOR;
        d[at..at + 12].copy_from_slice(&[0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F]);
    }
    let at = data * SECTOR;
    d[at..at + 11].copy_from_slice(LABEL);
    d[at + 11] = 0x08; // volume label
    d
}

/// Where a package's files go on the volume, from its meta.inf: the
/// folder that holds the package's own top folder. libLightningJSON's
/// CPkgPathBase pairs the package types System, Application, Avatar,
/// CYODataset, MicroDownload with the upload folders Base, ProgramFiles,
/// Data/Avatars, Data/CYO, Data/MDL (two lists in that order; the menu
/// shows JetPack Heroes as "GAME NOT DOWNLOADED" when it is anywhere else).
pub fn install_folder(meta: &Meta) -> &'static str {
    match meta.get("Type") {
        Some("Application") => "ProgramFiles",
        Some("Avatar") => "Data/Avatars",
        Some("CYODataset") => "Data/CYO",
        Some("MicroDownload") => "Data/MDL",
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
    fn the_format_matches_a_real_console() {
        let total = volume_sectors(nand::partition("Brio").unwrap().size / nand::BLOCK);
        assert_eq!(total, 442512);
        assert_eq!(fat_sectors(total), 861);
        let d = format(total);
        // What fsck.vfat -nv showed on the console: 2048-byte clusters, 38
        // reserved sectors, FATs of 861 sectors, data at sector 1760, 110188
        // clusters.
        assert_eq!(&d[3..11], b"MSDOS5.0");
        assert_eq!(d[11..36], [0, 2, 4, 38, 0, 2, 0, 0, 0, 0, 0xF8, 0, 0, 63, 0, 255, 0, 0, 0, 0, 0, 0x90, 0xC0, 6, 0]);
        let v = fatfs::Volume::parse(&d).unwrap();
        assert!(!v.fat16);
        assert_eq!((v.fat, v.data, v.root_cluster), (38 * 512, 1760 * 512, 2));
        assert_eq!((total - 1760) / 4, 110188);
        assert_eq!(d.len(), v.data + 2048);
        assert_eq!(d[..512], d[6 * 512..7 * 512], "backup boot sector");
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
        // FAT32 entries: AppManager takes 3 clusters, chained.
        let v = fatfs::Volume::parse(&d).unwrap();
        let fat: Vec<u32> = d[v.fat..v.fat + 64].chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap()) & 0x0FFF_FFFF).collect();
        assert_eq!(fat[..3], [0x0FFF_FFF8, 0x0FFF_FFFF, 0x0FFF_FFFF]);
        assert!(fat[3..].iter().filter(|&&e| e >= 0x0FFF_FFF8).count() >= 3);
        assert_eq!(d[v.fat..v.fat + v.fat_bytes], d[v.fat + v.fat_bytes..v.fat + 2 * v.fat_bytes]);
    }

    #[test]
    fn installed_on_the_nand() {
        let mut flash = Nand::erased();
        install(&mut flash, &[("Base/x.txt".to_string(), b"hi".to_vec())]).unwrap();
        let d = ubi::read_volume(&flash, nand::partition("Brio").unwrap(), 1024 * 1024);
        assert_eq!(fatfs::read_file(&d, "Base/x.txt").as_deref(), Some(&b"hi"[..]));
    }
}
