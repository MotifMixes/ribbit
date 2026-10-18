//! The USB device controller (0xC00E0000), as the System's driver uses it
//! (docs/rockit-twist.md, "USB"; worked out from the driver at
//! 0x4E000-0x56200). The firmware polls it from its USB task: it never
//! takes an interrupt. Over it the console is a USB mass-storage device
//! (Bulk-Only Transport, SCSI) whose LUN 0 is a 512-byte-block disk; LeapFrog
//! Connect sends its file-protocol commands as WRITE(10)s and reads the
//! replies with READ(10)s (hooks 0x25CF8 and 0x263C8).
//!
//! The host side is a queue of transfers (`usbip::Urb`) for endpoint 0
//! (control), 0x02 (bulk OUT) and 0x81 (bulk IN); each finishes with a
//! `Completion`. Requests the firmware does not handle (SET_ADDRESS,
//! GET_STATUS, CLEAR_FEATURE...) the controller answers itself, as it must
//! on the real chip for enumeration to work.

use crate::machine::Bus;
use crate::usbip::{ECONNRESET, EPIPE, Urb};
use std::collections::VecDeque;

pub const BASE: u32 = 0xC00E_0000;
/// Bulk packet size (high speed; 0x1A4 bit 15 tells the firmware so).
const BULK_PACKET: usize = 512;

/// A finished transfer: (sequence number, status 0 or -errno, IN data,
/// bytes transferred).
pub type Completion = (u32, i32, Vec<u8>, u32);

/// A control transfer the firmware is handling.
#[derive(Debug)]
struct Control {
    urb: Urb,
    /// IN data gathered so far.
    data: Vec<u8>,
    /// The data stage's IN token has been raised (E8 bit 4).
    in_raised: bool,
}

#[derive(Default)]
pub struct Udc {
    /// The host is attached (the cable is in and someone listens).
    pub attached: bool,
    /// A bus reset has been sent for this connection.
    reset_sent: bool,
    address: u8,
    configuration: u8,
    control: Option<Control>,
    /// Control transfers waiting their turn (one at a time, and only once
    /// the firmware has seen the bus reset).
    control_queue: VecDeque<Urb>,
    /// Bytes the firmware wrote to the FIFOs since the last commit.
    ep0_fifo: Vec<u8>,
    bulk_in_fifo: Vec<u8>,
    /// The CBW waiting to be read through 0xD4.
    bulk_out_fifo: VecDeque<u8>,
    /// Host transfers waiting.
    out_urbs: VecDeque<(Urb, usize)>,
    in_urbs: VecDeque<(Urb, Vec<u8>)>,
    /// Packets the device has sent that no IN transfer took yet.
    in_packets: VecDeque<Vec<u8>>,
    /// An OUT DMA in progress: RAM address, bytes, bytes done.
    out_dma: Option<(u32, usize, usize)>,
    /// Endpoint 0x81 and 0x02 halted.
    stall_in: bool,
    stall_out: bool,
    /// 0xDC status bits the firmware waits on (see `dc_read`).
    dc_status_ready: bool,
    dc_after_20: bool,
    dc_in_data_done: bool,
    dc_400: bool,
    /// Finished transfers for the host.
    pub done: VecDeque<Completion>,
}

impl Udc {
    /// The value of 0xDC as the firmware polls it: packets are taken at
    /// once (bits 4 and 8 read 0), bit 2 once the status stage's OUT
    /// packet is in, 5 after a 0x20 write, 6 between the 0x40 and 0x80
    /// writes, 10 after a 0x400 write, 12 while a CBW waits in the FIFO.
    fn dc_read(&self) -> u32 {
        (self.dc_status_ready as u32) << 2
            | (self.dc_after_20 as u32) << 5
            | (self.dc_in_data_done as u32) << 6
            | (self.dc_400 as u32) << 10
            | (!self.bulk_out_fifo.is_empty() as u32) << 12
    }
}

impl Bus {
    fn udc_latch(&self, off: u32) -> u32 {
        self.latch(BASE + off)
    }

    fn udc_set(&mut self, off: u32, v: u32) {
        self.set_latch(BASE + off, v);
    }

    /// Raises an event bit in 0xE8 if its enable bit (0xE4) is on.
    fn udc_event(&mut self, bit: u32) {
        if self.udc_latch(0xE4) & bit != 0 {
            let v = self.udc_latch(0xE8) | bit;
            self.udc_set(0xE8, v);
        }
    }

    /// A register read, if the controller model answers it.
    pub(crate) fn udc_read(&mut self, a: u32) -> Option<u32> {
        let off = a - BASE;
        Some(match off {
            0xC8 => 0,
            0xD4 => self.udc.bulk_out_fifo.pop_front().unwrap_or(0) as u32,
            0xDC => self.udc.dc_read(),
            // High speed once a host is attached.
            0x1A4 => self.udc_latch(0x1A4) | if self.udc.attached { 0x8000 } else { 0 },
            _ => return None,
        })
    }

    /// A register write (after it is latched): `old` the value before.
    pub(crate) fn udc_write(&mut self, a: u32, now: u32, written: u32, old: u32) {
        let off = a - BASE;
        match off {
            // Write-1-to-clear status registers.
            0xE8 | 0xF0 | 0x178 | 0x1AC | 0x150 | 0x164 => {
                self.udc_set(off, old & !written);
                // The firmware arms the IN token by enabling it and clearing
                // any old one (E4 |= 0x14; E8 = 0x10). After a STALL it
                // stays enabled, so a token raised with the next SETUP is
                // cleared that way before it is handled: raise it again.
                // (Handling it disables it first: E4 &= ~0x10; E8 = 0x10.)
                if off == 0xE8
                    && written & 0x10 != 0
                    && self.udc_latch(0xE4) & 0x10 != 0
                    && let Some(c) = &mut self.udc.control
                    && c.data.is_empty()
                {
                    c.in_raised = false;
                }
            }
            0xCC => {
                self.udc.ep0_fifo.push(written as u8);
            }
            0xD0 => {
                self.udc.bulk_in_fifo.push(written as u8);
            }
            0xDC => self.udc_command(now),
            0xC4 => {
                if now & 0x200 != 0 {
                    self.udc_dma_in();
                }
                if now & 0x400 != 0 {
                    let (addr, len) = (self.udc_latch(0x1D0), self.udc_latch(0x1C8) as usize);
                    self.udc.out_dma = Some((addr, len, 0));
                    self.udc_feed_out();
                }
            }
            0xF8 => self.udc_stall_request(now),
            _ => {}
        }
    }

    /// A command written to 0xDC.
    fn udc_command(&mut self, v: u32) {
        match v {
            // Start of SETUP handling.
            0x1 => {}
            0x20 => self.udc.dc_after_20 = true,
            // One EP0 IN packet (0x160 bit 0 with no bytes: a zero-length one).
            0x10 => {
                let packet = std::mem::take(&mut self.udc.ep0_fifo);
                if let Some(c) = &mut self.udc.control {
                    c.data.extend_from_slice(&packet);
                }
            }
            // EP0 IN data stage over: the host's status OUT packet follows.
            0x40 => {
                self.udc.dc_in_data_done = true;
                self.udc.dc_status_ready = true;
                if let Some(c) = self.udc.control.take() {
                    let want = c.urb.length as usize;
                    let mut data = c.data;
                    data.truncate(want);
                    let n = data.len() as u32;
                    self.udc.done.push_back((c.urb.seq, 0, data, n));
                    self.udc_event(0x4);
                }
            }
            0x4 => self.udc.dc_status_ready = false,
            0x80 => {
                self.udc.dc_in_data_done = false;
                self.udc.dc_after_20 = false;
            }
            // A bulk IN packet.
            0x100 => {
                let packet = std::mem::take(&mut self.udc.bulk_in_fifo);
                self.udc_send_in(packet);
            }
            0x400 => self.udc.dc_400 = true,
            // Arm bulk OUT (after SET_CONFIGURATION, a CSW, an OUT DMA...): a
            // no-data control request is over.
            0x2000 => {
                if let Some(c) = &self.udc.control
                    && c.urb.length == 0
                {
                    let seq = c.urb.seq;
                    self.udc.control = None;
                    self.udc.done.push_back((seq, 0, Vec::new(), 0));
                }
            }
            _ => {}
        }
    }

    /// STALL requests (0xF8): bit 0 EP0, bit 1 bulk IN, bit 2 bulk OUT.
    /// The controller reports each sent at once (0x150 bits 8-10).
    fn udc_stall_request(&mut self, v: u32) {
        let mut sent = self.udc_latch(0x150);
        if v & 1 != 0 {
            if let Some(c) = self.udc.control.take() {
                self.udc.done.push_back((c.urb.seq, -EPIPE, Vec::new(), 0));
            }
            sent |= 0x100;
        }
        if v & 2 != 0 {
            self.udc.stall_in = true;
            sent |= 0x200;
        }
        if v & 4 != 0 {
            self.udc.stall_out = true;
            sent |= 0x400;
        }
        self.udc_set(0x150, sent);
    }

    /// A packet for bulk IN (from the FIFO or a DMA).
    fn udc_send_in(&mut self, packet: Vec<u8>) {
        self.udc.in_packets.push_back(packet);
    }

    /// A bulk IN DMA: the bytes at 0x1D0 go out at once, as packets.
    fn udc_dma_in(&mut self) {
        let (addr, len) = (self.udc_latch(0x1D0) as usize, self.udc_latch(0x1C8) as usize);
        let data = self.ram.get(addr..addr + len).map(<[u8]>::to_vec).unwrap_or_else(|| vec![0; len]);
        for p in data.chunks(BULK_PACKET) {
            self.udc.in_packets.push_back(p.to_vec());
        }
        let v = self.udc_latch(0x164) | 1;
        self.udc_set(0x164, v);
    }

    /// Moves host OUT data into an OUT DMA in progress.
    fn udc_feed_out(&mut self) {
        while let Some((addr, len, mut got)) = self.udc.out_dma {
            let Some((urb, used)) = self.udc.out_urbs.front_mut() else { return };
            let take = (urb.data.len() - *used).min(len - got);
            let at = addr as usize + got;
            if let Some(dst) = self.ram.get_mut(at..at + take) {
                dst.copy_from_slice(&urb.data[*used..*used + take]);
            }
            *used += take;
            got += take;
            if *used == urb.data.len() {
                let (urb, _) = self.udc.out_urbs.pop_front().unwrap();
                self.udc.done.push_back((urb.seq, 0, Vec::new(), urb.data.len() as u32));
            }
            if got >= len {
                self.udc.out_dma = None;
                let v = self.udc_latch(0x164) | 1;
                self.udc_set(0x164, v);
            } else {
                self.udc.out_dma = Some((addr, len, got));
            }
        }
    }

    /// The host side moves on: bus reset, IN tokens, CBWs, IN packets to
    /// waiting transfers. Called from the machine's tick, not from the
    /// firmware's register writes: it enables an event and then clears
    /// its flag (E4 |= 0x14; E8 = 0x10), so an event raised at once would be
    /// lost.
    pub(crate) fn udc_poll(&mut self) {
        let connected = self.udc.attached && self.udc_latch(0xC0) == 0x930 && self.udc_latch(0x1A4) & 2 == 0;
        if !connected {
            return;
        }
        if !self.udc.reset_sent {
            self.udc.reset_sent = true;
            self.udc_event(0x8000);
            return;
        }
        // The next control transfer, once the reset has been handled.
        if self.udc.control.is_none()
            && self.udc_latch(0xE8) & 0x8000 == 0
            && let Some(urb) = self.udc.control_queue.pop_front()
        {
            self.udc_setup(urb);
        }
        // The data stage of a control read: the host's first IN token.
        let in_enabled = self.udc_latch(0xE4) & 0x10 != 0;
        if let Some(c) = &mut self.udc.control
            && c.urb.dir_in
            && c.urb.length > 0
            && !c.in_raised
            && in_enabled
        {
            c.in_raised = true;
            self.udc_event(0x10);
        }
        // IN packets into waiting IN transfers.
        while let (Some(_), Some(_)) = (self.udc.in_packets.front(), self.udc.in_urbs.front()) {
            if self.udc.stall_in {
                let (urb, _) = self.udc.in_urbs.pop_front().unwrap();
                self.udc.done.push_back((urb.seq, -EPIPE, Vec::new(), 0));
                continue;
            }
            let packet = self.udc.in_packets.pop_front().unwrap();
            let (urb, data) = self.udc.in_urbs.front_mut().unwrap();
            data.extend_from_slice(&packet);
            if data.len() >= urb.length as usize || packet.len() < BULK_PACKET {
                let (urb, mut data) = self.udc.in_urbs.pop_front().unwrap();
                data.truncate(urb.length as usize);
                let n = data.len() as u32;
                self.udc.done.push_back((urb.seq, 0, data, n));
            }
        }
        if self.udc.stall_in
            && let Some((urb, _)) = self.udc.in_urbs.pop_front()
        {
            self.udc.done.push_back((urb.seq, -EPIPE, Vec::new(), 0));
        }
        // The bulk IN pipe is empty and the firmware waits to send the CSW.
        if self.udc.in_packets.is_empty() && self.udc_latch(0xE4) & 0x100 != 0 && self.udc_latch(0xE8) & 0x100 == 0 {
            self.udc_event(0x100);
        }
        // Bulk OUT: data for a DMA, or a CBW when the firmware waits for one.
        if self.udc.stall_out
            && let Some((urb, _)) = self.udc.out_urbs.pop_front()
        {
            self.udc.done.push_back((urb.seq, -EPIPE, Vec::new(), 0));
        }
        if self.udc.out_dma.is_some() {
            self.udc_feed_out();
        } else if self.udc.bulk_out_fifo.is_empty()
            && self.udc_latch(0xE4) & 0x400 != 0
            && self.udc_latch(0xE8) & 0x400 == 0
            && let Some((urb, 0)) = self.udc.out_urbs.front()
            && urb.data.len() == 31
        {
            let (urb, _) = self.udc.out_urbs.pop_front().unwrap();
            self.udc.bulk_out_fifo.extend(urb.data.iter().copied());
            self.udc.done.push_back((urb.seq, 0, Vec::new(), 31));
            self.udc_event(0x400);
        }
    }

    /// A transfer from the host.
    pub fn udc_submit(&mut self, urb: Urb) {
        match (urb.endpoint & 0x0F, urb.dir_in) {
            (0, _) => self.udc.control_queue.push_back(urb),
            (_, true) => self.udc.in_urbs.push_back((urb, Vec::new())),
            (_, false) => self.udc.out_urbs.push_back((urb, 0)),
        }
        self.udc_poll();
    }

    /// Cancels a transfer still waiting; true if it was.
    pub fn udc_unlink(&mut self, seq: u32) -> bool {
        let before = self.udc.in_urbs.len() + self.udc.out_urbs.len();
        self.udc.in_urbs.retain(|(u, _)| u.seq != seq);
        let queued = self.udc.control_queue.len();
        self.udc.control_queue.retain(|u| u.seq != seq);
        let mut gone_queued = queued != self.udc.control_queue.len();
        self.udc.out_urbs.retain(|(u, _)| u.seq != seq);
        let mut gone = before != self.udc.in_urbs.len() + self.udc.out_urbs.len() || std::mem::take(&mut gone_queued);
        if self.udc.control.as_ref().is_some_and(|c| c.urb.seq == seq) {
            self.udc.control = None;
            gone = true;
        }
        if gone {
            self.udc.done.push_back((seq, -ECONNRESET, Vec::new(), 0));
        }
        gone
    }

    /// The host attaches (true) or goes (false); attaching starts a new
    /// connection (bus reset once the firmware has its pull-up on).
    pub fn udc_attach(&mut self, on: bool) {
        let done = std::mem::take(&mut self.udc.done);
        self.udc = Udc { attached: on, done, ..Default::default() };
        self.usb_plugged = on;
        self.udc_poll();
    }

    /// A SETUP from the host: the standard requests the controller answers
    /// itself, the rest handed to the firmware.
    fn udc_setup(&mut self, urb: Urb) {
        let s = urb.setup;
        let (kind, request, value, index, length) =
            (s[0], s[1], u16::from_le_bytes([s[2], s[3]]), u16::from_le_bytes([s[4], s[5]]), u16::from_le_bytes([s[6], s[7]]));
        let answer = |d: &[u8]| d[..d.len().min(length as usize)].to_vec();
        let handled = match (kind, request) {
            (0x00, 0x05) => {
                self.udc.address = value as u8;
                Some(Vec::new())
            }
            (0x80..=0x82, 0x00) => Some(answer(&[0, 0])),
            (0x00..=0x02, 0x01) => {
                // CLEAR_FEATURE(ENDPOINT_HALT).
                if kind == 0x02 && value == 0 {
                    match index & 0xFF {
                        0x81 => self.udc.stall_in = false,
                        0x02 => self.udc.stall_out = false,
                        _ => {}
                    }
                }
                Some(Vec::new())
            }
            (0x00..=0x02, 0x03) => Some(Vec::new()),
            (0x80, 0x08) => Some(answer(&[self.udc.configuration])),
            (0x81, 0x0A) => Some(answer(&[0])),
            _ => None,
        };
        if let Some(data) = handled {
            let n = data.len() as u32;
            self.udc.done.push_back((urb.seq, 0, data, n));
            return;
        }
        if (kind, request) == (0x00, 0x09) {
            self.udc.configuration = value as u8;
        }
        // To the firmware: the SETUP registers, then E8 bit 0.
        self.udc_set(0x118, kind as u32);
        self.udc_set(0x11C, request as u32);
        self.udc_set(0x120, value as u32);
        self.udc_set(0x124, index as u32);
        self.udc_set(0x128, length as u32);
        self.udc.ep0_fifo.clear();
        self.udc.dc_status_ready = false;
        self.udc.dc_in_data_done = false;
        self.udc.control = Some(Control { urb, data: Vec::new(), in_raised: false });
        self.udc_event(0x1);
    }

    /// Transfers finished since the last call.
    pub fn udc_take_done(&mut self) -> Vec<Completion> {
        self.udc.done.drain(..).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Twist;

    fn setup(seq: u32, s: [u8; 8]) -> Urb {
        let length = u16::from_le_bytes([s[6], s[7]]) as u32;
        Urb { seq, endpoint: 0, dir_in: s[0] & 0x80 != 0, setup: s, data: Vec::new(), length }
    }

    #[test]
    fn reset_first_then_the_standard_requests_the_silicon_answers() {
        let mut t = Twist::new(&[]);
        t.bus.udc_attach(true);
        t.bus.udc_submit(setup(1, [0, 5, 7, 0, 0, 0, 0, 0]));
        assert!(t.bus.udc_take_done().is_empty(), "nothing before the firmware connects");
        // The driver's init: reset events on, mode set, pull-up on.
        t.bus.write(BASE + 0xE4, 0x8409, 4);
        t.bus.write(BASE + 0xC0, 0x930, 4);
        t.bus.udc_poll();
        assert_ne!(t.bus.read(BASE + 0xE8, 4) & 0x8000, 0, "bus reset");
        assert_eq!(t.bus.read(BASE + 0x1A4, 4) & 0x8000, 0x8000, "high speed");
        t.bus.udc_poll();
        assert!(t.bus.udc_take_done().is_empty(), "held until the reset is handled");
        t.bus.write(BASE + 0xE8, 0x8000, 4);
        t.bus.udc_poll();
        t.bus.udc_submit(setup(2, [0x80, 0, 0, 0, 0, 0, 2, 0]));
        t.bus.udc_poll();
        let done = t.bus.udc_take_done();
        assert_eq!(done, vec![(1, 0, vec![], 0), (2, 0, vec![0, 0], 2)]);
        // GET_DESCRIPTOR goes to the firmware: SETUP registers and E8 bit 0.
        t.bus.udc_submit(setup(3, [0x80, 6, 0, 1, 0, 0, 18, 0]));
        t.bus.udc_poll();
        assert_eq!((t.bus.read(BASE + 0x11C, 4), t.bus.read(BASE + 0x120, 4)), (6, 0x100));
        assert_ne!(t.bus.read(BASE + 0xE8, 4) & 1, 0);
    }
}
