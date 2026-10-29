//! The ARM instructions the generator handles (phase 1), decoded exactly as
//! `arm::execute` tells them apart. Anything else is `None`: the block
//! stops before it and the interpreter runs it.

/// A shifter operand or an addressing offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operand {
    /// A value, and the shifter carry out when the rotation is not 0.
    Imm(u32, Option<bool>),
    /// A register shifted by an immediate (kind: LSL, LSR, ASR, ROR/RRX).
    Reg { rm: u32, kind: u32, amount: u32 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Instr {
    /// Data processing. `rd` 15 (without S) branches and ends the block.
    Alu { cond: u32, op: u32, s: bool, rn: u32, rd: u32, op2: Operand },
    /// LDR/STR/LDRB/STRB (not the T forms, not loads of r15).
    Mem { cond: u32, load: bool, byte: bool, pre: bool, up: bool, writeback: bool, rn: u32, rd: u32, offset: Operand },
    /// B/BL (ends the block).
    Branch { cond: u32, link: bool, target: u32 },
}

impl Instr {
    pub(crate) fn cond(&self) -> u32 {
        match *self {
            Instr::Alu { cond, .. } | Instr::Mem { cond, .. } | Instr::Branch { cond, .. } => cond,
        }
    }

    /// Writes r15: the block ends after it.
    pub(crate) fn ends_block(&self) -> bool {
        match *self {
            Instr::Alu { op, rd, .. } => rd == 15 && !(0x8..=0xB).contains(&op),
            Instr::Mem { .. } => false,
            Instr::Branch { .. } => true,
        }
    }
}

fn reg_operand(w: u32) -> Operand {
    Operand::Reg { rm: w & 0xF, kind: (w >> 5) & 3, amount: (w >> 7) & 0x1F }
}

/// The instruction `w` at address `pc`, if the generator handles it.
pub(crate) fn decode(w: u32, pc: u32) -> Option<Instr> {
    let cond = w >> 28;
    if cond == 0xF {
        return None;
    }
    let alu = |op2| {
        let (op, s, rn, rd) = ((w >> 21) & 0xF, w & (1 << 20) != 0, (w >> 16) & 0xF, (w >> 12) & 0xF);
        // MOVS pc etc. return from exceptions (a mode change).
        if s && rd == 15 && !(0x8..=0xB).contains(&op) {
            return None;
        }
        Some(Instr::Alu { cond, op, s, rn, rd, op2 })
    };
    let mem = |offset| {
        let (pre, up, byte, writeback, load) =
            (w & (1 << 24) != 0, w & (1 << 23) != 0, w & (1 << 22) != 0, w & (1 << 21) != 0, w & (1 << 20) != 0);
        let (rn, rd) = ((w >> 16) & 0xF, (w >> 12) & 0xF);
        // The T forms (User-mode access), loads of r15 (interworking), and
        // r15 written back are left to the interpreter.
        if !pre && writeback || load && rd == 15 || (!pre || writeback) && rn == 15 {
            return None;
        }
        Some(Instr::Mem { cond, load, byte, pre, up, writeback: writeback && pre, rn, rd, offset })
    };
    match (w >> 25) & 7 {
        0 => {
            // Register-specified shifts, multiplies, the extra loads and
            // stores, and the miscellaneous space all have bit 4 or bit 7
            // set or are MRS/MSR/the DSP multiplies.
            if w & 0x10 != 0 || w & 0x0F90_0000 == 0x0100_0000 {
                return None;
            }
            alu(reg_operand(w))
        }
        1 => {
            if w & 0x0FB0_0000 == 0x0320_0000 {
                // MSR (immediate).
                return None;
            }
            let rot = ((w >> 8) & 0xF) * 2;
            let v = (w & 0xFF).rotate_right(rot);
            alu(Operand::Imm(v, (rot != 0).then_some(v & 0x8000_0000 != 0)))
        }
        2 => mem(Operand::Imm(w & 0xFFF, None)),
        3 => {
            // Rm = r15 reads pc + 4 in the interpreter (UNPREDICTABLE).
            if w & 0x10 != 0 || w & 0xF == 15 {
                return None;
            }
            mem(reg_operand(w))
        }
        5 => {
            let offset = (((w & 0x00FF_FFFF) << 8) as i32 >> 6) as u32;
            Some(Instr::Branch { cond, link: w & (1 << 24) != 0, target: pc.wrapping_add(8).wrapping_add(offset) })
        }
        _ => None,
    }
}
