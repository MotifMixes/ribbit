//! ARMv7-A additions to the ARM (32-bit) instruction set, and the VFP
//! floating-point unit (VFPv2 with VFPv3's extra instructions, 32 single /
//! 16 double registers), for programs built for a Cortex-A9 (the LeapPad 2's
//! LF2000, docs/leappad2.md). Used only when `Arm::v7` is on, so the ARM926
//! machines still take an undefined-instruction exception for all of these.
//!
//! What's here, by the ARMv7-A Architecture Reference Manual (ARM DDI 0406C)
//! sections: MOVW/MOVT (A8.8.102/106), the exclusives LDREX/STREX and their
//! B/H/D forms with a local monitor (A3.4), MLS/UMAAL, the media instructions
//! (A5.4: parallel add/subtract, pack/unpack/extend, saturate, reverse, the
//! signed multiplies, USAD8, bit-field extract/insert), the barriers and
//! hints (DMB/DSB/ISB/CLREX/PLI/PLD as no-ops: one processor, no caches
//! modelled), and VFP (A7: VLDR/VSTR/VLDM/VSTM, VMOV between core and VFP
//! registers, VMRS/VMSR, the data-processing and conversion instructions).
//! Not here: Thumb-2, NEON (the LF2000 has none), ARMv7 virtualization.
//!
//! VFP arithmetic uses the host's IEEE 754 single and double operations with
//! round to nearest (the FPSCR's RMode is honoured by VCVTR and the integer
//! conversions only); flush-to-zero and default-NaN follow FPSCR.FZ/DN. The
//! cumulative exception flags are not kept.

use crate::{Arm, ArmBus, Fault, psr};

/// The ARMv7 state an ARM926 hasn't got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct V7 {
    /// S0-S63 (D0-D31 as pairs): VFP's register file.
    pub s: [u32; 64],
    pub fpscr: u32,
    pub fpexc: u32,
    /// The local exclusive monitor: the address LDREX marked.
    pub monitor: Option<u32>,
    /// TPIDRURW / TPIDRURO (CP15 c13, c0, 2/3): the thread ID registers.
    pub tpidrurw: u32,
    pub tpidruro: u32,
}

impl Default for V7 {
    fn default() -> Self {
        Self { s: [0; 64], fpscr: 0, fpexc: 1 << 30, monitor: None, tpidrurw: 0, tpidruro: 0 }
    }
}

impl V7 {
    pub fn d(&self, n: usize) -> u64 {
        self.s[2 * n] as u64 | (self.s[2 * n + 1] as u64) << 32
    }
    pub fn set_d(&mut self, n: usize, v: u64) {
        self.s[2 * n] = v as u32;
        self.s[2 * n + 1] = (v >> 32) as u32;
    }
}

fn reg(a: &Arm, n: u32) -> u32 {
    if n == 15 { a.current.wrapping_add(8) } else { a.r[n as usize] }
}

fn set_reg(a: &mut Arm, n: u32, v: u32) {
    if n == 15 {
        a.r[15] = v & !3;
    } else {
        a.r[n as usize] = v;
    }
}

fn v7(a: &mut Arm) -> &mut V7 {
    a.v7.as_mut().expect("ARMv7 instruction on an ARMv5 processor")
}

/// MOVW (bit 22 clear) / MOVT: cond 0011 0x00 imm4 Rd imm12.
pub(crate) fn mov_wide(a: &mut Arm, w: u32) {
    let imm = ((w >> 4) & 0xF000) | (w & 0xFFF);
    let rd = (w >> 12) & 0xF;
    let v = if w & (1 << 22) != 0 { (a.r[rd as usize] & 0xFFFF) | imm << 16 } else { imm };
    set_reg(a, rd, v);
}

/// MLS (cond 0000 0110 Rd Ra Rm 1001 Rn) and UMAAL (0000 0100 RdHi RdLo Rm 1001 Rn).
/// Returns false for other encodings of the space.
pub(crate) fn multiply_extra(a: &mut Arm, w: u32) -> bool {
    let (rd, ra, rm, rn) = ((w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    match w & 0x0FF0_00F0 {
        0x0060_0090 => {
            let r = a.r[ra as usize].wrapping_sub(a.r[rn as usize].wrapping_mul(a.r[rm as usize]));
            set_reg(a, rd, r);
            true
        }
        0x0040_0090 => {
            let r = a.r[rn as usize] as u64 * a.r[rm as usize] as u64 + a.r[ra as usize] as u64 + a.r[rd as usize] as u64;
            set_reg(a, ra, r as u32);
            set_reg(a, rd, (r >> 32) as u32);
            true
        }
        _ => false,
    }
}

/// LDREX/STREX and the D, B and H forms: cond 0001 1 op(2) L Rn Rt 1111 1001 Rt2.
pub(crate) fn exclusive(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let (rn, rd, rt) = ((w >> 16) & 0xF, (w >> 12) & 0xF, w & 0xF);
    let addr = a.r[rn as usize];
    let size = (w >> 21) & 3; // 0 word, 1 doubleword, 2 byte, 3 halfword
    if w & (1 << 20) != 0 {
        let r = match size {
            0 => a.load32(bus, addr).map(|v| (v, None)),
            1 => a.load32(bus, addr).and_then(|lo| a.load32(bus, addr.wrapping_add(4)).map(|hi| (lo, Some(hi)))),
            2 => a.load8(bus, addr).map(|v| (v, None)),
            _ => a.load16(bus, addr).map(|v| (v, None)),
        };
        match r {
            Ok((v, hi)) => {
                set_reg(a, rd, v);
                if let Some(hi) = hi {
                    set_reg(a, (rd + 1) & 15, hi);
                }
                v7(a).monitor = Some(addr);
            }
            Err(f) => a.data_abort(f),
        }
        return;
    }
    // Store: only while the monitor holds this address.
    if v7(a).monitor != Some(addr) {
        set_reg(a, rd, 1);
        return;
    }
    let v = a.r[rt as usize];
    let r = match size {
        0 => a.store32(bus, addr, v),
        1 => {
            let hi = a.r[((rt + 1) & 15) as usize];
            a.store32(bus, addr, v).and_then(|_| a.store32(bus, addr.wrapping_add(4), hi))
        }
        2 => a.store8(bus, addr, v),
        _ => a.store16(bus, addr, v),
    };
    if let Err(f) = r {
        return a.data_abort(f);
    }
    v7(a).monitor = None;
    set_reg(a, rd, 0);
}

/// Whether `w` (cond = 0b1111) is a barrier or hint this core treats as a
/// no-op: DMB/DSB/ISB, CLREX (which also clears the monitor), PLI/PLD.
pub(crate) fn unconditional(a: &mut Arm, w: u32) -> bool {
    if w & 0xFFFF_FF00 == 0xF57F_F000 {
        if w & 0xF0 == 0x10 {
            v7(a).monitor = None;
        }
        return true;
    }
    // PLI/PLD (immediate and register forms): 1111 01xx x1x1 Rn 1111 ...
    w & 0x0C10_F000 == 0x0410_F000 && (w & 0x0200_0000 == 0 || w & 0x10 == 0)
}

fn sat_signed(v: i64, bits: u32) -> (u32, bool) {
    let max = (1i64 << (bits - 1)) - 1;
    let min = -(1i64 << (bits - 1));
    if v > max {
        (max as u32, true)
    } else if v < min {
        (min as u32, true)
    } else {
        (v as u32, false)
    }
}

fn sat_unsigned(v: i64, bits: u32) -> (u32, bool) {
    let max = if bits == 32 { u32::MAX as i64 } else { (1i64 << bits) - 1 };
    if v > max {
        (max as u32, true)
    } else if v < 0 {
        (0, true)
    } else {
        (v as u32, false)
    }
}

/// The media instructions: cond 011x xxxx ... xxx1 xxxx (A5.4).
pub(crate) fn media(a: &mut Arm, w: u32) {
    let op1 = (w >> 20) & 0x1F;
    let op2 = (w >> 5) & 7;
    let (rd, rs, rm) = ((w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    match op1 >> 3 {
        0 => parallel(a, w),
        1 => pack_saturate_reverse(a, w),
        2 => signed_multiply(a, w),
        _ => match (op1, op2) {
            (0x18, 0) => {
                // USAD8 (Rd = 1111 in bits 15-12 means no accumulate) / USADA8.
                let (x, y) = (a.r[rm as usize], a.r[rs as usize]);
                let mut sum = 0u32;
                for i in 0..4 {
                    let (p, q) = ((x >> (8 * i)) as u8 as i32, (y >> (8 * i)) as u8 as i32);
                    sum += (p - q).unsigned_abs();
                }
                let d = (w >> 16) & 0xF;
                let acc = if rd == 15 { 0 } else { a.r[rd as usize] };
                set_reg(a, d, sum.wrapping_add(acc));
            }
            (0x1A | 0x1B, 2 | 6) | (0x1E | 0x1F, 2 | 6) => {
                // SBFX / UBFX: Rd, Rn(bits 3-0), lsb (bits 11-7), widthm1 (bits 20-16).
                let lsb = (w >> 7) & 0x1F;
                let width = ((w >> 16) & 0x1F) + 1;
                let v = a.r[rm as usize] >> lsb;
                let r = if width >= 32 {
                    v
                } else if op1 >= 0x1E {
                    v & ((1 << width) - 1)
                } else {
                    (((v << (32 - width)) as i32) >> (32 - width)) as u32
                };
                set_reg(a, rd, r);
            }
            (0x1C | 0x1D, 0 | 4) => {
                // BFI (Rn = Rm field) / BFC (Rm = 1111): msb bits 20-16, lsb 11-7.
                let lsb = (w >> 7) & 0x1F;
                let msb = (w >> 16) & 0x1F;
                if msb >= lsb {
                    let width = msb - lsb + 1;
                    let mask = if width == 32 { u32::MAX } else { ((1u32 << width) - 1) << lsb };
                    let src = if rm == 15 { 0 } else { a.r[rm as usize] << lsb };
                    let v = (a.r[rd as usize] & !mask) | (src & mask);
                    set_reg(a, rd, v);
                }
            }
            _ => a.undefined(w),
        },
    }
}

/// SADD16 ... UHSAX: the parallel add and subtract instructions (A5.4.1, A5.4.2).
fn parallel(a: &mut Arm, w: u32) {
    let op1 = (w >> 20) & 7; // 1 S/U, 2 Q/UQ, 3 SH/UH; 5-7 the unsigned ones
    let op2 = (w >> 5) & 7; // 0 ADD16, 1 ASX, 2 SAX, 3 SUB16, 4 ADD8, 7 SUB8
    let (rn, rd, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, w & 0xF);
    let unsigned = op1 & 4 != 0;
    let kind = op1 & 3;
    if kind == 0 || matches!(op2, 5 | 6) {
        return a.undefined(w);
    }
    let (x, y) = (a.r[rn as usize], a.r[rm as usize]);
    let lanes8 = op2 >= 4;
    let (n, bits) = if lanes8 { (4, 8) } else { (2, 16) };
    let lane = |v: u32, i: u32| -> i64 {
        let raw = (v >> (bits * i)) & ((1 << bits) - 1);
        if unsigned { raw as i64 } else if bits == 8 { raw as u8 as i8 as i64 } else { raw as u16 as i16 as i64 }
    };
    let mut result = 0u32;
    let mut ge = 0u32;
    for i in 0..n {
        let (p, q, add) = match op2 {
            0 | 4 => (lane(x, i), lane(y, i), true),
            3 | 7 => (lane(x, i), lane(y, i), false),
            // ASX: top = x.top + y.bottom, bottom = x.bottom - y.top.
            1 => (lane(x, i), lane(y, 1 - i), i == 1),
            // SAX: top = x.top - y.bottom, bottom = x.bottom + y.top.
            _ => (lane(x, i), lane(y, 1 - i), i == 0),
        };
        let r = if add { p + q } else { p - q };
        let v = match kind {
            1 => {
                let set = if unsigned { if add { r >> bits != 0 } else { r >= 0 } } else { r >= 0 };
                if set {
                    ge |= if lanes8 { 1 << i } else { 3 << (2 * i) };
                }
                r as u32
            }
            2 => {
                if unsigned { sat_unsigned(r, bits).0 } else { sat_signed(r, bits).0 }
            }
            _ => (r >> 1) as u32,
        };
        result |= (v & ((1 << bits) - 1)) << (bits * i);
    }
    if kind == 1 {
        a.cpsr = (a.cpsr & !(0xF << 16)) | ge << 16;
    }
    set_reg(a, rd, result);
}

/// PKH, SXT*/UXT* (and their accumulating forms), SEL, SSAT/USAT and the
/// 16-bit ones, REV/REV16/REVSH/RBIT (A5.4.3).
fn pack_saturate_reverse(a: &mut Arm, w: u32) {
    let op1 = (w >> 20) & 7;
    let op2 = (w >> 5) & 7;
    let (rn, rd, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, w & 0xF);
    let m = a.r[rm as usize];
    let rotated = m.rotate_right(8 * ((w >> 10) & 3));
    let acc = |a: &Arm| if rn == 15 { 0 } else { a.r[rn as usize] };
    match (op1, op2) {
        (0, 0 | 2 | 4 | 6) => {
            // PKHBT (tb = 0) / PKHTB.
            let imm = (w >> 7) & 0x1F;
            let n = a.r[rn as usize];
            let r = if op2 & 2 == 0 {
                (n & 0xFFFF) | (m << imm) & 0xFFFF_0000
            } else {
                let sh = if imm == 0 { ((m as i32) >> 31) as u32 } else { ((m as i32) >> imm) as u32 };
                (n & 0xFFFF_0000) | (sh & 0xFFFF)
            };
            set_reg(a, rd, r);
        }
        (0, 3) | (4, 3) => {
            // SXTB16/SXTAB16, UXTB16/UXTAB16.
            let (lo, hi) = (rotated & 0xFF, (rotated >> 16) & 0xFF);
            let (lo, hi) = if op1 == 0 { (lo as u8 as i8 as u32, hi as u8 as i8 as u32) } else { (lo, hi) };
            let n = acc(a);
            let r = ((n & 0xFFFF).wrapping_add(lo) & 0xFFFF) | ((n >> 16).wrapping_add(hi) & 0xFFFF) << 16;
            set_reg(a, rd, r);
        }
        (0, 5) => {
            // SEL: bytes by the GE flags.
            let ge = (a.cpsr >> 16) & 0xF;
            let n = a.r[rn as usize];
            let mut r = 0;
            for i in 0..4 {
                let src = if ge & (1 << i) != 0 { n } else { m };
                r |= src & (0xFF << (8 * i));
            }
            set_reg(a, rd, r);
        }
        (2 | 3, 0 | 2 | 4 | 6) | (6 | 7, 0 | 2 | 4 | 6) => {
            // SSAT / USAT: sat_imm bits 20-16, shift (bit 6) by imm5 (bits 11-7).
            let imm = (w >> 7) & 0x1F;
            let n = a.r[rm as usize];
            let v = if w & 0x40 != 0 {
                ((n as i32) >> if imm == 0 { 31 } else { imm }) as i64
            } else {
                ((n << imm) as i32) as i64
            };
            let sat = (w >> 16) & 0x1F;
            let (r, q) = if op1 & 4 == 0 { sat_signed(v, sat + 1) } else { sat_unsigned(v, sat) };
            set_reg(a, rd, r);
            if q {
                a.cpsr |= psr::Q;
            }
        }
        (2, 1) | (6, 1) => {
            // SSAT16 / USAT16.
            let sat = (w >> 16) & 0xF;
            let mut r = 0;
            let mut q = false;
            for i in 0..2 {
                let v = (m >> (16 * i)) as u16 as i16 as i64;
                let (s, hit) = if op1 == 2 { sat_signed(v, sat + 1) } else { sat_unsigned(v, sat) };
                q |= hit;
                r |= (s & 0xFFFF) << (16 * i);
            }
            set_reg(a, rd, r);
            if q {
                a.cpsr |= psr::Q;
            }
        }
        (2, 3) => set_reg(a, rd, acc(a).wrapping_add(rotated as u8 as i8 as u32)),
        (3, 3) => set_reg(a, rd, acc(a).wrapping_add(rotated as u16 as i16 as u32)),
        (6, 3) => set_reg(a, rd, acc(a).wrapping_add(rotated & 0xFF)),
        (7, 3) => set_reg(a, rd, acc(a).wrapping_add(rotated & 0xFFFF)),
        (3, 1) => set_reg(a, rd, m.swap_bytes()),
        (3, 5) => set_reg(a, rd, ((m & 0x00FF_00FF) << 8) | ((m >> 8) & 0x00FF_00FF)),
        (7, 1) => set_reg(a, rd, m.reverse_bits()),
        (7, 5) => set_reg(a, rd, (m as u16).swap_bytes() as i16 as i32 as u32),
        _ => a.undefined(w),
    }
}

/// SMLAD/SMUAD, SMLSD/SMUSD, SDIV/UDIV, SMLALD, SMLSLD, SMMLA/SMMUL, SMMLS (A5.4.4).
fn signed_multiply(a: &mut Arm, w: u32) {
    let op1 = (w >> 20) & 7;
    let op2 = (w >> 5) & 7;
    let (rd, ra, rm, rn) = ((w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    let (n, mut m) = (a.r[rn as usize], a.r[rm as usize]);
    if op2 & 1 != 0 && op1 != 1 && op1 != 3 {
        // The X (swap halves) bit of the dual multiplies.
        m = m.rotate_right(16);
    }
    let lo = |v: u32| v as u16 as i16 as i64;
    let hi = |v: u32| (v >> 16) as u16 as i16 as i64;
    match (op1, op2 & 6) {
        (0, 0) | (0, 2) => {
            // SMLAD / SMUAD (Ra = 1111), SMLSD / SMUSD (op2 bit 1).
            let (p1, p2) = (lo(n) * lo(m), hi(n) * hi(m));
            let sum = if op2 & 2 == 0 { p1 + p2 } else { p1 - p2 };
            let total = if ra == 15 { sum } else { sum + a.r[ra as usize] as i32 as i64 };
            if total != total as i32 as i64 {
                a.cpsr |= psr::Q;
            }
            set_reg(a, rd, total as u32);
        }
        (1, 0) | (3, 0) => {
            // SDIV (1) / UDIV (3): Rd = Rn / Rm, 0 for a division by 0.
            let r = if op1 == 1 {
                let (x, y) = (n as i32, a.r[rm as usize] as i32);
                if y == 0 { 0 } else { x.wrapping_div(y) as u32 }
            } else {
                let y = a.r[rm as usize];
                if y == 0 { 0 } else { n / y }
            };
            set_reg(a, rd, r);
        }
        (4, 0) | (4, 2) => {
            // SMLALD / SMLSLD: RdHi (bits 19-16), RdLo (bits 15-12).
            let (p1, p2) = (lo(n) * lo(m), hi(n) * hi(m));
            let sum = if op2 & 2 == 0 { p1 + p2 } else { p1 - p2 };
            let acc = ((a.r[rd as usize] as u64) << 32 | a.r[ra as usize] as u64) as i64;
            let r = acc.wrapping_add(sum) as u64;
            set_reg(a, ra, r as u32);
            set_reg(a, rd, (r >> 32) as u32);
        }
        (5, 0) | (5, 6) => {
            // SMMLA / SMMUL (Ra = 1111) (op2 0), SMMLS (op2 6); R (bit 5) rounds.
            let p = n as i32 as i64 * a.r[rm as usize] as i32 as i64;
            let acc = if ra == 15 { 0 } else { (a.r[ra as usize] as i64) << 32 };
            let mut r = if op2 & 6 == 0 { acc.wrapping_add(p) } else { acc.wrapping_sub(p) };
            if w & 0x20 != 0 {
                r = r.wrapping_add(0x8000_0000);
            }
            set_reg(a, rd, (r >> 32) as u32);
        }
        _ => a.undefined(w),
    }
}

// ---- VFP ----------------------------------------------------------------

const FPSCR_DN: u32 = 1 << 25;
const FPSCR_FZ: u32 = 1 << 24;
const DEFAULT_NAN32: u32 = 0x7FC0_0000;
const DEFAULT_NAN64: u64 = 0x7FF8_0000_0000_0000;

/// Whether `w` is a VFP instruction (coprocessor 10 or 11).
pub(crate) fn is_vfp(w: u32) -> bool {
    (w >> 28) != 0xF && matches!((w >> 25) & 7, 6 | 7) && matches!((w >> 8) & 0xF, 10 | 11)
}

fn sreg(v: u32, low: u32) -> usize {
    ((v << 1) | low) as usize
}
fn dreg(v: u32, high: u32) -> usize {
    ((high << 4) | v) as usize
}

fn f32_in(st: &V7, i: usize) -> f32 {
    let mut b = st.s[i];
    if st.fpscr & FPSCR_FZ != 0 && b & 0x7F80_0000 == 0 {
        b &= 0x8000_0000;
    }
    f32::from_bits(b)
}
fn f64_in(st: &V7, i: usize) -> f64 {
    let mut b = st.d(i);
    if st.fpscr & FPSCR_FZ != 0 && b & 0x7FF0_0000_0000_0000 == 0 {
        b &= 0x8000_0000_0000_0000;
    }
    f64::from_bits(b)
}
fn f32_out(st: &mut V7, i: usize, v: f32) {
    let mut b = v.to_bits();
    if v.is_nan() {
        b = if st.fpscr & FPSCR_DN != 0 { DEFAULT_NAN32 } else { b | 0x0040_0000 };
    } else if st.fpscr & FPSCR_FZ != 0 && b & 0x7F80_0000 == 0 {
        b &= 0x8000_0000;
    }
    st.s[i] = b;
}
fn f64_out(st: &mut V7, i: usize, v: f64) {
    let mut b = v.to_bits();
    if v.is_nan() {
        b = if st.fpscr & FPSCR_DN != 0 { DEFAULT_NAN64 } else { b | 0x0008_0000_0000_0000 };
    } else if st.fpscr & FPSCR_FZ != 0 && b & 0x7FF0_0000_0000_0000 == 0 {
        b &= 0x8000_0000_0000_0000;
    }
    st.set_d(i, b);
}

/// Rounds by the FPSCR's RMode (0 nearest even, 1 towards +inf, 2 towards -inf, 3 towards 0).
fn round(v: f64, rmode: u32) -> f64 {
    match rmode {
        0 => v.round_ties_even(),
        1 => v.ceil(),
        2 => v.floor(),
        _ => v.trunc(),
    }
}

fn to_int(v: f64, signed: bool, rmode: u32) -> u32 {
    if v.is_nan() {
        return 0;
    }
    let r = round(v, rmode);
    if signed {
        r.clamp(i32::MIN as f64, i32::MAX as f64) as i32 as u32
    } else {
        r.clamp(0.0, u32::MAX as f64) as u32
    }
}

fn compare_flags(x: f64, y: f64) -> u32 {
    if x.is_nan() || y.is_nan() {
        0x3
    } else if x == y {
        0x6
    } else if x < y {
        0x8
    } else {
        0x2
    }
}

/// VFP data processing (CDP to cp10/11): cond 1110 ... (A7.5).
pub(crate) fn vfp_data(a: &mut Arm, w: u32) {
    let double = w & (1 << 8) != 0;
    let (d, n, m) = ((w >> 22) & 1, (w >> 7) & 1, (w >> 5) & 1);
    let (vd, vn, vm) = ((w >> 12) & 0xF, (w >> 16) & 0xF, w & 0xF);
    let opc1 = (w >> 20) & 0xB; // bits 23, 21, 20 (bit 22 is D)
    let op = (w >> 6) & 1;
    let (di, ni, mi) = if double { (dreg(vd, d), dreg(vn, n), dreg(vm, m)) } else { (sreg(vd, d), sreg(vn, n), sreg(vm, m)) };
    let st = v7(a);
    let rmode = (st.fpscr >> 22) & 3;
    macro_rules! arith {
        ($x:ident, $y:ident, $acc:ident => $e:expr) => {{
            if double {
                let ($x, $y, $acc) = (f64_in(st, ni), f64_in(st, mi), f64_in(st, di));
                let r: f64 = $e;
                f64_out(st, di, r);
            } else {
                let ($x, $y, $acc) = (f32_in(st, ni), f32_in(st, mi), f32_in(st, di));
                let r: f32 = $e;
                f32_out(st, di, r);
            }
        }};
    }
    match (opc1, op) {
        // VMLA / VMLS: d = d + (n * m) / d - (n * m); the product is rounded first.
        (0x0, 0) => arith!(x, y, acc => acc + x * y),
        (0x0, 1) => arith!(x, y, acc => acc - x * y),
        // VNMLS / VNMLA: d = -d + n*m / -d - n*m.
        (0x1, 0) => arith!(x, y, acc => x * y - acc),
        (0x1, 1) => arith!(x, y, acc => -(x * y) - acc),
        (0x2, 0) => arith!(x, y, _acc => x * y),
        (0x2, 1) => arith!(x, y, _acc => -(x * y)),
        (0x3, 0) => arith!(x, y, _acc => x + y),
        (0x3, 1) => arith!(x, y, _acc => x - y),
        (0x8, 0) => arith!(x, y, _acc => x / y),
        (0xB, _) => vfp_other(a, w, double, di, mi, rmode),
        _ => a.undefined(w),
    }
}

/// VMOV (immediate, register), VABS, VNEG, VSQRT, VCMP(E), VCVT (A7.5 table A7-17).
fn vfp_other(a: &mut Arm, w: u32, double: bool, di: usize, mi: usize, rmode: u32) {
    let opc2 = (w >> 16) & 0xF;
    let opc3 = (w >> 6) & 3;
    let st = v7(a);
    if opc3 & 1 == 0 {
        // VMOV (immediate), VFPv3: imm8 = bits 19-16:3-0.
        let imm8 = ((w >> 16) & 0xF) << 4 | (w & 0xF);
        let sign = (imm8 >> 7) & 1;
        let b6 = (imm8 >> 6) & 1;
        if double {
            let exp = ((b6 ^ 1) as u64) << 10 | (if b6 == 1 { 0xFF } else { 0 }) << 2 | ((imm8 >> 4) & 3) as u64;
            let bits = (sign as u64) << 63 | exp << 52 | ((imm8 & 0xF) as u64) << 48;
            st.set_d(di, bits);
        } else {
            let exp = (b6 ^ 1) << 7 | (if b6 == 1 { 0x1F } else { 0 }) << 2 | ((imm8 >> 4) & 3);
            st.s[di] = sign << 31 | exp << 23 | (imm8 & 0xF) << 19;
        }
        return;
    }
    match (opc2, opc3) {
        (0, 1) => {
            if double {
                let v = st.d(mi);
                st.set_d(di, v);
            } else {
                st.s[di] = st.s[mi];
            }
        }
        (0, 3) => {
            if double {
                let v = st.d(mi) & !(1 << 63);
                st.set_d(di, v);
            } else {
                st.s[di] = st.s[mi] & 0x7FFF_FFFF;
            }
        }
        (1, 1) => {
            if double {
                let v = st.d(mi) ^ (1 << 63);
                st.set_d(di, v);
            } else {
                st.s[di] = st.s[mi] ^ 0x8000_0000;
            }
        }
        (1, 3) => {
            if double {
                let v = f64_in(st, mi).sqrt();
                f64_out(st, di, v);
            } else {
                let v = f32_in(st, mi).sqrt();
                f32_out(st, di, v);
            }
        }
        (4, _) | (5, _) => {
            // VCMP(E): with Vm, or with zero (opc2 5).
            let (x, y) = if double {
                (f64_in(st, di), if opc2 == 5 { 0.0 } else { f64_in(st, mi) })
            } else {
                (f32_in(st, di) as f64, if opc2 == 5 { 0.0 } else { f32_in(st, mi) as f64 })
            };
            st.fpscr = (st.fpscr & 0x0FFF_FFFF) | compare_flags(x, y) << 28;
        }
        (7, 3) => {
            // VCVT between double and single: Vd is the other size.
            let d = (w >> 22) & 1;
            let vd = (w >> 12) & 0xF;
            if double {
                let v = f64_in(st, mi) as f32;
                f32_out(st, sreg(vd, d), v);
            } else {
                let v = f32_in(st, mi) as f64;
                f64_out(st, dreg(vd, d), v);
            }
        }
        (8, _) => {
            // VCVT to floating point from a 32-bit integer in Sm (bit 7: signed).
            let m = (w >> 5) & 1;
            let src = st.s[sreg(w & 0xF, m)];
            let v = if opc3 & 2 != 0 { src as i32 as f64 } else { src as f64 };
            if double {
                f64_out(st, di, v);
            } else {
                f32_out(st, di, v as f32);
            }
        }
        (12, _) | (13, _) => {
            // VCVT(R) to a 32-bit integer in Sd: opc2 13 signed; bit 7 set rounds towards zero.
            let d = (w >> 22) & 1;
            let vd = (w >> 12) & 0xF;
            let v = if double { f64_in(st, mi) } else { f32_in(st, mi) as f64 };
            let mode = if opc3 & 2 != 0 { 3 } else { rmode };
            st.s[sreg(vd, d)] = to_int(v, opc2 == 13, mode);
        }
        (10, _) | (11, _) | (14, _) | (15, _) => {
            // VCVT between floating point and fixed point (VFPv3), in place:
            // opc2 bit 2 to fixed, bit 0 unsigned; sx (bit 7) 32-bit; frac bits = size - imm.
            let to_fixed = opc2 & 4 != 0;
            let unsigned = opc2 & 1 != 0;
            let size = if opc3 & 2 != 0 { 32 } else { 16 };
            let imm = ((w & 0xF) << 1) | ((w >> 5) & 1);
            let frac = size as i32 - imm as i32;
            let scale = 2f64.powi(frac);
            if to_fixed {
                let v = if double { f64_in(st, di) } else { f32_in(st, di) as f64 } * scale;
                let r = if size == 16 {
                    let t = v.trunc();
                    if unsigned { t.clamp(0.0, 65535.0) as u32 } else { t.clamp(-32768.0, 32767.0) as i32 as u32 }
                } else {
                    to_int(v, !unsigned, 3)
                };
                if double {
                    st.set_d(di, if unsigned { r as u64 } else { r as i32 as i64 as u64 });
                } else {
                    st.s[di] = r;
                }
            } else {
                let raw = if double { st.d(di) as u32 } else { st.s[di] };
                let raw = if size == 16 { if unsigned { raw & 0xFFFF } else { raw as u16 as i16 as i32 as u32 } } else { raw };
                let v = if unsigned { raw as f64 } else { raw as i32 as f64 } / scale;
                if double {
                    f64_out(st, di, v);
                } else {
                    f32_out(st, di, v as f32);
                }
            }
        }
        _ => a.undefined(w),
    }
}

/// VMOV between an ARM register and a single / a doubleword's half, VMRS, VMSR
/// (MCR/MRC to cp10/11, A7.8).
pub(crate) fn vfp_register(a: &mut Arm, w: u32) {
    let rt = (w >> 12) & 0xF;
    let load = w & (1 << 20) != 0;
    let opc1 = (w >> 21) & 7;
    if w & (1 << 8) == 0 {
        if opc1 == 0 {
            // VMOV Sn <-> Rt.
            let s = sreg((w >> 16) & 0xF, (w >> 7) & 1);
            if load {
                let v = v7(a).s[s];
                set_reg(a, rt, v);
            } else {
                let v = reg(a, rt);
                v7(a).s[s] = v;
            }
            return;
        }
        if opc1 == 7 {
            let which = (w >> 16) & 0xF;
            if load {
                let st = v7(a);
                let v = match which {
                    0 => 0x4103_3094, // FPSID: ARM, VFPv3 (subarch 3), Cortex-A9
                    1 => st.fpscr,
                    6 => 0, // MVFR1
                    7 => 0x1011_1221, // MVFR0: 16 doubles... (short vectors none)
                    8 => st.fpexc,
                    _ => 0,
                };
                if rt == 15 {
                    a.cpsr = (a.cpsr & 0x0FFF_FFFF) | (v & 0xF000_0000);
                } else {
                    set_reg(a, rt, v);
                }
            } else {
                let v = reg(a, rt);
                let st = v7(a);
                match which {
                    1 => st.fpscr = v,
                    8 => st.fpexc = v,
                    _ => {}
                }
            }
            return;
        }
        return a.undefined(w);
    }
    // VMOV Dn[x] <-> Rt (32-bit scalars only).
    if opc1 & 2 == 0 && (w >> 5) & 3 == 0 {
        let dn = dreg((w >> 16) & 0xF, (w >> 7) & 1);
        let half = ((w >> 21) & 1) as usize;
        if load {
            let v = v7(a).s[2 * dn + half];
            set_reg(a, rt, v);
        } else {
            let v = reg(a, rt);
            v7(a).s[2 * dn + half] = v;
        }
        return;
    }
    a.undefined(w);
}

/// VLDR/VSTR, VLDM/VSTM (VPUSH/VPOP), and VMOV between two ARM registers and
/// two singles or a double (LDC/STC/MCRR/MRRC to cp10/11, A7.6, A7.8).
pub(crate) fn vfp_transfer(a: &mut Arm, bus: &mut impl ArmBus, w: u32) {
    let double = w & (1 << 8) != 0;
    let (p, u, d, wb, l) = (w & (1 << 24) != 0, w & (1 << 23) != 0, (w >> 22) & 1, w & (1 << 21) != 0, w & (1 << 20) != 0);
    let (rn, vd) = ((w >> 16) & 0xF, (w >> 12) & 0xF);
    if w & 0x0FE0_0000 == 0x0C40_0000 {
        // VMOV Rt, Rt2 <-> Sm, Sm+1 / Dm.
        let (rt2, rt) = ((w >> 16) & 0xF, (w >> 12) & 0xF);
        let (m, vm) = ((w >> 5) & 1, w & 0xF);
        let first = if double { 2 * dreg(vm, m) } else { sreg(vm, m) };
        if first + 1 >= 64 {
            return a.undefined(w);
        }
        if l {
            let (x, y) = { let st = v7(a); (st.s[first], st.s[first + 1]) };
            set_reg(a, rt, x);
            set_reg(a, rt2, y);
        } else {
            let (x, y) = (a.r[rt as usize], a.r[rt2 as usize]);
            let st = v7(a);
            st.s[first] = x;
            st.s[first + 1] = y;
        }
        return;
    }
    let imm = (w & 0xFF) * 4;
    let base = reg(a, rn) & if rn == 15 { !3 } else { !0 };
    let first = if double { 2 * dreg(vd, d) } else { sreg(vd, d) };
    let (words, start, new_base) = if p && !wb {
        // VLDR / VSTR.
        let addr = if u { base.wrapping_add(imm) } else { base.wrapping_sub(imm) };
        (if double { 2 } else { 1 }, addr, None)
    } else if !p && u {
        // VLDM/VSTM increment after (FLDMX's odd count: the extra word is skipped).
        let n = w & 0xFF;
        (if double { n & !1 } else { n }, base, wb.then(|| base.wrapping_add(imm)))
    } else if p && !u && wb {
        let n = w & 0xFF;
        (if double { n & !1 } else { n }, base.wrapping_sub(imm), Some(base.wrapping_sub(imm)))
    } else {
        return a.undefined(w);
    };
    if first + words as usize > 64 {
        return a.undefined(w);
    }
    let mut addr = start;
    for i in 0..words as usize {
        if l {
            match a.load32(bus, addr) {
                Ok(v) => v7(a).s[first + i] = v,
                Err(f) => return a.data_abort(f),
            }
        } else {
            let v = v7(a).s[first + i];
            if let Err(f) = a.store32(bus, addr, v) {
                return a.data_abort(f);
            }
        }
        addr = addr.wrapping_add(4);
    }
    if let Some(nb) = new_base {
        set_reg(a, rn, nb);
    }
}

/// An unaligned word load (ARMv7 with SCTLR.U: no rotation).
pub(crate) fn load32_unaligned(a: &mut Arm, bus: &mut impl ArmBus, va: u32) -> Result<u32, Fault> {
    let mut v = 0;
    for i in 0..4 {
        v |= a.load8(bus, va.wrapping_add(i))? << (8 * i);
    }
    Ok(v)
}

pub(crate) fn store32_unaligned(a: &mut Arm, bus: &mut impl ArmBus, va: u32, v: u32) -> Result<(), Fault> {
    for i in 0..4 {
        a.store8(bus, va.wrapping_add(i), v >> (8 * i))?;
    }
    Ok(())
}
