//! LeapPad Explorer Enhanced (new): the LeapPad's programs on the HLE, as
//! the Leapster Explorer's (`explorer.rs`): its firmware is the Explorer's
//! later one (2.x), so only the file systems and the kernel's answers
//! differ: the root and Bulk come from the SD card's ext file systems
//! (`leappad::card_from_packages`), the board ID is 0x0B (Madrid) and the
//! screen 480x272.

use super::vfs::{Data, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, Vfs};

/// /sys/devices/platform files that differ from the Explorer's
/// (`explorer::PLATFORM_FILES`): VERIFIED for board_id, xres and yres from
/// the kernel's log on Original ("Reading Board ID = 0x0b", "screen:
/// 480x272"); the touch calibration is Ribbit's (`touch::pointercal_for`).
pub(super) fn platform_overrides() -> Vec<(&'static str, String)> {
    let cal = crate::touch::pointercal_for(crate::leappad::WIDTH, crate::leappad::HEIGHT);
    let [a1, a2, a0, a4, a5, a3, a6] = cal;
    vec![
        ("lf1000-gpio/board_id", "B\n".into()),
        ("lf1000-dpc/xres", format!("{}\n", crate::leappad::WIDTH)),
        ("lf1000-dpc/yres", format!("{}\n", crate::leappad::HEIGHT)),
        ("lf1000-alvgpio/scratchpad", "00040b41\n".into()),
        ("lf1000-touchscreen/a0", format!("{a0}\n")),
        ("lf1000-touchscreen/a1", format!("{a1}\n")),
        ("lf1000-touchscreen/a2", format!("{a2}\n")),
        ("lf1000-touchscreen/a3", format!("{a3}\n")),
        ("lf1000-touchscreen/a4", format!("{a4}\n")),
        ("lf1000-touchscreen/a5", format!("{a5}\n")),
        ("lf1000-touchscreen/a6", format!("{a6}\n")),
        ("lf1000-touchscreen/pointercal", format!("{a1} {a2} {a0} {a4} {a5} {a3} {a6}\n")),
    ]
}

/// Puts ext2 files into `vfs` under `at` ("" for the root).
fn add_files(vfs: &mut Vfs, at: &str, files: Vec<crate::ext2::File>) {
    for f in files {
        let path = format!("{at}/{}", f.path);
        let data = match f.mode & S_IFMT {
            S_IFDIR => Data::Dir,
            S_IFREG => Data::File(f.data),
            S_IFLNK => Data::Link(String::from_utf8_lossy(&f.data).into_owned()),
            _ => continue,
        };
        if matches!(data, Data::Dir) && vfs.get(&path).is_some_and(|n| n.is_dir()) {
            continue;
        }
        vfs.mkdir_all(super::vfs::split(&path).0);
        vfs.insert(&path, f.mode, data);
        if let Some(n) = vfs.get_mut(&path) {
            n.mtime = f.mtime;
        }
    }
}

/// The LeapPad's file systems out of its SD card (as Original leaves it:
/// the packages built in and the app's save applied): partition 3 the
/// root, 4 Bulk at /LF/Bulk, with /dev, /sys, /proc and the NOR's
/// partitions (`nor`, else Ribbit's).
pub fn system_from_card(card: &crate::nand::Nand, nor: Option<&[u8]>) -> Result<Vfs, String> {
    let root = crate::leappad::files_in(card, 3).map_err(|e| format!("root: {e}"))?;
    let bulk = crate::leappad::files_in(card, 4).map_err(|e| format!("Bulk: {e}"))?;
    let mut vfs = Vfs::new();
    add_files(&mut vfs, "", root);
    vfs.mkdir_all("/LF/Bulk");
    add_files(&mut vfs, "/LF/Bulk", bulk);
    let nor = nor.map(<[u8]>::to_vec).unwrap_or_else(|| crate::nor::image_for(crate::leappad::SERIAL, crate::touch::pointercal_for(crate::leappad::WIDTH, crate::leappad::HEIGHT)).data);
    super::explorer::finish_with(&mut vfs, Some(&nor), true);
    Ok(vfs)
}

/// CRC-32s of the root and Bulk trees (`write_changed_to_card` writes a
/// volume only when its own changed).
pub fn fingerprints(vfs: &Vfs) -> [u32; 2] {
    let [root, bulk] = trees(vfs);
    [tree_crc(&root), tree_crc(&bulk)]
}

fn tree_crc(tree: &crate::ubifs::Tree) -> u32 {
    use crate::ubifs::Kind;
    let mut h = crc32fast::Hasher::new();
    for (p, e) in tree {
        h.update(p.as_bytes());
        h.update(&e.mode.to_le_bytes());
        match &e.kind {
            Kind::File(d) => h.update(d),
            Kind::Symlink(t) => h.update(t.as_bytes()),
            _ => {}
        }
    }
    h.finalize()
}

fn trees(vfs: &Vfs) -> [crate::ubifs::Tree; 2] {
    let root = super::explorer::saved_tree(vfs, "", |_| true);
    let bulk = super::explorer::saved_tree(vfs, "/LF/Bulk", |_| true);
    [root, bulk]
}

/// Writes the HLE's root file system and Bulk into `card` (partitions 3
/// and 4, built again as ext2) when they changed since `written`, so a
/// save made in Enhanced (new) boots in Original too.
pub fn write_changed_to_card(vfs: &Vfs, card: &mut crate::nand::Nand, written: &mut [u32; 2]) -> Result<(), String> {
    let now = trees(vfs);
    let parts = crate::leappad::partitions(&card.read(0, 512));
    for (i, tree) in now.iter().enumerate() {
        let crc = tree_crc(tree);
        if crc == written[i] {
            continue;
        }
        let part = parts.get(2 + i).copied().ok_or("the card has no such partition")?;
        crate::leappad::write_fs(card, part, tree)?;
        written[i] = crc;
    }
    Ok(())
}

impl super::Process {
    /// A process for the LeapPad Explorer's programs: the Explorer's, with
    /// its 480x272 screen.
    pub fn new_leappad(vfs: Vfs) -> Self {
        let mut p = Self::new_explorer_sized(vfs, crate::leappad::WIDTH, crate::leappad::HEIGHT);
        p.x.leappad = true;
        p
    }

    /// The screen's size in pixels (320x240, or the LeapPad's 480x272).
    pub fn screen(&self) -> (u32, u32) {
        self.x.screen()
    }

    /// Draws the boot loader's splash (`leappad::splash_pixels`).
    pub fn leappad_splash(&mut self, card: &crate::nand::Nand) {
        let Some(pixels) = crate::leappad::splash_pixels(card) else { return };
        let at = super::explorer::FB_BASE as usize;
        for (i, px) in pixels.iter().enumerate() {
            self.m.bus.sdram[at + 4 * i..][..4].copy_from_slice(&px.to_le_bytes());
        }
    }
}
