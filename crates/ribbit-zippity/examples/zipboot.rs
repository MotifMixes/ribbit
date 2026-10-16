//! Research: runs a Zippity from its system NAND and reports what it does.
//!
//! zipboot <nand.bin> [instructions]
//!
//! The NAND image is the raw dump with spare bytes (pages of 0x800 + 0x40,
//! Micron MT29F2G08AACWP, ID 2C DA 80 15). Environment variables:
//!
//! | Variable | Use |
//! |---|---|
//! | ZIPBOOT_DUMP=dir | Save RAM, on-chip RAM and the logs below at the end |
//! | ZIPBOOT_FRAMES=n | Save the picture every n instructions (frame_<millions>.ppm) |
//! | ZIPBOOT_EVERY=n | Save RAM every n instructions |
//! | ZIPBOOT_SAVE=file:n, ZIPBOOT_LOAD=file | Save the machine after n instructions; carry on from such a file |
//! | ZIPBOOT_PAD=state:from:to[;...] | The controllers' 10-bit state (hex) over instruction counts |
//! | ZIPBOOT_PRESS=port:mask:from:to[;...] | Flip GPIO input bits (port a, b or c) for a while |
//! | ZIPBOOT_WATCH=lo-hi, ZIPBOOT_WATCH_FROM=n | Log I/O accesses in that range (watch.log) |
//! | ZIPBOOT_RWATCH=lo-hi | Log CPU writes to memory in that range (rwatch.log) |
//! | ZIPBOOT_JUMPS[=n] | Print the last n jumps (600) |
//! | ZIPBOOT_WAV=file | Save the sound as a WAV file |
//! | ZIPBOOT_SPULOG | Print each sound channel start |

use ribbit_zippity::{OUTPUT_RATE, SCREEN_H, SCREEN_W, Zippity};
use std::collections::{BTreeMap, VecDeque};

fn range(var: &str) -> Option<(u32, u32)> {
    let w = std::env::var(var).ok()?;
    let (lo, hi) = w.split_once('-').expect("lo-hi in hex");
    Some((u32::from_str_radix(lo, 16).unwrap(), u32::from_str_radix(hi, 16).unwrap()))
}

fn save_ppm(path: &str, z: &Zippity) {
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut img);
    let mut out = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for p in img {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    std::fs::write(path, out).unwrap();
}

/// Saves interleaved 16-bit stereo samples as a WAV file.
fn save_wav(path: &str, samples: &[i16], rate: u32) {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flash = std::fs::read(&args[1]).expect("read the NAND image");
    let steps: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
    let mut z = Zippity::new(flash);
    if let Ok(path) = std::env::var("ZIPBOOT_LOAD") {
        z.load_state(&std::fs::read(&path).expect("read the state")).expect("a state");
    }
    let save: Option<(String, u64)> = std::env::var("ZIPBOOT_SAVE").ok().map(|s| {
        let (p, n) = s.rsplit_once(':').expect("ZIPBOOT_SAVE=file:n");
        (p.to_string(), n.parse().expect("an instruction count"))
    });
    {
        let p = z.probe();
        p.enabled = true;
        p.watch = range("ZIPBOOT_WATCH");
        p.watch_from = std::env::var("ZIPBOOT_WATCH_FROM").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
        p.rwatch = range("ZIPBOOT_RWATCH");
        p.spu_log = std::env::var_os("ZIPBOOT_SPULOG").is_some();
        if let Ok(s) = std::env::var("ZIPBOOT_PAD") {
            for item in s.split(';') {
                let f: Vec<&str> = item.split(':').collect();
                p.pad_script.push((u16::from_str_radix(f[0], 16).unwrap(), f[1].parse().unwrap(), f[2].parse().unwrap()));
            }
        }
        if let Ok(s) = std::env::var("ZIPBOOT_PRESS") {
            for item in s.split(';') {
                let f: Vec<&str> = item.split(':').collect();
                let port = match f[0] {
                    "a" => 0xC000_0000,
                    "b" => 0xC000_0020,
                    _ => 0xC000_0040,
                };
                p.presses.push((port, u32::from_str_radix(f[1], 16).unwrap(), f[2].parse().unwrap(), f[3].parse().unwrap()));
            }
        }
    }
    z.set_nand_logging(true);

    let jumps_on = std::env::var_os("ZIPBOOT_JUMPS").is_some();
    let jumps_max: usize = std::env::var("ZIPBOOT_JUMPS").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    let mut jumps: VecDeque<(u32, u32, u32)> = VecDeque::new();
    let mut hist: BTreeMap<u32, u64> = BTreeMap::new();
    let mut prev = 0u32;
    let every: Option<u64> = std::env::var("ZIPBOOT_EVERY").ok().and_then(|s| s.parse().ok());
    let frames: Option<u64> = std::env::var("ZIPBOOT_FRAMES").ok().and_then(|s| s.parse().ok());
    let dump_dir = std::env::var("ZIPBOOT_DUMP").ok();
    let keep_audio = std::env::var("ZIPBOOT_WAV").is_ok();
    let mut audio = Vec::new();
    while z.instructions() < steps {
        let ran = z.instructions();
        if let Some((path, at)) = &save
            && ran == *at
        {
            std::fs::write(path, z.save_state()).unwrap();
        }
        if let (Some(n), Some(dir)) = (frames, &dump_dir)
            && ran > 0
            && ran.is_multiple_of(n)
        {
            save_ppm(&format!("{dir}/frame_{:04}.ppm", ran / 1_000_000), &z);
        }
        if let (Some(n), Some(dir)) = (every, &dump_dir)
            && ran > 0
            && ran.is_multiple_of(n)
        {
            std::fs::write(format!("{dir}/ram_{}.bin", ran / 1_000_000), &z.ram()[..0x100_0000]).unwrap();
        }
        if !jumps_on && ran < steps.saturating_sub(200_000) {
            // Nothing watches each step: run to the next point of interest.
            let mut next = steps.saturating_sub(200_000);
            for at in [save.as_ref().map(|s| s.1), frames.map(|n| (ran / n + 1) * n), every.map(|n| (ran / n + 1) * n)].into_iter().flatten() {
                if at > ran {
                    next = next.min(at);
                }
            }
            z.run(next - ran);
        } else {
            z.step();
            let cur = z.cpu().current;
            if jumps_on {
                let thumb = z.cpu().thumb();
                let next = prev.wrapping_add(if thumb { 2 } else { 4 });
                if cur != next && cur != prev {
                    match jumps.back_mut() {
                        Some(j) if (j.0, j.1) == (prev, cur) => j.2 += 1,
                        _ => {
                            jumps.push_back((prev, cur, 1));
                            if jumps.len() > jumps_max {
                                jumps.pop_front();
                            }
                        }
                    }
                }
            }
            prev = cur;
            if z.instructions() > steps.saturating_sub(200_000) {
                *hist.entry(cur).or_default() += 1;
            }
        }
        for (pc, w) in z.take_undefined() {
            println!("undefined {w:08x} at {pc:08x}");
        }
        if keep_audio {
            audio.extend(z.take_audio());
        } else {
            z.take_audio();
        }
    }

    if jumps_on {
        println!("--- last jumps (from -> to, times) ---");
        for (from, to, n) in &jumps {
            println!("{from:08x} -> {to:08x}{}", if *n > 1 { format!("  x{n}") } else { String::new() });
        }
    }
    println!("--- first accesses ---");
    for l in &z.probe().first {
        println!("{l}");
    }
    let nand_log = std::mem::take(z.nand_log());
    println!("--- NAND ({} events) ---", nand_log.len());
    for l in nand_log.iter().take(200) {
        println!("{l}");
    }
    println!("--- 0xD0800000 operations: {} ---", z.probe().gfx_ops);
    println!("--- serial output ---\n{}\n--- end of serial output ---", z.log());
    let stats = std::mem::take(&mut z.probe().stats);
    println!("--- registers touched ({}) ---", stats.len());
    for (a, e) in &stats {
        println!(
            "{a:08x}  reads {:>8}  writes {:>8}  last {:08x}  first pc {:08x}",
            e.reads, e.writes, e.last_write, e.first_pc
        );
    }
    let cpu = z.cpu();
    println!("--- state after {} instructions ---", z.instructions());
    println!("pc {:08x} cpsr {:08x} mode {:02x} thumb {}", cpu.r[15], cpu.cpsr, cpu.mode(), cpu.thumb());
    for i in 0..16 {
        print!("r{i}={:08x} ", cpu.r[i]);
        if i % 4 == 3 {
            println!();
        }
    }
    println!("cp15 control {:08x} ttb {:08x}", cpu.cp15.control, cpu.cp15.ttb);
    if let Some(dir) = &dump_dir {
        std::fs::write(format!("{dir}/watch.log"), z.probe().watched.join("\n")).ok();
        std::fs::write(format!("{dir}/rwatch.log"), z.probe().rwatched.join("\n")).ok();
        std::fs::write(format!("{dir}/ram.bin"), z.ram()).unwrap();
        std::fs::write(format!("{dir}/iram.bin"), z.iram()).unwrap();
    }
    if let Ok(path) = std::env::var("ZIPBOOT_WAV") {
        save_wav(&path, &audio, OUTPUT_RATE.round() as u32);
    }
    let mut hot: Vec<_> = hist.into_iter().collect();
    hot.sort_by_key(|h| std::cmp::Reverse(h.1));
    println!("--- hottest pcs (last 200k) ---");
    for (pc, n) in hot.iter().take(16) {
        println!("{pc:08x} {n}");
    }
}
