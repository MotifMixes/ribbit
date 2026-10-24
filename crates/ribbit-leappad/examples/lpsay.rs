//! Plays one sound through the LeapPad's sound processor model and writes it
//! as an 8 kHz WAV: `lpsay <bios> <cart|-> <voice> <start> [end/loop] [pitch] <out.wav>`
//! (hex numbers; start and end as the kernel stores them: physical start,
//! end index << 16 | loop index for voices 0-4, absolute end for 5-6).
//! Reports the length and how much of it is at full scale.
use ribbit_leappad::sound::{engine, Rom};

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex number")
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bios = std::fs::read(&a[1]).expect("BIOS");
    let cart = (a[2] != "-").then(|| std::fs::read(&a[2]).expect("cart"));
    let voice = hex(&a[3]) as usize;
    let mut e = engine();
    e.start[voice] = hex(&a[4]);
    if voice < 7 {
        e.end[voice] = hex(&a[5]);
    }
    if voice < 5 {
        e.pitch[voice] = hex(&a[6]) as u16;
    }
    e.volume[voice] = 0x4000;
    let mut rom = Rom { bios: &bios, cart: cart.as_deref() };
    e.command(ribbit_core::io::voices::START_COMMAND, voice as u8);
    if voice == 7 {
        e.start_lfc(&mut rom);
    }
    let mut out = Vec::new();
    let mut buf = [0i16; 800];
    while e.active() && out.len() < 8000 * 60 {
        e.advance(800, 8000, &mut rom);
        e.output.pop_into(&mut buf);
        out.extend_from_slice(&buf);
    }
    let path = a.last().unwrap();
    let mut f = Vec::new();
    let data = (out.len() * 2) as u32;
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + data).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 1 | 1 << 16, 8000, 16000, 2 | 16 << 16] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data.to_le_bytes());
    for s in &out {
        f.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, f).expect("write WAV");
    // Volume 0x4000 with the engine's 1/8 mix: full scale is 4096.
    let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let clipped = out.iter().filter(|s| s.unsigned_abs() >= 4095).count();
    println!(
        "{:.2} s, peak {peak} of 4096, {clipped} samples at full scale, mode {}, {} frames",
        out.len() as f64 / 8000.0,
        e.lfc.mode,
        e.lfc.frames
    );
}
