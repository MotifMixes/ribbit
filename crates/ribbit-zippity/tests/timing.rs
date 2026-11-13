//! Sound and timing against recordings of real units (docs/zippity.md,
//! "Speed" and "Sound unit"). The start-up checks need the system NAND dump:
//! RIBBIT_ZIPPITY_NAND=<file> cargo test -p ribbit-zippity -- --ignored

use ribbit_zippity::{OUTPUT_RATE, SCREEN_H, SCREEN_W, Zippity};

/// Sound comes at the sound unit's rate whatever the CPU runs: 2 s of
/// frames give exactly 2 s of samples.
#[test]
fn sound_samples_follow_the_sound_clock() {
    let mut z = Zippity::new(Vec::new());
    let mut pairs = 0;
    for _ in 0..120 {
        z.run_frame();
        pairs += z.take_audio().len() / 2;
    }
    assert_eq!(pairs as f64, 2.0 * OUTPUT_RATE);
}

/// What the start-up does, frame by frame: the picture's mean brightness
/// and the sound.
struct Startup {
    brightness: Vec<u32>,
    sound: Vec<i16>,
}

fn nand() -> Option<Vec<u8>> {
    let Some(path) = std::env::var_os("RIBBIT_ZIPPITY_NAND") else {
        eprintln!("RIBBIT_ZIPPITY_NAND not set; skipping");
        return None;
    };
    Some(std::fs::read(path).expect("read the NAND dump"))
}

fn start_up(frames: usize) -> Option<Startup> {
    let mut z = Zippity::new(nand()?);
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    let (mut brightness, mut sound) = (Vec::new(), Vec::new());
    for _ in 0..frames {
        z.run_frame();
        z.render(&mut img);
        let sum: u64 = img.iter().map(|&p| ((p >> 16 & 0xFF) + (p >> 8 & 0xFF) + (p & 0xFF)) as u64).sum();
        brightness.push((sum / (3 * img.len() as u64)) as u32);
        sound.extend(z.take_audio());
    }
    Some(Startup { brightness, sound })
}

/// The start-up's timeline against a recording of a real unit (60 fields a
/// second): the warning screen fades in (16 fields) and stays up 3.30 s from
/// the start of its fade, decoding the LeapFrog logo's pictures (the CPU
/// speed sets it), and the logo's sound starts 0.18 s after it goes (0.22 s
/// here, within the recording's frame steps).
/// Power-on to the warning is 2.10 s (2.17 s from the start of the
/// recording, whose power-on moment is not seen).
#[test]
#[ignore = "needs RIBBIT_ZIPPITY_NAND"]
fn start_up_timeline_matches_a_real_unit() {
    let Some(s) = start_up(400) else { return };
    // The warning fades in from black (16 fields, as on the recording).
    let on = s.brightness.iter().position(|&b| b > 2).expect("the warning screen");
    assert!(s.brightness[on + 4] < s.brightness[on + 10] && s.brightness[on + 10] < s.brightness[on + 20], "the warning fades in");
    // The warning's last field: its text block's brightness changes when the
    // next screen's background starts.
    let warning = s.brightness[on + 20];
    let off = on + 20 + s.brightness[on + 20..].iter().position(|&b| b.abs_diff(warning) > 2).expect("the warning goes");
    let sound = s.sound.chunks(2).position(|p| p[0].unsigned_abs() > 500 || p[1].unsigned_abs() > 500).expect("the first sound");
    let sound_at = sound as f64 / OUTPUT_RATE;
    eprintln!("warning at field {on}, gone at {off}; sound at {sound_at:.3} s");
    assert!((124..=130).contains(&on), "power-on to the warning: field {on}");
    assert!((off - on).abs_diff(198) <= 3, "the warning lasts {} fields, 198 on a real unit", off - on);
    let after = sound_at - off as f64 / 60.0;
    assert!((0.1..0.35).contains(&after), "the sound starts {after:.3} s after the warning");
}

/// The start-up's sound, sample for sample (a regression check for the
/// sound unit and its timing): its length, peak and hash over 15 s.
#[test]
#[ignore = "needs RIBBIT_ZIPPITY_NAND"]
fn start_up_sound_is_unchanged() {
    let Some(s) = start_up(900) else { return };
    let hash = s.sound.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &v| (h ^ v as u16 as u64).wrapping_mul(0x0100_0000_01B3));
    let peak = s.sound.iter().map(|v| v.unsigned_abs()).max().unwrap();
    eprintln!("sound: {} pairs, peak {peak}, hash {hash:016x}", s.sound.len() / 2);
    assert_eq!(s.sound.len() / 2, (900.0 / 60.0 * OUTPUT_RATE).round() as usize);
    assert_eq!(peak, 8121);
    assert_eq!(hash, 0x44de_74a4_f89a_b916);
}

/// Skipping idle loops changes nothing: sound, picture, memory and the CPU
/// come out the same as running every instruction (the start-up loads from
/// the NAND while polling its status, and waits for frames).
#[test]
#[ignore = "needs RIBBIT_ZIPPITY_NAND"]
fn idle_skip_is_bit_exact() {
    let Some(nand) = nand() else { return };
    let mut fast = Zippity::new(nand.clone());
    let mut slow = Zippity::new(nand);
    slow.probe().no_idle_skip = true;
    let (mut a, mut b) = (vec![0u32; SCREEN_W * SCREEN_H], vec![0u32; SCREEN_W * SCREEN_H]);
    for frame in 0..420 {
        fast.run_frame();
        slow.run_frame();
        assert_eq!(fast.take_audio(), slow.take_audio(), "sound, frame {frame}");
        assert_eq!(fast.cpu().r, slow.cpu().r, "CPU, frame {frame}");
        if frame % 30 == 29 {
            fast.render(&mut a);
            slow.render(&mut b);
            assert!(a == b, "picture, frame {frame}");
            assert!(fast.ram() == slow.ram(), "memory, frame {frame}");
        }
    }
    // About half: the controllers' packets come every millisecond.
    assert!(fast.skipped() > fast.instructions() * 2 / 5, "much of the start-up is skipped");
}
