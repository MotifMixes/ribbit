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

use std::collections::BTreeMap;

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
