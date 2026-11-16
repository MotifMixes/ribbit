//! Plays a title through `Tablet` (what the app does): lfa_tablet <apk> [frames] [tap frame].
//! Taps (520, 225) at the tap frame for 6 frames and writes the last picture to lfa_tablet.ppm
//! and the sound to lfa_tablet.wav.

use ribbit_lfandroid::tablet::Tablet;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apk = args.first().expect("usage: lfa_tablet <apk> [frames] [tap frame]");
    let frames: u32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(120);
    let tap: Option<u32> = args.get(2).and_then(|v| v.parse().ok());
    let start = std::time::Instant::now();
    let mut t = Tablet::open(apk.as_ref(), None, None).unwrap_or_else(|e| panic!("{e}"));
    println!("{}: started in {:?}, screen {:?}", t.package, start.elapsed(), t.screen());
    let start = std::time::Instant::now();
    let mut audio = Vec::new();
    for f in 0..frames {
        if Some(f) == tap {
            t.touch(Some((520.0, 225.0))).unwrap();
        } else if tap.is_some_and(|at| f == at + 6) {
            t.touch(None).unwrap();
        }
        if let Err(e) = t.frame() {
            println!("frame {f}: {e}");
            break;
        }
        t.take_audio(&mut audio);
    }
    println!("{frames} frames in {:?}", start.elapsed());
    let sl = &t.rt.sles;
    println!("sound: {} clip players ({} playing), {} queue callbacks, {} buffers queued, {} play-state changes", sl.mixer.voices.len(), sl.mixer.voices.values().filter(|v| v.playing).count(), sl.callbacks.len(), sl.queued.len(), sl.play_state.len());
    let (w, h, px) = t.picture().expect("no picture");
    let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
    for p in px {
        ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    std::fs::write("lfa_tablet.ppm", ppm).unwrap();
    let rate = ribbit_lfandroid::mixer::RATE;
    let mut wav = Vec::new();
    for (id, n) in [(&b"RIFF"[..], 36 + 2 * audio.len() as u32)] {
        wav.extend_from_slice(id);
        wav.extend_from_slice(&n.to_le_bytes());
    }
    wav.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 1 | 1 << 16, rate, 2 * rate, 2 | 16 << 16] {
        wav.extend_from_slice(&v.to_le_bytes());
    }
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(2 * audio.len() as u32).to_le_bytes());
    for s in &audio {
        wav.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write("lfa_tablet.wav", wav).unwrap();
    println!("sound: {:.1} s, peak {}", audio.len() as f64 / rate as f64, audio.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0));
}
