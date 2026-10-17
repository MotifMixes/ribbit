//! RESEARCH: HD capture + canvas end to end (hd.rs). Runs from a save state
//! with HD capture on, applies every finished pass to the HD canvas, and
//! writes LCD-vs-HD snapshots, with timing:
//!   cargo run --release -p ribbit-core --example hdplay -- <bios> <cart> <state> <ms> <every_ms> <out_prefix> [system.eeprom] [cart.sav]
//! RIBBIT_HD=0 runs the same span without capture (for the speed comparison).
//! RIBBIT_HD_PRESS="a,3000,100;right,5000,100": button presses (name, ms from
//! the start, ms held). RIBBIT_HD_TINT=1: the HD side shows the parts taken
//! from the LCD (not drawn as vectors) tinted magenta.
//! RIBBIT_HD_MONKEY=seed: a stylus tap (150 ms) at a random LCD pixel every
//! 1.5 s, to wander into a game's screens.

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
        if let Ok(o) = std::env::var("RIBBIT_HD_OFFSET") {
            c.shape_offset = o.parse().unwrap();
        }
        // RIBBIT_HD_FONTS: cartridge files (';'-separated) whose Flash
        // movies hold device fonts' outlines (swf_font.rs).
        if let Ok(list) = std::env::var("RIBBIT_HD_FONTS") {
            let mut fonts = std::collections::HashMap::new();
            for path in list.split(';').filter(|p| !p.is_empty()) {
                ribbit_core::swf_font::merge_fonts(&mut fonts, ribbit_core::swf_font::find_fonts(&ribbit_core::read_image(path).unwrap()));
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
    let presses: Vec<(ribbit_input::Button, u64, u64)> = std::env::var("RIBBIT_HD_PRESS")
        .map(|v| {
            v.split(';')
                .filter(|p| !p.is_empty())
                .map(|p| {
                    let q: Vec<&str> = p.split(',').collect();
                    (ribbit_input::Button::from_name(q[0]).expect("button name"), q[1].parse().unwrap(), q[2].parse().unwrap())
                })
                .collect()
        })
        .unwrap_or_default();
    let tint = std::env::var_os("RIBBIT_HD_TINT").is_some();
    let mut not_premultiplied = 0usize;
    // Device-text glyphs by font: (glyphs, with an outline).
    let mut text_fonts: std::collections::BTreeMap<String, (usize, usize)> = Default::default();
    let mut monkey = std::env::var("RIBBIT_HD_MONKEY").ok().map(|v| v.parse::<u64>().unwrap() | 1);
    for t in (0..total).step_by(16) {
        for &(b, at, dur) in &presses {
            m.buttons.set(b, (at..at + dur).contains(&t));
        }
        if let Some(r) = monkey.as_mut() {
            let phase = t % 1500;
            if phase < 16 {
                // xorshift
                *r ^= *r << 13;
                *r ^= *r >> 7;
                *r ^= *r << 17;
                let raw = |p: u32| ((p * 0x7F0 + 79) / 159).min(0x7F0);
                let (x, y) = (8 + (*r % 144) as u32, 8 + ((*r >> 16) % 144) as u32);
                m.io.touch.as_mut().unwrap().live = Some((raw(x), raw(y)));
            } else if phase >= 150 && phase < 166 {
                m.io.touch.as_mut().unwrap().live = None;
            }
        }
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
                // RIBBIT_HD_STROKEDBG=1: vertical stroke pieces against the BIOS's
                // outline edges of their fill (pass units).
                if std::env::var_os("RIBBIT_HD_STROKEDBG").is_some() {
                    for st in p.strokes.iter().filter(|s| (s.centre[0].0 - s.centre[2].0).abs() < 0.01 && (s.centre[0].1 - s.centre[2].1).abs() > 8.0).take(3) {
                        let (y0, y1) = (st.centre[0].1.min(st.centre[2].1), st.centre[0].1.max(st.centre[2].1));
                        let xs: Vec<i16> = p.edges.iter().filter(|e| e.fills[0] == st.fill && e.rule == 2 && e.p[0].0 == e.p[2].0 && (e.p[0].1.min(e.p[2].1) as f32) < y1 && (e.p[0].1.max(e.p[2].1) as f32) > y0 && (e.p[0].0 as f32 - st.centre[0].0).abs() < st.width + 8.0).map(|e| e.p[0].0).collect();
                        println!("{t} stroke x {} width {} y {y0}..{y1}: BIOS outline x {xs:?}", st.centre[0].0, st.width);
                    }
                    for st in p.strokes.iter().filter(|s| (s.centre[0].1 - s.centre[2].1).abs() < 0.01 && (s.centre[0].0 - s.centre[2].0).abs() > 8.0).take(3) {
                        let (x0, x1) = (st.centre[0].0.min(st.centre[2].0), st.centre[0].0.max(st.centre[2].0));
                        // Horizontal outline edges are dropped by the scanline renderer: the
                        // ends of the side edges give the rows instead.
                        let ys: Vec<i16> = p.edges.iter().filter(|e| e.fills[0] == st.fill && e.rule == 2 && (e.p[0].0.min(e.p[2].0) as f32) < x1 && (e.p[0].0.max(e.p[2].0) as f32) > x0).flat_map(|e| [e.p[0].1, e.p[2].1]).filter(|&y| (y as f32 - st.centre[0].1).abs() < st.width + 8.0).collect();
                        println!("{t} stroke y {} width {} x {x0}..{x1}: BIOS outline y {ys:?}", st.centre[0].1, st.width);
                    }
                }
                if std::env::var_os("RIBBIT_HD_STROKES").is_some() && !p.strokes.is_empty() {
                    let mut w: Vec<f32> = p.strokes.iter().map(|s| s.width / p.scale.max(1) as f32).collect();
                    w.sort_by(f32::total_cmp);
                    w.dedup();
                    println!("{t} strokes {} widths (LCD px) {w:?}", p.strokes.len());
                }
                for g in &p.glyphs {
                    let e = text_fonts.entry(format!("{} {}px", g.font, g.em)).or_default();
                    e.0 += 1;
                    if c.fonts.as_ref().and_then(|f| f.get(&g.font)).and_then(|f| f.glyphs.get(&g.ch)).is_some_and(|o| !o.pieces.is_empty()) {
                        e.1 += 1;
                    }
                }
                for f in p.fills.values() {
                    // A colour brighter than its alpha would not be premultiplied.
                    if !f.mask && f.rgba[3] < 255 && f.rgba[..3].iter().any(|&c| c > f.rgba[3]) {
                        not_premultiplied += 1;
                    }
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
                        let ramp = u32::from_le_bytes([0, 1, 2, 3].map(|k| m.bus.peek8(a + 0x94 + k).unwrap_or(0)));
                        if ramp != 0 {
                            let al: Vec<String> = (0..257u32).step_by(8).chain(250..257).map(|i| format!("{i}:{:02x}{:02x}{:02x}{:02x}", m.bus.peek8(ramp + 4 * i + 2).unwrap_or(0), m.bus.peek8(ramp + 4 * i + 1).unwrap_or(0), m.bus.peek8(ramp + 4 * i).unwrap_or(0), m.bus.peek8(ramp + 4 * i + 3).unwrap_or(0))).collect();
                            println!("       ramp (R G B A): {}", al.join(" "));
                        }
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
                if let Ok(b) = std::env::var("RIBBIT_HD_EXACT") {
                    let v: Vec<i16> = b.split(',').map(|s| s.parse().unwrap()).collect();
                    let near: Vec<_> = p.edges.iter().filter(|e| e.p.iter().any(|&(x, y)| x >= v[0] && x <= v[2] && y >= v[1] && y <= v[3])).collect();
                    let ex = near.iter().filter(|e| e.exact.is_some()).count();
                    if !near.is_empty() {
                        println!("exact {ex}/{} rules {:?}", near.len(), near.iter().filter(|e| e.exact.is_none()).map(|e| (e.rule, e.line)).collect::<Vec<_>>());
                    }
                }
                if std::env::var("RIBBIT_HD_ORDERS").is_ok_and(|v| t >= v.parse::<u64>().unwrap()) {
                    let mut other: std::collections::HashMap<u32, usize> = Default::default();
                    for e in &p.edges {
                        for f in e.fills {
                            if f != 0 && !p.fills.contains_key(&f) {
                                *other.entry(f).or_default() += 1;
                            }
                        }
                    }
                    for (a, n) in &other {
                        let words: Vec<String> = (0..0x30u32).map(|i| format!("{:08x}", u32::from_le_bytes([0, 1, 2, 3].map(|k| m.bus.peek8(a + 4 * i + k).unwrap_or(0))))).collect();
                        println!("{t} non-fill {a:08x} edges {n}: {}", words.join(" "));
                    }
                }
                // RIBBIT_HD_ORDERS=ms: list every fill of passes from then on.
                if std::env::var("RIBBIT_HD_ORDERS").is_ok_and(|v| t >= v.parse::<u64>().unwrap()) {
                    let mut fs: Vec<_> = p.fills.iter().collect();
                    fs.sort_by_key(|(_, f)| f.order);
                    let n = |a: u32| p.edges.iter().filter(|e| e.fills.contains(&a)).count();
                    println!("{t} pass {:?}: {}", p.rect, fs.iter().map(|(a, f)| format!("{a:08x} order {:08x} painter {:x} rgba {:?} mask {} clip {:08x} edges {} bmp {}", f.order, f.painter, f.rgba, f.mask, f.clip, n(**a), f.bitmap.as_ref().map_or("-".to_string(), |b| format!("{}x{} m {:?} clear {}", b.image.width, b.image.height, b.matrix, b.image.pixels.iter().filter(|&&p| p >> 24 < 255).count())))).collect::<Vec<_>>().join(" | "));
                }
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
            let before = c.why;
            fixed += c.update_mask(&lcd[..]);
            // RIBBIT_HD_WHEN=n: frames with more than n fallback blocks, by reason.
            if let Ok(n) = std::env::var("RIBBIT_HD_WHEN") {
                let d: Vec<u64> = (0..4).map(|k| c.why[k] - before[k]).collect();
                if d.iter().sum::<u64>() > n.parse::<u64>().unwrap() {
                    println!("{t} frame fallback {d:?}");
                }
            }
            draw_t += start.elapsed();
        }
        if (t + 16) % every < 16 {
            if let Some(c) = &canvas {
                let mut hd = c.composed(&lcd[..]);
                // RIBBIT_HD_FAR=1: the blocks that disagree with the LCD, by reason.
                if std::env::var_os("RIBBIT_HD_FAR").is_some() {
                    let mut by: std::collections::BTreeMap<u8, Vec<(usize, usize)>> = Default::default();
                    for (i, &r) in c.why_block.iter().enumerate() {
                        if r > 1 {
                            by.entry(r).or_default().push((i % 160, i / 160));
                        }
                    }
                    for (r, v) in &by {
                        let raw = |x: usize, y: usize| {
                            let mut a = [0u32; 4];
                            for yy in y * c.scale..(y + 1) * c.scale {
                                for xx in x * c.scale..(x + 1) * c.scale {
                                    let p = c.pixels[yy * c.size + xx];
                                    for (k, sh) in [16, 8, 0, 24].into_iter().enumerate() {
                                        a[k] += (p >> sh) & 0xFF;
                                    }
                                }
                            }
                            a.map(|v| v / (c.scale * c.scale) as u32)
                        };
                        let ex: Vec<String> = v.iter().step_by((v.len() / 8).max(1)).take(8).map(|&(x, y)| { let l = lcd[y * 160 + x]; format!("({x},{y}) lcd {:06x} hd {:?}", l & 0xFFFFFF, raw(x, y)) }).collect();
                        println!("{t} shot {shot} reason {r}: {} blocks, e.g. {}", v.len(), ex.join(" "));
                    }
                }
                // RIBBIT_HD_RAW=1: HD's own drawing only, over grey.
                if std::env::var_os("RIBBIT_HD_RAW").is_some() {
                    hd = c.pixels.iter().map(|&p| {
                        let a = (p >> 24) as f32 / 255.0;
                        let m = |v: u32| ((v & 0xFF) as f32 + 128.0 * (1.0 - a)).min(255.0) as u32;
                        0xFF00_0000 | m(p >> 16) << 16 | m(p >> 8) << 8 | m(p)
                    }).collect();
                } else if tint {
                    // Share of each pixel from the LCD: magenta over it.
                    let s = c.scale;
                    for (i, p) in hd.iter_mut().enumerate() {
                        let (x, y) = (i % c.size, i / c.size);
                        let keep = 1.0 - c.mask[(y / s) * 160 + x / s] as f32 / 255.0;
                        let lcd_share = 1.0 - (c.pixels[i] >> 24) as f32 / 255.0 * keep;
                        if lcd_share > 0.02 {
                            // By reason: stale magenta, far red, off yellow, unknown cyan.
                            let t = match c.why_block[(y / s) * 160 + x / s] {
                                2 => [255.0, 0.0, 0.0],
                                3 => [255.0, 255.0, 0.0],
                                4 => [0.0, 255.0, 255.0],
                                _ => [255.0, 0.0, 255.0],
                            };
                            let k = 0.6 * lcd_share;
                            let mix = |v: u32, t: f32| ((v as f32) * (1.0 - k) + t * k) as u32;
                            *p = 0xFF00_0000 | mix((*p >> 16) & 0xFF, t[0]) << 16 | mix((*p >> 8) & 0xFF, t[1]) << 8 | mix(*p & 0xFF, t[2]);
                        }
                    }
                }
                write(&format!("{prefix}{shot:03}.png"), &lcd, &hd, c.size);
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
    if let Some(c) = &canvas {
        println!("fallback by reason [stale, far, off, unknown]: {:?}", c.why);
    }
    println!("translucent fills brighter than their alpha: {not_premultiplied}");
    println!("text fonts (glyphs, from outlines): {text_fonts:?}");
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
