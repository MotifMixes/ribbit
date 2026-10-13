//! On-chip peripherals at 0x0180_0000-0x0180_FFFF.
//!
//! `MachineBus` gives the CPU one view of memory: RAM/ROM/VRAM/cart go to
//! `SystemBus`; peripheral addresses go to `Peripherals`, and any register
//! no device claims falls through to `SystemBus`, which records it as an
//! unmapped access (reads return 0). Nothing is faked: an unmodelled register
//! is reported, not answered with a plausible value.

pub mod adc;
pub mod audio;
pub mod eeprom;
pub mod gpio;
pub mod intc;
pub mod lcd;
pub mod straps;
pub mod timer;
pub mod touch;
pub mod uart;
pub mod voices;

use adc::Adc;
use audio::AudioUnit;
use eeprom::Eeprom;
use gpio::GpioInput;
use intc::IntFlags;
use lcd::LcdController;
use ribbit_debug::TraceRing;
use ribbit_memory::{Bus, SystemBus, map};
use straps::{ClockControl, Straps};
use timer::{MmioTimer, TIMER_IRQS};
use touch::MameTouch;
use uart::DebugUart;
use voices::VoiceEngine;

/// Converts CPU cycles into peripheral-clock counts.
///
/// Evidence for 16 MHz: the BIOS MQX BSP declares 100 ticks/s and 0x27100
/// timer counts per tick; the BIOS clock query (0x4007_8940) returns
/// 16,000,000 for the devices at 0x0180_D500/0x0180_D600; MAME also uses
/// 16 MHz. The CPU clock is configurable: 96 MHz is the nominal figure
/// (MAME header, eLinux, the BIOS's own clock table); MAME runs its core at
/// 48 MHz to match videos. Ribbit counts one cycle per CPU step, so the ratio
/// is only as good as that placeholder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clock {
    pub cpu_hz: u64,
    pub peripheral_hz: u64,
    /// Fractional peripheral counts carried between steps (in cpu_hz units).
    remainder: u64,
}

impl Clock {
    pub const PERIPHERAL_HZ: u64 = 16_000_000;
    pub const NOMINAL_CPU_HZ: u64 = 96_000_000;

    pub fn new(cpu_hz: u64, peripheral_hz: u64) -> Self {
        assert!(cpu_hz > 0);
        Self { cpu_hz, peripheral_hz, remainder: 0 }
    }

    /// The largest number of CPU cycles over which fewer than `ticks`
    /// peripheral counts elapse (from the current phase).
    pub fn max_cycles_below(&self, ticks: u64) -> u64 {
        // floor((K * p + r) / c) < ticks  <=>  K * p + r < ticks * c
        let limit = ticks as u128 * self.cpu_hz as u128;
        let r = self.remainder as u128;
        if limit <= r {
            return 0;
        }
        ((limit - r - 1) / self.peripheral_hz as u128).min(u64::MAX as u128) as u64
    }

    /// Peripheral counts elapsed over `cpu_cycles`; exact over any sequence
    /// of calls (no drift).
    pub fn ticks(&mut self, cpu_cycles: u64) -> u64 {
        // Fastest path (a step is a few cycles): repeated subtraction instead
        // of division; the remainder stays below cpu_hz, so results are equal.
        if cpu_cycles <= 16 && self.peripheral_hz <= self.cpu_hz {
            let mut total = self.remainder + cpu_cycles * self.peripheral_hz;
            let mut ticks = 0;
            while total >= self.cpu_hz {
                total -= self.cpu_hz;
                ticks += 1;
            }
            self.remainder = total;
            return ticks;
        }
        // Fast path in 64-bit arithmetic; same result.
        if let Some(total) = cpu_cycles.checked_mul(self.peripheral_hz).and_then(|t| t.checked_add(self.remainder)) {
            self.remainder = total % self.cpu_hz;
            return total / self.cpu_hz;
        }
        let total = cpu_cycles as u128 * self.peripheral_hz as u128 + self.remainder as u128;
        self.remainder = (total % self.cpu_hz as u128) as u64;
        (total / self.cpu_hz as u128) as u64
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new(Self::NOMINAL_CPU_HZ, Self::PERIPHERAL_HZ)
    }
}

/// A CPU access to a peripheral address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MmioAccess {
    pub cycle: u64,
    pub pc: u32,
    pub addr: u32,
    pub width: u8,
    pub write: bool,
    pub value: u32,
    /// False if no Ribbit device handled it.
    pub modelled: bool,
    /// The value came from a user-supplied debug stub (`Peripherals::stubs`).
    pub stub: bool,
}

impl std::fmt::Display for MmioAccess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{:>10} -------- mmio pc={:08x} {}{} {:#010x} {} {:#010x}{}",
            self.cycle,
            self.pc,
            if self.write { "W" } else { "R" },
            self.width * 8,
            self.addr,
            if self.write { "<-" } else { "->" },
            self.value,
            if self.stub {
                "  (debug stub)"
            } else if self.modelled {
                ""
            } else {
                "  (unmodelled)"
            }
        )
    }
}

/// Peripheral accesses kept for reports.
pub const MMIO_TAIL: usize = 64;

pub struct Peripherals {
    pub timers: [MmioTimer; 3],
    pub intc: IntFlags,
    pub adc: Adc,
    pub lcd: LcdController,
    pub audio: AudioUnit,
    pub voices: VoiceEngine,
    pub gpio: GpioInput,
    pub eeprom: Eeprom,
    /// None = unmodelled (reads 0 and is logged); the machine sets it.
    pub straps: Option<Straps>,
    pub clock_control: ClockControl,
    pub uart: DebugUart,
    /// EXPERIMENTAL, UNVERIFIED MAME-derived touch/ADC sample source. None
    /// (the default) = no ADC conversions and channel controls unmodelled.
    pub touch: Option<MameTouch>,
    pub mmio_tail: TraceRing<MmioAccess>,
    /// Accesses since the last `take_events` (for the trace sink).
    events: Vec<MmioAccess>,
    pub record_events: bool,
    /// Debug-only fixed read values for registers no device models, used to
    /// follow the same path as another emulator in differential tests (e.g.
    /// MAME 0.289's stubs). Empty by default; every use is labelled in traces.
    pub stubs: std::collections::BTreeMap<u32, u32>,
    /// For each unmodelled register: the instruction addresses that accessed
    /// it (first 16), for finding the driver code behind an access.
    pub unmodelled_pcs: std::collections::BTreeMap<u32, Vec<u32>>,
    /// Catch-up scheduling (speed only; results are identical to advancing
    /// every step): peripheral ticks not yet applied, the number of ticks
    /// after the last sync before any device can change an interrupt line,
    /// and whether lines must be recomputed (a register was accessed).
    pub deferred: u64,
    pub horizon: u64,
    pub sync_needed: bool,
    /// Device events applied so far (timer wraps, touch updates reached).
    pub event_syncs: u64,
    /// Idle-loop probe: set while one loop iteration is being checked; any
    /// register write, or a read of a register outside `idle_safe`, makes the
    /// iteration ineligible for fast-forwarding.
    pub probe: bool,
    pub probe_violation: bool,
}

impl Default for Peripherals {
    fn default() -> Self {
        Self {
            timers: [MmioTimer::reset(); 3],
            intc: IntFlags::default(),
            adc: Adc::default(),
            lcd: LcdController::default(),
            audio: AudioUnit::default(),
            voices: VoiceEngine::default(),
            gpio: GpioInput::default(),
            eeprom: Eeprom::default(),
            straps: None,
            clock_control: ClockControl::default(),
            uart: DebugUart::default(),
            touch: None,
            mmio_tail: TraceRing::new(MMIO_TAIL),
            events: Vec::new(),
            record_events: false,
            stubs: Default::default(),
            unmodelled_pcs: Default::default(),
            deferred: 0,
            horizon: 0,
            sync_needed: true,
            event_syncs: 0,
            probe: false,
            probe_violation: false,
        }
    }
}

impl Peripherals {
    /// 32-bit register read; `None` if no device implements `addr`.
    pub fn read32(&mut self, addr: u32) -> Option<u32> {
        if let Some((n, reg)) = timer::decode(addr) {
            return Some(self.timers[n].read(reg));
        }
        self.intc
            .read(addr)
            .or_else(|| self.lcd.read(addr))
            .or_else(|| self.adc.read(addr))
            .or_else(|| self.uart.read(addr))
            .or_else(|| self.straps.and_then(|s| s.read(addr)))
            .or_else(|| self.clock_control.read(addr))
            .or_else(|| self.gpio.read(addr))
            .or_else(|| self.eeprom.read(addr))
            .or_else(|| self.voices.read(addr))
            .or_else(|| self.touch.as_ref().and_then(|t| t.read(addr)))
    }

    /// 32-bit register write; `false` if no device implements `addr`.
    pub fn write32(&mut self, addr: u32, value: u32) -> bool {
        match timer::decode(addr) {
            Some((n, reg)) => {
                self.timers[n].write(reg, value);
                true
            }
            None => {
                self.intc.write(addr, value)
                    || self.uart.write(addr, value)
                    || self.lcd.write(addr, value)
                    || self.clock_control.write(addr, value)
                    || self.touch_write(addr, value)
                    || self.audio_write(addr, value)
                    || self.eeprom.write(addr, value, &mut self.intc)
                    || self.voices.write(addr, value)
            }
        }
    }

    /// Byte/halfword write; only devices with byte-wide registers accept it.
    pub fn write_narrow(&mut self, addr: u32, value: u32) -> bool {
        self.uart.write(addr, value)
            || self.lcd.write(addr, value)
            || self.touch_write(addr, value)
            || self.audio_write(addr, value)
            || self.voices.write(addr, value)
    }

    /// Voice command: acknowledged (audio.rs) and executed (voices.rs).
    fn audio_write(&mut self, addr: u32, value: u32) -> bool {
        if !self.audio.write(addr, value, &mut self.intc) {
            return false;
        }
        if let Some(c) = self.audio.last_command {
            self.voices.command(c.command, c.voice);
        }
        true
    }

    fn touch_write(&mut self, addr: u32, value: u32) -> bool {
        match &mut self.touch {
            Some(t) => t.write(addr, value, &mut self.adc, &mut self.intc),
            None => false,
        }
    }

    /// Applies the deferred ticks (the CPU was running during all of them).
    pub fn catch_up(&mut self) {
        if self.deferred > 0 {
            let ticks = std::mem::take(&mut self.deferred);
            self.advance(ticks, false);
        }
    }

    /// Ticks from now until the earliest device event that can change an
    /// interrupt line: a timer wrap or a touch-model update.
    pub fn next_event(&self) -> u64 {
        let timers = self.timers.iter().map(|t| t.ticks_to_wrap()).min().unwrap_or(u64::MAX);
        let touch = self.touch.as_ref().map_or(u64::MAX, |t| t.ticks_to_next_update());
        timers.min(touch)
    }

    /// Advances all devices by `ticks` peripheral-clock counts.
    pub fn advance(&mut self, ticks: u64, cpu_halted: bool) {
        for t in &mut self.timers {
            t.advance(ticks, cpu_halted);
        }
        if let Some(t) = &mut self.touch {
            t.advance(ticks, &mut self.adc, &mut self.intc);
        }
    }

    /// Every CPU line `irq_outputs` drives, as a mask.
    pub fn irq_mask() -> u32 {
        TIMER_IRQS.iter().fold(0, |m, &l| m | 1 << l)
            | 1 << intc::PCM_IRQ_LINE
            | 1 << intc::DMA_IRQ_LINE
            | 1 << intc::ADC_IRQ_LINE
            | 1 << intc::VOICE_ACK_IRQ_LINE
    }

    /// Levels of the lines in `irq_mask` (same values as `irq_outputs`).
    pub fn irq_levels(&self) -> u32 {
        let mut l = 0;
        for (t, line) in self.timers.iter().zip(TIMER_IRQS) {
            l |= (t.pending as u32) << line;
        }
        l |= (self.intc.pcm_line() as u32) << intc::PCM_IRQ_LINE;
        l |= (self.intc.dma_line() as u32) << intc::DMA_IRQ_LINE;
        l |= (self.intc.adc_line() as u32) << intc::ADC_IRQ_LINE;
        l |= (self.intc.voice_ack_line() as u32) << intc::VOICE_ACK_IRQ_LINE;
        l
    }

    /// Interrupt outputs as (CPU line, asserted).
    pub fn irq_outputs(&self) -> impl Iterator<Item = (u8, bool)> + '_ {
        self.timers
            .iter()
            .zip(TIMER_IRQS)
            .map(|(t, line)| (line, t.pending))
            .chain(std::iter::once((intc::PCM_IRQ_LINE, self.intc.pcm_line())))
            .chain(std::iter::once((intc::DMA_IRQ_LINE, self.intc.dma_line())))
            .chain(std::iter::once((intc::ADC_IRQ_LINE, self.intc.adc_line())))
            .chain(std::iter::once((intc::VOICE_ACK_IRQ_LINE, self.intc.voice_ack_line())))
    }

    #[cfg(test)]
    fn levels_from_outputs(&self) -> (u32, u32) {
        self.irq_outputs().fold((0, 0), |(m, l), (line, on)| (m | 1 << line, l | (on as u32) << line))
    }

    fn log(&mut self, access: MmioAccess) {
        tracing::trace!(target: "ribbit::mmio", "{access}");
        if self.record_events {
            self.events.push(access);
        }
        self.mmio_tail.push(access);
    }

    pub fn take_events(&mut self) -> Vec<MmioAccess> {
        std::mem::take(&mut self.events)
    }
}

/// The CPU's view of the machine for one step.
pub struct MachineBus<'a> {
    pub sys: &'a mut SystemBus,
    pub io: &'a mut Peripherals,
    pub cycle: u64,
    pub pc: u32,
}

/// Registers whose reads have no side effects and whose values change only
/// at device events or register writes (so an idle loop polling them is
/// identical from one iteration to the next until an event).
fn idle_safe(addr: u32) -> bool {
    matches!(addr & !3, straps::CLOCK_ADDR | straps::ADDR | gpio::ADDR | intc::FLAGS_ADDR | intc::ENABLE_ADDR)
}

fn is_mmio(addr: u32) -> bool {
    addr.wrapping_sub(map::MMIO_BASE) < map::MMIO_SIZE
}

impl MachineBus<'_> {
    fn access(&mut self, addr: u32, width: u8, write: bool, value: u32, modelled: bool) {
        let stub = !write && !modelled && self.io.stubs.contains_key(&(addr & !3));
        let a = MmioAccess { cycle: self.cycle, pc: self.pc, addr, width, write, value, modelled, stub };
        if self.io.probe && (write || !idle_safe(addr)) {
            self.io.probe_violation = true;
        }
        if !modelled {
            let pcs = self.io.unmodelled_pcs.entry(addr).or_default();
            if pcs.len() < 16 && !pcs.contains(&self.pc) {
                pcs.push(self.pc);
            }
        }
        self.io.log(a);
    }

    /// Device register, or a debug stub, if one is configured.
    fn read_word(&mut self, addr: u32) -> (Option<u32>, bool) {
        match self.io.read32(addr) {
            Some(v) => (Some(v), true),
            None => (self.io.stubs.get(&addr).copied(), false),
        }
    }

    /// Sub-word read of a 32-bit register: the selected bytes of one
    /// register read (the device is accessed exactly once, since some reads,
    /// such as the ADC FIFO, have side effects). Returns (value, modelled).
    fn narrow_read(&mut self, addr: u32, width: u8) -> (Option<u32>, bool) {
        let (word, modelled) = self.read_word(addr & !3);
        let shift = (addr & 3) * 8;
        let mask = if width == 1 { 0xFF } else { 0xFFFF };
        (word.map(|w| (w >> shift) & mask), modelled)
    }
}

impl Bus for MachineBus<'_> {
    fn read8(&mut self, addr: u32) -> u8 {
        if !is_mmio(addr) {
            return self.sys.read8(addr);
        }
        self.io.catch_up();
        self.io.sync_needed = true;
        let (v, modelled) = self.narrow_read(addr, 1);
        self.access(addr, 1, false, v.unwrap_or(0), modelled);
        v.map_or_else(|| self.sys.read8(addr), |v| v as u8)
    }

    fn read16(&mut self, addr: u32) -> u16 {
        if !is_mmio(addr) {
            return self.sys.read16(addr);
        }
        self.io.catch_up();
        self.io.sync_needed = true;
        let addr = addr & !1;
        let (v, modelled) = self.narrow_read(addr, 2);
        self.access(addr, 2, false, v.unwrap_or(0), modelled);
        v.map_or_else(|| self.sys.read16(addr), |v| v as u16)
    }

    fn read32(&mut self, addr: u32) -> u32 {
        if !is_mmio(addr) {
            return self.sys.read32(addr);
        }
        self.io.catch_up();
        self.io.sync_needed = true;
        let addr = addr & !3;
        let (v, modelled) = self.read_word(addr);
        self.access(addr, 4, false, v.unwrap_or(0), modelled);
        v.unwrap_or_else(|| self.sys.read32(addr))
    }

    // Sub-word writes are accepted only by byte-wide device registers (the
    // UART); elsewhere they have no documented meaning and are reported as
    // unmodelled rather than guessed.
    fn write8(&mut self, addr: u32, value: u8) {
        if !is_mmio(addr) {
            return self.sys.write8(addr, value);
        }
        self.io.catch_up();
        self.io.sync_needed = true;
        let handled = self.io.write_narrow(addr, value as u32);
        self.access(addr, 1, true, value as u32, handled);
        if !handled {
            self.sys.write8(addr, value);
        }
    }

    fn write16(&mut self, addr: u32, value: u16) {
        if !is_mmio(addr) {
            return self.sys.write16(addr, value);
        }
        self.io.catch_up();
        self.io.sync_needed = true;
        let addr = addr & !1;
        let handled = self.io.write_narrow(addr, value as u32);
        self.access(addr, 2, true, value as u32, handled);
        if !handled {
            self.sys.write16(addr, value);
        }
    }

    fn write32(&mut self, addr: u32, value: u32) {
        if !is_mmio(addr) {
            return self.sys.write32(addr, value);
        }
        self.io.catch_up();
        self.io.sync_needed = true;
        let addr = addr & !3;
        let handled = self.io.write32(addr, value);
        self.access(addr, 4, true, value, handled);
        if !handled {
            self.sys.write32(addr, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbit_memory::Firmware;

    #[test]
    fn irq_bitmask_matches_per_line_outputs() {
        let mut io = Peripherals::default();
        for flags in [0, intc::FLAG_DMA, intc::FLAG_ADC | intc::FLAG_VOICE_ACK, intc::FLAG_PCM, u32::MAX] {
            io.intc.flags = flags;
            io.intc.enable = flags.rotate_left(1) | intc::FLAG_DMA | intc::FLAG_VOICE_ACK;
            io.timers[1].pending = flags & 1 != 0;
            let (mask, levels) = io.levels_from_outputs();
            assert_eq!((Peripherals::irq_mask(), io.irq_levels()), (mask, levels), "flags {flags:#x}");
        }
    }

    #[test]
    fn clock_fast_path_matches_wide_arithmetic() {
        let (mut a, mut b) = (Clock::default(), Clock::default());
        for n in [1u64, 2, 3, 5, 7, 16, 17, 1, 1, 6, 1_000_003, u64::MAX / 2] {
            let fast = a.ticks(n);
            let wide = (n as u128 * b.peripheral_hz as u128 + b.remainder as u128) / b.cpu_hz as u128;
            b.remainder = ((n as u128 * b.peripheral_hz as u128 + b.remainder as u128) % b.cpu_hz as u128) as u64;
            assert_eq!(fast as u128, wide, "{n}");
            assert_eq!(a.remainder, b.remainder);
        }
    }

    #[test]
    fn clock_converts_without_drift() {
        let mut c = Clock::new(96_000_000, 16_000_000);
        let ticks: u64 = (0..600).map(|_| c.ticks(1)).sum();
        assert_eq!(ticks, 100, "6 CPU cycles per 16 MHz count at 96 MHz");
        assert_eq!(c.ticks(96_000_000), 16_000_000);
        let mut c = Clock::new(48_000_000, 16_000_000);
        assert_eq!(c.ticks(3), 1);
    }

    #[test]
    fn timer_registers_are_reachable_through_the_bus_and_logged() {
        let mut sys = SystemBus::new(&Firmware::from_bytes(vec![0; 4]).unwrap(), None);
        let mut io = Peripherals::default();
        let mut bus = MachineBus { sys: &mut sys, io: &mut io, cycle: 7, pc: 0x4000_0000 };
        bus.write32(0x0180_D808, 0xFFFF_FFFF);
        bus.write32(0x0180_D800, 0x1234_5678);
        assert_eq!(bus.read32(0x0180_D800), 0x1234_5678);
        assert_eq!(bus.read16(0x0180_D802), 0x1234, "narrow reads see the register bytes");
        assert_eq!(bus.read8(0x0180_D800), 0x78);
        // An unmodelled register still reads 0 and is reported.
        assert_eq!(bus.read32(0x0180_9004), 0);
        let log = io.mmio_tail.to_vec();
        assert_eq!(log.len(), 6);
        assert!(log[..5].iter().all(|a| a.modelled));
        assert!(!log[5].modelled);
        assert_eq!(sys.unmapped_total(), 1, "only the unmodelled access reaches the unmapped log");
    }

    #[test]
    fn debug_stubs_only_answer_unmodelled_reads_and_are_labelled() {
        let mut sys = SystemBus::new(&Firmware::from_bytes(vec![0; 4]).unwrap(), None);
        let mut io = Peripherals::default();
        io.stubs.insert(0x0180_9004, 0x0020_0000);
        io.stubs.insert(0x0180_D800, 0xDEAD_BEEF); // timer 2 is modelled: ignored
        let mut bus = MachineBus { sys: &mut sys, io: &mut io, cycle: 0, pc: 0 };
        assert_eq!(bus.read32(0x0180_9004), 0x0020_0000);
        assert_eq!(bus.read8(0x0180_9006), 0x20);
        assert_eq!(bus.read32(0x0180_D800), 0);
        let log = io.mmio_tail.to_vec();
        assert!(log[0].stub && log[1].stub && !log[2].stub);
        assert!(log[0].to_string().contains("debug stub"));
    }

    #[test]
    fn side_effecting_reads_happen_once_per_access() {
        let mut sys = SystemBus::new(&Firmware::from_bytes(vec![0; 4]).unwrap(), None);
        let mut io = Peripherals::default();
        io.adc.push(0, 1);
        io.adc.push(0, 2);
        let mut bus = MachineBus { sys: &mut sys, io: &mut io, cycle: 0, pc: 0 };
        assert_eq!(bus.read8(adc::DATA_ADDR), 1 << 5);
        assert_eq!(bus.read32(adc::DATA_ADDR), 2 << 5);
        assert_eq!(bus.read32(adc::STATUS_ADDR), adc::STATUS_EMPTY);
    }

    #[test]
    fn subword_writes_to_device_registers_are_not_guessed() {
        let mut sys = SystemBus::new(&Firmware::from_bytes(vec![0; 4]).unwrap(), None);
        let mut io = Peripherals::default();
        let mut bus = MachineBus { sys: &mut sys, io: &mut io, cycle: 0, pc: 0 };
        bus.write8(0x0180_D800, 0x55);
        assert_eq!(io.timers[2].count, 0);
        assert!(!io.mmio_tail.to_vec()[0].modelled);
    }
}
