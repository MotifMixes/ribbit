# How the Didj creates and maintains its UBI storage

Everything here is read from firmware 1.35.2.4222's root file
system (`/etc/init.d/ubi`, `/etc/ubi/ubi.config`, `/usr/bin/fsck_*.sh`), the 2.6.20 kernel sources (opendidj) and
the kernel log, and was watched happening in Ribbit, which runs that firmware unchanged. Real-console numbers
come from real consoles' logs.

## Layers

```
NAND mtd7 "Brio" (222 MiB, 1776 x 128 KiB blocks)
 └ UBI device ubi1 (attached by the kernel at boot)
    └ one volume, id 0, "ubi_Brio", dynamic, every free block (ubimkvol --maxavsize)
       └ gluebi: the volume shows up as another MTD, "ubi_Brio" (mtd9 without a cartridge)
          └ mtdblock9: a block device over it
             └ FAT (vfat) mounted at /Didj; the same block device is what USB mass storage exposes
```

The cartridge is the same stack on its own chip: mtd "Cartridge" (64 MiB, 16 KiB blocks) -> "ubi_Cartridge"
-> FAT at /Cart. With a cartridge in, its raw MTD "Cartridge" takes mtd9, so ubi_Brio becomes mtd10 /
mtdblock10 and ubi_Cartridge mtd11 (a real console's /proc/mtd; why guides say "mtdblock9 or 10 depending on
cartridge insertion").

## 1. The kernel attaches UBI (boot)

lightning-boot passes `ubi.mtd=Brio ubi.mtd=prg_Brio ubi.mtd=Cartridge ubi.mtd=EXT ubi.mtd=prg_EXT`. The UBI
driver attaches each MTD that exists (prg_Brio, prg_EXT are for manufacturing cartridges; EXT is 0 bytes on a
Didj, "out of reach -- disabled", so those lines print errors on every console; harmless).

Attaching scans every block's EC (erase counter) and VID headers. On an all-erased Brio (fresh flash) UBI
prints "empty MTD device detected" and writes the layout volume (the volume table, two copies). Parameters, the
same on Ribbit and a stock console:

- EC header at byte 0, VID header at 512 (512-byte subpages share page 0), data from 2048: a LEB is 129024 bytes
- 2 blocks for the layout volume, 1 for wear-levelling, 17 (1%) kept for bad-block handling
- the user volume gets all the rest (1756 LEBs), so "available PEBs: 0" afterwards
- wear-levelling threshold 4096

## 2. `/etc/init.d/ubi start` makes sure there is exactly one good volume

For each `MTD:mountpoint:FATlabel` word in ubi.config (`Brio:/Didj:Didj`, `prg_Brio:/opt/Didj:Didj`,
`Cartridge:/Cart:Cartridge`, `EXT:/opt/mfg:MFG_PAYLOAD`, `prg_EXT:/opt/prg_mfg:MFG_PAYLOAD`) whose MTD UBI
attached:

- `/sys/class/ubi/ubiN/volumes_count` is 1 (only the layout volume): "UBI device is empty. Formatting" ->
  `ubimkvol --devn=N --vol_id=0 --name=ubi_<MTD> --maxavsize`.
- 2 volumes but volume 0 isn't named `ubi_<MTD>`: removes it and makes it again.
- volume 0 has `corrupted` = 1: "ERROR: UBI Volume is corrupted. Reformatting." (removed and made again: the
  data is lost).
- any other count: "Unexpected number of volumes! Reformatting."
- no /sys/class/ubi at all: "Error: Can't find UBI! System is unrepairable over USB!" and the script stops.

## 3. The FAT check (this is the tune-up screen)

`/flags/booted` is created after every successful start and removed by `init.d/ubi stop` at a clean shutdown.
So if it is still there at boot, the last session ended without a clean shutdown (battery pulled, crash), and
the script checks the FAT; it also checks when `/flags/needs_repair` is already set:

1. `fsck_watch.sh /dev/mtdblockN /Didj` runs `fsck.vfat -n` (read-only) via `fsck_vfat.sh`, logging to
   /tmp/fsck.log, and kills it after 30 seconds or 20 log lines (exit 3 / 4).
2. If the log only lists files under `/Data/0-2/` or `/Data/GameWide/` (save data) and "Reclaimed" lost
   clusters, `fsck_delete.sh` runs `fsck.vfat -a` for the clusters and deletes those files: the console fixes
   itself and keeps going. Anything else in the log -> error.
3. Exit 0 -> `rm /flags/needs_repair`; otherwise `touch /flags/needs_repair`.
4. With needs_repair set the script refuses to mount ("Brazenly refusing to mount /Didj because
   /flags/needs_repair"); launch_main then shows the tune-up screen and USB mass storage exposes the raw
   mtdblock so LeapFrog Connect can repair the FAT from the PC. After the cable is unplugged ("Disconnect OK")
   the console restarts, the check runs again, and the flag goes away if the FAT is now clean.
5. Read-only production cartridges (`cartinfo -w` says ro) are never checked.

Otherwise it mounts `vfat async,noatime` (rw; ro for production carts). If the mount fails on a writable
volume: "Looks like /Didj doesn't have a file system yet. I'll create one." -> `mkfs.vfat -n Didj -S 512`
(FAT16 with 8-sector clusters), mounted. Finally `touch /flags/booted; sync`.

## What this means for repairs

- A console that boots to the tune-up screen has a UBI layer that is fine and a FAT that `fsck.vfat -n`
  doesn't like. Formatting the FAT over USB (or `fsck.vfat -a /dev/mtdblock9` from a shell) fixes it.
- A broken UBI layer (lost volume table, `corrupted`) is reformatted automatically by the script, losing
  /Didj; the console then needs its packages again (LeapFrog Connect, or Ribbit's own image builder).
- New bad blocks come out of the 17-block reserve; with "available PEBs: 0" there is nothing else to take them
  from, so once the reserve is used up UBI can't remap more ("no eraseblocks found").
- On a badly damaged FAT with many long names, real consoles have been reported taking up to an hour to reach
  the tune-up screen; where that time goes past fsck_watch.sh's 30-second limit is UNKNOWN.

## A 1:1 copy of the NAND

mtd0 to mtd7 tile the whole 256 MiB chip (0x0 to 0x10000000; mtd8 EXT is empty). mtd9 and later are not more
flash: they are the cartridge and the gluebi views of UBI volumes. So `nanddump` of mtd0..mtd7 put end to end
is the whole chip; with `nanddump -o` the spare areas (ECC, bad-block marks) come too. Ribbit boots such a dump:
put `didj.nand`, or `mtd0` .. `mtd7` (any extension), in the Didj firmware folder, and it is used instead of the
packages (`ribbit_didj::nand_dump_in`, `Nand::from_dump`; 268435456 bytes, or 276824064 with spare areas).
