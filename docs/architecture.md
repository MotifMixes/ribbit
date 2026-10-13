# Architecture

## Goals that shape the design

* **An accurate, deterministic core** that can run headless in CI and be compared trace-for-trace with other
  emulators.
* **Frontend independence.** The core never touches windows, GPUs, audio devices or files it was not handed.
* **Portability.** The same core builds for Windows, macOS, Linux, Android (AYN Thor: Snapdragon 8 Gen 2, Adreno 740,
  aarch64) and iOS.
* **Honesty.** Every unimplemented path stops and says so. Nothing is drawn or played that did not come from emulated
  hardware.

## Language: Rust

Rust 1.99 / Cargo were already installed (MSVC host). No C/C++ toolchain, CMake or Ninja is installed.
Research found no reason to prefer C++: the only mature CPU reference (MAME) cannot be reused as a component anyway
(cpu-core-options.md). Rust gives memory safety in a codebase full of address arithmetic, cross-compiles to Android
and iOS through rustup targets, and keeps the build to a single tool.

## Workspace layout

```
Cargo.toml                 workspace (edition 2024, rust-version 1.88)
crates/
  ribbit-memory/           memory map, SystemBus, Firmware/Cartridge images, NVRAM, known-dump hashes
  ribbit-cpu/              ARCompact state + decoder (execution: Phase 2)
  ribbit-video/            LCD registers, VRAM -> RGBA decoding
  ribbit-audio/            A-law, sample ring (voice engine: later)
  ribbit-input/            buttons -> GIODataIn, touch state
  ribbit-debug/            logging setup, hexdump, breakpoints, trace ring
  ribbit-core/             Machine: wiring, run loop, reset, render, config, image loading
  ribbit-desktop/          `ribbit` binary: CLI + headless runner + winit/wgpu window
docs/                      this documentation
```

Dependency direction (no cycles, no frontend crates below `ribbit-core`):

```
ribbit-desktop ──► ribbit-core ──► ribbit-cpu ──► ribbit-memory
       │              ├──► ribbit-video
       │              ├──► ribbit-audio
       │              ├──► ribbit-input
       │              └──► ribbit-debug
       └──► winit, wgpu, clap (frontend only)
```

`ribbit-core` and everything below it depend only on `thiserror`, `tracing`, `serde`/`toml`, `sha1` and `crc32fast`,
all pure Rust. They compile unchanged for `aarch64-linux-android` and `aarch64-apple-ios` (verified, see below).

## Core design

* **`SystemBus`** decodes ROM/RAM regions and records every unmapped access (address, width, kind, value), which
  turns boot failures into a list of addresses to investigate. Peripheral registers (`0x0180_xxxx`) are reported as
  `Region::Mmio`. In Phase 3, `ribbit-core` will wrap the bus in a `MachineBus` that routes MMIO to device models
  (interrupt controller, timers, UART, LCD, DMA, ADC, EEPROM, audio).
* **The `Bus` trait** is the CPU's only view of the world, so CPU tests use small synthetic buses.
* **`ribbit-core::io`** holds the on-chip peripherals (`timer`, `intc`, `uart`). For each step the machine
  builds a `MachineBus` (RAM/ROM via `SystemBus`, 0x0180_xxxx via `Peripherals`), then advances peripheral
  time by the step's cycles through `Clock` (CPU Hz to 16 MHz, exact remainder carry) and copies device
  interrupt outputs onto the CPU's level-sensitive IRQ lines. Unclaimed registers fall through to the
  unmapped-access log. Debug-only `stubs` can answer unmodelled reads for differential runs; they are off by
  default and labelled in every trace.
* **`Machine::run(max_instructions, breakpoints)`** is the single entry point. Frontends call it per frame; tests
  call it with exact budgets. It returns a `RunReport` that states why it stopped.
* **Time.** Phase 2 adds a scheduler keyed on CPU cycles (master clock 96 MHz nominal, timers at 16 MHz, DAC at
  32 kHz). Devices are advanced lazily from the scheduler. This avoids per-instruction polling and stays
  deterministic.
* **Determinism.** No wall-clock time, no host randomness and no threads inside the core. `Machine::state_digest`
  hashes architectural state so two runs can be compared.
* **Save states** are deferred until every device's state is serialisable and a restore-then-continue test matches
  an uninterrupted run (Phase 6).

## Frontend and graphics backend

**Chosen: winit 0.30 (windowing/input) + wgpu 30 (rendering).** cpal is planned for audio.

| Concern | Evidence |
|---|---|
| Desktop | Built and ran on this Windows 11 machine: the Vulkan backend was selected, the window opened, frames presented, and the title shows the CPU stop reason. |
| Android | wgpu uses Vulkan or GLES. winit supports Android through `android-activity` (`android-native-activity` / `android-game-activity` features). **A probe crate using the same winit/wgpu calls passed `cargo check --target aarch64-linux-android`.** Linking an APK needs the Android NDK and `cargo-ndk`/`xbuild`, which are not installed. |
| iOS | wgpu uses Metal and winit supports UIKit. **The same probe passed `cargo check --target aarch64-apple-ios`.** Building and signing need macOS + Xcode. |
| Limits | The renderer requests `Limits::downlevel_webgl2_defaults()` so GLES-only Android devices work. |
| Maintainability | Both crates are pure Rust, widely used and actively maintained. There is no C/C++ build step. |

Alternatives considered:

* **SDL2/SDL3 bindings:** need a C toolchain/CMake (absent) on every platform, and Android packaging goes through
  SDL's Java glue.
* **softbuffer:** simpler CPU blitting, but no shader path for LCD effects or scaling filters.
* **egui/eframe:** good for debugger panels later, and it sits on wgpu/winit, so it can be added on top without
  changing the backend.

The mobile frontend will be a separate crate (`ribbit-android`, later `ribbit-ios`) that reuses the wgpu
presentation code. To make that possible, the `Gpu` presenter in `ribbit-desktop/src/window.rs` should move into a
shared `ribbit-render` crate when mobile work starts. Nothing in the core depends on desktop-only APIs.

## Debugging facilities

| Facility | Now | Planned |
|---|---|---|
| Structured logs with per-subsystem targets (`ribbit::boot`, `ribbit::bus`, …), `RUST_LOG` filters, stderr output | ✓ | |
| Register dump (`Debug` for `Cpu`) | ✓ | |
| Memory hexdump | ✓ | interactive inspector |
| Unmapped-access log | ✓ | |
| PC breakpoints (`--breakpoint`) | ✓ (wired, needs CPU) | watchpoints, MMIO breakpoints |
| Trace ring (last N events) | ✓ (type exists) | instruction trace file, MAME-compatible format for diffing |
| Framebuffer dump (`--dump-frame out.ppm`) | ✓ | per-frame dumps, PNG |
| Boot milestones | first milestone (first instruction fetched) | UART log capture, named milestones |
| Interrupt tracing | | Phase 3 |

## Reuse and licensing

| Component | Use | License | Obligation |
|---|---|---|---|
| MAME `leapster.cpp`, `leapster_a.cpp`, `arcompact/*`, `bus/leapster/*` | Facts and behaviour (register addresses, formats, button bits). No code copied so far | BSD-3-Clause | If code is translated, keep the copyright notice and license text in that file and in a `THIRD_PARTY.md` |
| MAME `hash/leapster.xml` | Checksums in `known.rs` | CC0-1.0 | None |
| winit, wgpu, clap, tracing, serde, toml, sha1, crc32fast, thiserror, anyhow, pollster | Dependencies | MIT / Apache-2.0 | Standard notices in binary distributions |

**Project license: not chosen yet.** This is the owner's decision. Note that a permissive license (MIT/Apache-2.0)
keeps the option of embedding the core in mobile apps (App Store distribution conflicts with GPL/AGPL terms).

No firmware, game images or derived binary data are in the repository. `.gitignore` excludes `*.bin`, `*.rom`,
`*.nvram`, `*.sav`, `/local/` and `/roms/`.
