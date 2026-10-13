//! System EEPROM controller (0x0180_0030-003C) and the 24LC02B behind it.
//!
//! Evidence (docs/hardware-research.md §Phase 4d):
//!
//! | Behaviour | Status | Evidence |
//! |---|---|---|
//! | 0x0180_0030 command: address << 16, device code in bits 15:8, write data in bits 7:0 | VERIFIED-FW + SOURCED | v1.5 read routine 0x4008_1924 builds `addr << 16 \| type \| table[idx]` (type 0x2000, table[1] = 0x600, so device 0x26); MAME decodes the same fields |
//! | 0x0180_0034 = 1 starts a read, = 2 a write | VERIFIED-FW (read) / SOURCED (write, MAME) | v1.5 0x4008_197C writes 1 before polling; MAME performs writes on 2 |
//! | Completion is latched in interrupt-flag bit 7; software clears bits 7/6 (writes 0xC0) before each command | VERIFIED-FW | v1.5 0x4008_195E / 0x4008_1982-19C0; MAME instead forces bit 7 on in every read ("EEPROM IO expects bit 7 to be set as a ready bit") |
//! | 0x0180_0038 returns the byte read | VERIFIED-FW + SOURCED | v1.5 0x4008_19C4; MAME |
//! | 0x0180_003C = mode (the BIOS writes 3) | VERIFIED-FW (write only); meaning UNKNOWN | stored, no effect |
//! | Device 0x26 = the system EEPROM, a 24LC02B (256 bytes) | SOURCED | MAME's device test (it calls it "hacky"); MAME's PCB notes name a 24LC02B. MAME emulates 0x200 bytes (CONFLICT, open question Q6) |
//! | Erased contents 0xFF | HYPOTHESIS | typical of erased serial EEPROMs; MAME starts at 0 and ships "calibrated defaults" |
//! | Latency | UNKNOWN | modelled as immediate |
//!
//! | Device 0x20 = the cartridge save chip | VERIFIED-FW (use) + SOURCED (size) | Both BIOSes address device 0x20 only with a cartridge inserted (SpongeBob: 256 writes, addresses 0-0xFF); MAME routes non-0x26 commands to the cartridge NVRAM, 0x800 bytes, "exact NVRAM/EEPROM type unknown" |
//!
//! The BIOS device table gives four codes, 0x20/0x22/0x24/0x26 (type 0x2000
//! plus 0x000/0x200/0x400/0x600). Commands for 0x22/0x24, and for 0x20 with
//! no cartridge, get no completion, so the BIOS times out exactly as with no
//! device; they are counted.

use super::intc::{FLAG_EEPROM_DONE, IntFlags};

pub const COMMAND_ADDR: u32 = 0x0180_0030;
pub const CONTROL_ADDR: u32 = 0x0180_0034;
pub const DATA_ADDR: u32 = 0x0180_0038;
pub const MODE_ADDR: u32 = 0x0180_003C;
pub const SYSTEM_DEVICE: u32 = 0x26;
pub const CART_DEVICE: u32 = 0x20;
/// MAME's cartridge NVRAM size (chip type UNKNOWN).
pub const CART_SIZE: usize = 0x800;
pub const SIZE: usize = 256;
pub const ERASED: u8 = 0xFF;
const START_READ: u32 = 1;
const START_WRITE: u32 = 2;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Eeprom {
    pub data: Vec<u8>,
    /// Cartridge save chip (device 0x20); None without a cartridge.
    pub cart: Option<Vec<u8>>,
    pub cart_dirty: bool,
    pub command: u32,
    pub mode: u32,
    pub read_value: u32,
    pub reads: u64,
    pub writes: u64,
    /// Set when the contents change (for saving).
    pub dirty: bool,
    /// Commands for devices other than the system EEPROM (no response).
    pub other_device_commands: u64,
    /// The last such command word and start value (for diagnosis).
    pub last_other_command: Option<(u32, u32)>,
    /// Addresses beyond the 256-byte chip (wrapped, as a 24LC02B does).
    pub wrapped_addresses: u64,
}

impl Default for Eeprom {
    fn default() -> Self {
        Self {
            data: vec![ERASED; SIZE],
            cart: None,
            cart_dirty: false,
            command: 0,
            mode: 0,
            read_value: 0,
            reads: 0,
            writes: 0,
            dirty: false,
            other_device_commands: 0,
            last_other_command: None,
            wrapped_addresses: 0,
        }
    }
}

impl Eeprom {
    /// Loads saved contents (shorter files are padded with the erased value).
    pub fn load(&mut self, bytes: &[u8]) {
        self.data = vec![ERASED; SIZE];
        let n = bytes.len().min(SIZE);
        self.data[..n].copy_from_slice(&bytes[..n]);
        self.dirty = false;
    }

    /// Inserts a cartridge save chip, erased or with saved contents.
    pub fn insert_cart(&mut self, saved: Option<&[u8]>) {
        let mut chip = vec![ERASED; CART_SIZE];
        if let Some(bytes) = saved {
            let n = bytes.len().min(CART_SIZE);
            chip[..n].copy_from_slice(&bytes[..n]);
        }
        self.cart = Some(chip);
        self.cart_dirty = false;
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        match addr {
            COMMAND_ADDR => Some(self.command),
            DATA_ADDR => Some(self.read_value),
            MODE_ADDR => Some(self.mode),
            _ => None,
        }
    }

    pub fn write(&mut self, addr: u32, value: u32, intc: &mut IntFlags) -> bool {
        match addr {
            COMMAND_ADDR => self.command = value,
            MODE_ADDR => self.mode = value,
            CONTROL_ADDR => self.start(value, intc),
            _ => return false,
        }
        true
    }

    fn start(&mut self, op: u32, intc: &mut IntFlags) {
        if op != START_READ && op != START_WRITE {
            return;
        }
        let device = (self.command >> 8) & 0xFF;
        let address = (self.command >> 16) as usize;
        let (chip, dirty) = match (device, self.cart.as_mut()) {
            (SYSTEM_DEVICE, _) => (&mut self.data, &mut self.dirty),
            (CART_DEVICE, Some(cart)) => (cart, &mut self.cart_dirty),
            _ => {
                self.other_device_commands += 1;
                self.last_other_command = Some((self.command, op));
                return;
            }
        };
        if address >= chip.len() {
            self.wrapped_addresses += 1;
        }
        let a = address % chip.len();
        if op == START_READ {
            self.read_value = chip[a] as u32;
            self.reads += 1;
        } else {
            let byte = self.command as u8;
            if chip[a] != byte {
                chip[a] = byte;
                *dirty = true;
            }
            self.writes += 1;
        }
        intc.raise(FLAG_EEPROM_DONE);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::intc::FLAGS_ADDR;

    fn cmd(addr: u32, device: u32, data: u32) -> u32 {
        (addr << 16) | (device << 8) | data
    }

    #[test]
    fn bios_read_sequence_completes_through_flag_bit_7() {
        let (mut e, mut intc) = (Eeprom::default(), IntFlags::default());
        e.data[5] = 0x42;
        // v1.5 0x4008_1958..19C4: mode 3, clear flags 7/6, command, start(1), poll bit 7, read data.
        e.write(MODE_ADDR, 3, &mut intc);
        intc.write(FLAGS_ADDR, 0xC0);
        e.write(COMMAND_ADDR, cmd(5, 0x26, 0), &mut intc);
        assert_eq!(intc.flags & FLAG_EEPROM_DONE, 0);
        e.write(CONTROL_ADDR, 1, &mut intc);
        assert_eq!(intc.flags & FLAG_EEPROM_DONE, FLAG_EEPROM_DONE);
        assert_eq!(e.read(DATA_ADDR), Some(0x42));
    }

    #[test]
    fn writes_store_the_low_byte_and_mark_dirty() {
        let (mut e, mut intc) = (Eeprom::default(), IntFlags::default());
        e.write(COMMAND_ADDR, cmd(0x10, 0x26, 0xAB), &mut intc);
        e.write(CONTROL_ADDR, 2, &mut intc);
        assert_eq!((e.data[0x10], e.dirty, e.writes), (0xAB, true, 1));
        e.write(COMMAND_ADDR, cmd(0x10, 0x26, 0), &mut intc);
        e.write(CONTROL_ADDR, 1, &mut intc);
        assert_eq!(e.read(DATA_ADDR), Some(0xAB));
    }

    #[test]
    fn other_devices_never_complete() {
        let (mut e, mut intc) = (Eeprom::default(), IntFlags::default());
        e.write(COMMAND_ADDR, cmd(0, 0x22, 0), &mut intc);
        e.write(CONTROL_ADDR, 1, &mut intc);
        assert_eq!((intc.flags, e.other_device_commands), (0, 1));
    }

    #[test]
    fn blank_chip_reads_erased_and_addresses_wrap_at_256() {
        let (mut e, mut intc) = (Eeprom::default(), IntFlags::default());
        e.write(COMMAND_ADDR, cmd(0x105, 0x26, 0x11), &mut intc);
        e.write(CONTROL_ADDR, 2, &mut intc);
        assert_eq!((e.data[5], e.wrapped_addresses), (0x11, 1));
        e.write(COMMAND_ADDR, cmd(7, 0x26, 0), &mut intc);
        e.write(CONTROL_ADDR, 1, &mut intc);
        assert_eq!(e.read(DATA_ADDR), Some(ERASED as u32));
    }

    #[test]
    fn cart_chip_is_device_0x20_only_when_inserted() {
        let (mut e, mut intc) = (Eeprom::default(), IntFlags::default());
        e.write(COMMAND_ADDR, cmd(0xFB, 0x20, 0x53), &mut intc);
        e.write(CONTROL_ADDR, 2, &mut intc);
        assert_eq!((intc.flags, e.other_device_commands), (0, 1), "no cartridge: no response");
        e.insert_cart(None);
        e.write(CONTROL_ADDR, 2, &mut intc);
        assert_eq!(intc.flags, FLAG_EEPROM_DONE);
        assert_eq!((e.cart.as_ref().unwrap()[0xFB], e.cart_dirty, e.dirty), (0x53, true, false));
        assert_eq!(e.data[0xFB], ERASED, "system EEPROM untouched");
        for unknown in [0x22, 0x24] {
            e.write(COMMAND_ADDR, cmd(0, unknown, 0), &mut intc);
            e.write(CONTROL_ADDR, 1, &mut intc);
        }
        assert_eq!(e.other_device_commands, 3);
    }

    #[test]
    fn load_pads_short_files() {
        let mut e = Eeprom::default();
        e.load(&[1, 2, 3]);
        assert_eq!((&e.data[..4], e.data.len(), e.dirty), (&[1, 2, 3, ERASED][..], SIZE, false));
    }
}
