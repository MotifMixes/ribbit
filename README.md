# Ribbit

An emulator in development for the **original 2003 LeapFrog Leapster** (the blue-and-silver handheld).

> **Status: early, but SpongeBob is playable on the US v2.1 BIOS.** The ARCompact CPU core is validated against
> GNU binutils and matches MAME's control flow on real firmware. With the opt-in, clearly labelled **experimental**
> touch model borrowed from MAME, *SpongeBob SquarePants: Saves the Day* runs on the US v2.1 BIOS through name entry,
> its intro and the main menu; all four activities load, Krabby Patty gameplay responds to the d-pad, and saves
> persist. Sound uses MAME's (unverified) voice and speech model. The Universal v1.5 BIOS turns itself off a few
> seconds after start-up unless MAME's equally unverified power-line workaround is switched on. Only one game has
> been tested. See [docs/accuracy-matrix.md](docs/accuracy-matrix.md).

Ribbit also runs the **LeapFrog Zippity** (TV console, see [docs/zippity.md](docs/zippity.md)) and lists the
**LeapFrog RockIt Twist** in **early emulation**. From its system packages, the console's own firmware boots;
with unpacked content imported, its menu runs, the built-in game Nom-Nom plays with sound and its videos play. Its
.lf3 game packs are encrypted and are not run. See [docs/rockit-twist.md](docs/rockit-twist.md).

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
  File → Open Cartridge… (or drop files onto the window). Settings, saves, save states and screenshots go to
  `%APPDATA%\Ribbit`.
  * Keyboard: arrows, Z = A, X = B, H = Hint, Enter = Pause, Backspace = Home (Input → Key bindings…).
  * Mouse = stylus (experimental touch model).
  * Controllers (XInput and others via gilrs): d-pad/stick, A, B, X/Y = Hint, Start = Pause, Back = Home,
    LB/RB = volume, Guide = menu.
  * Esc = pause, Tab (hold) = fast-forward, \` (hold) = rewind, F5/F9 = quick save/load state (slots 1–5 in the
    Emulation menu), F11 = fullscreen, F12 = screenshot.
  * Video → **AI upscale (HD)** (default): the ArtCNN neural network (WGSL port, `scripts/gen-artcnn.py`)
    upscales the picture, up to 2560 × 2560. Video → AI upscale settings picks the style (Clean = ArtCNN, Soft =
    Anime4K `scripts/gen-anime4k.py`, Pixel-art edges = ScaleFX first), quality (Fast … Ultra) and resolution
    (Auto, 2×, 3×, 4×, 16×). `cargo run --release -p ribbit-frontend --example upscale` runs it on a saved frame.
    Also Sharp pixels, LCD grid, Pixel perfect, Smooth. Window size 2×, 3×, 4× or 16× (as big as fits; never maximized or full screen unless you ask).
    Display only: none of this changes emulation.
  * **Console view** (Video → Console view, or F7 for the running console): the screen inside a drawing of the
    handheld with clickable, animated controls, each showing its keyboard key as a keycap that lights up while
    pressed (Video → Show keys on the console). On by default for the RockIt Twist, off for the Leapster handhelds
    (Leapster, L-MAX and Leapster 2 share one drawing, after the original Leapster's Parent Guide); TV consoles
    have none. The game keeps running when it is switched; the screen keeps its shape and is never covered.
  * **RockIt Twist** (choose it in the library's console menu, or File → RockIt Twist (early emulation)): early emulation;
    with content imported (File → Import RockIt Twist games), the menu and Nom-Nom run.
    Keys: WASD/arrows = D-pad, Q/E = Switch, number pad = button grid, number pad * and - = Spinner, U/J, I/K,
    O/L = the three Dials, Page Up/Down or the mouse wheel = Slider, Shift/Tab = Hint, Home, Enter = check mark.
  * Emulation → Advanced holds the experimental options: MAME's power-line workaround (needed by v1.5) and the
    CPU clock (96 MHz nominal, 48 MHz as in MAME).
* **Release zip**: `python -I scripts/package-release.py` writes `target/dist/Ribbit-<version>-windows-x64.zip`
  (the two programs and a read-me only; no firmware).
* **Android** (arm64, untested on a device): `python -I scripts/build-apk.py` builds
  `target/android/ribbit-debug.apk` (needs the Android SDK/NDK and `cargo-ndk`).

## Documentation

* [Hardware research](docs/hardware-research.md): what is known, with sources and confidence labels
* [RockIt Twist](docs/rockit-twist.md): research, formats, controls, emulation status and what is missing
* [Architecture](docs/architecture.md): crate layout, frontend choice, mobile feasibility, licensing
* [Accuracy matrix](docs/accuracy-matrix.md): what Ribbit implements and how it is tested
* [Open questions](docs/open-questions.md): unknowns and recommended experiments
* [CPU core options](docs/cpu-core-options.md): reuse vs. implementation
* [ARCompact ISA](docs/isa-arcompact.md): CPU evidence, assumptions, validation and instruction status
* [Plan](docs/plan.md): phases and acceptance criteria
* [Test procedures](docs/test-procedures.md)

## License

Not yet chosen. See docs/architecture.md, "Reuse and licensing".
