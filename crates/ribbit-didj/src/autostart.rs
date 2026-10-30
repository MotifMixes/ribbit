//! Starts a chosen game from the menu by pressing the buttons a player
//! would, after recognising each screen (VERIFIED in Ribbit with firmware
//! 1.35.2.4222, a set-up console, English):
//!
//! 1. "Select a name": the first profile is highlighted; A.
//! 2. The main wheel, at "My Didj": DOWN to "My Games", then A.
//! 3. "Select a Game" (a cartridge first, then the games on the console):
//!    DOWN until the panel shows the game's art, then A.
//!
//! The firmware itself is untouched. Nothing is pressed on a screen it
//! doesn't recognise (a console still being set up, another language, a
//! popup): it waits, and gives up after a while, leaving the menu as it is.
//! A button pressed by the player ends it too.

use crate::machine::button;

/// The screen's size.
const W: usize = 320;
const H: usize = 240;

/// White text masks (every channel above 200) of the menu as Ribbit draws
/// it: the headers "Select a name" and "Select a Game" in `HEADER`, the
/// wheel's panel labels "My Didj" and "My Games" in `LABEL`. Bits row by
/// row, the lowest bit first.
const MASKS: &[u8] = include_bytes!("menu_masks.bin");
/// x, y, width, height.
const HEADER: (usize, usize, usize, usize) = (150, 6, 168, 24);
const LABEL: (usize, usize, usize, usize) = (150, 150, 110, 16);
/// Differing pixels allowed (the other screens differ by 140 or more).
const MASK_SLACK: usize = 50;

/// The panel draws the game's art texture (meta.inf `LargeIcon` when it is
/// 256x128, else `PreviewImage`) unscaled with its top-left near here.
const ART_AT: (usize, usize) = (129, 62);
/// The part of the art compared: the picture above the name bar.
const ART_PART: (usize, usize, usize, usize) = (4, 4, 170, 70);
/// Mean difference per channel accepted (21 for the right game, 84 for
/// another, Ribbit's drawing).
const ART_SLACK: u32 = 40;

/// Frames the compared parts of the screen must stay unchanged before a
/// press (the wheel's avatar, beside them, never stops moving).
const STILL: u32 = 10;
/// Frames a button is held.
const HOLD: u32 = 6;
/// Gives up after this many frames (two minutes).
const LIMIT: u32 = 2 * 60 * 60;
/// DOWN presses on "Select a Game" before giving up.
const MAX_GAMES: u32 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Profile,
    Wheel,
    Games,
    Done,
}

/// The game's art (ARGB, `width` wide): its texture as the menu loads it.
#[derive(Clone)]
pub struct Art {
    pub width: usize,
    pub pixels: Vec<u32>,
}

pub struct AutoStart {
    art: Art,
    step: Step,
    frames: u32,
    still: u32,
    last: u64,
    /// Frames left to hold `pressed`.
    hold: u32,
    pressed: u32,
    games_tried: u32,
}

impl AutoStart {
    pub fn new(art: Art) -> Self {
        Self { art, step: Step::Profile, frames: 0, still: 0, last: 0, hold: 0, pressed: 0, games_tried: 0 }
    }

    pub fn done(&self) -> bool {
        self.step == Step::Done && self.hold == 0
    }

    /// Called once a frame with the picture (0xRRGGBB, `W` x `H`) and the
    /// buttons the player holds; returns the buttons to press.
    pub fn frame(&mut self, px: &[u32], player: u32) -> u32 {
        if player != 0 || self.frames > LIMIT {
            (self.step, self.hold) = (Step::Done, 0);
        }
        if self.hold > 0 {
            self.hold -= 1;
            return self.pressed;
        }
        if self.step == Step::Done || px.len() != W * H {
            return 0;
        }
        self.frames += 1;
        let (ax, ay, aw, ah) = ART_PART;
        let art = (ART_AT.0 + ax, ART_AT.1 + ay, aw, ah);
        let hash = [HEADER, LABEL, art]
            .iter()
            .flat_map(|&(x, y, w, h)| (y..y + h).flat_map(move |r| px[r * W + x..r * W + x + w].iter()))
            .fold(0xcbf2_9ce4_8422_2325u64, |h, &p| (h ^ p as u64).wrapping_mul(0x100_0000_01b3));
        self.still = if hash == self.last { self.still + 1 } else { 0 };
        self.last = hash;
        if self.still < STILL {
            return 0;
        }
        let press = match self.step {
            Step::Profile if matches(px, HEADER, 0) => {
                self.step = Step::Wheel;
                button::A
            }
            Step::Profile | Step::Wheel if matches(px, LABEL, 2) => {
                self.step = Step::Wheel;
                button::DOWN
            }
            Step::Profile | Step::Wheel if matches(px, LABEL, 3) => {
                self.step = Step::Games;
                button::A
            }
            Step::Games if matches(px, HEADER, 1) => {
                if self.art_shown(px) {
                    self.step = Step::Done;
                    button::A
                } else if self.games_tried < MAX_GAMES {
                    self.games_tried += 1;
                    button::DOWN
                } else {
                    self.step = Step::Done;
                    0
                }
            }
            _ => 0,
        };
        if press != 0 {
            // Held for HOLD frames; the next press waits for the screen to
            // settle again.
            (self.hold, self.pressed, self.still) = (HOLD - 1, press, 0);
        }
        press
    }

    /// Whether the panel shows the art (a few pixels either way).
    fn art_shown(&self, px: &[u32]) -> bool {
        let (ax, ay, aw, ah) = ART_PART;
        let rows = self.art.pixels.len() / self.art.width.max(1);
        if self.art.width < ax + aw || rows < ay + ah {
            return false;
        }
        let diff = |x0: usize, y0: usize| {
            let (mut sum, mut n) = (0u64, 0u64);
            for y in ay..ay + ah {
                for x in ax..ax + aw {
                    let a = self.art.pixels[y * self.art.width + x];
                    if a >> 24 != 0xFF {
                        continue;
                    }
                    let p = px[(y0 + y) * W + x0 + x];
                    sum += (0..3).map(|k| ((a >> (8 * k)) as u8).abs_diff((p >> (8 * k)) as u8) as u64).sum::<u64>();
                    n += 3;
                }
            }
            if n == 0 { u64::MAX } else { sum / n }
        };
        (ART_AT.1 - 4..=ART_AT.1 + 4)
            .flat_map(|y| (ART_AT.0 - 4..=ART_AT.0 + 4).map(move |x| (x, y)))
            .any(|(x, y)| diff(x, y) < ART_SLACK as u64)
    }
}

/// Whether `px` shows mask `n` (0, 1 headers; 2, 3 labels) in `region`.
fn matches(px: &[u32], region: (usize, usize, usize, usize), n: usize) -> bool {
    let (rx, ry, rw, rh) = region;
    let header_bytes = (HEADER.2 * HEADER.3).div_ceil(8);
    let label_bytes = (LABEL.2 * LABEL.3).div_ceil(8);
    let start = if n < 2 { n * header_bytes } else { 2 * header_bytes + (n - 2) * label_bytes };
    let mask = &MASKS[start..];
    let mut differ = 0;
    for y in 0..rh {
        for x in 0..rw {
            let i = y * rw + x;
            let want = mask[i / 8] >> (i % 8) & 1 != 0;
            let p = px[(ry + y) * W + rx + x];
            let white = (0..3).all(|k| (p >> (8 * k)) as u8 > 200);
            differ += (want != white) as usize;
        }
    }
    differ < MASK_SLACK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_masks_fit_their_regions() {
        let bytes = |(_, _, w, h): (usize, usize, usize, usize)| (w * h).div_ceil(8);
        assert_eq!(MASKS.len(), 2 * bytes(HEADER) + 2 * bytes(LABEL));
    }

    #[test]
    fn nothing_is_pressed_on_an_unknown_screen_and_the_player_ends_it() {
        let mut a = AutoStart::new(Art { width: 256, pixels: vec![0xFF00_0000; 256 * 128] });
        let black = vec![0; W * H];
        assert!((0..100).all(|_| a.frame(&black, 0) == 0));
        assert!(!a.done());
        a.frame(&black, button::A);
        assert!(a.done());
    }
}
