//! Writes Ribbit's artwork for the GitHub page: the icon (the frog on a dark
//! circle, as on Android) and the 1280 x 640 social preview (the frog and
//! the name), into a folder:
//!   cargo run --release -p ribbit-frontend --example repo_art -- <dir>

#[path = "../src/frog_glyph.rs"]
mod frog_glyph;

use ab_glyph::{Font, FontRef, PxScale, ScaleFont, point};

/// Ribbit's dark green-black (the Android icon's circle).
const BACKGROUND: [f32; 3] = [16.0, 22.0, 19.0];
const TEXT: [f32; 3] = [240.0, 246.0, 242.0];
const SUBTLE: [f32; 3] = [150.0, 170.0, 158.0];

struct Canvas {
    w: usize,
    h: usize,
    px: Vec<[f32; 4]>,
}

impl Canvas {
    fn new(w: usize, h: usize, fill: [f32; 4]) -> Self {
        Self { w, h, px: vec![fill; w * h] }
    }

    /// Paints `rgb` at (x, y) with coverage `a` (0..1).
    fn blend(&mut self, x: i64, y: i64, rgb: [f32; 3], a: f32) {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h || a <= 0.0 {
            return;
        }
        let p = &mut self.px[y as usize * self.w + x as usize];
        for k in 0..3 {
            p[k] = p[k] * (1.0 - a) + rgb[k] * a;
        }
        p[3] = p[3] + (255.0 - p[3]) * a;
    }

    /// A filled circle (anti-aliased edge).
    fn circle(&mut self, cx: f32, cy: f32, r: f32, rgb: [f32; 3]) {
        for y in (cy - r - 1.0) as i64..=(cy + r + 1.0) as i64 {
            for x in (cx - r - 1.0) as i64..=(cx + r + 1.0) as i64 {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                self.blend(x, y, rgb, (r - d + 0.5).clamp(0.0, 1.0));
            }
        }
    }

    /// The frog, `size` pixels square, with its top left at (x, y).
    fn frog(&mut self, x: usize, y: usize, size: u32) {
        let frog = frog_glyph::frog_glyph_rgba(size);
        for j in 0..size as usize {
            for i in 0..size as usize {
                let p = &frog[(j * size as usize + i) * 4..][..4];
                self.blend((x + i) as i64, (y + j) as i64, [p[0] as f32, p[1] as f32, p[2] as f32], p[3] as f32 / 255.0);
            }
        }
    }

    /// `text` in `font` at `size` px, its baseline starting at (x, y).
    fn text(&mut self, font: &FontRef, text: &str, size: f32, x: f32, y: f32, rgb: [f32; 3]) {
        let scaled = font.as_scaled(PxScale::from(size));
        let mut pen = x;
        let mut last = None;
        for c in text.chars() {
            let id = font.glyph_id(c);
            if let Some(prev) = last {
                pen += scaled.kern(prev, id);
            }
            if let Some(g) = font.outline_glyph(id.with_scale_and_position(size, point(pen, y))) {
                let b = g.px_bounds();
                g.draw(|gx, gy, a| self.blend(b.min.x as i64 + gx as i64, b.min.y as i64 + gy as i64, rgb, a));
            }
            pen += scaled.h_advance(id);
            last = Some(id);
        }
    }

    fn save(&self, path: &std::path::Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut e = png::Encoder::new(file, self.w as u32, self.h as u32);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let data: Vec<u8> = self.px.iter().flat_map(|p| p.map(|v| v.round().clamp(0.0, 255.0) as u8)).collect();
        e.write_header().unwrap().write_image_data(&data).unwrap();
    }
}

fn main() {
    let dir = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: repo_art <dir>"));
    std::fs::create_dir_all(&dir).unwrap();
    let bold = FontRef::try_from_slice(include_bytes!("../src/fonts/Fredoka-Bold.ttf")).unwrap();
    let medium = FontRef::try_from_slice(include_bytes!("../src/fonts/Fredoka-Medium.ttf")).unwrap();

    // The icon: the frog on a dark circle, transparent around it.
    let n = 512;
    let mut icon = Canvas::new(n, n, [0.0; 4]);
    icon.circle(n as f32 / 2.0, n as f32 / 2.0, n as f32 / 2.0, BACKGROUND);
    let s = (n as f32 * 0.72) as u32;
    icon.frog((n - s as usize) / 2, (n - s as usize) / 2, s);
    icon.save(&dir.join("ribbit-icon.png"));

    // The social preview: frog on the left, the name and what it is.
    let (w, h) = (1280, 640);
    let mut card = Canvas::new(w, h, [BACKGROUND[0], BACKGROUND[1], BACKGROUND[2], 255.0]);
    let frog = 340;
    let fx = 150;
    card.frog(fx, (h - frog as usize) / 2, frog);
    let tx = (fx + frog as usize + 70) as f32;
    card.text(&bold, "Ribbit", 168.0, tx, 330.0, TEXT);
    let lines = ["An emulator for LeapFrog's", "handhelds and TV consoles"];
    for (i, l) in lines.iter().enumerate() {
        card.text(&medium, l, 46.0, tx, 410.0 + i as f32 * 58.0, SUBTLE);
    }
    card.save(&dir.join("social-preview.png"));
}
