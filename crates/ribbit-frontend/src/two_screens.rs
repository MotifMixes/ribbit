//! EXPERIMENTAL: the Leapster L-MAX's two pictures, its LCD and its TV
//! output, while its TV chip is driven (Video > L-MAX TV output; core
//! `Machine::set_lmax_tv_output`). Like Cemu's GamePad / TV views: one picture,
//! both side by side or stacked, one big with the other small in a corner,
//! or each in a window (or on a display) of its own.
//!
//! Both pictures are the game's 160 x 160 (the TV chip's 200 x 160 frame
//! without its side borders, `Machine::render_tv`). What a real L-MAX shows
//! on each with a TV plugged in is only known from its BIOS (io/lmax_tv.rs):
//! v2.2 keeps the LCD on, US v2.1 blanks it for original-Leapster carts.
//!
//! The larger picture is drawn like the usual screen (AI upscale, HD
//! vectors); the other with the same filter, AI and HD aside (Sharp for
//! those). Equal sizes: the LCD is the larger one (HD's vectors follow it).

use crate::gpu::Display;

/// x, y, width, height in physical pixels.
pub type Rect = (f32, f32, f32, f32);

/// How the two pictures share the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Layout {
    /// The TV picture only (the TV, as a player at the TV sees it).
    TvOnly,
    /// The handheld's LCD only (as without a TV).
    LcdOnly,
    /// TV on the left, LCD on the right.
    SideBySide,
    /// TV on top, LCD below (a dual-screen handheld's arrangement).
    #[default]
    Stacked,
    /// The TV picture, the LCD small in the bottom-right corner.
    TvWithLcdInset,
    /// The LCD, the TV picture small in the bottom-right corner.
    LcdWithTvInset,
    /// Each in a window of its own (desktop: the TV picture in a second
    /// window; Android: on the device's second display, if it has one,
    /// else stacked).
    Separate,
}

impl Layout {
    /// In the order F6 steps through them.
    pub const ALL: [Layout; 7] = [
        Layout::Stacked,
        Layout::SideBySide,
        Layout::TvWithLcdInset,
        Layout::LcdWithTvInset,
        Layout::TvOnly,
        Layout::LcdOnly,
        Layout::Separate,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Layout::TvOnly => "TV only",
            Layout::LcdOnly => "LCD only",
            Layout::SideBySide => "Side by side",
            Layout::Stacked => "Stacked (TV top, LCD bottom)",
            Layout::TvWithLcdInset => "Big TV, small LCD",
            Layout::LcdWithTvInset => "Big LCD, small TV",
            Layout::Separate if cfg!(target_os = "android") => "Second display (else stacked)",
            Layout::Separate => "Separate windows",
        }
    }

    /// The next layout (F6).
    pub fn next(self) -> Layout {
        let at = Self::ALL.iter().position(|&l| l == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }
}

/// One of the two pictures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Picture {
    Tv,
    Lcd,
}

impl Picture {
    pub fn other(self) -> Picture {
        match self {
            Picture::Tv => Picture::Lcd,
            Picture::Lcd => Picture::Tv,
        }
    }
}

/// Where the pictures go. `main` is drawn first and gets the full display
/// path; `other`, if shown, after it (over it for an inset).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub main: (Picture, Rect),
    pub other: Option<(Picture, Rect)>,
}

impl Placed {
    /// Where `p` is, if it is shown.
    pub fn rect_of(&self, p: Picture) -> Option<Rect> {
        if self.main.0 == p {
            return Some(self.main.1);
        }
        self.other.filter(|o| o.0 == p).map(|o| o.1)
    }
}

/// Inset size: this share of the big picture's side.
const INSET: f32 = 1.0 / 3.0;
/// Gap between an inset and the area's edges: this share of the big side.
const MARGIN: f32 = 1.0 / 48.0;

/// `cols` x `rows` square pictures as large as fit in `area`, the group
/// centred; whole multiples of 160 for Pixel. The pictures' side and the
/// group's top-left corner.
fn grid(area: Rect, cols: f32, rows: f32, display: Display) -> (f32, f32, f32) {
    let (ax, ay, aw, ah) = area;
    let fit = (aw / cols).min(ah / rows);
    let side = match display {
        // A pixel of tolerance, as `Gpu::lcd_rect`.
        Display::Pixel => ((fit + 1.0) / 160.0).floor().max(1.0) * 160.0,
        _ => fit.max(1.0),
    };
    (ax + (aw - side * cols) / 2.0, ay + (ah - side * rows) / 2.0, side)
}

/// The layout seen with `swap`: one picture alone or a big one with an
/// inset becomes the other way round; the rest swap their places instead.
pub fn swapped(layout: Layout, swap: bool) -> Layout {
    match (layout, swap) {
        (Layout::TvOnly, true) => Layout::LcdOnly,
        (Layout::LcdOnly, true) => Layout::TvOnly,
        (Layout::TvWithLcdInset, true) => Layout::LcdWithTvInset,
        (Layout::LcdWithTvInset, true) => Layout::TvWithLcdInset,
        (l, _) => l,
    }
}

/// Where the pictures go in `area` for `layout`. `swap` exchanges the TV and
/// the LCD in every layout, as Cemu's swap does (TV only shows the LCD, the
/// big picture and the inset trade places). `Separate` places the window's
/// own picture (`separate_main`) alone; the other goes elsewhere.
pub fn place(layout: Layout, area: Rect, display: Display, swap: bool, separate_main: Picture) -> Placed {
    let (first, second) = if swap { (Picture::Lcd, Picture::Tv) } else { (Picture::Tv, Picture::Lcd) };
    let layout = swapped(layout, swap);
    let alone = |p| {
        let (x, y, s) = grid(area, 1.0, 1.0, display);
        Placed { main: (p, (x, y, s, s)), other: None }
    };
    // The larger picture is `main`; with equal sizes the LCD.
    let pair = |a: (Picture, Rect), b: (Picture, Rect)| {
        if a.0 == Picture::Lcd { Placed { main: a, other: Some(b) } } else { Placed { main: b, other: Some(a) } }
    };
    match layout {
        Layout::TvOnly => alone(Picture::Tv),
        Layout::LcdOnly => alone(Picture::Lcd),
        Layout::Separate => alone(separate_main),
        Layout::SideBySide => {
            let (x, y, s) = grid(area, 2.0, 1.0, display);
            pair((first, (x, y, s, s)), (second, (x + s, y, s, s)))
        }
        Layout::Stacked => {
            let (x, y, s) = grid(area, 1.0, 2.0, display);
            pair((first, (x, y, s, s)), (second, (x, y + s, s, s)))
        }
        Layout::TvWithLcdInset | Layout::LcdWithTvInset => {
            let big = if layout == Layout::TvWithLcdInset { Picture::Tv } else { Picture::Lcd };
            let (x, y, s) = grid(area, 1.0, 1.0, display);
            let small = match display {
                // Whole multiples too, when the big picture is large enough.
                Display::Pixel if s >= 480.0 => (s * INSET / 160.0).floor() * 160.0,
                _ => s * INSET,
            };
            // In the area's bottom-right corner: beside the big picture when
            // the window is wide enough, else over its corner.
            let (ax, ay, aw, ah) = area;
            let m = (s * MARGIN).round();
            let inset = (ax + aw - small - m, ay + ah - small - m, small, small);
            Placed { main: (big, (x, y, s, s)), other: Some((big.other(), inset)) }
        }
    }
}

/// The window's size in 160 x 160 units for `layout` (window sizes, "Fit
/// screen"): the pictures side by side or stacked need two.
pub fn units(layout: Layout) -> (u32, u32) {
    match layout {
        Layout::SideBySide => (2, 1),
        Layout::Stacked => (1, 2),
        _ => (1, 1),
    }
}

/// How the non-main picture is drawn: the chosen filter, but neither AI nor
/// (for the TV, which has no LCD cells) the LCD grid.
pub fn other_display(display: Display, p: Picture) -> Display {
    match (display, p) {
        (Display::Ai, _) | (Display::Lcd, Picture::Tv) => Display::Sharp,
        (d, _) => d,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WIDE: Rect = (0.0, 0.0, 1600.0, 900.0);
    const TALL: Rect = (0.0, 0.0, 900.0, 1600.0);

    fn overlaps(a: Rect, b: Rect) -> bool {
        a.0 < b.0 + b.2 && b.0 < a.0 + a.2 && a.1 < b.1 + b.3 && b.1 < a.1 + a.3
    }

    fn inside(r: Rect, area: Rect) -> bool {
        r.0 >= area.0 && r.1 >= area.1 && r.0 + r.2 <= area.0 + area.2 + 0.01 && r.1 + r.3 <= area.1 + area.3 + 0.01
    }

    #[test]
    fn stacked_puts_the_tv_on_top_and_fills_the_height() {
        let p = place(Layout::Stacked, WIDE, Display::Sharp, false, Picture::Tv);
        let (tv, lcd) = (p.rect_of(Picture::Tv).unwrap(), p.rect_of(Picture::Lcd).unwrap());
        assert_eq!(p.main.0, Picture::Lcd, "equal sizes: the LCD gets the full path");
        assert_eq!(tv, (575.0, 0.0, 450.0, 450.0));
        assert_eq!(lcd, (575.0, 450.0, 450.0, 450.0));
        // Swapped: the LCD on top.
        let s = place(Layout::Stacked, WIDE, Display::Sharp, true, Picture::Tv);
        assert_eq!(s.rect_of(Picture::Lcd).unwrap().1, 0.0);
        assert_eq!(s.rect_of(Picture::Tv).unwrap().1, 450.0);
    }

    #[test]
    fn side_by_side_is_centred_with_no_gap() {
        let p = place(Layout::SideBySide, TALL, Display::Smooth, false, Picture::Tv);
        let (tv, lcd) = (p.rect_of(Picture::Tv).unwrap(), p.rect_of(Picture::Lcd).unwrap());
        assert_eq!(tv, (0.0, 575.0, 450.0, 450.0));
        assert_eq!(lcd, (450.0, 575.0, 450.0, 450.0));
    }

    #[test]
    fn pixel_perfect_uses_whole_multiples() {
        for layout in Layout::ALL {
            let p = place(layout, (10.0, 20.0, 1000.0, 700.0), Display::Pixel, false, Picture::Lcd);
            for (_, r) in std::iter::once(p.main).chain(p.other) {
                assert_eq!(r.2 % 160.0, 0.0, "{layout:?}: {r:?}");
                assert!(inside(r, (10.0, 20.0, 1000.0, 700.0)), "{layout:?}: {r:?}");
            }
        }
        // Stacked in 1000 x 700: 2 x 160 = 320 per picture (640 high).
        let p = place(Layout::Stacked, (0.0, 0.0, 1000.0, 700.0), Display::Pixel, false, Picture::Lcd);
        assert_eq!(p.main.1, (340.0, 350.0, 320.0, 320.0));
    }

    #[test]
    fn insets_are_small_in_the_corner_and_drawn_last() {
        let p = place(Layout::TvWithLcdInset, WIDE, Display::Sharp, false, Picture::Tv);
        assert_eq!(p.main.0, Picture::Tv);
        let (pic, inset) = p.other.unwrap();
        assert_eq!(pic, Picture::Lcd);
        assert_eq!(inset.2, 300.0);
        assert!(inside(inset, WIDE));
        // A wide window: beside the big picture, not over it.
        assert!(!overlaps(inset, p.main.1));
        // A square window: over the big picture's corner.
        let sq = (0.0, 0.0, 900.0, 900.0);
        let r = place(Layout::LcdWithTvInset, sq, Display::Sharp, false, Picture::Tv);
        assert_eq!((r.main.0, r.other.unwrap().0), (Picture::Lcd, Picture::Tv));
        assert!(overlaps(r.other.unwrap().1, r.main.1) && inside(r.other.unwrap().1, sq));
    }

    #[test]
    fn single_pictures_and_separate_windows() {
        let only = |l| place(l, WIDE, Display::Sharp, false, Picture::Lcd);
        assert_eq!(only(Layout::TvOnly).main, (Picture::Tv, (350.0, 0.0, 900.0, 900.0)));
        assert_eq!(only(Layout::LcdOnly).main.0, Picture::Lcd);
        assert!(only(Layout::TvOnly).other.is_none() && only(Layout::Separate).other.is_none());
        assert_eq!(only(Layout::Separate).main.0, Picture::Lcd);
        assert_eq!(place(Layout::Separate, WIDE, Display::Sharp, false, Picture::Tv).main.0, Picture::Tv);
    }

    #[test]
    fn f6_visits_every_layout_once() {
        let mut l = Layout::default();
        let mut seen = vec![l];
        for _ in 1..Layout::ALL.len() {
            l = l.next();
            assert!(!seen.contains(&l));
            seen.push(l);
        }
        assert_eq!(l.next(), Layout::default());
        assert_eq!(units(Layout::Stacked), (1, 2));
        assert_eq!(other_display(Display::Ai, Picture::Lcd), Display::Sharp);
        assert_eq!(other_display(Display::Lcd, Picture::Tv), Display::Sharp);
        assert_eq!(other_display(Display::Lcd, Picture::Lcd), Display::Lcd);
    }

    /// Swap works in every layout, as Cemu's does.
    #[test]
    fn swap_exchanges_the_pictures_in_every_layout() {
        let area = (0.0, 0.0, 800.0, 800.0);
        for &l in Layout::ALL.iter().filter(|&&l| l != Layout::Separate) {
            let a = place(l, area, Display::Sharp, false, Picture::Lcd);
            let b = place(l, area, Display::Sharp, true, Picture::Lcd);
            for p in [Picture::Tv, Picture::Lcd] {
                assert_eq!(a.rect_of(p), b.rect_of(p.other()), "{l:?}");
            }
        }
        assert_eq!(swapped(swapped(Layout::TvWithLcdInset, true), true), Layout::TvWithLcdInset);
    }
}
