//! The words on a book's pages, found by listening: cartridges hold no text,
//! only speech, so Ribbit touches every spot of a book drawn from the
//! cartridge (`Recorder`, a second pad of its own) and has the frontend
//! transcribe what each says; `assign` then decides what each spot shows
//! (docs/leappad.md, "The words on the pages"). Nothing transcribed ships
//! with Ribbit: it is made on the player's PC from their own cartridge.
//!
//! The rules (checked on the 16 books of the user's set):
//! * A **sentence button** is a roughly square box at the start of a line of
//!   word boxes that says a whole sentence (the printed speech bubble). Its
//!   sentence's words go over the word boxes in reading order: by count when
//!   the counts agree, else by aligning what each box says.
//! * A **page number** is a spot in a bottom corner that says only a number.
//! * The **stop sign** is the spot just above a page number whose shape is on
//!   most spreads.
//! * Misheard words that would be rude are dropped (a letter said alone, "L"
//!   heard as "Hell", becomes the letter).

use crate::book::{Book, PAGE_H, PAGE_W, Region, Side, Spot};
use crate::{MACHINE_HZ, Machine};
use std::collections::HashMap;

/// What one spot said, as the pad played it (8 kHz).
pub struct Clip {
    pub region: usize,
    pub audio: Vec<i16>,
}

/// A second LeapPad that touches a book's spots and records the answers.
pub struct Recorder {
    m: Machine,
    booted: bool,
    /// The app logs "FGDoneEvent" when a sound ends: then only that ends a
    /// recording (a sentence's pauses would cut it short otherwise).
    done_events: bool,
}

impl Recorder {
    pub fn new(bios: Vec<u8>, cart: Vec<u8>, eeprom: Option<Vec<u8>>) -> Self {
        let mut m = Machine::new(bios);
        m.insert_cartridge(cart);
        if let Some(e) = eeprom {
            m.set_eeprom(e);
        }
        Recorder { m, booted: false, done_events: false }
    }

    /// Runs `secs` of the pad, keeping its sound in `audio`.
    fn run(&mut self, secs: f32, audio: &mut Vec<i16>, log: &mut String) {
        let step = MACHINE_HZ / 200;
        let mut buf = Vec::new();
        for _ in 0..(secs * 200.0) as u32 {
            self.m.run_cycles(step);
            buf.resize(self.m.bus.sound.output.len(), 0);
            self.m.bus.sound.output.pop_into(&mut buf);
            audio.extend_from_slice(&buf);
            log.push_str(&self.m.take_log());
        }
    }

    /// Touches `spot` and keeps what answers: up to the firmware's
    /// "FGDoneEvent", or, in apps that never log it (Cars' State Table
    /// Model), 0.6 s of quiet after the sound began; within `max` seconds.
    fn touch(&mut self, spot: Spot, max: f32) -> Vec<i16> {
        let (mut audio, mut log) = (Vec::new(), String::new());
        self.m.set_pen(Some(spot));
        self.run(0.25, &mut audio, &mut log);
        self.m.set_pen(None);
        let (mut waited, mut heard, mut quiet) = (0.25, false, 0.0);
        while waited < max && !log.contains("FGDoneEvent") {
            let from = audio.len();
            self.run(0.05, &mut audio, &mut log);
            waited += 0.05;
            let loud = audio[from..].iter().any(|v| v.unsigned_abs() > 300);
            heard |= loud;
            quiet = if loud { 0.0 } else { quiet + 0.05 };
            if heard && quiet >= 0.6 && !self.done_events {
                break;
            }
        }
        self.done_events |= log.contains("FGDoneEvent");
        audio
    }

    /// Opens `spread` (its GO) and records each of `regions` (all when
    /// empty). Spots that say nothing are left out.
    pub fn record(&mut self, book: &Book, spread: usize, regions: &[usize]) -> Vec<Clip> {
        let mut scratch = (Vec::new(), String::new());
        if !self.booted {
            self.run(4.0, &mut scratch.0, &mut scratch.1);
            self.booted = true;
        }
        let Some(s) = book.spreads.get(spread) else { return Vec::new() };
        if let Some(go) = s.go {
            self.touch(go, 15.0);
            self.run(0.5, &mut scratch.0, &mut scratch.1);
        }
        let all: Vec<usize> = (0..s.regions.len()).collect();
        let wanted = if regions.is_empty() { &all } else { regions };
        let mut out = Vec::new();
        for &k in wanted {
            let Some(r) = s.regions.get(k) else { continue };
            // GSwitch spots switch the mode; touching them would change what
            // the next spots say.
            if r.name.starts_with("GSwitch") {
                continue;
            }
            let audio = self.touch(r.centre(), 25.0);
            self.run(0.3, &mut scratch.0, &mut scratch.1);
            scratch.0.clear();
            if audio.iter().filter(|v| v.unsigned_abs() > 200).count() > 400 {
                out.push(Clip { region: k, audio });
            }
        }
        out
    }
}

/// What a spot shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A printed word (or page number), drawn in its box.
    Word,
    /// More than a word, said but not printed (shown under the pen).
    Phrase,
    /// A sentence's button (a speech bubble before the line).
    Sentence,
    /// The page's stop sign.
    Stop,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Word => "word",
            Kind::Phrase => "phrase",
            Kind::Sentence => "sentence",
            Kind::Stop => "stop",
        }
    }
}

/// One spot's result.
#[derive(Clone, Debug, PartialEq)]
pub struct Found {
    pub region: usize,
    pub kind: Kind,
    pub text: String,
}

/// The words of a text, without punctuation.
pub fn tokens(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '\'' {
            cur.push(c);
        } else if !cur.is_empty() {
            out.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Whisper's loops on long sounds ("AHHHHHHHH…", "Thank you. Thank you.
/// Thank you. …"): one of what repeats is kept.
pub fn unloop(text: &str) -> String {
    // Runs of a character past four.
    let mut s = String::new();
    let (mut last, mut run) = (None, 0);
    for c in text.chars() {
        run = if Some(c) == last { run + 1 } else { 1 };
        last = Some(c);
        if run <= 4 {
            s.push(c);
        }
    }
    // A phrase said three or more times in a row.
    let words: Vec<&str> = s.split_whitespace().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut i = 0;
    'outer: while i < words.len() {
        for len in 1..=8.min(words.len() - i) {
            let unit = &words[i..i + len];
            let mut reps = 1;
            while i + (reps + 1) * len <= words.len() && &words[i + reps * len..i + (reps + 1) * len] == unit {
                reps += 1;
            }
            if reps >= 3 {
                out.extend_from_slice(unit);
                i += reps * len;
                continue 'outer;
            }
        }
        out.push(words[i]);
        i += 1;
    }
    out.join(" ")
}

/// Words a misheard letter or word can come out as, never shown in a
/// children's book: said alone, a letter's becomes the letter.
const UNSAFE: [(&str, Option<&str>); 21] = [
    ("hell", Some("L")),
    ("ass", Some("S")),
    ("damn", None),
    ("shit", None),
    ("fuck", None),
    ("bitch", None),
    ("crap", None),
    ("dick", None),
    ("piss", None),
    ("sex", None),
    ("bastard", None),
    ("slut", None),
    ("whore", None),
    ("cock", None),
    ("pussy", None),
    ("tits", None),
    ("boob", None),
    ("boobs", None),
    ("fart", None),
    ("nigger", None),
    ("fag", None),
];

/// The text with misheard words dropped (or, said `alone`, made the letter
/// they are); None if nothing is left.
pub fn safe(text: &str, alone: bool) -> Option<String> {
    let mut out = Vec::new();
    for w in text.split_whitespace() {
        let key: String = w.chars().filter(|c| c.is_ascii_alphabetic()).collect::<String>().to_ascii_lowercase();
        match UNSAFE.iter().find(|(bad, _)| *bad == key) {
            Some((_, Some(letter))) if alone => out.push(letter.to_string()),
            Some(_) => {}
            None => out.push(w.to_string()),
        }
    }
    (!out.is_empty()).then(|| out.join(" "))
}

/// A number said as a word or digits, as digits.
pub fn number(w: &str) -> Option<String> {
    const WORDS: [&str; 21] = [
        "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine", "ten", "eleven", "twelve", "thirteen",
        "fourteen", "fifteen", "sixteen", "seventeen", "eighteen", "nineteen", "twenty",
    ];
    let w = w.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_ascii_lowercase();
    if !w.is_empty() && w.chars().all(|c| c.is_ascii_digit()) {
        return Some(w);
    }
    if w == "thirty" {
        return Some("30".into());
    }
    WORDS.iter().position(|n| *n == w).map(|n| n.to_string())
}

/// Python's difflib ratio (Ratcliff/Obershelp): 2 x matching / total.
fn ratio(a: &[char], b: &[char]) -> f32 {
    fn matching(a: &[char], b: &[char]) -> usize {
        let (mut best, mut ai, mut bi) = (0, 0, 0);
        for i in 0..a.len() {
            for j in 0..b.len() {
                let mut k = 0;
                while i + k < a.len() && j + k < b.len() && a[i + k] == b[j + k] {
                    k += 1;
                }
                if k > best {
                    (best, ai, bi) = (k, i, j);
                }
            }
        }
        if best == 0 {
            return 0;
        }
        best + matching(&a[..ai], &b[..bi]) + matching(&a[ai + best..], &b[bi + best..])
    }
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    2.0 * matching(a, b) as f32 / (a.len() + b.len()) as f32
}

/// A word's consonants, for "sounds like" (with 2, 4, 8 spelt out).
fn sound_key(w: &str) -> Vec<char> {
    let w = match w {
        "2" => "two",
        "4" => "four",
        "8" => "eight",
        "1" => "one",
        "0" => "oh",
        _ => w,
    };
    let lower: Vec<char> = w.to_ascii_lowercase().chars().collect();
    let key: Vec<char> = lower.iter().copied().filter(|c| !"aeiouyhw".contains(*c)).collect();
    if key.is_empty() { lower } else { key }
}

/// How alike two heard words are (0..1): by spelling, or nearly as much by
/// sound.
pub fn similar(a: &str, b: &str) -> f32 {
    let (la, lb) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    if la == lb {
        return 1.0;
    }
    let ca: Vec<char> = la.chars().collect();
    let cb: Vec<char> = lb.chars().collect();
    ratio(&ca, &cb).max(ratio(&sound_key(&la), &sound_key(&lb)) * 0.9)
}

/// Needleman-Wunsch of `words` (what boxes said, in reading order) to
/// `vocab` (the sentences' words): each word's vocab index, or None.
/// Between two matches, as many boxes as words are those words in order
/// (also before the first match and after the last, where counts agree).
pub fn align(words: &[String], vocab: &[String]) -> Vec<Option<usize>> {
    let (n, m) = (words.len(), vocab.len());
    let mut cost = vec![vec![0.0f32; m + 1]; n + 1];
    let mut back = vec![vec![0u8; m + 1]; n + 1];
    for j in 1..=m {
        cost[0][j] = cost[0][j - 1] + 0.6;
        back[0][j] = 2;
    }
    for i in 1..=n {
        cost[i][0] = cost[i - 1][0] + 0.6;
        back[i][0] = 1;
        for j in 1..=m {
            let opts = [
                (cost[i - 1][j - 1] + (1.0 - similar(&words[i - 1], &vocab[j - 1])) * 1.5, 0u8),
                (cost[i - 1][j] + 0.6, 1),
                (cost[i][j - 1] + 0.6, 2),
            ];
            let best = opts.iter().copied().min_by(|a, b| a.0.total_cmp(&b.0)).unwrap();
            cost[i][j] = best.0;
            back[i][j] = best.1;
        }
    }
    let mut out = vec![None; n];
    let (mut i, mut j) = (n, m);
    while i > 0 && j > 0 {
        match back[i][j] {
            0 => {
                if !words[i - 1].is_empty() && similar(&words[i - 1], &vocab[j - 1]) >= 0.5 {
                    out[i - 1] = Some(j - 1);
                }
                i -= 1;
                j -= 1;
            }
            1 => i -= 1,
            _ => j -= 1,
        }
    }
    let anchors: Vec<(usize, usize)> = out.iter().enumerate().filter_map(|(i, a)| a.map(|a| (i, a))).collect();
    for w in anchors.windows(2) {
        let ((i, a), (k, b)) = (w[0], w[1]);
        if k - i == b - a {
            for t in 1..k - i {
                out[i + t] = Some(a + t);
            }
        }
    }
    if let (Some(&(i, a)), Some(&(k, b))) = (anchors.first(), anchors.last()) {
        if a >= i {
            for t in 0..i {
                out[t] = Some(a - i + t);
            }
        }
        if m - b >= n - k {
            for t in k + 1..n {
                out[t] = Some(b + t - k);
            }
        }
    }
    out
}

fn page(r: &Region) -> usize {
    (r.page == Side::Right) as usize
}

/// A box of one line's height (up to 118 units: titles are 112): a printed
/// word or a sentence's button.
fn is_box(r: &Region) -> bool {
    r.rect[3] - r.rect[1] < PAGE_H * 0.085
}

fn centre_y(r: &Region) -> f32 {
    (r.rect[1] + r.rect[3]) / 2.0
}

/// The given spots in reading order: left page then right, line by line,
/// left to right.
pub fn reading_order(regions: &[Region], keys: &[usize]) -> Vec<usize> {
    let mut keys = keys.to_vec();
    keys.sort_by(|&a, &b| (page(&regions[a]), centre_y(&regions[a])).partial_cmp(&(page(&regions[b]), centre_y(&regions[b]))).unwrap());
    let mut lines: Vec<(usize, f32, Vec<usize>)> = Vec::new();
    for k in keys {
        let r = &regions[k];
        let h = r.rect[3] - r.rect[1];
        match lines.last_mut() {
            Some((p, y, ks)) if *p == page(r) && (centre_y(r) - *y).abs() < h / 2.0 => ks.push(k),
            _ => lines.push((page(r), centre_y(r), vec![k])),
        }
    }
    lines
        .into_iter()
        .flat_map(|(_, _, mut ks)| {
            ks.sort_by(|&a, &b| regions[a].rect[0].total_cmp(&regions[b].rect[0]));
            ks
        })
        .collect()
}

/// Boxes that may be sentence buttons, to hear first: roughly square, first
/// on their line, with a word box just after them.
pub fn button_candidates(regions: &[Region]) -> Vec<usize> {
    let boxes: Vec<usize> = (0..regions.len()).filter(|&k| is_box(&regions[k])).collect();
    let same_line = |a: &Region, b: &Region| page(a) == page(b) && (centre_y(a) - centre_y(b)).abs() < (a.rect[3] - a.rect[1]) / 2.0;
    boxes
        .iter()
        .copied()
        .filter(|&k| {
            let r = &regions[k];
            let (w, h) = (r.rect[2] - r.rect[0], r.rect[3] - r.rect[1]);
            w <= 1.5 * h
                && !boxes.iter().any(|&b| b != k && same_line(r, &regions[b]) && (0.0..40.0).contains(&(r.rect[0] - regions[b].rect[2])))
                && boxes.iter().any(|&b| b != k && same_line(r, &regions[b]) && (0.0..40.0).contains(&(regions[b].rect[0] - r.rect[2])))
        })
        .collect()
}

/// Spots in a bottom corner (where page numbers are printed).
pub fn corner(r: &Region) -> bool {
    r.rect[1] > PAGE_H * 0.8 && ((page(r) == 0 && r.rect[0] < 160.0) || (page(r) == 1 && r.rect[2] > PAGE_W - 160.0))
}

/// The spread's stop sign, if any: just above a corner spot, roughly square,
/// a shape on most of the book's spreads.
pub fn stop_sign(book: &Book, spread: usize) -> Option<usize> {
    let regions = &book.spreads.get(spread)?.regions;
    let on = |name: &str| book.spreads.iter().filter(|s| s.regions.iter().any(|r| r.name == name)).count();
    (0..regions.len()).find(|&k| {
        let r = &regions[k];
        let (w, h) = (r.rect[2] - r.rect[0], r.rect[3] - r.rect[1]);
        (0.6..1.7).contains(&(w / h.max(1.0)))
            && w <= 170.0
            && regions.iter().any(|n| {
                corner(n)
                    && page(n) == page(r)
                    && r.rect[0] < n.rect[2]
                    && r.rect[2] > n.rect[0]
                    && (0.0..80.0).contains(&(n.rect[1] - r.rect[3]))
            })
            && on(&r.name) * 2 >= book.spreads.len()
    })
}

/// The word boxes of a spread that its sentence buttons don't cover by
/// count: these need hearing one by one (with the sentences as the prompt).
pub fn boxes_to_hear(book: &Book, spread: usize, heard: &HashMap<usize, String>) -> Vec<usize> {
    let p = plan(book, spread, heard);
    if p.words_at.len() == p.vocab.len() { Vec::new() } else { p.words_at }
}

/// The spread's sentences (from its buttons), in reading order: the prompt
/// for hearing single words.
pub fn sentences(book: &Book, spread: usize, heard: &HashMap<usize, String>) -> String {
    let p = plan(book, spread, heard);
    p.buttons.iter().map(|b| heard[b].as_str()).collect::<Vec<_>>().join(" ")
}

struct Plan {
    buttons: Vec<usize>,
    numbers: HashMap<usize, String>,
    stop: Option<usize>,
    words_at: Vec<usize>,
    vocab: Vec<String>,
}

fn plan(book: &Book, spread: usize, heard: &HashMap<usize, String>) -> Plan {
    let regions = &book.spreads[spread].regions;
    let stop = stop_sign(book, spread);
    let numbers: HashMap<usize, String> = (0..regions.len())
        .filter(|&k| corner(&regions[k]) && Some(k) != stop)
        .filter_map(|k| {
            let t = tokens(heard.get(&k)?);
            (t.len() == 1).then(|| number(&t[0])).flatten().map(|n| (k, n))
        })
        .collect();
    let boxes: Vec<usize> = (0..regions.len()).filter(|&k| is_box(&regions[k])).collect();
    let candidates = button_candidates(regions);
    let buttons: Vec<usize> = reading_order(regions, &candidates)
        .into_iter()
        .filter(|k| heard.get(k).is_some_and(|t| tokens(t).len() >= 3))
        .collect();
    let words_at: Vec<usize> = reading_order(regions, &boxes)
        .into_iter()
        .filter(|k| !buttons.contains(k) && !numbers.contains_key(k) && Some(*k) != stop && !regions[*k].name.starts_with("GSwitch"))
        .collect();
    let vocab: Vec<String> = buttons.iter().flat_map(|b| tokens(&heard[b])).collect();
    Plan { buttons, numbers, stop, words_at, vocab }
}

/// What each spot of `spread` shows, from what has been heard so far:
/// `heard` alone (no prompt), `prompted` (single word boxes heard with the
/// spread's sentences as the prompt).
pub fn assign(book: &Book, spread: usize, heard: &HashMap<usize, String>, prompted: &HashMap<usize, String>) -> Vec<Found> {
    let regions = &book.spreads[spread].regions;
    let p = plan(book, spread, heard);
    let mut fixed: HashMap<usize, String> = HashMap::new();
    if !p.vocab.is_empty() {
        if p.words_at.len() == p.vocab.len() {
            fixed.extend(p.words_at.iter().copied().zip(p.vocab.iter().cloned()));
        } else {
            let said: Vec<String> = p
                .words_at
                .iter()
                .map(|k| prompted.get(k).or(heard.get(k)).and_then(|t| tokens(t).into_iter().next()).unwrap_or_default())
                .collect();
            for (k, at) in p.words_at.iter().zip(align(&said, &p.vocab)) {
                if let Some(at) = at {
                    fixed.insert(*k, p.vocab[at].clone());
                }
            }
        }
    }
    let mut out = Vec::new();
    for k in 0..regions.len() {
        let found = if Some(k) == p.stop {
            Some((Kind::Stop, "stop".to_string()))
        } else if p.buttons.contains(&k) {
            safe(&unloop(&heard[&k]), false).map(|t| (Kind::Sentence, t))
        } else if let Some(n) = p.numbers.get(&k) {
            Some((Kind::Word, n.clone()))
        } else if let Some(w) = fixed.get(&k) {
            safe(w, true).map(|t| (Kind::Word, t))
        } else if let Some(t) = prompted.get(&k).or(heard.get(&k)) {
            let t = unloop(t);
            let words = tokens(&t);
            let lower = t.to_ascii_lowercase();
            let lower = lower.trim_matches(|c: char| !c.is_ascii_alphanumeric());
            if words.is_empty() || lower == "you" || lower == "thank you" {
                None
            } else if words.len() == 1 && is_box(&regions[k]) && number(&words[0]).is_some() && corner(&regions[k]) {
                number(&words[0]).map(|n| (Kind::Word, n))
            } else if words.len() <= 2 && p.vocab.is_empty() && p.words_at.contains(&k) {
                // A page without sentence buttons (a cover's title): its
                // word boxes show what they say.
                safe(&words.join(" "), true).map(|t| (Kind::Word, t))
            } else if words.len() <= 2 {
                safe(&words.join(" "), true).map(|t| (Kind::Phrase, t))
            } else {
                safe(&t, false).map(|t| (Kind::Phrase, t))
            }
        } else {
            None
        };
        if let Some((kind, text)) = found {
            out.push(Found { region: k, kind, text });
        }
    }
    out
}

/// A spread's results as words-file lines (Book::apply_words).
pub fn lines(book: &Book, spread: usize, found: &[Found]) -> String {
    let regions = &book.spreads[spread].regions;
    found.iter().map(|f| format!("{spread}\t{}\t{}\t{}\t{}\n", f.region, regions[f.region].name, f.kind.name(), f.text)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn loops_and_rude_mishearings_are_cut() {
        assert_eq!(unloop("AHHHHHHHHH"), "AHHHH");
        assert_eq!(unloop("Okay. Thank you. Thank you. Thank you. Thank you."), "Okay. Thank you.");
        assert_eq!(unloop("Good good. Very good."), "Good good. Very good.");
        assert_eq!(safe("Hell", true).as_deref(), Some("L"));
        assert_eq!(safe("what the hell", false).as_deref(), Some("what the"));
        assert_eq!(safe("damn", true), None);
    }

    #[test]
    fn numbers_and_likeness() {
        assert_eq!(number("Five.").as_deref(), Some("5"));
        assert_eq!(number("12").as_deref(), Some("12"));
        assert_eq!(number("Tigger"), None);
        assert!(similar("Sure", "shore") > 0.5);
        assert!(similar("Buh", "dock") < 0.5);
        // Python's difflib: SequenceMatcher(None, "abcd", "bcde").ratio() = 0.75.
        assert!((ratio(&['a', 'b', 'c', 'd'], &['b', 'c', 'd', 'e']) - 0.75).abs() < 1e-6);
    }

    #[test]
    fn alignment_fills_between_matches() {
        // (Made-up words, as heard alone: "2" for "to", a mumbled "the".)
        let heard = words("Pip? Hops says Ben can We Run 2. Buh Pond");
        let vocab = words("Pip hops says Ben Can we run to the pond");
        let got: Vec<Option<usize>> = align(&heard, &vocab);
        assert_eq!(got, (0..10).map(Some).collect::<Vec<_>>());
    }
}
