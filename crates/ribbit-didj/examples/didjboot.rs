//! Boots the Didj from a NAND built from its firmware packages and prints
//! the UART output and the registers the software touched that have no
//! model yet (milestone 2):
//!   cargo run -p ribbit-didj --example didjboot -- <folder of .lfp files> [million instructions]

use ribbit_core::zip;
use ribbit_didj::brio;
use ribbit_didj::machine::Didj;
use ribbit_didj::nand::Nand;
use ribbit_didj::package::{self, Firmware};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: didjboot <folder of .lfp files> [million instructions]");
    let millions: u64 = args.get(1).map_or(50, |s| s.parse().expect("not a number"));

    let mut fw = Firmware::default();
    let mut brio_files = Vec::new();
    for e in std::fs::read_dir(dir).expect("can't read the folder").flatten() {
        let path = e.path();
        if package::package_id(&path.to_string_lossy()).is_none() {
            continue;
        }
        let data = std::fs::read(&path).expect("can't read a package");
        let mut rest = Vec::new();
        let mut firmware = false;
        for entry in zip::entries(&data).expect("not a zip archive") {
            let bytes = zip::read_entry(&data, &entry).unwrap();
            if fw.add(&entry.name, bytes.clone()) {
                firmware = true;
            }
            rest.push((entry.name, bytes));
        }
        // DIDJ_NO_BRIO=1 leaves Brio erased (the console then formats it).
        if !firmware && std::env::var_os("DIDJ_NO_BRIO").is_none() {
            brio_files.extend(brio::package_files(rest));
        }
    }
    let mut flash = Nand::from_firmware(&fw).unwrap_or_else(|e| panic!("{e}"));
    if !brio_files.is_empty() {
        brio::install(&mut flash, &brio_files).unwrap_or_else(|e| panic!("Brio: {e}"));
        println!("Brio: {} files and folders", brio_files.len());
    }

    let mut didj = Didj::new(flash);
    let mut ran = 0;
    let mut last_pc = u32::MAX;
    let mut same = 0;
    // Where the CPU is, sampled once per million instructions: 4 KB page -> count.
    let mut samples = std::collections::HashMap::<u32, u32>::new();
    // DIDJ_TYPE=$'ps\nls /Didj': shell commands, one per line, typed one per "# " prompt on UART0.
    let typed = std::env::var("DIDJ_TYPE").unwrap_or_default();
    let mut commands = typed.lines().filter(|c| !c.is_empty()).map(|c| format!("{c}
"));
    let (mut prompts, mut len, mut quiet) = (0, 0, 0);
    // DIDJ_WAV=<file> saves the sound sent to PCM out (16-bit stereo).
    let frames_dir = std::env::var("DIDJ_FRAMES").ok();
    let mut last_frame = Vec::new();
    let mut wav: Vec<u8> = Vec::new();
    // DIDJ_KEYS="50000:RIGHT 60000:A": at that many million cycles (the
    // run's count), hold a button for 30 million.
    let keys: Vec<(u64, u32)> = std::env::var("DIDJ_KEYS")
        .unwrap_or_default()
        .split_whitespace()
        .map(|k| {
            let (at, name) = k.split_once(':').expect("DIDJ_KEYS entries are <millions>:<button>");
            use ribbit_didj::machine::button::*;
            let bit = match name {
                "LEFT" => LEFT,
                "RIGHT" => RIGHT,
                "UP" => UP,
                "DOWN" => DOWN,
                "A" => A,
                "B" => B,
                "L" => L,
                "R" => R,
                "MENU" => MENU,
                "HINT" => HINT,
                "PAUSE" => PAUSE,
                _ => panic!("unknown button {name}"),
            };
            (at.parse().expect("bad DIDJ_KEYS time"), bit)
        })
        .collect();
    for m in 0..millions {
        didj.bus.buttons = keys.iter().filter(|&&(at, _)| (at..at + 30).contains(&m)).fold(0, |b, &(_, k)| b | k);
        let out = didj.bus.uart.first().map_or("", |s| s.as_str());
        (quiet, len) = if out.len() == len { (quiet + 1, len) } else { (0, out.len()) };
        let seen = out.matches("# ").count();
        // A new prompt, then 50 million instructions without output.
        if seen > prompts && quiet >= 50 {
            prompts = seen;
            if let Some(c) = commands.next() {
                didj.bus.uart_rx.extend(c.bytes());
            }
        }
        let n = didj.run(1_000_000);
        wav.extend(didj.bus.dma.pcm.drain(..));
        ran += n;
        // DIDJ_FRAMES=<folder> saves the screen every 100 million
        // instructions when it has changed.
        if let Some(dir) = &frames_dir
            && ran / 100_000_000 != (ran - n) / 100_000_000
        {
            let (w, h, px) = didj.bus.frame();
            if px != last_frame {
                let mut f = format!("P6
{w} {h}
255
").into_bytes();
                f.extend(px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
                std::fs::write(format!("{dir}/frame_{:06}.ppm", ran / 1_000_000), f).expect("can't write a frame");
                last_frame = px;
            }
        }
        *samples.entry(didj.cpu.r[15] & !0xFFF).or_default() += 1;
        // A tight loop that never leaves one address range: report and stop.
        let pc = didj.cpu.r[15] & !0xFF;
        same = if pc == last_pc { same + 1 } else { 0 };
        last_pc = pc;
        if same >= 2000 {
            println!("stuck near {:#010x}", didj.cpu.r[15]);
            break;
        }
    }
    let t = &mut didj.bus.timers;
    let c = t.read(0xC000_1808);
    t.write(0xC000_1808, c & !0x20 | 0x40);
    let r = [0xC000_0814, 0xC000_0810, 0xC000_0820, 0xC000_1808, 0xC000_1804, 0xC000_1800].map(|a| t.read(a));
    println!("IC mask {:08x}:{:08x} pend {:08x}; timer0 ctrl {:08x} match {:08x} count {:08x}", r[0], r[1], r[2], r[3], r[4], r[5]);
    println!("{ran} instructions, pc {:#010x}, cpsr {:#x}", didj.cpu.r[15], didj.cpu.cpsr);
    println!("PCM out: {} bytes, {} not zero", wav.len(), wav.iter().filter(|&&b| b != 0).count());
    if let Ok(path) = std::env::var("DIDJ_WAV") {
        let rate = didj.bus.dma.pcm_rate as u32;
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
    for (i, s) in didj.bus.uart.iter().enumerate() {
        if !s.is_empty() {
            println!("--- UART{i} ---\n{s}");
        }
    }
    println!("--- MLC (0xC0004000) ---");
    for row in (0..0xC0).step_by(0x10) {
        let v: Vec<String> = (0..4).map(|k| format!("{:08x}", didj.bus.reg(0xC000_4000 + row + 4 * k))).collect();
        println!("{row:02x}: {}", v.join(" "));
    }
    println!("--- unmodelled registers (address: first pc, reads, writes) ---");
    for (a, (pc, r, w)) in &didj.bus.touched {
        println!("{a:#010x}: {pc:#010x} r{r} w{w}");
    }
    println!("--- last unmodelled accesses (address pc r/w) ---");
    for (a, pc, w) in &didj.bus.recent {
        println!("{a:#010x} {pc:#010x} {}", if *w { "w" } else { "r" });
    }
    let mut hot: Vec<_> = samples.into_iter().collect();
    hot.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
    let hot: Vec<String> = hot.iter().take(12).map(|(p, n)| format!("{p:08x}:{n}")).collect();
    println!("--- busiest pages (page:samples) ---
{}", hot.join(" "));
    let g = &didj.bus.ga3d;
    println!("--- GA3D: {} register writes, {} invalid commands, {} triangles, starts (control: count) ---", g.trace.len(), g.invalid, g.triangles);
    println!("{}", g.starts.iter().map(|(c, n)| format!("{c:#x}:{n}")).collect::<Vec<_>>().join(" "));
    println!("pixels (in, z fail, transparent, alpha fail, written): {:?}", g.pixels.get());
    // DIDJ_GA3D=<file> saves the GA3D register writes (offset value).
    if let Ok(path) = std::env::var("DIDJ_GA3D") {
        let t: String = g.trace.iter().map(|(o, v)| format!("{o:04x} {v:08x}
")).collect();
        std::fs::write(&path, t).expect("can't write the GA3D trace");
    }
    // DIDJ_FRAME=<file.ppm> saves the screen.
    if let Ok(path) = std::env::var("DIDJ_FRAME") {
        let (w, h, px) = didj.bus.frame();
        let mut f = format!("P6
{w} {h}
255
").into_bytes();
        f.extend(px.iter().flat_map(|p| [(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
        std::fs::write(&path, f).expect("can't write the frame");
    }
    // DIDJ_DUMP=<file> saves SDRAM, to disassemble the kernel.
    if let Ok(path) = std::env::var("DIDJ_DUMP") {
        std::fs::write(&path, &didj.bus.sdram).expect("can't write the dump");
    }
    // DIDJ_NAND=<file> saves the Brio partition, each page with its spare area.
    if let Ok(path) = std::env::var("DIDJ_NAND") {
        let p = ribbit_didj::nand::partition("Brio").unwrap();
        let first = p.offset / ribbit_didj::nand::PAGE;
        let pages = (first..first + p.size / ribbit_didj::nand::PAGE).flat_map(|n| didj.bus.nand.page(n).to_vec());
        std::fs::write(&path, pages.collect::<Vec<u8>>()).expect("can't write the NAND");
    }
    for (pc, word) in didj.cpu.undefined_log.iter().take(10) {
        println!("undefined {word:#010x} at {pc:#010x}");
    }
}
