//! In-app update: downloads this system's package from a GitHub release,
//! puts the new app in place of the running one, and restarts into it.
//!
//! Windows and Linux replace the program file: a running program can be
//! renamed but not overwritten, so it moves aside (`.old`, removed at the
//! next start) and the new one takes its name. macOS replaces Ribbit.app the
//! same way. Only a release package installs itself (the app named Ribbit
//! with the release's licence notices beside it, or inside Ribbit.app); a
//! build from source, Android, or a folder Ribbit can't write to keep the
//! release page.

#[cfg(not(target_os = "android"))]
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::updates::{Asset, Release};

#[derive(Debug, Clone, Default, PartialEq)]
pub enum State {
    #[default]
    Idle,
    /// Bytes so far, of `total`.
    Downloading { done: u64, total: u64 },
    Installing,
    /// The new version is in place: restart to run it.
    Ready,
    Failed(String),
}

/// Where the running app is, and so what an update replaces.
#[derive(Debug, Clone)]
enum Target {
    /// Windows, Linux: the program file.
    Program(PathBuf),
    /// macOS: Ribbit.app.
    Bundle(PathBuf),
}

/// The package for this system in a release ("…-windows-x64.zip").
fn package_suffix() -> Option<&'static str> {
    if cfg!(all(windows, target_arch = "x86_64")) {
        Some("-windows-x64.zip")
    } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some("-linux-x64.tar.gz")
    } else if cfg!(target_os = "macos") {
        Some("-macos-universal.zip")
    } else {
        None
    }
}

/// The running app, when it is a release package that can update itself.
fn target() -> Option<Target> {
    package_suffix()?;
    let exe = std::env::current_exe().ok()?;
    if cfg!(target_os = "macos") {
        // Ribbit.app/Contents/MacOS/ribbit-gui
        let bundle = exe.ancestors().nth(3)?;
        return bundle.file_name().is_some_and(|n| n.eq_ignore_ascii_case("Ribbit.app")).then(|| Target::Bundle(bundle.into()));
    }
    let name = if cfg!(windows) { "Ribbit.exe" } else { "ribbit" };
    let packaged = exe.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name))
        && exe.with_file_name("THIRD-PARTY-NOTICES.txt").is_file();
    packaged.then_some(Target::Program(exe))
}

/// The package to download from `release`, when this app can install it.
pub fn installable(release: &Release) -> Option<Asset> {
    target()?;
    let suffix = package_suffix()?;
    release.assets.iter().find(|a| a.name.ends_with(suffix)).cloned()
}

/// The download and install, run in the background.
#[derive(Default)]
pub struct Installer {
    state: Arc<Mutex<State>>,
}

impl Installer {
    pub fn state(&self) -> State {
        self.state.lock().unwrap().clone()
    }

    /// Downloads and installs `asset` (unless already under way); `ctx`
    /// repaints as it goes.
    pub fn start(&self, asset: Asset, ctx: &egui::Context) {
        {
            let mut s = self.state.lock().unwrap();
            if matches!(*s, State::Downloading { .. } | State::Installing | State::Ready) {
                return;
            }
            *s = State::Downloading { done: 0, total: asset.size };
        }
        let state = Arc::clone(&self.state);
        let ctx = ctx.clone();
        let spawned = std::thread::Builder::new().name("ribbit-install".into()).spawn(move || {
            let set = |s: State| {
                *state.lock().unwrap() = s;
                ctx.request_repaint();
            };
            let result = target().ok_or_else(|| "this copy of Ribbit can't update itself".to_string()).and_then(|t| {
                let data = download(&asset, |done| set(State::Downloading { done, total: asset.size }))?;
                set(State::Installing);
                install(&t, &data)
            });
            match result {
                Ok(()) => {
                    tracing::info!(target: "ribbit::updates", "installed {}", asset.name);
                    set(State::Ready);
                }
                Err(e) => {
                    tracing::warn!(target: "ribbit::updates", "update failed: {e}");
                    set(State::Failed(e));
                }
            }
        });
        if let Err(e) = spawned {
            *self.state.lock().unwrap() = State::Failed(e.to_string());
        }
    }
}

#[cfg(target_os = "android")]
fn download(_asset: &Asset, _progress: impl FnMut(u64)) -> Result<Vec<u8>, String> {
    Err("not available in the Android app".into())
}

#[cfg(not(target_os = "android"))]
fn download(asset: &Asset, mut progress: impl FnMut(u64)) -> Result<Vec<u8>, String> {
    let agent: ureq::Agent =
        ureq::Agent::config_builder().timeout_connect(Some(std::time::Duration::from_secs(15))).build().into();
    let response = agent
        .get(&asset.url)
        .header("User-Agent", concat!("Ribbit/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| format!("couldn't download the update: {e}"))?;
    // (At most 256 MB: a package is about 10.)
    let mut reader = response.into_body().into_with_config().limit(256 << 20).reader();
    let mut data = Vec::with_capacity(asset.size as usize);
    let mut chunk = vec![0u8; 64 << 10];
    loop {
        let n = reader.read(&mut chunk).map_err(|e| format!("the download stopped: {e}"))?;
        if n == 0 {
            break;
        }
        data.extend_from_slice(&chunk[..n]);
        progress(data.len() as u64);
    }
    if data.len() as u64 != asset.size {
        return Err(format!("the download is incomplete ({} of {} bytes)", data.len(), asset.size));
    }
    Ok(data)
}

fn install(target: &Target, package: &[u8]) -> Result<(), String> {
    match target {
        #[cfg(not(target_os = "macos"))]
        Target::Program(exe) => replace_program(exe, &program_in(package)?),
        #[cfg(target_os = "macos")]
        Target::Bundle(app) => replace_bundle(app, package),
        #[allow(unreachable_patterns)]
        _ => Err("this copy of Ribbit can't update itself".into()),
    }
}

/// The program in a Windows zip (in a folder named Ribbit), checked as one.
#[cfg(windows)]
fn program_in(package: &[u8]) -> Result<Vec<u8>, String> {
    let program =
        ribbit_core::zip::extract_named(package, "Ribbit/Ribbit.exe").map_err(|e| format!("the update is damaged: {e}"))?;
    is_program(program, b"MZ")
}

/// The program in a Linux .tar.gz (in a folder named Ribbit), checked as one.
#[cfg(target_os = "linux")]
fn program_in(package: &[u8]) -> Result<Vec<u8>, String> {
    is_program(tar_gz_file(package, "Ribbit/ribbit")?, b"\x7fELF")
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
fn program_in(_package: &[u8]) -> Result<Vec<u8>, String> {
    Err("no program to install on this system".into())
}

#[cfg(any(windows, target_os = "linux"))]
fn is_program(program: Vec<u8>, magic: &[u8]) -> Result<Vec<u8>, String> {
    if program.starts_with(magic) { Ok(program) } else { Err("the update holds no program for this system".into()) }
}

/// One file from a .tar.gz.
#[cfg(target_os = "linux")]
fn tar_gz_file(package: &[u8], name: &str) -> Result<Vec<u8>, String> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(package).read_to_end(&mut tar).map_err(|e| format!("the update is damaged: {e}"))?;
    let mut at = 0;
    while let Some(header) = tar.get(at..at + 512) {
        if header.iter().all(|&b| b == 0) {
            break;
        }
        let field = |range: std::ops::Range<usize>| {
            let f = &header[range];
            String::from_utf8_lossy(&f[..f.iter().position(|&b| b == 0).unwrap_or(f.len())]).into_owned()
        };
        let size = usize::from_str_radix(field(124..136).trim(), 8).map_err(|_| "the update is damaged".to_string())?;
        let body = at + 512;
        if field(0..100) == name && matches!(header[156], b'0' | 0) {
            return tar.get(body..body + size).map(<[u8]>::to_vec).ok_or_else(|| "the update is damaged".to_string());
        }
        at = body + size.div_ceil(512) * 512;
    }
    Err("the update holds no program for this system".into())
}

/// Puts `program` in place of the running `exe`, which moves aside.
#[cfg(not(target_os = "macos"))]
fn replace_program(exe: &Path, program: &[u8]) -> Result<(), String> {
    let name = exe.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let new = exe.with_file_name(format!("{name}.new"));
    let old = exe.with_file_name(format!("{name}.old"));
    let cant = |e: std::io::Error| format!("Ribbit can't write to its folder ({}): {e}", exe.parent().unwrap_or(exe).display());
    std::fs::write(&new, program).map_err(cant)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new, std::fs::Permissions::from_mode(0o755)).map_err(cant)?;
    }
    // An older one left behind (still running when the last update started it).
    let _ = std::fs::remove_file(&old);
    if let Err(e) = std::fs::rename(exe, &old) {
        let _ = std::fs::remove_file(&new);
        return Err(cant(e));
    }
    if let Err(e) = std::fs::rename(&new, exe) {
        // Back as it was.
        let _ = std::fs::rename(&old, exe);
        let _ = std::fs::remove_file(&new);
        return Err(cant(e));
    }
    Ok(())
}

/// Puts the Ribbit.app from the macOS zip in place of `app`, which moves
/// aside. `ditto` unpacks it with its permissions and signature intact.
#[cfg(target_os = "macos")]
fn replace_bundle(app: &Path, package: &[u8]) -> Result<(), String> {
    let parent = app.parent().ok_or("Ribbit.app has no folder")?;
    let cant = |e: std::io::Error| format!("Ribbit can't write to {}: {e}", parent.display());
    // Unpacked next to the app, so the new one moves into place within one disk.
    let work = parent.join(".ribbit-update");
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(cant)?;
    let result = (|| {
        let zip = work.join("update.zip");
        std::fs::write(&zip, package).map_err(cant)?;
        let unpacked = work.join("unpacked");
        let status = std::process::Command::new("/usr/bin/ditto")
            .args(["-x", "-k"])
            .arg(&zip)
            .arg(&unpacked)
            .status()
            .map_err(|e| format!("couldn't unpack the update: {e}"))?;
        if !status.success() {
            return Err("couldn't unpack the update (ditto failed)".to_string());
        }
        // Ribbit-<version>-macos-universal/Ribbit.app
        let new = std::fs::read_dir(&unpacked)
            .map_err(cant)?
            .flatten()
            .map(|e| e.path().join("Ribbit.app"))
            .find(|p| p.join("Contents/MacOS/ribbit-gui").is_file())
            .ok_or("the update holds no Ribbit.app")?;
        let old = parent.join("Ribbit.app.old");
        let _ = std::fs::remove_dir_all(&old);
        std::fs::rename(app, &old).map_err(cant)?;
        if let Err(e) = std::fs::rename(&new, app) {
            let _ = std::fs::rename(&old, app);
            return Err(cant(e));
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&work);
    result
}

/// Starts the installed version (the caller then quits).
pub fn restart() -> std::io::Result<()> {
    match target() {
        // `-n`: a new instance, as this one is still running.
        Some(Target::Bundle(app)) => std::process::Command::new("/usr/bin/open").arg("-n").arg(app).spawn().map(drop),
        Some(Target::Program(exe)) => std::process::Command::new(exe).spawn().map(drop),
        None => Err(std::io::Error::other("this copy of Ribbit can't restart itself")),
    }
}

/// Removes what the last update moved aside. The old version may still be
/// closing for a moment, so this keeps trying for a few seconds in the
/// background.
pub fn remove_leftovers() {
    let old = match target() {
        Some(Target::Program(exe)) => exe.with_file_name(format!("{}.old", exe.file_name().unwrap_or_default().to_string_lossy())),
        Some(Target::Bundle(app)) => app.with_file_name("Ribbit.app.old"),
        None => return,
    };
    if !old.exists() {
        return;
    }
    let _ = std::thread::Builder::new().name("ribbit-cleanup".into()).spawn(move || {
        for _ in 0..40 {
            let gone = if old.is_dir() { std::fs::remove_dir_all(&old) } else { std::fs::remove_file(&old) };
            if gone.is_ok() || !old.exists() {
                tracing::info!(target: "ribbit::updates", "removed the previous version ({})", old.display());
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
    });
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    #[ignore = "downloads Ribbit 1.2.2 from GitHub"]
    fn a_release_downloads_and_replaces_the_program() {
        let asset = Asset {
            name: "Ribbit-1.2.2-windows-x64.zip".into(),
            url: "https://github.com/MotifMixes/ribbit/releases/download/v1.2.2/Ribbit-1.2.2-windows-x64.zip".into(),
            size: 9_262_193,
        };
        let mut last = 0;
        let package = download(&asset, |done| last = done).unwrap();
        assert_eq!(last, asset.size);
        let program = program_in(&package).unwrap();
        let dir = std::env::temp_dir().join(format!("ribbit-install-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("Ribbit.exe");
        std::fs::write(&exe, b"the old version").unwrap();
        replace_program(&exe, &program).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), program);
        assert_eq!(std::fs::read(dir.join("Ribbit.exe.old")).unwrap(), b"the old version");
        assert!(!dir.join("Ribbit.exe.new").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
