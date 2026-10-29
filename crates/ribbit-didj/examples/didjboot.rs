//! Boots the Didj from a NAND built from its firmware packages and prints
//! the UART output and the registers the software touched that have no
//! model yet (milestone 2):
//!   cargo run -p ribbit-didj --example didjboot -- <folder of .lfp files> [million instructions]

use ribbit_core::zip;
use ribbit_didj::machine::Didj;
use ribbit_didj::nand::Nand;
use ribbit_didj::package::{self, Firmware};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: didjboot <folder of .lfp files> [million instructions]");
    let millions: u64 = args.get(1).map_or(50, |s| s.parse().expect("not a number"));

    let mut fw = Firmware::default();
    for e in std::fs::read_dir(dir).expect("can't read the folder").flatten() {
        let path = e.path();
        if package::package_id(&path.to_string_lossy()).is_none() {
            continue;
        }
        let data = std::fs::read(&path).expect("can't read a package");
        for entry in zip::entries(&data).expect("not a zip archive") {
            fw.add(&entry.name, zip::read_entry(&data, &entry).unwrap());
        }
    }
    let flash = Nand::from_firmware(&fw).unwrap_or_else(|e| panic!("{e}"));

    let mut didj = Didj::new(flash);
    let mut ran = 0;
    let mut last_pc = u32::MAX;
    let mut same = 0;
    // Where the CPU is, sampled once per million instructions: 4 KB page -> count.
    let mut samples = std::collections::HashMap::<u32, u32>::new();
    for _ in 0..millions {
        let n = didj.run(1_000_000);
        ran += n;
        *samples.entry(didj.cpu.r[15] & !0xFFF).or_default() += 1;
        // A tight loop that never leaves one address range: report and stop.
        let pc = didj.cpu.r[15] & !0xFF;
        same = if pc == last_pc { same + 1 } else { 0 };
        last_pc = pc;
        if same >= 2000 {
            println!("stuck near {:#010x}", didj.cpu.r[15]);
            break;
        }
    }
    let t = &mut didj.bus.timers;
    let c = t.read(0xC000_1808);
    t.write(0xC000_1808, c & !0x20 | 0x40);
    let r = [0xC000_0814, 0xC000_0810, 0xC000_0820, 0xC000_1808, 0xC000_1804, 0xC000_1800].map(|a| t.read(a));
    println!("IC mask {:08x}:{:08x} pend {:08x}; timer0 ctrl {:08x} match {:08x} count {:08x}", r[0], r[1], r[2], r[3], r[4], r[5]);
    println!("{ran} instructions, pc {:#010x}, cpsr {:#x}", didj.cpu.r[15], didj.cpu.cpsr);
    for (i, s) in didj.bus.uart.iter().enumerate() {
        if !s.is_empty() {
            println!("--- UART{i} ---\n{s}");
        }
    }
    println!("--- unmodelled registers (address: first pc, reads, writes) ---");
    for (a, (pc, r, w)) in &didj.bus.touched {
        println!("{a:#010x}: {pc:#010x} r{r} w{w}");
    }
    println!("--- last unmodelled accesses (address pc r/w) ---");
    for (a, pc, w) in &didj.bus.recent {
        println!("{a:#010x} {pc:#010x} {}", if *w { "w" } else { "r" });
    }
    let mut hot: Vec<_> = samples.into_iter().collect();
    hot.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let hot: Vec<String> = hot.iter().take(12).map(|(p, n)| format!("{p:08x}:{n}")).collect();
    println!("--- busiest pages (page:samples) ---
{}", hot.join(" "));
    // DIDJ_DUMP=<file> saves SDRAM, to disassemble the kernel.
    if let Ok(path) = std::env::var("DIDJ_DUMP") {
        std::fs::write(&path, &didj.bus.sdram).expect("can't write the dump");
    }
    for (pc, word) in didj.cpu.undefined_log.iter().take(10) {
        println!("undefined {word:#010x} at {pc:#010x}");
    }
}
