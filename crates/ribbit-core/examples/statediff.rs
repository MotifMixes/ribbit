//! Save-state and run-partition exactness: a run of N steps must equal two
//! runs of N/2, and a machine restored from a state must continue exactly.
//!   cargo run --release -p ribbit-core --example statediff -- <bios> [cart] <steps_before> <steps_after>
use ribbit_core::Machine;
use ribbit_debug::Breakpoints;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let (bios, cart, before, after) = match &a[..] {
        [b, before, after] => (b, None, before, after),
        [b, c, before, after] => (b, Some(c), before, after),
        _ => panic!("usage: statediff <bios> [cart] <steps_before> <steps_after>"),
    };
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(bios).unwrap()).unwrap();
    let cart = cart.map(|c| ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(c).unwrap()).unwrap());
    let make = || Machine::with_straps(fw.clone(), cart.clone(), 0x63FF_FFFF);
    let none = Breakpoints::default();
    let mut m = make();
    m.idle_skip = false;
    m.run(before.parse().unwrap(), &none);
    let state = m.save_state();
    let total: u64 = after.parse().unwrap();
    let fresh = || {
        let mut n = make();
        n.idle_skip = false;
        n.load_state(&state).unwrap();
        n
    };
    // Partition: one run vs two halves, both from the restored state.
    let (mut x, mut y) = (fresh(), fresh());
    x.run(total, &none);
    y.run(total / 2, &none);
    y.run(total - total / 2, &none);
    println!(
        "partition: one run {} cycles, two halves {} cycles, same state: {}",
        x.cpu.cycles,
        y.cpu.cycles,
        x.state_digest() == y.state_digest()
    );
    // Restored vs original.
    m.run(total, &none);
    println!(
        "restore: original {} cycles, restored {} cycles, same state: {}",
        m.cpu.cycles,
        x.cpu.cycles,
        m.state_digest() == x.state_digest()
    );
}
