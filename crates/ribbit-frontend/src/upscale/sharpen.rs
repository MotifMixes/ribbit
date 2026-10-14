//! Contrast-adaptive sharpening after the upscale (the method AMD published
//! as FidelityFX CAS, written for Ribbit): a cross-shaped unsharp mask whose
//! strength falls where the neighbourhood is already high-contrast, so edges
//! and texture come back without halos or clipping. Counters the flat,
//! painted look neural upscalers give soft gradients.

use super::{Pass, PassKind};

macro_rules! cas {
    ($desc:literal, $peak:literal) => {
        Pass {
            desc: $desc,
            inputs: &["MAIN"],
            outputs: &["MAIN"],
            kind: PassKind::Conv,
            wgsl: concat!(
                "const PEAK: f32 = ",
                $peak,
                r#";

@group(0) @binding(0) var t0: texture_2d<f32>;

fn px(p: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(t0));
    return clamp(textureLoad(t0, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(pos.xy);
    // a b c
    // d e f
    // g h i
    let a = px(q + vec2<i32>(-1, -1));
    let b = px(q + vec2<i32>(0, -1));
    let c = px(q + vec2<i32>(1, -1));
    let d = px(q + vec2<i32>(-1, 0));
    let e = px(q);
    let f = px(q + vec2<i32>(1, 0));
    let g = px(q + vec2<i32>(-1, 1));
    let h = px(q + vec2<i32>(0, 1));
    let i = px(q + vec2<i32>(1, 1));
    // Soft min/max: cross plus corners.
    let mn_cross = min(min(min(d, e), min(f, b)), h);
    let mx_cross = max(max(max(d, e), max(f, b)), h);
    let mn = mn_cross + min(mn_cross, min(min(a, c), min(g, i)));
    let mx = mx_cross + max(mx_cross, max(max(a, c), max(g, i)));
    // Room to sharpen without clipping, relative to the local maximum.
    let amp = sqrt(clamp(min(mn, 2.0 - mx) / max(mx, vec3<f32>(1e-5)), vec3<f32>(0.0), vec3<f32>(1.0)));
    let w = amp * PEAK;
    let out = (b * w + d * w + f * w + h * w + e) / (1.0 + 4.0 * w);
    return vec4<f32>(clamp(out, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
"#
            ),
        }
    };
}

// PEAK = -1 / mix(8, 5, sharpness): sharpness 0.4, 0.7, 1.0.
pub const LOW: &[Pass] = &[cas!("Sharpen (low)", "-0.1471")];
pub const MEDIUM: &[Pass] = &[cas!("Sharpen (medium)", "-0.1695")];
pub const HIGH: &[Pass] = &[cas!("Sharpen (high)", "-0.2")];

/// Strongest: an unsharp mask (3x3 Gaussian, amount 1.5) limited to the
/// local minimum and maximum, so edges steepen without halos or ringing.
pub const MAX: &[Pass] = &[Pass {
    desc: "Sharpen (max, limited unsharp mask)",
    inputs: &["MAIN"],
    outputs: &["MAIN"],
    kind: PassKind::Conv,
    wgsl: r#"
@group(0) @binding(0) var t0: texture_2d<f32>;

fn px(p: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(t0));
    return clamp(textureLoad(t0, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(pos.xy);
    var blur = vec3<f32>(0.0);
    var mn = vec3<f32>(1.0);
    var mx = vec3<f32>(0.0);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            let c = px(q + vec2<i32>(i, j));
            let w = f32((2 - abs(i)) * (2 - abs(j)));
            blur += c * w;
            mn = min(mn, c);
            mx = max(mx, c);
        }
    }
    blur /= 16.0;
    let e = px(q);
    let out = clamp(e + 1.5 * (e - blur), mn, mx);
    return vec4<f32>(out, 1.0);
}
"#,
}];

/// Shock filter (Osher-Rudin style), one iteration: near an edge each pixel
/// moves towards the colour on its own side (the local darkest where the
/// luma Laplacian says it is on the dark side, the local brightest
/// otherwise), by an amount that grows with the edge gradient. Soft ramps
/// become crisp steps, flat areas do not change: cartoon line art (and text)
/// regains hard, clean outlines. Run at the upscaled resolution, a few times.
const SHOCK_WGSL: &str = r#"
@group(0) @binding(0) var t0: texture_2d<f32>;

fn px(p: vec2<i32>) -> vec3<f32> {
    let size = vec2<i32>(textureDimensions(t0));
    return clamp(textureLoad(t0, clamp(p, vec2<i32>(0), size - vec2<i32>(1)), 0).rgb, vec3<f32>(0.0), vec3<f32>(1.0));
}

fn luma(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.299, 0.587, 0.114));
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let q = vec2<i32>(pos.xy);
    let e = px(q);
    var dark = e;
    var bright = e;
    var lap = -8.0 * luma(e);
    for (var j = -1; j <= 1; j++) {
        for (var i = -1; i <= 1; i++) {
            if (i == 0 && j == 0) {
                continue;
            }
            let c = px(q + vec2<i32>(i, j));
            lap += luma(c);
            if (luma(c) < luma(dark)) {
                dark = c;
            }
            if (luma(c) > luma(bright)) {
                bright = c;
            }
        }
    }
    let gx = luma(px(q + vec2<i32>(1, 0))) - luma(px(q - vec2<i32>(1, 0)));
    let gy = luma(px(q + vec2<i32>(0, 1))) - luma(px(q - vec2<i32>(0, 1)));
    let grad = sqrt(gx * gx + gy * gy);
    let amount = clamp(grad * 2.5, 0.0, 1.0) * 0.6;
    // Laplacian > 0: neighbours brighter than the centre, so the centre is
    // on the dark side of the edge.
    let goal = select(bright, dark, lap > 0.0);
    return vec4<f32>(mix(e, goal, amount), 1.0);
}
"#;

const SHOCK: Pass = Pass {
    desc: "Crisp lines (shock filter)",
    inputs: &["MAIN"],
    outputs: &["MAIN"],
    kind: PassKind::Conv,
    wgsl: SHOCK_WGSL,
};

/// Crisp cartoon lines: three shock-filter iterations, then light
/// contrast-adaptive sharpening.
pub const CRISP: &[Pass] = &[SHOCK, SHOCK, SHOCK];
