//! Writable files: the tablet's own storage (`/Local/0/<product id>/...`, `/sdcard/LF/Bulk/...`,
//! `/data/data/<package>/...`) in a host folder, `Files::root`. A guest path is that folder plus
//! the path's parts (`..` can't leave it). Without a folder every file is missing (ENOENT), as
//! before. Only `sdcard`, `Local` and `data` take writes (Android's root is read-only; PPAD2
//! writes its touchscreen rate to `/error_no_tssample_rate_in_hz` when `/tmp/ts_driver`
//! isn't there). Descriptors from `open`, FILEs from `fopen`/`fdopen` (`stdio.rs` streams with a
//! `fd`), folders from `opendir`. Asset descriptors (`assets.rs`) are another set.

use crate::runtime::{Runtime, Stop};
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;

const ENOENT: u32 = 2;
const EBADF: u32 = 9;
const EEXIST: u32 = 17;
const ENOTDIR: u32 = 20;
const EISDIR: u32 = 21;
const EINVAL: u32 = 22;
const ENOTEMPTY: u32 = 39;
const EROFS: u32 = 30;

/// The top folders that take writes.
const WRITABLE: [&str; 3] = ["sdcard", "Local", "data"];

fn writable(guest: &str) -> bool {
    guest.split('/').find(|p| !p.is_empty() && *p != ".").is_some_and(|top| WRITABLE.contains(&top))
}

// open() flags on ARM.
const O_ACCMODE: u32 = 3;
const O_CREAT: u32 = 0x40;
const O_EXCL: u32 = 0x80;
const O_TRUNC: u32 = 0x200;
const O_APPEND: u32 = 0x400;
const O_DIRECTORY: u32 = 0x4000;

/// bionic's 32-bit ARM `struct stat` (stat64) is 104 bytes; `struct dirent` 280.
const STAT_SIZE: u32 = 104;
const DIRENT_SIZE: u32 = 280;

/// Host files' descriptors start here (asset descriptors at 100).
const FIRST_FD: u32 = 1000;

#[derive(Default)]
pub struct Files {
    /// The host folder that is the tablet's storage, or None (nothing writable).
    pub root: Option<PathBuf>,
    fds: BTreeMap<u32, File>,
    /// Open folders by DIR*: (names, next index, the dirent handed out).
    dirs: BTreeMap<u32, (Vec<(String, bool)>, usize, u32)>,
}

impl Files {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self { root, ..Default::default() }
    }

    /// The host path for a guest path, or None without a folder.
    fn host(&self, guest: &str) -> Option<PathBuf> {
        let mut p = self.root.clone()?;
        let mut depth = 0;
        for part in guest.split('/') {
            match part {
                "" | "." => {}
                ".." => {
                    if depth > 0 {
                        p.pop();
                        depth -= 1;
                    }
                }
                _ => {
                    p.push(part);
                    depth += 1;
                }
            }
        }
        Some(p)
    }

    fn add(&mut self, f: File) -> u32 {
        let fd = (FIRST_FD..).find(|f| !self.fds.contains_key(f)).unwrap();
        self.fds.insert(fd, f);
        fd
    }

    pub fn is_open(&self, fd: u32) -> bool {
        self.fds.contains_key(&fd)
    }

    pub fn read(&mut self, fd: u32, n: usize) -> Option<Vec<u8>> {
        let f = self.fds.get_mut(&fd)?;
        let mut buf = vec![0; n];
        let got = f.read(&mut buf).ok()?;
        buf.truncate(got);
        Some(buf)
    }

    pub fn write(&mut self, fd: u32, data: &[u8]) -> Option<u32> {
        let f = self.fds.get_mut(&fd)?;
        f.write_all(data).ok()?;
        Some(data.len() as u32)
    }

    /// lseek: the new position, or None.
    pub fn seek(&mut self, fd: u32, off: u32, whence: u32) -> Option<u32> {
        let f = self.fds.get_mut(&fd)?;
        let to = match whence {
            0 => SeekFrom::Start(off as u64),
            1 => SeekFrom::Current(off as i32 as i64),
            2 => SeekFrom::End(off as i32 as i64),
            _ => return None,
        };
        f.seek(to).ok().map(|p| p as u32)
    }

    pub fn close(&mut self, fd: u32) -> bool {
        self.fds.remove(&fd).is_some()
    }
}

/// open(2) as OpenOptions; None for flags that make no sense.
fn options(flags: u32) -> Option<OpenOptions> {
    let mut o = OpenOptions::new();
    match flags & O_ACCMODE {
        0 => o.read(true),
        1 => o.write(true),
        2 => o.read(true).write(true),
        _ => return None,
    };
    if flags & O_APPEND != 0 {
        o.append(true);
    }
    if flags & O_TRUNC != 0 && flags & O_ACCMODE != 0 {
        o.truncate(true);
    }
    if flags & O_CREAT != 0 {
        if flags & O_EXCL != 0 {
            o.create_new(true);
        } else {
            o.create(true);
        }
    }
    Some(o)
}

/// fopen's mode as open(2) flags.
fn mode_flags(mode: &str) -> Option<u32> {
    let plus = mode.contains('+');
    let excl = if mode.contains('x') { O_EXCL } else { 0 };
    Some(match mode.chars().next()? {
        'r' => if plus { 2 } else { 0 },
        'w' => (if plus { 2 } else { 1 }) | O_CREAT | O_TRUNC | excl,
        'a' => (if plus { 2 } else { 1 }) | O_CREAT | O_APPEND | excl,
        _ => return None,
    })
}

fn errno_of(e: &std::io::Error) -> u32 {
    use std::io::ErrorKind::*;
    match e.kind() {
        NotFound => ENOENT,
        AlreadyExists => EEXIST,
        NotADirectory => ENOTDIR,
        IsADirectory => EISDIR,
        DirectoryNotEmpty => ENOTEMPTY,
        _ => EINVAL,
    }
}

pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "open" | "fopen" | "fdopen" | "access" | "stat" | "lstat" | "fstat" | "mkdir" | "rmdir" | "unlink" | "remove" | "rename" | "opendir"
            | "readdir" | "closedir" | "close" | "read" | "write" | "lseek" | "ftruncate" | "fsync" | "fileno"
    )
}

impl Runtime {
    pub(crate) fn set_errno(&mut self, e: u32) {
        if self.state.errno_at == 0 {
            self.state.errno_at = self.malloc(4);
        }
        let a = self.state.errno_at;
        self.mem.w32(a, e);
    }

    /// Returns -1 (or NULL) with errno `e`.
    fn fail(&mut self, e: u32, null: bool) {
        self.set_errno(e);
        self.ret(if null { 0 } else { u32::MAX })
    }

    fn write_stat(&mut self, out: u32, m: &std::fs::Metadata, ino: u32) {
        let secs = |t: std::io::Result<std::time::SystemTime>| {
            t.ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs() as u32)
        };
        let size = if m.is_dir() { 4096 } else { m.len() };
        self.mem.write(out, &[0; STAT_SIZE as usize]);
        self.mem.w32(out + 12, ino);
        self.mem.w32(out + 16, if m.is_dir() { 0o040777 } else { 0o100666 });
        self.mem.w32(out + 20, 1);
        self.mem.w32(out + 48, size as u32);
        self.mem.w32(out + 52, (size >> 32) as u32);
        self.mem.w32(out + 56, 4096);
        self.mem.w32(out + 64, size.div_ceil(512) as u32);
        self.mem.w32(out + 72, secs(m.accessed()));
        self.mem.w32(out + 80, secs(m.modified()));
        self.mem.w32(out + 88, secs(m.modified()));
        self.mem.w32(out + 96, ino);
    }

    /// A guest path argument, logged with LFA_LOG.
    fn path_arg(&mut self, i: usize, name: &str) -> (String, Option<PathBuf>) {
        let a = self.arg(i);
        let p = self.mem.cstr(a);
        let host = self.files.host(&p);
        if std::env::var_os("LFA_LOG").is_some() {
            let found = host.as_ref().is_some_and(|h| h.exists());
            eprintln!("[file] {name} {p}{} (from {:#x})", if found { "" } else { ": not there" }, self.cpu.r[14]);
        }
        (p, host)
    }

    /// Opens a host file; its descriptor, or None with errno set.
    fn open_host(&mut self, guest: &str, host: Option<PathBuf>, flags: u32) -> Option<u32> {
        let Some(host) = host else {
            self.set_errno(ENOENT);
            return None;
        };
        if flags & O_ACCMODE != 0 && !writable(guest) {
            self.set_errno(if host.exists() { EROFS } else { ENOENT });
            return None;
        }
        if host.is_dir() {
            self.set_errno(if flags & O_DIRECTORY != 0 { ENOTDIR } else { EISDIR });
            return None;
        }
        let Some(o) = options(flags) else {
            self.set_errno(EINVAL);
            return None;
        };
        match o.open(&host) {
            Ok(f) => Some(self.files.add(f)),
            Err(e) => {
                self.set_errno(errno_of(&e));
                None
            }
        }
    }
}

/// A stable inode number for a path (some code compares them).
fn ino(p: &str) -> u32 {
    p.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193)) | 1
}

pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    match name {
        // open(path, flags, mode)
        "open" => {
            let flags = rt.arg(1);
            let (p, host) = rt.path_arg(0, name);
            match rt.open_host(&p, host, flags) {
                Some(fd) => rt.ret(fd),
                None => rt.ret(u32::MAX),
            }
        }
        // fopen(path, mode)
        "fopen" => {
            let m = rt.arg(1);
            let mode = rt.mem.cstr(m);
            let (p, host) = rt.path_arg(0, name);
            let Some(flags) = mode_flags(&mode) else { return Ok(rt.fail(EINVAL, true)) };
            match rt.open_host(&p, host, flags) {
                Some(fd) => {
                    let fp = rt.host_stream(fd);
                    rt.ret(fp)
                }
                None => rt.ret(0),
            }
        }
        // fdopen(fd, mode)
        "fdopen" => {
            let fd = rt.arg(0);
            if rt.files.is_open(fd) {
                let fp = rt.host_stream(fd);
                rt.ret(fp)
            } else {
                rt.fail(EBADF, true)
            }
        }
        // access(path, mode): only whether it's there
        "access" => {
            let (_, host) = rt.path_arg(0, name);
            if host.is_some_and(|h| h.exists()) {
                rt.ret(0)
            } else {
                rt.fail(ENOENT, false)
            }
        }
        // stat(path, out), lstat
        "stat" | "lstat" => {
            let out = rt.arg(1);
            let (p, host) = rt.path_arg(0, name);
            match host.map(std::fs::metadata) {
                Some(Ok(m)) => {
                    rt.write_stat(out, &m, ino(&p));
                    rt.ret(0)
                }
                Some(Err(e)) => rt.fail(errno_of(&e), false),
                None => rt.fail(ENOENT, false),
            }
        }
        // fstat(fd, out): host files and asset descriptors
        "fstat" => {
            let (fd, out) = (rt.arg(0), rt.arg(1));
            if let Some(m) = rt.files.fds.get(&fd).and_then(|f| f.metadata().ok()) {
                rt.write_stat(out, &m, fd);
                rt.ret(0)
            } else if let Some(n) = rt.assets.fds.get(&fd).map(Vec::len) {
                rt.mem.write(out, &[0; STAT_SIZE as usize]);
                rt.mem.w32(out + 16, 0o100444);
                rt.mem.w32(out + 20, 1);
                rt.mem.w32(out + 48, n as u32);
                rt.mem.w32(out + 56, 4096);
                rt.ret(0)
            } else {
                rt.fail(EBADF, false)
            }
        }
        "mkdir" => {
            let (p, host) = rt.path_arg(0, name);
            if !writable(&p) {
                let e = if host.is_some_and(|h| h.exists()) { EEXIST } else { EROFS };
                return Ok(rt.fail(e, false));
            }
            match host.map(std::fs::create_dir) {
                Some(Ok(())) => rt.ret(0),
                Some(Err(e)) => rt.fail(errno_of(&e), false),
                None => rt.fail(ENOENT, false),
            }
        }
        "rmdir" | "unlink" | "remove" => {
            let (p, host) = rt.path_arg(0, name);
            if !writable(&p) {
                return Ok(rt.fail(EROFS, false));
            }
            let r = host.map(|h| if h.is_dir() && name != "unlink" { std::fs::remove_dir(h) } else { std::fs::remove_file(h) });
            match r {
                Some(Ok(())) => rt.ret(0),
                Some(Err(e)) => rt.fail(errno_of(&e), false),
                None => rt.fail(ENOENT, false),
            }
        }
        // rename(from, to): replaces `to`
        "rename" => {
            let (pf, from) = rt.path_arg(0, name);
            let (pt, to) = rt.path_arg(1, name);
            if !writable(&pf) || !writable(&pt) {
                return Ok(rt.fail(EROFS, false));
            }
            match from.zip(to).map(|(f, t)| std::fs::rename(f, t)) {
                Some(Ok(())) => rt.ret(0),
                Some(Err(e)) => rt.fail(errno_of(&e), false),
                None => rt.fail(ENOENT, false),
            }
        }
        // opendir(path): a DIR* over its names, "." and ".." first
        "opendir" => {
            let (_, host) = rt.path_arg(0, name);
            let list = host.map(|h| std::fs::read_dir(h).map(|d| d.flatten().map(|e| (e.file_name().to_string_lossy().into_owned(), e.path().is_dir())).collect::<Vec<_>>()));
            match list {
                Some(Ok(mut names)) => {
                    names.sort();
                    names.splice(0..0, [(".".to_string(), true), ("..".to_string(), true)]);
                    let d = rt.malloc(16);
                    let ent = rt.malloc(DIRENT_SIZE);
                    rt.files.dirs.insert(d, (names, 0, ent));
                    rt.ret(d)
                }
                Some(Err(e)) => rt.fail(errno_of(&e), true),
                None => rt.fail(ENOENT, true),
            }
        }
        // readdir(dir): d_ino 0, d_off 8, d_reclen 16, d_type 18 (4 folder, 8 file), d_name 19
        "readdir" => {
            let d = rt.arg(0);
            let next = rt.files.dirs.get_mut(&d).and_then(|(names, i, ent)| {
                let e = names.get(*i).cloned()?;
                *i += 1;
                Some((e, *i, *ent))
            });
            match next {
                Some(((n, dir), i, ent)) => {
                    rt.mem.write(ent, &[0; DIRENT_SIZE as usize]);
                    rt.mem.w32(ent, ino(&n));
                    rt.mem.w32(ent + 8, i as u32);
                    rt.mem.w16(ent + 16, DIRENT_SIZE as u16);
                    rt.mem.w8(ent + 18, if dir { 4 } else { 8 });
                    let b = n.as_bytes();
                    rt.mem.write(ent + 19, &b[..b.len().min(255)]);
                    rt.ret(ent)
                }
                None => rt.ret(0),
            }
        }
        "closedir" => {
            let d = rt.arg(0);
            if let Some((_, _, ent)) = rt.files.dirs.remove(&d) {
                rt.free(ent);
                rt.free(d);
            }
            rt.ret(0)
        }
        "close" => {
            let fd = rt.arg(0);
            rt.assets.fds.remove(&fd);
            rt.files.close(fd);
            rt.ret(0)
        }
        // read(fd, buf, n)
        "read" => {
            let (fd, buf, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            match rt.files.read(fd, n as usize) {
                Some(data) => {
                    rt.mem.write(buf, &data);
                    rt.ret(data.len() as u32)
                }
                None => rt.fail(EBADF, false),
            }
        }
        // write(fd, buf, n): stdout and stderr go to the log
        "write" => {
            let (fd, buf, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let data = rt.mem.read(buf, n as usize);
            if fd == 1 || fd == 2 {
                crate::imports::log_line(rt, String::from_utf8_lossy(&data).into_owned());
                rt.ret(n)
            } else {
                match rt.files.write(fd, &data) {
                    Some(n) => rt.ret(n),
                    None => rt.fail(EBADF, false),
                }
            }
        }
        "lseek" => {
            let (fd, off, whence) = (rt.arg(0), rt.arg(1), rt.arg(2));
            match rt.files.seek(fd, off, whence) {
                Some(p) => rt.ret(p),
                None => rt.fail(EBADF, false),
            }
        }
        // ftruncate(fd, length)
        "ftruncate" => {
            let (fd, len) = (rt.arg(0), rt.arg(1));
            match rt.files.fds.get(&fd).map(|f| f.set_len(len as u64)) {
                Some(Ok(())) => rt.ret(0),
                _ => rt.fail(EBADF, false),
            }
        }
        "fsync" => {
            let fd = rt.arg(0);
            if let Some(f) = rt.files.fds.get(&fd) {
                let _ = f.sync_all();
            }
            rt.ret(0)
        }
        // fileno(fp): a host FILE's descriptor
        "fileno" => {
            let fp = rt.arg(0);
            match rt.stream_fd(fp) {
                Some(fd) => rt.ret(fd),
                None => rt.fail(EBADF, false),
            }
        }
        _ => return Err(Stop::Import(name.to_string(), rt.cpu.r[14])),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_stay_in_the_folder() {
        let f = Files::new(Some(PathBuf::from("root")));
        assert_eq!(f.host("/Local/0/0x00370009/SAVE.DAT"), Some(PathBuf::from("root/Local/0/0x00370009/SAVE.DAT")));
        assert_eq!(f.host("sdcard//LF/./Bulk/"), Some(PathBuf::from("root/sdcard/LF/Bulk")));
        assert_eq!(f.host("/../../x/../y"), Some(PathBuf::from("root/y")));
        assert_eq!(Files::default().host("/Local"), None);
    }

    #[test]
    fn writes_only_in_storage() {
        assert!(writable("/Local/0/0x0020001D/MySave7.sav"));
        assert!(writable("sdcard/LF/Bulk"));
        assert!(!writable("/error_no_tssample_rate_in_hz"));
        assert!(!writable("/tmp/ts_driver"));
        assert_eq!(mode_flags("rb"), Some(0));
        assert_eq!(mode_flags("wb"), Some(1 | O_CREAT | O_TRUNC));
        assert_eq!(mode_flags("a+"), Some(2 | O_CREAT | O_APPEND));
    }
}
