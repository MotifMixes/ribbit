//! The mouse against the real system ROM (never in the repository):
//! `RIBBIT_CLICKSTART_BIOS=<clickstartbios.bin> cargo test --release -p
//! ribbit-clickstart --test mouse -- --ignored`.

use ribbit_clickstart::ClickStart;

/// The ROM's pointer in pixels (x, y down the screen).
fn pointer(cs: &ClickStart) -> (i32, i32) {
    (cs.peek(0x2c) as i16 as i32 >> 5, 239 - (cs.peek(0x2d) as i16 as i32 >> 5))
}

/// Sends (dx, dy) pixels as the app does: a packet once the last is in.
fn send(cs: &mut ClickStart, dx: i32, dy: i32) {
    while !cs.input_idle() {
        cs.run_frame();
    }
    cs.mouse_pixels(dx, dy, false);
    cs.run_frame();
}

/// Player Profile (a blank EEPROM, 18 s in): the pointer follows whole
/// pixel moves exactly, slow or fast, there and back. Before, 1 pixel a
/// packet (about 2.7 units) moved it 1 or 0 pixels right and 1 or 2 left.
#[test]
#[ignore]
fn pointer_follows_pixel_for_pixel() {
    let Some(bios) = std::env::var("RIBBIT_CLICKSTART_BIOS").ok().and_then(|p| std::fs::read(p).ok()) else {
        eprintln!("RIBBIT_CLICKSTART_BIOS not set; skipping");
        return;
    };
    let mut cs = ClickStart::new(&bios);
    for _ in 0..60 * 18 {
        cs.run_frame();
    }
    // To the middle (the screen keeps the pointer 11 pixels in on the left).
    let start = pointer(&cs);
    let (dx, dy) = (160 - start.0, 120 - start.1);
    send(&mut cs, dx / 2, dy / 2);
    send(&mut cs, dx - dx / 2, dy - dy / 2);
    for _ in 0..4 {
        cs.run_frame();
    }
    assert_eq!(pointer(&cs), (160, 120));
    for (step, n) in [(1, 30), (2, 15), (3, 10), (7, 4), (25, 2), (60, 1)] {
        for (sx, sy) in [(1, 0), (0, 1), (1, 1), (-1, 1)] {
            let from = pointer(&cs);
            for _ in 0..n {
                send(&mut cs, sx * step, sy * step);
            }
            for _ in 0..4 {
                cs.run_frame();
            }
            let there = pointer(&cs);
            assert_eq!(there, (from.0 + sx * step * n, from.1 + sy * step * n), "step {step} dir ({sx},{sy})");
            for _ in 0..n {
                send(&mut cs, -sx * step, -sy * step);
            }
            for _ in 0..4 {
                cs.run_frame();
            }
            assert_eq!(pointer(&cs), from, "back: step {step} dir ({sx},{sy})");
        }
    }
}
