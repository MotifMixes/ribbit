//! Automatic touch calibration: while the BIOS's calibration screen shows a
//! target (a 9-pixel plus on an otherwise black screen), press its centre
//! with the stylus, as a player would. Runs on the emulation thread in
//! emulated time, so it does not depend on the window or the host's speed.
//! While it is busy the emulation thread runs unpaced and the window keeps
//! the screen black, so the player never sees it.

use ribbit_core::Machine;
use ribbit_video::{HEIGHT, WIDTH};

/// How often to look for a target, how long to press it, and how long to
/// wait after letting go (the BIOS needs the stylus up between targets).
const CHECK_MS: u64 = 50;
const PRESS_MS: u64 = 300;
const RELEASE_MS: u64 = 300;
/// Still calibrating if the next target appears within this long of the
/// last press (the BIOS shows it within about a second).
const NEXT_TARGET_MS: u64 = 1500;

pub struct AutoCalibrate {
    frame: Box<[u32; WIDTH * HEIGHT]>,
    /// Cycle of the next look for a target.
    next_check: u64,
    /// Pressing until this cycle.
    pressing: Option<u64>,
    /// Calibrating until this cycle (extended by each target found).
    busy_until: u64,
}

impl AutoCalibrate {
    pub fn new() -> Self {
        Self { frame: Box::new([0; WIDTH * HEIGHT]), next_check: 0, pressing: None, busy_until: 0 }
    }

    /// Whether the calibration screen is up (or about to show its next target).
    pub fn busy(&self, m: &Machine) -> bool {
        m.cpu.cycles < self.busy_until
    }

    /// Call after each emulation chunk.
    pub fn tick(&mut self, m: &mut Machine) {
        let now = m.cpu.cycles;
        let ms = m.clock.cpu_hz / 1000;
        if let Some(until) = self.pressing {
            if now >= until {
                if let Some(t) = m.io.touch.as_mut() {
                    t.live = None;
                }
                self.pressing = None;
                self.next_check = now + RELEASE_MS * ms;
            }
            return;
        }
        // The cycle counter restarts after a reset or a loaded state.
        if now < self.next_check.saturating_sub((CHECK_MS + RELEASE_MS) * ms) {
            self.next_check = now;
            self.busy_until = 0;
        }
        if now < self.next_check || m.io.touch.is_none() {
            return;
        }
        self.next_check = now + CHECK_MS * ms;
        m.render_frame(&mut self.frame);
        let Some((x, y)) = target(&self.frame) else {
            return;
        };
        tracing::info!(target: "ribbit::touch", x, y, "auto-calibration: pressing target");
        // MAME's crosshair mapping (0..=0x7F0 across 160 pixels), as for the mouse.
        let raw = |p: usize| ((p as u32 * 0x7F0 + 79) / 159).min(0x7F0);
        if let Some(t) = m.io.touch.as_mut() {
            t.live = Some((raw(x), raw(y)));
        }
        self.pressing = Some(now + PRESS_MS * ms);
        self.busy_until = now + (PRESS_MS + NEXT_TARGET_MS) * ms;
    }
}

/// The centre of a lone plus-shaped target (exactly 9 lit pixels), if the
/// screen shows one.
pub fn target(frame: &[u32; WIDTH * HEIGHT]) -> Option<(usize, usize)> {
    let mut lit = Vec::with_capacity(9);
    for (i, &p) in frame.iter().enumerate() {
        if p & 0x00FF_FFFF != 0 {
            if lit.len() == 9 {
                return None;
            }
            lit.push((i % WIDTH, i / WIDTH));
        }
    }
    // Row-major order puts the centre fifth.
    let &(x, y) = lit.get(4)?;
    let mut want = [
        (x, y.checked_sub(2)?),
        (x, y - 1),
        (x.checked_sub(2)?, y),
        (x - 1, y),
        (x, y),
        (x + 1, y),
        (x + 2, y),
        (x, y + 1),
        (x, y + 2),
    ];
    want.sort_by_key(|&(x, y)| (y, x));
    (lit == want).then_some((x, y))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plus(cx: usize, cy: usize) -> Box<[u32; WIDTH * HEIGHT]> {
        let mut f = Box::new([0; WIDTH * HEIGHT]);
        for (x, y) in [(0, -2), (0, -1), (-2, 0), (-1, 0), (0, 0), (1, 0), (2, 0), (0, 1), (0, 2)] {
            f[(cy as i32 + y) as usize * WIDTH + (cx as i32 + x) as usize] = 0xFFFFFF;
        }
        f
    }

    #[test]
    fn finds_each_bios_target() {
        for (x, y) in [(10, 10), (10, 150), (150, 150), (150, 10), (80, 80)] {
            assert_eq!(target(&plus(x, y)), Some((x, y)));
        }
    }

    #[test]
    fn ignores_other_screens() {
        assert_eq!(target(&Box::new([0; WIDTH * HEIGHT])), None);
        let mut f = plus(80, 80);
        f[0] = 0xFFFFFF;
        assert_eq!(target(&f), None);
    }
}
