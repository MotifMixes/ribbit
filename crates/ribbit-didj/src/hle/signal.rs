//! Signals for the HLE, as Linux 2.6.20 delivers them on ARM
//! (arch/arm/kernel/signal.c, kernel/signal.c): handlers per process,
//! masks and pending signals per thread (and per process for kill), the
//! handler run on the thread's stack in the kernel's frame, sigreturn and
//! rt_sigreturn putting the thread back. A thread waiting in a call is
//! woken with EINTR, or runs the call again after the handler when it
//! was asked (SA_RESTART) and the call allows it.
//!
//! NPTL needs it: pthread_cancel sends SIGCANCEL (32) with tgkill and the
//! thread ends from its handler (AppManager's exit waits for that).

use super::task::Wait;
use super::{EFAULT, EINVAL, Process, err};
use ribbit_arm::psr;

pub(super) const EINTR: i32 = 4;
const SA_SIGINFO: u32 = 4;
const SA_RESTORER: u32 = 0x0400_0000;
const SA_RESTART: u32 = 0x1000_0000;
const SA_NODEFER: u32 = 0x4000_0000;
const SA_RESETHAND: u32 = 0x8000_0000;
/// si_code: from kill, and from tkill/tgkill (NPTL's cancel handler
/// takes only these).
pub(super) const SI_USER: i32 = 0;
pub(super) const SI_TKILL: i32 = -6;
/// The kernel's sigreturn code in the vector page (KERN_SIGRETURN_CODE),
/// where a handler returns without SA_RESTORER: sigreturn, and
/// rt_sigreturn 3 words on.
pub(super) const SIGRETURN_CODE: u32 = 0xFFFF_0500;
pub(super) const RT_SIGRETURN_CODE: u32 = SIGRETURN_CODE + 12;
/// struct siginfo, struct ucontext (uc_regspace 8-aligned) and the frames.
const SIGINFO: u32 = 128;
const UCONTEXT: u32 = 744;
const SIGFRAME: u32 = UCONTEXT + 8;
const RT_SIGFRAME: u32 = SIGINFO + SIGFRAME;
/// In the ucontext: uc_stack, uc_mcontext (struct sigcontext), uc_sigmask.
const UC_STACK: u32 = 8;
const UC_MCONTEXT: u32 = 20;
const UC_SIGMASK: u32 = 104;
const SS_DISABLE: u32 = 2;
/// SIGKILL and SIGSTOP can't be blocked or caught.
const UNBLOCKABLE: u64 = bit(9) | bit(19);

const fn bit(sig: u32) -> u64 {
    1 << (sig - 1)
}

/// What rt_sigaction sets (struct sigaction as the kernel takes it):
/// handler 0 default, 1 ignore.
#[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Action {
    pub handler: u32,
    pub flags: u32,
    pub restorer: u32,
    pub mask: u64,
}

/// A signal sent and not delivered yet, with what its siginfo says.
#[derive(Clone, Copy, serde::Serialize, serde::Deserialize)]
pub(super) struct Pending {
    pub sig: u32,
    pub code: i32,
    pub pid: u32,
}

/// A thread's mask and the signals sent to it.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(super) struct Signals {
    pub mask: u64,
    pub pending: Vec<Pending>,
}

impl Signals {
    /// For a new thread: the mask carries over, nothing is pending.
    pub fn inherit(&self) -> Self {
        Signals { mask: self.mask, pending: Vec::new() }
    }
}

/// Whether a signal's default action ends the process; the others are
/// ignored (SIGCHLD, SIGCONT, SIGURG, SIGWINCH; SIGSTOP and the like are
/// not modelled).
pub(super) fn fatal_by_default(sig: u32) -> bool {
    matches!(sig, 1..=3 | 6 | 9 | 11 | 13 | 14 | 15)
}

/// The first of `pending` not in `mask`.
fn unmasked(pending: &[Pending], mask: u64) -> Option<usize> {
    pending.iter().position(|p| bit(p.sig) & mask == 0)
}

/// Puts a signal on a list: the ordinary ones once, real-time ones (32
/// and up) queued.
fn post(list: &mut Vec<Pending>, p: Pending) {
    if p.sig >= 32 || !list.iter().any(|q| q.sig == p.sig) {
        list.push(p);
    }
}

impl Process {
    fn actions_of(&self, tgid: u32) -> Option<&Vec<Action>> {
        if tgid == self.g.tgid { Some(&self.g.actions) } else { self.groups.get(&tgid).map(|g| &g.actions) }
    }

    /// Sends `sig` to process `tgid`, or to its thread `tid` (tkill,
    /// tgkill). Returns false when its default action is to end the
    /// process (the caller does that).
    pub(super) fn send_signal(&mut self, tgid: u32, tid: Option<u32>, sig: u32) -> bool {
        let action = self.actions_of(tgid).and_then(|a| a.get(sig as usize).copied()).unwrap_or_default();
        if action.handler == 0 && fatal_by_default(sig) {
            return false;
        }
        if action.handler <= 1 {
            return true;
        }
        let p = Pending { sig, code: if tid.is_some() { SI_TKILL } else { SI_USER }, pid: self.g.tgid };
        match tid {
            Some(t) if t == self.tid && self.alive => post(&mut self.sig.pending, p),
            Some(t) => {
                if let Some(task) = self.tasks.iter_mut().find(|k| k.tid == t) {
                    post(&mut task.sig.pending, p);
                } else if t == self.tid {
                    post(&mut self.sig.pending, p);
                }
            }
            None if tgid == self.g.tgid => post(&mut self.g.pending, p),
            None => {
                if let Some(g) = self.groups.get_mut(&tgid) {
                    post(&mut g.pending, p);
                }
            }
        }
        true
    }

    /// Whether the running thread may have a signal to take (cheap; the
    /// mask is looked at by `deliver`).
    pub(super) fn signal_waiting(&self) -> bool {
        !self.sig.pending.is_empty() || !self.g.pending.is_empty()
    }

    /// Runs the running thread's first unmasked signal's handler: false if
    /// none can be taken now.
    pub(super) fn deliver(&mut self) -> bool {
        let p = if let Some(i) = unmasked(&self.sig.pending, self.sig.mask) {
            self.sig.pending.remove(i)
        } else if let Some(i) = unmasked(&self.g.pending, self.sig.mask) {
            self.g.pending.remove(i)
        } else {
            return false;
        };
        let action = self.g.actions[p.sig as usize];
        if action.handler <= 1 {
            // Set to default or ignore after it was sent.
            if action.handler == 0 && fatal_by_default(p.sig) {
                self.exit_group(p.sig);
            }
            return true;
        }
        let rt = action.flags & SA_SIGINFO != 0;
        let size = if rt { RT_SIGFRAME } else { SIGFRAME };
        let frame = self.cpu.r[13].wrapping_sub(size) & !7;
        let uc = if rt { frame + SIGINFO } else { frame };
        let mut f = vec![0u8; size as usize];
        let put = |f: &mut [u8], at: u32, v: u32| f[at as usize..at as usize + 4].copy_from_slice(&v.to_le_bytes());
        if rt {
            put(&mut f, 0, p.sig);
            put(&mut f, 8, p.code as u32);
            put(&mut f, 12, p.pid);
        } else {
            // A value sc.trap_no never has.
            put(&mut f, uc - frame, 0x5AC3_C35A);
        }
        let u = uc - frame;
        put(&mut f, u + UC_STACK + 4, SS_DISABLE);
        let mc = u + UC_MCONTEXT;
        put(&mut f, mc + 8, self.sig.mask as u32);
        for r in 0..16 {
            put(&mut f, mc + 12 + 4 * r as u32, self.cpu.r[r]);
        }
        put(&mut f, mc + 76, self.cpu.cpsr);
        put(&mut f, u + UC_SIGMASK, self.sig.mask as u32);
        put(&mut f, u + UC_SIGMASK + 4, (self.sig.mask >> 32) as u32);
        if !self.mem().write_bytes(frame, &f) {
            // No stack to run it on: as the kernel's force_sig(SIGSEGV).
            self.exit_group(11);
            return true;
        }
        let mut cpsr = self.cpu.cpsr & !0xFF00_0000;
        if action.handler & 1 != 0 {
            cpsr |= psr::T;
        } else {
            cpsr &= !psr::T;
        }
        self.cpu.cpsr = cpsr;
        self.cpu.r[0] = p.sig;
        if rt {
            self.cpu.r[1] = frame;
            self.cpu.r[2] = uc;
        }
        self.cpu.r[13] = frame;
        self.cpu.r[14] = if action.flags & SA_RESTORER != 0 {
            action.restorer
        } else if rt {
            RT_SIGRETURN_CODE
        } else {
            SIGRETURN_CODE
        };
        self.cpu.r[15] = action.handler & !1;
        let mut mask = self.sig.mask | action.mask;
        if action.flags & SA_NODEFER == 0 {
            mask |= bit(p.sig);
        }
        self.sig.mask = mask & !UNBLOCKABLE;
        if action.flags & SA_RESETHAND != 0 {
            self.g.actions[p.sig as usize] = Action::default();
        }
        true
    }

    /// sigreturn/rt_sigreturn: the registers and mask from the frame at sp
    /// (what `deliver` wrote); the call's own result is not set.
    pub(super) fn sigreturn(&mut self, rt: bool) -> Result<u32, i32> {
        let sp = self.cpu.r[13];
        let uc = if rt { sp + SIGINFO } else { sp };
        let Some(b) = self.mem().read_bytes(uc, (UC_SIGMASK + 8) as usize) else {
            self.exit_group(11);
            return Ok(0);
        };
        let get = |at: u32| u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().unwrap());
        for r in 0..16 {
            self.cpu.r[r] = get(UC_MCONTEXT + 12 + 4 * r as u32);
        }
        // Only the flags and the instruction set come back, never a mode.
        let keep = 0xFF00_0000 | psr::T;
        self.cpu.cpsr = (self.cpu.cpsr & !keep) | (get(UC_MCONTEXT + 76) & keep);
        self.cpu.r[15] &= if self.cpu.cpsr & psr::T != 0 { !1 } else { !3 };
        self.sig.mask = (get(UC_SIGMASK) as u64 | (get(UC_SIGMASK + 4) as u64) << 32) & !UNBLOCKABLE;
        self.skip_result = true;
        Ok(0)
    }

    /// rt_sigaction (struct sigaction: handler, flags, restorer, mask).
    pub(super) fn sigaction(&mut self, sig: u32, act: u32, old: u32) -> Result<u32, i32> {
        if !(1..=64).contains(&sig) {
            return Err(EINVAL);
        }
        let cur = self.g.actions[sig as usize];
        if old != 0 {
            let mut b = [0u8; 20];
            for (i, v) in [cur.handler, cur.flags, cur.restorer, cur.mask as u32, (cur.mask >> 32) as u32].into_iter().enumerate() {
                b[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
            }
            if !self.mem().write_bytes(old, &b) {
                return Err(EFAULT);
            }
        }
        if act != 0 {
            if bit(sig) & UNBLOCKABLE != 0 {
                return Err(EINVAL);
            }
            let b = self.mem().read_bytes(act, 20).ok_or(EFAULT)?;
            let w = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
            let a = Action { handler: w(0), flags: w(1), restorer: w(2), mask: (w(3) as u64 | (w(4) as u64) << 32) & !UNBLOCKABLE };
            self.g.actions[sig as usize] = a;
            // Ignored from now: what is pending goes (kernel/signal.c).
            if a.handler == 1 || (a.handler == 0 && !fatal_by_default(sig)) {
                self.g.pending.retain(|p| p.sig != sig);
                self.sig.pending.retain(|p| p.sig != sig);
                for t in self.tasks.iter_mut().filter(|t| t.tgid == self.g.tgid) {
                    t.sig.pending.retain(|p| p.sig != sig);
                }
            }
        }
        Ok(0)
    }

    /// rt_sigprocmask: SIG_BLOCK, SIG_UNBLOCK, SIG_SETMASK.
    pub(super) fn sigprocmask(&mut self, how: u32, set: u32, old: u32) -> Result<u32, i32> {
        let cur = self.sig.mask;
        if set != 0 {
            let lo = self.mem().get32(set).ok_or(EFAULT)? as u64;
            let hi = self.mem().get32(set + 4).ok_or(EFAULT)? as u64;
            let s = (lo | hi << 32) & !UNBLOCKABLE;
            self.sig.mask = match how {
                0 => cur | s,
                1 => cur & !s,
                2 => s,
                _ => return Err(EINVAL),
            };
        }
        if old != 0 && !(self.mem().put32(old, cur as u32) && self.mem().put32(old + 4, (cur >> 32) as u32)) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    /// Whether parked task `i` has a signal it can take now. If it waits in
    /// a call, the call is ended with EINTR, or (SA_RESTART, a call that
    /// restarts) left to run again after the handler; true either way.
    pub(super) fn interrupt(&mut self, i: usize) -> bool {
        let t = &self.tasks[i];
        let group = if t.tgid == self.g.tgid { Some(&self.g) } else { self.groups.get(&t.tgid) };
        let Some(group) = group else { return false };
        let sig = match unmasked(&t.sig.pending, t.sig.mask) {
            Some(j) => t.sig.pending[j].sig,
            None => match unmasked(&group.pending, t.sig.mask) {
                Some(j) => group.pending[j].sig,
                None => return false,
            },
        };
        let restart = group.actions[sig as usize].flags & SA_RESTART != 0;
        let tid = t.tid;
        let t = &mut self.tasks[i];
        match t.wait.take() {
            None | Some(Wait::Yield) => {}
            // poll/select are never restarted.
            Some(w) if w.restarts() && (!restart || matches!(w, Wait::Retry(_))) => {
                t.cpu.r[15] += if t.cpu.cpsr & psr::T != 0 { 2 } else { 4 };
                t.cpu.r[0] = err(EINTR);
            }
            Some(w) if w.restarts() => {}
            Some(_) => t.cpu.r[0] = err(EINTR),
        }
        self.poll_until.remove(&tid);
        true
    }
}
