# LeapFrog Didj

The Didj (2008, codename "Lightning") is a handheld built on the LF1000, LeapFrog's version of the MagicEyes
Pollux VR3520F: an ARM926EJ-S at 393 MHz with 32 MB of SDRAM, a 320x240 LCD, a 3D engine, and 256 MB of
internal NAND. It runs Linux 2.6.20 with LeapFrog's Brio libraries; games come on NAND cartridges.

**Status: boots to Linux user space, no screen yet.** `crates/ribbit-didj` holds the formats, the NAND image (M1)
and a boot machine (M2): the real lightning-boot and kernel run to the root file system mount and the first user
process; init runs its scripts to a shell (see "Boot log").

Tags: VERIFIED = checked on the firmware files or the GPL source; UNKNOWN = not known yet.

## Sources

- LeapFrog's GPL release, mirrored as [opendidj](https://github.com/BackupTheBerlios/opendidj): the kernel
  (`linux-2.6.20-lf1000`), lightning-boot, uniboot, u-boot. The packaged kernel is "Linux-2.6.20.1-leapfrog",
  the same version.
- [lightning-boot-2.0](https://github.com/Reggi3/lightning-boot-2.0) (a later version of the bootloader).
- eLinux.org's Didj pages (partition map).
- MAME's `magiceyes_pollux_vr3520f` driver (a skeleton).
- The firmware 1.35.2.4222 packages from LeapFrog's server (`digitalcontent.leapfrog.com/packages/DIDJ/`).

Ribbit keeps these sources in `local/didj-src/` (git-ignored) and the packages in `didj/` (git-ignored).

## Boot chain (VERIFIED by running the packaged files, except where marked)

1. The LF1000 loads the start of NAND into SDRAM at address 0 and runs it. The packaged lightning-boot.bin
   starts with a bootstrap that reads NAND 0x800-0x1FFFF to SDRAM 0x800 itself (seen in the binary; in the
   opendidj source that call is commented out), so the hardware needs to load only the first 0x800 bytes, as the
   README says. How SDRAM is set up first is UNKNOWN. Ribbit copies the whole first block.
2. lightning-boot latches the power on (Alive GPIO VDDPWRON), reads the board and cartridge IDs from GPIO
   pins into a scratch register, and reads the boot flags (JFFS2 in Atomic_Boot_Flags) to pick Kernel0/RFS0
   or Kernel1/RFS1. It shows `bootsplash.rgb`, loads `uImage` with its 64-byte header at 0x8000 - 64, builds
   ATAGs (CORE, MEM, CMDLINE) and jumps to the entry point (0x8000).
3. The kernel mounts the root file system (JFFS2) and the Brio partition. Brio's AppManager then runs the
   menu and games.

The packaged lightning-boot prints nothing on the UART (no `DEBUG`). It checks the battery on ADC channel 2
(mV = 1957 * reading / 256 + 190, at least 4400) and waits for PWRMODE's CHGPLL bit to clear.

## Boot log (Ribbit, firmware 1.35.2.4222)

`cargo run --release -p ribbit-didj --example didjboot -- didj 15000` (15 billion cycles, about 3 minutes)
prints the UART and the registers the software touched that have no model yet. The kernel reports:

- "Linux version 2.6.20.1-leapfrog (gstanfield@lightning-release) (gcc version 4.1.1) #9 Wed Apr 22 15:32:34
  PDT 2009", CPU ID 0x41069265, PLL0 393.218 MHz, PLL1 147 MHz, timers 4.593 MHz (PLL1 / 32), HZ 100.
- Command line: `mem=18M init=/sbin/init console=ttyS0,115200 root=31:04 ro rootflags=noatime
  rootfstype=jffs2 ubi.mtd=Brio ubi.mtd=prg_Brio ubi.mtd=Cartridge ubi.mtd=EXT ubi.mtd=prg_EXT`.
- The NAND driver uses Linux's software ECC (3 bytes per 256 in spare bytes 40-63; HWECC is off in
  lf1000_defconfig). With erased spare bytes JFFS2 sees flipped bits, so Ribbit's image carries that ECC.
- It expects LCD chip ID 0x96 over SPI0 and an audio codec on I2C; Ribbit answers neither yet.

What it took to get past the kernel (each a hardware behaviour Linux relies on):

- MMU access permissions and alignment faults (ARM926EJ-S). Without permission faults, writes to
  copy-on-write pages went straight into shared pages and corrupted the slab allocator (oops in
  `cache_alloc_refill`, NULL at 0x4). Linux runs with c1's A bit set, so unaligned accesses fault too.
- MLC dirty flags: software sets MLCCONTROLT bit 3 or a layer's control bit 4 (0x24, 0x58, 0x7C) and waits
  for the hardware to clear it at the next frame. Ribbit clears them once per frame (60 Hz, GUESS).
- UART0 "transmit empty" interrupt (IRQ 10; INTSTATUSREG 0x18: pending bits 0-3, write 1 to clear, enables
  bits 4-7). The kernel's printk polls, but user space's console writes wait for this interrupt, so `rcS`
  hung on its first `echo` without it.

- Cartridge detect: GPIO port A pin 18, low when a cartridge is in (nand.h `NAND_CART_DETECT_LEVEL`;
  `launch_main` reads it with `gpio-control /dev/gpio invalue 0 18`). Reading low, the kernel probed the
  internal NAND as a cartridge and `launch_main` stopped at REINSERT_CART_STATE. Ribbit reads it high
  (empty slot): "lf1000-mtd: cartridge not inserted".
- UART0 receive: RX byte at 0x12, data ready in TRSTATUS bit 0 / FSR bits 0-3, interrupt pending bit 1 and
  enable bit 5 of 0x18 (8250-lf1000.c). `didjboot` types commands into the shell with it.

- DMA (dma.c): 8 channels 0x80 apart from 0xC0000000, each with a command queue at +0x10 (CMDSPACE 0x2C
  = free slots; size GUESS 7); IRQ 3. Audio goes out in 4 KiB fragments to PCM out (request 24), so
  Ribbit paces those at 4 bytes per sample at 32000 Hz. Before this AppManager stopped at "HW is not
  busy, but fifo is not empty".
- GA3D, the 3D engine (0xC001A000, mapped to user space by /dev/ga3d, driven by the closed
  libopengles_lite.so, MagicEyes "MES_GRP3D" HAL): 0x00 queue on, 0x04 queue start and 0x08 its last word
  (physical, 8-byte entries), 0x0C rear (software's write pointer), 0x10 front (the engine's read
  pointer), 0x14 status (idle = bits 0xBF0000 all set). The menu waits for the front to reach the rear.
  Ribbit's stand-in (until M6): always idle, every command read at once, nothing drawn.

Now: BusyBox init runs `/etc/init.d/rcS`, which mounts the Brio volume Ribbit built (below) as `/Didj`,
starts monitord and `launch_main`, and gives a `#` shell on ttyS0. `launch_main` starts AppManager, which
loads the menu (`BLT/BLT.so`), plays the splash (VideoTask, the logo animation) with its sounds (heard
in `DIDJ_WAV`: about 13 s), leaves the splash state and waits for input ("Screen brightness set for low
power"). Nothing is shown yet.

Known gaps: no LCD (SPI0 ID 0x96), audio codec (I2C0), display, 3D drawing or buttons yet. The buttons are
GPIO port C pins 0-4 and 7-13 with edge interrupts (lf1000-keypad.c); headphones port B pin 10.

`didjboot` options (environment): `DIDJ_TYPE=$'ps\nls /Didj'` types one line at each new shell prompt;
`DIDJ_WAV=<file>` saves the sound sent to PCM out; `DIDJ_NAND=<file>` saves the Brio partition (pages with spare bytes); `DIDJ_NO_BRIO=1` leaves Brio
erased (the console then formats it itself); `DIDJ_DUMP=<file>` saves SDRAM.

## Brio partition (VERIFIED: what the kernel and rcS write to an empty one, in Ribbit)

- UBI version 1: EC header at byte 0 of each 128 KiB block, VID header at 512 (512-byte subpages), data
  from 2048, so a LEB is 129024 bytes. 1776 blocks: 2 for the layout volume, 1 for wear-levelling,
  17 (1%) for bad blocks, 1756 for the one user volume, id 0, dynamic, named `ubi_Brio`
  (`ubimkvol --maxavsize`). Volume table: 128 records of 172 bytes.
- In it, through gluebi/mtdblock, FAT16 from `mkfs.vfat -n Didj -S 512`: 442512 sectors, 8 per cluster,
  1 reserved, 2 FATs of 216 sectors, 512 root entries, boot sector OEM "mkdosfs", the label also as the
  root's first entry.
- Ribbit builds the same (crates/ribbit-didj/src/ubi.rs, brio.rs; FAT writing in ribbit-core fatfs.rs)
  and installs every package that isn't raw firmware: system packages and the avatar under
  `/Didj/Base/<folder>`, applications under `/Didj/<folder>` (GUESS). Each package is copied whole,
  meta.inf and .md5 lists too (what LeapFrog Connect copies is UNKNOWN).

## Root file system (VERIFIED: erootfs.jffs2 in 1.35.2.4222)

- `/etc/ubi/ubi.config`: `Brio:/Didj:Didj prg_Brio:/opt/Didj:Didj Cartridge:/Cart:Cartridge
  EXT:/opt/mfg:MFG_PAYLOAD prg_EXT:/opt/prg_mfg:MFG_PAYLOAD` (MTD, mount point, FAT label).
- `/etc/init.d/ubi`: each UBI device gets one volume, id 0, named `ubi_<MTD>` (`ubimkvol --maxavsize`),
  used through its MTD block device as FAT (`mkfs.vfat -n <label> -S 512`, mounted `vfat`).
- `/usr/bin/launch_main` runs `AppManager` (or `/flags/main_app`) from PATH
  `/Cart/Base/Brio/bin:/Didj/Base/Brio/bin/:/Didj/Base/bin/`; libraries come from
  `/Didj/Base/Brio/lib/:/Didj/Base/lib/`. So the system packages (meta.inf `Type="System"`,
  `Name="Base_<dir>"`) unpack under `/Didj/Base/` (Base_bin holds `bin/AppManager`). Where applications
  (`Type="Application"`, e.g. JetPack Heroes `POW/`) go is UNKNOWN; GUESS `/Didj/<dir>`, like a cartridge's
  `/JDI` at its root.

## Internal NAND (VERIFIED: kernel source and packages)

There are 2048 erase blocks of 128 KiB. Each block has 64 pages of 2 KiB plus 64 spare bytes. A block is bad
when spare byte 0 of its page 0 or 1 isn't 0xFF. lightning-boot runs the controller's BCH decoder but ignores
its error flag. Whether the kernel checks ECC is UNKNOWN.

| Offset     | Partition          | Size      | Contents (Ribbit's built image)              |
|------------|--------------------|-----------|----------------------------------------------|
| 0x00000000 | LF1000_uniboot     | 128 KiB   | lightning-boot.bin                           |
| 0x00020000 | Atomic_Boot_Flags  | 896 KiB   | bootflags.jffs2                              |
| 0x00100000 | Manufacturing_Data | 1 MiB     | erased (real contents UNKNOWN)               |
| 0x00200000 | Kernel0            | 2 MiB     | kernel.bin                                   |
| 0x00400000 | Linux_RFS0         | 14 MiB    | erootfs.jffs2                                |
| 0x01200000 | Kernel1            | 2 MiB     | kernel.bin                                   |
| 0x01400000 | Linux_RFS1         | 14 MiB    | erootfs.jffs2                                |
| 0x02200000 | Brio               | 222 MiB   | UBI + FAT16 with the other packages          |

`kernel.bin` uses lightning-boot's "table file system". Its first block is a summary: `0x12345678`, a file
count, then a 64-byte name and a block count for each file. The files follow block by block, skipping bad
blocks. In 1.35.2.4222 the files are `bootsplash.rgb` (2 blocks) and `uImage` (7 blocks, 862824 bytes).

Build and check the image: `cargo run -p ribbit-didj --example didjnand -- didj [out.nand]`.

## Packages (VERIFIED: 1.35.2.4222)

A `.lfp` is a plain zip with a `meta.inf` (`Device="Didj"`). IDs are `DIDJ-0x000E....` for system packages and
`DIDJ-0x000F....` for games.

| ID         | Name                    | Contents                                         |
|------------|-------------------------|--------------------------------------------------|
| 0x000E0002 | Didj Device Bootstrap   | lightning-boot.bin, bootflags.jffs2              |
| 0x000E0003 | Didj Device Firmware    | kernel.bin, erootfs.jffs2                        |
| 0x000E0004 | Brio 1.35.0.4182        | Brio libraries (`lib/lib*MPI.so`)                |
| 0x000E0005 | Base_Fonts              | TrueType fonts                                   |
| 0x000E0006 | Base_LocalizedAudio     | Ogg Vorbis voice clips                           |
| 0x000E0007 | Base_Tutorials          | tutorial data and audio                          |
| 0x000E0008 | Base_UniversialArt      | shared images                                    |
| 0x000E0009 | Base_UniversialAudio    | shared sounds                                    |
| 0x000E000A | Base_bin                | AppManager, bad-word lists                       |
| 0x000E000B | Base_lib                | libLightning*.so, tutorial apps                  |
| 0x000E000C | BLT                     | the menu's screens                               |
| 0x000E0010 | AstroGuyHelmet          | an avatar                                        |
| 0x000F0001 | JetPack Heroes 1.3.1.0  | the built-in game                                |

Cartridge dumps are 69206016 bytes: small-page NAND, 512 + 16 bytes per page, 32 pages per 16 KiB block, 4096
blocks. They hold UBI with one volume, "ubi_Cartridge", which contains FAT32.

## Plan

1. **M1 (done):** these notes, the package reader, and the NAND image.
2. **M2 (done):** boot lightning-boot and the kernel to its UART log (ARM926EJ-S from `ribbit-arm`, SDRAM, NAND
   controller, UART, timers, interrupt controller). Since then: user space, the Brio partition, DMA,
   AppManager and the menu running (splash sounds play).
3. **M3:** interrupts, timers, GPIO and the NAND controller, enough for the kernel to mount its file systems.
4. **M4:** display (MLC/DPC).
5. **M5:** audio and buttons.
6. **M6:** 3D engine.
7. **M7:** frontend, saves, and an Enhanced/Original mode toggle.
8. **M8:** speed (ARM JIT).
