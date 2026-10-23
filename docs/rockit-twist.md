# LeapFrog RockIt Twist

**Status: EARLY EMULATION.** From its system packages, Ribbit starts the console on its own bootloader and
firmware. Without installed content they show the firmware's own "Device Error" screen (the menu LeapFrog Connect
installs is missing, see "How far it gets"). With content installed (an unpacked pack zip, imported: "Using it in
Ribbit"), the menu runs and the built-in Arcade game Nom-Nom plays from its title screen into the game, with sound
and the Sneak Peeks station plays its videos (2026-10-17). Albert's Escape and a portrait-mode Arcade game start;
the packs other than their trailers are untested. The console keeps its profiles and progress between sessions (the
save folder's `rockit-twist.emmc`). Missing: parts of the text planes (turned or zoomed, attribute maps), and a check
of the sound's loudness against a real unit. All controls reach the console as the firmware reads them. The factory
test firmware runs too (hold the power-on combination below).
Game packs (.lf3) are encrypted and are not run. The crate is `crates/ribbit-twist`; the frontend pieces are
`crates/ribbit-frontend/src/twist.rs` and `console_view.rs`.

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

## Bringing up the machine (`ribbit_twist::machine`, 2026-10-16)

`cargo run --release -p ribbit-twist --example twistrun -- <package.lfp> [instructions]` runs firmware on the
machine model. `RIBBIT_EMMC=<FileSys image>` attaches the eMMC; with `RIBBIT_SYSTEM=<Base .lfp>` (and optionally
`RIBBIT_MFGTEST=<MfgTest .lfp>`) the whole chip is built as the bootloader reads it (`disk.rs`) and the run starts
from the Bootloader package. `RIBBIT_FRAMES=<dir>` saves the screen as a PNG whenever it changes. `RIBBIT_WATCH`,
`RIBBIT_PEEK`, `RIBBIT_TASKS`, `RIBBIT_DUMP`, `RIBBIT_SCREEN`, `RIBBIT_WAV` are research outputs (see the
example's source).

**How far it gets (VERIFIED by running it).** The real chain runs: the Bootloader (1.0.48.0) identifies the board,
reads the eMMC's boot header and partition table, checks and loads partition 1 (the System, Base 1.0.50.0) to
address 0 and jumps to it. The System mounts F:, plays the start-up logo (its own MP3 decoder, with the accelerator at 0xD0A00000, feeds the
DAC; the video frames go to the hardware MPEG-4 decoder, not emulated, so the logo is a red screen), then looks for
the menu, `f:\Applications\Bundle\main.bin` (0x215F0). The factory volume's `Applications` folder is empty, so
it shows "Device Error. Please connect your device to LeapFrog Connect." (0x330B4 -> 0x32E70 with 1; the screens
are chosen per language from the table at 0x7DC10). That screen reaches the emulated panel at 15.4 s, through the
firmware's own display path. This is what a real unit with a factory-fresh volume would show, as far as the code
says: **the firmware is behaving correctly; what is missing is the content LeapFrog Connect installs.**

**What LeapFrog Connect installs (from the System's code; layout otherwise UNKNOWN).** Installed content lives in
`F:\Applications\`, `F:\Music\`, `F:\Videos\` and `F:\Trailers\`, the built-in set under `Bundle` in each.
A `doom` file in a package folder marks it for deletion. A licence file, `F:\ldi.dat`, is decrypted by the System
with a key built from "LeapFrogVTech" and the unit's serial number (0x22744 on; "BEGIN_LDI" ... "END_LDI"); the
packages it lists by `PCKT-` ID are the ones the unit may keep. No public dump or write-up of an installed Twist
exists (searched 2026-10-16).

**Starting the menu, and the program format (VERIFIED by running it).** The launcher (0x215F0) starts
`f:\Applications\Bundle\main.bin` only if `F:\Music\Bundle` and `F:\Trailers\Bundle` exist too and none of
the three holds a `doom` file; otherwise it falls back to the Device Error screen. (So the Included Games, Included
Music and Sneak Peeks packs are installed together.) The loader (0x2C038) reads the whole file to 0x200000 (at most
3 MB, up to 0x500000); the launch code (0xA144-0xA294) then calls entry 0 at 0x200000 as `main(argc, argv)`. The
file starts with 8 entry points 16 bytes apart (0x2C0D4 calls entry n at 0x200000 + 16 n); only entry 0 is used to
start it. Nothing is checked or decrypted before it runs. Programs reach the System's services through the jump
table it builds at 0xFFF000 (each slot a branch; slot names such as `AppLoader_*` and `Ptp_*` are in the System's
string table, mapping not worked out yet). Checked with a 0x2C-byte program of Ribbit's own that fills the
framebuffer the panel streams (address from 0xD050033C): the screen turns green and the processor stays in its
loop.

**How LeapFrog Connect installs (VERIFIED from the System's code).** Over USB the System serves a small
file-transfer protocol (its task is "DftpJobTask"; command table at 0x7AE90, handlers after it): ATTR, DCON, GETS,
INFO, IPKG, LIST, MKD, NOOP, OFF, RETR, RM, RMD, RSET, SETS, SIZE, STAT, STOR, UPD8, with FTP-style replies
("200 OK", "550 Path not found: "...). GETS reports settings such as BASEVERSION, FREESPACE, MFG_SERIAL, MFG_SKU,
MFG_LOCALE and MFG_MFGTESTENABLE. `IPKG [dir]` (0x297E0) installs a package: it names the folder (0x31DA4, via
the jump table at 0xFFF260), then the data that follows is fed 512 bytes at a time (0x2AA58 -> 0x31E44) to a tar
("ustar") extractor (0x31A4C) that only creates folders and writes files. Nothing on that path decrypts. So the
console receives packs as plain tar archives: **LeapFrog Connect decrypts the `.lf3` on the computer.** An
emulated console running its own firmware will therefore never unpack an `.lf3` itself.

**The USB transport (VERIFIED by running it, docs/rockit-twist-usb.md).** The console is a USB mass-storage device
(Bulk-Only Transport, SCSI) whose one LUN is a 512-byte-block disk: INQUIRY "Leapfrog" / "Pocket Play 6060" (the
"GENPLUS USB-CDRM DISK" data in the firmware belongs to a LUN type it never sets). USB vendor 0x0F63, product 0x0047.
Its mass-storage driver has hooks (installed at 0x25C6C) for two block numbers on LUN 0: a SCSI WRITE(10) of one
block at LBA 0x20 sends a command line (0x25CF8), a READ(10) of one block at LBA 0x20 returns the reply
(0x263C8), and transfers at LBA 0x40 (up to 32 blocks each) carry file data. Replies come from the table at
0x7AE58 ("200 OK", "101 EOF:", "102 BUSY", "502 Syntax error: ", ... "554 Error writing file"). RETR replies
"200 OK:" with the file's size, then the data follows at LBA 0x40.

**RETR refuses games, videos and music (VERIFIED, 0x29ACC -> 0x26988).** Before opening a file, RETR compares its
extension (lowercased) with `.mp3`, `.avi` and `.bin` (list at 0x7AE3C) and answers "553 Cannot open file" for
them. Games and the menu are `.bin` files, so they cannot be copied off a console over USB; Ribbit does not try to
get round this (for example through the firmware-update command).

So, since Ribbit also does not decrypt `.lf3` packs or forge a licence, games can run only from a full copy of a
console's internal storage (its eMMC chip, read directly: a hardware job), or from content LeapFrog Connect
installs into the emulated console itself, which would need the emulated console to appear to the computer as a
USB device.

Devices found and modelled, each from the firmware's own use (addresses VERIFIED as used; meanings as stated):

| Device | Base | What the firmware shows | Status |
|---|---|---|---|
| Clock | 0xD000003C | waits for non-zero after clock set-up (also in MAME's GPL32612 stub) | reads 1 |
| Timers | 0xC0020000 + 0x20 n | +0x10 current count (time base, timer 5); timer 0 → IRQ 25 is the OS tick (handler 0x5968) | periodic, rates ASSUMED |
| Interrupt controller | 0xD0100000 | +0x28 pending IRQ number, +0x2C FIQ, +0x30 mask (bit 32-n), +0x38 bit 0 disable — as the Zippity's | modelled |
| SD/MMC hosts | 0xC00A0000 (eMMC), 0xC00B0000 (SD slot) | +0x00 TX data, +0x04 RX data, +0x08 command (bits 5-0 index, 7 start, 12 response, 13 no CRC; 0x40 power-up clocks, 0x800 init clocks), +0x0C argument, +0x10 response (one word per read), +0x14 status (0 busy, 2 complete, 4/5/9 errors, 6 response ready, 7 data ready; write 1s to clear), +0x18 clock divider/bus width/block length | modelled with a JEDEC eMMC card (`sd.rs`) |
| DMA | 0xD0300000 + 0x40 n | +0 control (0 start, 7 fixed source, 13-12 unit, 10 continue with the queued buffer), +4/+8/+C source/destination/count, 0x1FC done flags (IRQ 24) | modelled; DAC transfers paced |
| Audio DAC | 0xC00D0000 + 0x20 c | +0 control (bit 14 IRQ), +4 sample, +8 status (bit 15 full); IRQ 31/30; fed by DMA with the System's MP3 output (start-up logo, Music station) | modelled; 16-bit units (see "MP3") |
| Sound unit (SPU) | 0xD0400000 | 32 channels, the Zippity's unit with 16-bit registers and ADPCM36 (see "Sound") | modelled (`sound.rs`) |
| MPEG-4 decoder | 0xD0C00000 | +0xC0C compressed frame (VOP start code), +0xC08 RGB565 output, +0xC04 the reference frame's output, +0xA0 bit 4 start, +0xD0/+0xD4 IRQ enable/status (bit 2 done, bit 4 failed; IRQ 26) | decodes (`mpeg4.rs`, see "Video playback") |
| Timebase | 0xC0040000 | +0x50 control, +0x54/+0x58 status/enable; bit 8 tick (IRQ 1) drives the firmware's time-outs (0x5014) | ticks at 1024 Hz (ASSUMED) |
| ADC | 0xC00C0000 | +4 channel/start/done, +8 result; ch 1-2 board identity, ch 5 battery | modelled |
| RTC (external) | 0xC0090000 | serial access to an external clock chip (registers 0-7) | not modelled |
| Display | 0xD0500000 | panel command port +0x140/+0x16C (MfgTest), framebuffer +0x1E8, frame IRQ status +0x18C; PPU-style registers like the Zippity's | sprites and text planes (`video.rs`, see "Video") |
| GPIO | 0xC0000000 (ports at +0x40, +0xC0, +0xE0, +0x180) | buttons active-low; the 3x3 grid scanned (columns driven high, rows read high); USB power on port C bit 11 (see "Buttons") | modelled, VERIFIED |

**Board identity (VERIFIED).** One bootloader serves the PocketPlay family. It reads ADC channels 1 and 2
(resistor dividers) and looks them up in tables at 0xF052CC: channel 1 in 0x56-0x1A0 with channel 2 in 503-832 is
product 6, "RockIt Twist LF" (1002-1341: 7, "RockIt Twist VT"). Other products take other paths (charging/USB mode).

**Screen (VERIFIED).** 320 x 240 RGB565 (the LCD set-up writes 0x140 x 0xF0; the `system/*.bin` screens are
153 600 bytes). Pictures are stored 240 wide and 320 tall, as the panel scans them; the panel is mounted sideways.

**Display (VERIFIED from the firmware).** The panel takes ILI9341/ST7789-style commands through 0xD0500140 (+0x16C
byte: 0x8_ command, 0xA_ data): 3A 05 (RGB565), 2A/2B (240 x 320), 11, 29, 35 (tearing-effect line on), 2C.
Writing 0xD_ with bit 0 set to 0x140 streams the framebuffer at 0xD050033C to the panel. The tearing-effect pulse
arrives as external interrupt 0xD0100020 (enable bit 0, status bit 6, IRQ 6) and runs the frame handler (0x23B3C),
which starts the next transfer. The pulse rate, 60 Hz, is ASSUMED. The two firmwares set the panel up differently:
the System sends 36 40 (column order mirrored) with a 240 x 320 window, the factory test 36 20 (rows and columns
exchanged) with 320 x 240. The machine keeps the window and the 0x36 bits (`machine::Panel`) and places each frame
by them; with the panel mounted so that its memory rows run across the screen, both come out upright.

**Buttons (VERIFIED).** The bootloader (0xF003C8), the System (0x23380) and the factory test (0xA62C) share one
key-scan routine. The factory test's I/O test names each bit it returns (table at 0x4CE7C); `machine::key` has
them. Pins read low while pressed: port 0xC0000000 bit 9 Switch left, bit 8 Switch right; 0xC0000040 bit 9 Home,
bit 8 Help (the "?" button); 0xC00000C0 bit 14 power, bit 6 Vol+, bit 5 Vol-; 0xC00000E0 bit 0 Enter (the check
mark); 0xC0000180 bits 0-3 D-pad up, down, left, right. The grid is scanned: bits 12, 11, 10 of 0xC0000004 drive
one column high at a time and bits 15, 14, 13 of 0xC0000000 read its rows, high when pressed ("MATRIX 1-9": row
n / 3, column n % 3). Which corner MATRIX 1 is on the console is UNVERIFIED (taken as the top left). Checked by
running the factory test: grid key 2 opens the I/O Sequence Test, which asks for DPAD UP; pressing it moves on to
DPAD DOWN. The Dials, Spinner and Slider have their own routines (0xA85C-0xA8E0 in the factory test), modelled in
`machine::encoder`: the Spinner, Yellow and Green dials are quadrature encoders whose phase-A pins raise external
interrupts, the Red dial one interrupt a step, the Slider four touch pads. Games read them through Ptp_getSpinner,
Ptp_getDial and Ptp_getSlider as amounts turned since the last frame (Ptp_update 0xA8A8 reads and clears the
driver's counts at 0xC3214). The System acknowledges an external interrupt by writing back the status it read with
its bit set (0x23040, 0x23884), which clears every other pending bit too; the machine therefore sends each
control's edges in its own time slot (VERIFIED: turning all four at once, every count arrives).

**Power-on key combinations (VERIFIED, bootloader 0xF04F90-0xF051AC).** With Switch right, D-pad right, Vol+ and
the check mark held at power-on (keys 0x100212), the bootloader starts the factory test instead of the System,
but only if the manufacturing data's byte 0x31 is 1 (2 starts it every time). Ribbit writes 1 there (ASSUMED: a
retail unit's value is UNKNOWN). Switch left, D-pad left and Vol- (0x80120) take another path, not identified yet
(the machine stays in the bootloader).

**Storage (VERIFIED).** The layout the bootloader reads is in `disk.rs` (MBR with an "SDMM" pointer, boot header,
"MFGD" manufacturing data, "GPAP" partition table, "APDA" partitions; the FAT32 volume after them). Sector numbers
are Ribbit's own: no dump of a real chip exists. The `FileSys` volume's root holds a `FIRMWARE_UPGRADE` marker
that sends the firmware into update mode; the machine removes it (`fat.rs`), as on a unit that has finished
updating.

**Video (the menu runs, VERIFIED by running it).** With content installed, the menu (`Bundle\main.bin`) draws with
the video chip at 0xD0500000, the Zippity's family (ribbit-zippity `video.rs`). What it needed, each found where the
menu waited:

* Video DMA: 0x1C0 source, 0x1C4 destination, 0x1C8 words minus 1; the count reads 0 when done (0xE55C waits).
* The frame interrupt: 0x18C status bit 0, 0x188 enable, interrupt line 27 (the menu's mask, 0x79FFFE59, leaves it
  unmasked; its handler is 0xBFA0). Raised once a frame (ASSUMED: with the panel's 60 Hz).
* Sprites (`video.rs`): 16-byte entries at 0xD0502000, count from 0x108 (bits 15-8 x 4, bit 0 on). The picture's
  address is the sprite segment (0x88) plus halfword 0 (bits 6-0 of halfword 4 above it) times 16 (the System's own
  routine 0xF6CC computes it so). x and y are the sprite's centre (column 121, line 255 up). Depths 2/4/6/8 bits go
  through the palette (0xD0501000, one 15-bit colour a word); depth 3 with palette bit 0 is 32 bits a pixel, alpha,
  red, green, blue in that byte order. The flip bits are set on every sprite and the pictures read correctly
  unflipped, so a clear bit flips here (the Zippity's sense, reversed). These are EVIDENCE from the menu: the
  Profiles screen then comes out as on a real unit.
* The frame is drawn at each frame's end into the buffer at 0x1E8 (240 x 320, RGB 5:6:5) and reaches the panel
  through the same streaming as the System's screens (0x140 with bit 13 set: 0x20D3).

* Rotation, zoom, blending and quads, from the System's PPU_* routines (which build the list): halfword 1 bits
  15-10 the rotation in 64ths of a turn and halfword 2 bits 15-10 the zoom (1-32 in 32nds, then quarters up to
  8.75), on for every sprite with 0x108 bits 6 and 7; attribute bit 14 blending, the level in halfword 4 bits
  15-10. With 0x1FC bit 9 on ("V3D", on in the menu and the games), a sprite whose attribute bits 3-2 are not 3 is a
  quad: its four corners (top left, top right, bottom right, bottom left; VERIFIED on an axis-aligned one of
  Nom-Nom's) are packed into halfwords 1, 2, 5, 6 and 7 (PPU_fastCalculate 0xF774). Bits 3-2 = 3 is an ordinary,
  unflipped sprite, which is why every menu sprite has both "flip" bits set.
* Text planes: four, with registers at 0x40, 0x58, 0x00 and 0x20 (bases 0x80, 0x84, 0x8C, 0x90), copied in by the
  System's frame update (0xE2A0). Control bit 3 on, bit 1 register mode, bit 7 direct colour, bits 9-8 and 15-10
  blending; attributes as a sprite's plus the size (bits 15-14 and 16). The map holds 32-bit byte offsets of the
  tiles. VERIFIED with Nom-Nom's level menu: plane 0 holds its title picture, 320 x 240 in ARGB 8:8:8:8, behind a
  half-dark quad; the planes are laid out as the player sees the screen (landscape), so they are turned into the
  frame as it is drawn. Only register mode, unturned and unzoomed, is drawn yet.

* Frames and buffers (VERIFIED from the System's code, 2026-10-23):
  * Starting a frame: the frame update (0xE594) copies the sprite list by video DMA and the tables by the CPU,
    waits for the DMA, then writes 1 to 0x1F0 if a buffer is set at 0x1E8. 0x1F0 is very likely the start bit.
  * Triple buffering: the frame interrupt's handler (0xBFA0) counts each frame end, then moves 0x1E8 on to the
    next of three buffers, skipping the one being shown. 0xC23C puts the finished buffer at 0x33C and streams it
    to the panel (0x140 = 0x20D3) on the next tearing pulse. Games use the same code (buffers 0x8E7100,
    0x97D100, 0x932100 in Albert's Escape).
  * Pacing: Albert's Escape starts a frame every 20 ms exactly (50 fps). On the 60 Hz panel every sixth refresh
    repeats a frame, as on a real unit if its panel runs at 60 Hz.
  * What Ribbit does: it draws at each tearing pulse and raises 0x18C then (ASSUMED). Drawing at the 0x1F0
    write instead, with the frame end following it, was tried. The frames sent to the panel came out the same
    (none out of order in 4,521), so the simpler model stays.
* Panel set-up (the System, VERIFIED by logging it): 01; B1 00 1F; B4 80 00; B6 0A 42 27 04; 36 40; B7 ...; BA, BB,
  CD, E8, E9, EA, F2, F5; E4/E5 (gamma); BC; 3A 05; 2A/2B; 11; 29; 35 00; 2C. B1 is a frame-rate setting on
  panels with this command set; 00 1F gives about 61 Hz on the ILI9341 family (the controller is UNKNOWN).
* Sprite colour modes, from PPU_setSpriteColorMode (0x104C8). Modes 0-2 set the depth (2, 4 or 6 bits). The rest
  set depth 3 plus a palette nibble: 3 nibble 0 (8 bits), 4 nibble F, 5 nibble 1, 6 nibble 3, 7 and 9 nibble 5,
  8 nibble 7, 10 nibble 9, 11 nibble B. Albert's Escape draws its crystal in mode 11: as ARGB 4:4:4:4 it is a
  clean picture with a transparent surround, and as RGB 5:6:5 or ARGB 1:5:5:5 it is noise (VERIFIED). Modes 4
  and 10 are still drawn as ARGB 4:4:4:4 (UNVERIFIED); modes 5, 6 and 8 are not decoded.

Not decoded yet: the planes' bitmap and attribute-map modes, their rotation and zoom (0x340-0x35C), windows,
mosaic, and the 2D engine at 0xD0C00000 beyond answering its jobs.

**Speed.** The System idles in a branch to itself (0x57524); the machine skips ahead to the next device tick there.
The menu paces its frames by calling `System_getCount` until the next is due; calls to it (and to
`System_getHighPrecisionCount`, both found by name in the System's export table) close together count as waiting,
and time then skips ahead 10 µs at each. Boot to the menu (20 emulated seconds) takes about 5 s on one core of a
desktop i9; the menu runs about 1.7 times faster than real time.

## Video playback (`ribbit_twist::mpeg4`, 2026-10-17)

The start-up logo and the packs' videos are AVIs of MPEG-4 Part 2, Simple Profile (Xvid or libavcodec, 320 x 240,
some 240 x 320: I and P frames, H.263 quantisation, some with resync markers), decoded by the hardware at 0xD0C00000
one frame at a time. Ribbit decodes them with its own decoder (`mpeg4.rs`, from ISO/IEC 14496-2; its code tables are
generated from FFmpeg's, which hold the standard's values). `examples/mp4dec.rs` compares it with FFmpeg: all 17 of
the packs' videos decode at 56-69 dB PSNR against FFmpeg's pictures, the same to the eye.

What the hardware does, VERIFIED from the System's driver (0x572E0, its interrupt handler 0x56B90) and by the
pictures coming out right: a job decodes the frame at +0xC0C into +0xC08 as RGB 5:6:5, row by row; +0xC04 names the
previous frame's output, the reference; on success it sets status bit 2 (the handler posts 4 and the player shows
the frame); bit 4 is a failure (the driver resets the decoder). The start-up logo is sent to the panel straight from
the output buffers; the menu's player copies each frame into a text plane in bitmap mode.

## Sound (`ribbit_twist::sound`, 2026-10-17)

The menu and the games play through the sound unit at 0xD0400000, the Zippity's (after MAME's spg2xx_audio): the
System's Audio_* routines only set its registers, and no samples pass through the processor (the DAC carries only
the start-up logo's MP3). What differs, all VERIFIED from the System's own code:

* **16-bit registers.** A wave's address (halfwords, 28 bits) is split over register 0, mode bits 5-0 and register
  14 bits 5-0; the loop address over register 2, mode bits 11-6 and register 14 bits 11-6; the envelope's over
  registers 8, 7 and 15 (the routine that sets a channel, 0x11460).
* **Clock.** The System sets a wave's phase to its rate times 14913 / 8192 (0x11E5C), so the unit's clock is
  288 kHz (2^19 x 8192 / 14913), not the Zippity's 281.25 kHz.
* **Stopped, not paused.** Audio_getWaveStatus (0x145AC) calls a wave playing when its channel is busy (+0xE3C),
  stopped when it is enabled (+0xE00) and not busy, and paused when it is not enabled. So a channel that reaches
  its end keeps its enable bit (EVIDENCE: Nom-Nom waits for its sounds to stop before starting a level, and waits
  forever if the bit clears), and a channel starts when its bit goes from 0 to 1.
* **ADPCM36.** The packs' sounds (`*.pkg`, "JFS1" archives of "SPF2" waves: rate at +0x10, length in halfwords
  at +0x14, format flags at +0x20, data at +0x28) are mostly format 6, which the System plays with channel
  register 13 = 0xBE00 (bit 15: ADPCM36). The data is frames of a header word and 8 words of 4-bit residuals, low
  nibble first; each sample is (residual << 12 >> shift) + (c0 x[n-1] + c1 x[n-2] + 32) >> 6, with the shift in
  header bits 3-0 and the predictor pair (64ths) picked by bits 8-4 from a table of 30. MAME's guess (the filter as
  one tiny coefficient) is wrong. The table was fitted from six background-music waves that the packs hold both as
  ADPCM36 and as 16-bit samples (offset binary, format flags 0x3C15): decoding with it matches the 16-bit copies to
  29.9 dB. Its first five pairs are Sony XA's (0, 0), (60, 0), (115, -52), (98, -55), (122, -60).
* **Frame headers end the wave.** A frame's header gives its length and marks the wave's last frame: bit 15
  set, bit 14 the last frame, bits 13-9 the samples less one (VERIFIED over every wave in the packs: 15.6
  million frames read 0xBExx-0xBFxx, 32 samples; each wave's last frame alone has bit 14, and its count matches
  the data before the "ffff ffff" that follows). The wave ends after that frame; a 0xFFFF data word is only
  samples. (Read on past the last frame, the next wave's frames come out of step and the predictors overflow:
  the "farts" of 2026-10-17. Ending at any 0xFFFF word instead, the 1.2.0-1.4.0 fix, cut music short: the
  menu's 27 s song has 24 such words and looped after its first 0.63 s; fixed 2026-10-22.)
* **Stopping.** A channel that stops (its end, an envelope or ramp reaching 0, or the System clearing its
  enable bit) drops its output over about 1.5 ms instead of at once, which would click (`TAIL_FADE`).

The output (stereo, 72 kHz) goes to the speaker in the app; the DAC is left out for now (see `tick_sound`).
Its loudness against a real unit is UNVERIFIED (the menu's music comes out quiet).

**Next steps.** The remaining text-plane modes (turned, zoomed, attribute maps); playing through the other built-in
games and the packs; the DAC sample format; a small box drawn at the top of the screen in some games (from a sprite
not identified yet); checking the loudness and the rotation direction against a real unit.

## USB and LeapFrog Connect (2026-10-17)

The console's USB device controller (0xC00E0000, `udc.rs`) is emulated from its driver's own use of it
(docs/rockit-twist-usb.md): the firmware polls it from its USB task; the model answers the standard requests the
silicon handles itself (SET_ADDRESS, GET_STATUS, CLEAR_FEATURE...), passes the rest to the firmware, and moves bulk
data by FIFO and DMA as the driver expects. `usbip.rs` offers the device to the PC over USB/IP (TCP port 3240), the
protocol of Linux's usbip, which usbip-win2 brings to Windows. `examples/usbhost.rs` plays the PC's part, directly
or over USB/IP: VERIFIED that the console enumerates (descriptors, configuration, one LUN), answers INQUIRY, READ
CAPACITY and TEST UNIT READY, and runs LeapFrog's file protocol (INFO "VERSION=4", LIST, GETS BASEVERSION /
FREESPACE / MFG_SERIAL, RETR of a file through LBA 0x40).

In the app: File → RockIt Twist → "Plug into this PC (USB)" starts the server with the console; then, with
usbip-win2 installed, `usbip attach -r 127.0.0.1 -b 1-1` plugs it into Windows. While plugged in, the console keeps
running when another window (LeapFrog Connect) is active.

LeapFrog Connect 2 (4.6.5) accepts the emulated console (serial "RIBBIT00000000000001") and flashes it (user report,
2026-10-18). Found on the way (2026-10-18):
* Windows asks for Microsoft's OS string descriptor (0xEE) first; the firmware stalls it, and afterwards leaves the
  EP0 IN-token event (0xE8 bit 4) enabled. A token raised with the next SETUP was then cleared by the firmware's
  "enable and clear" (E4 |= 0x14; E8 = 0x10) before it was handled, and the console never answered again (the app
  fell behind real time). `udc.rs` raises the token again when the firmware clears it while keeping it enabled.
* Connect asks `GETS BATTERYLEVEL` (the RockIt Twist plugin's strings; `BATTERYSTATUS` is "504 Unknown setting" on
  the real firmware too). The System's battery monitor (0x31824, every 250 ticks) starts ADC conversions on channel 5
  with interrupt enable (0xC00C0004 bit 14) and waits for the conversion-done interrupt (bit 15 pending, line 29,
  handler 0x229B8); without it the level stayed 0 and Connect kept asking to charge. Now 2560 (0xA00), above the
  firmware's "good" threshold 0x8C3 (callback 0x31630: good > 0x8C3, low <= 0x863). What a real unit reads is
  UNVERIFIED.
* OPEN: Connect's update ends in "Error 2" although the flash is written (the console boots fine afterwards). The USB
  log (`ribbit::twist::usb` in ribbit.log; the file-protocol commands and replies, SCSI commands and control
  requests) should show what it waits for, probably the console restarting and coming back on USB.

## Using it in Ribbit

* **Choose the console**: the library's RockIt Twist tab lists `.lfp` and
  `.lf3` packages found in your games folders, named from `package-catalog.txt` when the archive has one; LF3 packs
  are tagged "Encrypted", system packages "System".
* **Open it**: click a package, or File → RockIt Twist (early emulation). When the Bootloader, Base and Firmware-Bulk
  packages are in your games folders, in the same archive folder as the package opened, or in Ribbit's own
  `rockit-twist\system` folder (in its data folder; as `.lfp` files or unpacked, a `PCKT-...` folder holding
  `Firmware\meta.inf`), the console starts
  from its bootloader and runs in real time: the start-up logo (no picture yet), then the menu if content is
  installed (below), otherwise the firmware's own Device Error screen, because the menu LeapFrog Connect would
  install is missing (see "How far it gets"). Sound plays through the app's speaker settings. Reset powers it off and on. Without those packages, the screen says what is missing and lists what
  is held, how far each wheel has turned and where the finger is on the slider.
* **Import a zip** (File → Import RockIt Twist games (.zip)…): a zip of package folders, each with its `meta.inf`
  (`Type` Application, Music, Video or Trailer), as installed on a console. Ribbit unpacks one region's packages
  (the system packages' region, else en-us) into its data folder laid out like F: (`ribbit_twist::content`), then
  uses that as the content folder below. It does not decrypt anything.
* **Content folder** (File → RockIt Twist content folder…): a folder on the computer laid out like the console's
  F: drive (`Applications\Bundle\main.bin`, `Music\Bundle`, `Trailers\Bundle`...). Its files are written onto
  the emulated storage at power-on (`fat::add_host_folder`, a FAT32 writer: long names, both FATs, the factory
  volume's free space), so the firmware finds them as LeapFrog Connect's installs. For your own programs (see
  "Starting the menu, and the program format") or content copied from your own console. Ribbit does not decrypt
  `.lf3` packs. Remove it from the same menu; a running RockIt Twist restarts either way.
* **Controls**: the buttons (D-pad, grid,
  Switch, Hint, Home, check mark, volume), the Dials, the Spinner and the Slider all reach the emulated
  console.
  To see the factory test, hold E, D, = and Enter (Switch right, D-pad right, Vol+, check mark) and press Ctrl+R (Reset);
  its menu items are the grid keys (number pad 7 = M1).
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
Every RockIt Twist key can be changed: Input → Key bindings…, "RockIt Twist keys" (a key given to an action stops
doing what it did before; "Restore RockIt Twist defaults" undoes them all).

**Game controller:** D-pad or left stick = D-pad, A = check mark, B = Hint, Y = Home, X = grid button 5 (the
middle), LB / RB = Switch left / right, LT / RT = Spinner left / right, right stick up / down = yellow dial, left /
right = green dial, Back / Start = red dial down / up, Guide = Ribbit's menu.

**Save states:** F5 saves and F9 loads (slot 1; File menu for the other slots), as for the Leapster. A state holds
the whole console except its storage's base image, so it loads only into a RockIt Twist started with the same
system packages and content.

**Sound level:** the sound unit's mix is raised 4 times, its peaks eased in rather than clipped (`LOUDNESS` in
`crates/ribbit-frontend/src/twist.rs`), then the app's volume applies. How loud a real unit is is UNVERIFIED.

## Open work (end of 2026-10-17)

**Sound gaps when the console is busy.** The app's log (`ribbit::audio` "audio ran dry", `ribbit::twist` "fell
0.25 s behind") shows short dropouts in heavy stretches: once at about 65% of real speed, 4 gaps in 5 s. The
emulator averages about 2.4 times real speed, but some half-seconds run at 50-75% (seen at 16, 28, 32, 36, 40 and
61 s of the scripted run). That is real firmware work (FAT code at 0x46300-0x47A00 while files are listed, deleted
and written; 2424 sector writes at 15.5 s), not a wait we could skip; the interpreter runs 60-85 M instructions/s
there. The sound queue holds 0.1 s, so it runs dry.

**Done 2026-10-22: the processor slows, not the console** (`Twist::stall_for`). Measured on the start-up (an i9):
from 13.5 to 18 s the System runs flat out, 240 M instructions per emulated second, which the interpreter does at
85-107 M a second; the console fell up to 0.79 s behind. Now, while the emulator cannot keep up, each instruction
takes up to `MAX_CPI` (4) cycles: the sound unit, timers and panel keep to real time and the music plays on, as
on a real unit, whose sound unit plays by itself. Worst lag on the same start-up: 12 ms. One cycle an instruction
was ASSUMED anyway (a real ARM9 waits on memory). A faster interpreter (or an ARM JIT) is still the way to make
slower PCs keep up below 4 cycles an instruction.

Tools for it: `twistrun` with `RIBBIT_SPEED=1` (or `all`) prints the slow half-seconds; `RIBBIT_SAVE_STATE` /
`RIBBIT_LOAD_STATE` time one stretch again and again; `examples/cpubench.rs` measures the bare interpreter.
Windows timings are noisy (pin to a P-core); for a profile, build in WSL with debug info and use `perf`.

**Fixed that day:** the "farts": ADPCM36 waves ending part way through a frame were read on into the next wave
(see "Sound"); stopped channels now fade instead of clicking; the register map's hasher and per-instruction checks
made cheaper (about 7% in heavy stretches).

## MP3: a decoder accelerator at 0xD0A00000 (found 2026-10-23, modelled in `mp3acc.rs`)

Music-station songs and the start-up logo's sound were silent: the System's software MP3 decoder (the task
`LibMp3DecodeTask`, decode call 0x33790) hands two stages to hardware at 0xD0A00000, and the DAC got only zeros
(VERIFIED by running the decode call in Ribbit and in Unicorn from the same state). Ribbit now does both jobs at
once when started, and the busy bits read clear.

* **Job A, the IMDCT** (0x37E40 starts it: +0 mode, then +0 |= 1). Mode bit 0 start; bits 3-2 the block type; bit
  4 a mixed block. +0x10/+0x14 the input: 576 signed 16-bit lines, Q15, equal to minimp3's values after the alias
  reduction x 32768 (VERIFIED against a reference decode). +0x18/+0x1C the channel's overlap buffer (0x480 bytes).
  +0x20/+0x24 the output: 576 16-bit values (VERIFIED as the size: a stereo stream's two outputs are 0x480 bytes
  apart). The scale is ASSUMED to be Q13, which gives 2 bits of headroom.
* **Job B, the polyphase synthesis** (0x37DA8: +0 = 0, five addresses as low/high halves at +0x28 ... +0x4C, then
  +4 = 0x45, or 9 for mono). +0x28 the left job-A output, +0x30 its filter state, +0x38 the right output, +0x40 its
  state, +0x48 the PCM out (1152 interleaved 16-bit samples).
* The overlap and filter state are kept as 32-bit floats inside the buffers the driver gives. They fit, so they
  live in emulated memory and in save states. The real format is UNKNOWN; only the accelerator reads them.
* The DAC's DMA counts 16-bit units (the driver flushes `count * 2` bytes).

Results:

| Clip | Where the PCM was taken | SNR vs. minimp3 |
|---|---|---|
| Start-up logo (mono) | emulated DAC | 44.5 dB |
| Al_Can.mp3 (stereo, 320 kbps) | emulated DAC | 48.0 dB per channel |

The ignored test `mp3acc::matches_a_reference` checks the stages offline (RIBBIT_MP3_DUMP / RIBBIT_MP3_PCM, from a
patched minimp3 build) at 48-49 dB.

The first model kept job A's output as 32-bit floats. That garbled stereo songs, because the right channel's input
overlapped the left channel's output. The mono logo came through fine.

The remaining difference is rounding: the real unit is fixed point, and its exact arithmetic is UNKNOWN.

**To the speaker (fixed in 1.4.3).** 1.4.2 decoded MP3s but still left the DAC out of the speaker mix, an old
stand-in from when only noise reached it, so nothing was heard. Now the DAC is mixed into the sound unit's
output (`sound::tick_sound`): its samples are interpolated at their own times, at a quarter level (`DAC_LEVEL`) so
that after the app's fourfold loudness a full-scale MP3 comes out near full scale. The balance between the DAC and
the sound unit on a real unit is UNVERIFIED. Checked through the speaker: the song's samples come out at 0.250,
33.8 dB above the rest (the rest being interpolation error, measured at the DAC's true rate of 240 MHz / 5442).

Tools: `examples/twistshot.rs` (TWIST_KEYS, TWIST_WAV, TWIST_MP3 job log). The route to the first song: Profiles at
~22 s; Left 24, Enter 26, Left 28.5, Enter 29.5 (Stations), Right x5 from 33 s, Enter 43, Enter 48. "Al Can"
starts at ~54 s.
