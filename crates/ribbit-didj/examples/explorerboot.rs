//! Boots the Leapster Explorer from its firmware packages and prints the
//! UART output, the registers the software touched that have no model yet,
//! and where the CPU spent its time.
//!
//! Run: cargo run --release -p ribbit-didj --example explorerboot -- <folder of .lfp files> [million cycles]
//! EXPLORER_JIT=0 runs the interpreter alone.
//! EXPLORER_INPUT=<million>:<what>;... changes the input at those times:
//! `x,y` touches the screen there (pixels), `lift` lifts the stylus, a button
//! name (a b l r up down left right home hint pause bright volup voldown)
//! holds it, `none` lets go of the buttons. EXPLORER_SHOTS=<million>,... also
//! saves the screen at those times. EXPLORER_WAV=<file> saves the sound sent
//! to PCM out (16-bit stereo).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: explorerboot <folder of .lfp files> [million cycles]");
    let millions: u64 = args.get(1).map_or(200, |s| s.parse().expect("not a number"));

    let flash = ribbit_didj::explorer::nand_from_packages(dir.as_ref()).unwrap_or_else(|e| panic!("{e}"));
    // EXPLORER_BULK=<file>: save the Bulk partition as a UBI image and stop.
    if let Ok(path) = std::env::var("EXPLORER_BULK") {
        let at = ribbit_didj::explorer::PARTITIONS[4].1;
        std::fs::write(&path, flash.read(at, flash.size() - at)).expect("can't write the Bulk image");
        return;
    }
    let mut lx = ribbit_didj::explorer::boot(flash, ribbit_didj::explorer::CMDLINE).unwrap_or_else(|e| panic!("{e}"));
    lx.jit.enabled &= std::env::var("EXPLORER_JIT").as_deref() != Ok("0");

    // EXPLORER_TRACE=<n>: interpret until the first exception vector, then
    // print the last n instructions' addresses and the registers.
    if let Some(n) = std::env::var("EXPLORER_TRACE").ok().and_then(|v| v.parse::<usize>().ok()) {
        let mut last = std::collections::VecDeque::new();
        for _ in 0..millions * 1_000_000 {
            let pc = lx.cpu.r[15];
            if pc & 0xFFFF_FF00 == 0xFFFF_0000 || pc < 0x20 {
                break;
            }
            last.push_back((pc, lx.cpu.cpsr));
            if last.len() > n {
                last.pop_front();
            }
            lx.cpu.step(&mut lx.bus);
        }
        for (pc, cpsr) in &last {
            println!("{pc:#010x} {cpsr:#x}");
        }
        println!("now pc {:#010x} cpsr {:#x} r {:08x?}", lx.cpu.r[15], lx.cpu.cpsr, lx.cpu.r);
        let c = &lx.cpu.cp15;
        let ttb = c.ttb & !0x3FFF;
        let entry = |va: u32| {
            let at = (ttb + (va >> 20) * 4).wrapping_sub(lx.bus.ram_base) as usize;
            lx.bus.sdram.get(at..at + 4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))
        };
        println!("control {:#x} ttb {:#x} dacr {:#x} fsr {:#x} far {:#x}; L1 entry for far {:#x}, for 0xfc000000 {:#x}", c.control, c.ttb, c.dacr, c.fsr, c.far, entry(c.far), entry(0xFC00_0000));
        println!("{}", lx.bus.uart.first().map_or("", |s| s.as_str()));
        println!("touched: {:x?}", lx.bus.touched);
        println!("recent: {:x?}", lx.bus.recent);
        return;
    }

    let mut input: Vec<(u64, String)> = std::env::var("EXPLORER_INPUT").unwrap_or_default().split(';').filter(|e| !e.is_empty()).map(|e| {
        let (at, what) = e.split_once(':').expect("EXPLORER_INPUT: <million>:<what>");
        (at.parse().expect("EXPLORER_INPUT: not a number"), what.to_string())
    }).collect();
    input.sort_by_key(|e| e.0);
    // EXPLORER_SHOTS=<million>,...: also save the screen at those times, as
    // <EXPLORER_SHOT without .png>-<million>.png.
    let shot_path = std::env::var("EXPLORER_SHOT").unwrap_or_else(|_| "shot.png".into());
    let shots_at: Vec<u64> = std::env::var("EXPLORER_SHOTS").unwrap_or_default().split(',').filter_map(|v| v.parse().ok()).collect();
    let mut wav: Vec<u8> = Vec::new();
    let started = std::time::Instant::now();
    let mut samples = std::collections::HashMap::<u32, u32>::new();
    let mut printed = 0;
    for m in 0..millions {
        for (_, what) in input.iter().filter(|e| e.0 == m) {
            use ribbit_didj::machine::button::*;
            let held = match what.as_str() {
                "a" => A, "b" => B, "l" => L, "r" => R, "up" => UP, "down" => DOWN, "left" => LEFT, "right" => RIGHT,
                "home" => MENU, "hint" => HINT, "pause" => PAUSE, "bright" => BRIGHTNESS, "volup" => VOLUME_UP, "voldown" => VOLUME_DOWN,
                "none" => 0,
                "lift" => { lx.bus.touch = None; continue; }
                xy => {
                    let (x, y) = xy.split_once(',').expect("EXPLORER_INPUT: x,y");
                    lx.bus.touch = Some((x.parse().expect("x"), y.parse().expect("y")));
                    continue;
                }
            };
            lx.bus.buttons = held;
            println!("
[{m}M: {what}]");
        }
        lx.run(1_000_000);
        wav.extend(lx.bus.dma.pcm.drain(..));
        if shots_at.contains(&(m + 1)) {
            shot(&lx, &format!("{}-{}.png", shot_path.trim_end_matches(".png"), m + 1));
        }
        *samples.entry(lx.cpu.r[15] & !0xFFF).or_default() += 1;
        // UART0 as it comes, so a hang still shows how far it got.
        if let Some(out) = lx.bus.uart.first()
            && out.len() > printed
        {
            print!("{}", &out[printed..]);
            printed = out.len();
        }
        if lx.bus.powered_off {
            println!("\n(powered off)");
            break;
        }
    }
    println!("\n--- {:.1} s, {} instructions, pc {:#010x}, cpsr {:#x} ---", started.elapsed().as_secs_f64(), lx.cpu.instructions, lx.cpu.r[15], lx.cpu.cpsr);
    for (i, s) in lx.bus.uart.iter().enumerate().skip(1) {
        if !s.is_empty() {
            println!("--- UART{i} ---\n{s}");
        }
    }
    println!("--- unmodelled registers (address: first pc, reads, writes) ---");
    for (a, (pc, r, w)) in &lx.bus.touched {
        println!("{a:#010x}: {pc:#010x} r{r} w{w}");
    }
    println!("--- last unmodelled accesses (address pc r/w) ---");
    for (a, pc, w) in &lx.bus.recent {
        println!("{a:#010x} {pc:#010x} {}", if *w { "w" } else { "r" });
    }
    println!("PCM out: {} bytes, {} not zero", wav.len(), wav.iter().filter(|&&b| b != 0).count());
    if let Ok(path) = std::env::var("EXPLORER_WAV") {
        let rate = lx.bus.dma.pcm_rate as u32;
        let mut f = b"RIFF".to_vec();
        f.extend((36 + wav.len() as u32).to_le_bytes());
        f.extend(b"WAVEfmt ");
        f.extend(16u32.to_le_bytes());
        f.extend([1u16, 2].iter().flat_map(|v| v.to_le_bytes()));
        f.extend([rate, rate * 4].iter().flat_map(|v| v.to_le_bytes()));
        f.extend([4u16, 16].iter().flat_map(|v| v.to_le_bytes()));
        f.extend(b"data");
        f.extend((wav.len() as u32).to_le_bytes());
        f.extend(&wav);
        std::fs::write(&path, f).expect("can't write the WAV");
    }
    // EXPLORER_SHOT=<file.png>: save the screen as the MLC composes it.
    if let Ok(path) = std::env::var("EXPLORER_SHOT") {
        shot(&lx, &path);
        println!("--- screen saved ---");
    }
    // EXPLORER_RAM=<file>: save SDRAM (e.g. to read the kernel's log buffer).
    if let Ok(path) = std::env::var("EXPLORER_RAM") {
        std::fs::write(&path, &lx.bus.sdram).expect("can't write the RAM dump");
    }
    let mut hot: Vec<_> = samples.into_iter().collect();
    hot.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let hot: Vec<String> = hot.iter().take(12).map(|(p, n)| format!("{p:08x}:{n}")).collect();
    println!("--- busiest pages (page:samples) ---\n{}", hot.join(" "));
}

/// Saves the screen as the MLC composes it.
fn shot(lx: &ribbit_didj::machine::Didj, path: &str) {
    let (w, h, pixels) = lx.bus.frame();
    let file = std::io::BufWriter::new(std::fs::File::create(path).expect("can't create the PNG"));
    let mut enc = png::Encoder::new(file, w as u32, h as u32);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let rgb: Vec<u8> = pixels.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]).collect();
    enc.write_header().and_then(|mut w| w.write_image_data(&rgb)).expect("can't write the PNG");
}
