//! "Turn off when idle" against the real system ROM (never in the
//! repository): `RIBBIT_CLICKSTART_BIOS=<clickstartbios.bin> cargo test
//! --release -p ribbit-clickstart --test idle -- --ignored`.

use ribbit_clickstart::ClickStart;

/// The console left alone until its home screen shows and counts (about 270 s on a
/// blank EEPROM: Player Profile and Log-In time out to it), or None
/// without the ROM.
fn at_home() -> Option<ClickStart> {
    let bios = std::fs::read(std::env::var("RIBBIT_CLICKSTART_BIOS").ok()?).ok()?;
    let mut cs = ClickStart::new(&bios);
    for _ in 0..60 * 400 {
        cs.run_frame();
        // Counting (after the home screen's own opening).
        if cs.idle_count().is_some_and(|(n, _)| n >= 90) {
            return Some(cs);
        }
    }
    panic!("the home screen never showed");
}

/// On (`keep_awake` false): the home screen's count runs out and the
/// console waves goodbye and turns off. Off: the count is set back and it
/// stays on.
#[test]
#[ignore]
fn home_screen_idle_power_off_and_keep_awake() {
    let Some(home) = at_home() else {
        eprintln!("RIBBIT_CLICKSTART_BIOS not set; skipping");
        return;
    };
    let state = home.save_state();
    let (_, limit) = home.idle_count().unwrap();
    assert_eq!(limit, 4499);
    let bios = std::fs::read(std::env::var("RIBBIT_CLICKSTART_BIOS").unwrap()).unwrap();
    for keep_awake in [false, true] {
        let mut cs = ClickStart::new(&bios);
        cs.load_state(&state).unwrap();
        cs.keep_awake = keep_awake;
        // Two seconds before it runs out (it counts 30 a second).
        cs.set_idle_count(limit - 60);
        let mut off_at = None;
        for f in 0..60 * 60 {
            cs.run_frame();
            if cs.powered_off() {
                off_at = Some(f / 60);
                break;
            }
        }
        if keep_awake {
            assert_eq!(off_at, None, "kept on");
            assert!(cs.idle_count().is_some_and(|(n, _)| n < 60), "{:?}", cs.idle_count());
        } else {
            // The count, then the goodbye animation (about 7 s).
            assert!(off_at.is_some_and(|s| (5..=15).contains(&s)), "turned off at {off_at:?} s");
        }
    }
}
