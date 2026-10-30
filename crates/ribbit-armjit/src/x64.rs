//! x86-64 code generation (dynasm-rs).
//!
//! A block is `extern "sysv64" fn(*mut Ctx) -> u32` (an `exit` code). Host
//! registers, fixed for the whole function:
//!
//! | Host | Holds |
//! |---|---|
//! | `r12` | the `Ctx` |
//! | `rbx` | the `Arm` (guest rN at `[rbx + R + 4*N]`, the CPSR and the counters at fixed offsets) |
//! | `r13d` | a load/store's written-back base, across the helper call |
//! | `r15d` | the CPSR (read from `[rbx + CPSR]` on entry and after the interpreter ran an instruction; every change is stored there too) |
//!
//! All four are callee-saved in the System V ABI, so they survive the
//! helpers; everything else is scratch. Each instruction computes exactly
//! what `arm::execute` computes; the NZCV flags are stored to the CPSR after
//! every flag-setting instruction. (The CPSR in a register keeps its reads
//! off the store-to-load path: flag-setting and conditional instructions
//! follow each other closely.)

use crate::decode::{Instr, Operand};
use crate::{Ctx, FAST, RECENT, Recent, Stats, exit};
use dynasmrt::x64::X64Relocation;
use dynasmrt::{DynamicLabel, DynasmApi, DynasmLabelApi, VecAssembler, dynasm};
use ribbit_arm::{Arm, psr};
use std::mem::offset_of;

type Asm = VecAssembler<X64Relocation>;

const R: i32 = offset_of!(Arm, r) as i32;
const R15: i32 = R + 60;
const CPSR: i32 = offset_of!(Arm, cpsr) as i32;
const CYCLES: i32 = offset_of!(Arm, cycles) as i32;
const INSTRUCTIONS: i32 = offset_of!(Arm, instructions) as i32;
const CURRENT: i32 = offset_of!(Arm, current) as i32;
const CPU: i32 = offset_of!(Ctx, cpu) as i32;
const LOAD32: i32 = offset_of!(Ctx, load32) as i32;
const LOAD8: i32 = offset_of!(Ctx, load8) as i32;
const LOAD16: i32 = offset_of!(Ctx, load16) as i32;
const STORE32: i32 = offset_of!(Ctx, store32) as i32;
const STORE8: i32 = offset_of!(Ctx, store8) as i32;
const STORE16: i32 = offset_of!(Ctx, store16) as i32;
const EXEC: i32 = offset_of!(Ctx, exec) as i32;
const RAM: i32 = offset_of!(Ctx, ram) as i32;
const READS: i32 = offset_of!(Ctx, reads) as i32;
const WRITES: i32 = offset_of!(Ctx, writes) as i32;
const STATS: i32 = offset_of!(Ctx, stats) as i32;
const RECENTS: i32 = offset_of!(Ctx, recent) as i32;
const LIMIT: i32 = offset_of!(Ctx, limit) as i32;
const BUDGET_END: i32 = offset_of!(Ctx, budget_end) as i32;
const DIRTY: i32 = offset_of!(Ctx, dirty) as i32;
const BLOCKS_RUN: i32 = offset_of!(Stats, blocks_run) as i32;
const JIT_INSTRUCTIONS: i32 = offset_of!(Stats, jit_instructions) as i32;
const LINKED: i32 = offset_of!(Stats, linked) as i32;
const RECENT_LEN: i32 = offset_of!(Recent, len) as i32;
const RECENT_CODE: i32 = offset_of!(Recent, code) as i32;
/// Bytes of the prologue every block starts with: a block going straight
/// on to the next jumps past it, with the registers and stack already set.
const PROLOGUE: i32 = 31;

/// Access sizes for `Gen::mem`.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Size {
    Word,
    Byte,
    Half,
    SignedByte,
    SignedHalf,
}

fn reg(n: u32) -> i32 {
    R + 4 * n as i32
}

/// For each condition, bit `nzcv` is set when it holds with those flags.
const fn cond_table(c: u32) -> u32 {
    let mut t = 0;
    let mut f = 0;
    while f < 16 {
        let (n, z, cy, v) = (f & 8 != 0, f & 4 != 0, f & 2 != 0, f & 1 != 0);
        let holds = match c {
            0x0 => z,
            0x1 => !z,
            0x2 => cy,
            0x3 => !cy,
            0x4 => n,
            0x5 => !n,
            0x6 => v,
            0x7 => !v,
            0x8 => cy && !z,
            0x9 => !cy || z,
            0xA => n == v,
            0xB => n != v,
            0xC => !z && n == v,
            0xD => z || n != v,
            _ => true,
        };
        if holds {
            t |= 1 << f;
        }
        f += 1;
    }
    t
}

/// A fault exit after `count` instructions (the helper took the abort).
struct FaultStub {
    label: DynamicLabel,
    count: u32,
}

/// A store that wrote compiled code: the block ends after it.
struct StoreStub {
    label: DynamicLabel,
    fault: DynamicLabel,
    count: u32,
    pc: u32,
    writeback: Option<u32>,
}

/// An interpreted instruction that ends the block (r15 already set).
struct ExecStub {
    label: DynamicLabel,
    count: u32,
    pc: u32,
}

struct Gen {
    ops: Asm,
    done: DynamicLabel,
    faults: Vec<FaultStub>,
    stores: Vec<StoreStub>,
    execs: Vec<ExecStub>,
    /// Instructions after which the block ends early with r15 the next
    /// one: block transfers that wrote compiled code, MSRs that unmasked
    /// interrupts. With the exit code.
    afters: Vec<(ExecStub, u32)>,
}

/// Machine code for `instrs`, the block at `va` (all ARM, one 1 KB page).
pub(crate) fn emit(va: u32, instrs: &[Instr]) -> Option<Vec<u8>> {
    let mut ops = Asm::new(0);
    let (done, ret) = (ops.new_dynamic_label(), ops.new_dynamic_label());
    let mut g = Gen { ops, done, faults: Vec::new(), stores: Vec::new(), execs: Vec::new(), afters: Vec::new() };
    // Five pushes and 64 bytes after the return address leave rsp 16-byte
    // aligned for the helper calls; [rsp] holds 16 words for LDM.
    dynasm!(g.ops
        ; .arch x64
        ; push rbx
        ; push r12
        ; push r13
        ; push r14
        ; push r15
        ; sub rsp, 64
        ; mov r12, rdi
        ; mov rbx, QWORD [r12 + CPU]
        ; mov r15d, DWORD [rbx + CPSR]
    );
    assert_eq!(g.ops.offset().0, PROLOGUE as usize);
    let n = instrs.len() as u32;
    for (i, ins) in instrs.iter().enumerate() {
        let pc = va.wrapping_add(4 * i as u32);
        let skip = g.ops.new_dynamic_label();
        if ins.cond() != 0xE {
            g.cond(ins.cond(), skip);
        }
        g.instr(ins, pc, i as u32 + 1);
        dynasm!(g.ops ; .arch x64 ; =>skip);
    }
    let next = va.wrapping_add(4 * n);
    let last = next.wrapping_sub(4);
    let code = exit::DONE;
    dynasm!(g.ops
        ; .arch x64
        ; mov DWORD [rbx + R15], next as i32
        ; =>done
        ; add QWORD [rbx + CYCLES], n as i32
        ; add QWORD [rbx + INSTRUCTIONS], n as i32
        ; mov DWORD [rbx + CURRENT], last as i32
    );
    g.next_block();
    dynasm!(g.ops
        ; .arch x64
        ; mov eax, code as i32
        ; =>ret
        ; add rsp, 64
        ; pop r15
        ; pop r14
        ; pop r13
        ; pop r12
        ; pop rbx
        ; ret
    );
    for s in std::mem::take(&mut g.stores) {
        let (label, fault) = (s.label, s.fault);
        dynasm!(g.ops
            ; .arch x64
            ; =>label
            ; cmp eax, 1
            ; je =>fault
        );
        if let Some(rn) = s.writeback {
            dynasm!(g.ops ; .arch x64 ; mov DWORD [rbx + reg(rn)], r13d);
        }
        dynasm!(g.ops
            ; .arch x64
            ; mov DWORD [rbx + R15], s.pc.wrapping_add(4) as i32
            ; add QWORD [rbx + CYCLES], s.count as i32
            ; add QWORD [rbx + INSTRUCTIONS], s.count as i32
            ; mov DWORD [rbx + CURRENT], s.pc as i32
            ; mov eax, exit::CODE_WRITTEN as i32
            ; jmp =>ret
        );
    }
    for (e, code) in std::mem::take(&mut g.afters) {
        let label = e.label;
        dynasm!(g.ops
            ; .arch x64
            ; =>label
            ; mov DWORD [rbx + R15], e.pc.wrapping_add(4) as i32
            ; add QWORD [rbx + CYCLES], e.count as i32
            ; add QWORD [rbx + INSTRUCTIONS], e.count as i32
            ; mov DWORD [rbx + CURRENT], e.pc as i32
            ; mov eax, code as i32
            ; jmp =>ret
        );
    }
    for e in std::mem::take(&mut g.execs) {
        let label = e.label;
        dynasm!(g.ops
            ; .arch x64
            ; =>label
            ; add QWORD [rbx + CYCLES], e.count as i32
            ; add QWORD [rbx + INSTRUCTIONS], e.count as i32
            ; mov DWORD [rbx + CURRENT], e.pc as i32
            ; mov eax, exit::DONE as i32
            ; jmp =>ret
        );
    }
    for f in std::mem::take(&mut g.faults) {
        let label = f.label;
        dynasm!(g.ops
            ; .arch x64
            ; =>label
            ; add QWORD [rbx + CYCLES], f.count as i32
            ; add QWORD [rbx + INSTRUCTIONS], f.count as i32
            ; mov eax, exit::FAULT as i32
            ; jmp =>ret
        );
    }
    g.ops.finalize().ok()
}

impl Gen {
    /// After a block that ran to its end: jumps to the next block when the
    /// cache has it and nothing needs the run loop (`Ctx::recent`), else
    /// falls through to return.
    fn next_block(&mut self) {
        const _: () = assert!(RECENT.is_power_of_two() && size_of::<Recent>() == 16);
        let out = self.ops.new_dynamic_label();
        dynasm!(self.ops
            ; .arch x64
            ; cmp DWORD [r12 + DIRTY], 0
            ; jne =>out
            ; test r15d, psr::T as i32
            ; jnz =>out
            ; mov rax, QWORD [rbx + CYCLES]
            ; cmp rax, QWORD [r12 + LIMIT]
            ; jae =>out
            ; mov ecx, DWORD [rbx + R15]
            ; mov edx, ecx
            ; shr edx, 2
            ; and edx, (RECENT - 1) as i32
            ; shl rdx, 4
            ; add rdx, QWORD [r12 + RECENTS]
            ; cmp DWORD [rdx], ecx
            ; jne =>out
            ; mov ecx, DWORD [rdx + RECENT_LEN]
            ; add rax, rcx
            ; cmp rax, QWORD [r12 + BUDGET_END]
            ; ja =>out
            ; mov rax, QWORD [r12 + STATS]
            ; add QWORD [rax + BLOCKS_RUN], 1
            ; add QWORD [rax + JIT_INSTRUCTIONS], rcx
            ; add QWORD [rax + LINKED], 1
            ; mov rax, QWORD [rdx + RECENT_CODE]
            ; add rax, PROLOGUE
            ; jmp rax
            ; =>out
        );
    }

    /// Jumps to `skip` unless condition `c` holds.
    fn cond(&mut self, c: u32, skip: DynamicLabel) {
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, r15d
            ; shr eax, 28
            ; mov ecx, cond_table(c) as i32
            ; bt ecx, eax
            ; jnc =>skip
        );
    }

    /// Loads guest register `n` (r15 reads `pc + 8`) into eax.
    fn rn_to_eax(&mut self, n: u32, pc: u32) {
        if n == 15 {
            dynasm!(self.ops ; .arch x64 ; mov eax, pc.wrapping_add(8) as i32);
        } else {
            dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + reg(n)]);
        }
    }

    /// The operand into ecx; with `carry`, the shifter's carry out into edx (0/1).
    fn operand(&mut self, op: Operand, pc: u32, carry: bool) {
        match op {
            Operand::Imm(v, c) => {
                dynasm!(self.ops ; .arch x64 ; mov ecx, v as i32);
                if carry {
                    match c {
                        Some(c) => dynasm!(self.ops ; .arch x64 ; mov edx, c as i32),
                        None => self.c_to_edx(),
                    }
                }
            }
            Operand::Reg { rm, kind, amount } => {
                if rm == 15 {
                    dynasm!(self.ops ; .arch x64 ; mov ecx, pc.wrapping_add(8) as i32);
                } else {
                    dynasm!(self.ops ; .arch x64 ; mov ecx, DWORD [rbx + reg(rm)]);
                }
                let n = amount as i8;
                match (kind, amount) {
                    (0, 0) => {
                        if carry {
                            self.c_to_edx();
                        }
                        return;
                    }
                    (0, _) => dynasm!(self.ops ; .arch x64 ; shl ecx, n),
                    (1, 0) => {
                        // LSR #32
                        dynasm!(self.ops ; .arch x64 ; mov edx, ecx ; shr edx, 31 ; xor ecx, ecx);
                        return;
                    }
                    (1, _) => dynasm!(self.ops ; .arch x64 ; shr ecx, n),
                    (2, 0) => {
                        // ASR #32
                        dynasm!(self.ops ; .arch x64 ; mov edx, ecx ; shr edx, 31 ; sar ecx, 31);
                        return;
                    }
                    (2, _) => dynasm!(self.ops ; .arch x64 ; sar ecx, n),
                    (_, 0) => {
                        // RRX
                        dynasm!(self.ops ; .arch x64 ; bt r15d, 29 ; rcr ecx, 1);
                    }
                    _ => dynasm!(self.ops ; .arch x64 ; ror ecx, n),
                }
                // x86 leaves the last bit shifted out in CF, as ARM's shifter.
                if carry {
                    dynasm!(self.ops ; .arch x64 ; setc dl ; movzx edx, dl);
                }
            }
            Operand::RegShift { rm, kind, rs } => {
                // The value in r10 (64 bits wide, so shifts of 32 and more
                // need no special cases), the amount in ecx.
                let (zero, end) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                dynasm!(self.ops
                    ; .arch x64
                    ; mov r10d, DWORD [rbx + reg(rm)]
                    ; mov ecx, DWORD [rbx + reg(rs)]
                    ; and ecx, 0xFF
                    ; jz =>zero
                );
                match kind {
                    // Shift by n - 1 (at most 63), take the carry, then by 1 more.
                    0 => dynasm!(self.ops
                        ; .arch x64
                        ; mov eax, 64 ; cmp ecx, eax ; cmova ecx, eax ; dec ecx
                        ; shl r10, cl ; bt r10, 31 ; setc dl ; shl r10, 1
                    ),
                    1 => dynasm!(self.ops
                        ; .arch x64
                        ; mov eax, 64 ; cmp ecx, eax ; cmova ecx, eax ; dec ecx
                        ; shr r10, cl ; bt r10, 0 ; setc dl ; shr r10, 1
                    ),
                    2 => dynasm!(self.ops
                        ; .arch x64
                        ; mov eax, 32 ; cmp ecx, eax ; cmova ecx, eax ; dec ecx
                        ; movsxd r10, r10d ; sar r10, cl ; bt r10, 0 ; setc dl ; sar r10, 1
                    ),
                    // ROR by a multiple of 32 leaves the value; the carry
                    // is bit 31 of the result either way.
                    _ => dynasm!(self.ops ; .arch x64 ; ror r10d, cl ; bt r10d, 31 ; setc dl),
                }
                dynasm!(self.ops ; .arch x64 ; movzx edx, dl ; jmp =>end ; =>zero);
                if carry {
                    self.c_to_edx();
                }
                dynasm!(self.ops ; .arch x64 ; =>end ; mov ecx, r10d);
            }
        }
    }

    /// Runs instruction `w` with the interpreter; the block ends when the
    /// helper says so.
    fn exec(&mut self, w: u32, pc: u32, count: u32) {
        let label = self.ops.new_dynamic_label();
        self.execs.push(ExecStub { label, count, pc });
        dynasm!(self.ops
            ; .arch x64
            ; mov rdi, r12
            ; mov esi, w as i32
            ; mov edx, pc as i32
            ; call QWORD [r12 + EXEC]
            ; mov r15d, DWORD [rbx + CPSR]
            ; test eax, eax
            ; jnz =>label
        );
    }

    /// N and Z of the CPSR from the host's SF and ZF.
    fn nz_from_host(&mut self) {
        dynasm!(self.ops
            ; .arch x64
            ; sets dl
            ; setz r8b
            ; movzx edx, dl
            ; shl edx, 31
            ; movzx r8d, r8b
            ; shl r8d, 30
            ; or edx, r8d
            ; and r15d, !(psr::N | psr::Z) as i32
            ; or r15d, edx
            ; mov DWORD [rbx + CPSR], r15d
        );
    }

    fn c_to_edx(&mut self) {
        dynasm!(self.ops ; .arch x64 ; mov edx, r15d ; shr edx, 29 ; and edx, 1);
    }

    /// CF = the guest's C flag.
    fn c_to_cf(&mut self) {
        dynasm!(self.ops ; .arch x64 ; bt r15d, 29);
    }

    fn instr(&mut self, ins: &Instr, pc: u32, count: u32) {
        match *ins {
            Instr::Alu { op, s, rn, rd, op2, .. } => self.alu(op, s, rn, rd, op2, pc),
            Instr::Mem { load, byte, pre, up, writeback, rn, rd, offset, .. } => {
                let size = if byte { Size::Byte } else { Size::Word };
                self.mem(load, size, pre, up, writeback, rn, rd, offset, pc, count)
            }
            Instr::Half { load, sh, pre, up, writeback, rn, rd, offset, .. } => {
                let size = match sh {
                    1 => Size::Half,
                    2 => Size::SignedByte,
                    _ => Size::SignedHalf,
                };
                self.mem(load, size, pre, up, writeback, rn, rd, offset, pc, count)
            }
            Instr::Multiple { load, pre, up, writeback, rn, list, .. } => {
                self.multiple(load, pre, up, writeback, rn, list, pc, count)
            }
            Instr::Mul { acc, s, rd, rn, rs, rm, .. } => {
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + reg(rm)] ; imul eax, DWORD [rbx + reg(rs)]);
                if acc {
                    dynasm!(self.ops ; .arch x64 ; add eax, DWORD [rbx + reg(rn)]);
                }
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rd)], eax);
                if s {
                    dynasm!(self.ops ; .arch x64 ; test eax, eax);
                    self.nz_from_host();
                }
            }
            Instr::MulLong { signed, acc, s, hi, lo, rs, rm, .. } => {
                // The low 64 bits of a 64-bit multiply are the same signed
                // or not; only the widening differs.
                if signed {
                    dynasm!(self.ops ; .arch x64 ; movsxd rax, DWORD [rbx + reg(rm)] ; movsxd rcx, DWORD [rbx + reg(rs)]);
                } else {
                    dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + reg(rm)] ; mov ecx, DWORD [rbx + reg(rs)]);
                }
                dynasm!(self.ops ; .arch x64 ; imul rax, rcx);
                if acc {
                    dynasm!(self.ops
                        ; .arch x64
                        ; mov edx, DWORD [rbx + reg(hi)]
                        ; shl rdx, 32
                        ; mov ecx, DWORD [rbx + reg(lo)]
                        ; or rdx, rcx
                        ; add rax, rdx
                    );
                }
                // Low word first: with hi == lo the high word wins.
                dynasm!(self.ops
                    ; .arch x64
                    ; mov DWORD [rbx + reg(lo)], eax
                    ; mov rcx, rax
                    ; shr rcx, 32
                    ; mov DWORD [rbx + reg(hi)], ecx
                );
                if s {
                    dynasm!(self.ops ; .arch x64 ; test rax, rax);
                    self.nz_from_host();
                }
            }
            Instr::Clz { rd, rm, .. } => {
                // BSR leaves the destination alone for 0 (all x86-64 CPUs
                // do; the manuals say undefined), so preset it to -1: 31 - -1 = 32.
                self.rn_to_eax(rm, pc);
                dynasm!(self.ops
                    ; .arch x64
                    ; mov ecx, -1
                    ; bsr ecx, eax
                    ; mov eax, 31
                    ; sub eax, ecx
                    ; mov DWORD [rbx + reg(rd)], eax
                );
            }
            Instr::Mrs { rd, .. } => {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rd)], r15d);
            }
            Instr::Exec { w, .. } => self.exec(w, pc, count),
            Instr::Nop => {}
            Instr::Bx { link, rm, .. } => {
                // The target is read before BLX writes lr.
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + reg(rm)]);
                if link {
                    dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(14)], pc.wrapping_add(4) as i32);
                }
                self.interwork();
            }
            Instr::Smul { acc, x, y, rd, rn, rs, rm, .. } => {
                let (hx, hy) = (if x { 2 } else { 0 }, if y { 2 } else { 0 });
                dynasm!(self.ops
                    ; .arch x64
                    ; movsx eax, WORD [rbx + reg(rm) + hx]
                    ; movsx ecx, WORD [rbx + reg(rs) + hy]
                    ; imul eax, ecx
                );
                if acc {
                    // Q on signed overflow of the accumulation.
                    let ok = self.ops.new_dynamic_label();
                    dynasm!(self.ops
                        ; .arch x64
                        ; add eax, DWORD [rbx + reg(rn)]
                        ; jno =>ok
                        ; or r15d, psr::Q as i32
                        ; mov DWORD [rbx + CPSR], r15d
                        ; =>ok
                    );
                }
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rd)], eax);
            }
            Instr::Msr { mask, src, cond } => {
                // In User mode only the flags byte is written. A mode change
                // banks registers: the interpreter does those.
                match src {
                    Operand::Imm(v, _) => dynasm!(self.ops ; .arch x64 ; mov ecx, v as i32),
                    _ => self.operand(src, pc, false),
                }
                let (slow, after) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
                dynasm!(self.ops
                    ; .arch x64
                    ; mov eax, r15d
                    ; mov edx, eax
                    ; and edx, 0x1F
                    ; mov r8d, mask as i32
                    ; mov r9d, (mask & 0xFF00_0000) as i32
                    ; cmp edx, ribbit_arm::mode::USR as i32
                    ; cmove r8d, r9d
                    ; and ecx, r8d
                    ; not r8d
                    ; and r8d, eax
                    ; or ecx, r8d
                    ; mov edx, ecx
                    ; xor edx, eax
                    ; test edx, 0x1F
                    ; jnz =>slow
                    ; mov r15d, ecx
                    ; mov DWORD [rbx + CPSR], ecx
                    // I or F cleared: an interrupt may be taken now.
                    ; and edx, eax
                    ; test edx, (psr::I | psr::F) as i32
                    ; jnz =>after
                );
                self.afters.push((ExecStub { label: after, count, pc }, exit::DONE));
                let over = self.ops.new_dynamic_label();
                dynasm!(self.ops ; .arch x64 ; jmp =>over ; =>slow);
                let w = msr_word(cond, mask, src);
                self.exec(w, pc, count);
                dynasm!(self.ops ; .arch x64 ; =>over);
            }
            Instr::Branch { link, target, .. } => {
                if link {
                    dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(14)], pc.wrapping_add(4) as i32);
                }
                let done = self.done;
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + R15], target as i32 ; jmp =>done);
            }
        }
    }

    fn alu(&mut self, op: u32, s: bool, rn: u32, rd: u32, op2: Operand, pc: u32) {
        let logical = matches!(op, 0x0 | 0x1 | 0x8 | 0x9 | 0xC..=0xF);
        self.operand(op2, pc, s && logical);
        if !matches!(op, 0xD | 0xF) {
            self.rn_to_eax(rn, pc);
        }
        // Result in eax; C in dl, V in r8b (arithmetic, when S).
        match op {
            0x0 | 0x8 => dynasm!(self.ops ; .arch x64 ; and eax, ecx),
            0x1 | 0x9 => dynasm!(self.ops ; .arch x64 ; xor eax, ecx),
            0xC => dynasm!(self.ops ; .arch x64 ; or eax, ecx),
            0xD => dynasm!(self.ops ; .arch x64 ; mov eax, ecx),
            0xE => dynasm!(self.ops ; .arch x64 ; not ecx ; and eax, ecx),
            0xF => dynasm!(self.ops ; .arch x64 ; mov eax, ecx ; not eax),
            0x4 | 0xB => dynasm!(self.ops ; .arch x64 ; add eax, ecx ; setc dl ; seto r8b),
            0x5 => {
                self.c_to_cf();
                dynasm!(self.ops ; .arch x64 ; adc eax, ecx ; setc dl ; seto r8b);
            }
            0x2 | 0xA => dynasm!(self.ops ; .arch x64 ; sub eax, ecx ; setnc dl ; seto r8b),
            0x3 => dynasm!(self.ops ; .arch x64 ; sub ecx, eax ; setnc dl ; seto r8b ; mov eax, ecx),
            0x6 => {
                // n + !op2 + C = n - op2 - !C; ARM's carry is not-borrow.
                self.c_to_cf();
                dynasm!(self.ops ; .arch x64 ; cmc ; sbb eax, ecx ; setnc dl ; seto r8b);
            }
            _ => {
                self.c_to_cf();
                dynasm!(self.ops ; .arch x64 ; cmc ; sbb ecx, eax ; setnc dl ; seto r8b ; mov eax, ecx);
            }
        }
        let write = !matches!(op, 0x8..=0xB);
        if s {
            let keep = if logical { !(psr::N | psr::Z | psr::C) } else { !(psr::N | psr::Z | psr::C | psr::V) };
            dynasm!(self.ops
                ; .arch x64
                ; and r15d, keep as i32
                ; movzx edx, dl
                ; shl edx, 29
                ; or r15d, edx
            );
            if !logical {
                dynasm!(self.ops ; .arch x64 ; movzx r8d, r8b ; shl r8d, 28 ; or r15d, r8d);
            }
            dynasm!(self.ops
                ; .arch x64
                ; mov edx, eax
                ; and edx, psr::N as i32
                ; or r15d, edx
                ; test eax, eax
                ; setz dl
                ; movzx edx, dl
                ; shl edx, 30
                ; or r15d, edx
                ; mov DWORD [rbx + CPSR], r15d
            );
        }
        if write {
            if rd == 15 {
                let done = self.done;
                dynasm!(self.ops ; .arch x64 ; and eax, !3 ; mov DWORD [rbx + R15], eax ; jmp =>done);
            } else {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rd)], eax);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn mem(&mut self, load: bool, size: Size, pre: bool, up: bool, writeback: bool, rn: u32, rd: u32, offset: Operand, pc: u32, count: u32) {
        // Offset in ecx, base in eax, the moved base in r13d.
        self.operand(offset, pc, false);
        self.rn_to_eax(rn, pc);
        dynasm!(self.ops ; .arch x64 ; mov r13d, eax);
        if up {
            dynasm!(self.ops ; .arch x64 ; add r13d, ecx);
        } else {
            dynasm!(self.ops ; .arch x64 ; sub r13d, ecx);
        }
        if pre {
            dynasm!(self.ops ; .arch x64 ; mov esi, r13d);
        } else {
            dynasm!(self.ops ; .arch x64 ; mov esi, eax);
        }
        let back = if !pre || writeback { Some(rn) } else { None };
        let fault = self.ops.new_dynamic_label();
        self.faults.push(FaultStub { label: fault, count });
        if load {
            self.access(true, size, pc, fault, None);
            if let Some(rn) = back {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rn)], r13d);
            }
            if rd == 15 {
                self.interwork();
            } else {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rd)], eax);
            }
        } else {
            if rd == 15 {
                // ARM926 stores the instruction's address plus 12.
                dynasm!(self.ops ; .arch x64 ; mov edx, pc.wrapping_add(12) as i32);
            } else {
                dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + reg(rd)]);
            }
            let label = self.ops.new_dynamic_label();
            self.stores.push(StoreStub { label, fault, count, pc, writeback: back });
            self.access(false, size, pc, fault, Some(label));
            if let Some(rn) = back {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rn)], r13d);
            }
        }
    }

    /// Branches to eax as a load into r15 does (ARMv5: bit 0 selects
    /// Thumb), ending the block.
    fn interwork(&mut self) {
        let (arm, set) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let done = self.done;
        dynasm!(self.ops
            ; .arch x64
            ; test al, 1
            ; jz =>arm
            ; or r15d, psr::T as i32
            ; mov DWORD [rbx + CPSR], r15d
            ; and eax, !1
            ; jmp =>set
            ; =>arm
            ; and eax, !3
            ; =>set
            ; mov DWORD [rbx + R15], eax
            ; jmp =>done
        );
    }

    /// A load into eax, or a store of edx, at esi: straight to RAM when the
    /// page is in the fast table, else through the helper. A helper that
    /// took an abort goes to `fault`. A store helper's other results go to
    /// `stub` (which tells the two apart), or without one are or-ed into
    /// r14d (`exit::CODE_WRITTEN` noted for a block transfer).
    fn access(&mut self, load: bool, size: Size, pc: u32, fault: DynamicLabel, stub: Option<DynamicLabel>) {
        // The page's entry in the table (eax its index, rcx the table); a
        // misaligned address never matches a tag.
        let mask = !0xFFFu32
            | match size {
                Size::Word => 3,
                Size::Half | Size::SignedHalf => 1,
                _ => 0,
            };
        let (slow, over) = (self.ops.new_dynamic_label(), self.ops.new_dynamic_label());
        let table = if load { READS } else { WRITES };
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, esi
            ; shr eax, 12
            ; and eax, (FAST - 1) as i32
            ; mov rcx, QWORD [r12 + table]
            ; mov r8d, esi
            ; and r8d, mask as i32
            ; cmp r8d, DWORD [rcx + rax * 8]
            ; jne =>slow
            ; mov r8d, DWORD [rcx + rax * 8 + 4]
            ; add r8d, esi
            ; add r8, QWORD [r12 + RAM]
        );
        if load {
            match size {
                Size::Word => dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [r8]),
                Size::Byte | Size::SignedByte => dynasm!(self.ops ; .arch x64 ; movzx eax, BYTE [r8]),
                _ => dynasm!(self.ops ; .arch x64 ; movzx eax, WORD [r8]),
            }
            let f = match size {
                Size::Word => LOAD32,
                Size::Byte | Size::SignedByte => LOAD8,
                Size::Half | Size::SignedHalf => LOAD16,
            };
            dynasm!(self.ops
                ; .arch x64
                ; jmp =>over
                ; =>slow
                ; mov rdi, r12
                ; mov edx, pc as i32
                ; call QWORD [r12 + f]
                ; bt rax, 32
                ; jc =>fault
                ; =>over
            );
            match size {
                Size::SignedByte => dynasm!(self.ops ; .arch x64 ; movsx eax, al),
                Size::SignedHalf => dynasm!(self.ops ; .arch x64 ; movsx eax, ax),
                _ => {}
            }
        } else {
            match size {
                Size::Word => dynasm!(self.ops ; .arch x64 ; mov DWORD [r8], edx),
                Size::Byte => dynasm!(self.ops ; .arch x64 ; mov BYTE [r8], dl),
                _ => dynasm!(self.ops ; .arch x64 ; mov WORD [r8], dx),
            }
            let f = match size {
                Size::Byte => STORE8,
                Size::Half => STORE16,
                _ => STORE32,
            };
            dynasm!(self.ops
                ; .arch x64
                ; jmp =>over
                ; =>slow
                ; mov rdi, r12
                ; mov ecx, pc as i32
                ; call QWORD [r12 + f]
            );
            match stub {
                Some(stub) => dynasm!(self.ops ; .arch x64 ; test eax, eax ; jnz =>stub),
                None => dynasm!(self.ops ; .arch x64 ; cmp eax, exit::FAULT as i32 ; je =>fault ; or r14d, eax),
            }
            dynasm!(self.ops ; .arch x64 ; =>over);
        }
    }

    /// LDM/STM without the S bit, `list` not empty, the base not r15.
    #[allow(clippy::too_many_arguments)]
    fn multiple(&mut self, load: bool, pre: bool, up: bool, writeback: bool, rn: u32, list: u32, pc: u32, count: u32) {
        let n = list.count_ones() as i32;
        // The lowest address from the base, and the written-back base's move.
        let (start, delta) = match (pre, up) {
            (false, true) => (0, 4 * n),
            (true, true) => (4, 4 * n),
            (false, false) => (4 - 4 * n, -4 * n),
            (true, false) => (-4 * n, -4 * n),
        };
        let fault = self.ops.new_dynamic_label();
        self.faults.push(FaultStub { label: fault, count });
        // The base stays in r[rn] until the end (loads go to the stack
        // first: an abort leaves every register as it was).
        dynasm!(self.ops ; .arch x64 ; mov r13d, DWORD [rbx + reg(rn)] ; add r13d, start);
        if !load {
            dynasm!(self.ops ; .arch x64 ; xor r14d, r14d);
        }
        let first = list.trailing_zeros();
        for (k, r) in (0..16).filter(|r| list & (1 << r) != 0).enumerate() {
            if !load {
                if r == 15 {
                    dynasm!(self.ops ; .arch x64 ; mov edx, pc.wrapping_add(12) as i32);
                } else if r == rn && writeback && r != first {
                    // The base, when not first in the list, is stored written back.
                    dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + reg(rn)] ; add edx, delta);
                } else {
                    dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + reg(r)]);
                }
            }
            dynasm!(self.ops ; .arch x64 ; mov esi, r13d);
            self.access(load, Size::Word, pc, fault, None);
            if load {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rsp + 4 * k as i32], eax);
            }
            dynasm!(self.ops ; .arch x64 ; add r13d, 4);
        }
        if writeback {
            dynasm!(self.ops ; .arch x64 ; add DWORD [rbx + reg(rn)], delta);
        }
        if load {
            let mut pc_slot = None;
            for (k, r) in (0..16).filter(|r| list & (1 << r) != 0).enumerate() {
                if r == 15 {
                    pc_slot = Some(k);
                } else {
                    dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rsp + 4 * k as i32] ; mov DWORD [rbx + reg(r)], eax);
                }
            }
            if let Some(k) = pc_slot {
                dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rsp + 4 * k as i32]);
                self.interwork();
            }
        } else {
            let label = self.ops.new_dynamic_label();
            self.afters.push((ExecStub { label, count, pc }, exit::CODE_WRITTEN));
            dynasm!(self.ops ; .arch x64 ; test r14d, r14d ; jnz =>label);
        }
    }
}

/// The MSR instruction word an `Instr::Msr` came from (for the interpreter).
fn msr_word(cond: u32, mask: u32, src: Operand) -> u32 {
    let fields = (0..4).filter(|i| mask & (0xFF << (8 * i)) != 0).fold(0, |f, i| f | 1 << (16 + i));
    let base = cond << 28 | fields;
    match src {
        Operand::Imm(v, _) => {
            // Any rotation that gives v back.
            let rot = (0..16).find(|r| v.rotate_left(2 * r) <= 0xFF).unwrap_or(0);
            base | 0x0320_F000 | rot << 8 | v.rotate_left(2 * rot)
        }
        Operand::Reg { rm, .. } | Operand::RegShift { rm, .. } => base | 0x0120_F000 | rm,
    }
}
