# Saves, save states and speed

## Game saves

Games keep their own progress the way they do on the console (the cartridge's and the console's memory, the Zippity's and the RockIt Twist's storage). Ribbit keeps these for you automatically; there is nothing to save.

The first time a new version of Ribbit starts, it copies all your saves and settings into a `backups` folder in the data folder (the newest five are kept). A save or settings file Ribbit can't read is kept aside as a `.bak` file instead of being written over.

## Save states

A save state is the whole console at one moment: load it to carry on from exactly there.

- **Emulation › Save State** and **Load State** have five slots each; each shows when it was saved.
- `F5` saves to slot 1 and `F9` loads it.
- On Android, the side menu's **Save state** and **Load state** use slot 1, and **Exit game** offers to save one first.

> Save states only load in the Ribbit version that made them. Game saves always carry over to new versions.

## Rewind and fast-forward

- Hold `` ` `` (the key left of `1`) to rewind: the last 30 seconds play backwards while you hold it.
- Hold `Tab` to fast-forward.
- **Emulation › Speed** runs at 50%, 100% (normal), 200%, 400% or as fast as possible. The status bar at the bottom shows the speed the game actually runs at.
