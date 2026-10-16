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
   and shows its start-up screens (see Video chip).

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
| 0xC00C0000 | Analogue-to-digital converter (below) |
| 0xD0100000 | Interrupt controller (below) |
| 0xD0300000 | DMA controller (below) |
| 0xD0400000 | USB controller? (endpoint-like registers at 0xE00-0xEFC, 0x1000..) |
| 0xD0500000 | Video chip (below): registers, palette RAM at +0x1000, sprite RAM at +0x2000 |
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

## Analogue-to-digital converter (0xC00C0000)

The kernel reads the batteries with it after the start-up screens, and waits
for the conversion before going on to the menu. Interrupt lines 28 and 29
have handlers.

| Offset | Use |
|---|---|
| +0x00 | Control: bit 15 on |
| +0x04 | Conversion: bit 6 starts one; bit 15 done, bits 5 and 4 errors (the kernel then starts again); all three cleared by writing 1 |
| +0x08 | The result, 16 bits |
| +0x0C, +0x14 | Cleared at start |

The kernel warns at a reading of 0x5BCB or less and gives up at 0x5928 or
less. What full batteries read is UNKNOWN; Ribbit's research tool returns
0x7000 at once.

## DMA controller (0xD0300000)

Four channels at +0x00, +0x40, +0x80 and +0xC0:

| Offset | Use |
|---|---|
| +0x0 | Control: bit 1 busy (waited on before starting); 0x200 is written first, then 0x109, 0x189 or 0x1109 to start (bit 0 start, bit 7 source fixed: a fill, bit 8 interrupt?, bits 13-12 the unit: 0 bytes, 1 halfwords) |
| +0x4 | Source |
| +0x8 | Destination |
| +0xC | Count of units. The palette copies use 0x1109 and flush twice the count in bytes from the cache, so bit 12 means halfwords |
| +0x1FC | Completion flags, one bit per channel, write 1s to clear; interrupt line 24 |

## Video chip (0xD0500000)

The video chip is a 32-bit relative of the PPU in GeneralPlus's 16-bit
GPL162xx chips (as MAME models them in `generalplus_gpl162xx_soc_video.cpp`):
register n of the 16-bit chip's block at 0x7000 sits at 0xD0500000 + 4n. The
interrupt bits, the video DMA, the PPU-enable register and the TV registers
all line up. What the system menu uses has been decoded from its data and
checked against a recording of a real Zippity starting up (the warning, the
LeapFrog and Disney logos, the Zippity logo, the credits, the dance-pad
picture and the "Sign In" menu with the unit's saved players, the top band's
colour changing per screen). Ribbit's research tool draws these screens
(`render` in `crates/ribbit-arm/examples/zipboot.rs`). All of it is
UNVERIFIED on hardware.

| Offset | 16-bit chip | Use |
|---|---|---|
| +0x00-0x3C | 0x7000-0x700F | Text layers 3 and 4 (unused by the menu) |
| +0x40-0x54 | 0x7010-0x7015 | Text layer 1: x scroll, y scroll, attributes, control, tile map, attribute map |
| +0x58-0x6C | 0x7016-0x701B | Text layer 2 (unused by the menu) |
| +0x80/+0x84 | 0x7020/0x7021 | Text layer 1/2 picture base (0x003633EC) |
| +0x88 | 0x7022 | Sprite picture base (0: sprites hold whole addresses) |
| +0xA8 | 0x702A | Blend level |
| +0xC0 | 0x7030 | Fade (written each frame, 0 so far; not modelled) |
| +0xE8 | 0x703A | Palette control (0x10 written) |
| +0xF0 | 0x703C | TV control |
| +0x108 | 0x7042 | Sprite control (bit 0 on; 0x00031935 written) |
| +0x188/+0x18C | 0x7062/0x7063 | Interrupt enable and status (below) |
| +0x1C0-0x1C8 | 0x7070-0x7072 | Video DMA: source, destination, 32-bit words minus 1 |
| +0x1E0/+0x1E8 | 0x7078/0x707A | Frame buffers: the one being drawn and the one shown (below) |
| +0x1FC | 0x707F | PPU enable (0x0038179F) |
| +0x200-0x220 | 0x7080-0x7088 | TV saturation, hue, brightness and so on (written once) |
| +0x300-0x330 | 0x70C0-0x70CC | Sprite corner converter (below) |
| +0x400 / +0x800 | 0x7100 / 0x7200 | Per-line tables (240 and 480 words; all 0 in the menu) |
| +0x1000 | 0x7300 | Palette RAM: 16-bit colours, two per word |
| +0x2000 | 0x7400 | Sprite RAM: 512 entries of 32 bytes |
| +0x7000 | - | 0xC00 bytes, cleared at start (sprite extras?) |

- **Interrupts (+0x188 enable, +0x18C status, write 1s to clear):** the
  kernel enables 0xC31. In the 16-bit chip's terms, bit 0 is vertical blank,
  bits 4/5 text and sprite under-run, bit 10 frame-buffer under-run and bit 11
  TV frame end. The handler (line 27) programs the next frame on bit 0 and
  counts frames shown on bit 11, which the main loop waits on. The research
  tool raises bits 0 and 11 together once per frame; their real timing is
  UNKNOWN.
- **Video DMA (+0x1C0/+0x1C4/+0x1C8):** each frame the kernel copies its
  palette (0x400 words from 0x001BE540) and sprite list (0x31F words from
  0x002D314C) into the chip. Writing the count starts it; it reads 0 when done.
- **Frame buffers:** the chip renders into memory, PPU-enable bit 7 being
  frame mode in the 16-bit chip. The kernel rotates three 0x96000-byte buffers
  (640x480 at 16 bits): +0x1E0 is drawn into, +0x1E8 shown, and the 0xD0800000
  engine is given the drawn one (source and destination, +0x0C = 0x642;
  perhaps a scaler or TV encoder). The buffers start filled with 0x8080. Their
  pixel format (PPU-enable bit 8: the 16-bit chip's "RGBG") is UNKNOWN; the
  research tool draws from the layers directly.
- **Palette:** 2048 16-bit entries (0RRRRRGGGGGBBBBB; bit 15 set:
  transparent, as in 0xFC1F, magenta, which starts each sub-palette). The
  menu's sprites take colours from 0x200 on, text layer 1 from 0x300. The
  kernel loads per-screen palettes by DMA: 0x80 entries to 0x200 (the top
  band, recoloured for each screen), 0x10 to 0x2F0 (text) and 0x100 to 0x300
  (the background).

### Text layer 1

The menu's background is a 640x480 picture at 8 bits per pixel.

- **Attributes (+0x48 = 0x00C08073):** bits 1-0 depth (3: 8 bits), 5-4 tile
  width and 7-6 tile height (8 << n: 64x16), bits 23-22 the palette bank (3:
  colours 0x300-0x3FF). The bank bits are inferred from this one value.
- **Control (+0x4C = 0x2A):** bit 3 on. Bit 1 (attributes from the register,
  not a map) and bit 5 are as in the 16-bit chip.
- **Tile map (+0x50 = 0xF8002F80, on-chip RAM):** 32-bit entries, 16 per row
  (pages of 1024x512), each the tile's byte offset from the base at +0x80
  (0x400, 0x800, ...: 30 rows of 10 tiles), 0 for no tile.

The kernel decompresses the picture with its own code: 16-bit commands for
runs (filled by the DMA controller with a fixed source) and back-references
(DMA copies, overlapping).

### Sprites

Sprite RAM entries are 32 bytes (the kernel keeps a copy of the list at
0x002D314C and sends it each frame):

| Bytes | Use |
|---|---|
| 0-1 | Picture address bits 15-0, in halfwords |
| 2-3 | x (signed, from the screen's centre, right) |
| 4-5 | y (signed, from the screen's centre, up): the sprite's top-left corner is at (320 + x, 240 - y) |
| 6-7 | Attributes: bits 1-0 depth (2/4/6/8 bits), 2 x flip, 3 y flip, 5-4 width and 7-6 height (8 << n), 11-8 palette, 13-12 priority, 15 palette bank (below) |
| 8 | Bits 6-0: picture address bits 22-16 |
| 9 | Bits 5-0 and 7-6 set by the kernel from its arguments (0x3F seen) |
| 10-15 | Words written from the corner converter |
| 19 | Bit 7 set |

- **Pixels** are one bit stream, most significant bit first: the 6-bit Zippity
  logo only comes out right this way.
- **Colours** are palette entry bank + palette x 16 (rounded down to the
  depth's size) + the pixel, where the bank is 0x200 if attribute bit 15 is
  set and 0x100 if not (the menu's blue buttons use 0x100).
- **16-bit pixels:** a sprite of depth 8 with palette bit 0 set (0x99F3) has
  16 bits per pixel: bits 7-0 a colour (entry bank + n), bits 13-8 its
  opacity (0 to 63) and bit 14 always set. The menu's background art (a
  64-pixel grid of these sprites) is blended over text layer 1 this way, and
  recolouring entries 0x200-0x27F gives each screen its colour. Which
  attribute bit really selects this mode is UNVERIFIED.
- **Unknown format:** on the dance-pad and "Sign In" screens a panel showing
  the joystick and pad with "wireless" arcs (attributes 0x33F3, 0x2073 and
  others; 16 bits per pixel by its size, but with bit 14 not set) does not
  come out right either as 8-bit or as colour-and-opacity. It is not in the
  recording, and is probably a "connect the controllers" prompt shown because
  the research tool has no controllers.
- **Order:** priority 0 to 3, and within a priority later sprites on top.

**Corner converter (+0x300-0x330):** for each sprite the kernel writes four
corners, x then y (+0x300/+0x304 to +0x318/+0x31C, the same centred
coordinates; a 64x64 sprite gives (-275, 61), (-212, 61), (-212, -2),
(-275, -2)). It then reads five halfwords (+0x320 to +0x330) into the
sprite's halfwords 1, 2, 5, 6 and 7. This looks like the 16-bit chip's
"virtual 3D" sprites (PPU-enable bit 9, set). What the hardware returns is
UNKNOWN. The research tool returns the first corner as x and y and 0 for the
rest, which places flat sprites as recordings show.
