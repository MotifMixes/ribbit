//! Discord Rich Presence: "Playing Ribbit", the game's name and how long it
//! has been played, like Dolphin's. Talks to the local Discord app over its
//! IPC pipe (frames of opcode, length, JSON); does nothing when Discord is
//! not running, and retries now and then.

use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Ribbit's application on the Discord developer portal.
const CLIENT_ID: &str = "1560155716667314216";
/// The art asset uploaded to that application.
const LOGO: &str = "ribbit";
const RETRY: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq)]
pub enum Presence {
    /// Discord shows nothing for Ribbit.
    Off,
    /// In the game library.
    Browsing,
    /// Playing a game on `system` since `since` (seconds since the Unix
    /// epoch); `series`: a LeapPad book's series, shown on its own line.
    Playing { title: String, system: String, series: Option<String>, since: u64 },
}

impl Presence {
    /// Playing `title` ("Name (Region)") on `system` since `since` (Unix seconds).
    pub fn playing(title: &str, system: &str, since: u64) -> Self {
        // Just the name: "Game (USA, alt)" -> "Game".
        let name = title.strip_suffix(')').and_then(|t| t.rsplit_once(" (")).map_or(title, |(n, _)| n);
        Presence::Playing { title: name.to_string(), system: system.to_string(), series: None, since }
    }

    /// The same, with the book's series on the second line ("Leap Start Pre
    /// Reading"), the system then going to the logo's hover text.
    pub fn with_series(self, series: Option<&str>) -> Self {
        match self {
            Presence::Playing { title, system, since, .. } => Presence::Playing { title, system, series: series.map(str::to_string), since },
            p => p,
        }
    }
}

/// The presence thread; dropping it clears the status.
pub struct Discord {
    tx: Sender<Presence>,
    last: Presence,
}

impl Discord {
    pub fn start() -> Self {
        let (tx, rx) = channel();
        let _ = std::thread::Builder::new().name("ribbit-discord".into()).spawn(move || worker(&rx));
        Self { tx, last: Presence::Off }
    }

    /// Sets what Discord shows (only sent when it changes).
    pub fn set(&mut self, p: Presence) {
        if p != self.last {
            self.last = p.clone();
            let _ = self.tx.send(p);
        }
    }
}

fn worker(rx: &Receiver<Presence>) {
    let mut want = Presence::Off;
    let mut pipe: Option<std::fs::File> = None;
    let mut sent: Option<Presence> = None;
    loop {
        if pipe.is_none() && want != Presence::Off {
            pipe = connect();
            sent = None;
        }
        if let Some(p) = &mut pipe
            && sent.as_ref() != Some(&want)
        {
            if send_activity(p, &want).is_ok() {
                sent = Some(want.clone());
            } else {
                pipe = None;
            }
        }
        match rx.recv_timeout(RETRY) {
            Ok(p) => want = p,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(p) = &mut pipe {
                    let _ = send_activity(p, &Presence::Off);
                }
                return;
            }
        }
    }
}

#[cfg(windows)]
fn open_pipe(n: u32) -> Option<std::fs::File> {
    std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{n}")).ok()
}

#[cfg(not(windows))]
fn open_pipe(_n: u32) -> Option<std::fs::File> {
    // Unix sockets need a different transport; not supported yet.
    None
}

/// Connects and says hello; None when Discord is not running.
fn connect() -> Option<std::fs::File> {
    let mut pipe = (0..10).find_map(open_pipe)?;
    let hello = format!(r#"{{"v":1,"client_id":"{CLIENT_ID}"}}"#);
    write_frame(&mut pipe, 0, &hello).ok()?;
    read_frame(&mut pipe).ok()?; // READY (or an error, which the next write reports)
    tracing::info!(target: "ribbit::discord", "connected to Discord");
    Some(pipe)
}

fn write_frame(pipe: &mut std::fs::File, op: u32, json: &str) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(8 + json.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
    frame.extend_from_slice(json.as_bytes());
    pipe.write_all(&frame)
}

fn read_frame(pipe: &mut std::fs::File) -> std::io::Result<(u32, String)> {
    let mut head = [0u8; 8];
    pipe.read_exact(&mut head)?;
    let op = u32::from_le_bytes(head[..4].try_into().unwrap());
    let len = u32::from_le_bytes(head[4..].try_into().unwrap()) as usize;
    if len > 1 << 20 {
        return Err(std::io::Error::other("oversized Discord frame"));
    }
    let mut body = vec![0; len];
    pipe.read_exact(&mut body)?;
    Ok((op, String::from_utf8_lossy(&body).into_owned()))
}

fn send_activity(pipe: &mut std::fs::File, p: &Presence) -> std::io::Result<()> {
    let msg = format!(
        r#"{{"cmd":"SET_ACTIVITY","args":{{"pid":{},"activity":{}}},"nonce":"{}"}}"#,
        std::process::id(),
        activity_json(p),
        SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
    );
    write_frame(pipe, 1, &msg)?;
    let (op, reply) = read_frame(pipe)?;
    if op == 2 || reply.contains(r#""evt":"ERROR""#) {
        tracing::warn!(target: "ribbit::discord", reply, "Discord refused the status");
    }
    Ok(())
}

fn activity_json(p: &Presence) -> String {
    let version = concat!("Ribbit v", env!("CARGO_PKG_VERSION"));
    match p {
        Presence::Off => "null".into(),
        Presence::Browsing => format!(
            r#"{{"details":"Browsing games","assets":{{"large_image":"{LOGO}","large_text":{}}}}}"#,
            json_str(version)
        ),
        Presence::Playing { title, system, series, since } => {
            let (state, hover) = match series {
                Some(s) => (s.clone(), format!("{system} · {version}")),
                None => (system.clone(), version.to_string()),
            };
            format!(
                r#"{{"details":{},"state":{},"timestamps":{{"start":{since}}},"assets":{{"large_image":"{LOGO}","large_text":{}}}}}"#,
                json_str(&clip(title, 128)),
                json_str(&clip(&state, 128)),
                json_str(&clip(&hover, 128))
            )
        }
    }
}

/// Discord limits these fields to 128 characters.
fn clip(s: &str, max: usize) -> String {
    if s.chars().count() <= max { s.to_string() } else { s.chars().take(max - 1).chain(['…']).collect() }
}

fn json_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playing_shows_the_name_and_system_and_escapes() {
        let p = Presence::playing("SpongeBob SquarePants - Saves the Day (USA, alt)", "Leapster", 1_700_000_000);
        let Presence::Playing { title, .. } = &p else { panic!() };
        assert_eq!(title, "SpongeBob SquarePants - Saves the Day");
        let json = activity_json(&p);
        assert!(json.contains(r#""details":"SpongeBob SquarePants - Saves the Day""#));
        assert!(!json.contains("USA") && json.contains(r#""state":"Leapster""#));
        assert!(json.contains(r#""large_image":"ribbit""#));
        assert_eq!(json_str("a\"b\\c\n"), r#""a\"b\\c\u000a""#);
        assert_eq!(activity_json(&Presence::Off), "null");
    }

    #[test]
    fn same_game_is_not_resent() {
        let (tx, rx) = channel();
        let mut d = Discord { tx, last: Presence::Off };
        d.set(Presence::playing("Game (USA)", "Leapster", 5));
        d.set(Presence::playing("Game (USA)", "Leapster", 5));
        d.set(Presence::Browsing);
        assert_eq!(rx.try_iter().count(), 2);
    }
}
