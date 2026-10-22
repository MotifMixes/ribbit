//! AArch64 code generation (dynasm-rs): the same code as `x64.rs`, function
//! for function, so that both stay the interpreter's exact copy (see there
//! for why each step is done; this file says only how it maps to AArch64).
//!
//! The generated function is `extern "C" fn(*mut JitCtx) -> u32` (the
//! `ExitKind`). Register use, fixed for the whole function (x19-x28 are
//! callee-saved in AAPCS64, so they survive the helpers):
//!
//! | Host | Holds | x86-64 |
//! |---|---|---|
//! | `x19` | the `JitCtx` | r12 |
//! | `x20` | `&regs[0]` (guest rN at `[x20 + 4*N]`) | rbx |
//! | `x21` | `&status32` | |
//! | `x22` | the cycle count (`now`) | rbp |
//! | `w23` | a taken branch's target while its delay slot runs | r14d |
//! | `w24` | 1 if that branch was taken, 0 if not | r15d |
//! | `x25` | `&ready[0]` (the scoreboard) | r13 |
//! | `x26` | `&ready_max` | |
//! | `w27` | LP_END (the context's copy, kept in step: an LP writes both) | |
//! | `x28` | the budget's end (`JitCtx::end`) | |
//!
//! Scratch: w9 is the x86 code's eax (a result, a loaded value, the stall),
//! w10 its ecx, w1 its esi (an address: the helpers' second argument), w2
//! its edx (a value to store: their third), w8 its r8d (the bus's exit
//! request after a memory instruction), w12-w15 the captured V, C, Z, N.
//! x18 is never touched (the platform register on Apple systems and
//! Android). The frame holds the instruction's slow-path stall at
//! `[sp + ACC]`.

// dynasm's expansion of computed operands.
#![allow(unused_parens)]

use crate::JitCtx;
use dynasmrt::aarch64::Aarch64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use ribbit_cpu::icache::{AluOp, DISCARD, Fast, Hot, LdKind, RotOp, UnOp};
use ribbit_cpu::isa::{BrCond, COND_AL, Cond, Dst, Instr, NO_LOAD, Op, Size, Src, WriteBack, cond_holds};
use ribbit_cpu::reg;
use std::mem::offset_of;

type Asm = VecAssembler<Aarch64Relocation>;

const BUS: u32 = offset_of!(JitCtx, bus) as u32;
const READ8: u32 = offset_of!(JitCtx, read8) as u32;
const READ16: u32 = offset_of!(JitCtx, read16) as u32;
const READ32: u32 = offset_of!(JitCtx, read32) as u32;
const WRITE8: u32 = offset_of!(JitCtx, write8) as u32;
const WRITE16: u32 = offset_of!(JitCtx, write16) as u32;
const WRITE32: u32 = offset_of!(JitCtx, write32) as u32;
const PREP: u32 = offset_of!(JitCtx, prep) as u32;
const TAKE: u32 = offset_of!(JitCtx, take) as u32;
const FETCH_TAKE: u32 = offset_of!(JitCtx, fetch_take) as u32;
const LP_START: u32 = offset_of!(JitCtx, lp_start) as u32;
const LP_END: u32 = offset_of!(JitCtx, lp_end) as u32;
const PC: u32 = offset_of!(JitCtx, pc) as u32;
const EXECUTED: u32 = offset_of!(JitCtx, executed) as u32;
const FROM: u32 = offset_of!(JitCtx, from) as u32;
const DELAY_PENDING: u32 = offset_of!(JitCtx, delay_pending) as u32;
const DELAY_TARGET: u32 = offset_of!(JitCtx, delay_target) as u32;
const REGS: u32 = offset_of!(JitCtx, regs) as u32;
const STATUS32: u32 = offset_of!(JitCtx, status32) as u32;
const READY: u32 = offset_of!(JitCtx, ready) as u32;
const READY_MAX: u32 = offset_of!(JitCtx, ready_max) as u32;
const NOW: u32 = offset_of!(JitCtx, now) as u32;
const END: u32 = offset_of!(JitCtx, end) as u32;
const FENCE: u32 = offset_of!(JitCtx, fence) as u32;
const DISPATCH: u32 = offset_of!(JitCtx, dispatch) as u32;
const CODE_TAGS: u32 = offset_of!(JitCtx, code_tags) as u32;
const DATA_TAGS: u32 = offset_of!(JitCtx, data_tags) as u32;
const REGIONS: u32 = offset_of!(JitCtx, regions) as u32;
const REGION_SIZE: u32 = std::mem::size_of::<crate::CtxRegion>() as u32;
const R_BASE: u32 = offset_of!(crate::CtxRegion, base) as u32;
const R_LIMIT: u32 = offset_of!(crate::CtxRegion, limit) as u32;
const R_PTR: u32 = offset_of!(crate::CtxRegion, ptr) as u32;
const R_WRITABLE: u32 = offset_of!(crate::CtxRegion, writable) as u32;
const TIMED_READ8: u32 = offset_of!(JitCtx, timed_read8) as u32;
const TIMED_READ16: u32 = offset_of!(JitCtx, timed_read16) as u32;
const TIMED_READ32: u32 = offset_of!(JitCtx, timed_read32) as u32;
const TIMED_WRITE8: u32 = offset_of!(JitCtx, timed_write8) as u32;
const TIMED_WRITE16: u32 = offset_of!(JitCtx, timed_write16) as u32;
const TIMED_WRITE32: u32 = offset_of!(JitCtx, timed_write32) as u32;
const WATCH: u32 = offset_of!(JitCtx, watch) as u32;
const WATCH_DATA: u32 = offset_of!(JitCtx, watch_data) as u32;
const LINK: u32 = offset_of!(JitCtx, link) as u32;
const PATCH: u32 = offset_of!(JitCtx, patch) as u32;
const FB_BLOCK: u32 = offset_of!(JitCtx, fb_block) as u32;
const FB_INDEX: u32 = offset_of!(JitCtx, fb_index) as u32;
/// Cache tag bits (`ribbit_memory::JitView`).
const TAG_VALID: u32 = 1 << 31;
const TAG_DIRTY: u32 = 1 << 30;
const CACHE_SETS: u32 = 512;

const EXIT_FLOW: u32 = crate::ExitKind::Flow as u32;
const EXIT_FALLBACK: u32 = crate::ExitKind::Fallback as u32;
const EXIT_BUS: u32 = crate::ExitKind::Bus as u32;
const EXIT_BACKWARD: u32 = crate::ExitKind::Backward as u32;
const EXIT_BUDGET: u32 = crate::ExitKind::Budget as u32;

/// `prep`'s third argument: the fetch is timed in full.
const PREP_FETCH: u32 = 0x100;
/// The frame: x29/x30 and x19-x28 (96 bytes), the slow-path stall, padding.
const FRAME: u32 = 112;
const ACC: u32 = 96;

/// Displacement of guest core register `r` (r0-r31) from x20.
fn r(n: u8) -> u32 {
    4 * (n & 31) as u32
}

const SP: u32 = 4 * reg::SP as u32;
const BLINK: u32 = 4 * reg::BLINK as u32;
const LP_COUNT: u32 = 4 * reg::LP_COUNT as u32;
const MLO: u32 = 4 * reg::MLO as u32;
const MMID: u32 = 4 * reg::MMID as u32;
const MHI: u32 = 4 * reg::MHI as u32;

/// Which flags an operation writes.
#[derive(Clone, Copy)]
enum Writes {
    /// Z, N, C, V.
    All,
    /// Z, N.
    Zn,
    /// Z, N, C.
    Znc,
}

/// Where an exit's PC comes from.
#[derive(Clone, Copy)]
enum Pc {
    Const(u32),
    /// `w23` (a taken branch's target).
    W23,
}

/// One instruction to compile (as `x64::Item`).
#[derive(Clone, Copy, Debug)]
pub struct Item {
    pub hot: Hot,
    pub fetch: bool,
    pub instr: Option<Instr>,
}

impl Item {
    fn mem(&self) -> bool {
        self.hot.fast.accesses_memory() || self.general().is_some_and(|i| matches!(i.op, Op::Ld | Op::St))
    }

    fn general(&self) -> Option<&Instr> {
        if self.hot.fast == Fast::General { self.instr.as_ref() } else { None }
    }
}

#[derive(Clone, Copy)]
enum GSrc {
    Imm(u32),
    Reg(u8),
}

fn gsrc(s: Src, i: &Instr) -> Option<GSrc> {
    match s {
        Src::None => Some(GSrc::Imm(0)),
        Src::Imm(v) | Src::Limm(v) => Some(GSrc::Imm(v)),
        Src::Reg(r) if r < 32 || (57..=60).contains(&r) => Some(GSrc::Reg(r)),
        Src::Reg(63) => Some(GSrc::Imm(i.addr & !3)),
        Src::Reg(_) => None,
    }
}

fn gdst(d: Dst) -> Option<Option<u8>> {
    match d {
        Dst::None | Dst::Discard => Some(None),
        Dst::Reg(r) if r < 32 || r == 60 => Some(Some(r)),
        Dst::Reg(_) => None,
    }
}

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

/// As `x64::generic_ok`.
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
        Op::Lp => true,
        Op::Ld | Op::St => {
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

pub(crate) fn compilable(it: &Item) -> bool {
    !it.hot.stop && (it.hot.fast != Fast::General || it.instr.as_ref().is_some_and(generic_ok))
}

fn cond_table(cc: Cond) -> u32 {
    (0..16u32).filter(|&i| cond_holds(cc, i & 8 != 0, i & 4 != 0, i & 2 != 0, i & 1 != 0)).fold(0, |t, i| t | 1 << i)
}

/// The direct jump at `site` (a `b` instruction the code emitted, see
/// `Gen::flow`) pointed at `target`, or None if out of its reach (±128 MB).
pub(crate) fn link_patch(site: u64, target: u64) -> Option<[u8; 4]> {
    let off = (target as i64).wrapping_sub(site as i64);
    if off % 4 != 0 || !(-(1 << 27)..(1 << 27)).contains(&off) {
        return None;
    }
    Some((0x1400_0000u32 | ((off >> 2) as u32 & 0x03FF_FFFF)).to_le_bytes())
}

struct Stub {
    label: DynamicLabel,
    kind: u32,
    pc: Pc,
    executed: u32,
    from: u32,
    delay: bool,
}

struct LoopStub {
    label: DynamicLabel,
    stay: DynamicLabel,
    executed: u32,
    from: u32,
    mem: bool,
}

struct Gen {
    ops: Asm,
    epilogue: DynamicLabel,
    loops: Vec<LoopStub>,
    stubs: Vec<Stub>,
    mmid_low: bool,
    id: u32,
    links: *mut crate::Link,
    links_cap: usize,
    links_used: usize,
    inline: bool,
    pc: u32,
    seq: u32,
    loads: Vec<(u32, u8)>,
    latency: u32,
    timing: bool,
    fence_check: bool,
    stall_zero: bool,
}

pub(crate) struct Emitted {
    pub code: Vec<u8>,
    pub compiled: usize,
    pub chain: usize,
}

/// As `x64::emit`.
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
    let code = g.ops.finalize().ok()?;
    Some(Emitted { code, compiled, chain })
}

impl Gen {
    fn prologue(&mut self) {
        dynasm!(self.ops
            ; .arch aarch64
            ; stp x29, x30, [sp, -(FRAME as i32)]!
            ; mov x29, sp
            ; stp x19, x20, [sp, 16]
            ; stp x21, x22, [sp, 32]
            ; stp x23, x24, [sp, 48]
            ; stp x25, x26, [sp, 64]
            ; stp x27, x28, [sp, 80]
            ; mov x19, x0
            ; ldr x20, [x19, REGS]
            ; ldr x21, [x19, STATUS32]
            ; ldr x22, [x19, NOW]
            ; ldr x25, [x19, READY]
            ; ldr x26, [x19, READY_MAX]
            ; ldr w27, [x19, LP_END]
            ; ldr x28, [x19, END]
            ; str xzr, [sp, ACC]
        );
    }

    fn epilogue(&mut self) {
        let epilogue = self.epilogue;
        dynasm!(self.ops
            ; .arch aarch64
            ; =>epilogue
            ; str x22, [x19, NOW]
            ; ldp x27, x28, [sp, 80]
            ; ldp x25, x26, [sp, 64]
            ; ldp x23, x24, [sp, 48]
            ; ldp x21, x22, [sp, 32]
            ; ldp x19, x20, [sp, 16]
            ; ldp x29, x30, [sp], FRAME as i32
            ; ret
        );
        loop {
            if let Some(l) = self.loops.pop() {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; =>l.label
                    ; ldr w9, [x20, LP_COUNT]
                    ; sub w10, w9, 1
                    ; str w10, [x20, LP_COUNT]
                    ; cmp w9, 1
                    ; b.eq =>l.stay
                    ; ldr w23, [x19, LP_START]
                );
                if l.mem {
                    let stub = self.stub(EXIT_BUS, Pc::W23, l.executed, l.from, false);
                    dynasm!(self.ops ; .arch aarch64 ; cbnz w8, =>stub);
                }
                self.flow(Pc::W23, l.executed, l.from);
            } else if let Some(s) = self.stubs.pop() {
                dynasm!(self.ops ; .arch aarch64 ; =>s.label);
                if s.delay {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; mov w9, 1
                        ; str w9, [x19, DELAY_PENDING]
                        ; str w23, [x19, DELAY_TARGET]
                    );
                }
                self.exit(s.kind, s.pc, s.executed, s.from);
            } else {
                break;
            }
        }
    }

    /// `[x19 + EXECUTED] += n` (w9 clobbered).
    fn add_executed(&mut self, n: u32) {
        dynasm!(self.ops ; .arch aarch64 ; ldr w9, [x19, EXECUTED]);
        self.add_imm(9, n);
        dynasm!(self.ops ; .arch aarch64 ; str w9, [x19, EXECUTED]);
    }

    fn exit(&mut self, kind: u32, pc: Pc, executed: u32, from: u32) {
        match pc {
            Pc::Const(v) => {
                self.movw32(10, v);
                dynasm!(self.ops ; .arch aarch64 ; str w10, [x19, PC]);
            }
            Pc::W23 => dynasm!(self.ops ; .arch aarch64 ; str w23, [x19, PC]),
        }
        self.add_executed(executed);
        self.movw32(9, from);
        dynasm!(self.ops ; .arch aarch64 ; str w9, [x19, FROM]);
        self.movw32(0, kind);
        let epilogue = self.epilogue;
        dynasm!(self.ops ; .arch aarch64 ; b =>epilogue);
    }

    fn fallback(&mut self, pc: u32, executed: u32, from: u32) {
        self.movw32(9, self.id);
        dynasm!(self.ops ; .arch aarch64 ; str w9, [x19, FB_BLOCK]);
        self.movw32(9, executed);
        dynasm!(self.ops ; .arch aarch64 ; str w9, [x19, FB_INDEX]);
        self.exit(EXIT_FALLBACK, Pc::Const(pc), executed, from);
    }

    fn stub(&mut self, kind: u32, pc: Pc, executed: u32, from: u32, delay: bool) -> DynamicLabel {
        let label = self.ops.new_dynamic_label();
        self.stubs.push(Stub { label, kind, pc, executed, from, delay });
        label
    }

    fn budget_check(&mut self, pc: Pc, executed: u32, from: u32, delay: bool) {
        let stub = self.stub(EXIT_BUDGET, pc, executed, from, delay);
        dynasm!(self.ops ; .arch aarch64 ; cmp x22, x28 ; b.hs =>stub);
    }

    fn after_sequential(&mut self, next: u32, executed: u32, from: u32, mem: bool) {
        let (label, stay) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        self.loops.push(LoopStub { label, stay, executed, from, mem });
        self.movw32(10, next);
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp w27, w10
            ; b.eq =>label
            ; =>stay
        );
        if mem {
            let stub = self.stub(EXIT_BUS, Pc::Const(next), executed, from, false);
            dynasm!(self.ops ; .arch aarch64 ; cbnz w8, =>stub);
        }
        self.budget_check(Pc::Const(next), executed, from, false);
    }

    fn flow(&mut self, target: Pc, executed: u32, from: u32) {
        match target {
            Pc::Const(t) => {
                if t <= from {
                    self.watch(target, executed, from);
                }
            }
            Pc::W23 => {
                let forward = self.ops.new_dynamic_label();
                self.movw32(9, from);
                dynasm!(self.ops ; .arch aarch64 ; cmp w23, w9 ; b.hi =>forward);
                self.watch(target, executed, from);
                dynasm!(self.ops ; .arch aarch64 ; =>forward);
            }
        }
        self.budget_check(target, executed, from, false);
        if self.links_used == self.links_cap {
            self.exit(EXIT_FLOW, target, executed, from);
            return;
        }
        // SAFETY: within the slots the caller allocated (checked above).
        let link = unsafe { self.links.add(self.links_used) } as u64;
        self.links_used += 1;
        let miss = self.ops.new_dynamic_label();
        if let (Pc::Const(_), false) = (target, self.fence_check)
            && self.inline
        {
            // A `b` the `Jit` points at the next block (`link_patch`);
            // until then it goes to the next instruction (the link slot).
            let site = self.ops.new_dynamic_label();
            self.add_executed(executed);
            dynasm!(self.ops ; .arch aarch64 ; =>site ; .u32 0x1400_0001);
            self.movx64(9, link);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x10, [x9, 8]
                ; cbz x10, =>miss
                ; br x10
                ; =>miss
                ; ldr w10, [x19, EXECUTED]
            );
            self.sub_imm(10, executed);
            dynasm!(self.ops
                ; .arch aarch64
                ; str w10, [x19, EXECUTED]
                ; str x9, [x19, LINK]
                ; adr x10, =>site
                ; str x10, [x19, PATCH]
            );
            self.exit(EXIT_FLOW, target, executed, from);
            return;
        }
        self.movx64(9, link);
        if let Pc::W23 = target {
            let cached = self.ops.new_dynamic_label();
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr w10, [x9]
                ; cmp w23, w10
                ; b.eq =>cached
                ; lsr w10, w23, 1
                ; and w10, w10, (crate::DISPATCH_SIZE - 1) as u32
                ; ldr x11, [x19, DISPATCH]
                ; add x11, x11, x10, lsl 4
                ; ldr w12, [x11]
                ; cmp w23, w12
                ; b.ne =>miss
            );
            if self.fence_check {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr w12, [x11, 4]
                    ; ldr w13, [x19, FENCE]
                    ; cmp w12, w13
                    ; b.hs =>miss
                );
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x12, [x11, 8]
                ; cbz x12, =>miss
            );
            self.add_executed(executed);
            dynasm!(self.ops
                ; .arch aarch64
                ; br x12
                ; =>cached
            );
        }
        if self.fence_check {
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr w12, [x9, 4]
                ; ldr w13, [x19, FENCE]
                ; cmp w12, w13
                ; b.hs =>miss
            );
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x12, [x9, 8]
            ; cbz x12, =>miss
        );
        self.add_executed(executed);
        dynasm!(self.ops
            ; .arch aarch64
            ; br x12
            ; =>miss
            ; str x9, [x19, LINK]
            ; str xzr, [x19, PATCH]
        );
        self.exit(EXIT_FLOW, target, executed, from);
    }

    fn watch(&mut self, target: Pc, executed: u32, from: u32) {
        let stub = self.stub(EXIT_BACKWARD, target, executed, from, false);
        dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x19, WATCH_DATA]);
        match target {
            Pc::Const(t) => self.movw32(1, t),
            Pc::W23 => dynasm!(self.ops ; .arch aarch64 ; mov w1, w23),
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; mov x2, x22
            ; ldr x16, [x19, WATCH]
            ; blr x16
            ; cbnz w0, =>stub
        );
    }

    fn instr(&mut self, it: &Item) {
        let h = it.hot;
        self.pc = h.addr;
        let mem = it.mem();
        if mem {
            if self.inline {
                self.prep_inline(&h, it.fetch);
            } else {
                self.prep(&h, it.fetch);
            }
        } else if it.general().is_some() && !self.inline {
            self.prep(&h, it.fetch);
        }
        match it.general() {
            Some(i) => self.generic(i),
            None => self.body(&h),
        }
        self.stall(&h, it.fetch, mem);
        self.cost(&h, 0);
    }

    fn prep(&mut self, h: &Hot, fetch: bool) {
        let len = h.next.wrapping_sub(h.addr) | if fetch { PREP_FETCH } else { 0 };
        dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x19, BUS]);
        self.movw32(1, h.addr);
        self.movw32(2, len);
        dynasm!(self.ops
            ; .arch aarch64
            ; mov x3, x22
            ; ldr x16, [x19, PREP]
            ; blr x16
        );
    }

    /// x9 = the instruction's memory stall (unless `stall_zero`).
    fn stall(&mut self, h: &Hot, fetch: bool, mem: bool) {
        self.stall_zero = false;
        if self.inline {
            return self.stall_inline(h, fetch, mem);
        }
        let mem = mem || h.fast == Fast::General;
        if mem {
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x0, [x19, BUS]
                ; ldr x16, [x19, TAKE]
                ; blr x16
                ; lsr x8, x0, 63
                ; and x9, x0, 0x7FFF_FFFF_FFFF_FFFF
            );
        } else if fetch {
            dynasm!(self.ops ; .arch aarch64 ; ldr x0, [x19, BUS]);
            self.movw32(1, h.addr);
            self.movw32(2, h.next.wrapping_sub(h.addr));
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x16, [x19, FETCH_TAKE]
                ; blr x16
                ; mov x9, x0
            );
        } else {
            self.stall_zero = true;
        }
    }

    /// As `x64::Gen::cost` (x10 = the wait, x9 = the stall).
    fn cost(&mut self, h: &Hot, extra: u64) {
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
                dynasm!(self.ops
                    ; .arch aarch64
                    ; mov x10, xzr
                    ; ldr x12, [x26]
                    ; cmp x12, x22
                    ; b.ls =>done
                );
            }
            dynasm!(self.ops ; .arch aarch64 ; ldr x10, [x25, 8 * first as u32]);
            for &r in rest {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr x12, [x25, 8 * r as u32]
                    ; cmp x10, x12
                    ; csel x10, x12, x10, lo
                );
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; subs x10, x10, x22
                ; b.hs =>done
                ; mov x10, xzr
                ; =>done
            );
        }
        let wait_zero = regs.is_empty();
        let stall_zero = self.stall_zero;
        if h.load_dst != NO_LOAD {
            let d = 8 * h.load_dst as u32;
            let keep = self.ops.new_dynamic_label();
            dynasm!(self.ops ; .arch aarch64 ; mov x12, x22);
            if !wait_zero {
                dynasm!(self.ops ; .arch aarch64 ; add x12, x12, x10);
            }
            if !stall_zero {
                dynasm!(self.ops ; .arch aarch64 ; add x12, x12, x9);
            }
            self.add_imm64(12, latency as u64);
            dynasm!(self.ops
                ; .arch aarch64
                ; str x12, [x25, d]
                ; ldr x13, [x26]
                ; cmp x13, x12
                ; b.hs =>keep
                ; str x12, [x26]
                ; =>keep
            );
            self.loads.push((seq, h.load_dst));
        }
        if !stall_zero {
            dynasm!(self.ops ; .arch aarch64 ; add x22, x22, x9);
        }
        if !wait_zero {
            dynasm!(self.ops ; .arch aarch64 ; add x22, x22, x10);
        }
        self.add_imm64(22, 1 + extra);
    }

    fn prep_inline(&mut self, h: &Hot, fetch: bool) {
        if !fetch || !self.timing {
            return;
        }
        let (hit, miss) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        self.code_lookup(h, hit, miss);
        dynasm!(self.ops ; .arch aarch64 ; =>miss ; ldr x0, [x19, BUS]);
        self.movw32(1, h.addr);
        self.movw32(2, h.next.wrapping_sub(h.addr));
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x19, FETCH_TAKE]
            ; blr x16
            ; ldr x9, [sp, ACC]
            ; add x9, x9, x0
            ; str x9, [sp, ACC]
            ; =>hit
        );
    }

    /// Jumps to `hit` if every line the fetch touches is in the code cache,
    /// else to `miss` (w9-w13, x16 clobbered).
    fn code_lookup(&mut self, h: &Hot, hit: DynamicLabel, miss: DynamicLabel) {
        let first = h.addr >> 4;
        let last = h.next.wrapping_sub(1) >> 4;
        dynasm!(self.ops ; .arch aarch64 ; ldr x16, [x19, CODE_TAGS]);
        for line in [first, last].into_iter().take(if first == last { 1 } else { 2 }) {
            let set = (line % CACHE_SETS) * 16;
            self.movw32(12, line | TAG_VALID);
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr w9, [x16, set]
                ; ldr w10, [x16, set + 4]
                ; ldr w11, [x16, set + 8]
                ; ldr w13, [x16, set + 12]
                ; cmp w9, w12
                ; ccmp w10, w12, 4, ne
                ; ccmp w11, w12, 4, ne
                ; ccmp w13, w12, 4, ne
                ; b.ne =>miss
            );
        }
        dynasm!(self.ops ; .arch aarch64 ; b =>hit);
    }

    fn stall_inline(&mut self, h: &Hot, fetch: bool, mem: bool) {
        if mem {
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x9, [sp, ACC]
                ; str xzr, [sp, ACC]
                ; lsr x8, x9, 63
                ; and x9, x9, 0x7FFF_FFFF_FFFF_FFFF
            );
            return;
        }
        if !(fetch && self.timing) {
            self.stall_zero = true;
            return;
        }
        let (hit, miss, done) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        self.code_lookup(h, hit, miss);
        dynasm!(self.ops ; .arch aarch64 ; =>miss ; ldr x0, [x19, BUS]);
        self.movw32(1, h.addr);
        self.movw32(2, h.next.wrapping_sub(h.addr));
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x16, [x19, FETCH_TAKE]
            ; blr x16
            ; mov x9, x0
            ; b =>done
            ; =>hit
            ; mov x9, xzr
            ; =>done
        );
    }

    /// As `x64::Gen::region_lookup`, the address in w1: x14 = its byte in
    /// the region, then the data-cache lookup (the four ways compared with
    /// a `ccmp` chain: no branch per way); to `slow` otherwise (w9-w13,
    /// w15-w17, x16 clobbered).
    fn region_lookup(&mut self, width: u32, write: bool, slow: DynamicLabel) {
        let found = self.ops.new_dynamic_label();
        match width {
            4 => dynasm!(self.ops ; .arch aarch64 ; and w12, w1, 0xFFFF_FFFC),
            2 => dynasm!(self.ops ; .arch aarch64 ; and w12, w1, 0xFFFF_FFFE),
            _ => dynasm!(self.ops ; .arch aarch64 ; mov w12, w1),
        }
        for i in 0..3 {
            let at = REGIONS + i * REGION_SIZE;
            let next = self.ops.new_dynamic_label();
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr w13, [x19, at + R_BASE]
                ; sub w13, w12, w13
                ; ldr w16, [x19, at + R_LIMIT]
                ; cmp w13, w16
                ; b.hs =>next
            );
            if write {
                dynasm!(self.ops ; .arch aarch64 ; ldr w16, [x19, at + R_WRITABLE] ; cbz w16, =>slow);
            }
            dynasm!(self.ops
                ; .arch aarch64
                ; ldr x14, [x19, at + R_PTR]
                ; add x14, x14, w13, uxtw
                ; b =>found
                ; =>next
            );
        }
        dynasm!(self.ops ; .arch aarch64 ; b =>slow ; =>found);
        if !self.timing {
            return;
        }
        let done = self.ops.new_dynamic_label();
        dynasm!(self.ops
            ; .arch aarch64
            ; lsr w12, w1, 4
            ; and w13, w12, (CACHE_SETS - 1)
            ; orr w12, w12, (TAG_VALID | TAG_DIRTY)
            ; ldr x16, [x19, DATA_TAGS]
            ; add x16, x16, w13, uxtw 4
            ; ldp w9, w10, [x16]
            ; ldp w11, w13, [x16, 8]
            ; orr w9, w9, TAG_DIRTY
            ; orr w10, w10, TAG_DIRTY
            ; orr w11, w11, TAG_DIRTY
            ; orr w13, w13, TAG_DIRTY
            ; cmp w9, w12
            ; ccmp w10, w12, 4, ne
            ; ccmp w11, w12, 4, ne
            ; ccmp w13, w12, 4, ne
            ; b.ne =>slow
        );
        if write {
            // The way (w17), then its dirty bit, stored only when not set.
            dynasm!(self.ops
                ; .arch aarch64
                ; mov w17, 3
                ; mov w15, 2
                ; cmp w11, w12
                ; csel w17, w15, w17, eq
                ; mov w15, 1
                ; cmp w10, w12
                ; csel w17, w15, w17, eq
                ; cmp w9, w12
                ; csel w17, wzr, w17, eq
                ; ldr w15, [x16, w17, uxtw 2]
                ; tbnz w15, 30, =>done
                ; orr w15, w15, TAG_DIRTY
                ; str w15, [x16, w17, uxtw 2]
            );
        }
        dynasm!(self.ops ; .arch aarch64 ; =>done);
    }

    /// w9 = the load at w1.
    fn load_inline(&mut self, kind: LdKind) {
        let (slow, done) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let (width, helper) = match kind {
            LdKind::Word => (4, TIMED_READ32),
            LdKind::Half | LdKind::HalfSigned => (2, TIMED_READ16),
            LdKind::Byte | LdKind::ByteSigned => (1, TIMED_READ8),
        };
        self.region_lookup(width, false, slow);
        match width {
            4 => dynasm!(self.ops ; .arch aarch64 ; ldr w9, [x14]),
            2 => dynasm!(self.ops ; .arch aarch64 ; ldrh w9, [x14]),
            _ => dynasm!(self.ops ; .arch aarch64 ; ldrb w9, [x14]),
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; b =>done
            ; =>slow
            ; ldr x0, [x19, BUS]
            ; mov x2, x22
        );
        self.movw32(3, self.pc);
        dynasm!(self.ops
            ; .arch aarch64
            ; add x4, sp, ACC
            ; ldr x16, [x19, helper]
            ; blr x16
            ; mov w9, w0
            ; =>done
        );
        match kind {
            LdKind::HalfSigned => dynasm!(self.ops ; .arch aarch64 ; sxth w9, w9),
            LdKind::ByteSigned => dynasm!(self.ops ; .arch aarch64 ; sxtb w9, w9),
            _ => {}
        }
    }

    /// The store of w2 at w1.
    fn store_inline(&mut self, size: Size) {
        let (slow, done) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let (width, helper) = match size {
            Size::Word => (4, TIMED_WRITE32),
            Size::Half => (2, TIMED_WRITE16),
            Size::Byte => (1, TIMED_WRITE8),
        };
        self.region_lookup(width, true, slow);
        match width {
            4 => dynasm!(self.ops ; .arch aarch64 ; str w2, [x14]),
            2 => dynasm!(self.ops ; .arch aarch64 ; strh w2, [x14]),
            _ => dynasm!(self.ops ; .arch aarch64 ; strb w2, [x14]),
        }
        dynasm!(self.ops
            ; .arch aarch64
            ; b =>done
            ; =>slow
            ; ldr x0, [x19, BUS]
            ; mov x3, x22
        );
        self.movw32(4, self.pc);
        dynasm!(self.ops
            ; .arch aarch64
            ; add x5, sp, ACC
            ; ldr x16, [x19, helper]
            ; blr x16
            ; =>done
        );
    }

    /// Jumps to `skip` unless condition `cc` holds (w9, w10 clobbered).
    fn cond_skip(&mut self, cc: Cond, skip: DynamicLabel) {
        if cc == COND_AL {
            return;
        }
        dynasm!(self.ops ; .arch aarch64 ; ldr w9, [x21] ; ubfx w9, w9, 8, 4);
        self.movw32(10, cond_table(cc));
        dynasm!(self.ops ; .arch aarch64 ; lsr w10, w10, w9 ; tbz w10, 0, =>skip);
    }

    /// Captures Z, N (w14, w15) and, for `All`, C and V (w13, w12) from the
    /// flags an `adds`/`subs` (or `cmp`) left. A subtraction's C is its
    /// borrow (AArch64's C is the opposite).
    fn flags_nzcv(&mut self, writes: Writes, sub: bool) {
        dynasm!(self.ops ; .arch aarch64 ; cset w14, eq ; cset w15, mi);
        if let Writes::All = writes {
            if sub {
                dynasm!(self.ops ; .arch aarch64 ; cset w13, lo);
            } else {
                dynasm!(self.ops ; .arch aarch64 ; cset w13, hs);
            }
            dynasm!(self.ops ; .arch aarch64 ; cset w12, vs);
        }
    }

    /// Captures Z, N of W(reg) (C and V are set by the caller when needed).
    fn flags_of(&mut self, reg: u8) {
        dynasm!(self.ops
            ; .arch aarch64
            ; cmp WSP(reg), #0
            ; cset w14, eq
            ; cset w15, mi
        );
    }

    /// Merges the captured flags into STATUS32 (w16 clobbered).
    fn merge_flags(&mut self, writes: Writes) {
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr w16, [x21]
            ; bfi w16, w14, 11, 1
            ; bfi w16, w15, 10, 1
        );
        if matches!(writes, Writes::All | Writes::Znc) {
            dynasm!(self.ops ; .arch aarch64 ; bfi w16, w13, 9, 1);
        }
        if matches!(writes, Writes::All) {
            dynasm!(self.ops ; .arch aarch64 ; bfi w16, w12, 8, 1);
        }
        dynasm!(self.ops ; .arch aarch64 ; str w16, [x21]);
    }

    /// w16 = the guest C (0 or 1).
    fn guest_carry(&mut self) {
        dynasm!(self.ops ; .arch aarch64 ; ldr w16, [x21] ; ubfx w16, w16, 9, 1);
    }

    /// NZCV.C = the guest C (`inverted`: its opposite): `cmp` of a 0 or 1
    /// with 1 leaves C set exactly for the 1.
    fn load_carry(&mut self, inverted: bool) {
        self.guest_carry();
        if inverted {
            dynasm!(self.ops ; .arch aarch64 ; eor w16, w16, 1);
        }
        dynasm!(self.ops ; .arch aarch64 ; cmp w16, 1);
    }

    /// w9 = b, w10 = c.
    fn operands(&mut self, b: u8, c: Operand) {
        dynasm!(self.ops ; .arch aarch64 ; ldr w9, [x20, r(b)]);
        self.operand_w10(c);
    }

    fn operand_w10(&mut self, c: Operand) {
        match c {
            Operand::Reg(c) => dynasm!(self.ops ; .arch aarch64 ; ldr w10, [x20, r(c)]),
            Operand::Imm(v) => self.movw32(10, v),
        }
    }

    /// w9 = op(w9, w10) without flags (x86's shifts take the count mod 32,
    /// as AArch64's register shifts do).
    fn alu(&mut self, op: AluOp) {
        match op {
            AluOp::Add => dynasm!(self.ops ; .arch aarch64 ; add w9, w9, w10),
            AluOp::Add1 => dynasm!(self.ops ; .arch aarch64 ; add w9, w9, w10, lsl 1),
            AluOp::Add2 => dynasm!(self.ops ; .arch aarch64 ; add w9, w9, w10, lsl 2),
            AluOp::Add3 => dynasm!(self.ops ; .arch aarch64 ; add w9, w9, w10, lsl 3),
            AluOp::Sub => dynasm!(self.ops ; .arch aarch64 ; sub w9, w9, w10),
            AluOp::Sub1 => dynasm!(self.ops ; .arch aarch64 ; sub w9, w9, w10, lsl 1),
            AluOp::Sub2 => dynasm!(self.ops ; .arch aarch64 ; sub w9, w9, w10, lsl 2),
            AluOp::Sub3 => dynasm!(self.ops ; .arch aarch64 ; sub w9, w9, w10, lsl 3),
            AluOp::Rsub => dynasm!(self.ops ; .arch aarch64 ; sub w9, w10, w9),
            AluOp::And => dynasm!(self.ops ; .arch aarch64 ; and w9, w9, w10),
            AluOp::Or => dynasm!(self.ops ; .arch aarch64 ; orr w9, w9, w10),
            AluOp::Xor => dynasm!(self.ops ; .arch aarch64 ; eor w9, w9, w10),
            AluOp::Bic => dynasm!(self.ops ; .arch aarch64 ; bic w9, w9, w10),
            AluOp::Asl => dynasm!(self.ops ; .arch aarch64 ; lsl w9, w9, w10),
            AluOp::Lsr => dynasm!(self.ops ; .arch aarch64 ; lsr w9, w9, w10),
            AluOp::Asr => dynasm!(self.ops ; .arch aarch64 ; asr w9, w9, w10),
            AluOp::Adc => {
                self.guest_carry();
                dynasm!(self.ops ; .arch aarch64 ; add w9, w9, w10 ; add w9, w9, w16);
            }
            AluOp::Sbc => {
                self.guest_carry();
                dynasm!(self.ops ; .arch aarch64 ; sub w9, w9, w10 ; sub w9, w9, w16);
            }
        }
    }

    fn alu_flags(&mut self, op: AluOp, d: u8) {
        self.alu_flags_to(op, (d != DISCARD).then_some(d & 31));
    }

    /// `alu_flags` storing to any plain register (`d`), or nowhere.
    fn alu_flags_to(&mut self, op: AluOp, d: Option<u8>) {
        let writes = match op {
            AluOp::Add => self.adds(None),
            AluOp::Add1 => self.adds(Some(1)),
            AluOp::Add2 => self.adds(Some(2)),
            AluOp::Add3 => self.adds(Some(3)),
            AluOp::Sub => self.subs(None),
            AluOp::Sub1 => self.subs(Some(1)),
            AluOp::Sub2 => self.subs(Some(2)),
            AluOp::Sub3 => self.subs(Some(3)),
            AluOp::Adc => {
                self.load_carry(false);
                dynasm!(self.ops ; .arch aarch64 ; adcs w9, w9, w10);
                self.flags_nzcv(Writes::All, false);
                Writes::All
            }
            AluOp::Sbc => {
                // sbcs takes away the inverted carry: the guest C, inverted.
                self.load_carry(true);
                dynasm!(self.ops ; .arch aarch64 ; sbcs w9, w9, w10);
                self.flags_nzcv(Writes::All, true);
                Writes::All
            }
            AluOp::Rsub => {
                dynasm!(self.ops ; .arch aarch64 ; subs w9, w10, w9);
                self.flags_nzcv(Writes::All, true);
                Writes::All
            }
            AluOp::And | AluOp::Or | AluOp::Xor | AluOp::Bic => {
                self.alu(op);
                self.flags_of(9);
                Writes::Zn
            }
            // C = the last bit shifted out, unchanged for a zero count.
            AluOp::Asl | AluOp::Lsr | AluOp::Asr => {
                self.guest_carry();
                dynasm!(self.ops ; .arch aarch64 ; ands w17, w10, 31);
                match op {
                    // Bit 32 - n of the value.
                    AluOp::Asl => dynasm!(self.ops
                        ; .arch aarch64
                        ; mov w13, 32
                        ; sub w13, w13, w17
                        ; lsr w13, w9, w13
                    ),
                    // Bit n - 1.
                    _ => dynasm!(self.ops
                        ; .arch aarch64
                        ; sub w13, w17, 1
                        ; lsr w13, w9, w13
                    ),
                }
                dynasm!(self.ops
                    ; .arch aarch64
                    ; and w13, w13, 1
                    ; cmp w17, 0
                    ; csel w13, w16, w13, eq
                );
                match op {
                    AluOp::Asl => dynasm!(self.ops ; .arch aarch64 ; lsl w9, w9, w10),
                    AluOp::Lsr => dynasm!(self.ops ; .arch aarch64 ; lsr w9, w9, w10),
                    _ => dynasm!(self.ops ; .arch aarch64 ; asr w9, w9, w10),
                }
                self.flags_of(9);
                Writes::Znc
            }
        };
        self.gwrite(d);
        self.merge_flags(writes);
    }

    /// w9 = w9 + (w10 << k) with its flags captured.
    fn adds(&mut self, k: Option<u32>) -> Writes {
        match k {
            None => dynasm!(self.ops ; .arch aarch64 ; adds w9, w9, w10),
            Some(1) => dynasm!(self.ops ; .arch aarch64 ; adds w9, w9, w10, lsl 1),
            Some(2) => dynasm!(self.ops ; .arch aarch64 ; adds w9, w9, w10, lsl 2),
            Some(_) => dynasm!(self.ops ; .arch aarch64 ; adds w9, w9, w10, lsl 3),
        }
        self.flags_nzcv(Writes::All, false);
        Writes::All
    }

    /// w9 = w9 - (w10 << k) with its flags captured.
    fn subs(&mut self, k: Option<u32>) -> Writes {
        match k {
            None => dynasm!(self.ops ; .arch aarch64 ; subs w9, w9, w10),
            Some(1) => dynasm!(self.ops ; .arch aarch64 ; subs w9, w9, w10, lsl 1),
            Some(2) => dynasm!(self.ops ; .arch aarch64 ; subs w9, w9, w10, lsl 2),
            Some(_) => dynasm!(self.ops ; .arch aarch64 ; subs w9, w9, w10, lsl 3),
        }
        self.flags_nzcv(Writes::All, true);
        Writes::All
    }

    /// Calls a memory helper: address in w1, value (writes) in w2; a
    /// read's result comes back in w9.
    fn call(&mut self, helper: u32) {
        dynasm!(self.ops
            ; .arch aarch64
            ; ldr x0, [x19, BUS]
            ; ldr x16, [x19, helper]
            ; blr x16
            ; mov w9, w0
        );
    }

    fn load(&mut self, kind: LdKind) {
        if self.inline {
            return self.load_inline(kind);
        }
        match kind {
            LdKind::Word => self.call(READ32),
            LdKind::Half => self.call(READ16),
            LdKind::HalfSigned => {
                self.call(READ16);
                dynasm!(self.ops ; .arch aarch64 ; sxth w9, w9);
            }
            LdKind::Byte => self.call(READ8),
            LdKind::ByteSigned => {
                self.call(READ8);
                dynasm!(self.ops ; .arch aarch64 ; sxtb w9, w9);
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

    /// w1 = the effective address with base `b`, offset `off` and
    /// write-back `wb`; the base is updated (w9 clobbered for .AB).
    fn address(&mut self, wb: WriteBack, b: u8, off: u32, size: Size) {
        dynasm!(self.ops ; .arch aarch64 ; ldr w1, [x20, r(b)]);
        match wb {
            WriteBack::None => self.add_imm(1, off),
            WriteBack::Pre => {
                self.add_imm(1, off);
                dynasm!(self.ops ; .arch aarch64 ; str w1, [x20, r(b)]);
            }
            WriteBack::Post => {
                self.movw32(9, off);
                dynasm!(self.ops ; .arch aarch64 ; add w9, w1, w9 ; str w9, [x20, r(b)]);
            }
            WriteBack::Scaled => {
                let scale = match size {
                    Size::Word => 2,
                    Size::Half => 1,
                    Size::Byte => 0,
                };
                self.add_imm(1, off << scale);
            }
        }
    }

    /// `reg` (0: w9, 1: w10, 2: w2, 3: w1) = a generic source.
    fn gread(&mut self, s: GSrc, reg: u8) {
        let w = match reg {
            0 => 9,
            1 => 10,
            2 => 2,
            _ => 1,
        };
        match s {
            GSrc::Imm(v) => self.movw32(w, v),
            GSrc::Reg(r) => dynasm!(self.ops ; .arch aarch64 ; ldr W(w), [x20, 4 * r as u32]),
        }
    }

    /// Writes w9 to a generic destination (none: nothing).
    fn gwrite(&mut self, d: Option<u8>) {
        if let Some(r) = d {
            dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, 4 * r as u32]);
        }
    }

    fn result_zn(&mut self, d: Option<u8>, f: bool) {
        if f {
            self.flags_of(9);
        }
        self.gwrite(d);
        if f {
            self.merge_flags(Writes::Zn);
        }
    }

    /// The one-bit shifts and rotates (`Fast::Rot`, and the generic
    /// single-operand ones) of w9, as x86's one-bit forms compute them:
    /// C the bit shifted out, ASL's V the result's sign changing (x86's OF).
    /// The flags are captured when `f`; returns what they are.
    fn rot1(&mut self, op: RotOp, f: bool) -> Writes {
        match op {
            RotOp::Asl => {
                if f {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; lsr w13, w9, 31
                        ; eor w12, w9, w9, lsl 1
                        ; lsr w12, w12, 31
                    );
                }
                dynasm!(self.ops ; .arch aarch64 ; lsl w9, w9, 1);
            }
            RotOp::Asr | RotOp::Lsr | RotOp::Ror => {
                if f {
                    dynasm!(self.ops ; .arch aarch64 ; and w13, w9, 1);
                }
                match op {
                    RotOp::Asr => dynasm!(self.ops ; .arch aarch64 ; asr w9, w9, 1),
                    RotOp::Lsr => dynasm!(self.ops ; .arch aarch64 ; lsr w9, w9, 1),
                    _ => dynasm!(self.ops ; .arch aarch64 ; ror w9, w9, 1),
                }
            }
            RotOp::Rrc => {
                self.guest_carry();
                dynasm!(self.ops
                    ; .arch aarch64
                    ; and w13, w9, 1
                    ; lsr w9, w9, 1
                    ; orr w9, w9, w16, lsl 31
                );
            }
            RotOp::Rlc => {
                self.guest_carry();
                dynasm!(self.ops
                    ; .arch aarch64
                    ; lsr w13, w9, 31
                    ; lsl w9, w9, 1
                    ; orr w9, w9, w16
                );
            }
        }
        if f {
            self.flags_of(9);
        }
        if op == RotOp::Asl { Writes::All } else { Writes::Znc }
    }

    fn generic(&mut self, i: &Instr) {
        let skip = self.ops.new_dynamic_label();
        self.cond_skip(i.cc, skip);
        let (b, c) = (gsrc(i.src1, i).expect("generic_ok"), gsrc(i.src2, i).expect("generic_ok"));
        let d = gdst(i.dst).expect("generic_ok");
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
                dynasm!(self.ops ; .arch aarch64 ; mov w11, 1);
                match i.op {
                    Op::Bset => dynasm!(self.ops ; .arch aarch64 ; lsl w11, w11, w10 ; orr w9, w9, w11),
                    Op::Bclr => dynasm!(self.ops ; .arch aarch64 ; lsl w11, w11, w10 ; bic w9, w9, w11),
                    Op::Bxor => dynasm!(self.ops ; .arch aarch64 ; lsl w11, w11, w10 ; eor w9, w9, w11),
                    // (2 << n) - 1: all ones for n = 31 (the shift gives 0).
                    _ => dynasm!(self.ops
                        ; .arch aarch64
                        ; mov w11, 2
                        ; lsl w11, w11, w10
                        ; sub w11, w11, 1
                        ; and w9, w9, w11
                    ),
                }
                self.result_zn(d, f);
            }
            Op::RorM => {
                self.gread(b, 0);
                self.gread(c, 1);
                dynasm!(self.ops ; .arch aarch64 ; ror w9, w9, w10);
                if f {
                    // C = the result's top bit (x86 ROR's CF), unchanged
                    // for a zero count.
                    self.guest_carry();
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; lsr w13, w9, 31
                        ; tst w10, 31
                        ; csel w13, w16, w13, eq
                    );
                    self.flags_of(9);
                    self.gwrite(d);
                    self.merge_flags(Writes::Znc);
                } else {
                    self.gwrite(d);
                }
            }
            Op::Cmp | Op::Rcmp => {
                self.gread(b, 0);
                self.gread(c, 1);
                if i.op == Op::Cmp {
                    dynasm!(self.ops ; .arch aarch64 ; cmp w9, w10);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; cmp w10, w9);
                }
                self.flags_nzcv(Writes::All, true);
                self.merge_flags(Writes::All);
            }
            Op::Tst | Op::Btst => {
                self.gread(b, 0);
                self.gread(c, 1);
                if i.op == Op::Btst {
                    dynasm!(self.ops ; .arch aarch64 ; mov w11, 1 ; lsl w11, w11, w10 ; and w9, w9, w11);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; and w9, w9, w10);
                }
                self.flags_of(9);
                self.merge_flags(Writes::Zn);
            }
            Op::Asl | Op::Asr | Op::Lsr | Op::Ror | Op::Rrc | Op::Rlc => {
                self.gread(c, 0);
                let op = match i.op {
                    Op::Asl => RotOp::Asl,
                    Op::Asr => RotOp::Asr,
                    Op::Lsr => RotOp::Lsr,
                    Op::Ror => RotOp::Ror,
                    Op::Rrc => RotOp::Rrc,
                    _ => RotOp::Rlc,
                };
                let writes = self.rot1(op, f);
                self.gwrite(d);
                if f {
                    self.merge_flags(writes);
                }
            }
            Op::Max | Op::Min => {
                self.gread(b, 0);
                self.gread(c, 1);
                // The flags of b - c; with .F: Z/N/V from it, C = c chosen.
                dynasm!(self.ops ; .arch aarch64 ; cmp w9, w10);
                if f {
                    dynasm!(self.ops ; .arch aarch64 ; cset w12, vs ; cset w14, eq ; cset w15, mi);
                    if i.op == Op::Max {
                        dynasm!(self.ops ; .arch aarch64 ; cset w13, le);
                    } else {
                        dynasm!(self.ops ; .arch aarch64 ; cset w13, ge);
                    }
                }
                if i.op == Op::Max {
                    dynasm!(self.ops ; .arch aarch64 ; csel w9, w10, w9, lt);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; csel w9, w10, w9, gt);
                }
                self.gwrite(d);
                if f {
                    self.merge_flags(Writes::All);
                }
            }
            Op::Normw => {
                // norm16: the leading sign bits of the low half less one,
                // 15 for 0 and all ones (clz of the sign-flipped value: 32).
                self.gread(c, 0);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; sxth w11, w9
                    ; eor w9, w11, w11, asr 31
                    ; clz w9, w9
                    ; sub w9, w9, 17
                );
                self.gwrite(d);
                if i.set_flags {
                    self.flags_of(11);
                    self.merge_flags(Writes::Zn);
                }
            }
            Op::Norm => {
                // norm32: 31 for 0 and all ones, else the sign bits less one.
                self.gread(c, 0);
                dynasm!(self.ops
                    ; .arch aarch64
                    ; mov w11, w9
                    ; eor w9, w11, w11, asr 31
                    ; clz w9, w9
                    ; sub w9, w9, 1
                );
                self.gwrite(d);
                if i.set_flags {
                    self.flags_of(11);
                    self.merge_flags(Writes::Zn);
                }
            }
            Op::Abs => {
                // C = the source's sign, V = it was 0x8000_0000 (PRM, ABS).
                self.gread(c, 0);
                if f {
                    dynasm!(self.ops
                        ; .arch aarch64
                        ; lsr w13, w9, 31
                    );
                    self.movw32(12, 0x8000_0000);
                    dynasm!(self.ops ; .arch aarch64 ; cmp w9, w12 ; cset w12, eq);
                }
                dynasm!(self.ops ; .arch aarch64 ; cmp w9, 0 ; cneg w9, w9, mi);
                if f {
                    self.flags_of(9);
                }
                self.gwrite(d);
                if f {
                    self.merge_flags(Writes::All);
                }
            }
            _ => {
                self.gread(c, 0);
                match i.op {
                    Op::Sexb => dynasm!(self.ops ; .arch aarch64 ; sxtb w9, w9),
                    Op::Sexw => dynasm!(self.ops ; .arch aarch64 ; sxth w9, w9),
                    Op::Extb => dynasm!(self.ops ; .arch aarch64 ; uxtb w9, w9),
                    Op::Extw => dynasm!(self.ops ; .arch aarch64 ; uxth w9, w9),
                    Op::Not => dynasm!(self.ops ; .arch aarch64 ; mvn w9, w9),
                    Op::Neg => dynasm!(self.ops ; .arch aarch64 ; neg w9, w9),
                    Op::Swap => dynasm!(self.ops ; .arch aarch64 ; ror w9, w9, 16),
                    other => unreachable!("generic_ok let {other:?} through"),
                }
                self.result_zn(d, f);
            }
        }
        dynasm!(self.ops ; .arch aarch64 ; =>skip);
    }

    /// w1 = the effective address of a generic load/store, the base written
    /// back; w9 and w10 clobbered, w2 kept.
    fn gaddress(&mut self, i: &Instr, b: GSrc, c: GSrc) {
        self.gread(b, 3);
        self.gread(c, 1);
        let base = match i.src1 {
            Src::Reg(r) => Some(r),
            _ => None,
        };
        match i.mem.wb {
            WriteBack::None => dynasm!(self.ops ; .arch aarch64 ; add w1, w1, w10),
            WriteBack::Scaled => match i.mem.size {
                Size::Word => dynasm!(self.ops ; .arch aarch64 ; add w1, w1, w10, lsl 2),
                Size::Half => dynasm!(self.ops ; .arch aarch64 ; add w1, w1, w10, lsl 1),
                Size::Byte => dynasm!(self.ops ; .arch aarch64 ; add w1, w1, w10),
            },
            WriteBack::Pre => {
                dynasm!(self.ops ; .arch aarch64 ; add w1, w1, w10);
                if let Some(r) = base {
                    dynasm!(self.ops ; .arch aarch64 ; str w1, [x20, 4 * r as u32]);
                }
            }
            WriteBack::Post => {
                dynasm!(self.ops ; .arch aarch64 ; add w9, w1, w10);
                if let Some(r) = base {
                    dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, 4 * r as u32]);
                }
            }
        }
    }

    fn gload(&mut self, i: &Instr, b: GSrc, c: GSrc, d: Option<u8>) {
        self.gaddress(i, b, c);
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
        self.gread(gsrc(i.data, i).expect("generic_ok"), 2);
        self.gaddress(i, b, c);
        self.store(i.mem.size);
    }

    fn body(&mut self, h: &Hot) {
        match h.fast {
            Fast::Mov { d, s } => dynasm!(self.ops
                ; .arch aarch64
                ; ldr w9, [x20, r(s)]
                ; str w9, [x20, r(d)]
            ),
            Fast::MovI { d, v } => {
                self.movw32(9, v);
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
            }
            Fast::Alu { op, d, b, c } => {
                self.operands(b, Operand::Reg(c));
                self.alu(op);
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
            }
            Fast::AluI { op, d, b, v } => {
                self.operands(b, Operand::Imm(v));
                self.alu(op);
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
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
                dynasm!(self.ops ; .arch aarch64 ; ldr w9, [x20, r(c)]);
                match op {
                    UnOp::Asl => dynasm!(self.ops ; .arch aarch64 ; lsl w9, w9, 1),
                    UnOp::Asr => dynasm!(self.ops ; .arch aarch64 ; asr w9, w9, 1),
                    UnOp::Lsr => dynasm!(self.ops ; .arch aarch64 ; lsr w9, w9, 1),
                    UnOp::Ror => dynasm!(self.ops ; .arch aarch64 ; ror w9, w9, 1),
                    UnOp::Sexb => dynasm!(self.ops ; .arch aarch64 ; sxtb w9, w9),
                    UnOp::Sexw => dynasm!(self.ops ; .arch aarch64 ; sxth w9, w9),
                    UnOp::Extb => dynasm!(self.ops ; .arch aarch64 ; uxtb w9, w9),
                    UnOp::Extw => dynasm!(self.ops ; .arch aarch64 ; uxth w9, w9),
                    UnOp::Not => dynasm!(self.ops ; .arch aarch64 ; mvn w9, w9),
                    UnOp::Neg => dynasm!(self.ops ; .arch aarch64 ; neg w9, w9),
                    // `wrapping_abs`: i32::MIN stays i32::MIN.
                    UnOp::Abs => dynasm!(self.ops ; .arch aarch64 ; cmp w9, 0 ; cneg w9, w9, mi),
                    UnOp::Swap => dynasm!(self.ops ; .arch aarch64 ; ror w9, w9, 16),
                }
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
            }
            Fast::Rot { op, f, d, c } => {
                dynasm!(self.ops ; .arch aarch64 ; ldr w9, [x20, r(c)]);
                let writes = self.rot1(op, f);
                if d != DISCARD {
                    dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
                }
                if f {
                    self.merge_flags(writes);
                }
            }
            Fast::Mul64 { signed, b, c } => self.mul64(signed, b, Operand::Reg(c)),
            Fast::Mul64I { signed, b, v } => self.mul64(signed, b, Operand::Imm(v)),
            Fast::Ld { kind, d, b, off } => {
                dynasm!(self.ops ; .arch aarch64 ; ldr w1, [x20, r(b)]);
                self.add_imm(1, off);
                self.load(kind);
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
            }
            Fast::LdR { kind, d, b, c } => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr w1, [x20, r(b)]
                    ; ldr w10, [x20, r(c)]
                    ; add w1, w1, w10
                );
                self.load(kind);
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
            }
            Fast::LdX { kind, wb, d, b, off } => {
                let size = match kind {
                    LdKind::Word => Size::Word,
                    LdKind::Half | LdKind::HalfSigned => Size::Half,
                    LdKind::Byte | LdKind::ByteSigned => Size::Byte,
                };
                self.address(wb, b, off, size);
                self.load(kind);
                dynasm!(self.ops ; .arch aarch64 ; str w9, [x20, r(d)]);
            }
            Fast::St { size, s, b, off } => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr w2, [x20, r(s)]
                    ; ldr w1, [x20, r(b)]
                );
                self.add_imm(1, off);
                self.store(size);
            }
            Fast::StX { size, wb, s, b, off } => {
                // The value is read before the base is written back.
                dynasm!(self.ops ; .arch aarch64 ; ldr w2, [x20, r(s)]);
                self.address(wb, b, off, size);
                self.store(size);
            }
            Fast::Push { s } => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr w2, [x20, r(s)]
                    ; ldr w1, [x20, SP]
                    ; sub w1, w1, 4
                    ; str w1, [x20, SP]
                );
                self.store(Size::Word);
            }
            Fast::Pop { d } => {
                dynasm!(self.ops ; .arch aarch64 ; ldr w1, [x20, SP]);
                self.load(LdKind::Word);
                // SP is written before the destination (POP into SP keeps
                // the loaded value).
                dynasm!(self.ops
                    ; .arch aarch64
                    ; ldr w10, [x20, SP]
                    ; add w10, w10, 4
                    ; str w10, [x20, SP]
                    ; str w9, [x20, r(d)]
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

    /// MUL64/MULU64: the 64-bit product into MLO (unless `mmid_low`), MHI,
    /// and MMID (its middle 32 bits, or the low word).
    fn mul64(&mut self, signed: bool, b: u8, c: Operand) {
        self.operands(b, c);
        if signed {
            dynasm!(self.ops ; .arch aarch64 ; smull x11, w9, w10);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; umull x11, w9, w10);
        }
        dynasm!(self.ops ; .arch aarch64 ; lsr x12, x11, 32 ; str w12, [x20, MHI]);
        if self.mmid_low {
            dynasm!(self.ops ; .arch aarch64 ; str w11, [x20, MMID]);
        } else {
            dynasm!(self.ops
                ; .arch aarch64
                ; str w11, [x20, MLO]
                ; lsr x12, x11, 16
                ; str w12, [x20, MMID]
            );
        }
    }

    fn alu_cc(&mut self, op: AluOp, cc: Cond, d: u8, b: u8, c: Operand) {
        let skip = self.ops.new_dynamic_label();
        self.cond_skip(cc, skip);
        self.operands(b, c);
        self.alu(op);
        dynasm!(self.ops
            ; .arch aarch64
            ; str w9, [x20, r(d)]
            ; =>skip
        );
    }

    /// CMP: Z/N/C/V of b - c.
    fn compare(&mut self, b: u8, c: Operand) {
        self.operands(b, c);
        dynasm!(self.ops ; .arch aarch64 ; cmp w9, w10);
        self.flags_nzcv(Writes::All, true);
        self.merge_flags(Writes::All);
    }

    /// As `x64::Gen::branch`.
    fn branch(&mut self, items: &[Item], k: usize) -> usize {
        let it = items[k];
        let h = it.hot;
        self.pc = h.addr;
        let executed = k as u32 + 1;
        let prev = if k > 0 { items[k - 1].hot.addr } else { h.addr };
        let slot = items.get(k + 1).copied().filter(|s| h.delay && compilable(s) && !is_branch(s.hot.fast) && !s.hot.branchy);
        let link = match h.fast {
            Fast::Bl { .. } | Fast::Jl { .. } => Some(h.next),
            Fast::BlD { .. } | Fast::JlD { .. } => match items.get(k + 1) {
                Some(s) => Some(s.hot.next),
                None => {
                    self.fallback(h.addr, k as u32, prev);
                    return k;
                }
            },
            _ => None,
        };
        // Taken (w24) and the target (w23), with no side effect yet.
        let decided = self.ops.new_dynamic_label();
        match h.fast {
            Fast::B { cc } | Fast::Bl { cc } | Fast::BlD { cc } => {
                dynasm!(self.ops ; .arch aarch64 ; mov w24, wzr);
                self.cond_skip(cc, decided);
                dynasm!(self.ops ; .arch aarch64 ; mov w24, 1 ; =>decided);
                self.movw32(23, h.target);
            }
            Fast::Br { cond, b, v } => {
                dynasm!(self.ops ; .arch aarch64 ; mov w24, wzr ; ldr w9, [x20, r(b)]);
                self.movw32(10, v);
                dynasm!(self.ops ; .arch aarch64 ; cmp w9, w10);
                self.br_skip(cond, decided);
                dynasm!(self.ops ; .arch aarch64 ; mov w24, 1 ; =>decided);
                self.movw32(23, h.target);
            }
            Fast::BrR { cond, b, c } => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; mov w24, wzr
                    ; ldr w9, [x20, r(b)]
                    ; ldr w10, [x20, r(c)]
                    ; cmp w9, w10
                );
                self.br_skip(cond, decided);
                dynasm!(self.ops ; .arch aarch64 ; mov w24, 1 ; =>decided);
                self.movw32(23, h.target);
            }
            Fast::Bbit { bit1, b, c } => {
                // The bit at position (c mod 32) of b.
                dynasm!(self.ops
                    ; .arch aarch64
                    ; mov w24, wzr
                    ; ldr w9, [x20, r(b)]
                    ; ldr w10, [x20, r(c)]
                    ; lsr w9, w9, w10
                );
                if bit1 {
                    dynasm!(self.ops ; .arch aarch64 ; tbz w9, 0, =>decided);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; tbnz w9, 0, =>decided);
                }
                dynasm!(self.ops ; .arch aarch64 ; mov w24, 1 ; =>decided);
                self.movw32(23, h.target);
            }
            Fast::BbitI { bit1, b, v } => {
                // x86's BT with an immediate takes it mod 32.
                let bit = (v & 31) as u32;
                dynasm!(self.ops ; .arch aarch64 ; mov w24, wzr ; ldr w9, [x20, r(b)]);
                if bit1 {
                    dynasm!(self.ops ; .arch aarch64 ; tbz w9, bit, =>decided);
                } else {
                    dynasm!(self.ops ; .arch aarch64 ; tbnz w9, bit, =>decided);
                }
                dynasm!(self.ops ; .arch aarch64 ; mov w24, 1 ; =>decided);
                self.movw32(23, h.target);
            }
            Fast::J { s } | Fast::Jl { s } | Fast::JlD { s } => dynasm!(self.ops
                ; .arch aarch64
                ; mov w24, 1
                ; ldr w23, [x20, r(s)]
                ; and w23, w23, 0xFFFF_FFFE
            ),
            Fast::Jcc { cc, s } => {
                dynasm!(self.ops
                    ; .arch aarch64
                    ; mov w24, wzr
                    ; ldr w23, [x20, r(s)]
                    ; and w23, w23, 0xFFFF_FFFE
                );
                self.cond_skip(cc, decided);
                dynasm!(self.ops ; .arch aarch64 ; mov w24, 1 ; =>decided);
            }
            _ => unreachable!(),
        }
        let target = match h.fast {
            Fast::B { .. } | Fast::Bl { .. } | Fast::BlD { .. } | Fast::Br { .. } | Fast::BrR { .. } | Fast::Bbit { .. } | Fast::BbitI { .. } => Pc::Const(h.target),
            _ => Pc::W23,
        };
        self.stall(&h, it.fetch, false);
        let not_taken = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch aarch64 ; cbz w24, =>not_taken);
        if let Some(link) = link {
            self.movw32(10, link);
            dynasm!(self.ops ; .arch aarch64 ; str w10, [x20, BLINK]);
        }
        self.cost(&h, (!h.delay) as u64 + h.compare_branch as u64);
        let slot_label = self.ops.new_dynamic_label();
        if !h.delay {
            self.flow(target, executed, h.addr);
        } else if slot.is_some() {
            self.budget_check(Pc::Const(h.next), executed, h.addr, true);
            dynasm!(self.ops ; .arch aarch64 ; b =>slot_label);
        } else {
            dynasm!(self.ops
                ; .arch aarch64
                ; mov w10, 1
                ; str w10, [x19, DELAY_PENDING]
                ; str w23, [x19, DELAY_TARGET]
            );
            self.exit(EXIT_FLOW, Pc::Const(h.next), executed, h.addr);
        }
        dynasm!(self.ops ; .arch aarch64 ; =>not_taken);
        self.cost(&h, 0);
        self.after_sequential(h.next, executed, h.addr, false);
        let Some(slot) = slot else {
            self.flow(Pc::Const(h.next), executed, h.addr);
            return k + 1;
        };
        dynasm!(self.ops ; .arch aarch64 ; =>slot_label);
        self.seq = k as u32 + 1;
        self.instr(&slot);
        let s = slot.hot;
        let mem = slot.mem();
        let sequential = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch aarch64 ; cbz w24, =>sequential);
        if mem {
            let stub = self.stub(EXIT_BUS, target, executed + 1, s.addr, false);
            dynasm!(self.ops ; .arch aarch64 ; cbnz w8, =>stub);
        }
        self.flow(target, executed + 1, s.addr);
        dynasm!(self.ops ; .arch aarch64 ; =>sequential);
        self.after_sequential(s.next, executed + 1, s.addr, mem);
        self.flow(Pc::Const(s.next), executed + 1, s.addr);
        k + 2
    }

    /// As `x64::Gen::lp` (LP_END also into w27).
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
        self.movw32(9, h.next);
        self.movw32(27, i.target);
        dynasm!(self.ops
            ; .arch aarch64
            ; str w9, [x19, LP_START]
            ; str w27, [x19, LP_END]
        );
        self.stall(&h, it.fetch, false);
        self.cost(&h, 0);
        self.after_sequential(h.next, executed, h.addr, false);
        self.flow(Pc::Const(h.next), executed, h.addr);
        if i.cc != COND_AL {
            dynasm!(self.ops ; .arch aarch64 ; =>skip);
            self.stall(&h, it.fetch, false);
            self.cost(&h, 1);
            self.flow(Pc::Const(i.target), executed, h.addr);
        }
        k + 1
    }

    /// After `cmp b, c`: jumps to `skip` unless BRcc's condition holds.
    fn br_skip(&mut self, cond: BrCond, skip: DynamicLabel) {
        match cond {
            BrCond::Eq => dynasm!(self.ops ; .arch aarch64 ; b.ne =>skip),
            BrCond::Ne => dynasm!(self.ops ; .arch aarch64 ; b.eq =>skip),
            BrCond::Lt => dynasm!(self.ops ; .arch aarch64 ; b.ge =>skip),
            BrCond::Ge => dynasm!(self.ops ; .arch aarch64 ; b.lt =>skip),
            BrCond::Lo => dynasm!(self.ops ; .arch aarch64 ; b.hs =>skip),
            BrCond::Hs => dynasm!(self.ops ; .arch aarch64 ; b.lo =>skip),
        }
    }

    /// W(r) = v.
    fn movw32(&mut self, r: u8, v: u32) {
        if v == 0 {
            dynasm!(self.ops ; .arch aarch64 ; mov W(r), wzr);
        } else if v <= 0xFFFF {
            dynasm!(self.ops ; .arch aarch64 ; movz W(r), v);
        } else if !v <= 0xFFFF {
            dynasm!(self.ops ; .arch aarch64 ; movn W(r), !v);
        } else if v & 0xFFFF == 0 {
            dynasm!(self.ops ; .arch aarch64 ; movz W(r), v >> 16, lsl 16);
        } else {
            dynasm!(self.ops ; .arch aarch64 ; movz W(r), v & 0xFFFF ; movk W(r), v >> 16, lsl 16);
        }
    }

    /// X(r) = v.
    fn movx64(&mut self, r: u8, v: u64) {
        dynasm!(self.ops ; .arch aarch64 ; movz X(r), (v & 0xFFFF) as u32);
        for (i, shift) in [(1u64, 16u32), (2, 32), (3, 48)] {
            let p = ((v >> (16 * i)) & 0xFFFF) as u32;
            if p != 0 {
                match shift {
                    16 => dynasm!(self.ops ; .arch aarch64 ; movk X(r), p, lsl 16),
                    32 => dynasm!(self.ops ; .arch aarch64 ; movk X(r), p, lsl 32),
                    _ => dynasm!(self.ops ; .arch aarch64 ; movk X(r), p, lsl 48),
                }
            }
        }
    }

    /// W(r) += v (w17 as scratch for a large v).
    fn add_imm(&mut self, r: u8, v: u32) {
        if v == 0 {
        } else if v < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; add WSP(r), WSP(r), #v);
        } else if v.wrapping_neg() < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; sub WSP(r), WSP(r), #(v.wrapping_neg()));
        } else {
            self.movw32(17, v);
            dynasm!(self.ops ; .arch aarch64 ; add W(r), W(r), w17);
        }
    }

    /// W(r) -= v (v small: a count of instructions).
    fn sub_imm(&mut self, r: u8, v: u32) {
        if v == 0 {
        } else if v < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; sub WSP(r), WSP(r), #v);
        } else {
            self.movw32(17, v);
            dynasm!(self.ops ; .arch aarch64 ; sub W(r), W(r), w17);
        }
    }

    /// X(r) += v (x17 as scratch for a large v).
    fn add_imm64(&mut self, r: u8, v: u64) {
        if v == 0 {
        } else if v < 4096 {
            dynasm!(self.ops ; .arch aarch64 ; add XSP(r), XSP(r), #(v as u32));
        } else {
            self.movx64(17, v);
            dynasm!(self.ops ; .arch aarch64 ; add X(r), X(r), x17);
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
        assert_eq!(cond_table(ribbit_cpu::isa::COND_EQ), 0xFF00);
        assert_eq!(cond_table(0x10), 0);
    }

    #[test]
    fn link_patch_encodes_b() {
        assert_eq!(link_patch(0x1000, 0x1004), Some(0x1400_0001u32.to_le_bytes()));
        assert_eq!(link_patch(0x1004, 0x1000), Some(0x17FF_FFFFu32.to_le_bytes()));
        assert_eq!(link_patch(0, 1 << 28), None);
    }
}
