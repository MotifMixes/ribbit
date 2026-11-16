//! Threads: the game's pthreads on Ribbit's one CPU, switched cooperatively (when a thread
//! waits) and by time slice. Each thread keeps its registers, VFP state and pthread keys; the
//! running one's are in the CPU. A thread is switched only at the outermost call level (no
//! nested call from Rust in progress), so Rust's stack never holds another thread's frames.
//!
//! A call that has to wait (a held mutex, a join) leaves the thread at the import's slot and
//! runs again when the thread is woken. `pthread_cond_wait` keeps what it gave up (`Resume`)
//! to take the mutex back. Timed waits and sleeps wake at their deadline; when every thread
//! waits, the clock skips to the earliest deadline instead of sleeping.

use crate::runtime::{Runtime, Stop};
use ribbit_arm::V7;
use std::collections::BTreeMap;
use std::time::Duration;

/// Where a thread's entry function returns to: the thread ends.
pub const THREAD_EXIT: u32 = 0xEFFF_E000;
/// Instructions a thread runs before another ready one gets the CPU.
const SLICE: u64 = 200_000;
const STACK: u32 = 0x0010_0000;
/// A frame on the tablet's screen (60 Hz).
pub const FRAME: Duration = Duration::from_nanos(16_666_667);
/// Instructions the other threads may run while the main one waits for the vertical blank:
/// about what one of the tablet's cores (Cortex-A7/A9, 1-1.3 GHz) runs in a frame (GUESS).
const FRAME_INSNS: u64 = 16_000_000;
const ETIMEDOUT: u32 = 110;
const EBUSY: u32 = 16;

#[derive(Clone)]
struct Context {
    r: [u32; 16],
    cpsr: u32,
    v7: Option<V7>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Wait {
    Mutex(u32),
    /// A condition variable, until a deadline (on the monotonic clock) if timed.
    Cond(u32, Option<Duration>),
    Join(usize),
    Sleep(Duration),
    /// The vertical blank at this time (`wait_vsync`).
    Vsync(Duration),
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum State {
    Ready,
    Waiting(Wait),
    Done(u32),
}

/// What a woken `pthread_cond_wait` takes back: the mutex and its lock count.
#[derive(Clone, Copy)]
struct Resume {
    mutex: u32,
    count: u32,
    timed_out: bool,
}

struct Thread {
    context: Option<Context>,
    tls: BTreeMap<u32, u32>,
    state: State,
    resume: Option<Resume>,
    entry: u32,
}

pub struct Threads {
    list: Vec<Thread>,
    /// Index of the running thread (its id is this + 1).
    current: usize,
    /// Locked mutexes: owner index, lock count (all are treated as recursive).
    mutexes: BTreeMap<u32, (usize, u32)>,
    slice_left: u64,
    /// The running thread changed during the last import.
    pub(crate) switched: bool,
    /// Time skipped while every thread waited, added to the clocks.
    pub(crate) skew: Duration,
    /// The last vertical blank (`wait_vsync`), on the monotonic clock.
    vsync: Duration,
    /// Instructions the other threads have left before the vertical blank ends the wait.
    frame_left: u64,
}

impl Default for Threads {
    fn default() -> Self {
        let main = Thread { context: None, tls: BTreeMap::new(), state: State::Ready, resume: None, entry: 0 };
        Self { list: vec![main], current: 0, mutexes: BTreeMap::new(), slice_left: SLICE, switched: false, skew: Duration::ZERO, vsync: Duration::ZERO, frame_left: 0 }
    }
}

impl Threads {
    /// The running thread's pthread_t (1 for the thread the natives are called on).
    pub fn current_id(&self) -> u32 {
        self.current as u32 + 1
    }

    /// Threads started and not ended (the main one included).
    pub fn alive(&self) -> usize {
        self.list.iter().filter(|t| !matches!(t.state, State::Done(_))).count()
    }

    /// One line per thread, for reports.
    pub fn describe(&self) -> Vec<String> {
        self.list.iter().enumerate().map(|(i, t)| format!("thread {} (entry {:#x}): {:?}", i + 1, t.entry, t.state)).collect()
    }
}

impl Runtime {
    fn save_context(&mut self) -> Context {
        Context { r: self.cpu.r, cpsr: self.cpu.cpsr, v7: self.cpu.v7.as_deref().cloned() }
    }

    fn load_context(&mut self, c: Context) {
        self.cpu.r = c.r;
        self.cpu.cpsr = c.cpsr;
        if let (Some(v7), Some(mut saved)) = (self.cpu.v7.as_mut(), c.v7) {
            // A context switch clears the exclusive monitor (CLREX).
            saved.monitor = None;
            **v7 = saved;
        }
    }

    /// Hands the CPU to thread index `to`.
    fn switch_to(&mut self, to: usize) {
        let from = self.threads.current;
        if to == from {
            return;
        }
        let c = self.save_context();
        self.threads.list[from].context = Some(c);
        std::mem::swap(&mut self.tls, &mut self.threads.list[from].tls);
        let c = self.threads.list[to].context.take().expect("a waiting thread has its context");
        if self.trace.is_some() {
            eprintln!("[switch] t{} pc {:#x} lr {:#x} -> t{} pc {:#x} lr {:#x} ({:?})", from + 1, self.cpu.r[15], self.cpu.r[14], to + 1, c.r[15], c.r[14], self.threads.list[from].state);
        }
        self.load_context(c);
        std::mem::swap(&mut self.tls, &mut self.threads.list[to].tls);
        self.threads.current = to;
        self.threads.switched = true;
        self.threads.slice_left = SLICE;
    }

    /// Starts a thread at `entry(arg)`; it first runs when the running one waits or its slice
    /// ends. Returns its pthread_t.
    pub fn spawn(&mut self, entry: u32, arg: u32) -> u32 {
        let stack = self.malloc(STACK);
        let mut r = [0u32; 16];
        r[0] = arg;
        r[13] = (stack + STACK) & !7;
        r[14] = THREAD_EXIT;
        r[15] = entry & !1;
        let t = ribbit_arm::psr::T;
        let cpsr = if entry & 1 != 0 { self.cpu.cpsr | t } else { self.cpu.cpsr & !t } & !0xF000_0000;
        // IT state lives in the CPSR too (bits 25-26, 10-15): a new thread has none.
        let cpsr = cpsr & !(0x0600_FC00);
        let v7 = self.cpu.v7.as_deref().map(|v| V7 { s: [0; 64], monitor: None, ..v.clone() });
        self.threads.list.push(Thread { context: Some(Context { r, cpsr, v7 }), tls: BTreeMap::new(), state: State::Ready, resume: None, entry });
        self.threads.list.len() as u32
    }

    /// What GLSurfaceView's thread does between `naDrawFrame`s: eglSwapBuffers waits for the
    /// next vertical blank, while the game's other threads (loaders, sound) run on the other
    /// cores. Here they get up to FRAME_INSNS instructions, or until they all wait. The clock
    /// is the host's and this CPU is far slower than the tablet's, so a wait by time alone
    /// gave a loading thread one 200,000-instruction slice a frame: Academy T-Rex Rush, whose
    /// image decoder is unoptimised code, showed its menu after about 900 frames.
    pub fn wait_vsync(&mut self) -> Result<(), Stop> {
        let now = self.state_elapsed();
        self.threads.vsync = (self.threads.vsync + FRAME).max(now);
        self.threads.frame_left = FRAME_INSNS;
        let f = self.host_fn("vsync");
        self.call(f, &[]).map(|_| ())
    }

    /// Whether the running thread may be switched now.
    fn can_switch(&self) -> bool {
        self.depth <= 1
    }

    /// The running thread waits for `w`; another one runs.
    fn wait(&mut self, w: Wait) -> Result<(), Stop> {
        if !self.can_switch() {
            return Err(Stop::Halt(format!("thread {} would wait ({w:?}) inside a nested call", self.threads.current_id())));
        }
        let cur = self.threads.current;
        self.threads.list[cur].state = State::Waiting(w);
        self.schedule()
    }

    /// Picks the next thread to run: deadlines that passed wake first, then the ready threads
    /// in turn after the running one. With none ready, the clock skips to the next deadline.
    pub(crate) fn schedule(&mut self) -> Result<(), Stop> {
        loop {
            let now = self.state_elapsed();
            for t in &mut self.threads.list {
                match t.state {
                    State::Waiting(Wait::Sleep(d)) if d <= now => t.state = State::Ready,
                    State::Waiting(Wait::Vsync(_)) if self.threads.frame_left == 0 => t.state = State::Ready,
                    State::Waiting(Wait::Cond(_, Some(d))) if d <= now => {
                        t.state = State::Ready;
                        if let Some(r) = t.resume.as_mut() {
                            r.timed_out = true;
                        }
                    }
                    _ => {}
                }
            }
            let n = self.threads.list.len();
            let cur = self.threads.current;
            if let Some(next) = (1..=n).map(|k| (cur + k) % n).find(|&i| self.threads.list[i].state == State::Ready) {
                self.switch_to(next);
                return Ok(());
            }
            let next_deadline = self
                .threads
                .list
                .iter()
                .filter_map(|t| match t.state {
                    State::Waiting(Wait::Sleep(d) | Wait::Cond(_, Some(d))) => Some(d),
                    _ => None,
                })
                .min();
            // Nothing else to run before it: the vertical blank comes (at its time, or now
            // if that is past).
            let vsync = self.threads.list.iter().position(|t| matches!(t.state, State::Waiting(Wait::Vsync(_))));
            if let Some(i) = vsync
                && let State::Waiting(Wait::Vsync(d)) = self.threads.list[i].state
                && next_deadline.is_none_or(|n| n >= d)
            {
                self.threads.skew += d.saturating_sub(now);
                self.threads.list[i].state = State::Ready;
                self.switch_to(i);
                return Ok(());
            }
            match next_deadline {
                Some(d) => self.threads.skew += d.saturating_sub(now) + Duration::from_micros(1),
                None => return Err(Stop::Halt(format!("every thread waits: {}", self.threads.describe().join("; ")))),
            }
        }
    }

    /// The running thread's time slice: another ready thread gets the CPU when it ends.
    pub(crate) fn tick(&mut self) -> Result<(), Stop> {
        self.threads.frame_left = self.threads.frame_left.saturating_sub(1);
        if self.threads.slice_left > 0 {
            self.threads.slice_left -= 1;
            return Ok(());
        }
        self.threads.slice_left = SLICE;
        if self.can_switch() && self.threads.list.len() > 1 && std::env::var_os("LFA_NO_SLICE").is_none() {
            self.schedule()?;
        }
        Ok(())
    }

    /// The running thread returned from its entry function (or called pthread_exit).
    pub(crate) fn thread_exit(&mut self, value: u32) -> Result<(), Stop> {
        let cur = self.threads.current;
        if cur == 0 {
            return Err(Stop::Halt("the main thread exited".into()));
        }
        self.threads.list[cur].state = State::Done(value);
        for t in &mut self.threads.list {
            if t.state == State::Waiting(Wait::Join(cur)) {
                t.state = State::Ready;
            }
        }
        // Mutexes it held are let go.
        self.threads.mutexes.retain(|_, (owner, _)| *owner != cur);
        self.schedule()
    }

    fn wake_mutex_waiters(&mut self, m: u32) {
        for t in &mut self.threads.list {
            if t.state == State::Waiting(Wait::Mutex(m)) {
                t.state = State::Ready;
            }
        }
    }

    /// Takes mutex `m` for the running thread if it's free (or already its own).
    fn try_lock(&mut self, m: u32, count: u32) -> bool {
        let cur = self.threads.current;
        match self.threads.mutexes.get_mut(&m) {
            Some((owner, n)) if *owner == cur => {
                *n += count;
                true
            }
            Some(_) => false,
            None => {
                self.threads.mutexes.insert(m, (cur, count));
                true
            }
        }
    }

    /// Lets go of mutex `m` entirely; returns the lock count it had.
    fn unlock_all(&mut self, m: u32) -> u32 {
        let cur = self.threads.current;
        match self.threads.mutexes.get(&m) {
            Some(&(owner, n)) if owner == cur => {
                self.threads.mutexes.remove(&m);
                self.wake_mutex_waiters(m);
                n
            }
            _ => 0,
        }
    }

    /// A deadline from a `struct timespec` interval at `ts` from now (monotonic clock).
    fn deadline_after(&mut self, ts: u32) -> Duration {
        let (s, ns) = (self.mem.r32(ts), self.mem.r32(ts + 4));
        self.state_elapsed() + Duration::new(s as u64, ns.min(999_999_999))
    }
}

/// The imports this module answers.
pub fn handles(name: &str) -> bool {
    matches!(
        name,
        "pthread_create"
            | "pthread_join"
            | "pthread_exit"
            | "pthread_self"
            | "pthread_mutex_lock"
            | "pthread_mutex_trylock"
            | "pthread_mutex_unlock"
            | "pthread_mutex_destroy"
            | "pthread_cond_wait"
            | "pthread_cond_timedwait"
            | "pthread_cond_signal"
            | "pthread_cond_broadcast"
            | "pthread_getspecific"
            | "pthread_setspecific"
            | "sched_yield"
            | "usleep"
            | "nanosleep"
            | "sleep"
    )
}

pub fn call(rt: &mut Runtime, name: &str) -> Result<(), Stop> {
    match name {
        // pthread_create(thread*, attr, entry, arg); the attributes' stack size isn't used.
        "pthread_create" => {
            let (out, f, arg) = (rt.arg(0), rt.arg(2), rt.arg(3));
            let id = rt.spawn(f, arg);
            rt.mem.w32(out, id);
            crate::imports::log_line(rt, format!("ribbit: thread {id} started at {f:#x}({arg:#x})"));
            rt.ret(0)
        }
        // pthread_join(thread, void** value): waits for it to end.
        "pthread_join" => {
            let (id, out) = (rt.arg(0), rt.arg(1));
            let i = id.wrapping_sub(1) as usize;
            match rt.threads.list.get(i).map(|t| t.state) {
                Some(State::Done(v)) => {
                    if out != 0 {
                        rt.mem.w32(out, v);
                    }
                    rt.ret(0)
                }
                Some(_) if i != rt.threads.current => return rt.wait(Wait::Join(i)),
                // Itself (EDEADLK) or no such thread (ESRCH).
                Some(_) => rt.ret(35),
                None => rt.ret(3),
            }
        }
        "pthread_exit" => {
            let v = rt.arg(0);
            return rt.thread_exit(v);
        }
        "pthread_self" => {
            let id = rt.threads.current_id();
            rt.ret(id)
        }
        "pthread_mutex_lock" => {
            let m = rt.arg(0);
            if rt.try_lock(m, 1) {
                rt.ret(0)
            } else {
                // Runs again when the mutex is let go.
                return rt.wait(Wait::Mutex(m));
            }
        }
        "pthread_mutex_trylock" => {
            let m = rt.arg(0);
            let ok = rt.try_lock(m, 1);
            rt.ret(if ok { 0 } else { EBUSY })
        }
        "pthread_mutex_unlock" => {
            let m = rt.arg(0);
            let cur = rt.threads.current;
            let r = match rt.threads.mutexes.get_mut(&m) {
                Some((owner, n)) if *owner == cur => {
                    *n -= 1;
                    if *n == 0 {
                        rt.threads.mutexes.remove(&m);
                        rt.wake_mutex_waiters(m);
                    }
                    0
                }
                // Not locked by this thread (EPERM).
                _ => 1,
            };
            rt.ret(r)
        }
        "pthread_mutex_destroy" => {
            let m = rt.arg(0);
            let held = rt.threads.mutexes.contains_key(&m);
            rt.ret(if held { EBUSY } else { 0 })
        }
        // pthread_cond_wait(cond, mutex), pthread_cond_timedwait(cond, mutex, abstime)
        "pthread_cond_wait" | "pthread_cond_timedwait" => {
            let cur = rt.threads.current;
            if let Some(r) = rt.threads.list[cur].resume.take() {
                // Woken: take the mutex back, then return.
                if rt.try_lock(r.mutex, r.count) {
                    rt.ret(if r.timed_out { ETIMEDOUT } else { 0 })
                } else {
                    rt.threads.list[cur].resume = Some(r);
                    return rt.wait(Wait::Mutex(r.mutex));
                }
            } else {
                let (c, m) = (rt.arg(0), rt.arg(1));
                let deadline = if name == "pthread_cond_timedwait" {
                    // abstime is on CLOCK_REALTIME.
                    let ts = rt.arg(2);
                    let (s, ns) = (rt.mem.r32(ts), rt.mem.r32(ts + 4));
                    let at = Duration::new(s as u64, ns.min(999_999_999));
                    let wall = rt.wall_clock();
                    Some(rt.state_elapsed() + at.saturating_sub(wall))
                } else {
                    None
                };
                if deadline.is_some_and(|d| d <= rt.state_elapsed()) {
                    rt.ret(ETIMEDOUT);
                    return Ok(());
                }
                let count = rt.unlock_all(m);
                rt.threads.list[cur].resume = Some(Resume { mutex: m, count: count.max(1), timed_out: false });
                return rt.wait(Wait::Cond(c, deadline));
            }
        }
        "pthread_cond_signal" | "pthread_cond_broadcast" => {
            let c = rt.arg(0);
            let n = rt.threads.list.len();
            let cur = rt.threads.current;
            // The waiters after this thread first, as they queued.
            for i in (1..=n).map(|k| (cur + k) % n) {
                let t = &mut rt.threads.list[i];
                if matches!(t.state, State::Waiting(Wait::Cond(w, _)) if w == c) {
                    t.state = State::Ready;
                    if name == "pthread_cond_signal" {
                        break;
                    }
                }
            }
            rt.ret(0)
        }
        "pthread_getspecific" => {
            let k = rt.arg(0);
            let v = rt.tls.get(&k).copied().unwrap_or(0);
            rt.ret(v)
        }
        "pthread_setspecific" => {
            let (k, v) = (rt.arg(0), rt.arg(1));
            rt.tls.insert(k, v);
            rt.ret(0)
        }
        // These return first, then the thread gives up the CPU.
        "sched_yield" => {
            rt.ret(0);
            if rt.can_switch() {
                return rt.schedule();
            }
        }
        // The main thread waits for the vertical blank (`wait_vsync`).
        "vsync" => {
            let d = rt.threads.vsync;
            rt.ret(0);
            return rt.wait(Wait::Vsync(d));
        }
        "usleep" | "sleep" | "nanosleep" => {
            let d = match name {
                "usleep" => rt.state_elapsed() + Duration::from_micros(rt.arg(0) as u64),
                "sleep" => rt.state_elapsed() + Duration::from_secs(rt.arg(0) as u64),
                _ => {
                    let ts = rt.arg(0);
                    rt.deadline_after(ts)
                }
            };
            rt.ret(0);
            if rt.can_switch() {
                return rt.wait(Wait::Sleep(d));
            }
        }
        _ => return Err(Stop::Import(name.to_string(), rt.cpu.r[14])),
    }
    Ok(())
}
