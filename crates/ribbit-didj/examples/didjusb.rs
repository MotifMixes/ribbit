//! Research: plays the PC's part on the emulated Didj's USB, with no real
//! USB: boots the console, plugs the cable in, enumerates it, then sends
//! Bulk-Only mass-storage commands (INQUIRY, TEST UNIT READY until the
//! console offers /Didj, unlocking it as LeapFrog Connect does, READ CAPACITY, READ(10) of block 0), printing what
//! comes back and what the console says on its serial port.
//!
//!   cargo run --release -p ribbit-didj --example didjusb -- <folder of .lfp files> [million cycles before the cable]
//!
//! With RIBBIT_USBIP=<seconds> it instead serves the console over USB/IP
//! (port 3240) for that long, so the PC's own USB stack can attach it
//! (`usbip attach -r 127.0.0.1 -b 1-1`), printing each transfer.

use ribbit_didj::machine::Didj;
use ribbit_didj::udc::Transfer;

struct Host {
    didj: Box<Didj>,
    seq: u32,
    tag: u32,
    uart_seen: usize,
}

impl Host {
    fn run(&mut self, cycles: u64) {
        self.didj.run(cycles);
        let out = self.didj.bus.uart.first().map_or("", |s| s.as_str());
        if out.len() > self.uart_seen {
            print!("{}", &out[self.uart_seen..]);
            self.uart_seen = out.len();
        }
    }

    /// Runs the console until transfer `seq` finishes (at most `secs`).
    fn wait(&mut self, seq: u32, secs: f64) -> Option<(i32, Vec<u8>)> {
        let hz = self.didj.cpu_hz();
        for _ in 0..(secs * 1000.0) as u64 {
            self.run(hz / 1000);
            if let Some((_, status, data, _)) = self.didj.bus.udc.take_done().into_iter().find(|d| d.0 == seq) {
                return Some((status, data));
            }
        }
        None
    }

    fn submit(&mut self, endpoint: u8, dir_in: bool, setup: [u8; 8], data: Vec<u8>, length: u32) -> u32 {
        self.seq += 1;
        self.didj.bus.udc.submit(Transfer { seq: self.seq, endpoint, dir_in, setup, data, length });
        self.seq
    }

    fn control(&mut self, setup: [u8; 8]) -> Option<(i32, Vec<u8>)> {
        let length = u16::from_le_bytes([setup[6], setup[7]]) as u32;
        let seq = self.submit(0, setup[0] & 0x80 != 0, setup, Vec::new(), length);
        self.wait(seq, 2.0)
    }

    /// A Bulk-Only command: CBW out, data in (`length` bytes), CSW in.
    fn scsi(&mut self, out: u8, inp: u8, cdb: &[u8], length: u32) -> Option<(Vec<u8>, u8)> {
        self.tag += 1;
        let mut cbw = b"USBC".to_vec();
        cbw.extend(self.tag.to_le_bytes());
        cbw.extend(length.to_le_bytes());
        cbw.extend([if length > 0 { 0x80 } else { 0 }, 0, cdb.len() as u8]);
        cbw.extend(cdb);
        cbw.resize(31, 0);
        let s = self.submit(out, false, [0; 8], cbw, 31);
        self.wait(s, 5.0)?;
        let mut data = Vec::new();
        if length > 0 {
            let s = self.submit(inp, true, [0; 8], Vec::new(), length);
            data = self.wait(s, 5.0)?.1;
        }
        let s = self.submit(inp, true, [0; 8], Vec::new(), 13);
        let csw = self.wait(s, 5.0)?.1;
        (csw.len() == 13 && &csw[..4] == b"USBS").then(|| (data, csw[12]))
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: didjusb <folder of .lfp files> [million cycles before the cable]");
    let before: u64 = args.get(1).map_or(4000, |s| s.parse().expect("not a number"));
    let mut flash = ribbit_didj::nand_from_packages(dir.as_ref(), true).unwrap_or_else(|e| panic!("{e}"));
    let original = flash.clone();
    ribbit_didj::enhanced::apply(&mut flash, &original, true);
    flash.add_factory_data();
    let didj = Box::new(Didj::with_sdram(flash, ribbit_didj::enhanced::sdram_size(true)));
    let mut h = Host { didj, seq: 0, tag: 0, uart_seen: 0 };
    for _ in 0..before {
        h.run(1_000_000);
    }
    if let Some(secs) = std::env::var("RIBBIT_USBIP").ok().and_then(|v| v.parse::<f64>().ok()) {
        return serve(&mut h, secs);
    }
    println!("\n=== cable in");
    h.didj.bus.udc.attach(true);
    let device = h.control([0x80, 6, 0, 1, 0, 0, 18, 0]);
    println!("=== device descriptor {device:02X?}");
    let config = h.control([0x80, 6, 0, 2, 0, 0, 0xFF, 0]);
    println!("=== configuration {config:02X?}");
    // The bulk endpoints from the endpoint descriptors.
    let (mut out, mut inp) = (2, 1);
    if let Some((0, c)) = &config {
        let mut i = 0;
        while i + 1 < c.len() && c[i] > 0 {
            if c[i + 1] == 5 && i + 3 < c.len() {
                if c[i + 2] & 0x80 != 0 { inp = c[i + 2] & 0x7F } else { out = c[i + 2] }
            }
            i += c[i] as usize;
        }
    }
    println!("=== bulk out {out}, in {inp}");
    println!("=== string 0 {:02X?}", h.control([0x80, 6, 0, 3, 0, 0, 0xFF, 0]));
    println!("=== set configuration {:?}", h.control([0, 9, 1, 0, 0, 0, 0, 0]));
    println!("=== max LUN {:02X?}", h.control([0xA1, 0xFE, 0, 0, 0, 0, 1, 0]));
    let inquiry = h.scsi(out, inp, &[0x12, 0, 0, 0, 36, 0], 36);
    println!("=== INQUIRY {:?}", inquiry.as_ref().map(|(d, s)| (String::from_utf8_lossy(&d[8.min(d.len())..]).into_owned(), *s)));
    let mut ready = false;
    for i in 0..60 {
        match h.scsi(out, inp, &[0; 6], 0) {
            Some((_, 0)) => {
                println!("=== TEST UNIT READY: ready after {i} tries");
                ready = true;
                break;
            }
            r => {
                let sense = h.scsi(out, inp, &[3, 0, 0, 0, 18, 0], 18);
                println!("=== TEST UNIT READY {:?}, sense {:02X?}", r.map(|r| r.1), sense.as_ref().map(|(d, _)| d.get(2..14).map(<[u8]>::to_vec)));
                // Medium not present: the disk is locked until the PC
                // unlocks it (file_storage.c LF_UNLOCK_DEVICE, 0xC2), as
                // LeapFrog Connect does.
                if sense.as_ref().is_some_and(|(d, _)| d.get(12) == Some(&0x3A)) {
                    println!("=== UNLOCK {:?}", h.scsi(out, inp, &[0xC2, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0).map(|r| r.1));
                }
                h.run(h.didj.cpu_hz() / 2);
            }
        }
    }
    if !ready {
        return;
    }
    let cap = h.scsi(out, inp, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8);
    println!("=== READ CAPACITY {cap:02X?}");
    if let Some((block0, status)) = h.scsi(out, inp, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0], 512) {
        println!("=== READ(10) block 0: status {status}, {} bytes, OEM {:?}, ends {:02X?}", block0.len(), String::from_utf8_lossy(&block0[3..11.min(block0.len())]), &block0[510.min(block0.len())..]);
    }
    println!("\n=== cable out");
    h.didj.bus.udc.attach(false);
    for _ in 0..2000 {
        h.run(1_000_000);
    }
}

/// Serves the console over USB/IP for `secs` of emulated time (the cable is
/// in while a client has it imported).
fn serve(h: &mut Host, secs: f64) {
    use ribbit_twist::usbip::{DeviceInfo, Event, Server};
    let info = DeviceInfo {
        busid: "1-1".into(),
        vendor: 0x0F63,
        product: 0x0800,
        bcd_device: 0x0318,
        class: (0, 0, 0),
        configuration: 1,
        num_configurations: 1,
        interfaces: vec![(8, 6, 0x50)],
        speed: 3,
    };
    let srv = Server::start(info, ribbit_twist::usbip::PORT).expect("port 3240");
    println!("=== serving on 3240");
    let hz = h.didj.cpu_hz();
    for _ in 0..(secs * 1000.0) as u64 {
        while let Some(e) = srv.next_event() {
            match e {
                Event::Attached => {
                    println!("=== attached");
                    h.didj.bus.udc.attach(true);
                }
                Event::Detached => {
                    println!("=== detached");
                    h.didj.bus.udc.attach(false);
                }
                Event::Submit(u) => {
                    if u.endpoint == 0 {
                        println!("=== control {:02X?}", u.setup);
                    } else if !u.dir_in && u.data.len() == 31 && &u.data[..4] == b"USBC" {
                        println!("=== SCSI {:02X?}", &u.data[15..15 + u.data[14] as usize]);
                    }
                    h.didj.bus.udc.submit(Transfer { seq: u.seq, endpoint: u.endpoint, dir_in: u.dir_in, setup: u.setup, data: u.data, length: u.length });
                }
                Event::Unlink(seq) => {
                    println!("=== unlink {seq}: {}", h.didj.bus.udc.unlink(seq));
                }
            }
        }
        h.run(hz / 1000);
        for (seq, status, data, length) in h.didj.bus.udc.take_done() {
            if status != 0 {
                println!("=== urb {seq} status {status}");
            }
            srv.complete(seq, status, data, length);
        }
    }
}
