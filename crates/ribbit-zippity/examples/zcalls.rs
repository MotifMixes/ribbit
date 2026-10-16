//! Research: which functions a running game calls, with what argument
//! (r0) and from where (lr), counted per stretch of time.
//!
//! zcalls <nand.bin> <state> <until> <addr,addr,...> [pad script] [window]
//!
//! Loads a saved machine (zipboot's ZIPBOOT_SAVE), runs to instruction
//! `until` stepping one instruction at a time, and prints for each window
//! (default 60M instructions, a second) the calls seen: address, caller,
//! r0, count.

use ribbit_zippity::Zippity;
use std::collections::BTreeMap;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut z = Zippity::new(std::fs::read(&args[1]).expect("read the NAND image"));
    z.load_state(&std::fs::read(&args[2]).expect("read the state")).expect("a state");
    let until: u64 = args[3].parse().expect("an instruction count");
    let addrs: Vec<u32> = args[4].split(',').map(|a| u32::from_str_radix(a.trim_start_matches("0x"), 16).unwrap()).collect();
    if let Some(script) = args.get(5).filter(|s| !s.is_empty()) {
        for item in script.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            let state = u16::from_str_radix(f[0], 16).unwrap();
            z.probe().pad_script.push((state, f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    let window: u64 = args.get(6).and_then(|s| s.parse().ok()).unwrap_or(60_000_000);
    let mut counts: BTreeMap<(u32, u32, u32), u64> = BTreeMap::new();
    let mut window_end = (z.instructions() / window + 1) * window;
    while z.instructions() < until {
        z.step();
        let c = z.cpu();
        if addrs.contains(&c.r[15]) {
            *counts.entry((c.r[15], c.r[14], c.r[0])).or_default() += 1;
        }
        if z.instructions() >= window_end {
            println!("--- to {}M", window_end / 1_000_000);
            for ((a, lr, r0), n) in &counts {
                println!("  {a:08x} from {lr:08x} r0 {r0:#06x} x{n}");
            }
            counts.clear();
            window_end += window;
        }
    }
}
