//! The BIOS's inactivity power-off timer, for frontends that want to keep
//! the console on or to notice when it turned itself off.
//!
//! VERIFIED-FW (US v2.1 152-11265, SpongeBob): the system manager counts
//! idle seconds down from a reload value (900 = 15 minutes) and powers the
//! console off at 0 (it then plays its "idle timeout" shutdown movie and
//! waits for the power button, which a frontend cannot press). Games reset
//! the count with LF_ResetIdleTimeout, whose native code copies the reload
//! into the count (US v2.1 0x4009_3908: `ld r0,[r1,0xCC]; st r0,[r1,0xD4]`
//! on the state at [0x3C1F_ED2C]).
//!
//! The reset routine is found at run time the way the BIOS calls it, through
//! the system manager's function table ([[gp + 4] + 0xAC] + 0x14, the same in
//! the v1.5, v2.1 and Leapster TV BIOSes); its first instructions give the
//! state pointer (a 32-bit constant) and the reload and count offsets (the
//! load/store pair).

use ribbit_cpu::decode::decode_with;
use ribbit_cpu::isa::{Dst, Op, Src};
use ribbit_memory::map;

/// Where one BIOS keeps its idle timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleTimer {
    /// Address of the pointer to the system manager's state.
    pub state_ptr: u32,
    pub reload_offset: u32,
    pub count_offset: u32,
}

impl IdleTimer {
    /// Resolves the timer from the running BIOS (`gp` = r26). None until the
    /// system manager is set up, or if its reset routine does not look as
    /// expected.
    pub fn find(gp: u32, peek: impl Fn(u32) -> Option<u8>) -> Option<IdleTimer> {
        let word = |a: u32| Some(u32::from_le_bytes([peek(a)?, peek(a + 1)?, peek(a + 2)?, peek(a + 3)?]));
        let table = word(word(gp.checked_add(4)?)?.checked_add(0xAC)?)?;
        let reset = word(table.checked_add(0x14)?)?;
        let in_bios = |a: u32| a.wrapping_sub(map::BIOS_MIRROR) < map::BIOS_WINDOW;
        if !in_bios(reset) {
            return None;
        }
        let fetch = |a: u32| peek(a).zip(peek(a + 1)).map_or(0, |(lo, hi)| u16::from_le_bytes([lo, hi]));
        let (mut state_ptr, mut loaded) = (None, None::<(u8, u8, u32)>);
        let mut pc = reset;
        for _ in 0..40 {
            let i = decode_with(pc, fetch);
            match (i.op, i.dst, i.src1, i.src2) {
                (Op::Mov, Dst::Reg(_), _, Src::Limm(v)) if state_ptr.is_none() && v.wrapping_sub(map::BIOS_MIRROR) >= map::BIOS_WINDOW => {
                    state_ptr = Some(v)
                }
                (Op::Ld, Dst::Reg(a), Src::Reg(b), Src::Imm(o)) => loaded = Some((a, b, o)),
                (Op::St, _, Src::Reg(b), Src::Imm(o)) => {
                    if let (Some((a, lb, ro)), Src::Reg(d)) = (loaded, i.data)
                        && d == a
                        && lb == b
                    {
                        return Some(IdleTimer { state_ptr: state_ptr?, reload_offset: ro, count_offset: o });
                    }
                }
                (Op::J, ..) => break,
                _ => {}
            }
            pc += i.len.max(2);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// US v2.1's reset routine resolves to its known state pointer and offsets.
    #[test]
    fn resolves_from_us_v21_shaped_code() {
        // mov_s r13,0x3C1FED2C; ld_s r1,[r13,0]; ld r0,[r1,0xCC]; st r0,[r1,0xD4]; j_s [blink]
        let code: [u8; 18] = [
            0xcf, 0x75, 0x1f, 0x3c, 0x2c, 0xed, 0x20, 0x85, 0xcc, 0x11, 0x00, 0x00, 0xd4, 0x19, 0x00, 0x00, 0xe0, 0x7e,
        ];
        let (gp, table, vt, f) = (0x3C00_0100u32, 0x3C00_0200u32, 0x3C00_0300u32, 0x4009_3908u32);
        let peek = |a: u32| -> Option<u8> {
            let w = |base: u32, v: u32| (a >= base && a < base + 4).then(|| v.to_le_bytes()[(a - base) as usize]);
            w(gp + 4, table)
                .or_else(|| w(table + 0xAC, vt))
                .or_else(|| w(vt + 0x14, f))
                .or_else(|| (a >= f && a < f + 18).then(|| code[(a - f) as usize]))
                .or(Some(0))
        };
        assert_eq!(
            IdleTimer::find(gp, peek),
            Some(IdleTimer { state_ptr: 0x3C1F_ED2C, reload_offset: 0xCC, count_offset: 0xD4 })
        );
    }
}
