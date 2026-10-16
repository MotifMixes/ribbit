//! Buttons held by each input source (keyboard, the console view's
//! controls, gamepads). A button is down while any source holds it, so
//! letting go of a virtual button never releases a key still held, or the
//! other way round.

use ribbit_input::Button;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Keyboard,
    Pointer,
    Gamepad,
}

#[derive(Debug, Default, Clone)]
pub struct Held {
    masks: [u32; 3],
}

fn bit(b: Button) -> u32 {
    1 << Button::ALL.iter().position(|&x| x == b).unwrap()
}

impl Held {
    fn any(&self) -> u32 {
        self.masks.iter().fold(0, |a, m| a | m)
    }

    /// `source` holds or lets go of `b`; the button's new state if it
    /// changed (to send to the machine).
    pub fn set(&mut self, source: Source, b: Button, down: bool) -> Option<bool> {
        let before = self.any() & bit(b) != 0;
        let m = &mut self.masks[source as usize];
        if down {
            *m |= bit(b);
        } else {
            *m &= !bit(b);
        }
        let after = self.any() & bit(b) != 0;
        (before != after).then_some(after)
    }

    pub fn pressed(&self, b: Button) -> bool {
        self.any() & bit(b) != 0
    }

    /// Lets go of all of one source's buttons; the buttons that came up.
    pub fn release(&mut self, source: Source) -> Vec<Button> {
        let before = self.any();
        self.masks[source as usize] = 0;
        let up = before & !self.any();
        Button::ALL.into_iter().filter(|&b| up & bit(b) != 0).collect()
    }

    pub fn clear(&mut self) {
        self.masks = [0; 3];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_button_stays_down_while_any_source_holds_it() {
        let mut h = Held::default();
        assert_eq!(h.set(Source::Keyboard, Button::A, true), Some(true));
        assert_eq!(h.set(Source::Pointer, Button::A, true), None, "already down");
        assert_eq!(h.set(Source::Pointer, Button::A, false), None, "the key still holds it");
        assert!(h.pressed(Button::A));
        assert_eq!(h.set(Source::Keyboard, Button::A, false), Some(false));
        // Two buttons at once from two sources, then the pointer leaves.
        h.set(Source::Pointer, Button::Left, true);
        h.set(Source::Keyboard, Button::B, true);
        h.set(Source::Pointer, Button::B, true);
        assert_eq!(h.release(Source::Pointer), [Button::Left]);
        assert!(h.pressed(Button::B));
        h.clear();
        assert!(!h.pressed(Button::B));
    }
}
