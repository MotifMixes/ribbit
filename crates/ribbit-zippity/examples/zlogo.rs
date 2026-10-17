//! RESEARCH: the Zippity's start-up from power-on, one picture every `step`
//! frames up to `frames`, as numbered PPMs (for timing the library's art):
//!   cargo run --release -p ribbit-zippity --example zlogo -- <nand.bin> <frames> <step> <out_prefix>
use ribbit_zippity::{SCREEN_H, SCREEN_W};
use std::io::Write;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let [nand, frames, step, out] = &a[..] else { panic!("usage: zlogo <nand.bin> <frames> <step> <out_prefix>") };
    let (frames, step): (usize, usize) = (frames.parse().unwrap(), step.parse().unwrap());
    let mut z = ribbit_zippity::Zippity::new(std::fs::read(nand).unwrap());
    let mut frame = vec![0u32; SCREEN_W * SCREEN_H];
    for i in 1..=frames {
        z.run_frame();
        z.take_audio();
        if i % step == 0 {
            z.render(&mut frame);
            let mut f = std::fs::File::create(format!("{out}{i:04}.ppm")).unwrap();
            write!(f, "P6\n{SCREEN_W} {SCREEN_H}\n255\n").unwrap();
            let rgb: Vec<u8> = frame.iter().flat_map(|&p| [(p >> 16) as u8, (p >> 8) as u8, p as u8]).collect();
            f.write_all(&rgb).unwrap();
        }
    }
}
