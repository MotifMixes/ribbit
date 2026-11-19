//! The fragment stage: clipping, triangle setup and the per-pixel tests,
//! texturing and blending into a [`Surface`].

use crate::{Raster, Texture, Vertex, gl};

/// A colour buffer (RGBA8, R in the low byte) and a depth buffer, rows
/// bottom-up as GL counts them.
#[derive(Clone)]
pub struct Surface {
    pub width: u32,
    pub height: u32,
    pub color: Vec<u32>,
    pub depth: Vec<f32>,
}

impl Surface {
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width * height) as usize;
        Self { width, height, color: vec![0xFF00_0000; n], depth: vec![1.; n] }
    }

    pub fn pixel(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return 0;
        }
        self.color[(y as u32 * self.width + x as u32) as usize]
    }

    /// The picture top-down as 0x00RRGGBB (Ribbit's frame format).
    pub fn to_xrgb(&self) -> Vec<u32> {
        let mut out = Vec::with_capacity(self.color.len());
        for y in (0..self.height).rev() {
            let row = &self.color[(y * self.width) as usize..((y + 1) * self.width) as usize];
            out.extend(row.iter().map(|&c| (c & 0xFF) << 16 | (c & 0xFF00) | (c >> 16) & 0xFF));
        }
        out
    }

    fn rect(&self, scissor: Option<[i32; 4]>) -> (i32, i32, i32, i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (0, 0, self.width as i32, self.height as i32);
        if let Some([x, y, w, h]) = scissor {
            x0 = x0.max(x);
            y0 = y0.max(y);
            x1 = x1.min(x + w);
            y1 = y1.min(y + h);
        }
        (x0, y0, x1, y1)
    }

    pub fn clear_color(&mut self, c: u32, mask: [bool; 4], scissor: Option<[i32; 4]>) {
        let keep: u32 = (0..4).filter(|&i| !mask[i]).map(|i| 0xFFu32 << (8 * i)).sum();
        let (x0, y0, x1, y1) = self.rect(scissor);
        for y in y0..y1 {
            for x in x0..x1 {
                let p = &mut self.color[(y as u32 * self.width + x as u32) as usize];
                *p = (*p & keep) | (c & !keep);
            }
        }
    }

    pub fn clear_depth(&mut self, d: f32, scissor: Option<[i32; 4]>) {
        let (x0, y0, x1, y1) = self.rect(scissor);
        for y in y0..y1 {
            for x in x0..x1 {
                self.depth[(y as u32 * self.width + x as u32) as usize] = d;
            }
        }
    }
}

/// Rows `y0..y1` of a surface (`color` and `depth` hold only those rows): what one thread
/// draws.
pub struct Band<'a> {
    pub width: u32,
    pub height: u32,
    pub y0: i32,
    pub y1: i32,
    pub color: &'a mut [u32],
    pub depth: &'a mut [f32],
}

impl Band<'_> {
    /// The pixels to draw: the band's rows, inside the scissor box.
    fn rect(&self, scissor: Option<[i32; 4]>) -> (i32, i32, i32, i32) {
        let (mut x0, mut y0, mut x1, mut y1) = (0, self.y0, self.width as i32, self.y1);
        if let Some([x, y, w, h]) = scissor {
            x0 = x0.max(x);
            y0 = y0.max(y);
            x1 = x1.min(x + w);
            y1 = y1.min(y + h);
        }
        (x0, y0, x1, y1)
    }
}

impl Surface {
    /// All of it as one band.
    pub fn band(&mut self) -> Band<'_> {
        Band { width: self.width, height: self.height, y0: 0, y1: self.height as i32, color: &mut self.color, depth: &mut self.depth }
    }
}

#[inline]
fn wrap(v: i32, n: i32, mode: u32) -> i32 {
    if mode != gl::REPEAT {
        v.clamp(0, n - 1)
    } else if n & (n - 1) == 0 {
        // A power of two (most textures): the same as rem_euclid without a division.
        v & (n - 1)
    } else {
        v.rem_euclid(n)
    }
}

/// Byte / 255 for every byte (the same values as dividing, without the division).
static UNIT: [f32; 256] = {
    let mut t = [0f32; 256];
    let mut i = 0;
    while i < 256 {
        t[i] = i as f32 / 255.;
        i += 1;
    }
    t
};

#[inline]
fn unpack(c: u32) -> [f32; 4] {
    [UNIT[(c & 0xFF) as usize], UNIT[(c >> 8 & 0xFF) as usize], UNIT[(c >> 16 & 0xFF) as usize], UNIT[(c >> 24) as usize]]
}

/// The texel colour at (s, t); `minify` picks the filter.
fn sample(t: &Texture, s: f32, tt: f32, minify: bool) -> [f32; 4] {
    unpack(sample_packed(t, s, tt, minify))
}

/// [`sample`] as RGBA8. Bilinear weights are 8-bit fractions (as GPUs filter), two channels
/// at a time.
#[inline]
fn sample_packed(t: &Texture, s: f32, tt: f32, minify: bool) -> u32 {
    let (w, h) = (t.width as i32, t.height as i32);
    let filter = if minify { t.min_filter } else { t.mag_filter };
    let linear = matches!(filter, gl::LINEAR | gl::LINEAR_MIPMAP_NEAREST | gl::LINEAR_MIPMAP_LINEAR);
    let at = |x: i32, y: i32| t.pixels[(wrap(y, h, t.wrap_t) * w + wrap(x, w, t.wrap_s)) as usize];
    if !linear {
        let x = (s * w as f32).floor() as i32;
        let y = (tt * h as f32).floor() as i32;
        return at(x, y);
    }
    let u = s * w as f32 - 0.5;
    let v = tt * h as f32 - 0.5;
    let (x0, y0) = (u.floor(), v.floor());
    let fx = ((u - x0) * 256.) as u32;
    let fy = ((v - y0) * 256.) as u32;
    let (x0, y0) = (x0 as i32, y0 as i32);
    let (a, b, c, d) = (at(x0, y0), at(x0 + 1, y0), at(x0, y0 + 1), at(x0 + 1, y0 + 1));
    // Weights out of 256: a channel times a weight fits in 16 bits.
    let mix = |p: u32, q: u32, f: u32| {
        let lo = ((p & 0x00FF_00FF) * (256 - f) + (q & 0x00FF_00FF) * f) >> 8 & 0x00FF_00FF;
        let hi = ((p >> 8 & 0x00FF_00FF) * (256 - f) + (q >> 8 & 0x00FF_00FF) * f) & 0xFF00_FF00;
        lo | hi
    };
    mix(mix(a, b, fx), mix(c, d, fx), fy)
}

/// The texture environment (OpenGL ES 1.1 table 3.15/3.16) on the incoming colour `f`.
fn tex_env(r: &Raster, format: u32, f: [f32; 4], t: [f32; 4]) -> [f32; 4] {
    let (rgb, alpha) = match format {
        gl::ALPHA => (false, true),
        gl::LUMINANCE | gl::RGB => (true, false),
        _ => (true, true),
    };
    let mut c = f;
    match r.tex_env {
        gl::REPLACE => {
            if rgb {
                c[..3].copy_from_slice(&t[..3]);
            }
            if alpha {
                c[3] = t[3];
            }
        }
        gl::DECAL => {
            if rgb {
                let a = if alpha { t[3] } else { 1. };
                for i in 0..3 {
                    c[i] = f[i] * (1. - a) + t[i] * a;
                }
            }
        }
        gl::BLEND => {
            if rgb {
                for i in 0..3 {
                    c[i] = f[i] * (1. - t[i]) + r.tex_env_color[i] * t[i];
                }
            }
            if alpha {
                c[3] = f[3] * t[3];
            }
        }
        gl::ADD => {
            if rgb {
                for i in 0..3 {
                    c[i] = (f[i] + t[i]).min(1.);
                }
            }
            if alpha {
                c[3] = f[3] * t[3];
            }
        }
        _ => {
            // MODULATE (and COMBINE's default, which is MODULATE).
            if rgb {
                for i in 0..3 {
                    c[i] = f[i] * t[i];
                }
            }
            if alpha {
                c[3] = f[3] * t[3];
            }
        }
    }
    c
}

fn compare(func: u32, a: f32, b: f32) -> bool {
    match func {
        gl::NEVER => false,
        gl::LESS => a < b,
        gl::EQUAL => a == b,
        gl::LEQUAL => a <= b,
        gl::GREATER => a > b,
        gl::NOTEQUAL => a != b,
        gl::GEQUAL => a >= b,
        _ => true,
    }
}

fn factor(f: u32, src: [f32; 4], dst: [f32; 4]) -> [f32; 4] {
    match f {
        gl::ZERO => [0.; 4],
        gl::ONE => [1.; 4],
        gl::SRC_COLOR => src,
        gl::ONE_MINUS_SRC_COLOR => src.map(|v| 1. - v),
        gl::SRC_ALPHA => [src[3]; 4],
        gl::ONE_MINUS_SRC_ALPHA => [1. - src[3]; 4],
        gl::DST_ALPHA => [dst[3]; 4],
        gl::ONE_MINUS_DST_ALPHA => [1. - dst[3]; 4],
        gl::DST_COLOR => dst,
        gl::ONE_MINUS_DST_COLOR => dst.map(|v| 1. - v),
        gl::SRC_ALPHA_SATURATE => {
            let a = src[3].min(1. - dst[3]);
            [a, a, a, 1.]
        }
        _ => [1.; 4],
    }
}

/// Writes one fragment: alpha, depth tests, blending, masks.
#[inline]
fn fragment(s: &mut Surface, r: &Raster, x: i32, y: i32, z: f32, color: [f32; 4]) {
    let i = (y as u32 * s.width + x as u32) as usize;
    fragment_at(&mut s.color[i], &mut s.depth[i], r, z, color);
}

/// [`fragment`] on one pixel's colour and depth.
#[inline]
fn fragment_at(c: &mut u32, d: &mut f32, r: &Raster, z: f32, color: [f32; 4]) {
    if let Some((func, reference)) = r.alpha_test
        && !compare(func, color[3], reference)
    {
        return;
    }
    if let Some(func) = r.depth_test {
        if !compare(func, z, *d) {
            return;
        }
        if r.depth_mask {
            *d = z;
        }
    }
    let out = match r.blend {
        Some((sf, df)) => {
            let dst = unpack(*c);
            let (a, b) = (factor(sf, color, dst), factor(df, color, dst));
            let mut o = [0f32; 4];
            for k in 0..4 {
                o[k] = (color[k] * a[k] + dst[k] * b[k]).clamp(0., 1.);
            }
            o
        }
        None => color,
    };
    let new = crate::rgba8(out);
    if r.color_mask == [true; 4] {
        *c = new;
    } else {
        let keep: u32 = (0..4).filter(|&k| !r.color_mask[k]).map(|k| 0xFFu32 << (8 * k)).sum();
        *c = (*c & keep) | (new & !keep);
    }
}

/// Clips a polygon against the view volume (-w <= x, y, z <= w) in clip space.
fn clip(poly: Vec<Vertex>) -> Vec<Vertex> {
    let mut poly = poly;
    for plane in 0..6 {
        let dist = |v: &Vertex| {
            let (c, w) = (v.clip[plane / 2], v.clip[3]);
            if plane % 2 == 0 { w + c } else { w - c }
        };
        let mut out = Vec::with_capacity(poly.len() + 2);
        for i in 0..poly.len() {
            let (a, b) = (&poly[i], &poly[(i + 1) % poly.len()]);
            let (da, db) = (dist(a), dist(b));
            if da >= 0. {
                out.push(*a);
            }
            if (da >= 0.) != (db >= 0.) {
                let t = da / (da - db);
                out.push(lerp(a, b, t));
            }
        }
        poly = out;
        if poly.len() < 3 {
            return poly;
        }
    }
    poly
}

fn lerp(a: &Vertex, b: &Vertex, t: f32) -> Vertex {
    let l4 = |x: [f32; 4], y: [f32; 4]| [0, 1, 2, 3].map(|i| x[i] + (y[i] - x[i]) * t);
    Vertex { clip: l4(a.clip, b.clip), color: l4(a.color, b.color), tex: l4(a.tex, b.tex), point_size: a.point_size + (b.point_size - a.point_size) * t }
}

/// A vertex in window coordinates: x, y, z, 1/w.
fn window(r: &Raster, v: &Vertex) -> [f32; 4] {
    let w = if v.clip[3] == 0. { 1e-20 } else { v.clip[3] };
    let inv = 1. / w;
    let [vx, vy, vw, vh] = r.viewport;
    let nx = v.clip[0] * inv;
    let ny = v.clip[1] * inv;
    let nz = v.clip[2] * inv;
    let [n, f] = r.depth_range;
    [vx as f32 + (nx + 1.) * vw as f32 / 2., vy as f32 + (ny + 1.) * vh as f32 / 2., n + (nz + 1.) * (f - n) / 2., inv]
}

pub fn triangle(s: &mut Band, r: &Raster, tex: Option<&Texture>, v: [&Vertex; 3]) {
    let needs_clip = v.iter().any(|p| (0..3).any(|k| p.clip[k].abs() > p.clip[3]) || p.clip[3] <= 0.);
    let poly = if needs_clip { clip(vec![*v[0], *v[1], *v[2]]) } else { vec![*v[0], *v[1], *v[2]] };
    if poly.len() < 3 {
        return;
    }
    let win: Vec<[f32; 4]> = poly.iter().map(|p| window(r, p)).collect();
    // Facing from the first (unclipped) polygon's area.
    let area = |w: &[[f32; 4]]| {
        let mut a = 0.;
        for i in 0..w.len() {
            let (p, q) = (w[i], w[(i + 1) % w.len()]);
            a += p[0] * q[1] - q[0] * p[1];
        }
        a
    };
    let total = area(&win);
    if let Some((mode, front)) = r.cull {
        let ccw = total > 0.;
        let is_front = if front == gl::CCW { ccw } else { !ccw };
        let culled = match mode {
            gl::FRONT => is_front,
            gl::BACK => !is_front,
            _ => true,
        };
        if culled {
            return;
        }
    }
    let flat_color = r.flat.then_some(v[2].color);
    for i in 1..poly.len() - 1 {
        fill(s, r, tex, [&poly[0], &poly[i], &poly[i + 1]], [win[0], win[i], win[i + 1]], flat_color);
    }
}

fn fill(s: &mut Band, r: &Raster, tex: Option<&Texture>, v: [&Vertex; 3], w: [[f32; 4]; 3], flat: Option<[f32; 4]>) {
    let area = (w[1][0] - w[0][0]) * (w[2][1] - w[0][1]) - (w[2][0] - w[0][0]) * (w[1][1] - w[0][1]);
    if area == 0. || !area.is_finite() {
        return;
    }
    let (x0, y0, x1, y1) = s.rect(r.scissor);
    let minx = w.iter().map(|p| p[0]).fold(f32::MAX, f32::min).floor().max(x0 as f32) as i32;
    let maxx = (w.iter().map(|p| p[0]).fold(f32::MIN, f32::max).ceil() as i32).min(x1);
    let miny = w.iter().map(|p| p[1]).fold(f32::MAX, f32::min).floor().max(y0 as f32) as i32;
    let maxy = (w.iter().map(|p| p[1]).fold(f32::MIN, f32::max).ceil() as i32).min(y1);
    if minx >= maxx || miny >= maxy {
        return;
    }
    // Texture minification: texels per pixel along the triangle's first edge, roughly.
    let minify = tex.is_some_and(|t| {
        let du = (v[1].tex[0] - v[0].tex[0]) * t.width as f32;
        let dv = (v[1].tex[1] - v[0].tex[1]) * t.height as f32;
        let dx = w[1][0] - w[0][0];
        let dy = w[1][1] - w[0][1];
        let texels = (du * du + dv * dv).sqrt();
        let pixels = (dx * dx + dy * dy).sqrt();
        pixels > 0. && texels > pixels * 1.01
    });
    let offset = r.polygon_offset.map_or(0., |(_, units)| units / 65536.);
    let inv_area = 1. / area;
    // Top-left fill rule: an edge's pixels on it belong to it when it is a
    // top or left edge.
    let edge = |a: [f32; 4], b: [f32; 4], px: f32, py: f32| (b[0] - a[0]) * (py - a[1]) - (b[1] - a[1]) * (px - a[0]);
    let sign = if area > 0. { 1. } else { -1. };
    let top_left = |a: [f32; 4], b: [f32; 4]| {
        let (dx, dy) = ((b[0] - a[0]) * sign, (b[1] - a[1]) * sign);
        dy < 0. || (dy == 0. && dx > 0.)
    };
    let owns = [top_left(w[1], w[2]), top_left(w[2], w[0]), top_left(w[0], w[1])];
    // Each edge is C - D * px along a row: where it changes sign bounds the row's span. The
    // span is widened by 2 pixels and every pixel in it still takes the exact test below, so
    // only pixels that can't be inside are skipped (near-flat edges, |D| < 1, bound nothing).
    let edges = [(w[1], w[2]), (w[2], w[0]), (w[0], w[1])];
    let span = |py: f32| {
        let (mut lo, mut hi) = (minx, maxx);
        for (a, b) in edges {
            let d = (b[1] - a[1]) * sign;
            if d.abs() < 1. {
                continue;
            }
            let c = ((b[0] - a[0]) * (py - a[1]) + (b[1] - a[1]) * a[0]) * sign;
            let at = c / d - 0.5;
            if d > 0. {
                hi = hi.min((at.floor() as i32).saturating_add(3));
            } else {
                lo = lo.max((at.ceil() as i32).saturating_sub(2));
            }
        }
        (lo, hi)
    };
    // Values across the triangle as planes, f(px, py) = f0 + fx (px - x0) + fy (py - y0) from
    // vertex 0, evaluated at each pixel (no running sums). Perspective: colours and texture
    // coordinates go as value/w with 1/w, one reciprocal per pixel.
    let dbdx = [-(w[2][1] - w[1][1]) * inv_area, -(w[0][1] - w[2][1]) * inv_area, -(w[1][1] - w[0][1]) * inv_area];
    let dbdy = [(w[2][0] - w[1][0]) * inv_area, (w[0][0] - w[2][0]) * inv_area, (w[1][0] - w[0][0]) * inv_area];
    let plane = |f: [f32; 3]| [f[0], f[0] * dbdx[0] + f[1] * dbdx[1] + f[2] * dbdx[2], f[0] * dbdy[0] + f[1] * dbdy[1] + f[2] * dbdy[2]];
    let iw = [w[0][3], w[1][3], w[2][3]];
    let z_plane = plane([w[0][2], w[1][2], w[2][2]]);
    let iw_plane = plane(iw);
    let same = |a: [f32; 4], b: [f32; 4], c: [f32; 4]| a == b && a == c;
    // A colour that doesn't change over the triangle needs no planes.
    let fixed_color = flat.or_else(|| same(v[0].color, v[1].color, v[2].color).then_some(v[0].color));
    let color_planes = [0, 1, 2, 3].map(|i| plane([v[0].color[i] * iw[0], v[1].color[i] * iw[1], v[2].color[i] * iw[2]]));
    let tex_planes = [0, 1, 3].map(|i| plane([v[0].tex[i] * iw[0], v[1].tex[i] * iw[1], v[2].tex[i] * iw[2]]));
    let (ox, oy) = (w[0][0], w[0][1]);
    // An RGBA texture REPLACEs the colour and nothing tests or blends it: the texel is the pixel.
    let direct = tex.is_some_and(|t| t.format == gl::RGBA)
        && r.tex_env == gl::REPLACE
        && r.blend.is_none()
        && r.alpha_test.is_none()
        && r.depth_test.is_none()
        && r.color_mask == [true; 4];
    // The usual sprite: an RGBA texture times the colour, the "over" blend, at most an alpha
    // test; done in 8-bit integers (within a step or two of the float path).
    let packed = tex.is_some_and(|t| t.format == gl::RGBA)
        && matches!(r.tex_env, gl::REPLACE | gl::MODULATE)
        && matches!(r.blend, None | Some((gl::SRC_ALPHA | gl::ONE, gl::ONE_MINUS_SRC_ALPHA)))
        && r.depth_test.is_none()
        && r.color_mask == [true; 4];
    let row = |y: i32, color_row: &mut [u32], depth_row: &mut [f32]| {
        let py = y as f32 + 0.5;
        let (lo, hi) = span(py);
        let dy = py - oy;
        let at_row = |f: [f32; 3]| f[0] + f[2] * dy;
        let z_row = at_row(z_plane);
        let iw_row = at_row(iw_plane);
        let color_row_v = color_planes.map(at_row);
        let tex_row = tex_planes.map(at_row);
        for x in lo..hi {
            let px = x as f32 + 0.5;
            let e = [edge(w[1], w[2], px, py) * sign, edge(w[2], w[0], px, py) * sign, edge(w[0], w[1], px, py) * sign];
            if (0..3).any(|k| e[k] < 0. || (e[k] == 0. && !owns[k])) {
                continue;
            }
            let dx = px - ox;
            let z = z_row + z_plane[1] * dx + offset;
            let mut color = match fixed_color {
                Some(c) => c,
                None => {
                    let inv = iw_row + iw_plane[1] * dx;
                    let rcp = if inv != 0. { 1. / inv } else { 1. };
                    [0, 1, 2, 3].map(|i| (color_row_v[i] + color_planes[i][1] * dx) * rcp)
                }
            };
            if let Some(t) = tex {
                let q = tex_row[2] + tex_planes[2][1] * dx;
                let rq = if q != 0. { 1. / q } else { 1. };
                let ts = (tex_row[0] + tex_planes[0][1] * dx) * rq;
                let tt = (tex_row[1] + tex_planes[1][1] * dx) * rq;
                if direct {
                    color_row[x as usize] = sample_packed(t, ts, tt, minify);
                    continue;
                }
                if packed {
                    let texel = sample_packed(t, ts, tt, minify);
                    let px = if r.tex_env == gl::MODULATE { modulate(texel, crate::rgba8(color)) } else { texel };
                    if let Some((func, reference)) = r.alpha_test
                        && !compare(func, UNIT[(px >> 24) as usize], reference)
                    {
                        continue;
                    }
                    let dst = &mut color_row[x as usize];
                    *dst = match r.blend {
                        None => px,
                        Some((sf, _)) => over(px, *dst, sf == gl::ONE),
                    };
                    continue;
                }
                color = tex_env(r, t.format, color, sample(t, ts, tt, minify));
            }
            fragment_at(&mut color_row[x as usize], &mut depth_row[x as usize], r, z.clamp(0., 1.), color);
        }
    };
    let width = s.width as usize;
    for y in miny..maxy {
        let i = (y - s.y0) as usize * width;
        row(y, &mut s.color[i..i + width], &mut s.depth[i..i + width]);
    }
}

/// Draws triangles (indices into `verts`) in order. When their boxes cover many pixels, the
/// rows they cover are split into bands, one thread each drawing every triangle in its rows:
/// each pixel still sees the triangles in order, so the picture is the same.
pub fn triangles(s: &mut Surface, r: &Raster, tex: Option<&Texture>, verts: &[Vertex], tris: &[[usize; 3]]) {
    let (sw, sh) = (s.width as f32, s.height as f32);
    let (mut lo, mut hi, mut pixels) = (sh, 0f32, 0f32);
    for t in tris {
        // A vertex behind the eye: the triangle may cover anything once clipped.
        let mut b = [0., sw, 0., sh];
        if t.iter().all(|&i| verts[i].clip[3] > 0.) {
            b = [f32::MAX, f32::MIN, f32::MAX, f32::MIN];
            for &i in t {
                let p = window(r, &verts[i]);
                b = [b[0].min(p[0]), b[1].max(p[0]), b[2].min(p[1]), b[3].max(p[1])];
            }
            b = [b[0].max(0.), b[1].min(sw), b[2].max(0.), b[3].min(sh)];
        }
        if b[1] > b[0] && b[3] > b[2] {
            pixels += (b[1] - b[0]) * (b[3] - b[2]);
            lo = lo.min(b[2]);
            hi = hi.max(b[3]);
        }
    }
    let (lo, hi) = ((lo.floor() as i32).max(0), (hi.ceil() as i32).min(s.height as i32));
    let threads = if pixels >= PARALLEL_PIXELS as f32 && hi - lo >= 16 { bands() } else { 1 };
    if threads <= 1 {
        let mut b = s.band();
        for t in tris {
            triangle(&mut b, r, tex, [&verts[t[0]], &verts[t[1]], &verts[t[2]]]);
        }
        return;
    }
    let (width, height) = (s.width, s.height);
    let per = ((hi - lo) as usize).div_ceil(threads);
    let w = width as usize;
    let span = lo as usize * w..hi as usize * w;
    let (color, depth) = (&mut s.color[span.clone()], &mut s.depth[span]);
    std::thread::scope(|sc| {
        for (k, (color, depth)) in color.chunks_mut(per * w).zip(depth.chunks_mut(per * w)).enumerate() {
            let y0 = lo + (k * per) as i32;
            let y1 = y0 + (color.len() / w) as i32;
            sc.spawn(move || {
                let mut b = Band { width, height, y0, y1, color, depth };
                for t in tris {
                    triangle(&mut b, r, tex, [&verts[t[0]], &verts[t[1]], &verts[t[2]]]);
                }
            });
        }
    });
}

/// v / 255, rounded, for v up to 255 * 255.
#[inline]
fn div255(v: u32) -> u32 {
    (v + 128 + ((v + 128) >> 8)) >> 8
}

/// Two RGBA8 colours multiplied channel by channel (MODULATE).
#[inline]
fn modulate(a: u32, b: u32) -> u32 {
    (0..4).fold(0, |o, k| o | div255((a >> (8 * k) & 0xFF) * (b >> (8 * k) & 0xFF)) << (8 * k))
}

/// `src` over `dst`: src * alpha (or src * 1 when `premultiplied`) + dst * (1 - alpha), every
/// channel alpha included.
#[inline]
fn over(src: u32, dst: u32, premultiplied: bool) -> u32 {
    let a = src >> 24;
    (0..4).fold(0, |o, k| {
        let (s, d) = (src >> (8 * k) & 0xFF, dst >> (8 * k) & 0xFF);
        let v = if premultiplied { (s + div255(d * (255 - a))).min(255) } else { div255(s * a + d * (255 - a)) };
        o | v << (8 * k)
    })
}

/// Draws whose triangles' boxes cover at least this many pixels are drawn on several threads.
const PARALLEL_PIXELS: usize = 32 * 1024;

/// Threads for a big draw: the host's, at most 16 (GLES_BANDS sets it).
fn bands() -> usize {
    static N: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *N.get_or_init(|| std::env::var("GLES_BANDS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get()).min(16)))
}

pub fn point(s: &mut Surface, r: &Raster, tex: Option<&Texture>, v: &Vertex) {
    if v.clip[3] <= 0. || (0..3).any(|k| v.clip[k].abs() > v.clip[3]) {
        return;
    }
    let w = window(r, v);
    let size = v.point_size.max(1.);
    let (x0, y0, x1, y1) = s.rect(r.scissor);
    let (cx, cy) = (w[0], w[1]);
    let half = size / 2.;
    for y in ((cy - half).floor() as i32).max(y0)..((cy + half).ceil() as i32).min(y1) {
        for x in ((cx - half).floor() as i32).max(x0)..((cx + half).ceil() as i32).min(x1) {
            let mut color = v.color;
            if let Some(t) = tex {
                color = tex_env(r, t.format, color, sample(t, v.tex[0], v.tex[1], false));
            }
            fragment(s, r, x, y, w[2], color);
        }
    }
}

pub fn line(s: &mut Surface, r: &Raster, tex: Option<&Texture>, a: &Vertex, b: &Vertex, width: f32) {
    let poly = clip(vec![*a, *b, *b]);
    if poly.len() < 2 {
        return;
    }
    let (a, b) = (poly[0], poly[poly.len() - 1]);
    let (wa, wb) = (window(r, &a), window(r, &b));
    let steps = (wb[0] - wa[0]).abs().max((wb[1] - wa[1]).abs()).ceil().max(1.) as i32;
    let (x0, y0, x1, y1) = s.rect(r.scissor);
    let half = (width.max(1.) / 2.).floor() as i32;
    let xmajor = (wb[0] - wa[0]).abs() >= (wb[1] - wa[1]).abs();
    for i in 0..steps {
        let t = (i as f32 + 0.5) / steps as f32;
        let v = lerp(&a, &b, t);
        let (px, py) = (wa[0] + (wb[0] - wa[0]) * t, wa[1] + (wb[1] - wa[1]) * t);
        let z = wa[2] + (wb[2] - wa[2]) * t;
        for k in -half..=half.max(0) {
            let (x, y) = if xmajor { (px.floor() as i32, py.floor() as i32 + k) } else { (px.floor() as i32 + k, py.floor() as i32) };
            if x < x0 || y < y0 || x >= x1 || y >= y1 {
                continue;
            }
            let mut color = if r.flat { b.color } else { v.color };
            if let Some(tx) = tex {
                color = tex_env(r, tx.format, color, sample(tx, v.tex[0], v.tex[1], false));
            }
            fragment(s, r, x, y, z, color);
        }
    }
}
