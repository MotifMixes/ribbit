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
//!
//! All three are callee-saved in the System V ABI, so they survive the
//! helpers; everything else is scratch. Each instruction computes exactly
//! what `arm::execute` computes; the NZCV flags are stored to the CPSR after
//! every flag-setting instruction (phase 1).

use crate::decode::{Instr, Operand};
use crate::{Ctx, exit};
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
const STORE32: i32 = offset_of!(Ctx, store32) as i32;
const STORE8: i32 = offset_of!(Ctx, store8) as i32;

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

struct Gen {
    ops: Asm,
    done: DynamicLabel,
    faults: Vec<FaultStub>,
    stores: Vec<StoreStub>,
}

/// Machine code for `instrs`, the block at `va` (all ARM, one 1 KB page).
/// `fallback`: the block stops before an instruction it does not handle.
pub(crate) fn emit(va: u32, instrs: &[Instr], fallback: bool) -> Option<Vec<u8>> {
    let mut ops = Asm::new(0);
    let (done, ret) = (ops.new_dynamic_label(), ops.new_dynamic_label());
    let mut g = Gen { ops, done, faults: Vec::new(), stores: Vec::new() };
    // Four pushes and 8 bytes after the return address leave rsp 16-byte
    // aligned for the helper calls.
    dynasm!(g.ops
        ; .arch x64
        ; push rbx
        ; push r12
        ; push r13
        ; push r14
        ; sub rsp, 8
        ; mov r12, rdi
        ; mov rbx, QWORD [r12 + CPU]
    );
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
    let code = if fallback { exit::FALLBACK } else { exit::DONE };
    dynasm!(g.ops
        ; .arch x64
        ; mov DWORD [rbx + R15], next as i32
        ; =>done
        ; add QWORD [rbx + CYCLES], n as i32
        ; add QWORD [rbx + INSTRUCTIONS], n as i32
        ; mov DWORD [rbx + CURRENT], last as i32
        ; mov eax, code as i32
        ; =>ret
        ; add rsp, 8
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
    /// Jumps to `skip` unless condition `c` holds.
    fn cond(&mut self, c: u32, skip: DynamicLabel) {
        dynasm!(self.ops
            ; .arch x64
            ; mov eax, DWORD [rbx + CPSR]
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
                        dynasm!(self.ops ; .arch x64 ; mov eax, DWORD [rbx + CPSR] ; bt eax, 29 ; rcr ecx, 1);
                    }
                    _ => dynasm!(self.ops ; .arch x64 ; ror ecx, n),
                }
                // x86 leaves the last bit shifted out in CF, as ARM's shifter.
                if carry {
                    dynasm!(self.ops ; .arch x64 ; setc dl ; movzx edx, dl);
                }
            }
        }
    }

    fn c_to_edx(&mut self) {
        dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + CPSR] ; shr edx, 29 ; and edx, 1);
    }

    /// CF = the guest's C flag.
    fn c_to_cf(&mut self) {
        dynasm!(self.ops ; .arch x64 ; mov r9d, DWORD [rbx + CPSR] ; bt r9d, 29);
    }

    fn instr(&mut self, ins: &Instr, pc: u32, count: u32) {
        match *ins {
            Instr::Alu { op, s, rn, rd, op2, .. } => self.alu(op, s, rn, rd, op2, pc),
            Instr::Mem { load, byte, pre, up, writeback, rn, rd, offset, .. } => {
                self.mem(load, byte, pre, up, writeback, rn, rd, offset, pc, count)
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
                ; mov r9d, DWORD [rbx + CPSR]
                ; and r9d, keep as i32
                ; movzx edx, dl
                ; shl edx, 29
                ; or r9d, edx
            );
            if !logical {
                dynasm!(self.ops ; .arch x64 ; movzx r8d, r8b ; shl r8d, 28 ; or r9d, r8d);
            }
            dynasm!(self.ops
                ; .arch x64
                ; mov edx, eax
                ; and edx, psr::N as i32
                ; or r9d, edx
                ; test eax, eax
                ; setz dl
                ; movzx edx, dl
                ; shl edx, 30
                ; or r9d, edx
                ; mov DWORD [rbx + CPSR], r9d
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
    fn mem(&mut self, load: bool, byte: bool, pre: bool, up: bool, writeback: bool, rn: u32, rd: u32, offset: Operand, pc: u32, count: u32) {
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
            let f = if byte { LOAD8 } else { LOAD32 };
            dynasm!(self.ops
                ; .arch x64
                ; mov rdi, r12
                ; mov edx, pc as i32
                ; call QWORD [r12 + f]
                ; bt rax, 32
                ; jc =>fault
            );
            if let Some(rn) = back {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rn)], r13d);
            }
            dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rd)], eax);
        } else {
            if rd == 15 {
                // ARM926 stores the instruction's address plus 12.
                dynasm!(self.ops ; .arch x64 ; mov edx, pc.wrapping_add(12) as i32);
            } else {
                dynasm!(self.ops ; .arch x64 ; mov edx, DWORD [rbx + reg(rd)]);
            }
            let f = if byte { STORE8 } else { STORE32 };
            let label = self.ops.new_dynamic_label();
            self.stores.push(StoreStub { label, fault, count, pc, writeback: back });
            dynasm!(self.ops
                ; .arch x64
                ; mov rdi, r12
                ; mov ecx, pc as i32
                ; call QWORD [r12 + f]
                ; test eax, eax
                ; jnz =>label
            );
            if let Some(rn) = back {
                dynasm!(self.ops ; .arch x64 ; mov DWORD [rbx + reg(rn)], r13d);
            }
        }
    }
}
