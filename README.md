<p align="center"><img src="docs/images/ribbit-icon.png" alt="Ribbit's frog icon" width="160"></p>

# Ribbit

An emulator for **LeapFrog's handhelds and TV consoles**: the original 2003 Leapster (the main target), the
Leapster L-MAX, Leapster TV and Leapster 2, the Zippity and the RockIt Twist. Written in Rust, for **Windows, macOS,
Linux and Android** (the Android app is new and has not been tested on real devices yet).

## Download and play

**You don't need to install Rust or anything else.** The download is the finished app. (Rust is only needed to
build Ribbit from its source code, which players never have to do.)

1. Open the **[latest release](https://github.com/MotifMixes/ribbit/releases/latest)** and, under *Assets*,
   download the file for your system:

   | System | File | Then |
   |---|---|---|
   | **Windows** 10 or 11 | `Ribbit-…-windows-x64.zip` | Unzip it and double-click **`Ribbit.exe`** in the `Ribbit` folder. |
   | **macOS** 11 or later | `Ribbit-…-macos-universal.zip` | Unzip it and drag **`Ribbit.app`** to Applications. The first time, right-click it and choose **Open** (it isn't notarised by Apple). |
   | **Linux** x86-64 | `Ribbit-…-linux-x64.tar.gz` | Unpack it and run **`./ribbit`** in the `Ribbit` folder. |
   | **Android** 8 or later | `Ribbit-…-android-arm64.apk` | Open it on the device and allow installing it. |

   (The *Source code* files at the bottom of the list are for developers, not for playing.)
2. In Ribbit, choose your BIOS dump with **File → Open BIOS…** (asked once).
3. Add your games folder in the library, or open a game with **File → Open Cartridge…**.

**Updating:** do it from inside Ribbit.

1. When a new version is out, a green button appears at the top of Ribbit, right after the menus. It shows the new
   version number, for example **⬆ Update to v1.2.3**. (No button? Use **Help → Check for updates**.)
2. Click it to see what's new, then click **Update now**. Ribbit downloads the new version, installs it and restarts
   by itself. A game you're playing closes first, and your saves and settings are kept.

If **Update now** isn't offered (Android, or versions before 1.2.3), the button says **Download** and opens the
release page instead: download the file for your system as above, close Ribbit, and unzip the new version **over your
old `Ribbit` folder** (replace the files when asked).

**Something wrong?** If Ribbit closes or shows an error, send the file `ribbit.log` with your report. It's in
`%APPDATA%\Ribbit` on Windows (paste that into the File Explorer address bar) and `~/.config/ribbit` on macOS and
Linux.

**→ [User guide](docs/manual/01-welcome.md)** (also in Ribbit: **Help → User Guide**)

**→ [Compatibility list: every console and every game tested](docs/compatibility.md)**

> [!IMPORTANT]
> ## This project was made with AI
>
> **Ribbit is written almost entirely by an AI coding assistant** (Anthropic's Claude, through Claude Code), working
> under my direction: I choose what to work on, test it, supply the hardware references and decide what goes in. The
> code, the research notes in `docs/` and most of the documentation were produced by the AI.
>
> I know many people in emulation and open source don't want to use AI-written software or contribute to it, for
> reasons I respect: questions about licensing and where training data comes from, code quality, and the work of
> people who reverse-engineer hardware by hand. If that's you, no hard feelings, and I'd rather you know up front
> than find out later. Please judge the project by its results and its sources: every hardware claim in `docs/` is
> labelled with how sure it is (VERIFIED, EVIDENCE, HYPOTHESIS, UNKNOWN) and where it comes from, and the tests are
> there to check the behaviour.
>
> Ribbit is not affiliated with or endorsed by LeapFrog Enterprises. LeapFrog, Leapster, Zippity and RockIt Twist
> are their trademarks.

## You need your own dumps

Ribbit includes **no firmware and no games**, and none will be accepted into this repository. To play, dump the
BIOS or system software from **your own console** and your own cartridges. Ribbit never decrypts encrypted game
packs (RockIt Twist `.lf3`) or works around copy protection.

> [!NOTE]
> **RockIt Twist games must be decrypted before Ribbit can play them.** LeapFrog's `.lf3` game packs are
> encrypted, and Ribbit does not decrypt them and includes no tools for it. It plays only packs that are already
> decrypted and unpacked: package folders with their `meta.inf`, laid out as they are installed on a console.
> Import a zip of them with File → Import RockIt Twist games. An `.lf3` file opened as it is will not run.

## Features

**Consoles**

* **Leapster** (original): runs the real BIOS (US v2.1; UK, Spanish 2.x; Universal v1.5 with an opt-in workaround)
  and its built-in Flash player. ARCompact CPU core checked against GNU binutils on millions of instructions and
  against MAME's control flow on real firmware.
* **Leapster L-MAX, Leapster 2, Leapster TV** (experimental): their own BIOSes boot and run the same games;
  Leapster 2 touch, ADC, SD slot and PCM sound are modelled.
* **Zippity** (ARM9 TV console): system menu, built-in games and cartridges, dance mat and bopper controls, sound.
* **RockIt Twist** (early): the real bootloader and firmware, menu, built-in games, MPEG-4 videos, sound, dials,
  spinner and slider, saves.

**App**

* Game library with a tab per console, title cards and drag-and-drop.
* Battery saves (cartridge and system EEPROM, Zippity NAND, RockIt Twist storage) kept automatically.
* Save states (5 slots), rewind, fast-forward, pause, screenshots.
* Keyboard, mouse (as the stylus) and game controllers (XInput and others), all remappable.
* **Touch controls** for phones and touchscreens: an on-screen D-pad and buttons for each console, several fingers
  at once. They appear when the screen is touched and hide as soon as a key or controller is used.
* **Update check**: tells you when a new release is out (Help → Check for updates, or at start-up), with the patch
  notes of every release in a pop-up and under Help → Patch notes. It only asks GitHub for the release list; turn
  it off in the pop-up.
* **Console view**: the screen inside a drawing of the console, with clickable, animated controls and keycaps
  that light up.
* Picture options (display only, emulation is unchanged): sharp pixels, LCD grid, pixel perfect, smooth, and
  optional AI upscaling (ArtCNN / Anime4K) up to 2560 × 2560.
* **HD vectors** (experimental, original Leapster and L-MAX): Leapster games are Flash, so their artwork is stored
  as vector shapes (outlines and fills), not pixels. Ribbit takes those original shapes from the console's own Flash
  player and redraws them at 640 × 640 instead of 160 × 160, so edges come out sharp because they are drawn again
  from the game's own data.

  **This is not AI upscaling.** Traditional AI upscalers (like the optional one above, or the "enhance" filters in
  other emulators) are neural networks trained on lots of pictures. They only ever see the small, finished image, and
  they guess what a sharper version would look like, so they can smear details or invent ones that were never there.
  HD vectors don't guess at all: the game already has its artwork as exact shapes, and Ribbit simply draws those same
  shapes again on a bigger canvas, the way the Flash player would on a higher-resolution screen. No neural network,
  smoothing or guessing is involved, and nothing is added. Parts that aren't vector art (bitmaps, text drawn from
  pixel fonts) keep their original pixels. The AI upscaler is a separate option and is never used with HD vectors.
* Exact idle-loop skipping: same output, much less CPU.
* Discord status, window sizes up to 16× (never maximized or full screen unless you ask).
* Command-line and headless runners for testing and research.

## Known bugs and limitations

**Leapster family**

* Only SpongeBob: Saves the Day has been played through by hand; players report Sonic X, Letterpillar and Counting on
  Zero working. The other 30 tested games reach their own menus in an automated check that presses nothing; most
  probably go further, but nobody has checked yet.
* Can be very slightly laggy on some PCs at the default CPU clock (96 MHz); Emulation > Advanced > CPU clock 48 MHz
  runs lighter.
* Touch uses MAME's guessed model (labelled experimental); its rate and values are not verified on hardware.
* Sound uses MAME's voice and speech model, also unverified against a real unit.
* Universal v1.5 BIOS turns itself off after a few seconds unless the (unverified) power-line workaround is on
  (Emulation → Advanced).
* The German 2.x BIOS dump never draws its screen (the dump is believed bad).
* CPU timing is a model calibrated on one video; some games may run a little fast or slow.
* Leapster TV: controller replies, pen calibration and parts of the picture are guesses
  ([open-questions.md](docs/open-questions.md), T1-T15).
* Leapster 2: faint high-frequency noise in raw PCM sound; LeapFrog Connect (USB) not emulated.
* L-MAX: TV output not emulated.
* HD vectors: Leapster 2 and Leapster TV not supported; gradient and bitmap fills stay at LCD resolution;
  occasional stale specks.

**Zippity**

* Sign In names appear about a second late and blink without fading.
* Ring pictures and the sound compressor / envelopes are not modelled.
* One colour format's U/V order is unverified (some Princess and the Frog backgrounds).
* Only two cartridges tested.

**RockIt Twist**

* Encrypted `.lf3` packs do not run (by design); games run only from unpacked content.
* Short sound gaps when the console does heavy file work and emulation dips below full speed.
* Some text-plane modes (rotated, zoomed, attribute maps) are missing.
* A small box appears at the top of the screen in some games.
* Loudness and sprite rotation direction not checked against a real unit.
* No LeapFrog Connect (USB) yet; USB/IP is a work in progress.

**General**

* Android: builds, but has never been run on a device; the Zippity is not wired up there. Controllers work
  through their buttons and D-pad; analogue sticks are not read yet. No update check in the Android app.
* Linux and macOS builds are new and less tested than Windows. Linux has been run under WSL only; macOS has not
  been run yet.

## Roadmap

Roughly in this order:

1. **Play-test the Leapster library** by hand and move games from "Menus" to "Playable", fixing what breaks.
2. **Perfect the Leapster 2 and L-MAX**: SD card and NAND persistence, a **virtual SD card** folder you can drag
   homebrew into, the L-MAX TV output.
3. **RockIt Twist**: a faster CPU interpreter (no sound gaps), the missing text-plane modes, test every game pack,
   LeapFrog Connect over emulated USB.
4. **Zippity**: test more cartridges, ring pictures, sound envelopes, the remaining timing differences.
5. **Accuracy**: replace MAME's guesses (touch, sound, v1.5 power line, Leapster TV) with hardware evidence;
   compare against more real-hardware recordings.
6. **HD vectors** for every Leapster model, with gradients and bitmaps.
7. **Homebrew**: documentation and tools for writing your own Leapster and RockIt Twist programs.
8. **More platforms**: Android on real devices (analogue sticks, update check), Linux and macOS tested on real
   machines.

Later, maybe: a BIOS-free mode (a separate Flash player for Leapster cartridges, clearly labelled, never replacing
the accurate path).

## Build from source (developers)

Players don't need this: [download a release](https://github.com/MotifMixes/ribbit/releases/latest) instead. Building
requires Rust 1.88+. It makes the app (`ribbit-gui`) and a command-line tool for development (`ribbit`: headless
runs, traces and memory dumps); the release packages hold only the app.

```sh
cargo test --workspace
cargo build --release
```

* **Windows release**: `python -I scripts/package-release.py` writes `target/dist/Ribbit-<version>-windows-x64.zip`
  (`Ribbit.exe` and a read-me; no firmware).
* **Linux release**: `python3 -I scripts/package-linux.py` on Linux, or `python -I scripts/package-linux.py` on
  Windows (builds inside WSL), writes `target/dist/Ribbit-<version>-linux-x64.tar.gz`. Needs `pkg-config`,
  `libasound2-dev` and `libudev-dev`.
* **macOS release** (on a Mac with Rust and the Xcode command-line tools): `python3 -I scripts/package-mac.py`
  writes `target/dist/Ribbit-<version>-macos-universal.zip` (`Ribbit.app` for Apple silicon and Intel).
* Add `--upload` to the Linux or macOS script to attach the file to the GitHub release `v<version>` (needs the `gh`
  CLI, logged in; make the release first, e.g. from the Windows build).
* **Android release** (arm64, untested on a device): `python -I scripts/build-apk.py` builds
  `target/android/ribbit-debug.apk` and the release copy `target/dist/Ribbit-<version>-android-arm64.apk` (needs the
  Android SDK/NDK and `cargo-ndk`). Android 8 or later.

## Playing

[Download a release](https://github.com/MotifMixes/ribbit/releases/latest) and run `Ribbit.exe` (Windows),
`Ribbit.app` (macOS) or `ribbit` (Linux); on Android, install the `.apk` (on a phone, touch controls appear on
screen). Built from source, the app is `target/release/ribbit-gui`. Add your games folder in the library, or use File →
Open BIOS… once and then File → Open Cartridge… (dropping files onto the window works too). Settings, saves, save
states and screenshots go to `%APPDATA%\Ribbit` on Windows and `~/.config/ribbit` on Linux and macOS.

**If Ribbit closes or shows an error**, send `ribbit.log` from that folder with your report (it is rewritten each
time Ribbit starts). When the graphics card can't draw the window, Ribbit on Windows draws in software instead; to
force that (a graphics driver that crashes or draws wrongly), set the environment variable `RIBBIT_SOFTWARE_GPU=1`.

**Updating**: replace the program files with the new release. Saves and settings live in the data folder above, not
next to the program, so an update never touches them. The first time a new version starts, Ribbit also copies all
saves and settings into `backups` in the data folder (the newest five are kept), and a save or settings file it can't
use is kept aside as a `.bak` file instead of being written over. Save states only load in the Ribbit version that
made them; in-game saves always carry over. On Android, install the new APK over the old one (don't uninstall first:
that deletes the app's data).

**Leapster keys**: arrows, Z = A, X = B, H = Hint, Enter = Pause, Backspace = Home. Mouse = stylus.
**Controllers**: d-pad/stick, A, B, X/Y = Hint, Start = Pause, Back = Home, LB/RB = volume, Guide = menu.
**Shortcuts**: Esc = pause, Tab (hold) = fast-forward, \` (hold) = rewind, F5/F9 = quick save/load state,
F7 = console view, F11 = fullscreen, F12 = screenshot, F1 = controls for the running console.

All keys can be changed in Input → Key bindings…. Zippity and RockIt Twist controls are listed in
[zippity.md](docs/zippity.md#in-the-ribbit-app) and [rockit-twist.md](docs/rockit-twist.md#keys).

## Documentation

* [Compatibility](docs/compatibility.md): every console and game tested
* [Hardware research](docs/hardware-research.md): what is known about the Leapster family, with sources and
  confidence labels
* [Zippity](docs/zippity.md) and [RockIt Twist](docs/rockit-twist.md): research, formats, controls and status
* [RockIt Twist USB](docs/rockit-twist-usb.md): the USB controller and LeapFrog Connect protocol
* [Architecture](docs/architecture.md): crate layout, frontend choice, licensing notes
* [Accuracy matrix](docs/accuracy-matrix.md): what Ribbit implements and how it is tested
* [Open questions](docs/open-questions.md): unknowns and experiments to settle them
* [ARCompact ISA](docs/isa-arcompact.md) and [CPU core options](docs/cpu-core-options.md)
* [Plan](docs/plan.md) and [test procedures](docs/test-procedures.md)

## Credits

Ribbit builds on public research, above all [MAME](https://www.mamedev.org/)'s Leapster driver and software list,
LeapFrog-Tools, GNU binutils' ARCompact support, and the people who dumped these consoles and recorded them running.
Third-party code and weights (ArtCNN, Anime4K and others) are listed in
[THIRD-PARTY-NOTICES.txt](THIRD-PARTY-NOTICES.txt).

## License

Not yet chosen. Until one is, the code is visible but all rights are reserved. See docs/architecture.md, "Reuse
and licensing".
