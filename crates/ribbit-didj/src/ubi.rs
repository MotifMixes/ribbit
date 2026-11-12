//! UBI, as the Didj's kernel lays it out on a NAND partition: one user
//! volume (id 0, dynamic), read by the kernel's gluebi as a block device.
//!
//! Sources: the kernel's drivers/mtd/ubi and include/mtd/ubi-header.h
//! (linux-2.6.20-lf1000, a UBI backport, on-flash version 1) and
//! /etc/init.d/ubi in erootfs.jffs2. VERIFIED against what that kernel
//! writes when it formats an empty Brio partition in Ribbit
//! (`DIDJ_NAND=<file> didjboot`): VID header at byte 512 of each erase
//! block, data from byte 2048, the layout volume in 2 blocks, the user
//! volume reserving every available block (`ubimkvol --maxavsize`).

use crate::nand::{self, Nand, Partition};

/// Where the VID header and the data sit in an erase block (the kernel
/// writes 512-byte subpages, so the VID header shares page 0 with the
/// EC header).
pub const VID_OFFSET: usize = 512;
pub const DATA_OFFSET: usize = nand::PAGE;
/// The data an erase block holds (a logical erase block, LEB).
pub const LEB: usize = nand::BLOCK - DATA_OFFSET;

const EC_MAGIC: u32 = 0x5542_4923; // "UBI#"
const VID_MAGIC: u32 = 0x5542_4921; // "UBI!"
const VERSION: u8 = 1;
const DYNAMIC: u8 = 1;
/// UBI_COMPAT_REJECT: older UBIs must not attach an image with this volume.
const COMPAT_REJECT: u8 = 5;
const LAYOUT_VOL_ID: u32 = 0x7FFF_FFFF - 4096;
const LAYOUT_BLOCKS: usize = 2;
const MAX_VOLUMES: usize = 128;
const VTBL_RECORD: usize = 172;
/// Blocks UBI keeps back: for wear-levelling (WL_RESERVED_PEBS) and for
/// bad-block handling (1% of the good blocks, at least 2: misc.c).
const WL_RESERVED: usize = 1;
/// The blocks UBI's EBA keeps for atomic LEB changes (EBA_RESERVED_PEBS):
/// not given to a volume the kernel auto-resizes.
const EBA_RESERVED: usize = 1;
/// UBI_VTBL_AUTORESIZE_FLG, in a volume table record's flags (byte 144).
const AUTORESIZE: u8 = 1;

/// UBI's CRC-32: the usual polynomial, starting at 0xFFFFFFFF, without the
/// final inversion.
fn crc(data: &[u8]) -> u32 {
    !crc32fast::hash(data)
}

/// The blocks the user volume can reserve on a partition of `blocks`
/// good blocks (1756 on Brio, as the kernel reports).
pub fn available_blocks(blocks: usize) -> usize {
    blocks - LAYOUT_BLOCKS - WL_RESERVED - (blocks / 100).max(2)
}

/// An erase counter header. The kernel's own format leaves counts of 1
/// and 2; the image uses 1.
fn ec_header(ec: u64) -> [u8; 64] {
    ec_header_at(ec, VID_OFFSET, DATA_OFFSET)
}

/// An erase counter header for a VID header and data at `vid` and `data`.
fn ec_header_at(ec: u64, vid: usize, data: usize) -> [u8; 64] {
    let mut h = [0u8; 64];
    h[0..4].copy_from_slice(&EC_MAGIC.to_be_bytes());
    h[4] = VERSION;
    h[8..16].copy_from_slice(&ec.to_be_bytes());
    h[16..20].copy_from_slice(&(vid as u32).to_be_bytes());
    h[20..24].copy_from_slice(&(data as u32).to_be_bytes());
    let c = crc(&h[..60]);
    h[60..64].copy_from_slice(&c.to_be_bytes());
    h
}

/// A volume identifier header for a dynamic volume's LEB (data size and
/// CRC are only kept for static volumes).
fn vid_header(vol_id: u32, compat: u8, lnum: u32, sqnum: u64) -> [u8; 64] {
    let mut h = [0u8; 64];
    h[0..4].copy_from_slice(&VID_MAGIC.to_be_bytes());
    h[4] = VERSION;
    h[5] = DYNAMIC;
    h[7] = compat;
    h[8..12].copy_from_slice(&vol_id.to_be_bytes());
    h[12..16].copy_from_slice(&lnum.to_be_bytes());
    h[40..48].copy_from_slice(&sqnum.to_be_bytes());
    let c = crc(&h[..60]);
    h[60..64].copy_from_slice(&c.to_be_bytes());
    h
}

/// The volume table: record 0 is the volume, the others are empty (zeroes
/// with their CRC). As many records as fit a LEB, at most 128 (vtbl.c
/// `vtbl_slots`: 89 on a cartridge).
fn volume_table(name: &str, reserved: usize, leb: usize) -> Vec<u8> {
    let mut t = vec![0u8; MAX_VOLUMES.min(leb / VTBL_RECORD) * VTBL_RECORD];
    for (i, r) in t.chunks_mut(VTBL_RECORD).enumerate() {
        if i == 0 {
            r[0..4].copy_from_slice(&(reserved as u32).to_be_bytes());
            r[4..8].copy_from_slice(&1u32.to_be_bytes()); // alignment
            r[12] = DYNAMIC;
            r[14..16].copy_from_slice(&(name.len() as u16).to_be_bytes());
            r[16..16 + name.len()].copy_from_slice(name.as_bytes());
        }
        let c = crc(&r[..VTBL_RECORD - 4]);
        r[VTBL_RECORD - 4..].copy_from_slice(&c.to_be_bytes());
    }
    t
}

/// Writes UBI onto `part`: every block erased with an EC header, the
/// layout volume in the first two blocks, and the volume `name` holding
/// `data` from the third block on. LEBs that would be all 0xFF stay
/// unmapped (they read back as 0xFF).
pub fn write_volume(flash: &mut Nand, part: Partition, name: &str, data: &[u8]) -> Result<(), String> {
    let first = part.offset / nand::BLOCK;
    let blocks = part.size / nand::BLOCK;
    let reserved = available_blocks(blocks);
    if data.len() > reserved * LEB {
        return Err(format!("{} bytes don't fit the {} volume ({} bytes)", data.len(), part.name, reserved * LEB));
    }
    let ec = ec_header(1);
    let mut page0 = [0xFFu8; nand::PAGE];
    page0[..64].copy_from_slice(&ec);
    for b in first..first + blocks {
        flash.erase(b);
        flash.write(b * nand::BLOCK, &page0);
    }
    let mut sqnum = 0;
    let mut put = |flash: &mut Nand, block: usize, vid: [u8; 64], contents: &[u8]| {
        page0[VID_OFFSET..VID_OFFSET + 64].copy_from_slice(&vid);
        let at = (first + block) * nand::BLOCK;
        flash.write(at, &page0);
        flash.write(at + DATA_OFFSET, contents);
    };
    let table = volume_table(name, reserved, LEB);
    for copy in 0..LAYOUT_BLOCKS {
        put(flash, copy, vid_header(LAYOUT_VOL_ID, COMPAT_REJECT, copy as u32, sqnum), &table);
        sqnum += 1;
    }
    for (lnum, leb) in data.chunks(LEB).enumerate() {
        if leb.iter().all(|&b| b == 0xFF) {
            continue;
        }
        put(flash, LAYOUT_BLOCKS + lnum, vid_header(0, 0, lnum as u32, sqnum), leb);
        sqnum += 1;
    }
    Ok(())
}

/// UBI as a cartridge holds it (VERIFIED against a retail Star Wars: The
/// Clone Wars dump): `blocks` erase blocks of `peb` bytes, the VID header
/// at 512 and the data from 1024, the layout volume in the first two and
/// the volume "ubi_Cartridge" reserving every available block (4053 of
/// 4096). Returns the blocks' data areas, unmapped LEBs left erased.
pub fn cart_image(peb: usize, blocks: usize, data: &[u8]) -> Result<Vec<u8>, String> {
    const VID: usize = 512;
    const DATA: usize = 1024;
    let leb = peb - DATA;
    let reserved = available_blocks(blocks);
    if data.len() > reserved * leb {
        return Err(format!("{} bytes don't fit a cartridge volume ({} bytes)", data.len(), reserved * leb));
    }
    let mut out = vec![0xFFu8; peb * blocks];
    let ec = ec_header_at(1, VID, DATA);
    for b in out.chunks_mut(peb) {
        b[..64].copy_from_slice(&ec);
    }
    let mut put = |block: usize, vid: [u8; 64], contents: &[u8]| {
        let b = &mut out[block * peb..][..peb];
        b[VID..VID + 64].copy_from_slice(&vid);
        b[DATA..DATA + contents.len()].copy_from_slice(contents);
    };
    // The rest of the LEB zeroed, as on the retail cartridge.
    let mut table = volume_table("ubi_Cartridge", reserved, leb);
    table.resize(leb, 0);
    let mut sqnum = 0;
    for copy in 0..LAYOUT_BLOCKS {
        put(copy, vid_header(LAYOUT_VOL_ID, COMPAT_REJECT, copy as u32, sqnum), &table);
        sqnum += 1;
    }
    for (lnum, l) in data.chunks(leb).enumerate() {
        if l.iter().all(|&b| b == 0xFF) {
            continue;
        }
        put(LAYOUT_BLOCKS + lnum, vid_header(0, 0, lnum as u32, sqnum), l);
        sqnum += 1;
    }
    Ok(out)
}

/// The erase block holding each of the user volume's first `count` LEBs
/// (None: unmapped): of two copies, the one with the higher sequence
/// number (UBI writes a changed LEB to another block).
pub fn lebs(flash: &Nand, part: Partition, count: usize) -> Vec<Option<usize>> {
    lebs_of(flash, part, 0, count)
}

fn lebs_of(flash: &Nand, part: Partition, vol_id: u32, count: usize) -> Vec<Option<usize>> {
    let first = part.offset / nand::BLOCK;
    let mut lebs: Vec<Option<(usize, u64)>> = vec![None; count];
    for b in first..first + part.size / nand::BLOCK {
        let vid = flash.read(b * nand::BLOCK + VID_OFFSET, 64);
        let word = |at: usize| u32::from_be_bytes(vid[at..at + 4].try_into().unwrap());
        if word(0) == VID_MAGIC && word(8) == vol_id && crc(&vid[..60]) == word(60) {
            let sqnum = u64::from_be_bytes(vid[40..48].try_into().unwrap());
            if let Some(slot) = lebs.get_mut(word(12) as usize)
                && slot.is_none_or(|(_, s)| sqnum >= s)
            {
                *slot = Some((b, sqnum));
            }
        }
    }
    lebs.into_iter().map(|l| l.map(|(b, _)| b)).collect()
}

/// Writes LEB `lnum` of the user volume anew, as the kernel's UBI does
/// when it changes one: `data` into a free erase block (an EC header and
/// nothing else) with a sequence number above every other, then the
/// block that held it erased (EC header with its count one up).
pub fn write_leb(flash: &mut Nand, part: Partition, lnum: usize, data: &[u8]) -> Result<(), String> {
    write_leb_of(flash, part, 0, 0, lnum, data)
}

/// `write_leb` for volume `vol_id` (its VID headers' compat byte `compat`).
fn write_leb_of(flash: &mut Nand, part: Partition, vol_id: u32, compat: u8, lnum: usize, data: &[u8]) -> Result<(), String> {
    if data.len() > LEB {
        return Err(format!("{} bytes don't fit a LEB", data.len()));
    }
    let first = part.offset / nand::BLOCK;
    let word = |h: &[u8], at: usize| u32::from_be_bytes(h[at..at + 4].try_into().unwrap());
    let ec_of = |flash: &Nand, b: usize| {
        let ec = flash.read(b * nand::BLOCK, 64);
        (word(&ec, 0) == EC_MAGIC && crc(&ec[..60]) == word(&ec, 60)).then(|| u64::from_be_bytes(ec[8..16].try_into().unwrap()))
    };
    let (mut top, mut old, mut free, mut erased) = (0u64, None, None, None);
    for b in first..first + part.size / nand::BLOCK {
        let vid = flash.read(b * nand::BLOCK + VID_OFFSET, 64);
        if word(&vid, 0) == VID_MAGIC && crc(&vid[..60]) == word(&vid, 60) {
            let sqnum = u64::from_be_bytes(vid[40..48].try_into().unwrap());
            top = top.max(sqnum);
            if word(&vid, 8) == vol_id && word(&vid, 12) as usize == lnum && old.is_none_or(|(_, s)| sqnum >= s) {
                old = Some((b, sqnum));
            }
        } else if free.is_none()
            && vid.iter().all(|&x| x == 0xFF)
            && let Some(ec) = ec_of(flash, b)
            && flash.read(b * nand::BLOCK + DATA_OFFSET, LEB).iter().all(|&x| x == 0xFF)
        {
            free = Some((b, ec));
        } else if erased.is_none() && !flash.is_written(b) {
            erased = Some(b);
        }
    }
    // A block never formatted (all erased, as past the end of LeapFrog's
    // root file system image) is one UBI's attach would format.
    let (b, ec) = free.or(erased.map(|b| (b, 1))).ok_or_else(|| format!("no free erase block on {}", part.name))?;
    let mut page0 = [0xFFu8; nand::PAGE];
    page0[..64].copy_from_slice(&ec_header(ec));
    page0[VID_OFFSET..VID_OFFSET + 64].copy_from_slice(&vid_header(vol_id, compat, lnum as u32, top + 1));
    flash.write(b * nand::BLOCK, &page0);
    flash.write(b * nand::BLOCK + DATA_OFFSET, data);
    if let Some((o, _)) = old {
        let ec = ec_of(flash, o).unwrap_or(0) + 1;
        flash.erase(o);
        let mut page0 = [0xFFu8; nand::PAGE];
        page0[..64].copy_from_slice(&ec_header(ec));
        flash.write(o * nand::BLOCK, &page0);
    }
    Ok(())
}

/// The user volume's size in LEBs as UBI attaches it: the volume table's
/// reserved count, or (the auto-resize flag set, as in LeapFrog's root file
/// system image) every block it can have, as the kernel's first attach
/// makes it (vtbl.c, `autoresize`: 678 on the Explorer's RFS, read back
/// from a save). Also whether it is still to be resized.
pub fn volume_size(flash: &Nand, part: Partition) -> Option<(usize, bool)> {
    let record = layout_record(flash, part)?;
    let reserved = u32::from_be_bytes(record[..4].try_into().unwrap()) as usize;
    match record[144] & AUTORESIZE != 0 {
        true => Some((available_blocks(part.size / nand::BLOCK) - EBA_RESERVED, true)),
        false => Some((reserved, false)),
    }
}

/// Volume table record 0 (the user volume) from the layout volume's newer
/// copy whose CRC checks out.
fn layout_record(flash: &Nand, part: Partition) -> Option<Vec<u8>> {
    let first = part.offset / nand::BLOCK;
    let mut best: Option<(u64, Vec<u8>)> = None;
    for b in first..first + part.size / nand::BLOCK {
        let vid = flash.read(b * nand::BLOCK + VID_OFFSET, 64);
        let word = |at: usize| u32::from_be_bytes(vid[at..at + 4].try_into().unwrap());
        if word(0) == VID_MAGIC && word(8) == LAYOUT_VOL_ID && crc(&vid[..60]) == word(60) {
            let sqnum = u64::from_be_bytes(vid[40..48].try_into().unwrap());
            let r = flash.read(b * nand::BLOCK + DATA_OFFSET, VTBL_RECORD);
            let ok = crc(&r[..VTBL_RECORD - 4]) == u32::from_be_bytes(r[VTBL_RECORD - 4..].try_into().unwrap());
            if ok && best.as_ref().is_none_or(|(s, _)| sqnum > *s) {
                best = Some((sqnum, r));
            }
        }
    }
    best.map(|b| b.1)
}

/// Resizes the user volume as the kernel's first attach does when the
/// auto-resize flag is set (`volume_size`): both copies of the volume table
/// written anew with the new size and the flag cleared. Nothing when it
/// isn't set.
pub fn autoresize(flash: &mut Nand, part: Partition) -> Result<(), String> {
    let Some((size, true)) = volume_size(flash, part) else { return Ok(()) };
    let lebs = lebs_of(flash, part, LAYOUT_VOL_ID, LAYOUT_BLOCKS);
    let Some(Some(b)) = lebs.first() else { return Err("no volume table".into()) };
    let mut table = flash.read(b * nand::BLOCK + DATA_OFFSET, LEB);
    let r = &mut table[..VTBL_RECORD];
    r[..4].copy_from_slice(&(size as u32).to_be_bytes());
    r[144] &= !AUTORESIZE;
    let c = crc(&r[..VTBL_RECORD - 4]);
    r[VTBL_RECORD - 4..].copy_from_slice(&c.to_be_bytes());
    while table.last() == Some(&0xFF) {
        table.pop();
    }
    for copy in 0..LAYOUT_BLOCKS {
        write_leb_of(flash, part, LAYOUT_VOL_ID, COMPAT_REJECT, copy, &table)?;
    }
    Ok(())
}

/// The volume's contents read back (unmapped LEBs as 0xFF), `len` bytes:
/// what gluebi presents as the block device.
pub fn read_volume(flash: &Nand, part: Partition, len: usize) -> Vec<u8> {
    let lebs = lebs(flash, part, len.div_ceil(LEB));
    let mut out = Vec::with_capacity(len);
    for leb in lebs {
        let n = (len - out.len()).min(LEB);
        match leb {
            Some(b) => out.extend(flash.read(b * nand::BLOCK + DATA_OFFSET, n)),
            None => out.resize(out.len() + n, 0xFF),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brio_reserves_what_the_kernel_reports() {
        let brio = nand::partition("Brio").unwrap();
        assert_eq!(available_blocks(brio.size / nand::BLOCK), 1756);
        assert_eq!(LEB, 129024);
    }

    #[test]
    fn headers_match_the_kernels_format() {
        // Bytes the kernel wrote (Ribbit boot, Brio formatted by rcS).
        let ec = ec_header(2);
        assert_eq!(ec[..24], [0x55, 0x42, 0x49, 0x23, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 2, 0, 0, 0, 8, 0]);
        assert_eq!(ec[60..], [0xd2, 0xd9, 0x1f, 0x5d]);
        let vid = vid_header(LAYOUT_VOL_ID, COMPAT_REJECT, 0, 3);
        assert_eq!(vid[..16], [0x55, 0x42, 0x49, 0x21, 1, 1, 0, 5, 0x7f, 0xff, 0xef, 0xff, 0, 0, 0, 0]);
        assert_eq!(vid[60..], [0x05, 0xef, 0x08, 0x66]);
        // A retail cartridge's (Star Wars: The Clone Wars, block 0).
        assert_eq!(ec_header_at(2, 512, 1024)[60..], [0x3e, 0x08, 0x18, 0x80]);
        let t = volume_table("ubi_Brio", 1756, LEB);
        assert_eq!(t[168..172], [0x25, 0xbd, 0x3b, 0xd8]);
        assert_eq!(t[172 + 168..172 * 2], [0xf1, 0x16, 0xc3, 0x6b]);
    }

    #[test]
    fn a_volume_reads_back() {
        let brio = nand::partition("Brio").unwrap();
        let mut flash = Nand::erased();
        let mut data = vec![0xFFu8; LEB * 3 + 100];
        data[5] = 1;
        data[LEB * 2..LEB * 2 + 4].copy_from_slice(&[0, 0, 0, 0]);
        data[LEB * 3 + 99] = 7;
        write_volume(&mut flash, brio, "ubi_Brio", &data).unwrap();
        assert_eq!(read_volume(&flash, brio, data.len()), data);
        // LEB 1 is all 0xFF: its block holds only an EC header.
        let b3 = brio.offset + 3 * nand::BLOCK;
        assert_eq!(flash.read(b3 + VID_OFFSET, 4), [0xFF; 4]);
        assert_eq!(flash.read(b3, 4), [0x55, 0x42, 0x49, 0x23]);
        assert!(write_volume(&mut flash, brio, "ubi_Brio", &vec![0; 1757 * LEB]).is_err());
    }

    /// A LEB written anew reads back changed, in another block, and the
    /// rest of the volume stays as it was.
    #[test]
    fn a_leb_is_written_anew() {
        let brio = nand::partition("Brio").unwrap();
        let mut flash = Nand::erased();
        let mut data = vec![3u8; LEB * 2];
        write_volume(&mut flash, brio, "ubi_Brio", &data).unwrap();
        let was = lebs(&flash, brio, 2);
        write_leb(&mut flash, brio, 1, &vec![9u8; LEB]).unwrap();
        data[LEB..].fill(9);
        assert_eq!(read_volume(&flash, brio, data.len()), data);
        let now = lebs(&flash, brio, 2);
        assert_eq!(now[0], was[0]);
        assert_ne!(now[1], was[1]);
        // The old block is free again: an EC header, its count one up.
        let old = was[1].unwrap() * nand::BLOCK;
        assert_eq!(flash.read(old + VID_OFFSET, 4), [0xFF; 4]);
        assert_eq!(flash.read(old + 15, 1), [2]);
    }
}
