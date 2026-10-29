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
    match r.below(20) {
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
    let sub = decode::decode(0xE250_0001, 0).unwrap();
    assert!(!sub.ends_block());
    assert!(decode::decode(0x1AFF_FFFD, 4).unwrap().ends_block());
    // ldm, mul, msr: not handled.
    assert_eq!(decode::decode(0xE8BD_8000, 0), None);
    assert_eq!(decode::decode(0xE000_0291, 0), None);
    assert_eq!(decode::decode(0xE129_F000, 0), None);
}
