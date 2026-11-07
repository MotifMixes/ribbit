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
//! `a@3000+400` holds it for 400 million).
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
    if let Ok(path) = std::env::var("DIDJ_HLE_CART") {
        let raw = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        p.insert_cart(&raw).unwrap_or_else(|e| panic!("{path}: {e}"));
    }
    let argv: Vec<&str> = args[1..].iter().map(String::as_str).collect();
    let envp = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/", "TERM=vt100"];
    p.exec(&args[1], &argv, &envp).unwrap_or_else(|e| panic!("{e}"));
    let millions: u64 = std::env::var("DIDJ_HLE_MILLIONS").ok().and_then(|v| v.parse().ok()).unwrap_or(500);
    let mut keys: Vec<(u64, u16, bool)> = Vec::new();
    for item in std::env::var("DIDJ_HLE_KEYS").unwrap_or_default().split(',').filter(|s| !s.is_empty()) {
        let (name, at) = item.split_once('@').expect("DIDJ_HLE_KEYS: name@millions");
        let code = ribbit_didj::hle::key::by_name(name).unwrap_or_else(|| panic!("no key {name}"));
        let (at, hold) = at.split_once('+').unwrap_or((at, "50"));
        let at: u64 = at.parse().expect("DIDJ_HLE_KEYS: millions");
        let hold: u64 = hold.parse().expect("DIDJ_HLE_KEYS: millions held");
        keys.push((at * 1_000_000, code, true));
        keys.push(((at + hold) * 1_000_000, code, false));
    }
    keys.sort_by_key(|k| k.0);
    let mut keys = keys.into_iter().peekable();
    let t = std::time::Instant::now();
    // DIDJ_HLE_WAV=<file> saves the sound written to /dev/dsp; DIDJ_HLE_LOG=1
    // prints when each block of it arrived (host ms, instructions).
    let mut sound: Vec<i16> = Vec::new();
    let (mut rate, mut channels) = (32000, 2);
    let sound_log = std::env::var_os("DIDJ_HLE_LOG").is_some();
    // DIDJ_HLE_FRAMES=<folder> saves the screen whenever it changed, after
    // each step, named by host ms.
    let frames_dir = std::env::var("DIDJ_HLE_FRAMES").ok();
    let mut last_frame: Vec<u32> = Vec::new();
    // In steps of 10 million, printing the console as it comes.
    let pcs: Option<(u64, u64)> = std::env::var("DIDJ_HLE_PCS").ok().map(|v| {
        let (a, b) = v.split_once('-').expect("DIDJ_HLE_PCS=<from>-<to>");
        (a.parse().unwrap(), b.parse().unwrap())
    });
    let mut left = millions * 1_000_000;
    let stop = loop {
        while let Some(&(at, code, down)) = keys.peek()
            && at <= p.executed
        {
            eprintln!("[didjhle] key {code} {}", if down { "down" } else { "up" });
            p.key(code, down);
            keys.next();
        }
        // DIDJ_HLE_PCS=<from>-<to>: steps of 1 million there, printing the
        // thread and PC after each (a cheap profile).
        let sampling = pcs.is_some_and(|(a, b)| (a..b).contains(&(p.executed / 1_000_000)));
        let step = left.min(if sampling { 1_000_000 } else { 10_000_000 });
        let stop = p.run(step);
        if sampling {
            eprintln!("[pc] {} {:08X}", p.tid(), p.cpu.r[15]);
        }
        if let Some(dir) = &frames_dir {
            let (w, h, pixels) = p.m.bus.frame();
            if pixels[..] != last_frame[..] {
                last_frame = pixels.to_vec();
                eprintln!("[mlc] {} ms: {}", t.elapsed().as_millis(), [0xC000_4000u32, 0xC000_4024, 0xC000_4038, 0xC000_4058, 0xC000_406C, 0xC000_407C, 0xC000_408C].map(|a| format!("{:08X}", p.m.bus.reg(a))).join(" "));
                save_png(&format!("{dir}/{:06}.png", t.elapsed().as_millis()), w, h, &last_frame);
            }
        }
        let (s, r, c) = p.take_sound();
        if !s.is_empty() {
            if sound_log {
                eprintln!("[didjhle] sound {} frames at {:.0} ms, {}M", s.len() / c as usize, t.elapsed().as_secs_f64() * 1000.0, p.executed / 1_000_000);
            }
            (rate, channels) = (r, c);
            sound.extend(s);
        }
        std::io::stdout().write_all(&std::mem::take(&mut p.console)).unwrap();
        std::io::stdout().flush().unwrap();
        left -= step;
        if stop != ribbit_didj::hle::Stop::Budget || left == 0 {
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
        let (w, h, pixels) = p.m.bus.frame();
        save_png(&path, w, h, &pixels);
    }
}

fn save_png(path: &str, w: usize, h: usize, pixels: &[u32]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("can't create the PNG"));
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&rgb)).expect("can't write the PNG");
}
