# LeapPad Explorer (LeapPad 1, 2011)

LeapFrog's first LeapPad tablet, codename "Madrid": the Leapster Explorer's LF1000 platform with
a 5" 480x272 touch screen, a camera and a microphone, and its 2 GB of storage on the SD
controller instead of NAND. Not to be confused with the 1999 LeapPad book reader
(`crates/ribbit-leappad`). Code: `crates/ribbit-didj/src/leappad.rs` (Original),
`sd.rs`, `ext2.rs`, `hle/leappad.rs` (Enhanced (new)); the app's LeapPad Explorer tab.

## Firmware

LeapFrog Connect's set for it (English: 45 files, French: 44), `EnglishLeapPadExplorer.xml`
(PackageList device "LeapPad", models 101 and others, mandatory version 2.4.10.3030). The
packages say `Device="LeapPadExplorer"`, IDs `LPAD-0x001E...`:

- **FW Base.lfp** (`LPAD-0x001E0012-000001`, Firmware-Lpad-Base 2.4.10.3030, built 2012-09-18):
  `firmware/sd/1raw/1/FIRST_Lpad.cbf` (the boot splash: a CBF holding the Explorer's 32-bit
  RLE, 480x272), `sd/1raw/2/kernel.cbf` (Linux 2.6.31-leapfrog, an **uncompressed** Image in a
  CBF, load and entry 0x8000), `sd/2ext4/3/rfs` (the root file system as a **tar**, 75 MB).
- **FW Partition.lfp**: `sd/0partition/mbr2G.image`, the card's MBR.
- **FW Bulk Empty.lfp**: `sd/2ext4/4/Bulk_Empty`, a tar of Bulk's empty folders.
- The `.lf2` packages (bzip2 tar, as the Explorer's): language packs (`LPAD-0x001E000F-000000`
  shared, `-000001` English), Camera/Gallery/Keyboard/Microphone/Paint/Video/Story Gallery
  widgets, Camera and Gallery apps, Included Pet, My Books, Sneak Peek, trailers, themes,
  patches. FW Base/Partition/Bulk Empty come again as `.lf2` (DiskImage; skipped).
- **Surgeon.lfp**: the recovery kernel (not used).

The paths name the layout: `<n><kind>/<partition>/<file>`, `n` the order, `partition` the
card's partition table, `raw` a file written at the start of the partition, `ext4` a tar
unpacked into a fresh file system there.

The root file system is **the same as the Leapster Explorer's 2.x firmware**: `/etc/setplatform`
calls board IDs 1, 2, 6, 7, A "EMERALD" and B, C, D "MADRID"; `/LF/Base` holds both
`EmeraldTitles` and `LPAD`/`LpadAssets`/`MadridTitles`.

## Hardware (from the firmware, LeapFrog's kernel source and Ribbit's boots)

| | LeapPad Explorer | Leapster Explorer | Source |
|---|---|---|---|
| SoC | LF1000 (Pollux, ARM926EJ-S, 393 MHz) | same | kernel "ARM-LF1000", mach-lf1000 drivers; TCRF |
| SDRAM | 64 MB at 0x8000_0000 | same | board_ids.h "64MB SDRAM" |
| Storage | 2 GB "MLC Flash + SD Controller": an SD card on SD channel 1 | 512 MB NAND (UBI) | board_ids.h; VERIFIED: the kernel drives channel 1 only, finds no NAND |
| Partitions | MBR: p1 1 MB raw (splash), p2 8 MB raw (kernel), p3 120 MB ext (root), p4 1.79 GB ext (Bulk); 1.92 GiB in all | MTD: Emerald_Boot, I18n_Screens, Kernel, RFS, Bulk | mbr2G.image; /etc/init.d/mounts |
| Board ID | 0x0B MADRID (0x0C with TV out, 0x0D with the LFP100) | 0x02 | board_ids.h; rcS shows Madrid-Boot-logoCW.png for B only |
| Screen | 480x272 (LCD480x272 / ILI6480G2, 9.2 MHz), 5" | 320x240 (ILI9322) | kernel's screen modules; "screen: 480x272" |
| Orientation | menus drawn on the panel turned clockwise (a tall home screen); the keyboard widget drawn upright | landscape | screenshots; Madrid-Boot-logoCW.png; libDisplay SetOrientation/SetAutoRotation, libButtonMPI SetDpadOrientation |
| Touch | 4-wire resistive, port A 11-14, ADC 4-7 | same | lf1000_ts2.c (first_adc 4 unless Didj) |
| Buttons | d-pad (left A29, up A28, down C3, right C4), Home (C6), volume up C2 / down C14, power | d-pad, A, B, L, R, hint, pause, home, brightness, volume | gpio_map.h MADRID_*; the 2.6.31 kernel's keypad: A, B, shoulders, HOME, hint, pause, brightness undefined, ESC defined |
| Camera | USB UVC camera on the LF1000's OHCI host (0xC000D000, IRQ 28) | none | kernel: uvcvideo, lf1000-ohci; "LF1000 OHCI" |
| Microphone | GUESS: the camera's USB audio (snd-usb-audio.ko in /lib/modules) | none | |
| Accelerometer | BMA220 on I2C (lf1000-aclmtr) | none | board_ids.h "Accelerometer, Vibration motor"; kernel bma_detect |
| Vibration | GPIO A30 (MADRID_VIBRATE) | LED enable there | gpio_map.h |
| Audio codec | CS43L22 on I2C (as the Explorer's) or the LFP100 (0x0D) | same | kernel log "found CS43L22" |
| Wi-Fi | "SDIO WIFI" in board_ids.h; lib80211.ko only | none | |

## Original (LLE)

`leappad::card_from_packages` builds the card: the MBR, the raw files, and each tar as an ext2
file system (`ext2::build`: 4 KiB blocks, 128-byte inodes, file types, no journal, no extents,
no sparse superblocks; the `.lf2` packages installed into Bulk's tree as on the Explorer,
`explorer::install_lf2`). The kernel's ext4 driver mounts them ("mounted filesystem without
journal"); `/etc/init.d/mounts` mounts p4 with ext4-only options (`noauto_da_alloc`), so ext4
it is. GUESS: the real updater's mkfs options. The card lives in a `Nand` used as plain storage
(`Nand::write_bytes`), so saves (`.nandblocks`), states and the app's save code are the NAND
consoles'.

`leappad::boot` does the boot loader's job as `explorer::boot` does (`explorer::start_kernel`):
the kernel from p2 at 0x8000_8000, ATAGs, the Alive scratch register with board ID 0x0B, the
splash from p1 on MLC layer 0. Command line, GUESSED (the boot loader's is UNKNOWN): the
Explorer's with `screen_module=LCD480x272 root=/dev/mmcblk0p3 rw rootfstype=ext4 rootwait`.

`sd.rs`, the SD controller (MagicEyes' DesignWare MMC, `mes-sdhc`, 0xC000_9800 and
0xC000_C800, IRQs 14 and 42, DMA requests 16 and 30; registers from LeapFrog's
drivers/mmc/host/mes_sdhc.h) and a high-capacity SD card. Its DMA commands wait for the bus
(`Dma::serve_sd`, `Bus::sd_pump`). Interrupt flags come 4000 cycles after the command:
Linux's MMC core retries a failed command from the interrupt handler, which then cleared the
flags it had read, the retry's among them when commands finished at once (the card's CMD5
timeout was lost and the card never found). GUESSES: the card's CID/CSD (manufacturer, name
"LPAD1"); its size is the MBR's end.

The NAND controller finds no chip (ID FF): the LeapPad has none; cartridges are still NAND on
that controller. OHCI isn't modelled: "USB HC reset timed out", so no camera (and microphone);
the BMA220 doesn't answer (no accelerometer).

VERIFIED (explorerboot, 2026-11-15): kernel log "mmc0: new high speed SDHC card at address
4c50", "mmcblk0: p1 p2 p3 p4", root and Bulk mounted, rcS, AppManager; the setup (Select
Language, Date and Time, Create Profile with the on-screen keyboard) by scripted taps, then the
home screen: Cartridge, Pet Pad, Camera, My Stuff, My Books, Sneak Peeks and the pet.

Board IDs 0x0C and 0x0D boot the same (rcS then shows the Explorer's LEGAL.png).

## Cartridges (Original)

The Explorer's slot and cartridge model (`cart.rs`, NAND on the controller). Tested 2026-11-15
(explorerboot, a set-up LeapPad on its home screen, the dump put in, the Cartridge square
tapped, the screen up to 26 s later), the 16 LeapPad cartridges among the Explorer dumps
(meta.inf `Device="LeapPadExplorer"`, which the Explorer refuses) and 3 Explorer ones:

- LeapPad games at their title or first screen (14 of 16): Cars 2 - Project Undercover, Cinderella, Tink's Midnight
  Tea Party, Dora's Amazing Show, Leap School - How Not to Clean Your Room, Learn to Read
  Adventure Stories, Fairy Tales, Mysteries, Letters on the Loose, Mini Game Greatest Hits,
  Mr. Pencil - The Lost Colors of Doodleberg, Pet Pals - Dog Show Detectives, SpongeBob - Best
  Day Ever, Thomas & Friends - The Great Penguin Rescue.
- Explorer games: Cars 2 plays in the LeapPad's frame for Explorer games (on-screen A, B, L, R,
  hint and pause buttons beside the picture).
- Black after 26 s, every layer off (still loading, or waiting for something not modelled):
  Disney Planes, Sofia the First (LeapPad), Toy Story 3, Frozen 1.2.1.0 (Explorer).
- Home (the Leapster's Menu key, pin C6) leaves a game for the home screen (Mr. Pencil).

The LeapPad's own games draw upright on the panel (landscape), its menus turned (portrait):
the app's "Screen upright" (LeapPad Explorer menu) shows either.

Before the doom files were removed (`card_with_files`), every game said "Connect your LeapPad
Explorer to your computer to download the language pack for this game".

## Enhanced (new) (HLE)

The Explorer's HLE (`hle/explorer.rs`) with `hle/leappad.rs`: the root and Bulk read from the
card's p3 and p4 (`ext2::Reader`, `system_from_card`), /sys answering board ID B, xres 480,
yres 272 and the touch calibration, the frame buffer set up for 480x272
(`Process::new_explorer_sized`; ILI6480G2 timings). Saves build the changed volume again as
ext2 on the card (`write_changed_to_card`), so Original boots them.

VERIFIED (explorerhle): Select Language from the packages; with Original's save, Sign In, the
profile tapped, the home screen.

## The app

- Library: a "LeapPad Explorer" tab (`Family::LeapPadExplorer`, `Hardware::LeapPadExplorer`);
  FW Base.lfp is the console (`didj::describe_leappad`), the folder's other packages aren't
  listed (`is_leappad_package`). Icon: a white-faced tablet in a green shell held upright,
  silver d-pad, green Home button, orange stylus (the green and white model).
- Session: the Explorer's (`DidjSession::build_explorer` builds a LeapPad when the folder holds
  its packages; `build_leappad`), the same settings: Original / Enhanced (new) is the Explorer's
  mode setting, frame skip, HD, saves as `leappad-explorer-<folder>.nandblocks` (+ `.nor`).
- The picture is shown upright, 272x480 (`didj::rotate_upright`), or as the panel with
  "Screen upright" off (LeapPad Explorer menu; its games); the mouse is the stylus, mapped back
  to the panel (`leappad_panel_point`).
- Keys: the d-pad, Home (the Leapster's Menu key), volume up/down.
- "Turn off when idle": the 2.x firmware's libLightningBase.so (UpdateInactivity at 0x354BC,
  its timer at 0x65904, shutdown past 240 s) is in `idle.rs`; off (the default), the timer is
  kept at 0 on both modes, so it never idles off.

## Left

- The camera (OHCI and a UVC device giving a test picture) and the microphone (USB audio):
  absent, the apps say there is no camera.
- The accelerometer (BMA220 on I2C): absent. The firmware rotates the display (libDisplay
  SetAutoRotation) and the d-pad (SetDpadOrientation) by it; games that ask for landscape draw
  upright on the panel and show turned in the app (the keyboard widget does).
- Which board ID retail units report, the real command line, the card's real CID/CSD, the real
  touch calibration (Ribbit's is the Explorer's scaled).
- The Home key's code: the 2.6.31 kernel's keypad defines ESC, not HOME, for Madrid.
- Saves made on another card image are set aside, not carried over.
- Installing games from the library (the Explorer's `connect::lx` works on UBIFS).
- Cartridges on Enhanced (new): the menu's Cartridge square stays crossed out with Mr. Pencil
  in (the Explorer's HLE slot; not looked into). Four cartridges stay black on Original.
- Which way up the real unit is held for its menus and for its games (the firmware draws the
  menus turned and the games not); the accelerometer would tell the firmware.
