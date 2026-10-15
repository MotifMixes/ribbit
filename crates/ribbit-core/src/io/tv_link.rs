//! EXPERIMENTAL: the Leapster TV's controller link: a second UART at
//! 0x0180_D600 (same register layout as the debug UART at 0x0180_D500) and
//! the controller on the other end.
//!
//! What is known (VERIFIED-FW, Leapster TV BIOS 152-11594; no hardware
//! documentation, no MAME support):
//!
//! | Behaviour | Evidence |
//! |---|---|
//! | +0x10 data, +0x14 status/control, +0x18 = 0xCF (baud divisor), +0x1C written at init | init at 0x4005_4CB6 |
//! | Status bit 7 = transmitter ready, bit 5 = receive FIFO empty | TX at 0x4005_5312, RX loop at 0x4005_5576 |
//! | Interrupt handler 0x4005_5298 on vector 0x1C; control 0x44 = TX + RX interrupts, 0x04 = RX only | 0x4005_4C50, 0x4005_5664 |
//! | One wire: the UART hears its own bytes, and the BIOS checks each echo | 0x4005_54A6-0x4005_54FA |
//! | Two controller ports share the UART, chosen through 0x0180_D700 bits 8/9 (bit 9 = port 0) | 0x4005_59A0 |
//! | Every 10 ms: send 0xFF, a command byte, its data; the controller answers with an ack byte and data whose bytes (ack included) sum to 0 | 0x4005_5694, 0x4005_5920 |
//! | Commands AA A9 A6 A5 AC 95 A3; acks AC A9 A3 A5 A9 6A; data after the ack 3 0 25 1(+n) 0 6 bytes; A3 (poll) is answered by 0xAA (nothing) or a 0x5X packet (even-parity nibble; bit 3 = 6 data bytes, else 2) | tables at 0x400B_FB24-0x400B_FB5B |
//!
//! Model: transmission and the controller's answers are instant (no 4800
//! baud timing). The controller's answers beyond the framing above are
//! HYPOTHESIS, refined by watching what the BIOS accepts (see `log`).

use std::collections::VecDeque;

pub const BASE: u32 = 0x0180_D600;
pub const DATA: u32 = BASE + 0x10;
pub const STATUS: u32 = BASE + 0x14;
pub const PORT_SELECT: u32 = 0x0180_D700;
pub const IRQ_LINE: u8 = 0x1C;
const TX_READY: u32 = 1 << 7;
const RX_EMPTY: u32 = 1 << 5;
const CTRL_TX_IRQ: u32 = 0x40;
const CTRL_RX_IRQ: u32 = 0x04;
const MEMORY_LEN: usize = 37;
/// The click (stylus tip): data bit 7, GIODataIn 0x0004_0000.
const CLICK: u32 = 0x0004_0000;
/// Polls (10 ms each) the pen is in range before the tip goes down, and
/// stays in range after it comes up: more than the BIOS's 2-poll debounce.
const TIP_DELAY: u8 = 3;
const LINGER: u8 = 3;
const CALIBRATION: [u16; 4] = [0x040, 0x3C0, 0x040, 0x3C0];
/// GIODataIn mask per packet bit: header bits 0-1, then data bits 0-7
/// (VERIFIED-FW, BIOS table at 0x400B_FA34): Left, Up, Right, Down, Pause,
/// A, B, Hint, Home, and 0x0004_0000 (the click, `CLICK`).
const BUTTON_MAP: [u32; 10] =
    [0x200, 0x8000_0000, 0x80, 0x100, 0x0400_0000, 0x4000, 0x2000, 0x1000_0000, 0x2000_0000, 0x0004_0000];

/// Data bytes the BIOS sends after each command byte.
fn command_data_len(cmd: u8) -> Option<usize> {
    match cmd {
        0xAA => Some(3),
        0xA6 => Some(15),
        0xAC => Some(0x27),
        0xA9 | 0xA3 | 0xA5 | 0x95 => Some(0),
        _ => None,
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TvLink {
    pub rx: VecDeque<u8>,
    pub control: u32,
    pub regs: [u32; 2],
    pub port_select: u32,
    /// Bytes since the last 0xFF sync (the current transaction).
    pub frame: Vec<u8>,
    /// Buttons held, as GIODataIn masks (`ribbit_input::Button::mask`); the
    /// machine copies them in before each run.
    pub pressed: u32,
    /// The controller's 37-byte memory block, read by A5 and written by AC.
    pub memory: Vec<u8>,
    /// Pen on the pad, as an LCD-sized screen pixel (None = lifted).
    pub pen: Option<(u16, u16)>,
    /// Last pen position, polls the pen has been in range, and polls left
    /// before it leaves range after a lift (see `poll_reply`).
    pub pen_at: (u16, u16),
    pub in_range: u8,
    pub linger: u8,
    /// Research: one line per transaction (not saved).
    #[serde(skip)]
    pub log: Vec<String>,
    #[serde(skip)]
    pub logging: bool,
}

impl TvLink {
    /// Selected controller port, numbered as the BIOS does: 0x0180_D700
    /// bit 9 selects port 0, bit 8 port 1 (0x4005_59A0).
    pub fn port(&self) -> usize {
        if self.port_select & 0x200 != 0 { 0 } else { 1 }
    }

    pub fn irq_line(&self) -> bool {
        self.control & CTRL_TX_IRQ != 0 || (self.control & CTRL_RX_IRQ != 0 && !self.rx.is_empty())
    }

    pub fn read(&mut self, addr: u32) -> Option<u32> {
        match addr {
            DATA => Some(self.rx.pop_front().unwrap_or(0) as u32),
            STATUS => Some(TX_READY | if self.rx.is_empty() { RX_EMPTY } else { 0 }),
            a if a == BASE + 0x18 || a == BASE + 0x1C => Some(self.regs[((a - BASE - 0x18) / 4) as usize]),
            PORT_SELECT => Some(self.port_select),
            _ => None,
        }
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        match addr {
            DATA => self.transmit(value as u8),
            STATUS => self.control = value & 0xFF,
            a if a == BASE + 0x18 || a == BASE + 0x1C => self.regs[((a - BASE - 0x18) / 4) as usize] = value,
            PORT_SELECT => self.port_select = value,
            _ => return false,
        }
        true
    }

    fn transmit(&mut self, b: u8) {
        self.rx.push_back(b); // the echo on the shared wire
        if b == 0xFF && self.frame.len() != 1 {
            self.frame.clear();
        }
        self.frame.push(b);
        let [0xFF, cmd, data @ ..] = &self.frame[..] else { return };
        let (cmd, data) = (*cmd, data.to_vec());
        let complete = match command_data_len(cmd) {
            Some(n) => data.len() == n,
            None => data.is_empty(), // unknown: answer at once, and log it
        };
        if !complete {
            return;
        }
        let reply = if self.port() == 0 { self.reply(cmd, &data) } else { vec![] };
        if self.logging {
            self.log.push(format!("port {} cmd {cmd:02X} data {data:02X?} -> {reply:02X?}", self.port()));
        }
        self.rx.extend(reply);
        self.frame.push(0); // the transaction is over: ignore stray bytes
    }

    fn reply(&mut self, cmd: u8, data: &[u8]) -> Vec<u8> {
        match cmd {
            0xAA => with_checksum(&[0xAC, 0x00, 0x00]),
            0xA9 => vec![0xA9],
            0xA6 => with_checksum(&[[0xA3].as_slice(), &A6_ANSWER].concat()),
            0xA5 => {
                if self.memory.len() != MEMORY_LEN {
                    self.memory = default_memory();
                }
                with_checksum(&[[0xA5, MEMORY_LEN as u8].as_slice(), &self.memory].concat())
            }
            0xAC => {
                // [length, block, checksum]: keep the block.
                if data.len() == MEMORY_LEN + 2 {
                    self.memory = data[1..=MEMORY_LEN].to_vec();
                }
                vec![0xA9]
            }
            0x95 => {
                // Four 10-bit values (low 8 bits each, then their high bits
                // packed into one byte), decoded at 0x4005_5F50 (joystick
                // calibration?). The values are a HYPOTHESIS: a full range.
                let v: [u16; 4] = CALIBRATION;
                let high = ((v[0] >> 8) << 6 | (v[1] >> 8) << 4 | (v[2] >> 8) << 2 | v[3] >> 8) as u8;
                with_checksum(&[0x6A, v[0] as u8, v[1] as u8, v[2] as u8, v[3] as u8, high])
            }
            0xA3 => self.poll_reply(),
            _ => vec![],
        }
    }

    /// Input poll. The BIOS maps header bits 0-1 and data bits 0-7 to
    /// GIODataIn masks through its table at 0x400B_FA34 (decoder
    /// 0x4005_4E78); header bit 2 makes the nibble parity even. Bit 3 set =
    /// short form (buttons only, the pen counts as lifted); clear = long form
    /// with the pen (decoder 0x4005_667C): [hdr, buttons, p, x, y, hi, sum]
    /// where x = x | (hi & 0x38) << 5, y = y | (hi & 7) << 8 (11 bits) and
    /// pressure = (p | (hi & 0xC0) << 2) << 1. Low "pressure" puts the pen
    /// in range of the pad (after 2 polls at <= 0x7C0; out of range after 10
    /// above 0x7F0; thresholds from A6): the BIOS tracks it as a pointer.
    /// A click is data bit 7 (GIODataIn 0x0004_0000, the mask the TV driver
    /// reports at 0x4005_0044): pressed while the pen is in range, the BIOS
    /// posts the stylus-down event games use (0x1001_0002, 0x4004_A550).
    ///
    /// The stylus from a frontend becomes: pen in range, then after
    /// TIP_DELAY polls the tip goes down; on lifting, the tip goes up first
    /// and the pen leaves range LINGER polls later, so both clicks land
    /// while the BIOS sees the pen in range.
    fn poll_reply(&mut self) -> Vec<u8> {
        match self.pen {
            Some(p) => {
                self.pen_at = p;
                self.in_range = self.in_range.saturating_add(1);
                self.linger = LINGER;
            }
            None if self.linger > 0 => self.linger -= 1,
            None => self.in_range = 0,
        }
        let tip = self.pen.is_some() && self.in_range > TIP_DELAY;
        let pressed = self.pressed | if tip { CLICK } else { 0 };
        let bit = |i: usize| (pressed & BUTTON_MAP[i] != 0) as u8;
        let low = bit(0) | bit(1) << 1;
        let data = (0..8).fold(0u8, |d, i| d | bit(2 + i) << i);
        let parity = |nibble: u8| (nibble.count_ones() as u8 & 1) << 2;
        if self.in_range == 0 {
            return with_checksum(&[0x58 | low | parity(0x8 | low), data]);
        }
        let (px, py) = self.pen_at;
        let (x, y) = (pad_coordinate(px), pad_coordinate(py));
        let hi = ((x >> 8) << 3 | y >> 8) as u8; // pressure 0: its high bits are 0
        with_checksum(&[0x50 | low | parity(low), data, 0, x as u8, y as u8, hi])
    }
}

/// Raw 11-bit pad coordinate for a screen pixel (0-159) under the ideal
/// calibration in `default_memory`: the BIOS halves it, then maps
/// u = x * 160 / 1024 (rounded), so x/2 = pixel * 1024 / 160 lands on it.
fn pad_coordinate(pixel: u16) -> u16 {
    2 * (pixel.min(159) as u32 * 1024 / 160) as u16
}

/// The controller memory block (37 bytes; byte i is the BIOS's port-state
/// byte 0x52 + i). The BIOS maps the pen with it in mode 3 (0x4005_5D50):
/// u = (A x + B y + C) / D, v = (D' x + E y + F) / D, then
/// pixel = (160 u + S/2) / S, with S (i16) at 0x54/0x56, D at 0x58, A B C at
/// 0x5C-0x64, D' E F at 0x68-0x70 (i32), and a checksum at 0x76 making the
/// block sum to 0. A real controller stores its own pad's calibration
/// (written back with AC); this one is an ideal pad: identity, S = 1024.
fn default_memory() -> Vec<u8> {
    let mut m = vec![0u8; MEMORY_LEN];
    let mut put = |at: usize, bytes: &[u8]| m[at - 0x52..at - 0x52 + bytes.len()].copy_from_slice(bytes);
    put(0x54, &1024i16.to_le_bytes());
    put(0x56, &1024i16.to_le_bytes());
    put(0x58, &1i32.to_le_bytes());
    put(0x5C, &1i32.to_le_bytes());
    put(0x6C, &1i32.to_le_bytes());
    let sum = m.iter().fold(0u8, |s, &b| s.wrapping_add(b));
    m[MEMORY_LEN - 1] = sum.wrapping_neg();
    m
}

/// A6 answer after the ack (24 bytes + checksum), as the BIOS stores it
/// (0x4005_4FEE): bytes 1-4 the simple-mode pen scale/offset (sixteenths,
/// eighths) per axis, 5-6 / 7-8 the pen-down / pen-up pressure thresholds,
/// 9-12 debounce counts, 13-14 movement thresholds. Values: the BIOS's own
/// defaults where it has them (0x7C0, 0x7F0, 2 10 10 10, seen before any A6
/// answer), otherwise neutral; the rest UNKNOWN (0).
const A6_ANSWER: [u8; 24] = [16, 0, 16, 0, 0xC0, 0x07, 0xF0, 0x07, 2, 10, 10, 10, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

/// Appends the byte that makes the sum of all bytes 0 (mod 256).
fn with_checksum(bytes: &[u8]) -> Vec<u8> {
    let mut v = bytes.to_vec();
    v.push(bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b)).wrapping_neg());
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(bytes: &[u8]) -> u8 {
        bytes.iter().fold(0u8, |s, &b| s.wrapping_add(b))
    }

    /// Sends 0xFF, `cmd` and `data` on `port`; returns what the UART receives
    /// after the echoes (the controller's answer).
    fn transact(l: &mut TvLink, port: usize, cmd: u8, data: &[u8]) -> Vec<u8> {
        l.write(PORT_SELECT, if port == 0 { 0x200 } else { 0x100 });
        let sent = [[0xFF, cmd].as_slice(), data].concat();
        for &b in &sent {
            assert!(l.write(DATA, b as u32));
        }
        let got: Vec<u8> =
            std::iter::from_fn(|| (l.read(STATUS).unwrap() & RX_EMPTY == 0).then(|| l.read(DATA).unwrap() as u8))
                .collect();
        assert_eq!(&got[..sent.len()], &sent[..], "the wire echoes every byte");
        got[sent.len()..].to_vec()
    }

    #[test]
    fn handshake_answers_with_acks_lengths_and_checksums() {
        let mut l = TvLink::default();
        let hello = transact(&mut l, 0, 0xAA, &[0x00, 0x10, 0x46]);
        assert_eq!((hello[0], hello.len(), sum(&hello)), (0xAC, 4, 0));
        let info = transact(&mut l, 0, 0xA6, &[0x44; 15]);
        assert_eq!((info[0], info.len(), sum(&info)), (0xA3, 1 + 0x19, 0));
        let mem = transact(&mut l, 0, 0xA5, &[]);
        assert_eq!((mem[0], mem[1], mem.len(), sum(&mem)), (0xA5, 0x25, 40, 0));
        let cal = transact(&mut l, 0, 0x95, &[]);
        assert_eq!((cal[0], cal.len(), sum(&cal)), (0x6A, 7, 0));
        assert_eq!(transact(&mut l, 0, 0xA9, &[]), [0xA9]);
        assert!(transact(&mut l, 1, 0xAA, &[0x00, 0x10, 0x46]).is_empty(), "nothing on port 1");
    }

    #[test]
    fn memory_block_is_written_back_by_ac() {
        let mut l = TvLink::default();
        let mut block = vec![7u8; MEMORY_LEN];
        block[MEMORY_LEN - 1] = sum(&block[..MEMORY_LEN - 1]).wrapping_neg();
        let data = [[0x25].as_slice(), &block, &[0]].concat();
        assert_eq!(transact(&mut l, 0, 0xAC, &data), [0xA9]);
        assert_eq!(&transact(&mut l, 0, 0xA5, &[])[2..2 + MEMORY_LEN], &block[..]);
    }

    #[test]
    fn buttons_travel_in_short_packets() {
        let mut l = TvLink::default();
        assert_eq!(transact(&mut l, 0, 0xA3, &[]), [0x5C, 0x00, 0xA4], "idle: even-parity header, no buttons");
        l.pressed = 0x200 | 0x4000; // Left, A
        let p = transact(&mut l, 0, 0xA3, &[]);
        assert_eq!((p[0], p[1], sum(&p)), (0x59, 1 << 3, 0));
    }

    #[test]
    fn stylus_puts_the_pen_in_range_then_clicks() {
        let mut l = TvLink::default();
        l.pen = Some((12, 44));
        let polls: Vec<Vec<u8>> = (0..6).map(|_| transact(&mut l, 0, 0xA3, &[])).collect();
        for p in &polls {
            assert_eq!((p[0] & 0x08, p.len(), sum(p)), (0, 7, 0), "long form while in range");
        }
        let clicks: Vec<bool> = polls.iter().map(|p| p[1] & 0x80 != 0).collect();
        assert_eq!(clicks, [false, false, false, true, true, true]);
        l.pen = None;
        let after: Vec<Vec<u8>> = (0..5).map(|_| transact(&mut l, 0, 0xA3, &[])).collect();
        assert!(after[..3].iter().all(|p| p.len() == 7 && p[1] & 0x80 == 0), "tip up while still in range");
        assert!(after[3..].iter().all(|p| p.len() == 3), "then out of range");
    }

    /// The BIOS's mode-3 mapping (0x4005_5D50) with `default_memory`.
    fn bios_pixel(raw: u16, mem: &[u8]) -> i32 {
        let i32_at = |at: usize| i32::from_le_bytes(mem[at - 0x52..at - 0x52 + 4].try_into().unwrap());
        let s = i16::from_le_bytes(mem[0x54 - 0x52..0x56 - 0x52].try_into().unwrap()) as i32;
        let u = (i32_at(0x5C) * (raw as i32 >> 1) + i32_at(0x64)) / i32_at(0x58);
        ((160 * u + s / 2) / s).clamp(0, 159)
    }

    #[test]
    fn pad_coordinates_land_on_the_pixel_through_the_bios_mapping() {
        let mem = default_memory();
        assert_eq!(sum(&mem), 0, "the block carries its own checksum");
        for pixel in 0..160 {
            assert_eq!(bios_pixel(pad_coordinate(pixel), &mem), pixel as i32);
            assert!(pad_coordinate(pixel) < 0x800, "11-bit");
        }
    }
}
