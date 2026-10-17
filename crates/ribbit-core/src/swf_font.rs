//! Font outlines from Flash movies (DefineFont2 tags), for drawing the
//! BIOS's device text in HD.
//!
//! The BIOS draws Flash text fields that use device fonts ("Leapster Mono",
//! "Leapster Prop") from pixel glyphs in its ROM (hd.rs, `Glyph`). Some
//! cartridges embed the same fonts with their outlines (Learning with Leap
//! has Leapster Mono, Kindergarten Leapster Prop), so HD can draw the text
//! from the typeface's own curves. Nothing here ships font data: the
//! outlines come from the player's own cartridges.

use std::collections::HashMap;

/// One glyph's outline: quadratic pieces (start, control, end) in em units
/// (1024 per em, y down, baseline at 0). Straight pieces have their control
/// point halfway.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    pub pieces: Vec<[(f32, f32); 3]>,
}

/// A font's glyphs by character code.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct OutlineFont {
    pub glyphs: HashMap<u16, Outline>,
}

/// Every font with outlines in the uncompressed Flash movies ("FWS")
/// inside `data` (a cartridge image), by name; a name seen twice has the
/// glyphs of both (see `merge_fonts`).
pub fn find_fonts(data: &[u8]) -> HashMap<String, OutlineFont> {
    let mut fonts: HashMap<String, OutlineFont> = HashMap::new();
    let mut i = 0;
    while let Some(at) = find(data, b"FWS", i) {
        i = at + 3;
        let Some(len) = u32_at(data, at + 4).map(|v| v as usize) else { break };
        let version = data[at + 3];
        if !(3..=10).contains(&version) || len < 30 || at + len > data.len() {
            continue;
        }
        let movie = &data[at..at + len];
        merge_fonts(&mut fonts, movie_fonts(movie));
        i = at + len;
    }
    fonts
}

/// Adds `from`'s fonts to `into`: a font in both gets the glyphs it lacks
/// (a movie embeds only the characters it uses, so one cartridge seldom
/// has a whole font: e.g. Leapster Prop, 23 glyphs in Creature Create, 188
/// in Pet Pals).
pub fn merge_fonts(into: &mut HashMap<String, OutlineFont>, from: impl IntoIterator<Item = (String, OutlineFont)>) {
    for (name, font) in from {
        let have = into.entry(name).or_default();
        for (ch, outline) in font.glyphs {
            let slot = have.glyphs.entry(ch).or_default();
            if slot.pieces.is_empty() {
                *slot = outline;
            }
        }
    }
}

fn find(data: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    data.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|p| p + from)
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(i..i + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(i..i + 4)?.try_into().ok()?))
}

/// The DefineFont2 fonts with glyphs in one movie.
fn movie_fonts(m: &[u8]) -> Vec<(String, OutlineFont)> {
    let mut out = vec![];
    let nbits = (m[8] >> 3) as usize;
    let mut p = 8 + (5 + 4 * nbits).div_ceil(8) + 4;
    while let Some(h) = u16_at(m, p) {
        p += 2;
        let code = h >> 6;
        let mut len = (h & 63) as usize;
        if len == 63 {
            let Some(l) = u32_at(m, p) else { break };
            len = l as usize;
            p += 4;
        }
        if code == 0 || p + len > m.len() {
            break;
        }
        if code == 48
            && let Some(f) = define_font2(&m[p..p + len])
        {
            out.push(f);
        }
        p += len;
    }
    out
}

/// A DefineFont2 tag body: id, flags, language, name, glyph count, offset
/// table, glyph shapes, code table (layout data after it is not needed).
fn define_font2(t: &[u8]) -> Option<(String, OutlineFont)> {
    let flags = *t.get(2)?;
    let name_len = *t.get(4)? as usize;
    let name = String::from_utf8_lossy(t.get(5..5 + name_len)?).trim_end_matches('\0').to_string();
    let base = 5 + name_len;
    let count = u16_at(t, base)? as usize;
    if count == 0 {
        return None;
    }
    let table = base + 2;
    let wide_offsets = flags & 0x08 != 0;
    let offset = |k: usize| if wide_offsets { u32_at(t, table + 4 * k).map(|v| v as usize) } else { u16_at(t, table + 2 * k).map(|v| v as usize) };
    let codes = table + offset(count)?;
    let mut font = OutlineFont::default();
    for k in 0..count {
        let (from, to) = (table + offset(k)?, table + offset(k + 1)?);
        let code = if flags & 0x04 != 0 { u16_at(t, codes + 2 * k)? } else { *t.get(codes + k)? as u16 };
        font.glyphs.insert(code, shape(t.get(from..to)?));
    }
    Some((name, font))
}

/// Bits of a SWF record, most significant first.
struct Bits<'a> {
    d: &'a [u8],
    p: usize,
}

impl Bits<'_> {
    fn u(&mut self, n: usize) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            let byte = self.d.get(self.p / 8).copied().unwrap_or(0);
            v = v << 1 | ((byte >> (7 - self.p % 8)) & 1) as u32;
            self.p += 1;
        }
        v
    }

    fn s(&mut self, n: usize) -> i32 {
        if n == 0 {
            return 0;
        }
        let v = self.u(n) as i32;
        if v >= 1 << (n - 1) { v - (1 << n) } else { v }
    }
}

/// A glyph SHAPE (fill and line bit counts, then shape records).
fn shape(d: &[u8]) -> Outline {
    let mut r = Bits { d, p: 0 };
    let (fill_bits, line_bits) = (r.u(4) as usize, r.u(4) as usize);
    let mut out = Outline::default();
    let (mut x, mut y) = (0i32, 0i32);
    while r.p < d.len() * 8 {
        if r.u(1) == 0 {
            let flags = r.u(5);
            if flags == 0 {
                break;
            }
            if flags & 1 != 0 {
                let n = r.u(5) as usize;
                (x, y) = (r.s(n), r.s(n));
            }
            for (bit, n) in [(2, fill_bits), (4, fill_bits), (8, line_bits)] {
                if flags & bit != 0 {
                    r.u(n);
                }
            }
        } else if r.u(1) == 1 {
            let n = r.u(4) as usize + 2;
            let (dx, dy) = if r.u(1) == 1 {
                (r.s(n), r.s(n))
            } else if r.u(1) == 1 {
                (0, r.s(n))
            } else {
                (r.s(n), 0)
            };
            let (a, b) = ((x as f32, y as f32), ((x + dx) as f32, (y + dy) as f32));
            out.pieces.push([a, ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0), b]);
            (x, y) = (x + dx, y + dy);
        } else {
            let n = r.u(4) as usize + 2;
            let (cx, cy) = (x + r.s(n), y + r.s(n));
            let (ax, ay) = (cx + r.s(n), cy + r.s(n));
            out.pieces.push([(x as f32, y as f32), (cx as f32, cy as f32), (ax as f32, ay as f32)]);
            (x, y) = (ax, ay);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_square_glyph() {
        // SHAPE: 1 fill bit, 0 line bits; move to (0,0) with fill1 = 1;
        // edges +100 x, +100 y, -100 x, -100 y; end.
        let mut bits = String::from("00010000");
        bits += "0"; // style change
        bits += "00101"; // fill style 1, move
        bits += "01000"; // move: 8-bit coordinates
        bits += "00000000";
        bits += "00000000";
        bits += "1"; // fill style 1 = 1
        // A straight edge (9-bit delta), horizontal or vertical.
        let edge = |v: i32, vertical: bool| {
            let mut s = String::from("11");
            s += "0111";
            s += "0";
            s += if vertical { "1" } else { "0" };
            s += &format!("{:09b}", (v & 0x1FF) as u32);
            s
        };
        bits += &edge(100, false);
        bits += &edge(100, true);
        bits += &edge(-100, false);
        bits += &edge(-100, true);
        bits += "000000";
        while bits.len() % 8 != 0 {
            bits.push('0');
        }
        let bytes: Vec<u8> = bits.as_bytes().chunks(8).map(|c| u8::from_str_radix(std::str::from_utf8(c).unwrap(), 2).unwrap()).collect();
        let o = shape(&bytes);
        assert_eq!(o.pieces.len(), 4);
        assert_eq!(o.pieces[0][0], (0.0, 0.0));
        assert_eq!(o.pieces[0][2], (100.0, 0.0));
        assert_eq!(o.pieces[1][2], (100.0, 100.0));
        assert_eq!(o.pieces[3][2], (0.0, 0.0));
    }
}
