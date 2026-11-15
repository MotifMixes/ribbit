//! The chip's general-purpose ports, and the keys wired to them.
//!
//! EVIDENCE (key scans: iQuest kernel 0x5701-0x5775, TTExtreme kernel
//! 0x3A36-0x3AA5 and 0x3B92-0x3C01):
//!
//! | Port | Data | Direction (1 = output) |
//! |---|---|---|
//! | A | FC1F | FC33 |
//! | B | FC49 | FC4B |
//! | C | FC48 | FC4A (HYPOTHESIS) |
//!
//! A pin set as an output shows its data bit; an input reads high (pulled
//! up) unless a pressed key joins it to ground or to an output driven low.
//! The scans drive one line low and read which others follow it.
//!
//! * iQuest: a 6 x 8 matrix. Column c drives port B bit [0,1,2,3,4,7][c];
//!   row r reads port A bit r. The key is c * 8 + r, named by the table at
//!   ROM 0x24917 ("12345678QWERTYUI…").
//! * Turbo Twist Extreme: port A bits 3 and 4 (and bits 0 and 1 for the
//!   twist end) drive, port B reads; port A bits 6-7 and port B bit 6 are
//!   read directly.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Port {
    A,
    B,
    C,
}

impl Port {
    pub fn data_reg(self) -> u8 {
        match self {
            Port::A => 0x1F,
            Port::B => 0x49,
            Port::C => 0x48,
        }
    }

    pub fn dir_reg(self) -> u8 {
        match self {
            Port::A => 0x33,
            Port::B => 0x4B,
            Port::C => 0x4A,
        }
    }

    pub fn of_data_reg(reg: u8) -> Option<Port> {
        [Port::A, Port::B, Port::C].into_iter().find(|p| p.data_reg() == reg)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Pin {
    pub port: Port,
    pub bit: u8,
}

/// What a key joins when pressed: two pins, or a pin and ground (`a` None).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Wire {
    pub a: Option<Pin>,
    pub b: Pin,
}

/// Whether a pin is an output driven low.
fn driven_low(io: &[u8], p: Pin) -> bool {
    io[p.port.dir_reg() as usize] >> p.bit & 1 != 0 && io[p.port.data_reg() as usize] >> p.bit & 1 == 0
}

/// The level of every pin of `port`, as a read of its data register sees it.
pub fn read(io: &[u8], port: Port, pressed: &[Wire]) -> u8 {
    let dir = io[port.dir_reg() as usize];
    let mut v = (io[port.data_reg() as usize] & dir) | !dir;
    for w in pressed {
        for (me, other) in [(Some(w.b), w.a), (w.a, Some(w.b))] {
            let Some(me) = me else { continue };
            if me.port != port || dir >> me.bit & 1 != 0 {
                continue;
            }
            let low = match other {
                None => true,
                Some(o) => driven_low(io, o),
            };
            if low {
                v &= !(1 << me.bit);
            }
        }
    }
    v
}
