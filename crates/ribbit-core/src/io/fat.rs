//! A freshly formatted SD card: a partition table, one FAT16 partition, empty root.
//!
//! Why: the Leapster 2 BIOS (0.6) never formats its SD volume. Its FAT
//! start-up (0x400D_0680 onwards) formats only volumes whose descriptor marks
//! them volatile (the RAM disk, "Leapster RD"); for the SD volume it prints
//! "Fat Volume %s (%s) not inserted" or uses it as found. On a blank card its
//! base system then fails ("Failed trying to mkdir 'b:\Leapster'"), so real
//! units ship with a formatted card. Layout: VERIFIED-FW, the BIOS's FAT mount
//! (0x400D_46E4, messages at 0x4013_80D4-0x4013_8394) requires a partition
//! table in sector 0 whose first entry is marked active (boot flag 0x80, read
//! at 0x400D_4800) with a FAT type (1, 4, 6, 0x0B, 0x0C or 0x0E, checked at
//! 0x400D_4684); a volume starting at sector 0 ("Partition table jump code or
//! media type is not valid") or an inactive entry is not mounted. HYPOTHESIS:
//! FAT16 with 2 KiB clusters, the partition at sector 32, and the BIOS's own
//! label for this volume, "Leapster SD" (0x4013_6884). The factory card's
//! contents are UNKNOWN.

/// Sector of the partition's boot sector (HYPOTHESIS, a usual alignment).
pub const PARTITION_START: u32 = 32;
const SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: u32 = 4;
const RESERVED: u32 = 1;
const FATS: u32 = 2;
const ROOT_ENTRIES: u32 = 512;
const LABEL: &[u8; 11] = b"Leapster SD";
const MEDIA: u8 = 0xF0;

/// The non-zero sectors of a card of `total` sectors, freshly formatted.
pub fn format_fat16(total: u32) -> Vec<(u32, Vec<u8>)> {
    let part = total - PARTITION_START;
    let root_sectors = ROOT_ENTRIES * 32 / SECTOR as u32;
    // FAT size: enough 16-bit entries for every cluster that fits after it.
    let mut fat = 1;
    loop {
        let clusters = (part - RESERVED - FATS * fat - root_sectors) / SECTORS_PER_CLUSTER;
        let need = ((clusters + 2) * 2).div_ceil(SECTOR as u32);
        if need <= fat {
            break;
        }
        fat = need;
    }
    let mut out = Vec::new();

    let mut mbr = vec![0u8; SECTOR];
    let entry = &mut mbr[0x1BE..0x1CE];
    entry[0] = 0x80; // active: the BIOS mounts only an active partition
    entry[4] = 0x06; // FAT16 (> 32 MiB)
    // CHS fields unused (LBA).
    entry[1..4].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    entry[5..8].copy_from_slice(&[0xFE, 0xFF, 0xFF]);
    entry[8..12].copy_from_slice(&PARTITION_START.to_le_bytes());
    entry[12..16].copy_from_slice(&part.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    out.push((0, mbr));

    let mut b = vec![0u8; SECTOR];
    b[0..3].copy_from_slice(&[0xEB, 0x3C, 0x90]);
    b[3..11].copy_from_slice(b"MSDOS5.0");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = SECTORS_PER_CLUSTER as u8;
    b[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    b[16] = FATS as u8;
    b[17..19].copy_from_slice(&(ROOT_ENTRIES as u16).to_le_bytes());
    // Total sectors in the 32-bit field (more than 65535).
    b[21] = MEDIA;
    b[22..24].copy_from_slice(&(fat as u16).to_le_bytes());
    b[24..26].copy_from_slice(&8u16.to_le_bytes()); // sectors per track
    b[26..28].copy_from_slice(&16u16.to_le_bytes()); // heads
    b[28..32].copy_from_slice(&PARTITION_START.to_le_bytes());
    b[32..36].copy_from_slice(&part.to_le_bytes());
    b[36] = 0x80; // drive number
    b[38] = 0x29; // extended boot signature
    b[39..43].copy_from_slice(&0x1BAD_CAFEu32.to_le_bytes()); // serial
    b[43..54].copy_from_slice(LABEL);
    b[54..62].copy_from_slice(b"FAT16   ");
    b[510] = 0x55;
    b[511] = 0xAA;
    out.push((PARTITION_START, b));

    // Each FAT starts with the media byte and an end-of-chain entry.
    for copy in 0..FATS {
        let mut f = vec![0u8; SECTOR];
        f[0..4].copy_from_slice(&[MEDIA, 0xFF, 0xFF, 0xFF]);
        out.push((PARTITION_START + RESERVED + copy * fat, f));
    }

    // Root directory: the volume label.
    let mut root = vec![0u8; SECTOR];
    root[0..11].copy_from_slice(LABEL);
    root[11] = 0x08;
    out.push((PARTITION_START + RESERVED + FATS * fat, root));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_128_mib_card_gets_a_valid_fat16_volume() {
        let total = 0x4_0000;
        let s = format_fat16(total);
        let sec = |n: u32| s.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone()).unwrap();
        let mbr = sec(0);
        assert_eq!((mbr[0x1BE], mbr[0x1C2], &mbr[510..]), (0x80, 0x06, &[0x55, 0xAA][..]));
        let b = sec(PARTITION_START);
        assert_eq!(&b[510..], &[0x55, 0xAA][..]);
        let u16_at = |o: usize| u16::from_le_bytes([b[o], b[o + 1]]) as u32;
        let u32_at = |o: usize| u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
        let fat = u16_at(22);
        let data = RESERVED + FATS * fat + ROOT_ENTRIES * 32 / 512;
        let clusters = (u32_at(32) - data) / b[13] as u32;
        assert!((4085..65525).contains(&clusters), "FAT16 by cluster count: {clusters}");
        assert!(fat * 256 >= clusters + 2, "the FAT covers every cluster");
        assert_eq!(&b[43..54], b"Leapster SD");
        assert_eq!(sec(PARTITION_START + 1)[..4], [0xF0, 0xFF, 0xFF, 0xFF]);
        assert_eq!((fat, &b[39..43]), (256, &[0xFE, 0xCA, 0xAD, 0x1B][..]), "as the B5 BIOS formats it");
    }
}
