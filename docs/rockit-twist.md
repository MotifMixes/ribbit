# LeapFrog RockIt Twist

**Status: NOT EMULATED. Research preview.** Ribbit lists the console, shows its packages in the game library,
draws it with working controls (the console view) and records every input, but no Twist software runs. The
research crate is `crates/ribbit-twist`; the frontend pieces are `crates/ribbit-frontend/src/twist.rs` and
`console_view.rs`.

The RockIt Twist (2019) is a square handheld with controls on four sides. It is **not a Leapster**: different
processor, different firmware, different software format. Nothing from the Leapster core is used for it.

Labels below: **VERIFIED** (seen in the console's own files or its official guide), **EVIDENCE** (strongly
suggested, not proven), **HYPOTHESIS** (a guess to test), **UNKNOWN**.

## Sources

| Source | What it is |
|---|---|
| Parent's Guide 91-003672-014 (US), `t7.leapfrog.com/images/6/606000-RockIt-Twist-System-PG-rev.pdf` | Official controls, ports, model number |
| Internet Archive item `leapfrog-rockit-twist-apps` (2026-06-03; folder of the same name, git-ignored) | US/UK/French system packages (.lfp) and game packs (.lf3), with `package-catalog.txt` |
| The system packages' own files (Boot, System, MfgTest, Surgeon, NandUsbBin, FileSys) | Firmware, read with `twistprobe` and string/literal scans |

No teardown, FCC internal photos, datasheet, emulator or prior reverse engineering was found (searched
2026-10-16).

## Hardware

| Part | Finding | Label |
|---|---|---|
| Model | 6060 ("Model: 6060" in the guide; "Pocket Play 6060" in the firmware) | VERIFIED |
| Codename | "PocketPlay" (`Device=` in every package's `meta.inf`); developer VTech Electronics | VERIFIED |
| Processor | 32-bit ARM: every image starts with an ARM vector table of `LDR PC` entries; the code uses CP15 (MMU/cache) instructions and nothing newer than ARMv5 was found in a scan | VERIFIED (ARM), EVIDENCE (ARMv5 / ARM9) |
| System-on-chip | GeneralPlus: the firmware links "Generalplus Multi-Media Parser" and "GENPLUS USB-CDRM" strings. Part number UNKNOWN | VERIFIED (vendor) |
| Peripheral map | Writes 0 to 0xFF000000 at reset and copies code to 0xF8000000, as the Zippity's GPL32300A does; uses 0xC0020000 (timers), 0xC0060000/0xC0060008 (UART data/status), 0xD0100038 (interrupt controller disable), 0xD0300000 (DMA), 0xD05001FC (video), 0xD0900140 (NAND controller) — all registers Ribbit's Zippity model knows | EVIDENCE: same GPL32 register family. Not verified register by register |
| New registers | Base firmware waits for 0xD000003C to become non-zero (clock/PLL setup at 0x3FB8); reads 0xF0001E00 (probably the boot ROM) | VERIFIED (accesses), UNKNOWN (meaning) |
| RTOS | uC/OS-II ("uC/OS-II Idle") | VERIFIED |
| Display | Factory test names the controller "ST7789V" or "ILI9340X" (both 240 x 320 controllers); screen shown wider than tall | VERIFIED (controllers), HYPOTHESIS (320 x 240 landscape, `ribbit_twist::SCREEN_W/H`) |
| Storage | eMMC as `F:` and a microSD-type volume as `K:` (factory test "eMMC & SD Test"); FAT32 (`FileSys` is a FAT32 image of a ~3.3 GB volume) | VERIFIED |
| Sound | MP3 and video playback (GeneralPlus parser, `LibMp3DecodeTask`, `LibMp4DecodeTask`); headphone jack, speaker | VERIFIED (features); hardware UNKNOWN |
| USB | Micro-USB: mass storage and LeapFrog Connect transfers ("DFTP" commands: LIST, RETR, STOR, IPKG...) | VERIFIED |
| Power | Rechargeable battery (not removable), ADC battery test | VERIFIED |

## Controls

The factory test firmware (MfgTest, package PCKT-0x004F000A) names every input, and the Parent's Guide describes
each one. **VERIFIED:**

| Control | MfgTest name | Parent's Guide |
|---|---|---|
| D-pad (red) | DPAD UP / DOWN / LEFT / RIGHT | "Press the D-Pad left, right, up, or down" |
| Switch (purple) | SWITCH LEFT / SWITCH RIGHT | "Rock the Switch back and forth" |
| Button grid (3 x 3, green) | MATRIX 1-9 | "Press any of the nine buttons" |
| Spinner (blue) | Spinner, "Turn R" / "Turn L" | "turns in both directions" |
| Slider (orange) | Slider, "Mov Up" | "Swipe your finger up and down the Slider" (a touch strip) |
| Dials (yellow, green, red rollers) | Yellow / Green / Red Dial | "Turn the Dials upward or downward ... independently" |
| Check mark (green) | ENTER | "confirm on-screen selections" |
| Hint ("?") | HELP | "when you need help in a game" |
| Home | HOME | "return to the game or system menu" |
| Volume up/down (right edge) | VOL+ / VOL- | "Press the Volume Control up or down" |
| Power (right edge) | ON/OFF | "Press and hold" |

**UNKNOWN:** how any of these reach the processor (GPIO bits, a key matrix, encoder phases, the slider's sensor),
whether the Switch springs back, and which grid button is MATRIX 1. `ribbit_twist::input::Controls` therefore
models the *physical* state (buttons held, detents turned, finger position on the slider), from which a future
hardware model will derive its registers.

## Software and formats

| Item | Finding | Label |
|---|---|---|
| `.lfp` system package | Zip archive: `Firmware/meta.inf` (key="value"), `Firmware/packagefiles.md5`, one binary named by `BinFile`. Not encrypted | VERIFIED |
| System packages (US) | Bootloader (`Boot`, 1.0.48.0), Base (`System`, 1.0.50.0), Firmware-Bulk (`FileSys`, FAT32), MfgTest, Surgeon and Paramedic (`NandUsbBin`, recovery) | VERIFIED |
| Firmware image | 0x18-byte header (magic 0x9ABCDEF0, 1, 0, 0, code size, check word), code linked at 0, 4-byte trailer. Check word and trailer: neither a sum, CRC32 nor size | VERIFIED (layout), UNKNOWN (check) |
| `.lf3` game pack | Built-in games, music, videos and add-on packs. Whole file encrypted: no header, high entropy. The base firmware links AES and reports "failed to decrypt LDI" | VERIFIED (encrypted) |
| Boot | Bootloader relocates to about 0x00F00000 (as the Zippity's does), then loads the System image | EVIDENCE (probe) |

## What is needed to emulate it

1. **The boot ROM** (on-chip; reads at 0xF0001E00 suggest it is used after boot). Not dumped. Without it the
   bootloader must be started directly, as Ribbit does for the Zippity.
2. **An eMMC dump of a real console** (or a reconstruction: the FAT32 `FileSys` package plus installed content).
   The built-in games are LF3 packs: on a console they are stored as LeapFrog Connect installed them, which may be
   decrypted or re-encrypted per device. UNKNOWN.
3. **The LF3 keys**, or decrypted content from a console. Ribbit does not break or bypass the encryption; game
   packs stay unreadable until content is dumped from a console the user owns.
4. **The register map**, verified by running the firmware: the probe shows the GPL32 family's layout, so the next
   step is a Twist machine built on `ribbit-arm`, reusing Zippity device models only after each one is checked
   against the Twist firmware's use of it.
5. **The display setup** (LCD controller commands over the SoC's display interface) to confirm 320 x 240.
6. **Input registers**: found by tracing what the factory test reads while each control changes (the I/O test
   screen shows every input live).

## Tools

* `cargo run --release -p ribbit-twist --example twistprobe -- <package.lfp | image> [instructions] [ADDR=VALUE ...]`
  runs a firmware image on the ARM9 model with plain memory around it and lists every page outside RAM it
  touched, with the first access, any text written to 0xC0060000 and the loops it ended in. `ADDR=VALUE` answers
  a register read with a fixed value to see past a wait; such runs print "STUB (not hardware)".

  Results (2026-10-16, 50 M instructions): Bootloader 1.0.48.0 and Base 1.0.50.0 both reach a wait on
  0xD000003C; with `D000003C=1` the Base firmware goes on to set up timers, the UART, DMA and on-chip RAM, then
  waits on a timer (the probe has none).

## Using it in Ribbit

* **Choose the console**: the library's console menu lists "RockIt Twist" (always). Its tab lists `.lfp` and
  `.lf3` packages found in your games folders, named from `package-catalog.txt` when the archive has one; LF3 packs
  are tagged "Encrypted", system packages "System".
* **Open it**: click a package, or File → RockIt Twist preview (not emulated). The screen stays dark and says
  what is happening: no software runs; the controls work and the screen lists what is held, how far each wheel
  has turned and where the finger is on the slider.
* **Console view** (on by default for the RockIt Twist; Video → Console view → RockIt Twist, or F7): the console
  drawn around the screen after the owner's layout picture, each control showing its key as a keycap. Click the
  buttons, rock the Switch, drag or scroll the Spinner and Dials (or click one end), drag the Slider (or scroll).
  Controls and keycaps light up and move while used, from the mouse or the keyboard.

### Keys

| Control | Keys |
|---|---|
| D-pad | W A S D, or the arrow keys |
| Switch left / right | Q / E |
| Button grid | Number pad 1-9, laid out as they are (7 8 9 on top) |
| Spinner left / right | Number pad * / - |
| Yellow, green, red dial up / down | U / J, I / K, O / L |
| Slider swipe up / down | Page Up / Page Down, or the mouse wheel |
| Hint | Shift or Tab |
| Home | Home |
| Check mark | Enter |
| Volume up / down | = / - |
| Power | (on the console view only) |

These keys apply only while a RockIt Twist session is open, so Tab is Hint there instead of fast-forward; the
Leapster's keys and shortcuts are unchanged. Escape, F1, F5, F7, F9-F12, Ctrl+O and Ctrl+R keep their usual jobs.
