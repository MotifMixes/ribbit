//! /dev/mlc and /dev/layer0..2: the LF1000 display driver's ioctls
//! (drivers/lf1000/mlc/main.c), ported from its register functions
//! (mlc.c) so they program Ribbit's MLC model on the Bus as the real
//! kernel does on Original. Its quirks are kept: the transparency colour
//! goes into the layer's control register, alpha and the invert colour are
//! found 0x20 apart per layer, and the invisible area's top/bottom take
//! left/right.

use super::{EFAULT, EINVAL, ENOTTY, Process};
use crate::mlc::BASE;
use ribbit_arm::ArmBus;

pub(super) const MAJOR: u32 = 251;
pub(super) const LAYER_MAJOR: u32 = 250;
pub(super) const LAYERS: u8 = 3;
const VIDEO_LAYER: u8 = LAYERS - 1;

/// The frame buffers (include/asm-arm/arch-lf1000/mlc.h): RGB, OpenGL ES,
/// YUV, from FB_BASE_ADDR.
const FB_BASE: u32 = 0x0120_0000;
const RGB_SIZE: u32 = (320 * 240 * 4 / 4096 + 1) * 4096;
const OGL_SIZE: u32 = 0xD0_0000;
const YUV_SIZE: u32 = 14 * (1 << 20) - OGL_SIZE - RGB_SIZE;
pub(super) const FB_ADDR: [u32; 3] = [FB_BASE, FB_BASE + RGB_SIZE + YUV_SIZE, FB_BASE + RGB_SIZE];
pub(super) const FB_SIZE: [u32; 3] = [RGB_SIZE, OGL_SIZE, YUV_SIZE];

const CONTROLT: u32 = 0x00;
const SCREENSIZE: u32 = 0x04;
const BGCOLOR: u32 = 0x08;
const LEFTRIGHT0: u32 = 0x0C;
const TOPBOTTOM0: u32 = 0x10;
const LEFTRIGHT0_0: u32 = 0x14;
const TOPBOTTOM0_0: u32 = 0x18;
const HSTRIDE0: u32 = 0x28;
const TPCOLOR0: u32 = 0x30;
const INVCOLOR0: u32 = 0x34;
const CONTROL: [u32; 3] = [0x24, 0x58, 0x7C];
const ADDRESS: [u32; 3] = [0x38, 0x6C, 0x8C];
const VSTRIDE: [u32; 3] = [0x2C, 0x60, 0x80];
const ADDRESSCB: u32 = 0x90;
const ADDRESSCR: u32 = 0x94;
const STRIDECB: u32 = 0x98;
const STRIDECR: u32 = 0x9C;
const HSCALE: u32 = 0xA0;
const VSCALE: u32 = 0xA4;
const CLKENB: u32 = 0x3C0;

// CONTROLT bits.
const PIXELBUFFER_SLD: u32 = 10;
const PIXELBUFFER_PWD: u32 = 11;
const PRIORITY: u32 = 8;
const DITTYFLAG: u32 = 3;
const MLCENB: u32 = 1;
const FIELDENB: u32 = 0;
// Layer control bits.
const FORMAT: u32 = 16;
const PALETTEPWD: u32 = 15;
const PALETTESLD: u32 = 14;
const LOCKSIZE: u32 = 12;
const GRP3DENB: u32 = 8;
const LAYERENB: u32 = 5;
const DIRTYFLAG: u32 = 4;
const BLENDENB: u32 = 2;
const INVENB: u32 = 1;
const TPENB: u32 = 0;
const UNVALIDENB: u32 = 28;
const ALPHA: u32 = 28;

fn bit(v: u32, n: u32, on: bool) -> u32 {
    if on { v | 1 << n } else { v & !(1 << n) }
}

impl Process {
    fn mlc_read(&mut self, off: u32) -> u32 {
        self.m.bus.read32(BASE + off)
    }

    fn mlc_write(&mut self, off: u32, v: u32) {
        self.m.bus.write32(BASE + off, v);
    }

    /// Read-modify-write of a register.
    fn mlc_change(&mut self, off: u32, f: impl FnOnce(u32) -> u32) {
        let v = self.mlc_read(off);
        self.mlc_write(off, f(v));
    }

    /// What lf1000_mlc_probe sets up at boot: clocks, 320x240, video layer
    /// priority first, each layer's frame buffer, the MLC on.
    pub(super) fn mlc_init(&mut self) {
        // PCLKMODE_ONLYWHENCPUACCESS (0) << 3 | BCLKMODE_DYNAMIC (2).
        self.mlc_change(CLKENB, |v| v & !0xF | 2);
        self.mlc_write(SCREENSIZE, (240 - 1) << 16 | (320 - 1));
        self.mlc_change(CONTROLT, |v| v & !(3 << PRIORITY));
        self.mlc_change(CONTROLT, |v| bit(v, FIELDENB, false));
        for layer in 0..LAYERS {
            self.mlc_write(ADDRESS[layer as usize], FB_ADDR[layer as usize]);
        }
        self.mlc_enable(true);
        self.mlc_change(CONTROLT, |v| bit(v, DITTYFLAG, true));
    }

    fn mlc_enable(&mut self, en: bool) {
        let mut t = bit(self.mlc_read(CONTROLT), DITTYFLAG, false);
        if en {
            t = bit(t, PIXELBUFFER_PWD, true);
            self.mlc_write(CONTROLT, t);
            t = bit(t, PIXELBUFFER_SLD, true);
            self.mlc_write(CONTROLT, t);
            t = bit(t, MLCENB, true);
            self.mlc_write(CONTROLT, t);
            t = bit(t, DITTYFLAG, true);
        } else {
            // The driver waits for the dirty flag to clear (the next
            // frame) before powering the pixel buffer down; nothing here
            // shows a frame meanwhile.
            t = bit(t, MLCENB, false);
            t = bit(t, PIXELBUFFER_SLD, false);
            t = bit(t, PIXELBUFFER_PWD, false);
        }
        self.mlc_write(CONTROLT, t);
    }

    fn args<const N: usize>(&mut self, arg: u32) -> Result<[u32; N], i32> {
        let b = self.mem().read_bytes(arg, 4 * N).ok_or(EFAULT)?;
        Ok(std::array::from_fn(|i| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap())))
    }

    fn put_words(&mut self, arg: u32, words: &[u32]) -> Result<u32, i32> {
        let b: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
        if self.mem().write_bytes(arg, &b) { Ok(0) } else { Err(EFAULT) }
    }

    /// mlc_ioctl: /dev/mlc.
    pub(super) fn mlc_ioctl(&mut self, cmd: u32, arg: u32) -> Result<u32, i32> {
        if (cmd >> 8) & 0xFF != b'm' as u32 {
            return Err(ENOTTY);
        }
        match cmd & 0xFF {
            0 => self.mlc_enable(arg as u8 != 0),
            1 => self.mlc_write(BGCOLOR, arg & 0xFF_FFFF),
            2 => return Ok(self.mlc_read(BGCOLOR)),
            3 => {
                // VID_PRIORITY_INVALID is 3 when CPU_LF1000 is defined
                // (asm-arm/arch-lf1000/mlc.h), so AppManager's 3 fails and
                // the video stays in front: Original shows CONTROLT 0xC02.
                if arg >= 3 {
                    return Err(EINVAL);
                }
                self.mlc_change(CONTROLT, |v| v & !(3 << PRIORITY) | arg << PRIORITY);
            }
            4 => return Ok((self.mlc_read(CONTROLT) >> PRIORITY) & 3),
            5 => self.mlc_change(CONTROLT, |v| bit(v, DITTYFLAG, true)),
            6 => {
                let [w, h] = self.args(arg)?;
                if w.wrapping_sub(1) >= 4096 || h.wrapping_sub(1) >= 4096 {
                    return Err(EINVAL);
                }
                self.mlc_write(SCREENSIZE, (h - 1) << 16 | (w - 1));
            }
            7 => {
                let v = self.mlc_read(SCREENSIZE);
                return self.put_words(arg, &[(v & 0x7FF) + 1, ((v >> 16) & 0x7FF) + 1]);
            }
            _ => return Err(ENOTTY),
        }
        Ok(0)
    }

    /// mlc_layer_ioctl: /dev/layerN.
    pub(super) fn layer_ioctl(&mut self, layer: u8, cmd: u32, arg: u32) -> Result<u32, i32> {
        if (cmd >> 8) & 0xFF != b'm' as u32 || layer >= LAYERS {
            return Err(ENOTTY);
        }
        let l = layer as usize;
        let ctl = CONTROL[l];
        let video = layer == VIDEO_LAYER;
        let not_video = || if video { Err(EINVAL) } else { Ok(()) };
        let set_bit = |p: &mut Self, n: u32| p.mlc_change(ctl, |v| bit(v, n, arg as u8 != 0));
        match cmd & 0xFF {
            8 => {
                let en = arg as u8 != 0;
                let mut v = bit(self.mlc_read(ctl), PALETTEPWD, true);
                self.mlc_write(ctl, v);
                v = bit(v, PALETTESLD, en);
                self.mlc_write(ctl, v);
                self.mlc_write(ctl, bit(v, LAYERENB, en));
            }
            9 => self.mlc_write(ADDRESS[l], arg),
            38 if video => self.mlc_write(ADDRESSCB, arg),
            39 if video => self.mlc_write(ADDRESSCR, arg),
            38 | 39 => return Err(EINVAL),
            12 => {
                not_video()?;
                if !matches!(arg, 4 | 8 | 16) {
                    return Err(EINVAL);
                }
                self.mlc_change(ctl, |v| v & !(3 << LOCKSIZE) | (arg / 8) << LOCKSIZE);
            }
            10 => {
                not_video()?;
                self.mlc_write(HSTRIDE0 + 0x34 * layer as u32, arg & 0x7FFF_FFFF);
            }
            26 => {
                not_video()?;
                return Ok(self.mlc_read(HSTRIDE0 + 0x34 * layer as u32));
            }
            11 => {
                self.mlc_write(VSTRIDE[l], arg);
                if video {
                    self.mlc_write(STRIDECB, arg);
                    self.mlc_write(STRIDECR, arg);
                }
            }
            27 => return Ok(self.mlc_read(VSTRIDE[l])),
            18 => {
                not_video()?;
                set_bit(self, GRP3DENB);
            }
            33 => {
                not_video()?;
                return Ok((self.mlc_read(ctl) >> GRP3DENB) & 1);
            }
            19 => self.mlc_change(TPCOLOR0 + 0x20 * layer as u32, |v| v & !(0xF << ALPHA) | (arg & 0xF) << ALPHA),
            28 => return Ok((self.mlc_read(TPCOLOR0 + 0x20 * layer as u32) >> ALPHA) & 0xF),
            29 => return Ok(FB_SIZE[l]),
            20 => {
                not_video()?;
                self.mlc_change(ctl, |v| v & !0xFF_FFFF | arg & 0xFF_FFFF);
            }
            13 => {
                let [top, left, right, bottom] = self.args(arg)?;
                let (right, bottom) = (right.wrapping_sub(1) & 0x7FF, bottom.wrapping_sub(1) & 0x7FF);
                self.mlc_write(LEFTRIGHT0 + 0x34 * layer as u32, (left & 0x7FF) << 16 | right);
                self.mlc_write(TOPBOTTOM0 + 0x34 * layer as u32, (top & 0x7FF) << 16 | bottom);
            }
            14 => {
                let lr = self.mlc_read(LEFTRIGHT0 + 0x34 * layer as u32);
                let tb = self.mlc_read(TOPBOTTOM0 + 0x34 * layer as u32);
                return self.put_words(arg, &[(tb >> 16) & 0x7FF, (lr >> 16) & 0x7FF, lr & 0x7FF, tb & 0x7FF]);
            }
            36 => {
                not_video()?;
                let [_top, left, right, _bottom] = self.args(arg)?;
                let (left, right) = (left & 0x7FF, right & 0x7FF);
                self.mlc_change(LEFTRIGHT0_0 + 0x34 * layer as u32, |v| v & !(0x7FF << 16 | 0x7FF) | left << 16 | right);
                self.mlc_change(TOPBOTTOM0_0 + 0x34 * layer as u32, |v| v & !(0x7FF << 16 | 0x7FF) | left << 16 | right);
            }
            37 => {
                not_video()?;
                let lr = self.mlc_read(LEFTRIGHT0_0 + 0x34 * layer as u32);
                let tb = self.mlc_read(TOPBOTTOM0_0 + 0x34 * layer as u32);
                return self.put_words(arg, &[(tb >> 16) & 0x7FF, (lr >> 16) & 0x7FF, lr & 0x7FF, tb & 0x7FF]);
            }
            34 => {
                not_video()?;
                self.mlc_change(LEFTRIGHT0_0 + 0x34 * layer as u32, |v| bit(v, UNVALIDENB, arg as u8 != 0));
            }
            35 => {
                not_video()?;
                return Ok((self.mlc_read(LEFTRIGHT0_0 + 0x34 * layer as u32) >> UNVALIDENB) & 1);
            }
            15 => {
                not_video()?;
                if arg > 0xFFFF {
                    return Err(EINVAL);
                }
                self.mlc_change(ctl, |v| v & !(0xFFFF << FORMAT) | arg << FORMAT);
            }
            16 => {
                not_video()?;
                return Ok((self.mlc_read(ctl) >> FORMAT) & 0xFFFF);
            }
            21 => set_bit(self, BLENDENB),
            22 => {
                not_video()?;
                set_bit(self, TPENB);
            }
            23 => {
                not_video()?;
                set_bit(self, INVENB);
            }
            24 => self.mlc_change(INVCOLOR0 + 0x20 * layer as u32, |v| v & !0xFF_FFFF | arg & 0xFF_FFFF),
            17 => self.mlc_change(ctl, |v| bit(v, DIRTYFLAG, true)),
            32 => return Ok((self.mlc_read(ctl) >> DIRTYFLAG) & 1),
            25 => return Ok(self.mlc_read(ADDRESS[l])),
            30 => {
                let [sw, sh, dw, dh] = self.args(arg)?;
                if dw == 0 || dh == 0 {
                    return Err(EINVAL);
                }
                let scale = |s: u32, d: u32| {
                    if s < d { 1 << 28 | ((s - 1) << 11) / (d - 1).max(1) } else { (s << 11) / d }
                };
                self.mlc_write(HSCALE, scale(sw, dw));
                self.mlc_write(VSCALE, scale(sh, dh));
            }
            31 => {
                let (h, v) = (self.mlc_read(HSCALE), self.mlc_read(VSCALE));
                return self.put_words(arg, &[(h >> 11) & 0x7FF, h & 0x7FF, (v >> 11) & 0x7FF, v & 0x7FF]);
            }
            _ => return Err(ENOTTY),
        }
        Ok(0)
    }
}
