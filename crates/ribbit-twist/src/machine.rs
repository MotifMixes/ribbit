//! The RockIt Twist as a machine: the ARM9 model with the console's memory
//! and devices around it, built up one device at a time as its firmware
//! asks for them. NOT a working emulator yet: see docs/rockit-twist.md,
//! "Bringing up the machine", for what each device is based on.
//!
//! Memory map (EVIDENCE from the firmware's accesses; sizes ASSUMED):
//!
//! | Address | What |
//! |---|---|
//! | 0x00000000 | SDRAM (64 MB assumed, as the Zippity's) |
//! | 0xC0000000-0xDFFFFFFF | I/O registers |
//! | 0xF8000000 | on-chip RAM (64 KB assumed) |
//! | 0xFF000000 | cache controller (writes ignored) |
//!
//! Devices modelled so far, each from the firmware's own use of it:
//!
//! * Clock: 0xD000003C reads non-zero ("ready") — the base firmware waits
//!   for it after programming the clocks (0x3FB8). ASSUMED meaning.
//! * Timers at 0xC0020000 + 0x20 n: +0x10 is the current count, a 16-bit
//!   up-counter (the firmware's time base reads timer 5's, 0x6124).
//!   Rate ASSUMED (`TIMER_HZ`).
//! * UART at 0xC0060000: bytes written to +0 are kept as text.
//!
//! Every other register remembers what was written and reads it back; the
//! machine counts reads of such registers (`unmodelled`), which is the list
//! of what to model next.

use crate::sd::{Card, Response};
use ribbit_arm::{Arm, ArmBus};
use std::collections::{BTreeMap, HashMap};

pub const RAM_SIZE: usize = 64 << 20;
pub const IRAM_BASE: u32 = 0xF800_0000;
pub const IRAM_SIZE: usize = 0x1_0000;
/// Instructions per emulated second: a stand-in until timing is measured
/// (MAME's GPL32612 driver gives this chip family's core 240 MHz).
pub const CLOCK: u64 = 240_000_000;
/// The timers' count rate (ASSUMED: 1 MHz).
pub const TIMER_HZ: u64 = 1_000_000;
const IO_START: u32 = 0xC000_0000;
const IO_END: u32 = 0xE000_0000;

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Bus {
    #[serde(with = "rle")]
    pub ram: Vec<u8>,
    pub iram: Vec<u8>,
    /// Last value written to each I/O word.
    io: HashMap<u32, u32, IoHash>,
    /// Reads of I/O words nothing models: (count, first pc).
    #[serde(skip)]
    pub unmodelled: BTreeMap<u32, (u64, u32)>,
    /// Research: the MP3 accelerator's jobs (0xD0A00000; docs/rockit-twist.md
    /// "MP3"), when they started: (time, kind 'A' or 'B', registers +0..+0x4C,
    /// and for the first jobs the memory at each address register).
    #[serde(skip)]
    pub mp3_jobs: Vec<Mp3Job>,
    #[serde(skip)]
    pub mp3_log: bool,
    /// Research: keep job memory only from this time (cycles).
    #[serde(skip)]
    pub mp3_log_from: u64,
    /// Text written to the UART.
    #[serde(skip)]
    pub uart: Vec<u8>,
    /// Instructions run so far (the machine's clock).
    pub now: u64,
    /// Research: accesses to I/O words in this range (inclusive) are kept
    /// in `watched` (at most `WATCH_MAX`).
    #[serde(skip)]
    pub watch: Option<(u32, u32)>,
    #[serde(skip)]
    pub watched: Vec<String>,
    /// Watch writes only, and never these registers.
    #[serde(skip)]
    pub watch_writes_only: bool,
    #[serde(skip)]
    pub watch_skip: Vec<u32>,
    /// Watch from this time on (cycles).
    #[serde(skip)]
    pub watch_from: u64,
    /// The SD hosts' status registers (+0x14).
    sd_status: [u32; 2],
    /// Response words not yet read (+0x10), per host.
    sd_response: [std::collections::VecDeque<u32>; 2],
    /// The eMMC chip behind the first host.
    pub emmc: Option<Card>,
    /// Timers 0-3: when each next overflows (0: stopped), and which have.
    timer_due: [u64; 4],
    timer_flags: u32,
    /// DMA channels finished (0xD03001FC, write 1s to clear).
    dma_status: u32,
    /// An interrupt input may have changed.
    pub lines_dirty: bool,
    /// DMA transfers to the DAC run at its sample rate: per channel, the
    /// next source address and the units left (None: idle).
    dma_paced: [Option<(u32, u32)>; 4],
    dma_paced_last: u64,
    /// DAC FIFO levels and when each last drained a sample.
    dac_level: [u32; 2],
    dac_last: [u64; 2],
    /// Samples written to the DAC (both channels interleaved as written).
    #[serde(skip)]
    pub audio: Vec<i16>,
    /// The DAC's latest sample (left, right), the one before it, and when
    /// the latest came; heard until `dac_hold_until` (`sound::tick_sound`
    /// adds it to the sound unit's output).
    pub(crate) dac_hold: (i16, i16),
    #[serde(default)]
    pub(crate) dac_prev: (i16, i16),
    #[serde(default)]
    pub(crate) dac_at: u64,
    pub(crate) dac_hold_until: u64,
    /// The sound unit (`sound`): its channels, stop flags (+0xE2C, +0xEAC),
    /// beat counter, and its output, stereo at `sound::OUTPUT_RATE`, made
    /// up to `sound_made` samples since power-on.
    pub(crate) voices: [crate::sound::Voice; 32],
    pub(crate) spu_stopped: [u32; 2],
    pub(crate) beat_base: u32,
    pub(crate) beat_flag: bool,
    pub(crate) sound_made: u64,
    /// When the next sample is due (cycles; 0: work it out).
    #[serde(skip)]
    pub(crate) sound_next: u64,
    /// Stopped channels' output, fading (`sound::spu_release`).
    #[serde(skip)]
    pub(crate) spu_tail: (f32, f32),
    /// The USB device controller (`udc`).
    #[serde(skip)]
    pub udc: crate::udc::Udc,
    #[serde(skip)]
    pub sound: Vec<i16>,
    /// The buttons held, as `key` bits.
    pub keys: u32,
    /// The rotary controls (`encoder`), in `input::Rotary` order: steps
    /// still to send (signed), and where the pulse for the current one is.
    turns_pending: [i32; 4],
    turn_phase: [u8; 4],
    /// Direction pins as last set, and phase A pins held low.
    turn_dir: [bool; 4],
    turn_low: [bool; 4],
    /// When the encoders next move on, and whose turn it is.
    turn_due: u64,
    turn_slot: usize,
    /// The Slider's four touch pads, bit n pad n (from the top: ASSUMED).
    pub slider_pads: u8,
    /// Research: writes into this RAM range are kept in `ram_writes`
    /// (time, pc, address, value).
    #[serde(skip)]
    pub ram_watch: Option<(u32, u32)>,
    #[serde(skip)]
    pub ram_writes: Vec<(u64, u32, u32, u32)>,
    /// Reads of a timer's count close together (a program waiting on the
    /// clock), when the last was, and time to skip at the next step.
    poll_streak: u32,
    last_poll: u64,
    pub poll_skip: u64,
    /// A USB cable is plugged in.
    pub usb_plugged: bool,
    /// What each ADC channel measures (12-bit).
    /// * Channels 1 and 2: the board's identity, from resistor dividers.
    ///   VERIFIED from the bootloader (0xF02CF4 and its tables at 0xF052CC):
    ///   channel 1 in 0x56-0x1A0 picks the second table, in which channel 2
    ///   in 503-832 is product 6, "RockIt Twist LF" (the factory test's
    ///   names); 1002-1341 would be 7, "RockIt Twist VT". Other values are
    ///   other PocketPlay products (LeapStart, Globe), for which the
    ///   bootloader takes another path. Here: the middle of each range.
    /// * Channel 5: the battery. The bootloader powers off below 0x8B0
    ///   (0xF05044). ASSUMED: 0xA00, about 4.1 V if the cell is halved into
    ///   a 3.3 V reference.
    pub adc: [u32; 8],
    /// When the timebase next ticks (0: stopped).
    timebase_due: u64,
    /// When the panel next pulses its tearing-effect line.
    te_due: u64,
    /// The panel's picture: the framebuffer last streamed to it (address),
    /// and how many frames have been sent.
    pub panel_source: Option<u32>,
    pub panel_frames: u64,
    /// The video chip's last frame (kept to reuse its memory), and how many
    /// it has drawn.
    #[serde(skip)]
    frame: Vec<u16>,
    pub frames_drawn: u64,
    /// Research: leave the video chip's frames undrawn (to time the rest).
    #[serde(skip)]
    pub skip_drawing: bool,
    /// Research: the drawing modes met (`video`), each with when first
    /// seen: text planes as (register base, control, attributes, the
    /// rotation/zoom words), sprites as (attributes, halfword 4's top
    /// bits, extended attributes) with bits that `video` does not decode.
    #[serde(skip)]
    pub modes_seen: std::cell::RefCell<std::collections::BTreeMap<String, u64>>,
    /// The panel controller's registers, as set through its command port.
    pub panel: Panel,
    /// The geometry the last frame was streamed with.
    panel_shown: Panel,
    /// 2D engine jobs (research log): (time, +0xC04, +0xC08, +0xC0C, and
    /// the registers 0xD0C00000-0xD0C00FFF written since the last job).
    #[serde(skip)]
    pub gfx_log: Vec<(u64, u32, u32, u32, Vec<(u32, u32)>)>,
    /// The video decoder (`mpeg4`) and the pictures it has made, by the
    /// address their RGB copy went to (the next frame's reference names
    /// it), newest last.
    mpeg: crate::mpeg4::Decoder,
    mpeg_pictures: Vec<(u32, crate::mpeg4::Picture)>,
    /// Research: frames decoded, and ones that failed.
    #[serde(skip)]
    pub mpeg_frames: (u64, u64),
    #[serde(skip)]
    gfx_regs: Vec<(u32, u32)>,
    /// DMA transfers (research log): (time, channel, control, source,
    /// destination, bytes).
    #[serde(skip)]
    pub dma_log: Vec<(u64, u32, u32, u32, u32, u32)>,
    pc: u32,
}

/// Interrupt lines (as numbered by the controller): timers 0-3 (the
/// Zippity's; the Twist firmware handles 25 only so far) and DMA.
const IRQ_TIMER: [u32; 4] = [25, 17, 10, 9];
const IRQ_DMA: u32 = 24;
/// The 2D engine at 0xD0C00000 (its driver registers line 26 at 0x5708C).
const IRQ_GFX: u32 = 26;
/// The timebase at 0xC0040000 (handler 0x44AC).
const IRQ_TIMEBASE: u32 = 1;
/// External interrupt pins (0xD0100020): status bits 6-7 raise line 6,
/// bits 11, 13 and 15 line 7 (dispatchers 0x23884 and 0x23040); writing 1s
/// clears them.
const IRQ_EXT_A: u32 = 6;
const IRQ_EXT_B: u32 = 7;
/// The video chip's frame interrupt (0xD050018C status, 0x188 enable): line
/// 27, as on the Zippity's chip of the same family; the menu unmasks it
/// (mask 0x79FFFE59 once it runs) and waits for its handler (0xBFA0).
const IRQ_VIDEO: u32 = 27;
/// The ADC's conversion-done interrupt (0xC00C0004 bits 14 enable, 15
/// pending): the System's battery monitor registers line 29 (0x22B4C,
/// handler 0x229B8).
const IRQ_ADC: u32 = 29;
const EXT_STATUS: u32 = 0x40 | 0x80 | 0x800 | 0x2000 | 0x8000;
/// The panel's tearing-effect pulses per second (ASSUMED: a 60 Hz panel).
/// The firmware turns the signal on (panel command 0x35) and takes it on
/// external interrupt 0 (enable bit 0, status bit 6): each pulse runs its
/// frame handler (0x23B3C), which starts the next transfer to the panel.
pub const TE_HZ: u64 = 60;

/// The rotary controls and the Slider, as the System reads them (VERIFIED
/// from its code; colours from the factory test's I/O test, which names
/// each counter: 0xA85C Spinner, 0xA888 Yellow, 0xA8B4 Green, 0xA8E0 Red).
/// The Spinner, Yellow and Green dials are quadrature encoders: phase A on
/// an external interrupt pin, whose handler (0x22F4C, 0x22F90, 0x22FD4)
/// counts when A falls, up if the direction pin is high, down if low. The
/// Red dial's handler (0x23018, external interrupt 1) counts on every
/// interrupt, by its direction pin. The Slider (0x23560) is four touch pads
/// read low when touched.
pub mod encoder {
    /// One rotary control: phase A (port, bit), its external interrupt
    /// status bit (0xD0100020), and the direction pin (port, bit).
    pub struct Encoder {
        pub phase_a: Option<(u32, u32)>,
        pub status: u32,
        pub dir_port: u32,
        pub dir_bit: u32,
    }

    /// In `input::Rotary` order: Spinner, Yellow, Green, Red.
    pub const ALL: [Encoder; 4] = [
        Encoder { phase_a: Some((0xC000_0110, 1)), status: 0x2000, dir_port: 0xC000_0020, dir_bit: 0 },
        Encoder { phase_a: Some((0xC000_0110, 0)), status: 0x800, dir_port: 0xC000_0040, dir_bit: 11 },
        Encoder { phase_a: Some((0xC000_0110, 2)), status: 0x8000, dir_port: 0xC000_0080, dir_bit: 2 },
        Encoder { phase_a: None, status: 0x80, dir_port: 0xC000_0020, dir_bit: 7 },
    ];

    /// The time between pulse edges (0.5 ms: ASSUMED, a quick hand).
    pub const EDGE: u64 = super::CLOCK / 2000;

    /// The Slider's pads (port, bit), in the order the System packs them
    /// (bits 0-3 of its value, 0x23560). Which end pad 0 is is UNVERIFIED.
    pub const SLIDER_PADS: [(u32, u32); 4] = [(0xC000_0040, 2), (0xC000_0040, 1), (0xC000_0040, 4), (0xC000_00E0, 8)];

    /// The pads under a finger at `pos` (0 = top, 1 = bottom), or none.
    pub fn pads(pos: Option<f32>) -> u8 {
        pos.map_or(0, |p| 1 << ((p.clamp(0.0, 0.999) * 4.0) as u8))
    }
}

/// The buttons, as the firmware's own key-scan routine reports them
/// (VERIFIED: the same routine in the System, 0x23380, and the factory
/// test, 0xA62C; the factory test's I/O test names each bit, table at
/// 0x4CE7C). The Dials, Spinner and Slider are read elsewhere.
pub mod key {
    pub const POWER: u32 = 0x1;
    pub const ENTER: u32 = 0x2;
    pub const HELP: u32 = 0x4;
    pub const HOME: u32 = 0x8;
    pub const VOL_UP: u32 = 0x10;
    pub const VOL_DOWN: u32 = 0x20;
    pub const UP: u32 = 0x40;
    pub const DOWN: u32 = 0x80;
    pub const LEFT: u32 = 0x100;
    pub const RIGHT: u32 = 0x200;
    pub const SWITCH_LEFT: u32 = 0x8_0000;
    pub const SWITCH_RIGHT: u32 = 0x10_0000;

    /// "MATRIX 1"-"MATRIX 9" (n = 0-8): row n / 3 (row bit 15 - n / 3),
    /// column n % 3 (column bit 12 - n % 3). Which corner MATRIX 1 is on
    /// the console is UNVERIFIED (taken as the top left, row by row).
    pub const fn grid(n: usize) -> u32 {
        0x400 << n
    }

    /// A control's bit.
    pub fn of(b: crate::input::Button) -> u32 {
        use crate::input::Button as B;
        match b {
            B::DpadUp => UP,
            B::DpadDown => DOWN,
            B::DpadLeft => LEFT,
            B::DpadRight => RIGHT,
            B::Hint => HELP,
            B::Home => HOME,
            B::Check => ENTER,
            B::VolumeUp => VOL_UP,
            B::VolumeDown => VOL_DOWN,
            B::Power => POWER,
            B::SwitchLeft => SWITCH_LEFT,
            B::SwitchRight => SWITCH_RIGHT,
            B::Grid(n) => grid(n as usize),
        }
    }

    /// The pins that read low while a button is held: (port, bit, key).
    pub const PINS: &[(u32, u32, u32)] = &[
        (0xC000_0000, 9, SWITCH_LEFT),
        (0xC000_0000, 8, SWITCH_RIGHT),
        (0xC000_0040, 9, HOME),
        (0xC000_0040, 8, HELP),
        (0xC000_00C0, 14, POWER),
        (0xC000_00C0, 6, VOL_UP),
        (0xC000_00C0, 5, VOL_DOWN),
        (0xC000_00E0, 0, ENTER),
        (0xC000_0180, 3, RIGHT),
        (0xC000_0180, 2, LEFT),
        (0xC000_0180, 1, DOWN),
        (0xC000_0180, 0, UP),
    ];
}

/// The LCD controller's registers that decide where a streamed picture
/// lands (ILI9341/ST7789 commands, VERIFIED as sent by the firmware):
/// 0x36 memory access control (bit 7 row order, 6 column order, 5
/// row/column exchange), 0x2A column window, 0x2B page window. The System
/// sends 36 40 with a 240 x 320 window; the factory test 36 20 with
/// 320 x 240.
///
/// The panel's memory is 240 columns by 320 rows. It is mounted so that
/// the player sees memory row r as screen column r and memory column c as
/// screen row c (VERIFIED: with this, both firmwares' pictures come out
/// upright and unmirrored).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Panel {
    pub madctl: u8,
    pub columns: (u16, u16),
    pub pages: (u16, u16),
    /// The command whose data bytes are arriving, and how many have.
    command: u8,
    got: usize,
    /// Display on (command 0x29; 0x28 display off and 0x10 sleep turn it
    /// off): until then the panel shows nothing.
    #[serde(default)]
    pub on: bool,
}

impl Default for Panel {
    fn default() -> Self {
        Self { madctl: 0, columns: (0, 239), pages: (0, 319), command: 0, got: 0, on: false }
    }
}

impl Panel {
    pub fn command(&mut self, c: u8) {
        self.command = c;
        self.got = 0;
        match c {
            0x29 => self.on = true,
            0x28 | 0x10 => self.on = false,
            _ => {}
        }
    }

    pub fn data(&mut self, d: u8) {
        let set = |pair: &mut (u16, u16), n: usize| match n {
            0 => pair.0 = (pair.0 & 0xFF) | (d as u16) << 8,
            1 => pair.0 = (pair.0 & 0xFF00) | d as u16,
            2 => pair.1 = (pair.1 & 0xFF) | (d as u16) << 8,
            3 => pair.1 = (pair.1 & 0xFF00) | d as u16,
            _ => {}
        };
        match self.command {
            0x36 if self.got == 0 => self.madctl = d,
            0x2A => set(&mut self.columns, self.got),
            0x2B => set(&mut self.pages, self.got),
            _ => {}
        }
        self.got += 1;
    }

    /// Which pixel of the streamed frame (row by row through the window)
    /// the player sees at screen (`x`, `y`), if one lands there.
    pub fn stored_index(&self, x: usize, y: usize) -> Option<usize> {
        let exchange = self.madctl & 0x20 != 0;
        // Screen to panel memory, then to the controller's addresses.
        let (mem_col, mem_row) = (y, x);
        let (mut c, mut p) = if exchange { (mem_row, mem_col) } else { (mem_col, mem_row) };
        let (cols, pages) = if exchange { (320, 240) } else { (240, 320) };
        if self.madctl & 0x40 != 0 {
            c = cols - 1 - c;
        }
        if self.madctl & 0x80 != 0 {
            p = pages - 1 - p;
        }
        let (c0, c1) = (self.columns.0 as usize, self.columns.1 as usize);
        let (p0, p1) = (self.pages.0 as usize, self.pages.1 as usize);
        ((c0..=c1).contains(&c) && (p0..=p1).contains(&p)).then(|| (p - p0) * (c1 - c0 + 1) + (c - c0))
    }
}
/// Its tick rate with rate select 6, the one the firmware uses (ASSUMED:
/// a 32768 Hz clock divided by 32; the firmware counts these ticks as its
/// time for time-outs, 0x5014).
pub const TIMEBASE_HZ: u64 = 1024;
/// The audio DAC's two channels (0xC00D0000 + 0x20 c: +0 control, +4
/// sample, +8 status), lines 31 and 30 (handlers 0xF20 and 0xE54).
const IRQ_DAC: [u32; 2] = [31, 30];
/// DAC samples per second: the start-up logo's sound is 44.1 kHz (its
/// AVI says so) and goes to the DAC as one 32-bit stereo pair per sample.
/// The firmware's own rate register is not decoded yet. FIFO depth: the
/// handlers top it up to 16.
pub const DAC_HZ: u64 = 44_100;
const DAC_FIFO: u32 = 16;
/// Timers 0-3 count at this rate (ASSUMED: gives timer 0's period of 3750
/// counts a 100 Hz operating-system tick, uC/OS-II's usual rate).
pub const TIMER0_HZ: u64 = 375_000;

const WATCH_MAX: usize = 200_000;

impl Bus {
    fn new() -> Self {
        Self {
            ram: vec![0; RAM_SIZE],
            iram: vec![0; IRAM_SIZE],
            io: HashMap::default(),
            unmodelled: BTreeMap::new(),
            mp3_jobs: Vec::new(),
            mp3_log: false,
            mp3_log_from: 0,
            uart: Vec::new(),
            now: 0,
            watch: None,
            watched: Vec::new(),
            watch_writes_only: false,
            watch_skip: Vec::new(),
            watch_from: 0,
            sd_status: [0; 2],
            sd_response: Default::default(),
            emmc: None,
            timer_due: [0; 4],
            timer_flags: 0,
            dma_status: 0,
            lines_dirty: true,
            dma_log: Vec::new(),
            dma_paced: [None; 4],
            dma_paced_last: 0,
            dac_level: [0; 2],
            dac_last: [0; 2],
            audio: Vec::new(),
            dac_hold: (0, 0),
            dac_prev: (0, 0),
            dac_at: 0,
            dac_hold_until: 0,
            voices: [crate::sound::Voice::default(); 32],
            spu_stopped: [0; 2],
            beat_base: 0,
            beat_flag: false,
            sound_made: 0,
            sound_next: 0,
            spu_tail: (0.0, 0.0),
            udc: Default::default(),
            sound: Vec::new(),
            gfx_log: Vec::new(),
            mpeg: crate::mpeg4::Decoder::new(),
            mpeg_pictures: Vec::new(),
            mpeg_frames: (0, 0),
            gfx_regs: Vec::new(),
            timebase_due: 0,
            te_due: 0,
            panel_source: None,
            panel_frames: 0,
            frame: Vec::new(),
            frames_drawn: 0,
            skip_drawing: false,
            modes_seen: Default::default(),
            panel: Panel::default(),
            panel_shown: Panel::default(),
            keys: 0,
            turns_pending: [0; 4],
            turn_phase: [0; 4],
            turn_dir: [false; 4],
            turn_low: [false; 4],
            turn_due: 0,
            turn_slot: 0,
            slider_pads: 0,
            ram_watch: None,
            ram_writes: Vec::new(),
            poll_streak: 0,
            last_poll: 0,
            poll_skip: 0,
            usb_plugged: false,
            adc: [0, 0x100, 668, 0, 0, 0xA00, 0, 0],
            pc: 0,
        }
    }

    pub fn latch(&self, a: u32) -> u32 {
        self.io.get(&(a & !3)).copied().unwrap_or(0)
    }

    /// A timer's current count (16-bit, counting up since power-on).
    fn timer_count(&self) -> u32 {
        ((self.now as u128 * TIMER_HZ as u128 / CLOCK as u128) & 0xFFFF) as u32
    }

    fn io_read(&mut self, a: u32) -> u32 {
        let a = a & !3;
        if (crate::udc::BASE..crate::udc::BASE + 0x200).contains(&a)
            && let Some(v) = self.udc_read(a)
        {
            self.trace(a, None, v);
            return v;
        }
        let v = match a {
            0xD000_003C => 1,
            // GPIO inputs (+0 of ports A, B, C, D and G): the buttons are
            // active-low, so a port reads its pins high (nothing pressed)
            // except those `pressed` holds low. The pins are the ones the
            // bootloader's key scan reads (0xF003C8 once relocated).
            0xC000_0000 | 0xC000_0020 | 0xC000_0040 | 0xC000_0080 | 0xC000_00C0 | 0xC000_00E0 | 0xC000_0110
            | 0xC000_0180 => self.gpio_in(a),
            0xC002_0000..=0xC002_01FF if a & 0x1F == 0x10 => {
                self.note_clock_poll();
                self.timer_count()
            }
            // Bit 7 is a level: data waits in the receive buffer (the
            // bootloader clears the status with 0xFFFF and then waits for
            // it before each word, 0xF0093C).
            0xC00A_0014 => {
                let waiting = self.emmc.as_ref().is_some_and(|c| !c.fifo.is_empty());
                (self.sd_status[0] & !0x80) | if waiting { 0x80 } else { 0 }
            }
            0xC00B_0014 => self.sd_status[1],
            0xC00A_0010 | 0xC00B_0010 => self.sd_response_word(((a >> 16) & 1) as usize),
            0xC00A_0004 => self.sd_data(),
            0xD010_0028 => self.pending_irq(),
            0xD010_002C => 0,
            0xD030_01FC => self.dma_status,
            // The sound unit (`sound`): busy channels, stop flags, the beat
            // flag (+0xE14 bit 14).
            0xD040_0E3C | 0xD040_0EBC => self.spu_busy(((a >> 7) & 1) as usize),
            0xD040_0E2C | 0xD040_0EAC => self.spu_stopped[((a >> 7) & 1) as usize],
            0xD040_0E14 => (self.latch(a) & !0x4000) | if self.beat_flag { 0x4000 } else { 0 },
            // DAC status: bit 15 the FIFO is full.
            0xC00D_0008 | 0xC00D_0028 => {
                let c = ((a >> 5) & 1) as usize;
                if self.dac_level[c] >= DAC_FIFO { 0x8000 } else { self.dac_level[c] }
            }
            _ => {
                let pc = self.pc;
                self.unmodelled.entry(a).or_insert((0, pc)).0 += 1;
                self.latch(a)
            }
        };
        self.trace(a, None, v);
        v
    }

    fn trace(&mut self, a: u32, write: Option<u32>, read: u32) {
        if let Some((lo, hi)) = self.watch
            && (lo..=hi).contains(&a)
            && self.watched.len() < WATCH_MAX
            && (write.is_some() || !self.watch_writes_only)
            && !self.watch_skip.contains(&a)
            && self.now >= self.watch_from
        {
            let line = match write {
                Some(v) => format!("{:>11} {:08X}  W {a:08X} = {v:08X}", self.now, self.pc),
                None => format!("{:>11} {:08X}  R {a:08X} -> {read:08X}", self.now, self.pc),
            };
            self.watched.push(line);
        }
    }

    fn io_write(&mut self, a: u32, v: u32, mask: u32) {
        let w = a & !3;
        if w == 0xC006_0000 && self.uart.len() < 1 << 20 {
            self.uart.push(v as u8);
        }
        let shift = 8 * (a & 3);
        let e = self.io.entry(w).or_insert(0);
        let old = *e;
        *e = (*e & !(mask << shift)) | ((v & mask) << shift);
        let now = *e;
        self.sd_write(w, now);
        self.device_write(w, now, v << shift, old);
        self.trace(w, Some(v << shift), 0);
    }

    pub(crate) fn set_latch(&mut self, a: u32, v: u32) {
        self.io.insert(a, v);
    }

    /// Research: notes an MP3 accelerator job (`mp3_jobs`).
    fn note_mp3_job(&mut self, kind: char) {
        if !self.mp3_log || self.mp3_jobs.len() >= 20_000 || self.now < self.mp3_log_from {
            return;
        }
        let regs: [u32; 20] = std::array::from_fn(|i| self.latch(0xD0A0_0000 + 4 * i as u32));
        // The address registers hold low and high halves.
        let mut memory = Vec::new();
        if self.mp3_jobs.len() < 400 && self.now >= self.mp3_log_from {
            for pair in [(0x10, 0x14), (0x18, 0x1C), (0x20, 0x24), (0x28, 0x2C), (0x30, 0x34), (0x38, 0x3C), (0x40, 0x44), (0x48, 0x4C)] {
                let a = (regs[pair.0 / 4] & 0xFFFF) | (regs[pair.1 / 4] & 0xFFFF) << 16;
                let bytes: Vec<u8> = (0..0x1200u32).map(|i| self.peek_byte(a.wrapping_add(i))).collect();
                memory.push((a, bytes));
            }
        }
        self.mp3_jobs.push(Mp3Job { time: self.now, kind, regs, memory });
    }

    fn peek_byte(&self, a: u32) -> u8 {
        self.ram.get(a as usize).copied().unwrap_or(0)
    }

    /// Timers, DMA, the interrupt controller and the sound unit.
    fn device_write(&mut self, w: u32, now: u32, written: u32, old: u32) {
        // The MP3 accelerator (mp3acc.rs): +0 bit 0 starts job A, a write
        // to +4 job B. Both finish at once: bits 0 and 1 (busy, which the
        // driver waits on at 0x3715C) read clear.
        if w == 0xD0A0_0000 && now & 1 != 0 && old & 1 == 0 {
            self.note_mp3_job('A');
            self.mp3_job_a();
            self.io.insert(w, now & !3);
            return;
        }
        if w == 0xD0A0_0004 && now != 0 {
            self.note_mp3_job('B');
            self.mp3_job_b();
            // Research: what the job wrote.
            if self.mp3_log && self.now >= self.mp3_log_from && self.mp3_jobs.len() <= 400 {
                let out = (self.latch(0xD0A0_0048) & 0xFFFF) | (self.latch(0xD0A0_004C) & 0xFFFF) << 16;
                let bytes: Vec<u8> = (0..2304u32).map(|i| self.peek_byte(out.wrapping_add(i))).collect();
                if let Some(j) = self.mp3_jobs.last_mut() {
                    j.memory.push((out, bytes));
                }
            }
            return;
        }
        if (crate::udc::BASE..crate::udc::BASE + 0x200).contains(&w) {
            self.udc_write(w, now, written, old);
            return;
        }
        match w {
            0xD040_0E00 | 0xD040_0E80 => self.spu_enable(((w >> 7) & 1) as usize, now & 0xFFFF, old),
            0xD040_0E28 | 0xD040_0EA8 => self.spu_ramp_down(((w >> 7) & 1) as usize, now, old),
            // Stop flags: written 1s clear.
            0xD040_0E2C | 0xD040_0EAC => self.spu_stopped[((w >> 7) & 1) as usize] &= !written,
            0xD040_0E14 => {
                if written & 0x4000 != 0 {
                    self.beat_flag = false;
                }
                self.io.insert(w, now & !0x4000);
            }
            // Timer control (+0): bit 15 written as 1 acknowledges its
            // interrupt (the handler at 0x5968 does this).
            0xC002_0000 | 0xC002_0020 | 0xC002_0040 | 0xC002_0060 => {
                let n = ((w >> 5) & 3) as usize;
                if written & 0x8000 != 0 {
                    self.timer_flags &= !(1 << n);
                    self.io.insert(w, now & !0x8000);
                }
                if now & 0x6000 != 0x6000 {
                    self.timer_due[n] = 0;
                }
                self.lines_dirty = true;
            }
            // DMA, 4 channels at 0xD0300000 + 0x40 n (the Zippity's layout,
            // seen in the Twist's sector reads): +0 control (bit 0 start,
            // bit 7 the device side stays put, bits 13-12 unit: bytes,
            // halfwords, words), +4 source, +8 destination, +C count of
            // units.
            // To the DAC: paced by its sample rate (see `tick`). The driver
            // queues the next buffer in +4/+8/+C while one plays; with
            // control bit 10 set the channel goes on with it.
            0xD030_0000 | 0xD030_0040 | 0xD030_0080 | 0xD030_00C0
                if now & 1 != 0 && matches!(self.latch(w + 8), 0xC00D_0004 | 0xC00D_0024) =>
            {
                let ch = ((w >> 6) & 3) as usize;
                self.dma_paced[ch] = Some((self.latch(w + 4), self.latch(w + 12)));
                if self.dma_log.len() < 100_000 {
                    self.dma_log.push((
                        self.now,
                        ch as u32,
                        now,
                        self.latch(w + 4),
                        self.latch(w + 8),
                        self.latch(w + 12) * 2,
                    ));
                }
                self.io.insert(w, (now & !1) | 2);
            }
            0xD030_0000 | 0xD030_0040 | 0xD030_0080 | 0xD030_00C0 if now & 1 != 0 => {
                let (src, dst, count) = (self.latch(w + 4), self.latch(w + 8), self.latch(w + 12));
                let unit = 1usize << ((now >> 12) & 3).min(2);
                // Bit 7: a device port takes part and stays put, whichever
                // side it is on (the I/O space, 0xC0000000 up). VERIFIED: the
                // eMMC driver uses the same control word, 0x6FCD, both to
                // read sectors (port to memory) and to write them (memory to
                // port); with the destination moving, writes spilled into the
                // host's other registers and never reached the chip.
                let fixed = now & 0x80 != 0;
                let port = |a: u32| a >= 0xC000_0000;
                for i in 0..count {
                    let step = i * unit as u32;
                    let from = if fixed && port(src) { src } else { src.wrapping_add(step) };
                    let to = if fixed && port(dst) { dst } else { dst.wrapping_add(step) };
                    let v = self.read(from, unit);
                    self.write(to, v, unit);
                }
                if self.dma_log.len() < 100_000 {
                    self.dma_log.push((self.now, (w >> 6) & 3, now, src, dst, count * unit as u32));
                }
                self.io.insert(w, now & !3);
                self.dma_status |= 1 << ((w >> 6) & 3);
                self.lines_dirty = true;
            }
            0xD030_01FC => {
                self.dma_status &= !written;
                self.lines_dirty = true;
            }
            0xD010_0030 | 0xD010_0038 => self.lines_dirty = true,
            // The panel interface (0xD0500000): +0x140 bits 7-4 0xD with
            // bit 0 set streams the framebuffer at +0x33C to the panel (the
            // base firmware's display code 0x24C20 and 0x25514; the frame
            // handler restarts it on every tearing-effect pulse). Commands
            // go through +0x16C, then 0x81 (a command) or 0xA1 (a data
            // byte) to +0x140; `Panel` keeps the registers that place the
            // picture.
            // Video DMA: 0xD05001C0 source, 0x1C4 destination, 0x1C8 the
            // number of words minus 1; copies at once and the count reads 0
            // when done (the menu waits for that at 0xE55C). As the Zippity's
            // video chip (ribbit-zippity bus.rs), same family and address.
            0xD050_01C8 => {
                let (src, dst) = (self.latch(0xD050_01C0), self.latch(0xD050_01C4));
                for i in 0..=now {
                    let v = self.read(src.wrapping_add(4 * i), 4);
                    self.write(dst.wrapping_add(4 * i), v, 4);
                }
                self.io.insert(w, 0);
            }
            0xD050_0140 if written & 1 != 0 && now & 0xF0 == 0xD0 => {
                // No display buffer (0x33C left 0: the game packs never
                // fill the System's buffer table, 0xC240): the panel is
                // sent the buffer the video chip draws into, 0x1E8.
                // ASSUMED: the menu, which sets both, shows the same either
                // way bar one frame of delay.
                let shown = self.latch(0xD050_033C);
                self.panel_source = Some(if shown != 0 { shown } else { self.latch(0xD050_01E8) });
                self.panel_shown = self.panel;
                self.panel_frames += 1;
                self.io.insert(w, now & !1);
            }
            0xD050_0140 if now & 1 != 0 && now & 0xF0 == 0x80 => self.panel.command(self.latch(0xD050_016C) as u8),
            0xD050_0140 if now & 1 != 0 && now & 0xF0 == 0xA0 => self.panel.data(self.latch(0xD050_016C) as u8),
            0xD050_018C => {
                self.io.insert(w, old & !written);
                self.lines_dirty = true;
            }
            0xD050_0188 => self.lines_dirty = true,
            0xD010_0020 => {
                self.io.insert(w, (old & EXT_STATUS & !written) | (now & !EXT_STATUS));
                self.lines_dirty = true;
            }
            0xC00D_0004 | 0xC00D_0024 => {
                let c = ((w >> 5) & 1) as usize;
                if self.dac_level[c] < DAC_FIFO {
                    self.dac_level[c] += 1;
                }
                if self.audio.len() < 1 << 24 {
                    self.audio.push(written as i16);
                }
                self.lines_dirty = true;
            }
            0xC00D_0000 | 0xC00D_0020 => self.lines_dirty = true,
            // The video decoder (0xD0C00000): +0xA0 bit 4 decodes the frame
            // at +0xC0C into +0xC08, predicting from the frame at +0xC04
            // (driver 0x572E0); +0xD0 enables its interrupts (bit clear =
            // on); +0xD4 is its status, write 1s to clear: bit 2 a frame
            // decoded, bit 4 a failure (the handler 0x56B90 posts 4 for the
            // first, and the player then shows the frame; for the second
            // the driver resets the decoder, 0x57074). Done at once.
            0xD0C0_00A0 if written & 0x10 != 0 => {
                if self.gfx_log.len() < 10_000 {
                    let regs = std::mem::take(&mut self.gfx_regs);
                    self.gfx_log.push((
                        self.now,
                        self.latch(0xD0C0_0C04),
                        self.latch(0xD0C0_0C08),
                        self.latch(0xD0C0_0C0C),
                        regs,
                    ));
                }
                self.io.insert(w, now & !0x10);
                let done = if self.decode_video_frame() { 0x4 } else { 0x10 };
                let s = self.latch(0xD0C0_00D4) | done;
                self.io.insert(0xD0C0_00D4, s);
                self.lines_dirty = true;
            }
            0xD0C0_00D4 => {
                self.io.insert(w, old & !written);
                self.lines_dirty = true;
            }
            0xD0C0_00D0 => self.lines_dirty = true,
            // The timebase (0xC0040000): +0x50 control (bit 15 on, bit 8
            // tick, bits 2-0 rate), +0x54 status (bit 8: a tick; write 1s
            // to clear), +0x58 interrupt enable (driver 0x4AA0, handler
            // 0x44AC).
            0xC004_0054 => {
                self.io.insert(w, old & !written);
                self.lines_dirty = true;
            }
            0xC004_0050 | 0xC004_0058 => self.lines_dirty = true,
            // The ADC (0xC00C0000), from the bootloader's reader 0xF00000:
            // +4 bits 2-0 channel, bit 6 starts a conversion, bit 7 says it
            // is done, bits 5-4 error flags (write 1s to clear); +8 the
            // 12-bit result in bits 15-4. The System's battery monitor
            // (0x22BB0) sets bit 14 to be interrupted when a conversion is
            // done: bit 15 is then pending, cleared by writing it back
            // (handler 0x229B8). Without it the monitor waits for good and
            // reports BATTERYLEVEL=0 to LeapFrog Connect.
            0xC00C_0004 => {
                let mut v = (now & !0x30 & !0x8000) | (old & !written & 0x8000);
                if written & 0x40 != 0 {
                    let ch = (now & 7) as usize;
                    self.io.insert(0xC00C_0008, self.adc[ch] << 4);
                    v = (v & !0x40) | 0x80;
                    if v & 0x4000 != 0 {
                        v |= 0x8000;
                    }
                } else if written & 0x80 == 0 {
                    v &= !0x80;
                }
                self.io.insert(w, v);
                self.lines_dirty = true;
            }
            0xD0C0_0000..=0xD0C0_0FFF => {
                if self.gfx_regs.len() < 256 {
                    self.gfx_regs.push((w, now));
                }
            }
            // Data written to the eMMC host's transmit port (+0x00; it
            // reads from +0x04): sector writes, by DMA.
            0xC00A_0000 => {
                if let Some(card) = &mut self.emmc
                    && card.data_in(written)
                {
                    // Bit 8: the block is written and the card ready again
                    // (the System waits for it after each write, 0x56B44,
                    // and gives up on the volume without it).
                    self.sd_status[0] |= 0x10C;
                }
            }
            _ => {}
        }
    }

    /// A GPIO port's input pins (`key` has the wiring). Buttons read low
    /// while pressed; the button grid is scanned: a column driven high
    /// (output bits 12-10 of 0xC0000004) shows its pressed keys as high rows
    /// (bits 15-13 of 0xC0000000).
    fn gpio_in(&self, port: u32) -> u32 {
        let mut v = !0u32;
        for &(p, bit, k) in key::PINS {
            if p == port && self.keys & k != 0 {
                v &= !(1 << bit);
            }
        }
        // Inputs that rest low: port C bit 11 is the USB cable's power
        // (high when plugged in: the bootloader's 0xF007D8 then goes into
        // its charging/USB mode).
        if port == 0xC000_00C0 && !self.usb_plugged {
            v &= !(1 << 11);
        }
        // The rotary encoders (`encoder`): direction pins, and phase A pins
        // low during a pulse.
        for (r, e) in encoder::ALL.iter().enumerate() {
            if e.dir_port == port {
                v = if self.turn_dir[r] { v | 1 << e.dir_bit } else { v & !(1 << e.dir_bit) };
            }
            if let Some((p, bit)) = e.phase_a
                && p == port
                && self.turn_low[r]
            {
                v &= !(1 << bit);
            }
        }
        // The Slider's pads read low where the finger is.
        for (n, &(p, bit)) in encoder::SLIDER_PADS.iter().enumerate() {
            if p == port && self.slider_pads & (1 << n) != 0 {
                v &= !(1 << bit);
            }
        }
        if port == 0xC000_0000 {
            let driven = self.latch(0xC000_0004) & 0x1C00;
            v &= !0xE000;
            for row in 0..3 {
                for col in 0..3 {
                    if self.keys & key::grid(row * 3 + col) != 0 && driven & (0x1000 >> col) != 0 {
                        v |= 0x8000 >> row;
                    }
                }
            }
        }
        v
    }

    /// Draws the sprites into the frame buffer at 0xD05001E8 (RAM).
    fn draw_into_buffer(&mut self) {
        let mut frame = std::mem::take(&mut self.frame);
        frame.resize(crate::video::FRAME_W * crate::video::FRAME_H, 0);
        self.draw_frame(&mut frame);
        let at = self.latch(0xD050_01E8) as usize;
        // No buffer yet (0 until the menu sets one up): nothing is drawn
        // (address 0 is the System's own code).
        if at == 0 {
            self.frame = frame;
            return;
        }
        if let Some(dst) = self.ram.get_mut(at..at + frame.len() * 2) {
            for (d, p) in dst.chunks_exact_mut(2).zip(&frame) {
                d.copy_from_slice(&p.to_le_bytes());
            }
        }
        self.frame = frame;
        self.frames_drawn += 1;
    }

    /// A timer's count was read. Read again and again within a few hundred
    /// instructions, it is being waited on (the menu paces its frames by
    /// reading the time until the next one is due, 0x6124): then time skips
    /// ahead a little at each read, as nothing else happens meanwhile.
    pub(crate) fn note_clock_poll(&mut self) {
        if self.now.wrapping_sub(self.last_poll) < POLL_GAP {
            self.poll_streak = self.poll_streak.saturating_add(1);
        } else {
            self.poll_streak = 0;
        }
        self.last_poll = self.now;
        if self.poll_streak >= POLL_STREAK {
            self.poll_skip = POLL_SKIP;
        }
    }

    /// Turns a rotary control (`input::Rotary` order) by `steps` detents
    /// (positive: the way that counts up).
    pub fn turn(&mut self, rotary: usize, steps: i32) {
        if let Some(p) = self.turns_pending.get_mut(rotary) {
            *p = p.saturating_add(steps);
        }
    }

    /// Moves the encoders on: one pulse edge per control at a time, the
    /// controls in turn, `EDGE / 4` apart. Never two edges at once: the
    /// System acknowledges an external interrupt by writing the status it
    /// read back with its own bit set (0x23040, 0x23884), which clears
    /// every other bit pending then too, so edges arriving together lose
    /// all but one (a hand never lines them up that closely).
    fn tick_encoders(&mut self) {
        if self.now < self.turn_due {
            return;
        }
        self.turn_due = self.now + encoder::EDGE / 4;
        let r = self.turn_slot;
        self.turn_slot = (r + 1) % encoder::ALL.len();
        let ext = self.latch(0xD010_0020);
        let mut raise = 0;
        {
            let e = &encoder::ALL[r];
            let pending = self.turns_pending[r];
            match (e.phase_a, self.turn_phase[r]) {
                // A step: the direction pin, then phase A falls (the
                // handler counts on this edge, reading the direction)...
                (Some(_), 0) if pending != 0 => {
                    self.turn_dir[r] = pending > 0;
                    self.turn_low[r] = true;
                    self.turn_phase[r] = 1;
                    raise |= e.status;
                }
                // ...and rises again (the handler only notes the level).
                (Some(_), 1) => {
                    self.turn_low[r] = false;
                    self.turn_phase[r] = 0;
                    self.turns_pending[r] -= pending.signum();
                    raise |= e.status;
                }
                // The red dial: one interrupt a step, read with its
                // direction pin.
                (None, _) if pending != 0 => {
                    self.turn_dir[r] = pending > 0;
                    self.turns_pending[r] -= pending.signum();
                    raise |= e.status;
                }
                _ => {}
            }
        }
        // Only interrupts the System has turned on (each status bit's
        // enable sits just below it: bits 10, 12, 14; bit 1 for status
        // bit 7).
        let enabled = |status: u32| match status {
            0x80 => ext & 0x2 != 0,
            s => ext & (s >> 1) != 0,
        };
        let raise = [0x80u32, 0x800, 0x2000, 0x8000].iter().filter(|&&b| raise & b != 0 && enabled(b)).fold(0, |a, b| a | b);
        if raise != 0 {
            self.io.insert(0xD010_0020, ext | raise);
            self.lines_dirty = true;
        }
    }

    /// One video decoder job (see `device_write`): the frame's data at
    /// +0xC0C, the reference picture the one sent to +0xC04 last, the
    /// result in RGB 5:6:5 at +0xC08, row by row, which the System sends to
    /// the panel as it is (VERIFIED: the start-up logo comes out upright).
    fn decode_video_frame(&mut self) -> bool {
        let (data_at, out_at, ref_at) =
            (self.latch(0xD0C0_0C0C) as usize, self.latch(0xD0C0_0C08), self.latch(0xD0C0_0C04));
        let Some(data) = self.ram.get(data_at..(data_at + 0x8_0000).min(self.ram.len())) else {
            self.mpeg_frames.1 += 1;
            return false;
        };
        let reference = self.mpeg_pictures.iter().rev().find(|(a, _)| *a == ref_at).map(|(_, p)| p);
        let pic = match self.mpeg.decode(data, reference) {
            Ok(p) => p,
            Err(_) => {
                self.mpeg_frames.1 += 1;
                return false;
            }
        };
        self.mpeg_frames.0 += 1;
        let (w, h) = (pic.width, pic.height);
        for y in 0..h {
            for x in 0..w {
                let a = out_at as usize + 2 * (y * w + x);
                if let Some(d) = self.ram.get_mut(a..a + 2) {
                    d.copy_from_slice(&pic.rgb565(x, y).to_le_bytes());
                }
            }
        }
        self.mpeg_pictures.retain(|(a, _)| *a != out_at);
        self.mpeg_pictures.push((out_at, pic));
        if self.mpeg_pictures.len() > 8 {
            self.mpeg_pictures.remove(0);
        }
        true
    }

    /// The screen as the player sees it, 320 x 240, as 0x00RRGGBB: the
    /// last frame streamed to the panel, placed in the panel's memory by the
    /// window and orientation it was sent with (`Panel::stored_index`).
    /// Black before the first frame, while the display is off (`Panel::on`)
    /// and outside the window.
    pub fn screen(&mut self, out: &mut [u32]) {
        let w = crate::SCREEN_W;
        let Some(base) = self.panel_source.filter(|_| self.panel.on) else {
            out.fill(0);
            return;
        };
        let geometry = self.panel_shown;
        // The System fills the screen buffer with 0x8000 as its blank before
        // the start-up logo (0x317A4); a whole frame of it is shown black
        // (EVIDENCE: a firmware would not flash dark red on purpose; how the
        // real panel shows it is UNVERIFIED).
        if (0..240 * 320).step_by(97).all(|n| self.read(base.wrapping_add(2 * n), 2) == 0x8000) {
            out.fill(0);
            return;
        }
        for (i, px) in out.iter_mut().enumerate() {
            *px = match geometry.stored_index(i % w, i / w) {
                Some(n) => {
                    let p = self.read(base.wrapping_add(2 * n as u32), 2);
                    let (r, g, b) = ((p >> 11) & 31, (p >> 5) & 63, p & 31);
                    (r * 255 / 31) << 16 | (g * 255 / 63) << 8 | (b * 255 / 31)
                }
                None => 0,
            };
        }
    }

    /// Interrupt lines raised (bit n: line n).
    fn raised(&self) -> u64 {
        let mut lines = 0u64;
        for (n, line) in IRQ_TIMER.iter().enumerate() {
            if self.timer_flags & (1 << n) != 0 {
                lines |= 1 << line;
            }
        }
        if self.dma_status != 0 {
            lines |= 1 << IRQ_DMA;
        }
        if self.latch(0xD0C0_00D4) & !self.latch(0xD0C0_00D0) & 0x3F != 0 {
            lines |= 1 << IRQ_GFX;
        }
        if self.latch(0xC004_0054) & self.latch(0xC004_0058) != 0 {
            lines |= 1 << IRQ_TIMEBASE;
        }
        let ext = self.latch(0xD010_0020);
        if ext & 0xC0 != 0 {
            lines |= 1 << IRQ_EXT_A;
        }
        if ext & 0xA800 != 0 {
            lines |= 1 << IRQ_EXT_B;
        }
        if self.latch(0xD050_018C) & self.latch(0xD050_0188) & 1 != 0 {
            lines |= 1 << IRQ_VIDEO;
        }
        if self.latch(0xC00C_0004) & 0xC000 == 0xC000 {
            lines |= 1 << IRQ_ADC;
        }
        // A DAC channel wants samples: enabled (bit 14) and half empty.
        for c in 0..2 {
            if self.latch(0xC00D_0000 + 0x20 * c as u32) & 0x4000 != 0 && self.dac_level[c] <= DAC_FIFO / 2 {
                lines |= 1 << IRQ_DAC[c];
            }
        }
        lines
    }

    /// The lowest raised line not masked (0xD0100030: bit 32 - n masks
    /// line n, as on the Zippity; the Twist's enable/disable functions at
    /// 0x3C68/0x3CC0 use the same rule), or 0.
    fn pending_irq(&self) -> u32 {
        let unmasked = ((!self.latch(0xD010_0030)).reverse_bits() as u64) << 1;
        let pending = self.raised() & unmasked & 0x1_FFFF_FFFE;
        if pending == 0 { 0 } else { pending.trailing_zeros() }
    }

    /// The processor's IRQ input: a pending line, with interrupts not
    /// disabled at the controller (0xD0100038 bit 0).
    pub fn irq(&self) -> bool {
        self.latch(0xD010_0038) & 1 == 0 && self.pending_irq() != 0
    }

    /// Runs the timers and the DAC up to now (every few hundred
    /// instructions).
    pub fn tick(&mut self) {
        self.tick_encoders();
        self.tick_sound();
        if self.udc.attached {
            self.udc_poll();
        }
        // The panel's tearing-effect pulse, when its interrupt is enabled.
        let period = CLOCK / TE_HZ;
        if self.now >= self.te_due {
            self.te_due = self.now + period;
            let ext = self.latch(0xD010_0020);
            if ext & 1 != 0 {
                self.io.insert(0xD010_0020, ext | 0x40);
                self.lines_dirty = true;
            }
            // A frame ends on the video chip too (ASSUMED: at the panel's
            // rate): it has drawn the frame from its sprite list into the
            // buffer at 0x1E8 (`video`; drawn here, at the end, as the menu
            // copies its list in more than one transfer), then raises bit 0
            // of 0xD050018C, kept until written as 1.
            if self.latch(0xD050_0108) & 1 != 0 && !self.skip_drawing {
                self.draw_into_buffer();
            }
            if self.latch(0xD050_0188) & 1 != 0 {
                self.io.insert(0xD050_018C, self.latch(0xD050_018C) | 1);
                self.lines_dirty = true;
            }
        }
        // The timebase's tick.
        if self.latch(0xC004_0050) & 0x8100 == 0x8100 {
            let period = CLOCK / TIMEBASE_HZ;
            if self.timebase_due == 0 {
                self.timebase_due = self.now + period;
            } else if self.now >= self.timebase_due {
                self.timebase_due += period;
                let s = self.latch(0xC004_0054) | 0x100;
                self.io.insert(0xC004_0054, s);
                self.lines_dirty = true;
            }
        } else {
            self.timebase_due = 0;
        }
        let per_sample = CLOCK / DAC_HZ;
        // DMA into the DAC: one 32-bit unit per sample period.
        let due = (self.now - self.dma_paced_last) / per_sample;
        if due > 0 {
            let first = self.dma_paced_last + per_sample;
            self.dma_paced_last += due * per_sample;
            for ch in 0..4 {
                let Some((mut src, mut left)) = self.dma_paced[ch] else { continue };
                let base = 0xD030_0000 + 0x40 * ch as u32;
                for k in 0..due {
                    if left == 0 {
                        break;
                    }
                    let v = self.read(src, 4);
                    if self.audio.len() < 1 << 24 {
                        self.audio.push(v as i16);
                        self.audio.push((v >> 16) as i16);
                    }
                    self.dac_prev = self.dac_hold;
                    self.dac_hold = (v as i16, (v >> 16) as i16);
                    // When this sample was due (not this tick's time: ticks
                    // come every 256 cycles, a twentieth of a sample).
                    self.dac_at = first + k * per_sample;
                    self.dac_hold_until = self.dac_at + 2 * per_sample;
                    src = src.wrapping_add(4);
                    // The count is in 16-bit units, two to a stereo sample
                    // (EVIDENCE: the MP3 driver's buffers are 0x900 units,
                    // 0x1200 bytes of samples each, 0x1220 apart; read as 32
                    // bits a unit, every buffer ran on into the next).
                    left = left.saturating_sub(2);
                }
                if left == 0 {
                    self.dma_status |= 1 << ch;
                    self.lines_dirty = true;
                    let ctrl = self.latch(base);
                    self.dma_paced[ch] = if ctrl & 0x400 != 0 && self.latch(base + 12) != 0 {
                        // Go on with the queued buffer (and start it again
                        // only when the driver queues another).
                        let next = (self.latch(base + 4), self.latch(base + 12));
                        if self.dma_log.len() < 100_000 {
                            // Research: a chained buffer (control bit 31 marks it).
                            self.dma_log.push((self.now, ch as u32, ctrl | 0x8000_0000, next.0, self.latch(base + 8), next.1 * 2));
                        }
                        self.io.insert(base + 12, 0);
                        Some(next)
                    } else {
                        self.io.insert(base, ctrl & !3);
                        None
                    };
                } else {
                    self.dma_paced[ch] = Some((src, left));
                }
            }
        }
        for c in 0..2 {
            let drained = (self.now - self.dac_last[c]) / per_sample;
            if drained > 0 {
                self.dac_last[c] += drained * per_sample;
                let before = self.dac_level[c];
                self.dac_level[c] = before.saturating_sub(drained.min(DAC_FIFO as u64) as u32);
                if before != self.dac_level[c] {
                    self.lines_dirty = true;
                }
            }
        }
        for n in 0..4 {
            let ctrl = self.latch(0xC002_0000 + 0x20 * n as u32);
            if ctrl & 0x6000 != 0x6000 {
                self.timer_due[n] = 0;
                continue;
            }
            let count = 0x1_0000 - (self.latch(0xC002_0008 + 0x20 * n as u32) & 0xFFFF) as u64;
            let period = (count * CLOCK / TIMER0_HZ).max(1);
            if self.timer_due[n] == 0 {
                self.timer_due[n] = self.now + period;
            } else if self.now >= self.timer_due[n] {
                self.timer_flags |= 1 << n;
                self.timer_due[n] += period;
                self.lines_dirty = true;
            }
        }
    }

    /// The SD/MMC hosts at 0xC00A0000 (eMMC) and 0xC00B0000 (SD card),
    /// from the base firmware's driver (0x56200-0x56900):
    /// +0x08 control: writing bit 6 starts the card's power-up clocks; the
    /// bit clears itself when they are done (the driver waits for it).
    /// +0x14 status: bit 0 busy (the driver waits for it to clear); writing
    /// 1s clears bits (the driver writes 0xFFFF before each operation).
    fn sd_write(&mut self, w: u32, v: u32) {
        let host = match w & !0xFFFF {
            0xC00A_0000 => 0,
            0xC00B_0000 => 1,
            _ => return,
        };
        match w & 0xFFFF {
            0x08 if v & 0x40 != 0 => {
                self.io.insert(w, v & !0x40);
            }
            // Bit 11: send the card its initialisation clocks; status bit 2
            // when done (the bootloader waits for it at 0xF00864).
            0x08 if v & 0x880 == 0x800 => {
                self.io.insert(w, v & !0x800);
                self.sd_status[host] |= 0x04;
            }
            0x14 => self.sd_status[host] &= !v,
            0x08 if v & 0x80 != 0 => self.sd_command(host, v),
            _ => {}
        }
    }

    /// Runs a command written to a host's +0x08 (bits 5-0: command; bit 7
    /// start; bit 12 a response is expected) with the argument in +0x0C.
    fn sd_command(&mut self, host: usize, v: u32) {
        let base = if host == 0 { 0xC00A_0000 } else { 0xC00B_0000 };
        self.io.insert(base + 8, v & !0x80);
        let (index, arg) = (v & 0x3F, self.latch(base + 0x0C));
        let now = self.now;
        let response = match (host, &mut self.emmc) {
            (0, Some(card)) => {
                card.now = now;
                card.command(index, arg)
            }
            _ => Response::None,
        };
        // Bit 2: the command is complete. Bit 6: a response word waits in
        // +0x10, which gives one word per read (a 128-bit response is read
        // as four words, most significant first): the bootloader's command
        // routine 0xF00A3C waits for bit 6 before each word, then bit 2.
        self.sd_status[host] |= 0x04;
        self.sd_response[host].clear();
        match response {
            // ASSUMED: bit 5 is the response time-out (the drivers treat
            // bits 4, 5 and 9 as errors). No response word: the
            // bootloader's wait (0xF00864) takes bit 6 as an answer.
            Response::None if v & 0x1000 != 0 => self.sd_status[host] |= 0x20,
            Response::None => {}
            Response::Short(r) => self.sd_response[host].push_back(r),
            Response::Long(r) => self.sd_response[host].extend(r),
        }
        if !self.sd_response[host].is_empty() {
            self.sd_status[host] |= 0x40;
        }
    }

    /// The next response word (+0x10); bit 6 clears when none is left.
    fn sd_response_word(&mut self, host: usize) -> u32 {
        let w = self.sd_response[host].pop_front().unwrap_or(0);
        if self.sd_response[host].is_empty() {
            self.sd_status[host] &= !0x40;
        }
        w
    }

    /// A word from the eMMC host's data port (+0x04).
    fn sd_data(&mut self) -> u32 {
        let Some(card) = &mut self.emmc else {
            return 0;
        };
        let w = card.fifo.pop_front().unwrap_or(0);
        if card.fifo.is_empty() {
            // Bit 3: a block is complete (the bootloader waits for it after
            // each block, 0xF009A4, then stops a multiple-block read with
            // CMD12). A multiple-block read then goes on with the next
            // sector.
            self.sd_status[0] |= 0x0C;
            card.next_block();
        }
        w
    }

    /// The bytes behind an address, if it is memory.
    fn mem(&mut self, a: u32, n: usize) -> Option<&mut [u8]> {
        let i = a as usize;
        if i + n <= RAM_SIZE {
            return Some(&mut self.ram[i..i + n]);
        }
        let o = a.wrapping_sub(IRAM_BASE) as usize;
        if o + n <= IRAM_SIZE {
            return Some(&mut self.iram[o..o + n]);
        }
        None
    }

    pub(crate) fn read(&mut self, a: u32, n: usize) -> u32 {
        if let Some(m) = self.mem(a, n) {
            return m.iter().rev().fold(0, |v, b| v << 8 | *b as u32);
        }
        if (IO_START..IO_END).contains(&a) {
            let v = self.io_read(a);
            return v >> (8 * (a & 3)) & if n == 4 { u32::MAX } else { (1 << (8 * n)) - 1 };
        }
        0
    }

    pub(crate) fn write(&mut self, a: u32, v: u32, n: usize) {
        if let Some((lo, hi)) = self.ram_watch
            && (lo..hi).contains(&a)
            && self.ram_writes.len() < 200_000
        {
            self.ram_writes.push((self.now, self.pc, a, v));
        }
        if let Some(m) = self.mem(a, n) {
            for (k, b) in m.iter_mut().enumerate() {
                *b = (v >> (8 * k)) as u8;
            }
            return;
        }
        if (IO_START..IO_END).contains(&a) {
            self.io_write(a, v, if n == 4 { u32::MAX } else { (1 << (8 * n)) - 1 });
        }
    }
}

impl ArmBus for Bus {
    fn read8(&mut self, a: u32) -> u8 {
        self.read(a, 1) as u8
    }
    fn read16(&mut self, a: u32) -> u16 {
        self.read(a, 2) as u16
    }
    fn read32(&mut self, a: u32) -> u32 {
        self.read(a, 4)
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.write(a, v as u32, 1)
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.write(a, v as u32, 2)
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.write(a, v, 4)
    }
}

/// Why `run` stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    Budget,
    /// Waiting for an interrupt, none of which is modelled yet.
    Idle,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Twist {
    pub cpu: Arm,
    pub bus: Bus,
    /// The System's clock functions (System_getCount and
    /// System_getHighPrecisionCount, found by name in its export table):
    /// calling them again and again is waiting on the clock.
    pub clock_fns: Vec<u32>,
    /// The System's wait for the next frame (PPU_waitVBLK, found by name):
    /// its first bytes, where it spins until the frame interrupt's handler
    /// clears a flag.
    pub vblank_wait: Option<std::ops::Range<u32>>,
    /// Research: count instructions by address in `hot` (costly: off
    /// unless asked for).
    #[serde(skip)]
    pub profile: bool,
    /// Extra cycles the processor takes for every 256 it runs, while the
    /// emulator cannot run it at full speed (`stall_for`): the sound unit,
    /// the timers and the panel keep to real time, so the music plays on
    /// while the processor does heavy work, as on a real unit.
    #[serde(skip)]
    pub stall: u32,
    #[serde(skip)]
    stall_left: u32,
    /// Research: registers r0-r15 each time execution reaches this address
    /// (at most `BREAKS_MAX`), with the time.
    #[serde(skip)]
    pub break_at: Option<u32>,
    #[serde(skip)]
    pub breaks: Vec<(u64, [u32; 16])>,
    /// The text at r0 at each stop.
    #[serde(skip)]
    pub break_texts: Vec<String>,
    /// Instructions run at each address in the last `HOT_WINDOW`.
    #[serde(skip)]
    pub hot: HashMap<u32, u64>,
    /// Research: times execution reached each of these addresses (only
    /// the keys put in beforehand are counted), from `count_from` on.
    #[serde(skip)]
    pub counts: HashMap<u32, u64>,
    #[serde(skip)]
    pub count_from: u64,
}

/// A job given to the MP3 accelerator (research, `Bus::mp3_jobs`).
#[derive(Clone, Debug)]
pub struct Mp3Job {
    pub time: u64,
    pub kind: char,
    pub regs: [u32; 20],
    /// (address, 0x1200 bytes from it) for each address register pair.
    pub memory: Vec<(u32, Vec<u8>)>,
}

pub const HOT_WINDOW: u64 = 500_000;
/// The most cycles an instruction may take when the emulator falls behind
/// (`Twist::stall_for`): the interpreter's slowest stretches run about 85
/// million instructions a second, a little over a third of 240 MHz.
pub const MAX_CPI: f64 = 4.0;

/// The registers' map hashes their addresses with one multiply (they
/// are looked up many times per device tick; the standard hasher is
/// several times slower and its protection against crafted keys is not
/// needed here).
#[derive(Clone, Copy, Default)]
pub struct IoHash;

impl std::hash::BuildHasher for IoHash {
    type Hasher = IoHasher;
    fn build_hasher(&self) -> IoHasher {
        IoHasher(0)
    }
}

pub struct IoHasher(u64);

impl std::hash::Hasher for IoHasher {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.0 = (self.0 << 8 | b as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
    }
    fn write_u32(&mut self, v: u32) {
        // Fibonacci hashing; the map uses the top bits as well as the low.
        self.0 = (v as u64 ^ self.0).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
}

/// Save states: this number first, then the machine (bincode). Bumped
/// whenever the machine's layout changes.
const STATE_VERSION: u32 = 1;

/// RAM in states: runs of zero bytes counted, the rest as they are (RAM is
/// 64 MB, much of it unused). Pairs of (zeros, literal bytes).
mod rle {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(v: &[u8], s: S) -> Result<S::Ok, S::Error> {
        let mut runs: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut i = 0;
        while i < v.len() {
            let z = v[i..].iter().take_while(|&&b| b == 0).count();
            i += z;
            let start = i;
            // A literal stretch ends at the next 64 zero bytes in a row.
            while i < v.len() && !(v[i] == 0 && v[i..].iter().take(64).all(|&b| b == 0) && v.len() - i >= 64) {
                i += 1;
            }
            runs.push((z as u32, v[start..i].to_vec()));
        }
        (v.len() as u64, runs).serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        let (len, runs): (u64, Vec<(u32, Vec<u8>)>) = Deserialize::deserialize(d)?;
        let mut v = Vec::with_capacity(len as usize);
        for (z, lit) in runs {
            v.resize(v.len() + z as usize, 0);
            v.extend_from_slice(&lit);
        }
        v.resize(len as usize, 0);
        Ok(v)
    }
}
pub const BREAKS_MAX: usize = 10_000;

/// The address of the System function exported as `name`: its export table
/// pairs a name's address with the function's (VERIFIED for Base 1.0.50.0:
/// "System_getCount" at 0x799E0, entry 0x6E380, function 0x15B08). The
/// System runs where it is loaded, address 0.
pub fn export(system: &[u8], name: &str) -> Option<u32> {
    let mut needle = name.as_bytes().to_vec();
    needle.push(0);
    let at = system.windows(needle.len()).position(|w| w == needle)? as u32;
    system.chunks_exact(4).enumerate().find_map(|(i, w)| {
        (u32::from_le_bytes(w.try_into().unwrap()) == at)
            .then(|| system.get(4 * i + 4..4 * i + 8).map(|f| u32::from_le_bytes(f.try_into().unwrap())))
            .flatten()
    })
}

/// Where the System waits for the next frame: PPU_waitVBLK's first 0x24
/// bytes, which hold its spin loop (VERIFIED for Base 1.0.50.0: 0xF394, the
/// loop at 0xF3A8-0xF3B4 reading a flag the frame interrupt clears; the
/// games spend most of their time there).
pub fn vblank_wait(system: &[u8]) -> Option<std::ops::Range<u32>> {
    export(system, "PPU_waitVBLK").map(|a| a..a + 0x24)
}

/// Clock polling (`Bus::note_clock_poll`): reads this close (cycles) count
/// as one wait, after this many time skips this much (10 µs, well under the
/// 1 ms the menu's frame timing works in).
const POLL_GAP: u64 = 3000;
const POLL_STREAK: u32 = 8;
const POLL_SKIP: u64 = CLOCK / 100_000;

/// The eMMC's size in sectors (4 GB, ASSUMED: the retail capacity is not
/// known).
pub const EMMC_SECTORS: u64 = 7_634_944;

impl Twist {
    /// The machine as it is now (a save state). The storage's base image is
    /// left out (`load_state` keeps the running one).
    pub fn save_state(&self) -> Vec<u8> {
        let mut out = STATE_VERSION.to_le_bytes().to_vec();
        out.extend(bincode::serialize(self).expect("serialise the machine"));
        out
    }

    /// Restores a state from `save_state`, keeping the storage's image and
    /// the USB connection.
    pub fn load_state(&mut self, state: &[u8]) -> Result<(), String> {
        let version = state.get(..4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        if version != Some(STATE_VERSION) {
            return Err("this state is from another version of the RockIt Twist model".into());
        }
        let mut t: Twist = bincode::deserialize(&state[4..]).map_err(|e| e.to_string())?;
        if let (Some(new), Some(old)) = (&mut t.bus.emmc, &mut self.bus.emmc) {
            std::mem::swap(&mut new.image, &mut old.image);
            new.logging = old.logging;
        }
        t.bus.udc = std::mem::take(&mut self.bus.udc);
        t.bus.frame = std::mem::take(&mut self.bus.frame);
        t.bus.lines_dirty = true;
        *self = t;
        Ok(())
    }

    /// The whole console, started from its bootloader: the code of the
    /// Bootloader, Base (System) and MfgTest packages (`image::code`) and
    /// the Firmware-Bulk package's `FileSys` volume. The eMMC is built as
    /// the bootloader expects it (`disk`), holding the volume as a unit
    /// that has finished its factory update keeps it (no FIRMWARE_UPGRADE
    /// marker). Its size, 4 GB, is ASSUMED.
    pub fn boot(bootloader: &[u8], system: &[u8], factory_test: &[u8], mut volume: Vec<u8>) -> Self {
        use crate::disk::{Partition, build};
        crate::fat::delete_root_entry(&mut volume, "FIRMWARE_UPGRADE");
        let image = build(&[Partition { data: factory_test, load: 0 }, Partition { data: system, load: 0 }], &volume);
        let mut t = Self::new(bootloader);
        t.bus.emmc = Some(Card::emmc(image, EMMC_SECTORS));
        t.clock_fns =
            ["System_getCount", "System_getHighPrecisionCount"].iter().filter_map(|n| export(system, n)).collect();
        t.vblank_wait = vblank_wait(system);
        t
    }

    /// A machine with `code` (a firmware image's code, see `image::code`)
    /// at address 0, reset to run it.
    pub fn new(code: &[u8]) -> Self {
        let mut bus = Bus::new();
        let n = code.len().min(RAM_SIZE);
        bus.ram[..n].copy_from_slice(&code[..n]);
        let mut cpu = Arm::new();
        cpu.reset();
        Self { cpu, bus, hot: HashMap::new(), clock_fns: Vec::new(), profile: false, stall: 0, stall_left: 0, break_at: None, breaks: Vec::new(), break_texts: Vec::new(), vblank_wait: None, counts: HashMap::new(), count_from: 0 }
    }

    /// Runs for `budget` clock cycles (one instruction each). While the
    /// processor waits for an interrupt or spins on a branch to itself,
    /// time skips to the next device tick.
    pub fn run(&mut self, budget: u64) -> Stop {
        let end = self.bus.now + budget;
        let hot_from = end.saturating_sub(HOT_WINDOW);
        let mut idle = 0u64;
        // The clock functions as four slots (1, never an instruction's
        // address, fills the rest): checked on every instruction.
        let mut clock_fns = [1u32; 4];
        for (slot, f) in clock_fns.iter_mut().zip(&self.clock_fns) {
            *slot = *f;
        }
        let clock_fns_more = self.clock_fns.len() > 4;
        while self.bus.now < end {
            if self.bus.now & 0xFF == 0 {
                self.bus.tick();
                self.stall_left += self.stall;
                while self.stall_left >= 256 && self.bus.now + 256 < end {
                    self.stall_left -= 256;
                    self.bus.now += 256;
                    self.bus.tick();
                }
            }
            if self.bus.lines_dirty {
                self.bus.lines_dirty = false;
                self.cpu.irq_line = self.bus.irq();
            }
            self.bus.pc = self.cpu.r[15];
            if self.break_at == Some(self.bus.pc) && self.breaks.len() < BREAKS_MAX {
                self.breaks.push((self.bus.now, self.cpu.r));
                // The text r0 points at, if any (a file name...).
                let at = self.cpu.r[0] as usize;
                let text: String = self
                    .bus
                    .ram
                    .get(at..at.saturating_add(160).min(self.bus.ram.len()))
                    .unwrap_or(&[])
                    .iter()
                    .take_while(|&&b| b != 0)
                    .map(|&b| if (32..127).contains(&b) { b as char } else { '.' })
                    .collect();
                self.break_texts.push(text);
            }
            if !self.counts.is_empty()
                && self.bus.now >= self.count_from
                && let Some(n) = self.counts.get_mut(&self.bus.pc)
            {
                *n += 1;
            }
            let pc = self.bus.pc;
            if (pc == clock_fns[0] || pc == clock_fns[1] || pc == clock_fns[2] || pc == clock_fns[3])
                || (clock_fns_more && self.clock_fns.contains(&pc))
            {
                self.bus.note_clock_poll();
            }
            if !self.cpu.step(&mut self.bus) {
                // Waiting for an interrupt: nothing happens before the
                // devices next tick.
                idle += 1;
                if idle > 1 << 24 {
                    return Stop::Idle;
                }
                self.bus.now = (self.bus.now | 0xFF).min(end - 1);
            } else if !self.cpu.irq_line && self.vblank_wait.as_ref().is_some_and(|r| r.contains(&self.cpu.current)) {
                // Waiting for the frame interrupt: nothing changes before
                // the devices next tick.
                self.bus.now = (self.bus.now | 0xFF).min(end - 1);
            } else if self.bus.poll_skip != 0 {
                // Waiting on the clock: skip ahead, landing just before a
                // device tick so timers and frames are serviced.
                let to = (self.bus.now + std::mem::take(&mut self.bus.poll_skip)) | 0xFF;
                self.bus.now = to.min(end - 1);
            } else if self.cpu.r[15] == self.cpu.current && !self.cpu.irq_line {
                // A branch to itself (the system's idle loop, 0x57524):
                // the same until a device ticks.
                self.bus.now = (self.bus.now | 0xFF).min(end - 1);
            } else {
                idle = 0;
                if self.profile && self.bus.now >= hot_from {
                    *self.hot.entry(self.cpu.current).or_default() += 1;
                }
            }
            self.bus.now += 1;
        }
        Stop::Budget
    }

    /// Sets `stall` from how fast the last `run` went: `budget` cycles in
    /// `took` seconds of the host's time. The processor gets as slow as
    /// it must for the emulator to keep up (with some room to spare), up
    /// to `MAX_CPI` cycles an instruction, and quickens again when it can.
    /// (One cycle an instruction at 240 MHz is ASSUMED; a real ARM9 takes
    /// more, waiting on memory.)
    pub fn stall_for(&mut self, budget: u64, took: f64) {
        let cpi = 1.0 + self.stall as f64 / 256.0;
        let speed = budget as f64 / CLOCK as f64 / took.max(1e-6);
        let want = (cpi * 1.15 / speed).clamp(1.0, MAX_CPI);
        let cpi = if want > cpi { want } else { cpi * 0.75 + want * 0.25 };
        self.stall = ((cpi - 1.0) * 256.0) as u32;
    }

    /// The addresses run most at the end of the last `run`.
    pub fn hottest(&self, n: usize) -> Vec<(u32, u64)> {
        let mut v: Vec<_> = self.hot.iter().map(|(&a, &c)| (a, c)).collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.truncate(n);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_processor_slows_only_while_the_emulator_falls_behind() {
        let mut t = Twist::new(&[]);
        let slice = CLOCK / 120;
        // Half speed: about 2.3 cycles an instruction at once.
        t.stall_for(slice, 2.0 / 120.0);
        assert!((300..400).contains(&t.stall), "stall {}", t.stall);
        // Far too slow: no more than MAX_CPI.
        t.stall_for(slice, 1.0);
        assert_eq!(t.stall, ((MAX_CPI - 1.0) * 256.0) as u32);
        // Fast again: back to one cycle an instruction.
        for _ in 0..40 {
            t.stall_for(slice, 0.1 / 120.0);
        }
        assert_eq!(t.stall, 0);
        // The stall moves time on: devices tick, fewer instructions run.
        t.stall = 768;
        t.run(1 << 16);
        assert_eq!(t.bus.now, 1 << 16);
    }

    fn words(ws: &[u32]) -> Vec<u8> {
        ws.iter().flat_map(|w| w.to_le_bytes()).collect()
    }

    fn panel(commands: &[(u8, &[u8])]) -> Panel {
        let mut p = Panel::default();
        for (c, data) in commands {
            p.command(*c);
            for d in *data {
                p.data(*d);
            }
        }
        p
    }

    #[test]
    fn buttons_read_as_the_firmwares_key_scan_expects() {
        let mut t = Twist::new(&[]);
        assert_eq!(t.bus.read(0xC000_0180, 4) & 0xF, 0xF, "nothing held: all high");
        t.bus.keys = key::UP | key::HOME | key::grid(4);
        assert_eq!(t.bus.read(0xC000_0180, 4) & 0xF, 0xE, "UP pulls bit 0 low");
        assert_eq!(t.bus.read(0xC000_0040, 4) & 0x300, 0x100, "HOME pulls bit 9 low");
        // MATRIX 5: row 2 (bit 14) shows only while column 2 (0x800) is driven.
        t.bus.write(0xC000_0004, 0x1000, 4);
        assert_eq!(t.bus.read(0xC000_0000, 4) & 0xE000, 0);
        t.bus.write(0xC000_0004, 0x800, 4);
        assert_eq!(t.bus.read(0xC000_0000, 4) & 0xE000, 0x4000);
    }

    #[test]
    fn a_dial_turn_is_a_pulse_on_its_pins_and_interrupt() {
        let mut t = Twist::new(&[]);
        // The System enables the encoders' interrupts (bits 10, 12, 14).
        t.bus.write(0xD010_0020, 0x5400, 4);
        let yellow = 1;
        t.bus.turn(yellow, 1);
        // Each control moves in its own slot, EDGE / 4 apart.
        let next_edge = |t: &mut Twist| {
            for _ in 0..4 {
                t.bus.now += encoder::EDGE / 4;
                t.bus.tick_encoders();
            }
        };
        next_edge(&mut t);
        assert_eq!(t.bus.read(0xC000_0110, 4) & 1, 0, "phase A low");
        assert_ne!(t.bus.read(0xC000_0040, 4) & (1 << 11), 0, "direction high: up");
        assert_ne!(t.bus.latch(0xD010_0020) & 0x800, 0, "its interrupt");
        t.bus.write(0xD010_0020, 0x800 | 0x5400, 4); // acknowledged
        next_edge(&mut t);
        assert_eq!(t.bus.read(0xC000_0110, 4) & 1, 1, "phase A back up");
        assert_ne!(t.bus.latch(0xD010_0020) & 0x800, 0);
        // Turned down: the direction pin low.
        t.bus.turn(yellow, -1);
        next_edge(&mut t);
        assert_eq!(t.bus.read(0xC000_0040, 4) & (1 << 11), 0);
        // The Slider: one pad low under the finger.
        t.bus.slider_pads = encoder::pads(Some(0.1));
        assert_eq!(t.bus.read(0xC000_0040, 4) & 0x16, 0x12, "pad 0: port B bit 2 low");
    }

    #[test]
    fn both_firmwares_panel_setups_come_out_upright() {
        // The System: 36 40, columns 0-239, pages 0-319. Its pictures are
        // stored 240 wide; the screen's top-left is stored pixel 239 (the
        // end of the first stored row), as seen on the Device Error screen.
        let system = panel(&[(0x36, &[0x40]), (0x2A, &[0, 0, 0, 0xEF]), (0x2B, &[0, 0, 1, 0x3F])]);
        assert_eq!(system.stored_index(0, 0), Some(239));
        assert_eq!(system.stored_index(1, 0), Some(240 + 239), "one screen column right is the next stored row");
        assert_eq!(system.stored_index(0, 239), Some(0));
        // The factory test: 36 20, columns 0-319, pages 0-239: a plain
        // 320 x 240 picture, row by row.
        let test = panel(&[(0x36, &[0x20]), (0x2A, &[0, 0, 1, 0x3F]), (0x2B, &[0, 0, 0, 0xEF])]);
        assert_eq!(test.stored_index(0, 0), Some(0));
        assert_eq!(test.stored_index(5, 2), Some(2 * 320 + 5));
        // Outside a smaller window, nothing lands.
        let small = panel(&[(0x36, &[0x20]), (0x2A, &[0, 10, 0, 19]), (0x2B, &[0, 0, 0, 9])]);
        assert_eq!(small.stored_index(10, 0), Some(0));
        assert_eq!(small.stored_index(0, 0), None);
    }

    #[test]
    fn timers_count_and_the_uart_keeps_text() {
        // mov r0,#0xC0000000; orr r0,#0x20000; ldr r1,[r0,#0x10]; b .
        let mut t = Twist::new(&words(&[0xE3A0_0103, 0xE380_0802, 0xE590_1010, 0xEAFF_FFFE]));
        t.bus.now = CLOCK / 1000; // 1 ms in: 1000 counts
        t.run(3);
        assert_eq!(t.cpu.r[1], 1000);
        t.bus.write(0xC006_0000, b'A' as u32, 1);
        assert_eq!(t.bus.uart, b"A");
        // An unmodelled register reads back what was written, and is listed.
        t.bus.write(0xD012_3450, 7, 4);
        assert_eq!(t.bus.read(0xD012_3450, 4), 7);
        assert!(t.bus.unmodelled.contains_key(&0xD012_3450));
        assert_eq!(t.bus.read(0xD000_003C, 4), 1);
    }

    #[test]
    fn a_save_state_round_trips_and_keeps_the_storage_image() {
        let mut t = Twist::new(&[1, 2, 3, 4]);
        t.bus.ram[0x10_0000..0x10_0004].copy_from_slice(&[9, 8, 7, 6]);
        t.bus.emmc = Some(crate::sd::Card::emmc(vec![0x55; 4096], 64));
        t.bus.emmc.as_mut().unwrap().write_sector(3, &[0xAA; 512]);
        t.cpu.r[5] = 0x1234;
        let state = t.save_state();
        assert!(state.len() < 1 << 20, "RAM's zeros packed: {} bytes", state.len());
        // Changed after saving, then loaded back.
        t.bus.ram[0x10_0000] = 0;
        t.cpu.r[5] = 0;
        t.bus.emmc.as_mut().unwrap().write_sector(4, &[0xBB; 512]);
        t.load_state(&state).unwrap();
        assert_eq!((&t.bus.ram[0x10_0000..0x10_0004], t.cpu.r[5]), (&[9u8, 8, 7, 6][..], 0x1234));
        let card = t.bus.emmc.as_ref().unwrap();
        assert_eq!((card.sector(3)[0], card.sector(4)[0], card.sector(1)[0]), (0xAA, 0x55, 0x55), "image kept, writes as saved");
        assert!(t.load_state(&[0, 0, 0, 0]).is_err());
    }
}
