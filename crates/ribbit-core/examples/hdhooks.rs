//! RESEARCH: which HD hooks (hd.rs signatures) an image holds, e.g. a
//! cartridge with its own copy of the Flash player:
//!   cargo run --release -p ribbit-core --example hdhooks -- <bios|cart>...
fn main() {
    for path in std::env::args().skip(1) {
        let data = ribbit_core::read_image(&path).unwrap();
        // BIOS images (by path) where the BIOS runs, cartridges at the cartridge window.
        let base = if path.to_lowercase().contains("bios") { ribbit_memory::map::BIOS_MIRROR } else { ribbit_memory::map::CART_BASE };
        println!("{path}: {:x?}", ribbit_core::hd::Hooks::find(&data, base));
        println!("    each: {:x?}", ribbit_core::hd::Hooks::find_each(&data, base));
    }
}
