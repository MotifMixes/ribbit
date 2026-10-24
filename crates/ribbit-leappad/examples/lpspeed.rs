//! How much faster than real time the LeapPad runs on one core, unthrottled:
//! `lpspeed <base rom> [cartridge] [seconds]`. Prints the speed every
//! emulated second.
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mut m = ribbit_leappad::Machine::new(std::fs::read(&a[1]).expect("base ROM"));
    if let Some(c) = a.get(2) {
        m.insert_cartridge(std::fs::read(c).expect("cartridge"));
    }
    let seconds: u32 = a.get(3).map_or(10, |s| s.parse().expect("seconds"));
    let mut out = Vec::new();
    for s in 0..seconds {
        let t = Instant::now();
        for _ in 0..200 {
            m.run_cycles(ribbit_leappad::MACHINE_HZ / 200);
            out.resize(m.bus.sound.output.len(), 0);
            m.bus.sound.output.pop_into(&mut out);
        }
        println!("second {s}: {:.1}x real time", 1.0 / t.elapsed().as_secs_f64());
    }
}
