//! Boot-progress tests against the user's own BIOS dumps (never committed).
//! Skipped unless RIBBIT_BIOS_DIR points at a folder with the MAME file names:
//!   RIBBIT_BIOS_DIR=roms/bios cargo test -p ribbit-core --release --test user_bios -- --nocapture
//! Addresses come from GNU objdump listings of these exact dumps; see
//! docs/hardware-research.md §Timers.

use ribbit_core::{Machine, StopReason};
use ribbit_debug::Breakpoints;
use ribbit_memory::Firmware;

const V15: &str = "155-10072-a.bin";
const V21: &str = "152-11265_2.1.bin";

fn firmware(name: &str) -> Option<Firmware> {
    let dir = std::env::var_os("RIBBIT_BIOS_DIR")?;
    let path = std::path::Path::new(&dir).join(name);
    let data = std::fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let fw = Firmware::from_bytes(data).unwrap();
    assert!(fw.known().is_some(), "{name} is not a known dump");
    Some(fw)
}

fn bios(name: &str) -> Option<Machine> {
    firmware(name).map(|fw| Machine::new(fw, None))
}

macro_rules! bios_or_skip {
    ($m:expr) => {
        match $m {
            Some(m) => m,
            None => {
                eprintln!("RIBBIT_BIOS_DIR not set; skipping");
                return;
            }
        }
    };
}

/// Runs until `pc` is about to execute; panics if it is not reached.
fn run_to(m: &mut Machine, pc: u32, budget: u64) -> u64 {
    let mut bp = Breakpoints::default();
    bp.add(pc);
    let r = m.run(budget, &bp);
    assert_eq!(r.stop, StopReason::Breakpoint { pc }, "did not reach {pc:#010x}: {:?}\n{:?}", r.stop, m.cpu);
    m.cpu.cycles
}

/// Runs `steps` steps; panics on anything but the step limit (no CPU error,
/// halt or unsupported instruction).
fn run_clean(m: &mut Machine, steps: u64) {
    let r = m.run(steps, &Breakpoints::default());
    assert_eq!(
        r.stop,
        StopReason::StepLimit,
        "stopped at {:#010x}
{:?}",
        r.pc,
        m.cpu
    );
}

/// Runs until the LCD format register reads `format`, in chunks; returns the
/// cycle count (to within one chunk).
fn run_until_format(m: &mut Machine, format: u32, budget: u64) -> u64 {
    const CHUNK: u64 = 100_000;
    let mut used = 0;
    while m.io.lcd.regs.format != format {
        assert!(used < budget, "LCD format never became {format:#x} (is {:#x})", m.io.lcd.regs.format);
        run_clean(m, CHUNK);
        used += CHUNK;
    }
    m.cpu.cycles
}

fn vram_contains(m: &Machine, text: &[u8]) -> Option<usize> {
    m.bus.vram().windows(text.len()).position(|w| w == text)
}

/// Shared boot milestones; each BIOS calls this with its own expected cycle
/// windows (measured per BIOS, never assumed equal).
fn check_display_boot(m: &mut Machine, lcd_init: std::ops::Range<u64>, format3: std::ops::Range<u64>) {
    // LcdMPI::Init: base 0, stride 0xF0 (160 px x 12 bpp), format 4 enabled.
    let c = run_until_format(m, 0x8000_0004, 10_000_000);
    assert!(lcd_init.contains(&c), "LCD init at cycle {c}");
    assert_eq!((m.io.lcd.regs.fb_base, m.io.lcd.regs.stride), (0, 0xF0));
    assert!(m.io.straps.unwrap().lcd_key() == 0x8000_0000, "default straps select panel config 2");

    // CopyToScreen: framebuffer DMA completes and its handler runs on vector 0x12.
    run_clean(m, 5_000_000);
    assert!(m.io.lcd.transfers >= 1, "no framebuffer DMA");
    assert_eq!(m.io.lcd.unknown_commands, 0);
    assert!(m.interrupt_counts[0x12] >= 1, "DMA-complete interrupt (vector 0x12) not taken");
    assert_eq!(m.interrupt_counts[0x12], m.io.lcd.transfers, "one DMA interrupt per transfer");
    assert_eq!(m.io.intc.flags & ribbit_core::io::intc::FLAG_DMA, 0, "handler acknowledged flag bit 14");

    // Later: the BIOS switches the display to format 3 (RGB332, stride 0xA0).
    let c = run_until_format(m, 0x8000_0003, 200_000_000);
    assert!(format3.contains(&c), "format 3 switch at cycle {c}");
    assert_eq!(m.io.lcd.regs.stride, 0xA0);

    // All three MMIO timer vectors and the DMA vector have been taken.
    for v in [0x12, 0x19, 0x1A, 0x1E] {
        assert!(m.interrupt_counts[v] > 0, "vector {v:#x} never taken: {:?}", &m.interrupt_counts[..32]);
    }
    let unexpected: Vec<usize> =
        (0..64).filter(|&v| m.interrupt_counts[v] > 0 && ![0x12, 0x19, 0x1A, 0x1E].contains(&v)).collect();
    assert!(unexpected.is_empty(), "unexpected vectors {unexpected:x?}");
    // Next blocker (docs/hardware-research.md): the BIOS enables ADC channels
    // 0 and 2 and waits for conversions; no ADC samples are generated.
    assert!(m.io.touch.is_none() && !m.experimental_touch_enabled(), "experimental touch is opt-in");
    assert!(m.io.adc.fifo.is_empty());
    assert_eq!(m.interrupt_counts[0x10], 0, "no ADC interrupt by default");
}

#[test]
fn v15_timer_waits_are_10_ms() {
    let mut m = bios_or_skip!(bios(V15));
    // First 10 ms wait: timer 2 reset to 0 at 0x4002ba36, polled at 0x4002ba3a
    // until >= 0x27100, released at 0x4002ba46. (The 1 ms wait at 0x4002b8c2
    // is on a different path and is not executed in this boot.)
    let c0 = run_to(&mut m, 0x4002_BA3A, 5_000_000);
    let c1 = run_to(&mut m, 0x4002_BA46, 5_000_000);
    // Second 10 ms wait at 0x4002ba66, released at 0x4002ba72.
    let c2 = run_to(&mut m, 0x4002_BA66, 5_000_000);
    let c3 = run_to(&mut m, 0x4002_BA72, 5_000_000);
    // 160,000 counts at 16 MHz with a 96 MHz CPU = 960,000 cycles. Measured
    // from the first poll (not the reset write), and the reset lands anywhere
    // within a 6-cycle peripheral tick, so allow -6, and up to one poll
    // iteration over (its uncached device read costs ~40 cycles: memtiming.rs).
    for (name, cycles) in [("first", c1 - c0), ("second", c3 - c2)] {
        assert!((959_994..960_100).contains(&cycles), "{name} 10 ms wait took {cycles} cycles");
    }
    // MQX BSP programs the tick timer (timer 0) after the waits.
    run_clean(&mut m, 5_000_000);
    let t0 = m.io.timers[0];
    assert_eq!((t0.control, t0.limit), (3, 0x0100_0000), "MQX tick timer programmed");
}

#[test]
fn v15_initialises_lcd_runs_dma_and_switches_display_format() {
    let mut m = bios_or_skip!(bios(V15));
    check_display_boot(&mut m, 2_900_000..3_200_000, 142_000_000..152_000_000);
}

#[test]
fn v21_initialises_lcd_runs_dma_and_switches_display_format() {
    let mut m = bios_or_skip!(bios(V21));
    // Same 10 ms wait sequence as v1.5, at 0x400390f6; released at 0x40039102.
    run_to(&mut m, 0x4003_9102, 5_000_000);
    check_display_boot(&mut m, 4_450_000..4_800_000, 162_000_000..173_000_000);
}

/// With all-zero straps no LCD config resource matches, and LcdMPI::Init
/// asserts. The BIOS formats the message into VRAM (and sends it to the UART,
/// interrupt-driven, line 0x1B).
fn check_zero_straps_assert(name: &str) {
    let fw = match firmware(name) {
        Some(fw) => fw,
        None => {
            eprintln!("RIBBIT_BIOS_DIR not set; skipping");
            return;
        }
    };
    let mut m = Machine::with_straps(fw, None, 0);
    run_clean(&mut m, 40_000_000);
    assert!(
        vram_contains(&m, b"LcdMPI::Init can't find config rsrc").is_some(),
        "{name}: assert text not in VRAM; uart {:?}",
        m.io.uart.text()
    );
    assert_eq!(m.io.lcd.regs.format, 0, "{name}: LCD never initialised");
}

#[test]
fn v15_zero_straps_reports_missing_lcd_config() {
    check_zero_straps_assert(V15);
}

#[test]
fn v21_zero_straps_reports_missing_lcd_config() {
    check_zero_straps_assert(V21);
}

// ---------------------------------------------------------------------------
// EXPERIMENTAL, UNVERIFIED: MAME's guessed touchscreen model (io/touch.rs).
// These tests pin what each BIOS does when fed MAME-style samples; they are
// regression markers for that experiment, not evidence of hardware accuracy.

use ribbit_core::io::touch::{MameTouchConfig, TouchPress};
use ribbit_video::{HEIGHT, WIDTH};

/// 96 MHz CPU.
const CYCLES_PER_MS: u64 = 96_000;

/// Raw ADC value for a pixel under MAME's crosshair mapping (0..=0x7F0 across
/// 160 pixels). The real panel mapping is UNKNOWN; calibration measures it.
fn raw(px: u32) -> u32 {
    (px * 0x7F0 + 159 / 2) / 159
}

/// The five targets both BIOSes draw, in order (observed in Ribbit).
const TARGETS: [(u32, u32); 5] = [(10, 10), (10, 150), (150, 150), (150, 10), (80, 80)];

fn calibration_presses() -> Vec<TouchPress> {
    TARGETS
        .iter()
        .enumerate()
        .map(|(n, &(x, y))| TouchPress { start_ms: 2000 + 1300 * n as u64, duration_ms: 300, x: raw(x), y: raw(y) })
        .collect()
}

/// Non-black pixels of the decoded LCD.
fn lit_pixels(m: &Machine) -> Vec<(usize, usize)> {
    let mut frame = Box::new([0u32; WIDTH * HEIGHT]);
    m.render_frame(&mut frame);
    (0..WIDTH * HEIGHT).filter(|&i| frame[i] & 0x00FF_FFFF != 0).map(|i| (i % WIDTH, i / WIDTH)).collect()
}

fn run_to_ms(m: &mut Machine, ms: u64) {
    let target = ms * CYCLES_PER_MS;
    assert!(m.cpu.cycles <= target);
    // A step takes 1 to 4 cycles (pipeline timing): approach the target.
    while m.cpu.cycles < target {
        run_clean(m, ((target - m.cpu.cycles) / 4).max(1));
    }
}

/// A plus-shaped 9-pixel target centred on (x, y).
fn assert_target_at(m: &Machine, x: usize, y: usize) {
    let mut lit = lit_pixels(m);
    lit.sort();
    let mut want =
        vec![(x, y - 2), (x, y - 1), (x - 2, y), (x - 1, y), (x, y), (x + 1, y), (x + 2, y), (x, y + 1), (x, y + 2)];
    want.sort();
    assert_eq!(lit, want, "calibration target at ({x},{y})");
}

/// Common part: calibration under the experimental model. Returns the machine
/// at 8.0 s, after the fifth press (6.9 s) has been handled.
fn experimental_calibration(name: &str) -> Option<Machine> {
    let mut m = bios(name)?;
    m.enable_experimental_mame_touch(MameTouchConfig { presses: calibration_presses() });
    m.coverage = Some(Default::default());
    // Stylus up so far: the BIOS has entered calibration (format 3) and draws
    // the first target.
    run_to_ms(&mut m, 1800); // v1.5 switches at ~1.53 s, v2.1 at ~1.75 s (2026-10-15 timing)
    assert_eq!(m.io.lcd.regs.format, 0x8000_0003);
    assert!(m.interrupt_counts[0x10] > 50, "ADC interrupts (vector 0x10) are taken");
    assert_target_at(&m, 10, 10);
    // Each accepted press moves the target on.
    for (n, &(x, y)) in TARGETS.iter().enumerate().skip(1) {
        run_to_ms(&mut m, 2000 + 1300 * n as u64 - 100);
        assert_target_at(&m, x as usize, y as usize);
    }
    run_to_ms(&mut m, 8300);
    assert_eq!(m.io.lcd.regs.format, 0x8000_0004, "calibration finished: back to 12 bpp");
    let t = m.io.touch.as_ref().unwrap();
    assert_eq!(t.touched_updates, 5 * 18, "each 300 ms press covers 18 updates");
    assert_eq!(t.dropped_samples, 0);
    assert_eq!(m.io.adc.empty_reads, 1, "only the boot-time drain reads an empty FIFO");
    Some(m)
}

/// The SysMgr power-off routine's `or r0,r0,0x100` before it stores to
/// 0x0180_B000 (routine at v1.5 0x4007_0240, v2.1 0x4008_0EEC). The register
/// is unmodelled and is also written during boot, so the executed instruction
/// is the evidence.
fn power_off_requested(m: &Machine, name: &str) -> bool {
    let pc = if name == V15 { 0x4007_0266 } else { 0x4008_0F12 };
    m.coverage.as_ref().expect("coverage enabled").contains_key(&pc)
}

#[test]
fn v15_experimental_touch_calibration_then_power_off_request() {
    let Some(m) = experimental_calibration(V15) else {
        eprintln!("RIBBIT_BIOS_DIR not set; skipping");
        return;
    };
    // Current blocker (docs/hardware-research.md): 0x0180_004C bit 11 reads 0
    // (unmodelled), which v1.5's SysMgr treats as an active-low shutdown
    // request (reason 4 at ~1.94 s), so after calibration it powers off.
    assert!(power_off_requested(&m, V15), "v1.5 requests power-off");
    assert_eq!(m.io.lcd.transfers, 1, "no splash screen is drawn");
}

#[test]
fn v21_experimental_touch_calibration_then_splash_screen() {
    let Some(mut m) = experimental_calibration(V21) else {
        eprintln!("RIBBIT_BIOS_DIR not set; skipping");
        return;
    };
    run_to_ms(&mut m, 9500);
    assert!(!power_off_requested(&m, V21), "v2.1 does not take the power-off path");
    assert!(m.io.lcd.transfers > 3, "post-calibration screens are DMA'd ({} transfers)", m.io.lcd.transfers);
    assert!(lit_pixels(&m).len() > 10_000, "a full-screen image is displayed");
}

// ---------------------------------------------------------------------------
// Audio voice-command acknowledge (io/audio.rs). Run with straps 0x63FF_FFFF
// (MAME's default; bit 14 set skips touch calibration) so calibration does not
// hide the audio path. This is an explicit test setting, not Ribbit's default.

const STRAPS_SKIP_CALIBRATION: u32 = 0x63FF_FFFF;

fn audio_machine(name: &str) -> Option<Machine> {
    let mut m = Machine::with_straps(firmware(name)?, None, STRAPS_SKIP_CALIBRATION);
    m.coverage = Some(Default::default());
    Some(m)
}

fn assert_first_voice_command_acknowledged(m: &Machine) {
    use ribbit_core::io::audio::VoiceCommand;
    // Both BIOSes start with 0x86: command 0x10 (start), voice 6.
    assert!(m.io.audio.commands >= 1, "a voice command was written to 0x0180_2070");
    assert!(m.interrupt_counts[0x1D] >= 1, "vector 0x1D (acknowledge) taken");
    assert_eq!(m.interrupt_counts[0x1D], m.io.audio.commands, "one acknowledge per command");
    assert_eq!(m.io.intc.flags & ribbit_core::io::intc::FLAG_VOICE_ACK, 0, "handler acknowledged flag bit 18");
    assert!(matches!(m.io.audio.last_command, Some(VoiceCommand { command: 0x10, .. })));
}

fn visited(m: &Machine, pc: u32) -> bool {
    m.coverage.as_ref().unwrap().contains_key(&pc)
}

#[test]
fn v15_voice_command_is_acknowledged_and_the_audio_task_continues() {
    let mut m = bios_or_skip!(audio_machine(V15));
    run_to_ms(&mut m, 1000); // first command at ~734 ms (2026-10-15 timing)
    assert_first_voice_command_acknowledged(&m);
    // 0x4002_6268: "last command for this voice still unacknowledged?" With
    // the acknowledge already taken, the event wait (0x4002_6286) is skipped
    // and the code continues at 0x4002_628A.
    assert!(visited(&m, 0x4002_628A));
    assert!(!visited(&m, 0x4002_6286), "no wait needed: the acknowledge came first");
}

#[test]
fn v21_voice_command_is_acknowledged_and_master_stop_audio_is_answered() {
    let mut m = bios_or_skip!(audio_machine(V21));
    run_to_ms(&mut m, 1300); // first command at ~1023 ms (2026-10-15 timing)
    assert_first_voice_command_acknowledged(&m);
    // The voice-6 check at 0x4002_FC50 (called from 0x4003_174E) used to wait
    // forever for event bit 1; now it continues at 0x4002_FC9E.
    assert!(visited(&m, 0x4002_FC9E));
    // PlayMovie (log call 0x4002_83D6) sends MasterStopAudio to AudioTask and
    // waits for the reply; 0x4002_C5A8 is reached only when it arrives.
    run_to(&mut m, 0x4002_83D6, 700_000_000);
    run_to(&mut m, 0x4002_C5A8, 50_000_000);
}

// ---------------------------------------------------------------------------
// GPIO input 0x0180_004C (io/gpio.rs): button bits active-low from the
// frontend; every other bit unresolved (0). v1.5's SysMgr still sees its
// shutdown line (bit 11) active and requests power-off; v2.1 does not.

#[test]
fn v15_buttons_read_released_and_unresolved_bit_11_still_requests_power_off() {
    let mut m = bios_or_skip!(audio_machine(V15));
    run_to_ms(&mut m, 4200);
    assert!(m.io.gpio.reads > 0, "the BIOS polls 0x0180_004C");
    assert_eq!(m.io.gpio.unresolved_lines, 0);
    assert!(power_off_requested(&m, V15), "bit 11 left unresolved: v1.5 powers off (~4.07 s)");
}

#[test]
fn v21_buttons_read_released_and_no_power_off() {
    let mut m = bios_or_skip!(audio_machine(V21));
    run_to_ms(&mut m, 4200);
    assert!(m.io.gpio.reads > 0, "the BIOS polls 0x0180_004C");
    assert!(!power_off_requested(&m, V21));
}

// ---------------------------------------------------------------------------
// System EEPROM (io/eeprom.rs) and voice engine (io/voices.rs).

#[test]
fn v15_eeprom_reads_complete_and_startup_voice_plays() {
    let mut m = bios_or_skip!(audio_machine(V15));
    run_to_ms(&mut m, 1000);
    assert!(m.io.eeprom.reads > 0, "EEPROM reads complete through flag bit 7");
    assert_eq!(m.io.eeprom.other_device_commands, 0);
    assert!(m.io.voices.samples_generated > 0, "voice 6 start-up sound produces samples");
    assert!(m.io.voices.status() & (1 << 5) != 0, "voice 6 reported playing (bit 5)");
}

#[test]
fn v21_eeprom_reads_complete_and_startup_voice_plays() {
    let mut m = bios_or_skip!(audio_machine(V21));
    run_to_ms(&mut m, 1300);
    assert!(m.io.eeprom.reads > 0, "EEPROM reads complete through flag bit 7");
    assert_eq!(m.io.eeprom.other_device_commands, 0);
    assert!(m.io.voices.samples_generated > 0, "voice 6 start-up sound produces samples");
}

/// Needs RIBBIT_BIOS_DIR and RIBBIT_CART (the SpongeBob 500-11002 dump).
/// EXPERIMENTAL touch model: calibrate, enter "BOB", OK; the game writes its
/// own save record (name + "SBSP" signature) to the cartridge save chip.
#[test]
fn v21_spongebob_name_entry_is_saved_to_the_cartridge() {
    let (Some(fw), Some(cart)) = (firmware(V21), std::env::var_os("RIBBIT_CART")) else {
        eprintln!("RIBBIT_BIOS_DIR / RIBBIT_CART not set; skipping");
        return;
    };
    let cart = ribbit_memory::Cartridge::from_bytes(std::fs::read(cart).unwrap()).unwrap();
    let mut m = Machine::new(fw, Some(cart));
    let tap = |at: u64, x: u32, y: u32| TouchPress { start_ms: at, duration_ms: 300, x: raw(x), y: raw(y) };
    let mut presses = calibration_presses();
    presses.extend([tap(23_000, 35, 43), tap(25_000, 12, 88), tap(27_000, 35, 43), tap(29_000, 112, 139)]);
    m.enable_experimental_mame_touch(MameTouchConfig { presses });
    run_to_ms(&mut m, 36_000);
    let save = m.io.eeprom.cart.as_ref().expect("cartridge save chip");
    let has = |pat: &[u8]| save.windows(pat.len()).any(|w| w == pat);
    assert!(m.io.eeprom.cart_dirty && has(b"BOB") && has(b"SBSP"), "save record written");
    assert!(m.io.eeprom.dirty, "calibration written to the system EEPROM");
    assert!(m.io.lcd.transfers > 100, "the game keeps drawing ({} DMAs)", m.io.lcd.transfers);
}

// ---------------------------------------------------------------------------
// Idle-loop fast-forwarding must be exact: identical state with it on or off.

fn idle_skip_is_exact(name: &str) {
    let Some(fw) = firmware(name) else {
        eprintln!("RIBBIT_BIOS_DIR not set; skipping");
        return;
    };
    let run = |skip: bool| {
        let mut m = Machine::with_straps(fw.clone(), None, STRAPS_SKIP_CALIBRATION);
        m.idle_skip = skip;
        let r = m.run(300_000_000, &Breakpoints::default());
        assert_eq!(r.stop, StopReason::StepLimit);
        (m.state_digest(), m.cpu.cycles, m.cpu.instructions_retired, m.idle_skipped_steps())
    };
    let (fast, slow) = (run(true), run(false));
    assert!(fast.3 > 100_000_000, "{name}: idle loop fast-forwarded ({} steps)", fast.3);
    assert_eq!(slow.3, 0);
    assert_eq!((fast.0, fast.1, fast.2), (slow.0, slow.1, slow.2), "{name}: same state, cycles and instructions");
    // As the window runs it: one frame's worth of steps per call.
    let mut m = Machine::with_straps(fw.clone(), None, STRAPS_SKIP_CALIBRATION);
    let mut steps = 0;
    for _ in 0..300_000_000 / 1_600_000 {
        steps += m.run(1_600_000, &Breakpoints::default()).steps;
    }
    m.run(300_000_000 - steps, &Breakpoints::default());
    assert!(m.idle_skipped_steps() > 100_000_000, "{name}: chunked runs fast-forward too");
    assert_eq!((m.state_digest(), m.cpu.cycles), (slow.0, slow.1), "{name}: chunked runs identical");
}

#[test]
fn v15_idle_skip_is_exact() {
    idle_skip_is_exact(V15);
}

#[test]
fn v21_idle_skip_is_exact() {
    idle_skip_is_exact(V21);
}

// ---------------------------------------------------------------------------
// Save states: a state loaded into a fresh machine continues identically.

fn save_state_is_exact(name: &str) {
    let Some(fw) = firmware(name) else {
        eprintln!("RIBBIT_BIOS_DIR not set; skipping");
        return;
    };
    let mut a = Machine::with_straps(fw.clone(), None, STRAPS_SKIP_CALIBRATION);
    a.run(150_000_000, &Breakpoints::default());
    let state = a.save_state();
    let mut b = Machine::with_straps(fw, None, STRAPS_SKIP_CALIBRATION);
    b.load_state(&state).expect("load");
    assert_eq!(a.state_digest(), b.state_digest(), "{name}: identical right after loading");
    a.run(150_000_000, &Breakpoints::default());
    b.run(150_000_000, &Breakpoints::default());
    assert_eq!(
        (a.state_digest(), a.cpu.cycles, a.cpu.instructions_retired, a.io.lcd.transfers),
        (b.state_digest(), b.cpu.cycles, b.cpu.instructions_retired, b.io.lcd.transfers),
        "{name}: identical 150 M steps later"
    );
    eprintln!("{name}: state size {} KiB", state.len() / 1024);
}

#[test]
fn v15_save_state_is_exact() {
    save_state_is_exact(V15);
}

#[test]
fn v21_save_state_is_exact() {
    save_state_is_exact(V21);
}
