//! GPIO: the input register "GIODataIn", 0x0180_004C (read-only), and the
//! output and configuration registers.
//!
//! Evidence (docs/hardware-research.md §Phase 4c):
//!
//! | Bits | Meaning | Status |
//! |---|---|---|
//! | 0xB703_E380 | Buttons, active-low (layout in `ribbit_input::Button::mask`) | SOURCED (MAME input ports); VERIFIED-FW that both BIOSes poll the register (v1.5 button scanner 0x4007_ECC8; v2.1 0x4008_F638) |
//! | 11 | v1.5 SysMgr treats 0 as a shutdown request (mask 0x800, polarity 0) | VERIFIED-FW (behaviour); what drives the line, and its idle level, UNKNOWN |
//! | other non-button bits | read by SysMgr's other configured lines | UNKNOWN |
//!
//! Model: button bits come from the frontend's `Buttons` (1 = released).
//! Every other bit reads `unresolved_lines` (default 0, the value an
//! unmodelled register read before) and such reads are counted. MAME forces
//! those bits to 1 "to avoid detecting a shutdown for now"; Ribbit does not
//! adopt that guess, but `unresolved_lines` can be set explicitly for
//! experiments (`--input-lines`).
//!
//! Output and configuration registers (names VERIFIED-FW: the Leapster 2
//! BIOS 0.6 prints them on its UART, reading them from these addresses,
//! 0x4005_824E-0x4005_82BE): GIODataOut 0x0180_0040, GIOPull 0x0180_0044,
//! GIODire 0x0180_0048, GIOSel0 0x0180_0050, GIOSel1 0x0180_0054, GIOSel2
//! 0x0180_0060. Modelled as latches that read back what was written; their
//! effect on pins is not modelled except for the Leapster 2's SPI flash
//! (spiflash.rs). The original BIOSes and games also read-modify-write
//! 0x0180_0040/0048.

use ribbit_input::{BUTTON_BITS, Buttons};

pub const ADDR: u32 = 0x0180_004C;
pub const DATA_OUT: u32 = 0x0180_0040;
pub const PULL: u32 = 0x0180_0044;
pub const DIRE: u32 = 0x0180_0048;
pub const SEL0: u32 = 0x0180_0050;
pub const SEL1: u32 = 0x0180_0054;
pub const SEL2: u32 = 0x0180_0060;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GpioInput {
    /// Copied from the machine's `Buttons` before each run.
    pub buttons: Buttons,
    /// Value of every non-button bit (UNKNOWN on hardware; default 0).
    pub unresolved_lines: u32,
    pub reads: u64,
    /// GIODataOut, GIOPull, GIODire, GIOSel0, GIOSel1, GIOSel2.
    #[serde(default)]
    pub outputs: [u32; 6],
}

fn output_index(addr: u32) -> Option<usize> {
    [DATA_OUT, PULL, DIRE, SEL0, SEL1, SEL2].iter().position(|&a| a == addr)
}

impl GpioInput {
    pub fn read(&mut self, addr: u32) -> Option<u32> {
        if let Some(i) = output_index(addr) {
            return Some(self.outputs[i]);
        }
        if addr != ADDR {
            return None;
        }
        self.reads += 1;
        Some((self.buttons.gio_data_in() & BUTTON_BITS) | (self.unresolved_lines & !BUTTON_BITS))
    }
}

impl GpioInput {
    /// An output or configuration register; false elsewhere.
    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match output_index(addr) {
            Some(i) => {
                self.outputs[i] = value;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbit_input::Button;

    #[test]
    fn output_registers_read_back() {
        let mut g = GpioInput::default();
        assert!(g.write(DIRE, 0x870));
        assert_eq!(g.read(DIRE), Some(0x870));
        assert!(!g.write(ADDR, 1), "GIODataIn is read-only");
    }

    #[test]
    fn released_buttons_read_1_and_unknown_bits_read_0() {
        let mut g = GpioInput::default();
        assert_eq!(g.read(ADDR), Some(BUTTON_BITS), "no button pressed, other lines unresolved (0)");
        assert_eq!(g.read(ADDR).unwrap() & 0x800, 0, "bit 11 (v1.5 shutdown line) stays unresolved");
    }

    #[test]
    fn pressed_buttons_pull_their_bit_low() {
        let mut g = GpioInput::default();
        g.buttons.set(Button::A, true);
        g.buttons.set(Button::Up, true);
        assert_eq!(g.read(ADDR), Some(BUTTON_BITS & !(0x4000 | 0x8000_0000)));
    }

    #[test]
    fn unresolved_lines_are_an_explicit_setting_and_never_touch_button_bits() {
        let mut g = GpioInput { unresolved_lines: 0xFFFF_FFFF, ..Default::default() };
        g.buttons.set(Button::B, true);
        assert_eq!(g.read(ADDR), Some(!0x2000));
        assert_eq!(g.read(0x0180_0058), None, "no register there");
    }
}
