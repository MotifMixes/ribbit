# LeapPad 2 and LeapPad 3

LeapFrog's LeapPad 2 ("LeapPad2 Explorer", 2012, codename Valencia) and LeapPad 3 (2014, codename
Cabo) are Linux tablets of the Leapster Explorer's software family (Brio, AppManager, Flash UI). Ribbit
runs their programs the way it runs the Didj's and the Explorer's in Enhanced (new) (docs/didj-enhanced-hle.md,
docs/explorer-enhanced-hle.md): the firmware's own ARM programs on Ribbit's ARM core, Ribbit answering
their Linux system calls instead of a kernel. There is no Original (real kernel) mode for these: the SoCs
(Nexell NXP3200 / NXP4330) aren't emulated.

Status (2026-11-15): the ARM core runs ARMv7 programs (`Arm::v7`); the HLE has a LeapPad 2 mode
(`hle::leappad2`, `Process::new_leappad2`) that boots the real firmware through the first-time setup to the
home menu, and a LeapPad cartridge passes its "play this on a LeapPad" check there. Games don't draw yet: the
LeapPad 2's Lightning draws with OpenGL ES on the LF2000's Nexell VR5 GPU, not emulated. Not in the app.

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

## What works (Enhanced (new), headless, 2026-11-15)

Run: `EXPLORER_HLE_LEAPPAD2=1 cargo run --release -p ribbit-didj --example explorerhle -- D:/ribbit-roms/LeapPad2/EnglishLeapPad2`
(Git Bash: MSYS_NO_PATHCONV=1) with the explorerhle options (docs/explorer-enhanced-hle.md), plus
EXPLORER_HLE_TILT=x,y,z,orientation (the accelerometer), EXPLORER_HLE_STATE_OUT / EXPLORER_HLE_STATE (save the
run's end, start from it). The file systems take 7-20 s to build (the language packs' bzip2).

* /sbin/init runs rcS: lfd, the volume, VideoDaemon with the startup video and chime (the sound comes out),
  setcal (with /flags/pointercal the setup has no Screen Calibration), mounts, networking (fails: no sockets
  but AF_UNIX), dftp2, then /usr/bin/app starts AppManager, which starts the Flash UI (LPAD/main.swf).
* The first-time setup by touch, as a player does it: Select Language (US), Date and Time, Create Profile
  (name typed on the on-screen keyboard), grade, then the LeapFrog Connect tutorial videos and the home menu
  (profile, Cartridge, My Profile, Check Here). Frames: D:/ribbit-scratch/leappad2/evidence/.
  The setup screens and the startup video are drawn turned 90 degrees: that is their own art
  (LanguageSelection.swf has a 272x480 stage; the logo in StartupVideo.ogg, a 480x272 video, is upright for a
  LeapPad held on its side), not a fault (CHECK on a real LeapPad 2).
* Cartridges: the Explorer dumps of LeapPad games go in the slot (`Process::explorer_cart` ->
  `leappad2_cart`: what /etc/mdev/cartridge.sh does: cart_hotswap, /LF/Cart mounted, cnotify 1 and 3 to
  /tmp/cart_events_socket). The home menu's Cartridge square shows the game's icon (Thomas & Friends).
  Tapped, the cartridge's launcher (App.so, UEBLApp) logs "screen stats (480 x 272)", "Firmware and Device
  OK...", "Microphone widget OK..." and AppManager loads the game, BookApp.so (here LeapFrog's LexPatches.json
  replaces Thomas's with the "LBA7 Patch" BookApp). The game then sets up OpenGL ES (eglInitialize,
  eglCreateWindowSurface, eglCreateContext) and crashes: libvr5 wants SysV semaphores (semget/semctl,
  system calls 299/300, not in the HLE) and the GPU (/dev/ogl_vr5, /dev/vmem).
* Speed: 40-100 million instructions a second while busy (video, Flash) on the i9; the real CPU is about
  530 MHz, so screens change slowly (a setup step takes 5-20 s). The JIT interprets every ARMv7 and VFP
  instruction inside its blocks (only MOVW is compiled).

Fixed on the way (shared with the Explorer's HLE): a socket name whose node was unlinked can be bound again
(VideoDaemon and AppManager both bind /tmp/cart_events_socket after unlinking it); save states keep the VFP
registers (appended after the state, older states still load); the JIT keeps its ARMv7 mode after a load.

## What's missing

* **The GPU.** LeapPad 2 Lightning (libLightning2D: textures, eglSwapBuffers) and so every Lightning game
  draws with OpenGL ES 1.1 through Nexell's libvr5.so: /dev/ogl_vr5 (ioctls), /dev/vmem (video memory),
  /dev/mem, SysV semaphores. The VR5 is undocumented; the kernel's side is in the GPL source
  (arch/arm/mach-nxp3200/vr5). Two ways: emulate the VR5 at its registers/command buffers (reverse
  engineering libvr5's command stream), or HLE the GL API (answer libGLESv1_CM/libEGL's exported functions
  in Rust, as Ribbit answers system calls, with a software or host-GPU renderer). The second looks smaller:
  GLES 1.1 is documented and Lightning uses a small part of it (textured quads).
* SysV IPC (semget/semop/semctl, shm*): needed by libvr5 and by ALSA's dmix (the Explorer's HLE lives
  without dmix already).
* Instructions: everything the scans found is implemented. Not implemented: Thumb-2 (only 63 libgcc_s
  functions are Thumb; none were called), NEON (tagged in the LeapPad 3's uClibc, not found in use).
  The JIT runs MOVT, the extends, the exclusives, the media and VFP instructions through the interpreter one
  at a time; compiling the common ones (MOVT, UXTH/UXTB/SXTH, VFP loads, stores and arithmetic) is the
  speed work. Unaligned LDR/STR are done in the interpreter only (the JIT rotates, as ARMv5).
* The camera (/dev/video*, VIP), microphone, USB, Wi-Fi: absent (the programs carry on).
* GUESSES to check on a real LeapPad 2: the input devices' order, the NAND/MTD table, MfgData's place
  (the Explorer's NOR layout is used; "NOR checksum error" is printed and the defaults taken), RAM size.

## LeapPad 3

Not started in the HLE. Its firmware (6.2.0.654) is the same family on Linux 3.4.24 (SMP), uClibc
0.9.33.1, with Qt and Wi-Fi; its root file system is ext4 on eMMC (Ribbit has no ext4 reader yet; 7-Zip
reads it), its kernel a uImage in a FAT partition. Its uClibc, Lightning and AppManager use the same ARMv7
instructions as the LeapPad 2's (no NEON found in them). Its platform is CABO, 480x272. The same work
applies (an ext4 reader, the LF3000's /sys and devices, and the GPU, likely a different one).

## Plan for the app

1. The GPU (HLE of GLES 1.1 as above), then a LeapPad cartridge to its title and play, checked frame by frame.
2. Library: a LeapPad 2 family (Device "LeapPad2Explorer" packages: Firmware-Base `PAD2-0x0022...`), its
   cartridges the Explorer dumps whose meta.inf says LeapPadExplorer (and the Explorer games, which it also
   runs), started in Enhanced (new) only (no Original mode: no LF2000 machine).
3. A session as `Hle::new_explorer` with `Process::new_leappad2`: the stylus, the d-pad and buttons (gpio-keys),
   volume, the accelerometer from the keyboard or a gamepad's tilt, saves of Bulk and /flags (the NAND layout
   of a LeapPad 2 is a GUESS: keep its files as a folder or a UBI image of our own), save states.
4. Settings: "Turn off when idle" must hold (off: never idles off; the LeapPad 2's AppManager has the
   Explorer's shutdown timer, "Shutdown Timer set for 12000 [ms]", so the Explorer's idle handling applies).
5. Speed: compile the common ARMv7/VFP instructions in the JIT.

## Open questions

* Is the sideways setup really how a LeapPad 2 shows it? (A real unit or a video.)
* The retail board ID (0x206/0x207/0x208) and what changes with it (front camera).
* Where MfgData lives on the LF2000 (NAND partition or NOR); the real /proc/mtd.
* The VR5's interface (ioctls of /dev/ogl_vr5, the command format) if the GPU is emulated instead of HLE'd.
