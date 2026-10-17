//! Copies of the player's saves and settings, taken before anything could
//! change them: the first time a different Ribbit version starts (an update,
//! or going back to an older one), and when a file can't be used as it is.
//! Nothing here deletes a save; old backups beyond the newest few are
//! removed.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// This build's version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// How many update backups are kept (the oldest go first).
const KEEP: usize = 5;

/// When `last` (the version that ran last) is not this one, copies the save
/// folder and the settings file into `<data>/backups/before-<version>-<UTC
/// time>`, and returns that folder. Nothing to copy (a first start): None.
pub fn before_update(data_dir: &Path, save_dir: &Path, last: &str) -> Option<PathBuf> {
    if last == VERSION {
        return None;
    }
    let settings = data_dir.join("settings.toml");
    let has_saves = std::fs::read_dir(save_dir).is_ok_and(|mut d| d.next().is_some());
    if !has_saves && !settings.exists() {
        return None;
    }
    let root = data_dir.join("backups");
    let dest = root.join(format!("before-{VERSION}-{}", utc_stamp(SystemTime::now())));
    let copied = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&dest)?;
        if has_saves {
            copy_tree(save_dir, &dest.join("saves"))?;
        }
        if settings.exists() {
            std::fs::copy(&settings, dest.join("settings.toml"))?;
        }
        Ok(())
    })();
    match copied {
        Ok(()) => {
            let from = if last.is_empty() { "an earlier version".to_string() } else { last.to_string() };
            tracing::info!(target: "ribbit::backup", dest = %dest.display(), "saves backed up (from {from} to {VERSION})");
            prune(&root);
            Some(dest)
        }
        Err(e) => {
            tracing::warn!(target: "ribbit::backup", dest = %dest.display(), "backing up saves failed: {e}");
            None
        }
    }
}

/// Moves `path` aside as `<name>.<what>-<UTC time>.bak` (so it is kept, not
/// overwritten); the new name, if it could be moved.
pub fn set_aside(path: &Path, what: &str) -> Option<PathBuf> {
    let mut name = path.file_name()?.to_os_string();
    name.push(format!(".{what}-{}.bak", utc_stamp(SystemTime::now())));
    let to = path.with_file_name(name);
    match std::fs::rename(path, &to) {
        Ok(()) => {
            tracing::warn!(target: "ribbit::backup", kept = %to.display(), "{} could not be used and was kept aside", path.display());
            Some(to)
        }
        Err(e) => {
            tracing::warn!(target: "ribbit::backup", "keeping {} aside failed: {e}", path.display());
            None
        }
    }
}

/// Moves `files` (those that exist) into `<data>/backups/deleted-<UTC
/// time>`, so a deleted save can still be brought back by hand. Returns how
/// many were moved, and where.
pub fn move_to_deleted(data_dir: &Path, files: &[PathBuf]) -> std::io::Result<(usize, PathBuf)> {
    let dest = data_dir.join("backups").join(format!("deleted-{}", utc_stamp(SystemTime::now())));
    let mut moved = 0;
    for f in files.iter().filter(|f| f.is_file()) {
        std::fs::create_dir_all(&dest)?;
        let to = dest.join(f.file_name().unwrap_or_default());
        // A rename where possible; across drives, copy then remove.
        if std::fs::rename(f, &to).is_err() {
            std::fs::copy(f, &to)?;
            std::fs::remove_file(f)?;
        }
        moved += 1;
    }
    Ok((moved, dest))
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

/// Keeps the newest `KEEP` update backups (their names sort by time).
fn prune(root: &Path) {
    let Ok(dir) = std::fs::read_dir(root) else { return };
    let mut backups: Vec<PathBuf> = dir
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir() && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("before-")))
        .collect();
    // By the "YYYYMMDD-HHMMSS" at the end of the name (oldest first).
    backups.sort_by_key(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        n.get(n.len().saturating_sub(15)..).unwrap_or_default().to_string()
    });
    while backups.len() > KEEP {
        let old = backups.remove(0);
        if let Err(e) = std::fs::remove_dir_all(&old) {
            tracing::warn!(target: "ribbit::backup", "removing old backup {}: {e}", old.display());
        }
    }
}

/// "20261017-183005" (UTC), for file names.
fn utc_stamp(t: SystemTime) -> String {
    let secs = t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rem) = (secs / 86_400, secs % 86_400);
    // Days since 1970-01-01 to a civil date (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{:02}{:02}{:02}", rem / 3600, rem % 3600 / 60, rem % 60)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("ribbit-backup-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stamps_are_utc_dates() {
        assert_eq!(utc_stamp(UNIX_EPOCH), "19700101-000000");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(951_782_400 + 3_723)), "20000229-010203");
        assert_eq!(utc_stamp(UNIX_EPOCH + Duration::from_secs(1_792_256_561)), "20261017-170241");
    }

    #[test]
    fn a_new_version_copies_saves_and_settings_once() {
        let data = temp("update");
        let saves = data.join("saves");
        std::fs::create_dir_all(saves.join("sub")).unwrap();
        std::fs::write(saves.join("cart-0f2ad725.sav"), b"progress").unwrap();
        std::fs::write(saves.join("sub").join("x.bin"), b"deep").unwrap();
        std::fs::write(data.join("settings.toml"), b"bios = 'x'").unwrap();
        assert_eq!(before_update(&data, &saves, VERSION), None, "same version: nothing to do");
        let dest = before_update(&data, &saves, "1.0.0").expect("backed up");
        assert_eq!(std::fs::read(dest.join("saves").join("cart-0f2ad725.sav")).unwrap(), b"progress");
        assert_eq!(std::fs::read(dest.join("saves").join("sub").join("x.bin")).unwrap(), b"deep");
        assert_eq!(std::fs::read(dest.join("settings.toml")).unwrap(), b"bios = 'x'");
        assert_eq!(std::fs::read(saves.join("cart-0f2ad725.sav")).unwrap(), b"progress", "originals untouched");
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_first_start_backs_up_nothing() {
        let data = temp("fresh");
        assert_eq!(before_update(&data, &data.join("saves"), ""), None);
        assert!(!data.join("backups").exists());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn only_the_newest_backups_are_kept() {
        let root = temp("prune");
        for i in 0..8 {
            std::fs::create_dir_all(root.join(format!("before-1.{i}.0-2026101{i}-000000"))).unwrap();
        }
        std::fs::create_dir_all(root.join("something-else")).unwrap();
        prune(&root);
        let mut left: Vec<String> =
            std::fs::read_dir(&root).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        left.sort();
        assert_eq!(left.len(), KEEP + 1);
        assert!(left.contains(&"something-else".to_string()));
        assert!(left.contains(&"before-1.7.0-20261017-000000".to_string()));
        assert!(!left.contains(&"before-1.0.0-20261010-000000".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn deleted_saves_are_moved_into_backups() {
        let data = temp("delete");
        let save = data.join("saves").join("cart-0f2ad725.sav");
        let state = data.join("states").join("cart-0f2ad725-slot1.state");
        for f in [&save, &state] {
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(f, b"x").unwrap();
        }
        let missing = data.join("saves").join("cart-0f2ad725-not-there.sav");
        let (n, dest) = move_to_deleted(&data, &[save.clone(), state.clone(), missing]).unwrap();
        assert_eq!(n, 2);
        assert!(!save.exists() && !state.exists());
        assert!(dest.join("cart-0f2ad725.sav").exists() && dest.join("cart-0f2ad725-slot1.state").exists());
        assert!(dest.starts_with(data.join("backups")));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn set_aside_keeps_the_file() {
        let d = temp("aside");
        let f = d.join("rockit-twist.emmc");
        std::fs::write(&f, b"old progress").unwrap();
        let kept = set_aside(&f, "mismatch").unwrap();
        assert!(!f.exists());
        assert_eq!(std::fs::read(&kept).unwrap(), b"old progress");
        assert!(kept.file_name().unwrap().to_string_lossy().starts_with("rockit-twist.emmc.mismatch-"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
