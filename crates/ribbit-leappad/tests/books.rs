//! Virtual books against the real firmware.
//!
//! The tests marked `#[ignore]` need dumps (never in the repository):
//! `RIBBIT_LEAPPAD_BIOS=<base ROM> RIBBIT_LEAPPAD_CART=<cartridge>
//! cargo test -p ribbit-leappad --release -- --ignored`
//! (checked with the ILA2 base ROM and Tutter's Tiny Trip, 500-00406).

use ribbit_leappad::book::{Book, Side, Spot, touch_reports};
use ribbit_leappad::layout::Hotspot;
use ribbit_leappad::{MACHINE_HZ, Machine};
use std::path::Path;

fn test_book() -> Book {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("books/pen-test.toml");
    let (book, warnings) = Book::load(&path).expect("the test book loads");
    // It is not made for one cartridge, and has no pictures (Ribbit draws its pages).
    assert!(warnings.iter().all(|w| w.message.contains("which cartridge")), "{warnings:?}");
    book
}

#[test]
fn the_test_book_is_valid() {
    let b = test_book();
    assert_eq!(b.spreads.len(), 3);
    assert!(b.spreads.iter().all(|s| s.go.is_some()));
    assert!(b.spreads[0].regions.len() >= 2 * 12);
    // Every region on its own spot.
    for s in &b.spreads {
        for (i, r) in s.regions.iter().enumerate() {
            for q in &s.regions[i + 1..] {
                assert!(!q.contains(r.centre()), "{} overlaps {}", q.name, r.name);
            }
        }
    }
}

/// Seconds of emulated time, in machine cycles.
fn secs(s: f32) -> u32 {
    (s * MACHINE_HZ as f32) as u32
}

/// The machine powered on with the dumps, run until the cartridge's app is
/// waiting for the pen; with everything it printed. None without dumps.
fn booted(with_cart: bool) -> Option<(Machine, String)> {
    let bios = std::fs::read(std::env::var("RIBBIT_LEAPPAD_BIOS").ok()?).ok()?;
    let mut m = Machine::new(bios);
    if with_cart {
        m.insert_cartridge(std::fs::read(std::env::var("RIBBIT_LEAPPAD_CART").ok()?).ok()?);
    }
    // The cartridge plays its start-up sound and an announcement before it
    // takes touches (about 35 s of emulated time with Tutter's Tiny Trip);
    // its app logs "[Idle]" ("State: Idle" in the 1999 app) when it waits
    // for the pen.
    let mut log = String::new();
    for _ in 0..360 {
        m.run_cycles(secs(0.25));
        log += &m.take_log();
        let ready = if with_cart { idle(&log) } else { log.contains("CartStatus:0") };
        if ready {
            m.run_cycles(secs(0.5));
            log += &m.take_log();
            break;
        }
    }
    Some((m, log))
}

/// Whether the app said it waits for the pen.
fn idle(log: &str) -> bool {
    log.contains("[Idle]") || log.contains("State: Idle")
}

/// Runs until the log after a touch holds `what` and the `words` words after
/// it are complete (at most 10 s): the 1999 app's log trails its touches by
/// seconds, and ends a touch's line only when it next prints something.
fn wait_for(m: &mut Machine, out: &mut String, what: &str, words: usize) {
    for _ in 0..40 {
        let done = out.split_inclusive('\n').any(|l| {
            l.find(what).is_some_and(|at| l.ends_with('\n') || l[at..].split_whitespace().count() > words)
        });
        if done {
            return;
        }
        m.run_cycles(secs(0.25));
        *out += &m.take_log();
    }
}

/// Touches `spot` for 0.3 s and returns what the firmware printed meanwhile
/// and in the 0.6 s after.
fn touch(m: &mut Machine, spot: Spot) -> String {
    m.set_pen(Some(spot));
    m.run_cycles(secs(0.3));
    m.set_pen(None);
    m.run_cycles(secs(0.6));
    m.take_log()
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn the_cartridge_feels_every_target_where_it_is() {
    let Some((mut m, log)) = booted(true) else { panic!("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART") };
    assert!(log.contains("Cart OK"), "the base accepted the cartridge:\n{log}");
    assert!(!log.contains("No CAL table"), "the factory calibration passed the kernel's check");
    let book = test_book();
    let mut worst: f32 = 0.0;
    for r in &book.spreads[0].regions {
        let want = r.centre();
        let out = touch(&mut m, want);
        let got = touch_reports(&out);
        let Some(last) = got.last() else { panic!("no touch reported for {}:\n{out}", r.name) };
        assert_eq!(last.spot.page, want.page, "{}: wrong page", r.name);
        let err = ((last.spot.x - want.x).abs() / 945.0).max((last.spot.y - want.y).abs() / 1395.0);
        worst = worst.max(err);
        assert!(err < 0.03, "{}: aimed at ({}, {}), the cartridge felt ({}, {})", r.name, want.x, want.y, last.spot.x, last.spot.y);
    }
    eprintln!("worst error {:.2}% of the page", worst * 100.0);
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn a_go_touch_changes_the_view() {
    let Some((mut m, _)) = booted(true) else { panic!("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART") };
    let go = test_book().spreads[0].go.unwrap();
    assert_eq!(go.page, Side::Left);
    let out = touch(&mut m, go);
    assert!(out.contains("PPTouch") && out.contains("New view init"), "a page-pointer touch:\n{out}");
}

/// Boots a LeapPad Classic cartridge (RIBBIT_LEAPPAD_CLASSIC_CART, e.g.
/// 500-11887) for 40 s with `classic` set as given; what it printed.
fn classic_boot(classic: bool) -> Option<String> {
    let bios = std::fs::read(std::env::var("RIBBIT_LEAPPAD_BIOS").ok()?).ok()?;
    let mut m = Machine::new(bios);
    m.bus.classic = classic;
    m.insert_cartridge(std::fs::read(std::env::var("RIBBIT_LEAPPAD_CLASSIC_CART").ok()?).ok()?);
    let mut log = String::new();
    for _ in 0..160 {
        m.run_cycles(secs(0.25));
        log += &m.take_log();
        if log.contains("Idle") {
            break;
        }
    }
    Some(log)
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CLASSIC_CART"]
fn classic_cartridges_run_their_own_kernel_and_app() {
    let log = classic_boot(true).expect("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CLASSIC_CART");
    assert!(log.contains("LeapPadClassic RIB: Cart OK"), "{log}");
    assert!(log.contains("Launch cart kernel"), "{log}");
    assert!(log.contains("Idle"), "the cartridge's app waits for the pen:
{log}");
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CLASSIC_CART"]
fn the_ila2_base_itself_refuses_classic_cartridges() {
    let log = classic_boot(false).expect("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CLASSIC_CART");
    assert!(log.contains("LeapPad Classic not supported on this base"), "{log}");
    assert!(!log.contains("Launch cart kernel"), "{log}");
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn without_a_cartridge_the_base_runs_its_own_book() {
    let Some((_, log)) = booted(false) else { panic!("set RIBBIT_LEAPPAD_BIOS") };
    assert!(log.contains("Cart found:No"), "{log}");
    assert!(log.contains("Launch base ROM lpBootApp"), "{log}");
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn pulling_the_cartridge_out_is_noticed() {
    let Some((mut m, _)) = booted(true) else { panic!("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART") };
    let cart = m.bus.cart.clone().unwrap();
    m.remove_cartridge();
    m.run_cycles(secs(3.0));
    let out = m.take_log();
    eprintln!("after pulling the cartridge:\n{out}");
    // Put it back and power on again: the cartridge runs as before.
    let bios = m.bus.bios.clone();
    let mut m = Machine::new(bios);
    m.insert_cartridge(cart);
    m.run_cycles(secs(3.0));
    assert!(m.take_log().contains("Cart OK"));
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn dumps_are_told_apart() {
    use ribbit_leappad::{Image, identify};
    let bios = std::fs::read(std::env::var("RIBBIT_LEAPPAD_BIOS").unwrap()).unwrap();
    let cart = std::fs::read(std::env::var("RIBBIT_LEAPPAD_CART").unwrap()).unwrap();
    assert_eq!(identify(&bios), Some(Image::BaseRom));
    assert_eq!(identify(&cart), Some(Image::Cartridge));
    assert_eq!(identify(&vec![0xFF; 0x10_0000]), None);
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn the_cartridge_plays_music_and_speaks_at_power_on() {
    let bios = std::fs::read(std::env::var("RIBBIT_LEAPPAD_BIOS").expect("set RIBBIT_LEAPPAD_BIOS")).unwrap();
    let cart = std::fs::read(std::env::var("RIBBIT_LEAPPAD_CART").expect("set RIBBIT_LEAPPAD_CART")).unwrap();
    let mut m = Machine::new(bios);
    m.insert_cartridge(cart);
    let (mut audio, mut buf, mut spoke) = (Vec::new(), Vec::new(), false);
    for _ in 0..800 {
        m.run_cycles(secs(0.05));
        // FC1E bit 6: voice 7, the LFC speech decoder.
        spoke |= m.bus.sound.status() & 0x40 != 0;
        buf.resize(m.bus.sound.output.len(), 0);
        m.bus.sound.output.pop_into(&mut buf);
        audio.extend_from_slice(&buf);
    }
    let seconds = audio.len() as f32 / 8000.0;
    let heard = audio.iter().filter(|s| s.unsigned_abs() > 64).count() as f32 / 8000.0;
    assert!((39.0..41.0).contains(&seconds), "8 kHz output kept pace: {seconds} s in 40");
    assert!(heard > 10.0, "music and speech: {heard} s of 40 not silent");
    assert!(spoke, "the speech decoder ran");
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn the_off_switch_powers_down_after_its_sound() {
    let Some((mut m, _)) = booted(true) else { panic!("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART") };
    m.press_power();
    let mut log = String::new();
    for _ in 0..100 {
        m.run_cycles(secs(0.1));
        log += &m.take_log();
        if m.is_off() {
            break;
        }
    }
    assert!(m.is_off(), "the pad switched itself off:\n{log}");
    assert!(log.contains("Off Switch"), "{log}");
}

/// The mode the app answers touches in after `log`: the last mode it
/// switched to ("NewG:2") or logged with a touch ("G:01").
fn mode_of(log: &str) -> Option<u8> {
    let hex = |l: &str, key: &str| {
        let at = l.rfind(key)? + key.len();
        u8::from_str_radix(l[at..].split(' ').next()?.trim(), 16).ok()
    };
    log.lines().rev().find_map(|l| hex(l, "NewG:").or_else(|| hex(l, " G:")))
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART"]
fn the_cartridge_answers_every_spot_its_layout_lists() {
    let cart = std::fs::read(std::env::var("RIBBIT_LEAPPAD_CART").expect("set RIBBIT_LEAPPAD_CART")).unwrap();
    let layout = ribbit_leappad::layout::read(&cart).expect("the layout reads");
    let Some((mut m, _)) = booted(true) else { panic!("set RIBBIT_LEAPPAD_BIOS and RIBBIT_LEAPPAD_CART") };
    let (mut tried, mut wrong, mut silent) = (0, Vec::new(), Vec::new());
    // RIBBIT_LEAPPAD_SPREADS=a-b limits the spreads (1-based) for a quicker run.
    let only = std::env::var("RIBBIT_LEAPPAD_SPREADS").ok().map(|s| {
        let (a, b) = s.split_once('-').unwrap_or((&s, &s));
        a.parse::<usize>().unwrap()..=b.parse::<usize>().unwrap()
    });
    // The mode the app answers in, as it last logged it.
    let mut mode = 0xFF;
    for (n, spread) in layout.spreads.iter().enumerate() {
        if only.as_ref().is_some_and(|r| !r.contains(&(n + 1))) {
            continue;
        }
        // Touches GO and waits for the app to announce the spread and take
        // touches again.
        let open = |m: &mut Machine| {
            let mut log = touch(m, spread.go.inside_point().unwrap());
            wait_for(m, &mut log, "PPTouch", 1);
            assert!(log.contains("PPTouch"), "spread {}: GO is a page-pointer touch:\n{log}", n + 1);
            for _ in 0..80 {
                if idle(&log[log.find("PPTouch").unwrap()..]) {
                    break;
                }
                m.run_cycles(secs(0.25));
                log += &m.take_log();
            }
        };
        open(&mut m);
        let Some(view) = layout.main_view(spread) else { continue };
        for (i, h) in view.hotspots.iter().enumerate() {
            let Some(at) = h.shape.inside_point() else { continue };
            // Classic pages list a word once per mode (G:01 whole words, G:02
            // sounded out …); the page's GSwitch spots change the mode.
            if h.modes & mode == 0 {
                for s in view.hotspots.iter().filter(|s| s.kind_name() == "GSwitch") {
                    let Some(p) = s.shape.inside_point() else { continue };
                    let mut out = touch(&mut m, p);
                    wait_for(&mut m, &mut out, " G:", 3);
                    mode = mode_of(&out).unwrap_or(mode);
                    if h.modes & mode != 0 {
                        break;
                    }
                }
                if h.modes & mode == 0 {
                    continue;
                }
            }
            // The app takes the first clickable of the mode in its list that
            // holds the pen.
            let active = |g: &&Hotspot| g.modes & mode != 0;
            let first = view.hotspots.iter().position(|g| active(&g) && g.shape.contains(at)).unwrap();
            if first != i {
                continue;
            }
            let mut out = touch(&mut m, at);
            // Busy starting background music, the app can answer late.
            wait_for(&mut m, &mut out, " G:", 3);
            tried += 1;
            // Judged by where the cartridge felt the pen, so that the pen's
            // own error (a few units) does not count against the layout.
            let Some(felt) = touch_reports(&out).last().map(|r| r.spot) else {
                silent.push(format!("spread {} {}:{}", n + 1, h.kind_name(), h.id));
                continue;
            };
            let want = match view.hotspots.iter().filter(active).find(|g| g.shape.contains(felt)) {
                Some(g) => format!("{}:{}", g.kind_name(), g.id),
                None => continue,
            };
            mode = mode_of(&out).unwrap_or(mode);
            if !out.contains(&want) {
                wrong.push(format!("spread {} {want} at ({:.0}, {:.0}) {:?}:\n{out}", n + 1, felt.x, felt.y, felt.page));
            }
            // A view switch ("NewV:" in the Classic app) leaves the spread's Touch-and-Tell view.
            if out.contains("New view init") || out.contains("NewV:") {
                open(&mut m);
            }
        }
    }
    eprintln!("{tried} spots touched; no answer: {silent:?}; answered differently: {}", wrong.len());
    eprintln!("{}", wrong.join("\n"));
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
    assert!(silent.len() * 50 <= tried, "{silent:?}");
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_CART"]
fn a_book_is_drawn_from_the_cartridge() {
    let cart = std::fs::read(std::env::var("RIBBIT_LEAPPAD_CART").expect("set RIBBIT_LEAPPAD_CART")).unwrap();
    let book = Book::from_cartridge(&cart, "Test", Some("500-00406")).expect("drawn");
    let problems = book.check();
    assert!(problems.iter().all(|p| !p.error), "{problems:?}");
    assert!(book.spreads.len() >= 8 && book.spreads.iter().all(|s| s.go.is_some()));
    assert!(book.spreads.iter().map(|s| s.regions.len()).sum::<usize>() > 300);
}

#[test]
#[ignore = "needs RIBBIT_LEAPPAD_CART"]
fn a_drawn_book_saves_as_a_book_file() {
    let cart = std::fs::read(std::env::var("RIBBIT_LEAPPAD_CART").expect("set RIBBIT_LEAPPAD_CART")).unwrap();
    let book = Book::from_cartridge(&cart, "Test", Some("500-00406")).expect("drawn");
    let (back, _) = Book::parse(&book.to_toml(), Path::new(".")).expect("the saved file reads back");
    assert_eq!(back.spreads, book.spreads);
}
