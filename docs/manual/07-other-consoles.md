# Other consoles

Choose the console in the library's console menu (see [The game library](#the-library/the-console-and-the-bios)). Every console needs its own system files from your own console.

## Leapster L-MAX, Leapster TV and Leapster 2

These play the same Leapster cartridges with their own BIOS. Put the BIOS in your games folder or open it with **File › Open BIOS…**; the console then appears in the library's console menu.

- The **Leapster TV** shows a 4:3 TV picture; its controller and stylus work as on the handhelds.
- The **Leapster 2** starts on its own main menu, as the real one does; **Emulation › Leapster 2: start games directly** goes straight into the cartridge. **Leapster 2 Main Menu** in the library turns it on without a game.

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

1. **File › Import RockIt Twist games (.zip)…** installs a zip of package folders (each with its `meta.inf`) on the emulated console.
2. **File › RockIt Twist (early emulation)** turns it on: its own menu shows the games.

> Ribbit does not decrypt LeapFrog's `.lf3` packs and includes no tools for it. An `.lf3` file opened as it is will not run.

The menu, the built-in games, the videos and the sound work; the console keeps its profiles and progress in the save folder. **File › RockIt Twist content folder…** adds a folder to the console's storage (for your own programs or content), and **Plug the RockIt Twist into this PC (USB)** offers it to LeapFrog Connect through USB/IP.

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
