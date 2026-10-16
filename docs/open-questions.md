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
| Q11 | Partly RESOLVED 2026-10-16: the touch protocol (pins, inputs, the six measurements, pen-up rule) is read from the drivers (hardware-research.md, "ADC, touch panel and board lines"). Open: FIFO depth, conversion rate, panel resistances and orientation | Stylus feel and timing | Scope the touch lines on hardware; log ADC values on a real unit |
| Q12 | Audio: interpolation vs sample repeat, analog filtering, mixer saturation, DAC rate options | Sound fidelity | Record real hardware audio of a known sample and compare spectra |
| Q13 | The ~2 MiB of non-SWF data at the start of the SpongeBob image | Determines whether carts contain native ARC code as well as Flash | Cross-reference cart directory pointers (tags 0x1005/0x1006/0x100C/0x2000) and watch which ranges the firmware reads |
| Q14 | "4Mb ATI chip" claim vs "no acceleration" | Whether a graphics core exists | Inspect PCB photos; check the firmware for accelerator register use |
| Q16 | Value of the straps register 0x0180_9004 on an original Leapster (MAME's value was measured on a Leapster 2) | Selects boot paths (ROM checksum, calibration, debug behaviour) | Read it on hardware (debug UART or a test cart); compare BIOS behaviour with each bit |
| Q17 | Timer roll-over at LIMIT (MAME) or one count after (ARC); timer reset values; timer 1/2 IRQ lines | 1-count timing differences; correctness of unused timers | Hardware timing of the MQX tick, or traces from MAME's development branch |
| Q18 | XY memory extension: Ribbit uses MAME's port semantics (post-modify, modulo, 16-bit mode) and memory size; r56 and the burst registers are unknown | Any code that computes with XY memory | Find ARC XY documentation; hardware test cart |
| Q19 | Real CPU clock: meaning of 0x0180_9008 (written 1 then 5), 0x0180_9080 (0x1104) and aux 0x56 | Instruction throughput per frame | Measure on hardware; trace what aux 0x56 reads after the clock writes |
| Q20 | Partly RESOLVED 2026-10-16: slot controls (run, mode, input), slot 2 = battery on input 6, pen-up values. Open: the conversion rate (Ribbit: 0x0180_00A4 bits 8-19 + 1 ticks, HYPOTHESIS), the modes, inputs 0/5/7, the battery scale | Interrupt load, battery level | Logic-analyser capture of the ADC/touch lines; log ADC values on a real unit |
| Q21 | (UART TX interrupt modelled 2026-10-15 from the MQX driver: level, control bit 6, 19,231 baud; real baud formula unconfirmed.) The device behind vector 0x18 (v1.5 handler 0x4008_00CC) | BIOS printf output; an unknown interrupt source | Trace the handlers; MAME's development branch; hardware UART capture |
| Q22 | RESOLVED 2026-10-16 (behaviour): bits 30 (battery OK), 12 (ordinary batteries) and 11 (v1.5 power-off request, active low) come from the battery-monitor resource; they read 1 on a running console. Open: the electrical source of each, and the other non-button bits | | Trace the board |
| Q23 | Audio unit: acknowledge latency (Ribbit: immediate), voice status 0x0180_2078 and its interrupt (vector 0x15, flag bit 16), register map 0x0180_2000-4FFF | The acknowledge (flag bit 18 / line 0x1D) is modelled from firmware evidence; playback and status are not | Logic-analyser timing of IRQ 0x1D after a 0x0180_2070 write; MAME `leapster_a.cpp` for the register map |
| Q25 | System EEPROM 0x0180_0030-003C: protocol and contents (calibration, player name?) | Written by both BIOSes, unmodelled; with straps bit 14 set the BIOS skips calibration and must take it from somewhere | MAME's EEPROM handler as a reference; dump a real unit's 24LC02B |
| Q24 | Straps bit 14 on an original Leapster (0 = boot into touch calibration, per MAME) | Ribbit's default (MAME's raw Leapster 2 measurement) has it clear, so every boot calibrates; MAME's own default sets it | Observe whether a real unit calibrates on every power-on; read the register |
| Q26 | Clock of the delay counters 0x0180_0010/0014/0018 (write a count with bit 7 set, poll until bit 7 clears; every BIOS uses them for LCD reset and NAND waits) | Unmodelled: they read 0, so those delays take no time | Measure on hardware; find a BIOS timing check |
| Q27 | Leapster 2 SD host controller timing and error behaviour, and which NAND chip the CIP3 board carries | Needed for a virtual SD card; NAND ID 0x98 0xD1 vs 0x20 0xF1 | A real CIP3 board; an SD card trace |
| Q28 | How the L-MAX US v2.1 BIOS senses a TV on its jack | Ribbit cannot offer the L-MAX TV picture without v2.1 thinking a TV is plugged in | Trace `LF_GetTvPlugState` (v2.1 0x4007_6EB4 → TV MPI table) to the register it reads; a real L-MAX with and without a TV |
| Q29 | Leapster 2 LCD colours: real units look blue where Ribbit's 12-bit decode (MAME's) shows purple, green and yellow (warning screen, LeapFrog logo) | Every Leapster 2/L-MAX picture | A straight-on daylight photo of a known screen; see hardware-research.md, "Leapster 2 real-hardware reference" |
| Q30 | Size of the Leapster 2's SD card (modelled 1 GiB; B5's format parameters require more than 512 MiB) | Free space shown to games; nothing else found | A real 0.6/B5 unit's card |
| Q15 | Hardware revisions and LCD panel variants among original Leapsters | Behaviour may differ per unit | Catalogue PCB markings and BIOS versions from several consoles |

## Leapster TV (experimental): guesses to resolve

Everything the TV BIOS checks is modelled from its code (docs/hardware-research.md, "Leapster TV"). These
are the parts Ribbit had to guess, because the BIOS accepts any value or never shows what hardware does.
Code: `crates/ribbit-core/src/io/tv_link.rs`, `io/lmax_tv.rs`.

| # | Guess (what Ribbit does now) | Why it matters | How to find out |
|---|---|---|---|
| T1 | Controller hello reply `AC 00 00`: the 2 bytes may be a device type or version | A TV-native app or another controller type may branch on them | Find where the BIOS reads the AC reply (A6 is queued after it, 0x4005_4FC8); a capture from a real controller |
| T2 | A6 reply: thresholds 0x7C0/0x7F0 and debounce 2/10/10/10 are the BIOS's own defaults; scale bytes 16/0/16/0, movement bytes 13-14 = 0 and bytes 15-24 = 0 are invented | Pen feel (when a touch registers, drag thresholds); bytes 15-24 go to a struct at port +0xAC when one exists | Trace the readers of port state 0xC4-0xCD and 0xAC; real controller capture |
| T3 | Controller memory (A5/AC): an ideal identity calibration. A real unit stores its own pad calibration; bytes 0x52-0x53 and 0x74-0x75 are unknown (0) | Absolute pen accuracy; what the BIOS's calibration screen (mode 1, routine 0x4005_5FA8) writes | Run the BIOS calibration flow (force mode 1) and read the block it writes back with AC |
| T4 | 95 reply: pad range 0x040-0x3C0 per axis | Used by the BIOS's simple pen mode and calibration, not mode 3 | Trace the readers of port state 0x2C-0x48 |
| T5 | Pen "pressure": Ribbit sends 0 for in range. Real meaning (proximity, height, real pressure?) unknown | Hover behaviour; TV-native drawing apps | Find a Leapster TV drawing title; hardware capture |
| T6 | The stylus becomes pen-in-range then click 3 polls later; lift = click up, pen out of range 3 polls later. No hover without the mouse button | Taps and drags in games; there is no on-screen pointer when hovering | Let the frontend send mouse position while not clicking (hover) and see if the BIOS draws a pointer; check how the real pen/click is wired (tip switch or button) |
| T7 | Controller only on port 0; port 1 silent | Two-player or a second device (the console's own panel?) may exist | Trace what the BIOS does with each port's device type (dispatcher 0x4004_94C0 requires type 6) |
| T8 | Transmission is instant (no 4800-baud timing); UART IRQ = control bit 6 (TX) or bit 2 with data waiting (RX) | Timing-sensitive code, interrupt load | UART datasheet/compare with the 0x0180_D500 UART; hardware timing |
| T9 | GIODataIn bit 0x800 (display mode toggle at 0x400A_AE46) is never produced; display mode stays 1 (pad mode) | Mode 0 sends pointer events directly; what physical control sets it is unknown | Find which controller or console input sets 0x800 |
| T10 | TV pixel widening: 5/6-bit fields zero-filled to 8 bits; BT.601 studio-range conversion | Exact colours (red decodes to F0 06 00) | ADV7179 datasheet and the TV chip's wiring; capture real TV output |
| T11 | Only the 160x160 game window is shown; the 200x160 frame's border (register +0x08) and the TV's aspect ratio are dropped | Faithful TV look | Decide display policy; check registers +0x00-+0x20 for position/size |
| T12 | Straps 0x63FF_FFFF for the TV (bit 14 clear boots to a blank calibration screen) | Boot path | What the blank screen is (pad calibration?); real unit's straps |
| T13 | TV chip registers +0x00-+0x20 other than the ID: unmodelled plain memory; +0x18 is a serial port to the encoder | Display mode bits (the mode setter flips +0x00 bits 0-1) | Trace writes; ADV7179 register map |
| T14 | Sound on the TV uses the handheld's audio model unchanged | Audio correctness | Compare TV BIOS audio register use with the handheld's |
| T15 | Only SpongeBob tested | Compatibility | Run the automated check over the library with the TV BIOS |

## Dumping (for the user)

Only dump hardware you own. The BIOS ROM (U3) on the console board is a standard parallel ROM. Options:

* MAME documents the BIOS chip and its sizes. A TL866/T48-class programmer with a suitable adapter can read
  removed chips.
* In-circuit reading through the cartridge slot is not known to be possible. The slot exposes the cart bus, not the
  BIOS.

Ribbit will identify the result against the known checksums. An unknown checksum is not necessarily a bad dump; it
could be an undocumented revision, which would itself be a valuable finding.
