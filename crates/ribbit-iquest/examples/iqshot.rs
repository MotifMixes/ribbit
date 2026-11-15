//! Headless run that saves what the LCD shows:
//! `iqshot <bios> [cart|-] <seconds> <out prefix> [every seconds]`
//! writes `<prefix>.<n>.pgm` every so often (default only at the end).
//! KEYS="t:key,t:key..." presses keys (Machine::press) at those seconds
//! for 0.15 s each.
use ribbit_iquest::Machine;

fn save(m: &Machine, path: &str) {
    let f = m.frame();
    let mut out = format!("P5\n{} {}\n255\n", f.width, f.height).into_bytes();
    out.extend(f.pixels.iter().map(|&p| if p != 0 { 0x20 } else { 0xC8 }));
    std::fs::write(path, out).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = Machine::new(std::fs::read(&a[1]).expect("BIOS"));
    if let Some(c) = a.get(2).filter(|c| *c != "-") {
        m.insert_cartridge(std::fs::read(c).expect("cart"));
    }
    let secs: f64 = a[3].parse().unwrap();
    // IO=reg:val,... (hex): preset register latches (research).
    for kv in std::env::var("IO").unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
        let (r, v) = kv.split_once(':').unwrap();
        m.bus.io[usize::from_str_radix(r, 16).unwrap()] = u8::from_str_radix(v, 16).unwrap();
    }
    let every: f64 = a.get(5).map(|s| s.parse().unwrap()).unwrap_or(secs);
    let keys: Vec<(f64, String)> = std::env::var("KEYS")
        .unwrap_or_default()
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|k| {
            let (t, n) = k.split_once(':').unwrap();
            (t.parse().unwrap(), n.to_string())
        })
        .collect();
    let slice = m.hz / 100;
    let mut t = 0.0;
    let mut n = 0;
    let mut next = every;
    let mut held: Option<(String, f64)> = None;
    while t < secs - 1e-9 {
        for (kt, k) in &keys {
            if (*kt - t).abs() < 0.005 {
                if !m.set_key(k, true) {
                    eprintln!("unknown key {k}");
                }
                let hold: f64 = std::env::var("HOLD").ok().and_then(|h| h.parse().ok()).unwrap_or(0.15);
                held = Some((k.clone(), t + hold));
            }
        }
        if let Some((k, until)) = held.clone()
            && t >= until
        {
            m.set_key(&k, false);
            held = None;
        }
        m.run_cycles(slice);
        t += 0.01;
        if std::env::var("LOG").is_ok() {
            for l in m.take_log().lines() {
                if !l.trim().is_empty() {
                    println!("{t:6.2} {l}");
                }
            }
        }
        if t >= next - 1e-9 {
            save(&m, &format!("{}.{n}.pgm", a[4]));
            n += 1;
            next += every;
        }
    }
    if let Some(f) = &m.bus.flash {
        let used = f.data.iter().filter(|&&b| b != 0xFF).count();
        println!("flash: {used} bytes not erased");
        if let Ok(p) = std::env::var("FLASHOUT") {
            std::fs::write(p, &f.data).unwrap();
        }
    }
    let log = m.take_log();
    let tail: Vec<&str> = log.lines().rev().take(12).collect();
    for l in tail.iter().rev() {
        println!("{l}");
    }
}
