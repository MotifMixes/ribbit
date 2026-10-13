//! Decoder throughput on the user's own BIOS dump (never committed).
use std::hint::black_box;

fn main() {
    let path = std::env::args().nth(1).expect("usage: decode_bench <bios>");
    let bios = std::fs::read(path).expect("read");
    let base = 0x4002_0000u32;
    let fetch = |a: u32| {
        let o = (a - 0x4000_0000) as usize;
        u16::from_le_bytes([bios[o], bios[o + 1]])
    };
    let n = 50_000_000u64;
    let t = std::time::Instant::now();
    let mut pc = base;
    let mut acc = 0u64;
    for _ in 0..n {
        let i = ribbit_cpu::decode::decode_with(pc, fetch);
        acc = acc.wrapping_add(i.word as u64);
        pc = if i.next() >= 0x4008_0000 { base } else { i.next() };
    }
    black_box(acc);
    let s = t.elapsed().as_secs_f64();
    println!("{n} decodes in {s:.2} s: {:.1} ns each", s * 1e9 / n as f64);
}
