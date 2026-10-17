//! RESEARCH: device fonts' outlines in cartridges (swf_font.rs), per font:
//! the glyph count in each image that has it.
//!   cargo run --release -p ribbit-core --example fonts -- <cart>...
fn main() {
    for path in std::env::args().skip(1) {
        let Ok(data) = ribbit_core::read_image(&path) else { continue };
        let mut fonts: Vec<(String, usize)> =
            ribbit_core::swf_font::find_fonts(&data).into_iter().map(|(n, f)| (n, f.glyphs.values().filter(|o| !o.pieces.is_empty()).count())).collect();
        fonts.sort();
        println!("{}: {fonts:?}", std::path::Path::new(&path).file_name().unwrap().to_string_lossy());
    }
}
