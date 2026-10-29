//! Just-in-time compiler for the ARM926EJ-S in `ribbit-arm`
//! (docs/arm-jit-design.md).
//!
//! STATUS: phase 1. Straight-line ARM code (data processing with immediate
//! shifts, LDR/STR word and byte, B/BL) compiles to x86-64 once a block has
//! started twice; everything else, Thumb, and pending interrupts go to the
//! interpreter (`Arm::step`). Other hosts compile nothing (`SUPPORTED` is
//! false).
//!
//! Guest state is never copied: generated code reads and writes `Arm::r`,
//! `cpsr` and the counters in place, and every load and store goes through
//! `Arm::load32` etc. (the MMU and the bus, as the interpreter's), so save
//! states, the debugger and the interpreter always see one state.
//!
//! Blocks are keyed by (virtual PC, physical PC) and never cross a 1 KB
//! page. The bus reports writes into pages holding code (`CodeWatch`), and
//! their blocks are dropped before the next block runs; a block that writes
//! compiled code stops right after that store.

mod decode;
#[cfg(target_arch = "x86_64")]
mod x64;

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
    /// Stopped before an instruction the generator does not handle.
    pub const FALLBACK: u32 = 2;
    /// A store wrote compiled code; the block stopped after it.
    pub const CODE_WRITTEN: u32 = 3;
}

type Load = extern "sysv64" fn(*mut Ctx, u32, u32) -> u64;
type Store = extern "sysv64" fn(*mut Ctx, u32, u32, u32) -> u32;

/// What generated code reaches through `r12`.
#[repr(C)]
pub(crate) struct Ctx {
    cpu: *mut Arm,
    bus: *mut u8,
    load32: Load,
    load8: Load,
    store32: Store,
    store8: Store,
}

/// Loads for generated code: the value, or bit 32 set when it faulted (the
/// data abort taken, with `current` the instruction's address).
extern "sysv64" fn load32<B: ArmBus>(ctx: *mut Ctx, va: u32, pc: u32) -> u64 {
    // SAFETY: `ArmJit::run` passes a live `Ctx` whose pointers it made from
    // the `&mut` it holds, and nothing else touches them during the call.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    match cpu.load32(bus, va) {
        Ok(v) => v as u64,
        Err(f) => {
            cpu.data_abort(f);
            1 << 32
        }
    }
}

extern "sysv64" fn load8<B: ArmBus>(ctx: *mut Ctx, va: u32, pc: u32) -> u64 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    match cpu.load8(bus, va) {
        Ok(v) => v as u64,
        Err(f) => {
            cpu.data_abort(f);
            1 << 32
        }
    }
}

/// Stores for generated code: 0, `exit::FAULT` (abort taken), or
/// `exit::CODE_WRITTEN`.
extern "sysv64" fn store32<B: ArmBus>(ctx: *mut Ctx, va: u32, v: u32, pc: u32) -> u32 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    let r = cpu_store(cpu, bus, va, v, false);
    stored(cpu, bus, r)
}

extern "sysv64" fn store8<B: ArmBus>(ctx: *mut Ctx, va: u32, v: u32, pc: u32) -> u32 {
    // SAFETY: as `load32`.
    let (cpu, bus) = unsafe { (&mut *(*ctx).cpu, &mut *((*ctx).bus as *mut B)) };
    cpu.current = pc;
    let r = cpu_store(cpu, bus, va, v, true);
    stored(cpu, bus, r)
}

fn cpu_store<B: ArmBus>(cpu: &mut Arm, bus: &mut B, va: u32, v: u32, byte: bool) -> Result<(), ribbit_arm::Fault> {
    if byte { cpu.store8(bus, va, v) } else { cpu.store32(bus, va, v) }
}

fn stored<B: ArmBus>(cpu: &mut Arm, bus: &mut B, r: Result<(), ribbit_arm::Fault>) -> u32 {
    match r {
        Err(f) => {
            cpu.data_abort(f);
            exit::FAULT
        }
        Ok(()) if bus.code_watch().is_some_and(|w| !w.written.is_empty()) => exit::CODE_WRITTEN,
        Ok(()) => 0,
    }
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
    /// Its first instruction is not handled.
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
    /// Starts before a block is compiled.
    pub hot: u8,
    pub enabled: bool,
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
            hot: 2,
            enabled: SUPPORTED,
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
        self.start = true;
    }

    fn interpret(&mut self, cpu: &mut Arm, bus: &mut impl ArmBus, start_next: bool) -> bool {
        let pc = cpu.r[15];
        let ran = cpu.step(bus);
        self.stats.interpreted += 1;
        self.start = start_next || cpu.r[15] != pc.wrapping_add(4);
        ran
    }

    /// Runs a compiled block, or one instruction (or a pending interrupt)
    /// with the interpreter. A block runs only if its instruction count
    /// fits in `budget` cycles. Returns false while waiting for an interrupt.
    pub fn step<B: ArmBus>(&mut self, cpu: &mut Arm, bus: &mut B, budget: u64) -> bool {
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
        let va = cpu.r[15];
        let Ok(pa) = cpu.cp15.translate(bus, va, false, cpu.mode() == mode::USR) else {
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
        if len as u64 > budget {
            return self.interpret(cpu, bus, true);
        }
        self.run(cpu, bus, code);
        self.stats.blocks_run += 1;
        self.stats.jit_instructions += len as u64;
        self.start = true;
        true
    }

    fn drop_page(&mut self, page: u32) {
        for key in self.pages.remove(&page).unwrap_or_default() {
            self.blocks.remove(&key);
            self.stats.dropped += 1;
        }
    }

    /// Compiles the block at `va` (physical `pa`); marks it `Never` when its
    /// first instruction is not handled.
    fn compile<B: ArmBus>(&mut self, bus: &mut B, va: u32, pa: u32, key: u64) -> Option<(*const u8, u32)> {
        if !SUPPORTED || pa >= self.ram_end {
            self.blocks.insert(key, Block::Never);
            return None;
        }
        let mut instrs = Vec::new();
        let mut fallback = false;
        for i in 0..MAX_BLOCK {
            let at = va.wrapping_add(4 * i);
            if i > 0 && at & ((1 << CodeWatch::PAGE_SHIFT) - 1) == 0 {
                break;
            }
            match decode::decode(bus.read32(pa + 4 * i), at) {
                Some(ins) => {
                    instrs.push(ins);
                    if ins.ends_block() {
                        break;
                    }
                }
                None => {
                    fallback = true;
                    break;
                }
            }
        }
        let page = pa >> CodeWatch::PAGE_SHIFT;
        bus.code_watch()?.watch(pa);
        self.pages.entry(page).or_default().push(key);
        let code = self.emit(va, &instrs, fallback);
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
    fn emit(&mut self, va: u32, instrs: &[decode::Instr], fallback: bool) -> Option<*const u8> {
        if instrs.is_empty() {
            return None;
        }
        if self.arena.used() > ARENA_LIMIT {
            // Start over (the caller re-registers this block afterwards).
            self.blocks.retain(|_, b| matches!(b, Block::Count(_)));
            self.pages.clear();
            self.arena = Default::default();
            self.stats.resets += 1;
        }
        let code = x64::emit(va, instrs, fallback)?;
        self.arena.add(&code).ok()
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn emit(&mut self, _: u32, _: &[decode::Instr], _: bool) -> Option<*const u8> {
        None
    }

    #[cfg(target_arch = "x86_64")]
    fn run<B: ArmBus>(&mut self, cpu: &mut Arm, bus: &mut B, code: *const u8) -> u32 {
        let mut ctx = Ctx {
            cpu,
            bus: bus as *mut B as *mut u8,
            load32: load32::<B>,
            load8: load8::<B>,
            store32: store32::<B>,
            store8: store8::<B>,
        };
        // SAFETY: `code` is a function `x64::emit` generated, in the arena.
        let f: extern "sysv64" fn(*mut Ctx) -> u32 = unsafe { std::mem::transmute(code) };
        f(&mut ctx)
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn run<B: ArmBus>(&mut self, _: &mut Arm, _: &mut B, _: *const u8) -> u32 {
        unreachable!("nothing is compiled on this host")
    }
}

#[cfg(test)]
mod tests;
