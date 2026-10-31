//! I2C0 and the Leapster Explorer's sound codec on it.
//!
//! Controller (mach/i2c.h, drivers/i2c/busses/i2c-lf1000.c): the driver
//! puts the address byte (bit 0 = read) or a data byte in IDSR and asks for
//! a bus condition by writing OP_HOLD to IRQ_PEND. With ST_BUSY in ICSR
//! that is a START (just after ICSR was written) or the next byte; without
//! it, a STOP. Every START and byte ends in an interrupt (PEND, write 1 to
//! clear) with ACK_STATUS in ICSR (1 = no acknowledge); a byte received is
//! in IDSR when the interrupt comes. Done at once here. OP_HOLD reads 0, so
//! the interrupt handler's write back of IRQ_PEND asks for nothing.
//!
//! Devices: `/etc/init.d/alsa` reads register 1 at 0x94 (Cirrus Logic
//! CS43L22, loads snd-soc-didj-cs43l22) or else 0xCC (LFP100, which
//! board_ids.h lists for the LeapPad Explorer boards only). Ribbit answers
//! as a CS43L22: a register file with the chip's address pointer (bit 7 =
//! step after each byte). Chip ID 0xE3 is revision B1 (GUESS; the driver
//! takes any 0xE0-0xE7). Volume and mute in the registers aren't applied to
//! the sound yet.

pub const BASE: u32 = 0xC000_E000;
pub const END: u32 = 0xC000_E104;
pub const IRQ: u32 = 32;

const ICCR: u32 = 0x00;
const ICSR: u32 = 0x04;
const IDSR: u32 = 0x0C;
const IRQ_PEND: u32 = 0x24;

const IRQ_ENB: u32 = 1 << 5;
const TX_RX: u32 = 1 << 6;
const ST_BUSY: u32 = 1 << 5;
const ACK_STATUS: u32 = 1 << 0;
const OP_HOLD: u32 = 1 << 1;
const PEND: u32 = 1 << 0;

const CS43L22: u8 = 0x94;
const CHIP_ID: u8 = 0xE3;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct I2c {
    iccr: u32,
    icsr: u32,
    idsr: u32,
    pend: bool,
    /// Other registers: the last value written.
    regs: std::collections::BTreeMap<u32, u32>,
    /// ICSR was written with ST_BUSY: the next condition is a START.
    start: bool,
    /// The addressed device acknowledged; cleared by a STOP.
    active: bool,
    /// The next byte written is the codec's register address.
    first: bool,
    /// Codec registers 0x00-0x34, and its address pointer.
    codec: Vec<u8>,
    map: u8,
}

impl Default for I2c {
    fn default() -> Self {
        let mut codec = vec![0; 0x35];
        codec[1] = CHIP_ID;
        codec[2] = 0x01; // powered down
        Self {
            iccr: 0,
            icsr: 0,
            idsr: 0,
            pend: false,
            regs: Default::default(),
            start: false,
            active: false,
            first: false,
            codec,
            map: 0,
        }
    }
}

impl I2c {
    pub fn read(&self, off: u32) -> u32 {
        match off {
            ICCR => self.iccr,
            ICSR => self.icsr,
            IDSR => self.idsr,
            IRQ_PEND => self.pend as u32,
            _ => self.regs.get(&off).copied().unwrap_or(0),
        }
    }

    pub fn write(&mut self, off: u32, v: u32) {
        match off {
            ICCR => self.iccr = v,
            ICSR => {
                self.icsr = v & !ACK_STATUS | self.icsr & ACK_STATUS;
                self.start = v & ST_BUSY != 0;
            }
            IDSR => self.idsr = v & 0xFF,
            IRQ_PEND => {
                if v & PEND != 0 {
                    self.pend = false;
                }
                if v & OP_HOLD != 0 {
                    self.condition();
                }
            }
            _ => {
                self.regs.insert(off, v);
            }
        }
    }

    pub fn irq(&self) -> bool {
        self.pend && self.iccr & IRQ_ENB != 0
    }

    /// The codec's register values (index = register).
    pub fn codec(&self) -> &[u8] {
        &self.codec
    }

    fn condition(&mut self) {
        let receive = self.icsr & TX_RX == 0;
        if self.start {
            self.start = false;
            self.active = self.idsr as u8 & 0xFE == CS43L22;
            self.first = true;
            if self.active && receive {
                self.idsr = self.codec_read() as u32;
            }
        } else if self.icsr & ST_BUSY == 0 {
            self.active = false;
            return;
        } else if self.active {
            if receive {
                self.idsr = self.codec_read() as u32;
            } else if self.first {
                self.first = false;
                self.map = self.idsr as u8;
            } else {
                self.codec_write(self.idsr as u8);
            }
        }
        self.icsr = self.icsr & !ACK_STATUS | !self.active as u32;
        self.pend = true;
    }

    fn step(&mut self) -> usize {
        let r = (self.map & 0x7F) as usize;
        if self.map & 0x80 != 0 {
            self.map = 0x80 | (self.map + 1) & 0x7F;
        }
        r
    }

    fn codec_read(&mut self) -> u8 {
        let r = self.step();
        self.codec.get(r).copied().unwrap_or(0)
    }

    fn codec_write(&mut self, v: u8) {
        let r = self.step();
        // Register 1 (chip ID) is read-only.
        if r != 1 {
            if let Some(c) = self.codec.get_mut(r) {
                *c = v;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One message the way i2c-lf1000.c sends it; None if not acknowledged.
    fn xfer(i: &mut I2c, addr: u8, out: &[u8], n: usize) -> Option<Vec<u8>> {
        i.write(ICCR, IRQ_ENB);
        i.write(ICSR, 0x1010);
        i.write(IRQ_PEND, OP_HOLD);
        let rd = n > 0;
        i.write(IDSR, (addr | rd as u8) as u32);
        i.write(ICSR, 0x10B0 | if rd { 0 } else { TX_RX });
        i.write(IRQ_PEND, OP_HOLD);
        assert!(i.irq());
        i.write(IRQ_PEND, PEND);
        let mut ok = i.read(ICSR) & ACK_STATUS == 0;
        let mut got = vec![];
        for &b in out {
            i.write(IDSR, b as u32);
            i.write(IRQ_PEND, OP_HOLD);
            i.write(IRQ_PEND, PEND);
            ok &= i.read(ICSR) & ACK_STATUS == 0;
        }
        for _ in 0..n {
            got.push(i.read(IDSR) as u8);
            i.write(IRQ_PEND, OP_HOLD);
            i.write(IRQ_PEND, PEND);
        }
        i.write(ICSR, 0x1090);
        i.write(IRQ_PEND, OP_HOLD);
        assert!(!i.irq());
        ok.then_some(got)
    }

    #[test]
    fn codec_answers_and_keeps_registers() {
        let mut i = I2c::default();
        assert_eq!(xfer(&mut i, 0xCC, &[0], 0), None);
        xfer(&mut i, 0x94, &[0x01], 0).unwrap();
        assert_eq!(xfer(&mut i, 0x94, &[], 1).unwrap(), [CHIP_ID]);
        xfer(&mut i, 0x94, &[0xA0, 0x18, 0x19], 0).unwrap();
        xfer(&mut i, 0x94, &[0xA0], 0).unwrap();
        assert_eq!(xfer(&mut i, 0x94, &[], 2).unwrap(), [0x18, 0x19]);
    }
}
