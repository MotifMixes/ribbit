//! The /sys and /proc files the Didj's programs read, which its kernel
//! makes. Values VERIFIED: read on Original (the real kernel in Ribbit,
//! board ID 3, no cartridge, on the speaker) with `cat` over the serial
//! console, 2026-11-07.

use super::vfs::Vfs;

const PLATFORM: &str = "/sys/devices/platform";

/// (file under /sys/devices/platform, contents).
const PLATFORM_FILES: &[(&str, &str)] = &[
    ("lf1000-gpio/board_id", "3\n"),
    ("lf1000-gpio/cart_id", "0\n"),
    ("lf1000-nand/cartridge", "none\n"),
    ("lf1000-alvgpio/scratchpad", "00000060\n"),
    ("lf1000-power/status", "2\n"),
    ("lf1000-power/external", "0\n"),
    ("lf1000-power/voltage", "5973\n"),
    ("lf1000-power/shutdown", "0\n"),
    ("lf1000-audio/output", "speaker\n"),
    ("lf1000-audio/mute", "on\n"),
    ("lf1000-audio/headphone", "183\n"),
    ("lf1000-audio/speaker", "231\n"),
    ("lf1000-audio/volume", "0\n"),
    ("lf1000-audio/jack_high", "0x00\n"),
    ("lf1000-audio/jack_low", "0x00\n"),
    ("lf1000-audio/stats", "Total underruns: 0\n"),
    ("lf1000-usbgadget/vbus", "0\n"),
    ("lf1000-usbgadget/gadget/disconnect", "1\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/needs_repair", "0\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/enabled", "0\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/locked", "1\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/file", "/dev/mtdblock9\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/ro", "0\n"),
    ("lf1000-dpc/backlight", "-11\n"),
    ("lf1000-dpc/lcd_id", "0x0\n"),
    ("lf1000-adc/channel0", "512\n"),
    ("lf1000-adc/channel1", "512\n"),
    ("lf1000-adc/channel2", "760\n"),
    ("lf1000-adc/channel3", "0\n"),
    ("lf1000-pwm/rate", "9800000\n"),
];

const MEMINFO: &str = "MemTotal:        16556 kB\nMemFree:          6972 kB\nBuffers:            52 kB\n\
Cached:           6396 kB\nSwapCached:          0 kB\nActive:           2620 kB\nInactive:         4220 kB\n\
SwapTotal:           0 kB\nSwapFree:            0 kB\nDirty:               0 kB\nWriteback:           0 kB\n\
AnonPages:         420 kB\nMapped:            508 kB\nSlab:             1512 kB\nSReclaimable:      320 kB\n\
SUnreclaim:       1192 kB\nPageTables:         84 kB\nNFS_Unstable:        0 kB\nBounce:              0 kB\n\
CommitLimit:      8276 kB\nCommitted_AS:     1772 kB\nVmallocTotal:   368640 kB\nVmallocUsed:     15200 kB\n\
VmallocChunk:   347132 kB\n";

const CPUINFO: &str = "Processor\t: ARM926EJ-S-LF1000 rev 5 (v5l)\nBogoMIPS\t: 786.43\n\
Features\t: swp half fastmult edsp java \nCPU implementer\t: 0x41\nCPU architecture: 5TEJ\nCPU variant\t: 0x0\n\
CPU part\t: 0x926\nCPU revision\t: 5\nCache type\t: write-back\nCache clean\t: cp15 c7 ops\n\
Cache lockdown\t: format C\nCache format\t: Harvard\nI size\t\t: 16384\nI assoc\t\t: 4\nI line length\t: 32\n\
I sets\t\t: 128\nD size\t\t: 16384\nD assoc\t\t: 4\nD line length\t: 32\nD sets\t\t: 128\n\n\
Hardware\t: ARM-LF1000\nRevision\t: 0000\nSerial\t\t: 0000000000000000\n";

const MTD: &str = "dev:    size   erasesize  name\nmtd0: 00020000 00020000 \"LF1000_uniboot\"\n\
mtd1: 000e0000 00020000 \"Atomic_Boot_Flags\"\nmtd2: 00100000 00020000 \"Manufacturing_Data\"\n\
mtd3: 00200000 00020000 \"Kernel0\"\nmtd4: 00e00000 00020000 \"Linux_RFS0\"\nmtd5: 00200000 00020000 \"Kernel1\"\n\
mtd6: 00e00000 00020000 \"Linux_RFS1\"\nmtd7: 0de00000 00020000 \"Brio\"\nmtd8: 00000000 00020000 \"EXT\"\n\
mtd9: 0d812000 0001f800 \"ubi_Brio\"\n";

/// The files that change with a production cartridge in: cart_id and the
/// scratch register's bits 10-13 are the type lightning-boot read (7), and
/// the NAND driver names it (lf1000.c `show_cart`, gpio_main.c
/// `show_cart_id`). Read from the kernel source, not checked on Original.
pub fn cart_in(vfs: &mut Vfs) {
    let files = [
        ("lf1000-gpio/cart_id", "7
"),
        ("lf1000-nand/cartridge", "production
"),
        ("lf1000-alvgpio/scratchpad", "00001C60
"),
    ];
    for (path, text) in files {
        vfs.add_file(&format!("{PLATFORM}/{path}"), text.as_bytes().to_vec());
    }
}

/// Puts the kernel's /sys and /proc files into `vfs`.
pub fn add(vfs: &mut Vfs) {
    for (path, text) in PLATFORM_FILES {
        vfs.add_file(&format!("{PLATFORM}/{path}"), text.as_bytes().to_vec());
    }
    let proc: &[(&str, &str)] = &[
        ("/proc/sys/kernel/version", "#9 Wed Apr 22 15:32:34 PDT 2009\n"),
        ("/proc/meminfo", MEMINFO),
        ("/proc/cpuinfo", CPUINFO),
        ("/proc/mtd", MTD),
        ("/proc/cmdline", concat!(
            "mem=18M init=/sbin/init console=ttyS0,115200 root=31:04 ro rootflags=noatime rootfstype=jffs2 ",
            "ubi.mtd=Brio ubi.mtd=prg_Brio ubi.mtd=Cartridge ubi.mtd=EXT ubi.mtd=prg_EXT \n",
        )),
    ];
    for (path, text) in proc {
        vfs.add_file(path, text.as_bytes().to_vec());
    }
}
