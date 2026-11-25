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
pub mod idle;
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

/// Which pad a base ROM belongs to. They share the FS80A363 chip, its
/// banking, sound and cartridge check (docs/leappad-family.md); they differ
/// in how the child picks a spot and a page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum Model {
    /// LeapPad (1999-2004 bases: ILA2 Universal, Canada Full Base): pen,
    /// two-page spreads, GO spots.
    #[default]
    LeapPad,
    /// My First LeapPad (2002, 2004): pen, one landscape page, GO spots,
    /// and a printed template for the built-in activity.
    MyFirst,
    /// Little Touch LeapPad (2005): no pen; a 5 x 12 matrix of switches
    /// under the two pages and 10 magnet switches that read which page is
    /// open.
    LittleTouch,
    /// LeapPad Plus Microphone (2004): a LeapPad with a recorder (Sunplus
    /// PA7790) for the child's voice.
    PlusMic,
}

impl Model {
    /// The pad a base ROM is for, from what its kernel says about itself
    /// ("MFLP US Base ROM - 2004", "MIB - LeapPad Plus Microphone baseROM",
    /// the Little Touch's "MagnetSwitch"). VERIFIED on the 7 dumped bases.
    pub fn of_base(bios: &[u8]) -> Model {
        let has = |s: &[u8]| bios.windows(s.len()).any(|w| w == s);
        if has(b"MagnetSwitch") {
            Model::LittleTouch
        } else if has(b"MFLP") {
            Model::MyFirst
        } else if has(b"Plus Microphone") || has(b"Internal Recorder") {
            Model::PlusMic
        } else {
            Model::LeapPad
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Model::LeapPad => "LeapPad",
            Model::MyFirst => "My First LeapPad",
            Model::LittleTouch => "Little Touch LeapPad",
            Model::PlusMic => "LeapPad Plus Microphone",
        }
    }

    /// A page's size in the app's coordinates (what it logs for a touch).
    /// LeapPad: from its calibration points; My First LeapPad: one page,
    /// -180..8580 by -180..5976 eighths (calibration::my_first), and only the
    /// left page is used; Little Touch: 6 x 5 switches 100 units apart (the
    /// right page's start 26 units further in, `book::key_at`).
    pub fn page_size(self) -> (f32, f32) {
        match self {
            Model::MyFirst => (1072.0, 747.0),
            Model::LittleTouch => (626.0, 500.0),
            _ => (book::PAGE_W, book::PAGE_H),
        }
    }

    /// Whether a spread has two pages (all but My First LeapPad).
    pub fn two_pages(self) -> bool {
        self != Model::MyFirst
    }

    /// The pad a cartridge is for (its app header, `layout::family`): a
    /// LeapPad cartridge also plays on the Plus Microphone.
    pub fn of_cartridge(cart: &[u8]) -> Model {
        layout::family(cart).model()
    }

    /// Whether a cartridge for `cart` plays on this pad: the LeapPad and the
    /// Plus Microphone take each other's books; My First LeapPad and Little
    /// Touch books need their own pads (a My First LeapPad answers others
    /// "Whoops! That's not a My First LeapPad cartridge").
    pub fn plays(self, cart: Model) -> bool {
        match self {
            Model::LeapPad | Model::PlusMic => matches!(cart, Model::LeapPad | Model::PlusMic),
            m => m == cart,
        }
    }

    /// Every model, in the order the app lists them.
    pub const ALL: [Model; 4] = [Model::LeapPad, Model::PlusMic, Model::MyFirst, Model::LittleTouch];

    /// Whether the child uses a pen (all but the Little Touch).
    pub fn has_pen(self) -> bool {
        self != Model::LittleTouch
    }
}

/// The 14-byte boot stub every LeapPad base ROM and cartridge starts with
/// (docs/leappad.md, "Boot stub"; VERIFIED on 3 base ROMs and 19 of the 20
/// cartridges in the user's set; the Mind Station cartridge lacks it).
const BOOT_STUB: [u8; 14] = [0x02, 0x00, 0x03, 0x90, 0xFC, 0x06, 0xE0, 0xF8, 0xA3, 0xE0, 0xF9, 0x90, 0xFC, 0x08];

/// Whether `data` is a LeapPad base ROM or cartridge. Base ROMs hold the
/// cartridge check ("Cart found:"); cartridges, which bring their own
/// kernel, do not.
///
/// Other handhelds on the same chip start with the stub too and are no
/// LeapPad images: the iQuest's and Turbo Twist Extreme's base ROMs (their
/// names in the header at 0x38; ribbit_iquest), and the older Turbo Twist
/// models' (`is_turbo_twist`). Taken for LeapPad images, the iQuest's base
/// opened as a LeapPad and the Turbo Twists were listed as books.
pub fn identify(data: &[u8]) -> Option<Image> {
    let data = data.get(lbk_header(data)..)?;
    if data.len() < 0x2_0000 || data[..BOOT_STUB.len()] != BOOT_STUB {
        return None;
    }
    let head = &data[0x30..0x80];
    let named = |s: &[u8]| head.windows(s.len()).any(|w| w == s);
    if named(b"IQuest FullBase") || named(b"TTExtreme FullBase") || is_turbo_twist(data) {
        return None;
    }
    let base = data.windows(11).any(|w| w == b"Cart found:");
    Some(if base { Image::BaseRom } else { Image::Cartridge })
}

/// An older Turbo Twist's base ROM (Spelling, Fact Blaster: MAME's
/// `ttwistsp`, `ttwistfb`; 1-2 MB, a one-line display; not emulated): the
/// LeapPad's boot stub, and FFFFh at 0x10102, where every LeapPad dump (782
/// checked) has its ROM Info Block's second word.
pub fn is_turbo_twist(data: &[u8]) -> bool {
    data.len() >= 0x2_0000 && data[..BOOT_STUB.len()] == BOOT_STUB && data.get(0x10102..0x10104) == Some(&[0xFF, 0xFF][..])
}

/// An LBK ("Leap Back-Up") file, what the Mind Station software writes when
/// it dumps a cartridge, is the ROM after a 0x13C-byte header (community
/// tools strip it the same way; no LBK file in the user's set, so the
/// header's own fields are UNKNOWN). Returns the header's length: 0x13C when
/// the boot stub sits there and not at 0, else 0.
fn lbk_header(data: &[u8]) -> usize {
    const LBK: usize = 0x13C;
    let stub_at = |o: usize| data.get(o..o + BOOT_STUB.len()) == Some(&BOOT_STUB[..]);
    if !stub_at(0) && stub_at(LBK) { LBK } else { 0 }
}

/// Turns an LBK file into the plain ROM: the header goes, and so does
/// anything past 4 MB, the most a LeapPad cartridge holds (LBK files of
/// bigger images come in chunks, each after its own header). Plain ROMs are
/// left alone.
pub fn unwrap_lbk(data: &mut Vec<u8>) {
    let header = lbk_header(data);
    if header > 0 {
        data.drain(..header);
        data.truncate(0x40_0000);
    }
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
    /// The pad the base ROM is for.
    pub model: Model,
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
    /// Keep the pad on however long it is left alone: the firmware's idle
    /// power-off (about 3 minutes) is told of activity once a second
    /// (idle.rs). Off (the default), it powers down as a real pad does.
    pub keep_awake: bool,
    /// Cycles run since `keep_awake` last acted.
    since_awake: u64,
}

impl Machine {
    /// Powers on with this base ROM (and no cartridge). The EEPROM holds a
    /// factory calibration (calibration.rs) until `set_eeprom` replaces it.
    pub fn new(bios: Vec<u8>) -> Self {
        let model = Model::of_base(&bios);
        let mut bus = Bus::new(bios);
        bus.eeprom = eeprom::Eeprom::new(Self::factory_eeprom(model));
        Machine { model, cpu: Mcs51::new(), bus, pin_phase: 0, hz: MACHINE_HZ, pin_hz: TIMER_PIN_HZ, breakpoints: Vec::new(), hit: None, resume: false, keep_awake: false, since_awake: 0 }
    }

    /// Loads a saved EEPROM (128 bytes; calibration and settings). One that
    /// fails the kernel's check is replaced by the factory calibration, as
    /// the pen would otherwise land in the wrong places, and so is Ribbit's
    /// own earlier factory image (calibration.rs).
    pub fn set_eeprom(&mut self, data: Vec<u8>) {
        let usable = match self.model {
            Model::MyFirst => calibration::my_first::is_calibrated(&data),
            _ => calibration::is_calibrated(&data) && !calibration::is_earlier_factory_image(&data),
        };
        let data = if usable { data } else { Self::factory_eeprom(self.model) };
        self.bus.eeprom = eeprom::Eeprom::new(data);
    }

    /// The factory calibration for a model's pen (the Little Touch has no
    /// EEPROM in use; it gets the LeapPad's, which it never reads).
    pub fn factory_eeprom(model: Model) -> Vec<u8> {
        match model {
            Model::MyFirst => calibration::my_first::factory_eeprom(),
            _ => calibration::factory_eeprom(),
        }
    }

    /// Little Touch LeapPad: a hand on a spot of the open book (or lifted).
    /// The switch under it is pressed (`book::key_at`).
    pub fn set_touch(&mut self, spot: Option<book::Spot>) {
        self.bus.key = spot.and_then(book::key_at);
    }

    /// Little Touch LeapPad: the open page's magnets (`layout::Spread::magnets`).
    pub fn set_magnets(&mut self, magnets: u16) {
        self.bus.magnets = magnets;
    }

    /// The EEPROM's contents, and whether they changed since last asked.
    pub fn take_eeprom(&mut self) -> Option<Vec<u8>> {
        std::mem::take(&mut self.bus.eeprom.dirty).then(|| self.bus.eeprom.data.clone())
    }

    /// Puts the pen down on a spot of the open book, or lifts it (None).
    pub fn set_pen(&mut self, spot: Option<book::Spot>) {
        if self.model == Model::LittleTouch {
            return self.set_touch(spot);
        }
        let model = self.model;
        self.bus.pen.input.position = spot.map(|s| book::pen_ratios_for(model, s));
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
        // Counted here: the processor's own count restarts at power-on.
        self.since_awake += done as u64;
        if self.keep_awake && self.since_awake >= self.hz as u64 {
            self.since_awake = 0;
            self.tell_activity();
        }
    }

    /// Sets the running firmware's activity bit (idle.rs), as a touch does,
    /// so that its idle power-off moves its deadline on.
    fn tell_activity(&mut self) {
        if self.is_off() {
            return;
        }
        let code: Vec<u8> = (0..=0xFFFFu16).map(|a| self.bus.code_peek(a)).collect();
        if let Some(bit) = idle::activity_bit(&code) {
            self.cpu.iram[0x20 + (bit >> 3) as usize] |= 1 << (bit & 7);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn other_handhelds_on_the_chip_are_no_leappad_images() {
        let mut rom = vec![0u8; 0x2_0000];
        rom[..BOOT_STUB.len()].copy_from_slice(&BOOT_STUB);
        rom[0x10102..0x10104].copy_from_slice(&[0xFE, 0xFF]);
        assert_eq!(identify(&rom), Some(Image::Cartridge));
        let mut iquest = rom.clone();
        iquest[0x3A..0x3A + 30].copy_from_slice(b"ToolPad V2.3.x IQuest FullBase");
        assert_eq!(identify(&iquest), None);
        let mut twist = rom.clone();
        twist[0x10102..0x10104].copy_from_slice(&[0xFF, 0xFF]);
        assert!(is_turbo_twist(&twist) && !is_turbo_twist(&rom));
        assert_eq!(identify(&twist), None);
    }

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
    fn an_lbk_file_is_its_rom_after_the_header() {
        let mut rom = cart(0x1F_0000);
        rom[..BOOT_STUB.len()].copy_from_slice(&BOOT_STUB);
        let mut lbk = vec![0x4C; 0x13C];
        lbk.extend_from_slice(&rom);
        assert_eq!(identify(&lbk), Some(Image::Cartridge));
        unwrap_lbk(&mut lbk);
        assert_eq!(lbk, rom);
        // A plain ROM is left alone.
        unwrap_lbk(&mut lbk);
        assert_eq!(lbk, rom);
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
