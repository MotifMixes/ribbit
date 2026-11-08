//! Didj Enhanced (new): the Didj's own ARM programs run on Ribbit's ARM
//! core with Ribbit answering their Linux system calls (no bootloader, no
//! kernel). Plan and phase checks: docs/didj-enhanced-hle.md.
//!
//! The programs run on the Didj machine itself (its SDRAM, JIT and device
//! models) with the MMU on and Ribbit's page tables (mem.rs). A system call takes the CPU to the SWI vector at 0xFFFF0008;
//! the run loop sees it there, does the call in Rust and returns to the
//! program. The kernel's user helpers at 0xFFFF0FA0-0xFFFF0FFF (get_tls,
//! cmpxchg, memory barrier) are done the same way.

mod dev;
mod mlc;
pub mod elf;
pub mod mem;
mod state;
mod sysfs;
mod task;

pub use dev::{cart_files, key};
pub mod vfs;

use mem::{Frames, Mem, PAGE, Space, page_up};
use ribbit_arm::{Arm, mode, psr};
use std::collections::{HashMap, HashSet};
use task::{Group, PipeEnd, Task, Wait, Zombie};
use vfs::{Data, S_IFCHR, S_IFDIR, S_IFMT, S_IFREG, Vfs};

/// Where the dynamic loader goes, and mmap's search starts
/// (TASK_UNMAPPED_BASE with a 3 GB user space).
const MMAP_BASE: u32 = 0x4000_0000;
/// The top of the main thread's stack (TASK_SIZE), and how much is mapped.
const STACK_TOP: u32 = 0xBF00_0000;
const STACK_SIZE: u32 = 8 << 20;
const VECTORS: u32 = 0xFFFF_0000;
const SWI_VECTOR: u32 = VECTORS + 8;
const KUSER_MEMORY_BARRIER: u32 = 0xFFFF_0FA0;
const KUSER_CMPXCHG: u32 = 0xFFFF_0FC0;
const KUSER_GET_TLS: u32 = 0xFFFF_0FE0;
/// __kuser_helper_version: 2.6.20 has three helpers.
const KUSER_VERSION: u32 = 3;
/// ARM926EJ-S: swp, half, thumb, fast_mult, edsp, java.
const HWCAP: u32 = 0x197;

const EBADF: i32 = 9;
const EAGAIN: i32 = 11;
const ENOMEM: i32 = 12;
const EFAULT: i32 = 14;
const EEXIST: i32 = 17;
const EISDIR: i32 = 21;
const EINVAL: i32 = 22;
const ENOTTY: i32 = 25;
const ENOSYS: i32 = 38;
const ENOTEMPTY: i32 = 39;
/// openat's "from the current directory".
const AT_FDCWD: u32 = -100i32 as u32;

/// open() flags (ARM).
const O_ACCMODE: u32 = 3;
const O_CREAT: u32 = 0o100;
const O_EXCL: u32 = 0o200;
const O_TRUNC: u32 = 0o1000;
const O_APPEND: u32 = 0o2000;
const O_NONBLOCK: u32 = 0o4000;
const O_DIRECTORY: u32 = 0o40000;
const O_NOFOLLOW: u32 = 0o100000;

/// The root file system out of a folder of Didj firmware packages: a loose
/// erootfs.jffs2 there, else the one in the packages.
pub fn rootfs_from_packages(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let loose = dir.join("erootfs.jffs2");
    if loose.is_file() {
        return std::fs::read(&loose).map_err(|e| format!("{}: {e}", loose.display()));
    }
    for (name, data) in super::package::packages_in(dir)? {
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{name}: {e}"))? {
            if entry.name.rsplit('/').next() == Some("erootfs.jffs2") {
                return ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{name}: {e}"));
            }
        }
    }
    Err("no erootfs.jffs2 in the Didj packages".into())
}

/// How long Original takes from power-on to the first program's first
/// picture (the copyright, 3.72 s), less the HLE's own time to it (0.24 s):
/// the boot loader and kernel, while the boot splash shows.
const BOOT_TIME: std::time::Duration = std::time::Duration::from_millis(3480);

/// The boot splash (bootsplash.rgb in kernel.bin's TFS) out of a folder of
/// Didj firmware packages: a loose kernel.bin there, else the packaged one.
pub fn boot_splash_from_packages(dir: &std::path::Path) -> Result<Vec<u8>, String> {
    let loose = dir.join("kernel.bin");
    let kernel = if loose.is_file() {
        std::fs::read(&loose).map_err(|e| format!("{}: {e}", loose.display()))?
    } else {
        let mut found = None;
        'packages: for (name, data) in super::package::packages_in(dir)? {
            for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{name}: {e}"))? {
                if entry.name.rsplit('/').next() == Some("kernel.bin") {
                    found = Some(ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{name}: {e}"))?);
                    break 'packages;
                }
            }
        }
        found.ok_or("no kernel.bin in the Didj packages")?
    };
    let files = super::tfs::files(&kernel).ok_or("kernel.bin: no TFS summary")?;
    let f = files.iter().find(|f| f.name == "bootsplash.rgb").ok_or("kernel.bin: no bootsplash.rgb")?;
    kernel.get(f.offset..f.offset + 320 * 240 * 3).map(<[u8]>::to_vec).ok_or_else(|| "kernel.bin: bootsplash.rgb is cut short".into())
}

/// The Didj's file systems out of a folder of firmware packages: the root
/// file system, /Didj holding what `nand_from_packages` puts on the Brio
/// volume (packages and installed cartridges), and an empty /tmp.
pub fn system_from_packages(dir: &std::path::Path) -> Result<Vfs, String> {
    let mut vfs = Vfs::from_jffs2(&rootfs_from_packages(dir)?);
    vfs.mkdir_all("/Didj");
    vfs.mkdir_all("/tmp");
    sysfs::add(&mut vfs);
    let mut files = Vec::new();
    for (name, data) in super::package::packages_in(dir)? {
        let mut entries = Vec::new();
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{name}: {e}"))? {
            let bytes = ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{name}: {e}"))?;
            entries.push((entry.name, bytes));
        }
        files.extend(super::brio::package_files(entries));
    }
    files.extend(super::installed_carts(dir)?);
    for (path, data) in files {
        let full = format!("/Didj/{}", path.trim_matches('/'));
        if path.ends_with('/') {
            vfs.mkdir_all(&full);
        } else {
            vfs.add_file(&full, data);
        }
    }
    Ok(vfs)
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
enum Fd {
    /// Standard input/output: the console.
    Console,
    Pipe(PipeEnd),
    /// A POSIX message queue (mqd_t is a file descriptor on Linux).
    Mq {
        #[serde(with = "state::shared")]
        q: std::sync::Arc<std::sync::Mutex<task::Mq>>,
        nonblock: bool,
    },
    File { path: String, pos: u64, append: bool },
    Dir { path: String, pos: usize },
    /// A device node (dev.rs).
    Dev { dev: dev::Device, path: String, nonblock: bool },
}

/// Why `Process::run` returned.
#[derive(Debug, PartialEq, Eq)]
pub enum Stop {
    Exited(i32),
    /// The instruction budget ran out.
    Budget,
    /// Every task waits for something that can't happen now.
    Blocked,
    /// What Linux would kill the program for (bad address, undefined
    /// instruction), described.
    Crashed(String),
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Process {
    /// The running thread's registers.
    pub cpu: Arm,
    /// Saved as its changes (state.rs).
    #[serde(skip)]
    pub vfs: Vfs,
    /// The running thread's process: memory, files, directory.
    g: Group,
    tid: u32,
    tls: u32,
    /// set_tid_address / CLONE_CHILD_CLEARTID: zeroed and woken at exit.
    clear_tid: u32,
    /// The running thread is still there (not exited).
    alive: bool,
    /// What the last system call left the running thread waiting for.
    block: Option<Wait>,
    /// The system call came as EABI `swi 0` (else the old ABI's
    /// `swi 0x900000+n`, which takes 64-bit arguments and structs packed).
    call_eabi: bool,
    /// The call replaced the registers (execve): r0 is left alone.
    skip_result: bool,
    /// The threads not running now, and the other processes.
    tasks: Vec<Task>,
    groups: HashMap<u32, Group>,
    zombies: Vec<Zombie>,
    /// sched_setscheduler's (policy, priority) by thread.
    sched: HashMap<u32, (u32, u32)>,
    /// Message queues by name.
    #[serde(with = "state::shared_map")]
    queues: HashMap<String, std::sync::Arc<std::sync::Mutex<task::Mq>>>,
    next_tid: u32,
    /// rt_sigaction handlers by signal (0 default, 1 ignore).
    handlers: Vec<u32>,
    /// What the programs wrote to the console.
    pub console: Vec<u8>,
    /// Init's (pid 1's) exit code, once it has exited.
    pub exit: Option<i32>,
    /// Instructions run, all tasks.
    pub executed: u64,
    /// System calls not handled yet, logged once each.
    pub unknown: HashSet<u32>,
    logged: HashSet<String>,
    /// Logging every system call (to stderr), for bring-up.
    pub trace: bool,
    #[serde(with = "state::inst")]
    start: std::time::Instant,
    /// CLOCK_REALTIME at `start` (the host's date then): the programs' date
    /// runs on from it with their clock, standing still while paused.
    epoch: std::time::Duration,
    /// Time spent paused (`set_paused`), taken off the host clock: the
    /// programs' clocks stand still meanwhile.
    #[serde(skip)]
    paused: std::time::Duration,
    #[serde(skip)]
    paused_at: Option<std::time::Instant>,
    /// How late the host woke the last time every thread waited, also
    /// taken off the host clock (`on_time`).
    #[serde(skip)]
    late: std::time::Duration,
    /// The programs' clock when `anchor_executed` instructions had run: it
    /// goes on with the instructions, at the processor's clock (`hz`), and
    /// meets the host's whenever every thread waits (`clock`).
    #[serde(with = "state::inst")]
    anchor: std::time::Instant,
    anchor_executed: u64,
    hz: u64,
    /// Every thread waited for something with no deadline (`schedule`).
    #[serde(skip)]
    idle: bool,
    /// Showing the boot splash: nothing runs before then.
    #[serde(with = "state::inst_opt")]
    boot_until: Option<std::time::Instant>,
    /// Processor cycles the 3D chip, DMA and frame timing have had: they
    /// follow the host clock, as the programs' clocks do.
    ticked: u64,
    /// The MLC's frames a second (DIDJ_HLE_MLC_HZ=<n>; 60 is the real one).
    frame_hz: u64,
    dev: dev::Devices,
    /// poll/select deadlines by thread, while they wait.
    #[serde(with = "state::inst_map")]
    poll_until: HashMap<u32, std::time::Instant>,
    /// The machine: SDRAM, the JIT and the device models (its own `cpu`
    /// is not used: the running thread's is `cpu`).
    pub m: crate::machine::Didj,
    frames: Frames,
    /// The thread whose processor last had its TLB flushed for the
    /// current tables.
    tlb_for: u32,
    tlb_generation: u32,
}

/// The JIT stops where Ribbit answers instead of the kernel: system calls,
/// the user helpers and the exception vectors.
fn set_up_jit(m: &mut crate::machine::Didj) {
    let mut stops = [u32::MAX; 8];
    stops[..7].copy_from_slice(&[SWI_VECTOR, KUSER_GET_TLS, KUSER_CMPXCHG, KUSER_MEMORY_BARRIER, VECTORS + 4, VECTORS + 0xC, VECTORS + 0x10]);
    m.jit.set_stops(stops);
    m.jit.enabled &= std::env::var_os("DIDJ_HLE_NOJIT").is_none();
}

fn err(e: i32) -> u32 {
    (-e) as u32
}

impl Process {
    pub fn new(vfs: Vfs) -> Self {
        let mut cpu = Arm::new();
        // MMU on, high vectors (at 0xFFFF0000); domain 0 manager.
        cpu.cp15.control |= 1 | 1 << 13;
        cpu.cp15.dacr = 3;
        let mut m = crate::machine::Didj::with_sdram(crate::nand::Nand::erased(), mem::SDRAM);
        let frames = Frames::new(&mut m.bus, KUSER_VERSION);
        set_up_jit(&mut m);
        // The 3D chip at the real one's speed (DIDJ_HLE_GA3D=<n>: n times as fast). 4 gave
        // JetPack Heroes 60 fps for 25-33, but NeoPets' title fades (a step when 40 ms have
        // passed) then came every 50 ms: its loops were no longer 20 ms. At 1 every cart
        // keeps its pace; smooth motion comes from in-between frames (user, 2026-11-08).
        m.bus.ga3d.speed_up = std::env::var("DIDJ_HLE_GA3D").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
        let start = std::time::Instant::now();
        let mut p = Self {
            cpu,
            vfs,
            g: Group {
                mem: Mem::default(),
                fds: vec![Some(Fd::Console), Some(Fd::Console), Some(Fd::Console)],
                cwd: "/".into(),
                umask: 0o022,
                brk_start: 0,
                brk: 0,
                tgid: 1,
                ppid: 0,
                threads: 1,
            },
            tid: 1,
            tls: 0,
            clear_tid: 0,
            alive: true,
            block: None,
            skip_result: false,
            call_eabi: true,
            tasks: Vec::new(),
            groups: HashMap::new(),
            zombies: Vec::new(),
            queues: HashMap::new(),
            sched: HashMap::new(),
            next_tid: 2,
            handlers: vec![0; 65],
            console: Vec::new(),
            exit: None,
            executed: 0,
            unknown: HashSet::new(),
            logged: HashSet::new(),
            dev: dev::Devices::default(),
            poll_until: HashMap::new(),
            trace: false,
            start,
            epoch: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default(),
            paused: std::time::Duration::ZERO,
            paused_at: None,
            late: std::time::Duration::ZERO,
            anchor: start,
            anchor_executed: 0,
            hz: 0,
            idle: false,
            boot_until: None,
            ticked: 0,
            frame_hz: std::env::var("DIDJ_HLE_MLC_HZ").ok().and_then(|v| v.parse().ok()).unwrap_or(60).max(1),
            m,
            frames,
            tlb_for: 0,
            tlb_generation: 0,
        };
        p.mlc_init();
        p.hz = p.m.hle_cpu_hz();
        p
    }

    /// The running process's memory.
    fn mem(&mut self) -> Space<'_> {
        Space { mem: &mut self.g.mem, bus: &mut self.m.bus, frames: &mut self.frames }
    }

    fn log_once(&mut self, msg: String) {
        if self.logged.insert(msg.clone()) {
            eprintln!("[hle] {msg}");
        }
    }

    fn file(&self, path: &str) -> Result<Vec<u8>, String> {
        let real = self.vfs.resolve(&self.g.cwd, path, true).map_err(|e| format!("{path}: error {e}"))?;
        match self.vfs.get(&real).map(|n| &n.data) {
            Some(Data::File(d)) => Ok(d.clone()),
            _ => Err(format!("{path}: not a file")),
        }
    }

    /// Loads program `path` (and its dynamic loader) and sets up the main
    /// thread's stack as the kernel does, ready to `run`.
    pub fn exec(&mut self, path: &str, argv: &[&str], envp: &[&str]) -> Result<(), String> {
        let data = self.file(path)?;
        let interp = match elf::interp(&data)? {
            Some(path) => Some(self.file(&path)?),
            None => None,
        };
        let mut mem = Space::fresh(&mut self.m.bus, &mut self.frames).ok_or("too many processes")?;
        let mut s = Space { mem: &mut mem, bus: &mut self.m.bus, frames: &mut self.frames };
        let loaded = elf::load(&mut s, &data, 0x8000).and_then(|exe| match &interp {
            Some(ld) => elf::load(&mut s, ld, MMAP_BASE).map(|ld| (ld.entry, ld.bias, exe)),
            None => Ok((exe.entry, 0, exe)),
        });
        let (entry, base, exe) = match loaded {
            Ok(l) => l,
            Err(e) => {
                s.release();
                return Err(e);
            }
        };
        // The new program replaces the process's memory and its other
        // threads.
        let tgid = self.g.tgid;
        self.tasks.retain(|t| t.tgid != tgid);
        self.g.threads = 1;
        self.mem().release();
        self.g.mem = mem;
        self.g.brk_start = exe.end;
        self.g.brk = exe.end;
        self.tls = 0;
        self.clear_tid = 0;

        self.mem().map(STACK_TOP - STACK_SIZE, STACK_SIZE);
        let mut top = STACK_TOP;
        let mut push_str = |mem: &mut Space, s: &str| {
            top -= s.len() as u32 + 1;
            mem.write_bytes(top, s.as_bytes());
            mem.write_bytes(top + s.len() as u32, &[0]);
            top
        };
        let mut s = Space { mem: &mut self.g.mem, bus: &mut self.m.bus, frames: &mut self.frames };
        let platform = push_str(&mut s, "v5l");
        let args: Vec<u32> = argv.iter().map(|a| push_str(&mut s, a)).collect();
        let envs: Vec<u32> = envp.iter().map(|e| push_str(&mut s, e)).collect();
        let auxv: [(u32, u32); 15] = [
            (3, exe.phdr),
            (4, 32),
            (5, exe.phnum),
            (6, PAGE),
            (7, base),
            (8, 0),
            (9, exe.entry),
            (11, 0),
            (12, 0),
            (13, 0),
            (14, 0),
            (16, HWCAP),
            (17, 100),
            (15, platform),
            (0, 0),
        ];
        let mut words = vec![args.len() as u32];
        words.extend(&args);
        words.push(0);
        words.extend(&envs);
        words.push(0);
        for (k, v) in auxv {
            words.extend([k, v]);
        }
        let sp = (top - 4 * words.len() as u32) & !15;
        for (i, w) in words.iter().enumerate() {
            self.mem().put32(sp + 4 * i as u32, *w);
        }

        self.cpu.set_cpsr(mode::USR);
        self.cpu.r = [0; 16];
        self.cpu.r[13] = sp;
        self.cpu.r[15] = entry;
        Ok(())
    }

    /// The thread that ran last.
    /// What every deadline and clock of the programs follows: from the
    /// last time they met the host's clock (`anchor`), the instructions run
    /// at the processor's clock. On the host's clock alone, the same code
    /// took 0.2 ms one frame and 0.4 the next, and a game timing its
    /// frames by gettimeofday saw 39.9 ms for 40 one frame in two and
    /// waited a loop more (NeoPets' title fades at 20 fps, Original 25);
    /// on a Didj, as in Original, the same code takes the same time.
    fn clock(&self) -> std::time::Instant {
        let ran = self.executed - self.anchor_executed;
        self.anchor + std::time::Duration::from_nanos((ran as u128 * 1_000_000_000 / self.hz.max(1) as u128) as u64)
    }

    /// The host clock less the time spent paused and `late`.
    fn host(&self) -> std::time::Instant {
        let now = self.paused_at.unwrap_or_else(std::time::Instant::now);
        now.checked_sub(self.paused + self.late).unwrap_or(self.start)
    }

    /// The programs' clock reads `t` from here on.
    fn set_clock(&mut self, t: std::time::Instant) {
        self.anchor = t;
        self.anchor_executed = self.executed;
    }

    /// Back from sleeping until `u` with every thread waiting: no program
    /// saw the clock meanwhile, so it reads `u` exactly, as on a Didj,
    /// where a sleep ends on the tick. The host wakes late, or its own work
    /// (drawing, system calls) put the programs behind (`late`, taken off
    /// the next sleeps); more than 100 ms (a host slower than a Didj) is
    /// time lost, as a pause is.
    pub(super) fn on_time(&mut self, u: std::time::Instant) {
        if self.paused_at.is_none() {
            let host = std::time::Instant::now().checked_sub(self.paused).unwrap_or(self.start);
            let late = host.saturating_duration_since(u);
            self.late = late.min(std::time::Duration::from_millis(100));
            self.paused += late - self.late;
        }
        self.set_clock(u.max(self.clock()));
    }

    /// Keeps the programs' clock with the host's while they run: waits when
    /// it is ahead (a host faster than a Didj), jumps when it is more than
    /// 20 ms behind (a slower one; programs spinning on gettimeofday then
    /// still keep time).
    fn pace(&mut self) {
        let (clock, host) = (self.clock(), self.host());
        if clock > host + std::time::Duration::from_millis(1) {
            std::thread::sleep(clock - host);
        } else if host > clock + std::time::Duration::from_millis(20) {
            self.set_clock(host);
        }
    }

    fn since_start(&self) -> std::time::Duration {
        self.clock().saturating_duration_since(self.start)
    }

    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Stops or restarts the programs' clocks (the app paused); nothing
    /// should `run` meanwhile.
    pub fn set_paused(&mut self, on: bool) {
        match (on, self.paused_at) {
            (true, None) => self.paused_at = Some(std::time::Instant::now()),
            (false, Some(at)) => {
                self.paused += at.elapsed();
                self.paused_at = None;
            }
            _ => {}
        }
    }

    pub fn tid(&self) -> u32 {
        self.tid
    }

    /// The sound written to /dev/dsp since the last call (16-bit samples,
    /// interleaved), with its rate and channel count.
    pub fn take_sound(&mut self) -> (Vec<i16>, u32, u32) {
        let d = &mut self.dev.dsp;
        (d.out.drain(..).collect(), d.rate, d.channels)
    }

    /// Runs up to `budget` instructions (all tasks together), or until init
    /// (the first program) exits, a program crashes, or nothing can run.
    pub fn run(&mut self, budget: u64) -> Stop {
        if let Some(until) = self.boot_until {
            let now = self.host();
            if now < until {
                std::thread::sleep((until - now).min(std::time::Duration::from_millis(10)));
                return Stop::Budget;
            }
            self.boot_until = None;
            self.on_time(until);
        }
        // Every thread waited with no deadline: the time went by meanwhile.
        if std::mem::take(&mut self.idle) {
            let host = self.host();
            self.set_clock(host.max(self.clock()));
        }
        let mut left = budget;
        let mut slice = 0;
        loop {
            if let Some(code) = self.exit {
                return Stop::Exited(code);
            }
            if !self.alive || self.block.is_some() || slice >= task::SLICE {
                slice = 0;
                self.pace();
                if !self.schedule() {
                    return Stop::Blocked;
                }
                continue;
            }
            if left == 0 {
                return Stop::Budget;
            }
            // Another thread's processor, or changed tables: the TLB and
            // the JIT's tables go.
            if self.g.mem.dirty || self.tlb_for != self.tid || self.cpu.cp15.ttb != self.g.mem.l1 {
                self.cpu.cp15.ttb = self.g.mem.l1;
                self.cpu.cp15.flush_tlb();
                // Counted across all threads' processors, so the JIT
                // never takes another's tables for its own.
                self.tlb_generation += 1;
                self.cpu.cp15.generation = self.tlb_generation;
                self.g.mem.dirty = false;
                self.tlb_for = self.tid;
            }
            let pc = self.cpu.r[15];
            if pc >= VECTORS {
                match pc {
                    SWI_VECTOR => self.system_call(),
                    KUSER_GET_TLS => {
                        self.cpu.r[0] = self.tls;
                        self.return_to_lr();
                    }
                    KUSER_CMPXCHG => self.cmpxchg(),
                    KUSER_MEMORY_BARRIER => self.return_to_lr(),
                    _ => {
                        // The exception's lr: the faulting instruction + 4
                        // (+ 8 for a data abort).
                        let lr = self.cpu.r[14];
                        let thumb = self.cpu.spsr() & psr::T != 0;
                        let (what, at) = match pc - VECTORS {
                            4 => ("undefined instruction".to_string(), lr.wrapping_sub(if thumb { 2 } else { 4 })),
                            0xC => ("prefetch abort".to_string(), lr.wrapping_sub(4)),
                            0x10 => (format!("bad address {:08X}", self.cpu.cp15.far), lr.wrapping_sub(8)),
                            _ => ("jump into the vector page".to_string(), self.cpu.current),
                        };
                        return Stop::Crashed(format!("pid {}: {what} at {at:08X}", self.g.tgid));
                    }
                }
                left -= 1;
                slice += 1;
                self.executed += 1;
                continue;
            }
            let before = self.cpu.cycles;
            let chain = (task::SLICE - slice).min(left.min(u32::MAX as u64) as u32).max(1) as u64;
            self.m.bus.pc = pc;
            self.m.jit.step(&mut self.cpu, &mut self.m.bus, chain, chain);
            let ran = (self.cpu.cycles - before).clamp(1, left);
            self.tick_machine();
            left -= ran;
            slice += ran as u32;
            self.executed += ran;
        }
    }

    /// The 3D chip, DMA and the MLC's frames go on to now (host time; at
    /// the processor's clock), not by instructions run: those come faster
    /// or slower than on a Didj, and none run while every thread sleeps.
    fn tick_machine(&mut self) {
        let hz = self.m.hle_cpu_hz() as u128;
        let due = (self.since_start().as_nanos() * hz / 1_000_000_000) as u64;
        if due > self.ticked {
            self.m.hle_tick(due - self.ticked, self.frame_hz);
            self.ticked = due;
        }
    }

    /// Back from a user helper (`bx lr`).
    fn return_to_lr(&mut self) {
        let lr = self.cpu.r[14];
        if lr & 1 != 0 {
            self.cpu.cpsr |= psr::T;
            self.cpu.r[15] = lr & !1;
        } else {
            self.cpu.cpsr &= !psr::T;
            self.cpu.r[15] = lr & !3;
        }
    }

    /// __kuser_cmpxchg: *r2 = r1 if *r2 == r0; r0 = 0 and C set if it did.
    fn cmpxchg(&mut self) {
        let (old, new, ptr) = (self.cpu.r[0], self.cpu.r[1], self.cpu.r[2]);
        let swapped = self.mem().get32(ptr) == Some(old) && self.mem().put32(ptr, new);
        self.cpu.r[0] = if swapped { 0 } else { 1 };
        if swapped {
            self.cpu.cpsr |= psr::C;
        } else {
            self.cpu.cpsr &= !psr::C;
        }
        self.return_to_lr();
    }

    fn system_call(&mut self) {
        // In SVC mode now: the return address is SVC's lr, the program's
        // CPSR is in SPSR.
        let ret = self.cpu.r[14];
        let user = self.cpu.spsr();
        self.cpu.set_cpsr(user);
        self.cpu.r[15] = ret;
        let n = if user & psr::T != 0 {
            self.cpu.r[7]
        } else {
            let swi = self.mem().get32(ret.wrapping_sub(4)).unwrap_or(0) & 0xFF_FFFF;
            if swi == 0 { self.cpu.r[7] } else { swi.wrapping_sub(0x90_0000) }
        };
        self.call_eabi = user & psr::T != 0 || self.mem().get32(ret.wrapping_sub(4)).unwrap_or(0) & 0xFF_FFFF == 0;
        let a: [u32; 6] = self.cpu.r[..6].try_into().unwrap();
        // The path a call names, for the trace (before the call changes it).
        let path = match n {
            _ if !self.trace => None,
            5 | 10 | 11 | 12 | 33 | 39 | 40 | 85 | 106 | 107 | 195 | 196 | 274 => self.mem().cstr(a[0]),
            322 => self.mem().cstr(a[1]),
            _ => None,
        };
        let result = self.dispatch(n, a);
        if self.trace {
            let note = if self.block.is_some() { " (waits)" } else { "" };
            let path = path.map(|p| format!(" \"{p}\"")).unwrap_or_default();
            eprintln!("[hle {} {:.1}] {}{path} {a:08X?} = {:#X}{note}", self.tid, self.since_start().as_secs_f64() * 1000.0, name(n), result);
        }
        if std::mem::take(&mut self.skip_result) || !self.alive {
            return;
        }
        match &self.block {
            // Run the call again on waking.
            Some(w) if w.restarts() => self.cpu.r[15] = ret.wrapping_sub(if user & psr::T != 0 { 2 } else { 4 }),
            // Its result is set on waking.
            Some(_) => {}
            None => self.cpu.r[0] = result,
        }
    }

    fn path_arg(&mut self, addr: u32) -> Result<String, i32> {
        self.mem().cstr(addr).ok_or(EFAULT)
    }

    fn resolve(&mut self, addr: u32, follow: bool) -> Result<String, i32> {
        let p = self.path_arg(addr)?;
        self.vfs.resolve(&self.g.cwd, &p, follow)
    }

    fn dispatch(&mut self, n: u32, a: [u32; 6]) -> u32 {
        let r = match n {
            1 => {
                self.exit_thread(a[0]);
                Ok(0)
            }
            248 => {
                self.exit_group((a[0] & 0xFF) << 8);
                Ok(0)
            }
            2 | 190 => self.fork(),
            120 => self.clone_task(a[0], a[1], a[2], a[3], a[4]),
            11 => self.execve(a[0], a[1], a[2]),
            7 => self.wait4(a[0], a[1], a[2], 0),
            114 => self.wait4(a[0], a[1], a[2], a[3]),
            37 | 238 => self.kill(a[0], a[1]),
            268 => self.kill(a[1], a[2]),
            42 => self.pipe(a[0]),
            240 => self.futex(a),
            154 => self.sched_set(a[0], None, a[1]),
            155 => {
                let (_, prio) = self.sched_of(a[0]);
                if self.mem().put32(a[1], prio) { Ok(0) } else { Err(EFAULT) }
            }
            156 => self.sched_set(a[0], Some(a[1]), a[2]),
            157 => Ok(self.sched_of(a[0]).0),
            // sched_get_priority_max/min: SCHED_FIFO and SCHED_RR 1-99.
            159 => Ok(if matches!(a[0], 1 | 2) { 99 } else { 0 }),
            160 => Ok(if matches!(a[0], 1 | 2) { 1 } else { 0 }),
            274 => self.mq_open(a[0], a[1], a[3]),
            275 => self.mq_unlink(a[0]),
            276 => self.mq_send(a[0], a[1], a[2], a[3], a[4]),
            277 => self.mq_receive(a[0], a[1], a[2], a[3], a[4]),
            278 => Ok(0), // mq_notify: nothing to notify with yet
            279 => self.mq_getsetattr(a[0], a[1], a[2]),
            3 => self.read(a[0], a[1], a[2]),
            4 => self.write(a[0], a[1], a[2]),
            5 => self.open(a[0], a[1], a[2]),
            6 => self.close(a[0]),
            9 => self.link(a[0], a[1], false),
            10 => self.unlink(a[0]),
            12 => self.chdir(a[0]),
            15 => self.chmod_path(a[0], a[1]),
            94 => self.chmod_fd(a[0], a[1]),
            // chown, lchown, fchown (and 32-bit ones): owners aren't kept.
            16 | 95 | 182 | 198 | 207 | 212 => Ok(0),
            30 => self.utime(a[0], a[1], false),
            269 => self.utime(a[0], a[1], true),
            36 | 118 | 148 => Ok(0), // sync, fsync, fdatasync
            38 => self.rename(a[0], a[1]),
            39 => self.mkdir(a[0], a[1]),
            40 => self.rmdir(a[0]),
            60 => {
                let old = self.g.umask;
                self.g.umask = a[0] & 0o777;
                Ok(old)
            }
            83 => self.link(a[0], a[1], true),
            92 => self.truncate_path(a[0], a[1] as u64),
            93 => self.truncate_fd(a[0], a[1] as u64),
            193 => {
                let len = self.arg64(a, 1);
                self.truncate_path(a[0], len)
            }
            194 => {
                let len = self.arg64(a, 1);
                self.truncate_fd(a[0], len)
            }
            99 => self.statfs(64, a[1]),
            100 => self.fd(a[0]).map(|_| ()).and_then(|_| self.statfs(64, a[1])),
            266 => self.statfs(a[1], a[2]),
            267 => self.fd(a[0]).map(|_| ()).and_then(|_| self.statfs(a[1], a[2])),
            141 => self.getdents(a[0], a[1], a[2], false),
            180 | 181 => {
                let off = self.arg64(a, 3);
                self.pread_write(n == 181, a[0], a[1], a[2], off)
            }
            322 => self.openat(a[0], a[1], a[2], a[3]),
            19 => self.seek(a[0], a[1] as i32 as i64, a[2]),
            20 => Ok(self.g.tgid),
            224 => Ok(self.tid),
            24 | 47 | 49 | 50 | 199..=202 => Ok(0),
            // setuid, setgid, setgroups (and 32-bit ones): all root anyway.
            23 | 46 | 81 | 206 | 213 | 214 => Ok(0),
            33 => self.resolve(a[0], true).and_then(|p| if self.vfs.get(&p).is_some() { Ok(0) } else { Err(vfs::ENOENT) }),
            41 => self.dup(a[0], 0),
            55 => self.fcntl(a[0], a[1], a[2]),
            45 => Ok(self.set_brk(a[0])),
            54 => self.ioctl(a[0], a[1], a[2]),
            63 => self.dup2(a[0], a[1]),
            64 => Ok(self.g.ppid),
            78 => self.gettimeofday(a[0]),
            85 => self.readlink(a[0], a[1], a[2]),
            91 => {
                self.mem().unmap(a[0], a[1]);
                Ok(0)
            }
            122 => self.uname(a[0]),
            125 => Ok(0), // mprotect: no protection kept
            140 => self.llseek(a[0], a[1], a[2], a[3], a[4]),
            145 => self.readv(a[0], a[1], a[2]),
            146 => self.writev(a[0], a[1], a[2]),
            158 => {
                self.block = Some(Wait::Yield);
                Ok(0)
            }
            162 => self.nanosleep(a[0]),
            168 => self.poll(a[0], a[1], a[2] as i32),
            142 => self.select(a),
            163 => self.mremap(a[0], a[1], a[2], a[3]),
            174 => self.sigaction(a[0], a[1], a[2]),
            175 | 186 => Ok(0), // signal masks, alternate stacks: none delivered yet
            183 => self.getcwd(a[0], a[1]),
            191 => self.getrlimit(a[0], a[1]),
            192 => self.mmap2(a),
            106 => self.stat(a[0], a[1], true, false),
            107 => self.stat(a[0], a[1], false, false),
            108 => self.fstat(a[0], a[1], false),
            195 => self.stat(a[0], a[1], true, true),
            196 => self.stat(a[0], a[1], false, true),
            197 => self.fstat(a[0], a[1], true),
            217 => self.getdents(a[0], a[1], a[2], true),
            221 => self.fcntl(a[0], a[1], a[2]),
            256 => {
                self.clear_tid = a[0];
                Ok(self.tid)
            }
            263 => self.clock_gettime(a[0], a[1]),
            0xF_0002 => Ok(0), // cacheflush
            0xF_0005 => {
                self.tls = a[0];
                Ok(0)
            }
            _ => {
                if self.unknown.insert(n) {
                    eprintln!("[hle] system call {n} ({}) not handled, args {a:08X?}", name(n));
                }
                Err(ENOSYS)
            }
        };
        r.unwrap_or_else(err)
    }

    // ---- memory ------------------------------------------------------

    fn set_brk(&mut self, to: u32) -> u32 {
        if to >= self.g.brk_start && to < MMAP_BASE {
            let brk = self.g.brk;
            // Out of memory: the break stays (brk returns the old one).
            if to > brk && !self.mem().map(page_up(brk), page_up(to) - page_up(brk)) {
                return brk;
            }
            self.g.brk = to;
        }
        self.g.brk
    }

    fn mmap2(&mut self, a: [u32; 6]) -> Result<u32, i32> {
        const MAP_FIXED: u32 = 0x10;
        const MAP_ANONYMOUS: u32 = 0x20;
        let (addr, len, flags, fd, pgoff) = (a[0], a[1], a[3], a[4], a[5]);
        if len == 0 {
            return Err(EINVAL);
        }
        let len = page_up(len);
        let at = if flags & MAP_FIXED != 0 {
            if addr % PAGE != 0 {
                return Err(EINVAL);
            }
            self.mem().unmap(addr, len);
            addr
        } else if addr != 0 && addr % PAGE == 0 && self.mem().find_free(addr, len, addr.saturating_add(len)) == Some(addr) {
            addr
        } else {
            self.mem().find_free(MMAP_BASE, len, STACK_TOP - STACK_SIZE).ok_or(ENOMEM)?
        };
        // A frame buffer, the 3D chip's registers or /dev/mem: physical
        // memory (io_remap_pfn_range with the offset as the address).
        if let Some(Fd::Dev { dev: dev::Device::Layer(_) | dev::Device::Ga3d | dev::Device::PhysMem, .. }) =
            self.g.fds.get(fd as usize).and_then(|f| f.as_ref())
            && flags & MAP_ANONYMOUS == 0
        {
            return if self.mem().map_phys(at, len, pgoff << 12) { Ok(at) } else { Err(ENOMEM) };
        }
        let contents = if flags & MAP_ANONYMOUS == 0 {
            let Some(Fd::File { path, .. }) = self.g.fds.get(fd as usize).and_then(|f| f.as_ref()) else { return Err(EBADF) };
            match self.vfs.get(path).map(|n| &n.data) {
                Some(Data::File(d)) => {
                    let from = (pgoff as usize * PAGE as usize).min(d.len());
                    d[from..(from + len as usize).min(d.len())].to_vec()
                }
                _ => return Err(EBADF),
            }
        } else {
            Vec::new()
        };
        if !self.mem().map(at, len) {
            self.mem().unmap(at, len);
            return Err(ENOMEM);
        }
        self.mem().write_bytes(at, &contents);
        Ok(at)
    }

    fn mremap(&mut self, old: u32, old_len: u32, new_len: u32, flags: u32) -> Result<u32, i32> {
        const MREMAP_MAYMOVE: u32 = 1;
        let (old_len, new_len) = (page_up(old_len), page_up(new_len));
        if new_len <= old_len {
            self.mem().unmap(old + new_len, old_len - new_len);
            return Ok(old);
        }
        let grow = new_len - old_len;
        if self.mem().find_free(old + old_len, grow, old + new_len) == Some(old + old_len) {
            self.mem().map(old + old_len, grow);
            return Ok(old);
        }
        if flags & MREMAP_MAYMOVE == 0 {
            return Err(ENOMEM);
        }
        let to = self.mem().find_free(MMAP_BASE, new_len, STACK_TOP - STACK_SIZE).ok_or(ENOMEM)?;
        let data = self.mem().read_bytes(old, old_len as usize).ok_or(EFAULT)?;
        if !self.mem().map(to, new_len) {
            self.mem().unmap(to, new_len);
            return Err(ENOMEM);
        }
        self.mem().write_bytes(to, &data);
        self.mem().unmap(old, old_len);
        Ok(to)
    }

    // ---- files -------------------------------------------------------

    fn fd(&self, fd: u32) -> Result<&Fd, i32> {
        self.g.fds.get(fd as usize).and_then(|f| f.as_ref()).ok_or(EBADF)
    }

    fn new_fd(&mut self, f: Fd, lowest: usize) -> u32 {
        let slot = (lowest..).find(|&i| self.g.fds.get(i).is_none_or(|f| f.is_none())).unwrap();
        if slot >= self.g.fds.len() {
            self.g.fds.resize_with(slot + 1, || None);
        }
        self.g.fds[slot] = Some(f);
        slot as u32
    }

    fn open(&mut self, path: u32, flags: u32, mode: u32) -> Result<u32, i32> {
        let path = self.resolve(path, flags & O_NOFOLLOW == 0)?;
        match self.vfs.get(&path) {
            None if flags & O_CREAT != 0 => {
                let parent = vfs::split(&path).0;
                if !self.vfs.get(parent).is_some_and(|n| n.is_dir()) {
                    return Err(vfs::ENOENT);
                }
                self.vfs.insert(&path, S_IFREG | (mode & 0o7777 & !self.g.umask), Data::File(Vec::new()));
                self.touch(&path);
            }
            None => return Err(vfs::ENOENT),
            Some(_) if flags & (O_CREAT | O_EXCL) == O_CREAT | O_EXCL => return Err(EEXIST),
            Some(_) => {}
        }
        let node = self.vfs.get_mut(&path).unwrap();
        if node.is_dir() {
            if flags & O_ACCMODE != 0 {
                return Err(EISDIR);
            }
            return Ok(self.new_fd(Fd::Dir { path, pos: 0 }, 0));
        }
        if flags & O_DIRECTORY != 0 {
            return Err(vfs::ENOTDIR);
        }
        if let Data::Dev(rdev) = node.data
            && node.mode & S_IFMT == S_IFCHR
        {
            let dev = dev::Device::open(rdev)?;
            return Ok(self.new_fd(Fd::Dev { dev, path, nonblock: flags & O_NONBLOCK != 0 }, 0));
        }
        if flags & O_TRUNC != 0
            && flags & O_ACCMODE != 0
            && let Data::File(d) = &mut node.data
        {
            d.clear();
        }
        Ok(self.new_fd(Fd::File { path, pos: 0, append: flags & O_APPEND != 0 }, 0))
    }

    fn close(&mut self, fd: u32) -> Result<u32, i32> {
        self.fd(fd)?;
        self.g.fds[fd as usize] = None;
        Ok(0)
    }

    fn clone_fd(&self, fd: u32) -> Result<Fd, i32> {
        Ok(self.fd(fd)?.clone())
    }

    fn dup(&mut self, fd: u32, lowest: usize) -> Result<u32, i32> {
        let f = self.clone_fd(fd)?;
        Ok(self.new_fd(f, lowest))
    }

    fn dup2(&mut self, fd: u32, to: u32) -> Result<u32, i32> {
        let f = self.clone_fd(fd)?;
        if to as usize >= self.g.fds.len() {
            self.g.fds.resize_with(to as usize + 1, || None);
        }
        self.g.fds[to as usize] = Some(f);
        Ok(to)
    }

    fn fcntl(&mut self, fd: u32, cmd: u32, arg: u32) -> Result<u32, i32> {
        self.fd(fd)?;
        match cmd {
            0 => self.dup(fd, arg as usize),
            // F_GETFL: O_RDWR, and O_NONBLOCK on a device.
            3 => Ok(match self.fd(fd)? {
                Fd::Dev { nonblock: true, .. } => 2 | O_NONBLOCK,
                _ => 2,
            }),
            4 => {
                // F_SETFL: O_NONBLOCK on a device.
                if let Some(Some(Fd::Dev { nonblock, .. })) = self.g.fds.get_mut(fd as usize) {
                    *nonblock = arg & O_NONBLOCK != 0;
                }
                Ok(0)
            }
            _ => Ok(0),
        }
    }

    fn read_fd(&mut self, fd: u32, len: usize) -> Result<Vec<u8>, i32> {
        let (path, pos) = match self.fd(fd)? {
            Fd::Console => return Ok(Vec::new()),
            &Fd::Dev { dev, nonblock, .. } => return self.dev_read(dev, nonblock, len),
            Fd::Pipe(end) if !end.write => {
                let end = end.clone();
                return self.pipe_read(&end, len);
            }
            Fd::Pipe(_) | Fd::Mq { .. } => return Err(EBADF),
            Fd::Dir { .. } => return Err(EISDIR),
            Fd::File { path, pos, .. } => (path.clone(), *pos),
        };
        let Some(Data::File(d)) = self.vfs.get(&path).map(|n| &n.data) else { return Err(EBADF) };
        let from = (pos as usize).min(d.len());
        let out = d[from..(from + len).min(d.len())].to_vec();
        if let Some(Some(Fd::File { pos, .. })) = self.g.fds.get_mut(fd as usize) {
            *pos += out.len() as u64;
        }
        Ok(out)
    }

    fn write_fd(&mut self, fd: u32, data: &[u8]) -> Result<usize, i32> {
        let (path, pos, append) = match self.fd(fd)? {
            Fd::Console => {
                self.console.extend_from_slice(data);
                return Ok(data.len());
            }
            &Fd::Dev { dev, nonblock, .. } => return self.dev_write(dev, nonblock, data),
            Fd::Pipe(end) if end.write => return Self::pipe_write(end, data),
            Fd::Pipe(_) | Fd::Mq { .. } => return Err(EBADF),
            Fd::Dir { .. } => return Err(EISDIR),
            Fd::File { path, pos, append } => (path.clone(), *pos, *append),
        };
        let Some(Data::File(d)) = self.vfs.get_mut(&path).map(|n| &mut n.data) else { return Err(EBADF) };
        let at = if append { d.len() } else { pos as usize };
        if d.len() < at + data.len() {
            d.resize(at + data.len(), 0);
        }
        d[at..at + data.len()].copy_from_slice(data);
        if let Some(Some(Fd::File { pos, .. })) = self.g.fds.get_mut(fd as usize) {
            *pos = (at + data.len()) as u64;
        }
        self.touch(&path);
        Ok(data.len())
    }

    fn read(&mut self, fd: u32, buf: u32, len: u32) -> Result<u32, i32> {
        let data = self.read_fd(fd, len as usize)?;
        if !self.mem().write_bytes(buf, &data) {
            return Err(EFAULT);
        }
        Ok(data.len() as u32)
    }

    fn write(&mut self, fd: u32, buf: u32, len: u32) -> Result<u32, i32> {
        let data = self.mem().read_bytes(buf, len as usize).ok_or(EFAULT)?;
        Ok(self.write_fd(fd, &data)? as u32)
    }

    fn iovecs(&mut self, iov: u32, count: u32) -> Result<Vec<(u32, u32)>, i32> {
        (0..count)
            .map(|i| {
                let at = iov + 8 * i;
                Ok((self.mem().get32(at).ok_or(EFAULT)?, self.mem().get32(at + 4).ok_or(EFAULT)?))
            })
            .collect()
    }

    fn readv(&mut self, fd: u32, iov: u32, count: u32) -> Result<u32, i32> {
        let mut total = 0;
        for (base, len) in self.iovecs(iov, count)? {
            let n = self.read(fd, base, len)?;
            total += n;
            if n < len {
                break;
            }
        }
        Ok(total)
    }

    fn writev(&mut self, fd: u32, iov: u32, count: u32) -> Result<u32, i32> {
        let mut total = 0;
        for (base, len) in self.iovecs(iov, count)? {
            total += self.write(fd, base, len)?;
        }
        Ok(total)
    }

    fn seek(&mut self, fd: u32, offset: i64, whence: u32) -> Result<u32, i32> {
        let size = match self.fd(fd)? {
            Fd::File { path, .. } => self.vfs.get(path).map_or(0, |n| n.size()) as i64,
            Fd::Dir { .. } => 0,
            Fd::Console | Fd::Pipe(_) | Fd::Mq { .. } | Fd::Dev { .. } => return Err(29), // ESPIPE
        };
        let Some(Some(f)) = self.g.fds.get_mut(fd as usize) else { return Err(EBADF) };
        let cur = match f {
            Fd::File { pos, .. } => *pos as i64,
            Fd::Dir { pos, .. } => *pos as i64,
            Fd::Console | Fd::Pipe(_) | Fd::Mq { .. } | Fd::Dev { .. } => 0,
        };
        let to = match whence {
            0 => offset,
            1 => cur + offset,
            2 => size + offset,
            _ => return Err(EINVAL),
        };
        if to < 0 {
            return Err(EINVAL);
        }
        match f {
            Fd::File { pos, .. } => *pos = to as u64,
            Fd::Dir { pos, .. } => *pos = to as usize,
            Fd::Console | Fd::Pipe(_) | Fd::Mq { .. } | Fd::Dev { .. } => {}
        }
        Ok(to as u32)
    }

    fn llseek(&mut self, fd: u32, hi: u32, lo: u32, result: u32, whence: u32) -> Result<u32, i32> {
        let to = self.seek(fd, ((hi as i64) << 32) | lo as i64, whence)?;
        if !self.mem().write_bytes(result, &(to as u64).to_le_bytes()) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    fn chdir(&mut self, path: u32) -> Result<u32, i32> {
        let p = self.resolve(path, true)?;
        match self.vfs.get(&p) {
            Some(n) if n.is_dir() => {
                self.g.cwd = p;
                Ok(0)
            }
            Some(_) => Err(vfs::ENOTDIR),
            None => Err(vfs::ENOENT),
        }
    }

    fn getcwd(&mut self, buf: u32, size: u32) -> Result<u32, i32> {
        let mut s = self.g.cwd.clone().into_bytes();
        s.push(0);
        if s.len() > size as usize {
            return Err(34); // ERANGE
        }
        if !self.mem().write_bytes(buf, &s) {
            return Err(EFAULT);
        }
        Ok(s.len() as u32)
    }

    fn readlink(&mut self, path: u32, buf: u32, size: u32) -> Result<u32, i32> {
        let p = self.resolve(path, false)?;
        let Some(Data::Link(target)) = self.vfs.get(&p).map(|n| &n.data) else {
            return Err(if self.vfs.get(&p).is_some() { EINVAL } else { vfs::ENOENT });
        };
        let t = target.as_bytes()[..target.len().min(size as usize)].to_vec();
        if !self.mem().write_bytes(buf, &t) {
            return Err(EFAULT);
        }
        Ok(t.len() as u32)
    }

    /// struct stat64 as these programs were built for it (2.6.20's
    /// asm-arm/stat.h: st_blksize, __pad4, a 32-bit st_blocks, then the
    /// times): VERIFIED from busybox ls -l reading st_mtime at 76. EABI
    /// layout, 104 bytes; the old ABI's packs st_size on, 4 bytes earlier.
    fn stat64(&self, ino: u32, mode: u32, size: u64, rdev: u32, mtime: u32) -> Vec<u8> {
        let mut s = vec![0u8; 104];
        let mut put = |at: usize, v: &[u8]| s[at..at + v.len()].copy_from_slice(v);
        put(0, &1u64.to_le_bytes()); // st_dev
        put(12, &ino.to_le_bytes());
        put(16, &mode.to_le_bytes());
        put(20, &1u32.to_le_bytes()); // st_nlink
        put(32, &(rdev as u64).to_le_bytes());
        let o = if self.call_eabi { 0 } else { 4 };
        put(48 - o, &size.to_le_bytes());
        put(56 - o, &4096u32.to_le_bytes());
        put(64 - o, &(size.div_ceil(512) as u32).to_le_bytes());
        for at in [68, 76, 84] {
            put(at - o, &mtime.to_le_bytes());
        }
        put(96 - 2 * o, &(ino as u64).to_le_bytes());
        s.truncate(104 - 2 * o);
        s
    }

    /// The old struct stat (64 bytes; uClibc's dynamic loader uses it).
    fn stat_old(ino: u32, mode: u32, size: u64, rdev: u32, mtime: u32) -> Vec<u8> {
        let mut s = vec![0u8; 64];
        let mut put = |at: usize, v: &[u8]| s[at..at + v.len()].copy_from_slice(v);
        put(0, &1u32.to_le_bytes()); // st_dev
        put(4, &ino.to_le_bytes());
        put(8, &(mode as u16).to_le_bytes());
        put(10, &1u16.to_le_bytes()); // st_nlink
        put(16, &rdev.to_le_bytes());
        put(20, &(size as u32).to_le_bytes());
        put(24, &4096u32.to_le_bytes());
        put(28, &(size.div_ceil(512) as u32).to_le_bytes());
        for at in [32, 40, 48] {
            put(at, &mtime.to_le_bytes());
        }
        s
    }

    fn stat_buf(&self, ino: u32, mode: u32, size: u64, rdev: u32, mtime: u32, wide: bool) -> Vec<u8> {
        if wide { self.stat64(ino, mode, size, rdev, mtime) } else { Self::stat_old(ino, mode, size, rdev, mtime) }
    }

    fn stat_path(&self, path: &str, wide: bool) -> Result<Vec<u8>, i32> {
        let n = self.vfs.get(path).ok_or(vfs::ENOENT)?;
        let rdev = if let Data::Dev(d) = n.data { d } else { 0 };
        Ok(self.stat_buf(n.ino, n.mode, n.size(), rdev, n.mtime, wide))
    }

    fn stat(&mut self, path: u32, buf: u32, follow: bool, wide: bool) -> Result<u32, i32> {
        let p = self.resolve(path, follow)?;
        let s = self.stat_path(&p, wide)?;
        if !self.mem().write_bytes(buf, &s) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    fn fstat(&mut self, fd: u32, buf: u32, wide: bool) -> Result<u32, i32> {
        let s = match self.fd(fd)? {
            // A terminal: character device 4,64 (ttyS0).
            Fd::Console => self.stat_buf(0, 0o020620, 0, (4 << 8) | 64, 0, wide),
            Fd::Pipe(_) | Fd::Mq { .. } => self.stat_buf(0, 0o010600, 0, 0, 0, wide),
            Fd::File { path, .. } | Fd::Dir { path, .. } | Fd::Dev { path, .. } => self.stat_path(&path.clone(), wide)?,
        };
        if !self.mem().write_bytes(buf, &s) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    /// getdents64 (`wide`), or the old getdents: struct linux_dirent with
    /// 32-bit inode and offset and the type in the record's last byte.
    fn getdents(&mut self, fd: u32, buf: u32, size: u32, wide: bool) -> Result<u32, i32> {
        let Fd::Dir { path, pos } = self.fd(fd)? else { return Err(vfs::ENOTDIR) };
        let (path, pos) = (path.clone(), *pos);
        let mut entries: Vec<(String, u32, u8)> = vec![(".".into(), 1, 4), ("..".into(), 1, 4)];
        entries.extend(self.vfs.list(&path).into_iter().map(|(name, n)| {
            let kind = match n.mode & S_IFMT {
                S_IFDIR => 4,
                S_IFREG => 8,
                vfs::S_IFLNK => 10,
                0o020000 => 2,
                0o060000 => 6,
                _ => 0,
            };
            (name, n.ino, kind)
        }));
        let mut out = Vec::new();
        let mut next = pos;
        for (i, (name, ino, kind)) in entries.iter().enumerate().skip(pos) {
            let len = if wide { (19 + name.len() + 1 + 7) & !7 } else { (10 + name.len() + 2 + 3) & !3 };
            if out.len() + len > size as usize {
                if out.is_empty() {
                    return Err(EINVAL);
                }
                break;
            }
            let mut rec = vec![0u8; len];
            if wide {
                rec[..8].copy_from_slice(&(*ino as u64).to_le_bytes());
                rec[8..16].copy_from_slice(&(i as u64 + 1).to_le_bytes());
                rec[16..18].copy_from_slice(&(len as u16).to_le_bytes());
                rec[18] = *kind;
                rec[19..19 + name.len()].copy_from_slice(name.as_bytes());
            } else {
                rec[..4].copy_from_slice(&ino.to_le_bytes());
                rec[4..8].copy_from_slice(&(i as u32 + 1).to_le_bytes());
                rec[8..10].copy_from_slice(&(len as u16).to_le_bytes());
                rec[10..10 + name.len()].copy_from_slice(name.as_bytes());
                rec[len - 1] = *kind;
            }
            out.extend(rec);
            next = i + 1;
        }
        if !self.mem().write_bytes(buf, &out) {
            return Err(EFAULT);
        }
        if let Some(Some(Fd::Dir { pos, .. })) = self.g.fds.get_mut(fd as usize) {
            *pos = next;
        }
        Ok(out.len() as u32)
    }

    fn ioctl(&mut self, fd: u32, cmd: u32, arg: u32) -> Result<u32, i32> {
        const TCGETS: u32 = 0x5401;
        const TIOCGWINSZ: u32 = 0x5413;
        match *self.fd(fd)? {
            Fd::Console => {}
            Fd::Dev { dev, .. } => return self.dev_ioctl(dev, cmd, arg),
            _ => return Err(ENOTTY),
        }
        let reply: Vec<u8> = match cmd {
            TCGETS => {
                // struct termios: c_iflag ICRNL, c_oflag OPOST|ONLCR,
                // c_cflag B115200|CS8|CREAD, c_lflag ISIG|ICANON|ECHO...
                let mut t = Vec::new();
                for v in [0o400u32, 0o5, 0o10262, 0o105073] {
                    t.extend(v.to_le_bytes());
                }
                t.resize(36, 0);
                t
            }
            TIOCGWINSZ => [25u16, 80, 0, 0].iter().flat_map(|v| v.to_le_bytes()).collect(),
            _ => return Ok(0),
        };
        if !self.mem().write_bytes(arg, &reply) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    // ---- changing the file system -------------------------------------

    /// Sets `path`'s modification time to now.
    fn touch(&mut self, path: &str) {
        let now = self.now(0).0;
        if let Some(n) = self.vfs.get_mut(path) {
            n.mtime = now;
        }
    }

    /// A 64-bit argument starting at register `at`: the EABI puts it in an
    /// even/odd pair (skipping a register), the old ABI in the next two.
    fn arg64(&self, a: [u32; 6], at: usize) -> u64 {
        let at = if self.call_eabi { (at + 1) & !1 } else { at };
        a[at] as u64 | (a.get(at + 1).copied().unwrap_or(0) as u64) << 32
    }

    /// The parent of canonical `path` must be an existing directory.
    fn parent_ok(&self, path: &str) -> Result<(), i32> {
        match self.vfs.get(vfs::split(path).0) {
            Some(n) if n.is_dir() => Ok(()),
            Some(_) => Err(vfs::ENOTDIR),
            None => Err(vfs::ENOENT),
        }
    }

    fn unlink(&mut self, path: u32) -> Result<u32, i32> {
        let p = self.resolve(path, false)?;
        match self.vfs.get(&p) {
            None => Err(vfs::ENOENT),
            Some(n) if n.is_dir() => Err(EISDIR),
            Some(_) => {
                self.vfs.remove(&p);
                Ok(0)
            }
        }
    }

    fn mkdir(&mut self, path: u32, mode: u32) -> Result<u32, i32> {
        let p = self.resolve(path, false)?;
        if self.vfs.get(&p).is_some() {
            return Err(EEXIST);
        }
        self.parent_ok(&p)?;
        self.vfs.insert(&p, S_IFDIR | (mode & 0o7777 & !self.g.umask), Data::Dir);
        self.touch(&p);
        Ok(0)
    }

    fn rmdir(&mut self, path: u32) -> Result<u32, i32> {
        let p = self.resolve(path, false)?;
        match self.vfs.get(&p) {
            None => return Err(vfs::ENOENT),
            Some(n) if !n.is_dir() => return Err(vfs::ENOTDIR),
            Some(_) if p == "/" => return Err(16), // EBUSY
            Some(_) => {}
        }
        if !self.vfs.list(&p).is_empty() {
            return Err(ENOTEMPTY);
        }
        self.vfs.remove(&p);
        Ok(0)
    }

    fn rename(&mut self, from: u32, to: u32) -> Result<u32, i32> {
        let (from, to) = (self.resolve(from, false)?, self.resolve(to, false)?);
        let from_dir = self.vfs.get(&from).ok_or(vfs::ENOENT)?.is_dir();
        self.parent_ok(&to)?;
        if from == to {
            return Ok(0);
        }
        if to.starts_with(&format!("{from}/")) {
            return Err(EINVAL);
        }
        match self.vfs.get(&to) {
            Some(n) if n.is_dir() && !from_dir => return Err(EISDIR),
            Some(n) if !n.is_dir() && from_dir => return Err(vfs::ENOTDIR),
            Some(_) if from_dir && !self.vfs.list(&to).is_empty() => return Err(ENOTEMPTY),
            _ => {}
        }
        self.vfs.remove(&to);
        self.vfs.rename(&from, &to);
        let under = format!("{from}/");
        for f in self.g.fds.iter_mut().flatten() {
            if let Fd::File { path, .. } | Fd::Dir { path, .. } = f
                && (*path == from || path.starts_with(&under))
            {
                *path = format!("{to}{}", &path[from.len()..]);
            }
        }
        Ok(0)
    }

    /// link (a copy: hard links aren't kept as such) or symlink.
    fn link(&mut self, old: u32, new: u32, symbolic: bool) -> Result<u32, i32> {
        let new = self.resolve(new, false)?;
        if self.vfs.get(&new).is_some() {
            return Err(EEXIST);
        }
        self.parent_ok(&new)?;
        if symbolic {
            let target = self.path_arg(old)?;
            self.vfs.insert(&new, vfs::S_IFLNK | 0o777, Data::Link(target));
        } else {
            let old = self.resolve(old, false)?;
            let n = self.vfs.get(&old).ok_or(vfs::ENOENT)?;
            let data = match &n.data {
                Data::File(d) => Data::File(d.clone()),
                Data::Link(t) => Data::Link(t.clone()),
                Data::Dev(d) => Data::Dev(*d),
                Data::Dir => return Err(1), // EPERM
            };
            let mode = n.mode;
            self.vfs.insert(&new, mode, data);
        }
        self.touch(&new);
        Ok(0)
    }

    fn set_mode(&mut self, path: &str, mode: u32) -> Result<u32, i32> {
        let n = self.vfs.get_mut(path).ok_or(vfs::ENOENT)?;
        n.mode = (n.mode & S_IFMT) | (mode & 0o7777);
        Ok(0)
    }

    fn chmod_path(&mut self, path: u32, mode: u32) -> Result<u32, i32> {
        let p = self.resolve(path, true)?;
        self.set_mode(&p, mode)
    }

    fn fd_path(&self, fd: u32) -> Result<String, i32> {
        match self.fd(fd)? {
            Fd::File { path, .. } | Fd::Dir { path, .. } | Fd::Dev { path, .. } => Ok(path.clone()),
            Fd::Console | Fd::Pipe(_) | Fd::Mq { .. } => Err(EINVAL),
        }
    }

    fn chmod_fd(&mut self, fd: u32, mode: u32) -> Result<u32, i32> {
        let p = self.fd_path(fd)?;
        self.set_mode(&p, mode)
    }

    /// utime (struct utimbuf) or utimes (two struct timeval): the
    /// modification time, now if `times` is NULL.
    fn utime(&mut self, path: u32, times: u32, timeval: bool) -> Result<u32, i32> {
        let p = self.resolve(path, true)?;
        let mtime = if times == 0 {
            self.now(0).0
        } else {
            self.mem().get32(times + if timeval { 8 } else { 4 }).ok_or(EFAULT)?
        };
        self.vfs.get_mut(&p).ok_or(vfs::ENOENT)?.mtime = mtime;
        Ok(0)
    }

    fn truncate(&mut self, path: &str, len: u64) -> Result<u32, i32> {
        match self.vfs.get_mut(path).map(|n| &mut n.data) {
            Some(Data::File(d)) => d.resize(len as usize, 0),
            Some(Data::Dir) => return Err(EISDIR),
            Some(_) => return Err(EINVAL),
            None => return Err(vfs::ENOENT),
        }
        self.touch(path);
        Ok(0)
    }

    fn truncate_path(&mut self, path: u32, len: u64) -> Result<u32, i32> {
        let p = self.resolve(path, true)?;
        self.truncate(&p, len)
    }

    fn truncate_fd(&mut self, fd: u32, len: u64) -> Result<u32, i32> {
        let p = self.fd_path(fd)?;
        self.truncate(&p, len)
    }

    /// pread64/pwrite64: at `offset`, the file position left alone.
    fn pread_write(&mut self, write: bool, fd: u32, buf: u32, len: u32, offset: u64) -> Result<u32, i32> {
        let Fd::File { pos, append, .. } = self.fd(fd)? else { return Err(29) }; // ESPIPE
        let (saved, append) = (*pos, *append);
        let set = |p: &mut Self, to: u64, app: bool| {
            if let Some(Some(Fd::File { pos, append, .. })) = p.g.fds.get_mut(fd as usize) {
                (*pos, *append) = (to, app);
            }
        };
        set(self, offset, false);
        let r = if write { self.write(fd, buf, len) } else { self.read(fd, buf, len) };
        set(self, saved, append);
        r
    }

    /// struct statfs64 (84 bytes; statfs's own is its first 64), cut to
    /// `size`: one roomy file system for everything.
    fn statfs(&mut self, size: u32, buf: u32) -> Result<u32, i32> {
        let mut s = Vec::new();
        s.extend(0x4D44u32.to_le_bytes()); // f_type: MSDOS_SUPER_MAGIC
        s.extend(4096u32.to_le_bytes()); // f_bsize
        for v in [65536u64, 32768, 32768, 65536, 32768] {
            s.extend(v.to_le_bytes()); // blocks, bfree, bavail, files, ffree
        }
        s.extend([0u8; 8]); // f_fsid
        s.extend(255u32.to_le_bytes()); // f_namelen
        s.extend(4096u32.to_le_bytes()); // f_frsize
        s.resize(84, 0);
        s.truncate(size as usize);
        if !self.mem().write_bytes(buf, &s) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    fn openat(&mut self, dirfd: u32, path: u32, flags: u32, mode: u32) -> Result<u32, i32> {
        let p = self.path_arg(path)?;
        if dirfd == AT_FDCWD || p.starts_with('/') {
            return self.open(path, flags, mode);
        }
        let Fd::Dir { path: dir, .. } = self.fd(dirfd)? else { return Err(vfs::ENOTDIR) };
        let dir = dir.clone();
        let saved = std::mem::replace(&mut self.g.cwd, dir);
        let r = self.open(path, flags, mode);
        self.g.cwd = saved;
        r
    }

    // ---- the rest ----------------------------------------------------

    /// A thread's (policy, priority); pid 0 is the caller.
    fn sched_of(&self, pid: u32) -> (u32, u32) {
        let tid = if pid == 0 { self.tid } else { pid };
        self.sched.get(&tid).copied().unwrap_or((0, 0))
    }

    /// sched_setscheduler / sched_setparam. Kept and reported back; the
    /// scheduler doesn't use priorities yet (round robin).
    fn sched_set(&mut self, pid: u32, policy: Option<u32>, param: u32) -> Result<u32, i32> {
        let prio = self.mem().get32(param).ok_or(EFAULT)?;
        let tid = if pid == 0 { self.tid } else { pid };
        let policy = policy.unwrap_or(self.sched_of(tid).0);
        if policy > 2 || (policy == 0) != (prio == 0) || prio > 99 {
            return Err(EINVAL);
        }
        self.sched.insert(tid, (policy, prio));
        Ok(0)
    }

    /// rt_sigaction: handlers are kept (not called yet; see `kill`).
    fn sigaction(&mut self, sig: u32, act: u32, old: u32) -> Result<u32, i32> {
        let Some(&cur) = self.handlers.get(sig as usize) else { return Err(EINVAL) };
        if old != 0 {
            self.mem().write_bytes(old, &[0; 20]);
            self.mem().put32(old, cur);
        }
        if act != 0 {
            self.handlers[sig as usize] = self.mem().get32(act).ok_or(EFAULT)?;
        }
        Ok(0)
    }

    fn uname(&mut self, buf: u32) -> Result<u32, i32> {
        let mut u = vec![0u8; 6 * 65];
        for (i, s) in ["Linux", "didj", "2.6.20-lf1000", "#1 PREEMPT", "armv5tejl", "(none)"].iter().enumerate() {
            u[i * 65..i * 65 + s.len()].copy_from_slice(s.as_bytes());
        }
        if !self.mem().write_bytes(buf, &u) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    /// CLOCK_REALTIME.
    fn realtime(&self) -> std::time::Duration {
        self.epoch + self.since_start()
    }

    fn now(&self, clock: u32) -> (u32, u32) {
        let d = if clock == 0 { self.realtime() } else { self.since_start() };
        (d.as_secs() as u32, d.subsec_nanos())
    }

    fn gettimeofday(&mut self, tv: u32) -> Result<u32, i32> {
        let (s, ns) = self.now(0);
        if tv != 0 && !(self.mem().put32(tv, s) && self.mem().put32(tv + 4, ns / 1000)) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    fn clock_gettime(&mut self, clock: u32, ts: u32) -> Result<u32, i32> {
        let (s, ns) = self.now(clock);
        if !(self.mem().put32(ts, s) && self.mem().put32(ts + 4, ns)) {
            return Err(EFAULT);
        }
        Ok(0)
    }

    fn getrlimit(&mut self, resource: u32, buf: u32) -> Result<u32, i32> {
        const RLIMIT_STACK: u32 = 3;
        let v = if resource == RLIMIT_STACK { STACK_SIZE } else { u32::MAX };
        if !(self.mem().put32(buf, v) && self.mem().put32(buf + 4, u32::MAX)) {
            return Err(EFAULT);
        }
        Ok(0)
    }
}

/// A system call's name, for logs.
fn name(n: u32) -> &'static str {
    match n {
        1 => "exit",
        3 => "read",
        4 => "write",
        5 => "open",
        6 => "close",
        7 => "waitpid",
        9 => "link",
        10 => "unlink",
        11 => "execve",
        12 => "chdir",
        15 => "chmod",
        19 => "lseek",
        20 => "getpid",
        30 => "utime",
        33 => "access",
        37 => "kill",
        38 => "rename",
        39 => "mkdir",
        40 => "rmdir",
        42 => "pipe",
        45 => "brk",
        54 => "ioctl",
        55 => "fcntl",
        60 => "umask",
        63 => "dup2",
        78 => "gettimeofday",
        83 => "symlink",
        85 => "readlink",
        91 => "munmap",
        92 => "truncate",
        93 => "ftruncate",
        94 => "fchmod",
        99 => "statfs",
        100 => "fstatfs",
        106 => "stat",
        107 => "lstat",
        108 => "fstat",
        114 => "wait4",
        118 => "fsync",
        120 => "clone",
        122 => "uname",
        125 => "mprotect",
        140 => "_llseek",
        141 => "getdents",
        142 => "_newselect",
        145 => "readv",
        146 => "writev",
        154 => "sched_setparam",
        155 => "sched_getparam",
        156 => "sched_setscheduler",
        157 => "sched_getscheduler",
        158 => "sched_yield",
        159 => "sched_get_priority_max",
        160 => "sched_get_priority_min",
        162 => "nanosleep",
        163 => "mremap",
        168 => "poll",
        174 => "rt_sigaction",
        175 => "rt_sigprocmask",
        180 => "pread64",
        181 => "pwrite64",
        183 => "getcwd",
        190 => "vfork",
        191 => "ugetrlimit",
        192 => "mmap2",
        193 => "truncate64",
        194 => "ftruncate64",
        195 => "stat64",
        196 => "lstat64",
        197 => "fstat64",
        217 => "getdents64",
        221 => "fcntl64",
        224 => "gettid",
        240 => "futex",
        248 => "exit_group",
        256 => "set_tid_address",
        263 => "clock_gettime",
        274 => "mq_open",
        275 => "mq_unlink",
        276 => "mq_timedsend",
        277 => "mq_timedreceive",
        278 => "mq_notify",
        279 => "mq_getsetattr",
        0xF_0002 => "cacheflush",
        0xF_0005 => "set_tls",
        _ => "?",
    }
}
