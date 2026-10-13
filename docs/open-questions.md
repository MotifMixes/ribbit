# Unresolved hardware questions and recommended experiments

Ordered roughly by how much they block progress.

| # | Question | Why it matters | Recommended experiment |
|---|---|---|---|
| Q1 | RESOLVED 2026-10-12: ARCompact PRM (5115-029) obtained; GNU toolchain installed. Remaining A5 ambiguities are listed in docs/isa-arcompact.md | | Hardware or MAME traces for the listed hypotheses |
| Q2 | RESOLVED 2026-10-12: user supplied all five original-Leapster BIOS dumps; all match MAME checksums | | |
| Q3 | Reset state (STATUS32, PC, vector base) and what lives in the first BIOS bytes | First instructions | With a BIOS: disassemble the vector table at 0x4000_0000 and compare with MAME's first 1000 traced instructions |
| Q4 | Behaviour of reads above 2 MiB of RAM, past the BIOS image, and in unmapped space | Firmware RAM sizing probes and bugs can depend on it | Trace the firmware's early RAM probe. On hardware, a test cart or JTAG read |
| Q5 | Real CPU speed (96 vs 48 MHz effective), wait states for ROM/SDRAM/cart, instruction timings | Frame rate, audio sync, timer-driven logic | Film the real console against a reference (e.g., time to reach the title screen; animation frame counts) and compare with emulator cycle counts. Best: measure a timer-vs-loop calibration in firmware via a test cart |
| Q6 | System EEPROM size (Ribbit 256 for a 24LC02B, MAME 0x200) and cart save size/type (Ribbit/MAME 0x800); controller latency; meaning of 0x0180_003C and flag bit 6 | Saves and calibration (protocol now modelled from the BIOS read routine) | Read the 24LC02B parts with an I²C reader (in-circuit or desoldered); log firmware EEPROM traffic in emulation |
| Q7 | Meaning of `0x0180_9004` straps on an **original** Leapster (MAME measured a Leapster 2) | Boot path: calibration, debug and frame-rate flags | Compare firmware branches on each bit. On hardware, probe the strap pins |
| Q8 | LCD controller: init sequence, VBlank/line timing, whether an IRQ exists, how the panel-type bits change behaviour | Tearing, frame pacing, games that wait on VBlank | Trace LCD register writes; look for a polling loop on an unknown status register |
| Q9 | Aux registers 0x10/0x11/0x1A/0x1B/0x47/0x48/0x4B | Startup writes 0x1000 words to 0x1A; unknown effect | Watch the firmware's use of them; correlate with display/audio behaviour |
| Q10 | Interrupt-line-to-vector mapping and level assignment for IRQs 0x10-0x1E | Interrupt-driven audio, timers, touch | Read `AUX_IRQ_LEV` and vector-table setup from the firmware |
| Q11 | ADC FIFO depth and touchscreen sample protocol (pressure semantics, timing) | Stylus accuracy is core to gameplay | Trace the BIOS touch driver. On hardware, scope the touch lines or log ADC values via a debug build |
| Q12 | Audio: interpolation vs sample repeat, analog filtering, mixer saturation, DAC rate options | Sound fidelity | Record real hardware audio of a known sample and compare spectra |
| Q13 | The ~2 MiB of non-SWF data at the start of the SpongeBob image | Determines whether carts contain native ARC code as well as Flash | Cross-reference cart directory pointers (tags 0x1005/0x1006/0x100C/0x2000) and watch which ranges the firmware reads |
| Q14 | "4Mb ATI chip" claim vs "no acceleration" | Whether a graphics core exists | Inspect PCB photos; check the firmware for accelerator register use |
| Q16 | Value of the straps register 0x0180_9004 on an original Leapster (MAME's value was measured on a Leapster 2) | Selects boot paths (ROM checksum, calibration, debug behaviour) | Read it on hardware (debug UART or a test cart); compare BIOS behaviour with each bit |
| Q17 | Timer roll-over at LIMIT (MAME) or one count after (ARC); timer reset values; timer 1/2 IRQ lines | 1-count timing differences; correctness of unused timers | Hardware timing of the MQX tick, or traces from MAME's development branch |
| Q18 | XY memory extension: Ribbit uses MAME's port semantics (post-modify, modulo, 16-bit mode) and memory size; r56 and the burst registers are unknown | Any code that computes with XY memory | Find ARC XY documentation; hardware test cart |
| Q19 | Real CPU clock: meaning of 0x0180_9008 (written 1 then 5), 0x0180_9080 (0x1104) and aux 0x56 | Instruction throughput per frame | Measure on hardware; trace what aux 0x56 reads after the clock writes |
| Q20 | ADC conversion behaviour: sample rate, no-touch/idle values, what channel 2 measures, the channel-control registers 0x0180_0090-009C and 0x0180_00A4 | Default boot waits for samples at the touch-calibration screen; the opt-in experimental model uses MAME's guesses | Logic-analyser capture of the ADC/touch lines; or log ADC values on a real original Leapster |
| Q21 | UART TX interrupt 0x1B timing/clear semantics; the device behind vector 0x18 (v1.5 handler 0x4008_00CC) | BIOS printf output; an unknown interrupt source | Trace the handlers; MAME's development branch; hardware UART capture |
| Q22 | Input register 0x0180_004C: what bit 11 (v1.5 SysMgr: active-low shutdown) and the other non-button bits are wired to, and their idle values | **Blocks v1.5**: reads 0 in Ribbit, so the BIOS powers off ~8 s after boot | Read the register on hardware (debug UART or test cart); trace the board (power switch, battery sense) |
| Q23 | Audio unit: acknowledge latency (Ribbit: immediate), voice status 0x0180_2078 and its interrupt (vector 0x15, flag bit 16), register map 0x0180_2000-4FFF | The acknowledge (flag bit 18 / line 0x1D) is modelled from firmware evidence; playback and status are not | Logic-analyser timing of IRQ 0x1D after a 0x0180_2070 write; MAME `leapster_a.cpp` for the register map |
| Q25 | System EEPROM 0x0180_0030-003C: protocol and contents (calibration, player name?) | Written by both BIOSes, unmodelled; with straps bit 14 set the BIOS skips calibration and must take it from somewhere | MAME's EEPROM handler as a reference; dump a real unit's 24LC02B |
| Q24 | Straps bit 14 on an original Leapster (0 = boot into touch calibration, per MAME) | Ribbit's default (MAME's raw Leapster 2 measurement) has it clear, so every boot calibrates; MAME's own default sets it | Observe whether a real unit calibrates on every power-on; read the register |
| Q15 | Hardware revisions and LCD panel variants among original Leapsters | Behaviour may differ per unit | Catalogue PCB markings and BIOS versions from several consoles |

## Dumping (for the user)

Only dump hardware you own. The BIOS ROM (U3) on the console board is a standard parallel ROM. Options:

* MAME documents the BIOS chip and its sizes. A TL866/T48-class programmer with a suitable adapter can read
  removed chips.
* In-circuit reading through the cartridge slot is not known to be possible. The slot exposes the cart bus, not the
  BIOS.

Ribbit will identify the result against the known checksums. An unknown checksum is not necessarily a bad dump; it
could be an undocumented revision, which would itself be a valuable finding.
