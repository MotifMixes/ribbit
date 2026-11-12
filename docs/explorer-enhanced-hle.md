# Leapster Explorer Enhanced (new): its programs without the kernel (plan, 2026-11-12)

The Didj's Enhanced (new) (docs/didj-enhanced-hle.md) runs the Didj's ARM programs on Ribbit's ARM core and
answers their Linux system calls in Rust. The Leapster Explorer is the same LF1000 with a newer kernel (2.6.31)
and userland (uClibc 0.9.32, BusyBox 1.18, Brio, AppManager, Flash UI), so the same HLE (crates/ribbit-didj/src/hle)
runs it. What differs is new code next to the Didj's, used only when the process runs the Explorer's programs
(`Process::new_explorer`, `x.on`), so the Didj's HLE is unchanged:

| | Didj | Explorer |
|---|---|---|
| Root file system | JFFS2 (erootfs.jffs2) | UBIFS (erootfs.ubi, LZO): `ubifs_read.rs` |
| Data | /Didj (FAT in UBI) | /LF/Bulk (UBIFS; the `.lf2` packages installed as lfpkg does) |
| Start | `AppManager` alone | the real `/sbin/init` and `/etc/init.d/rcS` (daemons, sockets, LEGAL.png, startup sound) |
| Display | /dev/mlc, /dev/layerN | /dev/fb0..2 (lf1000fb.c): `hle/xfb.rs` |
| Sound | OSS /dev/dsp | ALSA /dev/snd (hw_params refinement as the kernel's): `hle/xsnd.rs` |
| Input | keypad event2 | power event0, keypad event1, touch screen event2 (tslib) |
| Programs talk over | message queues | also AF_UNIX sockets (/tmp/*_socket, lfd-control, /dev/log): `hle/xsocket.rs` |
| Factory data | none | NOR MfgData (/dev/mtd5..7, files here) |

Original mode (the real kernel, `explorerboot`) stays the reference for every check.

## Phases (each ends with a check against Original)

1. **File tree and start.** UBIFS reader (every node, newest by sequence number; LZO1X), Bulk from the
   packages, /dev, /sys and /proc as Original's kernel shows them; `/sbin/init` runs rcS.
   Check: `busybox ls -l` of /var and /LF/Bulk matches Original's listing; rcS reaches "Launching AppManager".
2. **Devices.** Frame buffers (MLC layers), 3D chip (the Didj's /dev/ga3d), touch screen, buttons, ALSA
   sound, I2C codec, NOR, power, RTC.
   Check: the startup sound matches /var/sounds/startup.wav sample for sample; AppManager starts its UI.
3. **The menu.** Check: Language Selection is Original's picture pixel for pixel; touch walks the first-time
   setup as on Original (`EXPLORER_INPUT` vs `EXPLORER_HLE_INPUT`), sound plays.
4. **A game.** Check: a game (a Bulk package or a cartridge) starts and plays; frames a second as Original.
5. **The app.** Emulation > Explorer: "Enhanced (new, experimental)" next to Original; saves in the usual
   explorer-<folder>.nandblocks/.nor; save states; the clock pauses with the app. Tests as `didj_hle_session`.

## Tools

`cargo run --release -p ribbit-didj --example explorerhle -- <folder> [program args]` (Git Bash:
MSYS_NO_PATHCONV=1). With no program it runs /sbin/init. EXPLORER_HLE_MS (host ms to run), _SHOT/_SHOTS,
_FRAMES, _WAV, _FPS, _TRACE (every system call), _CONSOLE_OUT=1 (/flags/console_out: AppManager prints to
the console), _INPUT (`ms:x,y`, `ms:lift`, `ms:<key>`). explorerboot (Original) got EXPLORER_TYPE (shell
commands for the serial console, separated by `;;`), used to read Original's /dev, /sys, /proc.

## Status

- 2026-11-12, phase 1 DONE. `ubifs_read.rs` reads erootfs.ubi (VERIFIED: ls -l of /var shows Original's
  names, sizes and dates; the root's data is LZO, Bulk's built by Ribbit uncompressed). uClibc 0.9.32 has the
  2.6.31 kernel's stat64 (64-bit st_blocks, times from 72; the Didj's 0.9.29 has them from 68). /sbin/init
  runs rcS: execve of `#!` scripts added (binfmt_script), mknod, mount (nothing to do), setsid,
  sethostname, uname (2.6.31-leapfrog), fstatat64, syslog (klogd blocks), /proc/uptime made when read,
  /proc/mounts lists /LF/Bulk mounted so /etc/init.d/mounts leaves it (no UBI to attach).
  Memory: Ribbit copies every page on fork and gives every process its own copy of each library, so the
  Explorer's HLE machine had 256 MB at first (64 MB since, see Memory below); the frame buffer is 18 MB at 0x0120_0000 (the Didj's
  FB_BASE_ADDR) outside the page pool (`Frames::with_pool`).
- Phase 2: AF_UNIX sockets (stream, datagram, socketpair, sendmsg/recvmsg without passed descriptors);
  closing isn't seen, so a peer never reads end of file (GUESS: not needed). libDisplay picks /dev/fb* when
  /sys/class/graphics exists (else /dev/dpc, the Didj's way). xfb.rs ports lf1000fb.c's probe, set_par,
  pan, blank, position, alpha, video scaler and fbmem.c's set_var/pan checks.
  ALSA: no mmap, so dmix (needs SysV IPC) fails and Brio falls back to plughw:0,0 as it does on any dmix
  error; Ribbit mixes every open PCM at 32 kHz (dmix's rate in asound.emerald.conf). VERIFIED: the startup
  sound (aplay /var/sounds/startup.wav) comes out sample for sample.
  The codec: /dev/i2c-0 (I2C_RDWR, I2C_SLAVE; the LF1000 adapter takes 8-bit addresses: i2c-ctl's 0x95
  finds the CS43L22 at 0x94, so rcS says "found Cirrus Logic codec" as on Original), its registers as
  Original's codec_reg shows them after boot, and the driver's four mixer controls (Master, Headphone,
  Speaker Playback Volume, Master Playback Switch) for lfd's volume; the app applies `I2c::gain` as in
  Original. /proc/asound/card0/id makes Brio find the codec. Input: evdev's EVIOCGBIT/EVIOCGKEY copy only
  the bitmap's length (tslib passes a larger len for a 4-8 byte buffer; writing len bytes broke its stack).
  munmap checks alignment and range as the kernel (alsa-lib's close passes garbage; unmapping it took
  AppManager's libraries).
- Crash at boot (2026-11-12, FIXED): with the host stalling (frames recorded, a busy PC) pid 123 (the startup
  video's process) crashed in uClibc's free() about 1 run in 2-15; the garbage munmap above was the same bug.
  `pace` jumped the programs' clock over a whole host stall, Brio's 128 ms ALSA buffer underran, its recovery
  filled the buffer without START, the writer blocked until the video stopped (DROP, EBADFD) and closing
  the PCM then freed a bad pointer. A catch-up jump is now at most 50 ms (`MAX_JUMP`), the rest is time lost
  as a pause; test `a_host_hitch_moves_the_programs_clock_by_max_jump_at_most`. Stress after: 64 explorerhle
  runs with frames under load (boot, menu, Sonic) and 8 didjhle runs, no crash, no garbage munmap.
- Phase 3 DONE (2026-11-12): every screen of the first-time setup is Original's pixel for pixel (frames of
  both runs matched, mean difference 0): Language Selection, Set the Date, Enter your name (empty and with
  MOTIF typed), Choose your grade, Is this correct?, the Leaplets store and the tutorial's games. Set the
  Time differs only by the clock (the HLE's is the host's date, Original's starts at 2008). The same touch
  script (EXPLORER_INPUT in million cycles, EXPLORER_HLE_INPUT in ms) walks both. The tutorial video plays at
  10-13 frames a second in both (HLE: fb pans per second; Original: 31 screen changes per 1000 M cycles =
  2.54 s). The startup video: Original shows about 22 frames, the HLE all of them (about 36): Original's
  processor is busy booting then (kernel, rcS), as the Didj HLE loads faster too.
- Phase 4 (2026-11-12): the tutorial runs the Leaplets (Brush, Train, Dino) and then Pet Pad (a Bulk
  package, Flash), whose frames match Original's where both caught the same moment (Brush exactly). A game
  started by the player's own touch: see below.
- Phase 5 (2026-11-12): Emulation > Leapster Explorer: "Enhanced (new, experimental)" / "Original"
  (setting `explorer_hle`). The session (frontend didj.rs `Hle::new_explorer`) builds the file systems from
  the NAND as Original's save leaves it (`system_from_nand`: the RFS and Bulk UBIFS read back), starts
  /sbin/init, takes the stylus and the volume buttons, applies the codec's volume. Saves: when the files
  changed (`fingerprints`), the root file system and Bulk go into the NAND kept as the usual
  explorer-<folder>.nandblocks (`write_to_nand`; since 2026-11-12 written to their UBIFS journals, see
  Saves below), the NOR from /dev/mtd5..7 into .nor. VERIFIED both
  ways: Original boots an HLE save (the rebuilt RFS mounts, 89% full uncompressed; a file put on /flags and
  one on Bulk are there; Language Selection as from its own save) and the HLE starts from Original's save.
  The RFS keeps the NAND's own /dev nodes (init's console) and rcS's /var/dev_nodes.tar: the HLE's /dev is
  not the kernel's (the NOR partitions are files), so the HLE makes /dev itself (/var/dev_nodes_doom).
  UBIFS images can now hold device nodes (`ubifs::Kind::Special`). Save states: the Process with the
  Explorer's devices (fb, ALSA streams, sockets, input) and the file tree as changes from
  `system_from_packages`; test `explorer_hle_states` (28 MB, 2.8 s the first time, load 0.6 s, the same
  picture after). Test `explorer_hle_session`: speed 99-100%, the save round trip above.
  Frame pacing: the Explorer's programs wait in short polls, so the app runs them in 100,000-instruction
  steps (1 M overshot the 1/60 s frames: 42% speed).
  Checked in the app (ribbit run --cart "Firmware Base.lfp" with explorer_hle on, a separate data folder):
  the Explorer starts in Enhanced (new) and its save is written on closing.
- Volume buttons (2026-11-12): lfd reads the keypad itself and sets the mixer, AppManager asks lfd over
  /var/run/lfd-control; both read the same /dev/input/event1, so every open input file now gets every
  event (evdev's clients, `Explorer::inputs`), and a socket whose other end no process holds any more
  reads end of file (lfd serves one connection until it ends). VERIFIED: one press of volume up takes the
  codec's headphone/speaker volume from cd/f3 to d6/f8, two to df/fc, as on Original.

- Merged (2026-11-12) with the Explorer cartridge and USB/saves work. One UBIFS reader (`ubifs_read.rs`, the
  saves work's: committed index from the master node, journal replay, partial reading from a save's blocks,
  LZO/deflate) serves the carry-over code and the HLE's file tree (`files`, `files_in_ubi`); it now also
  returns device nodes (`Kind::Special`). Its tests and the carry tests pass, and the HLE's screens still
  match Original pixel for pixel.
- Memory (2026-11-12): the HLE machine has the real 64 MB again. Anonymous memory (thread stacks, heaps,
  the main stack) is given pages on first touch (`Space::reserve`; a data or prefetch abort on a reserved
  page maps a zero page and runs the instruction again), and files mapped without PROT_WRITE share their
  pages with every map of the same contents (`Space::map_shared`, frames by CRC and compared, counted;
  mprotect adding PROT_WRITE gives copies, `privatize`; fork shares them). Through the setup, the
  tutorial and Pet Pad 33 MB of the 45 MB user pool are used (was 130 MB without). No copy-on-write for
  fork (forked shells copy their writable pages). Save states 14 MB.
- Phase 4 DONE (2026-11-12) with a Didj cartridge: from a save made in the HLE after the setup (Sign In,
  MOTIF), Sonic's icon shows in the menu's first square, a touch on it starts it, A through its logos,
  title and menu to Green Hill Zone 1. Against Original (explorerboot, the same save and cartridge, the
  same touches): standing in the level, 135 of Original's 138 distinct frames appear pixel for pixel in the
  HLE run (the rest within 0.03 mean); logos to the level 317 of 593 exactly and 59 more within 2 (fades
  caught at other moments). Running and jumping (Right held, A, Left): Sonic runs, collects rings and
  jumps in both (the key times differ, so positions differ). Frame rate: the HLE shows 31-37 frames a
  second (fb pans) standing and running; Original's screen changes 30-33 times a second while running.
  Pet Pad (a Bulk package) runs from the tutorial in both.
- Cartridges (2026-11-12): `Process::explorer_cart` puts a Didj cartridge in or takes it out: the kernel's
  view (the lfcart device, cart_hotswap 1 1, mtd10/11 Cartridge/ubi_Cartridge, /sys/class/mtd names,
  ubi2), read on Original with Sonic in; then cnotify's message (1, 0x40 on /tmp/cartridge) when the
  programs run. CartManager attaches (/dev/ubi_ctrl) and mounts it at /LF/Cart through the HLE (mount and
  umount2 of /LF/Cart; /proc/mounts as Original's). In at power-on, put in at the menu and taken out:
  the menu's first square shows Sonic and goes back to the empty slot, as on Original. In the app
  `set_cart` reaches the HLE (the Explorer tab's cartridges start it in Enhanced (new) too); test
  `explorer_hle_cart`.
- Saves (2026-11-12): written as the Explorer's kernel writes its files, to the UBIFS journal
  (`ubifs::journal`), so only the erase blocks that takes change. For each volume whose files changed
  (`fingerprints`, one per volume; the flight recorder's logs /LF/Base/FR, /dev and rcS's dev_nodes.tar
  are left as the NAND has them) the volume is mounted as the kernel would (`ubifs_read::mount`: committed
  index, log, buds), the HLE's files compared with it, and the difference written as nodes: data nodes for
  the 4 KiB blocks that differ (uncompressed), a truncation node when a file shrinks, inode nodes (the old
  inode's owner, flags and xattrs kept), directory entries (a deletion entry and an nlink-0 inode for a
  removed file, as replay.c drops them), then the touched folders' inodes (size and links counted again).
  They go to the base head's last bud while it has room, then to LEBs the committed LEB properties tree
  shows empty (small model read from the master node; not the GC LEB, not a bud already), each named by a
  reference node appended to the log after its last node (one minimum I/O unit each, as log.c writes
  them). The index, the LPT and the master node are not touched: the kernel's mount replays the buds and
  takes their space from them (set_bud_lprops). LeapFrog's root file system image is made for 406 LEBs
  with UBI's auto-resize flag: the first write grows it as the kernel's first attach and mount do (the
  volume table to 678 LEBs, read back from a save Original made, then the superblock's leb_cnt; the master
  node follows at the kernel's next commit, master.c). Each LEB goes to a free erase block with a higher
  sequence number and the old one is erased (UBI's atomic LEB change, `ubi::write_leb`; never-formatted
  blocks count as free). Both volumes are planned and then read back on threads of their own; when the
  read-back doesn't hold the files, or the journal can't take them (the log's last LEB and one more full,
  max_bud_bytes 8 MB of buds, fewer than 8 free LEBs, the big LPT model, a hard link changed), the volume is
  built again whole as before (`rebuild`), which also empties its journal. Saves on top of saves append to
  the same bud and log; after about 8 MB of changes one rebuild resets the journal (Original's kernel
  commits it itself when it runs).
  Carry-over: a journal save holds the buds and the log, not the master node or the index, so the folders
  above every written file get their entries and inodes again in the journal, and `ubifs_read` reads a save
  without a master node from the newest commit start node in its log; a file created after the commit
  (its inode's creat_sqnum) is read whole from the journal. `carry::explorer_user_files` on such a save
  finds the new files (checked in `explorer_hle_save_journal`); `explorer_files_beyond` reads the whole
  NAND as before.
  VERIFIED (test `explorer_hle_save_journal`): a file put on Bulk and one on /flags in Enhanced (new) make
  a 0.9 MB save (was 93 MB: every RFS erase block) written in 0.4 s on closing (was 1.0-1.8 s: now both
  volumes are read in about 0.15 s and read back after the writes in 0.15 s); Original
  boots it, its kernel mounts both volumes with no UBIFS error or recovery (dmesg), `cat` shows both files,
  `df` shows the root grown to 667 main LEBs, 62% used (89% when rebuilt uncompressed); a file Original
  writes there and the next Enhanced (new) session's changes on top of Original's own uncommitted journal
  (one file shortened, one removed) read back in both modes.
  Memory: the NAND's blocks are shared between the image a save is compared with and the running
  console's NAND (`Nand` copy on write: `Arc` blocks, `changes_from` skips shared ones), in Original and
  Enhanced (new): private memory at the menu 1086 -> 600 MB (Original), 1277 -> 780 MB (Enhanced (new));
  the built NAND alone 1043 -> 568 MB (test `explorer_memory`). Most of what is left is the one image:
  every block of Bulk and the RFS has a UBI header, so all 4096 blocks are allocated. Save formats are
  unchanged.

## Known differences (by design)

- No dmix: each program's PCM is mixed by Ribbit (same sum; buffer sizes are the program's own).
- No USB, LeapFrog Connect, FUSE (fuse-flasher) or networking: they need the kernel.
- The clock is the host's date (Original's RTC starts at 2008).
- What Ribbit writes to a UBIFS (journal nodes, a rebuilt volume) is uncompressed (its writer has no LZO);
  a rebuilt root file system fits (89%).

## Left

- Game input timed by the game's own frames (as the Didj's checks) for frame-exact play comparisons.
- SysV IPC and dmix are absent (Brio's fallback is used); sockets can't pass file descriptors.
- The player's check of the app (menu, touch, volume, saves, states).
