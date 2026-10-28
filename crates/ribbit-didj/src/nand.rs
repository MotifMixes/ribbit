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
//! controller's BCH decoder but ignores its error flag, so an image built
//! here leaves the spare area erased (no ECC). Whether the kernel's MTD
//! driver checks ECC is UNKNOWN (to be checked at M3).

use crate::package::Firmware;

pub const PAGE: usize = 0x800;
pub const OOB: usize = 64;
pub const PAGE_RAW: usize = PAGE + OOB;
pub const PAGES_PER_BLOCK: usize = 64;
pub const BLOCK: usize = PAGE * PAGES_PER_BLOCK;
pub const BLOCKS: usize = 2048;
pub const SIZE: usize = BLOCK * BLOCKS;

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

pub fn partition(name: &str) -> Option<Partition> {
    PARTITIONS.iter().copied().find(|p| p.name == name)
}

/// The flash, data and spare area. Blocks never written read as erased
/// (0xFF) and take no memory.
pub struct Nand {
    blocks: Vec<Option<Box<[u8]>>>,
}

impl Default for Nand {
    fn default() -> Self {
        Self::erased()
    }
}

impl Nand {
    pub fn erased() -> Self {
        Self { blocks: (0..BLOCKS).map(|_| None).collect() }
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
        let b = self.blocks[n / PAGES_PER_BLOCK].get_or_insert_with(|| vec![0xFF; PAGE_RAW * PAGES_PER_BLOCK].into());
        &mut b[n % PAGES_PER_BLOCK * PAGE_RAW..][..PAGE_RAW]
    }

    /// Erases block `n` (to 0xFF, data and spare).
    pub fn erase(&mut self, n: usize) {
        self.blocks[n] = None;
    }

    pub fn is_bad(&self, block: usize) -> bool {
        (0..2).any(|i| self.page(block * PAGES_PER_BLOCK + i)[PAGE] != 0xFF)
    }

    /// Writes `data` at byte `offset` of the data area (page aligned),
    /// leaving the spare area as it is.
    pub fn write(&mut self, offset: usize, data: &[u8]) {
        assert_eq!(offset % PAGE, 0);
        for (i, chunk) in data.chunks(PAGE).enumerate() {
            self.page_mut(offset / PAGE + i)[..chunk.len()].copy_from_slice(chunk);
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

    /// The flash as LeapFrog Connect leaves it after a firmware install:
    /// lightning-boot, the boot flags, both kernel and root file system
    /// copies, and an erased Brio partition (its file system and the other
    /// packages are not built yet). Manufacturing_Data stays erased: what it
    /// holds on a real console is UNKNOWN.
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
