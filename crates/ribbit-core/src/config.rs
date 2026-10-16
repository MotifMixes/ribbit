//! User configuration (TOML). Paths point at the user's own dumps.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// System firmware ("BaseROM") dump.
    pub firmware: Option<PathBuf>,
    /// Cartridge ROM dump.
    pub cartridge: Option<PathBuf>,
    /// Directory for NVRAM files. Defaults to next to the cartridge.
    pub save_dir: Option<PathBuf>,
    /// `tracing` filter used when `RUST_LOG` is unset.
    pub log_filter: Option<String>,
    /// System configuration register 0x0180_9004 (board straps). Default:
    /// MAME's Leapster 2 measurement 0x63FF_BFFF; bit 26 follows cart presence.
    pub straps: Option<u32>,
    /// EXPERIMENTAL: attach a stylus to the touch panel model (io/touch.rs),
    /// stylus up. Off by default.
    pub experimental_mame_touch: bool,
    /// Value of the unknown non-button bits of GPIO input 0x0180_004C (default
    /// 0; the board lines 30/12/11 are separate, io/gpio.rs). An explicit
    /// experiment setting, like `straps`.
    pub input_lines: Option<u32>,
    /// File holding the system EEPROM (calibration, player data). Loaded at
    /// start if it exists, written back when it changed. Your own data.
    pub system_eeprom: Option<PathBuf>,
    /// File holding the cartridge save chip (game saves). Loaded at start if
    /// it exists, written back when it changed.
    pub cart_save: Option<PathBuf>,
    /// CPU clock in MHz (default 96, the nominal figure; MAME uses 48 to
    /// match real-hardware videos). Placeholder timing either way.
    pub cpu_mhz: Option<u64>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("reading {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("parsing {path}: {source}")]
    Parse { path: PathBuf, source: toml::de::Error },
}

impl Config {
    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(text)
    }

    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Io { path: path.into(), source })?;
        Self::from_toml(&text).map_err(|source| ConfigError::Parse { path: path.into(), source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_rejects_unknown_keys() {
        let cfg = Config::from_toml("firmware = \"bios.bin\"\ncartridge = \"game.bin\"\n").unwrap();
        assert_eq!(cfg.firmware.as_deref(), Some(Path::new("bios.bin")));
        assert_eq!(cfg.cartridge.as_deref(), Some(Path::new("game.bin")));
        assert!(Config::from_toml("firmwear = \"x\"").is_err());
        assert_eq!(Config::from_toml("").unwrap(), Config::default());
        assert!(!Config::default().experimental_mame_touch, "experimental touch is opt-in");
        assert!(Config::from_toml("experimental_mame_touch = true").unwrap().experimental_mame_touch);
    }
}
