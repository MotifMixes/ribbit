//! The LeapFrog iQuest (2004) and Turbo Twist Extreme (2004): handhelds
//! with a dot-matrix LCD and cartridges, built on the LeapPad's 8051-family
//! chip. See docs/iquest.md and docs/turbotwist.md.

pub mod bus;
pub mod e2;
pub mod flash;
pub mod gpio;
pub mod keys;
pub mod lcd;

use bus::Bus;
use ribbit_leappad::mcs51::Mcs51;

/// Machine cycles per second: the LeapPad's stand-in (UNKNOWN).
pub const MACHINE_HZ: u32 = 4_000_000;
/// The clock on the T0/T1 pins (the LeapPad's measured 16 kHz; EVIDENCE that
/// it is the same here: the same kernel timer code).
pub const TIMER_PIN_HZ: u32 = 16_000;

/// A cartridge's sparse checksum, as the base checks it ("Sparse CSum:
/// calc>x..:Cart>x.."): the LeapPad's (ribbit_leappad::checksum), over the
/// 8 MB window. VERIFIED: it gives the base's own numbers on the iQuest
/// cartridges (e.g. EEFB for the Starter Pack).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checksum {
    pub calc: u16,
    pub stored: u16,
    /// Cartridge offset of the stored sum.
    pub at: usize,
}

pub fn checksum(cart: &[u8]) -> Option<Checksum> {
    let field = |o: usize| Some(u32::from_be_bytes(cart.get(o..o + 4)?.try_into().ok()?));
    let (start, end, data) = (field(0x1014C)?, field(0x10150)?, field(0x10154)?);
    let base = bus::CART_BASE;
    if !(base <= start && start < data && data <= end && end < base + bus::CART_SPAN) {
        return None;
    }
    let off = |a: u32| (a - base) as usize;
    let at = off(data) + 1;
    let stored = u16::from_be_bytes(cart.get(at..at + 2)?.try_into().ok()?);
    let calc = (start..=data).step_by(0x3E5).fold(0u16, |s, a| s.wrapping_add(*cart.get(off(a)).unwrap_or(&0xFF) as u16));
    Some(Checksum { calc, stored, at })
}

/// Makes a damaged dump's checksum match, as Ribbit does for LeapPad
/// cartridges (the app says so). Returns the sums if it changed anything.
pub fn repair_checksum(cart: &mut [u8]) -> Option<Checksum> {
    let c = checksum(cart).filter(|c| c.calc != c.stored)?;
    cart[c.at..c.at + 2].copy_from_slice(&c.calc.to_be_bytes());
    Some(c)
}

/// Whether `data` looks like an iQuest cartridge: no code of its own (no
/// boot stub), 4 or 8 MB (give or take a damaged dump's few bytes), and a
/// ROM Info Block at 0x10100 whose second word is FEFFh (all 11 good dumps
/// of the 13 found).
pub fn looks_like_cartridge(data: &[u8]) -> bool {
    let size_ok = [0x40_0000usize, 0x80_0000].iter().any(|&n| data.len() <= n && data.len() + 0x100 >= n);
    size_ok && !data.starts_with(&BOOT_STUB) && data.get(0x10102..0x10104) == Some(&[0xFE, 0xFF][..])
}

/// Which handheld a base ROM is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Model {
    /// LeapFrog iQuest (base 152-10663 "IQuest Full BaseROM"): 90 x 64 LCD,
    /// keyboard, flash for the user's data.
    IQuest,
    /// LeapFrog Turbo Twist Extreme (base 152-10696 "TTExtreme FullBase"):
    /// 96 x 32 LCD, the twist handle and buttons.
    TurboTwistExtreme,
}

/// The boot stub every LeapPad-family ROM starts with (ribbit-leappad).
const BOOT_STUB: [u8; 14] = [0x02, 0x00, 0x03, 0x90, 0xFC, 0x06, 0xE0, 0xF8, 0xA3, 0xE0, 0xF9, 0x90, 0xFC, 0x08];

/// What a dump is: a base ROM of one of the two models (by the name in its
/// header at 0x38, "ToolPad V2.3.x IQuest FullBase" / "TTExtreme FullBase"),
/// or None.
pub fn identify(data: &[u8]) -> Option<Model> {
    if data.len() < 0x2_0000 || data[..BOOT_STUB.len()] != BOOT_STUB {
        return None;
    }
    let head = &data[0x30..0x80];
    let has = |s: &[u8]| head.windows(s.len()).any(|w| w == s);
    if has(b"IQuest FullBase") {
        Some(Model::IQuest)
    } else if has(b"TTExtreme FullBase") {
        Some(Model::TurboTwistExtreme)
    } else {
        None
    }
}

impl Model {
    /// The LCD's size in pixels (lcd.rs).
    pub fn screen(self) -> (usize, usize) {
        match self {
            Model::IQuest => (90, 64),
            Model::TurboTwistExtreme => (96, 32),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::IQuest => "iQuest",
            Model::TurboTwistExtreme => "Turbo Twist Extreme",
        }
    }
}

/// The kernel's "a key was pressed" flag, bit address 1Fh (lib.rs
/// run_cycles).
const ACTIVITY_BYTE: usize = 0x23;
const ACTIVITY_BIT: u8 = 0x80;

/// The LCD's colours (0RGB).
pub const GLASS: u32 = 0x00B8_C4A8;
pub const INK: u32 = 0x0020_2820;

/// Frames a second the app runs (the LCD has no frame timing of its own
/// that Ribbit knows of).
pub const FRAME_HZ: u32 = 60;
/// The sound's sample rate (the voice engine's).
pub const AUDIO_RATE: u32 = 8000;

pub struct Machine {
    pub model: Model,
    /// "Turn off when idle" is off (Bus::keep_awake): the apps' idle
    /// power-down is refused.
    pub keep_awake: bool,
    /// Machine cycles since keep_awake last marked the kernel active.
    awake_phase: u32,
    pub cpu: Mcs51,
    pub bus: Bus,
    pin_phase: u64,
    pub hz: u32,
    pub pin_hz: u32,
}

impl Machine {
    pub fn new(bios: Vec<u8>) -> Self {
        let model = identify(&bios).unwrap_or(Model::IQuest);
        let mut bus = Bus::new(bios);
        if model == Model::IQuest {
            bus.flash = Some(Default::default());
        }
        Machine { model, keep_awake: false, awake_phase: 0, cpu: Mcs51::new(), bus, pin_phase: 0, hz: MACHINE_HZ, pin_hz: TIMER_PIN_HZ }
    }

    pub fn insert_cartridge(&mut self, cart: Vec<u8>) {
        self.bus.cart = Some(cart);
    }

    /// Presses or lets go of a key by name (keys.rs). Unknown names are
    /// ignored; returns whether the name was known.
    pub fn set_key(&mut self, name: &str, down: bool) -> bool {
        if name == "power" {
            if down {
                self.press_power();
            }
            return true;
        }
        let Some(w) = keys::wire(self.model, name) else { return false };
        self.set_wire(w, down);
        true
    }

    /// The ON/OFF button, as on the LeapPad: while running the firmware is
    /// told (FC00 bit 3) and powers itself down; while off, the processor
    /// starts again with its memory kept.
    pub fn press_power(&mut self) {
        self.bus.since_power_press = 0;
        if self.is_off() {
            self.cpu.reset();
        } else {
            self.bus.status |= 0x08;
        }
    }

    /// Whether the processor has stopped itself (PCON power-down).
    pub fn is_off(&self) -> bool {
        self.cpu.power_down
    }

    /// Joins or parts two pins directly (research; keys.rs names them).
    pub fn set_wire(&mut self, w: gpio::Wire, down: bool) {
        self.bus.pressed.retain(|p| *p != w);
        if down {
            self.bus.pressed.push(w);
        }
    }

    /// What the LCD shows now.
    pub fn frame(&self) -> lcd::Frame {
        lcd::frame(&self.bus.io, &self.bus.ram, self.model == Model::TurboTwistExtreme)
    }

    pub fn take_log(&mut self) -> String {
        String::from_utf8_lossy(&std::mem::take(&mut self.bus.uart)).into_owned()
    }

    pub fn step(&mut self) -> u32 {
        self.bus.pc = self.cpu.pc;
        let c = self.cpu.step(&mut self.bus);
        self.bus.tick(c, self.hz);
        if std::mem::take(&mut self.bus.dsp_irq) {
            self.cpu.set_int_pin(1, false);
            self.cpu.set_int_pin(1, true);
        }
        self.pin_phase += c as u64 * self.pin_hz as u64;
        let edges = (self.pin_phase / self.hz as u64) as u32;
        if edges > 0 {
            self.pin_phase %= self.hz as u64;
            self.cpu.count_pin(0, edges);
            self.cpu.count_pin(1, edges);
        }
        c
    }

    /// Runs one 1/60 s frame.
    pub fn run_frame(&mut self) {
        self.run_cycles(self.hz / FRAME_HZ);
    }

    /// The LCD as 0RGB pixels at the model's size: dark pixels on the
    /// pale grey-green glass.
    pub fn render(&self, out: &mut Vec<u32>) {
        let (w, h) = self.model.screen();
        out.clear();
        out.resize(w * h, GLASS);
        let f = self.frame();
        for y in 0..h.min(f.height) {
            for x in 0..w.min(f.width) {
                if f.get(x, y) {
                    out[y * w + x] = INK;
                }
            }
        }
    }

    /// The sound made since last asked (AUDIO_RATE, mono).
    pub fn take_audio(&mut self, out: &mut Vec<i16>) {
        out.resize(self.bus.sound.output.len(), 0);
        self.bus.sound.output.pop_into(out);
    }

    /// What the machine keeps across power-offs: the iQuest's flash or the
    /// Turbo Twist Extreme's E2.
    pub fn save_data(&self) -> &[u8] {
        match &self.bus.flash {
            Some(f) => &f.data,
            None => &self.bus.e2.data,
        }
    }

    pub fn load_save(&mut self, data: &[u8]) {
        match &mut self.bus.flash {
            Some(f) => f.load(data),
            None => self.bus.e2.load(data),
        }
    }

    /// Whether the save changed since `save_kept` was last called.
    pub fn save_dirty(&self) -> bool {
        self.bus.flash.as_ref().map_or(self.bus.e2.dirty, |f| f.dirty)
    }

    pub fn save_kept(&mut self) {
        match &mut self.bus.flash {
            Some(f) => f.dirty = false,
            None => self.bus.e2.dirty = false,
        }
    }

    pub fn run_cycles(&mut self, cycles: u32) {
        self.bus.keep_awake = self.keep_awake;
        // The kernel's own inactivity timeout (about 5 minutes on the
        // iQuest, besides the apps' idle states that Bus::keep_awake
        // refuses): its idle loop (iQuest base app 0xF836 -> 0xADBC, TT
        // 0x2274B) renews the deadline whenever bit 1Fh (23h.7) is set,
        // which the key scan sets on every key (kernel 0x5799, 0x582E).
        // EVIDENCE; with the setting off it is set once a second.
        if self.keep_awake {
            self.awake_phase += cycles;
            if self.awake_phase >= self.hz {
                self.awake_phase = 0;
                self.cpu.iram[ACTIVITY_BYTE] |= ACTIVITY_BIT;
            }
        }
        let mut done = 0;
        while done < cycles {
            done += self.step();
        }
    }
}
