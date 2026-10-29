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
    /// The Leapster's silver plastic behind its controls (not a button),
    /// with its top left and right corners rounded or not, and the speaker
    /// grille and A and B's recess on it or not.
    Body {
        nw: bool,
        ne: bool,
        grille: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Pad {
    kind: Kind,
    rect: Rect,
    label: &'static str,
    color: Color32,
    /// Printed on the body above the button (the Leapster's HOME, HINT, PAUSE).
    caption: &'static str,
    /// Drawn in the Leapster's style: blue buttons, a silver cross in a blue ring.
    leap: bool,
    /// Round buttons drawn as ovals leaning this far (radians; negative
    /// leans the top left), like the Leapster's A and B.
    tilt: f32,
}

/// A control in the plain style (the Twist and the Zippity).
fn plain(kind: Kind, rect: Rect, label: &'static str, color: Color32) -> Pad {
    Pad { kind, rect, label, color, caption: "", leap: false, tilt: 0.0 }
}

/// A Leapster control.
fn leap(kind: Kind, rect: Rect, label: &'static str, caption: &'static str, tilt: f32) -> Pad {
    Pad { kind, rect, label, color: LEAP_BLUE, caption, leap: true, tilt }
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
    let l = Target::Leapster;
    let t = Target::Twist;
    let grey = Color32::from_gray(90);
    let mut pads = Vec::new();
    match hw {
        Hardware::RockItTwist => {
            // A (or the action button) lowest right, B left of it, the third above.
            let first = round(pos2(right - 0.7 * u, bottom - 1.9 * u));
            let second = round(pos2(right - 2.3 * u, bottom - 1.1 * u));
            let pill = |side: u8, row: f32| {
                let x = if side == 0 { dpad_c.x } else { right - 1.5 * u };
                Rect::from_center_size(pos2(x, bottom - (4.5 + 0.85 * row) * u), vec2(2.2 * u, 0.6 * u))
            };
            let dirs = [TwistButton::DpadUp, TwistButton::DpadDown, TwistButton::DpadLeft, TwistButton::DpadRight];
            pads.push(plain(Kind::Dpad(dirs.map(t)), dpad, "", grey));
            pads.push(plain(Kind::Round(t(TwistButton::Check)), first, "✔", GREEN));
            pads.push(plain(Kind::Round(t(TwistButton::Hint)), second, "?", BLUE));
            pads.push(plain(Kind::Pill(t(TwistButton::SwitchLeft)), pill(0, 0.0), "◀ Switch", grey));
            pads.push(plain(Kind::Pill(t(TwistButton::Home)), pill(1, 1.0), "Home", grey));
            pads.push(plain(Kind::Pill(t(TwistButton::SwitchRight)), pill(1, 0.0), "Switch ▶", grey));
        }
        Hardware::Zippity => {
            let first = round(pos2(right - 0.7 * u, bottom - 1.9 * u));
            let second = round(pos2(right - 2.3 * u, bottom - 1.1 * u));
            let third = round(pos2(right - 1.5 * u, bottom - 3.4 * u));
            let pill = |side: u8, row: f32| {
                let x = if side == 0 { dpad_c.x } else { right - 1.5 * u };
                Rect::from_center_size(pos2(x, bottom - (4.5 + 0.85 * row) * u), vec2(2.2 * u, 0.6 * u))
            };
            let dirs = [Button::Up, Button::Down, Button::Left, Button::Right];
            pads.push(plain(Kind::Dpad(dirs.map(l)), dpad, "", grey));
            pads.push(plain(Kind::Round(l(Button::A)), first, "", GREEN));
            pads.push(plain(Kind::Round(l(Button::B)), second, "", RED));
            pads.push(plain(Kind::Round(l(Button::Hint)), third, "", BLUE));
            pads.push(plain(Kind::Pill(l(Button::VolumeDown)), pill(0, 0.0), "Left foot", grey));
            pads.push(plain(Kind::Pill(l(Button::Home)), pill(0, 1.0), "Home", grey));
            pads.push(plain(Kind::Pill(l(Button::VolumeUp)), pill(1, 0.0), "Right foot", grey));
        }
        _ if !SHELL => {
            // The Leapster's own layout: the D-pad in its blue ring lower left,
            // HOME, HINT and PAUSE in a row above it (named on the body),
            // A on the right with the smaller B right of it and lower.
            let dirs = [Button::Up, Button::Down, Button::Left, Button::Right];
            pads.push(leap(Kind::Dpad(dirs.map(l)), dpad, "", "", 0.0));
            let a = Rect::from_center_size(pos2(right - 2.0 * u, bottom - 2.4 * u), vec2(1.55 * u, 1.55 * u));
            let b = Rect::from_center_size(pos2(right - 0.6 * u, bottom - 1.55 * u), vec2(1.2 * u, 1.2 * u));
            pads.push(leap(Kind::Round(l(Button::A)), a, "A", "", 0.0));
            pads.push(leap(Kind::Round(l(Button::B)), b, "B", "", 0.0));
            let small = |dx: f32, dy: f32| Rect::from_center_size(dpad_c + vec2(dx * u, dy * u), vec2(0.8 * u, 0.8 * u));
            pads.push(leap(Kind::Round(l(Button::Home)), small(-1.1, -2.45), "", "HOME", 0.0));
            pads.push(leap(Kind::Round(l(Button::Hint)), small(0.0, -2.45), "", "HINT", 0.0));
            pads.push(leap(Kind::Round(l(Button::Pause)), small(1.1, -2.45), "", "PAUSE", 0.0));
        }
        _ => {
            // The Leapster's own layout (from a photo of one): the D-pad in its
            // blue ring lower left with HOME, HINT and PAUSE in an arc above it
            // (named on the body); A and B leaning ovals in one silver recess on
            // the right, B lower, under the round speaker grille. Each side sits
            // on a silver wing growing out of the bottom corner; one body across
            // the bottom when the wings would nearly meet (a narrow window).
            let dirs = [Button::Up, Button::Down, Button::Left, Button::Right];
            let top = bottom - 5.35 * u;
            let left_wing = Rect::from_min_max(pos2(area.left(), top), pos2(dpad.right() + 0.5 * u, area.bottom()));
            let a_c = pos2(right - 2.15 * u, bottom - 1.64 * u);
            let b_c = pos2(right - 0.85 * u, bottom - 0.95 * u);
            let right_wing = Rect::from_min_max(pos2(a_c.x - 1.2 * u, top), pos2(area.right(), area.bottom()));
            if right_wing.left() - left_wing.right() < 3.0 * u {
                let body = left_wing.union(right_wing);
                pads.push(leap(Kind::Body { nw: true, ne: true, grille: true }, body, "", "", 0.0));
            } else {
                pads.push(leap(Kind::Body { nw: false, ne: true, grille: false }, left_wing, "", "", 0.0));
                pads.push(leap(Kind::Body { nw: true, ne: false, grille: true }, right_wing, "", "", 0.0));
            }
            pads.push(leap(Kind::Dpad(dirs.map(l)), dpad, "", "", 0.0));
            let a = Rect::from_center_size(a_c, vec2(1.15 * u, 1.7 * u));
            let b = Rect::from_center_size(b_c, vec2(0.95 * u, 1.4 * u));
            pads.push(leap(Kind::Round(l(Button::A)), a, "A", "", -0.45));
            pads.push(leap(Kind::Round(l(Button::B)), b, "B", "", -0.45));
            let small =
                |dx: f32, dy: f32| Rect::from_center_size(dpad_c + vec2(dx * u, dy * u), vec2(0.72 * u, 0.92 * u));
            pads.push(leap(Kind::Round(l(Button::Home)), small(-1.35, -1.95), "", "HOME", -0.2));
            pads.push(leap(Kind::Round(l(Button::Hint)), small(-0.4, -2.5), "", "HINT", 0.0));
            pads.push(leap(Kind::Round(l(Button::Pause)), small(0.75, -2.35), "", "PAUSE", 0.1));
        }
    }
    pads
}

const GREEN: Color32 = Color32::from_rgb(46, 170, 82);
const RED: Color32 = Color32::from_rgb(214, 64, 58);
const BLUE: Color32 = Color32::from_rgb(52, 120, 220);
/// The Leapster's periwinkle buttons and its silver plastic.
const LEAP_BLUE: Color32 = Color32::from_rgb(84, 108, 220);
const LEAP_BLUE_DARK: Color32 = Color32::from_rgb(52, 68, 160);
const LEAP_SILVER: Color32 = Color32::from_rgb(196, 200, 210);
const LEAP_SILVER_DOWN: Color32 = Color32::from_rgb(150, 154, 166);
/// The blue print on the body (HOME, HINT, PAUSE), on the silver body and
/// over the picture (Android).
const LEAP_PRINT: Color32 = Color32::from_rgb(64, 84, 196);
const LEAP_PRINT_LIGHT: Color32 = Color32::from_rgb(150, 168, 245);
/// The Leapster's controls drawn on its silver body, with oval A and B
/// (desktop). Android keeps the earlier look: loose round buttons.
const SHELL: bool = !cfg!(target_os = "android");
const LEAP_BODY: Color32 = Color32::from_rgb(178, 181, 190);
const LEAP_BODY_EDGE: Color32 = Color32::from_rgb(120, 123, 134);
const LEAP_RECESS: Color32 = Color32::from_rgb(160, 163, 174);

/// What a finger at `p` is on (the D-pad's side; round buttons answer a
/// little outside their edge, as fingers are not precise, the nearest
/// one where two reach).
pub fn hit(pads: &[Pad], p: Pos2) -> Option<Target> {
    let other = pads.iter().find_map(|pad| match pad.kind {
        Kind::Dpad(ts) => pad.rect.contains(p).then(|| dpad_side(pad.rect, p).map(|s| ts[s])).flatten(),
        Kind::Pill(t) => pad.rect.expand(4.0).contains(p).then_some(t),
        Kind::Round(_) | Kind::Body { .. } => None,
    });
    let round = pads
        .iter()
        .filter_map(|pad| match pad.kind {
            Kind::Round(t) => {
                let d = p.distance(pad.rect.center()) / (pad.rect.width().max(pad.rect.height()) * 0.62);
                (d <= 1.0).then_some((d, t))
            }
            _ => None,
        })
        .reduce(|x, y| if SHELL && y.0 < x.0 { y } else { x });
    other.or(round.map(|(_, t)| t))
}

/// An oval's outline: radii `rx`, `ry` around `c`, turned by `tilt`.
fn oval(c: Pos2, rx: f32, ry: f32, tilt: f32) -> Vec<Pos2> {
    let (s, co) = tilt.sin_cos();
    (0..40)
        .map(|i| {
            let t = i as f32 / 40.0 * std::f32::consts::TAU;
            let (x, y) = (rx * t.cos(), ry * t.sin());
            c + vec2(x * co - y * s, x * s + y * co)
        })
        .collect()
}

/// A capsule from `a` to `b`, `r` wide each side (the recess A and B sit in).
fn capsule(a: Pos2, b: Pos2, r: f32) -> Vec<Pos2> {
    let dir = (b - a).normalized();
    let n = vec2(-dir.y, dir.x);
    let half = |c: Pos2, from: egui::Vec2| {
        (0..=16).map(move |i| {
            let t = i as f32 / 16.0 * std::f32::consts::PI;
            c + (from * t.cos() - vec2(-from.y, from.x) * t.sin()) * r
        })
    };
    // Around b from n to -n through dir, then around a back to n.
    let mut points: Vec<Pos2> = half(b, n).collect();
    points.extend(half(a, -n));
    points
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
                let arrow = if p.leap {
                    // Solid, like the plastic (these sit beside the picture, not
                    // over it): a blue ring around one silver cross, outlines
                    // first so the fills hide them where the bars cross. The
                    // arm held goes darker, pressed in.
                    painter.circle(c, r.width() / 2.0, LEAP_BLUE, Stroke::new(3.0, LEAP_BLUE_DARK));
                    painter.circle_stroke(c, r.width() / 2.0 - 5.0, Stroke::new(2.0, Color32::from_white_alpha(40)));
                    let len = r.width() * 0.8;
                    let bars = [Rect::from_center_size(c, vec2(arm, len)), Rect::from_center_size(c, vec2(len, arm))];
                    let outline = Stroke::new(2.5, Color32::from_gray(60));
                    for bar in bars {
                        painter.rect_stroke(bar, arm * 0.3, outline, egui::StrokeKind::Outside);
                    }
                    for bar in bars {
                        painter.rect_filled(bar, arm * 0.3, LEAP_SILVER);
                    }
                    for (a, t) in arms.iter().zip(ts) {
                        if pad.holds(t) {
                            let a = a.intersect(bars[0].union(bars[1]));
                            painter.rect_filled(a, arm * 0.3, LEAP_SILVER_DOWN);
                        }
                    }
                    painter.circle_filled(c, arm * 0.3, LEAP_SILVER_DOWN);
                    Color32::from_gray(85)
                } else {
                    for (a, t) in arms.iter().zip(ts) {
                        painter.rect(*a, arm * 0.25, fill(p.color, pad.holds(t)), edge, egui::StrokeKind::Inside);
                    }
                    text
                };
                // Arrows drawn as triangles: the fonts have ◀ and ▶ but no ▲ or
                // ▼ (up and down showed as empty boxes).
                for a in &arms {
                    let at = a.center() + (a.center() - c) * 0.25;
                    let dir = (a.center() - c).normalized();
                    let side = egui::vec2(-dir.y, dir.x);
                    let size = arm * 0.16;
                    let tip = at + dir * size;
                    let base = at - dir * size;
                    let points = vec![tip, base + side * size, base - side * size];
                    painter.add(egui::Shape::convex_polygon(points, arrow, Stroke::NONE));
                }
            }
            Kind::Round(t) => {
                let r = p.rect.width() / 2.0;
                let c = p.rect.center();
                if p.leap && !SHELL {
                    let down = pad.holds(t);
                    let face = if down { LEAP_BLUE_DARK } else { LEAP_BLUE };
                    painter.circle(c, r, face, Stroke::new(2.5, LEAP_BLUE_DARK));
                    if !down {
                        // A shine on the upper half: the button is domed.
                        painter.circle_filled(c - vec2(0.0, r * 0.3), r * 0.55, Color32::from_white_alpha(28));
                    }
                    painter.text(c, egui::Align2::CENTER_CENTER, p.label, egui::FontId::proportional(r * 0.8), text);
                    if !p.caption.is_empty() {
                        let at = pos2(c.x, p.rect.top() - r * 0.2);
                        painter.text(at, egui::Align2::CENTER_BOTTOM, p.caption, egui::FontId::proportional(r * 0.55), LEAP_PRINT_LIGHT);
                    }
                    continue;
                }
                if p.leap {
                    let (rx, ry) = (p.rect.width() / 2.0, p.rect.height() / 2.0);
                    let down = pad.holds(t);
                    let face = if down { LEAP_BLUE_DARK } else { LEAP_BLUE };
                    // A dark rim, the face, and a shine on the upper part: domed.
                    let rim = oval(c, rx, ry, p.tilt);
                    painter.add(egui::Shape::convex_polygon(rim, face, Stroke::new(2.5, LEAP_BLUE_DARK)));
                    if !down {
                        let (s, co) = p.tilt.sin_cos();
                        let up = vec2(ry * 0.3 * s, -ry * 0.3 * co);
                        let shine = oval(c + up, rx * 0.55, ry * 0.5, p.tilt);
                        painter.add(egui::Shape::convex_polygon(shine, Color32::from_white_alpha(30), Stroke::NONE));
                    }
                    // The letters are moulded in silver.
                    let size = rx * 1.15;
                    painter.text(
                        c,
                        egui::Align2::CENTER_CENTER,
                        p.label,
                        egui::FontId::proportional(size),
                        LEAP_SILVER,
                    );
                    if !p.caption.is_empty() {
                        let at = pos2(c.x, p.rect.top() - ry * 0.12);
                        painter.text(
                            at,
                            egui::Align2::CENTER_BOTTOM,
                            p.caption,
                            egui::FontId::proportional(rx * 0.85),
                            LEAP_PRINT,
                        );
                    }
                    continue;
                }
                painter.circle(c, r, fill(p.color, pad.holds(t)), edge);
                let size = if p.label.chars().count() > 2 { r * 0.5 } else { r * 0.8 };
                painter.text(c, egui::Align2::CENTER_CENTER, p.label, egui::FontId::proportional(size), text);
            }
            Kind::Body { nw, ne, grille } => paint_body(painter, pads, p.rect, nw, ne, grille),
            Kind::Pill(t) => {
                let h = p.rect.height();
                painter.rect(p.rect, h / 2.0, fill(p.color, pad.holds(t)), edge, egui::StrokeKind::Inside);
                painter.text(
                    p.rect.center(),
                    egui::Align2::CENTER_CENTER,
                    p.label,
                    egui::FontId::proportional(h * 0.45),
                    text,
                );
            }
        }
    }
}

/// Leapster plastic from the bottom of the window, the top corners asked
/// for rounded; with `grille`, the speaker grille above A and B, which sit
/// in one recess.
fn paint_body(painter: &egui::Painter, pads: &[Pad], r: Rect, nw: bool, ne: bool, grille: bool) {
    let u = r.height() / 5.8;
    let round = (1.6 * u).min(255.0) as u8;
    let corners =
        egui::CornerRadius { nw: if nw { round } else { 0 }, ne: if ne { round } else { 0 }, ..Default::default() };
    painter.rect(r, corners, LEAP_BODY, Stroke::new(2.0, LEAP_BODY_EDGE), egui::StrokeKind::Inside);
    // Light along the top edge, as on the moulded plastic.
    painter.rect_stroke(
        r.shrink(3.0),
        corners,
        Stroke::new(1.5, Color32::from_white_alpha(60)),
        egui::StrokeKind::Inside,
    );
    if !grille {
        return;
    }
    let (a, b) = (pads.iter().find(|p| p.label == "A"), pads.iter().find(|p| p.label == "B"));
    if let (Some(a), Some(b)) = (a, b) {
        let outline = capsule(a.rect.center(), b.rect.center(), a.rect.height() * 0.56);
        painter.add(egui::Shape::convex_polygon(outline, LEAP_RECESS, Stroke::new(1.5, LEAP_BODY_EDGE)));
        // The grille: a round recess with slots across it.
        let c = pos2(a.rect.center().x + 0.55 * u, r.top() + 1.45 * u);
        let gr = 0.95 * u;
        painter.circle(c, gr, LEAP_RECESS, Stroke::new(1.5, LEAP_BODY_EDGE));
        let slots = 8;
        for i in 0..slots {
            let y = (i as f32 + 0.5) / slots as f32 * 1.5 - 0.75;
            let half = (0.8f32.powi(2) - y * y).max(0.0).sqrt() * gr;
            let at = c.y + y * gr;
            painter.line_segment(
                [pos2(c.x - half, at), pos2(c.x + half, at)],
                Stroke::new(0.08 * u, Color32::from_gray(88)),
            );
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
                        Kind::Body { .. } => {}
                    }
                }
            }
        }
    }

    #[test]
    fn fingers_slide_between_directions_and_let_go() {
        let area = Rect::from_min_size(Pos2::ZERO, vec2(400.0, 800.0));
        let pads = layout(Hardware::Leapster, area);
        let dpad = pads.iter().find(|p| matches!(p.kind, Kind::Dpad(_))).unwrap();
        let r = dpad.rect;
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
