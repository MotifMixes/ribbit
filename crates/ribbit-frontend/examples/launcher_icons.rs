//! Writes the Android launcher icons (the frog on a dark circle) at each
//! density, for scripts/build-apk.py:
//!   cargo run --release -p ribbit-frontend --example launcher_icons -- <res dir>

#[path = "../src/frog_glyph.rs"]
mod frog_glyph;

const DENSITIES: [(&str, u32); 5] = [("mdpi", 48), ("hdpi", 72), ("xhdpi", 96), ("xxhdpi", 144), ("xxxhdpi", 192)];
/// The circle behind the frog (Ribbit's dark green-black).
const BACKGROUND: [f32; 3] = [16.0, 22.0, 19.0];

fn icon(n: usize) -> Vec<u8> {
    let s = (n as f32 * 0.72) as usize;
    let frog = frog_glyph::frog_glyph_rgba(s as u32);
    let off = (n - s) / 2;
    let c = n as f32 / 2.0;
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let d = ((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt();
            let alpha = (c - d).clamp(0.0, 1.0);
            let mut px = BACKGROUND;
            if (off..off + s).contains(&x) && (off..off + s).contains(&y) {
                let p = &frog[((y - off) * s + (x - off)) * 4..][..4];
                let a = p[3] as f32 / 255.0;
                for k in 0..3 {
                    px[k] = px[k] * (1.0 - a) + p[k] as f32 * a;
                }
            }
            out[(y * n + x) * 4..][..4].copy_from_slice(&[px[0] as u8, px[1] as u8, px[2] as u8, (alpha * 255.0) as u8]);
        }
    }
    out
}

fn main() {
    let res = std::path::PathBuf::from(std::env::args().nth(1).expect("usage: launcher_icons <res dir>"));
    for (name, size) in DENSITIES {
        let dir = res.join(format!("mipmap-{name}"));
        std::fs::create_dir_all(&dir).unwrap();
        let file = std::fs::File::create(dir.join("ic_launcher.png")).unwrap();
        let mut e = png::Encoder::new(file, size, size);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        e.write_header().unwrap().write_image_data(&icon(size as usize)).unwrap();
    }
}
