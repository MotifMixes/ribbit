//! Images inside .zip archives: the archive's ROM file is read as if it
//! were the file itself. Only what ROM zips use: stored or deflated entries,
//! no encryption, no Zip64 (images are at most a few MB).

use thiserror::Error;

/// Extensions taken as the image inside an archive, best first.
const IMAGE_EXTENSIONS: [&str; 4] = ["bin", "rom", "u3", "lst"];
/// Nothing a Leapster uses is bigger.
const MAX_IMAGE: usize = 64 << 20;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ZipError {
    #[error("damaged zip archive ({0})")]
    Damaged(&'static str),
    #[error("the zip archive has no ROM file in it")]
    NoImage,
    #[error("the ROM in the zip archive is compressed in a way Ribbit does not read (method {0})")]
    Method(u16),
    #[error("the ROM in the zip archive fails its checksum")]
    Checksum,
}

/// Whether `data` is a zip archive.
pub fn is_zip(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06")
}

/// `data` itself, or the ROM inside it if it is a zip archive.
pub fn unzip_image(data: Vec<u8>) -> Result<Vec<u8>, ZipError> {
    if is_zip(&data) { extract_image(&data) } else { Ok(data) }
}

fn u16_at(d: &[u8], at: usize) -> Result<u16, ZipError> {
    d.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or(ZipError::Damaged("truncated"))
}

fn u32_at(d: &[u8], at: usize) -> Result<u32, ZipError> {
    d.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or(ZipError::Damaged("truncated"))
}

struct Entry {
    name: String,
    method: u16,
    crc: u32,
    packed: usize,
    size: usize,
    local: usize,
}

/// The ROM file in a zip archive: the largest entry with a ROM extension,
/// else the largest file.
pub fn extract_image(d: &[u8]) -> Result<Vec<u8>, ZipError> {
    // End of central directory: the last 22+ bytes (plus a comment).
    let from = d.len().saturating_sub(22 + 0xFFFF);
    let eocd = (from..=d.len().saturating_sub(22))
        .rev()
        .find(|&i| d[i..].starts_with(b"PK\x05\x06"))
        .ok_or(ZipError::Damaged("no directory"))?;
    let count = u16_at(d, eocd + 10)? as usize;
    let mut at = u32_at(d, eocd + 16)? as usize;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        if u32_at(d, at)? != 0x0201_4B50 {
            return Err(ZipError::Damaged("bad directory entry"));
        }
        let name_len = u16_at(d, at + 28)? as usize;
        let skip = name_len + u16_at(d, at + 30)? as usize + u16_at(d, at + 32)? as usize;
        let name = d.get(at + 46..at + 46 + name_len).ok_or(ZipError::Damaged("truncated"))?;
        entries.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method: u16_at(d, at + 10)?,
            crc: u32_at(d, at + 16)?,
            packed: u32_at(d, at + 20)? as usize,
            size: u32_at(d, at + 24)? as usize,
            local: u32_at(d, at + 42)? as usize,
        });
        at += 46 + skip;
    }
    let is_image = |e: &Entry| {
        let ext = e.name.rsplit_once('.').map_or("", |(_, x)| x);
        IMAGE_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(ext))
    };
    let files = entries.iter().filter(|e| !e.name.ends_with('/') && e.size > 0);
    let entry = files
        .clone()
        .filter(|e| is_image(e))
        .max_by_key(|e| e.size)
        .or_else(|| files.max_by_key(|e| e.size))
        .ok_or(ZipError::NoImage)?;
    if entry.size > MAX_IMAGE {
        return Err(ZipError::Damaged("entry too large"));
    }
    if u32_at(d, entry.local)? != 0x0403_4B50 {
        return Err(ZipError::Damaged("bad file header"));
    }
    let start = entry.local + 30 + u16_at(d, entry.local + 26)? as usize + u16_at(d, entry.local + 28)? as usize;
    let packed = d.get(start..start + entry.packed).ok_or(ZipError::Damaged("truncated"))?;
    let data = match entry.method {
        0 => packed.to_vec(),
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(packed, entry.size)
            .map_err(|_| ZipError::Damaged("bad compressed data"))?,
        m => return Err(ZipError::Method(m)),
    };
    if data.len() != entry.size || crc32fast::hash(&data) != entry.crc {
        return Err(ZipError::Checksum);
    }
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A minimal archive: `files` as (name, data, deflate?).
    fn zip(files: &[(&str, &[u8], bool)]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut dir = Vec::new();
        for (name, data, deflate) in files {
            let packed = if *deflate { miniz_oxide::deflate::compress_to_vec(data, 6) } else { data.to_vec() };
            let method: u16 = if *deflate { 8 } else { 0 };
            let crc = crc32fast::hash(data);
            let local = out.len() as u32;
            out.extend_from_slice(b"PK\x03\x04");
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 4]);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&packed);
            dir.extend_from_slice(b"PK\x01\x02");
            dir.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            dir.extend_from_slice(&method.to_le_bytes());
            dir.extend_from_slice(&[0; 4]);
            dir.extend_from_slice(&crc.to_le_bytes());
            dir.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(data.len() as u32).to_le_bytes());
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&local.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let dir_at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06");
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn reads_stored_and_deflated_roms() {
        let rom: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        for deflate in [false, true] {
            assert_eq!(unzip_image(zip(&[("game.bin", &rom, deflate)])).unwrap(), rom);
        }
    }

    #[test]
    fn picks_the_rom_over_other_files() {
        let rom = vec![0x5A; 300];
        let readme = vec![b'x'; 900];
        let z = zip(&[("readme.txt", &readme, true), ("Game (USA).bin", &rom, true)]);
        assert_eq!(unzip_image(z).unwrap(), rom);
    }

    #[test]
    fn plain_files_pass_through_and_damage_is_caught() {
        assert_eq!(unzip_image(vec![1, 2, 3]).unwrap(), vec![1, 2, 3]);
        let mut z = zip(&[("game.bin", &[9; 64], false)]);
        let at = z.windows(4).position(|w| w == [9, 9, 9, 9]).unwrap();
        z[at] = 8;
        assert_eq!(unzip_image(z), Err(ZipError::Checksum));
        assert!(matches!(unzip_image(b"PK\x03\x04 not really".to_vec()), Err(ZipError::Damaged(_))));
    }
}
