//! Ribbit's window and taskbar icon: the frog face from the emoji font egui
//! ships, in Ribbit green (see frog_glyph.rs, shared with the .exe icon).

#[path = "frog_glyph.rs"]
mod frog_glyph;
#[allow(unused_imports)]
pub use frog_glyph::{FROG_GREEN, frog_glyph_rgba};

fn icon(size: u32) -> Option<winit::window::Icon> {
    winit::window::Icon::from_rgba(frog_glyph_rgba(size), size, size).ok()
}

/// Title-bar icon (Windows scales it down to 16-32 px).
pub fn window_icon() -> Option<winit::window::Icon> {
    icon(64)
}

/// Taskbar / Alt-Tab icon (Windows' "big" icon).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn taskbar_icon() -> Option<winit::window::Icon> {
    icon(256)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frog_fills_a_square_with_transparent_corners() {
        for size in [16, 64, 256] {
            let rgba = frog_glyph_rgba(size);
            assert_eq!(rgba.len(), (size * size * 4) as usize);
            assert_eq!(rgba[3], 0, "corner is transparent");
            let chin = ((size * 4 / 5 * size + size / 2) * 4) as usize;
            assert_eq!(rgba[chin..chin + 4], [FROG_GREEN[0], FROG_GREEN[1], FROG_GREEN[2], 255], "solid face");
        }
        assert!(window_icon().is_some() && taskbar_icon().is_some());
    }
}
