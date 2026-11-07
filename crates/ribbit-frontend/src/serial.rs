//! The Didj's serial console (UART0, `console=ttyS0,115200`: Linux's log
//! and a root shell): kept for the app's window and offered to terminals on
//! this PC over TCP (telnet, PuTTY), on 127.0.0.1 only.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The TCP port terminals connect to.
pub const PORT: u16 = 2323;
/// The window keeps this much of the console's text.
const KEEP: usize = 256 * 1024;

#[derive(Default)]
pub struct Serial {
    inner: Mutex<Inner>,
    /// The listener's thread stops when this is set (`stop_tcp`).
    stop: Mutex<Option<Arc<AtomicBool>>>,
}

#[derive(Default)]
struct Inner {
    /// The text shown in the window: escape sequences taken out,
    /// backspaces applied.
    text: String,
    /// In an escape sequence (ESC, then ESC [ ...).
    escape: Escape,
    /// Bytes for the Didj, oldest first.
    input: VecDeque<u8>,
    /// The connected terminal (one at a time).
    client: Option<TcpStream>,
}

#[derive(Default, PartialEq)]
enum Escape {
    #[default]
    None,
    Esc,
    Csi,
}

impl Serial {
    /// What the Didj sent: to the window and the terminal.
    pub fn output(&self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let mut i = self.inner.lock().unwrap();
        if let Some(c) = &mut i.client {
            if c.write_all(bytes).is_err() {
                i.client = None;
            }
        }
        i.show(bytes);
    }

    /// A note from Ribbit in the window only (a power-on).
    pub fn note(&self, text: &str) {
        let mut i = self.inner.lock().unwrap();
        if !i.text.is_empty() && !i.text.ends_with('\n') {
            i.text.push('\n');
        }
        i.text.push_str(text);
    }

    /// Typed for the Didj (by the window or the terminal).
    pub fn type_bytes(&self, bytes: &[u8]) {
        self.inner.lock().unwrap().input.extend(bytes);
    }

    /// What was typed since the last call, for UART0's receiver.
    pub fn take_input(&self) -> Vec<u8> {
        self.inner.lock().unwrap().input.drain(..).collect()
    }

    pub fn text(&self) -> String {
        self.inner.lock().unwrap().text.clone()
    }

    pub fn clear(&self) {
        self.inner.lock().unwrap().text.clear();
    }

    pub fn listening(&self) -> bool {
        self.stop.lock().unwrap().is_some()
    }

    pub fn connected(&self) -> bool {
        self.inner.lock().unwrap().client.is_some()
    }

    /// Listens on 127.0.0.1:`PORT`. A new terminal replaces the old one.
    pub fn start_tcp(self: &Arc<Self>) -> std::io::Result<()> {
        if self.listening() {
            return Ok(());
        }
        let listener = TcpListener::bind(("127.0.0.1", PORT))?;
        listener.set_nonblocking(true)?;
        let stop = Arc::new(AtomicBool::new(false));
        *self.stop.lock().unwrap() = Some(stop.clone());
        let me = self.clone();
        std::thread::Builder::new().name("ribbit-serial".into()).spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => me.connect(stream, &stop),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
                    Err(_) => std::thread::sleep(Duration::from_millis(50)),
                }
            }
            me.inner.lock().unwrap().client = None;
        })?;
        Ok(())
    }

    pub fn stop_tcp(&self) {
        if let Some(stop) = self.stop.lock().unwrap().take() {
            stop.store(true, Ordering::Relaxed);
        }
        if let Some(c) = self.inner.lock().unwrap().client.take() {
            let _ = c.shutdown(std::net::Shutdown::Both);
        }
    }

    /// A terminal connected: a thread passes what it types to the Didj.
    fn connect(self: &Arc<Self>, stream: TcpStream, stop: &Arc<AtomicBool>) {
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_nodelay(true);
        // A terminal that stops reading must not hold up the Didj.
        let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));
        let Ok(mut reader) = stream.try_clone() else { return };
        let mut writer = stream;
        // Telnet: the Didj echoes (its shell does) and sends characters as
        // typed, not lines (IAC WILL ECHO, IAC WILL SUPPRESS-GO-AHEAD).
        let _ = writer.write_all(&[255, 251, 1, 255, 251, 3]);
        let _ = writer.write_all(b"\r\nRibbit: the Didj's serial console (ttyS0). Press Enter for a prompt.\r\n");
        if let Some(old) = self.inner.lock().unwrap().client.replace(writer) {
            let _ = old.shutdown(std::net::Shutdown::Both);
        }
        let (me, stop) = (self.clone(), stop.clone());
        let _ = std::thread::Builder::new().name("ribbit-serial-client".into()).spawn(move || {
            let mut telnet = Telnet::default();
            let mut buf = [0u8; 512];
            while !stop.load(Ordering::Relaxed) {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let typed = telnet.filter(&buf[..n]);
                        me.type_bytes(&typed);
                    }
                }
            }
            // This terminal left (unless a newer one took its place).
            let mut i = me.inner.lock().unwrap();
            let same = i.client.as_ref().zip(reader.peer_addr().ok()).is_some_and(|(c, a)| c.peer_addr().ok() == Some(a));
            if same {
                i.client = None;
            }
        });
    }
}

impl Inner {
    fn show(&mut self, bytes: &[u8]) {
        for &b in bytes {
            match self.escape {
                Escape::Esc => self.escape = if b == b'[' { Escape::Csi } else { Escape::None },
                // Ends at its final byte (0x40-0x7E).
                Escape::Csi => {
                    if (0x40..=0x7E).contains(&b) {
                        self.escape = Escape::None;
                    }
                }
                Escape::None => match b {
                    0x1B => self.escape = Escape::Esc,
                    b'\r' | 0x07 | 0 => {}
                    0x08 | 0x7F => {
                        if !self.text.ends_with('\n') {
                            self.text.pop();
                        }
                    }
                    // Latin-1, as the Didj's tty has no other.
                    _ => self.text.push(b as char),
                },
            }
        }
        if self.text.len() > KEEP {
            let mut cut = self.text.len() - KEEP * 3 / 4;
            while !self.text.is_char_boundary(cut) {
                cut += 1;
            }
            self.text.drain(..cut);
        }
    }
}

/// Takes telnet's commands out of what a terminal sends, and its Enter
/// (CR LF or CR NUL) down to the CR a serial terminal sends.
#[derive(Default)]
struct Telnet {
    state: u8,
    cr: bool,
}

impl Telnet {
    fn filter(&mut self, bytes: &[u8]) -> Vec<u8> {
        const IAC: u8 = 255;
        let mut out = Vec::with_capacity(bytes.len());
        for &b in bytes {
            match self.state {
                // After IAC: a command; WILL/WONT/DO/DONT take an option,
                // SB a subnegotiation up to IAC SE; IAC IAC is 255.
                1 => {
                    self.state = match b {
                        IAC => {
                            out.push(IAC);
                            0
                        }
                        251..=254 => 2,
                        250 => 3,
                        _ => 0,
                    }
                }
                2 => self.state = 0,
                3 => self.state = if b == IAC { 4 } else { 3 },
                4 => self.state = if b == 240 { 0 } else { 3 },
                _ if b == IAC => self.state = 1,
                _ => {
                    let after_cr = std::mem::replace(&mut self.cr, b == b'\r');
                    if after_cr && (b == b'\n' || b == 0) {
                        continue;
                    }
                    out.push(b);
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telnet_commands_and_enter() {
        let mut t = Telnet::default();
        assert_eq!(t.filter(&[255, 253, 1, b'l', b's', b'\r', b'\n']), b"ls\r");
        assert_eq!(t.filter(&[255, 250, 24, 1, 255, 240, b'a', b'\r', 0, 255, 255]), [b'a', b'\r', 255]);
    }

    #[test]
    fn window_text() {
        let s = Serial::default();
        s.output(b"# lx\x08s\r\n\x1b[1;34mbin\x1b[0m\r\n");
        assert_eq!(s.text(), "# ls\nbin\n");
    }

    /// A Didj from RIBBIT_DIDJ_DIR (its firmware packages) boots, and a
    /// terminal on TCP gets the shell's answer to `uname -r`.
    #[test]
    #[ignore]
    fn didj_shell_over_tcp() {
        let dir = std::path::PathBuf::from(std::env::var("RIBBIT_DIDJ_DIR").expect("RIBBIT_DIDJ_DIR"));
        let serial = Arc::new(Serial::default());
        serial.start_tcp().unwrap();
        let saves = std::env::temp_dir().join("ribbit-serial-test");
        let mode = crate::didj::Mode { enhanced: true, cpu_scale: 1, frame_skip: false, hd: 1, hle: false, smooth: false };
        let didj = crate::didj::DidjSession::start(&dir, &saves, None, None, None, mode, serial.clone()).unwrap();
        let mut term = TcpStream::connect(("127.0.0.1", PORT)).unwrap();
        term.set_read_timeout(Some(Duration::from_millis(500))).unwrap();
        let (mut got, mut buf, mut typed) = (Vec::new(), [0u8; 4096], false);
        let start = std::time::Instant::now();
        while start.elapsed() < Duration::from_secs(180) {
            if let Ok(n) = term.read(&mut buf) {
                got.extend_from_slice(&buf[..n]);
            }
            let text = String::from_utf8_lossy(&got).into_owned();
            if !typed && text.contains("# ") {
                term.write_all(b"uname -r\r\n").unwrap();
                typed = true;
            }
            if typed && text.contains("2.6.20.1-leapfrog\r\n") {
                break;
            }
        }
        didj.stop();
        let text = String::from_utf8_lossy(&got);
        println!("{}", &text[text.len().saturating_sub(600)..]);
        assert!(text.contains("2.6.20.1-leapfrog\r\n"), "no answer after {:?}", start.elapsed());
        assert!(serial.text().contains("uname -r"));
    }
}
