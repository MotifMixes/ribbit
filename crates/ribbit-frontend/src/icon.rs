//! Ribbit's window and taskbar icon: an original 32x32 pixel-art tree frog, drawn here so no image file (and no third-party artwork) is
//! needed.

/// One character per pixel; see `colour`. Left-right symmetric. Shared with
/// build.rs, which embeds the same frog in ribbit.exe.
const FROG_TXT: &str = include_str!("frog.txt");

fn rows() -> Vec<&'static str> {
    FROG_TXT.lines().collect()
}

fn colour(c: char) -> [u8; 4] {
    let rgb = match c {
        'g' => [0x6C, 0xCB, 0x3F], // tree-frog green
        'G' => [0x2F, 0x7D, 0x1E], // outline, smile
        'k' => [0x14, 0x14, 0x14], // pupils
        'w' => [0xFF, 0xFF, 0xFF], // eyes
        'o' => [0xFF, 0x98, 0x00], // toe pads
        'y' => [0xF1, 0xF8, 0xC8], // belly
        'b' => [0x3F, 0x7F, 0xD9], // blue flanks
        'p' => [0xFF, 0x8F, 0xB1], // cheeks
        _ => return [0, 0, 0, 0],
    };
    [rgb[0], rgb[1], rgb[2], 0xFF]
}

/// RGBA pixels of the frog at `scale` x 32 pixels square.
pub fn frog_rgba(scale: usize) -> (Vec<u8>, u32) {
    let size = 32 * scale;
    let mut out = Vec::with_capacity(size * size * 4);
    // Keep the palette in sync with build.rs.
    for y in 0..size {
        let row: Vec<char> = rows()[y / scale].chars().collect();
        for x in 0..size {
            out.extend_from_slice(&colour(row[x / scale]));
        }
    }
    (out, size as u32)
}

fn icon(scale: usize) -> Option<winit::window::Icon> {
    let (rgba, size) = frog_rgba(scale);
    winit::window::Icon::from_rgba(rgba, size, size).ok()
}

/// Title-bar icon (Windows scales it down to 16-32 px).
pub fn window_icon() -> Option<winit::window::Icon> {
    icon(2)
}

/// Taskbar / Alt-Tab icon (Windows' "big" icon).
#[cfg_attr(not(windows), allow(dead_code))]
pub fn taskbar_icon() -> Option<winit::window::Icon> {
    icon(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frog_is_32_square_symmetric_and_builds_icons() {
        let rows = rows();
        assert_eq!(rows.len(), 32);
        assert!(rows.iter().all(|r| r.chars().count() == 32));
        assert!(rows.iter().all(|r| r.chars().eq(r.chars().rev())), "mirror-symmetric");
        assert!(rows.iter().all(|r| r.chars().all(|c| c == '.' || colour(c)[3] == 0xFF)), "every letter has a colour");
        let (rgba, size) = frog_rgba(2);
        assert_eq!((size, rgba.len()), (64, 64 * 64 * 4));
        assert!(window_icon().is_some() && taskbar_icon().is_some());
    }
}
