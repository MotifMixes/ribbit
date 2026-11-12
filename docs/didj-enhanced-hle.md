# Didj Enhanced mode: running the Didj's programs without its kernel (plan, 2026-11-07)

The user's decision: Enhanced mode becomes a high-level emulation. Ribbit runs the Didj's own ARM programs
(AppManager, the menu, the games, every Brio library) on its ARM core, but answers their Linux system calls
itself instead of booting lightning-boot and the kernel, and later draws OpenGL ES itself instead of through the
3D chip. Original mode stays what it is now (real bootloader, kernel and chips) and remains the reference.

Until Enhanced-HLE runs all 16 games and the menu as well as Original, it lives behind a third choice in the Didj
menu ("Enhanced (new)"); today's Enhanced (LLE with patches) keeps working, and stays for good as the
in-between option (phase 7).

## Facts it rests on (VERIFIED)

- Kernel 2.6.20 with CONFIG_AEABI and CONFIG_OABI_COMPAT: system calls come as `swi 0` with the number in r7
  (EABI) or `swi 0x900000+n` (OABI). ARM private calls at 0x0F0000+ (set_tls 0x0F0005, cacheflush 0x0F0002).
- Userland: uClibc 0.9.29 (`ld-uClibc.so.0` the dynamic loader), NPTL threads, busybox.
- Ribbit's ARM core (ribbit-arm, ribbit-armjit) raises SWI through the vector at 0x08: the HLE catches the
  vector address, services the call in Rust and returns to LR. No CPU change.
- The programs reach the hardware only through device files, ioctls, mmap of device memory and sysfs/proc.
- /Didj is the FAT volume inside ubi_Brio (brio.rs reads and writes it), the root file system is JFFS2
  (jffs2.rs), cartridges are FAT inside UBI (cart.rs).

## Phases (each ends with a check that must pass)

1. **Process and loader.** Sparse 32-bit guest memory (pages), ELF32 loader with PT_INTERP, stack with argv,
   envp and auxv (AT_PHDR, AT_ENTRY, AT_BASE, AT_PAGESZ, AT_HWCAP), kuser helpers page at 0xFFFF0FE0
   (get_tls, cmpxchg, memory barrier). VFS: rootfs from erootfs.jffs2 (needs directory listing in jffs2.rs),
   /Didj from the Brio FAT volume, /tmp in memory.
   Check: `/bin/busybox echo hello` prints hello.
2. **Basic system calls.** brk, mmap2/munmap/mprotect/mremap, open/openat/close/read/write/readv/writev/lseek/
   _llseek, stat64/fstat64/lstat64, getdents64, access, readlink, getcwd/chdir, ioctl (TCGETS), uname,
   gettimeofday/clock_gettime, nanosleep, getpid/getuid..., exit/exit_group. Unknown calls logged once.
   Check: `busybox ls -l /Didj/Base` matches the volume.
3. **Threads and IPC.** clone (CLONE_VM|CLONE_THREAD|SETTLS|CHILD_CLEARTID) as guest threads on Ribbit's
   scheduler, futex (WAIT/WAKE/REQUEUE), set_tid_address, signals (rt_sigaction, rt_sigprocmask, tgkill, kill,
   rt_sigreturn/sigreturn, pthread_cancel), pipes, poll/select, AF_UNIX sockets, POSIX message queues (mq_open,
   mq_timedsend/receive; the Brio event system uses them), fork/execve/wait4 for launch scripts (or replace
   launch_main with Rust).
   Check: AppManager starts and reaches "PushApp Done: BLT/BLT.so" as in Original's log.
4. **Devices** (reuse Ribbit's models wherever the program maps device memory): display layers /dev/layer0..2
   (MLC ioctls, type 'm'), 3D chip registers and memory for libopengles_lite (Ribbit's ga3d.rs), OSS /dev/dsp
   and the mixer (to Ribbit's audio), /dev/input/event* (keypad: Linux key codes, power button, headphone and
   cartridge switches), sysfs/proc the libraries read (board_id 3, battery/power, cartridge state, clock,
   needs_repair flags, /proc/meminfo with mem=18M or Enhanced's larger memory), RTC.
   Check: the menu appears and a cartridge game and JetPack Heroes play, picture and sound matching Original.
5. **Ribbit's own 3D drawing, at the chip level** (user, 2026-11-07, instead of rewriting GLES): keep the Didj's
   libopengles_lite.so (exact game behaviour; the menu, JetPack and Clone Wars alone import 63 gl* functions,
   with matrix palettes, point sprites, fog, lighting, compressed textures) and draw the 3D chip's commands
   (GTE vertex programs with their matrices and points, TSE triangles) with Ribbit's HD renderer at 1x-4x.
   More than 60 fps: in-between frames from blending the GTE constants (matrices) and points of two frames.
   Check: every game's 3D at 1x matches the chip model pixel for pixel on reference frames; 2x-4x look right;
   in-between frames keep the game's pace.
6. **The app.** "Enhanced (new)" in the Didj menu; saves: the HLE /Didj writes back into the same ubi_Brio
   volume Original uses, so a save moves between modes. USB/LeapFrog Connect needs the kernel: the app says so
   and offers Original for it. Save states for the HLE machine.
7. **Three modes, kept for good** (user, 2026-11-07: "an in-between option as well"): Original (real kernel,
   reference), Enhanced (real kernel with the LLE patches: repair, memory, faster CPU, HD; keeps USB and
   LeapFrog Connect) and the HLE (no kernel: fastest, own OpenGL ES). The HLE drops "(new)" from its name when
   all 16 games and the menu pass phase 4-5 checks; the LLE Enhanced is NOT retired.

## Status

- 2026-11-07, phase 1 DONE: `crates/ribbit-didj/src/hle/` (mem.rs sparse pages with the MMU off, elf.rs,
  vfs.rs one in-memory tree from erootfs.jffs2 via `jffs2::tree` + /Didj from the packages as
  `nand_from_packages` lays out the Brio volume + /tmp; mod.rs system calls and kuser helpers caught at their
  addresses). `cargo run --release -p ribbit-didj --example didjhle -- didj /bin/busybox echo hello` prints hello
  (Git Bash: MSYS_NO_PATHCONV=1). DIDJ_HLE_TRACE=1 logs every call.
  Found: uClibc's ld.so uses the old stat/fstat (106/108, 64-byte struct), busybox calls setgid first.
- Phase 2 DONE: file ops (open/creat/unlink/mkdir/rmdir/rename/symlink/link/truncate/pread/pwrite/chmod/utime/
  statfs/getdents/openat), times. VERIFIED: struct stat64 here has a 32-bit st_blocks after __pad4 (mtime at 76;
  busybox's own date shows right in `ls -le`). The kernel picks the ABI per call by the `swi` form (OABI compat
  table), so the HLE does too (`call_eabi`). uClibc 0.9.29 passes ftruncate64's length in r1 on an EABI call,
  so the real kernel (and the HLE) truncates to 0: `dd seek=` makes empty files.
- Phase 3 mostly there (hle/task.rs): processes and threads (fork/vfork/clone, execve, exit/exit_group,
  wait4, CLONE_*TID), futex WAIT/WAKE/REQUEUE/CMP_REQUEUE/WAKE_OP, pipes, nanosleep/yield, POSIX message
  queues (mq_open/unlink/timedsend/timedreceive/getsetattr), sched_* (stored only: round robin, no
  priorities). busybox sh scripts with pipes, `$(...)`, sleep work. Not done: signal delivery (handlers
  stored, fatal signals end processes; DONE 2026-11-08, see Status), poll/select, AF_UNIX sockets,
  mq_notify, O_CLOEXEC on exec.
- AppManager (`. /etc/profile; exec AppManager`) loads Brio, makes its event queue and tasks, then asserts
  "CButtonModule::LightningButtonTask: cart read failed": first missing file is
  /sys/devices/platform/lf1000-nand/cartridge. Phase 4 (devices, sysfs, /proc) starts there.
- Open: `grep -r /etc` runs forever (loop not found yet).
- Phase 4 started (2026-11-07): hle/sysfs.rs (the /sys and /proc files AppManager reads, values VERIFIED by
  `cat` on Original over the serial console), hle/dev.rs (/dev/input/event0..2 with their real names, OSS
  /dev/dsp paced by the clock in whole 4 KB fragments, null/zero/random), poll and _newselect (look again
  every 2 ms). jffs2::tree now keeps device numbers (a device inode's isize is 0). PortAudio primes /dev/dsp
  with non-blocking writes until EAGAIN, so a full buffer must give EAGAIN. AppManager now gets past
  buttons, power and sound and asserts "DisplayModule::GetScreenSize: ioctl failed" (/dev/mlc,
  MLC_IOCGSCREENSIZE). The driver source for every device is in local/didj-src (Linux 2.6.20 LF1000 tree:
  drivers/lf1000/{mlc,dpc,ga3d,i2s}, include/linux/lf1000/*_ioctl.h); the shipped kernel matches it
  (same sysfs files). Next: MLC/DPC/layer ioctls into Ribbit's MLC model; mmap of /dev/layerN takes a
  physical address as the offset (io_remap_pfn_range), so the HLE needs physical memory pages shared by
  all processes and the display model.
- DECIDED for phase 4 (2026-11-07): the HLE moves onto the Didj machine itself (machine::Didj: Bus, JIT,
  every device model) with the MMU on and Ribbit as the kernel, instead of its own sparse memory and the
  interpreter (about 10 MIPS, too slow for games). Why: mmap of /dev/layerN, /dev/ga3d and block
  addresses (bit 29, `mlc::block_sdram`) then reach the real models through page tables with no copying,
  the MLC/GA3D/DMA code is the one Original uses, and the JIT runs the programs (it already flushes on a
  TTB change, cp15.rs). How: Ribbit writes each process's page tables in a reserved part of SDRAM and
  switches TTB on a task switch; user pages come from SDRAM outside the frame buffers (FB_BASE_ADDR
  0x0120_0000 to 32 MB: RGB, YUV, OGL, include/asm-arm/arch-lf1000/mlc.h) using Enhanced's 64 MB;
  the run loop catches the SWI vector (0xFFFF0008) as now; IRQs stay off (user mode can't change the I
  bit). hle::mem::Mem becomes the page-table owner; vfs, task, dev and sysfs stay as they are; ioctls on
  /dev/mlc, /dev/layerN and /dev/dpc write the MLC/DPC registers as drivers/lf1000/mlc/mlc.c does.
- 2026-11-07, the move DONE: hle::Process holds a machine::Didj (64 MB SDRAM, its JIT) and runs every
  thread on it with the MMU on. mem.rs: level-1 tables at 0 (16 KiB each, 60 processes), the shared
  top-megabyte table at 0xF0000 and the vector page at 0xF1000, user pages from 1 MB to 0x0120_0000 and
  32 MB to 64 MB (`Frames`); `Space` (map, unmap, fork copies, release, Rust's reads/writes, which tell the
  JIT's code watch). Aborts land on the vectors and stop the run (`Crashed` names the FAR). The TLB is
  flushed on a thread switch or a table change, and the CP15 generation is counted across all threads'
  processors (each thread has its own `Arm`; equal generations made the JIT keep another process's
  tables: forked shells crashed). DIDJ_HLE_NOJIT=1 interprets. Checks 1-3 pass again; AppManager reaches
  the same GetScreenSize assert at about 440 M instructions/s (was about 10 M).
- Display DONE (2026-11-07): hle/mlc.rs ports mlc.c's register functions (its quirks kept) for /dev/mlc
  (251,0) and /dev/layer0..2 (250,n), and the probe's set-up at start. mmap of /dev/layerN, /dev/ga3d
  (249,0, the 3D registers at 0xC001A000) and /dev/mem (1,1) maps those physical pages (`Space::map_phys`;
  not given back to the pool, shared on fork). `Didj::hle_tick` runs the 3D chip and DMA and clears the
  MLC dirty flags each frame. AppManager now passes phase 3's check ("PushApp Done: BLT/BLT.so") and the
  menu draws: "Select your Country" after 3 billion instructions (15 s, `DIDJ_HLE_SHOT=menu.png`).
  Not done: write() to /dev/layerN, /dev/dpc, AF_UNIX sockets (281/289: socket fails, send(-1)),
  _sysctl (149). Next: buttons from the example to walk the menu, then a cartridge game and JetPack Heroes.
- Buttons DONE (2026-11-07): `Process::key(code, down)` queues EV_KEY + EV_SYN on /dev/input/event2 with
  the keypad driver's codes (`hle::key`, drivers/input/keyboard/lf1000-keypad.c: arrows, A=KEY_A, B, L, R,
  Menu=KEY_M, Hint=KEY_H, Pause=KEY_P, Brightness=KEY_X) and EVIOCGKEY reports the keys down. The example
  takes `DIDJ_HLE_KEYS=a@3200,a@4000` (key at N million instructions, held 50 million): A on "Select your
  Country" opens "Is this OK? USA", A again opens the name keyboard (7 billion instructions, 31 s).
  Not handled: DPC_IOCTCONTRAST (/dev/dpc 252,0, ioctl 40046407). Next: a cartridge game.
- Cartridge DONE (2026-11-07): `Process::insert_cart(dump)` puts the volume at /Cart (as rcS mounts it), the
  sysfs files of a production cart (cart_id 7, cartridge "production", scratchpad 0x1C60: from lf1000.c
  `show_cart` and gpio_main.c, NOT checked on Original) and the keypad's cart switch (SW_TABLET_MODE, EVIOCGSW).
  /Didj and /Cart match names in any case, as vfat does (`Vfs::fat_match`; Clone Wars asks for Data/0/JDI,
  the save has jdi). Example: `DIDJ_HLE_CART=<dump>`, `DIDJ_HLE_CARRY=<.nandblocks>` (the player's files
  from an Original save via carry::user_files), keys `a@3000+400` (held 400 million).
  Star Wars: The Clone Wars: "Select a name" (A@3700) -> wheel, Down@5600 -> "Select a Game" (the cart's art),
  A@6400 -> A@7600 starts it: Lucas/Realtime logos, title, attract demo (Kashyyyk-1.dmo) and, with A on the
  title (presses every 400 million from 8400), main menu, story text, cutscenes C1S1-3 and the first level
  as Anakin by 15 billion instructions (65 s, about 230 M/s). A during the attract demo does nothing (on
  Original: UNKNOWN). Not yet compared with Original: picture frame by frame, sound, pacing.
  Next: JetPack Heroes, then sound and timing against Original.
- JetPack Heroes DONE (2026-11-07), no HLE change needed: with no cart it is first in "Select a Game";
  keys `a@3700+400,down@5600+400,a@6400+400,a@7600+400` start it (intro video, 186 frames in 15.5 s), its main
  menu by 11 billion instructions, A on Start Game (a@11000, a@13000, a@15000) loads the first level (Neutrino
  with the player's Didji, gems, HUD) by 17 billion (70 s, about 240 M/s). It logs "could not find
  Neutrino_..._suckUp000N.png", "parseSpriteCel ID number is wrong" and "getAvatarTexturePiece - Could not find
  component ..."; whether Original logs the same is NOT checked. Phase 4's picture/sound check against Original is
  still open. Next: sound and timing against Original.
- Sound and start-up timing (2026-11-07): `Process::take_sound`; didjhle `DIDJ_HLE_WAV=<file>`, `DIDJ_HLE_LOG=1`
  (when each block of sound arrived, host ms and instructions), `DIDJ_HLE_FRAMES=<folder>` (the screen when it
  changed, named by host ms, with the MLC registers on stderr), `DIDJ_HLE_PCS=<from>-<to>` (thread and PC every
  million instructions); didjboot prints when PCM out starts and the MLC registers with each frame. The HLE's
  sound and clocks follow the host clock, Original's WAV is in emulated time, and neither WAV holds the silence
  before the first sound (compare by frames, or by the loudness every 10 ms).
  FIXED: the didj logo animation was hidden behind the copyright screen. AppManager asks for video priority 3;
  the LF1000 build defines VID_PRIORITY_INVALID as 3 (asm-arm/arch-lf1000/mlc.h), so the driver refuses it and
  the video stays in front (Original: CONTROLT 0xC02); the HLE took it (0xF02). Hiding it had also cut the time
  to "Select a name" by about 0.8 s.
  Now, with the user's save and no keys: the same screens and sounds as Original, all about 3.5 s sooner (the
  bootloader and kernel; the health warning is the bootloader's picture, so the HLE never shows it):
  copyright 0.24 s (Original 3.72), logo 3.28 s (7.0), "Select a name" 10.8 s (about 14). Sound starts at
  0.18 s (Original 3.44 s); after that the splash sound is 0.35 s earlier and the menu voice 0.55 s earlier
  than Original, measured from each WAV's start.
  Also: the kernel has HZ 100 and no clock events, so a sleep ends on the first tick after its deadline;
  nanosleep now does that (`tick_after`). It changed nothing measurable here. AppManager's main thread
  busy-waits on gettimeofday while the copyright shows (fopen/fscanf of the USB vbus file each pass).
- Boot splash (2026-11-07, the user's choice): the HLE shows the health and safety warning (bootsplash.rgb from
  kernel.bin's TFS, 320x240 B8G8R8, layer 0 as lightning-boot's mlc_set_video_mode sets it, CONTROL0 0xC6539020
  as on Original) and holds the programs for BOOT_TIME (3.48 s) meanwhile; the clocks count from power-on as on
  the real Didj. `hle::boot_splash_from_packages`, `Process::boot_splash`; didjhle shows it unless
  DIDJ_HLE_NO_SPLASH=1. Checked: the warning at 10 ms (Original about 0.05 s), the copyright at 3.70 s (3.72).
- JetPack Heroes timing against Original (2026-11-07). Tools: didjboot `DIDJ_FRAME_CYCLES=<millions>` checks the
  screen by cycles (DIDJ_FRAME_EVERY counts instructions, so screen times were wrong while the processor idled; use
  DIDJ_CARRY, not DIDJ_SAVE, with the app's save: DIDJ_SAVE stalled after the copyright); didjhle `DIDJ_HLE_FPS=1`
  (frames shown and million instructions each host second, as DIDJ_SPEED), key presses print host ms.
  Original in gameplay: 33 frames a second with the processor 73% busy, so not CPU-bound; 33 and 25 fps are 3 and 4
  ticks. The HLE showed 41: three FIXES, all from the 2.6.20 kernel source:
  1. The 3D chip, DMA and the MLC's frames (dirty flags) went on by instructions run, so frames came at the HLE's
     instruction rate and stopped while every thread slept; now they follow the host clock (`Process::tick_machine`).
  2. nanosleep of 0 was a yield; 2.6.20's do_nanosleep has no shortcut and low-res hrtimers fire from the tick, so it
     waits for the next tick. The game's main thread loops on it (about 15,000 a second as a yield).
  3. poll/select wait ceil(t/10 ms) ticks from the current tick (fs/select.c), futex and mq_timed* ceil+1
     (kernel/futex.c, ipc/mqueue.c prepare_timeout): `ticks_from_now`, `jiffies`.
  Now (fps, HLE vs Original): main menu 50 vs 50, Choose Your Hero 33 vs 33-35, level start 50, 48, 33 vs 50, 50,
  40, 33, then 25 in both. Intro video 15.7 s at 12 fps in both. After A on "Select a Game" (before the fixes):
  title art 0.64 s vs 1.47, "Loading" 3.72 vs 4.78, video 5.06 vs 7.23; A on the mood to the level 3.45 vs 4.62:
  the HLE loads faster (no NAND or kernel file system work); kept, it is the HLE's point. Original shows a white
  screen after A where the HLE shows black.
  OPEN: in the first level with no input, Original drops from 33 to 25 fps about 26 s after the level appears (96 s,
  processor 56% busy, nothing changes on screen), the HLE about 12 s after. Not drawing load; some timed event in the
  game (sound ending? a timer?) comes sooner in the HLE. The HLE also runs about 420 million instructions a second
  there against Original's about 290 (thread 1, code at 0x4021xxxx and 0x4044xxxx; likely polling hardware faster).
  EXPLAINED (2026-11-07): about 87% of the main thread is libopengles_lite.so 0x3E320 (reads the 3D chip's queue
  registers; mode 2 works out the read position with __umoddi3, which is the 0x4044xxxx code in libgcc_s) in a
  loop waiting for the 3D chip. So the extra instructions are only faster spinning. The level's 3D work is about
  3.9 million chip cycles a frame (26.5 ms at 147.456 MHz) at 33 and at 25 fps alike, so a frame is about 30 ms
  of chip and processor time: right on the 3-tick edge. Any delay tips it to 4 ticks, and it stays there. HLE runs
  dropped after 9, 12 and 32 s, or not in 55 s (host timing jitter); Original's 26 s is deterministic but just as
  near the edge. Not a bug to chase further; what real hardware does is UNKNOWN (ask the user to check).
  FIXED with it: `hle_tick` set `frame_cycles` to 0 when a frame came, losing the rest; HLE ticks span whole
  sleeps, so frames slipped onto the 10 ms ticks. The rest is now kept (`%=`).
- No lag, same pace (2026-11-07, the user: "any fps, keep the speed"): the HLE ran the 3D chip 4 times as fast
  (REVERTED 2026-11-08 to the real speed, see the comparison below)
  (`Ga3d::speed_up`, didjhle DIDJ_HLE_GA3D=<n>, 1 = real). Checked by holding Right for 3 s of host time:
  JetPack 25-33 -> 60 fps, same distance (231 vs 224 px at 1x/2x); Clone Wars first level 20 -> 38-41 fps
  (55-62 while turning), same place at the end (frames differ by 1.0 of 255, as much as 0.4 s apart standing).
  Both games move by the clock up to 60 fps. Above 60 (2026-11-07, didjhle DIDJ_HLE_MLC_HZ=<n> gives the MLC n
  frames a second, DIDJ_HLE_TICK_US=<n> the kernel tick): the MLC's 60 Hz frame is the only cap (1 ms ticks
  change nothing: 60 fps at MLC 60, 120 at MLC 120 with 1 ms ticks, about 100 with 10 ms). But JetPack then
  moves too fast: Right held 2 s scrolls 61 px at 60 fps, 96 at about 82, 108 at about 90 (world, from the
  frames). So it counts at least one 60 Hz frame per frame shown: clock-timed below 60, frame-locked above.
  MLC 60 and HZ 100 stay; more than 60 fps must come from in-between frames in the phase 5 renderer.
  didjhle: `DIDJ_HLE_KEYS_MS=1` (key times in host ms; menus wait, so instruction times drift), `right@N+3000ms`,
  `DIDJ_HLE_UNTIL_MS`, `DIDJ_HLE_FRAMES_FROM`. FIXED: cart::files took only NAND dumps; a bare FAT volume
  (as the app keeps some carts) now works too. Clone Wars keys: see the session's script (New Game needs Down
  then A on "are you sure?").

- Phase 5 started (2026-11-07): HD drawing works in the HLE as it is (didjhle DIDJ_HLE_HD=<n>, JetPack 2x right).
  In-between frames, first version (`ga3d/tween.rs`): each frame's drawing is recorded (clipper triangles in clip
  space, TSE triangles/rectangles, fills, with registers 0-63), frames end at the MLC flip; a picture at t
  draws the newer frame again into its own buffers (saved, then put back) with positions blended towards the
  older one's, matched in order by kind, render state and textures (not the render target: two buffers take
  turns). Moves over 48 px or behind the camera are not blended. HD off while it draws (console size for now).
  didjhle `DIDJ_HLE_TWEEN=<n>`. JetPack with Right held: 4-6 px a frame scroll comes out in about 1-2 px steps.
  HD in-betweens: the HD store's tiles under the saved buffers are saved and put back too (`Hd::save`,
  `Hd::restore`); JetPack at 2x: 4/8/12 px a frame comes out in exactly even 1/2/3 px steps, as sharp as frames.
  Left: showing them in time (app, phase 6), Clone Wars (3D) check.
  FIXED (2026-11-07): Clone Wars' menus and cutscene video showed half of each 2D quad, in Original too (so in
  the released app): libopengles_lite starts the TSE directly with 0x402 then 0x602 per quad, and the inverse
  culling flag is bit 9 of GRP3D_CONTROL, not bit 8 (ga3d.rs). JetPack and the Didj menu draw through the
  primitive processor (flag in PPARAM bit 3) and are unchanged.
  Clone Wars in-betweens (2026-11-07): the first level's run and camera turn blend in even steps (mean change
  about 12 a picture, none at the real frames), every operation pairs, about 1500-2250 of 4100-4700 clipper
  triangles move. FIXED on the way: LUT fills weren't recorded, so the redraw drew with the last palette loaded
  (purple floor, faded HUD); they're operations now and the palette is put back after. Text that changed (the
  "A" icon coming and going) paired letters with other letters of the font and blended them across: same key
  but other texture coordinates now pairs only when no exact match is within RESYNC. Keys: the menu keys are
  instruction-timed (host-ms keys miss: the name screen came 5.3-9.5 s in), the old script plus A every 500 M
  from 30500 to 40000 reaches the first level at GA3D=4; Right from 41500.
- Phase 6 started (2026-11-07): "Enhanced (new, experimental)" in Emulation > Didj (settings `didj_hle`, with
  `didj_enhanced`; `didj_smooth` "Smooth motion", on by default, at once). ribbit-frontend didj.rs `Hle`: the
  session's thread runs the Process until each 1/60 s deadline (1 M instruction steps), buttons as keypad keys,
  /dev/dsp sound through the same resampler, AppManager stopping/crashing = a power-off (on again, left off after
  3 quick ones). In-betweens in time: the picture at `frame_between((now - t1) / (t1 - t0))` between the last two
  flips (one game frame behind; not over gaps > 150 ms). The HLE's clock pauses with the app
  (`Process::set_paused`: every Instant::now() in hle/ is `clock()`, the host clock less time paused).
  Saves: the player's files (/Didj files at paths the packages don't put there) are built into the image again
  (`nand_with_files`, ~1 s, only when they changed) and kept as the usual .nandblocks; read back with
  `carry::files_beyond` (whole image, so `user_files`' known-LEB test isn't needed). Test
  `didj::tests::didj_hle_session` (RIBBIT_DIDJ_DIR, ignored): boots at 99-101% speed, a file round-trips.
  USB says it needs Enhanced/Original. Autostart of a library game VERIFIED (test
  `didj_hle_autostart`, the app's MOTIF save, JetPack: My Games at 15 s, JetPack's main menu at 40 s).
  Save states DONE (hle/state.rs): the Process in bincode (deflated, first word "HLE" 1, so Original's
  and the HLE's states refuse each other), the file tree as changes from the built system + cartridge (the
  player's files and installed games go in the state: ~45 MB with the MOTIF save, as Original's NAND changes),
  times from the programs' clock, shared pipes/queues written once then by number (bincode::serialize_into:
  serialize's size pass would mark them all written). The clock stands still while a state is made or
  loaded. Found with it: CLOCK_REALTIME/gettimeofday read the host's date, so a load (or the app's pause)
  moved realtime-timed programs on (Clone Wars' crawl); now `epoch` + the programs' clock. Test
  `didj_hle_states` (RIBBIT_DIDJ_CART/SAVE/KEYS/WAIT/SHOT_DIR): Clone Wars' crawl saved and loaded, picture
  identical, state 1.5 s (first: builds the base tree), load 0.3 s.
  Left: player checks in the app (menu, JetPack, Clone Wars, smoothness, states).
- Cartridge sweep (2026-11-08, didjhle, the app's MOTIF save carried, Clone Wars' key path then A every 800
  million to 15 billion): all 14 other carts start and run with no HLE change: Didji Racing (Raceway garage),
  Foster's (phone keypad), Hannah Montana, High School Musical (story scenes), Indiana Jones (first level),
  Nancy Drew (room), NeoPets (name keyboard), Nicktoons (SpongeBob scene), Sonic (title, "Press A"), SpongeBob
  (story), Jedi Trials (first level), Super Chicks, Tinker Bell (tutorial), Wolverine (comic intro). Indiana
  Jones, Nicktoons, Sonic, SpongeBob and Jedi Trials sleep most of the time (~45 M instructions/s, sound as
  long as the run): paced by the clock, not stuck.
- Compared with Original (2026-11-08; scratchpad scripts: both runs with no input once the cart starts, Original
  by autostart, frames by cycles; each HLE screen held 0.3 s matched in order to Original's): every cart's
  screens are Original's pixel for pixel (80x60 mean difference 0; the rest are fades or attract demos caught
  at another moment). Found and fixed:
  - Nancy Drew showed black between its logos, Original white: every layer off shows the MLC background, which
    lightning-boot sets white (mlc.c) and the kernel keeps; the HLE now sets it in `mlc_init`.
  - NeoPets' title fades (51 pictures, a step when 40 ms have passed by gettimeofday) took 2.5 s, Original 2.0;
    its still screens 4.06 s, Original 4.84. Cause: the 3D chip at 4x. Its draw loop (flip; draw; nanosleep(0)
    until MLC_IOCQDIRTY is clear) then took 20, 20, 10 ms with the 60 Hz frames, so 40 ms came 50 ms apart.
    At 1x it is 40 ms, 2.0 s and 4.84 s, as Original. The user chose the real speed for every game, smooth
    motion from in-between frames (JetPack back to 25-33 game frames).
  - The programs' clock followed the host's while code ran (the same work read 0.2 ms one frame and 0.4 the
    next) and the host woke late from sleeps. Now `clock()` goes on with the instructions at 393 MHz from where
    it last met the host (`anchor`), the scheduler's sleep ends with the clock exactly on the deadline
    (`on_time`; up to 100 ms of host lag caught up by shorter sleeps, more is lost as a pause), and `pace()`
    waits when the programs run ahead or jumps when more than 20 ms behind. Frame gaps are now exact (40.0 ms).
  Timing: the games line up with Original within 1-1.5 s over their intros (the start differs by the keys);
  loading is host-bound (about 344 M instructions/s against Original's 394 in Didji Racing) or faster (Clone
  Wars' logos 2.6 s, Original 3.1: files come without the NAND, UBI and FAT work). Sound, lined up by the
  picture: correlation 0.6-0.87 within 0.35 s, the same level (Didji Racing, Super Chicks, SpongeBob, Nancy
  Drew, Nicktoons, Sonic, Jedi Trials, Fosters, Indiana Jones, Hannah Montana, NeoPets); Clone Wars, High
  School Musical, Tinker Bell and Wolverine not lined up (Original's autostart started them ~8 s later).
- Gameplay with input (2026-11-08, Sonic, Green Hill Zone 1): didjboot and didjhle press keys from when a
  screen shows (DIDJ_KEYS_AFTER / DIDJ_HLE_KEYS_AFTER + DIDJ_HLE_THEN, several screens separated by ";"),
  in ms or, from a given list on, in frames shown (DIDJ_KEYS_FLIPS / DIDJ_HLE_KEYS_FLIPS: ms-timed keys
  reached the game a frame apart and changed Sonic's jumps). A on the SEGA logo's screen to the level, then
  20 s of running, jumps, Left and Down by frames: Original against Original and the HLE against the HLE
  repeat exactly; the HLE against Original matches pixel for pixel over 22 s except a few single frames
  where one is a physics step ahead, and 1.5 s where the HLE hits a badnik one frame sooner (score 100 a
  frame early), so the bounce goes a few pixels another way until Sonic lands, then identical again.
  Ruled out: frame timing (both flip on exact 30/40 ms, Linux's tick is 10.0 ms: 45930 at 4.59375 MHz; the
  HLE's flips logged with the programs' clock, `Process::last_flip`), the button driver (lf1000-keypad
  reports in its GPIO interrupt, no polling delay), the kernel's time (Original's PCs during play: idle
  loop 85%, the game 14%, other kernel code 0.3%; charging each system call 5 or 20 us changed nothing at
  that spot), the date (the HLE's comes from the host, yet its runs repeat). Left: a sub-frame difference
  in when the game reads the clock within a frame; not visible in play.
  didjhle's frames were taken after 10-million-instruction steps, 200 ms apart while a game sleeps (Sonic
  looked like 5 fps); they are now looked at every million while recording.
- Idle shutdown (2026-11-08): left on the menu, AppManager dims the screen (/dev/dpc
  DPC_IOCTBACKLIGHTVIRT) and after 420000 ms "System shutting down due to inactivity", shows "Your Didj
  is shutting down." and calls Brio's Shutdown(), which is exit(0) (libPower.so; launch_main then runs
  poweroff). Original powers off at 433 s from power-on, the HLE's AppManager exits at 423 s (no boot).
  "ShutdownTimeMS" is not the idle time: it is monitord's 12 s grace after a power or USB event
  (opendidj packages/monitord), which the HLE has no part of (its socket fails). Fixed on the way:
  - Signal delivery (hle/signal.rs): AppManager's exit cancels its polling thread (NPTL: tgkill SIGCANCEL
    32, si_code SI_TKILL) and joins it; the signal was dropped, so it waited for ever. Handlers per
    process (reset on exec, copied on fork), masks and pending signals per thread and per process, the
    kernel's frames (sigframe 752 bytes, rt_sigframe 880: siginfo, ucontext with sigcontext at +20 and
    the mask at +104), sigreturn/rt_sigreturn (or the vector page's KERN_SIGRETURN_CODE without
    SA_RESTORER), rt_sigprocmask; a waiting call ends with EINTR, or runs again after the handler with
    SA_RESTART (never poll/select). busybox sh's trap and wait interrupted by a signal behave as on
    Original (wait returns at the signal with the same status).
  - A process that ended never gave back its pages and page table (60 tables: a shell loop's forks
    failed with "cannot fork"); end_group releases them.
  - When every thread waits for a time more than 10 ms off, `run` returns (Budget) and waits for it in
    steps (`wait_until`) instead of sleeping inside: the app kept no frames and buttons meanwhile, and
    didjhle's steps took minutes once only a 2 ms poll loop was left.
  - /dev/dpc (252,0): backlight (virtual -128..127 and physical), brightness and contrast kept and read
    back; nothing drawn (nor in Original).
  "Turn off when idle" (the Leapster's setting, off by default) now covers the Didj, all three modes
  (ribbit_didj::idle): CAppManager::UpdateInactivity in libLightningBase.so adds the time since its last
  call to the static inactivityTimer (offset 0x48458), zeroed by a button: past 120 s it dims, past 420 s
  (600 s paused) it shuts down. Off: about once a second Ribbit zeroes the timer through the running
  process's page tables (library found by its ELF header, the function's first instructions and the
  relocated literal; other firmware is left alone). On: it shuts down, stays off (a timer reading of
  400 s or more tells it from a cartridge update's power-off) and the app closes it. Test
  `didj::tests::didj_idle` (RIBBIT_DIDJ_DIR, ignored, 9 minutes): on, off at 424-434 s and still off; off,
  still on after 9 minutes, speed 100%, on Original, Enhanced and Enhanced (new).
- Didji Racing (2026-11-08): the HLE's frames show the menus, mini-games and race as Original does, also
  with HD 3x, by eye at full size (the two runs' rounds and moments differ, so not pixel for pixel). The
  faults were in the in-between pictures (ga3d/tween.rs), found by how far one is from both its frames
  (scratchpad twm.py), from 13.6 above the frames' own difference to 0 on the mini-games:
  - The row of letter tiles slid half a tile when the cursor blinked: identical tiles, drawn in another
    order, paired by order with their neighbours. Now each pairs with one about in its place
    (`nearest_alike`: within 6 pixels, the same picture first). Only about in place, so a fast scroll over
    repeating tiles keeps its order.
  - The swinging hose broke into shards: a new animation frame's pieces each flew to another's place. One
    showing another part of the texture now blends only as a whole and not far (`blends`, also in
    `blend_clip`: 12 pixels, corners within 1.5), otherwise it is drawn where it ends.
  Checked: Sonic at speed, right by eye. Not checked with in-between frames: the race itself (the window
  missed it), moving 3D games. Left: when Tune-Up's timer ends, one in-between picture shows two white
  bars by the emptied avatar panel (not in either frame). The player's white square on that panel (report
  of 2026-10-31) is not this: it came on 1.7.0, before in-between frames; still not reproduced.
- Release checks (2026-11-12; didjhle and didjboot, the MOTIF save, the same keys from a trigger screen):
  - /dev/dsp now answers as the LF1000 driver (i2s/main.c): SPEED 32000 whatever is asked (it took any
    rate before), CHANNELS 2, SETFMT S16_LE, SETFRAGMENT 8 fragments; the buffer is 8 x 4096 bytes (was a
    GUESS of 4) and a blocking write takes one fragment. Every cart asked 32000 anyway (logged once, "dsp:
    N Hz asked"), so Sonic's sound was not at a wrong rate; the 32000 -> 16000 lead is Brio's own mixing,
    the same in Original. With 8 fragments the HLE's sound lines up with Original: a Nicktoons menu move's
    sound starts 265-270 ms after the picture (Original 235-260; with 4 fragments it was ~128 ms sooner).
    The app plays the HLE's sound as it is written, so there it comes about 1/4 s sooner than in Original.
  - Nicktoons input: the menu answers Down 2 frames later (80 ms at its 25 fps) in both, the picture 2.47 s
    after the trigger in the HLE, 2.49 s in Original. Gameplay not reached by these keys. GUESS: the
    "1/4 s lag" is the driver's 256 ms sound buffer, which the real Didj has too (docs/didj.md).
  - WayForward games (SpongeBob, Nicktoons): their story scenes run 20-31 fps, logos up to 47, the Didj menu
    50, in the HLE as in Original; JetPack's play 25-33 (checked before, Original the same) and clock-timed.
    Nothing here runs a WayForward game above about 33 fps, so the "60, should be 30" lead (an LF3000
    compatibility setting) is not this emulator's; whether a real Didj shows 30 or 33 in JetPack's levels
    needs the user's Didj.
  - Indiana Jones: New Game, Medium, then no keys: the first story panel ("In Nazca, Peru...") stays for
    good, flipping at 50 fps, in the HLE (118 s) and in Original (the rest of a 350 s run) alike; A moves
    on (A every 2.5 s reaches the first level, ~35 fps). So no HLE fault; GUESS: the panels wait for a
    button. Whether a real Didj moves on alone needs the user's Didj.
  - In-betweens (tween.rs): the box row under Tune-Up's letters tore apart in the picture after the cursor
    blinked off (102 of 1148 pictures over 2 by twm.py; the old box, another picture with the cursor, took
    the next box's place, a box-width move that many boxes could make): `follow_layers` now leaves a pair
    that is in place unless most pictures take the new move (test
    `a_box_losing_its_cursor_stays_in_a_still_row`). And an operation taken back with its neighbour no
    longer is when that neighbour moved beyond MAX_MOVE (it isn't blended either). After: 0 of 305
    pictures over 2; the timer-end picture (00:00, avatar gone) has no white bars. TWEEN_DUMP removed.
    Left: one picture of the leaf wipe into "LET'S RACE" shows leaf and box shards (pieces moving over 48 px
    a frame pair with look-alikes). The race itself never flips the MLC layer address (it draws into the
    shown buffer, GUESS from the unchanged MLC registers), so it gets no in-between pictures, only
    its own frames; it looks right. After the fix, 0 of 431 pictures over 2 in a run from Tune-Up into the race.
  - Boot-logo "squeal" (player report, Enhanced (new) in the app at ~80% speed): the logo's sound,
    captured to WAV, is the same in Original, Original+ and Enhanced (new), on f4c9111 and with the
    /dev/dsp change, also with didjhle DIDJ_HLE_MIPS=250/150 (a slow host): no tone, click or rate error
    from Ribbit. The one narrow high tone is in all of them: 14336 and 14430 Hz, about 49 dB under the
    jingle's 1664 and 1570 Hz notes, their mirror around 16 kHz (high band vs mirrored low band
    correlation 0.76): Brio's mixer makes it raising 16 kHz content to 32 kHz, so a real Didj puts it out
    too (GUESS: its speaker hides it). The app's chain (32 kHz queue, linear to the device rate, the
    CrispStereo exciter the player has on) adds no tone in a simulation (+3 dB above 4 kHz).

## Known risks

- Timing: games paced by the 3D chip or by audio DMA lose their pacing; the HLE must pace frames at 60 Hz and
  audio by the host.
- Anything the firmware does in kernel space is lost: fsck/tune-up, UBI, USB gadget, power-off sequencing.
  Those become Rust (or are skipped) in HLE.
- Two backends to keep correct; Original stays the reference for every check.
