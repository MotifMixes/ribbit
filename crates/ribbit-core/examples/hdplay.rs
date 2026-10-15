//! RESEARCH: HD capture + canvas end to end (hd.rs). Runs from a save state
//! with HD capture on, applies every finished pass to the HD canvas, and
//! writes LCD-vs-HD snapshots, with timing:
//!   cargo run --release -p ribbit-core --example hdplay -- <bios> <cart> <state> <ms> <every_ms> <out_prefix> [system.eeprom] [cart.sav]
//! RIBBIT_HD=0 runs the same span without capture (for the speed comparison).

use ribbit_core::Machine;
use ribbit_core::hd::{HD_SIZE, HdCanvas};
use ribbit_debug::Breakpoints;
use ribbit_video::{HEIGHT, WIDTH};
use std::time::{Duration, Instant};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [bios, cart, state, ms, every, prefix, rest @ ..] = &a[..] else {
        panic!("usage: hdplay <bios> <cart> <state> <ms> <every_ms> <out_prefix> [system.eeprom] [cart.sav]");
    };
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(bios).unwrap()).unwrap();
    let cart = ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(cart).unwrap()).unwrap();
    let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    if let Some(p) = rest.first() {
        m.io.eeprom.load(&std::fs::read(p).unwrap());
    }
    if let Some(p) = rest.get(1) {
        m.io.eeprom.insert_cart(Some(&std::fs::read(p).unwrap()));
    }
    m.enable_experimental_mame_touch(Default::default());
    m.record_trace_tail = false;
    m.load_state(&std::fs::read(state).unwrap()).unwrap();
    let hd_on = std::env::var("RIBBIT_HD").map_or(true, |v| v != "0");
    let mut canvas = None;
    if hd_on {
        assert!(m.enable_hd_capture(), "HD hooks not found in this BIOS");
        canvas = Some(HdCanvas::new(m.hd.as_ref().unwrap().hooks));
    }
    let per_ms = m.clock.cpu_hz / 1000;
    let (total, every): (u64, u64) = (ms.parse().unwrap(), every.parse().unwrap());
    let (mut emu_t, mut draw_t, mut edges, mut passes, mut fixed) = (Duration::ZERO, Duration::ZERO, 0usize, 0usize, 0usize);
    let mut lcd = Box::new([0u32; WIDTH * HEIGHT]);
    let mut shot = 0;
    for t in (0..total).step_by(16) {
        let start = Instant::now();
        let end = m.cpu.cycles + 16 * per_ms;
        while m.cpu.cycles < end {
            m.run(end - m.cpu.cycles, &Breakpoints::default());
        }
        emu_t += start.elapsed();
        m.render_frame(&mut lcd);
        if let (Some(c), Some(hd)) = (canvas.as_mut(), m.hd.as_mut()) {
            let start = Instant::now();
            for p in hd.take() {
                edges += p.edges.len();
                passes += 1;
                c.apply(&p, &lcd[..]);
            }
            fixed += c.reconcile(&lcd[..]);
            draw_t += start.elapsed();
        }
        if (t + 16) % every < 16 {
            if let Some(c) = &canvas {
                write(&format!("{prefix}{shot:03}.png"), &lcd, &c.pixels);
                shot += 1;
            }
        }
    }
    let secs = total as f64 / 1000.0;
    println!(
        "hd={hd_on}: emulation {:.0}% of real time; {passes} passes, {edges} edges; HD drawing {:.1} ms per emulated second",
        100.0 * secs / emu_t.as_secs_f64(),
        draw_t.as_secs_f64() * 1000.0 / secs
    );
    println!("blocks taken from the LCD: {fixed}");
    if let Some(hd) = &m.hd {
        println!("dropped passes: {}", hd.dropped);
    }
}

fn write(path: &str, lcd: &[u32; WIDTH * HEIGHT], hd: &[u32]) {
    let (w, h) = (HD_SIZE * 2 + 8, HD_SIZE);
    let mut rgb = vec![255u8; w * h * 3];
    for y in 0..HD_SIZE {
        for x in 0..HD_SIZE {
            for (ox, p) in [(0, lcd[(y / 4) * WIDTH + x / 4]), (HD_SIZE + 8, hd[y * HD_SIZE + x])] {
                let o = (y * w + ox + x) * 3;
                rgb[o..o + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}
