//! A USB/IP server (the protocol of Linux's usbip, documented in its
//! Documentation/usb/usbip_protocol.rst): it offers one USB device to a
//! client on the same computer, such as usbip-win2 on Windows, which then
//! shows it to the system as if it were plugged in. Ribbit uses it to put the
//! emulated RockIt Twist on the PC's USB, so that LeapFrog Connect can talk
//! to it.
//!
//! The protocol is big-endian over TCP (port 3240). Before a device is in
//! use: OP_REQ_DEVLIST (the devices offered) and OP_REQ_IMPORT (take one).
//! Then the client sends USBIP_CMD_SUBMIT for each transfer (URB) and
//! USBIP_CMD_UNLINK to cancel one; the server answers each with
//! USBIP_RET_SUBMIT / USBIP_RET_UNLINK. Transfers are handed to the device
//! (`Server::next_urb`) and finished when it answers (`Server::complete`),
//! possibly much later (a bulk IN waits until the device has data).

use std::collections::HashSet;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

pub const PORT: u16 = 3240;
const VERSION: u16 = 0x0111;
const OP_REQ_DEVLIST: u16 = 0x8005;
const OP_REP_DEVLIST: u16 = 0x0005;
const OP_REQ_IMPORT: u16 = 0x8003;
const OP_REP_IMPORT: u16 = 0x0003;
const CMD_SUBMIT: u32 = 1;
const CMD_UNLINK: u32 = 2;
const RET_SUBMIT: u32 = 3;
const RET_UNLINK: u32 = 4;
/// Linux error numbers the protocol carries (negated).
pub const ECONNRESET: i32 = 104;
pub const EPIPE: i32 = 32;

/// What the device list and import replies say about the device.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    pub busid: String,
    pub vendor: u16,
    pub product: u16,
    pub bcd_device: u16,
    pub class: (u8, u8, u8),
    pub configuration: u8,
    pub num_configurations: u8,
    /// Interfaces' (class, subclass, protocol).
    pub interfaces: Vec<(u8, u8, u8)>,
    /// USB speed as Linux numbers it: 2 full, 3 high.
    pub speed: u32,
}

/// A transfer from the host.
#[derive(Clone, Debug, PartialEq)]
pub struct Urb {
    pub seq: u32,
    pub endpoint: u8,
    /// Device to host.
    pub dir_in: bool,
    /// The SETUP packet (control transfers on endpoint 0).
    pub setup: [u8; 8],
    /// OUT data; for IN, the most the host takes.
    pub data: Vec<u8>,
    pub length: u32,
}

/// What reaches the device: transfers, cancellations and the host going.
#[derive(Debug, PartialEq)]
pub enum Event {
    /// A client imported the device.
    Attached,
    Submit(Urb),
    Unlink(u32),
    Detached,
}

enum Reply {
    Submit { seq: u32, status: i32, data: Vec<u8>, length: u32 },
    Unlink { seq: u32, status: i32 },
}

/// The server: a listening thread, and the device's ends of the queues.
pub struct Server {
    events: Receiver<Event>,
    replies: Arc<Mutex<Option<Sender<Reply>>>>,
    /// Transfers the host is waiting on (an unlink of one not here is
    /// answered as already finished).
    pending: Arc<Mutex<HashSet<u32>>>,
    pub port: u16,
}

impl Server {
    /// Listens on 127.0.0.1:`port` (0: any free port) for clients.
    pub fn start(info: DeviceInfo, port: u16) -> std::io::Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port))?;
        let port = listener.local_addr()?.port();
        let (ev_tx, events) = channel();
        let replies: Arc<Mutex<Option<Sender<Reply>>>> = Arc::new(Mutex::new(None));
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let (r, p) = (replies.clone(), pending.clone());
        std::thread::Builder::new().name("usbip".into()).spawn(move || {
            for stream in listener.incoming().flatten() {
                let _ = serve(stream, &info, &ev_tx, &r, &p);
                *r.lock().unwrap() = None;
                p.lock().unwrap().clear();
                if ev_tx.send(Event::Detached).is_err() {
                    return;
                }
            }
        })?;
        Ok(Self { events, replies, pending, port })
    }

    /// The next transfer, cancellation or detach, if any (non-blocking).
    pub fn next_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }

    /// Finishes a transfer: `status` 0 or a negative Linux error number;
    /// for IN, `data` is what was read.
    pub fn complete(&self, seq: u32, status: i32, data: Vec<u8>, length: u32) {
        if !self.pending.lock().unwrap().remove(&seq) {
            return;
        }
        if let Some(tx) = self.replies.lock().unwrap().as_ref() {
            let _ = tx.send(Reply::Submit { seq, status, data, length });
        }
    }
}

fn be16(b: &[u8]) -> u16 {
    u16::from_be_bytes([b[0], b[1]])
}

fn be32(b: &[u8]) -> u32 {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]])
}

/// The device's description as both replies give it (without interfaces).
fn device_record(info: &DeviceInfo) -> Vec<u8> {
    let mut v = Vec::with_capacity(312);
    let mut path = [0u8; 256];
    let p = format!("/sys/devices/ribbit/{}", info.busid);
    path[..p.len().min(255)].copy_from_slice(&p.as_bytes()[..p.len().min(255)]);
    v.extend_from_slice(&path);
    let mut busid = [0u8; 32];
    busid[..info.busid.len().min(31)].copy_from_slice(&info.busid.as_bytes()[..info.busid.len().min(31)]);
    v.extend_from_slice(&busid);
    v.extend_from_slice(&1u32.to_be_bytes()); // busnum
    v.extend_from_slice(&2u32.to_be_bytes()); // devnum
    v.extend_from_slice(&info.speed.to_be_bytes());
    v.extend_from_slice(&info.vendor.to_be_bytes());
    v.extend_from_slice(&info.product.to_be_bytes());
    v.extend_from_slice(&info.bcd_device.to_be_bytes());
    v.extend_from_slice(&[info.class.0, info.class.1, info.class.2]);
    v.extend_from_slice(&[info.configuration, info.num_configurations, info.interfaces.len() as u8]);
    v
}

/// One client connection: the operations, then URB traffic.
fn serve(
    mut s: TcpStream,
    info: &DeviceInfo,
    events: &Sender<Event>,
    replies: &Arc<Mutex<Option<Sender<Reply>>>>,
    pending: &Arc<Mutex<HashSet<u32>>>,
) -> std::io::Result<()> {
    s.set_nodelay(true)?;
    loop {
        let mut op = [0u8; 8];
        s.read_exact(&mut op)?;
        match be16(&op[2..]) {
            OP_REQ_DEVLIST => {
                let mut r = Vec::new();
                r.extend_from_slice(&VERSION.to_be_bytes());
                r.extend_from_slice(&OP_REP_DEVLIST.to_be_bytes());
                r.extend_from_slice(&0u32.to_be_bytes());
                r.extend_from_slice(&1u32.to_be_bytes());
                r.extend_from_slice(&device_record(info));
                for i in &info.interfaces {
                    r.extend_from_slice(&[i.0, i.1, i.2, 0]);
                }
                s.write_all(&r)?;
                return Ok(());
            }
            OP_REQ_IMPORT => {
                let mut busid = [0u8; 32];
                s.read_exact(&mut busid)?;
                let wanted = String::from_utf8_lossy(&busid).trim_end_matches('\0').to_string();
                let ok = wanted == info.busid;
                let mut r = Vec::new();
                r.extend_from_slice(&VERSION.to_be_bytes());
                r.extend_from_slice(&OP_REP_IMPORT.to_be_bytes());
                r.extend_from_slice(&(if ok { 0u32 } else { 1 }).to_be_bytes());
                if ok {
                    r.extend_from_slice(&device_record(info));
                }
                s.write_all(&r)?;
                if !ok {
                    return Ok(());
                }
                let _ = events.send(Event::Attached);
                return urbs(s, events, replies, pending);
            }
            _ => return Ok(()),
        }
    }
}

/// The URB phase: a reader here, a writer thread for the replies.
fn urbs(
    mut s: TcpStream,
    events: &Sender<Event>,
    replies: &Arc<Mutex<Option<Sender<Reply>>>>,
    pending: &Arc<Mutex<HashSet<u32>>>,
) -> std::io::Result<()> {
    let (tx, rx) = channel::<Reply>();
    *replies.lock().unwrap() = Some(tx.clone());
    let mut w = s.try_clone()?;
    let writer = std::thread::spawn(move || {
        for reply in rx {
            let mut r = Vec::with_capacity(48);
            match reply {
                Reply::Submit { seq, status, data, length } => {
                    for v in [RET_SUBMIT, seq, 0, 0, 0] {
                        r.extend_from_slice(&v.to_be_bytes());
                    }
                    for v in [status, length as i32, 0, 0, 0] {
                        r.extend_from_slice(&v.to_be_bytes());
                    }
                    r.extend_from_slice(&[0; 8]);
                    r.extend_from_slice(&data);
                }
                Reply::Unlink { seq, status } => {
                    for v in [RET_UNLINK, seq, 0, 0, 0] {
                        r.extend_from_slice(&v.to_be_bytes());
                    }
                    r.extend_from_slice(&status.to_be_bytes());
                    r.extend_from_slice(&[0; 24]);
                }
            }
            if w.write_all(&r).is_err() {
                return;
            }
        }
    });
    let result = (|| -> std::io::Result<()> {
        loop {
            let mut h = [0u8; 48];
            s.read_exact(&mut h)?;
            let (cmd, seq, dir, ep) = (be32(&h[0..]), be32(&h[4..]), be32(&h[12..]), be32(&h[16..]));
            match cmd {
                CMD_SUBMIT => {
                    let length = be32(&h[24..]);
                    let mut setup = [0u8; 8];
                    setup.copy_from_slice(&h[40..48]);
                    let dir_in = dir == 1;
                    let mut data = Vec::new();
                    if !dir_in && length > 0 {
                        data = vec![0; length as usize];
                        s.read_exact(&mut data)?;
                    }
                    pending.lock().unwrap().insert(seq);
                    let urb = Urb { seq, endpoint: ep as u8, dir_in, setup, data, length };
                    if events.send(Event::Submit(urb)).is_err() {
                        return Ok(());
                    }
                }
                CMD_UNLINK => {
                    let victim = be32(&h[20..]);
                    let was = pending.lock().unwrap().remove(&victim);
                    if was {
                        let _ = events.send(Event::Unlink(victim));
                    }
                    let _ = tx.send(Reply::Unlink { seq, status: if was { -ECONNRESET } else { 0 } });
                }
                _ => return Ok(()),
            }
        }
    })();
    *replies.lock().unwrap() = None;
    drop(tx);
    let _ = s.shutdown(std::net::Shutdown::Both);
    let _ = writer.join();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> DeviceInfo {
        DeviceInfo {
            busid: "1-1".into(),
            vendor: 0x0F63,
            product: 0x1234,
            bcd_device: 0x0100,
            class: (0, 0, 0),
            configuration: 1,
            num_configurations: 1,
            interfaces: vec![(8, 6, 0x50)],
            speed: 3,
        }
    }

    #[test]
    fn lists_imports_and_carries_transfers() {
        let server = Server::start(info(), 0).unwrap();
        // The device list.
        let mut c = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        c.write_all(&[0x01, 0x11, 0x80, 0x05, 0, 0, 0, 0]).unwrap();
        let mut r = Vec::new();
        c.read_to_end(&mut r).unwrap();
        assert_eq!((be16(&r[2..]), be32(&r[8..])), (OP_REP_DEVLIST, 1));
        assert_eq!(&r[12 + 256..12 + 256 + 3], b"1-1");
        assert_eq!(be16(&r[12 + 300..]), 0x0F63, "vendor");
        assert_eq!(&r[r.len() - 4..], &[8, 6, 0x50, 0], "the mass-storage interface");
        // Import, then a GET_DESCRIPTOR control transfer and a bulk OUT.
        let mut c = TcpStream::connect(("127.0.0.1", server.port)).unwrap();
        let mut req = vec![0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0];
        let mut busid = [0u8; 32];
        busid[..3].copy_from_slice(b"1-1");
        req.extend_from_slice(&busid);
        c.write_all(&req).unwrap();
        let mut rep = [0u8; 8 + 312];
        c.read_exact(&mut rep).unwrap();
        assert_eq!(be32(&rep[4..]), 0, "imported");
        let submit = |seq: u32, dir: u32, ep: u32, len: u32, setup: [u8; 8], data: &[u8]| {
            let mut h = Vec::new();
            for v in [CMD_SUBMIT, seq, 0x10002, dir, ep, 0, len, 0, 0, 0] {
                h.extend_from_slice(&v.to_be_bytes());
            }
            h.extend_from_slice(&setup);
            h.extend_from_slice(data);
            h
        };
        c.write_all(&submit(7, 1, 0, 18, [0x80, 6, 0, 1, 0, 0, 18, 0], &[])).unwrap();
        c.write_all(&submit(8, 0, 2, 3, [0; 8], &[1, 2, 3])).unwrap();
        let mut got = Vec::new();
        // (The device-list connection's end may arrive among them.)
        while got.len() < 2 {
            match server.next_event() {
                Some(Event::Detached | Event::Attached) | None => {}
                Some(e) => got.push(e),
            }
        }
        let Event::Submit(u) = &got[0] else { panic!() };
        assert_eq!((u.seq, u.dir_in, u.setup[1], u.length), (7, true, 6, 18));
        assert_eq!(got[1], Event::Submit(Urb { seq: 8, endpoint: 2, dir_in: false, setup: [0; 8], data: vec![1, 2, 3], length: 3 }));
        server.complete(7, 0, vec![0x12, 1], 2);
        let mut ret = [0u8; 50];
        c.read_exact(&mut ret).unwrap();
        assert_eq!((be32(&ret[0..]), be32(&ret[4..]), be32(&ret[24..])), (RET_SUBMIT, 7, 2));
        assert_eq!(&ret[48..], &[0x12, 1]);
        // Cancelling the bulk transfer still waiting.
        let mut h = Vec::new();
        for v in [CMD_UNLINK, 9, 0x10002, 0, 0, 8] {
            h.extend_from_slice(&v.to_be_bytes());
        }
        h.extend_from_slice(&[0; 24]);
        c.write_all(&h).unwrap();
        let mut ret = [0u8; 48];
        c.read_exact(&mut ret).unwrap();
        assert_eq!((be32(&ret[0..]), be32(&ret[4..]), be32(&ret[20..]) as i32), (RET_UNLINK, 9, -ECONNRESET));
        loop {
            if let Some(e) = server.next_event() {
                assert_eq!(e, Event::Unlink(8));
                break;
            }
        }
    }
}
