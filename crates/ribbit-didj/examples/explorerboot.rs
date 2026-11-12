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
//! to PCM out (16-bit stereo). EXPLORER_CART=<file> puts a cartridge in the
//! slot; the inputs `insert` and `remove` put it in and take it out later.
//! EXPLORER_STATE_OUT=<file> saves the machine at the end, EXPLORER_STATE=<file>
//! starts from such a state (same firmware folder).

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: explorerboot <folder of .lfp files> [million cycles]");
    let millions: u64 = args.get(1).map_or(200, |s| s.parse().expect("not a number"));

    let mut flash = ribbit_didj::explorer::nand_from_packages(dir.as_ref()).unwrap_or_else(|e| panic!("{e}"));
    // EXPLORER_SAVE=<file.nandblocks>: start from the app's save of this image.
    if let Ok(path) = std::env::var("EXPLORER_SAVE") {
        flash.apply_changes(&std::fs::read(&path).expect("can't read the save")).unwrap_or_else(|e| panic!("{e}"));
    }
    // EXPLORER_BULK=<file>: save the Bulk partition as a UBI image and stop.
    if let Ok(path) = std::env::var("EXPLORER_BULK") {
        let at = ribbit_didj::explorer::PARTITIONS[4].1;
        std::fs::write(&path, flash.read(at, flash.size() - at)).expect("can't write the Bulk image");
        return;
    }
    let base = flash.clone();
    let mut lx = ribbit_didj::explorer::boot(flash, ribbit_didj::explorer::CMDLINE).unwrap_or_else(|e| panic!("{e}"));
    // EXPLORER_STATE=<file>: go on from a state EXPLORER_STATE_OUT saved.
    if let Ok(path) = std::env::var("EXPLORER_STATE") {
        lx.load_state(&std::fs::read(&path).expect("can't read the state"), &base).unwrap_or_else(|e| panic!("{e}"));
        // EXPLORER_COLD=1: power on afresh with the state's NAND.
        if std::env::var("EXPLORER_COLD").is_ok() {
            let flash = std::mem::take(&mut lx.bus.nand);
            lx = ribbit_didj::explorer::boot(flash, ribbit_didj::explorer::CMDLINE).unwrap_or_else(|e| panic!("{e}"));
        }
    }
    lx.jit.enabled &= std::env::var("EXPLORER_JIT").as_deref() != Ok("0");
    // EXPLORER_CART=<file>: a cartridge (a raw dump or its FAT volume), in
    // the slot from the start unless EXPLORER_INPUT inserts it later.
    let mut cart = None;
    if let Ok(path) = std::env::var("EXPLORER_CART") {
        let mut data = std::fs::read(&path).expect("can't read the cartridge");
        if ribbit_didj::cart::is_volume(&data) {
            data = ribbit_didj::cart::from_volume(&data).unwrap_or_else(|e| panic!("{e}"));
        }
        cart = Some(ribbit_didj::cart::Cart::new(data).unwrap_or_else(|e| panic!("{e}")));
    }

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
    if !input.iter().any(|e| e.1 == "insert") {
        lx.bus.cart = cart.take();
    }
    // EXPLORER_SHOTS=<million>,...: also save the screen at those times, as
    // <EXPLORER_SHOT without .png>-<million>.png.
    let shot_path = std::env::var("EXPLORER_SHOT").unwrap_or_else(|_| "shot.png".into());
    let shots_at: Vec<u64> = std::env::var("EXPLORER_SHOTS").unwrap_or_default().split(',').filter_map(|v| v.parse().ok()).collect();
    let mut wav: Vec<u8> = Vec::new();
    let started = std::time::Instant::now();
    let mut samples = std::collections::HashMap::<u32, u32>::new();
    let exact_pcs = std::env::var("EXPLORER_PCS").is_ok();
    let mut printed = 0;
    // EXPLORER_TYPE=<commands separated by ";;">: shell commands typed one
    // per "# " prompt on UART0 (after 50 million cycles without output).
    let typed = std::env::var("EXPLORER_TYPE").unwrap_or_default();
    let mut commands = typed.split(";;").filter(|c| !c.is_empty()).map(|c| format!("{c}\n")).collect::<Vec<_>>().into_iter();
    let (mut prompts, mut quiet) = (0, 0);
    let mut last_frame: Vec<u32> = Vec::new();
    for m in 0..millions {
        for (_, what) in input.iter().filter(|e| e.0 == m) {
            use ribbit_didj::machine::button::*;
            let held = match what.as_str() {
                "a" => A, "b" => B, "l" => L, "r" => R, "up" => UP, "down" => DOWN, "left" => LEFT, "right" => RIGHT,
                "home" => MENU, "hint" => HINT, "pause" => PAUSE, "bright" => BRIGHTNESS, "volup" => VOLUME_UP, "voldown" => VOLUME_DOWN,
                "none" => 0,
                "lift" => { lx.bus.touch = None; continue; }
                "insert" => { lx.bus.cart = cart.take(); println!("\n[{m}M: cartridge in]"); continue; }
                "remove" => { cart = lx.bus.cart.take(); println!("\n[{m}M: cartridge out]"); continue; }
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
        // EXPLORER_PCS=1: by instruction instead of by page.
        *samples.entry(lx.cpu.r[15] & if exact_pcs { !0 } else { !0xFFF }).or_default() += 1;
        // UART0 as it comes, so a hang still shows how far it got.
        quiet += 1;
        if let Some(out) = lx.bus.uart.first()
            && out.len() > printed
        {
            print!("{}", &out[printed..]);
            printed = out.len();
            quiet = 0;
        }
        let seen = lx.bus.uart.first().map_or(0, |o| o.matches("# ").count());
        if seen > prompts && quiet >= 50 {
            prompts = seen;
            if let Some(c) = commands.next() {
                lx.bus.uart_rx.extend(c.bytes());
            }
        }
        // EXPLORER_FRAMES=<folder>: the screen whenever it changed (looked at
        // every million cycles), named by million cycles, to compare with
        // explorerhle's EXPLORER_HLE_FRAMES.
        if let Ok(dir) = std::env::var("EXPLORER_FRAMES") {
            let (_, _, pixels) = lx.bus.frame();
            if pixels != last_frame {
                shot(&lx, &format!("{dir}/{:06}.png", m + 1));
                last_frame = pixels;
            }
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
    println!("codec registers: {:02x?}", lx.bus.i2c.codec());
    println!("JIT: {:?}", lx.jit.stats);
    let g = &lx.bus.ga3d;
    let regs: Vec<String> = g.regs.iter().enumerate().filter(|r| *r.1 != 0).take(48).map(|(i, v)| format!("{:x}={v:08x}", i * 4)).collect();
    println!("GA3D: {} triangles, {} invalid, pixels {:?}; registers {}", g.triangles, g.invalid, g.pixels, regs.join(" "));
    let mlc: Vec<String> = (0..=0xA4).step_by(4).filter(|o| lx.bus.reg(0xC000_4000 + o) != 0).map(|o| format!("{o:02x}={:08x}", lx.bus.reg(0xC000_4000 + o))).collect();
    println!("MLC: {}", mlc.join(" "));
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
    if let Ok(path) = std::env::var("EXPLORER_STATE_OUT") {
        std::fs::write(&path, lx.save_state(&base)).expect("can't write the state");
    }
    if let Ok(path) = std::env::var("EXPLORER_RAM") {
        std::fs::write(&path, &lx.bus.sdram).expect("can't write the RAM dump");
    }
    let mut hot: Vec<_> = samples.into_iter().collect();
    hot.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let hot: Vec<String> = hot.iter().take(12).map(|(p, n)| format!("{p:08x}:{n}")).collect();
    println!("--- busiest pages (page:samples) ---\n{}", hot.join(" "));
    // EXPLORER_SAVE_OUT=<file.nandblocks>: the NAND's changes from the
    // packages' image, as the app keeps them (with its .nandimage).
    if let Ok(path) = std::env::var("EXPLORER_SAVE_OUT") {
        let base = ribbit_didj::explorer::nand_from_packages_blocks(dir.as_ref(), lx.bus.nand.size() / ribbit_didj::nand::BLOCK).unwrap_or_else(|e| panic!("{e}"));
        std::fs::write(&path, lx.bus.nand.changes_from(&base)).expect("can't write the save");
        std::fs::write(std::path::Path::new(&path).with_extension("nandimage"), format!("{:08x}", base.fingerprint())).expect("can't write the .nandimage");
    }
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
