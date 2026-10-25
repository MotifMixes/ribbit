//! Leapster 2: the USB controller at 0x0180_F000-0x0180_F3FF, in device mode,
//! and the host at the other end of the cable (the PC, through USB/IP in the
//! frontend).
//!
//! VERIFIED-FW (BIOS 0.6; Ribbit and LeapLoader research, see LeapLoader's
//! docs/usb-research.md): the controller is Freescale's full-speed "USB-FS
//! OTG" block (as in Kinetis K20 and ColdFire MCF5xxx; the BIOS calls it "Fly
//! Usb OTG"). Every register the driver uses sits where that block's
//! reference manual puts it, and the driver's buffer descriptors have its
//! format:
//!
//! | Offset | Register | Use by the BIOS |
//! |---|---|---|
//! | 0x00, 0x04 | PERID, IDCOMP | byte reads at init (0x400B_E38E) |
//! | 0x10 | OTGISTAT | written 0xFF at init (write 1 to clear); the handler 0x400C_04B0 acknowledges what it read |
//! | 0x14 | OTGICR | 0x8D at init: ID, session valid, B session end and A VBUS change interrupts |
//! | 0x18 | OTGSTAT | bit 2 B session end: the attach check 0x400B_CB0C returns `(F018 >> 2 & 1) ^ 1`; bit 7 ID at init |
//! | 0x1C | OTGCTL | 0x34 at init, then bits 0-1 pulsed around the attach check (UNKNOWN: reserved on Kinetis) |
//! | 0x80 | ISTAT | 1 USBRST, 2 ERROR, 4 SOFTOK, 8 TOKDNE, 0x10 SLEEP, 0x20 RESUME, 0x40 ATTACH, 0x80 STALL; write 1 to clear |
//! | 0x84 | INTEN | 0x9F after a bus reset (0x400C_6070) |
//! | 0x90 | STAT | the finished token: endpoint << 4, TX << 3, ODD << 2 |
//! | 0x94 | CTL | 1 USBEN, 2 ODDRST, 8 HOSTMODEEN, 0x20 TXSUSPENDTOKENBUSY |
//! | 0x98 | ADDR | the device address |
//! | 0x9C, 0xB0, 0xB4 | BDTPAGE1-3 | buffer descriptor table address bits 9-15, 16-23, 24-31 (0x400C_37B8) |
//! | 0xC0 + 4n | ENDPTn | 1 EPHSHK, 2 EPSTALL, 4 EPTXEN, 8 EPRXEN |
//!
//! The buffer descriptor table (in RAM, 512-byte aligned) has one 8-byte
//! descriptor per endpoint, direction and ping-pong half at `endpoint * 32 +
//! tx * 16 + odd * 8`: word 0 = byte count << 16 | OWN 0x80 | DATA1 0x40 |
//! DTS 0x08 | STALL 0x04, word 1 = buffer address (writer 0x400C_5E74). The
//! controller owns a descriptor while OWN is set; when a token is done it
//! clears OWN, writes the byte count and the token's PID into bits 2-5, and
//! raises TOKDNE with STAT naming the descriptor.
//!
//! HYPOTHESIS (0x400C_67BC returns 15 for controller 0, passed with the
//! handler to the OS's interrupt attach): CPU interrupt line 15.
//!
//! Model, beyond the manual (UNVERIFIED on the hardware):
//! * Tokens one at a time: a new one only after the driver has cleared
//!   TOKDNE (the real STAT FIFO holds four); then STAT reads 0.
//! * Data toggles are not checked: a packet always carries the toggle its
//!   descriptor expects.
//! * The cable: VBUS present sets OTGSTAT's session-valid bits (A VBUS valid,
//!   session valid, ID = B device) and clears B session end, raising their
//!   change bits; the host resets the bus 10 ms after the driver has set
//!   USBEN and the D+ pull-up (OTGCTL bit 7), then sends SET_ADDRESS itself
//!   (USB/IP clients do not).
//! * Timing: a token every 50 µs, SOF every 1 ms while USBEN is set.
//! * After the console starts (power-on or restart) the host waits until
//!   6.5 s have passed before it resets and addresses the device. VERIFIED-FW
//!   (BIOS 0.6), why: the SET ADDRESS handler (0x400B_D0DC) posts the USB
//!   task's attach event, and the task restarts the console into the base
//!   system (OS 0x1003 +0x38 = 0x4005_35A0) unless `iuc` (0x3C7F_AF54, set by
//!   ConnectMPI's DoUsbAttached 0x400D_9EA4) is 1. Only the task's attach
//!   state machine (0x400B_E0E0, state in [0x3C7F_AE7C] + 0x1C, one step per
//!   1.5 s timer tick set up at 0x400B_9D04) sets it: state 1 (~3.7 s after
//!   a start) discharges VBUS for 100 ms (OTGCTL bit 0), reads B session end
//!   (the cable check 0x400B_CB0C) and with the cable in goes to state 2,
//!   which (~5.2 s) calls DoUsbAttached(1); without it, DoUsbAttached(0).
//!   VERIFIED-EMU (examples/l2usb.rs): the same times with the user's SD
//!   card and flash, and with the cable in at power-on and a 6.5 s wait the
//!   host gets both disks. A host that addresses the
//!   device sooner (a PC does it within a second of the pull-up, which the
//!   driver's OTG state machine 0x400C_0034 turns on at ~0.7 s) restarts it
//!   every ~0.9 s for ever. So the real host must come later. Plugging in
//!   while a game or the menu runs restarts the console once, as the BIOS
//!   intends, and the second start keeps the host until it is ready. UNKNOWN
//!   on the hardware: why the real PC is later (a slower restart, or
//!   VBUS/session timing this model lacks).
//! * Registers the driver does not use read back what was written.

use ribbit_memory::Bus;
use std::collections::VecDeque;

pub const BASE: u32 = 0x0180_F000;
pub const END: u32 = 0x0180_F3FC;
pub const STATUS: u32 = 0x0180_F018;
/// CPU interrupt line (HYPOTHESIS, see the module notes).
pub const IRQ_LINE: u8 = 15;

/// OTGSTAT bits.
pub const A_VBUS_VALID: u8 = 1 << 0;
pub const B_SESSION_END: u8 = 1 << 2;
pub const SESSION_VALID: u8 = 1 << 3;
pub const LINE_STABLE: u8 = 1 << 5;
pub const ID: u8 = 1 << 7;
/// 0x0180_F018 bit 2 (B session end): no host attached.
pub const NOT_ATTACHED: u32 = B_SESSION_END as u32;

/// ISTAT bits.
pub const USBRST: u8 = 1 << 0;
pub const SOFTOK: u8 = 1 << 2;
pub const TOKDNE: u8 = 1 << 3;
pub const SLEEP: u8 = 1 << 4;
pub const STALL: u8 = 1 << 7;

/// CTL bits.
const USBEN: u8 = 1 << 0;
const ODDRST: u8 = 1 << 1;
const TXSUSPENDTOKENBUSY: u8 = 1 << 5;

/// OTGCTL bit 7: the D+ pull-up; with bit 5 (the D+ pull-down) clear, the
/// device shows itself on the bus. The BIOS writes 0xB4, then 0x94, once the
/// cable is in. (Bits 0-1 are pulsed by the USB task's attach check
/// 0x400B_E126 without a cable: UNKNOWN, reserved on Kinetis.)
const DP_HIGH: u8 = 1 << 7;
const DP_LOW: u8 = 1 << 5;

/// ENDPTn bits.
const EPSTALL: u8 = 1 << 1;

/// Buffer descriptor word 0 bits.
const BD_OWN: u32 = 1 << 7;
const BD_DATA1: u32 = 1 << 6;
const BD_STALL: u32 = 1 << 2;

/// Token PIDs as the controller writes them into a finished descriptor.
const PID_OUT: u32 = 0x1;
const PID_IN: u32 = 0x9;
const PID_SETUP: u32 = 0xD;

/// Peripheral-clock ticks (16 MHz) per token and per frame.
const TOKEN_TICKS: u64 = 800;
const FRAME_TICKS: u64 = 16_000;
/// The bus reset's length (10 ms).
const CONNECT_TICKS: u64 = 10 * FRAME_TICKS;
/// From the connect to the bus reset (USB 2.0 7.1.7.3: at least 100 ms).
const DEBOUNCE_TICKS: u64 = 100 * FRAME_TICKS;
/// The host addresses the device no earlier than this long after the
/// console started (power-on or restart): 6.5 s. See the module notes.
const READY_TICKS: u64 = 6_500 * FRAME_TICKS;
/// Full-speed packet size of every endpoint.
const PACKET: usize = 64;

/// A transfer from the host.
#[derive(Clone, Debug, PartialEq)]
pub struct Transfer {
    /// The host's number for it (USB/IP's sequence number).
    pub id: u32,
    /// Endpoint number (0-15); 0 is the control endpoint.
    pub endpoint: u8,
    /// Device to host (for endpoint 0, the SETUP packet's bit 7 decides).
    pub dir_in: bool,
    /// The SETUP packet (endpoint 0).
    pub setup: [u8; 8],
    /// OUT data.
    pub data: Vec<u8>,
    /// For IN: the most bytes the host takes.
    pub length: u32,
}

/// How a transfer ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Ok,
    /// The device stalled the endpoint.
    Stalled,
    /// Cancelled, or the cable was pulled or the console restarted.
    Cancelled,
}

/// A finished transfer: its id, how it ended, IN data, bytes transferred.
#[derive(Clone, Debug, PartialEq)]
pub struct Finished {
    pub id: u32,
    pub outcome: Outcome,
    pub data: Vec<u8>,
    pub actual: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Stage {
    Setup,
    DataIn,
    DataOut,
    /// The status stage: IN after an OUT or no data stage, else OUT.
    StatusIn,
    StatusOut,
    Bulk,
}

#[derive(Clone, Debug)]
struct Active {
    t: Transfer,
    stage: Stage,
    /// IN data so far, or OUT bytes sent so far (`sent`).
    got: Vec<u8>,
    sent: usize,
    /// The host's own request (bus enumeration): not reported.
    internal: bool,
}

/// The cable and the host's transfers. Not in save states: a loaded state
/// has no cable.
#[derive(Clone, Debug, Default)]
pub struct Host {
    /// The cable is in (VBUS present).
    pub attached: bool,
    /// Ticks since the console started (power-on or restart).
    uptime: u64,
    /// Ticks until the bus reset / until it ends (None = not due).
    reset_in: Option<u64>,
    resetting: bool,
    /// Enumerated since the last reset (SET_ADDRESS sent).
    addressed: bool,
    /// The last SET_CONFIGURATION the host sent, replayed after the console
    /// restarts (USB/IP clients do not enumerate again).
    configuration: Option<[u8; 8]>,
    /// Transfers per endpoint (index = endpoint | 0x10 for IN, bulk only;
    /// 0 = control).
    queues: Vec<VecDeque<Active>>,
    /// The next queue to try (round robin).
    turn: usize,
    pub done: VecDeque<Finished>,
    /// The device descriptor and configuration descriptor as the device sent
    /// them during enumeration (for reports).
    pub descriptors: Vec<Vec<u8>>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct UsbDevice {
    /// Registers 0x0180_F000-0x0180_F3FC (byte registers in word slots).
    pub regs: Vec<u32>,
    /// Ping-pong half the controller uses next, per endpoint and direction
    /// (endpoint * 2 + tx).
    pub odd: [bool; 32],
    /// Ticks until the next token and the next SOF.
    pub token_in: u64,
    pub frame_in: u64,
    /// `service` has work (a tick count above reached 0).
    pub due: bool,
    pub frame: u16,
    /// Frames since power-on (SOFs).
    pub frames: u64,
    #[serde(skip)]
    pub host: Host,
    /// Log the tokens to stderr (research).
    #[serde(skip)]
    pub log: bool,
}

thread_local! {
    /// The save state being read is version 7 (`crate::STATE_VERSION`), from
    /// before this model: it holds the old stub's eight latches instead.
    pub(crate) static READING_V7: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Reads the controller from a save state of this or the previous version
/// (a version 7 state gets a new controller: the stub had no cable either).
pub(crate) fn deserialize_any_version<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<UsbDevice>, D::Error> {
    use serde::Deserialize;
    if READING_V7.with(|v| v.get()) {
        Ok(Option::<[u32; 8]>::deserialize(d)?.map(|_| UsbDevice::new()))
    } else {
        Option::<UsbDevice>::deserialize(d)
    }
}

fn index(addr: u32) -> Option<usize> {
    (BASE..=END).contains(&(addr & !3)).then(|| ((addr - BASE) / 4) as usize)
}

const OTGISTAT: usize = 0x10 / 4;
const OTGICR: usize = 0x14 / 4;
const OTGSTAT: usize = 0x18 / 4;
const OTGCTL: usize = 0x1C / 4;
const ISTAT: usize = 0x80 / 4;
const INTEN: usize = 0x84 / 4;
const ERRSTAT: usize = 0x88 / 4;
const STAT: usize = 0x90 / 4;
const CTL: usize = 0x94 / 4;
const ADDR: usize = 0x98 / 4;
const BDTPAGE1: usize = 0x9C / 4;
const FRMNUML: usize = 0xA0 / 4;
const FRMNUMH: usize = 0xA4 / 4;
const BDTPAGE2: usize = 0xB0 / 4;
const BDTPAGE3: usize = 0xB4 / 4;
const ENDPT0: usize = 0xC0 / 4;

impl UsbDevice {
    pub fn new() -> Self {
        let mut regs = vec![0; 0x100];
        // Kinetis K20's identification values (UNVERIFIED on the Leapster 2).
        regs[0] = 0x04;
        regs[1] = 0xFB;
        regs[2] = 0x33;
        regs[OTGSTAT] = (B_SESSION_END | ID | LINE_STABLE) as u32;
        Self { regs, token_in: u64::MAX, frame_in: u64::MAX, ..Default::default() }
    }

    fn reg(&self, i: usize) -> u8 {
        self.regs.get(i).copied().unwrap_or(0) as u8
    }

    pub fn read(&self, addr: u32) -> Option<u32> {
        let i = index(addr)?;
        Some(self.regs.get(i).copied().unwrap_or(0))
    }

    pub fn write(&mut self, addr: u32, value: u32) -> bool {
        let Some(i) = index(addr) else { return false };
        if self.regs.is_empty() {
            *self = Self::new();
        }
        let v = value & 0xFF;
        if self.log && i != ISTAT && i != OTGISTAT {
            eprintln!("usb: write {:#x} = {v:#04x}", i * 4);
        }
        match i {
            OTGISTAT | ISTAT | ERRSTAT => self.regs[i] &= !v,
            OTGSTAT | STAT | FRMNUML | FRMNUMH => {}
            CTL => {
                self.regs[i] = v;
                if v & ODDRST as u32 != 0 {
                    self.odd = [false; 32];
                }
                if v & USBEN as u32 != 0 && self.frame_in == u64::MAX {
                    self.frame_in = FRAME_TICKS;
                }
                self.wake();
            }
            OTGCTL => {
                let was = self.connected();
                self.regs[i] = v;
                if was != self.connected() {
                    self.log_line(if was { "disconnected" } else { "connected" });
                }
                self.wake();
            }
            _ => self.regs[i] = v,
        }
        if i == ISTAT && v & TOKDNE as u32 != 0 {
            // Acknowledging TOKDNE pops the finished token: STAT then reads
            // 0 (VERIFIED-FW: the handler 0x400C_5648 treats a non-zero STAT
            // as another finished token even with TOKDNE clear), and the next
            // token can come.
            self.regs[STAT] = 0;
            self.wake();
        }
        true
    }

    /// The controller's interrupt output.
    pub fn irq_line(&self) -> bool {
        self.reg(OTGISTAT) & self.reg(OTGICR) != 0 || self.reg(ISTAT) & self.reg(INTEN) != 0
    }

    /// Something to do at once (a register write or the host): `service`
    /// runs at the next sync.
    fn wake(&mut self) {
        if self.host.attached && self.token_in > TOKEN_TICKS {
            // Not at once: a packet takes bus time, and the driver clears
            // TOKDNE as its handler starts, before it has looked at the
            // finished descriptor.
            self.token_in = TOKEN_TICKS;
        }
    }

    /// The device shows itself on the bus.
    fn connected(&self) -> bool {
        self.reg(OTGCTL) & (DP_HIGH | DP_LOW) == DP_HIGH
    }

    /// Ticks until `service` must run.
    pub fn ticks_to_next(&self) -> u64 {
        if self.due { 0 } else { self.token_in.min(self.frame_in) }
    }

    pub fn advance(&mut self, ticks: u64) {
        self.host.uptime = self.host.uptime.saturating_add(ticks);
        for t in [&mut self.token_in, &mut self.frame_in] {
            if *t != u64::MAX {
                *t = t.saturating_sub(ticks);
                if *t == 0 {
                    self.due = true;
                }
            }
        }
    }

    // ---- The host's side (the frontend) ----

    /// Plugs the cable in or pulls it out. Pulling it cancels the host's
    /// transfers.
    pub fn plug(&mut self, attached: bool) {
        if self.regs.is_empty() {
            *self = Self::new();
        }
        if attached == self.host.attached {
            return;
        }
        let mut s = self.reg(OTGSTAT);
        if attached {
            s = (s | A_VBUS_VALID | SESSION_VALID) & !B_SESSION_END;
            self.host.attached = true;
            self.host.reset_in = None;
            self.host.addressed = false;
        } else {
            s = (s | B_SESSION_END) & !(A_VBUS_VALID | SESSION_VALID);
            self.cancel_all();
            self.host = Host { done: std::mem::take(&mut self.host.done), uptime: self.host.uptime, ..Default::default() };
            if self.reg(CTL) & USBEN != 0 {
                self.regs[ISTAT] |= SLEEP as u32;
            }
        }
        self.regs[OTGSTAT] = s as u32;
        // A VBUS, B session end and session valid changed.
        self.regs[OTGISTAT] |= 0x0D;
        self.token_in = 0;
        self.due = true;
    }

    /// The console restarted with the cable in: its controller is new, the
    /// host stays (and enumerates the device again).
    pub fn restarted(&mut self, mut host: Host) {
        self.log_line("the console restarted");
        for q in &mut host.queues {
            for a in q.drain(..) {
                if !a.internal {
                    host.done.push_back(Finished { id: a.t.id, outcome: Outcome::Cancelled, data: Vec::new(), actual: 0 });
                }
            }
        }
        // The cable was in before the controller started: VBUS is present,
        // but nothing changed (no change interrupt bits).
        host.reset_in = None;
        host.resetting = false;
        host.addressed = false;
        host.uptime = 0;
        if host.attached {
            self.regs[OTGSTAT] = ((self.reg(OTGSTAT) | A_VBUS_VALID | SESSION_VALID) & !B_SESSION_END) as u32;
        }
        self.host = host;
        self.token_in = 0;
        self.due = true;
    }

    pub fn submit(&mut self, t: Transfer) {
        let q = if t.endpoint == 0 { 0 } else { (t.endpoint as usize & 15) | (t.dir_in as usize) << 4 };
        let stage = if t.endpoint == 0 { Stage::Setup } else { Stage::Bulk };
        if t.endpoint == 0 && t.setup[0] == 0 && t.setup[1] == 9 {
            self.host.configuration = Some(t.setup);
        }
        self.queue(q, Active { t, stage, got: Vec::new(), sent: 0, internal: false });
    }

    fn queue(&mut self, q: usize, a: Active) {
        if self.host.queues.len() <= q {
            self.host.queues.resize_with(q + 1, VecDeque::new);
        }
        self.host.queues[q].push_back(a);
        self.wake();
    }

    /// Cancels a transfer; true if it was still waiting.
    pub fn unlink(&mut self, id: u32) -> bool {
        for q in &mut self.host.queues {
            if let Some(p) = q.iter().position(|a| a.t.id == id && !a.internal) {
                q.remove(p);
                return true;
            }
        }
        false
    }

    /// Cancels every transfer of the host's client (it went away); the
    /// cable stays in.
    pub fn cancel_all(&mut self) {
        for q in &mut self.host.queues {
            for a in q.drain(..) {
                if !a.internal {
                    self.host.done.push_back(Finished { id: a.t.id, outcome: Outcome::Cancelled, data: Vec::new(), actual: 0 });
                }
            }
        }
    }

    /// The host is waiting for the console: the cable's change not yet
    /// taken by the driver, the console still starting with the cable in,
    /// the connect (bus reset, enumeration) under way, or transfers queued.
    /// A paused frontend runs the console while this holds, as only the
    /// BIOS can answer.
    pub fn host_busy(&self) -> bool {
        let h = &self.host;
        h.attached
            && (self.reg(OTGISTAT) & self.reg(OTGICR) != 0
                || h.uptime < READY_TICKS
                || h.reset_in.is_some()
                || h.resetting
                || (self.connected() && !h.addressed)
                || h.queues.iter().any(|q| !q.is_empty()))
    }

    pub fn take_done(&mut self) -> Vec<Finished> {
        self.host.done.drain(..).collect()
    }

    // ---- The controller at work ----

    /// Runs what is due: SOF, the bus reset, the next token. Returns true if
    /// the interrupt output may have changed.
    pub fn service(&mut self, bus: &mut impl Bus) -> bool {
        self.due = false;
        let before = self.irq_line();
        let enabled = self.reg(CTL) & USBEN != 0;
        if self.frame_in == 0 {
            self.frame_in = if enabled { FRAME_TICKS } else { u64::MAX };
            if enabled && self.host.attached {
                self.frame = (self.frame + 1) & 0x7FF;
                self.frames += 1;
                self.regs[FRMNUML] = (self.frame & 0xFF) as u32;
                self.regs[FRMNUMH] = (self.frame >> 8) as u32;
                self.regs[ISTAT] |= SOFTOK as u32;
            }
            // The bus reset counts in frames.
            if let Some(t) = self.host.reset_in.as_mut() {
                *t = t.saturating_sub(FRAME_TICKS);
            }
        }
        if self.token_in == 0 {
            self.token_in = u64::MAX;
            if self.host.attached && enabled {
                self.run_host(bus);
            }
        }
        self.irq_line() != before
    }

    fn run_host(&mut self, bus: &mut impl Bus) {
        // The host sees the device while its D+ pull-up is on.
        if !self.connected() {
            if self.host.addressed || self.host.resetting || self.host.reset_in.is_some() {
                self.log_line("pull-up off: the host sees no device");
                self.host.addressed = false;
                self.host.resetting = false;
                self.host.reset_in = None;
            }
            return;
        }
        // Connect: 100 ms after the pull-up (the USB attach debounce), a bus
        // reset (10 ms), then SET_ADDRESS.
        match self.host.reset_in {
            None if !self.host.resetting && !self.host.addressed => {
                let ready = READY_TICKS.saturating_sub(self.host.uptime);
                self.host.reset_in = Some(DEBOUNCE_TICKS.max(ready));
                self.token_in = FRAME_TICKS;
                return;
            }
            Some(0) if !self.host.resetting => {
                self.log_line("bus reset");
                self.host.resetting = true;
                self.host.reset_in = Some(CONNECT_TICKS);
                self.regs[ISTAT] |= USBRST as u32;
                self.regs[ADDR] = 0;
                self.token_in = FRAME_TICKS;
                return;
            }
            Some(0) => {
                self.host.resetting = false;
                self.host.reset_in = None;
                self.host.addressed = true;
                self.enumerate();
            }
            Some(_) => {
                self.token_in = FRAME_TICKS;
                return;
            }
            None => {}
        }
        if self.reg(ISTAT) & TOKDNE != 0 || self.reg(CTL) & TXSUSPENDTOKENBUSY != 0 {
            // The driver has not taken the last token yet.
            self.token_in = TOKEN_TICKS;
            return;
        }
        let n = self.host.queues.len();
        for k in 0..n {
            let q = (self.host.turn + k) % n;
            if self.host.queues[q].is_empty() {
                continue;
            }
            self.host.turn = (q + 1) % n;
            if self.token(q, bus) {
                self.token_in = TOKEN_TICKS;
                return;
            }
        }
        // Everything NAKed: retry next token slot, if anything is waiting.
        if self.host.queues.iter().any(|q| !q.is_empty()) {
            self.token_in = TOKEN_TICKS;
        }
    }

    /// The host's own requests after a bus reset: SET_ADDRESS 1, the device
    /// and configuration descriptors (kept for reports), and the client's
    /// configuration again if it had set one.
    fn enumerate(&mut self) {
        let control = |id, setup: [u8; 8]| Active {
            t: Transfer { id, endpoint: 0, dir_in: setup[0] & 0x80 != 0, setup, data: Vec::new(), length: u16::from_le_bytes([setup[6], setup[7]]) as u32 },
            stage: Stage::Setup,
            got: Vec::new(),
            sent: 0,
            internal: true,
        };
        self.host.descriptors.clear();
        // Ahead of anything the client has queued.
        let mut first = vec![
            control(1, [0x00, 5, 1, 0, 0, 0, 0, 0]),
            control(2, [0x80, 6, 0, 1, 0, 0, 18, 0]),
            control(3, [0x80, 6, 0, 2, 0, 0, 255, 0]),
        ];
        if let Some(c) = self.host.configuration {
            first.push(control(4, c));
        }
        if self.host.queues.is_empty() {
            self.host.queues.push(VecDeque::new());
        }
        for a in first.into_iter().rev() {
            self.host.queues[0].push_front(a);
        }
    }

    fn bd_addr(&self, endpoint: usize, tx: bool) -> u32 {
        let table = (self.reg(BDTPAGE1) as u32 & 0xFE) << 8 | (self.reg(BDTPAGE2) as u32) << 16 | (self.reg(BDTPAGE3) as u32) << 24;
        let odd = self.odd[endpoint * 2 + tx as usize];
        table + endpoint as u32 * 32 + (tx as u32) * 16 + (odd as u32) * 8
    }

    /// One token for the head transfer of queue `q`. Returns false if the
    /// device NAKed it (nothing happened).
    fn token(&mut self, q: usize, bus: &mut impl Bus) -> bool {
        let a = self.host.queues[q].front().unwrap();
        let ep = a.t.endpoint as usize & 15;

        let (tx, pid) = match a.stage {
            Stage::Setup => (false, PID_SETUP),
            Stage::DataOut | Stage::StatusOut => (false, PID_OUT),
            Stage::DataIn | Stage::StatusIn => (true, PID_IN),
            Stage::Bulk if a.t.dir_in => (true, PID_IN),
            Stage::Bulk => (false, PID_OUT),
        };
        let setup = a.stage == Stage::Setup;
        let bd = self.bd_addr(ep, tx);
        let word0 = bus.read32(bd);
        let endpt = self.reg(ENDPT0 + ep);
        if word0 & BD_OWN == 0 {
            return false;
        }
        if !setup && (endpt & EPSTALL != 0 || word0 & BD_STALL != 0) {
            self.regs[ISTAT] |= STALL as u32;
            self.log_line(&format!("ep{ep} {} stalled", if tx { "IN" } else { "OUT" }));
            let a = self.host.queues[q].pop_front().unwrap();
            if !a.internal {
                self.host.done.push_back(Finished { id: a.t.id, outcome: Outcome::Stalled, data: Vec::new(), actual: 0 });
            }
            return true;
        }
        let buf = bus.read32(bd + 4);
        let room = (word0 >> 16 & 0x3FF) as usize;
        let a = self.host.queues[q].front_mut().unwrap();
        let count;
        if tx {
            // IN: the device's packet.
            count = room.min(PACKET);
            let data: Vec<u8> = (0..count as u32).map(|i| bus.read8(buf + i)).collect();
            a.got.extend_from_slice(&data);
        } else {
            let out: &[u8] = match a.stage {
                Stage::Setup => &a.t.setup,
                Stage::StatusOut => &[],
                _ => {
                    let end = a.t.data.len().min(a.sent + PACKET);
                    &a.t.data[a.sent..end]
                }
            };
            count = out.len().min(room);
            for (i, &b) in out[..count].iter().enumerate() {
                bus.write8(buf + i as u32, b);
            }
        }
        // The descriptor back to the driver: count, PID, its own toggle.
        bus.write32(bd, (count as u32) << 16 | word0 & BD_DATA1 | pid << 2);
        let o = &mut self.odd[ep * 2 + tx as usize];
        self.regs[STAT] = (ep as u32) << 4 | (tx as u32) << 3 | (*o as u32) << 2;
        *o = !*o;
        self.regs[ISTAT] |= TOKDNE as u32;
        if setup {
            self.regs[CTL] |= TXSUSPENDTOKENBUSY as u32;
            self.regs[ENDPT0] &= !(EPSTALL as u32);
        }
        if self.log {
            // Research (BIOS 0.6): the driver's endpoint record, [0x3C7F_AEE4]
            // + 0x18 (IN) / 0x1C (OUT) + 44 * endpoint; +0x23 bit 0 = its next
            // ping-pong half, +0x25 descriptors it has armed.
            let ctx = bus.read32(0x3C7F_AEE4);
            let mut r = Vec::new();
            for off in [0x18, 0x1C] {
                let rec = bus.read32(ctx + off) + 44 * ep as u32;
                r.push((0x20..0x29).map(|i| bus.read8(rec + i)).collect::<Vec<u8>>());
            }
            let other = bd ^ 8;
            eprintln!(
                "usb: ep{ep} {} {count} bytes (bd {bd:08x} was {word0:08x}, other {:08x}; records in {:02x?} out {:02x?})",
                ["OUT", "IN", "SETUP"][if setup { 2 } else { tx as usize }],
                bus.read32(other),
                r[0],
                r[1]
            );
        }
        self.advance_stage(q, count);
        true
    }

    /// After a packet of `count` bytes: the transfer's next stage, or done.
    fn advance_stage(&mut self, q: usize, count: usize) {
        let a = self.host.queues[q].front_mut().unwrap();
        let wanted = a.t.length as usize;
        let finished = match a.stage {
            Stage::Setup => {
                let len = u16::from_le_bytes([a.t.setup[6], a.t.setup[7]]) as usize;
                a.stage = match (len, a.t.setup[0] & 0x80 != 0) {
                    (0, _) => Stage::StatusIn,
                    (_, true) => Stage::DataIn,
                    (_, false) => Stage::DataOut,
                };
                false
            }
            Stage::DataIn => {
                if count < PACKET || a.got.len() >= wanted {
                    a.got.truncate(wanted);
                    a.stage = Stage::StatusOut;
                }
                false
            }
            Stage::DataOut => {
                a.sent += count;
                if a.sent >= a.t.data.len() || count < PACKET {
                    a.stage = Stage::StatusIn;
                }
                false
            }
            Stage::StatusIn | Stage::StatusOut => true,
            Stage::Bulk if a.t.dir_in => {
                if count < PACKET || a.got.len() >= wanted {
                    a.got.truncate(wanted);
                    true
                } else {
                    false
                }
            }
            Stage::Bulk => {
                a.sent += count;
                a.sent >= a.t.data.len()
            }
        };
        if finished {
            let a = self.host.queues[q].pop_front().unwrap();
            let actual = if a.t.dir_in { a.got.len() } else { a.sent } as u32;
            if a.internal {
                if a.t.setup[1] == 6 {
                    self.host.descriptors.push(a.got.clone());
                }
                self.log_line(&format!("enumeration request {:02x?} done: {:02x?}", a.t.setup, a.got));
            } else {
                self.host.done.push_back(Finished { id: a.t.id, outcome: Outcome::Ok, data: a.got, actual });
            }
        }
    }

    fn log_line(&self, s: &str) {
        if self.log {
            eprintln!("usb: {s}");
        }
        tracing::debug!(target: "ribbit::usb", "{s}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_attach_check_sees_no_host() {
        let u = UsbDevice::new();
        // The BIOS's check (0x400B_CB0C): (status >> 2 & 1) ^ 1.
        assert_eq!((u.read(STATUS).unwrap() >> 2 & 1) ^ 1, 0);
    }

    #[test]
    fn the_attach_check_sees_the_cable() {
        let mut u = UsbDevice::new();
        u.write(0x0180_F014, 0x8D);
        u.plug(true);
        assert_eq!((u.read(STATUS).unwrap() >> 2 & 1) ^ 1, 1);
        assert!(u.irq_line());
        // Acknowledged (write 1 to clear).
        u.write(0x0180_F010, 0xFF);
        assert!(!u.irq_line());
    }

    struct Mem(Vec<u8>);
    impl Bus for Mem {
        fn read8(&mut self, a: u32) -> u8 {
            self.0[a as usize]
        }
        fn read16(&mut self, a: u32) -> u16 {
            u16::from_le_bytes([self.read8(a), self.read8(a + 1)])
        }
        fn read32(&mut self, a: u32) -> u32 {
            self.read16(a) as u32 | (self.read16(a + 2) as u32) << 16
        }
        fn write8(&mut self, a: u32, v: u8) {
            self.0[a as usize] = v;
        }
        fn write16(&mut self, a: u32, v: u16) {
            self.write8(a, v as u8);
            self.write8(a + 1, (v >> 8) as u8);
        }
        fn write32(&mut self, a: u32, v: u32) {
            self.write16(a, v as u16);
            self.write16(a + 2, (v >> 16) as u16);
        }
    }

    /// Runs the controller for `ms` milliseconds; the time of the first bus
    /// reset in that stretch, in ms from its start.
    fn run_until_reset(u: &mut UsbDevice, mem: &mut Mem, ms: u64) -> Option<u64> {
        for t in 0..ms * FRAME_TICKS / TOKEN_TICKS {
            u.advance(TOKEN_TICKS);
            if u.ticks_to_next() == 0 {
                u.service(mem);
            }
            if u.reg(ISTAT) & USBRST != 0 {
                return Some(t * TOKEN_TICKS / FRAME_TICKS);
            }
        }
        None
    }

    /// The driver's start: USB enabled, D+ pull-up on (BIOS 0.6 ~0.7 s).
    fn driver_started() -> UsbDevice {
        let mut u = UsbDevice::new();
        u.write(0x0180_F094, USBEN as u32);
        u.write(0x0180_F01C, 0x94);
        u
    }

    #[test]
    fn the_host_waits_for_the_bios_after_a_start() {
        // Cable in from power-on: no bus reset (so no SET ADDRESS, whose
        // attach event restarts a console that has not checked the cable
        // yet) before 6.5 s.
        let mut mem = Mem(vec![0; 0x1000]);
        let mut u = driver_started();
        u.plug(true);
        let at = run_until_reset(&mut u, &mut mem, 8000).expect("a bus reset");
        assert!((6500..6600).contains(&at), "bus reset at {at} ms");
    }

    #[test]
    fn plugged_into_a_running_console_the_host_comes_at_once() {
        let mut mem = Mem(vec![0; 0x1000]);
        let mut u = driver_started();
        assert_eq!(run_until_reset(&mut u, &mut mem, 10_000), None);
        u.plug(true);
        let at = run_until_reset(&mut u, &mut mem, 1000).expect("a bus reset");
        assert!(at <= 150, "bus reset {at} ms after the plug");
        // The console restarts (the BIOS's attach reboot): the host waits again.
        let host = std::mem::take(&mut u.host);
        let mut u = driver_started();
        u.restarted(host);
        let at = run_until_reset(&mut u, &mut mem, 8000).expect("a bus reset");
        assert!((6500..6600).contains(&at), "bus reset {at} ms after the restart");
    }

    #[test]
    fn registers_are_byte_wide_and_status_is_write_one_to_clear() {
        let mut u = UsbDevice::new();
        assert!(u.write(0x0180_F01C, 0x34));
        assert_eq!(u.read(0x0180_F01C), Some(0x34));
        u.regs[ISTAT] = 0x0C;
        u.write(0x0180_F080, 0x08);
        assert_eq!(u.read(0x0180_F080), Some(0x04));
        assert!(u.write(STATUS, 0));
        assert_eq!(u.read(STATUS), Some(NOT_ATTACHED | 0xA0));
        assert_eq!(u.read(0x0180_F400), None);
    }
}
