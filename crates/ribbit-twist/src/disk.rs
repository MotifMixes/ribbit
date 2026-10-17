//! The internal storage (eMMC) as the bootloader expects to find it,
//! built from the system packages. VERIFIED format (the bootloader's
//! readers, relocated to 0xF02ED4, 0xF03050 and 0xF032F8):
//!
//! * Sector 0: an MBR (FAT32 partition entry at 0x1BE, 0x55AA) that also
//!   holds "SDMM" at 0xC8 and, at 0xCC, the sector of the boot header (the
//!   base firmware writes it so, 0x2FF94; the bootloader follows it,
//!   0xF0300C). Without the MBR the base firmware's FAT driver hunts
//!   through sectors 0-254 for the volume and then resets the console.
//! * An "SDMM" header: its 128 words XOR to 0. Its word at 0x110 is the
//!   sector of the partition table, counted from the header's own sector;
//!   0x11C is the FAT volume's sector.
//! * An "MFGD" manufacturing-data sector in the 32 sectors before the
//!   partition table (searched downward from it: bootloader 0xF031C0,
//!   base firmware 0x2F748): 128 words XOR to 0; a 20-byte serial at 0x08,
//!   a word at 0x1C, 10 bytes at 0x24, flags at 0x30-0x32 (0x31 = 2: the
//!   factory test starts instead of the system). Meanings of the fields
//!   beyond that are UNVERIFIED; the values here are Ribbit's own.
//! * A "GPAP" partition table: 128 words XOR to 0x5A5A55AA, byte 5 is 1,
//!   the halfword at 0x0C is the number of entries (at most 7), and entry i
//!   (16 bytes at 0x18 + 16 i) holds a start sector (from the table's
//!   sector), a size, a type byte and a load address.
//! * Each partition starts with an "APDA" sector: word 0x0C is the XOR of
//!   the data's words and 0x10 its complement (the two add to 0xFFFFFFFF),
//!   0x1C the number of data sectors that follow.
//!
//! The bootloader normally loads partition 1 and jumps to its load
//! address; partition 0 is for a special key combination or a factory flag
//! (0xF050F8). The base firmware is linked at 0, so partition 1 holds its
//! code (the package image without its 0x18-byte header) loaded at 0.
//!
//! The real chip's layout (sector numbers) is UNKNOWN: no dump exists.
//! This one follows the format with its own choice of positions.

use crate::sd::SECTOR;

/// Where things go (sectors): MBR, header, manufacturing data, table,
/// partitions, then the FAT volume.
pub const MBR: usize = 0;
pub const HEADER: usize = 1;
pub const TABLE: usize = 0x40;
pub const MFGD: usize = TABLE - 1;
const FIRST_PARTITION: usize = TABLE + 8;
/// The FAT volume's first sector.
pub const FAT_AREA: usize = 0x8000;

fn put32(s: &mut [u8], at: usize, v: u32) {
    s[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

#[cfg(test)]
fn get32(s: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(s[at..at + 4].try_into().unwrap())
}

fn xor_words(d: &[u8]) -> u32 {
    d.chunks(4).fold(0, |x, w| {
        let mut b = [0u8; 4];
        b[..w.len()].copy_from_slice(w);
        x ^ u32::from_le_bytes(b)
    })
}

/// Sets the last word of a sector so that its 128 words XOR to `target`.
fn seal(sector: &mut [u8], target: u32) {
    put32(sector, SECTOR - 4, 0);
    let x = xor_words(sector);
    put32(sector, SECTOR - 4, x ^ target);
}

/// One partition: its code and where it is loaded.
pub struct Partition<'a> {
    pub data: &'a [u8],
    pub load: u32,
}

/// The eMMC image: header, table, `partitions` (0 = factory/recovery, 1 =
/// the base firmware), then the FAT `volume` at `FAT_AREA`.
pub fn build(partitions: &[Partition], volume: &[u8]) -> Vec<u8> {
    let mut disk = vec![0u8; FAT_AREA * SECTOR + volume.len()];
    disk[FAT_AREA * SECTOR..].copy_from_slice(volume);
    let mut table = vec![0u8; SECTOR];
    table[..4].copy_from_slice(b"GPAP");
    table[5] = 1;
    table[0x0C..0x0E].copy_from_slice(&(partitions.len() as u16).to_le_bytes());
    let mut at = FIRST_PARTITION;
    for (i, p) in partitions.iter().enumerate() {
        let sectors = p.data.len().div_ceil(SECTOR);
        let mut data = p.data.to_vec();
        data.resize(sectors * SECTOR, 0);
        let sum = xor_words(&data);
        let mut apda = vec![0u8; SECTOR];
        apda[..4].copy_from_slice(b"APDA");
        put32(&mut apda, 0x0C, sum);
        put32(&mut apda, 0x10, !sum);
        put32(&mut apda, 0x1C, sectors as u32);
        disk[at * SECTOR..(at + 1) * SECTOR].copy_from_slice(&apda);
        disk[(at + 1) * SECTOR..(at + 1 + sectors) * SECTOR].copy_from_slice(&data);
        let e = 0x18 + 16 * i;
        put32(&mut table, e, (at - TABLE) as u32);
        put32(&mut table, e + 4, (sectors + 1) as u32);
        put32(&mut table, e + 12, p.load);
        at += sectors + 1;
    }
    assert!(at <= FAT_AREA, "the partitions overlap the FAT volume");
    seal(&mut table, 0x5A5A_55AA);
    disk[TABLE * SECTOR..(TABLE + 1) * SECTOR].copy_from_slice(&table);
    let mut header = vec![0u8; SECTOR];
    header[..4].copy_from_slice(b"SDMM");
    header[5] = 1;
    put32(&mut header, 0x110, (TABLE - HEADER) as u32);
    put32(&mut header, 0x11C, (FAT_AREA - HEADER) as u32);
    seal(&mut header, 0);
    disk[HEADER * SECTOR..(HEADER + 1) * SECTOR].copy_from_slice(&header);
    let mut mfgd = vec![0u8; SECTOR];
    mfgd[..4].copy_from_slice(b"MFGD");
    mfgd[8..8 + 20].copy_from_slice(b"RIBBIT00000000000001");
    put32(&mut mfgd, 0x1C, 0);
    mfgd[0x24..0x24 + 6].copy_from_slice(b"606000");
    mfgd[0x30] = 1; // UNVERIFIED: serial verified
    // 2 starts the factory test at every power-on. With 1, the bootloader
    // starts it only for a key combination held at power-on (Switch right,
    // D-pad right, Vol+ and the check mark: 0xF050F8, keys 0x100212); with
    // anything else, never. What a retail unit holds is UNKNOWN; 1 keeps
    // the combination working.
    mfgd[0x31] = 1;
    mfgd[0x32] = 1; // UNVERIFIED: I/O test passed
    seal(&mut mfgd, 0);
    disk[MFGD * SECTOR..(MFGD + 1) * SECTOR].copy_from_slice(&mfgd);
    let sectors = (volume.len() / SECTOR) as u32;
    let total =
        u32::from_le_bytes(volume.get(0x20..0x24).and_then(|b| b.try_into().ok()).unwrap_or([0; 4])).max(sectors);
    let mbr = &mut disk[MBR * SECTOR..(MBR + 1) * SECTOR];
    mbr[0xC8..0xCC].copy_from_slice(b"SDMM");
    put32(mbr, 0xCC, HEADER as u32);
    let e = 0x1BE;
    mbr[e] = 0x80;
    mbr[e + 4] = 0x0B; // FAT32
    put32(mbr, e + 8, FAT_AREA as u32);
    put32(mbr, e + 12, total);
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
    disk
}

/// Sets the manufacturing data's factory-test flag (0x31 = 2: the
/// bootloader starts partition 0, the factory test, instead of the System;
/// 1: only for the key combination, see `build`) and re-seals the sector.
pub fn set_factory_test(disk: &mut [u8], on: bool) {
    let s = &mut disk[MFGD * SECTOR..(MFGD + 1) * SECTOR];
    s[0x31] = if on { 2 } else { 1 };
    seal(s, 0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_layout_passes_the_bootloaders_checks() {
        let code0: Vec<u8> = (0..1000u32).flat_map(|i| (i * 7).to_le_bytes()).collect();
        let code1: Vec<u8> = (0..3000u32).flat_map(|i| (i ^ 0x5A).to_le_bytes()).collect();
        let volume = vec![0xEEu8; 4 * SECTOR];
        let d = build(&[Partition { data: &code0, load: 0x10_0000 }, Partition { data: &code1, load: 0 }], &volume);
        let sector = |n: usize| &d[n * SECTOR..(n + 1) * SECTOR];
        let m = sector(MBR);
        assert_eq!((&m[0xC8..0xCC], get32(m, 0xCC) as usize), (&b"SDMM"[..], HEADER), "points at the header");
        assert_eq!((m[0x1C2], get32(m, 0x1C6) as usize, &m[510..]), (0x0B, FAT_AREA, &[0x55, 0xAA][..]));
        let f = sector(MFGD);
        assert_eq!((&f[..4], xor_words(f), f[0x31]), (&b"MFGD"[..], 0, 1));
        let h = sector(HEADER);
        assert_eq!(&h[..4], b"SDMM");
        assert_eq!(xor_words(h), 0, "header checksum");
        let table_at = HEADER + get32(h, 0x110) as usize;
        let t = sector(table_at);
        assert_eq!((&t[..4], t[5]), (&b"GPAP"[..], 1));
        assert_eq!(xor_words(t), 0x5A5A_55AA, "table checksum");
        assert_eq!(u16::from_le_bytes([t[0x0C], t[0x0D]]), 2);
        for (i, code) in [&code0, &code1].into_iter().enumerate() {
            let e = 0x18 + 16 * i;
            let start = table_at + get32(t, e) as usize;
            let a = sector(start);
            assert_eq!(&a[..4], b"APDA");
            assert_eq!(get32(a, 0x0C).wrapping_add(get32(a, 0x10)), 0xFFFF_FFFF);
            let n = get32(a, 0x1C) as usize;
            let data = &d[(start + 1) * SECTOR..(start + 1 + n) * SECTOR];
            assert_eq!(xor_words(data), get32(a, 0x0C), "partition {i} checksum");
            assert_eq!(&data[..code.len()], &code[..]);
        }
        assert_eq!(get32(t, 0x18 + 16 + 12), 0, "the base firmware loads at 0");
        assert_eq!(&d[FAT_AREA * SECTOR..], &volume[..]);
    }
}
