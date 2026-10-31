//! The 3D engine (GA3D, MagicEyes GRP3D) at 0xC001_A000; user space maps it
//! through /dev/ga3d and drives it with the closed libopengles_lite.so.
//! Everything here is from the POLLUX data book (Release 0.91, chapter 22)
//! unless marked GUESS; docs/didj.md ("3D engine") has the details.
//!
//! The command processor reads 32-bit words from the queue in memory
//! (CMDQSTART 0x04 .. CMDQEND 0x08, physical) from the rear pointer (0x10,
//! the engine's) up to the front pointer (0x0C, software's) and writes them
//! into the register file ("Register3D"). The top four bits of a command's
//! first word pick it: 15 = register fill (count bits 16-24 + 1 words to
//! register bits 0-10, word-indexed), 4-13 = an "optimized" command that
//! writes CMDSIZE words to register CMDID * 2 and runs that sub-module.
//! Writing GRP3D_CONTROL (0x20) starts a sub-module: the primitive
//! processor (vertex streams -> GTE or TSE), the GTE (vertex program), the
//! TSE (triangle/rectangle setup) or the rasterizer (memory fill, LUT fill,
//! texture cache clear). Everything finishes at once (timing UNKNOWN).

use crate::hd::Hd;
use crate::mlc::{BLOCK_BASE, BLOCK_END, block_sdram};
use crate::texup::{Texel, UP, Upscaled};
use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

/// Pixels (the triangles' bounding boxes) from which a batch is drawn on
/// several threads.
const PARALLEL_PIXELS: i64 = 2048;
/// Rows of the screen each thread takes at a time.
const BAND: i32 = 8;
/// The registers the pixel pipeline reads (up to ZSCALE), copied with each
/// triangle.
const PIX_REGS: usize = ZSCALE + 1;
/// SDRAM tiles of the written-memory map: 64 bytes x 32 lines, 2 KB in both
/// layouts (`block_sdram`).
const TILE_SHIFT: usize = 11;

/// The threads drawing batches: up to 4 (Clone Wars' gameplay on an i9
/// P-core: 95% of real speed on one thread, 150% with 4, for a fifth more
/// CPU time), one fewer than the PC has; none on a PC with 2 or fewer.
/// Their own pool: rayon's global one spins 16 or more idle threads.
fn pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::LazyLock<Option<rayon::ThreadPool>> = std::sync::LazyLock::new(|| {
        let n = std::thread::available_parallelism().map_or(1, |n| n.get()).saturating_sub(1).min(4);
        (n >= 2).then(|| rayon::ThreadPoolBuilder::new().num_threads(n).thread_name(|i| format!("ga3d-{i}")).build().ok()).flatten()
    });
    POOL.as_ref()
}

/// The register window (0xC001_A000 .. 0xC001_C000).
pub const BASE: u32 = 0xC001_A000;
pub const END: u32 = 0xC001_C000;

const CPCONTROL: usize = 0x00 / 4;
const CMDQSTART: usize = 0x04 / 4;
const CMDQEND: usize = 0x08 / 4;
const CMDQFRONT: usize = 0x0C / 4;
const CMDQREAR: usize = 0x10 / 4;
const STATUS: usize = 0x14 / 4;
const CONTROL: usize = 0x20 / 4;
const RENDERSTATE: usize = 0x28 / 4;
const ALPHABLEND: usize = 0x30 / 4;
const TEXSUBSEGMENT: usize = 0x38 / 4;
const TPCOLOR: usize = 0x40 / 4;
const LUTPARAM: usize = 0x48 / 4;
const TEXINFO: [usize; 2] = [0x50 / 4, 0x58 / 4];
const TEXBLEND: [usize; 2] = [0x60 / 4, 0x68 / 4];
const ALPHATEST: usize = 0x88 / 4;
const TEXCONST: usize = 0x8C / 4;
const MFILLPARAM0: usize = 0xA0 / 4;
const RENDTRG0: usize = 0xB0 / 4;
const ZBINFO: usize = 0xC0 / 4;
const ZSCALE: usize = 0xC8 / 4;
const VPORTBOT: usize = 0xF0 / 4;
const PRIMCON1: usize = 0x100 / 4;
const IDXSTART: usize = 0x104 / 4;
const IDXEND: usize = 0x108 / 4;
const PPARAMLOOPCTRL: usize = 0x10C / 4;
const PPARAM: usize = 0x110 / 4;
const VS: usize = 0x120 / 4;
/// TSE input vertices: 16 words each (X Y Z W A R G B U0 V0 U1 V1).
const TSEINPUT: usize = 0x500 / 4;
/// GTE output (clipper input) vertices, 16 words each (GUESS layout:
/// position, colour, texture coordinates, 4 words apiece).
const GTEOUT: usize = 0x700 / 4;
const GTEPROG: usize = 0x800 / 4;
/// The GTE's input/constant vectors (4 words each).
const GTECONST: usize = 0x1000 / 4;

/// Every sub-module idle, both FIFO flags as after reset (GRP3D_STATUS).
const IDLE: u32 = 0x00BF_0000;

/// Drawing time, in cycles of the 3D engine's clock (GUESS: the bus clock,
/// PLL1's 147.456 MHz). Ribbit draws at once and reports the engine busy
/// (the command reader's rear held back, STATUS not idle) until that time
/// has passed, so software waits for it as on a Didj. The data book gives
/// no fill rate: these are GUESSES calibrated on a real Didj's "Select a
/// name" (filmed at 120 fps: a new picture every second refresh, 30 per
/// second, while Ribbit drew 60 with the engine free) and Clone Wars
/// gameplay (a YouTube recording: ~20 new pictures per second while
/// scrolling). 22 cycles a pixel gives 33 and 18-20 fps; 26 gave 33 and 17.
pub const GA3D_HZ: u64 = 147_456_000;
pub const CYCLES_PER_PIXEL: u64 = 22;
const CYCLES_PER_TRIANGLE: u64 = 64;
const CYCLES_PER_FILL_PIXEL: u64 = 1;

/// Words an optimized command writes, by CMDID (Table 22-7; 4..=13).
const CMDSIZE: [usize; 16] = [0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 1, 1, 0, 0];

/// How many register writes the trace keeps.
const TRACE_MAX: usize = 200_000;

/// GTE output modifiers (Table 22-22): the component each output takes,
/// 4-7 for a negated one. The data book's table is printed one row late
/// (0 is xyzw); the menu's colour swizzle (rgba -> argb with 3) confirms it.
const SWIZZLE: [[u8; 4]; 32] = {
    const P: [&[u8; 4]; 28] = [
        b"xyzw", b"yzwx", b"zwxy", b"wxyz", b"xywz", b"ywzx", b"zxyw", b"wzxy", b"xzyw", b"ywxz", b"zywx", b"wxzy",
        b"xwyz", b"yzxw", b"zxwy", b"wyzx", b"xzwy", b"yxzw", b"zwyx", b"wyxz", b"xwzy", b"yxwz", b"zyxw", b"wzyx",
        b"xxxx", b"yyyy", b"zzzz", b"wwww",
    ];
    let mut t = [[0u8; 4]; 32];
    let mut i = 0;
    while i < 32 {
        let mut c = 0;
        while c < 4 {
            t[i][c] = if i < 28 {
                match P[i][c] {
                    b'x' => 0,
                    b'y' => 1,
                    b'z' => 2,
                    _ => 3,
                }
            } else {
                4 + (i - 28) as u8
            };
            c += 1;
        }
        i += 1;
    }
    t
};

/// Physical memory as the engine sees it: SDRAM from 0, and block
/// addresses (`mlc::block_sdram`) from 0x2000_0000.
pub struct Mem<'a> {
    pub sdram: &'a mut [u8],
}

impl Mem<'_> {
    fn offset(a: u32, texture: bool) -> usize {
        if (BLOCK_BASE..BLOCK_END).contains(&a) { block_sdram(a, texture) } else { a as usize }
    }

    /// `size` bytes (within one 4-byte cell) at `a`, little-endian.
    fn read(&self, a: u32, size: u32) -> u32 {
        self.read_as(a, size, false)
    }

    fn read_as(&self, a: u32, size: u32, texture: bool) -> u32 {
        let o = Self::offset(a, texture);
        (0..size as usize).fold(0, |v, i| v | (self.sdram.get(o + i).copied().unwrap_or(0) as u32) << (8 * i))
    }

    fn write16(&mut self, a: u32, v: u16, texture: bool) {
        let o = Self::offset(a, texture);
        if let Some(b) = self.sdram.get_mut(o..o + 2) {
            b.copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// The SDRAM as the pixel pipeline sees it, shared by the threads drawing a
/// triangle's rows: relaxed byte atomics, plain loads and stores on x86. A
/// triangle's pixels are at different addresses, so the result doesn't
/// depend on how its rows are split between threads.
#[derive(Clone, Copy)]
struct Shared<'a>(&'a [AtomicU8]);

impl<'a> Shared<'a> {
    fn new(mem: &'a mut Mem) -> Self {
        // SAFETY: AtomicU8 has u8's size, alignment and bit validity, and
        // the borrow is exclusive while the view lives.
        Shared(unsafe { &*(&raw mut *mem.sdram as *const [AtomicU8]) })
    }

    fn read_as(self, a: u32, size: u32, texture: bool) -> u32 {
        let o = Mem::offset(a, texture);
        (0..size as usize).fold(0, |v, i| v | (self.0.get(o + i).map_or(0, |b| b.load(Ordering::Relaxed)) as u32) << (8 * i))
    }

    fn write16(self, a: u32, v: u16, texture: bool) {
        let o = Mem::offset(a, texture);
        if let Some(b) = self.0.get(o..o + 2) {
            b[0].store(v as u8, Ordering::Relaxed);
            b[1].store((v >> 8) as u8, Ordering::Relaxed);
        }
    }
}

/// Where the pixel pipeline reads and writes a pixel's colour and depth
/// (console address `a`): the console's memory, or an HD pixel of it.
trait Dest {
    fn read(&self, a: u32, texture: bool, mem: Shared) -> u32;
    fn write(&self, a: u32, v: u16, texture: bool, mem: Shared);
}

/// The console's memory, the HD store told what changes in it.
struct Native<'a>(Option<&'a Hd>);

impl Dest for Native<'_> {
    fn read(&self, a: u32, texture: bool, mem: Shared) -> u32 {
        mem.read_as(a, 2, texture)
    }

    fn write(&self, a: u32, v: u16, texture: bool, mem: Shared) {
        if let Some(hd) = self.0 {
            hd.note(a, mem.read_as(a, 2, texture) as u16, v);
        }
        mem.write16(a, v, texture);
    }
}

/// HD pixel (j, k) of each console pixel.
struct Sub<'a> {
    hd: &'a Hd,
    j: usize,
    k: usize,
}

impl Dest for Sub<'_> {
    fn read(&self, a: u32, texture: bool, mem: Shared) -> u32 {
        self.hd.get(a, self.j, self.k, mem.read_as(a, 2, texture) as u16) as u32
    }

    fn write(&self, a: u32, v: u16, texture: bool, mem: Shared) {
        self.hd.set(a, self.j, self.k, mem.read_as(a, 2, texture) as u16, v);
    }
}

/// A 24-bit float register (the low 8 bits of an IEEE single are dropped).
fn f24(bits: u32) -> f32 {
    f32::from_bits(bits & 0xFFFF_FF00)
}

/// A screen-space vertex for the rasterizer: x, y, z (0..1), q = 1/w, then
/// A R G B (0..255) and U0 V0 U1 V1 (texels).
type Vertex = [f32; 12];

/// The block address of a 16-bit pixel at (x, y) in a sub-segment, through
/// the address converting tables (2 bits per 256 pixels/lines).
fn pixel_addr(sub: u32, tblx: u32, tbly: u32, x: u32, y: u32) -> u32 {
    let xm = (tblx >> (2 * (x >> 8 & 3)) & 3) << 8 | (x & 0xFF);
    let ym = (tbly >> (2 * (y >> 8 & 3)) & 3) << 8 | (y & 0xFF);
    sub_addr(sub, 2 * xm, ym)
}

/// A byte at (u, v) of a sub-segment (2048 x 1024 bytes; 8 to a segment,
/// two across).
fn sub_addr(sub: u32, u: u32, v: u32) -> u32 {
    let (seg, n) = (sub >> 3 & 0xFF, sub & 7);
    BLOCK_BASE | seg << 24 | ((n >> 1) * 1024 + v) << 12 | ((n & 1) * 2048 + u)
}

/// A byte at (u, v) of a texture page (1024 x 512 bytes; 32 to a segment,
/// in 2 x 2 groups that make the sub-segments).
fn page_addr(seg: u32, page: u32, u: u32, v: u32) -> u32 {
    let pu = (page & 1) * 1024 + (page >> 2 & 1) * 2048;
    let pv = (page >> 1 & 1) * 512 + (page >> 3 & 3) * 1024;
    BLOCK_BASE | (seg & 0xFF) << 24 | ((pv + (v & 511)) & 0xFFF) << 12 | ((pu + (u & 1023)) & 0xFFF)
}

/// `i / (N - 1)` for each `i` (the same division, done at compile time).
const fn units<const N: usize>() -> [f32; N] {
    let mut t = [0.0; N];
    let mut i = 0;
    while i < N {
        t[i] = i as f32 / (N - 1) as f32;
        i += 1;
    }
    t
}

static UNIT1: [f32; 2] = units();
static UNIT4: [f32; 16] = units();
static UNIT5: [f32; 32] = units();
static UNIT6: [f32; 64] = units();
static UNIT8: [f32; 256] = units();

/// A `bits`-bit channel `x` as 0..1: `x / (2^bits - 1)`, by table.
fn unit(x: u32, bits: u32) -> f32 {
    let x = x as usize;
    match bits {
        1 => UNIT1[x & 1],
        4 => UNIT4[x & 15],
        5 => UNIT5[x & 31],
        6 => UNIT6[x & 63],
        _ => UNIT8[x & 255],
    }
}

/// A 16-bit colour of format `kind` (the low 3 bits of a colour mode) as
/// R G B A, 0..1.
fn decode16(kind: u32, v: u32) -> [f32; 4] {
    let n = unit;
    match kind {
        0 => [n(v >> 11, 5), n(v >> 5 & 63, 6), n(v & 31, 5), 1.0],
        1 => [n(v >> 12, 4), n(v >> 8 & 15, 4), n(v >> 4 & 15, 4), n(v & 15, 4)],
        2 => [n(v >> 11, 5), n(v >> 6 & 31, 5), n(v >> 1 & 31, 5), n(v & 1, 1)],
        3 => [n(v & 255, 8), n(v & 255, 8), n(v & 255, 8), n(v >> 8 & 255, 8)],
        4 => [n(v >> 8 & 15, 4), n(v >> 4 & 15, 4), n(v & 15, 4), n(v >> 12, 4)],
        5 => [n(v >> 10 & 31, 5), n(v >> 5 & 31, 5), n(v & 31, 5), n(v >> 15, 1)],
        _ => [n(v >> 8 & 255, 8), n(v >> 8 & 255, 8), n(v >> 8 & 255, 8), n(v & 255, 8)],
    }
}

// x86-64's baseline (no SSE4.1) makes `round` and `floor` library calls,
// a tenth of a game's time in the pixel pipeline. These are exact for every
// input, saturation and NaN included: a float minus its truncation is exact.

/// `x.round() as u32`.
fn round_u32(x: f32) -> u32 {
    let t = x as u32;
    if x - t as f32 >= 0.5 { t.saturating_add(1) } else { t }
}

/// `x.floor() as i32`.
fn floor_i32(x: f32) -> i32 {
    let t = x as i32;
    if t as f32 > x { t.saturating_sub(1) } else { t }
}

/// The depth and alpha test functions (NEVER, LESS, EQUAL, LESSEQUAL,
/// GREATER, NOTEQUAL, GREATEREQUAL, ALWAYS).
fn test(func: u32, new: u32, old: u32) -> bool {
    match func & 7 {
        0 => false,
        1 => new < old,
        2 => new == old,
        3 => new <= old,
        4 => new > old,
        5 => new != old,
        6 => new >= old,
        _ => true,
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Ga3d {
    pub regs: Vec<u32>,
    /// GTE working registers.
    work: [[f32; 4]; 16],
    /// GTE output vertices: position, colour (A R G B), texture coordinates.
    out: [[[f32; 4]; 3]; 3],
    /// The GTE's source pages (SETPAGE; reset 0 and 1).
    page: [u32; 2],
    /// The reciprocal unit's output (SETRCP/GETRCP).
    rcp: f32,
    /// The multiplier's last output, which MAD adds to (GUESS: MUL's).
    mul: [f32; 4],
    /// The colour lookup tables, one per texture stage (R5G6B5).
    #[serde(with = "lut_serde")]
    lut: [[u16; 256]; 2],
    /// Register writes from the command queue, oldest first: (offset, value).
    #[serde(skip)]
    pub trace: Vec<(u32, u32)>,
    /// Sub-module starts (GRP3D_CONTROL values) by kind, for the summary.
    #[serde(skip)]
    pub starts: std::collections::BTreeMap<u32, u64>,
    /// Invalid commands seen (CMDID below 4).
    #[serde(skip)]
    pub invalid: u64,
    /// Triangles that reached the rasterizer.
    #[serde(skip)]
    pub triangles: u64,
    /// Pixels: (reached the pipeline, failed the depth test, dropped by
    /// the transparency colour, failed the alpha test, written).
    #[serde(skip)]
    pub pixels: [u64; 5],
    /// Frame skipping: triangles and rectangles aren't drawn (the GTE still
    /// runs, fills still happen). See `Bus::skip_frames`.
    #[serde(skip)]
    pub skip: bool,
    /// Triangles set up and not drawn yet, in order (drawn before the CPU
    /// goes on: everything still finishes at once).
    #[serde(skip)]
    batch: Vec<Tri>,
    /// SDRAM tiles the batch may write (a bit per `TILE_SHIFT` tile).
    #[serde(skip)]
    written: Vec<u64>,
    /// Per texture stage: the texture (TEXINFO pair, sub-segment) last
    /// found clear of `written`, while no tile has been added.
    #[serde(skip)]
    clear: [Option<(u32, u32, u32)>; 2],
    /// Drawing time: engine cycles of work given and done so far, the
    /// queue's rear as shown and, per queue run still drawing, the rear it
    /// reached and the work total it ends at.
    #[serde(default)]
    cost: u64,
    #[serde(default)]
    done: u64,
    #[serde(default)]
    shown_rear: u32,
    #[serde(default)]
    drawing: std::collections::VecDeque<(u32, u64)>,
    /// CPU cycles left over from the last tick (a fraction of an engine
    /// cycle, times the CPU clock).
    #[serde(default)]
    tick_rest: u64,
    /// Engine cycles per pixel drawn (CYCLES_PER_PIXEL; didjboot sets it to
    /// calibrate).
    #[serde(default = "default_cycles_per_pixel")]
    pub cycles_per_pixel: u64,
    /// HD drawing (`Hd::set_scale`).
    #[serde(skip)]
    pub hd: Hd,
    /// HD textures (`texup`): upscaled textures by (stage, TEXINFO pair,
    /// sub-segment, transparency colour), each with the `epoch` it was last
    /// checked in, the hash of the memory (`Raster::raw_print`) and of the
    /// texels it was made from.
    #[serde(skip)]
    up_cache: std::collections::HashMap<(usize, u32, u32, u32, u32), (u64, u64, u64, Arc<Upscaled>)>,
    /// Bumped whenever texture memory may have changed (the CPU ran, the
    /// engine drew or filled).
    #[serde(skip)]
    epoch: u64,
    /// HD pixels read upscaled textures (with HD drawing on).
    #[serde(skip)]
    pub hd_textures: bool,
    /// Text the font module drew lately (`fonthd`): textures holding it
    /// are drawn again from its outlines.
    #[serde(skip)]
    pub text: crate::fonthd::TextStore,
}

/// The largest texture upscaled (texels), and the cache's size (bytes).
const UP_TEXELS: i32 = 512 * 512;
const UP_CACHE_BYTES: usize = 256 << 20;

fn default_cycles_per_pixel() -> u64 {
    CYCLES_PER_PIXEL
}

/// A triangle set up for drawing: its vertices, edges (b - a, a and the
/// top-left rule's ownership), bounding box, the texture coordinates in use
/// and its own copy of the pixel pipeline's registers.
struct Tri {
    v: [Vertex; 3],
    area: f32,
    edges: [(f32, f32, f32, f32, bool); 3],
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
    /// The HD pixels' box (in HD pixels; empty when HD is off).
    hd: [i32; 4],
    /// Per stage, the texture upscaled for the HD pixels, and the texels
    /// drawn (`Ga3d::up_bounds`).
    up: [Option<Arc<Upscaled>>; 2],
    bounds: [[i32; 4]; 2],
    /// Added to the HD pixels' texture coordinates (U0 V0 U1 V1): a texture
    /// drawn 1:1 has each console pixel's texel centred on it.
    snap: [f32; 4],
    uvs: usize,
    st: State,
    regs: [u32; PIX_REGS],
}

impl Tri {
    /// Draws row `y` (in the bounding box); the pixel counts.
    fn row(&self, lut: &[[u16; 256]; 2], y: i32, mem: Shared, hd: Option<&Hd>) -> [u64; 5] {
        let r = Raster { regs: &self.regs, lut, up: [None; 2] };
        let mut counts = [0; 5];
        let rows = self.rows(y as f32);
        for x in self.x0..=self.x1 {
            if let Some(p) = self.cover(&rows, x as f32) {
                counts[0] += 1;
                counts[r.pixel(&self.st, x as u32, y as u32, &p, mem, &Native(hd))] += 1;
            }
        }
        counts
    }

    /// Draws HD row `hy`: HD pixel (j, k) of a console pixel is at
    /// (x - 0.5 + (j + 0.5) / scale, y - 0.5 + (k + 0.5) / scale).
    fn row_hd(&self, lut: &[[u16; 256]; 2], hy: i32, mem: Shared, hd: &Hd) {
        let r = Raster { regs: &self.regs, lut, up: std::array::from_fn(|s| self.up[s].as_deref().map(|u| (u, self.bounds[s]))) };
        let s = hd.scale as i32;
        let at = |h: i32| (h as f32 + 0.5) / s as f32 - 0.5;
        let rows = self.rows(at(hy));
        let k = hy.rem_euclid(s) as usize;
        for hx in self.hd[0]..=self.hd[2] {
            if let Some(mut p) = self.cover(&rows, at(hx)) {
                for i in 8..12 {
                    p[i] += self.snap[i - 8];
                }
                let d = Sub { hd, j: hx.rem_euclid(s) as usize, k };
                r.pixel(&self.st, hx.div_euclid(s) as u32, hy.div_euclid(s) as u32, &p, mem, &d);
            }
        }
    }

    /// `snap` for a texture drawn 1:1 (a texel per pixel across and down,
    /// or turned a quarter): moves each stage's coordinates so the texel the
    /// first covered console pixel takes is centred on that pixel.
    fn snap(&self) -> [f32; 4] {
        let mut snap = [0.0; 4];
        if self.st.perspective {
            return snap;
        }
        let v = &self.v;
        // A coordinate's change per pixel across and down.
        let grad = |i: usize| {
            let (d1, d2) = (v[1][i] - v[0][i], v[2][i] - v[0][i]);
            let dx = (d1 * (v[2][1] - v[0][1]) - d2 * (v[1][1] - v[0][1])) / self.area;
            let dy = ((v[1][0] - v[0][0]) * d2 - (v[2][0] - v[0][0]) * d1) / self.area;
            (dx, dy)
        };
        let near = |a: f32, b: f32| (a - b).abs() < 1e-3;
        let unit = |(dx, dy): (f32, f32)| near(dx.abs(), 1.0) && near(dy, 0.0) || near(dx, 0.0) && near(dy.abs(), 1.0);
        let p = (self.y0..=self.y1).find_map(|y| {
            let rows = self.rows(y as f32);
            (self.x0..=self.x1).find_map(|x| self.cover(&rows, x as f32))
        });
        if let Some(p) = p {
            for i in 8..self.uvs {
                if unit(grad(i)) {
                    snap[i - 8] = p[i].floor() + 0.5 - p[i];
                }
            }
        }
        snap
    }

    /// The edge functions' row terms at `fy`.
    fn rows(&self, fy: f32) -> [f32; 3] {
        self.edges.map(|(dx, _, _, ay, _)| dx * (fy - ay))
    }

    /// The interpolated vertex at (fx, the row's y), if it's inside.
    fn cover(&self, rows: &[f32; 3], fx: f32) -> Option<Vertex> {
        let (v, area) = (&self.v, self.area);
        let mut w = [0.0f32; 3];
        for (k, &(_, dy, ax, _, own)) in self.edges.iter().enumerate() {
            let e = (rows[k] - dy * (fx - ax)) / area;
            if e < 0.0 || e == 0.0 && !own {
                return None;
            }
            w[k] = e;
        }
        let lin = |i: usize| w[0] * v[0][i] + w[1] * v[1][i] + w[2] * v[2][i];
        let mut p = [0.0; 12];
        for i in 2..8 {
            p[i] = lin(i);
        }
        if self.st.perspective {
            let q = p[3];
            for i in 8..self.uvs {
                p[i] = (w[0] * v[0][i] * v[0][3] + w[1] * v[1][i] * v[1][3] + w[2] * v[2][i] * v[2][3]) / q;
            }
        } else {
            for i in 8..self.uvs {
                p[i] = lin(i);
            }
        }
        Some(p)
    }
}

/// What the pixel pipeline reads: a triangle's registers and the colour
/// lookup tables (a LUT fill draws the batch first).
struct Raster<'a> {
    regs: &'a [u32; PIX_REGS],
    lut: &'a [[u16; 256]; 2],
    /// Per stage, the texture upscaled (HD pixels only) and the texels
    /// drawn (`up_bounds`).
    up: [Option<(&'a Upscaled, [i32; 4])>; 2],
}

/// A texture stage's texture, from its registers.
struct Tex {
    i0: u32,
    mode: u32,
    bpp: u32,
    width: i32,
    height: i32,
    sub: u32,
    page: u32,
    ou: u32,
    ov: u32,
}

fn add(a: [u64; 5], b: [u64; 5]) -> [u64; 5] {
    std::array::from_fn(|k| a[k] + b[k])
}

impl Default for Ga3d {
    fn default() -> Self {
        Self {
            regs: vec![0; ((END - BASE) / 4) as usize],
            work: [[0.0; 4]; 16],
            out: [[[0.0; 4]; 3]; 3],
            page: [0, 1],
            rcp: 0.0,
            mul: [0.0; 4],
            lut: [[0; 256]; 2],
            trace: Vec::new(),
            starts: Default::default(),
            invalid: 0,
            triangles: 0,
            pixels: Default::default(),
            skip: false,
            batch: Vec::new(),
            written: Vec::new(),
            clear: [None; 2],
            cost: 0,
            done: 0,
            shown_rear: 0,
            drawing: Default::default(),
            tick_rest: 0,
            cycles_per_pixel: CYCLES_PER_PIXEL,
            hd: Hd::default(),
            up_cache: Default::default(),
            epoch: 0,
            hd_textures: true,
            text: Default::default(),
        }
    }
}

impl Ga3d {
    /// Still drawing what the command queue gave it.
    fn busy(&self) -> bool {
        !self.drawing.is_empty()
    }

    /// `cycles` of a CPU at `cpu_hz` have passed: the engine draws on.
    pub fn tick(&mut self, cycles: u64, cpu_hz: u64) {
        if self.done >= self.cost {
            self.done = self.cost;
            self.tick_rest = 0;
            return;
        }
        let t = cycles as u128 * GA3D_HZ as u128 + self.tick_rest as u128;
        self.done = (self.done + (t / cpu_hz as u128) as u64).min(self.cost);
        self.tick_rest = (t % cpu_hz as u128) as u64;
        while let Some(&(rear, end)) = self.drawing.front() {
            if end > self.done {
                break;
            }
            self.shown_rear = rear;
            self.drawing.pop_front();
        }
    }

    pub fn read(&self, addr: u32) -> u32 {
        let i = ((addr - BASE) / 4) as usize;
        match i {
            // Drawing: the command reader, processor, primitive unit and
            // rasterizer busy, the rasterizer's FIFO not empty.
            STATUS if self.busy() => IDLE & !0x00BC_0000,
            STATUS => IDLE,
            CMDQREAR if self.busy() => self.shown_rear,
            _ => self.regs[i],
        }
    }

    /// A CPU write.
    pub fn write(&mut self, addr: u32, v: u32, mem: &mut Mem) {
        self.epoch += 1;
        let i = ((addr - BASE) / 4) as usize;
        if i <= CMDQREAR && self.trace.len() < TRACE_MAX {
            // CPU writes to the queue registers, marked by bit 16.
            self.trace.push(((i * 4) as u32 | 0x1_0000, v));
        }
        match i {
            CMDQREAR | STATUS => {}
            CPCONTROL => {
                self.regs[i] = v;
                if v & 1 != 0 {
                    self.regs[CMDQREAR] = self.regs[CMDQSTART];
                    self.drawing.clear();
                }
            }
            CMDQSTART => {
                // The reader starts at the queue's start (GUESS: the data
                // book only says the rear "is initialized by the Command
                // Reader"; libopengles_lite writes CPCONTROL after this).
                self.regs[i] = v;
                self.regs[CMDQREAR] = v;
                self.drawing.clear();
            }
            CMDQFRONT => {
                self.regs[i] = v;
                let rear = self.regs[CMDQREAR];
                let cost = self.cost;
                self.run_queue(mem);
                if self.cost != cost {
                    if !self.busy() {
                        self.shown_rear = rear;
                    }
                    self.drawing.push_back((self.regs[CMDQREAR], self.cost));
                }
            }
            _ => self.set(i, v, mem),
        }
        self.flush(mem);
    }

    /// Reads commands until the rear pointer meets the front one.
    fn run_queue(&mut self, mem: &mut Mem) {
        let (start, end) = (self.regs[CMDQSTART], self.regs[CMDQEND]);
        let mut rear = self.regs[CMDQREAR];
        let front = self.regs[CMDQFRONT];
        let next = |rear: &mut u32, mem: &Mem| {
            let w = mem.read(*rear, 4);
            // Queue mode wraps after CMDQEND, the last word (0x0131FFFC
            // for a queue at 0x01300000).
            *rear = if *rear >= end { start } else { *rear + 4 };
            w
        };
        let mut guard = 0u32;
        while rear != front && guard < 1 << 22 {
            guard += 1;
            let head = next(&mut rear, mem);
            let id = head >> 28;
            let (reg, count) = match id {
                15 => ((head & 0x7FF) as usize, ((head >> 16 & 0x1FF) + 1) as usize),
                4..=13 => (id as usize * 2, CMDSIZE[id as usize]),
                // NOP (14): one word, nothing written.
                14 => continue,
                _ => {
                    self.invalid += 1;
                    if self.trace.len() < TRACE_MAX {
                        self.trace.push((0xFFFF, head));
                    }
                    continue;
                }
            };
            // An optimized command's head is its first data word (those
            // registers keep the CMDID in their top bits).
            let mut first = Some(head).filter(|_| id != 15);
            for k in 0..count {
                let w = match first.take() {
                    Some(w) => w,
                    None if rear == front => break,
                    None => next(&mut rear, mem),
                };
                self.set(reg + k, w, mem);
            }
            if id == 9 {
                self.lut_fill(mem);
            }
        }
        self.regs[CMDQREAR] = rear;
    }

    /// A Register3D write (from the CPU or the command parser).
    fn set(&mut self, i: usize, v: u32, mem: &mut Mem) {
        if i >= self.regs.len() {
            return;
        }
        // A NOP control only waits for idle: not worth tracing.
        if self.trace.len() < TRACE_MAX && !(i == CONTROL && v & 4 != 0) {
            self.trace.push(((i * 4) as u32, v));
        }
        self.regs[i] = v;
        if i == CONTROL && v & 4 == 0 {
            *self.starts.entry(v & 0x7FF).or_default() += 1;
            self.control(v, mem);
        }
    }

    /// GRP3D_CONTROL: bits 0-1 the sub-module (0 primitive processor, 1 GTE,
    /// 2 TSE, 3 rasterizer), 3-4 the rasterizer's mode, 6-7 the GTE's output
    /// vertex, 8 inverse culling, 10 the TSE's triangle (1) or rectangle mode.
    fn control(&mut self, v: u32, mem: &mut Mem) {
        match v & 3 {
            0 => self.primitives(mem),
            // GUESS: bit 9 (the data book repeats CULLMODE) is the
            // rendering flag.
            1 => self.gte(v >> 6 & 3, v >> 8 & 1 != 0, v >> 9 & 1 != 0, mem),
            2 => {
                let tri = v >> 10 & 1 != 0;
                self.tse(tri, v >> 8 & 1 != 0, mem);
            }
            _ => match v >> 3 & 3 {
                1 => self.memory_fill(mem),
                2 => self.lut_fill(mem),
                // 0 renders (UNKNOWN from here: nothing seen); 3 clears the
                // texture cache, which isn't modelled.
                _ => {}
            },
        }
    }

    /// The primitive processor: fetches each vertex (by index) from the
    /// vertex streams into the GTE's input registers (or the TSE's) and
    /// runs the GTE (or TSE) with the next primitive control parameter.
    fn primitives(&mut self, mem: &mut Mem) {
        let con = self.regs[PRIMCON1];
        let (use_gte, use_palette, use_idx) = (con & 1 != 0, con >> 1 & 1 != 0, con >> 2 & 1 != 0);
        let idx_size = [1, 2, 3, 4][(con >> 6 & 3) as usize];
        let valid = con >> 8 & 0xFF;
        let (end_param, loop_param) = (self.regs[PPARAMLOOPCTRL] & 0xF, self.regs[PPARAMLOOPCTRL] >> 8 & 0xF);
        let (mut cur, last) = (self.regs[IDXSTART], self.regs[IDXEND]);
        let mut param = 0;
        let mut guard = 0;
        while cur <= last && guard < 1 << 20 {
            guard += 1;
            let p = self.regs[PPARAM + param as usize / 4] >> (8 * (param % 4)) & 0x1F;
            param = if param == end_param { loop_param } else { param + 1 };
            let index = if use_idx {
                let n = mem.read(cur, idx_size.min(4));
                cur += idx_size;
                n
            } else {
                cur += 1;
                cur - 1
            };
            let (vnum, render, invcull, rect) = (p & 3, p >> 2 & 1 != 0, p >> 3 & 1 != 0, p >> 4 & 1 != 0);
            self.fetch(index, valid, use_gte, use_palette, vnum, mem);
            if use_gte {
                self.gte(vnum, invcull, render, mem);
            } else if render || rect {
                // TSE mode: runs at the rendering (or rectangle) step.
                self.tse(!rect, invcull, mem);
            }
        }
    }

    /// Copies vertex `index` from the valid streams (GRP3D_VSADDRn,
    /// VSSTRIDEn, VSPARAMn: destination bits 20-31 in words, count - 1 bits
    /// 8-19, type bits 0-7) into the GTE's (or TSE vertex `vnum`'s) inputs.
    fn fetch(&mut self, index: u32, valid: u32, gte: bool, palette: bool, vnum: u32, mem: &mut Mem) {
        let read = |mem: &Mem, a: u32, ty: u32| -> (f32, u32) {
            match ty {
                0 => (mem.read(a, 1) as f32 / 255.0, 1),
                1 => (mem.read(a, 1) as f32, 1),
                2 => (mem.read(a, 1) as u8 as i8 as f32, 1),
                3 => (mem.read(a, 2) as u16 as i16 as f32, 2),
                4 => (mem.read(a, 4) as i32 as f32, 4),
                5 => (mem.read(a, 4) as i32 as f32 / 65536.0, 4),
                _ => (f32::from_bits(mem.read(a, 4)), 4),
            }
        };
        let mut matrices = Vec::new();
        for s in 0..8 {
            if valid >> s & 1 == 0 || palette && s == 7 {
                continue;
            }
            let (addr, stride, param) = (self.regs[VS + 3 * s], self.regs[VS + 3 * s + 1], self.regs[VS + 3 * s + 2]);
            let (dst, n, ty) = ((param >> 20) as usize, (param >> 8 & 0xFFF) + 1, param & 0xFF);
            let mut a = addr.wrapping_add(stride.wrapping_mul(index));
            for k in 0..n as usize {
                if palette && s == 0 {
                    matrices.push(mem.read(a, 1));
                    a += 1;
                    continue;
                }
                let (f, size) = read(mem, a, ty);
                a += size;
                let reg = if gte { dst + k } else { TSEINPUT + vnum as usize * 16 + (dst % 16) + k };
                if let Some(r) = self.regs.get_mut(reg) {
                    *r = f.to_bits();
                }
            }
        }
        if palette && valid >> 7 & 1 != 0 {
            let (addr, stride, param) = (self.regs[VS + 21], self.regs[VS + 22], self.regs[VS + 23]);
            let (mut dst, n, ty) = ((param >> 20) as usize, (param >> 8 & 0xFFF) + 1, param & 0xFF);
            for m in matrices {
                let mut a = addr.wrapping_add(stride.wrapping_mul(m));
                for _ in 0..n {
                    let (f, size) = read(mem, a, ty);
                    a += size;
                    if let Some(r) = self.regs.get_mut(dst) {
                        *r = f.to_bits();
                    }
                    dst += 1;
                }
            }
        }
    }

    fn input(&self, v: u32) -> [f32; 4] {
        let b = GTECONST + 4 * v as usize;
        // Whole (one load, not four stores the arithmetic must wait for).
        match self.regs.get(b..b + 4) {
            Some(r) => std::array::from_fn(|c| f24(r[c])),
            None => std::array::from_fn(|c| self.regs.get(b + c).map_or(0.0, |&r| f24(r))),
        }
    }

    /// Runs the GTE program (from instruction 0) for output vertex `vnum`.
    /// Instructions: bits 27-31 the ID; arithmetic ones have the destination
    /// at 22-26 (bit 26 set: output vertex bits 24-25, component 22-23;
    /// clear: working register 22-25), source 0 at 17-21 and source 1 at
    /// 12-16 (bit 4 of each: an input/constant vector, paged by SETPAGE),
    /// the output modifier at 7-11, the write mask (x = bit 3) at 3-6 and a
    /// delay at 0-2. Pipeline timing isn't modelled (results are ready at
    /// once).
    fn gte(&mut self, vnum: u32, invcull_in: bool, render_in: bool, mem: &mut Mem) {
        let mut loops = [(0usize, 0u32); 3];
        let mut pc = 0usize;
        let mut steps = 0;
        while pc < 512 && steps < 8192 {
            steps += 1;
            let w = self.regs[GTEPROG + pc];
            pc += 1;
            let id = w >> 27;
            match id {
                0 | 25 => {}
                1 => break,
                2 => loops[(w >> 24 & 3).min(2) as usize] = (pc, w & 0xFF_FFFF),
                3 => {
                    let l = &mut loops[(w >> 24 & 3).min(2) as usize];
                    if l.1 > 0 {
                        l.1 -= 1;
                        pc = l.0;
                    }
                }
                4 | 5 => {
                    // INVCULL (bits 0-1): GUESS 0/1 a fixed flag, 2 the
                    // input flag, 3 its inverse (Figure 22-18).
                    let inv = match w & 3 {
                        0 => false,
                        1 => true,
                        2 => invcull_in,
                        _ => !invcull_in,
                    };
                    if (id == 5 || render_in) && !self.skip {
                        self.clip_and_draw(inv, mem);
                    }
                }
                // SETPAGE: source page 1 in bits 8-11, page 0 in 0-3 (the
                // menu's 0x100 and 0x200 only make sense this way round).
                31 => self.page = [w & 15, w >> 8 & 15],
                6..=24 => self.arith(w, vnum),
                _ => {}
            }
        }
    }

    fn arith(&mut self, w: u32, vnum: u32) {
        let id = w >> 27;
        let src = |this: &Self, sel: u32, page: u32| -> [f32; 4] {
            if sel & 16 != 0 { this.input(page * 16 + (sel & 15)) } else { this.work[(sel & 15) as usize] }
        };
        let a = src(self, w >> 17 & 31, self.page[0]);
        let b = src(self, w >> 12 & 31, self.page[1]);
        let modifier = SWIZZLE[(w >> 7 & 31) as usize];
        let dot = |n: usize| (0..n).map(|c| a[c] * b[c]).sum::<f32>();
        fn pairs(a: [f32; 4], b: [f32; 4], f: impl Fn(f32, f32) -> f32) -> [f32; 4] {
            std::array::from_fn(|c| f(a[c], b[c]))
        }
        let each = |f: fn(f32, f32) -> f32| pairs(a, b, f);
        let r: [f32; 4] = match id {
            6 => [1.0 / a[3].abs().sqrt(); 4],
            7 => [dot(4); 4],
            8 => [dot(3); 4],
            9 => [dot(4).max(0.0); 4],
            10 => [dot(3).max(0.0); 4],
            11 => std::array::from_fn(|c| self.mul[c] + b[c]),
            12 => each(|x, y| x + y),
            13 => each(|x, y| x - y),
            14 => each(|x, y| if x >= y { 1.0 } else { 0.0 }),
            15 => each(|x, y| if x < y { 1.0 } else { 0.0 }),
            16 => each(|x, y| if x >= y { x } else { y }),
            17 => each(|x, y| if x < y { x } else { y }),
            18 => {
                let m = each(|x, y| x * y);
                self.mul = m;
                m
            }
            19 => {
                // SETRCP: the first component through the modifier.
                let c = modifier[0];
                let x = if c < 4 { a[c as usize] } else { -a[(c - 4) as usize] };
                self.rcp = 1.0 / x;
                return;
            }
            20 => a,
            21 => b,
            22 => [self.rcp; 4],
            23 => [1.0; 4],
            _ => [0.0; 4],
        };
        let r: [f32; 4] = std::array::from_fn(|c| {
            let m = modifier[c];
            let v = if m < 4 { r[m as usize] } else { -r[(m - 4) as usize] };
            f24(v.to_bits())
        });
        let mask = w >> 3 & 15;
        if w >> 26 & 1 != 0 {
            let ov = match w >> 24 & 3 {
                3 => vnum,
                n => n,
            } as usize;
            let comp = (w >> 22 & 3).min(2) as usize;
            for c in 0..4 {
                if mask >> c & 1 != 0 && ov < 3 {
                    self.out[ov][comp][c] = r[c];
                    self.regs[GTEOUT + ov * 16 + comp * 4 + c] = r[c].to_bits();
                }
            }
        } else {
            let d = (w >> 22 & 15) as usize;
            for c in 0..4 {
                if mask >> c & 1 != 0 {
                    self.work[d][c] = r[c];
                }
            }
        }
    }

    /// The clipper: clips the GTE's triangle to 0 <= x, y, z <= w, maps it
    /// to the viewport (Xs = left + W * Xc / Wc, Ys = bottom - H * Yc / Wc)
    /// and hands the pieces (a fan) to the TSE.
    fn clip_and_draw(&mut self, invcull: bool, mem: &mut Mem) {
        let clipv = |o: &[[f32; 4]; 3]| -> [f32; 12] {
            let mut v = [0.0; 12];
            v[..4].copy_from_slice(&o[0]);
            v[4..8].copy_from_slice(&o[1]);
            v[8..].copy_from_slice(&o[2]);
            v
        };
        // At most 3 + 6 corners (one more per plane); no allocations, as
        // this runs for every triangle.
        const MAX: usize = 9;
        let mut poly = [[0.0f32; 12]; MAX];
        for (p, o) in poly.iter_mut().zip(&self.out) {
            *p = clipv(o);
        }
        let mut len = 3;
        // Planes as (component, sign): d = w - c for upper ones, c for 0 <= c.
        for plane in 0..6 {
            let (c, upper) = (plane / 2, plane % 2 == 1);
            let d = |v: &[f32; 12]| if upper { v[3] - v[c] } else { v[c] };
            let mut next = [[0.0f32; 12]; MAX];
            let mut n = 0;
            for k in 0..len {
                let (p, q) = (poly[k], poly[(k + 1) % len]);
                let (dp, dq) = (d(&p), d(&q));
                if dp >= 0.0 {
                    next[n] = p;
                    n += 1;
                }
                if (dp >= 0.0) != (dq >= 0.0) {
                    let t = dp / (dp - dq);
                    next[n] = std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t);
                    n += 1;
                }
            }
            (poly, len) = (next, n);
            if len < 3 {
                return;
            }
        }
        let f = |i: usize| f24(self.regs[VPORTBOT + i]);
        let (bottom, height, left, width) = (f(0), f(1), f(2), f(3));
        let mut screen = [[0.0f32; 12]; MAX];
        for (s, v) in screen.iter_mut().zip(&poly[..len]) {
            let q = 1.0 / v[3];
            s[0] = left + width * v[0] * q;
            s[1] = bottom - height * v[1] * q;
            s[2] = v[2] * q;
            s[3] = q;
            s[4..].copy_from_slice(&v[4..]);
        }
        for k in 1..len - 1 {
            self.triangle(&[screen[0], screen[k], screen[k + 1]], invcull, mem);
        }
    }

    /// The TSE run directly or from the primitive processor in TSE mode:
    /// its input registers already hold screen-space vertices.
    fn tse(&mut self, triangle: bool, invcull: bool, mem: &mut Mem) {
        if self.skip {
            return;
        }
        let v: [Vertex; 3] = std::array::from_fn(|n| std::array::from_fn(|c| f24(self.regs[TSEINPUT + n * 16 + c])));
        if triangle {
            self.triangle(&v, invcull, mem);
        } else {
            self.rectangle(&v[0], &v[1], mem);
        }
    }

    /// A rectangle from vertex 0's corner to vertex 1's, texture
    /// coordinates spread between them, the rest from vertex 1 (GUESS).
    fn rectangle(&mut self, a: &Vertex, b: &Vertex, mem: &mut Mem) {
        let (x0, x1) = (a[0].min(b[0]), a[0].max(b[0]));
        let (y0, y1) = (a[1].min(b[1]), a[1].max(b[1]));
        self.cost += CYCLES_PER_TRIANGLE + ((x1 - x0) * (y1 - y0)).clamp(0.0, 1e7) as u64 * self.cycles_per_pixel;
        self.flush(mem);
        let st = self.state();
        let regs: [u32; PIX_REGS] = self.regs[..PIX_REGS].try_into().unwrap();
        let up = self.hd_up(mem).unwrap_or_default();
        let bounds: [[i32; 4]; 2] = std::array::from_fn(|s| self.up_bounds(s, &[[a[8 + 2 * s], a[9 + 2 * s]], [b[8 + 2 * s], b[9 + 2 * s]]]));
        let r = Raster { regs: &regs, lut: &self.lut, up: [None; 2] };
        let hd = self.hd.on().then_some(&self.hd);
        let mem = Shared::new(mem);
        let at = |fx: f32, fy: f32| {
            if fx < x0 || fx >= x1 || fy < y0 || fy >= y1 {
                return None;
            }
            let (tx, ty) = ((fx - a[0]) / (b[0] - a[0]), (fy - a[1]) / (b[1] - a[1]));
            let mut p = *b;
            for k in [8, 10] {
                p[k] = a[k] + (b[k] - a[k]) * tx;
                p[k + 1] = a[k + 1] + (b[k + 1] - a[k + 1]) * ty;
            }
            Some(p)
        };
        let mut counts = [0; 5];
        for y in st.clip[1]..=st.clip[3] {
            for x in st.clip[0]..=st.clip[2] {
                if let Some(p) = at(x as f32, y as f32) {
                    counts[0] += 1;
                    counts[r.pixel(&st, x as u32, y as u32, &p, mem, &Native(hd))] += 1;
                }
            }
        }
        if let Some(hd) = hd {
            let r = Raster { up: std::array::from_fn(|s| up[s].as_deref().map(|u| (u, bounds[s]))), ..r };
            // A texture drawn 1:1: each console pixel's texel centred on it.
            let mut snap = [0.0; 4];
            if up.iter().any(Option::is_some)
                && let Some(p) = (st.clip[1]..=st.clip[3]).flat_map(|y| (st.clip[0]..=st.clip[2]).map(move |x| (x, y))).find_map(|(x, y)| at(x as f32, y as f32))
            {
                for k in 0..4 {
                    let scale = (b[8 + k] - a[8 + k]) / (b[k & 1] - a[k & 1]);
                    if (scale.abs() - 1.0).abs() < 1e-3 {
                        snap[k] = p[8 + k].floor() + 0.5 - p[8 + k];
                    }
                }
            }
            let s = hd.scale as i32;
            let f = |h: i32| (h as f32 + 0.5) / s as f32 - 0.5;
            for hy in st.clip[1] * s..st.clip[3] * s + s {
                let fy = f(hy);
                if fy < y0 - 1.0 || fy >= y1 + 1.0 {
                    continue;
                }
                for hx in st.clip[0] * s..st.clip[2] * s + s {
                    if let Some(mut p) = at(f(hx), fy) {
                        for k in 0..4 {
                            p[8 + k] += snap[k];
                        }
                        let d = Sub { hd, j: hx.rem_euclid(s) as usize, k: hy.rem_euclid(s) as usize };
                        r.pixel(&st, hx.div_euclid(s) as u32, hy.div_euclid(s) as u32, &p, mem, &d);
                    }
                }
            }
        }
        self.count(counts);
    }

    /// The textures upscaled for the HD pixels of what's drawn next, when
    /// HD textures are on and it's textured and sampled nearest.
    fn hd_up(&mut self, mem: &mut Mem) -> Option<[Option<Arc<Upscaled>>; 2]> {
        let rs = self.regs[RENDERSTATE];
        if !self.hd.on() || !self.hd_textures || rs >> 1 & 1 == 0 || rs >> 5 & 1 != 0 {
            return None;
        }
        let stages = if rs >> 2 & 1 != 0 { 2 } else { 1 };
        Some(std::array::from_fn(|s| if s < stages { self.upscaled(s, mem) } else { None }))
    }

    /// The texels (first and last, across and down) the vertices' texture
    /// coordinates `uv` span in stage `s`'s texture, or all of them (as
    /// far as can be) when they span its whole width or height.
    fn up_bounds(&self, s: usize, uv: &[[f32; 2]]) -> [i32; 4] {
        let regs: &[u32; PIX_REGS] = self.regs[..PIX_REGS].try_into().unwrap();
        let t = Raster { regs, lut: &self.lut, up: [None; 2] }.tex(s);
        let span = |k: usize, size: i32| {
            let lo = uv.iter().map(|c| c[k]).fold(f32::INFINITY, f32::min);
            let hi = uv.iter().map(|c| c[k]).fold(f32::NEG_INFINITY, f32::max);
            let (first, last) = (floor_i32(lo + 1e-3), floor_i32(hi - 1e-3));
            if !(last - first < size - 1) { (-(1 << 20), 1 << 20) } else { (first, last) }
        };
        let ((u0, u1), (v0, v1)) = (span(0, t.width), span(1, t.height));
        [u0, v0, u1, v1]
    }

    /// Stage `s`'s texture upscaled: from the cache while the memory it's
    /// in is unchanged (checked once an epoch), or its texels as decoded.
    fn upscaled(&mut self, s: usize, mem: &mut Mem) -> Option<Arc<Upscaled>> {
        let regs: [u32; PIX_REGS] = self.regs[..PIX_REGS].try_into().unwrap();
        let tp = regs[RENDERSTATE] >> 3 & 1 != 0;
        let r = Raster { regs: &regs, lut: &self.lut, up: [None; 2] };
        let t = r.tex(s);
        if t.width * t.height > UP_TEXELS {
            return None;
        }
        let key = (s, t.i0, regs[TEXINFO[s] + 1], t.sub, if tp { 1 << 16 | regs[TPCOLOR] & 0xFFFF } else { 0 });
        let entry = self.up_cache.get_mut(&key);
        if let Some(e) = &entry
            && e.0 == self.epoch
        {
            return Some(e.3.clone());
        }
        let raw = r.raw_print(s, &t, mem.sdram);
        if let Some(e) = entry
            && e.1 == raw
        {
            e.0 = self.epoch;
            return Some(e.3.clone());
        }
        let mem = Shared::new(mem);
        let q = |c: f32| round_u32(c * 255.0);
        let colour: Vec<u32> = (0..t.height)
            .flat_map(|v| (0..t.width).map(move |u| (u, v)))
            .map(|(u, v)| match r.fetch(s, &t, u, v, tp, mem) {
                // Transparent texels are all alike.
                (key, _) if tp && key == regs[TPCOLOR] & 0xFFFF => 0,
                (_, c) => q(c[3]) << 24 | q(c[2]) << 16 | q(c[1]) << 8 | q(c[0]),
            })
            .collect();
        let hash = colour.iter().fold(0xCBF2_9CE4_8422_2325u64, |h, &c| (h ^ c as u64).wrapping_mul(0x0100_0000_01B3).rotate_left(5));
        if let Some(e) = self.up_cache.get_mut(&key)
            && e.2 == hash
        {
            (e.0, e.1) = (self.epoch, raw);
            return Some(e.3.clone());
        }
        let (w, h) = (t.width as usize, t.height as usize);
        let mut up = crate::texup::upscale(w, h, &colour);
        if let Some(map) = self.text.render(w, h, &colour, |x, y| up.colour(x, y, &colour)) {
            up = Upscaled::from_colours(w, h, map);
        }
        let up = Arc::new(up);
        if self.up_cache.values().map(|e| e.3.bytes()).sum::<usize>() + up.bytes() > UP_CACHE_BYTES {
            self.up_cache.clear();
        }
        self.up_cache.insert(key, (self.epoch, raw, hash, up.clone()));
        Some(up)
    }

    fn count(&mut self, counts: [u64; 5]) {
        for (total, n) in self.pixels.iter_mut().zip(counts) {
            *total += n;
        }
    }

    /// Rasterizes a screen-space triangle: pixel centres at whole
    /// coordinates (libopengles_lite puts the viewport's left at -0.5),
    /// top-left fill rule (GUESS), inside the render target's clipping
    /// rectangle.
    fn triangle(&mut self, v: &[Vertex; 3], invcull: bool, mem: &mut Mem) {
        let st = self.state();
        let area = (v[1][0] - v[0][0]) * (v[2][1] - v[0][1]) - (v[2][0] - v[0][0]) * (v[1][1] - v[0][1]);
        if area == 0.0 || !area.is_finite() {
            return;
        }
        // y grows downwards, so a positive area is clockwise on screen.
        if st.cull && (area > 0.0) == (st.cull_cw != invcull) {
            return;
        }
        self.triangles += 1;
        let min = |i: usize| v[0][i].min(v[1][i]).min(v[2][i]);
        let max = |i: usize| v[0][i].max(v[1][i]).max(v[2][i]);
        let x0 = (min(0).ceil() as i32).max(st.clip[0]);
        let x1 = (max(0).floor() as i32).min(st.clip[2]);
        let y0 = (min(1).ceil() as i32).max(st.clip[1]);
        let y1 = (max(1).floor() as i32).min(st.clip[3]);
        self.cost += CYCLES_PER_TRIANGLE;
        // HD pixels, at (h + 0.5) / scale - 0.5 in console pixels.
        let hd = if self.hd.on() {
            let s = self.hd.scale as f32;
            let lo = |c: f32| ((c + 0.5) * s - 0.5).ceil() as i32;
            let hi = |c: f32| ((c + 0.5) * s - 0.5).floor() as i32;
            let si = self.hd.scale as i32;
            [lo(min(0)).max(st.clip[0] * si), lo(min(1)).max(st.clip[1] * si), hi(max(0)).min(st.clip[2] * si + si - 1), hi(max(1)).min(st.clip[3] * si + si - 1)]
        } else {
            [0, 0, -1, -1]
        };
        if (x0 > x1 || y0 > y1) && (hd[0] > hd[2] || hd[1] > hd[3]) {
            return;
        }
        // The pixels covered: half the area, at most the clipped box.
        let boxed = ((x1 - x0 + 1) * (y1 - y0 + 1)) as f32;
        self.cost += (area.abs() / 2.0).min(boxed) as u64 * self.cycles_per_pixel;
        // Top-left rule: an edge owns its pixels if it's a top or left one.
        let owns = |a: &Vertex, b: &Vertex| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            if area > 0.0 { dy < 0.0 || dy == 0.0 && dx > 0.0 } else { dy > 0.0 || dy == 0.0 && dx < 0.0 }
        };
        // Edge k (from vertex a to b): (b - a) and a, for the edge function
        // (bx - ax) * (y - ay) - (by - ay) * (x - ax), its terms in that order.
        let edges = [(1, 2), (2, 0), (0, 1)].map(|(a, b)| (v[b][0] - v[a][0], v[b][1] - v[a][1], v[a][0], v[a][1], owns(&v[a], &v[b])));
        // The pixel pipeline reads z, q, the colour and the texture
        // coordinates of the stages in use, not x and y.
        let rs = self.regs[RENDERSTATE];
        let uvs = if rs >> 1 & 1 == 0 { 8 } else if rs >> 2 & 1 == 0 { 10 } else { 12 };
        let regs: [u32; PIX_REGS] = self.regs[..PIX_REGS].try_into().unwrap();
        let mut tri = Tri { v: *v, area, edges, x0, x1, y0, y1, hd, up: [None, None], bounds: [[0; 4]; 2], snap: [0.0; 4], uvs, st, regs };
        if let Some(up) = self.hd_up(mem) {
            tri.up = up;
            tri.bounds = std::array::from_fn(|s| self.up_bounds(s, &v.map(|v| [v[8 + 2 * s], v[9 + 2 * s]])));
            tri.snap = tri.snap();
        }
        // On one thread, at once.
        if pool().is_none() {
            self.batch.push(tri);
            return self.flush(mem);
        }
        // A texture the batch may have drawn into: the batch first.
        if self.reads_written(&regs, mem.sdram.len()) {
            self.flush(mem);
        }
        self.mark_written(&st, [x0, y0, x1, y1], mem.sdram.len());
        self.batch.push(tri);
    }

    /// Draws the batch: on several threads when it's big, each taking bands
    /// of rows and drawing every triangle's part in them in order, so each
    /// pixel sees the triangles in the order they came.
    fn flush(&mut self, mem: &mut Mem) {
        if self.batch.is_empty() {
            return;
        }
        self.epoch += 1;
        let batch = std::mem::take(&mut self.batch);
        let (lut, mem) = (&self.lut, Shared::new(mem));
        let hd = self.hd.on().then_some(&self.hd);
        let si = self.hd.scale as i32;
        // Each band draws its console rows, then their HD rows (each HD
        // pixel lies in its console pixel's row, so the band owns it).
        let band = |top: i32, bottom: i32| {
            let mut counts = [0; 5];
            for t in &batch {
                for y in t.y0.max(top)..=t.y1.min(bottom) {
                    counts = add(counts, t.row(lut, y, mem, hd));
                }
            }
            if let Some(hd) = hd {
                for t in &batch {
                    for hy in t.hd[1].max(top * si)..=t.hd[3].min(bottom * si + si - 1) {
                        t.row_hd(lut, hy, mem, hd);
                    }
                }
            }
            counts
        };
        let top = batch.iter().map(|t| t.y0.min(t.hd[1].div_euclid(si))).min().unwrap_or(0);
        let bottom = batch.iter().map(|t| t.y1.max(t.hd[3].div_euclid(si))).max().unwrap_or(0);
        let pixels: i64 = batch.iter().map(|t| (t.x1 - t.x0 + 1) as i64 * (t.y1 - t.y0 + 1) as i64).sum::<i64>() * (si * si) as i64;
        let counts = match pool() {
            Some(pool) if pixels >= PARALLEL_PIXELS => {
                use rayon::prelude::*;
                pool.install(|| (top / BAND..=bottom / BAND).into_par_iter().map(|b| band(b * BAND, b * BAND + BAND - 1)).reduce(|| [0; 5], add))
            }
            _ => band(top, bottom),
        };
        self.count(counts);
        self.batch = batch;
        self.batch.clear();
        self.written.fill(0);
        self.clear = [None; 2];
    }

    /// Marks the tiles of the render target and (when written) the Z
    /// buffer under a bounding box (left, top, right, bottom): a point in
    /// every 32-pixel column and 32-line row of tiles.
    fn mark_written(&mut self, st: &State, [x0, y0, x1, y1]: [i32; 4], len: usize) {
        self.written.resize(len.div_ceil(64 << TILE_SHIFT), 0);
        let steps = |a: i32, b: i32| (a..=b).filter(move |&i| i == a || i == b || i % 32 == 0);
        let mut added = false;
        for info in [Some(st.rt), (st.rs >> 9 & 1 != 0).then_some(st.zb)].into_iter().flatten() {
            for y in steps(y0, y1) {
                for x in steps(x0, x1) {
                    let tile = Mem::offset(pixel_addr(info >> 16 & 0x7FF, info & 0xFF, info >> 8 & 0xFF, x as u32, y as u32), false) >> TILE_SHIFT;
                    if let Some(w) = self.written.get_mut(tile / 64) {
                        added |= *w & 1 << (tile % 64) == 0;
                        *w |= 1 << (tile % 64);
                    }
                }
            }
        }
        if added {
            self.clear = [None; 2];
        }
    }

    /// Whether a texture the triangle reads may lie in a tile the batch
    /// writes: a point in every 32 bytes and 16 lines of each stage's
    /// texture (tiles are 64 x 32).
    fn reads_written(&mut self, regs: &[u32; PIX_REGS], len: usize) -> bool {
        let rs = regs[RENDERSTATE];
        if rs >> 1 & 1 == 0 || self.written.iter().all(|&w| w == 0) {
            return false;
        }
        let stages = if rs >> 2 & 1 != 0 { 2 } else { 1 };
        for s in 0..stages {
            let (i0, i1) = (regs[TEXINFO[s]], regs[TEXINFO[s] + 1]);
            let sub = regs[TEXSUBSEGMENT] >> (16 * (i0 >> 27 & 1)) & 0x7FF;
            if self.clear[s] == Some((i0, i1, sub)) {
                continue;
            }
            let page = (sub & 7) << 2 | (i0 >> 25 & 3);
            let (ou, ov) = ((i1 & 0xFF) * 4, (i1 >> 8 & 0xFF) * 2);
            let (bytes, lines) = (((i1 >> 16 & 0xFF) + 1) * 4, ((i1 >> 24) + 1) * 2);
            for v in (0..lines).step_by(16).chain([lines - 1]) {
                for u in (0..bytes).step_by(32).chain([bytes - 1]) {
                    let tile = Mem::offset(page_addr(sub >> 3, page, ou + u, ov + v), true) >> TILE_SHIFT;
                    if tile < len >> TILE_SHIFT && self.written.get(tile / 64).is_some_and(|w| w & 1 << (tile % 64) != 0) {
                        return true;
                    }
                }
            }
            self.clear[s] = Some((i0, i1, sub));
        }
        false
    }

    fn state(&self) -> State {
        let rs = self.regs[RENDERSTATE];
        let r = |i: usize| self.regs[i];
        let (rt1, rt2) = (r(RENDTRG0 + 1), r(RENDTRG0 + 2));
        State {
            rs,
            cull: rs >> 12 & 1 != 0,
            cull_cw: rs >> 13 & 1 != 0,
            perspective: rs & 3 == 3,
            clip: [(rt1 & 0x7FF) as i32, (rt1 >> 16 & 0x3FF) as i32, (rt2 & 0x7FF) as i32, (rt2 >> 16 & 0x3FF) as i32],
            rt: r(RENDTRG0),
            zb: r(ZBINFO),
            zscale: f24(r(ZSCALE)),
        }
    }

}

impl Raster<'_> {
    /// The pixel pipeline: depth test, texture blending, alpha test, alpha
    /// blending, writing (R5G6B5). Fog and dithering aren't modelled yet.
    /// The outcome, as an index into `pixels`.
    fn pixel(&self, st: &State, x: u32, y: u32, p: &Vertex, mem: Shared, d: &impl Dest) -> usize {
        let rs = st.rs;
        let zaddr = pixel_addr(st.zb >> 16 & 0x7FF, st.zb & 0xFF, st.zb >> 8 & 0xFF, x, y);
        let z = (p[2] * st.zscale).clamp(0.0, 65535.0) as u32;
        if rs >> 8 & 1 != 0 && !test(st.zb >> 28, z, d.read(zaddr, st.zb >> 27 & 1 != 0, mem)) {
            return 1;
        }
        let shade = [p[5] / 255.0, p[6] / 255.0, p[7] / 255.0, p[4] / 255.0].map(|c| c.clamp(0.0, 1.0));
        let mut colour = shade;
        if rs >> 1 & 1 != 0 {
            let stages = if rs >> 2 & 1 != 0 { 2 } else { 1 };
            let mut prev = shade;
            for s in 0..stages {
                let Some(t) = self.texel(s, p[8 + 2 * s], p[9 + 2 * s], rs >> 5 & 1 != 0, rs >> 3 & 1 != 0, mem) else {
                    return 2;
                };
                prev = self.blend_stage(s, t, shade, prev);
            }
            colour = prev;
        }
        if rs >> 14 & 1 != 0 {
            let at = self.regs[ALPHATEST];
            if !test(at, round_u32(colour[3] * 255.0), at >> 8 & 0xFF) {
                return 3;
            }
        }
        let addr = pixel_addr(st.rt >> 16 & 0x7FF, st.rt & 0xFF, st.rt >> 8 & 0xFF, x, y);
        if rs >> 7 & 1 != 0 {
            let dst = decode16(0, d.read(addr, st.rt >> 27 & 1 != 0, mem));
            let ab = self.regs[ALPHABLEND];
            let src = colour;
            let factor = |code: u32, c: usize| -> f32 {
                let f = match code & 15 {
                    1 => 0.0,
                    2 => src[c],
                    4 => src[3],
                    8 => dst[c],
                    _ => 1.0,
                };
                if code & 16 != 0 { 1.0 - f } else { f }
            };
            for c in 0..3 {
                colour[c] = (src[c] * factor(ab & 31, c) + dst[c] * factor(ab >> 5 & 31, c)).clamp(0.0, 1.0);
            }
        }
        let q = |c: f32, bits: u32| round_u32(c * ((1 << bits) - 1) as f32);
        d.write(addr, (q(colour[0], 5) << 11 | q(colour[1], 6) << 5 | q(colour[2], 5)) as u16, st.rt >> 27 & 1 != 0, mem);
        if rs >> 9 & 1 != 0 {
            d.write(zaddr, z as u16, st.zb >> 27 & 1 != 0, mem);
        }
        4
    }

    /// A hash of the memory stage `s`'s texture `t` is in: the 2 KB blocks
    /// (64 bytes by 32 lines) its texels touch, and its colour table.
    fn raw_print(&self, s: usize, t: &Tex, sdram: &[u8]) -> u64 {
        let row = (t.width as u32 * t.bpp).div_ceil(8);
        let mut h = 0xCBF2_9CE4_8422_2325u64;
        for v in (t.ov & !31..t.ov + t.height as u32).step_by(32) {
            for u in (t.ou & !63..t.ou + row).step_by(64) {
                let o = block_sdram(page_addr(t.sub >> 3, t.page, u, v), true) & !2047;
                if let Some(b) = sdram.get(o..o + 2048) {
                    h = b.chunks_exact(8).fold(h, |h, c| (h ^ u64::from_le_bytes(c.try_into().unwrap())).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23));
                }
            }
        }
        if matches!(t.mode, 0..=6 | 16..=22) {
            h = self.lut[s].iter().fold(h, |h, &c| (h ^ c as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(23));
        }
        h
    }

    /// Texture stage `s`'s texture as its registers give it.
    #[inline(always)]
    fn tex(&self, s: usize) -> Tex {
        let (i0, i1) = (self.regs[TEXINFO[s]], self.regs[TEXINFO[s] + 1]);
        let mode = i0 >> 19 & 0x3F;
        let bpp = match mode {
            0..=15 => 4,
            16..=31 => 8,
            _ => 16,
        };
        let sub = self.regs[TEXSUBSEGMENT] >> (16 * (i0 >> 27 & 1)) & 0x7FF;
        Tex {
            i0,
            mode,
            bpp,
            width: (((i1 >> 16 & 0xFF) + 1) * 32 / bpp) as i32,
            height: (((i1 >> 24) + 1) * 2) as i32,
            sub,
            page: (sub & 7) << 2 | (i0 >> 25 & 3),
            ou: (i1 & 0xFF) * 4,
            ov: (i1 >> 8 & 0xFF) * 2,
        }
    }

    /// Texel (tu, tv) of stage `s`'s texture `t` (wrapped or clamped): the
    /// R5G6B5 value the transparency check compares (when `tp`), and
    /// R G B A.
    #[inline(always)]
    fn fetch(&self, s: usize, t: &Tex, tu: i32, tv: i32, tp: bool, mem: Shared) -> (u32, [f32; 4]) {
        // A power-of-two size (most textures) wraps by a mask, without a
        // division; the same for negative coordinates.
        let wrap = |c: i32, size: i32, clamp: bool| match () {
            _ if clamp => c.clamp(0, size - 1),
            _ if size & (size - 1) == 0 => c & (size - 1),
            _ => c.rem_euclid(size),
        };
        let tu = wrap(tu, t.width, t.i0 & 1 != 0) as u32;
        let tv = wrap(tv, t.height, t.i0 >> 1 & 1 != 0) as u32;
        let at = |off: u32, n: u32| mem.read_as(page_addr(t.sub >> 3, t.page, t.ou + off, t.ov + tv), n, true);
        let raw = match t.bpp {
            4 => at(tu / 2, 1) >> if tu & 1 == 0 { 4 } else { 0 } & 15,
            8 => at(tu, 1),
            _ => at(tu * 2, 2),
        };
        let n = unit;
        let mode = t.mode;
        let rgba = match mode {
            0..=6 => decode16(mode, self.lut[s][(raw & 15) as usize] as u32),
            16..=22 => decode16(mode - 16, self.lut[s][raw as usize] as u32),
            9 => [1.0, 1.0, 1.0, n(raw, 4)],
            15 => [n(raw, 4), n(raw, 4), n(raw, 4), 1.0],
            25 => [n(raw >> 4, 4), n(raw >> 4, 4), n(raw >> 4, 4), n(raw & 15, 4)],
            27 => [n(raw, 8), n(raw, 8), n(raw, 8), 1.0],
            30 => [1.0, 1.0, 1.0, n(raw, 8)],
            31 => [n(raw & 15, 4), n(raw & 15, 4), n(raw & 15, 4), n(raw >> 4, 4)],
            40..=46 => decode16(mode - 40, raw),
            _ => [1.0, 0.0, 1.0, 1.0],
        };
        // The R5G6B5 value the transparency check compares (GUESS: the
        // colour as decoded, before filtering); only when it's on.
        let key = if tp { round_u32(rgba[0] * 31.0) << 11 | round_u32(rgba[1] * 63.0) << 5 | round_u32(rgba[2] * 31.0) } else { 0 };
        (key, rgba)
    }

    /// Texture stage `s`'s colour at texel (u, v) as R G B A, or None when
    /// the transparency colour check drops the pixel. An HD pixel with the
    /// texture upscaled (`up`, nearest sampling) takes the upscaled pixel
    /// there: the texel it was made from, or its colour.
    fn texel(&self, s: usize, u: f32, v: f32, bilinear: bool, tp: bool, mem: Shared) -> Option<[f32; 4]> {
        let t = self.tex(s);
        let fetch = |tu: i32, tv: i32| self.fetch(s, &t, tu, tv, tp, mem);
        if !bilinear && let Some((up, [u0, v0, u1, v1])) = self.up[s] {
            let n = UP as i32;
            let (x, y) = if up.centred() {
                // Upscaled pixel n * k is at texel k's centre, the ones
                // after it between it and the next. Only the texels drawn:
                // a texture's neighbour (in an atlas) isn't the picture's.
                let at = |c: f32| floor_i32(c * UP as f32 - (UP as f32 / 2.0 - 0.5));
                (at(u).clamp(u0 * n, u1 * n), at(v).clamp(v0 * n, v1 * n))
            } else {
                (floor_i32(u * UP as f32), floor_i32(v * UP as f32))
            };
            match up.at(x, y, t.i0 & 1 != 0, t.i0 >> 1 & 1 != 0) {
                Texel::Source(tu, tv) => {
                    let (key, c) = fetch(tu, tv);
                    if tp && key == self.regs[TPCOLOR] & 0xFFFF {
                        return None;
                    }
                    return Some(c);
                }
                // A painted texture's edge: transparent under half covered.
                Texel::Colour(c) if tp && c[3] < 0.5 => return None,
                Texel::Colour(c) => return Some(c),
            }
        }
        if !bilinear {
            let (key, c) = fetch(floor_i32(u), floor_i32(v));
            if tp && key == self.regs[TPCOLOR] & 0xFFFF {
                return None;
            }
            return Some(c);
        }
        let (fu, fv) = (u - 0.5, v - 0.5);
        let (iu, iv) = (floor_i32(fu), floor_i32(fv));
        let (du, dv) = (fu - iu as f32, fv - iv as f32);
        let mut c = [0.0; 4];
        for (dx, dy, wgt) in [(0, 0, (1.0 - du) * (1.0 - dv)), (1, 0, du * (1.0 - dv)), (0, 1, (1.0 - du) * dv), (1, 1, du * dv)] {
            let (key, t) = fetch(iu + dx, iv + dy);
            // A transparent texel adds nothing (GUESS).
            if tp && key == self.regs[TPCOLOR] & 0xFFFF {
                continue;
            }
            for k in 0..4 {
                c[k] += t[k] * wgt;
            }
        }
        Some(c)
    }

    /// One texture blending stage (GRP3D_TEXBLENDn): RGB args at bits
    /// 24-27, 20-23, 16-19 and op 12-15; alpha args 9-11, 6-8, 3-5, op 0-2.
    fn blend_stage(&self, s: usize, tex: [f32; 4], frag: [f32; 4], prev: [f32; 4]) -> [f32; 4] {
        let tb = self.regs[TEXBLEND[s]];
        let k = self.regs[TEXCONST];
        let cc = [unit(k >> 16, 8), unit(k >> 8, 8), unit(k, 8), unit(k >> 24, 8)];
        // Stage 0's code 12/14 is the texture again, stage 1's the previous
        // stage.
        let p = if s == 0 { tex } else { prev };
        let rgb_arg = |code: u32| -> [f32; 3] {
            let (src, inv, alpha) = match code {
                0..=3 => (tex, code & 1 != 0, code & 2 != 0),
                4..=7 => (cc, code & 1 != 0, code & 2 != 0),
                8..=11 => (frag, code & 1 != 0, code & 2 != 0),
                12 => (p, false, false),
                _ => (p, false, true),
            };
            std::array::from_fn(|c| {
                let v = if alpha { src[3] } else { src[c] };
                if inv { 1.0 - v } else { v }
            })
        };
        let a_arg = |code: u32| -> f32 {
            let v = match code >> 1 {
                0 => tex[3],
                1 => cc[3],
                2 => frag[3],
                _ => p[3],
            };
            if code & 1 != 0 && code < 6 { 1.0 - v } else { v }
        };
        let op = |op: u32, a: f32, b: f32, c: f32| match op {
            0 => a,
            1 => a * b,
            2 => a + b,
            3 => a + b - 0.5,
            4 => a * c + b * (1.0 - c),
            5 => a - b,
            _ => a,
        };
        let (r0, r1, r2, rop) = (rgb_arg(tb >> 24 & 15), rgb_arg(tb >> 20 & 15), rgb_arg(tb >> 16 & 15), tb >> 12 & 15);
        let mut out = [0.0; 4];
        if rop >= 6 {
            let d = 4.0 * (0..3).map(|c| (r0[c] - 0.5) * (r1[c] - 0.5)).sum::<f32>();
            out = [d; 4];
        } else {
            for c in 0..3 {
                out[c] = op(rop, r0[c], r1[c], r2[c]);
            }
        }
        if rop != 7 {
            out[3] = op(tb & 7, a_arg(tb >> 9 & 7), a_arg(tb >> 6 & 7), a_arg(tb >> 3 & 7));
        }
        out.map(|c| c.clamp(0.0, 1.0))
    }

}

impl Ga3d {
    /// Memory fill: a rectangle of a sub-segment (no tables, no clipping)
    /// with a 16-bit value. GUESS: the value is bits 0-15 (the data book
    /// says 8-15; the menu clears its Z buffer with 0xFFFF and its screen
    /// with 0x319E), coordinates in 16-bit pixels.
    fn memory_fill(&mut self, mem: &mut Mem) {
        self.epoch += 1;
        self.flush(mem);
        let (p0, p1, p2) = (self.regs[MFILLPARAM0], self.regs[MFILLPARAM0 + 1], self.regs[MFILLPARAM0 + 2]);
        let sub = p0 >> 16 & 0x7FF;
        let data = (p0 & 0xFFFF) as u16;
        let (left, top, right, bottom) = (p1 & 0x3FF, p1 >> 16 & 0x3FF, p2 & 0x3FF, p2 >> 16 & 0x3FF);
        self.cost += (right + 1).saturating_sub(left) as u64 * (bottom + 1).saturating_sub(top) as u64 * CYCLES_PER_FILL_PIXEL;
        for y in top..=bottom {
            for x in left..=right {
                let a = sub_addr(sub, 2 * x, y);
                mem.write16(a, data, p0 >> 27 & 1 != 0);
                if self.hd.on() {
                    self.hd.fill(a, data);
                }
            }
        }
    }

    /// LUT fill: 256 colours (8 x 32 16-bit pixels) or one bank of 16 (8 x 2)
    /// from texture sub-segment 0's page into a stage's table.
    fn lut_fill(&mut self, mem: &mut Mem) {
        self.epoch += 1;
        self.flush(mem);
        let p = self.regs[LUTPARAM];
        let sub = self.regs[TEXSUBSEGMENT] & 0x7FF;
        let page = (sub & 7) << 2 | (p >> 25 & 3);
        let (x, y) = ((p & 0xFF) * 4, (p >> 8 & 0xFF) * 2);
        let full = p >> 17 & 1 != 0;
        let (rows, first) = if full { (32, 0) } else { (2, (p >> 21 & 15) * 16) };
        let stage = (p >> 16 & 1) as usize;
        for r in 0..rows {
            for c in 0..8 {
                let v = mem.read_as(page_addr(sub >> 3, page, x + 2 * c, y + r), 2, true) as u16;
                self.lut[stage][(first + r * 8 + c) as usize & 255] = v;
            }
        }
    }
}

/// The render state a primitive needs, read once.
/// The lookup tables in save states (serde has no 256-entry arrays).
mod lut_serde {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[[u16; 256]; 2], s: S) -> Result<S::Ok, S::Error> {
        v.iter().flatten().copied().collect::<Vec<u16>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<[[u16; 256]; 2], D::Error> {
        let v = Vec::<u16>::deserialize(d)?;
        if v.len() != 512 {
            return Err(serde::de::Error::custom("lookup tables of the wrong size"));
        }
        let mut out = [[0; 256]; 2];
        for (i, x) in v.into_iter().enumerate() {
            out[i / 256][i % 256] = x;
        }
        Ok(out)
    }
}

#[derive(Clone, Copy)]
struct State {
    rs: u32,
    cull: bool,
    cull_cw: bool,
    perspective: bool,
    /// The clipping rectangle (left, top, right, bottom, inclusive).
    clip: [i32; 4],
    rt: u32,
    zb: u32,
    zscale: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swizzles() {
        assert_eq!(SWIZZLE[0], [0, 1, 2, 3]);
        assert_eq!(SWIZZLE[3], [3, 0, 1, 2]);
        assert_eq!(SWIZZLE[24], [0; 4]);
        assert_eq!(SWIZZLE[31], [7; 4]);
    }

    #[test]
    fn render_target_address() {
        // The menu's: sub-segment 0x0A (segment 1, sub-segment 2), rows
        // 256 on (table y 0x55), shown by the MLC at 0x2150_0000.
        assert_eq!(pixel_addr(0x0A, 0xE4, 0x55, 0, 0), 0x2150_0000);
        assert_eq!(pixel_addr(0x0A, 0xE4, 0x55, 1, 1), 0x2150_1002);
    }
}
