//! Runs a Zippity for a number of frames and saves the last picture.
//!
//! zrun <nand.bin> <frames> <out.ppm> [pad script: state:from:to;...]
//!
//! ZRUN_FROM=n times only frames n onwards (e.g. a game, after the menus).

use ribbit_zippity::{SCREEN_H, SCREEN_W, Zippity};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let nand = std::fs::read(&args[1]).expect("read the NAND image");
    let frames: u64 = args[2].parse().expect("a frame count");
    let mut z = Zippity::new(nand);
    z.probe().no_idle_skip = std::env::var_os("ZRUN_NO_SKIP").is_some();
    if let Some(script) = args.get(4) {
        for item in script.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            let state = u16::from_str_radix(f[0], 16).unwrap();
            z.probe().pad_script.push((state, f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    let from: u64 = std::env::var("ZRUN_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut start = Instant::now();
    let (mut timed_from, mut skipped_from) = (0, 0);
    let mut samples = 0;
    let mut audio_hash = 0u64;
    for frame in 0..frames {
        if frame == from {
            start = Instant::now();
            (timed_from, skipped_from) = (z.instructions(), z.skipped());
        }
        z.run_frame();
        let audio = z.take_audio();
        samples += audio.len() / 2;
        for s in audio {
            audio_hash = (audio_hash ^ s as u16 as u64).wrapping_mul(0x0100_0000_01B3);
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let timed = z.instructions() - timed_from;
    println!(
        "{timed} instructions in {secs:.2} s: {:.1} M/s ({:.2}x real time at 60 M/s); {samples} sound samples",
        timed as f64 / secs / 1e6,
        timed as f64 / secs / 60e6
    );
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut img);
    let mut out = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    for &p in &img {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    std::fs::write(&args[3], out).unwrap();
    let image_hash = img.iter().fold(0u64, |h, &p| (h ^ p as u64).wrapping_mul(0x0100_0000_01B3));
    println!("sound {audio_hash:016x}, picture {image_hash:016x}");
    let skipped = z.skipped() - skipped_from;
    println!("skipped in idle loops: {skipped} ({:.1}%)", 100.0 * skipped as f64 / timed as f64);
    if std::env::var_os("ZRUN_LOG").is_some() {
        print!("{}", z.log());
    }
}
