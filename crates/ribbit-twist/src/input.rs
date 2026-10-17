//! The RockIt Twist's controls and their physical state.
//!
//! VERIFIED (the controls themselves): the factory test firmware (MfgTest,
//! package PCKT-0x004F000A) lists DPAD UP/DOWN/LEFT/RIGHT, HELP, HOME,
//! ENTER, VOL+, VOL-, ON/OFF, MATRIX 1-9, SWITCH LEFT/RIGHT, Spinner,
//! Yellow/Green/Red Dial and Slider (with "Turn R", "Turn L", "Mov Up"); the
//! Parent's Guide (91-003672-014) describes each one: the Switch rocks back
//! and forth, the Spinner turns both ways, the three Dials turn upward or
//! downward independently, and the Slider is swiped with a finger.
//!
//! UNKNOWN (so not modelled): how any of these reach the processor (GPIO
//! bits, a key matrix, encoder phases, the slider's sensor). This module
//! keeps the *physical* state (what is held, how far each wheel has turned,
//! where a finger is on the slider); the hardware model, when it exists,
//! derives its registers from it.
//!
//! Inputs can come from several places at once (keyboard, mouse on the
//! virtual console, gamepad): each keeps its own held buttons and a button
//! is down while any source holds it, so letting go on one source never
//! releases another's press.

/// The push-buttons (anything held down or let go).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    /// The "?" button (MfgTest: HELP; Parent's Guide: Hint Button).
    Hint,
    Home,
    /// The green check mark (MfgTest: ENTER).
    Check,
    VolumeUp,
    VolumeDown,
    Power,
    /// The Switch rocked to one side (MfgTest: SWITCH LEFT / SWITCH RIGHT).
    /// Whether it springs back or stays is UNVERIFIED: held while pressed.
    SwitchLeft,
    SwitchRight,
    /// The 3 x 3 button grid, 0 = top left, row by row. That this is the
    /// order of MfgTest's MATRIX 1-9 is UNVERIFIED.
    Grid(u8),
}

impl Button {
    pub const ALL: [Button; 21] = [
        Button::DpadUp,
        Button::DpadDown,
        Button::DpadLeft,
        Button::DpadRight,
        Button::Hint,
        Button::Home,
        Button::Check,
        Button::VolumeUp,
        Button::VolumeDown,
        Button::Power,
        Button::SwitchLeft,
        Button::SwitchRight,
        Button::Grid(0),
        Button::Grid(1),
        Button::Grid(2),
        Button::Grid(3),
        Button::Grid(4),
        Button::Grid(5),
        Button::Grid(6),
        Button::Grid(7),
        Button::Grid(8),
    ];

    fn bit(self) -> u32 {
        let i = match self {
            Button::Grid(n) => 12 + n.min(8) as u32,
            b => Button::ALL.iter().position(|&x| x == b).unwrap() as u32,
        };
        1 << i
    }

    /// The name used in the UI and in MfgTest's I/O test (in brackets).
    pub fn label(self) -> String {
        match self {
            Button::DpadUp => "D-pad up".into(),
            Button::DpadDown => "D-pad down".into(),
            Button::DpadLeft => "D-pad left".into(),
            Button::DpadRight => "D-pad right".into(),
            Button::Hint => "Hint (HELP)".into(),
            Button::Home => "Home".into(),
            Button::Check => "Check mark (ENTER)".into(),
            Button::VolumeUp => "Volume up".into(),
            Button::VolumeDown => "Volume down".into(),
            Button::Power => "Power (ON/OFF)".into(),
            Button::SwitchLeft => "Switch left".into(),
            Button::SwitchRight => "Switch right".into(),
            Button::Grid(n) => format!("Grid button {}", n + 1),
        }
    }
}

/// The controls that turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Rotary {
    Spinner,
    YellowDial,
    GreenDial,
    RedDial,
}

impl Rotary {
    pub const ALL: [Rotary; 4] = [Rotary::Spinner, Rotary::YellowDial, Rotary::GreenDial, Rotary::RedDial];

    fn index(self) -> usize {
        self as usize
    }

    pub fn label(self) -> &'static str {
        match self {
            Rotary::Spinner => "Spinner",
            Rotary::YellowDial => "Yellow dial",
            Rotary::GreenDial => "Green dial",
            Rotary::RedDial => "Red dial",
        }
    }
}

/// Which way a control turns: for the Spinner +1 is clockwise (MfgTest:
/// "Turn R"), for the Dials +1 is upward.
pub type Step = i8;

/// Where a source of input comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Keyboard,
    Pointer,
    Gamepad,
}

impl Source {
    const COUNT: usize = 3;

    fn index(self) -> usize {
        self as usize
    }
}

/// How long a keyboard swipe of the Slider takes, and how far it goes
/// (host-side choices, not hardware facts).
pub const SWIPE_SECONDS: f32 = 0.15;
pub const SWIPE_FROM: f32 = 0.75;
pub const SWIPE_TO: f32 = 0.25;
/// A wheel turned by a held key: the first step at once, then one every
/// `REPEAT_EVERY` with no pause first, so it keeps turning straight away
/// (unlike typing's key repeat).
pub const REPEAT_EVERY: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
struct Swipe {
    from: f32,
    to: f32,
    elapsed: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct HeldTurn {
    step: Step,
    /// Seconds until the next repeat.
    next: f32,
}

/// The state of every control.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Controls {
    held: [u32; Source::COUNT],
    /// Detents turned so far by each rotary control (wrapping).
    turned: [i32; 4],
    /// Turns held by a key (repeating), per control and source.
    turning: [[Option<HeldTurn>; Source::COUNT]; 4],
    /// A finger on the Slider (0.0 = top, 1.0 = bottom) from the pointer.
    finger: Option<f32>,
    /// A swipe made from the keyboard or a mouse wheel, in progress.
    swipe: Option<Swipe>,
}

impl Controls {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set(&mut self, source: Source, b: Button, down: bool) {
        let held = &mut self.held[source.index()];
        if down {
            *held |= b.bit();
        } else {
            *held &= !b.bit();
        }
    }

    /// Whether any source holds `b`.
    pub fn pressed(&self, b: Button) -> bool {
        self.held.iter().any(|h| h & b.bit() != 0)
    }

    pub fn pressed_by(&self, source: Source, b: Button) -> bool {
        self.held[source.index()] & b.bit() != 0
    }

    /// Turns a control by `steps` detents.
    pub fn turn(&mut self, r: Rotary, steps: i32) {
        let t = &mut self.turned[r.index()];
        *t = t.wrapping_add(steps);
    }

    /// Detents turned so far (wrapping): the change between two readings
    /// is how far it turned in between.
    pub fn turned(&self, r: Rotary) -> i32 {
        self.turned[r.index()]
    }

    /// A key that turns a control is held (`Some(step)`) or let go: one
    /// step now, then repeats while held (see `tick`).
    pub fn hold_turn(&mut self, source: Source, r: Rotary, step: Option<Step>) {
        let slot = &mut self.turning[r.index()][source.index()];
        match step {
            Some(s) if slot.is_none_or(|h| h.step != s) => {
                *slot = Some(HeldTurn { step: s, next: REPEAT_EVERY });
                self.turn(r, s as i32);
            }
            Some(_) => {}
            None => *slot = None,
        }
    }

    /// A finger on the Slider at `pos` (0 = top, 1 = bottom), or lifted.
    pub fn touch_slider(&mut self, pos: Option<f32>) {
        self.finger = pos.map(|p| p.clamp(0.0, 1.0));
        if pos.is_some() {
            self.swipe = None;
        }
    }

    /// A quick swipe along the Slider, up or down (keyboard, mouse wheel).
    pub fn swipe(&mut self, up: bool) {
        let (from, to) = if up { (SWIPE_FROM, SWIPE_TO) } else { (SWIPE_TO, SWIPE_FROM) };
        self.swipe = Some(Swipe { from, to, elapsed: 0.0 });
    }

    /// The way a key is turning a control right now, if one is.
    pub fn turning(&self, r: Rotary) -> Option<Step> {
        self.turning[r.index()].iter().flatten().map(|h| h.step).next()
    }

    /// A keyboard or wheel swipe in progress: true if upward.
    pub fn swiping(&self) -> Option<bool> {
        self.swipe.map(|s| s.to < s.from)
    }

    /// Where a finger is on the Slider (0 = top, 1 = bottom), if one is.
    pub fn slider(&self) -> Option<f32> {
        self.finger.or_else(|| self.swipe.map(|s| s.from + (s.to - s.from) * (s.elapsed / SWIPE_SECONDS).min(1.0)))
    }

    /// Advances time by `dt` seconds: swipes move on, held turns repeat.
    pub fn tick(&mut self, dt: f32) {
        if let Some(s) = &mut self.swipe {
            s.elapsed += dt;
            if s.elapsed > SWIPE_SECONDS {
                self.swipe = None;
            }
        }
        for r in Rotary::ALL {
            for src in 0..Source::COUNT {
                let mut steps = 0;
                if let Some(h) = &mut self.turning[r.index()][src] {
                    h.next -= dt;
                    while h.next <= 0.0 {
                        steps += h.step as i32;
                        h.next += REPEAT_EVERY;
                    }
                }
                self.turn(r, steps);
            }
        }
    }

    /// Lets go of everything one source holds (its window lost focus, the
    /// virtual console was hidden...).
    pub fn release(&mut self, source: Source) {
        self.held[source.index()] = 0;
        for r in &mut self.turning {
            r[source.index()] = None;
        }
        if source == Source::Pointer {
            self.finger = None;
        }
    }

    pub fn release_all(&mut self) {
        for s in [Source::Keyboard, Source::Pointer, Source::Gamepad] {
            self.release(s);
        }
        self.swipe = None;
    }

    /// Every button held, for display.
    pub fn held_buttons(&self) -> Vec<Button> {
        Button::ALL.into_iter().filter(|&b| self.pressed(b)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_button_has_its_own_bit() {
        let bits: Vec<u32> = Button::ALL.iter().map(|b| b.bit()).collect();
        for (i, a) in bits.iter().enumerate() {
            assert_eq!(a.count_ones(), 1);
            assert!(bits[i + 1..].iter().all(|b| b != a), "{:?} shares a bit", Button::ALL[i]);
        }
    }

    #[test]
    fn sources_hold_buttons_independently() {
        let mut c = Controls::new();
        c.set(Source::Keyboard, Button::Check, true);
        c.set(Source::Pointer, Button::Check, true);
        c.set(Source::Pointer, Button::Check, false);
        assert!(c.pressed(Button::Check), "the key still holds it");
        c.set(Source::Keyboard, Button::Check, false);
        assert!(!c.pressed(Button::Check));
        // Simultaneous presses.
        c.set(Source::Pointer, Button::Grid(4), true);
        c.set(Source::Keyboard, Button::DpadLeft, true);
        assert_eq!(c.held_buttons(), [Button::DpadLeft, Button::Grid(4)]);
        c.release(Source::Pointer);
        assert_eq!(c.held_buttons(), [Button::DpadLeft]);
        c.release_all();
        assert!(c.held_buttons().is_empty());
    }

    #[test]
    fn held_turns_step_then_repeat() {
        let mut c = Controls::new();
        c.hold_turn(Source::Keyboard, Rotary::RedDial, Some(1));
        assert_eq!(c.turned(Rotary::RedDial), 1, "a step at once");
        assert_eq!((c.turning(Rotary::RedDial), c.turning(Rotary::Spinner)), (Some(1), None));
        c.hold_turn(Source::Keyboard, Rotary::RedDial, Some(1));
        assert_eq!(c.turned(Rotary::RedDial), 1, "key repeat adds nothing");
        c.tick(REPEAT_EVERY - 0.01);
        assert_eq!(c.turned(Rotary::RedDial), 1);
        c.tick(0.02 + REPEAT_EVERY);
        assert_eq!(c.turned(Rotary::RedDial), 3, "no pause before it keeps turning");
        c.hold_turn(Source::Keyboard, Rotary::RedDial, None);
        c.tick(1.0);
        assert_eq!(c.turned(Rotary::RedDial), 3, "stops when let go");
        c.turn(Rotary::Spinner, -2);
        assert_eq!((c.turned(Rotary::Spinner), c.turned(Rotary::GreenDial)), (-2, 0));
        // Lost focus while held: no runaway turning.
        c.hold_turn(Source::Keyboard, Rotary::Spinner, Some(-1));
        c.release(Source::Keyboard);
        c.tick(5.0);
        assert_eq!(c.turned(Rotary::Spinner), -3);
    }

    #[test]
    fn slider_swipes_and_touches() {
        let mut c = Controls::new();
        assert_eq!(c.slider(), None);
        c.swipe(true);
        assert_eq!(c.slider(), Some(SWIPE_FROM));
        assert_eq!(c.swiping(), Some(true));
        c.tick(SWIPE_SECONDS / 2.0);
        let mid = c.slider().unwrap();
        assert!(mid < SWIPE_FROM && mid > SWIPE_TO, "moving up: {mid}");
        c.tick(SWIPE_SECONDS);
        assert_eq!(c.slider(), None, "lifted at the end");
        c.touch_slider(Some(1.5));
        assert_eq!(c.slider(), Some(1.0));
        c.release(Source::Pointer);
        assert_eq!(c.slider(), None);
    }
}
