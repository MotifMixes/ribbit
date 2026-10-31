//! The Leapster Explorer's 4-wire resistive touch screen.
//!
//! Two resistive sheets: X1 and X2 are the ends of one, Y1 and Y2 of the
//! other, on GPIO port A pins 11-14 (gpio_map.h EMERALD_TOUCHSCREEN_*) and
//! ADC channels 4-7 (lf1000_ts2.c `first_adc` = 4 on Emerald). A touch joins
//! the sheets at one point. The kernel drives some ends high or low, leaves
//! the others as inputs (some with pull-ups) and reads their levels: the
//! GPIO pads at boot (gpio_main.c check_for_touchscreen: X2 must follow X1,
//! Y2 follow Y1) and the ADC for touch/no-touch, position and pressure
//! (lf1000_ts2.c read_xy). Ribbit solves that resistor network for every
//! reading, so all of them come out of the same model.

/// Sheet resistance end to end, the contact resistance of a touch and the
/// GPIO pull-up, in ohms. GUESSES (the real values are UNKNOWN); with them a
/// touch reads about 10 in touch/no-touch mode (the driver wants <= 19,
/// MAX_TNT_DOWN) and no touch 1023 (> 520, MIN_TNT_UP).
const R_SHEET: f64 = 400.0;
const R_TOUCH: f64 = 300.0;
const R_PULLUP: f64 = 100_000.0;
/// The ADC input and pin leakage: keeps an undriven node defined (at 0 V).
const R_LEAK: f64 = 10_000_000.0;

/// The first of the four pins (X1, Y1, X2, Y2 in that order, as the ADC
/// channels are).
pub const FIRST_PIN: u32 = 11;
pub const PINS: u32 = 0xF << FIRST_PIN;
pub const FIRST_ADC: u32 = 4;

/// The calibration in use, a0..a6: screen = (a1*x + a2*y + a0) / a6 and
/// (a4*x + a5*y + a3) / a6 from the averaged ADC readings (lf1000_ts2.c
/// get_touch). These are the firmware's lf1000_ts2.ko defaults, which
/// /usr/bin/setcal keeps when neither /flags/pointercal nor the NOR's
/// MfgData has one (it logs them as a1 a2 a0 a4 a5 a3 a6: "-29676 -170
/// 25786444 -632 -26927 21441222 65536"). A larger X reading is further
/// left, a larger Y reading higher up.
const CAL: [f64; 7] = [25_786_444.0, -29_676.0, -170.0, 21_441_222.0, -632.0, -26_927.0, 65_536.0];
/// The same in pointercal order (a1 a2 a0 a4 a5 a3 a6), as Ribbit's NOR
/// MfgData gives it.
pub const POINTERCAL: [i32; 7] = [-29_676, -170, 25_786_444, -632, -26_927, 21_441_222, 65_536];

/// The ADC readings the driver averages into its position for a screen
/// point (the calibration solved backwards, aiming at the pixel's middle).
fn raw_for(sx: f64, sy: f64) -> (f64, f64) {
    let [a0, a1, a2, a3, a4, a5, a6] = CAL;
    let (u, v) = ((sx + 0.5) * a6 - a0, (sy + 0.5) * a6 - a3);
    let det = a1 * a5 - a2 * a4;
    ((u * a5 - v * a2) / det, (a1 * v - a4 * u) / det)
}

/// The voltages (0..1) on X1, Y1, X2, Y2, from port A's output, output
/// enable and pull-up registers and the point touched (screen pixels).
pub fn levels(out: u32, out_enb: u32, pull_up: u32, touch: Option<(f32, f32)>) -> [f64; 4] {
    // Where the touch is along each sheet, from its first end. The driver's
    // X position is the voltage on the Y sheet's contact point with Y1 high
    // and Y2 low (set_read_x), so 1 - fy; its Y position 1 - fx likewise.
    let (fx, fy) = touch.map_or((0.5, 0.5), |(x, y)| {
        let (rx, ry) = raw_for(x as f64, y as f64);
        ((1.0 - ry / 1023.0).clamp(0.0, 1.0), (1.0 - rx / 1023.0).clamp(0.0, 1.0))
    });
    // Nodes 0-3 the pins, 4 the X sheet's contact point, 5 the Y sheet's.
    let mut links = vec![
        (0, 4, R_SHEET * fx),
        (4, 2, R_SHEET * (1.0 - fx)),
        (1, 5, R_SHEET * fy),
        (5, 3, R_SHEET * (1.0 - fy)),
    ];
    if touch.is_some() {
        links.push((4, 5, R_TOUCH));
    }
    let driven = |n: usize| n < 4 && out_enb >> (FIRST_PIN + n as u32) & 1 != 0;
    let mut v = [0.0; 6];
    for n in 0..4 {
        if driven(n) {
            v[n] = (out >> (FIRST_PIN + n as u32) & 1) as f64;
        }
    }
    // Nodal analysis over the undriven nodes: sum of G * (V - Vother) = I.
    let free: Vec<usize> = (0..6).filter(|&n| !driven(n)).collect();
    let k = free.len();
    let mut g = vec![vec![0.0; k + 1]; k];
    let row = |n: usize| free.iter().position(|&f| f == n);
    for &(a, b, r) in &links {
        let c = 1.0 / r.max(1.0);
        for (p, q) in [(a, b), (b, a)] {
            if let Some(i) = row(p) {
                g[i][i] += c;
                match row(q) {
                    Some(j) => g[i][j] -= c,
                    None => g[i][k] += c * v[q],
                }
            }
        }
    }
    for (i, &n) in free.iter().enumerate() {
        g[i][i] += 1.0 / R_LEAK;
        if n < 4 && pull_up >> (FIRST_PIN + n as u32) & 1 != 0 {
            g[i][i] += 1.0 / R_PULLUP;
            g[i][k] += 1.0 / R_PULLUP;
        }
    }
    // Gaussian elimination with partial pivoting (at most 6 unknowns).
    for c in 0..k {
        let p = (c..k).max_by(|&a, &b| g[a][c].abs().total_cmp(&g[b][c].abs())).unwrap();
        g.swap(c, p);
        for r in 0..k {
            if r != c {
                let f = g[r][c] / g[c][c];
                for j in c..=k {
                    g[r][j] -= f * g[c][j];
                }
            }
        }
    }
    for (i, &n) in free.iter().enumerate() {
        v[n] = g[i][k] / g[i][i];
    }
    [v[0], v[1], v[2], v[3]]
}

/// A 10-bit ADC reading of a level.
pub fn adc(level: f64) -> u32 {
    (level * 1023.0).round().clamp(0.0, 1023.0) as u32
}

/// The pins' levels as the GPIO pad register reads them (port A bits).
pub fn pad(levels: [f64; 4]) -> u32 {
    (0..4).filter(|&n| levels[n] > 0.5).fold(0, |p, n| p | 1 << (FIRST_PIN + n as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    const X1: u32 = 1 << 11;
    const Y1: u32 = 1 << 12;
    const X2: u32 = 1 << 13;
    const Y2: u32 = 1 << 14;

    /// The driver's read_xy and get_touch for one point: (screen x, y,
    /// touch/no-touch reading).
    fn measure(touch: Option<(f32, f32)>) -> (i64, i64, u32) {
        let read = |out, enb, pu, ch: usize| adc(levels(out, enb, pu, touch)[ch]);
        // set_read_tnt (mode 0): X ends inputs with pull-ups, Y ends low.
        let tnt = read(0, Y1 | Y2, X1 | X2, 0);
        // set_read_x: Y1 high, Y2 low, X ends read; set_read_y the other way.
        let x = (read(Y1, Y1 | Y2, 0, 0) + read(Y1, Y1 | Y2, 0, 2)) as i64 / 2;
        let y = (read(X1, X1 | X2, 0, 1) + read(X1, X1 | X2, 0, 3)) as i64 / 2;
        let c = CAL.map(|a| a as i64);
        ((c[1] * x + c[2] * y + c[0]) / c[6], (c[4] * x + c[5] * y + c[3]) / c[6], tnt)
    }

    #[test]
    fn touches_land_on_their_pixel() {
        for (sx, sy) in [(0.0, 0.0), (319.0, 239.0), (160.0, 120.0), (17.0, 201.0), (300.0, 5.0)] {
            let (x, y, tnt) = measure(Some((sx, sy)));
            assert_eq!((x, y), (sx as i64, sy as i64));
            assert!(tnt <= 19, "touch/no-touch {tnt}");
        }
        assert!(measure(None).2 > 520);
    }

    #[test]
    fn boot_check_finds_the_screen() {
        // X1 driven, X2 an input with a pull-up: X2 follows X1.
        for high in [false, true] {
            let out = if high { X1 | Y1 } else { 0 };
            let p = pad(levels(out, X1 | Y1, X2 | Y2, None));
            assert_eq!(p & X2 != 0, high);
            assert_eq!(p & Y2 != 0, high);
        }
    }
}
