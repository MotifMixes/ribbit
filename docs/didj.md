# LeapFrog Didj

The Didj (2008, codename "Lightning") is a handheld built on the LF1000, LeapFrog's version of the MagicEyes
Pollux VR3520F: an ARM926EJ-S at 393 MHz with 32 MB of SDRAM, a 320x240 LCD, a 3D engine, and 256 MB of
internal NAND. It runs Linux 2.6.20 with LeapFrog's Brio libraries; games come on NAND cartridges.

**Status: boots to the menu; the start-up screens show (health warning, copyright, the "didj" logo video).**
The menu itself is drawn by the 3D engine, not modelled yet, so it shows black. `crates/ribbit-didj` holds the
formats, the NAND image, and the machine: ARM926EJ-S, NAND, timers, DMA (sound), and the MLC display compositor.

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
  libopengles_lite.so): see "3D engine" below.

Now: BusyBox init runs `/etc/init.d/rcS`, which mounts the Brio volume Ribbit built (below) as `/Didj`,
starts monitord and `launch_main`, and gives a `#` shell on ttyS0. `launch_main` starts AppManager, which
loads the menu (`BLT/BLT.so`), plays the splash (VideoTask, the logo animation) with its sounds (heard
in `DIDJ_WAV`: about 13 s), leaves the splash state and waits for input ("Screen brightness set for low
power"). The menu shows its first-start "Select your Country" screen (drawn by the 3D engine).

Known gaps: no LCD (SPI0 ID 0x96) or audio codec (I2C0) yet.

Buttons: GPIO port C pins, low while pressed (lf1000-keypad.c): left 0, right 1, up 2, down 3, R 4,
L 7, A 8, B 9, menu 10, hint 11, pause 12, brightness 13 (L/R swapped on boards before EP1). The driver
uses *level* interrupts: it waits for low level, and after 4 interrupts in a row (its debounce; the
level keeps re-setting the flag after each clear) reports the press and switches the pin to high
level for the release. GPIO ports A-C sit 0x40 apart from 0xC000A000: DETMODE0/1 (2 bits a pin:
low, high, falling, rising), INTENB 0x10, DET 0x14 (write 1 to clear), PAD 0x18; IRQ 13.
Headphones are port B pin 10 (not modelled; reads low).

`didjboot` options (environment): `DIDJ_KEYS="50000:RIGHT 60000:A"` holds a button for 30 million
cycles at that many million cycles; `DIDJ_TYPE=$'ps\nls /Didj'` types one line at each new shell prompt;
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

## Display (VERIFIED in Ribbit unless marked)

The MLC (0xC000_4000, kernel drivers/lf1000/mlc) composes the 320 x 240 picture over a background colour
(white at the menu). Registers at the menu, 1.35.2.4222: SCREENSIZE 0x00EF013F (the size less one), CONTROLT
0x0C02 (enabled, video on top). Layer 0: A8R8G8B8 (format 0x0653) at 0x01200000, the kernel's frame buffer
(LF1000_MLC_FB_BASE, 18 MB): the start-up screens. Layer 1: R5G6B5 (0x4432) at 0x21500000, 4096-byte rows: the
3D engine's output (the menu). Layer 2 (video): YUV 4:2:0, Y 0x0124B000 (its block bit kept elsewhere: the driver
mentions MEMCONTROL), Cb 0x2124B800, Cr 0x212A3800, 4096-byte rows, scale across 0x800 (1:1) and down 0x5DB.

Addresses with bit 29 set are "block" (2D) addresses: segment (16 MB of SDRAM) in bits 24-28, row V in
12-23, byte U in 0-11 of a 4096 x 4096-byte square. Software maps them (through the MLC device's mmap) and
writes them directly (the splash video does). The memory controller stores the square in 2 KiB blocks of
64 x 32 bytes, 64 blocks to a row (POLLUX data book 22.2.4, Table 22-2 "display mode"): SDRAM offset =
segment << 24 | V[11:5] << 17 | U[11:6] << 11 | V[4:0] << 6 | U[5:0]. Texture mode (Table 22-3, how the 3D
engine reads textures and palettes) stores each block as 4 x 2-byte cells: ... | V[4:1] << 7 | U[5:2] << 3 |
V[0] << 2 | U[1:0]. Segment 1 (0x21xxxxxx) is SDRAM 16-32 MB, above the kernel's `mem=18M`
(`mlc::block_sdram`; VERIFIED: the menu's textures, written by the CPU at plain SDRAM addresses, appear).

## 3D engine (POLLUX data book Release 0.91, chapter 22, unless marked)

The data book is at gp2x.dev (the GP2X Wiz has the same chip). It documents the whole engine; Ribbit models it
in `crates/ribbit-didj/src/ga3d.rs`. What the menu (1.35.2.4222) uses is VERIFIED by it drawing correctly.

- Command processor: a queue in SDRAM (the menu's: 0x01300000-0x0131FFFC). 0x04 CMDQSTART, 0x08 CMDQEND (the
  last word), 0x0C CMDQFRONT (software's write pointer), 0x10 CMDQREAR (the engine's read pointer; GUESS:
  reset by writing CMDQSTART), 0x14 STATUS (idle flags 0xBF0000). Commands by bits 28-31: 15 = register fill
  (bits 16-24 count - 1, bits 0-10 register word index, bits 11-15 wait/interrupt flags), 4-13 = one-word
  head that is also the first data word, written to register CMDID * 2 (1, 1, 1, 1, 2, 2, 2, 2, 1, 1 words;
  9 also loads the palette), 14 = NOP. The library waits for idle with CONTROL 0x40000004 (NOP flag).
- CONTROL (0x20) starts a sub-module: 0 the primitive processor, 1 the GTE, 2 the TSE, 3 the rasterizer
  (mode bits 3-4: 1 memory fill, 2 LUT fill, 3 texture cache clear).
- Primitive processor (PRIMCON1 0x100: bit 0 GTE, 1 matrix palette, 2 index buffer, 6-7 index size,
  8-15 valid streams): indices from IDXSTART to IDXEND (0x104, 0x108, inclusive), up to 8 vertex streams
  (0x120 + 12n: address, stride, param = destination word bits 20-31, count - 1 bits 8-19, type 0-7), a
  per-vertex control table (PPARAM 0x110-0x11C, end/loop at 0x10C). The menu: indexed triangle lists, 16-bit
  indices, stream 1 positions (s15.16 into C0), stream 4 texture coordinates (into C4).
- GTE: a 4-wide float vector processor; 512 instructions at 0x800, 252 input/constant vectors at 0x1000.
  Instruction bits: 27-31 ID, 26 output (vertex 24-25, 3 = the current one; component 22-23: position,
  colour A R G B, texture coordinates) or working register 22-25, sources at 17-21 and 12-16 (bit 4 =
  constant, paged by SETPAGE), output modifier 7-11, write mask 3-6, delay 0-2. SETPAGE (31): page 1 in bits
  8-11, page 0 in 0-3 (the data book's figure and example disagree; the menu's program only makes sense
  this way). The modifier table (22-22) is printed one row late: 0 = xyzw, 3 = wxyz, 24-27 = xxxx..wwww,
  28-31 = -x..-w. MAD (11) adds the last MUL result (GUESS). Pipeline timing isn't modelled.
- Clipper: 0 <= x, y, z <= w, then Xs = left + W * Xc / Wc, Ys = bottom - H * Yc / Wc (VPORTBOT, VPORTH,
  VPORTLEFT, VPORTW at 0xF0-0xFC, floats; the menu: 239.6, 240, -0.5, 320, so pixel centres are at whole
  coordinates).
- Rasterizer: render target and Z buffer are 16-bit sub-segments (11-bit ids: segment << 3 | n; n's
  2048 x 1024-byte area is at U (n & 1) * 2048, V (n >> 1) * 1024) through 2-bit-per-256-pixel address tables.
  The menu: render target sub-segment 0x0A, three buffers (rows 0, 256, 512), Z at rows 768, 0x319E clear.
  Textures: TEXSUBSEGMENT {segment, page[4:2]} + TEXINFO page[1:0]; a page is 1024 x 512 bytes, 32 to a
  segment in 2 x 2 groups. The menu uses R4G4B4A4 (mode 41) textures modulated by the vertex colour, alpha test
  GREATER 0, alpha blending (source alpha, 1 - source alpha) and Z LESS.
- Not modelled yet: fog, dithering, mipmaps, tile-indexed textures, the Z-valid buffer, interrupts, timing.
  Memory fill's value: GUESS bits 0-15 (the data book says 8-15).

`didjboot` with `DIDJ_FRAMES=<folder>` saves the screen every 100 million instructions when it changed, and
`DIDJ_FRAME=<file>` the last one (PPM).

## Plan

1. **M1 (done):** these notes, the package reader, and the NAND image.
2. **M2 (done):** boot lightning-boot and the kernel to its UART log (ARM926EJ-S from `ribbit-arm`, SDRAM, NAND
   controller, UART, timers, interrupt controller). Since then: user space, the Brio partition, DMA,
   AppManager and the menu running (splash sounds play).
3. **M3:** interrupts, timers, GPIO and the NAND controller, enough for the kernel to mount its file systems.
4. **M4:** display (MLC/DPC). The MLC's RGB and video layers draw (2026-10-29); DPC timing not modelled.
5. **M5:** audio and buttons.
6. **M6:** 3D engine. The command processor, primitive processor, GTE, clipper, TSE and rasterizer draw the
   menu (2026-10-29).
7. **M7:** frontend, saves, and an Enhanced/Original mode toggle.
8. **M8:** speed (ARM JIT).
