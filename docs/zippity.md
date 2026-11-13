# LeapFrog Zippity

The Zippity (2009) is a TV console unrelated to the Leapster hardware. Its
system-on-chip is a GeneralPlus GPL32300A with an ARM9 core (ARMv5TE, emulated
by `crates/ribbit-arm`; the machine is `crates/ribbit-zippity`). There is no public documentation for the chip, and
MAME only has a stub (`tvgames/generalplus_gpl32612.cpp`). Everything below
was learned by running the dumped firmware on the ARM model with
`crates/ribbit-zippity/examples/zipboot.rs`, and is UNVERIFIED on hardware unless
it says otherwise.

## Dumps

- **System NAND:** Micron MT29F2G08AACWP, ID `2C DA 80 15`. 2048 blocks of 64
  pages, each page 0x800 data + 0x40 spare (0x1080_0000 bytes in all). Two
  dumps are known: US (CRC32 16248b63) and UK (27d172ae).
- **Page 0:** a header that starts "PGandnand", then words 0x20000000, 0x200fc,
  0x20100000, 0x20100258, 0x20100270, 1, 0x40001. Stage 1 runs in SRAM at
  about 0x20100000.
- **Cartridges:** raw dumps of a 64 MB small-page NAND (pages of 0x200 bytes
  plus 0x10 spare, 0x420_0000 bytes) holding a FAT16 file system (OEM name
  MSDOS5.0) with KERNEL.BIN plus mdp_sprites.bin, mdp_backgrounds.bin and
  mdp_wavs.bin. See "Cartridges" below.

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
| 0xC0000000 | GPIO: port A +0x00 (data in) / +0x04 (out) / +0x08 / +0x0C, port B +0x20.., port C +0x40.. (the kernel logs port C as R_IOC_I_DATA). Port C bit 10 is the power button (low: pressed); port A bit 6 is a TV-standard strap (low: 50 Hz; its callers choose 50 or 60 and 5000 or 5994); port B bit 5 is read active-low (meaning UNKNOWN); port A bit 4 is high when a cartridge is in the slot (the test mode's "Cart is seated"). 0x108 bits 5-4 select the NAND chip (see Cartridges). Port B bit 0, port C bits 5/4 and bits 6/5 of 0xC00000C4 drive five LEDs (the factory test mode's "Leds:"). Also clock or pad registers at 0xA0-0xAC, 0x104/0x108, 0x124, 0x140 |
| 0xC0020000 | Timers (below) |
| 0xC0060000 | Serial port (below): the debug log goes out, the controllers' radio receiver comes in |
| 0xC00C0000 | Analogue-to-digital converter (below) |
| 0xD0100000 | Interrupt controller (below) |
| 0xD0300000 | DMA controller (below) |
| 0xD0400000 | Sound unit (below) |
| 0xD0500000 | Video chip (below): registers, palette RAM at +0x1000, sprite RAM at +0x2000 |
| 0xD0800000 | Graphics engine? (MAME's stub feeds +0x18 random values): +0x00 control (0x901 starts), +0x04/+0x08/+0x0C parameters, +0x7C done (bit 0, write 1 to clear); interrupt line 12 |
| 0xD0900000 | NAND controller (below) |
| 0xF6004000 | Fixed-point maths unit (below): matrix product (started by writing 0xF6800000 or 0xF6800004) and arithmetic |
| 0xF8000000 | On-chip RAM (the bootloader runs code there; 64 KB assumed) |
| 0xFF000000 | Cache controller: +0x00 control (bit 2: enabled), +0x18 maintenance operation (0x80000001/5/9/D), +0x1C/+0x20 range (address \| 0x80000002) |

## NAND controller (0xD0900000)

Modelled from its driver in the bootloader:

| Offset | Use |
|---|---|
| +0x000 | DMA control: bit 0 start (reads 1 while busy), bit 1 direction (1: memory to NAND). Ribbit keeps it busy at 15 MB/s (fitted with the CPU speed, UNVERIFIED; see "Speed"); data from the NAND lands when the transfer ends |
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

## Cartridges

A cartridge is a second NAND chip on the same controller (0xD0900000):

- **Detect:** port A bit 4 (0xC0000000) reads 1 with a cartridge in the slot
  (the kernel's function at 0x2BC8C; the test mode prints "Cartridge: Cart is
  seated" or "No Cartridge detected").
- **Chip select:** GPIO 0xC0000108 bits 5-4. 0 is the system NAND. The
  bootloader and the kernel set 1 and then 2 and read the chip ID each time;
  the kernel's chip select (0x31908) also writes per-chip values to +0x154
  and +0x140's low byte from a table of 3-byte entries.
- **ID:** the kernel's check (0x31C14) returns 0 for no chip (ID 0xFFFF), 2
  for an ID starting 45 76 (SanDisk, 64 MB) and 1 for anything else; only 2
  is used as a cartridge ("Cart seated but loading cart fails:(" otherwise).
  The ID routine (0x39910) keeps four bytes, but nothing reads the two after
  45 76: the device code 0x76 alone picks the layout (0x1000 blocks of 32
  pages of 0x200 + 0x10 bytes), so what a real chip sends there does not
  matter (Ribbit sends FF). Having found it on select 1, the kernel reads it
  with select 3 (both bits; why is UNKNOWN).
- **Reads:** command 0x00, +0x148 = sector << 8, +0x14C = 0, then a 0x200-byte
  DMA. The controller sends +0x148 as two address bytes and +0x14C after
  them, so on a small-page chip (one column byte) +0x148 bits 15-8 are the
  page number's low byte. The read starts on the address (no 0x30).

With these, the kernel logs "Cart seated and sucessfully loaded!", copies
the cartridge's log files (`qqtUS.LPD` and others) to the system NAND, and
after the player signs in runs "Kernal load test... kernel OK", loads the
cartridge's KERNEL.BIN ("LK file found, size is 0X0028C768") and starts it:
the game mounts its own file system ("Loaded filesystem: OTP") and looks for
its save (`MDP.P01` for Disney Princess, `MPF.P01` for The Princess and the
Frog). Both dumps reach their game menus (Disney Princess: Ariel's Fireworks
Display, Cinderella's Dress Dash, Snow White's Dancing Delight; The Princess
and the Frog: Hop Hoppin' In The Bayou, Tiana's Magical Wedding Dance,
Orders Up, Louis's Ragtime Jam Band), backgrounds and all.

The system goes to the cartridge straight after Sign In, before its own
menu: a new player first plays the Mickey Says tutorial and then lands on
the system menu, with the cartridge left alone until the next sign-in. The
guest (the fourth Sign In entry) goes to the cartridge at once.

A save state holds neither flash: loading one keeps the system NAND and the
cartridge in the slot (`Zippity::load_state`; it used to drop the cartridge,
and the system then found none at the next sign-in).

Played with scripted input (`zrun`, below): Disney Princess's Dress Dash
(Gus runs along three lanes, the spots change lane, landing on the home spot
picks up the bead ahead; the counter rises) and Ariel's Fireworks Display
(rockets on the spots) play through. In The Princess and the Frog's Hop
Hoppin' In The Bayou the frog hops along the path (marching) and the stage
ends, but no scripted input tried picked up a mushroom (counter 0): what
the game wants is UNKNOWN.

## Fixed-point maths unit (0xF6004000)

Found in the system kernel (its loader at 0x2D504, placing sprites) and
Disney Princess's game kernel (the same driver at 0x3D2D8-0x3D9E8 and
callers); UNVERIFIED on hardware. The loader writes a 4 x 4 matrix as it is,
or a 3 x 3 into the 4 x 4 rows, repeating each row's last word in the
fourth: 2-D transforms, (x, y, 1) times a matrix whose third row is the
translation. Both kernels start it by writing 1. Numbers are 16.16 fixed point. Results are
read straight after the start, so the unit is taken to answer at once.

| Address | Use |
|---|---|
| +0x00..+0x3C | Matrix, rows of four words. For 3 x 3 the driver leaves the fourth word of each row unused |
| +0x40..+0x4C | Vector in |
| 0xF6800000 / 0xF6800004 | Write 1: row vector times matrix, 4 x 4 / 3 x 3 |
| +0x60..+0x6C | Vector out: out j = sum over i of in i x M[i][j], >> 16 |
| +0x84 | Read by the driver, which writes 0 to the address it gives (reads 0 here, so it clears RAM word 0; meaning UNKNOWN) |
| +0xA8 | Arithmetic operation: 1 = (a x b) >> 16 (a software routine for that is replaced by it), 3 = signed division (sprite scaling steps, wrapping) |
| +0xA0 / +0xA4 | Operands a and b |
| +0xAC / +0xB0 | Results: the product (and the bits above it) / the quotient and remainder. Other operations UNKNOWN (read 0) |

Dress Dash loads a 2-D transform (scale 0.6, translation in the last row:
x' = 0.6 x + 120) and puts its pictures through it. Before Ribbit modelled
the unit every result read 0: the bead counter was never drawn and Gus never
picked up a bead, so the game ran on with nothing collected, as reported for
Disney-Pixar Cars on the Zippity (McQueen drives on, the progress bar stays
at START, the hubcap counter at 0). The Cars cartridge has not been dumped
here, so whether it uses the unit is UNVERIFIED; The Princess and the Frog's
games do not touch it. Neither the system nor Mickey Says does.

**Disney-Pixar Cars** is not in either system dump (US or UK): both hold
only mmch, hm, le and mftp kernels (Mickey Mouse Clubhouse, Handy Manny,
Little Einsteins, My Friends Tigger & Pooh), and the game-select ring shows
those four. The system kernel's list of cartridges whose logs it copies has
`MDP` (Disney Princess) and `MCA` (`a:\MCAUS.LPD`, `MCAUK.LPD`, `MCA.LPI`):
Cars is that cartridge, not dumped here, so its stuck driving game cannot
be reproduced.

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
less. What full batteries read is UNKNOWN; Ribbit returns 0x7000 at once,
which the test mode (below) shows as "ADC: 28672, 5490mV".

## Timers (0xC0020000)

Four timers at 0xC0020000 + 0x20 n: +0x00 control (0x6062 written: bits 14
and 13 on; bit 15 the overflow flag, cleared by writing 1), +0x08 the reload
value (the counter runs from it up to 0x10000). Timer n interrupts on line
25, 17, 10 or 9. The kernel runs timer 3 with reload 0xFFE0 (32 counts) and
polls the controllers from its interrupt. The research tool assumes a 32768 Hz
clock, one interrupt a millisecond, which fits the controller code's timeout
of 2000 interrupts. The clock is UNVERIFIED. Mickey Says and both
cartridge games' kernels run timer 3 the same way and no other timer.

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
UNKNOWN; Ribbit sends once a millisecond (`ZIPBOOT_PAD=state:from:to` sets
the state). Once a frame is too slow: while any input is held (0x2B96C with
mask 0x3FF), the menus snapshot the valid-packet count (decoder +0xC) every
other frame (0xCC18), and the controller check (0x19FDC, every 600 of its
time units) compares it with the same snapshot (0x19EEC). With one packet a
frame the check could come a sixth of a frame after the snapshot with no
packet in between, take the controllers for switched off and show the
picture: standing on the home spot at the language screen brought it up.
The check keeps its snapshot at RAM 0x16FA40 and its state at 0x16FA44
(1: the panel); the decoder's data is at 0x1BE4A0. A packet goes into the
serial port's receive FIFO whole or not at all (a part would put the decoder
out of step). `tests/controllers.rs` stands on the home spot at the language
screen for two minutes and then mashes the feet and the bopper for one: the
panel never comes, while once a frame (`Probe::radio_period`, `ZRUN_RADIO`)
brings it up within 20 s.

The buttons are active-low: 0x3FF is nothing pressed. Each frame the kernel
(0x2C358) compares the state with the last one and keeps a mask of bits that
went from 1 to 0 (pressed, 0x2D29B8: tested in 103 places) and from 0 to 1
(released, 0x2D29C4: tested in one), and the menus also test bits held low.
Found by pressing each bit on the menus:

| Bit | Button | How it was found |
|---|---|---|
| 0 | Bopper (joystick) up | "Sign In": the highlight moves up |
| 1 | Bopper down | "Sign In": the highlight moves down |
| 2 | Bopper left | Player screen: the highlight moves left (to the cancel button) |
| 3 | Bopper right | Player screen: the highlight moves right (to OK) |
| 4 | Pause/Home button | Test menu: "Home = Back" (pressing it goes back a screen); on the menus it shows a panel with a forward arrow (pause?) |
| 5 | Red spot | Test menu: "Red = Cartridge Test" |
| 6 | Blue spot | Test menu: "Blue = Button Test" (opens it) |
| 7 | Green spot (Choose) | "Sign In": chooses the player, and the player screen follows as in the recording; test menu: "Green = Clear User Log" |
| 8, 9 | The home spot's two footprints | Mickey Says' "stand on the mat" screen goes on to the game when either or both are held; the kernel also tests them together (0x300) |

The kernel tests bits 5-7 together (0xE0) and bits 0-3 together (0xF). The
manual (LeapFrog's Parent Guide, 605-11430-A, at
https://t7.leapfrog.com/images/z/Zippity.pdf) names exactly ten inputs: the
bopper (a joystick on the main unit), a Pause/Home button on the main unit
("To exit a game, press the Home Button"), and the mat's orange home spot
(two footprints), red, blue and green spots (blue, the "Zippity" oval, in
front of the home spot; red and green to the sides). The main unit talks to
the console by infrared (the test menu's "IR Reciever"; the manual asks for
line of sight). A review says green "doubles as the 'enter' button". The
manual also says to change the language at Sign In by standing on red and
green and pushing the bopper up.

## Test mode and the user data

Holding the power button (port C bit 10 low) at power-on with port B bit 5
low makes the kernel print `R_IOC_I_DATA = 0xfbff` and "PowerButton Still
held down. Entering interactive test mode...". Its screen offers "Red =
Cartridge Test, Blue = Button Test, Green = Clear User Log, Home = Back" and
shows the battery reading and "Base Software ID 152-12300". (Dispatcher state
36 runs the test function 0x2BC0; the flag is read at 0x2BC74.)

Green (bit 7) deletes every file the system and games saved: UI.SAV (the
players), the games' progress (`MMCH.P01`..., one per game and player) and the
parent reports (`MAE*.LPD`, `MAE.LPI`), then prints "Erased all user data."
and shows "User Data Clear: OK" (the result flag is RAM 0xAB660). A system
started afterwards logs "LoadData could not open: UI.SAV", asks for the
language, then shows "Sign In" with three empty players.

The file system ("OSIA NFlash File...", blocks near the end of the NAND,
pages 0x1DF80-0x1E5xx in the US dump) only unlinks the files. A block it does
not rewrite keeps the old data: in the US dump block 1924 (page 0x1E100) still
held the players' names and a parent report after the clear. The system
erases that block itself the next time it needs room, so erasing it at once
changes nothing it relies on. `Zippity::factory_reset` does the test mode's
clear and then erases every block between the lowest and highest one the
clear wrote that the clear left alone. Checked on the US dump only: the result
boots the same, makes a player and saves it as before (UNVERIFIED as a rule
for other dumps). The two blocks at the very end (pages 0x1FF80-0x1FFFF) are
untouched (probably the bad block table).

The `zipclean` example writes such a cleaned copy of a dump. The US dump
cleaned this way is in Ribbit's list of known dumps as "Zippity (US) NAND,
user data cleared" (CRC32 de80d788).

## In the Ribbit app

The library lists a Zippity NAND dump as "Zippity Built-in Games (US)",
tagged "Internal"; cartridge dumps (raw, or a .zip of one) are listed as
Zippity games. Playing either starts the system (from the NAND dump in the
games folders, whatever console the library has chosen), with the cartridge
in the slot for a cartridge, on its own thread at 60 frames a second; the TV
picture is 640 x 480 and the window keeps 4:3. The Zippity has its own keys,
laid out as the mat: Z, X and C the red, blue and green spots (green
chooses), A and S the left and right feet above them, D standing on the
home spot, the arrows the bopper and Space its Pause/Home button. They
replace the Leapster's keys while a Zippity runs and can be changed in
Input > Key bindings (console "Zippity"; Help > Controls (F1) lists them).
A game controller keeps the Leapster's buttons: D-pad = bopper, A = green,
B = red, X or Y = blue, Back = Pause/Home, Start = standing on the home
spot, LB / RB = the left / right foot. Input > "Zippity: stand on the home
spot" (or Shift, while a Zippity runs and no input has it) keeps the home
spot's footprints pressed, as a child standing on it does: Mickey Says and
others wait for it. A foot key moves its foot from there: lifted while
standing on the home spot, put down while not, so marching in place is A
and S in turn. The stand key moves both the same way: while standing there
it lifts both feet, a jump, and letting go lands (games such as Dress Dash
act on both footprints pressed at once, which standing still never gives).
Every one of the ten inputs has a key.

### Mickey Says

The game (its own kernel, `mmch_kernel`: "LK file found, size is
0X0023BD10") reads the controllers through the same packet decoder as the
system and turns state changes into "pressed" and "released" masks each
frame (0x2C270; queries at 0x2B884 "held" and 0x2B8A8 "pressed"). Its pad
handler (0x12208) lights the pad stepped on for 15 frames. Its command types,
from its debug strings: COLORED_SPOT, MARCH_IN_PLACE, NUM_SPOT, NUM_BOPPER,
SPOT_COUNT_UP, SPOT_W_NUM_AFTER_X, MARCH_X_TIMES, BOPPER_X_TIMES,
SPOT_W_NUM_BEFORE_X, SPOT_W_HIGHEST_NUM, SPOT_W_LOWEST_NUM,
ALL_SPOTS_MORE_THAN_X and ALL_SPOTS_LESS_THAN_X. Played in Ribbit from a
saved state: the first command is marching in place (the feet lifted in
turn: 0 to 2 points), the second stepping on green (2 to 4), as in the
recording of a real unit (the score rising by 2 or 3 a command). It first
shows the mat and waits for the player to step onto the home spot.
A cheat mode ("MMCH: CHEAT MODE ON", 0x1B81C) is turned on by the launch
parameter the system passes in on-chip RAM at 0xF800000E, which is the
player's name (the game prints it as "MMCH: player_name: [%s]"): a player
named VMAESTER (compared at 0x279D0) plays with cheats. The Princess and the
Frog has a "Cheat Mode On" too. What the system saves goes to `<save folder>/<dump
name>.nandpages` (the pages that differ from the dump); the dump file is not
changed.

When the system loads saved players from a dump it is playing for the first
time ("LD open OK: UI.SAV" in its log), the app offers to clear them from the
dump file itself ("Start fresh?"). Clearing keeps the original next to it as
`<name>.with-saves.bak`; "Keep" is remembered (settings
`zippity_keep_saves`).

## Speed

The system spends most of its time waiting: about 77-87% of its
instructions (menus and Mickey Says alike) are a three-instruction loop at
0x2CFF8 polling a flag in RAM (0x2D2AE8 + 0x74) that the video interrupt
sets. `Zippity::run` skips such idle loops: a jump back of under 64 bytes,
with no write and no I/O access since the last time round and every
register and flag the same, can only end when a device tick or frame
changes something (or, for a loop polling the NAND controller's status,
the transfer's end). Whole times round are skipped up to the next of
those, and the few instructions left are run, so the CPU is exactly where
it would have been. The time round before must itself have had no tick,
frame or end of transfer in it. Sound, pictures, memory and registers come
out bit-identical with and without skipping (`zrun` prints hashes of both;
`ZRUN_NO_SKIP` turns skipping off; the test `idle_skip_is_bit_exact`
compares them). Before, a skip went straight to the next event, leaving the
CPU a few instructions off where it would have been and seeing the end of
a NAND transfer up to 256 instructions late; the timeline then drifted, and
the start-up's sound differed from 6 s on.

On an i9-13900K P-core the emulation runs about 7-9 times faster than real
time at 27 million instructions a second (start-up, menus, Mickey Says),
its slowest frame (loading) about 11 ms of the 16.7 ms a frame has. With
32 copies running at once on its 32 threads, each still ran 3.8 times
faster than real time. Of what is left, most is the CPU core itself
(instruction fetch through the MMU, RAM reads, the decode switch).

### CPU clock

The real CPU clock and memory wait states are UNKNOWN; Ribbit runs 27
million instructions a second (`FRAME`, 450,000 a 60 Hz field), with NAND
transfers at 15 MB/s. Both are fitted to recordings of real units, timed
frame by frame (`zrun` with `ZRUN_SIG`, against the recordings' frames
scaled to 8 x 6 blocks):

- **Start-up (YouTube nt37NPE5-R8):** the warning screen stays up while the
  kernel decodes the LeapFrog logo's pictures (busy 100%, mostly a bit
  unpacking loop at 0x29EC0): pure CPU work, unchanged by the NAND's speed.
  From its fade-in to the next screen: 3.30 s on the recording; here 4.20 s
  at 18 million, 3.60 s at 24, 2.97 s at 36, 3.30 s at 27. The LeapFrog
  logo's sound then starts 0.18 s after the warning goes (here 0.25 s, within
  the recording's 1/30 s frames). Power-on to the warning is 2.10 s here;
  the recording starts 2.17 s before it (with the TV's blue screen, so
  power-on is about its start).
- **Mickey Says' load (YouTube 15yjwhWUOYQ):** its "LOADING" screen lasts
  6.6 s. Here it lasts about 101 million / CPU + 40 MB / NAND + 0.3 s (from
  runs at 15-36 million and 7.2-28.8 MB/s): 11.5 s before (18 million, 7.2
  MB/s), 6.6 s now. That recording's LOADING is on another route (Mickey's
  hub to Mickey Says), where Ribbit's lasts 5.4 s (see "Fades"). The 2.35 s from choosing the player to "LOADING" does not
  depend on either (on the recording the screen before is a picture of the
  mat for 1.8 s, then black; here the Sign In picture stays up).

Frame pacing (video interrupt bits 0 and 11, "Video chip"): the kernel's
timers add up the fields counted on bit 11 and divide by 60 (50 on a PAL
unit), so bit 11 comes every field; its main loop runs every second field,
held by the frame commit's busy flag. Before, bit 11 came every second field:
the LeapFrog logo's animation changed picture every 10 fields (about 6 on the
recording, 4-6 now) and the logo stood still 0.6 s (1.1 s there, 1.0 s now).
Still short of the recording: from the warning going to the mat demo
9.8 s against 11.7 s. The start-up screens are not timed by a clock, a
timer, a sound ending or the RTC: the sequence (0xC2E8, called from the
state machine at 0x11F78 before the mat demo at 0xD744) counts its own
main-loop passes (`[fp-0x10]`, one per call of the frame wait 0x2CFF0) and
acts at fixed counts: 0x32 the warning goes, 0x42 the LeapFrog voice and
logo, 0x74 and 0x80 the Disney stings, 0xB2 the Zippity sting, 0xCF
"zippity", 0xF6 the credits' sting, 0x12C the end (301 passes; the
animations within each screen run on the field count, see below). Here a
pass is 2 fields except where the kernel loads in between. Each sting
starts its sound at a known pass, so matching the recording's sound to
Ribbit's (log-frequency spectra, 0.5 s windows at each sting) times those
passes on the real unit; the music, run by the beat counter, lines up
throughout (lag 0.00 s), so the offsets are the passes'. Relative to Ribbit:

| Pass | Event | Ribbit (s after the warning) | Real unit |
|---|---|---|---|
| 50 (0x32) | warning goes | 3.32 | +0.01 |
| 66 (0x42) | LeapFrog voice | 3.87 | -0.05 |
| 128 (0x80) | second Disney sting | 6.04 | +0.50 |
| 178 (0xB2) | Zippity sting | 7.70 | +0.66 |
| 207 (0xCF) | "zippity" | 8.66 | +0.76 (picture: +0.75) |
| 248 | credits shown | 10.05 | +0.98 (picture) |
| mat demo | first picture | 13.15 | +1.88 (picture and music) |

So the real unit loses time in every stretch, not only on the credits:
0.55 s over passes 66-128 (where Ribbit loads the Disney pictures between
passes, 5-field passes from field 354 to 430), 0.16 s over passes 128-178
and 0.23 s over 207-248 (no loading here: the real unit's passes average
2.2-2.3 fields, so some passes take 3 fields there), and 0.9 s from the
credits to the mat demo (52 passes, then 57 fields of loading the demo's
pictures, about 2.5 MB from the NAND unpacked with DMA copies, and 27
fields setting it up). Both kinds point at the video chip and the loads,
not the frame interrupts: the busy flag of the frame commit (0xD05001F0
bit 14, see `Bus::frame`), whose real length is UNKNOWN (a pass's own work
is about 5% of its 2 fields here, and the sprite lists of the screens,
80-93 sprites and about 0.27 Mpixels each, do not tell the slower screens
from the others), and the speed of loading, which Ribbit gives as one
instruction rate. The warning's decoding sets that rate (27 million); the
credits' load would need about 1.4 times as long, and the warning's own
pictures (below) much less, so a single rate cannot fit all three: the
real unit's speed depends on what the code does (memory, DMA, NAND),
which is not modelled. These recordings were made on units whose TV
standard is UNKNOWN (the Mickey Says and Goofy recordings carry 50 Hz
mains hum; the kernel's only TV strings say NTSC); 50 fields a second
would make every pass 1.2 times as long, which does not fit either (the
passes 50-66 take 0.49 s on the recording, 0.55 s here, 0.64 s at 50 Hz).

Two models were tried against all of these at once and rejected:

- **Frame-commit busy set by drawing time** (busy for a time growing with
  the sprite list's pixels or sprites, so heavy screens take 3 fields). The
  passes would need: 215 kpixels (passes 50-66, a steady list) 1.84 fields
  on average, 237-282 kpixels (Disney, Zippity) 2.2, 300-330 kpixels
  ("zippity") 2.34. A drawing time that crosses one field at 215 kpixels and
  two at about 270 kpixels needs a negative fixed cost (about -2.9 fields),
  and a steady list cannot give a mix of 1- and 2-field passes; the sprite
  counts (60-100) do not fit either. Ribbit keeps the commit busy until the
  field after next.
- **CPU cost by kind of work** (cached code, memory, DMA transfers and their
  interrupts, NAND reads). Work per stretch, counted instruction by
  instruction:

  | Stretch (fields) | Instructions | DMA transfers | NAND | Needed on hardware |
  |---|---|---|---|---|
  | 169-276 (logo decoding, warning up) | 48.2 M | 149 200 | 1.35 MB | about 0.95 times |
  | 828-914 (mat demo's load) | 38.3 M | 119 300 | 3.11 MB | about 1.4 times |
  | hub route, LOADING (323 fields) | | | | about 1.24 times (400 fields) |
  | hub route, black before the mat picture (22 fields) | | | | about 1.6 times (36) |

  Every load but the first would have to be slower, and the first (the
  anchor of the 27 million rate) a little faster. It does the same kind of
  work as the others (an LZ decoder copying with DMA, 290-320 instructions
  a transfer, NAND reads with interrupts held off about a fifth of the
  time), except for the bit unpacking at 0x29EC0 (39% of it, 3% of the mat
  demo's load): only making that 4 times faster than the rest fits, which
  nothing known about the hardware supports. Sign In's LOADING (178 M
  instructions, 41.5 MB from the NAND, 6.6 s here) has no recording. Ribbit
  keeps one instruction rate.

The warning's own pictures needed no faster loading after all: its fade
waits for the first frame (see "Fades" below).

Fades: the kernel steps the fade register (0xD05000C0) by 0x10 a field in
the bit 0 handler (0x2E410), 16 fields from black, as the recordings' 0.27 s
fade-ins. The video interrupt's bit 0 comes only while a committed frame is
on its way (each field from the commit until the field it comes up), and
the fade shown is the register's value when the last frame came up (see
`Bus::frame`). The firmware supports this: the bit 0 handler rotates the
three frame buffers whenever it finds the commit done, which only makes
sense once per frame. What it explains, all against recordings:

- **The warning's fade-in** (start-up recording nt37NPE5-R8): the kernel
  starts it (0x2E07C, field 107.9) while it still reads and unpacks the
  warning's pictures (fields 98-125: 1.1 MB from the NAND, an LZ decoder
  copying with DMA), and sends the first frame at 125.4. With bit 0 every
  field the fade had ended over black by then and the warning popped up;
  now it waits and runs over the warning (fields 126-142), and the warning
  stays 200 fields from the fade's start (198 on the recording).
- **Mickey's hub to Mickey Says** (YouTube 15yjwhWUOYQ, 9-19 s, the same
  route in Ribbit: the ring, Mickey Mouse Clubhouse's menu, Mickey Says,
  Level 1, standing on the home spot), in fields:

  | | Recording | Before | Now |
  |---|---|---|---|
  | Menu fade-out | 12 | 16 | 16 |
  | Black | 36 | 15, then the menu fading back in for 7 | 22 |
  | Mat picture fade-in | 16 | 8 | 16 |
  | Mat picture held | 100 | 117 | 109 |
  | Fade-out, black | 12, 16 | 15, 9 | 14, 10 |
  | LOADING | 400, steady | 325: black 2, fading in 16, then steady | 323, steady |
  | Game | fades in after 1 black frame | fades in | fades in after 1 black field |

  The mat picture's fade-in started 7 fields before its first frame, so the
  menu came back up dimmed under it. LOADING is shown at once (0x2E0CC,
  frame sent), then the kernel sets the fade to black and starts a fade-in
  (0x2E07C) just before reading the program; no frame comes until the game
  sends one, so LOADING stays up, as on the recording, and the game's first
  frames take the fade from black. The stretches of loading (the black
  before the mat picture, LOADING) are still shorter here: see "CPU clock".
- **Sign In to Mickey Says** (no recording of this route): LOADING stays up
  4.0 s, then the new program fades it out (0x00 to 0xFF from field 2659,
  sending frames) and its fade back in (2695-2711) comes while it sends no
  frames, so the screen stays black 2.3 s until its first picture fades in.
  Before, a fade-in left 16 fields without a step counted as finished
  (LOADING came up 0.27 s after black, with a 0.9 s dip later).

Interrupts are off (0xD0100038) only while the program is read (22.5 fields
on this route); the new program's video interrupt runs every field from
2453. The fade register itself does not redraw the picture.

The 1.9 s here between choosing the player and Sign In
fading out are frame-paced waiting (90% in the main loop's frame wait).

`zrun <nand> <frames> <out.ppm> [pad script]` times a run (`ZRUN_FROM=n`
times frames n onwards only; `ZRUN_WAV=file` saves the sound,
`ZRUN_SHOTS=dir:n` the picture every n frames (`ZRUN_SHOTS=n`: next to the
output picture), `ZRUN_SIG=file` each frame's 8 x 6 block colours,
`ZRUN_LOGT` prints the system's log with times, `ZRUN_CART=file` inserts a
cartridge); `zpc <nand> <frames>` histograms where the program counter
spends its time (`ZPC_FROM=n`: from frame n). The pad script's times are
instructions (frames x `FRAME`), or frames with an `f` suffix
(`37f:2420f:2428f`). From a cleared dump, Mickey Says starts with green
(state 37f) at frame 1800 (the language), bopper down (3fd) three times from
frame 2150, green at 2280 (Guest). `zrun` also plays games for research:
`ZRUN_SAVE=prefix:frame` / `ZRUN_LOAD=prefix` (the machine and its NAND, to
carry on from a screen), `ZRUN_IO` (every I/O register's access counts),
`ZRUN_RAM` and `ZRUN_LEVELS` (the sound's peak each second).
`crates/ribbit-zippity/tests/carts.rs` runs a cartridge through a state load
and the guest sign-in, and into Dress Dash (with `RIBBIT_ZIPPITY_NAND` and
`RIBBIT_ZIPPITY_CART` set); `tests/screens.rs` hashes the pictures along a
route scripted in instructions (`RIBBIT_ZIPPITY_NAND`, and
`RIBBIT_ZIPPITY_FROG` for The Princess and the Frog's screens).

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
the menus use phase 0x7482: 16000 Hz). The sounds are 4-bit **IMA ADPCM, low
nibble first** (mode bit 15), ending at the word 0xFFFF. Decoded that way
every sound tried stays centred and most end at exactly 0, as an encoder
fading to silence leaves them; OKI ADPCM (MAME's model of the 16-bit chips)
or high nibble first drift into rumble and clip. Before each sound in memory
is an 8-word header of the sound library's (e.g. `0000 0516 0000 3255 0032
0302 6440 8000`: the second word is about the length in words plus 0x16);
the channel is started after it.

Looping instruments (tone mode 2, modes like 0xE1C7: ADPCM and 16-bit) are
two pieces: an ADPCM attack ending in 0xFFFF, then, right after it, the loop
as plain 16-bit samples (unsigned, 0x8000 the middle) ending in 0xFFFF. The
loop address points at the second piece. At the first 0xFFFF the channel
jumps there and its ADPCM bit clears (as in MAME's model), so the loop plays
as 16-bit samples. Decoding the loop as ADPCM gave the buzz and hum of
Ribbit's first sound model. When a one-shot ends the channel stops, its
enable bit clears and its stop flag sets.

Checked against a recording of a real unit starting up (YouTube
nt37NPE5-R8): the spectrogram shows the same opening sweep, voice and music,
with clean harmonics; level, peak and the energy below 60 Hz are within a
few dB of the recording's (before: 12 dB too loud, with 29 dB more rumble
and a DC offset). Pitch and tempo, from log-frequency spectrograms of
the two (1/192 octave, 10 ms steps): every stretch that matches well comes
out at 0 cents (so the 281250 Hz clock and the phase formula are right to
about 0.4%), and over the music from 6 to 13.5 s into the recording the
emulation takes 7.40 s for its 7.5 s (tempo within about 1-2%, as fine as a
camera recording allows). Someone who knows the hardware also remembers it
using IMA ADPCM (hearsay, but it agrees).

The music comes from a sequencer in the kernel run by the beat counter:
+0xE10 the base count (0x160), +0xE14 the beat count (0x30) with bit 15
enabling and bit 14 flagging (write 1 to clear) its interrupt, which is fast
interrupt (FIQ) line 2 (handler 0x1DE18; FIQ mask 0xD0100034: bit 4-n masks
line n). Each tick (4 cycles of the 281250 Hz clock) the base count runs
down; when it reloads the beat count drops; at 0 the flag rises. The music
uses looping ADPCM instruments (modes like 0xE1C7) with hardware envelopes
and ramp-downs (+0xE28 written 0xFFFFFFFF to fade everything).

Ribbit plays all of this as MAME's model does (envelopes, ramp-down,
interpolation, volume and pan, the control register's volume select and the
main volume), at 70312.5 Hz; the research tool saves it with
`ZIPBOOT_WAV=file` (`ZIPBOOT_SPULOG` prints each channel start with its
mode, addresses, phase, volume and register 0xD, the ADPCM select, which the
kernel leaves 0). Its start-up loudness over time correlates 0.87 with the
recording's, best at normal speed.

Checked again against three recordings (start-up, Mickey Says 15yjwhWUOYQ,
Goofy's Fancy Dancing vbWOv7rICVU), with Ribbit's sound of the same scenes
(`zrun` with `ZRUN_WAV`):

- **Spectrum:** long-term spectra, each set to 0 dB at 500-2000 Hz, agree
  within about 2 dB from 30 Hz to 16 kHz over the start-up music, and
  within about 3 dB over Mickey Says (its opening and its play). Above 13
  kHz the sound is 40 dB or more down, so the app's low-pass at 13 kHz and
  its 32 kHz output lose nothing audible; the 8-16 kHz band (where
  interpolating 16 kHz sounds leaves images) is as strong as on the
  recordings.
- **Tempo:** onset envelopes of the start-up music matched by time
  stretching: Ribbit's is 0.2-0.6% faster (6-13.5 s, 6-26 s and 14-26 s of
  the recording), within what a camera recording shows.
- **Hum:** the Mickey Says and Goofy recordings have steady 50 and 100 Hz
  lines all through (mains hum where they were recorded; the start-up
  recording has none). Ribbit's sound has no hum: its only steady lines are
  musical notes.
- **Clicks and level:** no sample-to-sample jumps standing out from the
  signal around them, no clipping, no DC offset (under 60 of 32768) over 75 s
  of start-up and Mickey Says; the peak is about a third of full scale.
- **The other games:** 30 s of play from Tool Box Beat, Darby's Honey
  Pot Jam, Rocket's Musical Mission and Snow White's Dancing Delight
  against their recordings (VuQE81MjPLk, W0vh0j5lwM8, 7N60JYOXRi4,
  O2neYHag2ug; not the same moments, so only the long-term spectrum): from
  150 Hz to 8 kHz within about 3 dB, except Honey Pot Jam's 2.5-5 kHz
  (Ribbit 4-6 dB lower, the recording's moment having more voice). Below
  100 Hz the recordings vary with the room and hum; above 13 kHz their
  encoding cuts off. Voices, music and effects all play.
- **Restarts:** the kernel never starts a sound on a channel still playing
  (no enable rewritten with a new address), and writes the mode, ADPCM bit
  included, at every start, so a loop clearing the ADPCM bit never carries
  over to the next note.

Unit tests in `sound.rs` check the IMA ADPCM decoding, a one-shot's end,
an attack looping on 16-bit samples and the beat counter;
`tests/timing.rs` (with `RIBBIT_ZIPPITY_NAND`) the start-up's timeline, its
sound sample for sample, and that idle-loop skipping changes nothing.

Not modelled, and what the kernel does with them (start-up to Mickey Says,
in MAME's register layout): channel FIQs are left off (+0xE08 written 0);
the zero-cross register (+0xE30) and manual envelopes (+0xE54) are 0. The
compressor is on: control +0xE34 = 0x08A8 (compressor enable 0x0800, volume
select 2, FOF and INIT), compressor control +0xE38 = 0xE03D (peak mode,
threshold 0x60 of 0x7F, release scale 3, zero-cross disabled, ratio 5) and
+0xE68 (attack/release) = 0x02FF. MAME does not model the compressor, so how
it acts is UNKNOWN. Ribbit's loudest output over that run is 11131, about 34%
of full scale, well under a threshold of 0x60/0x7F if that is a fraction of
full scale (UNVERIFIED): the compressor would then never act on these
sounds.

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
(`crates/ribbit-zippity/src/video.rs`). All of it is
UNVERIFIED on hardware.

| Offset | 16-bit chip | Use |
|---|---|---|
| +0x00-0x3C | 0x7000-0x700F | Text layers 3 and 4: scroll x and y at +0x00/+0x04 and +0x20/+0x24, attributes, control, tile map and attribute map at +0x10.. and +0x30.. (unused by the menu; see Text layers) |
| +0x40-0x54 | 0x7010-0x7015 | Text layer 1: x scroll, y scroll, attributes, control, tile map, attribute map |
| +0x58-0x6C | 0x7016-0x701B | Text layer 2 (unused by the menu) |
| +0x80/+0x84 | 0x7020/0x7021 | Text layer 1/2 picture base (0x003633EC) |
| +0x90 | 0x7024 | Text layer 4 picture base (layer 3's is taken to be +0x8C) |
| +0x88 | 0x7022 | Sprite picture base (0: sprites hold whole addresses) |
| +0xA8 | 0x702A | Blend level |
| +0xC0 | 0x7030 | Fade: bits 7-0 darken the picture towards black (0xFF black). The frame handler steps it by 0x10 a field between screens (out before the warning, in after it) and holds 0xFF while loading. It often changes without a sprite list being sent: Ribbit applies it as the picture goes out, without drawing the picture again. A fade-in left unstepped for 16 fields counts as finished ("Speed", fades) |
| +0xE8 | 0x703A | Palette control (0x10 written) |
| +0xF0 | 0x703C | TV control |
| +0x108 | 0x7042 | Sprite control: bit 0 on; bits 15-8 the number of sprites drawn, in fours (0x2F: 188 sprites, matching the 188 x 8 words the kernel sends by DMA that frame); 0x00031935 written |
| +0x188/+0x18C | 0x7062/0x7063 | Interrupt enable and status (below) |
| +0x1C0-0x1C8 | 0x7070-0x7072 | Video DMA: source, destination, 32-bit words minus 1 |
| +0x1E0/+0x1E8 | 0x7078/0x707A | Frame buffers: the one being drawn and the one shown (below) |
| +0x1FC | 0x707F | PPU enable (0x0038179F) |
| +0x200-0x220 | 0x7080-0x7088 | TV saturation, hue, brightness and so on (written once) |
| +0x300-0x330 | 0x70C0-0x70CC | Sprite corner converter (below) |
| +0x340-0x34C, +0x350-0x35C | - | Text layer 1 and 2 zoom (see Text layers) |
| +0x400 / +0x800 | 0x7100 / 0x7200 | Per-line tables (240 and 480 words; all 0 in the menu) |
| +0x1000 | 0x7300 | Palette RAM: 16-bit colours, two per word |
| +0x2000 | 0x7400 | Sprite RAM: 512 entries of 32 bytes |
| +0x7000 | - | 0xC00 bytes, cleared at start (sprite extras?) |

- **Interrupts (+0x188 enable, +0x18C status, write 1s to clear):** the
  kernel enables 0xC31. In the 16-bit chip's terms, bit 0 is vertical blank,
  bits 4/5 text and sprite under-run, bit 10 frame-buffer under-run and bit 11
  TV frame end. The handler (line 27) programs the next frame on bit 0 and
  counts frames shown on bit 11, which the main loop waits on and the
  kernel's timers add up as 60ths of a second (50ths on PAL, strap at
  0xC0000000 bit 6). Ribbit raises both bits every field. +0x1F0 is the
  frame commit: the main loop writes 1 after sending the frame, and the
  bit 0 handler (0x35EA8) only signals the next frame while bit 14 reads
  clear. Ribbit reads bit 14 set from the commit until the field after next
  (a commit while it is set, which the handler makes, changes nothing): the
  main loop runs 30 times a second and sees two fields counted each time.
  How long the hardware's flag stays set is UNKNOWN; this matches the
  start-up's animations on a recording (a new picture every 4-6 fields).
  With bit 11 every field and no busy flag everything ran twice as fast;
  with bit 11 every second field (before) the timers ran at half speed.
- **CPU speed:** the screens that load the next one wait on the CPU, so the
  instruction rate sets their length. Ribbit runs 27 million instructions a
  second, fitted to the warning screen's length and Mickey Says' load
  ("Speed", "CPU clock"); the frame-timed screens after the warning run
  short of the recording whatever the clock (see there). Still off (noted
  at 18 million): the Zippity logo forms about 0.4 s
  late and stays about 0.5 s less (ears and logo together 2.06 s against
  2.23 s); the mat demo is 10.1 s against 11.4 s; at the Sign In menu the
  names come about 1 s after the buttons (0.1 s on the recording) and blink
  every 1.0 s (0.6 s, with a fade). Neither the timer clock nor the sound
  unit's beat counter changes those two.
- **Video DMA (+0x1C0/+0x1C4/+0x1C8):** each frame the kernel copies its
  palette (0x400 words from 0x001BE540) and sprite list (0x31F words from
  0x002D314C) into the chip. Writing the count starts it; it reads 0 when done.
- **Frame buffers:** the chip renders into memory, PPU-enable bit 7 being
  frame mode in the 16-bit chip. The kernel rotates three 0x96000-byte buffers
  (640x480 at 16 bits): +0x1E0 is drawn into, +0x1E8 shown, and the 0xD0800000
  engine is given the drawn one (source and destination, +0x0C = 0x642;
  perhaps a scaler or TV encoder). Ribbit draws the picture when the kernel
  hands the chip the frame's sprite list (the video DMA, about 1100
  instructions into each frame) and shows it until the next one. While it
  loads a screen the kernel sends no frame for many fields (its frame
  handler still runs, but for short stretches with interrupts held off
  around NAND reads), and the TV keeps the last picture, though the
  kernel is already decoding the next screen's pictures into the sprite
  cache the shown one came from. Drawing that memory instead showed garbage
  between the start-up screens (credits to mat, mat to Sign In), which
  recordings do not. The buffers start filled with 0x8080. Their
  pixel format (PPU-enable bit 8: the 16-bit chip's "RGBG") is UNKNOWN; the
  research tool draws from the layers directly.
- **Palette:** 2048 16-bit entries (0RRRRRGGGGGBBBBB; bit 15 set:
  transparent, as in 0xFC1F, magenta, which starts each sub-palette). The
  menu's sprites take colours from 0x200 on, text layer 1 from 0x300. The
  kernel loads per-screen palettes by DMA: 0x80 entries to 0x200 (the top
  band, recoloured for each screen), 0x10 to 0x2F0 (text) and 0x100 to 0x300
  (the background).

### Text layers

Text layers 1 and 2 have the same registers at +0x40 and +0x58 (scroll x and
y, attributes, control, tile map, attribute map) and picture bases at +0x80
and +0x84. The system menu draws its background on layer 1; Mickey Mouse
Clubhouse draws its clubhouse on layer 2 (attributes 0x00C0A073: the same
format, priority 2). Layers and sprites are drawn by priority (attribute
bits 13-12), 0 first: at each level the text layers, then the sprites.

The menu's background is a 640x480 picture at 8 bits per pixel.

- **Attributes (+0x48 = 0x00C08073):** bits 1-0 depth (3: 8 bits), 5-4 tile
  width and 7-6 tile height (8 << n: 64x16), bits 23-22 the palette bank (3:
  colours 0x300-0x3FF). The bank bits are inferred from this one value.
- **Control (+0x4C = 0x2A):** bit 3 on. Bit 1 (attributes from the register,
  not a map) and bit 5 are as in the 16-bit chip.
- **Tile map (+0x50 = 0xF8002F80, on-chip RAM):** 32-bit entries, 16 per row
  (pages of 1024x512), each the tile's byte offset from the base at +0x80
  (0x400, 0x800, ...: 30 rows of 10 tiles), 0 for no tile.

**16-bit layers.** Control bit 7 makes a layer's pixels 16-bit colours (no
palette; tiles with entry 0 are none), whatever
the depth bits say about 8-bit layers:

- **Depth 1: RGB 5:6:5.** The Disney Princess cartridge's menu (control
  0xAA, attributes 0x00408071). Drawn as RGB 5:6:5 it matches a recording
  of a real unit (YouTube oI6xRa59rnM: blue at the top fading to pink and
  lavender); as RGB 5:5:5 it is magenta and black, as YUV green rings.
- **Depth 3: YUV 4:2:2,** a pair of pixels in 4 bytes, V Y0 U Y1 (BT.601,
  full range). The Princess and the Frog's menu (control 0xAA, attributes
  0x8073): as any RGB it is noise, with Y in the odd bytes a clean picture.
  The order of U and V is UNVERIFIED: V first gives a night sky behind the
  film's evening star, U first a red sunset; no recording of that cartridge
  on a real unit was found.

**Zoom.** Each text layer has four more registers, at +0x340 (layer 1) and
+0x350 (layer 2), which games write together with the layer's scroll, in
the order scroll, +0/+4, +0xC, +8: +0/+4 a centre on screen, +8 the step in
the picture per screen pixel (0x400 = 1), +0xC UNKNOWN (0 so far; perhaps a
rotation). Screen pixel (x, y) shows the picture's pixel (scroll x + (x -
centre x) x step / 0x400, scroll y + (y - centre y) x step / 0x400). The
system and most games leave the centre 0 and the step 0x400 (or 0, taken
as 0x400). Disney Princess's Cinderella's Dress Dash zooms its wall slowly
out (centre (392, 240), scroll (493, 288), step 0x437 to 0x470): with the
scroll alone it showed the empty rows under the wall, black; this way the
pillar and the wall's foot land where a recording has them. UNVERIFIED
beyond that game.

16-bit RGB 5:6:5 layers treat 0 as transparent, as 16-bit sprites do: the
same game's floor (layer 2, priority 2) is a full-screen layer whose upper
rows are all 0, over the wall on layer 1.

**Layers 3 and 4.** Little Einsteins' Rocket's Musical Mission uses a
third layer: scroll x at +0x20, attributes 0x00C0B0A3 at +0x30 (8 bits,
32x32 tiles, priority 3), control 0x2A at +0x34, tile map at +0x38 (on-chip
RAM 0xF8000F80) and picture base at +0x90, written with layers 1 and 2's
(+0x80/+0x84, the same value). That is the 16-bit chip's text layer 4
(0x7008-0x700F; two words, written 0, sit between the scroll and the
attributes) with its segment at 0x7024. It holds the seabed's front rows;
Ribbit drew only layers 1 and 2 and left them out. Layer 3 (+0x00, base
taken to be +0x8C) has not been seen in use. No zoom registers are known for
either.

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
| 6-7 | Attributes: bits 1-0 depth (2/4/6/8 bits), 2 x flip, 3 y flip, 5-4 width and 7-6 height (8 << n), 11-8 palette, 13-12 priority, 14 blend (below), 15 palette bank bit 1 (below) |
| 8 | Bits 6-0: picture address bits 22-16; bit 7 palette bank bit 0 |
| 10-15 | Words written from the corner converter |
| 9 | Bits 5-0: blend level (with attribute bit 14, below); bits 7-6 UNKNOWN |
| 16-31 | Four words, bit 31 set in every sprite seen but one: with it clear the sprite is a colour fill (below) |

- **Pixels** are one bit stream, most significant bit first: the 6-bit Zippity
  logo only comes out right this way.
- **0xD0500358** is text layer 2's zoom step (see Text layers), not a
  sprite offset as once thought: the system sets it to 0x400 (no zoom)
  before Mickey Mouse Clubhouse.
- **Picture number 0** (bytes 0-1 zero) is no sprite, whatever byte 8 holds,
  as in the GPL162xx. The kernel leaves old entries at the end of the list
  with bytes 0-1 cleared but byte 8 kept. Drawing them put pink boxes on the
  controller screen and specks under the LeapFrog logo and beside the sock.
- **Colours** are palette entry 0x100 x bank + palette x 16 (rounded down
  to the depth's size) + the pixel. The bank is two bits: attribute bit 15
  and byte 8 bit 7. Found by matching recordings: the menus' background art
  and text are bank 2, the buttons and game pictures bank 1, Pooh's picture
  on the game-select ring bank 0, and Mickey Mouse Clubhouse's characters
  bank 3 (with banks 2 or 1 they come out black or in false colours).
- **16-bit pixels:** a sprite of depth 8 with palette bit 0 set has 16 bits
  per pixel, in one of two formats:
  - **Palette bit 1 also set** (0x23F3, 0x33F3): each pixel is a colour, RGB
    5:6:5, 0 transparent. The game-select ring's bubbles, the menus'
    buttons and the games' pause panels: smooth blue gradients this way (a
    pause panel's edge is 0x01DF, its face 0x067F to 0x5EFF); as RGB 5:5:5
    with bit 14 read as a palette flag they came out banded, with false
    colours where bit 14 happened to be set.
  - **Otherwise** (0x29F3, 0x99F3): bits 7-0 are a colour from the bank and
    bits 13-8 an opacity (0-63); bit 14 is set in every pixel seen. The
    menus' background art, a 64-pixel grid of these sprites, is blended over
    text layer 1 this way, and recolouring entries 0x200-0x27F gives each
    screen its colour. The ring's reflections under the bubbles fade from
    0x19 to 0 downwards, faint as in a recording (taken as solid, they were
    the "wide strips" once mistaken for the game pictures). Colours whose
    palette entry has bit 15 set are transparent (in The Princess and the
    Frog's menus they had drawn magenta lines; in the system menus, specks).
  UNVERIFIED beyond these screens.
- **Blending:** attribute bit 14 blends the whole sprite over what is below
  at the level in byte 9 bits 5-0 (0-63; the kernel's sprite routine at
  0x2F2B8 writes it, 0x3F on most sprites). The games' pause screens dim
  the picture with a 64x64 sprite of one dark colour (0x10A2) stretched over
  the screen at level 0x2A, as recordings show (the scene still visible,
  darker, behind the panel). Byte 9 bits 7-6 are UNKNOWN.
- **Count:** only the first (control bits 15-8) x 4 entries are drawn; the
  rest of sprite RAM holds stale entries from earlier screens.
- **Order:** priority 0 to 3, and within a priority later sprites on top.

- **Colour fill:** bytes 16-31 are four words with bit 31 (byte 19 bit 7)
  set in every sprite of every screen checked (system menus, the four
  built-in games, both cartridges) but one: Rocket's Musical Mission's
  water, a 64x64 sprite of picture address 1 that the corner converter
  stretches over the top 330 lines, whose words are 0x000890F8,
  0x0008508A, 0x00083862, 0x00083862. Read as 0x00RRGGBB they are blues
  from light to dark, and a recording of a real unit (YouTube 3-AsXrrGKW8)
  shows that water light blue at the top and darker below. Drawn as a
  picture, address 1 is the kernel's code: the top of the screen was noise.
  Ribbit fills such a sprite with the four colours, taken as the corners
  in the converter's order (top left, top right, bottom right, bottom
  left) and shaded between them, at the sprite's blend level. Whether a
  real picture would be tinted by them, and the corner order, are
  UNVERIFIED.

**Corner converter (+0x300-0x330):** for each sprite the kernel writes four
corners, x then y (+0x300/+0x304 to +0x318/+0x31C, the same centred
coordinates; a 64x64 sprite gives (-275, 61), (-212, 61), (-212, -2),
(-275, -2)). It then reads five halfwords (+0x320 to +0x330) into the
sprite's halfwords 1, 2, 5, 6 and 7. This looks like the 16-bit chip's
"virtual 3D" sprites (PPU-enable bit 9, set). What the hardware returns is
UNKNOWN. The kernel shrinks sprites with it: a 64x64 sprite sent as four
equal corners is hidden. Ribbit returns its own encoding, which only its
renderer reads: the first corner (halfwords 1 and 2: the sprite's x and y,
what the kernel would have stored without the converter), the third corner
(5 and 6) and 0x8000 (7); its renderer scales the sprite to that rectangle.
Skewed quads are not handled. The games keep their own positions for their
logic (The Princess and the Frog's frog collects mushrooms with this
encoding).

**Maths unit (0xF6004000, "Fixed-point maths unit" above):** the kernel
places sprites through its matrix product (0x30614: each corner transformed,
then sent to the corner converter). Unmodelled, its results read 0: the
game-select ring's bubbles and titles all went to the converter as one point
at the top-left corner, leaving only their reflections (the "wide strips")
and a patch of noise beside the logo.

With the controllers idle (0x3FF), choosing a player gives the screen the
recording shows: the name's button grows wide at the top, with Learning
Path, back, cancel and OK below. (Earlier runs sent 0, every button held, and
showed a large panel instead: controller bit 4, the Home button, opens it.
It is the pause panel, RGB 5:6:5 over the dimmed picture, as recordings
show it.)

Choosing OK on the player screen (right, then choose) logs "Already ran
tutorial for player..." and goes on to the game-select screen as in the
recording: the Zippity logo, the Playhouse Disney ears and a ring of game
pictures in bubbles that turns, the front game's title below it. The
bubbles are placed through the maths unit and shrunk through the corner
converter (64x64 to 39x39 at the back of the ring), with fading
reflections; drawn this way the ring matches a recording (tall bubbles,
characters and all).

Choosing the front game (Mickey Mouse Clubhouse) loads its kernel, which logs
"Loading with init_tv as: 0", mounts the file system again, reads the
player's save (MMCH.P01: "MMCH: player_name: [MATTHEW]") and shows the game's
menu: Mickey, Minnie and Goofy in the clubhouse with "Mickey Says", back and
"Goofy's Fancy Dancing", and the player's score. The research tool draws it
as a recording of a real unit shows it.

Mickey Says (choose, then Level 1) shows the mat and waits for the player to
stand on it (foot bits 8/9), then "LOADING...", then the game: Mickey on his
pedestal in the clubhouse, the mat drawn at the bottom and the score, which
matches the recording closely (a thin black line once seen across the left
window is not there with the rest of this model). Its scripted opening (Mickey talks, another load, Minnie comes in)
runs without input.

Played with scripted input against recordings of real units (a new player,
standing on the home spot between steps): Goofy's Fancy Dancing (vbWOv7rICVU:
the dot shows, Goofy waits in a listening pose, a step on that colour makes
him dance and adds 3; a step during the dot's announcement is ignored and
he keeps waiting, so a script must step again), Handy Manny's Tool Box Beat
(VuQE81MjPLk), Tigger & Pooh's Darby's Honey Pot Jam (W0vh0j5lwM8), Little
Einsteins' Rocket's Musical Mission (marching drives the rocket, the
instruments it meets count up) and June's Twist 'n Twirl (3-AsXrrGKW8), and
Disney Princess's Snow White's Dancing Delight (O2neYHag2ug: step on the
colour of the thing Dopey shows). Each scores, ends, saves its file
(`MMCH.P01`, `LE.P01`, `MDP.P01`...) and returns to its menu; none hangs.
The menus' extra props on the recordings (Handy Manny's playground, Tigger
& Pooh's shelf toys) are earned with points: those players had 52 and 31,
a new player's menus are plain, and after one game of Tool Box Beat (8
points) Handy Manny's menu has his tools on the table.

Also played to their ends: Cinderella's Dress Dash (oI6xRa59rnM: the beads
collected become the count for the dress, where the bopper moves the bead
and green places it, the count running down to 0 and Cinderella coming in,
as recorded), Ariel's Fireworks Display (bgqzD6vLouY), and The Princess and
the Frog's Hop Hoppin' In The Bayou (marching hops along the path; at the
letter crossings a step on the spot with the letter scores, here 5), Tiana's
Magical Wedding Dance (6) and Orders Up. No recordings of The Princess and
the Frog on a real unit were found.

Explained (game behaviour, not emulator faults):

- **Louis's Ragtime Jam Band** (The Princess and the Frog) is paid for with
  what the other three games earn: the counter at the top right. Each of
  them adds its score to a count in the game kernel's data (RAM 0x2D4E94,
  capped at 99, at 0xF7C and 0x198E0 among others); the band takes one
  from it (0xBA7C). At its start (0xA404) a count of 0 makes the game play
  Louis's voice line (sound 0x232), wait for it to end and leave (the loop
  at 0xA4C8 sees state 0 and clears its run flag): about 13 s of intro,
  score 0, no end panel, whatever the input. A new player, or one who has
  only entered the band, has 0. With a count above 0 (5 after Hop Hoppin'
  In The Bayou in these runs; also checked by setting it) the band plays
  on, the counter showing it.
- **Dress Dash's "DCompFail 6"** (Disney Princess's kernel, its image
  decompressor at 0x39300, called from 0x3A010): the expected size is
  worked out from the kernel's own tile table (static data in KERNEL.BIN,
  RAM 0x90E6C: six 64 x 64 tiles of attribute 0x09F0, 0x1000 pixels each,
  0x6000 in all, doubled to 0xC000 bytes for the two-byte mode). The input
  length is the next image's file offset minus this one's (0x2B1EB2 -
  0x2AF5C2 = 0x28F0 bytes). The stream there (identical in RAM and in the
  cartridge's sprites file) fills 0xC000 bytes after 0xB3F of its 0x1478
  halfwords and goes on to 0x16300 bytes (0xB180 pixels, as a 320 x 142
  picture would), so the decoder gives up with error 6 (output full, more
  than 0x7F halfwords left) twice, re-reading the file in between. The
  kernel's table and the cartridge's data disagree, and a real unit has to
  fail the same way: the game logs "Error decoding image, first tile:
  0x1137" and plays on without that picture.


Speed: the research tool runs about 37 million instructions a second on one
core of an i9-13900K, with its jump log and per-register bookkeeping; the
real CPU clock is UNKNOWN (Ribbit assumes 27 million a second; "Speed").

## Research tool

`crates/ribbit-zippity/examples/zipboot.rs <nand image> [instructions]`, with
environment variables:

| Variable | Use |
|---|---|
| `ZIPBOOT_DUMP=dir` | Save RAM (`ram.bin`), on-chip RAM and the logs below at the end |
| `ZIPBOOT_FRAMES=n` | Save the picture every n instructions (`frame_<millions>.ppm`) |
| `ZIPBOOT_EVERY=n` | Save RAM every n instructions |
| `ZIPBOOT_SAVE=file:n`, `ZIPBOOT_LOAD=file` | Save the machine after n instructions; carry on from such a file |
| `ZIPBOOT_PAD=state:from:to[;...]` | The controllers' 10-bit state (hex) over instruction counts |
| `ZIPBOOT_PRESS=port:mask:from:to[;...]` | Flip GPIO input bits (port a, b or c) for a while |
| `ZIPBOOT_WATCH=lo-hi`, `ZIPBOOT_WATCH_FROM=n` | Log I/O accesses in that range, with the values read (`watch.log`) |
| `ZIPBOOT_RWATCH=lo-hi` | Log CPU writes to memory in that range (`rwatch.log`) |
| `ZIPBOOT_JUMPS[=n]` | Print the last n jumps (600) |
| `ZIPBOOT_WAV=file` | Save the sound as a WAV file |
| `ZIPBOOT_SPULOG` | Print each sound channel start (mode, address, phase, volume) |
| `ZIPBOOT_POWER_HOLD=n` | Hold the power button for n instructions (with `ZIPBOOT_PRESS=b:20:...`: the test mode) |
| `ZIPBOOT_RESET` | Clear the user data first (`Zippity::factory_reset`) |
| `ZIPBOOT_SPRITES` | Print the sprites in use at the end |

`zipclean <nand> <out>` writes a copy of a dump with the user data cleared.
