# Game compatibility

How far each game gets in Ribbit. Update this file whenever a game is tested.

## Status levels

| Status | Meaning |
|---|---|
| **Perfect** | Plays start to finish with no known problems (graphics, sound, touch, saves) |
| **Playable** | Can be played through; minor problems listed in the notes |
| **In-game** | Reaches gameplay, but problems stop it being played through |
| **Menus** | The game's own screens run (logo, region or profile menus); gameplay not yet reached |
| **Boots** | The BIOS starts the game, which stops on a loading or blank screen |
| **Nothing** | The BIOS does not accept the cartridge |
| **Untested** | Not tried yet |

"Menus" from the automated check means the game was running its own code but was waiting for
input (the check presses nothing). Most such games may well go further when played.

## RockIt Twist

Early emulation. Game packs (.lf3) are encrypted and are not run. From the system packages (Bootloader, Base,
Firmware-Bulk), the console's own firmware boots to its "Device Error" screen, which a unit shows when LeapFrog
Connect has not installed the menu. With unpacked content imported (docs/rockit-twist.md), the menu runs and the
built-in Arcade game **Nom-Nom** is **Playable** (title screen, level menu, the game, with sound), and the Sneak
Peeks station plays its videos. Albert's Escape and a portrait-mode Arcade game start (not played through); the
packs are untested (2026-10-17).

## Results

BIOS: US v2.1 (152-11265). Automated check: 70 s from power-on, no input, with saved
calibration (Ribbit 1.0.0, 2026-10-15). Played: tested by hand. Every dump below matches MAME's
software list. Region pickers move on by themselves after about 30 s.

| Game | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|
| SpongeBob SquarePants: Saves the Day (USA, alt) | 0f2ad725 | **Playable** | played 2026-10-14 | All four minigames reach gameplay at full speed. Touch is the experimental model |
| Learning with Leap (USA) | 2c1fcf40 | Menus | automated | Title screen |
| Kindergarten | db262164 | Menus | automated | Title screen |
| Dora the Explorer: Wildlife Rescue | 74af5d57 | Menus | automated | Title screen |
| 1st Grade | db36283f | Menus | automated | On-screen letter keyboard (name entry) |
| Creature Create | 9f08d02a | Menus | automated | Region choice (U.S.A. / Canada / U.K.) |
| Disney Fairies (ES) | d2f560c7 | Menus | automated | Region choice (America / Europa) |
| Disney/Pixar Cars 2 | 29af42ba | Menus | automated | Region choice |
| Disney/Pixar Ratatouille | d0c4de47 | Menus | automated | Region choice |
| Dora the Explorer: Camping Adventure | 71778536 | Menus | automated | Region choice |
| Dora the Explorer: Piñata Party | ea544030 | Menus | automated | Region choice |
| DreamWorks Madagascar | a9c75d90 | Menus | automated | Region choice |
| Foster's Home for Imaginary Friends | e62d1684 | Menus | automated | Region choice |
| SpongeBob SquarePants: Through the Wormhole | 6b24a3ef | Menus | automated | Region choice |
| Star Wars: Jedi Math | 27000674 | Menus | automated | Region choice |
| The Batman: Multiply, Divide and Conquer (DE) | a0a362aa | Menus | automated | Kids' WB / DC logos |
| Cosmic Math | 979346d5 | Menus | automated | LeapFrog logo (game's own splash) |
| Disney/Pixar Cars | d4a7e5d4 | Menus | automated 2026-10-16 | Region choice, Pixar logo, title. (Was stuck on a white screen until the ADC delivered battery samples, 2026-10-16) |
| Disney Princess: Enchanted Learning | 5ccb2ad3 | Menus | automated | LeapFrog logo |
| Disney Princess: Worlds of Enchantment | 2465f01d | Menus | automated | LeapFrog logo |
| I Spy: Challenger! | da56181c | Menus | automated | LeapFrog logo (leapsterworld.com) |
| Letterpillar | 9b86a0fb | Menus | automated | LeapFrog logo |
| Letters on the Loose | b186a3bf | Menus | automated | LeapFrog logo |
| NASCAR | 95c78f34 | Menus | automated 2026-10-16 | Region choice, intro, NASCAR logo. (Was stuck on a black screen until the ADC delivered battery samples, 2026-10-16) |
| Number Raiders | 0f6d55cf | Menus | automated | LeapFrog logo |
| Numbers on the Run: Counting on Zero | 01f12ce7 | Menus | automated 2026-10-16 | Region choice, title, name entry. (Was stuck on a black screen until the ADC delivered battery samples, 2026-10-16) |
| Pet Pals | 3c428a13 | Menus | automated | LeapFrog logo |
| Scooby-Doo!: Spooky Snacks | 19da3dd8 | Menus | automated | LeapFrog logo |
| Sonic X | a9d2cdbd | Menus | automated | LeapFrog logo |
| Spider-Man: The Case of the Sinister Speller | 0ef61cbb | Menus | automated | LeapFrog logo |
| Star Wars: Jedi Reading | 6239a178 | Menus | automated | LeapFrog logo (leapsterworld.com) |
| Thomas & Friends: Calling All Engines! | 5303d366 | Menus | automated | Region choice, name entry (typing by touch works), then the main menu; no title card |
| Outwit! | 9e69c26a | Menus | automated | Loading screens, region choice, title at 60 s, then name entry |
| The Talking Words Factory | 58d08f36 | Menus | automated 2026-10-16 | Title, then the factory main menu. The only 16 MB image |

Images whose first bytes are not "LEAP" are later cartridges with a different header layout;
the BIOS boots them all the same.

## Leapster TV (EXPERIMENTAL)

BIOS: Leapster TV Universal v2.1.11 (152-11594). The game picture comes from the emulated TV chip and the
controller (buttons and pen) is Ribbit's model of the TV controller; see hardware-research.md, "Leapster TV".

| Game | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|
| SpongeBob SquarePants: Saves the Day (USA, alt) | 0f2ad725 | Menus | automated 2026-10-15 | LEAPSTER TV logo, title, name entry by stylus (letters and OK work), then the intro, full speed |

## Leapster 2 and L-MAX (EXPERIMENTAL)

Automated check 2026-10-16: 60 s from power-on, no input, straps 0x63FF_FFFF (no calibration screen), each game on
each BIOS, compared screen by screen with the original (US v2.1). "Same" = the same sequence of the game's own
screens (the Leapster 2 BIOSes skip the long LeapFrog logo, so they are a few seconds ahead). BIOSes: Leapster 2
0.6 (152-12206), Leapster 2 CIP3 m9 (152-12659), Leapster Connected B5 (152-12076); L-MAX US v2.1 (152-11238),
v2.2 (152-11476). Touch was checked by hand on the Leapster 2 0.6 with Sonic X (calibration accepted, name entry
typed, OK accepted).

| Game | Leapster 2 (0.6, m9, B5) | L-MAX (v2.1, v2.2) | Notes |
|---|---|---|---|
| 1st Grade, Cosmic Math, Creature Create, Disney Fairies, Dora Camping Adventure, Dora Wildlife Rescue, Madagascar, Foster's Home, I Spy Challenger, Kindergarten, Letterpillar, Letters on the Loose, Number Raiders, Scooby-Doo, Sonic X, Spider-Man, SpongeBob Saves the Day, SpongeBob Through the Wormhole, Star Wars Jedi Math, Thomas & Friends | Same | Same | |
| Disney/Pixar Cars | Same, then name entry | Same | |
| Disney/Pixar Cars 2, Dora Piñata Party, Pet Pals, Outwit, Star Wars Jedi Reading, The Batman (DE) | Same | Same | |
| Disney Princess: Enchanted Learning, Ratatouille | Main menu / intro without name entry | Same | The Leapster 2 supplies a player name (HYPOTHESIS: a real unit does the same) |
| NASCAR, Numbers on the Run | Same | Same | Run since 2026-10-16 on every model |
| The Talking Words Factory | Same (main menu) | Same | The Leapster 2 BIOS needs PCM DMA and the dual 16x16 MAC instructions for it |
| Learning with Leap | not run (file name with spaces skipped by the sweep) | not run | |

Not covered by the automated check: gameplay beyond the first screens, sound on the Leapster 2's PCM path (the
host hears it averaged to 8 kHz), the L-MAX TV output, SD cards, LeapFrog Connect (USB).
