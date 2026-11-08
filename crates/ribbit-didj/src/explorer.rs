//! LeapFrog Leapster Explorer (2010, codename "Emerald"): the Didj's LF1000
//! with 64 MB of SDRAM at 0x8000_0000 (no shadow mode), Linux 2.6.31 on a
//! UBIFS root, and its own partitions. See docs/explorer.md.
//!
//! LeapFrog's packages carry no boot loader (Emerald Boot lives in the
//! console's NOR flash), so `boot` does its job: the kernel from the Kernel
//! partition is unpacked into SDRAM and started with the command line the
//! shipped boot loader passes.

use ribbit_arm::mode;

use crate::machine::Didj;
use crate::nand::{self, Nand};
use crate::ubifs::{Entry, Kind, Tree};

/// The kernel's built-in partition table (partition_info in the 2.6.31
/// kernel image of Firmware Base 1.4.11.2128). Bulk runs to the end.
pub const PARTITIONS: [(&str, usize); 5] =
    [("Emerald_Boot", 0), ("I18n_Screens", 0x0010_0000), ("Kernel", 0x0020_0000), ("RFS", 0x00A0_0000), ("Bulk", 0x0600_0000)];

/// SDRAM: 64 MB at 0x8000_0000 (the command line's `ram=`).
pub const RAM_BASE: u32 = 0x8000_0000;
pub const SDRAM_SIZE: usize = 64 << 20;

/// 512 MB of 128 KiB blocks. EVIDENCE: a real Explorer's /proc/mtd ends at
/// 0x2000_0000 (Bulk 0x1A00_0000 from 0x0600_0000; ubi_bulk 0x1945_5000 in
/// LEBs of 0x1F800), though board_ids.h names an 8 Gbit part. Ribbit had
/// 1 GB until 2026-11-08: saves made on it go on using it (NAND_BLOCKS_1GB).
pub const NAND_BLOCKS: usize = 4096;
pub const NAND_BLOCKS_1GB: usize = 8192;

/// The board ID Emerald Boot reports (mach-lf1000/board_ids.h): Leapster
/// Explorer, no TV out, no RTC supercap. GUESS: which board retail units
/// report is UNKNOWN.
pub const BOARD_ID: u8 = 0x02;

/// The machine number the kernel accepts (its machine_desc: "ARM-LF1000").
pub const MACHINE: u32 = 2028;
/// The serial number in Ribbit's NOR MfgData: 20 hex digits; libMfgData
/// puts the product (digits 4-9) and board ID (10-11) in on reading.
/// GUESS: the rest of a real number's layout is UNKNOWN.
pub const SERIAL: &str = "00000000000000000001";

/// The command line the shipped Emerald Boot passes (eLinux.org, Leapster
/// Explorer Random Notes).
pub const CMDLINE: &str = "init=/sbin/init console=ttyS0,115200 mem=46M mlc_fb=0x82E00000,0x01200000 \
    ram=0x80000000-0x83FFFFFF screen_module=ILI9322 ebs=0x20000,0x0 root=ubi0_0 rw rootfstype=ubifs ubi.mtd=RFS";

/// Where the kernel image is unpacked (its ZRELADDR) and the boot tags go.
const KERNEL_AT: u32 = RAM_BASE + 0x8000;
const TAGS_AT: u32 = RAM_BASE + 0x100;

/// "CBF" boot images (kernel.cbf, surgeon.cbf): magic, version, load and
/// jump addresses, payload size, header CRC, then the payload (a zImage).
const CBF_MAGIC: u32 = 0x9ABC_DEF0;
const CBF_HEADER: usize = 24;

/// Whether a package file belongs to the Leapster Explorer (its meta.inf).
fn for_explorer(data: &[u8]) -> bool {
    let Ok(entries) = ribbit_core::zip::entries(data) else { return false };
    entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with("meta.inf")).any(|e| {
        ribbit_core::zip::read_entry(data, e).is_ok_and(|m| crate::package::Meta::parse(&String::from_utf8_lossy(&m)).get("Device") == Some("LeapsterExplorer"))
    })
}

/// The members of a tar archive: (path, entry), in archive order. Handles
/// ustar paths with a prefix, GNU long names, symbolic and hard links;
/// pax headers are skipped.
pub fn tar_entries(tar: &[u8]) -> Result<Vec<(String, Entry)>, String> {
    let octal = |f: &[u8]| -> u64 {
        f.iter().skip_while(|&&b| b == b' ').take_while(|&&b| (b'0'..=b'7').contains(&b)).fold(0, |v, &b| v * 8 + (b - b'0') as u64)
    };
    let text = |f: &[u8]| String::from_utf8_lossy(&f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())]).into_owned();
    let mut out: Vec<(String, Entry)> = Vec::new();
    let mut long_name = None;
    let mut at = 0;
    while at + 512 <= tar.len() {
        let h = &tar[at..at + 512];
        if h.iter().all(|&b| b == 0) {
            break;
        }
        let size = octal(&h[124..136]) as usize;
        let data = tar.get(at + 512..at + 512 + size).ok_or("a tar member is cut short")?;
        at += 512 + size.next_multiple_of(512);
        let kind = h[156];
        if kind == b'L' {
            long_name = Some(text(data));
            continue;
        }
        if matches!(kind, b'x' | b'g') {
            continue;
        }
        let mut name = long_name.take().unwrap_or_else(|| {
            let prefix = if &h[257..262] == b"ustar" { text(&h[345..500]) } else { String::new() };
            let name = text(&h[..100]);
            if prefix.is_empty() { name } else { format!("{prefix}/{name}") }
        });
        while let Some(rest) = name.strip_prefix("./") {
            name = rest.to_string();
        }
        let name = name.trim_matches('/').to_string();
        if name.is_empty() || name == "." {
            continue;
        }
        let (mode, mtime) = (octal(&h[100..108]) as u32 & 0o7777, octal(&h[136..148]));
        let kind = match kind {
            b'5' => Kind::Dir,
            b'2' => Kind::Symlink(text(&h[157..257])),
            b'1' => {
                let target = text(&h[157..257]);
                let target = target.trim_start_matches("./").trim_matches('/');
                match out.iter().rev().find(|(n, _)| n == target).map(|(_, e)| e.kind.clone()) {
                    Some(k @ Kind::File(_)) => k,
                    _ => return Err(format!("{name}: hard link to a missing file {target}")),
                }
            }
            b'0' | 0 | b'7' => Kind::File(data.to_vec()),
            // Devices, FIFOs: nothing a package installs.
            _ => continue,
        };
        out.push((name, Entry { kind, mode, mtime }));
    }
    Ok(out)
}

/// Where lfpkg installs a package of this `Type` under /LF (its
/// type_to_dir), relative to Bulk; None for types it puts elsewhere.
fn bulk_folder(package_type: &str) -> Option<&'static str> {
    Some(match package_type {
        "Application" => "ProgramFiles",
        "Avatar" | "CYODataset" | "CustomCurriculum" | "MicroDownload" | "Download" | "Consumable" => "Downloads",
        "LanguagePack" => "",
        "Music" => "Music",
        _ => return None,
    })
}

/// Installs a `.lf2` package (a bzip2-compressed tar) into `tree` (Bulk's
/// files) as the console's lfpkg does: its meta.inf's Type picks the
/// folder; a package whose meta.inf sits in a folder of its own unpacks
/// there as it is, one with meta.inf at the top into a folder named after
/// its PackageID.
pub fn install_lf2(tree: &mut Tree, lf2: &[u8]) -> Result<(), String> {
    use std::io::Read;
    let mut tar = Vec::new();
    bzip2::read::BzDecoder::new(lf2).read_to_end(&mut tar).map_err(|e| format!("bzip2: {e}"))?;
    let entries = tar_entries(&tar)?;
    let meta_at = entries
        .iter()
        .position(|(n, _)| n == "meta.inf")
        .or_else(|| entries.iter().position(|(n, _)| n.ends_with("/meta.inf")))
        .ok_or("no meta.inf in the package")?;
    let Kind::File(text) = &entries[meta_at].1.kind else { return Err("meta.inf isn't a file".into()) };
    let meta = crate::package::Meta::parse(&String::from_utf8_lossy(text));
    let package_type = meta.get("Type").unwrap_or("");
    let folder = bulk_folder(package_type).ok_or_else(|| format!("package type {package_type:?} doesn't install to Bulk"))?;
    let top = entries[meta_at].0.contains('/');
    let base = if top {
        folder.to_string()
    } else {
        let id = meta.get("PackageID").ok_or("meta.inf has no PackageID")?;
        if folder.is_empty() { id.to_string() } else { format!("{folder}/{id}") }
    };
    for (name, entry) in entries {
        let path = if base.is_empty() { name } else { format!("{base}/{name}") };
        tree.insert(path, entry);
    }
    Ok(())
}

/// The internal NAND as LeapFrog Connect leaves it with the packages in
/// `dir`: each "DiskImage" file of the `.lfp` packages, named
/// `<partition>,<size>,<name>`, written raw at the start of its partition
/// (Firmware Base: the splash, kernel and root file system; Firmware Bulk
/// Empty: an empty UBI for Bulk), and the `.lf2` packages installed in
/// Bulk (UBIFS, as lfpkg unpacks them).
pub fn nand_from_packages(dir: &std::path::Path) -> Result<Nand, String> {
    nand_from_packages_blocks(dir, NAND_BLOCKS)
}

/// `nand_from_packages` on a NAND of `blocks` blocks.
pub fn nand_from_packages_blocks(dir: &std::path::Path, blocks: usize) -> Result<Nand, String> {
    let mut flash = Nand::with_blocks(blocks);
    let mut written = Vec::new();
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lfp"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !for_explorer(&data) {
            continue;
        }
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{}: {e}", path.display()))? {
            let file = entry.name.rsplit('/').next().unwrap_or("");
            let mut parts = file.splitn(3, ',');
            let (Some(Ok(n)), Some(Ok(size)), Some(_)) = (parts.next().map(str::parse::<usize>), parts.next().map(str::parse::<usize>), parts.next()) else {
                continue;
            };
            let &(name, offset) = PARTITIONS.get(n).ok_or_else(|| format!("{}: no partition {n}", entry.name))?;
            let bytes = ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{}: {e}", path.display()))?;
            let end = PARTITIONS.get(n + 1).map_or(flash.size(), |p| p.1);
            if bytes.len() != size || offset + size > end {
                return Err(format!("{}: {} bytes don't fit {name}", entry.name, bytes.len()));
            }
            let mut padded = bytes;
            padded.resize(size.next_multiple_of(nand::PAGE), 0xFF);
            flash.write(offset, &padded);
            written.push(name);
        }
    }
    for needed in ["Kernel", "RFS"] {
        if !written.contains(&needed) {
            return Err(format!("no Leapster Explorer firmware (Firmware Base) in {}: its {needed} partition is missing", dir.display()));
        }
    }
    let mut bulk = Tree::new();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lf2"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        install_lf2(&mut bulk, &data).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    if !bulk.is_empty() {
        let offset = PARTITIONS[4].1;
        let part = nand::Partition { name: "Bulk", offset, size: flash.size() - offset };
        let lebs = crate::ubi::available_blocks(part.size / nand::BLOCK);
        let volume = crate::ubifs::build(&bulk, lebs).map_err(|e| format!("Bulk: {e}"))?;
        crate::ubi::write_volume(&mut flash, part, "ubi_bulk", &volume)?;
    }
    Ok(flash)
}

/// The kernel image (unpacked) from a CBF file's zImage.
pub fn kernel_from_cbf(cbf: &[u8]) -> Result<Vec<u8>, String> {
    let word = |o: usize| cbf.get(o..o + 4).map(|b| u32::from_le_bytes(b.try_into().unwrap()));
    if word(0) != Some(CBF_MAGIC) {
        return Err("the Kernel partition holds no CBF image".into());
    }
    let size = word(16).unwrap_or(0) as usize;
    let zimage = cbf.get(CBF_HEADER..CBF_HEADER + size).ok_or("the CBF image is cut short")?;
    // The zImage's decompressor is followed by the gzip stream (piggy.gz).
    let gz = zimage.windows(4).position(|w| w == [0x1F, 0x8B, 0x08, 0x00] || w[..3] == [0x1F, 0x8B, 0x08]).ok_or("no compressed kernel in the zImage")?;
    inflate_gzip(&zimage[gz..])
}

/// A gzip member's contents (RFC 1952 header, raw deflate data).
fn inflate_gzip(gz: &[u8]) -> Result<Vec<u8>, String> {
    let flags = *gz.get(3).ok_or("gzip header cut short")?;
    let mut at = 10;
    if flags & 0x04 != 0 {
        let xlen = u16::from_le_bytes([gz[at], gz[at + 1]]) as usize;
        at += 2 + xlen;
    }
    for bit in [0x08, 0x10] {
        if flags & bit != 0 {
            at += gz[at..].iter().position(|&b| b == 0).ok_or("gzip header cut short")? + 1;
        }
    }
    if flags & 0x02 != 0 {
        at += 2;
    }
    miniz_oxide::inflate::decompress_to_vec(&gz[at..]).or_else(|e| if e.output.is_empty() { Err(format!("kernel: {e}")) } else { Ok(e.output) })
}

/// Powers on as Emerald Boot leaves the machine: the kernel image from the
/// Kernel partition at 0x8000_8000, ATAG boot tags (core, memory, command
/// line) at 0x8000_0100, the processor in SVC mode with interrupts off and
/// the MMU off, r0 = 0, r1 = the machine number, r2 = the tags.
/// The frame buffer (the command line's `mlc_fb=`).
const FRAMEBUFFER: u32 = 0x82E0_0000;

/// Shows the boot splash as Emerald Boot leaves it: FIRST.32.rle from the
/// I18n_Screens partition (the LeapFrog logo) in the frame buffer, on MLC
/// layer 0, 320x240 XRGB (format 0x0653). Linux's frame buffer driver only
/// changes the format, address and enable, so without this the layer stayed
/// 1 pixel big and the screen black until AppManager (LEGAL.png, drawn by
/// rcS into /dev/fb0, never showed). The RLE: 32-bit words, run length in
/// bits 24-31, colour in bits 0-23 as the layer shows it (as LEGAL.png
/// drawn there by rcS; checked against FIRST.png's green). The registers are INFERRED from what the kernel and
/// AppManager set, not read from Emerald Boot.
fn show_splash(bus: &mut crate::machine::Bus) {
    let part = PARTITIONS[1].1;
    let rle = bus.nand.read(part, PARTITIONS[2].1 - part);
    let mut pixels = Vec::with_capacity(320 * 240);
    for w in rle.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)) {
        let (n, c) = ((w >> 24) as usize, w & 0xFF_FFFF);
        if n == 0 || pixels.len() + n > 320 * 240 {
            break;
        }
        pixels.extend(std::iter::repeat_n(0xFF00_0000 | c, n));
    }
    if pixels.len() != 320 * 240 {
        return;
    }
    let at = (FRAMEBUFFER - RAM_BASE) as usize;
    for (i, p) in pixels.iter().enumerate() {
        bus.sdram[at + 4 * i..][..4].copy_from_slice(&p.to_le_bytes());
    }
    let mlc = crate::mlc::BASE;
    for (off, v) in [(0x00, 0x2), (0x04, 239 << 16 | 319), (0x0C, 319), (0x10, 239), (0x24, 0x0653_0020), (0x28, 4), (0x2C, 320 * 4), (0x38, FRAMEBUFFER)] {
        bus.preset(mlc + off, v);
    }
}

pub fn boot(flash: Nand, cmdline: &str) -> Result<Didj, String> {
    let kernel_part = PARTITIONS[2].1;
    let cbf = flash.read(kernel_part, PARTITIONS[3].1 - kernel_part);
    let image = kernel_from_cbf(&cbf)?;
    let mut didj = Didj::with_sdram(flash, SDRAM_SIZE);
    let bus = &mut didj.bus;
    bus.ram_base = RAM_BASE;
    bus.dma.ram_base = RAM_BASE;
    bus.explorer = true;
    bus.nor = Some(crate::nor::image(SERIAL));
    // Didj::with_sdram copied the NAND's first block to SDRAM 0, as the
    // Didj's boot ROM does; the Explorer starts from a clean memory.
    bus.sdram.fill(0);
    let at = (KERNEL_AT - RAM_BASE) as usize;
    bus.sdram.get_mut(at..at + image.len()).ok_or("the kernel doesn't fit in SDRAM")?.copy_from_slice(&image);
    let tags = boot_tags(cmdline);
    let at = (TAGS_AT - RAM_BASE) as usize;
    bus.sdram[at..at + tags.len()].copy_from_slice(&tags);
    // The clocks as the Didj's boot loader sets them (lightning-boot:
    // PLL0 393.2 MHz for the CPU, PLL1 147 MHz for the buses). GUESS: Emerald
    // Boot's values are UNKNOWN; the kernel only reads them.
    for (reg, v) in [(0xC000_F000, 0x0020_0080), (0xC000_F004, 0x00DF_2100), (0xC000_F008, 0x0024_C402)] {
        bus.preset(reg, v);
    }
    // The Alive scratch register as Emerald Boot leaves it (the kernel's
    // mach-lf1000/include/mach/gpio.h layout): power COLD (bits 0-1), boot
    // image PLAY (6-7; RECOVERY makes rcS start recovery mode), board ID
    // (8-12) and boot source NAND (17-19).
    bus.preset(0xC001_9018, 1 | 1 << 6 | (BOARD_ID as u32) << 8 | 2 << 17);
    show_splash(bus);
    didj.jit = crate::machine::jit_for(RAM_BASE, SDRAM_SIZE);
    let cpu = &mut didj.cpu;
    cpu.set_mode(mode::SVC);
    cpu.r[0] = 0;
    cpu.r[1] = MACHINE;
    cpu.r[2] = TAGS_AT;
    cpu.r[15] = KERNEL_AT;
    Ok(didj)
}

/// ATAG_CORE, ATAG_MEM (all of SDRAM; the command line's `mem=` narrows
/// it), ATAG_CMDLINE, ATAG_NONE.
fn boot_tags(cmdline: &str) -> Vec<u8> {
    let mut words: Vec<u32> = vec![5, 0x5441_0001, 1, 4096, 0];
    words.extend([4, 0x5441_0002, SDRAM_SIZE as u32, RAM_BASE]);
    let mut text = cmdline.as_bytes().to_vec();
    text.push(0);
    text.resize(text.len().next_multiple_of(4), 0);
    words.extend([2 + text.len() as u32 / 4, 0x5441_0009]);
    words.extend(text.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())));
    words.extend([0, 0]);
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}
