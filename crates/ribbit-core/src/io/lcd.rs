//! LCD controller registers and framebuffer DMA (0x0180_8000 block).
//!
//! Evidence (docs/hardware-research.md §Display):
//! * MAME `leapster.cpp`: 0x0180_8088 framebuffer base (offset into VRAM),
//!   0x0180_808C stride, 0x0180_8090 format (bit 31 enable; 4 = 12 bpp packed,
//!   3 = RGB332). DMA at 0x0180_8800: +4 source, +8 stride (words), +0xC lines,
//!   +0x10 destination offset; writing 0x1B to +0 copies lines x stride words
//!   from the source to VRAM at base + offset, then IRQ 0x12.
//! * Both BIOSes (`LcdMPI::Init`, v1.5 0x4007_B414 / 0x4007_B60A):
//!   base = 0, stride = 0xF0 (160 px x 12 bpp), format = 0x8000_0004; install a
//!   handler on vector 0x12, acknowledge and enable interrupt-flag bit 14;
//!   `CopyToScreen` (0x4007_BAA0) programs source 0x3C01_6560, stride 0x3C,
//!   160 lines, offset 0, writes 0x0E to 0x0180_8818, then 0 and 0x1B to
//!   0x0180_8800, and waits (with a 500-tick timeout and retry) for the
//!   vector-0x12 handler, which reads 0x0180_8818 (bit 1 = error), writes 0x0E
//!   to it and acknowledges flag bit 14.
//!
//! Model: the copy completes instantly when 0x1B is written; completion sets
//! interrupt flag bit 14 (routed to CPU line 0x12 by `IntFlags`). The status
//! register never reports an error. Other command values, the panel timing
//! registers (0x0180_8004-802C, 0x0180_8094-809C) and the dither tables
//! (0x0180_8180-81BC) are not modelled.

use ribbit_video::LcdRegs;

pub const LCD_BASE: u32 = 0x0180_8088;
pub const LCD_STRIDE: u32 = 0x0180_808C;
pub const LCD_FORMAT: u32 = 0x0180_8090;

pub const DMA_CONTROL: u32 = 0x0180_8800;
pub const DMA_SOURCE: u32 = 0x0180_8804;
pub const DMA_STRIDE: u32 = 0x0180_8808;
pub const DMA_LINES: u32 = 0x0180_880C;
pub const DMA_OFFSET: u32 = 0x0180_8810;
pub const DMA_STATUS: u32 = 0x0180_8818;

/// The command that starts a copy (MAME; used by both BIOSes).
pub const DMA_START: u32 = 0x1B;

/// A copy to perform: `words` 32-bit words from `source` to `dest`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DmaRequest {
    pub source: u32,
    pub dest: u32,
    pub words: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LcdController {
    pub regs: LcdRegs,
    pub dma_source: u32,
    pub dma_stride: u32,
    pub dma_lines: u32,
    pub dma_offset: u32,
    pub dma_status: u32,
    pub dma_control: u32,
    /// Started by a 0x1B command; executed by the machine, which has bus access.
    pub pending: Option<DmaRequest>,
    pub transfers: u64,
    /// Command values other than 0 and 0x1B (not executed).
    pub unknown_commands: u64,
}

impl LcdController {
    pub fn read(&self, addr: u32) -> Option<u32> {
        Some(match addr {
            LCD_BASE => self.regs.fb_base,
            LCD_STRIDE => self.regs.stride,
            LCD_FORMAT => self.regs.format,
            DMA_SOURCE => self.dma_source,
            DMA_STRIDE => self.dma_stride,
            DMA_LINES => self.dma_lines,
            DMA_OFFSET => self.dma_offset,
            DMA_STATUS => self.dma_status,
            _ => return None,
        })
    }

    /// Accepts byte, halfword and word writes (the BIOS uses all three on
    /// these registers); the value is taken zero-extended.
    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            LCD_BASE => self.regs.fb_base = value,
            LCD_STRIDE => self.regs.stride = value,
            LCD_FORMAT => self.regs.format = value,
            DMA_SOURCE => self.dma_source = value,
            DMA_STRIDE => self.dma_stride = value,
            DMA_LINES => self.dma_lines = value,
            DMA_OFFSET => self.dma_offset = value,
            // Write-1-to-clear status (the ISR writes 0x0E).
            DMA_STATUS => self.dma_status &= !value,
            DMA_CONTROL => {
                self.dma_control = value;
                match value {
                    DMA_START => {
                        self.pending = Some(DmaRequest {
                            source: self.dma_source,
                            dest: ribbit_memory::map::VRAM_BASE
                                .wrapping_add(self.regs.fb_base)
                                .wrapping_add(self.dma_offset),
                            words: self.dma_stride.wrapping_mul(self.dma_lines),
                        });
                    }
                    0 => {}
                    _ => self.unknown_commands += 1,
                }
            }
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bios_display_setup_reaches_the_decoder_registers() {
        let mut lcd = LcdController::default();
        lcd.write(LCD_BASE, 0);
        lcd.write(LCD_STRIDE, 0xF0);
        lcd.write(LCD_FORMAT, 0x8000_0004);
        assert_eq!(lcd.regs, LcdRegs { fb_base: 0, stride: 0xF0, format: 0x8000_0004 });
        assert!(lcd.regs.enabled());
        assert_eq!(lcd.read(LCD_STRIDE), Some(0xF0));
    }

    #[test]
    fn start_command_requests_the_documented_copy() {
        let mut lcd = LcdController::default();
        lcd.write(LCD_BASE, 0x100);
        // CopyToScreen's sequence (v1.5 0x4007_BAA0..BB40).
        lcd.write(DMA_SOURCE, 0x3C01_6560);
        lcd.write(DMA_STRIDE, 0x3C);
        lcd.write(DMA_LINES, 0xA0);
        lcd.write(DMA_OFFSET, 0);
        lcd.write(DMA_STATUS, 0x0E);
        lcd.write(DMA_CONTROL, 0);
        assert_eq!(lcd.pending, None);
        lcd.write(DMA_CONTROL, DMA_START);
        assert_eq!(lcd.pending, Some(DmaRequest { source: 0x3C01_6560, dest: 0x0300_0100, words: 0x3C * 0xA0 }));
    }

    #[test]
    fn unknown_commands_are_counted_not_executed() {
        let mut lcd = LcdController::default();
        lcd.write(DMA_CONTROL, 0x13);
        assert_eq!((lcd.pending, lcd.unknown_commands), (None, 1));
    }

    #[test]
    fn status_is_write_one_to_clear() {
        let mut lcd = LcdController { dma_status: 0x0A, ..Default::default() };
        lcd.write(DMA_STATUS, 0x02);
        assert_eq!(lcd.read(DMA_STATUS), Some(0x08));
    }
}
