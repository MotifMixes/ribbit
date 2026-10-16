//! The touchscreen: a 4-wire resistive panel on GPIO pins 20-23, read
//! through the ADC (adc.rs), and the analog inputs the ADC converts.
//!
//! VERIFIED-FW (Leapster 2 BIOS 0.6 touch driver 0x400B_4F48-0x400B_56E6 and
//! touch task 0x400B_5ABE-0x400B_665C; the original's US v2.1 driver
//! 0x4009_0030-0x4009_05DC drives the same pins and inputs). After every sample
//! the driver discharges the panel (GIODataOut bits 20-23 low, then GIODire
//! bits 20-23 set, then cleared), drives one of these configurations, and
//! selects an ADC input. The configurations are a table in the driver's
//! context (Leapster 2: GIODire at +0x18, GIODataOut at +0x30, input at +0x48):
//!
//! | Entry | Driven low, high | Input |
//! |---|---|---|
//! | 0, 1 | 22, 23 | 3, 4 |
//! | 2, 3 | 20, 21 | 1, 2 |
//! | 4 | 21, 23 | 2 |
//! | 5 | 20, 22 | 1 |
//!
//! These are the measurements of a 4-wire resistive panel with one layer
//! between pins 21 and 23 (X) and the other between pins 20 and 22 (Y), the
//! analog inputs being 1 = pin 23, 2 = pin 22, 3 = pin 21, 4 = pin 20:
//!
//! | Entry | Drives | Senses | Measures |
//! |---|---|---|---|
//! | 4 | X layer (21 low, 23 high) | pin 22, on Y | X position |
//! | 5 | Y layer (20 low, 22 high) | pin 23, on X | Y position |
//! | 2, 0 | across the layers | pin 23 / 21, on the high-driven X layer | pressure (full scale with the pen up) |
//! | 3, 1 | across the layers | pin 22 / 20, on the low-driven Y layer | pressure (0 with the pen up) |
//!
//! Evidence for the assignment: the touch task converts the averages of
//! entries 4 and 5 (context +0xB6/+0xB8, copied to +0xC0/+0xC4 at 0x400B_653E)
//! to screen coordinates with the calibration matrix (0x400B_5764), so those
//! are the positions, and each drives one layer and senses the other. The
//! driver's cycle (states 0-5) uses entries 0, 2, 5, 4, 0, 2 and calls the pen
//! up when both of entries 0 and 2 read above 0x7F0 (context +0x98/+0x9A,
//! 0x400B_50AC and 0x400B_54F6): with no contact between the layers their
//! sensed pins sit on the X layer, which is driven high at one end. Every
//! entry senses a pin it does not drive. Which screen edge each pin is on is
//! UNVERIFIED: here X runs from pin 21 at the left to pin 23 at the right and
//! Y from pin 20 at the top to pin 22 at the bottom (the BIOS's calibration
//! maps whatever the orientation is to the screen).
//!
//! Model (component values UNVERIFIED, typical for such panels): layers of
//! 400 ohm, 200 ohm between the layers where the pen presses, solved as a
//! resistor network for the selected input; a node with no path to a driven
//! pin reads 0. Other inputs: 6 = battery (adc.rs), others read 0 (UNKNOWN;
//! input 7 is averaged once by the touch driver and the result is not used
//! elsewhere).
//!
//! The pen comes from a frontend (`live`, in "raw" units 0..=0x7F0 across the
//! screen, the units Ribbit's frontends and MAME use) or from scripted
//! presses (`TouchPress`).

use super::adc::{AnalogInputs, FULL_SCALE};

/// Raw pen units across the screen (MAME's input-port range).
pub const POSITION_MAX: u32 = 0x7F0;
const LAYER_X_OHMS: f64 = 400.0;
const LAYER_Y_OHMS: f64 = 400.0;
const CONTACT_OHMS: f64 = 200.0;
/// HYPOTHESIS: a fresh set of batteries. The BIOS accepts 0x98..0x7D3
/// (0x400E_2BA6) and grades the reading against a table (SysMgr 0x4005_4FE4).
pub const BATTERY_LEVEL: u32 = 0x600;

/// A scripted stylus press, in emulated time since reset. X/Y are raw units
/// (0..=0x7F0 across the screen).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TouchPress {
    pub start_ms: u64,
    pub duration_ms: u64,
    pub x: u32,
    pub y: u32,
}

impl std::str::FromStr for TouchPress {
    type Err = String;

    /// `START_MS,DURATION_MS,X,Y` (X/Y decimal or 0x-hex raw units).
    fn from_str(s: &str) -> Result<Self, String> {
        let parts: Vec<&str> = s.split(',').map(str::trim).collect();
        let [a, b, c, d] = parts[..] else {
            return Err(format!("expected START_MS,DURATION_MS,X,Y, got {s:?}"));
        };
        let num = |t: &str| -> Result<u64, String> {
            match t.strip_prefix("0x").or_else(|| t.strip_prefix("0X")) {
                Some(h) => u64::from_str_radix(h, 16),
                None => t.parse(),
            }
            .map_err(|e| format!("{t:?}: {e}"))
        };
        let (x, y) = (num(c)?, num(d)?);
        if x > POSITION_MAX as u64 || y > POSITION_MAX as u64 {
            return Err(format!("X/Y must be 0..={POSITION_MAX:#x}"));
        }
        Ok(Self { start_ms: num(a)?, duration_ms: num(b)?, x: x as u32, y: y as u32 })
    }
}

/// Scripted presses; kept across machine resets.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TouchConfig {
    /// Empty = the pen stays up unless a frontend sets `live`.
    pub presses: Vec<TouchPress>,
}

/// Former name (the touch model used to be MAME's).
pub type MameTouchConfig = TouchConfig;

/// The stylus.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Stylus {
    pub config: TouchConfig,
    /// Peripheral ticks since reset (for scripted presses).
    pub ticks: u64,
    /// Live stylus from a frontend (raw units); overrides the script.
    pub live: Option<(u32, u32)>,
}

impl Stylus {
    pub fn new(config: TouchConfig) -> Self {
        Self { config, ..Default::default() }
    }

    pub fn now_ms(&self) -> u64 {
        self.ticks / (super::Clock::PERIPHERAL_HZ / 1000)
    }

    /// The pen position (raw units) if it is down now.
    pub fn pen(&self) -> Option<(u32, u32)> {
        if self.live.is_some() {
            return self.live;
        }
        let ms = self.now_ms();
        self.config.presses.iter().find(|p| (p.start_ms..p.start_ms + p.duration_ms).contains(&ms)).map(|p| (p.x, p.y))
    }
}

/// What the ADC's inputs see: the panel's drive pins, the pen and the battery.
pub struct Analog {
    /// GIODataOut and GIODire.
    pub data_out: u32,
    pub dire: u32,
    pub pen: Option<(u32, u32)>,
    /// The battery input's level (`Peripherals::battery`).
    pub battery: u32,
}

impl Analog {
    /// The level at panel pin 20-23 when the ADC senses it.
    pub fn pin(&self, sensed: u32) -> u32 {
        // Nodes 0-3 = pins 20-23, 4 = the pen's contact point on the X layer
        // (pins 21-23), 5 = on the Y layer (pins 20-22).
        const N: usize = 6;
        let driven = |n: usize| -> Option<f64> {
            (n < 4 && self.dire >> (20 + n) & 1 != 0).then(|| (self.data_out >> (20 + n) & 1) as f64)
        };
        let s = (sensed - 20) as usize;
        if let Some(v) = driven(s) {
            return (v * FULL_SCALE as f64).round() as u32;
        }
        let frac = |r: u32| r.min(POSITION_MAX) as f64 / POSITION_MAX as f64;
        let (u, v) = self.pen.map_or((0.5, 0.5), |(x, y)| (frac(x), frac(y)));
        let mut edges = vec![
            (1, 4, LAYER_X_OHMS * u),
            (4, 3, LAYER_X_OHMS * (1.0 - u)),
            (0, 5, LAYER_Y_OHMS * v),
            (5, 2, LAYER_Y_OHMS * (1.0 - v)),
        ];
        if self.pen.is_some() {
            edges.push((4, 5, CONTACT_OHMS));
        }
        // Nodal analysis over the undriven nodes. Every node leaks a little to
        // ground, so a node with no path to a driven pin reads 0.
        let mut g = [[0.0f64; N]; N];
        let mut i = [0.0f64; N];
        for (n, row) in g.iter_mut().enumerate() {
            row[n] += 1e-9;
        }
        for &(a, b, ohms) in &edges {
            let c = 1.0 / ohms.max(1.0);
            for (p, q) in [(a, b), (b, a)] {
                match (driven(p), driven(q)) {
                    (Some(_), _) => {}
                    (None, Some(vq)) => {
                        g[p][p] += c;
                        i[p] += c * vq;
                    }
                    (None, None) => {
                        g[p][p] += c;
                        g[p][q] -= c;
                    }
                }
            }
        }
        let free: Vec<usize> = (0..N).filter(|&n| driven(n).is_none()).collect();
        let m = free.len();
        let mut a = vec![vec![0.0f64; m + 1]; m];
        for (r, &p) in free.iter().enumerate() {
            for (c, &q) in free.iter().enumerate() {
                a[r][c] = g[p][q];
            }
            a[r][m] = i[p];
        }
        for col in 0..m {
            let piv = (col..m).max_by(|&x, &y| a[x][col].abs().total_cmp(&a[y][col].abs())).unwrap();
            a.swap(col, piv);
            let pivot = a[col].clone();
            for (r, row) in a.iter_mut().enumerate() {
                if r != col {
                    let f = row[col] / pivot[col];
                    for (x, p) in row[col..].iter_mut().zip(&pivot[col..]) {
                        *x -= f * p;
                    }
                }
            }
        }
        let row = free.iter().position(|&n| n == s).unwrap();
        let volts = (a[row][m] / a[row][row]).clamp(0.0, 1.0);
        (volts * FULL_SCALE as f64).round() as u32
    }
}

impl AnalogInputs for Analog {
    fn level(&self, input: u32) -> u32 {
        match input {
            1 => self.pin(23),
            2 => self.pin(22),
            3 => self.pin(21),
            4 => self.pin(20),
            6 => self.battery,
            _ => 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Leapster 2 driver's table entries: (GIODire, GIODataOut, input).
    const ENTRY: [(u32, u32, u32); 6] = [
        (0x00C0_0000, 0x0080_0000, 3),
        (0x00C0_0000, 0x0080_0000, 4),
        (0x0030_0000, 0x0020_0000, 1),
        (0x0030_0000, 0x0020_0000, 2),
        (0x00A0_0000, 0x0080_0000, 2),
        (0x0050_0000, 0x0040_0000, 1),
    ];

    fn read(pen: Option<(u32, u32)>, entry: usize) -> u32 {
        let (dire, data_out, input) = ENTRY[entry];
        Analog { data_out, dire, pen, battery: BATTERY_LEVEL }.level(input)
    }

    #[test]
    fn pen_up_reads_full_scale_on_the_pen_up_check_entries() {
        // The driver calls the pen up when entries 0 and 2 both exceed 0x7F0.
        assert!(read(None, 0) > 0x7F0 && read(None, 2) > 0x7F0);
        assert_eq!((read(None, 1), read(None, 3)), (0, 0), "low-side pressure");
    }

    #[test]
    fn the_pen_is_seen_down_everywhere() {
        for x in (0..=0x7F0).step_by(0x7F) {
            for y in (0..=0x7F0).step_by(0x7F) {
                let p = Some((x, y));
                assert!(read(p, 0) <= 0x7F0 || read(p, 2) <= 0x7F0, "({x:#x}, {y:#x})");
            }
        }
    }

    #[test]
    fn positions_follow_the_pen() {
        for (x, y) in [(0x100, 0x700), (0x3F8, 0x3F8), (0x7F0, 0), (0, 0x7F0)] {
            let (rx, ry) = (read(Some((x, y)), 4), read(Some((x, y)), 5));
            assert!(rx.abs_diff(x) < 0x10, "x {x:#x} read {rx:#x}");
            assert!(ry.abs_diff(y) < 0x10, "y {y:#x} read {ry:#x}");
        }
    }

    #[test]
    fn a_driven_pin_reads_its_level() {
        // Pins 20 low and 21 high: input 3 (pin 21) and input 4 (pin 20).
        let a = Analog { data_out: 0x0020_0000, dire: 0x0030_0000, pen: None, battery: BATTERY_LEVEL };
        assert_eq!((a.level(3), a.level(4)), (FULL_SCALE, 0));
    }

    #[test]
    fn stylus_script_and_live() {
        let mut s = Stylus::new(TouchConfig { presses: vec![TouchPress { start_ms: 10, duration_ms: 5, x: 1, y: 2 }] });
        assert_eq!(s.pen(), None);
        s.ticks = 12 * 16_000;
        assert_eq!(s.pen(), Some((1, 2)));
        s.live = Some((3, 4));
        assert_eq!(s.pen(), Some((3, 4)));
    }

    #[test]
    fn press_parsing() {
        assert_eq!(
            "1000, 200, 0x3df, 64".parse::<TouchPress>(),
            Ok(TouchPress { start_ms: 1000, duration_ms: 200, x: 0x3DF, y: 64 })
        );
        assert!("1,2,3".parse::<TouchPress>().is_err());
        assert!("1,2,0x7f1,0".parse::<TouchPress>().is_err());
    }
}
