# Leapster software: BIOS services, cartridges, PEG and the MQX kernel

What Leapster software does, as seen from inside: enough to debug games in Ribbit and, eventually, to write new
ones. Labels as in hardware-research.md: **VERIFIED-FW** = read from firmware or cartridge code (addresses are
US BIOS v2.1 `152-11265` unless stated) and/or observed in Ribbit; HYPOTHESIS = inferred, not confirmed.
Hardware facts (registers, timing, caches) live in hardware-research.md.

Research tools (`crates/ribbit-core/examples/`): `scenes` (scripted runs; `logtrace` prints a game's debug log,
`bptrace` registers at a PC, `watch` writes to a word, `lcdrate` frames per second), `pcs` (hot PCs, MMIO and
call logs), `disasm`, `mix` (instruction mix and cache simulation).

## Engines

* **Flash** ("Fly player" in the BIOS: `kFlyPlayerRsrc*`): most games are Flash MX movies with ActionScript,
  played by the BIOS's player and rasterizer (hardware-research.md, "The BIOS's Flash rasterizer").
* **PEG**: LeapFrog's own native game framework, built into the BIOS (`kPegPlayerRsrc*`: ProcessEventsFcn,
  OnIdleFcn, TerminateFcn, ScreenInitFcn, MsgQueueInitFcn, PresentationInitFcn, PegAppCode, PegAppModel,
  PegAppTable, DefFontInitFcn; `kAppsRsrcPegMainApp`; `kAssetsRsrcPegFontTable`/`PegBitmapTable`). The BIOS
  also prints a PEG app's identity: "Peg App ID / Code ID / Model ID / Dataset ID / smIcon ID / lgIcon ID /
  displayName / queryName" (strings at 0x4010_03A8-0x4010_0418).
* **TSE** (from the user): Torus Games' engine, used through PEG by Sonic X and Counting on Zero. It renders a
  GBA-like tile/sprite picture in software and uses Torus's GBA asset containers (known from the Didj port's
  debug symbols). These games are CPU-bound (see hardware-research.md, "CPU timing").
* Native C++ games can mix both. Cars (Disney-Pixar) is a PEG cartridge whose game is a C++ state machine
  (`GameStateManager`, `FlashState`, `TrackIntro*State`, ...) that plays Flash movies for presentation: the BIOS
  log shows `PlayMovie:"Packed RIB Movie(s)"` and `Request URL: loading movie hndl=0x8000000N into layer 10/99/97`.

## Cartridge interface list (VERIFIED-FW: Cars, Sonic X)

The cartridge header word at **+0x30** points to a list of exported interfaces, 0x1C bytes each:

| Offset | Cars | Meaning |
|---|---|---|
| +0x00 | 0x3001_0100 | Interface ID: **PEG Core** |
| +0x04 | 0x0100_0100 | version (HYPOTHESIS) |
| +0x08 | 0x101 | |
| +0x0C | 0x800D_C814 | name or descriptor (HYPOTHESIS) |
| +0x18 | 0x800D_CF74 | method table |

Sonic X has the same structure at 0x8010_E020 (table 0x8010_DFE4). Cars' PEG Core method table:

| Index | Cars | Notes |
|---|---|---|
| 0 | 0x8008_88D8 | Init: called by the BIOS's component start-up loop (0x4003_9B8C-0x4003_9C62, which calls method 2 then method 0 of every registered interface) at 47 ms |
| 1 | 0x8008_9174 | Queries interface 0x3001_0005 through `[[gp+4]+0x28]+0x14`; if absent, sets event bit 0x40 on the app's event group |
| 2 | 0x8008_92BC | Called before Init by the start-up loop |
| 9 | 0x8008_930C | Stores a message, then sets event bit 0x80 |

The string "GetPegCoreInterface() can only be called on the PegPlayer task" (0x800D_CF34) shows the PEG player
runs as its own task. The app task waits for bits 0x40|0x80 (Cars 0x8008_A476) on the event group at
`[0x3C00_02D0] + 0x2C`.

## The MQX kernel (VERIFIED-FW)

Cartridges carry their own copy of Freescale/ARC **MQX** (the hardware-research.md timer notes already found the
MQX BSP in the BIOS): 100 ticks/s on timer 0 (vector 0x19).

* **Kernel data**: `[0x3C00_023C]` = kernel data (Cars 0x3C1C_2D60): +0x1C current task, +0x34/+0x38 the 64-bit
  tick count. `[0x3C00_0240]` = the running task. `gp` = 0x3C00_0100.
* **API table**: `[[gp+4]+0x10]` (Cars 0x800C_F158, 0x88 entries). Seen: 2 = lwevent get bits, 3 = lwevent set
  (0x8002_FEE4), 5 = lwevent wait (mask, all), 0x12 = install an ISR (vector, function, data), 0x4B, 0x52/0x53
  (message queues, HYPOTHESIS), 0x66/0x67 = interrupts on/off around the idle clock switch, 0x74 = debug
  `printf` to the console, 0x7D = stack check (scans for the fill pattern), 0x7F = start a timer (callback,
  mode, period).
* **Object tags** (word at +0x14 of each object, little-endian ASCII as stored in memory): `nvel` = lightweight
  event group (+0x08 waiter list, +0x10 waiter count (u16), +0x18 bits), `mswl` = lightweight semaphore,
  `xtum` = mutex, `qkst` and `ssem` (UNKNOWN), `lmep` = memory pool, `rmit`/`timr` = timer component
  (Cars 0x3C1D_1CEC: its own task waits on a semaphore that the tick ISR's timer hook 0x8003_7DA0 posts),
  `kats` = unused stack fill.
* **Task blocks**: +0x08 state (0x42B waiting on a semaphore, 0x42D on an event group), +0x0C priority,
  +0x10/+0x14/+0x18 stack top/saved sp/stack base, +0x30 the wait list it is queued on, +0xBC its wait-list node,
  +0xC4 the event mask it waits for.
* **Serial console**: an interrupt-driven tty on the debug UART (0x0180_D500, line 0x1B): driver 0x8003_73B4-
  0x8003_7710 in Cars (baud divisor, ISR 0x8003_762C, transmit start 0x8003_76E8). `printf` blocks when its
  queue is full until the transmit interrupt drains it. Ribbit models this since 2026-10-15 (before, Cars hung
  on a white screen after printing "[Error] version failed").

## Debug output

Both the BIOS and games keep their debug logging in release builds. In Ribbit, `scenes logtrace <PC> r2 <ms>`
prints it:

| Function | Arguments | Example output |
|---|---|---|
| BIOS 0x400A_1250 | level, module, format in r2, args | `PlayMovie:"Packed RIB Movie(s)",handle:0x80000000`, `Request URL: loading movie ...`, `LfSet*Volume` |
| BIOS 0x400A_1434 | level, file, message in r2, line | assertions |
| Cars 0x800A_B838 | level, module, format in r2, args | `{AudioInit}`, `{PlayRAW(x21,0)}`, `{TONE0Done(0,0)}` |
| Cars 0x800A_B6C4 | condition, message | checks |
| Cars 0x800A_C174 → API 0x74 | format in r0 | `[Framework]cart version: %s`, `[Error] version failed: %s != %s` (goes to the UART) |

## Cars boot, as a worked example (2026-10-15)

1. 23 ms: the cartridge's MQX starts (`{XSyncInit}`, `{AudioInit}`); 47 ms: the BIOS calls PEG Core Init.
2. 0.6 s: the start-up jingle (`LfPlayAudioRaw`). The BIOS shows the LEAPSTER and LeapFrog logos.
3. 8.1 s: `PlayMovie` of the cartridge's movies into layers 10, 99, 97.
4. 8.3 s: the framework checks the save's version and prints the result over the UART.
5. Then the country-select screen (U.S.A. / Canada / U.K.).
