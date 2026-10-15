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
    let fw = Firmware::from_bytes(ribbit_core::read_image(&path).expect("read bios")).expect("firmware");
    let cart = std::env::var("RIBBIT_CART")
        .ok()
        .map(|c| ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(&c).expect("read cart")).expect("cartridge"));
    let mut m = Machine::with_straps(fw, cart, 0x63FF_FFFF);
    // Optional: RIBBIT_CART (cartridge) and RIBBIT_STATE (save state to start from).
    if let Ok(state) = std::env::var("RIBBIT_STATE") {
        m.load_state(&std::fs::read(&state).expect("read state")).expect("load state");
    }
    // Third argument "noskip": run every idle-loop iteration (verification).
    m.idle_skip = args.next().as_deref() != Some("noskip");
    // RIBBIT_NO_TAIL=1: no per-instruction trace history, as in the GUI.
    m.record_trace_tail = std::env::var_os("RIBBIT_NO_TAIL").is_none();
    let retired0 = m.cpu.instructions_retired;
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
    println!(
        "batches {:?} (batches, batched instructions, steps: not tried / no budget / stopped at once); first cart pc {:x?}",
        m.batch_stats, m.first_cart_pc
    );
    if std::env::var_os("MMIO_TAIL").is_some() {
        for a in m.io.mmio_tail.to_vec() {
            println!("{a}");
        }
    }
    println!("batch exits (budget, slow, bus, backward, error): {:?}", m.batch_exits);
    let c = &m.cpu.icache;
    println!(
        "decode cache: {} hits, {} misses, {} uncached fetches (exact with noskip)",
        c.hits,
        c.misses,
        (m.cpu.instructions_retired - retired0).saturating_sub(c.hits + c.misses)
    );
}
