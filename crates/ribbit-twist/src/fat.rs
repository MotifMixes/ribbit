//! Just enough FAT32 to prepare the console's internal storage: the
//! Firmware-Bulk package's `FileSys` image is the volume as the factory
//! writes it (VERIFIED: the base firmware mounts it at sector 0 of the
//! eMMC, with no partition table, and reads it as F:).
//!
//! That image carries a `FIRMWARE_UPGRADE` marker in its root directory,
//! which sends the firmware into its update mode (it loads
//! `system/Firmware_Update_USeng.bin` and waits for LeapFrog Connect). A
//! unit that has finished updating no longer has it, so the machine is
//! given the volume without it.

/// The volume's layout, from its boot sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Volume {
    pub bytes_per_sector: usize,
    pub sectors_per_cluster: usize,
    /// Byte offset of the first data cluster (cluster 2).
    pub data: usize,
    /// Byte offset of the first FAT.
    pub fat: usize,
    pub root_cluster: u32,
    /// The number of FATs, and the bytes in each.
    pub fats: usize,
    pub fat_bytes: usize,
    /// Data clusters on the volume (numbered 2 to `clusters + 1`).
    pub clusters: u32,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

impl Volume {
    /// The layout of the FAT32 volume starting at `d[0]`, if it is one.
    pub fn parse(d: &[u8]) -> Option<Self> {
        if d.get(0x52..0x57)? != b"FAT32" {
            return None;
        }
        let bps = u16_at(d, 11)? as usize;
        let spc = *d.get(13)? as usize;
        let reserved = u16_at(d, 14)? as usize;
        let fats = *d.get(16)? as usize;
        let per_fat = u32_at(d, 36)? as usize;
        let total = u32_at(d, 32)? as usize;
        let data_sectors = total.checked_sub(reserved + fats * per_fat)?;
        Some(Self {
            bytes_per_sector: bps,
            sectors_per_cluster: spc,
            data: (reserved + fats * per_fat) * bps,
            fat: reserved * bps,
            root_cluster: u32_at(d, 44)?,
            fats,
            fat_bytes: per_fat * bps,
            // Never more than the FAT can number.
            clusters: (data_sectors / spc.max(1)).min(per_fat * bps / 4 - 2) as u32,
        })
    }

    fn cluster_at(&self, c: u32) -> usize {
        self.data + (c as usize - 2) * self.cluster_bytes()
    }

    fn cluster_bytes(&self) -> usize {
        self.bytes_per_sector * self.sectors_per_cluster
    }

    /// The clusters of a chain, in order (stopping at the image's end).
    fn chain(&self, d: &[u8], mut c: u32) -> Vec<u32> {
        let mut out = Vec::new();
        while (2..0x0FFF_FFF8).contains(&c) && out.len() < 1 << 20 {
            out.push(c);
            match u32_at(d, self.fat + 4 * c as usize) {
                Some(next) => c = next & 0x0FFF_FFFF,
                None => break,
            }
        }
        out
    }
}

/// Marks the root-directory entry named `name` (long or short name, any
/// case) as deleted, with its long-name records. Its clusters are left
/// allocated (harmless; the marker file is empty). True if it was found.
pub fn delete_root_entry(d: &mut [u8], name: &str) -> bool {
    let Some(v) = Volume::parse(d) else {
        return false;
    };
    let want = name.to_ascii_uppercase();
    for c in v.chain(d, v.root_cluster) {
        let start = v.data + (c as usize - 2) * v.cluster_bytes();
        let end = (start + v.cluster_bytes()).min(d.len());
        let mut lfn = String::new();
        let mut lfn_at: Vec<usize> = Vec::new();
        let mut at = start;
        while at + 32 <= end {
            let e = &d[at..at + 32];
            match e[0] {
                0 => return false,
                0xE5 => {
                    lfn.clear();
                    lfn_at.clear();
                }
                _ if e[11] == 0x0F => {
                    let units: Vec<u16> = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                        .iter()
                        .map(|&i| u16::from_le_bytes([e[i], e[i + 1]]))
                        .collect();
                    let part: String = char::decode_utf16(units.into_iter().take_while(|&u| u != 0 && u != 0xFFFF))
                        .map(|c| c.unwrap_or('?'))
                        .collect();
                    lfn = part + &lfn;
                    lfn_at.push(at);
                }
                _ => {
                    let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
                    let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
                    let short = if ext.is_empty() { base } else { format!("{base}.{ext}") };
                    if lfn.to_ascii_uppercase() == want || short.to_ascii_uppercase() == want {
                        for &l in lfn_at.iter().chain(std::iter::once(&at)) {
                            d[l] = 0xE5;
                        }
                        return true;
                    }
                    lfn.clear();
                    lfn_at.clear();
                }
            }
            at += 32;
        }
    }
    false
}

/// A directory entry as found on the volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// The long name if it has one, else the short name ("NAME.EXT").
    pub name: String,
    pub is_dir: bool,
    pub cluster: u32,
    pub size: u32,
}

const LFN_UNITS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];

fn short_text(e: &[u8]) -> String {
    let base = String::from_utf8_lossy(&e[0..8]).trim_end().to_string();
    let ext = String::from_utf8_lossy(&e[8..11]).trim_end().to_string();
    if ext.is_empty() { base } else { format!("{base}.{ext}") }
}

impl Volume {
    /// The entries of the directory starting at `cluster` ("." and ".."
    /// left out), with the byte offset of each one's short entry.
    fn list(&self, d: &[u8], cluster: u32) -> Vec<(Entry, usize)> {
        let mut out = Vec::new();
        let mut lfn = String::new();
        for c in self.chain(d, cluster) {
            let start = self.cluster_at(c);
            for at in (start..start + self.cluster_bytes()).step_by(32) {
                let Some(e) = d.get(at..at + 32) else {
                    return out;
                };
                match e[0] {
                    0 => return out,
                    0xE5 => lfn.clear(),
                    _ if e[11] == 0x0F => {
                        let units: Vec<u16> = LFN_UNITS.iter().map(|&i| u16::from_le_bytes([e[i], e[i + 1]])).collect();
                        let part: String = char::decode_utf16(units.into_iter().take_while(|&u| u != 0 && u != 0xFFFF))
                            .map(|c| c.unwrap_or('?'))
                            .collect();
                        lfn = part + &lfn;
                    }
                    _ if e[11] & 0x08 != 0 => lfn.clear(), // the volume label
                    _ => {
                        let short = short_text(e);
                        if short != "." && short != ".." {
                            let name = if lfn.is_empty() { short } else { std::mem::take(&mut lfn) };
                            let cluster = (u16_at(e, 20).unwrap_or(0) as u32) << 16 | u16_at(e, 26).unwrap_or(0) as u32;
                            out.push((
                                Entry { name, is_dir: e[11] & 0x10 != 0, cluster, size: u32_at(e, 28).unwrap_or(0) },
                                at,
                            ));
                        }
                        lfn.clear();
                    }
                }
            }
        }
        out
    }
}

/// The entries of the folder at `path` ("" or "/" for the root; "/" or
/// "\" between names, any case), or None if there is no such folder.
pub fn list(d: &[u8], path: &str) -> Option<Vec<Entry>> {
    let v = Volume::parse(d)?;
    let mut cluster = v.root_cluster;
    for part in path.split(['/', '\\']).filter(|p| !p.is_empty()) {
        let (e, _) = v.list(d, cluster).into_iter().find(|(e, _)| e.is_dir && e.name.eq_ignore_ascii_case(part))?;
        cluster = e.cluster;
    }
    Some(v.list(d, cluster).into_iter().map(|(e, _)| e).collect())
}

/// The contents of the file at `path`, or None.
pub fn read_file(d: &[u8], path: &str) -> Option<Vec<u8>> {
    let v = Volume::parse(d)?;
    let (dir, name) = path.trim_start_matches(['/', '\\']).rsplit_once(['/', '\\']).unwrap_or(("", path));
    let e = list(d, dir)?.into_iter().find(|e| !e.is_dir && e.name.eq_ignore_ascii_case(name))?;
    let mut out = Vec::with_capacity(e.size as usize);
    for c in v.chain(d, e.cluster) {
        let at = v.cluster_at(c);
        let n = (e.size as usize - out.len()).min(v.cluster_bytes());
        out.extend((0..n).map(|i| d.get(at + i).copied().unwrap_or(0)));
        if out.len() == e.size as usize {
            break;
        }
    }
    (out.len() == e.size as usize).then_some(out)
}

/// Why a file could not be added.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteError {
    NotFat32,
    /// No room left on the volume.
    Full,
    /// A file is in the way of a folder, or a folder of a file.
    NotAFolder(String),
    TooBig(String),
}

impl std::fmt::Display for WriteError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            WriteError::NotFat32 => write!(f, "the volume is not FAT32"),
            WriteError::Full => write!(f, "the console's storage is full"),
            WriteError::NotAFolder(p) => write!(f, "{p} is a file where a folder is needed"),
            WriteError::TooBig(p) => write!(f, "{p} is larger than FAT32 allows (4 GB)"),
        }
    }
}

impl std::error::Error for WriteError {}

/// Adds files and folders to a FAT32 volume image, as a computer would:
/// clusters from the free space (the image grows as needed: the factory
/// image stops where its written data ends), long names, both FATs kept
/// equal. Timestamps are a fixed date (Ribbit's own choice).
pub struct Writer<'a> {
    d: &'a mut Vec<u8>,
    v: Volume,
    /// Where to look for a free cluster next.
    hint: u32,
}

/// 2019-01-01 00:00, in FAT's date format (time 0).
const FAT_DATE: u16 = (2019 - 1980) << 9 | 1 << 5 | 1;

impl<'a> Writer<'a> {
    pub fn new(d: &'a mut Vec<u8>) -> Result<Self, WriteError> {
        let v = Volume::parse(d).ok_or(WriteError::NotFat32)?;
        Ok(Self { d, v, hint: 2 })
    }

    fn grow_to(&mut self, end: usize) {
        if self.d.len() < end {
            self.d.resize(end, 0);
        }
    }

    fn fat_get(&self, c: u32) -> u32 {
        u32_at(self.d, self.v.fat + 4 * c as usize).unwrap_or(0) & 0x0FFF_FFFF
    }

    fn fat_set(&mut self, c: u32, next: u32) {
        for f in 0..self.v.fats {
            let at = self.v.fat + f * self.v.fat_bytes + 4 * c as usize;
            self.grow_to(at + 4);
            let keep = u32_at(self.d, at).unwrap_or(0) & 0xF000_0000;
            self.d[at..at + 4].copy_from_slice(&(keep | next).to_le_bytes());
        }
    }

    /// A free cluster, marked as the end of a chain and zeroed.
    fn alloc(&mut self) -> Result<u32, WriteError> {
        let last = self.v.clusters + 1;
        let mut c = self.hint.max(2);
        for _ in 0..self.v.clusters {
            if c > last {
                c = 2;
            }
            if self.fat_get(c) == 0 {
                self.fat_set(c, 0x0FFF_FFFF);
                self.hint = c + 1;
                let at = self.v.cluster_at(c);
                let n = self.v.cluster_bytes();
                self.grow_to(at + n);
                self.d[at..at + n].fill(0);
                return Ok(c);
            }
            c += 1;
        }
        Err(WriteError::Full)
    }

    /// `n` clusters chained together (none for 0), and the first.
    fn alloc_chain(&mut self, n: usize) -> Result<u32, WriteError> {
        let mut first = 0;
        let mut prev = 0;
        for _ in 0..n {
            let c = self.alloc()?;
            if prev == 0 {
                first = c;
            } else {
                self.fat_set(prev, c);
            }
            prev = c;
        }
        Ok(first)
    }

    /// A short name for `long` not used in the directory: kept as is if it
    /// already is a valid upper-case 8.3 name (then no long name is
    /// needed: the bool is false), else "BASE~N.EXT".
    fn short_name(&self, dir: u32, long: &str) -> ([u8; 11], bool) {
        let valid = |c: char| c.is_ascii_alphanumeric() || "$%'-_@~`!(){}^#&".contains(c);
        let (base, ext) = match long.rsplit_once('.') {
            Some((b, e)) if !b.is_empty() => (b, e),
            _ => (long, ""),
        };
        let fits = !base.is_empty()
            && base.len() <= 8
            && ext.len() <= 3
            && base.chars().chain(ext.chars()).all(|c| valid(c) && !c.is_ascii_lowercase());
        let pad = |s: &str, n: usize| {
            let mut b: Vec<u8> = s.bytes().take(n).collect();
            b.resize(n, b' ');
            b
        };
        if fits {
            let mut n = [b' '; 11];
            n[..8].copy_from_slice(&pad(base, 8));
            n[8..].copy_from_slice(&pad(ext, 3));
            return (n, false);
        }
        let clean = |s: &str| -> String {
            s.chars()
                .filter(|c| *c != ' ' && *c != '.')
                .map(|c| if valid(c) { c.to_ascii_uppercase() } else { '_' })
                .collect()
        };
        let (b, e) = (clean(base), clean(ext));
        let used: Vec<[u8; 11]> = self
            .v
            .chain(self.d, dir)
            .into_iter()
            .flat_map(|c| {
                let start = self.v.cluster_at(c);
                (start..start + self.v.cluster_bytes()).step_by(32).collect::<Vec<_>>()
            })
            .filter_map(|at| self.d.get(at..at + 11).map(|s| s.try_into().unwrap()))
            .collect();
        for k in 1u32.. {
            let tail = format!("~{k}");
            let keep = 8 - tail.len();
            let mut n = [b' '; 11];
            n[..8].copy_from_slice(&pad(&format!("{}{tail}", &b[..b.len().min(keep)]), 8));
            n[8..].copy_from_slice(&pad(&e, 3));
            if !used.contains(&n) {
                return (n, true);
            }
        }
        unreachable!()
    }

    /// Byte offsets of `n` consecutive free entry slots in the directory,
    /// growing it by a cluster if needed.
    fn free_slots(&mut self, dir: u32, n: usize) -> Result<Vec<usize>, WriteError> {
        let chain = self.v.chain(self.d, dir);
        let mut run = Vec::new();
        for &c in &chain {
            let start = self.v.cluster_at(c);
            for at in (start..start + self.v.cluster_bytes()).step_by(32) {
                match self.d.get(at).copied().unwrap_or(0) {
                    0 | 0xE5 => {
                        run.push(at);
                        if run.len() == n {
                            return Ok(run);
                        }
                    }
                    _ => run.clear(),
                }
            }
        }
        // A run may carry on into the new cluster.
        let c = self.alloc()?;
        self.fat_set(*chain.last().expect("a directory has a cluster"), c);
        let start = self.v.cluster_at(c);
        run.extend((start..).step_by(32).take(n - run.len()));
        Ok(run)
    }

    /// Writes an entry for `name` into `dir`: long-name records, then the
    /// short entry.
    fn add_entry(&mut self, dir: u32, name: &str, attr: u8, cluster: u32, size: u32) -> Result<(), WriteError> {
        let (short, needs_long) = self.short_name(dir, name);
        let units: Vec<u16> = name.encode_utf16().collect();
        let records = if needs_long { units.len().div_ceil(13) } else { 0 };
        let slots = self.free_slots(dir, records + 1)?;
        let sum = short.iter().fold(0u8, |s, &b| (s >> 1 | s << 7).wrapping_add(b));
        for (i, &at) in slots[..records].iter().enumerate() {
            let ord = records - i; // last part first
            let mut e = [0u8; 32];
            e[0] = ord as u8 | if i == 0 { 0x40 } else { 0 };
            e[11] = 0x0F;
            e[13] = sum;
            for (k, &off) in LFN_UNITS.iter().enumerate() {
                let idx = (ord - 1) * 13 + k;
                let u = match idx.cmp(&units.len()) {
                    std::cmp::Ordering::Less => units[idx],
                    std::cmp::Ordering::Equal => 0,
                    std::cmp::Ordering::Greater => 0xFFFF,
                };
                e[off..off + 2].copy_from_slice(&u.to_le_bytes());
            }
            self.grow_to(at + 32);
            self.d[at..at + 32].copy_from_slice(&e);
        }
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(&short);
        e[11] = attr;
        for off in [16, 18, 24] {
            e[off..off + 2].copy_from_slice(&FAT_DATE.to_le_bytes());
        }
        e[20..22].copy_from_slice(&((cluster >> 16) as u16).to_le_bytes());
        e[26..28].copy_from_slice(&(cluster as u16).to_le_bytes());
        e[28..32].copy_from_slice(&size.to_le_bytes());
        let at = *slots.last().unwrap();
        self.grow_to(at + 32);
        self.d[at..at + 32].copy_from_slice(&e);
        Ok(())
    }

    /// The folder at `path`, made (with its parents) if missing.
    pub fn folder(&mut self, path: &str) -> Result<u32, WriteError> {
        let mut dir = self.v.root_cluster;
        let mut so_far = String::new();
        for part in path.split(['/', '\\']).filter(|p| !p.is_empty()) {
            so_far.push('/');
            so_far.push_str(part);
            match self.v.list(self.d, dir).into_iter().find(|(e, _)| e.name.eq_ignore_ascii_case(part)) {
                Some((e, _)) if e.is_dir => dir = e.cluster,
                Some(_) => return Err(WriteError::NotAFolder(so_far)),
                None => {
                    let c = self.alloc()?;
                    // "." and "..", as FAT asks (the root is cluster 0 here).
                    let parent = if dir == self.v.root_cluster { 0 } else { dir };
                    for (k, (n, target)) in [(".", c), ("..", parent)].into_iter().enumerate() {
                        let at = self.v.cluster_at(c) + 32 * k;
                        let mut e = [b' '; 11];
                        e[..n.len()].copy_from_slice(n.as_bytes());
                        self.d[at..at + 11].copy_from_slice(&e);
                        self.d[at + 11] = 0x10;
                        self.d[at + 20..at + 22].copy_from_slice(&((target >> 16) as u16).to_le_bytes());
                        self.d[at + 26..at + 28].copy_from_slice(&(target as u16).to_le_bytes());
                    }
                    self.add_entry(dir, part, 0x10, c, 0)?;
                    dir = c;
                }
            }
        }
        Ok(dir)
    }

    /// Writes the file at `path` (folders made as needed), replacing one
    /// already there (its old clusters are freed).
    pub fn file(&mut self, path: &str, data: &[u8]) -> Result<(), WriteError> {
        let size = u32::try_from(data.len()).map_err(|_| WriteError::TooBig(path.to_string()))?;
        let trimmed = path.trim_start_matches(['/', '\\']);
        let (dir_path, name) = trimmed.rsplit_once(['/', '\\']).unwrap_or(("", trimmed));
        let dir = self.folder(dir_path)?;
        if let Some((old, at)) = self.v.list(self.d, dir).into_iter().find(|(e, _)| e.name.eq_ignore_ascii_case(name)) {
            if old.is_dir {
                return Err(WriteError::NotAFolder(path.to_string()));
            }
            // Delete it: free its clusters, mark its records unused.
            for c in self.v.chain(self.d, old.cluster) {
                self.fat_set(c, 0);
            }
            let mut back = at;
            self.d[at] = 0xE5;
            while back >= 32 && self.d.get(back - 32 + 11) == Some(&0x0F) && self.d[back - 32] != 0xE5 {
                back -= 32;
                self.d[back] = 0xE5;
            }
        }
        let cb = self.v.cluster_bytes();
        let first = self.alloc_chain(data.len().div_ceil(cb))?;
        for (c, piece) in self.v.chain(self.d, first).into_iter().zip(data.chunks(cb)) {
            let at = self.v.cluster_at(c);
            self.d[at..at + piece.len()].copy_from_slice(piece);
        }
        self.add_entry(dir, name, 0x20, first, size)?;
        Ok(())
    }

    /// The FSInfo sector's free count and hint become "unknown", so the
    /// firmware recounts instead of trusting stale numbers.
    pub fn finish(self) {
        let at = u16_at(self.d, 48).unwrap_or(1) as usize * self.v.bytes_per_sector;
        if self.d.get(at..at + 4) == Some(b"RRaA") {
            self.d[at + 488..at + 496].copy_from_slice(&[0xFF; 8]);
        }
    }
}

/// Writes every file under `dir` (a folder on the computer standing for
/// the volume's root, F:) onto the volume, folders and all. Returns the
/// number of files written.
pub fn add_host_folder(volume: &mut Vec<u8>, dir: &std::path::Path) -> Result<usize, String> {
    let mut w = Writer::new(volume).map_err(|e| e.to_string())?;
    let mut stack = vec![dir.to_path_buf()];
    let mut files = 0;
    while let Some(d) = stack.pop() {
        let mut entries: Vec<_> = std::fs::read_dir(&d)
            .map_err(|e| format!("reading {}: {e}", d.display()))?
            .flatten()
            .map(|e| e.path())
            .collect();
        entries.sort();
        for p in entries {
            let rel = p.strip_prefix(dir).unwrap_or(&p).to_string_lossy().replace('\\', "/");
            if p.is_dir() {
                w.folder(&rel).map_err(|e| e.to_string())?;
                stack.push(p);
            } else {
                let data = std::fs::read(&p).map_err(|e| format!("reading {}: {e}", p.display()))?;
                w.file(&rel, &data).map_err(|e| format!("{rel}: {e}"))?;
                files += 1;
            }
        }
    }
    w.finish();
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A tiny FAT32 volume: 512-byte sectors, 1 sector per cluster, 1
    /// reserved sector, one 1-sector FAT (126 clusters), root at cluster 2.
    /// The image stops after the root, as the factory's does after its data.
    fn volume(entries: &[[u8; 32]]) -> Vec<u8> {
        let mut d = vec![0u8; 512 * 4];
        d[32..36].copy_from_slice(&(2 + 126u32).to_le_bytes());
        d[11..13].copy_from_slice(&512u16.to_le_bytes());
        d[13] = 1;
        d[14..16].copy_from_slice(&1u16.to_le_bytes());
        d[16] = 1;
        d[36..40].copy_from_slice(&1u32.to_le_bytes());
        d[44..48].copy_from_slice(&2u32.to_le_bytes());
        d[0x52..0x57].copy_from_slice(b"FAT32");
        d[512 + 8..512 + 12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes()); // root: one cluster
        for (i, e) in entries.iter().enumerate() {
            d[1024 + 32 * i..1024 + 32 * (i + 1)].copy_from_slice(e);
        }
        d
    }

    fn short(name: &[u8; 11], attr: u8) -> [u8; 32] {
        let mut e = [0u8; 32];
        e[..11].copy_from_slice(name);
        e[11] = attr;
        e
    }

    fn long(text: &str, ord: u8) -> [u8; 32] {
        let mut e = [0xFFu8; 32];
        e[0] = ord;
        e[11] = 0x0F;
        e[12] = 0;
        e[26] = 0;
        e[27] = 0;
        let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        for (k, &i) in [1usize, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30].iter().enumerate() {
            if let Some(u) = units.get(k) {
                e[i..i + 2].copy_from_slice(&u.to_le_bytes());
            }
        }
        e
    }

    #[test]
    fn files_written_read_back_with_their_long_names() {
        let mut d = volume(&[short(b"SYSTEM     ", 0x10)]);
        let big: Vec<u8> = (0..1300u32).map(|i| (i * 7) as u8).collect();
        {
            let mut w = Writer::new(&mut d).unwrap();
            w.file("Applications/Bundle/main.bin", &big).unwrap();
            w.file("/Applications/Bundle/README", b"hi").unwrap();
            w.file("ldi.dat", b"").unwrap();
            w.finish();
        }
        assert_eq!(read_file(&d, r"applications\bundle\MAIN.BIN").as_deref(), Some(&big[..]), "any case, either slash");
        assert_eq!(read_file(&d, "Applications/Bundle/README").as_deref(), Some(&b"hi"[..]));
        assert_eq!(read_file(&d, "ldi.dat").as_deref(), Some(&b""[..]));
        let root: Vec<String> = list(&d, "").unwrap().into_iter().map(|e| e.name).collect();
        assert_eq!(root, ["SYSTEM", "Applications", "ldi.dat"]);
        let bundle = list(&d, "Applications/Bundle").unwrap();
        assert_eq!(
            bundle.iter().map(|e| (e.name.as_str(), e.size)).collect::<Vec<_>>(),
            [("main.bin", 1300), ("README", 2)]
        );
        // "README" fits 8.3 as it is: no long name was needed for it.
        // Replacing frees the old clusters for the new contents.
        let before = (2..128u32).filter(|&c| u32_at(&d, 512 + 4 * c as usize) == Some(0)).count();
        Writer::new(&mut d).unwrap().file("Applications/Bundle/main.bin", b"short").unwrap();
        let after = (2..128u32).filter(|&c| u32_at(&d, 512 + 4 * c as usize) == Some(0)).count();
        assert_eq!(after, before + 2, "three clusters freed, one used");
        assert_eq!(read_file(&d, "Applications/Bundle/main.bin").as_deref(), Some(&b"short"[..]));
        assert_eq!(list(&d, "Applications/Bundle").unwrap().len(), 2);
    }

    #[test]
    fn a_full_directory_grows_and_a_full_volume_says_so() {
        let mut d = volume(&[]);
        let mut w = Writer::new(&mut d).unwrap();
        // Each long name takes 3 slots (2 records + entry): a 16-slot
        // cluster fills after 5.
        for i in 0..12 {
            w.file(&format!("a fairly long name {i}.txt"), &[i as u8]).unwrap();
        }
        assert_eq!(w.file("x", &vec![0u8; 512 * 200]), Err(WriteError::Full));
        drop(w);
        let names = list(&d, "/").unwrap();
        assert_eq!(names.len(), 12);
        assert_eq!(read_file(&d, "a fairly long name 11.txt").as_deref(), Some(&[11u8][..]));
    }

    /// Writes a real folder onto a real volume and reads every file back:
    /// RIBBIT_FAT_VOLUME=<FileSys> RIBBIT_FAT_FOLDER=<folder> cargo test
    /// --release -p ribbit-twist every_file_reads_back -- --ignored
    #[test]
    #[ignore]
    fn every_file_reads_back() {
        let mut d = std::fs::read(std::env::var("RIBBIT_FAT_VOLUME").unwrap()).unwrap();
        let root = std::path::PathBuf::from(std::env::var("RIBBIT_FAT_FOLDER").unwrap());
        let n = add_host_folder(&mut d, &root).unwrap();
        let mut stack = vec![root.clone()];
        let (mut checked, mut bad) = (0, Vec::new());
        while let Some(dir) = stack.pop() {
            for e in std::fs::read_dir(&dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let rel = p.strip_prefix(&root).unwrap().to_string_lossy().replace('\\', "/");
                checked += 1;
                if read_file(&d, &rel).as_deref() != Some(&std::fs::read(&p).unwrap()[..]) {
                    bad.push(rel);
                }
            }
        }
        println!("{n} written, {checked} checked, {} differ: {:?}", bad.len(), &bad[..bad.len().min(10)]);
        assert!(bad.is_empty());
    }

    #[test]
    fn the_upgrade_marker_is_deleted_with_its_long_name() {
        let mut d = volume(&[
            short(b"APPLIC~1   ", 0x10),
            long("RADE", 0x42),
            long("FIRMWARE_UPG", 0x01),
            short(b"FIRMWA~1   ", 0x20),
            short(b"SYSTEM     ", 0x10),
        ]);
        assert!(delete_root_entry(&mut d, "FIRMWARE_UPGRADE"));
        assert_eq!([d[1024 + 32], d[1024 + 64], d[1024 + 96]], [0xE5; 3]);
        assert_eq!(&d[1024..1031], b"APPLIC~", "others untouched");
        assert_eq!(&d[1024 + 128..1024 + 134], b"SYSTEM");
        assert!(!delete_root_entry(&mut d, "FIRMWARE_UPGRADE"), "gone");
        assert!(delete_root_entry(&mut d, "system"));
    }
}
