# Implementation plan and acceptance criteria

Every criterion must be demonstrated by an automated test, or by a documented reproducible procedure where
automation is impossible (e.g., needs the user's BIOS). Tests that need user images read them from environment
variables and skip when unset.

## Phase 0: Research and setup (done 2026-10-12)

- [x] hardware-research.md, architecture.md, accuracy-matrix.md, open-questions.md, cpu-core-options.md
- [x] User's cartridge identified locally (matches MAME `spongeba`)

## Phase 1: Executable skeleton (done 2026-10-12)

- [x] Workspace compiles; `cargo test --workspace` passes; `cargo clippy -- -D warnings` clean; `cargo fmt --check` clean
- [x] Structured logging (`tracing`, `RUST_LOG`), TOML config, CLI
- [x] Headless mode with exit codes; desktop window (winit + wgpu)
- [x] CI workflow (Windows, Linux, macOS)
- [x] Core type-checks for aarch64 Android and iOS

## Phase 2: CPU and memory (in progress; status 2026-10-12)

Done: decoder/disassembler validated against binutils (criterion 1); execution engine with 42 PRM-derived
tests (criteria 2-5, partially; see docs/isa-arcompact.md for the exact coverage); real BIOS executes
~19,700 instructions without a CPU error. Open: criterion 6 (trace comparison with MAME), DSP/XY extension.

Acceptance:

1. Decoder and disassembler agree with GNU `arc-elf32-objdump` on a generated corpus covering every major opcode
   and sub-opcode, including LIMM, short forms and delay-slot variants.
2. Per-instruction unit tests for results and every flag outcome (Z/N/C/V), including edge cases (0x8000_0000,
   carry across 32 bits, shift by 0/31/32).
3. Exception tests: illegal instruction to vector 2, interrupt entry/exit through ILINK1/2, priority, no interrupt in
   a delay slot, `RTIE` restoring STATUS32.
4. Zero-overhead loop tests: LP_COUNT 1, 0 and large; loop end at a branch.
5. Determinism: two runs of a fixed synthetic program give identical `state_digest` after N instructions.
6. With the user's BIOS: Ribbit executes the first 100,000 instructions with a PC/register trace identical to MAME's
   trace over the same span. Any divergence is investigated and documented, since MAME may be the one that is wrong.

## Phase 3: Boot diagnostics

1. MMIO routing with per-register logging of every unknown access (address, PC, value).
2. Interrupt controller, 3 timers, UART TX capture (written to a log file and shown in `headless` output), EEPROM
   interface, GIODataIn, system straps.
3. Boot milestones reported by `headless`: reset vector reached, RAM vector table copied, first UART line, MQX
   started, first LCD format write, first DMA to VRAM, cart header read at 0x8000_0000.
4. Debugger: instruction trace to file (MAME-diffable format), MMIO and IRQ trace, watchpoints, framebuffer dumps
   per N frames.
5. Acceptance: with the user's BIOS and no cart, Ribbit reaches "first UART line" and emits the same UART text as MAME.

## Phase 4: Graphics and input

1. LCD registers and DMA drive `ribbit-video`. `--dump-frame` shows the firmware's own touch-calibration screen
   (format 3) without any frontend drawing.
2. Golden-image tests: the framebuffer hash at a fixed instruction count matches a stored hash. The hash is stored,
   not the image, so no copyrighted pixels land in the repository.
3. GIODataIn reflects frontend button state. ADC FIFO touch protocol: calibration completes with scripted touches.

## Phase 5: SpongeBob

Milestones, each with a headless regression test keyed on instruction count and framebuffer hash:

1. Image loads (done). 2. CPU executes cart-initiated code. 3. Firmware recognises the cart. 4. Game initialises.
5. Title screen frame hash stable. 6. Scripted input reaches gameplay. 7. Controls and touch verified against a
video of real hardware. 8. Audio (voices, LFC, PCM), cart save round-trip, 1-hour soak test with no divergence
between two runs.

## Phase 6: Accuracy and portability

* Regression test for every bug fixed.
* Save states with a restore-and-continue equivalence test.
* CI on Windows, macOS and Linux. Android APK via `cargo-ndk` with a wgpu/winit `ribbit-android` frontend, tested on
  the AYN Thor (touch to stylus, gamepad to buttons). iOS: Xcode project, signing, MFi controller mapping.

## Phase 3 status (2026-10-12)

Done: `MachineBus`/`Peripherals` with the three timers (16 MHz), interrupt flag/enable registers and UART TX;
CPU interrupt integration; both BIOSes pass the 10 ms waits (v2.1 runs the MQX tick interrupt); MAME 0.289
differential check (identical control flow). Details: hardware-research.md, "Timers and peripheral clock".

## Phase 4 status (2026-10-12)

Done, each BIOS tested on its own (`tests/user_bios.rs`): XY memory extension (r32-r55, aux 0x80-0x9F, MAME
semantics, flagged); ADC FIFO status/data; system straps (MAME's Leapster 2 value as a configurable default) and
the clock-control latch; LCD controller registers; framebuffer DMA with its completion interrupt (line 0x12).
Universal v1.5 and US v2.1 both initialise the LCD, run framebuffer DMA, take all four observed interrupt sources
and switch to format 3 (MAME: touch calibration) without a CPU error. Neither draws a visible screen yet: VRAM stays
zero in format 3 while the BIOS waits for ADC samples. Phase 4 item 1 (`--dump-frame` showing the calibration
screen) is therefore **not** met. Details: hardware-research.md, "Phase 4".

## Phase 4b status (2026-10-13)

Done: opt-in **EXPERIMENTAL** MAME touch model (`--experimental-mame-touch`, `--touch-press`), coverage and
first-cycle diagnostics, the MUL64 A-field decoder fix, a full binutils sweep of v2.1. With the model, both BIOSes
complete their five-point calibration. v2.1 then shows its start-up screens and calls the Flash player on the
cartridge (`PlayMovie("Packed RIB Movie(s)", 0x8000_0000)`); v1.5 requests power-off at ~8 s because input
register 0x0180_004C reads 0. Nothing of SpongeBob is displayed and no game logic is known to run.

## Phase 4c status (2026-10-13)

Done: root cause of the "sound stall" (AudioTask waiting for the voice-command acknowledge, vector 0x1D); the
acknowledge is modelled (flag bit 18 / line 0x1D, firmware-verified routing, immediate latency); GPIO input
0x0180_004C with MAME's button layout (non-button bits unresolved); desktop: frog icon embedded in the exe,
Smooth/Pixel display (F), mouse as stylus when the experimental touch model is on. With the experimental touch
model, US v2.1 + SpongeBob reaches a name-entry keyboard, accepts taps, and shows what is presumably the game's
intro artwork. Details: hardware-research.md, "Phase 4c".

## Phase 4d status (2026-10-13)

Done: system EEPROM and cartridge save chip (completion flag bit 7, device codes from the BIOS table, save files),
the voice engine with LFC speech (MAME model), `--cpu-mhz`. SpongeBob on US v2.1 now reaches its main menu and
loads an activity; its profile is saved and read back. Details: hardware-research.md, "Phase 4d".

## Phase 4e status (2026-10-13): real time and sound

* Speed: exact idle-loop fast-forwarding (one iteration is probed; if CPU state except time, memory and devices
  are unchanged and only side-effect-free registers were read, time jumps to just before the next device or CPU
  timer event). Verified bit-identical by state digests with it on and off (`*_idle_skip_is_exact`) and by an
  identical SpongeBob frame. US v2.1 without a cartridge: 2.0x real time; SpongeBob in its menus: about 1.3x.
  Also: smaller step results (last instruction kept in the CPU, boxed error text).
* Desktop: real-time pacing (title shows % speed), host audio output (cpal; 8 kHz voice mix resampled linearly),
  save files via config. Local play config: `local/play.toml` (git-ignored).

## Phase 4f status (2026-10-13): usable app

* Save states (exact, verified per BIOS), 5 slots, rewind; controllers on desktop (gilrs); display modes Sharp
  (default) and LCD grid alongside Pixel / Smooth; MAME's 32 kHz output interpolator (host side); opt-in
  experimental settings for the v1.5 power line and the CPU clock; release zip script.
* SpongeBob on US v2.1: activities load and respond; 6-minute soak clean. v1.5 needs the experimental workaround.
* Android: builds, untested on a device; paused at the owner's request.

## Next task

1. **Play-test SpongeBob** further (the remaining activities, long sessions); fix what breaks (owner feedback).
2. **Sound accuracy**: compare against hardware recordings; volume levels; PCM DMA when a title uses it.
3. **v1.5 shutdown line** (GPIO bit 11): hardware evidence (Q22); the MAME workaround stays opt-in until then.
4. **Accuracy debts**: CPU timing model (1 cycle/step placeholder), touch model (experimental), straps default
   (Q24), EEPROM sizes (Q6).
5. Android: first run on a device (when the owner resumes it).

## Later, optional tracks (owner's decision)

* **BIOS-free mode.** Games are SWF v5 content run by the BIOS's Flash player. A replacement (e.g. built on
  Ruffle, MIT/Apache, Rust) could run carts without a BIOS dump, but only after the LeapFrog-specific
  services the games call have been identified by tracing the real BIOS in Ribbit. Must be a separate,
  clearly labelled mode; the accurate path keeps using the user's BIOS.
* **Static analysis.** Ghidra (an ARCompact SLEIGH module exists, SSTIC 2021) for decompiling the BIOS to
  map its hardware accesses. Complements, does not replace, execution traces.
* **Other Leapster models** (L-MAX, TV, Leapster 2): later. Firmware identification already distinguishes them.
