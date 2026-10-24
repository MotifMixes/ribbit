# LeapFrog LeapPad (original, 1999)

The LeapPad is a book-and-pen learning toy, not a handheld with a screen. A spiral-bound paper book lies on the
base. A cartridge for that book goes in the slot. A pen on a cord picks out spots on the printed pages, and the
base speaks. It has nothing in common with the Leapster hardware, and it is not the later LeapPad Explorer/LeapPad2
tablets.

Ribbit's model is `crates/ribbit-leappad`. Everything below was learned from the dumped firmware: reading its
code and running it on Ribbit's 8051 model with `examples/lpboot.rs`. It is **not** checked on real hardware
unless it says so. Research started 2026-10-23.

## Status (2026-10-23)

| | |
|---|---|
| **Verified working** (tests or the firmware's own log) | 8051 core (`tests/mcs51.rs`); bank windows; boot of the real base ROM; cartridge check and launch of the cartridge's kernel and app; I²C EEPROM with a factory calibration the kernel accepts; pen sensor sequencer and INT0 delivery; pen position to the cartridge's page coordinates (≤ 0.63% error on 24 targets); GO touch changes the cartridge's view; the cartridge's touch layout (every spot of every spread, and the GO spots) read from the cartridge: all 914 spots of Tutter's Tiny Trip answer where the layout says (`the_cartridge_answers_every_spot_its_layout_lists`), as do those tried on three other cartridges; sound: the start-up music and the cartridge's speech play (`the_cartridge_plays_music_and_speaks_at_power_on`); base ROM / cartridge detection; virtual book format, checks and the test book (`tests/books.rs`) |
| **Partly working** | Timers (T0/T1 counters on a 16 kHz clock: EVIDENCE for the ratio, the rate measured against a recording of the power-on tune); the seconds counter (EVIDENCE, 1 Hz); the ON/OFF switch (reaches the firmware's power-down, which then waits for sound); the frontend's LeapPad view (drawn pad, book pictures, pen cursor, debug panel; mouse to pen checked through the core, not yet by clicking in the window) |
| **Experimental / stand-ins** | CPU speed (4 000 000 machine cycles/s: at least 2 000 000 needed); pen sensor counts and conversion time; the ideal-sensor calibration; FCE5 delay counter rate; sound processor command acknowledgement (100 cycles); the sound's pitch scale and sample rate (taken from the Leapster); what the sound commands other than start and stop do |
| **Unknown / not emulated** | The internal boot ROM; FC3F/FC2E-FC2F, FC2B-FC2D, FC31-FC36, FC79-FC86 and other registers in the table below; cartridge removal while running; the finger-well page-turn sensor; recording (LeapPad Plus); the touch layout of question and game views, and Classic mode 4 |

Save states are not offered yet: the machine's state is serialisable except the ROMs, but loading one has not
been checked.

## Confidence labels

| Label | Meaning |
|---|---|
| **VERIFIED** | Checked by Ribbit against the user's dumps, with a reproducible command (see "Reproducing"). |
| **EVIDENCE** | Read from the firmware's code or behaviour and consistent with everything seen, but one step short of proof (e.g. the meaning of a register bit inferred from how the code uses it). |
| **SOURCED** | Stated by an outside source (patent, manual, MAME), not checked by Ribbit. |
| **HYPOTHESIS** | An inference that fits the evidence but is unproven. |
| **UNKNOWN** | No usable information yet. |

## Sources

| # | Source | Notes |
|---|---|---|
| S1 | MAME `src/mame/leapfrog/leappad.cpp` | A stub: I8032 core at 96 MHz/10, cartridge slot, no peripherals, `MACHINE_NOT_WORKING`. Names the main chip "LeapFrog FS80A363" with an undumped internal ROM, and gives the cartridge pinout. |
| S2 | MAME `hash/leapfrog_leappad_cart.xml` | 159 cartridges with hashes; all `supported="no"`. |
| S3 | US patent 5,686,705 "Surface position location system and method" (Explore Tech / Knowledge Kids, 1996) | Pen sensing. Named on the 2001 parent guide (S5). |
| S4 | US patents 6,661,405 (2000) and 6,668,156 (2000) | Two-antenna sensor refinement; GO buttons, finger wells, page registration. |
| S5 | "LeapPad Classic" parent guide, 2001, part 600-00564 (t7.leapfrog.com/images/l/LeapPadClassic.pdf) | Use, controls, patent numbers. |
| S6 | BLiNX's LeapFrog-Tools (GitHub) | `LeapPadSplit.py` only splits ROMs into 64 KB banks; no format knowledge. |
| A1 | The user's dumps (see "Dumps") | Base ROM and 20 cartridges. |

## How the toy is used (SOURCED, S4/S5)

* The pen does not need to touch the page: it is the **"Neartouch" pen**, an antenna that senses through the
  paper.
* **Nothing in the book is electronic.** The book has no ID chip, no conductive ink and no printed codes. The base
  knows which book it is from the cartridge (one cartridge per book).
* **Which page is open:** every page has a green **GO** spot printed in its border, in a slightly different place
  on each page. "You must touch the GO every time you turn the page" (S5). The position of the touch identifies
  the page (S4). Ribbit's firmware reads agree: the kernel and apps talk about *spreads* (two facing pages) and a
  "PagePointer" touch (A1).
* **Finger wells** (S4): an infrared beam at the page edge notices a page being turned, and the base then says
  "Touch the green Go button". Strings `FingerWell`, `FWellMDGV_DTGAS`, `_HoldGAS`, `_UTGAS` and `_PromptGAS` in
  cartridges (A1) match this. Whether the 1999 base has the beam is a HYPOTHESIS (no board photo yet).
* **Controls are printed, not physical**: STOP, volume up/down, Say It, Sound It, Spell It and game icons are spots
  on the page. The only physical button is ON/OFF (S5).
* Pages register on tabs in the base, so a given spot on a given page always lands on the same sensor position
  (S4).

So **a "book dump" does not exist**: a book is paper plus positions. The positions of everything touchable are
in the cartridge (see "What the cartridge holds"). Ribbit's virtual book is page pictures (supplied by the user,
or drawn for test books) plus those positions.

## Processor (VERIFIED)

* An **8051-family core**. The ROMs are plain MCS-51 machine code built with Keil C51: the base ROM starts
  `02 00 03 90 FC 06 E0 F8` (`LJMP 0003; MOV DPTR,#FC06h; MOVX A,@DPTR; MOV R0,A`). The kernel's reset code
  is Keil's standard startup (clear IRAM, `SP = AFh`, walk the init table, jump to main). The library helpers
  are Keil's too (`?C?LSHL` at 2CCB, `?C?ULSHR` at 2CB8, printf at 2E08).
* The chip is LeapFrog's own (marked FS80A363 per S1). Around the 8051 core it has registers at XDATA
  `FC00-FCFF`, on-chip XDATA RAM and a banking unit. None of that is documented anywhere we found.
* 256 bytes of internal RAM (8032/8052 style): the stack is at `AFh`, above the 128 directly addressable bytes.
  EVIDENCE.
* Standard 8052 timers, serial port and interrupt vectors are used (vectors in the base kernel: reset `5DFF`,
  EX0 `6C2F`, T0 `6C71`, EX1 `6B43`, T1 `6C50`, serial `6C92`, T2 → `0006`).
* **Clock: UNKNOWN.** MAME guesses 9.6 MHz (800 000 machine cycles/s at 12 clocks each). Measured lower bound
  (VERIFIED with `lpboot` and `HZ=`): the cartridge kernel stalls after "Ex1IntInit()" below about 2 000 000
  machine cycles per second, because its 2.5 kHz EX1 work never finishes. So the chip is much faster than a
  classic 8051 at 9.6 MHz. Ribbit uses 4 000 000 as a stand-in.
* **Timers (EVIDENCE):**
  * TMOD = 66h: timers 0 and 1 are both *counters* in auto-reload mode, counting a clock on their pins.
    * T0 reloads every 160 counts. T0's handler (5C33) is the system timebase, with software timers at ×1, ×10
      and ×100 ticks (F00D, F00A-F00C, F006/F008).
    * T1 reloads every 64 counts. Its handler (6D13) counts a 3-bit tick and then **raises EX1 in software**
      (TCON.3) for deferred work.
    * The T0 handler subtracts 25 T1 ticks every 10 T0 ticks, so T1 overflows exactly 2.5× as often. That holds
      only if both pins see the same clock.
    * **The pins' clock: 16 kHz (VERIFIED against a recording, 2026-10-24),** so T0 ticks every 10 ms. The
      music sequencer steps on T0. A recording of a real pad's power-on tune (G3, B4, D4 … then a G chord) has
      notes at 0, 0.17, 0.335 and 0.5 s; Ribbit at 16 kHz gives 0, 0.164, 0.333 and 0.497 s.
      * 160 kHz (a 1 ms tick, the first guess) played the music 10 times too fast, and 32.768 kHz twice too
        fast.
      * 16.384 kHz is 3% off, 15.625 kHz 1.6%. The crystal itself is UNKNOWN.
      * `PINHZ=n` in lpboot and lprec tries others.
  * T2CON = 36h: timer 2 is the serial baud generator, also counting an external clock.
* **Internal boot ROM: UNKNOWN.** S1 lists an undumped FS80A363 internal ROM. The external ROM boots by itself
  from address 0 in Ribbit (below), so the internal ROM is not needed to start. What it would do on real
  hardware (e.g. a factory test or download mode) is unknown.

## Memory map

### Bank windows (EVIDENCE, read from the kernel's address helpers at 0x0020 and 0x586B/0x63B4; boot VERIFIED)

The 8051's 64 KB code space and 64 KB data space are views into a **32 MB physical space** through five
windows:

| Window | Set by writing | Unit | CPU addresses |
|---|---|---|---|
| 0 | `FC06:FC07` (high:low) | 32 KB | code `0000-7FFF` |
| 1 | `FC08:FC09` | 32 KB | code `8000-FFFF` |
| 2 | `FC0A:FC0B` | 32 KB | XDATA `0000-7FFF` |
| 3 | `FC0C:FC0D` | 16 KB | XDATA `8000-BFFF` |
| 4 | `FC0E:FC0F` | 16 KB | XDATA `C000-FFFF` (but see below) |

* Physical address = bank × unit + offset. The helper that converts back masks bank numbers to 10 bits (32 KB
  windows) or 11 bits (16 KB windows).
* **Read-back is packed.** Reading `FC06` gives the high bits of windows 0 and 1 (bits 0-1 and 2-3). Reading
  `FC0A` gives those of windows 2, 3 and 4 (bits 0-1, 2-4, 5-7). The low bytes read back as written.
* All windows are 0 at reset (HYPOTHESIS, consistent with the boot stub below).
* XDATA `F000-FFFF` is **on-chip RAM** with the register page `FC00-FCFF` inside it, whatever window 4 holds
  (EVIDENCE). The kernel sets window 4 to bank 0x700 (physical 0x1C00000), which may be where the on-chip space
  lives physically. A RAM-size probe at kernel 0x6AC0 writes `F200` and checks that `F000` did not change. Ribbit
  gives 4 KB.

### Physical space (32 MB, chip selects every 4 MB)

| Physical | What | Confidence |
|---|---|---|
| `0000000-03FFFFF` | **CS0: base ROM** (1 MB in the ILA2 base, mirrored) | VERIFIED (boots) |
| `0400000-07FFFFF` | CS1: probed with 55/AA writes at kernel 0x8B59, probably for optional RAM: "Recorder Not Detected" follows | EVIDENCE |
| `1400000-17FFFFF` | **CS5: cartridge** (up to 4 MB) | VERIFIED: the base reads and checksums the cartridge here, and runs its kernel ("Starting Kernel in CS5 / Addr: 0x01400000") |
| `1C00000-` | CS7: probably on-chip (window 4 is pointed here) | HYPOTHESIS |
| other | Nothing seen yet | UNKNOWN |

The base's download monitor ("Checksum Location, 0 = CS0, 5 = CS5", "Duplicate CS0") uses the same chip select
numbering.

### Boot stub (VERIFIED by execution)

Both the base ROM and every cartridge start with the same 0x34-byte stub:

1. `LJMP 0003`, then copy `FC06:FC07` (window 0) into `FC08:FC09`. Window 1 now shows the same 32 KB as window
   0.
2. Jump to `801E`, the same code seen through window 1.
3. Add 2 to `FC07`. Window 0 moves 64 KB on, to the kernel image at ROM 0x10000.
4. `LJMP 0000`: the kernel's reset vector.

So each **kernel is a 64 KB image at ROM offset 0x10000**: its low half stays in window 0 and its high half
(0x18000) is in window 1. **Apps run in window 1** from 32 KB banks that begin with their own entry jump (base
banks 0x20000 and 0x28000 begin `LJMP 8F98`).

## Registers at XDATA FC00-FCFF

What each register does, as far as the code shows. "Seen" means accessed in the first 30 million instructions
with cartridge 500-00406.

| Reg | Use | Confidence |
|---|---|---|
| `FC00` | Status, write 1 to clear: bit 0 transfer done (`FC10-FC15`), bit 3 ON/OFF switch, bit 4 sound processor took a command | EVIDENCE (see "Audio") |
| `FC01-FC04` | 32-bit counter, the firmware's clock (count >> 3) | EVIDENCE; rate HYPOTHESIS (see "Time") |
| `FC1C` | Sound processor command: command << 3 \| voice (`80h` start, `40h` stop; `10h`, `20h`, `08h` see "Audio") | EVIDENCE |
| `FC1E` | Voices playing: voice 0 in bit 7, voices 1-7 in bits 0-6 (the Leapster's layout) | EVIDENCE |
| `FC2B:FC2C` | Set to 7FFF by DSPInit | HYPOTHESIS: output gain |
| `FC3D` | Sound processor hold/run (09/08 then 05/04 in DSPInit) | EVIDENCE |
| `FC06-FC0F` | Bank windows (above) | VERIFIED |
| `FC10`, `FC11`, `FC13`, `FC14` | Transfer unit: `FC14:FC15` written, `FC13` read back after `FC00.0`, `FC10` command (01, 03, 04) | EVIDENCE (maybe the "fast EEPROM"/flash interface) |
| `FC16`, `FC17` | Written 14/04 and 01 around power checks | UNKNOWN |
| `FC1F`, `FC33` | Port data and direction: the I²C EEPROM bus (bit 2 SCL, bit 3 SDA) | VERIFIED by execution (see "Serial EEPROM") |
| `FC16` bit 4 | Pen sequencer reset (low = reset) | EVIDENCE |
| `FC17` | Pen sequencer: queue a drive code | EVIDENCE |
| `FC18`, `FC19` | Pen sequencer: result high, low byte | EVIDENCE |
| `FC22` | Written 01 constantly from the app's loop | HYPOTHESIS: watchdog kick (strings "WDOG: …") |
| `FC2A` | **Bit 6: a cartridge is in** (set = present). Bit 1: a pen result is waiting. Bit 2: the pen queue has room | Bit 6 EVIDENCE (the kernel's probe at 0x36B1 prints "Cart found:Yes" exactly when it is set); bits 1-2 EVIDENCE (pen handler); bit 7 UNKNOWN |
| `FC2B`, `FC2C` | Written 7F, FF | UNKNOWN |
| `FC2D` | Written 48, 58, 78 at power-on | UNKNOWN (pen sensor setup?) |
| `FC2E:FC2F` | 16-bit reading taken after `FC3F` is set (kernel 0x325E: `FC3F = FFA8 & 9Dh`, then read FC2E, FC2F); also read by the serial receive handler. **`FC2F` bit 0: a cartridge is in** (the Canada Full Base's probe, kernel 0x600D-0x6038, reports none unless it is set, before it checks `FC2A` bit 6); `FC2F` looks like an input port whose latch is `FC3F` (the ILA2 kernel writes its shadow & 9Dh to `FC3F` and reads `FC2F` back into it, kernel 0x36A0) | Bit 0 EVIDENCE; the rest UNKNOWN. **Not the pen** (that is FC17-FC19); first thought so, wrongly |
| `FC3D`, `FC3E` | Written 08/04 and 18; FC3E read | UNKNOWN |
| `FC3F` | Control bits (00, 01, 08, 09, 0C, 0D seen; code masks with 9Dh); bit 0 set around serial receive | UNKNOWN |
| `FC31-FC36`, `FC4E`/`FC4F` | Set up at init (power-down and cartridge paths) | UNKNOWN |
| `FC79-FC86` | Written once with one of two value sets depending on `FCEF & 7 == 4` | HYPOTHESIS: memory wait states per chip select |
| `FCDF`, `FCE0`, `FCF3` | Cleared at init | UNKNOWN |
| `FCE5` | **Delay down-counter**: written E4h, polled until 0, 20× per unit (kernel 0x6622) | Behaviour EVIDENCE; count rate UNKNOWN (Ribbit: one count per machine cycle) |
| `FCEF` | Bits 0-2 chip revision. The kernel prints "ASIC Rev: 0x0=ASIC 1", and value 4 selects other timings and another cartridge probe | EVIDENCE |

The standard 8051 serial port (SBUF, interrupt-driven) carries the firmware's **debug log**. On a real unit
it is probably on test pads (the base also has an S-record download monitor: "S-Rec In Mode", "Monitor
Commands"). Ribbit captures it, and it is the main research tool. VERIFIED (the text is coherent).

## Software structure (VERIFIED: printed by the firmware itself)

### Base ROM 155-00450 "LeapPad ILA2 Universal Base ROM"

Built Aug 06 2001; kernel built Jun 12 2001; "Copyright (c) 1998-2001 Knowledge Kids Enterprises". Its power-on
log in Ribbit (no cartridge):

```
Starting Kernel in CS0
ROM  Size: 8 MBits  Addr: 0x00000000  Rev: V1.0  Part#: 155-00450  ID: 0x00000003
Hardware  ASIC Rev: 0x0=ASIC 1  Chip ID: 0x0=Unknown
Past CheckPowerOff()
Battery loss: Yes
Cart found:No
Hardware Init: PowerDownCheckInit BatteryCheckInit TimerInit E2Init EE_init ("Slow EEPROM used")
               NTInit ("CS bad No CAL table, Please Calibrate", "Recal EE: 2 Cal 4")
               DSPInit RecInit ("Recorder Not Detected") AudioInit Ex1IntInit
Launch base ROM lpBootApp: 0x00020000
AppMain: Prebuilt TnT: V1.1   App=0x00020000,0x0002E946
CartStatus:0:No Cart
hSection:0:PowerOn  hSpread:0:PowerOn  hCurView:0:PowerOn: Bank 0 at 0x00030058
TnT:V1.1 … [Init] [Entry] … Spread height left:0  Spread height right:0
```

* **NTInit** is the NearTouch pen driver. Its calibration table lives in the serial EEPROM ("E2"). With a blank
  EEPROM the firmware says so and falls back to defaults (what those are: UNKNOWN).
* **Battery loss: Yes** because Ribbit's on-chip RAM starts cleared. The firmware keeps state across power-off
  in RAM and checks a CRC ("XDataCRC").

### Cartridge check and launch (VERIFIED with 500-00406 Tutter's Tiny Trip)

```
Cart found:Yes - Validating...
ILA2 RIB: ProdID:14 = Base ProdID
  LangMask:00000001 = Generic
  Sparse CSum:calc>x7EDB:Cart>x7EDB:
  Cart OK
Launch cart kernel: 0x01400000
Starting Kernel in CS5 … Rev: V1.3  Built: Apr 15 2002  Name: TuttersTinyTrip
Launch cartridge lpBootApp: 0x01420000
AppMain: Prebuilt TnT, QnA3 & SnF: V1.3
CartStatus:6:Cart Ok
hSection:0:Common  hSpread:1:Spread TTT 0203  hCurView:0:PowerOn: Bank 0 at 0x01442235
Spread height left:2  Spread height right:14
```

* The base reads a **ROM Info Block ("RIB")** in the cartridge at cartridge offset 0x10100 onwards. It checks:
  * a security string and a copyright string;
  * the product ID against the base's list ("Generic" or matching);
  * a language mask;
  * a **sparse checksum** over the cartridge.

  Failure messages include "Bad Security string", "Bad sparse check sum", "Ann msg: not LeapPad cart" and
  "LeapPad Classic not supported on this base". There are "ILA1" and "ILA2" RIB versions, and older "LeapPad
  Classic" cartridges have their own path. Field offsets: UNKNOWN beyond the first reads at 0x10102 and
  0x10106/7, except the checksum's:
* **The sparse checksum (VERIFIED, 2026-10-24):** traced on the Full Base 152-10620 by logging every cartridge
  read during "Validating..." (`CARTREADS=file` in lpboot), and checked on the six ILA2 cartridges that have it
  (00406, 00652, 00730, 01022-B, 01148, 11374). The RIB holds three big-endian physical addresses: `0x1014C`
  the cartridge's start (0x1400000), `0x10150` the chip's end, `0x10154` the end of the data. The sum is the
  16-bit sum of every 997th (0x3E5) byte from the start to the end of the data (about 2100 bytes), stored
  big-endian in the two bytes after the data's end. `ribbit_leappad::checksum` computes it.
  * **500-00104 (Tad Goes Shopping) is a damaged dump too (VERIFIED, 2026-10-24).** Address line A8 was not
    read when it was made: 83% of its bytes equal their twin 256 bytes away (under 7% for every other line), so
    every odd 256-byte block is a copy of the even one before it, and half the cartridge is missing. The RIB at
    0x10100 is a copy of the code at 0x10000, so the base answers "LeapPadClassic RIB: Bad classic pCopyright
    value", "CartStatus:7:Bad Cart", and runs its own book. It cannot be repaired. Ribbit names the fault
    when the file is opened (`lost_address_line`); no other dump in the set trips the check.
  * **500-01022-B (The Messy Mystery) is a damaged dump:** it sums to 9AC7h where it says 9EBBh, so the base
    answers "Bad sparse check sum", "CartStatus:4:Bad Cart" and "Ann msg: unknown cart", and no spot speaks. Its
    address range is right (2 MB), and nothing in it is blank or doubled, so some sampled bytes differ from the
    chip's. With the stored sum changed to match, it starts ("Prebuilt TnT & QnA4: V1.4") and all 231 of its
    spots speak. Ribbit's app does that by itself (`repair_checksum`, logged and told to the player); a real pad
    would refuse the file.
* **A cartridge carries its own kernel and app.** The base checks it and then runs the cartridge's kernel (the
  same boot stub, at physical 0x1400000). That kernel runs the cartridge's app from 0x1420000. The base kernel
  is not used after that, so the pen, sound and EEPROM drivers of a newer cartridge replace the base's.
* Apps are a common engine ("TnT" Touch and Tell, "QnA" Question and Answer, "SnF") driven by tables. They work
  in **Sections** (activities), **Spreads** (an opened pair of pages), **Views** (states of a spread) and
  **Clickables** (touch regions with actions and rules: Simple, Modal, switches, Series, Counter, PlayBack…).
  Audio is requested through a script called **GAS**.

### What the cartridge holds (EVIDENCE)

* Kernel code (0x10000), app code (0x20000+) and per-view data, e.g. "Bank 0 at 0x01442235".
* Named views per page (`FCoverTnTV1`, `P03QnAV1` … in 500-00263; `FrontCover`, `Page00_01` … in a Disney
  cartridge).
* The touch layout of each spread (below).
* Speech as LFC streams and music samples as A-law (see "Audio").

### The cartridge's touch layout (VERIFIED for the "Prebuilt TnT" app; `src/layout.rs`)

Every touchable spot of the book is in the cartridge, so Ribbit draws a book's spots and GO tabs without a
book dump. Found by tracing the app from the touch to its answer (Tutter's Tiny Trip: the hit test at app bank
0x20000 CPU 0x4D6C, the clickable loop at bank 0x38000 CPU 0xBC7C, the page-pointer touch at 0xBB64).

* **App header:** a pointer at cartridge 0x1013C (0x01440000 in Tutter's Tiny Trip). Byte 0 is 2 in this app
  ("AppMain: Prebuilt TnT, QnA3 & SnF"). Its 16-bit pointers are addresses in its own 32 KB bank.
  * `+17h`: number of views; `+19h`: the view table, 9 bytes each: name pointer, the view data's physical
    address (32 bits), then the view's **spread** and two bytes.
  * `+1Fh`: the **GO list**, one entry per spread in book order: a shape index, then a pointer to 3 bytes whose
    last two are the view the GO spot opens. Bit 14 of the shape index ends the list.
* **View data:** byte `+1` is the view's kind (0: Touch and Tell; others are question and game views, laid out
  differently, not read yet). A Touch-and-Tell view's `+14h` points (in the view data's bank) to its
  **clickables**.
* **Clickable:** a type byte, a mode mask, a byte, a byte whose low 3 bits must be non-zero for it to be tested,
  and the **shape index** at `+4` (BFFFh: none; bit 14 ends the list), then type-specific data. Sizes: types 0
  and 7: 8 bytes; 1 and 8: 8 + 2 × (view byte `+25h` − 1); 2-6, 9 and 11: 10; 10: 14; 12: 9. The app names
  the types in its log: Simple, Modal, MSwitch, GSwitch, Series, BGSwitch, BGToggle, VSwitch, MVSwitch, PlayBack,
  Counter, AudCntrl, SmpCtrld (`Simple:32867` is a Simple clickable with shape 32867).
* **Shapes:** index n with bit 15 set is entry n of a word table at the pointer in cartridge 0x10134; the shape
  is at the table plus 4 × the entry. Without bit 15 the index is in the base ROM's table (pointer at 0x10130),
  not read yet. A shape is points (x, y) of two big-endian words, 13 bits each, in the app's page coordinates:
  * the first x's top bits give the kind: 1 a rectangle (two corners), 2 a polygon (bit 15 of an x marks its
    last point); other kinds never match;
  * the first y's bit 13 gives the page (0 left, 1 right).
* **Hit test:** the app tests the clickables in list order and the first whose shape holds the pen wins.
  Rectangles include their edges. Polygons: count the edges that span the pen's x (from the lower x inclusive
  to the higher exclusive) and pass at or above it, in integer arithmetic; odd is inside, and a pen on an edge
  is inside. GO spots are tested before the clickables, with a margin of 4.
* **What it shows:** whole pictures as polygons (Tutter, the boat …) and every word of the printed text as its
  own rectangle. The GO spots are tabs down the left edge of the left page (Tutter's Tiny Trip: 12, from
  y = 30 to 1342, x 11-116).
* **Checked (VERIFIED):** `the_cartridge_answers_every_spot_its_layout_lists` touches GO on every spread and
  then a point inside every clickable, and compares the clickable the app names with the one the layout puts
  under the pen where the app felt it. Tutter's Tiny Trip: 914 of 914. Spreads 1-4 of Bounce, Tigger, Bounce
  (152 of 152), Finding Nemo (308 of 308) and spreads 1-2 of Disney Princess Stories (197 of 199 answered, all
  as predicted).
* **Which dumps it reads:** 7 of the 20 have this app (500-00406, 00652, 00730, 01022-B, 01148, 11374 and
  the Mind Station magazine). Cars (500-11374) has a newer app with the same header, read as described under
  "The State Table Model app" below. The Classic cartridges have an older app (header byte 0 is 0), read as described under "The Classic app's
  touch layout" below. 500-00104 is a damaged dump (below).

### The State Table Model app (Cars, 500-11374; 2026-10-24; `layout.rs` `stm_touches`)

Cars logs "AppMain: Prebuilt STM: V1.9, Built: Oct 29 2004" and "State Table Model: v1.2", with a VM and named
states ("[Intro]", "[Home]", "[TNT_PlayingGoAudio]"). The app header, view table, GO list and shapes are the
TnT app's. Its views' data byte `+1` is 13h, and the rest of the data is different. Traced by touching spread 2
with `lpwatch` (all the reads kept with `RUNS=n`):

* **View data (EVIDENCE):** 32-bit pointers: `+9` to a run of state descriptors, `+0Dh` to the view's entry
  code (VM bytecode), and `+11h` to its first **state table**. A state table is a count byte and that many
  pointers to **states**. A state is a count byte and pointers to its **event records**.
* **Event record (EVIDENCE):** a pointer to its action code, the event type, an argument. A touch event is type
  09h, followed by `02 00` and a pointer to its **condition**. The condition holds the number of shapes at `+9`
  (16 bits), the list of their indexes at `+0Ch` (bit 15 set, as in the clickables), and at `+10h` a pointer
  just past that list. The app tests the shapes in the shape table as the TnT app does (CPU 0xC161).
* **What is not known:** the layout of the state descriptors, which lead to the states a view reaches after its
  first (spread 2's word list of 40 shapes is in one of them). Ribbit therefore works backwards:
  1. It finds every touch record by its form (the condition's `+10h` pointer is its list plus twice the count,
     and every index has bit 15 set).
  2. It walks back through the pointers to the state that lists the record and the state table that lists that
     state.
  3. It gives the record to the view whose `+9`/`+11h` points there, or else to the view whose descriptor run
     starts last before the pointer in the same 32 KB bank (HYPOTHESIS, but checked below).
* **Checked (VERIFIED, lpwords):** 1097 spots over the 13 spreads, 17-143 each. After touching each spread's GO,
  every one of them answered when touched, with 683 different sounds. So they are the book's own spots, not a
  fallback answer. Ribbit names them "Touch <index>".

### LeapPad Classic cartridges (VERIFIED by execution, 2026-10-24)

The 13 dumps with header byte 0 = 0 are **LeapPad Classic** cartridges (the RIB at 0x10100 starts `01 5E 01
64`). The ILA2 Universal base refuses them by its own configuration, not for want of anything emulated:

* VERIFIED-FW (kernel 0xA1B6): the word at +7Eh of the **base's** RIB (0x1017E = 7F55h) points, in bank 2, to a
  table whose word at +9 (7FFEh) points to a list of words ended by FFFFh; an entry 0001h accepts every
  Classic cartridge ("Cart OK", status 6). In this base the list is empty (0x17FFE = FFFF), so it prints
  "LeapPad Classic not supported on this base" (status 5) and its own app says "not LeapPad cart".
* With the list reading 0001h, the base validates the cartridge ("LeapPadClassic RIB: Cart OK") and runs the
  **cartridge's own kernel and app**, like the newer cartridges: "LeapPad 1.5 May 09 2001 … FWell, TnT & QnA3:
  V1.1" (500-11887), "LeapPad 1.5 08/26/99 … AppLaunch: V1:5" (500-00263). GO touches open the views the GO
  list predicts (500-11887: GO 3 opens view 4).
* Ribbit does this by default (`Bus::classic`, Emulation > LeapPad (1999) > "Play LeapPad Classic cartridges"); off is
  this base's own refusal (tests/books.rs, both ways). The 2 MB Canada Full Base's list holds 0001h, so it
  accepts Classic cartridges by itself (VERIFIED by execution: "LeapPadClassic RIB: Cart OK" with 500-11878-B).
  Which base the 1999 pad had: UNKNOWN.
* State per cartridge (their own kernels): 1.4-1.9 (00263, 00803, 11874, 11882, 11885, 11887) reach the pen
  wait, and 11878-B (1.7) too since a missing base sound no longer stalls it ("Base sounds" below); 00367 (2.0, Aug 2001) and 00394 ("0.0", Jul 2001) never get past their first
  foreground sound (00367: FG 33696, still playing after 430 s of emulated time). It is music: the app keeps
  sending start (80h + n), 10h + n and stop (40h + n) to voices 0-2 with changing pitches, as 11885 does briefly
  (its app's main loop at cartridge 0x11C37 is the same code). Why the song never ends: UNKNOWN (the end of a
  sequence, command 10h + n or 87h, or the timer rate). The pen still works during it: a touch cuts the song ("FGsc") and the
  spot answers (VERIFIED 2026-10-24 with lpwords on the Full Base: 00367 517 of 518 spots, 00394 511 of 516),
  so both books can be played. 00104 (Tad Goes Shopping) has no Classic RIB either (status 11: the base's own
  book runs). Every Classic kernel writes 55h to physical 0x400000 once (chip select 1: UNKNOWN, maybe a probe).

### Base sounds (VERIFIED-FW, 500-11878-B kernel 0x56FB-0x586A)

A sound number without bit 15 is a **base ROM sound**. The kernel finds the base's sound table at XDATA 0118h,
seen through the cartridge's RIB (cartridge 0x10118). If that is 0, it reads the same word from the base's own
RIB (bank 2, base 0x10118). Bit 15 sounds use 011Ch instead, the cartridge's own table. A table holds 8 bytes
per sound, start and end, counted from the table itself. The ILA2 base's table is at 0x58000: 198 sounds, the
first starting just after the table (0x630).

* Carts whose RIB +18h is 0 (500-00367, 00394, 00803, 11882, 11885, 11887) use the table of whatever base
  they are in.
* 500-00263/00973-B, 11874, 11878-B and both Quantum LeapPad dumps give **0x353000**. That is the 2 MB
  **Full Base**'s table (0x153000 in that ROM, the address its own RIB gives) seen through the 4 MB chip
  select, where a 2 MB ROM appears twice (VERIFIED: with the Canada Full Base, 500-11878-B's voice 6 gets sound
  C1h at 0x353630-0x359ADC, entry C1h of that table, and it plays). These cartridges were made for a Full
  Base.
* **The two tables match sound for sound:** both hold 198 sounds of the same lengths in the same order, and
  194 are byte for byte the same (126 and 168-170 differ). VERIFIED by comparing the dumps.
* On the 1 MB ILA2 base, 0x353000 mirrors unrelated data. The first such sound in 500-11878-B (after the
  first GO touch) got a start of 0x5BFD1623, never ended, and the book stopped answering. Ribbit does not start
  a voice whose start lies outside the 32 MB space, or whose absolute end (voices 5-6) is outside it or before
  its start (`sound.rs`), so on that base those sounds are silent and end at once. The chip's own behaviour
  with such addresses is UNKNOWN. Use the Full Base for these cartridges.

### The Classic app's touch layout (VERIFIED by touching, 2026-10-24; `src/layout.rs`)

The Classic app (header byte 0 = 0) keeps its book the way the newer app does, under a smaller header. Found by
watching which cartridge bytes the app reads during a touch (`examples/lpwatch.rs`) and then touching every
spot, as for the newer app.

* **App header** (pointer at 0x1013C; 500-11887 and 500-00263 at 0x28000), 16-bit fields relative to its
  32 KB bank: `+5` the **view list**, 32-bit pointers to the views' data up to the GO list; `+7` the **GO
  list**, the same format as the newer app's (shape index, pointer to 3 bytes `01 00 NN`: GO opens view `NN`).
  `+3` points to a table not read yet. Views have no names; Ribbit calls them "Page n".
* **View data:** byte `+1` the kind (0 for the pages; 3 and 5 seen for games and the cover), byte `+2` the
  page, the word at `+6` F140-F148 (F143 on pages with text; meaning UNKNOWN), and `+14h` the **clickables**,
  in the same format as the newer app (type, mode mask, two bytes, shape at `+4`; Simple 8 bytes, Series and
  GSwitch 10). The list ends at a shape index with bit 14 (`FFFF`).
* **Modes (VERIFIED):** byte `+1` of a clickable is the set of modes it answers in. The app logs the mode with
  every touch (`G:01`). Pages open in mode 1 (the 1999 app: mode 2). In mode 1 each word is one rectangle. Mode 2 sounds words out, and its
  rectangles are the parts of words; the meaning of mode 4 is UNKNOWN. A word is listed once per mode,
  pictures once with mask FFh. Three **GSwitch** clickables in boxes along the bottom of the left page (500-11887:
  x 372-888, y 1236-1336) switch to modes 1, 2 and 4 (`GSwitch:32770 NewG:2`); whether a new
  page resets the mode: UNKNOWN. The hit test is the newer app's, among the clickables of the current mode.
* **Shape table offsets differ by kernel (VERIFIED-FW):** the table entry is the shape's distance from the
  table in 4-byte units in most kernels (500-11887: shifted left by 2 at kernel 0x26C1), but in **bytes** in
  the two oldest (LeapPad 1.5 of 08/26/99, 500-00263/00973-B, kernel 0x1F39; and that of 07/14/00,
  500-11874). Ribbit takes the scale under which every GO spot is a shape.
* **The 1999 app's log** says `State: Idle` instead of `[Idle]`, reports the pen as `DT:v_  X15 Z 5(0: 265,
  73)` with no `[Touch]`, and comes out seconds after the touch; a touch's line ends only when the app next
  prints something. It opens pages in mode 2 (`G:02`).
* **Checked (VERIFIED):** `the_cartridge_answers_every_spot_its_layout_lists` (switching modes with the
  page's GSwitch spots where needed), every spread, all modes: 500-00263 2186 of 2186, 00803 990 (one spot did
  not answer), 11874 3641, 11878-B 2073, 11882 2679, 11885 3436, 11887 2927; none answered other than
  predicted. 500-00973-B is the same dump as 00263. 00367 and 00394 are not in the test (their start-up song never
  ends, above), but lpwords found them answering.
* The Quantum LeapPad dumps (500-00975-B, 00976-B) also read this way (34 spreads each); they are another
  system and were not run.

## Pen sensor (SOURCED for the principle, S3/S4; registers EVIDENCE)

* **Principle (S3):**
  * A resistive sheet under the book is driven with about 60 kHz from contacts on its edges, in several
    states: all off (offset), all driven (full), and pairs driven in opposite phase (two gradients).
  * The pen is a passive antenna. Its signal is synchronously demodulated and integrated, then measured.
  * Each reading is normalised against *full*. The two gradients give X and Y.
  * A touch counts when *full* is above a threshold.
  * One sequence takes about 12-16 ms.
* **Refinement (S4, 6,661,405):** two antennas with resistive dividers give orthogonal gradients in a five-state
  drive sequence. The integrator is measured by constant-slope discharge, i.e. a timed count: a counter value
  rather than an ADC.
* **In the firmware (EVIDENCE, base kernel INT0 handler 0x0410-0x0D9C, NTInit 0x4229):**
  * **NTInit:**
    * Reads a configuration byte from EEPROM address 4 (into IRAM 59h).
    * Its high nibble picks a touch threshold from the table at 0x5FCF: 7000, 6000, 5500, 5000, 4750 … 1500, 1000
      (big-endian words).
    * From that it sets ±1/8 hysteresis bounds (XDATA F0DB-F0DE) and a 1/16 stability margin (IRAM 32h:33h).
    * Its low nibble picks a step from the table at 0x5FEF (14 … 1, into F0ED).
    * Makes INT0 edge-triggered and enables it.
  * **Queue and FIFO.** The sensor is a sequencer with a queue and a FIFO:
    * `FC17` (write) queues a drive code. The handler keeps at most 2 outstanding while `FC2A` bit 2 says there
      is room.
    * `FC2A` bit 1 = a result is ready. `FC18`/`FC19` are its high/low bytes.
    * Each result raises **INT0**.
  * **Drive cycle** (table 0x6457): `01 11 69 09 19 01`, ending in FF. The other table at 0x645E (`01 00 0A 12`) is
    used in mode 3 (UNKNOWN when). The six results go to XDATA F0DF + 2i.
  * **Maths:**
    * Subtract the offset (IRAM 1Dh:1Eh, 0x2000) from each result.
    * A **touch** = both full readings (codes 01, first and last) above the threshold, and within 1/16 of it of
      each other.
    * Axis A = (r(69) × 0x4000) / r(11).
    * Axis B = (r(19) × 0x4000) / r(09).
    * These are **signed ratios**, accepted within ±15000 (of 16384).
    * Two cycles are averaged. A speed class is kept in 19h (tables 0x6465/0x6485).
    * While no pen is present, the threshold tracks the background level.
  * **Output:** when stable, the handler copies the ratios to IRAM **28h:29h (axis B)** and **2Ah:2Bh
    (axis A)** and sets bit **22h.6**.
  * **Sequencer reset (EVIDENCE):** every new cycle starts by writing `FC16` bit 4 low then high (cart kernel
    0x59DE). Ribbit takes that as clearing the queue and FIFO. Without that, codes left from start-up kept the
    queue full and the kernel only ever measured code 01.
  * The real count scale and conversion time: UNKNOWN. Ribbit's sensor model (`src/pen.rs`):
    * returns 0x2000 + 0x3000 for the full and reference codes, and 0x2000 + 0x3000 × ratio for the gradients;
    * returns 0x2000 for every code with the pen away;
    * takes 2 ms per conversion.

### Pen position (VERIFIED by execution with 500-00406; mechanism EVIDENCE)

* **The app asks for the pen** through a routine at app CPU 0xF77F (Tutter's Tiny Trip, bank 0x20000). With INT0
  masked, it copies IRAM 28h-2Bh, then transforms them (0xDD68):
  * **Axis B runs across the spread, axis A down the page.**
  * **Choosing the page:** axis B above the midpoint of two calibration values (the left page's 10th, the right
    page's 2nd) means the right page.
  * **Mapping each axis:** the 11 calibration values for that page and axis (XDATA: axis B F5C0/F5D6, axis A
    F0AD/F0C3) are interpolated piecewise linearly (routine 0xE223) against the cartridge's table of the
    calibration points' coordinates (app CPU 0xBDEA for X, 0xBE2E for Y; 17 words per page, the first 11 used).
  * **Scaling:** the result is divided by 8.
  * **Points (in page units):** X −180 … 7380 on the left page and 120 … 7680 on the right; Y −180 … 10980.
    Each page is about 7560 × 11160 units, i.e. portrait with an aspect of about 0.68.
* **App coordinates:** (x, y) ≈ page units / 8, with **y + 8192 on the right page**. The app's debug log prints
  them: `{DT}v_ X12 Z 5(0: 867,1240)[Touch]`.
* **Checked with Ribbit's ideal sensor and factory calibration**, aiming at (500, 625) and (875, 1250) on both
  pages: the app reports (495, 623), (867, 1240), (495, 629 + 8192) and (872, 1249 + 8192). Near the left and top
  edges the error was large until the EEPROM layout was corrected (see "Serial EEPROM").
* **A GO touch:** a touch on a GO tab (the left page's left edge, see "The cartridge's touch layout") produces
  "GlobalPP: S:33764 V:2", "Entry event: PPTouch" and "New view init" in the app: a page-pointer touch, which
  selects the spread.

### Serial EEPROM (VERIFIED by execution)

* **Bus:** I²C, 24C01/24C02-style: control bytes A0/A1, a word address, repeated-start random reads, sequential
  reads. The bits are banged by hand:
  * `FC1F` is the port data register: bit 2 is SCL; bit 3 reads SDA.
  * `FC33` is its direction register: bit 3 set pulls SDA low; bit 2 drives SCL.
  * The routines are at cart kernel 0x46CB (byte out) and 0x4E6B (byte in).
* **Ribbit's model** (`src/eeprom.rs`) answers it, and the kernel reads all 128 bytes through it at power-on.
* **Layout (EVIDENCE, from the kernel's checks and black-box tests with patterned images):**
  * `00`: calibration version, which must be 4.
  * `04`: pen sensor setup (threshold and step nibbles, see NTInit).
  * Four regions of 14 big-endian words: axis B left `0F-2A`, axis A left `2B-46`, axis B right `47-62`, axis A
    right `63-7D` (the last region is cut off by the checksum).
  * A region's **first word is the reading at a point just off the page** (the last entry of the cartridge's
    point table: x −780 on the left page, −1080 on the right, y −1080). Words 1-13 are the readings at
    calibration points 0-12.
  * Bytes `01-02` also move the edge entries of all four tables (UNKNOWN what they are; Ribbit leaves them 0).
  * `7E-7F`: the ones' complement of the 16-bit sum of bytes `00-7D`. A blank EEPROM fails it: "CS bad No CAL
    table, Please Calibrate".
* **How the tables are built:** at power-on the app (Tutter's Tiny Trip bank 0x20000, CPU 0xBB80-0xBDE3) sets
  table entry i from region word i + 1. Entries 3-5 are copied; the others are smoothed with their neighbours
  (kernels at CPU 0xB379, weights summing to 16384), so the off-page word bends entries 0-2.
* **Corrected (EVIDENCE):** Ribbit first put the regions one word later (`11`, `2D`, `49`, `65`). The rebuilt tables
  then folded back at the left and top edges (entry 0 above entry 1), and a touch on a GO tab at x = 63 read as
  x = 0. With the layout above, all four tables come out straight to within 1% (`lpboot FACTORY=1
  XDUMP=F5C0,16;F5D6,16;F0AD,16;F0C3,16`), and every GO tab answers. Ribbit replaces a saved EEPROM that holds
  its earlier factory image.
* **Calibrating:** the ROMs have no calibration procedure (presumably a factory fixture did it). Ribbit
  therefore writes a **factory calibration for an ideal sensor** (`calibration::factory_eeprom`): readings
  linear in position, axis B −15000…−1000 across the left page and +1000…+15000 across the right, axis A
  −14000…+14000 down. This is a HYPOTHESIS about the response. Any monotonic response gives the same app
  coordinates once calibrated.
* **The rest of the EEPROM** (bytes 03 and 05-0E) is UNKNOWN; changing them did not move the calibration.

## Audio (works: music, effects and speech play; the details are partly HYPOTHESIS)

**The sound processor is the Leapster's voice engine, or its ancestor** (EVIDENCE, below). Ribbit therefore plays
LeapPad sound with its Leapster voice model (`ribbit-core` `io/voices.rs`, from MAME and checked against a
Leapster recording), fed from the LeapPad's registers (`crates/ribbit-leappad/src/sound.rs`). What the code shows
(cart kernel 500-00406):

* **The match with the Leapster (EVIDENCE):**
  * `FC1E` reads the playing voices with **voice 0 in bit 7 and voices 1-7 in bits 0-6** (kernel 0x5A4D for
    voices 0-5, 0x5CD3 for 6-7). The Leapster's status register has the same odd layout ("not sure why it's out
    of place", MAME).
  * A command byte is **command << 3 | voice**, and start is `80h | n`, as on the Leapster.
  * The kernel stores a voice's start, end and loop exactly as the Leapster's registers hold them (table below).
  * The sounds it starts are **A-law at physical byte addresses**: the base ROM's music samples at 0x88A58,
    0xA0000 and so on, with D5/55 silence.
  * The cartridge's speech is **LFC** streams. They decode with the codebook that the base ROM holds at 0x900,
    which is byte for byte the Leapster's (both the 1 MB ILA2 base and the 2 MB Full Base have it there).
  * **Base speech** (VERIFIED-FW, 500-11878-B kernel 0x60CB-0x61DD): a foreground sound with bit 15 set that is
    not one of the cartridge's own is looked up in the table whose address is at RIB +10h: 32-bit big-endian
    entries, 4 bytes per sound, each a **signed offset from the table** (sounds lie both before and after it).
    The kernel points XDATA window 2 (FC0A:FC0B) at the table's bank to read the entry. The Full Base's own
    RIB gives 0x80000; the Classic cartridges made for it (00263, 00973-B, 11874, 11878-B) give **0x280000**,
    the same table through the 4 MB chip select mirror, as for the A-law table at +18h. VERIFIED: every entry
    tried lands on an LFC stream (mode word 0). At the Shore's answer to a touch off the GO on its cover
    ("FG:33004" in its log) starts entry 248h, the clip at Full Base 0x38000; how 33004 becomes 248h is not
    traced yet.
* **Voices (EVIDENCE for the roles):**
  * 0-4: pitched A-law with a loop: the music's instruments and the sound effects.
  * 5: plain A-law from start to an absolute end: the drums (hi-hat-like samples, mostly 3-4 kHz).
  * 7: LFC speech. Its starts come from 0x49F4, with addresses in the cartridge (0x15B9748 …), modes 0 and 1.
  * 6: not seen started yet.
* **Shared RAM (EVIDENCE for the layout; that the processor reads it when a command arrives is a HYPOTHESIS):**

  | XDATA | What |
  |---|---|
  | `F400-F427` | 20 words written by DSPInit (0x49FD) while the processor is held (`FC3D` = 09/08, then 05/04): UNKNOWN (coefficients or a small program: 25D2, B5E4, 6664, CEC0 …) |
  | `F462 + 4n` | start, physical byte address: two little-endian words, high word first |
  | `F482 + 4n` | voices 0-4: end index and loop index (low halves; the high half is the start's). Voice 5: absolute end. Same word order |
  | `F49E + 2n` | pitch, voices 0-4 (3F88 for effects; notes vary) |
  | `F4B8 + 2n` | voices 0-4: a value from the sound's descriptor (00C8, 0400, 0500): UNKNOWN (an envelope rate?) |
  | `F4C2 + 2n` | volume, 7FFF at start. Voice 5's (`F4CC`) is set per sound from the descriptor instead (0400, 0500) |
  | `F4D2 + 2n` | copied from `F4F4 + 2n` before an `08h \| n` command; set to the pitch at start when the voice's flag bit 4 is set: UNKNOWN |

* **Sound descriptor (EVIDENCE, kernel 0x13A0-0x1625):** 20 bytes, big-endian. The note picks one from a list
  whose byte `+11` is each entry's highest note (a multisample's key ranges):
  * `+00`: start (32 bits; FFFF in the high half means something else, kernel 0x0011);
  * `+04`: end (32 bits, voice 5 only);
  * `+08`: loop index; `+0A`: end index (voices 0-4);
  * `+0C`: a 32-bit value, multiplied by a per-note entry of the table at kernel 0x2153 to give the note's length
    in EX1 ticks (`F038 + 0Eh × n`);
  * `+11`: highest note; `+12`: the `F4B8` value (voice 5: its volume).
* **Commands (EVIDENCE for when the kernel sends them, HYPOTHESIS for what they do):**
  * `80h | n`, start: Ribbit latches the shared RAM above and starts the voice.
  * `40h | n`, stop: sent when a note's time is up and another note follows, or on a stop request.
  * `10h | n` (`25h` for voice 5): sent when a note's time is up and a rest or command follows. Ribbit takes it
    as a **release**: the voice leaves its loop and plays to its end. The `20h | n` sent when flag 04 is set is
    treated the same.
  * `08h | n`, sent after copying `F4F4 + 2n` to `F4D2 + 2n`, and `0Eh`/`0Fh` for voices 6/7: UNKNOWN, ignored.
* **The acknowledgement is an interrupt (VERIFIED by running it).** The processor's "command taken" raises
  external interrupt 1 as well as `FC00` bit 4. Ribbit raises it `DSP_LATENCY` (100) machine cycles after a
  command; that delay is ASSUMED. Without it the GAS player never got past its first command.
* **The kernel's player** (EVIDENCE) is EX1 work at 0x1849 for voices 0-5 and 0x1AAC for 6-7, run at T1's
  2.5 kHz.
  * Each voice has a 14-byte record at `F034 + 0Eh × n`: `+00` score pointer (FFFF for a single sound), `+02`
    ticks left, `+04` note length, `+0D` flags.
  * When a single sound's voice stops being busy, the sound is done ("FGDone").
* **What Ribbit plays (VERIFIED by running it; not compared with hardware):**
  * Tutter's Tiny Trip plays its start-up music, its announcement and its idle music (`BGStartRu`).
  * The speech streams decode to phrases of 1.7-4.9 s, with only a few samples at full scale (`lpsay`).
  * Before `FC1E` was modelled, every voice read as finished at once. Sounds then "ended" before they could
    play, and two pen tests failed.
* **Not known (HYPOTHESIS in Ribbit):**
  * The pitch scale and sample rate. Ribbit uses the Leapster's: pitch / 8000h samples per output sample at
    8 kHz, so 3F88 plays a sample at about 4 kHz. Some instrument samples have nothing above 2 kHz, which fits.
  * The mix level, and how volume scales.
  * What `F4B8`, `08h`, `10h`/`20h` and `F4D2` really do.
  * Where the decoder finds the codebook. Ribbit reads it from the base ROM's 0x900 (physical 0, through the
    wrap of the 32 MB space); no register for it has been found. Until 2026-10-24 Ribbit read physical
    0x100000, a mirror of 0x900 only in the 1 MB base: with the 2 MB Full Base every speech clip decoded to
    silence (the user's "clicking makes no sound"), while the A-law music still played.
* **To check on hardware:** a recording of a LeapPad playing Tutter's Tiny Trip's start-up music would settle the
  pitch scale (compare a held note's frequency).
* The LeapPad Plus Microphone has a separate Sunplus PA7790 sound chip (S1). The original's sound path is inside
  the FS80A363.

### Music: the voice registers (VERIFIED-FW, 2026-10-24)

The cartridge kernel's music code is the Leapster BIOS's sound driver in 8051 form. The kernel's volume routine
(cart kernel 0x1230-0x1284) matches the Leapster's (US v2.1 0x400301D8) step for step. Both:
1. add two bytes of the note (velocity and channel volume);
2. below 0x8F, take entry 4, otherwise entry (n − 0x7F) / 2;
3. look the result up in the same 128-entry table (base and cartridge 0x10300 = BIOS 0x4010_0F5C).

The Leapster writes that value to its volume registers (0x0180_413C and 0x0180_41A4, via 0x4002F6F4), so the
table is a velocity-to-volume curve, not a note table. Watching the kernel's writes (`lpboot XWATCH`) then
places the rest; the map is in `src/sound.rs`:

* `F49E + 2n` = note volume and `F4D2 + 2n` = current volume (the fades), as on the Leapster.
* `F4AE + 2n` = **pitch**, 0x8000 = one sample a step as on the Leapster. Cars' notes there are exact
  intervals: 0x4000 an octave down, 0xAADC a fourth, 0xBFC8 a fifth, 0xE411 a minor seventh.

Ribbit had read `F49E` (the volume) as the pitch and `F4C2` (a constant 0x7FFF) as the volume, so every note
played at a pitch made from its loudness. That is what players heard as farting, buzzing and "random noises".
The guesses made while that was so (a 4x pitch scale, `F4B8` as a rate or a release, muting music under speech)
are withdrawn.

* The processor follows `F4AE` and `F4D2` while a voice plays (the kernel rewrites them without a command).
* 0x10 | n stops a voice, as on the Leapster.
* A video of a real pad playing Cars (YouTube `MrrGFKw4-44`, analysed only) is the reference. Ribbit's start-up
  now runs in its order: a stepped jingle, "Cars!", then music of separate notes. The jingle's notes do not
  all match yet (both have 288 Hz at the same moment; the real first note is 600 Hz where Ribbit's is ~200 Hz):
  OPEN.
* `F4B8 + 2n` is the **release rate** (VERIFIED against a recording, 2026-10-24).
  * The kernel copies it from the instrument record's last field (record +0x12, big-endian; cart kernel
    0x1688-0x16DE). Voice 5's goes to `F4CC`, matching the Leapster's 0x0180_4170/4184.
  * After a key off (0x10 | n) the voice keeps playing while its volume falls by rate / 32768 every
    25 samples.
  * The power-on tune's last chord (rate 0x00C8) then fades as the recording does: log-RMS error 0.18 per
    50 ms.
* The power-on tune: G3, B4/D4 … then a G chord. Its notes, timing and loudness match a recording of a real
  pad (the user's own recording, not in the repository).
* The Leapster's two tables: 0x4010_0E5C is 128 u16 values (16 steps an octave), and the velocity curve
  follows it. SetPitch's bend factors are f32 at 0x4010_105C.
* Tools: `lprec` (`MUTE`, `DSPLOG` with times and `F4B8`, several touches `r,r:hold`), `lpboot XWATCH`.

## Time

* **`FC01-FC04`:** a 32-bit counter (FC01 most significant). Both the kernel (0x5B9A) and the app (0x8EE7) read it
  and shift it right by 3 for their clock. Writes load the byte written; the kernel waits between byte writes,
  suggesting a slower clock domain.
* **The app's idle power-off** (app 0xA2E8-0xA35F):
  * Any activity sets the deadline to the low byte of ((count + timeout) >> 3).
  * When the low byte of (count >> 3) reaches it, the app powers down ("Auto Power Down: powerdown GAS:… from
    Cart").
  * Since only 8 bits are compared, the longest timeout is 256 units.
* **Rate: 1 Hz, a seconds counter (EVIDENCE).**
  * The kernel's idle power-off (0x5A82) adds the cartridge's timeout, read from its resources (0x548D), to the
    count. For Tutter's Tiny Trip that timeout is **180 (00B4h)**: a round 3 minutes in seconds.
  * The firmware's unit (count >> 3) is then 8 s, and the longest timeout 256 units (34 minutes).
  * Tried first: at 8 kHz the app powered itself off 0.8 s after starting, and at 8 Hz after 22 s.
  * Unverified against a real unit: a video of one left alone for 3 minutes would confirm it.
* **Before this counter was modelled,** the app's main loop spun forever in the `>> 3` helper (app 0x8D7C),
  waiting for time to pass.

## Dumps (A1)

**Base ROM:** `leappadbios.bin`, 1 MB, CRC32 c886cddc, SHA1 f8a83b156feb28315d2321758678e141600a0d4e. It is
MAME's `leappad` "ila2_universal" BIOS (found in the user's "LeapFrog ROM Dumps Collection.zip" as
`leappadbios (1).bin`). The same zip holds My First LeapPad and Little Touch LeapPad BIOSes (4 MB each, also in
MAME), which are not the subject here.

**Full Base ROM:** `leappadbioscanada.bin`, 2 MB, SHA1 adf52232adcfd4de5d8e31c0e0c09be61718a9d4, beside the
ILA2 one in a MAME-style `leappad.zip`. It prints "2MB Canada Full Base ROM: US Library: Cart start app:", part
152-10620, ROM ID 12h, built Jan 23 2004 (kernel Feb 13 2003); its header says "ToolPad V2.3.2 LeapPad FullBase
V1.1.4". It boots in Ribbit, accepts Classic cartridges by itself, and holds the base sounds the older Classic
cartridges ask for ("Base sounds"). Its cartridge probe also needs `FC2F` bit 0 (registers). **This is the base to
use**; the ILA2 base remains supported.

**Cartridges:** 20 in `LeapPad_Reading_System_Archive/` (archive.org item of that name), 1-4 MB. Four match MAME:
500-00406, 500-00730, 500-01148 and 500-11374. The rest are not in MAME's list. 500-00263 and 500-00973-B are the
same dump. Two are Quantum LeapPad cartridges and one is Mind Station; those are other systems, not tried yet.

None of these files may be committed (`.gitignore` covers `*.bin`, `*.zip`).

## Reproducing

```
cargo build -p ribbit-leappad --examples
target/debug/examples/lpboot <base rom> [cartridge|-] [millions of instructions]
IOTRACE=1 target/debug/examples/lpboot …   # also lists every FCxx access by PC
target/debug/examples/lpdis <file> <file offset> <cpu address> <length>   # hex numbers
```

`lpboot` prints the firmware's serial log, every access the model does not understand (first time each), the
hottest PCs and the last instructions run. Its research switches (environment variables):

| Variable | Does |
|---|---|
| `FACTORY=1` / `EEPROM=file` | the factory-calibrated EEPROM / one from a file (default for `lpboot`: blank) |
| `PAGEPEN=page,x,y,start,length` | pen down at a point of a page (0 left, 1 right; the app's page coordinates) from `start` for `length` (millions of instructions) |
| `PEN=u,v,start,length` | the same as raw axis ratios |
| `HZ=n` | machine cycles per second |
| `IOTRACE=1` / `IOSEQ=1` | every FCxx access by PC / the raw sequence after the pen goes down |
| `EELOG=1`, `DSPLOG=1`, `PENLOG=1`, `PENDUMP=1` | EEPROM bus, sound commands, pen codes, pen driver state |
| `WATCH=lo,hi` | instructions that read internal RAM lo..hi |
| `XWATCH=lo,hi;…` (+`XWATCHALL=1`) / `XDUMP=addr,len;…` | writes to on-chip XDATA / XDATA at the end |
| `BREAK=phys` / `BREAKTEXT=text` | stop at a physical code address / when the log prints text; shows the last 300 instructions, registers and stack |
| `TIMES=1` | emulated time in the log every million instructions |
| `TRAIL=n` / `TRAILFROM=text` | how many instructions BREAK shows (300) / start the trail again whenever the log prints text |
| `WAV=file` | the sound processor's 8 kHz output, as a WAV file |

`lplayout <cartridge> [all]` prints the cartridge's touch layout: each spread's GO spot, the view it opens and
its clickables (`all`: with their outlines). `lplayout <cartridge> book <file.toml>` writes it as a book file.

`lpsay <bios> <cart|-> <voice> <start> [end/loop] [pitch] <out.wav>` plays one sound through the sound
model alone (hex; e.g. voice 7 and a speech start from `DSPLOG=1`) and reports its length and clipping.

The tests: `cargo test -p ribbit-leappad --release -- --include-ignored` with `RIBBIT_LEAPPAD_BIOS` and
`RIBBIT_LEAPPAD_CART` set (the ignored ones need the dumps). Disassemble the kernel with `lpdis rom 10000 0 8000` and
`lpdis rom 18000 8000 8000`, and an app bank with `lpdis rom 20000 8000 8000`.

## Open questions, in order

1. Audio details: the pitch scale (needs a hardware recording), the commands other than start and stop,
   `F4B8`, `F4D2`, and the 20 words DSPInit writes.
2. The touch layout of question and game views, Classic mode 4, the shapes in the base ROM's table, and the
   layout of the State Table Model's state descriptors (Cars' spots are found around them, below).
3. What `FC3F` and `FC2E:FC2F` are, since they are not the pen after all (touched by the serial receive
   handler).
4. Whether the calibration point tables are the same in every cartridge, and in the base ROM's app.
5. `FC01-FC04` and the other timers; the clock rate (sound pitch and timing depend on it).
6. Power: the ON/OFF switch, battery level, auto power-off.
7. The 1999 board: chip markings, finger-well beam, cartridge slot side.

## The words on the pages (2026-10-24)

Cartridges hold no text: a word spot only says its word. Ribbit gets the printed words by listening:

1. `examples/lpwords.rs <bios> <cart> <dir> [eeprom]` boots the cartridge, touches each spread's GO and then
   the middle of every spot of the book drawn from the cartridge, and writes what answers (up to the firmware's
   `FGDoneEvent`) as `<spread>-<region>.wav`, plus `index.tsv`. At the Shore: 593 clips in about a minute.
2. `scripts/leappad_words.py <dir>` transcribes them with Whisper (`medium.en`, on the GPU). One-word clips heard
   alone come out as homophones ("Sure" for *shore*, "2" for *to*), so each is heard again with the spread's
   sentence clips as the prompt and then aligned, in order, to the sentences' words. Page numbers become digits.
   Short clips that match no sentence word (sound effects, "Wow!") are kept as `phrase`, not drawn as text.
   Whisper's loops on long sounds ("AHHHH…" for a scream, "Thank you." many times over) are cut to one.
   * **Sentence buttons.** A roughly square box at the start of a line that says a whole sentence (the printed
     speech bubble) is written as `sentence`. The words of its sentence are laid over the word boxes in reading
     order (`lplayout <cart> regions` gives the boxes): a Needleman-Wunsch alignment of what each box says,
     plus filling between matches where the counts agree. Word boxes outside any sentence use the spread's
     sentences in spot order.
   * **Page numbers.** A spot in a bottom corner that says only a number keeps the number.
   * **Stop signs.** The spot just above the page number whose shape is on most spreads (Cars' `Touch 32774`,
     Tigger's `VSwitch 33585`) is written as `stop`, and drawn as a red STOP octagon.
   * **Never printed.** A misheard swear word is dropped, or, said alone, becomes the letter it was ("Hell" → L).
   * **Recording.** lpwords stops recording a touch at the firmware's `FGDoneEvent`, or else after 0.6 s of
     quiet (Cars' app logs no `FGDoneEvent`), within 25 s, so long sentences are not cut.
3. The result goes to `<data folder>/leappad-words/<part number>.tsv`; the app fills it into the book drawn from
   the cartridge (`Book::apply_words`): `word` lines are drawn in their spots, `phrase` lines shown under the pen.

Words files are transcriptions of the books' text, so they stay on the user's PC (not in the repository).
Accuracy: HYPOTHESIS per word (speech recognition, not the printed page); At the Shore read correctly on the
spreads checked by eye, with a few slips ("Ew will" for *You will*).
