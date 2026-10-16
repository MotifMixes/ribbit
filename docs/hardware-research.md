# Original Leapster: hardware research

Scope: the **original 2003 LeapFrog Leapster** only. Leapster TV, L-MAX, Leapster 2,
Explorer, GS and LeapPad facts appear only where a source mixes them in, and are labelled.

Research date: 2026-10-12. MAME was read at `master` immediately after commit
"arcompact CPU fixes + leapster progress checkpoint" (2026-10-07, PR #16299).

## Confidence labels

| Label | Meaning |
|---|---|
| **VERIFIED** | Checked by Ribbit against a real artifact (the user's dump), with a reproducible command or test. |
| **SOURCED** | Stated by a single secondary source (usually MAME), not checked by Ribbit. |
| **CONFLICT** | Sources disagree. |
| **HYPOTHESIS** | An inference or a guess that is consistent with the evidence but unproven. |
| **UNKNOWN** | No usable information. |

Nothing below is backed by documentation from LeapFrog or by traces from real hardware. MAME's own
driver is `MACHINE_NOT_WORKING`, and all 136 entries in `hash/leapster.xml` are `supported="no"`.
**As far as we found, no public emulator runs Leapster games yet.**

## Sources

| # | Source | License | Notes |
|---|---|---|---|
| S1 | MAME `src/mame/leapfrog/leapster.cpp` | BSD-3-Clause | Machine driver: memory map, peripherals, PCB notes. Many registers are guesses; the comments say so. |
| S2 | MAME `src/mame/leapfrog/leapster_a.cpp/.h` | BSD-3-Clause | Audio device: voices, LFC speech codec, PCM DMA. |
| S3 | MAME `src/devices/cpu/arcompact/*` | BSD-3-Clause | ARCompact CPU core with Leapster-specific flags. |
| S4 | MAME `src/devices/bus/leapster/*` | BSD-3-Clause | Cartridge slot, ROM and NVRAM devices. |
| S5 | MAME `hash/leapster.xml` | CC0-1.0 | Software list: 136 cartridge dumps with CRC32/SHA-1. |
| S6 | LeapFrog-Tools README (BLiNXthetimesweeperGOD, archived 2024-08-31) | none stated | Memory-map names (SDRM/BIOS/CART), ROM splitting, A-law audio. |
| S7 | eLinux "Leapster" page (gitbook mirror) | — | General specs: CPU, RAM, display, Flash MX 2004 content. |
| S8 | Punkster (LogicMage) | AGPL-3.0 | C emulator stub; ISA, ASIC, graphics and sound all listed as TODO. No usable facts. |
| S9 | ARCompact ISA Programmer's Reference (ARC 600), via the MIPS ARC 605 product page | proprietary documentation | The ISA reference for ARCtangent-A5/ARC 600. **Ribbit has not obtained a copy yet** (see open-questions Q1). |
| A1 | The user's SpongeBob dump (local only, never committed) | — | Ribbit's own analysis (`ribbit inspect-cart`, `tests/user_images.rs`). |

## CPU

| Fact | Status | Source |
|---|---|---|
| Custom ASIC containing an **ARCtangent-A5** ("ARCTangent 5.1") core, i.e. the **ARCompact** ISA (mixed 16/32-bit instructions), the same ISA family as ARC 600/700 | SOURCED (two sources) | S1, S3, S7 |
| Nominal clock 96 MHz | SOURCED | S1, S7 |
| MAME runs the core at 48 MHz because "/2 gives a closer speed/framerate to real hardware videos" | SOURCED (empirical) | S1 |
| Clock divider register at `0x0180_9008`: low 3 bits are a power-of-two divider, `96 MHz >> n` | HYPOTHESIS (MAME has it commented out) | S1 |
| **Little-endian** data | VERIFIED: the cart directory stores `0x8000_0048` as `48 00 00 80` | A1 |
| Has the ARC **XY memory / DSP extension**. MAME implements the subset the firmware uses: r32-r55 alias address generators | SOURCED | S3 |
| Optional `MULHI` aux register (0x12) is used. Firmware also writes MMID directly when restoring multiply state | SOURCED | S3 |
| **Misaligned accesses ignore the low address bits** (no exception). The Flash player depends on this through a null-pointer bug | SOURCED, **tested on a Leapster 2, not on an original** | S1 |
| Leapster-specific aux registers 0x10, 0x11, 0x1A, 0x1B, 0x47, 0x48 and 0x4B. 0x1A receives 0x1000 increasing words at startup ("probably not palette") | SOURCED, meaning UNKNOWN | S1 |
| MAME's core treats every instruction as one cycle | SOURCED, so MAME has **no cycle timing** | S3 |
| Firmware runs the **MQX RTOS** (magic numbers in the BIOS match MQX's TAD table) | HYPOTHESIS | S1 |

### Exceptions and interrupts (ARCompact model as implemented by MAME)

* The vector table is at `INT_VECTOR_BASE`, with 8 bytes per vector. MAME's default base for the Leapster is `0x4000_0000`.
* Vector 0 is reset, 1 is memory error, 2 is instruction error. Vectors 3-31 are IRQs.
* There are two priority levels. Level 1 links to ILINK1 (r29) and level 2 to ILINK2 (r30). `STATUS32.E1` (bit 1) and
  `E2` (bit 2) enable them, and `AUX_IRQ_LEV` selects each IRQ's level. STATUS32 is saved to `STATUS32_L1`/`STATUS32_L2`.
* Interrupts are not taken in a branch delay slot.
* Zero-overhead loops: `LP_START`, `LP_END`, `LP_COUNT` (r60).

Status: SOURCED (S3). The ISA reference (S9) is needed to confirm the details. MAME's own comments mark parts as uncertain.

### Leapster IRQ lines (from S1)

| IRQ | Source |
|---|---|
| 0x10 | ADC (touchscreen FIFO); also flagged by bit 8 of `0x0180_0080` |
| 0x12 | Framebuffer DMA complete |
| 0x13 | PCM DMA half-buffer (flag bits 20/21 of `0x0180_0080`, gated by `0x0180_0084`) |
| 0x19, 0x1A, 0x1E | Timers 0, 1, 2 |
| 0x1B | UART (receive, or transmit ready) |
| 0x1D | Audio voice-command acknowledge |

All SOURCED. How these lines map onto ARC vector numbers and levels needs confirmation.

## Memory map

| Range | Device | Status | Source |
|---|---|---|---|
| `0x0000_0000-0x007F_FFFF` | BIOS ("BaseROM"), also at `0x4000_0000` (MAME `.mirror(0x4000_0000)`) | SOURCED; BIOS at 0x4000_0000 is confirmed by two sources | S1, S6 |
| `0x0180_0000-0x0180_FFFF` | On-chip peripherals (table below) | SOURCED | S1 |
| `0x0300_0000-0x0300_FFFF` | VRAM, 64 KiB. Also used as stack before MQX starts | SOURCED | S1 |
| `0x0301_0000-0x0302_FFFF` | v1.5 BIOS clear-screen writes overrun VRAM here. MAME discards them | SOURCED; real behaviour UNKNOWN | S1 |
| `0x3C00_0000-0x3C1F_FFFF` | SDRAM, 2 MiB (ISSI IS42S16100 on the PCB) | SOURCED (three sources) | S1, S6, S7 |
| `0x3C20_0000-0x3FFF_FFFF` | MAME maps RAM here (for the Leapster 2's 16 MiB). On the original this is UNKNOWN: mirror, open bus or fault | CONFLICT | S1 |
| `0x3C80_0000` | "SDCD" region (Leapster 2 SD-card games). Not applicable | SOURCED | S6 |
| `0x5000_0000`, `0x6000_0000`, `0x7000_0000` | Possibly extra cartridge ports (the factory test screen lists 3-4 slots) | HYPOTHESIS | S6 |
| `0x8000_0000-` | Cartridge ROM (4/8/16 MiB) | VERIFIED: cart pointers are `0x80xx_xxxx` | S1, S6, A1 |

Ribbit implements this map in `crates/ribbit-memory/src/map.rs` and `bus.rs`. Unmapped accesses read 0 (a HYPOTHESIS
taken from MAME's default) and are recorded.

### Peripheral registers (all SOURCED from S1 unless noted)

| Address | Name / purpose | Notes |
|---|---|---|
| `0180_0030-003F` | EEPROM command interface | Word 0 = command (`addr<<16 \| bank<<8 \| data`), word 1 = 2 starts a write, word 2 reads data. Bank `0x26` selects the system EEPROM; otherwise the cart. MAME calls its own bank selection "hacky" |
| `0180_004C` | **GIODataIn** (official name) | Buttons, active-low, plus power/dock bits (Leapster 2 also has flash data) |
| `0180_0080` | Interrupt flag / ack | Write 1 to clear. Bit 7 = EEPROM ready (MAME forces it on) |
| `0180_0084` | Interrupt enable | |
| `0180_0090-00AB` | ADC | 4 channel controls; `+0x10` FIFO data (`channel<<16 \| data<<5`); `+0x18` status (bit 8 = FIFO empty). Ch0 touchscreen, ch2 LCD-related, ch3 battery? |
| `0180_1000-101B` | Unknown reads | MAME returns 0 |
| `0180_2070` | Audio voice command | `cmd<<3 \| voice`; IRQ 0x1D acks |
| `0180_2078` | Active-voice bitmap | |
| `0180_20E0-20E7` | LFC speech codebook page | |
| `0180_3000-300F` | 2 circular PCM DMA channels (control/base) | 0x7F01 means 32 kHz with 128-sample halves |
| `0180_4000-4FFF` | Voice registers (start/end/volume/pitch) | |
| `0180_8084-809B` | LCD controller | +4 framebuffer base, +8 stride, +C format (bit 31 enable; 3 = RGB332, 4 = 12 bpp packed) |
| `0180_8800-881B` | Framebuffer DMA | src, stride (words), lines, dest offset; write 0x1B to start; IRQ 0x12 |
| `0180_9004` | System configuration / straps | Cart present (bit 26, 0 = present), LCD type, debug and logging flags, Flash frame-rate uncap, skip touch calibration (bit 14). **Measured on a Leapster 2** |
| `0180_9008` | CPU clock divider | See CPU |
| `0180_B000-B00B` | Unknown | |
| `0180_D000-D8FF` | 3 timers at 16 MHz: count, control (bit 0 IRQ enable), max | Timer 0 sits at an offset of +0x84. Modelled; see "Timers and peripheral clock" |
| `0180_D510/D514` | **UART** data/status | Firmware debug console. MAME logs it. Valuable for boot diagnostics |

## Cartridge bus and image layout

* Edge connector, 60 contacts: **16-bit data bus** (D0-D15), address lines A0-A23, `CE`, `OE`, `WE` and a `Byte`
  (byte/word mode) pin. A 24LC02B serial EEPROM (I²C) is wired to two contacts. SOURCED (S1 pinout).
* Retail images are 4, 8 or 16 MiB (34 / 228 / 8 entries in S5). VERIFIED by counting S5.
* MAME maps the whole image linearly at `0x8000_0000`. SOURCED.

### Header directory: VERIFIED on one image (A1), unverified on other titles

```
0x00  "LEAP"
0x04  u16 0x0100          (unknown; perhaps a format version)
0x06  u16 N               number of directory entries (5 in SpongeBob)
0x20  N x { tag:u16, count:u16, ptr:u32 }
      ptr -> count x { id:u16, flags:u16, value:u32 }
```

The counts and pointers are self-consistent across all five entries in the SpongeBob image. Tag `0x1003` holds
metadata. `value` either points to a NUL-terminated string or is an immediate:

| id | SpongeBob value |
|---|---|
| 0x0A | `152-01508` (internal part number) |
| 0x0B | `SpongeBob SquarePants` |
| 0x0C | `ToolPad V2.2.29 Leapster Univ Cart V1.0.14` (build toolchain) |
| 0x0E | build machine name |
| 0x0F | `Oct 09 2003 18:23:53` (build date) |
| 0x0D | builder name (not reproduced here) |

Tags `0x1005`, `0x1006`, `0x100C` and `0x2000` point to data whose meaning is UNKNOWN.

### Content: VERIFIED (A1)

* The image contains **29 uncompressed SWF ("FWS") movies, all version 5 (Flash 5 / Flash MX era)**. They are stored
  back-to-back from `0x801E_BFA8`, sorted by decreasing size, and the last one ends near `0x807D_C2E1`.
* About 2 MiB before the first movie (`0x8000_0600-0x801E_BFA8`) is not yet identified. Candidates are ARC code, A-law
  samples, LFC speech and fonts.
* **Consequence:** game logic is Flash content (ActionScript) interpreted by a Flash player that lives in the **BIOS**.
  Accurate emulation therefore requires the user's BIOS dump. Re-implementing a Flash player (HLE) would violate
  the project's accuracy principle, and it is out of scope.

### User's image

The user's file matches MAME `spongeba` exactly: "SpongeBob SquarePants - Saves the Day (USA, alt)", serial
500-11002 (MAME notes the same ROM as 500-01168), 8 MiB, CRC32 `0f2ad725`, SHA-1
`258e5b006086a7fd4cfa4460f3008604a5d87e1d`. This is an original-Leapster cartridge, not the L-MAX release
(500-11905-A). VERIFIED.

## System firmware (BIOS)

Known original-Leapster dumps (S1). Hashes are in `crates/ribbit-memory/src/known.rs`:

| Revision | Size | Build date (from MAME comments) |
|---|---|---|
| Universal v1.5 (152-10346, chip 155-10072-a) | 2 MiB | Sep 04 2003 |
| US v2.1 (152-11265) | 8 MiB | Apr 13 2005 |
| UK v2.1 (152-11452) | 8 MiB | Aug 30 2005 |
| German v2.1 (152-11435) | 8 MiB, **MAME BAD_DUMP** | Oct 21 2005 |
| Spanish v1.0 (152-11546) | 8 MiB | Apr 03 2006 |

The v2.1 sets are larger because "Learning with Leap" is built in (S1). Ribbit recognises and **refuses**
Leapster TV, L-MAX and Leapster 2 firmware. Unknown dumps are accepted but reported as unverified.

**Supplied 2026-10-12:** the user provided all five original-Leapster dumps (plus a Leapster Connected image, which Ribbit refuses); every SHA-1 matches MAME. Kept in the git-ignored `roms/` folder.

## Graphics and display

| Fact | Status | Source |
|---|---|---|
| 160x160 CSTN colour LCD with touchscreen | SOURCED (two sources) | S1, S7 |
| "4Mb ATI chip" for graphics | CONFLICT: S7 says no hardware acceleration; MAME models none except DMA | S1, S7 |
| No 2D/3D acceleration: the Flash player renders in software into RAM, then DMA copies to VRAM | HYPOTHESIS consistent with S1 | S1 |
| Format 4: 12 bpp, two pixels in three bytes (`b0=G0B0`, `b1=R0R1`, `b2=G1B1`) | SOURCED | S1 |
| Format 3: RGB332 (touch calibration screen) | SOURCED | S1 |
| 60 Hz refresh | HYPOTHESIS (MAME picks 60) | S1 |
| LCD controller init sequence and framebuffer DMA | VERIFIED-FW (both BIOSes; see Phase 4) | BIOS, S1 |
| Display timing, VBlank/line IRQs | UNKNOWN | — |

## Audio

From S2 (SOURCED; MAME states that fixed-point behaviour and timing are unverified):

* 8 hardware voices. 0-4 play pitched A-law with loop points (pitch is 1.15 fixed point). 5-6 play raw 8 kHz A-law.
  7 is the **LFC speech codec**, 10th-order LPC driven by a codebook supplied by the BIOS.
* The legacy voice mix runs at 8 kHz. MAME upsamples ×4 to 32 kHz, but whether the hardware interpolates is unknown.
* Two circular PCM DMA channels. Channel 0 captures the legacy mix and channel 1 feeds signed 16-bit PCM to the DAC
  at 32 kHz. Firmware mixes in software and refills half-buffers on IRQ 0x13.
* A-law sample data is corroborated by S6. The G.711 decoding itself is standard (Ribbit tests it against reference
  values).

## Input

| Fact | Status | Source |
|---|---|---|
| Buttons on GIODataIn (`0x0180_004C`), active-low. Bit masks are in `ribbit-input` | SOURCED | S1 |
| Up, Down, Left, Right, A, B, Hint, Home, Pause, Volume ±. Brightness ± and Contrast on internal lines | SOURCED | S1 |
| Touchscreen read via ADC channel 0 FIFO. The BIOS discards the first 17 samples after enabling. Each report is 12 samples (pressure, pressure, X, Y, pressure, pressure) ×2 | SOURCED; MAME's own model is partly guessed | S1 |
| 11-bit ADC range, about 0x000-0x7F0 | SOURCED | S1 |
| Touch calibration is stored in the system EEPROM. MAME ships a "calibrated defaults" 0x200-byte NVRAM | SOURCED | S1 |

## Non-volatile storage

| Fact | Status |
|---|---|
| Console spec: "256 bytes NVRAM". The console PCB has a **24LC02B** (2 Kbit = 256 bytes) | SOURCED (S1 PCB notes, S7) |
| MAME's system EEPROM buffer is **0x200** bytes | CONFLICT with 256 |
| The cart PCB has a 24LC02B footprint ("B1") | SOURCED (S1) |
| MAME's cart NVRAM is **0x800** bytes; the S1 header says carts have "2-512 kb" NVRAM | CONFLICT |
| All 136 softlist entries use the `rom_nvram` slot type | VERIFIED (S5 count) |
| How the EEPROM is accessed (an I²C controller behind `0x0180_0030`?) | HYPOTHESIS |

## Observed in Ribbit (BIOS v1.5 and US v2.1, 2026-10-12)

* VERIFIED: the reset vector jumps to 0x4002_B5F4, which sets `sp = 0x0300_0800` (stack in VRAM).
* VERIFIED: within ~19,700 instructions the v1.5 BIOS touches 0x0180_100C/1018, 0x0180_9004/9008/9080,
  0x0180_B000, 0x0180_C000/C004 and 0x0180_D800-D808, then busy-waits until the word at **0x0180_D800**
  reaches 0x27100. (Phase 2 status; resolved in Phase 3, see "Timers and peripheral clock".)
* VERIFIED: a byte read of 0xE000_0100 occurs. Purpose UNKNOWN (open question).
* HYPOTHESIS: the Leapster-specific aux registers MAME logs (0x10, 0x11, 0x1A, 0x1B, 0x47, 0x48, 0x4B) are
  ARC cache controls; binutils names them ivic, che_mode, ic_ram_address, ic_tag, dc_ivdc, dc_ctrl, dc_flsh.
  MAME treats writes to 0x1A as a palette. Not yet reached by Ribbit.
* Firmware evidence for the MUL64/MMID quirk: see docs/isa-arcompact.md.

## Timers and peripheral clock (Phase 3, 2026-10-12)

Labels: **VERIFIED-FW** = read directly from the user's BIOS code (GNU objdump) and/or observed in Ribbit's
traces; SOURCED = MAME; HYPOTHESIS as before. No hardware measurement exists for any of this.

### The boot-time wait (VERIFIED-FW, identical in v1.5 and US v2.1)

| | Universal v1.5 | US v2.1 |
|---|---|---|
| Setup | 0x4002_BA1E-BA36 | 0x4003_90E2-90F2 |
| Poll loop | `0x4002_BA3A: ld.di r15,[r0,0xcc]` / `0x4002_BA3E: brlo r15,0x27100,0x4002BA3A` | `0x4003_90F6` / `0x4003_90FA` (same instructions with r3) |
| Released at | 0x4002_BA46 | 0x4003_9102 |

Sequence (both versions; base `r0 = 0x0180_D734`):

1. `0x0180_9008` (MAME: CPU clock divider): read, low 3 bits set to 1, written; read, low 3 bits set to 5, written.
2. `0x0180_9080` <- `0x1104` (from a BIOS configuration table at 0x4008_DF5C in v1.5, entries tagged 0xCAFExxxx).
3. Timer 2: `0x0180_D808` <- 0xFFFF_FFFF (limit), `0x0180_D804` <- 0 (control), `0x0180_D800` <- 0 (count).
4. Poll `0x0180_D800` (32-bit `ld.di`) until it is >= 0x27100 (unsigned `brlo`). Nothing else is required:
   no interrupt, no status flag, interrupts are still disabled (STATUS32.E1 = E2 = 0).
5. `0x0180_C008` <- 0xCAFE_203F, `0x0180_C014` <- 1, `0x0180_C018` <- 1, then the same 0x27100 wait again
   (0x4002_BA66 in v1.5), then more clock-register writes.

Interpretation (HYPOTHESIS): a clock/PLL change followed by a settle delay, then memory-controller setup, then a
second settle delay. Released from the wait purely by the counter value, so the counter must **count up while
CONTROL = 0** (i.e. CONTROL does not gate counting).

Other waits on the same timer: `brlo ...,0x3e80` (16,000 counts) at 0x4002_B8C2 and 0x4002_CB88 in v1.5, not
executed on the boot path observed so far.

### Counter frequency: 16 MHz (VERIFIED-FW constants; consistent with MAME)

* The MQX BSP initialisation at 0x4007_6440 stores **100** and **0x27100** side by side in its BSP structure,
  and its tick ISR advances the 64-bit tick count once per 0x27100 timer counts: 100 ticks/s × 160,000 =
  **16,000,000 counts/s**.
* A device-clock query at 0x4007_8940 returns the constant **16,000,000** for the devices at 0x0180_D500
  and 0x0180_D600 (UARTs, see below), and otherwise calls the CPU-clock query.
* The CPU-clock query at 0x4007_A148 reads **aux register 0x56** (Leapster-specific): 0 -> 12 MHz; else bits
  3:2 = 0 -> 96 MHz, 1 -> 48 MHz, 2 -> 24 MHz, 3 -> 12 MHz. So the CPU clock is variable while the peripheral
  clock is quoted as fixed: the timer frequency does **not** follow the CPU clock (also how MAME models it).
* MAME (S1) uses 16 MHz for all three timers.

The 160,000-count wait is therefore 10 ms of peripheral time. It is not 160,000 instructions: in Ribbit at a
96 MHz CPU, each wait takes exactly 960,000 CPU cycles (480,000 two-instruction poll iterations, about three
reads per count), measured by `tests/user_bios.rs`.

Unresolved: the real CPU clock and its relationship to the 0x0180_9008 writes (1 then 5) and to aux 0x56. MAME
interprets 0x0180_9008 as `96 MHz >> n`, which would make 5 mean 3 MHz; that seems unlikely and is not used by
Ribbit. Ribbit's CPU clock is a configuration value (default 96 MHz nominal; MAME uses 48 MHz) and only changes
how many instructions fit into a peripheral-time interval.

### Register model

| Address | Register | Status |
|---|---|---|
| 0x0180_D084 / 088 / 08C | Timer 0 count / control / limit | SOURCED (MAME) + VERIFIED-FW (MQX programs it here) |
| 0x0180_D400 / 404 / 408 | Timer 1 | SOURCED; v2.1 writes control = 3 |
| 0x0180_D800 / 804 / 808 | Timer 2 | SOURCED + VERIFIED-FW (boot waits) |
| IRQ lines | timer 0 -> 0x19, timer 1 -> 0x1A, timer 2 -> 0x1E | 0x19 VERIFIED-FW (MQX installs its tick ISR on vector 0x19); others SOURCED |

Behaviour implemented in `crates/ribbit-core/src/io/timer.rs`:

* 32-bit registers; the count increments by 1 per peripheral clock (VERIFIED-FW: never decrements; the BIOS
  helper at 0x4007_6608 treats it as an up-counter modulo 2^24 around the MQX tick).
* Reaching LIMIT, then one more count, returns it to 0 and (if CONTROL bit 0 = IE) asserts the interrupt.
  Counting is not gated by CONTROL.
* The interrupt is level-sensitive and is cleared by writing CONTROL. The MQX tick ISR (0x4007_664C) re-arms
  the timer by adding 0xFD8F03 to COUNT and writing CONTROL = 3 on every tick.
* CONTROL bit 1 (NH): count only while the CPU is not halted.
* The layout (COUNT/CONTROL/LIMIT) and the MQX value CONTROL = 3 = IE|NH match ARC's own processor-timer
  block (PRM), so ARC semantics fill MAME's gaps. HYPOTHESIS. In particular, whether the counter rolls over
  *at* LIMIT (MAME) or *after* LIMIT (ARC) is a 1-count difference that only hardware can settle.
* Reset values: count 0, control 0, limit 0xFFFF_FFFF (MAME). UNKNOWN on hardware; the BIOS writes all three
  before use.
* Byte/halfword writes to timer registers are reported as unmodelled; sub-word reads return the register bytes.

Validated in Ribbit: on US v2.1, timer 0 wraps 18 times in 20 M steps and the CPU takes exactly 18
interrupts, all on vector 0x19 (the MQX tick at 100 Hz of peripheral time), each returning through
`J.F [ILINK1]`.

### Interrupt flag/enable registers and debug UART

* 0x0180_0080 / 0x0180_0084 (interrupt flags / enable): modelled per MAME (flags are write-1-to-clear; PCM
  bits gated onto line 0x13). VERIFIED-FW usage: v2.1 writes 0x100 to 0x0180_0080 before initialising the ADC.
  MAME's forced "EEPROM ready" bit 7 is *not* reproduced.
* Debug UART at 0x0180_D500: transmit register 0x0180_D510 captured (SOURCED). The BIOS configures
  0x0180_D514/D518/D51C with byte writes (D514 <- 0, D518 <- 0xCF, D51C <- 0, then D514 |= 4) but had
  transmitted nothing by the end of either run. Status/control semantics are left unmodelled.

### Next blockers at the end of Phase 3 (all resolved in Phase 4, see below)

* **v1.5**: at 0x4007_3D74 MQX restores a task context by loading r32-r57 from the task's stack frame. The PRM
  forbids loads into extension core registers, so on the Leapster these registers really exist (MAME models
  r32-r55 as the XY-memory/DSP extension's address aliases). Ribbit stops: "extension core register r32 (XY
  memory/DSP extension) is not modelled".
* **US v2.1**: at 0x4008_EBA6 the BIOS drains the ADC FIFO: it reads 0x0180_00A0 while bit 8 of 0x0180_00A8 is 0
  (MAME: bit 8 = FIFO empty). The ADC is unmodelled, so the loop never ends.
* **Straps register 0x0180_9004** (both): read at boot; bit 21 selects whether the BIOS checksums a ROM region
  (0x4002_D698-D6D2 in v1.5). Ribbit leaves it unmodelled (reads 0). MAME 0.289 returns 0x0020_0000; MAME's
  development branch returns 0x63FF_BFFF | cart-absent | calibration bits, "taken from testing with a Leapster
  2". Its original-Leapster value is UNKNOWN.

### Independent comparison with MAME

* MAME's newest **release** (0.289, 2026-07-30) has no timer model: 0x0180_D800 returns the constant 0x27100.
  The timer model exists only in MAME's development branch (commit da56c5ac, 2026-10-07), which would have to
  be built from source (MinGW/MSYS2 toolchain not installed here).
* With MAME 0.289's constants supplied for the four registers neither emulator models (`--mmio-stub`, debug
  only), Ribbit and MAME 0.289 visit **the same 3,206 distinct BIOS addresses in the same order** (v1.5, reset to
  0x4007_3D74, ~1.96 M instructions in Ribbit) and **750** addresses (v2.1, reset to the end of the timer wait).
  At the shared v1.5 stop r0-r25 are identical.
* Discrepancy: MAME 0.289 reports STATUS32 = 0x400 (N set) at that stop; Ribbit has 0. The last flag-setting
  instruction is `tst_s r0,r0` on 0x3C00_0D8C (non-negative), so per the PRM N must be 0. Recorded as a
  suspected MAME 0.289 flag bug, to re-check against MAME's development branch or hardware.
* Without stubs, the first divergence is the straps-register branch at 0x4002_D6AE, i.e. a peripheral
  difference, not a CPU difference.

## Phase 4: XY extension, ADC, straps, LCD and framebuffer DMA (2026-10-12)

Labels as in Phase 3. Each BIOS was run and tested on its own; nothing measured on one is assumed for the
other. Both run 200 M steps with **no CPU error, halt or unsupported instruction**, and both stop at the same
logical point: waiting for ADC conversions.

### Per-BIOS results (default straps 0x63FF_BFFF, no cartridge, 200 M steps)

| | Universal v1.5 (`155-10072-a`) | US v2.1 (`152-11265`) |
|---|---|---|
| Milestone reached | MQX running; LCD initialised; framebuffer DMA done; display switched to format 3 (MAME: touch calibration) | same |
| LCD init (format 0x8000_0004, stride 0xF0, base 0) | cycle ~2.07 M | cycle ~2.17 M |
| Framebuffer DMA transfers | 1 (source 0x3C01_6540) | 3 (last source 0x3C01_66A0) |
| Switch to format 3 (0x8000_0003, stride 0xA0) | cycle ~145.9 M | cycle ~156.6 M |
| Interrupts by vector | 0x12 x1, 0x19 x206, 0x1A x6, 0x1E x390 (603) | 0x12 x3, 0x19 x206, 0x1A x16, 0x1E x390 (615) |
| Final PC / stop | 0x4008_04A8 loop, step limit | 0x4009_2904 loop, step limit |
| Screen | format 3, all black (VRAM is zero) | same |
| UART output | none | none |
| Regression tests | `v15_timer_waits_are_10_ms`, `v15_initialises_lcd_runs_dma_and_switches_display_format`, `v15_zero_straps_reports_missing_lcd_config` | `v21_initialises_lcd_runs_dma_and_switches_display_format`, `v21_zero_straps_reports_missing_lcd_config` |

The two BIOSes differ in DMA count and timer-1 interrupt count; the cause is UNKNOWN (different code, not
investigated). These cycle counts depend on Ribbit's placeholder timing (1 cycle per step, 96 MHz) and are
regression markers, not hardware timings. Reproduce with
`ribbit headless --firmware <bios> --max-steps 200000000 --tail 0`.

### XY memory / DSP extension (r32-r55, aux 0x80-0x9F)

See docs/isa-arcompact.md, "XY memory extension". Evidence that it exists on this CPU: the v1.5 MQX context
save at 0x4007_3CA0 stores r32-r59 to sp+140..248 and the restore at 0x4007_3D74 loads them back. That restore
also uses the saved MMID as a low word, more evidence for the MMID quirk.

### ADC FIFO (0x0180_00A0 / 0x0180_00A8)

* SOURCED (MAME) + VERIFIED-FW: reading 0x0180_00A0 pops a sample; bit 8 of 0x0180_00A8 = FIFO empty. Drain loops:
  v1.5 0x4007_CCBA, v2.1 0x4008_EBA6. Ribbit's FIFO is always empty (reads give 0 and are counted).
* VERIFIED-FW (v1.5 ISR at 0x4007_CBB0, vector 0x10): checks interrupt flag bit 8, drains the FIFO while bit 8 of
  0x0180_00A8 is 0; for each sample, channel = (data >> 16) & 3 and value = (data & 0xFFFF) >> 5, dispatched to a
  per-channel callback table at `[0x3C1F_ED00] + 8 + 4*ch`; then writes 0x100 to 0x0180_0080.
* VERIFIED-FW (both): the BIOS writes 0x8000 to 0x0180_0098 (channel 2) before LCD init and to 0x0180_0090
  (channel 0, touchscreen per MAME) afterwards; the interrupt-enable register ends at 0x0005_4520 (bit 8 = ADC).
* Not modelled: channel controls 0x0180_0090-009C, register 0x0180_00A4, conversion timing, the ADC interrupt.

### System straps (0x0180_9004) and clock control (0x0180_9008)

* With straps = 0 both BIOSes reach a fatal assert: the message "LcdMPI::Init can't find config rsrc" is formatted
  into VRAM (v1.5 near 0x0300_FF00, v2.1 near 0x0300_0800; v1.5 assert function 0x4008_BAD4, printf
  0x4008_BD24, format string `\n!ASSERT: ` at 0x4009_0F88). Only the first UART byte goes out: the BIOS's UART
  driver is interrupt-driven (it writes 0x44 to enable the TX interrupt) and Ribbit does not raise UART IRQ 0x1B.
  With bit 21 = 0 both then spin inside the SysMgr power-off routine (v1.5 at 0x4007_025A, v2.1 at
  0x4008_0F06; see Phase 4b, "Correction").
* VERIFIED-FW: `LcdMPI::Init` (v1.5 filter at 0x4007_B028) looks up resource ID 0x1008_0001 (call at 0x4007_B4A8)
  in the BIOS LEAP resource directory (BIOS offset 0x144) and accepts the record whose key equals
  `0x8003_0000 & ~straps`. Tag 0x1008 has three id-1 records, keys 0, 0x8000_0000 and 0x8001_0000, so straps
  bits 31/17/16 select the LCD panel.
* Ribbit's default is MAME's Leapster 2 measurement 0x63FF_BFFF (bit 26 follows cartridge presence), which selects
  key 0x8000_0000. **The original Leapster's value is UNKNOWN** (open question Q16); `--straps` / `Config.straps`
  override it.
* 0x0180_9008 is a read/write latch. Without it the power loop at v1.5 0x4008_0450 reprograms the clock forever.
  It has no effect on timing (meaning UNKNOWN, Q19).

### LCD controller and framebuffer DMA (0x0180_8000 block)

* SOURCED (MAME) + VERIFIED-FW: base 0x0180_8088, stride 0x0180_808C, format 0x0180_8090; both BIOSes write base 0,
  stride 0xF0, format 0x8000_0004. They also program panel timing 0x0180_8004-802C and CSTN dither tables
  0x0180_8180-81BC (stored, not modelled).
* DMA: 0x0180_8800 control (0x1B starts), +4 source, +8 stride in words, +0xC lines, +0x10 destination offset,
  +0x18 status (write-1-to-clear). `CopyToScreen` (v1.5 0x4007_BAA0) copies 0x3C words x 160 lines and waits,
  with a timeout and retry, for the vector-0x12 handler (v1.5 0x4007_AE8C), which tests status bit 1 (error),
  writes 0x0E and acknowledges interrupt flag bit 14.
* Model: the copy is instant; completion sets flag bit 14, routed to CPU line 0x12 when enabled. The real
  transfer time and any error conditions are UNKNOWN.

### Installed interrupt handlers (v1.5, observed)

| Vector | Handler | Device |
|---|---|---|
| 0x10 | 0x4007_CBB0 | ADC |
| 0x12 | 0x4007_AE8C | framebuffer DMA |
| 0x18 | 0x4008_00CC | UNKNOWN (not in MAME) |
| 0x19 | 0x4007_ECA4 | timer 0 (MQX tick) |
| 0x1A | 0x4007_0430 | timer 1 |
| 0x1E | 0x4002_509C | timer 2 |
| 0x15 | 0x4002_4C60 | audio: voice status (acks flag bit 16, reads 0x0180_2078); installed with the audio driver (Phase 4b) |
| 0x1D | 0x4002_4D2C | audio: voice-command acknowledge (MAME raises 0x1D); Phase 4b |

### Next blocker at the end of Phase 4: ADC conversions (both BIOSes; followed up in Phase 4b)

After switching to format 3 (the touch-calibration screen in MAME), both BIOSes wait for ADC samples on channels 0
and 2. Nothing produces them, so the screen stays black and no calibration UI is drawn. Ribbit does **not**
fabricate samples: MAME's touch model calls itself guesswork, and no hardware capture of the sample format, rate
or idle values exists. Options: (a) adopt MAME's model as a clearly flagged approximation, or (b) get hardware
evidence (logic-analyser capture or a debug-build log of ADC values on a real original Leapster). Open questions
Q11 and Q20.

## Phase 4b: experimental touch model, and what follows calibration (2026-10-13)

Everything in this section that involves touch samples was produced with the **EXPERIMENTAL, UNVERIFIED** MAME
touch model (`--experimental-mame-touch`, off by default). It shows what the BIOS code does when fed MAME-style
samples; it is not evidence about the hardware. Times are emulated time at Ribbit's placeholder 96 MHz / 1 cycle
per step, with the SpongeBob cartridge inserted unless stated.

### MAME's model (source: `src/mame/leapfrog/leapster.cpp`, master commit da56c5ac, 2026-10-07)

Implemented in `crates/ribbit-core/src/io/touch.rs`; its header lists each behaviour with MAME's own confidence.
Summary of what is inherited:

| Behaviour | MAME's basis |
|---|---|
| Channel controls 0x0180_0090-009C store and read back | none stated |
| Writing ch0 control with bit 15 starts conversions; first start pushes 17 zero samples and raises the ADC interrupt | "Whatever these are, they're never used" (the BIOS waits for 17 packets) |
| Every 1/60 s: 12 samples P,P,X,Y,P,P,P,P,X,Y,P,P on channel 0, then the interrupt | "not sure what the intended frequency is, so I've just set it at 60" |
| P = 0x7FF when up, 0 when touched; X/Y raw 0..0x7F0, idle 0x3DF | "pressure or something?"; input-port range |
| ADC interrupt = flag bit 8 + CPU line 0x10, not gated by the enable register | MAME code |
| Channels 1-3 produce nothing (MAME: 2 "something to do with the LCD", 3 "power / battery?") | guesses |

Ribbit-specific differences (documented in touch.rs): level-triggered line 0x10 dropped by the BIOS's flag-bit-8
acknowledge (MAME clears on entry); FIFO overflow counted instead of fatal; scripted presses in emulated ms with
raw X/Y values (MAME's crosshair maps 0..0x7F0 across the 160-pixel screen; the real panel mapping is UNKNOWN).

### Calibration (both BIOSes, independently tested)

* With the model on and the stylus up, both BIOSes take ADC interrupts at 60 Hz and draw a 9-pixel plus-shaped
  target centred on pixel (10,10) in format 3. Without the model the screen stays black.
* Pressing each target (300 ms, raw = pixel x 0x7F0 / 159) moves it on: (10,10), (10,150), (150,150), (150,10),
  (80,80). After the fifth press the display returns to format 0x8000_0004. Identical sequence on v1.5 and v2.1.
* Reproduce: `ribbit headless --firmware <bios> --experimental-mame-touch --touch-press 1700,300,128,128
  --touch-press 3000,300,128,1917 --touch-press 4300,300,1917,1917 --touch-press 5600,300,1917,128
  --touch-press 6900,300,1022,1022 --max-steps 960000000`. Tests: `v15_experimental_touch_calibration_then_power_off_request`,
  `v21_experimental_touch_calibration_then_splash_screen`.

**Straps bit 14 decides whether calibration runs at all.** MAME's `FORCE_CALIBRATION` input sets bit 14 by default
(effective value 0x63FF_FFFF); MAME's comment: "T: If not set, the system will boot to the touch calibration".
Ribbit's default is MAME's raw measurement 0x63FF_BFFF (bit 14 clear), which forces calibration. With
`--straps 0x63FFFFFF` and no touch model, both BIOSes skip calibration and draw their start-up screens
(v1.5: 14 DMAs, v2.1: 21 DMAs in the first 3.1 s). The original Leapster's value is UNKNOWN (Q16); the default is
unchanged, pending the owner's decision.

### After calibration (or with bit 14 set)

| Event | Universal v1.5 | US v2.1 |
|---|---|---|
| ADC interrupts (touch model on) | 60 Hz, vector 0x10 | same |
| First access to the audio block: reads 0x0180_2078, 0x0180_20F0; writes 0x0180_40F4/40F8/4134/4138; one write to 0x0180_2070 (voice command) | 7.34 s | 7.60 s |
| Start-up screens drawn by framebuffer DMA | none (see power-off) | yes, 23 DMAs by 11.2 s, 40 by 20 s |
| SysMgr power-off request | **yes**: armed at 2.02 s, executed at 8.02 s | no |
| `PlayMovie("Packed RIB Movie(s)", handle 0x8000_0000)`, i.e. the cartridge content handed to the Flash player | only with the debug stub below (12.39 s) | 14.14 s (7.43 s with bit 14 set) |
| New code after PlayMovie | none in the following 17 s | none in the following 6-16 s |
| Cartridge | read from 0x8000_0100 at boot (21,373 bytes by 20 s); no code executed from it (expected: games are Flash content) | 0x8000_0100 at boot, 22,505 bytes |
| CPU errors | none in 30 s | none in 30 s (after the MUL64 fix below) |

Without a cartridge, v2.1 calls PlayMovie with handle 0x2000_0000 and a BIOS resource pointer (0x4000_041C)
instead. The start-up screens are BIOS artwork (LeapFrog logo); frames are kept only in local temp folders.

### Blocker 1, v1.5: input register 0x0180_004C bit 11 (shutdown line)

* VERIFIED-FW (v1.5): the system manager (`SysMgr`, string at 0x4009_9E8C) samples 0x0180_004C, masks it with
  0x800 (bit 11, from its state at offset +0x7C) and, with polarity 0 (offset +0x80), treats a 0 as active. After
  debouncing, a stable active level calls 0x4008_0DF0 with reason 4 (0x4008_0BC0), arming a 6-step countdown;
  at 8.02 s the countdown expires and 0x4008_0E74 calls 0x4007_0240, which (straps bit 21 set) ORs 0x100 into
  0x0180_B000. Ribbit does not model 0x0180_004C, so it reads 0.
* SOURCED: MAME returns `0xFFFF_FFFF & ~0xB703_E380 | buttons` (button bits active-low), with the comment "Set all
  non-button bits to 1 to avoid detecting a shutdown for now". Bit 11 is not a button bit in MAME.
* Diagnosis check (DEBUG ONLY, not emulation): with `--mmio-stub 0x0180004C=0xFFFFFFFF` v1.5 does not power off,
  draws its LeapFrog start-up screen (23 DMAs) and reaches PlayMovie on the cartridge at 12.39 s.
* What bit 11 is on the board (power switch? battery? lid?) and the idle values of the other non-button bits are
  UNKNOWN (Q22). v2.1 also reads 0x0180_004C but its SysMgr did not request power-off in 20 s.
* 0x0180_B000 bit 8 is presumably a power-off control; UNKNOWN, unmodelled (MAME does not model writes either).

### Blocker 2, both: the audio unit

* Both BIOSes write one voice command to 0x0180_2070 right after calibration and never another in 30 s. The v1.5
  audio driver installs handlers on vector 0x15 (0x4002_4C60: acknowledges interrupt-flag bit 16 and reads
  0x0180_2078) and vector 0x1D (0x4002_4D2C); its command routine (0x4002_4CB4) waits on an RTOS object before
  writing the next command. MAME raises line 0x1D after every voice-command write ("the audio driver will wait for
  this before making any more voice changes"). Ribbit raises neither.
* After PlayMovie neither BIOS executes any new code. HYPOTHESIS (not verified): the Flash player or the sound
  task is waiting for the audio unit (voice acknowledge 0x1D, voice status 0x15, or PCM DMA 0x13). Next step is to
  identify which RTOS object the blocked tasks wait on, then model the audio unit from MAME's `leapster_a.cpp`
  with the same opt-in/flagging discipline where it is guesswork.

### CPU fix found on the way: MUL64 ignores its A field

US v2.1 at 0x4007_B49C executes `0x2B04_0F83 + limm` (MUL64 with A = 3, b = r3, c = 0x0001_66E9) and then reads
MMID; Ribbit stopped with "MUL64/MULU64 with A field other than 62" at 11.16 s. The PRM states "The destination is
always ignored for the multiply instruction" and MAME ignores the field, so the decoder now accepts any A value
(binutils still rejects it: 65 such words in v1.5, 416 in v2.1, counted under a documented oracle rule).

### Correction to Phase 4

The dead loops reached with straps = 0 (v1.5 0x4007_025A, v2.1 0x4008_0F06) are inside the SysMgr power-off routine
(v1.5 0x4007_0240, v2.1 0x4008_0EEC), which spins instead of writing 0x0180_B000 when straps bit 21 is 0. They
follow the "LcdMPI::Init can't find config rsrc" assert; they are not the assert itself.

## Phase 4c: the sound stall, the voice-command acknowledge, and GPIO input (2026-10-13)

Straps note: Ribbit's default stays 0x63FF_BFFF, MAME's raw measurement **from a Leapster 2**, unverified for the
original Leapster. The experiments below use `--straps 0x63FFFFFF` (MAME's default configuration, bit 14 set: no
forced touch calibration) only so calibration does not hide the audio path; each BIOS was run and tested on its own.

### The stall (VERIFIED-FW, both BIOSes)

There is no CPU wait loop: every task is blocked in the RTOS and only the idle task runs (it polls 0x0180_9008).

1. Shortly after boot `AudioTask` writes one voice command, `0x86`, to 0x0180_2070 (v1.5 0x4002_4D00 at 0.62 s,
   v2.1 0x4002_E12C at 0.89 s). The command routine (v1.5 0x4002_4CB4, v2.1 0x4002_E0E0) first waits on event
   bit 1 of the audio object (`[0x3C1F_EC10/EC50] + 196`, RTOS API slot 0x14), clears it (slot 0x10), then writes
   the byte with interrupts disabled and records "voice `byte & 7` awaiting acknowledge" (state byte -228 = 0,
   -227 = voice).
2. Before touching voice 6 again, `AudioTask` calls the per-voice check (v2.1 0x4002_FC50 from 0x4003_174E;
   v1.5 0x4002_6244): if that voice's command is still unacknowledged it waits on event bit 1 (v2.1 0x4002_FC9A,
   v1.5 0x4002_6286).
3. Event bit 1 is set only by the vector-0x1D handler (v1.5 0x4002_4D2C, v2.1 0x4002_E158), which acknowledges
   interrupt-flag bit 18 (writes 0x4_0000 to 0x0180_0080) and, if 0x0180_2078 & 0x300 == 0, sets the state byte
   and event bit 1. Ribbit never raised 0x1D, so `AudioTask` blocked forever at ~0.9 s.
4. Every later audio request then went unanswered. v2.1's PlayMovie path (0x4002_C528, log `{MasterStopAudio}`)
   sends command 3 through `0x4002_B744` (failure string `{AudioSendCmdMsgFail}`) and waits for the reply
   (0x4002_B7C4): the whole Flash-player start stalled behind the audio driver.

A second handler, vector 0x15 (v1.5 0x4002_4C60, v2.1 0x4002_E090), acknowledges flag bit 16 and sets event bit 2
when the voices it waits for are no longer reported in 0x0180_2078; it was not needed to pass this stall.

### MAME (`leapster.cpp` / `leapster_a.cpp`, master da56c5ac)

| Behaviour | MAME | Status in MAME |
|---|---|---|
| 0x0180_2070 write: IRQ 0x1D, then `do_voice_command(data >> 3, data & 7)`; command 0x10 starts the voice, others stop it | code + comment "the audio unit acks it by raising IRQ 0x1d" | consistent with the BIOS; latency not stated |
| 0x0180_2078: 8-bit "triggered voices" mask (voice 0 in bit 7, voices 1-7 in bits 0-6) | code | comment: "not sure why it's out of place" |
| Voice playback, LFC speech codec, PCM DMA (IRQ 0x13, flag bits 20/21) | implemented in detail | timing, interpolation, filtering "unverified" |
| Line 0x1D raised directly (no flag bit) | code | the BIOS acknowledges flag bit 18, so the hardware latches it there |

### Implemented (io/audio.rs): the acknowledge only

* A write to 0x0180_2070 sets interrupt-flag bit 18; CPU line 0x1D = flags & enable & bit 18; the handler's
  acknowledge clears it. Routing VERIFIED-FW (handler acknowledges bit 18; the BIOS enables bit 18 in
  0x0180_0084 = 0x0005_4520). Latency UNKNOWN: immediate, as in MAME.
* Not implemented: playback, 0x0180_2078 (still unmodelled, reads 0 = "no voice playing"), vector 0x15, PCM DMA,
  LFC, the 0x0180_4000 voice parameters. All remain in the unmapped-access report.
* Tests: `io::audio::tests` (4), `v15_voice_command_is_acknowledged_and_the_audio_task_continues`,
  `v21_voice_command_is_acknowledged_and_master_stop_audio_is_answered`.

### Results with the acknowledge (straps 0x63FF_FFFF, SpongeBob inserted, 30 s)

| | Universal v1.5 | US v2.1 |
|---|---|---|
| Sound wait | passed: acknowledge arrives before the per-voice check, the wait is skipped (0x4002_628A) | passed (0x4002_FC9E at 0.89 s) |
| Voice commands acknowledged | 2 (then power-off) | 501 |
| Next | SysMgr power-off at 4.07 s (GPIO bit 11, below) | `MasterStopAudio` answered; PlayMovie on the cartridge at 7.43 s |
| Cartridge reads | 21 K bytes (boot) | 235,751 bytes, mostly inside the embedded SWF movies at 0x8076_5854, 0x8077_B0D0, 0x8079_xxxx, 0x807A_6744, 0x807D_0D78 (the cart's ActionScript: `LeapsterASLibrary.as`, `SB_EndRound`, `LF_onButton` ...) |
| Screen | LeapFrog start-up screen, then power-off | start-up screens, then an on-screen letter keyboard with OK / back (only with the cartridge; without one v2.1 shows a U.S.A. / Canada choice) |
| With the DEBUG stub 0x0180004C=0xFFFFFFFF (diagnosis only) | PlayMovie on the cartridge at 5.67 s, 531 voice commands, same keyboard screen | - |

With the default straps, the EXPERIMENTAL touch model and the five calibration presses, v2.1 reaches the same
keyboard by 20 s. A scripted tap on "B" (raw 447, 550) enters "B" in the name field; a tap on OK (raw 1431, 1776)
leads to cartoon artwork (a buoy, a dock, a building) by 32 s, with 110 DMAs and 367,579 cartridge bytes read. The
artwork appears only with the cartridge and is presumably SpongeBob's own; whether game logic beyond this works is
not established, there is no sound, and touch is the unverified MAME model.

### GPIO input 0x0180_004C (io/gpio.rs)

* Only read, never written (about 100 reads/s). Readers: v1.5 button scanner 0x4007_ECC8 (debounces
  `reg & mask`) and SysMgr 0x4008_081A/083C/0ADC/0B1E/0B86; v2.1 0x4008_F638, 0x4009_2D10/2D2E/2FE2/3024.
* v1.5 SysMgr watches three configured lines (mask/polarity pairs at state offsets +0x6C/+0x70, +0x74/+0x78,
  +0x7C/+0x80); the one that powers off is mask 0x800 (bit 11), polarity 0 (active-low). The masks come from BIOS
  configuration; what the lines are wired to is UNKNOWN (Q22).
* Model: button bits (MAME's layout 0xB703_E380, active-low) come from the frontend; all other bits read
  `unresolved_lines`, default 0 (what the unmodelled register returned before), settable for experiments with
  `--input-lines` / `input_lines` (e.g. MAME's workaround 0x48FC_1C7F). Effect of releasing the buttons (they used
  to read as all pressed): no observable change in either BIOS in 30 s; v1.5 still powers off at 4.07 s.
* Tests: `io::gpio::tests` (3), `v15_buttons_read_released_and_unresolved_bit_11_still_requests_power_off`,
  `v21_buttons_read_released_and_no_power_off`.

### Next blockers

* **v1.5**: GPIO bit 11 (shutdown line). Needs hardware evidence for its idle level (Q22).
* **Both**: the rest of the audio unit, once the BIOS or game waits for it: voice playback and 0x0180_2078,
  vector 0x15 (flag bit 16), PCM DMA (0x13). Nothing observed waits on them yet.
* **Both**: the system EEPROM (0x0180_0030-003C, written but unmodelled), which presumably stores calibration and
  the player name.
* **Speed**: Ribbit runs at about 1/7 of real time (14 M steps/s against 96 MHz nominal).

## Phase 4d: EEPROMs, the voice engine, and SpongeBob's menus (2026-10-13)

### System EEPROM and cartridge save chip (io/eeprom.rs)

* VERIFIED-FW (v1.5 read routine 0x4008_1924): mode 3 -> 0x0180_003C; clear interrupt flags 7/6 (0xC0 ->
  0x0180_0080); command `addr << 16 | device` -> 0x0180_0030; 1 -> 0x0180_0034 (read); poll flag bit 7 with a
  timeout and one retry; data from 0x0180_0038, 0xFF on failure. Ribbit never set bit 7, so every read failed.
* The BIOS device table builds device codes from type 0x2000 and 0x000/0x200/0x400/0x600: 0x20, 0x22, 0x24, 0x26.
  0x26 is the system EEPROM (MAME's test agrees); **0x20 is the cartridge save chip**: both BIOSes address it only
  with a cartridge inserted (SpongeBob: 256 writes at boot, the game's own save record later). Writes: 2 -> 0x34
  (MAME). Sizes: system 256 bytes (24LC02B per MAME's PCB notes; MAME emulates 0x200, Q6), cartridge 0x800 (MAME,
  "type unknown"). Erased value 0xFF: HYPOTHESIS. Latency: immediate (UNKNOWN).
* Results: both BIOSes now read the EEPROM (v1.5 86, v2.1 100 reads by 1 s). Touch calibration is written twice (a
  48-byte record at 0x00 and 0x30). SpongeBob writes its profile ("BOB" at 0x20, signature "SBSP") to the cartridge
  chip; on the next boot it shows a SIGN IN screen listing BOB and Guest. Save files: `--eeprom`, `--cart-save`.

### Voice engine (io/voices.rs)

* Why: with the acknowledge alone, sounds "finished" in zero time and 0x0180_2078 always read 0. After the
  sign-in, SpongeBob's Mr. Krabs cut-scene starts speech on voice 7 (LFC) and the screen froze at ~20 s with the
  audio task idle (no sound recorded as active), i.e. the start looked failed and no completion ever arrived.
* Model (SOURCED from MAME `leapster_a.cpp`, presented there as researched, UNVERIFIED on hardware): voices 0-4
  pitched A-law with loop points, 5-6 raw 8 kHz A-law, 7 LFC speech decoded from memory (codebook page from
  0x0180_20E0/E4), 256 start-up samples skipped, 0x0FC0 terminator; status 0x0180_2078 (voice 0 in bit 7); 8 kHz
  sample clock in emulated time; mixed samples kept in a ring buffer (no host output yet). VERIFIED-FW: the
  registers the BIOSes write (voice 6 start/end/volume at boot) fall exactly in MAME's map.
* Result: the cut-scene completes; SpongeBob reaches its main menu (SpongeBob, Gary, the pineapple, four activity
  bubbles) and tapping the Krabby Patty bubble loads that activity's level picker. Both BIOSes play a start-up
  sound on voice 6.

### SpongeBob status (US v2.1, EXPERIMENTAL touch)

Calibration -> LEAPSTER / LeapFrog logos -> name entry (taps work) -> intro (Bikini Bottom) -> Mr. Krabs cut-scene
(with speech decoded) -> main menu -> Krabby Patty activity level picker. Not yet established: gameplay inside an
activity; correct timing (Ribbit runs 0.28x real time at the placeholder 96 MHz); sound output; v1.5 (powers off,
GPIO bit 11).

## Boot process (SOURCED from S1 comments)

1. Reset: PC = `INT_VECTOR_BASE` = `0x4000_0000` (BIOS mirror). The reset-time STATUS32 value is a HYPOTHESIS.
2. A vector table is copied from 0 to `0x3C00_0000` (RAM). MAME is unsure whether 0 is the right source.
3. VRAM serves as the stack until MQX starts.
4. Firmware reads the system configuration at `0x0180_9004`: cart present, skip touch calibration, debug flags.
5. If not calibrated (or forced), the touch calibration screen appears (display format 3).
6. The UART carries debug text throughout (MQX output blocks if the UART IRQ never fires).
7. The firmware finds the cartridge (`LEAP` header at `0x8000_0000`) and starts its Flash content.

## Hardware revisions

* Console PCB: "LEAPSTER MAIN (c) 2004, ASY 310-10069", etch "FAB-300-10069-C", "SW1208 Rev.5". The main ASIC (U1) is
  under an epoxy blob, with U2 = ISSI IS42S16100 SDRAM and U3 = BIOS ROM (S1).
* Cart PCB: "ASY 310-10028 REV:00", ROM under epoxy (U3), 24LC02B (B1) (S1).
* BIOS revisions: see the table above. How LCD panel variants (`0x0180_9004` bits) relate to production runs is
  UNKNOWN.

## Leapster TV (EXPERIMENTAL, 2026-10-15)

BIOS: Leapster TV Universal v2.1.11 (152-11594, 2 MiB), the user's dump matches MAME. Ribbit runs it on the
original Leapster's emulated hardware plus two TV-only devices. Everything below is VERIFIED-FW (read from
this BIOS and confirmed by running it); nothing comes from hardware documentation or measurements.

### Video: the TV chip at 0x2400_0000 (io/lmax_tv.rs)

* Same chip select and ID probe as the L-MAX (ID register +0x0C answers 0x0300_0000).
* Picture: 200x160 pixels of 16 bits at +0x2_0000, rows packed (400 bytes). Pixel format **YCbCr 5:6:5**:
  Cb bits 15-11, Y bits 10-5, Cr bits 4-0, BT.601 studio range. Derived from the BIOS's 4096-entry
  RGB444-to-TV table at 0x400D_94A4 (black 0x8090, white 0x8750, red 0x5A9D, green 0x3484, yellow 0x1692).
  Blue and magenta entries are lighter than textbook BT.601; shown as the table says.
* The BIOS converts the game's 12-bit LCD frame (format 4) through that table into the middle 160 columns
  (x = 20..179, loop at 0x4004_EA1E); the 20-pixel side borders take the colour in register +0x08.
* How the chip widens 5/6-bit fields to 8 bits is UNKNOWN (Ribbit zero-fills, so neutral chroma stays 128).
* Ribbit shows the 160x160 game window (frontends are LCD-sized); the border is not shown.

### Controller link: second UART at 0x0180_D600 (io/tv_link.rs)

* Same register layout as the debug UART at 0x0180_D500: +0x10 data, +0x14 status/control (bit 7 TX
  ready, bit 5 RX empty), +0x18 = 0xCF (about 4800 baud from 16 MHz). Interrupt handler 0x4005_5298 on
  vector 0x1C. Without the UART the BIOS hangs at 0x4005_581E draining a receive FIFO that never empties.
* One wire: the UART hears its own bytes and checks each echo. Two ports share it, chosen by
  0x0180_D700 bit 9 (port 0) or bit 8 (port 1). The BIOS polls both every 10 ms.
* Transaction: 0xFF, command, data; the controller answers ack + data whose bytes sum to 0.

| Command | Data sent | Answer | Meaning (from the BIOS's use of it) |
|---|---|---|---|
| AA | 00 10 46 | AC + 2 + sum | hello |
| A6 | 15 bytes | A3 + 24 + sum | pen settings: thresholds, debounce, simple-mode scale (stored at 0x4005_4FEE) |
| A5 | none | A5 25 + 36 + inner sum + sum | the controller's 37-byte memory: pen-pad calibration (affine map, 0x4005_5D50) |
| AC | 25 + 36 + inner sum + sum | A9 | write that memory back |
| 95 | none | 6A + 5 + sum | four 10-bit values (pad range) |
| A9 | none | A9 | ping |
| A3 | none | AA, or 0x5X packet | input poll |

* Poll packet: header nibble with even parity; bit 3 set = short form (header, buttons, sum), clear =
  long form with the pen (header, buttons, pressure, x, y, high bits, sum: 11-bit x/y, inverted pressure).
* Buttons: header bits 0-1 and data bits 0-7 map through the table at 0x400B_FA34 onto the handheld's own
  GIODataIn masks: Left, Up, Right, Down, Pause, A, B, Hint, Home, and 0x0004_0000.
* The pen hovers: low pressure means "in range of the pad" (2 polls at <= 0x7C0; out after 10 above 0x7F0).
  The BIOS tracks it as a pointer and posts pad events (0x1001_000A-C) for TV-native software.
* 0x0004_0000 (data bit 7) is the **click** (the mask the TV driver returns at 0x4005_0044): pressed while
  the pen is in range, the BIOS posts the stylus-down event that games use (0x1001_0002, at 0x4004_A550).
  Ordinary Leapster games therefore need the click, not just the pen.

### Ribbit's controller (HYPOTHESIS where the BIOS does not constrain it)

Answers on port 0 only; the hello, A6 and 95 contents are neutral values or the BIOS's own defaults; the
memory holds an ideal calibration (identity, so a pad position maps straight to the pixel). The stylus
becomes: pen in range, the click 3 polls later, and on lifting the click up 3 polls before the pen leaves
range. Transmission is instant (no baud timing).

### Other TV differences

* Straps: with bit 14 clear (the frontends' default) the TV BIOS boots to a blank calibration screen;
  Ribbit uses 0x63FF_FFFF for the TV (`straps::LEAPSTER_TV`).
* No GIODataIn reads: all buttons come from the controller.
* Tested: SpongeBob SquarePants: Saves the Day boots to its title, name entry by stylus (typed "ABB", OK),
  then the intro, all at full speed.

## The BIOS's Flash rasterizer, and HD rendering (2026-10-15)

Owner's decision: HD comes from the real BIOS's own vector data (an "HD layer"), not from a separate Flash
player. Evidence below is VERIFIED-FW on the US v2.1 BIOS (152-11265) by running it; other BIOSes have the
same code at other addresses (not yet located).

* The rasterizer is a scanline active-edge renderer working at **4x the LCD in each axis**: edge records hold
  16-bit device coordinates in quarter pixels (a full-screen rectangle is 0..640), scanlines are quarter
  scanlines, and the result is averaged down to 160x160 (anti-aliasing).
* Edge constructor 0x4004_E0D8 (r0 edge object, r1 edge record, r2 current quarter scanline). Steppers:
  lines 0x4004_DF64, quadratic curves 0x4004_DF78 (forward differencing, 16.16). Active edges are kept
  sorted by x (0x4004_EBF8); the scanline driver is around 0x4005_0656.
* Edge record: +0x08 / +0x0C fill-style pointers (two fills, Flash fill0/fill1 style), +0x10/+0x12 start,
  +0x14/+0x16 control, +0x18/+0x1A end (i16), +0x1C straight-line flag, +0x1D rule (selects the
  per-crossing handler), +0x1E direction.
* Fill style: +0x08 next, +0x0C depth (high half), +0x18 type (low byte 1 for the solid fills seen),
  +0x1C/+0x20 paint functions (solid: 0x4005_5728, 0x4005_544C), +0x24 colour as four u16: **B, G, R, A**,
  +0x34 the colour pre-packed in the LCD's 12-bit format.
* The BIOS only renders when something changes; one render pass ends with a framebuffer transfer.

Proof of concept: `crates/ribbit-core/examples/hdframe.rs` captures one pass (1,435 edges, 36 fills in the
SpongeBob intro), rasterizes it at 640x640 with even-odd toggling per fill and depth order, and matches the
BIOS's frame in shape, layering and colour, at 4x the detail. Not decoded yet: gradient and bitmap fills,
the rule byte, dirty-rectangle passes, clipping/masks, and the other BIOS revisions' addresses.

## Sequenced music: instruments, pitch bends (2026-10-15)

VERIFIED-FW on the US v2.1 BIOS (SpongeBob hub music), plus the LeapFrog sequence-format notes by
BLiNXthetimesweeperGOD (gist "LeapFrog Sequencer (Leap-Font)": MIDI-like tracks; command 0x8A "Pitch offset"
with bend and duration; program change 0xC0 selects cartridge instead of BIOS sounds).

* Instruments are A-law samples described by 20-byte records, e.g. at BIOS 0x401E_F25C: u32 sample start,
  u32 sample end, u16 loop start, u16 loop end (low halves of addresses), f32 root pitch (83.59 here),
  u16 unknown (0x4F40), u16 unknown (0x7FFF; the value written to 0x0180_4170/4184 below).
* Per note the driver writes for voice v: 0x0180_4184+4v and 0x0180_4170+4v (same value; record's last
  field), start 0x0180_40C4+8v, end/loop 0x0180_4104+8v, pitch 0x0180_415C+4v, volume 0x0180_413C+4v and
  again 0x0180_41A4+4v, then the start command. 0x4170/0x4184/0x41A4 are not in MAME's map: UNKNOWN
  (envelope? second channel?).
* **Pitch bends** are software: SetPitch (0x4003_06E0; voice, base pitch, offset with 0x80 = none, scaled
  through the table at 0x4010_0E5C) rewrites the pitch register about every 16 ms while the note plays
  (e.g. 0x9BB0 up to 0xBFC8 over 180 ms and back). So the hardware must apply pitch writes immediately;
  Ribbit now does (MAME latches pitch at note start, which plays bends flat).
* The intro cut scenes' soundtracks are LFC streams on voice 7 (one per scene), not sequenced notes.

## CPU timing, caches and clocks (2026-10-15)

Labels as above. Tools: `examples/mix.rs` (instruction mix and cache simulation from a save state),
`examples/scenes.rs` `lcdrate` (LCD frames per second).

### Caches (VERIFIED-FW geometry, US v2.1)

The BIOS locks lines into the caches with two routines that write the cache RAM-address registers directly:
instruction cache at 0x4003_8DF8 (aux 0x11 control, 0x1A address, 0x1B tag, 0x1C data, 0x19 and 0x13 line
operations) and data cache at 0x4003_8EC8 (aux 0x48 control, 0x58 address, 0x59 tag, 0x5A data, 0x49/0x4A line
operations; it polls 0x48 bit 8 while a line operation is busy). Both:

* mask the address with `~15`: **16-byte lines**;
* form the cache RAM address as `(way << 13) | (addr & 0x1FF0)`: **512 sets** (address bits 4-12) per way;
* try ways 3, 2, 1 for a free (tag bit 1 clear) line, else way 0: **4 ways**.

So each cache is **32 KB, 4-way set associative, 16-byte lines** (HYPOTHESIS only in that the full index range
is populated). Tag bit 0 reads as valid and bit 1 as locked. The replacement policy and the data cache's write
policy are UNKNOWN; the existence of flush operations (aux 0x4B) suggests write-back. Games (Cars, Sonic X) use
the same aux registers through their own copies of the routines. Ribbit does not model the caches yet.

### Clock control (VERIFIED-FW)

* BIOS CPU-clock function (US v2.1 0x4008_B940; copies in Cars 0x8003_8BB4 and Sonic X 0x8007_E538): reads aux
  register **0x56**. 0 gives 12 MHz; otherwise bits 2-3 select 0 = 96 MHz, 4 = 48 MHz, 8 = 24 MHz, else 12 MHz.
  No code writes aux 0x56, so it is a read-only hardware value. Ribbit returns IDENTITY (0x10), which reads as
  96 MHz. The only user found is a UART driver for a device at 0xFC1000 (clock source when that device is used).
* Boot (0x4003_9060, configuration table 0x400F_D378): 0x0180_9008 low 3 bits set to 1, then 5; 0x0180_9080 =
  0x1104; 10 ms wait; memory bank 2 configured (below); 10 ms wait; 0x0180_9008 = 1, then the table's value 0;
  0x0180_9080 = 0x1102. 0x0180_A000-A008 = 0x4981E0, 0x421122, 0x23 (meaning UNKNOWN). RAM is described by the
  same table as 0x3C00_0000-0x3C1F_FFFF (2 MB).
* At run time 0x0180_9008 low bits are 0 (MAME's commented-out code treats them as a divider, 96 MHz >> n). MQX
  idle tasks set them to 1 then 5 before sleeping (Cars 0x8003_9C5E), and every MQX interrupt-entry stub writes
  1 then 0 (Sonic X 0x8007_8262 onwards): a low-power clock while idle, full speed whenever code runs. It does
  not slow gameplay.

### Memory bank registers (VERIFIED-FW values, meaning HYPOTHESIS)

0x0180_C000-C010 are written 0xCAFE_00FF, 0xCAFE_1027, 0xCAFE_203F (at boot, from the table), 0xCAFE_3027
(0x4002_7F6A, with 0x0180_C024 = 0x80 then 0x30) and 0xCAFE_4027 (0x4002_81C8, with 0x0180_C020 = 0xBF, C01C = 1,
C020 = 0x58). The top half is a key, bits 12-15 the bank number, the low 12 bits presumably access timing.
Which address range each bank serves, and the field layout, are UNKNOWN.

### Pipeline costs the PRM documents (SOURCED: ARCompact PRM)

* Branches and jumps take effect one cycle late; without `.d` a taken branch wastes its delay slot.
* On the ARCtangent-A5, BRcc/BBIT branch a cycle later still: a second delay slot that is always nullified when
  taken.
* Loads take at least one cycle; using the loaded register in the next instruction stalls; latency depends on
  cache and memory.
* MUL64 is multi-cycle and stalls a read of its results; LR/SR are single-cycle; limm instructions are
  "single cycle immediate data".
* The PRM gives no per-instruction cycle table for the A5.

### Measured: Sonic X gameplay (Ribbit, 2026-10-15)

From a save state in Station Square act 1 (`mix`, 2 s, 192 M instructions at Ribbit's 1 cycle each):
99.8% of instructions fetched from the cartridge; loads 17.3% and stores 6.7% of instructions (almost all to
RAM); taken branches without `.d` 11.9% (BRcc/BBIT 8.0%); a load used by the next instruction 7.7%; long
immediates 3.5%; multiplies 1.6%. Simulated 32 KB caches: 0.79 instruction-cache misses per 1000 instructions,
2.7% data-cache misses. The game is CPU-bound: 49-53 LCD frames/s with no idle time.

### Real-hardware reference

"Sonic X Leapster Playthrough" (RescueHero's Nostalgia Zone, 2017, youtube.com/watch?v=ZywUhXukzRI) is a clean
digital capture at 29.97 fps, most likely of a **Leapster TV** (TV overscan border, the TV controller's pointer
on screen). Counting frames that differ from the previous one in 2-second windows of continuous motion gives
**17.4-17.9 game frames/s** in four separate stretches (2:25, 5:00, 10:00, 20:00). Ribbit (original-Leapster
mode, Sonic running right) gives 48-51. On the TV the picture goes through the external TV chip, probably at the
TV's 60 Hz (17.7 fits a mix of 3- and 4-field frames), so this bounds but does not equal the handheld's speed.
Needed next: the same measurement in Ribbit's Leapster TV mode, and a handheld reference.

### Like-for-like check and the two hypotheses (2026-10-15)

* Ribbit's **Leapster TV** mode (TV BIOS `am29pl160cb-90sf`, same cartridge, same act, running right): 43-45
  frames/s, against the real TV's 17.7. Ribbit is about **2.5x too fast** on the same hardware model. In the
  video, cutscenes (timer-paced at 8 frames/s in Ribbit) show at most 8-9 changes per second, so the method
  measures the game's own frame rate.
* Cache controls (VERIFIED-FW, US v2.1 0x4003_9244-0x4003_9364): IC_CTRL (aux 0x11) = 2 and DC_CTRL (aux 0x48) =
  0x8A, both with bit 0 (disable) clear, after invalidating both. The BIOS then locks the code ranges of
  resource 0x1000_000E into the instruction cache (routine 0x4003_8DF8) if IC_CTRL bit 0 reads 0. Sonic X scans
  48 instruction-cache tags (12 lines in each of the 4 ways) at start-up (0x8007_9236-0x8007_9266).
* **Hypothesis A: the CPU runs at 48 MHz** plus the PRM's pipeline costs. From the measured mix: CPI = 1 +
  0.119 (wasted delay slots) + 0.080 (the A5's second BRcc/BBIT slot) + 0.077 (load-use) + about 0.05
  (multiplies, cache misses) = about 1.32. Sonic X needs 1.81 M instructions per frame, giving 20 frames/s on the
  handheld and about 18 on the TV (Ribbit's TV/handheld ratio 44/49). **Measured: 17.7.**
* **Hypothesis B: 96 MHz with slow memory** needs a CPI of about 2.7. With the simulated miss rates (0.79
  instruction and 6.5 data misses per 1000 instructions), each miss would have to cost about 190 cycles.
  Implausible, unless large regions are uncached.
* A is supported by MAME's note that "/2 gives a closer speed/framerate to real hardware videos" and by the
  BIOS's clock function having an explicit 48 MHz mode (aux 0x56 bits 2-3 = 4). The board's actual aux 0x56
  value is UNKNOWN. To separate A from B: a second CPU-bound scene with a different memory profile, a handheld
  (non-TV) reference, or the real aux 0x56 value read on hardware.

### Correction, and pipeline timing in Ribbit (2026-10-15, later)

* **Hypothesis B is not implausible.** Nathan Farlow's leapster-emu (S10, below) models the memory system in
  detail. A cartridge line fill costs roughly 11 + 63 cycles plus about 144 cycles of bus strobes at the default
  chip-select setting. A first SDRAM data word from a line fill costs about 15 + 62. Loads land about 3 cycles
  after the access. With those costs, Sonic X's measured mix gives a CPI of roughly 2.3-2.5 at 96 MHz, which
  matches the video. Both A and B now fit the one measurement; B fits a detailed, apparently trace-derived
  model. Ribbit keeps 96 MHz (the nominal clock; S10 also uses 96 MHz with timers at 1/6) and treats the memory
  system as the open item.
* **Implemented**: `CpuConfig::pipeline_timing` (on for the Leapster) charges the PRM costs above (load-use +1,
  taken branch without `.d` +1, taken BRcc/BBIT +1 more). It is exact in both the single-step and block
  interpreters, with idle skipping and with save states (tests `pipeline_costs_load_use_and_taken_compare_branch`,
  `v15/v21_idle_skip_is_exact`, `v15/v21_save_state_is_exact`). Sonic X (handheld, running right): 49 -> 39
  frames/s. Real hardware is about 18-20. The remaining factor (~2x) is memory: cache misses, line fills,
  cartridge bus strobes, load latency beyond one cycle. None of it is modelled yet.
* Save-state format version 2 (UART timing and pipeline state). Version 1 states are refused with a clear
  message.
* `Machine::run_cycles` runs for emulated time. The app's pacing, library art capture and research tools use
  it, because one step no longer equals one cycle.

| # | Source | License | Use in Ribbit |
|---|---|---|---|
| S10 | Nathan Farlow, `leapster-emu` (github.com/nathanfarlow/leapster-emu, first seen 2026-10-15; C++, "a small, accurate Leapster emulator", "coming soon: tests against hardware traces") | **None stated** (all rights reserved) | Read for facts only (timing structure and magnitudes, cited above). No code or tables copied. Ask the author before using more |

### Memory timing in Ribbit, calibrated (2026-10-15, latest)

`io/memtiming.rs` models the memory system as stall cycles charged to the instruction that causes them
(`Bus::instruction` / `Bus::take_stall`; instruction-decoding reads use `Bus::fetch16` and are never timed):

* Instruction and data caches with the BIOS-derived geometry (4 ways x 512 sets x 16 bytes). The data cache is
  write-back and write-allocate (HYPOTHESIS). Replacement is round-robin advanced on fills only (UNKNOWN real
  policy, chosen so that hits never change state).
* CALIBRATED costs, per line fill: BIOS/cartridge ROM 200 cycles, SDRAM 80, on-chip RAM 40; a device-register
  access 40; a dirty write-back 42. Magnitudes follow S10.
* Load latency (`CpuConfig::load_latency`): a load's result arrives 3 cycles after it issues. Per-register
  ready times make an instruction wait only for registers it actually reads.
* Exactness: idle fast-forwarding only extrapolates an iteration that had no cache misses, and moves in-flight
  load results with the skipped time. A block whose last instruction stalls past a processor-timer limit
  advances the timers as single steps do. Save states (format 3) include both caches. Tests: `memtiming`
  unit tests, `v15/v21_idle_skip_is_exact`, `v15/v21_save_state_is_exact`; `examples/statediff.rs` checks run
  partitioning and restore.

**Result**: Sonic X gameplay (Station Square act 1, running right) on Ribbit's **Leapster TV** model: 16-18,
mean **17.0 frames/s**. The real Leapster TV capture: **17.7**. Handheld: 21-23. Emulation speed went from 192%
to 221% of real time (one P-core), since an emulated second now holds fewer instructions.

BIOS boot milestones moved later (`examples/milestones.rs`; the BIOS regression tests are re-pinned):

| | v1.5 | US v2.1 |
|---|---|---|
| LCD init (format 4) | 2.75 M cycles | 3.71 M |
| First voice command | 734 ms | 1023 ms |
| Calibration (format 3), default straps | 1.53 s | 1.75 s |

Open: second calibration points (other scenes, other games, a handheld reference); line prefetch; SDRAM row
effects; `.di` accesses; multiplier latency; DMA bus contention; the cartridge chip-select timing registers
(0x0180_C0xx) that should set the ROM cost per cartridge.
