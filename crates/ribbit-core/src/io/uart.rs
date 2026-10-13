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
//! * The BIOS device-clock query gives 16 MHz for 0x0180_D500; 0x0180_D518
//!   is written 0xCF (a baud divisor? UNKNOWN).
//!
//! Model: transmission completes instantly (no baud-rate timing), so bit 7 is
//! always set; nothing is ever received, so bit 5 is always set. Control
//! writes are recorded but have no effect; the UART interrupt is not raised.
//! The BIOS's printf path is interrupt-driven (0x44 enables the TX
//! interrupt), so only its first byte goes out; IRQ 0x1B timing and clearing
//! are UNKNOWN. 0x0180_D518/D51C stay unmodelled.

pub const DATA_ADDR: u32 = 0x0180_D510;
pub const STATUS_ADDR: u32 = 0x0180_D514;
pub const STATUS_TX_READY: u32 = 1 << 7;
pub const STATUS_RX_EMPTY: u32 = 1 << 5;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DebugUart {
    pub tx: Vec<u8>,
    /// Last value written to the status/control register.
    pub control: u32,
}

impl DebugUart {
    pub fn read(&self, addr: u32) -> Option<u32> {
        (addr == STATUS_ADDR).then_some(STATUS_TX_READY | STATUS_RX_EMPTY)
    }

    /// Accepts byte and word writes; the character is the low byte.
    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            DATA_ADDR => self.tx.push(value as u8),
            STATUS_ADDR => self.control = value & 0xFF,
            _ => return false,
        }
        true
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
        assert_eq!(u.read(0x0180_D518), None);
        assert!(!u.write(0x0180_D518, 0xCF));
    }

    #[test]
    fn bios_transmit_sequence() {
        // wait for bit 7, stb char to +0x10, stb 0x44 to +0x14
        let mut u = DebugUart::default();
        for &c in b"Hi\n" {
            assert_ne!(u.read(STATUS_ADDR).unwrap() & STATUS_TX_READY, 0);
            u.write(DATA_ADDR, c as u32);
            u.write(STATUS_ADDR, 0x44);
        }
        assert_eq!(u.text(), "Hi\n");
    }
}
