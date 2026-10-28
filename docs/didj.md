# LeapFrog Didj

The Didj (2008, codename "Lightning") is a handheld built on the LF1000, LeapFrog's version of the MagicEyes
Pollux VR3520F: an ARM926EJ-S at 393 MHz with 32 MB of SDRAM, a 320x240 LCD, a 3D engine, and 256 MB of
internal NAND. It runs Linux 2.6.20 with LeapFrog's Brio libraries; games come on NAND cartridges.

**Status: not emulated yet.** Milestone 1 is done (formats and the NAND image, `crates/ribbit-didj`).

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

## Boot chain (from the source; not run yet)

1. The LF1000 loads the start of NAND into SDRAM at address 0 and runs it. The README says the hardware loads
   0x800 bytes and the loader reads the rest, but that read is commented out in `bootstrap.c`, and the packaged
   lightning-boot.bin is 7386 bytes. So the hardware probably loads more than 0x800 bytes. How much it loads,
   and how SDRAM is set up first, is UNKNOWN.
2. lightning-boot latches the power on (Alive GPIO VDDPWRON), reads the board and cartridge IDs from GPIO
   pins into a scratch register, and reads the boot flags (JFFS2 in Atomic_Boot_Flags) to pick Kernel0/RFS0
   or Kernel1/RFS1. It shows `bootsplash.rgb`, loads `uImage` with its 64-byte header at 0x8000 - 64, builds
   ATAGs (CORE, MEM, CMDLINE) and jumps to the entry point (0x8000).
3. The kernel mounts the root file system (JFFS2) and the Brio partition. Brio's AppManager then runs the
   menu and games.

The bootloader's UART debug output is compiled in only with `DEBUG=1`. Whether the packaged build has it is
UNKNOWN.

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
| 0x02200000 | Brio               | 222 MiB   | erased; the other packages go here (not yet) |

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
2. **M2:** boot lightning-boot and the kernel to its UART log (ARM926EJ-S from `ribbit-arm`, SDRAM, NAND
   controller, UART, timers, interrupt controller).
3. **M3:** interrupts, timers, GPIO and the NAND controller, enough for the kernel to mount its file systems.
4. **M4:** display (MLC/DPC).
5. **M5:** audio and buttons.
6. **M6:** 3D engine.
7. **M7:** frontend, saves, and an Enhanced/Original mode toggle.
8. **M8:** speed (ARM JIT).
