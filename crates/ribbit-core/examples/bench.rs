//! Throughput benchmark on the user's own BIOS dump (never committed):
//!   cargo run --release -p ribbit-core --example bench -- roms/bios/152-11265_2.1.bin [steps]
//! Prints emulated steps per second and the speed relative to 96 MHz.

use ribbit_core::Machine;
use ribbit_debug::Breakpoints;
use ribbit_memory::Firmware;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: bench <bios> [steps]");
    let steps: u64 = args.next().map_or(200_000_000, |s| s.parse().expect("steps"));
    let fw = Firmware::from_bytes(std::fs::read(&path).expect("read bios")).expect("firmware");
    let mut m = Machine::with_straps(fw, None, 0x63FF_FFFF);
    // Third argument "noskip": run every idle-loop iteration (verification).
    m.idle_skip = args.next().as_deref() != Some("noskip");
    let t = std::time::Instant::now();
    let r = m.run(steps, &Breakpoints::default());
    let s = t.elapsed().as_secs_f64();
    let rate = r.steps as f64 / s;
    println!(
        "{} steps in {s:.2} s: {:.1} M steps/s ({:.2}x real time at 96 MHz); idle-skipped {} {:?}; stop {:?}; digest {:016x}",
        r.steps,
        rate / 1e6,
        rate / 96e6,
        m.idle_skipped_steps(),
        m.idle_stats(),
        r.stop,
        m.state_digest()
    );
}
