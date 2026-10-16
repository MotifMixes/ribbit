//! GPIO: the input register "GIODataIn", 0x0180_004C (read-only), and the
//! output and configuration registers.
//!
//! Evidence (docs/hardware-research.md §Phase 4c):
//!
//! | Bits | Meaning | Status |
//! |---|---|---|
//! | 0xB703_E380 | Buttons, active-low (layout in `ribbit_input::Button::mask`) | SOURCED (MAME input ports); VERIFIED-FW that both BIOSes poll the register (v1.5 button scanner 0x4007_ECC8; v2.1 0x4008_F638) |
//! | 30 | battery OK (SysMgr line A) | VERIFIED-FW, see below |
//! | 12 | batteries are not the rechargeable pack (SysMgr line B) | VERIFIED-FW, see below |
//! | 11 | v1.5 only: power-off request when 0 (SysMgr line C) | VERIFIED-FW, see below |
//! | other non-button bits | not read by the SysMgr's configured lines | UNKNOWN |
//!
//! The board lines. Every BIOS's SysMgr loads a battery-monitor resource
//! (`kSysMgrRsrcBatteryMonitorConfig`, ID 0x100B_0003; Leapster 2 0.6 at
//! 0x4013_E514, US v2.1 0x400F_D480, v1.5 0x4008_E12C, L-MAX v2.2
//! 0x4011_A79C, Leapster TV 0x400C_CC60) with three (mask, polarity) lines,
//! then makes those pins inputs and polls them with a debounce (Leapster 2
//! 0x4005_4E98-0x4005_4FB6):
//!
//! * A = bit 30, polarity 1. When it reads 0 the SysMgr posts a power event
//!   (0x1002_0002) and starts a 10-step countdown (`[ctx+0x5C] = 1`, `[ctx+0xB4]
//!   = 10`); seen in Ribbit as the low-battery warning, then power-off
//!   (US v2.1, Sonic X, ~9.5 s, once the ADC delivered battery samples). So 1 =
//!   battery OK.
//! * B = bit 12, polarity 1: 1 gives battery type 1, 0 type 2 (`[ctx+0xC2]`,
//!   then `[ctx+0xC8]`). Type 2 selects the second threshold table, and the
//!   Leapster 2's `LF_IsBatteryRechargeable` (0x400A_0364) returns type == 2.
//!   So 1 = ordinary batteries.
//! * C = bit 11, polarity 0, in v1.5 only (0 in the others). Active (0) is the
//!   shutdown request found earlier (docs/hardware-research.md, Phase 4c).
//!
//! Model: button bits come from the frontend's `Buttons` (1 = released). The
//! board lines read `board_lines`, by default a console with good ordinary
//! batteries and no power-off request (bits 30, 12 and 11 set). HYPOTHESIS:
//! what drives them electrically (a battery comparator, a battery-pack
//! contact, the power switch) is not known; only the BIOS's reading is. Every
//! other bit reads `unresolved_lines` (default 0) and such reads are counted.
//! MAME forces all non-button bits to 1 "to avoid detecting a shutdown for
//! now"; Ribbit does not adopt that guess for the unknown bits, but
//! `unresolved_lines` can be set explicitly for experiments (`--input-lines`).
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
/// GIODataIn bit 30: battery OK (SysMgr line A).
pub const BATTERY_OK: u32 = 1 << 30;
/// GIODataIn bit 12: ordinary batteries, not the rechargeable pack (line B).
pub const NOT_RECHARGEABLE: u32 = 1 << 12;
/// GIODataIn bit 11: v1.5's power-off request, active low (line C).
pub const NO_POWER_OFF_REQUEST: u32 = 1 << 11;
/// The board lines the SysMgr reads.
pub const BOARD_LINES: u32 = BATTERY_OK | NOT_RECHARGEABLE | NO_POWER_OFF_REQUEST;

fn default_board_lines() -> u32 {
    BOARD_LINES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GpioInput {
    /// Copied from the machine's `Buttons` before each run.
    pub buttons: Buttons,
    /// Value of the board lines (`BOARD_LINES` bits; default all set).
    #[serde(default = "default_board_lines")]
    pub board_lines: u32,
    /// Value of every other non-button bit (UNKNOWN on hardware; default 0).
    pub unresolved_lines: u32,
    pub reads: u64,
    /// GIODataOut, GIOPull, GIODire, GIOSel0, GIOSel1, GIOSel2.
    #[serde(default)]
    pub outputs: [u32; 6],
}

fn output_index(addr: u32) -> Option<usize> {
    [DATA_OUT, PULL, DIRE, SEL0, SEL1, SEL2].iter().position(|&a| a == addr)
}

impl Default for GpioInput {
    fn default() -> Self {
        Self { buttons: Buttons::default(), board_lines: BOARD_LINES, unresolved_lines: 0, reads: 0, outputs: [0; 6] }
    }
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
        Some(
            (self.buttons.gio_data_in() & BUTTON_BITS)
                | (self.board_lines & BOARD_LINES)
                | (self.unresolved_lines & !BUTTON_BITS & !BOARD_LINES),
        )
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
    fn released_buttons_and_board_lines_read_1_and_unknown_bits_read_0() {
        let mut g = GpioInput::default();
        assert_eq!(g.read(ADDR), Some(BUTTON_BITS | BOARD_LINES), "good batteries, no power-off request");
        g.board_lines = 0;
        assert_eq!(g.read(ADDR), Some(BUTTON_BITS), "battery low: bit 30 reads 0");
    }

    #[test]
    fn pressed_buttons_pull_their_bit_low() {
        let mut g = GpioInput::default();
        g.buttons.set(Button::A, true);
        g.buttons.set(Button::Up, true);
        assert_eq!(g.read(ADDR), Some((BUTTON_BITS | BOARD_LINES) & !(0x4000 | 0x8000_0000)));
    }

    #[test]
    fn unresolved_lines_are_an_explicit_setting_and_never_touch_button_bits() {
        let mut g = GpioInput { unresolved_lines: 0xFFFF_FFFF, board_lines: 0, ..Default::default() };
        g.buttons.set(Button::B, true);
        assert_eq!(g.read(ADDR), Some(!0x2000 & !BOARD_LINES), "board lines are separate");
        assert_eq!(g.read(0x0180_0058), None, "no register there");
    }
}
