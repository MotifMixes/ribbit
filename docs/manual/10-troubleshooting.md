# When something goes wrong

## Ribbit closes or shows an error

Ribbit writes a log, `ribbit.log`, in its data folder (`%APPDATA%\Ribbit` on Windows, `~/.config/ribbit` on macOS and Linux). It is written again each time Ribbit starts, so take it right after the problem. When Ribbit stops because of an error, it says why in a message and in `last-error.txt` next to the log.

Please send the log with a bug report.

## The window stays white or black, or the picture is wrong

Ribbit draws with your graphics card. When the card can't draw Ribbit's window, Ribbit on Windows draws in software instead.

If Ribbit closes by itself while it starts (the window black), start it again: on Windows it remembers that start and tries the graphics card another way (DirectX 12, then OpenGL, then software), and keeps the way that works. Delete `gpu-setup-crashed.txt` in the data folder to make it try every way again; an update does too.

If the picture is wrong, or Ribbit still crashes in the graphics driver, make it draw in software anyway:

1. Close Ribbit (from Task Manager if it doesn't close).
2. In the Ribbit folder, click File Explorer's address bar, type `cmd` and press `Enter`.
3. Type `set RIBBIT_SOFTWARE_GPU=1` and press `Enter`, then `Ribbit.exe` and `Enter`.

This lasts only for Ribbit started from that window: started any other way (a double-click, a shortcut), Ribbit
uses the graphics card again and may crash again. If Ribbit crashes in the graphics driver, `ribbit.log` ends with
"setting up the window with GPU" or a line "Ribbit crashed: exception ... in" naming the driver's file.

Updating your graphics driver often helps too.

Drawing in software is slow in a large window: the game runs at full speed, but the picture moves in jerks (fewer
than 20 frames a second when maximised; much better in a small window). So use it only while you need it. If you
set `RIBBIT_SOFTWARE_GPU` for good (in the System settings' environment variables or in a shortcut), remove it once
your graphics driver is fixed. `ribbit.log` says "drawing in software" when Ribbit does.

## The game turns itself off

- The **Universal v1.5** BIOS turns off after start-up unless **Emulation › Advanced (experimental) › MAME power-line workaround** is on.
- With **Emulation › Turn off when idle** on, the console turns off after a while with nothing pressed, as the real one does.

## The sound crackles or the game is slow

The status bar at the bottom shows the speed the game runs at. Below 100%, your computer is not keeping up: turn off **Video › AI upscale** and **HD vectors**, which use the graphics card most, and close other programs.

## A game doesn't work

The [compatibility list](https://github.com/MotifMixes/ribbit/blob/main/docs/compatibility.md) says which games have been tested on which console. If yours isn't there, or doesn't work as listed, please report it with the game's name, the console and BIOS chosen in the library, and the log.

## Reporting a bug

Report problems on [GitHub](https://github.com/MotifMixes/ribbit/issues) with:

- what you did and what happened,
- your system (Windows, macOS, Linux or Android, and its version),
- `ribbit.log`, and a screenshot or a short video if you can.
