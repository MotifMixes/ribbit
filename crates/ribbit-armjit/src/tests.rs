//! Lockstep: random programs (mostly instructions the generator handles,
//! some anything at all) run by the interpreter and by the JIT must leave
//! the same processor state and memory, cycle for cycle.

use super::*;

const RAM: usize = 0x1_0000;

#[derive(Clone)]
struct Flat {
    ram: Vec<u8>,
    watch: CodeWatch,
}

impl Flat {
    fn at(addr: u32) -> usize {
        addr as usize & (RAM - 1)
    }
}

impl ArmBus for Flat {
    fn read8(&mut self, a: u32) -> u8 {
        self.ram[Self::at(a)]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let a = Self::at(a) & !1;
        u16::from_le_bytes([self.ram[a], self.ram[a + 1]])
    }
    fn read32(&mut self, a: u32) -> u32 {
        let a = Self::at(a) & !3;
        u32::from_le_bytes(self.ram[a..a + 4].try_into().unwrap())
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.ram[Self::at(a)] = v;
        self.watch.wrote(Self::at(a));
    }
    fn write16(&mut self, a: u32, v: u16) {
        let a = Self::at(a) & !1;
        self.ram[a..a + 2].copy_from_slice(&v.to_le_bytes());
        self.watch.wrote(a);
    }
    fn write32(&mut self, a: u32, v: u32) {
        let a = Self::at(a) & !3;
        self.ram[a..a + 4].copy_from_slice(&v.to_le_bytes());
        self.watch.wrote(a);
    }
    fn code_watch(&mut self) -> Option<&mut CodeWatch> {
        Some(&mut self.watch)
    }
    fn ram(&mut self) -> Option<(*mut u8, usize)> {
        Some((self.ram.as_mut_ptr(), RAM))
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    fn below(&mut self, n: u32) -> u32 {
        self.next() % n
    }
}

/// A random instruction at word `i` of a program `len` words long.
fn instr(r: &mut Rng, i: u32, len: u32) -> u32 {
    let cond = if r.below(3) == 0 { r.below(15) } else { 0xE };
    let reg = |r: &mut Rng| if r.below(16) == 0 { 15 } else { r.below(13) };
    match r.below(28) {
        20 => {
            // Register-specified shifts (amounts mostly small, some 32 and up).
            let (op, s) = (r.below(16), r.below(2));
            let s = if (0x8..=0xB).contains(&op) { 1 } else { s };
            let rd = if (0x8..=0xB).contains(&op) { 0 } else { reg(r) };
            cond << 28 | op << 21 | s << 20 | reg(r) << 16 | rd << 12 | r.below(13) << 8 | r.below(4) << 5 | 1 << 4 | reg(r)
        }
        21 => {
            // LDRH/STRH/LDRSB/LDRSH/LDRD/STRD, bases into the data half.
            let (p, u, i, w, l, sh) = (r.below(4) != 0, r.below(2), r.below(2), r.below(4) == 0, r.below(2), 1 + r.below(3));
            let rn = if r.below(16) == 0 { 15 } else { 10 + r.below(3) };
            let off = if i == 1 { (r.below(16) << 8) | r.below(16) } else { r.below(10) };
            cond << 28 | (p as u32) << 24 | u << 23 | i << 22 | ((w && p) as u32) << 21 | l << 20 | rn << 16 | reg(r) << 12 | off & 0xF00 | 1 << 7 | sh << 5 | 1 << 4 | off & 0xF
        }
        22 => {
            // MUL/MLA, UMULL/UMLAL/SMULL/SMLAL.
            let long = r.below(2);
            cond << 28 | long << 23 | r.below(4) << 21 | r.below(2) << 20 | reg(r) << 16 | reg(r) << 12 | reg(r) << 8 | 0x90 | reg(r)
        }
        23 => {
            // CLZ, MRS, MSR (flags or the control byte).
            match r.below(3) {
                0 => cond << 28 | 0x016F_0F10 | r.below(13) << 12 | reg(r),
                1 => cond << 28 | 0x010F_0000 | r.below(13) << 12,
                _ => cond << 28 | 0x0320_F000 | r.below(2) << 16 | (if r.below(2) == 0 { 0x13 | 0xC0 } else { r.below(256) }),
            }
        }
        24..=25 => {
            // LDM/STM on a base in the data half (some with pc, a few S).
            let (p, u, w, l, s) = (r.below(2), r.below(2), r.below(2), r.below(2), (r.below(8) == 0) as u32);
            let list = r.below(0x8000).max(1) | ((r.below(4) == 0) as u32) << 15;
            cond << 28 | 0b100 << 25 | p << 24 | u << 23 | s << 22 | w << 21 | l << 20 | (10 + r.below(3)) << 16 | list
        }
        26 => 0xE59F_F000 | r.below(64) << 2 & 0xFFF, // ldr pc, [pc, #n]
        27 => match r.below(6) {
            // SMLA<x><y> / SMUL<x><y>.
            0 | 1 => cond << 28 | 0x0100_0080 | [0, 3][r.below(2) as usize] << 21 | reg(r) << 16 | reg(r) << 12 | reg(r) << 8 | r.below(4) << 5 | reg(r),
            // BX / BLX to a word of the program (sometimes Thumb).
            2 => cond << 28 | 0x012F_FF10 | r.below(2) << 5 | r.below(13),
            3 => 0xF5D0_F000 | r.below(13) << 16 | r.below(0x1000), // pld
            4 => cond << 28 | 0x0E10_0F10 | r.below(16) << 16 | reg(r) << 12 | r.below(8) << 5 | r.below(16), // mrc p15
            // MSR immediate (flags, or the control byte).
            _ => cond << 28 | 0x0320_F000 | (1 + r.below(15)) << 16 | r.below(16) << 8 | r.below(256),
        },
        0..=9 => {
            let (op, s) = (r.below(16), r.below(2));
            let (rn, mut rd) = (reg(r), reg(r));
            if (0x8..=0xB).contains(&op) {
                rd = 0;
            }
            let s = if (0x8..=0xB).contains(&op) { 1 } else { s };
            let op2 = if r.below(2) == 0 {
                (1 << 25) | (r.below(16) << 8) | r.below(256)
            } else {
                (r.below(32) << 7) | (r.below(4) << 5) | reg(r)
            };
            cond << 28 | op << 21 | s << 20 | rn << 16 | rd << 12 | op2
        }
        10..=14 => {
            // Bases r10-r12 point into the data half.
            let (p, u, b, w, l) = (r.below(4) != 0, r.below(2), r.below(2), r.below(4) == 0, r.below(2));
            let rn = if r.below(16) == 0 { 15 } else { 10 + r.below(3) };
            let off = if r.below(2) == 0 { r.below(64) } else { 1 << 25 | r.below(4) << 7 | r.below(4) << 5 | r.below(10) };
            cond << 28 | 1 << 26 | (p as u32) << 24 | u << 23 | b << 22 | ((w && p) as u32) << 21 | l << 20 | rn << 16 | reg(r) << 12 | off
        }
        15..=16 => {
            let target = r.below(len) as i32;
            let offset = ((target - i as i32 - 2) as u32) & 0x00FF_FFFF;
            cond << 28 | 0b101 << 25 | r.below(2) << 24 | offset
        }
        _ => r.next(),
    }
}

fn machine(seed: u64) -> (Arm, Flat) {
    let mut r = Rng(seed);
    let mut bus = Flat { ram: vec![0; RAM], watch: CodeWatch::default() };
    let len = 256;
    for i in 0..len {
        let w = instr(&mut r, i, len);
        bus.ram[4 * i as usize..4 * i as usize + 4].copy_from_slice(&w.to_le_bytes());
    }
    for a in (0x8000..RAM).step_by(4) {
        bus.ram[a..a + 4].copy_from_slice(&r.next().to_le_bytes());
    }
    let mut cpu = Arm::new();
    for n in 0..13 {
        cpu.r[n] = r.next();
    }
    for n in 10..13 {
        cpu.r[n] = 0x8000 + 4 * r.below(0x1000);
    }
    cpu.cpsr = mode::SVC | psr::I | psr::F | (r.next() & 0xF000_0000);
    (cpu, bus)
}

#[test]
fn lockstep_random_programs() {
    if !SUPPORTED {
        return;
    }
    let mut blocks = 0;
    for seed in 1..=300u64 {
        let (mut a, mut abus) = machine(seed * 0x9E37_79B9);
        let (mut b, mut bbus) = (a.clone(), abus.clone());
        let mut jit = ArmJit::new(RAM as u32);
        jit.hot = 1;
        const END: u64 = 20_000;
        while a.cycles < END {
            a.step(&mut abus);
        }
        while b.cycles < END {
            let left = END - b.cycles;
            jit.step(&mut b, &mut bbus, left);
        }
        blocks += jit.stats.blocks_run;
        assert_eq!(a.r, b.r, "seed {seed}: registers");
        assert_eq!(a.cpsr, b.cpsr, "seed {seed}: cpsr");
        assert_eq!(a, b, "seed {seed}: processor state");
        assert!(abus.ram == bbus.ram, "seed {seed}: memory");
    }
    assert!(blocks > 10_000, "only {blocks} blocks ran");
}

#[test]
fn decodes_a_loop() {
    // subs r0, r0, #1 ; bne -1
    let sub = decode::decode(0xE250_0001, 0);
    assert!(!sub.ends_block());
    assert!(decode::decode(0x1AFF_FFFD, 4).ends_block());
    // ldm (ends the block: loads pc), mul, msr cpsr compiled; msr spsr run
    // by the interpreter.
    assert!(decode::decode(0xE8BD_8000, 0).ends_block());
    assert!(matches!(decode::decode(0xE000_0291, 0), decode::Instr::Mul { .. }));
    assert!(matches!(decode::decode(0xE129_F000, 0), decode::Instr::Msr { .. }));
    assert!(matches!(decode::decode(0xE169_F000, 0), decode::Instr::Exec { .. }));
}

#[test]
#[ignore]
fn find_divergence() {
    let seed: u64 = std::env::var("SEED").unwrap().parse().unwrap();
    let (mut a, mut abus) = machine(seed * 0x9E37_79B9);
    let (mut b, mut bbus) = (a.clone(), abus.clone());
    let mut jit = ArmJit::new(RAM as u32);
    jit.hot = 1;
    while b.cycles < 20_000 {
        let (pc, before) = (b.r[15], b.clone());
        jit.step(&mut b, &mut bbus, u64::MAX);
        while a.cycles < b.cycles {
            a.step(&mut abus);
        }
        if a != b || abus.ram != bbus.ram {
            if let Some(i) = (0..RAM).find(|&i| abus.ram[i] != bbus.ram[i]) {
                println!("memory differs at {i:#x}: {:02x?} vs {:02x?}", &abus.ram[i & !3..][..4], &bbus.ram[i & !3..][..4]);
            }
            let w = |pc: u32| u32::from_le_bytes(abus.ram[pc as usize & 0xFFFC..][..4].try_into().unwrap());
            for i in 0..8 {
                let at = pc.wrapping_add(4 * i);
                println!("{at:#06x}: {:08x} {:?}", w(at), decode::decode(w(at), at));
            }
            println!("before {:x?} cpsr {:x}", before.r, before.cpsr);
            println!("interp {:x?} cpsr {:x}", a.r, a.cpsr);
            println!("jit    {:x?} cpsr {:x}", b.r, b.cpsr);
            panic!("diverged at block {pc:#x}");
        }
    }
}

#[test]
fn ldm_in_a_block() {
    if !SUPPORTED {
        return;
    }
    // ldmia r10, {r0-r3} ; add r4, r0, r1 ; b .
    let prog = [0xE89A_000Fu32, 0xE080_4001, 0xEAFF_FFFE];
    let mut bus = Flat { ram: vec![0; RAM], watch: CodeWatch::default() };
    for (i, w) in prog.iter().enumerate() {
        bus.ram[4 * i..4 * i + 4].copy_from_slice(&w.to_le_bytes());
    }
    for i in 0..4u32 {
        bus.ram[0x8000 + 4 * i as usize..][..4].copy_from_slice(&(i + 1).to_le_bytes());
    }
    let mut cpu = Arm::new();
    cpu.cpsr = mode::SVC | psr::I | psr::F;
    cpu.r[10] = 0x8000;
    let mut jit = ArmJit::new(RAM as u32);
    jit.hot = 1;
    jit.step(&mut cpu, &mut bus, u64::MAX);
    assert_eq!(jit.stats.blocks_run, 1);
    assert_eq!(&cpu.r[..5], &[1, 2, 3, 4, 3]);
}

/// The JIT on `ribbit-arm`'s `bench_loop` (load/add/store/count, a block of
/// five): mostly the cost of entering a block.
/// `cargo test --release -p ribbit-armjit bench_loop -- --ignored --nocapture`
#[test]
#[ignore]
fn bench_loop() {
    let code = [0xE592_1000u32, 0xE280_0001, 0xE582_0004, 0xE253_3001, 0x1AFF_FFFA];
    let mut bus = Flat { ram: vec![0; RAM], watch: CodeWatch::default() };
    for (i, w) in code.iter().enumerate() {
        bus.ram[0x1000 + 4 * i..][..4].copy_from_slice(&w.to_le_bytes());
    }
    let mut cpu = Arm::new();
    cpu.cpsr = mode::SVC | psr::I | psr::F;
    (cpu.r[15], cpu.r[2], cpu.r[3]) = (0x1000, 0x8000, u32::MAX);
    let mut jit = ArmJit::new(RAM as u32);
    let n = 100_000_000;
    let t = std::time::Instant::now();
    while cpu.cycles < n {
        jit.step(&mut cpu, &mut bus, u64::MAX);
    }
    let s = t.elapsed().as_secs_f64();
    println!("{} M instructions/s, {:.1} ns a block (r0 {})", (n as f64 / s / 1e6) as u64, s * 1e9 / jit.stats.blocks_run as f64, cpu.r[0]);
}
