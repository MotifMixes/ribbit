//! AF_UNIX sockets for the Explorer's programs (AppManager, VideoDaemon,
//! CartManager and lfd talk over sockets in /tmp; syslog uses /dev/log).
//! Only the Explorer's programs use them, so only its HLE answers these
//! calls (the Didj's keep failing, as before).
//!
//! A socket's file descriptor is a device node of Ribbit's own
//! (`Device::Other(SOCK | id)`), so reads, writes and poll come through
//! the Explorer's device hooks. File descriptors are copied on fork and
//! dropped at exit without a count, so a socket is closed when no
//! process's descriptor names it any more: a reader then gets end of file
//! (lfd serves one lfd-ctrl connection until it ends).

use super::dev::Device;
use super::task::Wait;
use super::{EAGAIN, EFAULT, EINVAL, Process};
use std::collections::{HashMap, VecDeque};

/// The device numbers Ribbit gives sockets (no real device has them).
pub(super) const SOCK: u32 = 0xFFFF_0000;
const AF_UNIX: u16 = 1;
const SOCK_STREAM: u32 = 1;
const SOCK_DGRAM: u32 = 2;
const SOCK_SEQPACKET: u32 = 5;
const SOCK_NONBLOCK: u32 = 0o4000;
const EAFNOSUPPORT: i32 = 97;
const ECONNREFUSED: i32 = 111;
const ENOTCONN: i32 = 107;
const EADDRINUSE: i32 = 98;
const EISCONN: i32 = 106;
const ENOENT: i32 = 2;

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Socket {
    kind: u32,
    /// The name it is bound to.
    name: Option<String>,
    listening: bool,
    /// Connections waiting for accept (the server side's sockets).
    backlog: VecDeque<u32>,
    /// The other end of a connection (or a datagram socket's default
    /// destination).
    peer: Option<u32>,
    /// What came in: packets (stream sockets read across them), with the
    /// sender's name for datagrams.
    rx: VecDeque<(Vec<u8>, Option<String>)>,
}

#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Sockets {
    all: HashMap<u32, Socket>,
    names: HashMap<String, u32>,
    next: u32,
}

impl Process {
    /// The socket an fd is, if it is one.
    fn sock_of(&self, fd: u32) -> Result<u32, i32> {
        match self.g.fds.get(fd as usize).and_then(|f| f.as_ref()) {
            Some(super::Fd::Dev { dev: Device::Other(r), .. }) if r & 0xFFFF_0000 == SOCK => Ok(r & 0xFFFF),
            Some(_) => Err(88), // ENOTSOCK
            None => Err(super::EBADF),
        }
    }

    fn sock_fd(&mut self, id: u32, nonblock: bool) -> u32 {
        self.new_fd(super::Fd::Dev { dev: Device::Other(SOCK | id), path: format!("socket:[{id}]"), nonblock }, 0)
    }

    fn new_sock(&mut self, kind: u32) -> u32 {
        let s = &mut self.x.sockets;
        let id = s.next;
        s.next += 1;
        s.all.insert(id, Socket { kind, ..Default::default() });
        id
    }

    /// A sockaddr_un's name ("@..." for the abstract namespace).
    fn sock_name(&mut self, addr: u32, len: u32) -> Result<String, i32> {
        let bytes = self.mem().read_bytes(addr, (len as usize).min(110)).ok_or(EFAULT)?;
        if bytes.len() < 2 || u16::from_le_bytes([bytes[0], bytes[1]]) != AF_UNIX {
            return Err(EAFNOSUPPORT);
        }
        let path = &bytes[2..];
        if path.first() == Some(&0) {
            return Ok(format!("@{}", String::from_utf8_lossy(&path[1..])));
        }
        let end = path.iter().position(|&b| b == 0).unwrap_or(path.len());
        let p = String::from_utf8_lossy(&path[..end]).into_owned();
        Ok(self.vfs.resolve(&self.g.cwd, &p, false).unwrap_or(p))
    }

    fn put_sock_name(&mut self, name: Option<&str>, addr: u32, len_at: u32) {
        if addr == 0 || len_at == 0 {
            return;
        }
        let mut b = AF_UNIX.to_le_bytes().to_vec();
        match name {
            Some(n) if n.starts_with('@') => {
                b.push(0);
                b.extend(n[1..].bytes());
            }
            Some(n) => {
                b.extend(n.bytes());
                b.push(0);
            }
            None => {}
        }
        let room = self.mem().get32(len_at).unwrap_or(0) as usize;
        let n = b.len().min(room);
        self.mem().write_bytes(addr, &b[..n]);
        self.mem().put32(len_at, b.len() as u32);
    }

    /// The socket system calls (2.6.31 EABI numbers 281-297).
    pub(super) fn x_socket_call(&mut self, n: u32, a: [u32; 6]) -> Result<u32, i32> {
        match n {
            281 => {
                if a[0] != AF_UNIX as u32 {
                    return Err(EAFNOSUPPORT);
                }
                let kind = a[1] & 0xF;
                if !matches!(kind, SOCK_STREAM | SOCK_DGRAM | SOCK_SEQPACKET) {
                    return Err(EINVAL);
                }
                let id = self.new_sock(kind);
                Ok(self.sock_fd(id, a[1] & SOCK_NONBLOCK != 0))
            }
            282 => {
                let id = self.sock_of(a[0])?;
                let name = self.sock_name(a[1], a[2])?;
                if self.x.sockets.names.contains_key(&name) || !name.starts_with('@') && self.vfs.get(&name).is_some() {
                    return Err(EADDRINUSE);
                }
                if !name.starts_with('@') {
                    let (dir, _) = super::vfs::split(&name);
                    if !self.vfs.get(dir).is_some_and(|n| n.is_dir()) {
                        return Err(ENOENT);
                    }
                    // A socket node, as `ls -l /tmp` shows on Original.
                    self.vfs.insert(&name, 0o140000 | 0o755, super::vfs::Data::File(Vec::new()));
                }
                self.x.sockets.names.insert(name.clone(), id);
                self.x.sockets.all.get_mut(&id).unwrap().name = Some(name);
                Ok(0)
            }
            283 => {
                let id = self.sock_of(a[0])?;
                let name = self.sock_name(a[1], a[2])?;
                let target = *self.x.sockets.names.get(&name).ok_or(if self.vfs.get(&name).is_some() { ECONNREFUSED } else { ENOENT })?;
                let s = &mut self.x.sockets;
                let kind = s.all[&id].kind;
                if kind == SOCK_DGRAM {
                    s.all.get_mut(&id).unwrap().peer = Some(target);
                    return Ok(0);
                }
                if s.all[&id].peer.is_some() {
                    return Err(EISCONN);
                }
                if !s.all.get(&target).is_some_and(|t| t.listening) {
                    return Err(ECONNREFUSED);
                }
                let server = self.new_sock(kind);
                let s = &mut self.x.sockets;
                s.all.get_mut(&server).unwrap().peer = Some(id);
                s.all.get_mut(&server).unwrap().name = s.all[&target].name.clone();
                s.all.get_mut(&id).unwrap().peer = Some(server);
                s.all.get_mut(&target).unwrap().backlog.push_back(server);
                Ok(0)
            }
            284 => {
                let id = self.sock_of(a[0])?;
                self.x.sockets.all.get_mut(&id).unwrap().listening = true;
                Ok(0)
            }
            285 | 366 => {
                let id = self.sock_of(a[0])?;
                let nonblock = matches!(self.fd(a[0]), Ok(super::Fd::Dev { nonblock: true, .. }));
                let Some(conn) = self.x.sockets.all.get_mut(&id).unwrap().backlog.pop_front() else {
                    if nonblock {
                        return Err(EAGAIN);
                    }
                    self.block = Some(Wait::Retry(self.clock() + std::time::Duration::from_millis(2)));
                    return Ok(0);
                };
                self.put_sock_name(None, a[1], a[2]);
                Ok(self.sock_fd(conn, n == 366 && a[3] & SOCK_NONBLOCK != 0))
            }
            286 | 287 => {
                let id = self.sock_of(a[0])?;
                let s = &self.x.sockets;
                let name = if n == 286 { s.all[&id].name.clone() } else { s.all[&id].peer.and_then(|p| s.all.get(&p)).and_then(|p| p.name.clone()) };
                self.put_sock_name(name.as_deref(), a[1], a[2]);
                Ok(0)
            }
            288 => {
                let kind = a[1] & 0xF;
                let (x, y) = (self.new_sock(kind), self.new_sock(kind));
                self.x.sockets.all.get_mut(&x).unwrap().peer = Some(y);
                self.x.sockets.all.get_mut(&y).unwrap().peer = Some(x);
                let nb = a[1] & SOCK_NONBLOCK != 0;
                let (fx, fy) = (self.sock_fd(x, nb), self.sock_fd(y, nb));
                if self.mem().put32(a[3], fx) && self.mem().put32(a[3] + 4, fy) { Ok(0) } else { Err(EFAULT) }
            }
            // send, sendto
            289 | 290 => {
                let id = self.sock_of(a[0])?;
                let data = self.mem().read_bytes(a[1], a[2] as usize).ok_or(EFAULT)?;
                let to = if n == 290 && a[4] != 0 { Some(self.sock_name(a[4], a[5])?) } else { None };
                self.sock_send(id, data, to).map(|n| n as u32)
            }
            // recv, recvfrom
            291 | 292 => {
                let id = self.sock_of(a[0])?;
                let nonblock = a[3] & 0x40 != 0 || matches!(self.fd(a[0]), Ok(super::Fd::Dev { nonblock: true, .. }));
                let peek = a[3] & 2 != 0;
                let Some((data, from)) = self.sock_recv(id, a[2] as usize, nonblock, peek)? else { return Ok(0) };
                if !self.mem().write_bytes(a[1], &data) {
                    return Err(EFAULT);
                }
                if n == 292 {
                    self.put_sock_name(from.as_deref(), a[4], a[5]);
                }
                Ok(data.len() as u32)
            }
            293 => Ok(0), // shutdown
            294 => Ok(0), // setsockopt
            295 => {
                // getsockopt: SO_ERROR 0, SO_TYPE, buffer sizes; 4-byte values.
                let id = self.sock_of(a[0])?;
                let v = match a[2] {
                    3 => self.x.sockets.all[&id].kind,
                    7 | 8 => 112 << 10,
                    _ => 0,
                };
                if self.mem().put32(a[3], v) && self.mem().put32(a[4], 4) { Ok(0) } else { Err(EFAULT) }
            }
            // sendmsg, recvmsg: the iovecs (no passed descriptors).
            296 | 297 => {
                let id = self.sock_of(a[0])?;
                let m = a[1];
                let get = |p: &mut Self, o: u32| p.mem().get32(m + o).ok_or(EFAULT);
                let (name, name_len, iov, iovlen) = (get(self, 0)?, get(self, 4)?, get(self, 8)?, get(self, 12)?);
                let mut parts = Vec::new();
                for i in 0..iovlen {
                    let at = self.mem().get32(iov + 8 * i).ok_or(EFAULT)?;
                    let len = self.mem().get32(iov + 8 * i + 4).ok_or(EFAULT)?;
                    parts.push((at, len));
                }
                if n == 296 {
                    if get(self, 20)? != 0 {
                        self.log_once("sendmsg with control data: dropped".into());
                    }
                    let mut data = Vec::new();
                    for (at, len) in parts {
                        data.extend(self.mem().read_bytes(at, len as usize).ok_or(EFAULT)?);
                    }
                    let to = if name != 0 { Some(self.sock_name(name, name_len)?) } else { None };
                    self.sock_send(id, data, to).map(|n| n as u32)
                } else {
                    let total: u32 = parts.iter().map(|p| p.1).sum();
                    let nonblock = a[2] & 0x40 != 0 || matches!(self.fd(a[0]), Ok(super::Fd::Dev { nonblock: true, .. }));
                    let Some((data, from)) = self.sock_recv(id, total as usize, nonblock, false)? else { return Ok(0) };
                    let mut at = 0;
                    for (p, len) in parts {
                        let n = (len as usize).min(data.len() - at);
                        self.mem().write_bytes(p, &data[at..at + n]);
                        at += n;
                    }
                    if name != 0 {
                        self.put_sock_name(from.as_deref(), name, m + 4);
                    }
                    // No control data, no flags.
                    self.mem().put32(m + 20, 0);
                    self.mem().put32(m + 24, 0);
                    Ok(data.len() as u32)
                }
            }
            _ => Err(super::ENOSYS),
        }
    }

    fn sock_send(&mut self, id: u32, data: Vec<u8>, to: Option<String>) -> Result<usize, i32> {
        let s = &mut self.x.sockets;
        let me = s.all.get(&id).ok_or(EINVAL)?;
        let from = me.name.clone();
        let target = match to {
            Some(name) if me.kind == SOCK_DGRAM => *s.names.get(&name).ok_or(ECONNREFUSED)?,
            _ => me.peer.ok_or(ENOTCONN)?,
        };
        let n = data.len();
        let t = s.all.get_mut(&target).ok_or(ECONNREFUSED)?;
        t.rx.push_back((data, from));
        Ok(n)
    }

    /// Whether any process still has a descriptor of socket `id`.
    fn sock_open(&self, id: u32) -> bool {
        let names = |fds: &Vec<Option<super::Fd>>| fds.iter().flatten().any(|f| matches!(f, super::Fd::Dev { dev: Device::Other(r), .. } if *r == SOCK | id));
        // A connection not accepted yet is open too.
        names(&self.g.fds) || self.groups.values().any(|g| g.threads > 0 && names(&g.fds)) || self.x.sockets.all.values().any(|s| s.backlog.contains(&id))
    }

    /// A connected socket whose other end has been closed.
    fn sock_hung_up(&self, id: u32) -> bool {
        let Some(s) = self.x.sockets.all.get(&id) else { return true };
        s.kind != SOCK_DGRAM && s.peer.is_some_and(|p| !self.sock_open(p))
    }

    /// What a socket has to read; None when the caller now waits.
    fn sock_recv(&mut self, id: u32, len: usize, nonblock: bool, peek: bool) -> Result<Option<(Vec<u8>, Option<String>)>, i32> {
        let wait = self.clock() + std::time::Duration::from_millis(2);
        let hung_up = self.x.sockets.all.get(&id).is_some_and(|s| s.rx.is_empty()) && self.sock_hung_up(id);
        let s = self.x.sockets.all.get_mut(&id).ok_or(EINVAL)?;
        if s.rx.is_empty() && hung_up {
            return Ok(Some((Vec::new(), None)));
        }
        if s.rx.is_empty() {
            if nonblock {
                return Err(EAGAIN);
            }
            self.block = Some(Wait::Retry(wait));
            return Ok(None);
        }
        if s.kind == SOCK_STREAM {
            let mut out = Vec::new();
            let from = s.rx[0].1.clone();
            let mut i = 0;
            while out.len() < len && i < s.rx.len() {
                let (p, _) = &s.rx[i];
                let n = (len - out.len()).min(p.len());
                out.extend(&p[..n]);
                i += 1;
            }
            if !peek {
                let mut left = out.len();
                while left > 0 {
                    let p = &mut s.rx[0].0;
                    if p.len() <= left {
                        left -= p.len();
                        s.rx.pop_front();
                    } else {
                        p.drain(..left);
                        left = 0;
                    }
                }
            }
            return Ok(Some((out, from)));
        }
        let (mut p, from) = if peek { s.rx[0].clone() } else { s.rx.pop_front().unwrap() };
        p.truncate(len);
        Ok(Some((p, from)))
    }

    /// What a program does to tell another over the socket at `name`
    /// (connect, send, close): Ribbit's own messages (cnotify's).
    pub(super) fn sock_notify(&mut self, name: &str, data: Vec<u8>) {
        let Some(&target) = self.x.sockets.names.get(name) else { return };
        let kind = self.x.sockets.all[&target].kind;
        if kind == SOCK_DGRAM {
            self.x.sockets.all.get_mut(&target).unwrap().rx.push_back((data, None));
            return;
        }
        if !self.x.sockets.all[&target].listening {
            return;
        }
        // A connection whose client has already closed: the data, then
        // end of file.
        let (client, server) = (self.new_sock(kind), self.new_sock(kind));
        let s = &mut self.x.sockets;
        s.all.get_mut(&server).unwrap().peer = Some(client);
        s.all.get_mut(&server).unwrap().rx.push_back((data, None));
        s.all.get_mut(&client).unwrap().peer = Some(server);
        s.all.get_mut(&target).unwrap().backlog.push_back(server);
    }

    /// read() on a socket.
    pub(super) fn sock_read(&mut self, rdev: u32, nonblock: bool, len: usize) -> Result<Vec<u8>, i32> {
        Ok(self.sock_recv(rdev & 0xFFFF, len, nonblock, false)?.map(|d| d.0).unwrap_or_default())
    }

    /// write() on a socket.
    pub(super) fn sock_write(&mut self, rdev: u32, data: &[u8]) -> Result<usize, i32> {
        self.sock_send(rdev & 0xFFFF, data.to_vec(), None)
    }

    /// (readable, writable, hung up) for poll.
    pub(super) fn sock_ready(&self, rdev: u32) -> (bool, bool, bool) {
        let Some(s) = self.x.sockets.all.get(&(rdev & 0xFFFF)) else { return (false, false, true) };
        let hung_up = s.rx.is_empty() && self.sock_hung_up(rdev & 0xFFFF);
        (!s.rx.is_empty() || !s.backlog.is_empty() || hung_up, true, hung_up)
    }
}
