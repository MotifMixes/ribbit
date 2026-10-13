//! Physical controls.
//!
//! Buttons are read through the GPIO input register "GIODataIn" at
//! 0x0180_004C, active-low. Bit assignments come from MAME `leapster.cpp`
//! (INPUT_PORTS_START) and are UNVERIFIED on an original Leapster. The same
//! register carries power/dock status bits; MAME forces those to 1 to avoid
//! a shutdown, which is a HYPOTHESIS about their idle level.
//!
//! The touchscreen is sampled by an ADC (channel 0) through a FIFO; that
//! protocol is not modelled yet. `Touch` holds frontend-provided state.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Button {
    Up,
    Down,
    Left,
    Right,
    A,
    B,
    Hint,
    Home,
    Pause,
    VolumeUp,
    VolumeDown,
    BrightnessUp,
    BrightnessDown,
    Contrast,
}

impl Button {
    pub const ALL: [Button; 14] = [
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
        Button::BrightnessUp,
        Button::BrightnessDown,
        Button::Contrast,
    ];

    /// Parses a button name (case-insensitive): up, down, left, right, a, b,
    /// hint, home, pause, volume_up, volume_down, brightness_up,
    /// brightness_down, contrast.
    pub fn from_name(name: &str) -> Option<Button> {
        let n = name.to_ascii_lowercase().replace('-', "_");
        Button::ALL.into_iter().find(|b| format!("{b:?}").to_ascii_lowercase() == n.replace('_', ""))
    }

    /// Bit in GIODataIn (pulled low while pressed).
    pub const fn mask(self) -> u32 {
        match self {
            Button::Right => 0x0000_0080,
            Button::Down => 0x0000_0100,
            Button::Left => 0x0000_0200,
            Button::B => 0x0000_2000,
            Button::A => 0x0000_4000,
            Button::VolumeDown => 0x0000_8000,
            Button::VolumeUp => 0x0001_0000,
            Button::BrightnessDown => 0x0002_0000,
            Button::BrightnessUp => 0x0100_0000,
            Button::Contrast => 0x0200_0000,
            Button::Pause => 0x0400_0000,
            Button::Hint => 0x1000_0000,
            Button::Home => 0x2000_0000,
            Button::Up => 0x8000_0000,
        }
    }
}

/// Union of all button bits.
pub const BUTTON_BITS: u32 = 0xB703_E380;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Buttons {
    pressed: u32,
}

impl Buttons {
    pub fn set(&mut self, button: Button, down: bool) {
        if down {
            self.pressed |= button.mask();
        } else {
            self.pressed &= !button.mask();
        }
    }

    pub fn is_down(&self, button: Button) -> bool {
        self.pressed & button.mask() != 0
    }

    /// Value the CPU reads from GIODataIn.
    pub fn gio_data_in(&self) -> u32 {
        !self.pressed
    }
}

/// Raw stylus state in ADC units. MAME uses a 0..=0x7F0 range per axis;
/// the real mapping from panel position to ADC code needs calibration data
/// from hardware and is an OPEN QUESTION.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Touch {
    pub x: u16,
    pub y: u16,
    pub pressed: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_are_distinct_and_match_union() {
        let mut union = 0;
        for b in Button::ALL {
            assert_eq!(union & b.mask(), 0, "{b:?} overlaps");
            assert_eq!(b.mask().count_ones(), 1);
            union |= b.mask();
        }
        assert_eq!(union, BUTTON_BITS);
    }

    #[test]
    fn gio_is_active_low() {
        let mut buttons = Buttons::default();
        assert_eq!(buttons.gio_data_in(), 0xFFFF_FFFF);
        buttons.set(Button::A, true);
        buttons.set(Button::Up, true);
        assert_eq!(buttons.gio_data_in(), !(0x0000_4000 | 0x8000_0000));
        buttons.set(Button::A, false);
        assert!(!buttons.is_down(Button::A));
        assert_eq!(buttons.gio_data_in(), 0x7FFF_FFFF);
    }
}
