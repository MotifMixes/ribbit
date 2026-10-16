//! Leapster 2: the SD card slot's pin block at 0x0180_00E0-0x0180_00F8.
//!
//! VERIFIED-FW (BIOS 0.6 `152-12206`; the SD driver's strings name it:
//! "gSDDMAContextPtr", "ResetAndGetToStandbyState", "!! Card Not Inserted !!"):
//!
//! | Register | Use by the BIOS | Meaning |
//! |---|---|---|
//! | 0x0180_00EC | read only. Card detect 0x400C_ED54: `(EC >> 6 & 1) ^ 1`; write-busy poll 0x400C_F766 loops while bit 1 is 0 and stops with "Card Ejected during write poll" if bit 6 is set | pin inputs: bit 6 = card absent, bit 1 = DAT0 level |
//! | 0x0180_00F0 | 0x7F normally; 0x7D (bit 1 clear) around the write-busy poll (0x400C_E824 / 0x400C_E83C) | which SD lines the controller drives (HYPOTHESIS: bit 1 = DAT0, released so its level can be read) |
//! | 0x0180_00E0, 00E4, 00E8, 00F4, 00F8 | written at init (0x400C_E7C0: E8 = 1, F0 = 0x7F, F4 = F8 = E4 = 0) and power-down (0x400C_E854: E8 = E0 = 0x7F, F0-F8 = 0) | UNKNOWN (pin function, pulls, power) |
//!
//! The SD host controller itself is at 0x0180_E400 (start/busy), 0x0180_E404
//! (command descriptor address) and 0x0180_E408 (clock/bus mode); it only runs
//! commands once a card is detected, so with no card it is not needed.
//!
//! Model: no card is inserted (the slot is empty, as on a console with no SD
//! card). EC reads bit 6 = 1 (no card) and bit 1 = 1 (DAT0 not busy). The
//! other input bits are UNKNOWN and read 0 (UNVERIFIED: the idle level of the
//! other SD lines is not read by the BIOS). The other registers are latches
//! that read back what was written (UNVERIFIED: no BIOS read of them seen).

pub const BASE: u32 = 0x0180_00E0;
pub const END: u32 = 0x0180_00F8;
pub const INPUTS: u32 = 0x0180_00EC;
/// 0x0180_00EC bit 6: 1 = no card in the slot.
pub const CARD_ABSENT: u32 = 1 << 6;
/// 0x0180_00EC bit 1: the DAT0 line (0 = the card is busy).
pub const DAT0: u32 = 1 << 1;

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SdSlot {
    /// Latches 0x0180_00E0..=0x0180_00F8 (the input register's entry unused).
    pub latches: [u32; 7],
}

fn index(addr: u32) -> Option<usize> {
    ((BASE..=END).contains(&addr) && addr.is_multiple_of(4)).then(|| ((addr - BASE) / 4) as usize)
}

impl SdSlot {
    pub fn inputs(&self) -> u32 {
        CARD_ABSENT | DAT0
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        if addr == INPUTS {
            return Some(self.inputs());
        }
        index(addr).map(|i| self.latches[i])
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match index(addr) {
            Some(_) if addr == INPUTS => false,
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
    fn empty_slot_reports_no_card_and_dat0_idle() {
        let s = SdSlot::default();
        let ec = s.read(INPUTS).unwrap();
        // The BIOS's card-detect routine (0x400C_ED54) returns 0 = no card.
        assert_eq!((ec >> 6 & 1) ^ 1, 0);
        assert_ne!(ec & DAT0, 0);
    }

    #[test]
    fn pin_registers_are_latches_and_inputs_are_read_only() {
        let mut s = SdSlot::default();
        assert!(s.write(0x0180_00F0, 0x7D));
        assert_eq!(s.read(0x0180_00F0), Some(0x7D));
        assert!(!s.write(INPUTS, 0));
        assert_eq!(s.read(0x0180_00FC), None);
        assert_eq!(s.read(0x0180_00DC), None);
    }
}
