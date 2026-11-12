//! The Didj's internal NAND: 256 MB of large-page flash (2 KiB pages with 64
//! bytes of spare area, 64 pages per 128 KiB erase block).
//!
//! Sources: lightning-boot nand.c/nand.h and the kernel's
//! drivers/mtd/nand/lf1000.c (opendidj, linux-2.6.20-lf1000); the partition
//! sizes are that driver's Kconfig defaults and match the map on eLinux.org.
//! The page and block sizes agree with the packages: kernel.bin's summary
//! counts 128 KiB blocks (tfs.rs), and every JFFS2 image is a whole number
//! of them.
//!
//! A block is bad when the first spare byte of its page 0 or 1 is not 0xFF
//! (lightning-boot `nand_check_block`). Lightning-boot reads with the
//! controller's BCH decoder but ignores its error flag. The kernel uses
//! Linux's software ECC instead (lf1000_defconfig: MTD_NAND_LF1000_HWECC
//! not set; VERIFIED in a boot: with erased spare bytes it "corrects" good
//! data): 3 bytes per 256 data bytes, in spare bytes 40-63 (nand_oob_64).
//! Pages written here get that ECC, as the console's kernel writes it.

use std::sync::Arc;

use crate::package::Firmware;

pub const PAGE: usize = 0x800;
pub const OOB: usize = 64;
pub const PAGE_RAW: usize = PAGE + OOB;
pub const PAGES_PER_BLOCK: usize = 64;
pub const BLOCK: usize = PAGE * PAGES_PER_BLOCK;
pub const BLOCKS: usize = 2048;
pub const SIZE: usize = BLOCK * BLOCKS;

/// Where the software ECC starts in the spare area (nand_oob_64 eccpos).
pub const ECC_POS: usize = 40;

/// Linux 2.6.20's software ECC of 256 bytes (drivers/mtd/nand/nand_ecc.c,
/// without CONFIG_MTD_NAND_ECC_SMC): a Hamming code of 6 column parities
/// and 16 line parities, inverted. All-0xFF data gives FF FF FF.
pub fn soft_ecc(data: &[u8; 256]) -> [u8; 3] {
    // Column parities: bits 0,2,4,6 / 1,3,5,7 / 0,1,4,5 / 2,3,6,7 /
    // 0-3 / 4-7, and (bit 6) the byte's parity: nand_ecc_precalc_table.
    const TABLE: [u8; 256] = {
        const GROUPS: [u8; 7] = [0x55, 0xAA, 0x33, 0xCC, 0x0F, 0xF0, 0xFF];
        let mut t = [0u8; 256];
        let mut b = 0;
        while b < 256 {
            let mut k = 0;
            while k < 7 {
                t[b] |= (((b as u8 & GROUPS[k]).count_ones() & 1) as u8) << k;
                k += 1;
            }
            b += 1;
        }
        t
    };
    let (mut reg1, mut reg2, mut reg3) = (0u8, 0u8, 0u8);
    for (i, &b) in data.iter().enumerate() {
        let idx = TABLE[b as usize];
        reg1 ^= idx & 0x3F;
        if idx & 0x40 != 0 {
            reg3 ^= i as u8;
            reg2 ^= !(i as u8);
        }
    }
    // Interleave the line parities: reg3 bit n, reg2 bit n, high bits first.
    let (mut tmp1, mut tmp2) = (0u8, 0u8);
    for n in 0..4 {
        tmp1 |= (reg3 >> (7 - n) & 1) << (7 - 2 * n) | (reg2 >> (7 - n) & 1) << (6 - 2 * n);
        tmp2 |= (reg3 >> (3 - n) & 1) << (7 - 2 * n) | (reg2 >> (3 - n) & 1) << (6 - 2 * n);
    }
    [!tmp1, !tmp2, (!reg1) << 2 | 0x03]
}

/// A partition (kernel MTD names).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Partition {
    pub name: &'static str,
    pub offset: usize,
    pub size: usize,
}

pub const PARTITIONS: [Partition; 8] = [
    Partition { name: "LF1000_uniboot", offset: 0, size: 0x0002_0000 },
    Partition { name: "Atomic_Boot_Flags", offset: 0x0002_0000, size: 0x000E_0000 },
    Partition { name: "Manufacturing_Data", offset: 0x0010_0000, size: 0x0010_0000 },
    Partition { name: "Kernel0", offset: 0x0020_0000, size: 0x0020_0000 },
    Partition { name: "Linux_RFS0", offset: 0x0040_0000, size: 0x00E0_0000 },
    Partition { name: "Kernel1", offset: 0x0120_0000, size: 0x0020_0000 },
    Partition { name: "Linux_RFS1", offset: 0x0140_0000, size: 0x00E0_0000 },
    Partition { name: "Brio", offset: 0x0220_0000, size: 0x0DE0_0000 },
];

/// The serial number a console leaves the factory with, in
/// /mfgdata/UnitID.txt (the only file the firmware reads there). Its form is
/// what init.d/mass_storage checks: 16 hex digits, starting "00000E" and the
/// board revision, 03 (machine.rs BOARD_ID; VERIFIED: real consoles'
/// 00000E0300CF058C, 00000E0300F6CF52, 00000E0300EF70C0). The last 8
/// digits are Ribbit's own. Saves made before keep 00000E0000000001.
/// Without it, a start with /flags/needs_repair set dies in mass_storage (`set -e`, then "echo > /Didj/brd.rev" on the unmounted
/// /Didj), the USB driver never loads, launch_main drops the flag and shows
/// "Download Failed" where a real console shows the tune-up screen.
pub const UNIT_ID: &str = "00000E0300000001\n";

pub fn partition(name: &str) -> Option<Partition> {
    PARTITIONS.iter().copied().find(|p| p.name == name)
}

/// The flash, data and spare area. Blocks never written read as erased
/// (0xFF) and take no memory. Blocks are shared between clones and copied
/// when one of them writes (copy on write): a running console's NAND and
/// the image its save is compared with (`changes_from`) take the memory of
/// one image and the blocks written since.
#[derive(Clone, PartialEq, Eq)]
pub struct Nand {
    blocks: Vec<Option<Arc<Vec<u8>>>>,
}

impl Default for Nand {
    fn default() -> Self {
        Self::erased()
    }
}

impl Nand {
    /// The Didj's 256 MB, erased.
    pub fn erased() -> Self {
        Self::with_blocks(BLOCKS)
    }

    /// An erased flash of `blocks` blocks (the Leapster Explorer's is 1 GB).
    pub fn with_blocks(blocks: usize) -> Self {
        Self { blocks: (0..blocks).map(|_| None).collect() }
    }

    pub fn blocks(&self) -> usize {
        self.blocks.len()
    }

    /// The data area's size in bytes.
    pub fn size(&self) -> usize {
        self.blocks.len() * BLOCK
    }

    /// Page `n` with its spare area (PAGE_RAW bytes).
    pub fn page(&self, n: usize) -> &[u8] {
        static ERASED: [u8; PAGE_RAW] = [0xFF; PAGE_RAW];
        match &self.blocks[n / PAGES_PER_BLOCK] {
            Some(b) => &b[n % PAGES_PER_BLOCK * PAGE_RAW..][..PAGE_RAW],
            None => &ERASED,
        }
    }

    pub fn page_mut(&mut self, n: usize) -> &mut [u8] {
        let b = self.blocks[n / PAGES_PER_BLOCK].get_or_insert_with(|| Arc::new(vec![0xFF; PAGE_RAW * PAGES_PER_BLOCK]));
        &mut Arc::make_mut(b)[n % PAGES_PER_BLOCK * PAGE_RAW..][..PAGE_RAW]
    }

    /// Writes the factory's serial number (`UNIT_ID`) to an empty
    /// Manufacturing_Data partition, as a JFFS2 file /mfgdata/UnitID.txt.
    /// Returns whether it was written.
    pub fn add_factory_data(&mut self) -> bool {
        let p = partition("Manufacturing_Data").unwrap();
        if (p.offset / BLOCK..(p.offset + p.size) / BLOCK).any(|b| self.is_written(b)) {
            return false;
        }
        self.write(p.offset, &crate::jffs2::new_file(2, "UnitID.txt", UNIT_ID.as_bytes(), 0));
        true
    }

    /// Blocks `first..first + count` as `from` has them (shared, not
    /// copied).
    pub fn copy_blocks(&mut self, from: &Nand, first: usize, count: usize) {
        self.blocks[first..first + count].clone_from_slice(&from.blocks[first..first + count]);
    }

    /// Whether block `n` was written since it was last erased.
    pub fn is_written(&self, n: usize) -> bool {
        self.blocks[n].is_some()
    }

    /// Erases block `n` (to 0xFF, data and spare).
    pub fn erase(&mut self, n: usize) {
        self.blocks[n] = None;
    }

    /// The blocks that differ from `base`, for a save file: each a 32-bit
    /// block number, then a byte (0 erased, 1 written) and, when written,
    /// the block with its spare areas.
    pub fn changes_from(&self, base: &Nand) -> Vec<u8> {
        let mut out = Vec::new();
        for (n, (a, b)) in self.blocks.iter().zip(&base.blocks).enumerate() {
            // Shared blocks are the same without comparing them.
            let same = match (a, b) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
                (None, None) => true,
                _ => false,
            };
            if !same {
                out.extend((n as u32).to_le_bytes());
                match a {
                    Some(data) => {
                        out.push(1);
                        out.extend_from_slice(data);
                    }
                    None => out.push(0),
                }
            }
        }
        out
    }

    /// A CRC-32 of every block (erased ones as their number only), to tell
    /// whether a save's changes were made against this image.
    pub fn fingerprint(&self) -> u32 {
        let mut h = crc32fast::Hasher::new();
        for (n, b) in self.blocks.iter().enumerate() {
            h.update(&(n as u32).to_le_bytes());
            if let Some(data) = b {
                h.update(data);
            }
        }
        h.finalize()
    }

    /// Applies `changes_from`'s output.
    pub fn apply_changes(&mut self, mut changes: &[u8]) -> Result<(), String> {
        const RAW: usize = PAGE_RAW * PAGES_PER_BLOCK;
        while !changes.is_empty() {
            let bad = || "a damaged NAND save".to_string();
            let n = u32::from_le_bytes(changes.get(..4).ok_or_else(bad)?.try_into().unwrap()) as usize;
            if n >= self.blocks.len() {
                return Err(bad());
            }
            match changes.get(4) {
                Some(0) => {
                    self.blocks[n] = None;
                    changes = &changes[5..];
                }
                Some(1) => {
                    self.blocks[n] = Some(Arc::new(changes.get(5..5 + RAW).ok_or_else(bad)?.to_vec()));
                    changes = &changes[5 + RAW..];
                }
                _ => return Err(bad()),
            }
        }
        Ok(())
    }

    pub fn is_bad(&self, block: usize) -> bool {
        (0..2).any(|i| self.page(block * PAGES_PER_BLOCK + i)[PAGE] != 0xFF)
    }

    /// Writes `data` at byte `offset` of the data area (page aligned), with
    /// each page's software ECC in its spare area.
    pub fn write(&mut self, offset: usize, data: &[u8]) {
        assert_eq!(offset % PAGE, 0);
        for (i, chunk) in data.chunks(PAGE).enumerate() {
            let p = self.page_mut(offset / PAGE + i);
            p[..chunk.len()].copy_from_slice(chunk);
            for step in 0..PAGE / 256 {
                let ecc = soft_ecc(p[step * 256..][..256].try_into().unwrap());
                p[PAGE + ECC_POS + step * 3..][..3].copy_from_slice(&ecc);
            }
        }
    }

    /// Reads the data area from byte `offset` (spare areas skipped).
    pub fn read(&self, offset: usize, len: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(len);
        let mut o = offset;
        while out.len() < len {
            let p = self.page(o / PAGE);
            let n = (PAGE - o % PAGE).min(len - out.len());
            out.extend_from_slice(&p[o % PAGE..][..n]);
            o += n;
        }
        out
    }

    /// A real console's flash from a dump: the 256 MB data area alone
    /// (`nanddump` of /dev/mtd0..mtd7 put end to end, or `cat` of their
    /// mtdblocks; spare areas then get Ribbit's ECC), or every page with
    /// its 64-byte spare area (`nanddump -o`, bad-block marks kept). Erased
    /// blocks take no memory.
    pub fn from_dump(data: &[u8]) -> Result<Self, String> {
        let raw = BLOCKS * PAGES_PER_BLOCK * PAGE_RAW;
        let mut nand = Self::erased();
        match data.len() {
            SIZE => {
                for (b, block) in data.chunks(BLOCK).enumerate() {
                    if block.iter().any(|&x| x != 0xFF) {
                        nand.write(b * BLOCK, block);
                    }
                }
            }
            n if n == raw => {
                for (b, block) in data.chunks(BLOCK / PAGE * PAGE_RAW).enumerate() {
                    if block.iter().any(|&x| x != 0xFF) {
                        nand.blocks[b] = Some(Arc::new(block.to_vec()));
                    }
                }
            }
            n => return Err(format!("a Didj NAND dump is {SIZE} bytes, or {raw} with spare areas, not {n}")),
        }
        Ok(nand)
    }

    /// The flash as LeapFrog Connect leaves it after a firmware install:
    /// lightning-boot, the boot flags, both kernel and root file system
    /// copies, and an erased Brio partition (brio::install writes it with the
    /// other packages). Manufacturing_Data stays erased here; the factory's
    /// serial number is added at power-on (`add_factory_data`), so saves
    /// made before it keep matching this image.
    pub fn from_firmware(fw: &Firmware) -> Result<Self, String> {
        let missing = fw.missing();
        if !missing.is_empty() {
            return Err(format!("missing firmware files: {}", missing.join(", ")));
        }
        let mut nand = Self::erased();
        let mut put = |part: &str, data: &[u8]| -> Result<(), String> {
            let p = partition(part).unwrap();
            if data.len() > p.size {
                return Err(format!("{} bytes don't fit {part} ({} bytes)", data.len(), p.size));
            }
            nand.write(p.offset, data);
            Ok(())
        };
        put("LF1000_uniboot", fw.lightning_boot.as_deref().unwrap())?;
        put("Atomic_Boot_Flags", fw.bootflags.as_deref().unwrap())?;
        for k in ["Kernel0", "Kernel1"] {
            put(k, fw.kernel.as_deref().unwrap())?;
        }
        for r in ["Linux_RFS0", "Linux_RFS1"] {
            put(r, fw.rootfs.as_deref().unwrap())?;
        }
        Ok(nand)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dumps_with_and_without_spare_areas() {
        let mut built = Nand::erased();
        built.write(partition("Kernel0").unwrap().offset, &[0x42; 3 * PAGE]);
        built.page_mut(5 * PAGES_PER_BLOCK)[PAGE] = 0; // a bad-block mark
        let data = built.read(0, SIZE);
        let from_data = Nand::from_dump(&data).unwrap();
        assert_eq!(from_data.read(0, SIZE), data);
        assert_eq!(from_data.page(partition("Kernel0").unwrap().offset / PAGE), built.page(partition("Kernel0").unwrap().offset / PAGE));
        let raw: Vec<u8> = (0..BLOCKS * PAGES_PER_BLOCK).flat_map(|p| built.page(p).to_vec()).collect();
        let from_raw = Nand::from_dump(&raw).unwrap();
        assert!(from_raw == built, "every page and spare area, the bad block too");
        assert!(from_raw.is_bad(5) && !from_data.is_bad(5));
        assert!(Nand::from_dump(&[0; 100]).is_err());
    }

    #[test]
    fn partitions_tile_the_flash() {
        let mut end = 0;
        for p in PARTITIONS {
            assert_eq!(p.offset, end, "{}", p.name);
            assert_eq!(p.size % BLOCK, 0);
            end += p.size;
        }
        assert_eq!(end, SIZE);
    }

    #[test]
    fn write_read_and_bad_blocks() {
        let mut n = Nand::erased();
        let data: Vec<u8> = (0..PAGE * 3).map(|i| i as u8).collect();
        n.write(BLOCK, &data);
        assert_eq!(n.read(BLOCK + 5, PAGE * 2), data[5..5 + PAGE * 2]);
        assert_eq!(n.page(PAGES_PER_BLOCK)[PAGE..], [0xFF; OOB]);
        assert!(!n.is_bad(1));
        n.page_mut(PAGES_PER_BLOCK + 1)[PAGE] = 0;
        assert!(n.is_bad(1));
        n.erase(1);
        assert!(!n.is_bad(1));
        assert_eq!(n.read(BLOCK, 4), [0xFF; 4]);
    }

    #[test]
    fn soft_ecc_values() {
        assert_eq!(soft_ecc(&[0xFF; 256]), [0xFF; 3]);
        assert_eq!(soft_ecc(&[0; 256]), [0xFF; 3]);
        // One set bit (byte 0, bit 0): line parity at line 0, column 0.
        let mut d = [0u8; 256];
        d[0] = 1;
        assert_eq!(soft_ecc(&d), [0xAA, 0xAA, 0xAB]);
    }

    #[test]
    fn firmware_layout() {
        let fw = Firmware {
            lightning_boot: Some(vec![1; 100]),
            bootflags: Some(vec![2; BLOCK]),
            kernel: Some(vec![3; BLOCK * 10]),
            rootfs: Some(vec![4; BLOCK * 54]),
        };
        let n = Nand::from_firmware(&fw).unwrap();
        assert_eq!(n.read(0, 101)[99..], [1, 0xFF]);
        assert_eq!(n.read(0x2_0000, 1), [2]);
        assert_eq!(n.read(0x120_0000 + BLOCK * 10 - 1, 2), [3, 0xFF]);
        assert_eq!(n.read(0x140_0000, 1), [4]);
        assert!(Nand::from_firmware(&Firmware::default()).is_err());
    }
}

#[cfg(test)]
mod save_tests {
    use super::*;

    #[test]
    fn changes_round_trip() {
        let mut base = Nand::erased();
        base.write(0, &[1; PAGE]);
        base.write(5 * BLOCK, &[2; PAGE]);
        let mut now = base.clone();
        now.write(3 * BLOCK, &[3; PAGE]);
        now.erase(5);
        let changes = now.changes_from(&base);
        let mut back = base.clone();
        back.apply_changes(&changes).unwrap();
        assert!(back == now);
        assert!(base.changes_from(&base).is_empty());
        assert!(back.apply_changes(&changes[..7]).is_err());
    }
}
