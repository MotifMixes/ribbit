# LeapFrog iQuest

The iQuest (Quantum Leap brand, 2002-2004) is a handheld for ages 10 and up:
a quiz and homework machine with a QWERTY keyboard, a dot-matrix LCD and
cartridges of textbook questions, plus organiser apps (schedule, address
book, notepad, calculator, dictionary). Ribbit emulates it in
`crates/ribbit-iquest` (EXPERIMENTAL), on the app side
`crates/ribbit-frontend/src/iquest.rs`. The same crate runs the Turbo Twist
Extreme (docs/turbotwist.md): both are built on the LeapPad's chip.

Research started 2026-11-15. Labels as in docs/leappad.md: VERIFIED (checked
by running), EVIDENCE (read from the firmware), SOURCED, HYPOTHESIS, UNKNOWN.

## Sources

| # | Source | Notes |
|---|---|---|
| S1 | MAME `src/mame/leapfrog/iquest.cpp` (BSD-3-Clause, David Haywood) | A stub, `MACHINE_NOT_WORKING`: I8032 at an unknown clock, a 90 x 64 screen guessed from two 0x2D0-byte buffers at C260 and C530, bank registers FC06-FC0B. Also lists the Turbo Twist family. Nothing copied; its facts were checked against the ROM. |
| S2 | MAME `hash/leapfrog_iquest_cart.xml` | Two cartridges: `start40` (Starter Pack, SHA1 4bc8533e…) and `scig5`. |
| S3 | archive.org `iquesthandheld` | 13 cartridge dumps (below) and a box photo (the device: blue-grey, screen at the top, purple D-pad at the left, round Enter at the right, the keyboard below with Space/@ and Menu). |
| S4 | The user's `iquest.bin` (4 MB, SHA1 ec002c18…, MAME's `iquest`) | Base ROM 152-10663 "IQuest Full BaseROM - Version 3.21", built Feb 12 2004. |

## Hardware

- **Chip (EVIDENCE):** the LeapPad's 8051-family ASIC (docs/leappad.md):
  the ROM starts with the same boot stub (`02 00 03 90 FC 06 …`), the
  header at 0x38 reads "ToolPad V2.3.7 IQuest FullBase V1.1.0", and the
  kernel uses the same bank windows (FC06-FC0F), counter (FC01-FC04), sound
  processor (FC1C/FC1E, shared RAM F462…), delay counter (FCE5) and debug
  serial port. Ribbit reuses `ribbit-leappad`'s 8051 core, sound and
  windows. Clock UNKNOWN (the LeapPad's 4 000 000 machine cycles a second).
- **On-chip RAM (EVIDENCE):** 16 KB at physical 0x1C00000 (chip select 7),
  seen through window 4 at XDATA C000-FFFF; F000-FFFF is its top 4 KB.
- **LCD (VERIFIED, pictures whole):** 90 x 64 pixels, one bit each, shown by
  the chip's LCD controller from on-chip RAM: `FCE3` = columns - 1 (59h),
  `FCE4` = buffer address / 8 (A6h: XDATA C530), `FCE2` bits 0-2 = pages - 1
  (87h), bit 7 on; `FCE1` = 12h (UNKNOWN). A byte is an 8-pixel column of a
  page, bit 7 at the top. The firmware draws at C260 and copies to C530.
  Set bits are drawn dark (HYPOTHESIS; the pictures look right that way).
- **Flash (VERIFIED: found, formatted, user state written):** the "secondary
  (local) flash" at chip select 4 (0x1000000) keeps the user's name,
  options, notes and schedule. The kernel identifies the part from a table
  (ROM Info Block +7Ch → four access methods, each with ID lists: Intel
  0x90; JEDEC 5555/2AAA byte mode; AAAA/5555 word mode). The part in a real
  iQuest is UNKNOWN; Ribbit fits an SST 39SF040 (BF D7, 512 KB, 4 KB
  sectors), which the kernel accepts. Its storage area is 0x70000-0x7FFFF.
  Kept as `iquest.flash` in the save folder.
- **Keys (VERIFIED by the firmware's own log):** a 6 x 8 matrix; column c is
  driven low on port B (FC49 data, FC4B direction) bit [0,1,2,3,4,7][c], row
  r read on port A (FC1F) bit r, active low. The kernel's own table at ROM
  0x24917 names the 48 keys: `12345678 QWERTYUI ASDFGHJK ZXCVBNM. __aLPO09
  durl-+m_` (`d u r l` the arrows, `-`/`+` volume, `m` Menu, `_` Space,
  `a` Repeat). Enter and Del are not in the matrix: they pull port C (FC48)
  bits 0 and 1 to ground.
- **Cartridges (VERIFIED: the base accepts them, "Cart OK"):** 8 MB data
  images (4 MB for the Starter Pack) at chip select 5 and on through 6
  (0x1400000-0x1BFFFFF); their pointer tables address 0x014xxxxx-0x017xxxxx.
  They hold no code of their own (no boot stub). The base checks the RIB at
  +0x10100 and a sparse checksum, as the LeapPad's does.
- **RTC:** the menu shows a date and time (1-1-01 12:00A at first start).
  Registers FCDF-FCE0, FCE6-FCE7 are read and written around it (UNKNOWN).

## Dumps found (S3)

All 8 MB unless noted; in `D:/ribbit-roms/iquest/carts`.

| Cartridge | Part | SHA1 (first 12) |
|---|---|---|
| iQuest 4.0 Starter Pack (4 MB) | 500-12323-A | 4bc8533e0266 (= MAME start40) |
| iQuest 4.0 Math Grade 5 | 500-12322-A | 34af1af7d79d |
| iQuest 4.0 Math Grade 6-8 | 500-12324-A | aa1c23c58b65 |
| iQuest 4.0 Science Grade 5 | 500-12328-A | f15b9b5df39f |
| iQuest 4.0 Science Grade 6-8 | 500-12326-A | 322a26614e99 |
| iQuest 4.0 Social Studies Grade 5 | 500-12327-A | 83d7a006e7dd |
| iQuest Math Grades 6-8 A / B | 500-00747 / 00749 | 1c1f97f3a700 / d4f02208ecd3 |
| iQuest Science Grade 5 A / B | 500-00750 / 00752 | a36e33aa64c6 / c669aa7bbe2b |
| iQuest Social Studies Grade 5 A / B | 500-00754 / 00756 | 1c8cb37da832 / 7fe41162b4b7 |
| iQuest Test Prep SAT/PSAT & ACT 2002 (8 388 564 bytes) | 500-00758, 500-00932 | 4966b53a67e5 |

## The shop demo ("Try Me")

After a memory loss (new batteries, or Ribbit's first start) the base's
default view is Try Me: a 53-second demo that walks through a chapter
challenge, says goodbye and powers off. It takes only Enter, Del and the
volume keys, none of which leave it. At the next power-on the view comes
from the user's state in flash, which only the Options app writes ("Updating
user state to flash"); without it, Try Me again. How a real unit leaves Try
Me is UNKNOWN (a factory-written state, a pull tab?).

Ribbit's choice (`Bus::skip_try_me`, on): the base's default view byte
(resource at 0x130000, +21h = 0Fh "Try Me") reads as its menu view (+24h =
08h "Menu"). Found by tracing the view look-up that follows "AppRAMInit";
the views are numbered by the name table at ROM 0x133D4B (0 Chapter
Challenge … 8 Menu, 9 Power Down, 10-12 TrapDoor, 15 Try Me).

## What works (2026-11-15)

- **Boots** to its logo, then (Try Me skipped) the menu: Chapter Challenge,
  Pop Quiz, Address Book, Schedule, Notepad, Dictionary, Calculator, with
  the date and time on the last line. VERIFIED by frames.
- **Cartridges:** 11 of the 13 dumps pass the base's check ("Cart OK") and
  start their intro ("Starter Pack 5th-8th Grade"); Science Grade 6-8 needs
  its checksum repaired (stored FFFFh, the app repairs it as it does a
  LeapPad cartridge's, `ribbit_iquest::repair_checksum`). Two dumps are bad:
  Math Grade 5 4.0 ("ILA1 RIB: Bad Security string": its ROM Info Block is
  not where the others' is) and Test Prep (44 bytes short; its RIB sits 21
  bytes early, and with 21 bytes put back the sum still fails).
- **Plays:** Enter on Chapter Challenge, Down to Science, Enter: its
  animation and questions. Every key reaches the firmware (`{Enter}`,
  `{Del}`, `{MenuDT}`, `{UArrowDT}`, `{RepeatDT}`, `{Other alpha
  keypressDT}`, `{VUp:127}`).
- **Sound:** speech and music play (14 s from power-on: RMS 1000-2100 a
  second, peaks 8191). Not compared with a real unit.
- **Idle:** the menu turns the iQuest off after about 2 minutes ("Auto Power
  Down"); with "Turn off when idle" off Ribbit refuses that view change
  (`Bus::keep_awake`, see bus.rs `IdleGuard`) and the app starts over.

- **In the app:** Library tab "iQuest" (cartridges, and the base ROM alone
  as "iQuest (no cartridge)"); the PC's letters, digits, space and '.' type,
  Enter is Enter, Backspace/Delete Del, arrows the D-pad, Home Menu, End
  Repeat, -/= volume, Pause ON/OFF. The flash is kept as `iquest.flash` in
  the save folder.

## Not done / unknown

- The clock speed, the RTC registers, the LCD's contrast and grey levels.
- How a real unit leaves Try Me (above).
- Save states (the session has none yet).
- Whether `FCE2` bit 6 (set when FC2E bit 4 is) flips the LCD.

## Reproducing

```
cargo build -p ribbit-iquest --examples
target/debug/examples/iqboot iquest.bin cart.bin 10          # debug log, unknown registers
LOG=1 KEYS="14:enter,17:down" target/debug/examples/iqshot iquest.bin cart.bin 26 out 2
WAV=out.wav target/debug/examples/iqshot iquest.bin cart.bin 14 out
```

`iqshot` writes the LCD as PGM files; KEYS takes key names (keys.rs) or raw
pins (`pin:C0`, `pin:B3-A2`).
