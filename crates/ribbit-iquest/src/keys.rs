//! The keys of each model and the pins they join (gpio.rs).

use crate::Model;
use crate::gpio::{Pin, Port, Wire};

/// The iQuest's matrix by the kernel's own names (ROM 0x24917, key = column
/// * 8 + row): letters and digits, '.', ' ', and 'a', 'd', 'u', 'r', 'l',
/// '-', '+', 'm', whose meanings are in docs/iquest.md.
pub const IQUEST_MATRIX: &[u8; 48] = b"12345678QWERTYUIASDFGHJKZXCVBNM.  aLPO09durl-+m ";

/// Port B bits that drive the iQuest's columns (kernel table at ROM 0x17775).
const IQUEST_COLUMNS: [u8; 6] = [0, 1, 2, 3, 4, 7];

/// The wire of the iQuest key at `index` (column * 8 + row).
pub fn iquest_key(index: usize) -> Wire {
    Wire { a: Some(Pin { port: Port::B, bit: IQUEST_COLUMNS[index / 8] }), b: Pin { port: Port::A, bit: (index % 8) as u8 } }
}

/// A key by name: for the iQuest one character of IQUEST_MATRIX (by its
/// first place) or a name from `iquest_named`; for the Turbo Twist Extreme
/// a name from `TT_KEYS`.
pub fn wire(model: Model, name: &str) -> Option<Wire> {
    if let Some(w) = name.strip_prefix("pin:") {
        return parse_wire(w);
    }
    match model {
        Model::IQuest => {
            let c = match name {
                // The round Enter button and Del pull port C bits 0 and 1 to
                // ground (the kernel reads FC48 bits 0-1 directly; the Try
                // Me demo printed "{Enter}" and "{Del}").
                "enter" => return Some(ground(pin(Port::C, 0))),
                "del" => return Some(ground(pin(Port::C, 1))),
                "repeat" => b'a',
                "up" => b'u',
                "down" => b'd',
                "left" => b'l',
                "right" => b'r',
                "menu" => b'm',
                "vol-" | "minus" => b'-',
                "vol+" | "plus" => b'+',
                "space" => b' ',
                _ if name.len() == 1 => name.as_bytes()[0].to_ascii_uppercase(),
                _ => return None,
            };
            IQUEST_MATRIX.iter().position(|&k| k == c).map(iquest_key)
        }
        Model::TurboTwistExtreme => TT_KEYS.iter().find(|(n, _)| *n == name).map(|(_, w)| *w),
    }
}

const fn pin(port: Port, bit: u8) -> Pin {
    Pin { port, bit }
}

const fn join(a: Pin, b: Pin) -> Wire {
    Wire { a: Some(a), b }
}

const fn ground(b: Pin) -> Wire {
    Wire { a: None, b }
}

/// The Turbo Twist Extreme's keys, VERIFIED-FW: found by joining every pair
/// of the pins its scans use at the menu and reading what the menu app
/// printed ("{DPadLf}", "TwistUpFast}", "{VUp:115}" ...; docs/turbotwist.md).
/// The D-pad end is a 2 x 2 matrix (port A bits 0-1 drive, port B bits 0-1
/// read), the twist end another (port A bits 3-4, port B bits 3-4); the other
/// buttons pull a pin to ground.
pub const TT_KEYS: &[(&str, Wire)] = &[
    ("left", join(pin(Port::A, 0), pin(Port::B, 0))),
    ("up", join(pin(Port::A, 0), pin(Port::B, 1))),
    ("right", join(pin(Port::A, 1), pin(Port::B, 0))),
    ("down", join(pin(Port::A, 1), pin(Port::B, 1))),
    ("twist-up", join(pin(Port::A, 3), pin(Port::B, 3))),
    ("twist-up-fast", join(pin(Port::A, 3), pin(Port::B, 4))),
    ("twist-down", join(pin(Port::A, 4), pin(Port::B, 3))),
    ("twist-down-fast", join(pin(Port::A, 4), pin(Port::B, 4))),
    ("repeat", ground(pin(Port::B, 5))),
    ("slam", ground(pin(Port::B, 6))),
    ("menu", ground(pin(Port::B, 7))),
    ("vol-", ground(pin(Port::A, 6))),
    ("vol+", ground(pin(Port::A, 7))),
];

/// A wire written as pins (research): "C0" joins port C bit 0 to ground,
/// "B3-A2" joins two pins.
pub fn parse_wire(s: &str) -> Option<Wire> {
    let pin = |t: &str| -> Option<Pin> {
        let port = match t.as_bytes().first()? {
            b'A' => Port::A,
            b'B' => Port::B,
            b'C' => Port::C,
            _ => return None,
        };
        Some(Pin { port, bit: t[1..].parse().ok().filter(|b| *b < 8)? })
    };
    match s.split_once('-') {
        Some((a, b)) => Some(Wire { a: Some(pin(a)?), b: pin(b)? }),
        None => Some(Wire { a: None, b: pin(s)? }),
    }
}
