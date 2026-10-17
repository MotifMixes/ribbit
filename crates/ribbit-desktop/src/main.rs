//! `ribbit` command-line entry point.
//!
//! Exit codes for `headless`: 0 = stopped at the step limit, a halt or a
//! breakpoint, 2 = CPU stopped (unimplemented instruction or halt),
//! 1 = usage or load error.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use ribbit_core::{Config, Machine, RunReport, StopReason, load_cartridge, load_firmware};
use ribbit_debug::{Breakpoints, hexdump};
use ribbit_memory::cartridge::scan_swf;
use ribbit_memory::{Cartridge, Firmware, map};
use ribbit_video::{FrameStatus, HEIGHT, WIDTH};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

#[derive(Parser)]
#[command(
    name = "ribbit",
    version,
    about = "Original LeapFrog Leapster emulator"
)]
struct Cli {
    /// TOML configuration file (paths may also be given per command).
    #[arg(long, global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Args, Default)]
struct Images {
    /// System firmware ("BaseROM") dumped from your own Leapster.
    #[arg(long)]
    firmware: Option<PathBuf>,
    /// Cartridge dump.
    #[arg(long)]
    cart: Option<PathBuf>,
    /// EXPERIMENTAL: also boot Leapster TV, L-MAX and Leapster 2 BIOSes on
    /// the original Leapster's emulated hardware (research only).
    #[arg(long)]
    experimental_any_model: bool,
}

// Parsed once at start-up; boxing the large `Headless` variant would buy nothing.
#[allow(clippy::large_enum_variant)]
#[derive(Subcommand)]
enum Command {
    /// Identify firmware and/or cartridge images (size, checksums, known dump).
    Info(Images),
    /// Show a cartridge's header directory and embedded Flash movies.
    InspectCart {
        path: PathBuf,
        /// Also hex-dump the first N bytes.
        #[arg(long, default_value_t = 0)]
        dump: usize,
    },
    /// Run without a window (for CI and automated tests).
    Headless {
        #[command(flatten)]
        images: Images,
        /// Maximum CPU steps (instructions, interrupt entries and sleep cycles).
        #[arg(long, alias = "max-instructions", default_value_t = 1_000_000)]
        max_steps: u64,
        /// Stop when PC reaches this address (hex, e.g. 0x40001234).
        #[arg(long, value_parser = parse_hex)]
        breakpoint: Vec<u32>,
        /// Ignore breakpoints during the first N steps.
        #[arg(long, default_value_t = 0)]
        arm_breakpoints_after: u64,
        /// Write every executed instruction and interrupt to this file.
        #[arg(long)]
        trace: Option<PathBuf>,
        /// Start the --trace output only after N steps.
        #[arg(long, default_value_t = 0)]
        trace_from: u64,
        /// Number of most recent instructions to print when the run stops.
        #[arg(long, default_value_t = 24)]
        tail: usize,
        /// Write the decoded LCD contents to a PPM file afterwards.
        #[arg(long)]
        dump_frame: Option<PathBuf>,
        /// Start from a save state (same BIOS and cartridge); --max-steps
        /// then counts from the state.
        #[arg(long)]
        load_state: Option<PathBuf>,
        /// Write a save state when the run ends.
        #[arg(long)]
        save_state: Option<PathBuf>,
        /// DEBUG ONLY: fixed read value for an unmodelled peripheral register,
        /// e.g. 0x01809004=0x00200000 (to follow another emulator's path).
        #[arg(long, value_parser = parse_stub)]
        mmio_stub: Vec<(u32, u32)>,
        /// Board straps (0x0180_9004) instead of the default 0x63FFBFFF.
        #[arg(long, value_parser = parse_hex)]
        straps: Option<u32>,
        /// System EEPROM file: loaded if present, saved if changed.
        #[arg(long)]
        eeprom: Option<PathBuf>,
        /// Cartridge save-chip file: loaded if present, saved if changed.
        #[arg(long)]
        cart_save: Option<PathBuf>,
        /// CPU clock in MHz (default 96 nominal; MAME uses 48). Placeholder timing.
        #[arg(long)]
        cpu_mhz: Option<u64>,
        /// Non-button bits of GPIO input 0x0180_004C (UNKNOWN on hardware;
        /// default 0). E.g. MAME's workaround value 0x48FC1C7F.
        #[arg(long, value_parser = parse_hex)]
        input_lines: Option<u32>,
        /// Hex-dump emulated memory after the run, e.g. 0x0300ff00:0x100.
        #[arg(long, value_parser = parse_range)]
        dump_mem: Vec<(u32, u32)>,
        /// EXPERIMENTAL: attach a stylus to the resistive touch panel model
        /// (component values unverified; up unless --touch-press is given).
        #[arg(long)]
        experimental_mame_touch: bool,
        /// Scripted stylus press for --experimental-mame-touch:
        /// START_MS,DURATION_MS,X,Y (emulated ms; X/Y raw units 0..=0x7F0 across the screen).
        #[arg(long, requires = "experimental_mame_touch")]
        touch_press: Vec<ribbit_core::io::touch::TouchPress>,
        /// Scripted button press: NAME,START_MS,DURATION_MS (emulated time),
        /// e.g. left,131000,500. Names: up down left right a b hint home pause.
        #[arg(long, value_parser = parse_button_press)]
        button_press: Vec<(ribbit_input::Button, u64, u64)>,
        /// Write every executed PC with the cycle it was first executed,
        /// in first-visit order ("cycle pc" lines).
        #[arg(long)]
        coverage: Option<PathBuf>,
    },
    /// Open the desktop window (the default when no command is given).
    Run(Images),
}

fn parse_hex(s: &str) -> Result<u32, String> {
    let digits = s.strip_prefix("0x").or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    u32::from_str_radix(&digits.replace('_', ""), 16).map_err(|e| e.to_string())
}

fn parse_button_press(s: &str) -> Result<(ribbit_input::Button, u64, u64), String> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    let [name, start, dur] = parts[..] else {
        return Err("expected NAME,START_MS,DURATION_MS".into());
    };
    let button = ribbit_input::Button::from_name(name).ok_or_else(|| format!("unknown button {name:?}"))?;
    Ok((button, start.parse().map_err(|e| format!("{e}"))?, dur.parse().map_err(|e| format!("{e}"))?))
}

fn parse_stub(s: &str) -> Result<(u32, u32), String> {
    let (a, v) = s.split_once('=').ok_or("expected ADDR=VALUE")?;
    Ok((parse_hex(a)?, parse_hex(v)?))
}

fn parse_range(s: &str) -> Result<(u32, u32), String> {
    let (a, n) = s.split_once(':').ok_or("expected ADDR:LEN")?;
    Ok((parse_hex(a)?, parse_hex(n)?))
}

fn main() -> ExitCode {
    match real_main() {
        Ok(code) => code,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::from(1)
        }
    }
}

fn real_main() -> Result<ExitCode> {
    let cli = Cli::parse();
    let config = match &cli.config {
        Some(path) => Config::load(path)?,
        None => Config::default(),
    };
    ribbit_debug::init_logging(config.log_filter.as_deref().unwrap_or("info"));

    // No command: open the window.
    match cli.command.unwrap_or(Command::Run(Images::default())) {
        Command::Info(images) => {
            let (fw, cart) = resolve(&images, &config);
            if fw.is_none() && cart.is_none() {
                bail!("give --firmware and/or --cart");
            }
            if let Some(path) = fw {
                print_firmware(&path, &load_firmware(&path)?);
            }
            if let Some(path) = cart {
                print_cart(&path, &load_cartridge(&path)?);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::InspectCart { path, dump } => {
            let cart = load_cartridge(&path)?;
            print_cart(&path, &cart);
            inspect_cart(&cart, dump);
            Ok(ExitCode::SUCCESS)
        }
        Command::Headless {
            images,
            max_steps,
            breakpoint,
            trace,
            tail,
            dump_frame,
            load_state,
            save_state,
            mmio_stub,
            dump_mem,
            straps,
            experimental_mame_touch,
            touch_press,
            coverage,
            arm_breakpoints_after,
            trace_from,
            input_lines,
            button_press,
            eeprom,
            cpu_mhz,
            cart_save,
        } => {
            let mut config = config;
            config.system_eeprom = eeprom.or(config.system_eeprom);
            config.cpu_mhz = cpu_mhz.or(config.cpu_mhz);
            config.cart_save = cart_save.or(config.cart_save);
            config.straps = straps.or(config.straps);
            config.input_lines = input_lines.or(config.input_lines);
            config.experimental_mame_touch |= experimental_mame_touch;
            let mut machine = build_machine(&images, &config)?;
            if let Some(path) = &load_state {
                let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
                machine.load_state(&bytes).with_context(|| format!("loading state {}", path.display()))?;
                println!("state loaded from {} (cycle {})", path.display(), machine.cpu.cycles);
            }
            if !touch_press.is_empty() {
                machine
                    .enable_experimental_mame_touch(ribbit_core::io::touch::MameTouchConfig { presses: touch_press });
            }
            for (addr, value) in mmio_stub {
                eprintln!("warning: debug stub {addr:#010x} reads as {value:#010x} (not emulation)");
                machine.io.stubs.insert(addr & !3, value);
            }
            let mut sink = match &trace {
                Some(path) => {
                    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
                    Some(Box::new(std::io::BufWriter::new(file)) as Box<dyn std::io::Write + Send>)
                }
                None => None,
            };
            let mut bps = Breakpoints::default();
            breakpoint.into_iter().for_each(|pc| bps.add(pc));
            if coverage.is_some() {
                machine.coverage = Some(Default::default());
            }
            // Run in segments so tracing and breakpoints can start late.
            // Button presses take effect at exact emulated times (one cycle
            // per step at the CPU clock).
            let steps_per_ms = machine.clock.cpu_hz / 1000;
            let steps_at = move |ms: u64| ms * steps_per_ms;
            let press_bounds: Vec<u64> =
                button_press.iter().flat_map(|&(_, start, dur)| [steps_at(start), steps_at(start + dur)]).collect();
            let mut bounds: Vec<u64> = [arm_breakpoints_after, trace_from, max_steps]
                .into_iter()
                .chain(press_bounds)
                .filter(|&b| b > 0 && b <= max_steps)
                .collect();
            bounds.sort_unstable();
            bounds.dedup();
            let none = Breakpoints::default();
            let mut report =
                RunReport { steps: 0, instructions: 0, interrupts: 0, pc: machine.cpu.pc, stop: StopReason::StepLimit };
            for end in bounds {
                if report.steps >= trace_from && machine.trace_sink.is_none() {
                    machine.trace_sink = sink.take();
                }
                let active = if report.steps >= arm_breakpoints_after { &bps } else { &none };
                for &(button, start, dur) in &button_press {
                    let down = (steps_at(start)..steps_at(start + dur)).contains(&report.steps);
                    machine.buttons.set(button, down);
                }
                let r = machine.run(end - report.steps, active);
                report = RunReport {
                    steps: report.steps + r.steps,
                    instructions: report.instructions + r.instructions,
                    interrupts: report.interrupts + r.interrupts,
                    pc: r.pc,
                    stop: r.stop,
                };
                if report.stop != StopReason::StepLimit {
                    break;
                }
            }
            if let (Some(path), Some(cov)) = (&coverage, &machine.coverage) {
                let mut rows: Vec<(u64, u32)> = cov.iter().map(|(&pc, &c)| (c, pc)).collect();
                rows.sort_unstable();
                let text: String = rows.iter().map(|(c, pc)| format!("{c} {pc:08x}\n")).collect();
                std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
            }
            machine.trace_sink = None; // flush
            print_report(&machine, &report, tail);
            save_paths(&config).flush(&mut machine)?;
            for (addr, len) in dump_mem {
                // Through SystemBus only: dumping must not touch peripherals.
                let bytes: Vec<u8> = (0..len).map(|i| ribbit_memory::Bus::read8(&mut machine.bus, addr + i)).collect();
                println!(
                    "== memory {addr:#010x}..{:#010x}
{}",
                    addr + len,
                    hexdump(addr, &bytes)
                );
            }
            if let Some(path) = dump_frame {
                let status = dump_ppm(&machine, &path)?;
                println!("frame: {status:?} -> {}", path.display());
            }
            if let Some(path) = save_state {
                std::fs::write(&path, machine.save_state()).with_context(|| format!("writing {}", path.display()))?;
                println!("state saved to {}", path.display());
            }
            Ok(match report.stop {
                StopReason::Cpu(_) => ExitCode::from(2),
                _ => ExitCode::SUCCESS,
            })
        }
        Command::Run(images) => {
            ribbit_frontend::run(start_options(&images, &config))?;
            Ok(ExitCode::SUCCESS)
        }
    }
}

fn print_report(machine: &Machine, report: &RunReport, tail: usize) {
    println!("\n== run report");
    if machine.experimental_touch_enabled() {
        println!("!! EXPERIMENTAL: a stylus was attached to the touch panel model (resistances and orientation");
        println!("!! unverified): touch results from this run are not hardware evidence.");
    }
    println!("steps: {}  instructions: {}  interrupts: {}", report.steps, report.instructions, report.interrupts);
    println!("pc: {:#010x}", report.pc);
    match &report.stop {
        StopReason::Cpu(err) => println!("stop: {err}"),
        StopReason::Breakpoint { pc } => println!("stop: breakpoint at {pc:#010x}"),
        StopReason::Halted(reason) => println!("stop: cpu halted ({reason:?})"),
        StopReason::StepLimit => println!("stop: step limit"),
    }
    let events = machine.trace_events();
    println!("\n== last {} events", tail.min(events.len()));
    for e in &events[events.len().saturating_sub(tail)..] {
        println!("{e}");
    }
    println!("\n== registers\n{:?}", machine.cpu);
    println!("== peripherals (clock: cpu {} Hz, peripheral {} Hz)", machine.clock.cpu_hz, machine.clock.peripheral_hz);
    for (n, t) in machine.io.timers.iter().enumerate() {
        println!(
            "  timer{n}: count={:#010x} control={:#x} limit={:#010x} pending={} wraps={}",
            t.count, t.control, t.limit, t.pending, t.wraps
        );
    }
    println!("  int flags={:#010x} enable={:#010x}", machine.io.intc.flags, machine.io.intc.enable);
    let taken: Vec<String> = (0..machine.interrupt_counts.len())
        .filter(|&v| machine.interrupt_counts[v] > 0)
        .map(|v| format!("{v:#04x} x{}", machine.interrupt_counts[v]))
        .collect();
    println!("  interrupts by vector since reset: {}", if taken.is_empty() { "none".into() } else { taken.join(", ") });
    let l = &machine.io.lcd;
    println!(
        "  lcd: base={:#x} stride={:#x} format={:#010x}; dma transfers={} unknown_commands={} last src={:#010x} {}x{} words",
        l.regs.fb_base,
        l.regs.stride,
        l.regs.format,
        l.transfers,
        l.unknown_commands,
        l.dma_source,
        l.dma_stride,
        l.dma_lines
    );
    let g = &machine.io.gpio;
    println!(
        "  gpio 0x0180004c: {} reads; buttons from frontend, other bits = {:#010x} (unresolved)",
        g.reads, g.unresolved_lines
    );
    let v = &machine.io.voices;
    println!(
        "  voices: playing {:#04x}, {} samples generated (8 kHz), {} LFC frames, codebook page {:#x}",
        v.status(),
        v.samples_generated,
        v.lfc.frames,
        v.codebook_page
    );
    let e = &machine.io.eeprom;
    println!(
        "  eeprom: {} reads, {} writes, changed={}, other-device commands={} (no response), last {:x?}",
        e.reads, e.writes, e.dirty, e.other_device_commands, e.last_other_command
    );
    let au = &machine.io.audio;
    println!("  audio: {} voice commands acknowledged (no playback), last {:?}", au.commands, au.last_command);
    let a = &machine.io.adc;
    println!(
        "  adc: slots {:x?} config {:#x}, {} conversions, {} queued, {} dropped, {} reads while empty",
        a.controls,
        a.config,
        a.conversions,
        a.fifo.len(),
        a.dropped,
        a.empty_reads
    );
    match &machine.io.touch {
        Some(t) => println!("  EXPERIMENTAL stylus: {:?} at t={} ms", t.pen(), t.now_ms()),
        None => println!("  stylus: none attached (panel untouched)"),
    }
    if let Some(s) = machine.io.straps {
        println!("  straps (0x01809004) = {:#010x} (LCD key {:#010x})", s.value, s.lcd_key());
    }
    if machine.io.uart.tx.is_empty() {
        println!("  debug uart: no output");
    } else {
        println!("  debug uart output ({} bytes):\n{}", machine.io.uart.tx.len(), machine.io.uart.text());
    }
    let mmio = machine.io.mmio_tail.to_vec();
    println!("== last {} peripheral accesses", tail.min(mmio.len()));
    for a in &mmio[mmio.len().saturating_sub(tail)..] {
        println!("{a}");
    }

    println!(
        "== cartridge: {} byte reads, first {}; first instruction from cart: {}",
        machine.bus.cart_reads,
        machine.bus.first_cart_read.map_or("none".into(), |(c, a)| format!("{a:#010x} at cycle {c}")),
        machine.first_cart_pc.map_or("none".into(), |(c, pc)| format!("{pc:#010x} at cycle {c}"))
    );

    let buckets: Vec<String> = machine
        .bus
        .cart_read_buckets
        .iter()
        .enumerate()
        .filter(|(_, n)| **n > 0)
        .map(|(i, n)| format!("{:#010x}:{n}", map::CART_BASE + (i as u32) * 0x1_0000))
        .collect();
    if !buckets.is_empty() {
        println!(
            "  cart reads per 64 KiB: {}; last {:?}",
            buckets.join(" "),
            machine.bus.last_cart_read.map(|(c, a)| format!("{a:#010x} at cycle {c}"))
        );
    }

    // Unmapped accesses grouped by address: in practice these are the
    // peripheral registers the firmware needs next.
    let summary = machine.bus.unmapped_summary();
    println!("== unmapped memory accesses: {} total, {} distinct", machine.bus.unmapped_total(), summary.len());
    for ((addr, width, kind), stat) in summary {
        let region = if addr.wrapping_sub(map::MMIO_BASE) < map::MMIO_SIZE { " (peripheral)" } else { "" };
        let kind = format!("{kind:?}{}", width * 8);
        println!("  {addr:#010x} {kind:<8} x{:<8} first at cycle {}{region}", stat.count, stat.first_cycle);
    }
    println!("== unmodelled peripheral registers: accessing instructions (first 16)");
    for (addr, pcs) in &machine.io.unmodelled_pcs {
        let pcs: Vec<String> = pcs.iter().map(|pc| format!("{pc:#010x}")).collect();
        println!("  {addr:#010x}: {}", pcs.join(" "));
    }
    let d = &machine.cpu.diag;
    println!("== unmodelled aux registers: {} accesses", d.aux_unmodelled_total);
    let mut aux: std::collections::BTreeMap<(u32, String), (u64, u32)> = Default::default();
    for a in &d.aux_unmodelled {
        let e = aux.entry((a.reg, format!("{:?}", a.kind))).or_insert((0, a.pc));
        e.0 += 1;
    }
    for ((reg, kind), (n, pc)) in &aux {
        let name = ribbit_cpu::isa::aux_name(*reg).unwrap_or("?");
        println!("  aux {reg:#x} ({name}) {kind} x{n} (first at pc {pc:#010x})");
    }
    if d.mul_result_writes > 0 {
        println!("== writes to MLO/MMID/MHI (read-only per PRM, performed): {}", d.mul_result_writes);
    }
}

fn save_paths(config: &Config) -> ribbit_frontend::saves::SavePaths {
    ribbit_frontend::saves::SavePaths { system: config.system_eeprom.clone(), cart: config.cart_save.clone(), ..Default::default() }
}

/// The GUI's start options from the command line and config file.
fn start_options(images: &Images, config: &Config) -> ribbit_frontend::StartOptions {
    let (firmware, cartridge) = resolve(images, config);
    ribbit_frontend::StartOptions {
        data_dir: ribbit_frontend::default_data_dir(),
        firmware,
        cartridge,
        system_eeprom: config.system_eeprom.clone(),
        cart_save: config.cart_save.clone(),
        straps: config.straps,
        experimental_touch: config.experimental_mame_touch.then_some(true),
        cpu_mhz: config.cpu_mhz,
        destroyed: None,
    }
}

fn resolve(images: &Images, config: &Config) -> (Option<PathBuf>, Option<PathBuf>) {
    (
        images.firmware.clone().or_else(|| config.firmware.clone()),
        images.cart.clone().or_else(|| config.cartridge.clone()),
    )
}

fn build_machine(images: &Images, config: &Config) -> Result<Machine> {
    let (fw, cart) = resolve(images, config);
    let fw_path = fw.context(
        "a firmware dump is required (--firmware). Leapster games are Flash content executed by the \
         console's built-in player, so a cartridge cannot boot without it.",
    )?;
    let firmware = if images.experimental_any_model {
        let data = ribbit_core::read_image(&fw_path)?;
        ribbit_memory::Firmware::from_bytes_any_model(data)?
    } else {
        load_firmware(&fw_path)?
    };
    print_firmware(&fw_path, &firmware);
    let cartridge = match cart {
        Some(path) => {
            let c = load_cartridge(&path)?;
            print_cart(&path, &c);
            Some(c)
        }
        None => None,
    };
    let straps = config.straps.unwrap_or(if firmware.is_leapster_tv() {
        ribbit_core::io::straps::LEAPSTER_TV
    } else {
        ribbit_core::io::straps::MAME_LEAPSTER2_MEASURED
    });
    let mut machine = Machine::with_straps(firmware, cartridge, straps);
    if let Some(path) = &config.system_eeprom
        && path.exists()
    {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        machine.io.eeprom.load(&bytes);
        eprintln!("system EEPROM loaded from {}", path.display());
    }
    if let Some(path) = &config.cart_save
        && path.exists()
        && machine.io.eeprom.cart.is_some()
    {
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        machine.io.eeprom.insert_cart(Some(&bytes));
        eprintln!("cartridge save loaded from {}", path.display());
    }
    if let Some(mhz) = config.cpu_mhz {
        anyhow::ensure!(mhz > 0, "--cpu-mhz must be positive");
        eprintln!("warning: CPU clock {mhz} MHz (default 96; timing is a placeholder either way)");
        machine.clock = ribbit_core::Clock::new(mhz * 1_000_000, ribbit_core::Clock::PERIPHERAL_HZ);
    }
    if let Some(lines) = config.input_lines {
        eprintln!("warning: GPIO 0x0180004C non-button bits set to {lines:#010x} (unverified test setting)");
        machine.io.gpio.unresolved_lines = lines;
    }
    if config.experimental_mame_touch {
        eprintln!("warning: EXPERIMENTAL stylus on the touch panel model (component values unverified)");
        machine.enable_experimental_mame_touch(Default::default());
    }
    Ok(machine)
}

fn print_firmware(path: &Path, fw: &Firmware) {
    println!("firmware: {}", path.display());
    println!("  {}", fw.id());
    match fw.known() {
        Some(k) if k.bad_dump => println!("  known: {} (MAME marks this dump BAD)", k.name),
        Some(k) => println!("  known: {}", k.name),
        None => println!("  known: no (unverified dump or undocumented revision)"),
    }
}

fn print_cart(path: &Path, cart: &Cartridge) {
    println!("cartridge: {}", path.display());
    println!("  {}", cart.id());
    match cart.known() {
        Some(k) => println!("  known: {}", k.name),
        None => println!("  known: no"),
    }
    match cart.header() {
        Ok(h) => println!("  header: LEAP, {} directory entries", h.directory.len()),
        Err(e) => println!("  header: {e}"),
    }
}

fn inspect_cart(cart: &Cartridge, dump: usize) {
    if let Ok(header) = cart.header() {
        println!("\nheader field 0x04: {:#06x}", header.field_04);
        for dir in &header.directory {
            println!("dir tag {:#06x} -> {:#010x} ({} entries)", dir.tag, dir.ptr, dir.entries.len());
            for e in &dir.entries {
                let text = string_at(cart.rom(), e.value);
                println!("    id {:#06x} flags {:#06x} value {:#010x}{}", e.id, e.flags, e.value, text);
            }
        }
    }
    let movies = scan_swf(cart.rom());
    println!("\nembedded SWF movies (uncompressed 'FWS'): {}", movies.len());
    for m in &movies {
        println!("  {:#010x}  v{}  {:#9x} bytes", map::CART_BASE as usize + m.offset, m.version, m.length);
    }
    if dump > 0 {
        let n = dump.min(cart.rom().len());
        println!("\n{}", hexdump(map::CART_BASE, &cart.rom()[..n]));
    }
}

/// If `ptr` points at printable text inside the cart, show it (helps
/// interpret directory entries; purely an analysis aid).
fn string_at(rom: &[u8], ptr: u32) -> String {
    let Some(offset) = ptr.checked_sub(map::CART_BASE).map(|o| o as usize) else { return String::new() };
    let Some(tail) = rom.get(offset..) else { return String::new() };
    let text: Vec<u8> = tail.iter().take(64).take_while(|b| b.is_ascii_graphic() || **b == b' ').copied().collect();
    if text.len() >= 4 { format!("  \"{}\"", String::from_utf8_lossy(&text)) } else { String::new() }
}

fn dump_ppm(machine: &Machine, path: &Path) -> Result<FrameStatus> {
    let mut frame = Box::new([0u32; WIDTH * HEIGHT]);
    let status = machine.render_frame(&mut frame);
    let mut out = format!("P6\n{WIDTH} {HEIGHT}\n255\n").into_bytes();
    for p in frame.iter() {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]);
    }
    std::fs::write(path, out).with_context(|| format!("writing {}", path.display()))?;
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parsing() {
        assert_eq!(parse_hex("0x4000_1234"), Ok(0x4000_1234));
        assert_eq!(parse_hex("80000000"), Ok(0x8000_0000));
        assert!(parse_hex("zz").is_err());
    }

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }
}
