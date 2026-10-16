//! Where emulated code spends its time: runs to <from_ms>, then counts the
//! program counter of every instruction for <ms> and prints the busiest
//! addresses (for finding what a stuck game waits on):
//!   cargo run --release -p ribbit-core --example pcs -- <bios> <cart> <from_ms> <ms> [system.eeprom]
//! RIBBIT_MMIO=1 also lists every device-register access in the window,
//! except the idle loop's clock-register polling. RIBBIT_CALLS=1 lists every
//! call (a change of BLINK) with its time in ms, caller and target.

use ribbit_core::Machine;
use ribbit_debug::Breakpoints;
use std::collections::HashMap;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [bios, cart, from, len, rest @ ..] = &args[..] else {
        panic!("usage: pcs <bios> <cart> <from_ms> <ms> [system.eeprom]");
    };
    // Research tool: any model's BIOS (Leapster TV, L-MAX, Leapster 2 too).
    let fw = ribbit_memory::Firmware::from_bytes_any_model(ribbit_core::read_image(bios).unwrap()).unwrap();
    let cart = ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(cart).unwrap()).unwrap();
    let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    if let Some(p) = rest.first() {
        m.io.eeprom.load(&std::fs::read(p).unwrap());
    }
    m.enable_experimental_mame_touch(Default::default());
    if let Ok(s) = std::env::var("RIBBIT_STUBS") {
        // Research: fixed read values for unmodelled registers, "addr=value,...".
        for kv in s.split(',') {
            let (a, v) = kv.split_once('=').expect("addr=value");
            let hex = |x: &str| u32::from_str_radix(x.trim_start_matches("0x"), 16).unwrap();
            m.io.stubs.insert(hex(a), hex(v));
        }
    }
    if std::env::var_os("RIBBIT_TV").is_some() {
        // EXPERIMENTAL: Leapster TV (video chip and controller link).
        m.enable_leapster_tv();
        m.io.tv_link.as_mut().unwrap().logging = true;
    }
    if std::env::var_os("RIBBIT_LMAX_TV").is_some() {
        // EXPERIMENTAL: the L-MAX / Leapster TV video chip (see io/lmax_tv.rs).
        m.io.lmax_tv = Some(Default::default());
    }
    m.record_trace_tail = false;
    if let Ok(p) = std::env::var("RIBBIT_STATE") {
        // Start from a save state (same BIOS and cartridge); <from_ms> is absolute.
        m.load_state(&std::fs::read(p).unwrap()).unwrap();
    }
    let none = Breakpoints::default();
    let ms = m.clock.cpu_hz / 1000;
    let target = from.parse::<u64>().unwrap() * ms;
    while m.cpu.cycles < target {
        m.run_cycles(target - m.cpu.cycles, &none);
    }
    m.idle_skip = false;
    let log_mmio = std::env::var_os("RIBBIT_MMIO").is_some();
    if log_mmio {
        m.io.mmio_tail = ribbit_debug::TraceRing::new(2_000_000);
        if let Ok(r) = std::env::var("RIBBIT_MMIO_RANGE") {
            // "lo-hi" (hex): keep only accesses in that range.
            let (lo, hi) = r.split_once('-').expect("lo-hi");
            let hex = |x: &str| u32::from_str_radix(x.trim_start_matches("0x"), 16).unwrap();
            m.io.mmio_filter = Some((hex(lo), hex(hi)));
        }
    }
    let end = m.cpu.cycles + len.parse::<u64>().unwrap() * ms;
    let log_calls = std::env::var_os("RIBBIT_CALLS").is_some();
    let mut counts: HashMap<u32, u64> = HashMap::new();
    while m.cpu.cycles < end {
        *counts.entry(m.cpu.pc).or_default() += 1;
        let (pc, blink) = (m.cpu.pc, m.cpu.regs[ribbit_cpu::reg::BLINK]);
        m.run(1, &none);
        let nb = m.cpu.regs[ribbit_cpu::reg::BLINK];
        if log_calls && nb != blink && nb.wrapping_sub(pc) <= 8 {
            println!("{:>10.3} call {pc:08x} -> {:08x}", m.cpu.cycles as f64 / ms as f64, m.cpu.pc);
        }
    }
    if log_mmio {
        for a in m.io.mmio_tail.to_vec() {
            let line = a.to_string();
            if m.io.mmio_filter.is_some() || !line.contains("0x01809008") {
                println!("{line}");
            }
        }
    }
    let mut v: Vec<_> = counts.into_iter().collect();
    v.sort_by_key(|&(pc, n)| (std::cmp::Reverse(n), pc));
    let total: u64 = v.iter().map(|x| x.1).sum();
    println!("{total} steps; {} distinct addresses", v.len());
    for (pc, n) in v.iter().take(std::env::var("RIBBIT_TOP").map_or(60, |s| s.parse().unwrap())) {
        println!("{pc:08x} {n:>10} {:5.2}%", 100.0 * *n as f64 / total as f64);
    }
}
