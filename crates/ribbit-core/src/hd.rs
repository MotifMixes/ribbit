//! EXPERIMENTAL: HD capture from the BIOS's Flash rasterizer.
//!
//! The BIOS rasterizes Flash shapes from edge records in quarter-pixel
//! device coordinates (a 640x640 grid) and averages the result down to the
//! 160x160 LCD (docs/hardware-research.md, "The BIOS's Flash rasterizer").
//! This module records, for each render pass, every edge record the BIOS
//! builds, the fill styles they use and the pass's redraw rectangle, so a
//! renderer can draw the same picture at high resolution. It observes only:
//! the emulated machine runs exactly as without it.
//!
//! Hooks (found by code signature, so BIOS revisions with the same player
//! build work): the edge constructor (r1 = edge record) and a point the
//! scanline driver passes once per scanline (r14 = renderer, whose +0x28,
//! +0x2C, +0x30, +0x34 are the redraw rectangle x0 x1 y0 y1 in quarter
//! pixels). A pass ends at the framebuffer transfer to the LCD.

use std::collections::HashMap;
use std::sync::Arc;

/// First 44 bytes of the edge constructor (US v2.1 0x4004_E0D8; no absolute
/// addresses inside, so the same compiled code matches in other builds).
const EDGE_CTOR_SIG: [u8; 44] = [
    0xe1, 0xc5, 0xe1, 0xc6, 0xe1, 0xc7, 0xfc, 0x1c, 0x08, 0xb4, 0xfc, 0x1c, 0x48, 0xb4, 0xfc, 0x1c, 0x88, 0xb4, 0xfc,
    0x1c, 0xc8, 0xb4, 0xfc, 0x1c, 0x08, 0xb5, 0xfc, 0x1c, 0x48, 0xb5, 0xf1, 0xc0, 0xa4, 0xc1, 0x08, 0x75, 0x1c, 0x89,
    0x28, 0x76, 0x1a, 0x72, 0x24, 0xe8,
];
/// The scanline driver at US v2.1 0x4005_0514 (20 bytes).
const SCANLINE_SIG: [u8; 20] =
    [0x1c, 0x86, 0x80, 0x20, 0x1f, 0x0f, 0xac, 0x10, 0x00, 0x80, 0x84, 0xe0, 0x1c, 0x01, 0x02, 0x00, 0xd4, 0x16, 0x03, 0x10];

/// The solid-colour span painter at US v2.1 0x4005_544C (24 bytes): fills
/// painted by it are plain colours; others (gradients, bitmaps) are not.
const SOLID_PAINTER_SIG: [u8; 24] = [
    0xe1, 0xc5, 0xe1, 0xc6, 0xfc, 0x1c, 0x08, 0xb4, 0xf1, 0xc0, 0x08, 0x76, 0x02, 0x22, 0x4d, 0x00, 0x8c, 0x25, 0x04, 0x90, 0x1a,
    0x73, 0xd0, 0xf6,
];

/// The bitmap-fill span painter at US v2.1 0x4005_4D24: these 32 bytes
/// start 0x32 bytes in (its prologue alone is shared by many functions).
const BITMAP_PAINTER_SIG: [u8; 32] = [
    0x5a, 0x70, 0x3a, 0x71, 0x40, 0x29, 0x00, 0x04, 0x41, 0xc0, 0x00, 0x12, 0x00, 0x20, 0x7a, 0x73, 0x5f, 0x10, 0x00, 0x06,
    0x1a, 0x72, 0x10, 0xb8, 0x42, 0xc0, 0x55, 0x22, 0x40, 0x25, 0x81, 0xc1,
];
const BITMAP_PAINTER_SIG_AT: u32 = 0x32;

/// The device-font text loop's call of its glyph blitter (US v2.1
/// 0x4009_7220, `jl_s.d [r15]` with `mov r7, r23` in its delay slot): these
/// 58 bytes start 0x32 before it. At the call r0 is the target surface, r1
/// the glyph's coverage bitmap, r2 its bits per pixel, r3/r4 its width and
/// height, r5/r6 its position, r23 the colour (bytes R G B). Device text
/// (Flash text fields with device fonts, such as "Leapster Mono") is drawn
/// this way, not by the shape rasterizer.
const GLYPH_CALL_SIG: [u8; 58] = [
    0x03, 0x14, 0xc0, 0x30, 0x02, 0x14, 0xc1, 0x30, 0xb8, 0x60, 0x2f, 0x25, 0x06, 0x00, 0x02, 0x23, 0x40, 0x20, 0x2f,
    0x26, 0x06, 0x00, 0x08, 0x11, 0x00, 0x20, 0x01, 0x14, 0x8f, 0x30, 0x03, 0x80, 0x02, 0xc1, 0x27, 0xb8, 0x53, 0x20,
    0xc3, 0x00, 0x70, 0x7a, 0x0a, 0x70, 0x20, 0xc3, 0x98, 0x77, 0x4a, 0x77, 0x60, 0x7f, 0x0a, 0x27, 0xc0, 0x05, 0x21,
    0xc0,
];
const GLYPH_CALL_SIG_AT: u32 = 0x32;

/// Where the hooks are in one BIOS image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hooks {
    pub edge_ctor: u32,
    pub scanline: u32,
    pub solid_painter: u32,
    pub bitmap_painter: u32,
    /// The device-font glyph blit call (None: device text stays as the
    /// LCD shows it).
    pub glyph: Option<u32>,
}

impl Hooks {
    /// Finds both hooks in a BIOS image mapped at `base`; None unless each
    /// signature occurs (the first match is used).
    pub fn find(bios: &[u8], base: u32) -> Option<Hooks> {
        let find = |sig: &[u8]| {
            bios.windows(sig.len()).enumerate().step_by(2).find(|(_, w)| *w == sig).map(|(i, _)| base + i as u32)
        };
        Some(Hooks {
            edge_ctor: find(&EDGE_CTOR_SIG)?,
            scanline: find(&SCANLINE_SIG)?,
            solid_painter: find(&SOLID_PAINTER_SIG)?,
            bitmap_painter: find(&BITMAP_PAINTER_SIG)? - BITMAP_PAINTER_SIG_AT,
            glyph: find(&GLYPH_CALL_SIG).map(|a| a + GLYPH_CALL_SIG_AT),
        })
    }
}

/// An edge in the pass's units (`Pass::scale` per pixel): start, control and end (a straight edge has
/// its control point on the line), and its two fill styles (0 = none).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Edge {
    pub p: [(i16, i16); 3],
    pub line: bool,
    pub fills: [u32; 2],
    /// How a crossing changes the fills (record +0x1D; VERIFIED-FW, US v2.1
    /// handlers 0x4005_0178, 0x4005_02F0, 0x4005_03B8): 0 flips both
    /// fills, 1 flips the first, 2 adds `dir` to the first fill's winding
    /// count (on while non-zero: strokes, whose pieces overlap).
    pub rule: u8,
    /// The edge's direction, +1 or -1 (record +0x1E, signed).
    pub dir: i8,
}

/// A fill style, keyed by its address in the pass.
#[derive(Debug, Clone, PartialEq)]
pub struct Fill {
    /// Stacking order (higher draws on top): +0x0C, depth in the high half.
    pub order: u32,
    /// +0x24: colour (stored B, G, R, A as u16).
    pub rgba: [u8; 4],
    /// The fill's span painter (+0x20): tells solid fills from others.
    pub painter: u32,
    /// A bitmap fill the renderer can follow exactly: its bitmap and mapping
    /// (None for other fills, and for bitmaps with a colour transform).
    pub bitmap: Option<BitmapFill>,
}

/// A decoded bitmap: 0xAARRGGBB, premultiplied as the BIOS keeps it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u32>,
}

/// A bitmap fill (VERIFIED-FW, US v2.1 painter 0x4005_4D24): texel (u, v) =
/// matrix x LCD pixel (x, y), matrix at fill +0x54 as 16.16 a b c d tx ty
/// (u = a x + c y + tx, v = b x + d y + ty); +0x34 bit 0 clamps at the edges
/// (else the bitmap repeats); +0x36 smooths (bilinear); +0x38 the bitmap.
#[derive(Debug, Clone, PartialEq)]
pub struct BitmapFill {
    pub matrix: [f32; 6],
    pub clamp: bool,
    pub smooth: bool,
    pub image: Arc<Image>,
}

/// A device-font glyph drawn on the LCD's framebuffer (VERIFIED-FW, US v2.1
/// blitter 0x4009_5BEC): its coverage per pixel (0-64, the BIOS's blend
/// weights for 4-bit glyphs: 4-bit values, high nibble first, rows of
/// whole bytes), where it goes and its clip rectangle (LCD pixels, x0 y0 x1
/// y1 inclusive, the surface's +0x14), and the colour as the 12-bit LCD
/// stores it.
#[derive(Debug, Clone, PartialEq)]
pub struct Glyph {
    pub x: i32,
    pub y: i32,
    pub width: usize,
    pub height: usize,
    pub coverage: Vec<u8>,
    pub clip: [i32; 4],
    pub rgb: [u8; 3],
    /// The character, its font's name (the BIOS font resource's +0x04,
    /// e.g. "Leapster Mono") and size (pixels per em, the resource's
    /// +0x10), and the pen: x and baseline y (LCD pixels), for drawing it
    /// from the typeface's outlines when they are known.
    pub ch: u16,
    pub font: String,
    pub em: f32,
    pub pen: (i32, i32),
}

/// Everything one render pass drew.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pass {
    /// Redraw rectangle x0, x1, y0, y1 in the pass's units (end exclusive).
    pub rect: Option<[i32; 4]>,
    /// Units per LCD pixel (renderer +0x00): 4 at the BIOS's highest
    /// quality, 2 at medium (games choose, e.g. LF_SetMovieQuality).
    pub scale: i32,
    /// Scanlines this render swept, first and last + 1, in the pass's units
    /// (the BIOS may draw one rectangle in several sweeps).
    pub rows: Option<(i32, i32)>,
    /// The renderer object (r14): which surface the render draws into.
    pub renderer: u32,
    pub edges: Vec<Edge>,
    pub fills: HashMap<u32, Fill>,
    /// Device-font glyphs drawn after the frame's shapes (in a pass of
    /// their own, with no edges).
    pub glyphs: Vec<Glyph>,
}

/// Recorder state, owned by the machine while HD capture is on.
#[derive(Debug, Clone)]
pub struct HdCapture {
    pub hooks: Hooks,
    current: Pass,
    /// Finished passes, oldest first, for the renderer to take.
    pub done: Vec<Pass>,
    /// Renders of the frame in progress: released to `done` together at
    /// the frame's transfer to the LCD, so a renderer never has part of a
    /// frame while the LCD still shows the previous one.
    frame: Vec<Pass>,
    /// Passes dropped because no renderer took them (the HD picture may then
    /// be stale until the next full redraw).
    pub dropped: u64,
    /// Edges (and their fills) built since the last scanline event: the BIOS
    /// builds a render's first edges before its first scanline reports the
    /// render's rectangle, so the next scanline event says where they go.
    pending: Pass,
    /// The last scanline counter seen (a lower one starts a new render).
    last_scan: i32,
    /// Bitmaps decoded so far, by (data, palette, format, size, stride, sample).
    images: HashMap<ImageKey, Arc<Image>>,
    /// Device-font glyphs of the frame in progress.
    glyphs: Vec<Glyph>,
}

/// A bitmap's identity: data, palette, format, width | height << 16,
/// stride and a content sample.
type ImageKey = (u32, u32, u32, u32, u32, u64);

/// More passes than this unconsumed means nobody is rendering; stop growing.
const MAX_PENDING: usize = 64;

impl HdCapture {
    pub fn new(hooks: Hooks) -> Self {
        Self {
            hooks,
            current: Pass::default(),
            done: vec![],
            frame: vec![],
            dropped: 0,
            pending: Pass::default(),
            last_scan: 0,
            images: HashMap::new(),
            glyphs: vec![],
        }
    }

    /// The device-font text loop is calling its glyph blitter (registers
    /// as at `GLYPH_CALL_SIG`). Only 4-bit glyphs drawn on a 160x160
    /// 12-bit surface (the LCD's framebuffer) are kept.
    pub fn glyph(&mut self, regs: &[u32], peek: impl Fn(u32) -> u8) {
        let w = |a: u32| u32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
        let h = |a: u32| i16::from_le_bytes([peek(a), peek(a + 1)]) as i32;
        let (surface, bitmap, bpp, width, height) = (regs[0], regs[1], regs[2], regs[3] as usize, regs[4] as usize);
        if bpp != 4 || w(surface + 4) != 160 || w(surface + 8) != 160 || w(surface + 0x10) != 12 || width > 64 || height > 64 {
            return;
        }
        let row = width.div_ceil(2) as u32;
        let coverage = (0..height as u32)
            .flat_map(|y| (0..width as u32).map(move |x| (y, x)))
            .map(|(y, x)| {
                let byte = peek(bitmap + y * row + x / 2);
                let v = if x % 2 == 0 { byte >> 4 } else { byte & 15 } as u32;
                // The BIOS's table: 0, 5, 9, 13, ... 60, 64.
                ((v * 64 + 7) / 15) as u8
            })
            .collect();
        let nib = |c: u32| ((c & 0xF0) | (c >> 4)) as u8;
        let c = regs[23];
        // The text loop's own registers: r22 the string, r14 the index,
        // r17 the run (+0x08 the BIOS font resource), r13 the pen x, r19
        // the baseline.
        let res = w(regs[17] + 8);
        let name_at = w(res + 4);
        let font: String = (0..32).map(|i| peek(name_at + i)).take_while(|&b| b != 0).map(|b| b as char).collect();
        self.glyphs.push(Glyph {
            x: regs[5] as i16 as i32,
            y: regs[6] as i16 as i32,
            width,
            height,
            coverage,
            clip: [h(surface + 0x14), h(surface + 0x16), h(surface + 0x18), h(surface + 0x1A)],
            rgb: [nib(c & 0xFF), nib((c >> 8) & 0xFF), nib((c >> 16) & 0xFF)],
            ch: peek(regs[22] + regs[14]) as u16,
            font,
            em: peek(res + 0x10) as f32,
            pen: (regs[13] as i16 as i32, regs[19] as i16 as i32),
        });
    }

    /// The edge constructor was executed: `rec` is its edge record.
    pub fn edge(&mut self, rec: u32, peek: impl Fn(u32) -> u8) {
        let b = |o: u32| peek(rec + o);
        let h = |o: u32| i16::from_le_bytes([b(o), b(o + 1)]);
        let w = |a: u32| u32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
        let e = Edge {
            p: [(h(0x10), h(0x12)), (h(0x14), h(0x16)), (h(0x18), h(0x1A))],
            line: b(0x1C) != 0,
            fills: [w(rec + 0x08), w(rec + 0x0C)],
            rule: b(0x1D),
            dir: b(0x1E) as i8,
        };
        for f in e.fills {
            if f != 0 && !self.pending.fills.contains_key(&f) {
                let c = |o: u32| u16::from_le_bytes([peek(f + o), peek(f + o + 1)]).min(255) as u8;
                let painter = w(f + 0x20);
                let bitmap = if painter == self.hooks.bitmap_painter { self.bitmap_fill(f, &peek) } else { None };
                let fill = Fill { order: w(f + 0x0C), rgba: [c(0x28), c(0x26), c(0x24), c(0x2A)], painter, bitmap };
                self.pending.fills.insert(f, fill);
            }
        }
        self.pending.edges.push(e);
    }

    /// Reads bitmap fill `f` and decodes its bitmap (cached). None when it
    /// has a colour transform (+0x80) or an unknown format.
    fn bitmap_fill(&mut self, f: u32, peek: &impl Fn(u32) -> u8) -> Option<BitmapFill> {
        let w = |a: u32| u32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
        if w(f + 0x80) != 0 {
            return None;
        }
        let matrix = [0, 4, 8, 12, 16, 20].map(|o| w(f + 0x54 + o) as i32 as f32 / 65536.0);
        let b = w(f + 0x38);
        let (data, palette, format, width, height, stride) =
            (w(b + 4), w(b + 8), w(b + 0x0C), w(b + 0x10), w(b + 0x14), w(b + 0x18));
        if format > 5 || width == 0 || height == 0 || width > 4096 || height > 4096 {
            return None;
        }
        // A content sample, so a reused buffer with new pixels is decoded again.
        let size = stride.saturating_mul(height);
        let sample = (0..64u32).fold(0u64, |h, i| {
            (h ^ peek(data.wrapping_add(i * (size / 64).max(1))) as u64).wrapping_mul(0x0000_0100_0000_01b3)
        });
        let key = (data, palette, format, width | height << 16, stride, sample);
        let image = self
            .images
            .entry(key)
            .or_insert_with(|| Arc::new(decode_image(peek, data, palette, format, width, height, stride)))
            .clone();
        Some(BitmapFill { matrix, clamp: peek(f + 0x34) & 1 != 0, smooth: peek(f + 0x36) != 0, image })
    }

    /// The scanline driver ran with renderer `r` at scanline counter `y`
    /// (r17, rising through a render): a different rectangle or scale, or a
    /// lower counter, means a new render. The edges built since the last
    /// event belong to this one.
    pub fn scanline(&mut self, r: u32, y: i32, peek: impl Fn(u32) -> u8) {
        let w = |a: u32| i32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
        let (rect, scale) = (Some([w(r + 0x28), w(r + 0x2C), w(r + 0x30), w(r + 0x34)]), w(r));
        if self.current.rect.is_some() && ((self.current.rect, self.current.scale) != (rect, scale) || y < self.last_scan) {
            self.finish();
        }
        self.last_scan = y;
        self.current.renderer = r;
        // r17 runs one ahead of the scanline being drawn.
        self.current.rows = Some(match self.current.rows {
            Some((a, _)) => (a, y),
            None => (y - 1, y),
        });
        self.current.rect = rect;
        self.current.scale = scale;
        self.absorb_pending();
    }

    fn absorb_pending(&mut self) {
        let p = std::mem::take(&mut self.pending);
        self.current.edges.extend(p.edges);
        for (a, f) in p.fills {
            self.current.fills.entry(a).or_insert(f);
        }
    }

    /// The frame went to the LCD: its renders are complete.
    pub fn end_pass(&mut self) {
        self.absorb_pending();
        self.finish();
        if !self.glyphs.is_empty() {
            self.frame.push(Pass { scale: 4, glyphs: std::mem::take(&mut self.glyphs), ..Default::default() });
        }
        if self.done.len() + self.frame.len() > MAX_PENDING {
            self.dropped += self.done.len() as u64;
            self.done.clear();
        }
        self.done.append(&mut self.frame);
    }

    fn finish(&mut self) {
        if self.current.edges.is_empty() && self.current.rect.is_none() {
            return;
        }
        self.frame.push(std::mem::take(&mut self.current));
    }

    /// Takes the finished passes, oldest first.
    pub fn take(&mut self) -> Vec<Pass> {
        std::mem::take(&mut self.done)
    }
}

/// Decodes a BIOS bitmap (VERIFIED-FW, texel fetch at US v2.1 0x4005_66E0):
/// formats 0-3 are 1/2/4/8-bit palette indices, most significant bits
/// first, palette entries B G R A from `palette + 4`; 4 is RGB555 (5-bit
/// channels widened by bit replication; the BIOS uses its own table, about
/// the same); 5 is 32-bit B G R A.
fn decode_image(
    peek: &impl Fn(u32) -> u8,
    data: u32,
    palette: u32,
    format: u32,
    width: u32,
    height: u32,
    stride: u32,
) -> Image {
    let word = |a: u32| u32::from_le_bytes([peek(a), peek(a + 1), peek(a + 2), peek(a + 3)]);
    let pal = |i: u32| word(palette + 4 + 4 * i);
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        let row = data + y * stride;
        for x in 0..width {
            let bits = |bpp: u32| {
                let bit = x * bpp;
                (peek(row + bit / 8) as u32 >> (8 - bpp - bit % 8)) & ((1 << bpp) - 1)
            };
            pixels.push(match format {
                0 => pal(bits(1)),
                1 => pal(bits(2)),
                2 => pal(bits(4)),
                3 => pal(peek(row + x) as u32),
                4 => {
                    let p = u16::from_le_bytes([peek(row + 2 * x), peek(row + 2 * x + 1)]) as u32;
                    let c = |v: u32| (v << 3) | (v >> 2);
                    0xFF00_0000 | c((p >> 10) & 31) << 16 | c((p >> 5) & 31) << 8 | c(p & 31)
                }
                _ => word(row + 4 * x),
            });
        }
    }
    Image { width: width as usize, height: height as usize, pixels }
}

/// Side of the default HD picture: the BIOS's own quarter-pixel grid. A
/// canvas can be larger (`HdCanvas::with_scale`): the BIOS's curves are
/// exact enough to draw at screen resolution.
pub const HD_SIZE: usize = 640;
/// Sub-rows sampled per canvas row (horizontal coverage is exact).
const SUB: usize = 4;

/// How a fill is drawn in HD.
#[derive(Clone)]
enum Paint {
    /// Plain colour, straight alpha.
    Solid([f32; 4]),
    /// A bitmap, sampled per canvas pixel.
    Bitmap(BitmapFill),
    /// Anything the canvas cannot draw (e.g. a gradient): left unknown, for
    /// the fallback picture to fill.
    Unknown,
}

/// What covers a stretch of a sub-row.
enum Look {
    /// The same everywhere: premultiplied colour of the drawable fills, the
    /// weight left beneath them, and whether that weight is a fill the
    /// canvas cannot draw (unknown) or nothing at all (the canvas keeps what
    /// it has: like the BIOS's framebuffer, a render only paints where it
    /// has fills, and one frame is often several renders layered).
    Uniform([f32; 3], f32, bool),
    /// Depends on the pixel (a bitmap is involved): the covering fills, top
    /// first, down to the first that hides everything below.
    Varying(Vec<usize>),
}

/// One flattened piece of an edge, in canvas pixels, with its fills
/// (indices into the pass's fill list, -1 = none).
#[derive(Clone, Copy)]
struct Seg {
    x0: f32,
    y0: f32,
    x1: f32,
    y1: f32,
    fills: [i32; 2],
    rule: u8,
    dir: i8,
}

/// The HD picture, kept across passes like the BIOS keeps its framebuffer
/// (a pass redraws only its rectangle). It holds only what was drawn from
/// the BIOS's shapes and bitmaps; whatever it cannot know comes from a
/// fallback picture (the LCD, smoothed or upscaled) when it is shown, as
/// marked by its alpha and by `mask`.
#[derive(Clone)]
pub struct HdCanvas {
    /// Side in pixels (160 x scale).
    pub size: usize,
    /// Canvas pixels per LCD pixel.
    pub scale: usize,
    /// 0xAARRGGBB, premultiplied: alpha is how much of the pixel the canvas
    /// knows (255 drawn, 0 unknown).
    pub pixels: Vec<u32>,
    /// Per LCD pixel (160x160): 255 where the shown picture must come from
    /// the fallback (`update_mask`).
    pub mask: Vec<u8>,
    solid_painter: u32,
    pub passes: u64,
    /// The LCD frame at the last `update_mask`, the LCD pixels renders have
    /// covered since, and those found stale: an LCD pixel that changed
    /// where no render drew was drawn some way the capture missed.
    prev_lcd: Vec<u32>,
    covered: Vec<bool>,
    stale: Vec<bool>,
    /// LCD pixels last drawn from a font outline: they differ from the
    /// BIOS's pixel glyph by design, so the LCD comparison does not apply
    /// (until a render draws there again).
    from_outline: Vec<bool>,
    /// Outline fonts by name (from the player's cartridges, swf_font.rs):
    /// device text in these is drawn from its curves, else from the BIOS's
    /// pixel glyphs.
    pub fonts: Option<Arc<HashMap<String, crate::swf_font::OutlineFont>>>,
}

/// Largest per-channel difference between a canvas block's average and its
/// LCD pixel that still counts as the same picture (the LCD keeps 4 bits per
/// channel, and the BIOS's anti-aliasing differs slightly from ours), beyond
/// the range of the LCD pixel's 3x3 neighbourhood (an edge block blends its
/// neighbours).
const AGREE: i32 = 40;
/// Largest difference from the block's own LCD pixel, whatever its
/// neighbours: anti-aliasing never differs this much, but things drawn
/// another way (the BIOS's pixel-font text) do.
const FAR: i32 = 96;

impl HdCanvas {
    /// A 640x640 canvas (the BIOS's quarter-pixel grid).
    pub fn new(hooks: Hooks) -> Self {
        Self::with_scale(hooks, 4)
    }

    /// A canvas of 160 x `scale` pixels square (scale 4 to 16).
    pub fn with_scale(hooks: Hooks, scale: usize) -> Self {
        let scale = scale.clamp(4, 16);
        let size = 160 * scale;
        Self {
            size,
            scale,
            pixels: vec![0; size * size],
            mask: vec![255; 160 * 160],
            solid_painter: hooks.solid_painter,
            passes: 0,
            prev_lcd: vec![],
            covered: vec![false; 160 * 160],
            stale: vec![false; 160 * 160],
            from_outline: vec![false; 160 * 160],
            fonts: None,
        }
    }

    /// Draws `pass` inside its redraw rectangle.
    pub fn apply(&mut self, pass: &Pass) {
        self.passes += 1;
        if pass.edges.is_empty() && !pass.glyphs.is_empty() {
            for g in &pass.glyphs {
                self.draw_glyph(g);
            }
            return;
        }
        // Pass units to canvas pixels (a pass with no rectangle seen is taken
        // as a full-quality full redraw).
        let units = match pass.scale {
            1 | 2 | 4 => pass.scale,
            _ => 4,
        };
        let k = self.scale as f32 / units as f32;
        let n = self.size as f32;
        let r = pass.rect.map_or([0.0, n, 0.0, n], |r| r.map(|v| v as f32 * k));
        let (mut ry0, mut ry1) = (r[2], r[3]);
        if let Some((a, b)) = pass.rows {
            (ry0, ry1) = (ry0.max(a as f32 * k), ry1.min(b as f32 * k));
        }
        let clamp = |v: f32| (v.round().max(0.0) as usize).min(self.size);
        let (x0, x1, y0, y1) = (clamp(r[0]), clamp(r[1]), clamp(ry0), clamp(ry1));
        if x0 >= x1 || y0 >= y1 {
            return;
        }
        let s = self.scale;
        for py in y0 / s..y1.div_ceil(s) {
            self.covered[py * 160 + x0 / s..py * 160 + x1.div_ceil(s)].fill(true);
            self.from_outline[py * 160 + x0 / s..py * 160 + x1.div_ceil(s)].fill(false);
        }
        // Fills bottom to top.
        let mut order: Vec<(&u32, &Fill)> = pass.fills.iter().collect();
        order.sort_by_key(|(a, f)| (f.order, **a));
        let index: HashMap<u32, i32> = order.iter().enumerate().map(|(i, (a, _))| (**a, i as i32)).collect();
        let paints: Vec<Paint> = order
            .iter()
            .map(|(_, f)| match (&f.bitmap, f.painter == self.solid_painter) {
                (Some(b), _) => Paint::Bitmap(b.clone()),
                (None, true) => Paint::Solid(f.rgba.map(|c| c as f32 / 255.0)),
                (None, false) => Paint::Unknown,
            })
            .collect();
        // Edges flattened (finer as the canvas grows); each segment listed
        // under the rows it crosses.
        let mut segs: Vec<Seg> = vec![];
        for e in &pass.edges {
            let fills = e.fills.map(|f| if f == 0 { -1 } else { index.get(&f).copied().unwrap_or(-1) });
            if fills == [-1, -1] {
                continue;
            }
            let p = e.p.map(|(x, y)| (x as f32 * k, y as f32 * k));
            let n = if e.line {
                1
            } else {
                let d = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() + (a.1 - b.1).abs();
                ((d(p[0], p[1]) + d(p[1], p[2])) / 6.0).ceil().clamp(1.0, 96.0) as usize
            };
            let at = |t: f32| {
                let u = 1.0 - t;
                let (a, b, c) = (u * u, 2.0 * u * t, t * t);
                (a * p[0].0 + b * p[1].0 + c * p[2].0, a * p[0].1 + b * p[1].1 + c * p[2].1)
            };
            let mut prev = p[0];
            for i in 1..=n {
                let q = if i == n { p[2] } else { at(i as f32 / n as f32) };
                if prev.1 != q.1 {
                    segs.push(Seg { x0: prev.0, y0: prev.1, x1: q.0, y1: q.1, fills, rule: e.rule, dir: e.dir });
                }
                prev = q;
            }
        }
        let mut by_row: Vec<Vec<u32>> = vec![vec![]; y1 - y0];
        for (i, s) in segs.iter().enumerate() {
            let (lo, hi) = (s.y0.min(s.y1), s.y0.max(s.y1));
            let (a, b) = ((lo.floor() as i64).max(y0 as i64), (hi.ceil() as i64).min(y1 as i64));
            for y in a..b {
                by_row[(y as usize) - y0].push(i as u32);
            }
        }
        // Rows are independent: big renders are split across cores.
        let size = self.size;
        let rows = &mut self.pixels[y0 * size..y1 * size];
        let work = (y1 - y0) * (x1 - x0);
        let threads = if work < 200_000 {
            1
        } else {
            std::thread::available_parallelism().map_or(4, |t| t.get()).clamp(1, 16)
        };
        let band = (y1 - y0).div_ceil(threads);
        let ctx = Raster { segs: &segs, paints: &paints, x0, x1, scale: s as f32 };
        std::thread::scope(|scope| {
            for (b, (chunk, lists)) in rows.chunks_mut(band * size).zip(by_row.chunks(band)).enumerate() {
                let ctx = &ctx;
                let first = y0 + b * band;
                if threads == 1 {
                    ctx.rows(chunk, lists, first, size);
                } else {
                    scope.spawn(move || ctx.rows(chunk, lists, first, size));
                }
            }
        });
    }

    /// Draws a device-font glyph as the BIOS does on the LCD, one block of
    /// canvas pixels per glyph pixel: the colour over what is there, by the
    /// pixel's coverage. The glyph exists only at the LCD's resolution (the
    /// font is a pixel font), so this is its true shape.
    fn draw_glyph(&mut self, g: &Glyph) {
        let outline = self.fonts.as_ref().and_then(|f| f.get(&g.font)).and_then(|f| f.glyphs.get(&g.ch)).filter(|o| !o.pieces.is_empty()).cloned();
        if let Some(o) = outline {
            return self.draw_outline(g, &o);
        }
        let s = self.scale;
        let [cx0, cy0, cx1, cy1] = g.clip;
        for gy in 0..g.height {
            for gx in 0..g.width {
                let (x, y) = (g.x + gx as i32, g.y + gy as i32);
                if x < cx0.max(0) || y < cy0.max(0) || x > cx1.min(159) || y > cy1.min(159) {
                    continue;
                }
                let a = g.coverage[gy * g.width + gx] as u32;
                if a == 0 {
                    continue;
                }
                self.covered[y as usize * 160 + x as usize] = true;
                for py in y as usize * s..(y as usize + 1) * s {
                    for p in &mut self.pixels[py * self.size + x as usize * s..py * self.size + (x as usize + 1) * s] {
                        let ch = |k: u32, c: u8| ((c as u32 * a + ((*p >> k) & 0xFF) * (64 - a)) / 64) << k;
                        let alpha = (255 * a + (*p >> 24) * (64 - a)) / 64;
                        *p = alpha << 24 | ch(16, g.rgb[0]) | ch(8, g.rgb[1]) | ch(0, g.rgb[2]);
                    }
                }
            }
        }
    }

    /// Draws a device-font glyph from its typeface's outline: em units (1024
    /// per em, y down from the baseline) scaled to `g.em` pixels at the
    /// BIOS's pen, clipped like the BIOS's glyph, the colour blended by
    /// coverage (4 x 4 samples per canvas pixel, even-odd).
    fn draw_outline(&mut self, g: &Glyph, o: &crate::swf_font::Outline) {
        let s = self.scale as f32;
        let k = g.em / 1024.0;
        let to_canvas = |(x, y): (f32, f32)| ((g.pen.0 as f32 + x * k) * s, (g.pen.1 as f32 + y * k) * s);
        let mut segs: Vec<[f32; 4]> = vec![];
        for p in &o.pieces {
            let [a, c, b] = p.map(to_canvas);
            let n = 16;
            let mut prev = a;
            for i in 1..=n {
                let t = i as f32 / n as f32;
                let u = 1.0 - t;
                let q = (u * u * a.0 + 2.0 * u * t * c.0 + t * t * b.0, u * u * a.1 + 2.0 * u * t * c.1 + t * t * b.1);
                segs.push([prev.0, prev.1, q.0, q.1]);
                prev = q;
            }
        }
        if segs.is_empty() {
            return;
        }
        let [cx0, cy0, cx1, cy1] = g.clip;
        let lim = |lo: i32, hi: i32| ((lo.max(0) as f32) * s, ((hi.min(159) + 1) as f32) * s);
        let ((lx0, lx1), (ly0, ly1)) = (lim(cx0, cx1), lim(cy0, cy1));
        let (mut bx0, mut bx1, mut by0, mut by1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for sg in &segs {
            bx0 = bx0.min(sg[0]).min(sg[2]);
            bx1 = bx1.max(sg[0]).max(sg[2]);
            by0 = by0.min(sg[1]).min(sg[3]);
            by1 = by1.max(sg[1]).max(sg[3]);
        }
        let (x0, x1) = (bx0.max(lx0).floor().max(0.0) as usize, (bx1.min(lx1).ceil() as usize).min(self.size));
        let (y0, y1) = (by0.max(ly0).floor().max(0.0) as usize, (by1.min(ly1).ceil() as usize).min(self.size));
        const N: usize = 4;
        let mut xs = vec![];
        for py in y0..y1 {
            let mut cov = vec![0u32; x1.saturating_sub(x0)];
            for j in 0..N {
                let sy = py as f32 + (j as f32 + 0.5) / N as f32;
                xs.clear();
                for sg in &segs {
                    if (sg[1] <= sy) != (sg[3] <= sy) {
                        xs.push(sg[0] + (sy - sg[1]) / (sg[3] - sg[1]) * (sg[2] - sg[0]));
                    }
                }
                xs.sort_by(f32::total_cmp);
                for pair in xs.chunks(2) {
                    let [a, b] = [pair[0], *pair.get(1).unwrap_or(&pair[0])];
                    for (i, c) in cov.iter_mut().enumerate() {
                        let px = (x0 + i) as f32;
                        for q in 0..N {
                            let sx = px + (q as f32 + 0.5) / N as f32;
                            if sx >= a && sx < b {
                                *c += 1;
                            }
                        }
                    }
                }
            }
            for (i, &c) in cov.iter().enumerate() {
                if c == 0 {
                    continue;
                }
                let a = c * 64 / (N * N) as u32;
                let p = &mut self.pixels[py * self.size + x0 + i];
                let ch = |k: u32, v: u8| ((v as u32 * a + ((*p >> k) & 0xFF) * (64 - a)) / 64) << k;
                let alpha = (255 * a + (*p >> 24) * (64 - a)) / 64;
                *p = alpha << 24 | ch(16, g.rgb[0]) | ch(8, g.rgb[1]) | ch(0, g.rgb[2]);
            }
        }
        // The LCD pixels the glyph covers are drawn now, from the outline:
        // the outline's and the BIOS glyph's boxes (they differ by a pixel
        // here and there), within the clip.
        let sc = self.scale;
        let (gx0, gy0) = ((g.x.max(cx0).max(0) as usize).min(160), (g.y.max(cy0).max(0) as usize).min(160));
        let (gx1, gy1) =
            (((g.x + g.width as i32).min(cx1 + 1).max(0) as usize).min(160), ((g.y + g.height as i32).min(cy1 + 1).max(0) as usize).min(160));
        for y in (y0 / sc).min(gy0)..y1.div_ceil(sc).min(160).max(gy1) {
            for x in (x0 / sc).min(gx0)..x1.div_ceil(sc).min(160).max(gx1) {
                self.covered[y * 160 + x] = true;
                self.from_outline[y * 160 + x] = true;
            }
        }
    }

    /// Updates `mask` against the BIOS's frame `lcd` (160x160): a block is
    /// taken from the fallback where the canvas does not know it, where it
    /// differs from its LCD pixel by more than `AGREE` beyond its
    /// neighbourhood or by more than `FAR`, or where the LCD changed with no
    /// render drawing there (until a render does). Returns the number of
    /// blocks taken from the fallback.
    pub fn update_mask(&mut self, lcd: &[u32]) -> usize {
        let (s, size) = (self.scale, self.size);
        // Up to 4x4 samples per block (enough to judge it, quick at any size).
        let step = (s / 4).max(1);
        let mut fallback = 0;
        for by in 0..160 {
            for bx in 0..160 {
                let i = by * 160 + bx;
                let l = lcd[i];
                if self.covered[i] {
                    self.stale[i] = false;
                } else if self.prev_lcd.get(i).is_some_and(|&p| p != l) {
                    self.stale[i] = true;
                }
                let lc = [(l >> 16) & 0xFF, (l >> 8) & 0xFF, l & 0xFF].map(|c| c as i32);
                // The block as shown (unknown parts from the LCD pixel).
                let (mut sum, mut n, mut known) = ([0i32; 3], 0i32, 0i32);
                for y in (by * s..(by + 1) * s).step_by(step) {
                    for x in (bx * s..(bx + 1) * s).step_by(step) {
                        let p = self.pixels[y * size + x];
                        let a = (p >> 24) as i32;
                        let rgb = [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF].map(|c| c as i32);
                        for k in 0..3 {
                            sum[k] += rgb[k] + lc[k] * (255 - a) / 255;
                        }
                        known += a;
                        n += 1;
                    }
                }
                let avg = sum.map(|v| v / n);
                let (mut lo, mut hi) = ([255i32; 3], [0i32; 3]);
                for ny in by.saturating_sub(1)..(by + 2).min(160) {
                    for nx in bx.saturating_sub(1)..(bx + 2).min(160) {
                        let q = lcd[ny * 160 + nx];
                        for (k, c) in [(q >> 16) & 0xFF, (q >> 8) & 0xFF, q & 0xFF].into_iter().enumerate() {
                            lo[k] = lo[k].min(c as i32);
                            hi[k] = hi[k].max(c as i32);
                        }
                    }
                }
                let far = (0..3).any(|k| (avg[k] - lc[k]).abs() > FAR);
                let off = (0..3).any(|k| avg[k] < lo[k] - AGREE || avg[k] > hi[k] + AGREE);
                let unknown = known < n * 128;
                let use_fallback = self.stale[i] || (!self.from_outline[i] && (far || off)) || unknown;
                self.mask[i] = if use_fallback { 255 } else { 0 };
                fallback += use_fallback as usize;
            }
        }
        self.prev_lcd.clear();
        self.prev_lcd.extend_from_slice(&lcd[..160 * 160]);
        self.covered.fill(false);
        fallback
    }

    /// The picture as shown, on the CPU (for saved pictures and tests):
    /// canvas where known and unmasked, else the LCD's own pixels (no
    /// smoothing), as the GPU shows it.
    pub fn composed(&self, lcd: &[u32]) -> Vec<u32> {
        let (s, size) = (self.scale as f32, self.size);
        let lcd_at = |x: f32, y: f32| -> [f32; 3] {
            let p = lcd[((y / s) as usize).min(159) * 160 + ((x / s) as usize).min(159)];
            [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF].map(|c| c as f32)
        };
        let mask_at = |x: f32, y: f32| -> f32 { self.mask[((y / s) as usize).min(159) * 160 + ((x / s) as usize).min(159)] as f32 / 255.0 };
        let mut out = vec![0u32; size * size];
        for y in 0..size {
            for x in 0..size {
                let p = self.pixels[y * size + x];
                let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
                let keep = 1.0 - mask_at(cx, cy);
                let a = (p >> 24) as f32 / 255.0 * keep;
                let hd = [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF].map(|c| c as f32 * keep);
                let f = lcd_at(cx, cy);
                let c = |k: usize| (hd[k] + f[k] * (1.0 - a)).round().clamp(0.0, 255.0) as u32;
                out[y * size + x] = 0xFF00_0000 | c(0) << 16 | c(1) << 8 | c(2);
            }
        }
        out
    }
}

/// What one render's rows need.
struct Raster<'a> {
    segs: &'a [Seg],
    paints: &'a [Paint],
    x0: usize,
    x1: usize,
    /// Canvas pixels per LCD pixel.
    scale: f32,
}

impl Raster<'_> {
    /// Rasterizes canvas rows `first..` (`chunk`, `size` pixels each), with
    /// each row's segments in `lists`.
    fn rows(&self, chunk: &mut [u32], lists: &[Vec<u32>], first: usize, size: usize) {
        let (x0, x1) = (self.x0, self.x1);
        let w = x1 - x0;
        let mut acc = vec![[0f32; 3]; w];
        let mut unknown_w = vec![0f32; w];
        let mut keep_w = vec![0f32; w];
        let mut xs: Vec<(f32, u32)> = vec![];
        let mut active = vec![false; self.paints.len()];
        // Per fill: parity (rules 0 and 1) or winding count (rule 2).
        let mut count = vec![0i32; self.paints.len()];
        for (r, list) in lists.iter().enumerate() {
            let y = first + r;
            acc.iter_mut().for_each(|a| *a = [0.0; 3]);
            unknown_w.iter_mut().for_each(|a| *a = 0.0);
            keep_w.iter_mut().for_each(|a| *a = 0.0);
            for j in 0..SUB {
                let sy = y as f32 + (j as f32 + 0.5) / SUB as f32;
                xs.clear();
                for &i in list {
                    let s = self.segs[i as usize];
                    let (lo, hi) = if s.y0 < s.y1 { ((s.x0, s.y0), (s.x1, s.y1)) } else { ((s.x1, s.y1), (s.x0, s.y0)) };
                    if sy >= lo.1 && sy < hi.1 {
                        xs.push((lo.0 + (sy - lo.1) / (hi.1 - lo.1) * (hi.0 - lo.0), i));
                    }
                }
                xs.sort_by(|a, b| a.0.total_cmp(&b.0));
                active.iter_mut().for_each(|a| *a = false);
                count.iter_mut().for_each(|c| *c = 0);
                let mut from = x0 as f32;
                let mut look = composite(&active, self.paints);
                let mut sink = Sink { acc: &mut acc, unknown_w: &mut unknown_w, keep_w: &mut keep_w, x0, sy, scale: self.scale };
                for &(x, i) in &xs {
                    let to = x.min(x1 as f32);
                    if to > from {
                        sink.span((from, to), &look, self.paints);
                    }
                    let s = &self.segs[i as usize];
                    let mut flip = |f: i32| {
                        if f >= 0 {
                            count[f as usize] ^= 1;
                            active[f as usize] = count[f as usize] != 0;
                        }
                    };
                    match s.rule {
                        1 => flip(s.fills[0]),
                        2 => {
                            if s.fills[0] >= 0 {
                                let f = s.fills[0] as usize;
                                count[f] += s.dir as i32;
                                active[f] = count[f] != 0;
                            }
                        }
                        _ => {
                            flip(s.fills[0]);
                            flip(s.fills[1]);
                        }
                    }
                    look = composite(&active, self.paints);
                    from = from.max(x);
                }
                if (x1 as f32) > from {
                    sink.span((from, x1 as f32), &look, self.paints);
                }
            }
            let row = &mut chunk[r * size..(r + 1) * size];
            for i in 0..w {
                let o = row[x0 + i];
                let oa = (o >> 24) as f32;
                let oc = [(o >> 16) & 0xFF, (o >> 8) & 0xFF, o & 0xFF].map(|c| c as f32);
                let keep = keep_w[i] / SUB as f32;
                // Premultiplied: drawn colour, plus what was kept; alpha is
                // the known share (drawn fills, plus the known part kept).
                let c = |k: usize| ((acc[i][k] * 255.0 / SUB as f32) + keep * oc[k]).round().clamp(0.0, 255.0) as u32;
                let drawn = 1.0 - (unknown_w[i] + keep_w[i]) / SUB as f32;
                let a = (drawn * 255.0 + keep * oa).round().clamp(0.0, 255.0) as u32;
                row[x0 + i] = a << 24 | c(0) << 16 | c(1) << 8 | c(2);
            }
        }
    }
}

/// A sub-row's accumulators.
struct Sink<'a> {
    acc: &'a mut [[f32; 3]],
    unknown_w: &'a mut [f32],
    keep_w: &'a mut [f32],
    x0: usize,
    sy: f32,
    /// Canvas pixels per LCD pixel, for bitmap sampling.
    scale: f32,
}

/// The look where the fills in `active` cover.
fn composite(active: &[bool], paints: &[Paint]) -> Look {
    let (mut c, mut rest) = ([0f32; 3], 1f32);
    let mut stack = vec![];
    let mut varying = false;
    for i in (0..paints.len()).rev() {
        if !active[i] {
            continue;
        }
        stack.push(i);
        match &paints[i] {
            Paint::Unknown => break,
            Paint::Bitmap(_) => varying = true,
            Paint::Solid(p) => {
                for k in 0..3 {
                    c[k] += rest * p[3] * p[k];
                }
                rest *= 1.0 - p[3];
                if p[3] >= 1.0 {
                    rest = 0.0;
                    break;
                }
            }
        }
    }
    let on_unknown = stack.last().is_some_and(|&i| matches!(paints[i], Paint::Unknown));
    if varying { Look::Varying(stack) } else { Look::Uniform(c, rest, on_unknown) }
}

/// A bitmap fill's colour at LCD position (px, py), premultiplied, 0..1:
/// bilinear, which smooths bitmaps the LCD shows blocky (the BIOS samples
/// the nearest texel unless the fill asks for smoothing).
fn sample(b: &BitmapFill, px: f32, py: f32) -> [f32; 4] {
    let m = b.matrix;
    let (u, v) = (m[0] * px + m[2] * py + m[4] - 0.5, m[1] * px + m[3] * py + m[5] - 0.5);
    let (w, h) = (b.image.width as i64, b.image.height as i64);
    let at = |tx: i64, ty: i64| {
        let (tx, ty) = if b.clamp { (tx.clamp(0, w - 1), ty.clamp(0, h - 1)) } else { (tx.rem_euclid(w), ty.rem_euclid(h)) };
        let p = b.image.pixels[(ty * w + tx) as usize];
        [(p >> 16) & 0xFF, (p >> 8) & 0xFF, p & 0xFF, p >> 24].map(|c| c as f32 / 255.0)
    };
    let (fu, fv) = (u.floor(), v.floor());
    let (tx, ty, ax, ay) = (fu as i64, fv as i64, u - fu, v - fv);
    let (p00, p10, p01, p11) = (at(tx, ty), at(tx + 1, ty), at(tx, ty + 1), at(tx + 1, ty + 1));
    std::array::from_fn(|k| (p00[k] * (1.0 - ax) + p10[k] * ax) * (1.0 - ay) + (p01[k] * (1.0 - ax) + p11[k] * ax) * ay)
}

impl Sink<'_> {
    /// Adds the sub-row's span [a, b) (canvas pixels) of `look`.
    fn span(&mut self, (a, b): (f32, f32), look: &Look, paints: &[Paint]) {
        let x0 = self.x0;
        let (first, last) = (a.floor() as usize, (b.ceil() as usize).min(x0 + self.acc.len()));
        for x in first.max(x0)..last {
            let cov = (b.min(x as f32 + 1.0) - a.max(x as f32)).max(0.0);
            let i = x - x0;
            let (c, rest, on_unknown) = match look {
                Look::Uniform(c, rest, on_unknown) => (*c, *rest, *on_unknown),
                Look::Varying(stack) => {
                    let (mut c, mut rest, mut on_unknown) = ([0f32; 3], 1f32, false);
                    for &f in stack {
                        match &paints[f] {
                            Paint::Unknown => {
                                on_unknown = true;
                                break;
                            }
                            Paint::Solid(p) => {
                                for k in 0..3 {
                                    c[k] += rest * p[3] * p[k];
                                }
                                rest *= 1.0 - p[3];
                            }
                            Paint::Bitmap(bf) => {
                                let s = self.scale.max(1.0);
                                let p = sample(bf, (x as f32 + 0.5) / s, self.sy / s);
                                for k in 0..3 {
                                    c[k] += rest * p[k];
                                }
                                rest *= 1.0 - p[3];
                            }
                        }
                    }
                    (c, rest, on_unknown)
                }
            };
            for (a, c) in self.acc[i].iter_mut().zip(c) {
                *a += c * cov;
            }
            if on_unknown {
                self.unknown_w[i] += rest * cov;
            } else {
                self.keep_w[i] += rest * cov;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_hooks_by_signature_at_any_even_offset() {
        let mut bios = vec![0u8; 0x400];
        bios[0x100..0x100 + 44].copy_from_slice(&EDGE_CTOR_SIG);
        bios[0x202..0x202 + 20].copy_from_slice(&SCANLINE_SIG);
        bios[0x300..0x300 + 24].copy_from_slice(&SOLID_PAINTER_SIG);
        bios[0x382..0x382 + 32].copy_from_slice(&BITMAP_PAINTER_SIG);
        assert_eq!(
            Hooks::find(&bios, 0x4000_0000),
            Some(Hooks {
                edge_ctor: 0x4000_0100,
                scanline: 0x4000_0202,
                solid_painter: 0x4000_0300,
                bitmap_painter: 0x4000_0350,
                glyph: None
            })
        );
        assert_eq!(Hooks::find(&bios[..0x200], 0x4000_0000), None);
    }

    #[test]
    fn records_edges_fills_and_the_first_rectangle_per_pass() {
        let mut mem = vec![0u8; 0x200];
        let put = |m: &mut Vec<u8>, a: usize, v: &[u8]| m[a..a + v.len()].copy_from_slice(v);
        // Edge record at 0x00 with fill 0x80; fill colour B=0x10 G=0x20 R=0x30 A=0xFF.
        put(&mut mem, 0x08, &0x80u32.to_le_bytes());
        put(&mut mem, 0x10, &[4, 0, 8, 0, 12, 0, 16, 0, 20, 0, 24, 0, 1]);
        put(&mut mem, 0x80 + 0x0C, &0x000A_0002u32.to_le_bytes());
        put(&mut mem, 0x80 + 0x24, &[0x10, 0, 0x20, 0, 0x30, 0, 0xFF, 0]);
        // Renderer at 0x100 with rectangle 40..106 x 208..256.
        put(&mut mem, 0x100, &2i32.to_le_bytes());
        for (i, v) in [40i32, 106, 208, 256].iter().enumerate() {
            put(&mut mem, 0x128 + 4 * i, &v.to_le_bytes());
        }
        let peek = |a: u32| mem[a as usize];
        let mut c = HdCapture::new(Hooks { edge_ctor: 0, scanline: 0, solid_painter: 0, bitmap_painter: 1, glyph: None });
        c.edge(0, peek); // built before its render's first scanline event
        c.scanline(0x100, 1, peek);
        c.scanline(0x100, 2, peek); // later scanlines of the same render
        c.end_pass();
        c.end_pass(); // nothing drawn: no empty pass
        let passes = c.take();
        assert_eq!(passes.len(), 1);
        let p = &passes[0];
        assert_eq!((p.rect, p.scale, p.rows), (Some([40, 106, 208, 256]), 2, Some((0, 2))));
        assert_eq!(p.edges[0].p, [(4, 8), (12, 16), (20, 24)]);
        assert!(p.edges[0].line);
        assert_eq!(
            p.fills[&0x80],
            Fill { order: 0x000A_0002, rgba: [0x30, 0x20, 0x10, 0xFF], painter: 0, bitmap: None }
        );
    }
}
