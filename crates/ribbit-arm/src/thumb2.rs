//! Thumb-2 (ARMv7-A): the 32-bit Thumb instructions, IT blocks, and the 16-bit ones ARMv6/v7
//! added (CBZ/CBNZ, IT, the hints, SXTB/UXTB/SXTH/UXTH, REV/REV16/REVSH). Used only when
//! `Arm::v7` is on (`step_thumb`), for programs built for a Cortex-A9/A35 in Thumb mode: the
//! static libstdc++ of the LeapFrog Android titles (docs/leappad-academy.md). The ARM926
//! machines keep ARMv5T Thumb, where 0xE800-0xFFFF are the two halves of BL/BLX.
//!
//! By the ARMv7-A Architecture Reference Manual (ARM DDI 0406C), chapter A6 (Thumb instruction
//! set encoding) and the instruction pages of A8. ITSTATE is kept in the CPSR (IT[1:0] in bits
//! 26-25, IT[7:2] in bits 15-10), as the architecture does. Not here yet: the parallel
//! add/subtract and dual multiplies (SMUAD...), the Q* saturating additions, SEL, SSAT16/USAT16,
//! MSR/MRS of banked registers, and NEON.

use crate::arm::{add_with_carry, shift_imm, shift_reg};
use crate::{Arm, ArmBus, Fault, psr};

const NZCV: u32 = 0xF000_0000;

fn it_state(a: &Arm) -> u32 {
    ((a.cpsr >> 25) & 3) | ((a.cpsr >> 10) & 0x3F) << 2
}

fn set_it(a: &mut Arm, it: u32) {
    a.cpsr = (a.cpsr & !(3 << 25 | 0x3F << 10)) | (it & 3) << 25 | ((it >> 2) & 0x3F) << 10;
}

/// ITAdvance: the next instruction's place in the block (0 after the last).
fn it_advance(it: u32) -> u32 {
    if it & 7 == 0 { 0 } else { (it & 0xE0) | ((it << 1) & 0x1F) }
}

/// A register as a 32-bit Thumb instruction reads it: r15 is its address plus 4.
fn reg(a: &Arm, n: u32) -> u32 {
    if n == 15 { a.current.wrapping_add(4) } else { a.r[n as usize] }
}

/// PC, word-aligned: the base of literal loads and ADR.
fn literal_base(a: &Arm) -> u32 {
    a.current.wrapping_add(4) & !3
}

/// Writes a register; r15 branches and stays in Thumb state.
fn set_reg(a: &mut Arm, n: u32, v: u32) {
    if n == 15 {
        a.r[15] = v & !1;
    } else {
        a.r[n as usize] = v;
    }
}

fn sign_extend(v: u32, bits: u32) -> u32 {
    let s = 32 - bits;
    (((v << s) as i32) >> s) as u32
}

/// The CMP/CMN/TST forms of 16-bit Thumb: inside an IT block they still set the flags; every
/// other 16-bit data-processing instruction doesn't.
fn is_compare16(h: u32) -> bool {
    h & 0xF800 == 0x2800 || matches!(h & 0xFFC0, 0x4200 | 0x4280 | 0x42C0) || h & 0xFF00 == 0x4500
}

/// Runs the Thumb instruction at `pc` (its first halfword `h` already fetched): `Arm::step`
/// in Thumb state when ARMv7 is on.
pub(crate) fn step_thumb(a: &mut Arm, bus: &mut impl ArmBus, pc: u32, h: u32) {
    let wide = h >> 11 >= 0b11101;
    let h2 = if wide {
        let user = a.mode() == crate::mode::USR;
        match a.cp15.translate(bus, pc.wrapping_add(2), false, user) {
            Ok(pa) => bus.read16(pa) as u32,
            Err(f) => {
                a.prefetch_abort(f);
                return;
            }
        }
    } else {
        0
    };
    a.r[15] = pc.wrapping_add(if wide { 4 } else { 2 });
    let it = it_state(a);
    let in_it = it & 0xF != 0;
    if in_it {
        set_it(a, it_advance(it));
        if !a.cond(it >> 4) {
            return;
        }
    }
    if wide {
        execute32(a, bus, h, h2);
    } else if !execute16(a, h) {
        if in_it && !is_compare16(h) {
            let flags = a.cpsr & NZCV;
            crate::thumb::execute(a, bus, h as u16);
            a.cpsr = (a.cpsr & !NZCV) | flags;
        } else {
            crate::thumb::execute(a, bus, h as u16);
        }
    }
}

/// The 16-bit instructions ARMv6/v7 added; false for the ARMv5T ones (`thumb::execute`).
fn execute16(a: &mut Arm, h: u32) -> bool {
    if h & 0xF500 == 0xB100 {
        // CBZ, CBNZ Rn, label
        let rn = (h & 7) as usize;
        let imm = ((h >> 9) & 1) << 6 | ((h >> 3) & 0x1F) << 1;
        if (a.r[rn] != 0) == (h & 0x800 != 0) {
            a.r[15] = a.current.wrapping_add(4 + imm);
        }
    } else if h & 0xFF00 == 0xBF00 {
        // IT (mask non-zero), else the hints: NOP, YIELD, WFE, WFI, SEV
        if h & 0xF != 0 {
            set_it(a, h & 0xFF);
        }
    } else if h & 0xFF00 == 0xB200 {
        let (rd, m) = ((h & 7) as usize, a.r[((h >> 3) & 7) as usize]);
        a.r[rd] = match (h >> 6) & 3 {
            0 => m as i16 as i32 as u32,
            1 => m as i8 as i32 as u32,
            2 => m & 0xFFFF,
            _ => m & 0xFF,
        };
    } else if h & 0xFF00 == 0xBA00 && (h >> 6) & 3 != 2 {
        let (rd, m) = ((h & 7) as usize, a.r[((h >> 3) & 7) as usize]);
        a.r[rd] = match (h >> 6) & 3 {
            0 => m.swap_bytes(),
            1 => ((m & 0x00FF_00FF) << 8) | ((m >> 8) & 0x00FF_00FF),
            _ => (m as u16).swap_bytes() as i16 as i32 as u32,
        };
    } else if h & 0xFFE8 == 0xB660 {
        // CPS: user mode ignores it
    } else {
        return false;
    }
    true
}

/// ThumbExpandImm_C: a modified immediate and its carry out.
fn expand_imm(imm12: u32, c: bool) -> (u32, bool) {
    if imm12 >> 10 == 0 {
        let b = imm12 & 0xFF;
        let v = match (imm12 >> 8) & 3 {
            0 => b,
            1 => b << 16 | b,
            2 => b << 24 | b << 8,
            _ => b * 0x0101_0101,
        };
        (v, c)
    } else {
        let v = (0x80 | (imm12 & 0x7F)).rotate_right(imm12 >> 7);
        (v, v >> 31 != 0)
    }
}

/// The data-processing operations of the shifted-register and modified-immediate forms
/// (their shared 4-bit op), with operand 2 already shifted.
fn data_processing(a: &mut Arm, op: u32, s: bool, rn: u32, rd: u32, op2: u32, shifter_c: bool) {
    let n = reg(a, rn);
    let c = a.carry();
    // Logical (result, write) or arithmetic (result, carry, overflow, write).
    let compare = rd == 15 && s;
    let logical = |a: &mut Arm, r: u32, write: bool| {
        if write {
            set_reg(a, rd, r);
        }
        if s {
            a.set_nz(r);
            a.set_flag(psr::C, shifter_c);
        }
    };
    let arith = |a: &mut Arm, (r, co, v): (u32, bool, bool), write: bool| {
        if write {
            set_reg(a, rd, r);
        }
        if s {
            a.set_nz(r);
            a.set_flag(psr::C, co);
            a.set_flag(psr::V, v);
        }
    };
    match op {
        0 => logical(a, n & op2, !compare),
        1 => logical(a, n & !op2, true),
        2 => logical(a, if rn == 15 { op2 } else { n | op2 }, true),
        3 => logical(a, if rn == 15 { !op2 } else { n | !op2 }, true),
        4 => logical(a, n ^ op2, !compare),
        8 => arith(a, add_with_carry(n, op2, false), !compare),
        10 => arith(a, add_with_carry(n, op2, c), true),
        11 => arith(a, add_with_carry(n, !op2, c), true),
        13 => arith(a, add_with_carry(n, !op2, true), !compare),
        14 => arith(a, add_with_carry(op2, !n, true), true),
        _ => a.undefined(op),
    }
}

/// Saturates `v` to a signed (`signed`) or unsigned range of `bits`; the bool is saturation.
fn saturate(v: i64, bits: u32, signed: bool) -> (u32, bool) {
    let (lo, hi) = if signed { (-(1i64 << (bits - 1)), (1i64 << (bits - 1)) - 1) } else { (0, (1i64 << bits) - 1) };
    if v < lo {
        (lo as u32, true)
    } else if v > hi {
        (hi as u32, true)
    } else {
        (v as u32, false)
    }
}

fn load(a: &mut Arm, bus: &mut impl ArmBus, addr: u32, size: u32, signed: bool) -> Result<u32, Fault> {
    Ok(match size {
        0 => {
            let v = a.load8(bus, addr)?;
            if signed { v as u8 as i8 as i32 as u32 } else { v }
        }
        1 => {
            let v = if addr & 1 != 0 { a.load8(bus, addr)? | a.load8(bus, addr.wrapping_add(1))? << 8 } else { a.load16(bus, addr)? };
            if signed { v as u16 as i16 as i32 as u32 } else { v }
        }
        _ => {
            if addr & 3 != 0 {
                crate::v7::load32_unaligned(a, bus, addr)?
            } else {
                a.load32(bus, addr)?
            }
        }
    })
}

fn store(a: &mut Arm, bus: &mut impl ArmBus, addr: u32, size: u32, v: u32) -> Result<(), Fault> {
    match size {
        0 => a.store8(bus, addr, v),
        1 if addr & 1 != 0 => a.store8(bus, addr, v).and_then(|_| a.store8(bus, addr.wrapping_add(1), v >> 8)),
        1 => a.store16(bus, addr, v),
        _ if addr & 3 != 0 => crate::v7::store32_unaligned(a, bus, addr, v),
        _ => a.store32(bus, addr, v),
    }
}

/// An ARM coprocessor instruction (VFP, CP15) in its Thumb encoding: the same bits under
/// cond AL, run by the ARM decoder with r15 reading as Thumb's word-aligned PC.
fn coprocessor(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let cur = a.current;
    let next = a.r[15];
    // The ARM decoder reads r15 as current + 8.
    a.current = literal_base(a).wrapping_sub(8);
    crate::arm::execute(a, bus, 0xE000_0000 | (w & 0x0FFF_FFFF));
    a.current = cur;
    a.r[15] = next;
}

/// A 32-bit Thumb instruction, halfwords `h1` and `h2`.
fn execute32(a: &mut Arm, bus: &mut impl ArmBus, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    if h1 & 0xFE40 == 0xE800 {
        // LDM/STM (IA, DB), PUSH.W/POP.W
        let (wb, l, rn) = (h1 & 0x20 != 0, h1 & 0x10 != 0, h1 & 0xF);
        let ia = match (h1 >> 7) & 3 {
            1 => true,
            2 => false,
            _ => return a.undefined(w),
        };
        let list = h2 & if l { 0xDFFF } else { 0x5FFF };
        let count = list.count_ones();
        let base = reg(a, rn);
        let new_base = if ia { base.wrapping_add(4 * count) } else { base.wrapping_sub(4 * count) };
        let mut addr = if ia { base } else { new_base };
        let mut values = [0u32; 16];
        for n in 0..16 {
            if list & (1 << n) == 0 {
                continue;
            }
            if l {
                match a.load32(bus, addr) {
                    Ok(v) => values[n] = v,
                    Err(f) => return a.data_abort(f),
                }
            } else if let Err(f) = a.store32(bus, addr, a.r[n]) {
                return a.data_abort(f);
            }
            addr = addr.wrapping_add(4);
        }
        if wb && !(l && list & (1 << rn) != 0) {
            a.r[rn as usize] = new_base;
        }
        if l {
            for n in 0..15 {
                if list & (1 << n) != 0 {
                    a.r[n] = values[n];
                }
            }
            if list & 0x8000 != 0 {
                a.branch_exchange(values[15]);
            }
        }
    } else if h1 & 0xFE40 == 0xE840 {
        load_store_dual(a, bus, h1, h2);
    } else if h1 & 0xFE00 == 0xEA00 {
        // Data processing (shifted register)
        let (op, s, rn) = ((h1 >> 5) & 0xF, h1 & 0x10 != 0, h1 & 0xF);
        let (rd, rm) = ((h2 >> 8) & 0xF, h2 & 0xF);
        let (kind, imm) = ((h2 >> 4) & 3, ((h2 >> 12) & 7) << 2 | (h2 >> 6) & 3);
        let (v, c) = shift_imm(reg(a, rm), kind, imm, a.carry());
        if op == 6 {
            // PKHBT, PKHTB
            let n = reg(a, rn);
            let r = if h2 & 0x20 != 0 { (n & 0xFFFF_0000) | (v & 0xFFFF) } else { (n & 0xFFFF) | (v & 0xFFFF_0000) };
            set_reg(a, rd, r);
        } else {
            data_processing(a, op, s, rn, rd, v, c);
        }
    } else if h1 & 0xEC00 == 0xEC00 {
        // Coprocessor: VFP (cp10/11) and CP15 in their ARM encoding; NEON isn't here.
        if h1 & 0x1000 != 0 {
            return a.undefined(w);
        }
        coprocessor(a, bus, w);
    } else if h1 & 0xF800 == 0xF000 && h2 & 0x8000 == 0 {
        let imm12 = ((h1 >> 10) & 1) << 11 | ((h2 >> 12) & 7) << 8 | (h2 & 0xFF);
        let (rn, rd) = (h1 & 0xF, (h2 >> 8) & 0xF);
        if h1 & 0x0200 == 0 {
            // Data processing (modified immediate)
            let (v, c) = expand_imm(imm12, a.carry());
            data_processing(a, (h1 >> 5) & 0xF, h1 & 0x10 != 0, rn, rd, v, c);
        } else {
            plain_immediate(a, h1, h2, imm12, rn, rd);
        }
    } else if h1 & 0xF800 == 0xF000 {
        branch_misc(a, h1, h2);
    } else if h1 & 0xFE00 == 0xF800 {
        load_store_single(a, bus, h1, h2);
    } else if h1 & 0xFF00 == 0xFA00 {
        register_ops(a, h1, h2);
    } else if h1 & 0xFF80 == 0xFB00 {
        multiply(a, h1, h2);
    } else if h1 & 0xFF80 == 0xFB80 {
        long_multiply(a, h1, h2);
    } else {
        a.undefined(w);
    }
}

/// LDRD/STRD, the exclusives, TBB/TBH.
fn load_store_dual(a: &mut Arm, bus: &mut impl ArmBus, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    let (p, u, wb, l) = (h1 & 0x100 != 0, h1 & 0x80 != 0, h1 & 0x20 != 0, h1 & 0x10 != 0);
    let rn = h1 & 0xF;
    let (rt, rt2) = ((h2 >> 12) & 0xF, (h2 >> 8) & 0xF);
    if p || wb {
        // LDRD/STRD Rt, Rt2, [Rn, #±imm8*4]{!} / [Rn], #±imm8*4
        let imm = (h2 & 0xFF) << 2;
        let base = if rn == 15 { literal_base(a) } else { a.r[rn as usize] };
        let offset = if u { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
        let addr = if p { offset } else { base };
        if l {
            let r = load(a, bus, addr, 2, false).and_then(|lo| load(a, bus, addr.wrapping_add(4), 2, false).map(|hi| (lo, hi)));
            match r {
                Ok((lo, hi)) => {
                    if wb {
                        a.r[rn as usize] = offset;
                    }
                    a.r[rt as usize] = lo;
                    a.r[rt2 as usize] = hi;
                }
                Err(f) => a.data_abort(f),
            }
        } else {
            let (lo, hi) = (a.r[rt as usize], a.r[rt2 as usize]);
            if let Err(f) = store(a, bus, addr, 2, lo).and_then(|_| store(a, bus, addr.wrapping_add(4), 2, hi)) {
                return a.data_abort(f);
            }
            if wb {
                a.r[rn as usize] = offset;
            }
        }
        return;
    }
    let base = reg(a, rn);
    match ((h1 >> 7) & 3, (h1 >> 4) & 3) {
        // STREX Rd, Rt, [Rn, #imm8*4] / LDREX Rt, [Rn, #imm8*4]
        (0, 0) | (0, 1) => {
            let addr = base.wrapping_add((h2 & 0xFF) << 2);
            exclusive(a, bus, l, addr, 2, rt, 15, rt2);
        }
        (1, 1) if (h2 >> 4) & 0xE == 0 => {
            // TBB [Rn, Rm], TBH [Rn, Rm, LSL #1]
            let m = a.r[(h2 & 0xF) as usize];
            let r = if h2 & 0x10 == 0 { load(a, bus, base.wrapping_add(m), 0, false) } else { load(a, bus, base.wrapping_add(m << 1), 1, false) };
            match r {
                Ok(off) => a.r[15] = a.current.wrapping_add(4).wrapping_add(off << 1),
                Err(f) => a.data_abort(f),
            }
        }
        (1, 0) | (1, 1) => {
            // STREXB/H/D, LDREXB/H/D: op3 0100 byte, 0101 half, 0111 doubleword
            let size = match (h2 >> 4) & 0xF {
                4 => 0,
                5 => 1,
                7 => 3,
                _ => return a.undefined(w),
            };
            exclusive(a, bus, l, base, size, rt, rt2, h2 & 0xF);
        }
        _ => a.undefined(w),
    }
}

/// LDREX*/STREX* at `addr`; size 0 byte, 1 half, 2 word, 3 doubleword (Rt, Rt2). A store
/// writes its status (0 done, 1 failed) to `rd`.
#[allow(clippy::too_many_arguments)]
fn exclusive(a: &mut Arm, bus: &mut impl ArmBus, l: bool, addr: u32, size: u32, rt: u32, rt2: u32, rd: u32) {
    if l {
        let r = match size {
            3 => load(a, bus, addr, 2, false).and_then(|lo| load(a, bus, addr.wrapping_add(4), 2, false).map(|hi| (lo, hi))),
            s => load(a, bus, addr, s, false).map(|v| (v, 0)),
        };
        match r {
            Ok((v, hi)) => {
                a.r[rt as usize] = v;
                if size == 3 {
                    a.r[rt2 as usize] = hi;
                }
                if let Some(v7) = a.v7.as_mut() {
                    v7.monitor = Some(addr);
                }
            }
            Err(f) => a.data_abort(f),
        }
        return;
    }
    let holds = a.v7.as_ref().is_some_and(|v| v.monitor == Some(addr));
    if !holds {
        a.r[rd as usize] = 1;
        return;
    }
    let v = a.r[rt as usize];
    let r = match size {
        3 => {
            let hi = a.r[rt2 as usize];
            store(a, bus, addr, 2, v).and_then(|_| store(a, bus, addr.wrapping_add(4), 2, hi))
        }
        s => store(a, bus, addr, s, v),
    };
    if let Err(f) = r {
        return a.data_abort(f);
    }
    if let Some(v7) = a.v7.as_mut() {
        v7.monitor = None;
    }
    a.r[rd as usize] = 0;
}

/// Data processing (plain binary immediate): ADDW/SUBW/ADR, MOVW/MOVT, SSAT/USAT, SBFX/UBFX,
/// BFI/BFC.
fn plain_immediate(a: &mut Arm, h1: u32, h2: u32, imm12: u32, rn: u32, rd: u32) {
    let w = h1 << 16 | h2;
    let n = if rn == 15 { literal_base(a) } else { a.r[rn as usize] };
    let imm16 = (h1 & 0xF) << 12 | imm12;
    let lsb = ((h2 >> 12) & 7) << 2 | (h2 >> 6) & 3;
    let field = h2 & 0x1F;
    match (h1 >> 4) & 0x1F {
        0x00 => set_reg(a, rd, n.wrapping_add(imm12)),
        0x0A => set_reg(a, rd, n.wrapping_sub(imm12)),
        0x04 => set_reg(a, rd, imm16),
        0x0C => {
            let v = (a.r[rd as usize] & 0xFFFF) | imm16 << 16;
            set_reg(a, rd, v);
        }
        op @ (0x10 | 0x12 | 0x18 | 0x1A) => {
            // SSAT/USAT Rd, #sat, Rn{, LSL/ASR #imm}
            let v = if op & 2 != 0 { (n as i32 >> lsb.max(1).min(31)) as i64 } else { ((n << lsb) as i32) as i64 };
            let v = if op & 2 != 0 && lsb == 0 { n as i32 as i64 } else { v };
            let signed = op < 0x18;
            let (r, sat) = saturate(v, if signed { field + 1 } else { field }, signed);
            set_reg(a, rd, r);
            if sat {
                a.cpsr |= psr::Q;
            }
        }
        0x14 | 0x1C => {
            // SBFX/UBFX Rd, Rn, #lsb, #width
            let width = field + 1;
            let v = (n >> lsb) & if width >= 32 { u32::MAX } else { (1 << width) - 1 };
            let v = if (h1 >> 4) & 0x1F == 0x14 { sign_extend(v, width) } else { v };
            set_reg(a, rd, v);
        }
        0x16 => {
            // BFI Rd, Rn, #lsb, #width (BFC: Rn = 15)
            if field < lsb {
                return a.undefined(w);
            }
            let width = field - lsb + 1;
            let mask = (if width >= 32 { u32::MAX } else { (1 << width) - 1 }) << lsb;
            let src = if rn == 15 { 0 } else { a.r[rn as usize] << lsb };
            let v = (a.r[rd as usize] & !mask) | (src & mask);
            set_reg(a, rd, v);
        }
        _ => a.undefined(w),
    }
}

/// B (T3, T4), BL, BLX, and the misc control space (hints, barriers, MRS).
fn branch_misc(a: &mut Arm, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    let s = (h1 >> 10) & 1;
    let (j1, j2) = ((h2 >> 13) & 1, (h2 >> 11) & 1);
    let (i1, i2) = (!(j1 ^ s) & 1, !(j2 ^ s) & 1);
    let pc = a.current.wrapping_add(4);
    let long = sign_extend(s << 24 | i1 << 23 | i2 << 22 | (h1 & 0x3FF) << 12 | (h2 & 0x7FF) << 1, 25);
    match h2 & 0xD000 {
        0xD000 => {
            a.r[14] = pc | 1;
            a.r[15] = pc.wrapping_add(long);
        }
        0xC000 => {
            // BLX to ARM: the target is word-aligned from Align(PC, 4).
            a.r[14] = pc | 1;
            a.cpsr &= !psr::T;
            a.r[15] = (pc & !3).wrapping_add(long & !3);
        }
        0x9000 => a.r[15] = pc.wrapping_add(long),
        _ => {
            let cond = (h1 >> 6) & 0xF;
            if cond < 0xE {
                let off = sign_extend(s << 20 | j2 << 19 | j1 << 18 | (h1 & 0x3F) << 12 | (h2 & 0x7FF) << 1, 21);
                if a.cond(cond) {
                    a.r[15] = pc.wrapping_add(off);
                }
                return;
            }
            match h1 & 0x7F0 {
                // NOP.W and the other hints
                0x3A0 => {}
                // DMB, DSB, ISB; CLREX clears the monitor
                0x3B0 => {
                    if (h2 >> 4) & 0xF == 2
                        && let Some(v7) = a.v7.as_mut()
                    {
                        v7.monitor = None;
                    }
                }
                // MRS Rd, APSR
                0x3E0 | 0x3F0 => {
                    let rd = (h2 >> 8) & 0xF;
                    set_reg(a, rd, a.cpsr & 0xF80F_0000);
                }
                // MSR APSR_nzcvq, Rn
                0x380 | 0x390 if (h2 >> 10) & 3 == 2 => {
                    let v = a.r[(h1 & 0xF) as usize];
                    a.cpsr = (a.cpsr & !0xF800_0000) | (v & 0xF800_0000);
                }
                _ => a.undefined(w),
            }
        }
    }
}

/// LDR/STR (byte, halfword, word, signed) with an immediate, register or literal address,
/// and PLD/PLI.
fn load_store_single(a: &mut Arm, bus: &mut impl ArmBus, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    let (signed, up12, size, l, rn) = (h1 & 0x100 != 0, h1 & 0x80 != 0, (h1 >> 5) & 3, h1 & 0x10 != 0, h1 & 0xF);
    let rt = (h2 >> 12) & 0xF;
    if size == 3 || (!l && (signed || rn == 15)) {
        return a.undefined(w);
    }
    let (addr, writeback) = if rn == 15 {
        let base = literal_base(a);
        let imm = h2 & 0xFFF;
        (if up12 { base.wrapping_add(imm) } else { base.wrapping_sub(imm) }, None)
    } else if up12 {
        (a.r[rn as usize].wrapping_add(h2 & 0xFFF), None)
    } else if h2 & 0x800 != 0 {
        let base = a.r[rn as usize];
        let imm = h2 & 0xFF;
        let (p, u, wb) = (h2 & 0x400 != 0, h2 & 0x200 != 0, h2 & 0x100 != 0);
        let offset = if u { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
        (if p { offset } else { base }, if wb { Some(offset) } else { None })
    } else if h2 & 0xFC0 == 0 {
        (a.r[rn as usize].wrapping_add(a.r[(h2 & 0xF) as usize] << ((h2 >> 4) & 3)), None)
    } else {
        return a.undefined(w);
    };
    if !l {
        let v = a.r[rt as usize];
        if let Err(f) = store(a, bus, addr, size, v) {
            return a.data_abort(f);
        }
        if let Some(nb) = writeback {
            a.r[rn as usize] = nb;
        }
        return;
    }
    // PLD, PLI: byte and halfword loads to r15.
    if rt == 15 && size < 2 {
        return;
    }
    match load(a, bus, addr, size, signed) {
        Ok(v) => {
            if let Some(nb) = writeback {
                a.r[rn as usize] = nb;
            }
            if rt == 15 {
                a.branch_exchange(v);
            } else {
                a.r[rt as usize] = v;
            }
        }
        Err(f) => a.data_abort(f),
    }
}

/// Data processing (register): shifts by register, the extends, REV/RBIT/CLZ.
fn register_ops(a: &mut Arm, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    let (op1, op2, rn) = ((h1 >> 4) & 0xF, (h2 >> 4) & 0xF, h1 & 0xF);
    let (rd, rm) = ((h2 >> 8) & 0xF, h2 & 0xF);
    if h2 & 0xF000 != 0xF000 {
        return a.undefined(w);
    }
    let m = a.r[rm as usize];
    if op1 & 8 == 0 && op2 == 0 {
        // LSL/LSR/ASR/ROR{S} Rd, Rn, Rm
        let (r, c) = shift_reg(a.r[rn as usize], (op1 >> 1) & 3, m & 0xFF, a.carry());
        set_reg(a, rd, r);
        if op1 & 1 != 0 {
            a.set_nz(r);
            a.set_flag(psr::C, c);
        }
    } else if op1 < 6 && op2 & 8 != 0 {
        // SXTAH/UXTAH/SXTAB16/UXTAB16/SXTAB/UXTAB (Rn = 15: no addition)
        let v = m.rotate_right(8 * ((h2 >> 4) & 3));
        let add = if rn == 15 { 0 } else { a.r[rn as usize] };
        let r = match op1 {
            0 => add.wrapping_add(v as i16 as i32 as u32),
            1 => add.wrapping_add(v & 0xFFFF),
            2 | 3 => {
                let ext = |b: u32| if op1 == 2 { b as u8 as i8 as i32 as u32 } else { b & 0xFF };
                let lo = (add & 0xFFFF).wrapping_add(ext(v)) & 0xFFFF;
                let hi = (add >> 16).wrapping_add(ext(v >> 16)) & 0xFFFF;
                hi << 16 | lo
            }
            4 => add.wrapping_add(v as i8 as i32 as u32),
            _ => add.wrapping_add(v & 0xFF),
        };
        set_reg(a, rd, r);
    } else if op1 & 0xC == 0x8 && op2 & 0xC == 0x8 {
        let r = match (op1 & 3, op2 & 3) {
            (1, 0) => m.swap_bytes(),
            (1, 1) => ((m & 0x00FF_00FF) << 8) | ((m >> 8) & 0x00FF_00FF),
            (1, 2) => m.reverse_bits(),
            (1, 3) => (m as u16).swap_bytes() as i16 as i32 as u32,
            (3, 0) => m.leading_zeros(),
            _ => return a.undefined(w),
        };
        set_reg(a, rd, r);
    } else {
        a.undefined(w);
    }
}

/// MUL, MLA, MLS, the halfword multiplies, SMMUL/SMMLA.
fn multiply(a: &mut Arm, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    let (op1, rn) = ((h1 >> 4) & 7, h1 & 0xF);
    let (ra, rd, rm) = ((h2 >> 12) & 0xF, (h2 >> 8) & 0xF, h2 & 0xF);
    let (n, m) = (a.r[rn as usize], a.r[rm as usize]);
    let acc = if ra == 15 { 0 } else { a.r[ra as usize] };
    let r = match (op1, (h2 >> 4) & 3) {
        (0, 0) => n.wrapping_mul(m).wrapping_add(acc),
        (0, 1) => acc.wrapping_sub(n.wrapping_mul(m)),
        (1, x) => {
            // SMUL<x><y>, SMLA<x><y>
            let half = |v: u32, top: bool| (if top { v >> 16 } else { v }) as i16 as i32;
            let p = half(n, x & 2 != 0) * half(m, x & 1 != 0);
            let (r, o) = p.overflowing_add(acc as i32);
            if o && ra != 15 {
                a.cpsr |= psr::Q;
            }
            r as u32
        }
        (3, x) if x < 2 => {
            // SMULW<y>, SMLAW<y>
            let mh = (if x & 1 != 0 { m >> 16 } else { m }) as i16 as i64;
            let p = ((n as i32 as i64 * mh) >> 16) as i32;
            let (r, o) = p.overflowing_add(acc as i32);
            if o && ra != 15 {
                a.cpsr |= psr::Q;
            }
            r as u32
        }
        (5, x) | (6, x) if x < 2 => {
            // SMMUL{R}, SMMLA{R}, SMMLS{R}
            let p = n as i32 as i64 * m as i32 as i64;
            let base = (acc as i64) << 32;
            let mut v = if op1 == 5 { base.wrapping_add(p) } else { base.wrapping_sub(p) };
            if x & 1 != 0 {
                v = v.wrapping_add(0x8000_0000);
            }
            (v >> 32) as u32
        }
        _ => return a.undefined(w),
    };
    set_reg(a, rd, r);
}

/// SMULL, UMULL, SMLAL, UMLAL, UMAAL, SDIV, UDIV.
fn long_multiply(a: &mut Arm, h1: u32, h2: u32) {
    let w = h1 << 16 | h2;
    let (op1, op2, rn) = ((h1 >> 4) & 7, (h2 >> 4) & 0xF, h1 & 0xF);
    let (lo, hi, rm) = ((h2 >> 12) & 0xF, (h2 >> 8) & 0xF, h2 & 0xF);
    let (n, m) = (a.r[rn as usize], a.r[rm as usize]);
    let acc = (a.r[hi as usize] as u64) << 32 | a.r[lo as usize] as u64;
    let r = match (op1, op2) {
        (0, 0) => (n as i32 as i64 * m as i32 as i64) as u64,
        (2, 0) => n as u64 * m as u64,
        (4, 0) => acc.wrapping_add((n as i32 as i64 * m as i32 as i64) as u64),
        (6, 0) => acc.wrapping_add(n as u64 * m as u64),
        (6, 6) => (n as u64 * m as u64).wrapping_add(a.r[lo as usize] as u64).wrapping_add(a.r[hi as usize] as u64),
        (1, 0xF) | (3, 0xF) => {
            // SDIV/UDIV Rd(= the RdHi field), Rn, Rm; division by 0 gives 0
            let q = if m == 0 {
                0
            } else if op1 == 1 {
                (n as i32).wrapping_div(m as i32) as u32
            } else {
                n / m
            };
            set_reg(a, hi, q);
            return;
        }
        _ => return a.undefined(w),
    };
    a.r[lo as usize] = r as u32;
    a.r[hi as usize] = (r >> 32) as u32;
}
