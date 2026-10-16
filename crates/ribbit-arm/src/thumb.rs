//! The Thumb (16-bit) instruction set, ARMv5T.

use crate::arm::{add_with_carry, shift_imm, shift_reg};
use crate::{Arm, ArmBus, mode, psr, vector};

/// A register as a Thumb instruction reads it: r15 is the instruction's
/// address plus 4.
fn reg(a: &Arm, n: u32) -> u32 {
    if n == 15 { a.current.wrapping_add(4) } else { a.r[n as usize] }
}

fn set_flags_add(a: &mut Arm, (r, c, v): (u32, bool, bool)) -> u32 {
    a.set_nz(r);
    a.set_flag(psr::C, c);
    a.set_flag(psr::V, v);
    r
}

pub fn execute(a: &mut Arm, bus: &mut impl ArmBus, h: u16) {
    let h = h as u32;
    match h >> 11 {
        // LSL, LSR, ASR Rd, Rm, #imm
        0b00000..=0b00010 => {
            let (rd, rm, imm) = (h & 7, (h >> 3) & 7, (h >> 6) & 0x1F);
            let (r, c) = shift_imm(a.r[rm as usize], h >> 11, imm, a.carry());
            a.r[rd as usize] = r;
            a.set_nz(r);
            a.set_flag(psr::C, c);
        }
        // ADD/SUB Rd, Rn, Rm/#imm3
        0b00011 => {
            let (rd, rn) = (h & 7, (h >> 3) & 7);
            let x = (h >> 6) & 7;
            let operand = if h & (1 << 10) != 0 { x } else { a.r[x as usize] };
            let n = a.r[rn as usize];
            let res = if h & (1 << 9) != 0 { add_with_carry(n, !operand, true) } else { add_with_carry(n, operand, false) };
            a.r[rd as usize] = set_flags_add(a, res);
        }
        // MOV/CMP/ADD/SUB Rd, #imm8
        0b00100..=0b00111 => {
            let (rd, imm) = ((h >> 8) & 7, h & 0xFF);
            let d = a.r[rd as usize];
            match (h >> 11) & 3 {
                0 => {
                    a.r[rd as usize] = imm;
                    a.set_nz(imm);
                }
                1 => {
                    set_flags_add(a, add_with_carry(d, !imm, true));
                }
                2 => a.r[rd as usize] = set_flags_add(a, add_with_carry(d, imm, false)),
                _ => a.r[rd as usize] = set_flags_add(a, add_with_carry(d, !imm, true)),
            }
        }
        0b01000 => {
            if h & (1 << 10) == 0 {
                alu(a, h);
            } else {
                hi_register(a, h);
            }
        }
        // LDR Rd, [PC, #imm8*4]
        0b01001 => {
            let addr = (a.current.wrapping_add(4) & !3).wrapping_add((h & 0xFF) << 2);
            match a.load32(bus, addr) {
                Ok(v) => a.r[((h >> 8) & 7) as usize] = v,
                Err(f) => a.data_abort(f),
            }
        }
        // Load/store, register offset.
        0b01010 | 0b01011 => {
            let (rd, rn, rm) = (h & 7, (h >> 3) & 7, (h >> 6) & 7);
            let addr = a.r[rn as usize].wrapping_add(a.r[rm as usize]);
            let v = a.r[rd as usize];
            let r = match (h >> 9) & 7 {
                0 => a.store32(bus, addr, v).map(|_| None),
                1 => a.store16(bus, addr, v).map(|_| None),
                2 => a.store8(bus, addr, v).map(|_| None),
                3 => a.load8(bus, addr).map(|v| Some(v as u8 as i8 as i32 as u32)),
                4 => a.load32(bus, addr).map(Some),
                5 => a.load16(bus, addr).map(Some),
                6 => a.load8(bus, addr).map(Some),
                _ => a.load16(bus, addr).map(|v| Some(v as u16 as i16 as i32 as u32)),
            };
            finish_load(a, rd, r);
        }
        // LDR/STR/LDRB/STRB Rd, [Rn, #imm5]
        0b01100..=0b01111 => {
            let (rd, rn, imm) = (h & 7, (h >> 3) & 7, (h >> 6) & 0x1F);
            let byte = h & (1 << 12) != 0;
            let addr = a.r[rn as usize].wrapping_add(if byte { imm } else { imm << 2 });
            let v = a.r[rd as usize];
            let r = match (byte, h & (1 << 11) != 0) {
                (false, false) => a.store32(bus, addr, v).map(|_| None),
                (false, true) => a.load32(bus, addr).map(Some),
                (true, false) => a.store8(bus, addr, v).map(|_| None),
                (true, true) => a.load8(bus, addr).map(Some),
            };
            finish_load(a, rd, r);
        }
        // LDRH/STRH Rd, [Rn, #imm5*2]
        0b10000 | 0b10001 => {
            let (rd, rn, imm) = (h & 7, (h >> 3) & 7, (h >> 6) & 0x1F);
            let addr = a.r[rn as usize].wrapping_add(imm << 1);
            let r = if h & (1 << 11) != 0 {
                a.load16(bus, addr).map(Some)
            } else {
                let v = a.r[rd as usize];
                a.store16(bus, addr, v).map(|_| None)
            };
            finish_load(a, rd, r);
        }
        // LDR/STR Rd, [SP, #imm8*4]
        0b10010 | 0b10011 => {
            let rd = (h >> 8) & 7;
            let addr = a.r[13].wrapping_add((h & 0xFF) << 2);
            let r = if h & (1 << 11) != 0 {
                a.load32(bus, addr).map(Some)
            } else {
                let v = a.r[rd as usize];
                a.store32(bus, addr, v).map(|_| None)
            };
            finish_load(a, rd, r);
        }
        // ADD Rd, PC/SP, #imm8*4
        0b10100 | 0b10101 => {
            let rd = (h >> 8) & 7;
            let base = if h & (1 << 11) != 0 { a.r[13] } else { a.current.wrapping_add(4) & !3 };
            a.r[rd as usize] = base.wrapping_add((h & 0xFF) << 2);
        }
        0b10110 | 0b10111 => misc(a, bus, h),
        // STMIA/LDMIA Rn!, {list}
        0b11000 | 0b11001 => {
            let rn = (h >> 8) & 7;
            let list = h & 0xFF;
            let mut addr = a.r[rn as usize];
            let load = h & (1 << 11) != 0;
            if list == 0 {
                // An empty list transfers r15 and moves the base by 0x40
                // (UNPREDICTABLE; ARM7/9 behaviour).
                if load {
                    match a.load32(bus, addr) {
                        Ok(v) => a.r[15] = v & !1,
                        Err(f) => return a.data_abort(f),
                    }
                } else {
                    let v = a.current.wrapping_add(6);
                    if let Err(f) = a.store32(bus, addr, v) {
                        return a.data_abort(f);
                    }
                }
                a.r[rn as usize] = addr.wrapping_add(0x40);
                return;
            }
            let end = addr.wrapping_add(4 * list.count_ones());
            let first = list.trailing_zeros();
            for n in 0..8 {
                if list & (1 << n) == 0 {
                    continue;
                }
                if load {
                    match a.load32(bus, addr) {
                        Ok(v) => a.r[n as usize] = v,
                        Err(f) => return a.data_abort(f),
                    }
                } else {
                    let v = if n == rn && n != first { end } else { a.r[n as usize] };
                    if let Err(f) = a.store32(bus, addr, v) {
                        return a.data_abort(f);
                    }
                }
                addr = addr.wrapping_add(4);
            }
            // LDMIA with the base in the list keeps the loaded value.
            if !(load && list & (1 << rn) != 0) {
                a.r[rn as usize] = end;
            }
        }
        // Conditional branch, SWI.
        0b11010 | 0b11011 => {
            let cond = (h >> 8) & 0xF;
            match cond {
                0xF => {
                    let ret = a.current.wrapping_add(2);
                    a.exception(vector::SWI, mode::SVC, ret);
                }
                0xE => a.undefined(h),
                _ => {
                    if a.cond(cond) {
                        let offset = ((h & 0xFF) as u8 as i8 as i32 as u32) << 1;
                        a.r[15] = a.current.wrapping_add(4).wrapping_add(offset);
                    }
                }
            }
        }
        // B (unconditional)
        0b11100 => {
            let offset = ((h << 21) as i32 >> 20) as u32;
            a.r[15] = a.current.wrapping_add(4).wrapping_add(offset);
        }
        // BLX suffix (ARM target).
        0b11101 => {
            if h & 1 != 0 {
                return a.undefined(h);
            }
            let target = a.r[14].wrapping_add((h & 0x7FF) << 1) & !3;
            a.r[14] = a.current.wrapping_add(2) | 1;
            a.cpsr &= !psr::T;
            a.r[15] = target;
        }
        // BL/BLX prefix: LR = PC + (offset << 12).
        0b11110 => {
            let offset = ((h << 21) as i32 >> 9) as u32;
            a.r[14] = a.current.wrapping_add(4).wrapping_add(offset);
        }
        // BL suffix.
        _ => {
            let target = a.r[14].wrapping_add((h & 0x7FF) << 1);
            a.r[14] = a.current.wrapping_add(2) | 1;
            a.r[15] = target;
        }
    }
}

/// Completes a load (writing Rd) or a store; aborts on a fault.
fn finish_load(a: &mut Arm, rd: u32, r: Result<Option<u32>, crate::Fault>) {
    match r {
        Ok(Some(v)) => a.r[rd as usize] = v,
        Ok(None) => {}
        Err(f) => a.data_abort(f),
    }
}

fn alu(a: &mut Arm, h: u32) {
    let (rd, rm) = ((h & 7) as usize, ((h >> 3) & 7) as usize);
    let (d, m) = (a.r[rd], a.r[rm]);
    let c = a.carry();
    match (h >> 6) & 0xF {
        0x0 => logical(a, rd, d & m),
        0x1 => logical(a, rd, d ^ m),
        0x2..=0x4 | 0x7 => {
            let kind = match (h >> 6) & 0xF {
                0x2 => 0,
                0x3 => 1,
                0x4 => 2,
                _ => 3,
            };
            let (r, carry) = shift_reg(d, kind, m & 0xFF, c);
            a.r[rd] = r;
            a.set_nz(r);
            a.set_flag(psr::C, carry);
        }
        0x5 => a.r[rd] = set_flags_add(a, add_with_carry(d, m, c)),
        0x6 => a.r[rd] = set_flags_add(a, add_with_carry(d, !m, c)),
        0x8 => a.set_nz(d & m),
        0x9 => a.r[rd] = set_flags_add(a, add_with_carry(0, !m, true)),
        0xA => {
            set_flags_add(a, add_with_carry(d, !m, true));
        }
        0xB => {
            set_flags_add(a, add_with_carry(d, m, false));
        }
        0xC => logical(a, rd, d | m),
        0xD => {
            // MUL: C is UNPREDICTABLE in ARMv4 and unchanged in ARMv5.
            let r = d.wrapping_mul(m);
            a.r[rd] = r;
            a.set_nz(r);
        }
        0xE => logical(a, rd, d & !m),
        _ => logical(a, rd, !m),
    }
}

fn logical(a: &mut Arm, rd: usize, r: u32) {
    a.r[rd] = r;
    a.set_nz(r);
}

/// ADD/CMP/MOV with high registers, BX, BLX (register).
fn hi_register(a: &mut Arm, h: u32) {
    let rd = (h & 7) | ((h >> 4) & 8);
    let rm = (h >> 3) & 0xF;
    let m = reg(a, rm);
    match (h >> 8) & 3 {
        0 => {
            let r = reg(a, rd).wrapping_add(m);
            if rd == 15 {
                a.r[15] = r & !1;
            } else {
                a.r[rd as usize] = r;
            }
        }
        1 => {
            let d = reg(a, rd);
            set_flags_add(a, add_with_carry(d, !m, true));
        }
        2 => {
            if rd == 15 {
                a.r[15] = m & !1;
            } else {
                a.r[rd as usize] = m;
            }
        }
        _ => {
            if h & (1 << 7) != 0 {
                a.r[14] = a.current.wrapping_add(2) | 1;
            }
            a.branch_exchange(m);
        }
    }
}

/// ADD SP, PUSH/POP, BKPT.
fn misc(a: &mut Arm, bus: &mut impl ArmBus, h: u32) {
    match (h >> 8) & 0xF {
        0x0 => {
            let imm = (h & 0x7F) << 2;
            a.r[13] = if h & 0x80 != 0 { a.r[13].wrapping_sub(imm) } else { a.r[13].wrapping_add(imm) };
        }
        0x4 | 0x5 => {
            // PUSH {list, LR?}
            let list = h & 0xFF;
            let lr = h & 0x100 != 0;
            let count = list.count_ones() + lr as u32;
            let mut addr = a.r[13].wrapping_sub(4 * count);
            let start = addr;
            for n in 0..8 {
                if list & (1 << n) != 0 {
                    let v = a.r[n];
                    if let Err(f) = a.store32(bus, addr, v) {
                        return a.data_abort(f);
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            if lr {
                let v = a.r[14];
                if let Err(f) = a.store32(bus, addr, v) {
                    return a.data_abort(f);
                }
            }
            a.r[13] = start;
        }
        0xC | 0xD => {
            // POP {list, PC?}
            let list = h & 0xFF;
            let mut addr = a.r[13];
            let mut values = [0u32; 9];
            for n in 0..9 {
                if (n < 8 && list & (1 << n) != 0) || (n == 8 && h & 0x100 != 0) {
                    match a.load32(bus, addr) {
                        Ok(v) => values[n] = v,
                        Err(f) => return a.data_abort(f),
                    }
                    addr = addr.wrapping_add(4);
                }
            }
            for n in 0..8 {
                if list & (1 << n) != 0 {
                    a.r[n] = values[n];
                }
            }
            a.r[13] = addr;
            if h & 0x100 != 0 {
                // ARMv5: POP {pc} interworks.
                a.branch_exchange(values[8]);
            }
        }
        0xE => {
            let ret = a.current.wrapping_add(4);
            a.exception(vector::PREFETCH_ABORT, mode::ABT, ret);
        }
        _ => a.undefined(h),
    }
}
