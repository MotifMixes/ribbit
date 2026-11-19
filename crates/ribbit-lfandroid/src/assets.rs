//! libandroid's asset manager over the APK's `assets/` folder. The titles' own file layer
//! goes through it: `/sdcard/LF/Bulk/...` is looked up as the asset `LF/Bulk/...`.
//! An AAsset is a whole entry read into memory; an AAssetDir is the list of files (not
//! folders, as on Android) directly in one folder.

use crate::apk::Apk;
use crate::runtime::{Runtime, Stop};
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Assets {
    pub apk: Option<Apk>,
    /// Open assets by AAsset* (contents, position).
    open: BTreeMap<u32, (Vec<u8>, usize)>,
    /// Open folders by AAssetDir* (names, next index, current name's string).
    dirs: BTreeMap<u32, (Vec<String>, usize, u32)>,
    /// What was opened and not found (LFA_LOG).
    pub missing: Vec<String>,
    /// File descriptors from AAsset_openFileDescriptor: each asset's contents, whole (on
    /// Android it's the APK's descriptor with the entry's offset; here offset 0).
    pub fds: BTreeMap<u32, Vec<u8>>,
}

/// The first descriptor handed out (above any the game could expect from the system).
const FIRST_FD: u32 = 100;

/// The one AAssetManager* handed out.
const MANAGER: u32 = 0x0A55_E700;

fn entry_name(name: &str) -> String {
    format!("assets/{}", name.trim_start_matches("./").trim_start_matches('/'))
}

pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    match name {
        "AAssetManager_fromJava" => rt.ret(MANAGER),
        // AAssetManager_open(mgr, filename, mode)
        "AAssetManager_open" => {
            let a = rt.arg(1);
            let file = rt.mem.cstr(a);
            let data = rt.assets.apk.as_ref().and_then(|apk| apk.read(&entry_name(&file)).ok());
            match data {
                Some(d) => {
                    if std::env::var_os("LFA_LOG").is_some() {
                        eprintln!("[asset] {file}: {} bytes", d.len());
                    }
                    let h = rt.malloc(16);
                    rt.assets.open.insert(h, (d, 0));
                    rt.ret(h)
                }
                None => {
                    if std::env::var_os("LFA_LOG").is_some() {
                        eprintln!("[asset] {file}: not found (from {:#x})", rt.cpu.r[14]);
                    }
                    // LFA_STOP=<text>: stop at the first missing asset whose name has it.
                    if std::env::var("LFA_STOP").is_ok_and(|t| file.contains(&t)) {
                        let bt = rt.backtrace();
                        return Err(Stop::Halt(format!("LFA_STOP: asset {file} not found; backtrace {bt:08x?}")));
                    }
                    rt.assets.missing.push(file);
                    rt.ret(0)
                }
            }
        }
        // AAsset_read(asset, buf, count): bytes read, 0 at the end
        "AAsset_read" => {
            let (h, buf, n) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let chunk = match rt.assets.open.get_mut(&h) {
                Some((d, pos)) => {
                    let end = (*pos + n as usize).min(d.len());
                    let c = d[*pos..end].to_vec();
                    *pos = end;
                    c
                }
                None => Vec::new(),
            };
            rt.mem.write(buf, &chunk);
            rt.ret(chunk.len() as u32)
        }
        // AAsset_seek(asset, offset, whence): the new position, or -1
        "AAsset_seek" => {
            let (h, off, whence) = (rt.arg(0), rt.arg(1) as i32 as i64, rt.arg(2));
            let r = match rt.assets.open.get_mut(&h) {
                Some((d, pos)) => {
                    let base = match whence {
                        0 => 0,
                        1 => *pos as i64,
                        _ => d.len() as i64,
                    };
                    let p = base + off;
                    if (0..=d.len() as i64).contains(&p) {
                        *pos = p as usize;
                        p as u32
                    } else {
                        u32::MAX
                    }
                }
                None => u32::MAX,
            };
            rt.ret(r)
        }
        "AAsset_getLength" | "AAsset_getRemainingLength" => {
            let h = rt.arg(0);
            let n = match rt.assets.open.get(&h) {
                Some((d, pos)) if name == "AAsset_getRemainingLength" => d.len() - pos,
                Some((d, _)) => d.len(),
                None => 0,
            };
            rt.ret(n as u32)
        }
        "AAsset_close" => {
            let h = rt.arg(0);
            if rt.assets.open.remove(&h).is_some() {
                rt.free(h);
            }
            rt.ret(0)
        }
        // AAsset_openFileDescriptor(asset, off_t* start, off_t* length): a descriptor
        "AAsset_openFileDescriptor" => {
            let (h, start, length) = (rt.arg(0), rt.arg(1), rt.arg(2));
            let Some((data, _)) = rt.assets.open.get(&h) else {
                rt.ret(u32::MAX);
                return Ok(());
            };
            let data = data.clone();
            let fd = (FIRST_FD..).find(|f| !rt.assets.fds.contains_key(f)).unwrap();
            rt.mem.w32(start, 0);
            rt.mem.w32(length, data.len() as u32);
            rt.assets.fds.insert(fd, data);
            rt.ret(fd)
        }
        // AAssetManager_openDir(mgr, dirName)
        "AAssetManager_openDir" => {
            let a = rt.arg(1);
            let dir = rt.mem.cstr(a);
            let prefix = entry_name(&dir);
            let prefix = if prefix.ends_with('/') { prefix } else { format!("{prefix}/") };
            let names: Vec<String> = match &rt.assets.apk {
                Some(apk) => apk
                    .entries
                    .keys()
                    .filter_map(|k| k.strip_prefix(&prefix))
                    .filter(|rest| !rest.is_empty() && !rest.contains('/'))
                    .map(|s| s.to_string())
                    .collect(),
                None => Vec::new(),
            };
            if std::env::var_os("LFA_LOG").is_some() {
                eprintln!("[asset] folder {dir}: {} files", names.len());
            }
            let h = rt.malloc(16);
            rt.assets.dirs.insert(h, (names, 0, 0));
            rt.ret(h)
        }
        // AAssetDir_getNextFileName(dir): the name (valid until the next call), or NULL
        "AAssetDir_getNextFileName" => {
            let h = rt.arg(0);
            let (next, old) = match rt.assets.dirs.get_mut(&h) {
                Some((names, i, cur)) => {
                    *i += 1;
                    (names.get(*i - 1).cloned(), std::mem::take(cur))
                }
                None => (None, 0),
            };
            if old != 0 {
                rt.free(old);
            }
            let r = match next {
                Some(n) => {
                    let s = rt.alloc_str(&n);
                    if let Some((_, _, cur)) = rt.assets.dirs.get_mut(&h) {
                        *cur = s;
                    }
                    s
                }
                None => 0,
            };
            rt.ret(r)
        }
        "AAssetDir_rewind" => {
            let h = rt.arg(0);
            if let Some((_, i, _)) = rt.assets.dirs.get_mut(&h) {
                *i = 0;
            }
            rt.ret(0)
        }
        "AAssetDir_close" => {
            let h = rt.arg(0);
            if let Some((_, _, cur)) = rt.assets.dirs.remove(&h) {
                if cur != 0 {
                    rt.free(cur);
                }
                rt.free(h);
            }
            rt.ret(0)
        }
        _ => return Err(Stop::Import(name.to_string(), rt.cpu.r[14])),
    }
    Ok(())
}
