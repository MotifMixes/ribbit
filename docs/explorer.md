# Leapster Explorer

LeapFrog's Leapster Explorer (2010, codename "Emerald", product line LST3) runs on
the same LF1000 (MagicEyes Pollux, ARM926EJ-S at 393 MHz) as the Didj, so Ribbit
emulates it with the Didj's machine (`crates/ribbit-didj`, `src/explorer.rs`).
Status: boots Linux and LeapFrog's AppManager; touch screen and buttons work in the
first-time setup (Language Selection, Set the Date); the NOR flash with factory data
is there. In the app (early): the library lists Firmware Base.lfp as the console and starts it from the packages beside it; the mouse is the stylus; saves in explorer-<folder>.nandblocks and .nor.

Run: `cargo run --release -p ribbit-didj --example explorerboot -- <folder> [million cycles]`
with the folder holding the English firmware set (archive.org item
`didj-leapsterexplorer-leappadexplorer`, folder EnglishLeapsterExplorer). Options:
`EXPLORER_SHOT=<png>` (the screen at the end), `EXPLORER_RAM=<file>` (SDRAM; the
kernel log is in it), `EXPLORER_BULK=<file>` (the Bulk partition as a UBI image),
`EXPLORER_TRACE=<n>` (interpret to the first exception and print the last n PCs),
`EXPLORER_JIT=0`, `EXPLORER_WAV=<file>` (the sound), `EXPLORER_INPUT=<million>:<what>;...` (`x,y` touches there, `lift`,
a button name, `none`), `EXPLORER_SHOTS=<million>,...` (screens along the way).

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
| NAND | 8 Gbit SLC (board_ids.h), 2 KiB pages, 128 KiB blocks, at 0x2C00_0000; Ribbit: 1 GB, answering READ ID as Samsung K9K8G08 (EC D3 51 95 58), GUESS |
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
Brightness 10, B 11, Home 12 (KEY_ENTER), L 13. Pin 14 (external power / LFP100
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

## Not done yet

* The LFP100 power chip, cartridges, USB, carrying saves over to another image. The app's start is untested by a player.
  D-pad tested (moves the highlight); A/B and the rest not tried yet.

Sources: LF1000 kernel 2.6.37 port (github.com/mac2612/lf1000_kernel; LeapFrog's
2.6.31 tree is LF-Linux-6905/8291), eLinux.org Leapster Explorer pages, the
firmware's own root file system.
