//! Console view: the emulated screen inside a drawing of the handheld, with
//! its controls around it, clickable. A presentation feature only: it turns
//! pointer input into the same button presses the keyboard makes and never
//! touches the emulation. Each console supplies a `Layout` (data, below);
//! consoles without one show the plain screen.
//!
//! Everything is drawn with vector shapes (Ribbit's own artwork, no
//! photographs or logos). The shell is painted as rings around the screen,
//! never over it, so the game picture is never covered, and the screen keeps
//! its own shape whatever the window's.
//!
//! Positions:
//! * RockIt Twist: from the layout picture the project owner supplied (a
//!   photograph of the front, annotated with keys), measured in pixels; the
//!   controls are the ones its Parent's Guide and factory test name. Power
//!   and volume are on the right edge per the Parent's Guide (side positions
//!   approximate).
//! * Leapster: from the 2005 Parent Guide's diagram (Pause, Hint and Home
//!   above the D-pad on the left, brightness and contrast at the bottom
//!   left, A and B on the right, volume at the bottom right); proportions
//!   APPROXIMATE (no measured drawing). Used for the Leapster, L-MAX and
//!   Leapster 2, whose cases differ: the arrangement, not the case, is
//!   what this shows for those.

use egui::{Color32, Pos2, Rect, Stroke, Vec2, pos2, vec2};
use ribbit_memory::known::Hardware;
use ribbit_twist::input::{Button as TwistButton, Rotary};

/// What a control drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Target {
    Leapster(ribbit_input::Button),
    Twist(TwistButton),
}

/// How a control behaves under the pointer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Action {
    /// Held while the pointer is down on it.
    Press(Target),
    /// A four-way pad: the side nearest the pointer (up, down, left, right).
    Dpad([Target; 4]),
    /// A rocker: the half under the pointer (left, right).
    Rocker([Target; 2]),
    /// A wheel turned by dragging (or clicking one end, or the mouse wheel):
    /// `vertical` dials turn with up/down drags, others go round.
    Wheel { rotary: Rotary, vertical: bool },
    /// The RockIt Twist's touch strip: the finger's position along it.
    Strip,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Shape {
    Circle,
    /// Rounded rectangle, corner radius as a fraction of the shorter side.
    Rounded(f32),
    /// A plus-shaped pad.
    Cross,
}

/// Where a button's keyboard key is drawn, relative to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapAt {
    Center,
    Below,
    Above,
    Left,
    Right,
}

/// What a keycap on the console view stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapFor {
    Press(Target),
    /// Turning a control one way (+1: clockwise / upward).
    Turn(Rotary, i8),
    /// Swiping the touch strip (true: up).
    Swipe(bool),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Control {
    pub name: &'static str,
    /// In the shell's unit square (0..1 across, 0..1 down).
    pub rect: Rect,
    pub shape: Shape,
    pub color: Color32,
    /// A symbol drawn on it.
    pub glyph: &'static str,
    /// Where its key is shown (buttons; the others place theirs).
    pub cap_at: CapAt,
    pub action: Action,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    pub name: &'static str,
    /// Width / height of the shell.
    pub aspect: f32,
    /// The screen, in the unit square.
    pub screen: Rect,
    /// Width / height of the emulated picture.
    pub screen_aspect: f32,
    pub body: Color32,
    pub face: Color32,
    /// The face (inner panel), in the unit square, and its corner radius
    /// (fraction of the shell height).
    pub face_rect: Rect,
    pub corner: f32,
    pub bezel: Color32,
    pub controls: Vec<Control>,
    /// Speaker grille position (unit square), if drawn.
    pub speaker: Option<Pos2>,
    /// Recesses the controls sit in, drawn under them.
    pub housings: Vec<Housing>,
}

/// A recess in the face (a well a control sits in, a tray, an LED slot),
/// optionally edged with a glowing light as on the RockIt Twist.
#[derive(Debug, Clone, PartialEq)]
pub struct Housing {
    /// In the unit square.
    pub rect: Rect,
    /// Corner radius, as a fraction of the shorter side (0.5: round ends).
    pub round: f32,
    pub fill: Color32,
    pub glow: Option<Color32>,
}

const fn rgb(r: u8, g: u8, b: u8) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// A rectangle from pixel measurements of a reference picture whose shell
/// spans `ox..ox+w`, `oy..oy+h`.
fn px(x0: f32, y0: f32, x1: f32, y1: f32, o: (f32, f32, f32, f32)) -> Rect {
    let (ox, oy, w, h) = o;
    Rect::from_min_max(pos2((x0 - ox) / w, (y0 - oy) / h), pos2((x1 - ox) / w, (y1 - oy) / h))
}

/// A circle of radius `r` (fraction of the shell height) at `c`.
fn circle(c: Pos2, r: f32, aspect: f32) -> Rect {
    Rect::from_center_size(c, vec2(2.0 * r / aspect, 2.0 * r))
}

/// The layout for a console, if it has one.
pub fn layout_for(hw: Hardware) -> Option<Layout> {
    match hw {
        Hardware::RockItTwist => Some(rockit_twist()),
        Hardware::Leapster | Hardware::LeapsterLmax | Hardware::Leapster2 => Some(leapster()),
        // TV consoles: no handheld to draw.
        Hardware::LeapsterTv | Hardware::Zippity => None,
    }
}

/// The RockIt Twist, measured from the owner's reference picture: the
/// shell spans x 330-1660, y 210-1590 (1330 x 1380 pixels).
pub fn rockit_twist() -> Layout {
    let o = (330.0, 210.0, 1330.0, 1380.0);
    let aspect = o.2 / o.3;
    let t = |b| Target::Twist(b);
    let press = |b| Action::Press(Target::Twist(b));
    // The LCD (x 695-1295, y 645-1105): 4:3, its width set from its height
    // so the picture keeps its shape exactly.
    let (top, h) = ((645.0 - o.1) / o.3, 460.0 / o.3);
    let w = h * (4.0 / 3.0) / aspect;
    let cx = (995.0 - o.0) / o.2;
    let screen = Rect::from_min_size(pos2(cx - w / 2.0, top), vec2(w, h));
    let mut controls = vec![
        Control {
            name: "Switch",
            rect: px(600.0, 322.0, 900.0, 468.0, o),
            shape: Shape::Rounded(0.5),
            color: rgb(150, 100, 225),
            glyph: "",
            cap_at: CapAt::Center,
            action: Action::Rocker([t(TwistButton::SwitchLeft), t(TwistButton::SwitchRight)]),
        },
        Control {
            name: "Spinner",
            rect: circle(pos2((1490.0 - o.0) / o.2, (560.0 - o.1) / o.3), 145.0 / o.3, aspect),
            shape: Shape::Circle,
            color: rgb(70, 170, 235),
            glyph: "⟳",
            cap_at: CapAt::Center,
            action: Action::Wheel { rotary: Rotary::Spinner, vertical: false },
        },
        Control {
            name: "Slider",
            rect: px(1378.0, 778.0, 1552.0, 1192.0, o),
            shape: Shape::Rounded(0.5),
            color: rgb(240, 130, 20),
            glyph: "⇕",
            cap_at: CapAt::Center,
            action: Action::Strip,
        },
        Control {
            name: "D-pad",
            rect: circle(pos2((485.0 - o.0) / o.2, (865.0 - o.1) / o.3), 168.0 / o.3, aspect),
            shape: Shape::Cross,
            color: rgb(225, 45, 45),
            glyph: "",
            cap_at: CapAt::Center,
            action: Action::Dpad([
                t(TwistButton::DpadUp),
                t(TwistButton::DpadDown),
                t(TwistButton::DpadLeft),
                t(TwistButton::DpadRight),
            ]),
        },
        Control {
            name: "Hint",
            rect: px(462.0, 1095.0, 568.0, 1238.0, o),
            shape: Shape::Rounded(0.45),
            color: rgb(45, 45, 48),
            glyph: "?",
            cap_at: CapAt::Right,
            action: press(TwistButton::Hint),
        },
        Control {
            name: "Home",
            rect: circle(pos2((615.0 - o.0) / o.2, (1300.0 - o.1) / o.3), 62.0 / o.3, aspect),
            shape: Shape::Rounded(0.3),
            color: rgb(45, 45, 48),
            glyph: "⌂",
            cap_at: CapAt::Right,
            action: press(TwistButton::Home),
        },
        Control {
            name: "Check mark",
            rect: circle(pos2((1420.0 - o.0) / o.2, (1305.0 - o.1) / o.3), 56.0 / o.3, aspect),
            shape: Shape::Circle,
            color: rgb(40, 150, 60),
            glyph: "✔",
            cap_at: CapAt::Left,
            action: press(TwistButton::Check),
        },
        // Right edge (Parent's Guide: power, then volume, down the side).
        Control {
            name: "Power",
            rect: px(1638.0, 610.0, 1668.0, 690.0, o),
            shape: Shape::Rounded(0.5),
            color: rgb(95, 185, 45),
            glyph: "",
            cap_at: CapAt::Left,
            action: press(TwistButton::Power),
        },
        Control {
            name: "Volume up",
            rect: px(1638.0, 1010.0, 1668.0, 1070.0, o),
            shape: Shape::Rounded(0.5),
            color: rgb(95, 185, 45),
            glyph: "",
            cap_at: CapAt::Left,
            action: press(TwistButton::VolumeUp),
        },
        Control {
            name: "Volume down",
            rect: px(1638.0, 1078.0, 1668.0, 1138.0, o),
            shape: Shape::Rounded(0.5),
            color: rgb(95, 185, 45),
            glyph: "",
            cap_at: CapAt::Left,
            action: press(TwistButton::VolumeDown),
        },
    ];
    // The bottom dials (rollers), over the case's lower edge.
    for (name, x, color, rotary) in [
        ("Yellow dial", 785.0, rgb(240, 215, 40), Rotary::YellowDial),
        ("Green dial", 997.0, rgb(60, 200, 70), Rotary::GreenDial),
        ("Red dial", 1205.0, rgb(215, 40, 40), Rotary::RedDial),
    ] {
        controls.push(Control {
            name,
            rect: px(x - 40.0, 1328.0, x + 40.0, 1575.0, o),
            shape: Shape::Rounded(0.45),
            color,
            glyph: "",
            cap_at: CapAt::Center,
            action: Action::Wheel { rotary, vertical: true },
        });
    }
    // The 3 x 3 grid (centres x 1045/1138/1230, y 280/375/468).
    const GRID: [&str; 9] = ["Grid 1", "Grid 2", "Grid 3", "Grid 4", "Grid 5", "Grid 6", "Grid 7", "Grid 8", "Grid 9"];
    for (i, name) in GRID.into_iter().enumerate() {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        let c = pos2((1045.0 + 92.5 * col - o.0) / o.2, (280.0 + 94.0 * row - o.1) / o.3);
        controls.push(Control {
            name,
            rect: circle(c, 34.0 / o.3, aspect),
            shape: Shape::Circle,
            color: rgb(55, 185, 75),
            glyph: "",
            cap_at: CapAt::Center,
            action: press(TwistButton::Grid(i as u8)),
        });
    }
    Layout {
        name: "RockIt Twist",
        aspect,
        screen,
        screen_aspect: 4.0 / 3.0,
        body: rgb(130, 205, 60),
        face: rgb(24, 24, 26),
        face_rect: px(392.0, 268.0, 1602.0, 1452.0, o),
        corner: 0.16,
        bezel: rgb(70, 160, 40),
        controls,
        speaker: Some(pos2((515.0 - o.0) / o.2, (575.0 - o.1) / o.3)),
        housings: {
            let well = |rect, round, glow| Housing { rect, round, fill: rgb(14, 14, 16), glow };
            let mut h = vec![
                // The grid's tray, edged in green light.
                well(px(966.0, 226.0, 1306.0, 546.0, o), 0.16, Some(rgb(80, 235, 70))),
                // The Switch's well.
                well(px(584.0, 306.0, 916.0, 484.0, o), 0.5, None),
                // The Slider's track, edged in orange light.
                well(px(1362.0, 760.0, 1568.0, 1210.0, o), 0.5, Some(rgb(255, 150, 30))),
                // Wells round the Spinner and the D-pad.
                well(circle(pos2((1490.0 - o.0) / o.2, (560.0 - o.1) / o.3), 160.0 / o.3, aspect), 0.5, None),
                well(circle(pos2((485.0 - o.0) / o.2, (865.0 - o.1) / o.3), 182.0 / o.3, aspect), 0.5, None),
            ];
            // The LEDs above the dials.
            for x in [785.0, 995.0, 1205.0] {
                h.push(well(px(x - 9.0, 1222.0, x + 9.0, 1268.0, o), 0.5, None));
            }
            h
        },
    }
}

/// The original Leapster, after the Parent Guide's diagram (approximate).
pub fn leapster() -> Layout {
    use ribbit_input::Button as B;
    let aspect = 2.0;
    let l = |b| Target::Leapster(b);
    let press = |b| Action::Press(Target::Leapster(b));
    // The 160 x 160 LCD, square, centred a little high.
    let h = 0.70;
    let screen = Rect::from_center_size(pos2(0.5, 0.47), vec2(h / aspect, h));
    let small = |name, c: Pos2, glyph, b| Control {
        name,
        rect: circle(c, 0.045, aspect),
        shape: Shape::Circle,
        color: rgb(70, 140, 70),
        glyph,
        cap_at: CapAt::Right,
        action: press(b),
    };
    let tiny = |name, c: Pos2, glyph, b| Control {
        name,
        rect: Rect::from_center_size(c, vec2(0.07 / aspect, 0.045)),
        shape: Shape::Rounded(0.5),
        color: rgb(150, 155, 165),
        glyph,
        cap_at: CapAt::Above,
        action: press(b),
    };
    let controls = vec![
        small("Pause", pos2(0.10, 0.17), "⏸", B::Pause),
        small("Hint", pos2(0.10, 0.30), "?", B::Hint),
        small("Home", pos2(0.10, 0.43), "⌂", B::Home),
        Control {
            name: "D-pad",
            rect: circle(pos2(0.17, 0.68), 0.17, aspect),
            shape: Shape::Cross,
            color: rgb(60, 62, 70),
            glyph: "",
            cap_at: CapAt::Center,
            action: Action::Dpad([l(B::Up), l(B::Down), l(B::Left), l(B::Right)]),
        },
        tiny("Brightness down", pos2(0.055, 0.93), "−", B::BrightnessDown),
        tiny("Brightness up", pos2(0.105, 0.93), "+", B::BrightnessUp),
        tiny("Contrast", pos2(0.175, 0.93), "◐", B::Contrast),
        Control {
            name: "A",
            rect: circle(pos2(0.80, 0.64), 0.075, aspect),
            shape: Shape::Circle,
            color: rgb(70, 160, 70),
            glyph: "A",
            cap_at: CapAt::Below,
            action: press(B::A),
        },
        Control {
            name: "B",
            rect: circle(pos2(0.885, 0.48), 0.075, aspect),
            shape: Shape::Circle,
            color: rgb(70, 160, 70),
            glyph: "B",
            cap_at: CapAt::Below,
            action: press(B::B),
        },
        tiny("Volume down", pos2(0.84, 0.93), "−", B::VolumeDown),
        tiny("Volume up", pos2(0.89, 0.93), "+", B::VolumeUp),
    ];
    Layout {
        name: "Leapster",
        aspect,
        screen,
        screen_aspect: 1.0,
        body: rgb(80, 165, 75),
        face: rgb(196, 200, 208),
        face_rect: Rect::from_min_max(pos2(0.035, 0.07), pos2(0.965, 0.97)),
        corner: 0.2,
        bezel: rgb(110, 115, 128),
        controls,
        speaker: Some(pos2(0.93, 0.18)),
        housings: vec![
            Housing { rect: circle(pos2(0.17, 0.68), 0.19, aspect), round: 0.5, fill: rgb(150, 154, 164), glow: None },
            Housing {
                rect: Rect::from_min_max(pos2(0.745, 0.37), pos2(0.94, 0.75)),
                round: 0.5,
                fill: rgb(170, 174, 184),
                glow: None,
            },
        ],
    }
}

/// Where the shell goes in `area`: as large as fits, its shape kept,
/// centred, with a small margin.
pub fn shell_rect(layout: &Layout, area: Rect) -> Rect {
    let margin = (area.width().min(area.height()) * 0.02).min(12.0);
    let a = area.shrink(margin);
    let w = a.width().min(a.height() * layout.aspect).max(1.0);
    Rect::from_center_size(a.center(), vec2(w, w / layout.aspect))
}

/// A unit-square rectangle placed in `shell`.
pub fn place(shell: Rect, r: Rect) -> Rect {
    Rect::from_min_max(
        pos2(shell.left() + r.left() * shell.width(), shell.top() + r.top() * shell.height()),
        pos2(shell.left() + r.right() * shell.width(), shell.top() + r.bottom() * shell.height()),
    )
}

/// The control at `p` (screen points) in a shell placed at `shell`: the
/// last one listed wins where they overlap. (egui does the hit-testing in
/// the app; this states the same rule for the tests.)
#[cfg(test)]
fn hit(layout: &Layout, shell: Rect, p: Pos2) -> Option<usize> {
    layout.controls.iter().rposition(|c| inside(c.shape, place(shell, c.rect), p))
}

fn inside(shape: Shape, r: Rect, p: Pos2) -> bool {
    match shape {
        Shape::Circle => {
            let d = p - r.center();
            (d.x / (r.width() / 2.0)).powi(2) + (d.y / (r.height() / 2.0)).powi(2) <= 1.0
        }
        Shape::Rounded(_) => r.contains(p),
        Shape::Cross => {
            let d = p - r.center();
            let (hw, hh) = (r.width() / 2.0, r.height() / 2.0);
            let arm = 0.36;
            (d.x.abs() <= hw && d.y.abs() <= hh * arm) || (d.y.abs() <= hh && d.x.abs() <= hw * arm)
        }
    }
}

/// Which side of a four-way pad the pointer is on (index into
/// `Action::Dpad`), or None in its centre.
pub fn dpad_side(r: Rect, p: Pos2) -> Option<usize> {
    let d = p - r.center();
    let (nx, ny) = (d.x / (r.width() / 2.0), d.y / (r.height() / 2.0));
    if nx.abs().max(ny.abs()) < 0.15 {
        return None;
    }
    Some(if ny.abs() >= nx.abs() {
        if ny < 0.0 { 0 } else { 1 }
    } else if nx < 0.0 {
        2
    } else {
        3
    })
}

/// What the pointer did this frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Press(Target, bool),
    Turn(Rotary, i32),
    /// A finger on the touch strip (0 = top) or lifted.
    Strip(Option<f32>),
    /// A swipe on the strip from the mouse wheel (true = up).
    Swipe(bool),
}

/// Drag distance (points) or angle (radians) per detent.
const DRAG_STEP: f32 = 14.0;
const ANGLE_STEP: f32 = std::f32::consts::PI / 6.0;

/// The pointer's state between frames.
#[derive(Default)]
pub struct ConsoleView {
    /// Targets the pointer holds now.
    held: Vec<Target>,
    /// The control being dragged, and how far it has gone (drag points or
    /// radians) since its last detent.
    drag: Option<(usize, f32, Option<f32>)>,
    strip_down: bool,
    wheel: f32,
}

/// What the view shows on its controls.
pub struct Lit<'a> {
    pub pressed: &'a dyn Fn(Target) -> bool,
    pub turned: &'a dyn Fn(Rotary) -> i32,
    /// The way a key is turning a control now.
    pub turning: &'a dyn Fn(Rotary) -> Option<i8>,
    pub strip: Option<f32>,
    /// A swipe in progress (true: up).
    pub swiping: Option<bool>,
    /// The keyboard key for a control, as its keycap shows it.
    pub cap: &'a dyn Fn(CapFor) -> Option<String>,
    pub show_caps: bool,
    /// The keys for a control, for its tooltip.
    pub keys: &'a dyn Fn(&Control) -> String,
}

/// Scrolling (points) per notch of a mouse wheel.
const NOTCH: f32 = 40.0;

impl ConsoleView {
    /// Whole wheel notches in `scroll` plus what was left over (+1 = up).
    fn notches(&mut self, scroll: f32) -> Vec<i32> {
        self.wheel += scroll;
        let mut out = Vec::new();
        while self.wheel.abs() >= NOTCH {
            let s = self.wheel.signum();
            self.wheel -= NOTCH * s;
            out.push(s as i32);
        }
        out
    }

    /// Lets go of everything the pointer held (events to apply).
    pub fn release_all(&mut self) -> Vec<Event> {
        let mut out: Vec<Event> = self.held.drain(..).map(|t| Event::Press(t, false)).collect();
        if std::mem::take(&mut self.strip_down) {
            out.push(Event::Strip(None));
        }
        self.drag = None;
        out
    }

    /// Draws the console in `area` and handles the pointer; returns where
    /// the screen is (for the picture) and what the pointer did. `under` is
    /// a slot painted before the rest of the UI, so menus and messages stay
    /// on top.
    pub fn show(
        &mut self,
        ui: &egui::Ui,
        under: egui::layers::ShapeIdx,
        layout: &Layout,
        area: Rect,
        lit: &Lit,
    ) -> (Rect, Vec<Event>) {
        let shell = shell_rect(layout, area);
        let screen = place(shell, layout.screen);
        let shapes = paint(layout, shell, screen, lit, ui.ctx());
        ui.painter().set(under, egui::Shape::Vec(shapes));
        let mut events = Vec::new();
        let mut held = Vec::new();
        // (Smoothed: one notch of a mouse wheel arrives over a few frames.)
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        let mut scroll_used = false;
        for (i, c) in layout.controls.iter().enumerate() {
            let r = place(shell, c.rect);
            let id = egui::Id::new(("console control", layout.name, i));
            let resp = ui.interact(r, id, egui::Sense::click_and_drag());
            let hover = resp.hovered() && ui.ctx().pointer_hover_pos().is_some_and(|p| inside(c.shape, r, p));
            if hover {
                let keys = (lit.keys)(c);
                let tip = if keys.is_empty() { c.name.to_string() } else { format!("{}  ({keys})", c.name) };
                resp.clone().on_hover_text(tip);
            }
            let down = resp.is_pointer_button_down_on();
            let pos = resp.interact_pointer_pos();
            match c.action {
                Action::Press(t) => {
                    if down && self.held.contains(&t) || down && pos.is_some_and(|p| inside(c.shape, r, p)) {
                        held.push(t);
                    }
                }
                Action::Dpad(ts) => {
                    if let (true, Some(p)) = (down, pos)
                        && let Some(side) = dpad_side(r, p)
                    {
                        held.push(ts[side]);
                    }
                }
                Action::Rocker(ts) => {
                    if let (true, Some(p)) = (down, pos) {
                        held.push(ts[if p.x < r.center().x { 0 } else { 1 }]);
                    }
                }
                Action::Wheel { rotary, vertical } => {
                    if hover && scroll != 0.0 {
                        scroll_used = true;
                        for s in self.notches(scroll) {
                            events.push(Event::Turn(rotary, if vertical { s } else { -s }));
                        }
                    }
                    if resp.clicked()
                        && let Some(p) = pos
                    {
                        // A click on one end turns it one detent that way.
                        let s = if vertical {
                            if p.y < r.center().y { 1 } else { -1 }
                        } else if p.x < r.center().x {
                            -1
                        } else {
                            1
                        };
                        events.push(Event::Turn(rotary, s));
                    } else if resp.dragged()
                        && let Some(p) = pos
                    {
                        let (_, acc, last) = self.drag.get_or_insert((i, 0.0, None));
                        if vertical {
                            *acc -= resp.drag_delta().y;
                            while acc.abs() >= DRAG_STEP {
                                let s = acc.signum();
                                *acc -= DRAG_STEP * s;
                                events.push(Event::Turn(rotary, s as i32));
                            }
                        } else {
                            let d = p - r.center();
                            let angle = d.y.atan2(d.x);
                            if let Some(prev) = *last {
                                let mut delta = angle - prev;
                                if delta > std::f32::consts::PI {
                                    delta -= std::f32::consts::TAU;
                                } else if delta < -std::f32::consts::PI {
                                    delta += std::f32::consts::TAU;
                                }
                                *acc += delta;
                                // Screen y grows downward: a positive angle
                                // change is clockwise.
                                while acc.abs() >= ANGLE_STEP {
                                    let s = acc.signum();
                                    *acc -= ANGLE_STEP * s;
                                    events.push(Event::Turn(rotary, s as i32));
                                }
                            }
                            *last = Some(angle);
                        }
                    }
                    if resp.drag_stopped() {
                        self.drag = None;
                    }
                }
                Action::Strip => {
                    if hover && scroll != 0.0 {
                        scroll_used = true;
                        for s in self.notches(scroll) {
                            events.push(Event::Swipe(s > 0));
                        }
                    }
                    match (down, pos) {
                        (true, Some(p)) => {
                            self.strip_down = true;
                            events.push(Event::Strip(Some(((p.y - r.top()) / r.height()).clamp(0.0, 1.0))));
                        }
                        _ if self.strip_down => {
                            self.strip_down = false;
                            events.push(Event::Strip(None));
                        }
                        _ => {}
                    }
                }
            }
        }
        // The mouse wheel anywhere else on a RockIt Twist: the Slider (the
        // owner's layout maps the wheel to it).
        if !scroll_used
            && scroll != 0.0
            && layout.controls.iter().any(|c| c.action == Action::Strip)
            && ui.ctx().pointer_hover_pos().is_some_and(|p| shell.contains(p))
            && ui
                .ctx()
                .layer_id_at(ui.ctx().pointer_hover_pos().unwrap())
                .is_none_or(|l| l.order == egui::Order::Background)
        {
            for s in self.notches(scroll) {
                events.push(Event::Swipe(s > 0));
            }
        }
        held.dedup();
        for t in &held {
            if !self.held.contains(t) {
                events.push(Event::Press(*t, true));
            }
        }
        for t in &self.held {
            if !held.contains(t) {
                events.push(Event::Press(*t, false));
            }
        }
        self.held = held;
        (screen, events)
    }
}

// ---- drawing --------------------------------------------------------------
//
// Lit from the top left: every surface is shaded by which way it faces
// (`lightness`), buttons are domes with a highlight and a soft shadow, the
// case is bevelled and the face and screen frame are inset. Presses, turns
// and swipes animate (egui's animation helpers repaint only while moving).

/// Where the light comes from (a unit vector: up and to the left).
const LIGHT: Vec2 = vec2(-0.5547, -0.8321);
/// How long a press, a turn or a light takes to animate (seconds).
const PRESS_TIME: f32 = 0.07;
const TURN_TIME: f32 = 0.12;

/// How much a surface facing `d` (from its centre) faces the light: -1..1.
fn lightness(d: Vec2) -> f32 {
    let n = d.normalized();
    if n.is_finite() { n.x * LIGHT.x + n.y * LIGHT.y } else { 0.0 }
}

fn rr_sdf(r: Rect, radius: f32) -> impl Fn(Pos2) -> f32 {
    let radius = radius.min(r.width().min(r.height()) / 2.0);
    move |p: Pos2| {
        let q = (p - r.center()).abs() - (r.size() / 2.0 - vec2(radius, radius));
        vec2(q.x.max(0.0), q.y.max(0.0)).length() + q.x.max(q.y).min(0.0) - radius
    }
}

fn cross_sdf(r: Rect, arm: f32, radius: f32) -> impl Fn(Pos2) -> f32 {
    let v = rr_sdf(Rect::from_center_size(r.center(), vec2(r.width() * 2.0 * arm, r.height())), radius);
    let h = rr_sdf(Rect::from_center_size(r.center(), vec2(r.width(), r.height() * 2.0 * arm)), radius);
    move |p| v(p).min(h(p))
}

/// Points along a shape's edge (`sdf` <= 0 inside), as seen from `from`
/// (inside it) in `n` evenly spaced directions, up to `reach` away.
fn outline(sdf: impl Fn(Pos2) -> f32, from: Pos2, reach: f32, n: usize) -> Vec<Pos2> {
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * i as f32 / n as f32;
            let d = vec2(a.cos(), a.sin());
            let (mut lo, mut hi) = (0.0, reach);
            for _ in 0..22 {
                let mid = (lo + hi) / 2.0;
                if sdf(from + d * mid) <= 0.0 { lo = mid } else { hi = mid }
            }
            from + d * lo
        })
        .collect()
}

/// Points along a rounded rectangle's edge, as seen from `from` (which must
/// be inside it) in `n` evenly spaced directions.
fn rim(r: Rect, radius: f32, from: Pos2, n: usize) -> Vec<Pos2> {
    outline(rr_sdf(r, radius), from, r.size().length() + (from - r.center()).length(), n)
}

/// The band between two outlines sampled in the same directions, each
/// vertex coloured by `outer` / `inner` (given the direction it faces).
fn band(
    o: &[Pos2],
    h: &[Pos2],
    from: Pos2,
    outer: impl Fn(Vec2) -> Color32,
    inner: impl Fn(Vec2) -> Color32,
) -> egui::Shape {
    let n = o.len();
    let mut mesh = egui::Mesh::default();
    for i in 0..n {
        mesh.colored_vertex(o[i], outer(o[i] - from));
        mesh.colored_vertex(h[i], inner(h[i] - from));
    }
    for i in 0..n {
        let (a, b) = (2 * i as u32, 2 * ((i + 1) % n) as u32);
        mesh.add_triangle(a, a + 1, b);
        mesh.add_triangle(a + 1, b + 1, b);
    }
    egui::Shape::mesh(mesh)
}

/// A filled outline: every edge vertex coloured by its direction, the
/// middle (`hub`) by `middle`.
fn fan(edge: &[Pos2], hub: Pos2, middle: Color32, rim_color: impl Fn(Vec2) -> Color32) -> egui::Shape {
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(hub, middle);
    for &p in edge {
        mesh.colored_vertex(p, rim_color(p - hub));
    }
    let n = edge.len() as u32;
    for i in 0..n {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % n);
    }
    egui::Shape::mesh(mesh)
}

fn shade(c: Color32, f: f32) -> Color32 {
    let m = |v: u8| ((v as f32 * f).round()).clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(m(c.r()), m(c.g()), m(c.b()), c.a())
}

/// `a` towards `b` by `t` (0..1).
fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgba_unmultiplied(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()), m(a.a(), b.a()))
}

/// A colour lit up: brighter and whiter.
fn glow_of(c: Color32) -> Color32 {
    mix(shade(c, 1.6), Color32::WHITE, 0.35)
}

fn alpha(c: Color32, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (a.clamp(0.0, 1.0) * 255.0) as u8)
}

fn text(
    ctx: &egui::Context,
    pos: Pos2,
    anchor: egui::Align2,
    s: &str,
    font: egui::FontId,
    color: Color32,
) -> egui::Shape {
    ctx.fonts_mut(|f| egui::Shape::text(f, pos, anchor, s, font, color))
}

/// A raised control: a soft shadow, a glow while lit, then the dome itself
/// (pushed in by `press`, 0..1), lit from the top left, with a highlight.
fn dome(out: &mut Vec<egui::Shape>, edge: &[Pos2], center: Pos2, base: Color32, press: f32, glow: f32, unit: f32) {
    let depth = unit * 3.0;
    let size = edge.iter().map(|p| (*p - center).length()).fold(0.0, f32::max).max(1.0);
    // Shadow: softer and closer as it goes down.
    let lift = 1.0 - press * 0.75;
    let drop = vec2(unit * 1.5, depth) * lift;
    let blur = unit * 7.0 * lift + unit * 2.0;
    let near: Vec<Pos2> = edge.iter().map(|p| *p + drop * 0.4).collect();
    let far: Vec<Pos2> = edge.iter().map(|p| *p + (*p - center).normalized() * blur + drop).collect();
    out.push(band(
        &far,
        &near,
        center,
        |_| Color32::TRANSPARENT,
        |_| Color32::from_black_alpha((120.0 * lift + 40.0) as u8),
    ));
    // Glow around it while lit.
    if glow > 0.01 {
        let halo: Vec<Pos2> = edge.iter().map(|p| *p + (*p - center).normalized() * unit * 14.0).collect();
        let g = glow_of(base);
        out.push(band(&halo, edge, center, |_| alpha(g, 0.0), |_| alpha(g, 0.75 * glow)));
    }
    let shift = vec2(0.0, depth * press);
    let pts: Vec<Pos2> = edge.iter().map(|p| *p + shift).collect();
    let c = center + shift;
    let face = mix(base, glow_of(base), glow * 0.7);
    // Pressed in, its top faces the light less.
    let top = shade(face, 1.38 - 0.18 * press);
    out.push(fan(&pts, c + LIGHT * size * 0.35, top, |d| shade(face, 0.68 + 0.28 * lightness(d) - 0.1 * press)));
    out.push(egui::Shape::closed_line(pts.clone(), Stroke::new((unit * 1.1).max(1.0), shade(base, 0.42))));
    // A highlight near the top left, inside the shape however long it is.
    let bounds = Rect::from_points(&pts);
    let spot = c + vec2(LIGHT.x * bounds.width(), LIGHT.y * bounds.height()) * 0.27;
    let spot_r = bounds.width().min(bounds.height()) * 0.16;
    out.push(egui::Shape::circle_filled(spot, spot_r, Color32::from_white_alpha((55.0 * (1.0 - press * 0.6)) as u8)));
    out.push(egui::Shape::circle_filled(
        spot,
        spot_r * 0.5,
        Color32::from_white_alpha((80.0 * (1.0 - press * 0.6)) as u8),
    ));
}

/// A keyboard key showing `label`, centred at `at`; lit (and pressed down)
/// by `lit`, 0..1.
fn keycap(out: &mut Vec<egui::Shape>, ctx: &egui::Context, at: Pos2, label: &str, lit: f32, unit: f32) {
    let size = (unit * 10.5).clamp(8.0, 22.0);
    let font = egui::FontId::proportional(size);
    let ink = mix(Color32::from_rgb(38, 40, 46), Color32::from_rgb(40, 28, 0), lit);
    // Arrows are drawn (Ribbit's font has none).
    let arrow = match label {
        "↑" => Some(vec2(0.0, -1.0)),
        "↓" => Some(vec2(0.0, 1.0)),
        "←" => Some(vec2(-1.0, 0.0)),
        "→" => Some(vec2(1.0, 0.0)),
        _ => None,
    };
    let text = if arrow.is_some() { String::new() } else { label.to_string() };
    let galley = ctx.fonts_mut(|f| f.layout_no_wrap(text, font, ink));
    let h = size * 1.75;
    let w = (galley.size().x + size * 0.9).max(h);
    let depth = (unit * 2.6).max(2.0);
    let sink = depth * 0.75 * lit;
    let face = Rect::from_center_size(at + vec2(0.0, sink - depth / 2.0), vec2(w, h));
    let skirt = Rect::from_min_max(face.min, pos2(face.max.x, at.y + h / 2.0 + depth / 2.0));
    let radius = h * 0.22;
    let accent = Color32::from_rgb(255, 205, 60);
    // Shadow, then the glow when lit.
    out.push(egui::Shape::rect_filled(
        skirt.translate(vec2(unit, unit * 1.5)).expand(unit * 0.8),
        radius,
        Color32::from_black_alpha(90),
    ));
    if lit > 0.01 {
        for k in 1..=3 {
            let a = 0.32 * lit / k as f32;
            out.push(egui::Shape::rect_filled(
                skirt.expand(unit * 2.5 * k as f32),
                radius + unit * 2.0 * k as f32,
                alpha(accent, a),
            ));
        }
    }
    // The key's side, then its top: light at the top, darker below.
    let top_color = mix(Color32::from_rgb(244, 245, 248), Color32::from_rgb(255, 236, 150), lit);
    let low_color = mix(Color32::from_rgb(196, 199, 207), accent, lit);
    out.push(egui::Shape::rect_filled(skirt, radius, shade(low_color, 0.62)));
    out.push(egui::Shape::rect_filled(face, radius, low_color));
    let upper =
        Rect::from_min_max(face.min, pos2(face.max.x, face.center().y + h * 0.1)).shrink2(vec2(unit * 0.8, unit * 0.6));
    out.push(egui::Shape::rect_filled(upper, radius * 0.8, alpha(top_color, 0.85)));
    out.push(egui::Shape::rect_stroke(
        face,
        radius,
        Stroke::new(1.0, shade(low_color, 0.55)),
        egui::StrokeKind::Inside,
    ));
    let g = galley.size();
    match arrow {
        Some(d) => {
            let c = face.center() - vec2(0.0, h * 0.03);
            let (tip, back, side) = (c + d * size * 0.42, c - d * size * 0.3, vec2(-d.y, d.x) * size * 0.36);
            out.push(egui::Shape::convex_polygon(vec![tip, back + side, back - side], ink, Stroke::NONE));
        }
        None => out.push(egui::Shape::galley(face.center() - g / 2.0 - vec2(0.0, h * 0.03), galley, ink)),
    }
}

/// Every shape of the console (never inside the screen).
fn paint(layout: &Layout, shell: Rect, screen: Rect, lit: &Lit, ctx: &egui::Context) -> Vec<egui::Shape> {
    const N: usize = 160;
    let mut out = Vec::new();
    let h = shell.height();
    let unit = h / 700.0;
    let bezel_pad = h * 0.025;
    let bezel = screen.expand(bezel_pad);
    let face = place(shell, layout.face_rect);
    let from = bezel.center();
    let body_r = h * layout.corner * 1.25;
    // A soft shadow around the case (a band: nothing is drawn under it,
    // so the screen stays clear).
    let case = rim(shell, body_r, from, N);
    let drop = vec2(unit * 4.0, unit * 9.0);
    let shadow = rim(shell.expand(unit * 14.0).translate(drop), body_r + unit * 14.0, from, N);
    out.push(band(&shadow, &case, from, |_| Color32::TRANSPARENT, |_| Color32::from_black_alpha(130)));
    // The case, bevelled: bright where its edge faces the light.
    let hole = rim(bezel, bezel_pad, from, N);
    let lip = rim(shell.shrink(unit * 9.0), body_r - unit * 9.0, from, N);
    out.push(band(&lip, &hole, from, |_| layout.body, |_| layout.body));
    out.push(band(
        &case,
        &lip,
        from,
        |d| shade(layout.body, 0.9 + 0.35 * lightness(d)),
        |d| shade(layout.body, 1.0 - 0.12 * lightness(d)),
    ));
    out.push(egui::Shape::closed_line(case.clone(), Stroke::new(1.0, shade(layout.body, 0.55))));
    // The face, set in: its top-left edge in shadow.
    let face_edge = rim(face, h * layout.corner, from, N);
    let face_in = rim(face.shrink(unit * 10.0), h * layout.corner - unit * 10.0, from, N);
    out.push(band(&face_in, &hole, from, |_| layout.face, |_| layout.face));
    out.push(band(&face_edge, &face_in, from, |d| shade(layout.face, 0.7 - 0.35 * lightness(d)), |_| layout.face));
    out.push(egui::Shape::closed_line(face_edge, Stroke::new(1.0, shade(layout.body, 0.75))));
    // The screen's frame: raised, the screen set into it.
    let frame_out = rim(bezel, bezel_pad * 1.4, from, N);
    let glass = rim(screen, 2.0, from, N);
    let frame_mid = rim(screen.expand(bezel_pad * 0.45), bezel_pad * 0.6, from, N);
    out.push(band(&frame_out, &frame_mid, from, |d| shade(layout.bezel, 0.95 + 0.3 * lightness(d)), |_| layout.bezel));
    out.push(band(
        &frame_mid,
        &glass,
        from,
        |_| shade(layout.bezel, 0.8),
        |d| shade(layout.bezel, 0.55 - 0.3 * lightness(d)),
    ));
    // Ribbit's own mark on the frame.
    out.push(text(
        ctx,
        pos2(bezel.center().x, bezel.top() + bezel_pad * 0.5),
        egui::Align2::CENTER_CENTER,
        "RIBBIT",
        egui::FontId::proportional((bezel_pad * 0.8).max(6.0)),
        shade(layout.bezel, 1.7),
    ));
    if let Some(sp) = layout.speaker {
        let c = pos2(shell.left() + sp.x * shell.width(), shell.top() + sp.y * shell.height());
        for k in 1..=3 {
            let r = h * 0.018 * k as f32;
            out.push(egui::Shape::circle_stroke(
                c + vec2(0.0, unit),
                r,
                Stroke::new((h * 0.006).max(1.0), shade(layout.face, 1.6)),
            ));
            out.push(egui::Shape::circle_stroke(c, r, Stroke::new((h * 0.006).max(1.0), shade(layout.face, 0.45))));
        }
    }
    for hs in &layout.housings {
        housing(&mut out, hs, place(shell, hs.rect), unit);
    }
    let mut caps: Vec<(Pos2, String, f32)> = Vec::new();
    let mut cap = |what: CapFor, at: Pos2, on: f32| {
        if lit.show_caps
            && let Some(label) = (lit.cap)(what)
        {
            caps.push((at, label, on));
        }
    };
    for (i, c) in layout.controls.iter().enumerate() {
        let r = place(shell, c.rect);
        let id = |k: usize| egui::Id::new(("console anim", layout.name, i, k));
        let anim = |k: usize, on: bool| ctx.animate_bool_with_time(id(k), on, PRESS_TIME);
        match c.action {
            Action::Press(t) => {
                let p = anim(0, (lit.pressed)(t));
                let edge = edge_of(c.shape, r);
                dome(&mut out, &edge, r.center(), c.color, p, p, unit);
                glyph(&mut out, ctx, c, r.translate(vec2(0.0, unit * 3.0 * p)), unit);
                let at = match c.cap_at {
                    CapAt::Center => r.center(),
                    CapAt::Below => pos2(r.center().x, r.bottom() + unit * 6.0),
                    CapAt::Above => pos2(r.center().x, r.top() - unit * 14.0),
                    CapAt::Left => pos2(r.left() - unit * 24.0, r.center().y),
                    CapAt::Right => pos2(r.right() + unit * 26.0, r.center().y),
                };
                cap(CapFor::Press(t), at, p);
            }
            Action::Dpad(ts) => {
                // It tilts towards the arms held.
                const DIRS: [Vec2; 4] = [vec2(0.0, -1.0), vec2(0.0, 1.0), vec2(-1.0, 0.0), vec2(1.0, 0.0)];
                let held: Vec<f32> = (0..4).map(|k| anim(k, (lit.pressed)(ts[k]))).collect();
                let tilt = (0..4).fold(Vec2::ZERO, |a, k| a + DIRS[k] * held[k]) * unit * 3.0;
                let any = held.iter().cloned().fold(0.0, f32::max);
                let rc = r.translate(tilt);
                let edge = outline(cross_sdf(rc, 0.36, r.width() * 0.08), rc.center(), r.width(), N);
                dome(&mut out, &edge, rc.center(), c.color, any * 0.5, 0.0, unit);
                for k in 0..4 {
                    let dir = DIRS[k];
                    // The arm held lights up.
                    if held[k] > 0.01 {
                        let arm = Rect::from_center_size(
                            rc.center() + dir * r.width() * 0.3,
                            if dir.x == 0.0 {
                                vec2(r.width() * 0.66, r.height() * 0.36)
                            } else {
                                vec2(r.width() * 0.36, r.height() * 0.66)
                            },
                        );
                        out.push(egui::Shape::rect_filled(
                            arm.shrink(unit * 2.0),
                            r.width() * 0.06,
                            alpha(glow_of(c.color), 0.55 * held[k]),
                        ));
                    }
                    let tip = rc.center() + dir * r.width() * 0.44;
                    let back = rc.center() + dir * r.width() * 0.36;
                    let side = vec2(-dir.y, dir.x) * r.width() * 0.05;
                    out.push(egui::Shape::convex_polygon(
                        vec![tip, back + side, back - side],
                        shade(c.color, 1.5),
                        Stroke::NONE,
                    ));
                    cap(CapFor::Press(ts[k]), rc.center() + dir * r.width() * 0.24, held[k]);
                }
                out.push(egui::Shape::circle_filled(rc.center(), r.width() * 0.09, shade(c.color, 0.75)));
                out.push(egui::Shape::circle_filled(
                    rc.center() + LIGHT * r.width() * 0.03,
                    r.width() * 0.05,
                    shade(c.color, 1.1),
                ));
            }
            Action::Rocker(ts) => {
                // It rocks towards the side held.
                let (l, rr) = (anim(0, (lit.pressed)(ts[0])), anim(1, (lit.pressed)(ts[1])));
                let rc = r.translate(vec2((rr - l) * unit * 4.0, 0.0));
                let edge = edge_of(c.shape, rc);
                dome(&mut out, &edge, rc.center(), c.color, l.max(rr) * 0.4, l.max(rr) * 0.6, unit);
                // A groove down the middle, and each half lit when held.
                let half = |left: bool| {
                    if left {
                        Rect::from_min_max(rc.min, pos2(rc.center().x, rc.max.y))
                    } else {
                        Rect::from_min_max(pos2(rc.center().x, rc.min.y), rc.max)
                    }
                };
                for (k, t) in [(0, l), (1, rr)] {
                    if t > 0.01 {
                        out.push(egui::Shape::rect_filled(
                            half(k == 0).shrink(unit * 4.0),
                            rc.height() * 0.4,
                            alpha(Color32::WHITE, 0.35 * t),
                        ));
                    }
                }
                out.push(egui::Shape::line_segment(
                    [
                        pos2(rc.center().x, rc.top() + rc.height() * 0.2),
                        pos2(rc.center().x, rc.bottom() - rc.height() * 0.2),
                    ],
                    Stroke::new(unit * 2.0, shade(c.color, 0.6)),
                ));
                cap(CapFor::Press(ts[0]), pos2(rc.left() + rc.width() * 0.25, rc.center().y), l);
                cap(CapFor::Press(ts[1]), pos2(rc.left() + rc.width() * 0.75, rc.center().y), rr);
            }
            Action::Wheel { rotary, vertical } => {
                // Turns glide; the control glows while it turns.
                let target = (lit.turned)(rotary) as f32;
                let shown = ctx.animate_value_with_time(id(0), target, TURN_TIME);
                let moving = target - shown;
                let held = (lit.turning)(rotary);
                let way = |s: i8| {
                    let gliding = if (moving > 0.0) == (s > 0) { (moving.abs() * 4.0).min(1.0) } else { 0.0 };
                    anim(if s > 0 { 1 } else { 2 }, held == Some(s)).max(gliding)
                };
                let (up, down) = (way(1), way(-1));
                let glow = up.max(down);
                let edge = edge_of(c.shape, r);
                dome(&mut out, &edge, r.center(), c.color, 0.0, glow, unit);
                let ridge = Stroke::new((unit * 2.2).max(1.0), shade(c.color, 0.55));
                let shine = Stroke::new((unit * 1.2).max(1.0), shade(c.color, 1.45));
                if vertical {
                    let gap = r.height() / 7.0;
                    let shift = (-shown * gap * 0.5).rem_euclid(gap);
                    let mut y = r.top() + shift;
                    while y < r.bottom() - unit * 3.0 {
                        if y > r.top() + unit * 3.0 {
                            // Ridges curve away at the ends: thinner there.
                            let k = 1.0 - ((y - r.center().y) / (r.height() / 2.0)).powi(2);
                            let half = r.width() * (0.18 + 0.2 * k.max(0.0));
                            out.push(egui::Shape::line_segment(
                                [pos2(r.center().x - half, y), pos2(r.center().x + half, y)],
                                ridge,
                            ));
                            out.push(egui::Shape::line_segment(
                                [pos2(r.center().x - half, y - unit * 1.6), pos2(r.center().x + half, y - unit * 1.6)],
                                shine,
                            ));
                        }
                        y += gap;
                    }
                    cap(CapFor::Turn(rotary, 1), pos2(r.center().x, r.top() + r.height() * 0.2), up);
                    cap(CapFor::Turn(rotary, -1), pos2(r.center().x, r.bottom() - r.height() * 0.2), down);
                } else {
                    let ang = shown * ANGLE_STEP;
                    for k in 0..12 {
                        let a = ang + std::f32::consts::TAU * k as f32 / 12.0;
                        let d = vec2(a.cos(), a.sin());
                        out.push(egui::Shape::line_segment(
                            [r.center() + d * r.width() * 0.36, r.center() + d * r.width() * 0.47],
                            ridge,
                        ));
                    }
                    // A cap that turns with it, with a marker.
                    let hub = Rect::from_center_size(r.center(), r.size() * 0.5);
                    dome(
                        &mut out,
                        &edge_of(Shape::Circle, hub),
                        hub.center(),
                        shade(c.color, 1.15),
                        0.0,
                        glow * 0.5,
                        unit * 0.6,
                    );
                    let d = vec2(ang.cos(), ang.sin());
                    out.push(egui::Shape::circle_filled(
                        r.center() + d * r.width() * 0.17,
                        r.width() * 0.035,
                        Color32::from_white_alpha(220),
                    ));
                    cap(CapFor::Turn(rotary, -1), r.center() - vec2(r.width() * 0.31, 0.0), down);
                    cap(CapFor::Turn(rotary, 1), r.center() + vec2(r.width() * 0.31, 0.0), up);
                }
            }
            Action::Strip => {
                let touched = anim(0, lit.strip.is_some());
                let swipe = lit.swiping;
                let up = anim(1, swipe == Some(true));
                let down = anim(2, swipe == Some(false));
                let edge = edge_of(c.shape, r);
                dome(&mut out, &edge, r.center(), c.color, 0.0, touched * 0.6, unit);
                // The groove the finger runs in.
                let groove = Rect::from_center_size(r.center(), vec2(r.width() * 0.5, r.height() * 0.82));
                out.push(egui::Shape::rect_filled(groove, groove.width() / 2.0, shade(c.color, 0.62)));
                out.push(egui::Shape::rect_filled(
                    groove.translate(vec2(0.0, unit * 1.5)).shrink(unit),
                    groove.width() / 2.0,
                    shade(c.color, 0.8),
                ));
                glyph(&mut out, ctx, c, groove, unit);
                // The finger: a light that glides along it.
                let target = lit.strip.unwrap_or(0.5);
                let pos = ctx.animate_value_with_time(id(3), target, 0.05);
                if touched > 0.01 {
                    let p = pos2(r.center().x, groove.top() + pos * groove.height());
                    for k in (1..=4).rev() {
                        out.push(egui::Shape::circle_filled(
                            p,
                            r.width() * 0.08 * k as f32,
                            Color32::from_white_alpha((60.0 * touched / k as f32) as u8),
                        ));
                    }
                    out.push(egui::Shape::circle_filled(
                        p,
                        r.width() * 0.13,
                        Color32::from_white_alpha((230.0 * touched) as u8),
                    ));
                }
                cap(CapFor::Swipe(true), pos2(r.center().x, r.top() + r.height() * 0.17), up);
                cap(CapFor::Swipe(false), pos2(r.center().x, r.bottom() - r.height() * 0.17), down);
            }
        }
    }
    // Keys on top of everything.
    for (at, label, on) in caps {
        keycap(&mut out, ctx, at, &label, on, unit);
    }
    out
}

/// A recess: its upper-left wall in shadow, its lower-right lip catching
/// the light, and its glowing edge if it has one.
fn housing(out: &mut Vec<egui::Shape>, hs: &Housing, r: Rect, unit: f32) {
    const N: usize = 96;
    let radius = r.width().min(r.height()) * hs.round;
    let edge = rim(r, radius, r.center(), N);
    if let Some(g) = hs.glow {
        let halo = rim(r.expand(unit * 16.0), radius + unit * 16.0, r.center(), N);
        out.push(band(&halo, &edge, r.center(), |_| alpha(g, 0.0), |_| alpha(g, 0.45)));
    }
    let lip = Color32::from_gray(255);
    out.push(fan(&edge, r.center(), hs.fill, |d| {
        let l = lightness(d);
        if l > 0.0 { mix(hs.fill, Color32::BLACK, 0.55 * l) } else { mix(hs.fill, lip, 0.12 * -l) }
    }));
    // The inner wall's shadow, cast down and to the right.
    let wall = rim(
        r.shrink(unit * 5.0).translate(vec2(unit * 2.0, unit * 3.0)),
        (radius - unit * 5.0).max(0.0),
        r.center(),
        N,
    );
    out.push(band(&edge, &wall, r.center(), |_| Color32::from_black_alpha(110), |_| Color32::TRANSPARENT));
    match hs.glow {
        Some(g) => {
            out.push(egui::Shape::closed_line(edge.clone(), Stroke::new((unit * 3.0).max(1.5), g)));
            out.push(egui::Shape::closed_line(edge, Stroke::new((unit * 1.2).max(1.0), mix(g, Color32::WHITE, 0.6))));
        }
        None => out.push(egui::Shape::closed_line(edge, Stroke::new(1.0, mix(hs.fill, lip, 0.18)))),
    }
}

/// A control's outline, from its middle.
fn edge_of(shape: Shape, r: Rect) -> Vec<Pos2> {
    const N: usize = 72;
    match shape {
        Shape::Circle => (0..N)
            .map(|i| {
                let a = std::f32::consts::TAU * i as f32 / N as f32;
                r.center() + vec2(a.cos() * r.width() / 2.0, a.sin() * r.height() / 2.0)
            })
            .collect(),
        Shape::Rounded(k) => rim(r, r.width().min(r.height()) * k, r.center(), N),
        Shape::Cross => outline(cross_sdf(r, 0.36, r.width() * 0.08), r.center(), r.width(), N),
    }
}

/// The symbol on a control.
fn glyph(out: &mut Vec<egui::Shape>, ctx: &egui::Context, c: &Control, r: Rect, unit: f32) {
    let ink = Color32::from_white_alpha(230);
    let s = r.height().min(r.width());
    let (cx, cy) = (r.center().x, r.center().y);
    match c.glyph {
        "" => {}
        "⌂" => {
            // A house (Ribbit's font has no house symbol).
            let roof =
                vec![pos2(cx, cy - s * 0.3), pos2(cx + s * 0.3, cy - s * 0.02), pos2(cx - s * 0.3, cy - s * 0.02)];
            out.push(egui::Shape::convex_polygon(roof, ink, Stroke::NONE));
            let walls = Rect::from_min_max(pos2(cx - s * 0.2, cy - s * 0.04), pos2(cx + s * 0.2, cy + s * 0.26));
            out.push(egui::Shape::rect_filled(walls, 1.0, ink));
            let door = Rect::from_min_max(pos2(cx - s * 0.06, cy + s * 0.08), pos2(cx + s * 0.06, cy + s * 0.26));
            out.push(egui::Shape::rect_filled(door, 0.0, shade(c.color, 0.8)));
        }
        "◐" => {
            // Contrast: a circle half filled.
            let rad = s * 0.28;
            out.push(egui::Shape::circle_stroke(r.center(), rad, Stroke::new((unit * 1.2).max(1.0), ink)));
            let half: Vec<Pos2> = (0..=16)
                .map(|i| {
                    let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * i as f32 / 16.0;
                    r.center() + vec2(a.cos(), a.sin()) * rad
                })
                .collect();
            out.push(egui::Shape::convex_polygon(half, ink, Stroke::NONE));
        }
        "⇕" => {
            // Up and down arrows with a ladder between, as on the Slider.
            let w = r.width();
            let (top, bottom) = (r.top() + r.height() * 0.04, r.bottom() - r.height() * 0.04);
            let head = r.height() * 0.13;
            let arrow = Color32::from_rgb(255, 238, 200);
            out.push(egui::Shape::convex_polygon(
                vec![pos2(cx, top), pos2(cx + w * 0.62, top + head), pos2(cx - w * 0.62, top + head)],
                arrow,
                Stroke::NONE,
            ));
            out.push(egui::Shape::convex_polygon(
                vec![pos2(cx, bottom), pos2(cx - w * 0.62, bottom - head), pos2(cx + w * 0.62, bottom - head)],
                arrow,
                Stroke::NONE,
            ));
            for k in 0..6 {
                let y = top + head * 1.5 + (bottom - top - head * 3.0) * k as f32 / 5.0;
                let bar = Rect::from_center_size(pos2(cx, y), vec2(w * 0.6, r.height() * 0.035));
                out.push(egui::Shape::rect_filled(bar, 1.0, alpha(arrow, 0.8)));
            }
        }
        g => {
            let font = egui::FontId::proportional((s * 0.5).max(6.0));
            out.push(text(
                ctx,
                r.center() + vec2(0.0, unit),
                egui::Align2::CENTER_CENTER,
                g,
                font.clone(),
                Color32::from_black_alpha(90),
            ));
            out.push(text(ctx, r.center(), egui::Align2::CENTER_CENTER, g, font, ink));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layouts() -> [Layout; 2] {
        [rockit_twist(), leapster()]
    }

    #[test]
    fn screens_keep_their_shape_and_controls_stay_off_them() {
        for l in layouts() {
            let ratio = l.screen.width() * l.aspect / l.screen.height();
            assert!((ratio - l.screen_aspect).abs() < 1e-3, "{}: screen {ratio} != {}", l.name, l.screen_aspect);
            for c in &l.controls {
                assert!(!c.rect.intersects(l.screen), "{}: {} covers the screen", l.name, c.name);
                assert!(c.rect.min.x >= -0.01 && c.rect.max.x <= 1.01, "{}: {} is outside the case", l.name, c.name);
                assert!(c.rect.min.y >= -0.01 && c.rect.max.y <= 1.01, "{}: {} is outside the case", l.name, c.name);
            }
        }
    }

    #[test]
    fn every_twist_control_is_on_the_picture() {
        let l = rockit_twist();
        let mut buttons: Vec<TwistButton> = Vec::new();
        let mut wheels = Vec::new();
        for c in &l.controls {
            match c.action {
                Action::Press(Target::Twist(b)) => buttons.push(b),
                Action::Dpad(ts) => buttons.extend(ts.iter().map(|t| match t {
                    Target::Twist(b) => *b,
                    _ => panic!("a Leapster button on the Twist"),
                })),
                Action::Rocker(ts) => buttons.extend(ts.iter().map(|t| match t {
                    Target::Twist(b) => *b,
                    _ => panic!("a Leapster button on the Twist"),
                })),
                Action::Wheel { rotary, .. } => wheels.push(rotary),
                Action::Strip => {}
                Action::Press(_) => panic!("a Leapster button on the Twist"),
            }
        }
        for b in TwistButton::ALL {
            assert_eq!(buttons.iter().filter(|&&x| x == b).count(), 1, "{b:?}");
        }
        for r in Rotary::ALL {
            assert!(wheels.contains(&r), "{r:?}");
        }
        assert_eq!(l.controls.iter().filter(|c| c.action == Action::Strip).count(), 1);
    }

    #[test]
    fn every_bindable_leapster_button_is_on_the_picture() {
        let l = leapster();
        let has = |b| {
            l.controls.iter().any(|c| match c.action {
                Action::Press(t) => t == Target::Leapster(b),
                Action::Dpad(ts) => ts.contains(&Target::Leapster(b)),
                _ => false,
            })
        };
        for b in crate::settings::BINDABLE {
            assert!(has(b), "{b:?}");
        }
    }

    #[test]
    fn the_shell_fits_any_window_with_its_shape() {
        let l = rockit_twist();
        for area in [
            Rect::from_min_size(pos2(0.0, 30.0), vec2(1920.0, 1000.0)),
            Rect::from_min_size(Pos2::ZERO, vec2(300.0, 900.0)),
        ] {
            let s = shell_rect(&l, area);
            assert!(area.contains_rect(s));
            assert!((s.width() / s.height() - l.aspect).abs() < 1e-3);
            let screen = place(s, l.screen);
            assert!((screen.width() / screen.height() - 4.0 / 3.0).abs() < 1e-3);
        }
    }

    #[test]
    fn pointer_hits_and_dpad_sides() {
        let l = rockit_twist();
        let shell = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 1000.0 / l.aspect));
        let check = l.controls.iter().position(|c| c.name == "Check mark").unwrap();
        let at = place(shell, l.controls[check].rect).center();
        assert_eq!(hit(&l, shell, at), Some(check));
        assert_eq!(hit(&l, shell, place(shell, l.screen).center()), None, "the screen is not a control");
        let r = Rect::from_center_size(Pos2::ZERO, vec2(100.0, 100.0));
        assert_eq!(dpad_side(r, pos2(0.0, -40.0)), Some(0));
        assert_eq!(dpad_side(r, pos2(0.0, 40.0)), Some(1));
        assert_eq!(dpad_side(r, pos2(-40.0, 5.0)), Some(2));
        assert_eq!(dpad_side(r, pos2(40.0, -5.0)), Some(3));
        assert_eq!(dpad_side(r, pos2(2.0, 2.0)), None);
        // The cross has no corners.
        assert!(!inside(Shape::Cross, r, pos2(45.0, 45.0)));
    }

    #[test]
    fn letting_go_releases_everything_held() {
        let mut v = ConsoleView {
            held: vec![Target::Twist(TwistButton::Check), Target::Twist(TwistButton::DpadUp)],
            strip_down: true,
            ..Default::default()
        };
        let ev = v.release_all();
        assert_eq!(
            ev,
            [
                Event::Press(Target::Twist(TwistButton::Check), false),
                Event::Press(Target::Twist(TwistButton::DpadUp), false),
                Event::Strip(None)
            ]
        );
        assert!(v.release_all().is_empty());
    }

    #[test]
    fn rings_follow_their_edges() {
        let r = Rect::from_min_size(Pos2::ZERO, vec2(200.0, 100.0));
        for p in rim(r, 10.0, pos2(100.0, 50.0), 64) {
            assert!(r.expand(0.01).contains(p));
            let on_edge =
                (p.x.abs() < 0.5 || (p.x - 200.0).abs() < 0.5 || p.y.abs() < 0.5 || (p.y - 100.0).abs() < 0.5)
                    || (p - pos2(10.0, 10.0)).length() > 9.0;
            assert!(on_edge, "{p:?}");
        }
    }
}
