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
5. **Ribbit's own OpenGL ES 1.1.** Catch calls into libopengles_lite's exported functions (the menu and games use
   fixed-point GLES 1.x only) and draw them with Ribbit's renderer at 1x-4x, instead of the 3D chip model.
   Check: every game's 3D at 1x matches the chip model pixel for pixel on reference frames; 2x-4x look right.
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
  stored, fatal signals end processes), poll/select, AF_UNIX sockets, mq_notify, O_CLOEXEC on exec.
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
  Open: timing of a game against Original.

## Known risks

- Timing: games paced by the 3D chip or by audio DMA lose their pacing; the HLE must pace frames at 60 Hz and
  audio by the host.
- Anything the firmware does in kernel space is lost: fsck/tune-up, UBI, USB gadget, power-off sequencing.
  Those become Rust (or are skipped) in HLE.
- Two backends to keep correct; Original stays the reference for every check.
