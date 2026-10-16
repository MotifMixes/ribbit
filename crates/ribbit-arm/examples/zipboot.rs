//! Research: runs the Zippity bootloader on the ARM9 model and logs the
//! unknown hardware it touches.
//!
//! zipboot <nand.bin> [instructions]
//!
//! The NAND image is the raw dump with spare bytes (pages of 0x800 + 0x40,
//! Micron MT29F2G08AACWP, ID 2C DA 80 15). As MAME's gpl32612 stub does, the
//! boot ROM is replaced by a copy of data 0xA0000..0x1F0000 (spare bytes
//! stripped) to address 0 and a jump to 0.
//!
//! The NAND controller at 0xD0900000 is modelled from its driver in the
//! bootloader (UNVERIFIED beyond what the driver does):
//!   +0x000 DMA control: bit 0 start/busy, bit 1 memory to NAND
//!   +0x004 DMA memory address, +0x008 DMA length (bytes)
//!   +0x140 status: bit 15 ready
//!   +0x144 command (a NAND command byte: FF reset, 90 ID, 00/30 read,
//!          80/10 program, 60/D0 erase, 70 status)
//!   +0x148 column (or, for erase, row bits 15-0)
//!   +0x14C row (or, for erase, row bits 31-16)
//!   +0x150 data port (a byte per access)
//!   +0x154 saved and restored around Read ID (chip select? UNKNOWN)
//!   +0x158..0x164 ECC engine (control, sector size, error bits, count);
//!          reads 0: no errors

use ribbit_arm::{Arm, ArmBus};
use std::collections::BTreeMap;

const RAM_SIZE: usize = 64 << 20;
const SRAM_BASE: u32 = 0x2000_0000;
const SRAM_SIZE: usize = 0x20_0000;
/// The bootloader copies code to 0xF8000000 and runs it: on-chip RAM
/// (size UNKNOWN; 64 KB assumed).
const IRAM_BASE: u32 = 0xF800_0000;
const IRAM_SIZE: usize = 0x1_0000;

const PAGE: usize = 0x840;
const PAGE_DATA: usize = 0x800;
const PAGES_PER_BLOCK: usize = 64;
const NAND_ID: [u8; 5] = [0x2C, 0xDA, 0x80, 0x15, 0x50];

#[derive(PartialEq, Clone, Copy, serde::Serialize, serde::Deserialize)]
enum Port {
    Idle,
    Id,
    Status,
    Page,
    Program,
}

struct Nand {
    flash: Vec<u8>,
    port: Port,
    column: u32,
    row: u32,
    /// The page register (read: loaded from flash; program: filled by writes).
    buf: Vec<u8>,
    cursor: usize,
    dma_addr: u32,
    dma_len: u32,
    regs: BTreeMap<u32, u32>,
    log: Vec<String>,
}

/// The NAND controller in a snapshot: port, column, row, page register,
/// cursor, DMA address and length, other registers.
type NandState = (Port, u32, u32, Vec<u8>, usize, u32, u32, Vec<(u32, u32)>);

/// Everything needed to carry on from a point in a run (ZIPBOOT_SAVE,
/// ZIPBOOT_LOAD), apart from the flash itself.
#[derive(serde::Serialize, serde::Deserialize)]
struct Snapshot {
    cpu: Arm,
    ran: u64,
    ram: Vec<u8>,
    sram: Vec<u8>,
    iram: Vec<u8>,
    io: Vec<(u32, u32)>,
    nand: NandState,
    video_status: u32,
    dma_status: u32,
    gfx_done: bool,
    timer_due: [u64; 4],
    timer_flags: u32,
    rx: Vec<u8>,
    radio_due: u64,
    voices: [Voice; 32],
    spu_stopped: [u32; 2],
}

impl Snapshot {
    fn take(cpu: &Arm, bus: &Bus, ran: u64) -> Self {
        let n = &bus.nand;
        Self {
            cpu: cpu.clone(),
            ran,
            ram: bus.ram.clone(),
            sram: bus.sram.clone(),
            iram: bus.iram.clone(),
            io: bus.io.iter().map(|(a, e)| (*a, e.last_write)).collect(),
            nand: (n.port, n.column, n.row, n.buf.clone(), n.cursor, n.dma_addr, n.dma_len, n.regs.iter().map(|(a, v)| (*a, *v)).collect()),
            video_status: bus.video_status,
            dma_status: bus.dma_status,
            gfx_done: bus.gfx_done,
            timer_due: bus.timer_due,
            timer_flags: bus.timer_flags,
            rx: bus.rx.iter().copied().collect(),
            radio_due: bus.radio_due,
            voices: bus.voices,
            spu_stopped: bus.spu_stopped,
        }
    }

    fn restore(self, cpu: &mut Arm, bus: &mut Bus) -> u64 {
        *cpu = self.cpu;
        bus.ram = self.ram;
        bus.sram = self.sram;
        bus.iram = self.iram;
        for (a, v) in self.io {
            bus.io.insert(a, Access { last_write: v, ..Default::default() });
        }
        let n = &mut bus.nand;
        (n.port, n.column, n.row, n.buf, n.cursor, n.dma_addr, n.dma_len) =
            (self.nand.0, self.nand.1, self.nand.2, self.nand.3, self.nand.4, self.nand.5, self.nand.6);
        n.regs = self.nand.7.into_iter().collect();
        bus.video_status = self.video_status;
        bus.dma_status = self.dma_status;
        bus.gfx_done = self.gfx_done;
        bus.timer_due = self.timer_due;
        bus.timer_flags = self.timer_flags;
        bus.rx = self.rx.into_iter().collect();
        bus.radio_due = self.radio_due;
        bus.voices = self.voices;
        bus.spu_stopped = self.spu_stopped;
        bus.audio_due = self.ran;
        self.ran
    }
}

impl Nand {
    fn note(&mut self, line: String) {
        if self.log.len() < 3000 {
            self.log.push(line);
        }
    }

    fn load(&mut self) {
        let at = self.row as usize * PAGE;
        self.buf = self.flash.get(at..at + PAGE).map_or_else(|| vec![0xFF; PAGE], <[u8]>::to_vec);
        self.cursor = self.column as usize;
        self.port = Port::Page;
        self.note(format!("read page {:#07x} col {:#x}", self.row, self.column));
    }

    fn command(&mut self, c: u32) {
        match c {
            0xFF => self.port = Port::Idle,
            0x90 => {
                self.port = Port::Id;
                self.cursor = 0;
            }
            0x70 => self.port = Port::Status,
            0x00 | 0x50 => self.port = Port::Idle,
            0x30 => self.load(),
            0x80 => {
                self.buf = vec![0xFF; PAGE];
                self.cursor = 0;
                self.port = Port::Program;
            }
            0x10 => {
                let at = self.row as usize * PAGE;
                for (i, b) in self.buf.iter().enumerate() {
                    if let Some(f) = self.flash.get_mut(at + i) {
                        *f &= b;
                    }
                }
                self.note(format!("program page {:#07x}", self.row));
                self.port = Port::Idle;
            }
            0x60 => {}
            0xD0 => {
                let row = ((self.row << 16) | self.column) as usize;
                let start = row / PAGES_PER_BLOCK * PAGES_PER_BLOCK * PAGE;
                if let Some(b) = self.flash.get_mut(start..start + PAGES_PER_BLOCK * PAGE) {
                    b.fill(0xFF);
                }
                self.note(format!("erase block of row {row:#07x}"));
            }
            _ => self.note(format!("NAND command {c:#04x}?")),
        }
    }

    fn read_data(&mut self) -> u8 {
        match self.port {
            Port::Id => {
                let v = NAND_ID.get(self.cursor).copied().unwrap_or(0);
                self.cursor += 1;
                v
            }
            // Ready, not write protected, no failure.
            Port::Status => 0xE0,
            Port::Page => {
                let v = self.buf.get(self.cursor).copied().unwrap_or(0xFF);
                self.cursor += 1;
                v
            }
            _ => 0xFF,
        }
    }

    fn write_data(&mut self, v: u8) {
        if self.port == Port::Program {
            let at = self.column as usize + self.cursor;
            if let Some(b) = self.buf.get_mut(at) {
                *b = v;
            }
            self.cursor += 1;
        }
    }
}

#[derive(Default)]
struct Access {
    reads: u64,
    writes: u64,
    last_write: u32,
    first_pc: u32,
}

struct Bus {
    ram: Vec<u8>,
    sram: Vec<u8>,
    iram: Vec<u8>,
    pc: u32,
    /// Instructions run (a stand-in for time).
    now: u64,
    io: BTreeMap<u32, Access>,
    log: Vec<String>,
    /// Text written to the serial port (0xC0060000).
    uart: String,
    nand: Nand,
    /// Video interrupt status (0xD050018C; write 1s to clear): bits 0 and
    /// 11 come each frame (bit 11 counts frames shown, at the kernel's +0x70).
    video_status: u32,
    /// DMA completion flags (0xD03001FC; write 1s to clear), one per channel.
    dma_status: u32,
    /// 0xD0800000 engine (2D graphics? UNKNOWN): operations started, and
    /// the done flag (+0x7C bit 0, write 1 to clear).
    gfx_ops: u64,
    gfx_done: bool,
    /// ZIPBOOT_WATCH=lo-hi (hex): every access in that range is logged.
    watch: Option<(u32, u32)>,
    /// Timers 0-3 (0xC0020000 + 0x20 n): when each next overflows (0: not
    /// running), and their overflow flags (control bit 15).
    timer_due: [u64; 4],
    timer_flags: u32,
    /// Bytes received by the serial port, not yet read.
    rx: std::collections::VecDeque<u8>,
    /// The controllers' state (10 bits) over time: (state, from, to), from
    /// ZIPBOOT_PAD=state:from:to[;...] (hex state, instruction counts).
    pad: Vec<(u16, u64, u64)>,
    /// When the radio receiver next sends a packet.
    radio_due: u64,
    /// The sound unit's channels, their stop flags (per bank of 16), the
    /// mixed output (ZIPBOOT_WAV) and when the next output sample is due.
    voices: [Voice; 32],
    spu_stopped: [u32; 2],
    audio: Vec<i16>,
    audio_due: u64,
    /// ZIPBOOT_PRESS=port:mask:from:to[;...] (port a/b/c, hex mask,
    /// instruction counts): those input bits are flipped from their idle
    /// level in that window, for finding what the inputs do.
    presses: Vec<(u32, u32, u64, u64)>,
    /// ZIPBOOT_WATCH_FROM=n: watching starts after n instructions.
    watch_from: u64,
    watched: Vec<String>,
    /// ZIPBOOT_RWATCH=lo-hi (hex): CPU writes to memory in that range are logged.
    rwatch: Option<(u32, u32)>,
    rwatched: Vec<String>,
}

/// Instructions between video frames (a stand-in: the CPU clock and the
/// frame rate are UNKNOWN; about 60 Hz at 60 MIPS).
const FRAME: u64 = 1_000_000;
/// Instructions the power button stays held from power-on.
const POWER_HOLD: u64 = 20_000_000;
/// The video interrupt's line on the interrupt controller.
const IRQ_VIDEO: u32 = 27;
/// The DMA controller's completion interrupt line.
const IRQ_DMA: u32 = 24;
/// The timers' interrupt lines (timer 0 to 3).
const IRQ_TIMER: [u32; 4] = [25, 17, 10, 9];
/// The serial port's interrupt line.
const IRQ_UART: u32 = 14;
/// Instructions per second (FRAME a 60th of a second; a stand-in).
const CLOCK: u64 = FRAME * 60;
/// The timers' clock: 32768 Hz assumed. With the kernel's reload of 0xFFE0
/// that is one interrupt per millisecond, which suits its 2000-tick
/// controller timeout. UNVERIFIED.
const TIMER_HZ: u64 = 32_768;
/// How often the radio receiver sends the controllers' state: UNKNOWN; once
/// a frame here.
const RADIO_PERIOD: u64 = FRAME;
/// The controllers' state with nothing pressed: the buttons are active-low
/// (the kernel turns 1-to-0 changes into presses; 0x3FF is all released).
const PAD_IDLE: u16 = 0x3FF;
/// The serial port's receive FIFO size (16 assumed, as in ARM's PL011).
const RX_FIFO: usize = 16;
/// The 0xD0800000 engine's completion interrupt line.
const IRQ_GFX: u32 = 12;
/// What the analogue-to-digital converter reads: the batteries (the kernel
/// warns at 0x5BCB or less and gives up at 0x5928 or less). Full batteries'
/// reading is UNKNOWN; this is comfortably above both.
const ADC_BATTERY: u32 = 0x7000;

impl Bus {
    fn latch(&self, a: u32) -> u32 {
        self.io.get(&a).map_or(0, |e| e.last_write)
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
        let mask = self.latch(0xD010_0030);
        (1..=32).find(|&n| self.raised() & (1 << n) != 0 && mask & (1u32 << (32 - n)) == 0).unwrap_or(0)
    }

    /// Runs the timers and the radio receiver up to now. Called every few
    /// hundred instructions.
    fn tick(&mut self) {
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
        // The sound unit, one output sample at a time.
        while self.now >= self.audio_due {
            self.audio_due += CLOCK / MIX_RATE;
            let (l, r) = self.spu_mix();
            if self.audio.len() < 200_000_000 {
                self.audio.extend_from_slice(&[l as i16, r as i16]);
            }
        }
        // The radio receiver (the mat and joystick are wireless) sends 4
        // bytes: a 16-bit word A, little-endian, then A ^ 1. The state's
        // bits 9-5 are A's bits 15-11 and bits 4-0 A's bits 7-3 (from the
        // kernel's decoder at 0x2B154).
        if self.now >= self.radio_due {
            self.radio_due = self.now + RADIO_PERIOD;
            let state = self.pad.iter().find(|p| (p.1..p.2).contains(&self.now)).map_or(PAD_IDLE, |p| p.0);
            let a = ((state >> 5) & 31) << 11 | (state & 31) << 3;
            for b in [a as u8, (a >> 8) as u8, (a ^ 1) as u8, ((a ^ 1) >> 8) as u8] {
                if self.rx.len() < RX_FIFO {
                    self.rx.push_back(b);
                }
            }
        }
    }

    /// The CPU's IRQ input: a pending line, interrupts not disabled at the
    /// controller (0xD0100038 bit 0).
    fn irq_line(&self) -> bool {
        self.pending_irq() != 0 && self.latch(0xD010_0038) & 1 == 0
    }

    fn note_write(&mut self, a: u32, v: u32, size: u32) {
        if let Some((lo, hi)) = self.rwatch
            && (lo..=hi).contains(&a)
            && self.rwatched.len() < 400_000
        {
            self.rwatched.push(format!("{:>10} {:08x}  W{size} {a:08x} = {v:08x}", self.now, self.pc));
        }
    }

    fn mem(&mut self, a: u32, n: usize) -> Option<&mut [u8]> {
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

    /// The NAND controller (see the top of the file); None: not its register.
    fn nand_io(&mut self, a: u32, write: Option<u32>) -> Option<u32> {
        let off = a.checked_sub(0xD090_0000).filter(|&o| o < 0x200)?;
        let n = &mut self.nand;
        let v = match (off, write) {
            (0x000, Some(v)) => {
                if v & 1 != 0 {
                    let (addr, len) = (n.dma_addr, n.dma_len);
                    let to_nand = v & 2 != 0;
                    for i in 0..len {
                        let at = addr.wrapping_add(i);
                        if to_nand {
                            let b = self.mem(at, 1).map_or(0xFF, |m| m[0]);
                            self.nand.write_data(b);
                        } else {
                            let b = self.nand.read_data();
                            if let Some(m) = self.mem(at, 1) {
                                m[0] = b;
                            }
                        }
                    }
                    let dir = if to_nand { "to NAND" } else { "from NAND" };
                    self.nand.note(format!("DMA {dir} {len:#x} bytes at {addr:08x}"));
                }
                0
            }
            (0x000, None) => 0, // DMA done at once: never busy
            (0x004, Some(v)) => {
                n.dma_addr = v;
                0
            }
            (0x008, Some(v)) => {
                n.dma_len = v;
                0
            }
            (0x140, None) => 0x8000,
            (0x144, Some(v)) => {
                n.command(v & 0xFF);
                0
            }
            (0x148, Some(v)) => {
                n.column = v;
                0
            }
            (0x14C, Some(v)) => {
                n.row = v;
                0
            }
            (0x150, None) => n.read_data() as u32,
            (0x150, Some(v)) => {
                n.write_data(v as u8);
                0
            }
            (o, Some(v)) => {
                n.regs.insert(o, v);
                0
            }
            (0x158 | 0x15C, None) => n.regs.get(&off).copied().unwrap_or(0),
            (_, None) => 0,
        };
        Some(v)
    }

    fn io(&mut self, a: u32, write: Option<u32>, size: u32) -> u32 {
        if let Some(v) = self.nand_io(a, write) {
            return v;
        }
        let pc = self.pc;
        let e = self.io.entry(a).or_insert_with(|| Access { first_pc: pc, ..Default::default() });
        let latched = e.last_write;
        let line = match write {
            Some(v) => {
                e.writes += 1;
                e.last_write = v;
                format!("{pc:08x}  W{size} {a:08x} = {v:08x}")
            }
            None => {
                e.reads += 1;
                format!("{pc:08x}  R{size} {a:08x}")
            }
        };
        if let Some((lo, hi)) = self.watch
            && self.now >= self.watch_from
            && (lo..=hi).contains(&a)
            && self.watched.len() < 400_000
        {
            self.watched.push(format!("{:>10} {line}", self.now));
        }
        // The first few accesses to each register.
        if e.reads + e.writes <= 6 && self.log.len() < 2000 {
            self.log.push(line);
        }
        match (a, write) {
            // Sound unit control (see Voice): enable, stop flags (write 1s
            // to clear), busy.
            (0xD040_0E00 | 0xD040_0E80, Some(v)) => {
                self.spu_enable(((a >> 7) & 1) as usize, v & 0xFFFF);
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
                if let Some(e) = self.io.get_mut(&a) {
                    e.last_write = v & !0x8000;
                }
                0
            }
            // Serial port (like ARM's PL011): +0 data (reads take a received
            // byte), +8 control (bits 6/5 receive interrupts), +0x10 status:
            // bit 4 receive FIFO empty, bit 5 transmit FIFO full (never),
            // bit 15 receive interrupt.
            (0xC006_0000, None) => self.rx.pop_front().unwrap_or(0) as u32,
            (0xC006_0010, None) => if self.rx.is_empty() { 0x10 } else { 0x8000 },
            (0xC006_0000, Some(v)) => {
                self.uart.push(v as u8 as char);
                0
            }
            // Video DMA (0xD05001C0 source, 0x1C4 destination, 0x1C8 words
            // minus 1): copies at once; the count reads 0 when done.
            (0xD050_01C8, Some(v)) => {
                let src = self.io.get(&0xD050_01C0).map_or(0, |e| e.last_write);
                let dst = self.io.get(&0xD050_01C4).map_or(0, |e| e.last_write);
                for i in 0..=v {
                    let w = self.read32(src.wrapping_add(4 * i));
                    self.write32(dst.wrapping_add(4 * i), w);
                }
                if let Some(e) = self.io.get_mut(&a) {
                    e.last_write = 0;
                }
                0
            }
            // DMA controller, 4 channels at 0xD0300000 + 0x40 * n: +0 control
            // (bit 0 start, bit 1 busy, bit 7 source fixed (a fill), bit 8
            // interrupt?, bits 13-12 unit: 0 bytes, 1 halfwords (the palette
            // copies use 0x1109 and flush twice the count in bytes)), +4
            // source, +8 destination, +C count of units. UNVERIFIED.
            (0xD030_0000 | 0xD030_0040 | 0xD030_0080 | 0xD030_00C0, Some(v)) if v & 1 != 0 => {
                let src = self.latch(a + 4);
                let dst = self.latch(a + 8);
                let count = self.latch(a + 12);
                let fixed = v & 0x80 != 0;
                let bytes = count << ((v >> 12) & 3);
                for i in 0..bytes {
                    let b = self.read8(if fixed { src } else { src.wrapping_add(i) });
                    self.write8(dst.wrapping_add(i), b);
                }
                self.dma_status |= 1 << ((a >> 6) & 3);
                if let Some(e) = self.io.get_mut(&a) {
                    e.last_write = v & !3;
                }
                0
            }
            // 0xD0800000 engine: +0 control (bit 0 starts an operation; 0x901
            // written), +4/+8/+C parameters, +0x7C done (bit 0). Operations
            // finish at once and draw nothing yet.
            (0xD080_0000, Some(v)) if v & 1 != 0 => {
                self.gfx_ops += 1;
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
            (0xD030_01FC, None) => self.dma_status,
            (0xD030_01FC, Some(v)) => {
                self.dma_status &= !v;
                0
            }
            // GPIO inputs (0xC0000000 port A, +0x20 B, +0x40 C; the kernel
            // logs port C as R_IOC_I_DATA). Nothing pressed: port C bit 10 is
            // the power button (low: held); A bit 6 and B bit 5 are read
            // inverted too, A bit 4 as is (meanings UNKNOWN). Idle levels
            // UNVERIFIED. The power button is held at power-on (the
            // bootloader powers off unless it is), then released (the kernel
            // enters a test mode if it is still held).
            (0xC000_0000 | 0xC000_0020 | 0xC000_0040, None) => {
                let idle = match a {
                    0xC000_0000 => 0xFFEF,
                    0xC000_0020 => 0xFFFF,
                    _ if self.now < POWER_HOLD => 0xFBFF,
                    _ => 0xFFFF,
                };
                self.presses.iter().filter(|p| p.0 == a && (p.2..p.3).contains(&self.now)).fold(idle, |v, p| v ^ p.1)
            }
            // Interrupt controller: the pending IRQ's number (0: none).
            (0xD010_0028, None) => self.pending_irq(),
            // Analogue-to-digital converter (0xC00C0000; interrupt lines 28
            // and 29 have handlers): +0 control (bit 15 on), +4 conversion:
            // bit 6 starts one, bit 15 done, bits 5 and 4 errors (the kernel
            // retries), all three cleared by writing 1; +8 the 16-bit
            // result. Conversions finish at once and read the batteries.
            (0xC00C_0004, Some(v)) => {
                let e = self.io.get_mut(&a).unwrap();
                let mut r = latched & !(v & 0x8030) | (v & !0x8070);
                if v & 0x40 != 0 {
                    r |= 0x8000;
                }
                e.last_write = r;
                0
            }
            (0xC00C_0004, None) => latched,
            (0xC00C_0008, None) => ADC_BATTERY,
            // Sprite corner converter (2.5D sprites?): the kernel writes four
            // corners (x at +0x300/+0x308/+0x310/+0x318, y at +0x304/..,
            // centred coordinates, y up) and stores five results (+0x320..
            // +0x330) in a sprite's words 1, 2, 5, 6 and 7. What the
            // hardware returns is UNKNOWN. This gives its own encoding, which
            // only this tool's renderer reads: the first corner's x and y
            // (words 1 and 2), the third corner's (words 5 and 6), and 0x8000
            // in word 7. The kernel shrinks sprites this way (a 64x64 sprite
            // sent as a single point is hidden).
            (0xD050_0320, None) => self.latch(0xD050_0300) & 0xFFFF,
            (0xD050_0324, None) => self.latch(0xD050_0304) & 0xFFFF,
            (0xD050_0328, None) => self.latch(0xD050_0310) & 0xFFFF,
            (0xD050_032C, None) => self.latch(0xD050_0314) & 0xFFFF,
            (0xD050_0330, None) => 0x8000,
            (0xD010_002C, None) => 0,
            (0xD050_018C, None) => self.video_status,
            (0xD050_018C, Some(v)) => {
                self.video_status &= !v;
                0
            }
            // Serial status: always ready.
            // Other registers read back what was last written (a latch:
            // e.g. the interrupt controller's disable flag, 0xD0100038).
            (_, None) => latched,
            _ => 0,
        }
    }
}

/// The picture's size (VGA mode, 0xD05001FC bit 4). Sprite coordinates are
/// centred: x to the right and y up from the middle of the screen.
const SCREEN_W: usize = 640;
const SCREEN_H: usize = 480;

/// A 15-bit palette colour (bit 15: transparent) as 8-bit RGB.
fn rgb555(c: u16) -> [u8; 3] {
    let f = |v: u16| ((v & 31) << 3 | (v & 31) >> 2) as u8;
    [f(c >> 10), f(c >> 5), f(c)]
}

impl Bus {
    /// A byte of RAM, SRAM or on-chip RAM, without side effects.
    fn peek8(&self, a: u32) -> u8 {
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

    fn peek32(&self, a: u32) -> u32 {
        u32::from_le_bytes([self.peek8(a), self.peek8(a + 1), self.peek8(a + 2), self.peek8(a + 3)])
    }

    /// Pixel `i` of a picture of `bpp` bits per pixel at `addr`: the bits
    /// are one stream, most significant bit first (6-bit sprites show this).
    fn pixel(&self, addr: u32, bpp: u32, i: u32) -> u32 {
        let bit = i * bpp;
        let a = addr + bit / 8;
        let w = (self.peek8(a) as u32) << 16 | (self.peek8(a + 1) as u32) << 8 | self.peek8(a + 2) as u32;
        (w >> (24 - bpp - bit % 8)) & ((1 << bpp) - 1)
    }

    /// The video chip's picture, from its registers and RAM, as RGB. Only
    /// what the system menu uses is decoded (docs/zippity.md): text layer 1
    /// as an 8-bit bitmap, and sprites. Everything here is UNVERIFIED on
    /// hardware; it matches recordings of the start-up screens.
    fn render(&self) -> Vec<u8> {
        let mut img = vec![0u8; SCREEN_W * SCREEN_H * 3];
        // Palette RAM (0xD0501000): 16-bit colours, two per word.
        let pal: Vec<u16> = (0..2048u32).map(|i| (self.latch(0xD050_1000 + i / 2 * 4) >> (16 * (i & 1))) as u16).collect();

        // Text layer 1: +0x40/+0x44 scroll, +0x48 attributes, +0x4C control
        // (bit 3 on), +0x50 tile map (32-bit entries: the tile's byte offset
        // from the base at +0x80, 0 for none), pages of 1024x512 pixels.
        let attr = self.latch(0xD050_0048);
        if self.latch(0xD050_004C) & 8 != 0 && attr & 3 == 3 {
            let (map, base) = (self.latch(0xD050_0050), self.latch(0xD050_0080));
            let (tw, th) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
            // Bits 23-22 pick the palette's 256-colour bank (0x300 seen).
            let bank = ((attr >> 22) & 3) * 0x100;
            let (sx, sy) = (self.latch(0xD050_0040), self.latch(0xD050_0044));
            for y in 0..SCREEN_H as u32 {
                for x in 0..SCREEN_W as u32 {
                    let (px, py) = ((x + sx) % 1024, (y + sy) % 512);
                    let entry = self.peek32(map + ((py / th) * (1024 / tw) + px / tw) * 4);
                    if entry == 0 {
                        continue;
                    }
                    let c = pal[(bank + self.peek8(base + entry + (py % th) * tw + px % tw) as u32) as usize & 2047];
                    if c & 0x8000 == 0 {
                        let o = (y as usize * SCREEN_W + x as usize) * 3;
                        img[o..o + 3].copy_from_slice(&rgb555(c));
                    }
                }
            }
        }

        // Sprites (control +0x108, bit 0 on): up to 512 entries of 32 bytes at
        // 0xD0502000. Halfword 0 and byte 8 bits 6-0: the picture's address
        // in halfwords; halfwords 1 and 2: x and y; halfword 3: attributes
        // (bits 1-0 depth 2/4/6/8 bits, 2/3 flips, 5-4 width and 7-6 height
        // 8 << n, 11-8 palette, 13-12 priority, 15 palette bank: colours
        // from 0x200 if set, 0x100 if clear).
        if self.latch(0xD050_0108) & 1 == 0 {
            return img;
        }
        let half = |n: u32, k: u32| (self.latch(0xD050_2000 + n * 32 + k / 2 * 4) >> (16 * (k & 1))) as u16;
        // Control bits 15-8: the number of sprites drawn, in fours (the
        // kernel sends that many entries by DMA each frame; later ones are
        // stale).
        let count = ((self.latch(0xD050_0108) >> 8) & 0xFF) * 4;
        let mut order: Vec<(u16, u32)> =
            (0..count.min(512)).filter(|&n| half(n, 0) != 0 || half(n, 4) & 0x7F != 0).map(|n| ((half(n, 3) >> 12) & 3, n)).collect();
        order.sort();
        for (_, n) in order {
            let attr = half(n, 3) as u32;
            let addr = (half(n, 0) as u32 | (half(n, 4) as u32 & 0x7F) << 16) * 2;
            let x0 = SCREEN_W as i32 / 2 + half(n, 1) as i16 as i32;
            let y0 = SCREEN_H as i32 / 2 - half(n, 2) as i16 as i32;
            let (w, h) = (8u32 << ((attr >> 4) & 3), 8u32 << ((attr >> 6) & 3));
            // The size on screen: the corner converter's third corner (this
            // tool's own encoding, see the converter), or the picture's.
            let (sw, sh) = if half(n, 7) == 0x8000 {
                let x2 = SCREEN_W as i32 / 2 + half(n, 5) as i16 as i32;
                let y2 = SCREEN_H as i32 / 2 - half(n, 6) as i16 as i32;
                ((x2 - x0 + 1).max(0) as u32, (y2 - y0 + 1).max(0) as u32)
            } else {
                (w, h)
            };
            // 8 bits with palette bit 0 set: 16 bits per pixel. A pixel with
            // bit 14 set is a colour (bits 7-0, from the bank) and an
            // opacity (below); otherwise it is the colour itself,
            // RGB 5:5:5, 0 transparent (bit 15 is set in some light pixels;
            // its meaning is UNKNOWN).
            let wide = attr & 3 == 3 && attr & 0x100 != 0;
            let bpp = if wide { 16 } else { 2 * ((attr & 3) + 1) };
            let bank = if attr & 0x8000 != 0 { 0x200 } else { 0x100 };
            let pal_base = bank + ((((attr >> 8) & 15) << 4) >> bpp << bpp);
            for sy in 0..sh {
                for sx in 0..sw {
                    let (px, py) = (sx * w / sw, sy * h / sh);
                    let x = x0 + if attr & 4 != 0 { (sw - 1 - sx) as i32 } else { sx as i32 };
                    let y = y0 + if attr & 8 != 0 { (sh - 1 - sy) as i32 } else { sy as i32 };
                    if x < 0 || y < 0 || x >= SCREEN_W as i32 || y >= SCREEN_H as i32 {
                        continue;
                    }
                    let o = (y as usize * SCREEN_W + x as usize) * 3;
                    let a = addr + (py * w + px) * 2;
                    let v = if wide { self.peek8(a) as u32 | (self.peek8(a + 1) as u32) << 8 } else { 0 };
                    if wide && v & 0x4000 == 0 {
                        if v != 0 {
                            img[o..o + 3].copy_from_slice(&rgb555(v as u16 & 0x7FFF));
                        }
                    } else if wide {
                        // The opacity's encoding depends on the bank: from 0x200,
                        // bits 13-8 (0-63; the menus' background art, which
                        // fades through it); from 0x100, opaque unless bit 13
                        // is set, then bits 12-8 (0-31; the game pictures,
                        // all 0x18 with bit 13 clear, are solid in
                        // recordings). UNVERIFIED.
                        let (alpha, full) = if bank == 0x200 {
                            ((v >> 8) & 63, 63)
                        } else if v & 0x2000 != 0 {
                            ((v >> 8) & 31, 31)
                        } else {
                            (1, 1)
                        };
                        let raw = pal[(bank + (v & 0xFF)) as usize];
                        if bank == 0x100 && raw & 0x8000 != 0 {
                            continue;
                        }
                        let c = rgb555(raw);
                        for k in 0..3 {
                            img[o + k] = ((c[k] as u32 * alpha + img[o + k] as u32 * (full - alpha)) / full) as u8;
                        }
                    } else {
                        let c = pal[(pal_base + self.pixel(addr, bpp, py * w + px)) as usize & 2047];
                        if c & 0x8000 == 0 {
                            img[o..o + 3].copy_from_slice(&rgb555(c));
                        }
                    }
                }
            }
        }
        img
    }
}

/// Saves a picture as a binary PPM.
fn save_ppm(path: &str, img: &[u8]) {
    let mut out = format!("P6\n{SCREEN_W} {SCREEN_H}\n255\n").into_bytes();
    out.extend_from_slice(img);
    std::fs::write(path, out).unwrap();
}

/// The sound unit (0xD0400000): 32 channels like the 16-bit GeneralPlus
/// chips' (MAME's spg2xx_audio), registers four bytes apart. Channel n's
/// registers are at +0x1000 + 0x40 n (0: wave address in halfwords, 1: mode,
/// 3: pan and volume, ...), its phase registers at +0x1800 + 0x40 n (4: the
/// phase, 19-bit), and the control registers at +0xE00 for channels 0-15 and
/// +0xE80 for 16-31 (+0x00 enable, +0x2C stop flags, +0x3C busy).
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
struct Voice {
    playing: bool,
    /// The next word's address, in halfwords.
    addr: u32,
    /// Nibbles (or bytes) of the word used so far, times 4 (or 8).
    shift: u32,
    signal: i32,
    step: i32,
    accum: u64,
    sample: i32,
    /// The sample before, for interpolating between the two.
    prev: i32,
}

/// The sound unit's base clock: a channel plays phase * SPU_CLOCK / 2^19
/// samples a second (as MAME's model has it; UNVERIFIED on the Zippity).
const SPU_CLOCK: u64 = 281_250;
/// The rate the research tool mixes at (a stand-in).
const MIX_RATE: u64 = 35_156;

/// OKI ADPCM: the step sizes and how each nibble moves the step index.
fn oki_step(i: i32) -> i32 {
    (16.0 * 1.1f64.powi(i)) as i32
}
const OKI_INDEX: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];

impl Bus {
    fn spu_reg(&self, ch: usize, r: u32) -> u32 {
        self.latch(0xD040_1000 + 0x40 * ch as u32 + 4 * r)
    }

    /// Channel enable writes: channels newly enabled start, ones cleared stop.
    fn spu_enable(&mut self, bank: usize, v: u32) {
        for i in 0..16 {
            let ch = bank * 16 + i;
            let on = v & (1 << i) != 0;
            if on && !self.voices[ch].playing {
                if std::env::var_os("ZIPBOOT_SPULOG").is_some() {
                    let phase = self.latch(0xD040_1800 + 0x40 * ch as u32 + 0x10);
                    println!("spu {:>10} ch {ch} mode {:04x} addr {:06x} loop {:06x} phase {phase:05x} vol {:04x}", self.now, self.spu_reg(ch, 1), self.spu_reg(ch, 0), self.spu_reg(ch, 2), self.spu_reg(ch, 3));
                }
                self.voices[ch] = Voice { playing: true, addr: self.spu_reg(ch, 0) & 0x3F_FFFF, ..Default::default() };
            } else if !on {
                self.voices[ch].playing = false;
            }
        }
    }

    /// The next sample of a channel, or None at the end of a one-shot.
    fn spu_fetch(&mut self, ch: usize) -> Option<i32> {
        let mode = self.spu_reg(ch, 1);
        let tone = (mode >> 12) & 3;
        // Tone mode 0: the CPU writes the samples (register 0xB, wave data).
        if tone == 0 {
            return Some((self.spu_reg(ch, 0xB) as u16 as i32) - 0x8000);
        }
        let v = self.voices[ch];
        let a = v.addr * 2;
        let word = self.peek8(a) as u32 | (self.peek8(a + 1) as u32) << 8;
        if tone != 0 && word == 0xFFFF {
            if tone == 1 {
                return None;
            }
            // Loop: from the loop address (register 2).
            self.voices[ch].addr = self.spu_reg(ch, 2) & 0x3F_FFFF;
            self.voices[ch].shift = 0;
            return Some(v.sample);
        }
        let vo = &mut self.voices[ch];
        if mode & 0x8000 != 0 {
            // 4-bit OKI ADPCM, high nibble first (low first, as in MAME's
            // 16-bit chips, decodes to noise).
            let n = ((word >> (12 - vo.shift)) & 15) as i32;
            let step = oki_step(vo.step);
            let mut diff = step >> 3;
            if n & 1 != 0 {
                diff += step >> 2;
            }
            if n & 2 != 0 {
                diff += step >> 1;
            }
            if n & 4 != 0 {
                diff += step;
            }
            vo.signal = (vo.signal + if n & 8 != 0 { -diff } else { diff }).clamp(-2048, 2047);
            vo.step = (vo.step + OKI_INDEX[(n & 7) as usize]).clamp(0, 48);
            vo.shift += 4;
            if vo.shift == 16 {
                vo.shift = 0;
                vo.addr += 1;
            }
            Some(vo.signal << 4)
        } else if mode & 0x4000 != 0 {
            vo.addr += 1;
            Some(word as i32 - 0x8000)
        } else {
            let b = (word >> vo.shift) & 0xFF;
            vo.shift += 8;
            if vo.shift == 16 {
                vo.shift = 0;
                vo.addr += 1;
            }
            Some(((b as i32) << 8) - 0x8000)
        }
    }

    /// Mixes one output sample (left, right).
    fn spu_mix(&mut self) -> (i32, i32) {
        let (mut l, mut r) = (0i64, 0i64);
        for ch in 0..32 {
            if !self.voices[ch].playing {
                continue;
            }
            let phase = (self.latch(0xD040_1800 + 0x40 * ch as u32 + 0x10) & 0x7_FFFF) as u64;
            self.voices[ch].accum += phase * SPU_CLOCK;
            while self.voices[ch].accum >= MIX_RATE << 19 {
                self.voices[ch].accum -= MIX_RATE << 19;
                match self.spu_fetch(ch) {
                    Some(s) => {
                        self.voices[ch].prev = self.voices[ch].sample;
                        self.voices[ch].sample = s;
                    }
                    None => {
                        self.voices[ch].playing = false;
                        // The stop flag; the channel's enable bit clears.
                        let (bank, bit) = (ch / 16, ch % 16);
                        self.spu_stopped[bank] |= 1 << bit;
                        let en = 0xD040_0E00 + 0x80 * bank as u32;
                        if let Some(e) = self.io.get_mut(&en) {
                            e.last_write &= !(1 << bit);
                        }
                        break;
                    }
                }
            }
            if !self.voices[ch].playing {
                continue;
            }
            let pv = self.spu_reg(ch, 3);
            let (vol, pan) = ((pv & 0x7F) as i64, ((pv >> 8) & 0x7F) as i64);
            // Linear interpolation between the last two samples (holding
            // each sample adds images of it above 8 kHz).
            let v = self.voices[ch];
            let frac = (v.accum >> 19) as i64;
            let s = v.prev as i64 + (v.sample - v.prev) as i64 * frac / MIX_RATE as i64;
            let s = s * vol / 127;
            l += s * (127 - pan) / 64;
            r += s * pan.max(1) / 64;
        }
        (l.clamp(-32768 * 4, 32767 * 4) as i32 / 4, r.clamp(-32768 * 4, 32767 * 4) as i32 / 4)
    }
}

/// Saves interleaved 16-bit stereo samples as a WAV file.
fn save_wav(path: &str, samples: &[i16], rate: u32) {
    let data = samples.len() as u32 * 2;
    let mut out = Vec::with_capacity(44 + data as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 4).to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, out).unwrap();
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
            Some(m) => u32::from_le_bytes(m[..4].try_into().unwrap()),
            None => self.io(a, None, 32),
        }
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.note_write(a, v as u32, 8);
        match self.mem(a, 1) {
            Some(m) => m[0] = v,
            None => {
                self.io(a, Some(v as u32), 8);
            }
        }
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.note_write(a, v as u32, 16);
        match self.mem(a, 2) {
            Some(m) => m.copy_from_slice(&v.to_le_bytes()),
            None => {
                self.io(a, Some(v as u32), 16);
            }
        }
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.note_write(a, v, 32);
        match self.mem(a, 4) {
            Some(m) => m.copy_from_slice(&v.to_le_bytes()),
            None => {
                self.io(a, Some(v), 32);
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let flash = std::fs::read(&args[1]).expect("read the NAND image");
    let steps: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5_000_000);
    // The boot ROM's copy: data 0xA0000..0x1F0000, spare bytes stripped.
    let data: Vec<u8> = flash.chunks(PAGE).take(0x1F_0000 / PAGE_DATA).flat_map(|p| p[..PAGE_DATA].to_vec()).collect();
    let (start, len) = (0xA_0000, 0x15_0000);
    let nand = Nand {
        flash,
        port: Port::Idle,
        column: 0,
        row: 0,
        buf: vec![0xFF; PAGE],
        cursor: 0,
        dma_addr: 0,
        dma_len: 0,
        regs: BTreeMap::new(),
        log: Vec::new(),
    };

    let mut bus = Bus { ram: vec![0; RAM_SIZE], sram: vec![0; SRAM_SIZE], iram: vec![0; IRAM_SIZE], pc: 0, now: 0, io: BTreeMap::new(), log: Vec::new(), uart: String::new(), nand, video_status: 0, dma_status: 0, gfx_ops: 0, gfx_done: false, watch: None, presses: Vec::new(), timer_due: [0; 4], timer_flags: 0, rx: Default::default(), pad: Vec::new(), radio_due: 0, voices: [Voice::default(); 32], spu_stopped: [0; 2], audio: Vec::new(), audio_due: 0, watch_from: 0, watched: Vec::new(), rwatch: None, rwatched: Vec::new() };
    if let Ok(w) = std::env::var("ZIPBOOT_RWATCH") {
        let (lo, hi) = w.split_once('-').expect("ZIPBOOT_RWATCH=lo-hi");
        bus.rwatch = Some((u32::from_str_radix(lo, 16).unwrap(), u32::from_str_radix(hi, 16).unwrap()));
    }
    if let Ok(p) = std::env::var("ZIPBOOT_PAD") {
        for item in p.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            bus.pad.push((u16::from_str_radix(f[0], 16).unwrap(), f[1].parse().unwrap(), f[2].parse().unwrap()));
        }
    }
    if let Ok(p) = std::env::var("ZIPBOOT_PRESS") {
        for item in p.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            let port = match f[0] {
                "a" => 0xC000_0000,
                "b" => 0xC000_0020,
                _ => 0xC000_0040,
            };
            bus.presses.push((port, u32::from_str_radix(f[1], 16).unwrap(), f[2].parse().unwrap(), f[3].parse().unwrap()));
        }
    }
    if let Some(n) = std::env::var("ZIPBOOT_WATCH_FROM").ok().and_then(|s| s.parse().ok()) {
        bus.watch_from = n;
    }
    if let Ok(w) = std::env::var("ZIPBOOT_WATCH") {
        let (lo, hi) = w.split_once('-').expect("ZIPBOOT_WATCH=lo-hi");
        bus.watch = Some((u32::from_str_radix(lo, 16).unwrap(), u32::from_str_radix(hi, 16).unwrap()));
    }
    bus.ram[..len].copy_from_slice(&data[start..start + len]);
    let mut cpu = Arm::new();
    cpu.reset();
    // ZIPBOOT_LOAD=file: carry on from a snapshot; ZIPBOOT_SAVE=file:n: save
    // one after n instructions. Instruction counts (and so the step limit)
    // carry on from the snapshot's.
    let mut ran = 0u64;
    if let Ok(path) = std::env::var("ZIPBOOT_LOAD") {
        let snap: Snapshot = bincode::deserialize(&std::fs::read(&path).expect("read the snapshot")).expect("a snapshot");
        ran = snap.restore(&mut cpu, &mut bus);
    }
    let save: Option<(String, u64)> = std::env::var("ZIPBOOT_SAVE").ok().map(|s| {
        let (p, n) = s.rsplit_once(':').expect("ZIPBOOT_SAVE=file:n");
        (p.to_string(), n.parse().expect("an instruction count"))
    });

    let mut hist: BTreeMap<u32, u64> = BTreeMap::new();
    // Jumps (from, to, times), the last ZIPBOOT_JUMPS (600) kept;
    // consecutive repeats collapse.
    let jumps_max: usize = std::env::var("ZIPBOOT_JUMPS").ok().and_then(|s| s.parse().ok()).unwrap_or(600);
    let mut jumps: std::collections::VecDeque<(u32, u32, u32)> = std::collections::VecDeque::new();
    let mut prev = 0u32;
    // ZIPBOOT_EVERY=n (with ZIPBOOT_DUMP): RAM is also saved every n
    // instructions, as ram_<millions>.bin.
    let every: Option<u64> = std::env::var("ZIPBOOT_EVERY").ok().and_then(|s| s.parse().ok());
    let dump_dir = std::env::var("ZIPBOOT_DUMP").ok();
    // ZIPBOOT_FRAMES=n (with ZIPBOOT_DUMP): the picture is saved every n
    // instructions, as frame_<millions>.ppm.
    let frames: Option<u64> = std::env::var("ZIPBOOT_FRAMES").ok().and_then(|s| s.parse().ok());
    while ran < steps {
        if let Some((path, at)) = &save
            && ran == *at
        {
            std::fs::write(path, bincode::serialize(&Snapshot::take(&cpu, &bus, ran)).unwrap()).unwrap();
        }
        if let (Some(n), Some(dir)) = (frames, &dump_dir)
            && ran > 0
            && ran.is_multiple_of(n)
        {
            save_ppm(&format!("{dir}/frame_{:04}.ppm", ran / 1_000_000), &bus.render());
        }
        if let (Some(n), Some(dir)) = (every, &dump_dir)
            && ran > 0
            && ran.is_multiple_of(n)
        {
            std::fs::write(format!("{dir}/ram_{}.bin", ran / 1_000_000), &bus.ram[..0x100_0000]).unwrap();
        }
        bus.pc = cpu.r[15];
        bus.now = ran;
        if ran % FRAME == FRAME - 1 {
            // Bit 0 and bit 11 (0x800) each frame: which edge each is, and
            // their spacing, are UNKNOWN.
            bus.video_status |= 0x801;
        }
        if ran.is_multiple_of(256) {
            bus.tick();
        }
        cpu.irq_line = bus.irq_line();
        if !cpu.step(&mut bus) {
            ran += 1;
            continue;
        }
        ran += 1;
        let next = prev.wrapping_add(if cpu.thumb() { 2 } else { 4 });
        if cpu.current != next && cpu.current != prev {
            match jumps.back_mut() {
                Some(j) if (j.0, j.1) == (prev, cpu.current) => j.2 += 1,
                _ => {
                    jumps.push_back((prev, cpu.current, 1));
                    if jumps.len() > jumps_max {
                        jumps.pop_front();
                    }
                }
            }
        }
        prev = cpu.current;
        if ran > steps.saturating_sub(200_000) {
            *hist.entry(cpu.current).or_default() += 1;
        }
        for (pc, w) in cpu.undefined_log.drain(..) {
            println!("undefined {w:08x} at {pc:08x}");
        }
    }

    if std::env::var_os("ZIPBOOT_JUMPS").is_some() {
        println!("--- last jumps (from -> to, times) ---");
        for (from, to, n) in &jumps {
            println!("{from:08x} -> {to:08x}{}", if *n > 1 { format!("  x{n}") } else { String::new() });
        }
    }
    println!("--- first accesses ---");
    for l in &bus.log {
        println!("{l}");
    }
    println!("--- NAND ({} events) ---", bus.nand.log.len());
    for l in bus.nand.log.iter().take(200) {
        println!("{l}");
    }
    println!("--- 0xD0800000 operations: {} ---", bus.gfx_ops);
    println!("--- serial output ---
{}
--- end of serial output ---", bus.uart);
    println!("--- registers touched ({}) ---", bus.io.len());
    for (a, e) in &bus.io {
        println!("{a:08x}  reads {:>8}  writes {:>8}  last {:08x}  first pc {:08x}", e.reads, e.writes, e.last_write, e.first_pc);
    }
    println!("--- state after {ran} instructions ---");
    println!("pc {:08x} cpsr {:08x} mode {:02x} thumb {}", cpu.r[15], cpu.cpsr, cpu.mode(), cpu.thumb());
    for i in 0..16 {
        print!("r{i}={:08x} ", cpu.r[i]);
        if i % 4 == 3 {
            println!();
        }
    }
    println!("cp15 control {:08x} ttb {:08x}", cpu.cp15.control, cpu.cp15.ttb);
    if let Ok(dir) = std::env::var("ZIPBOOT_DUMP") {
        std::fs::write(format!("{dir}/watch.log"), bus.watched.join("\n")).ok();
        std::fs::write(format!("{dir}/rwatch.log"), bus.rwatched.join("\n")).ok();
        std::fs::write(format!("{dir}/ram.bin"), &bus.ram).unwrap();
        std::fs::write(format!("{dir}/iram.bin"), &bus.iram).unwrap();
    }
    if let Ok(path) = std::env::var("ZIPBOOT_WAV") {
        save_wav(&path, &bus.audio, MIX_RATE as u32);
    }
    let mut hot: Vec<_> = hist.into_iter().collect();
    hot.sort_by_key(|h| std::cmp::Reverse(h.1));
    println!("--- hottest pcs (last 200k) ---");
    for (pc, n) in hot.iter().take(16) {
        println!("{pc:08x} {n}");
    }
}
