//! The Zippity's memory map and devices, as far as they are known
//! (docs/zippity.md). Everything here is UNVERIFIED on hardware unless the
//! documentation says otherwise; it is what the system software needs, and
//! matches recordings of real units where noted there.

use crate::nand::Nand;
use crate::probe::Probe;
use crate::sound::{RAMP_FRAMES, Voice};
use ribbit_arm::ArmBus;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};

/// SDRAM (64 MB assumed, as in MAME's stub).
pub const RAM_SIZE: usize = 64 << 20;
pub const SRAM_BASE: u32 = 0x2000_0000;
pub const SRAM_SIZE: usize = 0x20_0000;
/// The bootloader copies code to 0xF8000000 and runs it: on-chip RAM
/// (size UNKNOWN; 64 KB assumed).
pub const IRAM_BASE: u32 = 0xF800_0000;
pub const IRAM_SIZE: usize = 0x1_0000;

/// Instructions per video field (60 a second): 27 million instructions a
/// second (the clock and memory wait states themselves are UNKNOWN). Fitted
/// to a recording of a real unit starting up: the warning screen stays up
/// while the kernel decodes the LeapFrog logo's pictures, pure CPU work
/// (its length does not change with the NAND's speed), 3.30 s from its
/// fade-in to the next screen on the recording and here (18 million gave
/// 4.20 s, 24 million 3.60 s, 36 million 2.97 s). Frame-timed screens are
/// unaffected (the kernel is mostly idle, waiting for the next frame).
/// docs/zippity.md, "Speed".
pub const FRAME: u64 = 450_000;
/// Instructions per second (FRAME a 60th of a second; a stand-in).
pub const CLOCK: u64 = FRAME * 60;
/// Instructions the power button stays held from power-on (the bootloader
/// powers off unless it is held; the kernel enters a test mode if it still is).
const POWER_HOLD: u64 = 20 * FRAME;
const IRQ_VIDEO: u32 = 27;
/// The frame commit's busy flag (0xD05001F0 bit 14; see `Bus::frame`).
const COMMIT_BUSY: u32 = 0x4000;
const IRQ_DMA: u32 = 24;
/// The timers' interrupt lines (timer 0 to 3).
const IRQ_TIMER: [u32; 4] = [25, 17, 10, 9];
const IRQ_UART: u32 = 14;
/// The 0xD0800000 engine's completion interrupt line.
const IRQ_GFX: u32 = 12;
/// The timers' clock: 32768 Hz assumed. With the kernel's reload of 0xFFE0
/// that is one interrupt per millisecond, which suits its 2000-tick
/// controller timeout. UNVERIFIED.
const TIMER_HZ: u64 = 32_768;
/// How often the radio receiver sends the controllers' state: UNKNOWN; once
/// a millisecond here (as often as the kernel's decoder runs). Once a frame
/// was too slow: while any input is held, the menus take a snapshot of the
/// packet count every other frame (0xCC18) and the controller check (0x19FDC)
/// may look a sixth of a frame later; with no packet in between it took the
/// controllers for switched off and showed the "switch them on" panel.
const RADIO_PERIOD: u64 = CLOCK / 1000;
/// The controllers' state with nothing pressed: the buttons are active-low
/// (the kernel turns 1-to-0 changes into presses; 0x3FF is all released).
pub const PAD_IDLE: u16 = 0x3FF;
/// The serial port's receive FIFO size (16 assumed, as in ARM's PL011).
const RX_FIFO: usize = 16;
/// What the analogue-to-digital converter reads: the batteries (the kernel
/// warns at 0x5BCB or less and gives up at 0x5928 or less). Full batteries'
/// reading is UNKNOWN; this is comfortably above both.
const ADC_BATTERY: u32 = 0x7000;
/// How fast a NAND DMA transfer goes (bytes a second). Fitted with the CPU
/// speed to a recording of a real unit: Mickey Says' "LOADING" screen (about
/// 40 MB read, by fits at several speeds, and the CPU's work on them) lasts
/// 6.6 s there and here; at 7.2 MB/s and 18 million instructions a second
/// it was 11.5 s.
const NAND_DMA_RATE: u64 = 15_000_000;

const SPU_WORDS: usize = 0x2000 / 4;
const PPU_WORDS: usize = 0x8000 / 4;

/// Which register array an address is in (true: the sound unit's), and
/// its index; None for other registers and addresses not word-aligned.
#[inline]
fn block(a: u32) -> Option<(bool, usize)> {
    if a & 3 != 0 {
        return None;
    }
    let s = a.wrapping_sub(0xD040_0000) as usize;
    if s < 0x2000 {
        return Some((true, s / 4));
    }
    let p = a.wrapping_sub(0xD050_0000) as usize;
    if p < 0x8000 {
        return Some((false, p / 4));
    }
    None
}

/// What each I/O register last had written. The I/O space (0xC0000000-
/// 0xDFFFFFFF) is looked up directly in 4 KB pages made when first written,
/// as registers are read many times a frame; anything else (rare) goes to a
/// map. Saved as a map of address to value (unwritten registers left out).
#[derive(Default)]
pub struct Regs {
    /// Page number (from 0xC0000000) to 1 + its index in `pages` (0: none).
    page_of: Vec<u32>,
    pages: Vec<Box<[u32; 1024]>>,
    other: HashMap<u32, u32>,
}

const IO_BASE: u32 = 0xC000_0000;
const IO_PAGES: usize = 0x2_0000;

impl Regs {
    #[inline]
    pub fn get(&self, a: u32) -> u32 {
        let page = (a.wrapping_sub(IO_BASE) >> 12) as usize;
        if page < IO_PAGES && a & 3 == 0 {
            match self.page_of.get(page) {
                Some(&i) if i != 0 => self.pages[i as usize - 1][(a as usize >> 2) & 0x3FF],
                _ => 0,
            }
        } else {
            self.other.get(&a).copied().unwrap_or(0)
        }
    }

    /// Registers are words: an address's bits 1-0 are kept apart only in
    /// the map (byte and halfword registers off a word boundary).
    pub fn insert(&mut self, a: u32, v: u32) {
        let page = (a.wrapping_sub(IO_BASE) >> 12) as usize;
        if page >= IO_PAGES || a & 3 != 0 {
            self.other.insert(a, v);
            return;
        }
        if self.page_of.is_empty() {
            self.page_of = vec![0; IO_PAGES];
        }
        if self.page_of[page] == 0 {
            self.pages.push(Box::new([0; 1024]));
            self.page_of[page] = self.pages.len() as u32;
        }
        self.pages[self.page_of[page] as usize - 1][(a as usize >> 2) & 0x3FF] = v;
    }

    fn entries(&self) -> Vec<(u32, u32)> {
        let mut all: Vec<(u32, u32)> = self.other.iter().map(|(&a, &v)| (a, v)).collect();
        for (page, &i) in self.page_of.iter().enumerate().filter(|e| *e.1 != 0) {
            let base = IO_BASE + ((page as u32) << 12);
            all.extend(self.pages[i as usize - 1].iter().enumerate().filter(|e| *e.1 != 0).map(|(k, &v)| (base + 4 * k as u32, v)));
        }
        all.sort_unstable();
        all
    }
}

impl Serialize for Regs {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_map(self.entries())
    }
}

impl<'de> Deserialize<'de> for Regs {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let map: HashMap<u32, u32> = HashMap::deserialize(d)?;
        let mut r = Regs::default();
        for (a, v) in map {
            r.insert(a, v);
        }
        Ok(r)
    }
}

#[derive(Serialize, Deserialize)]
pub struct Bus {
    pub(crate) ram: Vec<u8>,
    pub(crate) sram: Vec<u8>,
    pub(crate) iram: Vec<u8>,
    /// The instruction being run (for logs).
    #[serde(skip)]
    pub(crate) pc: u32,
    /// Instructions run (time).
    pub(crate) now: u64,
    /// What each I/O register last had written (most read it back: e.g. the
    /// interrupt controller's disable flag, 0xD0100038).
    pub(crate) regs: Regs,
    /// The sound unit's and the video chip's register blocks
    /// (0xD0400000-0xD0401FFF and 0xD0500000-0xD0507FFF), as arrays: they are
    /// read many times a sample and a frame.
    pub(crate) spu_regs: Vec<u32>,
    pub(crate) ppu_regs: Vec<u32>,
    pub(crate) nand: Nand,
    /// Text written to the serial port (the system's log).
    #[serde(skip)]
    pub uart: String,
    /// Video interrupt status (0xD050018C; write 1s to clear): bits 0 and
    /// 11 come each frame (bit 11 counts frames shown).
    pub(crate) video_status: u32,
    /// DMA completion flags (0xD03001FC; write 1s to clear), one per channel.
    pub(crate) dma_status: u32,
    /// The 0xD0800000 engine's done flag (+0x7C bit 0, write 1 to clear).
    pub(crate) gfx_done: bool,
    /// Timers 0-3 (0xC0020000 + 0x20 n): when each next overflows (0: not
    /// running), and their overflow flags (control bit 15).
    pub(crate) timer_due: [u64; 4],
    pub(crate) timer_flags: u32,
    /// Bytes received by the serial port, not yet read.
    pub(crate) rx: VecDeque<u8>,
    /// The controllers' state (active-low, 10 bits), and when the radio
    /// receiver next sends it.
    pub(crate) pad: u16,
    pub(crate) radio_due: u64,
    pub(crate) voices: [Voice; 32],
    pub(crate) spu_stopped: [u32; 2],
    /// The beat counter: ticks left of the base count, and the beat flag
    /// (+0xE14 bit 14), which is fast interrupt (FIQ) line 2.
    pub(crate) beat_base: u32,
    pub(crate) beat_flag: bool,
    /// Mixed sound (stereo, interleaved) not yet taken, and when the next
    /// output sample is due.
    #[serde(skip)]
    pub(crate) audio: Vec<i16>,
    pub(crate) audio_due: u64,
    pub(crate) audio_frac: u64,
    /// Interrupt lines need working out again (after I/O or a tick).
    #[serde(skip)]
    pub(crate) lines_dirty: bool,
    /// When the NAND controller's DMA finishes (busy until then).
    #[serde(default)]
    pub(crate) nand_busy_until: u64,
    /// Data of a NAND-to-memory transfer under way, and where it lands
    /// when the transfer ends.
    #[serde(default)]
    pub(crate) nand_pending: Option<(u32, Vec<u8>)>,
    /// The frame's picture, drawn when the kernel hands the chip the
    /// frame's sprite list (see the video DMA), and when (instructions).
    #[serde(skip)]
    pub(crate) picture: Vec<u32>,
    #[serde(skip)]
    pub(crate) picture_at: Option<u64>,
    /// Whether to draw that picture this frame (the app skips frames when
    /// it falls behind).
    #[serde(skip)]
    pub(crate) want_picture: bool,
    /// When the fade (0xD05000C0) last stepped towards the picture (see
    /// `Bus::fade`; not in states).
    #[serde(skip, default = "pending_after_load")]
    pub(crate) frame_pending: bool,
    #[serde(skip)]
    pub(crate) shown_fade: u32,
    /// CPU writes and I/O accesses so far: a loop that makes none changes
    /// nothing (see `Zippity::run`'s idle loops).
    #[serde(skip)]
    pub(crate) activity: u64,
    /// Instructions skipped in idle loops (research).
    #[serde(skip)]
    pub(crate) skipped: u64,
    #[serde(skip)]
    pub probe: Probe,
}

/// A state loaded mid-frame: let the next field finish a frame (states do
/// not record whether one was in flight).
fn pending_after_load() -> bool {
    true
}

impl Bus {
    pub fn new(flash: Vec<u8>) -> Self {
        Self {
            ram: vec![0; RAM_SIZE],
            sram: vec![0; SRAM_SIZE],
            iram: vec![0; IRAM_SIZE],
            pc: 0,
            now: 0,
            regs: Regs::default(),
            spu_regs: vec![0; SPU_WORDS],
            ppu_regs: vec![0; PPU_WORDS],
            nand: Nand::new(flash),
            uart: String::new(),
            video_status: 0,
            dma_status: 0,
            gfx_done: false,
            timer_due: [0; 4],
            timer_flags: 0,
            rx: VecDeque::new(),
            pad: PAD_IDLE,
            radio_due: 0,
            voices: [Voice::default(); 32],
            spu_stopped: [0; 2],
            beat_base: 0,
            beat_flag: false,
            audio: Vec::new(),
            audio_due: 0,
            audio_frac: 0,
            lines_dirty: true,
            nand_busy_until: 0,
            nand_pending: None,
            picture: Vec::new(),
            picture_at: None,
            want_picture: true,
            frame_pending: false,
            shown_fade: 0,
            activity: 0,
            skipped: 0,
            probe: Probe::default(),
        }
    }

    pub(crate) fn latch(&self, a: u32) -> u32 {
        match block(a) {
            Some((true, i)) => self.spu_regs[i],
            Some((false, i)) => self.ppu_regs[i],
            None => self.regs.get(a),
        }
    }

    pub(crate) fn set_latch(&mut self, a: u32, v: u32) {
        match block(a) {
            Some((true, i)) => self.spu_regs[i] = v,
            Some((false, i)) => self.ppu_regs[i] = v,
            None => {
                self.regs.insert(a, v);
            }
        }
    }

    /// Interrupt lines raised (bit n for line n).
    fn raised(&self) -> u64 {
        let mut lines = 0u64;
        if self.video_status != 0 {
            lines |= 1 << IRQ_VIDEO;
        }
        if self.dma_status != 0 {
            lines |= 1 << IRQ_DMA;
        }
        if self.gfx_done {
            lines |= 1 << IRQ_GFX;
        }
        for (n, line) in IRQ_TIMER.iter().enumerate() {
            if self.timer_flags & (1 << n) != 0 {
                lines |= 1 << line;
            }
        }
        // Serial receive interrupts enabled by control bits 6/5.
        if !self.rx.is_empty() && self.latch(0xC006_0008) & 0x60 != 0 {
            lines |= 1 << IRQ_UART;
        }
        lines
    }

    /// The lowest raised line not masked (0xD0100030: bit 32-n masks line n).
    fn pending_irq(&self) -> u32 {
        let (raised, mask) = (self.raised(), self.latch(0xD010_0030));
        // Line n is unmasked when mask bit 32 - n is clear: reversed, bit n.
        let unmasked = ((!mask).reverse_bits() as u64) << 1;
        let pending = raised & unmasked & 0x1_FFFF_FFFE;
        if pending == 0 { 0 } else { pending.trailing_zeros() }
    }

    /// The pending fast interrupt's number (1-4), 0 if none: 0xD0100034
    /// bit 4-n masks line n. Only the sound unit's beat counter (line 2)
    /// is modelled.
    fn pending_fiq(&self) -> u32 {
        if self.beat_flag && self.latch(0xD010_0034) & (1 << 2) == 0 { 2 } else { 0 }
    }

    /// The CPU's IRQ and FIQ inputs: a pending line, with interrupts not
    /// disabled at the controller (0xD0100038 bit 0, which holds off both:
    /// the FIQ handler returns at once while it is set).
    pub(crate) fn lines(&self) -> (bool, bool) {
        let enabled = self.latch(0xD010_0038) & 1 == 0;
        (enabled && self.pending_irq() != 0, enabled && self.pending_fiq() != 0)
    }

    /// A video field: the frame interrupt's bits 0 and 11, and the frame
    /// commit's busy flag (0xD05001F0 bit 14) running out.
    pub(crate) fn frame(&mut self) {
        // Bit 0 (vertical blank) and bit 11 (frame shown) come every field,
        // 60 a second. The kernel counts bit 11 as the time: its timers add
        // up the fields counted and divide by 60 (50 on a PAL unit, the
        // strap at 0xC0000000 bit 6; code at 0x20A70), so the count must
        // run at the field rate. Its main loop (0x2CFF0) waits for the bit 0
        // handler's flag, sends the frame and commits it (writes 1 to
        // 0xD05001F0), then waits for the next bit 11. The bit 0 handler
        // (0x35EA8) skips the flag while the commit is busy (bit 14), so a
        // commit in one field holds the loop until the field after next: 30
        // loops a second, each seeing two fields counted. Without the busy
        // flag the loop ran 60 a second and everything went twice as fast;
        // with bit 11 every second field instead (as before) the count ran
        // at half the time, and the start-up's LeapFrog logo stepped every
        // 10 fields and stood still 0.6 s, against about 6 and 1.1 s on a
        // recording of a real unit (now 4-6 and 1.0 s). How long the busy
        // flag really lasts is UNKNOWN.
        //
        // Bit 0 comes only while a committed frame is on its way: at each
        // field from the commit to the field where it comes up (the busy
        // flag clear), and then the fade (0xD05000C0) that comes up with it
        // is taken. Not at all while the kernel sends no frames (loading).
        // The bit 0 handler (0x35EA8) rotates the three frame buffers each
        // time it finds the commit done, which only makes sense once per
        // frame; and it steps the fade (0x2E410), so fades wait for frames:
        // the start-up warning's fade-in, started 17 fields before its first
        // frame, then runs over the warning as on a recording of a real unit
        // (before, it ran over black and the warning popped up), a mat
        // picture's fade-in no longer shows the screen before it, and
        // LOADING, shown just before a program is read and then left without
        // frames while the fade is set to black, stays up as on the
        // recording (before, a 16-field timeout was needed).
        self.video_status |= 0x800;
        let commit = self.latch(0xD050_01F0);
        if commit & COMMIT_BUSY != 0 {
            // Bit 31 (not readable) marks the first field seen.
            let next = if commit & 0x8000_0000 != 0 { commit & !(0x8000_0000 | COMMIT_BUSY) } else { commit | 0x8000_0000 };
            self.set_latch(0xD050_01F0, next);
        }
        if self.frame_pending {
            self.video_status |= 1;
            if self.latch(0xD050_01F0) & COMMIT_BUSY == 0 {
                self.frame_pending = false;
                self.shown_fade = self.latch(0xD050_00C0) & 0xFF;
            }
        }
        self.lines_dirty = true;
    }

    /// Runs the timers, the sound unit and the radio receiver up to now.
    /// Called every few hundred instructions.
    pub(crate) fn tick(&mut self) {
        if self.nand_pending.is_some() {
            self.finish_nand_dma(false);
        }
        self.lines_dirty = true;
        for n in 0..4 {
            let ctrl = self.latch(0xC002_0000 + 0x20 * n as u32);
            if ctrl & 0x6000 != 0x6000 {
                self.timer_due[n] = 0;
                continue;
            }
            let count = 0x1_0000 - (self.latch(0xC002_0008 + 0x20 * n as u32) & 0xFFFF) as u64;
            let period = (count * CLOCK / TIMER_HZ).max(1);
            if self.timer_due[n] == 0 {
                self.timer_due[n] = self.now + period;
            } else if self.now >= self.timer_due[n] {
                self.timer_flags |= 1 << n;
                self.timer_due[n] += period;
            }
        }
        while self.now >= self.audio_due {
            // CLOCK / 70312.5 instructions per output sample (exact on
            // average: audio_frac keeps the remainder in 140625ths).
            self.audio_frac += CLOCK * 2;
            self.audio_due += self.audio_frac / (crate::sound::SPU_CLOCK / 2);
            self.audio_frac %= crate::sound::SPU_CLOCK / 2;
            // One beat tick per output sample (both are SPU_CLOCK / 4).
            self.beat_tick();
            let (l, r) = self.spu_mix();
            if self.audio.len() < 1 << 24 {
                self.audio.extend_from_slice(&[l.clamp(-32768, 32767) as i16, r.clamp(-32768, 32767) as i16]);
            }
        }
        // The radio receiver (the mat and joystick are wireless) sends 4
        // bytes: a 16-bit word A, little-endian, then A ^ 1. The state's
        // bits 9-5 are A's bits 15-11 and bits 4-0 A's bits 7-3 (from the
        // kernel's decoder at 0x2B154).
        if self.now >= self.radio_due {
            self.radio_due = self.now + self.probe.radio_period.unwrap_or(RADIO_PERIOD);
            let now = self.now;
            let state = self.probe.pad_script.iter().find(|p| (p.1..p.2).contains(&now)).map_or(self.pad, |p| p.0);
            let a = ((state >> 5) & 31) << 11 | (state & 31) << 3;
            // A whole packet or none: a part would leave the decoder out of
            // step until it finds the packet boundary again.
            if self.rx.len() + 4 <= RX_FIFO {
                self.rx.extend([a as u8, (a >> 8) as u8, (a ^ 1) as u8, ((a ^ 1) >> 8) as u8]);
            }
        }
    }

    pub(crate) fn mem(&mut self, a: u32, n: usize) -> Option<&mut [u8]> {
        let a = a as usize;
        if a + n <= RAM_SIZE {
            return Some(&mut self.ram[a..a + n]);
        }
        let s = a.wrapping_sub(SRAM_BASE as usize);
        if s + n <= SRAM_SIZE {
            return Some(&mut self.sram[s..s + n]);
        }
        let i = a.wrapping_sub(IRAM_BASE as usize);
        if i + n <= IRAM_SIZE {
            return Some(&mut self.iram[i..i + n]);
        }
        None
    }

    /// A byte of RAM, SRAM or on-chip RAM, without side effects.
    pub(crate) fn peek8(&self, a: u32) -> u8 {
        let a = a as usize;
        if a < RAM_SIZE {
            return self.ram[a];
        }
        let s = a.wrapping_sub(SRAM_BASE as usize);
        if s < SRAM_SIZE {
            return self.sram[s];
        }
        let i = a.wrapping_sub(IRAM_BASE as usize);
        if i < IRAM_SIZE {
            return self.iram[i];
        }
        0
    }

    pub(crate) fn peek16(&self, a: u32) -> u32 {
        self.peek8(a) as u32 | (self.peek8(a.wrapping_add(1)) as u32) << 8
    }

    pub(crate) fn peek32(&self, a: u32) -> u32 {
        self.peek16(a) | self.peek16(a.wrapping_add(2)) << 16
    }

    /// The NAND controller's registers (0xD0900000); None: not one of them.
    fn nand_io(&mut self, a: u32, write: Option<u32>) -> Option<u32> {
        let off = a.checked_sub(0xD090_0000).filter(|&o| o < 0x200)?;
        if off == 0 {
            // DMA: the controller reads busy (bit 0) for as long as the
            // transfer takes (NAND_DMA_RATE); data read from the NAND
            // lands in memory when it ends (the system starts loads into
            // sprite-cache memory a screen still shows, which on hardware
            // changes only once the transfer is through).
            self.finish_nand_dma(false);
            if let Some(v) = write
                && v & 1 != 0
            {
                let (addr, len) = (self.nand.dma_addr, self.nand.dma_len);
                let to_nand = v & 2 != 0;
                if to_nand {
                    for i in 0..len {
                        let b = self.mem(addr.wrapping_add(i), 1).map_or(0xFF, |m| m[0]);
                        self.nand.write_data(b);
                    }
                } else {
                    let data = (0..len).map(|_| self.nand.read_data()).collect();
                    self.nand_pending = Some((addr, data));
                }
                self.nand_busy_until = self.now + len as u64 * CLOCK / NAND_DMA_RATE;
                let dir = if to_nand { "to NAND" } else { "from NAND" };
                self.nand.note(|| format!("DMA {dir} {len:#x} bytes at {addr:08x}"));
            }
            return Some((self.now < self.nand_busy_until) as u32);
        }
        self.finish_nand_dma(false);
        Some(self.nand.reg(off, write))
    }

    /// Lands a NAND-to-memory transfer's data once it has ended (or now,
    /// with `force`).
    pub(crate) fn finish_nand_dma(&mut self, force: bool) {
        if !force && self.now < self.nand_busy_until {
            return;
        }
        if let Some((addr, data)) = self.nand_pending.take() {
            for (i, b) in data.into_iter().enumerate() {
                if let Some(m) = self.mem(addr.wrapping_add(i as u32), 1) {
                    m[0] = b;
                }
            }
        }
    }

    /// The maths unit's arithmetic (0xF60040A0; from the cartridge games'
    /// drivers, UNVERIFIED on hardware): +0xA8 the operation, +0xA0 and
    /// +0xA4 the operands, +0xAC and +0xB0 the results. Operation 1 is a
    /// 16.16 fixed-point product (a game replaced its software routine for
    /// (a * b) >> 16 with it; +0xB0 the bits above), 3 a signed division
    /// (+0xAC the quotient, +0xB0 the remainder: the games use them for
    /// sprite scaling steps and wrapping). Others are UNKNOWN (read 0).
    fn arith(&self, second: bool) -> u32 {
        let (a, b) = (self.latch(0xF600_40A0) as i32, self.latch(0xF600_40A4) as i32);
        match self.latch(0xF600_40A8) {
            1 => {
                let p = (a as i64 * b as i64) >> 16;
                if second { (p >> 32) as u32 } else { p as u32 }
            }
            3 if second => a.checked_rem(b).unwrap_or(a) as u32,
            3 => a.checked_div(b).unwrap_or(0) as u32,
            _ => 0,
        }
    }

    pub(crate) fn io(&mut self, a: u32, write: Option<u32>, size: u32) -> u32 {
        self.lines_dirty = true;
        // Polling the NAND controller's status changes nothing: a loop that
        // only waits on it is idle (Zippity::run skips it up to the
        // transfer's end, which it then sees at the same instruction as
        // without skipping).
        if write.is_some() || !matches!(a, 0xD090_0000 | 0xD090_0140) {
            self.activity += 1;
        }
        let v = self.io_access(a, write);
        self.probe.io(self.now, self.pc, a, write, size, v);
        v
    }

    fn io_access(&mut self, a: u32, write: Option<u32>) -> u32 {
        if let Some(v) = self.nand_io(a, write) {
            return v;
        }
        let latched = self.latch(a);
        if let Some(v) = write {
            self.set_latch(a, v);
        }
        match (a, write) {
            // Sound unit control (see sound.rs): beat counter, enable, ramp
            // down, stop flags (write 1s to clear), busy.
            (0xD040_0E14, None) => (latched & !0x4000) | if self.beat_flag { 0x4000 } else { 0 },
            (0xD040_0E14, Some(v)) => {
                if v & 0x4000 != 0 {
                    self.beat_flag = false;
                }
                self.set_latch(a, v & !0x4000);
                0
            }
            (0xD040_0E00 | 0xD040_0E80, Some(v)) => {
                self.spu_enable(((a >> 7) & 1) as usize, v & 0xFFFF);
                0
            }
            (0xD040_0E28 | 0xD040_0EA8, Some(v)) => {
                let bank = ((a >> 7) & 1) as usize;
                let busy = (0..16).filter(|&i| self.voices[bank * 16 + i].playing).fold(0u32, |m, i| m | 1 << i);
                let new = v & 0xFFFF & busy;
                for i in 0..16 {
                    if (new & !latched) & (1 << i) != 0 {
                        let ch = bank * 16 + i;
                        let clock = self.latch(0xD040_1800 + 0x40 * ch as u32 + 0xC) & 7;
                        self.voices[ch].ramp_frame = RAMP_FRAMES[clock as usize];
                    }
                }
                self.set_latch(a, new);
                0
            }
            (0xD040_0E2C | 0xD040_0EAC, None) => self.spu_stopped[((a >> 7) & 1) as usize],
            (0xD040_0E2C | 0xD040_0EAC, Some(v)) => {
                self.spu_stopped[((a >> 7) & 1) as usize] &= !v;
                0
            }
            (0xD040_0E3C | 0xD040_0EBC, None) => {
                let bank = ((a >> 7) & 1) as usize;
                (0..16).filter(|&i| self.voices[bank * 16 + i].playing).fold(0, |m, i| m | 1 << i)
            }
            // Timers (0xC0020000 + 0x20 n): +0 control (bits 14 and 13 on,
            // bit 15 the overflow flag, cleared by writing 1), +8 reload
            // (counts up from it to 0x10000).
            (0xC002_0000 | 0xC002_0020 | 0xC002_0040 | 0xC002_0060, None) => {
                latched | (self.timer_flags >> ((a >> 5) & 3) & 1) << 15
            }
            (0xC002_0000 | 0xC002_0020 | 0xC002_0040 | 0xC002_0060, Some(v)) => {
                if v & 0x8000 != 0 {
                    self.timer_flags &= !(1 << ((a >> 5) & 3));
                }
                self.set_latch(a, v & !0x8000);
                0
            }
            // Serial port (like ARM's PL011): +0 data (reads take a received
            // byte), +8 control (bits 6/5 receive interrupts), +0x10 status:
            // bit 4 receive FIFO empty, bit 5 transmit FIFO full (never),
            // bit 15 receive interrupt.
            (0xC006_0000, None) => self.rx.pop_front().unwrap_or(0) as u32,
            (0xC006_0010, None) => {
                if self.rx.is_empty() {
                    0x10
                } else {
                    0x8000
                }
            }
            (0xC006_0000, Some(v)) => {
                if self.uart.len() < 1 << 20 {
                    self.uart.push(v as u8 as char);
                }
                0
            }
            // Video DMA (0xD05001C0 source, 0x1C4 destination, 0x1C8 words
            // minus 1): copies at once; the count reads 0 when done.
            (0xD050_01C8, Some(v)) => {
                let (src, dst) = (self.latch(0xD050_01C0), self.latch(0xD050_01C4));
                for i in 0..=v {
                    let w = self.read32(src.wrapping_add(4 * i));
                    self.write32(dst.wrapping_add(4 * i), w);
                }
                self.set_latch(a, 0);
                // The sprite list (copied after the palette, at the start of
                // each frame): the chip draws the frame from this state, into
                // a frame buffer the TV shows next. Drawn now, not at the
                // frame's end, by when the kernel is already decoding the
                // next screen's pictures into the sprite cache (which showed
                // as scrambled and half-drawn sprites between screens).
                if dst == 0xD050_2000 && self.want_picture {
                    self.redraw();
                }
                0
            }
            // DMA controller, 4 channels at 0xD0300000 + 0x40 * n: +0 control
            // (bit 0 start, bit 1 busy, bit 7 source fixed (a fill), bit 8
            // interrupt?, bits 13-12 unit: 0 bytes, 1 halfwords), +4 source,
            // +8 destination, +C count of units.
            (0xD030_0000 | 0xD030_0040 | 0xD030_0080 | 0xD030_00C0, Some(v)) if v & 1 != 0 => {
                let (src, dst, count) = (self.latch(a + 4), self.latch(a + 8), self.latch(a + 12));
                let fixed = v & 0x80 != 0;
                let bytes = count << ((v >> 12) & 3);
                for i in 0..bytes {
                    let b = self.read8(if fixed { src } else { src.wrapping_add(i) });
                    self.write8(dst.wrapping_add(i), b);
                }
                self.dma_status |= 1 << ((a >> 6) & 3);
                self.set_latch(a, v & !3);
                0
            }
            (0xD030_01FC, None) => self.dma_status,
            (0xD030_01FC, Some(v)) => {
                self.dma_status &= !v;
                0
            }
            // 0xD0800000 engine (given the frame being drawn; a scaler or TV
            // encoder?): operations finish at once and do nothing.
            (0xD080_0000, Some(v)) if v & 1 != 0 => {
                self.probe.gfx_ops += 1;
                self.gfx_done = true;
                0
            }
            (0xD080_007C, None) => self.gfx_done as u32,
            (0xD080_007C, Some(v)) => {
                if v & 1 != 0 {
                    self.gfx_done = false;
                }
                0
            }
            // GPIO 0xC0000108 bits 5-4 select the NAND chip (0: the system
            // NAND; the bootloader tries 1 and 2 for a cartridge).
            (0xC000_0108, Some(v)) => {
                self.nand.select = (v >> 4) & 3;
                0
            }
            // GPIO inputs (0xC0000000 port A, +0x20 B, +0x40 C). Port C bit
            // 10 is the power button (low: held), held at power-on; port A
            // bit 6 the 50/60 Hz strap (high: 60), bit 4 a cartridge in the
            // slot (high; the test mode's "Cart is seated"). Other idle
            // levels UNVERIFIED.
            (0xC000_0000 | 0xC000_0020 | 0xC000_0040, None) => {
                let idle = match a {
                    0xC000_0000 if !self.nand.cart.is_empty() => 0xFFFF,
                    0xC000_0000 => 0xFFEF,
                    0xC000_0020 => 0xFFFF,
                    _ if self.now < self.probe.power_hold.unwrap_or(POWER_HOLD) => 0xFBFF,
                    _ => 0xFFFF,
                };
                let now = self.now;
                self.probe.presses.iter().filter(|p| p.0 == a && (p.2..p.3).contains(&now)).fold(idle, |v, p| v ^ p.1)
            }
            // Interrupt controller: the pending IRQ's and FIQ's numbers.
            (0xD010_0028, None) => self.pending_irq(),
            (0xD010_002C, None) => self.pending_fiq(),
            // Analogue-to-digital converter (0xC00C0000): +4 conversion: bit
            // 6 starts one, bit 15 done, bits 5 and 4 errors, all three
            // cleared by writing 1; +8 the result. Conversions finish at
            // once and read the batteries.
            (0xC00C_0004, Some(v)) => {
                let mut r = latched & !(v & 0x8030) | (v & !0x8070);
                if v & 0x40 != 0 {
                    r |= 0x8000;
                }
                self.set_latch(a, r);
                0
            }
            (0xC00C_0008, None) => ADC_BATTERY,
            // Sprite corner converter: four corners in (x at +0x300/+0x308/
            // +0x310/+0x318, y at +0x304/..), five results out (+0x320-
            // +0x330) that the kernel stores in a sprite's halfwords 1, 2, 5,
            // 6 and 7. What the hardware returns is UNKNOWN: this gives its
            // own encoding, which only this crate's renderer reads (the first
            // and third corners, and 0x8000).
            (0xD050_0320, None) => self.latch(0xD050_0300) & 0xFFFF,
            (0xD050_0324, None) => self.latch(0xD050_0304) & 0xFFFF,
            (0xD050_0328, None) => self.latch(0xD050_0310) & 0xFFFF,
            (0xD050_032C, None) => self.latch(0xD050_0314) & 0xFFFF,
            (0xD050_0330, None) => 0x8000,
            // The fixed-point maths unit (0xF6004000): its matrix product
            // (`geometry`, video.rs) and arithmetic (`arith`). Results are
            // ready at once: the kernels read them straight after starting.
            (0xF680_0000 | 0xF680_0004, Some(_)) => {
                self.geometry(a);
                0
            }
            (0xF600_40AC | 0xF600_40B0, None) => self.arith(a == 0xF600_40B0),
            // Frame commit (see `frame`): writing bit 0 sends the frame
            // and sets the busy flag (bit 14) until the field after next;
            // a commit while busy (the bit 0 handler repeats it) changes
            // nothing.
            (0xD050_01F0, Some(v)) => {
                if latched & COMMIT_BUSY != 0 {
                    self.set_latch(a, latched);
                } else if v & 1 != 0 {
                    self.set_latch(a, v | COMMIT_BUSY);
                    self.frame_pending = true;
                }
                0
            }
            (0xD050_01F0, None) => latched & !0x8000_0000,
            (0xD050_018C, None) => self.video_status,
            (0xD050_018C, Some(v)) => {
                self.video_status &= !v;
                0
            }
            (_, None) => latched,
            _ => 0,
        }
    }
}

impl ArmBus for Bus {
    fn read8(&mut self, a: u32) -> u8 {
        match self.mem(a, 1) {
            Some(m) => m[0],
            None => self.io(a, None, 8) as u8,
        }
    }
    fn read16(&mut self, a: u32) -> u16 {
        match self.mem(a, 2) {
            Some(m) => u16::from_le_bytes([m[0], m[1]]),
            None => self.io(a, None, 16) as u16,
        }
    }
    fn read32(&mut self, a: u32) -> u32 {
        match self.mem(a, 4) {
            Some(m) => u32::from_le_bytes([m[0], m[1], m[2], m[3]]),
            None => self.io(a, None, 32),
        }
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.probe.mem_write(self.now, self.pc, a, v as u32, 8);
        self.activity += 1;
        match self.mem(a, 1) {
            Some(m) => m[0] = v,
            None => {
                self.io(a, Some(v as u32), 8);
            }
        }
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.probe.mem_write(self.now, self.pc, a, v as u32, 16);
        self.activity += 1;
        match self.mem(a, 2) {
            Some(m) => m.copy_from_slice(&v.to_le_bytes()),
            None => {
                self.io(a, Some(v as u32), 16);
            }
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.probe.mem_write(self.now, self.pc, a, v, 32);
        self.activity += 1;
        match self.mem(a, 4) {
            Some(m) => m.copy_from_slice(&v.to_le_bytes()),
            None => {
                self.io(a, Some(v), 32);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: u32 = 0x1_0000;

    fn fixed(x: f64) -> u32 {
        (x * 65536.0) as i32 as u32
    }

    /// A 3 x 3 transform as Disney Princess's Dress Dash loads it (rows four
    /// words apart, translation in the last row) moves a point.
    #[test]
    fn maths_unit_matrix() {
        let mut b = Bus::new(Vec::new());
        let m = [fixed(0.6), 0, 0, 0, 0, fixed(0.6), 0, 0, 120 << 16, 60 << 16, ONE, 0];
        for (i, &v) in m.iter().enumerate() {
            b.write32(0xF600_4000 + 4 * i as u32, v);
        }
        for (i, v) in [10 << 16, 48 << 16, ONE].into_iter().enumerate() {
            b.write32(0xF600_4040 + 4 * i as u32, v);
        }
        b.write32(0xF680_0004, 1);
        let r: Vec<i32> = (0..3).map(|i| b.read32(0xF600_4060 + 4 * i) as i32).collect();
        // x' = 0.6 x + 120, y' = 0.6 y + 60 (0x9999 is just under 0.6).
        assert_eq!(r[0], 10 * 0x9999 + (120 << 16));
        assert_eq!(r[1], 48 * 0x9999 + (60 << 16));
        assert_eq!(r[2], ONE as i32);
        // 4 x 4: the identity leaves a vector as it is.
        for i in 0..16 {
            b.write32(0xF600_4000 + 4 * i, if i % 5 == 0 { ONE } else { 0 });
        }
        b.write32(0xF600_404C, fixed(-2.5));
        b.write32(0xF680_0000, 1);
        assert_eq!(b.read32(0xF600_406C), fixed(-2.5));
        assert_eq!(b.read32(0xF600_4060), 10 << 16);
    }

    #[test]
    fn maths_unit_arithmetic() {
        let mut b = Bus::new(Vec::new());
        let mut op = |mode: u32, a: i32, x: i32| {
            b.write32(0xF600_40A8, mode);
            b.write32(0xF600_40A0, a as u32);
            b.write32(0xF600_40A4, x as u32);
            (b.read32(0xF600_40AC) as i32, b.read32(0xF600_40B0) as i32)
        };
        assert_eq!(op(1, fixed(1.5) as i32, fixed(-2.0) as i32).0, fixed(-3.0) as i32);
        assert_eq!(op(3, -17, 5), (-3, -2), "C's signed division");
        assert_eq!(op(3, 7, 0).0, 0, "no answer for a division by zero (UNKNOWN on hardware)");
    }
}
