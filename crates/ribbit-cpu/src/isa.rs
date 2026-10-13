//! ARCompact instruction vocabulary shared by the decoder, disassembler and
//! executor.
//!
//! Encodings and semantics follow the *ARCompact ISA Programmer's Reference*
//! (doc 5115-029, 2008; "PRM" below), which covers the ARCtangent-A5, ARC 600
//! and ARC 700. Where the A5 differs from later cores the PRM says so, and the
//! A5 behaviour is used. See docs/isa-arcompact.md.

/// Condition-code field (Q, 5 bits). PRM Table 50.
pub type Cond = u8;

pub const COND_AL: Cond = 0x00;
pub const COND_EQ: Cond = 0x01;
pub const COND_NE: Cond = 0x02;
pub const COND_PL: Cond = 0x03;
pub const COND_MI: Cond = 0x04;
pub const COND_CS: Cond = 0x05;
pub const COND_CC: Cond = 0x06;
pub const COND_VS: Cond = 0x07;
pub const COND_VC: Cond = 0x08;
pub const COND_GT: Cond = 0x09;
pub const COND_GE: Cond = 0x0A;
pub const COND_LT: Cond = 0x0B;
pub const COND_LE: Cond = 0x0C;
pub const COND_HI: Cond = 0x0D;
pub const COND_LS: Cond = 0x0E;
pub const COND_PNZ: Cond = 0x0F;

/// Mnemonic suffixes as printed by GNU binutils for ARC (basecase codes).
pub fn cond_name(cc: Cond) -> Option<&'static str> {
    const NAMES: [&str; 16] =
        ["", "eq", "ne", "p", "n", "c", "nc", "v", "nv", "gt", "ge", "lt", "le", "hi", "ls", "pnz"];
    NAMES.get(cc as usize).copied()
}

/// Evaluates a condition against STATUS32 flags. Extension codes 0x10-0x1F
/// are not implemented on the Leapster as far as we know; PRM ch. 8: "For the
/// ARCtangent-A5 and ARC 600 processors, if an extension condition code is
/// used that is not implemented, then the condition code test will always
/// return false".
pub fn cond_holds(cc: Cond, z: bool, n: bool, c: bool, v: bool) -> bool {
    match cc {
        COND_AL => true,
        COND_EQ => z,
        COND_NE => !z,
        COND_PL => !n,
        COND_MI => n,
        COND_CS => c,
        COND_CC => !c,
        COND_VS => v,
        COND_VC => !v,
        COND_GT => (n && v && !z) || (!n && !v && !z),
        COND_GE => n == v,
        COND_LT => n != v,
        COND_LE => z || n != v,
        COND_HI => !c && !z,
        COND_LS => c || z,
        COND_PNZ => !n && !z,
        _ => false,
    }
}

/// Compare-and-branch conditions (BRcc sub-opcode, PRM Table 56).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BrCond {
    Eq,
    Ne,
    Lt,
    Ge,
    Lo,
    Hs,
}

impl BrCond {
    pub fn name(self) -> &'static str {
        match self {
            BrCond::Eq => "eq",
            BrCond::Ne => "ne",
            BrCond::Lt => "lt",
            BrCond::Ge => "ge",
            BrCond::Lo => "lo",
            BrCond::Hs => "hs",
        }
    }

    pub fn holds(self, b: u32, c: u32) -> bool {
        match self {
            BrCond::Eq => b == c,
            BrCond::Ne => b != c,
            BrCond::Lt => (b as i32) < (c as i32),
            BrCond::Ge => (b as i32) >= (c as i32),
            BrCond::Lo => b < c,
            BrCond::Hs => b >= c,
        }
    }
}

/// Memory access size (ZZ field).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Size {
    Word,
    Byte,
    Half,
}

impl Size {
    pub fn bytes(self) -> u32 {
        match self {
            Size::Word => 4,
            Size::Byte => 1,
            Size::Half => 2,
        }
    }
}

/// Address write-back mode (AA field, PRM Table 52).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteBack {
    None,
    /// `.a`/`.aw`: address = base + offset, base updated before the access.
    Pre,
    /// `.ab`: address = base, base += offset after the access.
    Post,
    /// `.as`: address = base + (offset << size), no write-back.
    Scaled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemMode {
    pub size: Size,
    pub sign_extend: bool,
    pub wb: WriteBack,
    pub bypass_cache: bool,
}

impl MemMode {
    pub const WORD: MemMode =
        MemMode { size: Size::Word, sign_extend: false, wb: WriteBack::None, bypass_cache: false };

    pub const fn plain(size: Size) -> Self {
        MemMode { size, sign_extend: false, wb: WriteBack::None, bypass_cache: false }
    }
}

/// Operations. 16-bit forms decode to the same operation as their 32-bit
/// equivalents; `Instr::short` records the encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    // Basecase dual-operand ALU (major 0x04, sub-opcodes 0x00-0x19).
    Add,
    Adc,
    Sub,
    Sbc,
    And,
    Or,
    Bic,
    Xor,
    Max,
    Min,
    Mov,
    Tst,
    Cmp,
    Rcmp,
    Rsub,
    Bset,
    Bclr,
    Btst,
    Bxor,
    Bmsk,
    Add1,
    Add2,
    Add3,
    Sub1,
    Sub2,
    Sub3,
    // Basecase single-operand (major 0x04, sub-opcode 0x2F).
    Asl,
    Asr,
    Lsr,
    Ror,
    Rrc,
    Sexb,
    Sexw,
    Extb,
    Extw,
    Abs,
    Not,
    Rlc,
    /// 16-bit only (NEG_S); the 32-bit form is RSUB a,b,0.
    Neg,
    // Barrel shifter (major 0x05, sub-opcodes 0x00-0x03; also 16-bit forms).
    AslM,
    LsrM,
    AsrM,
    RorM,
    // Optional multiply (MUL64 option).
    Mul64,
    Mulu64,
    // Optional extended-arithmetic library (major 0x05).
    Adds,
    Subs,
    Divaw,
    Asls,
    Asrs,
    Addsdw,
    Subsdw,
    Swap,
    Norm,
    Sat16,
    Rnd16,
    Abssw,
    Abss,
    Negsw,
    Negs,
    Normw,
    // Leapster DSP extension as identified by MAME (major 0x05, 0x0C/0x10/0x14).
    // No authoritative documentation: decoded but not executed.
    Muldw,
    Macdw,
    Msubdw,
    // Control.
    B,
    Bl,
    Br(BrCond),
    Bbit0,
    Bbit1,
    J,
    Jl,
    Lp,
    Flag,
    Lr,
    Sr,
    Sleep,
    /// SWI: on the A5 raises the Instruction Error interrupt (PRM, SWI).
    Swi,
    Brk,
    Nop,
    Unimp,
    // Memory.
    Ld,
    St,
    Push,
    Pop,
    /// An ARC 700-only instruction (MPY*, EX, SYNC, RTIE, TRAP_S): an
    /// Instruction Error on the ARCtangent-A5.
    Arc700Only(&'static str),
    /// An extension-instruction slot (majors 0x05-0x0B) with no known meaning.
    Extension {
        major: u8,
        sub: u8,
    },
    /// An encoding the PRM defines as raising an Instruction Error, or as
    /// undefined on the A5.
    Illegal(&'static str),
}

impl Op {
    /// Basecase or optional operation Ribbit executes.
    pub fn is_executable(self) -> bool {
        !matches!(self, Op::Muldw | Op::Macdw | Op::Msubdw | Op::Extension { .. })
    }
}

/// Register names used by GNU binutils.
pub fn reg_name(r: u8) -> String {
    match r {
        26 => "gp".into(),
        27 => "fp".into(),
        28 => "sp".into(),
        29 => "ilink1".into(),
        30 => "ilink2".into(),
        31 => "blink".into(),
        60 => "lp_count".into(),
        61 => "reserved".into(),
        62 => "limm".into(),
        63 => "pcl".into(),
        n => format!("r{n}"),
    }
}

/// Auxiliary registers documented for the ARCtangent-A5 (PRM ch. 3) and the
/// names binutils uses for them.
pub const AUX_NAMES: &[(u32, &str)] = &[
    (0x00, "status"),
    (0x01, "semaphore"),
    (0x02, "lp_start"),
    (0x03, "lp_end"),
    (0x04, "identity"),
    (0x05, "debug"),
    (0x06, "pc"),
    (0x0A, "status32"),
    (0x0B, "status32_l1"),
    (0x0C, "status32_l2"),
    (0x12, "mulhi"),
    (0x21, "count0"),
    (0x22, "control0"),
    (0x23, "limit0"),
    (0x25, "int_vector_base"),
    (0x41, "aux_macmode"),
    (0x43, "aux_irq_lv12"),
    (0x100, "count1"),
    (0x101, "control1"),
    (0x102, "limit1"),
    (0x200, "aux_irq_lev"),
    (0x201, "aux_irq_hint"),
];

pub fn aux_name(n: u32) -> Option<&'static str> {
    AUX_NAMES.iter().find(|(k, _)| *k == n).map(|(_, v)| *v)
}

/// A source operand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    None,
    Reg(u8),
    /// Immediate encoded in the instruction (already sign/zero extended and
    /// scaled where the format scales it).
    Imm(u32),
    /// Long immediate (register field 62).
    Limm(u32),
}

/// Destination of the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dst {
    None,
    Reg(u8),
    /// Register field 62: result discarded (flags may still update).
    Discard,
}

/// A fully decoded instruction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Instr {
    pub addr: u32,
    /// Total length in bytes, including any long immediate.
    pub len: u32,
    /// 32-bit instruction word (high halfword first), or the 16-bit
    /// instruction in the low half.
    pub word: u32,
    pub short: bool,
    pub op: Op,
    pub cc: Cond,
    /// `.f`: update flags (also set for instructions whose flag update is
    /// implicit: CMP, RCMP, TST, BTST and their 16-bit forms).
    pub set_flags: bool,
    /// `.d`: delay slot always executes.
    pub delay: bool,
    pub dst: Dst,
    pub src1: Src,
    pub src2: Src,
    /// Value written by ST/ST_S/PUSH_S.
    pub data: Src,
    pub mem: MemMode,
    /// Branch/loop target, or jump-to-ILINK marker usage; 0 when unused.
    pub target: u32,
    pub limm: Option<u32>,
}

impl Instr {
    /// Address of the next sequential instruction.
    pub fn next(&self) -> u32 {
        self.addr.wrapping_add(self.len)
    }

    pub fn is_branch(&self) -> bool {
        matches!(self.op, Op::B | Op::Bl | Op::Br(_) | Op::Bbit0 | Op::Bbit1 | Op::J | Op::Jl)
    }
}
