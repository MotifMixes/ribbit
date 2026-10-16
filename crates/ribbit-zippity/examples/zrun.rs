//! Runs a Zippity for a number of frames and saves the last picture.
//!
//! zrun <nand.bin> <frames> <out.ppm> [pad script: state:from:to;...]

use ribbit_zippity::{SCREEN_H, SCREEN_W, Zippity};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let nand = std::fs::read(&args[1]).expect("read the NAND image");
    let frames: u64 = args[2].parse().expect("a frame count");
    let mut z = Zippity::new(nand);
    if let Some(script) = args.get(4) {
        for item in script.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            let state = u16::from_str_radix(f[0], 16).unwrap();
            z.probe().pad_script.push((state, f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    let start = Instant::now();
    let mut samples = 0;
    for _ in 0..frames {
        z.run_frame();
        samples += z.take_audio().len() / 2;
    }
    let secs = start.elapsed().as_secs_f64();
    println!(
        "{} instructions in {secs:.2} s: {:.1} M/s ({:.2}x real time at 60 M/s); {samples} sound samples",
        z.instructions(),
        z.instructions() as f64 / secs / 1e6,
        z.instructions() as f64 / secs / 60e6
    );
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut img);
    let mut out = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for p in img {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    std::fs::write(&args[3], out).unwrap();
    print!("{}", z.log());
}
