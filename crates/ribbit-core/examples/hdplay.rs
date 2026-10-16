//! RESEARCH: HD capture + canvas end to end (hd.rs). Runs from a save state
//! with HD capture on, applies every finished pass to the HD canvas, and
//! writes LCD-vs-HD snapshots, with timing:
//!   cargo run --release -p ribbit-core --example hdplay -- <bios> <cart> <state> <ms> <every_ms> <out_prefix> [system.eeprom] [cart.sav]
//! RIBBIT_HD=0 runs the same span without capture (for the speed comparison).

use ribbit_core::Machine;
use ribbit_core::hd::HdCanvas;
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
        // RIBBIT_HD_SCALE: canvas pixels per LCD pixel (4 = 640, 16 = 2560).
        let scale = std::env::var("RIBBIT_HD_SCALE").map_or(4, |s| s.parse().unwrap());
        let mut c = HdCanvas::with_scale(m.hd.as_ref().unwrap().hooks, scale);
        // RIBBIT_HD_FONTS: cartridge files (';'-separated) whose Flash
        // movies hold device fonts' outlines (swf_font.rs).
        if let Ok(list) = std::env::var("RIBBIT_HD_FONTS") {
            let mut fonts = std::collections::HashMap::new();
            for path in list.split(';').filter(|p| !p.is_empty()) {
                for (name, f) in ribbit_core::swf_font::find_fonts(&ribbit_core::read_image(path).unwrap()) {
                    let better = fonts.get(&name).is_none_or(|g: &ribbit_core::swf_font::OutlineFont| g.glyphs.len() < f.glyphs.len());
                    if better {
                        fonts.insert(name, f);
                    }
                }
            }
            println!("fonts: {:?}", fonts.iter().map(|(n, f)| (n.clone(), f.glyphs.len())).collect::<Vec<_>>());
            c.fonts = Some(std::sync::Arc::new(fonts));
        }
        canvas = Some(c);
    }
    let per_ms = m.clock.cpu_hz / 1000;
    let (total, every): (u64, u64) = (ms.parse().unwrap(), every.parse().unwrap());
    let (mut emu_t, mut draw_t, mut edges, mut passes, mut fixed) = (Duration::ZERO, Duration::ZERO, 0usize, 0usize, 0usize);
    let mut lcd = Box::new([0u32; WIDTH * HEIGHT]);
    let mut shot = 0;
    let mut dumped = false;
    let mut painters: std::collections::BTreeMap<u32, usize> = Default::default();
    for t in (0..total).step_by(16) {
        let start = Instant::now();
        let end = m.cpu.cycles + 16 * per_ms;
        while m.cpu.cycles < end {
            m.run_cycles(end - m.cpu.cycles, &Breakpoints::default());
        }
        emu_t += start.elapsed();
        m.render_frame(&mut lcd);
        if let (Some(c), Some(hd)) = (canvas.as_mut(), m.hd.as_mut()) {
            let start = Instant::now();
            for p in hd.take() {
                if std::env::var_os("RIBBIT_HD_LOG").is_some() {
                    let mut ps: Vec<String> = p.fills.values().map(|f| format!("{:x}{}", f.painter, if f.bitmap.is_some() { "b" } else { "" })).collect();
                    ps.sort();
                    ps.dedup();
                    println!("{t:6} pass renderer {:08x} rect {:?} rows {:?} scale {} edges {} fills {} painters {ps:?}", p.renderer, p.rect, p.rows, p.scale, p.edges.len(), p.fills.len());
                }
                for f in p.fills.values() {
                    *painters.entry(f.painter).or_insert(0usize) += 1;
                }
                // RIBBIT_HD_PAINTER=hex: print the first few fills painted by
                // that painter (0x100 bytes as words) with the pass's rect.
                if let Ok(want) = std::env::var("RIBBIT_HD_PAINTER") {
                    let want = u32::from_str_radix(&want, 16).unwrap();
                    for (a, f) in p.fills.iter().filter(|(_, f)| f.painter == want).take(2) {
                        let words: Vec<String> =
                            (0..0x40u32).map(|i| format!("{:08x}", u32::from_le_bytes([0, 1, 2, 3].map(|k| m.bus.peek8(a + 4 * i + k).unwrap_or(0))))).collect();
                        println!("{t:6} fill {a:08x} rgba {:?} rect {:?}: {}", f.rgba, p.rect, words.join(" "));
                    }
                }
                if !dumped {
                    if let Some((a, _)) = p.fills.iter().find(|(_, f)| f.painter == 0x4005_4D24) {
                        let raw: Vec<u8> = (0..0xC0).map(|i| m.bus.peek8(a + i).unwrap_or(0)).collect();
                        let bmp = u32::from_le_bytes(raw[0x38..0x3C].try_into().unwrap());
                        let braw: Vec<u8> = (0..0x20).map(|i| m.bus.peek8(bmp + i).unwrap_or(0)).collect();
                        std::fs::write(format!("{prefix}fill.bin"), &raw).unwrap();
                        std::fs::write(format!("{prefix}bitmap.bin"), &braw).unwrap();
                        println!("bitmap fill {a:08x} at {t} ms, bitmap {bmp:08x}, rect {:?} scale {}", p.rect, p.scale);
                        dumped = true;
                    }
                }
                edges += p.edges.len();
                passes += 1;
                // RIBBIT_HD_EDGES=x0,y0,x1,y1 (pass units): print the edges
                // with a point in that box.
                if let Ok(b) = std::env::var("RIBBIT_HD_EDGES") {
                    let v: Vec<i16> = b.split(',').map(|s| s.parse().unwrap()).collect();
                    for e in p.edges.iter().filter(|e| e.p.iter().any(|&(x, y)| x >= v[0] && x <= v[2] && y >= v[1] && y <= v[3])) {
                        println!("edge {:?} line {} rule {} dir {} fills {:08x?}", e.p, e.line, e.rule, e.dir, e.fills);
                    }
                }
                c.apply(&p);
                if let Ok(from) = std::env::var("RIBBIT_HD_EACH") {
                    if t >= from.parse::<u64>().unwrap() {
                        write(&format!("{prefix}each{t}_{passes}_{:?}.png", p.rect.unwrap_or_default()).replace(['[', ']', ' '], "").replace(',', "-"), &lcd, &c.composed(&lcd[..]), c.size);
                    }
                }
            }
            fixed += c.update_mask(&lcd[..]);
            draw_t += start.elapsed();
        }
        if (t + 16) % every < 16 {
            if let Some(c) = &canvas {
                write(&format!("{prefix}{shot:03}.png"), &lcd, &c.composed(&lcd[..]), c.size);
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
    println!("blocks from the fallback: {fixed}");
    println!("fills by painter: {painters:x?}");
    if let Some(hd) = &m.hd {
        println!("dropped passes: {}", hd.dropped);
    }
}

fn write(path: &str, lcd: &[u32; WIDTH * HEIGHT], hd: &[u32], size: usize) {
    let (w, h) = (size * 2 + 8, size);
    let s = size / WIDTH;
    let mut rgb = vec![255u8; w * h * 3];
    for y in 0..size {
        for x in 0..size {
            for (ox, p) in [(0, lcd[(y / s) * WIDTH + x / s]), (size + 8, hd[y * size + x])] {
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
