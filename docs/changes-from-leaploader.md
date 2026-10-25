# Changes made during LeapLoader work

LeapLoader (an SD-card loader for the Leapster 2) is tested in Ribbit. Fixes found there are made in
Ribbit directly and listed here.

## 2026-10-25: a paused voice reads as free in the voice status register

* **Problem:** the Leapster 2 BIOS 0.6 stops a raw sound (`LF_StopAudioPlayer`) with the pause command
  (byte `0x20 | voice`) and then picks free voices for new raw sounds from the status register
  `0x0180_2078`. Ribbit kept paused voices set there, so after two stops both raw voices (5 and 6) read
  as busy and the BIOS did not start the next raw sound at all (LeapLoader's menu music fell silent at
  its second loop; logged commands: start 0x80|6, 0x80|5, pause 0x20|6, start, pause 0x20|5, then no
  start).
* **Change:** `VoiceEngine::status` (ribbit-core `io/voices.rs`) counts a voice only while it is
  triggered and not paused. Pause and resume themselves are unchanged (a paused voice keeps its place;
  Top-Secret Personal Beeswax's speech still pauses and resumes). This replaces the module's earlier
  HYPOTHESIS that paused voices stay set; real games stop sounds this way constantly, so on hardware a
  stopped voice must come free.
* **Test:** `paused_voice_holds_its_place_and_resumes` now expects the paused voice to read as free.


Also seen, not changed: for a raw sound of 64 KiB or more the BIOS writes the end's low half from a read
of the voice's start-low register (`0x0180_40C8 + 8 x voice`, BIOS 0x4003_64F8); Ribbit reads 0 there, so
such sounds stop at the 64 KiB boundary below their end. What the hardware returns is unknown, so
Ribbit is left as it is (LeapLoader pads its long sounds instead).

## 2026-10-25: rewind snapshots leave the SD card out

* **Problem:** with a large Leapster 2 SD card (LeapLoader's 77-game card is ~680 MB), every rewind
  snapshot (one per half second of emulated time) serialised and compressed the whole card. That took
  longer than half a second, so the snapshot queue to the compressor thread grew without bound: the
  game slowed down and `ribbit-gui` grew by ~0.4 GB/s (29 GB when it was stopped).
* **Change:** `Machine::rewind_snapshot` / `load_rewind_state` (ribbit-core `lib.rs`), used by the
  rewind in ribbit-frontend `app.rs`: the card's sectors are taken out while the snapshot is made and
  kept when one is loaded (with the card's unsaved-changes flag). Same state format, `STATE_VERSION`
  unchanged; save states (manual slots, the desktop `--save-state`) still include the whole card.
* **Effect:** rewinding no longer undoes what a game wrote to the card (the card has its own file anyway).
* **Test:** `tests::rewind_snapshots_leave_the_sd_card_alone`.

## 2026-10-25: the Leapster 2's USB (mass storage over USB/IP)

* **Why:** LeapLoader's desktop app installs games through the console's USB (two mass-storage LUNs,
  LeapFrog's vendor commands C2 mount / C1 lock / C6 eject). Research shared with LeapLoader:
  LeapLoader's `docs/usb-research.md`.
* **Change:** ribbit-core `io/usb.rs` models the controller at 0x0180_F000 (Freescale USB-FS OTG: OTG
  glue, device core, buffer descriptors in on-chip RAM, interrupt line 15) and the host at the other end
  of the cable; `Machine::usb_plug` / `usb_submit` / `usb_unlink` / `usb_take_done`. The frontend offers
  it through the RockIt Twist's USB/IP server (port 3240): Options menu, "Leapster 2: plug into this
  PC (USB)". The cable is in while the option is on; a USB/IP client's attach and detach only connect
  the PC (its unfinished transfers are cancelled on detach). `STATE_VERSION` 8.
* **The reboot loop and its fix:** the BIOS's SET ADDRESS handler posts an attach event, and the USB task
  restarts the console into the base system unless the USB attach state machine has already found the
  cable (it checks once, ~3.7 s after a start, and marks the console as connected at ~5.2 s). A host
  that enumerates within a second of the pull-up (any PC) therefore restarted the console every ~0.9 s.
  The emulated host now waits until 6.5 s after each console start before it resets and addresses the
  device. Plugging into a running console still enumerates at once, so the BIOS restarts the console
  once, as it intends; after that it stays connected. Why the real PC is later on hardware is UNKNOWN
  (details in `io/usb.rs`).
* **Verified:** `examples/l2usb.rs` (cable in at power-on, and plugged in at 6 s with one restart; the
  user's SD card and flash): GET MAX LUN 1, INQUIRY "Leapster 2-RAM 1.00" / "Leapster 2-SD 1.00", C2,
  TEST UNIT READY, READ CAPACITY, READ(10) of both. On this PC with usbip-win2 (`usbip attach -r
  127.0.0.1 -b 1-1`), Windows installed "USB Mass Storage Device" VID_0F63 PID_1100 with the disks
  "LeapFrog Leapster 2-RAM" and "LeapFrog Leapster 2-SD", both "No Media" until C2, as the BIOS does;
  no console restarts. LeapLoader's app (same day, isolated copy of the card): found both disks, C2,
  TEST UNIT READY, the SD volume mounted (FAT, 1022 MB), 11 files (~25 MB) written in 26 s with no errors
  or stalls, flush, C1, C6; the files read back byte-identical from the saved card, which boots. About 7 s
  after C6 the BIOS restarts the console (it leaves USB mode).
* **Paused:** only the BIOS can answer the PC, so while Ribbit is paused and the host waits
  (`UsbDevice::host_busy`: the cable's change not yet taken, the first 6.5 s after a start, the connect,
  or queued transfers) the frontend runs the console silently at real-time speed, plus 0.3 s after
  (ribbit-frontend `emu.rs` `run_paused_for_usb`); a debugger breakpoint still holds it. Checked on this
  PC: attached while paused, Windows got both disks; ~0.15 of a CPU core while paused with Windows
  polling the empty drives.
* **Tests:** `io::usb::tests` (attach check, registers, `the_host_waits_for_the_bios_after_a_start`,
  `plugged_into_a_running_console_the_host_comes_at_once`).
