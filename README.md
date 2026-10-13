# Ribbit

An emulator in development for the **original 2003 LeapFrog Leapster** (the blue-and-silver handheld).

> **Status: Ribbit does not emulate the Leapster yet.** The ARCompact CPU core executes (decoder validated against
> GNU binutils; control flow matches MAME on real firmware). Both original-Leapster BIOSes (Universal v1.5, US v2.1)
> boot their RTOS, initialise the LCD, run framebuffer DMA and switch to the touch-calibration display mode, then
> wait for touchscreen ADC samples, which are not emulated by default. An opt-in, clearly labelled
> **experimental** touch model borrowed from MAME (`--experimental-mame-touch`) gets both past calibration. With
> the EEPROMs and the voice engine modelled, SpongeBob on the US v2.1 BIOS runs through name entry, its intro and a
> cut-scene to the main menu and loads an activity, at full speed with sound; saves persist. Not yet play-tested
> beyond that.
> See [docs/accuracy-matrix.md](docs/accuracy-matrix.md).

Leapster games are Flash content run by a player built into the console firmware. To run games you will need a
dump of the **BIOS from your own console**, as well as your cartridge. No firmware or game data is included or will
be accepted into this repository.

## Build

Requires Rust 1.88+.

```sh
cargo test --workspace
cargo run -p ribbit-desktop -- --help
```

## Playing

* **Windows**: run `target/release/ribbit-gui.exe` (or `ribbit run`). Use File → Open BIOS… once, then
  File → Open Cartridge… (or drop files onto the window). Settings, saves and screenshots go to `%APPDATA%\Ribbit`.
  Mouse = stylus (experimental touch model), arrows/Z/X/H/Enter/Backspace = Leapster buttons (Input → Key
  bindings…), Esc = pause, Tab = fast-forward, F11 = fullscreen, F12 = screenshot.
* **Android** (arm64): `python -I scripts/build-apk.py` builds `target/android/ribbit-debug.apk` (needs the Android
  SDK/NDK and `cargo-ndk`). On first launch, grant "All files access" so the in-app browser can open your files.
  Touch = stylus; gamepad A/B, X/Y = Hint, Start = Pause, Select = Home; Back = menu.

## Documentation

* [Hardware research](docs/hardware-research.md): what is known, with sources and confidence labels
* [Architecture](docs/architecture.md): crate layout, frontend choice, mobile feasibility, licensing
* [Accuracy matrix](docs/accuracy-matrix.md): what Ribbit implements and how it is tested
* [Open questions](docs/open-questions.md): unknowns and recommended experiments
* [CPU core options](docs/cpu-core-options.md): reuse vs. implementation
* [ARCompact ISA](docs/isa-arcompact.md): CPU evidence, assumptions, validation and instruction status
* [Plan](docs/plan.md): phases and acceptance criteria
* [Test procedures](docs/test-procedures.md)

## License

Not yet chosen. See docs/architecture.md, "Reuse and licensing".
