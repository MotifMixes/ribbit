//! RESEARCH: pixel art traced into shapes (pixelart.rs), from the LCD half of
//! an hdplay snapshot (LCD | HD, `scale` canvas pixels per LCD pixel):
//!   cargo run --release -p ribbit-core --example pixtrace -- <shot.png> <scale> <out.png> [out_scale]
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [shot, scale, out, rest @ ..] = &a[..] else { panic!("usage: pixtrace <shot.png> <scale> <out.png> [out_scale]") };
    let (scale, k): (usize, usize) = (scale.parse().unwrap(), rest.first().map_or(8, |v| v.parse().unwrap()));
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(shot).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let ch = info.color_type.samples();
    let px: Vec<u32> = (0..160 * 160)
        .map(|i| {
            let (x, y) = (i % 160 * scale, i / 160 * scale);
            let o = (y * info.width as usize + x) * ch;
            0xFF00_0000 | (buf[o] as u32) << 16 | (buf[o + 1] as u32) << 8 | buf[o + 2] as u32
        })
        .collect();
    let t0 = Instant::now();
    let t = ribbit_core::pixelart::Traced::new(160, 160, &px);
    let t1 = t0.elapsed();
    let img = t.render(k);
    println!("trace {:?}, draw at {k}x {:?}", t1, t0.elapsed() - t1);
    let n = 160 * k;
    let mut rgb = vec![0u8; n * 2 * n * 3];
    for y in 0..n {
        for x in 0..n {
            let l = px[(y / k) * 160 + x / k];
            for (ox, p) in [(0, l), (n, img[y * n + x])] {
                let o = (y * 2 * n + ox + x) * 3;
                rgb[o..o + 3].copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
            }
        }
    }
    let mut e = png::Encoder::new(std::fs::File::create(out).unwrap(), (2 * n) as u32, n as u32);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(&rgb).unwrap();
}
