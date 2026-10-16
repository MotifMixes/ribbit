//! The Leapster 2's internal SPI flash, bit-banged by the BIOS through GPIO.
//!
//! Evidence (VERIFIED-FW, Leapster 2 BIOS 0.6 `152-12206`, driver
//! `BBSPIFlash` at 0x4005_81E0-0x4005_8A80; its debug output on the UART
//! names the GPIO registers):
//!
//! | Item | Evidence |
//! |---|---|
//! | Pins (`GIODataOut` 0x0180_0040): chip select (active low) bit 4, bit 5 held high (HOLD#/WP#, HYPOTHESIS), data to the flash bit 6, clock bit 11; data from the flash: `GIODataIn` 0x0180_004C bit 18 | deselect 0x4005_83D8 (writes 0x30, then 0x830); byte transfer 0x4005_8434 (per bit, MSB first: data with the clock low, sample bit 18, then the clock high) |
//! | Directions (`GIODire` 0x0180_0048): bits 4, 5, 6, 11 out; bit 18 in, pulled up (`GIOPull` 0x0180_0044 bit 18) | set-up 0x4005_82D2-0x4005_832A; the set-up is skipped when `GIODire & 0x40870 == 0x870` |
//! | Chips accepted: 0xAB (release power-down / read ID) + 3 dummy bytes, then 3 ID bytes: 9D 7B 7F (PMC, 16 x 4 KB), 9D 7C 7F (PMC Pm25LV010, 32 x 4 KB, sector erase 0xD7), BF 49 (SST25VF010: 0x50 EWSR, 0xAF AAI program); or, after 10 10 10, 0x9F JEDEC ID: 1C 20 11 (EON, 4 x 32 KB, erase 0xD8), C2 20 11 (Macronix) | 0x4005_84F8-0x4005_8640 |
//! | Fallback without a recognised ID: "Defaulting to 32 sectors of 4K; 256 byte prog page" | string 0x4010_2C20 |
//!
//! Model: a PMC Pm25LV010 (128 KB; which chip real units carry is UNKNOWN),
//! erased (0xFF) at first use. Commands: 0xAB (ID 9D 7C 7F, repeating), 0x9F
//! (same bytes), 0x03 read, 0x0B fast read, 0x05 read status, 0x06/0x04 write
//! enable/disable, 0x01 write status, 0x02 page program (within a 256-byte
//! page, AND into the cells, at chip deselect), 0xD7/0x20 4 KB sector erase,
//! 0xD8 64 KB block erase, 0xC7/0x60 chip erase. Programming and erasing are
//! instant (status WIP never reads 1; real timing UNKNOWN). Command details
//! follow the common SPI-flash conventions of these parts (HYPOTHESIS beyond
//! what the driver shows).

pub const SIZE: usize = 128 * 1024;
const PAGE: usize = 256;
const ID: [u8; 3] = [0x9D, 0x7C, 0x7F];

pub const PIN_CS_N: u32 = 1 << 4;
pub const PIN_MOSI: u32 = 1 << 6;
pub const PIN_CLK: u32 = 1 << 11;
/// The flash's data output, on GIODataIn.
pub const PIN_MISO: u32 = 1 << 18;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SpiFlash {
    pub data: Vec<u8>,
    /// Status register: bit 1 write-enable latch, bits 2-4 block protect.
    status: u8,
    selected: bool,
    clk: bool,
    /// Bits shifted in so far (current byte) and their count.
    shift_in: u8,
    bits_in: u8,
    /// Bytes of the current command received so far (opcode first).
    bytes: u32,
    opcode: u8,
    addr: u32,
    /// Byte being shifted out and the bits of it still to come.
    shift_out: u8,
    bits_out: u8,
    miso: bool,
    /// Page program: the bytes received, applied at deselect.
    page: Vec<(u32, u8)>,
    /// Set when the contents changed (for saving).
    #[serde(skip)]
    pub dirty: bool,
}

impl Default for SpiFlash {
    fn default() -> Self {
        Self {
            data: vec![0xFF; SIZE],
            status: 0,
            selected: false,
            clk: false,
            shift_in: 0,
            bits_in: 0,
            bytes: 0,
            opcode: 0,
            addr: 0,
            shift_out: 0xFF,
            bits_out: 0,
            miso: true,
            page: Vec::new(),
            dirty: false,
        }
    }
}

impl SpiFlash {
    /// Loads saved contents (shorter data is padded with erased bytes).
    pub fn load(&mut self, bytes: &[u8]) {
        let n = bytes.len().min(SIZE);
        self.data[..n].copy_from_slice(&bytes[..n]);
        self.data[n..].fill(0xFF);
    }

    /// The data-out line (GIODataIn bit 18; pulled up while deselected).
    pub fn miso(&self) -> bool {
        !self.selected || self.miso
    }

    /// The GPIO output pins changed (`GIODataOut`).
    pub fn pins(&mut self, out: u32) {
        let (selected, clk, mosi) = (out & PIN_CS_N == 0, out & PIN_CLK != 0, out & PIN_MOSI != 0);
        if selected != self.selected {
            if self.selected {
                self.end_command();
            }
            self.selected = selected;
            self.shift_in = 0;
            self.bits_in = 0;
            self.bytes = 0;
            self.bits_out = 0;
            self.miso = true;
            self.clk = clk;
            return;
        }
        if !self.selected || clk == self.clk {
            self.clk = clk;
            return;
        }
        self.clk = clk;
        if clk {
            // Rising edge: sample the input.
            self.shift_in = self.shift_in << 1 | mosi as u8;
            self.bits_in += 1;
            if self.bits_in == 8 {
                let b = self.shift_in;
                self.bits_in = 0;
                self.shift_in = 0;
                self.byte(b);
            }
        } else if self.bits_out > 0 {
            // Falling edge: the next output bit.
            self.bits_out -= 1;
            self.miso = self.shift_out >> self.bits_out & 1 != 0;
        }
    }

    /// A whole byte arrived; queue the byte to send during the next one.
    fn byte(&mut self, b: u8) {
        self.bytes += 1;
        let n = self.bytes;
        if n == 1 {
            self.opcode = b;
            self.addr = 0;
            match b {
                0x06 => self.status |= 2,
                0x04 => self.status &= !2,
                _ => {}
            }
        } else if matches!(self.opcode, 0x03 | 0x0B | 0x02 | 0xD7 | 0x20 | 0xD8) && n <= 4 {
            self.addr = (self.addr << 8 | b as u32) % SIZE as u32;
        } else if self.opcode == 0x02 && n > 4 {
            let a = (self.addr as usize & !(PAGE - 1)) | ((self.addr as usize + self.page.len()) % PAGE);
            self.page.push((a as u32, b));
        } else if self.opcode == 0x01 && n == 2 {
            self.status = (self.status & 3) | (b & 0x9C);
        }
        let out = match self.opcode {
            0xAB | 0x9F => {
                // ID bytes after the dummies (0xAB: 3; 0x9F: none).
                let skip = if self.opcode == 0xAB { 4 } else { 1 };
                (n >= skip).then(|| ID[((n - skip) % 3) as usize])
            }
            0x05 => Some(self.status),
            0x03 | 0x0B => {
                let first = if self.opcode == 0x03 { 4 } else { 5 };
                (n >= first).then(|| {
                    let a = (self.addr as usize + (n - first) as usize) % SIZE;
                    self.data[a]
                })
            }
            _ => None,
        };
        self.shift_out = out.unwrap_or(0xFF);
        self.bits_out = 8;
    }

    /// Chip deselected: program and erase commands take effect.
    fn end_command(&mut self) {
        let write_enabled = self.status & 2 != 0;
        let mut changed = false;
        match (self.opcode, self.bytes) {
            (0x02, n) if n > 4 && write_enabled => {
                for &(a, b) in &self.page {
                    self.data[a as usize] &= b;
                }
                changed = true;
            }
            (0xD7 | 0x20, 4) if write_enabled => {
                let a = self.addr as usize & !0xFFF;
                self.data[a..a + 0x1000].fill(0xFF);
                changed = true;
            }
            (0xD8, 4) if write_enabled => {
                let a = self.addr as usize & !0xFFFF;
                self.data[a..(a + 0x1_0000).min(SIZE)].fill(0xFF);
                changed = true;
            }
            (0xC7 | 0x60, 1) if write_enabled => {
                self.data.fill(0xFF);
                changed = true;
            }
            _ => {}
        }
        if changed || (self.opcode == 0x01 && self.bytes >= 2) {
            self.status &= !2;
        }
        self.dirty |= changed;
        self.page.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-bangs bytes the way the BIOS driver does; returns the bytes read.
    fn transfer(f: &mut SpiFlash, bytes: &[u8]) -> Vec<u8> {
        f.pins(0x30); // deselected, clock low
        f.pins(0x20); // selected
        let mut got = vec![];
        for &b in bytes {
            let mut r = 0u8;
            for bit in (0..8).rev() {
                let d = if b >> bit & 1 != 0 { PIN_MOSI } else { 0 };
                f.pins(0x20 | d);
                r = r << 1 | f.miso() as u8;
                f.pins(0x20 | d | PIN_CLK);
            }
            got.push(r);
        }
        f.pins(0x30);
        got
    }

    #[test]
    fn id_read_matches_what_the_driver_accepts() {
        let mut f = SpiFlash::default();
        assert_eq!(transfer(&mut f, &[0xAB, 0, 0, 0, 0, 0, 0])[4..], [0x9D, 0x7C, 0x7F]);
    }

    #[test]
    fn program_needs_write_enable_and_reads_back_and_erases() {
        let mut f = SpiFlash::default();
        transfer(&mut f, &[0x02, 0x00, 0x10, 0x00, 0x12, 0x34]);
        assert_eq!(f.data[0x1000], 0xFF, "no write enable: ignored");
        transfer(&mut f, &[0x06]);
        assert_eq!(transfer(&mut f, &[0x05, 0])[1] & 2, 2, "write-enable latch");
        transfer(&mut f, &[0x02, 0x00, 0x10, 0x00, 0x12, 0x34]);
        assert_eq!(transfer(&mut f, &[0x03, 0x00, 0x10, 0x00, 0, 0])[4..], [0x12, 0x34]);
        assert!(f.dirty);
        assert_eq!(transfer(&mut f, &[0x05, 0])[1] & 2, 0, "latch cleared after programming");
        transfer(&mut f, &[0x06]);
        transfer(&mut f, &[0xD7, 0x00, 0x10, 0x80]);
        assert_eq!(f.data[0x1000], 0xFF, "sector erased");
    }
}
