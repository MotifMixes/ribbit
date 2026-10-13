//! Audio unit: the voice-command acknowledge only (0x0180_2070).
//!
//! Evidence (docs/hardware-research.md §Phase 4c):
//!
//! | Behaviour | Status | Evidence |
//! |---|---|---|
//! | 0x0180_2070 takes a voice command byte: voice = bits 2:0, command = bits 7:3 (0x10 = start) | VERIFIED-FW + SOURCED | Both BIOSes' command routine (v1.5 0x4002_4CB4, v2.1 0x4002_E0E0) `stb`s it and records `cmd & 7` as the pending voice; MAME `do_voice_command(data >> 3, data & 7)` |
//! | The audio unit acknowledges each command with an interrupt on vector 0x1D | VERIFIED-FW | Both drivers install a 0x1D handler (v1.5 0x4002_4D2C, v2.1 0x4002_E158) that sets the event bit the next command waits for; MAME raises line 0x1D on every write |
//! | The acknowledge is latched in interrupt-flag bit 18 and gated by enable bit 18 | VERIFIED-FW (routing) | The handler acknowledges flag bit 18 (writes 0x4_0000 to 0x0180_0080); the BIOS sets enable bit 18 (0x0180_0084 = 0x0005_4520). MAME raises the CPU line directly instead |
//! | Acknowledge latency | UNKNOWN | Modelled as immediate, as in MAME |
//!
//! Voice playback, the status register 0x0180_2078 and the LFC codec live in
//! voices.rs. Not modelled: the vector-0x15 interrupt (flag bit 16; MAME never
//! raises it either) and PCM DMA.

use super::intc::{FLAG_VOICE_ACK, IntFlags};

pub const VOICE_COMMAND_ADDR: u32 = 0x0180_2070;
/// MAME: command 0x10 starts a voice; any other stops it.
pub const COMMAND_START: u8 = 0x10;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VoiceCommand {
    pub command: u8,
    pub voice: u8,
}

impl VoiceCommand {
    pub fn decode(byte: u8) -> Self {
        Self { command: byte >> 3, voice: byte & 7 }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AudioUnit {
    pub last_command: Option<VoiceCommand>,
    pub commands: u64,
}

impl AudioUnit {
    /// Accepts byte and word writes (the BIOS uses `stb`); the command is the
    /// low byte.
    pub fn write(&mut self, addr: u32, value: u32, intc: &mut IntFlags) -> bool {
        if addr != VOICE_COMMAND_ADDR {
            return false;
        }
        let cmd = VoiceCommand::decode(value as u8);
        tracing::debug!(target: "ribbit::audio", command = cmd.command, voice = cmd.voice, "voice command (acknowledged; no playback)");
        self.last_command = Some(cmd);
        self.commands += 1;
        intc.raise(FLAG_VOICE_ACK);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io::intc::{ENABLE_ADDR, FLAGS_ADDR};

    #[test]
    fn decodes_the_bios_command_byte() {
        // v2.1 writes 0x86 at 0.89 s and then waits on voice 6.
        assert_eq!(VoiceCommand::decode(0x86), VoiceCommand { command: COMMAND_START, voice: 6 });
    }

    #[test]
    fn command_is_acknowledged_through_flag_bit_18() {
        let (mut a, mut intc) = (AudioUnit::default(), IntFlags::default());
        intc.write(ENABLE_ADDR, 0x0005_4520); // the BIOS's enable mask
        assert!(!intc.voice_ack_line());
        assert!(a.write(VOICE_COMMAND_ADDR, 0x86, &mut intc));
        assert_eq!(intc.flags, FLAG_VOICE_ACK);
        assert!(intc.voice_ack_line());
        // The 0x1D handler's acknowledge (v2.1 0x4002_E168).
        intc.write(FLAGS_ADDR, 0x4_0000);
        assert!(!intc.voice_ack_line());
        assert_eq!((a.commands, a.last_command), (1, Some(VoiceCommand { command: 0x10, voice: 6 })));
    }

    #[test]
    fn ack_is_gated_by_enable_bit_18() {
        let (mut a, mut intc) = (AudioUnit::default(), IntFlags::default());
        a.write(VOICE_COMMAND_ADDR, 0x86, &mut intc);
        assert!(!intc.voice_ack_line(), "latched but masked");
        intc.write(ENABLE_ADDR, FLAG_VOICE_ACK);
        assert!(intc.voice_ack_line());
    }

    #[test]
    fn other_audio_registers_are_not_claimed() {
        let (mut a, mut intc) = (AudioUnit::default(), IntFlags::default());
        for addr in [0x0180_2078, 0x0180_20F0, 0x0180_40F4, 0x0180_3000] {
            assert!(!a.write(addr, 1, &mut intc), "{addr:#x}");
        }
        assert_eq!(intc.flags, 0);
    }
}
