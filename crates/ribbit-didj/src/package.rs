//! Software packages, as LeapFrog Connect installs them. VERIFIED on the
//! firmware 1.35.2.4222 packages (DIDJ-0x000E0002 to 0x000E0010) and the
//! built-in game JetPack Heroes (DIDJ-0x000F0001):
//!
//! * A `.lfp` is a zip archive, not encrypted. Files sit in one top-level
//!   folder, with a `meta.inf` (key="value" lines) and `.md5` files.
//! * System firmware: `bootstrap-LF_LF1000/` (0x000E0002: lightning-boot.bin,
//!   bootflags.jffs2) and `firmware-LF_LF1000/` (0x000E0003: kernel.bin,
//!   erootfs.jffs2). These are raw NAND partition contents (nand.rs).
//! * Everything else (Brio libraries, fonts, audio, apps, avatars, games)
//!   is a folder of files installed into the Brio partition's file system.
//!
//! Package IDs read `DIDJ-0xPPPPPPPP-VVVVVV`: product 0x000E.... for system
//! packages, 0x000F.... for games.

/// A package's `meta.inf`.
pub struct Meta {
    pub fields: Vec<(String, String)>,
}

impl Meta {
    pub fn parse(text: &str) -> Self {
        let fields = text
            .lines()
            .filter_map(|l| l.split_once('='))
            .map(|(k, v)| (k.trim().to_string(), v.trim().trim_matches('"').to_string()))
            .collect();
        Self { fields }
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.fields.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
    }

    pub fn is_didj(&self) -> bool {
        self.get("Device") == Some("Didj")
    }
}

/// The firmware files that are written to NAND partitions as they are.
#[derive(Default)]
pub struct Firmware {
    /// The first-stage bootloader (partition LF1000_uniboot).
    pub lightning_boot: Option<Vec<u8>>,
    /// Which kernel and root file system to boot (Atomic_Boot_Flags; JFFS2).
    pub bootflags: Option<Vec<u8>>,
    /// The boot splash and kernel (Kernel0 and Kernel1; tfs.rs).
    pub kernel: Option<Vec<u8>>,
    /// The root file system (Linux_RFS0 and Linux_RFS1; JFFS2).
    pub rootfs: Option<Vec<u8>>,
}

impl Firmware {
    /// Keeps `data` if `path` (inside a package) is one of the firmware
    /// files; returns whether it was.
    pub fn add(&mut self, path: &str, data: Vec<u8>) -> bool {
        let slot = match path.rsplit('/').next().unwrap_or(path) {
            "lightning-boot.bin" => &mut self.lightning_boot,
            "bootflags.jffs2" => &mut self.bootflags,
            "kernel.bin" => &mut self.kernel,
            "erootfs.jffs2" => &mut self.rootfs,
            _ => return false,
        };
        *slot = Some(data);
        true
    }

    /// The names of the files still missing.
    pub fn missing(&self) -> Vec<&'static str> {
        [
            (self.lightning_boot.is_none(), "lightning-boot.bin"),
            (self.bootflags.is_none(), "bootflags.jffs2"),
            (self.kernel.is_none(), "kernel.bin"),
            (self.rootfs.is_none(), "erootfs.jffs2"),
        ]
        .into_iter()
        .filter_map(|(m, n)| m.then_some(n))
        .collect()
    }
}

/// The package ID in a file name ("DIDJ-0x000E0003-000001.lfp" ->
/// "DIDJ-0x000E0003-000001").
pub fn package_id(file_name: &str) -> Option<&str> {
    let stem = file_name.rsplit(['/', '\\']).next()?.strip_suffix(".lfp")?;
    (stem.len() == 22 && stem.starts_with("DIDJ-0x")).then_some(stem)
}

/// The system packages a Didj is built from (product, name), as firmware
/// 1.35.2.4222 has them: without any of them the system is incomplete.
/// The avatar (0x000E0010 AstroGuyHelmet) and the built-in game
/// (0x000F0001 JetPack Heroes) came with it too, but the menu starts
/// without them (`EXTRAS`).
pub const NEEDED: [(&str, &str); 11] = [
    ("0x000E0002", "Didj Device Bootstrap"),
    ("0x000E0003", "Didj Device Firmware"),
    ("0x000E0004", "Brio"),
    ("0x000E0005", "Base_Fonts"),
    ("0x000E0006", "Base_LocalizedAudio"),
    ("0x000E0007", "Base_Tutorials"),
    ("0x000E0008", "Base_UniversialArt"),
    ("0x000E0009", "Base_UniversialAudio"),
    ("0x000E000A", "Base_bin"),
    ("0x000E000B", "Base_lib"),
    ("0x000E000C", "BLT"),
];

/// Packages a Didj came with that it starts without.
pub const EXTRAS: [(&str, &str); 2] = [("0x000E0010", "AstroGuyHelmet"), ("0x000F0001", "JetPack Heroes")];

/// A file bundling the packages: a zip of the `.lfp` files (Ribbit's own
/// idea, not LeapFrog's), read as if they lay loose beside it.
pub const BUNDLE: &str = "bios.didj";

/// Whether `path` names a package bundle (`BUNDLE`, any case).
pub fn is_bundle(path: &str) -> bool {
    path.rsplit(['/', '\\']).next().is_some_and(|n| n.eq_ignore_ascii_case(BUNDLE))
}

/// The product in a package ID ("DIDJ-0x000E0003-000001" -> "0x000E0003").
pub fn product(id: &str) -> &str {
    &id[5..15]
}

/// A package's top-level `meta.inf`, if `data` is a package (a zip).
pub fn meta_of(data: &[u8]) -> Option<Meta> {
    let entries = ribbit_core::zip::entries(data).ok()?;
    let entry = entries.iter().find(|e| e.name.to_ascii_lowercase().ends_with("meta.inf") && e.name.matches('/').count() <= 1)?;
    let text = ribbit_core::zip::read_entry(data, entry).ok()?;
    Some(Meta::parse(&String::from_utf8_lossy(&text)))
}

/// The package ID of the `.lfp` file `name` holding `data`: from its name,
/// else from its meta.inf `PackageID` (a renamed file, such as players'
/// "JetPack Heroes.lfp").
pub fn identify(name: &str, data: &[u8]) -> Option<String> {
    if let Some(id) = package_id(name) {
        return Some(id.to_string());
    }
    if !name.to_ascii_lowercase().ends_with(".lfp") {
        return None;
    }
    let meta = meta_of(data).filter(Meta::is_didj)?;
    package_id(&format!("{}.lfp", meta.get("PackageID")?)).map(str::to_string)
}

/// Whether every version of the package is a package of its own: a
/// micromod (`Type="MicroDownload"`) is numbered within its game's product
/// (DIDJ-0x000F0012-000012 "The Play", -000015 "Happy Ending": Tinker
/// Bell's cinematics), where for the rest the number is a version. Avatars
/// (`Type="Avatar"`: the Didjis players make in LeapFrog Connect, shown by
/// the menu's CScreenSelectAvatar, "NO DIDJIS CREATED") are kept every one
/// too: a player has up to 10, and whether they share a product number is
/// UNKNOWN (no Didji package seen); keeping only the newest per product
/// would drop the rest. An avatar's versions all install into its folder,
/// the newest last.
fn each_version_kept(data: &[u8]) -> bool {
    meta_of(data).is_some_and(|m| matches!(m.get("Type"), Some("MicroDownload" | "Avatar")))
}

/// The packages in `dir` as (file name, contents): the `.lfp` files there
/// and in its subfolders (players keep micromods in one), and those in a
/// `BUNDLE` there; one per product (the newest version; a loose file over
/// the same one bundled), but every micromod; in name order. A renamed
/// `.lfp` is known by its meta.inf and named here by its package ID.
pub fn packages_in(dir: &std::path::Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut found: std::collections::BTreeMap<String, (String, Vec<u8>)> = Default::default();
    let mut keep = |id: String, data: Vec<u8>| {
        let key = if each_version_kept(&data) { id.clone() } else { product(&id).to_string() };
        let newer = found.get(&key).is_none_or(|(old, _)| package_id(old).unwrap()[16..] <= id[16..]);
        if newer {
            found.insert(key, (format!("{id}.lfp"), data));
        }
    };
    let list = |d: &std::path::Path| -> Result<Vec<std::path::PathBuf>, String> {
        let mut v: Vec<_> = std::fs::read_dir(d).map_err(|e| format!("{}: {e}", d.display()))?.flatten().map(|e| e.path()).collect();
        v.sort();
        Ok(v)
    };
    let paths = list(dir)?;
    let mut files: Vec<_> = paths.iter().filter(|p| p.is_file()).cloned().collect();
    for sub in paths.iter().filter(|p| p.is_dir()) {
        files.extend(list(sub).unwrap_or_default().into_iter().filter(|p| p.is_file()));
    }
    let read = |p: &std::path::Path| std::fs::read(p).map_err(|e| format!("{}: {e}", p.display()));
    for path in paths.iter().filter(|p| is_bundle(&p.to_string_lossy())) {
        let data = read(path)?;
        let entries = ribbit_core::zip::entries(&data).map_err(|e| format!("{}: {e}", path.display()))?;
        for entry in entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with(".lfp")) {
            let bytes = ribbit_core::zip::read_entry(&data, entry).map_err(|e| format!("{}: {}: {e}", path.display(), entry.name))?;
            if let Some(id) = identify(&entry.name, &bytes) {
                keep(id, bytes);
            }
        }
    }
    for path in &files {
        let name = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        if name.to_ascii_lowercase().ends_with(".lfp") {
            let data = read(path)?;
            if let Some(id) = identify(&name, &data) {
                keep(id, data);
            }
        }
    }
    Ok(found.into_values().collect())
}

/// The `NEEDED` packages not among the file `names`, as
/// "DIDJ-0x000E0006 (Base_LocalizedAudio)".
pub fn missing<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    absent(&NEEDED, names)
}

/// The `EXTRAS` not among the file `names`, named as by `missing`.
pub fn missing_extras<'a>(names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    absent(&EXTRAS, names)
}

fn absent<'a>(list: &[(&str, &str)], names: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let have: Vec<&str> = names.into_iter().filter_map(package_id).map(product).collect();
    list.iter().filter(|(p, _)| !have.iter().any(|h| h.eq_ignore_ascii_case(p))).map(|(p, n)| format!("DIDJ-{p} ({n})")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_and_ids() {
        let m = Meta::parse("Device=\"Didj\"\r\nProductID=0x000E0003\r\nIcon= \r\n");
        assert!(m.is_didj());
        assert_eq!(m.get("ProductID"), Some("0x000E0003"));
        assert_eq!(m.get("Icon"), Some(""));
        assert_eq!(package_id("didj/DIDJ-0x000E0003-000001.lfp"), Some("DIDJ-0x000E0003-000001"));
        assert_eq!(package_id("swclonewrs.zip"), None);
    }

    #[test]
    fn firmware_files_by_name() {
        let mut f = Firmware::default();
        assert!(f.add("firmware-LF_LF1000/kernel.bin", vec![1]));
        assert!(!f.add("firmware-LF_LF1000/kernel.md5", vec![2]));
        assert_eq!(f.missing(), ["lightning-boot.bin", "bootflags.jffs2", "erootfs.jffs2"]);
    }

    /// A zip of stored (uncompressed) files.
    /// Every avatar (Didji) is installed; of a game, the newest version.
    #[test]
    fn every_avatar_kept() {
        let dir = std::env::temp_dir().join(format!("ribbit-didj-avatars-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let pkg = |ty: &str, folder: &str| stored_zip(&[(&format!("{folder}/meta.inf"), format!("Device=\"Didj\"
Type=\"{ty}\"
").as_bytes())]);
        for (name, ty, folder) in [
            ("DIDJ-0x000E0010-000002.lfp", "Avatar", "Avatar1"),
            ("DIDJ-0x000E0010-000003.lfp", "Avatar", "Avatar2"),
            ("DIDJ-0x000F0001-000000.lfp", "Application", "POW"),
            ("DIDJ-0x000F0001-000001.lfp", "Application", "POW"),
        ] {
            std::fs::write(dir.join(name), pkg(ty, folder)).unwrap();
        }
        let found = packages_in(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let mut names: Vec<&str> = found.iter().map(|(n, _)| n.as_str()).collect();
        names.sort();
        assert_eq!(names, ["DIDJ-0x000E0010-000002.lfp", "DIDJ-0x000E0010-000003.lfp", "DIDJ-0x000F0001-000001.lfp"]);
    }

    fn stored_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let (mut out, mut dir) = (Vec::new(), Vec::new());
        for (name, data) in files {
            let crc = !data.iter().fold(!0u32, |c, &b| (0..8).fold(c ^ b as u32, |c, _| if c & 1 != 0 { c >> 1 ^ 0xEDB8_8320 } else { c >> 1 }));
            let local = out.len() as u32;
            let sizes = [crc, data.len() as u32, data.len() as u32];
            out.extend_from_slice(b"PK\x03\x04\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            sizes.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes()));
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(data);
            dir.extend_from_slice(b"PK\x01\x02\x14\x00\x14\x00\x00\x00\x00\x00\x00\x00\x00\x00");
            sizes.iter().for_each(|v| dir.extend_from_slice(&v.to_le_bytes()));
            dir.extend_from_slice(&(name.len() as u16).to_le_bytes());
            dir.extend_from_slice(&[0; 12]);
            dir.extend_from_slice(&local.to_le_bytes());
            dir.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&dir);
        out.extend_from_slice(b"PK\x05\x06\x00\x00\x00\x00");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(dir.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn bundled_and_loose_packages() {
        let dir = std::env::temp_dir().join(format!("ribbit-didj-bundle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bundle = stored_zip(&[("DIDJ-0x000E0002-000001.lfp", b"old"), ("sub/DIDJ-0x000E0003-000001.lfp", b"bundled"), ("readme.txt", b"x")]);
        std::fs::write(dir.join("BIOS.didj"), bundle).unwrap();
        std::fs::write(dir.join("DIDJ-0x000E0002-000002.lfp"), b"new").unwrap();
        std::fs::write(dir.join("DIDJ-0x000E0003-000001.lfp"), b"loose").unwrap();
        let found = packages_in(&dir).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        let found: Vec<(&str, &[u8])> = found.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        assert_eq!(found, [("DIDJ-0x000E0002-000002.lfp", &b"new"[..]), ("DIDJ-0x000E0003-000001.lfp", &b"loose"[..])]);
        let names = found.iter().map(|(n, _)| *n);
        let missing = missing(names);
        assert_eq!(missing.len(), NEEDED.len() - 2);
        assert_eq!(missing[0], "DIDJ-0x000E0004 (Brio)");
    }
}
