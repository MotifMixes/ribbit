//! x86-64 code generation (dynasm-rs).
//!
//! The generated function is `extern "sysv64" fn(*mut JitCtx) -> u32` (the
//! `ExitKind`). Register use, fixed for the whole function:
//!
//! | Host | Holds |
//! |---|---|
//! | `r12` | the `JitCtx` |
//! | `rbx` | `&regs[0]` (guest rN at `[rbx + 4*N]`; STATUS32 and `ready_max`, fields of the same `Cpu`, at fixed offsets) |
//! | `r13` | `&ready[0]` (the scoreboard) |
//! | `rbp` | the cycle count (`now`: the start of the instruction running) |
//! | `r14d` | a taken branch's target while its delay slot runs |
//! | `r15d` | 1 if that branch was taken, 0 if not |
//!
//! All six are callee-saved in the System V ABI, so they survive the
//! helpers; everything else is scratch. Each instruction is emitted from its
//! `Fast` form exactly as `Cpu::execute` (the general executor) computes it,
//! and timed exactly as `Cpu::run_block` times it (see `Gen::instr`);
//! tests/lockstep.rs holds the two side by side.

use crate::JitCtx;
use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use ribbit_cpu::icache::{AluOp, DISCARD, Fast, Hot, LdKind, RotOp, UnOp};
use ribbit_cpu::isa::{BrCond, COND_AL, Cond, Dst, Instr, NO_LOAD, Op, Size, Src, WriteBack, cond_holds};
use ribbit_cpu::reg;
use std::mem::offset_of;

type Asm = VecAssembler<X64Relocation>;

const BUS: i32 = offset_of!(JitCtx, bus) as i32;
const READ8: i32 = offset_of!(JitCtx, read8) as i32;
const READ16: i32 = offset_of!(JitCtx, read16) as i32;
const READ32: i32 = offset_of!(JitCtx, read32) as i32;
const WRITE8: i32 = offset_of!(JitCtx, write8) as i32;
const WRITE16: i32 = offset_of!(JitCtx, write16) as i32;
const WRITE32: i32 = offset_of!(JitCtx, write32) as i32;
const PREP: i32 = offset_of!(JitCtx, prep) as i32;
const TAKE: i32 = offset_of!(JitCtx, take) as i32;
const FETCH_TAKE: i32 = offset_of!(JitCtx, fetch_take) as i32;
const LP_START: i32 = offset_of!(JitCtx, lp_start) as i32;
const LP_END: i32 = offset_of!(JitCtx, lp_end) as i32;
const PC: i32 = offset_of!(JitCtx, pc) as i32;
const EXECUTED: i32 = offset_of!(JitCtx, executed) as i32;
const FROM: i32 = offset_of!(JitCtx, from) as i32;
const DELAY_PENDING: i32 = offset_of!(JitCtx, delay_pending) as i32;
const DELAY_TARGET: i32 = offset_of!(JitCtx, delay_target) as i32;
const REGS: i32 = offset_of!(JitCtx, regs) as i32;
const READY: i32 = offset_of!(JitCtx, ready) as i32;
const NOW: i32 = offset_of!(JitCtx, now) as i32;
const END: i32 = offset_of!(JitCtx, end) as i32;

const FENCE: i32 = offset_of!(JitCtx, fence) as i32;
const DISPATCH: i32 = offset_of!(JitCtx, dispatch) as i32;
const DIRTY4: i32 = offset_of!(JitCtx, dirty4) as i32;
const CODE_TAGS: i32 = offset_of!(JitCtx, code_tags) as i32;
const DATA_TAGS: i32 = offset_of!(JitCtx, data_tags) as i32;
const PAGES: i32 = offset_of!(JitCtx, pages) as i32;
/// Where `JitCtx::pages` keeps the write pointers.
const WRITE_PAGES: i32 = (crate::full::PAGES * 8) as i32;
const TIMED_READ8: i32 = offset_of!(JitCtx, timed_read8) as i32;
const TIMED_READ16: i32 = offset_of!(JitCtx, timed_read16) as i32;
const TIMED_READ32: i32 = offset_of!(JitCtx, timed_read32) as i32;
const TIMED_WRITE8: i32 = offset_of!(JitCtx, timed_write8) as i32;
const TIMED_WRITE16: i32 = offset_of!(JitCtx, timed_write16) as i32;
const TIMED_WRITE32: i32 = offset_of!(JitCtx, timed_write32) as i32;
/// `Cpu::status32` and `Cpu::ready_max` from `rbx` (`&Cpu::regs[0]`): the
/// context's pointers are always into one `Cpu` (`JitCtx::new`).
const ST: i32 = offset_of!(ribbit_cpu::Cpu, status32) as i32 - offset_of!(ribbit_cpu::Cpu, regs) as i32;
const RMAX: i32 = offset_of!(ribbit_cpu::Cpu, ready_max) as i32 - offset_of!(ribbit_cpu::Cpu, regs) as i32;
/// Cache tag bits (`ribbit_memory::JitView`).
const TAG_VALID: u32 = 1 << 31;
const TAG_DIRTY: u32 = 1 << 30;
const CACHE_SETS: u32 = 512;
const WATCH: i32 = offset_of!(JitCtx, watch) as i32;
const WATCH_DATA: i32 = offset_of!(JitCtx, watch_data) as i32;
const LINK: i32 = offset_of!(JitCtx, link) as i32;
const PATCH: i32 = offset_of!(JitCtx, patch) as i32;
const FB_BLOCK: i32 = offset_of!(JitCtx, fb_block) as i32;
const FB_INDEX: i32 = offset_of!(JitCtx, fb_index) as i32;

const EXIT_FLOW: i32 = crate::ExitKind::Flow as i32;
const EXIT_FALLBACK: i32 = crate::ExitKind::Fallback as i32;
const EXIT_BUS: i32 = crate::ExitKind::Bus as i32;
const EXIT_BACKWARD: i32 = crate::ExitKind::Backward as i32;
const EXIT_BUDGET: i32 = crate::ExitKind::Budget as i32;

/// `prep`'s third argument: the fetch is timed in full (else the line is
/// the one just fetched).
pub(crate) const PREP_FETCH: u32 = 0x100;

/// Displacement of guest core register `r` (r0-r31) from `rbx`.
fn r(n: u8) -> i32 {
    4 * (n & 31) as i32
}

const SP: i32 = 4 * reg::SP as i32;
const BLINK: i32 = 4 * reg::BLINK as i32;
const LP_COUNT: i32 = 4 * reg::LP_COUNT as i32;
const MLO: i32 = 4 * reg::MLO as i32;
const MMID: i32 = 4 * reg::MMID as i32;
const MHI: i32 = 4 * reg::MHI as i32;

/// Status bit positions.
const V_BIT: i8 = 8;
const C_BIT: i8 = 9;
const N_BIT: i8 = 10;
const Z_BIT: i8 = 11;

/// Which flags an operation writes.
#[derive(Clone, Copy)]
enum Writes {
    /// Z, N, C, V.
    All,
    /// Z, N.
    Zn,
    /// Z, N, C (shifts by a nonzero amount, rotates).
    Znc,
}

/// Where an exit's PC comes from.
#[derive(Clone, Copy)]
enum Pc {
    Const(u32),
    /// `r14d` (a taken branch's target).
    R14,
}

/// One instruction to compile: its pre-digested form, whether its fetch
/// is timed in full (`icache::Entry::fetch`), and its full decode when it
/// has no fast form (`Fast::General`; see `generic_ok`).
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub hot: Hot,
    pub fetch: bool,
    pub instr: Option<Instr>,
}

impl Item {
    /// Reads or writes memory (prepared and timed as such).
    fn mem(&self) -> bool {
        self.hot.fast.accesses_memory() || self.general().is_some_and(|i| matches!(i.op, Op::Ld | Op::St))
    }

    /// The full decode, for an instruction compiled from it.
    fn general(&self) -> Option<&Instr> {
        if self.hot.fast == Fast::General { self.instr.as_ref() } else { None }
    }
}

/// A source the generic code reads like `Cpu::read`: a constant (an
/// immediate, a limm, no operand, PCL) or a register with no side effect.
#[derive(Clone, Copy)]
enum GSrc {
    Imm(u32),
    Reg(u8),
}

fn gsrc(s: Src, i: &Instr) -> Option<GSrc> {
    match s {
        Src::None => Some(GSrc::Imm(0)),
        Src::Imm(v) | Src::Limm(v) => Some(GSrc::Imm(v)),
        // r57-r60 (MLO, MMID, MHI, LP_COUNT) read plainly, with or without
        // the XY extension (`Cpu::read_reg_slow`); r63 is PCL.
        Src::Reg(r) if r < 32 || (57..=60).contains(&r) => Some(GSrc::Reg(r)),
        Src::Reg(63) => Some(GSrc::Imm(i.addr & !3)),
        Src::Reg(_) => None,
    }
}

/// A destination written like `Cpu::write_dst` with no side effect: none,
/// or r0-r31, LP_COUNT.
fn gdst(d: Dst) -> Option<Option<u8>> {
    match d {
        Dst::None | Dst::Discard => Some(None),
        Dst::Reg(r) if r < 32 || r == 60 => Some(Some(r)),
        Dst::Reg(_) => None,
    }
}

/// `AluOp` for an executor op (`icache::AluOp::of`).
fn alu_op(op: Op) -> Option<AluOp> {
    Some(match op {
        Op::Add => AluOp::Add,
        Op::Add1 => AluOp::Add1,
        Op::Add2 => AluOp::Add2,
        Op::Add3 => AluOp::Add3,
        Op::Sub => AluOp::Sub,
        Op::Sub1 => AluOp::Sub1,
        Op::Sub2 => AluOp::Sub2,
        Op::Sub3 => AluOp::Sub3,
        Op::Rsub => AluOp::Rsub,
        Op::And => AluOp::And,
        Op::Or => AluOp::Or,
        Op::Xor => AluOp::Xor,
        Op::Bic => AluOp::Bic,
        Op::AslM => AluOp::Asl,
        Op::LsrM => AluOp::Lsr,
        Op::AsrM => AluOp::Asr,
        Op::Adc => AluOp::Adc,
        Op::Sbc => AluOp::Sbc,
        _ => return None,
    })
}

/// Whether `generic` compiles `i` (a `Fast::General` instruction): the
/// shapes of `Cpu::execute`'s ALU part, compares, bit operations,
/// single-operand operations and loads/stores, under any condition, with
/// operands and destinations that have no side effect.
pub(crate) fn generic_ok(i: &Instr) -> bool {
    let srcs = gsrc(i.src1, i).is_some() && gsrc(i.src2, i).is_some();
    let Some(d) = gdst(i.dst) else {
        return false;
    };
    match i.op {
        op if alu_op(op).is_some() => srcs,
        Op::Mov | Op::Bset | Op::Bclr | Op::Bxor | Op::Bmsk | Op::RorM => srcs,
        Op::Cmp | Op::Rcmp | Op::Tst | Op::Btst => srcs,
        Op::Asl | Op::Asr | Op::Lsr | Op::Ror | Op::Rrc | Op::Rlc => srcs,
        Op::Sexb | Op::Sexw | Op::Extb | Op::Extw | Op::Not | Op::Neg | Op::Swap | Op::Abs | Op::Norm => srcs,
        Op::Max | Op::Min | Op::Normw => srcs,
        // A zero-overhead loop's set-up (`Gen::lp`).
        Op::Lp => true,
        Op::Ld | Op::St => {
            // The base's write-back must be a plain register too, and a load
            // writing back into its own destination is an error the executor
            // reports.
            let wb = matches!(i.mem.wb, WriteBack::Pre | WriteBack::Post);
            let base_ok = !wb || match i.src1 {
                Src::Reg(r) => gdst(Dst::Reg(r)).is_some_and(|b| b.is_some()),
                _ => true,
            };
            let own = matches!((i.dst, i.src1), (Dst::Reg(a), Src::Reg(b)) if a == b) && wb && i.op == Op::Ld;
            let data_ok = i.op == Op::Ld || gsrc(i.data, i).is_some();
            srcs && base_ok && !own && data_ok && (i.op == Op::St || d.is_some() || i.dst == Dst::Discard)
        }
        _ => false,
    }
}

fn is_branch(f: Fast) -> bool {
    matches!(
        f,
        Fast::Br { .. }
            | Fast::BrR { .. }
            | Fast::Bbit { .. }
            | Fast::BbitI { .. }
            | Fast::B { .. }
            | Fast::Bl { .. }
            | Fast::BlD { .. }
            | Fast::J { .. }
            | Fast::Jcc { .. }
            | Fast::Jl { .. }
            | Fast::JlD { .. }
    )
}

/// Compiled by this generator (everything else falls back).
pub(crate) fn compilable(it: &Item) -> bool {
    !it.hot.stop && (it.hot.fast != Fast::General || it.instr.as_ref().is_some_and(generic_ok))
}

/// For each condition code, bit `i` says whether it holds for flags
/// `i = V | C << 1 | N << 2 | Z << 3` (STATUS32 bits 8-11), from the
/// interpreter's own `cond_holds`.
fn cond_table(cc: Cond) -> u32 {
    (0..16u32).filter(|&i| cond_holds(cc, i & 8 != 0, i & 4 != 0, i & 2 != 0, i & 1 != 0)).fold(0, |t, i| t | 1 << i)
}

/// An exit taken from the middle of the code (out of line).
struct Stub {
    label: DynamicLabel,
    kind: i32,
    pc: Pc,
    executed: u32,
    from: u32,
    /// A taken delayed branch whose slot does not run here (target in r14d).
    delay: bool,
}

/// A zero-overhead loop end after a sequential instruction (out of line).
struct LoopStub {
    label: DynamicLabel,
    /// Back in the main path when the loop is over (LP_COUNT was 1).
    stay: DynamicLabel,
    executed: u32,
    from: u32,
    /// The instruction accessed memory: the bus's exit request (r8d) is
    /// looked at first.
    mem: bool,
}

struct Gen {
    ops: Asm,
    epilogue: DynamicLabel,
    loops: Vec<LoopStub>,
    stubs: Vec<Stub>,
    /// MUL64 leaves MLO alone and puts the low word in MMID (`MmidMode::LowLeapster`).
    mmid_low: bool,
    /// The block's id (fallback exits report it).
    id: u32,
    /// Link slots (`crate::Link`): `links_cap` of them from `links`.
    links: *mut crate::Link,
    links_cap: usize,
    links_used: usize,
    /// Memory and its timing done in the code (`ribbit_memory::JitView`),
    /// else every access and timing step through the bus's methods.
    inline: bool,
    /// The instruction being emitted (for the slow helpers' `at`).
    pc: u32,
    /// Research (RIBBIT_JIT_PERFMAP): where each kind of code starts.
    tags: Vec<(u32, u8)>,
    /// The instruction being emitted, counted from the block's first (0).
    seq: u32,
    /// The block's loads so far: (`seq`, destination register).
    loads: Vec<(u32, u8)>,
    /// `CpuConfig::load_latency` (the code is dropped if it changes).
    latency: u32,
    /// Memory timing is on (`JitView::timing`; the code is dropped if it
    /// changes): the inline code times fetches and data accesses.
    timing: bool,
    /// The batch fence can stop a link (`JitCtx::fence`; else it is
    /// `u32::MAX` and no block reaches it).
    fence_check: bool,
    /// The last `stall` is known to be zero (rax not set: `cost` adds nothing).
    stall_zero: bool,
}

/// The displacement of the direct jump whose 32-bit displacement is at `at`
/// (see `Gen::flow`) pointed at `target`, or None if out of reach.
pub(crate) fn link_patch(at: u64, target: u64) -> Option<[u8; 4]> {
    let rel = (target as i64).wrapping_sub(at as i64 + 4);
    i32::try_from(rel).ok().map(i32::to_le_bytes)
}

/// What a stretch of generated code does (`Emitted::tags`, for profiles).
pub(crate) mod tag {
    pub const ENTRY: u8 = 0;
    pub const BODY: u8 = 1;
    pub const COST: u8 = 2;
    pub const LOOP_CHECK: u8 = 3;
    pub const BUDGET: u8 = 4;
    pub const BUS_CHECK: u8 = 5;
    pub const PREP: u8 = 6;
    pub const STALL: u8 = 7;
    pub const REGION: u8 = 8;
    pub const MEM: u8 = 9;
    pub const FLOW: u8 = 10;
    pub const WATCH: u8 = 11;
    pub const STUB: u8 = 12;
    pub const LOOP_STUB: u8 = 13;
    pub const BRANCH: u8 = 14;
}

/// Generated code.
pub(crate) struct Emitted {
    pub code: Vec<u8>,
    /// Instructions covered.
    pub compiled: usize,
    /// Offset of the entry for a jump from another block (after the
    /// prologue: the registers are set up already).
    pub chain: usize,
    /// (offset, `tag`) where each kind of code starts.
    pub tags: Vec<(u32, u8)>,
}

/// Generates the code for `items` (block `id`, link slots at `links`; loads
/// arrive `latency` cycles after they issue), or None if the first
/// instruction cannot be compiled.
pub(crate) fn emit(items: &[Item], id: u32, links: *mut crate::Link, links_cap: usize, o: &crate::full::Opts) -> Option<Emitted> {
    let first = items.first()?;
    if !compilable(first) || matches!(first.hot.fast, Fast::BlD { .. } | Fast::JlD { .. }) && items.len() < 2 {
        return None;
    }
    let mut ops = Asm::new(0);
    let epilogue = ops.new_dynamic_label();
    let mut g = Gen {
        ops,
        epilogue,
        loops: Vec::new(),
        stubs: Vec::new(),
        mmid_low: o.mmid_low,
        id,
        links,
        links_cap,
        links_used: 0,
        inline: o.inline,
        pc: 0,
        tags: Vec::new(),
        seq: 0,
        loads: Vec::new(),
        latency: o.latency.min(u32::MAX as u64) as u32,
        timing: o.timing,
        fence_check: o.fence_check,
        stall_zero: false,
    };
    g.prologue();
    let chain = g.ops.offset().0;
    let mut k = 0;
    let compiled = loop {
        let Some(it) = items.get(k) else {
            // Off the end of the run (the last instruction's checks are done).
            let last = items[k - 1].hot;
            g.flow(Pc::Const(last.next), k as u32, last.addr);
            break k;
        };
        if !compilable(it) {
            g.fallback(it.hot.addr, k as u32, items[k - 1].hot.addr);
            break k;
        }
        g.seq = k as u32;
        if is_branch(it.hot.fast) {
            break g.branch(items, k);
        }
        if it.general().is_some_and(|i| i.op == Op::Lp) {
            break g.lp(items, k);
        }
        g.instr(it);
        let h = it.hot;
        g.after_sequential(h.next, k as u32 + 1, h.addr, it.mem());
        k += 1;
    };
    g.epilogue();
    let (ops, tags) = (g.ops, g.tags);
    let code = ops.finalize().ok()?;
    Some(Emitted { code, compiled, chain, tags })
}

impl Gen {
    fn tag(&mut self, t: u8) {
        let at = self.ops.offset().0 as u32;
        match self.tags.last_mut() {
            Some(last) if last.0 == at => last.1 = t,
            _ => self.tags.push((at, t)),
        }
    }

    fn prologue(&mut self) {
        self.tag(tag::ENTRY);
        // Six pushes and 8 bytes after the return address leave rsp 16-byte
        // aligned for the helper calls.
        dynasm!(self.ops
            ; .arch x64
            ; push rbx
            ; push rbp
            ; push r12
            ; push r13
            ; push r14
            ; push r15
            ; sub rsp, 8
            ; mov r12, rdi
            ; mov rbx, QWORD [r12 + REGS]
            ; mov r13, QWORD [r12 + READY]
            ; mov rbp, QWORD [r12 + NOW]
            // The instruction's slow-path stall (bit 63: the bus asked for
            // an exit), zero between instructions (`stall_inline`).
            ; mov QWORD [rsp], 0
        );
    }

    fn epilogue(&mut self) {
        let epilogue = self.epilogue;
        self.tag(tag::ENTRY);
        dynasm!(self.ops
            ; .arch x64
            ; =>epilogue
            ; mov QWORD [r12 + NOW], rbp
            ; add rsp, 8
            ; pop r15
            ; pop r14
            ; pop r13
            ; pop r12
            ; pop rbp
            ; pop rbx
            ; ret
        );
        // Out-of-line code (which can add more).
        loop {
            if let Some(l) = self.loops.pop() {
                // The interpreter's `loop_end_check`: LP_COUNT -= 1; back to
                // LP_START unless it was 1.
                self.tag(tag::LOOP_STUB);
                dynasm!(self.ops
                    ; .arch x64
                    ; =>l.label
                    ; mov eax, DWORD [rbx + LP_COUNT]
                    ; lea ecx, [rax - 1]
                    ; mov DWORD [rbx + LP_COUNT], ecx
                    ; cmp eax, 1
                    ; je =>l.stay
                    ; mov r14d, DWORD [r12 + LP_START]
                );
                if l.mem {
                    let stub = self.stub(EXIT_BUS, Pc::R14, l.executed, l.from, false);
                    dynasm!(self.ops ; .arch x64 ; test r8d, r8d ; jnz =>stub);
                }
                self.flow(Pc::R14, l.executed, l.from);
            } else if let Some(s) = self.stubs.pop() {
                self.tag(tag::STUB);
                dynasm!(self.ops ; .arch x64 ; =>s.label);
                if s.delay {
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov DWORD [r12 + DELAY_PENDING], 1
                        ; mov DWORD [r12 + DELAY_TARGET], r14d
                    );
                }
                self.exit(s.kind, s.pc, s.executed, s.from);
            } else {
                break;
            }
        }
    }

    /// Writes the exit's PC, adds the block's instructions to the count,
    /// writes the last instruction's address, and returns `kind` (the
    /// epilogue writes the cycle count).
    fn exit(&mut self, kind: i32, pc: Pc, executed: u32, from: u32) {
        match pc {
            Pc::Const(v) => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + PC], v as i32),
            Pc::R14 => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + PC], r14d),
        }
        let epilogue = self.epilogue;
        dynasm!(self.ops
            ; .arch x64
            ; add DWORD [r12 + EXECUTED], executed as i32
            ; mov DWORD [r12 + FROM], from as i32
            ; mov eax, kind
            ; jmp =>epilogue
        );
    }

    /// The instruction at `pc` is left to the interpreter (the block's
    /// `executed`-th).
    fn fallback(&mut self, pc: u32, executed: u32, from: u32) {
        let id = self.id;
        dynasm!(self.ops
            ; .arch x64
            ; mov DWORD [r12 + FB_BLOCK], id as i32
            ; mov DWORD [r12 + FB_INDEX], executed as i32
        );
        self.exit(EXIT_FALLBACK, Pc::Const(pc), executed, from);
    }

    /// An out-of-line exit to jump to.
    fn stub(&mut self, kind: i32, pc: Pc, executed: u32, from: u32, delay: bool) -> DynamicLabel {
        let label = self.ops.new_dynamic_label();
        self.stubs.push(Stub { label, kind, pc, executed, from, delay });
        label
    }

    /// Stops once the budget is reached (`run_block`'s `spent >= budget`
    /// after an instruction).
    fn budget_check(&mut self, pc: Pc, executed: u32, from: u32, delay: bool) {
        let stub = self.stub(EXIT_BUDGET, pc, executed, from, delay);
        self.tag(tag::BUDGET);
        dynasm!(self.ops
            ; .arch x64
            ; cmp rbp, QWORD [r12 + END]
            ; jae =>stub
        );
    }

    /// After a sequential instruction (`next`), as `batch_one` and
    /// `run_block` check it: the zero-overhead loop (`Cpu::loop_end_check`,
    /// out of line), a device access's exit request (r8d, for a memory
    /// instruction), the budget.
    fn after_sequential(&mut self, next: u32, executed: u32, from: u32, mem: bool) {
        let (label, stay) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        self.loops.push(LoopStub { label, stay, executed, from, mem });
        self.tag(tag::LOOP_CHECK);
        dynasm!(self.ops
            ; .arch x64
            ; cmp DWORD [r12 + LP_END], next as i32
            ; je =>label
            ; =>stay
        );
        if mem {
            self.tag(tag::BUS_CHECK);
            let stub = self.stub(EXIT_BUS, Pc::Const(next), executed, from, false);
            dynasm!(self.ops ; .arch x64 ; test r8d, r8d ; jnz =>stub);
        }
        self.budget_check(Pc::Const(next), executed, from, false);
    }

    /// The flow goes on at `target` (a constant, or r14d) after the
    /// instruction at `from` (the block's `executed`-th): on a backward jump
    /// the idle-loop detector is asked (`watch`; it may end the batch),
    /// then the budget; then the block linked there runs next (`crate::Link`,
    /// filled by `Jit` on an earlier exit from here), else the code returns.
    fn flow(&mut self, target: Pc, executed: u32, from: u32) {
        self.tag(tag::FLOW);
        match target {
            Pc::Const(t) => {
                if t <= from {
                    self.watch(target, executed, from);
                }
            }
            Pc::R14 => {
                let forward = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch x64 ; cmp r14d, from as i32 ; ja =>forward);
                self.watch(target, executed, from);
                dynasm!(self.ops ; .arch x64 ; =>forward);
            }
        }
        self.budget_check(target, executed, from, false);
        self.tag(tag::FLOW);
        if self.links_used == self.links_cap {
            self.exit(EXIT_FLOW, target, executed, from);
            return;
        }
        // SAFETY: within the slots the caller allocated (checked above).
        let link = unsafe { self.links.add(self.links_used) } as i64;
        self.links_used += 1;
        let miss = self.ops.new_dynamic_label();
        if let (Pc::Const(_), false) = (target, self.fence_check)
            && self.inline
        {
            // A jump the `Jit` points at the next block once it is known
            // (`JitCtx::patch`); until then (and if it is out of reach) it
            // falls through to the link slot.
            let rel = self.ops.new_dynamic_label();
            dynasm!(self.ops
                ; .arch x64
                ; add DWORD [r12 + EXECUTED], executed as i32
                ; .u8 0xE9
                ; =>rel
                ; .u32 0
                ; mov rax, QWORD link
                ; mov rdx, QWORD [rax + 8]
                ; test rdx, rdx
                ; jz =>miss
                ; jmp rdx
                ; =>miss
                ; sub DWORD [r12 + EXECUTED], executed as i32
                ; mov QWORD [r12 + LINK], rax
                ; lea rcx, [=>rel]
                ; mov QWORD [r12 + PATCH], rcx
            );
            self.exit(EXIT_FLOW, target, executed, from);
            return;
        }
        dynasm!(self.ops ; .arch x64 ; mov rax, QWORD link);
        if let Pc::R14 = target {
            // A computed target: this exit's last one, else the shared
            // table (`crate::DISPATCH_SIZE` slots by address).
            let cached = self.ops.new_dynamic_label();
            dynasm!(self.ops
                ; .arch x64
                ; cmp r14d, DWORD [rax]
                ; je =>cached
                ; mov ecx, r14d
                ; shr ecx, 1
                ; and ecx, (crate::DISPATCH_SIZE - 1) as i32
                ; shl rcx, 4
                ; add rcx, QWORD [r12 + DISPATCH]
                ; cmp r14d, DWORD [rcx]
                ; jne =>miss
            );
            if self.fence_check {
                dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rcx + 4] ; cmp edx, DWORD [r12 + FENCE] ; jae =>miss);
            }
            dynasm!(self.ops
                ; .arch x64
                ; mov rdx, QWORD [rcx + 8]
                ; test rdx, rdx
                ; jz =>miss
                ; add DWORD [r12 + EXECUTED], executed as i32
                ; jmp rdx
                ; =>cached
            );
        }
        if self.fence_check {
            dynasm!(self.ops ; .arch x64 ; mov ecx, DWORD [rax + 4] ; cmp ecx, DWORD [r12 + FENCE] ; jae =>miss);
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov rdx, QWORD [rax + 8]
            ; test rdx, rdx
            ; jz =>miss
            ; add DWORD [r12 + EXECUTED], executed as i32
            ; jmp rdx
            ; =>miss
            ; mov QWORD [r12 + LINK], rax
            ; mov QWORD [r12 + PATCH], 0
        );
        self.exit(EXIT_FLOW, target, executed, from);
    }

    /// `watch(target, now)` (the idle-loop detector); ends the batch when
    /// it accepts.
    fn watch(&mut self, target: Pc, executed: u32, from: u32) {
        let stub = self.stub(EXIT_BACKWARD, target, executed, from, false);
        self.tag(tag::WATCH);
        dynasm!(self.ops ; .arch x64 ; mov rdi, QWORD [r12 + WATCH_DATA]);
        match target {
            Pc::Const(t) => dynasm!(self.ops ; .arch x64 ; mov esi, t as i32),
            Pc::R14 => dynasm!(self.ops ; .arch x64 ; mov esi, r14d),
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov rdx, rbp
            ; call QWORD [r12 + WATCH]
            ; test eax, eax
            ; jnz =>stub
        );
        self.tag(tag::FLOW);
    }

    /// A non-branch instruction with its timing, as `Cpu::batch_one` runs
    /// it: the fetch (`Bus::instruction`, or only arming the timing when it
    /// is on the line just fetched), the bus's cycle and PC before a memory
    /// access (`Bus::at`), the instruction, its memory stall
    /// (`Bus::take_stall`) and `pipeline_cost`. An instruction that touches
    /// no memory and is on the line just fetched makes no call: arming and
    /// taking the stall would change nothing (nothing accrues between, and
    /// the timing is disarmed and the stall zero between instructions).
    /// For a memory access, r8d is left 1 if the bus asked for an exit.
    fn instr(&mut self, it: &Item) {
        let h = it.hot;
        self.pc = h.addr;
        let mem = it.mem();
        self.tag(tag::PREP);
        if mem {
            if self.inline {
                self.prep_inline(&h, it.fetch);
            } else {
                self.prep(&h, it.fetch);
            }
        } else if it.general().is_some() && !self.inline {
            // `run_block` stamps the bus before every general instruction
            // (`at`; only seen by a recording bus when it touches no device).
            self.prep(&h, it.fetch);
        }
        self.tag(tag::BODY);
        match it.general() {
            Some(i) => self.generic(i),
            None => self.body(&h),
        }
        self.stall(&h, it.fetch, mem);
        self.cost(&h, 0);
    }

    /// `Bus::instruction` (or `instruction_same_line`) and `Bus::at`.
    fn prep(&mut self, h: &Hot, fetch: bool) {
        let len = h.next.wrapping_sub(h.addr) | if fetch { PREP_FETCH } else { 0 };
        dynasm!(self.ops
            ; .arch x64
            ; mov rdi, QWORD [r12 + BUS]
            ; mov esi, h.addr as i32
            ; mov edx, len as i32
            ; mov rcx, rbp
            ; call QWORD [r12 + PREP]
        );
    }

    /// `rax` = the instruction's memory stall (after `prep` and the body).
    fn stall(&mut self, h: &Hot, fetch: bool, mem: bool) {
        self.tag(tag::STALL);
        self.stall_zero = false;
        if self.inline {
            return self.stall_inline(h, fetch, mem);
        }
        // A general instruction was prepared (`prep`) like a memory one.
        let mem = mem || h.fast == Fast::General;
        if mem {
            dynasm!(self.ops
                ; .arch x64
                ; mov rdi, QWORD [r12 + BUS]
                ; call QWORD [r12 + TAKE]
                ; mov r8, rax
                ; shr r8, 63
                ; btr rax, 63
            );
        } else if fetch {
            let len = h.next.wrapping_sub(h.addr);
            dynasm!(self.ops
                ; .arch x64
                ; mov rdi, QWORD [r12 + BUS]
                ; mov esi, h.addr as i32
                ; mov edx, len as i32
                ; call QWORD [r12 + FETCH_TAKE]
            );
        } else {
            self.stall_zero = true;
        }
    }

    /// `Cpu::pipeline_cost` with the stall in `rax`: waits for registers
    /// read whose loads have not arrived, records a load's arrival, and
    /// advances `rbp` by 1 + wait + stall + `extra` (the taken-branch costs).
    /// Which registers' arrival times can delay it are known here: a load
    /// at instruction `j` arrives at most `latency - 1` cycles after its
    /// own end, and each instruction takes at least one cycle, so the
    /// instruction `k` waits for it only if `k - j < latency`. Loads from
    /// before the block (`j <= -1`) matter only to the first `latency - 1`
    /// instructions, which check every register they read; the others look
    /// only at the registers loaded by the block's last `latency - 1`
    /// instructions (usually none: no code at all).
    fn cost(&mut self, h: &Hot, extra: u64) {
        self.tag(tag::COST);
        let mut regs: Vec<u8> = h.reads.iter().copied().filter(|&r| r < 64).collect();
        regs.sort_unstable();
        regs.dedup();
        let (seq, latency) = (self.seq, self.latency);
        let early = seq.saturating_add(1) < latency;
        if !early {
            regs.retain(|&r| self.loads.iter().any(|&(j, d)| d == r && seq - j < latency));
        }
        if let Some((&first, rest)) = regs.split_first() {
            let done = self.ops.new_dynamic_label();
            if early {
                // Nothing pending at all: no wait.
                dynasm!(self.ops
                    ; .arch x64
                    ; xor ecx, ecx
                    ; cmp QWORD [rbx + RMAX], rbp
                    ; jbe =>done
                );
            }
            dynasm!(self.ops ; .arch x64 ; mov rcx, QWORD [r13 + 8 * first as i32]);
            for &r in rest {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov rsi, QWORD [r13 + 8 * r as i32]
                    ; cmp rcx, rsi
                    ; cmovb rcx, rsi
                );
            }
            // wait = latest - now, or 0 when it has arrived.
            dynasm!(self.ops
                ; .arch x64
                ; sub rcx, rbp
                ; jae =>done
                ; xor ecx, ecx
                ; =>done
            );
        }
        let wait_zero = regs.is_empty();
        let stall_zero = self.stall_zero;
        if h.load_dst != NO_LOAD {
            let d = 8 * h.load_dst as i32;
            let keep = self.ops.new_dynamic_label();
            // rsi = now + wait + stall + latency.
            match (wait_zero, stall_zero) {
                (true, true) => dynasm!(self.ops ; .arch x64 ; lea rsi, [rbp + latency as i32]),
                (true, false) => dynasm!(self.ops ; .arch x64 ; lea rsi, [rbp + rax + latency as i32]),
                (false, true) => dynasm!(self.ops ; .arch x64 ; lea rsi, [rbp + rcx + latency as i32]),
                (false, false) => dynasm!(self.ops ; .arch x64 ; lea rsi, [rbp + rcx + latency as i32] ; add rsi, rax),
            }
            dynasm!(self.ops
                ; .arch x64
                ; mov QWORD [r13 + d], rsi
                ; cmp QWORD [rbx + RMAX], rsi
                ; jae =>keep
                ; mov QWORD [rbx + RMAX], rsi
                ; =>keep
            );
            self.loads.push((seq, h.load_dst));
        }
        let base = (1 + extra) as i32;
        match (wait_zero, stall_zero) {
            (true, true) => dynasm!(self.ops ; .arch x64 ; add rbp, base),
            (true, false) => dynasm!(self.ops ; .arch x64 ; lea rbp, [rbp + rax + base]),
            (false, true) => dynasm!(self.ops ; .arch x64 ; lea rbp, [rbp + rcx + base]),
            (false, false) => dynasm!(self.ops ; .arch x64 ; add rbp, rax ; lea rbp, [rbp + rcx + base]),
        }
    }

    /// `prep` done in place (`ribbit_memory::JitView`) for a memory
    /// instruction: the fetch's code-cache lookup when it is timed in full.
    /// Nothing is armed: hits change no timing state (`MemTiming`), so only
    /// the slow paths arm and take the stall, each adding it to `[rsp]`:
    /// here a code-cache miss (the bus's `instruction` + `take_stall`), and
    /// the data access's helper (`timed_read*`/`timed_write*`).
    fn prep_inline(&mut self, h: &Hot, fetch: bool) {
        if !fetch || !self.timing {
            return;
        }
        let (hit, miss) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        self.code_lookup(h, hit, miss);
        let len = h.next.wrapping_sub(h.addr);
        dynasm!(self.ops
            ; .arch x64
            ; =>miss
            ; mov rdi, QWORD [r12 + BUS]
            ; mov esi, h.addr as i32
            ; mov edx, len as i32
            ; call QWORD [r12 + FETCH_TAKE]
            ; add QWORD [rsp], rax
            ; =>hit
        );
    }

    /// Jumps to `hit` if every 16-byte line the instruction's fetch touches
    /// is in the code cache, else to `miss` (rcx, edx clobbered).
    fn code_lookup(&mut self, h: &Hot, hit: DynamicLabel, miss: DynamicLabel) {
        let first = h.addr >> 4;
        let last = h.next.wrapping_sub(1) >> 4;
        dynasm!(self.ops ; .arch x64 ; mov rcx, QWORD [r12 + CODE_TAGS]);
        for line in [first, last].into_iter().take(if first == last { 1 } else { 2 }) {
            let set = ((line % CACHE_SETS) * 16) as i32;
            let found = self.ops.new_dynamic_label();
            dynasm!(self.ops
                ; .arch x64
                ; mov edx, (line | TAG_VALID) as i32
                ; cmp edx, DWORD [rcx + set]
                ; je =>found
                ; cmp edx, DWORD [rcx + set + 4]
                ; je =>found
                ; cmp edx, DWORD [rcx + set + 8]
                ; je =>found
                ; cmp edx, DWORD [rcx + set + 12]
                ; jne =>miss
                ; =>found
            );
        }
        dynasm!(self.ops ; .arch x64 ; jmp =>hit);
    }

    /// `stall` done in place: `take_stall` (and `block_exit` into r8d) for a
    /// memory instruction, the code-cache lookup for one with its fetch
    /// timed in full (the bus's `instruction` + `take_stall` on a miss).
    fn stall_inline(&mut self, h: &Hot, fetch: bool, mem: bool) {
        if mem {
            // What the slow paths added (`prep_inline`); the exit request
            // can only come from one of them (a batch starts with it clear,
            // and only bus accesses set it).
            dynasm!(self.ops
                ; .arch x64
                ; mov rax, QWORD [rsp]
                ; mov QWORD [rsp], 0
                ; mov r8, rax
                ; shr r8, 63
                ; btr rax, 63
            );
            return;
        }
        if !(fetch && self.timing) {
            self.stall_zero = true;
            return;
        }
        let (done, miss) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        dynasm!(self.ops ; .arch x64 ; xor eax, eax);
        self.code_lookup(h, done, miss);
        let len = h.next.wrapping_sub(h.addr);
        dynasm!(self.ops
            ; .arch x64
            ; =>miss
            ; mov rdi, QWORD [r12 + BUS]
            ; mov esi, h.addr as i32
            ; mov edx, len as i32
            ; call QWORD [r12 + FETCH_TAKE]
            ; =>done
        );
    }

    /// Finds the plain memory holding the access at `esi` (`width` bytes,
    /// aligned as the bus aligns it) by its page (`JitCtx::pages`): r11 = its
    /// host byte, then the data-cache lookup (dirtying the line for a write);
    /// jumps to `slow` when the page is not plain memory (or not writable,
    /// for a write), or the line misses.
    fn region_lookup(&mut self, width: u32, write: bool, slow: DynamicLabel) {
        let mask = !(width - 1);
        self.tag(tag::REGION);
        let table = if write { WRITE_PAGES } else { 0 };
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, esi
            ; and eax, mask as i32
            ; mov ecx, eax
            ; shr ecx, crate::full::PAGE_SHIFT as i8
            ; mov r11, QWORD [r12 + PAGES]
            ; mov r11, QWORD [r11 + rcx * 8 + table]
            ; test r11, r11
            ; jz =>slow
            ; movzx ecx, ax
            ; add r11, rcx
        );
        // The data cache (`MemTiming::data`), on the unaligned address: the
        // set's four ways compared at once (which way holds a line varies
        // from access to access, so a branch per way mispredicts), each
        // with its dirty bit set on both sides, so it is ignored.
        let done = self.ops.new_dynamic_label();
        if !self.timing {
            dynasm!(self.ops ; .arch x64 ; =>done);
            return;
        }
        dynasm!(self.ops
            ; .arch x64
            ; mov r9d, esi
            ; shr r9d, 4
            ; mov r10d, r9d
            ; and r10d, (CACHE_SETS - 1) as i32
            ; shl r10d, 4
            ; or r9d, (TAG_VALID | TAG_DIRTY) as i32
            ; mov rcx, QWORD [r12 + DATA_TAGS]
            ; add rcx, r10
            ; movd xmm0, r9d
            ; pshufd xmm0, xmm0, 0
            ; movdqu xmm2, [r12 + DIRTY4]
            ; movdqu xmm1, [rcx]
            ; por xmm1, xmm2
            ; pcmpeqd xmm1, xmm0
            ; movmskps r10d, xmm1
            ; test r10d, r10d
            ; jz =>slow
        );
        if write {
            // Stored only when not dirty yet (usually it is): a narrow store
            // into the set would stall the next lookup's 16-byte load of it
            // (no store forwarding from a smaller store).
            dynasm!(self.ops
                ; .arch x64
                ; bsf r10d, r10d
                ; bt DWORD [rcx + r10 * 4], 30
                ; jc =>done
                ; or DWORD [rcx + r10 * 4], TAG_DIRTY as i32
            );
        }
        dynasm!(self.ops ; .arch x64 ; =>done);
    }

    /// `eax` = the load at `esi` (`load` done in place where it can be).
    fn load_inline(&mut self, kind: LdKind) {
        let (slow, done) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let (width, helper) = match kind {
            LdKind::Word => (4, TIMED_READ32),
            LdKind::Half | LdKind::HalfSigned => (2, TIMED_READ16),
            LdKind::Byte | LdKind::ByteSigned => (1, TIMED_READ8),
        };
        self.region_lookup(width, false, slow);
        self.tag(tag::MEM);
        match width {
            4 => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [r11]),
            2 => dynasm!(self.ops ; .arch x64 ; movzx eax, WORD [r11]),
            _ => dynasm!(self.ops ; .arch x64 ; movzx eax, BYTE [r11]),
        }
        let pc = self.pc;
        dynasm!(self.ops
            ; .arch x64
            ; jmp =>done
            ; =>slow
            ; mov rdi, QWORD [r12 + BUS]
            ; mov rdx, rbp
            ; mov ecx, pc as i32
            ; lea r8, [rsp]
            ; call QWORD [r12 + helper]
            ; =>done
        );
        match kind {
            LdKind::HalfSigned => dynasm!(self.ops ; .arch x64 ; movsx eax, ax),
            LdKind::ByteSigned => dynasm!(self.ops ; .arch x64 ; movsx eax, al),
            _ => {}
        }
        self.tag(tag::BODY);
    }

    /// The store of `edx` at `esi` (`store` done in place where it can be).
    fn store_inline(&mut self, size: Size) {
        let (slow, done) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let (width, helper) = match size {
            Size::Word => (4, TIMED_WRITE32),
            Size::Half => (2, TIMED_WRITE16),
            Size::Byte => (1, TIMED_WRITE8),
        };
        self.region_lookup(width, true, slow);
        self.tag(tag::MEM);
        match width {
            4 => dynasm!(self.ops ; .arch x64 ; mov DWORD [r11], edx),
            2 => dynasm!(self.ops ; .arch x64 ; mov WORD [r11], dx),
            _ => dynasm!(self.ops ; .arch x64 ; mov BYTE [r11], dl),
        }
        let pc = self.pc;
        dynasm!(self.ops
            ; .arch x64
            ; jmp =>done
            ; =>slow
            ; mov rdi, QWORD [r12 + BUS]
            ; mov rcx, rbp
            ; mov r8d, pc as i32
            ; lea r9, [rsp]
            ; call QWORD [r12 + helper]
            ; =>done
        );
        self.tag(tag::BODY);
    }

    /// Jumps to `skip` unless condition `cc` holds.
    fn cond_skip(&mut self, cc: Cond, skip: DynamicLabel) {
        if cc == COND_AL {
            return;
        }
        let table = cond_table(cc);
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, DWORD [rbx + ST]
            ; shr eax, 8
            ; and eax, 15
            ; mov ecx, table as i32
            ; bt ecx, eax
            ; jnc =>skip
        );
    }

    /// Sets STATUS32's flags from the host flags the last operation left
    /// (its result in `eax` for Z/N).
    fn flags(&mut self, writes: Writes) {
        match writes {
            Writes::All => dynasm!(self.ops
                ; .arch x64
                ; seto r8b
                ; setc r9b
                ; setz r10b
                ; sets r11b
            ),
            Writes::Zn => dynasm!(self.ops
                ; .arch x64
                ; setz r10b
                ; sets r11b
            ),
            // Shifts and rotates: C first, then Z/N from the result (a zero
            // count leaves the host's ZF/SF alone, and rotates never set them).
            Writes::Znc => dynasm!(self.ops
                ; .arch x64
                ; setc r9b
                ; test eax, eax
                ; setz r10b
                ; sets r11b
            ),
        }
    }

    /// Merges the flags captured by `flags` into STATUS32.
    fn merge_flags(&mut self, writes: Writes) {
        let (z, n, c, v) = (1u32 << Z_BIT, 1u32 << N_BIT, 1u32 << C_BIT, 1u32 << V_BIT);
        let mask = match writes {
            Writes::All => z | n | c | v,
            Writes::Zn => z | n,
            Writes::Znc => z | n | c,
        };
        dynasm!(self.ops
            ; .arch x64
            ; mov edx, DWORD [rbx + ST]
            ; and edx, !mask as i32
            ; movzx r10d, r10b
            ; shl r10d, Z_BIT
            ; or edx, r10d
            ; movzx r11d, r11b
            ; shl r11d, N_BIT
            ; or edx, r11d
        );
        if matches!(writes, Writes::All | Writes::Znc) {
            dynasm!(self.ops
                ; .arch x64
                ; movzx r9d, r9b
                ; shl r9d, C_BIT
                ; or edx, r9d
            );
        }
        if matches!(writes, Writes::All) {
            dynasm!(self.ops
                ; .arch x64
                ; movzx r8d, r8b
                ; shl r8d, V_BIT
                ; or edx, r8d
            );
        }
        dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + ST], edx);
    }

    /// Host CF = the guest's C flag.
    fn load_carry(&mut self) {
        dynasm!(self.ops ; .arch x64 ; bt DWORD [rbx + ST], C_BIT);
    }

    /// `eax = b`, `ecx = c` (a register or an immediate).
    fn operands(&mut self, b: u8, c: Operand) {
        dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + r(b)]);
        self.operand_ecx(c);
    }

    fn operand_ecx(&mut self, c: Operand) {
        match c {
            Operand::Reg(c) => dynasm!(self.ops ; .arch x64 ; mov ecx, DWORD [rbx + r(c)]),
            Operand::Imm(v) => dynasm!(self.ops ; .arch x64 ; mov ecx, v as i32),
        }
    }

    /// `eax = op(eax, ecx)` without flags (`AluOp::apply`).
    fn alu(&mut self, op: AluOp) {
        match op {
            AluOp::Add => dynasm!(self.ops ; .arch x64 ; add eax, ecx),
            AluOp::Add1 => dynasm!(self.ops ; .arch x64 ; shl ecx, 1 ; add eax, ecx),
            AluOp::Add2 => dynasm!(self.ops ; .arch x64 ; shl ecx, 2 ; add eax, ecx),
            AluOp::Add3 => dynasm!(self.ops ; .arch x64 ; shl ecx, 3 ; add eax, ecx),
            AluOp::Sub => dynasm!(self.ops ; .arch x64 ; sub eax, ecx),
            AluOp::Sub1 => dynasm!(self.ops ; .arch x64 ; shl ecx, 1 ; sub eax, ecx),
            AluOp::Sub2 => dynasm!(self.ops ; .arch x64 ; shl ecx, 2 ; sub eax, ecx),
            AluOp::Sub3 => dynasm!(self.ops ; .arch x64 ; shl ecx, 3 ; sub eax, ecx),
            AluOp::Rsub => dynasm!(self.ops ; .arch x64 ; sub ecx, eax ; mov eax, ecx),
            AluOp::And => dynasm!(self.ops ; .arch x64 ; and eax, ecx),
            AluOp::Or => dynasm!(self.ops ; .arch x64 ; or eax, ecx),
            AluOp::Xor => dynasm!(self.ops ; .arch x64 ; xor eax, ecx),
            AluOp::Bic => dynasm!(self.ops ; .arch x64 ; not ecx ; and eax, ecx),
            AluOp::Asl => dynasm!(self.ops ; .arch x64 ; shl eax, cl),
            AluOp::Lsr => dynasm!(self.ops ; .arch x64 ; shr eax, cl),
            AluOp::Asr => dynasm!(self.ops ; .arch x64 ; sar eax, cl),
            AluOp::Adc => {
                self.load_carry();
                dynasm!(self.ops ; .arch x64 ; adc eax, ecx)
            }
            AluOp::Sbc => {
                self.load_carry();
                dynasm!(self.ops ; .arch x64 ; sbb eax, ecx)
            }
        }
    }

    /// `eax = op(eax, ecx)` with .F (`Cpu::alu_flags`); result stored to
    /// `d` unless discarded.
    fn alu_flags(&mut self, op: AluOp, d: u8) {
        self.alu_flags_to(op, (d != DISCARD).then_some(d & 31));
    }

    /// `alu_flags` storing to any plain register (`d`), or nowhere.
    fn alu_flags_to(&mut self, op: AluOp, d: Option<u8>) {
        let writes = match op {
            AluOp::Add | AluOp::Add1 | AluOp::Add2 | AluOp::Add3 => {
                let k = match op {
                    AluOp::Add1 => 1,
                    AluOp::Add2 => 2,
                    AluOp::Add3 => 3,
                    _ => 0,
                };
                if k != 0 {
                    dynasm!(self.ops ; .arch x64 ; shl ecx, k);
                }
                dynasm!(self.ops ; .arch x64 ; add eax, ecx);
                Writes::All
            }
            AluOp::Sub | AluOp::Sub1 | AluOp::Sub2 | AluOp::Sub3 => {
                let k = match op {
                    AluOp::Sub1 => 1,
                    AluOp::Sub2 => 2,
                    AluOp::Sub3 => 3,
                    _ => 0,
                };
                if k != 0 {
                    dynasm!(self.ops ; .arch x64 ; shl ecx, k);
                }
                dynasm!(self.ops ; .arch x64 ; sub eax, ecx);
                Writes::All
            }
            AluOp::Adc => {
                self.load_carry();
                dynasm!(self.ops ; .arch x64 ; adc eax, ecx);
                Writes::All
            }
            AluOp::Sbc => {
                self.load_carry();
                dynasm!(self.ops ; .arch x64 ; sbb eax, ecx);
                Writes::All
            }
            AluOp::Rsub => {
                // c - b, flags of that subtraction (`mov` keeps them).
                dynasm!(self.ops ; .arch x64 ; sub ecx, eax ; mov eax, ecx);
                Writes::All
            }
            AluOp::And => {
                dynasm!(self.ops ; .arch x64 ; and eax, ecx);
                Writes::Zn
            }
            AluOp::Or => {
                dynasm!(self.ops ; .arch x64 ; or eax, ecx);
                Writes::Zn
            }
            AluOp::Xor => {
                dynasm!(self.ops ; .arch x64 ; xor eax, ecx);
                Writes::Zn
            }
            AluOp::Bic => {
                dynasm!(self.ops ; .arch x64 ; not ecx ; and eax, ecx);
                Writes::Zn
            }
            // C = the last bit shifted out, unchanged for a zero count: the
            // host's shift leaves CF alone then, so CF starts as the guest C.
            AluOp::Asl | AluOp::Lsr | AluOp::Asr => {
                self.load_carry();
                match op {
                    AluOp::Asl => dynasm!(self.ops ; .arch x64 ; shl eax, cl),
                    AluOp::Lsr => dynasm!(self.ops ; .arch x64 ; shr eax, cl),
                    _ => dynasm!(self.ops ; .arch x64 ; sar eax, cl),
                }
                Writes::Znc
            }
        };
        self.flags(writes);
        self.gwrite(d);
        self.merge_flags(writes);
    }

    /// Calls a memory helper: address in `esi`, value (writes) in `edx`;
    /// a read's result comes back in `eax`.
    fn call(&mut self, helper: i32) {
        dynasm!(self.ops
            ; .arch x64
            ; mov rdi, QWORD [r12 + BUS]
            ; call QWORD [r12 + helper]
        );
    }

    /// `eax` = the load at `esi`, extended as `kind` says (`exec::load`).
    fn load(&mut self, kind: LdKind) {
        if self.inline {
            return self.load_inline(kind);
        }
        match kind {
            LdKind::Word => self.call(READ32),
            LdKind::Half => self.call(READ16),
            LdKind::HalfSigned => {
                self.call(READ16);
                dynasm!(self.ops ; .arch x64 ; movsx eax, ax);
            }
            LdKind::Byte => self.call(READ8),
            LdKind::ByteSigned => {
                self.call(READ8);
                dynasm!(self.ops ; .arch x64 ; movsx eax, al);
            }
        }
    }

    fn store(&mut self, size: Size) {
        if self.inline {
            return self.store_inline(size);
        }
        match size {
            Size::Word => self.call(WRITE32),
            Size::Half => self.call(WRITE16),
            Size::Byte => self.call(WRITE8),
        }
    }

    /// `esi` = the effective address with base `b`, offset `off` and
    /// write-back `wb` (`Cpu::write_back`); the base is updated.
    fn address(&mut self, wb: WriteBack, b: u8, off: u32, size: Size) {
        dynasm!(self.ops ; .arch x64 ; mov esi, DWORD [rbx + r(b)]);
        match wb {
            WriteBack::None => dynasm!(self.ops ; .arch x64 ; add esi, off as i32),
            WriteBack::Pre => dynasm!(self.ops
                ; .arch x64
                ; add esi, off as i32
                ; mov DWORD [rbx + r(b)], esi
            ),
            WriteBack::Post => dynasm!(self.ops
                ; .arch x64
                ; lea eax, [rsi + off as i32]
                ; mov DWORD [rbx + r(b)], eax
            ),
            WriteBack::Scaled => {
                let scale = match size {
                    Size::Word => 2,
                    Size::Half => 1,
                    Size::Byte => 0,
                };
                dynasm!(self.ops ; .arch x64 ; add esi, (off << scale) as i32)
            }
        }
    }

    /// `reg` (eax, ecx, edx or esi) = a generic source.
    fn gread(&mut self, s: GSrc, reg: u8) {
        match (s, reg) {
            (GSrc::Imm(v), 0) => dynasm!(self.ops ; .arch x64 ; mov eax, v as i32),
            (GSrc::Imm(v), 1) => dynasm!(self.ops ; .arch x64 ; mov ecx, v as i32),
            (GSrc::Imm(v), 2) => dynasm!(self.ops ; .arch x64 ; mov edx, v as i32),
            (GSrc::Imm(v), _) => dynasm!(self.ops ; .arch x64 ; mov esi, v as i32),
            (GSrc::Reg(r), 0) => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + 4 * r as i32]),
            (GSrc::Reg(r), 1) => dynasm!(self.ops ; .arch x64 ; mov ecx, DWORD [rbx + 4 * r as i32]),
            (GSrc::Reg(r), 2) => dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + 4 * r as i32]),
            (GSrc::Reg(r), _) => dynasm!(self.ops ; .arch x64 ; mov esi, DWORD [rbx + 4 * r as i32]),
        }
    }

    /// Writes `eax` to a generic destination (none: nothing).
    fn gwrite(&mut self, d: Option<u8>) {
        if let Some(r) = d {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + 4 * r as i32], eax);
        }
    }

    /// `eax` result, Z/N from it: captured, written, merged (the
    /// executor's `write_dst` then `flags(r, None, None)`).
    fn result_zn(&mut self, d: Option<u8>, f: bool) {
        if f {
            dynasm!(self.ops ; .arch x64 ; test eax, eax);
            self.flags(Writes::Zn);
        }
        self.gwrite(d);
        if f {
            self.merge_flags(Writes::Zn);
        }
    }

    /// An instruction with no fast form, from its full decode, as
    /// `Cpu::execute` runs it (`generic_ok` shapes only).
    fn generic(&mut self, i: &Instr) {
        let skip = self.ops.new_dynamic_label();
        self.cond_skip(i.cc, skip);
        let (b, c) = (gsrc(i.src1, i).expect("generic_ok"), gsrc(i.src2, i).expect("generic_ok"));
        let d = gdst(i.dst).expect("generic_ok");
        // The executor sets flags from results only for 32-bit encodings.
        let f = i.set_flags && !i.short;
        match i.op {
            Op::Ld => self.gload(i, b, c, d),
            Op::St => self.gstore(i, b, c),
            op if alu_op(op).is_some() => {
                let op = alu_op(op).unwrap();
                self.gread(b, 0);
                self.gread(c, 1);
                if f {
                    self.alu_flags_to(op, d);
                } else {
                    self.alu(op);
                    self.gwrite(d);
                }
            }
            Op::Mov => {
                self.gread(c, 0);
                self.result_zn(d, f);
            }
            Op::Bset | Op::Bclr | Op::Bxor | Op::Bmsk => {
                self.gread(b, 0);
                self.gread(c, 1);
                match i.op {
                    Op::Bset => dynasm!(self.ops ; .arch x64 ; mov edx, 1 ; shl edx, cl ; or eax, edx),
                    Op::Bclr => dynasm!(self.ops ; .arch x64 ; mov edx, 1 ; shl edx, cl ; not edx ; and eax, edx),
                    Op::Bxor => dynasm!(self.ops ; .arch x64 ; mov edx, 1 ; shl edx, cl ; xor eax, edx),
                    // (2 << n) - 1: all ones for n = 31 (the shift gives 0).
                    _ => dynasm!(self.ops ; .arch x64 ; and ecx, 31 ; mov edx, 2 ; shl edx, cl ; sub edx, 1 ; and eax, edx),
                }
                self.result_zn(d, f);
            }
            Op::RorM => {
                self.gread(b, 0);
                self.gread(c, 1);
                if f {
                    // C = the last bit rotated out, unchanged for a zero count.
                    self.load_carry();
                    dynasm!(self.ops ; .arch x64 ; ror eax, cl);
                    self.flags(Writes::Znc);
                    self.gwrite(d);
                    self.merge_flags(Writes::Znc);
                } else {
                    dynasm!(self.ops ; .arch x64 ; ror eax, cl);
                    self.gwrite(d);
                }
            }
            Op::Cmp | Op::Rcmp => {
                self.gread(b, 0);
                self.gread(c, 1);
                if i.op == Op::Cmp {
                    dynasm!(self.ops ; .arch x64 ; sub eax, ecx);
                } else {
                    dynasm!(self.ops ; .arch x64 ; sub ecx, eax ; mov eax, ecx);
                }
                self.flags(Writes::All);
                self.merge_flags(Writes::All);
            }
            Op::Tst | Op::Btst => {
                self.gread(b, 0);
                self.gread(c, 1);
                if i.op == Op::Btst {
                    dynasm!(self.ops ; .arch x64 ; mov edx, 1 ; shl edx, cl ; and eax, edx);
                } else {
                    dynasm!(self.ops ; .arch x64 ; and eax, ecx);
                }
                self.flags(Writes::Zn);
                self.merge_flags(Writes::Zn);
            }
            Op::Asl | Op::Asr | Op::Lsr | Op::Ror | Op::Rrc | Op::Rlc => {
                self.gread(c, 0);
                let asl = i.op == Op::Asl;
                match i.op {
                    Op::Asl => dynasm!(self.ops ; .arch x64 ; shl eax, 1),
                    Op::Asr => dynasm!(self.ops ; .arch x64 ; sar eax, 1),
                    Op::Lsr => dynasm!(self.ops ; .arch x64 ; shr eax, 1),
                    Op::Ror => dynasm!(self.ops ; .arch x64 ; ror eax, 1),
                    Op::Rrc => {
                        self.load_carry();
                        dynasm!(self.ops ; .arch x64 ; rcr eax, 1)
                    }
                    _ => {
                        self.load_carry();
                        dynasm!(self.ops ; .arch x64 ; rcl eax, 1)
                    }
                }
                let writes = if asl { Writes::All } else { Writes::Znc };
                if f {
                    self.flags(writes);
                }
                self.gwrite(d);
                if f {
                    self.merge_flags(writes);
                }
            }
            Op::Max | Op::Min => {
                // MAX d,b,c = max(b,c) signed; MIN d,b,c = min(b,c) signed.
                // With .F: Z/N/V from b-c (like CMP), C = whether c was selected.
                self.gread(b, 0);  // eax = b
                self.gread(c, 1);  // ecx = c
                if f {
                    // Compute b-c for Z/N/V flags (edx = b-c; eax, ecx unchanged).
                    dynasm!(self.ops ; .arch x64 ; mov edx, eax ; sub edx, ecx);
                    // Capture V, Z, N from b-c; C = take_c (whether c was chosen).
                    // setle: ZF || SF!=OF, which is (b <= c signed) = take_c for MAX.
                    // setge: ZF || SF==OF, which is (b >= c signed) = take_c for MIN.
                    dynasm!(self.ops ; .arch x64 ; seto r8b ; setz r10b ; sets r11b);
                    if i.op == Op::Max {
                        dynasm!(self.ops ; .arch x64 ; setle r9b);   // C = (b <= c signed)
                        dynasm!(self.ops ; .arch x64 ; cmovl eax, ecx); // result = c if b < c
                    } else {
                        dynasm!(self.ops ; .arch x64 ; setge r9b);   // C = (b >= c signed)
                        dynasm!(self.ops ; .arch x64 ; cmovg eax, ecx); // result = c if b > c
                    }
                    self.gwrite(d);
                    self.merge_flags(Writes::All);
                } else {
                    if i.op == Op::Max {
                        dynasm!(self.ops ; .arch x64 ; cmp eax, ecx ; cmovl eax, ecx);
                    } else {
                        dynasm!(self.ops ; .arch x64 ; cmp eax, ecx ; cmovg eax, ecx);
                    }
                    self.gwrite(d);
                }
            }
            Op::Normw => {
                // norm16(c): 15 for the all-zeros/all-ones 16-bit value; otherwise
                // 14 - BSR(sign-flip when negative) of the lower 16 bits.
                // Flags: Z/N from sign-extended lower 16 bits.
                self.gread(c, 0);  // eax = c
                let (done, done_15) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx eax, ax          // eax = sign-extended lower 16 bits
                    ; mov esi, eax           // save for flags
                    ; test eax, eax
                    ; jz =>done_15
                    ; cmp eax, -1
                    ; je =>done_15
                    ; mov edx, eax
                    ; sar edx, 31            // sign mask: 0 for positive, -1 for negative
                    ; xor edx, eax           // flip bits if negative; upper bits -> 0
                    ; bsr edx, edx           // BSR of sign-flipped value
                    ; mov eax, 14
                    ; sub eax, edx           // 14 - BSR = norm16 result
                    ; jmp =>done
                    ; =>done_15
                    ; mov eax, 15
                    ; =>done
                );
                self.gwrite(d);
                if i.set_flags {
                    dynasm!(self.ops ; .arch x64 ; test esi, esi);
                    self.flags(Writes::Zn);
                    self.merge_flags(Writes::Zn);
                }
            }
            Op::Norm => {
                // `norm32`: 31 for 0 and all ones, else the leading sign
                // bits less one; Z/N describe the source (with .F, short or not).
                self.gread(c, 0);
                let done = self.ops.new_dynamic_label();
                dynasm!(self.ops
                    ; .arch x64
                    ; mov esi, eax
                    ; mov edx, eax
                    ; sar edx, 31
                    ; xor edx, eax
                    ; mov eax, 31
                    ; bsr edx, edx
                    ; jz =>done
                    ; mov eax, 30
                    ; sub eax, edx
                    ; =>done
                );
                self.gwrite(d);
                if i.set_flags {
                    dynasm!(self.ops ; .arch x64 ; test esi, esi);
                    self.flags(Writes::Zn);
                    self.merge_flags(Writes::Zn);
                }
            }
            Op::Abs => {
                // C = the source's sign, V = it was 0x8000_0000 (PRM, ABS).
                self.gread(c, 0);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov esi, eax
                    ; mov edx, eax
                    ; sar edx, 31
                    ; xor eax, edx
                    ; sub eax, edx
                );
                if f {
                    dynasm!(self.ops
                        ; .arch x64
                        ; cmp esi, 0x8000_0000u32 as i32
                        ; sete r8b
                        ; bt esi, 31
                        ; setc r9b
                        ; test eax, eax
                        ; setz r10b
                        ; sets r11b
                    );
                }
                self.gwrite(d);
                if f {
                    self.merge_flags(Writes::All);
                }
            }
            _ => {
                // Sexb, Sexw, Extb, Extw, Not, Neg, Swap: Z/N from the result.
                self.gread(c, 0);
                match i.op {
                    Op::Sexb => dynasm!(self.ops ; .arch x64 ; movsx eax, al),
                    Op::Sexw => dynasm!(self.ops ; .arch x64 ; movsx eax, ax),
                    Op::Extb => dynasm!(self.ops ; .arch x64 ; movzx eax, al),
                    Op::Extw => dynasm!(self.ops ; .arch x64 ; movzx eax, ax),
                    Op::Not => dynasm!(self.ops ; .arch x64 ; not eax),
                    Op::Neg => dynasm!(self.ops ; .arch x64 ; neg eax),
                    Op::Swap => dynasm!(self.ops ; .arch x64 ; rol eax, 16),
                    other => unreachable!("generic_ok let {other:?} through"),
                }
                self.result_zn(d, f);
            }
        }
        dynasm!(self.ops ; .arch x64 ; =>skip);
    }

    /// `esi` = the effective address of a generic load/store (`Cpu::address`),
    /// the base written back; eax and ecx clobbered, edx kept.
    fn gaddress(&mut self, i: &Instr, b: GSrc, c: GSrc) {
        self.gread(b, 3);
        self.gread(c, 1);
        let base = match i.src1 {
            Src::Reg(r) => Some(r),
            _ => None,
        };
        match i.mem.wb {
            WriteBack::None => dynasm!(self.ops ; .arch x64 ; add esi, ecx),
            WriteBack::Scaled => {
                let scale = match i.mem.size {
                    Size::Word => 2,
                    Size::Half => 1,
                    Size::Byte => 0,
                };
                dynasm!(self.ops ; .arch x64 ; shl ecx, scale ; add esi, ecx);
            }
            WriteBack::Pre => {
                dynasm!(self.ops ; .arch x64 ; add esi, ecx);
                if let Some(r) = base {
                    dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + 4 * r as i32], esi);
                }
            }
            WriteBack::Post => {
                dynasm!(self.ops ; .arch x64 ; lea eax, [rsi + rcx]);
                if let Some(r) = base {
                    dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + 4 * r as i32], eax);
                }
            }
        }
    }

    fn gload(&mut self, i: &Instr, b: GSrc, c: GSrc, d: Option<u8>) {
        self.gaddress(i, b, c);
        // PREFETCH (to no register): no access (`Cpu::exec_load`).
        if i.dst == Dst::Discard {
            return;
        }
        let kind = match (i.mem.size, i.mem.sign_extend) {
            (Size::Word, _) => LdKind::Word,
            (Size::Half, false) => LdKind::Half,
            (Size::Half, true) => LdKind::HalfSigned,
            (Size::Byte, false) => LdKind::Byte,
            (Size::Byte, true) => LdKind::ByteSigned,
        };
        self.load(kind);
        self.gwrite(d);
    }

    fn gstore(&mut self, i: &Instr, b: GSrc, c: GSrc) {
        // The value is read before the address (`Cpu::exec_store`).
        self.gread(gsrc(i.data, i).expect("generic_ok"), 2);
        self.gaddress(i, b, c);
        self.store(i.mem.size);
    }

    /// One non-branch instruction (`Cpu::execute_fast`).
    fn body(&mut self, h: &Hot) {
        match h.fast {
            Fast::Mov { d, s } => dynasm!(self.ops
                ; .arch x64
                ; mov eax, DWORD [rbx + r(s)]
                ; mov DWORD [rbx + r(d)], eax
            ),
            Fast::MovI { d, v } => dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], v as i32),
            Fast::Alu { op, d, b, c } => {
                self.operands(b, Operand::Reg(c));
                self.alu(op);
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
            }
            Fast::AluI { op, d, b, v } => {
                self.operands(b, Operand::Imm(v));
                self.alu(op);
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
            }
            Fast::AluCc { op, cc, d, b, c } => self.alu_cc(op, cc, d, b, Operand::Reg(c)),
            Fast::AluCcI { op, cc, d, b, v } => self.alu_cc(op, cc, d, b, Operand::Imm(v)),
            Fast::AluF { op, d, b, c } => {
                self.operands(b, Operand::Reg(c));
                self.alu_flags(op, d);
            }
            Fast::AluFI { op, d, b, v } => {
                self.operands(b, Operand::Imm(v));
                self.alu_flags(op, d);
            }
            Fast::Cmp { b, c } => self.compare(b, Operand::Reg(c)),
            Fast::CmpI { b, v } => self.compare(b, Operand::Imm(v)),
            Fast::Un { op, d, c } => {
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + r(c)]);
                match op {
                    UnOp::Asl => dynasm!(self.ops ; .arch x64 ; shl eax, 1),
                    UnOp::Asr => dynasm!(self.ops ; .arch x64 ; sar eax, 1),
                    UnOp::Lsr => dynasm!(self.ops ; .arch x64 ; shr eax, 1),
                    UnOp::Ror => dynasm!(self.ops ; .arch x64 ; ror eax, 1),
                    UnOp::Sexb => dynasm!(self.ops ; .arch x64 ; movsx eax, al),
                    UnOp::Sexw => dynasm!(self.ops ; .arch x64 ; movsx eax, ax),
                    UnOp::Extb => dynasm!(self.ops ; .arch x64 ; movzx eax, al),
                    UnOp::Extw => dynasm!(self.ops ; .arch x64 ; movzx eax, ax),
                    UnOp::Not => dynasm!(self.ops ; .arch x64 ; not eax),
                    UnOp::Neg => dynasm!(self.ops ; .arch x64 ; neg eax),
                    // `wrapping_abs`: i32::MIN stays i32::MIN.
                    UnOp::Abs => dynasm!(self.ops
                        ; .arch x64
                        ; mov edx, eax
                        ; sar edx, 31
                        ; xor eax, edx
                        ; sub eax, edx
                    ),
                    UnOp::Swap => dynasm!(self.ops ; .arch x64 ; rol eax, 16),
                }
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
            }
            Fast::Rot { op, f, d, c } => {
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + r(c)]);
                // `RotOp::apply`: x86's one-bit shifts and rotates give the
                // same result and C (and ASL's V, as OF).
                match op {
                    RotOp::Asl => dynasm!(self.ops ; .arch x64 ; shl eax, 1),
                    RotOp::Asr => dynasm!(self.ops ; .arch x64 ; sar eax, 1),
                    RotOp::Lsr => dynasm!(self.ops ; .arch x64 ; shr eax, 1),
                    RotOp::Ror => dynasm!(self.ops ; .arch x64 ; ror eax, 1),
                    RotOp::Rrc => {
                        self.load_carry();
                        dynasm!(self.ops ; .arch x64 ; rcr eax, 1)
                    }
                    RotOp::Rlc => {
                        self.load_carry();
                        dynasm!(self.ops ; .arch x64 ; rcl eax, 1)
                    }
                }
                // ASL's flags come straight from the shift (Z/N included);
                // the others' C from the shift, Z/N from the result.
                let writes = if op == RotOp::Asl { Writes::All } else { Writes::Znc };
                if f {
                    self.flags(writes);
                }
                if d != DISCARD {
                    dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
                }
                if f {
                    self.merge_flags(writes);
                }
            }
            Fast::Mul64 { signed, b, c } => self.mul64(signed, b, Operand::Reg(c)),
            Fast::Mul64I { signed, b, v } => self.mul64(signed, b, Operand::Imm(v)),
            Fast::Ld { kind, d, b, off } => {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov esi, DWORD [rbx + r(b)]
                    ; add esi, off as i32
                );
                self.load(kind);
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
            }
            Fast::LdR { kind, d, b, c } => {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov esi, DWORD [rbx + r(b)]
                    ; add esi, DWORD [rbx + r(c)]
                );
                self.load(kind);
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
            }
            Fast::LdX { kind, wb, d, b, off } => {
                let size = match kind {
                    LdKind::Word => Size::Word,
                    LdKind::Half | LdKind::HalfSigned => Size::Half,
                    LdKind::Byte | LdKind::ByteSigned => Size::Byte,
                };
                self.address(wb, b, off, size);
                self.load(kind);
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
            }
            Fast::St { size, s, b, off } => {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov edx, DWORD [rbx + r(s)]
                    ; mov esi, DWORD [rbx + r(b)]
                    ; add esi, off as i32
                );
                self.store(size);
            }
            Fast::StX { size, wb, s, b, off } => {
                // The value is read before the base is written back.
                dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + r(s)]);
                // `address` uses eax for .AB, not edx.
                self.address(wb, b, off, size);
                self.store(size);
            }
            Fast::Push { s } => {
                dynasm!(self.ops
                    ; .arch x64
                    ; mov edx, DWORD [rbx + r(s)]
                    ; mov esi, DWORD [rbx + SP]
                    ; sub esi, 4
                    ; mov DWORD [rbx + SP], esi
                );
                self.store(Size::Word);
            }
            Fast::Pop { d } => {
                dynasm!(self.ops ; .arch x64 ; mov esi, DWORD [rbx + SP]);
                self.load(LdKind::Word);
                // SP is written before the destination (POP into SP keeps
                // the loaded value).
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, DWORD [rbx + SP]
                    ; add ecx, 4
                    ; mov DWORD [rbx + SP], ecx
                    ; mov DWORD [rbx + r(d)], eax
                );
            }
            Fast::General
            | Fast::Br { .. }
            | Fast::BrR { .. }
            | Fast::Bbit { .. }
            | Fast::BbitI { .. }
            | Fast::B { .. }
            | Fast::Bl { .. }
            | Fast::BlD { .. }
            | Fast::J { .. }
            | Fast::Jcc { .. }
            | Fast::Jl { .. }
            | Fast::JlD { .. } => unreachable!("not a straight-line form: {:?}", h.fast),
        }
    }

    /// MUL64/MULU64 (`Cpu::mul64`): the 64-bit product into MLO (unless
    /// `mmid_low`), MHI, and MMID (its middle 32 bits, or the low word).
    fn mul64(&mut self, signed: bool, b: u8, c: Operand) {
        self.operands(b, c);
        if signed {
            dynasm!(self.ops ; .arch x64 ; imul ecx);
        } else {
            dynasm!(self.ops ; .arch x64 ; mul ecx);
        }
        if !self.mmid_low {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + MLO], eax);
        }
        dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + MHI], edx);
        if !self.mmid_low {
            dynasm!(self.ops ; .arch x64 ; shrd eax, edx, 16);
        }
        dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + MMID], eax);
    }

    fn alu_cc(&mut self, op: AluOp, cc: Cond, d: u8, b: u8, c: Operand) {
        let skip = self.ops.new_dynamic_label();
        self.cond_skip(cc, skip);
        self.operands(b, c);
        self.alu(op);
        dynasm!(self.ops
            ; .arch x64
            ; mov DWORD [rbx + r(d)], eax
            ; =>skip
        );
    }

    /// CMP: Z/N/C/V of b - c (`Cpu::compare`).
    fn compare(&mut self, b: u8, c: Operand) {
        dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + r(b)]);
        match c {
            Operand::Reg(c) => dynasm!(self.ops ; .arch x64 ; sub eax, DWORD [rbx + r(c)]),
            Operand::Imm(v) => dynasm!(self.ops ; .arch x64 ; sub eax, v as i32),
        }
        self.flags(Writes::All);
        self.merge_flags(Writes::All);
    }

    /// The branch at `k` (and its delay slot when it can run here); returns
    /// the number of instructions the code covers.
    fn branch(&mut self, items: &[Item], k: usize) -> usize {
        let it = items[k];
        let h = it.hot;
        self.pc = h.addr;
        self.tag(tag::BRANCH);
        let executed = k as u32 + 1;
        let prev = if k > 0 { items[k - 1].hot.addr } else { h.addr };
        // The delay slot, when it can run here: compiled, and not itself a
        // branch (the interpreter reports that as unpredictable).
        let slot = items.get(k + 1).copied().filter(|s| h.delay && compilable(s) && !is_branch(s.hot.fast) && !s.hot.branchy);
        // The link: the next instruction, or the one after the delay slot.
        let link = match h.fast {
            Fast::Bl { .. } | Fast::Jl { .. } => Some(h.next),
            Fast::BlD { .. } | Fast::JlD { .. } => match items.get(k + 1) {
                Some(s) => Some(s.hot.next),
                None => {
                    // The slot's length is unknown here: let the interpreter run it.
                    self.fallback(h.addr, k as u32, prev);
                    return k;
                }
            },
            _ => None,
        };
        // Taken (r15d) and the target (r14d), with no side effect yet.
        let decided = self.ops.new_dynamic_label();
        match h.fast {
            Fast::B { cc } | Fast::Bl { cc } | Fast::BlD { cc } => {
                dynasm!(self.ops ; .arch x64 ; xor r15d, r15d);
                self.cond_skip(cc, decided);
                dynasm!(self.ops ; .arch x64 ; mov r15d, 1);
                dynasm!(self.ops ; .arch x64 ; =>decided ; mov r14d, h.target as i32);
            }
            Fast::Br { cond, b, v } => {
                dynasm!(self.ops ; .arch x64 ; xor r15d, r15d ; mov eax, DWORD [rbx + r(b)] ; cmp eax, v as i32);
                self.br_skip(cond, decided);
                dynasm!(self.ops ; .arch x64 ; mov r15d, 1 ; =>decided ; mov r14d, h.target as i32);
            }
            Fast::BrR { cond, b, c } => {
                dynasm!(self.ops ; .arch x64 ; xor r15d, r15d ; mov eax, DWORD [rbx + r(b)] ; cmp eax, DWORD [rbx + r(c)]);
                self.br_skip(cond, decided);
                dynasm!(self.ops ; .arch x64 ; mov r15d, 1 ; =>decided ; mov r14d, h.target as i32);
            }
            Fast::Bbit { bit1, b, c } => {
                // BT eax, ecx sets CF = bit at position (ecx mod 32) in eax.
                dynasm!(self.ops
                    ; .arch x64
                    ; xor r15d, r15d
                    ; mov eax, DWORD [rbx + r(b)]
                    ; mov ecx, DWORD [rbx + r(c)]
                    ; bt eax, ecx
                );
                if bit1 {
                    dynasm!(self.ops ; .arch x64 ; jnc =>decided);
                } else {
                    dynasm!(self.ops ; .arch x64 ; jc =>decided);
                }
                dynasm!(self.ops ; .arch x64 ; mov r15d, 1 ; =>decided ; mov r14d, h.target as i32);
            }
            Fast::BbitI { bit1, b, v } => {
                // BT [mem], imm8 sets CF = bit at position imm8 in the memory dword.
                dynasm!(self.ops
                    ; .arch x64
                    ; xor r15d, r15d
                    ; bt DWORD [rbx + r(b)], v as i8
                );
                if bit1 {
                    dynasm!(self.ops ; .arch x64 ; jnc =>decided);
                } else {
                    dynasm!(self.ops ; .arch x64 ; jc =>decided);
                }
                dynasm!(self.ops ; .arch x64 ; mov r15d, 1 ; =>decided ; mov r14d, h.target as i32);
            }
            Fast::J { s } | Fast::Jl { s } | Fast::JlD { s } => dynasm!(self.ops
                ; .arch x64
                ; mov r15d, 1
                ; mov r14d, DWORD [rbx + r(s)]
                ; and r14d, -2
            ),
            Fast::Jcc { cc, s } => {
                dynasm!(self.ops
                    ; .arch x64
                    ; xor r15d, r15d
                    ; mov r14d, DWORD [rbx + r(s)]
                    ; and r14d, -2
                );
                self.cond_skip(cc, decided);
                dynasm!(self.ops ; .arch x64 ; mov r15d, 1 ; =>decided);
            }
            _ => unreachable!(),
        }
        let target = match h.fast {
            Fast::B { .. } | Fast::Bl { .. } | Fast::BlD { .. } | Fast::Br { .. } | Fast::BrR { .. } | Fast::Bbit { .. } | Fast::BbitI { .. } => Pc::Const(h.target),
            _ => Pc::R14,
        };
        // Its fetch timing (a branch touches no data memory).
        self.stall(&h, it.fetch, false);
        self.tag(tag::BRANCH);
        let not_taken = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch x64 ; test r15d, r15d ; jz =>not_taken);
        // Taken: +1 when the delay slot is killed, +1 for a taken BRcc.
        if let Some(link) = link {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + BLINK], link as i32);
        }
        self.cost(&h, (!h.delay) as u64 + h.compare_branch as u64);
        let slot_label = self.ops.new_dynamic_label();
        if !h.delay {
            self.flow(target, executed, h.addr);
        } else if slot.is_some() {
            self.budget_check(Pc::Const(h.next), executed, h.addr, true);
            dynasm!(self.ops ; .arch x64 ; jmp =>slot_label);
        } else {
            // The slot runs in the interpreter, with `delay_target` set.
            dynasm!(self.ops
                ; .arch x64
                ; mov DWORD [r12 + DELAY_PENDING], 1
                ; mov DWORD [r12 + DELAY_TARGET], r14d
            );
            self.exit(EXIT_FLOW, Pc::Const(h.next), executed, h.addr);
        }
        // Not taken: sequential flow.
        dynasm!(self.ops ; .arch x64 ; =>not_taken);
        self.cost(&h, 0);
        self.after_sequential(h.next, executed, h.addr, false);
        let Some(slot) = slot else {
            self.flow(Pc::Const(h.next), executed, h.addr);
            return k + 1;
        };
        dynasm!(self.ops ; .arch x64 ; =>slot_label);
        self.seq = k as u32 + 1;
        self.instr(&slot);
        // After the slot: the taken branch's target, or on in sequence.
        let s = slot.hot;
        let mem = slot.mem();
        let sequential = self.ops.new_dynamic_label();
        self.tag(tag::BRANCH);
        dynasm!(self.ops ; .arch x64 ; test r15d, r15d ; jz =>sequential);
        if mem {
            let stub = self.stub(EXIT_BUS, target, executed + 1, s.addr, false);
            dynasm!(self.ops ; .arch x64 ; test r8d, r8d ; jnz =>stub);
        }
        self.flow(target, executed + 1, s.addr);
        dynasm!(self.ops ; .arch x64 ; =>sequential);
        self.after_sequential(s.next, executed + 1, s.addr, mem);
        self.flow(Pc::Const(s.next), executed + 1, s.addr);
        k + 2
    }

    /// LPcc at `k` (the last of its block): if the condition holds,
    /// LP_START/LP_END are set (in the context, the authority while
    /// compiled code runs) and the flow goes on in sequence, else it jumps
    /// to the loop's end (`Cpu::execute`, Op::Lp).
    fn lp(&mut self, items: &[Item], k: usize) -> usize {
        let it = items[k];
        let (h, i) = (it.hot, it.instr.expect("an LP's decode"));
        self.pc = h.addr;
        let executed = k as u32 + 1;
        if !self.inline {
            self.prep(&h, it.fetch);
        }
        let skip = self.ops.new_dynamic_label();
        self.cond_skip(i.cc, skip);
        dynasm!(self.ops
            ; .arch x64
            ; mov DWORD [r12 + LP_START], h.next as i32
            ; mov DWORD [r12 + LP_END], i.target as i32
        );
        self.stall(&h, it.fetch, false);
        self.cost(&h, 0);
        self.after_sequential(h.next, executed, h.addr, false);
        self.flow(Pc::Const(h.next), executed, h.addr);
        if i.cc != COND_AL {
            // Not set up: a taken jump (its slot killed: +1).
            dynasm!(self.ops ; .arch x64 ; =>skip);
            self.stall(&h, it.fetch, false);
            self.cost(&h, 1);
            self.flow(Pc::Const(i.target), executed, h.addr);
        }
        k + 1
    }

    /// After `cmp b, c`: jumps to `skip` unless BRcc's condition holds.
    fn br_skip(&mut self, cond: BrCond, skip: DynamicLabel) {
        match cond {
            BrCond::Eq => dynasm!(self.ops ; .arch x64 ; jne =>skip),
            BrCond::Ne => dynasm!(self.ops ; .arch x64 ; je =>skip),
            BrCond::Lt => dynasm!(self.ops ; .arch x64 ; jge =>skip),
            BrCond::Ge => dynasm!(self.ops ; .arch x64 ; jl =>skip),
            BrCond::Lo => dynasm!(self.ops ; .arch x64 ; jae =>skip),
            BrCond::Hs => dynasm!(self.ops ; .arch x64 ; jb =>skip),
        }
    }
}

#[derive(Clone, Copy)]
enum Operand {
    Reg(u8),
    Imm(u32),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn condition_table_matches_the_interpreter() {
        assert_eq!(cond_table(COND_AL), 0xFFFF);
        // EQ: Z (bit 3 of the index).
        assert_eq!(cond_table(ribbit_cpu::isa::COND_EQ), 0xFF00);
        // Extension codes never hold on the A5.
        assert_eq!(cond_table(0x10), 0);
    }
}
