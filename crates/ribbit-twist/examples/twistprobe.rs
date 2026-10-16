//! Research: runs a RockIt Twist firmware image on the ARM9 model and
//! reports what it touched (see `ribbit_twist::probe`).
//!
//! cargo run --release -p ribbit-twist --example twistprobe -- <file> [instructions] [ADDR=VALUE ...]
//!
//! ADDR=VALUE (hex) makes reads of that word return VALUE: a research stub
//! to see past a wait, reported as such.
//!
//! <file> is a system package (.lfp: its BinFile is used) or a firmware
//! image taken out of one.

use ribbit_twist::{image, package, probe};

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(path) = args.next() else {
        eprintln!("usage: twistprobe <package.lfp | firmware image> [instructions]");
        std::process::exit(2);
    };
    let budget: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(20_000_000);
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex number");
    let answers: Vec<(u32, u32)> =
        args.map(|a| a.split_once('=').map(|(k, v)| (hex(k), hex(v))).expect("ADDR=VALUE")).collect();
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let data = if ribbit_core::zip::is_zip(&data) {
        let meta =
            ribbit_core::zip::extract_named(&data, "Firmware/meta.inf").expect("no Firmware/meta.inf in the package");
        let meta = package::Meta::parse(&String::from_utf8_lossy(&meta));
        let bin = meta.get("BinFile").expect("meta.inf names no BinFile");
        println!(
            "package {} {} ({}), device {}",
            meta.get("Name").unwrap_or("?"),
            meta.get("Version").unwrap_or("?"),
            bin,
            meta.get("Device").unwrap_or("?")
        );
        ribbit_core::zip::extract_named(&data, &format!("Firmware/{bin}")).expect("the BinFile is missing")
    } else {
        data
    };
    match image::parse(&data) {
        Some(h) => {
            println!("image: code 0x{:X} bytes, check 0x{:08X}, trailer 0x{:08X}", h.code_size, h.check, h.trailer)
        }
        None => println!("no image header: run as bare code"),
    }
    let mut p = probe::Probe::new(image::code(&data));
    for &(a, v) in &answers {
        println!("STUB (not hardware): reads of {a:08X} return {v:08X}");
        p.bus.answers.insert(a & !3, v);
    }
    let t = std::time::Instant::now();
    let end = p.run(budget);
    println!(
        "{end:?} after {} instructions ({:.1} s); pc {:08X}, cpsr {:08X}, mmu {}",
        p.cpu.instructions,
        t.elapsed().as_secs_f64(),
        p.cpu.r[15],
        p.cpu.cpsr,
        if p.cpu.cp15.mmu_on() { "on" } else { "off" }
    );
    if !p.cpu.undefined_log.is_empty() {
        println!("undefined instructions: {:08X?}", &p.cpu.undefined_log[..p.cpu.undefined_log.len().min(10)]);
    }
    println!("\npages outside RAM (64 KB), reads/writes, first access:");
    for (page, u) in &p.bus.pages {
        let (pc, a, w) = u.first;
        let what = w.map_or_else(|| "read".to_string(), |v| format!("write {v:08X}"));
        println!("  {:04X}xxxx  {:>9} / {:<9} first: pc {pc:08X} {what} at {a:08X}", page, u.reads, u.writes);
    }
    if !p.bus.uart.is_empty() {
        println!("\ntext written to 0xC0060000:\n{}", String::from_utf8_lossy(&p.bus.uart));
    }
    println!("\nhottest addresses at the end:");
    for (a, n) in p.hottest(12) {
        println!("  {a:08X}  {n}");
    }
}
