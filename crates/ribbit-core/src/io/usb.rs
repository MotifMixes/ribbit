//! Leapster 2: the USB 1.1 device controller at 0x0180_F000-0x0180_F01C, with
//! no host connected.
//!
//! VERIFIED-FW (BIOS 0.6; driver strings "UsbMPI", "USB: Attach Event",
//! "USB: Detached"; register base from 0x400C_67DC = 0x0180_F080 - 0x80):
//!
//! | Register | Use by the BIOS |
//! |---|---|
//! | 0x0180_F010 | written 0xFF at init (0x400C_041C; HYPOTHESIS: clear interrupts) |
//! | 0x0180_F014 | written 0, then 0x8D (0x400C_045C; HYPOTHESIS: interrupt enable) |
//! | 0x0180_F018 | status, read only: bit 2 tested by the attach check 0x400B_CB0C, which returns `(F018 >> 2 & 1) ^ 1` (1 = a host is attached); bit 7 at init (0x400C_0444) and in the event handler (0x400B_FE2A); bit 3 (0x400B_FECC) |
//! | 0x0180_F01C | control: 0x34 at init, read-modify-written with 0x30 / ~0xC0; at 3.7 s the base system pulses bits 0-1 (1, 0, then 2) around the attach check (0x400B_E130-0x400B_E180) |
//! | 0x0180_F000, F004 | byte reads at init (0x400B_E38E), meaning UNKNOWN |
//!
//! With these unmodelled (reading 0) the attach check reported a host, so a
//! Leapster 2 without a cartridge showed its LeapFrog Connect screen ("Click
//! Eject on the LeapFrog Connect Application to disconnect").
//!
//! Model: no USB cable. 0x0180_F018 reads bit 2 = 1; its other bits read 0
//! (UNVERIFIED: their idle values with no cable are not known). The other
//! registers are latches that read back what was written (UNVERIFIED).

pub const BASE: u32 = 0x0180_F000;
pub const END: u32 = 0x0180_F01C;
pub const STATUS: u32 = 0x0180_F018;
/// 0x0180_F018 bit 2: no host attached.
pub const NOT_ATTACHED: u32 = 1 << 2;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct UsbDevice {
    pub latches: [u32; 8],
}

fn index(addr: u32) -> Option<usize> {
    (BASE..=END).contains(&addr).then(|| ((addr - BASE) / 4) as usize)
}

impl UsbDevice {
    pub fn read(&self, addr: u32) -> Option<u32> {
        if addr & !3 == STATUS {
            return Some(NOT_ATTACHED);
        }
        index(addr & !3).map(|i| self.latches[i])
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match index(addr & !3) {
            Some(_) if addr & !3 == STATUS => false,
            Some(i) => {
                self.latches[i] = value;
                true
            }
            None => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attach_check_sees_no_host() {
        let u = UsbDevice::default();
        // The BIOS's check (0x400B_CB0C): (status >> 2 & 1) ^ 1.
        assert_eq!((u.read(STATUS).unwrap() >> 2 & 1) ^ 1, 0);
    }

    #[test]
    fn control_registers_are_latches() {
        let mut u = UsbDevice::default();
        assert!(u.write(0x0180_F01C, 0x34));
        assert_eq!(u.read(0x0180_F01C), Some(0x34));
        assert!(!u.write(STATUS, 0));
        assert_eq!(u.read(0x0180_F020), None);
    }
}
