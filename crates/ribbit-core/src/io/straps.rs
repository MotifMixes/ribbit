//! System configuration ("straps") register, 0x0180_9004 (read-only).
//!
//! A board-level input: its value is fixed by the hardware a unit was built
//! with, so it is a machine configuration value, not something derivable
//! from code. Evidence for the bits the BIOS uses (docs/hardware-research.md
//! §System configuration register):
//!
//! | Bits | Meaning | Evidence |
//! |---|---|---|
//! | 31, 17, 16 | LCD panel select | BIOS v1.5 `LcdMPI::Init` filter at 0x4007_B028 (v2.1 identical): accepts the LCD config resource whose key equals `0x8003_0000 & !straps`; the BIOS has 3 panel configs (keys 0, 0x8000_0000, 0x8001_0000) |
//! | 21 | On a fatal assert: 0 = spin forever, 1 = continue to the panic path | BIOS 0x4007_0240; MAME: "if not set ... infinite loops on error rather than panic" |
//! | 21 | Also selects a ROM checksum at boot (v1.5 0x4002_D698) | BIOS |
//! | 26 | 0 = cartridge present | MAME |
//! | 14 | 0 = boot into touch calibration | MAME |
//!
//! Default value: 0x63FF_BFFF, which MAME reports was "taken from testing
//! with a Leapster 2 with a retail cartridge". It is the only hardware
//! measurement available; the value of an original Leapster is UNKNOWN. It
//! selects panel config 2 (key 0x8000_0000). Bit 26 follows cartridge
//! presence, as MAME does.

pub const ADDR: u32 = 0x0180_9004;
/// MAME's Leapster 2 measurement (with a cartridge inserted).
pub const MAME_LEAPSTER2_MEASURED: u32 = 0x63FF_BFFF;
pub const CART_ABSENT: u32 = 1 << 26;
/// Leapster TV: MAME's value with bit 14 set. With bit 14 clear the TV BIOS
/// boots to a blank calibration screen (seen in Ribbit, 2026-10-15); a TV
/// keeps its pen-pad calibration in the controller instead. HYPOTHESIS.
pub const LEAPSTER_TV: u32 = 0x63FF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Straps {
    pub value: u32,
}

impl Straps {
    /// `base` with bit 26 set from cartridge presence.
    pub fn new(base: u32, cart_present: bool) -> Self {
        let value = if cart_present { base & !CART_ABSENT } else { base | CART_ABSENT };
        Self { value }
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        (addr == ADDR).then_some(self.value)
    }

    /// The LCD config key the BIOS will look for (`0x8003_0000 & !straps`).
    pub fn lcd_key(&self) -> u32 {
        0x8003_0000 & !self.value
    }
}

/// Clock control register 0x0180_9008: a read/write latch.
///
/// Evidence: both BIOSes read-modify-write its low 3 bits (to 1, then 5) at
/// boot and in a power-management loop (v1.5 0x4008_0450), which re-programs
/// the clock whenever the low bits read back as 0; MAME stores the value and
/// returns it. Whether it changes the CPU clock is UNKNOWN (MAME's
/// `96 MHz >> n` interpretation would make 5 mean 3 MHz, and MAME disables
/// it), so it has no effect on timing here.
pub const CLOCK_ADDR: u32 = 0x0180_9008;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClockControl {
    pub value: u32,
    pub writes: u64,
}

impl ClockControl {
    pub fn read(&self, addr: u32) -> Option<u32> {
        (addr == CLOCK_ADDR).then_some(self.value)
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        if addr != CLOCK_ADDR {
            return false;
        }
        self.value = value;
        self.writes += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cart_presence_drives_bit_26() {
        assert_eq!(Straps::new(MAME_LEAPSTER2_MEASURED, true).value, 0x63FF_BFFF);
        assert_eq!(Straps::new(MAME_LEAPSTER2_MEASURED, false).value, 0x67FF_BFFF);
    }

    #[test]
    fn default_selects_one_of_the_bios_panel_configs() {
        let keys = [0x0000_0000, 0x8000_0000, 0x8001_0000]; // BIOS tag 0x1008 records
        let s = Straps::new(MAME_LEAPSTER2_MEASURED, true);
        assert_eq!(s.lcd_key(), 0x8000_0000);
        assert!(keys.contains(&s.lcd_key()));
        assert!(
            !keys.contains(&Straps::new(0, true).lcd_key()),
            "all-zero straps match no panel (the observed assert)"
        );
    }

    #[test]
    fn clock_register_reads_back_what_was_written() {
        let mut c = ClockControl::default();
        assert_eq!(c.read(CLOCK_ADDR), Some(0));
        c.write(CLOCK_ADDR, (c.value & !7) | 1);
        c.write(CLOCK_ADDR, (c.value & !7) | 5);
        assert_eq!(c.read(CLOCK_ADDR), Some(5), "the BIOS's low-bits check now sees it configured");
        assert!(!c.write(CLOCK_ADDR + 4, 1));
    }

    #[test]
    fn read_only_at_its_address() {
        let s = Straps::new(1, true);
        assert_eq!(s.read(ADDR), Some(1));
        assert_eq!(s.read(ADDR + 4), None);
    }
}
