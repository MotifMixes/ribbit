//! Runs the Leapster Explorer's programs on the HLE (Enhanced (new)):
//! Ribbit answers their Linux system calls, no kernel. Prints what they
//! write to the console. Plan: docs/explorer-enhanced-hle.md.
//!
//! explorerhle <folder of .lfp/.lf2 files> [program args...]
//! With no program it starts the Explorer as its kernel would (rcS).
//! EXPLORER_HLE_TRACE=1 logs every system call; EXPLORER_HLE_MS=<n> stops
//! after n ms of host time (default 20000); EXPLORER_HLE_SHOT=<file.png>
//! saves the screen at the end, EXPLORER_HLE_SHOTS=<ms>,... also at those
//! times (<shot>-<ms>.png). EXPLORER_HLE_INPUT=<ms>:<what>;... as
//! explorerboot's EXPLORER_INPUT, in host ms: `x,y` touches there, `lift`,
//! a key name (hle::key::by_name) pressed for 100 ms. EXPLORER_HLE_WAV=<file>
//! saves the sound; EXPLORER_HLE_FRAMES=<folder> the screen whenever it
//! changed (named by host ms); EXPLORER_HLE_FPS=1 prints frames a second.

use ribbit_didj::hle::Process;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(dir) = args.first() else {
        eprintln!("usage: explorerhle <folder of .lfp files> [program args...]");
        std::process::exit(2);
    };
    let t = std::time::Instant::now();
    // EXPLORER_HLE_SAVE=<file.nandblocks>: the file systems of the app's
    // save (Original's NAND with it applied; its .nandimage tells the 1 GB
    // NAND of older saves; its .nor beside).
    let vfs = if let Ok(path) = std::env::var("EXPLORER_HLE_SAVE") {
        let path = std::path::PathBuf::from(path);
        let mut flash = ribbit_didj::explorer::nand_from_packages(dir.as_ref()).unwrap_or_else(|e| panic!("{e}"));
        let image = std::fs::read_to_string(path.with_extension("nandimage")).unwrap_or_default();
        if image != format!("{:08x}", flash.fingerprint()) {
            flash = ribbit_didj::explorer::nand_from_packages_blocks(dir.as_ref(), ribbit_didj::explorer::NAND_BLOCKS_1GB).unwrap_or_else(|e| panic!("{e}"));
        }
        flash.apply_changes(&std::fs::read(&path).expect("can't read the save")).unwrap_or_else(|e| panic!("{e}"));
        // EXPLORER_HLE_LIST=1: list the save's root file system and Bulk
        // (path, mode, size, CRC) and stop.
        if std::env::var_os("EXPLORER_HLE_LIST").is_some() {
            let (root, bulk) = ribbit_didj::hle::explorer::files_in_nand(&flash).unwrap_or_else(|e| panic!("{e}"));
            for (vol, files) in [("root", root), ("bulk", bulk)] {
                for f in files {
                    println!("{vol} {} {:o} {} {:08x}", f.path, f.mode, f.data.len(), crc32fast::hash(&f.data));
                }
            }
            return;
        }
        let nor = std::fs::read(path.with_extension("nor")).ok();
        ribbit_didj::hle::explorer::system_from_nand(&flash, nor.as_deref()).unwrap_or_else(|e| panic!("{e}"))
    } else {
        ribbit_didj::hle::explorer::system_from_packages(dir.as_ref()).unwrap_or_else(|e| panic!("{e}"))
    };
    eprintln!("[explorerhle] file systems read in {:.0} ms", t.elapsed().as_secs_f64() * 1000.0);
    let mut p = Process::new_explorer(vfs);
    p.trace = std::env::var_os("EXPLORER_HLE_TRACE").is_some();
    if let Some(rle) = ribbit_didj::hle::explorer::splash_from_packages(dir.as_ref()) {
        p.explorer_splash(&rle);
    }
    // EXPLORER_HLE_CART=<dump>: a cartridge in the slot from power-on
    // (unless EXPLORER_HLE_INPUT puts it in later with `cart`).
    if let Ok(path) = std::env::var("EXPLORER_HLE_CART")
        && !std::env::var("EXPLORER_HLE_INPUT").unwrap_or_default().contains(":cart")
    {
        p.explorer_cart(Some(&std::fs::read(&path).expect("the cartridge dump"))).unwrap_or_else(|e| panic!("{e}"));
    }
    // EXPLORER_HLE_CONSOLE_OUT=1: /flags/console_out, so /usr/bin/app lets
    // AppManager and CartManager print to the console.
    if std::env::var_os("EXPLORER_HLE_CONSOLE_OUT").is_some() {
        p.vfs.add_file("/flags/console_out", Vec::new());
    }
    let argv: Vec<&str> = if args.len() > 1 { args[1..].iter().map(String::as_str).collect() } else { vec!["/sbin/init"] };
    let envp = ["PATH=/usr/bin:/bin:/usr/sbin:/sbin", "HOME=/", "TERM=vt100"];
    p.exec(argv[0], &argv, &envp).unwrap_or_else(|e| panic!("{e}"));
    let until_ms: u128 = std::env::var("EXPLORER_HLE_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(20000);
    let shot_path = std::env::var("EXPLORER_HLE_SHOT").unwrap_or_else(|_| "shot.png".into());
    let mut shots: Vec<u128> = std::env::var("EXPLORER_HLE_SHOTS").unwrap_or_default().split(',').filter_map(|v| v.parse().ok()).collect();
    shots.sort();
    let mut input: Vec<(u128, String)> = std::env::var("EXPLORER_HLE_INPUT")
        .unwrap_or_default()
        .split(';')
        .filter(|e| !e.is_empty())
        .map(|e| {
            let (at, what) = e.split_once(':').expect("EXPLORER_HLE_INPUT: <ms>:<what>");
            (at.parse().expect("EXPLORER_HLE_INPUT: not a number"), what.to_string())
        })
        .collect();
    input.sort_by_key(|e| e.0);
    let mut ups: Vec<(u128, u16)> = Vec::new();
    let mut sound: Vec<i16> = Vec::new();
    let (mut rate, mut channels) = (32000, 2);
    let frames_dir = std::env::var("EXPLORER_HLE_FRAMES").ok();
    // EXPLORER_HLE_FRAMES_FROM=<ms>: record from then on only.
    let frames_from: u128 = std::env::var("EXPLORER_HLE_FRAMES_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut last_frame: Vec<u32> = Vec::new();
    let fps = std::env::var_os("EXPLORER_HLE_FPS").is_some();
    let (mut fps_second, mut fps_flips, mut fps_executed) = (0, 0, 0);
    let stop = loop {
        let ms = t.elapsed().as_millis();
        // EXPLORER_HLE_TRACE_FROM=<ms>: every system call from then on.
        if std::env::var("EXPLORER_HLE_TRACE_FROM").ok().and_then(|v| v.parse::<u128>().ok()).is_some_and(|f| ms >= f) {
            p.trace = true;
        }
        while let Some((at, what)) = input.first().cloned()
            && at <= ms
        {
            input.remove(0);
            eprintln!("[explorerhle] {what} at {ms} ms");
            if what == "cart" || what == "nocart" {
                // EXPLORER_HLE_INPUT's `cart` puts EXPLORER_HLE_CART's dump in, `nocart` takes it out.
                let raw = (what == "cart").then(|| std::fs::read(std::env::var("EXPLORER_HLE_CART").expect("EXPLORER_HLE_CART")).expect("the cartridge dump"));
                p.explorer_cart(raw.as_deref()).unwrap_or_else(|e| panic!("{e}"));
            } else if what == "lift" {
                p.stylus(None);
            } else if let Some((x, y)) = what.split_once(',') {
                p.stylus(Some((x.parse().expect("x"), y.parse().expect("y"))));
            } else {
                // A key name, or k<Linux key code> (k115 volume up, k114
                // down); `+<ms>` holds it that long (else 100 ms).
                let (name, hold) = what.split_once('+').map_or((what.as_str(), 100), |(n, h)| (n, h.parse().expect("ms held")));
                let code = name.strip_prefix('k').and_then(|c| c.parse().ok()).or_else(|| ribbit_didj::hle::key::by_name(name)).unwrap_or_else(|| panic!("no key {name}"));
                p.key(code, true);
                ups.push((ms + hold, code));
            }
        }
        while let Some(i) = ups.iter().position(|&(at, _)| at <= ms) {
            p.key(ups.remove(i).1, false);
        }
        let recording = frames_dir.is_some() && t.elapsed().as_millis() >= frames_from;
        let step = if recording { 1_000_000 } else { 5_000_000 };
        let before = std::time::Instant::now();
        let stop = p.run(step);
        // EXPLORER_HLE_SLOW=<ms>: steps that took longer (the HLE's own lag).
        if let Some(slow) = std::env::var("EXPLORER_HLE_SLOW").ok().and_then(|v| v.parse::<u128>().ok())
            && before.elapsed().as_millis() >= slow
        {
            eprintln!("[slow] step of {} ms at {} ms, tid {} pc {:08X}", before.elapsed().as_millis(), t.elapsed().as_millis(), p.tid(), p.cpu.r[15]);
        }
        if fps && t.elapsed().as_secs() > fps_second {
            eprintln!("[fps] {fps_second} s: {} frames shown, {} million instructions", p.explorer_pans() - fps_flips, (p.executed - fps_executed) / 1_000_000);
            (fps_second, fps_flips, fps_executed) = (t.elapsed().as_secs(), p.explorer_pans(), p.executed);
        }
        if let Some(dir) = frames_dir.as_ref().filter(|_| recording) {
            let (w, h, pixels) = p.m.bus.frame_hd();
            if pixels[..] != last_frame[..] {
                last_frame = pixels.to_vec();
                save_png(&format!("{dir}/{:06}.png", t.elapsed().as_millis()), w, h, &last_frame);
            }
        }
        while let Some(&at) = shots.first()
            && at <= t.elapsed().as_millis()
        {
            shots.remove(0);
            let (w, h, pixels) = p.m.bus.frame_hd();
            save_png(&format!("{}-{at}.png", shot_path.trim_end_matches(".png")), w, h, &pixels);
        }
        let (s, r, c) = p.take_sound();
        if !s.is_empty() {
            (rate, channels) = (r, c);
            sound.extend(s);
        }
        std::io::stdout().write_all(&std::mem::take(&mut p.console)).unwrap();
        std::io::stdout().flush().unwrap();
        if stop != ribbit_didj::hle::Stop::Budget || t.elapsed().as_millis() >= until_ms {
            break stop;
        }
    };
    eprintln!("[explorerhle] {stop:?} after {} instructions in {:.0} ms", p.executed, t.elapsed().as_secs_f64() * 1000.0);
    eprintln!("[explorerhle] sound: {:.1} s at {rate} Hz, {} not zero", sound.len() as f64 / channels as f64 / rate as f64, sound.iter().filter(|&&v| v != 0).count());
    if let Ok(path) = std::env::var("EXPLORER_HLE_WAV") {
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
    // EXPLORER_HLE_CAT=<path>,...: files' contents at the end (folders
    // listed).
    for path in std::env::var("EXPLORER_HLE_CAT").unwrap_or_default().split(',').filter(|p| !p.is_empty()) {
        match p.vfs.get(path).map(|n| (n.mode, &n.data)) {
            Some((mode, ribbit_didj::hle::vfs::Data::File(d))) => eprintln!("[cat] {path} ({mode:o}, {} bytes): {:?}", d.len(), String::from_utf8_lossy(d)),
            Some((_, ribbit_didj::hle::vfs::Data::Dir)) => eprintln!("[cat] {path}/: {:?}", p.vfs.list(path).iter().map(|(n, _)| n.clone()).collect::<Vec<_>>()),
            other => eprintln!("[cat] {path}: {:?}", other.map(|o| o.0)),
        }
    }
    // EXPLORER_HLE_SAVE_OUT=<file.nandblocks>: the file systems written
    // into the NAND (that of EXPLORER_HLE_SAVE, else the packages') as the
    // app's save, with its .nandimage and .nor.
    if let Ok(out) = std::env::var("EXPLORER_HLE_SAVE_OUT") {
        let base = ribbit_didj::explorer::nand_from_packages(dir.as_ref()).unwrap_or_else(|e| panic!("{e}"));
        let mut flash = base.clone();
        if let Ok(path) = std::env::var("EXPLORER_HLE_SAVE") {
            flash.apply_changes(&std::fs::read(&path).expect("can't read the save")).unwrap_or_else(|e| panic!("{e}"));
        }
        let t = std::time::Instant::now();
        ribbit_didj::hle::explorer::write_to_nand(&p.vfs, &mut flash).unwrap_or_else(|e| panic!("{e}"));
        let changes = flash.changes_from(&base);
        let parts = ribbit_didj::explorer::PARTITIONS;
        let mut counts = [0usize; 5];
        for b in 0..flash.size() / ribbit_didj::nand::BLOCK {
            let at = b * ribbit_didj::nand::BLOCK;
            if flash.read(at, ribbit_didj::nand::BLOCK) != base.read(at, ribbit_didj::nand::BLOCK) {
                counts[parts.iter().rposition(|p| p.1 <= at).unwrap()] += 1;
            }
        }
        eprintln!("[explorerhle] blocks changed from the packages' image by partition: {counts:?}");
        eprintln!("[explorerhle] save written in {} ms: {} bytes", t.elapsed().as_millis(), changes.len());
        std::fs::write(&out, changes).expect("can't write the save");
        let out = std::path::Path::new(&out);
        std::fs::write(out.with_extension("nandimage"), format!("{:08x}", base.fingerprint())).unwrap();
        std::fs::write(out.with_extension("nor"), ribbit_didj::hle::explorer::nor(&p.vfs)).unwrap();
    }
    eprintln!("[explorerhle] user pages free: {} MB of {} MB", p.free_memory() >> 20, ribbit_didj::hle::explorer::SDRAM >> 20);
    eprintln!("[explorerhle] codec volume registers 0x20-0x25: {:02x?}", &p.m.bus.i2c.codec()[0x20..0x26]);
    if std::env::var_os("EXPLORER_HLE_SHOT").is_some() {
        let (w, h, pixels) = p.m.bus.frame_hd();
        save_png(&shot_path, w, h, &pixels);
    }
}

fn save_png(path: &str, w: usize, h: usize, pixels: &[u32]) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("can't create the PNG"));
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_compression(png::Compression::Fast);
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&rgb)).expect("can't write the PNG");
}
