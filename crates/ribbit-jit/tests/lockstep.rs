//! The JIT against the interpreter, instruction by instruction.
//!
//! Instructions come from the real decoder (random encodings, and the user's
//! BIOS when it is there), so every encoding the fast forms accept is
//! covered, limm and 16-bit forms included. Each case runs the same code
//! from the same state twice: compiled (`Block::run`) and in the general
//! executor (`Cpu::step`, the reference: not the batched `execute_fast`),
//! then compares every core register, STATUS32, the PC, a pending delay
//! slot, all of data memory, the cycle count and the pipeline scoreboard
//! (the test bus has no memory stalls).

#![cfg(target_arch = "x86_64")]

use ribbit_cpu::decode::decode_with;
use ribbit_cpu::disasm::disassemble;
use ribbit_cpu::icache::{AluOp, Fast, Hot};
use ribbit_cpu::isa::Instr;
use ribbit_cpu::{Cpu, CpuConfig, status32};
use ribbit_jit::{ExitKind, compile};
use ribbit_memory::Bus;
use std::collections::BTreeMap;
use std::sync::{Arc, OnceLock};

const CODE: u32 = 0x0000_1000;
const DATA_MASK: u32 = 0xFFFF;

/// Code (instruction fetches) and data kept apart, so stores never change
/// the code the JIT compiled. Data wraps at 64 KiB; the alignment rules are
/// `SystemBus`'s.
#[derive(Clone)]
struct TestBus {
    code_base: u32,
    code: Arc<Vec<u8>>,
    data: Vec<u8>,
}

/// The same random data for every case (registers vary per case).
fn data() -> &'static Vec<u8> {
    static DATA: OnceLock<Vec<u8>> = OnceLock::new();
    DATA.get_or_init(|| {
        let mut rng = Rng(0xDA7A);
        (0..=DATA_MASK).map(|_| rng.next() as u8).collect()
    })
}

impl TestBus {
    fn new(code_base: u32, code: Arc<Vec<u8>>) -> Self {
        TestBus { code_base, code, data: data().clone() }
    }
    fn d(&self, a: u32) -> u8 {
        self.data[(a & DATA_MASK) as usize]
    }
}

impl Bus for TestBus {
    fn read8(&mut self, a: u32) -> u8 {
        self.d(a)
    }
    fn read16(&mut self, a: u32) -> u16 {
        let a = a & !1;
        u16::from_le_bytes([self.d(a), self.d(a.wrapping_add(1))])
    }
    fn read32(&mut self, a: u32) -> u32 {
        let a = a & !3;
        u32::from_le_bytes([self.d(a), self.d(a.wrapping_add(1)), self.d(a.wrapping_add(2)), self.d(a.wrapping_add(3))])
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.data[(a & DATA_MASK) as usize] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let a = a & !1;
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.data[(a.wrapping_add(i as u32) & DATA_MASK) as usize] = b;
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        let a = a & !3;
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.data[(a.wrapping_add(i as u32) & DATA_MASK) as usize] = b;
        }
    }
    fn fetch16(&mut self, a: u32) -> u16 {
        fetch(&self.code, self.code_base, a)
    }
}

fn fetch(code: &[u8], base: u32, a: u32) -> u16 {
    let o = a.wrapping_sub(base) as usize;
    match (code.get(o), code.get(o + 1)) {
        (Some(&lo), Some(&hi)) => u16::from_le_bytes([lo, hi]),
        _ => 0,
    }
}

/// xorshift64* (deterministic).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
    /// Register values, edge cases often.
    fn value(&mut self) -> u32 {
        const EDGES: [u32; 10] = [0, 1, 2, 31, 32, 0x7FFF_FFFF, 0x8000_0000, 0xFFFF_FFFF, 0xFFFF_FFFE, 0x0000_8000];
        match self.below(4) {
            0 => EDGES[self.below(EDGES.len())],
            1 => self.below(64) as u32,
            _ => self.next() as u32,
        }
    }
}

/// An encoding the fast forms accept: its bytes, and whether it branches.
#[derive(Clone)]
struct Enc {
    bytes: Vec<u8>,
    branch: bool,
    kind: String,
}

fn kind_of(f: Fast) -> String {
    let s = format!("{f:?}");
    let name = s.split([' ', '{']).next().unwrap().to_string();
    // ALU forms by operation too, so every AluOp is checked.
    let op = |op: AluOp| format!("{name}:{op:?}");
    match f {
        Fast::Alu { op: o, .. }
        | Fast::AluI { op: o, .. }
        | Fast::AluCc { op: o, .. }
        | Fast::AluCcI { op: o, .. }
        | Fast::AluF { op: o, .. }
        | Fast::AluFI { op: o, .. } => op(o),
        Fast::Un { op: o, .. } => format!("{name}:{o:?}"),
        Fast::Rot { op: o, f, .. } => format!("{name}:{o:?}:{f}"),
        _ => name,
    }
}

fn is_branch(f: Fast) -> bool {
    matches!(
        f,
        Fast::Br { .. } | Fast::BrR { .. } | Fast::B { .. } | Fast::Bl { .. } | Fast::BlD { .. } | Fast::J { .. } | Fast::Jcc { .. } | Fast::Jl { .. } | Fast::JlD { .. }
    )
}

/// Random encodings, decoded, kept when they have a fast form; at most
/// `per_kind` of each kind, so rare shapes are tested as much as common ones.
fn pool(rng: &mut Rng, tries: usize, per_kind: usize) -> Vec<Enc> {
    let mut out = Vec::new();
    let mut seen: BTreeMap<String, usize> = BTreeMap::new();
    for _ in 0..tries {
        let bytes: Vec<u8> = (0..8).map(|_| rng.next() as u8).collect();
        let i = decode_with(CODE, |a| fetch(&bytes, CODE, a));
        let h = Hot::of(&i);
        if h.stop || h.fast == Fast::General {
            continue;
        }
        let kind = kind_of(h.fast);
        let n = seen.entry(kind.clone()).or_default();
        if *n >= per_kind {
            continue;
        }
        *n += 1;
        out.push(Enc { bytes: bytes[..i.len as usize].to_vec(), branch: is_branch(h.fast), kind });
    }
    out
}

/// Lays `encs` out from `CODE` and decodes them there.
fn layout(encs: &[&Enc]) -> (Arc<Vec<u8>>, Vec<Instr>) {
    let code: Vec<u8> = encs.iter().flat_map(|e| e.bytes.iter().copied()).collect();
    let mut instrs = Vec::new();
    let mut pc = CODE;
    for _ in encs {
        let i = decode_with(pc, |a| fetch(&code, CODE, a));
        pc = i.next();
        instrs.push(i);
    }
    (Arc::new(code), instrs)
}

fn random_cpu(rng: &mut Rng, pc: u32) -> Cpu {
    let mut cpu = Cpu::new(CpuConfig::leapster());
    // Stack and data pointers anywhere: the test bus wraps.
    for r in cpu.regs.iter_mut().take(32) {
        *r = rng.value();
    }
    cpu.status32 = (rng.next() as u32) & (status32::Z | status32::N | status32::C | status32::V);
    cpu.lp_start = rng.next() as u32 & !1;
    cpu.lp_end = 0;
    cpu.regs[60] = rng.value();
    cpu.pc = pc;
    cpu
}

/// Runs `instrs` (code bytes `code` at `base`) both ways from `cpu`;
/// panics with both states on any difference. Returns the exit kind and
/// the number of instructions compiled (None: nothing compiled).
fn check(instrs: &[Instr], code: Arc<Vec<u8>>, base: u32, cpu: Cpu, what: &str) -> Option<(ExitKind, usize)> {
    let block = compile(instrs).ok()?;
    let mut bus = TestBus::new(base, code);
    let (mut ci, mut bi) = (cpu.clone(), bus.clone());
    let mut cj = cpu.clone();
    let exit = block.run(&mut cj, &mut bus);
    for n in 0..exit.executed {
        if let Err(e) = ci.step(&mut bi) {
            panic!("{what}: interpreter failed at step {n}: {e}\n{}", listing(instrs));
        }
    }
    let same = ci.regs == cj.regs
        && ci.status32 == cj.status32
        && ci.pc == cj.pc
        && ci.delay_target == cj.delay_target
        && bi.data == bus.data
        && ci.cycles == exit.now
        && ci.ready == cj.ready
        && ci.ready_max == cj.ready_max;
    if !same {
        let mem = bi.data.iter().zip(&bus.data).position(|(a, b)| a != b);
        eprintln!("cycles: interpreter {}, jit {}; ready interpreter {:?} jit {:?}", ci.cycles, exit.now, ci.ready_max, cj.ready_max);
        panic!(
            "{what}: JIT differs from the interpreter after {exit:?}\n{}\nstart:\n{cpu:?}\ninterpreter:\n{ci:?}\njit:\n{cj:?}\nfirst memory difference at {mem:x?}",
            listing(instrs)
        );
    }
    Some((exit.kind, block.compiled))
}

fn listing(instrs: &[Instr]) -> String {
    instrs.iter().map(|i| format!("  {:08x}: {}", i.addr, disassemble(i))).collect::<Vec<_>>().join("\n")
}

#[test]
fn every_fast_form_matches_the_interpreter_alone() {
    let mut rng = Rng(0x5EED_0001);
    let pool = pool(&mut rng, 8_000_000, 400);
    let mut kinds: BTreeMap<String, usize> = BTreeMap::new();
    for e in &pool {
        *kinds.entry(e.kind.clone()).or_default() += 1;
    }
    // Every form and every ALU operation must have been generated.
    let ops = [
        "Add", "Add1", "Add2", "Add3", "Sub", "Sub1", "Sub2", "Sub3", "Rsub", "And", "Or", "Xor", "Bic", "Asl", "Lsr", "Asr", "Adc", "Sbc",
    ];
    for form in ["Alu", "AluI", "AluF", "AluFI", "AluCc", "AluCcI"] {
        for op in ops {
            assert!(kinds.contains_key(&format!("{form}:{op}")), "no {form}:{op} generated; kinds: {kinds:?}");
        }
    }
    for k in [
        "Mov", "MovI", "Ld", "LdR", "LdX", "St", "StX", "Push", "Pop", "Br", "BrR", "B", "Bl", "BlD", "J", "Jcc", "Cmp", "CmpI", "Rot", "Mul64", "Mul64I", "Jl",
        "JlD",
    ] {
        assert!(kinds.keys().any(|n| n == k || n.starts_with(&format!("{k}:"))), "no {k} generated; kinds: {kinds:?}");
    }
    let mut cases = 0;
    for e in &pool {
        // Each encoding alone, from several random states (a lone BL.D
        // falls back: its slot's length is unknown).
        for _ in 0..3 {
            let (code, instrs) = layout(&[e]);
            let cpu = random_cpu(&mut rng, CODE);
            if check(&instrs, code, CODE, cpu, &e.kind).is_some() {
                cases += 1;
            }
        }
    }
    eprintln!("{} encodings, {} kinds, {cases} single-instruction cases", pool.len(), kinds.len());
    assert!(cases > 30_000);
}

/// Rotates and shifts by one are rare among random encodings: searched for
/// on their own, every operation with and without .F.
#[test]
fn rotates_match_the_interpreter() {
    let mut rng = Rng(0x5EED_0004);
    let mut found: BTreeMap<String, Vec<Enc>> = BTreeMap::new();
    for _ in 0..30_000_000 {
        let bytes: Vec<u8> = (0..8).map(|_| rng.next() as u8).collect();
        let i = decode_with(CODE, |a| fetch(&bytes, CODE, a));
        let h = Hot::of(&i);
        if h.stop || !matches!(h.fast, Fast::Rot { .. }) {
            continue;
        }
        let list = found.entry(kind_of(h.fast)).or_default();
        if list.len() < 60 {
            list.push(Enc { bytes: bytes[..i.len as usize].to_vec(), branch: false, kind: kind_of(h.fast) });
        }
    }
    let mut cases = 0;
    for e in found.values().flatten() {
        for _ in 0..20 {
            let (code, instrs) = layout(&[e]);
            let cpu = random_cpu(&mut rng, CODE);
            if check(&instrs, code, CODE, cpu, &e.kind).is_some() {
                cases += 1;
            }
        }
    }
    eprintln!("rotate kinds {:?}, {cases} cases", found.iter().map(|(k, v)| (k, v.len())).collect::<Vec<_>>());
    for op in ["Asl", "Asr", "Lsr", "Ror", "Rrc", "Rlc"] {
        assert!(found.keys().any(|k| k.starts_with(&format!("Rot:{op}:"))), "no {op} found: {:?}", found.keys());
    }
    assert!(cases > 500);
}

#[test]
fn straight_runs_with_a_branch_and_delay_slot_match() {
    let mut rng = Rng(0x5EED_0002);
    let pool = pool(&mut rng, 2_000_000, 300);
    let straight: Vec<&Enc> = pool.iter().filter(|e| !e.branch).collect();
    let branches: Vec<&Enc> = pool.iter().filter(|e| e.branch).collect();
    let (mut cases, mut kinds) = (0, BTreeMap::<String, usize>::new());
    for n in 0..30_000 {
        let mut encs: Vec<&Enc> = (0..rng.below(13)).map(|_| straight[rng.below(straight.len())]).collect();
        if rng.below(3) != 0 {
            encs.push(branches[rng.below(branches.len())]);
            // A delay slot (used only if the branch has .D).
            encs.push(straight[rng.below(straight.len())]);
        }
        if encs.is_empty() {
            continue;
        }
        let (code, instrs) = layout(&encs);
        let mut cpu = random_cpu(&mut rng, CODE);
        // Now and then a zero-overhead loop ending inside the run.
        if rng.below(4) == 0 {
            cpu.lp_end = instrs[rng.below(instrs.len())].next();
            cpu.regs[60] = [0, 1, 2, 3, 0xFFFF_FFFF][rng.below(5)];
        }
        if let Some((kind, _)) = check(&instrs, code, CODE, cpu, &format!("run {n}")) {
            cases += 1;
            *kinds.entry(format!("{kind:?}")).or_default() += 1;
        }
    }
    eprintln!("{cases} runs; exits {kinds:?}");
    assert!(cases > 25_000);
    assert!(kinds.contains_key("Flow"), "no Flow exit seen: {kinds:?}");
}

/// Straight runs from the user's own BIOS (roms/bios/152-11265_2.1.bin in
/// the main checkout, or RIBBIT_BIOS): real code, random states.
#[test]
fn runs_from_the_bios_match() {
    let path = std::env::var("RIBBIT_BIOS").unwrap_or_else(|_| "C:/Users/Home/Desktop/Ribbit/roms/bios/152-11265_2.1.bin".into());
    let Ok(bios) = std::fs::read(&path) else {
        eprintln!("skipped: no BIOS at {path}");
        return;
    };
    let bios = Arc::new(bios);
    const BASE: u32 = 0x4000_0000;
    let mut rng = Rng(0x5EED_0003);
    let (mut cases, mut fallbacks, mut compiled_total) = (0, 0, 0u64);
    for _ in 0..40_000 {
        let start = BASE + (rng.below(bios.len() / 2 - 64) as u32) * 2;
        let mut instrs = Vec::new();
        let mut pc = start;
        for _ in 0..24 {
            let i = decode_with(pc, |a| fetch(&bios, BASE, a));
            pc = i.next();
            instrs.push(i);
        }
        let cpu = random_cpu(&mut rng, start);
        if let Some((kind, compiled)) = check(&instrs, bios.clone(), BASE, cpu, &format!("bios {start:#x}")) {
            cases += 1;
            fallbacks += (kind == ExitKind::Fallback) as u32;
            compiled_total += compiled as u64;
        }
    }
    eprintln!("{cases} BIOS runs ({fallbacks} ended in a fallback), {compiled_total} instructions compiled");
    assert!(cases > 10_000);
}
