# ARCompact JIT: design

Goal: run Leapster code (original Leapster, Leapster TV, L-MAX, Leapster 2: the same ARCtangent-A5 core in
`ribbit-cpu`) 3-5x faster than the interpreter, so the worst scene (SpongeBob: Through the Wormhole's conveyor
belts, ~125% of real time on one i9-13900K P-core) holds 100% on a 15 W i3-1315U laptop, and later on phones.

Hard requirement: **bit-identical to the interpreter**, cycles included. Every speed-up so far was accepted only
with an unchanged state digest; the JIT is held to the same rule. Status: DESIGN (2026-10-18), nothing built yet.
It builds on the block cache being added to `run_block` (straight-line decoded blocks with per-block checks and
precomputed instruction-fetch line crossings).

Confidence labels: **FIXED** = a requirement from the existing code; **PLAN** = chosen design; **ESTIMATE**.

## 1. What the JIT must reproduce (FIXED)

Per instruction, `Cpu::run_block` (crates/ribbit-cpu/src/exec.rs) does:

1. `icache.hot(pc)`: the pre-digested `Hot` (fast form, `next`, `target`, `delay`, `stop`, `recheck`, `flag_reg`,
   `branchy`, `reads`, `load_dst`, `compare_branch`). Outside read-only ranges, decode now.
2. Stop before `hot.stop` (LR/SR, FLAG, SLEEP, BRK, SWI, J.F, illegal, HD capture stops from `ICache::set_stops`),
   a branch in a delay slot, a FLAG whose register has H set, or `pc >= fence`.
3. `bus.instruction(pc, len)` → `MemTiming::instruction`: code-cache lookup per 16-byte line (`last_code`
   shortcut), fill cost on a miss.
4. Execute (`execute_fast` or `execute`); memory instructions first call `bus.at(cycle, pc)`; each RAM/ROM access
   calls `MemTiming::data` (data cache, write-back), each device access `MemTiming::device` and, unless
   time-independent, syncs peripherals and sets `io.sync_needed`.
5. `bus.take_stall()` + `pipeline_cost(now, reads, load_dst, taken, delay, compare_branch, stall)`: register
   scoreboard `ready[64]`/`ready_max`, load latency 3, +1 for a killed delay slot, +1 for a taken BRcc/BBIT.
6. Next PC: delay slot target (`delay_target`), jump, or `loop_end_check` (LP_END / LP_COUNT / LP_START).
7. Exits: `bus.block_exit()` (a device access happened), `recheck` with an interrupt now pending, a backward jump
   the idle detector's `watch(target, cycles)` accepts. `run_batch` (ribbit-core/src/lib.rs) has already capped the
   budget at the next device event, voice sample, processor-timer event and step limit, so **interrupts are only
   taken between batches**; one `tick(spent)` settles the timers at the end.

The JIT replaces steps 1-6 inside a batch. `run_batch`, `step`, interrupts, idle skipping, save states and every
device stay as they are.

## 2. Architecture (PLAN)

* **Compile unit = a block of the block cache**: straight-line code from a start PC up to and including the first
  branch/jump (plus its delay slot) or the first `stop` instruction (not included), at most ~64 instructions.
  Only blocks wholly inside `ICache` read-only ranges (BIOS, cartridge) are compiled. RAM code keeps the
  interpreter (rare on the Leapster: BIOS and games run from ROM; to be measured with `--coverage`).
* **Tiering**: interpret a block until it has run N times (N≈8), then compile. Cold code never pays compile time.
* **Entry/exit**: `run_block` looks up `jit.block(pc)`; if compiled, it calls
  `extern "C" fn(ctx: *mut JitCtx) -> ExitCode` and loops. A compiled block returns when it ends, on budget, on
  a block exit, or before an instruction it cannot run (**per-instruction fallback**: the block returns with
  `pc` at that instruction and `run_block` runs one interpreted step, then re-enters the JIT at the next PC).
* **`JitCtx`** (repr(C)): pointers to `Cpu` regs, `status32`, `ready`, `ready_max`, `lp_start/lp_end`,
  `delay_target`, the cycle counter `spent`, the `budget`, RAM/ROM base pointers and sizes, a pointer to the
  `MachineBus`, and the exit reason. All guest state lives in `Cpu` as today, so save states, the debugger and the
  interpreter see one state; **the JIT keeps nothing that a save state would need** (code is rebuilt on demand).

## 3. Code emission (PLAN)

| Option | Compile speed | ARM64 | Fit |
|---|---|---|---|
| **dynasm-rs** | Very fast (templates assembled at build time, patched at run time) | Yes (aarch64 backend) | Direct control of every instruction; deterministic output; small dependency |
| iced-x86 (code assembler) | Fast | **No** | x86 only: would need a second emitter for ARM64 |
| cranelift-jit | Slow-ish per block (an optimizing compiler), big dependency | Yes | Good code, but compile pauses on block-heavy games and harder control of exits |

**Chosen: dynasm-rs** (x86-64 now, aarch64 later), with a thin per-architecture emitter behind one trait
(`Emitter::alu`, `load`, `store`, `call_helper`, `exit`...). Executable memory via a small W^X allocator
(`memmap2` or direct `VirtualAlloc`/`mmap`): write, then flip to RX; never RWX (Windows/macOS hardened runtime).
macOS ARM64 needs `MAP_JIT` + `pthread_jit_write_protect_np`; Android allows anonymous RX mappings in the app's
own process (ESTIMATE: fine for a sideloaded/Play app; verify on a real device early).

## 4. Registers and flags (PLAN)

* Guest r0-r63 stay in `Cpu::regs` (memory operands `[rbx + 4*n]` with `rbx = &regs`); no host register
  allocation in phase 1. x86-64 handles memory operands cheaply; a later pass may cache the 4-6 most used guest
  registers of a block in host registers.
* **Flags eager, but only when live**: within a block, Z/N/C/V are computed only if a later instruction of the
  block reads them before they are overwritten, or at the block's end (STATUS32 must be exact at every exit,
  since a save state or interrupt may follow). x86 flags map closely (ARC C on SUB is a borrow, as x86 CF).

## 5. Timing (PLAN; the hard part)

* **Static per block**: the base 1 cycle per instruction, the +1/+2 branch costs (known per exit path), and the
  **code-cache line sequence**: the block's fetch lines are known at compile time, so only line *crossings* call
  `MemTiming::instruction` (its `last_code` shortcut makes repeated lines free already; a crossing still needs the
  lookup because a fill elsewhere may have evicted the line). Folded into one constant added per exit.
* **Dynamic, emitted inline**: the scoreboard. For each instruction with `reads != 0`, emit
  `if now < ready_max { wait = max(ready[r] - now) }` only for registers that may still be pending. Within a block
  the compiler knows which registers were loaded by earlier instructions of the same block (exact latency known
  when no stall intervenes); registers from before the block use the runtime check. Loads write `ready[d]`.
* **Memory stalls**: each RAM/ROM access calls (or inlines) the `MemTiming::data` fast path: a compare against
  `last_data[0..2]`; only a miss calls out. Device accesses always call the `MachineBus` helper, which charges
  `cost::DEVICE`, syncs time, and sets `sync_needed` → the block exits right after that instruction (as
  `bus.block_exit()` does today).
* The block accumulates `spent` exactly as `run_block` does and returns before an instruction that would start at
  or beyond `budget` (same rule: the instruction that reaches the budget is the last).

## 6. Memory fast paths (PLAN)

`SystemBus::decode` order (RAM, BIOS, MMIO, VRAM, cartridge, SRAM) becomes, in generated code: a range check on
RAM (most data), then ROM/VRAM, else a helper call that runs the full `MachineBus` path (MMIO, unmapped, cartridge
diagnostics `cart_reads`/`last_cart_read`, write log for idle probes). The alignment rule (`addr & !3`, `& !1`)
is emitted as in `read32`/`read16`. Writes to ROM go to the helper (they are recorded).

## 7. Exits (FIXED behaviour, PLAN mechanism)

| Exit | When | Then |
|---|---|---|
| Budget | `spent` would pass `budget` | `run_batch` events, interrupts |
| Bus | a device access set `sync_needed` | `run_batch` handles it after the instruction |
| Backward | a taken jump with `target <= pc`, no delay pending | call `watch(target, cycles)`; return only if it accepts (idle probe) |
| Recheck | after FLAG/SR (not compiled in phase 1: they are `stop`) | |
| Fallback | an instruction the JIT does not compile | one interpreted step |
| Loop | `next == lp_end` | emitted inline: decrement LP_COUNT, jump to the block at LP_START |

Blocks may be **chained** (a direct jump to another compiled block without returning) only when no exit condition
can trigger at the link; the budget check is kept at each link.

## 8. Invalidation (PLAN)

Compiled code depends on ROM contents, `ICache` stops and ranges. `ICache::clear`, `set_stops` (HD capture on/off)
and `set_rom_ranges` (cartridge swap) also drop the JIT cache; loading a state keeps it (same ROM). Config
changes (`pipeline_timing`, `load_latency`) drop it too.

## 9. Verification (PLAN)

1. **Lockstep differential mode** (`RIBBIT_JIT_CHECK=1`): run each compiled block, then rerun it on a cloned
   `Cpu`/`MemTiming`/RAM with the interpreter, and compare registers, flags, `ready`, cycles, cache state and the
   bytes written. The first mismatch prints the block's disassembly and the generated code.
2. Digest equality on fixed runs: the belt benchmark (`digest belts 9c6fde55d3b9f32a at cycle 25523091575`),
   `examples/bench.rs` on Wormhole (2 billion steps, idle skip on: `63e5fcb972577c07`), scripted scenes for
   Sonic X (Leapster TV timing calibration), Leapster 2 games, and the `user_bios` tests.
3. Unit tests per fast form: random operands, JIT vs `execute_fast`.
4. `--no-jit` / a setting to switch it off, kept for bug reports.

## 10. Phases (ESTIMATE)

| Phase | Content | Expected speed (belts, i9 P-core) |
|---|---|---|
| 0 | Block cache in the interpreter (in progress) | 1.3-2x |
| 1 | x86-64: ALU, MOV, CMP, loads/stores (RAM/ROM fast path), BR/B/BL/J, push/pop, LP; everything else falls back; lockstep checker | 2.5-3.5x |
| 2 | Flags liveness, block chaining, remaining common ops (XY/DSP subset, MUL, shifts, .F forms), host register caching | 3.5-5x |
| 3 | aarch64 emitter (Android, Apple Silicon), MAP_JIT handling | same on ARM |
| 4 | Leapster 2 / L-MAX / TV checks, release behind a setting, then default on | - |

## 11. Risks

* **Timing drift** (pipeline scoreboard across block boundaries, line crossings, a missed `at`/`take_stall`):
  caught by lockstep mode, but every new fast path needs it.
* **Interrupt placement**: must stay at batch boundaries; any chaining that skips a budget check would move
  interrupts and break exactness.
* **Idle skipping**: `watch` must see the same backward jumps with the same cycle counts, or idle skip decisions
  (and digests with skip on) change.
* **Executable memory policies** (macOS hardened runtime, Android vendors): test early on each platform.
* **Debuggability**: crashes in generated code; keep the per-block disassembly dump and the off switch.
* **Maintenance**: two executors to keep in sync; the interpreter stays the reference.
