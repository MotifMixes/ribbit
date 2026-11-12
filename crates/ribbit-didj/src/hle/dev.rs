//! The HLE's device files (/dev): the Didj's buttons as Linux input
//! events, the OSS sound device, and the small character devices
//! (null, zero, random), and the display (mlc.rs). 3D and the rest come
//! later (phase 4).
//!
//! Device numbers and names VERIFIED on Original (the real kernel in
//! Ribbit, `ls -l /dev` and /sys/class/input, 2026-11-07): event0 is
//! "LF1000 USB", event1 "Power Button", event2 "LF1000 Keyboard";
//! event3 has a node but no device.

use super::task::Wait;
use super::{EAGAIN, EFAULT, EINVAL, ENOTTY, Process};
use std::collections::VecDeque;
use std::time::{Duration, Instant};

const ENODEV: i32 = 19;

pub const INPUT_NAMES: [&str; 3] = ["LF1000 USB", "Power Button", "LF1000 Keyboard"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(super) enum Device {
    Null,
    Zero,
    Random,
    /// /dev/input/eventN.
    Input(usize),
    /// /dev/dsp (OSS).
    Dsp,
    /// /dev/ga3d (the 3D chip's registers) and /dev/mem: mmap only, onto
    /// physical addresses.
    Ga3d,
    PhysMem,
    /// /dev/mlc, the display controller (mlc.rs).
    Mlc,
    /// /dev/layerN, one of its layers.
    Layer(u8),
    /// /dev/dpc, the display's timing and the LCD's backlight.
    Dpc,
    /// A node Ribbit doesn't model yet: (major << 8) | minor.
    Other(u32),
}

impl Device {
    /// The device a character node `rdev` opens.
    pub fn open(rdev: u32) -> Result<Self, i32> {
        Ok(match (rdev >> 8, rdev & 0xFF) {
            (1, 1) => Device::PhysMem,
            (1, 3) => Device::Null,
            (249, 0) => Device::Ga3d,
            (1, 5) => Device::Zero,
            (1, 8 | 9) => Device::Random,
            (13, m @ 64..=66) => Device::Input(m as usize - 64),
            (13, _) => return Err(ENODEV),
            (14, 3) => Device::Dsp,
            (super::mlc::MAJOR, 0) => Device::Mlc,
            (super::mlc::LAYER_MAJOR, m) if m < super::mlc::LAYERS as u32 => Device::Layer(m as u8),
            (252, 0) => Device::Dpc,
            _ => Device::Other(rdev),
        })
    }
}

/// The OSS output: format and how far ahead of the clock the program
/// has written.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Dsp {
    pub rate: u32,
    pub channels: u32,
    /// AFMT_*; only S16_LE (0x10) is played.
    pub format: u32,
    /// Sound written, waiting for the host (16-bit samples, interleaved).
    pub out: VecDeque<i16>,
    /// When the first sample was written and how many bytes since.
    #[serde(with = "super::state::inst_opt")]
    started: Option<Instant>,
    written: u64,
}

/// The LF1000 driver's buffer (i2s/main.c AUDIO_FRAG_SIZE,
/// AUDIO_NUM_FRAGS): 8 fragments of 4096 bytes.
const FRAG_SIZE: u32 = 4096;
const FRAGS: u32 = 8;

impl Default for Dsp {
    fn default() -> Self {
        Self { rate: 32000, channels: 2, format: 0x10, out: VecDeque::new(), started: None, written: 0 }
    }
}

impl Dsp {
    fn bytes_per_second(&self) -> u64 {
        self.rate as u64 * self.channels as u64 * 2
    }

    /// Bytes written but not yet played by the clock. The DMA takes a
    /// whole fragment at a time, so room comes in fragments.
    fn queued(&self, now: Instant) -> u64 {
        let Some(t) = self.started else { return 0 };
        let played = (now.duration_since(t).as_secs_f64() * self.bytes_per_second() as f64) as u64;
        self.written.saturating_sub(played - played % FRAG_SIZE as u64)
    }

    fn reset(&mut self) {
        self.started = None;
        self.written = 0;
    }
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Devices {
    /// Events waiting on /dev/input/event0..2 (struct input_event, 16 bytes).
    pub input: [VecDeque<[u8; 16]>; 3],
    /// Which keys each input device has down, for EVIOCGKEY (codes below 256).
    pub keys: [[u8; 32]; 3],
    /// Which switches each has on, for EVIOCGSW (codes below 8). The
    /// keypad's SW_TABLET_MODE is the cartridge detect.
    pub switches: [u8; 3],
    pub dsp: Dsp,
    pub dpc: Dpc,
}

/// What /dev/dpc keeps (drivers/lf1000/dpc/main.c): the values are only
/// stored and read back, as Ribbit doesn't draw the backlight (nor in
/// Original). AppManager dims the screen this way before its idle
/// shutdown and restores it after.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Dpc {
    /// The backlight in the kernel's "virtual" range -128..127 (0 the
    /// middle); the physical PWM value is 2v + 256 on board 3.
    pub backlight: i32,
    pub brightness: u32,
    pub contrast: u32,
}

/// The keypad's key codes (drivers/input/keyboard/lf1000-keypad.c), on
/// /dev/input/event2: Up, Down, Left, Right, A, B, L, R, Menu (M),
/// Hint (H), Pause (P), Brightness (X).
pub mod key {
    pub const UP: u16 = 103;
    pub const DOWN: u16 = 108;
    pub const LEFT: u16 = 105;
    pub const RIGHT: u16 = 106;
    pub const A: u16 = 30;
    pub const B: u16 = 48;
    pub const L: u16 = 38;
    pub const R: u16 = 19;
    pub const MENU: u16 = 50;
    pub const HINT: u16 = 35;
    pub const PAUSE: u16 = 25;
    pub const BRIGHTNESS: u16 = 45;

    /// A key by its name in DIDJ_HLE_KEYS-style lists (up, a, menu, ...).
    pub fn by_name(name: &str) -> Option<u16> {
        Some(match name.to_ascii_lowercase().as_str() {
            "up" => UP,
            "down" => DOWN,
            "left" => LEFT,
            "right" => RIGHT,
            "a" => A,
            "b" => B,
            "l" => L,
            "r" => R,
            "menu" | "m" => MENU,
            "hint" | "h" => HINT,
            "pause" | "p" => PAUSE,
            "brightness" | "x" => BRIGHTNESS,
            _ => return None,
        })
    }
}

/// A cartridge's files in `vfs`: its volume at /Cart, as rcS mounts it, and
/// the kernel's files reporting a production cartridge.
pub fn cart_files(vfs: &mut super::Vfs, raw: &[u8]) -> Result<(), String> {
    let files = crate::cart::files(raw).ok_or("no cartridge volume in the dump")?;
    vfs.mkdir_all("/Cart");
    for (path, data) in files {
        let full = format!("/Cart/{}", path.trim_matches('/'));
        if path.ends_with('/') {
            vfs.mkdir_all(&full);
        } else {
            vfs.add_file(&full, data);
        }
    }
    super::sysfs::cart_in(vfs);
    Ok(())
}

impl Process {
    /// A cartridge in the slot from power-on (`raw` its dump): its volume
    /// at /Cart as rcS mounts it, the kernel's files reporting a production
    /// cartridge, and the keypad's cartridge switch (SW_TABLET_MODE, pin
    /// A18 low; lf1000-keypad.c) on.
    pub fn insert_cart(&mut self, raw: &[u8]) -> Result<(), String> {
        cart_files(&mut self.vfs, raw)?;
        const SW_TABLET_MODE: u8 = 1;
        self.dev.switches[2] |= SW_TABLET_MODE;
        Ok(())
    }

    /// A key going down or up on the keypad (/dev/input/event2), as the
    /// driver reports it: EV_KEY then EV_SYN, stamped with the time of day.
    pub fn key(&mut self, code: u16, down: bool) {
        if self.x.on {
            return self.x_key(code, down);
        }
        const KEYPAD: usize = 2;
        let bits = &mut self.dev.keys[KEYPAD][code as usize / 8];
        let bit = 1 << (code % 8);
        if (*bits & bit != 0) == down {
            return;
        }
        *bits ^= bit;
        let (sec, nsec) = self.now(0);
        let event = |kind: u16, code: u16, value: i32| {
            let mut e = [0u8; 16];
            e[0..4].copy_from_slice(&sec.to_le_bytes());
            e[4..8].copy_from_slice(&(nsec / 1000).to_le_bytes());
            e[8..10].copy_from_slice(&kind.to_le_bytes());
            e[10..12].copy_from_slice(&code.to_le_bytes());
            e[12..16].copy_from_slice(&value.to_le_bytes());
            e
        };
        let q = &mut self.dev.input[KEYPAD];
        q.push_back(event(1, code, down as i32)); // EV_KEY
        q.push_back(event(0, 0, 0)); // EV_SYN SYN_REPORT
    }

    pub(super) fn dev_read(&mut self, d: Device, nonblock: bool, len: usize) -> Result<Vec<u8>, i32> {
        match d {
            Device::Null => Ok(Vec::new()),
            Device::Zero => Ok(vec![0; len]),
            Device::Random => {
                // xorshift from the clock: the programs only seed with it.
                let mut x = self.since_start().as_nanos() as u64 | 1;
                Ok((0..len)
                    .map(|_| {
                        x ^= x << 13;
                        x ^= x >> 7;
                        x ^= x << 17;
                        x as u8
                    })
                    .collect())
            }
            Device::Input(n) => {
                let q = &mut self.dev.input[n];
                if q.is_empty() {
                    if nonblock {
                        return Err(EAGAIN);
                    }
                    self.block = Some(Wait::Input(n));
                    return Ok(Vec::new());
                }
                let mut out = Vec::new();
                while out.len() + 16 <= len
                    && let Some(e) = q.pop_front()
                {
                    out.extend(e);
                }
                if out.is_empty() { Err(EINVAL) } else { Ok(out) }
            }
            Device::Dsp => Ok(Vec::new()),
            Device::Other(rdev) if self.x.on => self.x_read(rdev, nonblock, len),
            Device::Other(_) | Device::Dpc | Device::Mlc | Device::Layer(_) | Device::Ga3d | Device::PhysMem => Ok(Vec::new()),
        }
    }

    pub(super) fn dev_write(&mut self, d: Device, nonblock: bool, data: &[u8]) -> Result<usize, i32> {
        if let Device::Layer(n) = d {
            // mlc_layer_write copies into the frame buffer at the file
            // position; not done yet.
            self.log_once(format!("write to /dev/layer{n}: dropped"));
        }
        if let Device::Other(rdev) = d
            && self.x.on
        {
            return self.x_write(rdev, nonblock, data);
        }
        if d != Device::Dsp {
            return Ok(data.len());
        }
        let now = self.clock();
        let dsp = &mut self.dev.dsp;
        let size = (FRAG_SIZE * FRAGS) as u64;
        let room = size - dsp.queued(now).min(size);
        // Non-blocking (PortAudio primes the buffer with silence until
        // EAGAIN): what fits. Blocking: as the driver's audio_write, one
        // fragment's worth (at most 4096 bytes) once one is free.
        let n = if nonblock { (data.len() as u64).min(room) } else { (data.len() as u64).min(FRAG_SIZE as u64) } as usize & !1;
        if nonblock && n == 0 {
            return Err(EAGAIN);
        }
        if room < n as u64 {
            // Sleep until enough has played, then write again.
            let wait = Duration::from_secs_f64((n as u64 - room) as f64 / dsp.bytes_per_second() as f64);
            self.block = Some(Wait::DspFull(now + wait));
            return Ok(0);
        }
        if dsp.started.is_none() || dsp.queued(now) == 0 {
            dsp.started = Some(now);
            dsp.written = 0;
        }
        dsp.written += n as u64;
        if dsp.format == 0x10 {
            dsp.out.extend(data[..n].chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
        }
        Ok(n)
    }

    /// (readable, writable, hung up) for poll and select.
    pub(super) fn dev_ready(&self, d: Device) -> (bool, bool, bool) {
        match d {
            Device::Input(n) => (!self.dev.input[n].is_empty(), true, false),
            // Writable with a whole fragment free, as OSS reports it.
            Device::Dsp => (false, self.dev.dsp.queued(self.clock()) <= (FRAG_SIZE * (FRAGS - 1)) as u64, false),
            Device::Other(rdev) if self.x.on => self.x_ready(rdev),
            _ => (true, true, false),
        }
    }

    /// /dev/dpc's ioctls (dpc_ioctl.h, magic 'd'): the settings programs
    /// change; the timing ones (sync, clocks, mode) are taken and ignored.
    fn dpc_ioctl(&mut self, cmd: u32, arg: u32) -> Result<u32, i32> {
        let dpc = &mut self.dev.dpc;
        match cmd & 0xFFFF {
            0x6400..=0x6406 => Ok(0),
            0x6407 => {
                dpc.contrast = arg;
                Ok(0)
            }
            0x6408 => {
                dpc.brightness = arg;
                Ok(0)
            }
            // DPC_IOCTBACKLIGHT / QBACKLIGHT: the physical value.
            0x640A => {
                dpc.backlight = (arg as i32 - 256) / 2;
                Ok(0)
            }
            0x640B => Ok((dpc.backlight * 2 + 256) as u32),
            0x640C => Ok(dpc.brightness),
            0x640D => Ok(dpc.contrast),
            // DPC_IOCTBACKLIGHTVIRT: the value itself, -128..127.
            0x640E => {
                let v = arg as i32;
                if !(-128..=127).contains(&v) {
                    return Err(EINVAL);
                }
                dpc.backlight = v;
                Ok(0)
            }
            // DPC_IOCQBACKLIGHTVIRT: written to the argument.
            0x640F => {
                let v = dpc.backlight as u32;
                if self.mem().put32(arg, v) { Ok(0) } else { Err(EFAULT) }
            }
            _ => {
                if self.logged.insert(format!("ioctl Dpc {cmd:08X}")) {
                    eprintln!("[hle] dpc ioctl {cmd:08X} not handled");
                }
                Err(ENOTTY)
            }
        }
    }

    pub(super) fn dev_ioctl(&mut self, d: Device, cmd: u32, arg: u32) -> Result<u32, i32> {
        match d {
            Device::Input(n) => self.evdev_ioctl(n, cmd, arg),
            Device::Dsp => self.dsp_ioctl(cmd, arg),
            Device::Mlc => self.mlc_ioctl(cmd, arg),
            Device::Layer(n) => self.layer_ioctl(n, cmd, arg),
            Device::Dpc => self.dpc_ioctl(cmd, arg),
            Device::Other(rdev) if self.x.on => self.x_ioctl(rdev, cmd, arg),
            _ => {
                if self.logged.insert(format!("ioctl {d:?} {cmd:08X}")) {
                    eprintln!("[hle] ioctl {cmd:08X} on {d:?} not handled");
                }
                Err(ENOTTY)
            }
        }
    }

    fn put(&mut self, at: u32, bytes: &[u8]) -> Result<u32, i32> {
        if self.mem().write_bytes(at, bytes) { Ok(0) } else { Err(EFAULT) }
    }

    fn evdev_ioctl(&mut self, n: usize, cmd: u32, arg: u32) -> Result<u32, i32> {
        if (cmd >> 8) & 0xFF != b'E' as u32 {
            return Err(ENOTTY);
        }
        let size = ((cmd >> 16) & 0x3FFF) as usize;
        match cmd & 0xFF {
            0x01 => self.put(arg, &0x1_0000u32.to_le_bytes()), // EVIOCGVERSION
            0x02 => self.put(arg, &[0x19, 0, 0, 0, 0, 0, 0, 0]), // EVIOCGID: BUS_HOST
            0x06 => {
                // EVIOCGNAME(len): the name with its NUL, cut to len.
                let mut name = INPUT_NAMES[n].as_bytes().to_vec();
                name.push(0);
                name.truncate(size);
                self.put(arg, &name)?;
                Ok(name.len() as u32)
            }
            0x18 => {
                // EVIOCGKEY: the keys down now.
                let mut bits = self.dev.keys[n].to_vec();
                bits.resize(size, 0);
                self.put(arg, &bits).map(|_| size as u32)
            }
            // EVIOCGLED, EVIOCGSND: nothing on.
            0x19..=0x1A => self.put(arg, &vec![0; size]).map(|_| size as u32),
            0x1B => {
                // EVIOCGSW: the switches on now.
                let mut bits = vec![0; size];
                if let Some(b) = bits.first_mut() {
                    *b = self.dev.switches[n];
                }
                self.put(arg, &bits).map(|_| size as u32)
            }
            // EVIOCGBIT(type, len): GUESS, nothing listed.
            0x20..=0x3F => self.put(arg, &vec![0; size]).map(|_| size as u32),
            0x90 => Ok(0), // EVIOCGRAB
            _ => {
                if self.logged.insert(format!("evdev {cmd:08X}")) {
                    eprintln!("[hle] input ioctl {cmd:08X} not handled");
                }
                Err(EINVAL)
            }
        }
    }

    fn dsp_ioctl(&mut self, cmd: u32, arg: u32) -> Result<u32, i32> {
        const AFMT_S16_LE: u32 = 0x10;
        let int_arg = |p: &mut Self| p.mem().read_bytes(arg, 4).map(|b| u32::from_le_bytes(b[..4].try_into().unwrap()));
        let now = self.clock();
        match cmd {
            0x5000 | 0x5001 | 0x5008 => {
                // RESET, SYNC, POST
                if cmd == 0x5000 {
                    self.dev.dsp.reset();
                }
                Ok(0)
            }
            0xC004_5002 => {
                // SPEED: the LF1000 driver (i2s/main.c) plays 32000 Hz only
                // and answers 32000 whatever is asked; the program then
                // converts (Brio's mixer does).
                let r = int_arg(self).ok_or(EFAULT)?;
                self.log_once(format!("dsp: {r} Hz asked, 32000 given"));
                self.put(arg, &32000u32.to_le_bytes())
            }
            0xC004_5003 => Ok(0), // STEREO: taken and ignored, as the driver
            0xC004_5004 => self.put(arg, &FRAG_SIZE.to_le_bytes()), // GETBLKSIZE
            0xC004_5005 => {
                // SETFMT: 16-bit little-endian, whatever is asked.
                let f = int_arg(self).ok_or(EFAULT)?;
                if f != AFMT_S16_LE {
                    self.log_once(format!("dsp: format {f:X} asked, S16_LE given"));
                }
                self.put(arg, &AFMT_S16_LE.to_le_bytes())
            }
            0xC004_5006 => {
                // CHANNELS: always 2.
                let c = int_arg(self).ok_or(EFAULT)?;
                if c != 2 {
                    self.log_once(format!("dsp: {c} channels asked, 2 given"));
                }
                self.put(arg, &2u32.to_le_bytes())
            }
            0xC004_5009 => Ok(0), // SUBDIVIDE
            0xC004_500A => {
                // SETFRAGMENT: not changed; answers its fragments, selector 9.
                int_arg(self).ok_or(EFAULT)?;
                self.put(arg, &((FRAGS << 16) | 9).to_le_bytes())
            }
            0x8004_500B => self.put(arg, &AFMT_S16_LE.to_le_bytes()), // GETFMTS
            0x8010_500C => {
                // GETOSPACE: audio_buf_info.
                let free = ((FRAG_SIZE * FRAGS) as u64).saturating_sub(self.dev.dsp.queued(now)) as u32;
                let info: Vec<u8> = [free / FRAG_SIZE, FRAGS, FRAG_SIZE, free].iter().flat_map(|v| v.to_le_bytes()).collect();
                self.put(arg, &info)
            }
            0x500E => Ok(0), // NONBLOCK
            0x8004_500F => self.put(arg, &0x3000u32.to_le_bytes()), // GETCAPS: TRIGGER | MMAP
            0x8004_5010 => self.put(arg, &2u32.to_le_bytes()), // GETTRIGGER: output
            0x4004_5010 => Ok(0),                               // SETTRIGGER
            0x800C_5012 => {
                // GETOPTR: count_info {bytes, blocks, ptr}.
                let d = &self.dev.dsp;
                let played = d.written - d.queued(now);
                let info: Vec<u8> = [played as u32, 0, (played % (FRAG_SIZE * FRAGS) as u64) as u32]
                    .iter()
                    .flat_map(|v| v.to_le_bytes())
                    .collect();
                self.put(arg, &info)
            }
            0x8004_5017 => {
                // GETODELAY
                let q = self.dev.dsp.queued(now) as u32;
                self.put(arg, &q.to_le_bytes())
            }
            _ if (cmd >> 8) & 0xFF == b'M' as u32 => {
                // Mixer ioctls on the dsp: volume 100/100, else nothing.
                if cmd & 0x8000_0000 != 0 {
                    self.put(arg, &0x6464u32.to_le_bytes())
                } else {
                    Ok(0)
                }
            }
            _ => {
                if self.logged.insert(format!("dsp {cmd:08X}")) {
                    eprintln!("[hle] dsp ioctl {cmd:08X} not handled");
                }
                Err(EINVAL)
            }
        }
    }
}

/// Whether a task blocked on `w` (a device wait) can run again.
pub(super) fn ready(dev: &Devices, w: &Wait, now: Instant) -> bool {
    match w {
        Wait::Input(n) => !dev.input[*n].is_empty(),
        Wait::DspFull(u) | Wait::Retry(u) => *u <= now,
        _ => false,
    }
}
