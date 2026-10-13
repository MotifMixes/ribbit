// WGSL port of ScaleFX by Sp00kyFox (libretro slang-shaders, 2017-03-01),
// hand-translated for Ribbit. Default parameters: threshold 0.50, filter AA
// on, filter corners on.
//
// Copyright (c) 2016 Sp00kyFox - ScaleFX@web.de
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

//! ScaleFX: edge interpolation for pixel art, 3x. Outputs only colours of
//! the original; reconstructs slopes up to level 6.

use super::{Pass, PassKind};

pub const PASSES: &[Pass] = &[
    Pass {
        desc: "ScaleFX pass 0: colour metrics",
        inputs: &["MAIN"],
        outputs: &["sfx0"],
        kind: PassKind::Conv,
        wgsl: PASS0,
    },
    Pass {
        desc: "ScaleFX pass 1: corner strength",
        inputs: &["sfx0"],
        outputs: &["sfx1"],
        kind: PassKind::Conv,
        wgsl: PASS1,
    },
    Pass {
        desc: "ScaleFX pass 2: junction resolution",
        inputs: &["sfx1", "sfx0"],
        outputs: &["sfx2"],
        kind: PassKind::Conv,
        wgsl: PASS2,
    },
    Pass {
        desc: "ScaleFX pass 3: edge levels",
        inputs: &["sfx2"],
        outputs: &["sfx3"],
        kind: PassKind::Conv,
        wgsl: PASS3,
    },
    Pass {
        desc: "ScaleFX pass 4: subpixel output",
        inputs: &["sfx3", "MAIN"],
        outputs: &["MAIN"],
        kind: PassKind::Scale3,
        wgsl: PASS4,
    },
];

const PASS0: &str = r#"
@group(0) @binding(0) var t0: texture_2d<f32>;

var<private> P: vec2<i32>;

fn tap(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(P + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1)), 0);
}

// Reference: http://www.compuphase.com/cmetric.htm
fn dist(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let r = 0.5 * (a.r + b.r);
    let d = a - b;
    let c = vec3<f32>(2.0 + r, 4.0, 3.0 - r);
    return sqrt(dot(c * d, d)) / 3.0;
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    P = vec2<i32>(pos.xy);
    // A B C
    //   E F
    let a = tap(t0, -1, -1).rgb;
    let b = tap(t0, 0, -1).rgb;
    let c = tap(t0, 1, -1).rgb;
    let e = tap(t0, 0, 0).rgb;
    let f = tap(t0, 1, 0).rgb;
    return vec4<f32>(dist(e, a), dist(e, b), dist(e, c), dist(e, f));
}
"#;

const PASS1: &str = r#"
@group(0) @binding(0) var t0: texture_2d<f32>;

var<private> P: vec2<i32>;

fn tap(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(P + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1)), 0);
}

const SFX_CLR: f32 = 0.5;

// Corner strength (filter AA on).
fn strength(d: f32, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let diff = a.x - a.y;
    let wght1 = max(SFX_CLR - d, 0.0) / SFX_CLR;
    let wght2 = clamp((1.0 - d) + select(-diff, diff, min(a.x, b.x) + a.x > min(a.y, b.y) + a.y), 0.0, 1.0);
    return (wght1 * wght2) * (a.x * a.y);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    P = vec2<i32>(pos.xy);
    let A = tap(t0, -1, -1);
    let B = tap(t0, 0, -1);
    let D = tap(t0, -1, 0);
    let E = tap(t0, 0, 0);
    let F = tap(t0, 1, 0);
    let G = tap(t0, -1, 1);
    let H = tap(t0, 0, 1);
    let I = tap(t0, 1, 1);
    return vec4<f32>(
        strength(D.z, vec2<f32>(D.w, E.y), vec2<f32>(A.w, D.y)),
        strength(F.x, vec2<f32>(E.w, E.y), vec2<f32>(B.w, F.y)),
        strength(H.z, vec2<f32>(E.w, H.y), vec2<f32>(H.w, I.y)),
        strength(H.x, vec2<f32>(D.w, H.y), vec2<f32>(G.w, G.y)),
    );
}
"#;

const PASS2: &str = r#"
@group(0) @binding(0) var t0: texture_2d<f32>; // strengths (pass 1)
@group(0) @binding(1) var t1: texture_2d<f32>; // metrics (pass 0)

var<private> P: vec2<i32>;

fn tap(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(P + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1)), 0);
}

const ONE: vec4<f32> = vec4<f32>(1.0);
const ZERO: vec4<f32> = vec4<f32>(0.0);
fn LE(x: vec4<f32>, y: vec4<f32>) -> vec4<f32> { return select(ZERO, ONE, x < y); }
fn GE(x: vec4<f32>, y: vec4<f32>) -> vec4<f32> { return select(ZERO, ONE, x > y); }
fn LEQ(x: vec4<f32>, y: vec4<f32>) -> vec4<f32> { return select(ZERO, ONE, x <= y); }
fn GEs(x: f32, y: f32) -> f32 { return select(0.0, 1.0, x > y); }
fn NOT(x: f32) -> f32 { return 1.0 - x; }

// Corner dominance at junctions.
fn dom(x: vec3<f32>, y: vec3<f32>, z: vec3<f32>, w: vec3<f32>) -> vec4<f32> {
    return 2.0 * vec4<f32>(x.y, y.y, z.y, w.y) - (vec4<f32>(x.x, y.x, z.x, w.x) + vec4<f32>(x.z, y.z, z.z, w.z));
}

// Necessary but not sufficient junction condition for orthogonal edges.
fn clear(crn: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ok = crn.x >= max(min(a.x, a.y), min(b.x, b.y)) && crn.y >= max(min(a.x, b.y), min(b.x, a.y));
    return select(0.0, 1.0, ok);
}

fn vote(jD: vec4<f32>) -> vec4<f32> {
    return min(GE(jD, ZERO) * (LEQ(jD.yzwx, ZERO) * LEQ(jD.wxyz, ZERO) + GE(jD + jD.zwxy, jD.yzwx + jD.wxyz)), ONE);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    P = vec2<i32>(pos.xy);
    // metric data
    let A = tap(t1, -1, -1);
    let B = tap(t1, 0, -1);
    let D = tap(t1, -1, 0);
    let E = tap(t1, 0, 0);
    let F = tap(t1, 1, 0);
    let G = tap(t1, -1, 1);
    let H = tap(t1, 0, 1);
    let I = tap(t1, 1, 1);
    // strength data
    let As = tap(t0, -1, -1);
    let Bs = tap(t0, 0, -1);
    let Cs = tap(t0, 1, -1);
    let Ds = tap(t0, -1, 0);
    let Es = tap(t0, 0, 0);
    let Fs = tap(t0, 1, 0);
    let Gs = tap(t0, -1, 1);
    let Hs = tap(t0, 0, 1);
    let Is = tap(t0, 1, 1);

    // strength & dominance junctions
    let jSx = vec4<f32>(As.z, Bs.w, Es.x, Ds.y);
    let jDx = dom(As.yzw, Bs.zwx, Es.wxy, Ds.xyz);
    let jSy = vec4<f32>(Bs.z, Cs.w, Fs.x, Es.y);
    let jDy = dom(Bs.yzw, Cs.zwx, Fs.wxy, Es.xyz);
    let jSz = vec4<f32>(Es.z, Fs.w, Is.x, Hs.y);
    let jDz = dom(Es.yzw, Fs.zwx, Is.wxy, Hs.xyz);
    let jSw = vec4<f32>(Ds.z, Es.w, Hs.x, Gs.y);
    let jDw = dom(Ds.yzw, Es.zwx, Hs.wxy, Gs.xyz);

    // majority vote for ambiguous dominance junctions
    let jx = vote(jDx);
    let jy = vote(jDy);
    let jz = vote(jDz);
    let jw = vote(jDw);

    // inject strength without creating new contradictions
    var res: vec4<f32>;
    res.x = min(jx.z + NOT(jx.y) * NOT(jx.w) * GEs(jSx.z, 0.0) * (jx.x + GEs(jSx.x + jSx.z, jSx.y + jSx.w)), 1.0);
    res.y = min(jy.w + NOT(jy.z) * NOT(jy.x) * GEs(jSy.w, 0.0) * (jy.y + GEs(jSy.y + jSy.w, jSy.x + jSy.z)), 1.0);
    res.z = min(jz.x + NOT(jz.w) * NOT(jz.y) * GEs(jSz.x, 0.0) * (jz.z + GEs(jSz.x + jSz.z, jSz.y + jSz.w)), 1.0);
    res.w = min(jw.y + NOT(jw.x) * NOT(jw.z) * GEs(jSw.y, 0.0) * (jw.w + GEs(jSw.y + jSw.w, jSw.x + jSw.z)), 1.0);

    // single pixel & end of line detection
    res = min(res * (vec4<f32>(jx.z, jy.w, jz.x, jw.y) + (ONE - res.wxyz * res.yzwx)), ONE);

    // output
    let clr = vec4<f32>(
        clear(vec2<f32>(D.z, E.x), vec2<f32>(D.w, E.y), vec2<f32>(A.w, D.y)),
        clear(vec2<f32>(F.x, E.z), vec2<f32>(E.w, E.y), vec2<f32>(B.w, F.y)),
        clear(vec2<f32>(H.z, I.x), vec2<f32>(E.w, H.y), vec2<f32>(H.w, I.y)),
        clear(vec2<f32>(H.x, G.z), vec2<f32>(D.w, H.y), vec2<f32>(G.w, G.y)),
    );
    let h = vec4<f32>(min(D.w, A.w), min(E.w, B.w), min(E.w, H.w), min(D.w, G.w));
    let v = vec4<f32>(min(E.y, D.y), min(E.y, F.y), min(H.y, I.y), min(H.y, G.y));
    let orien = GE(h + vec4<f32>(D.w, E.w, E.w, D.w), v + vec4<f32>(E.y, E.y, H.y, H.y)); // orientation
    let hori = LE(h, v) * clr; // horizontal edges
    let vert = GE(h, v) * clr; // vertical edges
    return (res + 2.0 * hori + 4.0 * vert + 8.0 * orien) / 15.0;
}
"#;

const PASS3: &str = r#"
@group(0) @binding(0) var t0: texture_2d<f32>;

var<private> P: vec2<i32>;

fn tap(t: texture_2d<f32>, x: i32, y: i32) -> vec4<f32> {
    let size = vec2<i32>(textureDimensions(t));
    return textureLoad(t, clamp(P + vec2<i32>(x, y), vec2<i32>(0), size - vec2<i32>(1)), 0);
}

// The four bool4s packed by pass 2: corners, horizontal edges, vertical
// edges, orientation.
fn bits(x: vec4<f32>, m: f32, o: f32) -> vec4<bool> { return floor((x * m + o) % 2.0) != vec4<f32>(0.0); }
fn corn(x: vec4<f32>) -> vec4<bool> { return bits(x, 15.0, 0.5); }
fn hori(x: vec4<f32>) -> vec4<bool> { return bits(x, 7.5, 0.25); }
fn vert(x: vec4<f32>) -> vec4<bool> { return bits(x, 3.75, 0.125); }
fn orie(x: vec4<f32>) -> vec4<bool> { return bits(x, 1.875, 0.0625); }

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    P = vec2<i32>(pos.xy);
    let E = tap(t0, 0, 0);
    let D = tap(t0, -1, 0);
    let D0 = tap(t0, -2, 0);
    let D1 = tap(t0, -3, 0);
    let F = tap(t0, 1, 0);
    let F0 = tap(t0, 2, 0);
    let F1 = tap(t0, 3, 0);
    let B = tap(t0, 0, -1);
    let B0 = tap(t0, 0, -2);
    let B1 = tap(t0, 0, -3);
    let H = tap(t0, 0, 1);
    let H0 = tap(t0, 0, 2);
    let H1 = tap(t0, 0, 3);

    let Ec = corn(E); let Eh = hori(E); let Ev = vert(E); let Eo = orie(E);
    let Dc = corn(D); let Dh = hori(D); let Do = orie(D); let D0c = corn(D0); let D0h = hori(D0); let D1h = hori(D1);
    let Fc = corn(F); let Fh = hori(F); let Fo = orie(F); let F0c = corn(F0); let F0h = hori(F0); let F1h = hori(F1);
    let Bc = corn(B); let Bv = vert(B); let Bo = orie(B); let B0c = corn(B0); let B0v = vert(B0); let B1v = vert(B1);
    let Hc = corn(H); let Hv = vert(H); let Ho = orie(H); let H0c = corn(H0); let H0v = vert(H0); let H1v = vert(H1);

    // lvl1 corners (filter corners on)
    let lvl1x = Ec.x;
    let lvl1y = Ec.y;
    let lvl1z = Ec.z;
    let lvl1w = Ec.w;

    // lvl2 mid (left, right / up, down)
    let lvl2x = vec2<bool>((Ec.x && Eh.y) && Dc.z, (Ec.y && Eh.x) && Fc.w);
    let lvl2y = vec2<bool>((Ec.y && Ev.z) && Bc.w, (Ec.z && Ev.y) && Hc.x);
    let lvl2z = vec2<bool>((Ec.w && Eh.z) && Dc.y, (Ec.z && Eh.w) && Fc.x);
    let lvl2w = vec2<bool>((Ec.x && Ev.w) && Bc.z, (Ec.w && Ev.x) && Hc.y);

    // lvl3 corners (hori, vert)
    let lvl3x = vec2<bool>(lvl2x.y && (Dh.y && Dh.x) && Fh.z, lvl2w.y && (Bv.w && Bv.x) && Hv.z);
    let lvl3y = vec2<bool>(lvl2x.x && (Fh.x && Fh.y) && Dh.w, lvl2y.y && (Bv.z && Bv.y) && Hv.w);
    let lvl3z = vec2<bool>(lvl2z.x && (Fh.w && Fh.z) && Dh.x, lvl2y.x && (Hv.y && Hv.z) && Bv.x);
    let lvl3w = vec2<bool>(lvl2z.y && (Dh.z && Dh.w) && Fh.y, lvl2w.x && (Hv.x && Hv.w) && Bv.y);

    // lvl4 corners (hori, vert)
    let lvl4x = vec2<bool>(
        (Dc.x && Dh.y && Eh.x && Eh.y && Fh.x && Fh.y) && (D0c.z && D0h.w),
        (Bc.x && Bv.w && Ev.x && Ev.w && Hv.x && Hv.w) && (B0c.z && B0v.y),
    );
    let lvl4y = vec2<bool>(
        (Fc.y && Fh.x && Eh.y && Eh.x && Dh.y && Dh.x) && (F0c.w && F0h.z),
        (Bc.y && Bv.z && Ev.y && Ev.z && Hv.y && Hv.z) && (B0c.w && B0v.x),
    );
    let lvl4z = vec2<bool>(
        (Fc.z && Fh.w && Eh.z && Eh.w && Dh.z && Dh.w) && (F0c.x && F0h.y),
        (Hc.z && Hv.y && Ev.z && Ev.y && Bv.z && Bv.y) && (H0c.x && H0v.w),
    );
    let lvl4w = vec2<bool>(
        (Dc.w && Dh.z && Eh.w && Eh.z && Fh.w && Fh.z) && (D0c.y && D0h.x),
        (Hc.w && Hv.x && Ev.w && Ev.x && Bv.w && Bv.x) && (H0c.y && H0v.z),
    );

    // lvl5 mid (left, right / up, down)
    let lvl5x = vec2<bool>(lvl4x.x && (F0h.x && F0h.y) && (D1h.z && D1h.w), lvl4y.x && (D0h.y && D0h.x) && (F1h.w && F1h.z));
    let lvl5y = vec2<bool>(lvl4y.y && (H0v.y && H0v.z) && (B1v.w && B1v.x), lvl4z.y && (B0v.z && B0v.y) && (H1v.x && H1v.w));
    let lvl5z = vec2<bool>(lvl4w.x && (F0h.w && F0h.z) && (D1h.y && D1h.x), lvl4z.x && (D0h.z && D0h.w) && (F1h.x && F1h.y));
    let lvl5w = vec2<bool>(lvl4x.y && (H0v.x && H0v.w) && (B1v.z && B1v.y), lvl4w.y && (B0v.w && B0v.x) && (H1v.y && H1v.z));

    // lvl6 corners (hori, vert)
    let lvl6x = vec2<bool>(lvl5x.y && (D1h.y && D1h.x), lvl5w.y && (B1v.w && B1v.x));
    let lvl6y = vec2<bool>(lvl5x.x && (F1h.x && F1h.y), lvl5y.y && (B1v.z && B1v.y));
    let lvl6z = vec2<bool>(lvl5z.x && (F1h.w && F1h.z), lvl5y.x && (H1v.y && H1v.z));
    let lvl6w = vec2<bool>(lvl5z.y && (D1h.z && D1h.w), lvl5w.x && (H1v.x && H1v.w));

    // subpixels - 0 = E, 1 = D, 2 = D0, 3 = F, 4 = F0, 5 = B, 6 = B0, 7 = H, 8 = H0
    // (each chain is the original's ?: cascade, highest priority first)
    var crn = vec4<f32>(0.0);
    if ((lvl1x && Eo.x) || (lvl3x.x && Eo.y) || (lvl4x.x && Do.x) || (lvl6x.x && Fo.y)) { crn.x = 5.0; }
    else if (lvl1x || (lvl3x.y && !Eo.w) || (lvl4x.y && !Bo.x) || (lvl6x.y && !Ho.w)) { crn.x = 1.0; }
    else if (lvl3x.x) { crn.x = 3.0; } else if (lvl3x.y) { crn.x = 7.0; }
    else if (lvl4x.x) { crn.x = 2.0; } else if (lvl4x.y) { crn.x = 6.0; }
    else if (lvl6x.x) { crn.x = 4.0; } else if (lvl6x.y) { crn.x = 8.0; }

    if ((lvl1y && Eo.y) || (lvl3y.x && Eo.x) || (lvl4y.x && Fo.y) || (lvl6y.x && Do.x)) { crn.y = 5.0; }
    else if (lvl1y || (lvl3y.y && !Eo.z) || (lvl4y.y && !Bo.y) || (lvl6y.y && !Ho.z)) { crn.y = 3.0; }
    else if (lvl3y.x) { crn.y = 1.0; } else if (lvl3y.y) { crn.y = 7.0; }
    else if (lvl4y.x) { crn.y = 4.0; } else if (lvl4y.y) { crn.y = 6.0; }
    else if (lvl6y.x) { crn.y = 2.0; } else if (lvl6y.y) { crn.y = 8.0; }

    if ((lvl1z && Eo.z) || (lvl3z.x && Eo.w) || (lvl4z.x && Fo.z) || (lvl6z.x && Do.w)) { crn.z = 7.0; }
    else if (lvl1z || (lvl3z.y && !Eo.y) || (lvl4z.y && !Ho.z) || (lvl6z.y && !Bo.y)) { crn.z = 3.0; }
    else if (lvl3z.x) { crn.z = 1.0; } else if (lvl3z.y) { crn.z = 5.0; }
    else if (lvl4z.x) { crn.z = 4.0; } else if (lvl4z.y) { crn.z = 8.0; }
    else if (lvl6z.x) { crn.z = 2.0; } else if (lvl6z.y) { crn.z = 6.0; }

    if ((lvl1w && Eo.w) || (lvl3w.x && Eo.z) || (lvl4w.x && Do.w) || (lvl6w.x && Fo.z)) { crn.w = 7.0; }
    else if (lvl1w || (lvl3w.y && !Eo.x) || (lvl4w.y && !Ho.w) || (lvl6w.y && !Bo.x)) { crn.w = 1.0; }
    else if (lvl3w.x) { crn.w = 3.0; } else if (lvl3w.y) { crn.w = 5.0; }
    else if (lvl4w.x) { crn.w = 2.0; } else if (lvl4w.y) { crn.w = 8.0; }
    else if (lvl6w.x) { crn.w = 4.0; } else if (lvl6w.y) { crn.w = 6.0; }

    var mid = vec4<f32>(0.0);
    if ((lvl2x.x && Eo.x) || (lvl2x.y && Eo.y) || (lvl5x.x && Do.x) || (lvl5x.y && Fo.y)) { mid.x = 5.0; }
    else if (lvl2x.x) { mid.x = 1.0; } else if (lvl2x.y) { mid.x = 3.0; }
    else if (lvl5x.x) { mid.x = 2.0; } else if (lvl5x.y) { mid.x = 4.0; }
    else if (Ec.x && Dc.z && Ec.y && Fc.w) { mid.x = select(1.0, select(3.0, 5.0, Eo.y), Eo.x); }

    if ((lvl2y.x && !Eo.y) || (lvl2y.y && !Eo.z) || (lvl5y.x && !Bo.y) || (lvl5y.y && !Ho.z)) { mid.y = 3.0; }
    else if (lvl2y.x) { mid.y = 5.0; } else if (lvl2y.y) { mid.y = 7.0; }
    else if (lvl5y.x) { mid.y = 6.0; } else if (lvl5y.y) { mid.y = 8.0; }
    else if (Ec.y && Bc.w && Ec.z && Hc.x) { mid.y = select(5.0, select(7.0, 3.0, !Eo.z), !Eo.y); }

    if ((lvl2z.x && Eo.w) || (lvl2z.y && Eo.z) || (lvl5z.x && Do.w) || (lvl5z.y && Fo.z)) { mid.z = 7.0; }
    else if (lvl2z.x) { mid.z = 1.0; } else if (lvl2z.y) { mid.z = 3.0; }
    else if (lvl5z.x) { mid.z = 2.0; } else if (lvl5z.y) { mid.z = 4.0; }
    else if (Ec.z && Fc.x && Ec.w && Dc.y) { mid.z = select(3.0, select(1.0, 7.0, Eo.w), Eo.z); }

    if ((lvl2w.x && !Eo.x) || (lvl2w.y && !Eo.w) || (lvl5w.x && !Bo.x) || (lvl5w.y && !Ho.w)) { mid.w = 1.0; }
    else if (lvl2w.x) { mid.w = 5.0; } else if (lvl2w.y) { mid.w = 7.0; }
    else if (lvl5w.x) { mid.w = 6.0; } else if (lvl5w.y) { mid.w = 8.0; }
    else if (Ec.w && Hc.y && Ec.x && Bc.z) { mid.w = select(7.0, select(5.0, 1.0, !Eo.x), !Eo.w); }

    return (crn + 9.0 * mid) / 80.0;
}
"#;

const PASS4: &str = r#"
@group(0) @binding(0) var t0: texture_2d<f32>; // tags (pass 3)
@group(0) @binding(1) var t1: texture_2d<f32>; // original picture

fn load_crn(x: vec4<f32>) -> vec4<f32> { return floor((x * 80.0 + 0.5) % 9.0); }
fn load_mid(x: vec4<f32>) -> vec4<f32> { return floor((x * 8.888888 + 0.055555) % 9.0); }

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(pos.xy);
    let src = q / 3;
    let fp = q % 3;
    let tags = textureLoad(t0, src, 0);
    let crn = load_crn(tags);
    let mid = load_mid(tags);
    // corners x y / w z, mids x (top) y (right) z (bottom) w (left)
    var sp = 0.0;
    if (fp.y == 0) {
        sp = select(select(crn.y, mid.x, fp.x == 1), crn.x, fp.x == 0);
    } else if (fp.y == 1) {
        sp = select(select(mid.y, 0.0, fp.x == 1), mid.w, fp.x == 0);
    } else {
        sp = select(select(crn.z, mid.z, fp.x == 1), crn.w, fp.x == 0);
    }
    // 0 = E, 1 = D, 2 = D0, 3 = F, 4 = F0, 5 = B, 6 = B0, 7 = H, 8 = H0
    var offsets = array<vec2<i32>, 9>(
        vec2<i32>(0, 0), vec2<i32>(-1, 0), vec2<i32>(-2, 0), vec2<i32>(1, 0), vec2<i32>(2, 0),
        vec2<i32>(0, -1), vec2<i32>(0, -2), vec2<i32>(0, 1), vec2<i32>(0, 2),
    );
    let size = vec2<i32>(textureDimensions(t1));
    let at = clamp(src + offsets[u32(sp)], vec2<i32>(0), size - vec2<i32>(1));
    return vec4<f32>(textureLoad(t1, at, 0).rgb, 1.0);
}
"#;
