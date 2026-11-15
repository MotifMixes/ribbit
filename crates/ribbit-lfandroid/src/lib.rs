//! LeapFrog Android tablets: LeapFrog Epic (2016, MediaTek MT8127, Android 4.4) and the
//! LeapPad Academy (2019; RK3326 revision runs Android 10 Go, build "EPIC4").
//!
//! Their LeapFrog games (`com.leapfrog.ANDS.app.*` on the Epic, `com.leapfrog.AND4.app.*` on
//! the Academy) are the Brio SDK games of the LeapPad/Explorer line rebuilt for Android: a Java
//! shell (`LFMainActivity`) plus one ARMv7 native library per title that statically links the
//! game, its engine and Brio itself (`LeapFrog::Brio::C*MPI`, with `_android` back ends and
//! `LF::Android::LFAppManager`). See docs/leappad-academy.md.
//!
//! This crate is the first step of an "Enhanced (new)"-style HLE for them: it opens a title's
//! APK, finds and loads its native library, and reports exactly what an HLE has to stand in
//! for (the Android NDK imports and the JNI natives the Java shell calls).

pub mod apk;
pub mod elf;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Where the loader puts the game library.
pub const GAME_BASE: u32 = 0x4000_0000;
/// Each import gets its own 8-byte slot here; an HLE traps execution in this range.
pub const THUNK_BASE: u32 = 0xF000_0000;

/// The Android library an import would come from, by name. This is the HLE's work list.
pub fn api_area(name: &str) -> &'static str {
    if name.starts_with("egl") {
        "EGL"
    } else if name.starts_with("gl") {
        "OpenGL ES 1.1/2.0"
    } else if name.starts_with("SL_IID_") || name.starts_with("slCreate") {
        "OpenSL ES"
    } else if name.starts_with("XA_IID_") || name.starts_with("xaCreate") {
        "OpenMAX AL"
    } else if name.starts_with("AAsset") || name.starts_with("ANativeWindow") || name.starts_with("ASensor") || name.starts_with("ALooper") || name.starts_with("AConfiguration") {
        "libandroid (NDK)"
    } else if name.starts_with("__android_log") {
        "liblog"
    } else if name.starts_with("_Z") || name.starts_with("__cxa") || name.starts_with("__gxx") || name.starts_with("_Unwind") {
        "C++ runtime"
    } else if name.starts_with("inflate") || name.starts_with("deflate") || name.starts_with("crc32") || name.starts_with("adler32") || name.starts_with("zlib") || name.starts_with("uncompress") {
        "zlib"
    } else if name.starts_with("dl") && matches!(name, "dlopen" | "dlsym" | "dlclose" | "dlerror" | "dladdr") {
        "libdl"
    } else if name.starts_with("pthread_") || name.starts_with("sem_") {
        "bionic pthread"
    } else {
        "bionic libc/libm"
    }
}

pub struct Title {
    pub apk_path: PathBuf,
    pub package: String,
    pub version: String,
    pub lib_name: Option<String>,
    pub content_type: Option<String>,
    pub sdk_version: Option<String>,
    /// Asset folders (two levels under assets/) with file count and bytes.
    pub asset_dirs: BTreeMap<String, (usize, u64)>,
    pub library_source: Option<String>,
    pub library: Option<elf::Loaded>,
    /// JNI natives registered on LFMainActivity and friends (names like `naDrawFrame`).
    pub natives: Vec<String>,
}

fn after<'a>(strings: &'a [String], key: &str) -> Option<&'a String> {
    strings.iter().position(|s| s == key).and_then(|i| strings.get(i + 1))
}

/// Opens a LeapFrog Android title. `system_lib` is the firmware's /system/lib, where the
/// Academy keeps the libraries of its built-in titles (their APKs are "NoSO").
pub fn open_title(apk_path: &Path, system_lib: Option<&Path>) -> Result<Title, String> {
    let data = std::fs::read(apk_path).map_err(|e| format!("{}: {e}", apk_path.display()))?;
    let apk = apk::Apk::open(data)?;
    let man = apk::manifest_strings(&apk.read("AndroidManifest.xml")?);
    let package = man.iter().find(|s| s.starts_with("com.") && s.matches('.').count() >= 2 && !s.contains("permission")).cloned().unwrap_or_default();
    let version = man.iter().find(|s| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || c == '.') && s.contains('.')).cloned().unwrap_or_default();
    let lib_name = after(&man, "android.app.lib_name").cloned();
    let content_type = after(&man, "contentType").cloned();
    let sdk_version = after(&man, "sdkVersion").cloned();

    let mut asset_dirs: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for e in apk.entries.values() {
        if let Some(rest) = e.name.strip_prefix("assets/") {
            let parts: Vec<&str> = rest.split('/').collect();
            let key = if parts.len() > 2 { format!("{}/{}", parts[0], parts[1]) } else if parts.len() == 2 { parts[0].to_string() } else { ".".to_string() };
            let v = asset_dirs.entry(key).or_default();
            v.0 += 1;
            v.1 += e.size as u64;
        }
    }

    // The game library: inside the APK, else the firmware's /system/lib.
    let mut lib_bytes = None;
    let mut library_source = None;
    if let Some(name) = &lib_name {
        let so = format!("lib{name}.so");
        for abi in ["armeabi-v7a", "armeabi"] {
            let p = format!("lib/{abi}/{so}");
            if apk.entries.contains_key(&p) {
                lib_bytes = Some(apk.read(&p)?);
                library_source = Some(format!("APK {p}"));
                break;
            }
        }
        if lib_bytes.is_none() {
            if let Some(dir) = system_lib {
                let p = dir.join(&so);
                if let Ok(b) = std::fs::read(&p) {
                    lib_bytes = Some(b);
                    library_source = Some(format!("firmware {}", p.display()));
                }
            }
        }
    }

    let mut natives: Vec<String> = Vec::new();
    let library = match &lib_bytes {
        Some(bytes) => {
            let mut next = THUNK_BASE;
            let l = elf::load(bytes, GAME_BASE, |_| {
                let a = next;
                next += 8;
                Some(a)
            })?;
            natives = jni_natives(bytes);
            Some(l)
        }
        None => None,
    };

    Ok(Title { apk_path: apk_path.to_path_buf(), package, version, lib_name, content_type, sdk_version, asset_dirs, library_source, library, natives })
}

/// The JNI method names in the library's RegisterNatives tables: Brio-on-Android names them
/// `na...` (naInitMainActivity, naDrawFrame, naTouchDown, naPressA, ...).
pub fn jni_natives(bytes: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<u8> = Vec::new();
    for &b in bytes.iter().chain(std::iter::once(&0u8)) {
        if b == 0 {
            if cur.len() >= 4 && cur[0] == b'n' && cur[1] == b'a' && cur[2].is_ascii_uppercase() && cur.iter().all(|c| c.is_ascii_alphanumeric()) {
                let s = String::from_utf8_lossy(&cur).into_owned();
                if !out.contains(&s) {
                    out.push(s);
                }
            }
            cur.clear();
        } else if cur.len() < 80 {
            cur.push(b);
        }
    }
    out
}
