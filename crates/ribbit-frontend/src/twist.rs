//! The LeapFrog RockIt Twist in the frontend: its packages in the library,
//! its keyboard layout, and the research-preview session. The console is
//! NOT emulated (crates/ribbit-twist): a session shows its controls working
//! and says plainly that nothing runs.

use crate::library::{Art, Family, Game};
use ribbit_twist::input::{Button, Controls, Rotary, Source, Step};
use ribbit_twist::package::{self, Kind};
use std::path::{Path, PathBuf};
use std::time::Instant;
use winit::keyboard::KeyCode;

/// What a key does on the RockIt Twist.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TwistKey {
    Press(Button),
    /// Turns a control one way while held (repeating).
    Turn(Rotary, Step),
    /// A swipe along the Slider, up (true) or down.
    Swipe(bool),
}

/// The keyboard layout (from the layout picture the project owner drew):
/// WASD or the arrows for the D-pad, Q and E for the Switch, the number pad
/// for the button grid (laid out as it is: 7 8 9 on top), * and - on the
/// number pad for the Spinner, U/J, I/K and O/L for the yellow, green and
/// red Dials, Page Up/Down (or the mouse wheel) for the Slider, Shift or Tab
/// for Hint, Home, and Enter for the check mark. Volume is - and =, as on
/// the Leapster. These replace the Leapster's keys (and Tab's fast forward)
/// only while a RockIt Twist session is open.
pub const KEYS: &[(KeyCode, TwistKey)] = &[
    (KeyCode::KeyW, TwistKey::Press(Button::DpadUp)),
    (KeyCode::ArrowUp, TwistKey::Press(Button::DpadUp)),
    (KeyCode::KeyS, TwistKey::Press(Button::DpadDown)),
    (KeyCode::ArrowDown, TwistKey::Press(Button::DpadDown)),
    (KeyCode::KeyA, TwistKey::Press(Button::DpadLeft)),
    (KeyCode::ArrowLeft, TwistKey::Press(Button::DpadLeft)),
    (KeyCode::KeyD, TwistKey::Press(Button::DpadRight)),
    (KeyCode::ArrowRight, TwistKey::Press(Button::DpadRight)),
    (KeyCode::KeyQ, TwistKey::Press(Button::SwitchLeft)),
    (KeyCode::KeyE, TwistKey::Press(Button::SwitchRight)),
    (KeyCode::Numpad7, TwistKey::Press(Button::Grid(0))),
    (KeyCode::Numpad8, TwistKey::Press(Button::Grid(1))),
    (KeyCode::Numpad9, TwistKey::Press(Button::Grid(2))),
    (KeyCode::Numpad4, TwistKey::Press(Button::Grid(3))),
    (KeyCode::Numpad5, TwistKey::Press(Button::Grid(4))),
    (KeyCode::Numpad6, TwistKey::Press(Button::Grid(5))),
    (KeyCode::Numpad1, TwistKey::Press(Button::Grid(6))),
    (KeyCode::Numpad2, TwistKey::Press(Button::Grid(7))),
    (KeyCode::Numpad3, TwistKey::Press(Button::Grid(8))),
    (KeyCode::NumpadMultiply, TwistKey::Turn(Rotary::Spinner, -1)),
    (KeyCode::NumpadSubtract, TwistKey::Turn(Rotary::Spinner, 1)),
    (KeyCode::KeyU, TwistKey::Turn(Rotary::YellowDial, 1)),
    (KeyCode::KeyJ, TwistKey::Turn(Rotary::YellowDial, -1)),
    (KeyCode::KeyI, TwistKey::Turn(Rotary::GreenDial, 1)),
    (KeyCode::KeyK, TwistKey::Turn(Rotary::GreenDial, -1)),
    (KeyCode::KeyO, TwistKey::Turn(Rotary::RedDial, 1)),
    (KeyCode::KeyL, TwistKey::Turn(Rotary::RedDial, -1)),
    (KeyCode::PageUp, TwistKey::Swipe(true)),
    (KeyCode::PageDown, TwistKey::Swipe(false)),
    (KeyCode::ShiftLeft, TwistKey::Press(Button::Hint)),
    (KeyCode::ShiftRight, TwistKey::Press(Button::Hint)),
    (KeyCode::Tab, TwistKey::Press(Button::Hint)),
    (KeyCode::Home, TwistKey::Press(Button::Home)),
    (KeyCode::Enter, TwistKey::Press(Button::Check)),
    (KeyCode::NumpadEnter, TwistKey::Press(Button::Check)),
    (KeyCode::Equal, TwistKey::Press(Button::VolumeUp)),
    (KeyCode::Minus, TwistKey::Press(Button::VolumeDown)),
];

pub fn key(code: KeyCode) -> Option<TwistKey> {
    KEYS.iter().find(|(k, _)| *k == code).map(|(_, t)| *t)
}

/// The first key for an action, as its keycap shows it.
pub fn cap_for(what: TwistKey) -> Option<String> {
    KEYS.iter().find(|(_, t)| *t == what).map(|(k, _)| crate::settings::cap_text(&format!("{k:?}")))
}

/// The keys for a control, for tooltips ("W / Up").
pub fn keys_for(what: TwistKey) -> String {
    let names: Vec<String> = KEYS.iter().filter(|(_, t)| *t == what).map(|(k, _)| key_label(*k)).collect();
    names.join(" / ")
}

fn key_label(k: KeyCode) -> String {
    match k {
        KeyCode::NumpadMultiply => "Num *".into(),
        KeyCode::NumpadSubtract => "Num -".into(),
        KeyCode::NumpadEnter => "Num Enter".into(),
        KeyCode::Equal => "=".into(),
        KeyCode::Minus => "-".into(),
        k => {
            let n = format!("{k:?}");
            if let Some(d) = n.strip_prefix("Numpad") {
                return format!("Num {d}");
            }
            crate::settings::key_label(&n)
        }
    }
}

/// A RockIt Twist session: the controls, no emulation.
pub struct TwistSession {
    pub title: String,
    /// The package opened (None: the console on its own).
    pub package: Option<PathBuf>,
    pub kind: Option<Kind>,
    pub controls: Controls,
    pub started: u64,
    last_tick: Instant,
}

impl TwistSession {
    pub fn start(package: Option<PathBuf>) -> Self {
        let kind = package.as_deref().and_then(|p| {
            let name = p.file_name()?.to_string_lossy().into_owned();
            package::kind(&name, &head(p)?)
        });
        let title = match &package {
            Some(p) => describe_package(p).map_or_else(|| "RockIt Twist".to_string(), |g| g.title),
            None => "RockIt Twist".to_string(),
        };
        Self {
            title,
            package,
            kind,
            controls: Controls::new(),
            started: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()),
            last_tick: Instant::now(),
        }
    }

    /// Advances held turns and swipes (once per frame).
    pub fn tick(&mut self) {
        let now = Instant::now();
        // Capped: a stalled frame must not spin a held dial far ahead.
        let dt = now.duration_since(self.last_tick).as_secs_f32().min(0.1);
        self.last_tick = now;
        self.controls.tick(dt);
    }

    /// A key went down or up; false if the RockIt Twist does not use it.
    pub fn key(&mut self, code: KeyCode, pressed: bool) -> bool {
        let Some(k) = key(code) else {
            return false;
        };
        match k {
            TwistKey::Press(b) => self.controls.set(Source::Keyboard, b, pressed),
            TwistKey::Turn(r, s) => self.controls.hold_turn(Source::Keyboard, r, pressed.then_some(s)),
            TwistKey::Swipe(up) => {
                if pressed {
                    self.controls.swipe(up);
                }
            }
        }
        true
    }

    /// The picture: the screen stays dark (nothing is emulated).
    pub fn frame(&self, out: &mut Vec<u32>) {
        out.clear();
        out.resize(ribbit_twist::SCREEN_W * ribbit_twist::SCREEN_H, 0);
    }
}

fn head(p: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut b = [0u8; 4];
    std::fs::File::open(p).ok()?.read_exact(&mut b).ok()?;
    Some(b.to_vec())
}

/// What the screen says during a session (drawn over the dark screen; it
/// is Ribbit's own text, not the console's).
pub fn overlay(painter: &egui::Painter, screen: egui::Rect, s: &TwistSession) {
    let c = &s.controls;
    let size = (screen.height() / 16.0).clamp(9.0, 18.0);
    let font = egui::FontId::proportional(size);
    let small = egui::FontId::proportional(size * 0.8);
    let mut lines: Vec<(String, egui::FontId, egui::Color32)> = vec![
        ("RockIt Twist: not emulated yet".into(), font.clone(), egui::Color32::from_rgb(140, 230, 90)),
        ("Research preview: the controls work; no software runs.".into(), small.clone(), egui::Color32::from_gray(200)),
    ];
    match s.kind {
        Some(Kind::EncryptedPack) => lines.push((
            "This game pack is encrypted; Ribbit cannot read it.".into(),
            small.clone(),
            egui::Color32::from_rgb(250, 190, 90),
        )),
        Some(Kind::System) => {
            lines.push(("System package: firmware research only.".into(), small.clone(), egui::Color32::from_gray(200)))
        }
        None => {}
    }
    if let Some(name) = s.package.as_deref().and_then(Path::file_name) {
        lines.push((format!("File: {}", name.to_string_lossy()), small.clone(), egui::Color32::from_gray(160)));
    }
    lines.push((String::new(), small.clone(), egui::Color32::WHITE));
    let held: Vec<String> = c.held_buttons().iter().map(|b| b.label()).collect();
    lines.push((
        format!("Held: {}", if held.is_empty() { "nothing".to_string() } else { held.join(", ") }),
        small.clone(),
        egui::Color32::WHITE,
    ));
    let turns: Vec<String> = Rotary::ALL.iter().map(|&r| format!("{} {:+}", r.label(), c.turned(r))).collect();
    lines.push((format!("Turned: {}", turns.join("  ")), small.clone(), egui::Color32::WHITE));
    lines.push((
        format!(
            "Slider: {}",
            c.slider().map_or("not touched".to_string(), |p| format!("finger at {:.0}% from the top", p * 100.0))
        ),
        small,
        egui::Color32::WHITE,
    ));
    let p = painter.with_clip_rect(screen);
    let mut y = screen.top() + size * 0.8;
    for (text, f, color) in lines {
        let g = p.layout(text, f, color, screen.width() - size * 1.6);
        p.galley(egui::pos2(screen.left() + size * 0.8, y), g.clone(), color);
        y += g.size().y + size * 0.25;
    }
}

/// The parsed `package-catalog.txt` (ID -> name) that archives of these
/// packages come with, searched for in the package's folder and the three
/// above it.
fn catalog_name(path: &Path, id: &str) -> Option<String> {
    let mut dir = path.parent();
    for _ in 0..4 {
        let d = dir?;
        if let Ok(text) = std::fs::read_to_string(d.join("package-catalog.txt")) {
            let mut name: Option<&str> = None;
            for line in text.lines() {
                let t = line.trim();
                if let Some(found) = t.strip_prefix("ID: ") {
                    if found == id {
                        return name.map(|n| {
                            n.replace("RockIt Twist™ Game Pack: ", "").replace("Rockit Twist™ Game Pack: ", "")
                        });
                    }
                } else if !t.is_empty() && !t.starts_with('-') && !t.starts_with('=') {
                    name = Some(t);
                }
            }
            return None;
        }
        dir = d.parent();
    }
    None
}

/// A library entry for a RockIt Twist package, or None if `path` is not one.
pub fn describe_package(path: &Path) -> Option<Game> {
    let name = path.file_name()?.to_string_lossy().into_owned();
    let lower = name.to_ascii_lowercase();
    if !lower.ends_with(".lfp") && !lower.ends_with(".lf3") {
        return None;
    }
    let kind = package::kind(&name, &head(path)?)?;
    let size = std::fs::metadata(path).ok()?.len();
    let id = package::package_id(&name).map(str::to_string);
    // The locale is the folder name in the archive's layout (en-us/<ID>/<version>/).
    let locale = path
        .ancestors()
        .filter_map(|a| a.file_name()?.to_str())
        .find(|n| n.len() == 5 && n.as_bytes()[2] == b'-')
        .map(str::to_string);
    let mut details = vec![("System", "RockIt Twist".to_string())];
    let (title, what, tag) = match kind {
        Kind::System => {
            let meta = std::fs::read(path)
                .ok()
                .and_then(|d| ribbit_core::zip::extract_named(&d, "Firmware/meta.inf").ok())
                .map(|m| package::Meta::parse(&String::from_utf8_lossy(&m)))
                .unwrap_or_default();
            for (label, key) in [
                ("Name", "Name"),
                ("Version", "Version"),
                ("Type", "Type"),
                ("Binary", "BinFile"),
                ("Built", "BuildDate"),
            ] {
                if let Some(v) = meta.get(key) {
                    details.push((label, v.to_string()));
                }
            }
            let title = format!("RockIt Twist system: {}", meta.get("Name").unwrap_or("package"));
            (title, "System package (firmware)", "System")
        }
        Kind::EncryptedPack => {
            let title = id.as_deref().and_then(|i| catalog_name(path, i)).unwrap_or_else(|| name.clone());
            details.push(("Contents", "encrypted (LF3): cannot be read".to_string()));
            (title, "Encrypted game pack", "Encrypted")
        }
    };
    if let Some(i) = &id {
        details.push(("Package", i.clone()));
    }
    if let Some(l) = &locale {
        details.push(("Locale", l.clone()));
    }
    let mb = size.div_ceil(1 << 20);
    details.push(("Size", format!("{mb} MB")));
    details.push(("Status", "not emulated (research preview)".to_string()));
    details.push(("File", path.display().to_string()));
    let subtitle = [Some(what.to_string()), locale.clone(), Some(format!("{mb} MB"))]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join("  ·  ");
    let key = format!("twist-{}-{}-{size}", id.as_deref().unwrap_or(&name), locale.as_deref().unwrap_or(""));
    Some(Game {
        path: path.to_path_buf(),
        family: Family::RockItTwist,
        title,
        subtitle,
        details,
        tags: vec![tag],
        key,
        art: Art::Missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_control_has_a_key_and_no_key_does_two_things() {
        for (i, (k, _)) in KEYS.iter().enumerate() {
            assert!(KEYS[i + 1..].iter().all(|(o, _)| o != k), "{k:?} is bound twice");
        }
        for b in Button::ALL.into_iter().filter(|&b| b != Button::Power) {
            assert!(KEYS.iter().any(|(_, t)| *t == TwistKey::Press(b)), "{b:?} has no key");
        }
        for r in Rotary::ALL {
            for s in [-1, 1] {
                assert!(KEYS.iter().any(|(_, t)| *t == TwistKey::Turn(r, s)), "{r:?} {s} has no key");
            }
        }
        assert_eq!(keys_for(TwistKey::Press(Button::DpadUp)), "W / Up");
        assert_eq!(keys_for(TwistKey::Turn(Rotary::Spinner, -1)), "Num *");
        assert_eq!(keys_for(TwistKey::Press(Button::Grid(0))), "Num 7");
        assert_eq!(cap_for(TwistKey::Press(Button::Grid(0))).as_deref(), Some("7"));
        assert_eq!(cap_for(TwistKey::Turn(Rotary::RedDial, -1)).as_deref(), Some("L"));
        assert_eq!(cap_for(TwistKey::Swipe(true)).as_deref(), Some("PgUp"));
        assert_eq!(cap_for(TwistKey::Press(Button::Power)), None);
    }

    #[test]
    fn keys_press_and_release_and_turn() {
        let mut s = TwistSession::start(None);
        assert!(s.key(KeyCode::KeyE, true));
        assert!(s.controls.pressed(Button::SwitchRight));
        assert!(s.key(KeyCode::KeyE, false));
        assert!(!s.controls.pressed(Button::SwitchRight));
        assert!(s.key(KeyCode::KeyU, true));
        assert_eq!(s.controls.turned(Rotary::YellowDial), 1);
        s.key(KeyCode::KeyU, false);
        assert!(s.key(KeyCode::PageDown, true));
        assert!(s.controls.slider().is_some());
        assert!(!s.key(KeyCode::F12, true), "not a RockIt Twist key");
    }

    #[test]
    fn packages_are_listed_by_their_catalog_names() {
        let dir = std::env::temp_dir().join(format!("ribbit-twist-{}", std::process::id()));
        let pack = dir.join("en-us").join("PCKT-0x0050000E-000000").join("1.1.3.21");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            dir.join("package-catalog.txt"),
            "Package Catalog\n===\n\nen-us\n-----\n\nRockIt Twist™ Game Pack: Dinosaur Discoveries™\n  ID: PCKT-0x0050000E-000000\n",
        )
        .unwrap();
        let file = pack.join("PCKT-0x0050000E-000000.lf3");
        std::fs::write(&file, [0x31, 0x0A, 0x7F, 0x9C, 1, 2, 3]).unwrap();
        let g = describe_package(&file).unwrap();
        assert_eq!(g.title, "Dinosaur Discoveries™");
        assert_eq!(g.family, Family::RockItTwist);
        assert_eq!(g.tags, ["Encrypted"]);
        assert!(g.subtitle.contains("en-us"));
        assert!(describe_package(&dir.join("package-catalog.txt")).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
