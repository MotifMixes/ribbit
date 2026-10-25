# Changes made during LeapLoader work

LeapLoader (an SD-card loader for the Leapster 2) is tested in Ribbit. Fixes found there are made in
Ribbit directly and listed here.

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
