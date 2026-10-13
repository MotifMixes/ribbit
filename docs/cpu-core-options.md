# CPU core: reuse vs. implement

Target: an ARCtangent-A5 / ARCompact core with the Leapster's XY/DSP extension subset, the Leapster-specific aux
registers, misaligned access that ignores the low bits, two-level interrupts, and zero-overhead loops.
See hardware-research.md §CPU.

## Candidates

| Option | License | Completeness for Leapster | Integration cost | Mobile | Verdict |
|---|---|---|---|---|---|
| **A. MAME `arcompact` (C++) wrapped through FFI** | BSD-3-Clause (per-file) | Best available. Actively developed (latest fixes 2026-10-07). Still a `NOT_WORKING` driver; one cycle per instruction | Very high: depends on MAME's `device_t`, `address_space`, scheduler, debugger and logging (`emu.h`). It would need a C++ toolchain on every platform plus a hand-written shim for the whole MAME device API | C++ in an Android NDK/iOS build is possible, but painful | Rejected as a dependency |
| **B. Port MAME `arcompact` to Rust** (derived work, attribution kept) | BSD-3-Clause allows this with the notice retained | Same as A, including its known gaps and guesses | Medium: about 25 source files. Mechanical decoding, careful flag semantics | Pure Rust | **Use as reference / oracle** |
| **C. Fresh Rust implementation from the ARCompact PRM**, cross-checked against B and binutils | Ours | Starts at zero, but every behaviour can cite the spec | Medium-high | Pure Rust | **Selected (combined with B)** |
| D. Punkster `ARCA5.c` | **AGPL-3.0** | Stub (ISA marked TODO) | Would force AGPL on Ribbit | — | Rejected |
| E. QEMU | GPL-2.0 | No ARCompact target found. Synopsys upstreamed ARCv2/v3 work, a different ISA line | — | — | Not applicable |
| F. Synopsys nSIM | Proprietary (free edition exists) | Accurate ARC600-class simulator | Cannot ship it. Licensing forbids embedding | — | **Use offline only** as a reference oracle if the license permits, never in the product |
| G. Unicorn | GPL-2.0 | No ARC support | — | — | Not applicable |
| H. GNU binutils `opcodes/arc-*` | GPL-3.0 | Authoritative encodings and disassembler | Do not copy code. Use the *tool* (`arc-elf32-objdump`, `as`) to generate and check test vectors | — | **Use as a test oracle** |

## Decision

Implement the CPU in Rust in `ribbit-cpu` (option C), using:

1. **The ARC 600 / ARCompact Programmer's Reference** as the normative spec. It must be obtained (open-questions Q1).
2. **MAME's BSD-3-Clause core as a behavioural reference.** Where Ribbit adopts MAME-specific behaviour (Leapster
   aux registers, XY subset, alignment), the code cites it. Any file that substantially translates MAME code carries
   MAME's BSD-3-Clause notice and copyright holders (David Haywood, Alice Shelton, Peter Wilhelmsen, and others).
3. **binutils (GNU assembler/objdump for `arc-elf32`) as an encoding oracle.** Assemble instruction sequences,
   compare our decoder and disassembler against `objdump`, and keep the generated vectors as test fixtures.
4. **Differential traces against MAME.** Run MAME with `-debug` and `trace` on the user's BIOS, then compare PC and
   register traces with Ribbit's, instruction by instruction, for the first N million instructions. Divergence
   localises bugs in either emulator. MAME is not presumed correct.

Why not simply wrap MAME: Ribbit's core must stay independent of the frontend, build cleanly for Android and iOS,
support save states, and be inspectable by `ribbit-debug`. MAME's CPU cores are welded to MAME's device framework.
A port gives the same knowledge without that coupling. The cost (about 25 files of decode/execute logic) is moderate,
and it is the core of the project anyway.

## Implementation order (Phase 2)

1. Full decoder for all formats, including LIMM detection, with a disassembler. Test against binutils vectors.
2. ALU ops and flags (`ADD/SUB/AND/OR/XOR/BIC/MOV/CMP/...`, shifts), condition codes, `.F` flag updates.
3. Loads/stores (all widths, sign extension, `.AB/.AW/.AS` address writeback, misalignment rule).
4. Branches and jumps with delay slots (`.D`), `BL`/`JL`, `LP`/`LPcc` loops.
5. Aux registers (`LR`/`SR`), STATUS32, `FLAG`, `SLEEP`, `BRK`, `RTIE`.
6. Interrupts and exceptions (levels, ILINK, priority, delay-slot inhibit).
7. Multiply (`MUL64`/`MULU64`, MLO/MMID/MHI, MULHI aux) and the XY/DSP subset.
8. Cycle model: start with one cycle per instruction like MAME, then add measured costs when hardware data exists
   (open-questions Q5).
