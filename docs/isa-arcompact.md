# ARCompact ISA: evidence, assumptions and status (ribbit-cpu)

## Processor variant

| Claim | Status | Evidence |
|---|---|---|
| The original Leapster CPU is an **ARCtangent-A5** ("ARCTangent 5.1"), ARCompact ISA | SOURCED (MAME, eLinux) | No LeapFrog documentation. The A5 was the first ARCompact core; ARC 600/700 followed |
| ARCompact, *not* ARCv2 (EM/HS) | VERIFIED | GNU objdump in ARC600 mode decodes the BIOS consistently (764,950 lines, 0 length disagreements with Ribbit). ARCv2 16-bit encodings differ |
| Little-endian, 32-bit instructions and LIMMs stored high halfword first | VERIFIED | BIOS vector table: `2020 0f80 4002 b5f4` = `j 0x4002b5f4` |
| Vector table at 0x4000_0000, 8 bytes per vector, 32 vectors | VERIFIED | BIOS v1.5 disassembly: 32 `j limm` entries; reset → 0x4002_B5F4 |
| Reset code puts the stack in VRAM (`mov sp,0x3000800`) | VERIFIED | Reset handler, first instruction (confirms MAME's comment) |
| Extended-arithmetic, MUL64, NORM, SWAP options present | HYPOTHESIS (MAME enables them; the BIOS contains MUL64_S and NORM) | Option presence is a build-time choice; BCRs/IDENTITY not yet read from hardware |
| XY memory extension present (core registers r32-r55, aux 0x80-0x9F) | VERIFIED-FW | The v1.5 MQX context switch saves r32-r59 (0x4007_3CA0) and restores them (0x4007_3D74). See "XY memory extension" |
| DSP (dual 16-bit MAC) instructions present | SOURCED (MAME) | No specification found; MULDW/MACDW/MSUBDW follow MAME's `dsp_multiply` (UNVERIFIED), see below |
| IDENTITY register value | UNKNOWN | Ribbit returns 0x0000_0010 (ARCVER in the A5 range 0x10-0x1F) |

## Sources and licences

| Source | Use | Licence |
|---|---|---|
| *ARCompact ISA Programmer's Reference*, ARC International, 5115-029 (2008), public copy at me.bios.io | **Normative**: encodings (ch. 6-7), condition codes (ch. 8), semantics (ch. 9), registers (ch. 3), interrupts (ch. 4). Covers A5, ARC 600 and ARC 700 and marks the differences | Proprietary documentation; read for facts, nothing copied verbatim beyond short citations |
| GNU binutils 2.45.1 (ARCompact/ARCv2 toolchain build 10504, `foss-for-mips-arc-processors/toolchain` arc-2026.03 release) | **Oracle**: `arc-elf32-as` builds the test programs, `arc-elf32-objdump -m ARC600` checks the decoder | GPL-3.0 tools, run locally. Their *output* (fixtures) is committed; no binutils code is used |
| MAME `src/devices/cpu/arcompact/*` (2026-10-07) | Leapster-specific behaviour (MMID quirk, DSP opcode numbers, AUX_MACMODE bits), cross-check | BSD-3-Clause. No code copied |
| The user's BIOS dumps | Real-code evidence and the full-BIOS decoder sweep | Never committed |

The A5 is not available as a GNU target (`-mA5` was dropped); `-mcpu=arc600` is used as the closest
profile, and every place where ARC 600 and A5 differ uses the PRM's A5 text.

## Decoder validation

`crates/ribbit-cpu/tests/objdump_oracle.rs`:

| Fixture | Instructions | Result |
|---|---|---|
| `corpus.s` assembled with GNU as (every format we use) | 277 | **277 exact** (mnemonic, operands, length) |
| 64 KiB deterministic random stream | 23,708 | 0 length / 0 text mismatches. 16,528 agree, 343 extension-condition, 6,030 both reject, 793 Ribbit rejects, 14 accepted by documented rule |
| User's v1.5 BIOS, full linear sweep (local only) | 764,950 | 0 length / 0 text mismatches. 608,999 agree, 12,555 extension-condition, 127,916 both reject, 15,157 Ribbit rejects, 323 accepted by documented rule (65 of them MUL64 A field, 2026-10-13) |
| User's US v2.1 BIOS, full linear sweep (local only, 2026-10-13) | 3,162,386 | 0 length / 0 text mismatches. 2,560,475 agree, 60,545 extension-condition, 469,829 both reject, 69,983 Ribbit rejects, 1,554 accepted by documented rule (416 MUL64 A field, 3 SWI/BRK operand field) |

A linear sweep also decodes data, so most rejections are of non-code bytes.

### Documented disagreements with binutils

Ribbit follows the PRM. The test fails on any disagreement outside these classes.

| Class | binutils | Ribbit / PRM |
|---|---|---|
| Extension condition codes 0x10-0x1F | Prints most without a suffix; some as `nj`/`nm`/`nt` | Decoded; on the A5 an unimplemented extension condition "will always return false" (PRM ch. 8) |
| Reserved A field non-zero in TST/CMP/RCMP/BTST/LR/SR | Rejects | "The processor will ignore reserved bits" (PRM ch. 6) |
| PREFETCH (`ld 0,[b,s9]`) | Rejects for ARC600 | Defined (PRM, PREFETCH); byte, `.x`, `.di` variants are reserved |
| `J.F.D [ILINKn]` | Rejects | PRM: "J.F<.D> [ILINK1]" interrupt return |
| `ST c,[limm,offset]` | Rejects | The address is src2+src3 (PRM, ST); only `[limm]` is in the syntax list. Accepted, flagged |
| `UNIMP_S`, 32-bit `BRK` | Assembler rejects for ARC600 | PRM basecase |
| B far with reserved bit 4 set, LD.X of a long word, write-back with a limm base | Accepts | "will raise an Instruction Error" (PRM) |
| F=0 on CMP/TST/RCMP/BTST; F=1 on FLAG/LR/SR/LP/MUL64; MUL64 with A≠62; JLcc.F; Jcc.D/BRcc.D with limm | Rejects (agrees) | Not in the PRM syntax; decoded as illegal |
| Optional ARC600 DSP/XMAC instructions (`maclw`, `mulflw`, `crc`, `dmacpf`, ...) | Decodes | No specification in hand. Decoded as *extension*; execution stops with a diagnostic |
| MPY/MPYH/MPYHU/MPYU, EX, SYNC, RTIE, TRAP_S | Decodes | ARC 700 only; an Instruction Error on the A5 |

## Execution semantics: decisions and hypotheses

| Topic | Choice | Basis |
|---|---|---|
| Interrupt model | Two levels; level 2 = ILINK2/STATUS32_L2, masks E1+E2; level 1 = ILINK1/STATUS32_L1, masks E1; ILINK = next PC | PRM Figure 83 |
| Priority | Within a level the higher IRQ number wins; IRQ7 always first in its level. Default level 2 = IRQ6, IRQ7 | PRM "Interrupt Priority", Tables 23/24 |
| Interrupts in a delay slot | Held off until the delay-slot instruction completes | PRM: A5 has no BTA ("held off until the instruction in the delay slot commits") |
| Return from interrupt | `J.F [ILINKn]` restores STATUS32 from STATUS32_Ln. RTIE is ARC 700 only | PRM, Jcc |
| SWI | Instruction Error interrupt (vector 2, ILINK2) | PRM, SWI |
| Illegal encodings | Default: **stop with a diagnostic** (`IllegalPolicy::Stop`). Option: take vector 2 as hardware would | Bring-up choice: an illegal instruction is more likely an emulator bug |
| Zero-overhead loop | When NEXT_PC == LP_END: decrement LP_COUNT; jump to LP_START unless LP_COUNT was 1. Not applied after taken branches or delay slots | PRM "ARCtangent-A5 Loop Operation". The branch exclusion is the ARC 600 rule: **HYPOTHESIS** for the A5 |
| STATUS32 | Only H, E1, E2, V, C, N, Z exist; A1/A2/AE/DE/U/L read as 0 | PRM: "The ARCtangent-A5 and ARC 600 processors do not use the A1, A2, AE, DE, U or L fields" |
| Unknown aux registers | Read returns IDENTITY; writes ignored; both logged | PRM "Illegal Auxiliary Register Usage" (A5 basecase) |
| Processor timers 0/1 | Count up to LIMIT, wrap to 0, level IRQ3/IRQ7 if IE, cleared by writing CONTROL; LIMIT resets to 0x00FFFFFF. Watchdog: stops with "not modelled" | PRM "Processor Timers" |
| SLEEP | Sleeps until an *enabled* interrupt is taken; a masked interrupt does not wake | PRM, SLEEP (masked-interrupt case: **HYPOTHESIS**) |
| MUL64/MULU64 A field | Ignored (any value accepted); F = 1 still illegal | PRM, MUL64: "The destination is always ignored for the multiply instruction". Firmware evidence: US v2.1 0x4007_B49C (A = 3, result read from MMID). MAME ignores it. binutils rejects (documented oracle rule) |
| SWI/BRK 32-bit operand field | Ignored | PRM: SWI "takes no operands", "encoded as SWI 0". binutils rejects non-canonical fields (3 words in the v2.1 image's data area) |
| MMID after MUL64 | **Low 32 bits** (`MmidMode::LowLeapster`); the PRM says middle bits | Firmware evidence: BIOS v1.5 at 0x4007_6958 uses MMID as the low word right after `mul64_s`; MAME needs the same quirk to boot. **HYPOTHESIS** pending hardware |
| Writes to MLO/MMID/MHI | Performed and counted | PRM says read-only; the BIOS writes MMID when restoring state (MAME). **HYPOTHESIS** |
| Barrel shift by 0 | C unchanged | PRM silent. MAME clears C for ROR only. **HYPOTHESIS** |
| Misaligned access | Low address bits ignored (bus) | MAME, tested on a Leapster 2 only |
| AUX_MACMODE bits (CS=1, S2=5, S1=8) | From MAME | PRM figure not legible. **HYPOTHESIS** |
| Cycle timing | PRM pipeline costs (taken branch without .d +1, taken BRcc/BBIT +1 more), per-register load latency (3), caches and memory costs (io/memtiming.rs); interrupt entry and sleep 1 cycle | Calibrated to one real-hardware measurement (Sonic X, Leapster TV, within 4%); see hardware-research.md "CPU timing" |

## XY memory extension

Implemented in `crates/ribbit-cpu/src/xy.rs`; enabled by `CpuConfig::leapster()` (`xy_extension = true`).

| Behaviour | Ribbit | Evidence / status |
|---|---|---|
| Register names | r32-r47 = `x0_u0, x0_u1, x1_u0, ... y3_u1`; r48-r55 = `x0_nu ... y3_nu`; aux 0x80-0x87 `ax0-ax3, ay0-ay3`, 0x88-0x97 `mx00 ... my31`, 0x98 `xyconfig` | GNU binutils register tables: names only |
| Port access | Reads/writes the word at the generator's AX/AY address; u0/u1 then add the signed low 16 bits of MXnm / MYnm (post-modify); nu does not modify | MAME `arcompact.cpp`. SOURCED |
| Modulo addressing | Length in modifier bits 27:16, 0 = linear | MAME. SOURCED |
| 16-bit data | Modifier bit 29: data in the top half of the register | MAME. SOURCED |
| Memory | 2 banks x {X, Y} x 1024 words; bank = XYCONFIG bit 0 | MAME. SOURCED; real size UNKNOWN |
| r56 | Plain storage | Saved/restored by the BIOS context switch; meaning UNKNOWN |
| Burst registers (aux 0x99-0x9E) | Stored; a write to 0x9B (burst size) stops with Unsupported | No specification |
| Extension absent | Accessing r32+ reports "extension core register" (test `extension_register_is_reported_when_xy_extension_is_absent`) | PRM |

Tests: `xy::tests` (6), `tests/exec.rs` `xy_port_reads_y_memory_through_its_address_generator`. The BIOS reaches no
code that uses the ports for computation yet, only the context save/restore, so the post-modify and modulo
semantics are not exercised by real firmware.

## Instruction status

**Implemented and exercised by execution tests (`tests/exec.rs`, GNU-assembled programs):**
ADD ADC SUB SBC ADD1-3 SUB1-3 RSUB CMP TST BTST AND OR BIC XOR BSET BCLR BXOR BMSK NOT, MOV and conditional
MOV (LT HI C GE NE EQ), MAX MIN, ASL ASR LSR ROR RRC RLC (single), ASL ASR LSR ROR (barrel), SEXB SEXW EXTB EXTW
ABS NEG NEG_S, MUL64 MULU64 (both MMID modes), MULHI, LD/LDB/LDW (+.X, .A, .AB, .AS), ST/STB/STW, PUSH_S POP_S,
LD_S GP-relative and PCL-relative, B, B.D, Bcc.D, BL, BL.D, BRLT BRHS BRLO, BBIT0 BBIT1, BREQ_S, J, J.D, JL, J_S
[blink], J.F [ILINK1], J.F [ILINK2], LP, LPcc (false), LR/SR (IDENTITY, INT_VECTOR_BASE, STATUS32, LP_START,
unknown), FLAG, FLAG 1 (halt), SLEEP, SWI, BRK_S, NORM, SWAP, ADDS, SUBS, ADDSDW, SAT16, ABSS, NEGS, ASLS, ASRS,
level-1 interrupt entry/return, priority/level selection, masked interrupts, timer 0 interrupt, illegal and
unsupported diagnostics, delay-slot rule, determinism.

**Implemented, decoder-validated, not yet covered by a dedicated execution test:** RCMP; BREQ/BRNE/BRGE
(32-bit); Jcc/JLcc with conditions; JL_S, JEQ_S/JNE_S; register-register LD; the remaining 16-bit ALU, load and
store forms; SUB_S.NE; NORMW, ABSSW, NEGSW, SUBSDW; timer 1; AUX_IRQ_HINT; AUX_IRQ_LEV changes; level-2 entry
from IRQs; SEMAPHORE; legacy STATUS; 32-bit BRK; PREFETCH.

**Executed by the real v1.5 BIOS without error before the first hardware wait** (85 mnemonics, 1,261 distinct
addresses, ~19,700 instructions): brlo ld.di add_s mov_s ld_s asl_s or_s xor_s add st_s ldw_s ld lsr_s brne
ldb.ab ldb_s brne_s sub_s cmp_s extw_s push_s pop_s breq and lsr blt_s asl breq_s brlt.d brlt st.aw stb_s mov
ld.ab xor extb_s or ldb.di brge brhs.d bl_s stw_s bne_s extw bic brge.d j_s.d asr_s b_s ldh brhs st j_s extb stb
ld.aw bset and_s rsub bmsk and.f sub sth jeq bl.d st.di mov.f lsr.f lpne bne.d bne bc.d add2 jl_s.d ble_s beq_s
ldh.di bset_s bmsk_s add3 st.di.as ldh.ab jl_s j b.d. No oracle exists for these results yet: running without
error is **not** proof that they are correct.

**Executed since 2026-10-16:** MULDW/MACDW/MSUBDW (SOURCED: MAME `dsp_multiply`, UNVERIFIED: per lane a signed
16x16 product into a 64-bit accumulator, set/added/subtracted; result = (acc + (AUX_MACMODE bit 2 ? 0x4000 : 0))
>> (bit 3 ? 15 : 16), saturated to 16 bits with sticky S1/S2) and RND16 (PRM instruction page,
`(sat32(src + 0x8000) & 0xFFFF0000) >> 16`; the shift is VERIFIED-FW by the Leapster 2 BIOS 0.6, which follows
`rnd16 r10,r9` with `asl r10,r10,16; sub r11,r9,r10` at 0x4003_E0B8; the upper 16 bits are UNVERIFIED). The Leapster
2 BIOS runs both in XY-memory filter loops (0x4003_E080-0x4003_E6D4) for The Talking Words Factory. Tests:
`dual_16x16_multiply_accumulate` (vector `t_dsp`).

**Accumulators as r56/r57 (2026-10-16, VERIFIED-FW use, details UNVERIFIED):** the BIOS's PCM output filter
(0.6 0x4003_E0A4, AUX_MACMODE 0x12C) reads the two lanes' accumulators as `adds r9,r56,r57` right after its MACs,
rounds with RND16, feeds the residual back and doubles the result (`adds r10,r10,r10`). r56 is the high lane, r57
the low lane (summed, so the order is not observable here); r57 is also MLO. The filter's coefficients are stored
halved (poles at about 0.99: a DC-blocking high-pass), which only works if fractional mode (bit 3) doubles each
product, so Ribbit keeps the accumulator in Q31 (the lane results are unchanged from MAME's `>> 15`). Before this,
r56/r57 read stale values and The Talking Words Factory's 32 kHz stream sat at +/-32767; read as Q30 it carried 17%
of its energy above 12 kHz. Writes to r56/r57 set the accumulators (the RTOS restores them per task). Reads of an
accumulator beyond 32 bits saturate (UNVERIFIED). Test: `dual_mac_accumulators_read_back_in_fractional_mode`.

**Unsupported (stop with a diagnostic):** DIVAW (PRM does not state signedness), all other DSP/XMAC extension
instructions, XY burst transfers (aux 0x9B), extension
core registers r32-r56 when the XY extension is not configured, r61, writes to PCL, timer watchdog reset, any undefined major-0x05-0x0B extension.

**Illegal on the A5 (Instruction Error):** MPY family, EX, SYNC, RTIE, TRAP_S, UNIMP_S and the encodings listed
above.
