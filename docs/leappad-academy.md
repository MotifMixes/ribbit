# LeapPad Academy and LeapFrog Epic

Research notes and plan for the Android-based LeapFrog tablets. Written 2026-11-15 from the
stock firmware and public sources; the titles' own code runs since 2026-11-16 (see "First code"). The first code is in
`crates/ribbit-lfandroid`.

## Summary

- Both tablets run Android. Their LeapFrog games are **the Brio SDK games of the
  LeapPad 2/3, Ultra and Explorer line, rebuilt for Android**. Each title is a small Java shell
  (`LFMainActivity`) plus one ARMv7 native library. That library statically links the game, its
  engine and Brio itself: `LeapFrog::Brio::CDisplayMPI`, `CAudioMPI`, `CButtonMPI`, `CKernelMPI`,
  `CEventMPI`, `CFontMPI` and the rest, with `_android` back ends and `LF::Android::LFAppManager`.
- The Epic (`com.leapfrog.ANDS.app.*`, Brio SDK 8.x) and the LeapPad Academy
  (`com.leapfrog.AND4.app.*`, SDK 8.9 to 9.0) use the same format, the same 40 JNI entry points
  and nearly the same import list. **One HLE can run both.**
- The **LeapFrog Academy program** (the subscription learning path) is a different thing. It is a
  PlayCanvas/HTML5 web app inside a Crosswalk WebView. It needs a LeapFrog account and
  LeapFrog's servers, and only the menu UI ships in the firmware. It is out of scope.
- Recommendation: path **(b)**, an "Enhanced (new)"-style HLE of the Brio-on-Android runtime that
  runs the titles' own ARM code. The main costs are an ARMv7 + VFPv3 CPU and a GLES 1.1
  renderer. See "Paths" below.

## Hardware

| | LeapFrog Epic / Epic Academy Edition / LeapPad Academy, 1st gen | LeapPad Academy, 2nd gen (2021 revision) |
|---|---|---|
| SoC | MediaTek MT8127: 4x Cortex-A7 at 1.3 GHz, Mali-450 MP4 | Rockchip RK3326: 4x Cortex-A35 at 1.5 GHz, Mali-G31 |
| OS | Android 4.4.2 KitKat (`KOT49H`) | Android 10 Go (`rk3326_qgo`, `ro.config.low_ram=true`) |
| Firmware ID | `EPICv3`, device `UYT3`, board `quanta27_tb_uyt_kk` (ODM Quanta, "narnia") | `EPICv4`, device `UYT4`, brand `LeapFrog4`, display id `EPIC4.user.1.7.2109.312280833` |
| Common | 1 GB RAM, 16 GB flash + microSD, 7" 1024x600, 2 MP cameras front and back, Wi-Fi b/g/n, BT 4.0 | same |

- The Epic came out in 2016, the Epic Academy Edition in 2017 and the LeapPad Academy in 2019.
  In 2021 LeapFrog quietly changed the LeapPad Academy from the MT8127 to the RK3326 and left
  the rest the same ([Wikipedia: LeapFrog Epic](https://en.wikipedia.org/wiki/LeapFrog_Epic),
  citing FCC ID G2R-6022).
- The RK3326 unit has a sub-variant called "MIPI" (LCD interface). Its firmware is separate and
  not interchangeable with the other one
  ([archive.org: leapfrog-epicv4-firmware_MIPI](https://archive.org/details/leapfrog-epicv4-firmware_MIPI)).
- Bootloader: LeapFrog offers no official unlock. On the MT8127 Epic, the community unlocked it
  by exploiting the firmware being signed with MediaTek reference keys (SP Flash Tool, TWRP,
  LineageOS 14.1) ([XDA article](https://www.xda-developers.com/developers-port-lineageos-leapfrog-epic-tablet-for-children),
  [The Register](https://www.theregister.com/2020/09/15/lineageos_port_for_leapfrog_tablet/),
  [TWRP tree](https://github.com/huckleberrypie/device_quanta_narnia)). On the RK3326, the dump
  comes with a patched U-Boot that enables rockusb, and a 2nd-gen unit has been shown running
  LineageOS 18.1.

## Obtainable material

All of this is on archive.org. Nothing has to come from a device.

| Item | What | Local copy |
|---|---|---|
| [leapfrog-epicv4-firmware](https://archive.org/details/leapfrog-epicv4-firmware) | Full RK3326 LeapPad Academy eMMC dump (rkDumper). Identical to `EPIC4.user.1.7.2109.312280833.7z` | `D:/ribbit-roms/LeapFrogEpic4/` |
| [leapfrog-epicv4-firmware_MIPI](https://archive.org/details/leapfrog-epicv4-firmware_MIPI) | Same, MIPI sub-variant (1.3 GB) | not downloaded |
| [epic4-rk3326-source](https://archive.org/details/epic4-rk3326-source) | GPL source (kernel, U-Boot) for the RK3326 build | not downloaded |
| [leapfrog-android-apps](https://archive.org/details/leapfrog-android-apps) | 725 files, 231 GB: 117 Epic game APKs (`ANDS.app`), 30 Academy game APKs (`AND4.app`), about 117 music and about 436 video APKs, plus `SystemImages/1stGen_System.img.xz` and `2ndGen_System.img.xz` and the system apps | `D:/ribbit-roms/LeapFrogEpic/` (1st-gen system and system apps, 3 game APKs) |
| [android_kernel_quanta_narnia](https://github.com/huckleberrypie/android_kernel_quanta_narnia) | GPL kernel for the MT8127 Epic | - |

The archive item's description says the apps use a modified Android LVL licence check, and
videos use Widevine DRM in an obfuscated (DexProtector) player. The licence check lives in the
Java side, which an HLE does not run. Video APKs are out of scope.

### RK3326 dump (`D:/ribbit-roms/LeapFrogEpic4`)

- `img/`: the unpacked archive. `parameter` holds the partition map (EFI_part, uboot, trust,
  misc, dtb, dtbo, vbmeta, boot, recovery, backup, security, cache, metadata, app, frp, super,
  userdata). `uboot-patched.img` is the community rockusb U-Boot.
- `super/`: the logical partitions taken out of `super.img` (liblp metadata v10.0): system
  (1.5 GB, system-as-root), vendor, product and odm, all ext4. Unpacked with a small Python
  lpunpack.
- `userdata.img` is a factory-fresh sparse image and `backup.img` is all zeros, so nothing
  downloaded is in the dump. `app.img` (8 MB) is about half high-entropy data, and
  `libAppArea.so` reads it; it is probably encrypted per-device data.
- LeapFrog apps are in `/system/priv-app/vtech-*` (`apks/` holds the copies examined):
  - **Brio games and utilities, "NoSO" APKs**. Their native library is in `/system/lib`
    (copied to `gamelibs/`):
    - `AND4.app.x00200026.AP_SM2`: `libStretchyMonkey2.so`
    - `x00370009.PPAD2`: `libPPAD2.so`
    - `x00280010/12/14.NUTL`: `libVoiceMemo_Utility.so`, `libCalculator_Utility.so`, `libClock_Utility.so`
    - `/system/lib` also has `libAP_CCHAT.so`, `libAP_PWR04.so`, `libAP_XCOOK.so` and
      `libAP_XSCOUT.so`, whose APKs are not installed.
  - **Shells:**
    - `vtech-KidLauncher`: Unity (`libunity.so`, `libmono.so`).
    - `vtech-com.leapfrog.lfeshell.webview-939`: the LeapFrog Academy program. A Crosswalk
      WebView over `www.zip`, with a PlayCanvas `KIDUI` front end (`deploy.json`: kidver 1.2.52)
      and a `dj.zip` store page.
  - **Plain Java apps:** EngagementDaily/Weekly (JSON picture data), RewardStore, Camera,
    Gallery, Notepad, MusicPlayer, VideoPlayer, AppCenter, OOBE, DrmLiteServices, ContentProfile.
  - **4 music APKs** (`ands.music.*`): OGG files plus art, with no code that matters.

### MT8127 system image (`D:/ribbit-roms/LeapFrogEpic`)

`1stGen_System.img` (sparse; unsparsed to `sys/system.raw.img`, ext4) is `EPICv3`,
`KOT49H.user.1.1.171.20190530`, Android 4.4.2, MT8127. Its system apps
(`com.lvlstudio.leapfrog.kidlauncher`, contentmanager, drmliteservices, narnia fotaclient,
and so on) are in `apps/`. LVL Studio (`leapfrog-build.lvlstudio.local` appears in the Academy
web bundle too) built both launchers.

## How a title is built

These are the results of `lfa_inspect` (see below). They are the same for the Epic and the
Academy builds:

- APK: `classes.dex` (Java shell, about 2-3 MB) plus `assets/<CODE>/...` (game data) plus
  `assets/LF/Bulk/...` (shared tutorials, language pack). The manifest declares
  `android.app.lib_name` (the native library), `contentType` (`learning-game`, `utility`) and
  `sdkVersion` (Brio SDK: 8.0.0.6 and 8.6.0.6 on the Epic, 8.9.0.23 and 9.0.0.23 on the Academy).
- Native library: ELF32 ARM, `armeabi-v7a`: ARMv7-A, mostly ARM mode (about 12,000 ARM
  functions, about 100 Thumb-2), **VFPv3** (about 25,000-46,000 VFP instructions per library),
  a few hundred NEON-looking words, soft-float ABI. Each library is 6-9 MB, with about
  14,000-18,000 REL relocations (all RELATIVE, GLOB_DAT and JUMP_SLOT) and about 220-320
  constructors.
- Brio is linked in: about 2,500 exported `LeapFrog::Brio` symbols per library. The MPIs are
  Accelerometer, Audio, BootSafeKernel, Button, Camera, Debug, Display, Event, Font, Kernel,
  Microphone, Power, Video, Wireless and ICoreMPI. The games still use Brio paths
  (`/LF/Bulk/`, `/LF/Base/Brio/Module/`, even `/LF/Base/DidjAvatars/`) and Brio's
  CAppInterface/GameApp structure. That is the same API Ribbit's Explorer and Didj HLE already
  serve at the Linux level, except that here the MPIs are compiled into the game.
- Game data formats (AP_SM2): `.texar`, `.atlas`, `.poly-anim`, `.til3`, `.fun`,
  `.fdata`/`.ftex` (fonts), `.phrs`, OGG/WAV audio and PNG. PPAD2 uses `game-*.rdt`. These were
  not yet compared with Explorer cartridges (none were found unpacked on this PC).
- What an HLE must provide is about 400 imports, taking the union over 6 libraries:
  - bionic libc/libm: about 230
  - pthread: about 38
  - OpenGL ES: about 80, mostly **GLES 1.1 fixed-function** (glMatrixMode, glOrthof,
    glVertexPointer, glTexEnvi, OES framebuffer objects); `naIsGLES2Enabled` lets the shell
    choose
  - EGL: 7-9 queries
  - OpenSL ES: `slCreateEngine` + 8 IIDs (buffer-queue PCM, record)
  - OpenMAX AL: 6 (video playback, `.mp4.ts` tutorials)
  - libandroid: 22 (AAsset*, ASensor*, ALooper*, ANativeWindow_fromSurface)
  - zlib: 6
  - libdl: 4
  - liblog: 1
- JNI: `JNI_OnLoad` registers 40 natives on `LFMainActivity` and the adapter classes
  (from `AppManager-android/Src/LFNativeMethods.cpp`):
  - lifecycle: `naInitMainActivity`, `naLFGLSurfaceViewSurfaceCreated/Changed/Destroyed`,
    `naDrawFrame`, `naSuspend/Resume`
  - touch: `naTouchDown/Up/Move(FFI)V`
  - buttons: `naPressA/B/Home/Hint/Pause/Back/Joystick` and the matching releases
  - `naSetVideoSurface`
  - adapter inits for logging, profiles, rewards, camera and wireless

  The native code calls back into Java through a few methods: logging events, camera paths and
  capture, wireless state, and `naActivityDone`. A fake JNIEnv only has to answer those.

## Paths

**(a) Full Android system emulation.** Boot the RK3326 image: a Cortex-A35 SoC (AArch64 kernel
with a 32-bit userland), Mali-G31, eMMC, Rockchip peripherals, Android 10's SELinux and
binder, then the Unity launcher and Crosswalk.
- Effort: many months. Risk: very high (Mali GPU, SoC documentation).
- Ribbit would need a 64-bit ARM core and a GPU.
- Even QEMU has no RK3326 machine. The MT8127/Android 4.4 image would be smaller but has the
  same problems.
- Not recommended.

**(b) HLE of the Brio-on-Android runtime (recommended).** Load the title's `.so` (done:
`ribbit-lfandroid`), run it on Ribbit's ARM core, and give it Rust stand-ins for the about 400
imports and the Java shell. The work, with estimates:
1. **CPU: about 2-4 weeks.** `ribbit-arm`/`ribbit-armjit` are ARMv5TE (ARM926). Missing:
   - ARMv7 ARM-mode additions: movw/movt, ldrex/strex, dmb, ubfx/sbfx/bfi, rbit, and so on
     (small)
   - **VFPv3**: single and double precision, used everywhere (large, but regular)
   - Thumb-2 for about 100 functions (medium)
   - a NEON subset (check what is really used first)
   - The JIT can follow later; at first, interpreted speed on the desktop should be fine for 2D
     games.
2. **Loader and user-mode runtime: about 1-2 weeks.** Thunk traps at `THUNK_BASE`, TLS,
   threads (Brio starts a few pthreads; the Didj HLE's task model can be reused), heap,
   constructors, C++ exceptions (`__cxa_*`/unwind come from the library's own static libstdc++,
   only 2 C++ imports), and a file system over the APK assets with `/sdcard/LF/Bulk` mapped to
   a folder.
3. **Graphics: about 2-3 weeks.** A GLES 1.1 fixed-function implementation (the list above)
   into Ribbit's renderer, or a CPU rasterizer at 1024x600 first. EGL is just queries and swap.
4. **Sound and video: about 1 week.** OpenSL ES buffer queues to Ribbit audio. OpenMAX AL
   tutorial videos (MPEG-TS, H.264) can at first be skipped, or decoded through the existing
   FFmpeg route if there is one.
5. **Shell and front end: about 1 week.** A Rust LFMainActivity that calls the `na*` natives,
   maps touch and buttons, and gives Ribbit library cards from the APK icon and title. Saves
   go to a per-title folder (Brio profile and settings files).

In total it is about 2-3 months of focused work before the first title is playable. After
that, most titles should follow, because they share one SDK and one import list. Risks: VFP
correctness, threads (Brio uses pthread conditions and timed waits), GLES state details, and
titles that use the camera or microphone. Practical notes: only the user's own downloads or
dumps are used, and Ribbit never distributes APKs. The licence check is in Java, which this
path never runs. The Academy program, KidLauncher, Engagement apps, music and videos are not
covered.

**(c) A standard Android emulator, with Ribbit as launcher.** The titles want LeapFrog system
services (ContentProfile, DrmLiteServices, LoggingServices, `com.leapfrog.permission.*`), the
modified LVL check, and a `/sdcard/LF/Bulk` layout. The NoSO titles also need their library in
`/system/lib`. On an x86 emulator they need ARM translation (libndk/houdini). The Android
emulator's ARM images are slow, and 32-bit ARM support is being dropped.
- Effort: about 1-2 weeks of glue for a fragile result.
- It needs a large external dependency, so it isn't "the same way as the other ones".
- Possible only as an experiment.

**(d) Not feasible: the LeapFrog Academy program itself.** It is subscription content served
from LeapFrog's servers. Only the UI shell is in the firmware, and offline play covers just up
to 24 downloaded favourites. Nothing of it can be preserved or run without the service. The
Unity KidLauncher could be replaced by Ribbit's own library screen.

## First code: `crates/ribbit-lfandroid`

- `apk.rs`: a read-only ZIP reader (stored and deflate, through `miniz_oxide`) and the
  binary-manifest string pool.
- `elf.rs`: ELF32 ARM shared-object loader. It maps PT_LOAD at a base, reads PT_DYNAMIC, applies
  RELATIVE, ABS32, GLOB_DAT and JUMP_SLOT relocations, binds each import to a caller-chosen
  address (one 8-byte thunk slot each), and collects exports, DT_INIT/INIT_ARRAY and the aeabi
  CPU attributes.
- `lib.rs`: `open_title()` reads the manifest metadata and finds the library in the APK or in
  the firmware's `/system/lib` (for NoSO titles). It returns the loaded image, imports grouped
  by Android library (`api_area`), and the JNI natives.
- `examples/lfa_inspect.rs`:
  `cargo run -p ribbit-lfandroid --example lfa_inspect -- [--lib <system/lib>] [-v] <apk>...`

Checked on 3 Academy built-ins and 3 store APKs (Epic AP_SM2 and AP_DINO, Academy AP_DINO).
All loaded with 0 unbound relocations. The import union is 401-404.

- `runtime.rs`: a user-mode runner. Ribbit's ARM core with ARMv7 on runs over a flat 32-bit
  memory (allocated by page). An import's thunk slot, when reached, is answered in Rust by name;
  calls into the game can nest (pthread_once, qsort). Layout: library at 0x4000_0000, heap
  0x1000_0000-0x3000_0000, stack below 0xE000_0000, thunks 0xF000_0000, data imports
  0xF400_0000 (1 KB each: `_ctype_`, `__sF`, `SL_IID_*`...), JNI tables 0xF800_0000/0xF900_0000,
  host functions 0xFA00_0000.
- `imports.rs`: about 270 of the libc/libm/liblog/libdl/libz imports, a printf formatter,
  setjmp/longjmp, gmtime, strftime, zlib's `uncompress` and `crc32`. Plain files, folders and
  file descriptors don't open yet (ENOENT): everything the titles read so far is an asset.
- `threads.rs`: pthreads on the one CPU, switched when a thread waits and every 200,000
  instructions. Each keeps its registers, VFP state and pthread keys. Mutexes (all treated as
  recursive), condition variables (timed ones too), join and sleeps really wait; a waiting
  call stays at its import slot and runs again when woken. Threads switch only at the outer
  call level (no call from Rust into the game in progress). When every thread waits, the
  clocks skip to the next deadline instead of sleeping.
- `assets.rs`: libandroid's AAssetManager over the APK's `assets/`. The titles' own file layer
  tries the path as a file, then as an asset (`/sdcard/AP_DINO/x` -> `AP_DINO/x`).
  `AAsset_openFileDescriptor` hands out a descriptor (100 and up) holding the asset's bytes,
  for OpenSL ES players made from a file.
- `stdio.rs`: FILEs made by `funopen` (the file layer wraps each asset in one with its own
  read/seek/close functions, which are called back), with fread, fseek, fgets, ungetc...
- `gles.rs`: GLES 1.1 and EGL through ribbit-gles, in software. A 1024x600 window surface and
  context are current before the first GL call, as GLSurfaceView makes them.
- `android.rs`: sensors (an accelerometer that sends no events), the looper, native windows.
- `jni.rs`: a JavaVM and JNIEnv (all 233 slots of the JNI 1.6 table). Objects are handles,
  method calls are logged and answer 0 or an empty object (`File.getAbsolutePath` answers
  `/sdcard`, `LFProfileManager.getSystemLocale` "en-us" (GUESS)), and `RegisterNatives`
  records the natives.
- `sles.rs`: OpenSL ES and OpenMAX AL objects. Every interface is a table of host functions;
  buffer-queue callbacks and enqueued buffers are kept for sound later.
- `examples/lfa_run.rs`:
  `cargo run --release -p ribbit-lfandroid --example lfa_run -- [--lib <system/lib>] <apk>`
  runs the constructors, `JNI_OnLoad`, `naInitMainActivity(activity, "/data/data/<package>")`,
  the other `naInit*Adapter`s, `naLFGLSurfaceViewSurfaceCreated/Changed`, `naResume`, then
  LFA_FRAMES (60) `naDrawFrame`s, and writes the last frame to LFA_PPM (`lfa_frame.ppm`).
  LFA_LOG=1 prints the game's log and missing files and assets; LFA_CALLS=1 the imports and
  Java methods called; LFA_TRACE=1 keeps the last jumps and imports for a stop and logs thread
  switches; LFA_STOP=<text> stops at the first missing asset with that text, with a backtrace.
  LFA_INPUT=<frame>:<what>;... changes the input before those frames: `x,y` puts the finger
  there (window pixels; down, or a move if it is down), `lift` lifts it, button names (a b back
  hint pause home up down left right, joined with `+`) hold those, `none` lets go of them.
  LFA_SHOTS=<frame>,... also writes the screen before those frames (<LFA_PPM>-<frame>.ppm).
- `input.rs`: touch and buttons (2026-11-16), sent as the Java shell does, through the
  natives, between frames. `Runtime::touch(Some((x, y)) | None)` sends `naTouchDown`,
  `naTouchMove` or `naTouchUp(FF)V` as the finger changes; `Runtime::set_buttons(mask)` sends
  `naPress*`/`naRelease*` for each button that changed. The touch floats come in r2/r3
  (softfp). They are window pixels in the window's orientation (`Runtime::screen`); the
  library turns them into Brio screen pixels itself
  (`LFAppManager::transformHostScreenPositionToEmulatedDeviceScreenPosition`: T-Rex Rush
  1024x600 -> 480x272, Big City Hijinks 580 of 600 wide x 1024 -> 544x960) and posts a Brio
  `CTouchMessage` to its event queue. The buttons are Brio's `tButtonData` bits, read from
  the natives: A 0x10, B 0x20, Back 0x100 (Brio's menu button), Hint 0x200, Pause 0x400; each
  native sets or clears its bit (`SetButtonState`) and posts a `CButtonMessage`.
  `naPressJoystick(I)V`/`naReleaseJoystick(I)V` take the direction bits themselves: up 1,
  down 2, right 4, left 8 (GUESS: Brio's usual order; `RotateDpad` turns 1->8->2->4->1, which
  fits either way round). `naPressHome` sets no Brio bit and only logs here; `button::HOME`
  is Ribbit's own bit for it. `naPressPause` returns a boolean, not used.
- Players end (2026-11-16, in `sles.rs`): T-Rex Rush waits for a voice clip's
  `SL_PLAYEVENT_HEADATEND` before it leaves the menu. An audio player made from an asset
  descriptor gets its clip's length (WAV: data over byte rate; Ogg Vorbis: the last granule
  over the sample rate; unknown: 1 s, GUESS). When it plays, `Runtime::end_players` (called
  between frames) ends it after that long: the play state goes to paused and the play
  callback gets HEADATEND if its mask asks for it. No sound is made yet; a paused clip starts
  again from the beginning (GUESS: no position kept).

CPU work this needed in `ribbit-arm` (on only with `Arm::v7`): Thumb-2 (`thumb2.rs`: the 32-bit
encodings, IT blocks, CBZ/CBNZ, the v6/v7 16-bit additions; the libraries' static libstdc++ is
Thumb-2), and NEON VADD/VSUB integer. GCC uses NEON for 64-bit integer arithmetic; ARM code
has only those three NEON words.

ribbit-gles got GL_OES_framebuffer_object for this (the titles draw the Brio screen into a
texture through a framebuffer, then scale it to the window) and glGetTexEnv.

Status (2026-11-16): all six titles run their constructors (216-318) and `JNI_OnLoad`
(JNI 1.6, 47-48 natives). After 600 `naDrawFrame`s (about 5 s each title, headless):
- Epic T-Rex Rush (AP_DINO): its main menu (PLAY, BADGES), drawn right.
- Academy NUTL (built-in clock, with `--lib`): its clock screen, drawn right.
- Epic AP_SM2 and Academy AP_SM2_NoSO (Big City Hijinks): the whole title picture, in
  portrait (below).
- Academy NUTL 0x00280010 (voice recorder, portrait): its screen, drawn right.
- Academy T-Rex Rush (AND4 build): black after 600 frames.
- Academy PPAD2 (with `--lib`): jumps to address 0 in the first frame.

Portrait titles (2026-11-16): Big City Hijinks and the NUTL titles declare
`android:screenOrientation="portrait"` (1) on `.LFMainActivity`; T-Rex Rush and PPAD2 are
landscape (0). The tablet turns a portrait activity, so its window is 600x1024
(`Title::portrait`, `Runtime::screen`; EGL, `ANativeWindow_getWidth/Height`). Big City Hijinks
was squeezed because `LFAppManager::configureViewFrame` reads `PHRS/GameInfo.json.PHRS` ->
`PHRS/buttonMap.json`: `ViewFrameConfigs.Default` = `ow 544, oh 960, tw 580, th 1024` (its
Brio screen is 544x960, drawn 580x1024), and `initializeDisplay` takes the host size from
`eglQuerySurface`. With a 1024x600 window that came out 580 wide and 600 tall. The 580-wide
picture now sits at the left of the 600-wide window; whether the Java side centres the view
(`naGetHostDeviceWidth`) is a GUESS still open. `LFA_BREAK=<offset,...>` (hex, library
offsets) prints the registers when an address runs.

Input check (2026-11-16): Epic T-Rex Rush,
`LFA_FRAMES=1100 LFA_INPUT="600:520,225;606:lift" LFA_SHOTS=600,700,800,900`. The tap on PLAY
plays the menu click and the PLAY voice; when the voice ends, the screen fades to black (700),
the intro story starts (the dinosaurs at the campfire, 800-900) and goes on to the Dinosaur
Valley map (1100). Without the tap, or with only buttons (a, right+b, back), the menu stays.
Big City Hijinks (portrait) takes touches too: window (300, 512) is its screen (281, 480).

Bugs found on the way: the stand-in `sprintf` took itself for `snprintf` (its name has an
"n"), so the format came from the wrong argument; the scene loader read nothing because
`funopen` wasn't there.

Next steps, in order:
1. Epic AP_SM2 reads through pointers made of spaces (unmapped 0x2020xxxx): find where.
2. Academy T-Rex Rush's black screen, PPAD2's jump to 0.
3. Touch and buttons are done (`input.rs`); next the GUI: a console in the app. Open: the
   direction bits (check in a game that uses the d-pad), what `naPressHome` should do, and
   whether the Java side uses `naGetHostDeviceWidth/Height` to place a portrait view.
4. Sound from the OpenSL ES players (ogg/wav from the asset descriptors) and buffer queues.
5. Saves: `/Local/0/<product id>/MySave7.sav` and `/sdcard/LF/Bulk/settings.cfg` on a host
   folder (plain files and descriptors).

## Questions for the user

- Do you own a LeapPad Academy or an Epic? Which one (MT8127 or RK3326; a MIPI unit)? It would
  help to check behaviour on hardware and to dump a device that has store titles installed.
- Which titles matter most? Each of the 147 game APKs on archive.org is 70-210 MB, so they should
  be downloaded only as needed.
