//! Software packages, as LeapFrog Connect installs them. VERIFIED on the
//! packages archived for the console (US, UK and French, 2019-2025):
//!
//! * `.lfp`: a system package. A zip archive holding `Firmware/meta.inf`
//!   (key="value" lines), `Firmware/packagefiles.md5` and one binary named
//!   by `BinFile`: Boot (bootloader), System (base firmware), FileSys (a FAT32
//!   image of the internal storage's contents), MfgTest (factory test),
//!   Surgeon and NandUsbBin (recovery tools). Not encrypted.
//! * `.lf3`: a game pack (also the built-in games, music and videos). The
//!   whole file is encrypted: no header, no readable text. The System
//!   firmware links AES code and reports "failed to decrypt LDI"; the keys
//!   are not known. Ribbit does not try to decrypt them.
//!
//! Package IDs read `PCKT-0xPPPPPPPP-VVVVVV`: product 0x004F.... for system
//! packages, 0x0050.... for content.

/// What a package file is, from its name and first bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A system package (zip archive).
    System,
    /// An encrypted game pack.
    EncryptedPack,
}

/// The kind of a package file named `name` starting with `head` (at least
/// 4 bytes), or None if it is not one.
pub fn kind(name: &str, head: &[u8]) -> Option<Kind> {
    let lower = name.to_ascii_lowercase();
    let zip = head.starts_with(b"PK\x03\x04");
    if lower.ends_with(".lfp") && zip {
        return Some(Kind::System);
    }
    // LF3 files have no magic; the name is all there is (and a zip would
    // not be one).
    if lower.ends_with(".lf3") && !zip && head.len() >= 4 {
        return Some(Kind::EncryptedPack);
    }
    None
}

/// A package's `meta.inf`: `Key="value"` or `Key=value` lines.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
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

    /// For a RockIt Twist package: "PocketPlay" is the console's codename.
    pub fn is_rockit_twist(&self) -> bool {
        self.get("Device") == Some("PocketPlay")
    }
}

/// The package ID in a file name ("PCKT-0x00500001-000000.lf3" ->
/// "PCKT-0x00500001-000000").
pub fn package_id(file_name: &str) -> Option<&str> {
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    (stem.starts_with("PCKT-0x") && stem.len() == 22).then_some(stem)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_inf_is_read() {
        let m = Meta::parse(
            "MetaVersion=\"1.0\"\r\nDevice=\"PocketPlay\"\r\nProductID=0x004F0002\r\nName=\"Base\"\r\nBinFile=\"System\"\r\n",
        );
        assert!(m.is_rockit_twist());
        assert_eq!(m.get("ProductID"), Some("0x004F0002"));
        assert_eq!(m.get("BinFile"), Some("System"));
        assert_eq!(m.get("Missing"), None);
    }

    #[test]
    fn package_kinds_and_ids() {
        assert_eq!(kind("PCKT-0x004F0002-000000.lfp", b"PK\x03\x04rest"), Some(Kind::System));
        assert_eq!(kind("PCKT-0x00500001-000000.LF3", &[0xB1, 0x1A, 0xA1, 0x44]), Some(Kind::EncryptedPack));
        assert_eq!(kind("game.lfp", b"\0\0\0\0"), None, "an .lfp that is not a zip");
        assert_eq!(kind("game.bin", b"PK\x03\x04"), None);
        assert_eq!(package_id("PCKT-0x00500001-000000.lf3"), Some("PCKT-0x00500001-000000"));
        assert_eq!(package_id("PCKT-0x0050000D-PET000.lf3"), Some("PCKT-0x0050000D-PET000"));
        assert_eq!(package_id("other.lf3"), None);
    }
}
