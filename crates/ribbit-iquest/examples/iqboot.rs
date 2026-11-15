//! Boot probe: `iqboot <bios> [cart|-] [seconds]`. Prints the firmware's
//! debug port, accesses the model does not understand and the hottest PCs.
use ribbit_iquest::Machine;
use std::collections::HashMap;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = Machine::new(std::fs::read(&a[1]).expect("BIOS"));
    if let Some(c) = a.get(2).filter(|c| *c != "-") {
        m.insert_cartridge(std::fs::read(c).expect("cart"));
    }
    let secs: f64 = a.get(3).map(|s| s.parse().unwrap()).unwrap_or(5.0);
    m.bus.trace_io = std::env::var("IOTRACE").is_ok();
    // HOLDKEY=name: a key held from power-on.
    if let Ok(k) = std::env::var("HOLDKEY") {
        assert!(m.set_key(&k, true), "unknown key");
    }
    // PWATCH=lo,hi (hex): log data reads of those physical addresses.
    if let Ok(w) = std::env::var("PWATCH") {
        let v: Vec<u32> = w.split(',').map(|n| u32::from_str_radix(n, 16).unwrap()).collect();
        m.bus.pwatch = Some((v[0], v[1]));
    }
    let mut hist: HashMap<(u16, u16), u64> = HashMap::new();
    let total = (secs * m.hz as f64) as u64;
    let mut done = 0u64;
    // TRACEPC=bank,pc (hex): from the first time code window 1 holds that
    // bank at that PC, print the next 4000 instructions' (bank1, pc, phys).
    let trace_at: Option<(u16, u16)> = std::env::var("TRACEPC").ok().map(|t| {
        let v: Vec<u16> = t.split(',').map(|n| u16::from_str_radix(n, 16).unwrap()).collect();
        (v[0], v[1])
    });
    let mut tracing = 0u32;
    while done < total {
        let bank = u16::from_be_bytes(m.bus.banks[1]);
        if tracing == 0 && trace_at == Some((bank, m.cpu.pc)) {
            tracing = 4000;
        }
        if tracing > 0 {
            tracing -= 1;
            let p = m.bus.window_phys((m.cpu.pc >> 15) as usize, m.cpu.pc);
            let (t, _) = ribbit_leappad::disasm::disasm(m.cpu.pc, |a| m.bus.code_peek(a));
            println!("T {bank:03X}:{:04X} {p:06X} A={:02X} {t}", m.cpu.pc, m.cpu.acc());
        }
        *hist.entry((bank, m.cpu.pc)).or_default() += 1;
        done += m.step() as u64;
    }
    if let Ok(f) = std::env::var("RAMDUMP") {
        std::fs::write(f, &m.bus.ram).unwrap();
    }
    println!("--- serial output ---\n{}", m.take_log());
    println!("--- unknown accesses ---");
    for u in m.bus.unknown.iter().take(120) {
        println!("  {:04X}: {}", u.pc, u.what);
    }
    let mut h: Vec<_> = hist.into_iter().collect();
    h.sort_by(|a, b| b.1.cmp(&a.1));
    println!("--- hot (bank1, pc) ---");
    for ((b, pc), n) in h.iter().take(12) {
        println!("  {b:03X}:{pc:04X} {n}");
    }
    for (pc, p, v) in m.bus.pwatch_log.iter() {
        println!("  pw {pc:04X} {p:07X} {v:02X}");
    }
    if m.bus.trace_io {
        let mut seen: HashMap<(u16, u16, bool), (u64, std::collections::BTreeSet<u8>)> = HashMap::new();
        for (pc, reg, w, v) in &m.bus.io_trace {
            let e = seen.entry((*pc, *reg, *w)).or_default();
            e.0 += 1;
            e.1.insert(*v);
        }
        let mut v: Vec<_> = seen.into_iter().collect();
        v.sort_by_key(|(k, _)| (k.1, k.0));
        for ((pc, reg, w), (n, vals)) in v {
            let vals: Vec<String> = vals.iter().take(8).map(|x| format!("{x:02X}")).collect();
            println!("  io {reg:04X} {} pc {pc:04X} x{n} [{}]", if w { "W" } else { "R" }, vals.join(" "));
        }
    }
}
