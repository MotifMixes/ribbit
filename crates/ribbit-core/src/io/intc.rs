//! Interrupt flag and enable registers at 0x0180_0080 / 0x0180_0084.
//!
//! SOURCED (MAME `leapster.cpp`): the flag register collects interrupt
//! causes for sources that share a CPU line (ADC: bit 8; PCM DMA halves:
//! bits 20/21) and is acknowledged by writing 1s; the enable register gates
//! the PCM bits onto CPU line 0x13. The v2.1 BIOS writes 0x100 to the flag
//! register before initialising the ADC, consistent with write-1-to-clear.
//!
//! Bit 7 is EEPROM command completion, raised by eeprom.rs (MAME instead
//! forces it on in every read). Framebuffer DMA raises bit 14; each ADC
//! sample (adc.rs) raises bit 8.

pub const FLAGS_ADDR: u32 = 0x0180_0080;
pub const ENABLE_ADDR: u32 = 0x0180_0084;

pub const FLAG_ADC: u32 = 1 << 8;
/// System EEPROM command complete. VERIFIED-FW: the BIOS clears bits 7/6
/// before a command and polls bit 7 (see eeprom.rs).
pub const FLAG_EEPROM_DONE: u32 = 1 << 7;
/// ADC interrupt line (MAME: vector 0x10; VERIFIED-FW: the BIOS installs its
/// ADC handler there). Raised by every ADC sample (adc.rs).
pub const ADC_IRQ_LINE: u8 = 0x10;
/// PCM DMA half-buffer flags (MAME); routed to CPU line 0x13 when enabled.
pub const FLAG_PCM: u32 = (1 << 20) | (1 << 21);
pub const PCM_IRQ_LINE: u8 = 0x13;
/// Framebuffer DMA complete. VERIFIED-FW: `LcdMPI::Init` acknowledges and
/// enables bit 14 and installs its DMA handler on vector 0x12, and the
/// handler acknowledges bit 14. MAME raises line 0x12 on DMA completion.
pub const FLAG_DMA: u32 = 1 << 14;
pub const DMA_IRQ_LINE: u8 = 0x12;
/// Audio voice-command acknowledge. VERIFIED-FW: both BIOSes' vector-0x1D
/// handler acknowledges bit 18 and the BIOS enables bit 18 (see audio.rs).
pub const FLAG_VOICE_ACK: u32 = 1 << 18;
pub const VOICE_ACK_IRQ_LINE: u8 = 0x1D;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IntFlags {
    pub flags: u32,
    pub enable: u32,
}

impl IntFlags {
    /// Device side: latch interrupt causes.
    pub fn raise(&mut self, bits: u32) {
        self.flags |= bits;
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        match addr {
            FLAGS_ADDR => Some(self.flags),
            ENABLE_ADDR => Some(self.enable),
            _ => None,
        }
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            FLAGS_ADDR => self.flags &= !value,
            ENABLE_ADDR => self.enable = value,
            _ => return false,
        }
        true
    }

    /// Level of CPU line 0x12 (framebuffer DMA done).
    pub fn dma_line(&self) -> bool {
        self.flags & self.enable & FLAG_DMA != 0
    }

    /// Level of CPU line 0x1D (audio voice-command acknowledge).
    pub fn voice_ack_line(&self) -> bool {
        self.flags & self.enable & FLAG_VOICE_ACK != 0
    }

    /// Level of CPU line 0x10 (ADC). Not gated by the enable register, as in
    /// MAME's `fire_adc_interrupt`; dropped when software acknowledges bit 8.
    pub fn adc_line(&self) -> bool {
        self.flags & FLAG_ADC != 0
    }

    /// Level of CPU line 0x13 (PCM), per MAME's `update_pcm_irq`.
    pub fn pcm_line(&self) -> bool {
        self.flags & self.enable & FLAG_PCM != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_write_one_to_clear() {
        let mut f = IntFlags::default();
        f.raise(FLAG_ADC | (1 << 20));
        assert_eq!(f.read(FLAGS_ADDR), Some(FLAG_ADC | (1 << 20)));
        f.write(FLAGS_ADDR, FLAG_ADC);
        assert_eq!(f.read(FLAGS_ADDR), Some(1 << 20));
        f.write(FLAGS_ADDR, 0);
        assert_eq!(f.read(FLAGS_ADDR), Some(1 << 20), "writing 0 changes nothing");
    }

    #[test]
    fn pcm_line_requires_flag_and_enable() {
        let mut f = IntFlags::default();
        f.raise(1 << 21);
        assert!(!f.pcm_line());
        f.write(ENABLE_ADDR, 1 << 21);
        assert!(f.pcm_line());
        f.write(FLAGS_ADDR, 1 << 21);
        assert!(!f.pcm_line());
    }

    #[test]
    fn dma_line_follows_flag_and_enable_bit_14() {
        let mut f = IntFlags::default();
        f.write(ENABLE_ADDR, FLAG_DMA);
        assert!(!f.dma_line());
        f.raise(FLAG_DMA);
        assert!(f.dma_line());
        f.write(FLAGS_ADDR, FLAG_DMA);
        assert!(!f.dma_line(), "the handler's acknowledge drops the line");
    }

    #[test]
    fn other_addresses_are_not_claimed() {
        let mut f = IntFlags::default();
        assert_eq!(f.read(0x0180_0088), None);
        assert!(!f.write(0x0180_0088, 1));
    }
}
