//! The Leapster Explorer's boot NOR flash: 512 KB at physical address 0,
//! 8 bits wide ("Probing nor-flash at physical address 0x00000000 (8-bit
//! bankwidth)", lf1000-flash.c). The kernel finds it with jedec_probe: the
//! JEDEC unlock cycles, autoselect, and the manufacturer and device codes
//! looked up in its table of known chips.
//!
//! The chip: the kernel's partitions (NOR_Boot 0x7E000, MfgData0 and
//! MfgData1 4 KB each, 512 KB in all) fit the SST39VF040 / SST39LF040 entry
//! of that table (SST 0xBF, 0xD7: x8, 128 sectors of 4 KB, unlock addresses
//! 0x5555 / 0x2AAA, AMD-style commands), so Ribbit answers as one. Inferred
//! from the layout; the real unit's part is UNKNOWN.

pub const SIZE: usize = 512 << 10;
const SECTOR: usize = 4 << 10;
const MANUFACTURER: u8 = 0xBF;
const DEVICE: u8 = 0xD7;

#[derive(Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
enum Mode {
    #[default]
    Read,
    /// The unlock cycles seen so far (AA at 5555, 55 at 2AAA), and what
    /// they lead to: 0 a command, 0x80 an erase command.
    Unlock1(u8),
    Unlock2(u8),
    /// The next write is a byte to program.
    Program,
    /// 0x80 seen: the erase's own unlock cycles follow.
    EraseSetup,
    Autoselect,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Nor {
    pub data: Vec<u8>,
    mode: Mode,
    /// Programmed or erased since loaded (to be saved).
    #[serde(skip)]
    pub dirty: bool,
}

impl Nor {
    pub fn new(data: Vec<u8>) -> Self {
        let mut data = data;
        data.resize(SIZE, 0xFF);
        Nor { data, mode: Mode::Read, dirty: false }
    }

    pub fn read(&self, addr: u32) -> u8 {
        let a = addr as usize % SIZE;
        match self.mode {
            Mode::Autoselect => match a & 0xFF {
                0 => MANUFACTURER,
                1 => DEVICE,
                _ => 0,
            },
            // Programming and erasing finish at once, so status polls (DQ6
            // toggling) read the same, finished data twice.
            _ => self.data[a],
        }
    }

    pub fn write(&mut self, addr: u32, v: u8) {
        let a = addr as usize % SIZE;
        // The chip decodes A14-A0 for the unlock cycles.
        let (unlock1, unlock2) = (a & 0x7FFF == 0x5555, a & 0x7FFF == 0x2AAA);
        self.mode = match (self.mode, v) {
            (_, 0xF0) => Mode::Read,
            (Mode::Read | Mode::Autoselect, 0xAA) if unlock1 => Mode::Unlock1(0),
            (Mode::EraseSetup, 0xAA) if unlock1 => Mode::Unlock1(0x80),
            (Mode::Unlock1(k), 0x55) if unlock2 => Mode::Unlock2(k),
            (Mode::Unlock2(0), 0x90) if unlock1 => Mode::Autoselect,
            (Mode::Unlock2(0), 0xA0) if unlock1 => Mode::Program,
            (Mode::Unlock2(0), 0x80) if unlock1 => Mode::EraseSetup,
            (Mode::Unlock2(0x80), 0x30) => {
                let s = a / SECTOR * SECTOR;
                self.data[s..s + SECTOR].fill(0xFF);
                self.dirty = true;
                Mode::Read
            }
            (Mode::Unlock2(0x80), 0x10) if unlock1 => {
                self.data.fill(0xFF);
                self.dirty = true;
                Mode::Read
            }
            (Mode::Program, v) => {
                // Programming only clears bits.
                self.data[a] &= v;
                self.dirty = true;
                Mode::Read
            }
            (Mode::Autoselect, _) => Mode::Autoselect,
            _ => Mode::Read,
        };
    }
}

/// The factory record MfgData0/1 hold (layout from LeapFrog's
/// libMfgData.so getters): touch calibration at 0x00 (7 ints, pointercal
/// order), backlight threshold 0x1C, battery ADC constant and slope 0x20,
/// audio gain 0x28, serial number 0x2C (24 chars, 20 hex digits used), SN
/// valid flag 0x44, SIRAS bar code 0x48, locale 0x68 (8 chars), touch
/// pressure settings 0x70 (24 ints, `mfgdata get tsp`), soft board ID 0xD0,
/// accelerometer bias 0xD4, record version 0xE0, backlight v2 0xE4, MAC 0xF8,
/// IPv6 0x10C, SKU 0x120; a checksum over the first 0xFFC bytes at 0xFFC
/// (CMfgData::ComputeCheckSum: sum = (sum ^ byte) + 1).
///
/// A real unit's values are its own factory measurements (UNKNOWN). Ribbit
/// writes: the touch calibration its touch model uses (so the first-time
/// Screen Calibration is skipped, as on a calibrated unit: setcal saves it
/// as /flags/pointercal), backlight 0 and ADC 0 0 (the kernel defaults
/// stay), pressure settings left 0 (an unknown version: the driver's
/// defaults stay), locale en-US, version 1 (GUESS).
pub fn mfg_data(serial: &str) -> Vec<u8> {
    let mut r = vec![0u8; SECTOR];
    let put = |r: &mut Vec<u8>, at: usize, b: &[u8]| r[at..at + b.len()].copy_from_slice(b);
    let cal: Vec<u8> = crate::touch::POINTERCAL.iter().flat_map(|v| v.to_le_bytes()).collect();
    put(&mut r, 0x00, &cal);
    put(&mut r, 0x2C, serial.as_bytes());
    put(&mut r, 0x44, &1i32.to_le_bytes());
    put(&mut r, 0x68, b"en-US");
    put(&mut r, 0xE0, &1i32.to_le_bytes());
    let sum = r[..0xFFC].iter().fold(0u32, |s, &b| (s ^ b as u32).wrapping_add(1));
    put(&mut r, 0xFFC, &sum.to_le_bytes());
    r
}

/// The NOR as Ribbit makes it: no boot code (Ribbit boots the kernel
/// itself), MfgData0 and MfgData1 the same record.
pub fn image(serial: &str) -> Nor {
    let mut data = vec![0xFF; SIZE];
    let rec = mfg_data(serial);
    data[0x7E000..0x7F000].copy_from_slice(&rec);
    data[0x7F000..0x80000].copy_from_slice(&rec);
    Nor::new(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_program_erase() {
        let mut n = image("00000000000000000001");
        for (a, v) in [(0x5555, 0xAA), (0x2AAA, 0x55), (0x5555, 0x90)] {
            n.write(a, v);
        }
        assert_eq!([n.read(0), n.read(1)], [0xBF, 0xD7]);
        n.write(0, 0xF0);
        for (a, v) in [(0x5555, 0xAA), (0x2AAA, 0x55), (0x5555, 0xA0), (0x1234, 0x5A)] {
            n.write(a, v);
        }
        assert_eq!(n.read(0x1234), 0x5A);
        for (a, v) in [(0x5555, 0xAA), (0x2AAA, 0x55), (0x5555, 0x80), (0x5555, 0xAA), (0x2AAA, 0x55), (0x1000, 0x30)] {
            n.write(a, v);
        }
        assert_eq!(n.read(0x1234), 0xFF);
        assert_eq!(n.read(0x7E000 + 0x68), b'e');
    }
}
