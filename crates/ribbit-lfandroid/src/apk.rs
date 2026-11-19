//! Minimal read-only ZIP reader for APKs (stored and deflated entries only).
//! The file is never extracted to disk; entries are read into memory on request.

use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub compressed: u32,
    pub size: u32,
    local_offset: u32,
}

pub struct Apk {
    data: Vec<u8>,
    pub entries: BTreeMap<String, Entry>,
}

fn u16_at(d: &[u8], o: usize) -> Option<u16> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}
fn u32_at(d: &[u8], o: usize) -> Option<u32> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

impl Apk {
    pub fn open(data: Vec<u8>) -> Result<Self, String> {
        // End of central directory: last 22..(22 + 65535) bytes.
        let min = data.len().saturating_sub(22 + 0xFFFF);
        let eocd = (min..=data.len().saturating_sub(22))
            .rev()
            .find(|&o| u32_at(&data, o) == Some(0x0605_4b50))
            .ok_or("no ZIP end-of-central-directory record")?;
        let count = u16_at(&data, eocd + 10).ok_or("short EOCD")? as usize;
        let mut at = u32_at(&data, eocd + 16).ok_or("short EOCD")? as usize;
        let mut entries = BTreeMap::new();
        for _ in 0..count {
            if u32_at(&data, at) != Some(0x0201_4b50) {
                return Err(format!("bad central directory entry at {at:#x}"));
            }
            let method = u16_at(&data, at + 10).ok_or("short")?;
            let compressed = u32_at(&data, at + 20).ok_or("short")?;
            let size = u32_at(&data, at + 24).ok_or("short")?;
            let nlen = u16_at(&data, at + 28).ok_or("short")? as usize;
            let xlen = u16_at(&data, at + 30).ok_or("short")? as usize;
            let clen = u16_at(&data, at + 32).ok_or("short")? as usize;
            let local_offset = u32_at(&data, at + 42).ok_or("short")?;
            let name = String::from_utf8_lossy(data.get(at + 46..at + 46 + nlen).ok_or("short name")?).into_owned();
            entries.insert(name.clone(), Entry { name, method, compressed, size, local_offset });
            at += 46 + nlen + xlen + clen;
        }
        Ok(Apk { data, entries })
    }

    pub fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        let e = self.entries.get(name).ok_or_else(|| format!("{name}: not in archive"))?;
        let lo = e.local_offset as usize;
        if u32_at(&self.data, lo) != Some(0x0403_4b50) {
            return Err(format!("{name}: bad local header"));
        }
        let nlen = u16_at(&self.data, lo + 26).ok_or("short")? as usize;
        let xlen = u16_at(&self.data, lo + 28).ok_or("short")? as usize;
        let start = lo + 30 + nlen + xlen;
        let raw = self.data.get(start..start + e.compressed as usize).ok_or("entry past end of file")?;
        match e.method {
            0 => Ok(raw.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec(raw).map_err(|err| format!("{name}: inflate: {err:?}")),
            m => Err(format!("{name}: compression method {m} not supported")),
        }
    }
}

/// An APK's entry names and the bytes of one entry, read from the file without loading the
/// whole archive (titles are up to a few hundred MB): the end record and central directory
/// from the file's end, then that one entry. For the library's scan.
pub fn peek_file(path: &std::path::Path, entry: &str) -> Result<(Vec<String>, Vec<u8>), String> {
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let len = f.metadata().map_err(|e| e.to_string())?.len();
    let tail_len = len.min(22 + 0xFFFF);
    let mut tail = vec![0; tail_len as usize];
    f.seek(SeekFrom::Start(len - tail_len)).map_err(|e| e.to_string())?;
    f.read_exact(&mut tail).map_err(|e| e.to_string())?;
    let eocd = (0..=tail.len().saturating_sub(22)).rev().find(|&o| u32_at(&tail, o) == Some(0x0605_4b50)).ok_or("no ZIP end-of-central-directory record")?;
    let count = u16_at(&tail, eocd + 10).ok_or("short EOCD")? as usize;
    let dir_size = u32_at(&tail, eocd + 12).ok_or("short EOCD")? as u64;
    let dir_at = u32_at(&tail, eocd + 16).ok_or("short EOCD")? as u64;
    if dir_at + dir_size > len || dir_size > 64 << 20 {
        return Err("bad central directory".into());
    }
    let mut dir = vec![0; dir_size as usize];
    f.seek(SeekFrom::Start(dir_at)).map_err(|e| e.to_string())?;
    f.read_exact(&mut dir).map_err(|e| e.to_string())?;
    let (mut names, mut found) = (Vec::with_capacity(count), None);
    let mut at = 0;
    for _ in 0..count {
        if u32_at(&dir, at) != Some(0x0201_4b50) {
            return Err(format!("bad central directory entry at {at:#x}"));
        }
        let method = u16_at(&dir, at + 10).ok_or("short")?;
        let compressed = u32_at(&dir, at + 20).ok_or("short")?;
        let nlen = u16_at(&dir, at + 28).ok_or("short")? as usize;
        let xlen = u16_at(&dir, at + 30).ok_or("short")? as usize;
        let clen = u16_at(&dir, at + 32).ok_or("short")? as usize;
        let local = u32_at(&dir, at + 42).ok_or("short")?;
        let name = String::from_utf8_lossy(dir.get(at + 46..at + 46 + nlen).ok_or("short name")?).into_owned();
        if name == entry {
            found = Some((method, compressed, local));
        }
        names.push(name);
        at += 46 + nlen + xlen + clen;
    }
    let (method, compressed, local) = found.ok_or_else(|| format!("{entry}: not in archive"))?;
    let mut head = [0u8; 30];
    f.seek(SeekFrom::Start(local as u64)).map_err(|e| e.to_string())?;
    f.read_exact(&mut head).map_err(|e| e.to_string())?;
    if u32_at(&head, 0) != Some(0x0403_4b50) || compressed > 16 << 20 {
        return Err(format!("{entry}: bad local header"));
    }
    let skip = u16_at(&head, 26).ok_or("short")? as i64 + u16_at(&head, 28).ok_or("short")? as i64;
    f.seek(SeekFrom::Current(skip)).map_err(|e| e.to_string())?;
    let mut raw = vec![0; compressed as usize];
    f.read_exact(&mut raw).map_err(|e| e.to_string())?;
    let data = match method {
        0 => raw,
        8 => miniz_oxide::inflate::decompress_to_vec(&raw).map_err(|err| format!("{entry}: inflate: {err:?}"))?,
        m => return Err(format!("{entry}: compression method {m} not supported")),
    };
    Ok((names, data))
}

/// Reads the UTF-16 strings out of a binary AndroidManifest.xml string pool, in order.
/// Enough to find the package name, `android.app.lib_name` value and LeapFrog meta-data.
pub fn manifest_strings(axml: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // Chunk 0x0001 (string pool) follows the 8-byte XML header.
    let Some(kind) = u16_at(axml, 8) else { return out };
    if kind != 0x0001 {
        return out;
    }
    let count = u32_at(axml, 16).unwrap_or(0) as usize;
    let flags = u32_at(axml, 24).unwrap_or(0);
    let strings_start = u32_at(axml, 28).unwrap_or(0) as usize + 8;
    let utf8 = flags & 0x100 != 0;
    for i in 0..count.min(4096) {
        let Some(off) = u32_at(axml, 36 + i * 4) else { break };
        let p = strings_start + off as usize;
        if utf8 {
            // u8 utf16len (1-2 bytes), u8 utf8len (1-2 bytes), bytes
            let mut q = p;
            let skip = |q: &mut usize| {
                if axml.get(*q).copied().unwrap_or(0) & 0x80 != 0 { *q += 2 } else { *q += 1 }
            };
            skip(&mut q);
            let mut len = axml.get(q).copied().unwrap_or(0) as usize;
            if len & 0x80 != 0 {
                len = ((len & 0x7F) << 8) | axml.get(q + 1).copied().unwrap_or(0) as usize;
                q += 2;
            } else {
                q += 1;
            }
            out.push(String::from_utf8_lossy(axml.get(q..q + len).unwrap_or(&[])).into_owned());
        } else {
            let len = u16_at(axml, p).unwrap_or(0) as usize;
            let units: Vec<u16> = (0..len).filter_map(|k| u16_at(axml, p + 2 + k * 2)).collect();
            out.push(String::from_utf16_lossy(&units));
        }
    }
    out
}

/// The `android:screenOrientation` value of the activity named `activity` (its name as
/// written, e.g. `.LFMainActivity`), from the binary manifest's start-element chunks.
pub fn activity_orientation(axml: &[u8], activity: &str) -> Option<u32> {
    let strings = manifest_strings(axml);
    let mut off = 8;
    while let (Some(kind), Some(size)) = (u16_at(axml, off), u32_at(axml, off + 4)) {
        if size < 8 {
            break;
        }
        // 0x0102: start element; attributes (20 bytes each) follow its 36-byte header.
        if kind == 0x0102 && strings.get(u32_at(axml, off + 20)? as usize).is_some_and(|s| s == "activity") {
            let (mut name, mut orientation) = (None, None);
            for k in 0..u16_at(axml, off + 28)? as usize {
                let a = off + 36 + k * 20;
                let attr = strings.get(u32_at(axml, a + 4)? as usize).map(String::as_str);
                let raw = u32_at(axml, a + 8)?;
                match attr {
                    Some("name") => name = strings.get(raw as usize).cloned(),
                    Some("screenOrientation") => orientation = u32_at(axml, a + 16),
                    _ => {}
                }
            }
            if name.as_deref() == Some(activity) {
                return orientation;
            }
        }
        off += size as usize;
    }
    None
}
