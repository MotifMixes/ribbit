//! Research: where a running game spends its time, from a saved machine.
//!
//! zhist <nand.bin> <state> <until> [pad script] [granule]
//!
//! Counts the program counter (every 7th instruction) in granules of
//! `granule` bytes (default 0x100) and prints the busiest ones.

use ribbit_zippity::Zippity;
use std::collections::HashMap;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut z = Zippity::new(std::fs::read(&args[1]).expect("read the NAND image"));
    z.load_state(&std::fs::read(&args[2]).expect("read the state")).expect("a state");
    let until: u64 = args[3].parse().expect("an instruction count");
    if let Some(script) = args.get(4).filter(|s| !s.is_empty()) {
        for item in script.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            let state = u16::from_str_radix(f[0], 16).unwrap();
            z.probe().pad_script.push((state, f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    let granule: u32 = args.get(5).and_then(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).ok()).unwrap_or(0x100);
    let mut hist: HashMap<u32, u64> = HashMap::new();
    let mut n = 0u64;
    while z.instructions() < until {
        z.step();
        n += 1;
        if n.is_multiple_of(7) {
            *hist.entry(z.cpu().current / granule * granule).or_default() += 1;
        }
    }
    let total: u64 = hist.values().sum();
    let mut v: Vec<_> = hist.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    for (a, c) in v.iter().take(40) {
        println!("{a:08x} {:6.2}%", 100.0 * *c as f64 / total as f64);
    }
}
