//! The touch layout a cartridge's app keeps: where every touchable spot of
//! the book is, read from the cartridge itself (docs/leappad.md, "The
//! cartridge's touch layout"). EVIDENCE: read from the app's own lookup code
//! (500-00406) and checked by touching the spots on the running cartridge
//! (tests/books.rs).
//!
//! * The app header (pointer at cartridge 0x1013C; its byte 0 is 2 in this
//!   app) lists **views** (count at +17h, table at +19h): a name, a pointer
//!   to the view's data, and three bytes, the first being the view's spread.
//!   A Touch-and-Tell view (data byte +1 = 0) has its list of **clickables**
//!   at +14h.
//! * The header's word at +1Fh points to the **GO spots**, one per spread in
//!   book order: a shape, then a word pointing to 3 bytes whose last two are
//!   the view the spot opens. Bit 14 of the shape ends the list.
//! * The header's 16-bit pointers are addresses in its 32 KB bank, and a
//!   view's in the view data's bank.
//! * A clickable is a type byte (Simple, Modal …), a mode mask, two more
//!   bytes, and a **shape index** at +4 (BFFFh: none; bit 14 ends the list),
//!   then data that depends on the type.
//! * Shape index n (bit 15 set) is entry n of the word table at the pointer
//!   in cartridge 0x10134; entry × 4 from the table is the shape. Indexes
//!   without bit 15 use the base ROM's table (not read here).
//! * A shape is a list of big-endian points (x, y), 13 bits each. The top bits
//!   of the first x give the kind (1: a rectangle given by two corners, 2: a
//!   polygon), the top bits of the first y the page (0 left, 1 right), and
//!   bit 15 of an x marks a polygon's last point.
//! * The app tests the clickables in list order and the first whose shape
//!   holds the pen wins; a polygon holds the points it encloses (even-odd).

use crate::book::{Side, Spot};

/// Where the cartridge is in physical memory (its own pointers use it).
const CART: u32 = 0x140_0000;

/// The clickable types, as the app names them in its debug log; the last,
/// "Touch", is Ribbit's name for a spot of a State Table Model view (`STM`).
pub const KINDS: [&str; 14] = [
    "Simple", "Modal", "MSwitch", "GSwitch", "Series", "BGSwitch", "BGToggle", "VSwitch", "MVSwitch", "PlayBack",
    "Counter", "AudCntrl", "SmpCtrld", "Touch",
];

/// Data byte +1 of a view of the "Prebuilt STM" app (State Table Model,
/// 2004: Cars, 500-11374), whose spots `stm_touches` reads.
pub const STM: u8 = 0x13;
/// The clickable type Ribbit gives those spots (`KINDS`).
const TOUCH: u8 = 13;

/// A touchable area of one page.
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub page: Side,
    /// A rectangle's two corners, or a polygon's points in order.
    pub points: Vec<(f32, f32)>,
    pub rect: bool,
}

impl Shape {
    /// The outline: a rectangle's four corners, or the polygon.
    pub fn outline(&self) -> Vec<(f32, f32)> {
        if self.rect {
            let [x0, y0, x1, y1] = self.bounds();
            vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
        } else {
            self.points.clone()
        }
    }

    /// x0, y0, x1, y1.
    pub fn bounds(&self) -> [f32; 4] {
        let mut b = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
        for &(x, y) in &self.points {
            b = [b[0].min(x), b[1].min(y), b[2].max(x), b[3].max(y)];
        }
        b
    }

    pub fn contains(&self, s: Spot) -> bool {
        if s.page != self.page {
            return false;
        }
        if self.rect {
            let [x0, y0, x1, y1] = self.bounds();
            return (x0..=x1).contains(&s.x) && (y0..=y1).contains(&s.y);
        }
        // The app's test (app bank 0x20000, CPU 0x4FF9-0x549A), in its integer
        // arithmetic: count the edges that span the pen's x (from the lower
        // x, inclusive, to the higher, exclusive) and pass at or above the
        // pen; odd is inside. A pen exactly on an edge is inside.
        let (px, py) = (s.x.round() as i64, s.y.round() as i64);
        let p: Vec<(i64, i64)> = self.points.iter().map(|&(x, y)| (x as i64, y as i64)).collect();
        let mut count = 0;
        for i in 0..p.len() {
            let (a, b) = (p[i], p[(i + 1) % p.len()]);
            if !(a.0.min(b.0) <= px && px < a.0.max(b.0)) {
                continue;
            }
            if py > a.1 && py > b.1 {
                count += 1;
                continue;
            }
            if py < a.1.min(b.1) {
                continue;
            }
            // Keil's signed long division truncates toward zero, as Rust's does.
            let y = a.1 + (b.1 - a.1) * (px - a.0) / (b.0 - a.0);
            if y <= py {
                count += 1;
            }
            if y == py {
                return true;
            }
            // Its on-the-edge check for steep edges compares x; for shallow
            // ones the code reuses the same comparison with the y it found
            // (a quirk, kept so that Ribbit agrees with it).
            let (dx, dy) = ((b.0 - a.0).abs(), (b.1 - a.1).abs());
            let near = if dx < dy { a.0 + (b.0 - a.0) * (py - a.1) / (b.1 - a.1) } else { y };
            if near == px {
                return true;
            }
        }
        count % 2 == 1
    }

    /// A point well inside the shape: the middle of the widest stretch
    /// across it, on one of a few heights.
    pub fn inside_point(&self) -> Option<Spot> {
        let [x0, y0, x1, y1] = self.bounds();
        if self.rect {
            return Some(Spot { page: self.page, x: (x0 + x1) / 2.0, y: (y0 + y1) / 2.0 });
        }
        let mut best: Option<(f32, Spot)> = None;
        for f in [0.5, 0.4, 0.6, 0.3, 0.7, 0.2, 0.8] {
            let y = (y0 + (y1 - y0) * f).floor() + 0.5;
            let p = &self.points;
            let mut xs: Vec<f32> = (0..p.len())
                .filter_map(|i| {
                    let (a, b) = (p[i], p[(i + 1) % p.len()]);
                    ((a.1 > y) != (b.1 > y)).then(|| a.0 + (y - a.1) / (b.1 - a.1) * (b.0 - a.0))
                })
                .collect();
            xs.sort_by(f32::total_cmp);
            for w in xs.chunks(2).filter(|w| w.len() == 2) {
                let spot = Spot { page: self.page, x: (w[0] + w[1]) / 2.0, y };
                if best.is_none_or(|(len, _)| w[1] - w[0] > len) {
                    best = Some((w[1] - w[0], spot));
                }
            }
        }
        best.map(|(_, s)| s)
    }
}

/// A clickable: a spot the app answers when touched.
#[derive(Clone, Debug, PartialEq)]
pub struct Hotspot {
    /// Index into `KINDS`.
    pub kind: u8,
    /// The modes it answers in, one bit each (FFh: all). Classic pages list
    /// a word once per mode (read it, sound it out …).
    pub modes: u8,
    /// The shape index, as the app logs it ("Simple:32769").
    pub id: u16,
    pub shape: Shape,
}

impl Hotspot {
    pub fn kind_name(&self) -> &'static str {
        KINDS.get(self.kind as usize).copied().unwrap_or("?")
    }
}

/// A state of a spread, with the clickables the app tests in it.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub index: usize,
    pub name: String,
    pub spread: u8,
    /// Data byte +1: 0 for Touch and Tell, whose clickables are read; other
    /// kinds (questions, games) lay out their data differently.
    pub kind: u8,
    pub hotspots: Vec<Hotspot>,
}

/// A spread of the book, as its GO spot announces it.
#[derive(Clone, Debug, PartialEq)]
pub struct Spread {
    pub go: Shape,
    /// The view touching GO opens.
    pub opens: usize,
}

/// What a cartridge says about its book.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub views: Vec<View>,
    /// In book order.
    pub spreads: Vec<Spread>,
}

impl Layout {
    /// The Touch-and-Tell view of a spread: the one its GO spot opens, or
    /// else the first of its spread.
    pub fn main_view(&self, spread: &Spread) -> Option<&View> {
        let opens = self.views.iter().find(|v| v.index == spread.opens)?;
        if opens.kind == 0 || opens.kind == STM {
            return Some(opens);
        }
        self.views.iter().find(|v| v.kind == 0 && v.spread == opens.spread)
    }
}

struct Rom<'a>(&'a [u8]);

impl Rom<'_> {
    fn at(&self, o: usize, n: usize) -> Result<&[u8], String> {
        self.0.get(o..o + n).ok_or_else(|| format!("the layout points outside the cartridge ({o:#X})"))
    }
    fn u8(&self, o: usize) -> Result<u8, String> {
        Ok(self.at(o, 1)?[0])
    }
    fn u16(&self, o: usize) -> Result<u16, String> {
        let b = self.at(o, 2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    /// A physical address stored in the cartridge, as an offset into it.
    fn ptr(&self, o: usize) -> Result<Option<usize>, String> {
        let b = self.at(o, 4)?;
        let p = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
        Ok((CART..CART + self.0.len() as u32).contains(&p).then(|| (p - CART) as usize))
    }
}

/// Shape `id` (bit 15 set: in the cartridge's table) or None if the app
/// would never find the pen in it. An entry of the table is the shape's
/// distance from the table in units of `scale` bytes (see `read`).
fn shape(rom: &Rom, table: Option<usize>, scale: usize, id: u16) -> Result<Option<Shape>, String> {
    let (Some(table), true) = (table, id & 0x8000 != 0 && id != 0xBFFF) else { return Ok(None) };
    let mut o = table + scale * rom.u16(table + 2 * (id & 0x3FFF) as usize)? as usize;
    let (x, y) = (rom.u16(o)?, rom.u16(o + 2)?);
    // The app's hit test treats other kinds as never touched.
    let rect = match x >> 13 & 3 {
        1 => true,
        2 => false,
        _ => return Ok(None),
    };
    let page = if y >> 13 & 1 == 0 { Side::Left } else { Side::Right };
    let mut points = Vec::new();
    loop {
        let (x, y) = (rom.u16(o)?, rom.u16(o + 2)?);
        points.push(((x & 0x1FFF) as f32, (y & 0x1FFF) as f32));
        o += 4;
        if (rect && points.len() == 2) || (!rect && x & 0x8000 != 0) {
            break;
        }
        if points.len() > 1000 {
            return Err(format!("shape {id:#06X} does not end"));
        }
    }
    Ok(Some(Shape { page, points, rect }))
}

/// Reads the layout from a cartridge image.
pub fn read(cart: &[u8]) -> Result<Layout, String> {
    let rom = Rom(cart);
    let header = rom.ptr(0x1013C)?.ok_or("the cartridge has no app header")?;
    // Version 2 is the "Prebuilt TnT, QnA3 & SnF" app; 0 is the LeapPad
    // Classic app, which keeps the same GO list, clickables and shapes under
    // a smaller header.
    let classic = match rom.u8(header)? {
        2 => false,
        0 => true,
        v => return Err(format!("the cartridge's app is of an unknown kind (header version {v})")),
    };
    let table = rom.ptr(0x10134)?;
    // The header's own pointers are 16-bit addresses in its 32 KB bank.
    let hbank = header & !0x7FFF;
    let go_list = hbank + rom.u16(header + if classic { 7 } else { 0x1F })? as usize;
    // The table's entries count 4-byte units in every kernel read so far but
    // the oldest: LeapPad 1.5 of 08/26/99 (500-00263, kernel 0x1F39) and that
    // of 07/14/00 (500-11874) add the entry as bytes, while e.g. 500-11887's
    // shifts it left by 2 (kernel 0x26C1). Ribbit takes the scale under which
    // every GO spot is a shape.
    let mut go_ids = Vec::new();
    let mut o = go_list;
    loop {
        let id = rom.u16(o)?;
        go_ids.push((id, rom.u16(hbank + rom.u16(o + 2)? as usize + 1)? as usize));
        if id & 0x4000 != 0 {
            break;
        }
        o += 4;
        if go_ids.len() > 1000 {
            return Err("the GO list does not end".into());
        }
    }
    let fits = |scale| go_ids.iter().all(|&(id, _)| matches!(shape(&rom, table, scale, id & 0xBFFF), Ok(Some(_))));
    let scale = if classic && !fits(4) && fits(1) { 1 } else { 4 };
    let shape = |id: u16| shape(&rom, table, scale, id);
    // Each view: its name, its data and its spread. The Classic header's +5
    // lists the views' data (32-bit pointers, up to the GO list); a view has
    // no name there, and its byte +2 is its spread.
    let mut entries = Vec::new();
    if classic {
        let mut o = hbank + rom.u16(header + 5)? as usize;
        while o + 4 <= go_list {
            let Some(data) = rom.ptr(o)? else { break };
            let spread = rom.u8(data + 2)?;
            entries.push((format!("Page {spread}"), Some(data), spread));
            o += 4;
        }
    } else {
        let count = rom.u16(header + 0x17)? as usize;
        let table = hbank + rom.u16(header + 0x19)? as usize;
        for index in 0..count {
            let entry = table + 9 * index;
            let name_at = hbank + rom.u16(entry)? as usize;
            let name =
                rom.0[name_at.min(rom.0.len())..].iter().take(40).take_while(|&&c| c != 0).map(|&c| c as char).collect();
            entries.push((name, rom.ptr(entry + 2)?, rom.u8(entry + 6)?));
        }
    }
    let mut spreads = Vec::new();
    for (id, opens) in go_ids {
        if let Some(go) = shape(id & 0xBFFF)? {
            spreads.push(Spread { go, opens });
        }
    }
    let mut views = Vec::new();
    let mut stm = Vec::new();
    for (index, (name, data, spread)) in entries.into_iter().enumerate() {
        let Some(data) = data else { continue };
        let kind = rom.u8(data + 1)?;
        if kind == STM {
            stm.push((views.len(), data));
        }
        let mut hotspots = Vec::new();
        let mut o = (data & !0x7FFF) + rom.u16(data + 0x14)? as usize;
        // Only Touch-and-Tell views' clickables are read.
        for n in 0.. {
            if kind != 0 {
                break;
            }
            let ty = rom.u8(o)?;
            let id = rom.u16(o + 4)?;
            let size = match ty {
                0 | 7 => 8,
                1 | 8 => 8 + 2 * (rom.u8(data + 0x25)? as usize).saturating_sub(1),
                2..=6 | 9 | 11 => 10,
                10 => 14,
                12 => 9,
                _ => return Err(format!("view {name}: clickable of unknown type {ty}")),
            };
            if rom.u8(o + 3)? & 7 != 0
                && let Some(s) = shape(id & 0xBFFF)?
            {
                hotspots.push(Hotspot { kind: ty, modes: rom.u8(o + 1)?, id: id & 0xBFFF, shape: s });
            }
            if id & 0x4000 != 0 {
                break;
            }
            o += size;
            if n > 4000 {
                return Err(format!("view {name}: the clickable list does not end"));
            }
        }
        views.push(View { index, name, spread, kind, hotspots });
    }
    if !stm.is_empty() {
        stm_touches(&rom, &mut views, &stm, &shape)?;
    }
    Ok(Layout { views, spreads })
}

/// The spots of the State Table Model app's views (`STM`). EVIDENCE: traced
/// in Cars (500-11374) touching spread 2 (the app's reads, lpwatch), and
/// checked by touching every spot found (tests/books.rs).
///
/// * A view's data holds 32-bit pointers: +9 to a run of state descriptors,
///   +0Dh to its entry code, +11h to its first state table. A state table is
///   a count and that many pointers to states; a state, a count and pointers
///   to its event records.
/// * An event record is a pointer to its action code, the event type and an
///   argument. A touch event (type 09h) goes on with `02 00` and a pointer to
///   its condition: at +9 the number of shapes, at +0Ch the list of their
///   indexes (bit 15 set, as the clickables' shape indexes), at +10h a list
///   just after it.
/// * States reached from other states are found through the descriptors
///   only, whose layout is not known: Ribbit finds every touch record by its
///   form, walks back to the state table holding it, and gives it to the view
///   whose descriptors (or +11h) point to that table, taking the view whose
///   descriptor run starts last before the pointer in the same 32 KB bank.
fn stm_touches(rom: &Rom, views: &mut [View], stm: &[(usize, usize)], shape: &dyn Fn(u16) -> Result<Option<Shape>, String>) -> Result<(), String> {
    let len = rom.0.len();
    let ptr = |o: usize| rom.ptr(o).ok().flatten();
    let u16_at = |o: usize| rom.u16(o).ok();
    // Where each pointer is stored.
    let mut refs: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
    for o in 0..len.saturating_sub(3) {
        if rom.0[o] == 1
            && let Some(p) = ptr(o)
        {
            refs.entry(p).or_default().push(o);
        }
    }
    // The starts of the count-and-pointers lists that point to `x`.
    let lists_holding = |x: usize| -> Vec<usize> {
        let mut out = Vec::new();
        for &at in refs.get(&x).into_iter().flatten() {
            for j in 0..32 {
                let Some(s) = at.checked_sub(4 * j + 1) else { break };
                let n = rom.0[s] as usize;
                if n > j && n <= 32 && (0..n).all(|q| ptr(s + 1 + 4 * q).is_some()) && !out.contains(&s) {
                    out.push(s);
                }
            }
        }
        out
    };
    let starts: Vec<(usize, usize, Option<usize>)> = stm.iter().map(|&(v, data)| (v, data, ptr(data + 9))).collect();
    let owner = |at: usize| -> Option<usize> {
        if let Some(&(v, ..)) = starts.iter().find(|&&(_, data, _)| at == data + 9 || at == data + 0x11) {
            return Some(v);
        }
        starts
            .iter()
            .filter_map(|&(v, _, run)| run.filter(|&r| r <= at && r >> 15 == at >> 15).map(|r| (r, v)))
            .max()
            .map(|(_, v)| v)
    };
    let mut seen: Vec<std::collections::HashSet<u16>> = vec![Default::default(); views.len()];
    for o in 0..len.saturating_sub(12) {
        if rom.0[o + 4] != 9 || rom.0[o + 6] != 2 || rom.0[o + 7] != 0 || ptr(o).is_none() {
            continue;
        }
        let Some(cond) = ptr(o + 8) else { continue };
        let (Some(count), Some(list)) = (u16_at(cond + 9).map(usize::from), ptr(cond + 12)) else { continue };
        let ids: Option<Vec<u16>> = (0..count).map(|k| u16_at(list + 2 * k)).collect();
        let Some(ids) = ids.filter(|ids| !ids.is_empty() && ids.iter().all(|id| id & 0x8000 != 0)) else { continue };
        if ptr(cond + 16) != Some(list + 2 * count) {
            continue;
        }
        let mut owners = Vec::new();
        for state in lists_holding(o) {
            for table in lists_holding(state) {
                for &at in refs.get(&table).into_iter().flatten() {
                    if let Some(v) = owner(at).filter(|v| !owners.contains(v)) {
                        owners.push(v);
                    }
                }
            }
        }
        for v in owners {
            for &id in &ids {
                let id = id & 0xBFFF;
                if seen[v].insert(id)
                    && let Some(s) = shape(id)?
                {
                    views[v].hotspots.push(Hotspot { kind: TOUCH, modes: 0xFF, id, shape: s });
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cartridge image with one spread: a GO rectangle on the left page, and
    /// a view whose clickables are a rectangle and a triangle on the right.
    fn cartridge() -> Vec<u8> {
        let mut d = vec![0u8; 0x50000];
        let put16 = |d: &mut Vec<u8>, at: usize, v: u16| d[at..at + 2].copy_from_slice(&v.to_be_bytes());
        let put32 = |d: &mut Vec<u8>, at: usize, v: u32| d[at..at + 4].copy_from_slice(&v.to_be_bytes());
        put32(&mut d, 0x10134, CART + 0x30000); // shape table
        put32(&mut d, 0x1013C, CART + 0x40000); // app header
        // Shapes (entry × 4 from the table): 0 the GO tab, 1 a rectangle, 2 a triangle.
        let shapes: [&[(u16, u16)]; 3] = [
            &[(0x2000 | 10, 30), (100, 130)],
            &[(0x2000 | 200, 0x2000 | 300), (400, 500)],
            &[(0x4000 | 500, 0x2000 | 100), (700, 100), (0x8000 | 500, 300)],
        ];
        let mut at = 0x30000 + 0x10;
        for (i, s) in shapes.iter().enumerate() {
            put16(&mut d, 0x30000 + 2 * i, ((at - 0x30000) / 4) as u16);
            for &(x, y) in s.iter() {
                put16(&mut d, at, x);
                put16(&mut d, at + 2, y);
                at += 4;
            }
        }
        let h = 0x40000;
        d[h] = 2;
        put16(&mut d, h + 0x17, 1); // one view
        put16(&mut d, h + 0x19, 0x100); // its table
        put16(&mut d, h + 0x1F, 0x200); // the GO list
        put16(&mut d, h + 0x100, 0x300); // view 0: name,
        put32(&mut d, h + 0x102, CART + 0x40400); // data,
        d[h + 0x106] = 0; // spread 0
        d[h + 0x300..h + 0x305].copy_from_slice(b"Pg23\0");
        put16(&mut d, h + 0x200, 0xC000); // GO: shape 0, last
        put16(&mut d, h + 0x202, 0x210);
        put16(&mut d, h + 0x211, 0); // opens view 0
        put16(&mut d, 0x40414, 0x500); // the view's clickables
        d[0x40500..0x40508].copy_from_slice(&[0, 0xFF, 0, 3, 0x80, 1, 0, 0]);
        d[0x40508..0x40510].copy_from_slice(&[0, 0xFF, 0, 3, 0xC0, 2, 0, 0]);
        d
    }

    #[test]
    fn reads_views_shapes_and_go_spots() {
        let l = read(&cartridge()).expect("reads");
        assert_eq!(l.spreads.len(), 1);
        assert_eq!(l.spreads[0].go, Shape { page: Side::Left, points: vec![(10.0, 30.0), (100.0, 130.0)], rect: true });
        let v = l.main_view(&l.spreads[0]).expect("a Touch-and-Tell view");
        assert_eq!(v.name, "Pg23");
        assert_eq!(v.hotspots.len(), 2);
        assert_eq!((v.hotspots[0].kind_name(), v.hotspots[0].id), ("Simple", 0x8001));
        assert_eq!(v.hotspots[1].shape.points.len(), 3);
        assert_eq!(v.hotspots[1].shape.page, Side::Right);
    }

    #[test]
    fn hit_tests_as_the_app_does() {
        let l = read(&cartridge()).unwrap();
        let tri = &l.views[0].hotspots[1].shape;
        let at = |x: f32, y: f32| Spot { page: Side::Right, x, y };
        assert!(tri.contains(at(550.0, 150.0)));
        assert!(!tri.contains(at(650.0, 250.0)));
        assert!(!tri.contains(Spot { page: Side::Left, x: 550.0, y: 150.0 }), "the other page");
        // On an edge counts as inside.
        assert!(tri.contains(at(600.0, 100.0)));
        assert!(tri.contains(at(600.0, 200.0)));
        let p = tri.inside_point().unwrap();
        assert!(tri.contains(p));
        // A rectangle includes its edges.
        let rect = &l.views[0].hotspots[0].shape;
        assert!(rect.contains(at(200.0, 300.0)) && rect.contains(at(400.0, 500.0)) && !rect.contains(at(401.0, 400.0)));
    }

    #[test]
    fn unknown_apps_are_refused() {
        let mut d = cartridge();
        d[0x40000] = 1;
        assert!(read(&d).unwrap_err().contains("unknown"));
        assert!(read(&vec![0; 0x20000]).is_err());
    }

    /// The same book under the Classic app's header: views listed by data
    /// pointer at +5 (spread in the view's byte +2), the GO list at +7, and
    /// shapes found at the table plus the entry in bytes.
    #[test]
    fn reads_the_classic_app() {
        let mut d = cartridge();
        let put16 = |d: &mut Vec<u8>, at: usize, v: u16| d[at..at + 2].copy_from_slice(&v.to_be_bytes());
        let h = 0x40000;
        d[h] = 0;
        put16(&mut d, h + 5, 0x20); // the view list,
        d[h + 0x20..h + 0x24].copy_from_slice(&(CART + 0x40400).to_be_bytes());
        put16(&mut d, h + 7, 0x200); // ending at the GO list
        d[0x40402] = 0; // spread 0
        // Byte offsets in the table: move each entry to 4 × itself.
        for i in 0..3 {
            let e = u16::from_be_bytes([d[0x30000 + 2 * i], d[0x30001 + 2 * i]]);
            put16(&mut d, 0x30000 + 2 * i, e * 4);
        }
        // The rectangle answers in mode 2 only.
        d[0x40501] = 2;
        let l = read(&d).expect("reads");
        assert_eq!(l.spreads.len(), 1);
        assert_eq!(l.spreads[0].go.points, vec![(10.0, 30.0), (100.0, 130.0)]);
        let v = l.main_view(&l.spreads[0]).expect("the page's view");
        assert_eq!(v.name, "Page 0");
        assert_eq!(v.hotspots.iter().map(|h| h.modes).collect::<Vec<_>>(), [2, 0xFF]);
        assert_eq!(v.hotspots[1].shape.points.len(), 3);
    }
}
