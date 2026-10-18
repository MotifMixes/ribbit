//! Speed check: one hot straight-line loop body (a zero-overhead loop over
//! 16 instructions of the commonest shapes: loads, stores, ALU, compares,
//! moves), run by `Cpu::step`, by the batched interpreter `Cpu::run_block`
//! (what the app runs today) and compiled. Memory is a flat 64 KiB RAM.
//! Not a cycle-accurate comparison: the compiled code does no timing yet.
//!
//!   cargo run --release -p ribbit-jit --example jitbench

use ribbit_cpu::decode::decode_with;
use ribbit_cpu::icache::{Fast, Hot};
use ribbit_cpu::isa::Instr;
use ribbit_cpu::{Cpu, CpuConfig, NoAuxExt, reg};
use ribbit_memory::Bus;
use std::time::Instant;

const CODE: u32 = 0x4000_1000;

struct Ram {
    code: Vec<u8>,
    data: Vec<u8>,
}

impl Ram {
    fn off(&self, a: u32) -> usize {
        (a & 0xFFFF) as usize
    }
}

impl Bus for Ram {
    fn read8(&mut self, a: u32) -> u8 {
        self.data[self.off(a)]
    }
    fn read16(&mut self, a: u32) -> u16 {
        let o = self.off(a & !1);
        u16::from_le_bytes([self.data[o], self.data[(o + 1) & 0xFFFF]])
    }
    fn read32(&mut self, a: u32) -> u32 {
        let o = self.off(a & !3);
        u32::from_le_bytes(std::array::from_fn(|i| self.data[(o + i) & 0xFFFF]))
    }
    fn write8(&mut self, a: u32, v: u8) {
        let o = self.off(a);
        self.data[o] = v;
    }
    fn write16(&mut self, a: u32, v: u16) {
        let o = self.off(a & !1);
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.data[(o + i) & 0xFFFF] = b;
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        let o = self.off(a & !3);
        for (i, b) in v.to_le_bytes().into_iter().enumerate() {
            self.data[(o + i) & 0xFFFF] = b;
        }
    }
    fn fetch16(&mut self, a: u32) -> u16 {
        let o = a.wrapping_sub(CODE) as usize;
        u16::from_le_bytes([self.code.get(o).copied().unwrap_or(0), self.code.get(o + 1).copied().unwrap_or(0)])
    }
}

fn fetch(code: &[u8], a: u32) -> u16 {
    let o = a.wrapping_sub(CODE) as usize;
    u16::from_le_bytes([code.get(o).copied().unwrap_or(0), code.get(o + 1).copied().unwrap_or(0)])
}

/// 16 random instructions of common shapes (deterministic).
fn body() -> (Vec<u8>, Vec<Instr>) {
    let mut seed = 0x0B5E_55ED_u64;
    let mut next = || {
        seed ^= seed >> 12;
        seed ^= seed << 25;
        seed ^= seed >> 27;
        seed.wrapping_mul(0x2545_F491_4F6C_DD1D)
    };
    let want = |f: Fast| {
        matches!(
            f,
            Fast::Ld { .. }
                | Fast::St { .. }
                | Fast::Alu { .. }
                | Fast::AluI { .. }
                | Fast::Mov { .. }
                | Fast::MovI { .. }
                | Fast::AluF { .. }
                | Fast::AluFI { .. }
                | Fast::CmpI { .. }
                | Fast::Cmp { .. }
                | Fast::Un { .. }
        )
    };
    let mut code = Vec::new();
    while code.len() < 16 {
        let bytes: Vec<u8> = (0..8).map(|_| next() as u8).collect();
        let i = decode_with(CODE, |a| fetch(&bytes, a));
        let h = Hot::of(&i);
        // Keep SP and the loop counter out of it.
        let touches_special = format!("{:?}", h.fast).contains("d: 28");
        if !h.stop && want(h.fast) && !touches_special {
            code.push(bytes[..i.len as usize].to_vec());
        }
    }
    let code: Vec<u8> = code.concat();
    let mut instrs = Vec::new();
    let mut pc = CODE;
    while (pc - CODE) < code.len() as u32 {
        let i = decode_with(pc, |a| fetch(&code, a));
        pc = i.next();
        instrs.push(i);
    }
    (code, instrs)
}

fn fresh(code: &[u8], instrs: &[Instr]) -> (Cpu, Ram) {
    let mut cpu = Cpu::new(CpuConfig::leapster());
    for (n, r) in cpu.regs.iter_mut().enumerate().take(32) {
        *r = (n as u32).wrapping_mul(0x9E37_79B9);
    }
    cpu.regs[reg::SP] = 0x8000;
    cpu.pc = CODE;
    cpu.lp_start = CODE;
    cpu.lp_end = instrs.last().unwrap().next();
    cpu.regs[reg::LP_COUNT] = u32::MAX;
    (cpu, Ram { code: code.to_vec(), data: vec![0x5A; 0x1_0000] })
}

#[cfg(not(target_arch = "x86_64"))]
fn main() {}

#[cfg(target_arch = "x86_64")]
fn main() {
    let (code, instrs) = body();
    const N: u64 = 200_000_000;

    let (mut cpu, mut ram) = fresh(&code, &instrs);
    let t = Instant::now();
    for _ in 0..N / 10 {
        cpu.step(&mut ram).unwrap();
    }
    let step = (N / 10) as f64 / t.elapsed().as_secs_f64() / 1e6;

    let (mut cpu, mut ram) = fresh(&code, &instrs);
    cpu.icache.set_rom_ranges(&[(CODE, CODE + code.len() as u32)]);
    cpu.config.pipeline_timing = false;
    let t = Instant::now();
    let mut done = 0;
    while done < N {
        let b = cpu.run_block(&mut ram, &mut NoAuxExt, 1 << 20, u32::MAX, |_, _| false);
        if b.executed == 0 {
            // A processor-timer event: one step, as the machine does.
            cpu.step(&mut ram).unwrap();
            done += 1;
        }
        done += b.executed;
    }
    let block = done as f64 / t.elapsed().as_secs_f64() / 1e6;

    let (mut cpu, mut ram) = fresh(&code, &instrs);
    let compiled = ribbit_jit::compile(&instrs).expect("compiles");
    assert_eq!(compiled.compiled, instrs.len(), "the whole body compiles");
    let t = Instant::now();
    let mut done = 0u64;
    while done < N {
        done += compiled.run(&mut cpu, &mut ram).executed as u64;
    }
    let jit = done as f64 / t.elapsed().as_secs_f64() / 1e6;

    println!("loop body: {} instructions, {} bytes of x86-64", instrs.len(), compiled.code().len());
    println!("Cpu::step       {step:8.1} M instructions/s");
    println!("Cpu::run_block  {block:8.1} M instructions/s (pipeline timing off)");
    println!("JIT             {jit:8.1} M instructions/s ({:.1}x run_block)", jit / block);
}
