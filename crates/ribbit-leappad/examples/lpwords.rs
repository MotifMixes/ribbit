//! Records what every word spot of a cartridge says, for transcribing the
//! printed words (the cartridges hold no text, only speech):
//! `lpwords <bios> <cart> <out dir> [eeprom]`.
//! For each spread of the book drawn from the cartridge it touches the GO,
//! then the middle of each word spot, and writes the sound that answers until
//! the firmware's "FGDoneEvent" as `<spread>-<region>.wav` (8 kHz), plus
//! `index.tsv`: spread, region, region name, what the firmware logged.
use ribbit_leappad::book::{Book, Spot};
use ribbit_leappad::{MACHINE_HZ, Machine};
use std::io::Write as _;
use std::path::Path;

/// Runs `secs` of emulated time, collecting the sound and the firmware's log.
fn run(m: &mut Machine, secs: f32, audio: &mut Vec<i16>, log: &mut String) {
    let step = MACHINE_HZ / 200;
    for _ in 0..(secs * 200.0) as u32 {
        m.run_cycles(step);
        let mut buf = vec![0i16; m.bus.sound.output.len()];
        m.bus.sound.output.pop_into(&mut buf);
        audio.extend_from_slice(&buf);
        log.push_str(&m.take_log());
    }
}

/// Holds the pen at `spot` briefly, then waits for the answer to end (up to
/// `max` seconds): the firmware's "FGDoneEvent", or, for apps that don't log
/// it (Cars' State Table Model), 0.6 s of quiet after the sound began.
/// Returns the sound and the log from the touch on.
fn touch(m: &mut Machine, spot: Spot, max: f32) -> (Vec<i16>, String) {
    let (mut audio, mut log) = (Vec::new(), String::new());
    m.set_pen(Some(spot));
    run(m, 0.25, &mut audio, &mut log);
    m.set_pen(None);
    let mut waited = 0.25;
    let (mut heard, mut quiet) = (false, 0.0);
    while waited < max && !log.contains("FGDoneEvent") {
        let from = audio.len();
        run(m, 0.05, &mut audio, &mut log);
        waited += 0.05;
        let loud = audio[from..].iter().any(|v| v.unsigned_abs() > 300);
        heard |= loud;
        quiet = if loud { 0.0 } else { quiet + 0.05 };
        if heard && quiet >= 0.6 {
            break;
        }
    }
    (audio, log)
}

fn write_wav(path: &Path, audio: &[i16]) {
    let mut f = Vec::new();
    let data = (audio.len() * 2) as u32;
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&(36 + data).to_le_bytes());
    f.extend_from_slice(b"WAVEfmt ");
    for v in [16u32, 1 | 1 << 16, 8000, 16000, 2 | 16 << 16] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(b"data");
    f.extend_from_slice(&data.to_le_bytes());
    for s in audio {
        f.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, f).expect("write WAV");
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let cart = std::fs::read(&a[2]).expect("cartridge");
    let book = Book::from_cartridge(&cart, "words", None).expect("the cartridge's layout");
    let out = Path::new(&a[3]);
    std::fs::create_dir_all(out).expect("out dir");
    let mut m = Machine::new(std::fs::read(&a[1]).expect("BIOS"));
    m.insert_cartridge(cart);
    if let Some(e) = a.get(4) {
        m.set_eeprom(std::fs::read(e).expect("EEPROM"));
    }
    let mut index = std::fs::File::create(out.join("index.tsv")).expect("index");
    let (mut audio, mut log) = (Vec::new(), String::new());
    run(&mut m, 4.0, &mut audio, &mut log);
    for (i, s) in book.spreads.iter().enumerate() {
        let Some(go) = s.go else { continue };
        touch(&mut m, go, 15.0);
        run(&mut m, 0.5, &mut audio, &mut log);
        for (k, r) in s.regions.iter().enumerate() {
            let [x0, y0, x1, y1] = r.rect;
            let at = Spot { page: r.page, x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0 };
            let (sound, said) = touch(&mut m, at, 25.0);
            run(&mut m, 0.3, &mut audio, &mut log);
            let answer: Vec<&str> = said.lines().filter(|l| l.contains("FG")).map(str::trim).collect();
            let loud = sound.iter().filter(|v| v.unsigned_abs() > 200).count();
            if loud > 400 {
                write_wav(&out.join(format!("{i}-{k}.wav")), &sound);
            }
            writeln!(index, "{i}\t{k}\t{}\t{}\t{}", r.name, loud, answer.join(" | ")).unwrap();
        }
        println!("spread {i}: {} spots", s.regions.len());
    }
}
