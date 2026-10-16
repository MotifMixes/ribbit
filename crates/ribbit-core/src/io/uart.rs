//! Debug UART at 0x0180_D500 (byte-wide registers).
//!
//! Evidence:
//! * MAME `leapster.cpp`: writing 0x0180_D510 sends a character; status at
//!   0x0180_D514: bit 7 (R) = ready for output, bit 5 (R) = 0 when a received
//!   byte is waiting, bit 6 (W) = send, bit 2 (W) "IRQ clear?"; MAME raises
//!   IRQ 0x1B when 0x44 is written.
//! * Both BIOSes' polled transmit routine (v1.5 0x4007_8C68, v2.1
//!   0x4008_A482): wait for bit 7 of 0x0180_D514, `stb` the character to
//!   0x0180_D510, then `stb 0x44` to 0x0180_D514.
//! * The BIOS device-clock query gives 16 MHz for 0x0180_D500.
//! * The MQX serial driver in cartridges (Cars, 0x8003_73B4-0x8003_7710;
//!   VERIFIED-FW): +0x18/+0x1C = low/high byte of a divisor computed as
//!   ((16,000,000 / (2 x baud)) + 1) / 2 - 1, i.e. baud = 16 MHz / (4 x
//!   (divisor + 1)); 0xCF is 19,231 baud. Its interrupt handler (0x8003_762C)
//!   reads +0x10 while status bit 5 is clear, writes the next queued byte to
//!   +0x10 while bit 7 is set, then writes control 0x44 if bytes remain and
//!   0x04 if not: control bit 6 enables a level interrupt for "transmitter
//!   ready", bit 2 one for "byte received". Its transmit start (0x8003_76E8)
//!   writes the first byte, then 0x44, and leaves the rest to the interrupt.
//!   The Leapster TV's second UART (tv_link.rs) has the same layout.
//!
//! Model: a written byte keeps the transmitter busy (bit 7 clear) for one
//! 10-bit character at the programmed baud rate; the interrupt line (MAME:
//! 0x1B) is asserted while control bit 6 is set and the transmitter is idle.
//! The divisor formula and 10 bits per character (8N1) are from the driver,
//! not measured on hardware. Nothing is ever received, so bit 5 is always
//! set and the receive interrupt never fires.

pub const DATA_ADDR: u32 = 0x0180_D510;
pub const STATUS_ADDR: u32 = 0x0180_D514;
pub const STATUS_TX_READY: u32 = 1 << 7;
pub const STATUS_RX_EMPTY: u32 = 1 << 5;
pub const DIVISOR_LO_ADDR: u32 = 0x0180_D518;
pub const DIVISOR_HI_ADDR: u32 = 0x0180_D51C;
/// Control bit 6: interrupt while the transmitter is ready.
pub const CTRL_TX_IRQ: u32 = 1 << 6;
/// CPU interrupt line (MAME `leapster.cpp`).
pub const IRQ_LINE: u8 = 0x1B;
/// Bits per character on the wire: start, 8 data, stop.
const BITS_PER_CHAR: u64 = 10;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugUart {
    pub tx: Vec<u8>,
    /// Last value written to the status/control register.
    pub control: u32,
    /// Baud divisor (+0x18 low byte, +0x1C high byte).
    #[serde(default)]
    pub divisor: u32,
    /// Peripheral-clock ticks until the character being sent is out.
    #[serde(default)]
    pub busy: u64,
}

impl DebugUart {
    pub fn read(&self, addr: u32) -> Option<u32> {
        match addr {
            STATUS_ADDR => Some(if self.busy == 0 { STATUS_TX_READY } else { 0 } | STATUS_RX_EMPTY),
            DIVISOR_LO_ADDR => Some(self.divisor & 0xFF),
            DIVISOR_HI_ADDR => Some(self.divisor >> 8 & 0xFF),
            _ => None,
        }
    }

    /// Accepts byte and word writes; the character is the low byte.
    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            DATA_ADDR => {
                self.tx.push(value as u8);
                self.busy = self.char_ticks();
            }
            STATUS_ADDR => self.control = value & 0xFF,
            DIVISOR_LO_ADDR => self.divisor = self.divisor & 0xFF00 | value & 0xFF,
            DIVISOR_HI_ADDR => self.divisor = self.divisor & 0xFF | (value & 0xFF) << 8,
            _ => return false,
        }
        true
    }

    /// Peripheral-clock ticks (16 MHz) to send one character.
    pub fn char_ticks(&self) -> u64 {
        BITS_PER_CHAR * 4 * (self.divisor as u64 + 1)
    }

    /// The interrupt output (level).
    pub fn irq_line(&self) -> bool {
        self.control & CTRL_TX_IRQ != 0 && self.busy == 0
    }

    /// Ticks until the transmitter becomes ready (the only time the
    /// interrupt line can change by itself).
    pub fn ticks_to_ready(&self) -> u64 {
        if self.busy == 0 { u64::MAX } else { self.busy }
    }

    pub fn advance(&mut self, ticks: u64) {
        self.busy = self.busy.saturating_sub(ticks);
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.tx).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_transmitted_bytes() {
        let mut u = DebugUart::default();
        assert!(u.write(DATA_ADDR, b'O' as u32));
        assert!(u.write(DATA_ADDR, 0x1234_5600 | b'K' as u32));
        assert_eq!(u.text(), "OK");
    }

    #[test]
    fn status_reports_ready_and_no_receive_data() {
        let mut u = DebugUart::default();
        assert_eq!(u.read(STATUS_ADDR), Some(0xA0));
        assert!(u.write(STATUS_ADDR, 0x44));
        assert_eq!(u.control, 0x44);
        assert_eq!(u.read(STATUS_ADDR), Some(0xA0), "status is not the control value");
        assert!(u.write(DIVISOR_LO_ADDR, 0xCF));
        assert_eq!(u.read(DIVISOR_LO_ADDR), Some(0xCF));
        assert_eq!(u.read(0x0180_D520), None);
    }

    #[test]
    fn transmitter_busy_for_one_character_then_interrupts() {
        let mut u = DebugUart::default();
        u.write(DIVISOR_LO_ADDR, 0xCF);
        u.write(DIVISOR_HI_ADDR, 0);
        // 19,231 baud: 10 bits = 520 us = 8320 ticks at 16 MHz.
        assert_eq!(u.char_ticks(), 8320);
        u.write(DATA_ADDR, b'[' as u32);
        u.write(STATUS_ADDR, 0x44);
        assert_eq!(u.read(STATUS_ADDR), Some(STATUS_RX_EMPTY), "busy: not ready");
        assert!(!u.irq_line());
        assert_eq!(u.ticks_to_ready(), 8320);
        u.advance(8319);
        assert!(!u.irq_line());
        u.advance(1);
        assert_eq!(u.read(STATUS_ADDR), Some(0xA0));
        assert!(u.irq_line(), "TX interrupt enabled and transmitter ready");
        u.write(STATUS_ADDR, 0x04);
        assert!(!u.irq_line(), "0x04: receive interrupt only (nothing received)");
    }

    #[test]
    fn bios_transmit_sequence() {
        // wait for bit 7, stb char to +0x10, stb 0x44 to +0x14
        let mut u = DebugUart::default();
        for &c in b"Hi\n" {
            assert_ne!(u.read(STATUS_ADDR).unwrap() & STATUS_TX_READY, 0);
            u.write(DATA_ADDR, c as u32);
            u.write(STATUS_ADDR, 0x44);
            u.advance(u.ticks_to_ready());
        }
        assert_eq!(u.text(), "Hi\n");
    }
}
