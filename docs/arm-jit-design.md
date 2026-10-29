# ARM JIT (Didj, later RockIt Twist / Zippity)

Status: phase 2 mostly built (2026-10-29), crate `ribbit-armjit`. Checks:
`src/tests.rs` lockstep (300 random programs, 5000 checked by hand;
interpreter vs JIT, state and memory identical); `didjboot didj 3000` with
`DIDJ_JIT=exact` gives the interpreter's instruction count, PC and PCM
output exactly. Speed (i9, one P-core, first 3000 M cycles, including the
~4 s NAND build at start): interpreter 45 s, phase 1 23 s, now 11.0 s, about
real time for that stretch. `bench_loop` (ribbit-armjit): 695 M
instructions/s, 7.2 ns to enter a block.

Phase 2 so far: everything ARM compiles. Native: data processing (all
shifter forms), LDR/STR word/byte/halfword/signed (and LDR pc), LDM/STM
(not the S forms), MUL/MLA/UMULL/SMULL/..., SMUL<x><y>/SMLA<x><y>, CLZ,
MRS/MSR of the CPSR (a mode change goes to the interpreter), B/BL/BX/BLX,
PLD. The rest (LDM/STM with S, SPSR access, LDRT/STRT, SWP, LDRD/STRD,
coprocessor, SWI) calls `Arm::execute_arm` from inside the block, which
then stops only if r15, the mode, T, I or F changed or compiled code was
written; MCR, SWI and the cond-0xF space end the block. Loads and stores
reach SDRAM directly through two 1024-entry tables (virtual 4 KB page ->
physical, privileged and User mode each), filled by the helpers, emptied
when `Cp15::generation` changes; pages with compiled code are never in the
store table. Not done: Thumb, flags kept in host flags, block linking.

Found on the way: CP15 flushed the TLB on every c1 write; Linux wrote c1
474 K times in the first 3000 M cycles (~60 000 a second), so only M/S/R
changes (and real TTB/DACR changes) flush now (534 K -> 51 K flushes,
interpreter results unchanged).

## Why

The Didj's ARM926EJ-S runs at 393 MHz. Measured on the i9 (release build):

| What | Million instructions a second |
|---|---|
| `ribbit-arm` interpreter, flat RAM, MMU off (`bench_loop`) | 143 |
| `didjboot`, first 3000 M cycles, before 4bbeb5a | 54 |
| the same after 4bbeb5a (run-loop overhead cut) | 70 |
| needed for full speed (when busy) | 393 + machine time |

The interpreter is a plain decoder (`arm::execute`, `thumb::execute`). Even
with no machine around it, it reaches about a third of the speed needed, so
interpreter tuning alone cannot get there. The ARC JIT (`ribbit-jit`,
docs/jit-design.md) is the model.

## Shape

New crate `ribbit-armjit` (x86-64 first; other hosts keep the interpreter,
`SUPPORTED` false as in `ribbit-jit`). It uses `ribbit_jit::exec_mem` (W^X
executable memory) and dynasm 5.1.0, the same as `ribbit-jit`.

* **Blocks.** Straight-line code from one PC to the first branch, write to
  r15, mode/CPSR change, exception-causing instruction, coprocessor op, or
  an instruction the generator does not handle (max 32 instructions).
  Keyed by (physical PC, Thumb bit, privileged), looked up after the MMU
  translates the PC (the same TLB `step` uses). Compiled after a block has
  run N times (start with 2); until then the interpreter runs it.
* **State in place.** Generated code reads and writes `Arm::r`, `cpsr`,
  `cycles`, `instructions` through a pointer (as the ARC JIT does), so save
  states, the debugger and the interpreter always see one state. Banked
  registers stay in Rust: anything that changes mode ends the block.
* **Flags.** NZCV live in `cpsr` between instructions. In a block, a
  flag-setting instruction followed by a conditional one can keep them in
  host flags (phase 2); phase 1 stores them to `cpsr` every time. x86's
  carry after SUB is ARM's borrow inverted (`cmc`).
* **Memory.** Phase 1: every load/store calls a helper
  `extern "sysv64" fn(ctx, va, pc) -> u64` that runs `cp15.translate` and the
  bus, the same path the interpreter takes. A fault: the helper sets
  `current = pc` and takes the data abort itself, and the block returns at
  once (precise state: earlier instructions are already written back).
  Phase 2: an inline fast path for SDRAM through a per-page host-pointer
  table filled from the TLB.
* **Self-modifying code.** A bitmap of physical 1 KB pages that hold compiled
  code; the store helper (and DMA/NAND writes into SDRAM) checks it and
  drops the blocks of a page that is written. Linux loads programs and
  libraries this way, so this is needed from day one. Also drop everything on
  a CP15 TLB/cache flush that changes the translation (`ttb`, `dacr`,
  `control` writes) and on FCSE PID changes.
* **Interrupts and time.** Checked between blocks only. `Didj::run` keeps
  its slices; a block adds its instruction count to `cycles` and
  `instructions`. The interrupt delay is at most 32 instructions (it is up to
  the slice length today).
* **Exits.** A block returns the reason: done (next PC in r15), fault taken,
  or fallback (the interpreter runs the next instruction with `step`).

## Phases

1. Crate, block cache, the run loop calling it (`Didj::run` gets a
   `Runner` like `ribbit-cpu`'s `BlockRunner`), ARM data processing (all
   shifter forms with immediate shifts), LDR/STR word/byte (immediate and
   register offsets), B/BL; everything else falls back. Lockstep check: in
   debug, a mode that runs each block on a copy with the interpreter and
   compares `r`, `cpsr`, memory writes.
2. LDM/STM, halfword/signed loads, MUL/MLA/UMULL/SMULL, CLZ, register
   shifts, Thumb, flags kept in host flags, inline SDRAM fast path, block
   linking (jump straight to the next block when it is compiled).
3. AArch64 backend (macOS on Apple silicon; MAP_JIT) if the Didj ships on
   macOS.

## Checks

* `bench_loop` (ribbit-arm) gets a JIT twin.
* `didjboot` must give the same instruction count, PC, and PCM output with
  the JIT on and off (`DIDJ_JIT=0`), as the interpreter changes did in
  4bbeb5a.
* Lockstep over the first 50000 M cycles (to the country menu).
