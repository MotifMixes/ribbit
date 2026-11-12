//! ALSA on the Explorer: /dev/snd/controlC0 and /dev/snd/pcmC0D0p as its
//! kernel (2.6.31, ASoC with the CS43L22 codec and lf1000-pcm) answers
//! them, for libasound's "hw" plugin. The hw_params refinement is the
//! kernel's (pcm_native.c snd_pcm_hw_refine and its rules, pcm_lib.c's
//! interval arithmetic) with the runtime constraints the drivers set.
//!
//! Differences from Original, on purpose: the PCM has no mmap (access
//! RW_INTERLEAVED only), so asound.conf's dmix ("plugdmix", which needs
//! SysV shared memory) fails and Brio falls back to "plughw:0,0" as it
//! does when dmix can't open; several programs may open the PCM at once
//! and Ribbit mixes them at 32 kHz, the rate dmix mixes at on Original.
//! The hardware pointer moves with the clock, frame by frame (the DMA's
//! address, lf1000_pcm_pointer).

use super::task::Wait;
use super::{EAGAIN, EFAULT, EINVAL, ENOTTY, Process};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

/// Device numbers Ribbit gives each opened PCM (open files have state).
pub(super) const PCM: u32 = 0xFFFE_0000;
const EPIPE: i32 = 32;
const EBADFD: i32 = 77;
/// The rate Ribbit mixes the streams at (asound.emerald.conf's dmix slave).
pub(super) const OUT_RATE: u32 = 32000;

// States.
const OPEN: u32 = 0;
const SETUP: u32 = 1;
const PREPARED: u32 = 2;
const RUNNING: u32 = 3;
const XRUN: u32 = 4;
const DRAINING: u32 = 5;
const PAUSED: u32 = 6;

// hw_params layout.
const MASK_AT: usize = 4;
const INTERVAL_AT: usize = 260;
const RMASK: usize = 512;
const CMASK: usize = 516;
const INFO: usize = 520;
const MSBITS: usize = 524;
const RATE_NUM: usize = 528;
const RATE_DEN: usize = 532;
const FIFO_SIZE: usize = 536;
const HW_PARAMS_SIZE: usize = 604;

// Parameters.
const ACCESS: usize = 0;
const FORMAT: usize = 1;
const SUBFORMAT: usize = 2;
const SAMPLE_BITS: usize = 8;
const FRAME_BITS: usize = 9;
const CHANNELS: usize = 10;
const RATE: usize = 11;
const PERIOD_TIME: usize = 12;
const PERIOD_SIZE: usize = 13;
const PERIOD_BYTES: usize = 14;
const PERIODS: usize = 15;
const BUFFER_TIME: usize = 16;
const BUFFER_SIZE: usize = 17;
const BUFFER_BYTES: usize = 18;
const TICK_TIME: usize = 19;

/// SNDRV_PCM_INFO: BLOCK_TRANSFER, INTERLEAVED, PAUSE, RESUME (the
/// driver's, without MMAP).
const HW_INFO: u32 = 0x1_0000 | 0x100 | 0x8_0000 | 0x4_0000;

#[derive(Clone, Copy, PartialEq, Debug)]
struct Interval {
    min: u32,
    max: u32,
    openmin: bool,
    openmax: bool,
    integer: bool,
    empty: bool,
}

impl Interval {
    fn any() -> Self {
        Self { min: 0, max: u32::MAX, openmin: false, openmax: false, integer: false, empty: false }
    }

    fn minmax(min: u32, max: u32) -> Self {
        Self { min, max, ..Self::any() }
    }

    fn none() -> Self {
        Self { empty: true, ..Self::any() }
    }

    fn single(&self) -> bool {
        self.min == self.max || (self.min + 1 == self.max && self.openmax)
    }

    fn checkempty(&self) -> bool {
        self.min > self.max || (self.min == self.max && (self.openmin || self.openmax))
    }

    /// snd_interval_refine.
    fn refine(&mut self, v: &Interval) -> Result<bool, i32> {
        if self.empty {
            return Err(EINVAL);
        }
        let mut changed = false;
        if self.min < v.min {
            self.min = v.min;
            self.openmin = v.openmin;
            changed = true;
        } else if self.min == v.min && !self.openmin && v.openmin {
            self.openmin = true;
            changed = true;
        }
        if self.max > v.max {
            self.max = v.max;
            self.openmax = v.openmax;
            changed = true;
        } else if self.max == v.max && !self.openmax && v.openmax {
            self.openmax = true;
            changed = true;
        }
        if !self.integer && v.integer {
            self.integer = true;
            changed = true;
        }
        if self.integer {
            if self.openmin {
                self.min += 1;
                self.openmin = false;
            }
            if self.openmax {
                self.max -= 1;
                self.openmax = false;
            }
        } else if !self.openmin && !self.openmax && self.min == self.max {
            self.integer = true;
        }
        if self.checkempty() {
            *self = Self::none();
            return Err(EINVAL);
        }
        Ok(changed)
    }

    fn refine_first(&mut self) -> bool {
        if self.single() {
            return false;
        }
        self.max = self.min;
        self.openmax = self.openmin;
        if self.openmax {
            self.max += 1;
        }
        true
    }

    fn refine_last(&mut self) -> bool {
        if self.single() {
            return false;
        }
        self.min = self.max;
        self.openmin = self.openmax;
        if self.openmin {
            self.min -= 1;
        }
        true
    }

    fn read(b: &[u8]) -> Self {
        let w = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
        let f = w(2);
        Self { min: w(0), max: w(1), openmin: f & 1 != 0, openmax: f & 2 != 0, integer: f & 4 != 0, empty: f & 8 != 0 }
    }

    fn write(&self, b: &mut [u8]) {
        b[0..4].copy_from_slice(&self.min.to_le_bytes());
        b[4..8].copy_from_slice(&self.max.to_le_bytes());
        let f = self.openmin as u32 | (self.openmax as u32) << 1 | (self.integer as u32) << 2 | (self.empty as u32) << 3;
        b[8..12].copy_from_slice(&f.to_le_bytes());
    }
}

fn mul(a: u32, b: u32) -> u32 {
    if a == 0 {
        return 0;
    }
    if u32::MAX / a < b { u32::MAX } else { a * b }
}

/// (a * b / c, remainder) as the kernel's muldiv32.
fn muldiv32(a: u32, b: u32, c: u32) -> (u32, u32) {
    if c == 0 {
        return (u32::MAX, 0);
    }
    let n = a as u64 * b as u64;
    let (q, r) = (n / c as u64, (n % c as u64) as u32);
    if q >= u32::MAX as u64 { (u32::MAX, r) } else { (q as u32, r) }
}

fn i_mul(a: &Interval, b: &Interval) -> Interval {
    if a.empty || b.empty {
        return Interval::none();
    }
    Interval {
        min: mul(a.min, b.min),
        openmin: a.openmin || b.openmin,
        max: mul(a.max, b.max),
        openmax: a.openmax || b.openmax,
        integer: a.integer && b.integer,
        empty: false,
    }
}

fn i_div(a: &Interval, b: &Interval) -> Interval {
    if a.empty || b.empty {
        return Interval::none();
    }
    let mut c = Interval::any();
    let (q, r) = if b.max == 0 { (u32::MAX, 0) } else { (a.min / b.max, a.min % b.max) };
    c.min = q;
    c.openmin = r != 0 || a.openmin || b.openmax;
    if b.min > 0 {
        let (q, r) = (a.max / b.min, a.max % b.min);
        c.max = q;
        if r != 0 {
            c.max += 1;
            c.openmax = true;
        } else {
            c.openmax = a.openmax || b.openmin;
        }
    } else {
        c.max = u32::MAX;
        c.openmax = false;
    }
    c
}

fn i_muldivk(a: &Interval, b: &Interval, k: u32) -> Interval {
    if a.empty || b.empty {
        return Interval::none();
    }
    let mut c = Interval::any();
    let (q, r) = muldiv32(a.min, b.min, k);
    c.min = q;
    c.openmin = r != 0 || a.openmin || b.openmin;
    let (q, r) = muldiv32(a.max, b.max, k);
    c.max = q;
    if r != 0 {
        c.max = c.max.saturating_add(1);
        c.openmax = true;
    } else {
        c.openmax = a.openmax || b.openmax;
    }
    c
}

fn i_mulkdiv(a: &Interval, k: u32, b: &Interval) -> Interval {
    if a.empty || b.empty {
        return Interval::none();
    }
    let mut c = Interval::any();
    let (q, r) = muldiv32(a.min, k, b.max);
    c.min = q;
    c.openmin = r != 0 || a.openmin || b.openmax;
    if b.min > 0 {
        let (q, r) = muldiv32(a.max, k, b.min);
        c.max = q;
        if r != 0 {
            c.max = c.max.saturating_add(1);
            c.openmax = true;
        } else {
            c.openmax = a.openmax || b.openmin;
        }
    } else {
        c.max = u32::MAX;
        c.openmax = false;
    }
    c
}

/// One parameter set (struct snd_pcm_hw_params) being refined.
struct Params {
    masks: [[u32; 8]; 3],
    iv: [Interval; 12],
    rmask: u32,
    cmask: u32,
}

impl Params {
    fn read(b: &[u8]) -> Self {
        let mut masks = [[0u32; 8]; 3];
        for (k, m) in masks.iter_mut().enumerate() {
            for (w, v) in m.iter_mut().enumerate() {
                let at = MASK_AT + 32 * k + 4 * w;
                *v = u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
            }
        }
        let iv = std::array::from_fn(|i| Interval::read(&b[INTERVAL_AT + 12 * i..]));
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        Self { masks, iv, rmask: w(RMASK), cmask: w(CMASK) }
    }

    fn write(&self, b: &mut [u8]) {
        for (k, m) in self.masks.iter().enumerate() {
            for (w, v) in m.iter().enumerate() {
                let at = MASK_AT + 32 * k + 4 * w;
                b[at..at + 4].copy_from_slice(&v.to_le_bytes());
            }
        }
        for (i, v) in self.iv.iter().enumerate() {
            v.write(&mut b[INTERVAL_AT + 12 * i..]);
        }
        b[RMASK..RMASK + 4].copy_from_slice(&self.rmask.to_le_bytes());
        b[CMASK..CMASK + 4].copy_from_slice(&self.cmask.to_le_bytes());
    }

    fn i(&self, k: usize) -> Interval {
        self.iv[k - 8]
    }

    fn refine_i(&mut self, k: usize, v: &Interval) -> Result<bool, i32> {
        self.iv[k - 8].refine(v)
    }

    /// snd_mask_refine.
    fn refine_mask(&mut self, k: usize, v: &[u32; 8]) -> Result<bool, i32> {
        let old = self.masks[k];
        for (m, v) in self.masks[k].iter_mut().zip(v) {
            *m &= v;
        }
        if self.masks[k].iter().all(|&w| w == 0) {
            return Err(EINVAL);
        }
        Ok(old != self.masks[k])
    }
}

/// The kernel's rules (snd_pcm_hw_constraints_init): (variable, kind,
/// dependencies).
#[derive(Clone, Copy)]
enum Rule {
    Format,
    SampleBits,
    Div(usize, usize),
    Mul(usize, usize),
    MulKDiv(usize, u32, usize),
    MulDivK(usize, usize, u32),
}

const RULES: [(usize, Rule); 20] = [
    (FORMAT, Rule::Format),
    (SAMPLE_BITS, Rule::SampleBits),
    (SAMPLE_BITS, Rule::Div(FRAME_BITS, CHANNELS)),
    (FRAME_BITS, Rule::Mul(SAMPLE_BITS, CHANNELS)),
    (FRAME_BITS, Rule::MulKDiv(PERIOD_BYTES, 8, PERIOD_SIZE)),
    (FRAME_BITS, Rule::MulKDiv(BUFFER_BYTES, 8, BUFFER_SIZE)),
    (CHANNELS, Rule::Div(FRAME_BITS, SAMPLE_BITS)),
    (RATE, Rule::MulKDiv(PERIOD_SIZE, 1_000_000, PERIOD_TIME)),
    (RATE, Rule::MulKDiv(BUFFER_SIZE, 1_000_000, BUFFER_TIME)),
    (PERIODS, Rule::Div(BUFFER_SIZE, PERIOD_SIZE)),
    (PERIOD_SIZE, Rule::Div(BUFFER_SIZE, PERIODS)),
    (PERIOD_SIZE, Rule::MulKDiv(PERIOD_BYTES, 8, FRAME_BITS)),
    (PERIOD_SIZE, Rule::MulDivK(PERIOD_TIME, RATE, 1_000_000)),
    (BUFFER_SIZE, Rule::Mul(PERIOD_SIZE, PERIODS)),
    (BUFFER_SIZE, Rule::MulKDiv(BUFFER_BYTES, 8, FRAME_BITS)),
    (BUFFER_SIZE, Rule::MulDivK(BUFFER_TIME, RATE, 1_000_000)),
    (PERIOD_BYTES, Rule::MulDivK(PERIOD_SIZE, FRAME_BITS, 8)),
    (BUFFER_BYTES, Rule::MulDivK(BUFFER_SIZE, FRAME_BITS, 8)),
    (PERIOD_TIME, Rule::MulKDiv(PERIOD_SIZE, 1_000_000, RATE)),
    (BUFFER_TIME, Rule::MulKDiv(BUFFER_SIZE, 1_000_000, RATE)),
];

impl Rule {
    fn deps(&self) -> Vec<usize> {
        match *self {
            Rule::Format => vec![SAMPLE_BITS],
            Rule::SampleBits => vec![FORMAT, SAMPLE_BITS],
            Rule::Div(a, b) | Rule::Mul(a, b) | Rule::MulKDiv(a, _, b) | Rule::MulDivK(a, b, _) => vec![a, b],
        }
    }
}

/// S16_LE is the only format: physical width 16.
const S16_LE: usize = 2;

/// The runtime constraints: lf1000-pcm's hardware with the ASoC DAIs'
/// rates and channels (CS43L22 and the LF1000 I2S: continuous rates, the
/// lowest listed both have 22050, the highest 192000; 2 channels).
fn constraints() -> ([[u32; 8]; 3], [Interval; 12]) {
    let mut masks = [[0u32; 8]; 3];
    masks[ACCESS][0] = 1 << 3; // RW_INTERLEAVED
    masks[FORMAT][0] = 1 << S16_LE;
    masks[SUBFORMAT][0] = 1;
    let mut iv = [Interval::any(); 12];
    for k in [CHANNELS, BUFFER_SIZE, BUFFER_BYTES, SAMPLE_BITS, FRAME_BITS, PERIODS] {
        iv[k - 8].integer = true;
    }
    let mut minmax = |k: usize, a: u32, b: u32| {
        let _ = iv[k - 8].refine(&Interval::minmax(a, b));
    };
    minmax(CHANNELS, 2, 2);
    minmax(RATE, 22050, 192000);
    minmax(PERIOD_BYTES, 64, 16 * 1024);
    minmax(PERIODS, 1, 32);
    minmax(BUFFER_BYTES, 64, 64 * 1024);
    (masks, iv)
}

/// snd_pcm_hw_refine.
fn refine(p: &mut Params) -> Result<(), i32> {
    let (cmasks, civ) = constraints();
    for k in 0..3 {
        if p.masks[k].iter().all(|&w| w == 0) {
            return Err(EINVAL);
        }
        if p.rmask & 1 << k == 0 {
            continue;
        }
        if p.refine_mask(k, &cmasks[k])? {
            p.cmask |= 1 << k;
        }
    }
    for k in 8..20 {
        if p.i(k).empty {
            return Err(EINVAL);
        }
        if p.rmask & 1 << k == 0 {
            continue;
        }
        if p.refine_i(k, &civ[k - 8])? {
            p.cmask |= 1 << k;
        }
    }
    let mut rstamps = [0u32; RULES.len()];
    let mut vstamps = [0u32; 20];
    for (k, v) in vstamps.iter_mut().enumerate() {
        *v = (p.rmask >> k) & 1;
    }
    let mut stamp = 2;
    loop {
        let mut again = false;
        for (k, (var, rule)) in RULES.iter().enumerate() {
            if !rule.deps().iter().any(|&d| vstamps[d] > rstamps[k]) {
                continue;
            }
            let changed = match *rule {
                Rule::Format => {
                    let sb = p.i(SAMPLE_BITS);
                    let mut m = [u32::MAX; 8];
                    if !(sb.min..=sb.max).contains(&16) {
                        m[0] &= !(1 << S16_LE);
                    }
                    // Other formats aren't offered (the constraint mask
                    // already took them out).
                    p.refine_mask(FORMAT, &m)?
                }
                Rule::SampleBits => {
                    let t = if p.masks[FORMAT][0] & 1 << S16_LE != 0 {
                        Interval { min: 16, max: 16, integer: true, ..Interval::any() }
                    } else {
                        Interval { min: u32::MAX, max: 0, integer: true, ..Interval::any() }
                    };
                    p.refine_i(SAMPLE_BITS, &t)?
                }
                Rule::Div(a, b) => {
                    let t = i_div(&p.i(a), &p.i(b));
                    p.refine_i(*var, &t)?
                }
                Rule::Mul(a, b) => {
                    let t = i_mul(&p.i(a), &p.i(b));
                    p.refine_i(*var, &t)?
                }
                Rule::MulKDiv(a, kk, b) => {
                    let t = i_mulkdiv(&p.i(a), kk, &p.i(b));
                    p.refine_i(*var, &t)?
                }
                Rule::MulDivK(a, b, kk) => {
                    let t = i_muldivk(&p.i(a), &p.i(b), kk);
                    p.refine_i(*var, &t)?
                }
            };
            rstamps[k] = stamp;
            if changed {
                p.cmask |= 1 << var;
                vstamps[*var] = stamp;
                again = true;
            }
            stamp += 1;
        }
        if !again {
            break;
        }
    }
    Ok(())
}

/// One open PCM: its setup and where its pointers are.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Stream {
    state: u32,
    rate: u32,
    channels: u32,
    period_size: u32,
    buffer_size: u32,
    boundary: u32,
    avail_min: u32,
    start_threshold: u32,
    stop_threshold: u32,
    appl: u64,
    hw: u64,
    /// The frames written and not yet played (interleaved samples).
    data: VecDeque<i16>,
    /// When it last started running (the program's clock) and hw then.
    #[serde(with = "super::state::inst_opt")]
    started: Option<Instant>,
    hw_at_start: u64,
    tstamp: bool,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Sound {
    streams: HashMap<u32, Stream>,
    next: u32,
    /// The mix (stereo at OUT_RATE, summed), from `out_at` on.
    out: VecDeque<i32>,
    #[serde(with = "super::state::inst_opt")]
    out_at: Option<Instant>,
}

impl Process {
    /// A new open of /dev/snd/pcmC0D0p.
    pub(super) fn pcm_open(&mut self) -> u32 {
        let s = &mut self.x.snd;
        let id = s.next;
        s.next += 1;
        s.streams.insert(id, Stream::default());
        PCM | id
    }

    pub(super) fn pcm_close(&mut self, rdev: u32) {
        self.pcm_update(rdev & 0xFFFF);
        self.x.snd.streams.remove(&(rdev & 0xFFFF));
    }

    /// Moves a running stream's hardware pointer on to now, mixing what
    /// it played; a stream that ran dry stops (XRUN).
    fn pcm_update(&mut self, id: u32) {
        let now = self.clock();
        let Some(s) = self.x.snd.streams.get_mut(&id) else { return };
        if !matches!(s.state, RUNNING | DRAINING) {
            return;
        }
        let Some(t) = s.started else { return };
        let due = s.hw_at_start + (now.saturating_duration_since(t).as_secs_f64() * s.rate as f64) as u64;
        let to = due.min(s.appl);
        if to > s.hw {
            let n = (to - s.hw) as usize * s.channels as usize;
            let played: Vec<i16> = s.data.drain(..n.min(s.data.len())).collect();
            // When the first of them played.
            let at = t + Duration::from_secs_f64((s.hw - s.hw_at_start) as f64 / s.rate as f64);
            let (rate, ch) = (s.rate, s.channels);
            s.hw = to;
            if to == s.appl && due >= s.appl {
                s.state = if s.state == DRAINING { SETUP } else { XRUN };
                s.started = None;
            }
            self.mix(at, rate, ch, &played);
        } else if due >= s.appl {
            s.state = if s.state == DRAINING { SETUP } else { XRUN };
            s.started = None;
        }
    }

    /// Adds frames that started playing at `at` into the mix.
    fn mix(&mut self, at: Instant, rate: u32, ch: u32, samples: &[i16]) {
        let snd = &mut self.x.snd;
        let base = *snd.out_at.get_or_insert(at);
        let from = if at >= base {
            (at.duration_since(base).as_secs_f64() * OUT_RATE as f64).round() as usize
        } else {
            0
        };
        let frames = samples.len() / ch.max(1) as usize;
        let out_frames = (frames as u64 * OUT_RATE as u64 / rate.max(1) as u64) as usize;
        if snd.out.len() < 2 * (from + out_frames) {
            snd.out.resize(2 * (from + out_frames), 0);
        }
        for i in 0..out_frames {
            let src = (i as u64 * rate as u64 / OUT_RATE as u64) as usize;
            let (l, r) = if ch == 1 { (samples[src], samples[src]) } else { (samples[2 * src], samples[2 * src + 1]) };
            snd.out[2 * (from + i)] += l as i32;
            snd.out[2 * (from + i) + 1] += r as i32;
        }
    }

    /// The mixed sound up to now: (samples, rate, channels).
    pub(super) fn x_take_sound(&mut self) -> (Vec<i16>, u32, u32) {
        let ids: Vec<u32> = self.x.snd.streams.keys().copied().collect();
        for id in ids {
            self.pcm_update(id);
        }
        let now = self.clock();
        let snd = &mut self.x.snd;
        let Some(base) = snd.out_at else { return (Vec::new(), OUT_RATE, 2) };
        let ready = ((now.saturating_duration_since(base).as_secs_f64() * OUT_RATE as f64) as usize).min(snd.out.len() / 2);
        let out: Vec<i16> = snd.out.drain(..2 * ready).map(|v| v.clamp(-32768, 32767) as i16).collect();
        snd.out_at = Some(base + Duration::from_secs_f64(ready as f64 / OUT_RATE as f64));
        (out, OUT_RATE, 2)
    }

    pub(super) fn pcm_ready(&self, rdev: u32) -> (bool, bool, bool) {
        let Some(s) = self.x.snd.streams.get(&(rdev & 0xFFFF)) else { return (false, false, true) };
        match s.state {
            PREPARED | RUNNING | PAUSED => {
                let avail = s.buffer_size as u64 - (s.appl - self.pcm_hw_now(s));
                (false, avail >= s.avail_min.max(1) as u64, false)
            }
            DRAINING => (false, false, false),
            _ => (false, true, false),
        }
    }

    /// Where a stream's hardware pointer is now (without moving it).
    fn pcm_hw_now(&self, s: &Stream) -> u64 {
        match (s.state, s.started) {
            (RUNNING | DRAINING, Some(t)) => {
                (s.hw_at_start + (self.clock().saturating_duration_since(t).as_secs_f64() * s.rate as f64) as u64).min(s.appl)
            }
            _ => s.hw,
        }
    }

    pub(super) fn pcm_write(&mut self, rdev: u32, nonblock: bool, data: &[u8]) -> Result<usize, i32> {
        let (n, _) = self.pcm_writei(rdev & 0xFFFF, nonblock, data)?;
        let ch = self.x.snd.streams.get(&(rdev & 0xFFFF)).map_or(2, |s| s.channels.max(1)) as usize;
        Ok(n as usize * ch * 2)
    }

    /// Writes interleaved S16 frames; (frames taken, waits).
    fn pcm_writei(&mut self, id: u32, nonblock: bool, data: &[u8]) -> Result<(u32, bool), i32> {
        self.pcm_update(id);
        let now = self.clock();
        let s = self.x.snd.streams.get_mut(&id).ok_or(EBADFD)?;
        match s.state {
            XRUN => return Err(EPIPE),
            PREPARED | RUNNING | PAUSED => {}
            _ => return Err(EBADFD),
        }
        let frame = 2 * s.channels.max(1) as usize;
        let frames = (data.len() / frame) as u64;
        let room = s.buffer_size as u64 - (s.appl - s.hw);
        let n = frames.min(room);
        if n == 0 && frames > 0 {
            if nonblock {
                return Err(EAGAIN);
            }
            // Until a period's worth has played.
            let wait = Duration::from_secs_f64(s.avail_min.max(1) as f64 / s.rate.max(1) as f64);
            self.block = Some(Wait::Retry(now + wait.min(Duration::from_millis(10))));
            return Ok((0, true));
        }
        s.data.extend(data[..n as usize * frame].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
        s.appl += n;
        if s.state == PREPARED && s.appl - s.hw >= s.start_threshold.max(1) as u64 {
            s.state = RUNNING;
            s.started = Some(now);
            s.hw_at_start = s.hw;
        }
        Ok((n as u32, false))
    }

    fn snd_put(&mut self, at: u32, b: &[u8]) -> Result<u32, i32> {
        if self.mem().write_bytes(at, b) { Ok(0) } else { Err(EFAULT) }
    }

    /// struct snd_pcm_info for the playback PCM. GUESS: the names ASoC
    /// gives it (not read on Original).
    fn pcm_info_bytes() -> Vec<u8> {
        let mut b = vec![0u8; 288];
        b[200..204].copy_from_slice(&1u32.to_le_bytes()); // subdevices_count
        b[204..208].copy_from_slice(&1u32.to_le_bytes()); // subdevices_avail
        for (at, s) in [(16, "CS43L22 PCM"), (80, "CS43L22 PCM cs43l22-hifi-0"), (160, "subdevice #0")] {
            b[at..at + s.len()].copy_from_slice(s.as_bytes());
        }
        b
    }

    pub(super) fn pcm_ioctl(&mut self, rdev: u32, cmd: u32, arg: u32) -> Result<u32, i32> {
        let id = rdev & 0xFFFF;
        if !self.x.snd.streams.contains_key(&id) {
            return Err(EBADFD);
        }
        self.pcm_update(id);
        match cmd {
            0x8004_4100 => self.snd_put(arg, &0x0002_000Au32.to_le_bytes()), // PVERSION 2.0.10
            0x8120_4101 => self.snd_put(arg, &Self::pcm_info_bytes()),
            0x4004_4102 | 0x4004_4103 => Ok(0), // TSTAMP, TTSTAMP
            0xC25C_4110 | 0xC25C_4111 => {
                let mut b = self.mem().read_bytes(arg, HW_PARAMS_SIZE).ok_or(EFAULT)?;
                let mut p = Params::read(&b);
                if cmd == 0xC25C_4111 {
                    p.rmask = !0;
                }
                refine(&mut p)?;
                if cmd == 0xC25C_4111 {
                    // snd_pcm_hw_params_choose.
                    for v in [ACCESS, FORMAT, SUBFORMAT, CHANNELS, RATE, PERIOD_TIME, BUFFER_SIZE, TICK_TIME] {
                        let changed = if v < 8 {
                            let m = &mut p.masks[v];
                            let low = (0..256).find(|&i| m[i / 32] & 1 << (i % 32) != 0).unwrap_or(0);
                            let old = *m;
                            *m = [0; 8];
                            m[low / 32] = 1 << (low % 32);
                            old != *m
                        } else if v == BUFFER_SIZE {
                            p.iv[v - 8].refine_last()
                        } else {
                            p.iv[v - 8].refine_first()
                        };
                        if changed {
                            p.cmask |= 1 << v;
                            p.rmask = 1 << v;
                            refine(&mut p)?;
                        }
                    }
                }
                p.rmask = 0;
                p.write(&mut b);
                let w = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                if p.i(SAMPLE_BITS).single() {
                    w(&mut b, MSBITS, p.i(SAMPLE_BITS).min);
                }
                if p.i(RATE).single() {
                    w(&mut b, RATE_NUM, p.i(RATE).min);
                    w(&mut b, RATE_DEN, 1);
                }
                w(&mut b, INFO, HW_INFO);
                if p.i(CHANNELS).single() && p.masks[FORMAT][0].count_ones() == 1 {
                    w(&mut b, FIFO_SIZE, 16 / (2 * p.i(CHANNELS).min.max(1)));
                }
                if cmd == 0xC25C_4111 {
                    let s = self.x.snd.streams.get_mut(&id).unwrap();
                    s.state = SETUP;
                    s.rate = p.i(RATE).min;
                    s.channels = p.i(CHANNELS).min;
                    s.period_size = p.i(PERIOD_SIZE).min;
                    s.buffer_size = p.i(BUFFER_SIZE).min;
                    let mut boundary = s.buffer_size as u64;
                    while boundary * 2 <= i32::MAX as u64 - s.buffer_size as u64 {
                        boundary *= 2;
                    }
                    s.boundary = boundary as u32;
                    s.avail_min = s.period_size;
                    s.start_threshold = 1;
                    s.stop_threshold = s.buffer_size;
                    s.appl = 0;
                    s.hw = 0;
                    s.data.clear();
                }
                self.snd_put(arg, &b)
            }
            0x4112 => {
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                s.state = OPEN;
                Ok(0)
            }
            // SW_PARAMS: tstamp_mode, period_step, sleep_min, avail_min,
            // xfer_align, start_threshold, stop_threshold, silence
            // threshold and size, boundary.
            0xC068_4113 => {
                let b = self.mem().read_bytes(arg, 104).ok_or(EFAULT)?;
                let w = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                if s.state == OPEN {
                    return Err(EBADFD);
                }
                s.tstamp = w(0) != 0;
                s.avail_min = w(3).max(1);
                s.start_threshold = w(5);
                s.stop_threshold = w(6);
                let mut out = b.clone();
                out[16..20].copy_from_slice(&1u32.to_le_bytes()); // xfer_align
                self.snd_put(arg, &out)
            }
            // STATUS
            0x806C_4120 => {
                let s = self.x.snd.streams[&id].clone();
                let (sec, nsec) = self.now(1);
                let mut b = vec![0u8; 108];
                let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                put(&mut b, 0, s.state);
                put(&mut b, 12, sec);
                put(&mut b, 16, nsec);
                put(&mut b, 20, (s.appl % s.boundary.max(1) as u64) as u32);
                put(&mut b, 24, (s.hw % s.boundary.max(1) as u64) as u32);
                put(&mut b, 28, (s.appl - s.hw) as u32);
                put(&mut b, 32, s.buffer_size.saturating_sub((s.appl - s.hw) as u32));
                self.snd_put(arg, &b)
            }
            // DELAY
            0x8004_4121 => {
                let s = &self.x.snd.streams[&id];
                if s.state == XRUN {
                    return Err(EPIPE);
                }
                let d = (s.appl - s.hw) as u32;
                self.snd_put(arg, &d.to_le_bytes())
            }
            // HWSYNC
            0x4122 => match self.x.snd.streams[&id].state {
                XRUN => Err(EPIPE),
                _ => Ok(0),
            },
            // SYNC_PTR: flags, status (state, pad, hw_ptr, tstamp,
            // suspended_state), control (appl_ptr, avail_min).
            0xC084_4123 => {
                let mut b = self.mem().read_bytes(arg, 132).ok_or(EFAULT)?;
                let w = |b: &[u8], at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
                let flags = w(&b, 0);
                let (sec, nsec) = self.now(1);
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                let bd = s.boundary.max(1) as u64;
                // The program's appl_ptr (flag APPL clear) only moves with
                // mmap writes, which this PCM doesn't offer: kept as is.
                if flags & 4 == 0 {
                    s.avail_min = w(&b, 72).max(1);
                }
                let (state, hw, appl, avail_min) = (s.state, (s.hw % bd) as u32, (s.appl % bd) as u32, s.avail_min);
                let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                put(&mut b, 4, state);
                put(&mut b, 12, hw);
                put(&mut b, 16, sec);
                put(&mut b, 20, nsec);
                put(&mut b, 68, appl);
                put(&mut b, 72, avail_min);
                self.snd_put(arg, &b)
            }
            // PREPARE, RESET
            0x4140 | 0x4141 => {
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                if s.state == OPEN {
                    return Err(EBADFD);
                }
                if cmd == 0x4140 {
                    s.state = PREPARED;
                }
                s.data.clear();
                s.appl = s.hw;
                s.started = None;
                Ok(0)
            }
            // START
            0x4142 => {
                let now = self.clock();
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                if s.state != PREPARED {
                    return Err(EBADFD);
                }
                s.state = RUNNING;
                s.started = Some(now);
                s.hw_at_start = s.hw;
                Ok(0)
            }
            // DROP
            0x4143 => {
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                if s.state != OPEN {
                    s.state = SETUP;
                }
                s.data.clear();
                s.hw = s.appl;
                s.started = None;
                Ok(0)
            }
            // DRAIN: waits until it has all played.
            0x4144 => {
                let now = self.clock();
                let nonblock = false;
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                match s.state {
                    PREPARED if s.appl > s.hw => {
                        s.state = DRAINING;
                        s.started = Some(now);
                        s.hw_at_start = s.hw;
                    }
                    RUNNING => s.state = DRAINING,
                    DRAINING => {}
                    XRUN | PREPARED => {
                        s.state = SETUP;
                        return Ok(0);
                    }
                    _ => return Ok(0),
                }
                if s.state == DRAINING && s.hw < s.appl {
                    if nonblock {
                        return Err(EAGAIN);
                    }
                    let left = Duration::from_secs_f64((s.appl - s.hw) as f64 / s.rate.max(1) as f64);
                    self.block = Some(Wait::Retry(now + left.min(Duration::from_millis(10))));
                }
                Ok(0)
            }
            // PAUSE
            0x4004_4145 => {
                let now = self.clock();
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                if arg != 0 && s.state == RUNNING {
                    s.state = PAUSED;
                    s.started = None;
                } else if arg == 0 && s.state == PAUSED {
                    s.state = RUNNING;
                    s.started = Some(now);
                    s.hw_at_start = s.hw;
                }
                Ok(0)
            }
            // WRITEI_FRAMES: result, buffer, frames.
            0x400C_4150 => {
                let b = self.mem().read_bytes(arg, 12).ok_or(EFAULT)?;
                let w = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
                let (buf, frames) = (w(1), w(2));
                let ch = self.x.snd.streams[&id].channels.max(1);
                let data = self.mem().read_bytes(buf, (frames * ch * 2) as usize).ok_or(EFAULT)?;
                let nonblock = false;
                let (n, waits) = self.pcm_writei(id, nonblock, &data)?;
                if waits {
                    return Ok(0);
                }
                // The rest on the next call: the program loops on partial
                // writes, as on a signal.
                self.mem().put32(arg, n);
                Ok(0)
            }
            // REWIND, FORWARD: nothing moves back over played frames.
            0x4004_4146 | 0x4004_4149 => self.snd_put(arg, &0u32.to_le_bytes()).map(|_| 0),
            0x4147 => Err(super::ENOSYS), // RESUME
            0x4148 => {
                let s = self.x.snd.streams.get_mut(&id).unwrap();
                s.state = XRUN;
                s.started = None;
                Ok(0)
            }
            _ => {
                if self.logged.insert(format!("pcm ioctl {cmd:08X}")) {
                    eprintln!("[hle] pcm ioctl {cmd:08X} not handled");
                }
                Err(ENOTTY)
            }
        }
    }

    /// /dev/snd/controlC0. GUESS: the mixer name; card id, driver, name
    /// and long name are /proc/asound/cards' on Original.
    pub(super) fn ctl_ioctl(&mut self, cmd: u32, arg: u32) -> Result<u32, i32> {
        match cmd {
            0x8004_5500 => self.snd_put(arg, &0x0002_0006u32.to_le_bytes()), // PVERSION
            0x8178_5501 => {
                let mut b = vec![0u8; 376];
                for (at, s) in [(8, "DidjCS43L22"), (24, "CS43L22"), (40, "Didj-CS43L22"), (72, "Didj-CS43L22 (CS43L22)"), (168, "CS43L22")] {
                    b[at..at + s.len()].copy_from_slice(s.as_bytes());
                }
                self.snd_put(arg, &b)
            }
            // PCM_NEXT_DEVICE: device 0 after -1, then none.
            0x8004_5530 => {
                let cur = self.mem().get32(arg).ok_or(EFAULT)? as i32;
                let next: i32 = if cur < 0 { 0 } else { -1 };
                self.snd_put(arg, &next.to_le_bytes())
            }
            // PCM_INFO: playback device 0 only.
            0xC120_4531 => {
                let b = self.mem().read_bytes(arg, 12).ok_or(EFAULT)?;
                let w = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
                if w(0) != 0 || w(1) != 0 || w(2) != 0 {
                    return Err(super::vfs::ENOENT);
                }
                self.snd_put(arg, &Self::pcm_info_bytes())
            }
            0x4004_5532 => Ok(0), // PCM_PREFER_SUBDEVICE
            0xC004_5516 => self.snd_put(arg, &0u32.to_le_bytes()), // SUBSCRIBE_EVENTS
            // ELEM_LIST: no controls listed yet.
            0xC048_5510 => {
                let mut b = self.mem().read_bytes(arg, 72).ok_or(EFAULT)?;
                b[8..12].copy_from_slice(&0u32.to_le_bytes()); // used
                b[12..16].copy_from_slice(&0u32.to_le_bytes()); // count
                self.snd_put(arg, &b)
            }
            _ => {
                if self.logged.insert(format!("ctl ioctl {cmd:08X}")) {
                    eprintln!("[hle] sound control ioctl {cmd:08X} not handled");
                }
                Err(ENOTTY)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// alsa-lib's first refine (everything open) narrows to the driver's
    /// limits: 2 channels, S16_LE, 22050-192000 Hz, period 16-4096 frames.
    #[test]
    fn refine_any() {
        let mut p = Params { masks: [[!0u32; 8]; 3], iv: [Interval::any(); 12], rmask: !0, cmask: 0 };
        refine(&mut p).unwrap();
        assert_eq!((p.i(CHANNELS).min, p.i(CHANNELS).max), (2, 2));
        assert_eq!(p.masks[FORMAT][0], 1 << S16_LE);
        assert_eq!((p.i(FRAME_BITS).min, p.i(FRAME_BITS).max), (32, 32));
        assert_eq!((p.i(PERIOD_SIZE).min, p.i(PERIOD_SIZE).max), (16, 4096));
        assert_eq!((p.i(RATE).min, p.i(RATE).max), (22050, 192000));
        // A rate and period time then fix the period size.
        p.iv[RATE - 8] = Interval::minmax(32000, 32000);
        p.iv[PERIOD_TIME - 8] = Interval::minmax(32000, 32000);
        p.rmask = 1 << RATE | 1 << PERIOD_TIME;
        refine(&mut p).unwrap();
        assert_eq!((p.i(PERIOD_SIZE).min, p.i(PERIOD_SIZE).max), (1024, 1024));
    }
}

// ---- the codec: mixer controls and /dev/i2c-0 -----------------------------

/// The CS43L22 driver's mixer controls (sound/soc/codecs/cs43l22.c), in
/// the order it adds them (numid 1-4): name, left and right registers, the
/// physical value of logical 0. The switch is "Master Playback Switch"
/// (register 0x0F bits 4-7, inverted: 1 = sound on).
const CONTROLS: [(&str, usize, usize, u8); 3] =
    [("Master Playback Volume", 0x20, 0x21, 0x19), ("Headphone Playback Volume", 0x22, 0x23, 0x01), ("Speaker Playback Volume", 0x24, 0x25, 0x01)];
const SWITCH: &str = "Master Playback Switch";

/// The codec's registers after the driver's probe and rcS's volume, as
/// Original's /sys/devices/platform/soc-audio/codec_reg shows them
/// (2026-11-12): powered up, PCM 0 dB, master +3 dB, speaker -6.5 dB.
pub(super) const CODEC_BOOT: [(usize, u8); 26] = [
    (0x01, 0xE3), (0x02, 0x9E), (0x03, 0x07), (0x04, 0x50), (0x05, 0xA0), (0x06, 0x27), (0x08, 0x81), (0x09, 0x81),
    (0x0A, 0xA5), (0x0D, 0x10), (0x0E, 0x02), (0x0F, 0x0A), (0x18, 0x80), (0x19, 0x80), (0x1A, 0x00), (0x1B, 0x00),
    (0x1E, 0x3F), (0x1F, 0xFF), (0x20, 0x06), (0x21, 0x06), (0x22, 0xCD), (0x23, 0xCD), (0x24, 0xF3), (0x25, 0xF3),
    (0x00, 0x00), (0x07, 0x00),
];

impl Process {
    fn codec(&self, reg: usize) -> u8 {
        self.m.bus.i2c.codec().get(reg).copied().unwrap_or(0)
    }

    /// The control a struct snd_ctl_elem_id names (by numid, else by name).
    fn ctl_find(id: &[u8]) -> Option<usize> {
        let numid = u32::from_le_bytes(id[0..4].try_into().unwrap()) as usize;
        if (1..=4).contains(&numid) {
            return Some(numid - 1);
        }
        let end = id[16..60].iter().position(|&b| b == 0).unwrap_or(44);
        let name = std::str::from_utf8(&id[16..16 + end]).ok()?;
        CONTROLS.iter().position(|c| c.0 == name).or((name == SWITCH).then_some(3))
    }

    fn ctl_id(k: usize) -> Vec<u8> {
        let mut id = vec![0u8; 64];
        id[0..4].copy_from_slice(&(k as u32 + 1).to_le_bytes());
        id[4..8].copy_from_slice(&2u32.to_le_bytes()); // SNDRV_CTL_ELEM_IFACE_MIXER
        let name = if k < 3 { CONTROLS[k].0 } else { SWITCH };
        id[16..16 + name.len()].copy_from_slice(name.as_bytes());
        id
    }

    /// The mixer control ioctls: ELEM_LIST, ELEM_INFO, ELEM_READ, ELEM_WRITE.
    pub(super) fn ctl_elem_ioctl(&mut self, cmd: u32, arg: u32) -> Option<Result<u32, i32>> {
        let put = |p: &mut Self, at: u32, b: &[u8]| if p.mem().write_bytes(at, b) { Ok(0) } else { Err(EFAULT) };
        Some(match cmd {
            0xC048_5510 => {
                let Some(mut b) = self.mem().read_bytes(arg, 72) else { return Some(Err(EFAULT)) };
                let w = |b: &[u8], i: usize| u32::from_le_bytes(b[i..i + 4].try_into().unwrap());
                let (offset, space, pids) = (w(&b, 0) as usize, w(&b, 4) as usize, w(&b, 16));
                let used = space.min(4usize.saturating_sub(offset));
                for i in 0..used {
                    let id = Self::ctl_id(offset + i);
                    if !self.mem().write_bytes(pids + 64 * i as u32, &id) {
                        return Some(Err(EFAULT));
                    }
                }
                b[8..12].copy_from_slice(&(used as u32).to_le_bytes());
                b[12..16].copy_from_slice(&4u32.to_le_bytes());
                put(self, arg, &b)
            }
            0xC110_5511 => {
                let Some(mut b) = self.mem().read_bytes(arg, 272) else { return Some(Err(EFAULT)) };
                let Some(k) = Self::ctl_find(&b) else { return Some(Err(super::vfs::ENOENT)) };
                b[..64].copy_from_slice(&Self::ctl_id(k));
                let (kind, count, max) = if k < 3 { (2u32, 2u32, 255u32) } else { (1, 1, 1) };
                b[64..68].copy_from_slice(&kind.to_le_bytes());
                b[68..72].copy_from_slice(&3u32.to_le_bytes()); // READWRITE
                b[72..76].copy_from_slice(&count.to_le_bytes());
                b[76..80].copy_from_slice(&0u32.to_le_bytes());
                b[80..84].copy_from_slice(&0u32.to_le_bytes());
                b[84..88].copy_from_slice(&max.to_le_bytes());
                b[88..92].copy_from_slice(&0u32.to_le_bytes());
                put(self, arg, &b)
            }
            0xC2C8_5512 | 0xC2C8_5513 => {
                let Some(mut b) = self.mem().read_bytes(arg, 712) else { return Some(Err(EFAULT)) };
                let Some(k) = Self::ctl_find(&b) else { return Some(Err(super::vfs::ENOENT)) };
                let value = |b: &[u8], i: usize| i32::from_le_bytes(b[72 + 4 * i..76 + 4 * i].try_into().unwrap());
                if cmd == 0xC2C8_5513 {
                    if self.trace {
                        eprintln!("[hle] mixer control {k} written: {}, {}", value(&b, 0), value(&b, 1));
                    }
                    if k < 3 {
                        let (_, l, r, min) = CONTROLS[k];
                        self.m.bus.i2c.set_codec(l, (value(&b, 0) as u8).wrapping_add(min));
                        self.m.bus.i2c.set_codec(r, (value(&b, 1) as u8).wrapping_add(min));
                    } else {
                        let v = self.codec(0x0F) & 0x0F | if value(&b, 0) == 0 { 0xF0 } else { 0 };
                        self.m.bus.i2c.set_codec(0x0F, v);
                    }
                }
                b[..64].copy_from_slice(&Self::ctl_id(k));
                if k < 3 {
                    let (_, l, r, min) = CONTROLS[k];
                    let (lv, rv) = (self.codec(l).wrapping_sub(min) as u32, self.codec(r).wrapping_sub(min) as u32);
                    b[72..76].copy_from_slice(&lv.to_le_bytes());
                    b[76..80].copy_from_slice(&rv.to_le_bytes());
                } else {
                    let on = 1 - ((self.codec(0x0F) >> 4) & 1) as u32;
                    b[72..76].copy_from_slice(&on.to_le_bytes());
                }
                put(self, arg, &b)
            }
            _ => return None,
        })
    }

    /// /dev/i2c-0 (i2c-dev): the CS43L22 at 0x94 (8-bit address), its registers through I2C_RDWR or read/write after
    /// I2C_SLAVE; the LFP100 (0x66) isn't there.
    pub(super) fn i2c_ioctl(&mut self, cmd: u32, arg: u32) -> Result<u32, i32> {
        const ENXIO: i32 = 6;
        match cmd {
            // I2C_FUNCS: plain I2C and SMBus emulation.
            0x0705 => self.mem().put32(arg, 0x0EFF_0001).then_some(0).ok_or(EFAULT),
            0x0703 | 0x0706 => {
                self.x.i2c_addr = arg;
                Ok(0)
            }
            0x0701 | 0x0702 | 0x0708 => Ok(0), // RETRIES, TIMEOUT, PEC
            0x0707 => {
                let msgs = self.mem().get32(arg).ok_or(EFAULT)?;
                let n = self.mem().get32(arg + 4).ok_or(EFAULT)?;
                for i in 0..n {
                    let m = self.mem().read_bytes(msgs + 12 * i, 12).ok_or(EFAULT)?;
                    let addr = u16::from_le_bytes([m[0], m[1]]) as u32;
                    let flags = u16::from_le_bytes([m[2], m[3]]);
                    let len = u16::from_le_bytes([m[4], m[5]]) as usize;
                    let buf = u32::from_le_bytes(m[8..12].try_into().unwrap());
                    // The LF1000's adapter (i2c-lf1000.c) takes the 8-bit
                    // address as it goes on the bus (i2c-ctl passes 0x95).
                    if addr & 0xFE != 0x94 && addr != 0x4A {
                        return Err(ENXIO);
                    }
                    if flags & 1 != 0 {
                        let data: Vec<u8> = (0..len).map(|k| self.codec((self.x.i2c_map as usize + k) & 0x7F)).collect();
                        self.mem().write_bytes(buf, &data).then_some(()).ok_or(EFAULT)?;
                    } else {
                        let data = self.mem().read_bytes(buf, len).ok_or(EFAULT)?;
                        self.i2c_write(&data);
                    }
                }
                Ok(n)
            }
            _ => Err(super::ENOTTY),
        }
    }

    /// A write to the codec: its register address (bit 7: auto-increment),
    /// then values.
    fn i2c_write(&mut self, data: &[u8]) {
        if let Some((&map, values)) = data.split_first() {
            self.x.i2c_map = map;
            for (k, &v) in values.iter().enumerate() {
                self.m.bus.i2c.set_codec((map as usize & 0x7F) + k, v);
            }
        }
    }

    pub(super) fn i2c_read(&mut self, len: usize) -> Result<Vec<u8>, i32> {
        if self.x.i2c_addr & 0xFE != 0x94 && self.x.i2c_addr != 0x4A {
            return Err(6);
        }
        Ok((0..len).map(|k| self.codec((self.x.i2c_map as usize + k) & 0x7F)).collect())
    }

    pub(super) fn i2c_write_fd(&mut self, data: &[u8]) -> Result<usize, i32> {
        if self.x.i2c_addr & 0xFE != 0x94 && self.x.i2c_addr != 0x4A {
            return Err(6);
        }
        self.i2c_write(data);
        Ok(data.len())
    }
}
