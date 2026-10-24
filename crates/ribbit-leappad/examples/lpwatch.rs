//! Touch probe: boots a cartridge, makes touches and reports what the app
//! read in a range of the cartridge meanwhile (Bus::pwatch), to see how it
//! walks its tables. `lpwatch <bios> <cart> <from> <to> <touch>...` with
//! cartridge offsets in hex and touches as `L:x,y` or `R:x,y` (page
//! coordinates); `-` waits a second.

use ribbit_leappad::book::{Side, Spot};
use ribbit_leappad::{MACHINE_HZ, Machine};

const CART: u32 = 0x140_0000;

fn secs(s: f32) -> u32 {
    (s * MACHINE_HZ as f32) as u32
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 5 {
        eprintln!("usage: lpwatch <bios> <cart> <from> <to> <touch>...");
        std::process::exit(2);
    }
    let hex = |s: &str| u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("a hex offset");
    let mut m = Machine::new(std::fs::read(&args[1]).expect("the base ROM"));
    m.insert_cartridge(std::fs::read(&args[2]).expect("the cartridge"));
    // BOOTPC=lo-hi (hex): also watch during start-up, keeping the reads made
    // by instructions in that range, anywhere in the base ROM or cartridge.
    let boot_pcs = std::env::var("BOOTPC").ok().map(|r| {
        let (lo, hi) = r.split_once('-').expect("BOOTPC=lo-hi");
        (hex(lo) as u16, hex(hi) as u16)
    });
    if boot_pcs.is_some() {
        m.bus.pwatch = vec![(0, u32::MAX)];
    }
    let mut log = String::new();
    for _ in 0..400 {
        m.run_cycles(secs(0.25));
        log += &m.take_log();
        if log.contains("[Idle]") {
            break;
        }
    }
    m.run_cycles(secs(0.5));
    println!("booted: {}", if log.contains("[Idle]") { "idle" } else { "NOT idle" });
    if std::env::var_os("BOOTLOG").is_some() {
        println!("{log}");
    }
    if let Some((lo, hi)) = boot_pcs {
        let mut seen = std::collections::BTreeSet::new();
        for &(pc, p, v) in &m.bus.pwatch_log {
            if (lo..=hi).contains(&pc) && seen.insert((pc, p)) {
                println!("  boot pc {pc:04X}: phys {p:07X} = {v:02X}");
            }
        }
    }
    m.bus.pwatch_log.clear();
    m.take_log();
    m.bus.pwatch = vec![(CART + hex(&args[3]), CART + hex(&args[4]))];
    for t in &args[5..] {
        m.bus.pwatch_log.clear();
        if t == "-" {
            m.run_cycles(secs(1.0));
        } else {
            let (side, xy) = t.split_once(':').expect("L:x,y");
            let (x, y) = xy.split_once(',').expect("L:x,y");
            let page = if side == "R" { Side::Right } else { Side::Left };
            m.set_pen(Some(Spot { page, x: x.parse().unwrap(), y: y.parse().unwrap() }));
            // PENDOWN=seconds: how long the pen stays down (0.3).
            let down = std::env::var("PENDOWN").ok().and_then(|v| v.parse().ok()).unwrap_or(0.3);
            m.run_cycles(secs(down));
            m.set_pen(None);
            m.run_cycles(secs(0.6));
        }
        println!("== {t}");
        for line in m.take_log().lines().filter(|l| !l.trim().is_empty()) {
            println!("  | {line}");
        }
        // Runs of reads by the same instruction, as offsets in the cartridge.
        let mut runs: Vec<(u16, Vec<(u32, u8)>)> = Vec::new();
        for &(pc, p, v) in &m.bus.pwatch_log {
            match runs.last_mut() {
                Some((rpc, r)) if *rpc == pc => r.push((p - CART, v)),
                _ => runs.push((pc, vec![(p - CART, v)])),
            }
        }
        // RUNS=n: how many runs to show (400).
        let shown = std::env::var("RUNS").ok().and_then(|n| n.parse().ok()).unwrap_or(400);
        for (pc, r) in runs.iter().take(shown) {
            let cells: Vec<String> = r.iter().map(|(o, v)| format!("{o:05X}={v:02X}")).collect();
            println!("  pc {pc:04X}: {}", cells.join(" "));
        }
        if runs.len() > shown {
            println!("  … {} more runs", runs.len() - shown);
        }
    }
}
