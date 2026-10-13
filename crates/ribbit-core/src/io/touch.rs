//! EXPERIMENTAL, UNVERIFIED: MAME's guessed touchscreen/ADC sample source.
//!
//! Off by default. When off, the ADC channel controls stay unmodelled and no
//! samples are ever produced (see adc.rs). When on, this reproduces MAME's
//! model so the BIOS can be followed past touch calibration. Nothing here is
//! known to match hardware; every run that uses it is labelled.
//!
//! Source: MAME `src/mame/leapfrog/leapster.cpp`, master at commit da56c5ac
//! (2026-10-07), `leapster_adc_w`, `leapster_touch_adc_update`, ports
//! TOUCHX/TOUCHY/TOUCH (BSD-3; behaviour reimplemented, no code copied).
//! What MAME does, and how sure it is (its own comments):
//!
//! | Behaviour | MAME | Status |
//! |---|---|---|
//! | 0x0180_0090-009C are 4 channel controls; reads return the last write | stores/returns them | MAME, no stated evidence |
//! | Channel 0 = touchscreen, 2 = "something to do with the LCD", 3 = "power / battery?", 1 = "unused?" | comment only; only channel 0 produces samples | guess |
//! | Writing ch0 control with bit 15 set starts conversions; clearing it stops them | yes | MAME |
//! | First start: 17 samples of 0 on channel 0 and an ADC interrupt | "whatever these are, they're never used" | guess (BIOS waits for 17 packets) |
//! | Then every 1/60 s: 12 samples P,P,X,Y,P,P,P,P,X,Y,P,P on channel 0, then the interrupt | "not sure what the intended frequency is, so I've just set it at 60"; P "pressure or something?" | guess |
//! | P = 0x7FF when not touched, 0 when touched | an average > 0x7F0 "will get interpreted as there not being a touch" | guess |
//! | X/Y = raw position, 0..0x7F0, idle 0x3DF | input-port range and default | MAME's choice |
//! | ADC interrupt: flag bit 8 + CPU line 0x10, not gated by the enable register | yes; MAME clears its pending bit on entry | MAME |
//! | FIFO depth 0x10000, overflow is fatal | "not sure what the depth ... is" | guess |
//!
//! Ribbit differences, all forced by its design and documented: line 0x10 is
//! a level (flag bit 8), dropped when the BIOS acknowledges bit 8, instead of
//! MAME's clear-on-entry; FIFO overflow drops the sample and is counted
//! instead of aborting; time is the 16 MHz peripheral clock (MAME uses
//! emulated wall time with its CPU at 48 MHz, Ribbit 96 MHz nominal).
//! Channel 2 produces nothing, as in MAME.

use super::adc::Adc;
use super::intc::{FLAG_ADC, IntFlags};

pub const CONTROL_BASE: u32 = 0x0180_0090;
pub const CONTROL_END: u32 = 0x0180_009C;
pub const CHANNEL_ENABLE: u32 = 0x8000;
pub const TOUCH_CHANNEL: u32 = 0;
/// MAME's update rate (a guess).
pub const UPDATE_HZ: u64 = 60;
/// MAME's init burst.
pub const INIT_SAMPLES: usize = 17;
/// MAME's "pressure" values.
pub const PRESSURE_UP: u32 = 0x7FF;
pub const PRESSURE_DOWN: u32 = 0;
/// MAME's X/Y input-port range and default (idle) value.
pub const POSITION_MAX: u32 = 0x7F0;
pub const POSITION_IDLE: u32 = 0x3DF;
/// MAME's FIFO depth.
pub const FIFO_CAPACITY: usize = 0x10000;

/// A scripted stylus press, in emulated time since reset. X/Y are raw ADC
/// values in MAME's input-port units (0..=0x7F0); how they map to pixels on a
/// real panel is UNKNOWN (that is what calibration measures).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TouchPress {
    pub start_ms: u64,
    pub duration_ms: u64,
    pub x: u32,
    pub y: u32,
}

impl std::str::FromStr for TouchPress {
    type Err = String;

    /// `START_MS,DURATION_MS,X,Y` (X/Y decimal or 0x-hex raw ADC units).
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
            return Err(format!("X/Y must be 0..={POSITION_MAX:#x} (MAME's range)"));
        }
        Ok(Self { start_ms: num(a)?, duration_ms: num(b)?, x: x as u32, y: y as u32 })
    }
}

/// Configuration of the experimental model; kept across machine resets.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MameTouchConfig {
    /// Empty = no-touch mode (stylus always up).
    pub presses: Vec<TouchPress>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MameTouch {
    pub config: MameTouchConfig,
    peripheral_hz: u64,
    pub channel_control: [u32; 4],
    pub initted: bool,
    pub running: bool,
    /// Elapsed time in peripheral ticks x UPDATE_HZ (exact 60 Hz arithmetic).
    now: u128,
    /// When the next update is due, same units.
    due: u128,
    /// 60 Hz updates delivered, and how many of them saw the stylus down.
    pub updates: u64,
    pub touched_updates: u64,
    pub dropped_samples: u64,
    /// Live stylus from a frontend (raw X/Y, MAME units); overrides the script.
    pub live: Option<(u32, u32)>,
}

impl MameTouch {
    pub fn new(config: MameTouchConfig, peripheral_hz: u64) -> Self {
        assert!(peripheral_hz > 0);
        Self {
            config,
            peripheral_hz,
            channel_control: [0; 4],
            initted: false,
            running: false,
            now: 0,
            due: 0,
            updates: 0,
            touched_updates: 0,
            dropped_samples: 0,
            live: None,
        }
    }

    fn period(&self) -> u128 {
        self.peripheral_hz as u128
    }

    /// Emulated milliseconds since reset.
    pub fn now_ms(&self) -> u64 {
        self.ms_at(self.now)
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        Self::channel(addr).map(|ch| self.channel_control[ch])
    }

    fn channel(addr: u32) -> Option<usize> {
        ((CONTROL_BASE..=CONTROL_END).contains(&addr) && addr.is_multiple_of(4))
            .then(|| ((addr - CONTROL_BASE) / 4) as usize)
    }

    pub fn write(&mut self, addr: u32, value: u32, adc: &mut Adc, intc: &mut IntFlags) -> bool {
        let Some(ch) = Self::channel(addr) else {
            return false;
        };
        if ch == TOUCH_CHANNEL as usize {
            if value & CHANNEL_ENABLE != 0 {
                if self.initted && !self.running && self.due < self.now {
                    // MAME's timer keeps its expiry while disabled, so a late
                    // re-enable fires at once, then every 1/60 s from then.
                    self.due = self.now;
                }
                self.running = true;
                if !self.initted {
                    for _ in 0..INIT_SAMPLES {
                        self.push(adc, 0);
                    }
                    intc.raise(FLAG_ADC);
                    self.initted = true;
                    self.due = self.now + self.period();
                }
            } else {
                self.running = false;
            }
        }
        self.channel_control[ch] = value;
        true
    }

    fn push(&mut self, adc: &mut Adc, data: u32) {
        if adc.fifo.len() >= FIFO_CAPACITY {
            self.dropped_samples += 1;
        } else {
            adc.push(TOUCH_CHANNEL, data);
        }
    }

    /// Emulated milliseconds at time `t` (internal units).
    fn ms_at(&self, t: u128) -> u64 {
        (t * 1000 / (self.peripheral_hz as u128 * UPDATE_HZ as u128)) as u64
    }

    /// The scripted press active at the current time, if any.
    pub fn active_press(&self) -> Option<TouchPress> {
        self.press_at(self.now_ms())
    }

    fn press_at(&self, ms: u64) -> Option<TouchPress> {
        if let Some((x, y)) = self.live {
            return Some(TouchPress { start_ms: ms, duration_ms: 1, x, y });
        }
        self.config.presses.iter().copied().find(|p| (p.start_ms..p.start_ms + p.duration_ms).contains(&ms))
    }

    /// Peripheral ticks until the next update can happen (u64::MAX if none).
    pub fn ticks_to_next_update(&self) -> u64 {
        if !self.running {
            return u64::MAX;
        }
        let gap = self.due.saturating_sub(self.now);
        gap.div_ceil(UPDATE_HZ as u128).min(u64::MAX as u128) as u64
    }

    pub fn advance(&mut self, ticks: u64, adc: &mut Adc, intc: &mut IntFlags) {
        self.now += ticks as u128 * UPDATE_HZ as u128;
        while self.running && self.now >= self.due {
            self.update(self.ms_at(self.due), adc, intc);
            self.due += self.period();
        }
    }

    fn update(&mut self, ms: u64, adc: &mut Adc, intc: &mut IntFlags) {
        let (pressure, x, y) = match self.press_at(ms) {
            Some(p) => {
                self.touched_updates += 1;
                (PRESSURE_DOWN, p.x, p.y)
            }
            None => (PRESSURE_UP, POSITION_IDLE, POSITION_IDLE),
        };
        for _ in 0..2 {
            for v in [pressure, pressure, x, y, pressure, pressure] {
                self.push(adc, v);
            }
        }
        intc.raise(FLAG_ADC);
        self.updates += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::adc::{DATA_ADDR, STATUS_ADDR, STATUS_EMPTY};

    const HZ: u64 = 16_000_000;
    /// 1/60 s is 266,666 2/3 ticks; an update lands on tick 266,667.
    const PERIOD_CEIL: u64 = 266_667;

    fn setup(presses: Vec<TouchPress>) -> (MameTouch, Adc, IntFlags) {
        (MameTouch::new(MameTouchConfig { presses }, HZ), Adc::default(), IntFlags::default())
    }

    fn drain(adc: &mut Adc) -> Vec<u32> {
        let mut out = Vec::new();
        while adc.read(STATUS_ADDR).unwrap() & STATUS_EMPTY == 0 {
            out.push(adc.read(DATA_ADDR).unwrap());
        }
        out
    }

    /// The BIOS ISR's decode (v1.5 0x4007_CBB0).
    fn decode(word: u32) -> (u32, u32) {
        ((word >> 16) & 3, (word & 0xFFFF) >> 5)
    }

    #[test]
    fn channel_controls_read_back() {
        let (mut t, mut adc, mut intc) = setup(vec![]);
        assert!(t.write(0x0180_0098, 0x8000, &mut adc, &mut intc));
        assert_eq!(t.read(0x0180_0098), Some(0x8000));
        assert_eq!(t.read(0x0180_00A0), None, "data/status belong to the FIFO");
        assert!(!t.write(0x0180_00A4, 1, &mut adc, &mut intc));
        assert!(adc.fifo.is_empty(), "channel 2 produces nothing (as in MAME)");
        assert_eq!(intc.flags, 0);
    }

    #[test]
    fn first_enable_pushes_17_zero_samples_and_interrupts() {
        let (mut t, mut adc, mut intc) = setup(vec![]);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        assert_eq!(intc.flags, FLAG_ADC);
        assert_eq!(drain(&mut adc), vec![0; INIT_SAMPLES]);
        // Re-enabling does not repeat the burst.
        t.write(CONTROL_BASE, 0, &mut adc, &mut intc);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        assert!(adc.fifo.is_empty());
    }

    #[test]
    fn no_touch_update_format_and_rate() {
        let (mut t, mut adc, mut intc) = setup(vec![]);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        drain(&mut adc);
        intc.write(super::super::intc::FLAGS_ADDR, FLAG_ADC);
        t.advance(PERIOD_CEIL - 1, &mut adc, &mut intc);
        assert!(adc.fifo.is_empty(), "nothing before 1/60 s");
        t.advance(1, &mut adc, &mut intc);
        let words = drain(&mut adc);
        let (p, i) = (PRESSURE_UP, POSITION_IDLE);
        let expect: Vec<(u32, u32)> = [p, p, i, i, p, p, p, p, i, i, p, p].iter().map(|&v| (0, v)).collect();
        assert_eq!(words.iter().map(|&w| decode(w)).collect::<Vec<_>>(), expect);
        assert_eq!(words[0], 0x7FF << 5, "value in bits 15:5, channel 0 in bits 17:16");
        assert_eq!(intc.flags, FLAG_ADC);
        // One more simulated second = exactly 60 more updates (no drift).
        t.advance(HZ, &mut adc, &mut intc);
        assert_eq!(t.updates, 61);
    }

    #[test]
    fn scripted_press_reports_position_and_zero_pressure() {
        let press = TouchPress { start_ms: 105, duration_ms: 50, x: 0x100, y: 0x700 };
        let (mut t, mut adc, mut intc) = setup(vec![press]);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        drain(&mut adc);
        t.advance(HZ / 10, &mut adc, &mut intc); // to 100 ms: 6 untouched updates
        assert_eq!((t.updates, t.touched_updates), (6, 0));
        drain(&mut adc);
        t.advance(PERIOD_CEIL, &mut adc, &mut intc); // 116.7 ms
        let v: Vec<u32> = drain(&mut adc).iter().map(|&w| decode(w).1).collect();
        assert_eq!(v, [0, 0, 0x100, 0x700, 0, 0, 0, 0, 0x100, 0x700, 0, 0]);
        t.advance(HZ / 10, &mut adc, &mut intc); // past the press
        let v: Vec<u32> = drain(&mut adc).iter().map(|&w| decode(w).1).collect();
        assert_eq!(
            &v[v.len() - 12..],
            [0x7FF, 0x7FF, 0x3DF, 0x3DF, 0x7FF, 0x7FF, 0x7FF, 0x7FF, 0x3DF, 0x3DF, 0x7FF, 0x7FF]
        );
        assert_eq!(t.touched_updates, 3, "updates at 116.7, 133.3 and 150 ms fall in 105..155 ms");
    }

    #[test]
    fn disabling_stops_updates_and_late_reenable_fires_once() {
        let (mut t, mut adc, mut intc) = setup(vec![]);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        t.write(CONTROL_BASE, 0, &mut adc, &mut intc);
        t.advance(HZ, &mut adc, &mut intc);
        assert_eq!(t.updates, 0);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        t.advance(1, &mut adc, &mut intc);
        assert_eq!(t.updates, 1, "overdue update fires at once");
        // The next one is 1/60 s after the enable write (1 tick already gone).
        t.advance(PERIOD_CEIL - 2, &mut adc, &mut intc);
        assert_eq!(t.updates, 1);
        t.advance(1, &mut adc, &mut intc);
        assert_eq!(t.updates, 2);
    }

    #[test]
    fn live_stylus_overrides_the_script() {
        let (mut t, mut adc, mut intc) = setup(vec![]);
        t.write(CONTROL_BASE, CHANNEL_ENABLE, &mut adc, &mut intc);
        drain(&mut adc);
        t.live = Some((0x123, 0x456));
        t.advance(PERIOD_CEIL, &mut adc, &mut intc);
        let v: Vec<u32> = drain(&mut adc).iter().map(|&w| decode(w).1).collect();
        assert_eq!(&v[..6], [0, 0, 0x123, 0x456, 0, 0]);
        t.live = None;
        t.advance(PERIOD_CEIL, &mut adc, &mut intc);
        assert_eq!(decode(drain(&mut adc)[0]).1, PRESSURE_UP);
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
