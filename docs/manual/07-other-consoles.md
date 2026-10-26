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

## LeapPad (early emulation)

The original LeapPad (1999) is a book-and-pen pad: the child touches the printed page with the pen and the cartridge talks. Ribbit needs the pad's **base ROM** from your own LeapPad: open it with **File › Open BIOS…** (Ribbit keeps it as the LeapPad's; your Leapster BIOS stays). The 2 MB "Full Base" ROM works best.

- Your book cartridges are on the library's **Books** side. Click one to open it; **Emulation › LeapPad (1999) › Turn on (built-in book)** starts the pad with no cartridge.
- Cartridges hold no page pictures, so Ribbit draws each page from the cartridge itself: the spots it answers, its GO circle, its stop sign and its sentence buttons. Click anywhere on the page with the mouse (the pen).
- **◀ ▶** in the bar turn the page. A real pad needs a touch on the green **GO** after every page turn; with **Touch GO automatically** (on) Ribbit's pen does it for you.
- **ON/OFF** is the pad's switch.

### The books' words

Cartridges hold sound, not text. With **Show the books' words** (on), Ribbit fills in each page's printed words by listening to its spots in the background, the page you are on first (about 10 seconds a page on a fast PC). It needs a speech model (OpenAI's Whisper, about 150 MB): click **Get it** in the bar once. The words are kept, so each book is only done once. Nothing leaves your PC except the model's download. Not on Android yet.

The LeapPad is early emulation: see the [compatibility list](https://github.com/MotifMixes/ribbit/blob/main/docs/compatibility.md) for the books that have been tried.
