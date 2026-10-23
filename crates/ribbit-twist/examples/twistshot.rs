//! Research: boots the RockIt Twist as a Ribbit session does (system packages
//! and content from Ribbit's data folder), presses keys on a script and saves
//! screenshots.
//!
//! twistshot <data folder> <out folder> <seconds>
//!   TWIST_KEYS="20:RIGHT,23:ENTER,26.5:ENTER"   key presses (seconds:key, held 0.15 s)
//!   TWIST_SHOTS="30,31.5,40"                    screenshots (seconds)
//!   TWIST_EVERY=0.1 TWIST_FROM=40 TWIST_TO=42    also every 0.1 s in that stretch
//!   TWIST_MP3=1                                  log the MP3 accelerator's jobs (out/mp3)
//!   TWIST_SPRITES=1                              the sprite list beside each shot (.txt)
//!   TWIST_FRAME=1                                the video chip's frame too (.frame.png, unturned)
//!   TWIST_WAV=file                               what the DAC played
//!   TWIST_DMALOG=1                               DMA transfers (DAC, memory to memory)
//!   TWIST_RAMWATCH=lo-hi                         who wrote that RAM (hex), with write bursts
//!   TWIST_IOWATCH=lo-hi[@seconds]                writes to those registers (hex; TWIST_IOALL=1: all of them)
//!   TWIST_DUMP=lo-hi                             that RAM at the end (out/dump.bin)
//!
//! Keys: UP DOWN LEFT RIGHT ENTER HOME HELP POWER VOLUP VOLDOWN SWL SWR.

use ribbit_twist::{image, machine, package};
use std::path::{Path, PathBuf};

fn firmware(pkg: &Path, name: Option<&str>) -> Vec<u8> {
    let fw = pkg.join("Firmware");
    let entry = match name {
        Some(n) => n.to_string(),
        None => {
            let meta = std::fs::read(fw.join("meta.inf")).expect("meta.inf");
            package::Meta::parse(&String::from_utf8_lossy(&meta)).get("BinFile").expect("BinFile").to_string()
        }
    };
    std::fs::read(fw.join(&entry)).unwrap_or_else(|e| panic!("{}: {e}", fw.join(entry).display()))
}

fn key(name: &str) -> u32 {
    use machine::key::*;
    match name {
        "UP" => UP,
        "DOWN" => DOWN,
        "LEFT" => LEFT,
        "RIGHT" => RIGHT,
        "ENTER" => ENTER,
        "HOME" => HOME,
        "HELP" => HELP,
        "POWER" => POWER,
        "VOLUP" => VOL_UP,
        "VOLDOWN" => VOL_DOWN,
        "SWL" => SWITCH_LEFT,
        "SWR" => SWITCH_RIGHT,
        k => panic!("unknown key {k}"),
    }
}

fn list(var: &str) -> Vec<String> {
    std::env::var(var).map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()).unwrap_or_default()
}

fn save(path: &Path, px: &[u32]) {
    let (w, h) = (ribbit_twist::SCREEN_W, ribbit_twist::SCREEN_H);
    let mut rgb = Vec::with_capacity(w * h * 3);
    for p in px {
        rgb.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    let file = std::fs::File::create(path).expect("screenshot file");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().unwrap().write_image_data(&rgb).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let data = PathBuf::from(&a[1]);
    let out = PathBuf::from(&a[2]);
    let secs: f64 = a[3].parse().expect("seconds");
    std::fs::create_dir_all(&out).unwrap();
    let sys = data.join("rockit-twist").join("system");
    let boot = firmware(&sys.join("PCKT-0x004F0001-000000"), None);
    let base = firmware(&sys.join("PCKT-0x004F0002-000000"), None);
    let test = firmware(&sys.join("PCKT-0x004F000A-000000"), None);
    let mut volume = firmware(&sys.join("PCKT-0x004F000B-000000"), Some("FileSys"));
    let n = ribbit_twist::fat::add_host_folder(&mut volume, &data.join("rockit-twist").join("content")).expect("content");
    println!("content: {n} files");
    let mut t = machine::Twist::boot(image::code(&boot), image::code(&base), image::code(&test), volume);
    t.bus.mp3_log = std::env::var("TWIST_MP3").is_ok();
    if let Ok(f) = std::env::var("TWIST_MP3_FROM") {
        t.bus.mp3_log_from = (f.parse::<f64>().unwrap() * machine::CLOCK as f64) as u64;
    }
    // TWIST_RAMWATCH=lo-hi (hex): who writes there (pc, count, first and last time).
    if let Ok(r) = std::env::var("TWIST_RAMWATCH") {
        let (lo, hi) = r.split_once('-').expect("lo-hi");
        t.bus.ram_watch = Some((u32::from_str_radix(lo, 16).unwrap(), u32::from_str_radix(hi, 16).unwrap()));
    }
    // TWIST_IOWATCH=lo-hi[@from seconds] (hex): writes to those I/O registers.
    if let Ok(r) = std::env::var("TWIST_IOWATCH") {
        let (r, from) = r.split_once('@').map_or((r.as_str(), 0.0), |(a, b)| (a, b.parse().unwrap()));
        let (lo, hi) = r.split_once('-').expect("lo-hi");
        t.bus.watch = Some((u32::from_str_radix(lo, 16).unwrap(), u32::from_str_radix(hi, 16).unwrap()));
        t.bus.watch_writes_only = true;
        t.bus.watch_from = (from * machine::CLOCK as f64) as u64;
    }
    let presses: Vec<(f64, u32)> = list("TWIST_KEYS")
        .iter()
        .map(|s| {
            let (t, k) = s.split_once(':').expect("seconds:KEY");
            (t.parse().expect("seconds"), key(k))
        })
        .collect();
    let mut shots: Vec<f64> = list("TWIST_SHOTS").iter().map(|s| s.parse().expect("seconds")).collect();
    if let (Ok(e), Ok(f), Ok(to)) = (std::env::var("TWIST_EVERY"), std::env::var("TWIST_FROM"), std::env::var("TWIST_TO")) {
        let (e, mut s, to): (f64, f64, f64) = (e.parse().unwrap(), f.parse().unwrap(), to.parse().unwrap());
        while s <= to {
            shots.push(s);
            s += e;
        }
    }
    shots.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let step = machine::CLOCK / 200;
    let mut px = vec![0u32; ribbit_twist::SCREEN_W * ribbit_twist::SCREEN_H];
    let mut now = 0.0f64;
    let mut next_shot = 0;
    let started = std::time::Instant::now();
    while now < secs {
        t.bus.keys = presses.iter().filter(|(at, _)| now >= *at && now < at + 0.15).fold(0, |k, (_, b)| k | b);
        t.run(step);
        now += step as f64 / machine::CLOCK as f64;
        while next_shot < shots.len() && now >= shots[next_shot] {
            t.bus.screen(&mut px);
            let name = out.join(format!("shot-{:07.2}.png", shots[next_shot]));
            save(&name, &px);
            // TWIST_FRAME=1: also the video chip's frame as drawn now (240 x 320, unturned).
            if std::env::var("TWIST_FRAME").is_ok() {
                let (fw, fh) = (240usize, 320usize);
                let mut f = vec![0u16; fw * fh];
                t.bus.draw_frame(&mut f);
                let rgb: Vec<u8> = f.iter().flat_map(|p| [((p >> 11) << 3) as u8, ((p >> 5 & 63) << 2) as u8, ((p & 31) << 3) as u8]).collect();
                let file = std::fs::File::create(name.with_extension("frame.png")).unwrap();
                let mut enc = png::Encoder::new(std::io::BufWriter::new(file), fw as u32, fh as u32);
                enc.set_color(png::ColorType::Rgb);
                enc.write_header().unwrap().write_image_data(&rgb).unwrap();
            }
            if std::env::var("TWIST_SPRITES").is_ok() {
                let mut txt = format!("segment {:08X}
", t.bus.latch(0xD050_0088));
                for (n, x, y, w, h, hw) in t.bus.sprite_rects() {
                    txt += &format!("{n:4} frame x {x:5} y {y:5} {w:3}x{h:3} halves {:04X?}
", hw);
                }
                std::fs::write(name.with_extension("txt"), txt).unwrap();
            }
            next_shot += 1;
        }
    }
    // TWIST_WAV=file: what the DAC played (stereo, 16-bit, DAC_HZ).
    if let Ok(path) = std::env::var("TWIST_WAV") {
        let a = &t.bus.audio;
        let mut w = Vec::with_capacity(44 + a.len() * 2);
        let rate = machine::DAC_HZ as u32;
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + a.len() as u32 * 2).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&rate.to_le_bytes());
        w.extend_from_slice(&(rate * 4).to_le_bytes());
        w.extend_from_slice(&4u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(a.len() as u32 * 2).to_le_bytes());
        for s in a {
            w.extend_from_slice(&s.to_le_bytes());
        }
        std::fs::write(&path, w).unwrap();
        println!("wrote {} stereo samples to {path}", a.len() / 2);
    }
    if std::env::var("TWIST_DMALOG").is_ok() {
        // Memory to memory, with the "stays put" bit.
        let mut seen = std::collections::BTreeMap::new();
        for (_, ch, ctrl, src, _dst, bytes) in t.bus.dma_log.iter().filter(|e| e.3 < 0xC000_0000 && e.4 < 0xC000_0000) {
            *seen.entry((*ch, *ctrl, *src & 0xFFF0_0000 | if *ctrl & 0x80 != 0 { *src & 0xF_FFFF } else { 0 }, *bytes)).or_insert(0u32) += 1;
        }
        for ((ch, ctrl, src, bytes), n) in seen.iter().take(60) {
            println!("mem dma ch{ch} ctrl {ctrl:04X} src {src:08X} bytes {bytes:X}: {n} times");
        }
        for (when, ch, ctrl, src, dst, bytes) in t.bus.dma_log.iter().filter(|e| e.4 & 0xFFFF_FF00 == 0xC00D_0000).take(12) {
            println!("dma {:.4}s ch{ch} ctrl {ctrl:04X} {src:08X} -> {dst:08X} logged bytes {bytes:X}", *when as f64 / machine::CLOCK as f64);
        }
    }
    if t.bus.mp3_log {
        let dir = out.join("mp3");
        std::fs::create_dir_all(&dir).unwrap();
        let mut txt = String::new();
        for (i, j) in t.bus.mp3_jobs.iter().enumerate() {
            let regs: Vec<String> = j.regs.iter().map(|r| format!("{r:04X}")).collect();
            txt += &format!("{i:5} {:.4}s {} {}
", j.time as f64 / machine::CLOCK as f64, j.kind, regs.join(" "));
            for (k, (a, bytes)) in j.memory.iter().enumerate() {
                std::fs::write(dir.join(format!("job{i:04}-{k}-{a:08X}.bin")), bytes).unwrap();
            }
        }
        std::fs::write(dir.join("jobs.txt"), txt).unwrap();
        println!("{} MP3 accelerator jobs", t.bus.mp3_jobs.len());
    }
    // TWIST_DUMP=lo-hi (hex): that RAM at the end, to out/dump.bin.
    if let Ok(r) = std::env::var("TWIST_DUMP") {
        let (lo, hi) = r.split_once('-').expect("lo-hi");
        let (lo, hi) = (usize::from_str_radix(lo, 16).unwrap(), usize::from_str_radix(hi, 16).unwrap());
        std::fs::write(out.join("dump.bin"), &t.bus.ram[lo..hi]).unwrap();
    }
    if t.bus.ram_watch.is_some() {
        let mut by: std::collections::BTreeMap<u32, (u32, u64, u64, u32, u32)> = Default::default();
        for &(when, pc, a, _) in &t.bus.ram_writes {
            let e = by.entry(pc).or_insert((0, when, when, a, a));
            e.0 += 1;
            e.2 = when;
            e.3 = e.3.min(a);
            e.4 = e.4.max(a);
        }
        let c = machine::CLOCK as f64;
        // Bursts: writes from one pc with gaps under 1 ms.
        let mut burst: Option<(u32, u64, u64, u32, u32, u32)> = None;
        let flush = |b: &(u32, u64, u64, u32, u32, u32)| {
            println!("burst pc {:08X} {:.4}-{:.4}s {} writes {:08X}-{:08X}", b.0, b.1 as f64 / c, b.2 as f64 / c, b.5, b.3, b.4)
        };
        for &(when, pc, a, _) in &t.bus.ram_writes {
            match &mut burst {
                Some(b) if b.0 == pc && when - b.2 < machine::CLOCK / 1000 => {
                    b.2 = when;
                    b.3 = b.3.min(a);
                    b.4 = b.4.max(a);
                    b.5 += 1;
                }
                _ => {
                    if let Some(b) = &burst {
                        if b.5 > 16 {
                            flush(b);
                        }
                    }
                    burst = Some((pc, when, when, a, a, 1));
                }
            }
        }
        if let Some(b) = &burst {
            flush(b);
        }
        for (pc, (n, a, b, lo, hi)) in by {
            println!("ram write pc {pc:08X}: {n} writes {:.3}-{:.3}s, {lo:08X}-{hi:08X}", a as f64 / c, b as f64 / c);
        }
    }
    for w in t.bus.watched.iter().filter(|w| std::env::var("TWIST_IOALL").is_ok() || [" W D050018C", " W D05001E8", " W D05001F0", " W D050033C"].iter().any(|k| w.contains(k)) || (w.contains(" W D0500140") && w.ends_with("D3"))).take(20000) {
        println!("io {w}");
    }
    println!("{secs} s emulated in {:?}; pc {:08X}", started.elapsed(), t.cpu.r[15]);
}
