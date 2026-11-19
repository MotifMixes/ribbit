//! The LeapFrog ClickStart: My First Computer (2007), a TV console with a
//! keyboard and mouse, built on a Sunplus SPG28x: a 16-bit unSP CPU at
//! 27 MHz with the SPG2xx video, sound and I/O units. The system ROM (8 MB)
//! holds the menus and built-in activities; cartridges are 4 MB ROMs that
//! take the place of its lower half.
//!
//! ```no_run
//! # let bios = Vec::new();
//! let mut cs = ribbit_clickstart::ClickStart::new(&bios);
//! let mut frame = vec![0u32; ribbit_clickstart::SCREEN_W * ribbit_clickstart::SCREEN_H];
//! loop {
//!     cs.run_frame();
//!     cs.render(&mut frame);
//! }
//! ```

mod bus;
pub mod cpu;
mod eeprom;
pub mod idle;
mod sio_audio;
mod sound;
mod video;

pub use bus::{CLOCK, FRAME};
pub use sound::OUTPUT_RATE;
pub use video::{SCREEN_H, SCREEN_W};

use bus::Bus;
use cpu::Cpu;
use serde::{Deserialize, Serialize};

/// The system ROM's size in bytes.
pub const BIOS_SIZE: usize = 0x80_0000;
/// A cartridge ROM's size in bytes.
pub const CART_SIZE: usize = 0x40_0000;
/// The 24C08 EEPROM's size in bytes (the players).
pub const EEPROM_SIZE: usize = eeprom::SIZE;

/// The keyboard's code for a character: letters, digits, space, '.' and
/// '?'. From the system ROM's own key table (its function at 0x1bac5e,
/// checked by running it): codes 01-1a are the letters, 1b-24 the digits
/// 0-9, 25 gives '?', 26 '.' and 27 a space. (MAME's driver has other
/// digit codes, all but '9' wrong: its '8', 2f, is the Hint key.)
pub fn key_code(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    Some(match c {
        'a'..='z' => c as u8 - b'a' + 1,
        '0'..='9' => c as u8 - b'0' + 0x1b,
        '?' => KEY_QUESTION,
        '.' => KEY_PERIOD,
        ' ' => 0x27,
        _ => return None,
    })
}

// The keyboard's other keys, from the system ROM's key table (what it
// turns each code into is in brackets; all checked by running it).

/// '?' (63).
pub const KEY_QUESTION: u8 = 0x25;
/// '.' (46).
pub const KEY_PERIOD: u8 = 0x26;
/// Enter (0xc8): the system's text entry (0x00d594) takes it as "done";
/// the Player Profile screen ignores it.
pub const KEY_ENTER: u8 = 0x28;
/// Caps (0xca): the ROM flips its upper/lower case flag on each press.
pub const KEY_CAPS: u8 = 0x29;
/// Backspace (0xc9): deletes the last letter of a name.
pub const KEY_BACKSPACE: u8 = 0x2a;
/// The four yellow picture keys (0xe6, 0xe8, 0xe9, 0xea), not the arrows:
/// the arrows are held bits in the packet (ARROW_). Activities answer them
/// with a fun reaction, e.g. Dora the Explorer plays one of three clips for
/// each and Thomas and Friends sounds a whistle; with the system's key
/// sounds on, each plays a sound. Which picture (smiley, note, boat, cat) is
/// which code is UNVERIFIED (docs/clickstart.md).
pub const KEY_SPECIAL: [u8; 4] = [0x2b, 0x2c, 0x2d, 0x2e];
/// Hint (0xe7): the ROM sets its "hint asked for" flag (0x11e1).
pub const KEY_HINT: u8 = 0x2f;
/// Pause (0xcb), the one yellow key left once 28-2e are placed: the
/// system hands it to the activity as a message (0x00fa55). What a game
/// does with it is UNVERIFIED (nothing paused headlessly).
pub const KEY_SYSTEM: u8 = 0x30;

/// The arrow keys: held bits in the first byte of every packet, next to the
/// mouse button (bit 4). The system ROM hands bits 0-4 to the activities
/// (0x3f053c); a game reads them as held directions, e.g. Finding Nemo
/// moves by +32 or -32 in x on bits 2 and 1 and by +16 or -16 in y on bits
/// 0 and 3, and Dora the Explorer drops an opposite pair held together.
pub const ARROW_DOWN: u8 = 0x01;
pub const ARROW_LEFT: u8 = 0x02;
pub const ARROW_RIGHT: u8 = 0x04;
pub const ARROW_UP: u8 = 0x08;

/// The first byte's bits 5-6, never all clear: a 00 byte starts a packet
/// (0x3f0561), so no byte after it may be 00 (the key slots use 3f for no
/// key and the movement bytes add 1). The ROM's status call (0x3f04d9)
/// turns them into a value no cartridge reads; 0x60 gives the same value
/// as MAME's 0x01 without holding an arrow (MAME's bit 0 is ARROW_DOWN).
const PACKET_BASE: u8 = 0x60;
/// The first byte's mouse button bit.
const MOUSE_BUTTON: u8 = 0x10;

/// Mouse units per pointer pixel. The system ROM's pointer code (0x3f03cc)
/// adds each unit times a speed (12, set at start-up; no cartridge sets
/// another) to a position in 1/32 pixels, clamped to 0-319 and 0-239
/// pixels: 32/12 units move it one pixel. But the screens put the pointer
/// back through the ROM's setter (0x3f0380) every frame or two, which keeps
/// whole pixels only (x << 5), so a packet's fraction of a pixel is lost,
/// rounded down: 1 or 2 units move it 0 pixels, -1 or -2 one pixel left.
/// `mouse_pixels` sends what reaches the wanted pixel.
pub const MOUSE_UNITS_PER_PIXEL: f32 = 32.0 / 12.0;

/// RAM words of the ROM's pointer: x (then y) in 1/32 pixels (y up), the
/// speeds, and the movement the UART has taken that the pointer code
/// hasn't added yet (0x3f0540 reads and clears it).
const POINTER_X: u32 = 0x2c;
const POINTER_SPEED_X: u32 = 0x2e;
const POINTER_PENDING_X: u32 = 0x26;

/// The save state format's version (bumped when the layout changes).
const STATE_VERSION: u32 = 4;

#[derive(Serialize, Deserialize)]
pub struct ClickStart {
    pub cpu: Cpu,
    bus: Bus,
    instructions: u64,
    /// What the keyboard holds: the arrows and the mouse button (the first
    /// byte's bits 0-4) and the key in the first slot (3f = none).
    #[serde(skip)]
    held: u8,
    #[serde(skip, default = "no_key")]
    held_key: u8,
    /// Keep the console on however long it is left alone: the home
    /// screen's idle count (`idle`, 150 s) is set back to 0 once a second.
    /// Off (the default), the console turns itself off as a real one does.
    #[serde(skip)]
    pub keep_awake: bool,
    /// Where the system ROM keeps that count (None: not found).
    #[serde(skip)]
    idle: Option<idle::IdleCount>,
}

fn no_key() -> u8 {
    0x3f
}

/// Words from a ROM dump (the system ROM and cartridges are both stored
/// low byte first; its header text "chksum:... body:"SPG240"" reads in
/// order).
fn words_le(data: &[u8]) -> Vec<u16> {
    data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect()
}

impl ClickStart {
    /// A ClickStart with the system ROM `bios` (the 8 MB dump), switched on.
    pub fn new(bios: &[u8]) -> Self {
        let rom = words_le(bios);
        let idle = idle::IdleCount::find(&rom);
        let mut cs = ClickStart {
            cpu: Cpu::new(),
            bus: Bus::new(rom),
            instructions: 0,
            held: 0,
            held_key: 0x3f,
            keep_awake: false,
            idle,
        };
        cs.reset();
        cs
    }

    /// Puts a cartridge (its ROM dump) in; takes effect at the next reset.
    pub fn insert_cartridge(&mut self, cart: &[u8]) {
        self.bus.cart = Some(words_le(cart));
    }

    pub fn remove_cartridge(&mut self) {
        self.bus.cart = None;
    }

    pub fn reset(&mut self) {
        self.cpu.reset(&mut self.bus);
    }

    pub fn instructions(&self) -> u64 {
        self.instructions
    }

    pub fn cycles(&self) -> u64 {
        self.bus.now
    }

    /// The system turned the console off (its goodbye animation done).
    pub fn powered_off(&self) -> bool {
        self.bus.powered_off
    }

    pub fn frames(&self) -> u64 {
        self.bus.frames
    }

    /// Runs one instruction (taking an interrupt after it if one is due).
    #[inline]
    fn step(&mut self) -> bool {
        self.bus.ds = self.cpu.ds();
        let cycles = cpu_step(&mut self.cpu, &mut self.bus);
        self.instructions += 1;
        self.bus.now += cycles as u64;
        if self.bus.now >= self.bus.next_event {
            return self.bus.service();
        }
        false
    }

    /// Research: runs one instruction.
    pub fn step_one(&mut self) {
        self.step();
    }

    /// Runs until the next vertical blank starts (one frame).
    pub fn run_frame(&mut self) {
        let limit = self.bus.now + 2 * FRAME;
        while !self.step() {
            if self.bus.now > limit {
                break;
            }
        }
        if self.keep_awake && self.bus.frames % 60 == 0 {
            self.reset_idle_count();
        }
    }

    /// Sets the home screen's idle count back to 0, as a key would (idle.rs).
    pub fn reset_idle_count(&mut self) {
        if let Some(at) = self.idle.and_then(|c| c.address(&self.bus.ram)) {
            self.bus.ram[at as usize] = 0;
        }
    }

    /// Research and tests: the home screen's idle count, and the count it
    /// turns the console off past (None: not on the home screen).
    pub fn idle_count(&self) -> Option<(u16, u16)> {
        let c = self.idle?;
        c.address(&self.bus.ram).map(|at| (self.bus.ram[at as usize], c.limit))
    }

    /// Research and tests: sets the home screen's idle count (to try the
    /// power-off without waiting).
    pub fn set_idle_count(&mut self, count: u16) {
        if let Some(at) = self.idle.and_then(|c| c.address(&self.bus.ram)) {
            self.bus.ram[at as usize] = count;
        }
    }

    /// Runs `cycles` CPU cycles.
    pub fn run(&mut self, cycles: u64) {
        let end = self.bus.now + cycles;
        while self.bus.now < end {
            self.step();
        }
    }

    /// The picture: SCREEN_W x SCREEN_H pixels, 0x00RRGGBB.
    pub fn render(&self, out: &mut [u32]) {
        out.copy_from_slice(&self.bus.video.frame);
    }

    /// The machine's state (without the ROMs and the EEPROM's contents).
    pub fn save_state(&self) -> Vec<u8> {
        let mut out = STATE_VERSION.to_le_bytes().to_vec();
        out.extend(bincode::serialize(self).expect("serialise the machine"));
        out
    }

    /// Restores a state from `save_state`, keeping the ROMs, the cartridge
    /// in the slot and the EEPROM's current contents (the players).
    pub fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let version = state.get(..4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        if version != Some(STATE_VERSION) {
            return Err("this state is from another version of the ClickStart model".into());
        }
        let mut cs: ClickStart = bincode::deserialize(&state[4..]).map_err(|e| e.to_string())?;
        let (b, old) = (&mut cs.bus, &mut self.bus);
        std::mem::swap(&mut b.rom, &mut old.rom);
        std::mem::swap(&mut b.cart, &mut old.cart);
        std::mem::swap(&mut b.eeprom.data, &mut old.eeprom.data);
        b.eeprom.dirty = old.eeprom.dirty;
        // The picture stays until the next frame is drawn.
        std::mem::swap(&mut b.video.frame, &mut old.video.frame);
        b.sound.trace = old.sound.trace.take();
        // The keyboard holds what it holds.
        cs.held = self.held;
        cs.held_key = self.held_key;
        cs.keep_awake = self.keep_awake;
        cs.idle = self.idle;
        *self = cs;
        Ok(())
    }

    /// The 24C08 EEPROM's contents (EEPROM_SIZE bytes).
    pub fn eeprom(&self) -> &[u8] {
        &self.bus.eeprom.data
    }

    /// Loads the EEPROM's contents (a shorter file fills from the start;
    /// the rest stays blank).
    pub fn set_eeprom(&mut self, data: &[u8]) {
        let n = data.len().min(EEPROM_SIZE);
        self.bus.eeprom.data[..n].copy_from_slice(&data[..n]);
        self.bus.eeprom.dirty = false;
    }

    /// Whether the system has written the EEPROM since `eeprom_saved`.
    pub fn eeprom_dirty(&self) -> bool {
        self.bus.eeprom.dirty
    }

    pub fn eeprom_saved(&mut self) {
        self.bus.eeprom.dirty = false;
    }

    /// Research: logs sound channel starts (taken with `take_sound_log`).
    pub fn trace_sound(&mut self) {
        self.bus.sound.trace = Some(Vec::new());
    }

    pub fn take_sound_log(&mut self) -> Vec<String> {
        self.bus.sound.trace.as_mut().map(std::mem::take).unwrap_or_default()
    }

    /// The sound made since the last call: stereo, interleaved, at
    /// OUTPUT_RATE samples a second.
    pub fn take_audio(&mut self) -> Vec<i16> {
        std::mem::take(&mut self.bus.sound.out)
    }

    /// Sends a 6-byte packet on the input line, each byte inverted, then
    /// the inverted sum of the bytes, as MAME's driver models the
    /// keyboard's microcontroller (the mouse plugs into the keyboard). The
    /// system ROM's receiver (0x3f0554) takes a 00 byte as a packet's start
    /// and checks the sum. Byte 1: the arrows (bits 0-3) and the mouse
    /// button (bit 4), which the ROM hands to the activities, and bits 5-6
    /// (PACKET_BASE). Bytes 2 and 3: two key slots (low 6 bits, 3f = none;
    /// a change makes key-up and key-down events), with bit 7 the sign of
    /// the mouse's x and y. Bytes 4 and 5: the mouse's x and y movement,
    /// 9-bit with that sign, plus 1 when not negative; the ROM skips them
    /// when both key slots read 3f.
    fn send_packet(&mut self, p: [u8; 6]) {
        let sum = p.iter().map(|&b| b as u32).sum::<u32>() as u8 ^ 0xff;
        self.bus.input_bytes.extend(p.iter().map(|b| b ^ 0xff));
        self.bus.input_bytes.push_back(sum);
    }

    /// A key pressed (`code`, see `key_code` and the KEY_ constants) or
    /// released (all keys up). MAME sends 2f in the second key slot, which
    /// holds the Hint key down for good; this sends 3f (no key).
    pub fn key(&mut self, code: u8, down: bool) {
        self.held_key = if down { code } else { 0x3f };
        self.send_packet([0x00, PACKET_BASE | self.held, self.held_key, 0x3f, 0x01, 0x01]);
    }

    /// An arrow key (ARROW_UP, ARROW_DOWN, ARROW_LEFT, ARROW_RIGHT) pressed
    /// or released. The arrows are held bits, sent with every packet; this
    /// sends one without movement, keeping a held key held.
    pub fn arrow(&mut self, arrow: u8, down: bool) {
        if down {
            self.held |= arrow & 0x0f;
        } else {
            self.held &= !arrow;
        }
        self.send_packet([0x00, PACKET_BASE | self.held, self.held_key, 0x3f, 0x01, 0x01]);
    }

    /// The mouse moved by (dx, dy) units (y down) with the button held or
    /// not. The system ROM adds a unit times 12/32 of a pixel to its
    /// pointer (MOUSE_UNITS_PER_PIXEL), with no acceleration. The first key
    /// slot reads 3e (no key, as in MAME), so the ROM takes the movement.
    pub fn mouse(&mut self, dx: i32, dy: i32, button: bool) {
        // The first key slot's 3e lets go of a held key.
        self.held_key = 0x3f;
        if button {
            self.held |= MOUSE_BUTTON;
        } else {
            self.held &= !MOUSE_BUTTON;
        }
        // The ROM's pointer y counts up the screen.
        let (dx, dy) = (dx.clamp(-255, 254), (-dy).clamp(-255, 254));
        let sign = |v: i32| if v < 0 { 0x80 } else { 0 };
        let low = |v: i32| if v < 0 { (v + 256) as u8 } else { (v + 1) as u8 };
        self.send_packet([
            0x00,
            PACKET_BASE | self.held,
            0x3e | sign(dx),
            0x3f | sign(dy),
            low(dx),
            low(dy),
        ]);
    }

    /// The mouse moved by (dx, dy) whole pointer pixels (y down) with the
    /// button held or not: sends the units that put the ROM's pointer in
    /// the middle of the pixel that far from where it is (or will be, with
    /// movement taken but not yet added). Sending MOUSE_UNITS_PER_PIXEL
    /// times the pixels instead lost a pixel a packet one way and gained
    /// one the other (see MOUSE_UNITS_PER_PIXEL): moved slowly, the pointer
    /// crept left and down and wouldn't come back. At most about 90 pixels
    /// a packet (255 units).
    pub fn mouse_pixels(&mut self, dx: i32, dy: i32, button: bool) {
        let ux = self.pointer_units(0, dx);
        // The ROM's y counts up the screen; `mouse` takes y down.
        let uy = -self.pointer_units(1, -dy);
        self.mouse(ux, uy, button);
    }

    /// Units that move the ROM's pointer axis (0 x, 1 y; y up) by n pixels.
    fn pointer_units(&self, axis: u32, n: i32) -> i32 {
        if n == 0 {
            return 0;
        }
        let speed = match self.peek(POINTER_SPEED_X + axis) as i16 as i32 {
            s @ 1..=64 => s,
            _ => 12,
        };
        let pending = self.peek(POINTER_PENDING_X + axis) as i16 as i32;
        let pos = self.peek(POINTER_X + axis) as i16 as i32 + pending * speed;
        // The fraction of a pixel read here may still be dropped by the
        // setter before this packet's movement is added (the screens call
        // it every other frame), or kept (a screen that doesn't call it).
        // Moves of u * speed in [lo, hi) reach pixel + n either way.
        let f = pos & 31;
        let (lo, hi) = (32 * n, 32 * n + 32 - f);
        let fits = |u: i32| (lo..hi).contains(&(u * speed));
        let s = speed as f32;
        let mut u = ((lo + hi) as f32 / 2.0 / s).round() as i32;
        if !fits(u) {
            u = (lo as f32 / s).ceil() as i32;
        }
        if !fits(u) {
            // No move fits both: aim from where it is.
            u = ((32 * n + 16 - f) as f32 / s).round() as i32;
        }
        u.clamp(-255, 254)
    }

    /// Whether the input line has caught up (all packets delivered).
    pub fn input_idle(&self) -> bool {
        self.bus.input_bytes.is_empty()
    }

    /// What the system has sent on the UART (research).
    pub fn take_uart_out(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.bus.uart_out)
    }

    /// Research: word `addr` as the CPU would read it, without side effects
    /// for RAM and ROM.
    pub fn peek(&self, addr: u32) -> u16 {
        self.bus.peek(addr)
    }

    pub fn io_regs(&self) -> &[u16] {
        &self.bus.io
    }

    pub fn video_regs(&self) -> &[u16] {
        &self.bus.video.regs
    }

    pub fn line(&self) -> u32 {
        self.bus.line
    }
}

#[inline]
fn cpu_step(cpu: &mut Cpu, bus: &mut Bus) -> u32 {
    cpu.step(bus)
}
