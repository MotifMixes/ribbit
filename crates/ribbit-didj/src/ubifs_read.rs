//! Reading a UBIFS volume back as the kernel left it (the Leapster
//! Explorer's Bulk and root file system): the committed index from the
//! master node, then the journal replayed over it (log reference nodes to
//! the buds, their nodes in sequence order), as the kernel's mount does
//! (master.c, replay.c, tnc). Data nodes are LZO, zlib or uncompressed.
//!
//! `known` says which LEBs may be trusted: a save holds only the erase
//! blocks the console changed (`carry`), so index nodes and leaves in other
//! LEBs are skipped, and a file is returned only when its inode, its name
//! (all the way to the root) and every data block are known.
//!
//! The Explorer's HLE builds its file tree from the same reading
//! (`files_in_ubi`, `files`: a UBI image or a NAND partition's LEBs).

use std::collections::{BTreeMap, HashMap};

use crate::ubifs::{Entry, Kind, Tree};

const MAGIC: u32 = 0x0610_1831;
const CH_SZ: usize = 24;
const INO_NODE: u8 = 0;
const DATA_NODE: u8 = 1;
const DENT_NODE: u8 = 2;
const TRUN_NODE: u8 = 4;
const PAD_NODE: u8 = 5;
const MST_NODE: u8 = 7;
const REF_NODE: u8 = 8;
const IDX_NODE: u8 = 9;
const CS_NODE: u8 = 10;
const KEY_INO: u32 = 0;
const KEY_DATA: u32 = 1;
const BLOCK_SIZE: u64 = 4096;
const ROOT_INO: u32 = 1;
/// Ribbit's and mkfs.ubifs's layout: superblock, 2 master LEBs, the log
/// from LEB 3 (its length: the superblock's log_lebs, 5 here).
const MST_LNUM: usize = 1;
const LOG_LNUM: usize = 3;
const LOG_LEBS: usize = 5;
const MIN_IO: usize = 2048;

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().unwrap())
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().unwrap())
}

/// A key as (inode, type, hash or block): ordered as the index orders them.
type Key = (u32, u32, u32);

fn key_at(b: &[u8], at: usize) -> Key {
    let w = u32_at(b, at + 4);
    (u32_at(b, at), w >> 29, w & 0x1FFF_FFFF)
}

/// LZO1X decompression (the kernel's lzo1x_decompress_safe, before the
/// lzo-rle extension). None on damaged input.
pub fn lzo1x_decompress(src: &[u8], max_out: usize) -> Option<Vec<u8>> {
    let mut out: Vec<u8> = Vec::with_capacity(max_out);
    let mut ip = 0usize;
    let byte = |ip: &mut usize| -> Option<usize> {
        let b = *src.get(*ip)? as usize;
        *ip += 1;
        Some(b)
    };
    // A run length: zero bytes add 255 each, then the next byte and `base`.
    let extend = |ip: &mut usize, base: usize| -> Option<usize> {
        let mut t = 0usize;
        while *src.get(*ip)? == 0 {
            t += 255;
            *ip += 1;
        }
        Some(t + base + byte(ip)?)
    };
    let literals = |out: &mut Vec<u8>, ip: &mut usize, n: usize| -> Option<()> {
        out.extend_from_slice(src.get(*ip..*ip + n)?);
        *ip += n;
        (out.len() <= max_out).then_some(())
    };
    let copy = |out: &mut Vec<u8>, back: usize, n: usize| -> Option<()> {
        let from = out.len().checked_sub(back)?;
        if back >= n {
            out.extend_from_within(from..from + n);
        } else {
            for i in 0..n {
                let b = out[from + i];
                out.push(b);
            }
        }
        (out.len() <= max_out).then_some(())
    };
    let mut state;
    let mut next;
    let mut t = *src.first()? as usize;
    if t > 17 {
        ip = 1;
        t -= 17;
        if t < 4 {
            literals(&mut out, &mut ip, t)?;
            state = t;
        } else {
            literals(&mut out, &mut ip, t)?;
            state = 4;
        }
    } else {
        state = 0;
    }
    loop {
        let t0 = byte(&mut ip)?;
        let (back, len);
        if t0 < 16 {
            if state == 0 {
                let n = if t0 == 0 { extend(&mut ip, 15)? } else { t0 } + 3;
                literals(&mut out, &mut ip, n)?;
                state = 4;
                continue;
            } else if state != 4 {
                next = t0 & 3;
                back = 1 + (t0 >> 2) + (byte(&mut ip)? << 2);
                len = 2;
            } else {
                next = t0 & 3;
                back = 1 + 0x800 + (t0 >> 2) + (byte(&mut ip)? << 2);
                len = 3;
            }
        } else if t0 >= 64 {
            next = t0 & 3;
            back = 1 + ((t0 >> 2) & 7) + (byte(&mut ip)? << 3);
            len = (t0 >> 5) + 1;
        } else if t0 >= 32 {
            len = if t0 & 31 == 0 { extend(&mut ip, 31)? + 2 } else { (t0 & 31) + 2 };
            let w = u16::from_le_bytes(src.get(ip..ip + 2)?.try_into().ok()?) as usize;
            ip += 2;
            back = 1 + (w >> 2);
            next = w & 3;
        } else {
            len = if t0 & 7 == 0 { extend(&mut ip, 7)? + 2 } else { (t0 & 7) + 2 };
            let w = u16::from_le_bytes(src.get(ip..ip + 2)?.try_into().ok()?) as usize;
            ip += 2;
            let far = (t0 & 8) << 11;
            if far == 0 && w >> 2 == 0 {
                return Some(out); // end of stream
            }
            back = far + (w >> 2) + 0x4000;
            next = w & 3;
        }
        copy(&mut out, back, len)?;
        state = next;
        literals(&mut out, &mut ip, next)?;
    }
}

/// A data node's payload uncompressed (`size` bytes).
fn decompress(compr: u16, data: &[u8], size: usize) -> Option<Vec<u8>> {
    let out = match compr {
        0 => data.to_vec(),
        1 => lzo1x_decompress(data, size)?,
        // The kernel's "deflate": raw deflate.
        2 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, size).ok()?,
        _ => return None,
    };
    (out.len() == size).then_some(out)
}

/// The node at `at` in `leb` if its header and CRC check out: (type,
/// sqnum, the whole node).
fn node_at(leb: &[u8], at: usize) -> Option<(u8, u64, &[u8])> {
    let h = leb.get(at..at + CH_SZ)?;
    if u32_at(h, 0) != MAGIC {
        return None;
    }
    let len = u32_at(h, 16) as usize;
    let n = leb.get(at..at.checked_add(len)?)?;
    if len < CH_SZ || !crc32fast::hash(&n[8..]) != u32_at(h, 4) {
        return None;
    }
    Some((h[20], u64_at(h, 8), n))
}

/// The nodes written in `leb` from `offs` on, as the kernel's scan finds
/// them: (offset, type, sqnum, node), stopping where the writes end.
fn scan(leb: &[u8], offs: usize) -> Vec<(usize, u8, u64, &[u8])> {
    scan_to_end(leb, offs).0
}

/// `scan`, and the offset where the writes end.
fn scan_to_end(leb: &[u8], mut offs: usize) -> (Vec<(usize, u8, u64, &[u8])>, usize) {
    let mut out = Vec::new();
    while offs + CH_SZ <= leb.len() {
        if let Some((t, sq, n)) = node_at(leb, offs) {
            if t == PAD_NODE {
                offs += n.len() + u32_at(n, 24) as usize;
            } else {
                out.push((offs, t, sq, n));
                offs += n.len().next_multiple_of(8);
            }
        } else if leb[offs] == 0xCE {
            // Padding bytes up to the next minimum I/O unit.
            offs = (offs + 1).next_multiple_of(MIN_IO);
        } else {
            break;
        }
    }
    (out, offs.min(leb.len()))
}

struct Inode {
    /// The node as written (the journal writer keeps its other fields).
    node: Vec<u8>,
    /// The sequence number it was created with: after the last commit, no
    /// key of it can be in the index.
    creat_sqnum: u64,
    mode: u32,
    size: u64,
    nlink: u32,
    mtime: u64,
    /// A symbolic link's target.
    data: Vec<u8>,
}

#[derive(Default)]
struct State {
    inodes: HashMap<u32, Inode>,
    dents: BTreeMap<(u32, Vec<u8>), u32>,
    data: HashMap<(u32, u32), Vec<u8>>,
    /// Key ranges whose index nodes or leaves lie in unknown LEBs.
    unknown: Vec<(Key, Key)>,
    /// Data nodes that wouldn't decompress.
    broken: std::collections::HashSet<u32>,
}

impl State {
    /// Applies a leaf node (from the index or the journal).
    fn leaf(&mut self, t: u8, n: &[u8], journal: bool) {
        match t {
            INO_NODE if n.len() >= 160 => {
                let (inum, ..) = key_at(n, 24);
                let ino = Inode {
                    node: n.to_vec(),
                    creat_sqnum: u64_at(n, 40),
                    size: u64_at(n, 48),
                    mtime: u64_at(n, 72),
                    nlink: u32_at(n, 92),
                    mode: u32_at(n, 104),
                    data: n[160..160 + (u32_at(n, 112) as usize).min(n.len() - 160)].to_vec(),
                };
                if journal && ino.nlink == 0 {
                    // Deleted: the kernel's replay removes the inode's keys.
                    self.inodes.remove(&inum);
                    self.data.retain(|k, _| k.0 != inum);
                } else {
                    if journal && let Some(old) = self.inodes.get(&inum) && ino.size < old.size {
                        let keep = ino.size.div_ceil(BLOCK_SIZE) as u32;
                        self.data.retain(|k, _| k.0 != inum || k.1 < keep);
                    }
                    self.inodes.insert(inum, ino);
                }
            }
            DENT_NODE if n.len() >= 56 => {
                let (parent, ..) = key_at(n, 24);
                let inum = u64_at(n, 40) as u32;
                let nlen = u16_at(n, 50) as usize;
                let Some(name) = n.get(56..56 + nlen) else { return };
                if inum == 0 {
                    self.dents.remove(&(parent, name.to_vec()));
                } else {
                    self.dents.insert((parent, name.to_vec()), inum);
                }
            }
            DATA_NODE if n.len() >= 48 => {
                let (inum, _, block) = key_at(n, 24);
                match decompress(u16_at(n, 44), &n[48..], u32_at(n, 40) as usize) {
                    Some(d) => {
                        self.data.insert((inum, block), d);
                    }
                    None => {
                        self.broken.insert(inum);
                    }
                }
            }
            TRUN_NODE if n.len() >= 56 => {
                let inum = u32_at(n, 24);
                let keep = u64_at(n, 48).div_ceil(BLOCK_SIZE) as u32;
                self.data.retain(|k, _| k.0 != inum || k.1 < keep);
            }
            _ => {}
        }
    }

    /// Walks the index from the node at (lnum, offs), its keys in [lo, hi).
    fn index(&mut self, vol: &[&[u8]], known: &dyn Fn(usize) -> bool, lnum: usize, offs: usize, lo: Key, hi: Key, depth: u32) {
        let Some(leb) = vol.get(lnum).filter(|_| known(lnum) && depth < 64) else {
            self.unknown.push((lo, hi));
            return;
        };
        let Some((IDX_NODE, _, n)) = node_at(leb, offs) else {
            self.unknown.push((lo, hi));
            return;
        };
        let count = u16_at(n, 24) as usize;
        let level = u16_at(n, 26);
        let branch = |i: usize| -> Option<(usize, usize, Key)> {
            let at = 28 + i * 20;
            let b = n.get(at..at + 20)?;
            Some((u32_at(b, 0) as usize, u32_at(b, 4) as usize, key_at(b, 12)))
        };
        for i in 0..count {
            let Some((l, o, k)) = branch(i) else { break };
            let next = branch(i + 1).map_or(hi, |b| b.2);
            if level > 0 {
                self.index(vol, known, l, o, k, next, depth + 1);
            } else if let Some((t, _, leaf)) = vol.get(l).filter(|_| known(l)).and_then(|leb| node_at(leb, o)) {
                self.leaf(t, leaf, false);
            } else {
                self.unknown.push((k, (k.0, k.1, k.2 + 1)));
            }
        }
    }

    fn unknown_in(&self, lo: Key, hi: Key) -> bool {
        self.unknown.iter().any(|&(a, b)| a < hi && lo < b)
    }
}

/// The files of the UBIFS volume `vol` (its LEBs in order) that can be read
/// whole from the LEBs `known` accepts, as a tree of paths ("a/b", no
/// leading slash). Ownership isn't kept (Ribbit's UBIFS writer makes every
/// file root's).
pub fn read(vol: &[&[u8]], known: &dyn Fn(usize) -> bool) -> Tree {
    mount(vol, known).map_or_else(Tree::new, |m| m.tree)
}

/// A volume as `read` finds it, with what writing to its journal needs
/// (`ubifs::journal`): the superblock and master nodes, the log's end, the
/// buds, every inode (its node), name and data block, and each path's
/// inode number.
pub(crate) struct Mounted {
    pub sb: Option<Vec<u8>>,
    pub mst: Option<Vec<u8>>,
    /// The highest sequence number in the master node, the log and the
    /// buds.
    pub max_sqnum: u64,
    /// The log: (LEB, offset) where its writes end, and the LEBs it takes.
    pub log_end: (usize, usize),
    pub log_lebs_used: usize,
    /// The buds the log names: (LEB, start, journal head), and where each
    /// LEB's writes end.
    pub buds: Vec<(usize, usize, u32)>,
    pub bud_ends: HashMap<usize, usize>,
    /// Inode nodes by number.
    pub inodes: HashMap<u32, Vec<u8>>,
    /// Directory entries: (parent, name) to inode.
    pub dents: BTreeMap<(u32, Vec<u8>), u32>,
    /// Paths ("a/b") to inode numbers, for the files in `tree`.
    pub paths: BTreeMap<String, u32>,
    pub tree: Tree,
}

pub(crate) fn mount(vol: &[&[u8]], known: &dyn Fn(usize) -> bool) -> Option<Mounted> {
    let mut s = State::default();
    let sb = vol.first().filter(|_| known(0)).and_then(|leb| node_at(leb, 0)).filter(|n| n.0 == 6).map(|n| n.2.to_vec());
    let (log_first, log_lebs) = sb.as_ref().map_or((LOG_LNUM, LOG_LEBS), |sb| (LOG_LNUM, u32_at(sb, 56) as usize));
    // The master node: the last valid one in its LEB.
    let mst = vol.get(MST_LNUM).filter(|_| known(MST_LNUM)).and_then(|leb| scan(leb, 0).into_iter().filter(|n| n.1 == MST_NODE).max_by_key(|n| n.2));
    let (cmt_no, log_lnum, mut max_sqnum) = match mst {
        Some((_, _, sq, mst)) => {
            let (root_lnum, root_offs) = (u32_at(mst, 48) as usize, u32_at(mst, 52) as usize);
            s.index(vol, known, root_lnum, root_offs, (0, 0, 0), (u32::MAX, u32::MAX, u32::MAX), 0);
            (u64_at(mst, 32), u32_at(mst, 44) as usize, sq)
        }
        None => {
            // A save without the master node (written to the journal since
            // the last commit): the newest commit start node in the log
            // tells the commit, the index is unknown, and what the journal
            // holds whole (files made since, `creat_sqnum`) is read.
            let (_, lnum, cmt_no) = (log_first..log_first + log_lebs)
                .filter(|&l| known(l))
                .filter_map(|l| node_at(vol.get(l)?, 0).filter(|n| n.0 == CS_NODE).map(|n| (n.1, l, u64_at(n.2, 24))))
                .max()?;
            s.unknown.push(((0, 0, 0), (u32::MAX, u32::MAX, u32::MAX)));
            (cmt_no, lnum, 0)
        }
    };

    // The journal: reference nodes in the log after the commit start node,
    // the buds they name scanned from there, replayed in sequence order.
    let mut buds = Vec::new();
    let mut cs_sqnum = None;
    let mut lnum = log_lnum;
    let (mut log_end, mut log_lebs_used) = ((log_lnum, 0), 0);
    for _ in 0..log_lebs {
        let Some(leb) = vol.get(lnum).filter(|_| known(lnum)) else { break };
        let (nodes, end) = scan_to_end(leb, 0);
        if nodes.is_empty() {
            break;
        }
        let mut stop = false;
        let mut current = 0;
        for &(at, t, sq, n) in &nodes {
            match (t, cs_sqnum) {
                (CS_NODE, None) if u64_at(n, 24) == cmt_no => cs_sqnum = Some(sq),
                (_, None) => {
                    stop = true;
                    break;
                }
                (_, Some(cs)) if sq < cs => {
                    stop = true;
                    break;
                }
                (REF_NODE, Some(_)) => buds.push((u32_at(n, 24) as usize, u32_at(n, 28) as usize, u32_at(n, 32))),
                _ => {}
            }
            current = at + n.len();
            max_sqnum = max_sqnum.max(sq);
        }
        if current > 0 {
            log_end = (lnum, if stop { current } else { end });
            log_lebs_used += 1;
        }
        if stop {
            break;
        }
        lnum = if lnum + 1 >= log_first + log_lebs { log_first } else { lnum + 1 };
        if lnum == log_lnum {
            break;
        }
    }
    let mut journal = Vec::new();
    let mut journal_complete = true;
    let mut bud_ends = HashMap::new();
    for &(l, o, _) in &buds {
        match vol.get(l).filter(|_| known(l)) {
            Some(leb) => {
                let (nodes, end) = scan_to_end(leb, o);
                bud_ends.insert(l, end);
                journal.extend(nodes.into_iter().map(|(_, t, sq, n)| (sq, t, n)));
            }
            None => journal_complete = false,
        }
    }
    max_sqnum = journal.iter().map(|n| n.0).fold(max_sqnum, u64::max);
    journal.sort_by_key(|n| n.0);
    for (_, t, n) in journal {
        s.leaf(t, n, true);
    }
    if !journal_complete {
        // A bud the save doesn't hold: anything may have changed.
        s.unknown.push(((0, 0, 0), (u32::MAX, u32::MAX, u32::MAX)));
    }

    // Paths from the root down, through the names known.
    let mut children: HashMap<u32, Vec<(&[u8], u32)>> = HashMap::new();
    for ((parent, name), inum) in &s.dents {
        children.entry(*parent).or_default().push((name, *inum));
    }
    let mut tree = Tree::new();
    let mut paths = BTreeMap::new();
    let mut todo = vec![(ROOT_INO, String::new(), 0u32)];
    while let Some((dir, path, depth)) = todo.pop() {
        for &(name, inum) in children.get(&dir).map_or(&[][..], |v| v) {
            let Some(ino) = s.inodes.get(&inum) else { continue };
            let Ok(name) = std::str::from_utf8(name) else { continue };
            let p = if path.is_empty() { name.to_string() } else { format!("{path}/{name}") };
            let kind = match ino.mode & 0o170000 {
                0o040000 => {
                    if depth < 64 {
                        todo.push((inum, p.clone(), depth + 1));
                    }
                    Kind::Dir
                }
                0o120000 => Kind::Symlink(String::from_utf8_lossy(&ino.data).into_owned()),
                0o100000 => {
                    let since_commit = cs_sqnum.is_some_and(|cs| ino.creat_sqnum > cs);
                    if s.broken.contains(&inum) || !since_commit && (s.unknown_in((inum, KEY_DATA, 0), (inum, KEY_DATA + 1, 0)) || s.unknown_in((inum, KEY_INO, 0), (inum, KEY_INO, 1))) {
                        continue;
                    }
                    let mut d = vec![0u8; ino.size as usize];
                    for (b, chunk) in d.chunks_mut(BLOCK_SIZE as usize).enumerate() {
                        if let Some(src) = s.data.get(&(inum, b as u32)) {
                            let n = src.len().min(chunk.len());
                            chunk[..n].copy_from_slice(&src[..n]);
                        }
                    }
                    Kind::File(d)
                }
                // Device nodes, FIFOs, sockets (the root file system's /dev).
                t => Kind::Special(t, ino.data.clone()),
            };
            paths.insert(p.clone(), inum);
            tree.insert(p, Entry { kind, mode: ino.mode & 0o7777, mtime: ino.mtime });
        }
    }
    let inodes = s.inodes.into_iter().map(|(n, i)| (n, i.node)).collect();
    Some(Mounted { sb, mst: mst.map(|m| m.3.to_vec()), max_sqnum, log_end, log_lebs_used, buds, bud_ends, inodes, dents: s.dents, paths, tree })
}

/// A file, directory, link or device node of the volume.
#[derive(Debug, Clone)]
pub struct File {
    /// Absolute ("/" for the root).
    pub path: String,
    /// st_mode: type and permissions.
    pub mode: u32,
    pub mtime: u32,
    /// A file's contents, a link's target, a device's number (4 bytes, the
    /// kernel's new_encode_dev).
    pub data: Vec<u8>,
}

/// The logical erase blocks of volume `vol_id` in a UBI image (erase
/// blocks of `peb` bytes, each with its EC and VID headers), by number.
pub fn ubi_lebs(image: &[u8], peb: usize, vol_id: u32) -> BTreeMap<u32, Vec<u8>> {
    let be32 = |b: &[u8], o: usize| u32::from_be_bytes(b[o..o + 4].try_into().unwrap());
    let mut out: BTreeMap<u32, (u64, Vec<u8>)> = BTreeMap::new();
    for block in image.chunks(peb) {
        if block.len() < 64 || be32(block, 0) != 0x5542_4923 {
            continue;
        }
        let (vid, data) = (be32(block, 16) as usize, be32(block, 20) as usize);
        if block.len() < vid + 64 || be32(block, vid) != 0x5542_4921 || be32(block, vid + 8) != vol_id {
            continue;
        }
        let lnum = be32(block, vid + 12);
        let sqnum = u64::from_be_bytes(block[vid + 40..vid + 48].try_into().unwrap());
        if out.get(&lnum).is_none_or(|(s, _)| *s < sqnum) {
            out.insert(lnum, (sqnum, block.get(data..).unwrap_or(&[]).to_vec()));
        }
    }
    out.into_iter().map(|(k, (_, v))| (k, v)).collect()
}

/// The files of a UBIFS volume given as its LEBs by number (`ubi_lebs`, or
/// a NAND partition's), absolute paths, the type in `mode`.
pub fn files(lebs: &BTreeMap<u32, Vec<u8>>) -> Result<Vec<File>, String> {
    let count = lebs.keys().next_back().map_or(0, |&l| l as usize + 1);
    let slices: Vec<&[u8]> = (0..count as u32).map(|l| lebs.get(&l).map_or(&[][..], |v| &v[..])).collect();
    let tree = read(&slices, &|l| lebs.contains_key(&(l as u32)));
    if tree.is_empty() {
        return Err("no UBIFS (or an empty one) in the volume".into());
    }
    Ok(tree
        .into_iter()
        .map(|(path, e)| {
            let (kind, data) = match e.kind {
                Kind::Dir => (0o040000, Vec::new()),
                Kind::File(d) => (0o100000, d),
                Kind::Symlink(t) => (0o120000, t.into_bytes()),
                Kind::Special(t, d) => (t, d),
            };
            File { path: format!("/{path}"), mode: kind | e.mode, mtime: e.mtime as u32, data }
        })
        .collect())
}

/// The files of the UBIFS in a UBI image (erase blocks of 128 KiB, user
/// volume 0).
pub fn files_in_ubi(image: &[u8]) -> Result<Vec<File>, String> {
    let lebs = ubi_lebs(image, 128 << 10, 0);
    if lebs.is_empty() {
        return Err("no UBI volume 0 in the image".into());
    }
    files(&lebs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leb_slices(vol: &[u8]) -> Vec<&[u8]> {
        vol.chunks(crate::ubi::LEB).collect()
    }

    /// What Ribbit's writer builds reads back the same.
    #[test]
    fn reads_back_what_ubifs_build_wrote() {
        let mut tree = Tree::new();
        let big: Vec<u8> = (0..20_000u32).map(|i| (i * 7 % 251) as u8).collect();
        tree.insert("a/b/big.bin".into(), Entry { kind: Kind::File(big.clone()), mode: 0o644, mtime: 5 });
        tree.insert("a/empty".into(), Entry { kind: Kind::File(Vec::new()), mode: 0o600, mtime: 5 });
        tree.insert("link".into(), Entry { kind: Kind::Symlink("a/b".into()), mode: 0o777, mtime: 5 });
        let vol = crate::ubifs::build(&tree, 64).unwrap();
        let read = read(&leb_slices(&vol), &|_| true);
        let file = |p: &str| match &read.get(p).unwrap().kind {
            Kind::File(d) => d.clone(),
            _ => panic!("{p} isn't a file"),
        };
        assert_eq!(file("a/b/big.bin"), big);
        assert!(file("a/empty").is_empty());
        assert!(matches!(&read["link"].kind, Kind::Symlink(t) if t == "a/b"));
        assert!(matches!(read["a/b"].kind, Kind::Dir));
        assert_eq!(read["a/empty"].mode, 0o600);
        // Without the master LEB nothing is known.
        assert!(super::read(&leb_slices(&vol), &|l| l != 1).is_empty());
    }

    /// LZO1X: literals, a short match and the end marker.
    #[test]
    fn lzo_decompresses() {
        // "abcabcabcabc": 3 literals (first byte 17+3), an M2 match back 3
        // for 9 bytes isn't expressible in M2 (max 8), so use M3: length 9,
        // distance 3.
        let src = [20u8, b'a', b'b', b'c', 32 | 7, (2 << 2) as u8, 0, 0x11, 0, 0];
        assert_eq!(lzo1x_decompress(&src, 64).unwrap(), b"abcabcabcabc");
    }
}
