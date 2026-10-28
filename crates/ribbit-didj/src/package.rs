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
}
