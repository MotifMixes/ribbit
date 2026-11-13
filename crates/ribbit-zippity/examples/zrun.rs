//! Runs a Zippity for a number of frames and saves the last picture.
//!
//! zrun <nand.bin> <frames> <out.ppm> [pad script: state:from:to;...]
//!
//! ZRUN_FROM=n times only frames n onwards (e.g. a game, after the menus);
//! ZRUN_SLOW=n prints the n slowest frames (a frame over 16.7 ms would
//! starve the sound in the app); ZRUN_RENDER draws every frame, as the app
//! does; ZRUN_WAV=file saves the sound; ZRUN_SIG=file saves each frame's
//! picture as 8 x 6 blocks of mean R, G, B (144 bytes a frame: for timing
//! screens against a recording); ZRUN_LOGT prints the system's log lines as
//! they come, with the time (60 frames a second).
//!
//! Pad script times are instruction counts, or frames with an `f` suffix
//! (`7f:1200f;...`). ZRUN_CART=file puts a cartridge (a raw dump) in the
//! slot; ZRUN_SHOTS=n saves the picture every n frames (`<out>.<frame>.ppm`),
//! ZRUN_SHOTS=dir:n into dir (`dir/NNNNN.ppm`, the frame number);
//! ZRUN_SAVE=prefix:frame saves the machine (`prefix.state`, with the NAND
//! in `prefix.nand`) after that frame, ZRUN_LOAD=prefix carries on from it
//! (the frame count then counts on from there). ZRUN_IO prints every I/O
//! register's access counts; ZRUN_RAM=file saves the first 16 MB of RAM at
//! the end; ZRUN_LEVELS prints the sound's peak level each 60 frames;
//! ZRUN_SPU prints each sound channel start (time in instructions);
//! ZRUN_RADIO=n sends the controllers' packets every n instructions.

use ribbit_zippity::{CLOCK, OUTPUT_RATE, SCREEN_H, SCREEN_W, Zippity};
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let load = std::env::var("ZRUN_LOAD").ok();
    let nand_path = load.as_ref().map_or(args[1].clone(), |p| format!("{p}.nand"));
    let nand = std::fs::read(&nand_path).expect("read the NAND image");
    let frames: u64 = args[2].parse().expect("a frame count");
    let mut z = Zippity::new(nand);
    if let Ok(path) = std::env::var("ZRUN_CART") {
        z.insert_cartridge(std::fs::read(&path).expect("read the cartridge"));
    }
    if let Some(p) = &load {
        z.load_state(&std::fs::read(format!("{p}.state")).expect("read the state")).expect("a state");
    }
    z.probe().enabled = std::env::var_os("ZRUN_IO").is_some();
    let wav = std::env::var("ZRUN_WAV").ok();
    let sig_path = std::env::var("ZRUN_SIG").ok();
    let mut sig = Vec::new();
    let mut all_audio = Vec::new();
    // ZRUN_SHOTS: `dir:n` or `n` (next to the output picture).
    let shots: Option<(Option<String>, u64)> = std::env::var("ZRUN_SHOTS").ok().and_then(|s| match s.rsplit_once(':') {
        Some((dir, n)) => Some((Some(dir.to_string()), n.parse().ok()?)),
        None => Some((None, s.parse().ok()?)),
    });
    let save = std::env::var("ZRUN_SAVE").ok().map(|v| {
        let (p, f) = v.rsplit_once(':').expect("ZRUN_SAVE=prefix:frame");
        (p.to_string(), f.parse::<u64>().expect("a frame"))
    });
    let first = z.instructions() / ribbit_zippity::FRAME;
    let time = |t: &str| -> u64 {
        match t.strip_suffix('f') {
            Some(f) => f.parse::<u64>().unwrap() * ribbit_zippity::FRAME,
            None => t.parse().unwrap(),
        }
    };
    z.probe().no_idle_skip = std::env::var_os("ZRUN_NO_SKIP").is_some();
    z.probe().radio_period = std::env::var("ZRUN_RADIO").ok().map(|v| v.parse().expect("ZRUN_RADIO=instructions"));
    z.probe().spu_log = std::env::var_os("ZRUN_SPU").is_some();
    if let Some(script) = args.get(4) {
        for item in script.split(';').filter(|i| !i.is_empty()) {
            let f: Vec<&str> = item.split(':').collect();
            let state = u16::from_str_radix(f[0], 16).unwrap();
            z.probe().pad_script.push((state, time(f[1]), time(f[2])));
        }
    }
    let from: u64 = std::env::var("ZRUN_FROM").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    let mut start = Instant::now();
    let (mut timed_from, mut skipped_from) = (0, 0);
    let mut samples = 0;
    let mut audio_hash = 0u64;
    let mut slow: Vec<(f64, u64)> = Vec::new();
    let levels = std::env::var_os("ZRUN_LEVELS").is_some();
    let mut peak = 0u16;
    let render_each = std::env::var_os("ZRUN_RENDER").is_some();
    let mut frame_img = vec![0u32; SCREEN_W * SCREEN_H];
    let log_times = std::env::var_os("ZRUN_LOGT").is_some();
    let mut log_seen = 0;
    let mut render_ms = 0.0;
    for frame in first..first + frames {
        let t = Instant::now();
        if frame == from {
            start = Instant::now();
            (timed_from, skipped_from) = (z.instructions(), z.skipped());
        }
        z.run_frame();
        if render_each {
            let r = Instant::now();
            z.render(&mut frame_img);
            render_ms += r.elapsed().as_secs_f64() * 1000.0;
        }
        slow.push((t.elapsed().as_secs_f64() * 1000.0, frame));
        match &shots {
            Some((Some(dir), n)) if frame % n == 0 => {
                save_ppm(&format!("{dir}/{frame:05}.ppm"), &z);
            }
            Some((None, n)) if (frame + 1) % n == 0 => {
                save_ppm(&format!("{}.{}.ppm", args[3], frame + 1), &z);
            }
            _ => {}
        }
        if let Some((p, f)) = &save
            && frame + 1 == *f
        {
            std::fs::write(format!("{p}.state"), z.save_state()).unwrap();
            std::fs::write(format!("{p}.nand"), z.nand()).unwrap();
        }
        if log_times {
            let log = z.log();
            if let Some(end) = log.rfind('\n')
                && end + 1 > log_seen
            {
                for line in log[log_seen..end].lines() {
                    println!("{:8.3} {line}", (frame + 1) as f64 / 60.0);
                }
                log_seen = end + 1;
            }
        }
        if sig_path.is_some() {
            z.render(&mut frame_img);
            sig.extend(signature(&frame_img));
        }
        let audio = z.take_audio();
        if wav.is_some() {
            all_audio.extend_from_slice(&audio);
        }
        if levels {
            peak = audio.iter().fold(peak, |m, &s| m.max(s.unsigned_abs()));
            if (frame + 1) % 60 == 0 {
                print!("{peak} ");
                peak = 0;
            }
        }
        samples += audio.len() / 2;
        for s in audio {
            audio_hash = (audio_hash ^ s as u16 as u64).wrapping_mul(0x0100_0000_01B3);
        }
    }
    let secs = start.elapsed().as_secs_f64();
    let timed = z.instructions() - timed_from;
    println!(
        "{timed} instructions in {secs:.2} s: {:.1} M/s ({:.2}x real time at {} M/s); {samples} sound samples",
        timed as f64 / secs / 1e6,
        timed as f64 / secs / CLOCK as f64,
        CLOCK / 1_000_000
    );
    let img = save_ppm(&args[3], &z);
    if let Some(path) = &wav {
        save_wav(path, &all_audio);
    }
    if let Some(path) = &sig_path {
        std::fs::write(path, &sig).unwrap();
    }
    if render_each {
        println!("drawing: {:.2} ms a frame", render_ms / frames as f64);
    }
    if let Some(n) = std::env::var("ZRUN_SLOW").ok().and_then(|v| v.parse::<usize>().ok()) {
        slow.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (ms, frame) in slow.iter().take(n) {
            println!("slow frame {frame}: {ms:.1} ms");
        }
    }
    let image_hash = img.iter().fold(0u64, |h, &p| (h ^ p as u64).wrapping_mul(0x0100_0000_01B3));
    println!("sound {audio_hash:016x}, picture {image_hash:016x}");
    let skipped = z.skipped() - skipped_from;
    println!("skipped in idle loops: {skipped} ({:.1}%)", 100.0 * skipped as f64 / timed as f64);
    if let Ok(path) = std::env::var("ZRUN_RAM") {
        std::fs::write(path, &z.ram()[..0x0100_0000]).unwrap();
    }
    if std::env::var_os("ZRUN_IO").is_some() {
        for (a, e) in z.probe().stats.iter() {
            println!("io {a:08x}: {} reads, {} writes (last {:#x}), first at {:08x}", e.reads, e.writes, e.last_write, e.first_pc);
        }
    }
    if std::env::var_os("ZRUN_LOG").is_some() {
        print!("{}", z.log());
    }
}

/// Saves the picture as a PPM file and returns it.
fn save_ppm(path: &str, z: &Zippity) -> Vec<u32> {
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut img);
    let mut out = format!("P6
{SCREEN_W} {SCREEN_H}
255
").into_bytes();
    for &p in &img {
        out.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, p as u8]);
    }
    std::fs::write(path, out).unwrap();
    img
}

/// Saves stereo sound at the sound unit's output rate as a WAV file (the
/// rate rounded to whole hertz in the header).
fn save_wav(path: &str, samples: &[i16]) {
    let rate = OUTPUT_RATE.round() as u32;
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
}

/// A picture's 8 x 6 blocks' mean red, green and blue.
fn signature(img: &[u32]) -> Vec<u8> {
    let (bw, bh) = (SCREEN_W / 8, SCREEN_H / 6);
    let mut out = Vec::with_capacity(144);
    for by in 0..6 {
        for bx in 0..8 {
            let mut sum = [0u64; 3];
            for y in by * bh..(by + 1) * bh {
                for &p in &img[y * SCREEN_W + bx * bw..y * SCREEN_W + (bx + 1) * bw] {
                    sum[0] += (p >> 16 & 0xFF) as u64;
                    sum[1] += (p >> 8 & 0xFF) as u64;
                    sum[2] += (p & 0xFF) as u64;
                }
            }
            out.extend(sum.map(|v| (v / (bw * bh) as u64) as u8));
        }
    }
    out
}
