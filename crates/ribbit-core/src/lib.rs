//! The Leapster machine, independent of any frontend.
//!
//! STATUS: the CPU executes the implemented ARCompact subset (see
//! ribbit-cpu); peripherals at 0x0180_xxxx are not modelled yet, so firmware
//! runs until it depends on hardware that Ribbit does not provide. Ribbit does
//! not emulate the Leapster as a whole.

pub mod config;
pub mod hd;
pub mod idle_timer;
pub mod io;
pub mod pixelart;
pub mod swf_font;
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
    /// `Io` for a BIOS (`load_firmware`).
    #[error("reading the BIOS {path}")]
    BiosIo { path: PathBuf, source: std::io::Error },
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
    let data = read_image(path).map_err(|e| match e {
        LoadError::Io { path, source } => LoadError::BiosIo { path, source },
        e => e,
    })?;
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
/// Bumped whenever the machine state's layout changes (bincode is
/// positional): 2 = UART transmit timing (divisor, busy) and CPU pipeline
/// timing (pending load, config flag); 3 = memory timing (caches, register
/// ready times, load latency); 4 = GPIO output latches, Leapster 2 flash;
/// 5 = Leapster 2 on-chip RAM and SD slot; 6 = dual-MAC accumulators, PCM DMA;
/// 7 = paused voices; 8 = Leapster 2 USB controller.
pub const STATE_VERSION: u32 = 8;

#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("not a Ribbit save state")]
    NotAState,
    #[error("save state version {0} is from another Ribbit version (this build reads version {STATE_VERSION})")]
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
    sram: &'a [u8],
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
    sram: Vec<u8>,
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
/// doublings (20,000 << 10 = 20 M steps, ~0.2 s at 96 MHz). Speed only (a
/// skip is exact either way): a busy game has many loop heads, and at 6
/// doublings Animal Genius's globe started ~10,000 failing probes a second,
/// 18% of its time; at 10 it starts ~1,500, and Wormhole's menus skip as
/// much as before (1.5% fewer steps).
const IDLE_BACKOFF_MAX_SHIFT: u8 = 10;

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
    /// The last `backoff` entry `watch` found in force, as (head, until):
    /// a busy loop asks about the same head every iteration (speed only;
    /// cleared whenever `backoff` changes).
    last_backoff: Option<(u32, u64)>,
    /// The probe write log's storage, kept between probes.
    log_buf: Vec<(u32, u8)>,
    skipped_steps: u64,
    /// Probe outcomes, for diagnosis: started, interrupted/violated/too long,
    /// state differed, a device event happened, memory differed, no room, skipped,
    /// sleeps fast-forwarded (`sleep_skip`).
    pub stats: [u64; 8],
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
        if let Some((head, until)) = self.last_backoff
            && head == pc
            && cycles < until
        {
            return false;
        }
        match self.backoff.get(&pc) {
            Some(&(until, _)) if cycles < until => {
                self.last_backoff = Some((pc, until));
                false
            }
            _ => true,
        }
    }

    /// A probe of `head` failed at `cycles`.
    fn back_off(&mut self, head: u32, cycles: u64) {
        self.last_backoff = None;
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
    /// Cache misses so far: an iteration that missed is not repeatable.
    misses: [u64; 3],
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
    /// Keep the console from turning itself off when idle: before each run,
    /// the BIOS's idle count is set back to its reload value, as a game
    /// calling LF_ResetIdleTimeout would (idle_timer.rs). Off by default
    /// (the emulated BIOS behaves as on hardware).
    pub keep_awake: bool,
    /// Leapster 2: straps to use from the next BIOS-requested restart on
    /// (see `power_request`), e.g. to boot normally after a calibration
    /// boot. Frontend policy, not hardware.
    pub restart_straps: Option<u32>,
    /// Leapster 2: restarts the BIOS requested (0x0180_B000 bit 8).
    pub restarts: u64,
    /// The BIOS's idle timer, once found.
    idle_timer: Option<idle_timer::IdleTimer>,
    firmware: Firmware,
    cartridge: Option<Cartridge>,
    /// The JIT (`ribbit-jit`; x86-64 only): compiled blocks of read-only
    /// code, with the interpreter's exact results. On where it can run,
    /// unless RIBBIT_JIT=0; None runs everything interpreted. Not saved.
    pub jit: Option<ribbit_jit::Jit>,
}

impl Machine {
    pub fn new(firmware: Firmware, cartridge: Option<Cartridge>) -> Self {
        Self::with_straps(firmware, cartridge, io::straps::MAME_LEAPSTER2_MEASURED)
    }

    /// `straps`: board configuration (0x0180_9004); bit 26 is overridden by
    /// cartridge presence.
    /// The straps register value (0x0180_9004), if the machine has one.
    pub fn straps(&self) -> Option<u32> {
        self.io.straps.map(|s| s.value)
    }

    /// Replaces the straps register value before the machine runs (bit 26
    /// still follows cartridge presence).
    pub fn set_straps(&mut self, value: u32) {
        self.io.straps = Some(io::straps::Straps::new(value, self.cartridge.is_some()));
    }

    /// Whether skipping touch calibration at boot leaves a working touch
    /// screen: false on a Leapster 2 whose internal flash holds no
    /// calibration yet (see `Firmware::touch_calibration_offset`). Other models
    /// are assumed to have one (their storage is not modelled this way).
    pub fn touch_calibration_stored(&self) -> bool {
        let at = self.firmware.touch_calibration_offset();
        self.io.spi_flash.as_ref().is_none_or(|f| f.has_touch_calibration(at))
    }

    /// Leapster 2: boot the system (its start-up screens and main menu,
    /// where the cartridge is started from its icon) instead of going
    /// straight to the cartridge. A real Leapster 2 does this at power-on
    /// with a cartridge in (recordings: the warning, the Leapster 2 and
    /// LeapFrog logos, then the menu); the BIOS reads the choice from
    /// 0x0180_B008 bits 1-2 (00 the system, 01 the cartridge).
    pub fn boot_leapster2_system(&mut self) {
        if let Some(p) = self.io.leapster2_power.as_mut() {
            p[1] &= !6;
        }
    }

    /// Leapster 2: gives a console with no stored touch calibration the
    /// factory one (`io::spiflash::FACTORY_TOUCH_CALIBRATION`); true if it
    /// did. Call after loading saved storage, before running.
    pub fn ensure_touch_calibration(&mut self) -> bool {
        let at = self.firmware.touch_calibration_offset();
        match self.io.spi_flash.as_mut() {
            Some(f) if !f.has_touch_calibration(at) => {
                f.store_factory_touch_calibration(at);
                true
            }
            _ => false,
        }
    }

    pub fn with_straps(firmware: Firmware, cartridge: Option<Cartridge>, straps: u32) -> Self {
        let bus = SystemBus::new(&firmware, cartridge.as_ref());
        let mut io = Peripherals::default();
        io.straps = Some(io::straps::Straps::new(straps, cartridge.is_some()));
        Self::install_leapster2(&mut io, &firmware, cartridge.is_some());
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
            keep_awake: false,
            restart_straps: None,
            restarts: 0,
            idle_timer: None,
            firmware,
            cartridge,
            jit: ribbit_jit::Jit::from_env(),
        };
        if m.firmware.is_leapster_tv() {
            m.enable_leapster_tv();
        }
        m
    }

    /// EXPERIMENTAL: the Leapster 2's own devices, for a known Leapster 2 BIOS.
    fn install_leapster2(io: &mut Peripherals, firmware: &Firmware, cart_present: bool) {
        if !firmware.known().is_some_and(|k| k.hardware == ribbit_memory::known::Hardware::Leapster2) {
            return;
        }
        // Internal SPI flash (io/spiflash.rs).
        io.spi_flash = Some(Default::default());
        // B008 bits 1-2 select what boots (BIOS 0.6 0x4005_35D4, strings at
        // 0x400F_F8D0): 00 "Base selected", 01 "Cart selected", 10 a
        // developer load from SD, 11 "USB Attach Reboot". HYPOTHESIS: the
        // power controller reports a cartridge as 01 (with 00, Sonic X
        // never starts and the base system shows its LeapFrog Connect
        // screen; with 01 it boots). B004 MAME's 0xFFFF_FFFF.
        let selection = if cart_present { 0b011 } else { 0b001 };
        io.leapster2_power = Some([0xFFFF_FFFF, selection]);
        // The SD slot with its card (io/sd.rs) or, on CIP3, no SD slot: the
        // pin block carries the NAND's ready/busy line instead (io/nand.rs).
        // The USB controller, no cable until the frontend plugs one in (io/usb.rs).
        io.usb = Some(io::usb::UsbDevice::new());
        if firmware.leapster2_nand() {
            io.sd = Some(Default::default());
            io.nand = Some(Default::default());
        } else {
            io.sd = Some(io::sd::SdSlot::with_card());
        }
    }

    /// Attaches a stylus to the touchscreen (io/touch.rs): `io.touch.live`
    /// and the scripted `presses` then press the panel. Without it the panel
    /// is never touched. The panel's component values are UNVERIFIED, so
    /// reports made with a stylus must say the touch model is experimental.
    /// (The name is kept from the former MAME-derived sample generator.)
    pub fn enable_experimental_mame_touch(&mut self, config: io::touch::MameTouchConfig) {
        tracing::warn!(target: "ribbit::boot", "EXPERIMENTAL: stylus on the resistive touch panel model (component values unverified)");
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
        // The BIOS's Flash player, and a cartridge's own copy (run from the
        // cartridge where it is mapped).
        let players: Vec<hd::Hooks> = [
            hd::Hooks::find(self.firmware.data(), ribbit_memory::map::BIOS_MIRROR),
            self.cartridge.as_ref().and_then(|c| hd::Hooks::find(c.rom(), ribbit_memory::map::CART_BASE)),
        ]
        .into_iter()
        .flatten()
        .collect();
        if players.is_empty() {
            return false;
        }
        let mut stops = vec![];
        for hooks in &players {
            stops.extend([hooks.edge_ctor, hooks.scanline]);
            stops.extend(hooks.glyph);
            stops.extend(hooks.transform);
            stops.extend(hooks.shape_sites().map(|s| s.2));
            stops.extend(hooks.stroke);
        }
        self.cpu.icache.set_stops(&stops);
        self.hd = Some(hd::HdCapture::with_players(players));
        true
    }

    /// The idle count and its reload value, once the BIOS's timer is found.
    fn idle_state(&mut self) -> Option<(u32, u32, u32)> {
        // Verified on every BIOS family (idle_timer.rs): original, Leapster
        // TV, L-MAX (same layout as US v2.1) and Leapster 2 (state at
        // [0x3C7F_ACBC], reload +0xD8, count +0xE0, BIOS 0.6 0x4005_5AC4).
        if self.idle_timer.is_none() {
            self.idle_timer = idle_timer::IdleTimer::find(self.cpu.regs[ribbit_cpu::reg::GP], |a| self.bus.peek8(a));
        }
        let t = self.idle_timer?;
        let bus = &self.bus;
        let word =
            |a: u32| Some(u32::from_le_bytes([bus.peek8(a)?, bus.peek8(a + 1)?, bus.peek8(a + 2)?, bus.peek8(a + 3)?]));
        let state = word(t.state_ptr).filter(|&s| s != 0)?;
        Some((state + t.count_offset, word(state + t.count_offset)?, word(state + t.reload_offset)?))
    }

    fn maintain_idle_timer(&mut self) {
        if !self.keep_awake {
            return;
        }
        if let Some((count_addr, count, reload)) = self.idle_state()
            && reload != 0
            && count < reload
        {
            use ribbit_memory::Bus as _;
            self.bus.write32(count_addr, reload);
        }
    }

    /// Debug: keeps the BIOS's idle count at most `secs` (to try the
    /// power-off path without waiting).
    pub fn cap_idle_count(&mut self, secs: u32) {
        if let Some((count_addr, count, _)) = self.idle_state()
            && count > secs
        {
            use ribbit_memory::Bus as _;
            self.bus.write32(count_addr, secs);
        }
    }

    /// Seconds left on the BIOS's idle timer (None: not found or disabled).
    pub fn idle_seconds_left(&mut self) -> Option<u32> {
        self.idle_state().filter(|&(_, _, reload)| reload != 0).map(|(_, count, _)| count)
    }

    /// Restarts the BIOS's idle timer, as a game calling LF_ResetIdleTimeout.
    /// Where this BIOS keeps its idle timer, once found (see
    /// `set_idle_timer`).
    pub fn idle_timer(&self) -> Option<idle_timer::IdleTimer> {
        self.idle_timer
    }

    /// Tells a fresh machine where its BIOS keeps the idle timer. It is
    /// found through the GP of the code running when it is looked for, so a
    /// machine that has just loaded a state may not find it at once (a game
    /// can run with a GP of its own); a frontend resuming a state passes the
    /// location the earlier machine found (same BIOS), so the reset after it
    /// never depends on that.
    pub fn set_idle_timer(&mut self, timer: Option<idle_timer::IdleTimer>) {
        if timer.is_some() {
            self.idle_timer = timer;
        }
    }

    pub fn reset_idle_count(&mut self) {
        if let Some((count_addr, _, reload)) = self.idle_state()
            && reload != 0
        {
            use ribbit_memory::Bus as _;
            self.bus.write32(count_addr, reload);
        }
    }

    /// The BIOS turned the console off for inactivity: its idle count ran
    /// out (with the timer enabled). The console then waits for its power
    /// button, so a frontend should end the session.
    pub fn idle_powered_off(&mut self) -> bool {
        matches!(self.idle_state(), Some((_, 0, reload)) if reload != 0)
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
        self.io.touch = self.touch_config.clone().map(io::touch::Stylus::new);
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
        let (straps, lines, board) = (self.io.straps, self.io.gpio.unresolved_lines, self.io.gpio.board_lines);
        let battery = self.io.battery;
        // Non-volatile: the EEPROM keeps its contents across a power cycle.
        let eeprom = std::mem::take(&mut self.io.eeprom.data);
        let cart_save = self.io.eeprom.cart.take();
        // The Leapster 2's internal flashes are non-volatile too.
        let flash = self.io.spi_flash.take();
        let nand = self.io.nand.take();
        let sd_card = self.io.sd.as_mut().and_then(|s| s.card.take());
        let usb_host = self.io.usb.as_mut().map(|u| (std::mem::take(&mut u.host), u.log));
        self.io = Peripherals::default();
        Self::install_leapster2(&mut self.io, &self.firmware, self.cartridge.is_some());
        if let (Some(f), Some(slot)) = (flash, self.io.spi_flash.as_mut()) {
            slot.load(&f.data);
            slot.dirty = f.dirty;
        }
        if nand.is_some() {
            self.io.nand = nand;
        }
        if let (Some(card), Some(slot)) = (sd_card, self.io.sd.as_mut())
            && slot.card.is_some()
        {
            // The card keeps its sectors; the card powers up in its idle state.
            slot.card = Some(card.powered_on());
        }
        if let (Some((host, log)), Some(usb)) = (usb_host, self.io.usb.as_mut()) {
            // The cable stays in; the host sees the device again.
            usb.log = log;
            usb.restarted(host);
        }
        self.io.straps = straps;
        self.io.gpio.unresolved_lines = lines;
        self.io.gpio.board_lines = board;
        self.io.battery = battery;
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
        self.run_sd_command();
        if self.io.restart_requested {
            self.power_request();
            return result;
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
            self.io.deliver_pcm_irq();
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
            self.run_usb();
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
    /// Runs until at least `cycles` CPU cycles have passed (the last
    /// instruction may end a few cycles later: one can take several), or
    /// until a breakpoint or a stop. Emulated time, unlike `run`'s steps.
    pub fn run_cycles(&mut self, cycles: u64, breakpoints: &Breakpoints) -> RunReport {
        let target = self.cpu.cycles + cycles;
        let mut total =
            RunReport { steps: 0, instructions: 0, interrupts: 0, pc: self.cpu.pc, stop: StopReason::StepLimit };
        while self.cpu.cycles < target {
            // A step is at most 4 cycles, so this never passes the target by
            // more than one instruction.
            let r = self.run(((target - self.cpu.cycles) / 4).max(1), breakpoints);
            total.steps += r.steps;
            total.instructions += r.instructions;
            total.interrupts += r.interrupts;
            total.pc = r.pc;
            let done = r.stop != StopReason::StepLimit;
            total.stop = r.stop;
            if done {
                break;
            }
        }
        total
    }

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
        self.maintain_idle_timer();
        // Device state may have been changed from outside since the last run.
        self.io.sync_needed = true;
        // Batches need nothing observed per instruction.
        let batch =
            breakpoints.is_empty() && self.trace_sink.is_none() && self.coverage.is_none() && !self.cpu.record_last;
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
                if hd.select(from) {
                    let h = *hd.active();
                    if from == h.edge_ctor {
                        hd.edge(self.cpu.regs[1], peek);
                    } else if from == h.scanline {
                        hd.scanline(self.cpu.regs[14], self.cpu.regs[17] as i32, peek);
                    } else if Some(from) == h.glyph {
                        hd.glyph(&self.cpu.regs, peek);
                    } else if Some(from) == h.transform {
                        hd.transform(&self.cpu.regs, peek);
                    } else if h.shape_sites().is_some_and(|s| s.2 == from) {
                        hd.shape_edge(&self.cpu.regs, peek);
                    } else if Some(from) == h.stroke {
                        hd.stroke(&self.cpu.regs, peek);
                    }
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
                Ok(Step::Sleeping) => {
                    if self.idle_skip {
                        self.sleep_skip(max_steps, &mut report);
                    }
                }
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
        let watch = |pc, cycles| match probe_head {
            Some(head) => pc == head,
            None => skip && idle.watch(pc, cycles),
        };
        let block = match &mut self.jit {
            Some(jit) => self.cpu.run_block_with(&mut bus, &mut ribbit_cpu::NoAuxExt, budget, fence, watch, jit),
            None => self.cpu.run_block(&mut bus, &mut ribbit_cpu::NoAuxExt, budget, fence, watch),
        };
        let synced = bus.synced;
        if matches!(block.exit, ribbit_cpu::BlockExit::Slow)
            && ribbit_jit::PROFILING.load(std::sync::atomic::Ordering::Relaxed)
            && let Some(p) = ribbit_jit::PROFILE.lock().unwrap().as_mut()
        {
            *p[2].entry(self.cpu.pc).or_default() += 1;
        }
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
        self.run_sd_command();
        if self.io.restart_requested {
            self.power_request();
            return Some(Ok(()));
        }
        let ticks = self.clock.ticks(self.cpu.cycles - synced);
        if self.io.voices.lfc_start_pending {
            self.io.voices.start_lfc(&mut self.bus);
        }
        if ticks > 0 {
            self.io.voices.advance(ticks, self.clock.peripheral_hz, &mut self.bus);
            self.io.deliver_pcm_irq();
        }
        self.io.deferred += ticks;
        if self.io.sync_needed || self.io.deferred >= self.io.horizon {
            if self.io.deferred >= self.io.horizon {
                self.io.event_syncs += 1;
            }
            self.io.catch_up();
            self.run_usb();
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
                && self.io.mem.misses == p.misses
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
            self.idle.last_backoff = None;
            // One iteration: its cycles (absolute, so correct even when the
            // probe spans several `run` calls) and its steps (instructions,
            // interrupt entries, sleeps), which differ now that an instruction
            // can take several cycles (pipeline timing).
            let cycles = self.cpu.cycles - p.cycles;
            let steps = p.steps;
            let instructions = self.cpu.instructions_retired - p.retired;
            if cycles == 0 || steps == 0 {
                return;
            }
            // Largest number of iterations that ends strictly before the next
            // device event, CPU-timer event and the step budget.
            let to_event = self.io.horizon.saturating_sub(self.io.deferred);
            let mut budget_cycles = self.clock.max_cycles_below(to_event).min(self.cpu.cycles_to_timer_event());
            // PCM DMA moves memory and raises interrupts at each DAC tick.
            if self.io.voices.pcm.running() {
                let to_dac = self.io.voices.ticks_to_next_sample(self.clock.peripheral_hz);
                budget_cycles = budget_cycles.min(self.clock.max_cycles_below(to_dac));
            }
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
            self.io.deliver_pcm_irq();
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
            misses: self.io.mem.misses,
        });
        self.io.probe = true;
        self.io.probe_violation = false;
        self.bus.write_log = Some(std::mem::take(&mut self.idle.log_buf));
    }

    /// Sleep fast-forwarding, exact: a sleeping CPU (SLEEP) spends one cycle
    /// per step until an interrupt is pending, so with none pending and the
    /// devices up to date, time can advance to just before the next device
    /// or CPU-timer event (or PCM DAC tick, or the step budget) in one go.
    /// The Leapster 2 BIOS's idle task sleeps (0.6 0x4004_B174); stepping it
    /// a cycle at a time ran an idle machine at about 60% of real time.
    fn sleep_skip(&mut self, max_steps: u64, report: &mut RunReport) {
        if !self.cpu.sleeping || self.io.sync_needed || self.cpu.pending_interrupt().is_some() {
            return;
        }
        let to_event = self.io.horizon.saturating_sub(self.io.deferred);
        let mut budget = self.clock.max_cycles_below(to_event).min(self.cpu.cycles_to_timer_event());
        if self.io.voices.pcm.running() {
            let to_dac = self.io.voices.ticks_to_next_sample(self.clock.peripheral_hz);
            budget = budget.min(self.clock.max_cycles_below(to_dac));
        }
        // Each sleeping step is one cycle; leave this step's own count to `run`.
        let k = budget.min(max_steps.saturating_sub(report.steps + 2));
        if k == 0 {
            return;
        }
        self.cpu.skip_sleep(k);
        let ticks = self.clock.ticks(k);
        self.io.deferred += ticks;
        self.io.voices.advance(ticks, self.clock.peripheral_hz, &mut self.bus);
        self.io.deliver_pcm_irq();
        report.steps += k;
        self.idle.skipped_steps += k;
        self.idle.stats[7] += 1;
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
            sram: self.bus.sram(),
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

    /// A rewind snapshot: `snapshot` without the SD card's sectors (the card
    /// in it is empty; same format). A large card (a LeapLoader card holds
    /// ~680 MB) made every half-second snapshot cost seconds of serialising and
    /// compressing, and the queued snapshots grew without bound. The card is
    /// kept in its own file, so rewinding leaves it as it is
    /// (`load_rewind_state`). Not for save states: those keep the card.
    pub fn rewind_snapshot(&mut self) -> RawState {
        let sectors = self.sd_sectors_mut().map(std::mem::take);
        let raw = self.snapshot();
        if let (Some(sectors), Some(card)) = (sectors, self.sd_sectors_mut()) {
            *card = sectors;
        }
        raw
    }

    /// Loads a `rewind_snapshot`, keeping the SD card's current sectors (and
    /// what of them still needs saving).
    pub fn load_rewind_state(&mut self, bytes: &[u8]) -> Result<(), StateError> {
        let card = self.io.sd.as_mut().and_then(|sd| sd.card.as_mut());
        let kept = card.map(|c| (std::mem::take(&mut c.sectors), std::mem::take(&mut c.changed), c.synced, c.dirty));
        let result = self.load_state(bytes);
        if let (Some((sectors, changed, synced, dirty)), Some(card)) = (kept, self.io.sd.as_mut().and_then(|sd| sd.card.as_mut())) {
            card.sectors = sectors;
            card.changed = changed;
            card.synced = synced;
            card.dirty |= dirty;
        }
        result
    }

    fn sd_sectors_mut(&mut self) -> Option<&mut std::collections::BTreeMap<u32, Vec<u8>>> {
        self.io.sd.as_mut()?.card.as_mut().map(|c| &mut c.sectors)
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
        // Version 7 (1.4.x-1.5.1) differs only in the USB controller.
        if version != STATE_VERSION && version != 7 {
            return Err(StateError::Version(version));
        }
        let payload =
            miniz_oxide::inflate::decompress_to_vec(&body[4..]).map_err(|e| StateError::Damaged(format!("{e:?}")))?;
        io::usb::READING_V7.with(|v| v.set(version == 7));
        let s = bincode::deserialize::<State>(&payload);
        io::usb::READING_V7.with(|v| v.set(false));
        let s = s.map_err(|e| StateError::Damaged(e.to_string()))?;
        if s.bios_crc != self.firmware.id().crc32 || s.cart_crc != self.cartridge.as_ref().map(|c| c.id().crc32) {
            return Err(StateError::WrongImages);
        }
        if s.interrupt_counts.len() != self.interrupt_counts.len() {
            return Err(StateError::Damaged("interrupt counters".into()));
        }
        if !self.bus.restore_memory(&s.ram, &s.vram, &s.sram) {
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
    pub fn idle_stats(&self) -> [u64; 8] {
        self.idle.stats
    }

    /// Leapster 2: the BIOS wrote 0x0180_B000 bit 8 (see
    /// `io::Peripherals::leapster2_power_write`). If its idle timer has run
    /// out this is the inactivity power-off: nothing happens here and the
    /// frontend ends the session (`idle_powered_off`). Otherwise the console
    /// starts again (HYPOTHESIS: as a restart, the case of the fatal-assert
    /// handler, which expects to see its saved message on the next boot):
    /// devices and CPU reset, memory kept (the saved assert lives at
    /// 0x0300_FF00), B008 bits 0-2 kept (the saved-assert flag and the
    /// boot selection the system menu writes), time carries on. Before this was
    /// modelled the BIOS ran on after the assert with the asserting task's
    /// interrupt-disable count leaked, so that task always ran masked: the
    /// Leapster 2's PCM interrupts were lost (clicks every 256 samples).
    fn power_request(&mut self) {
        self.io.restart_requested = false;
        if self.idle_powered_off() {
            return;
        }
        let b008 = self.io.leapster2_power.map(|p| p[1]);
        let (ram, vram, sram) = (self.bus.ram().to_vec(), self.bus.vram().to_vec(), self.bus.sram().to_vec());
        let (cycles, retired) = (self.cpu.cycles, self.cpu.instructions_retired);
        if let Some(s) = self.restart_straps.take() {
            self.set_straps(s);
        }
        self.reset();
        self.bus.restore_memory(&ram, &vram, &sram);
        if let (Some(v), Some(p)) = (b008, self.io.leapster2_power.as_mut()) {
            p[1] = (p[1] & !7) | (v & 7);
        }
        self.cpu.cycles = cycles;
        self.cpu.instructions_retired = retired;
        self.restarts += 1;
        tracing::info!(target: "ribbit::boot", restarts = self.restarts, "the BIOS restarted the console (0x0180_B000 bit 8)");
    }

    /// Leapster 2: plugs the USB cable in or pulls it out (io/usb.rs).
    /// False on other models.
    pub fn usb_plug(&mut self, attached: bool) -> bool {
        let Some(u) = self.io.usb.as_mut() else { return false };
        u.plug(attached);
        self.io.sync_needed = true;
        true
    }

    /// Leapster 2: a transfer from the USB host.
    pub fn usb_submit(&mut self, t: io::usb::Transfer) {
        if let Some(u) = self.io.usb.as_mut() {
            u.submit(t);
            self.io.sync_needed = true;
        }
    }

    /// Leapster 2: cancels a host transfer; true if it had not finished.
    pub fn usb_unlink(&mut self, id: u32) -> bool {
        self.io.usb.as_mut().is_some_and(|u| u.unlink(id))
    }

    /// Leapster 2: the host transfers finished since the last call.
    pub fn usb_take_done(&mut self) -> Vec<io::usb::Finished> {
        self.io.usb.as_mut().map(|u| u.take_done()).unwrap_or_default()
    }

    /// Leapster 2: the USB controller's due work (io/usb.rs): it reads and
    /// writes buffer descriptors and packets in memory.
    fn run_usb(&mut self) {
        if let Some(u) = self.io.usb.as_mut()
            && u.due
        {
            u.service(&mut self.bus);
        }
    }

    /// Leapster 2: runs an SD command descriptor the program started (io/sd.rs).
    fn run_sd_command(&mut self) {
        if let Some(slot) = self.io.sd.as_mut()
            && let Some(d) = slot.pending.take()
        {
            slot.run(d, &mut self.bus);
        }
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
        // The Leapster 2's copy takes time (io/lcd.rs); the Leapster's is
        // instant.
        if self.io.leapster2_power.is_some() {
            self.io.lcd.start_transfer(dma.words);
        } else {
            self.io.intc.raise(io::intc::FLAG_DMA);
        }
    }

    /// The console's picture: the LCD, or on a Leapster TV (no LCD) the TV
    /// chip's picture.
    pub fn render_frame(&self, out: &mut [u32; WIDTH * HEIGHT]) -> FrameStatus {
        if self.is_leapster_tv()
            && let Some(status) = self.render_tv(out)
        {
            return status;
        }
        decode_frame(self.bus.vram(), &self.io.lcd.regs, out)
    }

    /// EXPERIMENTAL: the TV chip's picture (the 160x160 game area), on a
    /// Leapster TV or an L-MAX; None without a TV chip.
    pub fn render_tv(&self, out: &mut [u32; WIDTH * HEIGHT]) -> Option<FrameStatus> {
        let tv = self.io.lmax_tv.as_ref()?;
        tv.render_game(out);
        Some(FrameStatus::Decoded { format: io::lmax_tv::FORMAT })
    }

    /// Decodes the handheld LCD, also when a TV chip supplies the picture
    /// `render_frame` shows (the L-MAX can show a second picture there).
    pub fn render_lcd(&self, out: &mut [u32; WIDTH * HEIGHT]) -> FrameStatus {
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
    fn untouched_panel_samples_full_scale_and_raises_line_0x10() {
        use ribbit_memory::Bus as _;
        let mut m = machine(&[B_SELF[0], B_SELF[1]]);
        assert!(!m.experimental_touch_enabled() && m.io.touch.is_none(), "no stylus by default");
        let mut bus = MachineBus::new(&mut m.bus, &mut m.io, 0, 0);
        bus.write32(io::adc::CONFIG_ADDR, 0xFFFF_FF81);
        // The touch driver's pen-up check (entry 2): pins 20 low, 21 high, input 1.
        bus.write32(io::gpio::DATA_OUT, 0x0020_0000);
        bus.write32(io::gpio::DIRE, 0x0030_0000);
        bus.write32(io::adc::CONTROL_BASE, io::adc::RUNNING | 1);
        assert_eq!(bus.read32(io::adc::CONTROL_BASE), io::adc::RUNNING | 1);
        // 4096 ticks of 16 MHz = 24,576 cycles at 96 MHz (one per step here).
        m.cpu.config.pipeline_timing = false;
        m.run(24_576, &Breakpoints::default());
        assert_eq!(m.io.adc.fifo.len(), 1);
        assert!(m.io.adc.fifo[0] >> 5 & 0x7FF > 0x7F0, "pen up reads full scale");
        assert!(m.io.irq_outputs().any(|(line, level)| line == io::intc::ADC_IRQ_LINE && level));
    }

    #[test]
    fn stylus_survives_reset_and_presses_the_panel() {
        use ribbit_memory::Bus as _;
        let mut m = machine(&[B_SELF[0], B_SELF[1]]);
        m.enable_experimental_mame_touch(io::touch::MameTouchConfig::default());
        m.reset();
        assert!(m.experimental_touch_enabled() && m.io.touch.is_some(), "kept across reset");
        m.io.touch.as_mut().unwrap().live = Some((0x200, 0x600));
        let mut bus = MachineBus::new(&mut m.bus, &mut m.io, 0, 0);
        bus.write32(io::adc::CONFIG_ADDR, 0xFFFF_FF81);
        // The X measurement (entry 4): pins 21 low and 23 high, input 2.
        bus.write32(io::gpio::DATA_OUT, 0x0080_0000);
        bus.write32(io::gpio::DIRE, 0x00A0_0000);
        bus.write32(io::adc::CONTROL_BASE, io::adc::RUNNING | 2);
        m.cpu.config.pipeline_timing = false;
        m.run(24_576, &Breakpoints::default());
        let x = m.io.adc.fifo[0] >> 5 & 0x7FF;
        assert!(x.abs_diff(0x200) < 0x20, "{x:#x}");
    }

    #[test]
    fn rewind_snapshots_leave_the_sd_card_alone() {
        let prog = [MOV_R0_1[0], MOV_R0_1[1], B_SELF[0], B_SELF[1]];
        let mut m = machine(&prog);
        m.io.sd = Some(io::sd::SdSlot::with_card());
        fn card(m: &mut Machine) -> &mut io::sd::SdCard {
            m.io.sd.as_mut().unwrap().card.as_mut().unwrap()
        }
        card(&mut m).sectors.insert(9000, vec![1; 512]);
        let full = m.snapshot().compress();
        let rewind = m.rewind_snapshot().compress();
        assert!(rewind.len() < full.len(), "the card is left out");
        assert_eq!(card(&mut m).sectors[&9000], vec![1; 512], "and put back");
        // A later write survives rewinding, and still needs saving.
        card(&mut m).sectors.insert(9001, vec![2; 512]);
        card(&mut m).dirty = true;
        m.load_rewind_state(&rewind).unwrap();
        assert_eq!((card(&mut m).sectors[&9000][0], card(&mut m).sectors[&9001][0], card(&mut m).dirty), (1, 2, true));
        // A save state keeps the card.
        m.load_state(&full).unwrap();
        assert!(!card(&mut m).sectors.contains_key(&9001));
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
