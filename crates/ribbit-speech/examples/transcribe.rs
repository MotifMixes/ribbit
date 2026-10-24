//! Transcribes WAV clips with a Whisper model: `transcribe <model dir> <clip.wav>...`
//! (8 kHz or 16 kHz mono 16-bit, as lpwords writes them). Prints each clip's
//! text and how long it took.
use ribbit_speech::{Recognizer, SAMPLE_HZ};
use std::time::Instant;

fn read_wav(path: &str) -> Vec<f32> {
    let d = std::fs::read(path).expect("wav");
    let rate = u32::from_le_bytes(d[24..28].try_into().unwrap()) as usize;
    let at = d.windows(4).position(|w| w == b"data").expect("data chunk") + 8;
    let s: Vec<f32> = d[at..].chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0).collect();
    // Up to 16 kHz by linear interpolation.
    let n = s.len() * SAMPLE_HZ / rate;
    let mut out: Vec<f32> = (0..n)
        .map(|i| {
            let x = i as f64 * rate as f64 / SAMPLE_HZ as f64;
            let k = x as usize;
            let f = (x - k as f64) as f32;
            s[k] * (1.0 - f) + s.get(k + 1).copied().unwrap_or(0.0) * f
        })
        .collect();
    let peak = out.iter().fold(1e-4f32, |m, v| m.max(v.abs()));
    out.iter_mut().for_each(|v| *v = *v / peak * 0.8);
    let mut padded = vec![0.0; SAMPLE_HZ / 4];
    padded.extend(out);
    padded.extend(vec![0.0; SAMPLE_HZ / 2]);
    padded
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let t = Instant::now();
    let mut r = Recognizer::load(std::path::Path::new(&a[1])).expect("model");
    println!("loaded in {:.2}s", t.elapsed().as_secs_f32());
    for clip in &a[2..] {
        let t = Instant::now();
        let text = r.transcribe(&read_wav(clip), None).expect("transcribe");
        println!("{clip}: {text:?} ({:.2}s)", t.elapsed().as_secs_f32());
    }
}
