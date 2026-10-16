//! ARCompact instruction decoder (ARCtangent-A5 profile).
//!
//! Bit layouts are from PRM chapters 6 (32-bit formats) and 7 (16-bit
//! formats). Instruction fetch: the major opcode is bits 15..11 of the first
//! halfword; 0x00-0x0B are 32-bit, 0x0C-0x1F 16-bit. A 32-bit word, and a long
//! immediate, are stored as two little-endian halfwords, most-significant
//! halfword first ("packed middle-endian", PRM ch. 2).
//!
//! The decoder never rejects input: encodings the PRM defines as Instruction
//! Errors (or as undefined on the A5) decode to `Op::Illegal` with a reason, so
//! the executor and the disassembler can report them.

use crate::isa::*;
use crate::why;
use ribbit_memory::Bus;

/// Result of the old length-only probe; kept for callers that only need the
/// instruction boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Width {
    Short,
    Long,
}

impl Width {
    pub fn bytes(self) -> u32 {
        match self {
            Width::Short => 2,
            Width::Long => 4,
        }
    }
}

pub fn major_opcode(first_halfword: u16) -> u8 {
    (first_halfword >> 11) as u8
}

pub fn width_of(major: u8) -> Width {
    if major < 0x0C { Width::Long } else { Width::Short }
}

/// Decodes the instruction at `addr`, reading halfwords from the bus.
pub fn decode(bus: &mut impl Bus, addr: u32) -> Instr {
    decode_with(addr, |a| bus.fetch16(a))
}

/// Decodes the instruction at `addr`, reading halfwords with `fetch`.
pub fn decode_with(addr: u32, mut fetch: impl FnMut(u32) -> u16) -> Instr {
    let hw0 = fetch(addr);
    let mut i = Instr {
        addr,
        len: 2,
        word: hw0 as u32,
        short: true,
        op: Op::Nop,
        cc: COND_AL,
        set_flags: false,
        delay: false,
        dst: Dst::None,
        src1: Src::None,
        src2: Src::None,
        data: Src::None,
        mem: MemMode::WORD,
        target: 0,
        limm: None,
    };
    if major_opcode(hw0) < 0x0C {
        let w = ((hw0 as u32) << 16) | fetch(addr.wrapping_add(2)) as u32;
        i.word = w;
        i.len = 4;
        i.short = false;
        decode32(&mut i, w);
    } else {
        decode16(&mut i, hw0);
    }
    if [i.src1, i.src2, i.data].iter().any(|s| matches!(s, Src::Limm(_))) {
        let at = addr.wrapping_add(i.len);
        let limm = ((fetch(at) as u32) << 16) | fetch(at.wrapping_add(2)) as u32;
        for s in [&mut i.src1, &mut i.src2, &mut i.data] {
            if let Src::Limm(v) = s {
                *v = limm;
            }
        }
        i.limm = Some(limm);
        i.len += 4;
    }
    i
}

// ---------------------------------------------------------------------------
// Field helpers

fn bit(w: u32, n: u32) -> bool {
    (w >> n) & 1 != 0
}

/// Sign-extends the low `bits` bits of `v`.
fn sext(v: u32, bits: u32) -> u32 {
    let shift = 32 - bits;
    (((v << shift) as i32) >> shift) as u32
}

/// Source register field: r62 means "long immediate follows".
fn src_reg(r: u32) -> Src {
    if r == 62 { Src::Limm(0) } else { Src::Reg(r as u8) }
}

fn dst_reg(r: u32) -> Dst {
    if r == 62 { Dst::Discard } else { Dst::Reg(r as u8) }
}

/// 3-bit register field of 16-bit instructions: r0-r3, r12-r15.
fn r3(x: u32) -> u8 {
    let x = (x & 7) as u8;
    if x < 4 { x } else { x + 8 }
}

fn pcl(addr: u32) -> u32 {
    addr & !3
}

// ---------------------------------------------------------------------------
// 32-bit formats

fn breg32(w: u32) -> u32 {
    ((w >> 24) & 7) | (((w >> 12) & 7) << 3)
}

fn decode32(i: &mut Instr, w: u32) {
    let major = (w >> 27) as u8;
    match major {
        0x00 => {
            let lo = (w >> 17) & 0x3FF;
            let hi = (w >> 6) & 0x3FF;
            i.op = Op::B;
            i.delay = bit(w, 5);
            let off = if !bit(w, 16) {
                i.cc = (w & 0x1F) as u8;
                sext((hi << 10) | lo, 20) << 1
            } else {
                if bit(w, 4) {
                    i.op = Op::Illegal(why!("B far: reserved bit 4 set"));
                }
                sext(((w & 0xF) << 20) | (hi << 10) | lo, 24) << 1
            };
            i.target = pcl(i.addr).wrapping_add(off);
        }
        0x01 if !bit(w, 16) => {
            let lo = (w >> 18) & 0x1FF;
            let hi = (w >> 6) & 0x3FF;
            i.op = Op::Bl;
            i.delay = bit(w, 5);
            let off = if !bit(w, 17) {
                i.cc = (w & 0x1F) as u8;
                sext((hi << 9) | lo, 19) << 2
            } else {
                sext(((w & 0xF) << 19) | (hi << 9) | lo, 23) << 2
            };
            i.target = pcl(i.addr).wrapping_add(off);
        }
        0x01 => {
            let s = ((w >> 17) & 0x7F) | (((w >> 15) & 1) << 7);
            i.target = pcl(i.addr).wrapping_add(sext(s, 8) << 1);
            i.delay = bit(w, 5);
            let c = (w >> 6) & 0x3F;
            i.src1 = src_reg(breg32(w));
            i.src2 = if bit(w, 4) { Src::Imm(c) } else { src_reg(c) };
            i.op = match w & 0xF {
                0x0 => Op::Br(BrCond::Eq),
                0x1 => Op::Br(BrCond::Ne),
                0x2 => Op::Br(BrCond::Lt),
                0x3 => Op::Br(BrCond::Ge),
                0x4 => Op::Br(BrCond::Lo),
                0x5 => Op::Br(BrCond::Hs),
                0xE => Op::Bbit0,
                0xF => Op::Bbit1,
                _ => Op::Illegal(why!("BRcc: reserved sub-opcode")),
            };
            // The PRM defines the long-immediate forms only without .d.
            if i.delay && (matches!(i.src1, Src::Limm(_)) || matches!(i.src2, Src::Limm(_))) {
                i.op = Op::Illegal(why!("BRcc/BBIT.d with long immediate"));
            }
        }
        0x02 => {
            let s9 = sext(((w >> 16) & 0xFF) | (((w >> 15) & 1) << 8), 9);
            let b = breg32(w);
            let a = w & 0x3F;
            i.op = Op::Ld;
            i.mem = mem_mode((w >> 7) & 3, bit(w, 6), (w >> 9) & 3, bit(w, 11));
            i.src1 = src_reg(b);
            i.src2 = Src::Imm(s9);
            i.dst = dst_reg(a);
            check_load(i, (w >> 7) & 3, bit(w, 6), (w >> 9) & 3, b, a);
        }
        0x03 => {
            let s9 = sext(((w >> 16) & 0xFF) | (((w >> 15) & 1) << 8), 9);
            let b = breg32(w);
            let zz = (w >> 1) & 3;
            let aa = (w >> 3) & 3;
            i.op = Op::St;
            i.mem = mem_mode(zz, false, aa, bit(w, 5));
            i.src1 = src_reg(b);
            i.src2 = Src::Imm(s9);
            i.data = src_reg((w >> 6) & 0x3F);
            if zz == 3 {
                i.op = Op::Illegal(why!("ST: reserved data size"));
            } else if zz == 1 && aa == 3 {
                i.op = Op::Illegal(why!("STB.AS: undefined (PRM Table 52)"));
            } else if b == 62 && (aa == 1 || aa == 2) {
                i.op = Op::Illegal(why!("ST: address write-back with long-immediate base"));
            }
        }
        0x04 => decode_04(i, w),
        0x05 => decode_05(i, w),
        _ => {
            // 0x06-0x08 user/ARC extensions, 0x09-0x0B market-specific. Their
            // operand layout follows the general format (PRM ch. 6), which
            // also determines whether a long immediate follows.
            let sub = ((w >> 16) & 0x3F) as u8;
            general_operands(i, w);
            i.op = Op::Extension { major, sub };
        }
    }
}

fn mem_mode(zz: u32, x: bool, aa: u32, di: bool) -> MemMode {
    let size = match zz {
        1 => Size::Byte,
        2 => Size::Half,
        _ => Size::Word,
    };
    let wb = match aa {
        1 => WriteBack::Pre,
        2 => WriteBack::Post,
        3 => WriteBack::Scaled,
        _ => WriteBack::None,
    };
    MemMode { size, sign_extend: x, wb, bypass_cache: di }
}

/// Load restrictions (PRM "Load Register with Offset"/"Load Register-Register").
fn check_load(i: &mut Instr, zz: u32, x: bool, aa: u32, b: u32, a: u32) {
    if zz == 3 {
        i.op = Op::Illegal(why!("LD: reserved data size"));
    } else if x && zz == 0 {
        i.op = Op::Illegal(why!("LD: sign extension of a long word"));
    } else if b == 62 && (aa == 1 || aa == 2) {
        i.op = Op::Illegal(why!("LD: address write-back with long-immediate base"));
    } else if a == 60 || a == 61 || a == 63 {
        i.op = Op::Illegal(why!("LD: destination is LP_COUNT, r61 or PCL"));
    } else if zz == 1 && aa == 3 {
        i.op = Op::Illegal(why!("LDB.AS: undefined (PRM Table 52)"));
    } else if a == 62 && (zz == 1 || x || i.mem.bypass_cache) {
        // PREFETCH is LD with a = limm; byte sizes are "invalid and a reserved
        // format" and the syntax has no .di/.x (PRM, PREFETCH).
        i.op = Op::Illegal(why!("PREFETCH: reserved variant"));
    }
}

/// Operand fields for the general formats (PRM "General Operations" P[1:0]).
/// Returns the second-source operand; sets dst/src1/cc as the format implies.
fn general_operands(i: &mut Instr, w: u32) {
    let b = breg32(w);
    let c = (w >> 6) & 0x3F;
    let a = w & 0x3F;
    match (w >> 22) & 3 {
        0 => {
            i.dst = dst_reg(a);
            i.src1 = src_reg(b);
            i.src2 = src_reg(c);
        }
        1 => {
            i.dst = dst_reg(a);
            i.src1 = src_reg(b);
            i.src2 = Src::Imm(c);
        }
        2 => {
            i.dst = dst_reg(b);
            i.src1 = src_reg(b);
            i.src2 = Src::Imm(sext(c | (a << 6), 12));
        }
        _ => {
            i.dst = dst_reg(b);
            i.src1 = src_reg(b);
            i.src2 = if bit(w, 5) { Src::Imm(c) } else { src_reg(c) };
            i.cc = (w & 0x1F) as u8;
        }
    }
}

/// Second operand only (formats where the B field is a destination or unused).
fn second_operand(i: &mut Instr, w: u32) -> Src {
    let c = (w >> 6) & 0x3F;
    match (w >> 22) & 3 {
        0 => src_reg(c),
        1 => Src::Imm(c),
        2 => Src::Imm(sext(c | ((w & 0x3F) << 6), 12)),
        _ => {
            i.cc = (w & 0x1F) as u8;
            if bit(w, 5) { Src::Imm(c) } else { src_reg(c) }
        }
    }
}

fn decode_04(i: &mut Instr, w: u32) {
    let sub = (w >> 16) & 0x3F;
    let f = bit(w, 15);
    let b = breg32(w);
    let p = (w >> 22) & 3;
    let dual = |op| (op, true);
    let (op, is_dual) = match sub {
        0x00 => dual(Op::Add),
        0x01 => dual(Op::Adc),
        0x02 => dual(Op::Sub),
        0x03 => dual(Op::Sbc),
        0x04 => dual(Op::And),
        0x05 => dual(Op::Or),
        0x06 => dual(Op::Bic),
        0x07 => dual(Op::Xor),
        0x08 => dual(Op::Max),
        0x09 => dual(Op::Min),
        0x0E => dual(Op::Rsub),
        0x0F => dual(Op::Bset),
        0x10 => dual(Op::Bclr),
        0x12 => dual(Op::Bxor),
        0x13 => dual(Op::Bmsk),
        0x14 => dual(Op::Add1),
        0x15 => dual(Op::Add2),
        0x16 => dual(Op::Add3),
        0x17 => dual(Op::Sub1),
        0x18 => dual(Op::Sub2),
        0x19 => dual(Op::Sub3),
        0x1A => dual(Op::Arc700Only(why!("mpy"))),
        0x1B => dual(Op::Arc700Only(why!("mpyh"))),
        0x1C => dual(Op::Arc700Only(why!("mpyhu"))),
        0x1D => dual(Op::Arc700Only(why!("mpyu"))),
        _ => (Op::Nop, false),
    };
    if is_dual {
        general_operands(i, w);
        i.op = op;
        i.set_flags = f;
        return;
    }
    if f && matches!(sub, 0x28..=0x2B) {
        // LP, FLAG, LR and SR: "F is always encoded as 0" (PRM).
        general_operands(i, w);
        i.op = Op::Illegal(why!("LP/FLAG/LR/SR with F=1"));
        return;
    }
    match sub {
        // MOV b,c: destination is B; A is ignored (PRM "Move and Compare").
        0x0A => {
            i.op = Op::Mov;
            i.src2 = second_operand(i, w);
            i.dst = dst_reg(b);
            i.set_flags = f;
        }
        // TST/CMP/RCMP/BTST b,c: no destination, flags always updated.
        0x0B | 0x0C | 0x0D | 0x11 => {
            i.op = match sub {
                0x0B => Op::Tst,
                0x0C => Op::Cmp,
                0x0D => Op::Rcmp,
                _ => Op::Btst,
            };
            i.src1 = src_reg(b);
            i.src2 = second_operand(i, w);
            i.set_flags = true;
            if !f {
                // "The flag setting field, F, is always encoded as 1 for this
                // instruction" (PRM, CMP/TST/RCMP/BTST); F=0 is unspecified.
                i.op = Op::Illegal(why!("CMP/TST/RCMP/BTST with F=0"));
            }
        }
        0x20..=0x23 => {
            i.op = if sub & 2 == 0 { Op::J } else { Op::Jl };
            i.delay = sub & 1 != 0;
            i.src2 = second_operand(i, w);
            i.set_flags = f;
            let ilink = matches!(i.src2, Src::Reg(29 | 30));
            if i.delay && matches!(i.src2, Src::Limm(_)) {
                // The PRM lists Jcc/JLcc limm only without .D.
                i.op = Op::Illegal(why!("Jcc.D/JLcc.D with long immediate"));
            } else if f && i.op == Op::Jl {
                // Only Jcc.F [ILINKn] is defined; JLcc has no .F form.
                i.op = Op::Illegal(why!("JLcc.F"));
            } else if f && !ilink {
                i.op = Op::Illegal(why!("Jcc.F without ILINK1/ILINK2"));
            } else if ilink && !f {
                i.op = Op::Illegal(why!("Jcc [ILINKn] without .F"));
            }
        }
        0x28 => {
            i.op = Op::Lp;
            let c = (w >> 6) & 0x3F;
            match p {
                2 => i.target = pcl(i.addr).wrapping_add(sext(c | ((w & 0x3F) << 6), 12) << 1),
                3 if bit(w, 5) => {
                    i.cc = (w & 0x1F) as u8;
                    i.target = pcl(i.addr).wrapping_add(c << 1);
                }
                // "Encoding the operand mode to be 0x0 or 0x1 is not
                // recommended. Additionally using operand mode 0x3 with
                // sub-operand mode 0x0 is not recommended." (PRM, LPcc)
                _ => i.op = Op::Illegal(why!("LP: operand mode not recommended (undefined)")),
            }
        }
        0x29 => {
            i.op = Op::Flag;
            i.src2 = second_operand(i, w);
        }
        0x2A | 0x2B => {
            if p == 3 {
                i.op = Op::Illegal(why!("LR/SR: conditional operand mode undefined on ARCtangent-A5"));
                i.src2 = second_operand(i, w);
                return;
            }
            i.src2 = second_operand(i, w);
            if sub == 0x2A {
                i.op = Op::Lr;
                i.dst = dst_reg(b);
            } else {
                i.op = Op::Sr;
                i.src1 = src_reg(b);
            }
        }
        0x2F => decode_sop_04(i, w),
        0x30..=0x37 => {
            let a = w & 0x3F;
            let aa = (w >> 22) & 3;
            let zz = (w >> 17) & 3;
            let x = bit(w, 16);
            i.op = Op::Ld;
            i.mem = mem_mode(zz, x, aa, bit(w, 15));
            i.dst = dst_reg(a);
            i.src1 = src_reg(b);
            i.src2 = src_reg((w >> 6) & 0x3F);
            check_load(i, zz, x, aa, b, a);
            if a == 62 && matches!(i.op, Op::Ld) {
                // PRM: "Using an immediate value in the destination register
                // field is not allowed for the ARCtangent-A5 or ARC 600."
                i.op = Op::Illegal(why!("LD reg-reg: immediate destination not allowed on ARCtangent-A5"));
            }
        }
        _ => {
            general_operands(i, w);
            i.op = Op::Illegal(why!("major 0x04: reserved sub-opcode"));
        }
    }
}

fn decode_sop_04(i: &mut Instr, w: u32) {
    let p = (w >> 22) & 3;
    let b = breg32(w);
    let c = (w >> 6) & 0x3F;
    i.src2 = if p == 0 { src_reg(c) } else { Src::Imm(c) };
    if p >= 2 {
        i.op = Op::Illegal(why!("single-operand instruction with s12/conditional format"));
        return;
    }
    let sub2 = w & 0x3F;
    if sub2 == 0x3F {
        // Zero-operand group: sub-opcode is the B field.
        i.op = match b {
            0x01 => Op::Sleep,
            0x02 => Op::Swi,
            0x03 => Op::Arc700Only(why!("sync")),
            0x04 => Op::Arc700Only(why!("rtie")),
            0x05 => Op::Brk,
            _ => Op::Illegal(why!("zero-operand: reserved sub-opcode")),
        };
        return;
    }
    i.dst = dst_reg(b);
    i.set_flags = bit(w, 15);
    i.op = match sub2 {
        0x00 => Op::Asl,
        0x01 => Op::Asr,
        0x02 => Op::Lsr,
        0x03 => Op::Ror,
        0x04 => Op::Rrc,
        0x05 => Op::Sexb,
        0x06 => Op::Sexw,
        0x07 => Op::Extb,
        0x08 => Op::Extw,
        0x09 => Op::Abs,
        0x0A => Op::Not,
        0x0B => Op::Rlc,
        0x0C => Op::Arc700Only(why!("ex")),
        _ => Op::Illegal(why!("single-operand: reserved sub-opcode")),
    };
}

fn decode_05(i: &mut Instr, w: u32) {
    let sub = ((w >> 16) & 0x3F) as u8;
    let f = bit(w, 15);
    let op = match sub {
        0x00 => Op::AslM,
        0x01 => Op::LsrM,
        0x02 => Op::AsrM,
        0x03 => Op::RorM,
        0x04 => Op::Mul64,
        0x05 => Op::Mulu64,
        0x06 => Op::Adds,
        0x07 => Op::Subs,
        0x08 => Op::Divaw,
        0x0A => Op::Asls,
        0x0B => Op::Asrs,
        0x0C => Op::Muldw,
        0x10 => Op::Macdw,
        0x14 => Op::Msubdw,
        0x28 => Op::Addsdw,
        0x29 => Op::Subsdw,
        0x2F => {
            let p = (w >> 22) & 3;
            let c = (w >> 6) & 0x3F;
            i.src2 = if p == 0 { src_reg(c) } else { Src::Imm(c) };
            i.dst = dst_reg(breg32(w));
            i.set_flags = f;
            i.op = match w & 0x3F {
                _ if p >= 2 => Op::Illegal(why!("single-operand extension with s12/conditional format")),
                0x00 => Op::Swap,
                0x01 => Op::Norm,
                0x02 => Op::Sat16,
                0x03 => Op::Rnd16,
                0x04 => Op::Abssw,
                0x05 => Op::Abss,
                0x06 => Op::Negsw,
                0x07 => Op::Negs,
                0x08 => Op::Normw,
                0x3F => {
                    i.dst = Dst::None;
                    Op::Illegal(why!("extension zero-operand: reserved"))
                }
                sub2 => Op::Extension { major: 0x05, sub: 0x80 | sub2 as u8 },
            };
            return;
        }
        _ => Op::Extension { major: 0x05, sub },
    };
    general_operands(i, w);
    i.op = op;
    i.set_flags = f;
    if matches!(op, Op::Mul64 | Op::Mulu64) {
        // "MUL64 <0,>b,c": results go to MLO/MMID/MHI; A is ignored and the
        // flags are unchanged (PRM, MUL64).
        i.dst = Dst::None;
        i.set_flags = false;
        // The documented encodings put 62 ("<0,>") in the A field, but "the
        // destination is always ignored for the multiply instruction" (PRM,
        // MUL64), so any A value is accepted; binutils rejects A != 62.
        // Firmware evidence: US v2.1 at 0x4007_B49C executes 0x2B04_0F83
        // (A = 3, b = r3, c = limm) and then reads MMID (MAME ignores A too).
        if f {
            i.op = Op::Illegal(why!("MUL64/MULU64 with F=1"));
        }
    }
}

// ---------------------------------------------------------------------------
// 16-bit formats

fn decode16(i: &mut Instr, h: u16) {
    let h = h as u32;
    let b = r3(h >> 8);
    let c = r3(h >> 5);
    let a = r3(h);
    let reg = |r: u8| Src::Reg(r);
    let set = |i: &mut Instr, op: Op, dst: u8, s1: Src, s2: Src| {
        i.op = op;
        i.dst = Dst::Reg(dst);
        i.src1 = s1;
        i.src2 = s2;
    };
    let mem = |i: &mut Instr, op: Op, size: Size, sign: bool, base: Src, off: u32| {
        i.op = op;
        i.mem = MemMode { size, sign_extend: sign, wb: WriteBack::None, bypass_cache: false };
        i.src1 = base;
        i.src2 = Src::Imm(off);
    };
    match h >> 11 {
        0x0C => match (h >> 3) & 3 {
            0 => {
                mem(i, Op::Ld, Size::Word, false, reg(b), 0);
                i.src2 = reg(c);
                i.dst = Dst::Reg(a);
            }
            1 => {
                mem(i, Op::Ld, Size::Byte, false, reg(b), 0);
                i.src2 = reg(c);
                i.dst = Dst::Reg(a);
            }
            2 => {
                mem(i, Op::Ld, Size::Half, false, reg(b), 0);
                i.src2 = reg(c);
                i.dst = Dst::Reg(a);
            }
            _ => set(i, Op::Add, a, reg(b), reg(c)),
        },
        0x0D => {
            let op = [Op::Add, Op::Sub, Op::AslM, Op::AsrM][((h >> 3) & 3) as usize];
            set(i, op, c, reg(b), Src::Imm(h & 7));
        }
        0x0E => {
            let hreg = ((h >> 5) & 7) | ((h & 7) << 3);
            match (h >> 3) & 3 {
                0 => set(i, Op::Add, b, reg(b), src_reg(hreg)),
                1 => {
                    i.op = Op::Mov;
                    i.dst = Dst::Reg(b);
                    i.src2 = src_reg(hreg);
                }
                2 => {
                    i.op = Op::Cmp;
                    i.src1 = reg(b);
                    i.src2 = src_reg(hreg);
                    i.set_flags = true;
                }
                _ => {
                    i.op = Op::Mov;
                    i.dst = dst_reg(hreg);
                    i.src2 = reg(b);
                }
            }
        }
        0x0F => decode_0f(i, h, b, c),
        0x10..=0x16 => {
            let u5 = h & 0x1F;
            let (op, size, sign, scale) = match h >> 11 {
                0x10 => (Op::Ld, Size::Word, false, 2),
                0x11 => (Op::Ld, Size::Byte, false, 0),
                0x12 => (Op::Ld, Size::Half, false, 1),
                0x13 => (Op::Ld, Size::Half, true, 1),
                0x14 => (Op::St, Size::Word, false, 2),
                0x15 => (Op::St, Size::Byte, false, 0),
                _ => (Op::St, Size::Half, false, 1),
            };
            mem(i, op, size, sign, reg(b), u5 << scale);
            if op == Op::Ld {
                i.dst = Dst::Reg(c);
            } else {
                i.data = reg(c);
            }
        }
        0x17 => {
            let u5 = h & 0x1F;
            let op = [Op::AslM, Op::LsrM, Op::AsrM, Op::Sub, Op::Bset, Op::Bclr, Op::Bmsk, Op::Btst]
                [((h >> 5) & 7) as usize];
            if op == Op::Btst {
                i.op = op;
                i.src1 = reg(b);
                i.src2 = Src::Imm(u5);
                i.set_flags = true;
            } else {
                set(i, op, b, reg(b), Src::Imm(u5));
            }
        }
        0x18 => {
            let u7 = (h & 0x1F) << 2;
            let sp = reg(28);
            match (h >> 5) & 7 {
                0 => {
                    mem(i, Op::Ld, Size::Word, false, sp, u7);
                    i.dst = Dst::Reg(b);
                }
                1 => {
                    mem(i, Op::Ld, Size::Byte, false, sp, u7);
                    i.dst = Dst::Reg(b);
                }
                2 => {
                    mem(i, Op::St, Size::Word, false, sp, u7);
                    i.data = reg(b);
                }
                3 => {
                    mem(i, Op::St, Size::Byte, false, sp, u7);
                    i.data = reg(b);
                }
                4 => set(i, Op::Add, b, sp, Src::Imm(u7)),
                5 => match (h >> 8) & 7 {
                    0 => set(i, Op::Add, 28, sp, Src::Imm(u7)),
                    1 => set(i, Op::Sub, 28, sp, Src::Imm(u7)),
                    _ => i.op = Op::Illegal(why!("ADD_S/SUB_S sp: reserved sub-opcode")),
                },
                6 => match h & 0x1F {
                    0x01 => {
                        i.op = Op::Pop;
                        i.dst = Dst::Reg(b);
                    }
                    0x11 => {
                        i.op = Op::Pop;
                        i.dst = Dst::Reg(31);
                    }
                    _ => i.op = Op::Illegal(why!("POP_S: reserved sub-opcode")),
                },
                _ => match h & 0x1F {
                    0x01 => {
                        i.op = Op::Push;
                        i.data = reg(b);
                    }
                    0x11 => {
                        i.op = Op::Push;
                        i.data = reg(31);
                    }
                    _ => i.op = Op::Illegal(why!("PUSH_S: reserved sub-opcode")),
                },
            }
        }
        0x19 => {
            let s9 = sext(h & 0x1FF, 9);
            let gp = reg(26);
            match (h >> 9) & 3 {
                0 => {
                    mem(i, Op::Ld, Size::Word, false, gp, s9 << 2);
                    i.dst = Dst::Reg(0);
                }
                1 => {
                    mem(i, Op::Ld, Size::Byte, false, gp, s9);
                    i.dst = Dst::Reg(0);
                }
                2 => {
                    mem(i, Op::Ld, Size::Half, false, gp, s9 << 1);
                    i.dst = Dst::Reg(0);
                }
                _ => set(i, Op::Add, 0, gp, Src::Imm(s9 << 2)),
            }
        }
        0x1A => {
            mem(i, Op::Ld, Size::Word, false, reg(63), (h & 0xFF) << 2);
            i.dst = Dst::Reg(b);
        }
        0x1B => {
            i.op = Op::Mov;
            i.dst = Dst::Reg(b);
            i.src2 = Src::Imm(h & 0xFF);
        }
        0x1C => {
            let u7 = h & 0x7F;
            if bit(h, 7) {
                i.op = Op::Cmp;
                i.src1 = reg(b);
                i.src2 = Src::Imm(u7);
                i.set_flags = true;
            } else {
                set(i, Op::Add, b, reg(b), Src::Imm(u7));
            }
        }
        0x1D => {
            i.op = Op::Br(if bit(h, 7) { BrCond::Ne } else { BrCond::Eq });
            i.src1 = reg(b);
            i.src2 = Src::Imm(0);
            i.target = pcl(i.addr).wrapping_add(sext(h & 0x7F, 7) << 1);
        }
        0x1E => {
            i.op = Op::B;
            match (h >> 9) & 3 {
                3 => {
                    const CC: [Cond; 8] = [COND_GT, COND_GE, COND_LT, COND_LE, COND_HI, COND_CC, COND_CS, COND_LS];
                    i.cc = CC[((h >> 6) & 7) as usize];
                    i.target = pcl(i.addr).wrapping_add(sext(h & 0x3F, 6) << 1);
                }
                k => {
                    i.cc = [COND_AL, COND_EQ, COND_NE][k as usize];
                    i.target = pcl(i.addr).wrapping_add(sext(h & 0x1FF, 9) << 1);
                }
            }
        }
        _ => {
            i.op = Op::Bl;
            i.target = pcl(i.addr).wrapping_add(sext(h & 0x7FF, 11) << 2);
        }
    }
}

fn decode_0f(i: &mut Instr, h: u32, b: u8, c: u8) {
    let reg = |r: u8| Src::Reg(r);
    let bbc = |i: &mut Instr, op: Op| {
        i.op = op;
        i.dst = Dst::Reg(b);
        i.src1 = reg(b);
        i.src2 = reg(c);
    };
    let bc = |i: &mut Instr, op: Op| {
        i.op = op;
        i.dst = Dst::Reg(b);
        i.src2 = reg(c);
    };
    match h & 0x1F {
        0x00 => match (h >> 5) & 7 {
            k @ 0..=3 => {
                i.op = if k & 2 == 0 { Op::J } else { Op::Jl };
                i.delay = k & 1 != 0;
                i.src2 = reg(b);
            }
            6 => {
                bbc(i, Op::Sub);
                i.src2 = reg(b);
                i.cc = COND_NE;
            }
            7 => {
                let blink = reg(31);
                match (h >> 8) & 7 {
                    0 => i.op = Op::Nop,
                    1 => i.op = Op::Unimp,
                    4 => {
                        i.op = Op::J;
                        i.cc = COND_EQ;
                        i.src2 = blink;
                    }
                    5 => {
                        i.op = Op::J;
                        i.cc = COND_NE;
                        i.src2 = blink;
                    }
                    6 => {
                        i.op = Op::J;
                        i.src2 = blink;
                    }
                    7 => {
                        i.op = Op::J;
                        i.delay = true;
                        i.src2 = blink;
                    }
                    _ => i.op = Op::Illegal(why!("16-bit zero-operand: reserved sub-opcode")),
                }
            }
            _ => i.op = Op::Illegal(why!("16-bit single-operand: reserved sub-opcode")),
        },
        0x02 => bbc(i, Op::Sub),
        0x04 => bbc(i, Op::And),
        0x05 => bbc(i, Op::Or),
        0x06 => bbc(i, Op::Bic),
        0x07 => bbc(i, Op::Xor),
        0x0B => {
            i.op = Op::Tst;
            i.src1 = reg(b);
            i.src2 = reg(c);
            i.set_flags = true;
        }
        0x0C => {
            i.op = Op::Mul64;
            i.src1 = reg(b);
            i.src2 = reg(c);
        }
        0x0D => bc(i, Op::Sexb),
        0x0E => bc(i, Op::Sexw),
        0x0F => bc(i, Op::Extb),
        0x10 => bc(i, Op::Extw),
        0x11 => bc(i, Op::Abs),
        0x12 => bc(i, Op::Not),
        0x13 => bc(i, Op::Neg),
        0x14 => bbc(i, Op::Add1),
        0x15 => bbc(i, Op::Add2),
        0x16 => bbc(i, Op::Add3),
        0x18 => bbc(i, Op::AslM),
        0x19 => bbc(i, Op::LsrM),
        0x1A => bbc(i, Op::AsrM),
        0x1B => bc(i, Op::Asl),
        0x1C => bc(i, Op::Asr),
        0x1D => bc(i, Op::Lsr),
        0x1E => i.op = Op::Arc700Only(why!("trap_s")),
        0x1F if h == 0x7FFF => i.op = Op::Brk,
        0x1F => i.op = Op::Illegal(why!("BRK_S: encoding must be 0x7FFF")),
        _ => i.op = Op::Illegal(why!("16-bit general: reserved sub-opcode")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Decodes from a little-endian byte image placed at `base`.
    fn dec(base: u32, bytes: &[u8]) -> Instr {
        decode_with(base, |a| {
            let o = (a - base) as usize;
            u16::from_le_bytes([bytes.get(o).copied().unwrap_or(0), bytes.get(o + 1).copied().unwrap_or(0)])
        })
    }

    /// Halfwords as GNU objdump prints them, in memory order.
    fn hw(halves: &[u16]) -> Vec<u8> {
        halves.iter().flat_map(|h| h.to_le_bytes()).collect()
    }

    #[test]
    fn major_opcode_selects_width() {
        for major in 0u8..0x20 {
            let expected = if major <= 0x0B { Width::Long } else { Width::Short };
            assert_eq!(width_of(major_opcode((major as u16) << 11)), expected, "major {major:#x}");
        }
    }

    // Reference encodings below are taken verbatim from GNU objdump
    // (ARCompact toolchain 2.45.1, -m ARC600) disassembling the v1.5 BIOS
    // vector table and reset handler.

    #[test]
    fn mul64_ignores_the_a_field() {
        // US v2.1 BIOS 0x4007b49c: 2b04 0f83 0001 66e9 (objdump: .word; A = 3).
        let i = dec(0x4007_b49c, &hw(&[0x2b04, 0x0f83, 0x0001, 0x66e9]));
        assert_eq!((i.op, i.dst, i.src1, i.src2, i.len), (Op::Mul64, Dst::None, Src::Reg(3), Src::Limm(0x166e9), 8));
        assert!(!i.set_flags);
        // F = 1 stays illegal (no evidence either way).
        let i = dec(0, &hw(&[0x2b04, 0x8f83, 0x0001, 0x66e9]));
        assert!(matches!(i.op, Op::Illegal(_)), "{:?}", i.op);
    }

    #[test]
    fn jump_with_long_immediate() {
        // 40000000: 2020 0f80 4002 b5f4    j 0x4002b5f4
        let i = dec(0x4000_0000, &hw(&[0x2020, 0x0f80, 0x4002, 0xb5f4]));
        assert_eq!(i.op, Op::J);
        assert_eq!(i.len, 8);
        assert_eq!(i.src2, Src::Limm(0x4002_b5f4));
    }

    #[test]
    fn mov_limm_and_branch_link_delay() {
        // 4002b5f4: 240a 3f80 0300 0800    mov sp,0x3000800
        let i = dec(0x4002_b5f4, &hw(&[0x240a, 0x3f80, 0x0300, 0x0800]));
        assert_eq!((i.op, i.dst, i.src2, i.len), (Op::Mov, Dst::Reg(28), Src::Limm(0x0300_0800), 8));
        // 4002b5fc: 085e 0020              bl.d 92 ;0x4002b658
        let i = dec(0x4002_b5fc, &hw(&[0x085e, 0x0020]));
        assert_eq!((i.op, i.delay, i.target, i.cc), (Op::Bl, true, 0x4002_b658, COND_AL));
        // 4002b608: 0005 0000              b 4 ;0x4002b60c
        let i = dec(0x4002_b608, &hw(&[0x0005, 0x0000]));
        assert_eq!((i.op, i.delay, i.target), (Op::B, false, 0x4002_b60c));
        // 4002b630: 0001 0000              b 0 ;0x4002b630
        assert_eq!(dec(0x4002_b630, &hw(&[0x0001, 0x0000])).target, 0x4002_b630);
    }

    #[test]
    fn loads_stores_with_limm_address() {
        // 4002b61c: 1600 7800 0180 b000    ld.di r0,[0x180b000]
        let i = dec(0x4002_b61c, &hw(&[0x1600, 0x7800, 0x0180, 0xb000]));
        assert_eq!(i.op, Op::Ld);
        assert!(i.mem.bypass_cache);
        assert_eq!((i.dst, i.src1, i.src2), (Dst::Reg(0), Src::Limm(0x0180_b000), Src::Imm(0)));
        // 4002b628: 1e00 7020 0180 b000    st.di r0,[0x180b000]
        let i = dec(0x4002_b628, &hw(&[0x1e00, 0x7020, 0x0180, 0xb000]));
        assert_eq!(i.op, Op::St);
        assert_eq!((i.data, i.src1), (Src::Reg(0), Src::Limm(0x0180_b000)));
        // 4002b66c: 1ec0 1040              st r1,[r14,192]
        let i = dec(0x4002_b66c, &hw(&[0x1ec0, 0x1040]));
        assert_eq!((i.data, i.src1, i.src2), (Src::Reg(1), Src::Reg(14), Src::Imm(192)));
    }

    #[test]
    fn sixteen_bit_forms() {
        // 4002b658: c5e1 push_s r13 ; c0f1 push_s blink
        let i = dec(0, &hw(&[0xc5e1]));
        assert_eq!((i.op, i.data, i.len), (Op::Push, Src::Reg(13), 2));
        assert_eq!(dec(0, &hw(&[0xc0f1])).data, Src::Reg(31));
        // 4002b660: 71cf 4002 b754         mov_s r1,0x4002b754
        let i = dec(0x4002_b660, &hw(&[0x71cf, 0x4002, 0xb754]));
        assert_eq!((i.op, i.dst, i.src2, i.len), (Op::Mov, Dst::Reg(1), Src::Limm(0x4002_b754), 6));
        // 4002b67c: 8000 ld_s r0,[r0,0] ; 8aa0 ldb_s r13,[r2,0]
        let i = dec(0, &hw(&[0x8aa0]));
        assert_eq!((i.op, i.mem.size, i.dst, i.src1), (Op::Ld, Size::Byte, Dst::Reg(13), Src::Reg(2)));
        // 40076952: 782c mul64_s 0,r0,r1
        let i = dec(0, &hw(&[0x782c]));
        assert_eq!((i.op, i.src1, i.src2), (Op::Mul64, Src::Reg(0), Src::Reg(1)));
        // 400011b4: e83f breq_s r0,0,126 ;0x40001232
        let i = dec(0x4000_11b4, &hw(&[0xe83f]));
        assert_eq!((i.op, i.target), (Op::Br(BrCond::Eq), 0x4000_1232));
        // 400011f6: f5ca bne_s -108 ;0x40001188
        let i = dec(0x4000_11f6, &hw(&[0xf5ca]));
        assert_eq!((i.op, i.cc, i.target), (Op::B, COND_NE, 0x4000_1188));
        // 400007f0 <- 40000838: ffee bl_s -72
        assert_eq!(dec(0x4000_0838, &hw(&[0xffee])).target, 0x4000_07f0);
        // 400008fe: 7fff brk_s
        assert_eq!(dec(0, &hw(&[0x7fff])).op, Op::Brk);
    }

    #[test]
    fn compare_and_branch_and_bit_test() {
        // 40000e16: 08cb 7571 brne.d r56,0x15,202 ;0x40000ede
        let i = dec(0x4000_0e16, &hw(&[0x08cb, 0x7571]));
        assert_eq!(
            (i.op, i.delay, i.src1, i.src2, i.target),
            (Op::Br(BrCond::Ne), true, Src::Reg(56), Src::Imm(0x15), 0x4000_0ede)
        );
        // 40001112: 0e39 0c1e bbit0 r6,0x30,56 ;0x40001148
        let i = dec(0x4000_1112, &hw(&[0x0e39, 0x0c1e]));
        assert_eq!((i.op, i.src1, i.src2, i.target), (Op::Bbit0, Src::Reg(6), Src::Imm(0x30), 0x4000_1148));
    }

    #[test]
    fn loop_jump_ilink_and_zero_operand() {
        // 40084c14: 20e8 04a2 lpne 0x24 ;0x40084c38
        let i = dec(0x4008_4c14, &hw(&[0x20e8, 0x04a2]));
        assert_eq!((i.op, i.cc, i.target), (Op::Lp, COND_NE, 0x4008_4c38));
        // 40073e96: 2020 8780 j.f [ilink2]
        let i = dec(0, &hw(&[0x2020, 0x8780]));
        assert_eq!((i.op, i.set_flags, i.src2), (Op::J, true, Src::Reg(30)));
        // 40073d5c: 216f 003f sleep 0
        assert_eq!(dec(0, &hw(&[0x216f, 0x003f])).op, Op::Sleep);
        // 40089af8: 264a 7000 nop (mov 0,0)
        let i = dec(0, &hw(&[0x264a, 0x7000]));
        assert_eq!((i.op, i.dst, i.src2), (Op::Mov, Dst::Discard, Src::Imm(0)));
    }

    #[test]
    fn illegal_encodings_are_reported_not_dropped() {
        // major 0x04 sub-opcode 0x1E is reserved.
        let w: u32 = (0x04 << 27) | (0x1E << 16);
        let i = dec(0, &hw(&[(w >> 16) as u16, w as u16]));
        assert!(matches!(i.op, Op::Illegal(_)));
        // ARC 700-only MPY decodes as such.
        let w: u32 = (0x04 << 27) | (0x1A << 16);
        assert_eq!(dec(0, &hw(&[(w >> 16) as u16, w as u16])).op, Op::Arc700Only(why!("mpy")));
    }
}
