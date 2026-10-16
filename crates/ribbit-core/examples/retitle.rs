//! Re-times the library's title cards after an emulation timing change: boots
//! each game the way the library's art capture does (straps 0x63FF_FFFF, the
//! given system EEPROM, MAME touch) and finds the moment whose screen best
//! matches the game's previously captured art (`<art_dir>/<crc>-<suffix>`).
//!   cargo run --release -p ribbit-core --example retitle -- <bios> <eeprom> <art_dir> <suffix> <max_ms> <cart>...
//! Prints "crc best_ms score" (mean absolute difference per channel, 0-255).
use ribbit_core::Machine;
use ribbit_debug::Breakpoints;
use ribbit_video::{HEIGHT, WIDTH};

/// The art, reduced to 160x160 RGB by averaging.
fn reference(path: &std::path::Path) -> Option<Vec<[f32; 3]>> {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).ok()?));
    let mut r = dec.read_info().ok()?;
    let mut buf = vec![0; r.output_buffer_size()?];
    let info = r.next_frame(&mut buf).ok()?;
    let (w, h, ch) = (info.width as usize, info.height as usize, info.color_type.samples());
    let (sx, sy) = (w / WIDTH, h / HEIGHT);
    let mut out = vec![[0f32; 3]; WIDTH * HEIGHT];
    for y in 0..HEIGHT {
        for x in 0..WIDTH {
            let mut acc = [0f32; 3];
            for dy in 0..sy {
                for dx in 0..sx {
                    let i = ((y * sy + dy) * w + x * sx + dx) * ch;
                    for (k, a) in acc.iter_mut().enumerate() {
                        *a += buf[i + k] as f32;
                    }
                }
            }
            out[y * WIDTH + x] = acc.map(|a| a / (sx * sy) as f32);
        }
    }
    Some(out)
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [bios, eeprom, art_dir, suffix, max_ms, carts @ ..] = &a[..] else {
        panic!("usage: retitle <bios> <eeprom> <art_dir> <suffix> <max_ms> <cart>...");
    };
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(bios).unwrap()).unwrap();
    let eeprom = std::fs::read(eeprom).unwrap();
    let max_ms: u64 = max_ms.parse().unwrap();
    std::thread::scope(|s| {
        for cart_path in carts {
            let (fw, eeprom) = (fw.clone(), eeprom.clone());
            s.spawn(move || {
                let cart = ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(cart_path).unwrap()).unwrap();
                let crc = cart.id().crc32;
                let Some(refimg) = reference(&std::path::Path::new(art_dir).join(format!("{crc:08x}{suffix}"))) else {
                    println!("{crc:08x} no-art {cart_path}");
                    return;
                };
                let mut m = Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
                m.record_trace_tail = false;
                m.io.eeprom.load(&eeprom);
                m.enable_experimental_mame_touch(Default::default());
                let per_ms = m.clock.cpu_hz / 1000;
                let none = Breakpoints::default();
                let mut frame = Box::new([0u32; WIDTH * HEIGHT]);
                let mut best = (f32::MAX, 0u64);
                let mut t = 3000;
                while t <= max_ms {
                    m.run_cycles((t * per_ms).saturating_sub(m.cpu.cycles), &none);
                    m.render_frame(&mut frame);
                    let mut diff = 0f32;
                    for (p, r) in frame.iter().zip(&refimg) {
                        let c = [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF].map(|c| c as f32);
                        diff += (c[0] - r[0]).abs() + (c[1] - r[1]).abs() + (c[2] - r[2]).abs();
                    }
                    let score = diff / (3 * WIDTH * HEIGHT) as f32;
                    if score < best.0 {
                        best = (score, t);
                    }
                    t += 250;
                }
                println!("{crc:08x} {} {:.1} {cart_path}", best.1, best.0);
            });
        }
    });
}
