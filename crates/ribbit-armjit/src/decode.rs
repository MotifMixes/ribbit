//! ARM instructions as the generator sees them, told apart exactly as
//! `arm::execute` does. What it has no native code for is `Exec`: the
//! interpreter runs that one instruction from inside the block.

/// A shifter operand or an addressing offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operand {
    /// A value, and the shifter carry out when the rotation is not 0.
    Imm(u32, Option<bool>),
    /// A register shifted by an immediate (kind: LSL, LSR, ASR, ROR/RRX).
    Reg { rm: u32, kind: u32, amount: u32 },
    /// A register shifted by the bottom byte of another (neither is r15).
    RegShift { rm: u32, kind: u32, rs: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instr {
    /// Data processing. `rd` 15 (without S) branches and ends the block.
    Alu { cond: u32, op: u32, s: bool, rn: u32, rd: u32, op2: Operand },
    /// LDR/STR/LDRB/STRB (not the T forms). A load of r15 ends the block.
    Mem { cond: u32, load: bool, byte: bool, pre: bool, up: bool, writeback: bool, rn: u32, rd: u32, offset: Operand },
    /// LDRH/STRH (`sh` 1), LDRSB (2), LDRSH (3); not loads of r15.
    Half { cond: u32, load: bool, sh: u32, pre: bool, up: bool, writeback: bool, rn: u32, rd: u32, offset: Operand },
    /// LDM/STM without the S bit, `list` not empty, `rn` not r15. A load
    /// of r15 ends the block.
    Multiple { cond: u32, load: bool, pre: bool, up: bool, writeback: bool, rn: u32, list: u32 },
    /// MUL/MLA (none of the registers r15).
    Mul { cond: u32, acc: bool, s: bool, rd: u32, rn: u32, rs: u32, rm: u32 },
    /// UMULL/UMLAL/SMULL/SMLAL (none of the registers r15).
    MulLong { cond: u32, signed: bool, acc: bool, s: bool, hi: u32, lo: u32, rs: u32, rm: u32 },
    /// CLZ (`rd` not r15).
    Clz { cond: u32, rd: u32, rm: u32 },
    /// MRS of the CPSR (`rd` not r15).
    Mrs { cond: u32, rd: u32 },
    /// MSR to the CPSR: `mask` from the field bits (T left out), `src` an
    /// immediate or a register (not r15).
    Msr { cond: u32, mask: u32, src: Operand },
    /// BX/BLX (register; `rm` not r15). Ends the block.
    Bx { cond: u32, link: bool, rm: u32 },
    /// SMLA<x><y> (`acc`) and SMUL<x><y>: `x`/`y` take the top halves.
    Smul { cond: u32, acc: bool, x: bool, y: bool, rd: u32, rn: u32, rs: u32, rm: u32 },
    /// PLD: nothing to do.
    Nop,
    /// B/BL (ends the block).
    Branch { cond: u32, link: bool, target: u32 },
    /// Anything else, run by `Arm::execute_arm`. The block goes on after it
    /// unless it changed r15, the mode, T, I or F, or wrote compiled code;
    /// `ends` (coprocessor and unconditional instructions, SWI) ends it anyway.
    Exec { cond: u32, w: u32, ends: bool },
}

impl Instr {
    pub(crate) fn cond(&self) -> u32 {
        match *self {
            Instr::Alu { cond, .. }
            | Instr::Mem { cond, .. }
            | Instr::Multiple { cond, .. }
            | Instr::Half { cond, .. }
            | Instr::Mul { cond, .. }
            | Instr::MulLong { cond, .. }
            | Instr::Clz { cond, .. }
            | Instr::Mrs { cond, .. }
            | Instr::Msr { cond, .. }
            | Instr::Bx { cond, .. }
            | Instr::Smul { cond, .. }
            | Instr::Branch { cond, .. }
            | Instr::Exec { cond, .. } => cond,
            Instr::Nop => 0xE,
        }
    }

    /// Writes r15 (or may change the translation): the block ends after it.
    pub(crate) fn ends_block(&self) -> bool {
        match *self {
            Instr::Alu { op, rd, .. } => rd == 15 && !(0x8..=0xB).contains(&op),
            Instr::Mem { load, rd, .. } => load && rd == 15,
            Instr::Multiple { load, list, .. } => load && list & 0x8000 != 0,
            Instr::Branch { .. } | Instr::Bx { .. } => true,
            Instr::Exec { ends, .. } => ends,
            _ => false,
        }
    }
}

/// The CPSR bits an MSR writes (its field bits; never T).
fn msr_mask(w: u32) -> u32 {
    let mut mask = 0;
    for (bit, m) in [(16, 0x0000_00FF), (17, 0x0000_FF00), (18, 0x00FF_0000), (19, 0xFF00_0000)] {
        if w & (1 << bit) != 0 {
            mask |= m;
        }
    }
    mask & !0x20
}

fn reg_operand(w: u32) -> Operand {
    Operand::Reg { rm: w & 0xF, kind: (w >> 5) & 3, amount: (w >> 7) & 0x1F }
}

/// The instruction `w` at address `pc`.
pub(crate) fn decode(w: u32, pc: u32) -> Instr {
    let cond = w >> 28;
    let exec = Instr::Exec { cond, w, ends: false };
    if cond == 0xF {
        if w & 0x0D70_F000 == 0x0550_F000 {
            return Instr::Nop;
        }
        return Instr::Exec { cond: 0xE, w, ends: true };
    }
    let (rn, rd, rs, rm) = ((w >> 16) & 0xF, (w >> 12) & 0xF, (w >> 8) & 0xF, w & 0xF);
    let bit = |n: u32| w & (1 << n) != 0;
    let alu = |op2| {
        let (op, s) = ((w >> 21) & 0xF, bit(20));
        // MOVS pc etc. return from exceptions (a mode change).
        if s && rd == 15 && !(0x8..=0xB).contains(&op) {
            return exec;
        }
        Instr::Alu { cond, op, s, rn, rd, op2 }
    };
    let mem = |offset| {
        let (pre, up, byte, writeback, load) = (bit(24), bit(23), bit(22), bit(21), bit(20));
        // The T forms (User-mode access) and r15 written back are left to
        // the interpreter.
        if !pre && writeback || (!pre || writeback) && rn == 15 {
            return exec;
        }
        Instr::Mem { cond, load, byte, pre, up, writeback: writeback && pre, rn, rd, offset }
    };
    match (w >> 25) & 7 {
        0 => {
            if w & 0x0FC0_00F0 == 0x0000_0090 {
                // Rd is in bits 19-16 here, Rn in 15-12.
                let (rd, rn) = (rn, rd);
                if [rd, rs, rm].contains(&15) || bit(21) && rn == 15 {
                    return exec;
                }
                Instr::Mul { cond, acc: bit(21), s: bit(20), rd, rn, rs, rm }
            } else if w & 0x0F80_00F0 == 0x0080_0090 {
                // rn holds the high word, rd the low.
                if [rn, rd, rs, rm].contains(&15) {
                    return exec;
                }
                Instr::MulLong { cond, signed: bit(22), acc: bit(21), s: bit(20), hi: rn, lo: rd, rs, rm }
            } else if w & 0x0FB0_0FF0 == 0x0100_0090 {
                exec
            } else if w & 0x0E00_0090 == 0x0000_0090 {
                let (pre, up, imm, wb, load, sh) = (bit(24), bit(23), bit(22), bit(21), bit(20), (w >> 5) & 3);
                // LDRD/STRD, loads of r15, r15 written back or as the
                // offset register (it reads pc + 4) go to the interpreter.
                if sh == 0 || !load && sh >= 2 || load && rd == 15 || (!pre || wb) && rn == 15 || !imm && rm == 15 {
                    return exec;
                }
                let offset = if imm { Operand::Imm(((w >> 4) & 0xF0) | rm, None) } else { Operand::Reg { rm, kind: 0, amount: 0 } };
                Instr::Half { cond, load, sh, pre, up, writeback: !pre || wb, rn, rd, offset }
            } else if w & 0x0F90_0090 == 0x0100_0000 || w & 0x0F90_0090 == 0x0100_0010 {
                match ((w >> 4) & 0xF, (w >> 21) & 3) {
                    (0, 0) if rd != 15 => Instr::Mrs { cond, rd },
                    (0, 1) if rm != 15 => Instr::Msr { cond, mask: msr_mask(w), src: Operand::Reg { rm, kind: 0, amount: 0 } },
                    (1, 1) | (3, 1) if rm != 15 => Instr::Bx { cond, link: bit(5), rm },
                    (1, 3) if rd != 15 => Instr::Clz { cond, rd, rm },
                    _ => exec,
                }
            } else if w & 0x0F90_0090 == 0x0100_0080 {
                // Rd is in bits 19-16 here, Rn in 15-12.
                let (op, rd, rn) = ((w >> 21) & 3, rn, rd);
                if !matches!(op, 0 | 3) || [rd, rs, rm].contains(&15) || op == 0 && rn == 15 {
                    return exec;
                }
                Instr::Smul { cond, acc: op == 0, x: bit(5), y: bit(6), rd, rn, rs, rm }
            } else if bit(4) {
                if [rn, rs, rm].contains(&15) {
                    return exec;
                }
                alu(Operand::RegShift { rm, kind: (w >> 5) & 3, rs })
            } else {
                alu(reg_operand(w))
            }
        }
        1 => {
            // MSR (immediate): the CPSR's compiled, the SPSR's not.
            if w & 0x0FB0_0000 == 0x0320_0000 {
                if bit(22) {
                    return exec;
                }
                let v = (w & 0xFF).rotate_right(((w >> 8) & 0xF) * 2);
                return Instr::Msr { cond, mask: msr_mask(w), src: Operand::Imm(v, None) };
            }
            let rot = ((w >> 8) & 0xF) * 2;
            let v = (w & 0xFF).rotate_right(rot);
            alu(Operand::Imm(v, (rot != 0).then_some(v & 0x8000_0000 != 0)))
        }
        2 => mem(Operand::Imm(w & 0xFFF, None)),
        3 => {
            // Rm = r15 reads pc + 4 in the interpreter (UNPREDICTABLE).
            if bit(4) || rm == 15 {
                return exec;
            }
            mem(reg_operand(w))
        }
        5 => {
            let offset = (((w & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
            Instr::Branch { cond, link: bit(24), target: pc.wrapping_add(8).wrapping_add(offset) }
        }
        4 => {
            let list = w & 0xFFFF;
            if bit(22) || list == 0 || rn == 15 {
                return exec;
            }
            Instr::Multiple { cond, load: bit(20), pre: bit(24), up: bit(23), writeback: bit(21), rn, list }
        }
        // MRC only reads.
        7 if w & 0x0110_0010 == 0x0010_0010 => exec,
        // Other coprocessor instructions (CP15 may change the translation,
        // or wait for an interrupt) and SWI.
        _ => Instr::Exec { cond, w, ends: true },
    }
}

/// Names of the classes `class` sorts interpreted instructions into.
pub const CLASSES: [&str; 12] =
    ["thumb", "irq/wait", "handled", "ldm/stm", "extra ld/st", "mul", "reg shift", "psr", "coproc", "swi/undef", "ld pc/T", "other"];

/// Which kind of instruction the interpreter is about to run (diagnostics).
pub(crate) fn class(cpu: &mut ribbit_arm::Arm, bus: &mut impl ribbit_arm::ArmBus) -> usize {
    use ribbit_arm::psr;
    if cpu.cpsr & psr::T != 0 {
        return 0;
    }
    if cpu.waiting || cpu.fiq_line && cpu.cpsr & psr::F == 0 || cpu.irq_line && cpu.cpsr & psr::I == 0 {
        return 1;
    }
    let Ok(pa) = cpu.cp15.translate(bus, cpu.r[15], false, cpu.mode() == ribbit_arm::mode::USR) else { return 11 };
    let w = bus.read32(pa);
    if !matches!(decode(w, cpu.r[15]), Instr::Exec { .. }) {
        return 2;
    }
    match (w >> 25) & 7 {
        _ if w >> 28 == 0xF => 11,
        4 => 3,
        0 if w & 0x90 == 0x90 && w & 0x60 != 0 => 4,
        0 if w & 0xF0 == 0x90 => 5,
        0 if w & 0x0F90_0000 == 0x0100_0000 => 7,
        0 if w & 0x10 != 0 => 6,
        1 => 7,
        6 | 7 if w & 0x0F00_0000 != 0x0F00_0000 => 8,
        7 => 9,
        2 | 3 => 10,
        _ => 11,
    }
}
