//! Scripted play-through with a per-scene speed report, to find the scenes
//! that emulate slower than real time (the user's own BIOS, cartridge and
//! saves; nothing is committed):
//!   cargo run --release -p ribbit-core --example scenes -- <bios> <cart> <script> [system.eeprom] [cart.sav]
//! Script lines (emulated time; `#` comments):
//!   run MS              run for MS milliseconds
//!   tap X Y [MS]        stylus down at LCD pixel (X, Y) for MS (default 200), then up
//!   btn NAME MS         hold a button (up down left right a b hint home pause) for MS
//!   shot PATH           write the screen as a PNG (2x)
//!   snap                keep the screen for the next `sheet`
//!   every MS N          N times: run MS, then snap
//!   sheet PATH          write the kept screens as one PNG (8 per row), then forget them
//!   save PATH / load PATH   save state
//!   label TEXT          name the following part of the report
//!   digest [NAME]       print the machine state digest (for comparing builds)
//!   tvdump PATH         write the L-MAX TV chip's memory (RIBBIT_LMAX_TV=1)
//! RIBBIT_MAME_GPIO=1 reads unresolved GPIO lines as 1 (MAME's workaround).
//! Every 250 ms of emulated time prints the speed (percent of real time at
//! the CPU clock) and how much was idle-skipped.

use ribbit_core::Machine;
use ribbit_core::io::touch::MameTouchConfig;
use ribbit_debug::Breakpoints;
use ribbit_input::Button;
use ribbit_video::{HEIGHT, WIDTH};
use std::time::Instant;

const WINDOW_MS: u64 = 250;

struct Meter {
    label: String,
    worst: f64,
    worst_label: String,
    rows: Vec<(String, f64, f64)>,
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [bios, cart, script, rest @ ..] = &args[..] else {
        panic!("usage: scenes <bios> <cart> <script> [system.eeprom] [cart.sav]");
    };
    let image = ribbit_core::read_image(bios).unwrap();
    // RIBBIT_ANY_MODEL=1: EXPERIMENTAL, also boot Leapster TV / L-MAX / Leapster 2 BIOSes.
    let fw = if std::env::var_os("RIBBIT_ANY_MODEL").is_some() {
        ribbit_memory::Firmware::from_bytes_any_model(image)
    } else {
        ribbit_memory::Firmware::from_bytes(image)
    }
    .unwrap();
    let cart = ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(cart).unwrap()).unwrap();
    let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    if let Some(p) = rest.first() {
        m.io.eeprom.load(&std::fs::read(p).unwrap());
    }
    if let Some(p) = rest.get(1) {
        m.io.eeprom.insert_cart(Some(&std::fs::read(p).unwrap()));
    }
    m.enable_experimental_mame_touch(MameTouchConfig::default());
    m.record_trace_tail = false;
    if std::env::var_os("RIBBIT_LMAX_TV").is_some() {
        // EXPERIMENTAL: the L-MAX's TV chip (see io/lmax_tv.rs).
        m.io.lmax_tv = Some(Default::default());
    }
    if std::env::var_os("RIBBIT_MAME_GPIO").is_some() {
        // EXPERIMENTAL: unresolved GPIO lines read 1 (MAME's workaround).
        m.io.gpio.unresolved_lines = !ribbit_input::BUTTON_BITS;
    }
    if let Ok(mhz) = std::env::var("RIBBIT_MHZ") {
        m.clock = ribbit_core::Clock::new(mhz.parse::<u64>().unwrap() * 1_000_000, m.clock.peripheral_hz);
    }
    let mut kept: Vec<Box<[u32; WIDTH * HEIGHT]>> = vec![];
    let mut meter = Meter { label: "boot".into(), worst: f64::MAX, worst_label: String::new(), rows: vec![] };
    for line in std::fs::read_to_string(script).unwrap().lines() {
        let line = line.split('#').next().unwrap().trim();
        let w: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize, d: u64| w.get(i).map_or(d, |s| s.parse().unwrap());
        match w.first().copied() {
            None => {}
            Some("run") => run_ms(&mut m, num(1, 0), &mut meter),
            Some("tap") => {
                let raw = |p: u64| ((p as u32 * 0x7F0 + 79) / 159).min(0x7F0);
                m.io.touch.as_mut().unwrap().live = Some((raw(num(1, 0)), raw(num(2, 0))));
                run_ms(&mut m, num(3, 200), &mut meter);
                m.io.touch.as_mut().unwrap().live = None;
            }
            Some("btn") => {
                let b = Button::from_name(w[1]).expect("button name");
                m.buttons.set(b, true);
                run_ms(&mut m, num(2, 200), &mut meter);
                m.buttons.set(b, false);
            }
            Some("shot") => shot(&m, w[1]),
            Some("snap") => kept.push(frame(&m)),
            Some("every") => {
                for _ in 0..num(2, 1) {
                    run_ms(&mut m, num(1, 0), &mut meter);
                    kept.push(frame(&m));
                }
            }
            Some("sheet") => sheet(&std::mem::take(&mut kept), w[1]),
            Some("save") => std::fs::write(w[1], m.save_state()).unwrap(),
            Some("load") => m.load_state(&std::fs::read(w[1]).unwrap()).unwrap(),
            Some("label") => meter.label = w[1..].join(" "),
            Some("tvdump") => {
                let tv = m.io.lmax_tv.as_ref().expect("RIBBIT_LMAX_TV=1 for tvdump");
                std::fs::write(w[1], &tv.mem).unwrap();
                println!("tv memory: {} reads, {} writes", tv.reads, tv.writes);
            }
            Some("digest") => println!("digest {} {:016x} at cycle {}", w.get(1).unwrap_or(&""), m.state_digest(), m.cpu.cycles),
            Some(other) => panic!("unknown command {other}"),
        }
    }
    println!("\nworst window: {:.0}% ({})", meter.worst, meter.worst_label);
    let mut by: Vec<(String, f64, f64, usize)> = vec![];
    for (l, s, idle) in &meter.rows {
        match by.iter_mut().find(|r| &r.0 == l) {
            Some(r) => {
                r.1 = r.1.min(*s);
                r.2 += idle;
                r.3 += 1
            }
            None => by.push((l.clone(), *s, *idle, 1)),
        }
    }
    for (l, s, idle, n) in by {
        println!("{l:30} min {s:6.0}%  idle {:4.0}%  ({n} windows)", idle / n as f64);
    }
}

fn run_ms(m: &mut Machine, ms: u64, meter: &mut Meter) {
    let per_ms = m.clock.cpu_hz / 1000;
    let mut left = ms;
    while left > 0 {
        let chunk = left.min(WINDOW_MS);
        left -= chunk;
        let skipped0 = m.idle_skipped_steps();
        let t = Instant::now();
        let r = m.run(chunk * per_ms, &Breakpoints::default());
        let wall = t.elapsed().as_secs_f64();
        assert_eq!(r.stop, ribbit_core::StopReason::StepLimit, "{r:?}");
        let speed = 100.0 * (chunk as f64 / 1000.0) / wall;
        let idle = 100.0 * (m.idle_skipped_steps() - skipped0) as f64 / r.steps as f64;
        if chunk == WINDOW_MS {
            let t_ms = m.cpu.cycles / per_ms;
            if speed < meter.worst {
                meter.worst = speed;
                meter.worst_label = format!("{} at {t_ms} ms", meter.label);
            }
            meter.rows.push((meter.label.clone(), speed, idle));
            if std::env::var_os("VERBOSE").is_some() {
                println!("{t_ms:8} ms {:24} {speed:6.0}% idle {idle:3.0}%", meter.label);
            }
        }
    }
}

fn frame(m: &Machine) -> Box<[u32; WIDTH * HEIGHT]> {
    let mut px = Box::new([0u32; WIDTH * HEIGHT]);
    m.render_frame(&mut px);
    px
}

fn sheet(frames: &[Box<[u32; WIDTH * HEIGHT]>], path: &str) {
    let cols = 8.min(frames.len().max(1));
    let rows = frames.len().div_ceil(cols).max(1);
    let (w, h) = (cols * (WIDTH + 4), rows * (HEIGHT + 4));
    let mut rgb = vec![255u8; w * h * 3];
    for (i, f) in frames.iter().enumerate() {
        let (ox, oy) = ((i % cols) * (WIDTH + 4), (i / cols) * (HEIGHT + 4));
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let p = f[y * WIDTH + x];
                let o = ((oy + y) * w + ox + x) * 3;
                rgb[o..o + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}

fn shot(m: &Machine, path: &str) {
    // PNG at 2x (nearest), to look at.
    let px = frame(m);
    let s = 2;
    let mut rgb = Vec::with_capacity(WIDTH * HEIGHT * 3 * s * s);
    for y in 0..HEIGHT * s {
        for x in 0..WIDTH * s {
            let p = px[(y / s) * WIDTH + x / s];
            rgb.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), (WIDTH * s) as u32, (HEIGHT * s) as u32);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}
