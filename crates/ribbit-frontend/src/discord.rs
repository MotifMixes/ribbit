//! Discord Rich Presence: "Playing Ribbit", the game's name and how long it
//! has been played, like Dolphin's. Talks to the local Discord app over its
//! IPC pipe (frames of opcode, length, JSON: a named pipe on Windows, a Unix
//! socket on macOS and Linux); does nothing when Discord is
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

/// The connection to Discord.
trait Pipe: Read + Write + Send {}
impl<T: Read + Write + Send> Pipe for T {}
type Conn = Box<dyn Pipe>;

fn worker(rx: &Receiver<Presence>) {
    let mut want = Presence::Off;
    let mut pipe: Option<Conn> = None;
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
fn open_pipe(n: u32) -> Option<Conn> {
    let f = std::fs::OpenOptions::new().read(true).write(true).open(format!(r"\\.\pipe\discord-ipc-{n}")).ok()?;
    Some(Box::new(f))
}

#[cfg(unix)]
fn open_pipe(n: u32) -> Option<Conn> {
    let s = unix_socket_paths(n).into_iter().find_map(|p| std::os::unix::net::UnixStream::connect(p).ok())?;
    Some(Box::new(s))
}

#[cfg(not(any(windows, unix)))]
fn open_pipe(_n: u32) -> Option<Conn> {
    None
}

/// Where Discord puts its socket `discord-ipc-<n>`: the first of
/// XDG_RUNTIME_DIR, TMPDIR (macOS: the per-user temporary folder), TMP and
/// TEMP that is set, else /tmp; on Linux also inside the Flatpak and Snap
/// packages' folders there.
#[cfg(unix)]
fn unix_socket_paths(n: u32) -> Vec<std::path::PathBuf> {
    let base = ["XDG_RUNTIME_DIR", "TMPDIR", "TMP", "TEMP"]
        .iter()
        .find_map(|v| std::env::var_os(v).filter(|d| !d.is_empty()))
        .map_or_else(|| std::path::PathBuf::from("/tmp"), std::path::PathBuf::from);
    let name = format!("discord-ipc-{n}");
    ["", "app/com.discordapp.Discord", "snap.discord", "app/com.discordapp.DiscordCanary"]
        .iter()
        .map(|sub| base.join(sub).join(&name))
        .collect()
}

/// Connects and says hello; None when Discord is not running.
fn connect() -> Option<Conn> {
    hello((0..10).find_map(open_pipe)?)
}

/// The handshake on a fresh connection.
fn hello(mut pipe: Conn) -> Option<Conn> {
    let hello = format!(r#"{{"v":1,"client_id":"{CLIENT_ID}"}}"#);
    write_frame(&mut pipe, 0, &hello).ok()?;
    read_frame(&mut pipe).ok()?; // READY (or an error, which the next write reports)
    tracing::info!(target: "ribbit::discord", "connected to Discord");
    Some(pipe)
}

fn write_frame(pipe: &mut dyn Pipe, op: u32, json: &str) -> std::io::Result<()> {
    let mut frame = Vec::with_capacity(8 + json.len());
    frame.extend_from_slice(&op.to_le_bytes());
    frame.extend_from_slice(&(json.len() as u32).to_le_bytes());
    frame.extend_from_slice(json.as_bytes());
    pipe.write_all(&frame)
}

fn read_frame(pipe: &mut dyn Pipe) -> std::io::Result<(u32, String)> {
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

fn send_activity(pipe: &mut dyn Pipe, p: &Presence) -> std::io::Result<()> {
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

    /// macOS and Linux: Discord's side of a Unix socket answers the
    /// handshake and receives the status.
    #[cfg(unix)]
    #[test]
    fn talks_to_discord_over_a_unix_socket() {
        let (ours, mut discord) = std::os::unix::net::UnixStream::pair().unwrap();
        let server = std::thread::spawn(move || {
            let mut d: Conn = Box::new(discord.try_clone().unwrap());
            let (op, hello) = read_frame(&mut *d).unwrap();
            assert_eq!(op, 0);
            assert!(hello.contains(CLIENT_ID));
            write_frame(&mut *d, 1, r#"{"cmd":"DISPATCH","evt":"READY"}"#).unwrap();
            let (op, msg) = read_frame(&mut *d).unwrap();
            write_frame(&mut *d, 1, r#"{"cmd":"SET_ACTIVITY"}"#).unwrap();
            let _ = discord.flush();
            (op, msg)
        });
        let mut conn = hello(Box::new(ours)).expect("handshake");
        send_activity(&mut *conn, &Presence::Browsing).unwrap();
        let (op, msg) = server.join().unwrap();
        assert_eq!(op, 1);
        assert!(msg.contains(r#""cmd":"SET_ACTIVITY""#) && msg.contains("Browsing games"));
    }

    #[cfg(unix)]
    #[test]
    fn unix_socket_paths_include_the_plain_and_packaged_discords() {
        let p = unix_socket_paths(0);
        assert!(p[0].ends_with("discord-ipc-0"));
        assert!(p.iter().any(|p| p.to_string_lossy().contains("app/com.discordapp.Discord/discord-ipc-0")));
        assert!(p.iter().any(|p| p.to_string_lossy().contains("snap.discord/discord-ipc-0")));
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
