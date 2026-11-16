//! Plays a title through `Tablet` (what the app does): lfa_tablet <apk> [frames] [tap frame].
//! Taps (520, 225) at the tap frame for 6 frames and writes the last picture to lfa_tablet.ppm.

use ribbit_lfandroid::tablet::Tablet;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let apk = args.first().expect("usage: lfa_tablet <apk> [frames] [tap frame]");
    let frames: u32 = args.get(1).and_then(|v| v.parse().ok()).unwrap_or(120);
    let tap: Option<u32> = args.get(2).and_then(|v| v.parse().ok());
    let start = std::time::Instant::now();
    let mut t = Tablet::open(apk.as_ref(), None).unwrap_or_else(|e| panic!("{e}"));
    println!("{}: started in {:?}, screen {:?}", t.package, start.elapsed(), t.screen());
    let start = std::time::Instant::now();
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
    }
    println!("{frames} frames in {:?}", start.elapsed());
    let (w, h, px) = t.picture().expect("no picture");
    let mut ppm = format!("P6 {w} {h} 255\n").into_bytes();
    for p in px {
        ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    std::fs::write("lfa_tablet.ppm", ppm).unwrap();
}
