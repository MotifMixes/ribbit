//! UBIFS images: a file tree made into the contents of a UBI volume, as
//! mkfs.ubifs makes them, for the Leapster Explorer's Bulk partition (the
//! packages LeapFrog Connect would install).
//!
//! The layout follows the kernel's own `create_default_filesystem`
//! (fs/ubifs/sb.c) and `ubifs_create_dflt_lpt` (lpt.c), with the file
//! nodes and a full index added: the superblock in LEB 0, the master node
//! in LEBs 1 and 2, the log (a commit start node), the LEB properties tree
//! (the "small" model: one LEB), the orphan area, then the main area: the
//! inode, directory entry and data nodes, the index nodes in LEBs of their
//! own, and empty LEBs (the first one for garbage collection). Data is
//! stored uncompressed (the kernel's zlib only takes a 2 KiB window).
//! Settings match LeapFrog's own root file system (erootfs.ubi): 2 KiB
//! minimum I/O, fanout 8, 5 log LEBs, 1 orphan LEB, format 4.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use crate::ubi::LEB;

const MIN_IO: usize = 2048;
const FANOUT: usize = 8;
const LOG_LEBS: usize = 5;
const ORPH_LEBS: usize = 1;
const LOG_LNUM: usize = 3;
const BLOCK_SIZE: usize = 4096;
const ROOT_INO: u32 = 1;
const FIRST_INO: u32 = 64;

const MAGIC: u32 = 0x0610_1831;
const CH_SZ: usize = 24;
const INO_NODE_SZ: usize = 160;
const DENT_NODE_SZ: usize = 56;
const DATA_NODE_SZ: usize = 48;
const IDX_NODE_SZ: usize = 28;
const BRANCH_SZ: usize = 12 + KEY_LEN;
const PAD_NODE_SZ: usize = 28;
const SB_NODE_SZ: usize = 4096;
const MST_NODE_SZ: usize = 512;
const CS_NODE_SZ: usize = 32;
/// Keys are 8 bytes ("simple" format) in index branches, 16 in nodes.
const KEY_LEN: usize = 8;

// Node types (ubifs-media.h).
const INO_NODE: u8 = 0;
const DATA_NODE: u8 = 1;
const DENT_NODE: u8 = 2;
const PAD_NODE: u8 = 5;
const SB_NODE: u8 = 6;
const MST_NODE: u8 = 7;
const IDX_NODE: u8 = 9;
const CS_NODE: u8 = 10;

// Key types.
const INO_KEY: u32 = 0;
const DATA_KEY: u32 = 1;
const DENT_KEY: u32 = 2;

// Inode types in directory entries.
const ITYPE_REG: u8 = 0;
const ITYPE_DIR: u8 = 1;
const ITYPE_LNK: u8 = 2;

const COMPR_NONE: u16 = 0;
const COMPR_LZO: u16 = 1;
/// Inode flag: compress this inode's data when writing it.
const COMPR_FL: u32 = 1;
/// Master flag: the orphan area is empty.
const MST_NO_ORPHS: u32 = 2;
const LPROPS_INDEX: u32 = 32;

// LEB properties tree (lpt.c).
const LPT_FANOUT: usize = 4;
const LPT_CRC_BITS: usize = 16;
const LPT_TYPE_BITS: usize = 4;
const LPT_PNODE: u32 = 0;
const LPT_NNODE: u32 = 1;
const LPT_LTAB: u32 = 2;
const LSAVE_CNT: u32 = 256;

/// The kernel's space watermarks (super.c): a LEB with less free+dirty
/// space than `DEAD_WM` is dead, up to `DARK_WM` dark.
const DEAD_WM: usize = (DATA_NODE_SZ + 8).next_multiple_of(MIN_IO);
const DARK_WM: usize = (INO_NODE_SZ + BLOCK_SIZE).next_multiple_of(MIN_IO);

/// What a path in the tree is.
#[derive(Debug, Clone)]
pub enum Kind {
    Dir,
    File(Vec<u8>),
    Symlink(String),
    /// A device node, FIFO or socket: its type bits (S_IFCHR, S_IFBLK,
    /// S_IFIFO, S_IFSOCK) and, for a device, its number as UBIFS keeps it
    /// (4 bytes, the kernel's new_encode_dev). The Explorer's root file
    /// system has /dev nodes under its /dev mount.
    Special(u32, Vec<u8>),
}

/// A file, directory or symbolic link: its permission bits and time.
#[derive(Debug, Clone)]
pub struct Entry {
    pub kind: Kind,
    pub mode: u32,
    pub mtime: u64,
}

/// Files by path ("a/b/c", no leading slash). Missing parent folders are
/// made (mode 0755).
pub type Tree = BTreeMap<String, Entry>;

/// The kernel's r5 hash of a directory entry name (key.h), masked to 29
/// bits, 0-2 reserved.
fn r5_hash(name: &[u8]) -> u32 {
    let mut a: u32 = 0;
    for &c in name {
        let c = c as i8 as i32;
        a = a.wrapping_add((c << 4) as u32);
        a = a.wrapping_add((c >> 4) as u32);
        a = a.wrapping_mul(11);
    }
    a &= 0x1FFF_FFFF;
    if a <= 2 { a + 3 } else { a }
}

fn key(inum: u32, kind: u32, low: u32) -> [u8; KEY_LEN] {
    let mut k = [0; KEY_LEN];
    k[..4].copy_from_slice(&inum.to_le_bytes());
    k[4..].copy_from_slice(&(kind << 29 | low).to_le_bytes());
    k
}

/// Keys order by inode number, then type and hash or block.
fn key_order(k: &[u8; KEY_LEN]) -> (u32, u32) {
    (u32::from_le_bytes(k[..4].try_into().unwrap()), u32::from_le_bytes(k[4..].try_into().unwrap()))
}

/// The kernel's crc32 (no final inversion) from 0xFFFFFFFF.
fn crc32(data: &[u8]) -> u32 {
    !crc32fast::hash(data)
}

/// The kernel's crc16 (lib/crc16.c, polynomial 0x8005 reflected).
fn crc16(mut crc: u16, data: &[u8]) -> u16 {
    for &b in data {
        crc ^= b as u16;
        for _ in 0..8 {
            crc = if crc & 1 != 0 { crc >> 1 ^ 0xA001 } else { crc >> 1 };
        }
    }
    crc
}

/// Number of bits needed for `x` (Linux fls).
fn fls(x: usize) -> usize {
    (usize::BITS - x.leading_zeros()) as usize
}

/// A node: its common header filled in (magic, sequence number, length,
/// type), the CRC computed over the rest.
fn node(node_type: u8, sqnum: &mut u64, body: &[u8]) -> Vec<u8> {
    let len = CH_SZ + body.len();
    let mut n = Vec::with_capacity(len);
    n.extend(MAGIC.to_le_bytes());
    n.extend([0; 4]);
    *sqnum += 1;
    n.extend(sqnum.to_le_bytes());
    n.extend((len as u32).to_le_bytes());
    n.extend([node_type, 0, 0, 0]);
    n.extend(body);
    let crc = crc32(&n[8..]);
    n[4..8].copy_from_slice(&crc.to_le_bytes());
    n
}

/// Fills `buf` (from a node boundary to a minimum I/O boundary) as the
/// kernel's ubifs_pad does: a padding node, or padding bytes when too short
/// for one.
fn pad(buf: &mut [u8]) {
    if buf.len() >= PAD_NODE_SZ {
        // Sequence number 0; the padding's length after the node.
        buf.fill(0);
        buf[..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[16..20].copy_from_slice(&(PAD_NODE_SZ as u32).to_le_bytes());
        buf[20] = PAD_NODE;
        let pad_len = (buf.len() - PAD_NODE_SZ) as u32;
        buf[24..28].copy_from_slice(&pad_len.to_le_bytes());
        let crc = crc32(&buf[8..PAD_NODE_SZ]);
        buf[4..8].copy_from_slice(&crc.to_le_bytes());
    } else {
        buf.fill(0xCE);
    }
}

/// LEBs of the volume being written.
struct Volume {
    lebs: Vec<Vec<u8>>,
}

impl Volume {
    /// Writes nodes at the start of `lnum`, padded to the minimum I/O unit.
    /// Returns the bytes used (aligned).
    fn put(&mut self, lnum: usize, nodes: &[Vec<u8>]) -> usize {
        let leb = &mut self.lebs[lnum];
        let mut at = 0;
        for n in nodes {
            leb[at..at + n.len()].copy_from_slice(n);
            let end = (at + n.len()).next_multiple_of(8);
            leb[at + n.len()..end].fill(0);
            at = end;
        }
        let aligned = at.next_multiple_of(MIN_IO);
        pad(&mut leb[at..aligned]);
        aligned
    }
}

/// LEB properties: free space, dirty space, flags.
#[derive(Clone, Copy)]
struct Lprops {
    free: usize,
    dirty: usize,
    flags: u32,
}

/// Packs bits little-end first, as the kernel's pack_bits.
#[derive(Default)]
struct Bits {
    bytes: Vec<u8>,
    pos: usize,
}

impl Bits {
    fn put(&mut self, val: u32, nrbits: usize) {
        for i in 0..nrbits {
            if self.pos % 8 == 0 {
                self.bytes.push(0);
            }
            if val >> i & 1 != 0 {
                *self.bytes.last_mut().unwrap() |= 1 << (self.pos % 8);
            }
            self.pos += 1;
        }
    }

    /// The node: a 16-bit CRC16 of the rest (`size` bytes in all), then
    /// the packed fields.
    fn finish(mut self, size: usize) -> Vec<u8> {
        self.bytes.resize(size - 2, 0);
        let crc = crc16(0xFFFF, &self.bytes);
        let mut out = crc.to_le_bytes().to_vec();
        out.extend(self.bytes);
        out
    }

    /// Starts a node: room for the CRC is added by `finish`.
    fn new(node_type: u32) -> Self {
        let mut b = Bits::default();
        b.put(node_type, LPT_TYPE_BITS);
        b
    }
}

/// The LEB properties tree's sizes (do_calc_lpt_geom, small model).
struct LptGeom {
    lpt_lebs: usize,
    main_lebs: usize,
    pnode_cnt: usize,
    space_bits: usize,
    lpt_lnum_bits: usize,
    lpt_offs_bits: usize,
    lpt_spc_bits: usize,
    pnode_sz: usize,
    nnode_sz: usize,
    ltab_sz: usize,
    lpt_sz: usize,
}

fn lpt_geom(lpt_lebs: usize, main_lebs: usize) -> LptGeom {
    let pnode_cnt = main_lebs.div_ceil(LPT_FANOUT);
    let mut nnode_cnt = pnode_cnt.div_ceil(LPT_FANOUT);
    let mut n = nnode_cnt;
    let mut height = 1;
    let mut cap = LPT_FANOUT;
    while cap < pnode_cnt {
        height += 1;
        cap *= LPT_FANOUT;
    }
    for _ in 1..height {
        n = n.div_ceil(LPT_FANOUT);
        nnode_cnt += n;
    }
    let space_bits = fls(LEB) - 3;
    let lpt_lnum_bits = fls(lpt_lebs);
    let lpt_offs_bits = fls(LEB - 1);
    let lpt_spc_bits = fls(LEB);
    let pnode_sz = (LPT_CRC_BITS + LPT_TYPE_BITS + (space_bits * 2 + 1) * LPT_FANOUT).div_ceil(8);
    let nnode_sz = (LPT_CRC_BITS + LPT_TYPE_BITS + (lpt_lnum_bits + lpt_offs_bits) * LPT_FANOUT).div_ceil(8);
    let ltab_sz = (LPT_CRC_BITS + LPT_TYPE_BITS + lpt_lebs * lpt_spc_bits * 2).div_ceil(8);
    let mut sz = pnode_cnt * pnode_sz + nnode_cnt * nnode_sz + ltab_sz;
    // Wastage at the end of each LPT LEB, as the kernel counts it.
    let waste = pnode_sz.max(nnode_sz);
    let mut total = sz + waste;
    let mut wastage = waste;
    while total > LEB {
        total += waste;
        total -= LEB;
        wastage += waste;
    }
    wastage += total.next_multiple_of(MIN_IO) - total;
    sz += wastage;
    LptGeom { lpt_lebs, main_lebs, pnode_cnt, space_bits, lpt_lnum_bits, lpt_offs_bits, lpt_spc_bits, pnode_sz, nnode_sz, ltab_sz, lpt_sz: sz }
}

/// Where the LEB properties tree ended up (for the master node).
struct LptPlace {
    lpt_lnum: usize,
    lpt_offs: usize,
    nhead_lnum: usize,
    nhead_offs: usize,
    ltab_lnum: usize,
    ltab_offs: usize,
}

/// Writes the LEB properties tree for `props` (one per main LEB) into
/// `lpt_first` (ubifs_create_dflt_lpt with the given properties).
fn write_lpt(vol: &mut Volume, g: &LptGeom, lpt_first: usize, props: &[Lprops]) -> Result<LptPlace, String> {
    let mut buf = Vec::new();
    // pnodes: 4 LEBs' properties each; past the last main LEB, free.
    for p in 0..g.pnode_cnt {
        let mut b = Bits::new(LPT_PNODE);
        for i in 0..LPT_FANOUT {
            let lp = props.get(p * LPT_FANOUT + i).copied().unwrap_or(Lprops { free: LEB, dirty: 0, flags: 0 });
            b.put((lp.free >> 3) as u32, g.space_bits);
            b.put((lp.dirty >> 3) as u32, g.space_bits);
            b.put((lp.flags & LPROPS_INDEX != 0) as u32, 1);
        }
        buf.extend(b.finish(g.pnode_sz));
    }
    // nnodes, level by level up to the root; empty branches point past
    // the LPT area.
    let (mut below_cnt, mut below_sz, mut below_offs) = (g.pnode_cnt, g.pnode_sz, 0);
    let mut cnt = g.pnode_cnt;
    let mut root = 0;
    loop {
        cnt = cnt.div_ceil(LPT_FANOUT);
        let level_offs = buf.len();
        for _ in 0..cnt {
            if cnt == 1 {
                root = buf.len();
            }
            let mut b = Bits::new(LPT_NNODE);
            for _ in 0..LPT_FANOUT {
                let (lnum, offs) = if below_cnt > 0 {
                    below_cnt -= 1;
                    let at = below_offs;
                    below_offs += below_sz;
                    (lpt_first, at)
                } else {
                    (lpt_first + g.lpt_lebs, 0)
                };
                b.put((lnum - lpt_first) as u32, g.lpt_lnum_bits);
                b.put(offs as u32, g.lpt_offs_bits);
            }
            buf.extend(b.finish(g.nnode_sz));
        }
        if cnt == 1 {
            break;
        }
        (below_cnt, below_sz, below_offs) = (cnt, g.nnode_sz, level_offs);
    }
    // The LPT LEBs' own free and dirty space, then the table itself.
    let ltab_offs = buf.len();
    let len = ltab_offs + g.ltab_sz;
    let aligned = len.next_multiple_of(MIN_IO);
    if aligned > LEB {
        return Err("the LEB properties tree doesn't fit one LEB".into());
    }
    let mut b = Bits::new(LPT_LTAB);
    for i in 0..g.lpt_lebs {
        let (free, dirty) = if i == 0 { (LEB - aligned, aligned - len) } else { (LEB, 0) };
        b.put(free as u32, g.lpt_spc_bits);
        b.put(dirty as u32, g.lpt_spc_bits);
    }
    buf.extend(b.finish(g.ltab_sz));
    buf.resize(aligned, 0xFF);
    vol.lebs[lpt_first][..aligned].copy_from_slice(&buf);
    Ok(LptPlace { lpt_lnum: lpt_first, lpt_offs: root, nhead_lnum: lpt_first, nhead_offs: aligned, ltab_lnum: lpt_first, ltab_offs })
}

/// An inode being written: its number, entry and children (directories).
struct Inode<'a> {
    inum: u32,
    entry: &'a Entry,
    children: Vec<(Vec<u8>, u32, u8)>,
}

/// The contents of a UBI volume of `leb_cnt` LEBs holding `tree` as UBIFS
/// (`leb_cnt` * LEB bytes; unused LEBs all 0xFF).
pub fn build(tree: &Tree, leb_cnt: usize) -> Result<Vec<u8>, String> {
    // Every path with its parent folders, numbered in path order (the
    // root is inode 1).
    let dir = Entry { kind: Kind::Dir, mode: 0o755, mtime: tree.values().map(|e| e.mtime).max().unwrap_or(0) };
    let mut all: BTreeMap<String, &Entry> = BTreeMap::new();
    for (path, e) in tree {
        let path = path.trim_matches('/');
        let mut parent = String::new();
        for part in path.split('/').take(path.split('/').count().saturating_sub(1)) {
            parent = if parent.is_empty() { part.to_string() } else { format!("{parent}/{part}") };
            all.entry(parent.clone()).or_insert(&dir);
        }
        all.insert(path.to_string(), e);
    }
    let mut inodes = vec![Inode { inum: ROOT_INO, entry: &dir, children: Vec::new() }];
    let mut inum_of: BTreeMap<&str, usize> = BTreeMap::new();
    for (i, (path, e)) in all.iter().enumerate() {
        inum_of.insert(path, inodes.len());
        inodes.push(Inode { inum: FIRST_INO + i as u32, entry: e, children: Vec::new() });
        let (parent, name) = path.rsplit_once('/').map_or(("", path.as_str()), |(p, n)| (p, n));
        if name.len() > 255 {
            return Err(format!("{path}: name too long"));
        }
        let at = if parent.is_empty() { 0 } else { inum_of[parent] };
        let itype = match e.kind {
            Kind::Dir => ITYPE_DIR,
            Kind::File(_) => ITYPE_REG,
            Kind::Symlink(_) => ITYPE_LNK,
            Kind::Special(t, _) => match t & 0o170000 {
                0o060000 => 3, // UBIFS_ITYPE_BLK
                0o020000 => 4, // CHR
                0o010000 => 5, // FIFO
                _ => 6,        // SOCK
            },
        };
        let child = FIRST_INO + i as u32;
        inodes[at].children.push((name.as_bytes().to_vec(), child, itype));
    }
    let highest_inum = inodes.last().map_or(ROOT_INO, |i| i.inum);

    // Geometry: superblock, 2 master LEBs, the log, the LPT, orphans, main.
    let lpt_first = LOG_LNUM + LOG_LEBS;
    let non_main = lpt_first + ORPH_LEBS;
    let mut lpt_lebs = 2;
    let g = loop {
        let main_lebs = leb_cnt.checked_sub(non_main + lpt_lebs).filter(|&m| m > 3).ok_or("the volume is too small for UBIFS")?;
        let g = lpt_geom(lpt_lebs, main_lebs);
        if g.lpt_sz > LEB {
            return Err("the volume needs the big LEB properties tree model".into());
        }
        let needed = (g.lpt_sz * 4).div_ceil(LEB);
        if needed <= lpt_lebs {
            break g;
        }
        lpt_lebs = needed;
    };
    let main_first = leb_cnt - g.main_lebs;
    let mut vol = Volume { lebs: vec![vec![0xFF; LEB]; leb_cnt] };
    let mut props = vec![Lprops { free: LEB, dirty: 0, flags: 0 }; g.main_lebs];
    let mut sqnum = 0u64;

    // The leaf nodes, packed into main LEBs from the first.
    let mut leaves: Vec<([u8; KEY_LEN], usize, usize, usize)> = Vec::new();
    let mut lnum = main_first;
    let mut pending: Vec<Vec<u8>> = Vec::new();
    let mut used = 0;
    let flush = |vol: &mut Volume, props: &mut [Lprops], lnum: usize, pending: &mut Vec<Vec<u8>>, used: usize| {
        let aligned = vol.put(lnum, pending);
        props[lnum - main_first] = Lprops { free: LEB - aligned, dirty: aligned - used, flags: 0 };
        pending.clear();
    };
    let mut add = |n: Vec<u8>, k: [u8; KEY_LEN], vol: &mut Volume, props: &mut [Lprops]| -> Result<(), String> {
        if used + n.len() > LEB {
            flush(vol, props, lnum, &mut pending, used);
            lnum += 1;
            used = 0;
            if lnum >= leb_cnt - 2 {
                return Err("the files don't fit the volume".into());
            }
        }
        leaves.push((k, lnum, used, n.len()));
        used += n.len().next_multiple_of(8);
        pending.push(n);
        Ok(())
    };
    for ino in &inodes {
        let e = ino.entry;
        let (type_bits, size, nlink, data): (u32, u64, u32, &[u8]) = match &e.kind {
            Kind::Dir => {
                let size = INO_NODE_SZ + ino.children.iter().map(|(n, _, _)| (DENT_NODE_SZ + n.len() + 1).next_multiple_of(8)).sum::<usize>();
                let subdirs = ino.children.iter().filter(|c| c.2 == ITYPE_DIR).count() as u32;
                (0o040000, size as u64, 2 + subdirs, &[])
            }
            Kind::File(d) => (0o100000, d.len() as u64, 1, &[]),
            Kind::Symlink(t) => (0o120000, t.len() as u64, 1, t.as_bytes()),
            Kind::Special(t, d) => (t & 0o170000, d.len() as u64, 1, &d[..]),
        };
        let mut body = Vec::with_capacity(INO_NODE_SZ - CH_SZ + data.len());
        let k = key(ino.inum, INO_KEY, 0);
        body.extend(k);
        body.extend([0; 8]);
        body.extend((sqnum + 1).to_le_bytes()); // creat_sqnum
        body.extend(size.to_le_bytes());
        for _ in 0..3 {
            body.extend(e.mtime.to_le_bytes());
        }
        body.extend([0; 12]); // nanoseconds
        body.extend(nlink.to_le_bytes());
        body.extend([0; 8]); // uid, gid
        body.extend((type_bits | e.mode & 0o7777).to_le_bytes());
        body.extend(COMPR_FL.to_le_bytes());
        body.extend((data.len() as u32).to_le_bytes());
        body.extend([0; 16]); // xattr count, size, padding, names
        body.extend(COMPR_LZO.to_le_bytes());
        body.resize(INO_NODE_SZ - CH_SZ, 0);
        body.extend(data);
        add(node(INO_NODE, &mut sqnum, &body), k, &mut vol, &mut props)?;
        for (name, child, itype) in &ino.children {
            let k = key(ino.inum, DENT_KEY, r5_hash(name));
            let mut body = Vec::with_capacity(DENT_NODE_SZ - CH_SZ + name.len() + 1);
            body.extend(k);
            body.extend([0; 8]);
            body.extend((*child as u64).to_le_bytes());
            body.extend([0, *itype]);
            body.extend((name.len() as u16).to_le_bytes());
            body.extend([0; 4]);
            body.extend(name);
            body.push(0);
            add(node(DENT_NODE, &mut sqnum, &body), k, &mut vol, &mut props)?;
        }
        if let Kind::File(d) = &e.kind {
            for (block, chunk) in d.chunks(BLOCK_SIZE).enumerate() {
                // All-zero blocks are holes.
                if chunk.iter().all(|&b| b == 0) {
                    continue;
                }
                let k = key(ino.inum, DATA_KEY, block as u32);
                let mut body = Vec::with_capacity(DATA_NODE_SZ - CH_SZ + chunk.len());
                body.extend(k);
                body.extend([0; 8]);
                body.extend((chunk.len() as u32).to_le_bytes());
                body.extend(COMPR_NONE.to_le_bytes());
                body.extend([0; 2]);
                body.extend(chunk);
                add(node(DATA_NODE, &mut sqnum, &body), k, &mut vol, &mut props)?;
            }
        }
    }
    flush(&mut vol, &mut props, lnum, &mut pending, used);

    // The index, bottom up, in LEBs of its own after the data.
    leaves.sort_by_key(|l| key_order(&l.0));
    let mut level_nodes: Vec<([u8; KEY_LEN], usize, usize, usize)> = leaves;
    let mut idx_lnum = lnum + 1;
    let mut idx_used = 0;
    let mut idx_pending: Vec<Vec<u8>> = Vec::new();
    let mut index_size = 0;
    let mut level = 0u16;
    let (root_lnum, root_offs, root_len) = loop {
        let mut next = Vec::new();
        for group in level_nodes.chunks(FANOUT) {
            let mut body = Vec::with_capacity(IDX_NODE_SZ - CH_SZ + group.len() * BRANCH_SZ);
            body.extend((group.len() as u16).to_le_bytes());
            body.extend(level.to_le_bytes());
            for (k, l, o, n) in group {
                body.extend((*l as u32).to_le_bytes());
                body.extend((*o as u32).to_le_bytes());
                body.extend((*n as u32).to_le_bytes());
                body.extend(k);
            }
            let n = node(IDX_NODE, &mut sqnum, &body);
            if idx_used + n.len() > LEB {
                let aligned = vol.put(idx_lnum, &idx_pending);
                props[idx_lnum - main_first] = Lprops { free: LEB - aligned, dirty: aligned - idx_used, flags: LPROPS_INDEX };
                idx_pending.clear();
                idx_lnum += 1;
                idx_used = 0;
                if idx_lnum >= leb_cnt - 1 {
                    return Err("the index doesn't fit the volume".into());
                }
            }
            next.push((group[0].0, idx_lnum, idx_used, n.len()));
            index_size += n.len().next_multiple_of(8);
            idx_used += n.len().next_multiple_of(8);
            idx_pending.push(n);
        }
        if next.len() == 1 {
            break (next[0].1, next[0].2, next[0].3);
        }
        level_nodes = next;
        level += 1;
    };
    let ihead_offs = vol.put(idx_lnum, &idx_pending);
    props[idx_lnum - main_first] = Lprops { free: LEB - ihead_offs, dirty: ihead_offs - idx_used, flags: LPROPS_INDEX };
    let gc_lnum = idx_lnum + 1;
    if gc_lnum >= leb_cnt {
        return Err("no LEB left for garbage collection".into());
    }

    let place = write_lpt(&mut vol, &g, lpt_first, &props)?;

    // Space totals as the kernel keeps them (lprops.c).
    let (mut total_free, mut total_dirty, mut total_used, mut total_dead, mut total_dark) = (0, 0, 0, 0, 0);
    let (mut empty_lebs, mut idx_lebs) = (0, 0);
    for lp in &props {
        total_free += lp.free;
        total_dirty += lp.dirty;
        if lp.free == LEB {
            empty_lebs += 1;
        }
        if lp.flags & LPROPS_INDEX != 0 {
            idx_lebs += 1;
            continue;
        }
        let spc = lp.free + lp.dirty;
        if spc < DEAD_WM {
            total_dead += spc;
        } else {
            total_dark += if spc < DARK_WM { spc } else if spc - DARK_WM < DATA_NODE_SZ + 8 { spc - (DATA_NODE_SZ + 8) } else { DARK_WM };
        }
        total_used += LEB - spc;
    }

    // Superblock (sqnum first), the log's commit start, the master last.
    let mut sb = vec![0u8; SB_NODE_SZ - CH_SZ];
    let mut at = 2;
    let mut put = |b: &mut Vec<u8>, bytes: &[u8]| {
        b[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    put(&mut sb, &[0, 0]); // r5 hash, simple keys
    for v in [0u32, MIN_IO as u32, LEB as u32, leb_cnt as u32, leb_cnt as u32] {
        put(&mut sb, &v.to_le_bytes());
    }
    put(&mut sb, &(8u64 << 20).to_le_bytes()); // max_bud_bytes, as LeapFrog's
    for v in [LOG_LEBS as u32, g.lpt_lebs as u32, ORPH_LEBS as u32, 1, FANOUT as u32, LSAVE_CNT, 4] {
        put(&mut sb, &v.to_le_bytes());
    }
    put(&mut sb, &COMPR_LZO.to_le_bytes());
    put(&mut sb, &[0; 2]);
    put(&mut sb, &[0; 16]); // rp_uid, rp_gid, rp_size
    put(&mut sb, &1_000_000_000u32.to_le_bytes());
    let uuid: Vec<u8> = (0..4u32).flat_map(|i| crc32(&[&i.to_le_bytes()[..], &(highest_inum as u64 + total_used as u64).to_le_bytes()].concat()).to_le_bytes()).collect();
    put(&mut sb, &uuid);
    vol.put(0, &[node(SB_NODE, &mut sqnum, &sb)]);

    let cs = node(CS_NODE, &mut sqnum, &0u64.to_le_bytes());
    debug_assert_eq!(cs.len(), CS_NODE_SZ);
    vol.put(LOG_LNUM, &[cs]);

    let mut m = Vec::with_capacity(MST_NODE_SZ - CH_SZ);
    m.extend((highest_inum as u64).to_le_bytes());
    m.extend(0u64.to_le_bytes()); // cmt_no
    for v in [MST_NO_ORPHS, LOG_LNUM as u32, root_lnum as u32, root_offs as u32, root_len as u32, gc_lnum as u32, idx_lnum as u32, ihead_offs as u32] {
        m.extend(v.to_le_bytes());
    }
    for v in [index_size, total_free, total_dirty, total_used, total_dead, total_dark] {
        m.extend((v as u64).to_le_bytes());
    }
    for v in [place.lpt_lnum, place.lpt_offs, place.nhead_lnum, place.nhead_offs, place.ltab_lnum, place.ltab_offs, 0, 0, main_first, empty_lebs, idx_lebs, leb_cnt] {
        m.extend((v as u32).to_le_bytes());
    }
    m.resize(MST_NODE_SZ - CH_SZ, 0);
    let mst = node(MST_NODE, &mut sqnum, &m);
    vol.put(1, &[mst.clone()]);
    vol.put(2, &[mst]);

    Ok(vol.lebs.concat())
}

// ---- writing to the journal ----------------------------------------------
//
// What the kernel does between commits (journal.c, log.c): the nodes of a
// change go to a "bud", a free LEB the log names with a reference node;
// mounting replays the buds over the committed index (replay.c) and takes
// the buds' space from what it finds there (set_bud_lprops), so neither the
// index, the LEB properties tree nor the master node change. `journal`
// writes the files that differ from a tree that way: the LEBs written are
// the buds (the base head's last bud first, while it has room) and the
// log's last LEB.

const TRUN_NODE: u8 = 4;
const REF_NODE: u8 = 8;
/// The base journal head (journal.c's BASEHD): Ribbit puts every node there.
const BASEHD: u32 = 1;
/// UBIFS_FLG_BIGLPT: the LEB properties tree's big model (not read here).
const FLG_BIGLPT: u32 = 2;
/// Free LEBs left to the kernel (garbage collection, its next commit).
const FREE_MARGIN: usize = 8;

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// The type bits (S_IFMT) of a tree entry.
fn type_bits(kind: &Kind) -> u32 {
    match kind {
        Kind::Dir => 0o040000,
        Kind::File(_) => 0o100000,
        Kind::Symlink(_) => 0o120000,
        Kind::Special(t, _) => t & 0o170000,
    }
}

/// Whether two entries hold the same (kind, contents, target, device).
fn same_kind(a: &Kind, b: &Kind) -> bool {
    match (a, b) {
        (Kind::Dir, Kind::Dir) => true,
        (Kind::File(x), Kind::File(y)) => x == y,
        (Kind::Symlink(x), Kind::Symlink(y)) => x == y,
        (Kind::Special(t, x), Kind::Special(u, y)) => t == u && x == y,
        _ => false,
    }
}

/// A directory entry's inode type for st_mode `mode`.
fn itype_of(mode: u32) -> u8 {
    match mode & 0o170000 {
        0o040000 => ITYPE_DIR,
        0o120000 => ITYPE_LNK,
        0o060000 => 3,
        0o020000 => 4,
        0o010000 => 5,
        0o140000 => 6,
        _ => ITYPE_REG,
    }
}

/// An inode node's body: `template` (the inode's node as it was: owner,
/// flags, access time and extended attributes stay) with the size, times,
/// links, mode and inline data (a link's target, a device's number) set.
#[allow(clippy::too_many_arguments)]
fn inode_body(inum: u32, template: Option<&[u8]>, mode: u32, size: u64, nlink: u32, mtime: u64, data: &[u8], creat_sqnum: u64) -> Vec<u8> {
    let mut b = match template.filter(|t| t.len() >= INO_NODE_SZ) {
        Some(t) => t[CH_SZ..INO_NODE_SZ].to_vec(),
        None => {
            let mut b = vec![0u8; INO_NODE_SZ - CH_SZ];
            b[..KEY_LEN].copy_from_slice(&key(inum, INO_KEY, 0));
            b[16..24].copy_from_slice(&creat_sqnum.to_le_bytes());
            b[32..40].copy_from_slice(&mtime.to_le_bytes());
            b[84..88].copy_from_slice(&COMPR_FL.to_le_bytes());
            b[108..110].copy_from_slice(&COMPR_LZO.to_le_bytes());
            b
        }
    };
    b[24..32].copy_from_slice(&size.to_le_bytes());
    b[40..48].copy_from_slice(&mtime.to_le_bytes()); // ctime
    b[48..56].copy_from_slice(&mtime.to_le_bytes());
    b[60..68].fill(0); // ctime and mtime nanoseconds
    b[68..72].copy_from_slice(&nlink.to_le_bytes());
    b[80..84].copy_from_slice(&mode.to_le_bytes());
    b[88..92].copy_from_slice(&(data.len() as u32).to_le_bytes());
    b.extend_from_slice(data);
    b
}

fn dent_body(parent: u32, name: &[u8], inum: u32, itype: u8) -> Vec<u8> {
    let mut b = Vec::with_capacity(DENT_NODE_SZ - CH_SZ + name.len() + 1);
    b.extend(key(parent, DENT_KEY, r5_hash(name)));
    b.extend([0; 8]);
    b.extend((inum as u64).to_le_bytes());
    b.extend([0, itype]);
    b.extend((name.len() as u16).to_le_bytes());
    b.extend([0; 4]);
    b.extend(name);
    b.push(0);
    b
}

fn data_body(inum: u32, block: usize, chunk: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(DATA_NODE_SZ - CH_SZ + chunk.len());
    b.extend(key(inum, DATA_KEY, block as u32));
    b.extend([0; 8]);
    b.extend((chunk.len() as u32).to_le_bytes());
    b.extend(COMPR_NONE.to_le_bytes());
    b.extend([0; 2]);
    b.extend(chunk);
    b
}

/// The data nodes of `new` (a file's contents) where they differ from
/// `old`; zero blocks that were zero (or holes) stay holes.
fn data_bodies(inum: u32, new: &[u8], old: &[u8], out: &mut Vec<(u8, Vec<u8>)>) {
    for (block, chunk) in new.chunks(BLOCK_SIZE).enumerate() {
        let at = block * BLOCK_SIZE;
        let was = old.get(at..(at + BLOCK_SIZE).min(old.len())).unwrap_or(&[]);
        if chunk == was || chunk.iter().all(|&b| b == 0) && was.iter().all(|&b| b == 0) {
            continue;
        }
        out.push((DATA_NODE, data_body(inum, block, chunk)));
    }
}

/// Reads bits little-end first, as the kernel's ubifs_unpack_bits.
struct BitReader<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl BitReader<'_> {
    fn get(&mut self, n: usize) -> Option<u32> {
        let mut v = 0;
        for i in 0..n {
            let byte = *self.bytes.get(self.pos / 8)?;
            v |= ((byte >> (self.pos % 8)) as u32 & 1) << i;
            self.pos += 1;
        }
        Some(v)
    }
}

/// The main LEBs the committed LEB properties tree shows empty (all free,
/// nothing dirty, not the index's) of a volume of `leb_cnt` LEBs; small
/// model (lpt.c's unpack_pnode and unpack_nnode). A branch never written
/// (LEBs a resize added) holds empty LEBs, as the kernel's read_pnode
/// makes them.
fn empty_lebs(vol: &BTreeMap<u32, Vec<u8>>, sb: &[u8], mst: &[u8], leb_cnt: usize) -> Result<Vec<usize>, String> {
    let max_leb_cnt = u32_at(sb, 44) as usize;
    let (log_lebs, lpt_lebs, orph_lebs) = (u32_at(sb, 56) as usize, u32_at(sb, 60) as usize, u32_at(sb, 64) as usize);
    let lpt_first = LOG_LNUM + log_lebs;
    let main_first = lpt_first + lpt_lebs + orph_lebs;
    let main_lebs = leb_cnt.checked_sub(main_first).ok_or("a damaged superblock")?;
    let max_pnodes = (main_lebs + max_leb_cnt.saturating_sub(leb_cnt)).div_ceil(LPT_FANOUT);
    let mut height = 1u32;
    let mut cap = LPT_FANOUT;
    while cap < max_pnodes {
        height += 1;
        cap *= LPT_FANOUT;
    }
    let g = lpt_geom(lpt_lebs, main_lebs);
    let node = |lnum: usize, offs: usize, size: usize, want: u32| -> Result<BitReader<'_>, String> {
        let bytes = vol.get(&(lnum as u32)).and_then(|l| l.get(offs..offs + size)).ok_or("an LPT node outside its LEBs")?;
        if crc16(0xFFFF, &bytes[2..]) != u16::from_le_bytes([bytes[0], bytes[1]]) {
            return Err(format!("a damaged LPT node at {lnum}:{offs}"));
        }
        let mut r = BitReader { bytes, pos: 16 };
        if r.get(LPT_TYPE_BITS) != Some(want) {
            return Err(format!("not the LPT node expected at {lnum}:{offs}"));
        }
        Ok(r)
    };
    let mut empty = Vec::new();
    // (level, LEB, offset, first pnode below)
    let mut todo = vec![(height, u32_at(mst, 120) as usize, u32_at(mst, 124) as usize, 0usize)];
    while let Some((level, lnum, offs, base)) = todo.pop() {
        if level == 0 {
            let mut r = node(lnum, offs, g.pnode_sz, LPT_PNODE)?;
            for i in 0..LPT_FANOUT {
                let free = (r.get(g.space_bits).ok_or("a short pnode")? as usize) << 3;
                let dirty = (r.get(g.space_bits).ok_or("a short pnode")? as usize) << 3;
                let index = r.get(1).ok_or("a short pnode")?;
                let n = base * LPT_FANOUT + i;
                if n < main_lebs && free == LEB && dirty == 0 && index == 0 {
                    empty.push(main_first + n);
                }
            }
            continue;
        }
        let mut r = node(lnum, offs, g.nnode_sz, LPT_NNODE)?;
        let span = LPT_FANOUT.pow(level - 1);
        for i in 0..LPT_FANOUT {
            let l = r.get(g.lpt_lnum_bits).ok_or("a short nnode")? as usize + lpt_first;
            let o = r.get(g.lpt_offs_bits).ok_or("a short nnode")? as usize;
            let below = base + i * span;
            if below >= g.pnode_cnt {
                continue;
            }
            if l < lpt_first + lpt_lebs {
                todo.push((level - 1, l, o, below));
            } else {
                empty.extend((below * LPT_FANOUT..((below + span) * LPT_FANOUT).min(main_lebs)).map(|n| main_first + n));
            }
        }
    }
    Ok(empty)
}

/// Whether `p` ("a/b/c") or a folder above it is in `set`.
fn in_or_below(p: &str, set: &HashSet<&str>) -> bool {
    let mut at = 0;
    while let Some(i) = p[at..].find('/') {
        if set.contains(&p[..at + i]) {
            return true;
        }
        at += i + 1;
    }
    set.contains(p)
}

/// A path's folder and name.
fn split(p: &str) -> (String, Vec<u8>) {
    match p.rsplit_once('/') {
        Some((parent, name)) => (parent.to_string(), name.as_bytes().to_vec()),
        None => (String::new(), p.as_bytes().to_vec()),
    }
}

/// The LEBs to write (number, contents up to where the writes end) for the
/// UBIFS volume `vol` (its LEBs by number, as `ubifs_read::files` takes
/// them) to hold `tree` where `managed` (a path "a/b", whether it is a
/// folder) says the files are the tree's: what differs goes to the journal
/// as the kernel writes it, the rest of the volume stays. Nothing when
/// nothing differs. An error when the volume can't take it that way (its
/// log or journal full, too few free LEBs, the big LPT model, a hard link
/// changed): the caller builds the volume again instead.
///
/// `vol_lebs` is the UBI volume's size: a file system made smaller (as
/// LeapFrog's root file system image, 406 LEBs) grows to it as the kernel's
/// mount grows it (sb.c: the superblock's leb_cnt; the master node's
/// follows at the next commit, master.c), so its new LEBs take the journal.
pub fn journal(vol: &BTreeMap<u32, Vec<u8>>, vol_lebs: usize, tree: &Tree, managed: &dyn Fn(&str, bool) -> bool) -> Result<Vec<(usize, Vec<u8>)>, String> {
    let count = vol.keys().next_back().map_or(0, |&l| l as usize + 1);
    let slices: Vec<&[u8]> = (0..count as u32).map(|l| vol.get(&l).map_or(&[][..], |v| &v[..])).collect();
    let m = crate::ubifs_read::mount(&slices, &|l| vol.contains_key(&(l as u32))).ok_or("no UBIFS in the volume")?;
    let sb = m.sb.as_deref().ok_or("no superblock")?;
    if u32_at(sb, 28) & FLG_BIGLPT != 0 {
        return Err("the big LEB properties tree model".into());
    }
    if u32_at(sb, 32) as usize != MIN_IO || u32_at(sb, 36) as usize != LEB {
        return Err("another flash geometry".into());
    }
    let mst = m.mst.as_deref().ok_or("no master node")?;
    let mode_of = |inum: u32| m.inodes.get(&inum).map_or(0o100000, |n| u32_at(n, 104));

    // Paths that go: the managed ones the tree hasn't (nor files below:
    // folders may be left out, as for `build`), those that changed kind, and
    // everything below them.
    let mut above: HashSet<&str> = HashSet::new();
    for p in tree.keys() {
        let mut at = 0;
        while let Some(i) = p[at..].find('/') {
            above.insert(&p[..at + i]);
            at += i + 1;
        }
    }
    let gone: HashSet<&str> = m
        .tree
        .iter()
        .filter(|(p, e)| match tree.get(*p) {
            None => !above.contains(p.as_str()) && managed(p, matches!(e.kind, Kind::Dir)),
            Some(n) => type_bits(&n.kind) != type_bits(&e.kind),
        })
        .map(|(p, _)| p.as_str())
        .collect();
    let removed: Vec<&str> = m.tree.keys().map(String::as_str).filter(|p| in_or_below(p, &gone)).collect();
    let changed = |p: &str, e: &Entry| m.tree.get(p).is_none_or(|o| o.mode != e.mode || o.mtime != e.mtime || !same_kind(&o.kind, &e.kind));
    if removed.is_empty() && tree.iter().all(|(p, e)| !changed(p, e)) {
        return Ok(Vec::new());
    }
    for p in removed.iter().copied().chain(tree.iter().filter(|(p, e)| changed(p, e)).map(|(p, _)| p.as_str())) {
        if let Some(&inum) = m.paths.get(p)
            && mode_of(inum) & 0o170000 != 0o040000
            && m.inodes.get(&inum).is_some_and(|n| u32_at(n, 92) > 1)
        {
            return Err(format!("{p} is a hard link"));
        }
    }

    let mut next_inum = m.inodes.keys().copied().max().unwrap_or(0).max(u64_at(mst, 24) as u32) + 1;
    let creat = m.max_sqnum + 1;
    let mut bodies: Vec<(u8, Vec<u8>)> = Vec::new();
    // Each touched folder's entries (name: inode, type) as they end up.
    let mut dirs: BTreeMap<u32, BTreeMap<Vec<u8>, (u32, u8)>> = BTreeMap::new();
    let entries_of = |inum: u32| -> BTreeMap<Vec<u8>, (u32, u8)> {
        m.dents.range((inum, Vec::new())..(inum + 1, Vec::new())).map(|((_, name), &child)| (name.clone(), (child, itype_of(mode_of(child))))).collect()
    };
    // Folders whose (mode, mtime) the tree sets.
    let mut dir_attrs: BTreeMap<u32, (u32, u64)> = BTreeMap::new();
    let mut inum_of: HashMap<String, u32> = m.paths.iter().filter(|(p, _)| !in_or_below(p, &gone)).map(|(p, &i)| (p.clone(), i)).collect();
    inum_of.insert(String::new(), ROOT_INO);

    // Removals, deepest first: the name, then the inode (nlink 0: the
    // replay drops all its keys).
    for p in removed.iter().rev().copied() {
        let inum = m.paths[p];
        let (parent, name) = split(p);
        if parent.is_empty() || !in_or_below(&parent, &gone) {
            let parent_inum = if parent.is_empty() { ROOT_INO } else { m.paths[parent.as_str()] };
            bodies.push((DENT_NODE, dent_body(parent_inum, &name, 0, itype_of(mode_of(inum)))));
            dirs.entry(parent_inum).or_insert_with(|| entries_of(parent_inum)).remove(&name);
        }
        if let Some(n) = m.inodes.get(&inum) {
            bodies.push((INO_NODE, inode_body(inum, Some(n), u32_at(n, 104), u64_at(n, 48), 0, u64_at(n, 72), &n[INO_NODE_SZ..], creat)));
        }
    }

    // Creations and changes, folders before what is in them.
    let mut fresh: HashSet<u32> = HashSet::new();
    let mut chain: BTreeSet<String> = BTreeSet::new();
    for (p, e) in tree {
        if !changed(p, e) {
            continue;
        }
        let (parent, name) = split(p);
        let mut up = parent.as_str();
        while !up.is_empty() && chain.insert(up.to_string()) {
            up = up.rsplit_once('/').map_or("", |u| u.0);
        }
        // Missing folders above (0755, as `build` makes them).
        let mut missing = Vec::new();
        let mut up = parent.clone();
        while !inum_of.contains_key(&up) {
            missing.push(up.clone());
            up = split(&up).0;
        }
        for d in missing.into_iter().rev() {
            let (dp, dn) = split(&d);
            let (inum, parent_inum) = (next_inum, inum_of[&dp]);
            next_inum += 1;
            inum_of.insert(d, inum);
            fresh.insert(inum);
            dirs.entry(parent_inum).or_insert_with(|| entries_of(parent_inum)).insert(dn.clone(), (inum, ITYPE_DIR));
            bodies.push((DENT_NODE, dent_body(parent_inum, &dn, inum, ITYPE_DIR)));
            dirs.insert(inum, BTreeMap::new());
            dir_attrs.insert(inum, (0o040755, e.mtime));
        }
        let parent_inum = inum_of[&parent];
        let mode = type_bits(&e.kind) | e.mode & 0o7777;
        let existing = m.tree.contains_key(p.as_str()) && !in_or_below(p, &gone);
        let inum = if existing {
            m.paths[p.as_str()]
        } else {
            let inum = next_inum;
            next_inum += 1;
            inum_of.insert(p.clone(), inum);
            fresh.insert(inum);
            inum
        };
        let template = if existing { m.inodes.get(&inum).map(Vec::as_slice) } else { None };
        match &e.kind {
            Kind::Dir => {
                if !existing {
                    dirs.insert(inum, BTreeMap::new());
                }
                dir_attrs.insert(inum, (mode, e.mtime));
            }
            Kind::File(d) => {
                let old: &[u8] = match m.tree.get(p.as_str()).map(|o| &o.kind) {
                    Some(Kind::File(o)) if existing => o,
                    _ => &[],
                };
                if d.len() < old.len() {
                    let mut b = Vec::with_capacity(32);
                    b.extend(inum.to_le_bytes());
                    b.extend([0; 12]);
                    b.extend((old.len() as u64).to_le_bytes());
                    b.extend((d.len() as u64).to_le_bytes());
                    bodies.push((TRUN_NODE, b));
                }
                data_bodies(inum, d, old, &mut bodies);
                bodies.push((INO_NODE, inode_body(inum, template, mode, d.len() as u64, 1, e.mtime, &[], creat)));
            }
            Kind::Symlink(t) => bodies.push((INO_NODE, inode_body(inum, template, mode, t.len() as u64, 1, e.mtime, t.as_bytes(), creat))),
            Kind::Special(_, d) => bodies.push((INO_NODE, inode_body(inum, template, mode, d.len() as u64, 1, e.mtime, d, creat))),
        }
        if !existing {
            let itype = itype_of(mode);
            dirs.entry(parent_inum).or_insert_with(|| entries_of(parent_inum)).insert(name.clone(), (inum, itype));
            bodies.push((DENT_NODE, dent_body(parent_inum, &name, inum, itype)));
        }
    }
    // The folders above what was written, their names and inodes again: a
    // save holds the journal's LEBs and not the index's, and carry reads a
    // file from it only with its path known up to the root
    // (`ubifs_read::read`).
    for d in &chain {
        let (parent, name) = split(d);
        if let (Some(&inum), Some(&parent_inum)) = (inum_of.get(d), inum_of.get(&parent))
            && !fresh.contains(&inum)
        {
            bodies.push((DENT_NODE, dent_body(parent_inum, &name, inum, ITYPE_DIR)));
            dirs.entry(inum).or_insert_with(|| entries_of(inum));
        }
    }
    // The folders' inodes last: size and links from their entries as they
    // end up (as `build` counts them).
    for inum in dir_attrs.keys().copied().collect::<Vec<_>>() {
        dirs.entry(inum).or_insert_with(|| entries_of(inum));
    }
    for (inum, entries) in &dirs {
        if *inum != ROOT_INO && !inum_of.values().any(|i| i == inum) {
            continue; // removed
        }
        let template = m.inodes.get(inum).map(Vec::as_slice);
        let (mode, mtime) = match (dir_attrs.get(inum), template) {
            (Some(&a), _) => a,
            (None, Some(t)) => (u32_at(t, 104), u64_at(t, 72)),
            (None, None) => (0o040755, 0),
        };
        let size = INO_NODE_SZ + entries.keys().map(|n| (DENT_NODE_SZ + n.len() + 1).next_multiple_of(8)).sum::<usize>();
        let nlink = 2 + entries.values().filter(|e| e.1 == ITYPE_DIR).count() as u32;
        bodies.push((INO_NODE, inode_body(*inum, template, mode, size as u64, nlink, mtime, &[], creat)));
    }

    // Where they go: the base head's bud while it has room, then free LEBs
    // the log names, as the kernel's journal heads fill.
    let bud_lebs: HashSet<usize> = m.buds.iter().map(|b| b.0).collect();
    let gc_lnum = u32_at(mst, 60) as usize;
    let (leb_cnt, max_leb_cnt) = (u32_at(sb, 40) as usize, u32_at(sb, 44) as usize);
    let mut out = Vec::new();
    let grown = if leb_cnt < vol_lebs && leb_cnt < max_leb_cnt { max_leb_cnt.min(vol_lebs) } else { leb_cnt };
    if grown != leb_cnt {
        let leb0 = vol.get(&0).ok_or("no superblock LEB")?;
        let mut sup = sb.to_vec();
        sup[40..44].copy_from_slice(&(grown as u32).to_le_bytes());
        let crc = crc32(&sup[8..]);
        sup[4..8].copy_from_slice(&crc.to_le_bytes());
        let mut leb = leb0.clone();
        leb[..sup.len()].copy_from_slice(&sup);
        while leb.last() == Some(&0xFF) {
            leb.pop();
        }
        out.push((0, leb));
    }
    let mut free: Vec<usize> = empty_lebs(vol, sb, mst, grown)?.into_iter().filter(|l| *l != gc_lnum && !bud_lebs.contains(l)).collect();
    // Taken from the end: unmapped LEBs first (nothing to replace), lowest
    // numbers first.
    free.sort_by_key(|&l| std::cmp::Reverse((vol.contains_key(&(l as u32)), l)));
    let erased_from = |l: usize, at: usize| vol.get(&(l as u32)).is_some_and(|leb| at % MIN_IO == 0 && at <= leb.len() && leb[at..].iter().all(|&b| b == 0xFF));
    let mut buds: Vec<(usize, Vec<u8>)> = Vec::new();
    if let Some(&(head, ..)) = m.buds.iter().rev().find(|b| b.2 == BASEHD)
        && let Some(&end) = m.bud_ends.get(&head)
        && end < LEB
        && erased_from(head, end)
    {
        buds.push((head, vol[&(head as u32)][..end].to_vec()));
    }
    let mut sqnum = m.max_sqnum;
    let mut refs: Vec<Vec<u8>> = Vec::new();
    let mut new_buds = 0;
    let pad_out = |leb: &mut Vec<u8>| {
        let at = leb.len();
        leb.resize(at.next_multiple_of(MIN_IO), 0);
        pad(&mut leb[at..]);
    };
    for (t, body) in bodies {
        let len = CH_SZ + body.len();
        if buds.last().is_none_or(|b| b.1.len().next_multiple_of(8) + len > LEB) {
            if let Some(b) = buds.last_mut() {
                pad_out(&mut b.1);
            }
            let lnum = free.pop().ok_or("no free LEB for the journal")?;
            let mut r = Vec::with_capacity(40);
            r.extend((lnum as u32).to_le_bytes());
            r.extend(0u32.to_le_bytes());
            r.extend(BASEHD.to_le_bytes());
            r.extend([0; 28]);
            refs.push(node(REF_NODE, &mut sqnum, &r));
            buds.push((lnum, Vec::new()));
            new_buds += 1;
        }
        let b = &mut buds.last_mut().unwrap().1;
        b.resize(b.len().next_multiple_of(8), 0);
        b.extend(node(t, &mut sqnum, &body));
    }
    if let Some(b) = buds.last_mut() {
        pad_out(&mut b.1);
    }
    if free.len() < FREE_MARGIN {
        return Err("too few free LEBs left".into());
    }
    let bud_bytes = m.buds.iter().map(|b| LEB - b.1.min(LEB)).sum::<usize>() + new_buds * LEB;
    if bud_bytes as u64 > u64_at(sb, 48) {
        return Err("the journal is full (max_bud_bytes)".into());
    }

    // The reference nodes after the log's last node, each in a minimum I/O
    // unit of its own as the kernel writes them; into the next log LEB when
    // the last is full, keeping two for the kernel's next commit.
    if !refs.is_empty() {
        let (lnum, end) = m.log_end;
        if m.log_lebs_used == 0 || end == 0 || !erased_from(lnum, end) {
            return Err("the log doesn't end cleanly".into());
        }
        let mut leb = vol[&(lnum as u32)][..end].to_vec();
        let mut refs = refs.into_iter();
        for r in refs.by_ref().take((LEB - end) / MIN_IO) {
            leb.extend(r);
            pad_out(&mut leb);
        }
        out.push((lnum, leb));
        let rest: Vec<Vec<u8>> = refs.collect();
        if !rest.is_empty() {
            let log_lebs = u32_at(sb, 56) as usize;
            let next = if lnum + 1 >= LOG_LNUM + log_lebs { LOG_LNUM } else { lnum + 1 };
            if m.log_lebs_used + 1 > log_lebs.saturating_sub(2) || next == u32_at(mst, 44) as usize || rest.len() > LEB / MIN_IO {
                return Err("the log is full".into());
            }
            let mut leb = Vec::new();
            for r in rest {
                leb.extend(r);
                pad_out(&mut leb);
            }
            out.push((next, leb));
        }
    }
    out.extend(buds);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn r5_matches_known_values() {
        // key_mask_hash keeps 0-2 free.
        assert!(r5_hash(b"") >= 3);
        assert_eq!(r5_hash(b"a"), (((97u32 << 4) + (97 >> 4)) * 11) & 0x1FFF_FFFF);
    }

    #[test]
    fn crc16_check_value() {
        // CRC-16/ARC of "123456789" is 0xBB3D (initial 0).
        assert_eq!(crc16(0, b"123456789"), 0xBB3D);
    }

    /// Changes written to the journal read back (and nothing else moves):
    /// a file changed in place, one shortened, one added in new folders,
    /// one and a folder removed, a link changed; then more on top, into the
    /// same bud.
    #[test]
    fn journal_writes_read_back() {
        let file = |d: &[u8]| Entry { kind: Kind::File(d.to_vec()), mode: 0o644, mtime: 1 };
        let big: Vec<u8> = (0..50_000u32).map(|i| (i * 13 % 251) as u8).collect();
        let mut t = Tree::new();
        t.insert("a/big.bin".into(), file(&big));
        t.insert("a/short".into(), file(&big[..9000]));
        t.insert("old/x".into(), file(b"x"));
        t.insert("keep/raw".into(), file(b"not ours"));
        t.insert("link".into(), Entry { kind: Kind::Symlink("a".into()), mode: 0o777, mtime: 1 });
        let leb_cnt = 300;
        let image = build(&t, leb_cnt).unwrap();
        let mut vol: BTreeMap<u32, Vec<u8>> = image.chunks(LEB).enumerate().filter(|(_, l)| l.iter().any(|&b| b != 0xFF)).map(|(i, l)| (i as u32, l.to_vec())).collect();
        let read = |vol: &BTreeMap<u32, Vec<u8>>| {
            let n = vol.keys().next_back().map_or(0, |&l| l as usize + 1);
            let s: Vec<&[u8]> = (0..n as u32).map(|l| vol.get(&l).map_or(&[][..], |v| &v[..])).collect();
            crate::ubifs_read::read(&s, &|l| vol.contains_key(&(l as u32)))
        };
        let apply = |vol: &mut BTreeMap<u32, Vec<u8>>, writes: Vec<(usize, Vec<u8>)>| {
            for (l, mut d) in writes {
                d.resize(LEB, 0xFF);
                vol.insert(l as u32, d);
            }
        };
        // "keep" isn't managed: it stays though the new tree lacks it.
        let managed = |p: &str, _dir: bool| !p.starts_with("keep");
        let mut new = t.clone();
        new.remove("keep/raw");
        new.remove("keep");
        let mut changed = big.clone();
        changed[20_000] ^= 0xFF;
        new.insert("a/big.bin".into(), file(&changed));
        new.insert("a/short".into(), file(&big[..5000]));
        new.remove("old/x");
        new.remove("old");
        new.insert("n1/n2/new.txt".into(), Entry { kind: Kind::File(b"hello".to_vec()), mode: 0o600, mtime: 7 });
        new.insert("link".into(), Entry { kind: Kind::Symlink("n1".into()), mode: 0o777, mtime: 2 });
        let writes = journal(&vol, leb_cnt, &new, &managed).unwrap();
        // The log's LEB and one bud: a 4 KiB block of the big file, not all of it.
        assert_eq!(writes.len(), 2, "{:?}", writes.iter().map(|w| (w.0, w.1.len())).collect::<Vec<_>>());
        assert!(writes.iter().all(|w| w.1.len() <= 16 << 10), "{:?}", writes.iter().map(|w| (w.0, w.1.len())).collect::<Vec<_>>());
        let written: Vec<u32> = writes.iter().map(|w| w.0 as u32).collect();
        apply(&mut vol, writes);
        // Those LEBs alone (a save made on another image, as carry reads
        // it: no master node, no index): the file made since the commit is
        // there with its folders, the changed ones aren't known.
        let part: BTreeMap<u32, Vec<u8>> = vol.iter().filter(|(l, _)| written.contains(l)).map(|(l, d)| (*l, d.clone())).collect();
        let r = read(&part);
        assert!(matches!(&r["n1/n2/new.txt"].kind, Kind::File(d) if d == b"hello"), "{:?}", r.keys().collect::<Vec<_>>());
        assert!(!r.contains_key("a/big.bin"));
        let r = read(&vol);
        let data = |r: &Tree, p: &str| match &r[p].kind {
            Kind::File(d) => d.clone(),
            k => panic!("{p}: {k:?}"),
        };
        assert_eq!(data(&r, "a/big.bin"), changed);
        assert_eq!(data(&r, "a/short"), &big[..5000]);
        assert_eq!(data(&r, "n1/n2/new.txt"), b"hello");
        assert_eq!(r["n1/n2/new.txt"].mode, 0o600);
        assert_eq!(data(&r, "keep/raw"), b"not ours");
        assert!(!r.contains_key("old") && !r.contains_key("old/x"));
        assert!(matches!(&r["link"].kind, Kind::Symlink(s) if s == "n1"));
        assert!(matches!(r["n1/n2"].kind, Kind::Dir));
        // Nothing more to write; then a second change goes into the same bud.
        assert!(journal(&vol, leb_cnt, &new, &managed).unwrap().is_empty());
        new.insert("a/short".into(), file(&big[..12_000]));
        let writes = journal(&vol, leb_cnt, &new, &managed).unwrap();
        assert_eq!(writes.len(), 1);
        apply(&mut vol, writes);
        let r = read(&vol);
        assert_eq!(data(&r, "a/short"), &big[..12_000]);
        assert_eq!(data(&r, "n1/n2/new.txt"), b"hello");
        // A file bigger than the bud: more buds, each named in the log.
        let huge: Vec<u8> = (0..400_000u32).map(|i| (i * 7 % 253) as u8).collect();
        new.insert("huge".into(), file(&huge));
        let writes = journal(&vol, leb_cnt, &new, &managed).unwrap();
        assert!(writes.len() >= 4);
        apply(&mut vol, writes);
        let r = read(&vol);
        assert_eq!(data(&r, "huge"), huge);
        assert_eq!(data(&r, "a/big.bin"), changed);
        // The journal holds 8 MB (max_bud_bytes): past that, an error (the
        // caller builds the volume again).
        new.insert("huger".into(), file(&vec![5u8; 9 << 20]));
        assert!(journal(&vol, leb_cnt, &new, &managed).is_err());
    }

    /// A volume made for fewer LEBs than UBI gives it (and a bigger
    /// max_leb_cnt, as LeapFrog's root file system) grows as the kernel's
    /// mount grows it: its superblock says so, and the new LEBs are free.
    #[test]
    fn journal_grows_a_smaller_volume() {
        let mut t = Tree::new();
        t.insert("a".into(), Entry { kind: Kind::File(vec![1; 10]), mode: 0o644, mtime: 1 });
        let image = build(&t, 300).unwrap();
        let mut vol: BTreeMap<u32, Vec<u8>> = image.chunks(LEB).enumerate().filter(|(_, l)| l.iter().any(|&b| b != 0xFF)).map(|(i, l)| (i as u32, l.to_vec())).collect();
        // max_leb_cnt 1000: the LPT's height (4) stays.
        let sb = vol.get_mut(&0).unwrap();
        sb[44..48].copy_from_slice(&1000u32.to_le_bytes());
        let crc = crc32(&sb[8..SB_NODE_SZ]);
        sb[4..8].copy_from_slice(&crc.to_le_bytes());
        let mut new = t.clone();
        new.insert("b".into(), Entry { kind: Kind::File(vec![2; 10]), mode: 0o644, mtime: 1 });
        let writes = journal(&vol, 500, &new, &|_, _| true).unwrap();
        let sb = &writes.iter().find(|w| w.0 == 0).expect("the superblock").1;
        assert_eq!(u32_at(sb, 40), 500);
        assert_eq!(crc32(&sb[8..SB_NODE_SZ]), u32_at(sb, 4));
        let n = vol.keys().next_back().map_or(0, |&l| l as usize + 1);
        let s: Vec<&[u8]> = (0..n as u32).map(|l| vol.get(&l).map_or(&[][..], |v| &v[..])).collect();
        let mst = crate::ubifs_read::mount(&s, &|l| vol.contains_key(&(l as u32))).unwrap().mst.unwrap();
        let free = empty_lebs(&vol, &vol[&0], &mst, 500).unwrap();
        assert!(free.contains(&499) && free.contains(&300) && !free.contains(&500));
    }

    #[test]
    fn small_tree_builds() {
        let mut t = Tree::new();
        t.insert("ProgramFiles/Game/meta.inf".into(), Entry { kind: Kind::File(b"Name=x\n".to_vec()), mode: 0o644, mtime: 1 });
        t.insert("link".into(), Entry { kind: Kind::Symlink("ProgramFiles".into()), mode: 0o777, mtime: 1 });
        let v = build(&t, 1270).unwrap();
        assert_eq!(v.len(), 1270 * LEB);
        assert_eq!(&v[..4], &MAGIC.to_le_bytes());
    }
}
