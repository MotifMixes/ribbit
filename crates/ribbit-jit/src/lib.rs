//! Just-in-time compiler for the ARCtangent-A5 (ARCompact) core in
//! `ribbit-cpu` (docs/jit-design.md).
//!
//! STATUS: phases 1-3. Blocks of the interpreter's block cache
//! (`icache::Blocks`) compile to x86-64 or AArch64 (the same code,
//! function for function: `x64.rs`, `aarch64.rs`) once they have run a few times
//! (`Jit`, a `BlockRunner` for `Cpu::run_block_with`), with the
//! interpreter's exact results, cycle timing included: the pipeline
//! scoreboard runs in the generated code, and the memory system's timing
//! is called where `run_block` calls it (fetches off the line just fetched,
//! every memory access). Instructions the generator does not handle end the
//! compiled code with a fallback exit, and the interpreter runs them.
//! Checked against the interpreter instruction by instruction
//! (tests/lockstep.rs) and, in a running machine, block by block
//! (`Jit::check`). Other architectures compile nothing (`SUPPORTED` is
//! false).
//!
//! Guest state is never copied: generated code reads and writes `Cpu::regs`,
//! `Cpu::status32` and the scoreboard in place through `JitCtx`, so save
//! states, the debugger and the interpreter always see one state, and
//! nothing compiled has to be saved.

#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub mod exec_mem;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
mod full;
#[cfg(target_arch = "x86_64")]
mod x64;
#[cfg(target_arch = "aarch64")]
mod aarch64;
#[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
pub use full::*;

#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
mod stub;
#[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
pub use stub::*;
