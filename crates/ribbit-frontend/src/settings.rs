//! User settings, kept in `settings.toml` in the app's data folder
//! (Windows: %APPDATA%\Ribbit; Android: the app's private storage).

use crate::gpu::Display;
use ribbit_input::Button;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use winit::keyboard::KeyCode;

pub const MAX_RECENT: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// System firmware dumped from your own Leapster.
    pub bios: Option<PathBuf>,
    /// Cartridges opened recently, newest first.
    pub recent_carts: Vec<PathBuf>,
    /// Folders the game library scans for cartridges.
    pub rom_folders: Vec<PathBuf>,
    pub display: Display,
    /// AI upscaler style, quality and resolution (Display::Ai).
    pub ai: crate::upscale::AiSettings,
    /// Window size as a multiple of 160 pixels (0 = the largest that fits
    /// the screen).
    pub window_scale: u32,
    pub volume: f32,
    /// Host-side sound effects (the hardware is mono, 8 kHz).
    pub audio_enhance: crate::audio_out::Enhance,
    pub muted: bool,
    /// Emulation speed in percent (0 = unlimited).
    pub speed_percent: u32,
    /// Button name -> key name (winit `KeyCode` names, e.g. "ArrowUp").
    pub keys: BTreeMap<String, String>,
    /// EXPERIMENTAL, UNVERIFIED MAME-derived touchscreen model.
    pub experimental_touch: bool,
    /// Boot with straps bit 14 set (MAME's default), so the BIOS uses the
    /// saved calibration instead of forcing calibration at every boot.
    /// Ribbit's machine default (a Leapster 2 measurement) has it clear.
    pub skip_boot_calibration: bool,
    /// Press the calibration targets automatically (needs the touchscreen).
    pub auto_calibrate: bool,
    /// Show the game being played on Discord.
    pub discord_presence: bool,
    /// CPU clock in MHz: 96 is the nominal figure, 48 is what MAME runs.
    pub cpu_mhz: u64,
    /// EXPERIMENTAL: MAME's workaround of reading every unresolved GPIO line
    /// as 1 ("to avoid detecting a shutdown for now"), which the v1.5 BIOS
    /// needs to stay on. A guess, not hardware behaviour.
    pub mame_gpio_lines: bool,
    /// Where saves go (default: `saves` in the data folder).
    pub save_dir: Option<PathBuf>,
    pub show_status_bar: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            bios: None,
            recent_carts: Vec::new(),
            rom_folders: Vec::new(),
            display: Display::Pixel,
            ai: Default::default(),
            window_scale: 0,
            volume: 0.8,
            audio_enhance: Default::default(),
            muted: false,
            speed_percent: 100,
            keys: default_keys(),
            experimental_touch: true,
            skip_boot_calibration: false,
            auto_calibrate: true,
            discord_presence: true,
            cpu_mhz: 96,
            mame_gpio_lines: false,
            save_dir: None,
            show_status_bar: true,
        }
    }
}

pub fn button_name(b: Button) -> String {
    format!("{b:?}")
}

/// Buttons shown in the bindings dialog, in Leapster order.
pub const BINDABLE: [Button; 12] = [
    Button::Up,
    Button::Down,
    Button::Left,
    Button::Right,
    Button::A,
    Button::B,
    Button::Hint,
    Button::Home,
    Button::Pause,
    Button::VolumeUp,
    Button::VolumeDown,
    Button::Contrast,
];

pub fn default_keys() -> BTreeMap<String, String> {
    [
        (Button::Up, "ArrowUp"),
        (Button::Down, "ArrowDown"),
        (Button::Left, "ArrowLeft"),
        (Button::Right, "ArrowRight"),
        (Button::A, "KeyZ"),
        (Button::B, "KeyX"),
        (Button::Hint, "KeyH"),
        (Button::Home, "Backspace"),
        (Button::Pause, "Enter"),
        (Button::VolumeUp, "Equal"),
        (Button::VolumeDown, "Minus"),
    ]
    .into_iter()
    .map(|(b, k)| (button_name(b), k.to_string()))
    .collect()
}

/// Keys that can be bound, by name.
const KEYS: &[KeyCode] = &[
    KeyCode::ArrowUp,
    KeyCode::ArrowDown,
    KeyCode::ArrowLeft,
    KeyCode::ArrowRight,
    KeyCode::Enter,
    KeyCode::Space,
    KeyCode::Backspace,
    KeyCode::Tab,
    KeyCode::ShiftLeft,
    KeyCode::ShiftRight,
    KeyCode::ControlLeft,
    KeyCode::AltLeft,
    KeyCode::Minus,
    KeyCode::Equal,
    KeyCode::Comma,
    KeyCode::Period,
    KeyCode::Slash,
    KeyCode::Semicolon,
    KeyCode::Quote,
    KeyCode::BracketLeft,
    KeyCode::BracketRight,
    KeyCode::Numpad0,
    KeyCode::Numpad1,
    KeyCode::Numpad2,
    KeyCode::Numpad3,
    KeyCode::Numpad4,
    KeyCode::Numpad5,
    KeyCode::Numpad6,
    KeyCode::Numpad7,
    KeyCode::Numpad8,
    KeyCode::Numpad9,
    KeyCode::KeyA,
    KeyCode::KeyB,
    KeyCode::KeyC,
    KeyCode::KeyD,
    KeyCode::KeyE,
    KeyCode::KeyF,
    KeyCode::KeyG,
    KeyCode::KeyH,
    KeyCode::KeyI,
    KeyCode::KeyJ,
    KeyCode::KeyK,
    KeyCode::KeyL,
    KeyCode::KeyM,
    KeyCode::KeyN,
    KeyCode::KeyO,
    KeyCode::KeyP,
    KeyCode::KeyQ,
    KeyCode::KeyR,
    KeyCode::KeyS,
    KeyCode::KeyT,
    KeyCode::KeyU,
    KeyCode::KeyV,
    KeyCode::KeyW,
    KeyCode::KeyX,
    KeyCode::KeyY,
    KeyCode::KeyZ,
    KeyCode::Digit0,
    KeyCode::Digit1,
    KeyCode::Digit2,
    KeyCode::Digit3,
    KeyCode::Digit4,
    KeyCode::Digit5,
    KeyCode::Digit6,
    KeyCode::Digit7,
    KeyCode::Digit8,
    KeyCode::Digit9,
];

pub fn key_name(k: KeyCode) -> String {
    format!("{k:?}")
}

pub fn key_from_name(name: &str) -> Option<KeyCode> {
    KEYS.iter().copied().find(|k| key_name(*k) == name)
}

/// A friendlier label for menus ("ArrowUp" -> "Up", "KeyZ" -> "Z").
pub fn key_label(name: &str) -> String {
    name.strip_prefix("Key")
        .or_else(|| name.strip_prefix("Digit"))
        .or_else(|| name.strip_prefix("Arrow"))
        .unwrap_or(name)
        .to_string()
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                tracing::warn!(target: "ribbit::settings", "ignoring unreadable {}: {e}", path.display());
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> anyhow::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, toml::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    }

    /// The button a key is bound to, if any.
    pub fn button_for(&self, key: KeyCode) -> Option<Button> {
        let name = key_name(key);
        self.keys
            .iter()
            .find(|(_, k)| **k == name)
            .and_then(|(b, _)| BINDABLE.into_iter().find(|x| button_name(*x) == *b))
    }

    pub fn key_for(&self, b: Button) -> Option<&str> {
        self.keys.get(&button_name(b)).map(String::as_str)
    }

    pub fn bind(&mut self, b: Button, key: KeyCode) {
        let name = key_name(key);
        self.keys.retain(|_, k| *k != name); // one button per key
        self.keys.insert(button_name(b), name);
    }

    pub fn add_recent(&mut self, cart: &Path) {
        self.recent_carts.retain(|p| p != cart);
        self.recent_carts.insert(0, cart.to_path_buf());
        self.recent_carts.truncate(MAX_RECENT);
    }
}

/// The app's data folder (settings, saves, screenshots).
pub fn default_data_dir() -> PathBuf {
    if let Some(appdata) = std::env::var_os("APPDATA") {
        return PathBuf::from(appdata).join("Ribbit");
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME") {
        return PathBuf::from(xdg).join("ribbit");
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".config").join("ribbit");
    }
    PathBuf::from("ribbit-data")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip_and_bindings_are_unique() {
        let s = Settings::default();
        let text = toml::to_string_pretty(&s).unwrap();
        assert_eq!(toml::from_str::<Settings>(&text).unwrap(), s);
        assert_eq!(s.button_for(KeyCode::KeyZ), Some(Button::A));
        let mut s = s;
        s.bind(Button::B, KeyCode::KeyZ);
        assert_eq!(s.button_for(KeyCode::KeyZ), Some(Button::B));
        assert_eq!(s.key_for(Button::A), None, "the key moved to B");
        assert!(default_keys().values().all(|k| key_from_name(k).is_some()));
    }

    #[test]
    fn recent_list_is_most_recent_first_and_bounded() {
        let mut s = Settings::default();
        for i in 0..10 {
            s.add_recent(Path::new(&format!("game{i}.bin")));
        }
        s.add_recent(Path::new("game5.bin"));
        assert_eq!(s.recent_carts.len(), MAX_RECENT);
        assert_eq!(s.recent_carts[0], Path::new("game5.bin"));
    }

    #[test]
    fn partial_files_fill_in_defaults() {
        let s: Settings = toml::from_str("volume = 0.25").unwrap();
        assert_eq!((s.volume, s.window_scale), (0.25, 0));
    }
}
