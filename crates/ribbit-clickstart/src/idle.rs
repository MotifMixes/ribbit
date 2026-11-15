//! The system ROM's idle power-off, for a frontend that wants the console to
//! stay on however long it is left alone.
//!
//! VERIFIED-FW (the dumped system ROM; 2026-11-15): the home screen's handler
//! (0x28f2f6, screen 1) counts its passes in word 6 of the state its pointer
//! at 0x1328 names (0x174a on the home screen), 30 a second (it runs every
//! other frame), and sets it back to 0 on a key, an arrow or a pointer move
//! (0x28f56e-0x28f57e). Past 4499 (0x28f5ef: `cmp r3,0x1193`), unless bit 10
//! of the state's first word is set, it goes to screen 0x0c, the shutdown
//! (0x28f5fc): Leap, Lily and Scout wave goodbye and the ROM sets 0x3d20 bit
//! 15 (`Bus::powered_off`). So the home screen left alone for 150 s turns
//! the console off. No other screen and no cartridge has a count of its own:
//! the only other ways to screen 0x0c are the developer mode's 's'
//! (0x28f463) and 0x28ee2b, which nothing found calls.
//!
//! Headlessly the home screen itself puts the pointer back now and then
//! (0x29714d through 0x3fff0a, every 20-60 s), which counts as a move, so
//! the count has not been seen past 0x381 in an hour; whether a real console
//! does the same is unknown.
//!
//! Keeping the console awake sets the count back to 0 about once a second
//! while the home screen shows, as a key would; nothing else changes.

/// The screen being shown (`0x297e16` asks for the next one at 0x1205).
pub const SCREEN: u32 = 0x1204;
/// The home screen's number.
pub const HOME_SCREEN: u16 = 1;

/// Where the home screen keeps its idle count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleCount {
    /// The RAM word holding the pointer to the screen's state.
    pub state_ptr: u32,
    /// The count's word in that state.
    pub offset: u32,
    /// The count it turns the console off past.
    pub limit: u16,
}

impl IdleCount {
    /// Finds the home screen's count in the system ROM from its code:
    /// `r1 = [ptr]; r2 = r1; r1 += off; r4 = r1; r2 = [r4]; r3 = r2;
    /// r2 += 1; r4 = r1 (sic); [r4] = r2; cmp r3, limit` (0x28f5e1).
    pub fn find(rom: &[u16]) -> Option<IdleCount> {
        const BODY: [u16; 8] = [0x9501, 0x0246, 0x9901, 0x94c4, 0x9702, 0x0441, 0x9901, 0xd4c4];
        rom.windows(12).find_map(|w| {
            (w[0] == 0x9311 && w[2..10] == BODY && w[10] == 0x470b).then(|| IdleCount {
                state_ptr: w[1] as u32,
                offset: (BODY[1] & 0x3f) as u32,
                limit: w[11],
            })
        })
    }

    /// The count's RAM address while the home screen shows.
    pub fn address(&self, ram: &[u16]) -> Option<u32> {
        if ram.get(SCREEN as usize) != Some(&HOME_SCREEN) {
            return None;
        }
        let state = *ram.get(self.state_ptr as usize)? as u32;
        let at = state + self.offset;
        (state != 0 && (at as usize) < ram.len()).then_some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn found_from_the_home_screens_code() {
        let mut rom = vec![0u16; 64];
        rom.extend_from_slice(&[0x9311, 0x1328, 0x9501, 0x0246, 0x9901, 0x94c4, 0x9702, 0x0441, 0x9901, 0xd4c4, 0x470b, 0x1193, 0xae13]);
        let c = IdleCount::find(&rom).unwrap();
        assert_eq!(c, IdleCount { state_ptr: 0x1328, offset: 6, limit: 0x1193 });
        let mut ram = vec![0u16; 0x2800];
        ram[0x1328] = 0x1744;
        assert_eq!(c.address(&ram), None, "not on the home screen");
        ram[SCREEN as usize] = HOME_SCREEN;
        assert_eq!(c.address(&ram), Some(0x174a));
        ram[0x1328] = 0;
        assert_eq!(c.address(&ram), None, "no state yet");
    }
}
