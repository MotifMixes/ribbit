# LeapFrog ClickStart

The ClickStart: My First Computer (2007) is a TV console with a child's
keyboard and mouse. Ribbit emulates it in `crates/ribbit-clickstart`
(EXPERIMENTAL), on the app side `crates/ribbit-frontend/src/clickstart.rs`.

## Hardware

- **Chip:** Sunplus SPG28x ("QL8041C" on the die, per MAME): a 16-bit unSP
  1.0 CPU at 27 MHz with the SPG2xx video, sound, I/O and DMA units.
- **System ROM:** 8 MB (`clickstartbios.bin`, CRC32 7c833bd0). Its header
  reads `chksum:26015916 ver:"2.2.1" user:"" body:"SPG240"`.
- **Cartridges:** 4 MB ROMs. With one in, the cartridge takes word addresses
  0x000000-0x1fffff (reset and interrupt vectors included), and the system
  ROM's upper half stays at 0x200000-0x3fffff (as in MAME's driver).
- **Byte order:** the dumps store each word low byte first. MAME loads the
  system ROM byte-swapped only because it treats the unSP as big-endian.
- **Keyboard and mouse:** the keyboard talks to the console by infrared,
  arriving on the SPG's UART at 3200 baud; the mouse plugs into the
  keyboard (box and manual). See "Keyboard and mouse" below.
- **EEPROM:** a 24C08 (1 KB I2C) on the board holds the players. The
  system ROM bit-bangs it on port C, bit 0 SDA and bit 1 SCL (see EEPROM
  below), not through the SIO as MAME's notes suggest.
- **SIO** (port C bits 11 SDA, 12 SCK, per MAME): a second sound chip
  that the system streams compressed sound to (see "The SIO sound chip").

## What works

- The system ROM boots to its Player Profile screen, which matches MAME's
  picture. A name can be typed, and the mouse moves the pointer.
- Players are saved (the 24C08) and come back at the next start; save
  states (see below).
- All 15 known cartridges (MAME's `clickstart_cart` list, all marked as not
  working there) reach their title screens or menus within 30 seconds.
- Sound: the SPG2xx sound unit (8-bit, 16-bit and ADPCM samples, envelopes).
  The system ROM's music starts at the same moment as in MAME (12 s after
  power-on), at a similar level. The SIO sound chip's A1600 sounds (the
  "Click" jingle, the built-in activities' speech, much of the cartridges')
  and its S720 speech (Player Profile's line, Finding Nemo's Grab Shell
  and the other cartridges' speech) play (see "The SIO sound chip").
- Speed: about 13x real time on one P-core (an interpreter).

## Keyboard and mouse

**Packets.** The system ROM's UART interrupt (0x3f0554-0x3f0638) reads a
byte, inverts it, and takes a 00 as a packet's start; then come 5 bytes
and their sum (low 8 bits; a mismatch drops the packet). Ribbit sends:

- byte 1: bits 0-3 the four arrow keys, held (bit 0 Down, 1 Left, 2
  Right, 3 Up), bit 4 the mouse button; 0x3f053c hands bits 0-4 to the
  activities. With nothing held, bits 5-6 must not both be clear (a 00
  byte starts a packet, 0x3f0561); the status call 0x3f04d9 turns them
  into a value nothing reads, and bit 7 into a flag Thomas and Friends
  answers with a sound (perhaps the keyboard's low battery). Ribbit sends
  0x60 | the held bits;
- bytes 2 and 3: two key slots (low 6 bits, 3f = no key), bit 7 of each
  the sign of the mouse's x and y. Each slot's change makes events in a
  16-entry ring at 0x0007 (the old code = key up, the new one | 0x80 =
  key down; read by 0x3f04b7);
- bytes 4 and 5: the mouse's x and y movement. Not negative: value + 1;
  negative: value + 256 (with the sign bit). Taken only when the two key
  slots aren't both 3f, so a mouse packet puts 3e in slot 2 (as MAME).

Key: `00 60 code 3f 01 01`, key up `00 60 3f 3f 01 01`. MAME sends 01 as
byte 1, which holds Down for good (Ribbit did too until 2026-11-15: a
menu's "still there?" timer never ran, Finding Nemo 0x10b915), and 2f in
slot 3, which is the Hint key held down for good. Mouse: `00 60|10
3e|sx 3f|sy x y`. Arrow: `00 60|bits key 3f 01 01` (a held key kept). MAME's `(d & 0xfe) + 1` loses the low bit, so a
movement of 1 unit did nothing and -2 moved -1; Ribbit sends the exact
value. A mouse packet's 3e in slot 2 also lets go of a held key (left
as is: the app sends a key's packets before the mouse's).

Checked headlessly: 10 mouse packets of 16 units each, sent 1, 3, 6, 15
or 40 frames apart (also right after typing), all move the pointer the
full 60 pixels; none is dropped.

**Key codes.** The ROM's key function (0x1bac5e, behind every activity's
key reading; the carts have copies) turns codes into characters and
actions. Checked by running it (`CSRUN_BREAK=1bad98`, its return):

| code | ROM gives | key |
|---|---|---|
| 01-1a | 'a'-'z' ('A'-'Z' with Caps on) | letters |
| 1b-24 | '0'-'9' | digits |
| 25 | '?' | ? |
| 26 | '.' | . |
| 27 | ' ' | space |
| 28 | c8 | Enter: the system's text entry (0x00d594) takes it as "done" and hands the text on; most cartridges compare it (Bob the Builder in 50 places); the Player Profile screen ignores it |
| 29 | ca, flips the case flag (0x12d3) | Caps |
| 2a | c9 | Backspace: deletes a name's last letter |
| 2b, 2c, 2d, 2e | e6, e8, e9, ea | the four yellow picture keys (smiley, note, boat, cat; which is which unknown). Activities answer them with a fun reaction: Dora the Explorer plays one of three clips per key (0x18a9cd-0x18aadb), Thomas and Friends a sound per key (0x01e7f5); many just ignore them (Finding Nemo 0x1090c0). Not the arrows (bits in byte 1) |
| 2f | e7, sets "hint asked for" (0x11e1) | Hint (the manual's Hint button) |
| 30 | cb | Pause (the one yellow key left). The system hands it to the activity as a message (0x00fa55, event 8); Thomas and Friends opens something in one state (0x007fe6) or plays sound 20. Headless, pressing it in Finding Nemo's Grab Shell and on Thomas's and Toy Story's menus paused nothing |
| 31-3e | nothing | |

MAME's digit codes (0=25 ... 8=2f) are wrong except 9 (24). The name
"A0", Backspace, "8" gives "A8" on the Player Profile screen.

**The real keyboard** (a photo, 2026-11-14; the system's own keyboard
picture, shown before an activity with the key to press lit, has the same
layout) has 52 keys: 1-0, QWERTY letters, '.', '?', a red space bar,
Hint, eight yellow keys (a smiley, a back arrow, a music note, Pause, a
boat, a cat face, Enter, Aa) and four arrow keys in a pad. Codes 01-30
are 48 keys: the arrows have no code. CONFIRMED from the code
(2026-11-15): they are byte 1's bits 0-3, held, not events. Finding Nemo
(0x1104c1) moves an object by +32/-32 in x on bits 2/1 and by +16/-16 in
y on bits 0/3 (positions in 1/8 pixel, y clamped to 0x2f8-0x6e0, so y
counts down: bit 0 is Down); Dora the Explorer (0x18ccd1) drops an
opposite pair held together and returns 1-4 for bits 1, 2, 3, 0 (left,
right, up, down); several cartridges test bits 0-3 together (0xf) as "any
arrow". CONFIRMED against a real console (2026-11-15) through the
developer mode below: a capture of a real ClickStart shows Left opening
the build-info screen and Down the country screen, and headlessly bit 1
opens the build-info screen and bit 0 the country screen. So bit 1 is
Left and bit 0 is Down; bit 2 is Right (Nemo, Dora) and bit 3 Up by
elimination. 28 = Enter, 29 = Aa, 2a = the back arrow, 30 = Pause and
2b-2e = the picture keys fit the photo.

**Developer mode** (CONFIRMED headlessly, 2026-11-15, and in a real
console's capture, "LeapFrog ClickStart - Hidden Developer Cheat
Username" on YouTube): a player named ROKSTER (0x28ef03 compares the name
with the string at 0x00bc65) sets 0x12d2 when signed in. Then the home
screen's key handler (0x28f3f0-0x28f495) takes 'c' (0x28f43f: 0x282a4c
writes 0x00 to EEPROM bytes 0x000-0x3fe one at a time, then a sound),
's' (screen 0x0c, the shutdown), 'l' (screen 7, Log-In), '1' and '0'
(one or ten bones, per the capture), the Left arrow (bit 1: screen 0x0d,
the build-info screen) and the Down arrow (bit 0: screen 9 with
sub-page 2, the country screen). Screens go through 0x297e16(n); the
current one is at 0x1204, screen 9's sub-page at 0x1347 (1 Settings, 2
the country screen, 3 and 4 Player Profile for a new player). The
arrows are read through 0x3fff14 (0x3f053c, byte 1's bits).

Headlessly: type ROKSTER on Player Profile, click Update, click the name
on Log-In (the pointer starts at the top right there), wait for the
intro to reach the home screen (about 26 s), then press the arrow:

```
CSRUN_EEPROM=rok.bin CSRUN_MOUSE=<to the name, click> CSRUN_KEYS="4000:a2;4010:a2:u"
```

The build-info screen reads "bROK Alpha 1.08 Feb 28 2007", "ROKSTER"
(the player), "PN 152-11811", "3LD bROK", then "Name BaseRom", "PN
152-" (the cartridge slot's header; see "A real console's second
capture" for what the real one shows). The strings are at 0x009a9e, one
character per word.

App keys: letters, digits, space, '.' and '/' (for '?'), Enter, Backspace,
Caps Lock; Home or Pause = Pause (30), End = Hint (2f); the arrow keys =
the arrows; Page Up, Page Down, Insert, Delete = the picture keys 2b, 2c,
2d, 2e (which picture is which is unknown). Esc, Tab, `, the F keys and
Ctrl+R/Ctrl+O stay the app's.

**Key sounds.** The system's text entry (0x00d61c) plays a sound through
0x27a074 for a key it takes when its key sounds are on; the picture keys
and Hint are left to the activity. Each activity picks its own: Dora
three clips per picture key, Thomas a sound per key. No table of which
key makes which sound on the real console shows in the code.

**Mouse.** The ROM's pointer code (0x3f03cc, called by the activities
through 0x3fff0c) adds each movement unit times a speed (0x002e/0x002f,
set to 12 at start-up by 0x3f0431/0x3f0450, range 1-20; no cartridge sets
another) to a position in 1/32 pixels (0x002c/0x002d), clamped to 0-319
and 0-239 pixels; y counts up the screen. No acceleration. So 32/12 units
are one pixel (`MOUSE_UNITS_PER_PIXEL`). CONFIRMED (2026-11-15): no
cartridge calls the speed setters (0x3fff0e, 0x3fff10) or reads the raw
movement (0x3f0540); all 13 move their pointers through 0x3fff0c.

But the screens keep the pointer on whole pixels: the system's pointer
handler (0x296f7d) reads it through 0x3fff0c, keeps it inside the screen
by the pointer sprite's size, and puts it back through the setter
(0x3f0380, `x << 5`) every other frame. So a packet's fraction of a pixel
is lost, rounded down (measured: 1 or 2 units move it 0 pixels, -1 or -2
one pixel left, 3 one right, -3 two left). Until 2026-11-16 the app sent
32/12 units per PC pixel, so a slowly moved mouse (a pixel or two a
packet) crept left and down and barely came back right or up: "the mouse
becomes insanely difficult to control". `ClickStart::mouse_pixels` now
takes whole pixels and sends the units that land in the wanted pixel
whether or not the setter drops the fraction first (from the pointer and
the movement taken but not added, 0x0026/0x0027), and the app keeps the
remaining fraction of a PC pixel for the next packet. Test
`tests/mouse.rs`: 1 to 60 pixels a packet, four directions, there and
back, exact. The app also forgets the cursor's position when it leaves
the window (`CursorLeft`): coming back in elsewhere was one long move, to
an edge.

**The pointer jumped by itself** (fixed 2026-11-16): typing a letter on
Player Profile put it at the left edge or a corner, and on Log-In it
went back to the top a few frames after every move down. The handler's
sprite size (0x2791ef/0x27920b, through 0x2788c0) came back as garbage
(0x6d04, 0x3d39) because a far read (0x28508b: `[3d2f] = segment`, then
`ds:[r4]`) read the wrong bank: the CPU took the data segment write into
SR only after taking an interrupt, so the interrupt pushed the old SR and
RETI brought the old segment back. Now a write to 0x3d2f goes into SR as
the instruction ends, before interrupts are looked at (as MAME, whose
`set_ds` is immediate).

**Wrong notes** (the same race, fixed 2026-11-16): the sound driver's
reads of its sequences went through the same far read, so now and then a
channel started with a garbage pitch or sample. 123 Click, 180 s from the
same saved state (sound unit channel starts): before, 17 of 3326 starts
had a rate of 0 or over 30 kHz (205486, 256122 Hz...) and 32 sample
addresses were used once or twice (mid-sample starts); after, 0 of 3362
and 12 (the activity's own one-off sounds and the restarts listed under
"Open questions"). Against a clean render of the Log-In music ("Sign
In", 30 s) the pitch profile (CQT, C1-C8) matches within 2 dB below
4 kHz, and the mean chroma cosine is 0.94 before and after (that loop
was rarely hit).

## EEPROM

The BIOS's I2C routines sit at 0x3f0a01-0x3f0c55 (port setup, START, STOP,
byte out, byte in, ACK in, ACK polling). `eeprom.rs` is a 24C08: device
byte `1010 A2 P1 P0 R/W` (A2 taken as tied low), word address, a 16-byte
page buffer written at STOP, sequential reads. Port C's pins are wired to
it in `Bus::gpio`: an output pin drives its level, an input pin reads high
(the pull-up) unless the chip pulls SDA low. Writes are instant (the real
chip NACKs for up to 5 ms; the BIOS polls the ACK, so either works).

On a blank chip (0xFF) the system formats it: a header starting `a5` and
two copies of its 128-byte record, at 0x000 and 0x380, each ending in a
checksum. CONFIRMED (2026-11-15) for all-0x00 and random garbage too: the
same record is written (0x000-0x07f and 0x380-0x3fe; the rest of a
garbage chip stays as it was), and the first screen is Player Profile (a
name to type). Only the developer mode's 'c' erases everything: it
writes 0x00 to bytes 0x000-0x3fe (headlessly the file then reads `a5`
and 1023 zeros), and the next start formats the chip as a blank one and
shows Player Profile. That a "cleared" chip is 0x00 says nothing about a
factory-fresh one; both go the same way. It has a
"Delete Player? Yes No" box (0x1aeeb7), a Delete button (0x09db18) and a
parents' password: "Type the Password", compared with PARENT (0x298a70).
INFERRED from the strings and code, not reached headlessly; the Log-In
screen on a real console has a Settings tab, probably where they are. A player's name is at 0x0b in both (`02 'ANNA'` after creating
"Anna"); byte 0x03 changes at every start.

The app keeps it as `clickstart.eeprom` in the save folder, one for all
games: read at start, written once a second while it has changed and when
play stops. Reset keeps it. `ClickStart::eeprom()`, `set_eeprom()`,
`eeprom_dirty()`, `eeprom_saved()`.

Checked headlessly: `CSRUN_EEPROM=file` loads the file into the chip and
writes it back at the end. A blank chip, the name "anna" typed, the mouse
on Update and a click: "Profile Saved", and the name is in the file. A new
run with that file shows Anna on the Log-In screen.

## The SIO sound chip

The system ROM's driver (0x00fa90-0x00fbd8; every cartridge has a copy,
e.g. Finding Nemo 0x116448) sends a 16-bit command and 15 data words per
transfer and reads a 16-bit status back by bit-banging port C (0x3f0c90:
bit 11 turned to an input, a bit read before each rise of bit 12, the top
bit first). Commands: 6011, 6012 and 680n at start (n perhaps a volume),
7800|n a clock trim (at start it steps n down from 0x3e8 until port C bit
10 toggles fast enough, 0x3f0c61), 6020 before each sound, 7000|n to play
one, command 0 with the next 15 words of it, 6040 to stop. Status: 0x10
playing, 0x40 wants the next 15 words, 0x01 finished (the driver then
sets its "done" flag, 0x1301; Nemo 0x1201).

A sound starts with its length in bytes, those four included (two words,
low first). The n of 7000|n is the format, one of Sunplus's SACM codecs
(identified 2026-11-15 with Sunplus's PC codec DLLs, 32-bit, run from a
scratch folder for research only; Ribbit never calls them):

- **7002: SACM-A1600.** A header word 0x8004-0x8009 (mode 4-9; the DLL
  also takes 0, 1 and 2, an older layout no ROM here uses), then frames of
  10, 12, 14, 16, 20 or 24 words for each 128 samples. The ROMs use modes
  5 (the system, Toy Story, Dora, Learning Carnival) and 6 (Toy Story,
  Thomas, Disney Princess, Cars, Kai-lan). A1600.DLL's `a1600_dec` decodes
  all of them cleanly (the system's jingle notes are pure tones, 94% of one
  sound's energy within 1-2 kHz; the speech has syllables and silences);
  as A1600 the 7003 sounds are clipped noise.
- **7003: SACM-S480/S720,** a CELP built on Federal Standard 1016: S4872.DLL holds
  FS-1016's line spectral pair tables (100, 170, 225, 250, ... Hz; 3, 4,
  4, 4, 4, 3, 3, 3, 3, 3 bits) and decodes the sounds as speech (voiced
  stretches with formants, unvoiced hiss, clean silences). A frame is 144
  bits (9 words) in 4 subframes; its first byte's top nibble is 1 for 240
  samples (S480, 4.8 kbit/s at 8 kHz) or 2 for 160 (S720, 7.2 kbit/s). All
  the ROMs' 7003 sounds are S720. Many start with the same silent frame
  (e727 b6dd 35b5 0d00 0003 061a a000 ...), which looked like a header.
  Its length counts only the frames (18 bytes each, not its own four), but
  the sounds are laid out, and sent, as if it counted them: every sound's
  last frame is cut to 14 bytes, the next sound's length in its place (136
  of the system's sounds and those of 5 cartridges checked). Ribbit plays
  the whole frames only, as the DLL does.

**S720's frame** (Ribbit's decoder, `S4872` in `sio_audio.rs`, written
2026-11-15 from FS-1016 and a study of S4872.DLL's frame decoder,
`s4872_dec_frame` 0x10003f70, its tables and its output; Ribbit never calls
it). It is not FS-1016's bit stream nor its excitation: only the LSPs and
the LPC synthesis are the standard's. The 144 bits are read from each
byte's low bit up, the first byte's top nibble (the mode: 2 = 40-sample
subframes, 1 = 60) skipped: the 10 LSP indices (3, 4, 4, 4, 4, 3, 3, 3, 3,
3 bits; FS-1016's tables in Hz, held as Q15 of 4 kHz), then for each of the
4 subframes a pitch delay (8 bits, or 5 in subframes 2 and 4: a step of
-16..+15 from the one before), a pulse pair (9 bits), a grid bit, a pitch
gain (4), a pulse gain (4) and two pulse signs (the second pulse's first).
- The delay code c is 13 + c/3 samples up to 191 (thirds of a sample,
  13-76 2/3), then c - 115 (77-140). A fractional delay interpolates 8
  past excitation samples (a 1/3-sample windowed sinc, Q15: -235, 1050,
  -3571, 12714, 26674, -5216, 1629, -383 and its mirror); where the delay
  is shorter than the subframe the vector repeats itself.
- The pitch gain is 0.075 x the code (0-1.125). The excitation is that
  times the adaptive vector, plus two pulses of the pulse gain's
  amplitude (0, 1, 3, 13, 35, 64, 98, 136, 178, 224, 278, 340, 418, 520,
  660, 870) and their signs, at grid + 2a and grid + 2b, where the pair
  code is the index of (a, b) among the pairs a < b of 0-29 in order (435
  codes; none of the ROMs uses the other 77). Each pulse repeats every
  delay samples (pitch sharpening). No stochastic codebook.
- The LSPs are interpolated from the last frame's to this one's (1/4,
  1/2, 3/4, all) per subframe and turned into a 10th-order LPC filter
  (Q11); its output, at most 2047, times 16 is the sample. No postfilter.

Ribbit does all of that in the DLL's integer steps except the LSP-to-LPC
conversion, which it does in floating point (the DLL's fixed-point one is
a little less exact: its coefficients are off by up to 16/2048). Against
the DLL's output (40 sounds from each of the system ROM, Finding Nemo,
Animal Art Studio, Scout's Puppy Pals and Thomas): 10-16% rms error
overall (26% at worst) and a log spectral distance of 0.4-0.8 dB, i.e. the
same speech: the small filter differences only shift its fine phase over
time. The Player Profile line is a voiced speaker (F0 about 150 Hz at
8 kHz) with syllables and silences (4.4 s).

**A1600's frame** (Ribbit's decoder, `sio_audio.rs`, written from the
DLL's frame unpacking, 0x10006990, and its tables): bits are read from
each word's low end up. 16 4-bit band scales come first (gain 1, 3, 6,
11, 20, 36, 61, 103, 172, 288, 479, 797, 1324, 2200, 3653, 6066). The 16
bands of 8 MDCT coefficients take an allocation each by scale, the
loudest first (the lower band on a tie), from the mode's list (mode 5:
6 6 6 2 1 1 ...). Each allocated band then has a 3-bit peak position;
the peak is its gain with a sign bit, and a table per allocation and peak
gives the other coefficients 0, 1, 2 or 4 bits: a level in 64ths of the
gain (4 bits: 1, 5, 9, 15, 21, 27, 35, 47; 3: 3, 12, 24, 41; 2: 3, 24;
1: 5) indexed by the code >> 1, its low bit the sign. The synthesis is a
128-coefficient IMDCT with a sine window and overlap-add, times -2 (the
DLL's fixed-point version: a 64-point complex FFT with pre- and
post-twiddles). A plain floating-point one matches the DLL's output to
0.3% rms (0.7% on a sound that clips). The first frame's block isn't
output, so a sound of F frames gives (F - 1) x 128 samples.

**Rate:** the chip plays A1600 at about 12.29 kHz (12288 is used), not
16 kHz. Against the real console's recording below, the jingle's tones
and the built-in activities' speech (the system's 255 A1600 sounds
searched by spectrogram; Scout's lines found at 73, 97, 115, 203 and
239 s, matching 0.86-0.89) fit best at 0.768 x 16 kHz, a little less at
0.766 and 0.770, far worse at 0.75 and 0.80. That makes mode 5 18.4
kbit/s. None of the recording's sounds is S720 (all 136 system S720
sounds searched by spectrogram at 0.768-1.15 x 8 kHz: best 0.60-0.74, all
at the same unrelated moment, where Scout's A1600 lines matched
0.86-0.89), so its rate isn't measured. Until 2026-11-16 Ribbit used the
codec's own 8 kHz (S720's 7.2 kbit/s is 144 bits per 160 samples at
8 kHz), which puts its speech's first formant where Scout's A1600 lines
have theirs (about 510 Hz; at 12288 Hz it would be 790; but Scout is an
adult man's voice and most S720 lines are children's and a woman
narrator's, so that comparison says little). Whether the 0.768 applies
(6144 Hz) is open: nothing ties A1600's rate to a slow chip clock (the
codec's 16 kHz isn't known to be what LeapFrog's sounds were made at), no
sound is in both codecs to compare, and voice statistics don't decide it
(the system's S720 voices at 8 kHz have a median F0 of 286 Hz, Scout's
recorded lines 242; a crude syllable count fits 6144 a little better, the
formants 8000). A real console's Player Profile line (no players saved)
should have settled it, but the second capture (below, 2026-11-15) has
none: its console runs an older system ROM, and no S720 sound of this ROM
is heard anywhere in its 554 s. All 674 S720 sounds in the system ROM
(found by their frames: a length that is a multiple of 18 and every
frame's first byte 0x2_) were searched by spectrogram cross-correlation
(40 log bands 100-3000 Hz, 10 ms steps) at 6144, 7000 and 8000 Hz, and
the first 127 also at 5000-16000 Hz: best 0.70-0.76, only for 1-1.5 s
sounds, at unrelated moments (chance level). The same search finds this
ROM's A1600 sounds in that capture at 0.80-0.975 (a 2.7 s Scout line,
0x2b0553, at 0.975; the jingle at 0.93). Its Player Profile screen
(0:20-0:27) and its 56 s intro play only the system music, where Ribbit
plays two S720 lines and seven. So the rate stayed 8000 Hz, unmeasured.

**12288 Hz since 2026-11-16** (GUESS, the chip's one output rate, as for
A1600). Players heard the 8 kHz voices as "at least half as fast as they
should be" ("His name is Scout... and your new computer!", the intro).
All 674 S720 and 307 A1600 sounds of the system ROM, decoded and
transcribed (Whisper) with their pitch (pYIN): at 8 kHz the 431 S720
speech sounds ran at a median 2.2 syllables a second (25-75%: 1.9-2.6;
"To play 123Click, go to the desktop and click on the 123Click icon."
8.8 s) with voices at a median 177 Hz (the woman narrator 140-165 Hz);
at 12288 Hz that is 3.4 syllables a second and 272 Hz (the narrator
215-250 Hz; the A1600 children's lines at their measured rate: 337 Hz).
And Player Profile plays its first line (0x2c1dae) again 10.3 s after it
started: at 8 kHz its two lines (4.4 s + 6.9 s) didn't fit and the
second was cut off every time; at 12288 Hz (2.9 s + 4.5 s) both end with
3 s to spare. 16 kHz would give 4.4 syllables a second and a 354 Hz
narrator. Still unmeasured: a real console's recording of any S720
sound (Player Profile with no players saved, or the intro of this ROM)
would settle it.

**In Ribbit** (2026-11-15, `SioAudio` in `sio_audio.rs`): the chip takes
up to 64 words ahead (asking for more while 15 fit), decodes A1600 as it
plays, resamples it to the sound unit's rate (linearly) and adds it to
both sides at 16/32 of full scale, from the recording's jingle to system
music loudness (rms 642 : 710; Ribbit now 0.90). "Finished" comes when
the last frame has played. S720/S480 is decoded the same way (12288 Hz),
at the same level, "Finished" coming when its last whole frame has
played. The system plays the "Click" jingle (0x3682ee 390 bytes and
0x3adb46 3246; 3.25 s later 0x3ad98c and 0x38c890 3846, cut by the
LeapFrog logo after 0.9 s) and an S720 line when Player Profile comes up
(0x2c1dae, 3960 bytes, 2.9 s; heard in a headless run from 12.9 s, its
loudness following the DLL's decoding of it). Finding Nemo plays S720
lines on its title and menu and through Grab Shell's Squirt intro (the
menu's right button, then Level 1).

Before 2026-11-15 the status read 0 (as in MAME), so no sound ever
finished: speech-led scenes went on only after timeouts, and Finding
Nemo's Grab Shell stayed on Squirt's intro for good (its game now
starts).

## A real console's recording

"ClickStart Quick Walkthrough" (YouTube, a capture card, 480p, with
sound; 2026-11-15), against Ribbit's headless run:

- Start-up: the "Click" logo for 5 s, white, the LeapFrog logo for 5 s,
  then the Log-In screen and its music 5.5 s after the LeapFrog logo
  appeared. Ribbit: the same 5 s and 5 s, music 5 s after the LeapFrog
  logo (frames sampled every 0.25 s; the capture every 0.5 s). The
  console was switched on three times; power-on to music about 13.5 s,
  Ribbit 12.5 s.
- The chip's jingle: its four sounds start 0.46, 3.26 and 3.68 s after
  the first in the capture (20.14, 20.60, 23.40, 23.82 s), 0.44, 3.24 and
  3.67 s in Ribbit; the last stops 0.85 s in there, 0.91 s in Ribbit. The
  first starts 0.64 s after the middle of the logo's fade-in in the
  capture, 0.50 s in Ribbit (the capture's sound may lag its picture).
  Strongest tones: 555, 675, 1023, 2044 Hz and 850, 1011, 1350, 1700 Hz
  in the capture; 551, 685, 1023, 2047 and 506, 1012, 1350, 1700 in
  Ribbit.
- Sound levels. Peaks in the
  capture: the chip's jingle 14134, the system music 5000-7000 (rms
  600-900), the noise floor rms 25; nothing clips. The music and the
  speech the activities play are both the SPG's ADPCM, so their ratio
  says nothing about Ribbit's x16, and the capture's level isn't
  absolute.
- Pictures: the "Click" logo with Scout, the LeapFrog logo and the menus
  match Ribbit's (the capture crops the edges). The real console had
  players saved, so it shows Log-In (players, Settings tab, Music button)
  where a blank Ribbit shows Player Profile. Before an activity the
  system shows its keyboard picture (Scout's face round the keys).

## A real console's second capture

"LeapFrog ClickStart - Hidden Developer Cheat Username" (YouTube
3wXI2Janwq4, a capture card, 554 s, with sound; 2026-11-15). It starts from a
blank console: "Click" and LeapFrog logos, a country screen (US and
Canada flags, OK), Player Profile ("Rokster" typed), "Profile Saved", the
intro, the home screen, the developer keys, a shutdown, the start-up
again.

**Its system ROM is another build.** Its build-info screen reads "Beta
1.1x Jan 1x 2007" (the digits blur), "ROKSTER", "PN 152-11757", "3LD
ROK", then Name, PN and ID empty. This dump reads "bROK Alpha 1.08 Feb 28
2007", "PN 152-11811", "3LD bROK" (header ver "2.2.1"). The capture's
console is the older one, so where the two differ below, the ROMs may.

**The country screen.** This ROM has it but skips it. Screen 9's
per-frame handler jumps on its sub-page (0x285cee, a table at 0x009a53);
sub-page 2, the country screen, goes straight to 0x285e10, which calls
0x2866a2(2): country 2 is stored (RAM 0x0003) and the screen moves on
(sub-page 3, Player Profile, or Log-In), without drawing flags or waiting
for OK. The start-up code does the same (0x0a3ee5: if 0x0003 isn't 2, it
calls 0x2866a2(2)). Headlessly the developer mode's Down shows the empty
panel for under a second, then Log-In. No EEPROM or SIO value brings it
back: the choice was taken out of this build (it reads like a one-region
release). So a blank Ribbit console going straight to Player Profile is
what this ROM does; showing the country screen would need the older ROM
(or a patch of this one, not done). 0x00, 0xFF or garbage in the chip
make no difference.

**Other differences** (capture, then Ribbit with this ROM):

- After "Profile Saved" (shown about 20 s) the capture goes to the intro
  with the new player signed in; Ribbit goes to Log-In after 12 s
  (screen 7, a timeout: the same with S720 sounds made to finish at
  once).
- The intro: 56 s of music in the capture, no speech; Ribbit's (from
  Log-In) lasts 26 s with seven S720 lines and some A1600 sounds (S720: 0x2dd5d9,
  0x3a7ab7, 0x2f9e97, 0x308096, 0x2ffaa8, 0x2d8d97, 0x2c43d3; it waits
  for them: with S720 never finishing it stalls for good).
- Player Profile: the capture plays only music; Ribbit plays 0x2c1dae
  (4.4 s), then 0x2af930 (6.9 s), then 0x2c1dae again 10.3 s after
  the first (cutting the second short) while no name is typed.
- The home screen: the capture has no middle icon; Ribbit shows a
  cartridge icon named "BaseRom", and its build-info screen shows "Name
  BaseRom, PN 152-" where the capture's are empty. With no cartridge,
  Ribbit (as MAME) shows the system ROM's lower half at 0x000000-0x1fffff,
  and that half ends in a cartridge-style header at 0x1fff80 ("1.0",
  "R0000000010", "152-11811", "BaseRom"; the carts have theirs at the same
  place). The system reads the name from 0x1fffa1. On the real console
  the slot's header reads empty without a cartridge, or the older ROM
  has no such header; which is open. Port A bits 12 and 14 go into the
  status block at 0x08cc (0x3f04d9; Ribbit reads 0x4000 there); bit 14
  low makes the main loop call 0x3f0161 (an off/sleep path), so neither
  is obviously a cartridge switch.
- Single frames: on Player Profile and Log-In a name's text sometimes
  showed shifted or with letters missing for one frame ("ste" wrapped
  left of "Name:Rok", "Rok ter", "Gu" for "Guest"), and on Log-In a block
  of garbage tiles appeared at the top left for a frame now and then. Not
  seen in the capture (sampled every 0.5-3 s, so rare frames could be
  missed). GUESS: the data segment race (see "Mouse"), fixed 2026-11-16,
  which made far reads read the wrong bank; not rechecked frame by frame.
- The same in both: the logos, the flags' panel and Player Profile's
  layout, "Profile Saved", the Log-In screen (Settings tab, Music), the
  home screen's other icons, the intro's Leap, Lily and Scout, the
  shutdown animation.

**The shutdown** ('s' in developer mode, screen 0x0c): Leap, Lily and
Scout wave with an A1600 sound (6172 bytes) for about 7 s, the screen
goes black (the pointer sprite stays), then the ROM sets bit 15 of
0x3d20 (the system control register) after 0x3f0726(3 or 7) and loops at
0x3f0370 for good (0x3f0340). The capture's console turns off (no signal).
Ribbit notes the bit (`ClickStart::powered_off`) and the app then closes
the game, as for a Leapster that turned off.

**Idle power-off** (VERIFIED-FW and headlessly, 2026-11-15; `src/idle.rs`):
the home screen's handler (0x28f2f6, screen 1) counts its passes, 30 a
second, in word 6 of the state [0x1328] points to (0x174a there), and sets
it back to 0 on a key, an arrow or a pointer move (0x28f56e-0x28f57e).
Past 4499 (`cmp r3,0x1193` at 0x28f5ef), unless bit 10 of the state's
first word is set, it asks for screen 0x0c (0x28f5fc): the shutdown above.
So the home screen left alone for 150 s turns the console off. Nothing
else counts: the only other ways to screen 0x0c are 's' in developer mode
(0x28f463) and 0x28ee2b, which nothing found calls; Player Profile and
Log-In time out to the home screen instead (a blank EEPROM reaches it
after about 4.5 minutes); and the 13 English cartridges write 0x3d20 only
with bit 15 clear (0x4017, at start-up). Left alone, the system ROM ran an
hour on its home screen and Finding Nemo an hour on its menu without
turning off: headlessly the home screen puts the pointer back itself now
and then (0x29714d through 0x3fff0a, every 20-60 s), which counts as a
move, so the count was never seen past 0x381. Whether a real console does
that is unknown (no capture of one left alone).

"Turn off when idle" (`ClickStart::keep_awake`, the setting off): the
count is set back to 0 once a second while the home screen shows, as a key
would. On: the count runs, and when it runs out the console waves goodbye
and the app closes the game. The developer mode's shutdown closes it
either way. Test `tests/idle.rs` (needs the ROM): two seconds before the
count runs out, on turns the console off within 5-15 s, off keeps it on.
`csrun`'s CSRUN_WATCH=addr[:frame] prints where a word is written smaller
(how the count's resets were found).

## Save states

`ClickStart::save_state()` / `load_state()`: bincode of the machine with a
4-byte version first (4 since the S720 decoder, 2026-11-15). Left out: the ROMs, the cartridge, the EEPROM's
contents (kept as they are on load, as the Zippity keeps its NAND), the
picture being shown and the research logs. The app's state slots key them
by game title (`clickstart-<title>`).

Checked with `examples/csstate.rs`: run to a frame, save, run on (typing
meanwhile), load into the same machine and into a new one, run again.
Every frame's picture and sound match (system ROM at 110 frames, in the
jingle, 700, 780, in the Player Profile line, and 1000; Toy Story at 900;
150-600 frames each), with the EEPROM's contents put back
too: a state taken before the first format replays differently on a chip
that has since been formatted.

## Open questions

- Which picture key (smiley, note, boat, cat) is 2b, 2c, 2d, 2e, and
  which sound each makes.
- Arrows: Left (bit 1) and Down (bit 0) are confirmed by the developer
  mode; Up (bit 3) only by elimination.
- What Pause does in a game (nothing visible headlessly).
- The SIO chip's S720 rate (12288 Hz assumed since 2026-11-16, from
  speech rate and pitch; see "Rate"; no real console's S720 sound has
  been heard) and its own output filter. Ribbit's S720 decoder
  differs from S4872.DLL only in the LSP-to-LPC rounding; whether the chip
  rounds like the DLL is unknown. S480 (60-sample subframes) is decoded
  but no ROM uses it.
- Byte 1's bits 5-6 and 7 from the real keyboard (Ribbit sends 0x60).
- ADPCM level: MAME mixes the 12-bit OKI decoder output unscaled. Ribbit
  scales it by 16 to the full 16-bit range, and keeps that (2026-11-14):
  - The Sunplus PC codec DLLs (A1600/A1800/A2000/A34/A3600/A4800/A6400/
    A66/AM1100, S200/S320/S4872/S530/S880; all 32-bit) are the SACM
    codecs (the SIO chip's), not the sound unit's ADPCM. None holds
    an IMA or OKI step table, so they can't give the hardware level.
  - The other formats all fill the 16-bit wave data register at full
    scale: 16-bit as is, 8-bit as b<<8|b, ADPCM36 (in MAME too) as the
    nibble at the top of a word >> shift. A 12-bit OKI path would be the
    odd one out.
  - The BIOS and the 13 English carts start only OKI ADPCM channels
    (speech included; no PCM samples, no ADPCM36), so a mix against PCM
    can't be compared in a game. Absolute level decides: with x16 the
    BIOS music peaks at 2720 (-22 dBFS) and the games at 2260-2870, never
    clipping; unscaled they would peak near 170 (-46 dBFS).
  - Still not measured on a real ClickStart; a recording of the BIOS
    music next to a known level would settle it.
- A new console's 24C08: Ribbit starts it blank as 0xFF (erased cells).
  The system formats 0x00 and garbage the same way, so the only open part
  is what a factory-fresh chip holds.
- Where "Delete Player?" and the PARENT password are reached.
- What the slot's cartridge header reads on a real console with no
  cartridge (Ribbit shows the system ROM's "BaseRom" header there, so a
  "BaseRom" icon on the home screen), and port A bits 12 and 14.
- Whether a real console's home screen puts the pointer back by itself
  as it does headlessly, which keeps its 150 s idle count from running out.
- The one-frame text glitches on Player Profile and Log-In: probably the
  data segment race fixed 2026-11-16; to be rechecked.
- A channel stopped and started again without its registers rewritten
  (123 Click, as an activity starts: channels 0-3 at once) plays on from
  where it was; after a loop or a stop Ribbit (as MAME) has cleared the
  mode's ADPCM bit, so the rest of the ADPCM data plays as 16-bit PCM for
  a moment (a burst of noise, about 25 ms). Whether the real sound unit
  clears that bit or keeps a hidden "in the PCM loop" state is unknown.
- The older system ROM (Beta, Jan 2007, PN 152-11757) with the country
  screen isn't dumped.
- The SPG28x's exact model and timings (MAME's own question).
