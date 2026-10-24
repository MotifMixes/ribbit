//! The in-app words pipeline end to end, for checking it against the Python
//! transcriber: `bookwords <bios> <cart> <model dir> <out.tsv> [spreads] [eeprom]`.
//! Records each spread with a second pad, hears its sentence buttons and
//! corner spots, then the word boxes they don't cover (prompted with the
//! sentences), then the rest, and writes the words file. Prints counts and
//! timings only.
use ribbit_leappad::book::Book;
use ribbit_leappad::words::{self, Recorder};
use ribbit_speech::{Recognizer, prepare_8k};
use std::collections::HashMap;
use std::time::Instant;

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let bios = std::fs::read(&a[1]).expect("BIOS");
    let cart = std::fs::read(&a[2]).expect("cartridge");
    let book = Book::from_cartridge(&cart, "words", None).expect("layout");
    let mut rec = Recognizer::load(std::path::Path::new(&a[3])).expect("model");
    let spreads: Vec<usize> = match a.get(5) {
        Some(s) => s.split(',').map(|n| n.parse().expect("spread")).collect(),
        None => (0..book.spreads.len()).collect(),
    };
    let eeprom = a.get(6).map(|e| std::fs::read(e).expect("EEPROM"));
    let mut recorder = Recorder::new(bios, cart, eeprom);
    let mut out = String::new();
    let all = Instant::now();
    for s in spreads {
        let t = Instant::now();
        let clips: HashMap<usize, Vec<i16>> = recorder.record(&book, s, &[]).into_iter().map(|c| (c.region, c.audio)).collect();
        let recorded = t.elapsed().as_secs_f32();
        let regions = &book.spreads[s].regions;
        let mut heard = HashMap::new();
        let mut hear = |k: usize, prompt: Option<&str>| -> Option<String> {
            clips.get(&k).map(|a| rec.transcribe(&prepare_8k(a), prompt).expect("transcribe"))
        };
        let mut n = 0;
        let first: Vec<usize> =
            words::button_candidates(regions).into_iter().chain((0..regions.len()).filter(|&k| words::corner(&regions[k]))).collect();
        for k in first {
            if let Some(t) = hear(k, None) {
                heard.insert(k, t);
                n += 1;
            }
        }
        let after_buttons = t.elapsed().as_secs_f32();
        let prompt = words::sentences(&book, s, &heard);
        let mut prompted = HashMap::new();
        let to_hear = words::boxes_to_hear(&book, s, &heard);
        for &k in &to_hear {
            if let Some(t) = hear(k, Some(&prompt)) {
                prompted.insert(k, t);
                n += 1;
            }
        }
        for &k in clips.keys() {
            if !heard.contains_key(&k) && !prompted.contains_key(&k) {
                if let Some(t) = hear(k, None) {
                    heard.insert(k, t);
                    n += 1;
                }
            }
        }
        if std::env::var_os("TRACE").is_some() {
            println!("prompt: {prompt}");
            for &k in &to_hear {
                println!("  box {k}: heard {:?} prompted {:?}", heard.get(&k), prompted.get(&k));
            }
        }
        let found = words::assign(&book, s, &heard, &prompted);
        out.push_str(&words::lines(&book, s, &found));
        let count = |k| found.iter().filter(|f| f.kind == k).count();
        println!(
            "spread {s}: {} clips recorded in {recorded:.1}s; words on screen after {after_buttons:.1}s; {} boxes heard alone; \
             {n} heard in {:.1}s; {} words, {} sentences, {} phrases, {} stop",
            clips.len(),
            to_hear.len(),
            t.elapsed().as_secs_f32(),
            count(words::Kind::Word),
            count(words::Kind::Sentence),
            count(words::Kind::Phrase),
            count(words::Kind::Stop)
        );
    }
    std::fs::write(&a[4], out).expect("write");
    println!("all in {:.0}s", all.elapsed().as_secs_f32());
}
