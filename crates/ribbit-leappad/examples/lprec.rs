//! Records what the pad plays: `lprec <bios> <cart> <out.wav> [secs] [go spread] [regions]`.
//! Boots the cartridge, touches the GO of `go spread` (counted from 0 in the
//! book drawn from the cartridge) if given, and writes `secs` (10) of sound
//! as an 8 kHz WAV, touching the middle of each of `regions` of that spread
//! first if given (comma-separated, 3 s apart; `n:secs` holds the pen down
//! that long, 0.25 s otherwise). MUTE=mask (hex) silences those voices (bit n: voice n),
//! to hear which voice plays what.
use ribbit_leappad::book::Book;
use ribbit_leappad::{MACHINE_HZ, Machine};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    // `-` for no cartridge (the base ROM's own start-up).
    let cart = (a[2] != "-").then(|| std::fs::read(&a[2]).expect("cartridge"));
    let secs: f32 = a.get(4).map_or(10.0, |s| s.parse().expect("secs"));
    let go = a.get(5).map(|s| s.parse::<usize>().expect("spread"));
    let regions: Vec<(usize, f32)> = a.get(6).map_or(Vec::new(), |s| {
        s.split(',')
            .map(|t| match t.split_once(':') {
                Some((r, h)) => (r.parse().expect("region"), h.parse().expect("hold secs")),
                None => (t.parse().expect("region"), 0.25),
            })
            .collect()
    });
    let mute = std::env::var("MUTE").ok().map_or(0, |m| u8::from_str_radix(&m, 16).expect("MUTE=hex mask"));
    let book = cart.as_ref().and_then(|c| Book::from_cartridge(c, "rec", None).ok());
    let mut m = Machine::new(std::fs::read(&a[1]).expect("BIOS"));
    if let Some(c) = cart {
        m.insert_cartridge(c);
    }
    m.bus.mute = mute;
    if let Ok(hz) = std::env::var("PINHZ") {
        m.pin_hz = hz.parse().expect("PINHZ");
    }
    // DSPLOG=1: print the sound commands of the recorded part (time, command,
    // start, pitch, F4B8).
    let dsplog = std::env::var("DSPLOG").is_ok();
    let step = MACHINE_HZ / 200;
    let mut audio = Vec::new();
    let mut times: Vec<f32> = Vec::new();
    let mut run = |m: &mut Machine, secs: f32, audio: &mut Vec<i16>| {
        for _ in 0..(secs * 200.0) as u32 {
            m.run_cycles(step);
            let mut buf = vec![0i16; m.bus.sound.output.len()];
            m.bus.sound.output.pop_into(&mut buf);
            audio.extend_from_slice(&buf);
            m.take_log();
            if let Some(l) = &m.bus.dsp_log {
                times.resize(l.len(), audio.len() as f32 / 8000.0);
            }
        }
    };
    if let Some(spot) = go.and_then(|g| book.as_ref()?.spreads.get(g)?.go) {
        run(&mut m, 4.0, &mut Vec::new());
        m.set_pen(Some(spot));
        run(&mut m, 0.25, &mut Vec::new());
        m.set_pen(None);
        for &(r, hold) in &regions {
            let Some(r) = book.as_ref().and_then(|b| b.spreads.get(go?)?.regions.get(r)) else { continue };
            let [x0, y0, x1, y1] = r.rect;
            run(&mut m, 3.0, &mut Vec::new());
            m.set_pen(Some(ribbit_leappad::book::Spot { page: r.page, x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0 }));
            run(&mut m, hold, &mut Vec::new());
            m.set_pen(None);
        }
    }
    if dsplog {
        m.bus.dsp_log = Some(Vec::new());
    }
    run(&mut m, secs, &mut audio);
    for ((_, c, a), t) in m.bus.dsp_log.iter().flatten().zip(&times) {
        let word = |i: usize| u16::from_le_bytes([a[i], a[i + 1]]);
        println!("{t:7.3} {c:02X} start {} pitch {:04X} rate {:04X}", a[0..4].iter().map(|b| format!("{b:02X}")).collect::<String>(), word(8), word(10));
    }
    let mut f = Vec::new();
    let data = (audio.len() * 2) as u32;
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + data).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 1 | 1 << 16, 8000, 16000, 2 | 16 << 16] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data.to_le_bytes());
    for s in &audio {
        f.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(&a[3], f).expect("write WAV");
}
