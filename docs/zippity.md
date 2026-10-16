# LeapFrog Zippity

The Zippity (2009) is a TV console unrelated to the Leapster hardware. Its
system-on-chip is a GeneralPlus GPL32300A with an ARM9 core (ARMv5TE, emulated
by `crates/ribbit-arm`). There is no public documentation for the chip, and
MAME only has a stub (`tvgames/generalplus_gpl32612.cpp`). Everything below
was learned by running the dumped firmware on the ARM model with
`crates/ribbit-arm/examples/zipboot.rs`, and is UNVERIFIED on hardware unless
it says otherwise.

## Dumps

- **System NAND:** Micron MT29F2G08AACWP, ID `2C DA 80 15`. 2048 blocks of 64
  pages, each page 0x800 data + 0x40 spare (0x1080_0000 bytes in all). Two
  dumps are known: US (CRC32 16248b63) and UK (27d172ae).
- **Page 0:** a header that starts "PGandnand", then words 0x20000000, 0x200fc,
  0x20100000, 0x20100258, 0x20100270, 1, 0x40001. Stage 1 runs in SRAM at
  about 0x20100000.
- **Cartridges:** FAT16 images (OEM name MSDOS5.0, 0x420_0000 bytes) holding
  KERNEL.BIN plus mdp_sprites.bin, mdp_backgrounds.bin and mdp_wavs.bin.

## Boot

The boot ROM has not been dumped. As MAME's stub does, Ribbit's research tool
replaces it by copying NAND data 0xA0000..0x1F0000 (spare bytes stripped) to
address 0 and starting there.

1. **Bootloader** ("Maestro BIOS", linked at 0). It has an ARM vector table of
   `LDR PC` entries. The reset code switches to SVC mode and writes 0 to
   0xFF000000 (the cache controller). It copies a routine to the on-chip RAM
   at 0xF8000000 and runs it, then relocates itself to about 0x00F00000.
2. **Bootloader log** (serial port, 0xC0060000):

   ```
   Maestro BIOS
   SDRAM interface running at 4 mA.
   Logging partition has 1927168 free bytes.
   Checking for kernel
   Kernel found.  Loading.
   ```

   It checks every block's bad-block marker: page 3 of each block, spare
   column 0x800. Its strings name `a:\kernel.bin` (the system menu) and
   `b:\kernel.bin` (the cartridge), plus mmch/hm/mftp/le_kernel.bin variants,
   and "No kernel found. Listening for USB".
3. **Kernel** (loaded to low RAM, entry near 0x12F00). It runs an RTOS and
   prints "Free mem start initial: 2e33ec" and "SPRITE_CACHE_SIZE: 11358608".
   It sets up the hardware at 0xD0500000 (about 6,700 addresses written; most
   likely video memory). In Ribbit it then prints "SFS_API_EXIT" and stops:
   its first task returns at once, the filesystem layer's user count drops to
   0, and with bit 0 of 0xD0100038 clear it halts (loop at 0x41630). Why the
   task returns is the next thing to find out.

## Memory map (as far as known)

| Address | What |
|---|---|
| 0x00000000 | SDRAM (64 MB assumed, as in MAME's stub) |
| 0x20000000 | SRAM (stage 1; size unknown, 2 MB assumed) |
| 0xC0000000 | System control (clocks?): the bootloader reads and writes 0x00-0x4C, 0xA0-0xAC, 0x104/0x108 (read-modify-write), 0x124, 0x140 |
| 0xC0060000 | Serial port: +0x00 transmit byte, +0x08 control, +0x0C divisor? (0x7530 written), +0x10 status (reads 0: ready) |
| 0xD0100030-38 | +0x38 bit 0: set at reset, cleared by the bootloader; the kernel halts on "SFS_API_EXIT" when it is clear (power hold? UNKNOWN) |
| 0xD0500000 | Written heavily by the kernel (video memory?) |
| 0xD0900000 | NAND controller (below) |
| 0xF8000000 | On-chip RAM (the bootloader runs code there; 64 KB assumed) |
| 0xFF000000 | Cache controller: +0x00 control (bit 2: enabled), +0x18 maintenance operation (0x80000001/5/9/D), +0x1C/+0x20 range (address \| 0x80000002) |

## NAND controller (0xD0900000)

Modelled from its driver in the bootloader:

| Offset | Use |
|---|---|
| +0x000 | DMA control: bit 0 start (reads 1 while busy), bit 1 direction (1: memory to NAND) |
| +0x004 | DMA memory address |
| +0x008 | DMA length in bytes |
| +0x140 | Status: bit 15 ready. Drivers wait for it after each command (if it is set, they poll once more, then wait until it is set) |
| +0x144 | Command: the NAND command byte (FF reset, 90 read ID, 00/30 read, 80/10 program, 60/D0 erase, 70 status) |
| +0x148 | Column; for an erase, row bits 15-0 |
| +0x14C | Row (page number); for an erase, row bits 31-16 |
| +0x150 | Data port: one byte per access (16-bit for 16-bit chips) |
| +0x154 | Saved and restored around Read ID (chip select? UNKNOWN) |
| +0x158 | ECC control: bits 15-8 are the sector count, plus mode bits (0x21/0x31 \| 2) |
| +0x15C | ECC sector size (0x400 written) |
| +0x160 | ECC error flags, one bit per sector (reads 0: no errors) |
| +0x164 | ECC error count |

The driver identifies the chip by its ID: maker 0x2C, then a device-code
table, where 0xDA means 2 Gbit with 2 KB pages. A read is: command 0x00, the
column to +0x148, the row to +0x14C, command 0x30, wait for ready, then DMA
or data-port reads. Large-page chips use the 0x30 confirm; small-page chips
start reading on the address.
