//! The Leapster machine, independent of any frontend.
//!
//! STATUS: the CPU executes the implemented ARCompact subset (see
//! ribbit-cpu); peripherals at 0x0180_xxxx are not modelled yet, so firmware
//! runs until it depends on hardware that Ribbit does not provide. Ribbit does
//! not emulate the Leapster as a whole.

pub mod config;
pub mod io;

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
}

pub fn load_firmware(path: &Path) -> Result<Firmware, LoadError> {
    let data = std::fs::read(path).map_err(|source| LoadError::Io { path: path.into(), source })?;
    Firmware::from_bytes(data).map_err(|source| LoadError::Firmware { path: path.into(), source })
}

pub fn load_cartridge(path: &Path) -> Result<Cartridge, LoadError> {
    let data = std::fs::read(path).map_err(|source| LoadError::Io { path: path.into(), source })?;
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

const IDLE_PROBE_MAX_STEPS: u64 = 512;
const IDLE_BACKOFF_STEPS: u64 = 20_000;

#[derive(Default)]
struct IdleTracker {
    /// Recent backward-jump targets; a repeat marks a loop.
    recent: [u32; 8],
    recent_next: usize,
    probe: Option<IdleProbe>,
    /// Loop heads that failed a probe, and the step count before which they
    /// are not probed again.
    backoff: std::collections::HashMap<u32, u64>,
    skipped_steps: u64,
    /// Probe outcomes, for diagnosis: started, interrupted/violated/too long,
    /// state differed, a device event happened, memory differed, no room, skipped.
    pub stats: [u64; 7],
}

struct IdleProbe {
    head: u32,
    snapshot: ribbit_cpu::idle::IdleSnapshot,
    cycles: u64,
    retired: u64,
    report_steps: u64,
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
    /// Debug option: first cycle at which each PC was executed (None = off).
    pub coverage: Option<std::collections::HashMap<u32, u64>>,
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
        Self {
            cpu,
            bus,
            buttons: Buttons::default(),
            trace_tail: TraceRing::new(TRACE_TAIL),
            trace_sink: None,
            io,
            clock: Clock::default(),
            interrupt_counts: [0; 64],
            touch_config: None,
            first_cart_pc: None,
            idle_skip: true,
            idle: IdleTracker::default(),
            coverage: None,
            firmware,
            cartridge,
        }
    }

    /// EXPERIMENTAL, UNVERIFIED: feeds ADC channel 0 with MAME's guessed
    /// touchscreen samples (io/touch.rs). Empty `presses` = stylus never down.
    /// Off unless called; every report produced with it on must say so.
    pub fn enable_experimental_mame_touch(&mut self, config: io::touch::MameTouchConfig) {
        tracing::warn!(target: "ribbit::boot", "EXPERIMENTAL: MAME-derived touchscreen/ADC model enabled (unverified, not hardware behaviour)");
        self.touch_config = Some(config);
        self.install_touch();
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
        let mut bus = MachineBus { sys: &mut self.bus, io: &mut self.io, cycle: start, pc: self.cpu.pc };
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
        self.io.voices.advance(ticks, self.clock.peripheral_hz, &mut self.bus);
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
        self.io.gpio.buttons = self.buttons;
        // Device state may have been changed from outside since the last run.
        self.io.sync_needed = true;
        while report.steps < max_steps {
            if report.steps > 0 && breakpoints.hit(self.cpu.pc) {
                report.stop = StopReason::Breakpoint { pc: self.cpu.pc };
                break;
            }
            let cycle = self.cpu.cycles;
            let from = self.cpu.pc;
            let result = self.step();
            if self.idle_skip && breakpoints.is_empty() && self.trace_sink.is_none() {
                self.idle_track(&result, from, max_steps, &mut report);
            }
            if let Some(sink) = &mut self.trace_sink {
                for a in self.io.take_events() {
                    let _ = writeln!(sink, "{a}");
                }
            }
            match result {
                Ok(Step::Executed) => {
                    report.instructions += 1;
                    self.record_instr(cycle);
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

    /// Idle-loop fast-forwarding, exact. After a backward jump to the same
    /// address twice in a row, one iteration is run as a probe; if it leaves
    /// the CPU state (except time), memory and devices exactly as they were,
    /// and touched only side-effect-free registers, the following iterations
    /// are identical until the next device or CPU-timer event, so time is
    /// advanced to just before that event in one go. Verified by comparing
    /// state digests with `idle_skip` off.
    fn idle_track(&mut self, result: &Result<Step, CpuError>, from: u32, max_steps: u64, report: &mut RunReport) {
        let pc = self.cpu.pc;
        if let Some(p) = &mut self.idle.probe {
            p.steps += 1;
            let interrupted = !matches!(result, Ok(Step::Executed));
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
            self.bus.write_log = None;
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
                self.idle.backoff.insert(p.head, report.steps + IDLE_BACKOFF_STEPS);
                return;
            }
            let (cycles, steps) = (self.cpu.cycles - p.cycles, report.steps + 1 - p.report_steps);
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
        let backward = matches!(result, Ok(Step::Executed)) && pc <= from && self.cpu.delay_target.is_none();
        if !backward {
            return;
        }
        if !self.idle.recent.contains(&pc) {
            self.idle.recent[self.idle.recent_next] = pc;
            self.idle.recent_next = (self.idle.recent_next + 1) % self.idle.recent.len();
            return;
        }
        if self.idle.backoff.get(&pc).is_some_and(|&until| report.steps < until) || self.io.sync_needed {
            return;
        }
        self.idle.stats[0] += 1;
        self.idle.probe = Some(IdleProbe {
            head: pc,
            snapshot: self.cpu.idle_snapshot(),
            cycles: self.cpu.cycles,
            retired: self.cpu.instructions_retired,
            report_steps: report.steps + 1,
            steps: 0,
            event_syncs: self.io.event_syncs,
        });
        self.io.probe = true;
        self.io.probe_violation = false;
        self.bus.write_log = Some(Vec::new());
    }

    fn idle_end_probe(&mut self, _ok: bool) {
        if let Some(p) = self.idle.probe.take() {
            self.idle.backoff.insert(p.head, self.cpu.instructions_retired + IDLE_BACKOFF_STEPS);
        }
        self.io.probe = false;
        self.bus.write_log = None;
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
        self.io.intc.raise(io::intc::FLAG_DMA);
    }

    /// Decodes the current LCD contents from emulated VRAM.
    pub fn render_frame(&self, out: &mut [u32; WIDTH * HEIGHT]) -> FrameStatus {
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
        let mut bus = MachineBus { sys: &mut m.bus, io: &mut m.io, cycle: 0, pc: 0 };
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
        let mut bus = MachineBus { sys: &mut m.bus, io: &mut m.io, cycle: 0, pc: 0 };
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
