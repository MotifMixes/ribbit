//! RESEARCH: HD rendering proof of concept (US v2.1 BIOS 152-11265 only).
//!
//! The BIOS's Flash rasterizer works on edge records in quarter-pixel
//! device coordinates (a 640x640 grid) before averaging them down to the
//! 160x160 LCD. This tool captures every edge the BIOS builds during one
//! render pass (its edge constructor, 0x4004_E0D8) with the fill styles the
//! edges point to, rasterizes them itself at 640x640, and writes that next
//! to the BIOS's own frame:
//!   cargo run --release -p ribbit-core --example hdframe -- <bios> <cart> <state> <run_ms> <out.png> [system.eeprom] [cart.sav]
//!
//! Edge record (r1 at the constructor): +0x08 / +0x0C fill-style pointers,
//! +0x10/+0x12 start, +0x14/+0x16 control, +0x18/+0x1A end (i16, quarter
//! pixels), +0x1C straight-line flag. Fill style: +0x0C depth (high half),
//! +0x18 type, +0x24 B G R A (u16 each). All VERIFIED-FW by observation
//! only; gradients and bitmaps are not decoded (drawn with their +0x24 colour).

use ribbit_core::Machine;
use ribbit_debug::Breakpoints;
use ribbit_video::{HEIGHT, WIDTH};
use std::collections::HashMap;

const EDGE_CTOR: u32 = 0x4004_E0D8;
const HD: usize = 640; // the BIOS's own quarter-pixel grid
const SS: usize = 4; // anti-aliasing samples per HD pixel, per axis

#[derive(Clone, Copy, Debug)]
struct Edge {
    p: [(f32, f32); 3],
    line: bool,
    fills: [u32; 2],
}

#[derive(Clone, Copy, Debug)]
struct Fill {
    depth: u32,
    kind: u32,
    rgba: [u8; 4],
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [bios, cart, state, run_ms, out, rest @ ..] = &a[..] else {
        panic!("usage: hdframe <bios> <cart> <state> <run_ms> <out.png> [system.eeprom] [cart.sav]");
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
    let ms = m.clock.cpu_hz / 1000;
    let target = m.cpu.cycles + run_ms.parse::<u64>().unwrap() * ms;
    while m.cpu.cycles < target {
        m.run(target - m.cpu.cycles, &Breakpoints::default());
    }

    // Passes are split at framebuffer transfers to the LCD; the busiest
    // pass is kept (a full redraw, not a small dirty rectangle), with the
    // LCD frame as it stood when that pass was shown.
    let mut bp = Breakpoints::default();
    bp.add(EDGE_CTOR);
    let mut cur: (u64, Vec<Edge>) = (m.io.lcd.transfers, vec![]);
    let mut best: Vec<Edge> = vec![];
    let mut lcd = Box::new([0u32; WIDTH * HEIGHT]);
    let limit = m.cpu.cycles + run_window(rest) * ms;
    while m.cpu.cycles < limit {
        m.run(limit - m.cpu.cycles, &bp);
        if m.io.lcd.transfers != cur.0 {
            if cur.1.len() > best.len() {
                best = std::mem::take(&mut cur.1);
                m.render_frame(&mut lcd);
            }
            cur = (m.io.lcd.transfers, vec![]);
        }
        if m.cpu.pc == EDGE_CTOR {
            cur.1.push(read_edge(&m, m.cpu.regs[1]));
            m.run(1, &Breakpoints::default());
        }
    }
    let edges = best;
    let mut fills: HashMap<u32, Fill> = HashMap::new();
    for e in &edges {
        for f in e.fills {
            if f != 0 {
                fills.entry(f).or_insert_with(|| read_fill(&m, f));
            }
        }
    }
    println!("captured {} edges, {} fills", edges.len(), fills.len());
    let mut kinds: HashMap<u32, usize> = HashMap::new();
    for f in fills.values() {
        *kinds.entry(f.kind).or_default() += 1;
    }
    println!("fill types: {kinds:x?}");

    let hd = rasterize(&edges, &fills);
    write_side_by_side(out, &lcd, &hd);
}

/// Capture window in ms (RIBBIT_WINDOW, default 2000).
fn run_window(_: &[String]) -> u64 {
    std::env::var("RIBBIT_WINDOW").map_or(2000, |s| s.parse().unwrap())
}

fn rd(m: &Machine, a: u32, n: usize) -> Vec<u8> {
    (0..n as u32).map(|i| m.bus.peek8(a + i).unwrap_or(0)).collect()
}

fn read_edge(m: &Machine, rec: u32) -> Edge {
    let d = rd(m, rec, 0x20);
    let h = |o: usize| i16::from_le_bytes([d[o], d[o + 1]]) as f32;
    let w = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    Edge {
        p: [(h(0x10), h(0x12)), (h(0x14), h(0x16)), (h(0x18), h(0x1A))],
        line: d[0x1C] != 0,
        fills: [w(0x08), w(0x0C)],
    }
}

fn read_fill(m: &Machine, f: u32) -> Fill {
    let d = rd(m, f, 0x40);
    let w = |o: usize| u32::from_le_bytes(d[o..o + 4].try_into().unwrap());
    let c = |o: usize| u16::from_le_bytes([d[o], d[o + 1]]).min(255) as u8;
    Fill { depth: w(0x0C), kind: w(0x18), rgba: [c(0x28), c(0x26), c(0x24), c(0x2A)] }
}

/// Scanline fill at HD x SS: every edge flattened to segments; along each
/// sample row, crossing an edge toggles both of its fills (even-odd per
/// fill); the covered colour is the active fills composited by depth.
fn rasterize(edges: &[Edge], fills: &HashMap<u32, Fill>) -> Vec<[f32; 3]> {
    let n = HD * SS;
    let scale = SS as f32; // quarter pixels -> sample units
    let mut segs: Vec<((f32, f32), (f32, f32), [u32; 2])> = vec![];
    for e in edges {
        let pts: Vec<(f32, f32)> = if e.line {
            vec![e.p[0], e.p[2]]
        } else {
            (0..=16)
                .map(|i| {
                    let t = i as f32 / 16.0;
                    let u = 1.0 - t;
                    let x = u * u * e.p[0].0 + 2.0 * u * t * e.p[1].0 + t * t * e.p[2].0;
                    let y = u * u * e.p[0].1 + 2.0 * u * t * e.p[1].1 + t * t * e.p[2].1;
                    (x, y)
                })
                .collect()
        };
        for w in pts.windows(2) {
            let s = |p: (f32, f32)| (p.0 * scale, p.1 * scale);
            segs.push((s(w[0]), s(w[1]), e.fills));
        }
    }
    let mut order: Vec<u32> = fills.keys().copied().collect();
    order.sort_by_key(|f| fills[f].depth);
    let mut acc = vec![[0f32; 3]; HD * HD];
    let mut xs: Vec<(f32, [u32; 2])> = vec![];
    let mut active: HashMap<u32, bool> = HashMap::new();
    for sy in 0..n {
        let y = sy as f32 + 0.5;
        xs.clear();
        for &(a, b, f) in &segs {
            let (lo, hi) = if a.1 < b.1 { (a, b) } else { (b, a) };
            if y >= lo.1 && y < hi.1 {
                xs.push((lo.0 + (y - lo.1) / (hi.1 - lo.1) * (hi.0 - lo.0), f));
            }
        }
        xs.sort_by(|p, q| p.0.total_cmp(&q.0));
        active.clear();
        let mut i = 0;
        for sx in 0..n {
            let x = sx as f32 + 0.5;
            while i < xs.len() && xs[i].0 <= x {
                for f in xs[i].1 {
                    if f != 0 {
                        let v = active.entry(f).or_insert(false);
                        *v = !*v;
                    }
                }
                i += 1;
            }
            let mut c = [0f32; 3];
            for f in &order {
                if active.get(f).copied().unwrap_or(false) {
                    let fl = fills[f];
                    let al = fl.rgba[3] as f32 / 255.0;
                    for k in 0..3 {
                        c[k] = c[k] * (1.0 - al) + fl.rgba[k] as f32 * al;
                    }
                }
            }
            let p = &mut acc[(sy / SS) * HD + sx / SS];
            for k in 0..3 {
                p[k] += c[k] / (SS * SS) as f32;
            }
        }
    }
    acc
}

fn write_side_by_side(path: &str, lcd: &[u32; WIDTH * HEIGHT], hd: &[[f32; 3]]) {
    let (w, h) = (HD * 2 + 8, HD);
    let mut rgb = vec![255u8; w * h * 3];
    for y in 0..HD {
        for x in 0..HD {
            let p = lcd[(y / 4) * WIDTH + x / 4];
            let o = (y * w + x) * 3;
            rgb[o..o + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            let q = hd[y * HD + x];
            let o = (y * w + HD + 8 + x) * 3;
            rgb[o..o + 3].copy_from_slice(&[q[0] as u8, q[1] as u8, q[2] as u8]);
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}
