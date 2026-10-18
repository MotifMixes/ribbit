//! Just-in-time compiler for the ARCtangent-A5 (ARCompact) core in
//! `ribbit-cpu` (docs/jit-design.md).
//!
//! STATUS: phase 1 foundation. Straight-line runs of the instruction shapes
//! the batched interpreter pre-digests (`icache::Fast`) compile to x86-64;
//! anything else ends the compiled code with a fallback exit, and the
//! interpreter runs it. Results are checked against the interpreter
//! instruction by instruction (tests/lockstep.rs). Not yet: cycle timing
//! (pipeline scoreboard, memory stalls; the exit reports how many
//! instructions ran so they can be added), inline memory fast paths, and
//! the hook-up to `Cpu::run_block`. Other architectures compile nothing
//! (`SUPPORTED` is false).
//!
//! Guest state is never copied: generated code reads and writes `Cpu::regs`
//! and `Cpu::status32` in place through `JitCtx`, so save states, the
//! debugger and the interpreter always see one state, and nothing compiled
//! has to be saved.

pub mod exec_mem;
#[cfg(target_arch = "x86_64")]
mod x64;

use ribbit_cpu::Cpu;
use ribbit_cpu::isa::Instr;
use ribbit_memory::Bus;
use std::ffi::c_void;

/// This build can compile (x86-64 only, for now).
pub const SUPPORTED: bool = cfg!(target_arch = "x86_64");

/// A memory read helper: (bus, address) -> value (zero-extended).
pub type ReadFn = extern "sysv64" fn(*mut c_void, u32) -> u32;
/// A memory write helper: (bus, address, value; the low bits for 8/16-bit).
pub type WriteFn = extern "sysv64" fn(*mut c_void, u32, u32);

/// What generated code works on. Inputs are set by `Block::run` before the
/// call; outputs are written by the code at its exit. `repr(C)`: the code
/// addresses the fields by offset. The helpers use the System V calling
/// convention on every OS (`extern "sysv64"`), so the generated code is the
/// same on Windows and Linux.
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
    /// `Cpu::lp_start` / `lp_end` (compiled code never changes them).
    pub lp_start: u32,
    pub lp_end: u32,
    /// Out: the next PC.
    pub pc: u32,
    /// Out: instructions executed (each retired, for timing and counts).
    pub executed: u32,
    /// Out: nonzero if a taken delayed branch's slot is next
    /// (`Cpu::delay_target` = `delay_target`).
    pub delay_pending: u32,
    pub delay_target: u32,
}

extern "sysv64" fn read8<B: Bus>(bus: *mut c_void, a: u32) -> u32 {
    // SAFETY: `bus` is the `&mut B` that `Block::run` put in the context.
    unsafe { (*(bus as *mut B)).read8(a) as u32 }
}
extern "sysv64" fn read16<B: Bus>(bus: *mut c_void, a: u32) -> u32 {
    // SAFETY: as `read8`.
    unsafe { (*(bus as *mut B)).read16(a) as u32 }
}
extern "sysv64" fn read32<B: Bus>(bus: *mut c_void, a: u32) -> u32 {
    // SAFETY: as `read8`.
    unsafe { (*(bus as *mut B)).read32(a) }
}
extern "sysv64" fn write8<B: Bus>(bus: *mut c_void, a: u32, v: u32) {
    // SAFETY: as `read8`.
    unsafe { (*(bus as *mut B)).write8(a, v as u8) }
}
extern "sysv64" fn write16<B: Bus>(bus: *mut c_void, a: u32, v: u32) {
    // SAFETY: as `read8`.
    unsafe { (*(bus as *mut B)).write16(a, v as u16) }
}
extern "sysv64" fn write32<B: Bus>(bus: *mut c_void, a: u32, v: u32) {
    // SAFETY: as `read8`.
    unsafe { (*(bus as *mut B)).write32(a, v) }
}

impl JitCtx {
    /// A context for `cpu` and `bus`, valid while both are borrowed.
    pub fn new<B: Bus>(cpu: &mut Cpu, bus: &mut B) -> JitCtx {
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
            lp_start: cpu.lp_start,
            lp_end: cpu.lp_end,
            pc: cpu.pc,
            executed: 0,
            delay_pending: 0,
            delay_target: 0,
        }
    }
}

/// Why compiled code returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum ExitKind {
    /// The block ran to its end: off its last instruction, or a branch
    /// decided the PC (taken or not).
    End = 0,
    /// The instruction at `pc` is not compiled: the interpreter runs it
    /// (one `Cpu::step_with`), then compiled code can resume after it.
    Fallback = 1,
    /// A zero-overhead loop's end was reached (LP_COUNT decremented; `pc`
    /// is LP_START, or the next instruction on the last pass).
    Loop = 2,
}

/// How a run of compiled code ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Exit {
    pub kind: ExitKind,
    pub pc: u32,
    /// Instructions executed. Timing (phase 1, part 2) will add the cycles
    /// the interpreter's `pipeline_cost` and memory stalls give; the exit
    /// point identifies which instructions those were.
    pub executed: u32,
    /// A taken delayed branch whose slot was not compiled: `Cpu::delay_target`.
    pub delay_target: Option<u32>,
}

/// Compiled code for a straight-line run of guest instructions.
pub struct Block {
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))]
    mem: exec_mem::ExecMemory,
    /// Guest address of the first instruction.
    pub start: u32,
    /// Instructions compiled (the fallback exit's index, or the block's
    /// length up to and including a branch and its delay slot).
    pub compiled: usize,
}

impl Block {
    /// Generated machine code (for dumps).
    pub fn code(&self) -> &[u8] {
        self.mem.code()
    }

    /// Runs the block on `cpu` with `bus`: registers, flags, LP_COUNT and
    /// memory change as the interpreter would change them; `cpu.pc` and
    /// `cpu.delay_target` are set from the exit. Cycles, timers, the
    /// pipeline scoreboard and `instructions_retired` are left to the caller
    /// (from `Exit::executed`). `cpu.pc` must be `start` and no delay slot
    /// pending.
    pub fn run<B: Bus>(&self, cpu: &mut Cpu, bus: &mut B) -> Exit {
        assert_eq!(cpu.pc, self.start, "block entered at another pc");
        assert!(cpu.delay_target.is_none(), "block entered in a delay slot");
        let mut ctx = JitCtx::new(cpu, bus);
        // SAFETY: the context points at live `cpu` and `bus` for the call;
        // the code only touches what the context names.
        let kind = unsafe { self.enter(&mut ctx) };
        let exit = Exit {
            kind,
            pc: ctx.pc,
            executed: ctx.executed,
            delay_target: (ctx.delay_pending != 0).then_some(ctx.delay_target),
        };
        cpu.pc = exit.pc;
        cpu.delay_target = exit.delay_target;
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
            let f: extern "sysv64" fn(*mut JitCtx) -> u32 = unsafe { std::mem::transmute(self.mem.as_ptr()) };
            match f(ctx) {
                0 => ExitKind::End,
                1 => ExitKind::Fallback,
                _ => ExitKind::Loop,
            }
        }
        #[cfg(not(target_arch = "x86_64"))]
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

/// Compiles `instrs`, a straight run of decoded instructions (from
/// read-only memory: the code is not watched for changes). Compilation
/// stops before the first instruction it cannot compile (that becomes a
/// fallback exit) or after the first branch or jump and its delay slot.
pub fn compile(instrs: &[Instr]) -> Result<Block, CompileError> {
    if instrs.windows(2).any(|w| w[1].addr != w[0].next()) {
        return Err(CompileError::NotSequential);
    }
    let Some(first) = instrs.first() else {
        return Err(CompileError::NotCompilable);
    };
    #[cfg(target_arch = "x86_64")]
    {
        let (code, compiled) = x64::emit(instrs).ok_or(CompileError::NotCompilable)?;
        let mem = exec_mem::ExecMemory::new(&code).map_err(|e| CompileError::Memory(e.to_string()))?;
        Ok(Block { mem, start: first.addr, compiled })
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        let _ = first;
        Err(CompileError::Unsupported)
    }
}
