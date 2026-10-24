//! Virtual books: what Ribbit shows in place of the paper book on the pad.
//!
//! A LeapPad book has no electronics (docs/leappad.md, "How the toy is
//! used"): the pad senses where the pen is, and the cartridge decides what
//! that spot means on the page the player told it is open (by touching the
//! page's GO spot). So a virtual book is pages to look at plus positions,
//! in the cartridge's own page coordinates, and touching it only moves the
//! emulated pen: every response comes from the cartridge's software.
//!
//! The file format (TOML) is documented in docs/leappad-books.md.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The file format version this code reads.
pub const FORMAT: u32 = 1;

/// The page area in the cartridge's coordinates (what its app reports for a
/// touch: x across, y down, the same on both pages). From the calibration
/// points (calibration.rs), divided by 8 as the app does.
pub const PAGE_W: f32 = 945.0;
pub const PAGE_H: f32 = 1395.0;

/// Left or right page of a spread.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Side {
    Left,
    Right,
}

impl Side {
    pub fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

/// A point on a page.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub struct Spot {
    pub page: Side,
    pub x: f32,
    pub y: f32,
}

/// A named area of a page (for overlays, tests and debugging; the cartridge
/// does its own hit-testing).
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Region {
    pub name: String,
    pub page: Side,
    /// x0, y0, x1, y1 in page coordinates (for an outlined region, the box
    /// around its outline).
    pub rect: [f32; 4],
    /// Optional: the region's outline, a polygon of points (x, y), when it
    /// is not a rectangle.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub outline: Vec<[f32; 2]>,
    /// What the region is expected to do, for people reading the file.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// The word printed in the spot, when known (drawn in it). Cartridges
    /// hold no text: Ribbit gets it by transcribing what the spot says.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// What the spot says when touched, when it is more than a word (shown
    /// under the pen; heard, not printed).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub said: String,
    /// The spot is a sentence's button (a speech bubble before the line that
    /// reads the whole sentence; `said` holds it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sentence: bool,
    /// The spot is the page's stop sign (above the page number: it stops
    /// what is playing).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stop: bool,
}

impl Region {
    pub fn centre(&self) -> Spot {
        let [x0, y0, x1, y1] = self.rect;
        Spot { page: self.page, x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0 }
    }

    pub fn contains(&self, s: Spot) -> bool {
        let [x0, y0, x1, y1] = self.rect;
        if s.page != self.page || s.x < x0 || s.x > x1 || s.y < y0 || s.y > y1 {
            return false;
        }
        let p = &self.outline;
        if p.len() < 3 {
            return true;
        }
        let mut inside = false;
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            if (a[1] > s.y) != (b[1] > s.y) && s.x < a[0] + (s.y - a[1]) / (b[1] - a[1]) * (b[0] - a[0]) {
                inside = !inside;
            }
        }
        inside
    }
}

/// One opened pair of pages.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Spread {
    pub name: String,
    /// Page pictures, relative to the book file.
    #[serde(default)]
    pub left: Option<PathBuf>,
    #[serde(default)]
    pub right: Option<PathBuf>,
    /// The spread's printed GO spot: touching it tells the cartridge this
    /// spread is open.
    #[serde(default)]
    pub go: Option<Spot>,
    #[serde(default, rename = "region")]
    pub regions: Vec<Region>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Book {
    pub format: u32,
    pub title: String,
    /// The cartridge the book goes with: a part number (e.g. "500-00406").
    #[serde(default)]
    pub cartridge: Option<String>,
    /// Or its SHA-1, to tell versions apart.
    #[serde(default)]
    pub cartridge_sha1: Option<String>,
    #[serde(default, rename = "spread")]
    pub spreads: Vec<Spread>,
    /// The directory the file was in (page pictures are relative to it).
    #[serde(skip)]
    pub dir: PathBuf,
    /// Made by Ribbit from the cartridge's own layout (`from_cartridge`),
    /// not loaded from a file.
    #[serde(skip)]
    pub from_cartridge: bool,
}

/// Something wrong with a book file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// Errors stop the book loading; warnings are shown and it loads.
    pub error: bool,
    pub message: String,
}

impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", if self.error { "error" } else { "warning" }, self.message)
    }
}

impl Book {
    /// Reads and checks a book file. Returns the book with any warnings, or
    /// every problem found if it has errors.
    pub fn load(path: &Path) -> Result<(Book, Vec<Problem>), Vec<Problem>> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| vec![Problem { error: true, message: format!("cannot read {}: {e}", path.display()) }])?;
        Book::parse(&text, path.parent().unwrap_or(Path::new(".")))
    }

    /// Parses and checks book text; `dir` is where page pictures are looked
    /// for.
    pub fn parse(text: &str, dir: &Path) -> Result<(Book, Vec<Problem>), Vec<Problem>> {
        let mut book: Book =
            toml::from_str(text).map_err(|e| vec![Problem { error: true, message: format!("not a book file: {e}") }])?;
        book.dir = dir.to_path_buf();
        let problems = book.check();
        if problems.iter().any(|p| p.error) { Err(problems) } else { Ok((book, problems)) }
    }

    /// Everything wrong or doubtful in the book.
    pub fn check(&self) -> Vec<Problem> {
        let mut out = Vec::new();
        let mut err = |m: String| out.push(Problem { error: true, message: m });
        if self.format != FORMAT {
            err(format!("format {} is not supported (this Ribbit reads format {FORMAT})", self.format));
        }
        if self.title.trim().is_empty() {
            err("the book has no title".into());
        }
        if self.spreads.is_empty() {
            err("the book has no spreads".into());
        }
        let on_page = |x: f32, y: f32| (0.0..=PAGE_W).contains(&x) && (0.0..=PAGE_H).contains(&y);
        for (i, s) in self.spreads.iter().enumerate() {
            let at = format!("spread {} ({})", i + 1, s.name);
            if let Some(g) = s.go
                && !on_page(g.x, g.y)
            {
                err(format!("{at}: the GO spot ({}, {}) is off the page", g.x, g.y));
            }
            for r in &s.regions {
                let [x0, y0, x1, y1] = r.rect;
                if !(x0 < x1 && y0 < y1) {
                    err(format!("{at}: region \"{}\" has an empty or inverted rectangle", r.name));
                } else if !on_page(x0, y0) || !on_page(x1, y1) {
                    err(format!("{at}: region \"{}\" goes off the page (0-{PAGE_W} x 0-{PAGE_H})", r.name));
                }
                if r.outline.len() == 1 || r.outline.len() == 2 {
                    err(format!("{at}: region \"{}\" has an outline of fewer than 3 points", r.name));
                } else if r.outline.iter().any(|&[x, y]| !on_page(x, y)) {
                    err(format!("{at}: region \"{}\" has an outline point off the page", r.name));
                }
            }
        }
        let mut warn = |m: String| out.push(Problem { error: false, message: m });
        for (i, s) in self.spreads.iter().enumerate() {
            let at = format!("spread {} ({})", i + 1, s.name);
            for p in [&s.left, &s.right].into_iter().flatten() {
                if !self.dir.join(p).is_file() {
                    warn(format!("{at}: page picture {} not found; a plain page is shown", p.display()));
                }
            }
            if s.go.is_none() {
                warn(format!("{at}: no GO spot; the cartridge cannot be told this spread is open"));
            }
            for (a, ra) in s.regions.iter().enumerate() {
                for rb in &s.regions[a + 1..] {
                    if ra.name == rb.name {
                        warn(format!("{at}: two regions are called \"{}\"", ra.name));
                    }
                }
            }
        }
        if self.cartridge.is_none() && self.cartridge_sha1.is_none() {
            warn("the book does not say which cartridge it goes with".into());
        }
        out
    }

    /// Whether this book is for the cartridge with this part number / SHA-1.
    pub fn matches(&self, part: Option<&str>, sha1: &str) -> bool {
        match (&self.cartridge_sha1, &self.cartridge) {
            (Some(h), _) => h.eq_ignore_ascii_case(sha1),
            (None, Some(p)) => part.is_some_and(|q| q.eq_ignore_ascii_case(p)),
            (None, None) => false,
        }
    }

    /// A book drawn from the cartridge's own layout (layout.rs): a spread
    /// for each GO spot, with every spot its Touch-and-Tell view answers as
    /// a region. It has no pictures; Ribbit draws the outlines.
    pub fn from_cartridge(cart: &[u8], title: &str, part: Option<&str>) -> Result<Book, String> {
        let layout = crate::layout::read(cart)?;
        let clamp = |(x, y): (f32, f32)| [x.clamp(0.0, PAGE_W), y.clamp(0.0, PAGE_H)];
        let spreads = layout
            .spreads
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let view = layout.main_view(s);
                // Mode 1's spots (G:01: whole words), each once; the other
                // modes' spots (letters, sounds) lie inside the same words.
                let mut seen = std::collections::HashSet::new();
                let regions = view.map_or(Vec::new(), |v| {
                    v.hotspots
                        .iter()
                        .filter(|h| h.modes & 1 != 0 && seen.insert(h.id))
                        .map(|h| {
                            let [x0, y0, x1, y1] = h.shape.bounds();
                            let [x0, y0] = clamp((x0, y0));
                            let [x1, y1] = clamp((x1, y1));
                            Region {
                                name: format!("{} {}", h.kind_name(), h.id),
                                page: h.shape.page,
                                rect: [x0, y0, x1, y1],
                                outline: if h.shape.rect { Vec::new() } else { h.shape.points.iter().map(|&p| clamp(p)).collect() },
                                note: String::new(),
                                text: String::new(),
                                said: String::new(),
                                sentence: false,
                                stop: false,
                            }
                        })
                        .collect()
                });
                let name = view.map_or_else(|| format!("Spread {}", i + 1), |v| v.name.clone());
                Spread { name, left: None, right: None, go: s.go.inside_point(), regions }
            })
            .collect::<Vec<_>>();
        if spreads.is_empty() {
            return Err("the cartridge lists no GO spots".into());
        }
        Ok(Book {
            format: FORMAT,
            title: title.to_string(),
            cartridge: part.map(str::to_string),
            cartridge_sha1: None,
            spreads,
            dir: PathBuf::new(),
            from_cartridge: true,
        })
    }

    /// Forgets the words put into a spread's spots (before newer ones).
    pub fn clear_words(&mut self, spread: usize) {
        for r in self.spreads.get_mut(spread).into_iter().flat_map(|s| s.regions.iter_mut()) {
            r.text.clear();
            r.said.clear();
            r.sentence = false;
            r.stop = false;
        }
    }

    /// Fills in the spots' words from a words file (tab-separated: spread,
    /// region index, region name, `word`, `phrase`, `sentence` or `stop`, text; written by
    /// transcribing what each spot says). A line applies only where the
    /// spread's region of that index has that name. Returns how many applied.
    pub fn apply_words(&mut self, tsv: &str) -> usize {
        let mut applied = 0;
        for line in tsv.lines() {
            let f: Vec<&str> = line.split('\t').collect();
            let [spread, region, name, kind, text] = f[..] else { continue };
            let (Ok(spread), Ok(region)) = (spread.parse::<usize>(), region.parse::<usize>()) else { continue };
            let Some(r) = self.spreads.get_mut(spread).and_then(|s| s.regions.get_mut(region)) else { continue };
            if r.name != name || text.trim().is_empty() {
                continue;
            }
            match kind {
                "word" => r.text = text.trim().to_string(),
                "phrase" => r.said = text.trim().to_string(),
                "sentence" => {
                    r.said = text.trim().to_string();
                    r.sentence = true;
                }
                "stop" => r.stop = true,
                _ => continue,
            }
            applied += 1;
        }
        applied
    }

    /// The book as a book file (TOML), e.g. to add page pictures to a book
    /// drawn from the cartridge.
    pub fn to_toml(&self) -> String {
        toml::to_string(self).expect("a book is plain data")
    }

    /// The picture file for a page, if the book has one.
    pub fn picture(&self, spread: usize, side: Side) -> Option<PathBuf> {
        let s = self.spreads.get(spread)?;
        let p = match side {
            Side::Left => s.left.as_ref(),
            Side::Right => s.right.as_ref(),
        }?;
        Some(self.dir.join(p))
    }
}

/// The pen readings (the kernel's two axis ratios, see pen.rs) with the pen
/// on this spot of an open spread. The spread itself does not matter: pages
/// lie in the same place whichever is open.
pub fn pen_ratios(s: Spot) -> (f32, f32) {
    crate::calibration::ratios(s.page.index(), s.x * 8.0, s.y * 8.0)
}

/// Where a point of the shown spread falls, given as fractions of the shown
/// spread's width and height (0..1 each; the left page is the left half).
pub fn spot_at(fx: f32, fy: f32) -> Option<Spot> {
    if !(0.0..=1.0).contains(&fx) || !(0.0..=1.0).contains(&fy) {
        return None;
    }
    let (page, px) = if fx < 0.5 { (Side::Left, fx * 2.0) } else { (Side::Right, fx * 2.0 - 1.0) };
    Some(Spot { page, x: px * PAGE_W, y: fy * PAGE_H })
}

/// The touch the cartridge's app logs on its debug port, e.g.
/// `DT:v_  X 8 Z 5(0: 495, 623)[Touch]` (right page: y + 8192).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TouchReport {
    pub spot: Spot,
}

/// Every touch report in the firmware's debug text, in order.
pub fn touch_reports(log: &str) -> Vec<TouchReport> {
    let mut out = Vec::new();
    for line in log.lines() {
        // The 1999 app (500-00263) leaves out "[Touch]": `DT:v_  X15 Z 5(0: 265,  73)`.
        let Some(dt) = line.find("DT") else { continue };
        let Some(open) = line[dt..].find('(').map(|o| dt + o) else { continue };
        let Some(end) = line[open..].find(')').map(|e| open + e) else { continue };
        let inner = &line[open + 1..end];
        let Some((_, xy)) = inner.split_once(':') else { continue };
        let Some((x, y)) = xy.split_once(',') else { continue };
        let (Ok(x), Ok(y)) = (x.trim().parse::<i32>(), y.trim().parse::<i32>()) else { continue };
        let (page, y) = if y >= 8192 { (Side::Right, y - 8192) } else { (Side::Left, y) };
        out.push(TouchReport { spot: Spot { page, x: x as f32, y: y as f32 } });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOOK: &str = r#"
format = 1
title = "Test"
cartridge = "500-00406"

[[spread]]
name = "One"
go = { page = "left", x = 100, y = 100 }

  [[spread.region]]
  name = "A"
  page = "right"
  rect = [100, 100, 300, 300]
"#;

    #[test]
    fn parses_a_book() {
        let (b, warnings) = Book::parse(BOOK, Path::new(".")).expect("valid");
        assert_eq!(b.spreads.len(), 1);
        assert_eq!(b.spreads[0].regions[0].centre(), Spot { page: Side::Right, x: 200.0, y: 200.0 });
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(b.matches(Some("500-00406"), "x"));
        assert!(!b.matches(Some("500-00730"), "x"));
    }

    #[test]
    fn reports_every_error() {
        let bad = BOOK.replace("format = 1", "format = 9").replace("[100, 100, 300, 300]", "[300, 100, 100, 300]");
        let e = Book::parse(&bad, Path::new(".")).unwrap_err();
        assert!(e.iter().any(|p| p.message.contains("format 9")));
        assert!(e.iter().any(|p| p.message.contains("inverted")));
        let e = Book::parse("title = 3", Path::new(".")).unwrap_err();
        assert!(e[0].message.starts_with("not a book file"));
    }

    #[test]
    fn warns_about_missing_pictures_and_go() {
        let t = BOOK.replace("go = { page = \"left\", x = 100, y = 100 }", "left = \"nope.png\"");
        let (_, w) = Book::parse(&t, Path::new(".")).expect("loads with warnings");
        assert!(w.iter().any(|p| p.message.contains("nope.png")));
        assert!(w.iter().any(|p| p.message.contains("no GO spot")));
    }

    #[test]
    fn outlines_are_checked_and_hit_tested() {
        let t = BOOK.replace("rect = [100, 100, 300, 300]", "rect = [100, 100, 300, 300]
  outline = [[100, 100], [300, 100], [100, 300]]");
        let (b, _) = Book::parse(&t, Path::new(".")).expect("valid");
        let r = &b.spreads[0].regions[0];
        assert!(r.contains(Spot { page: Side::Right, x: 120.0, y: 120.0 }));
        assert!(!r.contains(Spot { page: Side::Right, x: 280.0, y: 280.0 }), "outside the triangle, inside the box");
        let bad = t.replace("[[100, 100], [300, 100], [100, 300]]", "[[100, 100], [300, 100]]");
        assert!(Book::parse(&bad, Path::new(".")).unwrap_err().iter().any(|p| p.message.contains("fewer than 3")));
    }

    #[test]
    fn view_points_map_to_pages() {
        assert_eq!(spot_at(0.25, 0.5), Some(Spot { page: Side::Left, x: PAGE_W / 2.0, y: PAGE_H / 2.0 }));
        assert_eq!(spot_at(0.75, 0.0).map(|s| s.page), Some(Side::Right));
        assert_eq!(spot_at(1.2, 0.5), None);
    }

    #[test]
    fn reads_the_apps_touch_reports() {
        let log = "FGsc\n{DT}v_  X 8 Z 5(0: 495, 623)[Touch]\nx\n{DT}v_  X 0 Z 5(0: 872,9441)[Touch]\nDT:v_  X15 Z 5(0: 265,  73) G:02\n";
        let r = touch_reports(log);
        assert_eq!(r.len(), 3);
        assert_eq!(r[2].spot, Spot { page: Side::Left, x: 265.0, y: 73.0 }, "the 1999 app's form");
        assert_eq!(r[0].spot, Spot { page: Side::Left, x: 495.0, y: 623.0 });
        assert_eq!(r[1].spot, Spot { page: Side::Right, x: 872.0, y: 1249.0 });
    }
}
