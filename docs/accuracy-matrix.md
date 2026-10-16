# Accuracy matrix

Status of each component **in Ribbit**, separate from what is known about the hardware.

Legend:

* **Not started**: no code exists.
* **Skeleton**: types and plumbing only; does nothing observable.
* **Partial**: some behaviour is implemented and unit-tested; gaps are listed.
* **Matches reference**: agrees with MAME/spec under tests, but is unverified on hardware.
* **Hardware-verified**: agrees with traces or measurements from a real original Leapster.

No component is hardware-verified. **Ribbit does not emulate the Leapster yet.** Both original-Leapster BIOSes
(Universal v1.5, US v2.1) boot their RTOS, initialise the LCD, run framebuffer DMA and switch to the
touch-calibration display format, then wait for ADC samples that Ribbit does not generate by default. The screen
is black. With the opt-in **EXPERIMENTAL** MAME touch model (or straps bit 14 set) both get past calibration; v2.1
then hands the cartridge to its Flash player, v1.5 powers itself off (unmodelled input line). Per-BIOS details:
hardware-research.md, "Phase 4" and "Phase 4b".

| Component | Ribbit status | Basis | Tests | Known gaps / risks |
|---|---|---|---|---|
| Firmware loading/validation | Partial | MAME ROM hashes | `firmware::tests` | Unknown dumps accepted, flagged unverified |
| Cartridge loading/validation | Partial | MAME softlist sizes/hashes | `cartridge::tests`, `tests/user_images.rs` | |
| Cartridge header directory | Partial | Ribbit's analysis of one image | synthetic + real-image test | Inferred layout; other titles unchecked |
| Memory map (ROM/RAM/VRAM/cart) | Partial | MAME + LeapFrog-Tools + cart pointers | `bus::tests` | RAM above 2 MiB, BIOS-window tail, open-bus value all HYPOTHESIS |
| Misaligned-access rule | Partial | MAME (tested on Leapster 2) | `misaligned_accesses_ignore_low_bits` | Unverified on the original |
| Peripheral bus (MMIO routing) | Partial | — | `io::tests` | Timers, interrupt flags, ADC FIFO, straps, clock latch, LCD/DMA, UART modelled; everything else logged and reads 0 |
| Leapster timers 0-2 (0x0180_D000 block) | Partial | MAME + BIOS code (16 MHz, IRQ 0x19) + ARC timer semantics | `io::timer::tests`, `tests/user_bios.rs` | Roll-over at vs. after LIMIT; reset values; timer 1/2 IRQ numbers SOURCED only |
| Interrupt flags/enable (0x0180_0080/84) | Partial | MAME + BIOS code | `io::intc::tests` | Framebuffer DMA raises bit 14 (line 0x12); ADC bit 8 never raised; MAME's bit 7 not reproduced |
| System straps (0x0180_9004) | Partial | MAME Leapster 2 measurement + BIOS code (LCD panel select) | `io::straps::tests`, `*_zero_straps_reports_missing_lcd_config` | Original-Leapster value UNKNOWN; default 0x63FF_BFFF, overridable |
| Clock control (0x0180_9008) | Partial (latch) | MAME + BIOS code | `clock_register_reads_back_what_was_written` | No effect on timing; 0x0180_9080 and memory controller 0x0180_C000 not started |
| ADC (0x0180_0090-00AB) | Partial (FIFO status/data only) | MAME + BIOS drain loops and ISR | `io::adc::tests` | **Current blocker**: no conversions or ADC interrupt; channel controls 0x90-0x9C and 0xA4 unmodelled |
| CPU state/registers | Partial | PRM ch. 3 | `tests/exec.rs` | Reset state per PRM Fig. 83; IDENTITY value UNKNOWN |
| CPU decoder + disassembler | Matches reference | PRM ch. 6-7, GNU objdump | `tests/objdump_oracle.rs` (277 corpus, 23,708 random; full sweeps of v1.5, 764,950 lines, and v2.1, 3,162,386 lines) | Documented disagreements in isa-arcompact.md (incl. MUL64 A field, SWI/BRK operand field) |
| CPU execution | Partial | PRM ch. 9 | 43 execution tests on GNU-assembled programs | No independent execution oracle yet (MAME/hardware traces); DSP/XMAC instructions, DIVAW, RND16 unsupported |
| XY memory extension (r32-r55, aux 0x80-0x9F) | Partial | binutils names + MAME semantics + BIOS context switch | `xy::tests`, `tests/exec.rs` | Semantics SOURCED from MAME only; burst transfers (aux 0x9B) unsupported; memory size from MAME |
| Interrupts/exceptions | Partial | PRM ch. 4 | entry/return, priority, masking, SWI, sleep wake | Leapster sources: timers 0x19/0x1A/0x1E, DMA 0x12, voice acknowledge 0x1D, ADC 0x10 (experimental model only); UART 0x1B, voice status 0x15, vector 0x18 device not raised |
| ARC processor timers 0/1 | Partial | PRM | timer 0 IRQ test | Leapster MMIO timers (0x0180_D000) not started |
| UART (debug console) | Partial | MAME + BIOS putc | `io::uart::tests` | TX captured, status always ready; TX interrupt 0x1B not raised, so the BIOS's interrupt-driven printf sends only one byte |
| System EEPROM (device 0x26) | Partial | BIOS read routine + MAME | `io::eeprom::tests` (6), `v15/v21_eeprom_reads_complete_*` | Completion via flag bit 7 (firmware); 256 bytes (24LC02B) vs MAME 0x200 (Q6); erased 0xFF HYPOTHESIS; save file `--eeprom` |
| Cartridge save chip (device 0x20) | Partial | BIOS use + MAME | `cart_chip_is_device_0x20_only_when_inserted`, `v21_spongebob_name_entry_is_saved_to_the_cartridge` | 0x800 bytes (MAME, type unknown); present whenever a cartridge is inserted; save file `--cart-save` |
| LCD decode formats 3/4 | Matches reference | MAME `screen_update_leapster` | `ribbit-video` tests | Driven by the emulated LCD registers |
| LCD controller registers (base/stride/format) | Partial | MAME + BIOS `LcdMPI::Init` | `io::lcd::tests`, `tests/user_bios.rs` | Panel timing (0x8004-802C) and dither tables (0x8180-81BC) stored nowhere, not modelled |
| LCD timing / VBlank | Not started | — | — | UNKNOWN in all sources |
| Framebuffer DMA | Partial | MAME + BIOS `CopyToScreen`/ISR | `io::lcd::tests`, `framebuffer_dma_copies_to_vram_and_raises_line_0x12`, `tests/user_bios.rs` | Instant copy; transfer time and error status UNKNOWN; only command 0x1B executed |
| Buttons (GIODataIn bits) | Matches reference | MAME input ports | `ribbit-input` tests | Mapped to 0x0180_004C (row below) |
| Touchscreen / ADC | Skeleton (`Touch`) | — | — | Sample format known from the BIOS ISR (channel in bits 17:16, value in 15:5); rate, idle values and pressure UNKNOWN; MAME's model is guesswork |
| **EXPERIMENTAL** touch sample source (`--experimental-mame-touch`, off by default) | **Experimental, unverified** (not a status level: reproduces another emulator's guesses) | MAME master da56c5ac `leapster.cpp` | `io::touch::tests` (6), `experimental_touch_*` machine tests (2), `v15/v21_experimental_touch_*` BIOS tests | Rate, sample order, pressure values, idle position, channels 1-3 all MAME guesses; reports and logs label every run that uses it |
| Input register 0x0180_004C (buttons, power/shutdown line) | Partial | MAME (button bits) + both BIOSes' pollers; v1.5 SysMgr (bit 11 = active-low shutdown) | `io::gpio::tests`, `v15/v21_buttons_read_released_*` | Buttons active-low from the frontend; all other bits unresolved, read 0. **Blocks v1.5**: power-off at ~4 s. Opt-in **experimental** MAME workaround (all unresolved bits 1 = 0x48FC1C7F; `--input-lines`, GUI Emulation → Advanced): v1.5 + SpongeBob then reaches name entry (30 s run). Not adopted as default: no hardware evidence |
| Audio unit: voice-command acknowledge (0x0180_2070 → flag bit 18 → line 0x1D) | Partial | BIOS audio driver (both) + MAME | `io::audio::tests`, `v15/v21_voice_command_is_acknowledged_*` | Latency UNKNOWN (immediate). Unblocked AudioTask and v2.1's PlayMovie |
| Audio voice engine: voices 0-7, LFC speech, status 0x0180_2078 | Partial | MAME `leapster_a.cpp` (researched, unverified) + BIOS register use | `io::voices::tests` (5), `*_startup_voice_plays` | 8 kHz mix; the host output raises it to 32 kHz with MAME's 64-tap interpolator (`upsampler_passes_dc_and_delays_by_about_1ms`; whether hardware interpolates is UNVERIFIED), then resamples linearly to the device rate. PCM DMA (0x0180_3000, vector 0x13) not modelled: neither BIOS nor SpongeBob accesses it (200 s soak). Vector 0x15 not modelled; levels unverified |
| Idle-loop fast-forwarding | Exact (speed only) | Probe + state comparison | `v15/v21_idle_skip_is_exact` (digest, cycles, instructions equal with it on/off) | Disabled while tracing or with breakpoints; diagnostics counters (MMIO read counts) differ |
| Power control 0x0180_B000 | Not started | v1.5/v2.1 SysMgr | `power_off_requested` in BIOS tests | Bit 8 = power-off request (HYPOTHESIS); writes are only reported |
| Audio A-law decode | Matches reference | ITU-T G.711 | `alaw_*` tests | Leapster use of standard A-law is SOURCED, not verified |
| Audio output buffering | Partial (`SampleRing`) | — | `ring_counts_overruns_and_underruns` | Host output via cpal; queue capped at 0.25 s |
| Clock/timing model | Partial | BIOS constants | `io::tests::clock_converts_without_drift`, `clock_fast_path_matches_wide_arithmetic` | Peripheral clock 16 MHz (firmware evidence); CPU 96 MHz nominal; PRM pipeline costs per instruction (2026-10-15), memory system not yet modelled (real Sonic X is ~2x slower still; MAME uses 48 MHz to match videos). Speed: ~30 M steps/s raw, 2.0x real time with exact idle-loop fast-forwarding (about 1.3x in SpongeBob's menus) |
| CPU vs MAME 0.289 (real BIOS) | Matches reference (control flow) | MAME trace | `scripts/compare-mame-trace.py` | 3,206 + 750 PCs identical order; one STATUS32 discrepancy attributed to MAME 0.289 |
| Save states | Exact | Whole-machine serialisation (CPU, memory, devices; ROMs excluded and checked by CRC) | `save_state_round_trips_and_checks_images`, `v15/v21_save_state_is_exact` (continuation identical to an uninterrupted run) | Format version 1; states from other builds are refused, not converted. Rewind uses the same snapshots |
| Desktop app (`ribbit-gui`) | Partial | — | manual runs, screenshots | egui menus, settings, file dialogs, autosave, 5 state slots + rewind, controllers (gilrs), display modes Sharp / LCD grid / Pixel / Smooth (display only) |
| Headless runner | Partial | — | manual runs, exit codes | |
| Android | Builds | — | `cargo ndk` Clippy, `scripts/build-apk.py` (signature verified) | **Never run on a device**; paused at the owner's request |
| iOS | Not started | type-check of core | `cargo check` only | Xcode not available |

## Compatibility

| Title | Image | Status |
|---|---|---|
| SpongeBob SquarePants: Saves the Day (USA, alt), 500-11002 | Identified (CRC32 `0f2ad725`) | **Playable on US v2.1 with the experimental touch model**: name entry, intro, main menu; all four activities load; Krabby Patty gameplay responds to the d-pad; profile saved to and read back from the cartridge chip; sound (MAME model); ~1.3x real time; 360 s soak with no CPU error. UK/Spanish 2.x BIOSes behave like US v2.1; the German dump (bad) never draws its screen. v1.5 powers off at ~4 s (GPIO bit 11) unless the experimental workaround is on, then reaches name entry |

### BIOS boot status (no cartridge, default straps; tests in `crates/ribbit-core/tests/user_bios.rs`)

| BIOS | Furthest milestone | Stop | Tested by |
|---|---|---|---|
| Universal v1.5 (`155-10072-a`) | Default: LCD init (~2.07 M cycles), 1 framebuffer DMA, format 3 (~145.9 M). **Experimental** touch: calibration completes | Default: waits for ADC samples. Experimental: SysMgr power-off at 8.02 s (input 0x0180_004C bit 11 unmodelled); no CPU error | `v15_*` (4 tests) |
| US v2.1 (`152-11265`) | Default: LCD init (~2.17 M cycles), 3 DMAs, format 3 (~156.6 M). **Experimental** touch: calibration, start-up screens, PlayMovie on the cartridge at 14.14 s | Default: waits for ADC samples. Experimental: idle after PlayMovie (audio unit suspected); no CPU error | `v21_*` (3 tests) |
| UK / German / Spanish 2.x | Not tested | — | — |
