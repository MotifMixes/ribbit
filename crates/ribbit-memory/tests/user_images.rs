//! Tests against the user's own dumps. Skipped unless the environment
//! variables point at files; images are never stored in the repository.
//!
//! RIBBIT_CART=path/to/cart.bin cargo test -p ribbit-memory --test user_images -- --nocapture

use ribbit_memory::Cartridge;
use ribbit_memory::cartridge::scan_swf;

const SPONGEBOB_ALT_SHA1: &str = "258e5b006086a7fd4cfa4460f3008604a5d87e1d";

fn env_image(var: &str) -> Option<Vec<u8>> {
    let path = std::env::var_os(var)?;
    Some(std::fs::read(&path).unwrap_or_else(|e| panic!("{var}={}: {e}", path.to_string_lossy())))
}

#[test]
fn user_cartridge_loads_and_header_parses() {
    let Some(data) = env_image("RIBBIT_CART") else {
        eprintln!("RIBBIT_CART not set; skipping");
        return;
    };
    let cart = Cartridge::from_bytes(data).expect("cartridge validates");
    let header = cart.header().expect("LEAP header parses");
    eprintln!("{} known={:?} dir_entries={}", cart.id(), cart.known().map(|k| k.name), header.directory.len());

    if cart.id().sha1_hex() == SPONGEBOB_ALT_SHA1 {
        // Observed on this exact dump (500-11002); see docs/hardware-research.md.
        assert_eq!(header.field_04, 0x0100);
        let tags: Vec<u16> = header.directory.iter().map(|d| d.tag).collect();
        assert_eq!(tags, [0x1003, 0x1005, 0x1006, 0x100c, 0x2000]);
        let counts: Vec<usize> = header.directory.iter().map(|d| d.entries.len()).collect();
        assert_eq!(counts, [14, 1, 6, 1, 1]);
        let movies = scan_swf(cart.rom());
        assert_eq!(movies.len(), 29);
        assert!(movies.iter().all(|m| m.version == 5));
    }
}
