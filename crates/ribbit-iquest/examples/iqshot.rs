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
        let mut cart = std::fs::read(c).expect("cart");
        if let Some(r) = ribbit_iquest::repair_checksum(&mut cart) {
            eprintln!("checksum repaired: {r:?}");
        }
        m.insert_cartridge(cart);
    }
    let secs: f64 = a[3].parse().unwrap();
    // AWAKE=1: "Turn off when idle" off.
    m.keep_awake = std::env::var("AWAKE").is_ok();
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
    let mut audio: Vec<i16> = Vec::new();
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
        let mut chunk = vec![0i16; m.bus.sound.output.len()];
        m.bus.sound.output.pop_into(&mut chunk);
        audio.extend_from_slice(&chunk);
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
    // WAV=path: the sound (8 kHz mono).
    if let Ok(p) = std::env::var("WAV") {
        let mut w = Vec::new();
        let n = audio.len() as u32 * 2;
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + n).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&[16, 0, 0, 0, 1, 0, 1, 0]);
        w.extend_from_slice(&8000u32.to_le_bytes());
        w.extend_from_slice(&16000u32.to_le_bytes());
        w.extend_from_slice(&[2, 0, 16, 0]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&n.to_le_bytes());
        for v in &audio {
            w.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(p, w).unwrap();
        let loud = audio.iter().filter(|v| v.unsigned_abs() > 1000).count();
        println!("audio: {} samples, {} above 1000, peak {}", audio.len(), loud, audio.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0));
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
