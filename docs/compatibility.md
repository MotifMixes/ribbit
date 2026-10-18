# Compatibility

How far each game gets in Ribbit, for every console it emulates. Update this file whenever a game is tested.
Last updated 2026-10-17 (Ribbit 1.2.1).

## Consoles at a glance

| Console | Year | State in Ribbit | Best result so far | Section |
|---|---|---|---|---|
| Leapster (original handheld) | 2003 | Main target | SpongeBob: Saves the Day **Playable**; Sonic X, Letterpillar and Counting on Zero **Work**; 30 more games reach their own menus | [Leapster](#leapster-original-handheld) |
| Leapster L-MAX | 2005 | Experimental | Every tested game shows the same screens as on the original | [Leapster 2 and L-MAX](#leapster-2-and-l-max-experimental) |
| Leapster TV | 2005 | Experimental | SpongeBob reaches its intro | [Leapster TV](#leapster-tv-experimental) |
| Leapster 2 | 2008 | Experimental | Every tested game shows the same screens as on the original; touch works | [Leapster 2 and L-MAX](#leapster-2-and-l-max-experimental) |
| Zippity | 2009 | Working | Built-in Mickey Says **Playable**; two cartridges reach their game menus | [Zippity](#zippity) |
| RockIt Twist | 2019 | Early emulation | Built-in Nom-Nom **Playable**, videos play | [RockIt Twist](#rockit-twist-early-emulation) |

You need your own console's firmware (BIOS or system dump) and your own game dumps for every console. None are
included.

## Status levels

| Status | Meaning |
|---|---|
| **Perfect** | Plays start to finish with no known problems (graphics, sound, controls, saves) |
| **Playable** | Can be played through; minor problems listed in the notes |
| **Works** | A player reports it working in gameplay; not yet checked to the end |
| **In-game** | Reaches gameplay, but problems stop it being played through |
| **Menus** | The game's own screens run (logo, region or profile menus); gameplay not yet reached |
| **Boots** | The console starts the game, which stops on a loading or blank screen |
| **Nothing** | The console does not accept the game |
| **Untested** | Not tried yet |

"Menus" from an automated check means the game was running its own code but was waiting for input (the check
presses nothing). Most such games will probably go further when played by hand; they simply have not been yet.

## Leapster (original handheld)

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
| Letterpillar | 9b86a0fb | **Works** | tester, Ribbit 1.2.1 (Windows), 2026-10-17 | Reported working by a player |
| Letters on the Loose | b186a3bf | Menus | automated | LeapFrog logo |
| NASCAR | 95c78f34 | Menus | automated 2026-10-16 | Region choice, intro, NASCAR logo. (Was stuck on a black screen until the ADC delivered battery samples, 2026-10-16) |
| Number Raiders | 0f6d55cf | Menus | automated | LeapFrog logo |
| Numbers on the Run: Counting on Zero | 01f12ce7 | **Works** | tester, Ribbit 1.2.1 (Windows), 2026-10-17 | Reported working by a player. (Was stuck on a black screen until the ADC delivered battery samples, 2026-10-16) |
| Pet Pals | 3c428a13 | Menus | automated 2026-10-18 | Region choice, title, the pets' room and pet choice on every Leapster model. **The first start on a cartridge with an empty save shows ~20 s of black screen** while the game creates its save (~690 bytes, VERIFIED; authentic as far as known: the work is the game's own, not timing). Later starts: ~4 s, as a real original Leapster (video) |
| Scooby-Doo!: Spooky Snacks | 19da3dd8 | Menus | automated | LeapFrog logo |
| Sonic X | a9d2cdbd | **Works** | tester, Ribbit 1.2.1 (Windows), 2026-10-17 | Reported working by a player |
| Spider-Man: The Case of the Sinister Speller | 0ef61cbb | Menus | automated | LeapFrog logo |
| Star Wars: Jedi Reading | 6239a178 | Menus | automated | LeapFrog logo (leapsterworld.com) |
| Thomas & Friends: Calling All Engines! | 5303d366 | Menus | automated | Region choice, name entry (typing by touch works), then the main menu; no title card |
| Outwit! | 9e69c26a | Menus | automated | Loading screens, region choice, title at 60 s, then name entry |
| The Talking Words Factory | 58d08f36 | Menus | automated 2026-10-16 | Title, then the factory main menu. The only 16 MB image |

Images whose first bytes are not "LEAP" are later cartridges with a different header layout;
the BIOS boots them all the same.

### Prototypes and unreleased builds

Player reports (tester, 2026-10-18). These images are not in MAME's software list, so no CRC32 is given.

| Build | Status | Checked | Notes |
|---|---|---|---|
| Bratz J2S | **Perfect** | tester, 2026-10-18 | A graphics showcase rather than a game; the tester counts it as fully working |
| Disney Princess: Enchanted Learning (1.8 Beta) | **Works** | tester, 2026-10-18 | Seems to work fine; more testing needed |
| The Incredibles: Animation Sample 1 | **Works** | tester, 2026-10-18 | Boots and runs fine; little content. Shows the red "Content NOT approved by LeapFrog" screen instead of the LeapFrog logo |
| The Incredibles: Animation Submission 2 | **Works** | tester, 2026-10-18 | As Animation Sample 1 |
| ROX Alpha 1.15 | **Works** | tester, 2026-10-18 | Boots and runs; an alpha-quality build |
| ROX Alpha 1.15r2 | **Works** | tester, 2026-10-18 | Boots and runs; revised menus and significant fixes over 1.15; pause works

## Leapster 2 and L-MAX (experimental)

Automated check 2026-10-16: 60 s from power-on, no input, straps 0x63FF_FFFF (no calibration screen), each game on
each BIOS, compared screen by screen with the original (US v2.1). "Same" = the same sequence of the game's own
screens (the Leapster 2 BIOSes skip the long LeapFrog logo, so they are a few seconds ahead). BIOSes: Leapster 2
0.6 (152-12206), Leapster 2 CIP3 m9 (152-12659), Leapster Connected B5 (152-12076); L-MAX US v2.1 (152-11238),
v2.2 (152-11476). Touch was checked by hand on the Leapster 2 0.6 with Sonic X (calibration accepted, name entry
typed, OK accepted).

| Game | Leapster 2 (0.6, m9, B5) | L-MAX (v2.1, v2.2) | Notes |
|---|---|---|---|
| 1st Grade, Cosmic Math, Creature Create, Disney Fairies, Dora Camping Adventure, Dora Wildlife Rescue, Madagascar, Foster's Home, I Spy Challenger, Kindergarten, Letterpillar, Letters on the Loose, Number Raiders, Scooby-Doo, Sonic X, Spider-Man, SpongeBob Through the Wormhole, Star Wars Jedi Math, Thomas & Friends | Same | Same | |
| SpongeBob Saves the Day | Same | **Works** on L-MAX US v2.1 | Reported working by a player (Ribbit 1.2.1, 2026-10-17) |
| Disney/Pixar Cars | Same, then name entry | Same | |
| Disney/Pixar Cars 2, Dora Piñata Party, Pet Pals, Outwit, Star Wars Jedi Reading, The Batman (DE) | Same | Same | |
| Disney Princess: Enchanted Learning, Ratatouille | Main menu / intro without name entry | Same | The Leapster 2 supplies a player name (HYPOTHESIS: a real unit does the same) |
| NASCAR, Numbers on the Run | Same | Same | Run since 2026-10-16 on every model |
| The Talking Words Factory | Same (main menu) | Same | The Leapster 2 BIOS needs PCM DMA and the dual 16x16 MAC instructions for it |
| Learning with Leap | not run (file name with spaces skipped by the sweep) | not run | |

Not covered by the automated check: gameplay beyond the first screens, sound on the Leapster 2's PCM path (the
host hears it averaged to 8 kHz), the L-MAX TV output, SD cards, LeapFrog Connect (USB).

## Leapster TV (experimental)

BIOS: Leapster TV Universal v2.1.11 (152-11594). The game picture comes from the emulated TV chip and the
controller (buttons and pen) is Ribbit's model of the TV controller; see hardware-research.md, "Leapster TV".

| Game | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|
| SpongeBob SquarePants: Saves the Day (USA, alt) | 0f2ad725 | Menus | automated 2026-10-15 | LEAPSTER TV logo, title, name entry by stylus (letters and OK work), then the intro, full speed |

## Zippity

System dump: US system NAND (CRC32 16248b63). Controls are the dance mat, bopper (joystick) and feet keys; see
[zippity.md](zippity.md), "In the Ribbit app".

| Game | Kind | Status | Checked | Notes |
|---|---|---|---|---|
| System menu (Mickey's clubhouse, player sign-in) | Built-in | **Playable** | played 2026-10-16 | Start-up screens timed against a real unit. Sign In names appear about 1 s late and blink without fading |
| Mickey Says | Built-in | **Playable** | played 2026-10-16 | Scores rise as on a real unit. Only some of its command types checked |
| Goofy's Fancy Dancing | Built-in | Untested | | Listed on the menu |
| Disney Princess | Cartridge | Menus | 2026-10-16 | Boots from the cartridge, mounts its file system, reaches the game menu (Ariel's Fireworks Display, Cinderella's Dress Dash, Snow White's Dancing Delight) |
| The Princess and the Frog | Cartridge | Menus | 2026-10-16 | Reaches the game menu. Some backgrounds use a colour format whose U/V order is unverified |
| Other cartridges | Cartridge | Untested | | No other dumps tried yet |

## RockIt Twist (early emulation)

System packages: Bootloader, Base and Firmware-Bulk (`.lfp`). Without installed content the firmware shows its own
"Device Error" screen, which is correct: a real unit shows it too until LeapFrog Connect installs the menu. Games
run only from **unpacked** content (File → Import RockIt Twist games). Encrypted `.lf3` game packs are not run, and
Ribbit will not decrypt them. See [rockit-twist.md](rockit-twist.md).

| Game | Kind | Status | Checked | Notes |
|---|---|---|---|---|
| Menu (profiles, stations) | Built-in | **Playable** | played 2026-10-17 | Profiles and progress are saved |
| Nom-Nom (Arcade) | Built-in | **Playable** | played 2026-10-17 | Title screen, level menu, gameplay, sound |
| Sneak Peeks (videos) | Built-in | **Playable** | played 2026-10-17 | MPEG-4 videos and the start-up logo play |
| Music station | Built-in | Untested | | |
| Albert's Escape | Built-in | Boots | 2026-10-17 | Starts; not played through |
| Portrait-mode Arcade game | Built-in | Boots | 2026-10-17 | Starts in portrait; not played through |
| Banzai Beans: Showdown | Game pack | Untested | | |
| Banzai Beans Pet | Game pack | Untested | | |
| Dinosaur Discoveries | Game pack | Untested | | |
| Trolls Party Time with Poppy | Game pack | Untested | | |
| Cookie's Sweet Treats | Game pack | Untested | | |
| Penelope Penguin: Pet Detective | Game pack | Untested | | |
| Animals, Animals, Animals | Game pack | Untested | | |
| Chloe's Fashion Studio | Game pack | Untested | | |
| Storybook Hotel | Game pack | Untested | | |
| PJ Masks: Save the Day | Game pack | Untested | | |
| Job Quests | Game pack | Untested | | |
| RockIt Pets: Blast off to Space | Game pack | Untested | | |
| Any `.lf3` pack | Encrypted pack | Nothing | | Encrypted; not supported by design |

Some games draw a small box at the top of the screen that a real unit probably does not show (cause unknown).
