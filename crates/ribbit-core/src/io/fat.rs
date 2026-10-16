//! A freshly formatted SD card: one FAT32 volume, as the Leapster 2's own
//! firmware formats it.
//!
//! Why: the Leapster 2 BIOS 0.6 never formats its SD volume. Its FAT start-up
//! (0x400D_0680 onwards) formats only volumes whose descriptor marks them
//! volatile (the RAM disk, "Leapster RD"); for the SD volume it prints "Fat
//! Volume %s (%s) not inserted" or uses it as found. On a blank card its base
//! system then fails ("Failed trying to mkdir 'b:\Leapster'"), so real units
//! ship with a formatted card.
//!
//! The layout copies what the Connected BIOS B5 writes to a blank 1 GiB card
//! (VERIFIED-FW, captured with `scenes sdsector`; its formatter is
//! 0x400D_243C, called from 0x400C_E920 with the SD volume's parameters: 2
//! FATs, 0 root entries, serial 0x1BAD_CAFE, label "Leapster SD"). B5 picks
//! FAT12 below 8 MiB, FAT16 below 512 MiB and FAT32 above (0x400D_295C); its
//! "0 root entries" is valid only for FAT32, which is why the card is taken to
//! be larger than 512 MiB (sd.rs). No partition table: the volume starts at
//! sector 0, which both BIOSes mount ("There is no partition table, treat it
//! as boot sector"). The factory card's contents are UNKNOWN.

const SECTOR: usize = 512;
const SECTORS_PER_CLUSTER: u32 = 8;
const RESERVED: u32 = 32;
const FATS: u32 = 2;
const FSINFO: u32 = 1;
const BACKUP_BOOT: u32 = 6;
const ROOT_CLUSTER: u32 = 2;
const LABEL: &[u8; 11] = b"Leapster SD";
const MEDIA: u8 = 0xF0;

/// FAT size in sectors, by the usual FAT32 formula (B5 gets 2046 for 1 GiB).
fn fat_sectors(total: u32) -> u32 {
    let per = (256 * SECTORS_PER_CLUSTER + FATS) / 2;
    (total - RESERVED).div_ceil(per)
}

fn signed() -> Vec<u8> {
    let mut s = vec![0u8; SECTOR];
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

/// The non-zero sectors of a card of `total` sectors, freshly formatted.
pub fn format_fat32(total: u32) -> Vec<(u32, Vec<u8>)> {
    let fat = fat_sectors(total);
    let mut b = signed();
    b[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"MSDOS5.0");
    b[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    b[13] = SECTORS_PER_CLUSTER as u8;
    b[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    b[16] = FATS as u8;
    b[21] = MEDIA;
    b[24..26].copy_from_slice(&63u16.to_le_bytes()); // sectors per track
    b[26..28].copy_from_slice(&255u16.to_le_bytes()); // heads
    b[32..36].copy_from_slice(&total.to_le_bytes());
    b[36..40].copy_from_slice(&fat.to_le_bytes());
    b[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    b[48..50].copy_from_slice(&(FSINFO as u16).to_le_bytes());
    b[50..52].copy_from_slice(&(BACKUP_BOOT as u16).to_le_bytes());
    b[64] = 0x80; // drive number
    b[66] = 0x29; // extended boot signature
    b[67..71].copy_from_slice(&0x1BAD_CAFEu32.to_le_bytes()); // serial
    b[71..82].copy_from_slice(LABEL);
    b[82..90].copy_from_slice(b"FAT32   ");

    // FSInfo: free count unknown (B5 writes 0xFFFFFFFF), next free cluster 3.
    let mut info = signed();
    info[0..4].copy_from_slice(b"RRaA");
    info[484..488].copy_from_slice(b"rrAa");
    info[488..492].copy_from_slice(&u32::MAX.to_le_bytes());
    info[492..496].copy_from_slice(&3u32.to_le_bytes());

    let mut out = Vec::new();
    for base in [0, BACKUP_BOOT] {
        out.push((base, b.clone()));
        out.push((base + FSINFO, info.clone()));
        out.push((base + 2, signed()));
    }
    // Each FAT: media entry, end-of-chain entry, and the root directory's
    // cluster ended (B5 writes 0xFFFFFFFF; its allocator later 0x0FFFFFFF).
    for copy in 0..FATS {
        let mut f = vec![0u8; SECTOR];
        f[0..12].copy_from_slice(&[MEDIA, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF]);
        out.push((RESERVED + copy * fat, f));
    }
    // The root directory's cluster starts empty (no volume label entry).
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_1_gib_card_gets_the_volume_b5_formats() {
        let total = 0x20_0000;
        let s = format_fat32(total);
        let sec = |n: u32| s.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone()).unwrap();
        let b = sec(0);
        // B5's boot sector, captured from a blank 1 GiB card (first 0x5A bytes).
        let b5: [u8; 0x5A] = [
            0xeb, 0x58, 0x90, 0x4d, 0x53, 0x44, 0x4f, 0x53, 0x35, 0x2e, 0x30, 0x00, 0x02, 0x08, 0x20, 0x00, //
            0x02, 0x00, 0x00, 0x00, 0x00, 0xf0, 0x00, 0x00, 0x3f, 0x00, 0xff, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x00, 0x00, 0x20, 0x00, 0xfe, 0x07, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, //
            0x01, 0x00, 0x06, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, //
            0x80, 0x00, 0x29, 0xfe, 0xca, 0xad, 0x1b, 0x4c, 0x65, 0x61, 0x70, 0x73, 0x74, 0x65, 0x72, 0x20, //
            0x53, 0x44, 0x46, 0x41, 0x54, 0x33, 0x32, 0x20, 0x20, 0x20,
        ];
        assert_eq!(&b[..0x5A], &b5[..]);
        assert_eq!(&b[0x5A..510], &[0u8; 510 - 0x5A][..]);
        assert_eq!(&b[510..], &[0x55, 0xAA][..]);
        assert_eq!(sec(6), b, "backup boot sector");
        assert_eq!(&sec(1)[484..496], &[0x72, 0x72, 0x41, 0x61, 0xff, 0xff, 0xff, 0xff, 3, 0, 0, 0][..]);
        let fat = 2046;
        assert_eq!(sec(RESERVED)[..12], sec(RESERVED + fat)[..12]);
        assert_eq!(sec(RESERVED)[..4], [0xF0, 0xFF, 0xFF, 0xFF]);
        let clusters = (total - RESERVED - FATS * fat) / SECTORS_PER_CLUSTER;
        assert!(clusters >= 65525 && fat * 128 >= clusters + 2, "FAT32 by cluster count, FAT covers them");
    }
}
