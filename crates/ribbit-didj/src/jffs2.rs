//! Just enough JFFS2 to read a file from the root file system image and to
//! write files as new nodes (enhanced.rs, the factory data). Layouts from
//! the kernel's include/linux/jffs2.h (opendidj, linux-2.6.20-lf1000).

pub const MAGIC: u16 = 0x1985;
pub const DIRENT: u16 = 0xE001;
pub const INODE: u16 = 0xE002;
pub const INODE_HEADER: usize = 68;
pub const DIRENT_HEADER: usize = 40;
const COMPR_NONE: u8 = 0;
const COMPR_RTIME: u8 = 2;
const COMPR_ZERO: u8 = 5;
const COMPR_ZLIB: u8 = 6;
/// mkfs.jffs2 keeps each data node to one page of the file.
const NODE_DATA: usize = 4096;
/// The root directory's inode (it has no node of its own).
pub const ROOT_INO: u32 = 1;
const DT_REG: u8 = 8;

/// JFFS2's CRC-32 (Linux crc32_le from 0, no final inversion).
pub fn crc(data: &[u8]) -> u32 {
    let mut h = crc32fast::Hasher::new_with_initial(!0);
    h.update(data);
    !h.finalize()
}

fn u32_at(d: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(d[at..at + 4].try_into().unwrap())
}

fn put(d: &mut [u8], at: usize, v: u32) {
    d[at..at + 4].copy_from_slice(&v.to_le_bytes());
}

fn pad(node: &mut Vec<u8>) {
    node.resize((node.len() + 3) & !3, 0xFF);
}

/// The valid nodes in `image`: (offset, type, length).
pub fn nodes(image: &[u8]) -> impl Iterator<Item = (usize, u16, usize)> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        while i + 12 <= image.len() {
            let at = i;
            let magic = u16::from_le_bytes([image[at], image[at + 1]]);
            let kind = u16::from_le_bytes([image[at + 2], image[at + 3]]);
            let len = u32_at(image, at + 4) as usize;
            if magic != MAGIC || len < 12 || at + len > image.len() || crc(&image[at..at + 8]) != u32_at(image, at + 8) {
                i += 4;
                continue;
            }
            i += (len + 3) & !3;
            return Some((at, kind, len));
        }
        None
    })
}

/// A file found in an image: its newest inode node's header, and contents.
pub struct File {
    pub header: Vec<u8>,
    pub data: Vec<u8>,
}

/// The file `name` in `image` (any directory). None if missing or
/// compressed other than zlib.
pub fn read_file(image: &[u8], name: &str) -> Option<File> {
    let ino = nodes(image)
        .filter(|&(_, kind, len)| kind == DIRENT && len >= DIRENT_HEADER)
        .filter(|&(at, ..)| image.get(at + DIRENT_HEADER..at + DIRENT_HEADER + image[at + 28] as usize) == Some(name.as_bytes()))
        .map(|(at, ..)| u32_at(image, at + 20))
        .last()?;
    let parts: Vec<usize> = nodes(image)
        .filter(|&(at, kind, len)| kind == INODE && len >= INODE_HEADER && u32_at(image, at + 12) == ino)
        .map(|(at, ..)| at)
        .collect();
    let (newest, data) = inode_data(image, parts)?;
    Some(File { header: image[newest..newest + INODE_HEADER].to_vec(), data })
}

/// The contents of the inode whose nodes are at `parts` (any order), and
/// its newest node's offset. None if a node can't be decompressed.
fn inode_data(image: &[u8], mut parts: Vec<usize>) -> Option<(usize, Vec<u8>)> {
    parts.sort_by_key(|&at| u32_at(image, at + 16));
    let &newest = parts.last()?;
    // A device node's isize is 0; its data (the device number) is dsize long.
    let size = match u32_at(image, newest + 20) & 0o170000 {
        0o020000 | 0o060000 => u32_at(image, newest + 52),
        _ => u32_at(image, newest + 28),
    };
    let mut data = vec![0; size as usize];
    for &at in &parts {
        let (offset, csize, dsize) = (u32_at(image, at + 44) as usize, u32_at(image, at + 48) as usize, u32_at(image, at + 52) as usize);
        let raw = image.get(at + INODE_HEADER..at + INODE_HEADER + csize)?;
        let bytes = match image[at + 56] {
            COMPR_NONE => raw.to_vec(),
            COMPR_ZLIB => miniz_oxide::inflate::decompress_to_vec_zlib(raw).ok()?,
            COMPR_RTIME => rtime(raw, dsize),
            COMPR_ZERO => vec![0; dsize],
            _ => return None,
        };
        let end = (offset + dsize.min(bytes.len())).min(data.len());
        if offset < end {
            data[offset..end].copy_from_slice(&bytes[..end - offset]);
        }
    }
    Some((newest, data))
}

/// JFFS2's "rtime" compressor, undone (fs/jffs2/compr_rtime.c).
fn rtime(raw: &[u8], dsize: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(dsize);
    let mut positions = [0usize; 256];
    let mut i = 0;
    while out.len() < dsize && i + 1 < raw.len() {
        let value = raw[i];
        let repeat = raw[i + 1] as usize;
        i += 2;
        out.push(value);
        let mut back = positions[value as usize];
        positions[value as usize] = out.len();
        for _ in 0..repeat {
            let b = out[back];
            out.push(b);
            back += 1;
        }
    }
    out.truncate(dsize);
    out
}

/// One entry of a whole file system (`tree`).
pub struct Entry {
    /// From the root, starting with '/'.
    pub path: String,
    /// st_mode: file type and permissions.
    pub mode: u32,
    pub mtime: u32,
    /// A file's contents, a symbolic link's target, a device's
    /// (major << 8 | minor) as two bytes (old 16-bit dev_t), or empty.
    pub data: Vec<u8>,
}

/// Every file, directory, link and device in `image`, parents before
/// their children. Newest directory entries win; deleted ones (inode 0)
/// and entries whose inode can't be read are left out.
pub fn tree(image: &[u8]) -> Vec<Entry> {
    use std::collections::HashMap;
    // (parent, name) -> (version, inode)
    let mut dirents: HashMap<(u32, Vec<u8>), (u32, u32)> = HashMap::new();
    let mut inodes: HashMap<u32, Vec<usize>> = HashMap::new();
    for (at, kind, len) in nodes(image) {
        if kind == DIRENT && len >= DIRENT_HEADER {
            let Some(name) = image.get(at + DIRENT_HEADER..at + DIRENT_HEADER + image[at + 28] as usize) else { continue };
            let (pino, version, ino) = (u32_at(image, at + 12), u32_at(image, at + 16), u32_at(image, at + 20));
            let slot = dirents.entry((pino, name.to_vec())).or_insert((0, 0));
            if version >= slot.0 {
                *slot = (version, ino);
            }
        } else if kind == INODE && len >= INODE_HEADER {
            inodes.entry(u32_at(image, at + 12)).or_default().push(at);
        }
    }
    let mut children: HashMap<u32, Vec<(Vec<u8>, u32)>> = HashMap::new();
    for ((pino, name), (_, ino)) in dirents {
        if ino != 0 {
            children.entry(pino).or_default().push((name, ino));
        }
    }
    let mut out = vec![Entry { path: "/".into(), mode: 0o040755, mtime: 0, data: Vec::new() }];
    let mut todo = vec![(ROOT_INO, String::new())];
    while let Some((dir, prefix)) = todo.pop() {
        let Some(mut list) = children.remove(&dir) else { continue };
        list.sort();
        for (name, ino) in list {
            let Some((newest, data)) = inodes.get(&ino).and_then(|p| inode_data(image, p.clone())) else { continue };
            let path = format!("{prefix}/{}", String::from_utf8_lossy(&name));
            let mode = u32_at(image, newest + 20);
            if mode & 0o170000 == 0o040000 {
                todo.push((ino, path.clone()));
            }
            out.push(Entry { path, mode, mtime: u32_at(image, newest + 36), data });
        }
    }
    out
}

/// Uncompressed inode nodes holding the whole file `data`, after the node
/// whose header is `newest` (its inode, owner and times kept; versions
/// count on from it).
pub fn rewrite(newest: &[u8], data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut version = u32_at(newest, 16);
    let chunks: Vec<&[u8]> = if data.is_empty() { vec![&[]] } else { data.chunks(NODE_DATA).collect() };
    for (n, chunk) in chunks.into_iter().enumerate() {
        version += 1;
        let mut node = newest[..INODE_HEADER].to_vec();
        put(&mut node, 4, (INODE_HEADER + chunk.len()) as u32);
        let header_crc = crc(&node[..8]);
        put(&mut node, 8, header_crc);
        put(&mut node, 16, version);
        put(&mut node, 28, data.len() as u32); // isize
        put(&mut node, 44, (n * NODE_DATA) as u32); // offset
        put(&mut node, 48, chunk.len() as u32); // csize
        put(&mut node, 52, chunk.len() as u32); // dsize
        node[56..60].copy_from_slice(&[COMPR_NONE, 0, 0, 0]);
        put(&mut node, 60, crc(chunk));
        let node_crc = crc(&node[..60]);
        put(&mut node, 64, node_crc);
        node.extend_from_slice(chunk);
        pad(&mut node);
        out.extend(node);
    }
    out
}

/// A new regular file `name` (inode `ino`, owner root, mode 0644) in the
/// root directory: its directory entry and data nodes.
pub fn new_file(ino: u32, name: &str, data: &[u8], time: u32) -> Vec<u8> {
    let mut dirent = vec![0u8; DIRENT_HEADER];
    dirent[..2].copy_from_slice(&MAGIC.to_le_bytes());
    dirent[2..4].copy_from_slice(&DIRENT.to_le_bytes());
    put(&mut dirent, 4, (DIRENT_HEADER + name.len()) as u32);
    let header_crc = crc(&dirent[..8]);
    put(&mut dirent, 8, header_crc);
    put(&mut dirent, 12, ROOT_INO); // pino
    put(&mut dirent, 16, 1); // version
    put(&mut dirent, 20, ino);
    put(&mut dirent, 24, time); // mctime
    dirent[28] = name.len() as u8;
    dirent[29] = DT_REG;
    let node_crc = crc(&dirent[..32]);
    put(&mut dirent, 32, node_crc);
    put(&mut dirent, 36, crc(name.as_bytes()));
    dirent.extend_from_slice(name.as_bytes());
    pad(&mut dirent);

    // Version 0 so that `rewrite` writes version 1 onward.
    let mut header = vec![0u8; INODE_HEADER];
    header[..2].copy_from_slice(&MAGIC.to_le_bytes());
    header[2..4].copy_from_slice(&INODE.to_le_bytes());
    put(&mut header, 12, ino);
    put(&mut header, 20, 0o100644); // mode
    for at in [32, 36, 40] {
        put(&mut header, at, time); // atime, mtime, ctime
    }
    let mut out = dirent;
    out.extend(rewrite(&header, data));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_jffs2() {
        // crc32_le(0, "123456789") without inversions.
        assert_eq!(crc(b"123456789"), 0x2DFD_2D88);
    }

    #[test]
    fn writes_and_reads_back() {
        let data: Vec<u8> = (0..10_000).map(|i| (i % 251) as u8).collect();
        let mut image = new_file(2, "UnitID.txt", b"old", 5);
        let file = read_file(&image, "UnitID.txt").unwrap();
        assert_eq!(file.data, b"old");
        assert_eq!(u32_at(&file.header, 16), 1);
        assert_eq!(u32_at(&file.header, 20), 0o100644);
        // A larger file over it: three data nodes, versions 2-4.
        image.extend(rewrite(&file.header, &data));
        let file = read_file(&image, "UnitID.txt").unwrap();
        assert_eq!(file.data, data);
        assert_eq!(u32_at(&file.header, 16), 4);
        assert_eq!(nodes(&image).count(), 5);
        // And a shorter one: its size cuts the older nodes off.
        image.extend(rewrite(&file.header, b"short"));
        assert_eq!(read_file(&image, "UnitID.txt").unwrap().data, b"short");
        assert!(read_file(&image, "other").is_none());
    }

    #[test]
    fn tree_lists_newest_entries() {
        let mut image = new_file(2, "a.txt", b"one", 1);
        image.extend(new_file(3, "b.txt", b"two", 1));
        let b = read_file(&image, "b.txt").unwrap();
        image.extend(rewrite(&b.header, b"three"));
        let t = tree(&image);
        let paths: Vec<&str> = t.iter().map(|e| e.path.as_str()).collect();
        assert_eq!(paths, ["/", "/a.txt", "/b.txt"]);
        assert_eq!(t[2].data, b"three");
        assert_eq!(t[2].mode, 0o100644);
    }

    #[test]
    fn undoes_rtime() {
        // "aaaa" + "b": 'a' then 3 repeats of the bytes from its last spot.
        assert_eq!(rtime(&[b'a', 0, b'a', 2, b'b', 0], 5), b"aaaab");
    }

    #[test]
    fn reads_zlib_nodes() {
        let mut image = new_file(7, "f", b"", 0);
        let file = read_file(&image, "f").unwrap();
        let mut node = rewrite(&file.header, b"plain text here");
        let packed = miniz_oxide::deflate::compress_to_vec_zlib(b"plain text here", 6);
        node.truncate(INODE_HEADER);
        put(&mut node, 4, (INODE_HEADER + packed.len()) as u32);
        let header_crc = crc(&node[..8]);
        put(&mut node, 8, header_crc);
        put(&mut node, 48, packed.len() as u32);
        node[56] = COMPR_ZLIB;
        node.extend(packed);
        pad(&mut node);
        image.extend(node);
        assert_eq!(read_file(&image, "f").unwrap().data, b"plain text here");
    }
}
