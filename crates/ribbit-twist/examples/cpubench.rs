//! Research: the interpreter's speed on a few loops (millions of
//! instructions a second).
//!
//!   cargo run --release -p ribbit-twist --example cpubench

use ribbit_twist::machine::{self, Twist};

fn bench(name: &str, code: &[u32]) {
    let bytes: Vec<u8> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut t = Twist::new(&bytes);
    t.cpu.r[13] = 0x10_0000;
    t.cpu.r[1] = 0x20_0000;
    let clock = std::time::Instant::now();
    t.run(machine::CLOCK);
    let secs = clock.elapsed().as_secs_f64();
    println!("{name}: {:.0} M instructions/s", t.cpu.instructions as f64 / secs / 1e6);
    // The processor alone, without the machine's per-instruction checks.
    let mut t = Twist::new(&bytes);
    t.cpu.r[1] = 0x20_0000;
    let clock = std::time::Instant::now();
    for _ in 0..machine::CLOCK {
        t.cpu.step(&mut t.bus);
    }
    let secs = clock.elapsed().as_secs_f64();
    println!("{name} (processor alone): {:.0} M instructions/s", t.cpu.instructions as f64 / secs / 1e6);
}

fn main() {
    // add r0, r0, #1; subs r2, r2, #1; b .-8
    bench("alu", &[0xE2800001, 0xE2522001, 0xEAFFFFFC]);
    // ldr r3, [r1]; str r3, [r1, #4]; add r0, r0, #1; b .-12
    bench("memory", &[0xE5913000, 0xE5813004, 0xE2800001, 0xEAFFFFFB]);
}
