//! Other architectures: no code generator yet (docs/jit-design.md, phase
//! 3). The same API, compiling nothing: `Jit::from_env` is None.

use ribbit_cpu::icache::{BlockHead, Blocks};
use ribbit_cpu::{BlockRunner, Cpu, RunnerExit};
use ribbit_memory::Bus;

/// This build can compile (x86-64 only, for now).
pub const SUPPORTED: bool = false;

/// What the JIT did (speed diagnostics).
#[derive(Debug, Clone, Copy, Default)]
pub struct JitStats {
    pub compiled: u64,
    pub runs: u64,
    pub instructions: u64,
    pub fallbacks: u64,
    pub code_bytes: u64,
    pub checked: u64,
    pub exits: [u64; 6],
    pub continued: u64,
    pub interpreted: u64,
    pub batches: u64,
}

/// Research (RIBBIT_JIT_PROFILE): unused here.
#[allow(clippy::type_complexity)]
pub static PROFILE: std::sync::Mutex<Option<[std::collections::HashMap<u32, u64>; 3]>> = std::sync::Mutex::new(None);

/// The JIT: never made on this architecture.
#[derive(Default)]
pub struct Jit {
    pub check: bool,
    pub stats: JitStats,
}

impl Jit {
    /// None: no code generator for this architecture.
    pub fn from_env() -> Option<Jit> {
        None
    }
}

impl<B: Bus> BlockRunner<B> for Jit {
    fn run(
        &mut self,
        _: &mut Cpu,
        _: &mut B,
        _: &mut Blocks,
        _: BlockHead,
        _: u64,
        _: u64,
        _: u32,
        _: &mut dyn FnMut(u32, u64) -> bool,
    ) -> Option<RunnerExit> {
        None
    }
}
