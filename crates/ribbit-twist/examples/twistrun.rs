//! Research: runs a RockIt Twist firmware image on `ribbit_twist::machine`
//! and reports where it got to, what it printed and which unmodelled
//! registers it read most.
//!
//! cargo run --release -p ribbit-twist --example twistrun -- <package.lfp | image> [instructions]

use ribbit_twist::{image, machine, package};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: twistrun <package.lfp | firmware image> [instructions]");
    let budget: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(200_000_000);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let data = if ribbit_core::zip::is_zip(&data) {
        let meta = ribbit_core::zip::extract_named(&data, "Firmware/meta.inf").expect("no meta.inf");
        let meta = package::Meta::parse(&String::from_utf8_lossy(&meta));
        let bin = meta.get("BinFile").expect("no BinFile");
        ribbit_core::zip::extract_named(&data, &format!("Firmware/{bin}")).expect("BinFile missing")
    } else {
        data
    };
    let mut t = machine::Twist::new(image::code(&data));
    let mut export_names: std::collections::HashMap<u32, String> = Default::default();
    // RIBBIT_WATCH=lo-hi (hex): log accesses to those I/O registers.
    t.bus.watch = std::env::var("RIBBIT_WATCH").ok().and_then(|w| {
        let (lo, hi) = w.split_once('-')?;
        Some((u32::from_str_radix(lo, 16).ok()?, u32::from_str_radix(hi, 16).ok()?))
    });
    // RIBBIT_WATCH_WRITES=1: writes only; RIBBIT_WATCH_SKIP=a,b (hex): not these.
    t.bus.watch_writes_only = std::env::var("RIBBIT_WATCH_WRITES").is_ok();
    // RIBBIT_WATCH_FROM=cycles: start watching then.
    t.bus.watch_from = std::env::var("RIBBIT_WATCH_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    t.bus.watch_skip = std::env::var("RIBBIT_WATCH_SKIP")
        .map(|s| s.split(',').filter_map(|a| u32::from_str_radix(a.trim(), 16).ok()).collect())
        .unwrap_or_default();
    // RIBBIT_EMMC=<image>: an eMMC chip (4 GB) holding that image from
    // sector 0.
    if let Ok(p) = std::env::var("RIBBIT_EMMC") {
        let mut image = std::fs::read(&p).unwrap_or_else(|e| panic!("reading {p}: {e}"));
        // As on a unit that has finished updating (RIBBIT_KEEP_UPGRADE=1:
        // keep the marker, for the firmware's update mode).
        if std::env::var("RIBBIT_KEEP_UPGRADE").is_err()
            && ribbit_twist::fat::delete_root_entry(&mut image, "FIRMWARE_UPGRADE")
        {
            println!("eMMC: removed the FIRMWARE_UPGRADE marker");
        }
        // RIBBIT_ADD=<folder>: its files are written onto the volume (the
        // folder stands for F:\); RIBBIT_VOLUME_OUT=<file> saves the result.
        if let Ok(dir) = std::env::var("RIBBIT_ADD") {
            let n =
                ribbit_twist::fat::add_host_folder(&mut image, std::path::Path::new(&dir)).expect("adding the folder");
            println!("eMMC: added {n} files from {dir}");
            if let Ok(out) = std::env::var("RIBBIT_VOLUME_OUT") {
                std::fs::write(out, &image).expect("saving the volume");
            }
        }
        // RIBBIT_SYSTEM=<Base .lfp> [RIBBIT_MFGTEST=<MfgTest .lfp>]: the
        // whole chip as the bootloader reads it (disk.rs), the volume at
        // disk::FAT_AREA.
        if let Ok(sys) = std::env::var("RIBBIT_SYSTEM") {
            let system = package_code(&sys);
            let mfg = std::env::var("RIBBIT_MFGTEST").map(|p| package_code(&p)).unwrap_or_else(|_| vec![0; 512]);
            use ribbit_twist::disk::{Partition, build};
            image = build(&[Partition { data: &mfg, load: 0 }, Partition { data: &system, load: 0 }], &image);
            t.clock_fns = ["System_getCount", "System_getHighPrecisionCount"]
                .iter()
                .filter_map(|n| ribbit_twist::machine::export(&system, n))
                .collect();
            t.vblank_wait = ribbit_twist::machine::vblank_wait(&system);
            // RIBBIT_CALLS=sec: count calls to each System export from then.
            if let Some(from) = std::env::var("RIBBIT_CALLS").ok().and_then(|v| v.parse::<f64>().ok()) {
                for (_, f) in exports(&system) {
                    t.counts.insert(f, 0);
                }
                t.count_from = (from * machine::CLOCK as f64) as u64;
                export_names = exports(&system).into_iter().map(|(n, f)| (f, n)).collect();
            }
            println!("System clock functions: {:X?}, frame wait {:X?}", t.clock_fns, t.vblank_wait);
            // RIBBIT_FACTORY=1: the manufacturing data asks for the factory test.
            if std::env::var("RIBBIT_FACTORY").is_ok() {
                ribbit_twist::disk::set_factory_test(&mut image, true);
                println!("eMMC: factory test flag set");
            }
            println!(
                "eMMC: boot header, partition table, System ({} bytes), volume at sector {:#x}",
                system.len(),
                ribbit_twist::disk::FAT_AREA
            );
        }
        t.bus.emmc = Some(ribbit_twist::sd::Card::emmc(image, 7_634_944));
        if let Some(c) = &mut t.bus.emmc {
            c.logging = true;
        }
    }
    t.bus.skip_drawing = std::env::var("RIBBIT_NO_DRAW").is_ok();
    // RIBBIT_RAM_WATCH=lo-hi (hex): writes into that RAM range, by pc.
    t.bus.ram_watch = std::env::var("RIBBIT_RAM_WATCH").ok().and_then(|w| {
        let (lo, hi) = w.split_once('-')?;
        Some((u32::from_str_radix(lo, 16).ok()?, u32::from_str_radix(hi, 16).ok()?))
    });
    t.profile = true;
    // RIBBIT_BREAK=addr (hex): registers each time execution gets there.
    t.break_at = std::env::var("RIBBIT_BREAK").ok().and_then(|v| u32::from_str_radix(&v, 16).ok());
    // RIBBIT_LOAD_STATE=file: start from a save state (RIBBIT_SAVE_STATE
    // writes one at the end), for timing a stretch again and again.
    if let Ok(p) = std::env::var("RIBBIT_LOAD_STATE") {
        let state = std::fs::read(&p).unwrap_or_else(|e| panic!("reading {p}: {e}"));
        t.load_state(&state).expect("loading the state");
        t.profile = false;
    }
    let started = t.bus.now;
    let clock = std::time::Instant::now();
    // RIBBIT_FRAMES=dir: save the screen (as the panel shows it) whenever it
    // changes, checked every emulated 1/30 s.
    let frames_dir = std::env::var("RIBBIT_FRAMES").ok();
    let stop = match &frames_dir {
        None => t.run(budget),
        Some(dir) => {
            std::fs::create_dir_all(dir).expect("creating the frames folder");
            let usb_at: Option<f64> = std::env::var("RIBBIT_USB_AT").ok().and_then(|s| s.parse().ok());
            // RIBBIT_TURN=sec:rotary:steps,... (rotary 0 Spinner, 1 Yellow,
            // 2 Green, 3 Red): turns, once each.
            let mut turns: Vec<(f64, usize, i32)> = std::env::var("RIBBIT_TURN")
                .unwrap_or_default()
                .split(',')
                .filter_map(|p| {
                    let mut f = p.split(':');
                    Some((f.next()?.parse().ok()?, f.next()?.parse().ok()?, f.next()?.parse().ok()?))
                })
                .collect();
            let presses: Vec<(f64, u32, f64)> = std::env::var("RIBBIT_PRESS")
                .unwrap_or_default()
                .split(',')
                .filter_map(|p| {
                    let mut f = p.split(':');
                    let from = f.next()?.parse().ok()?;
                    let mask = u32::from_str_radix(f.next()?.trim_start_matches("0x"), 16).ok()?;
                    Some((from, mask, f.next().and_then(|l| l.parse().ok()).unwrap_or(0.2)))
                })
                .collect();
            let slice = machine::CLOCK / 30;
            let mut pixels = vec![0u32; ribbit_twist::SCREEN_W * ribbit_twist::SCREEN_H];
            let mut last: Vec<u32> = Vec::new();
            let (mut done, mut saved) = (0u64, 0);
            let speed_check = std::env::var("RIBBIT_SPEED").is_ok();
            let (mut half_real, mut half_n, mut half_ins) = (0.0f64, 0, 0u64);
            let mut stop = machine::Stop::Budget;
            while done < budget {
                // RIBBIT_PRESS=sec:mask:sec,... (mask in hex, `machine::key`
                // bits): hold those buttons from the first time for the second.
                let secs = t.bus.now as f64 / machine::CLOCK as f64;
                t.bus.keys = presses
                    .iter()
                    .filter(|(from, _, len)| secs >= *from && secs < from + len)
                    .fold(0, |k, (_, m, _)| k | m);
                // RIBBIT_USB_AT=sec: the USB cable is plugged in from then on.
                t.bus.usb_plugged = usb_at.is_some_and(|at| secs >= at);
                turns.retain(|&(at, r, n)| {
                    if secs >= at {
                        t.bus.turn(r, n);
                        false
                    } else {
                        true
                    }
                });
                let ran = std::time::Instant::now();
                stop = t.run(slice.min(budget - done));
                // RIBBIT_SPEED=1: report emulated half-seconds that took
                // longer than real ones (the app's sound would run dry).
                if speed_check {
                    half_real += ran.elapsed().as_secs_f64();
                    half_n += 1;
                    if half_n == 15 {
                        let ins = t.cpu.instructions - half_ins;
                        if half_real > 0.5 {
                            println!(
                                "slow at {secs:.1} s: {:.0}% speed, {:.0} M instructions/s",
                                50.0 / half_real,
                                ins as f64 / half_real / 1e6
                            );
                        } else if std::env::var("RIBBIT_SPEED").as_deref() == Ok("all") {
                            println!("{secs:.1} s: {:.0}% speed, {:.0} M instructions/s", 50.0 / half_real, ins as f64 / half_real / 1e6);
                        }
                        (half_real, half_n, half_ins) = (0.0, 0, t.cpu.instructions);
                    }
                }
                done += slice;
                t.bus.screen(&mut pixels);
                if pixels != last {
                    let secs = t.bus.now as f64 / machine::CLOCK as f64;
                    let path = format!("{dir}/frame-{saved:04}-{secs:07.2}s.png");
                    write_png(&path, &pixels);
                    println!("screen changed at {secs:.2} s -> {path}");
                    last.clone_from(&pixels);
                    saved += 1;
                }
                if stop != machine::Stop::Budget {
                    break;
                }
            }
            stop
        }
    };
    // RIBBIT_STATE_TEST=1: a save state now, loaded back, then 2 s more.
    if let Ok(p) = std::env::var("RIBBIT_SAVE_STATE") {
        std::fs::write(&p, t.save_state()).expect("writing the state");
    }
    if std::env::var("RIBBIT_STATE_TEST").is_ok() {
        let t0 = std::time::Instant::now();
        let state = t.save_state();
        let saved = t0.elapsed();
        let frames = t.bus.panel_frames;
        t.load_state(&state).expect("loading the state");
        t.run(2 * machine::CLOCK);
        println!(
            "state: {:.1} MB, saved in {:.0} ms, loaded; {} panel frames in the 2 s after",
            state.len() as f64 / 1e6,
            saved.as_secs_f64() * 1e3,
            t.bus.panel_frames - frames
        );
    }
    println!(
        "{stop:?} after {} instructions ({:.2} s emulated, {:.2} s real); pc {:08X} cpsr {:08X}",
        t.cpu.instructions,
        (t.bus.now - started) as f64 / machine::CLOCK as f64,
        clock.elapsed().as_secs_f64(),
        t.cpu.r[15],
        t.cpu.cpsr
    );
    println!("registers: {:08X?}", &t.cpu.r[..15]);
    if !t.bus.uart.is_empty() {
        println!("\n== UART\n{}", String::from_utf8_lossy(&t.bus.uart));
    }
    let mut regs: Vec<_> = t.bus.unmodelled.iter().map(|(a, (n, pc))| (*n, *a, *pc)).collect();
    regs.sort_by(|a, b| b.0.cmp(&a.0));
    println!("\n== unmodelled registers read (most first)");
    for (n, a, pc) in regs.iter().take(25) {
        println!("  {a:08X}  {n:>10}  first at {pc:08X}");
    }
    if !export_names.is_empty() {
        let mut calls: Vec<_> = t.counts.iter().filter(|(_, n)| **n > 0).collect();
        calls.sort_by(|a, b| b.1.cmp(a.1));
        println!("
== System calls since RIBBIT_CALLS (most first)");
        for (f, n) in calls.iter().take(60) {
            println!("  {f:08X}  {n:>9}  {}", export_names.get(f).map_or("?", |s| s));
        }
    }
    // RIBBIT_PEEK=addr:words[,addr:words] (hex addresses): memory after the run.
    if let Ok(spec) = std::env::var("RIBBIT_PEEK") {
        use ribbit_arm::ArmBus;
        for part in spec.split(',') {
            let (a, n) = part.split_once(':').unwrap_or((part, "8"));
            let a = u32::from_str_radix(a, 16).expect("hex address");
            let n: u32 = n.parse().unwrap_or(8);
            let words: Vec<String> = (0..n).map(|i| format!("{:08X}", t.bus.read32(a + 4 * i))).collect();
            println!("peek {a:08X}: {}", words.join(" "));
        }
    }
    let spu = t.bus.spu_state();
    if !spu.is_empty() {
        println!("
== sound unit: channels playing");
        for l in spu {
            println!("  {l}");
        }
    }
    // RIBBIT_SOUND=file: what the speaker gets (the sound unit with the
    // DAC), a stereo WAV at the sound unit's output rate.
    if let Ok(path) = std::env::var("RIBBIT_SOUND") {
        let rate = ribbit_twist::sound::OUTPUT_RATE as u32;
        let pcm: Vec<u8> = t.bus.sound.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = Vec::with_capacity(44 + pcm.len());
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        for v in [16u32, 1 | 2 << 16, rate, rate * 4, 4 | 16 << 16] {
            wav.extend_from_slice(&v.to_le_bytes());
        }
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(&pcm);
        std::fs::write(&path, wav).expect("writing the WAV");
        println!("wrote {:.1} s of sound to {path}", t.bus.sound.len() as f64 / 2.0 / rate as f64);
    }
    // RIBBIT_WAV=file: everything the DAC received, as a 44.1 kHz stereo WAV.
    if let Ok(path) = std::env::var("RIBBIT_WAV") {
        let pcm: Vec<u8> = t.bus.audio.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut wav = Vec::with_capacity(44 + pcm.len());
        let rate = machine::DAC_HZ as u32;
        for part in [
            &b"RIFF"[..],
            &(36 + pcm.len() as u32).to_le_bytes(),
            b"WAVEfmt ",
            &16u32.to_le_bytes(),
            &1u16.to_le_bytes(),
            &2u16.to_le_bytes(),
        ] {
            wav.extend_from_slice(part);
        }
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
        wav.extend_from_slice(&pcm);
        std::fs::write(&path, wav).expect("writing the WAV");
        println!("wrote {} s of DAC output to {path}", t.bus.audio.len() as f64 / 2.0 / rate as f64);
    }
    // RIBBIT_SCREEN=addr:file.png (hex addr): a picture from memory as the
    // screen shows it. Pictures are stored as the panel scans them, 240
    // wide and 320 tall (RGB565), and the panel is mounted sideways: the
    // 320 x 240 screen is the picture turned a quarter left.
    if let Ok(spec) = std::env::var("RIBBIT_SCREEN") {
        use ribbit_arm::ArmBus;
        let (a, path) = spec.split_once(':').expect("addr:file.png");
        let a = u32::from_str_radix(a, 16).expect("hex address");
        let (w, h) = (ribbit_twist::SCREEN_W as u32, ribbit_twist::SCREEN_H as u32);
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for i in 0..w * h {
            let (x, y) = (i % w, i / w);
            // Stored row x, column (h - 1 - y).
            let p = t.bus.read16(a + 2 * (x * h + (h - 1 - y))) as u32;
            let (r, g, b) = ((p >> 11) & 31, (p >> 5) & 63, p & 31);
            rgb.extend_from_slice(&[(r * 255 / 31) as u8, (g * 255 / 63) as u8, (b * 255 / 31) as u8]);
        }
        let file = std::io::BufWriter::new(std::fs::File::create(path).expect("creating the PNG"));
        let mut enc = png::Encoder::new(file, w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.write_header().and_then(|mut wr| wr.write_image_data(&rgb)).expect("writing the PNG");
        println!("wrote the 320x240 RGB565 image at {a:08X} to {path}");
    }
    // RIBBIT_DUMP=addr:bytes:file (hex addr and size): memory to a file.
    if let Ok(spec) = std::env::var("RIBBIT_DUMP") {
        use ribbit_arm::ArmBus;
        let parts: Vec<&str> = spec.splitn(3, ':').collect();
        if let [a, n, file] = parts.as_slice() {
            let (a, n) = (u32::from_str_radix(a, 16).expect("hex"), u32::from_str_radix(n, 16).expect("hex"));
            let bytes: Vec<u8> = (0..n).map(|i| t.bus.read8(a + i)).collect();
            std::fs::write(file, bytes).expect("writing the dump");
            println!("dumped {n:#x} bytes from {a:08X} to {file}");
        }
    }
    // RIBBIT_TASKS=1: the operating system's tasks (uC/OS-II), found by the
    // name pointers in their control blocks, with their saved stack frames
    // (a blocked task's saved PC is in it).
    if std::env::var("RIBBIT_TASKS").is_ok() {
        use ribbit_arm::ArmBus;
        let ram = &t.bus.ram;
        let cstr = |a: u32| -> Option<String> {
            let a = a as usize;
            let s = ram.get(a..a + 48)?;
            let end = s.iter().position(|&b| b == 0)?;
            let s = &s[..end];
            (end >= 4 && s.iter().all(|b| (0x20..0x7F).contains(b))).then(|| String::from_utf8_lossy(s).into_owned())
        };
        println!("\n== tasks (name pointer at, name, words before it)");
        let code_end = image::code(&data).len() as u32;
        let mut found = Vec::new();
        for at in (0x0008_0000..0x0100_0000usize).step_by(4) {
            let p = u32::from_le_bytes(ram[at..at + 4].try_into().unwrap());
            if p >= 0x7_0000 && p < code_end {
                if let Some(name) = cstr(p) {
                    if name.contains("Task") || name.contains("uC/OS") {
                        found.push((at as u32, name));
                    }
                }
            }
        }
        for (at, name) in &found {
            let words: Vec<String> = (0..24).map(|i| format!("{:08X}", t.bus.read32(at - 96 + 4 * i))).collect();
            println!("  {at:08X} {name}\n    {}", words.join(" "));
        }
    }
    if let Some(card) = &t.bus.emmc {
        // Commands, with runs of sector reads folded into ranges.
        let mut lines: Vec<String> = Vec::new();
        let mut run: Option<(u64, u64)> = None;
        for l in &card.log {
            if let Some(n) = l.strip_prefix("  read sector ").and_then(|n| n.parse::<u64>().ok()) {
                run = match run {
                    Some((a, b)) if b + 1 == n => Some((a, n)),
                    Some((a, b)) => {
                        lines.push(format!("read {a}-{b}"));
                        Some((n, n))
                    }
                    None => Some((n, n)),
                };
                continue;
            }
            if let Some((a, b)) = run.take() {
                lines.push(format!("read {a}-{b}"));
            }
            if !l.starts_with("CMD17 ")
                && !l.starts_with("CMD18 ")
                && !l.starts_with("CMD13 ")
                && !l.starts_with("CMD12 ")
            {
                lines.push(l.clone());
            }
        }
        if let Some((a, b)) = run {
            lines.push(format!("read {a}-{b}"));
        }
        std::fs::write("twist-emmc.txt", lines.join("\n")).expect("writing twist-emmc.txt");
        println!("\n== eMMC: {} commands; reads folded (all in twist-emmc.txt; last 60 lines)", card.log.len());
        for l in &lines[lines.len().saturating_sub(60)..] {
            println!("  {l}");
        }
    }
    println!("\n== audio: {} samples written to the DAC", t.bus.audio.len());
    let modes = t.bus.modes_seen.borrow();
    if !modes.is_empty() {
        println!("== drawing modes seen (first time):");
        let mut v: Vec<_> = modes.iter().collect();
        v.sort_by_key(|e| *e.1);
        for (k, &at) in v {
            println!("  {:7.2} s  {k}", at as f64 / machine::CLOCK as f64);
        }
    }
    println!("== video decoder: {} frames decoded, {} failed", t.bus.mpeg_frames.0, t.bus.mpeg_frames.1);
    println!(
        "== video: {} frames drawn by the video chip, {} sent to the panel",
        t.bus.frames_drawn, t.bus.panel_frames
    );
    // RIBBIT_SPRITE_AT=x,y (in the 240 x 320 frame): the sprites covering
    // that point.
    if let Some((x, y)) = std::env::var("RIBBIT_SPRITE_AT")
        .ok()
        .and_then(|v| v.split_once(',').and_then(|(a, b)| Some((a.parse::<i32>().ok()?, b.parse::<i32>().ok()?))))
    {
        println!("== sprites at ({x}, {y})");
        for (n, x0, y0, w, h, e) in t.bus.sprite_rects() {
            if (x0..x0 + w as i32).contains(&x) && (y0..y0 + h as i32).contains(&y) {
                let hw: Vec<String> = e.iter().map(|v| format!("{v:04x}")).collect();
                println!("  {n:3} at ({x0},{y0}) {w}x{h}  {}", hw.join(" "));
            }
        }
    }
    // RIBBIT_SPRITE_LIST=1: every sprite in the list, with the global
    // sprite controls (0x108, 0x1FC) and each one's extended attributes.
    if std::env::var("RIBBIT_SPRITE_LIST").is_ok() {
        println!(
            "== sprite list: 0x108 {:08X}, 0x1FC {:08X}",
            t.bus.latch(0xD050_0108),
            t.bus.latch(0xD050_01FC)
        );
        // The text planes' registers (see video.rs).
        for (name, regs) in [("0x000", 0x00u32..0x40), ("0x040", 0x40..0x80), ("0x080", 0x80..0xC0), ("0x340", 0x340..0x360)] {
            let v: Vec<String> = regs.step_by(4).map(|r| format!("{:08X}", t.bus.latch(0xD050_0000 + r))).collect();
            println!("  regs {name}: {}", v.join(" "));
        }
        for (n, x0, y0, w, h, e) in t.bus.sprite_rects() {
            let hw: Vec<String> = e.iter().map(|v| format!("{v:04x}")).collect();
            let ext = t.bus.latch(0xD050_6000 + 4 * n);
            println!("  {n:3} at ({x0},{y0}) {w}x{h}  {}  ext {ext:08X}", hw.join(" "));
        }
    }
    // RIBBIT_SPRITES=png: the video chip's frame from its sprite list now
    // (240 x 320), and how many of its pixels are not black.
    if let Ok(path) = std::env::var("RIBBIT_SPRITES") {
        let mut frame = vec![0u16; ribbit_twist::video::FRAME_W * ribbit_twist::video::FRAME_H];
        t.bus.draw_frame(&mut frame);
        let lit = frame.iter().filter(|&&p| p != 0).count();
        println!("== sprites: {lit} pixels drawn -> {path}");
        let rgb: Vec<u32> = frame
            .iter()
            .map(|&p| {
                let p = p as u32;
                ((p >> 11) << 3) << 16 | (((p >> 5) & 63) << 2) << 8 | (p & 31) << 3
            })
            .collect();
        write_png_sized(&path, &rgb, ribbit_twist::video::FRAME_W, ribbit_twist::video::FRAME_H);
    }
    if !t.bus.gfx_log.is_empty() {
        println!("\n== 2D engine: {} jobs (first 6, then the last)", t.bus.gfx_log.len());
        let n = t.bus.gfx_log.len();
        for (i, (at, a, b, c, regs)) in t.bus.gfx_log.iter().enumerate() {
            if i < 6 || i + 1 == n {
                let r: Vec<String> = regs.iter().map(|(w, v)| format!("{:03X}={v:08X}", w & 0xFFF)).collect();
                println!("  t={at} C04={a:08X} C08={b:08X} C0C={c:08X}  {}", r.join(" "));
            }
        }
    }
    // DMA transfers, grouped by (source kind, destination kind).
    if !t.bus.dma_log.is_empty() {
        let kind = |a: u32| if a >= 0xC000_0000 { format!("{a:08X}") } else { "RAM".to_string() };
        let mut groups: std::collections::BTreeMap<(String, String), (u32, u64, u32)> = Default::default();
        for &(_, _, _, src, dst, bytes) in &t.bus.dma_log {
            let g = groups.entry((kind(src), kind(dst))).or_default();
            g.0 += 1;
            g.1 += bytes as u64;
            g.2 = g.2.max(bytes);
        }
        println!("\n== DMA: {} transfers", t.bus.dma_log.len());
        let mut ctrls: std::collections::BTreeMap<(u32, String, String), u32> = Default::default();
        for &(_, _, ctrl, src, dst, _) in &t.bus.dma_log {
            *ctrls.entry((ctrl & 0xFFFF, kind(src), kind(dst))).or_default() += 1;
        }
        for ((c, s, d), n) in ctrls {
            println!("  ctrl {c:04X}: {s} -> {d}, {n} times");
        }
        for &(at, ch, ctrl, src, dst, bytes) in t.bus.dma_log.iter().filter(|e| e.5 >= 0x1_0000) {
            println!("  large: t={at} ch{ch} ctrl {ctrl:08X} {src:08X} -> {dst:08X}, {bytes} bytes");
        }
        // RIBBIT_DMA_INTO=lo-hi (hex): each transfer writing into that range.
        if let Some((lo, hi)) = std::env::var("RIBBIT_DMA_INTO").ok().and_then(|v| {
            let (a, b) = v.split_once('-')?;
            Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(b, 16).ok()?))
        }) {
            for &(at, ch, ctrl, src, dst, bytes) in t.bus.dma_log.iter().filter(|e| e.4 < hi && e.4 + e.5 > lo).take(60) {
                println!("  into: t={at} ch{ch} ctrl {ctrl:08X} {src:08X} -> {dst:08X}, {bytes} bytes");
            }
        }
        for ((s, d), (n, total, max)) in groups {
            println!("  {s:>8} -> {d:<8}  {n:>6} transfers, {total:>10} bytes, largest {max}");
        }
    }
    // RIBBIT_READ_FILE=path[,path]: files on the emulated volume after the
    // run (as the firmware left them); RIBBIT_LIST_DIR=path: a folder.
    if let Some(card) = &t.bus.emmc {
        // RIBBIT_SECTORS=n,n (decimal): those sectors of the chip now.
        for n in std::env::var("RIBBIT_SECTORS").unwrap_or_default().split(',').filter_map(|v| v.parse::<u64>().ok()) {
            let s = card.sector(n);
            println!("== sector {n}:");
            for row in s.chunks(32) {
                println!("   {:02X?} {}", row, row.iter().map(|&b| if (32..127).contains(&b) { b as char } else { '.' }).collect::<String>());
            }
        }
        let read = std::env::var("RIBBIT_READ_FILE").ok();
        let list = std::env::var("RIBBIT_LIST_DIR").ok();
        if read.is_some() || list.is_some() {
            let disk = card.contents();
            let volume = &disk[ribbit_twist::disk::FAT_AREA * ribbit_twist::sd::SECTOR..];
            for path in read.iter().flat_map(|r| r.split(',')) {
                match ribbit_twist::fat::read_file(volume, path) {
                    Some(d) => println!("== file {path}: {} bytes {:02X?} {:?}", d.len(), &d[..d.len().min(64)], String::from_utf8_lossy(&d[..d.len().min(128)])),
                    None => println!("== file {path}: not found"),
                }
            }
            if let Some(dir) = list {
                match ribbit_twist::fat::list(volume, &dir) {
                    Some(es) => {
                        for e in es {
                            println!("== {dir}: {} {} bytes{}", e.name, e.size, if e.is_dir { " (folder)" } else { "" });
                        }
                    }
                    None => println!("== {dir}: not found"),
                }
            }
        }
    }
    if !t.breaks.is_empty() {
        let path = "twist-breaks.txt";
        let lines: Vec<String> = t
            .breaks
            .iter()
            .zip(&t.break_texts)
            .map(|((at, r), text)| {
                format!("{at} {} {text}", r.iter().map(|v| format!("{v:08X}")).collect::<Vec<_>>().join(" "))
            })
            .collect();
        std::fs::write(path, lines.join("\n")).expect("writing the breaks");
        println!("
== {} stops at the breakpoint written to {path}", t.breaks.len());
    }
    if !t.bus.ram_writes.is_empty() {
        println!("
== RAM writes watched (first {})", t.bus.ram_writes.len());
        let mut by_pc: std::collections::BTreeMap<u32, (u64, u64, u32)> = Default::default();
        for &(at, pc, a, _) in &t.bus.ram_writes {
            let e = by_pc.entry(pc).or_insert((0, at, a));
            e.0 += 1;
        }
        for (pc, (n, first, a)) in by_pc {
            println!("  pc {pc:08X}: {n} writes, first t={first} at {a:08X}");
        }
    }
    if !t.bus.watched.is_empty() {
        let path = "twist-watch.txt";
        std::fs::write(path, t.bus.watched.join("\n")).expect("writing the watch log");
        println!("\n== {} watched accesses written to {path}", t.bus.watched.len());
    }
    println!("\n== hottest addresses at the end");
    for (a, n) in t.hottest(10) {
        println!("  {a:08X}  {n}");
    }
    // By 256-byte block, share of all samples.
    let mut blocks: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    let total: u64 = t.hot.values().sum();
    for (a, n) in &t.hot {
        *blocks.entry(a & !0xFF).or_default() += n;
    }
    let mut v: Vec<_> = blocks.into_iter().collect();
    v.sort_by(|a, b| b.1.cmp(&a.1));
    println!(
        "
== hottest code blocks (256 bytes), share of samples"
    );
    for (a, n) in v.into_iter().take(15) {
        println!("  {a:08X}  {:.1}%", 100.0 * n as f64 / total.max(1) as f64);
    }
}

/// The code of a system package's firmware image (`.lfp`), or of an image
/// file.
fn package_code(path: &str) -> Vec<u8> {
    let data = std::fs::read(path).unwrap_or_else(|e| panic!("reading {path}: {e}"));
    let data = if ribbit_core::zip::is_zip(&data) {
        let meta = ribbit_core::zip::extract_named(&data, "Firmware/meta.inf").expect("no meta.inf");
        let meta = package::Meta::parse(&String::from_utf8_lossy(&meta));
        let bin = meta.get("BinFile").expect("no BinFile");
        ribbit_core::zip::extract_named(&data, &format!("Firmware/{bin}")).expect("BinFile missing")
    } else {
        data
    };
    image::code(&data).to_vec()
}

/// A 320 x 240 picture (0x00RRGGBB) as a PNG.
fn write_png(path: &str, pixels: &[u32]) {
    write_png_sized(path, pixels, ribbit_twist::SCREEN_W, ribbit_twist::SCREEN_H);
}

/// A picture of `w` x `h` (0x00RRGGBB) as a PNG.
fn write_png_sized(path: &str, pixels: &[u32], w: usize, h: usize) {
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("creating the PNG"));
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.write_header().and_then(|mut w| w.write_image_data(&rgb)).expect("writing the PNG");
}

/// The System's export table: (name, function) pairs, a name's address
/// next to its function's, as `machine::export` reads them.
fn exports(system: &[u8]) -> Vec<(String, u32)> {
    let word = |i: usize| system.get(i..i + 4).map(|w| u32::from_le_bytes(w.try_into().unwrap()));
    let name_at = |a: u32| -> Option<String> {
        let b = system.get(a as usize..)?;
        let end = b.iter().position(|&c| c == 0)?;
        let n = std::str::from_utf8(&b[..end]).ok()?;
        (n.len() >= 4 && n.contains('_') && n.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')).then(|| n.to_string())
    };
    let mut out = Vec::new();
    for i in (0..system.len().saturating_sub(8)).step_by(4) {
        if let (Some(a), Some(f)) = (word(i), word(i + 4))
            && (f as usize) < system.len()
            && f & 3 == 0
            && f != 0
            && let Some(n) = name_at(a)
        {
            out.push((n, f));
        }
    }
    out
}
