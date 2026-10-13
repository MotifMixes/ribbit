//! Disassembler.
//!
//! Mnemonics follow GNU binutils' spelling (`ldh`, `ldw_s`, `blle`, `mov.ne`,
//! `ld.di.ab.x`) so traces can be compared with `arc-elf32-objdump`.
//! Operand formatting is Ribbit's own; [`canonical`] reduces either
//! disassembler's text to a comparable form for the oracle tests.

use crate::isa::*;
use std::fmt::Write as _;

/// 32-bit NEG is "an encoding of the reverse subtract instruction using an
/// unsigned 6-bit immediate value set to 0" (PRM, NEG).
fn is_neg_alias(i: &Instr) -> bool {
    let p = (i.word >> 22) & 3;
    let u6 = p == 1 || (p == 3 && i.word & 0x20 != 0);
    !i.short && i.op == Op::Rsub && u6 && i.src2 == Src::Imm(0)
}

pub fn disassemble(i: &Instr) -> String {
    let m = mnemonic(i);
    let ops = operands(i);
    if ops.is_empty() { m } else { format!("{m} {ops}") }
}

fn base_name(i: &Instr) -> String {
    let s = i.short;
    let pick = |long: &str, short: &str| if s { short.to_string() } else { long.to_string() };
    match i.op {
        Op::Add => pick("add", "add_s"),
        Op::Adc => "adc".into(),
        Op::Sub => pick("sub", "sub_s"),
        Op::Sbc => "sbc".into(),
        Op::And => pick("and", "and_s"),
        Op::Or => pick("or", "or_s"),
        Op::Bic => pick("bic", "bic_s"),
        Op::Xor => pick("xor", "xor_s"),
        Op::Max => "max".into(),
        Op::Min => "min".into(),
        Op::Mov => pick("mov", "mov_s"),
        Op::Tst => pick("tst", "tst_s"),
        Op::Cmp => pick("cmp", "cmp_s"),
        Op::Rcmp => "rcmp".into(),
        Op::Rsub => "rsub".into(),
        Op::Bset => pick("bset", "bset_s"),
        Op::Bclr => pick("bclr", "bclr_s"),
        Op::Btst => pick("btst", "btst_s"),
        Op::Bxor => "bxor".into(),
        Op::Bmsk => pick("bmsk", "bmsk_s"),
        Op::Add1 => pick("add1", "add1_s"),
        Op::Add2 => pick("add2", "add2_s"),
        Op::Add3 => pick("add3", "add3_s"),
        Op::Sub1 => "sub1".into(),
        Op::Sub2 => "sub2".into(),
        Op::Sub3 => "sub3".into(),
        Op::Asl | Op::AslM => pick("asl", "asl_s"),
        Op::Asr | Op::AsrM => pick("asr", "asr_s"),
        Op::Lsr | Op::LsrM => pick("lsr", "lsr_s"),
        Op::Ror | Op::RorM => "ror".into(),
        Op::Rrc => "rrc".into(),
        Op::Sexb => pick("sexb", "sexb_s"),
        Op::Sexw => pick("sexw", "sexw_s"),
        Op::Extb => pick("extb", "extb_s"),
        Op::Extw => pick("extw", "extw_s"),
        Op::Abs => pick("abs", "abs_s"),
        Op::Not => pick("not", "not_s"),
        Op::Rlc => "rlc".into(),
        Op::Neg => "neg_s".into(),
        Op::Mul64 => pick("mul64", "mul64_s"),
        Op::Mulu64 => "mulu64".into(),
        Op::Adds => "adds".into(),
        Op::Subs => "subs".into(),
        Op::Divaw => "divaw".into(),
        Op::Asls => "asls".into(),
        Op::Asrs => "asrs".into(),
        Op::Addsdw => "addsdw".into(),
        Op::Subsdw => "subsdw".into(),
        Op::Swap => "swap".into(),
        Op::Norm => "norm".into(),
        Op::Sat16 => "sat16".into(),
        Op::Rnd16 => "rnd16".into(),
        Op::Abssw => "abssw".into(),
        Op::Abss => "abss".into(),
        Op::Negsw => "negsw".into(),
        Op::Negs => "negs".into(),
        Op::Normw => "normw".into(),
        Op::Muldw => "muldw".into(),
        Op::Macdw => "macdw".into(),
        Op::Msubdw => "msubdw".into(),
        Op::B => "b".into(),
        Op::Bl => "bl".into(),
        Op::Br(c) => format!("br{}", c.name()),
        Op::Bbit0 => "bbit0".into(),
        Op::Bbit1 => "bbit1".into(),
        Op::J => "j".into(),
        Op::Jl => "jl".into(),
        Op::Lp => "lp".into(),
        Op::Flag => "flag".into(),
        Op::Lr => "lr".into(),
        Op::Sr => "sr".into(),
        Op::Sleep => "sleep".into(),
        Op::Swi => "swi".into(),
        Op::Brk => pick("brk", "brk_s"),
        Op::Nop => pick("nop", "nop_s"),
        Op::Unimp => "unimp_s".into(),
        Op::Ld => {
            let b = match i.mem.size {
                Size::Word => "ld",
                Size::Byte => "ldb",
                Size::Half if s => "ldw",
                Size::Half => "ldh",
            };
            if s { format!("{b}_s") } else { b.into() }
        }
        Op::St => {
            let b = match i.mem.size {
                Size::Word => "st",
                Size::Byte => "stb",
                Size::Half if s => "stw",
                Size::Half => "sth",
            };
            if s { format!("{b}_s") } else { b.into() }
        }
        Op::Push => "push_s".into(),
        Op::Pop => "pop_s".into(),
        Op::Arc700Only(n) => n.into(),
        Op::Extension { major, sub } => format!("ext{major:02x}.{sub:02x}"),
        Op::Illegal(_) => "illegal".into(),
    }
}

fn cc_suffix(cc: Cond) -> String {
    match cond_name(cc) {
        Some(n) => n.into(),
        None => format!(".cc{cc:02x}"),
    }
}

pub fn mnemonic(i: &Instr) -> String {
    let mut m = if is_neg_alias(i) { "neg".to_string() } else { base_name(i) };
    if !i.short && i.word == 0x264A_7000 {
        return "nop".into();
    }
    match i.op {
        Op::B | Op::Bl | Op::J | Op::Jl | Op::Lp => {
            // Condition is spelled into the mnemonic: bne, blle, jlt, lpne;
            // 16-bit forms put it before the _s: bne_s, jeq_s.
            let cc = match i.cc {
                COND_AL => String::new(),
                // binutils spells the 16-bit carry conditions BLO_S/BHS_S (PRM Table 84).
                COND_CS if i.short => "lo".into(),
                COND_CC if i.short => "hs".into(),
                cc => cc_suffix(cc),
            };
            if i.short {
                m = format!("{m}{cc}_s");
            } else {
                m.push_str(&cc);
            }
            if i.set_flags {
                m.push_str(".f");
            }
        }
        Op::Br(_) | Op::Bbit0 | Op::Bbit1 => {
            if i.short {
                m.push_str("_s");
            }
        }
        Op::Ld | Op::St => {
            if i.mem.bypass_cache {
                m.push_str(".di");
            }
            // GNU objdump omits `.as` on `[limm]` (zero offset), where scaling
            // cannot change the address (seen in the US v2.1 image's data).
            let absolute = matches!(i.src1, Src::Limm(_)) && i.src2 == Src::Imm(0);
            m.push_str(match i.mem.wb {
                WriteBack::None => "",
                WriteBack::Pre => ".aw",
                WriteBack::Post => ".ab",
                WriteBack::Scaled if absolute => "",
                WriteBack::Scaled => ".as",
            });
            if i.mem.sign_extend {
                m.push_str(".x");
            }
        }
        _ => {
            let implicit = matches!(i.op, Op::Cmp | Op::Tst | Op::Rcmp | Op::Btst);
            if i.set_flags && !implicit && !i.short {
                m.push_str(".f");
            }
            if i.cc != COND_AL {
                m.push('.');
                m.push_str(cc_suffix(i.cc).trim_start_matches('.'));
            }
        }
    }
    if i.delay {
        m.push_str(".d");
    }
    m
}

fn src(s: Src) -> String {
    match s {
        Src::None => String::new(),
        Src::Reg(r) => reg_name(r),
        Src::Limm(v) => format!("{v:#x}"),
        Src::Imm(v) => {
            // Negative and small values in decimal, the rest in hex.
            let n = v as i32;
            if n < 10 { n.to_string() } else { format!("{v:#x}") }
        }
    }
}

fn dst(d: Dst) -> String {
    match d {
        Dst::None => String::new(),
        Dst::Reg(r) => reg_name(r),
        Dst::Discard => "0".into(),
    }
}

pub fn operands(i: &Instr) -> String {
    let join = |parts: &[String]| parts.iter().filter(|p| !p.is_empty()).cloned().collect::<Vec<_>>().join(",");
    match i.op {
        _ if !i.short && i.word == 0x264A_7000 => String::new(),
        _ if is_neg_alias(i) => join(&[dst(i.dst), src(i.src1)]),
        Op::B | Op::Bl | Op::Lp => format!("{:#x}", i.target),
        Op::Br(_) | Op::Bbit0 | Op::Bbit1 => join(&[src(i.src1), src(i.src2), format!("{:#x}", i.target)]),
        Op::J | Op::Jl => match i.src2 {
            Src::Reg(r) => format!("[{}]", reg_name(r)),
            s => src(s),
        },
        Op::Mov
        | Op::Asl
        | Op::Asr
        | Op::Lsr
        | Op::Ror
        | Op::Rrc
        | Op::Sexb
        | Op::Sexw
        | Op::Extb
        | Op::Extw
        | Op::Abs
        | Op::Not
        | Op::Rlc
        | Op::Neg
        | Op::Swap
        | Op::Norm
        | Op::Sat16
        | Op::Rnd16
        | Op::Abssw
        | Op::Abss
        | Op::Negsw
        | Op::Negs
        | Op::Normw => join(&[dst(i.dst), src(i.src2)]),
        Op::Tst | Op::Cmp | Op::Rcmp | Op::Btst => join(&[src(i.src1), src(i.src2)]),
        Op::Mul64 | Op::Mulu64 => join(&["0".into(), src(i.src1), src(i.src2)]),
        Op::Flag | Op::Sleep => src(i.src2),
        Op::Lr => format!("{},[{}]", dst(i.dst), aux_operand(i.src2)),
        Op::Sr => format!("{},[{}]", src(i.src1), aux_operand(i.src2)),
        Op::Ld => format!("{},[{}]", dst(i.dst), mem_operand(i)),
        Op::St => format!("{},[{}]", src(i.data), mem_operand(i)),
        Op::Push => src(i.data),
        Op::Pop => dst(i.dst),
        Op::Swi | Op::Brk | Op::Nop | Op::Unimp => String::new(),
        Op::Illegal(why) => format!("{:#010x} ; {why}", i.word),
        Op::Arc700Only(_) | Op::Extension { .. } => {
            let mut s = join(&[dst(i.dst), src(i.src1), src(i.src2)]);
            let _ = write!(s, " ; {:#010x} not an ARCtangent-A5 basecase instruction", i.word);
            s
        }
        _ => join(&[dst(i.dst), src(i.src1), src(i.src2)]),
    }
}

fn aux_operand(s: Src) -> String {
    match s {
        Src::Imm(n) | Src::Limm(n) => aux_name(n).map_or_else(|| src(s), str::to_string),
        _ => src(s),
    }
}

fn mem_operand(i: &Instr) -> String {
    match (i.src1, i.src2) {
        (Src::Limm(v), Src::Imm(0)) => format!("{v:#x}"),
        (b, o) => format!("{},{}", src(b), src(o)),
    }
}

// ---------------------------------------------------------------------------
// Canonical form for comparing against GNU objdump.

fn reg_number(tok: &str) -> Option<u32> {
    let named = match tok {
        "gp" => Some(26),
        "fp" => Some(27),
        "sp" => Some(28),
        "ilink1" | "ilink" => Some(29),
        "ilink2" => Some(30),
        "blink" => Some(31),
        "mlo" => Some(57),
        "mmid" => Some(58),
        "mhi" => Some(59),
        "lp_count" => Some(60),
        "reserved" | "r61" => Some(61),
        "pcl" => Some(63),
        _ => None,
    };
    named.or_else(|| tok.strip_prefix('r').and_then(|n| n.parse().ok()).filter(|&n| n < 64))
}

fn parse_number(tok: &str) -> Option<u32> {
    let (neg, t) = match tok.strip_prefix('-') {
        Some(t) => (true, t),
        None => (false, tok),
    };
    let v = match t.strip_prefix("0x") {
        Some(h) => u32::from_str_radix(h, 16).ok()?,
        None => t.parse::<u64>().ok().map(|v| v as u32)?,
    };
    Some(if neg { v.wrapping_neg() } else { v })
}

/// Reduces a disassembly line (`mnemonic operands [;comment]`) to a form that
/// is independent of register aliases, number bases and bracket placement.
/// For branch-class mnemonics a `;0x...` comment (objdump's computed target)
/// replaces the last operand.
pub fn canonical(line: &str) -> String {
    let line = line.trim();
    let (mn, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
    let (ops, comment) = match rest.split_once(';') {
        Some((o, c)) => (o.trim(), Some(c.trim())),
        None => (rest.trim(), None),
    };
    let mut toks: Vec<String> =
        ops.replace(['[', ']'], ",").split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect();
    let branchy = mn.starts_with('b')
        && !mn.starts_with("bclr")
        && !mn.starts_with("bset")
        && !mn.starts_with("bic")
        && !mn.starts_with("bmsk")
        && !mn.starts_with("btst")
        && !mn.starts_with("bxor")
        && !mn.starts_with("brk")
        || mn.starts_with("lp");
    if branchy
        && let Some(c) = comment
        && let Some(t) = c.split_whitespace().next()
        && let Ok(target) = u32::from_str_radix(t.trim_start_matches("0x"), 16)
    {
        let t = format!("{target:#x}");
        if let Some(last) = toks.last_mut() {
            *last = t.to_string();
        } else {
            toks.push(t.to_string());
        }
    }
    let memory = mn.starts_with("ld") || mn.starts_with("st");
    if memory && toks.len() == 2 {
        toks.push("0".into());
    }
    let canon: Vec<String> = toks
        .iter()
        .map(|t| match (reg_number(t), parse_number(t), AUX_NAMES.iter().find(|(_, n)| n == t)) {
            (Some(r), _, _) => format!("r{r}"),
            (None, Some(v), _) => format!("#{v:08x}"),
            (None, None, Some((v, _))) => format!("#{v:08x}"),
            _ => t.clone(),
        })
        .collect();
    format!("{mn} {}", canon.join(","))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_with;

    fn dis(addr: u32, halves: &[u16]) -> String {
        let bytes: Vec<u8> = halves.iter().flat_map(|h| h.to_le_bytes()).collect();
        let i = decode_with(addr, |a| {
            let o = (a - addr) as usize;
            u16::from_le_bytes([bytes.get(o).copied().unwrap_or(0), bytes.get(o + 1).copied().unwrap_or(0)])
        });
        disassemble(&i)
    }

    #[test]
    fn matches_objdump_after_canonicalisation() {
        // (address, halfwords, GNU objdump text)
        let cases: &[(u32, &[u16], &str)] = &[
            (0x4000_0000, &[0x2020, 0x0f80, 0x4002, 0xb5f4], "j\t0x4002b5f4"),
            (0x4002_b5fc, &[0x085e, 0x0020], "bl.d\t92\t;0x4002b658"),
            (0x4002_b61c, &[0x1600, 0x7800, 0x0180, 0xb000], "ld.di\tr0,[0x180b000]"),
            (0x4002_b624, &[0x2085, 0x0004], "or\tr0,r0,256"),
            (0x4002_b66c, &[0x1ec0, 0x1040], "st\tr1,[r14,192]"),
            (0x4000_0100, &[0x6f43], "add_s\tr2,r15,0x3"),
            (0x4000_11bc, &[0x9b7a], "ldw_s.x\tr3,[r3,0x34]"),
            (0x4002_0114, &[0x20ca, 0x0062], "mov.ne\tr0,0x1"),
            (0x4000_e0b8, &[0x79c0], "sub_s.ne\tr1,r1,r1"),
            (0x4007_83a0, &[0x7ce0], "jeq_s\t[blink]"),
            (0x4000_1922, &[0xc2d1], "pop_s\tblink"),
            (0x4000_3204, &[0x7cac], "mul64_s\t0,r12,r13"),
            (0x4008_4c14, &[0x20e8, 0x04a2], "lpne\t0x24\t;0x40084c38"),
            (0x4000_0e16, &[0x08cb, 0x7571], "brne.d\tr56,0x15,202\t;0x40000ede"),
            (0x4000_23e0, &[0x145a, 0xb021], "ld\tr33,[sp,-166]"),
            (0x4008_9af8, &[0x264a, 0x7000], "nop"),
            (0x4000_08fe, &[0x7fff], "brk_s"),
            (0x4007_3e96, &[0x2020, 0x8780], "j.f\t[ilink2]"),
        ];
        for &(addr, halves, objdump) in cases {
            let mine = dis(addr, halves);
            assert_eq!(canonical(&mine), canonical(objdump), "at {addr:#x}: ribbit `{mine}` vs objdump `{objdump}`");
        }
    }

    #[test]
    fn prm_illegal_encodings_that_objdump_accepts() {
        // objdump prints `ld.ab r57,[0xf1cb39ba,-121]`, but the PRM (Load
        // Register with Offset) says write-back with a long-immediate base
        // raises an Instruction Error. Ribbit follows the PRM.
        assert!(dis(0x4000_1a30, &[0x1687, 0xf439, 0xf1cb, 0x39ba]).starts_with("illegal"));
    }

    #[test]
    fn canonical_normalises_aliases_and_bases() {
        assert_eq!(canonical("mov sp,0x3000800"), canonical("mov r28,50333696"));
        assert_eq!(canonical("ld r0,[r1]"), canonical("ld r0,[r1,0]"));
        assert_eq!(canonical("bne_s -108\t;0x40001188"), "bne_s #40001188");
    }
}
