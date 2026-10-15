// Ribbit's frog: the frog-face emoji from Noto Emoji (SIL Open Font License,
// shipped with egui), drawn in Ribbit green. Shared with ribbit-desktop's
// build.rs, which `include!`s this file to make the .exe icon.

/// Ribbit green.
#[allow(dead_code)]
pub const FROG_GREEN: [u8; 3] = [0x3D, 0xC2, 0x5C];

/// RGBA pixels of the frog, `size` pixels square, on a transparent
/// background.
pub fn frog_glyph_rgba(size: u32) -> Vec<u8> {
    use ab_glyph::{Font, FontRef, point};
    let font = FontRef::try_from_slice(epaint_default_fonts::NOTO_EMOJI_REGULAR).expect("the emoji font");
    let id = font.glyph_id('\u{1F438}');
    let unit = font.outline_glyph(id.with_scale(100.0)).expect("a frog in the emoji font").px_bounds();
    let side = size as f32;
    let margin = (side * 0.04).max(1.0);
    let scale = 100.0 * (side - 2.0 * margin) / unit.width().max(unit.height());
    let glyph = font.outline_glyph(id.with_scale_and_position(scale, point(0.0, 0.0))).expect("frog outline");
    let b = glyph.px_bounds();
    let (ox, oy) = (((side - b.width()) / 2.0).round() as i32, ((side - b.height()) / 2.0).round() as i32);
    let n = size as usize;
    let mut cover = vec![0f32; n * n];
    glyph.draw(|x, y, c| {
        let (x, y) = (x as i32 + ox, y as i32 + oy);
        if (0..size as i32).contains(&x) && (0..size as i32).contains(&y) {
            cover[y as usize * n + x as usize] = c;
        }
    });
    let mut out = Vec::with_capacity(n * n * 4);
    for c in cover {
        let [r, g, b] = FROG_GREEN;
        out.extend_from_slice(&[r, g, b, (c.clamp(0.0, 1.0) * 255.0).round() as u8]);
    }
    out
}
