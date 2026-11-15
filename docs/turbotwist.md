# LeapFrog Turbo Twist Extreme

The Turbo Twist family (Quantum Leap brand) are learning toys with a handle
that twists, a small LCD and cartridges: Turbo Twist Spelling (2000),
Vocabulator (2001), Math and Brain Quest (2002), Fact Blaster, and the Turbo
Extreme (2004), whose cartridges cover maths, spelling, science and social
studies and do not fit the older models. Ribbit emulates the Turbo Twist
Extreme in `crates/ribbit-iquest` (shared with the iQuest, docs/iquest.md;
EXPERIMENTAL), on the app side `crates/ribbit-frontend/src/iquest.rs`.

Research started 2026-11-15. Labels as in docs/leappad.md.

## Sources

| # | Source | Notes |
|---|---|---|
| S1 | MAME `src/mame/leapfrog/iquest.cpp` (BSD-3-Clause) | `turboex` "Turbo Extreme (US)", a stub on the iQuest driver: screen "64 x 32, unknown resolution, lower than iquest"; software list `turboextreme_cart`, which MAME does not have. The older models (`ttwistm`, `ttwistsp`, `ttwistvc`, `ttwistbq`, `ttwistfb`, 1-2 MB ROMs, "single row display") are on the same chip family. |
| S2 | MAME `hash/ttwist_brainquest_cart.xml` | One Brain Quest cartridge (Grades 5 & 6); not for the Extreme. |
| S3 | The user's `turbotwistextreme.bin` (4 MB, SHA1 7813811c…, MAME's `turboex`) | Base ROM 152-10696 "TTExtreme FullBase V1.2", built Mar 05 2004. |

## Hardware

- **Chip (EVIDENCE):** the LeapPad's 8051 ASIC, as the iQuest (header
  "ToolPad V2.3.2 TTExtreme FullBase V1.1.0"; same boot stub, windows,
  sound processor, counter, serial debug log).
- **LCD (VERIFIED, pictures whole):** 96 x 32 pixels (MAME's 64 x 32 was a
  guess). `FCE3` = 5Fh (96 columns), `FCE4` = D0h (XDATA C680), `FCE2` =
  83h (4 pages). The firmware draws at C500 and copies each page with its
  columns reversed to C680, so the glass shows column 0 at the right
  (EVIDENCE: C680 read left to right is mirrored text); Ribbit mirrors it
  back.
- **E2 (VERIFIED: the kernel reads and writes it, CRC checked):** settings,
  rank, points and high scores live in an EEPROM behind the chip's transfer
  unit, `FC10-FC15` (crates/ribbit-iquest/src/e2.rs): FC14:FC15 address,
  FC10 bit 2 sequential mode, FC11 write a byte / start a read, FC13 the
  byte read, FC00 bits 0, 6, 7 done / byte waiting / busy. Size a HYPOTHESIS
  (8 KB). Kept as `turbotwist.e2` in the save folder.
- **Keys (VERIFIED: every pair of the scan's pins joined at the menu, the
  menu app printing what it got):**

  | Key | Wiring | Menu app printed |
  |---|---|---|
  | D-pad left / up | port A bit 0 to port B bit 0 / 1 | {DPadLf} / {DPadUp} |
  | D-pad right / down | port A bit 1 to port B bit 0 / 1 | {DPadRt} / {DPadDn} |
  | Twist up slow / fast | port A bit 3 to port B bit 3 / 4 | {TwistUpSlow} / TwistUpFast} |
  | Twist down slow / fast | port A bit 4 to port B bit 3 / 4 | {TwistDownSlow} / {TwistDownFast} |
  | Repeat, Slam, Menu | port B bits 5, 6, 7 to ground | {Repeat}, {Slam}, {Menu} |
  | Volume down / up | port A bits 6 / 7 to ground | {VDown:n} / {VUp:n} |

  Port A is FC1F (data) / FC33 (direction), port B FC49 / FC4B. What the
  real buttons look like is UNKNOWN (no photo found): the names are the
  firmware's.
- **Cartridges:** at chip select 5 like the iQuest's (the kernel probes for
  them: "CartStatus", "No Cart"). No Turbo Extreme cartridge dump was found
  (archive.org, MAME, the LeapFrog collections there): untested.

## The shop demo ("TryMe")

As on the iQuest, a memory loss starts TryMe (view 8) and E2 holds the
state afterwards. Ribbit reads the default view byte at 0x7000E (08h) as
view 1, "Menu" (found by patching it to every value and reading the view
the log named: 0 PowerDown, 1 Menu, 2 MissingLetter, 3 MathFactory, 4
BlastOff, 5 HazardSpelling, 6 HazardMath, 7 HallOfDoors, 8 TryMe, 9-10
TrapDoor).

## What works

- Boots to its menu (Hazard Spelling, Hazard Math, Missing Letter, …) with
  the monkey host's animations, speech and music.

## Reproducing

```
LOG=1 KEYS="16:down,17:slam" target/debug/examples/iqshot turbotwistextreme.bin - 20 out 2
```
