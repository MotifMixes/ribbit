# The LeapPad family: My First LeapPad, Little Touch LeapPad, LeapPad Plus Microphone

The original LeapPad is in `docs/leappad.md`. This file covers the three other pads Ribbit plays, all built on
the same LeapFrog FS80A363 chip: the 8051 core, its bank windows, the sound processor, the RIB cartridge check
and the cartridge's own kernel and app work as described there. Only what differs is here. Research started
2026-11-15. Labels (VERIFIED, VERIFIED-FW, EVIDENCE, HYPOTHESIS …) as in `docs/leappad.md`.

## Summary

| Model | Base ROMs | Input | Books found | In Ribbit |
|---|---|---|---|---|
| **My First LeapPad** (2002, 2004) | US 2004 152-10573 (4 MB), International V1.3 152-00932 (1 MB) | pen; one landscape page; GO spots; a template switch | 8 (archive.org) | boots, every spot of The Foot Book answers (501 of 501) |
| **Little Touch LeapPad** (2005) | 152-11244 "Full Base ROM V1.1 - One Bear in the Bedroom" (4 MB), Germany 152-11010 (4 MB) | no pen: a 5 x 12 matrix of switches under two pages; 10 magnet switches read the open page | 6 (archive.org) | boots, every spot of Pooh Loves You! answers (211 of 211), pages opened by their magnets |
| **LeapPad Plus Microphone** (2004) | 152-10793 "MIB - LeapPad Plus Microphone baseROM - US 2004" (8 MB) | the LeapPad's pen, plus a Sunplus PA7790 recorder | LeapPad books | boots, LeapPad books play; recording is **not** emulated |

MAME (`src/mame/leapfrog/leappad.cpp`, BSD-3-Clause, David Haywood) has all of them as stubs: an I8032 at
96 MHz/10, a generic cartridge slot, `MACHINE_NOT_WORKING`, no peripherals. It says the three pads share the
cartridge pinout and that My First LeapPad cartridge boards are the LeapPad's (SOURCED). It lists the PA7790
of the Plus Microphone as `pa7790.u15`, **NO_DUMP**: the recorder's own program is not dumped. Ribbit follows
only these facts; no MAME code is used. Its software lists `leapfrog_mfleappad_cart.xml` (23 entries) and
`leapfrog_ltleappad_cart.xml` (9 entries) gave the names to look for.

## What all three share with the LeapPad (VERIFIED by running them)

* The same boot stub, kernel at 0x10000, app at 0x20000, RIB at 0x10100 and sparse checksum.
* **The product ID check is what refuses other pads' books.** The LeapPad's base and books are ProdID 14,
  My First LeapPad's 13, the Little Touch's 33 (its books carry 48 and a list holding 33). A LeapPad book in a
  My First LeapPad base: "ILA2 RIB: ProdID:14: No matching Base ProdID … CartStatus:5:Bad Cart … Ann msg: not
  LeapPad cart", and the base's announcement plays: the "Whoops! That's not a My First LeapPad cartridge" a
  player heard. Ribbit's app sends each book to its own pad's base (`Model::plays`).
* **On-chip RAM below the registers.** The 2003-2005 kernels point window 4 at bank 0x700 (physical
  0x1C00000) and keep their app's globals at XDATA C000-EFFF ("Clear RAM:View:x:C2C8 to x:C5FF"). Ribbit
  first gave that space nothing (reads FF): the apps then read "hCurView:65535" and stopped at
  "ASSERT:107:pViewData:hView1=hNULL". Ribbit now has 16 KB there, the top 4 KB being F000-FFFF
  (`Bus::sram`). EVIDENCE.
* **GPIO ports with direction registers** (EVIDENCE): FC20 (direction FC34), FC48 (FC4A), FC49 (FC4B); a set
  direction bit drives the latch, inputs are pulled up. The Little Touch reads its switches through them, the
  Plus Microphone talks to its recorder through FC48.
* The idle power-off is the newer kernels' routine with activity bit 0x1F in all 19 firmwares here (the bases
  and every cartridge; checked with the same pattern `ribbit-leappad/src/idle.rs` in the zip-int worktree
  looks for). Left alone they power down after 175 s (My First LeapPad, Plus Microphone) and 300 s (Little
  Touch) (`lpidle`, VERIFIED).

## My First LeapPad

### Bases (A1)

| File | Size | SHA1 | Says |
|---|---|---|---|
| `myfirstleappadbios.bin` | 4 MB | e0ba644fdf38fd5f91ab8c4b673c4a658cc3e612 | "MFLP US Base ROM - 2004", 152-10573, Feb 13 2004, kernel May 20 2003 |
| `myfirstleappadinternational.bin` | 1 MB | 573ecf2efaccf70e619cf54d63be9169e469ee6f | "MFLP International base ROM V1.3", 152-00932, May 07 2002 |

Both are MAME's `mfleappad` BIOSes. The US one is also in "LeapFrog ROM Dumps Collection.zip".

### The template (VERIFIED-FW, kernel 0x00F3 and 0x59BE, The Foot Book 500-10523)

The pad comes with a printed template (the "ABC Phonics Bus" activity) that lies on it instead of a book.
**FC20 bit 3** is its switch: clear while the template is in. The kernel debounces it over 5 reads into F0C3
bit 6; the app logs "Template In: InitialView:…" or "Template Out:" and, with the template in, uses the base's
own activity (the GO list at app header +0Bh instead of +5). Ribbit leaves the template out
(`Bus::template_in`, `lpboot TEMPLATE=1`).

### The pen (VERIFIED by execution)

* The same sequencer (FC16-FC19, INT0) and drive codes, plus the mode-3 cycle `01 00 0A 12`.
* **A test signal.** At power-on the kernel (cart kernel 0x3D89) measures the sensor, turns on **FC30 bit 0**,
  measures again and sets the gain from the difference against a nominal 4000 counts: "DC 8192 Raw AFE FS 3999
  Gain 16384". Without the signal the gain came out -9876 and no touch was ever felt. Ribbit adds 4134 counts
  while it is on (`pen::TEST_SIGNAL`, size ASSUMED: it gives a gain of about 1.0).
* **Its own EEPROM layout** (VERIFIED-FW, kernel 0x3D12, app 0xB12A/0xB48A; `calibration::my_first`): the
  kernel sums bytes 00-4D and wants the ones' complement at 4E-4F ("CS bad No CAL table, Please Calibrate"
  otherwise); byte 0 is 0 ("Recal EE" otherwise). Two regions of 28 bytes, axis B (across) at 08 and axis A
  (down) at 24: a byte, 13 big-endian words (the reading just off the page, then at calibration points 0-11),
  a byte. The two bytes go times 32 into F0A1 and F09B (UNKNOWN; 0 here). The app builds its tables F5C0 and
  F5D6 from the words as the LeapPad's app does (entry i from word i + 1), found with patterned images.
* **One page, landscape.** The cartridges' point tables (17 words each, the same in all 8 cartridges and both
  bases): x -180 … 8580 and y -180 … 5976 eighths, so the app's page is about 1072 x 747 and all touches report
  page 0. Ribbit's factory calibration is again an ideal sensor (axes -14000 … +14000 across the page). `lpgrid
  <bios> <cart> mflp` touches a grid of raw readings: 0 … 1049 across and 0 … 724 down, linear to about 1%.
* Ribbit keeps its EEPROM apart from the LeapPad's (`myfirstleappad.eeprom`).

### The app (VERIFIED-FW; `layout.rs`)

* The header (pointer at 0x1013C) has version 2 or 3; the LeapPad header's bytes 3-16h sit 10 bytes later, so
  the views are at +21h (count) and +23h (table), in the LeapPad's format.
* **The GO list is at +5** (with the template out; app 0xC3B3 in The Foot Book), or for version 3, through the
  word at +45h: a pointer to two list pointers (template out, template in). The GO spots lie along the top and
  left edges of the page.
* Views, clickables and shapes are the LeapPad's. Two 2002 books (Leap's Big Day, El gran día de Croki) have
  cover views whose clickable pointer equals their +0Eh word and whose +10h is FFFF: they point to another
  view's data, whose clickables they use (EVIDENCE).

### Books (A1, all from archive.org, each SHA1 matching MAME's list)

| archive.org item | Cartridge | MAME |
|---|---|---|
| `footbook_202211` | 500-10523 The Foot Book (UK) | footbook |
| `thomasst` | 500-00543 Thomas & Friends - Thomas and the School Trip (UK) | thomasst |
| `leapbigday` | 500-00547 Leap's Big Day (UK) | leapbigday |
| `iknowmyabc` | 500-01120 I Know My abc's (UK, Dec 18 2002) | iknowmyabc |
| `crokis` | 500-11074 El gran día de Croki (Spain) | crokis |
| `vroom_202211` | 500-11282 Vroom! Vroom! - On the Go (UK) | vroom |
| `princess_202211` | 500-11436 Las Princesas Disney (Spain) | princess |
| `schools_202211` | 500-12049 Mi Primer Día de Escuela (Spain) | schools |

Kept in `D:/ribbit-roms/leappad-family/carts/mflp` on the user's PC (never in git). Not found: the 15 other
titles of MAME's list (Dora, The Wiggles, Cars, the French ones …).

## Little Touch LeapPad

### Bases (A1)

| File | Size | SHA1 | Says |
|---|---|---|---|
| `littletouchleappadbios.bin` | 4 MB | 6ec1a47aaef9c9ed134bb143c2631f4d89d7c236 | "Full Base ROM: V1.1 - One Bear in the Bedroom", 152-11244, Mar 10 2005 |
| `leappad_little_touch_german.bin` | 4 MB | 34f1b6e075e10e14380d925944f4c84d068ec58e | "Full Base ROM: V1.0 - Germany", 152-11010, Jan 11 2005 |

Both are MAME's `ltleappad` BIOSes. **The base holds a book**: the US base's "One Bear in the Bedroom" plays
with no cartridge in (its app header is found as a cartridge's, `layout::read_base`; Ribbit draws it).

### Hardware (VERIFIED by execution, Pooh Loves You! 500-11375 on base 152-11244)

* **No pen, no EEPROM in use.** "Hardware Init" lists SwitchInit instead of E2Init/NTInit. The base checks the
  cartridge and runs **its own kernel** with the cartridge's app ("Launch cartridge lpBootApp: 0x01420000",
  no "Launch cart kernel").
* **A switch matrix under the pages** (VERIFIED-FW, base kernel 0x4906): five rows are driven one at a time by
  setting bit r of FC33 (the direction of port FC1F, whose latch is 0), and twelve columns read back low:
  columns 0-11 are FC49 bits 0-5, FC48 bit 4, FC49 bits 6-7, FC48 bits 0, 1 and 3 (table at 0x5627). The key
  code is row and column (C0B2).
* **Keys become points** (VERIFIED by pressing them, `lpboot KEY=r,c,start,len`): key (r, c) reaches the app as
  x = 50 + 100c, y = 50 + 100r on the left page (columns 0-5) and x = 76 + 100(c - 6) on the right page
  (columns 6-11, y + 8192), so each page is a grid of 6 by 5 switches about 100 units square, and the app
  hit-tests its shapes as the LeapPad's does (`book::key_at`).
* **Ten magnet switches name the open page** (VERIFIED-FW, base kernel 0x335E): FC20 bits 0-7, FC1F bit 5 and
  FC48 bit 2, low where the page has a magnet; debounced over 100 reads (about a second). The pages' magnets
  give each spread its own pattern (Pooh Loves You!: 03E1, 03E3, 03E7 … 001F; 0 is no book). The app logs
  "MagnetSwitch: pData: x:0148 S:03E1 V:2|02".

### The app (VERIFIED-FW; `layout.rs`)

* The header (version 1 or 2, "5A 0A 7F 5A 0A 73" at +9): views at +1Fh/+21h; at +23h an optional list of
  page-pointer keys in the GO format ("GlobalPP"); at +3Eh the **magnet list**: a pattern and a pointer to the
  view record (as in the GO list), up to 0FFFh, the default (traced with `lpboot PWATCH`).
* View data: byte +1 is 1 for Touch and Tell (2 for activities); the clickables are at +12h. Clickables and
  shapes are the LeapPad's.

### Books (A1, archive.org, SHA1 matching MAME's list)

| archive.org item | Cartridge |
|---|---|
| `poohlovesu` | 500-11375 Disney Pooh Loves You! (USA) |
| `poohlovesug` | 500-12032 Disney Puuh mag dich! (Germany) |
| `babyfwords` | 500-10247 Baby's First Words (UK/USA) |
| `babyanimlsg` | 500-11543 Tierbabys (Germany) |
| `ifiwere` | 500-11468 If I were... (USA) |
| `ifiwereg` | 500-11542 Was wäre, wenn ...? (Germany) |

Not found: Animal World, Animal Dance, One Bear in the Bedroom as a cartridge (it is in the US base).

## LeapPad Plus Microphone

* **Base** (A1, from the user's "LeapPad BIOS set.7z"): `leapfrogvpadwithmic.bin`, 8 MB, SHA1
  fa163661260942eab92e34ae802b3da0a6130a39: "MIB - LeapPad Plus Microphone baseROM - US 2004", 152-10793, Apr 29
  2004, kernel Mar 21 2004. MAME's `leappadmic`. It occupies chip selects 0 and 1.
* **It is a LeapPad** (VERIFIED): ProdID 14, the LeapPad's pen and EEPROM; Tutter's Tiny Trip and Cars play
  on it, GO and spots answer. Its own app is "Prebuilt RTnT, QnA4, Atlas&Eureka, Announce Cart Status".
* **Without a cartridge** it says "Recorder required by book and hardware is not present. Launching
  RecNotPresentInitialView: 30" (view NoMicPowerOn).

### The recorder (EVIDENCE from the 8051 side only; not emulated)

The PA7790 is a separate Sunplus chip with its own, undumped program. What the base's kernel does with it
(RecInit, kernel 0x34D2-0x35E3; the handshake in the interrupt code at 0x3A43-0x3C56):

* FC20 bit 1 (an output, FC34 = 02) powers or enables it.
* **Internal recorder:** FC48 bit 0 is the 8051's request line and bit 1 a second output (FC4A = 03); FC48
  bit 3 is the recorder's **ACK**. Detection waits for ACK = 0, toggles the request, and calls the recorder
  present if ACK follows ("Internal Recorder Detected, ACK changed to 0x8"). FC49 bit 7 (an output, FC4B = 80)
  is a busy flag the interrupt code clears.
* **Pass-through recorder** (a cartridge-side one, perhaps MAME's "Microphone Upgrade Kit"): the same lines at
  XDATA 7FFF through window 2, at the cartridge: bit 0 ACK, bit 1 request, bit 3 the second line, bit 2 busy.
* The handshake writes sound-processor commands to FC1C as it goes (start 80h + voice, stop): the recorder
  works with the FS80A363's sound processor, not around it. The app's recorder layer reports "ASIC recorder
  command set", "SunPlus recorder command set" and "SunPlus code version" and keeps **record buffers** with
  persistence levels.
* With ACK made to follow the request (an experiment, removed again), the base finds the recorder and then
  waits forever after "O 99 - N 0" for its version answer. The command set, the data path of the samples and
  where the buffers live are UNKNOWN. Emulating recording needs them; with the PA7790's program undumped they
  have to be read from the base's 8051 code (RecorderCommandDialogue.c, AppRecorderInit.c in its asserts).
* So Ribbit's Plus Microphone has **no recorder**: ACK reads 1 (pulled up), "Internal Recorder Not Detected",
  and the books that do not need one play. Books that require it would play their "not present" views. No
  Plus Microphone book dump has been found to try.

## The app (ribbit-frontend)

* Base ROMs are told apart when opened (`Model::of_base`) and kept by model (`leappad_bases` in the
  settings; the LeapPad's stays `leappad_bios`). A cartridge goes to its own model's base
  (`Model::of_cartridge`, from the app header); LeapPad books use the LeapPad's base, or the Plus Microphone's.
* My First LeapPad: one landscape page, the pen as on the LeapPad. Little Touch: two pages, no pen (a press
  of the mouse is a hand on a switch); turning the page sets the spread's magnets, so no GO touch is needed;
  with no cartridge the base's own book opens. Each model's icon (`console_icons::paint_leappad_model`) marks
  its books in the library.
* "Turn off when idle": all three use the LeapPad's idle routine (bit 0x1F), so the LeapPad's keep-awake
  covers them once it is in (being added in the zip-int worktree).

## Reproducing

```
lpboot <base> <cart> 150                       # FACTORY=1 now gives the model's own factory EEPROM
lpboot littletouchleappadbios.bin pooh.bin 100 MAGNETS="3E1@20" KEY="2,5,60,3"
lpgrid myfirstleappadbios.bin footbook.bin mflp [-0.95,0.95,-0.95,0.95,5]
lplayout <cart> [all]                           # also prints each spread's magnets
lpboot <base> <cart> 60 PWATCH=1440000,1440400,40   # data reads of a physical range after 40 M instructions
RIBBIT_LEAPPAD_BIOS=<base> RIBBIT_LEAPPAD_CART=<cart> cargo test -p ribbit-leappad --release --test books -- --ignored --nocapture the_cartridge_answers_every
```

## Open questions

1. The PA7790's command set and sample path (record, play back, buffers): needs the base's recorder code read
   further, or a real Plus Microphone to watch.
2. Plus Microphone books (with recording pages) and the "Microphone Upgrade Kit" cartridge: no dumps found.
3. My First LeapPad's template activity in the app (the template on the pad), and the EEPROM bytes 08/23
   and 24/3F.
4. The Little Touch's real magnet layout (which page carries which magnet) is not needed: the cartridges give
   the patterns.
5. The test signal's size and the AFE gain on real hardware.
