//! Scripted play-through with a per-scene speed report, to find the scenes
//! that emulate slower than real time (the user's own BIOS, cartridge and
//! saves; nothing is committed):
//!   cargo run --release -p ribbit-core --example scenes -- <bios> <cart> <script> [system.eeprom] [cart.sav]
//! Script lines (emulated time; `#` comments):
//!   run MS              run for MS milliseconds
//!   tap X Y [MS]        stylus down at LCD pixel (X, Y) for MS (default 200), then up
//!   btn NAME MS         hold a button (up down left right a b hint home pause) for MS
//!   shot PATH           write the screen as a PNG (2x)
//!   snap                keep the screen for the next `sheet`
//!   every MS N          N times: run MS, then snap
//!   sheet PATH          write the kept screens as one PNG (8 per row), then forget them
//!   save PATH / load PATH   save state
//!   label TEXT          name the following part of the report
//!   digest [NAME]       print the machine state digest (for comparing builds)
//!   lcdrate MS N        N times: run MS, print LCD frames/s and the idle-skipped share
//!   auxlog              list accesses to unmodelled auxiliary registers so far
//!   lcdregs             print the LCD registers and the last framebuffer DMA
//!   watch ADDR MS       single-step MS, printing each change of the word at ADDR and the PC
//!   bptrace PC MS [rN..] print registers (default r0-r3, blink) each time PC runs
//!   logtrace PC rF MS   print a printf-style debug log call (format in rF, args after)
//!   tvdump PATH         write the L-MAX TV chip's memory (RIBBIT_LMAX_TV=1)
//!   record / wav PREFIX record the host sound, then write it as the frontend
//!                       plays it with Leapster 2 PCM at 32 kHz (as emulated and
//!                       clean) and at 8 kHz
//! RIBBIT_MAME_GPIO=1 reads unresolved GPIO lines as 1 (MAME's workaround);
//! RIBBIT_GPIO_LINES=hex sets them; RIBBIT_BATTERY=hex sets the ADC battery input.
//! Every 250 ms of emulated time prints the speed (percent of real time at
//! the CPU clock) and how much was idle-skipped.

use ribbit_core::Machine;
use ribbit_core::io::touch::MameTouchConfig;
use ribbit_debug::Breakpoints;
use ribbit_input::Button;
use ribbit_video::{HEIGHT, WIDTH};
use std::time::Instant;

const WINDOW_MS: u64 = 250;

struct Meter {
    label: String,
    worst: f64,
    worst_label: String,
    rows: Vec<(String, f64, f64)>,
    /// `record`: the host sound as the frontend plays it, both ways (32 kHz).
    rec: Option<Recording>,
}

/// The frontend's two PCM choices (Audio menu), recorded side by side: per
/// 16 ms frame, the Leapster 2's 32 kHz DAC stream when it plays (else the
/// 8 kHz mix upsampled), and always the 8 kHz mix upsampled.
#[derive(Default)]
struct Recording {
    pcm32: Vec<i16>,
    pcm8: Vec<i16>,
    history: [f64; 16],
    dac_frames: u64,
    frames: u64,
}

impl Recording {
    fn take(&mut self, m: &mut Machine) {
        let mut low = vec![0i16; m.io.voices.output.len()];
        m.io.voices.output.pop_into(&mut low);
        let mut dac = vec![0i16; m.io.voices.dac_output.len()];
        m.io.voices.dac_output.pop_into(&mut dac);
        let mut up = Vec::with_capacity(low.len() * 4);
        for &x in &low {
            up.extend_from_slice(&ribbit_audio::upsample4x(&mut self.history, x));
        }
        self.frames += 1;
        if dac.is_empty() {
            self.pcm32.extend_from_slice(&up);
        } else {
            self.dac_frames += 1;
            self.pcm32.extend_from_slice(&dac);
        }
        self.pcm8.extend_from_slice(&up);
    }
}

fn write_wav(path: &str, samples: &[i16], hz: u32) {
    let mut b = Vec::with_capacity(44 + samples.len() * 2);
    let data = samples.len() as u32 * 2;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes()); // PCM
    b.extend_from_slice(&1u16.to_le_bytes()); // mono
    b.extend_from_slice(&hz.to_le_bytes());
    b.extend_from_slice(&(hz * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [bios, cart, script, rest @ ..] = &args[..] else {
        panic!("usage: scenes <bios> <cart> <script> [system.eeprom] [cart.sav]");
    };
    let image = ribbit_core::read_image(bios).unwrap();
    // RIBBIT_ANY_MODEL=1: EXPERIMENTAL, also boot Leapster TV / L-MAX / Leapster 2 BIOSes.
    let fw = if std::env::var_os("RIBBIT_ANY_MODEL").is_some() {
        ribbit_memory::Firmware::from_bytes_any_model(image)
    } else {
        ribbit_memory::Firmware::from_bytes(image)
    }
    .unwrap();
    // `none`: no cartridge inserted.
    let cart = (cart != "none").then(|| ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(cart).unwrap()).unwrap());
    let straps = std::env::var("RIBBIT_STRAPS").map_or(0x63FF_FFFF, |s| u32::from_str_radix(s.trim_start_matches("0x"), 16).unwrap());
    let mut m = Machine::with_straps(fw, cart, straps);
    if let Some(p) = rest.first() {
        m.io.eeprom.load(&std::fs::read(p).unwrap());
    }
    if let Some(p) = rest.get(1) {
        m.io.eeprom.insert_cart(Some(&std::fs::read(p).unwrap()));
    }
    m.enable_experimental_mame_touch(MameTouchConfig::default());
    if let Ok(s) = std::env::var("RIBBIT_STUBS") {
        // Research: fixed read values for unmodelled registers, "addr=value,...".
        for kv in s.split(',') {
            let (a, v) = kv.split_once('=').expect("addr=value");
            let hex = |x: &str| u32::from_str_radix(x.trim_start_matches("0x"), 16).unwrap();
            m.io.stubs.insert(hex(a), hex(v));
        }
    }
    if let (Ok(v), Some(p)) = (std::env::var("RIBBIT_L2_B008"), m.io.leapster2_power.as_mut()) {
        // Research: the Leapster 2's 0x0180_B008 (bits 1-2 = boot selection).
        p[1] = u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap();
    }
    m.record_trace_tail = false;
    if std::env::var_os("RIBBIT_TV").is_some() {
        // EXPERIMENTAL: Leapster TV (video chip and controller link).
        m.enable_leapster_tv();
        m.io.tv_link.as_mut().unwrap().logging = true;
    }
    if std::env::var_os("RIBBIT_LMAX_TV").is_some() {
        // EXPERIMENTAL: the L-MAX's TV chip (see io/lmax_tv.rs).
        m.io.lmax_tv = Some(Default::default());
    }
    m.keep_awake = std::env::var_os("RIBBIT_KEEP_AWAKE").is_some();
    // As the frontend: a new Leapster 2 gets the factory touch calibration
    // (RIBBIT_NO_FACTORY_CAL=1 leaves the flash blank), and after a
    // calibration boot the BIOS's restart skips calibration.
    if std::env::var_os("RIBBIT_NO_FACTORY_CAL").is_none() {
        m.ensure_touch_calibration();
    }
    if straps & ribbit_core::io::straps::SKIP_CALIBRATION == 0 {
        m.restart_straps = Some(straps | ribbit_core::io::straps::SKIP_CALIBRATION);
    }
    // Research: RIBBIT_IDLE_SKIP=0 turns off idle-loop fast-forwarding.
    m.idle_skip = std::env::var("RIBBIT_IDLE_SKIP").map_or(true, |v| v != "0");
    if let Ok(v) = std::env::var("RIBBIT_BATTERY") {
        // Research: the ADC's battery input level (hex, 0..=0x7FF).
        m.io.battery = u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap();
    }
    if std::env::var_os("RIBBIT_MAME_GPIO").is_some() {
        // EXPERIMENTAL: unresolved GPIO lines read 1 (MAME's workaround).
        m.io.gpio.unresolved_lines = !ribbit_input::BUTTON_BITS;
    }
    if let Ok(v) = std::env::var("RIBBIT_GPIO_LINES") {
        // Research: the value of the non-button GIODataIn bits (hex).
        m.io.gpio.unresolved_lines = u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap();
    }
    if let Ok(mhz) = std::env::var("RIBBIT_MHZ") {
        m.clock = ribbit_core::Clock::new(mhz.parse::<u64>().unwrap() * 1_000_000, m.clock.peripheral_hz);
    }
    let mut kept: Vec<Box<[u32; WIDTH * HEIGHT]>> = vec![];
    let mut meter = Meter { label: "boot".into(), worst: f64::MAX, worst_label: String::new(), rows: vec![], rec: None };
    for line in std::fs::read_to_string(script).unwrap().lines() {
        let line = line.split('#').next().unwrap().trim();
        let w: Vec<&str> = line.split_whitespace().collect();
        let num = |i: usize, d: u64| w.get(i).map_or(d, |s| s.parse().unwrap());
        match w.first().copied() {
            None => {}
            Some("run") => run_ms(&mut m, num(1, 0), &mut meter),
            Some("tap") => {
                let raw = |p: u64| ((p as u32 * 0x7F0 + 79) / 159).min(0x7F0);
                m.io.touch.as_mut().unwrap().live = Some((raw(num(1, 0)), raw(num(2, 0))));
                run_ms(&mut m, num(3, 200), &mut meter);
                m.io.touch.as_mut().unwrap().live = None;
            }
            Some("touch") => {
                // touch X Y: stylus down at an LCD pixel until `release` (no run).
                let raw = |p: u64| ((p as u32 * 0x7F0 + 79) / 159).min(0x7F0);
                m.io.touch.as_mut().unwrap().live = Some((raw(num(1, 0)), raw(num(2, 0))));
            }
            Some("hold") => m.buttons.set(Button::from_name(w[1]).expect("button name"), true),
            Some("lift") => m.buttons.set(Button::from_name(w[1]).expect("button name"), false),
            Some("release") => m.io.touch.as_mut().unwrap().live = None,
            Some("btn") => {
                let b = Button::from_name(w[1]).expect("button name");
                m.buttons.set(b, true);
                run_ms(&mut m, num(2, 200), &mut meter);
                m.buttons.set(b, false);
            }
            Some("shot") => shot(&m, w[1]),
            Some("pcm") => {
                // pcm: the Leapster 2 PCM DMA channels and the first samples of each buffer.
                let p = &m.io.voices.pcm;
                for ch in 0..2 {
                    let rd = |a: u32| (m.bus.peek8(a).unwrap_or(0) as u16 | (m.bus.peek8(a + 1).unwrap_or(0) as u16) << 8) as i16;
                    let words: Vec<String> = (0..16).map(|i| format!("{}", rd(p.base[ch] + 2 * i))).collect();
                    println!("pcm ch{ch} control {:#x} base {:#x} pos {:#x} (macmode {:#x}): {}", p.control[ch], p.base[ch], p.position[ch], m.cpu.macmode, words.join(" "));
                }
            }
            Some("record") => {
                // record: start recording the host sound (see `Recording`).
                m.io.voices.output.pop_into(&mut vec![0; m.io.voices.output.len()]);
                m.io.voices.dac_output.pop_into(&mut vec![0; m.io.voices.dac_output.len()]);
                meter.rec = Some(Recording::default());
            }
            Some("wav") => {
                // wav PREFIX: write PREFIX-32k.wav and PREFIX-8k.wav (both 32 kHz files).
                let r = meter.rec.take().expect("no `record`");
                write_wav(&format!("{}-32k.wav", w[1]), &r.pcm32, 32_000);
                write_wav(&format!("{}-8k.wav", w[1]), &r.pcm8, 32_000);
                // The frontend's "clean" option: the 32 kHz file low-passed.
                let mut lp = ribbit_audio::LowPass32k::new();
                let clean: Vec<i16> = r.pcm32.iter().map(|&x| lp.push(x)).collect();
                write_wav(&format!("{}-32k-clean.wav", w[1]), &clean, 32_000);
                println!("wav: {} frames, {} with the 32 kHz DAC stream, {:.1} s", r.frames, r.dac_frames, r.pcm8.len() as f64 / 32_000.0);
            }
            Some("snap") => kept.push(frame(&m)),
            Some("tvsnap") => {
                // tvsnap: keep the TV chip's picture (L-MAX / Leapster TV) for `sheet`.
                let mut px = Box::new([0u32; WIDTH * HEIGHT]);
                m.render_tv(&mut px).expect("no TV chip");
                kept.push(px);
            }
            Some("lcdsnap") => {
                // lcdsnap: keep the handheld LCD (not a TV picture) for `sheet`.
                let mut px = Box::new([0u32; WIDTH * HEIGHT]);
                m.render_lcd(&mut px);
                kept.push(px);
            }
            Some("every") => {
                for _ in 0..num(2, 1) {
                    run_ms(&mut m, num(1, 0), &mut meter);
                    kept.push(frame(&m));
                }
            }
            Some("sheet") => sheet(&std::mem::take(&mut kept), w[1]),
            Some("save") => std::fs::write(w[1], m.save_state()).unwrap(),
            Some("load") => {
                m.load_state(&std::fs::read(w[1]).unwrap()).unwrap();
                if std::env::var_os("RIBBIT_NO_TIMING").is_some() {
                    // Research: one cycle per instruction, no memory timing.
                    m.cpu.config.pipeline_timing = false;
                    m.io.mem.enabled = false;
                }
            }
            Some("label") => meter.label = w[1..].join(" "),
            Some("tvdump") => {
                let tv = m.io.lmax_tv.as_ref().expect("RIBBIT_LMAX_TV=1 for tvdump");
                std::fs::write(w[1], &tv.mem).unwrap();
                println!("tv memory: {} reads, {} writes", tv.reads, tv.writes);
                let mut v: Vec<_> = tv.pcs.iter().collect();
                v.sort_by_key(|p| std::cmp::Reverse(*p.1));
                for (pc, n) in v.iter().take(24) {
                    println!("  tv access pc {pc:08x} x{n}");
                }
                let mut seen = std::collections::BTreeMap::new();
                for &(pc, off, val, wr) in &tv.reg_log {
                    *seen.entry((off, wr, pc, val)).or_insert(0u32) += 1;
                }
                for ((off, wr, pc, val), n) in seen {
                    println!("  tv reg +{off:02x} {} {val:#010x} pc {pc:08x} x{n}", if wr { "W" } else { "R" });
                }
            }
            Some("break") => {
                // Run until the instruction at PC (or MAXMS), then print the registers.
                let pc = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let mut bp = Breakpoints::default();
                bp.add(pc);
                let end = m.cpu.cycles + num(2, 10_000) * (m.clock.cpu_hz / 1000);
                if m.cpu.pc == pc {
                    m.run(1, &Breakpoints::default()); // step off the last hit
                }
                while m.cpu.pc != pc && m.cpu.cycles < end {
                    m.run_cycles(end - m.cpu.cycles, &bp);
                }
                println!("break {pc:08x} at {} ms: hit={}", m.cpu.cycles / (m.clock.cpu_hz / 1000), m.cpu.pc == pc);
                for (i, r) in m.cpu.regs.iter().enumerate().take(32) {
                    print!("r{i}={r:08x}{}", if i % 8 == 7 { "\n" } else { " " });
                }
            }
            Some("idlecap") => {
                // idlecap SECS: cut the BIOS's idle count to at most SECS (to see
                // its power-off without waiting).
                m.cap_idle_count(num(1, 3) as u32);
            }
            Some("idlewatch") => {
                // idlewatch MS N: N times, run MS then print the idle count and
                // the screen's mean brightness.
                for _ in 0..num(2, 10) {
                    run_ms(&mut m, num(1, 100), &mut meter);
                    let left = m.idle_seconds_left();
                    let f = frame(&m);
                    let light = f.iter().map(|p| ((p >> 16) & 0xFF).max((p >> 8) & 0xFF).max(p & 0xFF) as u64).sum::<u64>() / f.len() as u64;
                    println!("{:10.1} ms  idle left {left:?}  brightness {light}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64);
                }
            }
            Some("idletimer") => {
                // The BIOS's inactivity timer (idle_timer.rs), if found.
                let t = ribbit_core::idle_timer::IdleTimer::find(m.cpu.regs[26], |a| m.bus.peek8(a));
                let w = |a: u32| (0..4).fold(0u32, |v, i| v | (m.bus.peek8(a + i).unwrap_or(0) as u32) << (8 * i));
                match t {
                    Some(t) => {
                        let s = w(t.state_ptr);
                        println!("idle timer {t:x?}: state {s:08x} reload {} count {}", w(s + t.reload_offset), w(s + t.count_offset));
                    }
                    None => println!("idle timer not found"),
                }
                println!("idle powered off: {}", m.idle_powered_off());
            }
            Some("hooktrace") => {
                // hooktrace N: the next N HD hook events (US v2.1 addresses):
                // edge constructor (scanline) or scanline driver (rectangle).
                let (ctor, scan) = (0x4004_E0D8u32, 0x4005_0514u32);
                let mut bp = Breakpoints::default();
                bp.add(ctor);
                bp.add(scan);
                let (mut last_rect, mut n) = (None, 0);
                let end = m.cpu.cycles + 2000 * (m.clock.cpu_hz / 1000);
                while n < num(1, 100) && m.cpu.cycles < end {
                    if m.cpu.pc == ctor || m.cpu.pc == scan {
                        m.run(1, &Breakpoints::default());
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    let w = |a: u32| (0..4).fold(0u32, |v, i| v | (m.bus.peek8(a + i).unwrap_or(0) as u32) << (8 * i)) as i32;
                    let t = m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64;
                    if m.cpu.pc == ctor {
                        println!("{t:10.3} edge y={}", m.cpu.regs[2] as i32);
                        n += 1;
                    } else if m.cpu.pc == scan {
                        let r = m.cpu.regs[14];
                        let rect = (w(r), w(r + 0x28), w(r + 0x2C), w(r + 0x30), w(r + 0x34));
                        if last_rect != Some(rect) || std::env::var_os("RIBBIT_ALLSCAN").is_some() {
                            println!("{t:10.3} scan r17={} rect x {}..{} y {}..{} scale {} (lcd transfers {})", m.cpu.regs[17] as i32, rect.1, rect.2, rect.3, rect.4, rect.0, m.io.lcd.transfers);
                            last_rect = Some(rect);
                            n += 1;
                        }
                    }
                }
            }
            Some("bptrace") => {
                // bptrace PC MS [r..]: each time PC runs within MS, print the time
                // and the given registers (default r0-r3, blink).
                let pc = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let regs: Vec<usize> = if w.len() > 3 { w[3..].iter().map(|r| r.trim_start_matches('r').parse().unwrap()).collect() } else { vec![0, 1, 2, 3, 31] };
                let mut bp = Breakpoints::default();
                bp.add(pc);
                let end = m.cpu.cycles + num(2, 1000) * (m.clock.cpu_hz / 1000);
                let mut hits = 0;
                while m.cpu.cycles < end && hits < 100000 {
                    if m.cpu.pc == pc {
                        m.run(1, &Breakpoints::default());
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    if m.cpu.pc == pc {
                        hits += 1;
                        print!("{:10.3} ms", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64);
                        for &r in &regs {
                            print!("  r{r}={:08x}", m.cpu.regs[r]);
                        }
                        println!();
                    }
                }
            }
            Some("bpdump") => {
                // bpdump PC MS REG LEN: each time PC runs, print LEN 16-bit words
                // at the address in register REG (research: audio buffers).
                let pc = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let reg: usize = w[3].trim_start_matches('r').parse().unwrap();
                let len = num(4, 16) as u32;
                let mut bp = Breakpoints::default();
                bp.add(pc);
                let end = m.cpu.cycles + num(2, 1000) * (m.clock.cpu_hz / 1000);
                while m.cpu.cycles < end {
                    if m.cpu.pc == pc {
                        m.run(1, &Breakpoints::default());
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    if m.cpu.pc == pc {
                        let a = m.cpu.regs[reg];
                        let rd = |a: u32| (m.bus.peek8(a).unwrap_or(0) as u16 | (m.bus.peek8(a + 1).unwrap_or(0) as u16) << 8) as i16;
                        let words: Vec<String> = (0..len).map(|i| rd(a + 2 * i).to_string()).collect();
                        println!("{:10.3} ms {a:#010x}: {}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, words.join(" "));
                    }
                }
            }
            Some("bpxy") => {
                // bpxy PC MS MEM START LEN: each time PC runs, print LEN XY-memory
                // words from START (MEM x or y, current bank) as 16-bit halves.
                let pc = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let which = usize::from(w[3] == "y");
                let start = usize::from_str_radix(w[4].trim_start_matches("0x"), 16).unwrap();
                let len = num(5, 16) as usize;
                let mut bp = Breakpoints::default();
                bp.add(pc);
                let end = m.cpu.cycles + num(2, 1000) * (m.clock.cpu_hz / 1000);
                while m.cpu.cycles < end {
                    if m.cpu.pc == pc {
                        m.run(1, &Breakpoints::default());
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    if m.cpu.pc == pc {
                        let xy = m.cpu.xy.as_ref().expect("no XY memory");
                        let bank = (xy.aux[0x18] & 1) as usize;
                        let words: Vec<String> = (start..start + len)
                            .flat_map(|i| {
                                let v = xy.mem[bank][which][i % 0x400];
                                [(v as u16 as i16).to_string(), ((v >> 16) as u16 as i16).to_string()]
                            })
                            .collect();
                        println!("{:10.3} ms: {}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, words.join(" "));
                    }
                }
            }
            Some("bpspan") => {
                // bpspan FROM TO MS: for each run from PC FROM to PC TO, print
                // the interrupts taken in between (research: DSP glitches).
                let from = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let to = u32::from_str_radix(w[2].trim_start_matches("0x"), 16).unwrap();
                let mut bp = Breakpoints::default();
                bp.add(from);
                bp.add(to);
                let end = m.cpu.cycles + num(3, 1000) * (m.clock.cpu_hz / 1000);
                let mut start: Option<[u64; 64]> = None;
                while m.cpu.cycles < end {
                    if m.cpu.pc == from || m.cpu.pc == to {
                        m.run(1, &Breakpoints::default());
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    if m.cpu.pc == from {
                        start = Some(m.interrupt_counts);
                    } else if m.cpu.pc == to
                        && let Some(s0) = start.take()
                    {
                        let taken: Vec<String> =
                            (0..64).filter(|&i| m.interrupt_counts[i] != s0[i]).map(|i| format!("{i}x{}", m.interrupt_counts[i] - s0[i])).collect();
                        println!("{:10.3} ms span irqs [{}]", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, taken.join(" "));
                    }
                }
            }
            Some("irqpc") => {
                // irqpc MS: single-step for MS, printing the PC each interrupt
                // interrupts (and the interrupt number).
                let end = m.cpu.cycles + num(1, 1000) * (m.clock.cpu_hz / 1000);
                let none = Breakpoints::default();
                while m.cpu.cycles < end {
                    let (pc, before) = (m.cpu.pc, m.interrupt_counts);
                    m.run(1, &none);
                    if let Some(i) = (0..64).find(|&i| m.interrupt_counts[i] != before[i]) {
                        println!("{:10.3} ms irq {i} at pc {pc:#010x} -> {:#010x}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, m.cpu.pc);
                        if std::env::var_os("IRQ_PATH").is_some() {
                            // The handler's path until it returns to `pc`.
                            let mut path = vec![];
                            while m.cpu.pc != pc && path.len() < 4000 {
                                path.push(format!("{:08x}", m.cpu.pc));
                                m.run(1, &none);
                            }
                            println!("  path {}", path.join(" "));
                        }
                    }
                }
            }
            Some("spanirq") => {
                // spanirq FROM TO MS: run to FROM, single-step to TO; for spans
                // with interrupts print where they hit and the peak of the 128
                // samples at FROM's r1 (research: the Leapster 2 PCM filter).
                let from = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let to = u32::from_str_radix(w[2].trim_start_matches("0x"), 16).unwrap();
                let mut bp = Breakpoints::default();
                bp.add(from);
                let none = Breakpoints::default();
                let end = m.cpu.cycles + num(3, 1000) * (m.clock.cpu_hz / 1000);
                while m.cpu.cycles < end {
                    if m.cpu.pc == from {
                        m.run(1, &none);
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    if m.cpu.pc != from {
                        continue;
                    }
                    let out = m.cpu.regs[1];
                    let mut hits = vec![];
                    while m.cpu.pc != to {
                        let (pc, before) = (m.cpu.pc, m.interrupt_counts);
                        m.run(1, &none);
                        if let Some(i) = (0..64).find(|&i| m.interrupt_counts[i] != before[i]) {
                            hits.push(format!("{i}@{pc:08x}"));
                        }
                    }
                    let rd = |a: u32| (m.bus.peek8(a).unwrap_or(0) as u16 | (m.bus.peek8(a + 1).unwrap_or(0) as u16) << 8) as i16;
                    let peak = (0..128).map(|i| rd(out + 2 * i).unsigned_abs()).max().unwrap();
                    if !hits.is_empty() || peak >= 32767 {
                        println!("{:10.3} ms span peak {peak} first {} irqs {}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, rd(out), hits.join(" "));
                    }
                }
            }
            Some("pchist") => {
                // pchist MS: sample the PC every 10 us for MS; print the top
                // 256-byte code blocks (where the CPU spends its time).
                let mut h: std::collections::HashMap<u32, u64> = Default::default();
                let step = m.clock.cpu_hz / 100_000;
                let none = Breakpoints::default();
                for _ in 0..num(1, 100) * 100 {
                    m.run_cycles(step, &none);
                    let masked = m.cpu.status32 & (ribbit_cpu::status32::E1 | ribbit_cpu::status32::E2) == 0;
                    if std::env::var_os("PCHIST_MASKED").is_none() || masked {
                        *h.entry(m.cpu.pc & !std::env::var("PCHIST_MASK").map_or(0xFF, |v| u32::from_str_radix(&v, 16).unwrap())).or_default() += 1;
                    }
                }
                let mut v: Vec<_> = h.into_iter().collect();
                v.sort_by_key(|a| std::cmp::Reverse(a.1));
                for (pc, n) in v.iter().take(12) {
                    println!("pchist {pc:#010x} {n}");
                }
            }
            Some("adcstat") => {
                // adcstat: the ADC's conversion counters and slot controls.
                let a = &m.io.adc;
                println!("adc conversions {} dropped {} empty reads {} fifo {} controls {:x?} config {:#x}", a.conversions, a.dropped, a.empty_reads, a.fifo.len(), a.controls, a.config);
            }
            Some("flashstat") => {
                // flashstat [PATH]: the Leapster 2 SPI flash's programmed bytes
                // (and their first offsets); with PATH, write it to a file.
                if let Some(f) = m.io.spi_flash.as_ref() {
                    let used: Vec<usize> = (0..f.data.len()).filter(|&i| f.data[i] != 0xFF).collect();
                    println!("flash {} bytes programmed, first {:x?}", used.len(), &used[..used.len().min(8)]);
                    if let Some(p) = w.get(1) {
                        std::fs::write(p, &f.data).unwrap();
                    }
                }
            }
            Some("flashload") => {
                // flashload PATH: load the SPI flash from a file (as the frontend does).
                let data = std::fs::read(w[1]).unwrap();
                m.io.spi_flash.as_mut().expect("no SPI flash").load(&data);
            }
            Some("irqrate") => {
                // irqrate MS: interrupts taken per line over the next MS.
                let before = m.interrupt_counts;
                run_ms(&mut m, num(1, 1000), &mut meter);
                let per: Vec<String> =
                    (0..64).filter(|&i| m.interrupt_counts[i] != before[i]).map(|i| format!("{i}:{}", m.interrupt_counts[i] - before[i])).collect();
                println!("irqrate {}", per.join(" "));
            }
            Some("maskspans") => {
                // maskspans MS: single-step for MS; print each stretch with
                // interrupts masked longer than 1 ms (start PC, length).
                let none = Breakpoints::default();
                let end = m.cpu.cycles + num(1, 1000) * (m.clock.cpu_hz / 1000);
                let per_us = m.clock.cpu_hz / 1_000_000;
                let mut start: Option<(u64, u32)> = None;
                let mut prev_pc = 0u32;
                let mut pcs: std::collections::HashMap<u32, u64> = Default::default();
                while m.cpu.cycles < end {
                    let masked = m.cpu.status32 & (ribbit_cpu::status32::E1 | ribbit_cpu::status32::E2) == 0;
                    match (masked, start) {
                        (true, None) => {
                            start = Some((m.cpu.cycles, m.cpu.pc));
                            pcs.clear();
                            if let Ok(n) = std::env::var("MASK_PATH") {
                                // Research: the first N PCs of every masked stretch.
                                let (save, mut path) = (m.save_state(), vec![]);
                                for _ in 0..n.parse::<usize>().unwrap() {
                                    path.push(format!("{:x}", m.cpu.pc));
                                    m.run(1, &none);
                                }
                                if m.cpu.status32 & (ribbit_cpu::status32::E1 | ribbit_cpu::status32::E2) == 0 {
                                    println!("  path {}", path.join(" "));
                                }
                                m.load_state(&save).unwrap();
                            }
                        }
                        (true, Some(_)) => *pcs.entry(m.cpu.pc & !0xFF).or_default() += 1,
                        (false, Some((c0, pc0))) => {
                            let us = (m.cpu.cycles - c0) / per_us;
                            if us >= 1000 {
                                println!("  unmasked at pc {:#010x} (previous pc {:#010x})", m.cpu.pc, prev_pc);
                                let mut v: Vec<_> = pcs.iter().collect();
                                v.sort_by(|a, b| b.1.cmp(a.1));
                                let top: Vec<String> = v.iter().take(4).map(|(p, n)| format!("{p:08x}x{n}")).collect();
                                println!("masked {us} us from pc {pc0:#010x}, mostly {}", top.join(" "));
                            }
                            start = None;
                        }
                        _ => {}
                    }
                    prev_pc = m.cpu.pc;
                    m.run(1, &none);
                }
            }
            Some("logtrace") => {
                // logtrace PC FMTREG MS: each time PC runs, print the printf-style
                // string in register FMTREG with the following registers as its
                // arguments (a game's debug log).
                let pc = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let fr: usize = w[2].trim_start_matches('r').parse().unwrap();
                let mut bp = Breakpoints::default();
                bp.add(pc);
                let end = m.cpu.cycles + num(3, 1000) * (m.clock.cpu_hz / 1000);
                let cstr = |m: &Machine, a: u32| {
                    let mut v = vec![];
                    for i in 0..200 {
                        match m.bus.peek8(a + i) {
                            Some(0) | None => break,
                            Some(c) => v.push(c),
                        }
                    }
                    String::from_utf8_lossy(&v).into_owned()
                };
                while m.cpu.cycles < end {
                    if m.cpu.pc == pc {
                        m.run(1, &Breakpoints::default());
                    }
                    m.run_cycles(end - m.cpu.cycles, &bp);
                    if m.cpu.pc != pc {
                        continue;
                    }
                    let fmt = cstr(&m, m.cpu.regs[fr]);
                    let (mut out, mut arg, mut it) = (String::new(), fr + 1, fmt.chars().peekable());
                    while let Some(c) = it.next() {
                        if c != '%' {
                            out.push(c);
                            continue;
                        }
                        let mut spec = String::new();
                        while let Some(&d) = it.peek() {
                            it.next();
                            if d.is_ascii_alphabetic() || d == '%' {
                                spec.push(d);
                                break;
                            }
                        }
                        let v = if arg < 8 { m.cpu.regs[arg] } else { 0 };
                        arg += 1;
                        match spec.as_str() {
                            "s" => out.push_str(&cstr(&m, v)),
                            "x" | "X" | "p" => out.push_str(&format!("{v:x}")),
                            "%" => out.push('%'),
                            _ => out.push_str(&format!("{}", v as i32)),
                        }
                    }
                    println!("{:10.3} ms  {}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, out.trim_end());
                }
            }
            Some("lcdrate") => {
                // lcdrate MS N: N times, run MS and print the LCD transfers
                // (frames sent to the screen) per second and the share of steps idle-skipped.
                for _ in 0..num(2, 10) {
                    let (t0, c0, i0) = (m.io.lcd.transfers, m.cpu.cycles, m.idle_skipped_steps());
                    run_ms(&mut m, num(1, 1000), &mut meter);
                    let secs = (m.cpu.cycles - c0) as f64 / m.clock.cpu_hz as f64;
                    let idle = (m.idle_skipped_steps() - i0) as f64 / (m.cpu.cycles - c0) as f64;
                    println!(
                        "{:10.1} ms  {:5.1} frames/s  idle-skipped {:4.1}%",
                        m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64,
                        (m.io.lcd.transfers - t0) as f64 / secs,
                        100.0 * idle
                    );
                }
            }
            Some("auxlog") => {
                // auxlog: accesses to auxiliary registers Ribbit does not model
                // (cache controls and others) so far, each distinct one once.
                let mut seen = std::collections::BTreeMap::new();
                for x in &m.cpu.diag.aux_unmodelled {
                    *seen.entry((x.reg, format!("{:?}", x.kind), x.value, x.pc)).or_insert(0u32) += 1;
                }
                for ((reg, kind, value, pc), n) in seen {
                    println!("aux {reg:#06x} {kind:5} {value:#010x} pc {pc:08x} x{n}");
                }
                println!("({} unmodelled accesses in all)", m.cpu.diag.aux_unmodelled_total);
            }
            Some("uart") => {
                // uart: everything sent to the debug UART so far.
                println!("--- uart ({} bytes) ---
{}
---", m.io.uart.tx.len(), m.io.uart.text());
            }
            Some("unmapped") => {
                // unmapped: distinct accesses to unmapped addresses so far.
                for ((addr, width, kind), s) in m.bus.unmapped_summary() {
                    println!("unmapped {addr:#010x} w{width} {kind:?} {s:?}");
                }
            }
            Some("strtrace") => {
                // strtrace LO HI MS: single-step MS; print each PC (once) at which
                // one of r0-r3 points into LO..HI (who uses a string or table).
                let hex = |x: &str| u32::from_str_radix(x.trim_start_matches("0x"), 16).unwrap();
                let (lo, hi) = (hex(w[1]), hex(w[2]));
                let end = m.cpu.cycles + num(3, 1000) * (m.clock.cpu_hz / 1000);
                let (mut seen, none) = (std::collections::BTreeSet::new(), Breakpoints::default());
                while m.cpu.cycles < end {
                    let pc = m.cpu.pc;
                    if let Some(r) = (0..4).find(|&r| (lo..hi).contains(&m.cpu.regs[r]))
                        && seen.insert(pc)
                    {
                        println!("{:10.3} ms pc {pc:08x} r{r}={:08x} blink={:08x}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64, m.cpu.regs[r], m.cpu.regs[31]);
                    }
                    m.run(1, &none);
                }
            }
            Some("unmappedtrace") => {
                // unmappedtrace MS [N]: single-step MS; at each of the first N
                // (default 20) accesses to an unmapped address, print the PC
                // and registers r0-r4, r13-r25, blink.
                let end = m.cpu.cycles + num(1, 1000) * (m.clock.cpu_hz / 1000);
                let (mut left, none) = (num(2, 20), Breakpoints::default());
                let mut seen = m.bus.unmapped_total();
                while m.cpu.cycles < end && left > 0 {
                    let pc = m.cpu.pc;
                    let regs = m.cpu.regs;
                    m.run(1, &none);
                    if m.bus.unmapped_total() != seen {
                        seen = m.bus.unmapped_total();
                        left -= 1;
                        print!("{:10.3} ms pc {pc:08x}", m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64);
                        for r in (0..5).chain(13..26).chain([31]) {
                            print!(" r{r}={:x}", regs[r]);
                        }
                        println!();
                    }
                }
            }
            Some("sdsector") => {
                // sdsector N: hex dump of SD card sector N (Leapster 2).
                let n: u32 = w[1].parse().unwrap();
                let card = m.io.sd.as_ref().and_then(|s| s.card.as_ref()).expect("no SD card");
                let data = card.sectors.get(&n).cloned().unwrap_or_else(|| vec![0; 512]);
                for (i, row) in data.chunks(16).enumerate() {
                    if row.iter().any(|&b| b != 0) {
                        println!("sd {n}+{:03x}: {}", i * 16, row.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" "));
                    }
                }
            }
            Some("sdlog") => {
                // sdlog: the SD commands so far (Leapster 2), runs collapsed.
                let log = m.io.sd.as_ref().map(|s| s.log.clone()).unwrap_or_default();
                let mut i = 0;
                while i < log.len() {
                    let (c, a, st) = log[i];
                    let mut n = 1;
                    while i + n < log.len() && log[i + n].0 == c && log[i + n].2 == st && (c != 17 && c != 24 || log[i + n].1 == a + 512 * n as u32) {
                        n += 1;
                    }
                    println!("sd cmd{c} arg {a:#x} status {st:#06x}{}", if n > 1 { format!(" x{n}") } else { String::new() });
                    i += n;
                }
            }
            Some("lcdregs") => {
                // lcdregs: the LCD controller and its last framebuffer DMA.
                let l = &m.io.lcd;
                println!(
                    "lcd base {:#x} stride {:#x} format {:#x}; dma source {:#010x} stride {:#x} lines {} offset {:#x}",
                    l.regs.fb_base, l.regs.stride, l.regs.format, l.dma_source, l.dma_stride, l.dma_lines, l.dma_offset
                );
            }
            Some("watch") => {
                // watch ADDR MS: single-step for MS, printing each change of the
                // 32-bit word at ADDR with the PC that changed it.
                let addr = u32::from_str_radix(w[1].trim_start_matches("0x"), 16).unwrap();
                let rd = |m: &Machine| (0..4).fold(0u32, |v, i| v | (m.bus.peek8(addr + i).unwrap_or(0) as u32) << (8 * i));
                let end = m.cpu.cycles + num(2, 1000) * (m.clock.cpu_hz / 1000);
                let (mut last, none) = (rd(&m), Breakpoints::default());
                while m.cpu.cycles < end {
                    let pc = m.cpu.pc;
                    m.run(1, &none);
                    let v = rd(&m);
                    if v != last {
                        println!(
                            "{:10.3} ms  [{addr:08x}] {last:08x} -> {v:08x}  pc {pc:08x} blink {:08x}",
                            m.cpu.cycles as f64 / (m.clock.cpu_hz / 1000) as f64,
                            m.cpu.regs[31]
                        );
                        last = v;
                    }
                }
            }
            Some("mem") => {
                // mem ADDR LEN PATH: write emulated memory (RAM/ROM/VRAM) to a file.
                let hex = |x: &str| u32::from_str_radix(x.trim_start_matches("0x"), 16).unwrap();
                let a = match w[1].strip_prefix('r') {
                    Some(r) => m.cpu.regs[r.parse::<usize>().unwrap()], // "rN": the address in a register
                    None => hex(w[1]),
                };
                let n = hex(w[2]);
                let bytes: Vec<u8> = (0..n).map(|i| m.bus.peek8(a + i).unwrap_or(0)).collect();
                std::fs::write(w[3], bytes).unwrap();
            }
            Some("linklog") => {
                // The Leapster TV controller link's transactions since the last linklog.
                let l = m.io.tv_link.as_mut().expect("RIBBIT_TV=1 for linklog");
                let mut last = String::new();
                for s in std::mem::take(&mut l.log) {
                    if s != last {
                        println!("link: {s}");
                    }
                    last = s;
                }
            }
            Some("digest") => println!("digest {} {:016x} at cycle {}", w.get(1).unwrap_or(&""), m.state_digest(), m.cpu.cycles),
            Some(other) => panic!("unknown command {other}"),
        }
    }
    println!("\nworst window: {:.0}% ({})", meter.worst, meter.worst_label);
    let mut by: Vec<(String, f64, f64, usize)> = vec![];
    for (l, s, idle) in &meter.rows {
        match by.iter_mut().find(|r| &r.0 == l) {
            Some(r) => {
                r.1 = r.1.min(*s);
                r.2 += idle;
                r.3 += 1
            }
            None => by.push((l.clone(), *s, *idle, 1)),
        }
    }
    for (l, s, idle, n) in by {
        println!("{l:30} min {s:6.0}%  idle {:4.0}%  ({n} windows)", idle / n as f64);
    }
}

fn run_ms(m: &mut Machine, ms: u64, meter: &mut Meter) {
    let per_ms = m.clock.cpu_hz / 1000;
    let mut left = ms;
    while left > 0 {
        let chunk = left.min(if meter.rec.is_some() { 16 } else { WINDOW_MS });
        left -= chunk;
        let skipped0 = m.idle_skipped_steps();
        let t = Instant::now();
        let r = match std::env::var("REC_CHUNK_US").ok().filter(|_| meter.rec.is_some()) {
            // Research: run the window in small slices, as the frontend does.
            Some(us) => {
                let slice = us.parse::<u64>().unwrap() * per_ms / 1000;
                let end = m.cpu.cycles + chunk * per_ms;
                let mut last = m.run_cycles(slice.min(end - m.cpu.cycles), &Breakpoints::default());
                let mut steps = last.steps;
                while m.cpu.cycles < end {
                    last = m.run_cycles(slice.min(end - m.cpu.cycles), &Breakpoints::default());
                    steps += last.steps;
                }
                last.steps = steps;
                last
            }
            None => m.run_cycles(chunk * per_ms, &Breakpoints::default()),
        };
        let wall = t.elapsed().as_secs_f64();
        assert_eq!(r.stop, ribbit_core::StopReason::StepLimit, "{r:?}");
        let speed = 100.0 * (chunk as f64 / 1000.0) / wall;
        let idle = 100.0 * (m.idle_skipped_steps() - skipped0) as f64 / r.steps as f64;
        if let Some(rec) = meter.rec.as_mut() {
            rec.take(m);
        }
        if chunk == WINDOW_MS {
            let t_ms = m.cpu.cycles / per_ms;
            if speed < meter.worst {
                meter.worst = speed;
                meter.worst_label = format!("{} at {t_ms} ms", meter.label);
            }
            meter.rows.push((meter.label.clone(), speed, idle));
            if std::env::var_os("VERBOSE").is_some() {
                println!("{t_ms:8} ms {:24} {speed:6.0}% idle {idle:3.0}% {:.1} ns/step {} steps", meter.label, wall * 1e9 / r.steps as f64, r.steps);
            }
        }
    }
}

fn frame(m: &Machine) -> Box<[u32; WIDTH * HEIGHT]> {
    let mut px = Box::new([0u32; WIDTH * HEIGHT]);
    m.render_frame(&mut px);
    px
}

fn sheet(frames: &[Box<[u32; WIDTH * HEIGHT]>], path: &str) {
    let cols = 8.min(frames.len().max(1));
    let rows = frames.len().div_ceil(cols).max(1);
    let (w, h) = (cols * (WIDTH + 4), rows * (HEIGHT + 4));
    let mut rgb = vec![255u8; w * h * 3];
    for (i, f) in frames.iter().enumerate() {
        let (ox, oy) = ((i % cols) * (WIDTH + 4), (i / cols) * (HEIGHT + 4));
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let p = f[y * WIDTH + x];
                let o = ((oy + y) * w + ox + x) * 3;
                rgb[o..o + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w as u32, h as u32);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}

fn shot(m: &Machine, path: &str) {
    // PNG at 2x (nearest), to look at.
    let px = frame(m);
    let s = 2;
    let mut rgb = Vec::with_capacity(WIDTH * HEIGHT * 3 * s * s);
    for y in 0..HEIGHT * s {
        for x in 0..WIDTH * s {
            let p = px[(y / s) * WIDTH + x / s];
            rgb.extend([(p >> 16) as u8, (p >> 8) as u8, p as u8]);
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), (WIDTH * s) as u32, (HEIGHT * s) as u32);
    e.set_color(png::ColorType::Rgb);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}
