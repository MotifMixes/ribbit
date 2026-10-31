//! LeapFrog Didj (2008, codename "Lightning"): an LF1000 (MagicEyes Pollux
//! VR3520F, ARM926EJ-S) running Linux 2.6.20 and LeapFrog's Brio libraries.
//! See docs/didj.md.
//!
//! Boots to Linux user space (no screen yet). This crate holds:
//!
//! * `package`: its software packages (`.lfp` zip archives) and their
//!   `meta.inf`, and the firmware files they carry.
//! * `tfs`: the "table file system" of `kernel.bin` (the boot splash and the
//!   kernel uImage, laid out by erase block).
//! * `nand`: the internal 256 MB NAND: its geometry, partitions, and an
//!   image built from the firmware packages, as LeapFrog Connect installs
//!   them.
//! * `ubi`, `brio`: the Brio partition: UBI holding a FAT16 volume with
//!   the packages that are not raw firmware.
//! * `machine`: the boot machine (ARM926EJ-S, SDRAM, NAND controller,
//!   UARTs, ADC, SPI0); every other register is logged.
//! * `timer`: the timers and the interrupt controller.
//! * `dma`: the DMA controller (audio out paced at the sample rate).
//! * `enhanced`: Enhanced mode's fixes for the firmware's own faults.
//! * `jffs2`: reading and writing files in the JFFS2 file systems.
//! * `mlc`: the display's layer compositor (the RGB layers; not video yet).

pub mod autostart;
pub mod brio;
pub mod carry;
pub mod cart;
pub mod dma;
pub mod enhanced;
pub mod explorer;
pub mod ga3d;
pub mod hd;
pub mod fonthd;
pub mod texup;
pub mod i2c;
pub mod jffs2;
pub mod machine;
pub mod mlc;
pub mod nand;
pub mod linux_nand;
pub mod nor;
pub mod package;
pub mod tfs;
pub mod timer;
pub mod touch;
pub mod ubi;
pub mod ubifs;
pub mod udc;

/// The internal NAND as LeapFrog Connect leaves it with the packages in
/// `dir` (the `DIDJ-0x...lfp` files): the firmware written raw, the rest
/// installed on the Brio partition (left erased with `brio` false; the
/// console then formats it). The packages may also be bundled in a
/// `bios.didj` there (`package::packages_in`); any of `package::NEEDED`
/// missing is an error naming them all.
pub fn nand_from_packages(dir: &std::path::Path, brio: bool) -> Result<nand::Nand, String> {
    nand_and_extras_missing(dir, brio).map(|(nand, _)| nand)
}

/// As `nand_from_packages`, and the `package::EXTRAS` it was built without
/// (the menu starts without them), as `package::missing_extras` names them.
pub fn nand_and_extras_missing(dir: &std::path::Path, brio: bool) -> Result<(nand::Nand, Vec<String>), String> {
    nand_with_files(dir, brio, &[])
}

/// As `nand_and_extras_missing`, with `extra` (paths on /Didj, contents)
/// added to the Brio volume after the packages: a player's files carried
/// over from a save made on another image (`carry::user_files`).
pub fn nand_with_files(dir: &std::path::Path, brio: bool, extra: &[(String, Vec<u8>)]) -> Result<(nand::Nand, Vec<String>), String> {
    let mut fw = package::Firmware::default();
    let mut brio_files = Vec::new();
    let packages = package::packages_in(dir)?;
    let extras = package::missing_extras(packages.iter().map(|(n, _)| n.as_str()));
    let missing = package::missing(packages.iter().map(|(n, _)| n.as_str()));
    if !missing.is_empty() {
        return Err(format!("missing Didj packages: {}", missing.join(", ")));
    }
    for (name, data) in packages {
        let mut rest = Vec::new();
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{name}: {e}"))? {
            let bytes = ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{name}: {e}"))?;
            fw.add(&entry.name, bytes.clone());
            rest.push((entry.name, bytes));
        }
        // The firmware packages too, beside being written raw: LeapFrog
        // Connect wants /Didj/Base/bootstrap-LF_LF1000 and
        // firmware-LF_LF1000 (a player: without them it stops with an
        // error; extracted into its view of the Didj it carries on).
        if brio {
            brio_files.extend(brio::package_files(rest));
        }
    }
    let mut flash = nand::Nand::from_firmware(&fw)?;
    if !brio_files.is_empty() {
        brio_files.extend(extra.iter().cloned());
        // Didjis, from packages or carried over (Connect sent them to the
        // console): their meta.inf as the menu takes it.
        for (path, data) in &mut brio_files {
            let p = path.trim_start_matches('/').to_ascii_lowercase();
            if p.starts_with("data/avatars/") && p.ends_with("/meta.inf") && p.matches('/').count() == 3
                && let Some(fixed) = package::fix_avatar_meta(data)
            {
                *data = fixed;
            }
        }
        brio::install(&mut flash, &brio_files).map_err(|e| format!("Brio: {e}"))?;
    }
    Ok((flash, extras))
}
