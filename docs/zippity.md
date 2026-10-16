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
3. **Kernel** (loaded to low RAM, vectors at 0, entry near 0x12F00). It runs
   an RTOS. Its log:

   ```
   Free mem start initial: 2e33ec
   SPRITE_CACHE_SIZE: 11358608
   v_ffs_fmount worked
   Logging partition has 1927168 free bytes.
   START L/S
   LD open OK: UI.SAV
   LD read OK: UI.SAV
   END L/S
   callee_param: 229
   ```

   It then waits for video frames and DMA and graphics-engine completions, and
   checks the power button: if it is still held, it prints
   "R_IOC_I_DATA = 0x0000 / PowerButton Still held down. Entering interactive
   test mode...". With the hardware below modelled it reaches its main loop
   (hundreds of graphics-engine operations; nothing is drawn yet).

The power button must be held when power comes on: the bootloader reads it
(port C bit 10, low = pressed) and otherwise prints "Debouncing..." and powers
off. The power-off routine (0x00F11824 in the bootloader) shuts the devices
down and jumps to the on-chip RAM at 0xF8000000, which ends in a loop.

## Memory map (as far as known)

| Address | What |
|---|---|
| 0x00000000 | SDRAM (64 MB assumed, as in MAME's stub) |
| 0x20000000 | SRAM (stage 1; size unknown, 2 MB assumed) |
| 0xC0000000 | GPIO: port A +0x00 (data in) / +0x04 (out) / +0x08 / +0x0C, port B +0x20.., port C +0x40.. (the kernel logs port C as R_IOC_I_DATA). Port C bit 10 is the power button (low: pressed); port A bit 6 and port B bit 5 are read active-low, port A bit 4 active-high (meanings UNKNOWN). Also clock or pad registers at 0xA0-0xAC, 0x104/0x108, 0x124, 0x140 |
| 0xC0060000 | Serial port: +0x00 transmit byte, +0x08 control, +0x0C divisor? (0x7530 written), +0x10 status (reads 0: ready) |
| 0xD0100000 | Interrupt controller (below) |
| 0xD0300000 | DMA controller (below) |
| 0xD0400000 | USB controller? (endpoint-like registers at 0xE00-0xEFC, 0x1000..) |
| 0xD0500000 | Video: registers (interrupt enable +0x188, status +0x18C, copy engine +0x1C0-0x1C8, +0x1E0/0x1E8/0x1F0/0x1FC), tables at +0x400 and +0x800, palette? at +0x1000 |
| 0xD0800000 | Graphics engine? (MAME's stub feeds +0x18 random values): +0x00 control (0x901 starts), +0x04/+0x08/+0x0C parameters, +0x7C done (bit 0, write 1 to clear); interrupt line 12 |
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

## Interrupt controller (0xD0100000)

From the kernel's IRQ and FIQ handlers (vectors at 0x18/0x1C jump through
pointers at 0xFFFF74/0xFFFF78 in RAM):

| Offset | Use |
|---|---|
| +0x28 | Read: the number (1-32) of the pending IRQ to serve, 0 if none. The handler calls table[n-1] and reads again until 0 |
| +0x2C | Read: the pending FIQ's number (1-4), 0 if none |
| +0x30 | IRQ mask: bit 32-n masks line n (all set at start, then cleared per line) |
| +0x34 | 0xF written at start (the FIQ mask?) |
| +0x38 | Bit 0: interrupts disabled. Read-write: the kernel's "disable" stores 1 and returns the old bit; "restore" writes it back |

Lines with handlers: 6, 9, 12 (graphics engine), 14, 24 (DMA), 27 (video),
28, 29, 30, 31.

## Video (0xD0500000)

- **+0x188:** interrupt enable (0xC31: bits 0, 4, 5, 10, 11).
- **+0x18C:** interrupt status, write 1s to clear. The handler (line 27)
  serves bit 0 (a frame: it programs the next frame's registers and counts
  frames at +0x6C of its state), bit 11 (counts frames shown at +0x70, which
  the main loop waits on), bit 10, bit 4 and bit 5 (counters). Ribbit's
  research tool raises bits 0 and 11 together once per frame; their real
  timing is UNKNOWN.
- **+0x1C0 / +0x1C4 / +0x1C8:** a copy engine: source, destination, and the
  count of 32-bit words minus 1. Writing +0x1C8 starts the copy; it reads 0
  when done. It is used to load the tables at +0x400 (240 words, one per
  line?) and +0x800 (480 words), and 0x400 words to +0x1000.

## DMA controller (0xD0300000)

Four channels at +0x00, +0x40, +0x80 and +0xC0:

| Offset | Use |
|---|---|
| +0x0 | Control: bit 1 busy (waited on before starting); 0x200 is written first, then 0x109 or 0x189 to start (bit 0 start, bit 7 source fixed: a fill, bit 8 interrupt?) |
| +0x4 | Source |
| +0x8 | Destination |
| +0xC | Count (bytes assumed: a fill's source is an odd address) |
| +0x1FC | Completion flags, one bit per channel, write 1s to clear; interrupt line 24 |

## Picture (in progress)

The frame interrupt writes two addresses each frame: +0x1E0 and +0x1E8
(e.g. 0x00F64580 and 0x00ECE580). They are 0x78000 bytes apart, the size of
a 512x480 16-bit picture, and are swapped from frame to frame (double
buffering, UNVERIFIED). The buffers hold 0x8080 words: as YUV 4:2:2 (Y U Y V
bytes) that is mid grey, which suits a TV console, but the pixel format is
UNVERIFIED. The 0xD0800000 engine's operations so far all take a buffer as
both source (+0x04) and destination (+0x08) with +0x0C = 0x642. The menu's
pictures (the kernel logs SPRITE_CACHE_SIZE, and cartridges carry
mdp_sprites.bin and mdp_backgrounds.bin) are presumably drawn by the video
chip's sprite and background layers (registers at 0xD0500000-0xFF, with
+0x80 = 0x003633EC and +0x48 changing every frame), which are not decoded
yet.
