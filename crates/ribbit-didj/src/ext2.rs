//! ext2 file systems: built from a tree of files and read back.
//!
//! The LeapPad Explorer's firmware packages carry its root file system and
//! Bulk as tar archives ("2ext4" partitions, `leappad.rs`); the console's
//! updater formats the SD card's partitions and unpacks them. Ribbit builds
//! the file systems itself: revision 1 ext2 with 4 KiB blocks, 128-byte
//! inodes, file types in directory entries and nothing else (no journal,
//! no extents, no sparse superblocks), which the kernel's ext4 driver
//! mounts read-write ("without journal"). GUESS: the real updater's mkfs
//! options are UNKNOWN; the scripts mount Bulk with ext4-only options
//! (noauto_da_alloc), so it is ext4 there.
//!
//! The reader follows block maps (direct, single, double and triple
//! indirect), which is all the ext4 driver writes on a file system without
//! the extents feature.

use std::collections::BTreeMap;

use crate::ubifs::{Entry, Kind, Tree};

pub const BLOCK: usize = 4096;
const INODE_SIZE: usize = 128;
const BLOCKS_PER_GROUP: u64 = 32768;
const INODES_PER_GROUP: u32 = 8192;
const FIRST_INO: u32 = 11;
const ROOT_INO: u32 = 2;
const MAGIC: u16 = 0xEF53;
const FEATURE_INCOMPAT_FILETYPE: u32 = 0x2;
const PTRS: u64 = (BLOCK / 4) as u64;

const S_IFREG: u32 = 0o100000;
const S_IFDIR: u32 = 0o040000;
const S_IFLNK: u32 = 0o120000;
const S_IFMT: u32 = 0o170000;

struct Geometry {
    blocks: u64,
    groups: u64,
    gdt_blocks: u64,
    inode_table_blocks: u64,
}

impl Geometry {
    fn new(size: u64) -> Result<Self, String> {
        let mut blocks = size / BLOCK as u64;
        let inode_table_blocks = INODES_PER_GROUP as u64 * INODE_SIZE as u64 / BLOCK as u64;
        let groups = blocks.div_ceil(BLOCKS_PER_GROUP);
        let gdt_blocks = (groups * 32).div_ceil(BLOCK as u64);
        let meta = 1 + gdt_blocks + 2 + inode_table_blocks;
        // A last group too small for its own metadata is left out.
        let last = blocks - (groups - 1) * BLOCKS_PER_GROUP;
        let groups = if last < meta + 16 && groups > 1 {
            blocks = (groups - 1) * BLOCKS_PER_GROUP;
            groups - 1
        } else {
            groups
        };
        if blocks < meta + 16 {
            return Err(format!("{size} bytes is too small for a file system"));
        }
        Ok(Self { blocks, groups, gdt_blocks, inode_table_blocks })
    }

    fn group_start(&self, g: u64) -> u64 {
        g * BLOCKS_PER_GROUP
    }

    fn group_end(&self, g: u64) -> u64 {
        ((g + 1) * BLOCKS_PER_GROUP).min(self.blocks)
    }

    fn block_bitmap(&self, g: u64) -> u64 {
        self.group_start(g) + 1 + self.gdt_blocks
    }

    fn inode_bitmap(&self, g: u64) -> u64 {
        self.block_bitmap(g) + 1
    }

    fn inode_table(&self, g: u64) -> u64 {
        self.block_bitmap(g) + 2
    }

    fn first_data(&self, g: u64) -> u64 {
        self.inode_table(g) + self.inode_table_blocks
    }
}

/// Hands out data blocks in order, skipping each group's metadata.
struct Alloc<'a> {
    geo: &'a Geometry,
    group: u64,
    next: u64,
    used: Vec<u64>,
}

impl Alloc<'_> {
    fn block(&mut self) -> Result<u64, String> {
        loop {
            if self.group >= self.geo.groups {
                return Err("the file system is full".into());
            }
            if self.next < self.geo.first_data(self.group) {
                self.next = self.geo.first_data(self.group);
            }
            if self.next < self.geo.group_end(self.group) {
                self.used[self.group as usize] += 1;
                self.next += 1;
                return Ok(self.next - 1);
            }
            self.group += 1;
        }
    }
}

struct Node {
    ino: u32,
    mode: u32,
    mtime: u32,
    links: u16,
    size: u64,
    blocks: Vec<u64>,
    /// Block map (i_block) and the indirect blocks' count.
    i_block: [u32; 15],
    meta_blocks: u64,
}

/// The file type in a directory entry, from the mode.
fn dirent_type(mode: u32) -> u8 {
    match mode & S_IFMT {
        S_IFREG => 1,
        S_IFDIR => 2,
        0o020000 => 3,
        0o060000 => 4,
        0o010000 => 5,
        0o140000 => 6,
        S_IFLNK => 7,
        _ => 0,
    }
}

/// Builds an ext2 file system of `size` bytes holding `tree`; `out` gets
/// every block written, by byte offset in the file system (blocks never
/// given are free and may hold anything).
pub fn build(tree: &Tree, size: u64, mtime: u32, out: &mut dyn FnMut(u64, &[u8])) -> Result<(), String> {
    let geo = Geometry::new(size)?;
    // Every folder, with the missing ones made.
    let mut all: BTreeMap<String, Entry> = BTreeMap::new();
    all.insert(String::new(), Entry { kind: Kind::Dir, mode: 0o755, mtime: mtime as u64 });
    all.insert("lost+found".into(), Entry { kind: Kind::Dir, mode: 0o700, mtime: mtime as u64 });
    for (path, e) in tree {
        let path = path.trim_matches('/');
        if path.is_empty() {
            continue;
        }
        let mut at = 0;
        while let Some(i) = path[at..].find('/') {
            let parent = &path[..at + i];
            all.entry(parent.to_string()).or_insert(Entry { kind: Kind::Dir, mode: 0o755, mtime: mtime as u64 });
            at += i + 1;
        }
        all.insert(path.to_string(), e.clone());
    }
    // Inode numbers: the root 2, lost+found 11, the rest from 12.
    let mut ino: BTreeMap<&str, u32> = BTreeMap::new();
    ino.insert("", ROOT_INO);
    ino.insert("lost+found", FIRST_INO);
    let mut n = FIRST_INO + 1;
    for p in all.keys() {
        if !ino.contains_key(p.as_str()) {
            ino.insert(p, n);
            n += 1;
        }
    }
    let inodes_count = geo.groups as u32 * INODES_PER_GROUP;
    if n > inodes_count {
        return Err("too many files for the file system".into());
    }
    fn parent_of(p: &str) -> &str {
        p.rfind('/').map_or("", |i| &p[..i])
    }
    let name_of = |p: &str| p.rfind('/').map_or(p, |i| &p[i + 1..]).to_string();
    let mut children: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for p in all.keys().filter(|p| !p.is_empty()) {
        children.entry(parent_of(p)).or_default().push(p);
    }
    let mut alloc = Alloc { geo: &geo, group: 0, next: 0, used: vec![0; geo.groups as usize] };
    let mut nodes: Vec<Node> = Vec::new();
    for (path, e) in &all {
        let type_bits = match &e.kind {
            Kind::Dir => S_IFDIR,
            Kind::File(_) => S_IFREG,
            Kind::Symlink(_) => S_IFLNK,
            Kind::Special(t, _) => *t & S_IFMT,
        };
        let mut node = Node {
            ino: ino[path.as_str()],
            mode: type_bits | (e.mode & 0o7777),
            mtime: if e.mtime == 0 { mtime } else { e.mtime as u32 },
            links: 1,
            size: 0,
            blocks: Vec::new(),
            i_block: [0; 15],
            meta_blocks: 0,
        };
        let data: Vec<u8> = match &e.kind {
            Kind::Dir => {
                let kids = children.get(path.as_str()).map_or(&[][..], |v| v.as_slice());
                node.links = 2 + kids.iter().filter(|k| matches!(all[**k].kind, Kind::Dir)).count() as u16;
                let mut entries: Vec<(u32, u8, Vec<u8>)> = vec![(node.ino, 2, b".".to_vec()), (ino[parent_of(path)], 2, b"..".to_vec())];
                if path.is_empty() {
                    entries[1].0 = ROOT_INO;
                }
                for k in kids {
                    let kind = match &all[*k].kind {
                        Kind::Dir => 2,
                        Kind::File(_) => 1,
                        Kind::Symlink(_) => 7,
                        Kind::Special(t, _) => dirent_type(*t),
                    };
                    entries.push((ino[*k], kind, name_of(k).into_bytes()));
                }
                dir_blocks(&entries)
            }
            Kind::File(d) => d.clone(),
            Kind::Symlink(t) if t.len() < 60 => {
                node.size = t.len() as u64;
                let mut raw = [0u8; 60];
                raw[..t.len()].copy_from_slice(t.as_bytes());
                for (i, w) in raw.chunks(4).enumerate() {
                    node.i_block[i] = u32::from_le_bytes(w.try_into().unwrap());
                }
                Vec::new()
            }
            Kind::Symlink(t) => t.as_bytes().to_vec(),
            Kind::Special(_, dev) => {
                // UBIFS's new_encode_dev back to major and minor.
                let d = dev.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
                let (major, minor) = ((d & 0xF_FF00) >> 8, (d & 0xFF) | ((d >> 12) & 0xF_FF00));
                if major < 256 && minor < 256 {
                    node.i_block[0] = major << 8 | minor;
                } else {
                    node.i_block[1] = d;
                }
                Vec::new()
            }
        };
        if !data.is_empty() {
            node.size = data.len() as u64;
            let count = data.len().div_ceil(BLOCK);
            for chunk in data.chunks(BLOCK) {
                let b = alloc.block()?;
                let mut block = chunk.to_vec();
                block.resize(BLOCK, 0);
                out(b * BLOCK as u64, &block);
                node.blocks.push(b);
            }
            debug_assert_eq!(node.blocks.len(), count);
            map_blocks(&mut node, &mut alloc, out)?;
        }
        nodes.push(node);
    }
    // Inode tables and bitmaps.
    let mut free_inodes = vec![INODES_PER_GROUP; geo.groups as usize];
    let mut dirs = vec![0u32; geo.groups as usize];
    let mut tables: Vec<Vec<u8>> = (0..geo.groups).map(|_| Vec::new()).collect();
    let mut inode_bits: Vec<Vec<u8>> = (0..geo.groups).map(|_| vec![0u8; BLOCK]).collect();
    for i in 1..FIRST_INO {
        inode_bits[0][(i - 1) as usize / 8] |= 1 << ((i - 1) % 8);
    }
    free_inodes[0] -= FIRST_INO - 1;
    for node in &nodes {
        let g = ((node.ino - 1) / INODES_PER_GROUP) as usize;
        let idx = ((node.ino - 1) % INODES_PER_GROUP) as usize;
        if node.ino >= FIRST_INO {
            inode_bits[g][idx / 8] |= 1 << (idx % 8);
            free_inodes[g] -= 1;
        }
        if node.mode & S_IFMT == S_IFDIR {
            dirs[g] += 1;
        }
        let t = &mut tables[g];
        if t.len() < (idx + 1) * INODE_SIZE {
            t.resize((idx + 1) * INODE_SIZE, 0);
        }
        let raw = &mut t[idx * INODE_SIZE..][..INODE_SIZE];
        raw[0..2].copy_from_slice(&(node.mode as u16).to_le_bytes());
        raw[4..8].copy_from_slice(&(node.size as u32).to_le_bytes());
        for at in [8, 12, 16] {
            raw[at..at + 4].copy_from_slice(&node.mtime.to_le_bytes());
        }
        raw[26..28].copy_from_slice(&node.links.to_le_bytes());
        let sectors = (node.blocks.len() as u64 + node.meta_blocks) * (BLOCK as u64 / 512);
        raw[28..32].copy_from_slice(&(sectors as u32).to_le_bytes());
        for (i, w) in node.i_block.iter().enumerate() {
            raw[40 + 4 * i..44 + 4 * i].copy_from_slice(&w.to_le_bytes());
        }
        raw[108..112].copy_from_slice(&((node.size >> 32) as u32).to_le_bytes());
    }
    let mut free_blocks_total = 0u64;
    let mut gdt = vec![0u8; geo.gdt_blocks as usize * BLOCK];
    for g in 0..geo.groups {
        let meta = geo.first_data(g) - geo.group_start(g);
        let in_group = geo.group_end(g) - geo.group_start(g);
        let used = meta + alloc.used[g as usize];
        let mut bits = vec![0u8; BLOCK];
        for b in 0..used {
            bits[b as usize / 8] |= 1 << (b % 8);
        }
        // Bits past the group's end are set.
        for b in in_group..BLOCKS_PER_GROUP {
            bits[b as usize / 8] |= 1 << (b % 8);
        }
        out(geo.block_bitmap(g) * BLOCK as u64, &bits);
        // Inode bitmap: bits past INODES_PER_GROUP set.
        let ib = &mut inode_bits[g as usize];
        for i in INODES_PER_GROUP as usize..BLOCK * 8 {
            ib[i / 8] |= 1 << (i % 8);
        }
        out(geo.inode_bitmap(g) * BLOCK as u64, ib);
        let mut table = std::mem::take(&mut tables[g as usize]);
        table.resize(geo.inode_table_blocks as usize * BLOCK, 0);
        out(geo.inode_table(g) * BLOCK as u64, &table);
        let free = in_group - used;
        free_blocks_total += free;
        let d = &mut gdt[g as usize * 32..][..32];
        d[0..4].copy_from_slice(&(geo.block_bitmap(g) as u32).to_le_bytes());
        d[4..8].copy_from_slice(&(geo.inode_bitmap(g) as u32).to_le_bytes());
        d[8..12].copy_from_slice(&(geo.inode_table(g) as u32).to_le_bytes());
        d[12..14].copy_from_slice(&(free as u16).to_le_bytes());
        d[14..16].copy_from_slice(&(free_inodes[g as usize] as u16).to_le_bytes());
        d[16..18].copy_from_slice(&(dirs[g as usize] as u16).to_le_bytes());
    }
    let free_inodes_total: u32 = free_inodes.iter().sum();
    for g in 0..geo.groups {
        let mut sb = vec![0u8; 1024];
        let w32 = |sb: &mut Vec<u8>, at: usize, v: u32| sb[at..at + 4].copy_from_slice(&v.to_le_bytes());
        let w16 = |sb: &mut Vec<u8>, at: usize, v: u16| sb[at..at + 2].copy_from_slice(&v.to_le_bytes());
        w32(&mut sb, 0, inodes_count);
        w32(&mut sb, 4, geo.blocks as u32);
        w32(&mut sb, 12, free_blocks_total as u32);
        w32(&mut sb, 16, free_inodes_total);
        w32(&mut sb, 20, 0);
        w32(&mut sb, 24, 2);
        w32(&mut sb, 28, 2);
        w32(&mut sb, 32, BLOCKS_PER_GROUP as u32);
        w32(&mut sb, 36, BLOCKS_PER_GROUP as u32);
        w32(&mut sb, 40, INODES_PER_GROUP);
        w32(&mut sb, 48, mtime);
        w16(&mut sb, 54, 0xFFFF);
        w16(&mut sb, 56, MAGIC);
        w16(&mut sb, 58, 1);
        w16(&mut sb, 60, 1);
        w32(&mut sb, 64, mtime);
        w32(&mut sb, 76, 1);
        w32(&mut sb, 84, FIRST_INO);
        w16(&mut sb, 88, INODE_SIZE as u16);
        w16(&mut sb, 90, g as u16);
        w32(&mut sb, 96, FEATURE_INCOMPAT_FILETYPE);
        sb[104..120].copy_from_slice(b"RibbitLeapPad\0\0\x01");
        let start = geo.group_start(g) * BLOCK as u64;
        let mut first = vec![0u8; BLOCK];
        first[if g == 0 { 1024 } else { 0 }..][..1024].copy_from_slice(&sb);
        out(start, &first);
        out(start + BLOCK as u64, &gdt);
    }
    Ok(())
}

/// Fills `node.i_block` from its data blocks, writing indirect blocks.
fn map_blocks(node: &mut Node, alloc: &mut Alloc, out: &mut dyn FnMut(u64, &[u8])) -> Result<(), String> {
    let blocks = node.blocks.clone();
    let mut rest = &blocks[..];
    for i in 0..12 {
        if let Some((&b, r)) = rest.split_first() {
            node.i_block[i] = b as u32;
            rest = r;
        }
    }
    for level in 1..=3u32 {
        if rest.is_empty() {
            break;
        }
        let span = PTRS.pow(level) as usize;
        let take = rest.len().min(span);
        let (now, later) = rest.split_at(take);
        node.i_block[11 + level as usize] = indirect(now, level, alloc, out, &mut node.meta_blocks)? as u32;
        rest = later;
    }
    if !rest.is_empty() {
        return Err("a file too big for ext2".into());
    }
    Ok(())
}

fn indirect(blocks: &[u64], level: u32, alloc: &mut Alloc, out: &mut dyn FnMut(u64, &[u8]), meta: &mut u64) -> Result<u64, String> {
    let at = alloc.block()?;
    *meta += 1;
    let mut raw = vec![0u8; BLOCK];
    let per = PTRS.pow(level - 1) as usize;
    for (i, chunk) in blocks.chunks(per).enumerate() {
        let b = if level == 1 { chunk[0] } else { indirect(chunk, level - 1, alloc, out, meta)? };
        raw[4 * i..4 * i + 4].copy_from_slice(&(b as u32).to_le_bytes());
    }
    out(at * BLOCK as u64, &raw);
    Ok(at)
}

/// Directory blocks holding `entries` (inode, type, name).
fn dir_blocks(entries: &[(u32, u8, Vec<u8>)]) -> Vec<u8> {
    let mut out: Vec<u8> = Vec::new();
    let mut block: Vec<u8> = Vec::new();
    let mut last = 0;
    let finish = |block: &mut Vec<u8>, last: usize, out: &mut Vec<u8>| {
        let rec = (BLOCK - last) as u16;
        block[last + 4..last + 6].copy_from_slice(&rec.to_le_bytes());
        block.resize(BLOCK, 0);
        out.extend_from_slice(block);
        block.clear();
    };
    for (ino, kind, name) in entries {
        let len = (8 + name.len()).next_multiple_of(4);
        if block.len() + len > BLOCK {
            finish(&mut block, last, &mut out);
        }
        last = block.len();
        block.extend(ino.to_le_bytes());
        block.extend((len as u16).to_le_bytes());
        block.push(name.len() as u8);
        block.push(*kind);
        block.extend(name);
        block.resize(last + len, 0);
    }
    if !block.is_empty() {
        finish(&mut block, last, &mut out);
    }
    out
}

/// Reading an ext2/ext4 file system (block maps only) through `read`
/// (offset, length).
pub struct Reader<'a> {
    read: Box<dyn Fn(u64, usize) -> Vec<u8> + 'a>,
    block: usize,
    inodes_per_group: u32,
    inode_size: usize,
    gdt_at: u64,
    desc_size: usize,
}

/// A file read back: path, mode (type and permissions), mtime, contents
/// (a symlink's target for a symlink; empty for folders).
pub struct File {
    pub path: String,
    pub mode: u32,
    pub mtime: u32,
    pub data: Vec<u8>,
}

impl<'a> Reader<'a> {
    pub fn new(read: impl Fn(u64, usize) -> Vec<u8> + 'a) -> Result<Self, String> {
        let sb = read(1024, 1024);
        if u16::from_le_bytes([sb[56], sb[57]]) != MAGIC {
            return Err("no ext2 file system".into());
        }
        let r32 = |at: usize| u32::from_le_bytes(sb[at..at + 4].try_into().unwrap());
        let block = 1024usize << r32(24);
        let inode_size = if r32(76) >= 1 { u16::from_le_bytes([sb[88], sb[89]]) as usize } else { 128 };
        let incompat = r32(96);
        let desc_size = if incompat & 0x80 != 0 { u16::from_le_bytes([sb[254], sb[255]]).max(32) as usize } else { 32 };
        let gdt_at = (r32(20) as u64 + 1) * block as u64;
        Ok(Self { read: Box::new(read), block, inodes_per_group: r32(40), inode_size, gdt_at, desc_size })
    }

    fn inode(&self, ino: u32) -> Vec<u8> {
        let g = (ino - 1) / self.inodes_per_group;
        let d = (self.read)(self.gdt_at + g as u64 * self.desc_size as u64, 32);
        let table = u32::from_le_bytes(d[8..12].try_into().unwrap()) as u64;
        let idx = ((ino - 1) % self.inodes_per_group) as u64;
        (self.read)(table * self.block as u64 + idx * self.inode_size as u64, 128)
    }

    fn blocks_of(&self, inode: &[u8], count: u64) -> Vec<u64> {
        let w = |i: usize| u32::from_le_bytes(inode[40 + 4 * i..44 + 4 * i].try_into().unwrap()) as u64;
        let mut out = Vec::new();
        for i in 0..12 {
            if out.len() as u64 >= count {
                return out;
            }
            out.push(w(i));
        }
        for level in 1..=3u32 {
            if out.len() as u64 >= count {
                break;
            }
            self.walk(w(11 + level as usize), level, count, &mut out);
        }
        out
    }

    fn walk(&self, at: u64, level: u32, count: u64, out: &mut Vec<u64>) {
        let ptrs = self.block / 4;
        if at == 0 {
            let span = (ptrs as u64).pow(level);
            for _ in 0..span.min(count - out.len() as u64) {
                out.push(0);
            }
            return;
        }
        let raw = (self.read)(at * self.block as u64, self.block);
        for i in 0..ptrs {
            if out.len() as u64 >= count {
                return;
            }
            let b = u32::from_le_bytes(raw[4 * i..4 * i + 4].try_into().unwrap()) as u64;
            if level == 1 {
                out.push(b);
            } else {
                self.walk(b, level - 1, count, out);
            }
        }
    }

    fn data(&self, inode: &[u8]) -> Vec<u8> {
        let size = u32::from_le_bytes(inode[4..8].try_into().unwrap()) as u64 | (u32::from_le_bytes(inode[108..112].try_into().unwrap()) as u64) << 32;
        let mode = u16::from_le_bytes([inode[0], inode[1]]) as u32;
        let sectors = u32::from_le_bytes(inode[28..32].try_into().unwrap());
        if mode & S_IFMT == S_IFLNK && sectors == 0 {
            return inode[40..40 + (size as usize).min(60)].to_vec();
        }
        let flags = u32::from_le_bytes(inode[32..36].try_into().unwrap());
        if flags & 0x8_0000 != 0 {
            // Extents: not written on these file systems.
            return Vec::new();
        }
        let count = size.div_ceil(self.block as u64);
        let mut out = Vec::with_capacity(size as usize);
        for b in self.blocks_of(inode, count) {
            if b == 0 {
                out.extend(std::iter::repeat_n(0, self.block));
            } else {
                out.extend((self.read)(b * self.block as u64, self.block));
            }
        }
        out.truncate(size as usize);
        out
    }

    /// Every file, folder and link under the root, parents first.
    pub fn files(&self) -> Vec<File> {
        let mut out = Vec::new();
        let mut stack = vec![(ROOT_INO, String::new())];
        while let Some((ino, path)) = stack.pop() {
            let inode = self.inode(ino);
            let dir = self.data(&inode);
            let mut at = 0;
            while at + 8 <= dir.len() {
                let child = u32::from_le_bytes(dir[at..at + 4].try_into().unwrap());
                let rec = u16::from_le_bytes([dir[at + 4], dir[at + 5]]) as usize;
                let len = dir[at + 6] as usize;
                if rec < 8 {
                    break;
                }
                let name = String::from_utf8_lossy(&dir[at + 8..(at + 8 + len).min(dir.len())]).into_owned();
                at += rec;
                if child == 0 || name == "." || name == ".." || (path.is_empty() && name == "lost+found") {
                    continue;
                }
                let full = if path.is_empty() { name } else { format!("{path}/{name}") };
                let node = self.inode(child);
                let mode = u16::from_le_bytes([node[0], node[1]]) as u32;
                let mtime = u32::from_le_bytes(node[16..20].try_into().unwrap());
                match mode & S_IFMT {
                    S_IFDIR => {
                        out.push(File { path: full.clone(), mode, mtime, data: Vec::new() });
                        stack.push((child, full));
                    }
                    S_IFREG | S_IFLNK => out.push(File { path: full, mode, mtime, data: self.data(&node) }),
                    _ => {}
                }
            }
        }
        out
    }
}

/// The files `files` read back as a tree (devices left out).
pub fn tree_of(files: &[File]) -> Tree {
    files
        .iter()
        .map(|f| {
            let kind = match f.mode & S_IFMT {
                S_IFDIR => Kind::Dir,
                S_IFLNK => Kind::Symlink(String::from_utf8_lossy(&f.data).into_owned()),
                _ => Kind::File(f.data.clone()),
            };
            (f.path.clone(), Entry { kind, mode: f.mode & 0o7777, mtime: f.mtime as u64 })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let mut tree = Tree::new();
        let big: Vec<u8> = (0..5_000_000u32).map(|i| (i * 7) as u8).collect();
        tree.insert("a/b/c.txt".into(), Entry { kind: Kind::File(b"hello".to_vec()), mode: 0o644, mtime: 5 });
        tree.insert("big".into(), Entry { kind: Kind::File(big.clone()), mode: 0o644, mtime: 5 });
        tree.insert("link".into(), Entry { kind: Kind::Symlink("a/b/c.txt".into()), mode: 0o777, mtime: 5 });
        for i in 0..400 {
            tree.insert(format!("many/file{i:04}"), Entry { kind: Kind::File(vec![i as u8; 10]), mode: 0o644, mtime: 5 });
        }
        let size = 200u64 << 20;
        let mut image = vec![0u8; size as usize];
        build(&tree, size, 1, &mut |at, d| image[at as usize..at as usize + d.len()].copy_from_slice(d)).unwrap();
        let r = Reader::new(|at, n| image[at as usize..at as usize + n].to_vec()).unwrap();
        let files = r.files();
        let get = |p: &str| files.iter().find(|f| f.path == p).unwrap();
        assert_eq!(get("a/b/c.txt").data, b"hello");
        assert_eq!(get("big").data, big);
        assert_eq!(get("link").data, b"a/b/c.txt");
        assert_eq!(get("many/file0399").data, vec![399u32 as u8; 10]);
        assert_eq!(files.iter().filter(|f| f.path.starts_with("many/")).count(), 400);
    }
}
