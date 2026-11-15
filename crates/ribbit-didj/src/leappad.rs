//! LeapFrog LeapPad Explorer (2011, the first LeapPad; codename "Madrid"):
//! the Leapster Explorer's LF1000 platform with a 5" 480x272 touch screen,
//! a USB camera on the OHCI host, a microphone, and 2 GB of storage on the
//! SD controller instead of NAND. Its firmware (2.x) is the same as the
//! Leapster Explorer's later firmware: one root file system for both
//! (/etc/setplatform: board IDs 1, 2, 6, 7, A are EMERALD, B, C, D MADRID).
//! See docs/leappad1.md.
//!
//! LeapFrog's packages carry no boot loader (it lives in the console's NOR
//! flash), so `boot` does its job as `explorer::boot` does: the kernel from
//! the SD card's second partition into SDRAM, started with a command line.

use ribbit_arm::mode;

use crate::explorer::{install_lf2, tar_entries};
use crate::machine::Didj;
use crate::nand::{self, Nand};
use crate::ubifs::Tree;

/// The board ID the boot loader reports (mach-lf1000/board_ids.h
/// LF1000_BOARD_MADRID: 5" LCD, 2 GB flash on the SD controller, no TV
/// out). GUESS: which of 0x0B-0x0D retail units report is UNKNOWN; 0x0D
/// adds the LFP100 power chip, 0x0C TV out.
pub const BOARD_ID: u8 = 0x0B;

/// The screen (the kernel's LCD480x272 / ILI6480G2 module).
pub const WIDTH: u32 = 480;
pub const HEIGHT: u32 = 272;

/// SDRAM: 64 MB at 0x8000_0000, as on the Explorer.
pub const RAM_BASE: u32 = crate::explorer::RAM_BASE;
pub const SDRAM_SIZE: usize = crate::explorer::SDRAM_SIZE;

/// The SD controller channel the internal card is on: the LeapPad's
/// kernel drives channel 1 alone (VERIFIED in a boot: no access to channel
/// 0's registers; the board has MADRID_SD1_POWER).
pub const CARD_CHANNEL: usize = 1;

/// The command line. GUESS: the boot loader's real one is UNKNOWN; this is
/// the Explorer's (eLinux.org) with the LeapPad's screen module and the
/// SD card's root partition (the scripts' /dev/mmcblk0p3 and p4). The
/// root is mounted as ext4 (`ext2.rs`: no journal).
pub const CMDLINE: &str = "init=/sbin/init console=ttyS0,115200 mem=46M mlc_fb=0x82E00000,0x01200000 \
    ram=0x80000000-0x83FFFFFF screen_module=LCD480x272 root=/dev/mmcblk0p3 rw rootfstype=ext4 rootwait";

/// Whether a package file belongs to the LeapPad Explorer (its meta.inf).
pub fn for_leappad(data: &[u8]) -> bool {
    let Ok(entries) = ribbit_core::zip::entries(data) else { return false };
    entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with("meta.inf")).any(|e| {
        ribbit_core::zip::read_entry(data, e).is_ok_and(|m| crate::package::Meta::parse(&String::from_utf8_lossy(&m)).get("Device") == Some("LeapPadExplorer"))
    })
}

/// Whether `dir` holds LeapPad Explorer firmware (an `.lfp` of its).
pub fn is_leappad_dir(dir: &std::path::Path) -> bool {
    std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lfp"))).any(|p| {
        // The firmware packages are small but for FW Base; peek at the
        // small ones first by size.
        std::fs::metadata(&p).is_ok_and(|m| m.len() < 1 << 20) && std::fs::read(&p).is_ok_and(|d| for_leappad(&d))
    })
}

/// A partition from the MBR: (first byte, size in bytes).
pub fn partitions(mbr: &[u8]) -> Vec<(u64, u64)> {
    (0..4)
        .filter_map(|i| {
            let e = mbr.get(446 + 16 * i..462 + 16 * i)?;
            let start = u32::from_le_bytes(e[8..12].try_into().unwrap()) as u64;
            let count = u32::from_le_bytes(e[12..16].try_into().unwrap()) as u64;
            (e[4] != 0 && count != 0).then_some((start * 512, count * 512))
        })
        .collect()
}

/// The language packs the firmware needs, as on the Explorer
/// (`explorer::missing_packages`): the shared one (LPAD-0x001E000F-000000)
/// and one of a language.
pub fn missing_packages(installed: &[crate::package::Meta]) -> Vec<&'static str> {
    const SHARED: &str = "LPAD-0x001E000F-000000";
    let shared = |m: &crate::package::Meta| m.get("PackageID").is_some_and(|id| id.eq_ignore_ascii_case(SHARED));
    let mut missing = Vec::new();
    if !installed.iter().any(shared) {
        missing.push("Language Pack.lf2 (LPAD-0x001E000F-000000, the shared language pack)");
    }
    if !installed.iter().any(|m| m.get("Type").is_some_and(|t| t.trim() == "LanguagePack") && !shared(m)) {
        missing.push("Language Pack EN.lf2 (LPAD-0x001E000F-000001, or another language's pack)");
    }
    missing
}

/// How `card_with_files` starts its error when packages are missing.
pub const MISSING_PACKAGES: &str = "missing LeapPad Explorer packages: ";

/// The time stamp of files the packages give none (Firmware Base's build
/// date, 2012-09-18).
const MTIME: u32 = 1_347_990_000;

/// The SD card as LeapFrog Connect leaves it with the packages in `dir`:
/// the `.lfp` "DiskImage" files laid out by their paths,
/// `sd/<n><kind>/<partition>/<file>`: `0partition` the MBR, `1raw` a file
/// written at the start of its partition (the splash, the kernel), `2ext4`
/// a tar archive unpacked into a new file system there (the root, an
/// empty Bulk); the `.lf2` packages installed in Bulk as on the Explorer.
pub fn card_from_packages(dir: &std::path::Path) -> Result<Nand, String> {
    card_with_files(dir, &Tree::new())
}

/// `card_from_packages` with `extra` added to Bulk after the packages.
pub fn card_with_files(dir: &std::path::Path, extra: &Tree) -> Result<Nand, String> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    entries.sort();
    // (order, kind, partition number, contents)
    let mut pieces: Vec<(u32, String, usize, Vec<u8>)> = Vec::new();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lfp"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !for_leappad(&data) {
            continue;
        }
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{}: {e}", path.display()))? {
            let parts: Vec<&str> = entry.name.split('/').collect();
            let Some(sd) = parts.iter().position(|&p| p == "sd") else { continue };
            let Some(stage) = parts.get(sd + 1) else { continue };
            let digits = stage.chars().take_while(char::is_ascii_digit).count();
            let order = stage[..digits].parse().unwrap_or(9);
            let kind = stage[digits..].to_string();
            // `0partition/<file>`; the others `<kind>/<partition>/<file>`.
            let (n, file) = if kind == "partition" {
                (0, parts.get(sd + 2).copied().unwrap_or(""))
            } else {
                let n = parts.get(sd + 2).and_then(|p| p.parse().ok()).unwrap_or(0);
                (n, parts.get(sd + 3).copied().unwrap_or(""))
            };
            if file.is_empty() || (kind != "partition" && n == 0) {
                continue;
            }
            let bytes = ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{}: {e}", path.display()))?;
            pieces.push((order, kind, n, bytes));
        }
    }
    pieces.sort_by_key(|p| (p.0, p.2));
    let mbr = pieces.iter().find(|p| p.1 == "partition").map(|p| p.3.clone()).ok_or_else(|| format!("no LeapPad Explorer firmware (FW Partition) in {}", dir.display()))?;
    let parts = partitions(&mbr);
    let end = parts.iter().map(|p| p.0 + p.1).max().ok_or("the MBR has no partitions")?;
    let mut card = Nand::with_blocks(end.div_ceil(nand::BLOCK as u64) as usize);
    card.write_bytes(0, &mbr[..512.min(mbr.len())]);
    let part = |n: usize| parts.get(n.wrapping_sub(1)).copied().ok_or_else(|| format!("no partition {n} in the MBR"));
    let mut written = Vec::new();
    let mut bulk: Option<(usize, Tree)> = None;
    for (_, kind, n, bytes) in &pieces {
        match kind.as_str() {
            "partition" => {}
            "raw" => {
                let (at, size) = part(*n)?;
                if bytes.len() as u64 > size {
                    return Err(format!("{} bytes don't fit partition {n}", bytes.len()));
                }
                card.write_bytes(at as usize, bytes);
                written.push(*n);
            }
            "ext4" | "ext3" | "ext2" => {
                let mut tree = Tree::new();
                for (path, e) in tar_entries(bytes)? {
                    tree.insert(path, e);
                }
                if bulk.is_none() && tree.contains_key("ProgramFiles") {
                    bulk = Some((*n, tree));
                } else {
                    write_fs(&mut card, part(*n)?, &tree)?;
                    written.push(*n);
                }
            }
            other => return Err(format!("unknown partition kind {other:?} in the firmware")),
        }
    }
    for needed in [2, 3] {
        if !written.contains(&needed) {
            return Err(format!("no LeapPad Explorer firmware (FW Base) in {}: partition {needed} is missing", dir.display()));
        }
    }
    let (bulk_n, mut tree) = bulk.ok_or_else(|| format!("no LeapPad Explorer Bulk (FW Bulk Empty) in {}", dir.display()))?;
    let mut installed = Vec::new();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lf2"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        match install_lf2(&mut tree, &data) {
            Ok(meta) => installed.push(meta),
            // The firmware's own packages again, as .lf2 (FW Base.lf2 ...).
            Err(e) if e.contains("\"DiskImage\"") => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    // Bulk Empty marks the folders of packages the firmware needs with a
    // `doom` file (libLightningBase: "critical package doom file
    // detected"): the package isn't there yet. Installed, it goes, as the
    // console's lfpkg replaces the folder (else every game asked for its
    // language pack).
    let doomed: Vec<String> = tree.keys().filter(|p| p.ends_with("/doom")).filter(|p| tree.contains_key(&format!("{}/meta.inf", &p[..p.len() - 5]))).cloned().collect();
    for p in doomed {
        tree.remove(&p);
    }
    let missing = missing_packages(&installed);
    if !missing.is_empty() {
        return Err(format!("{MISSING_PACKAGES}{}", missing.join(", ")));
    }
    tree.extend(extra.iter().map(|(p, e)| (p.clone(), e.clone())));
    write_fs(&mut card, part(bulk_n)?, &tree)?;
    Ok(card)
}

/// The Bulk partition (the fourth) of a card: (first byte, size).
pub fn bulk_partition(card: &Nand) -> Option<(u64, u64)> {
    partitions(&card.read(0, 512)).get(3).copied()
}

/// Writes `tree` as a new ext2 file system into partition `(at, size)`.
pub fn write_fs(card: &mut Nand, (at, size): (u64, u64), tree: &Tree) -> Result<(), String> {
    crate::ext2::build(tree, size, MTIME, &mut |o, data| card.write_bytes((at + o) as usize, data))
}

/// The files of the file system in partition `n` (1-4) of a card.
pub fn files_in(card: &Nand, n: usize) -> Result<Vec<crate::ext2::File>, String> {
    let (at, size) = partitions(&card.read(0, 512)).get(n - 1).copied().ok_or("no such partition")?;
    let r = crate::ext2::Reader::new(|o, len| if o + len as u64 <= size { card.read((at + o) as usize, len) } else { vec![0; len] })?;
    Ok(r.files())
}

/// The kernel from partition 2's CBF image: an uncompressed Image (the
/// LeapPad's) or a zImage (unpacked).
pub fn kernel(card: &Nand) -> Result<Vec<u8>, String> {
    let parts = partitions(&card.read(0, 512));
    let (at, size) = parts.get(1).copied().ok_or("no kernel partition")?;
    let cbf = card.read(at as usize, size as usize);
    let word = |o: usize| cbf.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if word(0) != Some(0x9ABC_DEF0) {
        return Err("the kernel partition holds no CBF image".into());
    }
    let len = word(16).unwrap_or(0) as usize;
    let payload = cbf.get(24..24 + len).ok_or("the CBF image is cut short")?;
    // A zImage has its magic number at 0x24.
    if payload.get(0x24..0x28) == Some(&0x016F_2818u32.to_le_bytes()) {
        return crate::explorer::kernel_from_cbf(&cbf);
    }
    Ok(payload.to_vec())
}

/// The frame buffer (the command line's `mlc_fb=`).
const FRAMEBUFFER: u32 = 0x82E0_0000;

/// The boot splash (FIRST_Lpad.cbf in partition 1: a CBF holding the
/// Explorer's 32-bit RLE, 480x272) as ARGB pixels.
pub fn splash_pixels(card: &Nand) -> Option<Vec<u32>> {
    let (at, _) = partitions(&card.read(0, 512)).first().copied()?;
    let head = card.read(at as usize, 24);
    let len = u32::from_le_bytes(head[16..20].try_into().unwrap()) as usize;
    if u32::from_le_bytes(head[..4].try_into().unwrap()) != 0x9ABC_DEF0 || len > 4 << 20 {
        return None;
    }
    let rle = card.read(at as usize + 24, len);
    crate::explorer::rle_pixels(&rle, WIDTH as usize, HEIGHT as usize)
}

/// Shows the splash as the boot loader leaves it, on MLC layer 0 (as
/// `explorer::show_splash`, 480x272).
fn show_splash(bus: &mut crate::machine::Bus) {
    let Some(pixels) = splash_pixels(&bus.nand) else { return };
    let at = (FRAMEBUFFER - RAM_BASE) as usize;
    for (i, p) in pixels.iter().enumerate() {
        bus.sdram[at + 4 * i..][..4].copy_from_slice(&p.to_le_bytes());
    }
    let (w, h) = (WIDTH - 1, HEIGHT - 1);
    let mlc = crate::mlc::BASE;
    for (off, v) in [(0x00, 0x2), (0x04, h << 16 | w), (0x0C, w), (0x10, h), (0x24, 0x0653_0020), (0x28, 4), (0x2C, WIDTH * 4), (0x38, FRAMEBUFFER)] {
        bus.preset(mlc + off, v);
    }
}

/// The serial number in Ribbit's NOR MfgData (as the Explorer's).
pub const SERIAL: &str = "00000000000000000001";

/// Powers on as the boot loader leaves the machine (`explorer::boot`): the
/// kernel at 0x8000_8000, boot tags at 0x8000_0100, SVC mode, MMU off.
pub fn boot(card: Nand, cmdline: &str) -> Result<Didj, String> {
    boot_board(card, cmdline, BOARD_ID)
}

/// `boot` reporting board ID `board`.
pub fn boot_board(card: Nand, cmdline: &str, board: u8) -> Result<Didj, String> {
    let image = kernel(&card)?;
    let mut didj = Didj::with_sdram(card, SDRAM_SIZE);
    let bus = &mut didj.bus;
    bus.ram_base = RAM_BASE;
    bus.dma.ram_base = RAM_BASE;
    bus.dma.serve_sd = true;
    bus.explorer = true;
    bus.leappad = true;
    bus.sd = Some(crate::sd::Sdhc::with_card(CARD_CHANNEL));
    bus.nor = Some(crate::nor::image_for(SERIAL, crate::touch::pointercal_for(WIDTH, HEIGHT)));
    bus.sdram.fill(0);
    crate::explorer::start_kernel(&mut didj, &image, cmdline, board)?;
    show_splash(&mut didj.bus);
    didj.cpu.set_mode(mode::SVC);
    Ok(didj)
}
