//! Audio hardware model.
//!
//! STATUS: only sample-format primitives exist. The voice engine (five
//! pitched A-law voices, two raw 8 kHz A-law voices, an LPC speech voice
//! and two circular PCM DMA channels feeding a 32 kHz DAC, as described by
//! MAME `leapster_a.cpp`) is not implemented. See docs/hardware-research.md.

/// Output rate of the DAC path observed by MAME (control value 0x7f01).
/// UNVERIFIED on hardware.
pub const DAC_RATE_HZ: u32 = 32_000;

/// Decodes one ITU-T G.711 A-law byte to 16-bit linear PCM (13-bit
/// magnitude scaled by 8, the reference `alaw2linear` output).
pub fn alaw_to_linear(a: u8) -> i16 {
    let a = a ^ 0x55;
    let mut t = ((a & 0x0F) as i16) << 4;
    let seg = (a & 0x70) >> 4;
    match seg {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= seg - 1;
        }
    }
    if a & 0x80 != 0 { t } else { -t }
}

/// Fixed-capacity ring buffer between the emulation thread and a host audio
/// callback. Overflow drops the oldest samples and is counted, never hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SampleRing {
    buf: Vec<i16>,
    read: usize,
    len: usize,
    pub overruns: u64,
    pub underruns: u64,
}

impl SampleRing {
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0);
        Self { buf: vec![0; capacity], read: 0, len: 0, overruns: 0, underruns: 0 }
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn push(&mut self, sample: i16) {
        let cap = self.buf.len();
        if self.len == cap {
            self.read = (self.read + 1) % cap;
            self.len -= 1;
            self.overruns += 1;
        }
        self.buf[(self.read + self.len) % cap] = sample;
        self.len += 1;
    }

    /// Fills `out`; missing samples are silence and counted as underruns.
    pub fn pop_into(&mut self, out: &mut [i16]) {
        for slot in out {
            if self.len == 0 {
                *slot = 0;
                self.underruns += 1;
            } else {
                *slot = self.buf[self.read];
                self.read = (self.read + 1) % self.buf.len();
                self.len -= 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alaw_matches_g711_reference_points() {
        assert_eq!(alaw_to_linear(0xD5), 8);
        assert_eq!(alaw_to_linear(0x55), -8);
        assert_eq!(alaw_to_linear(0xAA), 32256);
        assert_eq!(alaw_to_linear(0x2A), -32256);
        assert_eq!(alaw_to_linear(0x80), 5504);
        assert_eq!(alaw_to_linear(0x00), -5504);
    }

    #[test]
    fn alaw_is_monotonic_within_each_sign() {
        // Positive codes ordered by magnitude: XOR 0xD5 gives 0..=0x7F.
        let mut prev = i16::MIN;
        for m in 0u8..=0x7F {
            let v = alaw_to_linear(m ^ 0xD5);
            assert!(v > prev, "code {m:#x}");
            prev = v;
        }
    }

    #[test]
    fn ring_counts_overruns_and_underruns() {
        let mut ring = SampleRing::new(3);
        for s in 1..=4 {
            ring.push(s);
        }
        assert_eq!(ring.overruns, 1);
        let mut out = [0i16; 4];
        ring.pop_into(&mut out);
        assert_eq!(out, [2, 3, 4, 0]);
        assert_eq!(ring.underruns, 1);
    }
}
