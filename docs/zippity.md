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
| 0xC0000000 | GPIO: port A +0x00 (data in) / +0x04 (out) / +0x08 / +0x0C, port B +0x20.., port C +0x40.. (the kernel logs port C as R_IOC_I_DATA). Port C bit 10 is the power button (low: pressed); port A bit 6 is a TV-standard strap (low: 50 Hz; its callers choose 50 or 60 and 5000 or 5994); port B bit 5 is read active-low and port A bit 4 active-high (meanings UNKNOWN). Port B bit 0, port C bits 5/4 and bits 6/5 of 0xC00000C4 drive five LEDs (the factory test mode's "Leds:"). Also clock or pad registers at 0xA0-0xAC, 0x104/0x108, 0x124, 0x140 |
| 0xC0020000 | Timers (below) |
| 0xC0060000 | Serial port (below): the debug log goes out, the controllers' radio receiver comes in |
| 0xC00C0000 | Analogue-to-digital converter (below) |
| 0xD0100000 | Interrupt controller (below) |
| 0xD0300000 | DMA controller (below) |
| 0xD0400000 | Sound unit (below) |
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

Lines with handlers (registered through the kernel's function at 0x31564):

| Line | Source |
|---|---|
| 2 | 0xC0030000 (two flags at +0x00/+0x04, bit 15; masked) |
| 6 | Power button (interrupt controller +0x20; "Noise on Power Button pin") |
| 9, 10, 17, 25 | Timers 3, 2, 1, 0 (only 9 is unmasked) |
| 12 | Graphics engine (0xD0800000) |
| 14 | Serial port receive |
| 24 | DMA controller |
| 27 | Video |
| 28, 29 | Analogue-to-digital converter |
| 30, 31 | 0xC00D0000 channels 1 and 0: two DMA-fed channels, set up at start but not used by the menus (perhaps an audio codec link: the test mode has an "Audio CODEC test" and reports left and right channels) |

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

## Timers (0xC0020000)

Four timers at 0xC0020000 + 0x20 n: +0x00 control (0x6062 written: bits 14
and 13 on; bit 15 the overflow flag, cleared by writing 1), +0x08 the reload
value (the counter runs from it up to 0x10000). Timer n interrupts on line
25, 17, 10 or 9. The kernel runs timer 3 with reload 0xFFE0 (32 counts) and
polls the controllers from its interrupt. The research tool assumes a 32768 Hz
clock, one interrupt a millisecond, which fits the controller code's timeout
of 2000 interrupts. The clock is UNVERIFIED.

## Serial port (0xC0060000)

Like ARM's PL011 UART, except the status register's place:

| Offset | Use |
|---|---|
| +0x00 | Data: writes send a byte (the log), reads take a received byte |
| +0x08 | Control (0xB070 at the end): bits 6 and 5 enable receive interrupts (the kernel has helpers to set and clear them) |
| +0x0C | 0x7530 written (baud divisor?) |
| +0x10 | Status: bit 4 receive FIFO empty, bit 5 transmit FIFO full (the log waits while set), bit 15 and bit 13 receive interrupts (both read the FIFO until it is empty) |

The receive interrupt (line 14) copies bytes into a 32-byte ring at
0x002D72E4.

## Controllers

The mat and the joystick ("bopper") are wireless; the console's radio
receiver sends their state to the serial port. Every millisecond the timer
interrupt calls the kernel's decoder (0x2B154), which takes 4-byte packets:
a 16-bit word A, little-endian, then A ^ 1. The state is 10 bits: A's bits
15-11 become bits 9-5 and A's bits 7-3 bits 4-0 (A's bits 10-8 and 2-1 are
not used). When the state changes, it is queued as an event. With no packets
the menu shows a picture of the mat and joystick with radio waves (asking for
them to be switched on); with packets it does not, and the start-up goes on
to the "Sign In" menu as in the recording. How often the receiver sends is
UNKNOWN (the research tool sends once a frame; `ZIPBOOT_PAD=state:from:to`
sets the state).

The buttons are active-low: 0x3FF is nothing pressed. Each frame the kernel
(0x2C358) compares the state with the last one and keeps a mask of bits that
went from 1 to 0 (pressed, 0x2D29B8: tested in 103 places) and from 0 to 1
(released, 0x2D29C4: tested in one), and the menus also test bits held low.
Found by pressing each bit on the menus:

| Bit | Button | How it was found |
|---|---|---|
| 0 | Joystick up | "Sign In": the highlight moves up |
| 1 | Joystick down | "Sign In": the highlight moves down |
| 2 | Joystick left | Player screen: the highlight moves left (to the cancel button) |
| 3 | Joystick right | Player screen: the highlight moves right (to OK) |
| 4 | UNKNOWN | Shows a panel with a forward arrow over the menu |
| 5, 6 | UNKNOWN | No effect on these two screens |
| 7 | Choose | "Sign In": chooses the player, and the player screen follows as in the recording (the name at the top; Learning Path, back, cancel and OK) |
| 8, 9 | UNKNOWN | No effect; only ever tested as held, alone or together (0x300) |

The kernel tests bits 5-7 together (0xE0) and bits 0-3 together (0xF). A
guess (UNVERIFIED) is that bits 5-7 are the mat's red, green and blue pads,
bits 8 and 9 its two foot pads (both held: standing on the mat), and bit 4
the button on top of the joystick.

## Sound unit (0xD0400000)

A 32-channel sample player like the one in GeneralPlus's 16-bit chips
(MAME's `spg2xx_audio`), each 16-bit register four bytes apart. The kernel's
library for it is at 0x42B48 onward; `PlayAudio` (0x22F10) takes a free
channel from 20 to 31.

| Address | Use |
|---|---|
| +0x1000 + 0x40 n | Channel n: +0x00 wave address (halfwords, 22 bits), +0x04 mode (bit 15 ADPCM, bit 14 16-bit, bits 13-12 tone mode: 0 the CPU writes samples, 1 one-shot, 2 loop; bits 5-0 the address's top bits), +0x08 loop address, +0x0C pan (bits 14-8) and volume (bits 6-0), +0x10-0x28 envelope, +0x2C wave data, +0x34 ADPCM select |
| +0x1800 + 0x40 n | Channel n's phase registers: +0x10 the phase (19 bits) |
| +0xE00 / +0xE80 | Control for channels 0-15 / 16-31: +0x00 enable, +0x04 main volume, +0x08 FIQ enable, +0x0C FIQ status, +0x14 beat count (bit 15 interrupt enable, bit 14 its flag), +0x18-0x24 envelope clocks (4 bits a channel), +0x28 envelope ramp-down, +0x2C stop flags, +0x30 zero cross, +0x34 control, +0x3C busy |

A channel plays phase x 281250 / 2^19 samples a second (MAME's figure;
the menus use phase 0x7482: 16000 Hz). The menus' sounds are 4-bit OKI ADPCM,
one-shot, ending at the word 0xFFFF; each starts with a 4-word header
(0x007F 0x7F00 0x6440 0x8000). Unlike MAME's 16-bit chips, the nibbles are
taken high first: low first decodes to noise, high first to clean tones.
When a one-shot ends the channel stops, its enable bit clears and its stop
flag sets.

The music comes from a sequencer in the kernel run by the beat counter:
+0xE10 the base count (0x160), +0xE14 the beat count (0x30) with bit 15
enabling and bit 14 flagging (write 1 to clear) its interrupt, which is fast
interrupt (FIQ) line 2 (handler 0x1DE18; FIQ mask 0xD0100034: bit 4-n masks
line n). Each tick (4 cycles of the 281250 Hz clock) the base count runs
down; when it reloads the beat count drops; at 0 the flag rises. The music
uses looping ADPCM instruments (modes like 0xE1C7) with hardware envelopes
and ramp-downs (+0xE28 written 0xFFFFFFFF to fade everything).

Ribbit's research tool plays all of this as MAME's model does (envelopes,
ramp-down, interpolation, volume and pan, the control register's volume
select and the main volume), at 70312.5 Hz, and saves it with
`ZIPBOOT_WAV=file`. Its start-up audio follows a recording of a real unit:
the loudness over time correlates 0.64 with the recording's at one offset
and about 0 at others. Envelope IRQs, channel FIQs and the compressor are
not modelled. MAME keeps OKI ADPCM to 12 bits; the tool scales it to 16.

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
| +0x108 | 0x7042 | Sprite control: bit 0 on; bits 15-8 the number of sprites drawn, in fours (0x2F: 188 sprites, matching the 188 x 8 words the kernel sends by DMA that frame); 0x00031935 written |
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
- **16-bit pixels:** a sprite of depth 8 with palette bit 0 set (0x99F3,
  0x29F3, 0x33F3) has 16 bits per pixel, decided pixel by pixel:
  - **Bit 14 set:** bits 7-0 are a colour from the bank, with an opacity.
    From bank 0x200 (attribute bit 15 set) the opacity is bits 13-8 (0 to
    63): the menus' background art, a 64-pixel grid of these sprites, is
    blended over text layer 1 this way, and recolouring entries 0x200-0x27F
    gives each screen its colour. From bank 0x100 the pixel is solid unless
    bit 13 is set, and then bits 12-8 (0 to 31) are the opacity: the game
    pictures on the game-select ring are all 0x18 in bits 13-8 and look
    solid in recordings. Colours whose palette entry has bit 15 set are
    transparent.
  - **Bit 14 clear:** the pixel is the colour itself, RGB 5:5:5, and 0 is
    transparent. The menus' button panels use this. Bit 15 is set in some
    light pixels; its meaning is UNKNOWN.

  Which attribute bit selects 16-bit pixels, and the two opacity encodings,
  are UNVERIFIED.
- **Byte 8 bit 7** is set on some sprites (e.g. game pictures). It is not an
  address bit (nothing is there); its meaning is UNKNOWN.
- **Count:** only the first (control bits 15-8) x 4 entries are drawn; the
  rest of sprite RAM holds stale entries from earlier screens.
- **Order:** priority 0 to 3, and within a priority later sprites on top.

**Corner converter (+0x300-0x330):** for each sprite the kernel writes four
corners, x then y (+0x300/+0x304 to +0x318/+0x31C, the same centred
coordinates; a 64x64 sprite gives (-275, 61), (-212, 61), (-212, -2),
(-275, -2)). It then reads five halfwords (+0x320 to +0x330) into the
sprite's halfwords 1, 2, 5, 6 and 7. This looks like the 16-bit chip's
"virtual 3D" sprites (PPU-enable bit 9, set). What the hardware returns is
UNKNOWN. The kernel shrinks sprites with it: a 64x64 sprite sent as four
equal corners is hidden. The research tool returns its own encoding, which
only its renderer reads: the first corner (halfwords 1 and 2), the third
corner (5 and 6) and 0x8000 (7); its renderer scales the sprite to that
rectangle. Skewed quads are not handled.

With the controllers idle (0x3FF), choosing a player gives the screen the
recording shows: the name's button grows wide at the top, with Learning
Path, back, cancel and OK below. (Earlier runs sent 0, every button held, and
showed a large direct-colour panel instead; controller bit 4 still shows that
panel, and whether it looks right is UNKNOWN.)

Choosing OK on the player screen (right, then choose) logs "Already ran
tutorial for player..." and goes on to the game-select screen as in the
recording: the Zippity logo, the Playhouse Disney ears and a ring of game
pictures in bubbles that turns. The pictures are shrunk through the corner
converter (64x64 to 39x39 at the back of the ring). Still wrong: only the top
part of each picture shows, and a patch next to the logo is noise.

## Research tool

`crates/ribbit-arm/examples/zipboot.rs <nand image> [instructions]`, with
environment variables:

| Variable | Use |
|---|---|
| `ZIPBOOT_DUMP=dir` | Save RAM (`ram.bin`), on-chip RAM and the logs below at the end |
| `ZIPBOOT_FRAMES=n` | Save the picture every n instructions (`frame_<millions>.ppm`) |
| `ZIPBOOT_EVERY=n` | Save RAM every n instructions |
| `ZIPBOOT_SAVE=file:n`, `ZIPBOOT_LOAD=file` | Save the machine after n instructions; carry on from such a file |
| `ZIPBOOT_PAD=state:from:to[;...]` | The controllers' 10-bit state (hex) over instruction counts |
| `ZIPBOOT_PRESS=port:mask:from:to[;...]` | Flip GPIO input bits (port a, b or c) for a while |
| `ZIPBOOT_WATCH=lo-hi`, `ZIPBOOT_WATCH_FROM=n` | Log I/O accesses in that range (`watch.log`) |
| `ZIPBOOT_RWATCH=lo-hi` | Log CPU writes to memory in that range (`rwatch.log`) |
| `ZIPBOOT_JUMPS[=n]` | Print the last n jumps (600) |
| `ZIPBOOT_WAV=file` | Save the sound as a WAV file |
| `ZIPBOOT_SPULOG` | Print each sound channel start (mode, address, phase, volume) |
