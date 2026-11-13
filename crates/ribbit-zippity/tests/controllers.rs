//! The controllers' radio packets must come often enough that the system
//! never takes the controllers for switched off: standing on the home spot
//! (or pressing anything) on the language screen once brought up the
//! "switch on your controllers" panel (the mat and the bopper with radio
//! waves) when Ribbit sent one packet a frame (docs/zippity.md,
//! "Controllers"). Needs the system NAND dump:
//!
//!   RIBBIT_ZIPPITY_NAND=<system NAND dump, raw> cargo test -p ribbit-zippity
//!   --release --test controllers -- --include-ignored

use ribbit_zippity::{FRAME, Pad, SCREEN_H, SCREEN_W, Zippity};

/// The menus' controller check (0x19F20): +0 the valid-packet count at the
/// last snapshot, +4 its state (0 while the controllers answer; the check
/// at 0x19FDC sets 1, the panel, when the count has not moved).
const CHECK: usize = 0x0016_FA40;
/// The kernel's packet decoder (0x2B154): +0xC counts the valid packets.
const DECODER: usize = 0x001B_E4A0;
/// Frames from power-on to the language screen (a dump with no players).
const LANGUAGE: u32 = 1700;

fn word(z: &Zippity, a: usize) -> u32 {
    u32::from_le_bytes(z.ram()[a..a + 4].try_into().unwrap())
}

/// Whether the panel is on the picture: it covers the pink gap between the
/// first two language buttons with its light blue.
fn panel_shown(img: &[u32]) -> bool {
    [(200, 224), (320, 224), (200, 228), (320, 228)].iter().all(|&(x, y)| {
        let p = img[y * SCREEN_W + x];
        (p >> 16 & 0xFF) < 128 && (p >> 8 & 0xFF) > 180 && (p & 0xFF) > 230
    })
}

/// What a frame on the language screen holds: standing on the home spot for
/// the first `stand` frames, then mashing the feet and the bopper (never a
/// spot or the Home button, which would leave the screen).
fn held(frame: u32, stand: u32) -> Pad {
    const MASH: [u16; 8] = [
        Pad::LEFT_FOOT | Pad::RIGHT_FOOT,
        Pad::LEFT_FOOT,
        Pad::RIGHT_FOOT,
        Pad::UP,
        Pad::DOWN | Pad::LEFT_FOOT | Pad::RIGHT_FOOT,
        0,
        Pad::UP | Pad::RIGHT_FOOT,
        Pad::DOWN,
    ];
    if frame < stand { Pad::held(Pad::LEFT_FOOT | Pad::RIGHT_FOOT) } else { Pad::held(MASH[(frame / 3) as usize % MASH.len()]) }
}

/// Runs `frames` frames on the language screen from `state`, with the
/// packets every `period` instructions (None: the emulator's rate), and
/// returns the first frame the panel came up (the check's state or the
/// picture), or None.
fn first_panel(state: &[u8], nand: &[u8], period: Option<u64>, frames: u32, stand: u32) -> Option<u32> {
    let mut z = Zippity::new(nand.to_vec());
    z.load_state(state).expect("a state");
    z.probe().radio_period = period;
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    let mut count = word(&z, DECODER + 0xC);
    for f in 0..frames {
        z.set_pad(held(f, stand));
        z.run_frame();
        // The valid packets keep coming (about 16 a frame at once a
        // millisecond; 1 at once a frame).
        let now = word(&z, DECODER + 0xC);
        assert!(now > count, "frame {f}: no valid packet this frame");
        count = now;
        z.render(&mut img);
        if z.ram()[CHECK + 4] != 0 || panel_shown(&img) {
            return Some(f);
        }
    }
    None
}

#[test]
#[ignore = "needs RIBBIT_ZIPPITY_NAND"]
fn no_switch_on_panel_while_inputs_are_held() {
    let Some(path) = std::env::var_os("RIBBIT_ZIPPITY_NAND") else {
        eprintln!("RIBBIT_ZIPPITY_NAND not set; skipping");
        return;
    };
    let nand = Zippity::factory_reset(std::fs::read(path).expect("read the NAND dump")).expect("clear the user data");
    let mut z = Zippity::new(nand.clone());
    for _ in 0..LANGUAGE {
        z.run_frame();
    }
    assert!(z.log().contains("could not open: UI.SAV"), "the cleared dump asks for the language");
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut img);
    let gap = img[224 * SCREEN_W + 200];
    assert!(gap >> 16 & 0xFF > 200, "not on the language screen (gap pixel {gap:06x})");
    let state = z.save_state();
    // Once a frame (the old rate) the panel comes up within seconds: the
    // detector sees it.
    let old = first_panel(&state, &nand, Some(FRAME), 1200, 1200);
    assert!(old.is_some(), "once a frame no longer brings up the panel: is the detector still right?");
    eprintln!("once a frame: the panel after {} frames", old.unwrap());
    // At the emulator's rate it never does: two minutes standing on the home
    // spot, then a minute of mashing the feet and the bopper.
    assert_eq!(first_panel(&state, &nand, None, 3 * 3600, 2 * 3600), None, "the panel came up");
}
