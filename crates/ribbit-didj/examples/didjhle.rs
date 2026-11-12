//! Runs one Didj program on the HLE (Enhanced (new)): Ribbit answers its
//! Linux system calls, no kernel. Prints what it writes to the console.
//!
//! didjhle <folder of .lfp files> <program> [args...]
//! e.g. didjhle didj /bin/busybox echo hello
//! DIDJ_HLE_TRACE=1 logs every system call; DIDJ_HLE_MILLIONS sets the
//! instruction budget (default 500 million); DIDJ_HLE_SHOT=<file.png> saves
//! the screen at the end, as the MLC composes it. DIDJ_HLE_KEYS presses keys:
//! `a@3000,down@3500` presses A after 3000 million instructions and Down
//! after 3500, each held for 50 million (names in hle::key::by_name;
//! `a@3000+400` holds it for 400 million, `right@3000+2000ms` for 2000 host
//! ms). DIDJ_HLE_KEYS_MS=1: those times (and DIDJ_HLE_FRAMES_FROM) are host
//! ms instead, for menus that wait; DIDJ_HLE_UNTIL_MS=<ms> stops then.
//! DIDJ_HLE_GA3D=<n> runs the 3D chip n times as fast; DIDJ_HLE_MLC_HZ=<n>
//! gives the MLC n frames a second (60 is real); DIDJ_HLE_TICK_US=<n> makes
//! the kernel tick n microseconds (10000 is real).
//! DIDJ_HLE_MIPS=<n>: as on a host that runs only n million instructions a
//! second (a busy PC).
//! DIDJ_HLE_CART=<dump> puts a cartridge in the slot; DIDJ_HLE_CARRY=<save>
//! puts the player's files from an Original save (.nandblocks) on /Didj.
//! DIDJ_HLE_WAV, DIDJ_HLE_LOG, DIDJ_HLE_FRAMES and DIDJ_HLE_PCS: sound,
//! frames and a PC profile for timing checks (see below).

use ribbit_didj::hle::Process;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: didjhle <folder of .lfp files> <program> [args...]");
        std::process::exit(2);
    }
    let t = std::time::Instant::now();
    let vfs = ribbit_didj::hle::system_from_packages(args[0].as_ref()).unwrap_or_else(|e| panic!("{e}"));
    eprintln!("[didjhle] file systems read in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let mut p = Process::new(vfs);
    p.trace = std::env::var_os("DIDJ_HLE_TRACE").is_some();
    // The boot splash for the boot loader's and kernel's time, as Original
    // (DIDJ_HLE_NO_SPLASH=1: the programs start at once).
    if std::env::var_os("DIDJ_HLE_NO_SPLASH").is_none() {
        match ribbit_didj::hle::boot_splash_from_packages(args[0].as_ref()) {
            Ok(rgb) => p.boot_splash(&rgb),
            Err(e) => eprintln!("[didjhle] no boot splash: {e}"),
        }
    }
    if let Ok(path) = std::env::var("DIDJ_HLE_CARRY") {
        let base = ribbit_didj::nand_from_packages(args[0].as_ref(), true).unwrap_or_else(|e| panic!("{e}"));
        let save = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let files = ribbit_didj::carry::user_files(&save, &base).unwrap_or_else(|e| panic!("{e}"));
        eprintln!("[didjhle] carried {} files", files.len());
        for (name, data) in files {
            eprintln!("[didjhle]   {name} ({} bytes)", data.len());
            p.vfs.add_file(&format!("/Didj/{}", name.trim_start_matches('/')), data);
        }
    }
    // DIDJ_HLE_HD=<n>: the 3D engine also draws at n times the resolution;
    // DIDJ_HLE_FRAMES and DIDJ_HLE_SHOT save that picture.
    if let Some(n) = std::env::var("DIDJ_HLE_HD").ok().and_then(|v| v.parse().ok()) {
        p.m.bus.ga3d.hd.set_scale(n);
    }
    if let Ok(path) = std::env::var("DIDJ_HLE_CART") {
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        p.insert_cart(&raw).unwrap_or_else(|e| panic!("{path}: {e}"));
    }
    let argv: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let envp = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/", "TERM=vt100"];
    p.exec(&args[1], &argv, &envp).unwrap_or_else(|e| panic!("{e}"));
    let millions: u64 = std::env::var("DIDJ_HLE_MILLIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
    // (at instructions, key, down, hold in host ms): a key held by host time
    // comes up from `ups`.
    let keys_ms = std::env::var_os("DIDJ_HLE_KEYS_MS").is_some();
    let until_ms: Option<u128> = std::env::var("DIDJ_HLE_UNTIL_MS").ok().and_then(|v| v.parse().ok());
    // In millions of instructions, or host ms (DIDJ_HLE_KEYS_MS).
    let unit = if keys_ms { 1 } else { 1_000_000 };
    let parse = |list: &str| {
        let mut keys: Vec<(u64, u16, bool, Option<u128>)> = Vec::new();
        for item in list.split(',').filter(|s| !s.is_empty()) {
            let (name, at) = item.split_once('@').expect("DIDJ_HLE_KEYS: name@millions");
            let code = ribbit_didj::hle::key::by_name(name).unwrap_or_else(|| panic!("no key {name}"));
            let (at, hold) = at.split_once('+').unwrap_or((at, "50"));
            let at: u64 = at.parse().expect("DIDJ_HLE_KEYS: millions");
            if let Some(ms) = hold.strip_suffix("ms") {
                keys.push((at * unit, code, true, Some(ms.parse().expect("DIDJ_HLE_KEYS: ms held"))));
                continue;
            }
            let hold: u64 = hold.parse().expect("DIDJ_HLE_KEYS: millions held");
            keys.push((at * unit, code, true, None));
            keys.push(((at + hold) * unit, code, false, None));
        }
        keys.sort_by_key(|k| k.0);
        keys
    };
    let mut keys = parse(&std::env::var("DIDJ_HLE_KEYS").unwrap_or_default());
    let mut ups: Vec<(u128, u16)> = Vec::new();
    // DIDJ_HLE_KEYS_AFTER=<png> and DIDJ_HLE_THEN=<keys>: those keys' times
    // count from when the screen first looks like that picture (its size,
    // mean difference under 3), as didjboot's DIDJ_KEYS_AFTER; several
    // separated by ";" follow each other.
    let afters: Vec<Vec<u32>> = std::env::var("DIDJ_HLE_KEYS_AFTER").map_or(Vec::new(), |v| v.split(';').map(read_png).collect());
    let mut after = afters.iter().enumerate().peekable();
    // DIDJ_HLE_KEYS_FLIPS=<n>: from DIDJ_HLE_THEN's n-th list on, times and
    // holds count frames shown (MLC flips), as didjboot's DIDJ_KEYS_FLIPS.
    let flips_from: Option<usize> = std::env::var("DIDJ_HLE_KEYS_FLIPS").ok().and_then(|v| v.parse().ok());
    let mut flip_keys: Vec<(u64, u16, bool, Option<u128>)> = Vec::new();
    let scripts: Vec<_> = std::env::var("DIDJ_HLE_THEN").unwrap_or_default().split(';').map(parse).collect();
    let t = std::time::Instant::now();
    // DIDJ_HLE_WAV=<file> saves the sound written to /dev/dsp; DIDJ_HLE_LOG=1
    // prints when each block of it arrived (host ms, instructions).
    let mut sound: Vec<i16> = Vec::new();
    let (mut rate, mut channels) = (32000, 2);
    let sound_log = std::env::var_os("DIDJ_HLE_LOG").is_some();
    // DIDJ_HLE_FRAMES=<folder> saves the screen whenever it changed, after
    // each step, named by host ms (DIDJ_HLE_FRAMES_FROM=<millions>: from
    // then on).
    let frames_dir = std::env::var("DIDJ_HLE_FRAMES").ok();
    let frames_from: u64 = std::env::var("DIDJ_HLE_FRAMES_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let fps = std::env::var_os("DIDJ_HLE_FPS").is_some();
    let (mut fps_second, mut fps_flips, mut fps_executed) = (0, 0, 0);
    let mut last_frame: Vec<u32> = Vec::new();
    // DIDJ_HLE_TWEEN=<n> (with DIDJ_HLE_FRAMES): at each frame shown, n - 1
    // in-between pictures and the frame, named <flip>_<k>.png (k = n: the
    // frame itself).
    let tween: Option<u32> = std::env::var("DIDJ_HLE_TWEEN").ok().and_then(|v| v.parse().ok());
    if tween.is_some() {
        p.m.bus.ga3d.set_tween(true);
    }
    let mut tween_flips = 0;
    // In steps of 10 million, printing the console as it comes.
    let pcs: Option<(u64, u64)> = std::env::var("DIDJ_HLE_PCS").ok().map(|v| {
        let (a, b) = v.split_once('-').expect("DIDJ_HLE_PCS=<from>-<to>");
        (a.parse().unwrap(), b.parse().unwrap())
    });
    let mut left = millions * 1_000_000;
    let slow: Option<f64> = std::env::var("DIDJ_HLE_MIPS").ok().and_then(|v| v.parse().ok()).filter(|&m: &f64| m > 0.0);
    let stop = loop {
        let now = if keys_ms { t.elapsed().as_millis() as u64 } else { p.executed };
        // DIDJ_HLE_TRACE_FROM=<ms or millions, as the keys>: the trace from then.
        if std::env::var("DIDJ_HLE_TRACE_FROM").ok().and_then(|v| v.parse::<u64>().ok()).is_some_and(|f| now >= f * unit) {
            p.trace = true;
        }
        while let Some(&(at, code, down, ms)) = keys.first()
            && at <= now
        {
            eprintln!("[didjhle] key {code} {} at {} ms", if down { "down" } else { "up" }, t.elapsed().as_millis());
            p.key(code, down);
            if let Some(ms) = ms {
                ups.push((t.elapsed().as_millis() + ms, code));
            }
            keys.remove(0);
        }
        while let Some(&(at, code, down, _)) = flip_keys.first()
            && at <= p.m.bus.flips
        {
            eprintln!("[didjhle] key {code} {} at flip {}", if down { "down" } else { "up" }, p.m.bus.flips);
            p.key(code, down);
            flip_keys.remove(0);
        }
        while let Some(i) = ups.iter().position(|&(at, _)| at <= t.elapsed().as_millis()) {
            let (_, code) = ups.remove(i);
            eprintln!("[didjhle] key {code} up at {} ms", t.elapsed().as_millis());
            p.key(code, false);
        }
        // DIDJ_HLE_PCS=<from>-<to>: steps of 1 million there, printing the
        // thread and PC after each (a cheap profile).
        let sampling = pcs.is_some_and(|(a, b)| (a..b).contains(&(p.executed / 1_000_000)));
        let tweening = tween.is_some() && frames_dir.is_some() && now >= frames_from * unit;
        if let Some(&(i, art)) = after.peek()
            && screen_diff(&p.m.bus.frame().2, art) < 3.0
        {
            eprintln!("[didjhle] screen {i} matched at {} ms, {} million instructions", t.elapsed().as_millis(), p.executed / 1_000_000);
            let (list, base, per) = if flips_from.is_some_and(|n| i >= n) { (&mut flip_keys, p.m.bus.flips, unit) } else { (&mut keys, now, 1) };
            list.extend(scripts.get(i).into_iter().flatten().map(|&(at, code, down, ms)| (at / per + base, code, down, ms)));
            list.sort_by_key(|k| k.0);
            after.next();
        }
        // Frames are looked at after each step: 10 million instructions take
        // 200 ms of a program that mostly sleeps.
        let recording = frames_dir.is_some() && now >= frames_from * unit;
        let step = left.min(if sampling || tweening || recording || after.peek().is_some() { 1_000_000 } else { 10_000_000 });
        let before = p.executed;
        // DIDJ_HLE_MIPS=<n>: a slower (or busy) host, running at most n
        // million instructions a second: a pause after each step.
        let (step_t, step_n) = (std::time::Instant::now(), p.executed);
        let stop = p.run(step);
        if let Some(mips) = slow {
            let least = std::time::Duration::from_secs_f64((p.executed - step_n) as f64 / (mips * 1e6));
            std::thread::sleep(least.saturating_sub(step_t.elapsed()));
        }
        if sampling {
            eprintln!("[pc] {} {:08X} {} ms", p.tid(), p.cpu.r[15], t.elapsed().as_millis());
        }
        // DIDJ_HLE_FPS=1: the frames shown in each second (as didjboot's
        // DIDJ_SPEED counts them).
        if fps && t.elapsed().as_secs() > fps_second {
            eprintln!("[fps] {fps_second} s: {} frames shown, {} million instructions", p.m.bus.flips - fps_flips, (p.executed - fps_executed) / 1_000_000);
            (fps_second, fps_flips, fps_executed) = (t.elapsed().as_secs(), p.m.bus.flips, p.executed);
        }
        if let (Some(n), Some(dir)) = (tween, &frames_dir)
            && now >= frames_from * unit
        {
            if p.m.bus.flips != tween_flips {
                tween_flips = p.m.bus.flips;
                let tw = p.m.bus.ga3d.tween().unwrap();
                eprintln!("[tween] flip {tween_flips} at {} ms: {} -> {} operations", t.elapsed().as_millis(), tw.older.len(), tw.newer.len());
                for k in 1..n {
                    if let Some((w, h, pixels)) = p.m.bus.frame_between(k as f32 / n as f32) {
                        save_png(&format!("{dir}/{tween_flips:06}_{k}.png"), w, h, &pixels);
                    }
                }
                let (w, h, pixels) = p.m.bus.frame_hd();
                save_png(&format!("{dir}/{tween_flips:06}_{n}.png"), w, h, &pixels);
            }
        } else if let Some(dir) = &frames_dir
            && now >= frames_from * unit
        {
            let (w, h, pixels) = p.m.bus.frame_hd();
            if pixels[..] != last_frame[..] {
                last_frame = pixels.to_vec();
                eprintln!("[mlc] {} ms (programs {:.1}, flip {} at {:.1}): {}", t.elapsed().as_millis(), p.program_time().as_secs_f64() * 1000.0, p.last_flip.0, p.last_flip.1.as_secs_f64() * 1000.0, [0xC000_4000u32, 0xC000_4024, 0xC000_4038, 0xC000_4058, 0xC000_406C, 0xC000_407C, 0xC000_408C].map(|a| format!("{:08X}", p.m.bus.reg(a))).join(" "));
                save_png(&format!("{dir}/{:06}.png", t.elapsed().as_millis()), w, h, &last_frame);
            }
        }
        let (s, r, c) = p.take_sound();
        if !s.is_empty() {
            if sound_log {
                eprintln!("[didjhle] sound {} frames at {:.0} ms (programs {:.1}), {}M", s.len() / c as usize, t.elapsed().as_secs_f64() * 1000.0, p.program_time().as_secs_f64() * 1000.0, p.executed / 1_000_000);
            }
            (rate, channels) = (r, c);
            sound.extend(s);
        }
        std::io::stdout().write_all(&std::mem::take(&mut p.console)).unwrap();
        std::io::stdout().flush().unwrap();
        // Nothing runs while the boot splash shows.
        left -= (p.executed - before).min(left);
        if stop != ribbit_didj::hle::Stop::Budget || left == 0 || until_ms.is_some_and(|u| t.elapsed().as_millis() >= u) {
            break stop;
        }
    };
    eprintln!("[didjhle] {stop:?} after {} instructions in {:.0} ms", p.executed, t.elapsed().as_secs_f64() * 1000.0);
    eprintln!("[didjhle] sound: {} frames ({:.1} s at {rate} Hz), {} not zero", sound.len() / channels as usize, sound.len() as f64 / channels as f64 / rate as f64, sound.iter().filter(|&&v| v != 0).count());
    if let Ok(path) = std::env::var("DIDJ_HLE_WAV") {
        let data: Vec<u8> = sound.iter().flat_map(|v| v.to_le_bytes()).collect();
        let mut f = b"RIFF".to_vec();
        f.extend((36 + data.len() as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend([1u16, channels as u16].iter().flat_map(|v| v.to_le_bytes()));
        f.extend([rate, rate * channels * 2].iter().flat_map(|v| v.to_le_bytes()));
        f.extend([channels as u16 * 2, 16].iter().flat_map(|v| v.to_le_bytes()));
        f.extend(b"data");
        f.extend((data.len() as u32).to_le_bytes());
        f.extend(&data);
        std::fs::write(&path, f).expect("can't write the WAV");
    }
    if let Ok(path) = std::env::var("DIDJ_HLE_SHOT") {
        let (w, h, pixels) = p.m.bus.frame_hd();
        save_png(&path, w, h, &pixels);
    }
}

/// A PNG as 0x00RRGGBB pixels.
fn read_png(path: &str) -> Vec<u32> {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).expect("can't open the PNG")));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut r = dec.read_info().expect("not a PNG");
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).expect("bad PNG");
    let n = info.color_type.samples();
    buf[..info.buffer_size()].chunks(n).map(|c| if n >= 3 { u32::from_be_bytes([0, c[0], c[1], c[2]]) } else { u32::from_be_bytes([0, c[0], c[0], c[0]]) }).collect()
}

/// The mean difference of two pictures' red, green and blue (0-255),
/// infinite when their sizes differ.
fn screen_diff(px: &[u32], art: &[u32]) -> f64 {
    if px.len() != art.len() {
        return f64::INFINITY;
    }
    let sum: u64 = px.iter().zip(art).map(|(a, b)| [16, 8, 0].iter().map(|s| ((a >> s) as u8).abs_diff((b >> s) as u8) as u64).sum::<u64>()).sum();
    sum as f64 / (px.len() * 3) as f64
}

fn save_png(path: &str, w: usize, h: usize, pixels: &[u32]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("can't create the PNG"));
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&rgb)).expect("can't write the PNG");
}
