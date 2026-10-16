//! When boot milestones happen (emulated ms and cycles), for re-pinning the
//! BIOS regression tests after a timing-model change:
//!   cargo run --release -p ribbit-core --example milestones -- <bios> <straps_hex> <ms>
use ribbit_core::Machine;
use ribbit_debug::Breakpoints;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [bios, straps, ms] = &a[..] else {
        panic!("usage: milestones <bios> <straps_hex> <ms>");
    };
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(bios).unwrap()).unwrap();
    let straps = u32::from_str_radix(straps.trim_start_matches("0x"), 16).unwrap();
    let mut m = Machine::with_straps(fw, None, straps);
    let none = Breakpoints::default();
    let per_ms = m.clock.cpu_hz / 1000;
    let end = ms.parse::<u64>().unwrap() * per_ms;
    let mut seen: Vec<String> = vec![];
    let mut note = |m: &Machine, what: String| {
        if !seen.contains(&what) {
            println!("{:9.2} ms  cycle {:>11}  {what}", m.cpu.cycles as f64 / per_ms as f64, m.cpu.cycles);
            seen.push(what);
        }
    };
    while m.cpu.cycles < end {
        m.run_cycles(per_ms / 10, &none);
        note(&m, format!("LCD format {:#x}", m.io.lcd.regs.format));
        if m.io.lcd.transfers > 0 {
            note(&m, "first framebuffer DMA".into());
        }
        if m.io.audio.commands > 0 {
            note(&m, "first voice command".into());
        }
        if m.io.eeprom.reads > 0 {
            note(&m, "first EEPROM read".into());
        }
        if m.io.voices.samples_generated > 0 {
            note(&m, "voice samples generated".into());
        }
        if m.io.timers[0].control == 3 {
            note(&m, "MQX tick timer programmed".into());
        }
    }
}
