//! Where the pen is on the open book, in the cartridge's own page
//! coordinates, and the sensor readings and factory calibration that go
//! with it.
//!
//! What the firmware does (EVIDENCE: Tutter's Tiny Trip app bank 0x20000,
//! CPU 0xF77F "get pen" and 0xDD68 transform; docs/leappad.md, "Pen
//! position"):
//!
//! * The kernel's two axis ratios: **axis B** (IRAM 28h, drive codes 09/19)
//!   runs across the spread, **axis A** (2Ah, codes 11/69) down the pages.
//! * Axis B picks the page: right if above the midpoint of the left page's
//!   10th and the right page's 2nd calibration value.
//! * Each axis is mapped through 11 calibration values per page (the
//!   ratios at 11 known points) and the cartridge's table of those points'
//!   coordinates (17 words per page and axis; the first 11 are used),
//!   piecewise linearly; the result is divided by 8.
//! * The calibration values are built at power-on from the serial EEPROM
//!   (128 bytes): byte 0 = calibration version (4), byte 4 = pen sensor
//!   setup (threshold and step nibbles), four regions of 14 big-endian words
//!   (axis B left 0x0F, axis A left 0x2B, axis B right 0x47, axis A right
//!   0x63; the last is cut off after 13 words by the checksum), and at
//!   0x7E-0x7F the ones' complement of the 16-bit sum of bytes 0x00-0x7D.
//! * A region's first word is the reading at a point just off the page (the
//!   cartridge table's last entry: x -780 left, -1080 right, y -1080), and
//!   words 1-13 the readings at calibration points 0-12. Table entry i is
//!   word i + 1; the firmware smooths entries 0-2 and 6-10 with their
//!   neighbours, so the off-page word bends the edge entries. EVIDENCE: with
//!   the words one place later (Ribbit's first layout) the rebuilt tables
//!   folded back at the left and top edges and a touch at x = 63 read as 0;
//!   with this layout they come out straight to within 1% (lpboot XDUMP of
//!   F5C0/F5D6/F0AD/F0C3).
//!
//! The factory calibration here is that of an ideal sensor: readings linear
//! in position (HYPOTHESIS: the real sheet's response, and the real
//! fixture's calibration points, are unknown; any monotonic response gives
//! the same page coordinates once calibrated, which is what the cartridge
//! hit-tests).

/// Calibration points of one page and axis, in the cartridge's units
/// (Tutter's Tiny Trip 500-00406, app bank 0x20000 CPU 0xBDEA/0xBE2E; the
/// base ROM's app has the same tables: UNVERIFIED for other cartridges).
pub const POINTS_X: [[i32; 14]; 2] = [
    [-180, 240, 720, 1680, 2640, 3600, 4560, 5520, 6480, 6960, 7380, 8280, 9180, 10800],
    [120, 540, 1020, 1980, 2940, 3900, 4860, 5820, 6780, 7260, 7680, 8580, 9480, 11100],
];
pub const POINTS_Y: [i32; 14] = [-180, 240, 720, 1960, 3680, 5400, 7120, 8840, 10080, 10560, 10980, 11880, 12780, 13680];

/// The point just off each page that a region's first word is read at (the
/// cartridge table's last entries).
pub const OFF_PAGE_X: [i32; 2] = [-780, -1080];
pub const OFF_PAGE_Y: i32 = -1080;

/// Where the four regions start in the EEPROM: axis B left, axis A left,
/// axis B right, axis A right.
pub const REGIONS: [usize; 4] = [0x0F, 0x2B, 0x47, 0x63];

/// The page area in page units (the kernel's coordinates times 8): from the
/// first to the 11th calibration point.
pub fn page_extent(page: usize) -> ((i32, i32), (i32, i32)) {
    let x = POINTS_X[page.min(1)];
    ((x[0], x[10]), (POINTS_Y[0], POINTS_Y[10]))
}

/// Axis ratios (of 0x4000) of the ideal sensor across each page and down.
const B_RANGE: [(f32, f32); 2] = [(-15000.0, -1000.0), (1000.0, 15000.0)];
const A_RANGE: (f32, f32) = (-14000.0, 14000.0);

/// The kernel's two axis ratios (as fractions of 0x4000: axis B, axis A)
/// with the pen at (x, y) page units on `page` (0 = left, 1 = right).
pub fn ratios(page: usize, x: f32, y: f32) -> (f32, f32) {
    let page = page.min(1);
    let ((x0, x1), (y0, y1)) = page_extent(page);
    let lerp = |(a, b): (f32, f32), t: f32| a + (b - a) * t;
    let b = lerp(B_RANGE[page], (x - x0 as f32) / (x1 - x0) as f32);
    let a = lerp(A_RANGE, (y - y0 as f32) / (y1 - y0) as f32);
    (b / 16384.0, a / 16384.0)
}

/// The EEPROM byte for the pen sensor setup: touch threshold index 3 (5000
/// counts) and step 7. HYPOTHESIS: a mid-range choice; real units' values
/// are unknown.
pub const SENSOR_SETUP: u8 = 0x37;

/// A factory-calibrated EEPROM for the ideal sensor.
pub fn factory_eeprom() -> Vec<u8> {
    let mut d = vec![0u8; crate::eeprom::SIZE];
    d[0] = 4;
    d[4] = SENSOR_SETUP;
    let mut put = |base: usize, words: &[i32]| {
        for (j, &w) in words.iter().enumerate() {
            let at = base + 2 * j;
            if at + 1 < 0x7E {
                let w = w.clamp(-16000, 16000) as i16 as u16;
                d[at] = (w >> 8) as u8;
                d[at + 1] = w as u8;
            }
        }
    };
    for page in 0..2 {
        let xs = std::iter::once(OFF_PAGE_X[page]).chain(POINTS_X[page][..13].iter().copied());
        let ys = std::iter::once(OFF_PAGE_Y).chain(POINTS_Y[..13].iter().copied());
        let b: Vec<i32> = xs.map(|x| (ratios(page, x as f32, 0.0).0 * 16384.0).round() as i32).collect();
        let a: Vec<i32> = ys.map(|y| (ratios(page, 0.0, y as f32).1 * 16384.0).round() as i32).collect();
        put(REGIONS[2 * page], &b);
        put(REGIONS[2 * page + 1], &a);
    }
    seal(&mut d);
    d
}

/// Writes the checksum the kernel checks (0x7E-0x7F).
pub fn seal(d: &mut [u8]) {
    let sum = d[..0x7E].iter().map(|&b| b as u32).sum::<u32>() as u16;
    let c = 0xFFFF - sum;
    d[0x7E] = (c >> 8) as u8;
    d[0x7F] = c as u8;
}

/// Whether an EEPROM image is the factory calibration Ribbit wrote before
/// the regions' layout was corrected (nothing at 0x0F, -15000 at 0x11): it
/// passes the check but puts the page edges in the wrong places.
pub fn is_earlier_factory_image(d: &[u8]) -> bool {
    d.len() >= 0x13 && d[0x0F..0x11] == [0, 0] && d[0x11..0x13] == (-15000i16).to_be_bytes()
}

/// Whether an EEPROM image passes the kernel's calibration check.
pub fn is_calibrated(d: &[u8]) -> bool {
    if d.len() < 0x80 || d[0] != 4 {
        return false;
    }
    let sum = d[..0x7E].iter().map(|&b| b as u32).sum::<u32>() as u16;
    u16::from_be_bytes([d[0x7E], d[0x7F]]) == 0xFFFF - sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_image_passes_the_kernel_check() {
        let d = factory_eeprom();
        assert!(is_calibrated(&d));
        assert_eq!(d[0], 4);
        // Axis B, left page, word 4: the reading at x = 1680 (point 3).
        let w = i16::from_be_bytes([d[0x0F + 8], d[0x0F + 9]]);
        let want = (ratios(0, 1680.0, 0.0).0 * 16384.0).round() as i16;
        assert_eq!(w, want);
        assert!(!is_earlier_factory_image(&d));
    }

    #[test]
    fn pages_split_across_the_spread() {
        let (l, _) = ratios(0, 7380.0, 0.0);
        let (r, _) = ratios(1, 120.0, 0.0);
        assert!(l < 0.0 && r > 0.0, "left page reads below the right page");
        let (_, top) = ratios(0, 0.0, -180.0);
        let (_, bottom) = ratios(0, 0.0, 10980.0);
        assert!(top < bottom);
    }
}
