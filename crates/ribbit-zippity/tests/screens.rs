//! Pictures of known screens, as hashes: the system's start-up and player
//! screens, and The Princess and the Frog cartridge's title and menu (a
//! 16-bit YUV layer, palette-and-opacity sprites, transparent colours).
//! Needs the dumps, so it is skipped without them:
//!
//!   RIBBIT_ZIPPITY_NAND=<system NAND dump (raw), US, no players>
//!   RIBBIT_ZIPPITY_FROG=<The Princess and the Frog cartridge dump (raw);
//!   only the cartridge route needs it>
//!   cargo test -p ribbit-zippity --release --test screens -- --nocapture
//!
//! RIBBIT_ZIPPITY_SHOTS=<dir> also saves each picture there (PPM). The
//! route is scripted in instructions, so a change to the CPU's timing or
//! the devices moves the screens: look at the saved pictures, then take the
//! new hashes the failure prints.

use ribbit_zippity::{SCREEN_H, SCREEN_W, Zippity};

/// Controller state (active-low) pressed over a span of instructions.
const PAD: &[(u16, u64, u64)] = &[
    // Language: United States English.
    (0x37F, 900_000_000, 909_000_000),
    // Sign In: the first (empty) player.
    (0x37F, 1_095_000_000, 1_102_500_000),
    // Name entry: A, then down three times and right seven times to OK.
    (0x37F, 1_500_000_000, 1_506_000_000),
    (0x3FD, 1_518_000_000, 1_524_000_000),
    (0x3FD, 1_536_000_000, 1_542_000_000),
    (0x3FD, 1_554_000_000, 1_560_000_000),
    (0x3F7, 1_572_000_000, 1_578_000_000),
    (0x3F7, 1_590_000_000, 1_596_000_000),
    (0x3F7, 1_608_000_000, 1_614_000_000),
    (0x3F7, 1_626_000_000, 1_632_000_000),
    (0x3F7, 1_644_000_000, 1_650_000_000),
    (0x3F7, 1_662_000_000, 1_668_000_000),
    (0x3F7, 1_680_000_000, 1_686_000_000),
    (0x37F, 1_698_000_000, 1_704_000_000),
    // The player screen: OK. The cartridge's game starts.
    (0x3F7, 1_941_000_000, 1_947_000_000),
    (0x37F, 1_959_000_000, 1_965_000_000),
];

/// (instructions, screen, picture hash).
const SHOTS: &[(u64, &str, u64)] = &[
    (870_000_000, "language", 0x32c2_fdb3_6024_c01f),
    (1_440_000_000, "name entry", 0x1450_8ef0_b559_80ea),
    (1_875_000_000, "player", 0xd442_3d44_a7a7_a873),
    (2_550_000_000, "cartridge title", 0x59d1_0bb7_fdb2_8a3a),
    (3_000_000_000, "cartridge menu", 0x0d56_b064_d1a5_b2c6),
];

/// Without a cartridge the new player gets the system's tutorial (Mickey
/// Says) first: stepping on the mat and the spots, then the bopper, until
/// it ends on the game-select ring (bubbles placed through the maths
/// unit, RGB 5:6:5 pictures, fading reflections).
fn tutorial() -> Vec<(u16, u64, u64)> {
    let mut pad = Vec::new();
    for (from, rounds, states) in [(2_415, 40, &[0x0FF, 0x07F, 0x1FF, 0x2FF][..]), (4_050, 20, &[0x3FE, 0x3FD, 0x3FB, 0x3F7, 0x0FF][..])] {
        let mut t = from * 1_000_000u64;
        for _ in 0..rounds {
            for &s in states {
                pad.push((s, t, t + 6_000_000));
                t += 10_500_000;
            }
        }
    }
    pad
}

fn hash(img: &[u32]) -> u64 {
    img.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &p| (h ^ p as u64).wrapping_mul(0x0100_0000_01B3))
}

/// Runs the route, checking the pictures at `shots`.
fn check(cart: bool, pad: &[(u16, u64, u64)], shots: &[(u64, &str, u64)]) {
    let Some(nand) = std::env::var_os("RIBBIT_ZIPPITY_NAND") else {
        eprintln!("RIBBIT_ZIPPITY_NAND not set; skipping");
        return;
    };
    let mut z = Zippity::new(std::fs::read(&nand).expect("read the NAND dump"));
    if cart {
        let Some(cart_file) = std::env::var_os("RIBBIT_ZIPPITY_FROG") else {
            eprintln!("RIBBIT_ZIPPITY_FROG not set; skipping");
            return;
        };
        z.insert_cartridge(std::fs::read(&cart_file).expect("read the cartridge dump"));
    }
    z.probe().pad_script.extend_from_slice(pad);
    let save = std::env::var_os("RIBBIT_ZIPPITY_SHOTS").map(std::path::PathBuf::from);
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    let mut wrong = Vec::new();
    for &(at, name, want) in shots {
        z.run(at - z.instructions());
        z.render(&mut img);
        let got = hash(&img);
        println!("{at:>13} {name}: {got:#018x}");
        if let Some(dir) = &save {
            let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
            img.iter().for_each(|p| ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
            std::fs::write(dir.join(format!("{}.ppm", name.replace(' ', "_"))), ppm).unwrap();
        }
        if got != want {
            wrong.push(format!("{name} ({at}): {got:#018x}, expected {want:#018x}"));
        }
    }
    assert!(wrong.is_empty(), "pictures changed:\n{}", wrong.join("\n"));
}

#[test]
fn cartridge_route() {
    check(true, PAD, SHOTS);
}

#[test]
fn game_select_ring() {
    let pad: Vec<_> = PAD.iter().copied().chain(tutorial()).collect();
    check(false, &pad, &[(5_400_000_000, "game-select ring", 0xfe4c_34b9_9a95_34df)]);
}

/// From the game-select ring into Little Einsteins' Rocket's Musical
/// Mission, marching: the water is a colour-filled sprite (light blue
/// shading darker downwards; drawn as a picture it was noise) and the
/// seabed is on text layer 4. Times in frames of `FRAME` instructions.
#[test]
fn rocket_musical_mission() {
    use ribbit_zippity::FRAME;
    const GREEN: u16 = 0x37F;
    const FEET: u16 = 0x0FF;
    let f = |a: u64, b: u64, s: u16| (s, a * FRAME, b * FRAME);
    let mut pad: Vec<_> = PAD.iter().copied().chain(tutorial()).collect();
    // The ring stops on My Friends Tigger & Pooh: turn it twice to Little
    // Einsteins and choose it, then Rocket's Musical Mission and level 1,
    // standing on the home spot.
    const LEFT: u16 = 0x3FB;
    pad.extend([f(12_010, 12_018, LEFT), f(12_040, 12_048, LEFT)]);
    let d = 70;
    pad.extend([f(12_010 + d, 12_018 + d, GREEN), f(13_210 + d, 13_218 + d, GREEN), f(13_218 + d, 13_358 + d, FEET)]);
    pad.extend([f(13_358 + d, 13_366 + d, GREEN), f(13_366 + d, 16_200 + d, FEET)]);
    // March: left foot down, both, right foot down, both.
    for i in 0..120 {
        let t = 16_210 + d + 24 * i;
        pad.extend([f(t, t + 8, 0x2FF), f(t + 8, t + 12, FEET), f(t + 12, t + 20, 0x1FF), f(t + 20, t + 24, FEET)]);
    }
    pad.push(f(19_090 + d, 19_300 + d, FEET));
    let Some(nand) = std::env::var_os("RIBBIT_ZIPPITY_NAND") else {
        eprintln!("RIBBIT_ZIPPITY_NAND not set; skipping");
        return;
    };
    let mut z = Zippity::new(std::fs::read(&nand).expect("read the NAND dump"));
    z.probe().pad_script.extend_from_slice(&pad);
    z.run((19_200 + d) * FRAME - z.instructions());
    let mut img = vec![0u32; SCREEN_W * SCREEN_H];
    z.render(&mut img);
    println!("rocket's musical mission: {:#018x}", hash(&img));
    if let Some(dir) = std::env::var_os("RIBBIT_ZIPPITY_SHOTS") {
        let mut ppm = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
        img.iter().for_each(|p| ppm.extend_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8]));
        std::fs::write(std::path::Path::new(&dir).join("rocket.ppm"), ppm).unwrap();
    }
    // The water between the two screens at the top: blue, red low, and
    // smooth (neighbouring pixels within a few levels), away from the fish.
    for y in 10..70 {
        for x in 130..300 {
            let p = img[y * SCREEN_W + x];
            let (r, b) = (p >> 16 & 255, p & 255);
            assert!(b > 90 && r < 40, "({x}, {y}) is {p:06x}, not water");
            let q = img[y * SCREEN_W + x + 1];
            let d = |s: u32| ((p >> s & 255) as i32 - (q >> s & 255) as i32).abs();
            assert!(d(0) < 8 && d(8) < 8, "({x}, {y}) {p:06x} next to {q:06x}");
        }
    }
    assert_eq!(hash(&img), 0xfd82_b995_8d14_2332, "picture changed (look at it with RIBBIT_ZIPPITY_SHOTS)");
}
