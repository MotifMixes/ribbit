//! The MP3 decoder's accelerator at 0xD0A00000 (docs/rockit-twist.md, "MP3").
//!
//! The System's MP3 library (`LibMp3DecodeTask`) does the bitstream,
//! Huffman decoding, requantisation, stereo processing, reordering and
//! antialiasing itself, and gives the rest to this unit as two jobs per
//! granule:
//!
//! * **Job A** (+0 bit 0, started by the routine at 0x37E40): the IMDCT of
//!   one channel's 576 lines. +0 bits 3-2 the block type, bit 4 a mixed
//!   block; +0x10/+0x14 the lines (576 signed 16-bit values, Q15), +0x18/+0x1C
//!   the channel's overlap buffer (0x480 bytes), +0x20/+0x24 the output (576
//!   16-bit values: a stereo stream's two are 0x480 bytes apart).
//! * **Job B** (+4 written, 0x45 or 9, routine 0x37DA8): the polyphase
//!   synthesis of a granule into 576 stereo 16-bit samples. +0x28 and +0x38
//!   the two channels' job A outputs (the same for a mono stream), +0x30 and
//!   +0x40 the filter's state, +0x48 the samples.
//!
//! VERIFIED against a reference decoder (minimp3) on the start-up logo's
//! sound: each job A input is minimp3's spectrum after antialiasing times
//! 32768 (to within 0.5%), and the mode bits are 1 | block type << 2 for
//! the granule's block type. What the unit computes in between and how it
//! rounds are not observable here: the jobs are modelled with minimp3's
//! IMDCT and synthesis (lieff/minimp3, CC0, ported), in floating point. The
//! overlap and the filter state are kept in the buffers the driver gives,
//! as 32-bit floats (they fit: 288 in 0x480 bytes, 2 x 480 in 2 x 0x800), so
//! they live in emulated memory and in save states. Job A's output is 16
//! bits (`OUT_SCALE`). Command 9 (job B when the
//! routine's first argument is 3) is taken as 0x45: UNVERIFIED.

use crate::machine::Bus;

const BASE: u32 = 0xD0A0_0000;

impl Bus {
    /// An address the driver writes as low and high halfwords.
    fn acc_addr(&self, lo: u32) -> u32 {
        (self.latch(BASE + lo) & 0xFFFF) | (self.latch(BASE + lo + 4) & 0xFFFF) << 16
    }

    fn read_f32s(&mut self, at: u32, out: &mut [f32]) {
        for (i, v) in out.iter_mut().enumerate() {
            *v = f32::from_bits(self.read(at.wrapping_add(4 * i as u32), 4));
        }
    }

    fn write_f32s(&mut self, at: u32, v: &[f32]) {
        for (i, x) in v.iter().enumerate() {
            self.write(at.wrapping_add(4 * i as u32), x.to_bits(), 4);
        }
    }

    /// Job A: one channel's IMDCT, overlap-add and frequency inversion.
    pub(crate) fn mp3_job_a(&mut self) {
        let mode = self.latch(BASE);
        let (input, overlap_at, output) = (self.acc_addr(0x10), self.acc_addr(0x18), self.acc_addr(0x20));
        let mut gr = [0f32; 576];
        for (i, v) in gr.iter_mut().enumerate() {
            *v = self.read(input.wrapping_add(2 * i as u32), 2) as u16 as i16 as f32 / 32768.0;
        }
        let mut overlap = [0f32; 288];
        self.read_f32s(overlap_at, &mut overlap);
        let block_type = (mode >> 2) & 3;
        // A mixed block's first two subbands are long (44.1/48/32 kHz; the
        // 8 kHz MPEG 2.5 rate would make it four: UNVERIFIED).
        let long_bands = if mode & 0x10 != 0 { 2 } else { 0 };
        imdct_gr(&mut gr, &mut overlap, block_type, long_bands);
        change_sign(&mut gr);
        self.write_f32s(overlap_at, &overlap);
        for (i, v) in gr.iter().enumerate() {
            self.write(output.wrapping_add(2 * i as u32), to_q(*v) as u16 as u32, 2);
        }
    }

    /// Job B: a granule's synthesis into 576 stereo samples.
    pub(crate) fn mp3_job_b(&mut self) {
        let (left, state0, right, state1, out) =
            (self.acc_addr(0x28), self.acc_addr(0x30), self.acc_addr(0x38), self.acc_addr(0x40), self.acc_addr(0x48));
        let mut gr = vec![0f32; 1152];
        for i in 0..576u32 {
            gr[i as usize] = from_q(self.read(left.wrapping_add(2 * i), 2));
            gr[576 + i as usize] = from_q(self.read(right.wrapping_add(2 * i), 2));
        }
        let mut qmf = [0f32; 960];
        self.read_f32s(state0, &mut qmf[..480]);
        self.read_f32s(state1, &mut qmf[480..]);
        let mut pcm = [0i16; 1152];
        synth_granule(&mut qmf, &mut gr, &mut pcm);
        self.write_f32s(state0, &qmf[..480]);
        self.write_f32s(state1, &qmf[480..]);
        for (i, s) in pcm.iter().enumerate() {
            self.write(out.wrapping_add(2 * i as u32), *s as u16 as u32, 2);
        }
    }
}

/// Job A's output, 16 bits a sample: VERIFIED as the size (a stereo
/// stream's two outputs are 0x480 bytes apart, 576 x 2), with 2 bits of
/// headroom over the input's Q15 (ASSUMED: the IMDCT's output can exceed
/// the input's range; only job B reads it).
const OUT_SCALE: f32 = 8192.0;

fn to_q(v: f32) -> i16 {
    (v * OUT_SCALE).round().clamp(-32768.0, 32767.0) as i16
}

fn from_q(v: u32) -> f32 {
    v as u16 as i16 as f32 / OUT_SCALE
}

fn dct3_9(y: &mut [f32; 9]) {
    let (mut s0, mut s2, mut s4, mut s6, mut s8) = (y[0], y[2], y[4], y[6], y[8]);
    let mut t0 = s0 + s6 * 0.5;
    s0 -= s6;
    let mut t4 = (s4 + s2) * 0.939_692_6;
    let mut t2 = (s8 + s2) * 0.766_044_4;
    s6 = (s4 - s8) * 0.173_648_18;
    s4 += s8 - s2;
    s2 = s0 - s4 * 0.5;
    y[4] = s4 + s0;
    s8 = t0 - t2 + s6;
    s0 = t0 - t4 + t2;
    s4 = t0 + t4 - s6;
    let (mut s1, mut s3, mut s5, mut s7) = (y[1], y[3], y[5], y[7]);
    s3 *= 0.866_025_4;
    t0 = (s5 + s1) * 0.984_807_75;
    t4 = (s5 - s7) * 0.342_020_14;
    t2 = (s1 + s7) * 0.642_787_6;
    s1 = (s1 - s5 - s7) * 0.866_025_4;
    s5 = t0 - s3 - t2;
    s7 = t4 - s3 - t0;
    s3 = t4 + s3 - t2;
    y[0] = s4 - s7;
    y[1] = s2 + s1;
    y[2] = s0 - s3;
    y[3] = s8 + s5;
    y[5] = s8 - s5;
    y[6] = s0 + s3;
    y[7] = s2 - s1;
    y[8] = s4 + s7;
}

const TWID9: [f32; 18] = [
    0.737_277_3, 0.793_353_3, 0.843_391_45, 0.887_010_8, 0.923_879_5, 0.953_716_95, 0.976_296, 0.991_444_86, 0.999_048_2,
    0.675_590_2, 0.608_761_4, 0.537_299_6, 0.461_748_6, 0.382_683_43, 0.300_705_8, 0.216_439_6, 0.130_526_19, 0.043_619_38,
];

const MDCT_WINDOW: [[f32; 18]; 2] = [
    [
        0.999_048_2, 0.991_444_86, 0.976_296, 0.953_716_95, 0.923_879_5, 0.887_010_8, 0.843_391_45, 0.793_353_3, 0.737_277_3,
        0.043_619_38, 0.130_526_19, 0.216_439_6, 0.300_705_8, 0.382_683_43, 0.461_748_6, 0.537_299_6, 0.608_761_4, 0.675_590_2,
    ],
    [1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.991_444_86, 0.923_879_5, 0.793_353_3, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.130_526_19, 0.382_683_43, 0.608_761_4],
];

fn imdct36(gr: &mut [f32], overlap: &mut [f32], window: &[f32; 18], nbands: usize) {
    for j in 0..nbands {
        let g = &mut gr[18 * j..18 * j + 18];
        let o = &mut overlap[9 * j..9 * j + 9];
        let (mut co, mut si) = ([0f32; 9], [0f32; 9]);
        co[0] = -g[0];
        si[0] = g[17];
        for i in 0..4 {
            si[8 - 2 * i] = g[4 * i + 1] - g[4 * i + 2];
            co[1 + 2 * i] = g[4 * i + 1] + g[4 * i + 2];
            si[7 - 2 * i] = g[4 * i + 4] - g[4 * i + 3];
            co[2 + 2 * i] = -(g[4 * i + 3] + g[4 * i + 4]);
        }
        dct3_9(&mut co);
        dct3_9(&mut si);
        for k in [1, 3, 5, 7] {
            si[k] = -si[k];
        }
        for i in 0..9 {
            let ovl = o[i];
            let sum = co[i] * TWID9[9 + i] + si[i] * TWID9[i];
            o[i] = co[i] * TWID9[i] - si[i] * TWID9[9 + i];
            g[i] = ovl * window[i] - sum * window[9 + i];
            g[17 - i] = ovl * window[9 + i] + sum * window[i];
        }
    }
}

fn idct3(x0: f32, x1: f32, x2: f32) -> [f32; 3] {
    let m1 = x1 * 0.866_025_4;
    let a1 = x0 - x2 * 0.5;
    [a1 + m1, x0 + x2, a1 - m1]
}

/// `x` the 18 inputs from the first used (every third: x[0], x[3] ...).
fn imdct12(x: &[f32], dst: &mut [f32], overlap: &mut [f32]) {
    const TWID3: [f32; 6] = [0.793_353_3, 0.923_879_5, 0.991_444_86, 0.608_761_4, 0.382_683_43, 0.130_526_19];
    let co = idct3(-x[0], x[6] + x[3], x[12] + x[9]);
    let mut si = idct3(x[15], x[12] - x[9], x[6] - x[3]);
    si[1] = -si[1];
    for i in 0..3 {
        let ovl = overlap[i];
        let sum = co[i] * TWID3[3 + i] + si[i] * TWID3[i];
        overlap[i] = co[i] * TWID3[i] - si[i] * TWID3[3 + i];
        dst[i] = ovl * TWID3[2 - i] - sum * TWID3[5 - i];
        dst[5 - i] = ovl * TWID3[5 - i] + sum * TWID3[2 - i];
    }
}

fn imdct_short(gr: &mut [f32], overlap: &mut [f32], nbands: usize) {
    for j in 0..nbands {
        let g = &mut gr[18 * j..18 * j + 18];
        let o = &mut overlap[9 * j..9 * j + 9];
        let mut tmp = [0f32; 18];
        tmp.copy_from_slice(g);
        g[..6].copy_from_slice(&o[..6]);
        let (o_lo, o_hi) = o.split_at_mut(6);
        imdct12(&tmp, &mut g[6..12], o_hi);
        imdct12(&tmp[1..], &mut g[12..18], o_hi);
        imdct12(&tmp[2..], o_lo, o_hi);
    }
}

fn imdct_gr(gr: &mut [f32; 576], overlap: &mut [f32; 288], block_type: u32, long_bands: usize) {
    if long_bands > 0 {
        imdct36(gr, overlap, &MDCT_WINDOW[0], long_bands);
    }
    let (g, o) = (&mut gr[18 * long_bands..], &mut overlap[9 * long_bands..]);
    if block_type == 2 {
        imdct_short(g, o, 32 - long_bands);
    } else {
        imdct36(g, o, &MDCT_WINDOW[(block_type == 3) as usize], 32 - long_bands);
    }
}

fn change_sign(gr: &mut [f32; 576]) {
    for b in 0..16 {
        let base = 18 + 36 * b;
        for i in (1..18).step_by(2) {
            gr[base + i] = -gr[base + i];
        }
    }
}

/// minimp3's DCT-II over the 32 subbands, for each of the `n` time slots.
fn dct_ii(gr: &mut [f32], n: usize) {
    const SEC: [f32; 24] = [
        10.190_008, 0.500_603, 0.502_419_3, 3.407_608_5, 0.505_470_9, 0.522_498_6, 2.057_781, 0.515_447_3, 0.566_944_06,
        1.484_164_6, 0.531_042_6, 0.646_821_8, 1.169_439_9, 0.553_103_9, 0.788_154_6, 0.972_568_2, 0.582_935, 1.060_677_6,
        0.839_349_6, 0.622_504_1, 1.722_447_2, 0.744_536_3, 0.674_808_3, 5.101_148_6,
    ];
    for k in 0..n {
        let mut t = [[0f32; 8]; 4];
        let y = |i: usize| k + 18 * i;
        for i in 0..8 {
            let (x0, x1, x2, x3) = (gr[y(i)], gr[y(15 - i)], gr[y(16 + i)], gr[y(31 - i)]);
            let (t0, t1) = (x0 + x3, x1 + x2);
            let t2 = (x1 - x2) * SEC[3 * i];
            let t3 = (x0 - x3) * SEC[3 * i + 1];
            t[0][i] = t0 + t1;
            t[1][i] = (t0 - t1) * SEC[3 * i + 2];
            t[2][i] = t3 + t2;
            t[3][i] = (t3 - t2) * SEC[3 * i + 2];
        }
        for x in t.iter_mut() {
            let (mut x0, mut x1, mut x2, mut x3, mut x4, mut x5, mut x6, mut x7) = (x[0], x[1], x[2], x[3], x[4], x[5], x[6], x[7]);
            let mut xt = x0 - x7;
            x0 += x7;
            x7 = x1 - x6;
            x1 += x6;
            x6 = x2 - x5;
            x2 += x5;
            x5 = x3 - x4;
            x3 += x4;
            x4 = x0 - x3;
            x0 += x3;
            x3 = x1 - x2;
            x1 += x2;
            x[0] = x0 + x1;
            x[4] = (x0 - x1) * 0.707_106_77;
            x5 += x6;
            x6 = (x6 + x7) * 0.707_106_77;
            x7 += xt;
            x3 = (x3 + x4) * 0.707_106_77;
            x5 -= x7 * 0.198_912_37;
            x7 += x5 * 0.382_683_43;
            x5 -= x7 * 0.198_912_37;
            x0 = xt - x6;
            xt += x6;
            x[1] = (xt + x7) * 0.509_795_6;
            x[2] = (x4 + x3) * 0.541_196_1;
            x[3] = (x0 - x5) * 0.601_344_9;
            x[5] = (x0 + x5) * 0.899_976_2;
            x[6] = (x4 - x3) * 1.306_563;
            x[7] = (xt - x7) * 2.562_915_6;
        }
        let mut at = 0;
        for i in 0..7 {
            gr[y(at)] = t[0][i];
            gr[y(at + 1)] = t[2][i] + t[3][i] + t[3][i + 1];
            gr[y(at + 2)] = t[1][i] + t[1][i + 1];
            gr[y(at + 3)] = t[2][i + 1] + t[3][i] + t[3][i + 1];
            at += 4;
        }
        gr[y(at)] = t[0][7];
        gr[y(at + 1)] = t[2][7] + t[3][7];
        gr[y(at + 2)] = t[1][7];
        gr[y(at + 3)] = t[3][7];
    }
}

fn scale_pcm(s: f32) -> i16 {
    if s >= 32766.5 {
        return 32767;
    }
    if s <= -32767.5 {
        return -32768;
    }
    let v = (s + 0.5) as i16;
    v - (v < 0) as i16
}

/// Two output samples from the filter memory at `z` (offset into `lins`).
fn synth_pair(pcm: &mut [i16], at: usize, nch: usize, lins: &[f32], z: usize) {
    let zf = |k: usize| lins[z + k];
    let mut a = (zf(14 * 64) - zf(0)) * 29.0;
    a += (zf(64) + zf(13 * 64)) * 213.0;
    a += (zf(12 * 64) - zf(2 * 64)) * 459.0;
    a += (zf(3 * 64) + zf(11 * 64)) * 2037.0;
    a += (zf(10 * 64) - zf(4 * 64)) * 5153.0;
    a += (zf(5 * 64) + zf(9 * 64)) * 6574.0;
    a += (zf(8 * 64) - zf(6 * 64)) * 37489.0;
    a += zf(7 * 64) * 75038.0;
    pcm[at] = scale_pcm(a);
    let z2 = |k: usize| lins[z + 2 + k];
    let mut b = z2(14 * 64) * 104.0;
    b += z2(12 * 64) * 1567.0;
    b += z2(10 * 64) * 9727.0;
    b += z2(8 * 64) * 64019.0;
    b += z2(6 * 64) * -9975.0;
    b += z2(4 * 64) * -45.0;
    b += z2(2 * 64) * 146.0;
    b += z2(0) * -5.0;
    pcm[at + 16 * nch] = scale_pcm(b);
}

#[rustfmt::skip]
const WIN: [f32; 240] = [
    -1.,26.,-31.,208.,218.,401.,-519.,2063.,2000.,4788.,-5517.,7134.,5959.,35640.,-39336.,74992.,
    -1.,24.,-35.,202.,222.,347.,-581.,2080.,1952.,4425.,-5879.,7640.,5288.,33791.,-41176.,74856.,
    -1.,21.,-38.,196.,225.,294.,-645.,2087.,1893.,4063.,-6237.,8092.,4561.,31947.,-43006.,74630.,
    -1.,19.,-41.,190.,227.,244.,-711.,2085.,1822.,3705.,-6589.,8492.,3776.,30112.,-44821.,74313.,
    -1.,17.,-45.,183.,228.,197.,-779.,2075.,1739.,3351.,-6935.,8840.,2935.,28289.,-46617.,73908.,
    -1.,16.,-49.,176.,228.,153.,-848.,2057.,1644.,3004.,-7271.,9139.,2037.,26482.,-48390.,73415.,
    -2.,14.,-53.,169.,227.,111.,-919.,2032.,1535.,2663.,-7597.,9389.,1082.,24694.,-50137.,72835.,
    -2.,13.,-58.,161.,224.,72.,-991.,2001.,1414.,2330.,-7910.,9592.,70.,22929.,-51853.,72169.,
    -2.,11.,-63.,154.,221.,36.,-1064.,1962.,1280.,2006.,-8209.,9750.,-998.,21189.,-53534.,71420.,
    -2.,10.,-68.,147.,215.,2.,-1137.,1919.,1131.,1692.,-8491.,9863.,-2122.,19478.,-55178.,70590.,
    -3.,9.,-73.,139.,208.,-29.,-1210.,1870.,970.,1388.,-8755.,9935.,-3300.,17799.,-56778.,69679.,
    -3.,8.,-79.,132.,200.,-57.,-1283.,1817.,794.,1095.,-8998.,9966.,-4533.,16155.,-58333.,68692.,
    -4.,7.,-85.,125.,189.,-83.,-1356.,1759.,605.,814.,-9219.,9959.,-5818.,14548.,-59838.,67629.,
    -4.,7.,-91.,117.,177.,-106.,-1428.,1698.,402.,545.,-9416.,9916.,-7154.,12980.,-61289.,66494.,
    -5.,6.,-97.,111.,163.,-127.,-1498.,1634.,185.,288.,-9585.,9838.,-8540.,11455.,-62684.,65290.,
];

/// minimp3's `mp3d_synth`: 64 samples a channel from the slots at `xl`
/// (offset into `gr`), filter memory `lins` from `base`.
fn synth(gr: &[f32], xl: usize, pcm: &mut [i16], dst: usize, nch: usize, lins: &mut [f32], base: usize) {
    let xr = xl + 576 * (nch - 1);
    let (dstl, dstr) = (dst, dst + (nch - 1));
    let zlin = base + 15 * 64;
    lins[zlin + 4 * 15] = gr[xl + 18 * 16];
    lins[zlin + 4 * 15 + 1] = gr[xr + 18 * 16];
    lins[zlin + 4 * 15 + 2] = gr[xl];
    lins[zlin + 4 * 15 + 3] = gr[xr];
    lins[zlin + 4 * 31] = gr[xl + 1 + 18 * 16];
    lins[zlin + 4 * 31 + 1] = gr[xr + 1 + 18 * 16];
    lins[zlin + 4 * 31 + 2] = gr[xl + 1];
    lins[zlin + 4 * 31 + 3] = gr[xr + 1];
    synth_pair(pcm, dstr, nch, lins, base + 4 * 15 + 1);
    synth_pair(pcm, dstr + 32 * nch, nch, lins, base + 4 * 15 + 64 + 1);
    synth_pair(pcm, dstl, nch, lins, base + 4 * 15);
    synth_pair(pcm, dstl + 32 * nch, nch, lins, base + 4 * 15 + 64);
    let mut w = 0;
    for i in (0..15).rev() {
        let (mut a, mut b) = ([0f32; 4], [0f32; 4]);
        lins[zlin + 4 * i] = gr[xl + 18 * (31 - i)];
        lins[zlin + 4 * i + 1] = gr[xr + 18 * (31 - i)];
        lins[zlin + 4 * i + 2] = gr[xl + 1 + 18 * (31 - i)];
        lins[zlin + 4 * i + 3] = gr[xr + 1 + 18 * (31 - i)];
        lins[zlin + 4 * (i + 16)] = gr[xl + 1 + 18 * (1 + i)];
        lins[zlin + 4 * (i + 16) + 1] = gr[xr + 1 + 18 * (1 + i)];
        lins[zlin + 4 * i - 64 + 2] = gr[xl + 18 * (1 + i)];
        lins[zlin + 4 * i - 64 + 3] = gr[xr + 18 * (1 + i)];
        // S0(0) S2(1) S1(2) S2(3) S1(4) S2(5) S1(6) S2(7)
        for k in 0..8 {
            let (w0, w1) = (WIN[w], WIN[w + 1]);
            w += 2;
            let vz = zlin + 4 * i - k * 64;
            let vy = zlin + 4 * i - (15 - k) * 64;
            for j in 0..4 {
                let (z, y) = (lins[vz + j], lins[vy + j]);
                let (db, da) = (z * w1 + y * w0, if k % 2 == 1 { y * w1 - z * w0 } else { z * w0 - y * w1 });
                if k == 0 {
                    b[j] = db;
                    a[j] = da;
                } else {
                    b[j] += db;
                    a[j] += da;
                }
            }
        }
        pcm[dstr + (15 - i) * nch] = scale_pcm(a[1]);
        pcm[dstr + (17 + i) * nch] = scale_pcm(b[1]);
        pcm[dstl + (15 - i) * nch] = scale_pcm(a[0]);
        pcm[dstl + (17 + i) * nch] = scale_pcm(b[0]);
        pcm[dstr + (47 - i) * nch] = scale_pcm(a[3]);
        pcm[dstr + (49 + i) * nch] = scale_pcm(b[3]);
        pcm[dstl + (47 - i) * nch] = scale_pcm(a[2]);
        pcm[dstl + (49 + i) * nch] = scale_pcm(b[2]);
    }
}

/// minimp3's `mp3d_synth_granule` for two channels (18 slots).
fn synth_granule(qmf: &mut [f32; 960], gr: &mut [f32], pcm: &mut [i16; 1152]) {
    const NCH: usize = 2;
    const NBANDS: usize = 18;
    dct_ii(&mut gr[..576], NBANDS);
    dct_ii(&mut gr[576..], NBANDS);
    let mut lins = vec![0f32; (NBANDS + 15) * 64];
    lins[..960].copy_from_slice(qmf);
    for i in (0..NBANDS).step_by(2) {
        synth(gr, i, pcm, 32 * NCH * i, NCH, &mut lins, i * 64);
    }
    qmf.copy_from_slice(&lins[NBANDS * 64..NBANDS * 64 + 960]);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A full-scale line in subband 0 makes a tone, not silence or noise.
    #[test]
    fn a_line_becomes_sound() {
        let mut qmf = [0f32; 960];
        let mut overlap = [0f32; 288];
        let mut energy = 0f64;
        for _ in 0..4 {
            let mut gr = [0f32; 576];
            gr[2] = 0.25;
            imdct_gr(&mut gr, &mut overlap, 0, 0);
            change_sign(&mut gr);
            let mut both = vec![0f32; 1152];
            both[..576].copy_from_slice(&gr);
            both[576..].copy_from_slice(&gr);
            let mut pcm = [0i16; 1152];
            synth_granule(&mut qmf, &mut both, &mut pcm);
            energy += pcm.iter().map(|&s| (s as f64).powi(2)).sum::<f64>();
            // Mono in, the same on both sides out.
            assert!(pcm.chunks(2).all(|p| p[0] == p[1]));
        }
        assert!(energy > 1e6, "energy {energy}");
    }
}

#[cfg(test)]
mod reference {
    use super::*;

    /// Feeds a reference decoder's spectra (after antialiasing; minimp3
    /// with dump hooks, docs/rockit-twist.md "MP3") through the model and
    /// compares the samples with that decoder's own output.
    /// RIBBIT_MP3_DUMP=<dump> RIBBIT_MP3_PCM=<pcm> cargo test -p ribbit-twist --release -- --ignored matches_a_reference
    #[test]
    #[ignore]
    fn matches_a_reference() {
        let dump = std::fs::read(std::env::var("RIBBIT_MP3_DUMP").expect("RIBBIT_MP3_DUMP")).unwrap();
        let want: Vec<i16> = std::fs::read(std::env::var("RIBBIT_MP3_PCM").expect("RIBBIT_MP3_PCM"))
            .unwrap()
            .chunks(2)
            .map(|b| i16::from_le_bytes([b[0], b[1]]))
            .collect();
        let int = |at: usize| i32::from_le_bytes(dump[at..at + 4].try_into().unwrap());
        let (mut at, mut granules) = (0, Vec::new());
        while at + 16 <= dump.len() {
            let (stage, ch, bt, n) = (int(at), int(at + 4), int(at + 8), int(at + 12) as usize);
            at += 16;
            let x: Vec<f32> = (0..n).map(|i| f32::from_le_bytes(dump[at + 4 * i..at + 4 * i + 4].try_into().unwrap())).collect();
            at += 4 * n;
            if stage == 3 {
                granules.push((ch, bt as u32, x));
            }
        }
        let channels = if granules.len() > 1 && granules[1].0 == 1 { 2 } else { 1 };
        let mut overlap = [[0f32; 288]; 2];
        let mut qmf = [0f32; 960];
        let (mut got, mut worst) = (Vec::new(), 0f64);
        for g in granules.chunks(channels).take(400) {
            let mut both = vec![0f32; 1152];
            for (c, (_, bt, x)) in g.iter().enumerate() {
                let mut gr = [0f32; 576];
                // As the System hands them over: Q15, 16 bits.
                for (o, v) in gr.iter_mut().zip(x) {
                    *o = ((v * 32768.0).round().clamp(-32768.0, 32767.0)) / 32768.0;
                }
                imdct_gr(&mut gr, &mut overlap[c], *bt, 0);
                change_sign(&mut gr);
                for (o, v) in both[576 * c..576 * c + 576].iter_mut().zip(gr) {
                    *o = from_q(to_q(v) as u16 as u32);
                }
            }
            if channels == 1 {
                let (a, b) = both.split_at_mut(576);
                b.copy_from_slice(a);
            }
            let mut pcm = [0i16; 1152];
            synth_granule(&mut qmf, &mut both, &mut pcm);
            got.extend_from_slice(&pcm);
        }
        // The reference's own samples (interleaved when stereo).
        let n = got.len().min(want.len() * 2 / channels);
        let (mut sig, mut noise) = (0f64, 0f64);
        for i in 0..n / 2 {
            for c in 0..channels {
                let w = want[i * channels + c] as f64;
                let g = got[2 * i + c] as f64;
                sig += w * w;
                noise += (w - g) * (w - g);
                worst = worst.max((w - g).abs());
            }
        }
        let snr = 10.0 * (sig / noise.max(1.0)).log10();
        println!("{channels} channel(s), {} granules: SNR {snr:.1} dB, worst {worst}", got.len() / 1152);
        assert!(snr > 30.0, "SNR {snr:.1} dB");
    }
}
