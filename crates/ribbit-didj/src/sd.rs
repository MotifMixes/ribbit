//! The LF1000's SD/MMC controllers ("mes-sdhc", two channels at
//! 0xC000_9800 and 0xC000_C800, IRQs 14 and 42) and the SD card behind one.
//!
//! The LeapPad Explorer (2011, codename "Madrid", board IDs 0x0B-0x0D) keeps
//! its 2 GB internal storage on the SD bus ("2GB MLC Flash + SD Controller",
//! board_ids.h), not on the NAND controller: an MBR with two raw partitions
//! (boot splash, kernel) and two ext file systems (root, Bulk), the layout
//! of its firmware packages (`leappad.rs`).
//!
//! The controller is MagicEyes' version of Synopsys' DesignWare MMC block
//! (register names and bits from the kernel's drivers/mmc/host/mes_sdhc.h,
//! LeapFrog's 2.6.37 source; the LeapPad's 2.6.31 kernel has the same
//! driver name). Data moves through the FIFO at 0x100, by the DMA
//! controller (request 16 for channel 0, 30 for channel 1) or by the CPU.
//!
//! Commands finish at once; a read's data is ready as the command is taken
//! and goes out as fast as the DMA or the CPU takes it, a write's comes in
//! likewise; "data transfer over" follows the last byte. The card is a
//! high-capacity SD card (block addressing) answering the commands Linux
//! 2.6.31's MMC core sends; SDIO (CMD5) and MMC (CMD1) get no answer, as on
//! any SD card. Its contents live in a `Nand` used as plain storage (its
//! data area, byte for byte), so saves and states work as on the NAND
//! consoles. GUESSES: the card's CID and CSD details (a real unit's are
//! UNKNOWN; the capacity is the MBR's end).

use crate::nand::Nand;

pub const BASES: [u32; 2] = [0xC000_9800, 0xC000_C800];
pub const SIZE: u32 = 0x800;
pub const IRQS: [u32; 2] = [14, 42];
/// DMA request IDs (mach-lf1000 dma.h DMA_PERI_SD0RW, DMA_PERI_SD1RW).
pub const DMA_REQUESTS: [u32; 2] = [16, 30];

const CTRL: u32 = 0x000;
const BYTCNT: u32 = 0x020;
const INTMASK: u32 = 0x024;
const CMDARG: u32 = 0x028;
const CMD: u32 = 0x02C;
const RESP0: u32 = 0x030;
const MINTSTS: u32 = 0x040;
const RINTSTS: u32 = 0x044;
const STATUS: u32 = 0x048;
const TCBCNT: u32 = 0x05C;
const TBBCNT: u32 = 0x060;
const DAT: u32 = 0x100;
const DAT_END: u32 = 0x200;

// CTRL
const CTRL_RESETS: u32 = 0x7; // controller, FIFO, DMA reset (self-clearing)
const INT_ENA: u32 = 1 << 4;
const DMA_ENA: u32 = 1 << 5;
const ABORT_DATA: u32 = 1 << 8;

// CMD
const START: u32 = 1 << 31;
const UPDATE_CLOCK_ONLY: u32 = 1 << 21;
const SEND_AUTO_STOP: u32 = 1 << 12;
const DATA_EXPECTED: u32 = 1 << 9;
#[cfg(test)]
const RESPONSE_LONG: u32 = 1 << 7;
const RESPONSE_EXPECTED: u32 = 1 << 6;

// Interrupts (RINTSTS, INTMASK, MINTSTS)
const ACD: u32 = 1 << 14;
const RTO: u32 = 1 << 8;
const RXDR: u32 = 1 << 5;
const TXDR: u32 = 1 << 4;
const DTO: u32 = 1 << 3;
const CD: u32 = 1 << 2;

// STATUS
const FIFO_EMPTY: u32 = 1 << 2;
const FIFO_FULL: u32 = 1 << 3;
const TX_WATERMARK: u32 = 1 << 1;
const RX_WATERMARK: u32 = 1 << 0;
const CARD_PRESENT: u32 = 1 << 8;

/// The card's relative address (any non-zero value).
const RCA: u32 = 0x4C50;

/// Card states (the R1 status's CURRENT_STATE).
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug, serde::Serialize, serde::Deserialize)]
enum State {
    #[default]
    Idle = 0,
    Ready = 1,
    Ident = 2,
    Standby = 3,
    Transfer = 4,
}

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Channel {
    regs: [u32; 0x20],
    sysclk: [u32; 2],
    resp: [u32; 4],
    rintsts: u32,
    /// Read data on its way out of the FIFO.
    rx: std::collections::VecDeque<u8>,
    /// A write in progress: where on the card, and the bytes still wanted.
    write_at: u64,
    write_left: u32,
    /// Data bytes moved for the current command (TBBCNT), and whether it
    /// ends with an automatic stop.
    moved: u32,
    auto_stop: bool,
    busy_data: bool,
    /// The card on this channel.
    card: bool,
    state: State,
    app_cmd: bool,
    /// Interrupt flags waiting to be raised, and the processor cycles until
    /// they are (`COMPLETION_CYCLES`).
    pending: u32,
    countdown: u64,
}

/// Processor cycles from a command (or the last data byte) to its
/// interrupt flags. A command can't finish at once: Linux's MMC core
/// retries a failed command from inside the interrupt handler, which then
/// clears the flags it read, the retry's among them if it were done already
/// (the card's CMD5 timeout was lost so, and the card never found). About
/// 10 us at 393 MHz; a real 400 kHz command and response take ~120 us.
const COMPLETION_CYCLES: u64 = 4000;

#[derive(Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct Sdhc {
    pub ch: [Channel; 2],
    /// Commands and register writes, oldest first, while `tracing` (a
    /// diagnostic for the examples; at most 4000 lines).
    #[serde(skip)]
    pub trace: Vec<String>,
    #[serde(skip)]
    pub tracing: bool,
}

impl Sdhc {
    /// Controllers with the card on channel `card_channel`.
    pub fn with_card(card_channel: usize) -> Self {
        let mut s = Self::default();
        s.ch[card_channel].card = true;
        s
    }

    /// The channel at `addr`, if it is an SD controller's.
    pub fn channel(addr: u32) -> Option<usize> {
        BASES.iter().position(|&b| addr.wrapping_sub(b) < SIZE)
    }

    /// Runs the controllers' clocks `cycles` processor cycles on.
    pub fn tick(&mut self, cycles: u64) {
        for ch in &mut self.ch {
            if ch.pending != 0 {
                ch.countdown = ch.countdown.saturating_sub(cycles);
                if ch.countdown == 0 {
                    ch.rintsts |= std::mem::take(&mut ch.pending);
                }
            }
        }
    }

    /// Processor cycles until the next interrupt flags are raised.
    pub fn next_event(&self) -> u64 {
        self.ch.iter().filter(|c| c.pending != 0).map(|c| c.countdown.max(1)).min().unwrap_or(u64::MAX)
    }

    pub fn irq_lines(&self) -> impl Iterator<Item = u32> + '_ {
        self.ch.iter().zip(IRQS).filter(|(c, _)| c.regs[0] & INT_ENA != 0 && c.rintsts & c.regs[(INTMASK / 4) as usize] != 0).map(|(_, i)| i)
    }

    pub fn read(&mut self, c: usize, off: u32, storage: &Nand) -> u32 {
        let ch = &mut self.ch[c];
        let _ = storage;
        match off {
            MINTSTS => ch.rintsts & ch.reg(INTMASK),
            RINTSTS => ch.rintsts,
            RESP0..=0x03C => ch.resp[((off - RESP0) / 4) as usize],
            STATUS => {
                let n = ch.rx.len().div_ceil(4) as u32;
                let mut s = (n.min(0x1FFF)) << 17 | (ch.state as u32) << 11;
                if ch.card {
                    s |= CARD_PRESENT;
                }
                if ch.rx.is_empty() {
                    s |= FIFO_EMPTY | TX_WATERMARK;
                } else {
                    s |= RX_WATERMARK;
                    if n >= 16 {
                        s |= FIFO_FULL;
                    }
                }
                s
            }
            TCBCNT | TBBCNT => ch.moved,
            DAT..DAT_END => {
                let mut v = 0;
                for i in 0..4 {
                    v |= (ch.rx.pop_front().unwrap_or(0) as u32) << (8 * i);
                }
                ch.moved += 4;
                ch.check_done();
                v
            }
            0x7C0 => ch.sysclk[0],
            0x7C4 => ch.sysclk[1],
            o if o < 0x80 => ch.reg(o),
            _ => 0,
        }
    }

    pub fn write(&mut self, c: usize, off: u32, v: u32, storage: &mut Nand) {
        if self.tracing && self.trace.len() < 4000 && !(DAT..DAT_END).contains(&off) {
            let ch = &self.ch[c];
            self.trace.push(format!("ch{c} w {off:03x}={v:08x} (arg {:08x} bytes {} rint {:x})", ch.reg(CMDARG), ch.reg(BYTCNT), ch.rintsts));
        }
        let ch = &mut self.ch[c];
        match off {
            CTRL => {
                if v & (CTRL_RESETS | ABORT_DATA) != 0 {
                    ch.pending = 0;
                    ch.rx.clear();
                    ch.write_left = 0;
                    ch.busy_data = false;
                }
                ch.regs[0] = v & !(CTRL_RESETS | ABORT_DATA);
            }
            RINTSTS => ch.rintsts &= !v,
            CMD => {
                ch.regs[(CMD / 4) as usize] = v & !START;
                if v & START != 0 {
                    ch.command(v, storage);
                }
            }
            DAT..DAT_END => {
                if ch.write_left > 0 {
                    let n = ch.write_left.min(4) as usize;
                    storage_write(storage, ch.write_at, &v.to_le_bytes()[..n]);
                    ch.write_at += n as u64;
                    ch.write_left -= n as u32;
                    ch.moved += n as u32;
                    ch.check_done();
                }
            }
            0x7C0 => ch.sysclk[0] = v,
            0x7C4 => ch.sysclk[1] = v,
            o if o < 0x80 => ch.regs[(o / 4) as usize] = v,
            _ => {}
        }
    }

    /// Serves a DMA command for channel `c`: `len` bytes to (`into_memory`)
    /// or from memory. Returns the bytes read for memory (None: not ready).
    pub fn dma_read(&mut self, c: usize, len: usize) -> Option<Vec<u8>> {
        let ch = &mut self.ch[c];
        if ch.rx.is_empty() {
            return None;
        }
        let n = len.min(ch.rx.len());
        let out: Vec<u8> = ch.rx.drain(..n).collect();
        ch.moved += n as u32;
        ch.check_done();
        Some(out)
    }

    /// Whether channel `c` wants written data (a DMA command may go).
    pub fn wants_write(&self, c: usize) -> bool {
        self.ch[c].write_left > 0
    }

    pub fn dma_write(&mut self, c: usize, data: &[u8], storage: &mut Nand) {
        let ch = &mut self.ch[c];
        let n = data.len().min(ch.write_left as usize);
        storage_write(storage, ch.write_at, &data[..n]);
        ch.write_at += n as u64;
        ch.write_left -= n as u32;
        ch.moved += n as u32;
        ch.check_done();
    }
}

/// Writes card bytes into the storage's data area.
fn storage_write(storage: &mut Nand, at: u64, data: &[u8]) {
    storage.write_bytes(at as usize, data);
}

impl Channel {
    fn reg(&self, off: u32) -> u32 {
        self.regs[(off / 4) as usize]
    }

    /// The card's R1 status.
    fn status(&self) -> u32 {
        (self.state as u32) << 9 | 1 << 8 | if self.app_cmd { 1 << 5 } else { 0 }
    }

    /// Raises interrupt flags after `COMPLETION_CYCLES`.
    fn raise(&mut self, bits: u32) {
        if self.pending == 0 {
            self.countdown = COMPLETION_CYCLES;
        }
        self.pending |= bits;
    }

    fn check_done(&mut self) {
        if self.busy_data && self.rx.is_empty() && self.write_left == 0 {
            self.busy_data = false;
            self.raise(if self.auto_stop { DTO | ACD } else { DTO });
            self.rintsts &= !(RXDR | TXDR);
            self.pending &= !(RXDR | TXDR);
        } else if self.busy_data && self.reg(CTRL) & DMA_ENA == 0 {
            self.raise(if self.rx.is_empty() { TXDR } else { RXDR });
        }
    }

    fn command(&mut self, cmd: u32, storage: &mut Nand) {
        if cmd & UPDATE_CLOCK_ONLY != 0 {
            return;
        }
        let (index, arg) = (cmd & 0x3F, self.reg(CMDARG));
        let app = std::mem::take(&mut self.app_cmd);
        self.resp = [0; 4];
        self.moved = 0;
        let response = if self.card { self.respond(index, arg, app, cmd, storage) } else { None };
        match response {
            Some(r) => {
                if cmd & RESPONSE_EXPECTED != 0 {
                    self.resp = r;
                }
                self.raise(CD);
            }
            None => {
                self.raise(CD | if cmd & RESPONSE_EXPECTED != 0 { RTO } else { 0 });
                return;
            }
        }
        if cmd & DATA_EXPECTED != 0 {
            self.auto_stop = cmd & SEND_AUTO_STOP != 0;
            self.busy_data = true;
            self.check_done();
        }
    }

    /// The card's answer (resp[0] holds a short response, resp[3..0] a
    /// long one, bits 127:96 in resp[3]); None for no answer (a timeout).
    fn respond(&mut self, index: u32, arg: u32, app: bool, cmd: u32, storage: &mut Nand) -> Option<[u32; 4]> {
        let short = |v: u32| [v, 0, 0, 0];
        let bytes = self.reg(BYTCNT) as usize;
        let size = storage.size() as u64;
        if app {
            return match index {
                6 | 23 | 42 => Some(short(self.status())),
                41 => {
                    // Ready at once; high capacity if the host said it can.
                    if self.state == State::Idle && arg & 0x00FF_8000 != 0 {
                        self.state = State::Ready;
                    }
                    let ready = if self.state == State::Ready { 1 << 31 } else { 0 };
                    Some(short(ready | 1 << 30 | 0x00FF_8000))
                }
                51 => {
                    // SCR: structure 0, SD 2.0, security 3 (SDHC), 1- and
                    // 4-bit buses.
                    self.read_data(&[0x02, 0x35, 0, 0, 0, 0, 0, 0], bytes);
                    Some(short(self.status() | 1 << 5))
                }
                13 => {
                    let mut st = [0u8; 64];
                    st[0] = 0x80; // 4-bit bus
                    self.read_data(&st, bytes);
                    Some(short(self.status() | 1 << 5))
                }
                22 => {
                    self.read_data(&[0, 0, 0, 0], bytes);
                    Some(short(self.status()))
                }
                _ => self.respond(index, arg, false, cmd, storage),
            };
        }
        match index {
            0 => {
                self.state = State::Idle;
                Some(short(0))
            }
            8 => Some(short(arg & 0xFFF)),
            55 => {
                self.app_cmd = true;
                Some(short(self.status() | 1 << 5))
            }
            2 => {
                self.state = State::Ident;
                Some(long(cid()))
            }
            3 => {
                self.state = State::Standby;
                Some(short(RCA << 16 | (self.state as u32) << 9 | 1 << 8))
            }
            9 => Some(long(csd(size))),
            10 => Some(long(cid())),
            7 => {
                if arg >> 16 == RCA {
                    self.state = State::Transfer;
                    Some(short(self.status()))
                } else {
                    self.state = State::Standby;
                    None
                }
            }
            13 => Some(short(self.status())),
            12 => Some(short(self.status())),
            15 => {
                self.state = State::Idle;
                None
            }
            16 => Some(short(self.status())),
            6 => {
                // Switch function: group 1 supports default and high speed;
                // the function asked for is the one chosen.
                let mut st = [0u8; 64];
                st[1] = 0x64;
                st[13] = 0x03;
                let f = arg & 0xF;
                st[16] = if f <= 1 { f as u8 } else { 0xF };
                self.read_data(&st, bytes);
                Some(short(self.status()))
            }
            17 | 18 => {
                let at = arg as u64 * 512;
                let n = (bytes as u64).min(size.saturating_sub(at)) as usize;
                let data = storage.read(at as usize, n);
                self.read_data(&data, bytes);
                Some(short(self.status()))
            }
            24 | 25 => {
                self.write_at = arg as u64 * 512;
                self.write_left = (bytes as u64).min(size.saturating_sub(self.write_at)) as u32;
                Some(short(self.status()))
            }
            32 | 33 | 38 => Some(short(self.status())),
            // CMD1 (MMC), CMD5 (SDIO) and the rest: no answer.
            _ => None,
        }
    }

    fn read_data(&mut self, data: &[u8], bytes: usize) {
        let n = if bytes == 0 { data.len() } else { bytes };
        self.rx.clear();
        self.rx.extend(data.iter().take(n));
        self.rx.resize(n, 0);
    }
}

/// A 128-bit register (bits 127:96 first) as the controller's response
/// registers hold it.
fn long(r: [u8; 16]) -> [u32; 4] {
    let w = |i: usize| u32::from_be_bytes(r[i..i + 4].try_into().unwrap());
    [w(12), w(8), w(4), w(0)]
}

/// The CRC7 of a register's first 15 bytes, in bits 7:1 of the last, with
/// the end bit.
fn with_crc(mut r: [u8; 16]) -> [u8; 16] {
    let mut crc = 0u8;
    for &b in &r[..15] {
        for i in (0..8).rev() {
            let bit = (b >> i) & 1 ^ (crc >> 6) & 1;
            crc = (crc << 1) & 0x7F;
            if bit != 0 {
                crc ^= 0x09;
            }
        }
    }
    r[15] = crc << 1 | 1;
    r
}

/// The card ID: maker, OEM, product name "LPAD1", revision, serial, made
/// June 2011. GUESS: a real unit's is UNKNOWN.
fn cid() -> [u8; 16] {
    with_crc([0x1B, b'S', b'M', b'L', b'P', b'A', b'D', b'1', 0x10, 0, 0, 0, 1, 0x00, 0xB6, 0])
}

/// A version 2.0 (high capacity) CSD for `size` bytes: 512-byte blocks,
/// 25 MHz, C_SIZE = size / 512 KiB - 1.
fn csd(size: u64) -> [u8; 16] {
    let c_size = (size / (512 * 1024)).saturating_sub(1) as u32;
    with_crc([
        0x40,
        0x0E,
        0x00,
        0x32,
        0x5B,
        0x59,
        0x00,
        (c_size >> 16) as u8 & 0x3F,
        (c_size >> 8) as u8,
        c_size as u8,
        0x7F,
        0x80,
        0x0A,
        0x40,
        0x00,
        0,
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cmd(s: &mut Sdhc, nand: &mut Nand, index: u32, arg: u32, flags: u32) -> u32 {
        let base = 0;
        s.write(0, base + CMDARG, arg, nand);
        s.write(0, base + CMD, START | flags | index, nand);
        s.tick(COMPLETION_CYCLES);
        let r = s.read(0, RESP0, nand);
        s.write(0, RINTSTS, !0, nand);
        r
    }

    #[test]
    fn card_comes_up_and_reads_a_block() {
        let mut nand = Nand::with_blocks(16);
        nand.write_bytes(0x400, b"hello");
        let mut s = Sdhc::with_card(0);
        cmd(&mut s, &mut nand, 0, 0, 0);
        assert_eq!(cmd(&mut s, &mut nand, 8, 0x1AA, RESPONSE_EXPECTED), 0x1AA);
        cmd(&mut s, &mut nand, 55, 0, RESPONSE_EXPECTED);
        assert!(cmd(&mut s, &mut nand, 41, 0x40FF_8000, RESPONSE_EXPECTED) & 1 << 31 != 0);
        cmd(&mut s, &mut nand, 2, 0, RESPONSE_EXPECTED | RESPONSE_LONG);
        assert_eq!(cmd(&mut s, &mut nand, 3, 0, RESPONSE_EXPECTED) >> 16, RCA);
        cmd(&mut s, &mut nand, 7, RCA << 16, RESPONSE_EXPECTED);
        s.write(0, BYTCNT, 512, &mut nand);
        cmd(&mut s, &mut nand, 17, 2, RESPONSE_EXPECTED | DATA_EXPECTED);
        let data = s.dma_read(0, 512).unwrap();
        assert_eq!(&data[..5], b"hello");
        s.tick(COMPLETION_CYCLES);
        assert!(s.ch[0].rintsts & DTO != 0);
        // No SDIO.
        s.write(0, CMD, START | RESPONSE_EXPECTED | 5, &mut nand);
        s.tick(COMPLETION_CYCLES);
        assert!(s.ch[0].rintsts & RTO != 0);
    }

    #[test]
    fn csd_capacity() {
        let c = csd(2_061_500_416);
        let c_size = ((c[7] as u32 & 0x3F) << 16) | (c[8] as u32) << 8 | c[9] as u32;
        assert_eq!(c_size, 3931);
    }
}
