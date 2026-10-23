//! The JIT on x86-64 (everything but the stub's API for other targets).

use ribbit_cpu::decode::decode;
use ribbit_cpu::icache::{BlockHead, Blocks, Entry, Hot};
use ribbit_cpu::isa::Instr;
use ribbit_cpu::{BlockRunner, Cpu, MmidMode, RunnerExit, RunnerStop};
use ribbit_memory::Bus;
use std::ffi::c_void;

/// This build can compile (x86-64 and AArch64).
pub const SUPPORTED: bool = cfg!(any(target_arch = "x86_64", target_arch = "aarch64"));

/// The calling convention for JIT helpers.
#[cfg(target_arch = "x86_64")]
macro_rules! jit_abi { () => { "sysv64" } }
#[cfg(not(target_arch = "x86_64"))]
macro_rules! jit_abi { () => { "C" } }

/// A memory read helper: (bus, address) -> value (zero-extended).
#[cfg(target_arch = "x86_64")]
pub type ReadFn = extern "sysv64" fn(*mut c_void, u32) -> u32;
#[cfg(not(target_arch = "x86_64"))]
pub type ReadFn = extern "C" fn(*mut c_void, u32) -> u32;
/// A memory write helper: (bus, address, value; the low bits for 8/16-bit).
#[cfg(target_arch = "x86_64")]
pub type WriteFn = extern "sysv64" fn(*mut c_void, u32, u32);
#[cfg(not(target_arch = "x86_64"))]
pub type WriteFn = extern "C" fn(*mut c_void, u32, u32);
/// Before a memory access: (bus, pc, length | `PREP_FETCH`, cycle).
#[cfg(target_arch = "x86_64")]
pub type PrepFn = extern "sysv64" fn(*mut c_void, u32, u32, u64);
#[cfg(not(target_arch = "x86_64"))]
pub type PrepFn = extern "C" fn(*mut c_void, u32, u32, u64);
/// After a memory access: (bus) -> stall, with bit 63 set if the bus asks
/// for an exit.
#[cfg(target_arch = "x86_64")]
pub type TakeFn = extern "sysv64" fn(*mut c_void) -> u64;
#[cfg(not(target_arch = "x86_64"))]
pub type TakeFn = extern "C" fn(*mut c_void) -> u64;
/// An instruction with no memory access and its fetch timed in full:
/// (bus, pc, length) -> stall.
#[cfg(target_arch = "x86_64")]
pub type FetchTakeFn = extern "sysv64" fn(*mut c_void, u32, u32) -> u64;
#[cfg(not(target_arch = "x86_64"))]
pub type FetchTakeFn = extern "C" fn(*mut c_void, u32, u32) -> u64;
/// The idle-loop detector on a backward jump: (data, target, cycle) -> stop.
#[cfg(target_arch = "x86_64")]
pub type WatchFn = extern "sysv64" fn(*mut c_void, u32, u64) -> u32;
#[cfg(not(target_arch = "x86_64"))]
pub type WatchFn = extern "C" fn(*mut c_void, u32, u64) -> u32;

/// Where one exit of compiled code goes next when the flow leaves it for
/// `pc`: the compiled block there (`code`, its entry after the prologue;
/// 0 = none yet), run without returning if its last instruction is below the
/// fence. Filled by `Jit` when the exit is taken; an exit with a computed
/// target (J [r]) also compares `pc` (one cached target).
#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct Link {
    pub pc: u32,
    pub last: u32,
    pub code: u64,
}

/// What generated code works on. Inputs are set before the call; outputs
/// are written by the code at its exit. `repr(C)`: the code addresses the
/// fields by offset. The helpers use the System V calling convention on
/// every OS (`extern "sysv64"`), so the generated code is the same on
/// Windows and Linux.
#[repr(C)]
pub struct JitCtx {
    /// `Cpu::regs` (r0-r63).
    pub regs: *mut u32,
    /// `Cpu::status32` (Z/N/C/V as the interpreter keeps them).
    pub status32: *mut u32,
    /// The bus the helpers act on (opaque to generated code).
    pub bus: *mut c_void,
    pub read8: ReadFn,
    pub read16: ReadFn,
    pub read32: ReadFn,
    pub write8: WriteFn,
    pub write16: WriteFn,
    pub write32: WriteFn,
    pub prep: PrepFn,
    pub take: TakeFn,
    pub fetch_take: FetchTakeFn,
    /// `Cpu::ready` (64 entries) and `Cpu::ready_max`: the pipeline scoreboard.
    pub ready: *mut u64,
    pub ready_max: *mut u64,
    pub load_latency: u64,
    /// In: the cycle the first instruction starts at; out: after the last.
    pub now: u64,
    /// Stop once `now` reaches this (`run_block`'s budget).
    pub end: u64,
    /// `Cpu::lp_start` / `lp_end`: in, and out (an LP sets them).
    pub lp_start: u32,
    pub lp_end: u32,
    /// Out: the next PC.
    pub pc: u32,
    /// Out: instructions executed (each retired).
    pub executed: u32,
    /// Out: address of the last instruction executed.
    pub from: u32,
    /// Out: nonzero if a taken delayed branch's slot is next
    /// (`Cpu::delay_target` = `delay_target`).
    pub delay_pending: u32,
    pub delay_target: u32,
    /// `run_block`'s fence: no compiled block reaching it is entered.
    pub fence: u32,
    /// `DISPATCH_SIZE` links by address (slot `(pc >> 1) % DISPATCH_SIZE`),
    /// for computed jumps (`Jit` fills them; null: none).
    pub dispatch: *const Link,
    pub watch: WatchFn,
    pub watch_data: *mut c_void,
    /// Out (a `Flow` exit): the link slot of that exit (null: none).
    pub link: *mut Link,
    /// Out (a `Fallback` exit): the block (`BlockHead::id`) and the index
    /// of the instruction left to the interpreter.
    pub fb_block: u32,
    pub fb_index: u32,
    /// Inline memory (`ribbit_memory::JitView`; code compiled `inline`).
    pub timing_on: u32,
    pub armed: *mut bool,
    pub stall: *mut u64,
    pub code_tags: *const u32,
    pub data_tags: *mut u32,
    pub exit_flag: *const bool,
    pub regions: [CtxRegion; 3],
    /// The regions by 64 KB page (`Pages`): `PAGES` host pointers for
    /// reads, then `PAGES` for writes (0: not plain memory there).
    pub pages: *const usize,
    /// The bus's `instruction` (a fetch that missed the code cache).
    pub ifetch: FetchFn,
    /// Accesses outside the regions, or missing the data cache: `at`, then
    /// the access, through the bus. (bus, address, cycle, pc) -> value.
    pub slow_read8: SlowReadFn,
    pub slow_read16: SlowReadFn,
    pub slow_read32: SlowReadFn,
    /// (bus, address, value, cycle, pc).
    pub slow_write8: SlowWriteFn,
    pub slow_write16: SlowWriteFn,
    pub slow_write32: SlowWriteFn,
    /// The slow accesses timed on their own (x86-64 inline code): armed,
    /// `at`, the access, then its stall added to `*acc` (bit 63 set if the
    /// bus asked for an exit). (bus, address, cycle, pc, acc) -> value.
    pub timed_read8: TimedReadFn,
    pub timed_read16: TimedReadFn,
    pub timed_read32: TimedReadFn,
    /// (bus, address, value, cycle, pc, acc).
    pub timed_write8: TimedWriteFn,
    pub timed_write16: TimedWriteFn,
    pub timed_write32: TimedWriteFn,
    /// A cache tag's dirty bit in each of four lanes (the inline data-cache
    /// lookup ignores it).
    pub dirty4: [u32; 4],
    /// Out (a `Flow` exit with `link`): where that exit's direct jump keeps
    /// its 32-bit displacement (pointed at the next block once it is
    /// compiled; 0: the exit has none).
    pub patch: u64,
}

/// A `ribbit_memory::JitRegion` as compiled code reads it: an access at
/// `addr` is inside when `addr - base < limit` (`limit` = its length less 3,
/// so a 4-byte access fits; 0: never).
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct CtxRegion {
    pub base: u32,
    pub limit: u32,
    pub ptr: *mut u8,
    pub writable: u32,
}

impl CtxRegion {
    const NONE: CtxRegion = CtxRegion { base: 0, limit: 0, ptr: std::ptr::null_mut(), writable: 0 };
}

/// Bits of an address below its page (`Pages`).
pub const PAGE_SHIFT: u32 = 16;
/// Pages in the 4 GB address space.
pub const PAGES: usize = 1 << (32 - PAGE_SHIFT);

/// A table with nothing in it (for code run without a view).
static EMPTY_PAGES: EmptyPages = EmptyPages([0; 2 * PAGES]);
#[repr(align(64))]
struct EmptyPages([usize; 2 * PAGES]);

/// The `JitView` regions by page, for compiled code (`JitCtx::pages`): an
/// access whose page lies wholly inside a region is a plain access to that
/// page's host memory, found with one load instead of a search. Pages a
/// region covers only in part (and everything else) stay 0, so those
/// accesses take the slow path, as accesses outside the regions do.
pub struct Pages {
    /// Read pointers, then write pointers (`PAGES` each), as addresses.
    table: Box<[usize]>,
    /// The regions the table was built from (base, length, pointer,
    /// writable).
    built: [(u32, u32, usize, bool); 3],
}

impl Default for Pages {
    fn default() -> Self {
        Pages { table: vec![0; 2 * PAGES].into_boxed_slice(), built: [(0, 0, 0, false); 3] }
    }
}

impl Pages {
    /// Rebuilt when the regions changed since the last time.
    pub fn update(&mut self, regions: &[ribbit_memory::JitRegion; 3]) {
        let now = regions.map(|r| (r.base, r.len, r.ptr as usize, r.writable));
        if now == self.built {
            return;
        }
        for (base, len, _, _) in self.built {
            for page in Self::whole_pages(base, len) {
                self.table[page] = 0;
                self.table[PAGES + page] = 0;
            }
        }
        for (base, len, ptr, writable) in now {
            for page in Self::whole_pages(base, len) {
                let host = ptr + ((page << PAGE_SHIFT) - base as usize);
                self.table[page] = host;
                if writable {
                    self.table[PAGES + page] = host;
                }
            }
        }
        self.built = now;
    }

    /// The pages wholly inside `[base, base + len)`.
    fn whole_pages(base: u32, len: u32) -> std::ops::Range<usize> {
        let size = 1u64 << PAGE_SHIFT;
        let first = (base as u64).div_ceil(size);
        let end = (base as u64 + len as u64) / size;
        first as usize..end.max(first) as usize
    }

    pub fn as_ptr(&self) -> *const usize {
        self.table.as_ptr()
    }
}

/// (bus, pc, length).
#[cfg(target_arch = "x86_64")]
pub type FetchFn = extern "sysv64" fn(*mut c_void, u32, u32);
#[cfg(not(target_arch = "x86_64"))]
pub type FetchFn = extern "C" fn(*mut c_void, u32, u32);
/// (bus, address, cycle, pc) -> value.
#[cfg(target_arch = "x86_64")]
pub type SlowReadFn = extern "sysv64" fn(*mut c_void, u32, u64, u32) -> u32;
#[cfg(not(target_arch = "x86_64"))]
pub type SlowReadFn = extern "C" fn(*mut c_void, u32, u64, u32) -> u32;
/// (bus, address, value, cycle, pc).
#[cfg(target_arch = "x86_64")]
pub type SlowWriteFn = extern "sysv64" fn(*mut c_void, u32, u32, u64, u32);
#[cfg(not(target_arch = "x86_64"))]
pub type SlowWriteFn = extern "C" fn(*mut c_void, u32, u32, u64, u32);
/// (bus, address, cycle, pc, acc) -> value.
#[cfg(target_arch = "x86_64")]
pub type TimedReadFn = extern "sysv64" fn(*mut c_void, u32, u64, u32, *mut u64) -> u32;
#[cfg(not(target_arch = "x86_64"))]
pub type TimedReadFn = extern "C" fn(*mut c_void, u32, u64, u32, *mut u64) -> u32;
/// (bus, address, value, cycle, pc, acc).
#[cfg(target_arch = "x86_64")]
pub type TimedWriteFn = extern "sysv64" fn(*mut c_void, u32, u32, u64, u32, *mut u64);
#[cfg(not(target_arch = "x86_64"))]
pub type TimedWriteFn = extern "C" fn(*mut c_void, u32, u32, u64, u32, *mut u64);

/// The stall of one slow access, armed on its own: `instruction_same_line`
/// arms the timing as `instruction` would (only when it is on), so the
/// access is timed as it would be in the middle of the instruction.
#[inline(always)]
fn timed<B: Bus, T>(b: &mut B, now: u64, pc: u32, acc: *mut u64, access: impl FnOnce(&mut B) -> T) -> T {
    b.instruction_same_line();
    b.at(now, pc);
    let v = access(b);
    let stall = b.take_stall() | (b.block_exit() as u64) << 63;
    // SAFETY: `acc` is the generated code's stack slot, live for the call.
    unsafe { *acc = (*acc + (stall & !(1 << 63))) | (stall & 1 << 63) };
    v
}

macro_rules! jit_fns {
    ($abi:literal) => {
        extern $abi fn ifetch<B: Bus>(bus: *mut c_void, pc: u32, len: u32) {
            // SAFETY: as `read8`.
            unsafe { (*(bus as *mut B)).instruction(pc, len) }
        }
        extern $abi fn slow_read8<B: Bus>(bus: *mut c_void, a: u32, now: u64, pc: u32) -> u32 {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.at(now, pc);
            b.read8(a) as u32
        }
        extern $abi fn slow_read16<B: Bus>(bus: *mut c_void, a: u32, now: u64, pc: u32) -> u32 {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.at(now, pc);
            b.read16(a) as u32
        }
        extern $abi fn slow_read32<B: Bus>(bus: *mut c_void, a: u32, now: u64, pc: u32) -> u32 {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.at(now, pc);
            b.read32(a)
        }
        extern $abi fn slow_write8<B: Bus>(bus: *mut c_void, a: u32, v: u32, now: u64, pc: u32) {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.at(now, pc);
            b.write8(a, v as u8)
        }
        extern $abi fn slow_write16<B: Bus>(bus: *mut c_void, a: u32, v: u32, now: u64, pc: u32) {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.at(now, pc);
            b.write16(a, v as u16)
        }
        extern $abi fn slow_write32<B: Bus>(bus: *mut c_void, a: u32, v: u32, now: u64, pc: u32) {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.at(now, pc);
            b.write32(a, v)
        }
        extern $abi fn timed_read8<B: Bus>(bus: *mut c_void, a: u32, now: u64, pc: u32, acc: *mut u64) -> u32 {
            // SAFETY: as `read8`.
            timed(unsafe { &mut *(bus as *mut B) }, now, pc, acc, |b| b.read8(a) as u32)
        }
        extern $abi fn timed_read16<B: Bus>(bus: *mut c_void, a: u32, now: u64, pc: u32, acc: *mut u64) -> u32 {
            // SAFETY: as `read8`.
            timed(unsafe { &mut *(bus as *mut B) }, now, pc, acc, |b| b.read16(a) as u32)
        }
        extern $abi fn timed_read32<B: Bus>(bus: *mut c_void, a: u32, now: u64, pc: u32, acc: *mut u64) -> u32 {
            // SAFETY: as `read8`.
            timed(unsafe { &mut *(bus as *mut B) }, now, pc, acc, |b| b.read32(a))
        }
        extern $abi fn timed_write8<B: Bus>(bus: *mut c_void, a: u32, v: u32, now: u64, pc: u32, acc: *mut u64) {
            // SAFETY: as `read8`.
            timed(unsafe { &mut *(bus as *mut B) }, now, pc, acc, |b| b.write8(a, v as u8))
        }
        extern $abi fn timed_write16<B: Bus>(bus: *mut c_void, a: u32, v: u32, now: u64, pc: u32, acc: *mut u64) {
            // SAFETY: as `read8`.
            timed(unsafe { &mut *(bus as *mut B) }, now, pc, acc, |b| b.write16(a, v as u16))
        }
        extern $abi fn timed_write32<B: Bus>(bus: *mut c_void, a: u32, v: u32, now: u64, pc: u32, acc: *mut u64) {
            // SAFETY: as `read8`.
            timed(unsafe { &mut *(bus as *mut B) }, now, pc, acc, |b| b.write32(a, v))
        }
        extern $abi fn watch_dyn(data: *mut c_void, pc: u32, now: u64) -> u32 {
            // SAFETY: `data` is the `&mut &mut dyn FnMut` the context was made with.
            let f = unsafe { &mut *(data as *mut &mut dyn FnMut(u32, u64) -> bool) };
            f(pc, now) as u32
        }
        extern $abi fn watch_never(_: *mut c_void, _: u32, _: u64) -> u32 {
            0
        }
        extern $abi fn read8<B: Bus>(bus: *mut c_void, a: u32) -> u32 {
            // SAFETY: `bus` is the `&mut B` the context was made with.
            unsafe { (*(bus as *mut B)).read8(a) as u32 }
        }
        extern $abi fn read16<B: Bus>(bus: *mut c_void, a: u32) -> u32 {
            // SAFETY: as `read8`.
            unsafe { (*(bus as *mut B)).read16(a) as u32 }
        }
        extern $abi fn read32<B: Bus>(bus: *mut c_void, a: u32) -> u32 {
            // SAFETY: as `read8`.
            unsafe { (*(bus as *mut B)).read32(a) }
        }
        extern $abi fn write8<B: Bus>(bus: *mut c_void, a: u32, v: u32) {
            // SAFETY: as `read8`.
            unsafe { (*(bus as *mut B)).write8(a, v as u8) }
        }
        extern $abi fn write16<B: Bus>(bus: *mut c_void, a: u32, v: u32) {
            // SAFETY: as `read8`.
            unsafe { (*(bus as *mut B)).write16(a, v as u16) }
        }
        extern $abi fn write32<B: Bus>(bus: *mut c_void, a: u32, v: u32) {
            // SAFETY: as `read8`.
            unsafe { (*(bus as *mut B)).write32(a, v) }
        }
        extern $abi fn prep<B: Bus>(bus: *mut c_void, pc: u32, len: u32, now: u64) {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            if len & PREP_FETCH != 0 {
                b.instruction(pc, len & 0xFF);
            } else {
                b.instruction_same_line();
            }
            b.at(now, pc);
        }
        extern $abi fn take<B: Bus>(bus: *mut c_void) -> u64 {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            let stall = b.take_stall();
            stall | (b.block_exit() as u64) << 63
        }
        extern $abi fn fetch_take<B: Bus>(bus: *mut c_void, pc: u32, len: u32) -> u64 {
            // SAFETY: as `read8`.
            let b = unsafe { &mut *(bus as *mut B) };
            b.instruction(pc, len);
            b.take_stall()
        }
    };
}
#[cfg(target_arch = "x86_64")]
jit_fns!("sysv64");
#[cfg(not(target_arch = "x86_64"))]
jit_fns!("C");

static NEVER: bool = false;

/// Slots of the computed-jump table (a power of two).
pub const DISPATCH_SIZE: usize = 8192;

/// A table with nothing in it (for code run alone).
static EMPTY_DISPATCH: [Link; DISPATCH_SIZE] = [Link { pc: 0, last: 0, code: 0 }; DISPATCH_SIZE];


/// `prep`'s length flag: time the fetch in full.
const PREP_FETCH: u32 = 0x100;

impl JitCtx {
    /// A context for `cpu` and `bus` starting at cycle `now`, stopping at
    /// `end`; valid while both are borrowed.
    pub fn new<B: Bus>(cpu: &mut Cpu, bus: &mut B, now: u64, end: u64) -> JitCtx {
        assert!(cpu.ready.len() >= 64, "the scoreboard covers r0-r63");
        JitCtx {
            regs: cpu.regs.as_mut_ptr(),
            status32: &mut cpu.status32,
            bus: bus as *mut B as *mut c_void,
            read8: read8::<B>,
            read16: read16::<B>,
            read32: read32::<B>,
            write8: write8::<B>,
            write16: write16::<B>,
            write32: write32::<B>,
            prep: prep::<B>,
            take: take::<B>,
            fetch_take: fetch_take::<B>,
            ready: cpu.ready.as_mut_ptr(),
            ready_max: &mut cpu.ready_max,
            load_latency: cpu.config.load_latency,
            now,
            end,
            lp_start: cpu.lp_start,
            lp_end: cpu.lp_end,
            pc: cpu.pc,
            executed: 0,
            from: cpu.pc,
            delay_pending: 0,
            delay_target: 0,
            fence: u32::MAX,
            dispatch: EMPTY_DISPATCH.as_ptr(),
            watch: watch_never,
            watch_data: std::ptr::null_mut(),
            link: std::ptr::null_mut(),
            fb_block: 0,
            fb_index: 0,
            timing_on: 0,
            armed: std::ptr::null_mut(),
            stall: std::ptr::null_mut(),
            code_tags: std::ptr::null(),
            data_tags: std::ptr::null_mut(),
            exit_flag: &NEVER,
            regions: [CtxRegion::NONE; 3],
            pages: EMPTY_PAGES.0.as_ptr(),
            ifetch: ifetch::<B>,
            slow_read8: slow_read8::<B>,
            slow_read16: slow_read16::<B>,
            slow_read32: slow_read32::<B>,
            slow_write8: slow_write8::<B>,
            slow_write16: slow_write16::<B>,
            slow_write32: slow_write32::<B>,
            timed_read8: timed_read8::<B>,
            timed_read16: timed_read16::<B>,
            timed_read32: timed_read32::<B>,
            timed_write8: timed_write8::<B>,
            timed_write16: timed_write16::<B>,
            timed_write32: timed_write32::<B>,
            dirty4: [1 << 30; 4],
            patch: 0,
        }
    }

    /// Sets the inline-memory fields from the bus's view (code compiled
    /// `inline` needs them).
    fn set_view(&mut self, v: &ribbit_memory::JitView) {
        self.timing_on = v.timing as u32;
        self.armed = v.armed;
        self.stall = v.stall;
        self.code_tags = v.code_tags;
        self.data_tags = v.data_tags;
        self.exit_flag = v.exit;
        for (c, r) in self.regions.iter_mut().zip(&v.regions) {
            *c = CtxRegion { base: r.base, limit: r.len.saturating_sub(3), ptr: r.ptr, writable: r.writable as u32 };
        }
    }
}

/// Why compiled code returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExitKind {
    /// The flow goes on at `pc` (off the code's last instruction, a branch,
    /// a loop end, or a delay slot not run here) and no check stopped it.
    Flow = 0,
    /// The instruction at `pc` is not compiled: the interpreter runs it,
    /// and what follows it in its block (`JitCtx::fb_block`/`fb_index`).
    Fallback = 1,
    /// The last instruction made the bus request an exit.
    Bus = 3,
    /// A backward jump that `watch` accepted.
    Backward = 4,
    /// The budget was reached.
    Budget = 5,
}

/// How a run of compiled code ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub kind: ExitKind,
    pub pc: u32,
    /// Instructions executed.
    pub executed: u32,
    /// A taken delayed branch whose slot was not run: `Cpu::delay_target`.
    pub delay_target: Option<u32>,
    /// The cycle after the last instruction (the pipeline's costs and the
    /// bus's stalls included).
    pub now: u64,
    /// Address of the last instruction executed.
    pub from: u32,
}

/// Compiled code for a straight-line run of guest instructions.
pub struct Block {
    code: *const u8,
    len: usize,
    /// The entry for a jump from another block (after the prologue).
    chain: *const u8,
    /// The mapping when the block has its own (`compile`); None in a `Jit`'s arena.
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    _own: Option<crate::exec_mem::ExecMemory>,
    /// The exits' link slots: held for the code, which addresses them.
    _links: Box<[Link]>,
    /// Guest address of the first instruction.
    pub start: u32,
    /// Instructions compiled (the fallback exit's index, or the run's
    /// length up to and including a branch and its delay slot).
    pub compiled: usize,
    /// Address of the last instruction compiled.
    pub last: u32,
}

// SAFETY: a block's pointers point into code it owns (`_own`) or that its
// `Jit` owns (the arena), which moves with it; the code is never written
// after it is mapped read-execute.
unsafe impl Send for Block {}

impl Block {
    /// Generated machine code (for dumps).
    pub fn code(&self) -> &[u8] {
        // SAFETY: `code` points at `len` bytes of mapped, readable code.
        unsafe { std::slice::from_raw_parts(self.code, self.len) }
    }

    /// Runs the block on `cpu` with `bus` from cycle `cpu.cycles`, with no
    /// budget: registers, flags, LP_COUNT, the scoreboard and memory change
    /// as the interpreter would change them; `cpu.pc` and `cpu.delay_target`
    /// are set from the exit. `cycles`, timers and `instructions_retired`
    /// are left to the caller (`Exit::now`, `Exit::executed`). `cpu.pc`
    /// must be `start` and no delay slot pending.
    pub fn run<B: Bus>(&self, cpu: &mut Cpu, bus: &mut B) -> Exit {
        let now = cpu.cycles;
        self.run_until(cpu, bus, now, u64::MAX)
    }

    /// `run` from cycle `now`, stopping at cycle `end` (`run_block`'s budget).
    pub fn run_until<B: Bus>(&self, cpu: &mut Cpu, bus: &mut B, now: u64, end: u64) -> Exit {
        let mut ctx = JitCtx::new(cpu, bus, now, end);
        self.run_ctx(cpu, &mut ctx)
    }

    /// Runs with a prepared context (`now`, `end` and the rest set).
    fn run_ctx(&self, cpu: &mut Cpu, ctx: &mut JitCtx) -> Exit {
        assert_eq!(cpu.pc, self.start, "block entered at another pc");
        assert!(cpu.delay_target.is_none(), "block entered in a delay slot");
        ctx.pc = cpu.pc;
        ctx.lp_start = cpu.lp_start;
        ctx.lp_end = cpu.lp_end;
        ctx.executed = 0;
        ctx.delay_pending = 0;
        ctx.link = std::ptr::null_mut();
        ctx.patch = 0;
        // SAFETY: the context points at live `cpu` and `bus` for the call;
        // the code only touches what the context names.
        let kind = unsafe { self.enter(ctx) };
        let exit = Exit {
            kind,
            pc: ctx.pc,
            executed: ctx.executed,
            delay_target: (ctx.delay_pending != 0).then_some(ctx.delay_target),
            now: ctx.now,
            from: ctx.from,
        };
        cpu.pc = exit.pc;
        cpu.delay_target = exit.delay_target;
        (cpu.lp_start, cpu.lp_end) = (ctx.lp_start, ctx.lp_end);
        exit
    }

    /// Calls the generated code.
    ///
    /// # Safety
    /// Every pointer in `ctx` must be valid for the call (`JitCtx::new`).
    pub unsafe fn enter(&self, ctx: &mut JitCtx) -> ExitKind {
        #[cfg(target_arch = "x86_64")]
        {
            // SAFETY: the mapping holds code with this signature (x64.rs).
            let f: extern "sysv64" fn(*mut JitCtx) -> u32 = unsafe { std::mem::transmute(self.code) };
            match f(ctx) {
                0 => ExitKind::Flow,
                1 => ExitKind::Fallback,
                3 => ExitKind::Bus,
                4 => ExitKind::Backward,
                _ => ExitKind::Budget,
            }
        }
        #[cfg(target_arch = "aarch64")]
        {
            // SAFETY: the mapping holds code with this signature (aarch64.rs).
            let f: extern "C" fn(*mut JitCtx) -> u32 = unsafe { std::mem::transmute(self.code) };
            match f(ctx) {
                0 => ExitKind::Flow,
                1 => ExitKind::Fallback,
                3 => ExitKind::Bus,
                4 => ExitKind::Backward,
                _ => ExitKind::Budget,
            }
        }
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            let _ = ctx;
            unreachable!("no JIT on this architecture")
        }
    }
}

/// Why nothing was compiled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompileError {
    /// This architecture has no code generator yet.
    Unsupported,
    /// The first instruction cannot be compiled (the interpreter runs it).
    NotCompilable,
    /// The instructions are not one straight run (each must start where the
    /// previous ends).
    NotSequential,
    /// Executable memory could not be mapped.
    Memory(String),
}

/// One instruction to compile: its pre-digested form, whether its fetch is
/// timed in full (`icache::Entry::fetch`; else it is on the 16-byte line
/// the instruction before it ended on), and its full decode (needed for an
/// instruction with no fast form, `Fast::General`).
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub hot: Hot,
    pub fetch: bool,
    pub instr: Option<Instr>,
}

/// Compiles `instrs`, a straight run of decoded instructions (from
/// read-only memory: the code is not watched for changes), each fetch
/// timed in full, MUL64 as on the Leapster (`MmidMode::LowLeapster`).
/// Compilation stops before the first instruction it cannot compile (that
/// becomes a fallback exit) or after the first branch or jump and its delay
/// slot.
pub fn compile(instrs: &[Instr]) -> Result<Block, CompileError> {
    let items: Vec<Item> = instrs.iter().map(|i| Item { hot: Hot::of(i), fetch: true, instr: Some(*i) }).collect();
    compile_items(&items, true)
}

/// `compile` for pre-digested instructions (`mmid_low`: MUL64 leaves MLO
/// alone and puts the low word in MMID), in its own mapping. Its exits are
/// never linked (it runs alone).
pub fn compile_items(items: &[Item], mmid_low: bool) -> Result<Block, CompileError> {
    let mut links = links_for(items);
    let o = Opts { mmid_low, inline: false, latency: ribbit_cpu::CpuConfig::leapster().load_latency, timing: true, fence_check: true };
    let (code, compiled, chain, _) = generate(items, 0, &mut links, &o)?;
    let own = crate::exec_mem::ExecMemory::new(&code).map_err(|e| CompileError::Memory(e.to_string()))?;
    let base = own.as_ptr();
    Ok(Block {
        code: base,
        len: code.len(),
        // SAFETY: within the code just mapped.
        chain: unsafe { base.add(chain) },
        _own: Some(own),
        _links: links,
        start: items[0].hot.addr,
        compiled,
        last: items[compiled - 1].hot.addr,
    })
}

/// Link slots for compiling `items`: enough for every exit that can link.
fn links_for(items: &[Item]) -> Box<[Link]> {
    vec![Link::default(); 3 * items.len() + 4].into_boxed_slice()
}

/// The bytes that point the direct jump of an exit (`JitCtx::patch`) at
/// `target`, or None if it is out of the jump's reach.
fn link_patch(at: u64, target: u64) -> Option<[u8; 4]> {
    #[cfg(target_arch = "x86_64")]
    {
        crate::x64::link_patch(at, target)
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::aarch64::link_patch(at, target)
    }
}

/// What code is generated for: the machine's settings it depends on (the
/// `Jit` drops its code when one of them changes).
pub(crate) struct Opts {
    /// MUL64 leaves MLO alone and puts the low word in MMID.
    pub mmid_low: bool,
    /// Memory and its timing in the code (`ribbit_memory::JitView`), else
    /// every access and timing step through the bus.
    pub inline: bool,
    /// `CpuConfig::load_latency`.
    pub latency: u64,
    /// `JitView::timing` (inline code only).
    pub timing: bool,
    /// Links check the batch fence (it is not `u32::MAX`).
    pub fence_check: bool,
}

/// The code, the instructions it covers, its chain entry's offset and
/// where each kind of code starts (x86-64 only; for profiles).
#[allow(clippy::type_complexity)]
fn generate(items: &[Item], id: u32, links: &mut [Link], o: &Opts) -> Result<(Vec<u8>, usize, usize, Vec<(u32, u8)>), CompileError> {
    if items.windows(2).any(|w| w[1].hot.addr != w[0].hot.next) {
        return Err(CompileError::NotSequential);
    }
    if items.is_empty() {
        return Err(CompileError::NotCompilable);
    }
    #[cfg(target_arch = "x86_64")]
    {
        let x: Vec<crate::x64::Item> =
            items.iter().map(|i| crate::x64::Item { hot: i.hot, fetch: i.fetch, instr: i.instr }).collect();
        let e = crate::x64::emit(&x, id, links.as_mut_ptr(), links.len(), o).ok_or(CompileError::NotCompilable)?;
        if e.compiled == 0 {
            return Err(CompileError::NotCompilable);
        }
        Ok((e.code, e.compiled, e.chain, e.tags))
    }
    #[cfg(target_arch = "aarch64")]
    {
        let x: Vec<crate::aarch64::Item> =
            items.iter().map(|i| crate::aarch64::Item { hot: i.hot, fetch: i.fetch, instr: i.instr }).collect();
        let e = crate::aarch64::emit(&x, id, links.as_mut_ptr(), links.len(), o).ok_or(CompileError::NotCompilable)?;
        if e.compiled == 0 {
            return Err(CompileError::NotCompilable);
        }
        Ok((e.code, e.compiled, e.chain, Vec::new()))
    }
    #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
    {
        let _ = (id, links, o);
        Err(CompileError::Unsupported)
    }
}

/// Research (RIBBIT_JIT_PERFMAP=1, Linux): code is mapped anonymously and each
/// block is listed in /tmp/perf-PID.map (so `perf report` names it), with
/// where each kind of its code starts in /tmp/ribbit-jit-tags-PID.txt.
pub fn perf_map_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var("RIBBIT_JIT_PERFMAP").is_ok_and(|v| v != "0"))
}

fn perf_map(ptr: *const u8, len: usize, start: u32, tags: &[(u32, u8)]) {
    use std::io::Write;
    if !perf_map_on() {
        return;
    }
    #[allow(clippy::type_complexity)]
    static FILES: std::sync::Mutex<Option<(std::fs::File, std::fs::File, std::fs::File, u64)>> = std::sync::Mutex::new(None);
    let mut files = FILES.lock().unwrap_or_else(|e| e.into_inner());
    let (map, tag_file, code_file, at) = files.get_or_insert_with(|| {
        let pid = std::process::id();
        let open = |p: String| std::fs::File::create(p).expect("perf map file");
        (
            open(format!("/tmp/perf-{pid}.map")),
            open(format!("/tmp/ribbit-jit-tags-{pid}.txt")),
            open(format!("/tmp/ribbit-jit-code-{pid}.bin")),
            0,
        )
    });
    let _ = writeln!(map, "{:x} {len:x} jit_{start:08x}", ptr as usize);
    let list: Vec<String> = tags.iter().map(|(o, t)| format!("{o}:{t}")).collect();
    // (Its bytes go to the code file at `at`.)
    let _ = writeln!(tag_file, "{:x} {len} {start:08x} {} {at}", ptr as usize, list.join(","));
    // SAFETY: the block's code, just mapped (readable).
    let _ = code_file.write_all(unsafe { std::slice::from_raw_parts(ptr, len) });
    *at += len as u64;
}

/// A block of the block cache, as the `Jit` knows it.
enum Slot {
    /// Interpreted so far, this many times.
    Counting(u32),
    Compiled(Block),
    /// Its first instruction cannot be compiled.
    Never,
}

/// Runs a block compiled after it has run this many times interpreted.
const TIER: u32 = 32;

/// What the JIT did (speed diagnostics).
#[derive(Debug, Clone, Copy, Default)]
pub struct JitStats {
    pub compiled: u64,
    pub runs: u64,
    pub instructions: u64,
    pub fallbacks: u64,
    pub code_bytes: u64,
    pub checked: u64,
    /// Returns from compiled code by `ExitKind` (flow, fallback, -, bus,
    /// backward, budget).
    pub exits: [u64; 6],
    /// Flow exits continued in the next block, and blocks left to the
    /// interpreter (not compiled yet, or never).
    pub continued: u64,
    pub interpreted: u64,
    pub batches: u64,
    /// Time spent generating code, and putting it in executable memory.
    pub compile_ns: u64,
    pub map_ns: u64,
}

/// Research (RIBBIT_JIT_PROFILE): per address, how often compiled code
/// stopped there for the interpreter (a fallback), and how often a block
/// starting there was interpreted.
#[allow(clippy::type_complexity)]
pub static PROFILE: std::sync::Mutex<Option<[std::collections::HashMap<u32, u64>; 3]>> = std::sync::Mutex::new(None);
/// `PROFILE` is being filled (checked first: no lock otherwise).
pub static PROFILING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// The JIT as a `BlockRunner`: per block of `icache::Blocks` (by id, for
/// one epoch of the cache), interpreted until it has run `TIER` times, then
/// compiled and run compiled. Holds nothing a save state needs; the
/// compiled code is dropped whenever the block cache starts over (ROM
/// ranges, HD capture stops, a full cache).
pub struct Jit {
    epoch: u64,
    slots: Vec<Slot>,
    /// Compiled blocks by address, for computed jumps (`JitCtx::dispatch`).
    dispatch: Box<[Link]>,
    /// The code (dropped after the blocks pointing into it).
    arena: crate::exec_mem::Arena,
    /// Check every compiled run against the interpreter (`RIBBIT_JIT_CHECK`):
    /// panics with both results on any difference.
    pub check: bool,
    pub stats: JitStats,
    /// The bus's plain memory by page (`JitCtx::pages`).
    pages: Pages,
    /// Why code could not be mapped executable, the first time it failed
    /// (security software or a system policy refusing it): everything is
    /// interpreted then, much slower; frontends log it.
    pub memory_error: Option<String>,
    /// `CpuConfig::load_latency` the code was compiled for.
    latency: u64,
    /// Memory timing on (`JitView::timing`) when the code was compiled.
    timing: bool,
    /// The lowest batch fence since the code was compiled: code compiled
    /// while it was `u32::MAX` does not check it.
    fence: u32,
}

impl Default for Jit {
    fn default() -> Self {
        Jit {
            epoch: u64::MAX,
            slots: Vec::new(),
            dispatch: vec![Link::default(); DISPATCH_SIZE].into_boxed_slice(),
            arena: crate::exec_mem::Arena::default(),
            check: false,
            stats: JitStats::default(),
            pages: Pages::default(),
            memory_error: None,
            latency: u64::MAX,
            timing: false,
            fence: u32::MAX,
        }
    }
}

impl Jit {
    /// A JIT, or None where it cannot run (`SUPPORTED`) or when the
    /// environment variable RIBBIT_JIT is 0. RIBBIT_JIT_CHECK=1 turns on
    /// `check`.
    pub fn from_env() -> Option<Jit> {
        if !SUPPORTED || std::env::var("RIBBIT_JIT").is_ok_and(|v| v == "0") {
            return None;
        }
        let check = std::env::var("RIBBIT_JIT_CHECK").is_ok_and(|v| v != "0");
        Some(Jit { check, ..Jit::default() })
    }

    fn start_over(&mut self, epoch: u64) {
        self.slots.clear();
        self.dispatch.fill(Link::default());
        self.arena = crate::exec_mem::Arena::default();
        self.epoch = epoch;
    }

    /// Compiles block `head`: its entries, and its branch's delay slot when
    /// the slot is in read-only code (so a BL.D's link is known).
    fn compile<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, head: &BlockHead, entries: &[Entry]) -> Slot {
        // The full decode of instructions with no fast form.
        let full = |cpu: &mut Cpu, bus: &mut B, h: &Hot| {
            (h.fast == ribbit_cpu::icache::Fast::General).then(|| cpu.icache.get(h.addr, || decode(bus, h.addr)))
        };
        let mut items: Vec<Item> =
            entries.iter().map(|e| Item { hot: e.hot, fetch: e.fetch, instr: full(cpu, bus, &e.hot) }).collect();
        let last = entries[entries.len() - 1].hot;
        if last.delay && last.branchy {
            let at = last.next;
            if let Some(h) = cpu.icache.hot(at, || decode(bus, at)) {
                // Timed as `icache::Blocks::build` would time it after `last`.
                let fetch = (last.next.wrapping_sub(1) >> 4) != at >> 4 || (h.next.wrapping_sub(1) >> 4) != at >> 4;
                let instr = full(cpu, bus, &h);
                items.push(Item { hot: h, fetch, instr });
            }
        }
        let mmid_low = cpu.config.mmid == MmidMode::LowLeapster;
        let mut links = links_for(&items);
        let t0 = std::time::Instant::now();
        // Inline memory, except when checking (the check records every
        // access and timing step through the bus).
        let o = Opts {
            mmid_low,
            inline: !self.check,
            latency: cpu.config.load_latency,
            timing: self.timing,
            fence_check: self.fence != u32::MAX,
        };
        let Ok((code, compiled, chain, tags)) = generate(&items, head.id, &mut links, &o) else {
            return Slot::Never;
        };
        let t1 = std::time::Instant::now();
        let ptr = match self.arena.add(&code) {
            Ok(p) => p,
            Err(e) => {
                self.memory_error.get_or_insert_with(|| e.to_string());
                return Slot::Never;
            }
        };
        perf_map(ptr, code.len(), head.start, &tags);
        self.stats.compile_ns += (t1 - t0).as_nanos() as u64;
        self.stats.map_ns += t1.elapsed().as_nanos() as u64;
        self.stats.compiled += 1;
        self.stats.code_bytes += code.len() as u64;
        Slot::Compiled(Block {
            code: ptr,
            len: code.len(),
            // SAFETY: within the code just added.
            chain: unsafe { ptr.add(chain) },
            _own: None,
            _links: links,
            start: head.start,
            compiled,
            last: items[compiled - 1].hot.addr,
        })
    }

    /// The block for `head`, compiled once it has been seen `TIER` times;
    /// None while it is interpreted (or never compiles).
    fn block<B: Bus>(&mut self, cpu: &mut Cpu, bus: &mut B, blocks: &Blocks, head: &BlockHead) -> Option<&Block> {
        let id = head.id as usize;
        if id >= self.slots.len() {
            self.slots.resize_with(id + 1, || Slot::Counting(0));
        }
        if let Slot::Counting(n) = &mut self.slots[id] {
            *n += 1;
            if *n < TIER {
                return None;
            }
            let entries = &blocks.entries[head.first as usize..(head.first + head.len) as usize];
            self.slots[id] = self.compile(cpu, bus, head, entries);
        }
        match &self.slots[id] {
            Slot::Compiled(b) => Some(b),
            _ => None,
        }
    }
}

impl<B: Bus> BlockRunner<B> for Jit {
    #[allow(clippy::too_many_arguments)]
    fn run(
        &mut self,
        cpu: &mut Cpu,
        bus: &mut B,
        blocks: &mut Blocks,
        head: BlockHead,
        now: u64,
        end: u64,
        fence: u32,
        watch: &mut dyn FnMut(u32, u64) -> bool,
    ) -> Option<RunnerExit> {
        // Without pipeline timing every instruction is one cycle: interpreted.
        if !cpu.config.pipeline_timing {
            return None;
        }
        if head.epoch != self.epoch || cpu.config.load_latency != self.latency {
            self.start_over(head.epoch);
            self.latency = cpu.config.load_latency;
        }
        let mut out = RunnerExit { executed: 0, now, from: cpu.pc, stop: None, resume: None };
        self.stats.batches += 1;
        let mut watch: &mut dyn FnMut(u32, u64) -> bool = watch;
        // Code compiled with inline memory needs the bus's view.
        let view = if self.check { None } else { Some(bus.jit_view()?) };
        // Compiled in: whether memory is timed, and whether links check the
        // fence (code compiled while it was `u32::MAX` would run past a
        // lower one).
        let timing = view.as_ref().is_some_and(|v| v.timing);
        if timing != self.timing || fence < self.fence {
            self.start_over(head.epoch);
        }
        (self.timing, self.fence) = (timing, fence);
        let mut ctx = JitCtx::new(cpu, bus, now, end);
        if let Some(v) = &view {
            ctx.set_view(v);
            self.pages.update(&v.regions);
            ctx.pages = self.pages.as_ptr();
        }
        ctx.fence = fence;
        // (The table is never reallocated: written through this pointer
        // while a block is borrowed.)
        let dispatch = self.dispatch.as_mut_ptr();
        if !self.check {
            ctx.dispatch = dispatch;
        }
        ctx.watch = watch_dyn;
        ctx.watch_data = &mut watch as *mut &mut dyn FnMut(u32, u64) -> bool as *mut c_void;
        let mut head = head;
        // The exit that led here, to link to this block once it is compiled,
        // and its direct jump's displacement (`JitCtx::patch`).
        let mut pending: *mut Link = std::ptr::null_mut();
        let mut pending_jump = 0u64;
        loop {
            let check = self.check;
            let Some(block) = self.block(cpu, bus, blocks, &head) else {
                // Interpreted (for now).
                self.stats.interpreted += 1;
                if PROFILING.load(std::sync::atomic::Ordering::Relaxed)
                    && let Some(p) = PROFILE.lock().unwrap().as_mut()
                {
                    *p[1].entry(head.start).or_default() += 1;
                }
                if out.executed == 0 {
                    return None;
                }
                out.resume = Some((head, 0));
                return Some(out);
            };
            if block.last >= fence {
                if out.executed == 0 {
                    return None;
                }
                out.resume = Some((head, 0));
                return Some(out);
            }
            let mut jump = None;
            if !check {
                let link = Link { pc: head.start, last: block.last, code: block.chain as u64 };
                if !pending.is_null() {
                    // SAFETY: the slot belongs to a block of this epoch (still held).
                    unsafe { *pending = link };
                    // The jump goes straight there from now on (when in reach).
                    if pending_jump != 0 {
                        jump = link_patch(pending_jump, block.chain as u64).map(|b| (pending_jump, b));
                    }
                }
                // SAFETY: within the table (masked index).
                unsafe { *dispatch.add((head.start as usize >> 1) & (DISPATCH_SIZE - 1)) = link };
            }
            let exit = if check {
                let entries = &blocks.entries[head.first as usize..(head.first + head.len) as usize];
                check_run(block, cpu, &mut ctx, bus, entries)
            } else {
                block.run_ctx(cpu, &mut ctx)
            };
            // (Patched now that no code runs; that jump's code is not the
            // block's own.)
            if let Some((at, bytes)) = jump {
                self.arena.patch(at as *const u8, &bytes);
            }
            self.stats.runs += 1;
            self.stats.exits[exit.kind as usize] += 1;
            self.stats.checked += check as u64;
            self.stats.instructions += exit.executed as u64;
            out.executed += exit.executed as u64;
            out.now = exit.now;
            ctx.now = exit.now;
            if exit.executed > 0 {
                out.from = exit.from;
            }
            match exit.kind {
                ExitKind::Bus => out.stop = Some(RunnerStop::Bus),
                ExitKind::Backward => out.stop = Some(RunnerStop::Backward),
                ExitKind::Budget => out.stop = Some(RunnerStop::Budget),
                ExitKind::Fallback => {
                    self.stats.fallbacks += 1;
                    if PROFILING.load(std::sync::atomic::Ordering::Relaxed)
                        && let Some(p) = PROFILE.lock().unwrap().as_mut()
                    {
                        *p[0].entry(cpu.pc).or_default() += 1;
                    }
                    let h = blocks.heads[ctx.fb_block as usize];
                    out.resume = Some((h, ctx.fb_index as usize));
                }
                ExitKind::Flow => {
                    pending = ctx.link;
                    pending_jump = ctx.patch;
                    let pc = cpu.pc;
                    if cpu.delay_target.is_none() && pc < fence {
                        // The next block, as `run_block` would find it.
                        if let Some(next) = blocks.find(&cpu.icache, pc, |a| decode(bus, a))
                            && next.last < fence
                        {
                            if next.epoch != self.epoch {
                                // The cache started over: so does the JIT.
                                self.start_over(next.epoch);
                                pending = std::ptr::null_mut();
                                pending_jump = 0;
                            }
                            head = next;
                            self.stats.continued += 1;
                            continue;
                        }
                    }
                }
            }
            return if out.executed == 0 { None } else { Some(out) };
        }
    }
}

/// What a bus saw, in order (`Jit::check`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Event {
    At {
        cycle: u64,
        pc: u32,
    },
    Read {
        width: u8,
        addr: u32,
        value: u32,
    },
    Write {
        width: u8,
        addr: u32,
        value: u32,
    },
    /// A stall taken, for the instruction at `pc`.
    Take {
        pc: u32,
        stall: u64,
    },
}

/// The real bus, recording what the compiled code does with it.
struct Recorder<'a, B> {
    bus: &'a mut B,
    events: Vec<Event>,
    /// The instruction the timing calls are about.
    pc: u32,
}

impl<B: Bus> Bus for Recorder<'_, B> {
    fn read8(&mut self, a: u32) -> u8 {
        let v = self.bus.read8(a);
        self.events.push(Event::Read { width: 1, addr: a, value: v as u32 });
        v
    }
    fn read16(&mut self, a: u32) -> u16 {
        let v = self.bus.read16(a);
        self.events.push(Event::Read { width: 2, addr: a, value: v as u32 });
        v
    }
    fn read32(&mut self, a: u32) -> u32 {
        let v = self.bus.read32(a);
        self.events.push(Event::Read { width: 4, addr: a, value: v });
        v
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.events.push(Event::Write { width: 1, addr: a, value: v as u32 });
        self.bus.write8(a, v)
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.events.push(Event::Write { width: 2, addr: a, value: v as u32 });
        self.bus.write16(a, v)
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.events.push(Event::Write { width: 4, addr: a, value: v });
        self.bus.write32(a, v)
    }
    fn at(&mut self, cycle: u64, pc: u32) {
        self.pc = pc;
        self.events.push(Event::At { cycle, pc });
        self.bus.at(cycle, pc)
    }
    fn instruction(&mut self, pc: u32, len: u32) {
        self.pc = pc;
        self.bus.instruction(pc, len)
    }
    fn instruction_same_line(&mut self) {
        self.bus.instruction_same_line()
    }
    fn fetch16(&mut self, a: u32) -> u16 {
        self.bus.fetch16(a)
    }
    fn take_stall(&mut self) -> u64 {
        let stall = self.bus.take_stall();
        self.events.push(Event::Take { pc: self.pc, stall });
        stall
    }
    fn block_exit(&self) -> bool {
        self.bus.block_exit()
    }
}

/// Plays a recording back to the interpreter: reads return what the real
/// bus returned, every access and cycle stamp must be the recorded one,
/// stalls are the recorded ones (zero for an instruction the compiled code
/// took none for: one with no memory access, on the line just fetched).
struct Replay<'a, B> {
    bus: &'a mut B,
    events: std::vec::IntoIter<Event>,
    peeked: Option<Event>,
    pc: u32,
    trouble: Option<String>,
}

impl<B: Bus> Replay<'_, B> {
    fn next(&mut self) -> Option<Event> {
        self.peeked.take().or_else(|| self.events.next())
    }
    fn expect(&mut self, want: Event) -> Option<Event> {
        let got = self.next();
        if self.trouble.is_none()
            && !matches!((&got, &want), (Some(Event::Read { width: a, addr: b, .. }), Event::Read { width: c, addr: d, .. }) if a == c && b == d)
            && got.as_ref() != Some(&want)
        {
            self.trouble = Some(format!("the interpreter did {want:?} where the compiled code did {got:?}"));
        }
        got
    }
}

impl<B: Bus> Bus for Replay<'_, B> {
    fn read8(&mut self, a: u32) -> u8 {
        match self.expect(Event::Read { width: 1, addr: a, value: 0 }) {
            Some(Event::Read { value, .. }) => value as u8,
            _ => 0,
        }
    }
    fn read16(&mut self, a: u32) -> u16 {
        match self.expect(Event::Read { width: 2, addr: a, value: 0 }) {
            Some(Event::Read { value, .. }) => value as u16,
            _ => 0,
        }
    }
    fn read32(&mut self, a: u32) -> u32 {
        match self.expect(Event::Read { width: 4, addr: a, value: 0 }) {
            Some(Event::Read { value, .. }) => value,
            _ => 0,
        }
    }
    fn write8(&mut self, a: u32, v: u8) {
        self.expect(Event::Write { width: 1, addr: a, value: v as u32 });
    }
    fn write16(&mut self, a: u32, v: u16) {
        self.expect(Event::Write { width: 2, addr: a, value: v as u32 });
    }
    fn write32(&mut self, a: u32, v: u32) {
        self.expect(Event::Write { width: 4, addr: a, value: v });
    }
    fn at(&mut self, cycle: u64, pc: u32) {
        self.expect(Event::At { cycle, pc });
    }
    fn fetch16(&mut self, a: u32) -> u16 {
        // Decoding (not timed, not recorded): the real code.
        self.bus.fetch16(a)
    }
    fn take_stall(&mut self) -> u64 {
        let next = self.next();
        match next {
            Some(Event::Take { pc, stall }) if pc == self.pc => stall,
            other => {
                self.peeked = other;
                0
            }
        }
    }
}

/// The scoreboard and everything compiled code changes in a `Cpu`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CpuState {
    regs: Vec<u32>,
    status32: u32,
    pc: u32,
    delay_target: Option<u32>,
    ready: Vec<u64>,
    ready_max: u64,
    lp: (u32, u32),
}

impl CpuState {
    fn of(cpu: &Cpu) -> Self {
        CpuState {
            regs: cpu.regs.to_vec(),
            status32: cpu.status32,
            pc: cpu.pc,
            delay_target: cpu.delay_target,
            ready: cpu.ready.clone(),
            ready_max: cpu.ready_max,
            lp: (cpu.lp_start, cpu.lp_end),
        }
    }
    fn restore(&self, cpu: &mut Cpu) {
        cpu.regs.copy_from_slice(&self.regs);
        cpu.status32 = self.status32;
        cpu.pc = self.pc;
        cpu.delay_target = self.delay_target;
        cpu.ready.clone_from(&self.ready);
        cpu.ready_max = self.ready_max;
        (cpu.lp_start, cpu.lp_end) = self.lp;
    }
}

/// Runs `block` with the real bus recorded, then the same instructions in
/// the interpreter from the same state with the recording played back;
/// panics with both results on any difference. Leaves the compiled run's
/// result.
fn check_run<B: Bus>(block: &Block, cpu: &mut Cpu, ctx: &mut JitCtx, bus: &mut B, entries: &[Entry]) -> Exit {
    let before = CpuState::of(cpu);
    let now = ctx.now;
    let mut rec = Recorder { bus, events: Vec::new(), pc: cpu.pc };
    let mut rctx = JitCtx::new(cpu, &mut rec, now, ctx.end);
    rctx.fence = ctx.fence;
    rctx.watch = ctx.watch;
    rctx.watch_data = ctx.watch_data;
    let exit = block.run_ctx(cpu, &mut rctx);
    (ctx.fb_block, ctx.fb_index, ctx.link) = (rctx.fb_block, rctx.fb_index, std::ptr::null_mut());
    let jit = CpuState::of(cpu);
    let Recorder { bus, events, .. } = rec;
    let recorded = events.clone();
    before.restore(cpu);
    let mut replay = Replay { bus, events: events.into_iter(), peeked: None, pc: cpu.pc, trouble: None };
    let mut t = now;
    let mut trace = Vec::new();
    for k in 0..exit.executed as usize {
        // The fetch flag the compiled code used (the delay slot past the
        // block: timed in full, the same when it hits).
        let fetch = entries.get(k).is_none_or(|e| e.fetch);
        replay.pc = cpu.pc;
        trace.push(cpu.pc);
        match cpu.check_one(&mut replay, &mut ribbit_cpu::NoAuxExt, t, fetch) {
            Ok(next) => t = next,
            Err(e) => panic!("JIT check: the interpreter failed at {:#x} in block {:#x}: {e}", cpu.pc, block.start),
        }
    }
    let leftover: Vec<Event> = replay
        .peeked
        .take()
        .into_iter()
        .chain(replay.events.by_ref())
        .filter(|e| !matches!(e, Event::Take { stall: 0, .. }))
        .collect();
    let interp = CpuState::of(cpu);
    let mut trouble = replay.trouble.take();
    if trouble.is_none() && !leftover.is_empty() {
        trouble = Some(format!("the compiled code did more: {leftover:?}"));
    }
    if trouble.is_none() && (interp != jit || t != exit.now) {
        let regs: Vec<String> = (0..64)
            .filter(|&r| interp.regs[r] != jit.regs[r])
            .map(|r| format!("r{r}: interpreter {:#x}, jit {:#x}", interp.regs[r], jit.regs[r]))
            .collect();
        let ready: Vec<String> = (0..64)
            .filter(|&r| interp.ready[r] != jit.ready[r])
            .map(|r| format!("ready[{r}]: interpreter {}, jit {}", interp.ready[r], jit.ready[r]))
            .collect();
        trouble = Some(format!(
            "results differ: cycle interpreter {t}, jit {}; status32 {:#x} / {:#x}; pc {:#x} / {:#x}; delay {:x?} / {:x?}; ready_max {} / {}; {regs:?} {ready:?}",
            exit.now,
            interp.status32,
            jit.status32,
            interp.pc,
            jit.pc,
            interp.delay_target,
            jit.delay_target,
            interp.ready_max,
            jit.ready_max
        ));
    }
    if let Some(t) = trouble {
        let forms: Vec<String> =
            entries.iter().map(|e| format!("  {:08x} {:?} fetch={}", e.hot.addr, e.hot.fast, e.fetch)).collect();
        panic!(
            "JIT check failed in block {:#x} ({} compiled, exit {exit:?}): {t}\nran: {trace:x?}\nblock:\n{}\nrecorded: {recorded:?}",
            block.start,
            block.compiled,
            forms.join("\n")
        );
    }
    jit.restore(cpu);
    exit
}

#[cfg(test)]
mod tests {
    use super::*;
    use ribbit_memory::JitRegion;

    #[test]
    fn pages_cover_only_whole_pages_and_follow_the_regions() {
        let mut ram = vec![0u8; 0x2_0000];
        let mut rom = vec![0u8; 0x1_8000];
        let base = ram.as_mut_ptr() as usize;
        let mut p = Pages::default();
        p.update(&[
            JitRegion { base: 0x3C00_0000, len: 0x2_0000, ptr: ram.as_mut_ptr(), writable: true },
            // 1.5 pages: only the first is plain memory.
            JitRegion { base: 0x4000_0000, len: 0x1_8000, ptr: rom.as_mut_ptr(), writable: false },
            JitRegion::NONE,
        ]);
        let page = |a: u32| (a >> PAGE_SHIFT) as usize;
        assert_eq!(p.table[page(0x3C00_0000)], base);
        assert_eq!(p.table[page(0x3C01_0000)], base + 0x1_0000);
        assert_eq!(p.table[PAGES + page(0x3C01_0000)], base + 0x1_0000, "writable");
        assert_eq!(p.table[page(0x3C02_0000)], 0, "past the end");
        assert_eq!(p.table[page(0x4000_0000)], rom.as_mut_ptr() as usize);
        assert_eq!(p.table[PAGES + page(0x4000_0000)], 0, "read-only");
        assert_eq!(p.table[page(0x4001_0000)], 0, "a page only half inside");
        // A write log makes RAM read-only to the code: rebuilt.
        p.update(&[
            JitRegion { base: 0x3C00_0000, len: 0x2_0000, ptr: ram.as_mut_ptr(), writable: false },
            JitRegion::NONE,
            JitRegion::NONE,
        ]);
        assert_eq!(p.table[PAGES + page(0x3C00_0000)], 0);
        assert_eq!(p.table[page(0x3C00_0000)], base);
        assert_eq!(p.table[page(0x4000_0000)], 0, "the old region is gone");
    }
}
