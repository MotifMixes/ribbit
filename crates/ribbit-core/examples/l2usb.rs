//! Plugs an emulated Leapster 2 into an emulated PC (io/usb.rs) and talks to
//! it as a USB mass-storage host would (research; the user's own BIOS):
//!   cargo run --release -p ribbit-core --example l2usb -- <bios> [cart|none] [state] [ms-before-plug]
//! Boots the system menu (or loads `state`), runs `ms-before-plug` ms
//! (default 4000), plugs the cable in, then: the descriptors the host read
//! while enumerating, SET_CONFIGURATION 1, GET MAX LUN, and per LUN INQUIRY,
//! the LeapFrog mount command (C2), TEST UNIT READY, READ CAPACITY and the
//! first sector. RIBBIT_USB_LOG=1 lists every token; RIBBIT_L2_FLASH=path loads
//! the internal flash (as the frontend's saved flash-<bios crc>.bin).
//! RIBBIT_L2_SD=path loads the SD card (the saved sdcard-<bios crc>.img).

use ribbit_core::Machine;
use ribbit_core::io::usb::{Outcome, Transfer};
use ribbit_debug::Breakpoints;

const MS: u64 = 96_000;

struct Host {
    m: Machine,
    id: u32,
    tag: u32,
    restarts: u64,
    uart_seen: usize,
    watched: (u32, u32, u8),
}

impl Host {
    fn run_ms(&mut self, ms: u64) {
        if std::env::var_os("RIBBIT_USB_WATCH").is_some() {
            // OTGCTL changes and the USB task's attach state (BIOS 0.6
            // 0x3C7F_AE98), per millisecond.
            use ribbit_memory::Bus as _;
            for _ in 0..ms {
                self.m.run_cycles(MS, &Breakpoints::default());
                let now = (self.m.io.usb.as_ref().unwrap().regs[7], self.m.bus.read32(0x3C7F_AE98), self.m.bus.read8(0x3C7F_AF54));
                if now != self.watched {
                    self.watched = now;
                    println!("  {:.3} s: OTGCTL {:02x} attach state {} iuc {}", self.m.cpu.cycles as f64 / (1000 * MS) as f64, now.0, now.1, now.2);
                }
            }
            return;
        }
        if std::env::var_os("RIBBIT_USB_ARMS").is_none() {
            self.m.run_cycles(ms * MS, &Breakpoints::default());
            return;
        }
        // Each descriptor the driver arms (0x400C_5F44: r0 = descriptor,
        // r2 = word 0, r7 = buffer).
        let arm = std::env::var("RIBBIT_USB_BP").map_or(0x400C_5F44, |v| u32::from_str_radix(v.trim_start_matches("0x"), 16).unwrap());
        let mut bp = Breakpoints::default();
        bp.add(arm);
        let end = self.m.cpu.cycles + ms * MS;
        while self.m.cpu.cycles < end {
            if self.m.cpu.pc == arm {
                let r = &self.m.cpu.regs;
                eprintln!("arm: r0 {:08x} r1 {:08x} r2 {:08x} r3 {:08x} r7 {:08x} (blink {:08x})", r[0], r[1], r[2], r[3], r[7], r[31]);
                if std::env::var_os("RIBBIT_USB_STACK").is_some() {
                    // Code addresses on the stack: the callers, roughly.
                    use ribbit_memory::Bus as _;
                    let sp = self.m.cpu.regs[28];
                    let calls: Vec<String> = (0..256)
                        .map(|i| self.m.bus.read32(sp + 4 * i))
                        .filter(|v| (0x4000_0000..0x4020_0000).contains(v))
                        .map(|v| format!("{v:08x}"))
                        .collect();
                    eprintln!("  stack: {}", calls.join(" "));
                    let setup: Vec<u8> = (0..8).map(|i| self.m.bus.read8(0x3C7F_AE6C + i)).collect();
                    eprintln!("  setup packet: {setup:02x?}");
                    // The USB task's event listeners (0x400B_9C1C): the
                    // list at [0x3C7F_AE00] + 4, nodes {+8 function, +0xC
                    // context, +0x10 byte, +0x1C next}.
                    let head = self.m.bus.read32(0x3C7F_AE00);
                    let mut n = self.m.bus.read32(head + 4);
                    while n != 0 && n != 0xFFFF_FFFF {
                        eprintln!("  listener {n:08x}: fn {:08x} ctx {:08x} flag {}", self.m.bus.read32(n + 8), self.m.bus.read32(n + 12), self.m.bus.read8(n + 16));
                        n = self.m.bus.read32(n + 0x1C);
                    }
                }
                self.m.run(1, &Breakpoints::default());
            }
            self.m.run_cycles(end - self.m.cpu.cycles, &bp);
        }
    }

    /// Submits a transfer and runs until it finishes (at most 3 s).
    fn transfer(&mut self, mut t: Transfer) -> Option<(Outcome, Vec<u8>)> {
        self.id += 1;
        t.id = self.id;
        self.m.usb_submit(t);
        for _ in 0..3000 {
            self.run_ms(1);
            let tx = &self.m.io.uart.tx;
            if tx.len() < self.uart_seen {
                self.uart_seen = 0;
            }
            if std::env::var_os("RIBBIT_UART").is_some() {
                print!("{}", String::from_utf8_lossy(&tx[self.uart_seen..]));
            }
            self.uart_seen = tx.len();
            if self.m.restarts != self.restarts {
                self.restarts = self.m.restarts;
                println!("  console restart {} at {:.3} s, B008 {:x?}", self.restarts, self.m.cpu.cycles as f64 / (1000 * MS) as f64, self.m.io.leapster2_power);
                use ribbit_memory::Bus as _;
                let msg: Vec<u8> = (0..0x100).map(|i| self.m.bus.read8(0x0300_FF00 + i)).collect();
                println!("    0x0300_FF00: \"{}\"", text(&msg));
            }
            if let Some(f) = self.m.usb_take_done().into_iter().find(|f| f.id == self.id) {
                return Some((f.outcome, f.data));
            }
        }
        self.m.usb_unlink(self.id);
        self.dump();
        None
    }

    /// The driver's endpoint records (BIOS 0.6: [0x3C7F_AEE4] + 0x18 IN,
    /// + 0x1C OUT, 44 bytes per endpoint; +0x23 bit 0 = next ping-pong half).
    fn records(&mut self, what: &str) {
        use ribbit_memory::Bus as _;
        let ctx = self.m.bus.read32(0x3C7F_AEE4);
        let mut line = format!("  [{what}] odd {:?}", &self.m.io.usb.as_ref().unwrap().odd[..4]);
        for (name, off) in [("in", 0x18), ("out", 0x1C)] {
            let base = self.m.bus.read32(ctx + off);
            for ep in 0..2 {
                let r = base + 44 * ep;
                let b: Vec<u8> = (0..0x2C).map(|i| self.m.bus.read8(r + i)).collect();
                line += &format!(" | ep{ep} {name}: st {} 0x20.. {}", b[0], hex(&b[0x20..0x2C]));
            }
        }
        println!("{line}");
    }

    /// The registers and buffer descriptors of endpoints 0-2.
    fn dump(&mut self) {
        use ribbit_memory::Bus as _;
        let u = self.m.io.usb.as_ref().unwrap();
        println!("  timed out: ISTAT {:02x} INTEN {:02x} STAT {:02x} CTL {:02x} ENDPT0-2 {:02x?} odd {:?}", u.regs[0x20], u.regs[0x21], u.regs[0x24], u.regs[0x25], &u.regs[0x30..0x33], &u.odd[..6]);
        let table = (u.regs[0x27] & 0xFE) << 8 | u.regs[0x2C] << 16 | u.regs[0x2D] << 24;
        for i in 0..12 {
            let a = table + 8 * i;
            println!("    bd {a:08x}: {:08x} {:08x}", self.m.bus.read32(a), self.m.bus.read32(a + 4));
        }
    }

    fn control(&mut self, setup: [u8; 8], data: Vec<u8>) -> Option<(Outcome, Vec<u8>)> {
        let length = u16::from_le_bytes([setup[6], setup[7]]) as u32;
        self.records("before");
        self.transfer(Transfer { id: 0, endpoint: 0, dir_in: setup[0] & 0x80 != 0, setup, data, length })
    }

    /// One Bulk-Only command: CBW out, data in (or out), CSW in.
    fn scsi(&mut self, lun: u8, cdb: &[u8], data_in: u32, data_out: Vec<u8>) -> Option<(Vec<u8>, Vec<u8>)> {
        self.tag += 1;
        let mut cbw = vec![0u8; 31];
        cbw[..4].copy_from_slice(b"USBC");
        cbw[4..8].copy_from_slice(&self.tag.to_le_bytes());
        let len = if data_in > 0 { data_in } else { data_out.len() as u32 };
        cbw[8..12].copy_from_slice(&len.to_le_bytes());
        cbw[12] = if data_in > 0 { 0x80 } else { 0 };
        cbw[13] = lun;
        cbw[14] = cdb.len() as u8;
        cbw[15..15 + cdb.len()].copy_from_slice(cdb);
        let out = |data: Vec<u8>| Transfer { id: 0, endpoint: 1, dir_in: false, setup: [0; 8], length: data.len() as u32, data };
        let inp = |length| Transfer { id: 0, endpoint: 1, dir_in: true, setup: [0; 8], data: Vec::new(), length };
        let (o, _) = self.transfer(out(cbw))?;
        if o != Outcome::Ok {
            println!("  CBW: {o:?}");
            return None;
        }
        let mut data = Vec::new();
        if data_in > 0 {
            let (o, d) = self.transfer(inp(data_in))?;
            if o != Outcome::Ok {
                println!("  data in: {o:?}");
            }
            data = d;
        } else if !data_out.is_empty() {
            let (o, _) = self.transfer(out(data_out))?;
            if o != Outcome::Ok {
                println!("  data out: {o:?}");
            }
        }
        let (o, csw) = self.transfer(inp(13))?;
        if o != Outcome::Ok {
            println!("  CSW: {o:?}");
        }
        Some((data, csw))
    }
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join(" ")
}

fn text(b: &[u8]) -> String {
    b.iter().map(|&c| if (0x20..0x7f).contains(&c) { c as char } else { '.' }).collect()
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let bios = args.first().expect("usage: l2usb <bios> [cart|none] [state] [ms-before-plug]");
    let fw = ribbit_memory::Firmware::from_bytes_any_model(ribbit_core::read_image(bios).unwrap()).unwrap();
    let cart = args
        .get(1)
        .filter(|c| *c != "none")
        .map(|c| ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(c).unwrap()).unwrap());
    let mut m = Machine::with_straps(fw, cart, 0x63FF_FFFF);
    if let Ok(p) = std::env::var("RIBBIT_L2_FLASH") {
        // The internal flash (the frontend's flash-<bios crc>.bin), read only.
        m.io.spi_flash.as_mut().expect("a Leapster 2").load(&std::fs::read(p).unwrap());
    }
    if let Ok(p) = std::env::var("RIBBIT_L2_SD") {
        // The SD card (the frontend's sdcard-<bios crc>.img), read only.
        let card = m.io.sd.as_mut().and_then(|s| s.card.as_mut()).expect("an SD slot with a card");
        assert!(card.load_image(&std::fs::read(p).unwrap()), "not an SD card image");
    }
    m.boot_leapster2_system();
    m.ensure_touch_calibration();
    if let Some(p) = args.get(2).filter(|p| *p != "none") {
        m.load_state(&std::fs::read(p).unwrap()).unwrap();
    }
    if let Some(u) = m.io.usb.as_mut() {
        u.log = std::env::var_os("RIBBIT_USB_LOG").is_some();
    }
    let before: u64 = args.get(3).map_or(4000, |s| s.parse().unwrap());
    let mut h = Host { m, id: 100, tag: 0, restarts: 0, uart_seen: 0, watched: (0, 0, 0) };
    h.run_ms(before);
    println!("plugging in at {:.2} s", h.m.cpu.cycles as f64 / (1000 * MS) as f64);
    h.m.usb_plug(true);
    // Enumeration (the host's own requests) within 3 s.
    for _ in 0..150 {
        h.run_ms(100);
        if h.m.io.usb.as_ref().unwrap().host.descriptors.len() >= 2 {
            break;
        }
    }
    let u = h.m.io.usb.as_ref().unwrap();
    for d in &u.host.descriptors {
        println!("descriptor: {}", hex(d));
    }
    if u.host.descriptors.len() < 2 {
        println!("enumeration did not finish; registers: {:02x?}", &u.regs[..0x40]);
        h.dump();
        h.records("stuck");
        return;
    }
    h.records("after enumeration");
    println!("SET_CONFIGURATION 1: {:?}", h.control([0x00, 9, 1, 0, 0, 0, 0, 0], Vec::new()).map(|r| r.0));
    let max_lun = h.control([0xA1, 0xFE, 0, 0, 0, 0, 1, 0], Vec::new());
    println!("GET MAX LUN: {max_lun:?}");
    let luns = (max_lun.and_then(|(_, d)| d.first().copied()).unwrap_or(0) + 1).min(2);
    for lun in 0..luns {
        println!("LUN {lun}:");
        if let Some((d, csw)) = h.scsi(lun, &[0x12, 0, 0, 0, 36, 0], 36, Vec::new()) {
            println!("  INQUIRY: {} \"{}\"  csw {}", hex(&d[..d.len().min(8)]), text(&d[8.min(d.len())..]), hex(&csw));
        }
        if let Some((_, csw)) = h.scsi(lun, &[0xC2, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0, Vec::new()) {
            println!("  C2 (mount): csw {}", hex(&csw));
        }
        h.run_ms(500);
        if let Some((_, csw)) = h.scsi(lun, &[0x00, 0, 0, 0, 0, 0], 0, Vec::new()) {
            println!("  TEST UNIT READY: csw {}", hex(&csw));
        }
        if let Some((d, csw)) = h.scsi(lun, &[0x25, 0, 0, 0, 0, 0, 0, 0, 0, 0], 8, Vec::new()) {
            println!("  READ CAPACITY: {}  csw {}", hex(&d), hex(&csw));
        }
        if let Some((d, csw)) = h.scsi(lun, &[0x28, 0, 0, 0, 0, 0, 0, 0, 1, 0], 512, Vec::new()) {
            println!("  READ(10) 0: {} bytes, {} .. {}  csw {}", d.len(), hex(&d[..d.len().min(16)]), hex(&d[d.len().saturating_sub(2)..]), hex(&csw));
        }
    }
}
