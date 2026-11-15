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
    // PWATCH=lo,hi (hex): log data reads of those physical addresses.
    if let Ok(w) = std::env::var("PWATCH") {
        let v: Vec<u32> = w.split(',').map(|n| u32::from_str_radix(n, 16).unwrap()).collect();
        m.bus.pwatch = Some((v[0], v[1]));
    }
    let mut hist: HashMap<(u16, u16), u64> = HashMap::new();
    let total = (secs * m.hz as f64) as u64;
    let mut done = 0u64;
    while done < total {
        let bank = u16::from_be_bytes(m.bus.banks[1]);
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
    for (pc, p, v) in m.bus.pwatch_log.iter().take(200) {
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
