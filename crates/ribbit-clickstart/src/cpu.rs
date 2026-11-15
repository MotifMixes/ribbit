//! Sunplus unSP 1.0, the 16-bit CPU in the SPG2xx chips. Eight registers:
//! SP, R1-R4, BP, SR and PC. SR holds the flags (N Z S C, bits 6-9), the
//! data segment DS (bits 10-15, the top six address bits for `D:[r]`
//! accesses) and the code segment CS (bits 0-5, the top bits of the
//! 22-bit program counter). Memory is words only.
//!
//! Instruction semantics follow the unSP manual's instruction set as MAME's
//! unsp core models it (used for facts, written anew here), including its
//! choices where hardware behaviour is unknown: an FIQ that is pending but
//! masked blocks IRQs, and the 0xExxx group does nothing on 1.0.

use serde::{Deserialize, Serialize};

pub const SP: usize = 0;
pub const R1: usize = 1;
pub const R3: usize = 3;
pub const R4: usize = 4;
pub const BP: usize = 5;
pub const SR: usize = 6;
pub const PC: usize = 7;

const FLAG_C: u16 = 1 << 6;
const FLAG_S: u16 = 1 << 7;
const FLAG_Z: u16 = 1 << 8;
const FLAG_N: u16 = 1 << 9;

/// The interrupt vectors: reset at +7, FIQ at +6, IRQ n at +8 + n.
const VECTORS: u32 = 0xfff0;
/// RETI, after which pending interrupts are not looked at until the next
/// instruction.
const RETI: u16 = 0x9a98;

/// What the CPU reaches memory through. Addresses are 22-bit word
/// addresses.
pub trait Bus {
    fn read(&mut self, addr: u32) -> u16;
    fn write(&mut self, addr: u32, value: u16);
    /// The interrupt lines: bit 0 the FIQ, bit 1 + n IRQ n.
    fn lines(&self) -> u16;
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Cpu {
    pub r: [u16; 8],
    pub irq_enable: bool,
    pub fiq_enable: bool,
    pub fir_move: bool,
    /// Shift buffer: the bits shifted out by the last shift or rotate.
    pub sb: u16,
    /// In an FIQ / IRQ handler.
    pub in_fiq: bool,
    pub in_irq: bool,
    /// Nested-interrupt mode and the current priority (FR register).
    pub ine: bool,
    pub pri: u16,
    pub aq: bool,
    pub bnk: bool,
    pub fra: bool,
    /// Opcodes met that are not implemented (the first few, for logs).
    #[serde(skip)]
    pub unknown: Vec<(u32, u16)>,
}

impl Cpu {
    pub fn new() -> Self {
        Cpu {
            r: [0; 8],
            irq_enable: false,
            fiq_enable: false,
            fir_move: true,
            sb: 0,
            in_fiq: false,
            in_irq: false,
            ine: false,
            pri: 8,
            aq: false,
            bnk: false,
            fra: false,
            unknown: Vec::new(),
        }
    }

    pub fn reset(&mut self, bus: &mut impl Bus) {
        *self = Cpu::new();
        self.r[PC] = bus.read(VECTORS + 7);
    }

    /// The 22-bit program counter.
    #[inline]
    pub fn lpc(&self) -> u32 {
        ((self.r[SR] as u32 & 0x3f) << 16) | self.r[PC] as u32
    }

    #[inline]
    fn set_lpc(&mut self, lpc: u32) {
        self.r[PC] = lpc as u16;
        self.r[SR] = (self.r[SR] & 0xffc0) | ((lpc >> 16) & 0x3f) as u16;
    }

    #[inline]
    fn add_lpc(&mut self, offset: u32) {
        self.set_lpc(self.lpc().wrapping_add(offset));
    }

    pub fn ds(&self) -> u16 {
        self.r[SR] >> 10
    }

    pub fn set_ds(&mut self, ds: u16) {
        self.r[SR] = (self.r[SR] & 0x03ff) | ((ds & 0x3f) << 10);
    }

    fn fr(&self) -> u16 {
        (self.aq as u16) << 14
            | (self.bnk as u16) << 13
            | (self.fra as u16) << 12
            | (self.fir_move as u16) << 11
            | (self.sb & 0xf) << 7
            | (self.fiq_enable as u16) << 6
            | (self.irq_enable as u16) << 5
            | (self.ine as u16) << 4
            | self.pri & 0xf
    }

    fn set_fr(&mut self, fr: u16) {
        self.aq = fr & 1 << 14 != 0;
        self.bnk = fr & 1 << 13 != 0;
        self.fra = fr & 1 << 12 != 0;
        self.fir_move = fr & 1 << 11 != 0;
        self.sb = (fr >> 7) & 0xf;
        self.fiq_enable = fr & 1 << 6 != 0;
        self.irq_enable = fr & 1 << 5 != 0;
        self.ine = fr & 1 << 4 != 0;
        self.pri = fr & 0xf;
    }

    #[inline]
    fn push(&mut self, bus: &mut impl Bus, value: u16, sp: usize) {
        bus.write(self.r[sp] as u32, value);
        self.r[sp] = self.r[sp].wrapping_sub(1);
    }

    #[inline]
    fn pop(&mut self, bus: &mut impl Bus, sp: usize) -> u16 {
        self.r[sp] = self.r[sp].wrapping_add(1);
        bus.read(self.r[sp] as u32)
    }

    fn flags_nz(&mut self, value: u32) {
        let mut sr = self.r[SR] & !(FLAG_N | FLAG_Z);
        if value & 0x8000 != 0 {
            sr |= FLAG_N;
        }
        if value as u16 == 0 {
            sr |= FLAG_Z;
        }
        self.r[SR] = sr;
    }

    /// N, Z, C and S (signed less-than: the carry out differs from the
    /// operands' sign difference) for value = a + b (+ carry).
    fn flags_nzsc(&mut self, value: u32, a: u16, b: u16) {
        let mut sr = self.r[SR] & !(FLAG_N | FLAG_Z | FLAG_S | FLAG_C);
        if (value >> 16) & 1 != ((a ^ b) as u32 >> 15) & 1 {
            sr |= FLAG_S;
        }
        if value & 0x8000 != 0 {
            sr |= FLAG_N;
        }
        if value as u16 == 0 {
            sr |= FLAG_Z;
        }
        if value & 0x1_0000 != 0 {
            sr |= FLAG_C;
        }
        self.r[SR] = sr;
    }

    /// `D:[r]`: the register with DS as the top address bits.
    #[inline]
    fn ds_addr(&self, reg: usize) -> u32 {
        ((self.r[SR] as u32) << 6 & 0x3f_0000) | self.r[reg] as u32
    }

    fn unknown_op(&mut self, at: u32, op: u16) {
        if self.unknown.len() < 32 {
            self.unknown.push((at, op));
        }
    }

    /// Takes a pending interrupt if it can; then runs one instruction.
    /// Returns the cycles it took.
    pub fn step(&mut self, bus: &mut impl Bus) -> u32 {
        let at = self.lpc();
        let op = bus.read(at);
        self.add_lpc(1);
        let cycles = self.execute(bus, at, op);
        if op != RETI {
            self.check_irqs(bus);
        }
        cycles.max(1)
    }

    pub fn check_irqs(&mut self, bus: &mut impl Bus) {
        let lines = bus.lines();
        if lines == 0 {
            return;
        }
        let first = lines.trailing_zeros() as u16;
        if first == 0 {
            if !self.fiq_enable || self.in_fiq {
                return;
            }
            self.in_fiq = true;
            self.push(bus, self.r[PC], SP);
            self.push(bus, self.r[SR], SP);
            self.r[PC] = bus.read(VECTORS + 6);
            self.r[SR] = 0;
        } else {
            let line = first - 1;
            if (!self.ine && self.in_irq) || self.pri <= line || !self.irq_enable {
                return;
            }
            self.in_irq = true;
            self.push(bus, self.r[PC], SP);
            self.push(bus, self.r[SR], SP);
            if self.ine {
                self.push(bus, self.fr(), SP);
                self.pri = line;
            }
            self.r[PC] = bus.read(VECTORS + 8 + line as u32);
            self.r[SR] = 0;
        }
    }

    fn execute(&mut self, bus: &mut impl Bus, at: u32, op: u16) -> u32 {
        let op0 = op >> 12;
        let opa = ((op >> 9) & 7) as usize;
        let op1 = (op >> 6) & 7;
        if op0 == 0xf {
            return self.fxxx(bus, at, op);
        }
        if opa == 7 && op1 < 2 {
            return self.jump(op0, op1, op);
        }
        if op0 == 0xe {
            // MUL / bit / shift ops of later unSP versions; nothing on 1.0.
            return 2;
        }
        self.alu(bus, op)
    }

    fn jump(&mut self, op0: u16, op1: u16, op: u16) -> u32 {
        let sr = self.r[SR];
        let (n, z, s, c) = (sr & FLAG_N != 0, sr & FLAG_Z != 0, sr & FLAG_S != 0, sr & FLAG_C != 0);
        let taken = match op0 {
            0 => !c,       // JB
            1 => c,        // JAE
            2 => !s,       // JGE
            3 => s,        // JL
            4 => !z,       // JNE
            5 => z,        // JE
            6 => !n,       // JPL
            7 => n,        // JMI
            8 => z || !c,  // JBE
            9 => !z && c,  // JA
            10 => z || s,  // JLE
            11 => !z && !s, // JG
            12 => n == s,  // JVC
            13 => n != s,  // JVS
            _ => true,     // JMP
        };
        if !taken {
            return 2;
        }
        let imm = (op & 0x3f) as u32;
        self.add_lpc(if op1 == 0 { imm } else { imm.wrapping_neg() });
        4
    }

    fn alu(&mut self, bus: &mut impl Bus, op: u16) -> u32 {
        let op0 = op >> 12;
        let opa = ((op >> 9) & 7) as usize;
        let op1 = (op >> 6) & 7;
        let opn = (op >> 3) & 7;
        let opb = (op & 7) as usize;
        let store = op0 == 0xd;
        let pc_dest = opa == PC;

        if op1 == 2 && op0 == 0xd {
            // PUSH rA-n+1..rA to [rB].
            let mut reg = opa;
            for _ in 0..opn {
                let v = self.r[reg];
                self.push(bus, v, opb);
                reg = reg.wrapping_sub(1) & 7;
            }
            return 4 + 2 * opn as u32;
        }
        if op1 == 2 && op0 == 9 {
            if op == RETI {
                if self.ine {
                    let fr = self.pop(bus, SP);
                    self.set_fr(fr);
                }
                self.r[SR] = self.pop(bus, SP);
                self.r[PC] = self.pop(bus, SP);
                if self.in_fiq {
                    self.in_fiq = false;
                } else {
                    self.in_irq = false;
                }
                return 8;
            }
            // POP rA+1..rA+n from [rB].
            let mut reg = opa;
            for _ in 0..opn {
                reg = (reg + 1) & 7;
                self.r[reg] = self.pop(bus, opb);
            }
            return 4 + 2 * opn as u32;
        }

        let mut a = self.r[opa];
        let mut b: u16 = 0;
        let mut addr: u32 = 0;
        let mut store_imm = false;
        let cycles;
        match op1 {
            0 => {
                // [BP+imm6]
                cycles = 6;
                addr = self.r[BP].wrapping_add(op & 0x3f) as u32;
                if !store {
                    b = bus.read(addr);
                }
            }
            1 => {
                cycles = 2;
                b = op & 0x3f;
            }
            3 => {
                cycles = if pc_dest { 7 } else { 6 };
                let ds = opn & 4 != 0;
                match opn & 3 {
                    0 => {
                        addr = if ds { self.ds_addr(opb) } else { self.r[opb] as u32 };
                    }
                    1 => {
                        addr = if ds { self.ds_addr(opb) } else { self.r[opb] as u32 };
                        self.r[opb] = self.r[opb].wrapping_sub(1);
                        if ds && self.r[opb] == 0xffff {
                            self.r[SR] = self.r[SR].wrapping_sub(0x0400);
                        }
                    }
                    2 => {
                        addr = if ds { self.ds_addr(opb) } else { self.r[opb] as u32 };
                        self.r[opb] = self.r[opb].wrapping_add(1);
                        if ds && self.r[opb] == 0 {
                            self.r[SR] = self.r[SR].wrapping_add(0x0400);
                        }
                    }
                    _ => {
                        self.r[opb] = self.r[opb].wrapping_add(1);
                        if ds && self.r[opb] == 0 {
                            self.r[SR] = self.r[SR].wrapping_add(0x0400);
                        }
                        addr = if ds { self.ds_addr(opb) } else { self.r[opb] as u32 };
                    }
                }
                if !store {
                    b = bus.read(addr);
                }
            }
            4 => match opn {
                0 => {
                    cycles = if pc_dest { 5 } else { 3 };
                    b = self.r[opb];
                }
                1 => {
                    // rA = rB op imm16
                    cycles = if pc_dest { 5 } else { 4 };
                    a = self.r[opb];
                    b = bus.read(self.lpc());
                    self.add_lpc(1);
                }
                2 => {
                    // rA = rB op [imm16]
                    cycles = if pc_dest { 8 } else { 7 };
                    a = self.r[opb];
                    addr = bus.read(self.lpc()) as u32;
                    self.add_lpc(1);
                    if !store {
                        b = bus.read(addr);
                    }
                }
                3 => {
                    // [imm16] = rB op rA
                    cycles = if pc_dest { 8 } else { 7 };
                    b = a;
                    a = self.r[opb];
                    addr = bus.read(self.lpc()) as u32;
                    self.add_lpc(1);
                    store_imm = true;
                }
                _ => {
                    // rB ASR 1-4
                    cycles = if pc_dest { 5 } else { 3 };
                    let mut v = (self.r[opb] as u32) << 4 | self.sb as u32;
                    if v & 0x8_0000 != 0 {
                        v |= 0xf0_0000;
                    }
                    v >>= opn - 3;
                    self.sb = (v & 0xf) as u16;
                    b = (v >> 4) as u16;
                }
            },
            5 => {
                cycles = if pc_dest { 5 } else { 3 };
                if opn & 4 != 0 {
                    // LSR 1-4
                    let v = ((self.r[opb] as u32) << 4 | self.sb as u32) >> (opn - 3);
                    self.sb = (v & 0xf) as u16;
                    b = (v >> 4) as u16;
                } else {
                    // LSL 1-4
                    let v = ((self.sb as u32) << 16 | self.r[opb] as u32) << (opn + 1);
                    self.sb = ((v >> 16) & 0xf) as u16;
                    b = v as u16;
                }
            }
            6 => {
                // ROR / ROL 1-4, through SB.
                cycles = if pc_dest { 5 } else { 3 };
                let sb = self.sb as u32;
                let mut v = ((sb << 16 | self.r[opb] as u32) << 4) | sb;
                if opn & 4 != 0 {
                    v >>= opn - 3;
                    self.sb = (v & 0xf) as u16;
                } else {
                    v <<= opn + 1;
                    self.sb = ((v >> 20) & 0xf) as u16;
                }
                b = (v >> 4) as u16;
            }
            7 => {
                // [imm6]
                cycles = if pc_dest { 6 } else { 5 };
                addr = (op & 0x3f) as u32;
                b = bus.read(addr);
            }
            _ => cycles = 2,
        }

        let flags = !pc_dest;
        let carry = (self.r[SR] & FLAG_C != 0) as u32;
        let result: u32 = match op0 {
            0 => {
                let v = a as u32 + b as u32;
                if flags {
                    self.flags_nzsc(v, a, b);
                }
                v
            }
            1 => {
                let v = a as u32 + b as u32 + carry;
                if flags {
                    self.flags_nzsc(v, a, b);
                }
                v
            }
            2 | 4 => {
                let v = a as u32 + (!b) as u32 + 1;
                if flags {
                    self.flags_nzsc(v, a, !b);
                }
                if op0 == 4 {
                    return cycles;
                }
                v
            }
            3 => {
                let v = a as u32 + (!b) as u32 + carry;
                if flags {
                    self.flags_nzsc(v, a, !b);
                }
                v
            }
            6 => {
                let v = (b as u32).wrapping_neg();
                if flags {
                    self.flags_nz(v);
                }
                v
            }
            8 | 9 | 0xa | 0xb | 0xc => {
                let v = match op0 {
                    8 => a ^ b,
                    9 => b,
                    0xa => a | b,
                    _ => a & b,
                } as u32;
                if flags {
                    self.flags_nz(v);
                }
                if op0 == 0xc {
                    return cycles;
                }
                v
            }
            0xd => {
                bus.write(addr, a);
                return cycles;
            }
            _ => return cycles,
        };
        if store_imm {
            bus.write(addr, result as u16);
        } else {
            self.r[opa] = result as u16;
        }
        cycles
    }

    /// Signed x unsigned (`signed_a`: both signed) 16x16 multiply into R4:R3.
    fn mul(&mut self, a: u16, b: u16, signed_a: bool) {
        let mut v = (a as u32).wrapping_mul(b as u32);
        if b & 0x8000 != 0 {
            v = v.wrapping_sub((a as u32) << 16);
        }
        if signed_a && a & 0x8000 != 0 {
            v = v.wrapping_sub((b as u32) << 16);
        }
        self.r[R4] = (v >> 16) as u16;
        self.r[R3] = v as u16;
    }

    /// MULS: the sum of `size` signed products of the words at [rd] and
    /// [rs] into R4:R3; with FIR_MOV on, the [rd] words move up one (an FIR
    /// filter's delay line).
    fn muls(&mut self, bus: &mut impl Bus, rd: usize, rs: usize, size: u16) -> u32 {
        let (pd, ps) = (self.r[rd] as u32, self.r[rs] as u32);
        let mut sum: i64 = 0;
        let mut values = [0u16; 16];
        for i in 0..size as u32 {
            let x = bus.read(pd + i);
            let y = bus.read(ps + i);
            values[i as usize] = x;
            sum += (x as i16 as i32 * y as i16 as i32) as i64;
        }
        self.sb = 0;
        if self.fir_move {
            for i in (1..size as u32).rev() {
                bus.write(pd + i, values[i as usize - 1]);
            }
        }
        self.r[rd] = self.r[rd].wrapping_add(size);
        self.r[rs] = self.r[rs].wrapping_add(size);
        self.r[R4] = (sum >> 16) as u16;
        self.r[R3] = sum as u16;
        10 + 2 * size as u32
    }

    fn fxxx(&mut self, bus: &mut impl Bus, at: u32, op: u16) -> u32 {
        let opa = ((op >> 9) & 7) as usize;
        let opb = (op & 7) as usize;
        match (op >> 6) & 7 {
            0 => {
                // MUL.us rA, rB
                self.mul(self.r[opa], self.r[opb], false);
                12
            }
            1 if op & 0xf3c0 == 0xf040 => {
                // CALL imm22
                let target = bus.read(self.lpc());
                self.add_lpc(1);
                self.push(bus, self.r[PC], SP);
                self.push(bus, self.r[SR], SP);
                self.r[PC] = target;
                self.r[SR] = (self.r[SR] & 0xffc0) | (op & 0x3f);
                9
            }
            2 if op & 0xffc0 == 0xfe80 => {
                // GOTO imm22
                self.r[PC] = bus.read(self.lpc());
                self.r[SR] = (self.r[SR] & 0xffc0) | (op & 0x3f);
                5
            }
            4 if op & 0xf1f8 == 0xf108 => {
                // MUL.ss rA, rB
                self.mul(self.r[opa], self.r[opb], true);
                12
            }
            5 => {
                match op & 0x1ff {
                    0x140 => (self.irq_enable, self.fiq_enable) = (false, false),
                    0x141 => (self.irq_enable, self.fiq_enable) = (true, false),
                    0x142 => (self.irq_enable, self.fiq_enable) = (false, true),
                    0x143 => (self.irq_enable, self.fiq_enable) = (true, true),
                    0x144 => self.fir_move = true,
                    0x145 => self.fir_move = false,
                    0x148 => self.irq_enable = false,
                    0x149 => self.irq_enable = true,
                    0x14c => self.fiq_enable = false,
                    0x14e => self.fiq_enable = true,
                    0x165 | 0x16d | 0x175 | 0x17d => {} // NOP
                    _ => self.unknown_op(at, op),
                }
                2
            }
            6 => {
                // MULS.ss [rA], [rB], size (1-15, 0 = 16)
                let n = (op >> 3) & 7;
                self.muls(bus, opa, opb, if n == 0 { 16 } else { n })
            }
            7 => self.muls(bus, opa, opb, ((op >> 3) & 7) + 8),
            _ => {
                self.unknown_op(at, op);
                2
            }
        }
    }
}

impl Default for Cpu {
    fn default() -> Self {
        Cpu::new()
    }
}
