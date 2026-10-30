# Other consoles

Choose the console in the library's console menu (see [The game library](#the-library/the-console-and-the-bios)). Every console needs its own system files from your own console.

## Leapster L-MAX, Leapster TV and Leapster 2

These play the same Leapster cartridges with their own BIOS. Put the BIOS in your games folder or open it with **File › Open BIOS…**; the console then appears in the library's console menu.

- The **Leapster TV** shows a 4:3 TV picture; its controller and stylus work as on the handhelds.
- The **Leapster 2** starts on its own main menu, as the real one does; **Emulation › Leapster 2: start games directly** goes straight into the cartridge. **Leapster 2 Main Menu** in the library turns it on without a game.

### The Leapster 2's SD card

The Leapster 2 (BIOS 0.6 and Connected B5) has an SD card; the CIP3 board has none. **Emulation › 💾 Leapster 2 SD card** changes it, with nothing to install:

- **SD card folder…** makes a folder on this PC the card, as the RockIt Twist's content folder: the card is made from the folder each time the Leapster 2 starts, and what the console writes to the card (profiles, saves) goes back into it, deleted files included. Lay the folder out like the card's top level; for a LeapLoader card, choose the folder LeapLoader built. An empty folder first gets the card's files. **Open the folder** shows it, and **Stop using the folder** goes back to the card file in the save folder. A `Leapster` folder is added if there is none: the Connected B5 BIOS formats a card without one.
- **Load a card image…** takes a real card's image (`.img` from Win32 Disk Imager or `dd`) or a Ribbit card file (`sdcard-….img`). Without an SD card folder it replaces the card, and the old card is kept as `.img.bak`; with one, the image's files are copied into the folder.
- **Copy the card's files to a folder…** and **Empty the card** (a freshly formatted card; the old one is kept as `.img.bak`) are there while no SD card folder is used.

A running Leapster 2 restarts with the new card. Without a game running, these change the card of the Leapster 2 BIOS picked in the library. The emulated card holds 2 GB (a card made by an earlier Ribbit keeps its 1 GB volume until it is emptied); an image of a larger card has its files copied onto it. Each BIOS keeps its own card file (`sdcard-<BIOS CRC>.img`, in the save folder).

These consoles are experimental: some games may not work yet. The [compatibility list](https://github.com/MotifMixes/ribbit/blob/main/docs/compatibility.md) says what has been tested.

## Zippity

The Zippity is LeapFrog's TV console with a dance mat and a bopper. It needs a dump of its system memory (its NAND) in your games folder; its built-in games and its cartridges then play.

| Zippity | Key |
|---|---|
| Bopper | Arrow keys |
| Green spot (choose) | `Z` |
| Red spot | `X` |
| Blue spot | `H` |
| Pause / Home | `Backspace` |
| Stand on the home spot | `Enter` (or **Input › Zippity: stand on the home spot**, `Shift`) |
| Left / right foot | `-` / `=` |

## RockIt Twist (early emulation)

The RockIt Twist needs its system packages and its games, decrypted and unpacked from your own packs:

1. **Emulation › RockIt Twist › Import games (.zip)…** installs a zip of package folders (each with its `meta.inf`) on the emulated console.
2. **Emulation › RockIt Twist › Turn on (early emulation)** turns it on: its own menu shows the games.

> Ribbit does not decrypt LeapFrog's `.lf3` packs and includes no tools for it. An `.lf3` file opened as it is will not run.

The menu, the built-in games, the videos and the sound work; the console keeps its profiles and progress in the save folder. **Emulation › RockIt Twist › Content folder…** adds a folder to the console's storage (for your own programs or content), and **Plug into this PC (USB)** in the same menu offers it to LeapFrog Connect through USB/IP.

| RockIt Twist | Keys |
|---|---|
| D-pad | `W` `A` `S` `D` or the arrows |
| Switch left / right | `Q` / `E` |
| Button grid | Number pad `1`–`9` |
| Dials | `U`/`J`, `I`/`K`, `O`/`L` |
| Spinner | Number pad `*` / `-` |
| Slider | `Page Up` / `Page Down`, mouse wheel |
| Check mark | `Enter` |
| Home / Hint | `Home` / `Shift` or `Tab` |

## Plugging a console into this PC (USB)

**Emulation › Leapster 2: plug into this PC (USB)** and **Emulation › RockIt Twist › Plug into this PC (USB)** offer the emulated console to your PC as a USB device through USB/IP, so programs such as LeapLoader (Leapster 2) or LeapFrog Connect (RockIt Twist) can talk to it. Then connect it with a USB/IP client:

- **Windows**: install usbip-win2 (`winget install usbip-win2`), then run `usbip attach -r 127.0.0.1 -b 1-1`.
- **Linux**: the client is part of the kernel. Install the `usbip` tool (Ubuntu/Debian: `sudo apt install linux-tools-generic`; Fedora: `sudo dnf install usbip`), load its module once with `sudo modprobe vhci-hcd`, then run `sudo usbip attach -r 127.0.0.1 -b 1-1`. The Leapster 2 shows up as two drives (Leapster 2-RAM and Leapster 2-SD).
- For the Leapster 2, wait about 7 seconds after plugging in before you attach: a running Leapster 2 restarts once, as a real one does.
- `usbip detach -p 0` (with `sudo` on Linux) unplugs it from the PC's side.

## Didj (early emulation)

The Didj (2008) is LeapFrog's Linux handheld with NAND cartridges. Ribbit builds the console's internal memory from its firmware packages, the `DIDJ-0x….lfp` files LeapFrog Connect downloads (Bootstrap, Device Firmware, Brio and the `Base_` ones). Put them all in one folder and add it with **Add games folder**; the Didj then shows on the library's **Didj** tab, with your cartridge dumps beside it.

- The first start goes through the Didj's own setup (country, name, grade). After that, choosing a cartridge or a built-in game in the library starts it from the Didj's menu for you; press any button to take over.
- What the console saves (players, progress, settings) is kept in the save folder. Save states work too.
- Cartridge dumps can be the raw chip (69,206,016 bytes, with spare areas) or an image of the cartridge's FAT volume alone (about 62 MB, often with no file extension); both show on the Didj tab.
- Some cartridges install an update to the Didj the first time they are played, and the Didj then turns itself off. Ribbit turns it back on and starts the game again.
- Closing the game presses the Didj's power button and waits for it to turn off, so its files are never left damaged.
- **Emulation › Didj › Enhanced** (the default) fixes faults in the Didj's own software: after a crash it repairs its files and goes back to the menu instead of locking on "Your Didj needs a tune-up"; it gets 50 MB of memory instead of 16; and its processor runs faster, up to 2× (or 3×, in the same menu), as far as your PC keeps up: never slower than a real Didj. **Original** runs it as LeapFrog shipped it. Both apply from the next power-on.
- **Emulation › Didj › Skip frames when the PC is too slow** (off by default): when your PC can't keep up, up to 2 of every 3 frames aren't drawn. The game still runs at its real speed (sound, timing and play are the same); only the picture moves less smoothly. Off, every frame is drawn and the game slows down instead. It applies at once.
- It runs best on a Windows or Linux PC (or an Intel Mac): there Ribbit translates the Didj's processor code to the PC's own. The Didj's 3D is drawn on up to 4 of the PC's cores. Apple-silicon Macs and Android phones run it slower for now.

| Didj | Key |
|---|---|
| D-pad, A, B | as on the Leapster |
| L / R (shoulder buttons) | the Leapster's volume buttons (LB / RB on a gamepad) |
| Menu | Home |
| Hint, Pause | Hint, Pause |

## LeapPad (early emulation)

The original LeapPad (1999) is a book-and-pen pad: the child touches the printed page with the pen and the cartridge talks. Ribbit needs the pad's **base ROM** from your own LeapPad: open it with **File › Open BIOS…** (Ribbit keeps it as the LeapPad's; your Leapster BIOS stays). The 2 MB "Full Base" ROM works best.

- Your book cartridges are on the library's **Books** side. Click one to open it; **Emulation › LeapPad (1999) › Turn on (built-in book)** starts the pad with no cartridge.
- Cartridges hold no page pictures, so Ribbit draws each page from the cartridge itself: the spots it answers, its GO circle, its stop sign and its sentence buttons. Click anywhere on the page with the mouse (the pen).
- **◀ ▶** in the bar turn the page. A real pad needs a touch on the green **GO** after every page turn; with **Touch GO automatically** (on) Ribbit's pen does it for you.
- **ON/OFF** is the pad's switch.

### The books' words

Cartridges hold sound, not text. With **Show the books' words** (on), Ribbit fills in each page's printed words by listening to its spots in the background, the page you are on first (about 10 seconds a page on a fast PC). It needs a speech model (OpenAI's Whisper, about 150 MB): click **Get it** in the bar once. The words are kept, so each book is only done once. Nothing leaves your PC except the model's download. Not on Android yet.

The LeapPad is early emulation: see the [compatibility list](https://github.com/MotifMixes/ribbit/blob/main/docs/compatibility.md) for the books that have been tried.
