fn main() {
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image("roms/bios/152-11265_2.1.bin").unwrap()).unwrap();
    let cart =
        ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(std::env::var("RIBBIT_CART").unwrap()).unwrap()).unwrap();
    let mut m = ribbit_core::Machine::with_straps(fw, Some(cart), 0x63FF_FFFF);
    m.load_state(&std::fs::read(std::env::var("RIBBIT_STATE").unwrap()).unwrap()).unwrap();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        let raw = m.snapshot();
        let a = t.elapsed();
        let s = raw.compress();
        println!(
            "snapshot {:.2} ms + compress {:.1} ms, {} KiB",
            a.as_secs_f64() * 1e3,
            (t.elapsed() - a).as_secs_f64() * 1e3,
            s.len() / 1024
        );
    }
}
