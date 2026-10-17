//! On-screen touch controls: a D-pad and buttons drawn in the bottom
//! corners of the window, for phones and touchscreens. Several fingers at
//! once (each finger holds what it is on; sliding along the D-pad changes
//! direction). Shown on Android from the start and elsewhere once the
//! screen is touched; any key or controller button hides them until the
//! next touch. A presentation feature only, like the console view: presses
//! go to the same buttons the keyboard drives. Not shown while the console
//! view is on (its own controls take touches).

use crate::console_view::{Target, dpad_side};
use egui::{Color32, Pos2, Rect, Stroke, pos2, vec2};
use ribbit_input::Button;
use ribbit_memory::known::Hardware;
use ribbit_twist::input::Button as TwistButton;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    /// Up, down, left, right.
    Dpad([Target; 4]),
    Round(Target),
    /// A small rounded button (Home, Pause...).
    Pill(Target),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    kind: Kind,
    rect: Rect,
    label: &'static str,
    color: Color32,
}

/// The controls for `hw` placed in `area` (points; the window's game area).
pub fn layout(hw: Hardware, area: Rect) -> Vec<Pad> {
    // One unit: about a fingertip, scaled with the shorter side.
    let u = (area.width().min(area.height()) * 0.1).clamp(40.0, 78.0);
    let margin = u * 0.45;
    let dpad_c = pos2(area.left() + margin + 1.6 * u, area.bottom() - margin - 1.6 * u);
    let dpad = Rect::from_center_size(dpad_c, vec2(3.2 * u, 3.2 * u));
    let round = |c: Pos2| Rect::from_center_size(c, vec2(1.35 * u, 1.35 * u));
    let right = area.right() - margin;
    let bottom = area.bottom() - margin;
    // A (or the action button) lowest right, B left of it, the third above.
    let first = round(pos2(right - 0.7 * u, bottom - 1.9 * u));
    let second = round(pos2(right - 2.3 * u, bottom - 1.1 * u));
    let third = round(pos2(right - 1.5 * u, bottom - 3.4 * u));
    // Small buttons in rows above the D-pad (side 0) and the round buttons
    // (side 1), clear of the middle where the game's screen is.
    let pill = |side: u8, row: f32| {
        let x = if side == 0 { dpad_c.x } else { right - 1.5 * u };
        Rect::from_center_size(pos2(x, bottom - (4.5 + 0.85 * row) * u), vec2(2.2 * u, 0.6 * u))
    };
    let l = Target::Leapster;
    let t = Target::Twist;
    let grey = Color32::from_gray(90);
    let mut pads = Vec::new();
    match hw {
        Hardware::RockItTwist => {
            let dirs = [TwistButton::DpadUp, TwistButton::DpadDown, TwistButton::DpadLeft, TwistButton::DpadRight];
            pads.push(Pad { kind: Kind::Dpad(dirs.map(t)), rect: dpad, label: "", color: grey });
            pads.push(Pad { kind: Kind::Round(t(TwistButton::Check)), rect: first, label: "✔", color: GREEN });
            pads.push(Pad { kind: Kind::Round(t(TwistButton::Hint)), rect: second, label: "?", color: BLUE });
            pads.push(Pad { kind: Kind::Pill(t(TwistButton::SwitchLeft)), rect: pill(0, 0.0), label: "◀ Switch", color: grey });
            pads.push(Pad { kind: Kind::Pill(t(TwistButton::Home)), rect: pill(1, 1.0), label: "Home", color: grey });
            pads.push(Pad { kind: Kind::Pill(t(TwistButton::SwitchRight)), rect: pill(1, 0.0), label: "Switch ▶", color: grey });
        }
        Hardware::Zippity => {
            let dirs = [Button::Up, Button::Down, Button::Left, Button::Right];
            pads.push(Pad { kind: Kind::Dpad(dirs.map(l)), rect: dpad, label: "", color: grey });
            pads.push(Pad { kind: Kind::Round(l(Button::A)), rect: first, label: "", color: GREEN });
            pads.push(Pad { kind: Kind::Round(l(Button::B)), rect: second, label: "", color: RED });
            pads.push(Pad { kind: Kind::Round(l(Button::Hint)), rect: third, label: "", color: BLUE });
            pads.push(Pad { kind: Kind::Pill(l(Button::VolumeDown)), rect: pill(0, 0.0), label: "Left foot", color: grey });
            pads.push(Pad { kind: Kind::Pill(l(Button::Home)), rect: pill(0, 1.0), label: "Home", color: grey });
            pads.push(Pad { kind: Kind::Pill(l(Button::VolumeUp)), rect: pill(1, 0.0), label: "Right foot", color: grey });
        }
        _ => {
            let dirs = [Button::Up, Button::Down, Button::Left, Button::Right];
            pads.push(Pad { kind: Kind::Dpad(dirs.map(l)), rect: dpad, label: "", color: grey });
            pads.push(Pad { kind: Kind::Round(l(Button::A)), rect: first, label: "A", color: GREEN });
            pads.push(Pad { kind: Kind::Round(l(Button::B)), rect: second, label: "B", color: RED });
            pads.push(Pad { kind: Kind::Round(l(Button::Hint)), rect: third, label: "Hint", color: BLUE });
            pads.push(Pad { kind: Kind::Pill(l(Button::Home)), rect: pill(0, 0.0), label: "Home", color: grey });
            pads.push(Pad { kind: Kind::Pill(l(Button::Pause)), rect: pill(1, 0.0), label: "Pause", color: grey });
        }
    }
    pads
}

const GREEN: Color32 = Color32::from_rgb(46, 170, 82);
const RED: Color32 = Color32::from_rgb(214, 64, 58);
const BLUE: Color32 = Color32::from_rgb(52, 120, 220);

/// What a finger at `p` is on (the D-pad's side; round buttons answer a
/// little outside their edge, as fingers are not precise).
pub fn hit(pads: &[Pad], p: Pos2) -> Option<Target> {
    pads.iter().find_map(|pad| match pad.kind {
        Kind::Dpad(ts) => pad.rect.contains(p).then(|| dpad_side(pad.rect, p).map(|s| ts[s])).flatten(),
        Kind::Round(t) => (p.distance(pad.rect.center()) <= pad.rect.width() * 0.62).then_some(t),
        Kind::Pill(t) => pad.rect.expand(4.0).contains(p).then_some(t),
    })
}

/// The fingers on the controls.
#[derive(Default)]
pub struct TouchPad {
    /// Drawn and taking touches.
    pub visible: bool,
    /// Finger id -> what it holds (None: on no control, e.g. the D-pad's
    /// centre, but started on the controls so it stays theirs).
    fingers: Vec<(u64, Option<Target>)>,
}

impl TouchPad {
    pub fn new() -> Self {
        Self { visible: cfg!(target_os = "android"), fingers: Vec::new() }
    }

    /// A finger came down at `p`: whether the controls take it, and the
    /// press to make.
    pub fn down(&mut self, pads: &[Pad], id: u64, p: Pos2) -> (bool, Option<Target>) {
        let near = pads.iter().any(|pad| pad.rect.expand(8.0).contains(p));
        if !near {
            return (false, None);
        }
        let t = hit(pads, p);
        self.fingers.push((id, t));
        (true, t)
    }

    /// A finger moved: (let go, press) when it slid onto another control.
    /// None when the finger is not one of the controls'.
    pub fn moved(&mut self, pads: &[Pad], id: u64, p: Pos2) -> Option<(Option<Target>, Option<Target>)> {
        let f = self.fingers.iter_mut().find(|(i, _)| *i == id)?;
        let now = hit(pads, p);
        if now == f.1 {
            return Some((None, None));
        }
        Some((std::mem::replace(&mut f.1, now), now))
    }

    /// A finger lifted: what it let go of (Some(None): it held nothing;
    /// None: not one of the controls' fingers).
    pub fn up(&mut self, id: u64) -> Option<Option<Target>> {
        let i = self.fingers.iter().position(|(f, _)| *f == id)?;
        Some(self.fingers.remove(i).1)
    }

    /// Lets go of everything (what was held).
    pub fn release_all(&mut self) -> Vec<Target> {
        self.fingers.drain(..).filter_map(|(_, t)| t).collect()
    }

    pub fn holds(&self, t: Target) -> bool {
        self.fingers.iter().any(|(_, h)| *h == Some(t))
    }
}

/// Draws the controls (translucent, so the picture shows through).
pub fn paint(painter: &egui::Painter, pads: &[Pad], pad: &TouchPad) {
    let fill = |c: Color32, on: bool| c.gamma_multiply(if on { 0.85 } else { 0.42 });
    let edge = Stroke::new(1.5, Color32::from_white_alpha(70));
    let text = Color32::from_white_alpha(225);
    for p in pads {
        match p.kind {
            Kind::Dpad(ts) => {
                let r = p.rect;
                let arm = r.width() * 0.34;
                let c = r.center();
                let arms = [
                    Rect::from_min_max(pos2(c.x - arm / 2.0, r.top()), pos2(c.x + arm / 2.0, c.y)),
                    Rect::from_min_max(pos2(c.x - arm / 2.0, c.y), pos2(c.x + arm / 2.0, r.bottom())),
                    Rect::from_min_max(pos2(r.left(), c.y - arm / 2.0), pos2(c.x, c.y + arm / 2.0)),
                    Rect::from_min_max(pos2(c.x, c.y - arm / 2.0), pos2(r.right(), c.y + arm / 2.0)),
                ];
                for (a, t) in arms.iter().zip(ts) {
                    painter.rect(*a, arm * 0.25, fill(p.color, pad.holds(t)), edge, egui::StrokeKind::Inside);
                }
                for (a, s) in arms.iter().zip(["▲", "▼", "◀", "▶"]) {
                    let at = a.center() + (a.center() - c) * 0.25;
                    painter.text(at, egui::Align2::CENTER_CENTER, s, egui::FontId::proportional(arm * 0.4), text);
                }
            }
            Kind::Round(t) => {
                let r = p.rect.width() / 2.0;
                painter.circle(p.rect.center(), r, fill(p.color, pad.holds(t)), edge);
                let size = if p.label.chars().count() > 2 { r * 0.5 } else { r * 0.8 };
                painter.text(p.rect.center(), egui::Align2::CENTER_CENTER, p.label, egui::FontId::proportional(size), text);
            }
            Kind::Pill(t) => {
                let h = p.rect.height();
                painter.rect(p.rect, h / 2.0, fill(p.color, pad.holds(t)), edge, egui::StrokeKind::Inside);
                painter.text(p.rect.center(), egui::Align2::CENTER_CENTER, p.label, egui::FontId::proportional(h * 0.45), text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_fit_in_the_area_and_are_hit_where_drawn() {
        for hw in [Hardware::Leapster, Hardware::Zippity, Hardware::RockItTwist] {
            for area in [Rect::from_min_size(Pos2::ZERO, vec2(400.0, 800.0)), Rect::from_min_size(Pos2::ZERO, vec2(900.0, 420.0))] {
                let pads = layout(hw, area);
                for p in &pads {
                    assert!(area.contains_rect(p.rect), "{hw:?} {:?} outside {area:?}", p.rect);
                    match p.kind {
                        Kind::Dpad(ts) => {
                            let up = p.rect.center() - vec2(0.0, p.rect.height() * 0.4);
                            assert_eq!(hit(&pads, up), Some(ts[0]));
                        }
                        Kind::Round(t) | Kind::Pill(t) => assert_eq!(hit(&pads, p.rect.center()), Some(t)),
                    }
                }
            }
        }
    }

    #[test]
    fn fingers_slide_between_directions_and_let_go() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 800.0));
        let pads = layout(Hardware::Leapster, area);
        let Kind::Dpad(_) = pads[0].kind else { panic!() };
        let r = pads[0].rect;
        let mut tp = TouchPad::default();
        let (taken, t) = tp.down(&pads, 1, r.center() - vec2(0.0, r.height() * 0.4));
        assert!(taken);
        assert_eq!(t, Some(Target::Leapster(Button::Up)));
        let (off, on) = tp.moved(&pads, 1, r.center() + vec2(r.width() * 0.4, 0.0)).unwrap();
        assert_eq!((off, on), (Some(Target::Leapster(Button::Up)), Some(Target::Leapster(Button::Right))));
        // A second finger on A at the same time.
        let a = pads.iter().find(|p| p.label == "A").unwrap().rect.center();
        assert_eq!(tp.down(&pads, 2, a).1, Some(Target::Leapster(Button::A)));
        assert_eq!(tp.up(1), Some(Some(Target::Leapster(Button::Right))));
        assert_eq!(tp.release_all(), vec![Target::Leapster(Button::A)]);
        // Away from the controls: the stylus's.
        assert!(!tp.down(&pads, 3, area.center()).0);
        assert_eq!(tp.up(3), None);
    }
}
