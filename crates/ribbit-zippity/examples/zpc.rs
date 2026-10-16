//! Where the system spends its time: a histogram of the program counter
//! (every 97th instruction) over a number of frames.
//!
//! zpc <nand.bin> <frames> [pad script: state:from:to;...]

use ribbit_zippity::{FRAME, Zippity};
use std::collections::HashMap;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let nand = std::fs::read(&args[1]).expect("read the NAND image");
    let frames: u64 = args[2].parse().expect("a frame count");
    let mut z = Zippity::new(nand);
    if let Some(script) = args.get(3) {
        for item in script.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            let state = u16::from_str_radix(f[0], 16).unwrap();
            z.probe().pad_script.push((state, f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    let mut hist: HashMap<u32, u64> = HashMap::new();
    let mut n = 0u64;
    while z.instructions() < frames * FRAME {
        z.step();
        n += 1;
        if n.is_multiple_of(97) {
            *hist.entry(z.cpu().current).or_default() += 1;
        }
    }
    // The code at the end, for a disassembler.
    std::fs::write("zpc_ram.bin", &z.ram()[0x2C000..0x2E000]).unwrap();
    let c = z.cpu();
    println!("thumb {} cpsr {:08x}", c.thumb(), c.cpsr);
    let total: u64 = hist.values().sum();
    // By 64-byte region (a loop), then the top addresses.
    let mut regions: HashMap<u32, u64> = HashMap::new();
    for (&pc, &c) in &hist {
        *regions.entry(pc & !0x3F).or_default() += c;
    }
    let mut r: Vec<_> = regions.into_iter().collect();
    r.sort_by(|a, b| b.1.cmp(&a.1));
    println!("--- 64-byte regions ---");
    for (pc, c) in r.iter().take(25) {
        println!("{pc:08x}  {:5.2}%", 100.0 * *c as f64 / total as f64);
    }
}
