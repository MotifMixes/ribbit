//! The firmware's idle power-off, for a frontend that wants the pad to stay
//! on however long it is left alone.
//!
//! VERIFIED by execution (`examples/lpidle.rs`, Canada Full Base, all 17
//! cartridges in roms/leappad that start): left alone, every one prints
//! "Auto Power Down" and powers the processor down (PCON) after about 3
//! minutes: 175 s for the cartridges with the newer kernel (00406, 00652,
//! 00730, 01148, 11374), 180.6 s for the Classic ones.
//!
//! VERIFIED-FW (500-00406 app 0xA2E8-0xA35F, 500-00367 kernel 0x6BBC-0x6C10):
//! the firmware keeps a deadline, the low byte of (count + timeout) >> n, in
//! XDATA (F088 in the newer kernels, F0E6 / F0F0 in the Classic ones; count
//! is the FC01-FC04 seconds counter, timeout 180, n 3 or 2), and powers down
//! when the low byte of count >> n reaches it. Activity (a pen touch) sets a
//! bit in the bit-addressable RAM; the main loop then moves the deadline on
//! and clears the bit:
//!
//! * Classic kernels: `JNB bit,skip; MOV DPTR,#FC01h; MOVX A,@DPTR ...
//!   MOV DPTR,#F0xxh; MOV A,R7; MOVX @DPTR,A; CLR bit` (bit 0x18, 0x19 or
//!   0x1E by cartridge).
//! * Newer kernels and the Full Base: `JBC bit,+2; SJMP skip; ... MOV
//!   DPTR,#FC01h; MOVX A,@DPTR ... MOV DPTR,#F088h; MOV A,R7; MOVX @DPTR,A`
//!   (bit 0x1F).
//!
//! Keeping the pad awake sets that bit about once a second, as a touch
//! would, so the firmware itself moves its deadline on; nothing else
//! changes. The routine is looked for in the code the processor sees at the
//! time (the running kernel and app), so it follows the base ROM, then the
//! cartridge.

/// The activity bit (a bit address, 0x00-0x7F) of the idle power-off
/// routine in this 64 KB code space, if one is found.
pub fn activity_bit(code: &[u8]) -> Option<u8> {
    let at = |i: usize, pat: &[u8]| code.get(i..i + pat.len()) == Some(pat);
    let find = |from: usize, len: usize, pat: &[u8]| (from..from + len).find(|&i| at(i, pat));
    // MOV DPTR,#F0xxh; MOV A,R7; MOVX @DPTR,A within `len` bytes of `from`.
    let store = |from: usize, len: usize| (from..from + len).find(|&i| at(i, &[0x90, 0xF0]) && at(i + 3, &[0xEF, 0xF0]));
    const READ_COUNT: [u8; 4] = [0x90, 0xFC, 0x01, 0xE0];
    for i in 0..code.len().saturating_sub(8) {
        let bit = code[i + 1];
        if bit >= 0x80 {
            continue;
        }
        match code[i] {
            // JNB bit,rel; MOV DPTR,#FC01h; MOVX ...; store; CLR bit.
            0x30 if at(i + 3, &READ_COUNT) => {
                if let Some(s) = store(i + 7, 56)
                    && at(s + 5, &[0xC2, bit])
                {
                    return Some(bit);
                }
            }
            // JBC bit,+2; SJMP rel; ... MOV DPTR,#FC01h ...; store.
            0x10 if code[i + 2] == 0x02 && code[i + 3] == 0x80 => {
                if let Some(r) = find(i + 5, 16, &READ_COUNT)
                    && store(r + 4, 56).is_some()
                {
                    return Some(bit);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_classic_and_the_newer_routines() {
        // 500-00367 kernel 0x6BBC (shortened: the addition left out).
        let mut classic = vec![0u8; 16];
        classic.extend_from_slice(&[0x30, 0x1E, 0x28, 0x90, 0xFC, 0x01, 0xE0, 0xFC, 0xA3, 0xE0, 0x24, 0xB4, 0xFF]);
        classic.extend_from_slice(&[0x90, 0xF0, 0xF0, 0xEF, 0xF0, 0xC2, 0x1E, 0x22, 0, 0, 0, 0]);
        assert_eq!(activity_bit(&classic), Some(0x1E));
        // 500-00406 app 0xA2E8.
        let mut newer = vec![0u8; 16];
        newer.extend_from_slice(&[0x10, 0x1F, 0x02, 0x80, 0x31, 0x12, 0xEF, 0x31, 0xE4, 0xFC, 0xFD]);
        newer.extend_from_slice(&[0x90, 0xFC, 0x01, 0xE0, 0xF8, 0x2F, 0xFF, 0x90, 0xF0, 0x88, 0xEF, 0xF0, 0x80, 0x3F, 0, 0, 0, 0]);
        assert_eq!(activity_bit(&newer), Some(0x1F));
        // Without the clear of the same bit, the Classic shape is not taken.
        let n = classic.len();
        classic[n - 6] = 0x1D;
        assert_eq!(activity_bit(&classic), None);
    }
}
