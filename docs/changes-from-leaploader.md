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
