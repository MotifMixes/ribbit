//! The LeapFrog Zippity (2009), a TV console built on a GeneralPlus
//! GPL32300A: an ARM9 core (`ribbit-arm`) with GeneralPlus's video chip,
//! sound unit and peripherals. Everything about the hardware here was
//! learned from the system software and recordings of real units; see
//! docs/zippity.md for what is known and what is a stand-in.
//!
//! ```no_run
//! # let nand = Vec::new();
//! let mut z = ribbit_zippity::Zippity::new(nand);
//! let mut frame = vec![0u32; ribbit_zippity::SCREEN_W * ribbit_zippity::SCREEN_H];
//! loop {
//!     z.set_pad(ribbit_zippity::Pad::IDLE);
//!     z.run_frame();
//!     z.render(&mut frame);
//!     let _sound = z.take_audio();
//! }
//! ```

mod bus;
mod nand;
pub mod probe;
mod sound;
mod video;

pub use bus::{CLOCK, FRAME};
pub use nand::{PAGE, PAGE_DATA, PAGES_PER_BLOCK};
pub use sound::OUTPUT_RATE;
pub use video::{SCREEN_H, SCREEN_W};

use bus::Bus;
use ribbit_arm::Arm;
use serde::{Deserialize, Serialize};

/// The controllers' state as the radio receiver sends it: 10 bits,
/// active-low (a 0 bit is a button held).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pad(pub u16);

impl Pad {
    /// Nothing held.
    pub const IDLE: Pad = Pad(bus::PAD_IDLE);
    /// Joystick directions (found on the system menu).
    pub const UP: u16 = 1 << 0;
    pub const DOWN: u16 = 1 << 1;
    pub const LEFT: u16 = 1 << 2;
    pub const RIGHT: u16 = 1 << 3;
    /// Brings up a panel with a forward arrow (which button is UNKNOWN; the
    /// button on top of the joystick?).
    pub const BIT4: u16 = 1 << 4;
    /// UNKNOWN (the mat's colour pads?). The test menu calls it red.
    pub const BIT5: u16 = 1 << 5;
    pub const BIT6: u16 = 1 << 6;
    /// Chooses on the menus (green, the test menu says).
    pub const CHOOSE: u16 = 1 << 7;
    /// The mat's foot pads (standing on the mat starts Mickey Says).
    pub const LEFT_FOOT: u16 = 1 << 8;
    pub const RIGHT_FOOT: u16 = 1 << 9;

    /// The state with these buttons (bits above) held.
    pub fn held(buttons: u16) -> Pad {
        Pad(bus::PAD_IDLE & !buttons)
    }
}

/// A Zippity: the CPU and everything on its bus, running from a dump of the
/// system NAND (raw, with spare bytes).
#[derive(Serialize, Deserialize)]
pub struct Zippity {
    cpu: Arm,
    bus: Bus,
}

/// The state at a loop's jump back, to tell whether the loop is idle.
#[derive(Default)]
struct IdleLoop {
    r: [u32; 16],
    cpsr: u32,
    activity: u64,
}

impl IdleLoop {
    /// Whether the CPU is where it was at the last jump back, with the same
    /// registers and no write or I/O access since (an idle loop); notes the
    /// state for next time if not.
    fn again(&mut self, cpu: &Arm, activity: u64) -> bool {
        if self.activity == activity && self.cpsr == cpu.cpsr && self.r == cpu.r {
            return true;
        }
        *self = IdleLoop { r: cpu.r, cpsr: cpu.cpsr, activity };
        false
    }
}

/// The save state format's version (bumped when the layout changes).
const STATE_VERSION: u32 = 1;

impl Zippity {
    /// Powers on from a raw NAND image (pages of 0x840 bytes). The boot ROM
    /// has not been dumped: as MAME's stub does, its work is done here by
    /// copying NAND data 0xA0000..0x1F0000 (spare bytes stripped) to
    /// address 0 and starting there.
    pub fn new(nand: Vec<u8>) -> Self {
        let data: Vec<u8> = nand.chunks(PAGE).take(0x1F_0000 / PAGE_DATA).flat_map(|p| p[..PAGE_DATA.min(p.len())].to_vec()).collect();
        let mut bus = Bus::new(nand);
        let (start, end) = (0xA_0000, 0x1F_0000.min(data.len()));
        if end > start {
            bus.ram[..end - start].copy_from_slice(&data[start..end]);
        }
        let mut cpu = Arm::new();
        cpu.reset();
        Self { cpu, bus }
    }

    /// Clears what users have saved (players, progress, the play log), as
    /// the system's own test mode does: power on holding the power button
    /// with port B bit 5 low, then press green (CHOOSE) on the test menu.
    /// Returns the NAND afterwards, or why the system did not finish.
    ///
    /// The system only unlinks the files: blocks of its file system it does
    /// not rewrite keep the old data (players' names, the parent report)
    /// until it needs the room and erases them itself. Those are erased here
    /// too: blocks among the ones the clear rewrote (from the lowest to the
    /// highest) that it left alone. UNVERIFIED as a rule (one dump checked:
    /// it boots, makes a player and saves as before; docs/zippity.md).
    pub fn factory_reset(nand: Vec<u8>) -> Result<Vec<u8>, String> {
        /// How long to wait for each step, in frames.
        const LIMIT: u32 = 600;
        let mut z = Zippity::new(nand);
        z.bus.probe.power_hold = Some(u64::MAX);
        z.bus.probe.presses.push((0xC000_0020, 0x20, 0, u64::MAX));
        z.run_until("Entering interactive test mode", LIMIT)?;
        z.bus.probe.power_hold = Some(0);
        z.bus.probe.presses.clear();
        // The test menu draws, then takes the press.
        for _ in 0..60 {
            z.run_frame();
        }
        z.set_pad(Pad::held(Pad::CHOOSE));
        for _ in 0..15 {
            z.run_frame();
        }
        z.set_pad(Pad::IDLE);
        z.run_until("Erased all user data.", LIMIT)?;
        let nand = &mut z.bus.nand;
        if let (Some(&lo), Some(&hi)) = (nand.touched.first(), nand.touched.last()) {
            for block in (lo..=hi).filter(|b| !nand.touched.contains(b)) {
                let at = block * PAGES_PER_BLOCK * PAGE;
                if let Some(b) = nand.flash.get_mut(at..at + PAGES_PER_BLOCK * PAGE) {
                    b.fill(0xFF);
                }
            }
        }
        Ok(std::mem::take(&mut nand.flash))
    }

    /// Runs frames until the system's log shows `text` (at most `limit`).
    fn run_until(&mut self, text: &str, limit: u32) -> Result<(), String> {
        for _ in 0..limit {
            if self.log().contains(text) {
                return Ok(());
            }
            self.run_frame();
        }
        Err(format!("the system did not log \"{text}\""))
    }

    /// Instructions run since power-on.
    pub fn instructions(&self) -> u64 {
        self.bus.now
    }

    /// Instructions skipped in idle loops since power-on.
    pub fn skipped(&self) -> u64 {
        self.bus.skipped
    }

    /// Runs `n` instructions.
    ///
    /// Idle loops are skipped: a short loop (a jump back of under 64 bytes)
    /// that makes no write and no I/O access, and comes round with every
    /// register and flag as before, does the same thing again until
    /// something else changes memory or the interrupt lines. Only device
    /// ticks and frames (or an interrupt they raise) can, so time moves on
    /// to the next one at once. The system spends most of its time in such
    /// loops (waiting for the video interrupt's flag in RAM), and the result
    /// is the same as running them. Waiting for an interrupt (CP15) is
    /// skipped the same way.
    pub fn run(&mut self, n: u64) {
        let end = self.bus.now + n;
        // The next frame interrupt (at instruction FRAME - 1 of each frame)
        // and device tick (every 256 instructions).
        let mut frame_at = self.bus.now - self.bus.now % FRAME + FRAME - 1;
        if frame_at < self.bus.now {
            frame_at += FRAME;
        }
        let mut tick_at = self.bus.now.next_multiple_of(256);
        let mut idle = IdleLoop::default();
        while self.bus.now < end {
            let now = self.bus.now;
            if now == frame_at {
                self.bus.frame();
                frame_at += FRAME;
            }
            if now == tick_at {
                self.bus.tick();
                tick_at += 256;
            }
            // Instructions up to the next event.
            let next = frame_at.min(tick_at).min(end);
            while self.bus.now < next {
                if self.bus.lines_dirty {
                    self.bus.lines_dirty = false;
                    (self.cpu.irq_line, self.cpu.fiq_line) = self.bus.lines();
                }
                self.bus.pc = self.cpu.r[15];
                self.cpu.step(&mut self.bus);
                self.bus.now += 1;
                let (pc, at) = (self.cpu.r[15], self.cpu.current);
                if (self.cpu.waiting || pc <= at && at - pc < 64) && !self.bus.probe.no_idle_skip
                    && idle.again(&self.cpu, self.bus.activity)
                {
                    self.bus.skipped += next - self.bus.now;
                    self.bus.now = next;
                }
            }
        }
    }

    /// Runs one video frame's worth of instructions.
    pub fn run_frame(&mut self) {
        let to_next = FRAME - self.bus.now % FRAME;
        self.run(to_next);
    }

    /// The controllers' state from now on.
    pub fn set_pad(&mut self, pad: Pad) {
        self.bus.pad = pad.0 & bus::PAD_IDLE;
    }

    /// Draws the picture: SCREEN_W x SCREEN_H pixels, 0x00RRGGBB.
    pub fn render(&self, out: &mut [u32]) {
        self.bus.render(out);
    }

    /// The sound made since the last call: stereo, interleaved, at
    /// OUTPUT_RATE samples a second.
    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.bus.audio)
    }

    /// Research: the sprites in use, one line each.
    pub fn sprite_list(&self) -> Vec<String> {
        self.bus.sprite_list()
    }

    /// What the system has written to its serial port (its log).
    pub fn log(&self) -> &str {
        &self.bus.uart
    }

    /// The NAND, including what the system has saved to it (players, scores).
    pub fn nand(&self) -> &[u8] {
        &self.bus.nand.flash
    }

    /// Whether the NAND has changed since the last `nand_saved`.
    pub fn nand_dirty(&self) -> bool {
        self.bus.nand.dirty
    }

    pub fn nand_saved(&mut self) {
        self.bus.nand.dirty = false;
    }

    /// The machine's state (without the NAND, which is saved on its own).
    pub fn save_state(&self) -> Vec<u8> {
        let mut out = STATE_VERSION.to_le_bytes().to_vec();
        out.extend(bincode::serialize(self).expect("serialise the machine"));
        out
    }

    /// Restores a state from `save_state`, keeping the current NAND.
    pub fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let version = state.get(..4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        if version != Some(STATE_VERSION) {
            return Err("this state is from another version of the Zippity model".into());
        }
        let mut z: Zippity = bincode::deserialize(&state[4..]).map_err(|e| e.to_string())?;
        std::mem::swap(&mut z.bus.nand.flash, &mut self.bus.nand.flash);
        z.bus.nand.dirty = self.bus.nand.dirty;
        z.bus.lines_dirty = true;
        z.bus.probe = std::mem::take(&mut self.bus.probe);
        *self = z;
        Ok(())
    }

    /// Research access: the CPU, and the bus's probe, memory and logs.
    pub fn cpu(&self) -> &Arm {
        &self.cpu
    }

    pub fn probe(&mut self) -> &mut probe::Probe {
        &mut self.bus.probe
    }

    pub fn ram(&self) -> &[u8] {
        &self.bus.ram
    }

    pub fn iram(&self) -> &[u8] {
        &self.bus.iram
    }

    pub fn nand_log(&mut self) -> &mut Vec<String> {
        &mut self.bus.nand.log
    }

    pub fn set_nand_logging(&mut self, on: bool) {
        self.bus.nand.logging = on;
    }

    /// Runs one instruction (for research tools that watch every step).
    pub fn step(&mut self) -> bool {
        let now = self.bus.now;
        if now % FRAME == FRAME - 1 {
            self.bus.frame();
        }
        if now.is_multiple_of(256) {
            self.bus.tick();
        }
        let (irq, fiq) = self.bus.lines();
        self.cpu.irq_line = irq;
        self.cpu.fiq_line = fiq;
        self.bus.pc = self.cpu.r[15];
        let ran = self.cpu.step(&mut self.bus);
        self.bus.now += 1;
        ran
    }

    /// Undefined instructions met since the last call: (address, word).
    pub fn take_undefined(&mut self) -> Vec<(u32, u32)> {
        std::mem::take(&mut self.cpu.undefined_log)
    }
}
