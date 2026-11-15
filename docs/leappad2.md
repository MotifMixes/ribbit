# LeapPad 2 and LeapPad 3

LeapFrog's LeapPad 2 ("LeapPad2 Explorer", 2012, codename Valencia) and LeapPad 3 (2014, codename
Cabo) are Linux tablets of the Leapster Explorer's software family (Brio, AppManager, Flash UI). Ribbit
runs their programs the way it runs the Didj's and the Explorer's in Enhanced (new) (docs/didj-enhanced-hle.md,
docs/explorer-enhanced-hle.md): the firmware's own ARM programs on Ribbit's ARM core, Ribbit answering
their Linux system calls instead of a kernel. There is no Original (real kernel) mode for these: the SoCs
(Nexell NXP3200 / NXP4330) aren't emulated.

Status (2026-11-15): research done; the ARM core runs ARMv7 programs (`Arm::v7`); the HLE has a LeapPad 2
mode (`hle::leappad2`, `Process::new_leappad2`). See "What works" below. Not in the app yet.

## Hardware

| | LeapPad 2 | LeapPad 3 |
|---|---|---|
| Codename (kernel's /sys/devices/system/board/platform) | VALENCIA (family LPAD) | CABO (family LPAD) |
| SoC | LF2000 = Nexell NXP3200 (kernel `arch/arm/mach-nxp3200`) | LF3000 = Nexell NXP4330 (string `nxp4330q` in its kernel) |
| CPU | ARMv7-A ("ARMv7 Processor" in the kernel; userland built for v7 + VFP); Cortex-A9 (GUESS from NXP3200 being a Cortex-A9 part; the kernel's proc-v7 is generic) | ARMv7-A, SMP kernel (several cores; NXP4330 is a quad Cortex-A9, GUESS from Nexell's naming) |
| Kernel | Linux 2.6.37.4, PREEMPT (Firmware-Base 4.6.0.784's kernel.bin, built 2013-08-14) | Linux 3.4.24, SMP PREEMPT (6.2.0.654's uImage, built 2016-03-22) |
| Screen | 480x272, ILI6480G2 module (board.c `get_lcd_size`/`get_lcd_type`: MADRID and VALENCIA) | 480x272 (rcS draws `cabo_boot_battery_low_480_272.png`) |
| RAM | 128 MB (GUESS from the `bid` board config: kernel 62 MB at 0x8020_0000, linear 32 MB at 0x8400_0000, video 24 MB at 0x8600_0000, ending at 0x8780_0000) | UNKNOWN |
| Storage | NAND, UBI with 1 MiB erase blocks and 4 KiB pages ("C4G-E1M-W4K-erootfs.ubi": 4 Gbit... GUESS from the name) | eMMC: a FAT partition with the uImage, an ext4 root file system (`firmware/emmc/1fat/2/FAT32`, `2ext4/3/RFS`) |
| Touch | resistive touch screen, `lf2000_ts` driver ("LF2000 touchscreen") | as LeapPad 2 (rcS calibrates it the same way) |
| Buttons | d-pad, A/B (`read_button a/b`), volume up/down, home, power | as LeapPad 2 |
| Accelerometer | Bosch BMA220/BMA222 ("LF2000 Accelerometer", `lf2000_aclmtr.c`) | yes (`lf2000-aclmtr` in rcS) |
| Cameras | rear Hynix HI253 on I2C 1 (Valencia); front HI253 on the EP/FEP revisions (board.c) | yes |
| Sound | LFP100 power/audio chip (`CONFIG_SND_SOC_LFP100`), Nexell I2S | UNKNOWN |
| Board IDs | system_rev 0x206 VALENCIA, 0x207 VALENCIA_EP, 0x208 VALENCIA_FEP; LeapsterGS is LUCY 0x205/0x209 (board_revisions.h) | |

Sources: LeapFrog's GPL release LF2-Linux-1575 (2013-10, archive.org `lf-2-linux-1575-201310.tar.gz`), its
kernel `linux-2.6.37+svn-r1575`: arch/arm/mach-nxp3200/board.c, board_revisions.h, plat-nxp3200/*/cfg_mem.h,
configs/nxp3200_bid_defconfig; the firmware images below (kernel version strings, ELF attributes, rcS).

The Leapster GS ("LeapsterGS Explorer", LUCY) is the same LF2000 with a 320x240 screen; the LeapPad
Ultra (GLASGOW), Platinum and Ultimate (XANADU) and LeapPad Jr. are later LF3000 models (names from the
LeapPad 3 kernel and the firmware packages' Device fields).

## Software

* The same family as the Explorer's: uClibc 0.9.32.1 (LeapPad 2) / 0.9.33.1 (LeapPad 3), BusyBox,
  /sbin/init running /etc/init.d/rcS, lfd, AppManager, Brio's MPI libraries, Lightning, a Flash player
  (LeapFrogPlugin), CartManager. The LeapPad 2's root file system also has /LF/Base/EmeraldTitles and
  DidjCompat-style files: it runs Explorer and Didj cartridges too.
* Built for ARMv7 in ARM state (Tag_CPU_arch v7, VFPv2 on the LeapPad 2; VFPv3 and NEON tags on the
  LeapPad 3's uClibc). A scan of all 135 ELF files of the LeapPad 2's root file system with Capstone:
  no Thumb code except 63 functions of libgcc_s; ARMv7 instructions used: MOVW/MOVT (31 k), VFP loads,
  moves, conversions and arithmetic (about 45 k), the extends (UXTH/UXTB/SXTH/SXTB, SXTAH), DMB, LDREX/STREX,
  UBFX/BFI/BFC, REV, MLS. The LeapPad 3's uClibc, Lightning and AppManager use the same set (no NEON found
  in them).
* The kernel tells the consoles apart in /sys/devices/system/board: `platform` (DIDJ, EMERALD, MADRID,
  LUCY, VALENCIA; CABO and others on the LF3000), `platform_family` (DIDJ, LEX, LPAD), `lcd_size`
  (320x240 / 480x272), `lcd_type`, `system_rev`. rcS picks the boot logo and videos by them; libDisplay
  reads lcd_size; the frame buffer driver keeps the LF1000's ioctls (`CONFIG_FB_NEXELL_LF1000_EXTENSION`).
* How a LeapPad cartridge decides it's on a LeapPad: the cartridge's top-level App.so (a small launcher,
  "UEBLApp") asks Brio's `CDisplayMPI::GetScreenStats` for the screen and compares it: 480 wide or more
  and 272 high or more goes on, anything smaller (the Explorer's 320x240) shows the cartridge's
  `assets/LEX_CartridgeCompatibilityAlert.png` and says "This cartridge only works on LeapPad"
  (disassembly of Thomas & Friends' App.so at 0x8CC4-0x8CD8: `cmp r3, #0x1e0` on the width, `#0x110` on
  the height). On a LeapPad it then checks the firmware version (at least 2.1.3, else
  `errorLPAD.png`), finds or installs the MicrophoneWidget (LPAD-0x001E0003) and starts the game named
  in AppInfo.json (`LPAD-0x001F000A-000000-1.0.6.25619/BookApp.so`).
* Packages: as the Explorer's. Firmware-Base is a zip ("DiskImage": `4,…,kernel.bin` (a uImage),
  `5,…,erootfs.ubi`); `.lf2` packages are bzip2 tars installed to /LF/Bulk by lfpkg. LeapPad 3 firmware
  is a zip of eMMC partition images (FAT with the uImage, ext4 root).
* Cartridges: the LeapPad 2's /etc/init.d/cartridge is the Explorer's (lf1000-nand/cart_hotswap, UBI or
  plain FAT on the Cartridge MTD, mounted at /LF/Cart).

## Firmware (outside git)

* `D:/ribbit-roms/LeapPad2/PAD2-0x00220004-000000.lfp`: Firmware-Base 4.6.0.784, Device LeapPad2Explorer,
  en-us (archive.org `LF-Firmware-Collection`); `PAD2-0x00290004-000000.lfp`: 4.5.15.1029, fr-fr.
  Unpacked in `fw4.6.0/` (rootfs/ extracted with ubireader, symbolic links lost).
* `D:/ribbit-roms/LeapPad2/lfc/`: a LeapFrog Connect download cache from a LeapPad 2 (archive.org
  `lfc-downloads-full`, 164 files: Firmware-Base and Firmware-BulkEmpty 4.6.0.784, Surgeon, language
  packs, widgets, Pet Pad, trailers, themes, patches). `D:/ribbit-roms/LeapPad2/EnglishLeapPad2/`: the
  37 packages a LeapPad 2 needs, copied from it with readable names (themes, patches and device
  assets left out).
* `D:/ribbit-roms/LeapPad3/PAD3-0x002A0001-000000.lfp`: Firmware-Base 6.2.0.654, Device LeapPad3, en-us
  (LF-Firmware-Collection), unpacked in `fw6.2.0/` (the ext4 root read with 7-Zip).
* Also downloaded (`D:/ribbit-roms/LFC/col/`): LeapsterGS 3.6.2.305 (GAM2), LeapPad Explorer (LeapPad 1)
  2.5.1.3805 fr-fr (LPAD), LeapPad Jr. 7.0.0.67 (PAD4), LeapPad Ultra 5.3.0.1007 (PHR1), Platinum
  8.0.3.2579 (PHR2), Ultimate 9.1.0.252 (PHR3).
* The GPL source: `D:/ribbit-scratch/leappad2/lf2-linux` (the user's copy, cut short about 430 KB before
  the end, but the kernel's tarball inside is whole: `D:/ribbit-scratch/leappad2/kernel/linux-2.6.37`).

## Cartridges in the user's dumps

Of the 86 Explorer-slot dumps, these carry LeapPad games (meta.inf Device "LeapPadExplorer": the
LeapPad 1 and later, all LeapPads run them): Cars 2 - Project Undercover, Cinderella, Disney Fairies,
Disney Planes, Dora's Amazing Show, Leap School - How Not to Clean Your Room, Learn to Read Collection
(Adventure Stories, Fairy Tales, Mysteries), Letters on the Loose, Mini Game Greatest Hits (an
AppBundle), Mr. Pencil - The Lost Colors of Doodleberg, Pet Pals - Dog Show Detectives, Sofia the First,
SpongeBob - Best Day Ever, Thomas & Friends - The Great Penguin Rescue. Four carry a Base_lib with
Device "LeapPad2Explorer" (Cooking! Recipes on the Road, Hello Kitty and Friends, The Magic School Bus -
Dinosaurs, Monsters University) but are Explorer games otherwise. None says LeapPad 3.
