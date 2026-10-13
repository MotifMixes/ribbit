//! Game controllers on desktop (gilrs: XInput / DirectInput / evdev / IOKit).
//!
//! Fixed layout, positional (Xbox names): A (south) = A, B (east) = B,
//! X / Y = Hint, Start = Pause, Back/Select = Home, LB / RB = volume,
//! D-pad or left stick = arrows. Guide opens the menu.

use gilrs::{Axis, Button as Pad, EventType, Gilrs};
use ribbit_input::Button;

/// Stick deflection that counts as a direction.
const STICK: f32 = 0.5;

pub struct Gamepads {
    gilrs: Gilrs,
    /// Pad-held state of each Leapster button, so releasing a pad button
    /// never releases a key still held on the keyboard and vice versa.
    held: [bool; Button::ALL.len()],
}

/// One change for the app to apply.
pub enum PadEvent {
    Button(Button, bool),
    Menu,
    Connected(String),
    Disconnected(String),
}

impl Gamepads {
    pub fn open() -> Option<Self> {
        match Gilrs::new() {
            Ok(gilrs) => Some(Self { gilrs, held: Default::default() }),
            Err(e) => {
                tracing::warn!(target: "ribbit::input", "no gamepad support: {e}");
                None
            }
        }
    }

    /// Drains pending controller events.
    pub fn poll(&mut self, mut emit: impl FnMut(PadEvent)) {
        while let Some(ev) = self.gilrs.next_event() {
            let name = || self.gilrs.gamepad(ev.id).name().to_string();
            match ev.event {
                EventType::Connected => emit(PadEvent::Connected(name())),
                EventType::Disconnected => emit(PadEvent::Disconnected(name())),
                EventType::ButtonPressed(Pad::Mode, _) => emit(PadEvent::Menu),
                EventType::ButtonPressed(b, _) | EventType::ButtonReleased(b, _) => {
                    if let Some(button) = map_button(b) {
                        let pressed = matches!(ev.event, EventType::ButtonPressed(..));
                        self.set(button, pressed, &mut emit);
                    }
                }
                EventType::AxisChanged(axis, v, _) => {
                    let (neg, pos) = match axis {
                        Axis::LeftStickX => (Button::Left, Button::Right),
                        // gilrs: up is positive.
                        Axis::LeftStickY => (Button::Down, Button::Up),
                        _ => continue,
                    };
                    self.set(neg, v < -STICK, &mut emit);
                    self.set(pos, v > STICK, &mut emit);
                }
                _ => {}
            }
        }
    }

    fn set(&mut self, b: Button, pressed: bool, emit: &mut impl FnMut(PadEvent)) {
        let i = Button::ALL.iter().position(|x| *x == b).unwrap();
        if self.held[i] != pressed {
            self.held[i] = pressed;
            emit(PadEvent::Button(b, pressed));
        }
    }
}

fn map_button(b: Pad) -> Option<Button> {
    Some(match b {
        Pad::South => Button::A,
        Pad::East => Button::B,
        Pad::West | Pad::North => Button::Hint,
        Pad::Start => Button::Pause,
        Pad::Select => Button::Home,
        Pad::LeftTrigger => Button::VolumeDown,
        Pad::RightTrigger => Button::VolumeUp,
        Pad::DPadUp => Button::Up,
        Pad::DPadDown => Button::Down,
        Pad::DPadLeft => Button::Left,
        Pad::DPadRight => Button::Right,
        _ => return None,
    })
}
