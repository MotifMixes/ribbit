//! Runs a ClickStart for a number of frames and saves the last picture.
//!
//! csrun <bios.bin> <frames> <out.ppm> [cartridge.bin]
//!
//! CSRUN_TYPE=frame:text types text from that frame on (a key every 10
//! frames, held 4). CSRUN_MOUSE=frame:dx:dy:button;... moves the mouse.
//! CSRUN_KEYS=frame:code[:u];... sends raw key codes (hex; `u` = released;
//! `aN` = the arrow bits N, e.g. a4 = ARROW_RIGHT).
//! CSRUN_PEEK=addr,... prints those words (hex addresses) every 60 frames.
//! CSRUN_PPM_AT=frame,... also saves the picture at those frames
//! (<out>.<frame>.ppm).
//! CSRUN_RAM_AT=frame,... saves RAM (words 0-0x27ff, low byte first) at
//! those frames (<out>.<frame>.ram).
//! CSRUN_BREAK=addr,... prints the registers each time execution reaches
//! one of those addresses (hex; runs instruction by instruction).
//! CSRUN_WAV=file.wav saves the sound. CSRUN_EEPROM=file loads the 24C08
//! from the file (if there) and writes it back at the end.
//! CSRUN_TRACE=n prints the first n instructions' addresses and opcodes.
//! CSRUN_WATCH=addr[:from] prints where the word at addr (hex) is written
//! with a smaller value (from frame `from` on; runs instruction by
//! instruction).

use ribbit_clickstart::{CLOCK, ClickStart, SCREEN_H, SCREEN_W};
use std::io::Write;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bios = std::fs::read(&args[1]).expect("read the system ROM");
    let frames: u64 = args[2].parse().expect("a frame count");
    let mut cs = ClickStart::new(&bios);
    if let Some(cart) = args.get(4) {
        cs.insert_cartridge(&std::fs::read(cart).expect("read the cartridge"));
        cs.reset();
    }
    let eeprom = std::env::var("CSRUN_EEPROM").ok();
    if let Some(data) = eeprom.as_ref().and_then(|p| std::fs::read(p).ok()) {
        cs.set_eeprom(&data);
    }
    println!("reset: pc {:06x}", cs.cpu.lpc());
    if let Some(n) = std::env::var("CSRUN_TRACE").ok().and_then(|v| v.parse::<usize>().ok()) {
        // Steps until execution leaves ROM, then prints the last n.
        let mut ring = std::collections::VecDeque::new();
        for i in 0..2_000_000_000u64 {
            let pc = cs.cpu.lpc();
            if pc < 0x4000 {
                for (pc, op, r) in &ring {
                    println!("{pc:06x}: {op:04x}  {r:04x?}");
                }
                println!("left ROM at instruction {i}: pc {pc:06x}, frame {}", cs.frames());
                return;
            }
            ring.push_back((pc, cs.peek(pc), cs.cpu.r));
            if ring.len() > n {
                ring.pop_front();
            }
            cs.step_one();
        }
    }
    let typing = std::env::var("CSRUN_TYPE").ok().map(|v| {
        let (f, t) = v.split_once(':').unwrap();
        (f.parse::<u64>().unwrap(), t.to_string())
    });
    let mouse: Vec<(u64, i32, i32, bool)> = std::env::var("CSRUN_MOUSE")
        .map(|v| {
            v.split(';')
                .map(|m| {
                    let p: Vec<&str> = m.split(':').collect();
                    (p[0].parse().unwrap(), p[1].parse().unwrap(), p[2].parse().unwrap(), p[3] == "1")
                })
                .collect()
        })
        .unwrap_or_default();
    let keys: Vec<(u64, u16, bool)> = std::env::var("CSRUN_KEYS")
        .map(|v| {
            v.split(';')
                .map(|k| {
                    let p: Vec<&str> = k.split(':').collect();
                    let code = match p[1].strip_prefix('a') {
                        Some(a) => 0x100 | u16::from_str_radix(a, 16).unwrap(),
                        None => u16::from_str_radix(p[1], 16).unwrap(),
                    };
                    (p[0].parse().unwrap(), code, p.get(2) != Some(&"u"))
                })
                .collect()
        })
        .unwrap_or_default();
    let peeks: Vec<u32> = std::env::var("CSRUN_PEEK")
        .map(|v| v.split(',').map(|a| u32::from_str_radix(a, 16).unwrap()).collect())
        .unwrap_or_default();
    let ppm_at: Vec<u64> = std::env::var("CSRUN_PPM_AT")
        .map(|v| v.split(',').map(|f| f.parse().unwrap()).collect())
        .unwrap_or_default();
    let ram_at: Vec<u64> = std::env::var("CSRUN_RAM_AT")
        .map(|v| v.split(',').map(|f| f.parse().unwrap()).collect())
        .unwrap_or_default();
    let breaks: Vec<u32> = std::env::var("CSRUN_BREAK")
        .map(|v| v.split(',').map(|a| u32::from_str_radix(a, 16).unwrap()).collect())
        .unwrap_or_default();
    let watch: Option<(u32, u64)> = std::env::var("CSRUN_WATCH").ok().map(|v| {
        let p: Vec<&str> = v.split(':').collect();
        (u32::from_str_radix(p[0], 16).unwrap(), p.get(1).map_or(0, |f| f.parse().unwrap()))
    });
    let mut wav: Vec<i16> = Vec::new();
    let spu_log = std::env::var_os("CSRUN_SPU").is_some();
    if spu_log {
        cs.trace_sound();
    }
    let mut off_seen = false;
    let start = Instant::now();
    for f in 0..frames {
        for &(at, dx, dy, b) in &mouse {
            if at == f {
                cs.mouse(dx, dy, b);
            }
        }
        for &(at, code, down) in &keys {
            if at == f {
                if code & 0x100 != 0 {
                    cs.arrow(code as u8, down);
                } else {
                    cs.key(code as u8, down);
                }
            }
        }
        if let Some((from, text)) = &typing {
            if f >= *from && (f - from) / 10 < text.len() as u64 {
                let c = text.as_bytes()[((f - from) / 10) as usize] as char;
                match (f - from) % 10 {
                    0 => cs.key(ribbit_clickstart::key_code(c).unwrap_or(0x3f), true),
                    4 => cs.key(0, false),
                    _ => {}
                }
            }
        }
        if let Some((addr, from)) = watch.filter(|&(_, from)| f >= from) {
            let _ = from;
            let end = cs.frames() + 1;
            while cs.frames() < end {
                let (pc, old) = (cs.cpu.lpc(), cs.peek(addr));
                cs.step_one();
                let new = cs.peek(addr);
                if new < old {
                    let sp = cs.cpu.r[0] as u32;
                    println!(
                        "f{f} {pc:06x}: [{addr:04x}] {old:04x} -> {new:04x} (stack {:04x?})",
                        (1..13).map(|o| cs.peek(sp + o)).collect::<Vec<_>>()
                    );
                }
            }
        } else if breaks.is_empty() {
            cs.run_frame();
        } else {
            let end = cs.frames() + 1;
            while cs.frames() < end {
                if breaks.contains(&cs.cpu.lpc()) {
                    println!("f{f} {:06x}: {:04x?}", cs.cpu.lpc(), cs.cpu.r);
                }
                cs.step_one();
            }
        }
        if cs.powered_off() && !off_seen {
            off_seen = true;
            println!("powered off at frame {f} ({:.1} s)", cs.cycles() as f64 / CLOCK as f64);
        }
        if ram_at.contains(&f) {
            let ram: Vec<u8> = (0..0x2800).flat_map(|a| cs.peek(a).to_le_bytes()).collect();
            std::fs::write(format!("{}.{f}.ram", args[3]), ram).unwrap();
        }
        if ppm_at.contains(&f) {
            save_ppm(&cs, &format!("{}.{f}.ppm", args[3]));
        }
        wav.extend(cs.take_audio());
        for l in cs.take_sound_log() {
            println!("f{f} {l}");
        }
        if f % 60 == 59 {
            println!(
                "frame {}: pc {:06x} sr {:04x} sp {:04x} line {}",
                cs.frames(), cs.cpu.lpc(), cs.cpu.r[ribbit_clickstart::cpu::SR], cs.cpu.r[0], cs.line()
            );
            if !peeks.is_empty() {
                println!("  peek {:04x?}", peeks.iter().map(|&a| cs.peek(a)).collect::<Vec<_>>());
            }
        }
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{} instructions, {:.2} s emulated in {secs:.2} s ({:.1}x real time)",
        cs.instructions(),
        cs.cycles() as f64 / CLOCK as f64,
        cs.cycles() as f64 / CLOCK as f64 / secs
    );
    if !cs.cpu.unknown.is_empty() {
        println!("unknown opcodes: {:x?}", cs.cpu.unknown);
    }
    let io = cs.io_regs();
    println!(
        "int ctrl {:04x} status {:04x} fiq_sel {:04x} uart {:04x?} ioa-c {:04x?}",
        io[0x21], io[0x22], io[0x2e], &io[0x30..0x38], &io[0x00..0x10]
    );
    println!("cpu irq {} fiq {} in_irq {} in_fiq {}", cs.cpu.irq_enable, cs.cpu.fiq_enable, cs.cpu.in_irq, cs.cpu.in_fiq);
    let uart = cs.take_uart_out();
    if !uart.is_empty() {
        println!("uart out: {:02x?}", &uart[..uart.len().min(64)]);
    }
    let peak = wav.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    println!("sound: {} samples, peak {peak}", wav.len() / 2);
    if let Ok(path) = std::env::var("CSRUN_WAV") {
        let rate = ribbit_clickstart::OUTPUT_RATE as u32;
        let mut w = std::fs::File::create(path).unwrap();
        let data = wav.len() as u32 * 2;
        w.write_all(b"RIFF").unwrap();
        w.write_all(&(36 + data).to_le_bytes()).unwrap();
        w.write_all(b"WAVEfmt ").unwrap();
        w.write_all(&16u32.to_le_bytes()).unwrap();
        w.write_all(&[1, 0, 2, 0]).unwrap();
        w.write_all(&rate.to_le_bytes()).unwrap();
        w.write_all(&(rate * 4).to_le_bytes()).unwrap();
        w.write_all(&[4, 0, 16, 0]).unwrap();
        w.write_all(b"data").unwrap();
        w.write_all(&data.to_le_bytes()).unwrap();
        for s in &wav {
            w.write_all(&s.to_le_bytes()).unwrap();
        }
    }
    if let Some(p) = &eeprom {
        println!("eeprom: {}", if cs.eeprom_dirty() { "written" } else { "unchanged" });
        std::fs::write(p, cs.eeprom()).expect("write the EEPROM");
    }
    save_ppm(&cs, &args[3]);
}

fn save_ppm(cs: &ClickStart, path: &str) {
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    cs.render(&mut img);
    let mut out = std::fs::File::create(path).expect("create the picture");
    write!(out, "P6\n{SCREEN_W} {SCREEN_H}\n255\n").unwrap();
    let bytes: Vec<u8> = img.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    out.write_all(&bytes).unwrap();
}
