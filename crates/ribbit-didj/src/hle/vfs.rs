//! The HLE's file system: one tree in memory, filled from the root file
//! system image (erootfs.jffs2) and whatever else is put in (/Didj, /tmp).
//! Paths are kept canonical: absolute, no "." or "..", no trailing '/'.

use std::collections::{BTreeMap, VecDeque};

pub const S_IFMT: u32 = 0o170000;
pub const S_IFDIR: u32 = 0o040000;
pub const S_IFREG: u32 = 0o100000;
pub const S_IFLNK: u32 = 0o120000;
pub const S_IFCHR: u32 = 0o020000;

pub const ENOENT: i32 = 2;
pub const ENOTDIR: i32 = 20;
pub const ELOOP: i32 = 40;

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum Data {
    Dir,
    File(Vec<u8>),
    Link(String),
    /// A device node: (major << 8) | minor.
    Dev(u32),
}

#[derive(Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Node {
    pub ino: u32,
    /// st_mode: type and permissions.
    pub mode: u32,
    pub mtime: u32,
    pub data: Data,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        matches!(self.data, Data::Dir)
    }

    pub fn size(&self) -> u64 {
        match &self.data {
            Data::File(d) => d.len() as u64,
            Data::Link(t) => t.len() as u64,
            Data::Dir => 4096,
            Data::Dev(_) => 0,
        }
    }
}

#[derive(Default, Clone)]
pub struct Vfs {
    pub(super) nodes: BTreeMap<String, Node>,
    pub(super) next_ino: u32,
}

/// `parent` joined with `name` ("/" + name at the root).
fn join(parent: &str, name: &str) -> String {
    if parent == "/" { format!("/{name}") } else { format!("{parent}/{name}") }
}

/// The directory holding `path` and the last component.
pub fn split(path: &str) -> (&str, &str) {
    match path.rfind('/') {
        Some(0) => ("/", &path[1..]),
        Some(i) => (&path[..i], &path[i + 1..]),
        None => ("/", path),
    }
}

impl Vfs {
    pub fn new() -> Self {
        let mut v = Self { nodes: BTreeMap::new(), next_ino: 1 };
        v.insert("/", S_IFDIR | 0o755, Data::Dir);
        v
    }

    /// The tree in a JFFS2 image (the root file system).
    pub fn from_jffs2(image: &[u8]) -> Self {
        let mut v = Self::new();
        for e in super::super::jffs2::tree(image) {
            let data = match e.mode & S_IFMT {
                S_IFDIR => Data::Dir,
                S_IFREG => Data::File(e.data),
                S_IFLNK => Data::Link(String::from_utf8_lossy(&e.data).into_owned()),
                _ => Data::Dev(match e.data.len() {
                    2 => u16::from_le_bytes([e.data[0], e.data[1]]) as u32,
                    4 => u32::from_le_bytes(e.data[..4].try_into().unwrap()),
                    _ => 0,
                }),
            };
            v.insert(&e.path, e.mode, data);
            if let Some(n) = v.nodes.get_mut(&e.path) {
                n.mtime = e.mtime;
            }
        }
        v
    }

    /// Puts a node at canonical `path` (its parent must exist), replacing
    /// what was there.
    pub fn insert(&mut self, path: &str, mode: u32, data: Data) {
        let ino = self.next_ino;
        self.next_ino += 1;
        self.nodes.insert(path.to_string(), Node { ino, mode, mtime: 0, data });
    }

    /// A file at canonical `path`, making its missing parent directories.
    pub fn add_file(&mut self, path: &str, data: Vec<u8>) {
        let (dir, _) = split(path);
        self.mkdir_all(dir);
        self.insert(path, S_IFREG | 0o644, Data::File(data));
    }

    pub fn mkdir_all(&mut self, path: &str) {
        if path == "/" || self.nodes.contains_key(path) {
            return;
        }
        self.mkdir_all(split(path).0);
        self.insert(path, S_IFDIR | 0o755, Data::Dir);
    }

    pub fn get(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path)
    }

    pub fn get_mut(&mut self, path: &str) -> Option<&mut Node> {
        self.nodes.get_mut(path)
    }

    pub fn remove(&mut self, path: &str) -> Option<Node> {
        self.nodes.remove(path)
    }

    /// Moves `from` (and everything under it) to `to`, which must be free.
    pub fn rename(&mut self, from: &str, to: &str) {
        let prefix = format!("{from}/");
        let moved: Vec<String> = std::iter::once(from.to_string())
            .chain(self.nodes.range(prefix.clone()..).take_while(|(p, _)| p.starts_with(&prefix)).map(|(p, _)| p.clone()))
            .collect();
        for p in moved {
            if let Some(n) = self.nodes.remove(&p) {
                self.nodes.insert(format!("{to}{}", &p[from.len()..]), n);
            }
        }
    }

    /// The entries of directory `dir`, in name order.
    pub fn list(&self, dir: &str) -> Vec<(String, &Node)> {
        let prefix = if dir == "/" { "/".to_string() } else { format!("{dir}/") };
        self.nodes
            .range(prefix.clone()..)
            .take_while(|(p, _)| p.starts_with(&prefix))
            .filter(|(p, _)| p.len() > prefix.len() && !p[prefix.len()..].contains('/'))
            .map(|(p, n)| (p[prefix.len()..].to_string(), n))
            .collect()
    }

    /// Every file anywhere under `dir`: (path from `dir`, starting with
    /// '/', contents).
    pub fn files_under(&self, dir: &str) -> Vec<(String, &[u8])> {
        let prefix = format!("{}/", dir.trim_end_matches('/'));
        self.nodes
            .range(prefix.clone()..)
            .take_while(|(p, _)| p.starts_with(&prefix))
            .filter_map(|(p, n)| match &n.data {
                Data::File(d) => Some((p[prefix.len() - 1..].to_string(), &d[..])),
                _ => None,
            })
            .collect()
    }

    /// The canonical path `path` names, from directory `cwd`, following
    /// symbolic links on the way (and in the last component if `follow`).
    /// A missing last component is still named (for creating it); a
    /// missing directory on the way is ENOENT.
    pub fn resolve(&self, cwd: &str, path: &str, follow: bool) -> Result<String, i32> {
        if path.is_empty() {
            return Err(ENOENT);
        }
        let mut parts: VecDeque<String> = path.split('/').filter(|s| !s.is_empty()).map(String::from).collect();
        let mut cur = if path.starts_with('/') { "/".to_string() } else { cwd.to_string() };
        let mut links = 0;
        while let Some(name) = parts.pop_front() {
            match name.as_str() {
                "." => continue,
                ".." => {
                    cur = split(&cur).0.to_string();
                    continue;
                }
                _ => {}
            }
            let mut next = join(&cur, &name);
            let last = parts.is_empty();
            if !self.nodes.contains_key(&next)
                && let Some(found) = self.fat_match(&cur, &name)
            {
                next = found;
            }
            match self.nodes.get(&next) {
                None if last => return Ok(next),
                None => return Err(ENOENT),
                Some(Node { data: Data::Link(target), .. }) if !last || follow => {
                    links += 1;
                    if links > 40 {
                        return Err(ELOOP);
                    }
                    if target.starts_with('/') {
                        cur = "/".into();
                    }
                    for p in target.split('/').filter(|s| !s.is_empty()).rev() {
                        parts.push_front(p.to_string());
                    }
                }
                Some(n) if !last && !n.is_dir() => return Err(ENOTDIR),
                Some(_) => cur = next,
            }
        }
        Ok(cur)
    }

    /// /Didj and /Cart are vfat on the console, so names there match in
    /// any case: the entry of `dir` named `name` that way.
    fn fat_match(&self, dir: &str, name: &str) -> Option<String> {
        let fat = |root: &str| dir == root || dir.starts_with(&format!("{root}/"));
        if !fat("/Didj") && !fat("/Cart") {
            return None;
        }
        self.list(dir).into_iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(n, _)| join(dir, &n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_links_and_dots() {
        let mut v = Vfs::new();
        v.add_file("/lib/ld-uClibc-0.9.29.so", b"ld".to_vec());
        v.insert("/lib/ld-uClibc.so.0", S_IFLNK | 0o777, Data::Link("ld-uClibc-0.9.29.so".into()));
        v.mkdir_all("/usr");
        v.insert("/usr/lib", S_IFLNK | 0o777, Data::Link("/lib".into()));
        assert_eq!(v.resolve("/", "/usr/lib/ld-uClibc.so.0", true), Ok("/lib/ld-uClibc-0.9.29.so".into()));
        assert_eq!(v.resolve("/", "/usr/lib/ld-uClibc.so.0", false), Ok("/lib/ld-uClibc.so.0".into()));
        assert_eq!(v.resolve("/usr", "../lib/./new", true), Ok("/lib/new".into()));
        assert_eq!(v.resolve("/", "/nope/x", true), Err(ENOENT));
        assert_eq!(v.resolve("/", "/lib/ld-uClibc-0.9.29.so/x", true), Err(ENOTDIR));
        let names: Vec<String> = v.list("/lib").into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["ld-uClibc-0.9.29.so", "ld-uClibc.so.0"]);
        assert_eq!(v.list("/").len(), 2);
        v.rename("/lib", "/lib2");
        assert!(v.get("/lib2/ld-uClibc.so.0").is_some() && v.get("/lib").is_none());
        assert_eq!(v.resolve("/", "/usr/lib/x", true), Err(ENOENT));
        v.rename("/lib", "/lib2");
        assert!(v.get("/lib2/ld-uClibc.so.0").is_some() && v.get("/lib").is_none());
        assert_eq!(v.resolve("/", "/usr/lib/x", true), Err(ENOENT));
    }

    #[test]
    fn fat_volumes_match_any_case() {
        let mut v = Vfs::new();
        v.add_file("/Didj/Data/0/jdi/Profile.ini", b"x".to_vec());
        v.add_file("/etc/jdi", b"x".to_vec());
        assert_eq!(v.resolve("/", "/Didj/Data/0/JDI/PROFILE.INI", true), Ok("/Didj/Data/0/jdi/Profile.ini".into()));
        assert_eq!(v.resolve("/", "/Didj/Data/0/JDI/New.bin", true), Ok("/Didj/Data/0/jdi/New.bin".into()));
        assert_eq!(v.resolve("/", "/etc/JDI", true), Ok("/etc/JDI".into()));
    }
}
