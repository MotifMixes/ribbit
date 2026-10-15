//! The Leapster machine, independent of any frontend.
//!
//! STATUS: the CPU executes the implemented ARCompact subset (see
//! ribbit-cpu); peripherals at 0x0180_xxxx are not modelled yet, so firmware
//! runs until it depends on hardware that Ribbit does not provide. Ribbit does
//! not emulate the Leapster as a whole.

pub mod config;
pub mod hd;
pub mod io;
pub mod zip;

use ribbit_cpu::isa::Instr;
use ribbit_cpu::{Cpu, CpuConfig, CpuError, HaltReason, Step};
use ribbit_debug::{Breakpoints, TraceRing};
use ribbit_input::Buttons;
use ribbit_memory::{Cartridge, CartridgeError, Firmware, FirmwareError, SystemBus, map};
use ribbit_video::{FrameStatus, HEIGHT, WIDTH, decode_frame};
use std::io::Write as _;
use std::path::{Path, PathBuf};

pub use config::Config;
pub use io::{Clock, MachineBus, Peripherals};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    // The source error is reported by the error chain, not repeated here.
    #[error("reading {path}")]
    Io { path: PathBuf, source: std::io::Error },
    #[error("{path}")]
    Firmware { path: PathBuf, source: FirmwareError },
    #[error("{path}")]
    Cartridge { path: PathBuf, source: CartridgeError },
    #[error("{path}")]
    Zip { path: PathBuf, source: zip::ZipError },
}

/// A file's bytes, or the ROM inside it if it is a .zip archive.
pub fn read_image(path: impl AsRef<Path>) -> Result<Vec<u8>, LoadError> {
    let path = path.as_ref();
    let data = std::fs::read(path).map_err(|source| LoadError::Io { path: path.into(), source })?;
    zip::unzip_image(data).map_err(|source| LoadError::Zip { path: path.into(), source })
}

pub fn load_firmware(path: &Path) -> Result<Firmware, LoadError> {
    let data = read_image(path)?;
    Firmware::from_bytes(data).map_err(|source| LoadError::Firmware { path: path.into(), source })
}

pub fn load_cartridge(path: &Path) -> Result<Cartridge, LoadError> {
    let data = read_image(path)?;
    Cartridge::from_bytes(data).map_err(|source| LoadError::Cartridge { path: path.into(), source })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StopReason {
    /// The step budget was used up.
    StepLimit,
    Breakpoint {
        pc: u32,
    },
    /// The program halted the CPU (FLAG 1 or BRK).
    Halted(HaltReason),
    /// The CPU could not continue (unsupported/illegal/unpredictable).
    Cpu(CpuError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunReport {
    /// Successful calls to `Cpu::step` (instructions, interrupt entries,
    /// sleep cycles).
    pub steps: u64,
    pub instructions: u64,
    pub interrupts: u64,
    pub pc: u32,
    pub stop: StopReason,
}

/// One entry of the execution trace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceEvent {
    Instr { cycle: u64, instr: Instr },
    Interrupt { cycle: u64, vector: u8, from: u32 },
}

impl std::fmt::Display for TraceEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TraceEvent::Instr { cycle, instr } => {
                write!(f, "{cycle:>10} {:08x}: {}", instr.addr, ribbit_cpu::disasm::disassemble(instr))
            }
            TraceEvent::Interrupt { cycle, vector, from } => {
                write!(f, "{cycle:>10} -------- interrupt vector {vector} taken (return to {from:#010x})")
            }
        }
    }
}

/// Events kept for crash reports.
pub const TRACE_TAIL: usize = 64;

/// Save-state format: magic, version, then a deflated bincode payload.
/// An uncompressed machine state from `Machine::snapshot`.
pub struct RawState(Vec<u8>);

impl RawState {
    /// The `save_state` format (header + deflate).
    pub fn compress(self) -> Vec<u8> {
        let mut out = STATE_MAGIC.to_vec();
        out.extend_from_slice(&STATE_VERSION.to_le_bytes());
        out.extend_from_slice(&miniz_oxide::deflate::compress_to_vec(&self.0, 1));
        out
    }
}

const STATE_MAGIC: &[u8; 8] = b"RIBBITST";
pub const STATE_VERSION: u32 = 1;

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("not a Ribbit save state")]
    NotAState,
    #[error("save state version {0} is not supported (this build reads version {STATE_VERSION})")]
    Version(u32),
    #[error("this save state is for a different BIOS or cartridge")]
    WrongImages,
    #[error("damaged save state: {0}")]
    Damaged(String),
}

#[derive(serde::Serialize)]
struct StateRef<'a> {
    bios_crc: u32,
    cart_crc: Option<u32>,
    cpu: &'a Cpu,
    ram: &'a [u8],
    vram: &'a [u8],
    buttons: &'a Buttons,
    io: &'a Peripherals,
    clock: &'a Clock,
    interrupt_counts: &'a [u64],
    first_cart_pc: Option<(u64, u32)>,
    touch_config: &'a Option<io::touch::MameTouchConfig>,
}

#[derive(serde::Deserialize)]
struct State {
    bios_crc: u32,
    cart_crc: Option<u32>,
    cpu: Cpu,
    ram: Vec<u8>,
    vram: Vec<u8>,
    buttons: Buttons,
    io: Peripherals,
    clock: Clock,
    interrupt_counts: Vec<u64>,
    first_cart_pc: Option<(u64, u32)>,
    touch_config: Option<io::touch::MameTouchConfig>,
}

const IDLE_PROBE_MAX_STEPS: u64 = 512;
const IDLE_BACKOFF_STEPS: u64 = 20_000;
/// A loop head whose probes keep failing (a busy loop, not an idle one) is
/// left alone for twice as long after each failure, up to this many
/// doublings (20,000 << 6 = 1.28 M steps, ~13 ms at 96 MHz).
const IDLE_BACKOFF_MAX_SHIFT: u8 = 6;

/// Hashes loop-head addresses (speed only: the default SipHash is costly on
/// the hot path, and the keys are not attacker-controlled).
#[derive(Default, Clone, Copy)]
struct PcHasher(u64);

impl std::hash::Hasher for PcHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 ^ b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u32(&mut self, n: u32) {
        self.0 = (n as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 16;
    }
}

#[derive(Default)]
struct IdleTracker {
    /// Recent backward-jump targets; a repeat marks a loop.
    recent: [u32; 8],
    recent_next: usize,
    probe: Option<IdleProbe>,
    /// Loop heads that failed a probe: the step count before which they are
    /// not probed again, and how many times in a row they failed.
    backoff: std::collections::HashMap<u32, (u64, u8), std::hash::BuildHasherDefault<PcHasher>>,
    /// The probe write log's storage, kept between probes.
    log_buf: Vec<(u32, u8)>,
    skipped_steps: u64,
    /// Probe outcomes, for diagnosis: started, interrupted/violated/too long,
    /// state differed, a device event happened, memory differed, no room, skipped.
    pub stats: [u64; 7],
}

impl IdleTracker {
    /// After a backward jump to `pc` during a batch: true if `idle_track`
    /// would start a probe here (the same bookkeeping, done inline).
    #[inline]
    fn watch(&mut self, pc: u32, cycles: u64) -> bool {
        if !self.recent.contains(&pc) {
            self.recent[self.recent_next] = pc;
            self.recent_next = (self.recent_next + 1) % self.recent.len();
            return false;
        }
        !self.backoff.get(&pc).is_some_and(|&(until, _)| cycles < until)
    }

    /// A probe of `head` failed at `cycles`.
    fn back_off(&mut self, head: u32, cycles: u64) {
        let e = self.backoff.entry(head).or_insert((0, 0));
        // A failure long after the last one starts over.
        let shift = if cycles > e.0 + (IDLE_BACKOFF_STEPS << IDLE_BACKOFF_MAX_SHIFT) {
            0
        } else {
            (e.1 + 1).min(IDLE_BACKOFF_MAX_SHIFT)
        };
        *e = (cycles + (IDLE_BACKOFF_STEPS << shift), shift);
    }
}

struct IdleProbe {
    head: u32,
    snapshot: ribbit_cpu::idle::IdleSnapshot,
    cycles: u64,
    retired: u64,
    steps: u64,
    event_syncs: u64,
}

/// Compact form of a `TraceEvent` for the always-on trace tail (recorded
/// every step). An instruction is kept as its raw halfwords and re-decoded,
/// exactly, only when the tail is displayed: decoding is a pure function of
/// those bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawEvent {
    Instr { cycle: u64, addr: u32, word: u32, limm: Option<u32>, short: bool },
    Interrupt { cycle: u64, vector: u8, from: u32 },
}

impl RawEvent {
    #[inline]
    fn instr(cycle: u64, i: &ribbit_cpu::isa::Instr) -> Self {
        RawEvent::Instr { cycle, addr: i.addr, word: i.word, limm: i.limm, short: i.short }
    }

    pub fn expand(&self) -> TraceEvent {
        match *self {
            RawEvent::Instr { cycle, addr, word, limm, short } => {
                let mut halfwords = [0u16; 4];
                let n = if short {
                    halfwords[0] = word as u16;
                    1
                } else {
                    halfwords[0] = (word >> 16) as u16;
                    halfwords[1] = word as u16;
                    2
                };
                if let Some(limm) = limm {
                    halfwords[n] = (limm >> 16) as u16;
                    halfwords[n + 1] = limm as u16;
                }
                let instr = ribbit_cpu::decode::decode_with(addr, |a| {
                    halfwords.get((a.wrapping_sub(addr) / 2) as usize).copied().unwrap_or(0)
                });
                TraceEvent::Instr { cycle, instr }
            }
            RawEvent::Interrupt { cycle, vector, from } => TraceEvent::Interrupt { cycle, vector, from },
        }
    }
}

pub struct Machine {
    pub cpu: Cpu,
    pub bus: SystemBus,
    pub buttons: Buttons,
    /// The most recent events, always recorded.
    pub trace_tail: TraceRing<RawEvent>,
    /// Record every instruction in `trace_tail` (the "last events" shown
    /// when a run stops). Costs ~10% speed; the GUI turns it off. Tracing to
    /// a sink records regardless.
    pub record_trace_tail: bool,
    /// Optional full trace output (one line per event, peripheral accesses
    /// included).
    pub trace_sink: Option<Box<dyn std::io::Write + Send>>,
    pub io: Peripherals,
    pub clock: Clock,
    /// Interrupts taken since reset, per vector.
    pub interrupt_counts: [u64; 64],
    /// Set by `enable_experimental_mame_touch`; survives `reset`.
    touch_config: Option<io::touch::MameTouchConfig>,
    /// First (cycle, pc) at which the CPU executed from the cartridge window.
    pub first_cart_pc: Option<(u64, u32)>,
    /// Exact idle-loop fast-forwarding (on by default; `idle_skip = false`
    /// runs every iteration, for verification).
    pub idle_skip: bool,
    idle: IdleTracker,
    /// Speed diagnostics: batches run, instructions run batched, and single
    /// steps (by `run_batch` result: not attempted, budget too small, block
    /// stopped at its first instruction).
    pub batch_stats: [u64; 5],
    /// Speed diagnostics: batch exits by reason (budget, slow, bus, backward, error).
    pub batch_exits: [u64; 5],
    /// Debug option: first cycle at which each PC was executed (None = off).
    pub coverage: Option<std::collections::HashMap<u32, u64>>,
    /// EXPERIMENTAL: HD capture from the BIOS's Flash rasterizer (hd.rs).
    pub hd: Option<hd::HdCapture>,
    firmware: Firmware,
    cartridge: Option<Cartridge>,
}

impl Machine {
    pub fn new(firmware: Firmware, cartridge: Option<Cartridge>) -> Self {
        Self::with_straps(firmware, cartridge, io::straps::MAME_LEAPSTER2_MEASURED)
    }

    /// `straps`: board configuration (0x0180_9004); bit 26 is overridden by
    /// cartridge presence.
    pub fn with_straps(firmware: Firmware, cartridge: Option<Cartridge>, straps: u32) -> Self {
        let bus = SystemBus::new(&firmware, cartridge.as_ref());
        let mut io = Peripherals::default();
        io.straps = Some(io::straps::Straps::new(straps, cartridge.is_some()));
        if cartridge.is_some() {
            io.eeprom.insert_cart(None);
        }
        let mut cpu = Cpu::new(CpuConfig::leapster());
        // ROM contents never change (writes are discarded and reported), so
        // decoded instructions there can be cached. RAM/VRAM are not cached.
        let bios_len = firmware.data().len() as u32;
        let mut rom = vec![
            (map::BIOS_BASE, map::BIOS_BASE + bios_len.min(map::BIOS_WINDOW)),
            (map::BIOS_MIRROR, map::BIOS_MIRROR + bios_len.min(map::BIOS_WINDOW)),
        ];
        if let Some(c) = &cartridge {
            rom.push((map::CART_BASE, map::CART_BASE.saturating_add(c.rom().len() as u32)));
        }
        cpu.icache.set_rom_ranges(&rom);
        let mut m = Self {
            cpu,
            bus,
            buttons: Buttons::default(),
            trace_tail: TraceRing::new(TRACE_TAIL),
            record_trace_tail: true,
            trace_sink: None,
            io,
            clock: Clock::default(),
            interrupt_counts: [0; 64],
            touch_config: None,
            first_cart_pc: None,
            idle_skip: true,
            idle: IdleTracker::default(),
            batch_stats: [0; 5],
            batch_exits: [0; 5],
            coverage: None,
            hd: None,
            firmware,
            cartridge,
        };
        if m.firmware.is_leapster_tv() {
            m.enable_leapster_tv();
        }
        m
    }

    /// EXPERIMENTAL, UNVERIFIED: feeds ADC channel 0 with MAME's guessed
    /// touchscreen samples (io/touch.rs). Empty `presses` = stylus never down.
    /// Off unless called; every report produced with it on must say so.
    pub fn enable_experimental_mame_touch(&mut self, config: io::touch::MameTouchConfig) {
        tracing::warn!(target: "ribbit::boot", "EXPERIMENTAL: MAME-derived touchscreen/ADC model enabled (unverified, not hardware behaviour)");
        self.touch_config = Some(config);
        self.install_touch();
    }

    /// EXPERIMENTAL: Leapster TV hardware: the TV video chip (the picture
    /// comes from it, not the LCD) and the controller link. Done by
    /// `with_straps` for a known Leapster TV BIOS. The stylus reaches the
    /// controller's pen through `io.touch.live`, so touch input is enabled.
    pub fn enable_leapster_tv(&mut self) {
        tracing::warn!(target: "ribbit::boot", "EXPERIMENTAL: Leapster TV video chip and controller link");
        if self.touch_config.is_none() {
            self.touch_config = Some(Default::default());
            self.install_touch();
        }
        self.install_leapster_tv();
    }

    fn install_leapster_tv(&mut self) {
        self.io.lmax_tv = Some(Default::default());
        self.io.tv_link = Some(Default::default());
    }

    /// The TV video chip and controller link are present (Leapster TV).
    pub fn is_leapster_tv(&self) -> bool {
        self.io.tv_link.is_some()
    }

    /// EXPERIMENTAL: starts recording the BIOS's Flash edges for HD
    /// rendering (hd.rs). False if this BIOS's rasterizer is not recognised.
    /// Observes only: emulation results are unchanged.
    pub fn enable_hd_capture(&mut self) -> bool {
        let Some(hooks) = hd::Hooks::find(self.firmware.data(), ribbit_memory::map::BIOS_MIRROR) else {
            return false;
        };
        self.cpu.icache.set_stops(&[hooks.edge_ctor, hooks.scanline]);
        self.hd = Some(hd::HdCapture::new(hooks));
        true
    }

    /// Stops HD capture (see `enable_hd_capture`).
    pub fn disable_hd_capture(&mut self) {
        self.hd = None;
        self.cpu.icache.set_stops(&[]);
    }

    pub fn experimental_touch_enabled(&self) -> bool {
        self.touch_config.is_some()
    }

    fn install_touch(&mut self) {
        self.io.touch = self.touch_config.clone().map(|c| io::touch::MameTouch::new(c, self.clock.peripheral_hz));
        self.io.sync_needed = true;
    }

    pub fn firmware(&self) -> &Firmware {
        &self.firmware
    }

    pub fn cartridge(&self) -> Option<&Cartridge> {
        self.cartridge.as_ref()
    }

    /// Power-on reset: rebuilds RAM, VRAM and all device state.
    pub fn reset(&mut self) {
        self.bus = SystemBus::new(&self.firmware, self.cartridge.as_ref());
        self.cpu.reset();
        self.trace_tail = TraceRing::new(TRACE_TAIL);
        self.interrupt_counts = [0; 64];
        self.first_cart_pc = None;
        self.idle = IdleTracker::default();
        let (straps, lines) = (self.io.straps, self.io.gpio.unresolved_lines);
        // Non-volatile: the EEPROM keeps its contents across a power cycle.
        let eeprom = std::mem::take(&mut self.io.eeprom.data);
        let cart_save = self.io.eeprom.cart.take();
        self.io = Peripherals::default();
        self.io.straps = straps;
        self.io.gpio.unresolved_lines = lines;
        self.io.eeprom.load(&eeprom);
        if let Some(save) = cart_save {
            self.io.eeprom.insert_cart(Some(&save));
        }
        self.install_touch();
        if self.firmware.is_leapster_tv() {
            self.install_leapster_tv();
        }
        self.clock = Clock::new(self.clock.cpu_hz, self.clock.peripheral_hz);
    }

    /// One CPU step through the machine bus, then peripheral time and
    /// interrupt lines catch up with the cycles the step took.
    fn step(&mut self) -> Result<Step, CpuError> {
        let start = self.cpu.cycles;
        self.bus.now = start;
        if let Some(cov) = &mut self.coverage {
            cov.entry(self.cpu.pc).or_insert(start);
        }
        if self.cpu.pc >= ribbit_memory::map::CART_BASE && self.first_cart_pc.is_none() {
            self.first_cart_pc = Some((start, self.cpu.pc));
        }
        let mut bus = MachineBus::new(&mut self.bus, &mut self.io, start, self.cpu.pc);
        let result = self.cpu.step(&mut bus);
        if let Some(dma) = self.io.lcd.pending.take() {
            self.run_framebuffer_dma(dma);
            self.io.sync_needed = true;
        }
        let ticks = self.clock.ticks(self.cpu.cycles - start);
        // Voice engine: needs memory (speech stream, samples), so it runs
        // here, after the instruction, like the framebuffer DMA.
        if self.io.voices.lfc_start_pending {
            self.io.voices.start_lfc(&mut self.bus);
        }
        // (Checked here too: `advance` is out of line and most steps at
        // 96:16 MHz have no tick.)
        if ticks > 0 {
            self.io.voices.advance(ticks, self.clock.peripheral_hz, &mut self.bus);
        }
        if self.cpu.halted() {
            // Earlier deferred ticks ran with the CPU active; this step's not.
            self.io.catch_up();
            self.io.advance(ticks, true);
            self.io.sync_needed = true;
        } else {
            self.io.deferred += ticks;
        }
        // Devices only change interrupt lines at an event or after a register
        // access, so outside those the per-step update can be skipped.
        if self.io.sync_needed || self.io.deferred >= self.io.horizon {
            if self.io.deferred >= self.io.horizon {
                self.io.event_syncs += 1;
            }
            self.io.catch_up();
            self.cpu.set_irq_lines(Peripherals::irq_mask(), self.io.irq_levels());
            self.io.horizon = self.io.next_event();
            self.io.sync_needed = false;
        }
        result
    }

    /// Records `Cpu::last_instr` (the instruction just executed).
    fn record_instr(&mut self, cycle: u64) {
        let Some(instr) = self.cpu.last_instr.as_ref() else {
            return;
        };
        if let Some(sink) = &mut self.trace_sink {
            let _ = writeln!(sink, "{}", TraceEvent::Instr { cycle, instr: *instr });
        }
        self.trace_tail.push(RawEvent::instr(cycle, instr));
    }

    fn record_interrupt(&mut self, cycle: u64, vector: u8, from: u32) {
        let raw = RawEvent::Interrupt { cycle, vector, from };
        if let Some(sink) = &mut self.trace_sink {
            let _ = writeln!(sink, "{}", raw.expand());
        }
        self.trace_tail.push(raw);
    }

    /// The most recent events, oldest first.
    pub fn trace_events(&self) -> Vec<TraceEvent> {
        self.trace_tail.to_vec().iter().map(RawEvent::expand).collect()
    }

    /// Runs for at most `max_steps` CPU steps, stopping early at a breakpoint
    /// (checked before every step after the first) or when the CPU halts or
    /// errors. Deterministic: identical inputs give identical results.
    pub fn run(&mut self, max_steps: u64, breakpoints: &Breakpoints) -> RunReport {
        let mut report =
            RunReport { steps: 0, instructions: 0, interrupts: 0, pc: self.cpu.pc, stop: StopReason::StepLimit };
        self.io.record_events = self.trace_sink.is_some();
        self.cpu.record_last = self.record_trace_tail || self.trace_sink.is_some();
        self.io.gpio.buttons = self.buttons;
        if let Some(l) = self.io.tv_link.as_mut() {
            l.pressed = !self.buttons.gio_data_in();
            // The stylus, as frontends give it (MAME touch units, 0..=0x7F0 per
            // 160 pixels), becomes the pen on the TV controller's pad.
            let live = self.io.touch.as_ref().and_then(|t| t.live);
            l.pen = live.map(|(x, y)| {
                let px = |r: u32| ((r * 159 + 0x3F8) / 0x7F0).min(159) as u16;
                (px(x), px(y))
            });
        }
        // Device state may have been changed from outside since the last run.
        self.io.sync_needed = true;
        // Batches need nothing observed per instruction.
        let batch = breakpoints.is_empty() && self.trace_sink.is_none() && self.coverage.is_none() && !self.cpu.record_last;
        while report.steps < max_steps {
            if report.steps > 0 && breakpoints.hit(self.cpu.pc) {
                report.stop = StopReason::Breakpoint { pc: self.cpu.pc };
                break;
            }
            if !(batch && !self.io.sync_needed) {
                self.batch_stats[2] += 1;
            } else {
                match self.run_batch(max_steps, &mut report) {
                    None => {}
                    Some(Ok(())) => continue,
                    Some(Err(err)) => {
                        tracing::info!(target: "ribbit::boot", pc = format_args!("{:#010x}", self.cpu.pc), %err, "cpu stopped");
                        report.stop = StopReason::Cpu(err);
                        break;
                    }
                }
            }
            let cycle = self.cpu.cycles;
            let from = self.cpu.pc;
            let result = self.step();
            if let Some(hd) = &mut self.hd
                && matches!(result, Ok(Step::Executed))
            {
                let bus = &self.bus;
                let peek = |a: u32| bus.peek8(a).unwrap_or(0);
                if from == hd.hooks.edge_ctor {
                    hd.edge(self.cpu.regs[1], peek);
                } else if from == hd.hooks.scanline {
                    hd.scanline(self.cpu.regs[14], self.cpu.regs[17] as i32, peek);
                }
            }
            // `idle_track` only acts while probing or after a backward jump;
            // checking that here saves an out-of-line call on most steps.
            if self.idle_skip
                && breakpoints.is_empty()
                && self.trace_sink.is_none()
                && (self.idle.probe.is_some() || self.cpu.pc <= from)
            {
                self.idle_track(matches!(result, Ok(Step::Executed)), from, max_steps, &mut report);
            }
            if let Some(sink) = &mut self.trace_sink {
                for a in self.io.take_events() {
                    let _ = writeln!(sink, "{a}");
                }
            }
            match result {
                Ok(Step::Executed) => {
                    report.instructions += 1;
                    if self.cpu.record_last {
                        self.record_instr(cycle);
                    }
                }
                Ok(Step::Interrupt { vector }) => {
                    report.interrupts += 1;
                    self.interrupt_counts[vector as usize & 63] += 1;
                    tracing::debug!(target: "ribbit::irq", vector, from = format_args!("{from:#010x}"), "interrupt");
                    self.record_interrupt(cycle, vector, from);
                }
                Ok(Step::Sleeping) => {}
                Ok(Step::Halted(reason)) => {
                    report.steps += 1;
                    report.instructions += 1;
                    self.record_instr(cycle);
                    report.stop = StopReason::Halted(reason);
                    break;
                }
                Err(err) => {
                    tracing::info!(target: "ribbit::boot", pc = format_args!("{:#010x}", self.cpu.pc), %err, "cpu stopped");
                    report.stop = StopReason::Cpu(err);
                    break;
                }
            }
            report.steps += 1;
        }
        // Leave every device exactly up to date for observers.
        self.io.catch_up();
        report.pc = self.cpu.pc;
        report
    }

    /// Batched execution (`Cpu::run_block`): instructions back to back up
    /// to the next device event, voice sample, processor-timer event or the
    /// step budget, or until an instruction needs `step`. Identical results
    /// to stepping: devices only act at events and register accesses, and a
    /// register access ends the batch after its instruction, which then gets
    /// the same after-step handling as in `step`. None if nothing could run
    /// batched (the caller steps instead).
    fn run_batch(&mut self, max_steps: u64, report: &mut RunReport) -> Option<Result<(), CpuError>> {
        let to_event = self.io.horizon.saturating_sub(self.io.deferred);
        let mut budget = self.clock.max_cycles_below(to_event).min(max_steps - report.steps);
        let to_sample = self.io.voices.ticks_to_next_sample(self.clock.peripheral_hz);
        if to_sample != u64::MAX {
            budget = budget.min(self.clock.max_cycles_below(to_sample));
        }
        // While an idle-loop probe runs: stop on returning to the loop head,
        // and within the probe's step limit (`idle_track` then checks the
        // last step as it would after a single step).
        let probe_head = self.idle.probe.as_ref().map(|p| {
            budget = budget.min((IDLE_PROBE_MAX_STEPS + 1).saturating_sub(p.steps));
            p.head
        });
        if budget < 2 {
            self.batch_stats[3] += 1;
            return None;
        }
        let start = self.cpu.cycles;
        let fence = if self.first_cart_pc.is_none() { ribbit_memory::map::CART_BASE } else { u32::MAX };
        let (idle, skip) = (&mut self.idle, self.idle_skip);
        let mut bus = MachineBus {
            sys: &mut self.bus,
            io: &mut self.io,
            cycle: start,
            pc: self.cpu.pc,
            clock: Some(&mut self.clock),
            synced: start,
        };
        let block = self.cpu.run_block(&mut bus, &mut ribbit_cpu::NoAuxExt, budget, fence, |pc, cycles| match probe_head {
            Some(head) => pc == head,
            None => skip && idle.watch(pc, cycles),
        });
        let synced = bus.synced;
        if block.executed == 0 {
            self.batch_stats[4] += 1;
            return match block.exit {
                ribbit_cpu::BlockExit::Error(e) => Some(Err(e)),
                _ => None,
            };
        }
        // The last instruction's after-step handling, as in `step` (earlier
        // ones had nothing to do but time, accounted here or at a register
        // access).
        if let Some(dma) = self.io.lcd.pending.take() {
            self.run_framebuffer_dma(dma);
            self.io.sync_needed = true;
        }
        let ticks = self.clock.ticks(self.cpu.cycles - synced);
        if self.io.voices.lfc_start_pending {
            self.io.voices.start_lfc(&mut self.bus);
        }
        if ticks > 0 {
            self.io.voices.advance(ticks, self.clock.peripheral_hz, &mut self.bus);
        }
        self.io.deferred += ticks;
        if self.io.sync_needed || self.io.deferred >= self.io.horizon {
            if self.io.deferred >= self.io.horizon {
                self.io.event_syncs += 1;
            }
            self.io.catch_up();
            self.cpu.set_irq_lines(Peripherals::irq_mask(), self.io.irq_levels());
            self.io.horizon = self.io.next_event();
            self.io.sync_needed = false;
        }
        self.batch_stats[0] += 1;
        self.batch_exits[match block.exit {
            ribbit_cpu::BlockExit::Budget => 0,
            ribbit_cpu::BlockExit::Slow => 1,
            ribbit_cpu::BlockExit::Bus => 2,
            ribbit_cpu::BlockExit::Backward => 3,
            ribbit_cpu::BlockExit::Error(_) => 4,
        }] += 1;
        self.batch_stats[1] += block.executed;
        report.steps += block.executed - 1;
        report.instructions += block.executed;
        if let Some(p) = &mut self.idle.probe {
            // Steps before the last: none reached the head, touched a device
            // register or exceeded the limit (each would have ended the block).
            p.steps += block.executed - 1;
        }
        if self.idle.probe.is_some() || matches!(block.exit, ribbit_cpu::BlockExit::Backward) {
            self.idle_track(true, block.from, max_steps, report);
        }
        report.steps += 1;
        match block.exit {
            ribbit_cpu::BlockExit::Error(e) => Some(Err(e)),
            _ => Some(Ok(())),
        }
    }

    /// Idle-loop fast-forwarding, exact. After a backward jump to the same
    /// address twice in a row, one iteration is run as a probe; if it leaves
    /// the CPU state (except time), memory and devices exactly as they were,
    /// and touched only side-effect-free registers, the following iterations
    /// are identical until the next device or CPU-timer event, so time is
    /// advanced to just before that event in one go. Verified by comparing
    /// state digests with `idle_skip` off.
    /// `executed`: the step was a plain instruction (no interrupt entry, sleep,
    /// halt or error). Takes a bool, not the result, so the hot loop keeps the
    /// result in registers.
    fn idle_track(&mut self, executed: bool, from: u32, max_steps: u64, report: &mut RunReport) {
        let pc = self.cpu.pc;
        if let Some(p) = &mut self.idle.probe {
            p.steps += 1;
            let interrupted = !executed;
            if interrupted || self.io.probe_violation || p.steps > IDLE_PROBE_MAX_STEPS {
                self.idle.stats[1] += 1;
                self.idle_end_probe(false);
                return;
            }
            if pc != p.head {
                return;
            }
            let p = self.idle.probe.take().unwrap();
            let exact = self.cpu.idle_snapshot() == p.snapshot
                && self.io.event_syncs == p.event_syncs
                && self
                    .bus
                    .write_log
                    .as_ref()
                    .is_some_and(|log| log.iter().all(|&(a, old)| self.bus.peek8(a) == Some(old)));
            self.io.probe = false;
            self.release_write_log();
            if !exact {
                let snap = self.cpu.idle_snapshot() == p.snapshot;
                let ev = self.io.event_syncs == p.event_syncs;
                self.idle.stats[if !snap {
                    2
                } else if !ev {
                    3
                } else {
                    4
                }] += 1;
                self.idle.back_off(p.head, self.cpu.cycles);
                return;
            }
            self.idle.backoff.remove(&p.head);
            // Every step (instruction, interrupt entry, sleep) is one cycle,
            // so the absolute cycle count measures the iteration even when
            // the probe spans several `run` calls.
            let cycles = self.cpu.cycles - p.cycles;
            let steps = cycles;
            let instructions = self.cpu.instructions_retired - p.retired;
            if cycles == 0 || steps == 0 {
                return;
            }
            // Largest number of iterations that ends strictly before the next
            // device event, CPU-timer event and the step budget.
            let to_event = self.io.horizon.saturating_sub(self.io.deferred);
            let budget_cycles = self.clock.max_cycles_below(to_event).min(self.cpu.cycles_to_timer_event());
            let budget_steps = max_steps.saturating_sub(report.steps + 2);
            let k = (budget_cycles / cycles).min(budget_steps / steps);
            if k == 0 {
                self.idle.stats[5] += 1;
                return;
            }
            self.idle.stats[6] += 1;
            let (skip_cycles, skip_steps) = (k * cycles, k * steps);
            self.cpu.skip_idle(skip_cycles, k * instructions);
            let ticks = self.clock.ticks(skip_cycles);
            self.io.deferred += ticks;
            self.io.voices.advance(ticks, self.clock.peripheral_hz, &mut self.bus);
            report.steps += skip_steps;
            report.instructions += k * instructions;
            self.idle.skipped_steps += skip_steps;
            return;
        }
        let backward = executed && pc <= from && self.cpu.delay_target.is_none();
        if !backward {
            return;
        }
        if !self.idle.recent.contains(&pc) {
            self.idle.recent[self.idle.recent_next] = pc;
            self.idle.recent_next = (self.idle.recent_next + 1) % self.idle.recent.len();
            return;
        }
        if self.idle.backoff.get(&pc).is_some_and(|&(until, _)| self.cpu.cycles < until) || self.io.sync_needed {
            return;
        }
        self.idle.stats[0] += 1;
        self.idle.probe = Some(IdleProbe {
            head: pc,
            snapshot: self.cpu.idle_snapshot(),
            cycles: self.cpu.cycles,
            retired: self.cpu.instructions_retired,
            steps: 0,
            event_syncs: self.io.event_syncs,
        });
        self.io.probe = true;
        self.io.probe_violation = false;
        self.bus.write_log = Some(std::mem::take(&mut self.idle.log_buf));
    }

    fn idle_end_probe(&mut self, _ok: bool) {
        if let Some(p) = self.idle.probe.take() {
            self.idle.back_off(p.head, self.cpu.cycles);
        }
        self.io.probe = false;
        self.release_write_log();
    }

    /// Ends write logging, keeping the buffer for the next probe.
    fn release_write_log(&mut self) {
        if let Some(mut log) = self.bus.write_log.take() {
            log.clear();
            self.idle.log_buf = log;
        }
    }

    /// The complete machine state (CPU, memory, devices, time), compressed.
    /// ROMs are not included: a state loads only into a machine with the
    /// same BIOS and cartridge.
    /// The machine state, serialised but not yet compressed: the fast part of
    /// `save_state`, so a caller holding the machine can compress elsewhere.
    pub fn snapshot(&self) -> RawState {
        let state = StateRef {
            bios_crc: self.firmware.id().crc32,
            cart_crc: self.cartridge.as_ref().map(|c| c.id().crc32),
            cpu: &self.cpu,
            ram: self.bus.ram(),
            vram: self.bus.vram(),
            buttons: &self.buttons,
            io: &self.io,
            clock: &self.clock,
            interrupt_counts: &self.interrupt_counts,
            first_cart_pc: self.first_cart_pc,
            touch_config: &self.touch_config,
        };
        let payload = bincode::serialize(&state).expect("in-memory serialisation cannot fail");
        RawState(payload)
    }

    /// The complete machine state, compressed (see `snapshot`).
    pub fn save_state(&self) -> Vec<u8> {
        self.snapshot().compress()
    }

    /// Restores a state made by `save_state` on the same BIOS and cartridge.
    /// On error the machine is unchanged.
    pub fn load_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let body = bytes.strip_prefix(STATE_MAGIC.as_slice()).ok_or(StateError::NotAState)?;
        let version = u32::from_le_bytes(body.get(..4).ok_or(StateError::NotAState)?.try_into().unwrap());
        if version != STATE_VERSION {
            return Err(StateError::Version(version));
        }
        let payload =
            miniz_oxide::inflate::decompress_to_vec(&body[4..]).map_err(|e| StateError::Damaged(format!("{e:?}")))?;
        let s: State = bincode::deserialize(&payload).map_err(|e| StateError::Damaged(e.to_string()))?;
        if s.bios_crc != self.firmware.id().crc32 || s.cart_crc != self.cartridge.as_ref().map(|c| c.id().crc32) {
            return Err(StateError::WrongImages);
        }
        if s.interrupt_counts.len() != self.interrupt_counts.len() {
            return Err(StateError::Damaged("interrupt counters".into()));
        }
        if !self.bus.restore_memory(&s.ram, &s.vram) {
            return Err(StateError::Damaged("memory size".into()));
        }
        // The ROM is the same, so the decode cache stays valid.
        let icache = std::mem::take(&mut self.cpu.icache);
        self.cpu = s.cpu;
        self.cpu.icache = icache;
        self.buttons = s.buttons;
        self.io = s.io;
        self.io.sync_needed = true;
        self.clock = s.clock;
        self.interrupt_counts.copy_from_slice(&s.interrupt_counts);
        self.first_cart_pc = s.first_cart_pc;
        self.touch_config = s.touch_config;
        self.idle = IdleTracker::default();
        self.trace_tail = TraceRing::new(TRACE_TAIL);
        Ok(())
    }

    /// Steps skipped by idle fast-forwarding since reset.
    pub fn idle_skipped_steps(&self) -> u64 {
        self.idle.skipped_steps
    }

    /// Idle probe outcomes (see `IdleTracker::stats`).
    pub fn idle_stats(&self) -> [u64; 7] {
        self.idle.stats
    }

    /// Framebuffer DMA: copies `words` words, then signals completion through
    /// interrupt flag bit 14 (CPU line 0x12). Completes instantly; the real
    /// transfer time is UNKNOWN. See io/lcd.rs.
    fn run_framebuffer_dma(&mut self, dma: io::lcd::DmaRequest) {
        use ribbit_memory::Bus as _;
        tracing::debug!(target: "ribbit::mmio", ?dma, "framebuffer dma");
        for i in 0..dma.words {
            let v = self.bus.read32(dma.source.wrapping_add(4 * i));
            self.bus.write32(dma.dest.wrapping_add(4 * i), v);
        }
        self.io.lcd.transfers += 1;
        if let Some(hd) = &mut self.hd {
            hd.end_pass();
        }
        self.io.intc.raise(io::intc::FLAG_DMA);
    }

    /// Decodes the current LCD contents from emulated VRAM.
    pub fn render_frame(&self, out: &mut [u32; WIDTH * HEIGHT]) -> FrameStatus {
        // EXPERIMENTAL: a TV model shows the TV chip's picture, not the LCD.
        if let Some(tv) = &self.io.lmax_tv {
            tv.render_game(out);
            return FrameStatus::Decoded { format: io::lmax_tv::FORMAT };
        }
        decode_frame(self.bus.vram(), &self.io.lcd.regs, out)
    }

    /// FNV-1a digest of architectural state, for determinism tests and
    /// for comparing runs. Not a save state.
    pub fn state_digest(&self) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut feed = |bytes: &[u8]| {
            for &b in bytes {
                h ^= b as u64;
                h = h.wrapping_mul(0x0000_0100_0000_01b3);
            }
        };
        feed(&self.cpu.pc.to_le_bytes());
        feed(&self.cpu.status32.to_le_bytes());
        for r in self.cpu.regs {
            feed(&r.to_le_bytes());
        }
        feed(self.bus.ram());
        feed(self.bus.vram());
        for t in &self.io.timers {
            for v in [t.count, t.control, t.limit, t.pending as u32] {
                feed(&v.to_le_bytes());
            }
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbit_memory::map;

    fn machine(halfwords: &[u16]) -> Machine {
        let mut bios = vec![0u8; 0x100];
        for (i, hw) in halfwords.iter().enumerate() {
            bios[2 * i..2 * i + 2].copy_from_slice(&hw.to_le_bytes());
        }
        Machine::new(Firmware::from_bytes(bios).unwrap(), None)
    }

    // Synthetic programs, hand-encoded; each encoding was checked with GNU
    // objdump. Not firmware.
    const MOV_R0_1: [u16; 2] = [0x204a, 0x0040]; // mov r0,0x1
    const ADD_R0_R0_1: [u16; 2] = [0x2040, 0x0040]; // add r0,r0,0x1
    const B_SELF: [u16; 2] = [0x0001, 0x0000]; // b 0 (branch to itself)
    const BRK_S: u16 = 0x7fff;

    #[test]
    fn executes_until_brk() {
        let mut m = machine(&[MOV_R0_1[0], MOV_R0_1[1], ADD_R0_R0_1[0], ADD_R0_R0_1[1], BRK_S]);
        let report = m.run(100, &Breakpoints::default());
        assert_eq!(report.stop, StopReason::Halted(HaltReason::Brk));
        assert_eq!(report.instructions, 3);
        assert_eq!(m.cpu.regs[0], 2);
        let tail: Vec<String> = m.trace_events().iter().map(|e| e.to_string()).collect();
        assert!(tail[0].ends_with("40000000: mov r0,1"), "{tail:?}");
        assert!(tail[2].ends_with("brk_s"), "{tail:?}");
    }

    #[test]
    fn step_limit_and_breakpoints() {
        let prog = [MOV_R0_1[0], MOV_R0_1[1], B_SELF[0], B_SELF[1]];
        let mut m = machine(&prog);
        assert_eq!(m.run(0, &Breakpoints::default()).stop, StopReason::StepLimit);
        let report = m.run(1000, &Breakpoints::default());
        assert_eq!((report.stop, report.steps, report.instructions), (StopReason::StepLimit, 1000, 1000));
        let mut m = machine(&prog);
        let mut bp = Breakpoints::default();
        bp.add(0x4000_0004);
        let report = m.run(1000, &bp);
        assert_eq!(report.stop, StopReason::Breakpoint { pc: 0x4000_0004 });
        assert_eq!(report.instructions, 1);
    }

    #[test]
    fn cpu_errors_stop_the_run_with_a_diagnostic() {
        // major 0x04, reserved sub-opcode 0x1E
        let mut m = machine(&[0x201e, 0x0000]);
        let report = m.run(10, &Breakpoints::default());
        assert!(matches!(report.stop, StopReason::Cpu(CpuError::Illegal { pc: 0x4000_0000, .. })), "{:?}", report.stop);
        assert_eq!((report.steps, report.instructions), (0, 0));
    }

    #[test]
    fn trace_sink_receives_every_event() {
        #[derive(Clone, Default)]
        struct Shared(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Shared {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let out = Shared::default();
        let mut m = machine(&[MOV_R0_1[0], MOV_R0_1[1], BRK_S]);
        m.trace_sink = Some(Box::new(out.clone()));
        m.run(10, &Breakpoints::default());
        let text = String::from_utf8(out.0.lock().unwrap().clone()).unwrap();
        assert_eq!(text.lines().count(), 2, "{text}");
    }

    #[test]
    fn runs_are_deterministic_and_reset_restores_power_on_state() {
        let prog = [MOV_R0_1[0], MOV_R0_1[1], B_SELF[0], B_SELF[1]];
        let (mut a, mut b) = (machine(&prog), machine(&prog));
        let power_on = a.state_digest();
        assert_eq!(a.run(500, &Breakpoints::default()), b.run(500, &Breakpoints::default()));
        assert_eq!(a.state_digest(), b.state_digest());
        assert_ne!(a.state_digest(), power_on);
        ribbit_memory::Bus::write32(&mut a.bus, map::RAM_BASE, 0x1234_5678);
        a.reset();
        assert_eq!(a.state_digest(), power_on);
    }

    #[test]
    fn display_is_blank_until_firmware_enables_it() {
        let m = machine(&[BRK_S]);
        let mut frame = Box::new([0u32; WIDTH * HEIGHT]);
        assert!(matches!(m.render_frame(&mut frame), FrameStatus::Blank { .. }));
    }

    #[test]
    fn framebuffer_dma_copies_to_vram_and_raises_line_0x12() {
        use io::{intc, lcd};
        use ribbit_memory::Bus as _;
        let mut m = machine(&[B_SELF[0], B_SELF[1]]);
        let src = map::RAM_BASE + 0x1000;
        for i in 0..6 {
            m.bus.write32(src + 4 * i, 0x1111_1111 * (i + 1));
        }
        m.io.intc.write(intc::ENABLE_ADDR, intc::FLAG_DMA);
        for (addr, v) in [
            (lcd::LCD_BASE, 0x100),
            (lcd::DMA_SOURCE, src),
            (lcd::DMA_STRIDE, 3),
            (lcd::DMA_LINES, 2),
            (lcd::DMA_OFFSET, 8),
        ] {
            m.io.lcd.write(addr, v);
        }
        m.io.lcd.write(lcd::DMA_CONTROL, lcd::DMA_START);
        assert_eq!(m.bus.read32(map::VRAM_BASE + 0x108), 0, "nothing copied before the machine steps");
        m.run(1, &Breakpoints::default());
        for i in 0..6 {
            assert_eq!(m.bus.read32(map::VRAM_BASE + 0x108 + 4 * i), 0x1111_1111 * (i + 1));
        }
        assert_eq!(m.bus.read32(map::VRAM_BASE + 0x108 + 24), 0, "exactly stride x lines words");
        assert_eq!((m.io.lcd.transfers, m.io.lcd.pending), (1, None));
        assert!(m.io.irq_outputs().any(|(line, level)| line == intc::DMA_IRQ_LINE && level));
        m.io.intc.write(intc::FLAGS_ADDR, intc::FLAG_DMA);
        assert!(!m.io.irq_outputs().any(|(line, level)| line == intc::DMA_IRQ_LINE && level), "ack drops the line");
    }

    #[test]
    fn experimental_touch_is_off_by_default_and_channel_controls_stay_unmodelled() {
        use ribbit_memory::Bus as _;
        let mut m = machine(&[B_SELF[0], B_SELF[1]]);
        assert!(!m.experimental_touch_enabled());
        assert!(m.io.touch.is_none());
        let mut bus = MachineBus::new(&mut m.bus, &mut m.io, 0, 0);
        bus.write32(io::touch::CONTROL_BASE, io::touch::CHANNEL_ENABLE);
        assert_eq!(bus.read32(io::touch::CONTROL_BASE), 0, "unmodelled: reads 0");
        assert!(m.io.adc.fifo.is_empty());
        assert!(m.bus.unmapped_summary().keys().any(|&(a, _, _)| a == io::touch::CONTROL_BASE), "reported as unmapped");
        m.run(1_000_000, &Breakpoints::default());
        assert_eq!(m.interrupt_counts[io::intc::ADC_IRQ_LINE as usize], 0);
    }

    #[test]
    fn experimental_touch_feeds_the_fifo_and_raises_line_0x10_and_survives_reset() {
        use ribbit_memory::Bus as _;
        let mut m = machine(&[B_SELF[0], B_SELF[1]]);
        m.enable_experimental_mame_touch(io::touch::MameTouchConfig::default());
        m.reset();
        assert!(m.experimental_touch_enabled() && m.io.touch.is_some(), "kept across reset");
        let mut bus = MachineBus::new(&mut m.bus, &mut m.io, 0, 0);
        bus.write32(io::touch::CONTROL_BASE, io::touch::CHANNEL_ENABLE);
        assert_eq!(bus.read32(io::touch::CONTROL_BASE), io::touch::CHANNEL_ENABLE);
        assert_eq!(m.io.adc.fifo.len(), io::touch::INIT_SAMPLES);
        assert!(m.io.irq_outputs().any(|(line, level)| line == io::intc::ADC_IRQ_LINE && level));
        m.io.intc.write(io::intc::FLAGS_ADDR, io::intc::FLAG_ADC);
        assert!(!m.io.irq_outputs().any(|(line, level)| line == io::intc::ADC_IRQ_LINE && level), "ack drops it");
        // 1/60 s = 1,600,002 cycles later (96 MHz, 1 cycle per step): 12 samples.
        let r = m.run(1_600_002, &Breakpoints::default());
        assert_eq!(r.stop, StopReason::StepLimit);
        assert_eq!(m.io.adc.fifo.len(), io::touch::INIT_SAMPLES + 12);
        assert_eq!(m.io.intc.flags & io::intc::FLAG_ADC, io::intc::FLAG_ADC);
    }

    #[test]
    fn save_state_round_trips_and_checks_images() {
        let prog = [MOV_R0_1[0], MOV_R0_1[1], ADD_R0_R0_1[0], ADD_R0_R0_1[1], B_SELF[0], B_SELF[1]];
        let mut a = machine(&prog);
        a.run(1000, &Breakpoints::default());
        let state = a.save_state();
        assert!(state.starts_with(b"RIBBITST"));
        let mut b = machine(&prog);
        b.load_state(&state).unwrap();
        assert_eq!(a.state_digest(), b.state_digest());
        a.run(5000, &Breakpoints::default());
        b.run(5000, &Breakpoints::default());
        assert_eq!((a.state_digest(), a.cpu.cycles), (b.state_digest(), b.cpu.cycles));
        // Another BIOS: refused, machine unchanged.
        let mut other = machine(&[BRK_S, BRK_S]);
        let before = other.state_digest();
        assert!(matches!(other.load_state(&state), Err(StateError::WrongImages)));
        assert_eq!(other.state_digest(), before);
        assert!(matches!(other.load_state(b"junk"), Err(StateError::NotAState)));
    }
}

#[cfg(test)]
mod raw_event_tests {
    use super::*;

    /// The compact trace form re-decodes to exactly the executed instruction,
    /// for every 16-bit word and a sweep of 32-bit words with and without
    /// long immediates.
    #[test]
    fn raw_events_round_trip_every_encoding_class() {
        let mut checked = 0;
        let mut words: Vec<[u16; 4]> = (0..=u16::MAX).map(|h| [h, 0x1234, 0x5678, 0x9ABC]).collect();
        let mut x: u32 = 0x1234_5678;
        for _ in 0..200_000 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            words.push([(x >> 16) as u16, x as u16, (x >> 3) as u16, (x >> 7) as u16]);
        }
        for hw in words {
            let addr = 0x4000_1000;
            let i = ribbit_cpu::decode::decode_with(addr, |a| hw[((a - addr) / 2) as usize & 3]);
            match RawEvent::instr(7, &i).expand() {
                TraceEvent::Instr { cycle: 7, instr } => assert_eq!(instr, i, "{hw:04x?}"),
                other => panic!("{other:?}"),
            }
            checked += 1;
        }
        assert!(checked > 250_000);
        println!(
            "sizes: Instr {} TraceEvent {} RawEvent {}",
            std::mem::size_of::<ribbit_cpu::isa::Instr>(),
            std::mem::size_of::<TraceEvent>(),
            std::mem::size_of::<RawEvent>()
        );
    }
}
