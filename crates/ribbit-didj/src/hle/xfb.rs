//! /dev/fb0..2 on the Explorer: its kernel's frame buffer driver
//! (drivers/video/lf1000fb.c, one fb per MLC layer, the YUV layer last)
//! with the generic fbmem.c ioctls, ported so the programs program
//! Ribbit's MLC model as the real driver does on Original. The three
//! layers share one 18 MB frame buffer (`FB_BASE`); the YUV layer's
//! address has bit 29 set (block addressing). No TV out (board 2).

use super::explorer::{FB_BASE, FB_SIZE};
use super::{EFAULT, EINVAL, Process};
use crate::mlc::BASE;
use ribbit_arm::ArmBus;

const LAYERS: usize = 3;
const YUV: usize = LAYERS - 1;
const RGBREGLEN: u32 = 0x34;

const MLCCONTROLT: u32 = 0x00;
const MLCSCREENSIZE: u32 = 0x04;
const MLCBGCOLOR: u32 = 0x08;

const RGBLEFTRIGHT: u32 = 0x00;
const RGBTOPBOTTOM: u32 = 0x04;
const RGBCONTROL: u32 = 0x18;
const RGBHSTRIDE: u32 = 0x1C;
const RGBVSTRIDE: u32 = 0x20;
const RGBTPCOLOR: u32 = 0x24;
const RGBADDRESS: u32 = 0x2C;
const YUVCONTROL: u32 = 0x08;
const YUVVSTRIDE: u32 = 0x0C;
const YUVTPCOLOR: u32 = 0x10;
const YUVADDRESS: u32 = 0x18;
const YUVADDRESSCB: u32 = 0x1C;
const YUVADDRESSCR: u32 = 0x20;
const YUVVSTRIDECB: u32 = 0x24;
const YUVVSTRIDECR: u32 = 0x28;
const YUVHSCALE: u32 = 0x2C;
const YUVVSCALE: u32 = 0x30;

const NONSTD_PLANAR: u32 = 23;
const NONSTD_PRIORITY: u32 = 24;
const NONSTD_FORMAT: u32 = 20;
const FORMAT_RGB: u32 = 0;
const FORMAT_YUV420: u32 = 1;
const FORMAT_YUV422: u32 = 2;

// struct fb_var_screeninfo, in words.
const XRES: usize = 0;
const YRES: usize = 1;
const XRES_VIRTUAL: usize = 2;
const YRES_VIRTUAL: usize = 3;
const XOFFSET: usize = 4;
const YOFFSET: usize = 5;
const BPP: usize = 6;
const RED: usize = 8;
const GREEN: usize = 11;
const BLUE: usize = 14;
const TRANSP: usize = 17;
const NONSTD: usize = 20;
const ACTIVATE: usize = 21;
const PIXCLOCK: usize = 25;
const LEFT_MARGIN: usize = 26;
const VMODE: usize = 33;
const ROTATE: usize = 34;
const VAR_WORDS: usize = 40;

const FB_TYPE_PACKED_PIXELS: u32 = 0;
const FB_TYPE_PLANES: u32 = 1;
const FB_VISUAL_TRUECOLOR: u32 = 2;
const FB_VISUAL_PSEUDOCOLOR: u32 = 3;

/// One layer's fb_info and the driver's lf1000fb_layer.
#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Layer {
    var: Vec<u32>,
    smem_start: u32,
    line_length: u32,
    kind: u32,
    visual: u32,
    enabled: bool,
    vflip: bool,
    format: u32,
    vstride: u32,
    hstride: u32,
    hsrc: u32,
    vsrc: u32,
}

#[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Fb {
    layers: Vec<Layer>,
    /// Pans (FBIOPAN_DISPLAY): the frames programs showed.
    pub pans: u64,
}

fn linear_to_xy(addr: u32) -> u32 {
    let (segment, offset) = (addr >> 24, addr & 0xFF_FFFF);
    0x2000_0000 | segment << 24 | (offset >> 12) << 12 | (offset & 0xFFF)
}

/// The format setters (set_rgb565 ...): offsets and lengths into the var,
/// the MLC's format code out.
fn set_channels(var: &mut [u32], rgb: [(u32, u32); 3]) {
    for (i, (offset, length)) in rgb.into_iter().enumerate() {
        var[RED + 3 * i] = offset;
        var[RED + 3 * i + 1] = length;
    }
}

fn set_transp(var: &mut [u32], t: (u32, u32), x: u16, a: u16) -> u16 {
    if var[TRANSP + 1] == 0 {
        return x;
    }
    var[TRANSP] = t.0;
    var[TRANSP + 1] = t.1;
    a
}

fn rgb_format(var: &mut [u32]) -> Result<(u16, u32), i32> {
    let (r, g, b) = (var[RED], var[GREEN], var[BLUE]);
    let none = |var: &mut [u32]| {
        var[TRANSP] = 0;
        var[TRANSP + 1] = 0;
    };
    Ok(match var[BPP] {
        1 | 2 | 4 | 8 => {
            let n = var[BPP];
            var[RED + 1] = n;
            var[GREEN + 1] = n;
            var[BLUE + 1] = n;
            (0x443A, FB_VISUAL_PSEUDOCOLOR)
        }
        16 => {
            let f = match (r, g, b) {
                (0, 5, 11) => {
                    set_channels(var, [(0, 5), (5, 6), (11, 5)]);
                    none(var);
                    0xC432
                }
                (8, 4, 0) => {
                    set_channels(var, [(8, 4), (4, 4), (0, 4)]);
                    set_transp(var, (12, 4), 0x4211, 0x2211)
                }
                (0, 4, 8) => {
                    set_channels(var, [(0, 4), (4, 4), (8, 4)]);
                    set_transp(var, (12, 4), 0xC211, 0xA211)
                }
                (5, 2, 0) => {
                    set_channels(var, [(5, 3), (2, 3), (0, 2)]);
                    set_transp(var, (8, 8), 0x4120, 0x1120)
                }
                (0, 2, 5) => {
                    set_channels(var, [(0, 2), (2, 3), (5, 3)]);
                    set_transp(var, (8, 8), 0xC120, 0x9120)
                }
                (10, 5, 0) => {
                    set_channels(var, [(10, 5), (5, 5), (0, 5)]);
                    set_transp(var, (15, 1), 0x4342, 0x3342)
                }
                (0, 5, 10) => {
                    set_channels(var, [(0, 5), (5, 5), (10, 5)]);
                    set_transp(var, (15, 1), 0xC342, 0xB342)
                }
                // (11, 5, 0) and anything else: RGB565.
                _ => {
                    set_channels(var, [(11, 5), (5, 6), (0, 5)]);
                    none(var);
                    0x4432
                }
            };
            (f, FB_VISUAL_PSEUDOCOLOR)
        }
        24 | 32 => {
            let bgr = (r, g, b) == (0, 8, 16);
            set_channels(var, if bgr { [(0, 8), (8, 8), (16, 8)] } else { [(16, 8), (8, 8), (0, 8)] });
            let f = if var[BPP] == 32 {
                if bgr { set_transp(var, (24, 8), 0xC653, 0x8653) } else { set_transp(var, (24, 8), 0x4653, 0x0653) }
            } else {
                none(var);
                if bgr { 0xC653 } else { 0x4653 }
            };
            (f, FB_VISUAL_TRUECOLOR)
        }
        _ => return Err(EINVAL),
    })
}

impl Process {
    fn fb_reg(&self, l: usize, r: u32) -> u32 {
        BASE + l as u32 * RGBREGLEN + r + 0xC
    }

    fn fb_read(&mut self, l: usize, r: u32) -> u32 {
        let a = self.fb_reg(l, r);
        self.m.bus.read32(a)
    }

    fn fb_write(&mut self, l: usize, r: u32, v: u32) {
        let a = self.fb_reg(l, r);
        self.m.bus.write32(a, v);
    }

    fn control_of(l: usize) -> u32 {
        if l == YUV { YUVCONTROL } else { RGBCONTROL }
    }

    fn fb_set_dirty(&mut self, l: usize) {
        let c = Self::control_of(l);
        let v = self.fb_read(l, c);
        self.fb_write(l, c, v | 1 << 4);
    }

    fn fb_set_address(&mut self, l: usize, address: u32) {
        if l == YUV {
            if (self.x.fb.layers[l].var[NONSTD] >> NONSTD_FORMAT) & 7 == FORMAT_YUV420 {
                let pitch = 4096;
                let vsrc = self.x.fb.layers[l].vsrc;
                self.fb_write(l, YUVADDRESS, linear_to_xy(address));
                self.fb_write(l, YUVADDRESSCB, linear_to_xy(address + pitch / 2));
                self.fb_write(l, YUVADDRESSCR, linear_to_xy(address + pitch / 2 + pitch * vsrc / 2));
            }
        } else {
            self.fb_write(l, RGBADDRESS, address);
        }
    }

    fn fb_set_invert(&mut self, l: usize, en: bool) {
        let v = self.fb_read(l, RGBCONTROL);
        self.fb_write(l, RGBCONTROL, if en { v | 2 } else { v & !2 });
    }

    fn fb_set_blend(&mut self, l: usize, en: bool, alpha: u32) {
        let (c, t) = if l == YUV { (YUVCONTROL, YUVTPCOLOR) } else { (RGBCONTROL, RGBTPCOLOR) };
        let v = self.fb_read(l, t) & !(0xF << 28);
        self.fb_write(l, t, v | (alpha & 0xF) << 28);
        let v = self.fb_read(l, c);
        self.fb_write(l, c, if en { v | 4 } else { v & !4 });
        self.fb_set_dirty(l);
    }

    fn fb_set_yuv_position(&mut self, l: usize, pos: u32) -> u32 {
        if l != YUV {
            return pos;
        }
        let pos = pos.min(LAYERS as u32 - 1);
        let v = self.m.bus.read32(BASE + MLCCONTROLT) & !(3 << 8);
        self.m.bus.write32(BASE + MLCCONTROLT, v | 1 << 3 | (pos & 3) << 8);
        pos
    }

    fn fb_set_rgb_format(&mut self, l: usize, format: u16) {
        if l == YUV {
            return;
        }
        let var = &self.x.fb.layers[l].var;
        let bpp = var[BPP] >> 3;
        let mut hstride = bpp;
        let mut vstride = if (var[NONSTD] >> NONSTD_PLANAR) & 1 != 0 { 4096 } else { bpp * var[XRES] };
        let layer = &mut self.x.fb.layers[l];
        layer.vstride = vstride;
        layer.hstride = hstride;
        if layer.vflip {
            vstride = 0u32.wrapping_sub(vstride);
            hstride = 0u32.wrapping_sub(hstride);
        }
        self.fb_write(l, RGBHSTRIDE, hstride);
        self.fb_write(l, RGBVSTRIDE, vstride);
        let v = self.fb_read(l, RGBCONTROL) & !(0xFFFF << 16);
        self.fb_write(l, RGBCONTROL, v | (format as u32) << 16);
        self.x.fb.layers[l].format = FORMAT_RGB;
    }

    fn fb_set_yuv_format(&mut self, l: usize, format: u32, bpp: u32, width: u32) {
        let stride = bpp * width;
        self.x.fb.layers[l].vstride = stride;
        self.x.fb.layers[l].hstride = bpp;
        self.fb_write(l, YUVVSTRIDE, stride);
        self.fb_write(l, YUVVSTRIDECB, stride);
        self.fb_write(l, YUVVSTRIDECR, stride);
        let v = self.fb_read(l, YUVCONTROL) & !(0xFFFF << 16);
        self.fb_write(l, YUVCONTROL, v | format << 16);
    }

    /// set_yuv / set_rgb: checks `var` (and fills in its channels); with
    /// `apply` programs the layer from the current var.
    fn fb_set_mode(&mut self, l: usize, var: &mut [u32], apply: bool) -> Result<(), i32> {
        if l == YUV {
            let pf = (var[NONSTD] >> NONSTD_FORMAT) & 7;
            match pf {
                FORMAT_YUV420 | FORMAT_YUV422 => {
                    if apply {
                        if pf == FORMAT_YUV420 {
                            self.fb_set_yuv_format(l, 0, 1, 4096);
                        } else {
                            let xres = self.x.fb.layers[l].var[XRES];
                            self.fb_set_yuv_format(l, 2, 2, xres);
                        }
                        self.x.fb.layers[l].format = pf;
                    }
                    Ok(())
                }
                _ => Err(EINVAL),
            }
        } else {
            let (format, visual) = rgb_format(var)?;
            if apply {
                self.fb_set_rgb_format(l, format);
                self.x.fb.layers[l].visual = visual;
            }
            Ok(())
        }
    }

    fn fb_set_video_scaler(&mut self, l: usize, xres: u32, yres: u32, apply: bool) -> Result<(), i32> {
        if l != YUV {
            return Err(EINVAL);
        }
        let var = self.x.fb.layers[l].var.clone();
        self.x.fb.layers[l].hsrc = xres;
        self.x.fb.layers[l].vsrc = yres;
        let scale = |src: u32, res: u32| {
            let s = ((src - 1) << 12) / (res - 1);
            (s >> 1) + (s & 1)
        };
        if xres < var[XRES] {
            self.fb_write(l, YUVHSCALE, 1 << 28 | scale(xres, var[XRES]));
        } else {
            if xres > var[XRES_VIRTUAL] / 2 {
                return Err(EINVAL);
            }
            self.fb_write(l, YUVHSCALE, (xres << 11) / var[XRES]);
        }
        if yres < var[YRES] {
            self.fb_write(l, YUVVSCALE, 1 << 28 | scale(yres, var[YRES]));
        } else {
            if yres > var[YRES_VIRTUAL] {
                return Err(EINVAL);
            }
            self.fb_write(l, YUVVSCALE, (yres << 11) / var[YRES]);
        }
        if apply {
            self.fb_set_dirty(l);
        }
        Ok(())
    }

    fn fb_get_video_scaler(&mut self, l: usize) -> Result<(u32, u32), i32> {
        if l != YUV {
            return Err(EINVAL);
        }
        let (xres, yres) = (self.x.fb.layers[l].var[XRES], self.x.fb.layers[l].var[YRES]);
        let get = |v: u32, res: u32| if v & 1 << 28 != 0 { (((v & !(1 << 28)) * (res - 1)) >> 11) + 1 } else { (v * res) >> 11 };
        let h = self.fb_read(l, YUVHSCALE);
        let v = self.fb_read(l, YUVVSCALE);
        Ok((get(h, xres), get(v, yres)))
    }

    fn fb_set_position(&mut self, l: usize, left: i32, top: i32, right: i32, bottom: i32, apply: bool) {
        self.fb_write(l, RGBLEFTRIGHT, ((left as u32 & 0xFFF) << 16) | ((right - 1) as u32 & 0xFFF));
        self.fb_write(l, RGBTOPBOTTOM, ((top as u32 & 0xFFF) << 16) | ((bottom - 1) as u32 & 0xFFF));
        if apply {
            self.fb_set_dirty(l);
        }
    }

    fn fb_get_position(&mut self, l: usize) -> (i32, i32, i32, i32) {
        let lr = self.fb_read(l, RGBLEFTRIGHT);
        let tb = self.fb_read(l, RGBTOPBOTTOM);
        (((lr >> 16) & 0xFFF) as i32, ((tb >> 16) & 0xFFF) as i32, ((lr & 0xFFF) + 1) as i32, ((tb & 0xFFF) + 1) as i32)
    }

    fn fb_set_resolution(&mut self, l: usize) {
        let (left, top, _, _) = self.fb_get_position(l);
        let (xres, yres) = (self.x.fb.layers[l].var[XRES] as i32, self.x.fb.layers[l].var[YRES] as i32);
        self.fb_set_position(l, left, top, left + xres, top + yres, false);
    }

    fn fb_set_vflip(&mut self, l: usize, flip: bool) {
        let layer = &self.x.fb.layers[l];
        if layer.vflip == flip {
            return;
        }
        let (vstride, yres) = (layer.vstride, layer.var[YRES]);
        let addr = self.fb_read(l, RGBADDRESS);
        if flip {
            self.fb_write(l, RGBADDRESS, addr.wrapping_add(vstride * yres));
            self.fb_write(l, RGBVSTRIDE, 0u32.wrapping_sub(vstride));
            self.fb_set_invert(l, true);
        } else {
            self.fb_write(l, RGBADDRESS, addr.wrapping_sub(vstride * yres));
            self.fb_write(l, RGBVSTRIDE, vstride);
            self.fb_set_invert(l, false);
        }
        self.x.fb.layers[l].vflip = flip;
    }

    fn fb_rotate(&mut self, l: usize, rotate: u32) {
        if l == YUV {
            return;
        }
        match rotate {
            0 => self.fb_set_vflip(l, false),
            180 => self.fb_set_vflip(l, true),
            _ => return,
        }
        self.x.fb.layers[l].var[ROTATE] = rotate;
    }

    fn fb_set_type(&mut self, l: usize, planar: bool) {
        let addr = self.fb_read(l, RGBADDRESS);
        if planar {
            let layer = &mut self.x.fb.layers[l];
            layer.line_length = 4096;
            layer.kind = FB_TYPE_PLANES;
            layer.smem_start |= 0x2000_0000;
            self.fb_set_address(l, addr | 0x2000_0000);
        } else {
            let layer = &mut self.x.fb.layers[l];
            layer.kind = FB_TYPE_PACKED_PIXELS;
            layer.smem_start &= !0x2000_0000;
            layer.line_length = layer.var[XRES] * (layer.var[BPP] >> 3);
            self.fb_write(l, RGBADDRESS, addr & !0x2000_0000);
        }
    }

    fn fb_set_par(&mut self, l: usize) -> Result<(), i32> {
        self.fb_set_resolution(l);
        let var = self.x.fb.layers[l].var.clone();
        if var[BPP] == 32 {
            self.fb_rotate(l, var[ROTATE]);
        }
        if l == YUV {
            self.x.fb.layers[l].line_length = 4096;
        }
        let mut v = self.x.fb.layers[l].var.clone();
        let r = self.fb_set_mode(l, &mut v, true);
        if r.is_ok() {
            let nonstd = self.x.fb.layers[l].var[NONSTD];
            if l == YUV {
                let pos = self.fb_set_yuv_position(l, (nonstd >> NONSTD_PRIORITY) & 3);
                self.x.fb.layers[l].var[NONSTD] = nonstd & !(3 << NONSTD_PRIORITY) | pos << NONSTD_PRIORITY;
            } else {
                self.fb_set_type(l, (nonstd >> NONSTD_PLANAR) & 1 != 0);
                self.x.fb.layers[l].var[NONSTD] = nonstd & !(3 << NONSTD_PRIORITY);
            }
        }
        self.fb_set_dirty(l);
        r
    }

    fn fb_blank(&mut self, l: usize, blank: u32) {
        let c = Self::control_of(l);
        let mut reg = self.fb_read(l, c);
        match blank.min(4) {
            0 => {
                reg |= 1 << 15;
                self.fb_write(l, c, reg);
                reg |= 1 << 14;
                self.fb_write(l, c, reg);
                reg |= 1 << 5;
                self.fb_write(l, c, reg);
                self.x.fb.layers[l].enabled = true;
                self.fb_set_dirty(l);
            }
            n => {
                if n == 1 {
                    self.m.bus.write32(BASE + MLCBGCOLOR, 0);
                    let t = self.m.bus.read32(BASE + MLCCONTROLT);
                    self.m.bus.write32(BASE + MLCCONTROLT, t | 1 << 3);
                }
                // Disable, then (the driver waits for the next frame to
                // take it) sleep and palette off.
                reg &= !(1 << 5);
                reg |= 1 << 4;
                self.fb_write(l, c, reg);
                reg &= !(1 << 14);
                self.fb_write(l, c, reg);
                reg &= !(1 << 15);
                self.fb_write(l, c, reg);
                self.x.fb.layers[l].enabled = false;
                self.fb_set_dirty(l);
            }
        }
    }

    fn fb_pan(&mut self, l: usize, xoffset: u32, yoffset: u32) -> Result<(), i32> {
        let layer = &self.x.fb.layers[l];
        let mut address = layer.smem_start;
        let offset = xoffset + layer.vstride * yoffset;
        if l != YUV && layer.vflip {
            address += layer.vstride * layer.var[YRES];
        }
        if address >= layer.smem_start + FB_SIZE {
            return Err(EINVAL);
        }
        self.x.fb.pans += 1;
        self.x.fb.layers[l].var[XOFFSET] = xoffset;
        self.x.fb.layers[l].var[YOFFSET] = yoffset;
        self.fb_set_address(l, address + offset);
        self.fb_set_dirty(l);
        Ok(())
    }

    /// fbmem.c's fb_pan_display: the generic checks, then the driver.
    fn fb_pan_display(&mut self, l: usize, var: &[u32]) -> Result<(), i32> {
        let cur = &self.x.fb.layers[l].var;
        if var[YOFFSET] > cur[YRES_VIRTUAL] - cur[YRES] || var[XOFFSET] > cur[XRES_VIRTUAL] - cur[XRES] {
            return Err(EINVAL);
        }
        self.fb_pan(l, var[XOFFSET], var[YOFFSET])
    }

    /// fbmem.c's fb_set_var.
    fn fb_set_var(&mut self, l: usize, var: &mut [u32]) -> Result<(), i32> {
        const FB_ACTIVATE_FORCE: u32 = 128;
        if var[ACTIVATE] & FB_ACTIVATE_FORCE == 0 && var[..] == self.x.fb.layers[l].var[..] {
            return Ok(());
        }
        // check_var
        let format = (var[NONSTD] >> NONSTD_FORMAT) & 7;
        if (format == FORMAT_YUV420 || format == FORMAT_YUV422) && l != YUV {
            return Err(EINVAL);
        }
        if var[ROTATE] != 0 && var[BPP] != 32 {
            return Err(EINVAL);
        }
        self.fb_set_mode(l, var, false)?;
        if var[ACTIVATE] & 15 == 0 {
            let old = std::mem::replace(&mut self.x.fb.layers[l].var, var.to_vec());
            if let Err(e) = self.fb_set_par(l) {
                self.x.fb.layers[l].var = old;
                return Err(e);
            }
            let now = self.x.fb.layers[l].var.clone();
            let _ = self.fb_pan_display(l, &now);
            var.copy_from_slice(&self.x.fb.layers[l].var);
        }
        Ok(())
    }

    /// What lf1000fb_probe leaves: the screen size, each layer set up (the
    /// top one keeps the boot loader's size and stays on, the others
    /// blanked), the MLC on.
    pub(super) fn fb_init(&mut self) {
        self.m.bus.write32(BASE + MLCSCREENSIZE, (320 - 1) | (240 - 1) << 16);
        self.x.fb.layers = vec![Layer::default(); LAYERS];
        for l in 0..LAYERS {
            let mut var = vec![0u32; VAR_WORDS];
            var[XRES] = 320;
            var[YRES] = 240;
            // ILI9322 (mach-lf1000/screen.c): hfp 50, hbp 36, vfp 4, vbp 17,
            // hsw 2, vsw 1, 6.4 MHz.
            var[LEFT_MARGIN..LEFT_MARGIN + 6].copy_from_slice(&[50, 36, 4, 17, 2, 1]);
            var[PIXCLOCK] = 1_000_000_000 / (6_400_000 / 1000);
            if l == 0 {
                let (left, top, right, bottom) = self.fb_get_position(0);
                var[XRES] = (right - left) as u32;
                var[YRES] = (bottom - top) as u32;
            }
            var[XRES_VIRTUAL] = 4096;
            var[YRES_VIRTUAL] = FB_SIZE / 4096;
            let layer = &mut self.x.fb.layers[l];
            layer.smem_start = FB_BASE;
            if l == YUV {
                var[NONSTD] = (LAYERS as u32 - 1) << NONSTD_PRIORITY | FORMAT_YUV420 << NONSTD_FORMAT;
                layer.kind = FB_TYPE_PLANES;
                layer.smem_start |= 0x2000_0000;
                layer.line_length = 4096;
                layer.var = var;
                self.fb_set_yuv_position(l, LAYERS as u32 - 1);
                let _ = self.fb_set_video_scaler(l, 320, 240, false);
            } else {
                var[NONSTD] = FORMAT_RGB << NONSTD_FORMAT;
                layer.kind = FB_TYPE_PACKED_PIXELS;
                var[BPP] = 32;
                var[TRANSP] = 24;
                var[TRANSP + 1] = 8;
                set_channels(&mut var, [(0, 8), (8, 8), (16, 8)]);
                layer.var = var;
            }
            self.fb_set_address(l, FB_BASE);
            let c = Self::control_of(l);
            let v = self.fb_read(l, c) & !(3 << 12);
            self.fb_write(l, c, v);
            self.x.fb.layers[l].var[ACTIVATE] = 128;
            let _ = self.fb_set_par(l);
            self.fb_blank(l, if l == 0 { 0 } else { 4 });
        }
        let t = self.m.bus.read32(BASE + MLCCONTROLT);
        self.m.bus.write32(BASE + MLCCONTROLT, t | 1 << 3 | 1 << 1);
    }

    fn fb_var_bytes(&self, l: usize) -> Vec<u8> {
        self.x.fb.layers[l].var.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    /// struct fb_fix_screeninfo (68 bytes).
    fn fb_fix_bytes(&self, l: usize) -> Vec<u8> {
        let layer = &self.x.fb.layers[l];
        let mut f = vec![0u8; 68];
        let id = format!("lf1000-fb{l}");
        f[..id.len()].copy_from_slice(id.as_bytes());
        let mut put = |at: usize, v: u32| f[at..at + 4].copy_from_slice(&v.to_le_bytes());
        put(16, layer.smem_start);
        put(20, FB_SIZE);
        put(24, layer.kind);
        put(32, layer.visual);
        // xpanstep 1, ypanstep 1, ywrapstep 0.
        put(36, 1 | 1 << 16);
        put(44, layer.line_length);
        f
    }

    /// The frames the Explorer's programs have shown (fb pans).
    pub fn explorer_pans(&self) -> u64 {
        self.x.fb.pans
    }

    /// An ioctl on /dev/fb<l>.
    pub(super) fn fb_ioctl(&mut self, l: usize, cmd: u32, arg: u32) -> Result<u32, i32> {
        if self.x.fb.layers.is_empty() {
            return Err(19);
        }
        let read_var = |p: &mut Self| -> Result<Vec<u32>, i32> {
            let b = p.mem().read_bytes(arg, VAR_WORDS * 4).ok_or(EFAULT)?;
            Ok(b.chunks(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect())
        };
        match cmd {
            // FBIOGET_VSCREENINFO
            0x4600 => {
                let v = self.fb_var_bytes(l);
                if self.mem().write_bytes(arg, &v) { Ok(0) } else { Err(EFAULT) }
            }
            // FBIOPUT_VSCREENINFO
            0x4601 => {
                let mut var = read_var(self)?;
                self.fb_set_var(l, &mut var)?;
                let b: Vec<u8> = var.iter().flat_map(|w| w.to_le_bytes()).collect();
                if self.mem().write_bytes(arg, &b) { Ok(0) } else { Err(EFAULT) }
            }
            // FBIOGET_FSCREENINFO
            0x4602 => {
                let f = self.fb_fix_bytes(l);
                if self.mem().write_bytes(arg, &f) { Ok(0) } else { Err(EFAULT) }
            }
            // FBIOPAN_DISPLAY
            0x4606 => {
                let var = read_var(self)?;
                let _ = var[VMODE];
                self.fb_pan_display(l, &var).map(|_| 0)
            }
            // FBIOBLANK
            0x4611 => {
                self.fb_blank(l, arg);
                Ok(0)
            }
            // FBIOGET_VBLANK: flags HAVE_COUNT, the DPC interrupts so far.
            0x8020_4612 => {
                let mut b = vec![0u8; 32];
                b[..4].copy_from_slice(&0x10u32.to_le_bytes());
                b[4..8].copy_from_slice(&(self.m.bus.flips as u32).to_le_bytes());
                if self.mem().write_bytes(arg, &b) { Ok(0) } else { Err(EFAULT) }
            }
            // FBIO_WAITFORVSYNC: whether the layer's changes are still waiting.
            0x4004_4620 => {
                let c = Self::control_of(l);
                Ok((self.fb_read(l, c) >> 4) & 1)
            }
            // FBIOGETCMAP / FBIOPUTCMAP: the palette (8-bit modes only).
            0x4604 | 0x4605 => {
                self.log_once(format!("fb{l}: colour map ioctl {cmd:04X} ignored"));
                Ok(0)
            }
            // LF1000FB_IOCSALPHA / IOCGALPHA
            0x4004_6D01 => {
                let b = self.mem().read_bytes(arg, 2).ok_or(EFAULT)?;
                self.fb_set_blend(l, b[0] != 0, b[1] as u32);
                Ok(0)
            }
            0x8004_6D02 => {
                let (c, t) = if l == YUV { (YUVCONTROL, YUVTPCOLOR) } else { (RGBCONTROL, RGBTPCOLOR) };
                let alpha = (self.fb_read(l, t) >> 28) as u8 & 0xF;
                let en = (self.fb_read(l, c) >> 2) as u8 & 1;
                if self.mem().write_bytes(arg, &[en, alpha]) { Ok(0) } else { Err(EFAULT) }
            }
            // LF1000FB_IOCSPOSTION / IOCGPOSTION: left, top, apply, right, bottom.
            0x4004_6D03 => {
                let b = self.mem().read_bytes(arg, 20).ok_or(EFAULT)?;
                let w = |i: usize| i32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
                self.fb_set_position(l, w(0), w(1), w(3), w(4), w(2) & 1 != 0);
                Ok(0)
            }
            0x8004_6D04 => {
                let (left, top, right, bottom) = self.fb_get_position(l);
                let b: Vec<u8> = [left, top, 0, right, bottom].iter().flat_map(|w| w.to_le_bytes()).collect();
                if self.mem().write_bytes(arg, &b) { Ok(0) } else { Err(EFAULT) }
            }
            // LF1000FB_IOCSVIDSCALE / IOCGVIDSCALE: sizex, sizey, apply.
            0x4004_6D05 => {
                let b = self.mem().read_bytes(arg, 12).ok_or(EFAULT)?;
                let w = |i: usize| u32::from_le_bytes(b[4 * i..4 * i + 4].try_into().unwrap());
                self.fb_set_video_scaler(l, w(0), w(1), w(2) & 1 != 0).map(|_| 0)
            }
            0x8004_6D06 => {
                let (x, y) = self.fb_get_video_scaler(l)?;
                let b: Vec<u8> = [x, y, 0].iter().flat_map(|w| w.to_le_bytes()).collect();
                if self.mem().write_bytes(arg, &b) { Ok(0) } else { Err(EFAULT) }
            }
            _ => {
                if self.logged.insert(format!("fb ioctl {cmd:08X}")) {
                    eprintln!("[hle] fb{l} ioctl {cmd:08X} not handled");
                }
                Err(25)
            }
        }
    }

    /// mmap of /dev/fb<l>: the frame buffer at the layer's smem_start
    /// (fbmem.c's fb_mmap).
    pub(super) fn fb_mmap(&mut self, l: usize, at: u32, len: u32, pgoff: u32) -> Result<u32, i32> {
        let start = self.x.fb.layers.get(l).map_or(FB_BASE, |x| x.smem_start);
        let off = pgoff << 12;
        if off.saturating_add(len) > FB_SIZE.next_multiple_of(4096) {
            return Err(EINVAL);
        }
        if self.mem().map_phys(at, len, start + off) { Ok(at) } else { Err(super::ENOMEM) }
    }
}
