//! The iQuest and Turbo Twist Extreme against their real base ROMs.
//!
//! The tests marked `#[ignore]` need dumps (never in the repository):
//! `RIBBIT_IQUEST_BIOS=<iquest.bin> RIBBIT_IQUEST_CART=<cartridge>
//! RIBBIT_TTX_BIOS=<turbotwistextreme.bin> cargo test -p ribbit-iquest --
//! --ignored` (checked with MAME's `iquest` and `turboex` and the 4.0
//! Starter Pack).

use ribbit_iquest::gpio::{Pin, Port, Wire, read};
use ribbit_iquest::{MACHINE_HZ, Machine, Model};

#[test]
fn a_key_joins_a_driven_column_to_its_row() {
    let mut io = vec![0u8; 0x100];
    let key = Wire { a: Some(Pin { port: Port::B, bit: 7 }), b: Pin { port: Port::A, bit: 6 } };
    // Nothing driven: rows read high.
    assert_eq!(read(&io, Port::A, &[key]), 0xFF);
    // Column bit 7 an output, driven low: row 6 follows it.
    io[0x4B] = 0x80;
    io[0x49] = 0x7F;
    assert_eq!(read(&io, Port::A, &[key]), 0xBF);
    // Driven high: no.
    io[0x49] = 0xFF;
    assert_eq!(read(&io, Port::A, &[key]), 0xFF);
    // A key to ground pulls its pin low whatever is driven.
    let enter = Wire { a: None, b: Pin { port: Port::C, bit: 0 } };
    assert_eq!(read(&io, Port::C, &[enter]), 0xFE);
}

#[test]
fn every_named_key_is_known() {
    for k in ["enter", "del", "menu", "repeat", "up", "down", "left", "right", "vol-", "vol+", "space", ".", "Q", "0"] {
        assert!(ribbit_iquest::keys::wire(Model::IQuest, k).is_some(), "iQuest {k}");
    }
    for k in ["up", "down", "left", "right", "twist-up", "twist-up-fast", "twist-down", "twist-down-fast", "slam", "menu", "repeat", "vol-", "vol+"] {
        assert!(ribbit_iquest::keys::wire(Model::TurboTwistExtreme, k).is_some(), "TT {k}");
    }
}

fn machine(var: &str) -> Option<Machine> {
    Some(Machine::new(std::fs::read(std::env::var(var).ok()?).ok()?))
}

fn run_secs(m: &mut Machine, s: f32) -> String {
    let mut log = String::new();
    for _ in 0..(s * 10.0) as u32 {
        m.run_cycles(MACHINE_HZ / 10);
        log.push_str(&m.take_log());
    }
    log
}

#[test]
#[ignore = "needs RIBBIT_IQUEST_BIOS and RIBBIT_IQUEST_CART"]
fn the_iquest_reaches_its_menu_and_a_chapter_challenge() {
    let mut m = machine("RIBBIT_IQUEST_BIOS").expect("set RIBBIT_IQUEST_BIOS");
    let cart = std::fs::read(std::env::var("RIBBIT_IQUEST_CART").expect("set RIBBIT_IQUEST_CART")).unwrap();
    m.insert_cartridge(cart);
    let log = run_secs(&mut m, 12.0);
    assert!(log.contains("Cart OK"), "{log}");
    assert!(log.contains("Flash:Secondary Init:Success"));
    assert!(log.contains("[InitMenu"), "the menu, not the shop demo");
    let f = m.frame();
    assert_eq!((f.width, f.height), (90, 64));
    assert!(f.pixels.iter().filter(|&&p| p != 0).count() > 200, "the menu is drawn");
    m.set_key("enter", true);
    let log = run_secs(&mut m, 0.3);
    m.set_key("enter", false);
    assert!(log.contains("{Enter}"), "{log}");
}

#[test]
#[ignore = "needs RIBBIT_TTX_BIOS"]
fn the_turbo_twist_extreme_reaches_its_menu_and_takes_keys() {
    let mut m = machine("RIBBIT_TTX_BIOS").expect("set RIBBIT_TTX_BIOS");
    let log = run_secs(&mut m, 16.0);
    assert!(log.contains("[InitMenu"), "{log}");
    let f = m.frame();
    assert_eq!((f.width, f.height), (96, 32));
    for (k, says) in [("left", "{DPadLf}"), ("twist-down-fast", "{TwistDownFast}"), ("menu", "{Menu}")] {
        m.set_key(k, true);
        let log = run_secs(&mut m, 0.3);
        m.set_key(k, false);
        run_secs(&mut m, 0.5);
        assert!(log.contains(says), "{k}: {log}");
    }
}

#[test]
#[ignore = "needs RIBBIT_TTX_BIOS"]
fn turn_off_when_idle_off_keeps_it_on() {
    let mut m = machine("RIBBIT_TTX_BIOS").expect("set RIBBIT_TTX_BIOS");
    m.keep_awake = true;
    run_secs(&mut m, 120.0);
    assert!(!m.is_off(), "it idled off");
    let mut m = machine("RIBBIT_TTX_BIOS").unwrap();
    let log = run_secs(&mut m, 120.0);
    assert!(log.contains("Auto Power Down") && m.is_off(), "setting on: it turns itself off");
}
