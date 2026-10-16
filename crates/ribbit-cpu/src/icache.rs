//! Decoded-instruction cache for read-only code.
//!
//! Decoding is a pure function of the bytes at the PC, so for memory that
//! cannot change (the machine declares ROM ranges: BIOS, cartridge) a decoded
//! instruction can be reused. Anything outside the declared ranges (RAM,
//! VRAM) is decoded on every fetch. The cache is not architectural state: it
//! never changes behaviour, only speed, and compares equal regardless of
//! contents.
//!
//! Each entry also holds a pre-digested form of the instruction (`Hot`) for
//! the batched executor (`Cpu::run_block`): the commonest shapes in compiled
//! code (register moves, unconditional ALU operations without flags, plain
//! loads and stores, push/pop, compare-and-branch, calls and returns) with
//! their operands resolved, so they run without the general executor's
//! operand and condition decoding. Anything else is `Fast::General`.

use crate::isa::{BrCond, COND_AL, Cond, Dst, Instr, Op, Size, Src, WriteBack};

/// Direct-mapped slots, one per halfword of 512 KiB of code (104 bytes
/// each, 26 MiB). With 1 << 16 (128 KiB) the Leapster 2 BIOS's code 128 KiB
/// apart evicted itself: Pet Pals and Outwit missed 2.3% of their fetches
/// (26 M decodes per 1.15 G instructions; now 0.2 M), 10% of their time.
const SETS: usize = 1 << 18;
/// Longest instruction (32-bit word + long immediate).
const MAX_LEN: u32 = 8;

/// ALU operations with a fast form (no flags, always executed).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AluOp {
    Add,
    Add1,
    Add2,
    Add3,
    Sub,
    Sub1,
    Sub2,
    Sub3,
    Rsub,
    And,
    Or,
    Xor,
    Bic,
    Asl,
    Lsr,
    Asr,
    Adc,
    Sbc,
}

impl AluOp {
    fn of(op: Op) -> Option<AluOp> {
        Some(match op {
            Op::Add => AluOp::Add,
            Op::Add1 => AluOp::Add1,
            Op::Add2 => AluOp::Add2,
            Op::Add3 => AluOp::Add3,
            Op::Sub => AluOp::Sub,
            Op::Sub1 => AluOp::Sub1,
            Op::Sub2 => AluOp::Sub2,
            Op::Sub3 => AluOp::Sub3,
            Op::Rsub => AluOp::Rsub,
            Op::And => AluOp::And,
            Op::Or => AluOp::Or,
            Op::Xor => AluOp::Xor,
            Op::Bic => AluOp::Bic,
            Op::AslM => AluOp::Asl,
            Op::LsrM => AluOp::Lsr,
            Op::AsrM => AluOp::Asr,
            Op::Adc => AluOp::Adc,
            Op::Sbc => AluOp::Sbc,
            _ => return None,
        })
    }

    /// The result, as the general executor computes it (`carry`: the C flag).
    #[inline(always)]
    pub fn apply(self, b: u32, c: u32, carry: bool) -> u32 {
        match self {
            AluOp::Adc => b.wrapping_add(c).wrapping_add(carry as u32),
            AluOp::Sbc => b.wrapping_sub(c).wrapping_sub(carry as u32),
            AluOp::Add => b.wrapping_add(c),
            AluOp::Add1 => b.wrapping_add(c << 1),
            AluOp::Add2 => b.wrapping_add(c << 2),
            AluOp::Add3 => b.wrapping_add(c << 3),
            AluOp::Sub => b.wrapping_sub(c),
            AluOp::Sub1 => b.wrapping_sub(c << 1),
            AluOp::Sub2 => b.wrapping_sub(c << 2),
            AluOp::Sub3 => b.wrapping_sub(c << 3),
            AluOp::Rsub => c.wrapping_sub(b),
            AluOp::And => b & c,
            AluOp::Or => b | c,
            AluOp::Xor => b ^ c,
            AluOp::Bic => b & !c,
            AluOp::Asl => b << (c & 31),
            AluOp::Lsr => b >> (c & 31),
            AluOp::Asr => ((b as i32) >> (c & 31)) as u32,
        }
    }
}

/// Single-operand operations with a fast form (no flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnOp {
    Asl,
    Asr,
    Lsr,
    Ror,
    Sexb,
    Sexw,
    Extb,
    Extw,
    Not,
    Neg,
    Abs,
    Swap,
}

impl UnOp {
    fn of(op: Op) -> Option<UnOp> {
        Some(match op {
            Op::Asl => UnOp::Asl,
            Op::Asr => UnOp::Asr,
            Op::Lsr => UnOp::Lsr,
            Op::Ror => UnOp::Ror,
            Op::Sexb => UnOp::Sexb,
            Op::Sexw => UnOp::Sexw,
            Op::Extb => UnOp::Extb,
            Op::Extw => UnOp::Extw,
            Op::Not => UnOp::Not,
            Op::Neg => UnOp::Neg,
            Op::Abs => UnOp::Abs,
            Op::Swap => UnOp::Swap,
            _ => return None,
        })
    }

    /// The result, as the general executor computes it.
    #[inline(always)]
    pub fn apply(self, c: u32) -> u32 {
        match self {
            UnOp::Asl => c << 1,
            UnOp::Asr => ((c as i32) >> 1) as u32,
            UnOp::Lsr => c >> 1,
            UnOp::Ror => c.rotate_right(1),
            UnOp::Sexb => c as u8 as i8 as i32 as u32,
            UnOp::Sexw => c as u16 as i16 as i32 as u32,
            UnOp::Extb => c & 0xFF,
            UnOp::Extw => c & 0xFFFF,
            UnOp::Not => !c,
            UnOp::Neg => 0u32.wrapping_sub(c),
            UnOp::Abs => (c as i32).wrapping_abs() as u32,
            UnOp::Swap => c.rotate_left(16),
        }
    }
}

/// `d` of `Fast::AluF`/`AluFI` when the result is discarded (flags only).
pub const DISCARD: u8 = 0xFF;

/// Load widths with a fast form.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LdKind {
    Word,
    Byte,
    ByteSigned,
    Half,
    HalfSigned,
}

/// Pre-digested instruction. Register numbers are core registers r0-r31
/// (plain storage); immediates and long immediates are resolved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fast {
    /// Run by the general executor.
    General,
    Mov { d: u8, s: u8 },
    MovI { d: u8, v: u32 },
    Alu { op: AluOp, d: u8, b: u8, c: u8 },
    AluI { op: AluOp, d: u8, b: u8, v: u32 },
    /// LD d,[b,off] (no write-back).
    Ld { kind: LdKind, d: u8, b: u8, off: u32 },
    /// LD d,[b,c] (no write-back).
    LdR { kind: LdKind, d: u8, b: u8, c: u8 },
    /// LD.A/.AB/.AS d,[b,off] (`d != b` with write-back).
    LdX { kind: LdKind, wb: WriteBack, d: u8, b: u8, off: u32 },
    /// ST s,[b,off] (no write-back).
    St { size: Size, s: u8, b: u8, off: u32 },
    /// ST.A/.AB/.AS s,[b,off].
    StX { size: Size, wb: WriteBack, s: u8, b: u8, off: u32 },
    /// Conditional ALU operation without flags.
    AluCc { op: AluOp, cc: Cond, d: u8, b: u8, c: u8 },
    AluCcI { op: AluOp, cc: Cond, d: u8, b: u8, v: u32 },
    /// ALU operation with .F (32-bit encoding, always executed); `d` may
    /// be `DISCARD`.
    AluF { op: AluOp, d: u8, b: u8, c: u8 },
    AluFI { op: AluOp, d: u8, b: u8, v: u32 },
    /// Single-operand operation without flags.
    Un { op: UnOp, d: u8, c: u8 },
    Push { s: u8 },
    Pop { d: u8 },
    /// BRcc b,v (target in `Hot`).
    Br { cond: BrCond, b: u8, v: u32 },
    BrR { cond: BrCond, b: u8, c: u8 },
    /// Bcc (target in `Hot`).
    B { cc: Cond },
    /// BLcc without a delay slot: BLINK = next.
    Bl { cc: Cond },
    /// BLcc.D: BLINK = the address after the delay slot.
    BlD { cc: Cond },
    /// J [s] (always, no flags).
    J { s: u8 },
    /// Jcc [s] (no flags).
    Jcc { cc: Cond, s: u8 },
    Cmp { b: u8, c: u8 },
    CmpI { b: u8, v: u32 },
}

impl Fast {
    /// Reads or writes memory (the bus is told the cycle and address first).
    #[inline(always)]
    pub fn accesses_memory(self) -> bool {
        matches!(
            self,
            Fast::Ld { .. }
                | Fast::LdR { .. }
                | Fast::LdX { .. }
                | Fast::St { .. }
                | Fast::StX { .. }
                | Fast::Push { .. }
                | Fast::Pop { .. }
        )
    }
}

/// What `Cpu::run_block` needs per instruction, without the full `Instr`
/// (kept apart from it in the cache: the batched loop touches only these).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hot {
    /// Address of the instruction (the cache tag; odd = empty slot).
    pub addr: u32,
    pub fast: Fast,
    /// Address of the next sequential instruction.
    pub next: u32,
    pub target: u32,
    /// `.d`: delay slot.
    pub delay: bool,
    /// Left to `Cpu::step_with` (changes interrupt state, halts, or raises).
    pub stop: bool,
    /// May make an interrupt pending (FLAG, SR): the batch ends after it if
    /// one is, as stepping would take it next.
    pub recheck: bool,
    /// FLAG with a register operand: halts if that register's bit 0 (H) is
    /// set, so the batch stops before it then (`NO_REG` otherwise).
    pub flag_reg: u8,
    /// A branch, jump or LP: unpredictable in a delay slot.
    pub branchy: bool,
    /// Pipeline timing (`Instr::reg_reads`, `load_dst`, `is_compare_branch`).
    pub reads: u64,
    pub load_dst: u8,
    pub compare_branch: bool,
}

impl Hot {
    pub fn of(i: &Instr) -> Hot {
        Hot {
            addr: i.addr,
            fast: fast_form(i),
            next: i.next(),
            target: i.target,
            delay: i.delay,
            stop: (i.op.block_unsafe() && !in_block(i)) || (matches!(i.op, Op::J | Op::Jl) && i.set_flags),
            recheck: matches!(i.op, Op::Flag | Op::Sr),
            flag_reg: match (i.op, i.src2) {
                (Op::Flag, Src::Reg(r)) => r,
                _ => NO_REG,
            },
            branchy: i.is_branch() || matches!(i.op, Op::Lp),
            reads: i.reg_reads(),
            load_dst: i.load_dst(),
            compare_branch: i.is_compare_branch(),
        }
    }
}

/// `Hot::flag_reg`: none.
pub const NO_REG: u8 = 0xFF;

/// FLAG, LR and SR that a batch (`Cpu::run_block`) can run: they only
/// touch CPU state that nothing in a batch reads ahead of time. FLAG must
/// not halt (a constant without H, or a core register checked when it runs);
/// LR and SR need a constant register number from the list below. Not the
/// processor timers (0x21-0x23, 0x100-0x102: a batch brings them up to date
/// only at its end), DEBUG, PC or anything handled outside the CPU.
fn in_block(i: &Instr) -> bool {
    let aux = match i.src2 {
        Src::Imm(n) | Src::Limm(n) => Some(n),
        _ => None,
    };
    let xy = |n: u32| (crate::xy::AUX_BASE..crate::xy::AUX_BASE + crate::xy::AUX_COUNT).contains(&n);
    match i.op {
        Op::Flag => match i.src2 {
            Src::Imm(v) | Src::Limm(v) => v & crate::status32::H == 0,
            Src::Reg(r) => r < 32,
            Src::None => false,
        },
        // LP_START, LP_END, IDENTITY, STATUS32, STATUS32_L1/L2, INT_VECTOR_BASE,
        // AUX_MACMODE, AUX_IRQ_LV12, AUX_IRQ_LEV, AUX_IRQ_HINT, XY.
        Op::Lr => aux.is_some_and(|n| matches!(n, 0x02..=0x04 | 0x0A..=0x0C | 0x25 | 0x41 | 0x43 | 0x200 | 0x201) || xy(n)),
        // As LR, with MULHI and without the read-only ones.
        Op::Sr => aux.is_some_and(|n| matches!(n, 0x02 | 0x03 | 0x0B | 0x0C | 0x12 | 0x25 | 0x41 | 0x43 | 0x200 | 0x201) || xy(n)),
        _ => false,
    }
}

fn core(r: u8) -> Option<u8> {
    (r < 32).then_some(r)
}

fn reg(s: Src) -> Option<u8> {
    match s {
        Src::Reg(r) => core(r),
        _ => None,
    }
}

fn imm(s: Src) -> Option<u32> {
    match s {
        Src::Imm(v) | Src::Limm(v) => Some(v),
        _ => None,
    }
}

fn dreg(d: Dst) -> Option<u8> {
    match d {
        Dst::Reg(r) => core(r),
        _ => None,
    }
}

/// The fast form of `i`, or `General`. Each form is exactly what
/// `Cpu::execute` does for that shape (see `fast_forms_match_the_executor`
/// in the CPU tests).
pub fn fast_form(i: &Instr) -> Fast {
    let plain = i.cc == COND_AL && !i.set_flags;
    let f = match i.op {
        // The executor reads src1 even for MOV; only skip it if that is free.
        Op::Mov if plain && (i.src1 == Src::None || reg(i.src1).is_some()) => match (dreg(i.dst), reg(i.src2), imm(i.src2)) {
            (Some(d), Some(s), _) => Some(Fast::Mov { d, s }),
            (Some(d), _, Some(v)) => Some(Fast::MovI { d, v }),
            _ => None,
        },
        op if plain && AluOp::of(op).is_some() => {
            let op = AluOp::of(op).unwrap();
            match (dreg(i.dst), reg(i.src1), reg(i.src2), imm(i.src2)) {
                (Some(d), Some(b), Some(c), _) => Some(Fast::Alu { op, d, b, c }),
                (Some(d), Some(b), _, Some(v)) => Some(Fast::AluI { op, d, b, v }),
                _ => None,
            }
        }
        op if !i.set_flags && AluOp::of(op).is_some() => {
            let (op, cc) = (AluOp::of(op).unwrap(), i.cc);
            match (dreg(i.dst), reg(i.src1), reg(i.src2), imm(i.src2)) {
                (Some(d), Some(b), Some(c), _) => Some(Fast::AluCc { op, cc, d, b, c }),
                (Some(d), Some(b), _, Some(v)) => Some(Fast::AluCcI { op, cc, d, b, v }),
                _ => None,
            }
        }
        // The general executor sets flags only for 32-bit encodings.
        op if i.set_flags && !i.short && i.cc == COND_AL && AluOp::of(op).is_some() => {
            let op = AluOp::of(op).unwrap();
            let d = match i.dst {
                Dst::Discard => Some(DISCARD),
                d => dreg(d),
            };
            match (d, reg(i.src1), reg(i.src2), imm(i.src2)) {
                (Some(d), Some(b), Some(c), _) => Some(Fast::AluF { op, d, b, c }),
                (Some(d), Some(b), _, Some(v)) => Some(Fast::AluFI { op, d, b, v }),
                _ => None,
            }
        }
        op if plain && UnOp::of(op).is_some() && (i.src1 == Src::None || reg(i.src1).is_some()) => {
            match (dreg(i.dst), reg(i.src2)) {
                (Some(d), Some(c)) => Some(Fast::Un { op: UnOp::of(op).unwrap(), d, c }),
                _ => None,
            }
        }
        Op::Ld if i.cc == COND_AL && i.mem.wb != WriteBack::None => {
            let kind = match (i.mem.size, i.mem.sign_extend) {
                (Size::Word, false) => Some(LdKind::Word),
                (Size::Byte, false) => Some(LdKind::Byte),
                (Size::Byte, true) => Some(LdKind::ByteSigned),
                (Size::Half, false) => Some(LdKind::Half),
                (Size::Half, true) => Some(LdKind::HalfSigned),
                (Size::Word, true) => None,
            };
            match (kind, dreg(i.dst), reg(i.src1), imm(i.src2)) {
                // Write-back into the destination is an error the general
                // executor reports.
                (Some(kind), Some(d), Some(b), Some(off)) if d != b || i.mem.wb == WriteBack::Scaled => {
                    Some(Fast::LdX { kind, wb: i.mem.wb, d, b, off })
                }
                _ => None,
            }
        }
        Op::St if i.cc == COND_AL && i.mem.wb != WriteBack::None => match (reg(i.data), reg(i.src1), imm(i.src2)) {
            (Some(s), Some(b), Some(off)) => Some(Fast::StX { size: i.mem.size, wb: i.mem.wb, s, b, off }),
            _ => None,
        },
        Op::Ld if i.cc == COND_AL && i.mem.wb == WriteBack::None => {
            let kind = match (i.mem.size, i.mem.sign_extend) {
                (Size::Word, false) => Some(LdKind::Word),
                (Size::Byte, false) => Some(LdKind::Byte),
                (Size::Byte, true) => Some(LdKind::ByteSigned),
                (Size::Half, false) => Some(LdKind::Half),
                (Size::Half, true) => Some(LdKind::HalfSigned),
                (Size::Word, true) => None,
            };
            match (kind, dreg(i.dst), reg(i.src1), reg(i.src2), imm(i.src2)) {
                (Some(kind), Some(d), Some(b), Some(c), _) => Some(Fast::LdR { kind, d, b, c }),
                (Some(kind), Some(d), Some(b), _, Some(off)) => Some(Fast::Ld { kind, d, b, off }),
                _ => None,
            }
        }
        Op::St if i.cc == COND_AL && i.mem.wb == WriteBack::None => match (reg(i.data), reg(i.src1), imm(i.src2)) {
            (Some(s), Some(b), Some(off)) => Some(Fast::St { size: i.mem.size, s, b, off }),
            _ => None,
        },
        Op::Push if i.cc == COND_AL => reg(i.data).map(|s| Fast::Push { s }),
        Op::Pop if i.cc == COND_AL => dreg(i.dst).map(|d| Fast::Pop { d }),
        Op::Br(cond) => match (reg(i.src1), reg(i.src2), imm(i.src2)) {
            (Some(b), Some(c), _) => Some(Fast::BrR { cond, b, c }),
            (Some(b), _, Some(v)) => Some(Fast::Br { cond, b, v }),
            _ => None,
        },
        Op::B => Some(Fast::B { cc: i.cc }),
        Op::Bl if !i.delay => Some(Fast::Bl { cc: i.cc }),
        Op::Bl => Some(Fast::BlD { cc: i.cc }),
        Op::J if plain => reg(i.src2).map(|s| Fast::J { s }),
        Op::J if !i.set_flags => reg(i.src2).map(|s| Fast::Jcc { cc: i.cc, s }),
        Op::Cmp if i.cc == COND_AL => match (reg(i.src1), reg(i.src2), imm(i.src2)) {
            (Some(b), Some(c), _) => Some(Fast::Cmp { b, c }),
            (Some(b), _, Some(v)) => Some(Fast::CmpI { b, v }),
            _ => None,
        },
        _ => None,
    };
    f.unwrap_or(Fast::General)
}

const EMPTY: Hot = Hot {
    addr: 1,
    fast: Fast::General,
    next: 0,
    target: 0,
    delay: false,
    stop: false,
    recheck: false,
    flag_reg: NO_REG,
    branchy: false,
    reads: 0,
    load_dst: crate::isa::NO_LOAD,
    compare_branch: false,
};

#[derive(Clone, Default)]
pub struct ICache {
    /// Half-open address ranges whose contents never change.
    ranges: Vec<(u32, u32)>,
    /// Per 64 KiB page: every instruction starting in it lies inside one range.
    pages: Vec<bool>,
    /// Per slot: the pre-digested form (tag included) and the full decode.
    hot: Vec<Hot>,
    instrs: Vec<Instr>,
    /// Addresses whose cached form ends a batch before them (`set_stops`).
    stops: Vec<u32>,
    /// Lookups that missed, and full-decode lookups that hit (`get`; the
    /// batched loop's hits are not counted).
    pub hits: u64,
    pub misses: u64,
}

impl PartialEq for ICache {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for ICache {}

impl std::fmt::Debug for ICache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ICache {{ ranges: {:x?}, hits: {}, misses: {} }}", self.ranges, self.hits, self.misses)
    }
}

impl ICache {
    /// Declares read-only ranges (start, end exclusive) and clears the cache.
    pub fn set_rom_ranges(&mut self, ranges: &[(u32, u32)]) {
        self.ranges = ranges.to_vec();
        let blank = crate::decode::decode_with(0, |_| 0);
        (self.hot, self.instrs) =
            if ranges.is_empty() { (Vec::new(), Vec::new()) } else { (vec![EMPTY; SETS], vec![blank; SETS]) };
        self.pages = (0..1u32 << 16)
            .map(|p| {
                let (start, last) = (p << 16, (p << 16) | 0xFFFF);
                self.ranges.iter().any(|&(s, e)| start >= s && last.checked_add(MAX_LEN).is_some_and(|end| end <= e))
            })
            .collect();
    }

    pub fn clear(&mut self) {
        self.hot.iter_mut().for_each(|e| *e = EMPTY);
    }

    /// Makes `run_block` stop before each of `pcs` (in read-only ranges), so
    /// the caller sees them executed by `step`. Free per instruction: the
    /// flag is part of the cached form. Clears the cache.
    pub fn set_stops(&mut self, pcs: &[u32]) {
        self.stops = pcs.to_vec();
        self.clear();
    }

    #[inline(always)]
    fn cacheable(&self, pc: u32) -> bool {
        if self.pages.get((pc >> 16) as usize).is_some_and(|&p| p) {
            return true;
        }
        self.cacheable_slow(pc)
    }

    #[inline(never)]
    fn cacheable_slow(&self, pc: u32) -> bool {
        self.ranges.iter().any(|&(s, e)| pc >= s && pc.checked_add(MAX_LEN).is_some_and(|end| end <= e))
    }

    /// The cache slot holding `pc`, filled with `decode` on a miss; None if
    /// `pc` is not in a read-only range.
    #[inline(always)]
    fn slot(&mut self, pc: u32, decode: impl FnOnce() -> Instr) -> Option<usize> {
        let slot = (pc as usize >> 1) & (SETS - 1);
        // A hit needs no range check: only cacheable addresses are stored
        // (and `set_rom_ranges` empties the cache).
        if self.hot.get(slot).is_some_and(|h| h.addr == pc) {
            return Some(slot);
        }
        if self.hot.is_empty() || !self.cacheable(pc) {
            return None;
        }
        self.misses += 1;
        let i = decode();
        self.hot[slot] = Hot::of(&i);
        self.hot[slot].stop |= self.stops.contains(&pc);
        self.instrs[slot] = i;
        Some(slot)
    }

    /// The pre-digested form of `pc` (cached), or None outside read-only ranges.
    #[inline(always)]
    pub fn hot(&mut self, pc: u32, decode: impl FnOnce() -> Instr) -> Option<Hot> {
        self.slot(pc, decode).map(|s| self.hot[s])
    }

    /// The cached decode of `pc`, decoding with `decode` on a miss.
    #[inline]
    pub fn get(&mut self, pc: u32, decode: impl FnOnce() -> Instr) -> Instr {
        let mut decode = Some(decode);
        let misses = self.misses;
        match self.slot(pc, || (decode.take().unwrap())()) {
            Some(s) => {
                self.hits += (self.misses == misses) as u64;
                self.instrs[s]
            }
            None => (decode.take().unwrap())(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_with;

    fn rom(addr: u32) -> u16 {
        // mov_s r0,0x1 repeated (0xd801), any even address.
        let _ = addr;
        0xd801
    }

    #[test]
    fn caches_only_inside_rom_ranges() {
        let mut c = ICache::default();
        c.set_rom_ranges(&[(0x4000_0000, 0x4000_1000)]);
        let mut decodes = 0;
        for _ in 0..3 {
            c.get(0x4000_0010, || {
                decodes += 1;
                decode_with(0x4000_0010, rom)
            });
        }
        assert_eq!((decodes, c.hits, c.misses), (1, 2, 1));
        for _ in 0..2 {
            c.get(0x3C00_0000, || {
                decodes += 1;
                decode_with(0x3C00_0000, rom)
            });
        }
        assert_eq!(decodes, 3, "RAM is decoded every time");
        // The last 8 bytes of a range are not cached (the instruction could
        // extend past it).
        c.get(0x4000_0FFC, || {
            decodes += 1;
            decode_with(0x4000_0FFC, rom)
        });
        c.get(0x4000_0FFC, || {
            decodes += 1;
            decode_with(0x4000_0FFC, rom)
        });
        assert_eq!(decodes, 5);
    }

    #[test]
    fn colliding_addresses_are_told_apart() {
        let mut c = ICache::default();
        c.set_rom_ranges(&[(0, 0x100_0000)]);
        let a = c.get(0x10, || decode_with(0x10, rom));
        let b = c.get(0x10 + (SETS as u32) * 2, || decode_with(0x10 + (SETS as u32) * 2, rom));
        assert_eq!((a.addr, b.addr), (0x10, 0x10 + (SETS as u32) * 2));
        assert_eq!(c.hits, 0);
    }

    #[test]
    fn mov_s_has_a_fast_form() {
        assert_eq!(fast_form(&decode_with(0, rom)), Fast::MovI { d: 0, v: 1 });
    }
}
