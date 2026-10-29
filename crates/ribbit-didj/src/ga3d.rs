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

use crate::mlc::{BLOCK_BASE, BLOCK_END, block_sdram};

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

/// A 16-bit colour of format `kind` (the low 3 bits of a colour mode) as
/// R G B A, 0..1.
fn decode16(kind: u32, v: u32) -> [f32; 4] {
    let n = |x: u32, bits: u32| x as f32 / ((1 << bits) - 1) as f32;
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
    lut: [[u16; 256]; 2],
    /// Register writes from the command queue, oldest first: (offset, value).
    pub trace: Vec<(u32, u32)>,
    /// Sub-module starts (GRP3D_CONTROL values) by kind, for the summary.
    pub starts: std::collections::BTreeMap<u32, u64>,
    /// Invalid commands seen (CMDID below 4).
    pub invalid: u64,
    /// Triangles that reached the rasterizer.
    pub triangles: u64,
    /// Pixels: (reached the pipeline, failed the depth test, dropped by
    /// the transparency colour, failed the alpha test, written).
    pub pixels: std::cell::Cell<[u64; 5]>,
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
        }
    }
}

impl Ga3d {
    pub fn read(&self, addr: u32) -> u32 {
        let i = ((addr - BASE) / 4) as usize;
        match i {
            STATUS => IDLE,
            _ => self.regs[i],
        }
    }

    /// A CPU write.
    pub fn write(&mut self, addr: u32, v: u32, mem: &mut Mem) {
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
                }
            }
            CMDQSTART => {
                // The reader starts at the queue's start (GUESS: the data
                // book only says the rear "is initialized by the Command
                // Reader"; libopengles_lite writes CPCONTROL after this).
                self.regs[i] = v;
                self.regs[CMDQREAR] = v;
            }
            CMDQFRONT => {
                self.regs[i] = v;
                self.run_queue(mem);
            }
            _ => self.set(i, v, mem),
        }
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
        std::array::from_fn(|c| self.regs.get(b + c).map_or(0.0, |&r| f24(r)))
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
                    if id == 5 || render_in {
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
        let each = |f: &dyn Fn(f32, f32) -> f32| -> [f32; 4] { std::array::from_fn(|c| f(a[c], b[c])) };
        let r: [f32; 4] = match id {
            6 => [1.0 / a[3].abs().sqrt(); 4],
            7 => [dot(4); 4],
            8 => [dot(3); 4],
            9 => [dot(4).max(0.0); 4],
            10 => [dot(3).max(0.0); 4],
            11 => std::array::from_fn(|c| self.mul[c] + b[c]),
            12 => each(&|x, y| x + y),
            13 => each(&|x, y| x - y),
            14 => each(&|x, y| if x >= y { 1.0 } else { 0.0 }),
            15 => each(&|x, y| if x < y { 1.0 } else { 0.0 }),
            16 => each(&|x, y| if x >= y { x } else { y }),
            17 => each(&|x, y| if x < y { x } else { y }),
            18 => {
                let m = each(&|x, y| x * y);
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
        let mut poly: Vec<[f32; 12]> = self.out.iter().map(clipv).collect();
        // Planes as (component, sign): d = w - c for upper ones, c for 0 <= c.
        for plane in 0..6 {
            let (c, upper) = (plane / 2, plane % 2 == 1);
            let d = |v: &[f32; 12]| if upper { v[3] - v[c] } else { v[c] };
            let mut next = Vec::with_capacity(poly.len() + 2);
            for k in 0..poly.len() {
                let (p, q) = (poly[k], poly[(k + 1) % poly.len()]);
                let (dp, dq) = (d(&p), d(&q));
                if dp >= 0.0 {
                    next.push(p);
                }
                if (dp >= 0.0) != (dq >= 0.0) {
                    let t = dp / (dp - dq);
                    next.push(std::array::from_fn(|i| p[i] + (q[i] - p[i]) * t));
                }
            }
            poly = next;
            if poly.len() < 3 {
                return;
            }
        }
        let f = |i: usize| f24(self.regs[VPORTBOT + i]);
        let (bottom, height, left, width) = (f(0), f(1), f(2), f(3));
        let screen: Vec<Vertex> = poly
            .iter()
            .map(|v| {
                let q = 1.0 / v[3];
                let mut s = [0.0; 12];
                s[0] = left + width * v[0] * q;
                s[1] = bottom - height * v[1] * q;
                s[2] = v[2] * q;
                s[3] = q;
                s[4..].copy_from_slice(&v[4..]);
                s
            })
            .collect();
        for k in 1..screen.len() - 1 {
            self.triangle(&[screen[0], screen[k], screen[k + 1]], invcull, mem);
        }
    }

    /// The TSE run directly or from the primitive processor in TSE mode:
    /// its input registers already hold screen-space vertices.
    fn tse(&mut self, triangle: bool, invcull: bool, mem: &mut Mem) {
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
        let st = self.state();
        for y in st.clip[1]..=st.clip[3] {
            let fy = y as f32;
            if fy < y0 || fy >= y1 {
                continue;
            }
            for x in st.clip[0]..=st.clip[2] {
                let fx = x as f32;
                if fx < x0 || fx >= x1 {
                    continue;
                }
                let (tx, ty) = ((fx - a[0]) / (b[0] - a[0]), (fy - a[1]) / (b[1] - a[1]));
                let mut p = *b;
                for k in [8, 10] {
                    p[k] = a[k] + (b[k] - a[k]) * tx;
                    p[k + 1] = a[k + 1] + (b[k + 1] - a[k + 1]) * ty;
                }
                self.pixel(&st, x as u32, y as u32, &p, mem);
            }
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
        if x0 > x1 || y0 > y1 {
            return;
        }
        let edge = |a: &Vertex, b: &Vertex, x: f32, y: f32| (b[0] - a[0]) * (y - a[1]) - (b[1] - a[1]) * (x - a[0]);
        // Top-left rule: an edge owns its pixels if it's a top or left one.
        let owns = |a: &Vertex, b: &Vertex| {
            let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
            if area > 0.0 { dy < 0.0 || dy == 0.0 && dx > 0.0 } else { dy > 0.0 || dy == 0.0 && dx < 0.0 }
        };
        let edges = [(1, 2, owns(&v[1], &v[2])), (2, 0, owns(&v[2], &v[0])), (0, 1, owns(&v[0], &v[1]))];
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (fx, fy) = (x as f32, y as f32);
                let mut w = [0.0f32; 3];
                let mut inside = true;
                for (k, &(a, b, own)) in edges.iter().enumerate() {
                    let e = edge(&v[a], &v[b], fx, fy) / area;
                    if e < 0.0 || e == 0.0 && !own {
                        inside = false;
                        break;
                    }
                    w[k] = e;
                }
                if !inside {
                    continue;
                }
                let lin = |i: usize| w[0] * v[0][i] + w[1] * v[1][i] + w[2] * v[2][i];
                let mut p = [0.0; 12];
                p[..8].iter_mut().enumerate().for_each(|(i, p)| *p = lin(i));
                if st.perspective {
                    let q = p[3];
                    for i in 8..12 {
                        p[i] = (w[0] * v[0][i] * v[0][3] + w[1] * v[1][i] * v[1][3] + w[2] * v[2][i] * v[2][3]) / q;
                    }
                } else {
                    for i in 8..12 {
                        p[i] = lin(i);
                    }
                }
                self.pixel(&st, x as u32, y as u32, &p, mem);
            }
        }
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

    /// The pixel pipeline: depth test, texture blending, alpha test, alpha
    /// blending, writing (R5G6B5). Fog and dithering aren't modelled yet.
    fn pixel(&self, st: &State, x: u32, y: u32, p: &Vertex, mem: &mut Mem) {
        let count = |k: usize| {
            let mut c = self.pixels.get();
            c[k] += 1;
            self.pixels.set(c);
        };
        count(0);
        let rs = st.rs;
        let zaddr = pixel_addr(st.zb >> 16 & 0x7FF, st.zb & 0xFF, st.zb >> 8 & 0xFF, x, y);
        let z = (p[2] * st.zscale).clamp(0.0, 65535.0) as u32;
        if rs >> 8 & 1 != 0 && !test(st.zb >> 28, z, mem.read_as(zaddr, 2, st.zb >> 27 & 1 != 0)) {
            count(1);
            return;
        }
        let shade = [p[5] / 255.0, p[6] / 255.0, p[7] / 255.0, p[4] / 255.0].map(|c| c.clamp(0.0, 1.0));
        let mut colour = shade;
        if rs >> 1 & 1 != 0 {
            let stages = if rs >> 2 & 1 != 0 { 2 } else { 1 };
            let mut prev = shade;
            for s in 0..stages {
                let Some(t) = self.texel(s, p[8 + 2 * s], p[9 + 2 * s], rs >> 5 & 1 != 0, rs >> 3 & 1 != 0, mem) else {
                    count(2);
                    return;
                };
                prev = self.blend_stage(s, t, shade, prev);
            }
            colour = prev;
        }
        if rs >> 14 & 1 != 0 {
            let at = self.regs[ALPHATEST];
            if !test(at, (colour[3] * 255.0).round() as u32, at >> 8 & 0xFF) {
                count(3);
                return;
            }
        }
        let addr = pixel_addr(st.rt >> 16 & 0x7FF, st.rt & 0xFF, st.rt >> 8 & 0xFF, x, y);
        if rs >> 7 & 1 != 0 {
            let dst = decode16(0, mem.read_as(addr, 2, st.rt >> 27 & 1 != 0));
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
        let q = |c: f32, bits: u32| (c * ((1 << bits) - 1) as f32).round() as u32;
        count(4);
        mem.write16(addr, (q(colour[0], 5) << 11 | q(colour[1], 6) << 5 | q(colour[2], 5)) as u16, st.rt >> 27 & 1 != 0);
        if rs >> 9 & 1 != 0 {
            mem.write16(zaddr, z as u16, st.zb >> 27 & 1 != 0);
        }
    }

    /// Texture stage `s`'s colour at texel (u, v) as R G B A, or None when
    /// the transparency colour check drops the pixel.
    fn texel(&self, s: usize, u: f32, v: f32, bilinear: bool, tp: bool, mem: &Mem) -> Option<[f32; 4]> {
        let (i0, i1) = (self.regs[TEXINFO[s]], self.regs[TEXINFO[s] + 1]);
        let mode = i0 >> 19 & 0x3F;
        let bpp = match mode {
            0..=15 => 4,
            16..=31 => 8,
            _ => 16,
        };
        let width = (((i1 >> 16 & 0xFF) + 1) * 32 / bpp) as i32;
        let height = (((i1 >> 24) + 1) * 2) as i32;
        let sub = self.regs[TEXSUBSEGMENT] >> (16 * (i0 >> 27 & 1)) & 0x7FF;
        let page = (sub & 7) << 2 | (i0 >> 25 & 3);
        let (ou, ov) = ((i1 & 0xFF) * 4, (i1 >> 8 & 0xFF) * 2);
        let wrap = |c: i32, size: i32, clamp: bool| if clamp { c.clamp(0, size - 1) } else { c.rem_euclid(size) };
        let fetch = |tu: i32, tv: i32| -> (u32, [f32; 4]) {
            let tu = wrap(tu, width, i0 & 1 != 0) as u32;
            let tv = wrap(tv, height, i0 >> 1 & 1 != 0) as u32;
            let at = |off: u32, n: u32| mem.read_as(page_addr(sub >> 3, page, ou + off, ov + tv), n, true);
            let raw = match bpp {
                4 => at(tu / 2, 1) >> if tu & 1 == 0 { 4 } else { 0 } & 15,
                8 => at(tu, 1),
                _ => at(tu * 2, 2),
            };
            let n = |x: u32, bits: u32| x as f32 / ((1 << bits) - 1) as f32;
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
            // The R5G6B5 value the transparency check compares (GUESS:
            // the colour as decoded, before filtering).
            let key = ((rgba[0] * 31.0).round() as u32) << 11 | ((rgba[1] * 63.0).round() as u32) << 5 | (rgba[2] * 31.0).round() as u32;
            (key, rgba)
        };
        if !bilinear {
            let (key, c) = fetch(u.floor() as i32, v.floor() as i32);
            if tp && key == self.regs[TPCOLOR] & 0xFFFF {
                return None;
            }
            return Some(c);
        }
        let (fu, fv) = (u - 0.5, v - 0.5);
        let (iu, iv) = (fu.floor() as i32, fv.floor() as i32);
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
        let cc = [(k >> 16 & 0xFF) as f32 / 255.0, (k >> 8 & 0xFF) as f32 / 255.0, (k & 0xFF) as f32 / 255.0, (k >> 24) as f32 / 255.0];
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

    /// Memory fill: a rectangle of a sub-segment (no tables, no clipping)
    /// with a 16-bit value. GUESS: the value is bits 0-15 (the data book
    /// says 8-15; the menu clears its Z buffer with 0xFFFF and its screen
    /// with 0x319E), coordinates in 16-bit pixels.
    fn memory_fill(&mut self, mem: &mut Mem) {
        let (p0, p1, p2) = (self.regs[MFILLPARAM0], self.regs[MFILLPARAM0 + 1], self.regs[MFILLPARAM0 + 2]);
        let sub = p0 >> 16 & 0x7FF;
        let data = (p0 & 0xFFFF) as u16;
        let (left, top, right, bottom) = (p1 & 0x3FF, p1 >> 16 & 0x3FF, p2 & 0x3FF, p2 >> 16 & 0x3FF);
        for y in top..=bottom {
            for x in left..=right {
                mem.write16(sub_addr(sub, 2 * x, y), data, p0 >> 27 & 1 != 0);
            }
        }
    }

    /// LUT fill: 256 colours (8 x 32 16-bit pixels) or one bank of 16 (8 x 2)
    /// from texture sub-segment 0's page into a stage's table.
    fn lut_fill(&mut self, mem: &mut Mem) {
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
