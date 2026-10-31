//! The LF1000's USB device controller (0xC0018000, interrupt 20), as the
//! Didj's kernel driver uses it (opendidj linux-2.6.20-lf1000
//! drivers/usb/gadget/lf1000_udc.c; the firmware's kernel is 2.6.20.1 and
//! loads the same driver). The console's own g_file_storage (LeapFrog's
//! "Leapfrog Mass Storage", loaded at boot by /etc/init.d/mass_storage)
//! does the rest: Bulk-Only mass storage whose LUN is /Didj, enabled by
//! usbctl once the system sees the cable.
//!
//! Registers are 16 bits wide. The driver moves every byte by hand through
//! a FIFO port per endpoint (0x30 + 2 * endpoint), one packet at a time:
//! no DMA. Indexed registers (EPSTAT, EPCTL, BRCR, BWCR, MPR) act on the
//! endpoint in EPINDEX. Status bits clear by writing 1.
//!
//! The host side is a queue of transfers for endpoint 0 (control) and the
//! bulk endpoints; each finishes with a `Done`. What the chip does on its
//! own (the bus reset, the speed handshake, SET_ADDRESS's address) Ribbit
//! does here. The timings (the host's reset after the cable is in, the
//! status stage of a control transfer) are GUESSES; nothing in the driver
//! depends on them.

use std::collections::VecDeque;

pub const BASE: u32 = 0xC001_8000;
pub const END: u32 = 0xC001_8880;
pub const IRQ: u32 = 20;
/// ep0 and the bulk endpoints (the LF1000 has ep1 and ep2; the register
/// file has room for ep3).
const ENDPOINTS: usize = 4;
const EP0_PACKET: usize = 64;
const BULK_PACKET: usize = 512;

const EPINDEX: u32 = 0x0;
const EPINT: u32 = 0x2;
const EPINTEN: u32 = 0x4;
const FUNCADDR: u32 = 0x6;
const TEST: u32 = 0xC;
const SYSSTAT: u32 = 0xE;
const SYSCTL: u32 = 0x10;
const EP0STAT: u32 = 0x12;
const EP0CTL: u32 = 0x14;
const EPSTAT: u32 = 0x16;
const EPCTL: u32 = 0x18;
const BRCR: u32 = 0x1A;
const BWCR: u32 = 0x1C;
const MPR: u32 = 0x1E;
const FIFOS: u32 = 0x30;

/// TEST: the cable's 5 V is there.
const VBUS: u16 = 1 << 15;
// SYSSTAT
const VBUSOFF: u16 = 1 << 9;
const VBUSON: u16 = 1 << 8;
const HSP: u16 = 1 << 4;
const SDE: u16 = 1 << 3;
const HFSUSP: u16 = 1 << 1;
const HFRES: u16 = 1 << 0;
// SYSCTL: which SYSSTAT events interrupt.
const VBUSOFFEN: u16 = 1 << 11;
const VBUSONEN: u16 = 1 << 10;
const SPDEN: u16 = 1 << 6;
const HSUSPE: u16 = 1 << 1;
const HRESE: u16 = 1 << 0;
// EP0STAT
const EP0LWO: u16 = 1 << 6;
const SHT: u16 = 1 << 4;
const TST: u16 = 1 << 1;
const RSR: u16 = 1 << 0;
// EP0CTL
const EP0ESS: u16 = 1 << 1;
// EPSTAT: PSIF (bits 2-3) is the number of packets in the FIFO.
const EPLWO: u16 = 1 << 4;
const PSIF_SHIFT: u16 = 2;
const TPS: u16 = 1 << 1;
const RPS: u16 = 1 << 0;
// EPCTL
const FLUSH: u16 = 1 << 6;
const ESS: u16 = 1 << 1;

/// The host's reset follows the cable after this long (GUESS; a real host
/// waits ~100 ms for the connection to settle).
const RESET_AFTER_US: u64 = 10_000;
/// A control transfer with no data stage is answered this long after the
/// driver took its SETUP, if it did not stall it (GUESS: long enough for the
/// driver to set the stall in the same interrupt).
const STATUS_AFTER_US: u64 = 200;
/// A control IN whose last packet was full and filled less than the host
/// asked for (the driver sends no zero-length packet) ends after this.
const IN_QUIET_US: u64 = 2_000;

/// A transfer from the host.
#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    pub seq: u32,
    pub endpoint: u8,
    /// Device to host.
    pub dir_in: bool,
    /// The SETUP packet (endpoint 0).
    pub setup: [u8; 8],
    /// OUT data.
    pub data: Vec<u8>,
    /// IN: the most the host takes.
    pub length: u32,
}

/// A finished transfer: (sequence number, status 0 or -errno, IN data,
/// bytes transferred).
pub type Done = (u32, i32, Vec<u8>, u32);
pub const ECONNRESET: i32 = 104;
pub const EPIPE: i32 = 32;

/// Where the connection is: the events the driver must see in order.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
enum Stage {
    #[default]
    Unplugged,
    /// VBUSON raised; the host resets the bus at the time given.
    Vbus(u64),
    /// HFRES raised.
    Reset,
    /// SDE raised (high speed).
    Speed,
    /// Enumeration may start.
    Ready,
}

#[derive(Debug)]
struct Control {
    t: Transfer,
    data: Vec<u8>,
    /// When the driver cleared RSR (took the SETUP), and when it last sent
    /// a packet.
    taken: Option<u64>,
    last_packet: Option<u64>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Udc {
    /// Registers with no behaviour of their own (by halfword).
    regs: Vec<u16>,
    index: usize,
    epint: u16,
    sysstat: u16,
    ep0stat: u16,
    epstat: [u16; ENDPOINTS],
    epctl: [u16; ENDPOINTS],
    mpr: [u16; ENDPOINTS],
    bwcr: [u16; ENDPOINTS],
    /// The cable is in (a host imported the device).
    #[serde(skip)]
    pub attached: bool,
    #[serde(skip)]
    stage: Stage,
    /// Time in microseconds (`tick`), and the cycles towards the next one.
    #[serde(skip)]
    now: u64,
    #[serde(skip)]
    cycles: u64,
    /// The packet each endpoint's FIFO holds for the driver to read.
    #[serde(skip)]
    rx: [VecDeque<u8>; ENDPOINTS],
    /// Bytes the driver is writing (up to BWCR), and a written packet the
    /// host has not taken yet.
    #[serde(skip)]
    tx: [Vec<u8>; ENDPOINTS],
    #[serde(skip)]
    tx_ready: [Option<Vec<u8>>; ENDPOINTS],
    #[serde(skip)]
    control: Option<Control>,
    #[serde(skip)]
    control_queue: VecDeque<Transfer>,
    /// Host transfers per endpoint: OUT with the bytes already given.
    #[serde(skip)]
    outs: [VecDeque<(Transfer, usize)>; ENDPOINTS],
    #[serde(skip)]
    ins: [VecDeque<(Transfer, Vec<u8>)>; ENDPOINTS],
    #[serde(skip)]
    done: Vec<Done>,
}

impl Default for Udc {
    fn default() -> Self {
        Self {
            regs: vec![0; ((END - BASE) / 2) as usize],
            index: 0,
            epint: 0,
            sysstat: 0,
            ep0stat: 0,
            epstat: [0; ENDPOINTS],
            epctl: [0; ENDPOINTS],
            mpr: [0; ENDPOINTS],
            bwcr: [0; ENDPOINTS],
            attached: false,
            stage: Stage::Unplugged,
            now: 0,
            cycles: 0,
            rx: Default::default(),
            tx: Default::default(),
            tx_ready: Default::default(),
            control: None,
            control_queue: VecDeque::new(),
            outs: Default::default(),
            ins: Default::default(),
            done: Vec::new(),
        }
    }
}

impl Udc {
    fn reg(&self, off: u32) -> u16 {
        self.regs.get(off as usize / 2).copied().unwrap_or(0)
    }

    fn packet_size(&self, ep: usize) -> usize {
        if ep == 0 {
            return EP0_PACKET;
        }
        match self.mpr[ep] as usize & 0x7FF {
            0 => BULK_PACKET,
            n => n,
        }
    }

    /// Packets in the endpoint's FIFO (EPSTAT's PSIF).
    fn packets(&self, ep: usize) -> u16 {
        (!self.rx[ep].is_empty() || self.tx_ready[ep].is_some()) as u16
    }

    pub fn read(&mut self, off: u32, size: u32) -> u32 {
        if size == 4 {
            return self.read16(off) as u32 | (self.read16(off + 2) as u32) << 16;
        }
        let v = self.read16(off & !1) as u32;
        if off & 1 != 0 { v >> 8 } else { v & if size == 1 { 0xFF } else { 0xFFFF } }
    }

    fn read16(&mut self, off: u32) -> u16 {
        let ep = self.index;
        match off {
            EPINDEX => ep as u16,
            EPINT => self.epint,
            TEST => self.reg(TEST) & !VBUS | if self.attached { VBUS } else { 0 },
            SYSSTAT => self.sysstat | if matches!(self.stage, Stage::Speed | Stage::Ready) { HSP } else { 0 },
            EP0STAT => self.ep0stat | if self.rx[0].len() % 2 == 1 { EP0LWO } else { 0 },
            EPSTAT => self.epstat[ep] | self.packets(ep) << PSIF_SHIFT | if self.rx[ep].len() % 2 == 1 { EPLWO } else { 0 },
            EPCTL => self.epctl[ep],
            BRCR => self.rx[ep].len().div_ceil(2) as u16,
            BWCR => self.bwcr[ep],
            MPR => self.mpr[ep],
            o if (FIFOS..FIFOS + 2 * ENDPOINTS as u32).contains(&o) => {
                let rx = &mut self.rx[((o - FIFOS) / 2) as usize];
                let lo = rx.pop_front().unwrap_or(0) as u16;
                lo | (rx.pop_front().unwrap_or(0) as u16) << 8
            }
            _ => self.reg(off),
        }
    }

    pub fn write(&mut self, off: u32, v: u32, size: u32) {
        if size == 4 {
            self.write16(off, v as u16, 2);
            return self.write16(off + 2, (v >> 16) as u16, 2);
        }
        self.write16(off, v as u16, size)
    }

    fn write16(&mut self, off: u32, v: u16, size: u32) {
        let ep = self.index;
        match off {
            EPINDEX => self.index = v as usize % ENDPOINTS,
            EPINT => self.epint &= !v,
            SYSSTAT => self.sysstat &= !v,
            EP0STAT => self.ep0stat &= !(v & (RSR | TST | SHT)),
            EPSTAT => self.epstat[ep] &= !v,
            EPCTL => {
                if v & FLUSH != 0 {
                    self.rx[ep].clear();
                    self.tx[ep].clear();
                    self.tx_ready[ep] = None;
                }
                self.epctl[ep] = v & !FLUSH;
            }
            BWCR => {
                self.bwcr[ep] = v;
                self.tx[ep].clear();
            }
            MPR => self.mpr[ep] = v,
            o if (FIFOS..FIFOS + 2 * ENDPOINTS as u32).contains(&o) => {
                let ep = ((o - FIFOS) / 2) as usize;
                let want = self.bwcr[ep] as usize;
                for b in v.to_le_bytes().into_iter().take(size as usize) {
                    if self.tx[ep].len() < want {
                        self.tx[ep].push(b);
                    }
                }
                if want > 0 && self.tx[ep].len() == want {
                    self.commit(ep);
                }
            }
            SYSCTL => {
                // The cable was in before the driver listened for it (it
                // clears SYSSTAT first): it hears of it when it starts to.
                if self.attached && v & VBUSONEN != 0 && self.reg(SYSCTL) & VBUSONEN == 0 {
                    self.sysstat |= VBUSON;
                }
                self.regs[(off / 2) as usize] = v;
            }
            _ => {
                if let Some(r) = self.regs.get_mut(off as usize / 2) {
                    *r = v;
                }
            }
        }
    }

    /// The driver has written a whole packet.
    fn commit(&mut self, ep: usize) {
        let packet = std::mem::take(&mut self.tx[ep]);
        if ep != 0 {
            self.tx_ready[ep] = Some(packet);
            return;
        }
        // Endpoint 0: the data stage of the control transfer (sent at once;
        // TST tells the driver the FIFO is free for the next packet).
        let Some(c) = self.control.as_mut() else { return };
        let short = packet.len() < EP0_PACKET;
        c.data.extend_from_slice(&packet);
        c.last_packet = Some(self.now);
        self.ep0stat |= TST;
        self.epint |= 1;
        let wanted = u16::from_le_bytes([c.t.setup[6], c.t.setup[7]]) as usize;
        if short || c.data.len() >= wanted {
            self.finish_control(0);
        }
    }

    fn finish_control(&mut self, status: i32) {
        let Some(mut c) = self.control.take() else { return };
        let wanted = u16::from_le_bytes([c.t.setup[6], c.t.setup[7]]) as usize;
        c.data.truncate(wanted);
        let n = if c.t.dir_in { c.data.len() } else { c.t.data.len() } as u32;
        self.done.push((c.t.seq, status, if c.t.dir_in { c.data } else { Vec::new() }, n));
    }

    /// The interrupt line.
    pub fn irq(&self) -> bool {
        let ctl = self.reg(SYSCTL);
        let enabled = [(HRESE, HFRES), (HSUSPE, HFSUSP), (SPDEN, SDE), (VBUSONEN, VBUSON), (VBUSOFFEN, VBUSOFF)]
            .iter()
            .fold(0, |m, &(en, bit)| if ctl & en != 0 { m | bit } else { m });
        self.sysstat & enabled != 0 || self.epint & self.reg(EPINTEN) != 0
    }

    /// The host has work for the console, so the processor should not sleep
    /// past the next slice.
    pub fn busy(&self) -> bool {
        self.attached
            && (!matches!(self.stage, Stage::Ready)
                || self.control.is_some()
                || !self.control_queue.is_empty()
                || self.outs.iter().any(|q| !q.is_empty())
                || self.ins.iter().any(|q| !q.is_empty()))
    }

    /// Plugs the cable in (a host imported the device) or pulls it out.
    pub fn attach(&mut self, on: bool) {
        if on == self.attached {
            return;
        }
        self.attached = on;
        if on {
            self.sysstat |= VBUSON;
            self.stage = Stage::Vbus(self.now + RESET_AFTER_US);
            return;
        }
        self.sysstat |= VBUSOFF;
        self.sysstat &= !(VBUSON | HFRES | SDE);
        self.stage = Stage::Unplugged;
        self.control = None;
        self.control_queue.clear();
        self.outs.iter_mut().for_each(VecDeque::clear);
        self.ins.iter_mut().for_each(VecDeque::clear);
        self.rx.iter_mut().for_each(VecDeque::clear);
        self.tx_ready = Default::default();
        self.done.clear();
    }

    pub fn submit(&mut self, t: Transfer) {
        if !self.attached {
            return self.done.push((t.seq, -ECONNRESET, Vec::new(), 0));
        }
        let ep = t.endpoint as usize % ENDPOINTS;
        if ep == 0 {
            self.control_queue.push_back(t);
        } else if t.dir_in {
            self.ins[ep].push_back((t, Vec::new()));
        } else {
            self.outs[ep].push_back((t, 0));
        }
    }

    /// Cancels a transfer; false if it was not waiting (already done).
    pub fn unlink(&mut self, seq: u32) -> bool {
        if self.control.as_ref().is_some_and(|c| c.t.seq == seq) {
            self.control = None;
            self.rx[0].clear();
            return true;
        }
        let before = self.queued();
        self.control_queue.retain(|t| t.seq != seq);
        self.outs.iter_mut().for_each(|q| q.retain(|(t, _)| t.seq != seq));
        self.ins.iter_mut().for_each(|q| q.retain(|(t, _)| t.seq != seq));
        self.queued() != before
    }

    fn queued(&self) -> usize {
        self.control_queue.len() + self.outs.iter().map(VecDeque::len).sum::<usize>() + self.ins.iter().map(VecDeque::len).sum::<usize>()
    }

    /// Finished transfers since last asked.
    pub fn take_done(&mut self) -> Vec<Done> {
        std::mem::take(&mut self.done)
    }

    /// Moves the host's side on by `cycles` of a `hz` processor clock.
    pub fn tick(&mut self, cycles: u64, hz: u64) {
        if !self.attached {
            return;
        }
        self.cycles += cycles * 1_000_000;
        self.now += self.cycles / hz.max(1);
        self.cycles %= hz.max(1);
        let now = self.now;
        // The connection's events, each once the driver has seen the last.
        self.stage = match self.stage {
            Stage::Vbus(at) if now >= at && self.sysstat & VBUSON == 0 => {
                self.sysstat |= HFRES;
                self.regs[(FUNCADDR / 2) as usize] = 0;
                Stage::Reset
            }
            Stage::Reset if self.sysstat & HFRES == 0 => {
                self.sysstat |= SDE;
                Stage::Speed
            }
            Stage::Speed if self.sysstat & SDE == 0 => Stage::Ready,
            s => s,
        };
        if self.stage != Stage::Ready {
            return;
        }
        self.tick_control(now);
        for ep in 1..ENDPOINTS {
            self.tick_bulk(ep);
        }
    }

    fn tick_control(&mut self, now: u64) {
        if self.control.is_none() && self.ep0stat & RSR == 0 && self.rx[0].is_empty()
            && let Some(t) = self.control_queue.pop_front()
        {
            // SET_ADDRESS: the chip takes the address itself.
            if t.setup[0] == 0 && t.setup[1] == 5 {
                self.regs[(FUNCADDR / 2) as usize] = t.setup[2] as u16 & 0x7F;
            }
            self.rx[0].extend(t.setup);
            self.ep0stat |= RSR;
            self.epint |= 1;
            self.control = Some(Control { t, data: Vec::new(), taken: None, last_packet: None });
            return;
        }
        let stalled = self.reg(EP0CTL) & EP0ESS != 0;
        let Some(c) = self.control.as_mut() else { return };
        if c.taken.is_none() && self.ep0stat & RSR == 0 {
            c.taken = Some(now);
        }
        let Some(taken) = c.taken else { return };
        if stalled {
            // Stalled: the host hears of it, the driver sees SHT.
            self.ep0stat |= SHT;
            self.epint |= 1;
            return self.finish_control(-EPIPE);
        }
        let wanted = u16::from_le_bytes([c.t.setup[6], c.t.setup[7]]);
        let quiet = match (c.t.dir_in && wanted > 0, c.last_packet) {
            (false, _) => now >= taken + STATUS_AFTER_US,
            (true, Some(at)) => now >= at + IN_QUIET_US,
            (true, None) => false,
        };
        if quiet {
            self.finish_control(0);
        }
    }

    fn tick_bulk(&mut self, ep: usize) {
        let stalled = self.epctl[ep] & ESS != 0;
        let size = self.packet_size(ep);
        // OUT: the next packet once the driver has taken the last.
        if self.rx[ep].is_empty() && self.epstat[ep] & RPS == 0
            && let Some((t, given)) = self.outs[ep].front_mut()
        {
            if stalled {
                let (t, _) = self.outs[ep].pop_front().unwrap();
                return self.done.push((t.seq, -EPIPE, Vec::new(), 0));
            }
            let end = (*given + size).min(t.data.len());
            self.rx[ep].extend(&t.data[*given..end]);
            *given = end;
            if end > 0 || !t.data.is_empty() {
                self.epstat[ep] |= RPS;
                self.epint |= 1 << ep;
            }
            if *given >= t.data.len() {
                let (t, _) = self.outs[ep].pop_front().unwrap();
                self.done.push((t.seq, 0, Vec::new(), t.data.len() as u32));
            }
        }
        // IN: a written packet goes to the waiting transfer.
        if let Some((t, data)) = self.ins[ep].front_mut() {
            if stalled {
                let (t, _) = self.ins[ep].pop_front().unwrap();
                return self.done.push((t.seq, -EPIPE, Vec::new(), 0));
            }
            if let Some(packet) = self.tx_ready[ep].take() {
                let short = packet.len() < size;
                data.extend_from_slice(&packet);
                self.epstat[ep] |= TPS;
                self.epint |= 1 << ep;
                if short || data.len() >= t.length as usize {
                    let (t, mut data) = self.ins[ep].pop_front().unwrap();
                    data.truncate(t.length as usize);
                    let n = data.len() as u32;
                    self.done.push((t.seq, 0, data, n));
                }
            }
        }
    }
}
