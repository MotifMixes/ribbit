# Compatibility

How far each game gets in Ribbit, for every console it emulates. Update this file whenever a game is tested.
Last updated 2026-10-25 (Ribbit 1.5.1).

## Consoles at a glance

| Console | Year | State in Ribbit | Best result so far | Section |
|---|---|---|---|---|
| Leapster (original handheld) | 2003 | Main target | SpongeBob: Saves the Day **Playable**; Sonic X, Letterpillar and Counting on Zero **Work**; all 122 retail games and 150 prototype builds tested reach their own menus | [Leapster](#leapster-original-handheld) |
| Leapster L-MAX | 2005 | Experimental | Every tested game shows the same screens as on the original | [Leapster 2 and L-MAX](#leapster-2-and-l-max-experimental) |
| Leapster TV | 2005 | Experimental | SpongeBob reaches its intro | [Leapster TV](#leapster-tv-experimental) |
| Leapster 2 | 2008 | Experimental | Every tested game shows the same screens as on the original; touch works | [Leapster 2 and L-MAX](#leapster-2-and-l-max-experimental) |
| Zippity | 2009 | Working | Built-in Mickey Says **Playable**; two cartridges reach their game menus | [Zippity](#zippity) |
| LeapPad (original book-and-pen pad) | 1999 | Early emulation, with sound | Books drawn from their cartridges; every spot answers; At the Shore's words filled in | [LeapPad](#leappad-early-emulation) |
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
BIOS: US v2.1 (152-11265). Automated check (2026-10-25): every cartridge in the user's collection, 30 s from
power-on, no input, one screenshot every 2 s, with the headless `scenes` example. Every one of the 272 images
below boots and runs its own screens (logo, region choice, name entry or title); none faulted. Earlier checks: 70 s
from power-on (Ribbit 1.0.0, 2026-10-15). Played: tested by hand. Region pickers move on by themselves after about
30 s. Game names and regions follow MAME's software list where the dump is in it; version and build
date are read from the cartridge header (version only where the header gives one).

| Game | Region | Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|---|---|
| 1st Grade | USA | - | 2003-10-09 | db36283f | Menus | automated 2026-10-25 | On-screen letter keyboard (name entry) |
| 2nd Grade - Musical Menace | USA | - | 2004-08-19 | 597418d4 | Menus | automated 2026-10-25 | Own screens within 30 s |
| A La Conquête Des Nombres - Un Jeu D'Arcade Éducatif! | France | v1.0 | 2006-05-04 | c12334cf | Menus | automated 2026-10-25 | Own screens within 30 s |
| Adibou - A La Recherche De Robitoc | France | - | 2006-07-19 | b0ba6d76 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Bob Leponge - Heros D'un Jour | France | - | 2006-06-21 | 7ccf1715 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Bratz World - The Jet Set | USA | - | 2004-12-11 | 5d4f7cd0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Cars | France | v1.0 | 2006-05-15 | b6aabb7b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Cars | Germany | v1.0 | 2006-06-15 | c7b62602 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Cars | Spain | v1.0 | 2006-05-07 | 2a704606 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Cars | USA | v1.0 | 2005-12-09 | d4a7e5d4 | Menus | automated 2026-10-25 | Region choice, Pixar logo, title. (Was stuck on a white screen until the ADC delivered battery samples, 2026-10-16) |
| Cars - Supercharged | USA | v1.0 | 2007-03-21 | beca3909 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Cars 2 | USA | - | 2011-01-11 | 29af42ba | Menus | automated 2026-10-25 | Region choice |
| Clifford - The Big Red Dog - Reading | USA | - | 2006-06-14 | 0134af49 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Cosmic Math - Arcade-Style Learning! | USA | v1.0 | 2005-07-20 | 979346d5 | Menus | automated 2026-10-25 | LeapFrog logo (game's own splash) |
| Cosmic Math - Arcade-Style Learning! | USA | v1.1 | 2006-04-07 | 3531ad05 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Crayola - Art Adventure | USA | - | 2011-06-01 | 0eec61a7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Creature Create | USA | - | 2007-02-12 | 9f08d02a | Menus | automated 2026-10-25 | Region choice (U.S.A. / Canada / U.K.) |
| Demo - Herbst 2004 II | Germany | - | 2004-10-14 | 71062cd7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Didou - Dessine-moi une aventure ! | France | - | 2008-05-28 | 5a66a307 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Dis Pourquoi Kirikou | France | - | 2008-06-05 | fdcd1321 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney Fairies | Spain | - | 2009-05-13 | d2f560c7 | Menus | automated 2026-10-25 | Region choice (America / Europa) |
| Disney Fairies | USA | - | 2009-03-17 | 8a605512 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney Princesa - La Magia De Aprender | Spain | - | 2006-06-23 | 5a52d3f0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney Princess | USA | - | 2008-04-09 | 5ccb2ad3 | Menus | automated 2026-10-25 | LeapFrog logo |
| Disney Princess - Enchanted Learning | USA | - | 2004-06-22 | 8c2fe2e8 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney Princess - Worlds of Enchantment | USA | - | 2008-03-26 | 2465f01d | Menus | automated 2026-10-25 | LeapFrog logo |
| Disney Princesse - La Magie D'Apprendre | France | - | 2006-06-21 | 69eaa1ea | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney Prinzessinnen - Zauberhaftes Lernen | Germany | - | 2005-04-07 | bb6d5375 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney Tangled | USA | - | 2011-01-24 | ce9ce136 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney-Pixar Findet Nemo | Germany | - | 2005-03-03 | 6a5e20ba | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney-Pixar Finding Nemo | USA | - | 2006-03-13 | 1895f946 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney-Pixar Toy Story 3 | France | - | 2010-03-29 | db3522eb | Menus | automated 2026-10-25 | Own screens within 30 s |
| Disney-Pixar Toy Story 3 | USA | - | 2010-01-28 | a72c61fd | Menus | automated 2026-10-25 | Own screens within 30 s |
| Dora L'Exploratrice - Au Secours Des Animaux | France | - | 2006-05-18 | e557c760 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Dora L'Exploratrice - Le Pont Casse | France | - | 2006-04-12 | b6607eee | Menus | automated 2026-10-25 | Own screens within 30 s |
| Foster's Home for Imaginary Friends | USA | - | 2007-05-14 | e62d1684 | Menus | automated 2026-10-25 | Region choice |
| Get Puzzled! | USA | - | 2007-02-08 | 05b608f0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Grundschule 1 | Germany | - | 2004-09-17 | 6fd44f1a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Grundschule 2 - Musik in Gefahr | Germany | - | 2005-02-09 | 96d30f93 | Menus | automated 2026-10-25 | Own screens within 30 s |
| I Spy - Treasure Hunt | USA | - | 2008-01-08 | 7c716016 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Kindergarten | USA | v1.0 | 2003-09-09 | db262164 | Menus | automated 2026-10-25 | Title screen |
| La Rivière Des Lapins + L'Atelier De Peinture | France | - | 2006-05-10 | e8c84241 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Lapin Malin - Danse Avec Les Mots | France | - | 2007-03-08 | 680f3323 | Menus | automated 2026-10-25 | Own screens within 30 s |
| LeapFrog SchoolHouse - Story Explorers | USA | - | 2007-03-29 | 9a3bed14 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Leapster In-Store Demo | USA, POP LP7 | - | 2007-04-30 | 9d614433 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Learning with Leap | USA | - | 2003-08-24 | 2c1fcf40 | Menus | automated 2026-10-25 | Title screen |
| Lernen mit Leap | Germany | - | 2004-09-02 | e1f505e7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Lernst du Malen & Schreiben - Mit Bruno Bleistift | Germany | - | 2004-12-28 | e58eac06 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Letter Factory | USA | v1.0 | 2003-11-05 | 9d1c2f35 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Letterpillar | USA | v1.1 | 2006-04-06 | 9b86a0fb | **Works** | tester, Ribbit 1.2.1 (Windows), 2026-10-17 | Reported working by a player |
| Letters on the Loose | USA, L-MAX | v1.0 | 2005-04-20 | b186a3bf | Menus | automated 2026-10-25 | LeapFrog logo |
| Madagascar | USA, L-MAX | - | 2005-05-19 | a9c75d90 | Menus | automated 2026-10-25 | Region choice |
| Marvel Spider-Man - The Case of the Sinister Speller | USA | v1.0 | 2004-09-08 | 0ef61cbb | Menus | automated 2026-10-25 | LeapFrog logo |
| Math Baseball | USA | - | 2003-12-22 | fe8bd3fd | Menus | automated 2026-10-25 | Own screens within 30 s |
| Mr. Pencil's Learn to Draw & Write | USA | - | 2004-06-28 | dbb767cc | Menus | automated 2026-10-25 | Own screens within 30 s |
| My Amusement Park | USA | - | 2009-04-17 | 2fc77c9d | Menus | automated 2026-10-25 | LOADING screen from about 16 s to 40 s, then name entry; real-hardware loading time not checked |
| NASCAR | USA, L-MAX | v1.0 | 2006-02-24 | 95c78f34 | Menus | automated 2026-10-25 | Region choice, intro, NASCAR logo. (Was stuck on a black screen until the ADC delivered battery samples, 2026-10-16) |
| Ni Hao, Kai-lan | USA | - | 2009-02-10 | 686ece3f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Nick Jr. Dora - Retter der Wildnis | Germany | - | 2006-11-09 | 242907d4 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Nick Jr. Dora the Explorer - Camping Adventure | USA | v1.0 | 2008-03-13 | 71778536 | Menus | automated 2026-10-25 | Region choice |
| Nick Jr. Dora the Explorer - Piñata Party! - Arcade Style Learning! | USA | - | 2006-03-29 | ea544030 | Menus | automated 2026-10-25 | Region choice |
| Nick Jr. Dora the Explorer - Wildlife Rescue | USA | - | 2003-10-23 | 2a20cbbb | Menus | automated 2026-10-25 | Own screens within 30 s |
| Nick Jr. Dora the Explorer - Wildlife Rescue | USA, L-MAX | - | 2005-03-30 | 74af5d57 | Menus | automated 2026-10-25 | Title screen |
| Nick Jr. Go Diego Go! - Animal Rescuer | USA | - | 2008-04-26 | ebbf83a0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Nick Jr. Go Diego Go! - Animal Rescuer | USA | v1.0 | 2007-05-08 | dcdd48bd | Menus | automated 2026-10-25 | Own screens within 30 s |
| Nick Jr. The Backyardigans | USA | - | 2008-03-06 | de5fb26e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Nickelodeon The Penguins of Madagascar - Race for 1st Place! | USA | - | 2010-03-10 | 076ad299 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Number Raiders - Arcade-Style Learning! | USA | v1.0 | 2005-06-29 | 0f6d55cf | Menus | automated 2026-10-25 | LeapFrog logo |
| Numbers on the Run - Counting on Zero | USA, L-MAX | v1.0 | 2005-06-14 | 01f12ce7 | **Works** | tester, Ribbit 1.2.1 (Windows), 2026-10-17 | Reported working by a player. (Was stuck on a black screen until the ADC delivered battery samples, 2026-10-16) |
| Oui-Oui - Aventures Au Pays Des Jouets | France | - | 2007-08-01 | 24c28bcd | Menus | automated 2026-10-25 | Own screens within 30 s |
| Perrito Club - ¡Adopta Un Nuevo Amiguito Y Aprende! | Spain | - | 2007-06-06 | b6d630e0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Pet Pals | USA | - | 2008-03-03 | 3c428a13 | Menus | automated 2026-10-25 | Region choice, title, the pets' room and pet choice on every Leapster model. **The first start on a cartridge with an empty save shows ~20 s of black screen** while the game creates its save (~690 bytes, VERIFIED; authentic as far as known: the work is the game's own, not timing). Later starts: ~4 s, as a real original Leapster (video) |
| Phonics - Lesson One | USA | - | 2004-02-13 | 0b83b8fa | Menus | automated 2026-10-25 | Intro, then the "The Day Leap Ate Olives" title |
| Ratatouille | France | - | 2007-06-13 | 8f835af7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Ratatouille | Germany | - | 2007-06-11 | df729fe7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Ratatouille | USA | - | 2008-03-10 | d0c4de47 | Menus | automated 2026-10-25 | Region choice |
| Reading with Phonics - Mole's Huge Nose | USA | - | 2004-05-24 | 2c6e623e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Rock the World - A Reading Adventure | USA, L-MAX | - | 2005-08-10 | 49aab0fe | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scholastic - Animal Genius | USA | - | 2006-03-08 | af063ba4 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scholastic - Digging for Dinosaurs | USA | - | 2009-02-20 | 822ca3da | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scholastic - I Spy - Challenger | USA | - | 2005-06-06 | da56181c | Menus | automated 2026-10-25 | LeapFrog logo (leapsterworld.com) |
| Scholastic - Math Missions | USA | - | 2005-11-02 | 7d7fc192 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scholastic - OutWit! | USA | - | 2008-02-14 | 9e69c26a | Menus | automated 2026-10-25 | Loading screens, region choice, title at 60 s, then name entry |
| School House Rock! - America Rock | USA | - | 2004-11-23 | f3c31faf | Menus | automated 2026-10-25 | Own screens within 30 s |
| School House Rock! - Grammar Rock | USA | - | 2004-11-16 | 53c9d9b2 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scooby Doo! | USA | - | 2009-09-02 | 4cd2c497 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scooby-Doo! - Et Les Nombres Fous! | France | - | 2006-10-26 | 648e3ee5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Scooby-Doo! - Spooky Snacks! | USA | - | 2006-03-23 | 19da3dd8 | Menus | automated 2026-10-25 | LeapFrog logo |
| Sonic X | USA | - | 2008-04-26 | 44000e5c | Menus | automated 2026-10-25 | Own screens within 30 s |
| Sonic X | USA | v1.0 | 2006-07-12 | a9d2cdbd | **Works** | tester, Ribbit 1.2.1 (Windows), 2026-10-17 | Reported working by a player |
| Spider-Man - L'Affaire Des Mots Mystères | France | - | 2006-06-12 | cbe0de6f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Spider-Man - Schachmatt den Schildersaboteuren! | Germany | - | 2005-06-16 | 9dc2e5f6 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Spider-Man - The Case of the Sinister Speller | USA, L-MAX | - | 2005-03-29 | 2eee5925 | Menus | automated 2026-10-25 | Own screens within 30 s |
| SpongeBob Schwammkopf - Zeitreise durch das Wurmloch | Germany | - | 2007-10-26 | 8761fc61 | Menus | automated 2026-10-25 | Own screens within 30 s |
| SpongeBob Schwammkopf Hat Alles im Griff | Germany | - | 2004-09-27 | d11e7c30 | Menus | automated 2026-10-25 | Own screens within 30 s |
| SpongeBob SquarePants - Saves the Day | USA | - | 2003-10-09 | 0f2ad725 | **Playable** | played 2026-10-14 | All four minigames reach gameplay at full speed. Touch is the experimental model |
| SpongeBob SquarePants - Saves the Day | USA | - | 2006-03-23 | e94ae85d | Menus | automated 2026-10-25 | Own screens within 30 s |
| SpongeBob SquarePants - Saves the Day | USA, L-MAX | - | 2005-04-12 | 4d893b32 | Menus | automated 2026-10-25 | Own screens within 30 s |
| SpongeBob SquarePants - Through The Wormhole | USA | - | 2007-03-21 | 6b24a3ef | Menus | automated 2026-10-25 | Region choice |
| Star Wars - Jedi Math | USA | - | 2008-03-07 | 27000674 | Menus | automated 2026-10-25 | Region choice |
| Star Wars - Jedi Reading | USA | - | 2008-08-05 | 6239a178 | Menus | automated 2026-10-25 | LeapFrog logo (leapsterworld.com) |
| Star Wars - Matemáticas Jedi | Spain | - | 2008-06-13 | cf835b84 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Talking WORDS Factory | USA | v1.0 | 2004-03-09 | 58d08f36 | Menus | automated 2026-10-25 | Title, then the factory main menu. The only 16 MB image |
| The Batman - El Poder De Los Números | Spain | v1.0 | 2007-06-19 | 6a7afd1c | Menus | automated 2026-10-25 | Own screens within 30 s |
| The Batman - Multipliziere, dividiere und regiere | Germany | v1.0 | 2006-01-19 | a0a362aa | Menus | automated 2026-10-25 | Kids' WB / DC logos |
| The Batman - Multiply, Divide and Conquer | USA | v1.0 | 2005-03-09 | bf60ee5f | Menus | automated 2026-10-25 | Own screens within 30 s |
| The Batman - Strength in Numbers | USA | v1.0 | 2007-03-16 | 7c934681 | Menus | automated 2026-10-25 | Own screens within 30 s |
| The Disney-Pixar Collection | USA | - | 2011-03-11 | d8ac1a09 | Menus | automated 2026-10-25 | Own screens within 30 s |
| The Incredibles | USA | - | 2004-08-25 | f1d6db36 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Thomas & Friends - Calling all Engines! | USA | - | 2005-03-28 | 5303d366 | Menus | automated 2026-10-25 | Region choice, name entry (typing by touch works), then the main menu; no title card |
| Top-Secret Personal Beeswax - Share a journal with Junie B. | USA | - | 2004-07-23 | 78fce75a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Up | Spain | - | 2009-04-16 | 975a0a85 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Up | USA | - | 2009-01-14 | 1e2a5031 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Vorschule | Germany | v1.0 | 2004-09-10 | 4c5e775e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Wall-E | Germany | v1.0 | 2008-05-21 | ee2c39ea | Menus | automated 2026-10-25 | Own screens within 30 s |
| Wall-E | Spain | v1.0 | 2008-05-08 | c10ab3dd | Menus | automated 2026-10-25 | Own screens within 30 s |
| Wall-E | USA | v1.0 | 2008-01-14 | c71cff35 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Weltraum-Mathe - Lernen im Arcade-Stil! | Germany | v1.0 | 2006-02-16 | 75164c8e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Wolverine and the X-Men | USA | - | 2009-04-28 | 67bd4a6a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Word Chasers - Arcade-Style Learning! | USA | v1.1 | 2006-04-06 | f5a3052a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Wörterjäger - Lernen im Arcade-Stil! | Germany | v1.0 | 2006-03-27 | a707de61 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Zahlenjäger - Lernen im Arcade-Stil! | Germany | v1.0 | 2006-01-05 | 334ab5d0 | Menus | automated 2026-10-25 | Own screens within 30 s |

Images whose first bytes are not "LEAP" are later cartridges with a different header layout;
the BIOS boots them all the same.

### Prototypes and unreleased builds

Not in MAME's software list. Dev versions (Alpha, Beta) are LeapFrog's own build numbers from the original file
names; build dates are from the cartridge header. Builds marked **Works** or **Perfect** were reported by a tester
(2026-10-18); the rest are from the automated check. Prototypes may show a red "Content NOT approved by LeapFrog"
screen instead of the LeapFrog logo; that is the game's own screen.

<details><summary>2nd Grade - Musical Menace (USA): 2 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 1.15 | 2004-06-15 | f558b045 | **Works** | tester, 2026-10-18 | Boots and runs; an alpha-quality build. Also reaches its own screens in the automated check (2026-10-25) |
| Alpha 1.15r2 | 2004-06-17 | 95da07f0 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Bratz World - The Jet Set (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2003-12-24 | abcce1bb | **Perfect** | tester, 2026-10-18 | A graphics showcase rather than a game; the tester counts it as fully working. Also reaches its own screens in the automated check (2026-10-25) |

</details>

<details><summary>Cars (France): 11 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 1.01 | 2006-03-27 | beb80b0b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.02 | 2006-04-05 | 850bb7eb | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.03 | 2006-04-11 | 310a5224 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.04 | 2006-04-18 | 547457a9 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.05 | 2006-04-28 | bc86e5af | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.06 | 2006-05-03 | f2233a88 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.07 | 2006-05-04 | d22f7358 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.08 | 2006-05-05 | 7f8ceb1e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.09 | 2006-05-08 | 5d05888b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.01 | 2006-05-10 | f80620c5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2006-05-12 | 356f476f | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Cars (Germany): 10 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 1.01b | 2006-04-12 | fcca14b3 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.02 | 2006-04-21 | 2dbc1375 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.03 | 2006-04-28 | 8320bc26 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.04 | 2006-05-05 | 61699a2f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.05 | 2006-05-12 | 0da2f79a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.06 | 2006-05-22 | 8bb67af6 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.07 | 2006-05-23 | ec5e02aa | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.01 | 2006-06-02 | 958610b0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2006-06-06 | dcf001bb | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.03 | 2006-06-08 | 0a437927 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Cars (Spain): 13 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 1.01 | 2006-03-22 | 7682b893 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.02 | 2006-03-30 | e5cf40f9 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.03 | 2006-04-04 | b3110cd5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.04 | 2006-04-07 | b97fa29f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.05 | 2006-04-13 | aed3a825 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.06 | 2006-04-21 | e432866b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.07 | 2006-04-26 | 6a95ed59 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.08 | 2006-04-27 | dae3281e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.09 | 2006-04-28 | dff2deea | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.10 | 2006-05-01 | 81cc820a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.11 | 2006-05-02 | 246644da | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2006-05-03 | 67a7a445 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.03 | 2006-05-05 | a9de6b49 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Cars (USA): 3 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2005-07-26 | 3ba2351d | Menus | automated 2026-10-25 | Own screens within 30 s |
| - | 2005-08-08 | 30c1043e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.13 | 2005-11-08 | ee97e89f | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Cars - Supercharged (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 002 | 2008-04-23 | f85d963a | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Disney Princess - Enchanted Learning (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Beta 1.8 | 2004-05-17 | 704c4891 | **Works** | tester, 2026-10-18 | Seems to work fine; more testing needed. Also reaches its own screens in the automated check (2026-10-25) |

</details>

<details><summary>Disney Princess - Worlds of Enchantment (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2007-02-26 | c603c575 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Dora the Explorer - Wildlife Rescue (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2005-02-11 | d3ab3a70 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Foster's Home for Imaginary Friends (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2007-03-20 | 7a6813c0 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Go Diego Go! - Animal Rescuer (USA): 14 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 1.07 | 2007-03-15 | 12cb4755 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.08 | 2007-03-22 | d4b8754d | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.09 | 2007-03-23 | 9f0fd4c8 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.01 | 2007-03-27 | d086c10b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.02 | 2007-03-28 | e8098be0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.03 | 2007-04-03 | 1ad404f0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| - | 2007-04-05 | 28b9d48b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.05 | 2007-04-10 | 0afdbd41 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.08 | 2007-04-18 | 8cde814f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.01 | 2007-04-20 | 46a27ea9 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2007-04-24 | c7c232c3 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.03 | 2007-04-26 | 1790b02a | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.04 | 2007-05-01 | 3ae05f02 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.05 | 2007-05-04 | ef62a302 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Leapster TV Kiosk Demo (Germany): 7 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Pre-Alpha 1.01 | 2006-06-08 | 006cff85 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.01 | 2006-06-15 | c68ea61d | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.02 | 2006-06-20 | 4fefd6ea | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.03 | 2006-06-22 | 31c61d0b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.01 | 2006-06-27 | fa95412d | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2006-06-29 | 8b33e558 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.03 | 2006-06-30 | e408462f | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Leapster TV Kiosk Demo (Spain): 8 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Pre-Alpha 1.01 | 2006-05-01 | 106296c5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.01 | 2006-05-18 | a1c76a7f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.02 | 2006-05-19 | dbf6aca9 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.03 | 2006-05-24 | b87e1127 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.01 | 2006-05-26 | f584fd88 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2006-06-01 | 8c139a3d | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.03 | 2006-06-05 | dbea7896 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.04 | 2006-06-06 | cfe5ea23 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>NASCAR (USA): 42 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Pre-Alpha 1.0 | 2005-09-21 | 9a90adb6 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Pre-Alpha 1.02 | 2005-10-24 | a02d47bd | Menus | automated 2026-10-25 | Own screens within 30 s |
| Pre-Alpha 1.03 | 2005-10-25 | dd9ab0f2 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Pre-Alpha 1.04 | 2005-10-26 | 4056f874 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.01 | 2005-10-27 | 2bd79012 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.02 | 2005-11-08 | 9b8a9a88 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.03 | 2005-11-15 | 7fdeacb5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.04 | 2005-11-29 | 72ebd594 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.05 | 2005-12-02 | a0f90da7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.06 | 2005-12-06 | a7cc0994 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.07 | 2005-12-09 | 210d78c8 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.08 | 2005-12-13 | efed9446 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.09 | 2005-12-14 | 017728b0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.10 | 2005-12-16 | a50acc35 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.11 | 2005-12-19 | 8e1066c4 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.12 | 2005-12-20 | 5367b7aa | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.13 | 2005-12-21 | 9708a5f0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.14 | 2005-12-23 | 6f04be86 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.01 | 2006-01-06 | 73c7f92f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.02 | 2006-01-10 | 9cfc5ef3 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.03 | 2006-01-13 | 7f4881ba | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.04 | 2006-01-17 | 35efae33 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.05 | 2006-01-20 | a10fb7c3 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.06 | 2006-01-24 | 82732336 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.07 | 2006-01-24 | de794dc5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.08 | 2006-01-25 | e57b70ae | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.09 | 2006-01-27 | f1d538d9 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.10 | 2006-01-30 | 8b22620c | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.11 | 2006-01-31 | 845f931c | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.12 | 2006-02-01 | ffcd0b89 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.13 | 2006-02-03 | 25b70cfb | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.14 | 2006-02-03 | d733979c | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.01 | 2006-02-08 | 6cd7d79b | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.02 | 2006-02-10 | a2e55804 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.03 | 2006-02-13 | 8acf7010 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.04 | 2006-02-14 | bd0a6222 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.05 | 2006-02-15 | 1a05aa26 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.06 | 2006-02-16 | 0eaf3f0c | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.07 | 2006-02-17 | cce21349 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.08 | 2006-02-18 | 76a4e613 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.09 | 2006-02-19 | 3dc64f80 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.10 | 2006-02-20 | ce135039 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Numbers on the Run - Counting on Zero (UK): 2 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 1.1 | 2005-07-25 | df795fba | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 1.2 | 2005-07-29 | ff79720b | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Numbers on the Run - Counting on Zero (USA): 21 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| Alpha 2.4 | 2005-04-29 | 7ba5d123 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.5 | 2005-05-02 | a93b66fc | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.6 | 2005-05-03 | d69be670 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.7 | 2005-05-04 | 67c829b0 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.8 | 2005-05-05 | c92c8bb1 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Alpha 2.9 | 2005-05-10 | 75dc37c2 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.1 | 2005-05-12 | f88660a7 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.2 | 2005-05-17 | 083fd72f | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.3 | 2005-05-19 | db787b6d | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.4 | 2005-05-23 | da883470 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.5 | 2005-05-24 | a873dae5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.6 | 2005-05-26 | 41df69db | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.7 | 2005-05-27 | 2c121b6e | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.8 | 2005-05-30 | 743ef522 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.9 | 2005-06-01 | 80248fad | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.10 | 2005-06-02 | 7b266e03 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.11 | 2005-06-03 | 38a54c95 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.12 | 2005-06-06 | c2ace2dc | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.13 | 2005-06-07 | 824ebdaf | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.14 | 2005-06-08 | ff3365d6 | Menus | automated 2026-10-25 | Own screens within 30 s |
| Beta 1.15 | 2005-06-08 | ac5b5c67 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Pet Pals (USA): 3 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2006-10-17 | 80b568d5 | Menus | automated 2026-10-25 | Own screens within 30 s |
| - | 2006-11-30 | 21753990 | Menus | automated 2026-10-25 | Own screens within 30 s |
| - | 2006-12-13 | 1501fe3c | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Scooby-Doo! - Spooky Snacks! (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2005-10-25 | c12b39a7 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>Spider-Man - The Case of the Sinister Speller (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2004-05-18 | e792cd2b | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>SpongeBob SquarePants - Saves the Day (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2006-04-28 | a43697bf | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>SpongeBob SquarePants - Through the Wormhole (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2006-11-30 | c950058f | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>The Backyardigans (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2005-11-15 | 7a65fa2b | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

<details><summary>The Incredibles - Animation Sample (USA): 2 builds</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2004-03-22 | 748faa1d | **Works** | tester, 2026-10-18 | Boots and runs fine; little content. Shows the red "Content NOT approved by LeapFrog" screen instead of the LeapFrog logo. Also reaches its own screens in the automated check (2026-10-25) |
| - | 2004-04-13 | ff6f07f1 | **Works** | tester, 2026-10-18 | As Animation Sample 1. Also reaches its own screens in the automated check (2026-10-25) |

</details>

<details><summary>Wall-E (USA): 1 build</summary>

| Version | Build date | CRC32 | Status | Checked | Notes |
|---|---|---|---|---|---|
| - | 2007-12-20 | 2c247582 | Menus | automated 2026-10-25 | Own screens within 30 s |

</details>

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
run only from **unpacked** content (Emulation → RockIt Twist → Import games). Encrypted `.lf3` game packs are not run, and
Ribbit will not decrypt them. See [rockit-twist.md](rockit-twist.md).

| Game | Kind | Status | Checked | Notes |
|---|---|---|---|---|
| Menu (profiles, stations) | Built-in | **Playable** | played 2026-10-17 | Profiles and progress are saved |
| Nom-Nom (Arcade) | Built-in | **Playable** | played 2026-10-17 | Title screen, level menu, gameplay, sound |
| Sneak Peeks (videos) | Built-in | **Playable** | played 2026-10-17 | MPEG-4 videos and the start-up logo play |
| Music station | Built-in | **Works** | 2026-10-23 | MP3 songs play (stereo checked against a reference decoder); the start-up logo's sound too |
| Albert's Escape | Built-in | **Works** | 2026-10-23 | Played in level 1; a player finds Albert's movement and some colours off (not found yet) |
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

## LeapPad (early emulation)

Base ROMs: 152-10620 "2MB Canada Full Base ROM" (best: accepts every cartridge and holds the shared sounds the older books use) or 155-00450 "LeapPad ILA2 Universal Base ROM" (MAME's `leappad`). Sound plays (music, effects, speech); the power-on
tune's notes, timing and loudness match a recording of a real pad. For cartridges with the "Prebuilt TnT" app Ribbit draws the book's spots,
GO tabs, stop sign and sentence buttons from the cartridge itself (no page pictures), and does the same for LeapPad Classic cartridges.
The printed words are filled in on the player's PC by listening to the spots (speech recognition; nothing transcribed ships with Ribbit).
Details: [leappad.md](leappad.md), books: [leappad-books.md](leappad-books.md).

| Cartridge | Part | Status | Notes |
|---|---|---|---|
| Tutter's Tiny Trip | 500-00406 | **Menus** | The base checks it ("Cart OK") and runs its kernel and app. Pen touches reach its software at the right place (24 targets, worst error 0.63%); a GO touch changes its view. Start-up music, announcement and idle music play; the ON/OFF switch powers it down after its sound. Its book is drawn from the cartridge: all 914 spots on 12 spreads answer where drawn. |
| Bounce, Tigger, Bounce; Finding Nemo; Disney Princess Stories | 500-00652, 500-01148, 500-00730 | **Menus** | Book drawn from the cartridge; the spots of the spreads tried (1-4) answer where drawn. Not otherwise played. |
| The Messy Mystery; Cars; Leap's Pond (Mind Station) | 500-01022-B, 500-11374, 540-00166 | Untested | The layout reads (Cars: GO tabs only). |
| The pad alone (built-in book) | — | **Menus** | Runs its built-in "TnT" app. |
| LeapPad Classic: Leap's Friends From A to Z, Once Upon A Time, The Bike Race, At the Shore, On the Train to Maine, At the Art Mart, Squire Hare and Sir Turtle | 500-00263 (= 00973-B), 00803, 11874, 11878-B, 11882, 11885, 11887 | **Menus** | At the Shore: every sentence's words heard right. Run their own kernel and app (Emulation > LeapPad (1999) > "Play LeapPad Classic cartridges", on by default). Book drawn from the cartridge; every spot of every spread answers where drawn, in all three reading modes (00803: one spot silent). 00263, 11874 and 11878-B use sounds of the 2 MB Full Base ROM; with the ILA2 base those sounds are silent. With the Canada Full Base, 00263, 11878-B and Tutter's Tiny Trip were also touched in full (all answer). |
| The Birthday Surprise; Leap's Pond Premier Issue | 500-00367, 500-00394 | Untested | LeapPad Classic. Their start-up music never ends, so they never take touches (cause unknown). Book drawn from the cartridge. |
| Tad Goes Shopping | 500-00104 | Bad dump | The known dump lost address line A8 (half of every 512 bytes repeats); Ribbit says so instead of starting it. A good dump is needed. |

