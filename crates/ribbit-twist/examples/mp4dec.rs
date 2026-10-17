//! Research: decodes an AVI's MPEG-4 video with `ribbit_twist::mpeg4` and,
//! given FFmpeg's decoding of the same file (raw yuv420p), reports how
//! close each frame is (PSNR of the luma):
//!   ffmpeg -i video.avi -fps_mode passthrough -f rawvideo -pix_fmt yuv420p ref.yuv
//!   cargo run --release -p ribbit-twist --example mp4dec -- video.avi [ref.yuv] [out.yuv]

use ribbit_twist::mpeg4::{Decoder, Picture};

/// The video chunks ("00dc") of an AVI's movi list, in order.
fn chunks(avi: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let Some(movi) = avi.windows(4).position(|w| w == b"movi") else { return out };
    let mut at = movi + 4;
    while at + 8 <= avi.len() {
        let id = &avi[at..at + 4];
        let size = u32::from_le_bytes(avi[at + 4..at + 8].try_into().unwrap()) as usize;
        if id == b"LIST" {
            at += 12;
            continue;
        }
        if id == b"idx1" {
            break;
        }
        if &id[2..] == b"dc" || &id[2..] == b"db" {
            out.push(&avi[at + 8..(at + 8 + size).min(avi.len())]);
        }
        at += 8 + size + (size & 1);
    }
    out
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let avi = std::fs::read(&a[0]).expect("reading the AVI");
    let reference = a.get(1).map(|p| std::fs::read(p).expect("reading the reference"));
    let mut out = a.get(2).map(|p| std::fs::File::create(p).expect("creating the output"));
    let mut dec = Decoder::new();
    let mut last: Option<Picture> = None;
    let (mut worst, mut sum, mut n) = (f64::MAX, 0.0, 0);
    let mut errors = 0;
    let clock = std::time::Instant::now();
    for (i, c) in chunks(&avi).iter().enumerate() {
        // An empty chunk repeats the frame before: skipped, as FFmpeg with
        // -fps_mode passthrough does.
        if c.is_empty() {
            continue;
        }
        let pic = match dec.decode(c, last.as_ref()) {
            Ok(p) => p,
            Err(e) => {
                errors += 1;
                if errors < 10 {
                    println!("frame {i}: {e:?}");
                }
                continue;
            }
        };
        let (w, h, s) = (pic.width, pic.height, pic.width.div_ceil(16) * 16);
        if let Some(f) = &mut out {
            use std::io::Write;
            for y in 0..h {
                f.write_all(&pic.y[y * s..y * s + w]).unwrap();
            }
            for plane in [&pic.u, &pic.v] {
                for y in 0..h / 2 {
                    f.write_all(&plane[y * s / 2..y * s / 2 + w / 2]).unwrap();
                }
            }
        }
        if let Some(r) = &reference {
            let size = w * h * 3 / 2;
            let Some(frame) = r.get(n * size..(n + 1) * size) else { break };
            let mse: f64 = (0..h)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .map(|(x, y)| (frame[y * w + x] as f64 - pic.y[y * s + x] as f64).powi(2))
                .sum::<f64>()
                / (w * h) as f64;
            let psnr = if mse == 0.0 { 99.0 } else { 10.0 * (255.0 * 255.0 / mse).log10() };
            if psnr < worst {
                worst = psnr;
                println!("frame {n}: {psnr:.1} dB (worst so far)");
            }
            sum += psnr;
        }
        n += 1;
        last = Some(pic);
    }
    println!(
        "{n} frames in {:.2} s, {errors} errors; mean {:.1} dB, worst {:.1} dB",
        clock.elapsed().as_secs_f64(),
        sum / n.max(1) as f64,
        worst
    );
}
