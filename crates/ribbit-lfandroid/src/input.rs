//! Touch and buttons, handed to the title as the Java shell's LFMainActivity does: through its
//! natives, between frames (at the outer call level, like `naDrawFrame`).
//!
//! `naTouchDown/Move/Up(FF)V` take the window position in pixels (MotionEvent's x and y, in
//! the window's orientation: 1024x600, or 600x1024 for a portrait title). The floats come in
//! r2/r3 (softfp). The library turns them into Brio screen pixels itself and posts a Brio
//! touch message (Down is type 1) to its event queue. The buttons are Brio's: each
//! `naPress*`/`naRelease*` sets or clears its bit in the button state and posts a button
//! message. `naPressJoystick(I)V`/`naReleaseJoystick(I)V` take the direction bits themselves.

use crate::runtime::{Runtime, Stop};

/// Brio button bits (`tButtonData`), as the natives set them. A, B, BACK, HINT and PAUSE are
/// the constants in `naPressA/B/Back/Hint/Pause`. The direction bits are Brio's usual order
/// (GUESS: from the Didj SDK's ButtonTypes; `RotateDpad` turns 1->8->2->4->1, which fits).
pub mod button {
    pub const UP: u32 = 0x1;
    pub const DOWN: u32 = 0x2;
    pub const RIGHT: u32 = 0x4;
    pub const LEFT: u32 = 0x8;
    pub const A: u32 = 0x10;
    pub const B: u32 = 0x20;
    /// `naPressBack`: Brio's menu button.
    pub const BACK: u32 = 0x100;
    pub const HINT: u32 = 0x200;
    pub const PAUSE: u32 = 0x400;
    /// `naPressHome`: not a Brio bit (that native sets none); Ribbit's own.
    pub const HOME: u32 = 0x1_0000;
    pub const DPAD: u32 = UP | DOWN | RIGHT | LEFT;

    /// A button by name: a b back hint pause home up down left right.
    pub fn named(name: &str) -> Option<u32> {
        Some(match name {
            "a" => A,
            "b" => B,
            "back" => BACK,
            "hint" => HINT,
            "pause" => PAUSE,
            "home" => HOME,
            "up" => UP,
            "down" => DOWN,
            "left" => LEFT,
            "right" => RIGHT,
            _ => return None,
        })
    }
}

/// What the title has been told so far.
#[derive(Debug, Default, Clone)]
pub struct Input {
    /// Where the finger is down (window pixels), if it is.
    pub touch: Option<(f32, f32)>,
    /// The buttons held (`button::*`).
    pub buttons: u32,
}

/// The press and release natives of the buttons with their own, by bit.
const NATIVES: [(u32, &str, &str); 6] = [
    (button::A, "naPressA", "naReleaseA"),
    (button::B, "naPressB", "naReleaseB"),
    (button::BACK, "naPressBack", "naReleaseBack"),
    (button::HINT, "naPressHint", "naReleaseHint"),
    (button::PAUSE, "naPressPause", "naReleasePause"),
    (button::HOME, "naPressHome", "naReleaseHome"),
];

impl Runtime {
    /// Puts the finger at a window position (pixels in `screen`'s orientation), or lifts it
    /// (None). Down, move or up is sent as it changes; the same place again sends nothing.
    pub fn touch(&mut self, at: Option<(f32, f32)>) -> Result<(), Stop> {
        let at = at.map(|(x, y)| (x.clamp(0.0, self.screen.0 as f32 - 1.0), y.clamp(0.0, self.screen.1 as f32 - 1.0)));
        let (name, (x, y)) = match (self.input.touch, at) {
            (None, Some(p)) => ("naTouchDown", p),
            (Some(old), Some(p)) if old != p => ("naTouchMove", p),
            (Some(old), None) => ("naTouchUp", old),
            _ => return Ok(()),
        };
        self.input.touch = at;
        if self.natives.contains_key(name) {
            self.call_native(name, &[x.to_bits(), y.to_bits()])?;
        }
        Ok(())
    }

    /// Holds the buttons in `held` (`button::*`) and lets go of the others, sending a press or
    /// release for each that changed (releases first).
    pub fn set_buttons(&mut self, held: u32) -> Result<(), Stop> {
        let old = self.input.buttons;
        self.input.buttons = held;
        for (press, mask) in [(false, old & !held), (true, held & !old)] {
            for &(bit, p, r) in &NATIVES {
                if mask & bit != 0 && self.natives.contains_key(if press { p } else { r }) {
                    self.call_native(if press { p } else { r }, &[])?;
                }
            }
            let dpad = mask & button::DPAD;
            let name = if press { "naPressJoystick" } else { "naReleaseJoystick" };
            if dpad != 0 && self.natives.contains_key(name) {
                self.call_native(name, &[dpad])?;
            }
        }
        Ok(())
    }
}
