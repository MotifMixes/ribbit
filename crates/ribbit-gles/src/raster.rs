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

fn wrap(v: i32, n: i32, mode: u32) -> i32 {
    if mode == gl::REPEAT { v.rem_euclid(n) } else { v.clamp(0, n - 1) }
}

fn unpack(c: u32) -> [f32; 4] {
    [(c & 0xFF) as f32 / 255., (c >> 8 & 0xFF) as f32 / 255., (c >> 16 & 0xFF) as f32 / 255., (c >> 24) as f32 / 255.]
}

/// The texel colour at (s, t); `minify` picks the filter.
fn sample(t: &Texture, s: f32, tt: f32, minify: bool) -> [f32; 4] {
    let (w, h) = (t.width as i32, t.height as i32);
    let filter = if minify { t.min_filter } else { t.mag_filter };
    let linear = matches!(filter, gl::LINEAR | gl::LINEAR_MIPMAP_NEAREST | gl::LINEAR_MIPMAP_LINEAR);
    let at = |x: i32, y: i32| t.pixels[(wrap(y, h, t.wrap_t) * w + wrap(x, w, t.wrap_s)) as usize];
    if !linear {
        let x = (s * w as f32).floor() as i32;
        let y = (tt * h as f32).floor() as i32;
        return unpack(at(x, y));
    }
    let u = s * w as f32 - 0.5;
    let v = tt * h as f32 - 0.5;
    let (x0, y0) = (u.floor() as i32, v.floor() as i32);
    let (fx, fy) = (u - x0 as f32, v - y0 as f32);
    let (a, b, c, d) = (unpack(at(x0, y0)), unpack(at(x0 + 1, y0)), unpack(at(x0, y0 + 1)), unpack(at(x0 + 1, y0 + 1)));
    let mut out = [0f32; 4];
    for i in 0..4 {
        out[i] = (a[i] * (1. - fx) + b[i] * fx) * (1. - fy) + (c[i] * (1. - fx) + d[i] * fx) * fy;
    }
    out
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
    if let Some((func, reference)) = r.alpha_test
        && !compare(func, color[3], reference)
    {
        return;
    }
    if let Some(func) = r.depth_test {
        if !compare(func, z, s.depth[i]) {
            return;
        }
        if r.depth_mask {
            s.depth[i] = z;
        }
    }
    let out = match r.blend {
        Some((sf, df)) => {
            let dst = unpack(s.color[i]);
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
        s.color[i] = new;
    } else {
        let keep: u32 = (0..4).filter(|&k| !r.color_mask[k]).map(|k| 0xFFu32 << (8 * k)).sum();
        s.color[i] = (s.color[i] & keep) | (new & !keep);
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

pub fn triangle(s: &mut Surface, r: &Raster, tex: Option<&Texture>, v: [&Vertex; 3]) {
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

fn fill(s: &mut Surface, r: &Raster, tex: Option<&Texture>, v: [&Vertex; 3], w: [[f32; 4]; 3], flat: Option<[f32; 4]>) {
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
    for y in miny..maxy {
        let py = y as f32 + 0.5;
        for x in minx..maxx {
            let px = x as f32 + 0.5;
            let e = [edge(w[1], w[2], px, py) * sign, edge(w[2], w[0], px, py) * sign, edge(w[0], w[1], px, py) * sign];
            if (0..3).any(|k| e[k] < 0. || (e[k] == 0. && !owns[k])) {
                continue;
            }
            let b = [e[0] * inv_area * sign, e[1] * inv_area * sign, e[2] * inv_area * sign];
            let z = b[0] * w[0][2] + b[1] * w[1][2] + b[2] * w[2][2] + offset;
            // Perspective-correct weights.
            let q = [b[0] * w[0][3], b[1] * w[1][3], b[2] * w[2][3]];
            let qs = q[0] + q[1] + q[2];
            let p = if qs != 0. { [q[0] / qs, q[1] / qs, q[2] / qs] } else { b };
            let mut color = flat.unwrap_or_else(|| [0, 1, 2, 3].map(|i| p[0] * v[0].color[i] + p[1] * v[1].color[i] + p[2] * v[2].color[i]));
            if let Some(t) = tex {
                let tq = p[0] * v[0].tex[3] + p[1] * v[1].tex[3] + p[2] * v[2].tex[3];
                let tq = if tq == 0. { 1. } else { tq };
                let ts = (p[0] * v[0].tex[0] + p[1] * v[1].tex[0] + p[2] * v[2].tex[0]) / tq;
                let tt = (p[0] * v[0].tex[1] + p[1] * v[1].tex[1] + p[2] * v[2].tex[1]) / tq;
                color = tex_env(r, t.format, color, sample(t, ts, tt, minify));
            }
            fragment(s, r, x, y, z.clamp(0., 1.), color);
        }
    }
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
