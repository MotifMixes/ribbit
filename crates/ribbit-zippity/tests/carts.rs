//! Scripted runs of the Zippity with a cartridge, which must reach given
//! states. They need dumps, which are not in the repository:
//!
//!   RIBBIT_ZIPPITY_NAND=<system NAND dump> RIBBIT_ZIPPITY_CART=<Disney Princess
//!   cartridge dump, raw> cargo test -p ribbit-zippity --release --test carts
//!
//! Without them the tests pass without running.

use ribbit_zippity::{Pad, Zippity};

fn dumps() -> Option<(Vec<u8>, Vec<u8>)> {
    let nand = std::env::var("RIBBIT_ZIPPITY_NAND").ok()?;
    let cart = std::env::var("RIBBIT_ZIPPITY_CART").ok()?;
    Some((std::fs::read(nand).expect("read the NAND dump"), std::fs::read(cart).expect("read the cartridge dump")))
}

macro_rules! dumps_or_skip {
    () => {
        match dumps() {
            Some(d) => d,
            None => {
                eprintln!("RIBBIT_ZIPPITY_NAND / RIBBIT_ZIPPITY_CART not set; skipping");
                return;
            }
        }
    };
}

fn run(z: &mut Zippity, frames: u32) {
    for _ in 0..frames {
        z.run_frame();
    }
}

/// Runs until the system's log (since power-on or the last state loaded)
/// shows `text`.
fn run_until(z: &mut Zippity, text: &str, limit: u32) {
    for _ in 0..limit {
        if z.log().contains(text) {
            return;
        }
        z.run_frame();
    }
    panic!("the log never showed {text:?}; it ends:\n{}", &z.log()[z.log().len().saturating_sub(600)..]);
}

/// Holds buttons for a few frames, then lets go for a while.
fn press(z: &mut Zippity, buttons: u16) {
    z.set_pad(Pad::held(buttons));
    run(z, 8);
    z.set_pad(Pad::IDLE);
    run(z, 40);
}

/// Powers on with the cartridge in, carries on from a saved state (the
/// cartridge must stay in the slot: states do not hold the flashes) and
/// signs in as the guest: the system loads the cartridge's own kernel.
fn to_cartridge_game(nand: Vec<u8>, cart: Vec<u8>) -> Zippity {
    let mut z = Zippity::new(nand.clone());
    z.insert_cartridge(cart.clone());
    run_until(&mut z, "callee_param: 229", 3000);
    let state = z.save_state();
    let mut z = Zippity::new(nand);
    z.insert_cartridge(cart);
    z.load_state(&state).unwrap();
    // The language (a dump without players) and Sign In screens: down three
    // times and choose takes the guest, the last of four.
    for _ in 0..40 {
        if z.log().contains("Kernal load test") {
            break;
        }
        for b in [Pad::DOWN, Pad::DOWN, Pad::DOWN, Pad::CHOOSE] {
            press(&mut z, b);
        }
        run(&mut z, 60);
    }
    run_until(&mut z, "Cart seated and sucessfully loaded!", 600);
    run_until(&mut z, "Loaded filesystem: OTP", 3000);
    z
}

#[test]
fn guest_plays_the_cartridge_after_a_state_load() {
    let (nand, cart) = dumps_or_skip!();
    let mut z = to_cartridge_game(nand, cart);
    run_until(&mut z, "MDP.P04", 600);
}

/// Disney Princess's Cinderella's Dress Dash places its pictures with the
/// maths unit's matrix product (0xF6004000): with no results the bead
/// counter was never drawn and beads were never picked up.
#[test]
fn dress_dash_uses_the_maths_unit() {
    let (nand, cart) = dumps_or_skip!();
    let mut z = to_cartridge_game(nand, cart);
    // The title, then the menu (Dress Dash first), level 1, and the "stand
    // on the mat" screen.
    run(&mut z, 1500);
    press(&mut z, Pad::CHOOSE);
    run(&mut z, 400);
    press(&mut z, Pad::CHOOSE);
    run(&mut z, 600);
    z.set_pad(Pad::held(Pad::LEFT_FOOT | Pad::RIGHT_FOOT));
    run(&mut z, 3000);
    let results: Vec<u32> = (0..3).map(|i| z.io_reg(0xF600_4060 + 4 * i)).collect();
    assert!(results.iter().any(|&r| r != 0), "the game ran the matrix product: {results:x?}");
}
