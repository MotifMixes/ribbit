# ARCompact JIT: design

Goal: run Leapster code (original Leapster, Leapster TV, L-MAX, Leapster 2: the same ARCtangent-A5 core in
`ribbit-cpu`) 3-5x faster than the interpreter, so the worst scene (SpongeBob: Through the Wormhole's conveyor
belts, ~125% of real time on one i9-13900K P-core) holds 100% on a 15 W i3-1315U laptop, and later on phones.

Hard requirement: **bit-identical to the interpreter**, cycles included. Every speed-up so far was accepted only
with an unchanged state digest; the JIT is held to the same rule. Status (2026-10-18): phase 1 built and on by
default on x86-64 (`crates/ribbit-jit`; sections 12 and 13), compiling the block cache's blocks
(`icache::Blocks`) with the interpreter's exact timing.

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

## 12. Phase 1 foundation: what is built (2026-10-18)

`crates/ribbit-jit` (x86-64; other targets build a stub, `SUPPORTED = false`):

* `compile(&[Instr]) -> Block`: a straight run of decoded instructions. Every `icache::Fast` form compiles (MOV,
  ALU with and without .F and with conditions, CMP, single-operand ops, LD/ST with write-back, PUSH/POP), ending
  after a branch (BRcc, Bcc, BL, BL.D, J, Jcc) and its delay slot, or with a **fallback exit** before the first
  instruction it cannot compile. The zero-overhead loop check follows every sequential instruction (out-of-line
  stub, `ExitKind::Loop`).
* `Block::run(cpu, bus) -> Exit { kind, pc, executed, delay_target }`: writes `cpu.pc` and `cpu.delay_target`;
  registers, STATUS32, LP_COUNT and memory change in place. Cycles, timers, `ready`/`ready_max` and
  `instructions_retired` are the caller's (from `executed`), until the timing part below.
* `JitCtx` (repr(C)): pointers to `regs` and `status32`, the bus and six memory helpers, LP_START/LP_END in, PC /
  executed / delay slot out.
* Lockstep tests (tests/lockstep.rs) against `Cpu::step`, the general executor: 138 instruction kinds (every ALU
  operation in every form) from random encodings through the real decoder, 103k single instructions, 29k runs
  with branches, delay slots and loop ends, 28k runs of real BIOS code; no differences. Planted bugs are caught.
* examples/jitbench.rs (16-instruction loop body, one P-core, no timing on either side): `Cpu::step` ~46 M
  instructions/s, `run_block` ~145 M/s, JIT ~780 M/s (5.2-5.6x `run_block`). An upper bound: timing will cost.

Deviations from the plan above, and why:

* **Helpers use `extern "sysv64"` on every OS** (also Windows): one calling convention, so the generated code is
  identical on Windows and Linux (rdi/rsi/rdx arguments; rbx, r12-r15 survive calls). A panic inside a helper
  aborts the process instead of unwinding through generated code (which has no unwind tables).
* **dynasm-rs's `VecAssembler` + our own `ExecMemory`** instead of dynasmrt's executable buffer: the code is
  position-independent (relative jumps, calls through `JitCtx`), so it is assembled into a `Vec` and copied into
  pages mapped RW and then flipped to RX (`VirtualAlloc`/`VirtualProtect`/`FlushInstructionCache`,
  `mmap`/`mprotect`). One mapping per block for now; an arena comes with the block cache integration.
* `JitCtx` has no `ready`/`spent`/`budget` yet; they arrive with timing.

Integration points left (after the block engine lands):

1. `Cpu::run_block`: look up a compiled block for `pc` (in the block cache entry), call `Block::run`/`enter`
   with a `JitCtx` whose helpers are the `MachineBus` methods; on `Fallback`, one interpreted instruction.
2. Timing: per instruction the exit index tells which instructions ran; next step is computing `spent`
   in generated code (static costs folded, `ready` scoreboard inline, `MemTiming::instruction` at line
   crossings, `bus.at`/`take_stall` via helpers) and returning before the budget.
3. Exits for `bus.block_exit()` after device accesses (a helper return flag), the backward-jump `watch`, and
   `recheck`.
4. Invalidation with `ICache::clear`/`set_stops`/`set_rom_ranges`; a setting/`--no-jit` and `RIBBIT_JIT_CHECK`.

## 13. Phase 1, part 2: in the machine (2026-10-18)

`Cpu::run_block_with` offers each block of read-only code to a `BlockRunner`; `ribbit_jit::Jit` is one, and
`Machine::jit` holds it (`Jit::from_env`: on where `SUPPORTED`, off with RIBBIT_JIT=0; other architectures build a
stub with the same API, `src/stub.rs`). Built (all VERIFIED by the checks below):

* **Tiering and invalidation.** A block (`BlockHead::id`, per `Blocks::epoch`) is compiled after 32 interpreted
  runs; everything compiled is dropped when the block cache starts over (ROM ranges, HD capture stops, a full
  cache). Nothing is saved in a save state.
* **Timing in the code.** `pipeline_cost` inline (the `ready` scoreboard read and written in place, `ready_max`
  first); the fetch timed in full where `Entry::fetch` says, else nothing (arming and taking a zero stall change
  nothing); `bus.at` before a memory access that goes through the bus; the stall taken after it. Each instruction is
  followed by `run_block`'s checks: the zero-overhead loop end, a device access's exit request, the budget; a
  backward jump calls `watch` (the idle-loop detector) from the code. Interrupts stay at batch boundaries.
* **Inline memory** (`Bus::jit_view`, `ribbit_memory::JitView`): RAM, VRAM and the BIOS mirror read and written in
  place, the data cache's 4-way lookup inline (dirty bit on a write), the code cache's lookup inline for a fetch
  timed in full; a miss, a device, the cartridge and anything else goes through the bus (with `at`). The view is
  MemTiming's own state (tags, `armed`, `stall`) and `sync_needed`, so both paths share one state.
* **Chaining.** Every exit with a known next PC has a link slot (`Link`): the next compiled block runs without
  returning to Rust; computed jumps also look in a shared table (`DISPATCH_SIZE` slots by address).
* **More than the fast forms.** Instructions with no fast form compile from their full decode when their shape is
  `Cpu::execute`'s ALU part, a compare, a bit operation (BSET/BCLR/BXOR/BMSK/BTST), a single-operand operation,
  NORM, MAX, MIN, NORMW, a load/store with any base (jump tables), LPcc, under any condition, with r57-r60, PCL
  and limm operands (`x64::generic_ok`). BBIT0/BBIT1 have fast forms with x86 `BT`+`Jcc` (`Fast::Bbit`/`BbitI`).
  Left to the interpreter: LR/SR/FLAG and the rest of `stop`, BRcc with a constant first operand, the DSP/XY
  instructions.
* **Code memory.** Mapped twice where the system allows (Windows: a section, two views; Linux: a memfd, two
  mappings), so no page is ever writable and executable and a compile needs no system call; elsewhere the pages
  written are flipped RW and back to RX.
* **Check mode** (`RIBBIT_JIT_CHECK=1`): blocks compile with every memory and timing step as a bus call; each
  compiled run is recorded, then replayed in the interpreter (`Cpu::check_one`) from the same state; any difference
  in a register, flag, the scoreboard, LP_START/LP_END, the cycle count, or a bus access or its cycle stamp panics.

Verification (2026-10-18): belt digest `9c6fde55d3b9f32a` at cycle 25523091575 (Windows and Linux, JIT on, and
checked: 190 M compiled runs replayed); `examples/bench.rs` on Wormhole, 2 billion steps, digest `63e5fcb972577c07`
with identical idle-skip counts; five digests each over scripted play of Sonic X and Pet Pals on the US v2.1 and
Leapster 2 0.6 BIOSes, Sonic X on the Leapster TV and Wormhole on the L-MAX v2.2: identical JIT off, on and
checked; the `user_bios` tests with the user's BIOSes; the lockstep tests (now with cycles and the scoreboard).

Speed (belt benchmark, one i9-13900K P-core, interleaved runs, same session): interpreter (block engine) average
203-208%, worst 250 ms window 181-195%; JIT average 283-286%, worst 235-243% (about 1.38x and 1.27x). Linux (WSL):
198% / 263% average. Compiling 7.6k blocks takes ~60 ms of code generation and ~18 ms of first-touch page faults.

Phase 2 (2026-10-21): MAX/MIN, NORMW added to `generic_ok`; BBIT0/BBIT1 fast forms (`Fast::Bbit`/`BbitI`) using
x86 `BT`+`Jcc`. Verification: all 4 lockstep tests pass; `RIBBIT_JIT_CHECK=1` on 50 M BIOS steps — no differences.
`examples/jitbench.rs` (16-instruction loop, one P-core): `Cpu::step` 49 M inst/s, `run_block` 199 M inst/s,
JIT 290 M inst/s (1.5× `run_block`). Belt benchmark needs the belt save state (`RIBBIT_STATE`); to be re-measured.

Left: generated-code quality (flags computed only when read, registers cached in host registers, shared exit
stubs: blocks average ~4.8 KB of code), BRcc with constant first operand, DSP/XY instructions, aarch64 (phase 3).

## 14. Generated-code cost (2026-10-22)

Measured first: a player's i3-1315U laptop lagged in Animal Genius's globe menu on 1.3.0 (the game never idles
there). On one i9 P-core that scene ran at worst 186-192% with the JIT and 155-165% without, so the JIT bought
only ~1.2x. `RIBBIT_JIT_PERFMAP=1` (Linux; research) maps code anonymously, names each block in
`/tmp/perf-PID.map`, and writes which kind of code starts where (`x64::tag`) and the code bytes, so a `perf`
profile can be summed by kind and by instruction. Only ~5% of the time was the guest's own operations; the rest
was timing bookkeeping: region search + data-cache ways 24%, scoreboard 12%, arming/taking the stall 16%.

Changes, all exact (same digests JIT on/off; check mode and lockstep tests pass):

* **Stall on slow paths only.** A cache hit changes no `MemTiming` state, so inline code no longer arms the
  timing before a memory instruction nor takes the stall after it. Each slow path does: a code-cache miss
  (`fetch_take`), and the data access's `timed_read*`/`timed_write*` helpers (`instruction_same_line` to arm,
  `at`, the access, `take_stall`, `block_exit` in bit 63), adding into a stack slot (`[rsp]`) that the
  instruction's end reads and clears. The exit request can only come from such a helper: a batch starts with
  `sync_needed` clear and only bus accesses set it (`watch` does not).
* **Static scoreboard.** A load at instruction `j` arrives at most `latency - 1` cycles after its own end and every
  instruction takes at least one cycle, so instruction `k` can wait for it only if `k - j < latency`. The first
  `latency - 1` instructions of a block check every register they read (loads from before the block); later
  ones only the registers loaded by the block's last `latency - 1` instructions: usually no code at all.
  `load_latency` is compiled in (the code is dropped if it changes).
* **Fewer pointer loads.** STATUS32 and `ready_max` are read at fixed offsets from `rbx` (`&Cpu::regs`); `r13`
  holds `&ready[0]`.
* **Data-cache ways at once** (SSE2: broadcast, OR the dirty bit on both sides, `pcmpeqd`, `movmskps`); the dirty
  bit is stored only when not yet set (a narrow store stalls the next 16-byte load of the set).
* **Compiled-in settings.** Memory timing on/off and whether links check the fence (it is `u32::MAX` once the first
  cartridge instruction ran); the code is dropped when either changes the wrong way.
* **Patched links.** A constant-target exit has a 5-byte `jmp` that falls through to its link slot until the
  `Jit` points it at the next block (`JitCtx::patch`, `Arena::patch` through the write view).
* Cycle updates skip the wait or the stall where they are statically zero.

Speed (one i9-13900K P-core at high priority; another process loaded the machine): Animal Genius globe, worst
250 ms window / average: before 186-192% / 239-254%; after ~415-426% / 448-463% (interpreter alone ~200% /
232%). Wormhole `bench.rs`, 2 billion steps with batches (`RIBBIT_NO_TAIL=1`): 2.19x -> 3.72x real time (interpreter
1.74x), digest `63e5fcb972577c07` unchanged. Digests also unchanged JIT on/off over scripted play of Sonic X, Pet
Pals, Ratatouille and Cars 2 on the US v2.1 BIOS, Sonic X and Pet Pals on Leapster 2 0.6, Wormhole on L-MAX 2.2.
The aarch64 emitter is unchanged (it keeps the old helpers and context fields).

## 15. AArch64 (phase 3, 2026-10-22)

`aarch64.rs` is a translation of `x64.rs` function for function (the earlier agent-written AArch64 file failed the
lockstep tests: flags overwrote results, post-increment addressing was unfinished). Same design and the section-14
speed-ups; the register map is in its header. AArch64 notes: the four data-cache ways are compared with a `ccmp`
chain (no NEON needed); NZCV.C is loaded with `cmp w, #1` (C set exactly for a 1) for ADC/SBC (SBC takes the
inverted carry); subtraction C is AArch64's inverted; patched links are a `b` (±128 MB, else the link slot);
LP_END and the budget are cached in x27/x28 (an LP writes both copies); x18 is never used (the platform register on
Apple systems and Android). Android has no memfd dual mapping here (`target_os = "android"`): pages are flipped.

Verification without an ARM machine: `aarch64-unknown-linux-musl` built with `rust-lld` and run under user-mode QEMU
in WSL (the `qemu-user-static` package unpacked, no root). The lockstep tests pass (tests/lockstep.rs now runs on
both architectures); digests JIT on vs off are identical on the Animal Genius globe state, Sonic X (US v2.1), Pet
Pals (Leapster 2 0.6) and Wormhole (L-MAX 2.2); check mode replayed ~105 M compiled runs with no difference. Not
measured: speed on a real phone or Apple silicon Mac (the x86-64 Android emulator translates ARM code and runs it at
12-61% of real time, so it says nothing about speed). Exec memory on macOS is RW then RX by `mprotect` (the app is
ad-hoc signed without the hardened runtime); if a mapping is refused, blocks stay interpreted.
