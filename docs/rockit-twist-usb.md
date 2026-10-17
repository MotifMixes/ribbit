# RockIt Twist: the USB device controller

Research notes behind `crates/ribbit-twist/src/udc.rs` (the controller model), `usbip.rs` (the USB/IP server)
and `examples/usbhost.rs` (a test host). Worked out from the System firmware's own USB driver (0x4E000-0x56200);
checked by running it: with the model, the console enumerates and answers LeapFrog's file protocol
(`usbhost`: vendor 0x0F63, product 0x0047, "LeapFrog Enterprises, Inc." / "Pocket Play"; INFO, LIST, GETS,
RETR answered). See docs/rockit-twist.md, "USB and LeapFrog Connect".

Confidence labels: **VERIFIED** = read directly from code; **EVIDENCE** = strongly implied by how the
code uses it; **GUESS** = plausible, not proven.

## 0. Big picture (VERIFIED)

* **Nothing is interrupt-driven.** The "ISR" at 0x500E0 is called only by the System's poll routine
  0x25F5C, which runs from the USB task `DrvUsbdTask` (0x25FF0) loop:
  `0x25F5C: bl 0x500E0 (event poll) ; bl 0x51B84 (EP0 task, one pass) ; bl 0x4EBA8 (bulk/BOT task, one pass)`.
  There is no IRQ registration for the controller. So the emulator only needs register semantics;
  no IRQ line is needed.
* The device-side code that is actually used: init 0x4FE70 (via 0x25C24), deinit 0x4F900, event poll
  0x500E0, EP0 task 0x51B84 + setup handler 0x5167C, BOT task 0x4EBA8 + SCSI dispatcher 0x4EA28,
  DMA helpers 0x4E8B8 (bulk IN) / 0x4E958 (bulk OUT), EP0 IN sender 0x5072C.
* Dead/alternative code in the same library (not called on the Twist): second init 0x4FCD0, second
  EP0 task 0x51188 + setup handler 0x50858 (UVC/UAC/HID class requests, the "Set Feature to ENDPOINT"
  string, stubs 0x692C..0x6974 that return constants), PIO block movers 0x52F6C/0x53078/0x53164,
  iso/other DMA 0x513FC/0x51490 (regs 0x168/0x170/0x1E0/0x1E8/0x1EC, C4 bits 13/14, 0x178 bit 2).
* **The 0x800-0xC00 block and 0xC0070000 belong to the USB *host* controller** (EVIDENCE): only code in
  0x52xxx-0x56xxx touches them (host init 0x549EC writes C0=0xA00, 0x83C reset pulse, 0x860/0x868/
  0x8B0/0x8B8 timing values, 0xB00 = host interrupt status W1C, 0x844 bit0 = port connect,
  0xC0070000 +0x38 = its W1C status, +0x44/+0x3C config). That code is a USB host mass-storage stack
  (it builds SCSI CDBs itself, e.g. WRITE(10) at 0x52E28) reached only from FS code at 0x6B7C/0x6BBC
  and owns the "USBH SUPER PLUG OUT DETECTED" string. The device path never touches it, so for device
  emulation it can stay unmodelled. 0xC00E0000+0xC0 is shared: device mode writes 0x930, host 0xA00.
* The interface is **USB Mass Storage, Bulk-Only Transport, SCSI transparent** (class 08/06/50),
  EP 0x81 bulk IN and EP 0x02 bulk OUT. LUN 0 is a **direct-access disk**, not a CD-ROM (see section 6;
  this corrects docs/rockit-twist.md).

## 1. Register map (device side, base 0xC00E0000)

| Off | Dir | Meaning | How used | Conf. |
|---|---|---|---|---|
| 0x0C0 | W | Mode/control. Device init: 0 then 0x930; deinit 0; host init 0xA00 | | EVIDENCE |
| 0x0C4 | RW | DMA/command. 0x800 = reset pulse at init (set, read back, clear bit 11); 0x200 = start bulk-IN DMA; 0x400 = start bulk-OUT DMA; 0 = stop | 0x4FEC8, 0x4E8F0, 0x4E98C | VERIFIED use, meaning EVIDENCE |
| 0x0C8 | R | Abort flags: any wait loop (0x4FB90/0x4FA9C) gives up if bit0 or bit1 is set. Must read 0 | 0x4FC20 | VERIFIED use, meaning GUESS (VBUS lost / disconnected) |
| 0x0CC | RW | **EP0 FIFO**, one byte per 32-bit access (low 8 bits). IN data written here; OUT data read here | 0x50670, 0x506E8, 0x510D8 | VERIFIED |
| 0x0D0 | W | **Bulk IN FIFO** (EP 0x81), one byte per write (low 8 bits). Up to 512 bytes per commit | 0x4E060.., 0x4F500 | VERIFIED |
| 0x0D4 | R | **Bulk OUT FIFO** (EP 0x02), one byte per read (low 8 bits) | 0x4E0F0, 0x4EC7C | VERIFIED |
| 0x0DC | RW | **Endpoint command/status register** (see section 3) | 61 writes | VERIFIED waits; bit meanings EVIDENCE |
| 0x0E4 | RW | Enable mask for 0xE8. Reset value written by firmware: 0x8409 (bits 0,3,10,15) | | VERIFIED |
| 0x0E8 | RW | **Device event flags, write-1-to-clear** (the poll writes the single bit it handles). The poll reads E8 **raw, without masking by E4**, so the emulator should only set a bit while its E4 bit is set | 0x501C4.. | VERIFIED |
| 0x0EC | RW | Enable mask for 0xF0 (init: EC \|= 4; the poll sets bit0 again after handling F0 bit0) | | VERIFIED |
| 0x0F0 | RW | **Top-level/aux status, write-1-to-clear** (bit0 acked by writing it, bit2 by writing 4, 0xFF at init). Read raw, not masked | 0x500E4 | VERIFIED |
| 0x0F4 | W | DMA mode: 0xC while a bulk DMA is set up, 0 after | 0x4E8D8 | VERIFIED use, meaning GUESS |
| 0x0F8 | W | **STALL request**: bit0 EP0, bit1 bulk IN, bit2 bulk OUT. Cleared to 0 by firmware once 0x150 says the STALL went out | 0x4FC98, 0x4ED4C, 0x4ED78 | VERIFIED |
| 0x0FC | W | FIFO flush/reset (0xF on bus reset, 0x7F at init, 0xFF after an EP0 ZLP, 6 in dead PIO code) | | GUESS |
| 0x100 | W | Written 0x10 after the CBW is read and after a bulk-OUT DMA (bulk OUT FIFO clear/ack?) | 0x4ECA4, 0x4E9EC | GUESS |
| 0x118 | R | SETUP bmRequestType (byte) | 0x51698 | VERIFIED |
| 0x11C | R | SETUP bRequest | | VERIFIED |
| 0x120 | R | SETUP wValue (16-bit; descriptor type = bits 15-8, index = bits 7-0) | | VERIFIED |
| 0x124 | R | SETUP wIndex (16-bit). Also read in the F0-bit0 path as an interface number (1 or 3) | 0x517CC, 0x50110 | VERIFIED |
| 0x128 | R | SETUP wLength (16-bit) | | VERIFIED |
| 0x140 | W | DMA: blocks << 8 (512-byte packet count?) | 0x4E8DC | VERIFIED value, GUESS meaning |
| 0x150 | RW | **STALL-sent status, W1C**: bit8 EP0, bit9 bulk IN, bit10 bulk OUT. Firmware waits for the bit, writes it back, then F8=0 | 0x4FCA0, 0x4ED54, 0x4ED84 | VERIFIED |
| 0x154 | R | Alternate-setting bits read with F0 bit0 (bits 3:2 = interface 1, bits 7:6 = interface 3). Not relevant to MSC | 0x50124, 0x50288 | EVIDENCE |
| 0x158, 0x15C, 0x19C, 0x18C | W | Configuration written at init (1, 0xFA50, 0x7C, 3). Never read | 0x4FEAC.. | VERIFIED values, meaning unknown |
| 0x160 | W | Set to 1 around an EP0 IN commit with no data = **send zero-length packet**, then 0 | 0x5080C | EVIDENCE |
| 0x164 | RW | **DMA done, W1C bit0**. Firmware waits for bit0=1, then writes 1 | 0x4E8F8, 0x4E934 | VERIFIED |
| 0x168 | W | 0 at init (other DMA config) | | - |
| 0x174 | W | 4 at init (A), 0 at deinit | | unknown |
| 0x178 | RW | Aux status W1C: bit5 (just acked), bit2 (signals OS event 0xBAD5C, iso DMA done). 0x7F at init | 0x501A4 | VERIFIED |
| 0x1A4 | RW | PHY/connect control. Bit1 = soft-disconnect (set at task start and deinit, pulsed 1->0 at init, i.e. D+ pull-up on when 0); bit2 and bit7 set at init; **bit15 read = high-speed** (selects 512 vs 64 byte endpoint sizes in the configuration descriptor) | 0x4FF00.., 0x5156C, 0x51624 | bit15 EVIDENCE (strong), bit1 EVIDENCE |
| 0x1A8 | RW | Enable mask for 0x1AC (2 at reset/init; bit0 set after a SETUP is handled) | | VERIFIED |
| 0x1AC | RW | Second event register, W1C: bit0 is handled exactly like E8 bit2, bit1 like E8 bit10 | 0x501D0 | VERIFIED |
| 0x1C4/0x1CC | RW | Indirect PHY register access: 1C4=(reg<<8)\|data, 1CC=3 write / 1 read, poll 1CC bit0 until 0, result in 1C4 (used only by host init) | 0x50FF8 | EVIDENCE |
| 0x1C8 | W | DMA byte count (blocks << 9) | 0x4E8E0 | VERIFIED |
| 0x1D0 | W | DMA RAM address (CPU address, buffer 32-byte aligned; caches cleaned/invalidated around it) | 0x4E8E8 | VERIFIED |

### 0xE8 event bits (poll at 0x501C4; the order of the checks is bit0, 2, 3, 4, 5, 8, 10, 13, 14, 15)

| Bit | Firmware action | Meaning | Conf. |
|---|---|---|---|
| 0 | E8=1; state(0xBADA0)=1 | **SETUP packet received** (regs 0x118-0x128 valid) | VERIFIED action, EVIDENCE meaning |
| 2 (or 1AC bit0) | ev(0xBAD84)=4; E4&=~4; 1A8&=~1; E8=4; 1AC=1 | **EP0 OUT packet received** (status-stage ZLP of a control read) | EVIDENCE |
| 3 | E8=8, nothing else | unknown (enabled in 0x8409) | VERIFIED |
| 4 | ev=0x10; E4&=~0x10; E8=0x10 | **EP0 IN ready** (host is asking for data-stage IN) | EVIDENCE |
| 5 | E8=0x20; state=0 | control transfer aborted? | GUESS |
| 8 | ev=0x100; E4&=~0x100; E8=0x100 | **Bulk IN ready / FIFO empty**: lets the BOT task send the CSW | EVIDENCE |
| 10 (or 1AC bit1) | ev=0x400; state=4; E4&=~0x400; E8=0x400; 1A8&=~2; 1AC=2 | **Bulk OUT packet received** (a CBW is waiting in the D4 FIFO) | EVIDENCE |
| 13 | E4=(E4&~0x2000)\|0x4000; E8=0x2000; callback 0xBAD58 (System 0x25BCC: abort flag 0xBAD9C=1) | **Suspend** | EVIDENCE |
| 14 | E8=0x4000; E4=(E4&~0x4000)\|0x2000; 0xBAD9C=0; callback 0xBAD70 (no-op) | **Resume** | EVIDENCE |
| 15 | E8=0x8000; clears EP0 count, abort flag, state, ev, stall flag; FC=0xF; E4=0x8409; 1A8=2; callback 0xBAD74 (System 0x25BE4: task state 2 -> 3) | **Bus reset** | VERIFIED action |

F0: bit0 -> (EC bit0 masked during handling) F0 written with bit0 set, then 0x124/0x154 read (alternate
setting chosen by the hardware-handled SET_INTERFACE), then EC bit0 set again. Bit2 -> W1C, callback
0xBAD60 (System 0x25C08 = no-op). **If F0 bit0 or bit2 stays set, the poll returns before ever looking
at E8** (this is the current emulator hang). F0 must be W1C and should normally read 0.
0x178 bits 5/2 are checked next, also before E8; they must read 0 (W1C).

E8 is only examined if `E8 != 0 || (1AC & 3) != 0`; first a callback at 0xF573C is called if non-zero
(it is 0 on the Twist).

## 2. Init, deinit, plug and reset

**Init 0x4FE70** (VERIFIED, matches the earlier dynamic trace exactly; it contains no polling loop):
```
C0=0; 15C=0xFA50; 19C=0x7C; E8=0xFFFF; 178=0x7F; 18C=3; 1AC=0x3F; F0=0xFF
C4=0x800; C4 &= ~0x800          (reset pulse; reads C4 back)
FC=0x7F; 168=0; 158=1; E4=0x8409; EC|=4; 1A8=2; C0=0x930
1A4|=2; 1A4&=~2; 1A4|=4; 1A4|=0x80   (connect: pull-up on)
RAM: state/ev/stall/abort=0, 0xF5738=1, descriptor callback 0xBAD54=0x4F8E4, suspend cb 0xBAD58=0x4F8EC
```
(The System then overrides callbacks in 0x25C24 before calling init: 0xBADA4 signal=0x25BC8 (no-op),
0xBAD58=0x25BCC, 0xBAD70=0x25BE0, 0xBAD74=0x25BE4, 0xBAD60=0x25C08, 0xBAD54=0x25C0C (returns 0 -> use
built-in descriptors), LUN table, SCSI hooks 0x25CF8 (write) / 0x263C8 (read), and 0x4EA08(0xC0)
(vendor-opcode threshold).)

**Deinit 0x4F900**: E4=0, EC=0, 1A8=0, 174=0, FC=0xF, E8=0xFFFFFFFF, F0=0xFF, 178=0xFFFF, C4=0, C0=0,
1A4|=2 (disconnect).

**What the task waits for** (DrvUsbdTask 0x25FF0, task state byte 0xC325C):
* 0 idle -> on a "start" message (Ptp_startUsbTask): if cable present (jump-table service 0xFFF0B4, the
  GPIO port C bit 11 check) init and go to state 2, else state 1 (poll cable, debounced by 0x25ED8).
* **State 2: it must see a bus reset (E8 bit15 -> callback 0x25BE4 -> state 3) within 1000 ticks of
  0xFFF0C4 (`cmp r0,#0x3e8` at 0x26150), otherwise it deinits and goes to state 5 (charge only).**
  So: after the firmware writes 1A4 bit1=0 (pull-up on), the host model must raise E8 bit15 soon.
* State 3: calls 0x25F5C continuously; cable removal (0x25ED8) or SCSI START STOP UNIT with LoEj
  (flag 0xF5718) ends the session.

**Speed** (EVIDENCE): 0x1A4 bit15=1 means high speed. GET_DESCRIPTOR(config) patches both endpoint
wMaxPacketSize fields to 0x200 when bit15=1, to 0x40 when 0; other-speed config (type 7) gets the
opposite. Recommend emulating high speed: 0x1A4 reads with bit15 set once connected.

## 3. Register 0xDC (endpoint command/status)

Every write the firmware makes, and every wait (wait helpers: 0x4FB90 (timeout 0xEA1 timer ticks) and
0x4FA9C (timeout 0x3924E8); both return early if E8 bit13, abort flag 0xBAD9C, or 0xC8 bits 0/1;
a timeout just returns 1 and the firmware mostly carries on). All VERIFIED:

| Firmware writes | then waits for | where | Context |
|---|---|---|---|
| 0x1 | - | 0x51690 | start of SETUP handling |
| 0x20 | **(DC & 0x20) != 0, no timeout (infinite loop!)** | 0x51804 | GET_MAX_LUN, before sending data |
| 0x20 | - | 0x51930 | after GET_DESCRIPTOR set-up (data sent later) |
| 0x10 | (DC & 0x10) == 0 (counted loop 0x10000) | 0x5072C/0x506A0 | each EP0 IN packet (after writing bytes to 0xCC; with 0x160=1 for a ZLP) |
| 0x40 | - | 0x51834, 0x51CCC | EP0 IN data stage finished |
| 0x2 | (DC & 0x4) == 0x4 | 0x51848, 0x51C78 | status stage: accept EP0 OUT |
| 0x4 | - | 0x51860, 0x51C98 | ack EP0 OUT |
| (after 0x4) | (DC & 0x40) == 0x40 | 0x51CA8 | EP0 task state 3 only |
| 0x80 | - | 0x51CB0 | end of control transfer |
| (read) | if (DC & 0x80) stop sending | 0x507E0 | EP0 IN loop: host ended the data stage early |
| 0x100 | (DC & 0x100) == 0 | 0x4F4D0, 0x4E0BC.. | commit bulk IN FIFO (0xD0 bytes) |
| 0x800 | (DC & 0x1000) == 0x1000 | 0x4EC64 | before reading the CBW from 0xD4 |
| 0x1000 | - | 0x4ECA0 | after reading the CBW (release FIFO) |
| 0x2400 | - | 0x4EC08 | CBW accepted, about to execute |
| 0x400 | (DC & 0x400) == 0x400 (long timeout), then 0x400 again | 0x4EAB0, 0x4ED30 | start of SCSI dispatch; before a bulk IN stall |
| 0x800 | - | 0x4E994 | with C4=0x400: start bulk OUT DMA |
| 0x600 | - | 0x4E944 | after bulk IN DMA finished |
| 0x2000 | - | after CSW, after OUT DMA, after GET_MAX_LUN, BOT reset, SET_CONFIGURATION(1), SET_INTERFACE | **arm bulk OUT for the next packet** (EVIDENCE) |

Also, after a DMA (either direction) the firmware waits for `(DC & 0x100) == 0`.

Suggested read model (GUESS, but it satisfies every wait above):
```
DC read = latched value of the last write, modified by:
  bit4  (0x10)  : cleared when the host has taken the EP0 IN packet
  bit8  (0x100) : cleared when the host has taken the bulk IN packet / DMA IN data
  bit2  (0x4)   : set when the EP0 status-stage OUT ZLP has arrived (stays set until DC=4 written)
  bit6  (0x40)  : set from the 0x40 write until 0x80 is written (status done)
  bit5  (0x20)  : set from the 0x20 write (must read 1 or GET_MAX_LUN hangs forever)
  bit10 (0x400) : set when 0x400 is written
  bit12 (0x1000): set while the bulk OUT FIFO holds an unread packet (the CBW)
  bit7  (0x80)  : 0 unless the host aborted a control-IN data stage
```

## 4. EP0 (control) from the host model's point of view

Requests the firmware never handles (they would be STALLed by its default branch 0x516FC -> 0x4FC84),
so **the controller must handle them itself without raising E8 bit0** (EVIDENCE): SET_ADDRESS (00 05),
GET_STATUS, CLEAR_FEATURE/SET_FEATURE (including ENDPOINT_HALT on 0x81/0x02), GET_CONFIGURATION,
GET_INTERFACE. SET_ADDRESS: the firmware never writes an address register; apply it in the model.

Requests passed to the firmware (setup handler 0x5167C, VERIFIED):

| bmRequestType/bRequest | Firmware |
|---|---|
| 0x80, any bRequest (treated as GET_DESCRIPTOR) | type (wValue>>8): 1 device (0x846C4, 18), 2 config (0x84744, 32; also E4\|=0x2000 suspend enable), 3 string index 0..3, 6 qualifier (0x84764, 10), 7 other-speed config; length = min(wLength, descriptor length); DC=0x20; data sent later by the EP0 task. Other types -> STALL |
| 0xA1 0xFE wValue 0 wIndex 0 wLength 1 (GET_MAX_LUN) | sends 1 byte (0x848E0 = 0, one LUN) synchronously, does the status stage itself, then sets BOT state 4, DC=0x2000, E4\|=0x400, 1A8=2 |
| 0x21 0xFF wValue 0 wLength 0 wIndex != 0xFFFF (BOT reset) | writes 0 to 0xD4 and 0xD0 64 times each, DC=0x2000 |
| 0x00 0x09 wValue 1 (SET_CONFIGURATION 1) | DC=0x2000 (anything else -> STALL) |
| 0x01 0x0B (SET_INTERFACE) | DC=0x2000 |
| 0x21 0x0A wValue 0 (HID SET_IDLE) | DC=0x2000 |
| anything else | STALL: F8=1, wait 0x150 bit8, 0x150=0x100, F8=0 |

**Deliver a SETUP** (host model):
1. If it is one of the hardware-handled standard requests, complete it in the model (for SET_ADDRESS
   switch address after the status stage). Done.
2. Else latch 0x118=bmRequestType, 0x11C=bRequest, 0x120=wValue, 0x124=wIndex, 0x128=wLength,
   set E8 bit0 (E4 bit0 is on after reset). Clear any stale EP0 FIFO.
3. Poll: E8 bit0 -> state 1. EP0 task: state 2, DC=1, handler runs, then `ev=0; E4|=0x14; E8=0x10;
   1A8|=1`.
4. No-data request (wLength 0): the firmware does nothing for the status stage (no DC handshake), so
   the model completes the status stage itself once DC=1 has been written and the handler returned
   (or once F8 bit0 shows it STALLed). Firmware state stays 2 until the next SETUP: harmless.
5. Control read: once E4 bit4 is set, raise **E8 bit4** (host IN token). Poll -> ev=0x10 -> EP0 task
   calls 0x5072C: for each packet the firmware writes <=64 bytes to 0xCC (one byte per write), DC=0x10,
   waits DC bit4=0. The model collects the FIFO bytes as one packet when DC=0x10 is written (a packet
   with 0x160 bit0=1 and no bytes is a ZLP; it is sent when the total is a multiple of 64 and shorter
   than wLength), clears DC bit4. Then firmware writes DC=0x40 and moves to state 3.
   Between packets it does `E4|=0x10; E8=0x10` itself, so do not raise bit4 again during the loop.
6. Status OUT: when the data stage is complete, set DC bit2 and raise **E8 bit2** (E4 bit2 is on).
   Poll -> ev=4 -> EP0 task state 3: DC=2, waits DC bit2, DC=4, state 0, waits DC bit6, DC=0x80.
   For GET_MAX_LUN the same handshake happens inside the setup handler (DC=0x40, DC=2, wait bit2,
   DC=4), so DC bit2 must already be readable as 1 at that point: set it as soon as the host has
   received all data.
7. STALL: firmware sets F8 bit0; the model answers the next EP0 token with STALL and sets 0x150 bit8
   (W1C). Firmware then clears F8.
8. Control-write data stage (not used by MSC): firmware would DC|=2, wait DC bit2, read bytes from 0xCC
   (dead code 0x51098).

### Descriptors (byte-exact, from system.bin; VID/PID patched at run time)

Device descriptor @0x846C4 (18 bytes):
`12 01 00 02 00 00 00 40 VV VV PP PP 00 01 01 00 02 01`
(bcdUSB 2.00, class 0, EP0 size 64, **VID/PID = 0 in the image; set by 0x26740(vid,pid) <- 0x3199C,
which gets them from jump-table service 0xFFF278** (not determined statically), bcdDevice 1.00,
iManufacturer 1, iProduct 0, iSerialNumber 2, 1 configuration).

Configuration @0x84744 (wTotalLength 32):
```
09 02 20 00 01 01 00 C0 32        config 1, self-powered, 100 mA
09 04 00 00 02 08 06 50 00        interface 0, 2 EPs, MSC / SCSI / Bulk-Only
07 05 81 02 00 02 00              EP 0x81 bulk IN,  512 (64 at full speed)
07 05 02 02 00 02 00              EP 0x02 bulk OUT, 512 (64 at full speed)
```
Device qualifier @0x84764: `0A 06 00 02 00 00 00 40 01 00`.
Strings: 0 @0x84740 `04 03 09 04`; 1 @0x84708 "LeapFrog Enterprises, Inc." (0x36 bytes);
2 @0x95470 (RAM, filled by the task at 0x261EC: "Pocket Play   " 0x1E bytes from 0x7AE00, or
"LeapStart Holo"/"MagicAdvGlobe " for other product ids); 3 @0x84704 "0".

## 5. Bulk-Only Transport (BOT task 0x4EBA8, one pass per poll; VERIFIED)

RAM: 0xBADA0 = shared state (EP0 uses 1-3, BOT 4-6, 0 idle), 0xBAD84 = last event, 0xBACA8 = CBW
buffer (31 bytes), 0xBACDC = residue, 0xBAD50 = CSW status, 0xBACC8 = stall request (1 = bulk IN,
2 = bulk OUT), 0xF5720 = sense index.

```
event E8 bit10 (bulk OUT packet)  -> state 4, ev 0x400
state 4 & ev 0x400: DC=0x800; wait DC bit12; read 31 bytes from 0xD4; DC=0x1000; 0x100=0x10;
                    ev=0; state 5
state 5:            DC=0x2400; dispatch 0x4EA28:
                       residue=dCBWDataTransferLength, LUN=CBW[13], opcode=CBW[15], status=0
                       (the 'USBC' signature is NOT checked); DC=0x400, wait DC bit10, DC=0x400
                       handler from table 0x7DEFC (17 entries) or "invalid opcode" (sense 0x1A)
                    if stall==1: DC=0x400, wait bit10, DC=0x400; F8=2; wait 0x150 bit9; 0x150=0x200; F8=0
                    if stall==2: F8=4; wait 0x150 bit10; 0x150=0x400; F8=0
                    E4 |= 0x100; state 6
event E8 bit8 (bulk IN ready)     -> ev 0x100
state 6 & ev 0x100: CSW: 13 bytes to 0xD0: 'U','S','B','S', tag = CBW[4..7], residue LE, status;
                    DC=0x100; wait DC bit8=0; ev=0; DC=0x2000; 1AC=2; E4|=0x400; 1A8|=2; state 0
```
If a SETUP arrives during a BOT transaction it overwrites the shared state; a normal host does not do
that.

Data phases:
* **PIO IN** (INQUIRY, READ CAPACITY, REQUEST SENSE, MODE SENSE, READ FORMAT CAPACITIES, READ TOC...):
  bytes written one per access to 0xD0, then DC=0x100, wait DC bit8 = 0. 0x4F490 sends >512 bytes in
  512-byte chunks, each committed with DC=0x100.
* **DMA IN** (READ data), 0x4E8B8: setup `F4=0xC; 140=n<<8; 1C8=n*512; 1D0=addr; C4=0x200`; finish:
  wait 0x164 bit0=1, wait DC bit8=0, `164=1; F4=0; C4=0; DC=0x600`. n <= 32 blocks.
* **DMA OUT** (WRITE data), 0x4E958: setup `F4=0xC; 140=n<<8; 1C8=n*512; 1D0=addr; C4=0x400; DC=0x800`;
  finish: wait 0x164 bit0=1, wait DC bit8=0, `164=1; F4=0; C4=0; 100=0x10; DC=0x2000`.
* The firmware never reads a received-byte count. The CBW is always read as exactly 31 bytes; OUT data
  sizes come from 0x1C8. (No count register exists in the used path.)

**Host model, per command:**
1. Bulk OUT CBW (31 bytes): only when E4 bit10 is set (re-enabled after each CSW). Put the bytes in the
   OUT FIFO (readable through 0xD4, one byte per read), set DC bit12, raise E8 bit10.
2. Host-to-device data: wait until the firmware starts an OUT DMA (C4 bit10 written with F4/1C8/1D0
   set, plus DC=0x800). Copy up to 1C8 bytes of host data to RAM at 0x1D0, set 0x164 bit0 (and keep
   DC bit8 = 0). Do not put data-stage bytes in the 0xD4 FIFO.
3. Device-to-host data: whenever DC=0x100 is written, take the 0xD0 FIFO bytes as data (clear DC bit8).
   When C4=0x200 is written, read 0x1C8 bytes from RAM at 0x1D0 as data, set 0x164 bit0.
4. STALL: when F8 bit1/bit2 is written, the corresponding pipe's next token gets STALL; set 0x150
   bit9/bit10. The host then CLEAR_FEATURE(HALT)s the pipe (hardware-handled, see section 4).
5. CSW: after the data phase, when E4 bit8 becomes set, raise E8 bit8; the firmware writes 13 bytes to
   0xD0 and DC=0x100. A data-IN phase shorter than dCBWDataTransferLength just ends there (the residue
   in the CSW says so; e.g. the LBA 0x20 read hook returns no data when it has no reply).

## 6. SCSI layer and the LeapFrog hooks (VERIFIED)

LUN table at 0xBACE4 (12 bytes per LUN): +0 LUN number, +1 **type**, +4 block count, +8 flags
(bit1 write-protect, bit2 media-changed, value 1 = not ready). 0x25C24 sets LUN 0: type **0x0E**,
blocks 0x680000, flags 0; LUNs 1-5 flags 1. GET_MAX_LUN answers 0 (one LUN).

Because LUN 0's type is 0x0E (not 8), the CD-ROM paths (inquiry 0x846D8 "GENPLUS USB-CDRM DISK",
2048-byte blocks, MODE SENSE page 0x2A, READ TOC) are **not** used; that INQUIRY string belongs to an
unused LUN type. The Twist presents a **512-byte-block direct-access disk**.

| Opcode | Handler | Behaviour |
|---|---|---|
| 0x00 TEST UNIT READY | 0x4EF08 | dCBWDataTransferLength must be 0 (else status 1, stall OUT); ready check 0x4EDFC (flags/timer); status 0 |
| 0x03 REQUEST SENSE | 0x4E200 | alloc (CBW[0x13]) == 18: fixed sense `F0 00 key 00 00 00 00 0A 00 00 00 00 asc ascq 00 00 00 00` from table 0x7DE84 (3 bytes per index), then clears sense; alloc 0 -> no data; alloc > 0x40 -> error; other alloc -> zeros |
| 0x12 INQUIRY | 0x4E468 | dCBWDataTransferLength 0 or 0xFF -> status 1; else sends min(len, 44) bytes from 0x846A0: `00 00 00 00 1F 00 00 00 "Leapfrog" "Pocket Play 6060" "0100"` + 8 zero bytes (product field written by the task from 0x7AE20; other products "LeapStartHolo   "/"MagicAdvGlobe   "). Peripheral type 0, not removable |
| 0x1A MODE SENSE(6) | 0x4EFAC | 4 bytes `03 00 WP 00` (WP = 0x80 if flags bit1) |
| 0x1B START STOP | 0x4DD14 | LoEj (CDB[4] bit1) sets eject flag 0xF5718 -> task ends the session |
| 0x1E PREVENT ALLOW | 0x4DDAC | prints "usb_removal_cmd_exist", status 0 |
| 0x23 READ FORMAT CAPACITIES | 0x4F008 | 12 bytes `00 00 00 08` + blocks (BE) + `02 00 02 00` |
| 0x25 READ CAPACITY(10) | 0x4F064 | 8 bytes: last LBA (BE, 0x0067FFFF) + `00 00 02 00` |
| 0x28/0xA8 READ | 0x4F0D4 | length check: dCBWDataTransferLength must equal blocks*512 (else residue = difference, status 1, stall IN); direction must be IN; ready check; then hook 0xF5728 = **0x263C8** |
| 0x2A/0xAA WRITE | 0x4F2D8 | same checks (direction OUT, stall OUT on mismatch), write-protect -> sense 0x25; then hook 0xF5724 = **0x25CF8** |
| 0x2F VERIFY | 0x4DD5C | status 0 |
| 0x43 READ TOC | 0x4F578 | CD style TOC (only meaningful for type 8) |
| 0x5A MODE SENSE(10) | 0x4F860 | page 5/0x3F: 8-byte header `00 06 00 WP 00 00 00 00`; page 0x2A: 28 bytes from 0x7DF84; else error |
| 0xF0/0xF3 | 0x4DE24/0x4DF10 | GeneralPlus vendor commands (CBW[0x1D..0x1E] = "GP") |
| other | - | status 1, sense 0x1A (05/20/00), stall in the CBW's direction if dCBWDataTransferLength != 0 |

LBA = CDB[2..5] big-endian (0x4E7C8), block count = CDB[7..8] (0x4E7F4; 4 bytes for the 12-byte
forms). Sense indices used: 0x10 = 03/03/00 (hook failure), 0x12 = 03/11/00 (read hook failure),
0x1B = 05/21/00 (LBA not 0x20/0x40), 0x1A = 05/20/00, 0x22 = 06/28/00, 0x0B = 02/3A/00.

**Write hook 0x25CF8** (buffer = task message buffer, 32-byte aligned, at 0xC3268):
* LBA 0x20, 1 block: DMA OUT 512 bytes to the buffer, invalidate cache, call the command callback
  (0xC325C+0x10) with the buffer. OK -> residue 0, status 0. Callback error -> residue 0x200, sense 0x10.
* LBA 0x20, more than one block: no data taken, residue = blocks*512, sense 0x10.
* LBA 0x40: n = min(blocks, 32); DMA OUT n*512 bytes; callback +0x18(buf, n) must return n; residue =
  (blocks-n)*512.
* other LBA: sense 0x1B, returns -1 (status 1).

**Read hook 0x263C8**:
* LBA 0 and 1 block: sends 512 zero bytes (DMA IN).
* LBA 0x20, 1 block: zero the buffer, callback +0x14(buf) -> reply text. -1 -> error (residue 0x200,
  sense 0x12); 0 -> no data at all, residue 0x200, status 0; otherwise DMA IN 512 bytes, residue 0.
* LBA 0x20, other counts: residue blocks*512, sense 0x12.
* LBA 0x40: n = min(blocks, 32); callback +0x1C(buf, n) must return n; DMA IN n*512; residue
  (blocks-n)*512.
* other LBA: sense 0x1B, status 1.

### Host script that exercises everything

```
plug: GPIO C bit11=1. Wait for 1A4 bit1 = 0 (pull-up). Raise E8 bit15 (bus reset); 1A4 bit15=1 (HS).
SETUP 80 06 0100 0000 0040 -> 18 bytes; status OUT
SET_ADDRESS (model only)
SETUP 80 06 0200 0000 00FF -> 32 bytes; status
SETUP 80 06 0300/0301/0302 ... -> strings
SETUP 00 09 0001 0000 0000 -> SET_CONFIGURATION (firmware writes DC=0x2000)
SETUP A1 FE 0000 0000 0001 -> 1 byte 0x00 (firmware arms bulk OUT itself)
CBW TEST UNIT READY (len 0)         -> CSW status 0
CBW INQUIRY (len 36, IN)            -> 36 bytes PIO, CSW
CBW READ CAPACITY(10) (len 8, IN)   -> 8 bytes PIO, CSW
CBW WRITE(10) LBA 0x20 n=1 (len 512, OUT) + 512 data bytes via DMA -> CSW
CBW READ(10)  LBA 0x20 n=1 (len 512, IN)  -> 512 bytes via DMA (or none, residue 512) -> CSW
```

## 7. Still uncertain

* Exact semantics of individual DC bits (only the write/wait pairs are verified); the read model in
  section 3 is a design that satisfies the code.
* Which standard requests the silicon answers by itself. Inferred from the firmware's stall-by-default
  handler: SET_ADDRESS, GET_STATUS, CLEAR/SET_FEATURE, GET_CONFIGURATION, GET_INTERFACE must be
  handled by hardware (otherwise enumeration could never work).
* Whether a no-data control request needs any firmware handshake for its status stage (the firmware
  does none; DC=0x2000 after SET_CONFIGURATION looks like "arm bulk OUT").
* E8 bits 3 and 5, F0 bit2, 0x178, 0x1AC versus E8 (duplicates of E8 bits 2/10, maybe "short
  packet"/"NAK" variants), 0xFC, 0x100, 0x140, 0xF4, 0xC8 meanings.
* VID/PID values (from jump-table service 0xFFF278 at 0x3198C; not resolved statically).
* The tick unit of 0xFFF0C4 for the 1000-tick bus-reset deadline (probably milliseconds).
