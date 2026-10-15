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
| Disney/Pixar Cars | d4a7e5d4 | Boots | automated | White screen from 7.5 s on, after the LEAPSTER logo: stuck. Runs its own copy of the system code; every task then waits. Same on the UK and Spanish BIOSes; not fixed by a voice-end interrupt or by MAME's GPIO workaround (2026-10-15) |
| Disney Princess: Enchanted Learning | 5ccb2ad3 | Menus | automated | LeapFrog logo |
| Disney Princess: Worlds of Enchantment | 2465f01d | Menus | automated | LeapFrog logo |
| I Spy: Challenger! | da56181c | Menus | automated | LeapFrog logo (leapsterworld.com) |
| Letterpillar | 9b86a0fb | Menus | automated | LeapFrog logo |
| Letters on the Loose | b186a3bf | Menus | automated | LeapFrog logo |
| NASCAR | 95c78f34 | Boots | automated | Black screen from 7.5 s on: stuck. Runs its own copy of the system code; every task then waits. Same on the UK and Spanish BIOSes; not fixed by a voice-end interrupt or by MAME's GPIO workaround (2026-10-15) |
| Number Raiders | 0f6d55cf | Menus | automated | LeapFrog logo |
| Numbers on the Run: Counting on Zero | 01f12ce7 | Boots | automated | Black screen from 5 s on: stuck. Runs its own copy of the system code; every task then waits. Same on the UK and Spanish BIOSes; not fixed by a voice-end interrupt or by MAME's GPIO workaround (2026-10-15) |
| Pet Pals | 3c428a13 | Menus | automated | LeapFrog logo |
| Scooby-Doo!: Spooky Snacks | 19da3dd8 | Menus | automated | LeapFrog logo |
| Sonic X | a9d2cdbd | Menus | automated | LeapFrog logo |
| Spider-Man: The Case of the Sinister Speller | 0ef61cbb | Menus | automated | LeapFrog logo |
| Star Wars: Jedi Reading | 6239a178 | Menus | automated | LeapFrog logo (leapsterworld.com) |
| Thomas & Friends: Calling All Engines! | 5303d366 | Menus | automated | Region choice, name entry (typing by touch works), then the main menu; no title card |
| Outwit! | 9e69c26a | Menus | automated | Loading screens, region choice, title at 60 s, then name entry |
| The Talking Words Factory | 58d08f36 | Boots | automated | Stops with a CPU error: uses XY-memory burst transfers (`sr` to aux 0x9B), not emulated yet. The only 16 MB image |

Images whose first bytes are not "LEAP" are later cartridges with a different header layout;
the BIOS boots them all the same.
