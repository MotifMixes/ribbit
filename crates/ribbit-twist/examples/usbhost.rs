//! Research: plays the PC's part on the emulated RockIt Twist's USB, with
//! no real USB: boots the console, plugs the cable in, enumerates it, sends
//! mass-storage commands and then file-protocol commands (written to block
//! 0x20, the reply read back from it), printing what comes back.
//!
//! With RIBBIT_USBIP=1 it goes through the USB/IP server (`usbip`) over
//! TCP as a USB/IP client would (usbip-win2 does the same), the console
//! running on its own thread.
//!
//!   cargo run --release -p ribbit-twist --example usbhost -- <system dir> [content dir] [commands...]
//!
//! <system dir> holds the unpacked system packages (PCKT-0x004F0001... with
//! Firmware\Boot etc.); commands default to "INFO" and "LIST /".

use ribbit_twist::machine::{CLOCK, Twist};
use ribbit_twist::usbip::{Event, Server, Urb};
use std::io::{Read, Write};
use std::net::TcpStream;
use ribbit_twist::{fat, image};
use std::path::Path;

/// The console, here or across a USB/IP connection.
enum Link {
    Direct(Box<Twist>),
    Tcp(TcpStream),
}

struct Host {
    link: Link,
    seq: u32,
    tag: u32,
    /// IN transfers (their replies carry data).
    ins: std::collections::HashSet<u32>,
}

impl Host {
    /// Runs the console until transfer `seq` finishes (at most `secs`).
    fn wait(&mut self, seq: u32, secs: f64) -> Option<(i32, Vec<u8>)> {
        let t = match &mut self.link {
            Link::Direct(t) => t,
            Link::Tcp(c) => {
                c.set_read_timeout(Some(std::time::Duration::from_secs_f64(secs * 4.0))).unwrap();
                loop {
                    let mut h = [0u8; 48];
                    c.read_exact(&mut h).ok()?;
                    let be = |i: usize| u32::from_be_bytes([h[i], h[i + 1], h[i + 2], h[i + 3]]);
                    assert_eq!(be(0), 3, "RET_SUBMIT");
                    let (s, status, len) = (be(4), be(20) as i32, be(24) as usize);
                    // Only IN replies carry data.
                    let mut data = vec![0; if self.ins.contains(&s) { len } else { 0 }];
                    c.read_exact(&mut data).ok()?;
                    if s == seq {
                        return Some((status, data));
                    }
                    println!("    (transfer {s} done: {status}, {len} bytes)");
                }
            }
        };
        let end = t.bus.now + (secs * CLOCK as f64) as u64;
        let mut found = None;
        while t.bus.now < end {
            t.run(CLOCK / 2000);
            for (s, status, data, _) in t.bus.udc_take_done() {
                if s == seq {
                    found = Some((status, data));
                } else {
                    println!("    (transfer {s} done: {status}, {} bytes)", data.len());
                }
            }
            if found.is_some() {
                return found;
            }
        }
        None
    }

    fn submit(&mut self, endpoint: u8, dir_in: bool, setup: [u8; 8], data: Vec<u8>, length: u32) -> u32 {
        self.seq += 1;
        let seq = self.seq;
        if dir_in {
            self.ins.insert(seq);
        }
        match &mut self.link {
            Link::Direct(t) => t.bus.udc_submit(Urb { seq, endpoint, dir_in, setup, data, length }),
            Link::Tcp(c) => {
                let mut m = Vec::new();
                for v in [1, seq, 0x0001_0002, dir_in as u32, (endpoint & 0x0F) as u32, 0, length, 0, 0, 0] {
                    m.extend_from_slice(&v.to_be_bytes());
                }
                m.extend_from_slice(&setup);
                m.extend_from_slice(&data);
                c.write_all(&m).unwrap();
            }
        }
        seq
    }

    fn control(&mut self, setup: [u8; 8]) -> Option<(i32, Vec<u8>)> {
        let length = u16::from_le_bytes([setup[6], setup[7]]) as u32;
        let seq = self.submit(0, setup[0] & 0x80 != 0, setup, Vec::new(), length);
        self.wait(seq, 2.0)
    }

    /// One SCSI command over Bulk-Only Transport: (data in or out, CSW).
    fn scsi(&mut self, cdb: &[u8], data_in: u32, data_out: &[u8]) -> (Option<(i32, Vec<u8>)>, Option<(i32, Vec<u8>)>) {
        self.tag += 1;
        let len = if data_in > 0 { data_in } else { data_out.len() as u32 };
        let mut cbw = b"USBC".to_vec();
        cbw.extend_from_slice(&self.tag.to_le_bytes());
        cbw.extend_from_slice(&len.to_le_bytes());
        cbw.push(if data_in > 0 { 0x80 } else { 0 });
        cbw.push(0);
        cbw.push(cdb.len() as u8);
        let mut c = [0u8; 16];
        c[..cdb.len()].copy_from_slice(cdb);
        cbw.extend_from_slice(&c);
        let s = self.submit(2, false, [0; 8], cbw, 31);
        if self.wait(s, 2.0).is_none() {
            println!("    CBW not taken");
            return (None, None);
        }
        let data = if data_in > 0 {
            let s = self.submit(0x81, true, [0; 8], Vec::new(), data_in);
            self.wait(s, 5.0)
        } else if !data_out.is_empty() {
            let s = self.submit(2, false, [0; 8], data_out.to_vec(), data_out.len() as u32);
            self.wait(s, 5.0)
        } else {
            None
        };
        let s = self.submit(0x81, true, [0; 8], Vec::new(), 13);
        let csw = self.wait(s, 5.0);
        (data, csw)
    }
}

fn package(system: &Path, id: &str, file: &str) -> Vec<u8> {
    std::fs::read(system.join(id).join("Firmware").join(file)).unwrap_or_else(|e| panic!("{id}/{file}: {e}"))
}

fn text(d: &[u8]) -> String {
    d.iter().map(|&b| if (32..127).contains(&b) || b == b'\n' { b as char } else { '.' }).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let system = Path::new(&a[0]);
    let boot = package(system, "PCKT-0x004F0001-000000", "Boot");
    let base = package(system, "PCKT-0x004F0002-000000", "System");
    let mut volume = package(system, "PCKT-0x004F000B-000000", "FileSys");
    if let Some(dir) = a.get(1) {
        fat::add_host_folder(&mut volume, Path::new(dir)).expect("content");
    }
    let commands: Vec<String> = if a.len() > 2 { a[2..].to_vec() } else { vec!["INFO".into(), "LIST /".into()] };
    let mut t = Twist::boot(image::code(&boot), image::code(&base), &vec![0; 512], volume);
    // To the menu (or the Device Error screen), then plug in.
    t.run(20 * CLOCK);
    println!("plugging in at {:.1} s", t.bus.now as f64 / CLOCK as f64);
    let tcp = std::env::var("RIBBIT_USBIP").is_ok();
    let link = if tcp {
        // The console on its own thread behind the USB/IP server; this
        // thread imports the device over TCP, as usbip-win2 would.
        let info = ribbit_twist::usbip::DeviceInfo {
            busid: "1-1".into(),
            vendor: 0x0F63,
            product: 0x0047,
            bcd_device: 0x0100,
            class: (0, 0, 0),
            configuration: 1,
            num_configurations: 1,
            interfaces: vec![(8, 6, 0x50)],
            speed: 3,
        };
        let server = Server::start(info, 0).expect("listening");
        let port = server.port;
        std::thread::spawn(move || {
            loop {
                while let Some(e) = server.next_event() {
                    match e {
                        Event::Attached => t.bus.udc_attach(true),
                        Event::Detached => return,
                        Event::Submit(u) => t.bus.udc_submit(u),
                        Event::Unlink(s) => {
                            t.bus.udc_unlink(s);
                        }
                    }
                }
                t.run(CLOCK / 2000);
                for (s, status, data, len) in t.bus.udc_take_done() {
                    server.complete(s, status, data, len);
                }
            }
        });
        let mut c = TcpStream::connect(("127.0.0.1", port)).unwrap();
        let mut req = vec![0x01, 0x11, 0x80, 0x03, 0, 0, 0, 0];
        let mut busid = [0u8; 32];
        busid[..3].copy_from_slice(b"1-1");
        req.extend_from_slice(&busid);
        c.write_all(&req).unwrap();
        let mut rep = [0u8; 8 + 312];
        c.read_exact(&mut rep).unwrap();
        println!("imported over USB/IP (status {})", u32::from_be_bytes([rep[4], rep[5], rep[6], rep[7]]));
        Link::Tcp(c)
    } else {
        t.bus.udc_attach(true);
        Link::Direct(Box::new(t))
    };
    let mut h = Host { link, seq: 0, tag: 0, ins: Default::default() };
    let show = |name: &str, r: &Option<(i32, Vec<u8>)>| match r {
        Some((s, d)) => println!("{name}: status {s}, {} bytes {:02X?}", d.len(), &d[..d.len().min(40)]),
        None => println!("{name}: no answer"),
    };
    show("GET_DESCRIPTOR device", &h.control([0x80, 6, 0, 1, 0, 0, 18, 0]));
    show("SET_ADDRESS", &h.control([0, 5, 3, 0, 0, 0, 0, 0]));
    show("GET_DESCRIPTOR config", &h.control([0x80, 6, 0, 2, 0, 0, 0xFF, 0]));
    for i in 0..3u8 {
        let r = h.control([0x80, 6, i, 3, 9, 4, 0xFF, 0]);
        let s = r.as_ref().map(|(_, d)| d.iter().skip(2).step_by(2).map(|&c| c as char).collect::<String>());
        println!("string {i}: {s:?}");
    }
    show("SET_CONFIGURATION", &h.control([0, 9, 1, 0, 0, 0, 0, 0]));
    show("GET_MAX_LUN", &h.control([0xA1, 0xFE, 0, 0, 0, 0, 1, 0]));
    let (_, csw) = h.scsi(&[0, 0, 0, 0, 0, 0], 0, &[]);
    show("TEST UNIT READY csw", &csw);
    let (d, csw) = h.scsi(&[0x12, 0, 0, 0, 36, 0], 36, &[]);
    println!("INQUIRY: {:?}", d.as_ref().map(|(_, d)| text(d)));
    show("  csw", &csw);
    let (d, csw) = h.scsi(&[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, &[]);
    show("READ CAPACITY", &d);
    show("  csw", &csw);
    for cmd in commands {
        let mut block = cmd.clone().into_bytes();
        block.resize(512, 0);
        let (_, csw) = h.scsi(&[0x2A, 0, 0, 0, 0, 0x20, 0, 0, 1, 0], 0, &block);
        show(&format!("WRITE '{cmd}' csw"), &csw);
        // "102 BUSY": the console is still at it; ask again.
        for _ in 0..50 {
            let (d, _) = h.scsi(&[0x28, 0, 0, 0, 0, 0x20, 0, 0, 1, 0], 512, &[]);
            let reply = d.as_ref().map(|(_, d)| text(d).trim_end_matches('.').to_string()).unwrap_or_default();
            if reply.starts_with("102") {
                if let Link::Direct(t) = &mut h.link {
                    t.run(CLOCK / 50);
                }
                continue;
            }
            println!("reply: {reply:?}");
            // "101 EOF:" and "200 OK:" with a size: data follows at block 0x40.
            if let Some(size) = reply.split(|c: char| !c.is_ascii_digit()).filter(|w| !w.is_empty()).nth(1)
                && reply.starts_with("200 OK:")
            {
                let n: usize = size.parse().unwrap_or(0);
                let blocks = n.div_ceil(512).clamp(1, 32) as u8;
                let (d, _) = h.scsi(&[0x28, 0, 0, 0, 0, 0x40, 0, 0, blocks, 0], blocks as u32 * 512, &[]);
                println!("  data: {:?}", d.map(|(_, d)| text(&d[..n.min(d.len())])));
            }
            break;
        }
    }
    if let Link::Direct(t) = &h.link
        && !t.bus.uart.is_empty()
    {
        let u = String::from_utf8_lossy(&t.bus.uart);
        println!("\n== UART (last 40 lines)");
        for l in u.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev() {
            println!("  {l}");
        }
    }
}
