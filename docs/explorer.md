# Leapster Explorer

LeapFrog's Leapster Explorer (2010, codename "Emerald", product line LST3) runs on
the same LF1000 (MagicEyes Pollux, ARM926EJ-S at 393 MHz) as the Didj, so Ribbit
emulates it with the Didj's machine (`crates/ribbit-didj`, `src/explorer.rs`).
Status: boots Linux and LeapFrog's AppManager; plays Didj cartridges from its menu (Sonic tested); touch screen and buttons work in the
first-time setup (Language Selection, Set the Date); the NOR flash with factory data
is there. In the app (early): the library lists Firmware Base.lfp as the console and starts it from the packages beside it (built on a worker thread, so the window doesn't freeze meanwhile); the mouse is the stylus; Page Up / Page Down are the volume buttons; saves in explorer-<folder>.nandblocks and .nor, carried over to another image (below); Emulation > Leapster Explorer > Plug into this PC (USB) offers it over USB/IP (below). Speed about 100% in the app (tested by the user).

Run: `cargo run --release -p ribbit-didj --example explorerboot -- <folder> [million cycles]`
with the folder holding the English firmware set (archive.org item
`didj-leapsterexplorer-leappadexplorer`, folder EnglishLeapsterExplorer). Options:
`EXPLORER_SHOT=<png>` (the screen at the end), `EXPLORER_RAM=<file>` (SDRAM; the
kernel log is in it), `EXPLORER_BULK=<file>` (the Bulk partition as a UBI image),
`EXPLORER_TRACE=<n>` (interpret to the first exception and print the last n PCs),
`EXPLORER_JIT=0`, `EXPLORER_WAV=<file>` (the sound), `EXPLORER_INPUT=<million>:<what>;...` (`x,y` touches there, `lift`,
a button name, `none`, `insert` / `remove` the cartridge), `EXPLORER_SHOTS=<million>,...` (screens along the way),
`EXPLORER_CART=<file>` (a cartridge dump in the slot), `EXPLORER_STATE_OUT=<file>` /
`EXPLORER_STATE=<file>` (save the machine at the end / start from it; with
`EXPLORER_COLD=1`, power on afresh with that state's NAND).

First-time setup by script (times in million cycles):
`EXPLORER_INPUT="3050:100,93;3100:lift;3350:294,214;3400:lift"` picks English (US)
and reaches Set the Date at about 3700. Without touch data in the NOR the setup
starts with Screen Calibration instead (targets top left, top right, bottom right,
bottom left, centre, about 16 px in; it worked by script too).

## Hardware (as far as known)

| | |
|---|---|
| SDRAM | 64 MB at 0x8000_0000 (no shadow mode; kernel `PHYS_OFFSET` 0x8000_0000) |
| Boot flash | 512 KB NOR at 0, 8-bit: NOR_Boot 0x7E000, MfgData0 4 KB, MfgData1 4 KB (kernel table). Found by jedec_probe; Ribbit answers as SST39LF040 (0xBF 0xD7), INFERRED from the layout |
| NAND | 512 MB (4 Gbit) SLC, 2 KiB pages, 128 KiB blocks, at 0x2C00_0000. EVIDENCE: a real unit's /proc/mtd ends at 0x2000_0000 (Bulk 0x1A00_0000, ubi_bulk 0x1945_5000, Cartridge 64 MB in 16 KiB blocks); board_ids.h names an 8 Gbit part, and Ribbit used 1 GB until 2026-11-08. READ ID: Samsung K9F4G08 (EC DC 10 95 54), GUESS |
| Screen | 320x240 (ILI9322 module), MLC layer at `mlc_fb=0x82E00000` |
| Board ID | in the Alive scratch register (bits 8-12); retail values 0x01, 0x02, 0x06, 0x07, 0x0A (board_ids.h). Ribbit: 0x02, GUESS |

`/proc/iomem` of a real unit (eLinux.org, Leapster Explorer Memory Map) matches the
Didj's register blocks (DMA, DPC, MLC, ADC, SPI, GPIO, PWM, OHCI, audio, I2C, power,
RTC, IDCT, UDC, Alive GPIO, GA3D).

## Firmware packages

`EnglishLeapsterExplorer.xml` / `PackageList.lfp` list them (device LeapsterExplorer,
mandatory firmware 1.4.11.2128, IDs `LST3-0x0017....`).

* `.lfp` files are zips. "DiskImage" packages name their files `<partition>,<size>,<name>`:
  Firmware Base has `1,…,FIRST.32.rle` (splash), `2,…,kernel.cbf`, `3,…,erootfs.ubi`;
  Firmware Bulk Empty has `4,…,bulk.ubi`. Ribbit writes each raw at its partition.
* `.lf2` files are bzip2-compressed tars. The console's `/usr/bin/lfpkg` installs them by
  their meta.inf `Type`: Application to `/LF/Bulk/ProgramFiles`, LanguagePack to
  `/LF/Bulk`, Download/MicroDownload/… to `/LF/Bulk/Downloads`; a package with
  meta.inf at the top goes into a folder named after its `PackageID`.
* `kernel.cbf`, `surgeon.cbf`: "CBF" = magic 0x9ABCDEF0, version 1, load and jump
  address (0x0010_0000), payload size, header CRC (24 bytes), then a zImage.
* Without the `.lf2` packages AppManager shows ATTENTION_NEEDED.png (PENDING_REPAIR:
  "connect me to LeapFrog Connect").

## NAND partitions

From the kernel image's built-in table (partition_info, Firmware Base 1.4.11.2128):

| Partition | Offset | Size |
|---|---|---|
| Emerald_Boot | 0 | 1 MB |
| I18n_Screens | 0x0010_0000 | 1 MB |
| Kernel | 0x0020_0000 | 8 MB |
| RFS | 0x00A0_0000 | 86 MB (UBI, volume 0 UBIFS root) |
| Bulk | 0x0600_0000 | the rest (UBI volume `ubi_bulk`, UBIFS) |

UBI: VID header at 512, data at 2048 (LEB 129024 bytes), as on the Didj. The kernel
uses Linux's software ECC (3 bytes per 256, spare bytes 40-63), as the Didj's. Its
2.6.31 driver reads subpages with "random data out" (commands 0x05, 0xE0).

## Booting (what Ribbit does for Emerald Boot)

The packages carry no boot loader, so `explorer::boot` does its job:

* The kernel from the Kernel partition (the CBF's zImage, gunzipped) at 0x8000_8000,
  ATAG tags (core, memory, command line) at 0x8000_0100, r1 = machine 2028
  (machine_desc "ARM-LF1000"), SVC mode, MMU off.
* Command line as shipped (eLinux.org, Leapster Explorer Random Notes):
  `init=/sbin/init console=ttyS0,115200 mem=46M mlc_fb=0x82E00000,0x01200000
  ram=0x80000000-0x83FFFFFF screen_module=ILI9322 ebs=0x20000,0x0 root=ubi0_0 rw
  rootfstype=ubifs ubi.mtd=RFS`.
* PLLs as the Didj's lightning-boot sets them (PLLSETREG0 0x00DF2100 = 393.2 MHz,
  PLLSETREG1 0x0024C402 = 147 MHz, CLKMODEREG 0x00200080); with them unset every
  clock divider is a division by zero and the PWM, DPC and ADC drivers fail.
  GUESS: Emerald Boot's own values are UNKNOWN.
* Alive scratch register 0x0004_0241: power COLD, boot image PLAY (0 = RECOVERY makes
  `/etc/init.d/rcS` start recovery mode), board ID, boot source NAND
  (kernel `mach-lf1000/include/mach/gpio.h`).

The kernel's `map_io` reads the NAND controller (to tell shadow mode) through the
boot-time I/O section mapping that `paging_init` has just cleared; on the ARM926 this
works because the TLB still holds the 1 MB section from the early UART output. Ribbit's
TLB therefore keeps section entries whole (`ribbit-arm` cp15.rs).

## Bulk: UBIFS built by Ribbit

`ubifs.rs` writes UBIFS as mkfs.ubifs does (layout of the kernel's
`create_default_filesystem` and `ubifs_create_dflt_lpt`, settings of LeapFrog's own
erootfs.ubi: min I/O 2048, fanout 8, 5 log LEBs, format 4), data uncompressed (the
kernel's zlib takes only a 2 KiB window). VERIFIED: ubi_reader extracts it with every
file identical to the packages, and the console's kernel mounts it and AppManager
starts the first-time setup (Language Selection).

## Touch screen and buttons

`src/touch.rs`. A 4-wire resistive screen: X1, Y1, X2, Y2 on GPIO port A pins 11-14
and ADC channels 4-7 (gpio_map.h EMERALD_TOUCHSCREEN_*, lf1000_ts2.c `first_adc`).
Ribbit solves the resistor network (sheets, contact, pull-ups; values GUESSED: 400 Ω
sheets, 300 Ω contact, 100 kΩ pull-up) for every GPIO pad read and ADC conversion, so
the kernel's boot check (gpio_main.c check_for_touchscreen: X2 follows X1, Y2 follows
Y1), touch/no-touch (~10, driver wants <= 19; untouched 1023), position and pressure
all come from one model. The touch point is placed by inverting the calibration in
use: lf1000_ts2.ko's built-in one (a1 a2 a0 a4 a5 a3 a6 = -29676 -170 25786444 -632
-26927 21441222 65536), which `/usr/bin/setcal` keeps when neither /flags/pointercal
nor MfgData has one. The first-time Screen Calibration then writes a pointercal close
to it.

With a touch screen found, the first-time setup starts with Screen Calibration (before
Language Selection). `/etc/init.d/touchscreen` loads lf1000_ts2 only if
`/sys/devices/platform/lf1000-gpio/touchscreen` says YES.

Buttons: port C, low while pressed (lf1000-keypad.c), pins (gpio_map.h EMERALD_*):
Down 0, Right 1, Volume down 2, A 3, Pause 4, Hint 5, R 6, Volume up 7, Left 8, Up 9,
Brightness 10, B 11, Home 12 (the kernel's key code: KEY_ENTER in the Explorer kernel source in hand, KEY_M in evtest on a real Explorer and on the Didj), L 13. Pin 14 (external power / LFP100
interrupt) reads high, pin 18 (headphones) low. Ribbit keeps the Didj's `button` bits
and maps them (`button::EXPLORER`; VOLUME_UP/DOWN are extra bits).

## NOR and MfgData

`src/nor.rs`. The kernel's map uses jedec_probe (JEDEC IDs looked up in its chip
table), not CFI; 512 KB x8 with 4 KB sectors and unlock addresses 0x5555/0x2AAA is
the table's SST39LF040/SST39VF040 entry, so Ribbit answers as that (kernel: "Found: SST
39LF040"). Programming and erasing finish at once.

MfgData0/1 (layout from libMfgData.so's getters; checksum at 0xFFC over the first
0xFFC bytes, sum = (sum ^ byte) + 1): touch calibration 0x00 (7 ints, pointercal
order), backlight 0x1C, ADC constant/slope 0x20, audio gain 0x28, serial 0x2C (24
chars, 20 hex digits used; digits 4-9 product and 10-11 board ID are put in on
reading), SN valid 0x44, SIRAS 0x48, locale 0x68, touch pressure settings 0x70 (24
ints), soft board ID 0xD0, accelerometer bias 0xD4, version 0xE0, backlight v2 0xE4,
MAC 0xF8, IPv6 0x10C, SKU 0x120. Ribbit's record: the touch calibration above (so
setcal saves it as /flags/pointercal and the first-time Screen Calibration is
skipped, as on a calibrated unit), serial `00000000000000000001` (hostname
Explorer-00001700010200000001), locale en-US, version 1, backlight 0 and ADC 0 0
(kernel defaults stay), pressure settings empty (driver defaults stay). A real
unit's values are its own (UNKNOWN). NOR_Boot is blank (Ribbit boots the kernel
itself). The NOR isn't saved yet (`Nor::dirty`).

## Sound

`/etc/init.d/alsa` reads register 1 over I2C at 0x94 (Cirrus Logic CS43L22, loads
snd-soc-didj-cs43l22) or else 0xCC (LFP100; board_ids.h has it on LeapPad Explorer
boards only). Ribbit models I2C0 (0xC000_E000, IRQ 32, `src/i2c.rs`, Explorer only)
and answers as a CS43L22: a register file with its address pointer, chip ID 0xE3
(revision B1, GUESS). The app applies the codec's volume and mutes to the sound
(`I2c::gain`: PCM 0x1A/0x1B, master 0x20/0x21, speaker 0x24/0x25, mutes 0x0D/0x0F;
silent while powered down); the speaker's mono mix and tone control aren't. The volume
buttons work: two presses of volume up in explorerboot took the speaker volume from
0xF3 (-6.5 dB, booted) to 0xFC (-2 dB). In the app they are Page Up / Page Down (Ribbit's
volume keys are the shoulders L/R, as on the Didj), unless bound to a button.

ALSA (lf1000-pcm.c) plays through circular DMA to PCM out (request 24), the same
pacing as the Didj. Two things the Didj's own audio driver never needed:
* DMA buffer addresses are physical, so 0x8000_0000-based here (`Dma::ram_base`).
* `dma_stop` sets MODE_STOP (bit 20) and waits for RUN to clear, with msleep if it
  doesn't. ALSA calls it from the DMA interrupt when playback ends; without STOP
  handling it slept in an interrupt taken in idle and a timer later woke `swapper`
  (kernel oops in try_to_wake_up, PC 0). STOP now stops the channel at once.

Checked: `EXPLORER_WAV=<file>` of a boot to 3000M holds `/var/sounds/startup.wav`
sample for sample.

## Speed, video, splash

* JIT: SDRAM starts at 0x8000_0000 here, not 0, so the JIT's RAM range is
  `machine::jit_for(ram_base, size)`. Before, its end was the RAM size and no
  Explorer code was ever compiled (about 7x slower).
* Video: the kernel uses SDRAM through two more windows, the MLC's block addresses at
  ram_base + 0x2000_0000 and the video planes at 0xA3F0_0000; both are mapped to SDRAM
  (`Bus::sdram_offset`). Without them the screen was green.
* Boot splash: `explorer::show_splash` draws FIRST.32.rle on MLC layer 0, with the
  layer set up as Emerald Boot would (GUESS: its exact register values are UNKNOWN).
* Health and safety screen: a YouTube video (hFn62e6TbRU, other firmware) shows one;
  firmware 1.4.11.2128's rcS shows LEGAL.png instead and nothing shows
  HEALTH_AND_SAFETY.png, so its absence is right for this firmware.

## Cartridges

The slot is the Didj's (`src/cart.rs`): a small-page NAND (512-byte pages, 16 KiB
blocks) on the NAND controller's second bank, detect on GPIO port A pin 18 (low
with a cartridge in; gpio_map.h EMERALD_CARTRIDGE_DETECT, as the Didj's), the
cartridge type on port B pins 2-5 (`/LF/Base/EmeraldMfgTest/SetupNANDCartridge.sh`
reads them the same way). VERIFIED in Ribbit with Didj cartridge dumps:

* The kernel's NAND driver logs "cartridge inserted", "type: OTP", "ECC mode: none",
  finds the Toshiba 64 MiB part, makes the "Cartridge" MTD (64 MB, 16 KiB blocks, as a
  real Explorer's /proc/mtd shows), sees its UBI layer and attaches it as ubi2.
* `lfcart.ko` (detect pin interrupt) and CartManager mount `ubi_Cartridge` (FAT) at
  /LF/Cart (`/etc/init.d/cartridge`), both with the cartridge in at power-on and put in
  or taken out at the menu: the menu's first square shows the game's icon, and goes
  back to the empty-slot picture when it is taken out.
* The Explorer plays Didj cartridges: its firmware has DidjTitles.json (17 titles)
  and DidjPatches (App.so replacements for Indiana Jones and Sonic). Sonic, picked in
  the menu, loads /LF/Cart/snc/App.so, shows its title screen and plays Green Hill
  Zone (its 3D engine pictures; the engine reads its command buffer at
  0x8320_0000, so `ga3d::Mem` now takes the SDRAM base, and block addresses reach all
  64 MB). Indiana Jones reaches its own menu the same way.

The Explorer's own cartridges (2026-11-13, from 86 dumps: 64 MiB, or 128 MiB for 19 of
them): the dump is a FAT32 volume labelled "Cartridge" from byte 0, no UBI and no spare
area. Its meta.inf (one folder, as a Didj game's) says Device="LeapsterExplorer" (70) or
"LeapPadExplorer" (16, LeapPad games), PackageID LST3-.... The kernel tells an "Emerald
Cart ID" (port B pin 4 low) from the Didj's and expects no UBI layer on one ("cart has UBI
layer, but is using Emerald Cart ID"); `/etc/init.d/cartridge` then mounts the MTD
itself as FAT. Ribbit (`cart::is_explorer_image`, `from_explorer_image`) puts the pages
in as they are with erased spare areas, gives the chip the ID of its size (0x98 0x76 or
0x79) and the type 3 (`cart::EMERALD`: the Didj's 7 with the Emerald pin low): the
kernel logs "cartridge has no UBI" and "cart driver ready" with no error. GUESS: the
real type bits (only pin 4 is known; SetupNANDCartridge.sh turns on internal ECC for
6, 11 and 15), the real chips' IDs and page size.

VERIFIED in Ribbit, all 86 (a set-up Explorer powered on with the cartridge in, signed
in, the cartridge's square tapped, the screen 11 s later): 61 reach their title,
menu or game; 15 LeapPad games show their "Please play this cartridge on a LeapPad"
screen (Mini Game Greatest Hits, also a LeapPad game, plays); 8 later ones (Finding Dory,
Frozen x2, PJ Masks, Paw Patrol, Trolls, The Little Mermaid, Rusty Rivets) show "This
cartridge is not supported on this platform"; the 2 French ones ask for their language
pack (only the English set was there). Those screens are the games' own. Test:
`explorer_cartridges_are_explorer_games` (RIBBIT_EXPLORER_CARTS=<folder>).

In the app: every Didj cartridge in the library is listed on the Explorer's page too
(tag "Didj cartridge"), and every Explorer cartridge on that page alone (tag
"Cartridge"). Picking one turns the Explorer on with it in the slot, or puts it in the
Explorer that is running; Emulation > Take the Cartridge Out takes it out.

## Saves and carrying them over

The app keeps the NAND's changed erase blocks in `explorer-<folder>.nandblocks`, the
image's fingerprint in `.nandimage` and the NOR flash whole in `.nor` (the NOR doesn't
depend on the image and is always loaded). A save made on another image (other
packages, Ribbit building Bulk differently, the 1 GB NAND Ribbit had before 2026-11-08)
would mix two UBIFS layouts, so the player's files are carried over instead
(`carry::explorer_user_files` / `explorer_files_beyond`) and the old save is kept
beside it (`.nandblocks.other-image`, as on the Didj):

* `ubifs_read.rs` reads UBIFS back as the kernel left it: the committed index from the
  master node, then the journal (log reference nodes, buds) replayed in sequence order;
  data LZO1X, deflate or plain. With only a save's blocks, index nodes and leaves in LEBs
  the save doesn't hold are skipped and a file is taken only when its inode, its name up to
  the root and all its data are known.
* 1 GB save: that image is built again (`nand_from_packages_blocks(.., NAND_BLOCKS_1GB)`),
  so every Bulk file that differs from it is carried (a changed file too) onto the 512 MB NAND.
* Any other image: the Bulk files read whole from the save's blocks, less those at paths the
  new image has (the packages' files stay as built), as the Didj does.
* VERIFIED on a real 1 GB save from the app (first-time setup done): both ways find the same
  files, `settings.cfg`, `Data/Uploads/0-2/profile.log`, `Data/Local/...` folders; test
  `explorer_files_carried_over`, `ubifs_read` tests, ignored `explorer_real_save`
  (`RIBBIT_EXPLORER_DIR`, `RIBBIT_EXPLORER_SAVE`).
* VERIFIED in the app (2026-11-12): the 1 GB save moved to 512 MB (14 files carried, the
  old one kept aside); that save had never finished the first-time setup (no profile), so
  Language Selection came up again. Setup done (profile ANN, a pet), closed (powered off in
  0.7 s), reopened: Sign In lists ANN. On that 512 MB save the blocks-only reading finds the
  same 34 files as the whole one (`profile.dsc`, `Pets/*.emrs`, `sharedobjects`, ...).
* Not carried: the root file system's changes (the RFS partition comes raw from
  erootfs.ubi): `/flags/volume`, `/flags/lasttime`, `/flags/pointercal` (rebuilt from the
  NOR), FR logs. So the volume level and clock go back to their defaults after a carry-over.

## USB

The Explorer's kernel loads the same lf1000_udc as the Didj, so Ribbit's UDC model serves
it unchanged. Retail boot (`/etc/init.d/rcS`, `dftp2`): `fuse-flasher` mounts /LF/fuse
and `g_file_storage file=/LF/fuse/dftp enabled=1` offers that file as a disk: LeapFrog
Connect speaks DFTP through it. With `/flags/dftp1` or `/flags/developer` it loads
`g_ether` (USB Ethernet, DFTP over TCP, telnet/ftp for developers) instead; Ribbit has
neither flag.

VERIFIED with `EXPLORER=1 didjusb <folder> 4500` (no real USB): device 0x0F63:0x0020,
release 3.25, USB 2.0 high speed, one interface 8/6/0x50 (mass storage, SCSI,
Bulk-Only) with bulk endpoints 0x81/0x02 of 512 bytes; INQUIRY "LeapFrog
LeapsterExplorer 0325"; ready at once (no unlock, unlike the Didj); READ CAPACITY
102400 blocks of 512 bytes (50 MB); block 0 reads as zeros. Also VERIFIED in the app
(2026-11-12) with a small USB/IP client (import, descriptors, SET_CONFIGURATION, INQUIRY):
the Explorer then shows its "connected to a computer" picture; closing the app while
attached closes the client's connection.

In the app: Emulation > Leapster Explorer > Plug into this PC (USB) (setting
`explorer_usb`); `usbip attach -r 127.0.0.1 -b 1-1`. As on the Didj, an import waits
until the driver is loaded (`Udc::ready`), and a restart or the console turning itself
on again disconnects the client first. UNTESTED: LeapFrog Connect itself (the DFTP
exchange) and Windows' view of the 50 MB disk (it has no file system: Windows may offer
to format it, which must be declined).

## Library, installing games, real Explorers (Ribbit, `connect::lx`, `explorer_library`, 2026-11-12)

The Explorer tab lists the console, with a drawn icon, then its games:
- **Built-in**: Application `.lf2` packages beside Firmware Base (Pet Pad, Movie Player, ...).
- **Installed**: games read from the save's Bulk `ProgramFiles`.
- **Didj cartridge**: Didj cartridges, which play in the Explorer's slot.
- **Cartridge**: the Explorer's own cartridges (and LeapPad ones), which play in its slot.
- **Not installed**: Explorer `.lf2` / `.lfp` game packages elsewhere in the games folders.

A click turns the Explorer on. Ribbit doesn't pick the game in the Explorer's menu yet. A package that isn't installed is installed first.

- **Install** (VERIFIED from the console's own `/usr/bin/lfpkg`): an Application package's files go to `/LF/Bulk/ProgramFiles/<folder>`. lfpkg keeps no registry. It unpacks the package and runs a `postinstall` script if there is one. Ribbit doesn't run postinstall scripts (UNKNOWN what a game's would do). The menu reads the folders' meta.inf. "remove" is unsupported in lfpkg ("must be manually removed"), so Ribbit's uninstall deletes the folder. With "delete its saves" it also deletes the game's progress: the folders in its meta.inf `PersistentData`, else `Data/<0-2|GameWide>/<folder>` (GUESS).
- **Model**: the player's files are the Bulk tree that differs from the image built from the packages (`carry::explorer_files_beyond`, or `explorer_user_files` for a save made on another image). Ribbit builds the image again with them (`explorer::nand_with_files`) and keeps the changed blocks as the save. That is the same path the carry-over uses, so it doesn't depend on the mode. An Enhanced (new) HLE that reads its /LF/Bulk from this tree, as the Didj's HLE does with `files_beyond`, sees the same games and saves. When the HLE writes a file, it should write back the same way (`nand_with_files`, as Didj `hle_to_nand` does).
- **Tested**: no Explorer game package is on this PC. The ignored test `didj::library_tests::explorer_install_list_uninstall` makes a stand-in package at test time in the real layout (`RibbitTest/meta.inf`, icon, App.so; none is kept in the repo), or takes RIBBIT_EXPLORER_LF2. It installs, lists, is on the NAND `build_explorer` starts with, exports, comes back after the save is deleted and imported, and uninstalls. UNTESTED with a real game, and the game was not started (its App.so is a stub).
- **Real Explorers**: over USB a real Explorer offers only a 50 MB file for LeapFrog Connect's DFTP protocol, not its file system (see USB). So no safe way is known to copy files onto one. "Export the Explorer's files" writes `LF/Bulk/{Data,ProgramFiles,Downloads}` as a backup in the real layout (UNTESTED ON HARDWARE). "Import from a real Explorer" takes a copy of `/LF/Bulk` (e.g. made over the developer image's telnet/ftp) or a folder holding a 512 MB NAND dump as `explorer.nand`. It adds Data, ProgramFiles and Downloads and leaves `settings.cfg` and the language packs out. UNTESTED ON HARDWARE.

## Not done yet

* The LFP100 power chip, the Explorer's real cartridge type bits and chip IDs (guessed), LeapFrog Connect over USB (untested), carrying the root file system's /flags over. The app's start is untested by a player.
  D-pad tested (moves the highlight); A/B and the rest not tried yet.

Sources: LF1000 kernel 2.6.37 port (github.com/mac2612/lf1000_kernel; LeapFrog's
2.6.31 tree is LF-Linux-6905/8291), eLinux.org Leapster Explorer pages, the
firmware's own root file system.
