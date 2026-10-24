//! Leaves the pad alone and reports when the firmware's idle power-off
//! happens and whether the processor then stops: `lpidle <bios> <cart> [seconds]`.
use ribbit_leappad::{MACHINE_HZ, Machine};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = Machine::new(std::fs::read(&a[1]).expect("BIOS"));
    m.insert_cartridge(std::fs::read(&a[2]).expect("cartridge"));
    let secs: u32 = a.get(3).map_or(400, |s| s.parse().expect("seconds"));
    let mut said = String::new();
    for t in 0..secs * 10 {
        m.run_cycles(MACHINE_HZ / 10);
        let log = m.take_log();
        for line in log.lines().filter(|l| l.contains("Power") || l.contains("power")) {
            println!("{:6.1} s: {}", t as f32 / 10.0, line.trim());
        }
        said.push_str(&log);
        if m.is_off() {
            println!("{:6.1} s: the processor powered down (pad off)", t as f32 / 10.0);
            return;
        }
    }
    println!("still on after {secs} s; last log:\n{}", &said[said.len().saturating_sub(400)..]);
}
