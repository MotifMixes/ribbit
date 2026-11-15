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

pub struct Machine {
    pub model: Model,
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
        Machine { model, cpu: Mcs51::new(), bus, pin_phase: 0, hz: MACHINE_HZ, pin_hz: TIMER_PIN_HZ }
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

    pub fn run_cycles(&mut self, cycles: u32) {
        let mut done = 0;
        while done < cycles {
            done += self.step();
        }
    }
}
