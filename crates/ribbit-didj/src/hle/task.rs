//! Processes and threads for the HLE: fork and thread clone, exit, wait4,
//! futexes, pipes, sleeping, and the scheduler that runs them in turn.
//!
//! The running task's registers live in `Process::cpu`, its process's
//! memory, files and directory in `Process::g` (a `Group`, shared by its
//! threads). The others wait in `Process::tasks` and `Process::groups`; a
//! switch moves them in and out (no copying).

use super::mem::{Mem, Space};
use super::{EAGAIN, EFAULT, Fd, Process, err};
use ribbit_arm::Arm;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Instructions a task runs before the next one gets a turn.
pub(super) const SLICE: u32 = 20_000;

/// The kernel's timer tick: 10 ms (HZ 100; DIDJ_HLE_TICK_US=<n> for others).
fn tick_ns() -> u64 {
    static TICK: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *TICK.get_or_init(|| std::env::var("DIDJ_HLE_TICK_US").ok().and_then(|v| v.parse::<u64>().ok()).map_or(10_000_000, |us| us.max(1) * 1000))
}

const CLONE_VM: u32 = 0x100;
const CLONE_THREAD: u32 = 0x1_0000;
const CLONE_SETTLS: u32 = 0x8_0000;
const CLONE_PARENT_SETTID: u32 = 0x10_0000;
const CLONE_CHILD_CLEARTID: u32 = 0x20_0000;
const CLONE_CHILD_SETTID: u32 = 0x100_0000;
const SIGCHLD: u32 = 17;
const ECHILD: i32 = 10;
const ESRCH: i32 = 3;
const ETIMEDOUT: i32 = 110;
const WNOHANG: u32 = 1;

/// A process: what its threads share.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Group {
    pub mem: Mem,
    pub fds: Vec<Option<Fd>>,
    pub cwd: String,
    pub umask: u32,
    pub brk_start: u32,
    pub brk: u32,
    pub tgid: u32,
    pub ppid: u32,
    /// Threads still running; 0 once it has exited.
    pub threads: u32,
}

/// A thread not running now.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Task {
    pub tid: u32,
    pub tgid: u32,
    pub cpu: Arm,
    pub tls: u32,
    pub clear_tid: u32,
    pub wait: Option<Wait>,
}

/// What a task is blocked on.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) enum Wait {
    /// FUTEX_WAIT; `woken` by FUTEX_WAKE. The call returns 0 (or
    /// ETIMEDOUT), it isn't run again.
    Futex {
        addr: u32,
        #[serde(with = "super::state::inst_opt")]
        until: Option<Instant>,
        woken: bool,
    },
    Sleep(#[serde(with = "super::state::inst")] Instant),
    /// Gives the others a turn first.
    Yield,
    /// wait4 for this pid (-1 any): the call runs again when a child ends.
    Child(i32),
    /// Reading an empty pipe: the call runs again when there is data.
    Pipe(#[serde(with = "super::state::shared")] Arc<Mutex<Pipe>>),
    /// mq_timedsend on a full queue (`send`) or mq_timedreceive on an empty
    /// one: runs again when there's room or a message; ETIMEDOUT at `until`.
    Mq {
        #[serde(with = "super::state::shared")]
        q: Arc<Mutex<Mq>>,
        send: bool,
        #[serde(with = "super::state::inst_opt")]
        until: Option<Instant>,
    },
    /// Reading /dev/input/eventN with no events: runs again on one.
    Input(usize),
    /// Writing /dev/dsp with its buffer full: runs again at the time.
    DspFull(#[serde(with = "super::state::inst")] Instant),
    /// poll/select with nothing ready: runs again at the time, to look
    /// again (its deadline is in `Process::poll_until`).
    Retry(#[serde(with = "super::state::inst")] Instant),
}

impl Wait {
    /// The system call is run again on waking (else it returns a value).
    pub fn restarts(&self) -> bool {
        matches!(self, Wait::Child(_) | Wait::Pipe(_) | Wait::Mq { .. } | Wait::Input(_) | Wait::DspFull(_) | Wait::Retry(_))
    }
}

#[derive(Default)]
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Pipe {
    pub data: VecDeque<u8>,
    pub readers: u32,
    pub writers: u32,
}

/// One end of a pipe; the pipe counts its open ends.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct PipeEnd {
    #[serde(with = "super::state::shared")]
    pub pipe: Arc<Mutex<Pipe>>,
    pub write: bool,
}

impl PipeEnd {
    fn new(pipe: &Arc<Mutex<Pipe>>, write: bool) -> Self {
        let mut p = pipe.lock().unwrap();
        if write { p.writers += 1 } else { p.readers += 1 }
        Self { pipe: pipe.clone(), write }
    }
}

impl Clone for PipeEnd {
    fn clone(&self) -> Self {
        Self::new(&self.pipe, self.write)
    }
}

impl Drop for PipeEnd {
    fn drop(&mut self) {
        let mut p = self.pipe.lock().unwrap();
        if self.write { p.writers -= 1 } else { p.readers -= 1 }
    }
}

/// A message queue: messages by priority, highest first.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Mq {
    pub msgs: Vec<(u32, Vec<u8>)>,
    pub maxmsg: u32,
    pub msgsize: u32,
}

const O_NONBLOCK: u32 = 0o4000;

/// A process ended, until its parent collects it with wait4.
#[derive(serde::Serialize, serde::Deserialize)]
pub(super) struct Zombie {
    pub pid: u32,
    pub ppid: u32,
    /// As wait4 reports it: exit code << 8, or the signal.
    pub status: u32,
}

impl Process {
    // ---- clone, exec, exit, wait ---------------------------------------

    pub(super) fn clone_task(&mut self, flags: u32, stack: u32, ptid: u32, tls: u32, ctid: u32) -> Result<u32, i32> {
        let tid = self.next_tid;
        self.next_tid += 1;
        let mut cpu = self.cpu.clone();
        cpu.r[0] = 0;
        if stack != 0 {
            cpu.r[13] = stack;
        }
        if flags & CLONE_PARENT_SETTID != 0 && !self.mem().put32(ptid, tid) {
            return Err(EFAULT);
        }
        let tgid = if flags & CLONE_THREAD != 0 {
            if flags & CLONE_CHILD_SETTID != 0 {
                self.mem().put32(ctid, tid);
            }
            self.g.threads += 1;
            self.g.tgid
        } else {
            if flags & CLONE_VM != 0 {
                self.log_once(format!("clone {flags:#X}: shared memory without CLONE_THREAD, copied instead"));
            }
            let mut child = Group {
                mem: self.mem().fork().ok_or(super::ENOMEM)?,
                fds: self.g.fds.clone(),
                cwd: self.g.cwd.clone(),
                umask: self.g.umask,
                brk_start: self.g.brk_start,
                brk: self.g.brk,
                tgid: tid,
                ppid: self.g.tgid,
                threads: 1,
            };
            if flags & CLONE_CHILD_SETTID != 0 {
                Space { mem: &mut child.mem, bus: &mut self.m.bus, frames: &mut self.frames }.put32(ctid, tid);
            }
            self.groups.insert(tid, child);
            tid
        };
        let clear_tid = if flags & CLONE_CHILD_CLEARTID != 0 { ctid } else { 0 };
        let tls = if flags & CLONE_SETTLS != 0 { tls } else { self.tls };
        self.tasks.push(Task { tid, tgid, cpu, tls, clear_tid, wait: None });
        Ok(tid)
    }

    pub(super) fn fork(&mut self) -> Result<u32, i32> {
        self.clone_task(SIGCHLD, 0, 0, 0, 0)
    }

    /// execve: the program replaces this process's memory; on success
    /// nothing returns to the old one.
    pub(super) fn execve(&mut self, path: u32, argv: u32, envp: u32) -> Result<u32, i32> {
        let path = self.path_arg(path)?;
        let argv = self.str_array(argv)?;
        let envp = self.str_array(envp)?;
        let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
        let envp: Vec<&str> = envp.iter().map(String::as_str).collect();
        match self.vfs.resolve(&self.g.cwd, &path, true) {
            Ok(p) if self.vfs.get(&p).is_some() => {}
            Ok(_) => return Err(super::vfs::ENOENT),
            Err(e) => return Err(e),
        }
        self.exec(&path, &argv, &envp).map_err(|e| {
            self.log_once(format!("execve {path}: {e}"));
            8 // ENOEXEC
        })?;
        self.skip_result = true;
        Ok(0)
    }

    fn str_array(&mut self, mut at: u32) -> Result<Vec<String>, i32> {
        let mut out = Vec::new();
        if at == 0 {
            return Ok(out);
        }
        loop {
            let p = self.mem().get32(at).ok_or(EFAULT)?;
            if p == 0 {
                return Ok(out);
            }
            out.push(self.mem().cstr(p).ok_or(EFAULT)?);
            at += 4;
        }
    }

    /// The running thread ends (exit).
    pub(super) fn exit_thread(&mut self, code: u32) {
        if self.clear_tid != 0 {
            let at = self.clear_tid;
            self.mem().put32(at, 0);
            self.futex_wake(self.clear_tid, 1);
        }
        self.alive = false;
        self.g.threads -= 1;
        if self.g.threads == 0 {
            self.end_group((code & 0xFF) << 8);
        }
    }

    /// The running process ends, all its threads (exit_group, or killed).
    pub(super) fn exit_group(&mut self, status: u32) {
        let tgid = self.g.tgid;
        self.tasks.retain(|t| t.tgid != tgid);
        self.alive = false;
        self.end_group(status);
    }

    fn end_group(&mut self, status: u32) {
        self.g.threads = 0;
        self.g.fds.clear();
        let (pid, ppid) = (self.g.tgid, self.g.ppid);
        if pid == 1 {
            self.exit = Some(if status & 0x7F != 0 { 128 + (status & 0x7F) as i32 } else { (status >> 8) as i32 });
        }
        // Orphans go to init, as on Linux.
        for g in self.groups.values_mut().filter(|g| g.ppid == pid) {
            g.ppid = 1;
        }
        for z in self.zombies.iter_mut().filter(|z| z.ppid == pid) {
            z.ppid = 1;
        }
        self.zombies.push(Zombie { pid, ppid, status });
    }

    fn has_children(&self, tgid: u32) -> bool {
        (self.g.ppid == tgid && self.g.threads > 0)
            || self.groups.values().any(|g| g.ppid == tgid)
            || self.zombies.iter().any(|z| z.ppid == tgid)
    }

    fn zombie_for(&self, tgid: u32, pid: i32) -> Option<usize> {
        self.zombies.iter().position(|z| z.ppid == tgid && (pid <= 0 || z.pid == pid as u32))
    }

    pub(super) fn wait4(&mut self, pid: u32, status: u32, options: u32, rusage: u32) -> Result<u32, i32> {
        let pid = pid as i32;
        if let Some(i) = self.zombie_for(self.g.tgid, pid) {
            let z = self.zombies.remove(i);
            if status != 0 && !self.mem().put32(status, z.status) {
                return Err(EFAULT);
            }
            if rusage != 0 {
                self.mem().write_bytes(rusage, &[0; 72]);
            }
            return Ok(z.pid);
        }
        if !self.has_children(self.g.tgid) {
            return Err(ECHILD);
        }
        if options & WNOHANG != 0 {
            return Ok(0);
        }
        self.block = Some(Wait::Child(pid));
        Ok(0)
    }

    /// kill/tkill/tgkill. Signals aren't delivered to handlers yet: one
    /// that would end a process ends it, the rest are logged and dropped.
    pub(super) fn kill(&mut self, pid: u32, sig: u32) -> Result<u32, i32> {
        let exists = pid == self.g.tgid || pid == self.tid || self.groups.contains_key(&pid) || self.tasks.iter().any(|t| t.tid == pid);
        if !exists {
            return Err(ESRCH);
        }
        let fatal = matches!(sig, 1..=3 | 6 | 9 | 11 | 13 | 14 | 15);
        if sig == 0 {
            return Ok(0);
        }
        if !fatal || self.handlers.get(sig as usize).is_some_and(|&h| h > 1) {
            self.log_once(format!("signal {sig} to {pid}: not delivered (signal handlers not done yet)"));
            return Ok(0);
        }
        let tgid = if pid == self.tid { self.g.tgid } else { self.tasks.iter().find(|t| t.tid == pid).map_or(pid, |t| t.tgid) };
        if tgid == self.g.tgid {
            self.exit_group(sig);
        } else if let Some(mut g) = self.groups.remove(&tgid) {
            self.tasks.retain(|t| t.tgid != tgid);
            // Ends it as `end_group` does, for a process not running now.
            std::mem::swap(&mut self.g, &mut g);
            self.end_group(sig);
            std::mem::swap(&mut self.g, &mut g);
        }
        Ok(0)
    }

    // ---- futexes, sleeping, pipes ---------------------------------------

    /// Wakes up to `n` threads of this process waiting on `addr`.
    pub(super) fn futex_wake(&mut self, addr: u32, n: u32) -> u32 {
        let tgid = self.g.tgid;
        let mut woke = 0;
        for t in self.tasks.iter_mut().filter(|t| t.tgid == tgid) {
            if woke == n {
                break;
            }
            if let Some(Wait::Futex { addr: a, woken, .. }) = &mut t.wait
                && *a == addr
                && !*woken
            {
                *woken = true;
                woke += 1;
            }
        }
        woke
    }

    pub(super) fn futex(&mut self, a: [u32; 6]) -> Result<u32, i32> {
        let (addr, op, val, timeout, addr2, val3) = (a[0], a[1], a[2], a[3], a[4], a[5]);
        let word = |p: &mut Self, at: u32| p.mem().get32(at).ok_or(EFAULT);
        match op & 0x7F {
            0 => {
                if word(self, addr)? != val {
                    return Err(EAGAIN);
                }
                let until = if timeout == 0 {
                    None
                } else {
                    // kernel/futex.c: timespec_to_jiffies + 1 ticks.
                    let (s, ns) = (word(self, timeout)?, word(self, timeout + 4)?);
                    Some(self.ticks_from_now(jiffies(Duration::new(s as u64, ns.min(999_999_999))) + 1))
                };
                self.block = Some(Wait::Futex { addr, until, woken: false });
                Ok(0)
            }
            1 => Ok(self.futex_wake(addr, val)),
            3 | 4 => {
                if op & 0x7F == 4 && word(self, addr)? != val3 {
                    return Err(EAGAIN);
                }
                let woke = self.futex_wake(addr, val);
                let tgid = self.g.tgid;
                let mut moved = 0;
                for t in self.tasks.iter_mut().filter(|t| t.tgid == tgid) {
                    if let Some(Wait::Futex { addr: at, woken: false, .. }) = &mut t.wait
                        && *at == addr
                        && moved < timeout
                    {
                        *at = addr2;
                        moved += 1;
                    }
                }
                Ok(woke + if op & 0x7F == 4 { moved } else { 0 })
            }
            5 => {
                // FUTEX_WAKE_OP: change *addr2, wake on addr, and on addr2
                // too if its old value passes the test.
                let (kind, cmp) = ((val3 >> 28) & 7, (val3 >> 24) & 15);
                let mut arg = (val3 >> 12) & 0xFFF;
                if val3 & (8 << 28) != 0 {
                    arg = 1 << arg.min(31);
                }
                let cmparg = val3 & 0xFFF;
                let old = word(self, addr2)?;
                let new = match kind {
                    0 => arg,
                    1 => old.wrapping_add(arg),
                    2 => old | arg,
                    3 => old & !arg,
                    _ => old ^ arg,
                };
                self.mem().put32(addr2, new);
                let mut woke = self.futex_wake(addr, val);
                let (o, c) = (old as i32, cmparg as i32);
                let pass = match cmp {
                    0 => o == c,
                    1 => o != c,
                    2 => o < c,
                    3 => o <= c,
                    4 => o > c,
                    _ => o >= c,
                };
                if pass {
                    woke += self.futex_wake(addr2, timeout);
                }
                Ok(woke)
            }
            other => {
                self.log_once(format!("futex op {other} not handled"));
                Err(super::ENOSYS)
            }
        }
    }

    pub(super) fn nanosleep(&mut self, req: u32) -> Result<u32, i32> {
        let s = self.mem().get32(req).ok_or(EFAULT)?;
        let ns = self.mem().get32(req + 4).ok_or(EFAULT)?;
        let d = Duration::new(s as u64, ns.min(999_999_999));
        if self.trace {
            eprintln!("[hle {}] nanosleep {d:?}", self.tid);
        }
        // Even a zero sleep waits for the next tick: 2.6.20's do_nanosleep
        // has no shortcut, and the timer only fires from the tick.
        self.block = Some(Wait::Sleep(self.tick_after(self.clock() + d)));
        Ok(0)
    }

    /// The first timer tick at or after `t`. The LF1000 kernel has HZ 100
    /// and no clock events (arch/arm/mach-lf1000/core.c), so hrtimers run
    /// from the tick (kernel/timer.c run_timer_softirq): a sleep ends on
    /// the first tick after its deadline.
    pub(super) fn tick_after(&self, t: Instant) -> Instant {
        let tick = tick_ns() as u128;
        let ns = t.saturating_duration_since(self.start).as_nanos();
        self.start + Duration::from_nanos((ns.div_ceil(tick) * tick) as u64)
    }

    /// The `n`th timer tick from now: when schedule_timeout(n) ends (the
    /// timeouts of poll, select, futex and message queues).
    pub(super) fn ticks_from_now(&self, n: u64) -> Instant {
        let tick = tick_ns();
        let ticks = self.since_start().as_nanos() as u64 / tick;
        self.start + Duration::from_nanos((ticks + n) * tick)
    }

    // ---- poll and select -------------------------------------------------

    /// Whether `fd` can be read without blocking, written, and has hung up.
    fn fd_ready(&self, fd: u32) -> Option<(bool, bool, bool)> {
        Some(match self.fd(fd).ok()? {
            Fd::Console => (false, true, false),
            Fd::File { .. } | Fd::Dir { .. } => (true, true, false),
            Fd::Pipe(end) => {
                let p = end.pipe.lock().unwrap();
                if end.write { (false, p.readers > 0, p.readers == 0) } else { (!p.data.is_empty(), false, p.writers == 0) }
            }
            Fd::Mq { q, .. } => {
                let q = q.lock().unwrap();
                (!q.msgs.is_empty(), q.msgs.len() < q.maxmsg as usize, false)
            }
            &Fd::Dev { dev, .. } => self.dev_ready(dev),
        })
    }

    /// Nothing ready: wait a little and run the call again, until
    /// `ticks` timer ticks from the first call (None: no deadline); true
    /// when it has passed.
    fn poll_wait(&mut self, ticks: Option<u64>) -> bool {
        let now = self.clock();
        let until = match ticks {
            Some(0) => return true,
            Some(n) => {
                let at = self.ticks_from_now(n);
                *self.poll_until.entry(self.tid).or_insert(at)
            }
            None => now + Duration::from_secs(3600),
        };
        if until <= now {
            self.poll_until.remove(&self.tid);
            return true;
        }
        // 2 ms: the scheduler has no wake-up for "any of these fds".
        self.block = Some(Wait::Retry((now + Duration::from_millis(2)).min(until)));
        false
    }

    /// poll(fds, nfds, timeout ms).
    pub(super) fn poll(&mut self, fds: u32, nfds: u32, timeout: i32) -> Result<u32, i32> {
        const POLLIN: u16 = 0x1;
        const POLLOUT: u16 = 0x4;
        const POLLHUP: u16 = 0x10;
        const POLLNVAL: u16 = 0x20;
        let mut count = 0;
        for i in 0..nfds {
            let at = fds + i * 8;
            let fd = self.mem().get32(at).ok_or(EFAULT)? as i32;
            let events = self.mem().get32(at + 4).ok_or(EFAULT)? as u16;
            let revents = if fd < 0 {
                0
            } else if let Some((r, w, hup)) = self.fd_ready(fd as u32) {
                // POLLRDNORM (0x40) and POLLWRNORM (0x100) count as IN and OUT.
                let mut m = 0;
                if r {
                    m |= events & (POLLIN | 0x40);
                }
                if w {
                    m |= events & (POLLOUT | 0x100);
                }
                if hup {
                    m |= POLLHUP;
                }
                m
            } else {
                POLLNVAL
            };
            let word = events as u32 | (revents as u32) << 16;
            self.mem().put32(at + 4, word);
            count += (revents != 0) as u32;
        }
        // fs/select.c: msecs_to_jiffies.
        if count > 0 || self.poll_wait(if timeout < 0 { None } else { Some((timeout as u64).div_ceil(10)) }) {
            self.poll_until.remove(&self.tid);
            return Ok(count);
        }
        Ok(0)
    }

    /// _newselect(nfds, readfds, writefds, exceptfds, timeout).
    pub(super) fn select(&mut self, a: [u32; 6]) -> Result<u32, i32> {
        let (nfds, sets) = (a[0].min(1024), [a[1], a[2], a[3]]);
        let words = nfds.div_ceil(32);
        let mut out = [vec![0u32; words as usize], vec![0u32; words as usize], vec![0u32; words as usize]];
        let mut count = 0;
        for fd in 0..nfds {
            let (w, bit) = ((fd / 32) as usize, 1u32 << (fd % 32));
            let want: Vec<bool> = sets
                .iter()
                .map(|&s| s != 0 && self.mem().get32(s + w as u32 * 4).is_some_and(|v| v & bit != 0))
                .collect();
            if !want.iter().any(|&x| x) {
                continue;
            }
            let (r, wr, hup) = self.fd_ready(fd).ok_or(super::EBADF)?;
            for (k, ready) in [r || hup, wr, false].into_iter().enumerate() {
                if want[k] && ready {
                    out[k][w] |= bit;
                    count += 1;
                }
            }
        }
        let timeout = if a[4] == 0 {
            None
        } else {
            let s = self.mem().get32(a[4]).ok_or(EFAULT)? as u64;
            let us = self.mem().get32(a[4] + 4).ok_or(EFAULT)? as u64;
            // fs/select.c: rounded up to ticks.
            Some(s * 100 + us.div_ceil(10_000))
        };
        if count == 0 && !self.poll_wait(timeout) {
            return Ok(0);
        }
        self.poll_until.remove(&self.tid);
        for (k, &set) in sets.iter().enumerate() {
            if set != 0 {
                for (i, v) in out[k].iter().enumerate() {
                    self.mem().put32(set + i as u32 * 4, *v);
                }
            }
        }
        Ok(count)
    }

    pub(super) fn pipe(&mut self, fds: u32) -> Result<u32, i32> {
        let pipe = Arc::new(Mutex::new(Pipe::default()));
        let r = self.new_fd(Fd::Pipe(PipeEnd::new(&pipe, false)), 0);
        let w = self.new_fd(Fd::Pipe(PipeEnd::new(&pipe, true)), 0);
        if !(self.mem().put32(fds, r) && self.mem().put32(fds + 4, w)) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    /// Reads from a pipe; blocks while it's empty and has writers.
    pub(super) fn pipe_read(&mut self, end: &PipeEnd, len: usize) -> Result<Vec<u8>, i32> {
        let mut p = end.pipe.lock().unwrap();
        if p.data.is_empty() && p.writers > 0 {
            drop(p);
            self.block = Some(Wait::Pipe(end.pipe.clone()));
            return Ok(Vec::new());
        }
        let n = len.min(p.data.len());
        Ok(p.data.drain(..n).collect())
    }

    pub(super) fn pipe_write(end: &PipeEnd, data: &[u8]) -> Result<usize, i32> {
        let mut p = end.pipe.lock().unwrap();
        if p.readers == 0 {
            return Err(32); // EPIPE
        }
        p.data.extend(data);
        Ok(data.len())
    }

    // ---- POSIX message queues (the Brio event system's) -----------------

    /// mq_open(name, oflag, mode, attr): uClibc passes the name without
    /// its leading '/'.
    pub(super) fn mq_open(&mut self, name: u32, flags: u32, attr: u32) -> Result<u32, i32> {
        const O_CREAT: u32 = 0o100;
        const O_EXCL: u32 = 0o200;
        let name = self.path_arg(name)?;
        let q = match self.queues.get(&name) {
            Some(_) if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL => return Err(17), // EEXIST
            Some(q) => q.clone(),
            None if flags & O_CREAT == 0 => return Err(super::vfs::ENOENT),
            None => {
                // Linux's defaults: 10 messages of 8 KiB.
                let (maxmsg, msgsize) = if attr == 0 {
                    (10, 8192)
                } else {
                    (self.mem().get32(attr + 4).ok_or(EFAULT)?, self.mem().get32(attr + 8).ok_or(EFAULT)?)
                };
                if maxmsg == 0 || msgsize == 0 {
                    return Err(super::EINVAL);
                }
                let q = Arc::new(Mutex::new(Mq { msgs: Vec::new(), maxmsg, msgsize }));
                self.queues.insert(name, q.clone());
                q
            }
        };
        Ok(self.new_fd(Fd::Mq { q, nonblock: flags & O_NONBLOCK != 0 }, 0))
    }

    fn mq_fd(&self, fd: u32) -> Result<(Arc<Mutex<Mq>>, bool), i32> {
        match self.fd(fd)? {
            Fd::Mq { q, nonblock } => Ok((q.clone(), *nonblock)),
            _ => Err(super::EBADF),
        }
    }

    /// An absolute CLOCK_REALTIME timespec, as an Instant.
    fn deadline(&mut self, ts: u32) -> Result<Option<Instant>, i32> {
        if ts == 0 {
            return Ok(None);
        }
        let s = self.mem().get32(ts).ok_or(EFAULT)? as u64;
        let ns = self.mem().get32(ts + 4).ok_or(EFAULT)?.min(999_999_999);
        let at = Duration::new(s, ns);
        let now = self.realtime();
        // ipc/mqueue.c prepare_timeout: timespec_to_jiffies + 1 ticks.
        Ok(Some(if at <= now { self.clock() } else { self.ticks_from_now(jiffies(at - now) + 1) }))
    }

    pub(super) fn mq_send(&mut self, fd: u32, msg: u32, len: u32, prio: u32, timeout: u32) -> Result<u32, i32> {
        let (q, nonblock) = self.mq_fd(fd)?;
        let until = self.deadline(timeout)?;
        let data = self.mem().read_bytes(msg, len as usize).ok_or(EFAULT)?;
        let mut m = q.lock().unwrap();
        if len > m.msgsize {
            return Err(90); // EMSGSIZE
        }
        if m.msgs.len() >= m.maxmsg as usize {
            if nonblock {
                return Err(EAGAIN);
            }
            drop(m);
            self.block = Some(Wait::Mq { q, send: true, until });
            return Ok(0);
        }
        // Highest priority first, oldest first within one.
        let at = m.msgs.iter().position(|(p, _)| *p < prio).unwrap_or(m.msgs.len());
        m.msgs.insert(at, (prio, data));
        Ok(0)
    }

    pub(super) fn mq_receive(&mut self, fd: u32, buf: u32, len: u32, prio: u32, timeout: u32) -> Result<u32, i32> {
        let (q, nonblock) = self.mq_fd(fd)?;
        let until = self.deadline(timeout)?;
        let mut m = q.lock().unwrap();
        if len < m.msgsize {
            return Err(90); // EMSGSIZE
        }
        if m.msgs.is_empty() {
            if nonblock {
                return Err(EAGAIN);
            }
            drop(m);
            self.block = Some(Wait::Mq { q, send: false, until });
            return Ok(0);
        }
        let (p, data) = m.msgs.remove(0);
        drop(m);
        if !self.mem().write_bytes(buf, &data) || (prio != 0 && !self.mem().put32(prio, p)) {
            return Err(EFAULT);
        }
        Ok(data.len() as u32)
    }

    /// mq_getsetattr(fd, new, old): struct mq_attr is flags, maxmsg,
    /// msgsize, curmsgs; only O_NONBLOCK can change.
    pub(super) fn mq_getsetattr(&mut self, fd: u32, new: u32, old: u32) -> Result<u32, i32> {
        let (q, nonblock) = self.mq_fd(fd)?;
        if old != 0 {
            let m = q.lock().unwrap();
            let words = [if nonblock { O_NONBLOCK } else { 0 }, m.maxmsg, m.msgsize, m.msgs.len() as u32];
            drop(m);
            for (i, w) in words.iter().enumerate() {
                if !self.mem().put32(old + 4 * i as u32, *w) {
                    return Err(EFAULT);
                }
            }
        }
        if new != 0 {
            let flags = self.mem().get32(new).ok_or(EFAULT)?;
            if let Some(Some(Fd::Mq { nonblock, .. })) = self.g.fds.get_mut(fd as usize) {
                *nonblock = flags & O_NONBLOCK != 0;
            }
        }
        Ok(0)
    }

    pub(super) fn mq_unlink(&mut self, name: u32) -> Result<u32, i32> {
        let name = self.path_arg(name)?;
        self.queues.remove(&name).map(|_| 0).ok_or(super::vfs::ENOENT)
    }

    // ---- the scheduler ---------------------------------------------------

    /// Parks the running task (unless it ended) and switches to the first
    /// one ready, sleeping when all of them wait for a time. False when
    /// nothing can run again.
    pub(super) fn schedule(&mut self) -> bool {
        if self.alive {
            let t = Task {
                tid: self.tid,
                tgid: self.g.tgid,
                cpu: std::mem::take(&mut self.cpu),
                tls: self.tls,
                clear_tid: self.clear_tid,
                wait: self.block.take(),
            };
            self.tasks.push(t);
            self.alive = false;
        }
        loop {
            let now = self.clock();
            let mut earliest: Option<Instant> = None;
            for i in 0..self.tasks.len() {
                if self.wake(i, now) {
                    self.switch_to(i);
                    return true;
                }
                if let Some(Wait::Sleep(u) | Wait::Futex { until: Some(u), .. } | Wait::Mq { until: Some(u), .. } | Wait::DspFull(u) | Wait::Retry(u)) = self.tasks[i].wait {
                    earliest = Some(earliest.map_or(u, |e| e.min(u)));
                }
            }
            match earliest {
                Some(u) => {
                    // To the host's time less pauses only, so `late` shrinks.
                    std::thread::sleep(u.saturating_duration_since(self.host() + self.late));
                    self.on_time(u);
                }
                None => {
                    self.idle = true;
                    return false;
                }
            }
        }
    }

    /// Whether parked task `i` can run now; its call's result is set.
    fn wake(&mut self, i: usize, now: Instant) -> bool {
        let tgid = self.tasks[i].tgid;
        let (ready, r0) = match &self.tasks[i].wait {
            None | Some(Wait::Yield) => (true, None),
            Some(Wait::Futex { woken: true, .. }) => (true, Some(0)),
            Some(Wait::Futex { until: Some(u), .. }) if *u <= now => (true, Some(err(ETIMEDOUT))),
            Some(Wait::Futex { .. }) => (false, None),
            Some(Wait::Sleep(u)) => (*u <= now, Some(0)),
            Some(Wait::Child(pid)) => (self.zombie_for(tgid, *pid).is_some() || !self.has_children(tgid), None),
            Some(Wait::Mq { until: Some(u), .. }) if *u <= now => {
                // Timed out: past the call (it was set to run again).
                let t = &mut self.tasks[i];
                t.cpu.r[15] += if t.cpu.cpsr & ribbit_arm::psr::T != 0 { 2 } else { 4 };
                (true, Some(err(ETIMEDOUT)))
            }
            Some(Wait::Mq { q, send, .. }) => {
                let q = q.lock().unwrap();
                (if *send { q.msgs.len() < q.maxmsg as usize } else { !q.msgs.is_empty() }, None)
            }
            Some(Wait::Pipe(p)) => {
                let p = p.lock().unwrap();
                (!p.data.is_empty() || p.writers == 0, None)
            }
            Some(w @ (Wait::Input(_) | Wait::DspFull(_) | Wait::Retry(_))) => (super::dev::ready(&self.dev, w, now), None),
        };
        if ready {
            let t = &mut self.tasks[i];
            t.wait = None;
            if let Some(v) = r0 {
                t.cpu.r[0] = v;
            }
        }
        ready
    }

    fn switch_to(&mut self, i: usize) {
        let t = self.tasks.remove(i);
        if t.tgid != self.g.tgid {
            let incoming = self.groups.remove(&t.tgid).expect("a task's process");
            let out = std::mem::replace(&mut self.g, incoming);
            if out.threads > 0 {
                self.groups.insert(out.tgid, out);
            }
        }
        self.cpu = t.cpu;
        self.tid = t.tid;
        self.tls = t.tls;
        self.clear_tid = t.clear_tid;
        self.block = None;
        self.alive = true;
    }
}

/// timespec_to_jiffies: rounded up to whole ticks (10 ms at HZ 100).
fn jiffies(d: Duration) -> u64 {
    d.as_nanos().div_ceil(tick_ns() as u128) as u64
}
