//! The original LeapFrog LeapPad (1999): a book-and-pen learning toy built
//! on an 8051-family microcontroller (LeapFrog FS80A363). See
//! docs/leappad.md for what is known about the hardware and how sure it is.
//!
//! ```no_run
//! # let bios = Vec::new(); let cart = Vec::new();
//! let mut lp = ribbit_leappad::Machine::new(bios);
//! lp.insert_cartridge(cart);
//! lp.run_cycles(ribbit_leappad::MACHINE_HZ / 60);
//! print!("{}", String::from_utf8_lossy(&lp.bus.uart));
//! ```

pub mod book;
pub mod bus;
pub mod calibration;
pub mod disasm;
pub mod eeprom;
pub mod layout;
pub mod mcs51;
pub mod pen;
pub mod sound;
pub mod words;

use bus::Bus;
use mcs51::Mcs51;

/// Machine cycles per second. UNKNOWN. MAME guesses a 9.6 MHz 8032 (800 000), but
/// the kernel cannot keep up with its own 2.5 kHz EX1 work below about 2 000 000
/// (VERIFIED with lpboot HZ=...); twice that is a stand-in until the real rate is found.
pub const MACHINE_HZ: u32 = 4_000_000;

/// The clock on the T0 and T1 pins, which the kernel's timers count
/// (TMOD 66h: both counters, auto-reload). EVIDENCE for the two being the
/// same clock (the kernel keeps T1 at exactly 2.5 times T0's overflow rate
/// with reloads of 64 and 160 counts). 16 kHz (T0 every 10 ms) is measured:
/// the base ROM's power-on tune, whose notes the music sequencer steps on
/// T0, then starts its notes 0, 0.164, 0.333, 0.497 s apart, against 0,
/// 0.17, 0.335, 0.5 s in a recording of a real pad (docs/leappad.md,
/// "Timers"). 160 kHz and 32.768 kHz, tried before, played it 10 and 2
/// times too fast.
pub const TIMER_PIN_HZ: u32 = 16_000;

/// What kind of LeapPad dump a file is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Image {
    /// A base ROM (the system software in the pad itself).
    BaseRom,
    /// A cartridge (a book's software).
    Cartridge,
}

/// The 14-byte boot stub every LeapPad base ROM and cartridge starts with
/// (docs/leappad.md, "Boot stub"; VERIFIED on 3 base ROMs and 19 of the 20
/// cartridges in the user's set; the Mind Station cartridge lacks it).
const BOOT_STUB: [u8; 14] = [0x02, 0x00, 0x03, 0x90, 0xFC, 0x06, 0xE0, 0xF8, 0xA3, 0xE0, 0xF9, 0x90, 0xFC, 0x08];

/// Whether `data` is a LeapPad base ROM or cartridge. Base ROMs hold the
/// cartridge check ("Cart found:"); cartridges, which bring their own
/// kernel, do not.
pub fn identify(data: &[u8]) -> Option<Image> {
    if data.len() < 0x2_0000 || data[..BOOT_STUB.len()] != BOOT_STUB {
        return None;
    }
    let base = data.windows(11).any(|w| w == b"Cart found:");
    Some(if base { Image::BaseRom } else { Image::Cartridge })
}

/// A cartridge's sparse checksum, as the base checks it before running the
/// cartridge ("Sparse CSum:calc>x9AC7:Cart>x9EBB"). VERIFIED on the six ILA2
/// cartridges of the user's set that have it (base 152-10620 traced, cart
/// reads during "Validating..."): the RIB holds three big-endian physical
/// addresses at cartridge offset 0x1014C (start), 0x10150 (end of the chip)
/// and 0x10154 (end of the data); the sum is the 16-bit sum of every 997th
/// (0x3E5) byte from the start to the end of the data, stored big-endian
/// just after it. Cartridges without these fields (Classic, older RIBs) give
/// None.
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
    if !(bus::CART_BASE <= start && start < data && data <= end && end < bus::CART_BASE + 0x40_0000) {
        return None;
    }
    let off = |a: u32| (a - bus::CART_BASE) as usize;
    let at = off(data) + 1;
    let stored = u16::from_be_bytes(cart.get(at..at + 2)?.try_into().ok()?);
    let calc = (start..=data).step_by(0x3E5).fold(0u16, |s, a| s.wrapping_add(*cart.get(off(a)).unwrap_or(&0xFF) as u16));
    Some(Checksum { calc, stored, at })
}

/// Makes a damaged dump's checksum match (some of the sampled bytes differ
/// from the chip's, so the base would refuse it as "Bad Cart"), the way the
/// Classic list is opened (Bus::classic). Returns the sums if it did.
pub fn repair_checksum(cart: &mut [u8]) -> Option<Checksum> {
    let c = checksum(cart).filter(|c| c.calc != c.stored)?;
    cart[c.at..c.at + 2].copy_from_slice(&c.calc.to_be_bytes());
    Some(c)
}

/// The address line a dump lost, if one was not read when it was made: then
/// each byte equals the one with that address bit flipped, and half the
/// cartridge is missing (500-00104 Tad Goes Shopping: A8, 83% of bytes equal
/// their twin 256 bytes away, against under 7% for every other line).
pub fn lost_address_line(cart: &[u8]) -> Option<u32> {
    (8..20).find(|&bit| {
        let m = 1usize << bit;
        let (mut same, mut n) = (0, 0);
        for i in (0..cart.len()).step_by(97).filter(|&i| i & m == 0 && i | m < cart.len()) {
            n += 1;
            same += (cart[i] == cart[i | m]) as usize;
        }
        n > 100 && same * 4 > n * 3
    })
}

/// A LeapPad: the processor and what is on its bus.
pub struct Machine {
    pub cpu: Mcs51,
    pub bus: Bus,
    /// Timer pin clock phase, in units of 1 / `hz`.
    pin_phase: u64,
    /// Machine cycles per second (MACHINE_HZ unless set for research).
    pub hz: u32,
    /// The timer pins' clock (TIMER_PIN_HZ unless set for research).
    pub pin_hz: u32,
    /// The debugger's breakpoints (code addresses): `run_cycles` stops
    /// before running one and sets `hit`.
    pub breakpoints: Vec<u16>,
    pub hit: Option<u16>,
    /// Run the next instruction even if it is a breakpoint (continuing from
    /// one).
    pub resume: bool,
}

impl Machine {
    /// Powers on with this base ROM (and no cartridge). The EEPROM holds a
    /// factory calibration (calibration.rs) until `set_eeprom` replaces it.
    pub fn new(bios: Vec<u8>) -> Self {
        let mut bus = Bus::new(bios);
        bus.eeprom = eeprom::Eeprom::new(calibration::factory_eeprom());
        Machine { cpu: Mcs51::new(), bus, pin_phase: 0, hz: MACHINE_HZ, pin_hz: TIMER_PIN_HZ, breakpoints: Vec::new(), hit: None, resume: false }
    }

    /// Loads a saved EEPROM (128 bytes; calibration and settings). One that
    /// fails the kernel's check is replaced by the factory calibration, as
    /// the pen would otherwise land in the wrong places, and so is Ribbit's
    /// own earlier factory image (calibration.rs).
    pub fn set_eeprom(&mut self, data: Vec<u8>) {
        let usable = calibration::is_calibrated(&data) && !calibration::is_earlier_factory_image(&data);
        let data = if usable { data } else { calibration::factory_eeprom() };
        self.bus.eeprom = eeprom::Eeprom::new(data);
    }

    /// The EEPROM's contents, and whether they changed since last asked.
    pub fn take_eeprom(&mut self) -> Option<Vec<u8>> {
        std::mem::take(&mut self.bus.eeprom.dirty).then(|| self.bus.eeprom.data.clone())
    }

    /// Puts the pen down on a spot of the open book, or lifts it (None).
    pub fn set_pen(&mut self, spot: Option<book::Spot>) {
        self.bus.pen.input.position = spot.map(book::pen_ratios);
    }

    /// Text the firmware printed on its debug port since last asked.
    pub fn take_log(&mut self) -> String {
        String::from_utf8_lossy(&std::mem::take(&mut self.bus.uart)).into_owned()
    }

    /// Puts a cartridge in the slot. The kernel looks for it at power-on.
    pub fn insert_cartridge(&mut self, cart: Vec<u8>) {
        self.bus.cart = Some(cart);
    }

    /// Takes the cartridge out.
    pub fn remove_cartridge(&mut self) {
        self.bus.cart = None;
    }

    /// The ON/OFF switch pressed. While running, the firmware is told (FC00
    /// bit 3) and goes through its own power-down; while off, the pad powers
    /// on with its memory kept, as the batteries keep it.
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

    /// Runs one instruction (or interrupt entry) and the hardware for the
    /// time it took; returns the machine cycles.
    pub fn step(&mut self) -> u32 {
        self.bus.pc = self.cpu.pc;
        let c = self.cpu.step(&mut self.bus);
        self.bus.tick(c);
        if std::mem::take(&mut self.bus.pen.irq) {
            // One falling edge on INT0 per finished conversion.
            self.cpu.set_int_pin(0, false);
            self.cpu.set_int_pin(0, true);
        }
        if std::mem::take(&mut self.bus.dsp_irq) {
            // The sound processor took a command: an edge on INT1.
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

    /// Runs for at least `cycles` machine cycles, or until a breakpoint
    /// (`hit` is then set).
    pub fn run_cycles(&mut self, cycles: u32) {
        let mut done = 0;
        while done < cycles {
            if !self.breakpoints.is_empty() && !std::mem::take(&mut self.resume) && self.breakpoints.contains(&self.cpu.pc) {
                self.hit = Some(self.cpu.pc);
                return;
            }
            self.resume = false;
            done += self.step();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cartridge with the RIB's checksum fields over `len` bytes of data.
    fn cart(len: u32) -> Vec<u8> {
        let mut c: Vec<u8> = (0..0x20_0000u32).map(|i| (i * 7 + i / 300) as u8).collect();
        for (o, v) in [(0x1014C, bus::CART_BASE), (0x10150, bus::CART_BASE + 0x1F_FFFF), (0x10154, bus::CART_BASE + len - 1)] {
            c[o..o + 4].copy_from_slice(&v.to_be_bytes());
        }
        let sum = (0..len).step_by(997).fold(0u16, |s, i| s.wrapping_add(c[i as usize] as u16));
        c[len as usize..len as usize + 2].copy_from_slice(&sum.to_be_bytes());
        c
    }

    #[test]
    fn a_damaged_sampled_byte_is_caught_and_the_sum_repaired() {
        let mut c = cart(0x1F_0000);
        assert_eq!(checksum(&c).map(|s| s.calc == s.stored), Some(true));
        assert_eq!(repair_checksum(&mut c), None, "a good cartridge is left as it is");
        c[997 * 5] ^= 0x40;
        let bad = checksum(&c).unwrap();
        assert_ne!(bad.calc, bad.stored);
        assert_eq!(repair_checksum(&mut c), Some(bad));
        assert_eq!(checksum(&c).map(|s| s.calc == s.stored), Some(true));
    }

    #[test]
    fn a_dump_that_lost_an_address_line_is_found() {
        let good: Vec<u8> = (0..0x2_0000u32).map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8).collect();
        assert_eq!(lost_address_line(&good), None);
        let bad: Vec<u8> = (0..good.len()).map(|i| good[i & !0x100]).collect();
        assert_eq!(lost_address_line(&bad), Some(8));
    }

    #[test]
    fn cartridges_without_the_fields_have_no_sum() {
        let mut c = cart(0x1F_0000);
        c[0x1014C..0x10158].fill(0x31);
        assert_eq!(checksum(&c), None);
        assert_eq!(repair_checksum(&mut c), None);
    }
}
