//! HD textures: a game's texture upscaled 4x for the HD pixels, by one of
//! two 2x scalers done twice:
//!
//! - Pixel art (few colours): MMPX (Morgan McGuire and Mara Gagiu, "MMPX
//!   Style-Preserving Pixel-Art Magnification", JCGT 2021). It only ever
//!   copies a neighbour, so the upscaled texture is kept as which source
//!   texel each pixel is: the pixel pipeline reads that texel as usual
//!   (palette, transparency colour and all), and nothing is blended.
//! - Painted art (many colours, shaded edges): edge-directed interpolation
//!   after Hyllian's Super-xBR, which blends along edges and never across
//!   them. Kept as colours; transparent texels blend as no colour at all.

/// Upscaled pixels per texel, across and down.
pub const UP: usize = 4;

/// More distinct colours than this make a texture painted art.
const PIXEL_ART_COLOURS: usize = 64;

/// A texture's upscaled form, per pixel: its source texel (v * w + u), or
/// its colour (A B G R, 8 bits each).
pub struct Upscaled {
    w: usize,
    h: usize,
    colours: bool,
    map: Vec<u32>,
    /// Colours whose pixel n * k is at texel k's centre (`xbr`'s).
    centred: bool,
}

/// What an upscaled pixel is.
pub enum Texel {
    /// Source texel (u, v).
    Source(i32, i32),
    /// R G B A, 0..1.
    Colour([f32; 4]),
}

impl Upscaled {
    /// Upscaled pixel (x, y), which is wrapped or clamped like the texture.
    #[inline(always)]
    pub fn at(&self, x: i32, y: i32, clamp_u: bool, clamp_v: bool) -> Texel {
        let wrap = |c: i32, size: usize, clamp: bool| if clamp { c.clamp(0, size as i32 - 1) } else { c.rem_euclid(size as i32) } as usize;
        let (x, y) = (wrap(x, self.w * UP, clamp_u), wrap(y, self.h * UP, clamp_v));
        let v = self.map[y * self.w * UP + x];
        if self.colours {
            Texel::Colour([0, 8, 16, 24].map(|k| (v >> k & 255) as f32 / 255.0))
        } else {
            Texel::Source((v as usize % self.w) as i32, (v as usize / self.w) as i32)
        }
    }

    /// Kept as colours centred like `xbr` makes them (painted art).
    pub fn centred(&self) -> bool {
        self.centred
    }

    /// Made from colours (A B G R per pixel, `UP` times `w` x `h`), kept
    /// as they are (not centred like `xbr`'s).
    pub fn from_colours(w: usize, h: usize, map: Vec<u32>) -> Self {
        Self { w, h, colours: true, map, centred: false }
    }

    /// Upscaled pixel (x, y)'s colour (A B G R), the texture's texels being
    /// `texels`.
    pub fn colour(&self, x: usize, y: usize, texels: &[u32]) -> u32 {
        let v = self.map[y * self.w * UP + x];
        if self.colours { v } else { texels[v as usize] }
    }

    /// Bytes held (for the cache's limit).
    pub fn bytes(&self) -> usize {
        self.map.len() * 4
    }
}

/// Upscales a `w` x `h` texture whose texels are `colour` (A B G R, 8 bits
/// each; texels that are equal must have equal values; transparent ones 0).
pub fn upscale(w: usize, h: usize, colour: &[u32]) -> Upscaled {
    let mut seen = std::collections::HashSet::new();
    let few = colour.iter().all(|&c| {
        seen.insert(c);
        seen.len() <= PIXEL_ART_COLOURS
    });
    if few {
        let once = mmpx(w, h, colour);
        let mid: Vec<u32> = once.iter().map(|&i| colour[i as usize]).collect();
        let twice = mmpx(w * 2, h * 2, &mid);
        return Upscaled { w, h, colours: false, map: twice.iter().map(|&i| once[i as usize]).collect(), centred: false };
    }
    // Premultiplied R G B A.
    let img: Vec<[f32; 4]> = colour
        .iter()
        .map(|&c| {
            let a = (c >> 24) as f32 / 255.0;
            [(c & 255) as f32 / 255.0 * a, (c >> 8 & 255) as f32 / 255.0 * a, (c >> 16 & 255) as f32 / 255.0 * a, a]
        })
        .collect();
    let once = xbr(w, h, &img);
    let twice = xbr(w * 2, h * 2, &once);
    let q = |c: f32| (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
    let map = twice
        .iter()
        .map(|&[r, g, b, a]| match a {
            a if a <= 1.0 / 512.0 => 0,
            a => q(a) << 24 | q(b / a) << 16 | q(g / a) << 8 | q(r / a),
        })
        .collect();
    Upscaled { w, h, colours: true, map, centred: true }
}

/// Edge-directed 2x (after Super-xBR): the input lands on the even pixels;
/// the pixels between four of them (odd, odd) are interpolated, then those
/// between the even and odd ones, from the same 4 x 4 pattern turned 45
/// degrees. Colours are premultiplied R G B A.
fn xbr(w: usize, h: usize, img: &[[f32; 4]]) -> Vec<[f32; 4]> {
    let (ow, oh) = (2 * w as i32, 2 * h as i32);
    let mut out = vec![[0.0f32; 4]; (ow * oh) as usize];
    for y in 0..h {
        for x in 0..w {
            out[2 * y * ow as usize + 2 * x] = img[y * w + x];
        }
    }
    // An output pixel, clamped to the picture at the same parity (a pixel
    // already made).
    let fix = |c: i32, size: i32| c.clamp(c & 1, size - 2 + (c & 1));
    let pass = |out: &mut Vec<[f32; 4]>, targets: &[(i32, i32)], a: (i32, i32), b: (i32, i32)| {
        let new: Vec<[f32; 4]> = targets
            .iter()
            .map(|&(tx, ty)| {
                // E is (0, 0) of the pattern, the target at (0.5, 0.5).
                let (ex, ey) = (tx - (a.0 + b.0) / 2, ty - (a.1 + b.1) / 2);
                let px = |i: i32, j: i32| out[(fix(ey + i * a.1 + j * b.1, oh) * ow + fix(ex + i * a.0 + j * b.0, ow)) as usize];
                interpolate(&px)
            })
            .collect();
        for (&(x, y), c) in targets.iter().zip(new) {
            out[(y * ow + x) as usize] = c;
        }
    };
    let odd: Vec<(i32, i32)> = (0..h as i32).flat_map(|y| (0..w as i32).map(move |x| (2 * x + 1, 2 * y + 1))).collect();
    pass(&mut out, &odd, (2, 0), (0, 2));
    let mixed: Vec<(i32, i32)> = (0..h as i32).flat_map(|y| (0..w as i32).flat_map(move |x| [(2 * x + 1, 2 * y), (2 * x, 2 * y + 1)])).collect();
    pass(&mut out, &mixed, (1, -1), (1, 1));
    out
}

/// Super-xBR's edge strength: a diagonal edge this clear is followed fully.
const EDGE: f32 = 2.0;

/// Super-xBR's colour between pattern pixels E (0, 0), F (1, 0), H (0, 1)
/// and I (1, 1), from the 4 x 4 pattern `px(i, j)` (i across, j down, -1..=2).
fn interpolate(px: &impl Fn(i32, i32) -> [f32; 4]) -> [f32; 4] {
    let [p0, b, c, p1] = [-1, 0, 1, 2].map(|i| px(i, -1));
    let [d, e, f, f4] = [-1, 0, 1, 2].map(|i| px(i, 0));
    let [g, h, i, i4] = [-1, 0, 1, 2].map(|i| px(i, 1));
    let [p2, h5, i5, p3] = [-1, 0, 1, 2].map(|i| px(i, 2));
    // Brightness, with transparency an edge too.
    let y = |c: [f32; 4]| 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2] + 0.5 * c[3];
    let [yb, yc, yd, ye, yf, yg, yh, yi, yi4, yi5, yh5, yf4, yp0, yp1, yp2, yp3] = [b, c, d, e, f, g, h, i, i4, i5, h5, f4, p0, p1, p2, p3].map(y);
    let df = |a: f32, b: f32| (a - b).abs();
    const WP: [f32; 6] = [2.0, 1.0, -1.0, 4.0, -1.0, 1.0];
    let d_wd = |b: [f32; 14]| {
        let [b0, b1, c0, c1, c2, d0, d1, d2, d3, e1, e2, e3, f2, f3] = b;
        WP[0] * (df(c1, c2) + df(c1, c0) + df(e2, e1) + df(e2, e3))
            + WP[1] * (df(d2, d3) + df(d0, d1))
            + WP[2] * (df(d1, d3) + df(d0, d2))
            + WP[3] * df(d1, d2)
            + WP[4] * (df(c0, c2) + df(e1, e3))
            + WP[5] * (df(b0, b1) + df(f2, f3))
    };
    let hv_wd = |[i1, i2, i3, i4, e1, e2, e3, e4]: [f32; 8]| {
        WP[3] * (df(i1, i2) + df(i3, i4)) + WP[0] * (df(i1, e1) + df(i2, e2) + df(i3, e3) + df(i4, e4)) + WP[2] * (df(i1, e2) + df(i3, e4) + df(e1, i2) + df(e3, i4))
    };
    // Edginess across the two diagonals, and across and down.
    let d_edge = d_wd([yd, yb, yg, ye, yc, yp2, yh, yf, yp1, yh5, yi, yf4, yi5, yi4]) - d_wd([yc, yf4, yb, yf, yi4, yp0, ye, yi, yp3, yd, yh, yi5, yg, yh5]);
    let hv_edge = hv_wd([yf, yi, ye, yh, yc, yi5, yb, yh5]) - hv_wd([ye, yf, yh, yi, yd, yf4, yg, yi4]);
    let t = (d_edge.abs() / EDGE).clamp(0.0, 1.0);
    let strength = t * t * (3.0 - 2.0 * t);
    let (w1, w2) = (0.129_633f32, 0.175_068f32 / 2.0);
    let mut out = [0.0; 4];
    for k in 0..4 {
        // Along each diagonal, then across and down (two taps a side).
        let c1 = -w1 * p2[k] + (w1 + 0.5) * h[k] + (w1 + 0.5) * f[k] - w1 * p1[k];
        let c2 = -w1 * p0[k] + (w1 + 0.5) * e[k] + (w1 + 0.5) * i[k] - w1 * p3[k];
        let c3 = -w2 * (d[k] + g[k]) + (w2 + 0.25) * (e[k] + h[k]) + (w2 + 0.25) * (f[k] + i[k]) - w2 * (f4[k] + i4[k]);
        let c4 = -w2 * (c[k] + b[k]) + (w2 + 0.25) * (f[k] + e[k]) + (w2 + 0.25) * (i[k] + h[k]) - w2 * (i5[k] + h5[k]);
        let diag = if d_edge >= 0.0 { c2 } else { c1 };
        let hv = if hv_edge >= 0.0 { c4 } else { c3 };
        let v = diag * strength + hv * (1.0 - strength);
        // No ringing: within the four pixels around.
        let (lo, hi) = (e[k].min(f[k]).min(h[k]).min(i[k]), e[k].max(f[k]).max(h[k]).max(i[k]));
        out[k] = v.clamp(lo, hi);
    }
    // Premultiplied: no colour beyond its alpha.
    for k in 0..3 {
        out[k] = out[k].min(out[3]);
    }
    out
}

/// MMPX's brightness: transparent texels count as bright (background).
fn luma(c: u32) -> u32 {
    ((c >> 16 & 255) + (c >> 8 & 255) + (c & 255) + 1) * (256 - (c >> 24))
}

/// MMPX 2x: per output pixel, the index of the input pixel it copies.
fn mmpx(w: usize, h: usize, img: &[u32]) -> Vec<u32> {
    let mut out = vec![0u32; w * h * 4];
    let (wi, hi) = (w as i32, h as i32);
    for y in 0..hi {
        for x in 0..wi {
            // Neighbours by index, clamped at the edges.
            let at = |dx: i32, dy: i32| ((y + dy).clamp(0, hi - 1) * wi + (x + dx).clamp(0, wi - 1)) as u32;
            let c = |i: u32| img[i as usize];
            let (a, b, cc) = (at(-1, -1), at(0, -1), at(1, -1));
            let (d, e, f) = (at(-1, 0), at(0, 0), at(1, 0));
            let (g, hh, i) = (at(-1, 1), at(0, 1), at(1, 1));
            let (mut j, mut k, mut l, mut m) = (e, e, e, e);
            let [ca, cb, cc_, cd, ce, cf, cg, ch, ci] = [a, b, cc, d, e, f, g, hh, i].map(c);
            if [ca, cb, cc_, cd, cf, cg, ch, ci].iter().any(|&v| v != ce) {
                let (p, s) = (at(0, -2), at(0, 2));
                let (q, r) = (at(-2, 0), at(2, 0));
                let [cp, cs, cq, cr] = [p, s, q, r].map(c);
                let (bl, dl, el, fl, hl) = (luma(cb), luma(cd), luma(ce), luma(cf), luma(ch));
                let eq2 = |v: u32, a0: u32, a1: u32| v == a0 && v == a1;
                let eq3 = |v: u32, a0: u32, a1: u32, a2: u32| v == a0 && v == a1 && v == a2;
                let eq4 = |v: u32, a0: u32, a1: u32, a2: u32, a3: u32| v == a0 && v == a1 && v == a2 && v == a3;
                let any3 = |v: u32, a0: u32, a1: u32, a2: u32| v == a0 || v == a1 || v == a2;
                let none2 = |v: u32, a0: u32, a1: u32| v != a0 && v != a1;
                let none4 = |v: u32, a0: u32, a1: u32, a2: u32, a3: u32| v != a0 && v != a1 && v != a2 && v != a3;

                // 1:1 slopes.
                if cd == cb && cd != ch && cd != cf && (el >= dl || ce == ca) && any3(ce, ca, cc_, cg) && (el < dl || ca != cd || ce != cp || ce != cq) {
                    j = d;
                }
                if cb == cf && cb != cd && cb != ch && (el >= bl || ce == cc_) && any3(ce, ca, cc_, ci) && (el < bl || cc_ != cb || ce != cp || ce != cr) {
                    k = b;
                }
                if ch == cd && ch != cf && ch != cb && (el >= hl || ce == cg) && any3(ce, ca, cg, ci) && (el < hl || cg != ch || ce != cs || ce != cq) {
                    l = hh;
                }
                if cf == ch && cf != cb && cf != cd && (el >= fl || ce == ci) && any3(ce, cc_, cg, ci) && (el < fl || ci != ch || ce != cr || ce != cs) {
                    m = f;
                }

                // Intersections.
                if ce != cf && eq4(ce, cc_, ci, cd, cq) && eq2(cf, cb, ch) && cf != c(at(3, 0)) {
                    (k, m) = (f, f);
                }
                if ce != cd && eq4(ce, ca, cg, cf, cr) && eq2(cd, cb, ch) && cd != c(at(-3, 0)) {
                    (j, l) = (d, d);
                }
                if ce != ch && eq4(ce, cg, ci, cb, cp) && eq2(ch, cd, cf) && ch != c(at(0, 3)) {
                    (l, m) = (hh, hh);
                }
                if ce != cb && eq4(ce, ca, cc_, ch, cs) && eq2(cb, cd, cf) && cb != c(at(0, -3)) {
                    (j, k) = (b, b);
                }

                // Triangle tips.
                if bl < el && eq4(ce, cg, ch, ci, cs) && none4(ce, ca, cd, cc_, cf) {
                    (j, k) = (b, b);
                }
                if hl < el && eq4(ce, ca, cb, cc_, cp) && none4(ce, cd, cg, ci, cf) {
                    (l, m) = (hh, hh);
                }
                if fl < el && eq4(ce, ca, cd, cg, cq) && none4(ce, cb, cc_, ci, ch) {
                    (k, m) = (f, f);
                }
                if dl < el && eq4(ce, cc_, cf, ci, cr) && none4(ce, cb, ca, cg, ch) {
                    (j, l) = (d, d);
                }

                // 2:1 slopes.
                if ch != cb {
                    if ch != ca && ch != ce && ch != cc_ {
                        if eq3(ch, cg, cf, cr) && none2(ch, cd, c(at(2, -1))) {
                            l = m;
                        }
                        if eq3(ch, ci, cd, cq) && none2(ch, cf, c(at(-2, -1))) {
                            m = l;
                        }
                    }
                    if cb != ci && cb != cg && cb != ce {
                        if eq3(cb, ca, cf, cr) && none2(cb, cd, c(at(2, 1))) {
                            j = k;
                        }
                        if eq3(cb, cc_, cd, cq) && none2(cb, cf, c(at(-2, 1))) {
                            k = j;
                        }
                    }
                }
                if cf != cd {
                    if cd != ci && cd != ce && cd != cc_ {
                        if eq3(cd, ca, ch, cs) && none2(cd, cb, c(at(1, 2))) {
                            j = l;
                        }
                        if eq3(cd, cg, cb, cp) && none2(cd, ch, c(at(1, -2))) {
                            l = j;
                        }
                    }
                    if cf != ce && cf != ca && cf != cg {
                        if eq3(cf, cc_, ch, cs) && none2(cf, cb, c(at(-1, 2))) {
                            k = m;
                        }
                        if eq3(cf, ci, cb, cp) && none2(cf, ch, c(at(-1, -2))) {
                            m = k;
                        }
                    }
                }
            }
            let (ox, oy, ow) = (2 * x as usize, 2 * y as usize, 2 * w);
            out[oy * ow + ox] = j;
            out[oy * ow + ox + 1] = k;
            out[(oy + 1) * ow + ox] = l;
            out[(oy + 1) * ow + ox + 1] = m;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_stays_flat() {
        let up = upscale(3, 2, &[7; 6]);
        assert_eq!(up.map.len(), 3 * 2 * UP * UP);
        // Each upscaled pixel comes from the texel it lies in.
        for y in 0..2 * UP as i32 {
            for x in 0..3 * UP as i32 {
                assert!(matches!(up.at(x, y, false, false), Texel::Source(u, v) if (u, v) == (x / UP as i32, y / UP as i32)));
            }
        }
    }

    #[test]
    fn diagonal_is_smoothed() {
        // A staircase: the 1:1 slope rule fills the corner pixels.
        let (o, x) = (0xFF00_0000u32, 0xFFFF_FFFFu32);
        let img = [x, o, o, o, x, x, o, o, x, x, x, o, x, x, x, x];
        let once = mmpx(4, 4, &img);
        assert_ne!(once.iter().filter(|&&i| img[i as usize] == x).count(), 10 * 4);
    }

    #[test]
    fn xbr_follows_a_diagonal_line() {
        // A white line down the main diagonal, on black: the pixel between
        // two of its texels stays on it, the one beside it off.
        let n = 8;
        let img: Vec<[f32; 4]> = (0..n * n).map(|k| if k % n == k / n { [1.0; 4] } else { [0.0, 0.0, 0.0, 1.0] }).collect();
        let out = xbr(n, n, &img);
        let at = |x: usize, y: usize| out[y * 2 * n + x];
        assert!(at(7, 7)[0] > 0.9, "on the line: {:?}", at(7, 7));
        assert!(at(9, 7)[0] < 0.1, "beside it: {:?}", at(9, 7));
    }
}
