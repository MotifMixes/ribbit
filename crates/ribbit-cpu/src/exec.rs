//! Instruction execution, interrupts, zero-overhead loops and timers.
//!
//! Semantics cite the PRM ("Instruction Set Details", ch. 9) unless noted.
//! Every path that the PRM leaves undefined, or that Ribbit does not model,
//! returns a `CpuError` instead of guessing.

use crate::decode::decode;
use crate::disasm::disassemble;
use crate::icache::{AluOp, DISCARD, Fast, Hot, LdKind};
use crate::isa::NO_LOAD;
use crate::isa::*;
use crate::{
    AuxAccess, AuxExt, AuxKind, Cpu, CpuError, HaltReason, IllegalPolicy, MmidMode, Step, Timer, debug, reg, status32,
};
use ribbit_memory::Bus;

/// AUX_MACMODE bits. CS and the S flags' positions are taken from MAME; the
/// PRM figure (Figure 71) is not legible enough to confirm them. HYPOTHESIS.
const MACMODE_CS: u32 = 1 << 1;
const MACMODE_S2: u32 = 1 << 5;
const MACMODE_S1: u32 = 1 << 8;

const UNMODELLED_AUX_LOG_LIMIT: usize = 256;

/// What `Cpu::run_block` did.
#[derive(Debug)]
pub struct Block {
    /// Instructions executed (one cycle each).
    pub executed: u64,
    pub exit: BlockExit,
    /// Address of the last instruction executed (the start PC if none).
    pub from: u32,
}

#[derive(Debug)]
pub enum BlockExit {
    /// The budget ran out.
    Budget,
    /// The next step needs `step_with` (interrupt pending, sleeping, halted,
    /// or an instruction that changes interrupt state).
    Slow,
    /// The last instruction made the bus request an exit.
    Bus,
    /// The last instruction jumped backward to an address `watch` accepted.
    Backward,
    /// The next instruction failed, exactly as `step_with` would have; the
    /// failed instruction did not execute.
    Error(CpuError),
}

enum Flow {
    Next,
    Jump(u32),
    Halt(HaltReason),
    /// Take exception/interrupt `vector` with ILINK2 = next PC.
    Exception(u8),
}

fn kind_size(kind: LdKind) -> Size {
    match kind {
        LdKind::Word => Size::Word,
        LdKind::Half | LdKind::HalfSigned => Size::Half,
        LdKind::Byte | LdKind::ByteSigned => Size::Byte,
    }
}

/// `.as` scaling (PRM Table 52), as `Cpu::address` applies it.
#[inline(always)]
fn scale_of(size: Size) -> u32 {
    match size {
        Size::Word => 2,
        Size::Half => 1,
        Size::Byte => 0,
    }
}

#[inline(always)]
fn load(bus: &mut impl Bus, kind: LdKind, addr: u32) -> u32 {
    match kind {
        LdKind::Word => bus.read32(addr),
        LdKind::Half => bus.read16(addr) as u32,
        LdKind::HalfSigned => bus.read16(addr) as i16 as i32 as u32,
        LdKind::Byte => bus.read8(addr) as u32,
        LdKind::ByteSigned => bus.read8(addr) as i8 as i32 as u32,
    }
}

fn add(b: u32, c: u32, cin: bool) -> (u32, bool, bool) {
    let (r1, c1) = b.overflowing_add(c);
    let (r, c2) = r1.overflowing_add(cin as u32);
    let v = ((b ^ r) & (c ^ r)) >> 31 != 0;
    (r, c1 || c2, v)
}

fn sub(b: u32, c: u32, bin: bool) -> (u32, bool, bool) {
    let r = b.wrapping_sub(c).wrapping_sub(bin as u32);
    let borrow = (b as u64) < c as u64 + bin as u64;
    let v = ((b ^ c) & (b ^ r)) >> 31 != 0;
    (r, borrow, v)
}

fn sat32(x: i64) -> (u32, bool) {
    if x > i32::MAX as i64 {
        (i32::MAX as u32, true)
    } else if x < i32::MIN as i64 {
        (i32::MIN as u32, true)
    } else {
        (x as i32 as u32, false)
    }
}

fn sat16(x: i32) -> (u16, bool) {
    if x > i16::MAX as i32 {
        (i16::MAX as u16, true)
    } else if x < i16::MIN as i32 {
        (i16::MIN as u16, true)
    } else {
        (x as i16 as u16, false)
    }
}

fn norm32(c: u32) -> u32 {
    if c == 0 || c == u32::MAX {
        31
    } else if (c as i32) >= 0 {
        c.leading_zeros() - 1
    } else {
        (!c).leading_zeros() - 1
    }
}

fn norm16(c: u32) -> u32 {
    let v = c as u16;
    if v == 0 || v == u16::MAX {
        15
    } else if (v as i16) >= 0 {
        v.leading_zeros() - 1
    } else {
        (!v).leading_zeros() - 1
    }
}

impl Cpu {
    pub fn step_with(&mut self, bus: &mut impl Bus, ext: &mut impl AuxExt) -> Result<Step, crate::CpuError> {
        if self.halted() {
            return Err(CpuError::Halted { pc: self.pc });
        }
        // The A5 has no BTA register, so "interrupts and exceptions are held
        // off until the instruction in the delay slot commits" (PRM, BTA).
        if self.delay_target.is_none()
            && let Some(vector) = self.pending_interrupt()
        {
            self.enter_interrupt(vector);
            self.tick(1)?;
            return Ok(Step::Interrupt { vector });
        }
        if self.sleeping {
            self.tick(1)?;
            return Ok(Step::Sleeping);
        }

        let pc = self.pc;
        let i = self.icache.get(pc, || decode(bus, pc));
        if self.delay_target.is_some() && (i.is_branch() || matches!(i.op, Op::Lp)) {
            // "Bcc ... cannot immediately follow a Bcc.D, BLcc.D, Jcc.D,
            // JLcc.D, BRcc.D or BBITn.D instruction" (PRM); LPcc likewise.
            return Err(self.unpredictable(&i, "branch, jump or LP in a delay slot"));
        }
        bus.instruction(pc, i.len);
        let flow = self.execute(&i, bus, ext)?;
        self.instructions_retired += 1;
        let stall = bus.take_stall();
        let (now, taken) = (self.cycles, matches!(flow, Flow::Jump(_)));
        let cost = self.pipeline_cost(now, i.reg_reads(), i.load_dst(), taken, i.delay, i.is_compare_branch(), stall);
        let delay_slot_of = self.delay_target.take();
        match flow {
            Flow::Halt(reason) => {
                if reason == HaltReason::Flag {
                    self.pc = i.next();
                }
                self.tick(1)?;
                self.last_instr = Some(i);
                return Ok(Step::Halted(reason));
            }
            Flow::Exception(vector) => {
                self.pc = i.next();
                self.enter_interrupt(vector);
            }
            Flow::Jump(target) if i.delay => {
                self.delay_target = Some(target);
                self.pc = i.next();
            }
            Flow::Jump(target) => self.pc = target,
            Flow::Next => {
                self.pc = match delay_slot_of {
                    Some(target) => target,
                    None => self.loop_end_check(i.next()),
                };
            }
        }
        self.tick(cost)?;
        if self.record_last {
            self.last_instr = Some(i);
        }
        Ok(Step::Executed)
    }

    /// Executes up to `budget` instructions back to back, with exactly the
    /// results of as many `step_with` calls, for a caller that knows nothing
    /// time-driven happens meanwhile: no interrupt is pending (checked here),
    /// no processor timer reaches LIMIT (`budget <= cycles_to_timer_event`,
    /// checked) and the machine has no event due (the caller's part).
    ///
    /// Stops early, before executing it, at any instruction that can change
    /// interrupt state or halt (LR/SR, FLAG, SLEEP, BRK, SWI, J.F, illegal
    /// and unsupported encodings, a branch in a delay slot): `step_with`
    /// runs those. Stops after an instruction for which the bus reports
    /// `block_exit`, or after a backward jump that `watch(target, cycles)` accepts
    /// (the machine's idle-loop detector), and before any instruction at or
    /// above `fence`. `last_instr` is not kept.
    pub fn run_block<B: Bus>(
        &mut self,
        bus: &mut B,
        ext: &mut impl AuxExt,
        budget: u64,
        fence: u32,
        mut watch: impl FnMut(u32, u64) -> bool,
    ) -> Block {
        // A block ends with the instruction that reaches `budget`, so only
        // that last one can take a processor timer to LIMIT (an instruction
        // can stall for many cycles); the epilogue advances the timers as
        // `tick` does, wrap included.
        let budget = budget.min(self.cycles_to_timer_event());
        let mut out = Block { executed: 0, exit: BlockExit::Budget, from: self.pc };
        if self.halted() || self.sleeping || self.pending_interrupt().is_some() {
            out.exit = BlockExit::Slow;
            return out;
        }
        // Counters stay in locals in the loop and are written back after.
        let (start, mut n, mut spent) = (self.cycles, 0u64, 0u64);
        while spent < budget {
            let pc = self.pc;
            if pc >= fence {
                out.exit = BlockExit::Slow;
                break;
            }
            // Cached (ROM) code: the pre-digested form. Elsewhere, decode now.
            let mut uncached = None;
            let hot = match self.icache.hot(pc, || decode(bus, pc)) {
                Some(h) => h,
                None => Hot::of(uncached.insert(decode(bus, pc))),
            };
            if hot.stop
                || (hot.branchy && self.delay_target.is_some())
                || (hot.flag_reg != crate::icache::NO_REG && self.regs[(hot.flag_reg & 31) as usize] & status32::H != 0)
            {
                out.exit = BlockExit::Slow;
                break;
            }
            bus.instruction(pc, hot.next.wrapping_sub(pc));
            let flow = match hot.fast {
                Fast::General => {
                    let i = match uncached {
                        Some(i) => i,
                        None => self.icache.get(pc, || decode(bus, pc)),
                    };
                    self.cycles = start + spent;
                    bus.at(start + spent, pc);
                    match self.execute(&i, bus, ext) {
                        Ok(f) => f,
                        Err(e) => {
                            out.exit = BlockExit::Error(e);
                            break;
                        }
                    }
                }
                f => {
                    if f.accesses_memory() {
                        bus.at(start + spent, pc);
                    }
                    self.execute_fast(f, &hot, bus)
                }
            };
            let stall = bus.take_stall();
            let taken = matches!(flow, Flow::Jump(_));
            spent += self.pipeline_cost(start + spent, hot.reads, hot.load_dst, taken, hot.delay, hot.compare_branch, stall);
            let delay_slot_of = self.delay_target.take();
            match flow {
                Flow::Jump(target) if hot.delay => {
                    self.delay_target = Some(target);
                    self.pc = hot.next;
                }
                Flow::Jump(target) => self.pc = target,
                Flow::Next => {
                    self.pc = match delay_slot_of {
                        Some(target) => target,
                        None => self.loop_end_check(hot.next),
                    };
                }
                // Only produced by the instructions excluded above.
                Flow::Halt(_) | Flow::Exception(_) => unreachable!("halt or exception in run_block at {pc:#x}"),
            }
            n += 1;
            if bus.block_exit() || (hot.recheck && self.pending_interrupt().is_some()) {
                out.from = pc;
                out.exit = BlockExit::Bus;
                break;
            }
            if self.pc <= pc && self.delay_target.is_none() && watch(self.pc, start + spent) {
                out.from = pc;
                out.exit = BlockExit::Backward;
                break;
            }
            out.from = pc;
        }
        self.instructions_retired += n;
        out.executed = n;
        // What `tick` did per step: before the last instruction no timer
        // reached LIMIT (the budget), so one call does the same.
        self.cycles = start;
        if let Err(e) = self.tick(spent) {
            out.exit = BlockExit::Error(e);
        }
        out
    }

    /// A pre-digested instruction (`icache::fast_form`): what `execute` does
    /// for that shape, without its operand and condition decoding.
    #[inline(always)]
    fn execute_fast(&mut self, f: Fast, hot: &Hot, bus: &mut impl Bus) -> Flow {
        // Fast forms only name r0-r31; the mask lets the compiler drop
        // bounds checks.
        let r = |n: u8| (n & 31) as usize;
        match f {
            Fast::General => unreachable!(),
            Fast::Mov { d, s } => self.regs[r(d)] = self.regs[r(s)],
            Fast::MovI { d, v } => self.regs[r(d)] = v,
            Fast::Alu { op, d, b, c } => {
                self.regs[r(d)] = op.apply(self.regs[r(b)], self.regs[r(c)], self.flag(status32::C))
            }
            Fast::AluI { op, d, b, v } => self.regs[r(d)] = op.apply(self.regs[r(b)], v, self.flag(status32::C)),
            Fast::Ld { kind, d, b, off } => {
                let v = load(bus, kind, self.regs[r(b)].wrapping_add(off));
                self.regs[r(d)] = v;
            }
            Fast::LdR { kind, d, b, c } => {
                let v = load(bus, kind, self.regs[r(b)].wrapping_add(self.regs[r(c)]));
                self.regs[r(d)] = v;
            }
            Fast::St { size, s, b, off } => {
                let (v, addr) = (self.regs[r(s)], self.regs[r(b)].wrapping_add(off));
                match size {
                    Size::Word => bus.write32(addr, v),
                    Size::Half => bus.write16(addr, v as u16),
                    Size::Byte => bus.write8(addr, v as u8),
                }
            }
            Fast::Push { s } => {
                let v = self.regs[r(s)];
                let sp = self.regs[reg::SP].wrapping_sub(4);
                self.regs[reg::SP] = sp;
                bus.write32(sp, v);
            }
            Fast::Pop { d } => {
                let sp = self.regs[reg::SP];
                let v = bus.read32(sp);
                self.regs[reg::SP] = sp.wrapping_add(4);
                self.regs[r(d)] = v;
            }
            Fast::Br { cond, b, v } => {
                if cond.holds(self.regs[r(b)], v) {
                    return Flow::Jump(hot.target);
                }
            }
            Fast::BrR { cond, b, c } => {
                if cond.holds(self.regs[r(b)], self.regs[r(c)]) {
                    return Flow::Jump(hot.target);
                }
            }
            Fast::B { cc } => {
                if self.cond(cc) {
                    return Flow::Jump(hot.target);
                }
            }
            Fast::Bl { cc } => {
                if self.cond(cc) {
                    self.regs[reg::BLINK] = hot.next;
                    return Flow::Jump(hot.target);
                }
            }
            Fast::J { s } => return Flow::Jump(self.regs[r(s)] & !1),
            Fast::Cmp { b, c } => self.compare(self.regs[r(b)], self.regs[r(c)]),
            Fast::CmpI { b, v } => self.compare(self.regs[r(b)], v),
            Fast::AluCc { op, cc, d, b, c } => {
                if self.cond(cc) {
                    self.regs[r(d)] = op.apply(self.regs[r(b)], self.regs[r(c)], self.flag(status32::C));
                }
            }
            Fast::AluCcI { op, cc, d, b, v } => {
                if self.cond(cc) {
                    self.regs[r(d)] = op.apply(self.regs[r(b)], v, self.flag(status32::C));
                }
            }
            Fast::AluF { op, d, b, c } => self.alu_flags(op, d, self.regs[r(b)], self.regs[r(c)]),
            Fast::AluFI { op, d, b, v } => self.alu_flags(op, d, self.regs[r(b)], v),
            Fast::Un { op, d, c } => self.regs[r(d)] = op.apply(self.regs[r(c)]),
            Fast::LdX { kind, wb, d, b, off } => {
                let addr = self.write_back(wb, b, off, kind_size(kind));
                let v = load(bus, kind, addr);
                self.regs[r(d)] = v;
            }
            Fast::StX { size, wb, s, b, off } => {
                let v = self.regs[r(s)];
                let addr = self.write_back(wb, b, off, size);
                match size {
                    Size::Word => bus.write32(addr, v),
                    Size::Half => bus.write16(addr, v as u16),
                    Size::Byte => bus.write8(addr, v as u8),
                }
            }
            Fast::BlD { cc } => {
                if self.cond(cc) {
                    self.regs[reg::BLINK] = self.after_slot(bus, hot.next);
                    return Flow::Jump(hot.target);
                }
            }
            Fast::Jcc { cc, s } => {
                let target = self.regs[r(s)] & !1;
                if self.cond(cc) {
                    return Flow::Jump(target);
                }
            }
        }
        Flow::Next
    }

    /// Effective address of a fast load/store with base register `b` and
    /// immediate offset, with the base write-back, as `address` does it.
    #[inline(always)]
    fn write_back(&mut self, wb: WriteBack, b: u8, off: u32, size: Size) -> u32 {
        let b = (b & 31) as usize;
        let base = self.regs[b];
        match wb {
            WriteBack::None => base.wrapping_add(off),
            WriteBack::Pre => {
                self.regs[b] = base.wrapping_add(off);
                base.wrapping_add(off)
            }
            WriteBack::Post => {
                self.regs[b] = base.wrapping_add(off);
                base
            }
            WriteBack::Scaled => base.wrapping_add(off << scale_of(size)),
        }
    }

    /// An ALU operation with .F, as `execute` does it: the result (unless
    /// discarded), then Z/N and the operation's C/V.
    #[inline(always)]
    fn alu_flags(&mut self, op: AluOp, d: u8, b: u32, c: u32) {
        let carry = self.flag(status32::C);
        let shift_carry = |n: u32, bit: u32| if n == 0 { None } else { Some((b >> bit) & 1 != 0) };
        let (r, cf, vf) = match op {
            AluOp::Add | AluOp::Add1 | AluOp::Add2 | AluOp::Add3 => {
                let k = match op {
                    AluOp::Add1 => 1,
                    AluOp::Add2 => 2,
                    AluOp::Add3 => 3,
                    _ => 0,
                };
                let (r, cy, v) = add(b, c << k, false);
                (r, Some(cy), Some(v))
            }
            AluOp::Adc => {
                let (r, cy, v) = add(b, c, carry);
                (r, Some(cy), Some(v))
            }
            AluOp::Sub | AluOp::Sub1 | AluOp::Sub2 | AluOp::Sub3 => {
                let k = match op {
                    AluOp::Sub1 => 1,
                    AluOp::Sub2 => 2,
                    AluOp::Sub3 => 3,
                    _ => 0,
                };
                let (r, bw, v) = sub(b, c << k, false);
                (r, Some(bw), Some(v))
            }
            AluOp::Sbc => {
                let (r, bw, v) = sub(b, c, carry);
                (r, Some(bw), Some(v))
            }
            AluOp::Rsub => {
                let (r, bw, v) = sub(c, b, false);
                (r, Some(bw), Some(v))
            }
            AluOp::And => (b & c, None, None),
            AluOp::Or => (b | c, None, None),
            AluOp::Xor => (b ^ c, None, None),
            AluOp::Bic => (b & !c, None, None),
            AluOp::Asl => {
                let n = c & 31;
                (b << n, shift_carry(n, 32u32.wrapping_sub(n) & 31), None)
            }
            AluOp::Lsr => {
                let n = c & 31;
                (b >> n, shift_carry(n, n.wrapping_sub(1)), None)
            }
            AluOp::Asr => {
                let n = c & 31;
                (((b as i32) >> n) as u32, shift_carry(n, n.wrapping_sub(1)), None)
            }
        };
        if d != DISCARD {
            self.regs[(d & 31) as usize] = r;
        }
        self.flags(r, cf, vf);
    }

    /// Address of the instruction after the delay slot at `slot`.
    fn after_slot(&mut self, bus: &mut impl Bus, slot: u32) -> u32 {
        self.icache.get(slot, || decode(bus, slot)).next()
    }

    /// CMP's flags: Z, N, C and V of b - c.
    #[inline(always)]
    fn compare(&mut self, b: u32, c: u32) {
        let (r, bw, v) = sub(b, c, false);
        self.flags(r, Some(bw), Some(v));
    }

    // -----------------------------------------------------------------------
    // Interrupts (PRM ch. 4, Figure 83; A5 priorities, Tables 23/24)

    fn pending_irqs(&self) -> u32 {
        let mut p = self.irq_lines;
        if self.timers[0].pending {
            p |= 1 << 3;
        }
        if self.timers[1].pending {
            p |= 1 << 7;
        }
        if (3..32).contains(&self.aux_irq_hint) {
            p |= 1 << self.aux_irq_hint;
        }
        p & 0xFFFF_FFF8
    }

    /// Highest-priority interrupt that would be taken now, if any.
    pub fn pending_interrupt(&self) -> Option<u8> {
        let p = self.pending_irqs();
        // "The higher the interrupt number the higher the priority within the
        // interrupt level set ... IRQ7 always has the highest relative
        // priority within its level set" (PRM, Interrupt Priority).
        let pick = |set: u32| if set & (1 << 7) != 0 { 7 } else { 31 - set.leading_zeros() as u8 };
        let l2 = p & self.aux_irq_lev;
        let l1 = p & !self.aux_irq_lev;
        if self.flag(status32::E2) && l2 != 0 {
            Some(pick(l2))
        } else if self.flag(status32::E1) && l1 != 0 {
            Some(pick(l1))
        } else {
            None
        }
    }

    /// Interrupt/exception entry. Vectors 1 (memory error) and 2 (instruction
    /// error) and level-2 IRQs use ILINK2 and mask both levels; level-1 IRQs
    /// use ILINK1 and mask level 1. ILINK holds the next PC.
    fn enter_interrupt(&mut self, vector: u8) {
        let level2 = vector < 3 || self.aux_irq_lev & (1 << vector) != 0;
        if level2 {
            self.regs[reg::ILINK2] = self.pc;
            self.status32_l2 = self.status32;
            self.status32 &= !(status32::E1 | status32::E2);
            self.aux_irq_lv12 |= 2;
        } else {
            self.regs[reg::ILINK1] = self.pc;
            self.status32_l1 = self.status32;
            self.status32 &= !status32::E1;
            self.aux_irq_lv12 |= 1;
        }
        self.sleeping = false;
        self.debug &= !debug::ZZ;
        self.pc = self.int_vector_base.wrapping_add(8 * vector as u32);
    }

    /// ARCtangent-A5 loop mechanism (PRM "ARCtangent-A5 Loop Operation"):
    /// when NEXT_PC equals LP_END, LP_COUNT is decremented and, unless it was
    /// 1, PC becomes LP_START. Only checked on sequential flow: after a taken
    /// branch or a delay slot the next PC comes from the branch (the ARC 600
    /// rule; the A5 description does not cover branches, so this is a
    /// HYPOTHESIS for the A5).
    fn loop_end_check(&mut self, next: u32) -> u32 {
        if next != self.lp_end {
            return next;
        }
        let count = self.regs[reg::LP_COUNT];
        self.regs[reg::LP_COUNT] = count.wrapping_sub(1);
        if count != 1 { self.lp_start } else { next }
    }

    /// Cycles an instruction starting at cycle `now` takes (ARCompact PRM,
    /// ARCtangent-A5): one; plus waiting for any register it reads whose
    /// load has not arrived ("the pipeline will stall until the register has
    /// been loaded"; a load's result arrives `load_latency` cycles after
    /// it issues); plus `memory`, the memory system's stall (cache misses,
    /// device accesses: `Bus::take_stall`); plus one if it is a taken
    /// branch or jump whose delay slot is not executed (the slot is killed);
    /// plus one more for a taken BRcc/BBIT ("there are two delay slots ...
    /// the second delay slot is always nullified if the branch is taken").
    /// Not modelled: multiplier latency (UNKNOWN).
    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    fn pipeline_cost(
        &mut self,
        now: u64,
        reads: u64,
        load_dst: u8,
        taken: bool,
        delay: bool,
        compare_branch: bool,
        memory: u64,
    ) -> u64 {
        if !self.config.pipeline_timing {
            return 1;
        }
        let mut wait = 0;
        if now < self.ready_max {
            let mut m = reads;
            while m != 0 {
                let r = m.trailing_zeros() as usize;
                m &= m - 1;
                wait = wait.max(self.ready[r].saturating_sub(now));
            }
        }
        if load_dst != NO_LOAD {
            let at = now + wait + memory + self.config.load_latency;
            self.ready[load_dst as usize] = at;
            self.ready_max = self.ready_max.max(at);
        }
        1 + wait + memory + (taken && !delay) as u64 + (taken && compare_branch) as u64
    }

    fn tick(&mut self, cycles: u64) -> Result<(), CpuError> {
        self.cycles += cycles;
        // Fast path (same result): neither timer reaches LIMIT in this step.
        if self.timers.iter().all(|t| (t.limit.wrapping_sub(t.count) as u64) >= cycles) {
            for t in &mut self.timers {
                t.count = t.count.wrapping_add(cycles as u32);
            }
            return Ok(());
        }
        for _ in 0..cycles {
            for t in &mut self.timers {
                if t.count == t.limit {
                    t.count = 0;
                    if t.control & Timer::W != 0 {
                        return Err(CpuError::Unsupported {
                            pc: self.pc,
                            word: 0,
                            info: crate::ErrorInfo::new(
                                String::new(),
                                "processor timer watchdog reset (system-specific; not modelled)",
                            ),
                        });
                    }
                    if t.control & Timer::IE != 0 {
                        t.pending = true;
                    }
                } else {
                    t.count = t.count.wrapping_add(1);
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Errors

    fn unpredictable(&self, i: &Instr, reason: &str) -> CpuError {
        CpuError::Unpredictable { pc: i.addr, info: crate::ErrorInfo::new(disassemble(i), reason) }
    }

    fn unsupported(&self, i: &Instr, reason: &str) -> CpuError {
        CpuError::Unsupported { pc: i.addr, word: i.word, info: crate::ErrorInfo::new(disassemble(i), reason) }
    }

    fn illegal(&self, i: &Instr, reason: &str) -> Result<Flow, CpuError> {
        match self.config.illegal {
            IllegalPolicy::Stop => {
                Err(CpuError::Illegal { pc: i.addr, word: i.word, info: crate::ErrorInfo::new(disassemble(i), reason) })
            }
            IllegalPolicy::Exception => Ok(Flow::Exception(2)),
        }
    }

    // -----------------------------------------------------------------------
    // Operands

    // Hot helpers: an always-inlined fast path (core registers r0-r31) and
    // an out-of-line slow path, so error formatting does not stop inlining.
    #[inline(always)]
    fn read_reg(&mut self, r: u8, i: &Instr) -> Result<u32, CpuError> {
        if r < 32 {
            return Ok(self.regs[r as usize]);
        }
        self.read_reg_slow(r, i)
    }

    #[inline(never)]
    fn read_reg_slow(&mut self, r: u8, i: &Instr) -> Result<u32, CpuError> {
        if let Some(xy) = &mut self.xy {
            if crate::xy::is_port(r) {
                return Ok(xy.read(r));
            }
            if r == 56 {
                // The dual MAC's high-lane accumulator (see MULDW); the
                // Leapster RTOS saves and restores it per task.
                return Ok(self.regs[56]);
            }
        }
        match r {
            0..=31 | 57..=60 => Ok(self.regs[r as usize]),
            63 => Ok(i.addr & !3),
            32..=56 => {
                Err(self
                    .unsupported(i, &format!("extension core register r{r} (XY memory/DSP extension) is not modelled")))
            }
            _ => Err(self.unsupported(i, &format!("read of reserved core register r{r}"))),
        }
    }

    #[inline(always)]
    fn read(&mut self, s: Src, i: &Instr) -> Result<u32, CpuError> {
        match s {
            Src::None => Ok(0),
            Src::Imm(v) | Src::Limm(v) => Ok(v),
            Src::Reg(r) => self.read_reg(r, i),
        }
    }

    #[inline(always)]
    fn check_dst(&self, d: Dst, i: &Instr) -> Result<(), CpuError> {
        if matches!(d, Dst::None | Dst::Discard) || matches!(d, Dst::Reg(r) if r < 32) {
            return Ok(());
        }
        self.check_dst_slow(d, i)
    }

    #[inline(never)]
    fn check_dst_slow(&self, d: Dst, i: &Instr) -> Result<(), CpuError> {
        if self.xy.is_some() && matches!(d, Dst::Reg(32..=56)) {
            return Ok(());
        }
        match d {
            Dst::Reg(r @ 32..=56) => Err(self.unsupported(
                i,
                &format!("write to extension core register r{r} (XY memory/DSP extension) is not modelled"),
            )),
            Dst::Reg(61) => Err(self.unsupported(i, "write to reserved core register r61")),
            Dst::Reg(63) => Err(self.unpredictable(i, "PCL as a destination")),
            _ => Ok(()),
        }
    }

    #[inline(always)]
    fn write_dst(&mut self, d: Dst, v: u32) {
        if let Dst::Reg(r) = d
            && r < 32
        {
            self.regs[r as usize] = v;
            return;
        }
        self.write_dst_slow(d, v)
    }

    #[inline(never)]
    fn write_dst_slow(&mut self, d: Dst, v: u32) {
        if let (Dst::Reg(r), Some(xy)) = (d, &mut self.xy)
            && crate::xy::is_port(r)
        {
            xy.write(r, v);
            return;
        }
        if let Dst::Reg(r) = d {
            if (57..=59).contains(&r) {
                self.diag.mul_result_writes += 1;
            }
            // r56/r57 are also the dual MAC's accumulators (see MULDW): the
            // RTOS restores them per task.
            if let 56 | 57 = r {
                self.mac_acc[57 - r as usize] = v as i32 as i64;
            }
            self.regs[r as usize] = v;
        }
    }

    fn set_zn(&mut self, r: u32) {
        self.status32 &= !(status32::Z | status32::N);
        if r == 0 {
            self.status32 |= status32::Z;
        }
        if r >> 31 != 0 {
            self.status32 |= status32::N;
        }
    }

    fn set_bit(&mut self, bit: u32, on: bool) {
        if on {
            self.status32 |= bit;
        } else {
            self.status32 &= !bit;
        }
    }

    fn flags(&mut self, r: u32, c: Option<bool>, v: Option<bool>) {
        self.set_zn(r);
        if let Some(c) = c {
            self.set_bit(status32::C, c);
        }
        if let Some(v) = v {
            self.set_bit(status32::V, v);
        }
    }

    #[inline(always)]
    fn cond(&self, cc: Cond) -> bool {
        cond_holds(cc, self.flag(status32::Z), self.flag(status32::N), self.flag(status32::C), self.flag(status32::V))
    }

    /// Address of the instruction after the delay slot (BLINK for BL.D/JL.D).
    fn after_delay_slot(&mut self, bus: &mut impl Bus, i: &Instr) -> u32 {
        let pc = i.next();
        self.icache.get(pc, || decode(bus, pc)).next()
    }

    // -----------------------------------------------------------------------
    // Auxiliary registers (PRM ch. 3)

    fn log_unmodelled_aux(&mut self, pc: u32, reg: u32, kind: AuxKind, value: u32) {
        self.diag.aux_unmodelled_total += 1;
        if self.diag.aux_unmodelled.len() < UNMODELLED_AUX_LOG_LIMIT {
            self.diag.aux_unmodelled.push(AuxAccess { pc, reg, kind, value });
        }
    }

    fn aux_read(&mut self, n: u32, i: &Instr, ext: &mut impl AuxExt) -> Result<u32, CpuError> {
        let s = self.status32;
        let bit = |b: u32, pos: u32| ((s & b != 0) as u32) << pos;
        Ok(match n {
            // Obsolete STATUS: Z N C V E2 E1 H in bits 31..25, PC[25:2] in 23..0.
            0x00 => {
                bit(status32::Z, 31)
                    | bit(status32::N, 30)
                    | bit(status32::C, 29)
                    | bit(status32::V, 28)
                    | bit(status32::E2, 27)
                    | bit(status32::E1, 26)
                    | bit(status32::H, 25)
                    | ((i.next() >> 2) & 0x00FF_FFFF)
            }
            0x01 => self.semaphore & 0xF,
            0x02 => self.lp_start & !1,
            0x03 => self.lp_end & !1,
            0x04 => self.config.identity,
            0x05 => self.debug,
            // PC "contains the next instruction address"; in a delay slot it
            // is the branch target (PRM, PC register).
            0x06 => self.delay_target.unwrap_or(i.next()) & !1,
            0x0A => s & status32::IMPLEMENTED,
            0x0B => self.status32_l1 & status32::IMPLEMENTED & !status32::H,
            0x0C => self.status32_l2 & status32::IMPLEMENTED & !status32::H,
            0x12 => return Err(self.unsupported(i, "read of write-only aux register MULHI")),
            0x21 => self.timers[0].count,
            0x22 => self.timers[0].control | if self.timers[0].pending { Timer::IP } else { 0 },
            0x23 => self.timers[0].limit,
            0x100 => self.timers[1].count,
            0x101 => self.timers[1].control | if self.timers[1].pending { Timer::IP } else { 0 },
            0x102 => self.timers[1].limit,
            0x25 => self.int_vector_base,
            0x41 => self.macmode,
            0x43 => self.aux_irq_lv12 & 3,
            0x200 => self.aux_irq_lev & 0xFFFF_FFF8,
            0x201 => self.aux_irq_hint,
            n if self.xy.is_some()
                && (crate::xy::AUX_BASE..crate::xy::AUX_BASE + crate::xy::AUX_COUNT).contains(&n) =>
            {
                self.xy.as_ref().map_or(0, |xy| xy.aux_read(n))
            }
            _ => match ext.aux_read(n) {
                Some(v) => v,
                None => {
                    // "Accessing the extension auxiliary register region in the
                    // basecase version of the ARCtangent-A5 processor will
                    // return the ID register" (PRM, Illegal Auxiliary Register
                    // Usage). Logged because the Leapster's extensions are unknown.
                    self.log_unmodelled_aux(i.addr, n, AuxKind::Read, 0);
                    self.config.identity
                }
            },
        })
    }

    fn aux_write(
        &mut self,
        n: u32,
        v: u32,
        i: &Instr,
        ext: &mut impl AuxExt,
        bus: &mut impl Bus,
    ) -> Result<(), CpuError> {
        let implemented_status = status32::IMPLEMENTED & !status32::H;
        if let Some(xy) = &mut self.xy
            && (crate::xy::AUX_BASE..crate::xy::AUX_BASE + crate::xy::AUX_COUNT).contains(&n)
        {
            xy.aux_write(n, v, bus);
            return Ok(());
        }
        match n {
            0x01 => self.semaphore = v & 0xF,
            0x02 => self.lp_start = v & !1,
            0x03 => self.lp_end = v & !1,
            0x0B => self.status32_l1 = v & implemented_status,
            0x0C => self.status32_l2 = v & implemented_status,
            0x12 => self.regs[reg::MHI] = v,
            0x21 => self.timers[0].count = v,
            0x22 => {
                self.timers[0].control = v & (Timer::IE | Timer::NH | Timer::W);
                self.timers[0].pending = false;
            }
            0x23 => self.timers[0].limit = v,
            0x100 => self.timers[1].count = v,
            0x101 => {
                self.timers[1].control = v & (Timer::IE | Timer::NH | Timer::W);
                self.timers[1].pending = false;
            }
            0x102 => self.timers[1].limit = v,
            0x25 => self.int_vector_base = v & !0x3FF,
            0x41 => {
                let flags = MACMODE_S1 | MACMODE_S2;
                let keep = if v & MACMODE_CS != 0 { 0 } else { self.macmode & flags };
                self.macmode = keep | (v & !(flags | MACMODE_CS));
            }
            0x43 => self.aux_irq_lv12 &= !(v & 3),
            0x200 => self.aux_irq_lev = v & 0xFFFF_FFF8,
            // "Writing 0x00 ... clears a software triggered interrupt. Writing
            // a value greater than 0x1F will clear ... Writing values 0x0 to
            // 0x2 have no effect" (PRM; read as: 0 clears, 1-2 ignored).
            0x201 => match v {
                3..=31 => self.aux_irq_hint = v,
                1 | 2 => {}
                _ => self.aux_irq_hint = 0,
            },
            // Read-only registers (STATUS, IDENTITY, DEBUG, PC, STATUS32) and
            // everything else: offer to the machine, otherwise log. "Writes to
            // non implemented auxiliary registers are ignored" (PRM).
            _ => {
                if !ext.aux_write(n, v) {
                    self.log_unmodelled_aux(i.addr, n, AuxKind::Write, v);
                }
            }
        }
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Execution

    fn execute(&mut self, i: &Instr, bus: &mut impl Bus, ext: &mut impl AuxExt) -> Result<Flow, CpuError> {
        match i.op {
            Op::Illegal(why) => return self.illegal(i, why.as_str()),
            Op::Arc700Only(_) => {
                return self.illegal(i, "ARC 700-only instruction (Instruction Error on the ARCtangent-A5)");
            }
            Op::Unimp => return self.illegal(i, "UNIMP_S"),
            Op::Extension { .. } => return Err(self.unsupported(i, "extension instruction with no known definition")),
            Op::Divaw => return Err(self.unsupported(i, "DIVAW: PRM does not state whether the comparison is signed")),
            _ => {}
        }
        self.check_dst(i.dst, i)?;

        // Control-flow instructions evaluate their own conditions.
        match i.op {
            Op::B | Op::Bl => {
                if !self.cond(i.cc) {
                    return Ok(Flow::Next);
                }
                if matches!(i.op, Op::Bl) {
                    let ret = if i.delay { self.after_delay_slot(bus, i) } else { i.next() };
                    self.regs[reg::BLINK] = ret;
                }
                return Ok(Flow::Jump(i.target));
            }
            Op::Br(c) => {
                let (b, v) = (self.read(i.src1, i)?, self.read(i.src2, i)?);
                return Ok(if c.holds(b, v) { Flow::Jump(i.target) } else { Flow::Next });
            }
            Op::Bbit0 | Op::Bbit1 => {
                let (b, v) = (self.read(i.src1, i)?, self.read(i.src2, i)?);
                let set = b & (1 << (v & 31)) != 0;
                return Ok(if set == (matches!(i.op, Op::Bbit1)) { Flow::Jump(i.target) } else { Flow::Next });
            }
            Op::J | Op::Jl => {
                let target = self.read(i.src2, i)? & !1;
                if !self.cond(i.cc) {
                    return Ok(Flow::Next);
                }
                if matches!(i.op, Op::Jl) {
                    let ret = if i.delay { self.after_delay_slot(bus, i) } else { i.next() };
                    self.regs[reg::BLINK] = ret;
                }
                if i.set_flags {
                    // J.F [ILINKn]: STATUS32 <- STATUS32_Ln (PRM, Jcc).
                    let saved = if i.src2 == Src::Reg(reg::ILINK1 as u8) { self.status32_l1 } else { self.status32_l2 };
                    self.status32 = saved & status32::IMPLEMENTED & !status32::H;
                }
                return Ok(Flow::Jump(target));
            }
            Op::Lp => {
                if self.cond(i.cc) {
                    self.lp_start = i.next();
                    self.lp_end = i.target;
                    return Ok(Flow::Next);
                }
                return Ok(Flow::Jump(i.target));
            }
            _ => {}
        }

        if !self.cond(i.cc) {
            return Ok(Flow::Next);
        }

        match i.op {
            Op::Ld => return self.exec_load(i, bus),
            Op::St => return self.exec_store(i, bus),
            Op::Push => {
                let v = self.read(i.data, i)?;
                let sp = self.regs[reg::SP].wrapping_sub(4);
                self.regs[reg::SP] = sp;
                bus.write32(sp, v);
                return Ok(Flow::Next);
            }
            Op::Pop => {
                let sp = self.regs[reg::SP];
                let v = bus.read32(sp);
                self.regs[reg::SP] = sp.wrapping_add(4);
                self.write_dst(i.dst, v);
                return Ok(Flow::Next);
            }
            Op::Flag => {
                let v = self.read(i.src2, i)?;
                if v & status32::H != 0 {
                    // "If the H flag is set ... all other flag states are
                    // ignored and are not updated" (PRM, FLAG).
                    self.status32 |= status32::H;
                    self.debug |= debug::SH;
                    return Ok(Flow::Halt(HaltReason::Flag));
                }
                self.status32 = (self.status32 & !status32::FLAG_WRITABLE) | (v & status32::FLAG_WRITABLE);
                return Ok(Flow::Next);
            }
            Op::Lr => {
                let n = self.read(i.src2, i)?;
                let v = self.aux_read(n, i, ext)?;
                self.write_dst(i.dst, v);
                return Ok(Flow::Next);
            }
            Op::Sr => {
                let v = self.read(i.src1, i)?;
                let n = self.read(i.src2, i)?;
                self.aux_write(n, v, i, ext, bus)?;
                return Ok(Flow::Next);
            }
            Op::Sleep => {
                self.sleeping = true;
                self.debug |= debug::ZZ;
                return Ok(Flow::Next);
            }
            Op::Brk => {
                self.status32 |= status32::H;
                self.debug |= debug::BH;
                return Ok(Flow::Halt(HaltReason::Brk));
            }
            // SWI "immediately raises the Instruction Error interrupt ...
            // ILINK2 is used as the return address" (PRM, SWI).
            Op::Swi => return Ok(Flow::Exception(2)),
            Op::Nop => return Ok(Flow::Next),
            _ => {}
        }

        let b = self.read(i.src1, i)?;
        let c = self.read(i.src2, i)?;
        let f = i.set_flags;
        let carry = self.flag(status32::C);
        let shift_carry = |n: u32, bit: u32| if n == 0 { None } else { Some((b >> bit) & 1 != 0) };

        let (result, cf, vf): (Option<u32>, Option<bool>, Option<bool>) = match i.op {
            Op::Add | Op::Add1 | Op::Add2 | Op::Add3 => {
                let k = match i.op {
                    Op::Add1 => 1,
                    Op::Add2 => 2,
                    Op::Add3 => 3,
                    _ => 0,
                };
                let (r, cy, v) = add(b, c << k, false);
                (Some(r), Some(cy), Some(v))
            }
            Op::Adc => {
                let (r, cy, v) = add(b, c, carry);
                (Some(r), Some(cy), Some(v))
            }
            Op::Sub | Op::Sub1 | Op::Sub2 | Op::Sub3 => {
                let k = match i.op {
                    Op::Sub1 => 1,
                    Op::Sub2 => 2,
                    Op::Sub3 => 3,
                    _ => 0,
                };
                let (r, bw, v) = sub(b, c << k, false);
                (Some(r), Some(bw), Some(v))
            }
            Op::Sbc => {
                let (r, bw, v) = sub(b, c, carry);
                (Some(r), Some(bw), Some(v))
            }
            Op::Rsub => {
                let (r, bw, v) = sub(c, b, false);
                (Some(r), Some(bw), Some(v))
            }
            Op::Neg => (Some(0u32.wrapping_sub(c)), None, None),
            Op::Cmp => {
                let (r, bw, v) = sub(b, c, false);
                self.flags(r, Some(bw), Some(v));
                return Ok(Flow::Next);
            }
            Op::Rcmp => {
                let (r, bw, v) = sub(c, b, false);
                self.flags(r, Some(bw), Some(v));
                return Ok(Flow::Next);
            }
            Op::Tst => {
                self.flags(b & c, None, None);
                return Ok(Flow::Next);
            }
            Op::Btst => {
                self.flags(b & (1 << (c & 31)), None, None);
                return Ok(Flow::Next);
            }
            Op::Max | Op::Min => {
                let (d, _, v) = sub(b, c, false);
                let take_c = if matches!(i.op, Op::Max) { (c as i32) >= (b as i32) } else { (c as i32) <= (b as i32) };
                let r = if take_c { c } else { b };
                self.write_dst(i.dst, r);
                if f {
                    // Z/N/V from src1-src2; C set if src2 is selected (PRM, MAX/MIN).
                    self.flags(d, Some(take_c), Some(v));
                }
                return Ok(Flow::Next);
            }
            Op::And => (Some(b & c), None, None),
            Op::Or => (Some(b | c), None, None),
            Op::Bic => (Some(b & !c), None, None),
            Op::Xor => (Some(b ^ c), None, None),
            Op::Mov => (Some(c), None, None),
            Op::Bset => (Some(b | (1 << (c & 31))), None, None),
            Op::Bclr => (Some(b & !(1 << (c & 31))), None, None),
            Op::Bxor => (Some(b ^ (1 << (c & 31))), None, None),
            Op::Bmsk => {
                let n = c & 31;
                let mask = if n == 31 { u32::MAX } else { (1u32 << (n + 1)) - 1 };
                (Some(b & mask), None, None)
            }
            // Single-operand, source in src2 (PRM, ASL/ASR/LSR/ROR/RRC/RLC).
            Op::Asl => {
                let r = c << 1;
                (Some(r), Some(c >> 31 != 0), Some((c ^ r) >> 31 != 0))
            }
            Op::Asr => (Some(((c as i32) >> 1) as u32), Some(c & 1 != 0), None),
            Op::Lsr => (Some(c >> 1), Some(c & 1 != 0), None),
            Op::Ror => (Some(c.rotate_right(1)), Some(c & 1 != 0), None),
            Op::Rrc => (Some(((carry as u32) << 31) | (c >> 1)), Some(c & 1 != 0), None),
            // RLC: "V = Undefined"; left unchanged.
            Op::Rlc => (Some((c << 1) | carry as u32), Some(c >> 31 != 0), None),
            Op::Sexb => (Some(c as u8 as i8 as i32 as u32), None, None),
            Op::Sexw => (Some(c as u16 as i16 as i32 as u32), None, None),
            Op::Extb => (Some(c & 0xFF), None, None),
            Op::Extw => (Some(c & 0xFFFF), None, None),
            Op::Abs => {
                let r = (c as i32).wrapping_abs() as u32;
                // N and V set if src = 0x8000_0000; C = MSB of src (PRM, ABS).
                (Some(r), Some(c >> 31 != 0), Some(c == 0x8000_0000))
            }
            Op::Not => (Some(!c), None, None),
            // Barrel shifts: only the bottom 5 bits of src2 are used. For a
            // shift of 0 no bit is shifted out and C is left unchanged
            // (HYPOTHESIS: the PRM does not cover the zero case).
            Op::AslM => {
                let n = c & 31;
                (Some(b << n), shift_carry(n, 32u32.wrapping_sub(n) & 31), None)
            }
            Op::LsrM => {
                let n = c & 31;
                (Some(b >> n), shift_carry(n, n.wrapping_sub(1)), None)
            }
            Op::AsrM => {
                let n = c & 31;
                (Some(((b as i32) >> n) as u32), shift_carry(n, n.wrapping_sub(1)), None)
            }
            Op::RorM => {
                let n = c & 31;
                (Some(b.rotate_right(n)), shift_carry(n, n.wrapping_sub(1)), None)
            }
            Op::Mul64 | Op::Mulu64 => {
                let p = if matches!(i.op, Op::Mul64) {
                    ((b as i32 as i64) * (c as i32 as i64)) as u64
                } else {
                    b as u64 * c as u64
                };
                // Leapster: r57 is the dual MAC's low-lane accumulator (see
                // MULDW), and MUL64 leaves it alone; the low word is read from
                // MMID instead. HYPOTHESIS from the Leapster 2 BIOS 0.6: its
                // interrupt entry (0x4004_B308) saves only r58/r59 and restores
                // them with `mulu64 0,r0,1; sr r6,[mulhi]` (0x4004_B434), while
                // its PCM filter reads r57 after MACs (0x4003_E0B0). With MUL64
                // writing r57, every interrupt landing on that read corrupted
                // the filter, which then rang at full scale (audible clicks in
                // The Talking Words Factory, each one at an interrupt there).
                if self.config.mmid != MmidMode::LowLeapster {
                    self.regs[reg::MLO] = p as u32;
                }
                self.regs[reg::MHI] = (p >> 32) as u32;
                self.regs[reg::MMID] = match self.config.mmid {
                    MmidMode::Middle => (p >> 16) as u32,
                    MmidMode::LowLeapster => p as u32,
                };
                return Ok(Flow::Next);
            }
            Op::Adds => {
                let (r, s) = sat32(b as i32 as i64 + c as i32 as i64);
                self.sticky_saturation(s, s);
                (Some(r), Some(b.overflowing_add(c).1), Some(s))
            }
            Op::Subs => {
                let (r, s) = sat32(b as i32 as i64 - c as i32 as i64);
                self.sticky_saturation(s, s);
                (Some(r), Some(b < c), Some(s))
            }
            Op::Addsdw | Op::Subsdw => {
                let lane = |x: u32, y: u32| {
                    let (x, y) = (x as u16 as i16 as i32, y as u16 as i16 as i32);
                    sat16(if matches!(i.op, Op::Addsdw) { x + y } else { x - y })
                };
                let (hi, s1) = lane(b >> 16, c >> 16);
                let (lo, s2) = lane(b, c);
                self.sticky_saturation(s1, s2);
                (Some(((hi as u32) << 16) | lo as u32), None, Some(s1 || s2))
            }
            Op::Muldw | Op::Macdw | Op::Msubdw => {
                // SOURCED (MAME `arcompact_device::dsp_multiply`), UNVERIFIED:
                // no specification of the dual 16x16 MAC option was found (the
                // PRM documents only AUX_MACMODE and the dual 16-bit format).
                // Each lane (low = channel 2, high = channel 1) multiplies the
                // signed 16-bit halves; MULDW sets the lane's accumulator, MACDW
                // adds to it, MSUBDW subtracts. The lane's result is the
                // accumulator plus a rounding half (AUX_MACMODE bit 2) shifted
                // right by 16, saturated to 16 bits (sticky S2 / S1). In
                // fractional mode (AUX_MACMODE bit 3) each product is doubled
                // (Q15 x Q15 -> Q31); MAME instead shifts the sum right by 15,
                // which gives the same lane results.
                //
                // The accumulators are readable as r56 (high lane) and r57 (low
                // lane). VERIFIED-FW: the Leapster 2 BIOS 0.6's PCM filter
                // (0x4003_E0A4, AUX_MACMODE 0x12C) reads them right after its
                // MACs (`adds r9,r56,r57`, then `rnd16`, a residual fed back,
                // and the result doubled with `adds r10,r10,r10`). Its
                // coefficients are stored halved (0x7EA6 / 0xC115 = poles at
                // 0.99, a DC-blocking high-pass), which needs the doubled Q31
                // products: read as Q30 the filter's poles move and the output
                // carried 17% of its energy above 12 kHz (hiss); unread, it
                // saturated at +/-32767. Reading an accumulator beyond 32 bits
                // gives it saturated (UNVERIFIED).
                let mut r = 0u32;
                let mut sat = [false; 2];
                for (lane, saturated) in sat.iter_mut().enumerate() {
                    let fractional = self.macmode & (1 << 3) != 0;
                    let product = ((b >> (16 * lane)) as u16 as i16 as i64 * (c >> (16 * lane)) as u16 as i16 as i64)
                        << fractional as u32;
                    let acc = &mut self.mac_acc[lane];
                    *acc = match i.op {
                        Op::Muldw => product,
                        Op::Macdw => acc.wrapping_add(product),
                        _ => acc.wrapping_sub(product),
                    };
                    let round = if self.macmode & (1 << 2) != 0 { 0x4000 << fractional as u32 } else { 0 };
                    let scaled = (*acc + round) >> 16;
                    let out = scaled.clamp(i16::MIN as i64, i16::MAX as i64);
                    *saturated = out != scaled;
                    r |= (out as u16 as u32) << (16 * lane);
                }
                self.sticky_saturation(sat[1], sat[0]);
                self.regs[56] = sat32(self.mac_acc[1]).0;
                self.regs[57] = sat32(self.mac_acc[0]).0;
                (Some(r), None, None)
            }
            Op::Asls | Op::Asrs => {
                let n = c as i32;
                let left = if matches!(i.op, Op::Asls) { n } else { n.wrapping_neg() };
                let (r, s) = if left >= 0 {
                    if left > 31 {
                        // PRM, ASLS: saturates to 0x7FFF_FFFF / 0x8000_0000 for
                        // positive non-zero / negative input.
                        match (b as i32).signum() {
                            0 => (0, false),
                            1 => (i32::MAX as u32, true),
                            _ => (i32::MIN as u32, true),
                        }
                    } else {
                        sat32((b as i32 as i64) << left)
                    }
                } else {
                    ((((b as i32) >> left.unsigned_abs().min(31)) as u32), false)
                };
                self.sticky_saturation(s, s);
                (Some(r), None, Some(s))
            }
            Op::Rnd16 => {
                // PRM instruction page: (sat32(src + 0x8000) & 0xFFFF0000) >> 16;
                // the summary tables omit the shift. VERIFIED-FW for the
                // shift: the Leapster 2 BIOS 0.6 follows `rnd16 r10,r9` with
                // `asl r10,r10,16; sub r11,r9,r10` (0x4003_E0B8). The upper 16
                // bits (zero here, as the formula reads) are UNVERIFIED.
                let (r, s) = sat32(c as i32 as i64 + 0x8000);
                self.sticky_saturation(s, s);
                (Some((r & 0xFFFF_0000) >> 16), None, Some(s))
            }
            Op::Sat16 => {
                let (r, s) = sat16(c as i32);
                self.sticky_saturation(s, s);
                (Some(r as i16 as i32 as u32), None, Some(s))
            }
            Op::Abss | Op::Negs => {
                let x = c as i32 as i64;
                let (r, s) = sat32(if matches!(i.op, Op::Abss) { x.abs() } else { -x });
                self.sticky_saturation(s, s);
                self.write_dst(i.dst, r);
                if f {
                    if matches!(i.op, Op::Abss) {
                        // "In contrast with other instructions, the value of
                        // the input operand is used to set the flags" (PRM, ABSS).
                        self.flags(c, None, Some(s));
                    } else {
                        self.flags(r, None, Some(s));
                    }
                }
                return Ok(Flow::Next);
            }
            Op::Abssw | Op::Negsw => {
                let x = c as u16 as i16 as i32;
                let (r, s) = sat16(if matches!(i.op, Op::Abssw) { x.abs() } else { -x });
                let r = r as i16 as i32 as u32;
                self.sticky_saturation(s, s);
                self.write_dst(i.dst, r);
                if f {
                    if matches!(i.op, Op::Abssw) {
                        // Flags from the 16-bit input (PRM, ABSSW).
                        self.flags((c as u16 as i16 as i32) as u32, None, Some(s));
                    } else {
                        self.flags(r, None, Some(s));
                    }
                }
                return Ok(Flow::Next);
            }
            Op::Swap => (Some(c.rotate_left(16)), None, None),
            Op::Norm => {
                self.write_dst(i.dst, norm32(c));
                if f {
                    // Z/N describe the source (PRM, NORM).
                    self.flags(c, None, None);
                }
                return Ok(Flow::Next);
            }
            Op::Normw => {
                self.write_dst(i.dst, norm16(c));
                if f {
                    self.flags(c as u16 as i16 as i32 as u32, None, None);
                }
                return Ok(Flow::Next);
            }
            other => return Err(self.unsupported(i, &format!("no executor for {other:?}"))),
        };

        if let Some(r) = result {
            self.write_dst(i.dst, r);
            if f && !i.short {
                self.flags(r, cf, vf);
            }
        }
        Ok(Flow::Next)
    }

    fn sticky_saturation(&mut self, s1: bool, s2: bool) {
        if s1 {
            self.macmode |= MACMODE_S1;
        }
        if s2 {
            self.macmode |= MACMODE_S2;
        }
    }

    /// Effective address and base write-back (PRM Table 52).
    fn address(&mut self, i: &Instr) -> Result<u32, CpuError> {
        let base = self.read(i.src1, i)?;
        let off = self.read(i.src2, i)?;
        let scale = match i.mem.size {
            Size::Word => 2,
            Size::Half => 1,
            Size::Byte => 0,
        };
        let (addr, new_base) = match i.mem.wb {
            WriteBack::None => (base.wrapping_add(off), None),
            WriteBack::Pre => (base.wrapping_add(off), Some(base.wrapping_add(off))),
            WriteBack::Post => (base, Some(base.wrapping_add(off))),
            WriteBack::Scaled => (base.wrapping_add(off << scale), None),
        };
        if let (Some(nb), Src::Reg(r)) = (new_base, i.src1) {
            self.check_dst(Dst::Reg(r), i)?;
            self.write_dst(Dst::Reg(r), nb);
        }
        Ok(addr)
    }

    fn exec_load(&mut self, i: &Instr, bus: &mut impl Bus) -> Result<Flow, CpuError> {
        if let (Dst::Reg(d), Src::Reg(b)) = (i.dst, i.src1)
            && d == b
            && i.mem.wb != WriteBack::None
            && i.mem.wb != WriteBack::Scaled
        {
            return Err(self.unpredictable(i, "load with write-back into its own base register"));
        }
        let addr = self.address(i)?;
        if i.dst == Dst::Discard {
            // PREFETCH: "The returning load is not written to any core
            // register." No cache is modelled, so no memory access is made.
            return Ok(Flow::Next);
        }
        let v = match (i.mem.size, i.mem.sign_extend) {
            (Size::Word, _) => bus.read32(addr),
            (Size::Half, false) => bus.read16(addr) as u32,
            (Size::Half, true) => bus.read16(addr) as i16 as i32 as u32,
            (Size::Byte, false) => bus.read8(addr) as u32,
            (Size::Byte, true) => bus.read8(addr) as i8 as i32 as u32,
        };
        self.write_dst(i.dst, v);
        Ok(Flow::Next)
    }

    fn exec_store(&mut self, i: &Instr, bus: &mut impl Bus) -> Result<Flow, CpuError> {
        let v = self.read(i.data, i)?;
        let addr = self.address(i)?;
        match i.mem.size {
            Size::Word => bus.write32(addr, v),
            Size::Half => bus.write16(addr, v as u16),
            Size::Byte => bus.write8(addr, v as u8),
        }
        Ok(Flow::Next)
    }
}
