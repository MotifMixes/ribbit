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
//! * `mlc`: the display's layer compositor (the RGB layers; not video yet).

pub mod brio;
pub mod cart;
pub mod dma;
pub mod ga3d;
pub mod machine;
pub mod mlc;
pub mod nand;
pub mod package;
pub mod tfs;
pub mod timer;
pub mod ubi;

/// The internal NAND as LeapFrog Connect leaves it with the packages in
/// `dir` (the `DIDJ-0x...lfp` files): the firmware written raw, the rest
/// installed on the Brio partition (left erased with `brio` false; the
/// console then formats it).
pub fn nand_from_packages(dir: &std::path::Path, brio: bool) -> Result<nand::Nand, String> {
    let mut fw = package::Firmware::default();
    let mut brio_files = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        if package::package_id(&path.to_string_lossy()).is_none() {
            continue;
        }
        let data = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut rest = Vec::new();
        let mut firmware = false;
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{}: {e}", path.display()))? {
            let bytes = ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{}: {e}", path.display()))?;
            if fw.add(&entry.name, bytes.clone()) {
                firmware = true;
            }
            rest.push((entry.name, bytes));
        }
        if !firmware && brio {
            brio_files.extend(brio::package_files(rest));
        }
    }
    let mut flash = nand::Nand::from_firmware(&fw)?;
    if !brio_files.is_empty() {
        brio::install(&mut flash, &brio_files).map_err(|e| format!("Brio: {e}"))?;
    }
    Ok(flash)
}
