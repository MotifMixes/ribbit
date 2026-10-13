//! Differential decoder tests against GNU binutils (arc-elf32-objdump,
//! ARCompact toolchain 2.45.1, ARC600 profile). Fixtures are produced by
//! scripts/gen-cpu-vectors.py; see docs/isa-arcompact.md for why the two
//! decoders are allowed to disagree on specific encodings.
//!
//! Set RIBBIT_ORACLE_VERBOSE=1 to print every disagreement.

use ribbit_cpu::decode::decode_with;
use ribbit_cpu::disasm::{canonical, disassemble};
use ribbit_cpu::isa::Op;
use std::collections::BTreeMap;

struct Line {
    addr: u32,
    halves: Vec<u16>,
    text: String,
}

fn parse(listing: &str) -> Vec<Line> {
    let mut out = Vec::new();
    for raw in listing.lines() {
        let parts: Vec<&str> = raw.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }
        let Some(addr) = parts[0].trim().strip_suffix(':').and_then(|a| u32::from_str_radix(a, 16).ok()) else {
            continue;
        };
        let halves: Option<Vec<u16>> = parts[1].split_whitespace().map(|h| u16::from_str_radix(h, 16).ok()).collect();
        let Some(halves) = halves.filter(|h| !h.is_empty()) else { continue };
        out.push(Line { addr, halves, text: parts[2..].join(" ") });
    }
    out
}

fn image(lines: &[Line]) -> (u32, Vec<u8>) {
    let base = lines.first().map_or(0, |l| l.addr);
    let end = lines.iter().map(|l| l.addr + 2 * l.halves.len() as u32).max().unwrap_or(base);
    let mut img = vec![0u8; (end - base) as usize + 8];
    for l in lines {
        for (k, h) in l.halves.iter().enumerate() {
            let o = (l.addr - base) as usize + 2 * k;
            img[o..o + 2].copy_from_slice(&h.to_le_bytes());
        }
    }
    (base, img)
}

#[derive(Default, Debug)]
struct Report {
    total: usize,
    agree: usize,
    both_reject: usize,
    length_mismatch: Vec<String>,
    text_mismatch: Vec<String>,
    /// Extension condition codes (0x10-0x1F): objdump omits most of them and
    /// names a few (nj, nm, nt); compared with the condition removed.
    ext_cond: usize,
    ribbit_rejects: BTreeMap<String, usize>,
    /// Ribbit decodes, objdump rejects, for a reason documented below.
    accepted_by_rule: BTreeMap<&'static str, usize>,
    /// Ribbit decodes, objdump rejects, for no documented reason (a bug).
    ribbit_accepts: Vec<String>,
}

/// Encodings GNU objdump rejects but the PRM defines. See docs/isa-arcompact.md.
fn accepted_reason(i: &ribbit_cpu::isa::Instr) -> Option<&'static str> {
    let w = i.word;
    let p = (w >> 22) & 3;
    match i.op {
        // The A field of MOV/TST/CMP/RCMP/BTST/LR/SR is reserved; "The processor
        // will ignore reserved bits" (PRM, Use of Reserved Encodings). objdump
        // insists on zero.
        Op::Tst | Op::Cmp | Op::Rcmp | Op::Btst | Op::Lr | Op::Sr if !i.short && p <= 1 && w & 0x3F != 0 => {
            Some("reserved A field non-zero")
        }
        // ST with a long-immediate base and an offset: the PRM's ST syntax
        // only lists [limm], but the address is still src2 + src3.
        Op::St if matches!(i.src1, ribbit_cpu::isa::Src::Limm(_)) => Some("ST [limm,offset]"),
        // PREFETCH is LD with a = limm (PRM, PREFETCH); binutils' ARC600
        // profile rejects it.
        Op::Ld if i.dst == ribbit_cpu::isa::Dst::Discard => Some("PREFETCH"),
        // "J.F<.D> [ILINK1]" is the PRM's interrupt return; binutils rejects .D.
        Op::J if i.set_flags && i.delay => Some("J.F.D [ILINKn]"),
        // UNIMP_S is a PRM basecase instruction; binutils omits it for ARC600.
        Op::Unimp => Some("UNIMP_S"),
        // "The destination is always ignored for the multiply instruction"
        // (PRM, MUL64); binutils insists on A = 62.
        Op::Mul64 | Op::Mulu64 if !i.short && p <= 1 && w & 0x3F != 62 => Some("MUL64 A field ignored"),
        // SWI and BRK "take no operands" (PRM, SWI); binutils rejects the
        // 32-bit forms unless the unused C/u6 field has its canonical value.
        Op::Swi | Op::Brk if !i.short => Some("SWI/BRK unused operand field"),
        _ => None,
    }
}

/// objdump's aux-register spellings (a superset across all ARC cores, used
/// only to translate its output back to numbers).
fn objdump_aux_names() -> Vec<(String, u32)> {
    parse(include_str!("vectors/aux-regs.objdump"))
        .iter()
        .filter_map(|l| {
            let name = l.text.split('[').nth(1)?.trim_end_matches(']').to_string();
            let numeric = name.starts_with("0x") || name.starts_with('-') || name.chars().all(|c| c.is_ascii_digit());
            (!numeric).then_some((name, l.addr / 4))
        })
        .collect()
}

fn translate_aux(text: &str, names: &[(String, u32)]) -> String {
    let Some((head, tail)) = text.split_once('[') else { return text.to_string() };
    let Some((name, rest)) = tail.split_once(']') else { return text.to_string() };
    match names.iter().find(|(n, _)| n == name) {
        Some((_, v)) if head.starts_with("lr") || head.starts_with("sr") => format!("{head}[{v:#x}]{rest}"),
        _ => text.to_string(),
    }
}

fn compare(listing: &str) -> Report {
    let aux = objdump_aux_names();
    let lines = parse(listing);
    let (base, img) = image(&lines);
    let mut r = Report::default();
    for l in &lines {
        r.total += 1;
        let i = decode_with(l.addr, |a| {
            let o = a.wrapping_sub(base) as usize;
            u16::from_le_bytes([img.get(o).copied().unwrap_or(0), img.get(o + 1).copied().unwrap_or(0)])
        });
        let mine = translate_aux(&disassemble(&i), &aux);
        let l = &Line { addr: l.addr, halves: l.halves.clone(), text: translate_aux(&l.text, &aux) };
        let objdump_invalid =
            l.text.starts_with(".word") || l.text.starts_with(".short") || l.text.starts_with(".long");
        let ribbit_invalid = matches!(i.op, Op::Illegal(_) | Op::Arc700Only(_) | Op::Extension { .. });
        let where_ = format!("{:#010x} {:04x?}: objdump `{}` | ribbit `{}`", l.addr, l.halves, l.text, mine);
        match (objdump_invalid, ribbit_invalid) {
            (true, true) => r.both_reject += 1,
            (true, false) => match accepted_reason(&i) {
                Some(reason) => *r.accepted_by_rule.entry(reason).or_default() += 1,
                None => r.ribbit_accepts.push(where_),
            },
            (false, true) => {
                let key = l.text.split_whitespace().next().unwrap_or("").to_string();
                *r.ribbit_rejects
                    .entry(format!("{key} -> {}", mine.split_whitespace().next().unwrap_or("")))
                    .or_default() += 1;
            }
            (false, false) => {
                if i.len as usize != 2 * l.halves.len() {
                    r.length_mismatch.push(where_);
                } else if i.cc >= 0x10 {
                    let strip = |t: &str| {
                        let (m, rest) = t.split_once(' ').unwrap_or((t, ""));
                        let m: Vec<&str> = m
                            .split('.')
                            .filter(|p| !p.starts_with("cc1") && !matches!(*p, "nj" | "nm" | "nt"))
                            .collect();
                        let m = m.join(".");
                        let m = ["bnj", "bnm", "bnt"].iter().fold(m, |m, p| match m.strip_prefix(p) {
                            Some(rest) => format!("b{rest}"),
                            None => m,
                        });
                        format!("{m} {rest}")
                    };
                    if strip(&canonical(&mine)) == strip(&canonical(&l.text)) {
                        r.ext_cond += 1;
                    } else {
                        r.text_mismatch.push(format!(
                            "{where_}
      {} != {}",
                            canonical(&mine),
                            canonical(&l.text)
                        ));
                    }
                } else if canonical(&mine) != canonical(&l.text) {
                    r.text_mismatch.push(format!("{where_}\n      {} != {}", canonical(&mine), canonical(&l.text)));
                } else {
                    r.agree += 1;
                }
            }
        }
    }
    if std::env::var_os("RIBBIT_ORACLE_VERBOSE").is_some() {
        for m in r.length_mismatch.iter().chain(&r.text_mismatch).chain(&r.ribbit_accepts) {
            eprintln!("{m}");
        }
        eprintln!("{:#?}", r.ribbit_rejects);
    }
    eprintln!(
        "total {} agree {} ext-cond {} both-reject {} length-mismatch {} text-mismatch {} ribbit-rejects {} ribbit-accepts {}",
        r.total,
        r.agree,
        r.ext_cond,
        r.both_reject,
        r.length_mismatch.len(),
        r.text_mismatch.len(),
        r.ribbit_rejects.values().sum::<usize>(),
        r.ribbit_accepts.len()
    );
    r
}

#[test]
fn assembled_corpus_matches_binutils_exactly() {
    let r = compare(include_str!("vectors/corpus.objdump"));
    assert!(r.total > 250, "fixture looks truncated");
    assert!(r.length_mismatch.is_empty(), "{:#?}", r.length_mismatch);
    assert!(r.text_mismatch.is_empty(), "{:#?}", r.text_mismatch);
    assert!(r.ribbit_accepts.is_empty(), "{:#?}", r.ribbit_accepts);
    assert!(r.ribbit_rejects.is_empty(), "{:#?}", r.ribbit_rejects);
    assert_eq!(r.agree, r.total);
}

#[test]
fn random_stream_matches_binutils() {
    let r = compare(include_str!("vectors/fuzz.objdump"));
    assert!(r.total > 20_000, "fixture looks truncated");
    assert!(r.length_mismatch.is_empty(), "{:#?}", &r.length_mismatch[..r.length_mismatch.len().min(20)]);
    assert!(r.text_mismatch.is_empty(), "{:#?}", &r.text_mismatch[..r.text_mismatch.len().min(20)]);
    assert!(r.ribbit_accepts.is_empty(), "{:#?}", r.ribbit_accepts);
    eprintln!("accepted by documented rule: {:?}", r.accepted_by_rule);
}

/// Local only: compares against an objdump listing of the user's own BIOS,
/// e.g. `arc-elf32-objdump -D -b binary -m ARC600 -EL --adjust-vma=0x40000000
/// bios.bin > bios.dis`, then RIBBIT_BIOS_OBJDUMP=bios.dis. The linear sweep
/// also decodes data, so only the strict classes are asserted.
#[test]
fn user_bios_sweep_matches_binutils() {
    let Some(path) = std::env::var_os("RIBBIT_BIOS_OBJDUMP") else {
        eprintln!("RIBBIT_BIOS_OBJDUMP not set; skipping");
        return;
    };
    let listing = std::fs::read_to_string(&path).expect("read listing");
    let r = compare(&listing);
    assert!(r.length_mismatch.is_empty(), "{:#?}", &r.length_mismatch[..r.length_mismatch.len().min(20)]);
    assert!(r.text_mismatch.is_empty(), "{:#?}", &r.text_mismatch[..r.text_mismatch.len().min(20)]);
    assert!(r.ribbit_accepts.is_empty(), "{:#?}", &r.ribbit_accepts[..r.ribbit_accepts.len().min(20)]);
    eprintln!("accepted by documented rule: {:?}", r.accepted_by_rule);
}
