//! An MPEG-4 Part 2 video decoder, Simple Profile: what the RockIt Twist's
//! videos use (Xvid and libavcodec AVIs, 320 x 240: I- and P-frames, H.263
//! quantisation, progressive, no quarter-pel; some with resync markers).
//! It stands in for the console's hardware decoder at 0xD0C00000
//! (`machine`), which is given one frame's data at a time. Written from
//! ISO/IEC 14496-2; the code tables are in `mpeg4_tables`. Checked frame by
//! frame against FFmpeg on the packs' own videos (`examples/mp4dec.rs`).

use crate::mpeg4_tables as t;

/// Most-significant-bit-first reader.
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8], pos: usize) -> Self {
        Self { d, pos: pos * 8 }
    }

    fn left(&self) -> usize {
        (self.d.len() * 8).saturating_sub(self.pos)
    }

    /// The next `n` (at most 32) bits, without reading them; past the end
    /// reads as zeros.
    fn peek(&self, n: u32) -> u32 {
        let mut v: u64 = 0;
        let byte = self.pos / 8;
        for i in 0..5 {
            v = v << 8 | *self.d.get(byte + i).unwrap_or(&0) as u64;
        }
        let shift = 40 - (self.pos % 8) as u32 - n;
        ((v >> shift) & ((1u64 << n) - 1)) as u32
    }

    fn skip(&mut self, n: u32) {
        self.pos += n as usize;
    }

    fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let v = self.peek(n);
        self.skip(n);
        v
    }

    fn bit(&mut self) -> bool {
        self.get(1) == 1
    }
}

/// A variable-length code: a lookup of `bits` bits giving (symbol, length).
struct Vlc {
    bits: u32,
    table: Vec<(u16, u8)>,
}

impl Vlc {
    fn new(codes: &[(u16, u8)]) -> Self {
        let bits = codes.iter().map(|c| c.1 as u32).max().unwrap_or(1);
        let mut table = vec![(u16::MAX, 0u8); 1 << bits];
        for (sym, &(code, len)) in codes.iter().enumerate() {
            if len == 0 {
                continue;
            }
            let shift = bits - len as u32;
            let first = (code as usize) << shift;
            for e in &mut table[first..first + (1 << shift)] {
                *e = (sym as u16, len);
            }
        }
        Self { bits, table }
    }

    /// The symbol, or None for a code not in the table.
    fn read(&self, b: &mut Bits) -> Option<usize> {
        let (sym, len) = self.table[b.peek(self.bits) as usize];
        if sym == u16::MAX {
            return None;
        }
        b.skip(len as u32);
        Some(sym as usize)
    }
}

/// A coefficient table (TCOEF) with its escape limits.
struct RunLevel {
    vlc: Vlc,
    run: &'static [i8],
    level: &'static [i8],
    last: usize,
    /// Largest level for (last, run), largest run for (last, level).
    max_level: [[i32; 64]; 2],
    max_run: [[i32; 64]; 2],
}

impl RunLevel {
    fn new(codes: &[(u16, u8)], run: &'static [i8], level: &'static [i8], last: usize) -> Self {
        let mut max_level = [[0; 64]; 2];
        let mut max_run = [[0; 64]; 2];
        for i in 0..run.len() {
            let l = (i >= last) as usize;
            let (r, v) = (run[i] as usize, level[i] as usize);
            max_level[l][r] = max_level[l][r].max(v as i32);
            max_run[l][v] = max_run[l][v].max(r as i32);
        }
        Self { vlc: Vlc::new(codes), run, level, last, max_level, max_run }
    }

    /// One (last, run, level) event; None on a bad code.
    fn read(&self, b: &mut Bits) -> Option<(bool, i32, i32)> {
        let escape = self.run.len();
        let code = self.vlc.read(b)?;
        if code != escape {
            let level = self.level[code] as i32;
            let sign = b.bit();
            return Some((code >= self.last, self.run[code] as i32, if sign { -level } else { level }));
        }
        // Escapes (14496-2 7.4.1.3): type 1 adds to the level, type 2 to
        // the run; type 3 spells the event out.
        if !b.bit() {
            let code = self.vlc.read(b)?;
            if code == escape {
                return None;
            }
            let last = code >= self.last;
            let run = self.run[code] as i32;
            let level = self.level[code] as i32 + self.max_level[last as usize][run as usize];
            let sign = b.bit();
            return Some((last, run, if sign { -level } else { level }));
        }
        if !b.bit() {
            let code = self.vlc.read(b)?;
            if code == escape {
                return None;
            }
            let last = code >= self.last;
            let level = self.level[code] as i32;
            let run = self.run[code] as i32 + self.max_run[last as usize][level as usize] + 1;
            let sign = b.bit();
            return Some((last, run, if sign { -level } else { level }));
        }
        let last = b.bit();
        let run = b.get(6) as i32;
        b.skip(1);
        let level = ((b.get(12) as i32) << 20) >> 20;
        b.skip(1);
        Some((last, run, level))
    }
}

struct Tables {
    intra_mcbpc: Vlc,
    inter_mcbpc: Vlc,
    cbpy: Vlc,
    mv: Vlc,
    dc_lum: Vlc,
    dc_chrom: Vlc,
    intra: RunLevel,
    inter: RunLevel,
}

fn tables() -> &'static Tables {
    static T: std::sync::OnceLock<Tables> = std::sync::OnceLock::new();
    T.get_or_init(|| Tables {
        intra_mcbpc: Vlc::new(&t::INTRA_MCBPC),
        inter_mcbpc: Vlc::new(&t::INTER_MCBPC),
        cbpy: Vlc::new(&t::CBPY),
        mv: Vlc::new(&t::MV),
        dc_lum: Vlc::new(&t::DC_LUM),
        dc_chrom: Vlc::new(&t::DC_CHROM),
        intra: RunLevel::new(&t::INTRA_VLC, &t::INTRA_RUN, &t::INTRA_LEVEL, t::INTRA_LAST),
        inter: RunLevel::new(&t::INTER_VLC, &t::INTER_RUN, &t::INTER_LEVEL, t::INTER_LAST),
    })
}

const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55,
    62, 63,
];

/// A picture in YUV 4:2:0, sized in whole macroblocks.
#[derive(Clone)]
pub struct Picture {
    pub width: usize,
    pub height: usize,
    /// Planes of (mb_w * 16) x (mb_h * 16) luma and half that chroma.
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Picture {
    fn new(width: usize, height: usize) -> Self {
        let (w, h) = (width.div_ceil(16) * 16, height.div_ceil(16) * 16);
        Self { width, height, y: vec![16; w * h], u: vec![128; w * h / 4], v: vec![128; w * h / 4] }
    }

    fn stride(&self) -> usize {
        self.width.div_ceil(16) * 16
    }

    /// The pixel at (x, y), BT.601 to RGB 5:6:5.
    pub fn rgb565(&self, x: usize, y: usize) -> u16 {
        let s = self.stride();
        let yy = self.y[y * s + x] as i32 - 16;
        let (u, v) = (self.u[y / 2 * s / 2 + x / 2] as i32 - 128, self.v[y / 2 * s / 2 + x / 2] as i32 - 128);
        let c = |n: i32| (n >> 8).clamp(0, 255) as u16;
        let (r, g, b) = (c(298 * yy + 409 * v + 128), c(298 * yy - 100 * u - 208 * v + 128), c(298 * yy + 516 * u + 128));
        (r >> 3) << 11 | (g >> 2) << 5 | b >> 3
    }
}

/// The stream's settings, from its video object layer header.
#[derive(Clone, Copy)]
struct Vol {
    width: usize,
    height: usize,
    time_bits: u32,
    resync: bool,
}

/// Decoding state kept between frames.
#[derive(Default)]
pub struct Decoder {
    vol: Option<Vol>,
}

/// Per macroblock, what the next ones predict from.
#[derive(Clone, Copy, Default)]
struct MbInfo {
    intra: bool,
    /// Video packet it belongs to (prediction stays inside one).
    packet: u32,
    quant: i32,
    /// The four luma blocks' motion vectors (half pels).
    mv: [(i32, i32); 4],
}

/// Per 8x8 block: the dequantised DC and the quantised first row and column
/// (AC prediction).
#[derive(Clone, Copy)]
struct BlockPred {
    dc: i32,
    row: [i32; 8],
    col: [i32; 8],
}

impl Default for BlockPred {
    fn default() -> Self {
        Self { dc: 1024, row: [0; 8], col: [0; 8] }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    NoVol,
    NoVop,
    Unsupported(&'static str),
    Bitstream,
}

impl Decoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Decodes the frame in `data` (a VOP, possibly with the headers before
    /// it), predicting from `reference`. A frame marked not coded is the
    /// reference unchanged.
    pub fn decode(&mut self, data: &[u8], reference: Option<&Picture>) -> Result<Picture, Error> {
        let mut at = 0;
        let mut vop = None;
        while let Some(i) = find_start(data, at) {
            match data[i + 3] {
                0x20..=0x2F => self.vol = Some(parse_vol(&data[i + 4..])?),
                0xB6 => {
                    vop = Some(i + 4);
                    break;
                }
                _ => {}
            }
            at = i + 4;
        }
        let vol = self.vol.ok_or(Error::NoVol)?;
        let vop = vop.ok_or(Error::NoVop)?;
        let mut b = Bits::new(data, vop);
        let kind = b.get(2);
        if kind > 1 {
            return Err(Error::Unsupported("B or S frame"));
        }
        while b.bit() {}
        b.skip(1);
        b.skip(vol.time_bits);
        b.skip(1);
        if !b.bit() {
            return Ok(reference.cloned().unwrap_or_else(|| Picture::new(vol.width, vol.height)));
        }
        let rounding = if kind == 1 { b.get(1) as i32 } else { 0 };
        let dc_thr = t::DC_THRESHOLD[b.get(3) as usize] as i32;
        let quant = b.get(5) as i32;
        let fcode = if kind == 1 { b.get(3) } else { 1 };
        let blank = Picture::new(vol.width, vol.height);
        let reference = match (kind, reference) {
            (0, _) => None,
            (_, Some(r)) if r.width == vol.width && r.height == vol.height => Some(r),
            _ => Some(&blank),
        };
        let mut f = Frame::new(vol, kind == 1, rounding, dc_thr, fcode, quant, reference);
        f.decode(&mut b)?;
        Ok(f.pic)
    }
}

fn find_start(d: &[u8], from: usize) -> Option<usize> {
    (from..d.len().saturating_sub(3)).find(|&i| d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1)
}

fn parse_vol(d: &[u8]) -> Result<Vol, Error> {
    let mut b = Bits::new(d, 0);
    b.skip(1 + 8);
    let verid = if b.bit() {
        let v = b.get(4);
        b.skip(3);
        v
    } else {
        1
    };
    if b.get(4) == 15 {
        b.skip(16);
    }
    if b.bit() {
        b.skip(3);
        if b.bit() {
            b.skip(79);
        }
    }
    if b.get(2) != 0 {
        return Err(Error::Unsupported("shaped video"));
    }
    b.skip(1);
    let res = b.get(16).max(1);
    // vop_time_increment takes the bits to count up to res - 1 (at least 1).
    let time_bits = (32 - (res - 1).leading_zeros()).max(1);
    b.skip(1);
    if b.bit() {
        b.skip(time_bits);
    }
    b.skip(1);
    let width = b.get(13) as usize;
    b.skip(1);
    let height = b.get(13) as usize;
    b.skip(1);
    if b.bit() {
        return Err(Error::Unsupported("interlaced"));
    }
    b.skip(1);
    if b.get(if verid == 1 { 1 } else { 2 }) != 0 {
        return Err(Error::Unsupported("sprites"));
    }
    if b.bit() {
        return Err(Error::Unsupported("not 8-bit"));
    }
    if b.bit() {
        return Err(Error::Unsupported("MPEG quantisation"));
    }
    if verid != 1 && b.bit() {
        return Err(Error::Unsupported("quarter pel"));
    }
    if !b.bit() {
        return Err(Error::Unsupported("complexity estimation"));
    }
    let resync = !b.bit();
    if b.bit() {
        return Err(Error::Unsupported("data partitioning"));
    }
    if width == 0 || height == 0 || width > 1024 || height > 1024 {
        return Err(Error::Bitstream);
    }
    Ok(Vol { width, height, time_bits, resync })
}

struct Frame<'r> {
    vol: Vol,
    p: bool,
    rounding: i32,
    dc_thr: i32,
    fcode: u32,
    quant: i32,
    mb_w: usize,
    mb_h: usize,
    pic: Picture,
    reference: Option<&'r Picture>,
    mbs: Vec<MbInfo>,
    /// Per 8x8 block: luma in a (2 mb_w) x (2 mb_h) grid, then U, then V
    /// each mb_w x mb_h.
    preds: Vec<BlockPred>,
    packet: u32,
    packet_start: usize,
}

impl<'r> Frame<'r> {
    fn new(vol: Vol, p: bool, rounding: i32, dc_thr: i32, fcode: u32, quant: i32, reference: Option<&'r Picture>) -> Self {
        let (mb_w, mb_h) = (vol.width.div_ceil(16), vol.height.div_ceil(16));
        Self {
            vol,
            p,
            rounding,
            dc_thr,
            fcode,
            quant,
            mb_w,
            mb_h,
            pic: Picture::new(vol.width, vol.height),
            reference,
            mbs: vec![MbInfo::default(); mb_w * mb_h],
            preds: vec![BlockPred::default(); mb_w * mb_h * 6],
            packet: 0,
            packet_start: 0,
        }
    }

    fn decode(&mut self, b: &mut Bits) -> Result<(), Error> {
        let n = self.mb_w * self.mb_h;
        let mut i = 0;
        while i < n {
            if self.vol.resync
                && i > 0
                && let Some(next) = self.video_packet(b)?
            {
                i = next;
                if i >= n {
                    return Err(Error::Bitstream);
                }
            }
            if b.left() == 0 {
                return Err(Error::Bitstream);
            }
            self.macroblock(b, i)?;
            i += 1;
        }
        Ok(())
    }

    /// A video packet header at the next byte boundary (after the stuffing
    /// bits): its first macroblock, with the quantiser set from it.
    fn video_packet(&mut self, b: &mut Bits) -> Result<Option<usize>, Error> {
        let stuff = 8 - (b.pos % 8) as u32;
        if b.peek(stuff) != (1 << (stuff - 1)) - 1 {
            return Ok(None);
        }
        let marker = if self.p { 16 + self.fcode } else { 17 };
        let save = b.pos;
        b.skip(stuff);
        if b.peek(marker) != 1 {
            b.pos = save;
            return Ok(None);
        }
        b.skip(marker);
        let n = self.mb_w * self.mb_h;
        let bits = usize::BITS - (n - 1).leading_zeros();
        let mb = b.get(bits) as usize;
        self.quant = b.get(5) as i32;
        if b.bit() {
            while b.bit() {}
            b.skip(1);
            b.skip(self.vol.time_bits);
            b.skip(1);
            b.skip(2 + 3);
            if self.p {
                b.skip(3);
            }
        }
        self.packet += 1;
        self.packet_start = mb;
        Ok(Some(mb))
    }

    fn macroblock(&mut self, b: &mut Bits, i: usize) -> Result<(), Error> {
        let tb = tables();
        let (mx, my) = (i % self.mb_w, i / self.mb_w);
        let mut info = MbInfo { packet: self.packet, ..Default::default() };
        let (kind, cbpc);
        loop {
            if self.p {
                if b.bit() {
                    // Not coded: the reference, unmoved.
                    info.quant = self.quant;
                    self.mbs[i] = info;
                    self.reset_preds(mx, my);
                    self.predict_mb(mx, my, &[(0, 0); 4]);
                    return Ok(());
                }
                let c = tb.inter_mcbpc.read(b).ok_or(Error::Bitstream)?;
                if c == 20 {
                    continue;
                }
                if c > 20 {
                    return Err(Error::Bitstream);
                }
                // Table order: bit 2 intra, bit 3 quantiser change, bit 4
                // four vectors; as macroblock types 0 inter, 1 inter+q,
                // 2 inter with four vectors, 3 intra, 4 intra+q.
                (kind, cbpc) = ([0, 3, 1, 4, 2][c >> 2], (c & 3) as u32);
            } else {
                let c = tb.intra_mcbpc.read(b).ok_or(Error::Bitstream)?;
                if c == 8 {
                    continue;
                }
                (kind, cbpc) = (3 + (c >> 2), (c & 3) as u32);
            }
            break;
        }
        let intra = kind >= 3;
        let ac_pred = intra && b.bit();
        let mut cbpy = tb.cbpy.read(b).ok_or(Error::Bitstream)? as u32;
        if !intra {
            cbpy = 15 - cbpy;
        }
        if kind == 1 || kind == 4 {
            self.quant = (self.quant + [-1, -2, 1, 2][b.get(2) as usize]).clamp(1, 31);
        }
        info.intra = intra;
        info.quant = self.quant;
        let cbp = cbpy << 2 | cbpc;
        if intra {
            self.mbs[i] = info;
            for blk in 0..6 {
                self.intra_block(b, mx, my, blk, ac_pred, cbp & (32 >> blk) != 0)?;
            }
            return Ok(());
        }
        // Motion vectors: one for the macroblock, or one per luma block.
        let count = if kind == 2 { 4 } else { 1 };
        for k in 0..count {
            let (px, py) = self.mv_pred(mx, my, k, &info);
            let x = self.read_mv(b, px)?;
            let y = self.read_mv(b, py)?;
            if count == 1 {
                info.mv = [(x, y); 4];
            } else {
                info.mv[k] = (x, y);
            }
        }
        self.mbs[i] = info;
        self.reset_preds(mx, my);
        self.predict_mb(mx, my, &info.mv);
        for blk in 0..6 {
            if cbp & (32 >> blk) != 0 {
                let mut c = [0i32; 64];
                self.coefficients(b, &tb.inter, &mut c, 0, &ZIGZAG)?;
                for v in c.iter_mut() {
                    *v = dequant(*v, self.quant);
                }
                let (plane, stride, x, y) = self.block_place(mx, my, blk);
                add_idct(&c, plane_of(&mut self.pic, plane), stride, x, y);
            }
        }
        Ok(())
    }

    fn read_mv(&self, b: &mut Bits, pred: i32) -> Result<i32, Error> {
        let code = tables().mv.read(b).ok_or(Error::Bitstream)? as i32;
        if code == 0 {
            return Ok(pred);
        }
        let sign = b.bit();
        let shift = self.fcode as i32 - 1;
        let mut v = code;
        if shift > 0 {
            v = ((v - 1) << shift | b.get(shift as u32) as i32) + 1;
        }
        if sign {
            v = -v;
        }
        let v = v + pred;
        // Wrapped into the range the f_code gives (5 + f_code bits).
        let bits = 5 + self.fcode;
        Ok((v << (32 - bits)) >> (32 - bits))
    }

    /// The median prediction for luma block `k`'s vector (14496-2 7.6.5).
    fn mv_pred(&self, mx: usize, my: usize, k: usize, cur: &MbInfo) -> (i32, i32) {
        // Candidates: (macroblock dx, dy, block), the current macroblock's
        // own blocks given as (0, 0).
        let cands: [(isize, isize, usize); 3] = match k {
            0 => [(-1, 0, 1), (0, -1, 2), (1, -1, 2)],
            1 => [(0, 0, 0), (0, -1, 3), (1, -1, 2)],
            2 => [(-1, 0, 3), (0, 0, 0), (0, 0, 1)],
            _ => [(0, 0, 2), (0, 0, 0), (0, 0, 1)],
        };
        let mut v: [Option<(i32, i32)>; 3] = [None; 3];
        for (j, &(dx, dy, blk)) in cands.iter().enumerate() {
            if dx == 0 && dy == 0 {
                v[j] = Some(cur.mv[blk]);
                continue;
            }
            let (x, y) = (mx as isize + dx, my as isize + dy);
            if x < 0 || y < 0 || x >= self.mb_w as isize {
                continue;
            }
            let idx = y as usize * self.mb_w + x as usize;
            if idx < self.packet_start {
                continue;
            }
            let m = &self.mbs[idx];
            v[j] = Some(if m.intra { (0, 0) } else { m.mv[blk] });
        }
        let valid: Vec<(i32, i32)> = v.iter().flatten().copied().collect();
        match valid.len() {
            0 => (0, 0),
            1 => valid[0],
            _ => {
                let get = |j: usize| v[j].unwrap_or((0, 0));
                let (a, b, c) = (get(0), get(1), get(2));
                (median(a.0, b.0, c.0), median(a.1, b.1, c.1))
            }
        }
    }

    /// Motion-compensated prediction for the whole macroblock.
    fn predict_mb(&mut self, mx: usize, my: usize, mv: &[(i32, i32); 4]) {
        let Some(r) = self.reference else {
            return;
        };
        let s = self.pic.stride();
        let (w, h) = (self.vol.width, self.vol.height);
        for (k, &(vx, vy)) in mv.iter().enumerate() {
            let (x, y) = (mx * 16 + (k & 1) * 8, my * 16 + (k >> 1) * 8);
            mc(&r.y, s, (w, h), &mut self.pic.y, x, y, 8, vx, vy, self.rounding);
        }
        // Chroma: one vector, from the luma one (or the four's sum).
        let (cx, cy) = if mv.iter().all(|m| *m == mv[0]) {
            ((mv[0].0 >> 1) | (mv[0].0 & 1), (mv[0].1 >> 1) | (mv[0].1 & 1))
        } else {
            let round = |s: i32| {
                const TAB: [i32; 16] = [0, 0, 0, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2];
                TAB[(s & 15) as usize] + (s >> 4) * 2
            };
            let (sx, sy) = mv.iter().fold((0, 0), |a, m| (a.0 + m.0, a.1 + m.1));
            (round(sx), round(sy))
        };
        let (cs, cdim) = (s / 2, (w.div_ceil(2), h.div_ceil(2)));
        mc(&r.u, cs, cdim, &mut self.pic.u, mx * 8, my * 8, 8, cx, cy, self.rounding);
        mc(&r.v, cs, cdim, &mut self.pic.v, mx * 8, my * 8, 8, cx, cy, self.rounding);
    }

    /// An inter macroblock leaves nothing for intra prediction.
    fn reset_preds(&mut self, mx: usize, my: usize) {
        for blk in 0..6 {
            let i = self.pred_index(mx, my, blk);
            self.preds[i] = BlockPred::default();
        }
    }

    fn pred_index(&self, mx: usize, my: usize, blk: usize) -> usize {
        let luma = self.mb_w * self.mb_h * 4;
        match blk {
            0..=3 => (my * 2 + (blk >> 1)) * self.mb_w * 2 + mx * 2 + (blk & 1),
            4 => luma + my * self.mb_w + mx,
            _ => luma + self.mb_w * self.mb_h + my * self.mb_w + mx,
        }
    }

    /// The prediction block `dx`, `dy` blocks away from block `blk` of
    /// macroblock (mx, my), if it is in the picture, in an intra
    /// macroblock of the same video packet: (its prediction values, its
    /// macroblock's quantiser).
    fn neighbour(&self, mx: usize, my: usize, blk: usize, dx: isize, dy: isize) -> Option<(BlockPred, i32)> {
        let (bx, by, per) = if blk < 4 { (mx * 2 + (blk & 1), my * 2 + (blk >> 1), 2) } else { (mx, my, 1) };
        let (x, y) = (bx as isize + dx, by as isize + dy);
        if x < 0 || y < 0 {
            return None;
        }
        let (x, y) = (x as usize, y as usize);
        let (nmx, nmy) = (x / per, y / per);
        let ni = nmy * self.mb_w + nmx;
        let m = &self.mbs[ni];
        if !m.intra || m.packet != self.packet || ni < self.packet_start {
            return None;
        }
        let nblk = if blk < 4 { (y % 2) * 2 + x % 2 } else { blk };
        Some((self.preds[self.pred_index(nmx, nmy, nblk)], m.quant))
    }

    fn intra_block(&mut self, b: &mut Bits, mx: usize, my: usize, blk: usize, ac_pred: bool, coded: bool) -> Result<(), Error> {
        let tb = tables();
        let luma = blk < 4;
        let scale = if luma { t::Y_DC_SCALE } else { t::C_DC_SCALE }[self.quant as usize] as i32;
        // DC prediction: from the left (A) or above (C), whichever the
        // gradient through the above-left (B) favours.
        let a = self.neighbour(mx, my, blk, -1, 0);
        let bb = self.neighbour(mx, my, blk, -1, -1);
        let c = self.neighbour(mx, my, blk, 0, -1);
        let dc = |n: &Option<(BlockPred, i32)>| n.map_or(1024, |p| p.0.dc);
        let from_above = (dc(&a) - dc(&bb)).abs() < (dc(&bb) - dc(&c)).abs();
        let pred_dc = if from_above { dc(&c) } else { dc(&a) };
        let scan: &[u8; 64] = match (ac_pred, from_above) {
            (false, _) => &ZIGZAG,
            (true, true) => &t::ALT_HORIZONTAL_SCAN,
            (true, false) => &t::ALT_VERTICAL_SCAN,
        };
        let mut c_ = [0i32; 64];
        let use_dc_vlc = self.quant < self.dc_thr;
        let mut start = 0;
        if use_dc_vlc {
            let size = if luma { tb.dc_lum.read(b) } else { tb.dc_chrom.read(b) }.ok_or(Error::Bitstream)? as u32;
            let diff = if size == 0 {
                0
            } else {
                let v = b.get(size) as i32;
                let d = if v >> (size - 1) == 0 { v - (1 << size) + 1 } else { v };
                if size > 8 {
                    b.skip(1);
                }
                d
            };
            c_[0] = diff;
            start = 1;
        }
        if coded {
            self.coefficients(b, &tb.intra, &mut c_, start, scan)?;
        }
        // The DC level, then dequantised by the DC scaler.
        let dc_level = c_[0] + (pred_dc + (scale >> 1)) / scale;
        let dc_val = (dc_level * scale).clamp(-2048, 2047);
        // AC prediction from the same neighbour's first row or column.
        if ac_pred {
            let q = self.quant;
            let rescale = |v: i32, qn: i32| if qn == q { v } else { div_round(v * qn, q) };
            if from_above {
                if let Some((p, qn)) = c {
                    for k in 1..8 {
                        c_[k] += rescale(p.row[k], qn);
                    }
                }
            } else if let Some((p, qn)) = a {
                for k in 1..8 {
                    c_[k * 8] += rescale(p.col[k], qn);
                }
            }
        }
        let mut pred = BlockPred { dc: dc_val, ..Default::default() };
        for k in 1..8 {
            pred.row[k] = c_[k];
            pred.col[k] = c_[k * 8];
        }
        let pi = self.pred_index(mx, my, blk);
        self.preds[pi] = pred;
        c_[0] = dc_val;
        for v in c_.iter_mut().skip(1) {
            *v = dequant(*v, self.quant);
        }
        let (plane, stride, x, y) = self.block_place(mx, my, blk);
        set_idct(&c_, plane_of(&mut self.pic, plane), stride, x, y);
        Ok(())
    }

    /// Coefficient events into `c` (natural order) from position `start`.
    fn coefficients(&self, b: &mut Bits, rl: &RunLevel, c: &mut [i32; 64], start: usize, scan: &[u8; 64]) -> Result<(), Error> {
        let mut i = start;
        loop {
            let (last, run, level) = rl.read(b).ok_or(Error::Bitstream)?;
            i += run as usize;
            if i >= 64 {
                return Err(Error::Bitstream);
            }
            c[scan[i] as usize] = level;
            if last {
                return Ok(());
            }
            i += 1;
        }
    }

    /// Where block `blk` of macroblock (mx, my) goes: (plane 0-2, stride,
    /// x, y).
    fn block_place(&self, mx: usize, my: usize, blk: usize) -> (usize, usize, usize, usize) {
        let s = self.pic.stride();
        match blk {
            0..=3 => (0, s, mx * 16 + (blk & 1) * 8, my * 16 + (blk >> 1) * 8),
            4 => (1, s / 2, mx * 8, my * 8),
            _ => (2, s / 2, mx * 8, my * 8),
        }
    }
}

fn plane_of(p: &mut Picture, n: usize) -> &mut [u8] {
    match n {
        0 => &mut p.y,
        1 => &mut p.u,
        _ => &mut p.v,
    }
}

fn median(a: i32, b: i32, c: i32) -> i32 {
    a.max(b).min(a.min(b).max(c))
}

fn div_round(a: i32, b: i32) -> i32 {
    if a >= 0 { (a + b / 2) / b } else { -((-a + b / 2) / b) }
}

/// H.263 inverse quantisation.
fn dequant(level: i32, q: i32) -> i32 {
    if level == 0 {
        return 0;
    }
    let m = (2 * level.abs() + 1) * q - if q % 2 == 0 { 1 } else { 0 };
    (if level < 0 { -m } else { m }).clamp(-2048, 2047)
}

/// Half-pel motion compensation of an n x n block at (x, y) from `src`,
/// whose picture is `size` (its edges repeated beyond it).
#[allow(clippy::too_many_arguments)]
fn mc(src: &[u8], stride: usize, size: (usize, usize), dst: &mut [u8], x: usize, y: usize, n: usize, vx: i32, vy: i32, r: i32) {
    let (fx, fy) = (x as i32 + (vx >> 1), y as i32 + (vy >> 1));
    let (hx, hy) = (vx & 1, vy & 1);
    let at = |px: i32, py: i32| {
        let px = px.clamp(0, size.0 as i32 - 1) as usize;
        let py = py.clamp(0, size.1 as i32 - 1) as usize;
        src[py * stride + px] as i32
    };
    for j in 0..n as i32 {
        for i in 0..n as i32 {
            let (sx, sy) = (fx + i, fy + j);
            let v = match (hx, hy) {
                (0, 0) => at(sx, sy),
                (1, 0) => (at(sx, sy) + at(sx + 1, sy) + 1 - r) >> 1,
                (0, 1) => (at(sx, sy) + at(sx, sy + 1) + 1 - r) >> 1,
                _ => (at(sx, sy) + at(sx + 1, sy) + at(sx, sy + 1) + at(sx + 1, sy + 1) + 2 - r) >> 2,
            };
            dst[(y + j as usize) * stride + x + i as usize] = v as u8;
        }
    }
}

/// The 8x8 inverse DCT (separable, in double precision, rounded).
fn idct(c: &[i32; 64]) -> [i32; 64] {
    static COS: std::sync::OnceLock<[[f64; 8]; 8]> = std::sync::OnceLock::new();
    let cos = COS.get_or_init(|| {
        let mut m = [[0.0; 8]; 8];
        for (x, row) in m.iter_mut().enumerate() {
            for (u, v) in row.iter_mut().enumerate() {
                let cu = if u == 0 { std::f64::consts::FRAC_1_SQRT_2 } else { 1.0 };
                *v = cu / 2.0 * ((2 * x + 1) as f64 * u as f64 * std::f64::consts::PI / 16.0).cos();
            }
        }
        m
    });
    let mut tmp = [0.0f64; 64];
    for y in 0..8 {
        for x in 0..8 {
            tmp[y * 8 + x] = (0..8).map(|u| cos[x][u] * c[y * 8 + u] as f64).sum();
        }
    }
    let mut out = [0i32; 64];
    for x in 0..8 {
        for y in 0..8 {
            let v: f64 = (0..8).map(|v| cos[y][v] * tmp[v * 8 + x]).sum();
            out[y * 8 + x] = v.round() as i32;
        }
    }
    out
}

fn set_idct(c: &[i32; 64], plane: &mut [u8], stride: usize, x: usize, y: usize) {
    let p = idct(c);
    for j in 0..8 {
        for i in 0..8 {
            plane[(y + j) * stride + x + i] = p[j * 8 + i].clamp(0, 255) as u8;
        }
    }
}

fn add_idct(c: &[i32; 64], plane: &mut [u8], stride: usize, x: usize, y: usize) {
    let p = idct(c);
    for j in 0..8 {
        for i in 0..8 {
            let at = (y + j) * stride + x + i;
            plane[at] = (plane[at] as i32 + p[j * 8 + i]).clamp(0, 255) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_tables_are_prefix_free_and_complete() {
        for codes in
            [&t::INTRA_MCBPC[..], &t::INTER_MCBPC, &t::CBPY, &t::MV, &t::DC_LUM, &t::DC_CHROM, &t::INTER_VLC, &t::INTRA_VLC]
        {
            for (i, &(a, la)) in codes.iter().enumerate() {
                for &(b, lb) in &codes[i + 1..] {
                    let l = la.min(lb) as u32;
                    assert!(
                        la == 0 || lb == 0 || (a >> (la as u32 - l)) != (b >> (lb as u32 - l)),
                        "{a:#x}/{la} and {b:#x}/{lb} overlap"
                    );
                }
            }
        }
        // The escape is the last coefficient code: 0000011.
        assert_eq!(t::INTER_VLC[102], (3, 7));
        assert_eq!(t::INTRA_VLC[102], (3, 7));
    }

    #[test]
    fn h263_dequantisation_and_escapes() {
        assert_eq!((dequant(1, 5), dequant(-2, 5), dequant(1, 6), dequant(0, 9)), (15, -25, 17, 0));
        let rl = &tables().inter;
        // Type 3 escape: 0000011 1 1, last 1, run 5, marker, level -3, marker.
        let bits: u64 = (0b0000011 << 23) | (0b11 << 21) | (1 << 20) | (5 << 14) | (1 << 13) | ((-3i32 as u64 & 0xFFF) << 1) | 1;
        let data = (bits << 34).to_be_bytes();
        let mut b = Bits::new(&data, 0);
        assert_eq!(rl.read(&mut b), Some((true, 5, -3)));
        assert_eq!(b.pos, 30);
    }
}
