//! Just-in-time compiler for the ARM926EJ-S in `ribbit-arm`
//! (docs/arm-jit-design.md).
//!
//! STATUS: phase 2 in progress. ARM code compiles to x86-64 once a block
//! has started twice: most instructions natively, the rest by calling the
//! interpreter for that one instruction (`Arm::execute_arm`) from inside
//! the block (docs/arm-jit-design.md lists which). Loads and stores reach
//! RAM directly through per-page tables. Thumb and pending interrupts go
//! to `Arm::step`. Other hosts compile nothing (`SUPPORTED` is false).
//!
//! Guest state is never copied: generated code reads and writes `Arm::r`,
//! `cpsr` and the counters in place, and loads and stores that miss the
//! tables go through `Arm::load32` etc. (the MMU and the bus, as the
//! interpreter's), so save states, the debugger and the interpreter always
//! see one state.
//!
//! Blocks are keyed by (virtual PC, physical PC) and never cross a 1 KB
//! page. The bus reports writes into pages holding code (`CodeWatch`), and
//! their blocks are dropped before the next block runs; a block that writes
//! compiled code stops right after that store.

mod decode;
#[cfg(target_arch = "x86_64")]
mod x64;

pub use decode::CLASSES;
use ribbit_arm::{Arm, ArmBus, CodeWatch, mode, psr};
use std::collections::HashMap;
use std::hash::{BuildHasherDefault, Hasher};

/// Whether this host compiles anything.
pub const SUPPORTED: bool = cfg!(target_arch = "x86_64");

/// Most instructions in a block.
const MAX_BLOCK: u32 = 32;
/// Arena size at which everything compiled is dropped and compiled again.
const ARENA_LIMIT: usize = 64 << 20;

/// What a block returns.
pub(crate) mod exit {
    /// Ran to its end (r15 holds the next instruction).
    pub const DONE: u32 = 0;
    /// A load or store faulted; the data abort has been taken.
    pub const FAULT: u32 = 1;
    /// A store wrote compiled code; the block stopped after it.
    pub const CODE_WRITTEN: u32 = 3;
}

#[cfg(target_arch = "x86_64")]
type Load = extern "sysv64" fn(*mut Ctx, u32, u32) -> u64;
#[cfg(target_arch = "x86_64")]
type Store = extern "sysv64" fn(*mut Ctx, u32, u32, u32) -> u32;
#[cfg(target_arch = "x86_64")]
type Exec = extern "sysv64" fn(*mut Ctx, u32, u32) -> u32;

/// Entries in each fast-access table.
const FAST: usize = 1024;
/// The tag of an empty entry (a page address never has low bits set).
const NO_PAGE: u32 = 0xFFF;

/// A fast-access table entry: a virtual 4 KB page of plain RAM (`va &
/// !0xFFF`, or `NO_PAGE`) and its physical page minus it. Generated code
/// reads and writes such pages directly; the slow helpers fill the tables.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Fast {
    tag: u32,
    delta: u32,
}

const EMPTY: Fast = Fast { tag: NO_PAGE, delta: 0 };

/// Entries in the cache of blocks by virtual address.
const RECENT: usize = 4096;

/// A compiled block by its virtual address (1 = empty: ARM code is word
/// aligned). Valid while the translation stays as it was
/// (`ArmJit::fast_for`), so it needs no lookup through the MMU.
#[derive(Clone, Copy)]
struct Recent {
    va: u32,
    len: u32,
    code: *const u8,
}

const NO_BLOCK: Recent = Recent { va: 1, len: 0, code: std::ptr::null() };

#[cfg(target_arch = "x86_64")]
/// What generated code reaches through `r12`.
#[repr(C)]
pub(crate) struct Ctx {
    cpu: *mut Arm,
    bus: *mut u8,
    /// Plain RAM (null when the bus has none) and its size.
    ram: *mut u8,
    ram_size: u64,
    /// `FAST` entries each, for loads and for stores.
    reads: *mut Fast,
    writes: *mut Fast,
    stats: *mut Stats,
    load32: Load,
    load8: Load,
    load16: Load,
    store32: Store,
    store8: Store,
    store16: Store,
    exec: Exec,
}

#[cfg(target_arch = "x86_64")]
/// Loads for generated code: the value, or bit 32 set when it faulted (the
/// data abort taken, with `current` the instruction's address).
extern "sysv64" fn load32<B: ArmBus>(ctx: *mut Ctx, va: u32, pc: u32) -> u64 {
    // SAFETY: `ArmJit::run` passes a live `Ctx` whose pointers it made from
    // the `&mut` it holds, and nothing else touches them during the call.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    match cpu.load32(bus, va) {
        Ok(v) => {
            fill(ctx, cpu, bus, va, false);
            v as u64
        }
        Err(f) => {
            cpu.data_abort(f);
            1 << 32
        }
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn load8<B: ArmBus>(ctx: *mut Ctx, va: u32, pc: u32) -> u64 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    match cpu.load8(bus, va) {
        Ok(v) => {
            fill(ctx, cpu, bus, va, false);
            v as u64
        }
        Err(f) => {
            cpu.data_abort(f);
            1 << 32
        }
    }
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn load16<B: ArmBus>(ctx: *mut Ctx, va: u32, pc: u32) -> u64 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    match cpu.load16(bus, va) {
        Ok(v) => {
            fill(ctx, cpu, bus, va, false);
            v as u64
        }
        Err(f) => {
            cpu.data_abort(f);
            1 << 32
        }
    }
}

#[cfg(target_arch = "x86_64")]
/// Stores for generated code: 0, `exit::FAULT` (abort taken), or
/// `exit::CODE_WRITTEN`.
extern "sysv64" fn store32<B: ArmBus>(ctx: *mut Ctx, va: u32, v: u32, pc: u32) -> u32 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    let r = cpu_store(cpu, bus, va, v, false);
    stored(ctx, cpu, bus, va, r)
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn store8<B: ArmBus>(ctx: *mut Ctx, va: u32, v: u32, pc: u32) -> u32 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    let r = cpu_store(cpu, bus, va, v, true);
    stored(ctx, cpu, bus, va, r)
}

#[cfg(target_arch = "x86_64")]
extern "sysv64" fn store16<B: ArmBus>(ctx: *mut Ctx, va: u32, v: u32, pc: u32) -> u32 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    let r = cpu.store16(bus, va, v);
    stored(ctx, cpu, bus, va, r)
}

#[cfg(target_arch = "x86_64")]
fn cpu_store<B: ArmBus>(cpu: &mut Arm, bus: &mut B, va: u32, v: u32, byte: bool) -> Result<(), ribbit_arm::Fault> {
    if byte { cpu.store8(bus, va, v) } else { cpu.store32(bus, va, v) }
}

#[cfg(target_arch = "x86_64")]
/// Runs instruction `w` at `pc` with the interpreter. Returns 1 when the
/// block must stop after it (r15 then holds where to go on): it branched,
/// took an exception, changed the mode, T, I or F, or wrote compiled code.
extern "sysv64" fn exec<B: ArmBus>(ctx: *mut Ctx, w: u32, pc: u32) -> u32 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    const KEPT: u32 = 0x1F | psr::T | psr::I | psr::F;
    let next = pc.wrapping_add(4);
    cpu.current = pc;
    cpu.r[15] = next;
    let before = cpu.cpsr & KEPT;
    let kind = match (w >> 25) & 7 {
        4 if w & (1 << 20) != 0 => 0,
        4 => 1,
        0 | 1 if w & 0x0DB0_0000 == 0x0120_0000 => 2,
        6 | 7 => 3,
        _ => 4,
    };
    // SAFETY: as above.
    unsafe { (*(*ctx).stats).execs[kind] += 1 };
    cpu.execute_arm(bus, w);
    let stop = cpu.r[15] != next || cpu.cpsr & KEPT != before || bus.code_watch().is_some_and(|w| !w.written.is_empty());
    stop as u32
}

#[cfg(target_arch = "x86_64")]
fn stored<B: ArmBus>(ctx: *mut Ctx, cpu: &mut Arm, bus: &mut B, va: u32, r: Result<(), ribbit_arm::Fault>) -> u32 {
    match r {
        Err(f) => {
            cpu.data_abort(f);
            exit::FAULT
        }
        Ok(()) if bus.code_watch().is_some_and(|w| !w.written.is_empty()) => exit::CODE_WRITTEN,
        Ok(()) => {
            fill(ctx, cpu, bus, va, true);
            0
        }
    }
}

#[cfg(target_arch = "x86_64")]
/// After an access at `va` that worked: when it is plain RAM (and for
/// stores, holds no compiled code), lets generated code reach its page
/// directly.
fn fill<B: ArmBus>(ctx: *mut Ctx, cpu: &mut Arm, bus: &mut B, va: u32, write: bool) {
    // SAFETY: as `load32`; the tables hold `FAST` entries.
    let ctx = unsafe { &*ctx };
    unsafe {
        if write {
            (*ctx.stats).slow_stores += 1;
        } else {
            (*ctx.stats).slow_loads += 1;
        }
    }
    if ctx.ram.is_null() {
        return;
    }
    let Ok(pa) = cpu.cp15.translate(bus, va, write, cpu.mode() == mode::USR) else { return };
    let page = pa & !0xFFF;
    if page as u64 + 0x1000 > ctx.ram_size || write && bus.code_watch().is_some_and(|w| w.watched_4k(page)) {
        return;
    }
    let table = if write { ctx.writes } else { ctx.reads };
    let vpage = va & !0xFFF;
    unsafe { *table.add((va >> 12) as usize % FAST) = Fast { tag: vpage, delta: page.wrapping_sub(vpage) } };
}

/// A multiplicative hash for the block keys (SipHash is slow here).
#[derive(Default)]
struct KeyHasher(u64);

impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.write_u64(self.0 ^ b as u64);
        }
    }
    fn write_u64(&mut self, v: u64) {
        let x = v.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        self.0 = x ^ (x >> 29);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}

type Map<K, V> = HashMap<K, V, BuildHasherDefault<KeyHasher>>;

#[derive(Clone, Copy)]
enum Block {
    /// Started this many times, not compiled yet.
    Count(u8),
    /// Compiled code and its instruction count.
    Code(*const u8, u32),
    /// Not compiled (not in RAM, or the arena refused it).
    Never,
}

/// Counters for diagnostics.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    pub compiled: u64,
    pub blocks_run: u64,
    pub jit_instructions: u64,
    pub interpreted: u64,
    pub dropped: u64,
    pub resets: u64,
    /// Interpreted instructions by `decode::class` (with `ArmJit::profile`).
    pub classes: [u64; decode::CLASSES.len()],
    /// Translation changes (the fast tables and block cache emptied).
    pub flushes: u64,
    /// Loads and stores that missed the fast tables.
    pub slow_loads: u64,
    pub slow_stores: u64,
    /// Instructions compiled code ran with the interpreter: LDM, STM,
    /// MSR, coprocessor, other.
    pub execs: [u64; 5],
}

/// The block cache and run loop step: `step` replaces `Arm::step`.
pub struct ArmJit {
    blocks: Map<u64, Block>,
    /// Keys of the blocks compiled from each 1 KB physical page.
    pages: Map<u32, Vec<u64>>,
    #[cfg(target_arch = "x86_64")]
    arena: ribbit_jit::exec_mem::Arena,
    /// The next instruction starts a block (after a branch, a block, or an
    /// instruction found not compilable).
    start: bool,
    /// Physical addresses below this are RAM (code is compiled only there).
    ram_end: u32,
    /// The fast tables and the block cache, privileged mode's then User
    /// mode's (`FAST` or `RECENT` entries each).
    reads: Vec<Fast>,
    writes: Vec<Fast>,
    recent: Vec<Recent>,
    /// The CP15 generation they were filled under.
    fast_for: u32,
    /// Starts before a block is compiled.
    pub hot: u8,
    pub enabled: bool,
    /// Fill `stats.classes` (slow).
    pub profile: bool,
    pub stats: Stats,
}

// SAFETY: the code pointers point into `arena`, owned and only used here.
unsafe impl Send for ArmJit {}

impl ArmJit {
    pub fn new(ram_end: u32) -> Self {
        Self {
            blocks: Map::default(),
            pages: Map::default(),
            #[cfg(target_arch = "x86_64")]
            arena: Default::default(),
            start: true,
            ram_end,
            reads: vec![EMPTY; 2 * FAST],
            writes: vec![EMPTY; 2 * FAST],
            recent: vec![NO_BLOCK; 2 * RECENT],
            fast_for: 0,
            hot: 2,
            enabled: SUPPORTED,
            profile: false,
            stats: Stats::default(),
        }
    }

    /// Drops everything compiled (after a state is loaded, or a reset).
    pub fn clear(&mut self, bus: &mut impl ArmBus) {
        self.blocks.clear();
        self.pages.clear();
        #[cfg(target_arch = "x86_64")]
        {
            self.arena = Default::default();
        }
        if let Some(w) = bus.code_watch() {
            w.clear();
        }
        self.flush_fast();
        self.start = true;
    }

    fn flush_fast(&mut self) {
        self.reads.fill(EMPTY);
        self.writes.fill(EMPTY);
        self.recent.fill(NO_BLOCK);
    }

    fn interpret(&mut self, cpu: &mut Arm, bus: &mut impl ArmBus, start_next: bool) -> bool {
        let pc = cpu.r[15];
        if self.profile {
            let c = decode::class(cpu, bus);
            self.stats.classes[c] += 1;
        }
        let ran = cpu.step(bus);
        self.stats.interpreted += 1;
        self.start = start_next || cpu.r[15] != pc.wrapping_add(4);
        ran
    }

    /// Runs compiled blocks, or one instruction (or a pending interrupt)
    /// with the interpreter. A block runs only if its instruction count
    /// fits in `budget` cycles; more blocks follow while fewer than `chain`
    /// cycles have run (0: one block). Returns false while waiting for an
    /// interrupt.
    pub fn step<B: ArmBus>(&mut self, cpu: &mut Arm, bus: &mut B, budget: u64, chain: u64) -> bool {
        if !self.enabled
            || !self.start
            || cpu.waiting
            || cpu.cpsr & psr::T != 0
            || cpu.fiq_line && cpu.cpsr & psr::F == 0
            || cpu.irq_line && cpu.cpsr & psr::I == 0
        {
            return self.interpret(cpu, bus, false);
        }
        match bus.code_watch() {
            None => return self.interpret(cpu, bus, false),
            Some(w) if !w.written.is_empty() => {
                let written = std::mem::take(&mut w.written);
                for p in written {
                    self.drop_page(p);
                }
            }
            Some(_) => {}
        }
        let user = cpu.mode() == mode::USR;
        if cpu.cp15.generation != self.fast_for {
            self.stats.flushes += 1;
            self.flush_fast();
            self.fast_for = cpu.cp15.generation;
        }
        let va = cpu.r[15];
        let slot = user as usize * RECENT + (va >> 2) as usize % RECENT;
        let recent = self.recent[slot];
        let (code, len) = if recent.va == va {
            (recent.code, recent.len)
        } else {
            let Ok(pa) = cpu.cp15.translate(bus, va, false, user) else {
                return self.interpret(cpu, bus, false);
            };
            let key = (pa as u64) << 32 | va as u64;
            let block = *self.blocks.entry(key).or_insert(Block::Count(0));
            let (code, len) = match block {
                Block::Code(code, len) => (code, len),
                Block::Never => return self.interpret(cpu, bus, true),
                Block::Count(n) if n + 1 < self.hot => {
                    self.blocks.insert(key, Block::Count(n + 1));
                    return self.interpret(cpu, bus, false);
                }
                Block::Count(_) => match self.compile(bus, va, pa, key) {
                    Some(c) => c,
                    None => return self.interpret(cpu, bus, true),
                },
            };
            self.recent[slot] = Recent { va, len, code };
            (code, len)
        };
        if len as u64 > budget {
            return self.interpret(cpu, bus, true);
        }
        // Blocks found in the cache run one after another for up to `chain`
        // cycles, without going back to the caller, while nothing it handles
        // between blocks is needed: an interrupt, Thumb code, waiting,
        // written code, a translation or privilege change.
        let (first, mut code, mut len) = (cpu.cycles, code, len);
        loop {
            let exit = self.run(cpu, bus, code, user);
            self.stats.blocks_run += 1;
            self.stats.jit_instructions += len as u64;
            let ran = cpu.cycles - first;
            if exit != exit::DONE
                || ran >= chain
                || cpu.waiting
                || cpu.cpsr & psr::T != 0
                || cpu.fiq_line && cpu.cpsr & psr::F == 0
                || cpu.irq_line && cpu.cpsr & psr::I == 0
                || (cpu.mode() == mode::USR) != user
                || cpu.cp15.generation != self.fast_for
                || bus.code_watch().is_some_and(|w| !w.written.is_empty())
            {
                break;
            }
            let va = cpu.r[15];
            let next = self.recent[user as usize * RECENT + (va >> 2) as usize % RECENT];
            if next.va != va || next.len as u64 > budget.saturating_sub(ran) {
                break;
            }
            (code, len) = (next.code, next.len);
        }
        self.start = true;
        true
    }

    fn drop_page(&mut self, page: u32) {
        let Some(keys) = self.pages.remove(&page) else { return };
        for key in keys {
            self.blocks.remove(&key);
            self.stats.dropped += 1;
        }
        self.recent.fill(NO_BLOCK);
    }

    /// Compiles the block at `va` (physical `pa`); marks it `Never` when it
    /// is not in RAM.
    fn compile<B: ArmBus>(&mut self, bus: &mut B, va: u32, pa: u32, key: u64) -> Option<(*const u8, u32)> {
        if !SUPPORTED || pa >= self.ram_end {
            self.blocks.insert(key, Block::Never);
            return None;
        }
        let mut instrs = Vec::new();
        for i in 0..MAX_BLOCK {
            let at = va.wrapping_add(4 * i);
            if i > 0 && at & ((1 << CodeWatch::PAGE_SHIFT) - 1) == 0 {
                break;
            }
            let ins = decode::decode(bus.read32(pa + 4 * i), at);
            instrs.push(ins);
            if ins.ends_block() {
                break;
            }
        }
        let page = pa >> CodeWatch::PAGE_SHIFT;
        bus.code_watch()?.watch(pa);
        // Stores into this page must now go through the watch.
        let page4k = pa & !0xFFF;
        for e in self.writes.iter_mut() {
            if e.tag.wrapping_add(e.delta) == page4k {
                *e = EMPTY;
            }
        }
        self.pages.entry(page).or_default().push(key);
        let code = self.emit(va, &instrs);
        let Some(code) = code else {
            self.blocks.insert(key, Block::Never);
            return None;
        };
        let len = instrs.len() as u32;
        self.blocks.insert(key, Block::Code(code, len));
        self.stats.compiled += 1;
        Some((code, len))
    }

    #[cfg(target_arch = "x86_64")]
    fn emit(&mut self, va: u32, instrs: &[decode::Instr]) -> Option<*const u8> {
        if instrs.is_empty() {
            return None;
        }
        if self.arena.used() > ARENA_LIMIT {
            // Start over (the caller re-registers this block afterwards).
            self.blocks.retain(|_, b| matches!(b, Block::Count(_)));
            self.pages.clear();
            self.recent.fill(NO_BLOCK);
            self.arena = Default::default();
            self.stats.resets += 1;
        }
        let code = x64::emit(va, instrs)?;
        self.arena.add(&code).ok()
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn emit(&mut self, _: u32, _: &[decode::Instr]) -> Option<*const u8> {
        None
    }

    #[cfg(target_arch = "x86_64")]
    fn run<B: ArmBus>(&mut self, cpu: &mut Arm, bus: &mut B, code: *const u8, user: bool) -> u32 {
        let side = user as usize * FAST;
        let (ram, ram_size) = bus.ram().unwrap_or((std::ptr::null_mut(), 0));
        let mut ctx = Ctx {
            cpu,
            bus: bus as *mut B as *mut u8,
            ram,
            ram_size: ram_size as u64,
            reads: self.reads[side..].as_mut_ptr(),
            writes: self.writes[side..].as_mut_ptr(),
            stats: &mut self.stats,
            load32: load32::<B>,
            load8: load8::<B>,
            load16: load16::<B>,
            store32: store32::<B>,
            store8: store8::<B>,
            store16: store16::<B>,
            exec: exec::<B>,
        };
        // SAFETY: `code` is a function `x64::emit` generated, in the arena.
        let f: extern "sysv64" fn(*mut Ctx) -> u32 = unsafe { std::mem::transmute(code) };
        f(&mut ctx)
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn run<B: ArmBus>(&mut self, _: &mut Arm, _: &mut B, _: *const u8, _: bool) -> u32 {
        unreachable!("nothing is compiled on this host")
    }
}

#[cfg(test)]
mod tests;
