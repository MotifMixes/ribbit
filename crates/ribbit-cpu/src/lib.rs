//! ARCtangent-A5 ("ARCTangent 5.1") processor model.
//!
//! The ISA is ARCompact as specified by the *ARCompact ISA Programmer's
//! Reference* (5115-029); A5-specific behaviour is used where the manual
//! distinguishes processors. Evidence, assumptions and known gaps are in
//! docs/isa-arcompact.md.
//!
//! STATUS: decoder validated against GNU binutils; executor implements the
//! basecase ISA plus the MUL64, NORM and SWAP options and part of the
//! extended-arithmetic library. Timing is one cycle per instruction, which is
//! a placeholder, not a model of the A5 pipeline.

pub mod decode;
pub mod disasm;
mod exec;
pub mod icache;
pub mod idle;
pub mod isa;
pub mod xy;

pub use exec::{Block, BlockExit, BlockRunner, NoRunner, RunnerExit, RunnerStop};
use isa::Instr;
use std::fmt;
use thiserror::Error;

pub const NUM_CORE_REGS: usize = 64;

pub mod reg {
    pub const GP: usize = 26;
    pub const FP: usize = 27;
    pub const SP: usize = 28;
    pub const ILINK1: usize = 29;
    pub const ILINK2: usize = 30;
    pub const BLINK: usize = 31;
    pub const MLO: usize = 57;
    pub const MMID: usize = 58;
    pub const MHI: usize = 59;
    pub const LP_COUNT: usize = 60;
    pub const LIMM: usize = 62;
    pub const PCL: usize = 63;
}

/// STATUS32 bit layout (PRM Figure 45). The A5 does not implement A1, A2,
/// AE, DE, U or L; they read as 0 (PRM, STATUS32).
pub mod status32 {
    pub const H: u32 = 1 << 0;
    pub const E1: u32 = 1 << 1;
    pub const E2: u32 = 1 << 2;
    pub const V: u32 = 1 << 8;
    pub const C: u32 = 1 << 9;
    pub const N: u32 = 1 << 10;
    pub const Z: u32 = 1 << 11;
    /// Bits an A5 STATUS32 can hold.
    pub const IMPLEMENTED: u32 = H | E1 | E2 | V | C | N | Z;
    /// Bits FLAG may write (when H is not being set).
    pub const FLAG_WRITABLE: u32 = E1 | E2 | V | C | N | Z;
}

/// DEBUG register bits (PRM Figure 43).
pub mod debug {
    pub const BH: u32 = 1 << 29;
    pub const SH: u32 = 1 << 30;
    pub const ZZ: u32 = 1 << 23;
}

/// What MMID holds after MUL64/MULU64.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MmidMode {
    /// PRM: the middle 32 bits of the 64-bit product (bits 47:16).
    Middle,
    /// The low 32 bits, as MAME's Leapster configuration does: the v1.5 BIOS
    /// at 0x4007_6958 adds MMID to a constant right after `mul64_s` to size
    /// an allocation, which only works if MMID holds the low word. HYPOTHESIS
    /// pending a hardware test.
    LowLeapster,
}

/// What happens on an encoding the PRM defines as an Instruction Error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum IllegalPolicy {
    /// Stop with a diagnostic. During bring-up an illegal instruction is far
    /// more likely to be an emulator bug than intended behaviour.
    Stop,
    /// Take the Instruction Error interrupt (vector 2, ILINK2), as hardware does.
    Exception,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CpuConfig {
    /// INT_VECTOR_BASE after reset (VECBASE_AC_BUILD). 0x4000_0000 for the
    /// Leapster per MAME; the BIOS vector table is there.
    pub reset_vector_base: u32,
    /// IDENTITY (aux 0x04). ARCVER 0x10-0x1F means ARCtangent-A5 (PRM); the
    /// Leapster's actual CHIPID/ARCNUM/ARCVER are UNKNOWN.
    pub identity: u32,
    pub mmid: MmidMode,
    pub illegal: IllegalPolicy,
    /// XY memory / DSP extension (r32-r56, aux 0x80-0x9F). Present on the
    /// Leapster: its RTOS saves and restores r32-r59 per task.
    pub xy_extension: bool,
    /// Charge the pipeline costs the PRM documents for the ARCtangent-A5
    /// (`Cpu::pipeline_cost`); otherwise every instruction is one cycle.
    #[serde(default = "pipeline_timing_default")]
    pub pipeline_timing: bool,
    /// Cycles from a load's issue (after any memory stall) until its result
    /// can be read (pipeline timing). CALIBRATED (docs/hardware-research.md,
    /// "CPU timing"): the PRM says only "a minimum of one cycle".
    pub load_latency: u64,
}

fn pipeline_timing_default() -> bool {
    true
}

impl CpuConfig {
    pub fn leapster() -> Self {
        Self {
            reset_vector_base: 0x4000_0000,
            identity: 0x0000_0010,
            mmid: MmidMode::LowLeapster,
            illegal: IllegalPolicy::Stop,
            xy_extension: true,
            pipeline_timing: true,
            load_latency: 3,
        }
    }
}

/// An ARC processor timer (PRM "Processor Timers Auxiliary Registers").
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Timer {
    pub count: u32,
    pub control: u32,
    pub limit: u32,
    /// Level interrupt raised on reaching the limit; cleared by writing CONTROL.
    pub pending: bool,
}

impl Timer {
    pub const IE: u32 = 1 << 0;
    pub const NH: u32 = 1 << 1;
    pub const W: u32 = 1 << 2;
    pub const IP: u32 = 1 << 3;

    fn reset() -> Self {
        Timer { count: 0, control: 0, limit: 0x00FF_FFFF, pending: false }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxKind {
    Read,
    Write,
}

/// An LR/SR to an auxiliary register Ribbit does not model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuxAccess {
    pub pc: u32,
    pub reg: u32,
    pub kind: AuxKind,
    pub value: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// First unmodelled aux accesses, kept verbatim.
    pub aux_unmodelled: Vec<AuxAccess>,
    pub aux_unmodelled_total: u64,
    /// Writes to MLO/MMID/MHI, which the PRM calls read-only. They are
    /// performed (the Leapster BIOS writes MMID when restoring multiply state).
    pub mul_result_writes: u64,
}

/// Leapster-specific auxiliary registers are supplied by the machine.
pub trait AuxExt {
    fn aux_read(&mut self, reg: u32) -> Option<u32>;
    fn aux_write(&mut self, reg: u32, value: u32) -> bool;
}

/// No machine-specific auxiliary registers.
pub struct NoAuxExt;

impl AuxExt for NoAuxExt {
    fn aux_read(&mut self, _: u32) -> Option<u32> {
        None
    }
    fn aux_write(&mut self, _: u32, _: u32) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HaltReason {
    /// FLAG with H set.
    Flag,
    /// BRK / BRK_S.
    Brk,
}

/// What one call to `Cpu::step` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// An instruction executed; it is in `Cpu::last_instr`.
    Executed,
    /// An interrupt or exception was taken; PC is now its vector.
    Interrupt { vector: u8 },
    /// The CPU is asleep (SLEEP) and no enabled interrupt is pending.
    Sleeping,
    /// The instruction halted the CPU (`Cpu::last_instr`).
    Halted(HaltReason),
}

/// Text of a CPU error. Boxed so that `Result<_, CpuError>` stays small: it is
/// returned by every instruction, and only built when something goes wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorInfo {
    /// Disassembly of the instruction.
    pub text: String,
    pub reason: String,
}

impl ErrorInfo {
    pub fn new(text: String, reason: impl Into<String>) -> Box<Self> {
        Box::new(Self { text, reason: reason.into() })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CpuError {
    #[error("unsupported instruction `{}` ({word:#010x}) at pc {pc:#010x}: {}", .info.text, .info.reason)]
    Unsupported { pc: u32, word: u32, info: Box<ErrorInfo> },
    #[error("illegal instruction `{}` ({word:#010x}) at pc {pc:#010x}: {}", .info.text, .info.reason)]
    Illegal { pc: u32, word: u32, info: Box<ErrorInfo> },
    #[error("unpredictable behaviour at pc {pc:#010x} (`{}`): {}", .info.text, .info.reason)]
    Unpredictable { pc: u32, info: Box<ErrorInfo> },
    #[error("cpu is halted at pc {pc:#010x}")]
    Halted { pc: u32 },
}

/// `Serialize`/`Deserialize` cover the architectural state (save states);
/// the decode cache, diagnostics and `last_instr` are rebuilt, not saved.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Cpu {
    #[serde(with = "ribbit_memory::serde_array")]
    pub regs: [u32; NUM_CORE_REGS],
    pub pc: u32,
    pub status32: u32,
    pub status32_l1: u32,
    pub status32_l2: u32,
    pub lp_start: u32,
    pub lp_end: u32,
    pub int_vector_base: u32,
    pub aux_irq_lv12: u32,
    pub aux_irq_lev: u32,
    pub aux_irq_hint: u32,
    pub semaphore: u32,
    pub debug: u32,
    pub macmode: u32,
    /// Dual 16x16 multiply-accumulate accumulators, [low lane, high lane]
    /// (MULDW/MACDW/MSUBDW). SOURCED (MAME `dsp_multiply`), UNVERIFIED.
    #[serde(default)]
    pub mac_acc: [i64; 2],
    pub timers: [Timer; 2],
    /// Level-sensitive interrupt inputs IRQ3-IRQ31 driven by the machine.
    pub irq_lines: u32,
    /// Target of a taken delayed branch whose delay slot executes next.
    pub delay_target: Option<u32>,
    pub sleeping: bool,
    pub cycles: u64,
    pub instructions_retired: u64,
    /// Cycle at which each core register's pending load result arrives
    /// (pipeline timing): reading it earlier stalls until then.
    pub ready: Vec<u64>,
    /// The latest of `ready` (a quick "nothing pending" check).
    pub ready_max: u64,
    pub config: CpuConfig,
    #[serde(skip)]
    pub diag: Diagnostics,
    /// Present when `config.xy_extension` is set.
    pub xy: Option<xy::XyExtension>,
    /// Decoded-instruction cache for ROM ranges (speed only).
    #[serde(skip)]
    pub icache: icache::ICache,
    /// The most recently executed instruction (kept here rather than in the
    /// step result, which is moved on every step).
    #[serde(skip)]
    pub last_instr: Option<Instr>,
    /// Keep `last_instr` after every step (diagnostics; an 88-byte copy per
    /// instruction). A halting instruction is always kept.
    #[serde(skip, default = "keep_last_instr")]
    pub record_last: bool,
}

fn keep_last_instr() -> bool {
    true
}

impl Cpu {
    pub fn new(config: CpuConfig) -> Self {
        let mut cpu = Self {
            regs: [0; NUM_CORE_REGS],
            pc: 0,
            status32: 0,
            status32_l1: 0,
            status32_l2: 0,
            lp_start: 0,
            lp_end: 0,
            int_vector_base: 0,
            aux_irq_lv12: 0,
            aux_irq_lev: 0,
            aux_irq_hint: 0,
            semaphore: 0,
            debug: 0,
            macmode: 0,
            mac_acc: [0; 2],
            timers: [Timer::reset(); 2],
            irq_lines: 0,
            delay_target: None,
            sleeping: false,
            cycles: 0,
            instructions_retired: 0,
            ready: vec![0; 64],
            ready_max: 0,
            config,
            diag: Diagnostics::default(),
            xy: None,
            icache: icache::ICache::default(),
            last_instr: None,
            record_last: true,
        };
        cpu.reset();
        cpu
    }

    /// Reset (PRM Figure 83): E1 = E2 = 0, ZNCV = 0, SEMAPHORE = 0,
    /// DEBUG = 0, LP_COUNT/LP_START/LP_END = 0, INT_VECTOR_BASE = configured
    /// base, PC = INT_VECTOR_BASE. Core registers are not defined by the PRM
    /// after reset; Ribbit zeroes them for determinism. AUX_IRQ_LEV resets to
    /// the default levels (IRQ6/IRQ7 level 2, PRM Table 23).
    pub fn reset(&mut self) {
        let config = self.config.clone();
        // ROM contents survive a reset, so the decode cache stays valid.
        let icache = std::mem::take(&mut self.icache);
        *self = Self {
            regs: [0; NUM_CORE_REGS],
            pc: config.reset_vector_base,
            status32: 0,
            status32_l1: 0,
            status32_l2: 0,
            lp_start: 0,
            lp_end: 0,
            int_vector_base: config.reset_vector_base,
            aux_irq_lv12: 0,
            aux_irq_lev: (1 << 6) | (1 << 7),
            aux_irq_hint: 0,
            semaphore: 0,
            debug: 0,
            macmode: 0,
            mac_acc: [0; 2],
            timers: [Timer::reset(); 2],
            irq_lines: 0,
            delay_target: None,
            sleeping: false,
            cycles: 0,
            instructions_retired: 0,
            ready: vec![0; 64],
            ready_max: 0,
            xy: config.xy_extension.then(xy::XyExtension::default),
            icache,
            last_instr: None,
            record_last: self.record_last,
            config,
            diag: Diagnostics::default(),
        };
    }

    pub fn halted(&self) -> bool {
        self.status32 & status32::H != 0
    }

    pub fn flag(&self, bit: u32) -> bool {
        self.status32 & bit != 0
    }

    /// Sets or clears an external interrupt input (IRQ3-IRQ31).
    /// Sets every line in `mask` at once: asserted where `levels` has a 1.
    pub fn set_irq_lines(&mut self, mask: u32, levels: u32) {
        let mask = mask & 0xFFFF_FFF8;
        self.irq_lines = (self.irq_lines & !mask) | (levels & mask);
    }

    pub fn set_irq_line(&mut self, irq: u8, asserted: bool) {
        if (3..32).contains(&irq) {
            if asserted {
                self.irq_lines |= 1 << irq;
            } else {
                self.irq_lines &= !(1 << irq);
            }
        }
    }

    /// Executes one instruction (or takes one interrupt) with no
    /// machine-specific auxiliary registers.
    pub fn step(&mut self, bus: &mut impl ribbit_memory::Bus) -> Result<Step, CpuError> {
        self.step_with(bus, &mut NoAuxExt)
    }
}

impl fmt::Debug for Cpu {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "pc={:#010x} status32={:#010x} cycles={} retired={}{}",
            self.pc,
            self.status32,
            self.cycles,
            self.instructions_retired,
            if self.sleeping { " SLEEPING" } else { "" }
        )?;
        let flags = [(status32::Z, 'Z'), (status32::N, 'N'), (status32::C, 'C'), (status32::V, 'V')];
        let flags: String = flags.iter().map(|&(b, c)| if self.flag(b) { c } else { '-' }).collect();
        writeln!(
            f,
            "flags={flags} E1={} E2={} H={} lp_start={:#010x} lp_end={:#010x} lp_count={:#x}",
            self.flag(status32::E1) as u8,
            self.flag(status32::E2) as u8,
            self.halted() as u8,
            self.lp_start,
            self.lp_end,
            self.regs[reg::LP_COUNT]
        )?;
        for (row, regs) in self.regs[..32].chunks(8).enumerate() {
            let cells: Vec<String> = regs
                .iter()
                .enumerate()
                .map(|(j, r)| format!("{:>6}={r:08x}", isa::reg_name((row * 8 + j) as u8)))
                .collect();
            writeln!(f, "{}", cells.join(" "))?;
        }
        writeln!(
            f,
            "mlo={:08x} mmid={:08x} mhi={:08x} ilink1->{:#010x} ilink2->{:#010x}",
            self.regs[reg::MLO],
            self.regs[reg::MMID],
            self.regs[reg::MHI],
            self.regs[reg::ILINK1],
            self.regs[reg::ILINK2]
        )
    }
}
