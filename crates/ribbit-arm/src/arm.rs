//! The ARM (32-bit) instruction set, ARMv5TE.

use crate::{Arm, ArmBus, mode, psr, vector};

/// A register as an instruction reads it: r15 is the instruction's address
/// plus 8.
fn reg(a: &Arm, n: u32) -> u32 {
    if n == 15 { a.current.wrapping_add(8) } else { a.r[n as usize] }
}

/// Writes a register; r15 branches (ARM state kept, bits 1-0 cleared).
fn set_reg(a: &mut Arm, n: u32, v: u32) {
    if n == 15 {
        a.r[15] = v & !3;
    } else {
        a.r[n as usize] = v;
    }
}

/// The shifter operand of a data-processing instruction and its carry out.
fn shifter(a: &Arm, w: u32) -> (u32, bool) {
    let c = a.carry();
    if w & (1 << 25) != 0 {
        let rot = ((w >> 8) & 0xF) * 2;
        let v = (w & 0xFF).rotate_right(rot);
        return (v, if rot == 0 { c } else { v & 0x8000_0000 != 0 });
    }
    let rm = w & 0xF;
    let kind = (w >> 5) & 3;
    if w & (1 << 4) == 0 {
        let amount = (w >> 7) & 0x1F;
        let v = reg(a, rm);
        return shift_imm(v, kind, amount, c);
    }
    // Register-specified shift: r15 as an operand reads 12 ahead.
    let v = if rm == 15 { a.current.wrapping_add(12) } else { a.r[rm as usize] };
    let amount = a.r[((w >> 8) & 0xF) as usize] & 0xFF;
    shift_reg(v, kind, amount, c)
}

pub(crate) fn shift_imm(v: u32, kind: u32, amount: u32, c: bool) -> (u32, bool) {
    match (kind, amount) {
        (0, 0) => (v, c),
        (0, n) => (v << n, (v >> (32 - n)) & 1 != 0),
        (1, 0) => (0, v & 0x8000_0000 != 0), // LSR #32
        (1, n) => (v >> n, (v >> (n - 1)) & 1 != 0),
        (2, 0) => (((v as i32) >> 31) as u32, v & 0x8000_0000 != 0), // ASR #32
        (2, n) => (((v as i32) >> n) as u32, (v >> (n - 1)) & 1 != 0),
        (_, 0) => ((v >> 1) | ((c as u32) << 31), v & 1 != 0), // RRX
        (_, n) => (v.rotate_right(n), (v >> (n - 1)) & 1 != 0),
    }
}

pub(crate) fn shift_reg(v: u32, kind: u32, n: u32, c: bool) -> (u32, bool) {
    if n == 0 {
        return (v, c);
    }
    match kind {
        0 => match n {
            1..=31 => (v << n, (v >> (32 - n)) & 1 != 0),
            32 => (0, v & 1 != 0),
            _ => (0, false),
        },
        1 => match n {
            1..=31 => (v >> n, (v >> (n - 1)) & 1 != 0),
            32 => (0, v & 0x8000_0000 != 0),
            _ => (0, false),
        },
        2 => match n {
            1..=31 => (((v as i32) >> n) as u32, (v >> (n - 1)) & 1 != 0),
            _ => (((v as i32) >> 31) as u32, v & 0x8000_0000 != 0),
        },
        _ => {
            let r = n & 31;
            if r == 0 { (v, v & 0x8000_0000 != 0) } else { (v.rotate_right(r), (v >> (r - 1)) & 1 != 0) }
        }
    }
}

/// a + b + carry_in with carry and overflow out.
pub(crate) fn add_with_carry(a: u32, b: u32, cin: bool) -> (u32, bool, bool) {
    let wide = a as u64 + b as u64 + cin as u64;
    let r = wide as u32;
    let v = (!(a ^ b) & (a ^ r)) & 0x8000_0000 != 0;
    (r, wide >> 32 != 0, v)
}

pub fn execute(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let cond = w >> 28;
    if cond == 0xF {
        unconditional(a, bus, w);
        return;
    }
    if !a.cond(cond) {
        return;
    }
    match (w >> 25) & 7 {
        0 => {
            if w & 0x0FC0_00F0 == 0x0000_0090 {
                multiply(a, w);
            } else if w & 0x0F80_00F0 == 0x0080_0090 {
                multiply_long(a, w);
            } else if w & 0x0FB0_0FF0 == 0x0100_0090 {
                swap(a, bus, w);
            } else if w & 0x0E00_0090 == 0x0000_0090 {
                halfword_transfer(a, bus, w);
            } else if w & 0x0F90_0090 == 0x0100_0000 || w & 0x0F90_0090 == 0x0100_0010 {
                // MRS, MSR, BX, BLX, CLZ, the saturating adds, BKPT.
                misc(a, bus, w);
            } else if w & 0x0F90_0090 == 0x0100_0080 {
                dsp_multiply(a, w);
            } else {
                data_processing(a, w);
            }
        }
        1 => {
            if w & 0x0FB0_0000 == 0x0320_0000 {
                msr(a, w);
            } else if w & 0x0F90_0000 == 0x0100_0000 {
                a.undefined(w);
            } else {
                data_processing(a, w);
            }
        }
        2 => single_transfer(a, bus, w),
        3 => {
            if w & 0x10 != 0 {
                a.undefined(w);
            } else {
                single_transfer(a, bus, w);
            }
        }
        4 => block_transfer(a, bus, w),
        5 => {
            let offset = (((w & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
            if w & (1 << 24) != 0 {
                a.r[14] = a.current.wrapping_add(4);
            }
            a.r[15] = a.current.wrapping_add(8).wrapping_add(offset);
        }
        6 => coprocessor_transfer(a, w),
        _ => {
            if w & (1 << 24) != 0 {
                let ret = a.current.wrapping_add(4);
                a.exception(vector::SWI, mode::SVC, ret);
            } else if w & 0x10 != 0 {
                register_transfer(a, w);
            } else {
                // CDP: no coprocessor implements it.
                a.undefined(w);
            }
        }
    }
}

/// cond = 0b1111: BLX (immediate), PLD; everything else undefined.
fn unconditional(a: &mut Arm, _bus: &mut impl ArmBus, w: u32) {
    if w & 0x0E00_0000 == 0x0A00_0000 {
        let offset = (((w & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
        let h = (w >> 24) & 1;
        a.r[14] = a.current.wrapping_add(4);
        a.cpsr |= psr::T;
        a.r[15] = a.current.wrapping_add(8).wrapping_add(offset).wrapping_add(h << 1);
    } else if w & 0x0D70_F000 == 0x0550_F000 {
        // PLD: a hint.
    } else if w & 0x0E00_0000 == 0x0C00_0000 || w & 0x0F00_0000 == 0x0E00_0000 {
        // LDC2/STC2/CDP2/MCR2/MRC2: no coprocessor.
        a.undefined(w);
    } else {
        a.undefined(w);
    }
}

fn data_processing(a: &mut Arm, w: u32) {
    let op = (w >> 21) & 0xF;
    let s = w & (1 << 20) != 0;
    let rn = (w >> 16) & 0xF;
    let rd = (w >> 12) & 0xF;
    let (op2, shift_c) = shifter(a, w);
    // With a register-specified shift, r15 as Rn also reads 12 ahead.
    let n = if rn == 15 && w & (1 << 25) == 0 && w & (1 << 4) != 0 { a.current.wrapping_add(12) } else { reg(a, rn) };
    let c = a.carry();
    let (result, carry, overflow, write) = match op {
        0x0 => (n & op2, shift_c, None, true),
        0x1 => (n ^ op2, shift_c, None, true),
        0x2 => {
            let (r, c, v) = add_with_carry(n, !op2, true);
            (r, c, Some(v), true)
        }
        0x3 => {
            let (r, c, v) = add_with_carry(op2, !n, true);
            (r, c, Some(v), true)
        }
        0x4 => {
            let (r, c, v) = add_with_carry(n, op2, false);
            (r, c, Some(v), true)
        }
        0x5 => {
            let (r, c, v) = add_with_carry(n, op2, c);
            (r, c, Some(v), true)
        }
        0x6 => {
            let (r, c, v) = add_with_carry(n, !op2, c);
            (r, c, Some(v), true)
        }
        0x7 => {
            let (r, c, v) = add_with_carry(op2, !n, c);
            (r, c, Some(v), true)
        }
        0x8 => (n & op2, shift_c, None, false),
        0x9 => (n ^ op2, shift_c, None, false),
        0xA => {
            let (r, c, v) = add_with_carry(n, !op2, true);
            (r, c, Some(v), false)
        }
        0xB => {
            let (r, c, v) = add_with_carry(n, op2, false);
            (r, c, Some(v), false)
        }
        0xC => (n | op2, shift_c, None, true),
        0xD => (op2, shift_c, None, true),
        0xE => (n & !op2, shift_c, None, true),
        _ => (!op2, shift_c, None, true),
    };
    if write {
        if rd == 15 && s {
            // MOVS pc, ... : return from an exception.
            let spsr = a.spsr();
            a.set_cpsr(spsr);
            a.r[15] = if a.thumb() { result & !1 } else { result & !3 };
            return;
        }
        set_reg(a, rd, result);
    }
    if s && !(rd == 15 && write) {
        a.set_nz(result);
        a.set_flag(psr::C, carry);
        if let Some(v) = overflow {
            a.set_flag(psr::V, v);
        }
    }
}

fn multiply(a: &mut Arm, w: u32) {
    let (rd, rn, rs, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    let mut r = a.r[rm as usize].wrapping_mul(a.r[rs as usize]);
    if w & (1 << 21) != 0 {
        r = r.wrapping_add(a.r[rn as usize]);
    }
    set_reg(a, rd, r);
    if w & (1 << 20) != 0 {
        a.set_nz(r);
    }
}

fn multiply_long(a: &mut Arm, w: u32) {
    let (hi, lo, rs, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    let signed = w & (1 << 22) != 0;
    let (x, y) = (a.r[rm as usize], a.r[rs as usize]);
    let mut r = if signed { (x as i32 as i64).wrapping_mul(y as i32 as i64) as u64 } else { x as u64 * y as u64 };
    if w & (1 << 21) != 0 {
        r = r.wrapping_add(((a.r[hi as usize] as u64) << 32) | a.r[lo as usize] as u64);
    }
    set_reg(a, lo, r as u32);
    set_reg(a, hi, (r >> 32) as u32);
    if w & (1 << 20) != 0 {
        a.set_flag(psr::N, r >> 63 != 0);
        a.set_flag(psr::Z, r == 0);
    }
}

/// SMLA<x><y>, SMLAW<y>, SMULW<y>, SMLAL<x><y>, SMUL<x><y> (ARMv5TE).
fn dsp_multiply(a: &mut Arm, w: u32) {
    let op = (w >> 21) & 3;
    let (rd, rn, rs, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    let half = |v: u32, top: bool| if top { (v >> 16) as i16 as i32 } else { v as i16 as i32 };
    let (x, y) = (w & (1 << 5) != 0, w & (1 << 6) != 0);
    let (m, s) = (a.r[rm as usize], a.r[rs as usize]);
    match op {
        0 => {
            let p = half(m, x).wrapping_mul(half(s, y));
            let acc = a.r[rn as usize] as i32;
            let (r, v) = p.overflowing_add(acc);
            set_reg(a, rd, r as u32);
            if v {
                a.cpsr |= psr::Q;
            }
        }
        1 => {
            let p = ((m as i32 as i64 * half(s, y) as i64) >> 16) as i32;
            if x {
                set_reg(a, rd, p as u32);
            } else {
                let (r, v) = p.overflowing_add(a.r[rn as usize] as i32);
                set_reg(a, rd, r as u32);
                if v {
                    a.cpsr |= psr::Q;
                }
            }
        }
        2 => {
            let p = (half(m, x) as i64).wrapping_mul(half(s, y) as i64);
            let acc = ((a.r[rd as usize] as u64) << 32 | a.r[rn as usize] as u64) as i64;
            let r = acc.wrapping_add(p) as u64;
            set_reg(a, rn, r as u32);
            set_reg(a, rd, (r >> 32) as u32);
        }
        _ => set_reg(a, rd, half(m, x).wrapping_mul(half(s, y)) as u32),
    }
}

/// MRS, MSR (register), BX, BLX (register), CLZ, BKPT, QADD/QSUB/QDADD/QDSUB.
fn misc(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let _ = bus;
    let op = (w >> 4) & 0xF;
    let op21 = (w >> 21) & 3;
    match (op, op21) {
        (0, 0) | (0, 2) => {
            // MRS
            let v = if w & (1 << 22) != 0 { a.spsr() } else { a.cpsr };
            set_reg(a, (w >> 12) & 0xF, v);
        }
        (0, 1) | (0, 3) => msr(a, w),
        (1, 1) => a.branch_exchange(reg(a, w & 0xF)),
        (1, 3) => {
            let rm = reg(a, w & 0xF);
            set_reg(a, (w >> 12) & 0xF, rm.leading_zeros());
        }
        (3, 1) => {
            let target = reg(a, w & 0xF);
            a.r[14] = a.current.wrapping_add(4);
            a.branch_exchange(target);
        }
        (5, _) => {
            let (rn, rd, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, w & 0xF);
            let (x, mut y) = (a.r[rm as usize] as i32, a.r[rn as usize] as i32);
            let mut q = false;
            if op21 & 2 != 0 {
                let (d, sat) = sat_add(y, y);
                y = d;
                q |= sat;
            }
            let (r, sat) = if op21 & 1 != 0 { sat_sub(x, y) } else { sat_add(x, y) };
            q |= sat;
            set_reg(a, rd, r as u32);
            if q {
                a.cpsr |= psr::Q;
            }
        }
        (7, 1) => {
            // BKPT: a prefetch abort.
            let ret = a.current.wrapping_add(4);
            a.exception(vector::PREFETCH_ABORT, mode::ABT, ret);
        }
        _ => a.undefined(w),
    }
}

fn sat_add(x: i32, y: i32) -> (i32, bool) {
    match x.checked_add(y) {
        Some(r) => (r, false),
        None => (if y > 0 { i32::MAX } else { i32::MIN }, true),
    }
}

fn sat_sub(x: i32, y: i32) -> (i32, bool) {
    match x.checked_sub(y) {
        Some(r) => (r, false),
        None => (if y < 0 { i32::MAX } else { i32::MIN }, true),
    }
}

fn msr(a: &mut Arm, w: u32) {
    let v = if w & (1 << 25) != 0 {
        (w & 0xFF).rotate_right(((w >> 8) & 0xF) * 2)
    } else {
        a.r[(w & 0xF) as usize]
    };
    let fields = (w >> 16) & 0xF;
    let mut mask = 0u32;
    for (bit, m) in [(0, 0x0000_00FF), (1, 0x0000_FF00), (2, 0x00FF_0000), (3, 0xFF00_0000)] {
        if fields & (1 << bit) != 0 {
            mask |= m;
        }
    }
    if w & (1 << 22) != 0 {
        let old = a.spsr();
        a.set_spsr((old & !mask) | (v & mask));
    } else {
        if a.mode() == mode::USR {
            mask &= 0xFF00_0000;
        }
        // The T bit is not writable with MSR.
        mask &= !psr::T;
        let new = (a.cpsr & !mask) | (v & mask);
        a.set_cpsr(new);
    }
}

fn swap(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let (rn, rd, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, w & 0xF);
    let addr = a.r[rn as usize];
    let src = a.r[rm as usize];
    let byte = w & (1 << 22) != 0;
    let old = if byte { a.load8(bus, addr) } else { a.load32(bus, addr) };
    let old = match old {
        Ok(v) => v,
        Err(f) => return a.data_abort(f),
    };
    let stored = if byte { a.store8(bus, addr, src) } else { a.store32(bus, addr, src) };
    if let Err(f) = stored {
        return a.data_abort(f);
    }
    set_reg(a, rd, old);
}

/// LDR/STR/LDRB/STRB (and the T forms, treated as the plain ones).
fn single_transfer(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let (p, u, b, wb, l) =
        (w & (1 << 24) != 0, w & (1 << 23) != 0, w & (1 << 22) != 0, w & (1 << 21) != 0, w & (1 << 20) != 0);
    let (rn, rd) = ((w >> 16) & 0xF, (w >> 12) & 0xF);
    let offset = if w & (1 << 25) == 0 {
        w & 0xFFF
    } else {
        shift_imm(a.r[(w & 0xF) as usize], (w >> 5) & 3, (w >> 7) & 0x1F, a.carry()).0
    };
    let base = reg(a, rn);
    let moved = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
    let addr = if p { moved } else { base };
    if l {
        let v = if b { a.load8(bus, addr) } else { a.load32(bus, addr) };
        let v = match v {
            Ok(v) => v,
            Err(f) => return a.data_abort(f),
        };
        if !p || wb {
            set_reg(a, rn, moved);
        }
        if rd == 15 {
            // ARMv5: loading the pc interworks.
            a.branch_exchange(v);
        } else {
            set_reg(a, rd, v);
        }
    } else {
        // STR of r15 stores the instruction's address plus 12 (ARM9: +8 is
        // IMPLEMENTATION DEFINED; ARM926 stores +12).
        let v = if rd == 15 { a.current.wrapping_add(12) } else { a.r[rd as usize] };
        let r = if b { a.store8(bus, addr, v) } else { a.store32(bus, addr, v) };
        if let Err(f) = r {
            return a.data_abort(f);
        }
        if !p || wb {
            set_reg(a, rn, moved);
        }
    }
}

/// LDRH/STRH/LDRSB/LDRSH/LDRD/STRD.
fn halfword_transfer(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let (p, u, imm, wb, l) =
        (w & (1 << 24) != 0, w & (1 << 23) != 0, w & (1 << 22) != 0, w & (1 << 21) != 0, w & (1 << 20) != 0);
    let (rn, rd) = ((w >> 16) & 0xF, (w >> 12) & 0xF);
    let sh = (w >> 5) & 3;
    let offset = if imm { ((w >> 4) & 0xF0) | (w & 0xF) } else { a.r[(w & 0xF) as usize] };
    let base = reg(a, rn);
    let moved = if u { base.wrapping_add(offset) } else { base.wrapping_sub(offset) };
    let addr = if p { moved } else { base };
    let writeback = !p || wb;
    if !l && sh >= 2 {
        // LDRD (sh 2) / STRD (sh 3).
        if sh == 2 {
            let lo = a.load32(bus, addr);
            let hi = a.load32(bus, addr.wrapping_add(4));
            let (lo, hi) = match (lo, hi) {
                (Ok(lo), Ok(hi)) => (lo, hi),
                (Err(f), _) | (_, Err(f)) => return a.data_abort(f),
            };
            if writeback {
                set_reg(a, rn, moved);
            }
            set_reg(a, rd, lo);
            set_reg(a, rd + 1, hi);
        } else {
            let (lo, hi) = (a.r[rd as usize], a.r[(rd + 1) as usize & 15]);
            if let Err(f) = a.store32(bus, addr, lo).and_then(|_| a.store32(bus, addr.wrapping_add(4), hi)) {
                return a.data_abort(f);
            }
            if writeback {
                set_reg(a, rn, moved);
            }
        }
        return;
    }
    if l {
        let v = match sh {
            1 => a.load16(bus, addr),
            2 => a.load8(bus, addr).map(|v| v as u8 as i8 as i32 as u32),
            _ => a.load16(bus, addr).map(|v| v as u16 as i16 as i32 as u32),
        };
        let v = match v {
            Ok(v) => v,
            Err(f) => return a.data_abort(f),
        };
        if writeback {
            set_reg(a, rn, moved);
        }
        set_reg(a, rd, v);
    } else {
        let v = if rd == 15 { a.current.wrapping_add(12) } else { a.r[rd as usize] };
        if let Err(f) = a.store16(bus, addr, v) {
            return a.data_abort(f);
        }
        if writeback {
            set_reg(a, rn, moved);
        }
    }
}

/// LDM/STM (all addressing modes, the S bit's user-bank and SPSR forms).
fn block_transfer(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let (p, u, s, wb, l) =
        (w & (1 << 24) != 0, w & (1 << 23) != 0, w & (1 << 22) != 0, w & (1 << 21) != 0, w & (1 << 20) != 0);
    let rn = (w >> 16) & 0xF;
    let list = w & 0xFFFF;
    let count = list.count_ones().max(1);
    let base = a.r[rn as usize];
    // Lowest address and the written-back base.
    let (start, new_base) = match (p, u) {
        (false, true) => (base, base.wrapping_add(4 * count)),
        (true, true) => (base.wrapping_add(4), base.wrapping_add(4 * count)),
        (false, false) => (base.wrapping_sub(4 * count).wrapping_add(4), base.wrapping_sub(4 * count)),
        (true, false) => (base.wrapping_sub(4 * count), base.wrapping_sub(4 * count)),
    };
    let user_bank = s && !(l && list & 0x8000 != 0);
    let mut addr = start;
    if l {
        let mut values = [0u32; 16];
        for n in 0..16 {
            if list & (1 << n) != 0 {
                match a.load32(bus, addr) {
                    Ok(v) => values[n] = v,
                    Err(f) => return a.data_abort(f),
                }
                addr = addr.wrapping_add(4);
            }
        }
        if wb {
            a.r[rn as usize] = new_base;
        }
        for n in 0..15 {
            if list & (1 << n) != 0 {
                if user_bank {
                    a.set_user_reg(n, values[n]);
                } else {
                    a.r[n] = values[n];
                }
            }
        }
        if list & 0x8000 != 0 {
            if s {
                let spsr = a.spsr();
                a.set_cpsr(spsr);
                a.r[15] = if a.thumb() { values[15] & !1 } else { values[15] & !3 };
            } else {
                a.branch_exchange(values[15]);
            }
        }
    } else {
        let first = list.trailing_zeros();
        for n in 0..16 {
            if list & (1 << n) != 0 {
                let v = if n == 15 {
                    a.current.wrapping_add(12)
                } else if user_bank {
                    a.user_reg(n as usize)
                } else if n == rn && wb && n != first {
                    // The base, when not first in the list, is stored written back.
                    new_base
                } else {
                    a.r[n as usize]
                };
                if let Err(f) = a.store32(bus, addr, v) {
                    return a.data_abort(f);
                }
                addr = addr.wrapping_add(4);
            }
        }
        if wb {
            a.r[rn as usize] = new_base;
        }
    }
}

/// LDC/STC: no coprocessor besides CP15, which has none of these.
fn coprocessor_transfer(a: &mut Arm, w: u32) {
    a.undefined(w);
}

/// MCR/MRC: CP15 only.
fn register_transfer(a: &mut Arm, w: u32) {
    let cp = (w >> 8) & 0xF;
    let (op1, crn, rd, op2, crm) = ((w >> 21) & 7, (w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 5) & 7, w & 0xF);
    if cp != 15 {
        // CP14 (debug) reads as zero; anything else is undefined.
        if cp == 14 {
            if w & (1 << 20) != 0 && rd != 15 {
                a.r[rd as usize] = 0;
            }
            return;
        }
        return a.undefined(w);
    }
    if w & (1 << 20) != 0 {
        let v = a.cp15.read(crn, op1, crm, op2);
        if rd == 15 {
            // Flags from bits 31-28 (e.g. "test and clean" loops).
            a.cpsr = (a.cpsr & 0x0FFF_FFFF) | (v & 0xF000_0000);
        } else {
            a.r[rd as usize] = v;
        }
    } else {
        let v = reg(a, rd);
        if a.cp15.write(crn, op1, crm, op2, v) {
            a.waiting = true;
        }
    }
}
