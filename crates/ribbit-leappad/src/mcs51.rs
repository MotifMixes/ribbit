//! An MCS-51 (8051/8052) processor: the instruction set, the internal RAM
//! (256 bytes, as on the 8032/8052), the standard special function
//! registers, timers 0-2, the serial port and the two-level interrupt
//! system, all as in Intel's MCS-51 user's manual.
//!
//! The LeapPad's main chip (LeapFrog FS80A363) is an 8051 derivative whose
//! extra hardware is not documented. Everything that is not standard 8052 is
//! left to the [`Bus`]: code fetches, external data (MOVX) and every SFR
//! address this core does not know, so the machine model can log and
//! investigate them instead of guessing.
//!
//! Timing is counted in machine cycles (12 oscillator clocks on a classic
//! 8051). Whether the FS80A363 is a 12-clock core is UNKNOWN.

use serde::{Deserialize, Serialize};

/// What the processor is connected to.
pub trait Bus {
    /// Reads program memory (instruction fetch and MOVC).
    fn code(&mut self, addr: u16) -> u8;
    /// MOVX read. `addr` is DPTR, or P2:Ri for `MOVX A,@Ri`.
    fn xread(&mut self, addr: u16) -> u8;
    /// MOVX write.
    fn xwrite(&mut self, addr: u16, value: u8);
    /// Reads an SFR the core does not implement (0x80-0xFF).
    fn sfr_read(&mut self, addr: u8) -> u8;
    /// Writes an SFR the core does not implement.
    fn sfr_write(&mut self, addr: u8, value: u8);
    /// Reads a port's pins (0-3). The value read is pins AND latch, as on
    /// the quasi-bidirectional ports.
    fn port_in(&mut self, _port: u8) -> u8 {
        0xFF
    }
    /// A port latch was written (0-3).
    fn port_out(&mut self, _port: u8, _value: u8) {}
    /// A byte was sent on the serial port.
    fn serial_tx(&mut self, _byte: u8) {}
}

// Standard SFR addresses.
pub const P0: u8 = 0x80;
pub const SP: u8 = 0x81;
pub const DPL: u8 = 0x82;
pub const DPH: u8 = 0x83;
pub const PCON: u8 = 0x87;
pub const TCON: u8 = 0x88;
pub const TMOD: u8 = 0x89;
pub const TL0: u8 = 0x8A;
pub const TL1: u8 = 0x8B;
pub const TH0: u8 = 0x8C;
pub const TH1: u8 = 0x8D;
pub const P1: u8 = 0x90;
pub const SCON: u8 = 0x98;
pub const SBUF: u8 = 0x99;
pub const P2: u8 = 0xA0;
pub const IE: u8 = 0xA8;
pub const P3: u8 = 0xB0;
pub const IP: u8 = 0xB8;
pub const T2CON: u8 = 0xC8;
pub const RCAP2L: u8 = 0xCA;
pub const RCAP2H: u8 = 0xCB;
pub const TL2: u8 = 0xCC;
pub const TH2: u8 = 0xCD;
pub const PSW: u8 = 0xD0;
pub const ACC: u8 = 0xE0;
pub const B: u8 = 0xF0;

/// Whether the core itself holds this SFR (the rest go to the bus).
pub fn is_core_sfr(addr: u8) -> bool {
    matches!(
        addr,
        P0 | SP
            | DPL
            | DPH
            | PCON
            | TCON
            | TMOD
            | TL0
            | TL1
            | TH0
            | TH1
            | P1
            | SCON
            | SBUF
            | P2
            | IE
            | P3
            | IP
            | T2CON
            | RCAP2L
            | RCAP2H
            | TL2
            | TH2
            | PSW
            | ACC
            | B
    )
}

const CY: u8 = 0x80;
const AC: u8 = 0x40;
const OV: u8 = 0x04;

/// Interrupt sources in their fixed polling order: (vector, IE bit).
const SOURCES: [(u16, u8); 6] = [(0x03, 0x01), (0x0B, 0x02), (0x13, 0x04), (0x1B, 0x08), (0x23, 0x10), (0x2B, 0x20)];

/// Machine cycles from a serial write to TI (a stand-in: the real time
/// depends on the baud rate, which only the timing of polling loops sees).
const TX_CYCLES: u32 = 64;

#[derive(Clone, Serialize, Deserialize)]
pub struct Mcs51 {
    pub pc: u16,
    /// Internal RAM, 0x00-0xFF (indirect access reaches all of it; direct
    /// access 0x80-0xFF is the SFRs instead).
    #[serde(with = "serde_bytes_256")]
    pub iram: [u8; 256],
    /// The core's SFRs, indexed by address - 0x80.
    #[serde(with = "serde_bytes_128")]
    pub sfr: [u8; 128],
    /// Interrupts in service: bit 0 low priority, bit 1 high.
    pub in_service: u8,
    /// Machine cycles run since reset.
    pub cycles: u64,
    /// The INT0/INT1 pin levels (true = high, idle). Edge-triggered inputs
    /// set IE0/IE1 on a falling edge.
    pub int_pins: [bool; 2],
    /// Cycles until the byte being sent sets TI.
    tx_pending: u32,
    /// The last instruction was RETI or wrote IE/IP: one more instruction
    /// runs before an interrupt is taken.
    hold_irq: bool,
    /// Set by PCON.IDL: halted until an interrupt.
    pub idle: bool,
    /// Set by PCON.PD: stopped until reset.
    #[serde(default)]
    pub power_down: bool,
}

mod serde_bytes_256 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8; 256], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 256], D::Error> {
        let v: Vec<u8> = Deserialize::deserialize(d)?;
        v.try_into().map_err(|_| serde::de::Error::custom("internal RAM must be 256 bytes"))
    }
}

mod serde_bytes_128 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[u8; 128], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(v)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[u8; 128], D::Error> {
        let v: Vec<u8> = Deserialize::deserialize(d)?;
        v.try_into().map_err(|_| serde::de::Error::custom("SFRs must be 128 bytes"))
    }
}

impl Default for Mcs51 {
    fn default() -> Self {
        Self::new()
    }
}

impl Mcs51 {
    pub fn new() -> Self {
        let mut cpu = Mcs51 {
            pc: 0,
            iram: [0; 256],
            sfr: [0; 128],
            in_service: 0,
            cycles: 0,
            int_pins: [true; 2],
            tx_pending: 0,
            hold_irq: false,
            idle: false,
            power_down: false,
        };
        cpu.reset();
        cpu
    }

    /// The reset state: PC 0, SP 7, ports 0xFF, everything else 0. Internal
    /// RAM is kept, as on the chip.
    pub fn reset(&mut self) {
        self.pc = 0;
        self.sfr = [0; 128];
        self.sfr[(SP - 0x80) as usize] = 7;
        for p in [P0, P1, P2, P3] {
            self.sfr[(p - 0x80) as usize] = 0xFF;
        }
        self.in_service = 0;
        self.tx_pending = 0;
        self.hold_irq = false;
        self.idle = false;
        self.power_down = false;
    }

    #[inline]
    fn s(&self, addr: u8) -> u8 {
        self.sfr[(addr - 0x80) as usize]
    }
    #[inline]
    fn s_mut(&mut self, addr: u8) -> &mut u8 {
        &mut self.sfr[(addr - 0x80) as usize]
    }

    pub fn acc(&self) -> u8 {
        self.s(ACC)
    }
    fn set_acc(&mut self, v: u8) {
        *self.s_mut(ACC) = v;
    }
    pub fn psw(&self) -> u8 {
        let p = (self.acc().count_ones() & 1) as u8;
        (self.s(PSW) & !1) | p
    }
    fn cy(&self) -> bool {
        self.s(PSW) & CY != 0
    }
    fn set_cy(&mut self, c: bool) {
        let p = self.s_mut(PSW);
        *p = if c { *p | CY } else { *p & !CY };
    }
    pub fn dptr(&self) -> u16 {
        u16::from_le_bytes([self.s(DPL), self.s(DPH)])
    }
    fn set_dptr(&mut self, v: u16) {
        *self.s_mut(DPL) = v as u8;
        *self.s_mut(DPH) = (v >> 8) as u8;
    }
    pub fn sp(&self) -> u8 {
        self.s(SP)
    }
    /// Register Rn of the current bank.
    pub fn r(&self, n: u8) -> u8 {
        self.iram[((self.s(PSW) & 0x18) + n) as usize]
    }
    fn set_r(&mut self, n: u8, v: u8) {
        self.iram[((self.s(PSW) & 0x18) + n) as usize] = v;
    }

    /// Reads a core SFR's current value (for debuggers; no side effects).
    pub fn peek_sfr(&self, addr: u8) -> u8 {
        if addr == PSW { self.psw() } else { self.s(addr) }
    }

    fn read_direct(&mut self, bus: &mut impl Bus, addr: u8) -> u8 {
        if addr < 0x80 {
            return self.iram[addr as usize];
        }
        match addr {
            PSW => self.psw(),
            P0 | P1 | P2 | P3 => {
                let port = (addr - 0x80) >> 4;
                bus.port_in(port) & self.s(addr)
            }
            a if is_core_sfr(a) => self.s(a),
            a => bus.sfr_read(a),
        }
    }

    /// The read half of a read-modify-write instruction: ports give their
    /// latch, not their pins.
    fn read_direct_rmw(&mut self, bus: &mut impl Bus, addr: u8) -> u8 {
        if matches!(addr, P0 | P1 | P2 | P3) { self.s(addr) } else { self.read_direct(bus, addr) }
    }

    fn write_direct(&mut self, bus: &mut impl Bus, addr: u8, v: u8) {
        if addr < 0x80 {
            self.iram[addr as usize] = v;
            return;
        }
        if !is_core_sfr(addr) {
            bus.sfr_write(addr, v);
            return;
        }
        *self.s_mut(addr) = v;
        match addr {
            P0 | P1 | P2 | P3 => bus.port_out((addr - 0x80) >> 4, v),
            SBUF => {
                bus.serial_tx(v);
                self.tx_pending = TX_CYCLES;
            }
            IE | IP => self.hold_irq = true,
            PCON if v & 2 != 0 => self.power_down = true,
            PCON if v & 1 != 0 => self.idle = true,
            _ => {}
        }
    }

    fn bit_addr(bit: u8) -> (u8, u8) {
        if bit < 0x80 { (0x20 + (bit >> 3), 1 << (bit & 7)) } else { (bit & 0xF8, 1 << (bit & 7)) }
    }

    fn read_bit(&mut self, bus: &mut impl Bus, bit: u8) -> bool {
        let (a, m) = Self::bit_addr(bit);
        self.read_direct(bus, a) & m != 0
    }

    fn write_bit(&mut self, bus: &mut impl Bus, bit: u8, v: bool) {
        let (a, m) = Self::bit_addr(bit);
        let old = self.read_direct_rmw(bus, a);
        self.write_direct(bus, a, if v { old | m } else { old & !m });
    }

    fn fetch(&mut self, bus: &mut impl Bus) -> u8 {
        let b = bus.code(self.pc);
        self.pc = self.pc.wrapping_add(1);
        b
    }

    fn push(&mut self, v: u8) {
        let sp = self.s(SP).wrapping_add(1);
        *self.s_mut(SP) = sp;
        self.iram[sp as usize] = v;
    }

    fn pop(&mut self) -> u8 {
        let sp = self.s(SP);
        *self.s_mut(SP) = sp.wrapping_sub(1);
        self.iram[sp as usize]
    }

    fn push_pc(&mut self) {
        self.push(self.pc as u8);
        self.push((self.pc >> 8) as u8);
    }

    fn rel(&mut self, off: u8) {
        self.pc = self.pc.wrapping_add(off as i8 as i16 as u16);
    }

    fn add(&mut self, v: u8, carry: bool) {
        let a = self.acc();
        let c = carry as u16;
        let sum = a as u16 + v as u16 + c;
        let half = (a & 0xF) + (v & 0xF) + c as u8;
        let ov = ((a ^ !v) & (a ^ sum as u8) & 0x80) != 0;
        self.set_acc(sum as u8);
        self.set_flags(sum > 0xFF, half > 0xF, ov);
    }

    fn subb(&mut self, v: u8) {
        let a = self.acc();
        let c = self.cy() as i16;
        let diff = a as i16 - v as i16 - c;
        let half = (a & 0xF) as i16 - (v & 0xF) as i16 - c;
        let r = diff as u8;
        let ov = ((a ^ v) & (a ^ r) & 0x80) != 0;
        self.set_acc(r);
        self.set_flags(diff < 0, half < 0, ov);
    }

    fn set_flags(&mut self, cy: bool, ac: bool, ov: bool) {
        let p = self.s_mut(PSW);
        *p &= !(CY | AC | OV);
        if cy {
            *p |= CY;
        }
        if ac {
            *p |= AC;
        }
        if ov {
            *p |= OV;
        }
    }

    /// The operand of the low-nibble addressing modes 4-F (#imm only for
    /// opcodes whose low nibble is 4): direct (5), @R0/@R1 (6, 7), Rn (8-F).
    fn src_operand(&mut self, bus: &mut impl Bus, op: u8) -> u8 {
        match op & 0xF {
            4 => self.fetch(bus),
            5 => {
                let a = self.fetch(bus);
                self.read_direct(bus, a)
            }
            6 | 7 => self.iram[self.r(op & 1) as usize],
            n => self.r(n - 8),
        }
    }

    /// Runs one instruction (or takes an interrupt) and returns the machine
    /// cycles it took.
    pub fn step(&mut self, bus: &mut impl Bus) -> u32 {
        if self.power_down {
            self.cycles += 1;
            return 1;
        }
        if let Some(cycles) = self.check_interrupts(bus) {
            self.tick(bus, cycles);
            return cycles;
        }
        if self.idle {
            self.tick(bus, 1);
            return 1;
        }
        self.hold_irq = false;
        let cycles = self.execute(bus);
        self.tick(bus, cycles);
        cycles
    }

    fn execute(&mut self, bus: &mut impl Bus) -> u32 {
        let op = self.fetch(bus);
        match op {
            0x00 => 1,
            // AJMP / ACALL
            o if o & 0x1F == 0x01 || o & 0x1F == 0x11 => {
                let lo = self.fetch(bus);
                if o & 0x10 != 0 {
                    self.push_pc();
                }
                self.pc = (self.pc & 0xF800) | (((o as u16) >> 5) << 8) | lo as u16;
                2
            }
            0x02 => {
                let hi = self.fetch(bus);
                let lo = self.fetch(bus);
                self.pc = u16::from_be_bytes([hi, lo]);
                2
            }
            0x03 => {
                let a = self.acc();
                self.set_acc(a.rotate_right(1));
                1
            }
            0x04 => {
                let a = self.acc().wrapping_add(1);
                self.set_acc(a);
                1
            }
            0x05 => {
                let d = self.fetch(bus);
                let v = self.read_direct_rmw(bus, d).wrapping_add(1);
                self.write_direct(bus, d, v);
                1
            }
            0x06 | 0x07 => {
                let i = self.r(op & 1) as usize;
                self.iram[i] = self.iram[i].wrapping_add(1);
                1
            }
            0x08..=0x0F => {
                let v = self.r(op - 8).wrapping_add(1);
                self.set_r(op - 8, v);
                1
            }
            0x10 | 0x20 | 0x30 => {
                let bit = self.fetch(bus);
                let off = self.fetch(bus);
                let set = self.read_bit(bus, bit);
                let jump = if op == 0x30 { !set } else { set };
                if op == 0x10 && set {
                    self.write_bit(bus, bit, false);
                }
                if jump {
                    self.rel(off);
                }
                2
            }
            0x12 => {
                let hi = self.fetch(bus);
                let lo = self.fetch(bus);
                self.push_pc();
                self.pc = u16::from_be_bytes([hi, lo]);
                2
            }
            0x13 => {
                let a = self.acc();
                let c = self.cy();
                self.set_cy(a & 1 != 0);
                self.set_acc((a >> 1) | ((c as u8) << 7));
                1
            }
            0x14 => {
                let a = self.acc().wrapping_sub(1);
                self.set_acc(a);
                1
            }
            0x15 => {
                let d = self.fetch(bus);
                let v = self.read_direct_rmw(bus, d).wrapping_sub(1);
                self.write_direct(bus, d, v);
                1
            }
            0x16 | 0x17 => {
                let i = self.r(op & 1) as usize;
                self.iram[i] = self.iram[i].wrapping_sub(1);
                1
            }
            0x18..=0x1F => {
                let v = self.r(op - 0x18).wrapping_sub(1);
                self.set_r(op - 0x18, v);
                1
            }
            0x22 | 0x32 => {
                let hi = self.pop();
                let lo = self.pop();
                self.pc = u16::from_be_bytes([hi, lo]);
                if op == 0x32 {
                    // Clear the highest-priority level in service.
                    if self.in_service & 2 != 0 {
                        self.in_service &= !2;
                    } else {
                        self.in_service &= !1;
                    }
                    self.hold_irq = true;
                }
                2
            }
            0x23 => {
                let a = self.acc();
                self.set_acc(a.rotate_left(1));
                1
            }
            0x24..=0x2F => {
                let v = self.src_operand(bus, op);
                self.add(v, false);
                1
            }
            0x33 => {
                let a = self.acc();
                let c = self.cy();
                self.set_cy(a & 0x80 != 0);
                self.set_acc((a << 1) | c as u8);
                1
            }
            0x34..=0x3F => {
                let v = self.src_operand(bus, op);
                let c = self.cy();
                self.add(v, c);
                1
            }
            0x40 | 0x50 | 0x60 | 0x70 | 0x80 => {
                let off = self.fetch(bus);
                let jump = match op {
                    0x40 => self.cy(),
                    0x50 => !self.cy(),
                    0x60 => self.acc() == 0,
                    0x70 => self.acc() != 0,
                    _ => true,
                };
                if jump {
                    self.rel(off);
                }
                2
            }
            // ORL / ANL / XRL: dir,A (x2), dir,#imm (x3), A,src (x4-xF).
            0x42 | 0x43 | 0x52 | 0x53 | 0x62 | 0x63 => {
                let d = self.fetch(bus);
                let v = if op & 1 == 0 { self.acc() } else { self.fetch(bus) };
                let old = self.read_direct_rmw(bus, d);
                let r = Self::logic(op, old, v);
                self.write_direct(bus, d, r);
                if op & 1 == 0 { 1 } else { 2 }
            }
            0x44..=0x4F | 0x54..=0x5F | 0x64..=0x6F => {
                let v = self.src_operand(bus, op);
                let r = Self::logic(op, self.acc(), v);
                self.set_acc(r);
                1
            }
            0x72 | 0x82 | 0xA0 | 0xB0 => {
                let bit = self.fetch(bus);
                let b = self.read_bit(bus, bit);
                let c = self.cy();
                self.set_cy(match op {
                    0x72 => c | b,
                    0x82 => c & b,
                    0xA0 => c | !b,
                    _ => c & !b,
                });
                2
            }
            0x73 => {
                self.pc = self.dptr().wrapping_add(self.acc() as u16);
                2
            }
            0x74 => {
                let v = self.fetch(bus);
                self.set_acc(v);
                1
            }
            0x75 => {
                let d = self.fetch(bus);
                let v = self.fetch(bus);
                self.write_direct(bus, d, v);
                2
            }
            0x76 | 0x77 => {
                let v = self.fetch(bus);
                let i = self.r(op & 1) as usize;
                self.iram[i] = v;
                1
            }
            0x78..=0x7F => {
                let v = self.fetch(bus);
                self.set_r(op - 0x78, v);
                1
            }
            0x83 => {
                let a = self.pc.wrapping_add(self.acc() as u16);
                let v = bus.code(a);
                self.set_acc(v);
                2
            }
            0x84 => {
                let (a, b) = (self.acc(), self.s(B));
                if b == 0 {
                    self.set_flags(false, false, true);
                } else {
                    self.set_acc(a / b);
                    *self.s_mut(B) = a % b;
                    self.set_flags(false, false, false);
                }
                4
            }
            0x85 => {
                let src = self.fetch(bus);
                let dst = self.fetch(bus);
                let v = self.read_direct(bus, src);
                self.write_direct(bus, dst, v);
                2
            }
            0x86 | 0x87 => {
                let d = self.fetch(bus);
                let v = self.iram[self.r(op & 1) as usize];
                self.write_direct(bus, d, v);
                2
            }
            0x88..=0x8F => {
                let d = self.fetch(bus);
                let v = self.r(op - 0x88);
                self.write_direct(bus, d, v);
                2
            }
            0x90 => {
                let hi = self.fetch(bus);
                let lo = self.fetch(bus);
                self.set_dptr(u16::from_be_bytes([hi, lo]));
                2
            }
            0x92 => {
                let bit = self.fetch(bus);
                let c = self.cy();
                self.write_bit(bus, bit, c);
                2
            }
            0x93 => {
                let a = self.dptr().wrapping_add(self.acc() as u16);
                let v = bus.code(a);
                self.set_acc(v);
                2
            }
            0x94..=0x9F => {
                let v = self.src_operand(bus, op);
                self.subb(v);
                1
            }
            0xA2 => {
                let bit = self.fetch(bus);
                let b = self.read_bit(bus, bit);
                self.set_cy(b);
                1
            }
            0xA3 => {
                let d = self.dptr().wrapping_add(1);
                self.set_dptr(d);
                2
            }
            0xA4 => {
                let r = self.acc() as u16 * self.s(B) as u16;
                self.set_acc(r as u8);
                *self.s_mut(B) = (r >> 8) as u8;
                self.set_flags(false, false, r > 0xFF);
                4
            }
            0xA5 => 1, // reserved; a no-op here (UNKNOWN on the FS80A363)
            0xA6 | 0xA7 => {
                let d = self.fetch(bus);
                let v = self.read_direct(bus, d);
                let i = self.r(op & 1) as usize;
                self.iram[i] = v;
                2
            }
            0xA8..=0xAF => {
                let d = self.fetch(bus);
                let v = self.read_direct(bus, d);
                self.set_r(op - 0xA8, v);
                2
            }
            0xB2 => {
                let bit = self.fetch(bus);
                let (a, m) = Self::bit_addr(bit);
                let old = self.read_direct_rmw(bus, a);
                self.write_direct(bus, a, old ^ m);
                1
            }
            0xB3 => {
                let c = self.cy();
                self.set_cy(!c);
                1
            }
            0xB4..=0xBF => {
                let (lhs, rhs) = match op {
                    0xB4 => (self.acc(), self.fetch(bus)),
                    0xB5 => {
                        let d = self.fetch(bus);
                        (self.acc(), self.read_direct(bus, d))
                    }
                    0xB6 | 0xB7 => (self.iram[self.r(op & 1) as usize], self.fetch(bus)),
                    _ => (self.r(op - 0xB8), self.fetch(bus)),
                };
                let off = self.fetch(bus);
                self.set_cy(lhs < rhs);
                if lhs != rhs {
                    self.rel(off);
                }
                2
            }
            0xC0 => {
                let d = self.fetch(bus);
                let v = self.read_direct(bus, d);
                self.push(v);
                2
            }
            0xC2 | 0xD2 => {
                let bit = self.fetch(bus);
                self.write_bit(bus, bit, op == 0xD2);
                1
            }
            0xC3 | 0xD3 => {
                self.set_cy(op == 0xD3);
                1
            }
            0xC4 => {
                let a = self.acc();
                self.set_acc(a.rotate_left(4));
                1
            }
            0xC5 => {
                let d = self.fetch(bus);
                let v = self.read_direct_rmw(bus, d);
                let a = self.acc();
                self.write_direct(bus, d, a);
                self.set_acc(v);
                1
            }
            0xC6 | 0xC7 => {
                let i = self.r(op & 1) as usize;
                let a = self.acc();
                self.set_acc(self.iram[i]);
                self.iram[i] = a;
                1
            }
            0xC8..=0xCF => {
                let a = self.acc();
                let v = self.r(op - 0xC8);
                self.set_r(op - 0xC8, a);
                self.set_acc(v);
                1
            }
            0xD0 => {
                let d = self.fetch(bus);
                let v = self.pop();
                self.write_direct(bus, d, v);
                2
            }
            0xD4 => {
                let mut a = self.acc() as u16;
                let psw = self.s(PSW);
                let mut cy = psw & CY != 0;
                if a & 0xF > 9 || psw & AC != 0 {
                    a += 6;
                    if a > 0xFF {
                        cy = true;
                    }
                }
                if (a >> 4) & 0xF > 9 || cy || a > 0xFF {
                    a += 0x60;
                    cy = true;
                }
                self.set_acc(a as u8);
                self.set_cy(cy);
                1
            }
            0xD5 => {
                let d = self.fetch(bus);
                let off = self.fetch(bus);
                let v = self.read_direct_rmw(bus, d).wrapping_sub(1);
                self.write_direct(bus, d, v);
                if v != 0 {
                    self.rel(off);
                }
                2
            }
            0xD6 | 0xD7 => {
                let i = self.r(op & 1) as usize;
                let (a, m) = (self.acc(), self.iram[i]);
                self.set_acc((a & 0xF0) | (m & 0x0F));
                self.iram[i] = (m & 0xF0) | (a & 0x0F);
                1
            }
            0xD8..=0xDF => {
                let off = self.fetch(bus);
                let v = self.r(op - 0xD8).wrapping_sub(1);
                self.set_r(op - 0xD8, v);
                if v != 0 {
                    self.rel(off);
                }
                2
            }
            0xE0 => {
                let v = bus.xread(self.dptr());
                self.set_acc(v);
                2
            }
            0xE2 | 0xE3 => {
                let addr = u16::from_be_bytes([self.s(P2), self.r(op & 1)]);
                let v = bus.xread(addr);
                self.set_acc(v);
                2
            }
            0xE4 => {
                self.set_acc(0);
                1
            }
            0xE5 => {
                let d = self.fetch(bus);
                let v = self.read_direct(bus, d);
                self.set_acc(v);
                1
            }
            0xE6 | 0xE7 => {
                let v = self.iram[self.r(op & 1) as usize];
                self.set_acc(v);
                1
            }
            0xE8..=0xEF => {
                let v = self.r(op - 0xE8);
                self.set_acc(v);
                1
            }
            0xF0 => {
                bus.xwrite(self.dptr(), self.acc());
                2
            }
            0xF2 | 0xF3 => {
                let addr = u16::from_be_bytes([self.s(P2), self.r(op & 1)]);
                bus.xwrite(addr, self.acc());
                2
            }
            0xF4 => {
                let a = self.acc();
                self.set_acc(!a);
                1
            }
            0xF5 => {
                let d = self.fetch(bus);
                let a = self.acc();
                self.write_direct(bus, d, a);
                1
            }
            0xF6 | 0xF7 => {
                let i = self.r(op & 1) as usize;
                self.iram[i] = self.acc();
                1
            }
            0xF8..=0xFF => {
                let a = self.acc();
                self.set_r(op - 0xF8, a);
                1
            }
            // Every opcode is covered above; the compiler cannot see that
            // the AJMP/ACALL guard covers the x1 column.
            _ => unreachable!("opcode {op:02X}"),
        }
    }

    fn logic(op: u8, a: u8, b: u8) -> u8 {
        match op & 0xF0 {
            0x40 => a | b,
            0x50 => a & b,
            _ => a ^ b,
        }
    }

    /// Raises (or clears) an interrupt request flag from outside the core,
    /// e.g. a peripheral of the FS80A363 wired to an INT pin.
    pub fn set_int_pin(&mut self, n: usize, level: bool) {
        let tcon = self.s(TCON);
        let (it, ie) = if n == 0 { (0x01, 0x02) } else { (0x04, 0x08) };
        let was = self.int_pins[n];
        self.int_pins[n] = level;
        if tcon & it != 0 {
            // Edge-triggered: a falling edge sets the flag.
            if was && !level {
                *self.s_mut(TCON) |= ie;
            }
        } else if !level {
            *self.s_mut(TCON) |= ie;
        } else {
            *self.s_mut(TCON) &= !ie;
        }
    }

    fn pending(&self) -> [bool; 6] {
        let tcon = self.s(TCON);
        let scon = self.s(SCON);
        let t2 = self.s(T2CON);
        // Level-triggered external inputs request for as long as the pin
        // is low.
        let ie0 = if tcon & 0x01 == 0 { !self.int_pins[0] } else { tcon & 0x02 != 0 };
        let ie1 = if tcon & 0x04 == 0 { !self.int_pins[1] } else { tcon & 0x08 != 0 };
        [ie0, tcon & 0x20 != 0, ie1, tcon & 0x80 != 0, scon & 0x03 != 0, t2 & 0xC0 != 0]
    }

    fn check_interrupts(&mut self, bus: &mut impl Bus) -> Option<u32> {
        let ie = self.s(IE);
        if ie & 0x80 == 0 || self.hold_irq || self.in_service & 2 != 0 {
            return None;
        }
        let ip = self.s(IP);
        let pend = self.pending();
        let mut chosen = None;
        for level in [true, false] {
            if !level && self.in_service & 1 != 0 {
                break;
            }
            for (i, &(_, bit)) in SOURCES.iter().enumerate() {
                if pend[i] && ie & bit != 0 && (ip & bit != 0) == level {
                    chosen = Some((i, level));
                    break;
                }
            }
            if chosen.is_some() {
                break;
            }
        }
        let (i, high) = chosen?;
        let _ = bus;
        self.idle = false;
        // Hardware clears the edge flags of the external inputs and the
        // timer 0/1 overflow flags when it vectors.
        let tcon = self.s_mut(TCON);
        match i {
            0 if *tcon & 0x01 != 0 => *tcon &= !0x02,
            1 => *tcon &= !0x20,
            2 if *tcon & 0x04 != 0 => *tcon &= !0x08,
            3 => *tcon &= !0x80,
            _ => {}
        }
        self.in_service |= if high { 2 } else { 1 };
        self.push_pc();
        self.pc = SOURCES[i].0;
        Some(2)
    }

    /// Advances the timers and the serial port by `cycles` machine cycles.
    fn tick(&mut self, bus: &mut impl Bus, cycles: u32) {
        self.cycles += cycles as u64;
        if self.tx_pending > 0 {
            self.tx_pending = self.tx_pending.saturating_sub(cycles);
            if self.tx_pending == 0 {
                *self.s_mut(SCON) |= 0x02;
            }
        }
        let _ = bus;
        let tcon = self.s(TCON);
        let tmod = self.s(TMOD);
        // Whether timer t counts machine cycles now (TRt set, timer rather
        // than counter, and the GATE input high if gated). Counter mode
        // (pin T0/T1 edges) is not modelled: nothing drives those pins.
        let enabled = |t: usize| {
            let bits = tmod >> (t * 4);
            tcon & (0x10 << (t * 2)) != 0 && bits & 0x04 == 0 && (bits & 0x08 == 0 || self.int_pins[t])
        };
        let (en0, en1) = (enabled(0), enabled(1));
        if tmod & 3 == 3 {
            self.timer0_split(cycles, en0, tcon & 0x40 != 0);
        } else if en0 {
            self.timer01(0, tmod & 3, cycles);
        }
        if en1 && (tmod >> 4) & 3 != 3 {
            self.timer01(1, (tmod >> 4) & 3, cycles);
        }
        let t2 = self.s(T2CON);
        if t2 & 0x04 != 0 && t2 & 0x02 == 0 {
            let v = u16::from_le_bytes([self.s(TL2), self.s(TH2)]);
            let (n, over) = v.overflowing_add(cycles as u16);
            let baud = t2 & 0x30 != 0;
            // Baud-rate mode counts oscillator / 2 rather than machine
            // cycles; that only changes the rate, which nothing here uses.
            let n = if over || (n < v) {
                if !baud {
                    *self.s_mut(T2CON) |= 0x80;
                }
                if t2 & 0x01 == 0 || baud {
                    let reload = u16::from_le_bytes([self.s(RCAP2L), self.s(RCAP2H)]);
                    reload.wrapping_add(n)
                } else {
                    n
                }
            } else {
                n
            };
            *self.s_mut(TL2) = n as u8;
            *self.s_mut(TH2) = (n >> 8) as u8;
        }
    }

    /// Counts `n` falling edges on the T0 (`t` = 0) or T1 (1) pin: the
    /// timer advances if it is running in counter mode (C/T set).
    pub fn count_pin(&mut self, t: usize, n: u32) {
        let tmod = self.s(TMOD);
        let tcon = self.s(TCON);
        let bits = tmod >> (t * 4);
        let running = tcon & (0x10 << (t * 2)) != 0 && (bits & 0x08 == 0 || self.int_pins[t]);
        if bits & 0x04 == 0 || !running || n == 0 {
            return;
        }
        let mode = bits & 3;
        if t == 0 && mode == 3 {
            self.timer0_split(n, true, false);
        } else if !(t == 1 && mode == 3) {
            self.timer01(t, mode, n);
        }
    }

    /// Timer 0 in mode 3: TL0 an 8-bit timer on TR0/TF0, TH0 another on
    /// TR1/TF1.
    fn timer0_split(&mut self, cycles: u32, tl_run: bool, th_run: bool) {
        for _ in 0..cycles {
            if tl_run {
                let v = self.s(TL0).wrapping_add(1);
                *self.s_mut(TL0) = v;
                if v == 0 {
                    *self.s_mut(TCON) |= 0x20;
                }
            }
            if th_run {
                let v = self.s(TH0).wrapping_add(1);
                *self.s_mut(TH0) = v;
                if v == 0 {
                    *self.s_mut(TCON) |= 0x80;
                }
            }
        }
    }

    fn timer01(&mut self, t: usize, mode: u8, cycles: u32) {
        let (tl, th) = if t == 0 { (TL0, TH0) } else { (TL1, TH1) };
        let tf = if t == 0 { 0x20 } else { 0x80 };
        for _ in 0..cycles {
            match mode {
                0 => {
                    let v = ((self.s(th) as u16) << 5) | (self.s(tl) as u16 & 0x1F);
                    let n = (v + 1) & 0x1FFF;
                    *self.s_mut(tl) = (self.s(tl) & 0xE0) | (n & 0x1F) as u8;
                    *self.s_mut(th) = (n >> 5) as u8;
                    if n == 0 {
                        *self.s_mut(TCON) |= tf;
                    }
                }
                1 => {
                    let v = u16::from_le_bytes([self.s(tl), self.s(th)]).wrapping_add(1);
                    *self.s_mut(tl) = v as u8;
                    *self.s_mut(th) = (v >> 8) as u8;
                    if v == 0 {
                        *self.s_mut(TCON) |= tf;
                    }
                }
                _ => {
                    let v = self.s(tl).wrapping_add(1);
                    if v == 0 {
                        *self.s_mut(tl) = self.s(th);
                        *self.s_mut(TCON) |= tf;
                    } else {
                        *self.s_mut(tl) = v;
                    }
                }
            }
        }
    }
}
