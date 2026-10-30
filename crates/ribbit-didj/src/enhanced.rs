//! Enhanced mode: fixes for faults in the Didj's own software, applied to
//! the NAND before the console starts. Original mode leaves the firmware as
//! LeapFrog shipped it. docs/didj.md lists the faults, with the evidence.
//!
//! The "Your Didj needs a tune-up" screen (/var/screens/ATTENTION_NEEDED.png)
//! is launch_main's PENDING_REPAIR_STATE, left only by plugging in USB. It
//! comes after any file check with a finding: at power-on after the power
//! went off without a shutdown (init.d/ubi), and after the menu or a game
//! crashed or a download was cut short (`usbctl -a fsck`). Every check runs
//! fsck_watch.sh, which only looks (`fsck.vfat -n`). Fixed here:
//!
//! * **fsck_watch.sh** repairs (`fsck.vfat -a`, no time limit) before its
//!   timed check, so a finding is mended instead of locking the console.
//! * **launch_main** goes back to the menu after a crash instead of turning
//!   the console off (as its own comment intends: "Restart AppManager instead
//!   of powering down"); only a game's segmentation fault did. At most three
//!   times per power-on, in case the menu itself keeps crashing.
//!
//! And for the lag (files loading, the menu, sound) and out-of-memory
//! crashes:
//!
//! * **More memory.** lightning-boot gives Linux `mem=18M` of the 32 MB
//!   (video and 3D memory above), about 16 MB for the system and the game,
//!   with no swap and a small file cache. Enhanced adds a second 32 MB of
//!   SDRAM at 32 MB (`SDRAM_SIZE`) and gives it to Linux as a second bank
//!   (`mem=32M@32M mem=18M`): 50 MB, as on LeapFrog's 64 MB development board
//!   (board.h, CONFIG_MACH_ME_LF1000: `mem=50M`). The video memory stays
//!   where the drivers expect it.
//!
//! The root file system is JFFS2, mounted read-only (`root=31:04 ro`), so a
//! changed file is newer nodes for its inode, written to the first unused
//! block after the image in both copies (Linux_RFS0/1). The kernel never
//! writes there, so the mode can change at any power-on; the save keeps the
//! block like any other, and the next start writes or erases it again.

use crate::jffs2;
use crate::nand::{partition, Nand, BLOCK};

const ROOT_FILE_SYSTEMS: [&str; 2] = ["Linux_RFS0", "Linux_RFS1"];

/// Enhanced mode's SDRAM: the Didj's 32 MB and 32 MB more after it.
pub const SDRAM_SIZE: usize = 64 << 20;

/// The SDRAM to power on with.
pub fn sdram_size(enhanced: bool) -> usize {
    if enhanced { SDRAM_SIZE } else { crate::machine::SDRAM_SIZE }
}

/// lightning-boot's kernel command lines (four: two root file systems, with
/// and without a cartridge) start with this; Enhanced gives the kernel the
/// second bank instead of the default `init=` (/sbin/init either way). The
/// high bank comes first: with the banks out of order the kernel keeps the
/// page structures of the gap between them ("MEM: unordered memory banks.
/// Not freeing memmap.", arch/arm/mm/init.c), which the video memory there
/// needs (pfn_valid() is true for it; in order, a freed memmap made
/// AppManager's mapping of the screen oops).
const MEM_OLD: &[u8] = b"mem=18M init=/sbin/init ";
const MEM_NEW: &[u8] = b"mem=32M@32M mem=18M     ";

/// A change to one file of the root file system: `old` (found once) becomes
/// `new`.
struct Edit {
    file: &'static str,
    old: &'static str,
    new: &'static str,
}

const EDITS: [Edit; 2] = [
    Edit {
        file: "fsck_watch.sh",
        old: "\ntouch $LOGFILE",
        new: "
# Ribbit (Enhanced mode): repair first, with no time limit. The check
# alone (-n) left any finding locked behind the tune-up screen.
/sbin/fsck.vfat -a $DEVICE > /tmp/fsck_repair.log 2>&1
touch $LOGFILE",
    },
    Edit {
        file: "launch_main",
        old: "\t\t\t\t\telif [ -e $RUN_APP_FILE -a $RESULT = 139 ]; then
\t\t\t\t\t\tnew_state RUN_MAIN_APP_STATE
\t\t\t\t\telse
",
        new: "\t\t\t\t\telif [ -e $RUN_APP_FILE -a $RESULT = 139 ]; then
\t\t\t\t\t\tnew_state RUN_MAIN_APP_STATE
\t\t\t\t\telif [ ${CRASH_RESTARTS:-0} -lt 3 ]; then
\t\t\t\t\t\t# Ribbit (Enhanced mode): back to the menu after a crash,
\t\t\t\t\t\t# not off (at most 3 times per power-on).
\t\t\t\t\t\tCRASH_RESTARTS=$((${CRASH_RESTARTS:-0} + 1))
\t\t\t\t\t\trm -f $RUN_APP_FILE
\t\t\t\t\t\tnew_state RUN_MAIN_APP_STATE
\t\t\t\t\telse
",
    },
];

/// Writes (`enhanced`) or erases (Original) the fixes in `flash`. `original`
/// is the NAND as built from the packages; it says where each image ends.
/// Returns the fixes made: 1 for the boot command lines and 1 per file per
/// root file system (5 when enhanced; fewer if a firmware differs).
pub fn apply(flash: &mut Nand, original: &Nand, enhanced: bool) -> usize {
    let mut fixed = 0;
    let boot = partition("LF1000_uniboot").unwrap();
    let mut loader = original.read(boot.offset, boot.size);
    if enhanced {
        fixed += replace_all(&mut loader, MEM_OLD, MEM_NEW).min(1);
    }
    for b in boot.offset / BLOCK..(boot.offset + boot.size) / BLOCK {
        flash.erase(b);
    }
    flash.write(boot.offset, &loader);
    for name in ROOT_FILE_SYSTEMS {
        let p = partition(name).unwrap();
        let (first, end) = (p.offset / BLOCK, (p.offset + p.size) / BLOCK);
        let Some(used) = (first..end).rev().find(|&b| original.is_written(b)) else { continue };
        let spare = used + 1;
        if spare >= end {
            continue;
        }
        flash.erase(spare);
        if enhanced {
            let image = original.read(p.offset, (spare - first) * BLOCK);
            let mut nodes = Vec::new();
            for edit in &EDITS {
                if let Some(n) = edited(&image, edit) {
                    nodes.extend(n);
                    fixed += 1;
                }
            }
            if !nodes.is_empty() && nodes.len() <= BLOCK {
                flash.write(spare * BLOCK, &nodes);
            }
        }
    }
    fixed
}

/// Replaces every `old` in `data` with `new` (the same length); returns how
/// many.
fn replace_all(data: &mut [u8], old: &[u8], new: &[u8]) -> usize {
    debug_assert_eq!(old.len(), new.len());
    let mut n = 0;
    let mut i = 0;
    while let Some(at) = data[i..].windows(old.len()).position(|w| w == old) {
        data[i + at..i + at + old.len()].copy_from_slice(new);
        i += at + old.len();
        n += 1;
    }
    n
}

/// The nodes giving `edit.file` its change, if `image` has that file with
/// the text to change.
fn edited(image: &[u8], edit: &Edit) -> Option<Vec<u8>> {
    let file = jffs2::read_file(image, edit.file)?;
    let text = String::from_utf8(file.data).ok()?;
    if text.matches(edit.old).count() != 1 {
        return None;
    }
    Some(jffs2::rewrite(&file.header, text.replacen(edit.old, edit.new, 1).as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_command_lines() {
        let mut d = b"xxmem=18M init=/sbin/init console=1\0mem=18M init=/sbin/init root\0".to_vec();
        assert_eq!(replace_all(&mut d, MEM_OLD, MEM_NEW), 2);
        assert!(d.starts_with(b"xxmem=32M@32M mem=18M     console=1\0mem=32M@32M mem=18M     root"));
    }

    #[test]
    fn edits_a_file() {
        let script = "#!/bin/sh\nDEVICE=$1\ntouch $LOGFILE\t# start\n";
        let mut image = jffs2::new_file(7, "fsck_watch.sh", script.as_bytes(), 0);
        let node = edited(&image, &EDITS[0]).unwrap();
        image.extend(node);
        let text = String::from_utf8(jffs2::read_file(&image, "fsck_watch.sh").unwrap().data).unwrap();
        assert!(text.starts_with("#!/bin/sh\nDEVICE=$1\n# Ribbit"));
        assert!(text.ends_with("2>&1\ntouch $LOGFILE\t# start\n"));
        // Text that isn't there: no change.
        let image = jffs2::new_file(7, "launch_main", b"#!/bin/sh\n", 0);
        assert!(edited(&image, &EDITS[1]).is_none());
    }
}
