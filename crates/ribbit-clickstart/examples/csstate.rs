//! Checks save states: runs a ClickStart to frame `at`, saves a state, runs
//! `more` frames (typing a name meanwhile), then loads the state into the
//! same machine and into a new one and runs the same frames again. Every
//! frame's picture and sound must match the first run. A state leaves the
//! EEPROM's contents out, so they are put back as they were too.
//!
//! csstate <bios.bin> <at> <more> [cartridge.bin]

use ribbit_clickstart::{ClickStart, SCREEN_H, SCREEN_W};

/// Runs `more` frames from frame `at`, typing "abc" from the 20th, and
/// returns a hash of each frame's picture and sound.
fn run(cs: &mut ClickStart, at: u64, more: u64) -> Vec<u64> {
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    let mut hashes = Vec::new();
    for f in at..at + more {
        let k = f - at;
        if (20..50).contains(&k) {
            let c = b"abc"[((k - 20) / 10) as usize] as char;
            match (k - 20) % 10 {
                0 => cs.key(ribbit_clickstart::key_code(c).unwrap(), true),
                4 => cs.key(0, false),
                _ => {}
            }
        }
        cs.run_frame();
        cs.render(&mut img);
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let audio = cs.take_audio();
        for v in img.iter().copied().chain(audio.iter().map(|&s| s as u16 as u32)) {
            h = (h ^ v as u64).wrapping_mul(0x100_0000_01b3);
        }
        hashes.push(h);
    }
    hashes
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let bios = std::fs::read(&args[1]).expect("read the system ROM");
    let at: u64 = args[2].parse().expect("a frame count");
    let more: u64 = args[3].parse().expect("a frame count");
    let cart = args.get(4).map(|p| std::fs::read(p).expect("read the cartridge"));
    let boot = || {
        let mut cs = ClickStart::new(&bios);
        if let Some(c) = &cart {
            cs.insert_cartridge(c);
            cs.reset();
        }
        cs
    };
    let mut cs = boot();
    for _ in 0..at {
        cs.run_frame();
        cs.take_audio();
    }
    let state = cs.save_state();
    let eeprom = cs.eeprom().to_vec();
    println!("state: {} bytes", state.len());
    let first = run(&mut cs, at, more);
    cs.load_state(&state).expect("load the state");
    cs.set_eeprom(&eeprom);
    let again = run(&mut cs, at, more);
    let mut fresh = boot();
    fresh.load_state(&state).expect("load the state into a new machine");
    fresh.set_eeprom(&eeprom);
    let fresh_run = run(&mut fresh, at, more);
    for (name, h) in [("same machine", &again), ("new machine", &fresh_run)] {
        match first.iter().zip(h.iter()).position(|(a, b)| a != b) {
            None => println!("{name}: all {more} frames identical"),
            Some(i) => println!("{name}: frame {} differs", at + i as u64),
        }
    }
}
