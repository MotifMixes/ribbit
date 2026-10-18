//! x86-64 code generation (dynasm-rs).
//!
//! The generated function is `extern "sysv64" fn(*mut JitCtx) -> u32` (the
//! `ExitKind`). Register use, fixed for the whole function:
//!
//! | Host | Holds |
//! |---|---|
//! | `r12` | the `JitCtx` |
//! | `rbx` | `&regs[0]` (guest rN at `[rbx + 4*N]`) |
//! | `r13` | `&status32` |
//! | `r14d` | a taken branch's target while its delay slot runs |
//! | `r15d` | 1 if that branch was taken, 0 if not |
//!
//! All five are callee-saved in the System V ABI, so they survive the memory
//! helpers; everything else is scratch. Each instruction is emitted from its
//! `Fast` form exactly as `Cpu::execute` (the general executor) computes it;
//! tests/lockstep.rs holds the two side by side.

use crate::JitCtx;
use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use ribbit_cpu::icache::{AluOp, DISCARD, Fast, Hot, LdKind, UnOp};
use ribbit_cpu::isa::{BrCond, COND_AL, Cond, Instr, Size, WriteBack, cond_holds};
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
const LP_START: i32 = offset_of!(JitCtx, lp_start) as i32;
const LP_END: i32 = offset_of!(JitCtx, lp_end) as i32;
const PC: i32 = offset_of!(JitCtx, pc) as i32;
const EXECUTED: i32 = offset_of!(JitCtx, executed) as i32;
const DELAY_PENDING: i32 = offset_of!(JitCtx, delay_pending) as i32;
const DELAY_TARGET: i32 = offset_of!(JitCtx, delay_target) as i32;
const REGS: i32 = offset_of!(JitCtx, regs) as i32;
const STATUS32: i32 = offset_of!(JitCtx, status32) as i32;

const EXIT_END: i32 = crate::ExitKind::End as i32;
const EXIT_FALLBACK: i32 = crate::ExitKind::Fallback as i32;
const EXIT_LOOP: i32 = crate::ExitKind::Loop as i32;

/// Displacement of guest core register `r` (r0-r31) from `rbx`.
fn r(n: u8) -> i32 {
    4 * (n & 31) as i32
}

const SP: i32 = 4 * reg::SP as i32;
const BLINK: i32 = 4 * reg::BLINK as i32;
const LP_COUNT: i32 = 4 * reg::LP_COUNT as i32;

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
    /// Z, N, C (shifts by a nonzero amount).
    Znc,
}

/// Where an exit's PC comes from.
#[derive(Clone, Copy)]
enum Pc {
    Const(u32),
    /// `eax`.
    Eax,
    /// `r14d` (a taken branch's target).
    R14,
}

fn is_branch(f: Fast) -> bool {
    matches!(f, Fast::Br { .. } | Fast::BrR { .. } | Fast::B { .. } | Fast::Bl { .. } | Fast::BlD { .. } | Fast::J { .. } | Fast::Jcc { .. })
}

/// Compiled by this generator (everything else falls back).
fn compilable(h: &Hot) -> bool {
    !h.stop && h.fast != Fast::General
}

/// For each condition code, bit `i` says whether it holds for flags
/// `i = V | C << 1 | N << 2 | Z << 3` (STATUS32 bits 8-11), from the
/// interpreter's own `cond_holds`.
fn cond_table(cc: Cond) -> u32 {
    (0..16u32).filter(|&i| cond_holds(cc, i & 8 != 0, i & 4 != 0, i & 2 != 0, i & 1 != 0)).fold(0, |t, i| t | 1 << i)
}

struct Gen {
    ops: Asm,
    epilogue: DynamicLabel,
    /// Out-of-line loop-end stubs: (label, sequential next PC, executed).
    loops: Vec<(DynamicLabel, u32, u32)>,
}

/// Generates the code for `instrs`; returns it with the number of
/// instructions covered, or None if the first one cannot be compiled.
pub(crate) fn emit(instrs: &[Instr]) -> Option<(Vec<u8>, usize)> {
    let hots: Vec<Hot> = instrs.iter().map(Hot::of).collect();
    if !compilable(&hots[0]) || matches!(hots[0].fast, Fast::BlD { .. }) && hots.len() < 2 {
        return None;
    }
    let mut ops = Asm::new(0);
    let epilogue = ops.new_dynamic_label();
    let mut g = Gen { ops, epilogue, loops: Vec::new() };
    g.prologue();
    let mut k = 0;
    let compiled = loop {
        let Some(h) = hots.get(k) else {
            // Off the end of the run (the last instruction's loop check is done).
            g.exit(EXIT_END, Pc::Const(instrs[k - 1].next()), k as u32);
            break k;
        };
        if !compilable(h) {
            g.exit(EXIT_FALLBACK, Pc::Const(h.addr), k as u32);
            break k;
        }
        if is_branch(h.fast) {
            break g.branch(&hots, k);
        }
        g.body(h);
        g.loop_check(h.next, k as u32 + 1);
        k += 1;
    };
    g.epilogue();
    let ops = g.ops;
    let code = ops.finalize().ok()?;
    Some((code, compiled))
}

impl Gen {
    fn prologue(&mut self) {
        // Five pushes after the return address leave rsp 16-byte aligned for
        // the helper calls.
        dynasm!(self.ops
            ; .arch x64
            ; push rbx
            ; push r12
            ; push r13
            ; push r14
            ; push r15
            ; mov r12, rdi
            ; mov rbx, QWORD [r12 + REGS]
            ; mov r13, QWORD [r12 + STATUS32]
        );
    }

    fn epilogue(&mut self) {
        let epilogue = self.epilogue;
        dynasm!(self.ops
            ; .arch x64
            ; =>epilogue
            ; pop r15
            ; pop r14
            ; pop r13
            ; pop r12
            ; pop rbx
            ; ret
        );
        for (label, next, executed) in std::mem::take(&mut self.loops) {
            // The interpreter's `loop_end_check`: LP_COUNT -= 1; back to
            // LP_START unless it was 1.
            let stay = self.ops.new_dynamic_label();
            dynasm!(self.ops
                ; .arch x64
                ; =>label
                ; mov eax, DWORD [rbx + LP_COUNT]
                ; lea ecx, [rax - 1]
                ; mov DWORD [rbx + LP_COUNT], ecx
                ; cmp eax, 1
                ; je =>stay
                ; mov eax, DWORD [r12 + LP_START]
            );
            self.exit(EXIT_LOOP, Pc::Eax, executed);
            dynasm!(self.ops ; .arch x64 ; =>stay);
            self.exit(EXIT_LOOP, Pc::Const(next), executed);
        }
    }

    /// Writes the exit's PC and count and returns `kind`.
    fn exit(&mut self, kind: i32, pc: Pc, executed: u32) {
        match pc {
            Pc::Const(v) => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + PC], v as i32),
            Pc::Eax => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + PC], eax),
            Pc::R14 => dynasm!(self.ops ; .arch x64 ; mov DWORD [r12 + PC], r14d),
        }
        let epilogue = self.epilogue;
        dynasm!(self.ops
            ; .arch x64
            ; mov DWORD [r12 + EXECUTED], executed as i32
            ; mov eax, kind
            ; jmp =>epilogue
        );
    }

    /// Sequential flow after an instruction (`next`): the zero-overhead
    /// loop check (`Cpu::loop_end_check`), out of line.
    fn loop_check(&mut self, next: u32, executed: u32) {
        let stub = self.ops.new_dynamic_label();
        self.loops.push((stub, next, executed));
        dynasm!(self.ops
            ; .arch x64
            ; cmp DWORD [r12 + LP_END], next as i32
            ; je =>stub
        );
    }

    /// Jumps to `skip` unless condition `cc` holds.
    fn cond_skip(&mut self, cc: Cond, skip: DynamicLabel) {
        if cc == COND_AL {
            return;
        }
        let table = cond_table(cc);
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, DWORD [r13]
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
            // Shifts: C first, then Z/N from the result (a zero count
            // leaves the host's ZF/SF alone).
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
            ; mov edx, DWORD [r13]
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
        dynasm!(self.ops ; .arch x64 ; mov DWORD [r13], edx);
    }

    /// Host CF = the guest's C flag.
    fn load_carry(&mut self) {
        dynasm!(self.ops ; .arch x64 ; bt DWORD [r13], C_BIT);
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
        if d != DISCARD {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + r(d)], eax);
        }
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
                self.call(WRITE32);
            }
            Fast::Pop { d } => {
                dynasm!(self.ops ; .arch x64 ; mov esi, DWORD [rbx + SP]);
                self.call(READ32);
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
            | Fast::B { .. }
            | Fast::Bl { .. }
            | Fast::BlD { .. }
            | Fast::J { .. }
            | Fast::Jcc { .. } => unreachable!("not a straight-line form: {:?}", h.fast),
        }
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

    /// The branch at `k` (and its delay slot); returns the number of
    /// instructions the block covers.
    fn branch(&mut self, hots: &[Hot], k: usize) -> usize {
        let h = hots[k];
        let executed = k as u32 + 1;
        // The delay slot, when it can run here: compiled, and not itself a
        // branch (the interpreter reports that as unpredictable).
        let slot = hots.get(k + 1).filter(|s| h.delay && compilable(s) && !s.branchy);
        // BL.D's link is the address after the slot.
        let link = match h.fast {
            Fast::Bl { .. } => Some(h.next),
            Fast::BlD { .. } => match hots.get(k + 1) {
                Some(s) => Some(s.next),
                None => {
                    // The slot's length is unknown here: let the interpreter run it.
                    self.exit(EXIT_FALLBACK, Pc::Const(h.addr), k as u32);
                    return k;
                }
            },
            _ => None,
        };
        let not_taken = self.ops.new_dynamic_label();
        // Condition (jump to `not_taken` if false), then the target in r14d.
        match h.fast {
            Fast::B { cc } | Fast::Bl { cc } | Fast::BlD { cc } => {
                self.cond_skip(cc, not_taken);
                dynasm!(self.ops ; .arch x64 ; mov r14d, h.target as i32);
            }
            Fast::Br { cond, b, v } => {
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + r(b)] ; cmp eax, v as i32);
                self.br_skip(cond, not_taken);
                dynasm!(self.ops ; .arch x64 ; mov r14d, h.target as i32);
            }
            Fast::BrR { cond, b, c } => {
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + r(b)] ; cmp eax, DWORD [rbx + r(c)]);
                self.br_skip(cond, not_taken);
                dynasm!(self.ops ; .arch x64 ; mov r14d, h.target as i32);
            }
            Fast::J { s } => dynasm!(self.ops
                ; .arch x64
                ; mov r14d, DWORD [rbx + r(s)]
                ; and r14d, -2
            ),
            Fast::Jcc { cc, s } => {
                self.cond_skip(cc, not_taken);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov r14d, DWORD [rbx + r(s)]
                    ; and r14d, -2
                );
            }
            _ => unreachable!(),
        }
        // Taken.
        if let Some(link) = link {
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + BLINK], link as i32);
        }
        let slot_label = self.ops.new_dynamic_label();
        if !h.delay {
            self.exit(EXIT_END, Pc::R14, executed);
        } else if slot.is_some() {
            dynasm!(self.ops ; .arch x64 ; mov r15d, 1 ; jmp =>slot_label);
        } else {
            // The slot runs in the interpreter, with `delay_target` set.
            dynasm!(self.ops
                ; .arch x64
                ; mov DWORD [r12 + DELAY_PENDING], 1
                ; mov DWORD [r12 + DELAY_TARGET], r14d
            );
            self.exit(EXIT_END, Pc::Const(h.next), executed);
        }
        // Not taken: sequential flow, with the loop check.
        dynasm!(self.ops ; .arch x64 ; =>not_taken);
        self.loop_check(h.next, executed);
        let Some(slot) = slot else {
            self.exit(EXIT_END, Pc::Const(h.next), executed);
            return k + 1;
        };
        dynasm!(self.ops ; .arch x64 ; xor r15d, r15d ; =>slot_label);
        self.body(slot);
        // After the slot: the taken branch's target, or on in sequence.
        let sequential = self.ops.new_dynamic_label();
        dynasm!(self.ops ; .arch x64 ; test r15d, r15d ; jz =>sequential);
        self.exit(EXIT_END, Pc::R14, executed + 1);
        dynasm!(self.ops ; .arch x64 ; =>sequential);
        self.loop_check(slot.next, executed + 1);
        self.exit(EXIT_END, Pc::Const(slot.next), executed + 1);
        k + 2
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
