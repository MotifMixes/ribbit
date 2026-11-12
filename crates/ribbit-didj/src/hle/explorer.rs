//! Leapster Explorer Enhanced (new): the Explorer's programs on the HLE.
//! The Explorer is the Didj's LF1000 with a newer kernel (2.6.31) and
//! userland (uClibc 0.9.32, Brio, AppManager), so the Didj's HLE runs it;
//! this module holds what differs: the file systems (a UBIFS root and
//! Bulk built from the packages), the /sys and /proc files its programs
//! read, and how it starts. Plan: docs/explorer-enhanced-hle.md.

use super::vfs::{Data, S_IFDIR, S_IFLNK, S_IFMT, S_IFREG, Vfs};

/// The HLE machine's SDRAM: the real Explorer's 64 MB. It holds the 18 MB
/// frame buffer and the programs' pages, which are given as they are touched
/// (demand paging) and shared between processes for the libraries' code and
/// read-only data (`Space::map_shared`), as the kernel does.
pub const SDRAM: usize = 64 << 20;

/// The frame buffer (the command line's `mlc_fb=0x82E00000,0x01200000`):
/// 18 MB shared by /dev/fb0..2, here at the Didj's FB_BASE_ADDR with the
/// HLE's SDRAM at 0.
pub(super) const FB_BASE: u32 = 0x0120_0000;
pub(super) const FB_SIZE: u32 = 0x0120_0000;

const PLATFORM: &str = "/sys/devices/platform";

/// (file under /sys/devices/platform, contents). VERIFIED: read on Original
/// (the Explorer's kernel in Ribbit, first-time setup, no cartridge, on
/// mains power) with `grep -r .` over the serial console, 2026-11-12.
const PLATFORM_FILES: &[(&str, &str)] = &[
    ("didj-asoc/force_audio", "0\n"),
    ("didj-asoc/force_mixer", "0\n"),
    ("lf1000-adc/channel0", "512\n"),
    ("lf1000-adc/channel1", "512\n"),
    ("lf1000-adc/channel2", "760\n"),
    ("lf1000-adc/channel3", "0\n"),
    ("lf1000-adc/channel4", "1008\n"),
    ("lf1000-adc/channel5", "0\n"),
    ("lf1000-adc/channel6", "1008\n"),
    ("lf1000-adc/channel7", "0\n"),
    ("lf1000-alvgpio/scratchpad", "00040241\n"),
    ("lf1000-clock/cpu_freq_in_hz", "393218109\n"),
    ("lf1000-clock/pll0", "23,801,0\n"),
    ("lf1000-clock/pll1", "3,64,2\n"),
    ("lf1000-dpc/tvout", "0\n"),
    ("lf1000-dpc/xres", "320\n"),
    ("lf1000-dpc/yres", "240\n"),
    ("lf1000-gpio/board_id", "2\n"),
    ("lf1000-gpio/boot_image", "PLAY\n"),
    ("lf1000-gpio/boot_source", "NAND\n"),
    ("lf1000-gpio/panic", "0\n"),
    ("lf1000-gpio/power", "COLD\n"),
    ("lf1000-gpio/request", "PLAY\n"),
    ("lf1000-gpio/shutdown", "CLEAN\n"),
    ("lf1000-gpio/touchscreen", "YES\n"),
    ("lf1000-gpio/user_0", "0\n"),
    ("lf1000-nand/cart_hotswap", "0\t0\n"),
    ("lf1000-nand/ramsize", "8MB\n"),
    ("lf1000-power/adc_constant", "0\n"),
    ("lf1000-power/adc_slope_256", "2012\n"),
    ("lf1000-power/critical_battery_mv", "2000\n"),
    ("lf1000-power/external", "1\n"),
    ("lf1000-power/low_battery_mv", "4200\n"),
    ("lf1000-power/low_battery_repeat_mv", "100\n"),
    ("lf1000-power/low_battery_reported_voltage", "4200\n"),
    ("lf1000-power/max_battery_mv", "8000\n"),
    ("lf1000-power/normal_battery_mv", "4600\n"),
    ("lf1000-power/power_source", "NIMH_EXTERNAL\n"),
    ("lf1000-power/shutdown", "0\n"),
    ("lf1000-power/status", "1\n"),
    ("lf1000-power/voltage", "5973\n"),
    ("lf1000-touchscreen/a0", "25786444\n"),
    ("lf1000-touchscreen/a1", "-29676\n"),
    ("lf1000-touchscreen/a2", "-170\n"),
    ("lf1000-touchscreen/a3", "21441222\n"),
    ("lf1000-touchscreen/a4", "-632\n"),
    ("lf1000-touchscreen/a5", "-26927\n"),
    ("lf1000-touchscreen/a6", "65536\n"),
    ("lf1000-touchscreen/pointercal", "-29676 -170 25786444 -632 -26927 21441222 65536\n"),
    ("lf1000-touchscreen/pressure_curve", "303 405 428 505 603 632 651 670 733\n"),
    ("lf1000-touchscreen/report_events", "1\n"),
    ("lf1000-touchscreen/sample_rate_in_hz", "50\n"),
    ("lf1000-touchscreen/max_tnt_down", "28\n"),
    ("lf1000-touchscreen/min_tnt_up", "500\n"),
    ("lf1000-touchscreen/max_delta_tnt", "5\n"),
    ("lf1000-touchscreen/debounce_in_samples_down", "1\n"),
    ("lf1000-touchscreen/debounce_in_samples_up", "1\n"),
    ("lf1000-touchscreen/averaging", "-1\n"),
    ("lf1000-touchscreen/delay_in_us", "10\n"),
    ("lf1000-touchscreen/y_delay_in_us", "10\n"),
    ("lf1000-touchscreen/tnt_delay_in_us", "18\n"),
    ("lf1000-touchscreen/tnt_mode", "0\n"),
    ("lf1000-touchscreen/touch_button", "0\n"),
    ("lf1000-usbgadget/vbus", "0\n"),
    ("lf1000-usbgadget/gadget/disconnect", "1\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/ro", "0\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/file", "/LF/fuse/dftp\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/locked", "0\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/enabled", "1\n"),
    ("lf1000-usbgadget/gadget/gadget-lun0/needs_repair", "0\n"),
];

/// /proc files, as Original shows them (2026-11-12). /proc/mounts lists
/// /LF/Bulk already mounted, so /etc/init.d/mounts leaves it (the HLE has
/// no UBI to attach).
const PROC_FILES: &[(&str, &str)] = &[
    (
        "/proc/cmdline",
        "init=/sbin/init console=ttyS0,115200 mem=46M mlc_fb=0x82E00000,0x01200000 ram=0x80000000-0x83FFFFFF \
         screen_module=ILI9322 ebs=0x20000,0x0 root=ubi0_0 rw rootfstype=ubifs ubi.mtd=RFS\n",
    ),
    (
        "/proc/mtd",
        "dev:    size   erasesize  name\nmtd0: 00100000 00020000 \"Emerald_Boot\"\nmtd1: 00100000 00020000 \"I18n_Screens\"\n\
         mtd2: 00800000 00020000 \"Kernel\"\nmtd3: 05600000 00020000 \"RFS\"\nmtd4: 1a000000 00020000 \"Bulk\"\n\
         mtd5: 0007e000 00001000 \"NOR_Boot\"\nmtd6: 00001000 00001000 \"MfgData0\"\nmtd7: 00001000 00001000 \"MfgData1\"\n\
         mtd8: 0536d000 0001f800 \"ubi_rfs\"\nmtd9: 19512000 0001f800 \"ubi_bulk\"\n",
    ),
    (
        "/proc/version",
        "Linux version 2.6.31-leapfrog (emerald@lf2000-release) (gcc version 4.5.4 20110722 (prerelease) (GCC) ) \
         #1 Tue Sep 25 13:39:13 PDT 2012\n",
    ),
    ("/proc/sys/kernel/version", "#1 Tue Sep 25 13:39:13 PDT 2012\n"),
    ("/proc/sys/kernel/core_pattern", "core\n"),
    (
        "/proc/iomem",
        "00000000-0007ffff : lf1000-nor
2c000000-2c000017 : lf1000-nand
  2c000000-2c000017 : lf1000-nand
         80000000-82dfffff : System RAM
  8001f000-80405fff : Kernel text
  8041a000-8048028b : Kernel data
         82e00000-84000000 : lf1000-fb
  82e00000-83ffffff : lf1000-fb
",
    ),
    ("/proc/asound/card0/id", "DidjCS43L22\n"),
    ("/proc/asound/cards", " 0 [DidjCS43L22    ]: CS43L22 - Didj-CS43L22\n                      Didj-CS43L22 (CS43L22)\n"),
    (
        "/proc/mounts",
        "rootfs / rootfs rw 0 0\nubi0_0 / ubifs rw,relatime 0 0\nmdev /dev tmpfs rw,relatime 0 0\nnone /proc proc rw,relatime 0 0\n\
         sysfs /sys sysfs rw,relatime 0 0\n/dev/ram0 /tmp tmpfs rw,relatime,mode=777 0 0\n\
         /dev/ram0 /var/volatile/run tmpfs rw,relatime,mode=755 0 0\nnone /dev/pts devpts rw,relatime,mode=600 0 0\n\
         ubi1:ubi_bulk /LF/Bulk ubifs rw,relatime 0 0\n",
    ),
    (
        "/proc/meminfo",
        "MemTotal:          42136 kB\nMemFree:           19120 kB\nBuffers:               0 kB\nCached:             9596 kB\n\
         SwapCached:            0 kB\nActive:             8460 kB\nInactive:           7556 kB\nSwapTotal:             0 kB\n\
         SwapFree:              0 kB\nDirty:                20 kB\nWriteback:             0 kB\nAnonPages:          6432 kB\n\
         Mapped:             5356 kB\nSlab:               3720 kB\nCommitLimit:       21068 kB\nCommitted_AS:      18624 kB\n\
         VmallocTotal:     344064 kB\nVmallocUsed:       21608 kB\nVmallocChunk:     309244 kB\n",
    ),
    (
        "/proc/bus/input/devices",
        "I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"Power Button\"\nP: Phys=lf1000/power_button\n\
         S: Sysfs=/class/input/input0\nU: Uniq=\nH: Handlers=event0 \nB: EV=3\nB: KEY=1000 0 0 0 300180 0 0 3000\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF1000 Keyboard\"\nP: Phys=lf1000/input0\n\
         S: Sysfs=/class/input/input1\nU: Uniq=\nH: Handlers=event1 \nB: EV=23\n\
         B: KEY=4000 0 0 0 0 0 0 0 0 c1680 0 52048 42080002\nB: SW=4\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF1000 touchscreen interface\"\n\
         P: Phys=lf1000/touchscreen\nS: Sysfs=/class/input/input3\nU: Uniq=\nH: Handlers=event2 \nB: EV=b\n\
         B: KEY=400 0 0 0 0 0 0 0 0 0 0\nB: ABS=1000003\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF1000 touchscreen raw\"\n\
         P: Phys=lf1000/touchscreen-raw\nS: Sysfs=/class/input/input4\nU: Uniq=\nH: Handlers=event3 \nB: EV=9\nB: ABS=7ff\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF1000 USB\"\nP: Phys=lf1000/usb\n\
         S: Sysfs=/class/input/input5\nU: Uniq=\nH: Handlers=event4 \nB: EV=21\nB: SW=1\n\n",
    ),
];

/// The Explorer's /dev as Original has it (`ls -l /dev`, 2026-11-12):
/// (name, major, minor). rcS also unpacks /var/dev_nodes.tar over it.
const DEV_NODES: &[(&str, u32, u32)] = &[
    ("console", 5, 1),
    ("fb0", 29, 0),
    ("fb1", 29, 1),
    ("fb2", 29, 2),
    ("full", 1, 7),
    ("ga3d", 249, 0),
    ("gpio", 246, 0),
    ("i2c-0", 89, 0),
    ("i2c-1", 89, 1),
    ("idct", 248, 0),
    ("mem", 1, 1),
    ("null", 1, 3),
    ("random", 1, 8),
    ("urandom", 1, 9),
    ("zero", 1, 5),
    ("rtc0", 254, 0),
    ("tty", 5, 0),
    ("ttyS0", 4, 64),
    ("ubi_ctrl", 10, 63),
    ("watchdog", 10, 130),
    ("input/event0", 13, 64),
    ("input/event1", 13, 65),
    ("input/event2", 13, 66),
    ("input/event3", 13, 67),
    ("input/event4", 13, 68),
    ("snd/controlC0", 116, 0),
    ("snd/pcmC0D0p", 116, 16),
    ("snd/timer", 116, 33),
];

/// The NOR's partitions (/proc/mtd): NOR_Boot, MfgData0, MfgData1, as
/// /dev/mtd5..7. They are files here (reads and seeks as on the MTD
/// character device; the MTD ioctls aren't there).
const NOR_PARTS: [(u32, usize, usize); 3] = [(5, 0, 0x7E000), (6, 0x7E000, 0x7F000), (7, 0x7F000, 0x80000)];

/// The Explorer's file systems out of a folder of its firmware packages:
/// the root file system (erootfs.ubi in Firmware Base), /LF/Bulk with the
/// `.lf2` packages installed as lfpkg would (`explorer::install_lf2`), /dev,
/// /sys and /proc as the kernel shows them, and an empty /tmp.
pub fn system_from_packages(dir: &std::path::Path) -> Result<Vfs, String> {
    let mut rootfs = None;
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lfp"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !crate::explorer::for_explorer(&data) {
            continue;
        }
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{}: {e}", path.display()))? {
            let file = entry.name.rsplit('/').next().unwrap_or("");
            if file.starts_with("3,") && file.ends_with(".ubi") {
                rootfs = Some(ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{}: {e}", path.display()))?);
            }
        }
    }
    let rootfs = rootfs.ok_or_else(|| format!("no Leapster Explorer firmware (Firmware Base) in {}", dir.display()))?;
    let mut vfs = Vfs::new();
    add_files(&mut vfs, "", crate::ubifs_read::files_in_ubi(&rootfs).map_err(|e| format!("erootfs.ubi: {e}"))?);
    let mut bulk = crate::ubifs::Tree::new();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lf2"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        crate::explorer::install_lf2(&mut bulk, &data).map_err(|e| format!("{}: {e}", path.display()))?;
    }
    add_tree(&mut vfs, "/LF/Bulk", &bulk);
    finish(&mut vfs, None);
    Ok(vfs)
}

/// Puts a UBIFS volume's files into `vfs` under `at` ("" for the root).
fn add_files(vfs: &mut Vfs, at: &str, files: Vec<crate::ubifs_read::File>) {
    for f in files {
        if f.path == "/" {
            continue;
        }
        let path = format!("{at}{}", f.path);
        let data = match f.mode & S_IFMT {
            S_IFDIR => Data::Dir,
            S_IFREG => Data::File(f.data),
            S_IFLNK => Data::Link(String::from_utf8_lossy(&f.data).into_owned()),
            _ => Data::Dev(f.data.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()))),
        };
        vfs.insert(&path, f.mode, data);
        if let Some(n) = vfs.get_mut(&path) {
            n.mtime = f.mtime;
        }
    }
}

/// What the kernel and boot add around the file systems: /dev, /tmp,
/// /sys, /proc, the NOR's partitions.
fn finish(vfs: &mut Vfs, nor: Option<&[u8]>) {
    for d in ["/tmp", "/dev/pts", "/var/volatile/run", "/var/volatile/log", "/LF/Cart", "/LF/fuse"] {
        vfs.mkdir_all(d);
    }
    for (name, major, minor) in DEV_NODES {
        let path = format!("/dev/{name}");
        vfs.mkdir_all(super::vfs::split(&path).0);
        vfs.insert(&path, super::vfs::S_IFCHR | 0o666, Data::Dev(major << 8 | minor));
    }
    vfs.insert("/dev/input/touchscreen0", S_IFLNK | 0o777, Data::Link("/dev/input/event2".into()));
    match nor {
        Some(n) => set_nor(vfs, n),
        None => set_nor(vfs, &crate::nor::image(crate::explorer::SERIAL).data),
    }
    add_sysfs(vfs);
}

/// Emerald Boot's splash (FIRST.32.rle, Firmware Base's I18n_Screens
/// image) out of a folder of Explorer packages.
pub fn splash_from_packages(dir: &std::path::Path) -> Option<Vec<u8>> {
    let mut entries: Vec<_> = std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lfp"))) {
        let data = std::fs::read(path).ok()?;
        if !crate::explorer::for_explorer(&data) {
            continue;
        }
        for entry in ribbit_core::zip::entries(&data).ok()? {
            if entry.name.rsplit('/').next().is_some_and(|f| f.starts_with("1,")) {
                return ribbit_core::zip::read_entry(&data, &entry).ok();
            }
        }
    }
    None
}

/// Puts the NOR flash's contents (512 KB, as the app keeps it in .nor)
/// into /dev/mtd5..7.
pub fn set_nor(vfs: &mut Vfs, nor: &[u8]) {
    for (n, from, to) in NOR_PARTS {
        for name in [format!("/dev/mtd{n}"), format!("/dev/mtd{n}ro")] {
            vfs.insert(&name, S_IFREG | 0o666, Data::File(nor.get(from..to).unwrap_or(&[]).to_vec()));
        }
    }
}

/// The NOR flash's contents back out of /dev/mtd5..7.
pub fn nor(vfs: &Vfs) -> Vec<u8> {
    let mut out = vec![0xFF; crate::nor::SIZE];
    for (n, from, to) in NOR_PARTS {
        if let Some(Data::File(d)) = vfs.get(&format!("/dev/mtd{n}")).map(|n| &n.data) {
            let len = d.len().min(to - from);
            out[from..from + len].copy_from_slice(&d[..len]);
        }
    }
    out
}

/// Puts a UBIFS tree's files (paths relative to `at`) into `vfs`.
pub fn add_tree(vfs: &mut Vfs, at: &str, tree: &crate::ubifs::Tree) {
    use crate::ubifs::Kind;
    vfs.mkdir_all(at);
    for (path, e) in tree {
        let full = format!("{at}/{}", path.trim_matches('/'));
        vfs.mkdir_all(super::vfs::split(&full).0);
        let (mode, data) = match &e.kind {
            Kind::Dir => (S_IFDIR, Data::Dir),
            Kind::File(d) => (S_IFREG, Data::File(d.clone())),
            Kind::Symlink(t) => (S_IFLNK, Data::Link(t.clone())),
            Kind::Special(t, d) => (*t, Data::Dev(d.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap())))),
        };
        if matches!(data, Data::Dir) && vfs.get(&full).is_some_and(|n| n.is_dir()) {
            continue;
        }
        vfs.insert(&full, mode | (e.mode & 0o7777), data);
        if let Some(n) = vfs.get_mut(&full) {
            n.mtime = e.mtime as u32;
        }
    }
}

/// The kernel's /sys and /proc files for the Explorer.
pub fn add_sysfs(vfs: &mut Vfs) {
    for (path, text) in PLATFORM_FILES {
        vfs.add_file(&format!("{PLATFORM}/{path}"), text.as_bytes().to_vec());
    }
    for (path, text) in PROC_FILES {
        vfs.add_file(path, text.as_bytes().to_vec());
    }
    vfs.add_file("/proc/cpuinfo", super::sysfs::CPUINFO.as_bytes().to_vec());
    // The MTDs by name (CartManager looks for "Cartridge" here).
    for (n, name) in ["Emerald_Boot", "I18n_Screens", "Kernel", "RFS", "Bulk", "NOR_Boot", "MfgData0", "MfgData1", "ubi_rfs", "ubi_bulk"].iter().enumerate() {
        vfs.add_file(&format!("/sys/class/mtd/mtd{n}/name"), format!("{name}\n").into_bytes());
    }
    // libDisplay looks for it to use /dev/fb* (else the Didj's /dev/dpc).
    for n in 0..3 {
        vfs.add_file(&format!("/sys/class/graphics/fb{n}/dev"), format!("29:{n}\n").into_bytes());
    }
    // rcS makes /dev itself (make_dev_nodes.sh) instead of unpacking
    // Original's archive of it: the HLE's /dev is its own.
    vfs.add_file("/var/dev_nodes_doom", Vec::new());
    // Bulk is there at once (rcS's /etc/init.d/mounts writes this after
    // mounting it).
    vfs.add_file("/tmp/bulk_ready", b"1\n".to_vec());
}

// ---- the Explorer's devices -------------------------------------------

use super::dev::Device;
use super::task::Wait;
use super::{EAGAIN, EFAULT, EINVAL, ENOTTY, Process};
use std::collections::{BTreeMap, VecDeque};

const ENODEV: i32 = 19;

/// /dev/input/event0..4 (`cat /proc/bus/input/devices` on Original).
pub const INPUT_NAMES: [&str; 5] = ["Power Button", "LF1000 Keyboard", "LF1000 touchscreen interface", "LF1000 touchscreen raw", "LF1000 USB"];
const KEYPAD: usize = 1;
/// Device numbers Ribbit gives open input files: device in bits 12-14,
/// the file's number below.
const INPUT: u32 = 0xFFFD_0000;
/// evdev's client buffer (events).
const EVDEV_BUFFER: usize = 256;
const TOUCH: usize = 2;

/// The Explorer's device state (the Didj's devices that are the same,
/// /dev/ga3d and /dev/mem, stay in `Devices`).
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct Explorer {
    /// Running the Explorer's programs.
    pub on: bool,
    /// Keys down per input device, for EVIOCGKEY (codes below 512).
    keys: Vec<Vec<u8>>,
    /// The stylus: where it is down (screen pixels), as the driver last
    /// reported it.
    touch: Option<(i32, i32)>,
    /// The last absolute values reported (x, y, pressure): the input core
    /// drops repeats.
    abs: [i32; 3],
    /// sethostname's name.
    hostname: String,
    /// AF_UNIX sockets (xsocket.rs).
    pub sockets: super::xsocket::Sockets,
    /// /dev/fb0..2 (xfb.rs).
    pub fb: super::xfb::Fb,
    /// ALSA (xsnd.rs).
    pub snd: super::xsnd::Sound,
    /// Open input files: (device 0-4, its events). Every open file of an
    /// input device gets each event, as evdev's clients do (lfd and
    /// AppManager both read the keypad).
    pub inputs: std::collections::HashMap<u32, (usize, VecDeque<[u8; 16]>)>,
    pub next_input: u32,
    /// The cartridge in the slot: its volume's files (`explorer_cart`).
    pub cart: Option<Vec<(String, Vec<u8>)>>,
    /// /dev/i2c-0: the device addressed (I2C_SLAVE) and the codec's
    /// register address.
    pub i2c_addr: u32,
    pub i2c_map: u8,
}

impl Process {
    /// Opening a device node on the Explorer: each open of the PCM gets
    /// its own stream.
    pub(super) fn x_open(&mut self, rdev: u32) -> Result<Device, i32> {
        if rdev == 116 << 8 | 16 {
            return Ok(Device::Other(self.pcm_open()));
        }
        if let (13, m @ 64..=68) = (rdev >> 8, rdev & 0xFF) {
            let id = self.x.next_input & 0xFFF;
            self.x.next_input += 1;
            self.x.inputs.insert(id, (m as usize - 64, VecDeque::new()));
            return Ok(Device::Other(INPUT | (m - 64) << 12 | id));
        }
        open(rdev)
    }

    /// A file descriptor of a device closing.
    pub(super) fn x_close(&mut self, rdev: u32) {
        if rdev & 0xFFFF_0000 == super::xsnd::PCM {
            self.pcm_close(rdev);
        }
        if rdev & 0xFFFF_0000 == INPUT {
            self.x.inputs.remove(&(rdev & 0xFFF));
        }
    }

    /// Queues events for every open file of input device `n` (at most
    /// EVDEV_BUFFER each, the oldest dropped).
    fn x_events(&mut self, n: usize, events: &[[u8; 16]]) {
        for (dev, q) in self.x.inputs.values_mut() {
            if *dev == n {
                q.extend(events.iter().copied());
                while q.len() > EVDEV_BUFFER {
                    q.pop_front();
                }
            }
        }
    }
}

/// The device a character node opens on the Explorer.
pub(super) fn open(rdev: u32) -> Result<Device, i32> {
    match (rdev >> 8, rdev & 0xFF) {
        (10, 63) | (4, _) | (5, 0..=1) | (13, 64..=68) | (29, 0..=2) | (116, _) | (89, 0..=1) | (90, 10..=15) | (254, 0) | (246, 0) | (248, 0) => Ok(Device::Other(rdev)),
        (13, _) => Err(ENODEV),
        // The Didj's display nodes aren't there.
        (250..=253, _) => Ok(Device::Other(rdev)),
        _ => Device::open(rdev),
    }
}

fn event(sec: u32, nsec: u32, kind: u16, code: u16, value: i32) -> [u8; 16] {
    let mut e = [0u8; 16];
    e[0..4].copy_from_slice(&sec.to_le_bytes());
    e[4..8].copy_from_slice(&(nsec / 1000).to_le_bytes());
    e[8..10].copy_from_slice(&kind.to_le_bytes());
    e[10..12].copy_from_slice(&code.to_le_bytes());
    e[12..16].copy_from_slice(&value.to_le_bytes());
    e
}

impl Process {
    /// A process for the Leapster Explorer's programs.
    pub fn new_explorer(vfs: Vfs) -> Self {
        let mut p = Self::with_memory(vfs, SDRAM, Some([(0x0010_0000, FB_BASE), (FB_BASE + FB_SIZE, SDRAM as u32)]));
        p.x.on = true;
        p.x.keys = vec![vec![0; 64]; 5];
        // The MLC as Emerald Boot leaves it showing its splash (as
        // explorer::show_splash; GUESS: its own register values are
        // UNKNOWN), then as the kernel's frame buffer driver sets it up.
        for off in (0..=0xA4).step_by(4) {
            p.m.bus.preset(crate::mlc::BASE + off, 0);
        }
        for (off, v) in [(0x00, 0x2), (0x04, 239 << 16 | 319), (0x0C, 319), (0x10, 239), (0x24, 0x0653_0020), (0x28, 4), (0x2C, 320 * 4), (0x38, FB_BASE)] {
            p.m.bus.preset(crate::mlc::BASE + off, v);
        }
        p.fb_init();
        for (reg, v) in super::xsnd::CODEC_BOOT {
            p.m.bus.i2c.set_codec(reg, v);
        }
        p
    }

    /// Bytes of user pages free in the HLE machine's SDRAM.
    pub fn free_memory(&self) -> usize {
        self.frames.free_pages() * 4096
    }

    /// Draws Emerald Boot's splash (FIRST.32.rle: 32-bit words, run length
    /// in bits 24-31, the colour below) into the frame buffer, where layer 0
    /// shows it until rcS draws LEGAL.png.
    pub fn explorer_splash(&mut self, rle: &[u8]) {
        let mut pixels = Vec::with_capacity(320 * 240);
        for w in rle.as_chunks::<4>().0.iter().map(|b| u32::from_le_bytes(*b)) {
            let (n, c) = ((w >> 24) as usize, w & 0xFF_FFFF);
            if n == 0 || pixels.len() + n > 320 * 240 {
                break;
            }
            pixels.extend(std::iter::repeat_n(0xFF00_0000 | c, n));
        }
        let at = FB_BASE as usize;
        for (i, px) in pixels.iter().enumerate() {
            self.m.bus.sdram[at + 4 * i..][..4].copy_from_slice(&px.to_le_bytes());
        }
    }

    /// A key going down or up on the keypad (/dev/input/event1).
    pub(super) fn x_key(&mut self, code: u16, down: bool) {
        let bits = &mut self.x.keys[KEYPAD][code as usize / 8];
        let bit = 1 << (code % 8);
        if (*bits & bit != 0) == down {
            return;
        }
        *bits ^= bit;
        let (sec, nsec) = self.now(0);
        self.x_events(KEYPAD, &[event(sec, nsec, 1, code, down as i32), event(sec, nsec, 0, 0, 0)]);
    }

    /// The stylus down at (x, y) on the screen, or lifted: what
    /// lf1000_ts2 reports on /dev/input/event2 (BTN_TOUCH on going down and
    /// up, then ABS_X, ABS_Y and ABS_PRESSURE; the input core passes on
    /// only values that changed, and a SYN_REPORT only after one did).
    pub fn stylus(&mut self, at: Option<(i32, i32)>) {
        const BTN_TOUCH: u16 = 330;
        /// GUESS: the driver's pressure for a firm touch (its curve gives
        /// 10-100).
        const PRESSURE: i32 = 100;
        let (sec, nsec) = self.now(0);
        let mut out = Vec::new();
        let was = self.x.touch;
        if was.is_some() != at.is_some() {
            out.push(event(sec, nsec, 1, BTN_TOUCH, at.is_some() as i32));
            let bits = &mut self.x.keys[TOUCH][BTN_TOUCH as usize / 8];
            *bits ^= 1 << (BTN_TOUCH % 8);
        }
        let (x, y) = at.or(was).unwrap_or((0, 0));
        let values = [x, y, if at.is_some() { PRESSURE } else { 0 }];
        for (i, v) in values.into_iter().enumerate() {
            let code = if i == 2 { 24 } else { i as u16 };
            if self.x.abs[i] != v {
                self.x.abs[i] = v;
                out.push(event(sec, nsec, 3, code, v));
            }
        }
        if !out.is_empty() {
            out.push(event(sec, nsec, 0, 0, 0));
        }
        self.x.touch = at;
        self.x_events(TOUCH, &out);
    }

    pub(super) fn x_read(&mut self, rdev: u32, nonblock: bool, len: usize) -> Result<Vec<u8>, i32> {
        if rdev & 0xFFFF_0000 == super::xsocket::SOCK {
            return self.sock_read(rdev, nonblock, len);
        }
        if rdev & 0xFFFF_0000 == super::xsnd::PCM {
            return Err(EINVAL);
        }
        match (rdev >> 8, rdev & 0xFF) {
            _ if rdev & 0xFFFF_0000 == INPUT => {
                let Some((_, q)) = self.x.inputs.get_mut(&(rdev & 0xFFF)) else { return Err(EINVAL) };
                if q.is_empty() {
                    if nonblock {
                        return Err(EAGAIN);
                    }
                    self.block = Some(Wait::Retry(self.clock() + std::time::Duration::from_millis(2)));
                    return Ok(Vec::new());
                }
                let mut out = Vec::new();
                while out.len() + 16 <= len
                    && let Some(e) = q.pop_front()
                {
                    out.extend(e);
                }
                if out.is_empty() { Err(EINVAL) } else { Ok(out) }
            }
            (89, 0) => self.i2c_read(len),
            // The serial console and ttys: no one types.
            (4, _) | (5, 0..=1) => {
                if nonblock {
                    return Err(EAGAIN);
                }
                self.block = Some(Wait::Retry(self.clock() + std::time::Duration::from_secs(3600)));
                Ok(Vec::new())
            }
            _ => Ok(Vec::new()),
        }
    }

    pub(super) fn x_write(&mut self, rdev: u32, _nonblock: bool, data: &[u8]) -> Result<usize, i32> {
        if rdev & 0xFFFF_0000 == super::xsocket::SOCK {
            return self.sock_write(rdev, data);
        }
        if rdev & 0xFFFF_0000 == super::xsnd::PCM {
            return self.pcm_write(rdev, _nonblock, data);
        }
        if rdev == 89 << 8 {
            return self.i2c_write_fd(data);
        }
        if matches!((rdev >> 8, rdev & 0xFF), (4, _) | (5, 0..=1)) {
            self.console.extend_from_slice(data);
            return Ok(data.len());
        }
        self.log_once(format!("write to device {}:{} dropped", rdev >> 8, rdev & 0xFF));
        Ok(data.len())
    }

    pub(super) fn x_ready(&self, rdev: u32) -> (bool, bool, bool) {
        if rdev & 0xFFFF_0000 == super::xsocket::SOCK {
            return self.sock_ready(rdev);
        }
        if rdev & 0xFFFF_0000 == super::xsnd::PCM {
            return self.pcm_ready(rdev);
        }
        match (rdev >> 8, rdev & 0xFF) {
            _ if rdev & 0xFFFF_0000 == INPUT => (self.x.inputs.get(&(rdev & 0xFFF)).is_some_and(|(_, q)| !q.is_empty()), true, false),
            _ => (true, true, false),
        }
    }

    pub(super) fn x_mmap(&mut self, rdev: u32, at: u32, len: u32, pgoff: u32) -> Result<u32, i32> {
        if rdev >> 8 == 29 {
            return self.fb_mmap((rdev & 0xFF) as usize, at, len, pgoff);
        }
        self.log_once(format!("mmap of device {}:{} not handled", rdev >> 8, rdev & 0xFF));
        Err(ENODEV)
    }

    fn x_put(&mut self, at: u32, bytes: &[u8]) -> Result<u32, i32> {
        if self.mem().write_bytes(at, bytes) { Ok(0) } else { Err(EFAULT) }
    }

    pub(super) fn x_ioctl(&mut self, rdev: u32, cmd: u32, arg: u32) -> Result<u32, i32> {
        if rdev & 0xFFFF_0000 == super::xsnd::PCM {
            return self.pcm_ioctl(rdev, cmd, arg);
        }
        match (rdev >> 8, rdev & 0xFF) {
            (116, 0) => match self.ctl_elem_ioctl(cmd, arg) {
                Some(r) => r,
                None => self.ctl_ioctl(cmd, arg),
            },
            (89, 0) => self.i2c_ioctl(cmd, arg),
            (10, 63) => self.ubi_ctrl_ioctl(cmd, arg),
            _ if rdev & 0xFFFF_0000 == INPUT => self.x_evdev_ioctl(((rdev >> 12) & 7) as usize, cmd, arg),
            (29, n) => self.fb_ioctl(n as usize, cmd, arg),
            _ => {
                if self.logged.insert(format!("ioctl {rdev:X} {cmd:08X}")) {
                    eprintln!("[hle] ioctl {cmd:08X} on device {}:{} not handled", rdev >> 8, rdev & 0xFF);
                }
                Err(ENOTTY)
            }
        }
    }

    fn x_evdev_ioctl(&mut self, n: usize, cmd: u32, arg: u32) -> Result<u32, i32> {
        if (cmd >> 8) & 0xFF != b'E' as u32 {
            return Err(ENOTTY);
        }
        let size = ((cmd >> 16) & 0x3FFF) as usize;
        // The kernel copies min(len, the bitmap's length) and returns that
        // (evdev.c bits_to_user): the caller's buffer may be smaller than len.
        let fill = |bits: &[u8], kind: u32| {
            let max = match kind {
                1 => 96, // KEY_MAX 0x2ff
                3 => 8,  // ABS_MAX 0x3f
                0x15 => 16,
                _ => 4,
            };
            let mut v = bits.to_vec();
            v.resize(max, 0);
            v.truncate(size.min(max));
            v
        };
        match cmd & 0xFF {
            0x01 => self.x_put(arg, &0x1_0000u32.to_le_bytes()), // EVIOCGVERSION
            0x02 => self.x_put(arg, &[0x19, 0, 1, 0, 1, 0, 1, 0]), // EVIOCGID: BUS_HOST 1 1 1
            0x06 => {
                let mut name = INPUT_NAMES[n].as_bytes().to_vec();
                name.push(0);
                name.truncate(size);
                self.x_put(arg, &name)?;
                Ok(name.len() as u32)
            }
            0x18 => {
                let v = fill(&self.x.keys[n], 1);
                self.x_put(arg, &v).map(|_| v.len() as u32)
            }
            // EVIOCGLED, EVIOCGSND, EVIOCGSW: nothing on.
            0x19..=0x1B => {
                let v = fill(&[], 0);
                self.x_put(arg, &v).map(|_| v.len() as u32)
            }
            0x20..=0x3F => {
                // EVIOCGBIT(type, len), from /proc/bus/input/devices.
                let words: &[u32] = match (n, cmd & 0x1F) {
                    (0, 0) => &[0x3],
                    (0, 1) => &[0x3000, 0, 0, 0x30_0180, 0, 0, 0, 0x1000],
                    (1, 0) => &[0x23],
                    (1, 1) => &[0x4208_0002, 0x5_2048, 0, 0xC_1680, 0, 0, 0, 0, 0, 0, 0, 0, 0x4000],
                    (1, 5) => &[0x4],
                    (2, 0) => &[0xB],
                    (2, 1) => &[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x400],
                    (2, 3) => &[0x100_0003],
                    (3, 0) => &[0x9],
                    (3, 3) => &[0x7FF],
                    (4, 0) => &[0x21],
                    (4, 5) => &[0x1],
                    _ => &[],
                };
                let bytes: Vec<u8> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
                let kind = if cmd & 0x1F == 0 { 0 } else { cmd & 0x1F };
                let v = fill(&bytes, kind);
                self.x_put(arg, &v).map(|_| v.len() as u32)
            }
            0x40..=0x7F => {
                // EVIOCGABS(axis): value, min, max, fuzz, flat. GUESS: the
                // screen's size for x and y, 0-100 for pressure.
                let (v, max) = match cmd & 0x3F {
                    0 => (self.x.abs[0], 319),
                    1 => (self.x.abs[1], 239),
                    24 => (self.x.abs[2], 100),
                    _ => (0, 65535),
                };
                let bytes: Vec<u8> = [v, 0, max, 0, 0].iter().flat_map(|w| w.to_le_bytes()).collect();
                self.x_put(arg, &bytes[..size.min(20)])
            }
            0x90 => Ok(0), // EVIOCGRAB
            _ => {
                if self.logged.insert(format!("x evdev {cmd:08X}")) {
                    eprintln!("[hle] input ioctl {cmd:08X} not handled");
                }
                Err(EINVAL)
            }
        }
    }
}

// ---- system calls the Explorer's programs make that the Didj's don't ----

impl Process {
    /// The Explorer's own system calls (2.6.31 numbers), or None for the
    /// Didj's handling.
    pub(super) fn x_syscall(&mut self, n: u32, a: [u32; 6]) -> Option<Result<u32, i32>> {
        Some(match n {
            14 => self.x_mknod(a[0], a[1], a[2]),
            281..=297 | 366 => self.x_socket_call(n, a),
            5 | 322 => {
                // /proc/uptime is made when read.
                let at = if n == 5 { a[0] } else { a[1] };
                if self.mem().cstr(at).as_deref() == Some("/proc/uptime") {
                    let t = self.since_start().as_secs_f64();
                    self.vfs.add_file("/proc/uptime", format!("{t:.2} {:.2}\n", t * 0.8).into_bytes());
                }
                return None;
            }
            // munmap with the kernel's checks: an unaligned address, no
            // length or an end past user space is EINVAL (alsa-lib's PCM
            // close passes garbage when the status page wasn't mapped).
            91 => {
                let end = a[0] as u64 + a[1] as u64;
                if a[0] % 4096 != 0 || a[1] == 0 || end > 0xBF00_0000 {
                    return Some(Err(EINVAL));
                }
                return None;
            }
            // madvise: advice only.
            220 => Ok(0),
            192 => self.x_mmap2(a),
            // mprotect: pages about to be written stop being shared.
            125 => {
                if a[2] & 2 != 0 && !self.mem().privatize(a[0] & !0xFFF, a[1].wrapping_add(a[0] & 0xFFF)) {
                    return Some(Err(super::ENOMEM));
                }
                Ok(0)
            }
            // getpriority: nice 0 (the kernel returns 20 - nice).
            96 => Ok(20),
            // syslog: klogd reads the kernel's messages; none come.
            103 if a[0] == 2 => {
                self.block = Some(Wait::Retry(self.clock() + std::time::Duration::from_secs(3600)));
                Ok(0)
            }
            // setpriority, syslog's other actions, reboot's Ctrl-Alt-Del setting.
            97 | 103 => Ok(0),
            88 if a[2] == 0 || a[2] == 0x89AB_CDEF => Ok(0),
            // mount, umount2: the HLE's file systems are there already, but
            // for the cartridge's.
            21 => self.x_mount(a[1]),
            52 => self.x_umount(a[0]),
            // setsid: a new session led by this process.
            66 => Ok(self.g.tgid),
            74 => {
                let len = (a[1] as usize).min(64);
                let name = self.mem().read_bytes(a[0], len).ok_or(EFAULT);
                name.map(|n| {
                    self.x.hostname = String::from_utf8_lossy(&n).trim_end_matches('\0').to_string();
                    0
                })
            }
            // setrlimit, init_module, delete_module, set_robust_list,
            // ioprio_set: nothing to do.
            75 | 128 | 129 | 338 | 314 => Ok(0),
            122 => self.x_uname(a[0]),
            // sysinfo: uptime, no load, the memory /proc/meminfo shows,
            // the processes.
            116 => {
                let mut b = vec![0u8; 64];
                let put = |b: &mut Vec<u8>, at: usize, v: u32| b[at..at + 4].copy_from_slice(&v.to_le_bytes());
                put(&mut b, 0, self.since_start().as_secs() as u32);
                put(&mut b, 16, 42136 * 1024);
                put(&mut b, 20, 19120 * 1024);
                let procs = (self.groups.len() + 1) as u16;
                b[40..42].copy_from_slice(&procs.to_le_bytes());
                put(&mut b, 52, 1);
                if self.mem().write_bytes(a[0], &b) { Ok(0) } else { Err(EFAULT) }
            }
            // fstatat64 (dirfd, path, stat64, flags): from the current
            // directory or an absolute path; AT_SYMLINK_NOFOLLOW 0x100.
            327 => {
                const AT_FDCWD: u32 = -100i32 as u32;
                let path = self.mem().cstr(a[1]).unwrap_or_default();
                if a[0] != AT_FDCWD && !path.starts_with('/') {
                    // From a directory's descriptor (tar walking /dev).
                    let Ok(super::Fd::Dir { path: dir, .. }) = self.fd(a[0]) else { return Some(Err(super::EBADF)) };
                    let full = format!("{}/{path}", dir.trim_end_matches('/'));
                    let r = self.vfs.resolve("/", &full, a[3] & 0x100 == 0).and_then(|p| self.stat_path(&p, true));
                    return Some(r.and_then(|b| if self.mem().write_bytes(a[2], &b) { Ok(0) } else { Err(EFAULT) }));
                }
                self.stat(a[1], a[2], a[3] & 0x100 == 0, true)
            }
            _ => return None,
        })
    }

    /// mmap2 on the Explorer: anonymous memory is given pages as it is
    /// touched (demand paging), a file mapped without PROT_WRITE shares its
    /// pages with every other map of the same contents (each process maps
    /// the same libraries), devices as on the Didj.
    fn x_mmap2(&mut self, a: [u32; 6]) -> Result<u32, i32> {
        use super::mem::{PAGE, page_up};
        const MAP_FIXED: u32 = 0x10;
        const MAP_ANONYMOUS: u32 = 0x20;
        const MMAP_BASE: u32 = 0x4000_0000;
        const STACK_LOW: u32 = 0xBF00_0000 - (8 << 20);
        let (addr, len, prot, flags, fd, pgoff) = (a[0], a[1], a[2], a[3], a[4], a[5]);
        if len == 0 {
            return Err(EINVAL);
        }
        let len = page_up(len);
        let at = if flags & MAP_FIXED != 0 {
            if addr % PAGE != 0 {
                return Err(EINVAL);
            }
            self.mem().unmap(addr, len);
            addr
        } else if addr != 0 && addr % PAGE == 0 && self.mem().find_free(addr, len, addr.saturating_add(len)) == Some(addr) {
            addr
        } else {
            self.mem().find_free(MMAP_BASE, len, STACK_LOW).ok_or(super::ENOMEM)?
        };
        if flags & MAP_ANONYMOUS != 0 {
            self.mem().reserve(at, len);
            return Ok(at);
        }
        let file = match self.g.fds.get(fd as usize).and_then(|f| f.as_ref()) {
            Some(super::Fd::File { path, .. }) => path.clone(),
            Some(super::Fd::Dev { dev: Device::Other(rdev), .. }) => {
                let rdev = *rdev;
                return self.x_mmap(rdev, at, len, pgoff);
            }
            Some(super::Fd::Dev { dev: Device::Layer(_) | Device::Ga3d | Device::PhysMem, .. }) => {
                return if self.mem().map_phys(at, len, pgoff << 12) { Ok(at) } else { Err(super::ENOMEM) };
            }
            _ => return Err(super::EBADF),
        };
        let Some(Data::File(d)) = self.vfs.get(&file).map(|n| &n.data) else { return Err(super::EBADF) };
        let from = (pgoff as usize * PAGE as usize).min(d.len());
        let contents = d[from..(from + len as usize).min(d.len())].to_vec();
        if prot & 2 == 0 {
            for i in 0..len / PAGE {
                let lo = (i * PAGE) as usize;
                let part = contents.get(lo..(lo + PAGE as usize).min(contents.len())).unwrap_or(&[]);
                if !self.mem().map_shared(at + i * PAGE, part) {
                    self.mem().unmap(at, len);
                    return Err(super::ENOMEM);
                }
            }
            return Ok(at);
        }
        if !self.mem().map(at, len) {
            self.mem().unmap(at, len);
            return Err(super::ENOMEM);
        }
        self.mem().write_bytes(at, &contents);
        Ok(at)
    }

    fn x_uname(&mut self, buf: u32) -> Result<u32, i32> {
        let mut u = vec![0u8; 6 * 65];
        let host = if self.x.hostname.is_empty() { "(none)".to_string() } else { self.x.hostname.clone() };
        for (i, s) in ["Linux", host.as_str(), "2.6.31-leapfrog", "#1 Tue Sep 25 13:39:13 PDT 2012", "armv5tejl", "(none)"].iter().enumerate() {
            let s = &s.as_bytes()[..s.len().min(64)];
            u[i * 65..i * 65 + s.len()].copy_from_slice(s);
        }
        if self.mem().write_bytes(buf, &u) { Ok(0) } else { Err(EFAULT) }
    }

    /// mknod: device nodes and empty files (make_dev_nodes.sh, tar of
    /// /var/dev_nodes.tar). FIFOs aren't made.
    fn x_mknod(&mut self, path: u32, mode: u32, dev: u32) -> Result<u32, i32> {
        use super::vfs::S_IFCHR;
        const S_IFBLK: u32 = 0o060000;
        let p = self.resolve(path, false)?;
        if self.vfs.get(&p).is_some() {
            return Err(17); // EEXIST
        }
        let perm = mode & 0o7777 & !self.g.umask;
        match mode & S_IFMT {
            S_IFCHR | S_IFBLK => self.vfs.insert(&p, (mode & S_IFMT) | perm, Data::Dev(dev & 0xFFFF)),
            0 | S_IFREG => self.vfs.insert(&p, S_IFREG | perm, Data::File(Vec::new())),
            _ => return Err(1), // EPERM
        }
        Ok(0)
    }
}

// ---- the file systems in the NAND (the app's saves) ---------------------

/// The LEBs of UBI volume 0 on a NAND partition.
fn nand_lebs(flash: &crate::nand::Nand, offset: usize, size: usize) -> std::collections::BTreeMap<u32, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    let mut seq: std::collections::HashMap<u32, u64> = Default::default();
    let block = crate::nand::BLOCK;
    for b in 0..size / block {
        let head = flash.read(offset + b * block, crate::nand::PAGE);
        let be32 = |o: usize| u32::from_be_bytes(head[o..o + 4].try_into().unwrap());
        if be32(0) != 0x5542_4923 {
            continue;
        }
        let (vid, data) = (be32(16) as usize, be32(20) as usize);
        if vid + 64 > head.len() || be32(vid) != 0x5542_4921 || be32(vid + 8) != 0 {
            continue;
        }
        let lnum = be32(vid + 12);
        let sqnum = u64::from_be_bytes(head[vid + 40..vid + 48].try_into().unwrap());
        if seq.get(&lnum).is_none_or(|&s| s < sqnum) {
            seq.insert(lnum, sqnum);
            out.insert(lnum, flash.read(offset + b * block + data, block - data));
        }
    }
    out
}

/// The partition (offset, size) of RFS and Bulk on a NAND.
fn rfs_and_bulk(flash: &crate::nand::Nand) -> [(usize, usize); 2] {
    let p = crate::explorer::PARTITIONS;
    [(p[3].1, p[4].1 - p[3].1), (p[4].1, flash.size() - p[4].1)]
}

/// The files of the root file system and Bulk on an Explorer's NAND (as
/// Original leaves it: the packages built in and the app's save applied).
pub fn files_in_nand(flash: &crate::nand::Nand) -> Result<(Vec<crate::ubifs_read::File>, Vec<crate::ubifs_read::File>), String> {
    let [rfs, bulk] = rfs_and_bulk(flash);
    let root = crate::ubifs_read::files(&nand_lebs(flash, rfs.0, rfs.1)).map_err(|e| format!("RFS: {e}"))?;
    let bulk = crate::ubifs_read::files(&nand_lebs(flash, bulk.0, bulk.1)).map_err(|e| format!("Bulk: {e}"))?;
    Ok((root, bulk))
}

/// The Explorer's file systems out of its NAND (see `files_in_nand`), with
/// /dev, /sys and /proc and the NOR's partitions (`nor`, else Ribbit's).
pub fn system_from_nand(flash: &crate::nand::Nand, nor: Option<&[u8]>) -> Result<Vfs, String> {
    let (root, bulk) = files_in_nand(flash)?;
    let mut vfs = Vfs::new();
    add_files(&mut vfs, "", root);
    vfs.mkdir_all("/LF/Bulk");
    add_files(&mut vfs, "/LF/Bulk", bulk);
    finish(&mut vfs, nor);
    Ok(vfs)
}

/// The paths of the root file system the HLE keeps in the NAND when
/// saving: not what the kernel makes at run time (/dev, /proc, /sys, the
/// tmpfs mounts) nor the other volumes (Bulk, the cartridge), but their
/// mount points; under /var/volatile (tmpfs) the folders only.
fn saved(path: &str) -> bool {
    // rcS's copy of /dev (make_dev_nodes.sh): the HLE's /dev isn't the
    // kernel's (the NOR's partitions are files here), so Original's stays.
    if DEV_TAR.contains(&path) {
        return false;
    }
    let under = |p: &str| path.starts_with(&format!("{p}/"));
    if ["/dev", "/proc", "/sys", "/tmp", "/LF/Bulk", "/LF/Cart", "/LF/fuse"].iter().any(|p| under(p)) {
        return false;
    }
    true
}

/// The flight recorder's logs (syslogd -O /LF/Base/FR/frlog).
const FR_LOGS: &str = "/LF/Base/FR/";

/// make_dev_nodes.sh's archive of /dev and the file that tells it to make
/// it again.
const DEV_TAR: [&str; 2] = ["/var/dev_nodes.tar", "/var/dev_nodes_doom"];

/// `saved` for a node: under /var/volatile only folders.
fn saved_node(path: &str, node: &super::vfs::Node) -> bool {
    saved(path) && (!path.starts_with("/var/volatile/") || node.is_dir())
}

/// The UBIFS tree (paths relative to `under`) of the files below `under`
/// that `keep` accepts.
fn tree_of(vfs: &Vfs, under: &str, keep: impl Fn(&str) -> bool) -> crate::ubifs::Tree {
    use crate::ubifs::Kind;
    let prefix = if under.is_empty() { "/".to_string() } else { format!("{under}/") };
    let mut tree = crate::ubifs::Tree::new();
    for (path, node) in vfs.nodes.range(prefix.clone()..).take_while(|(p, _)| p.starts_with(&prefix)) {
        if !keep(path) || under.is_empty() && !saved_node(path, node) {
            continue;
        }
        let kind = match &node.data {
            Data::Dir => Kind::Dir,
            Data::File(d) => Kind::File(d.clone()),
            Data::Link(t) => Kind::Symlink(t.clone()),
            // Device nodes and sockets aren't kept (mdev and the programs
            // make them again).
            Data::Dev(_) => continue,
        };
        if node.mode & S_IFMT == 0o140000 {
            continue;
        }
        tree.insert(path[prefix.len()..].to_string(), crate::ubifs::Entry { kind, mode: node.mode & 0o7777, mtime: node.mtime as u64 });
    }
    tree
}

/// Writes the HLE's root file system and Bulk into `flash` (an Explorer
/// NAND) where their files differ from what is there, so a save made in
/// Enhanced (new) boots in Original too. As the Explorer's kernel writes
/// them: the changed files go to the volume's UBIFS journal
/// (`ubifs::journal`), and only the LEBs that takes are written (a few
/// erase blocks: the save stays small). When the journal can't take them
/// (full: its log, max_bud_bytes, free space), the volume is built again
/// whole (`rebuild`), which empties the journal for the next saves.
pub fn write_to_nand(vfs: &Vfs, flash: &mut crate::nand::Nand) -> Result<(), String> {
    write_volumes(vfs, flash, [true; 2])
}

/// `write_to_nand` for the root file system (`which[0]`) and Bulk. Both
/// volumes are read (what they hold, to tell what changed) and checked
/// after the writes at the same time, on threads of their own.
fn write_volumes(vfs: &Vfs, flash: &mut crate::nand::Nand, which: [bool; 2]) -> Result<(), String> {
    use crate::nand::Partition;
    let [rfs, bulk] = rfs_and_bulk(flash);
    let parts = [Partition { name: "RFS", offset: rfs.0, size: rfs.1 }, Partition { name: "Bulk", offset: bulk.0, size: bulk.1 }];
    // The root file system: what `saved` keeps (not the flight recorder's
    // logs: syslog writes them all the time); what is under /dev, rcS's copy
    // of /dev and those logs stay as the NAND has them.
    let root_managed = |p: &str, dir: bool| {
        let p = format!("/{p}");
        saved(&p) && !p.starts_with(FR_LOGS) && (dir || !p.starts_with("/var/volatile/"))
    };
    let managed: [&(dyn Fn(&str, bool) -> bool + Sync); 2] = [&root_managed, &|_: &str, _: bool| true];
    let trees = [
        which[0].then(|| tree_of(vfs, "", |p| saved(p) && !p.starts_with(FR_LOGS))),
        which[1].then(|| tree_of(vfs, "/LF/Bulk", |_| true)),
    ];
    let mut failed: [Option<String>; 2] = [None, None];
    // What each volume's journal takes.
    let plans: Vec<Option<Result<Plan, String>>> = std::thread::scope(|s| {
        let flash = &*flash;
        let jobs: Vec<_> = (0..2).map(|i| trees[i].as_ref().map(|t| s.spawn(move || plan(flash, parts[i], t, managed[i])))).collect();
        jobs.into_iter().map(|j| j.map(|j| j.join().unwrap_or_else(|_| Err("a panic".into())))).collect()
    });
    // Written, one volume after the other.
    let was = flash.clone();
    let mut written: [Option<BTreeMap<u32, Vec<u8>>>; 2] = [None, None];
    for (i, p) in plans.into_iter().enumerate() {
        match p {
            None => {}
            Some(Err(e)) => failed[i] = Some(e),
            Some(Ok(p)) if p.writes.is_empty() => {}
            Some(Ok(p)) => match apply(flash, parts[i], p) {
                Ok(after) => written[i] = Some(after),
                Err(e) => {
                    flash.copy_blocks(&was, parts[i].offset / crate::nand::BLOCK, parts[i].size / crate::nand::BLOCK);
                    failed[i] = Some(e);
                }
            },
        }
    }
    // Read back as the kernel's replay will find them.
    let checks: Vec<Option<Result<(), String>>> = std::thread::scope(|s| {
        let flash = &*flash;
        let jobs: Vec<_> = (0..2)
            .map(|i| match (&written[i], &trees[i]) {
                (Some(after), Some(tree)) => Some(s.spawn(move || check(flash, parts[i], after, tree, managed[i]))),
                _ => None,
            })
            .collect();
        jobs.into_iter().map(|j| j.map(|j| j.join().unwrap_or_else(|_| Err("a panic".into())))).collect()
    });
    for (i, c) in checks.into_iter().enumerate() {
        if let Some(Err(e)) = c {
            flash.copy_blocks(&was, parts[i].offset / crate::nand::BLOCK, parts[i].size / crate::nand::BLOCK);
            failed[i] = Some(e);
        }
    }
    // A volume the journal couldn't take is built again whole.
    let [root_tree, bulk_tree] = trees;
    if let (Some(e), Some(tree)) = (&failed[0], root_tree) {
        eprintln!("[hle] the root file system's journal: {e}; building it again");
        rebuild_root(flash, parts[0], tree)?;
    }
    if let (Some(e), Some(tree)) = (&failed[1], bulk_tree) {
        eprintln!("[hle] Bulk's journal: {e}; building it again");
        rebuild(flash, parts[1], "ubi_bulk", &tree)?;
    }
    Ok(())
}

/// What `ubifs::journal` takes on a volume: its LEBs before, whether UBI
/// must first resize it, the LEBs to write.
struct Plan {
    lebs: BTreeMap<u32, Vec<u8>>,
    resize: bool,
    writes: Vec<(usize, Vec<u8>)>,
}

fn plan(flash: &crate::nand::Nand, part: crate::nand::Partition, tree: &crate::ubifs::Tree, managed: &dyn Fn(&str, bool) -> bool) -> Result<Plan, String> {
    let lebs = nand_lebs(flash, part.offset, part.size);
    let (size, resize) = crate::ubi::volume_size(flash, part).ok_or("no UBI volume table")?;
    let writes = crate::ubifs::journal(&lebs, size, tree, managed)?;
    Ok(Plan { lebs, resize, writes })
}

/// A plan's LEBs written as UBI changes a LEB (after its resize); the
/// volume's LEBs as they should now read.
fn apply(flash: &mut crate::nand::Nand, part: crate::nand::Partition, plan: Plan) -> Result<BTreeMap<u32, Vec<u8>>, String> {
    if plan.resize {
        crate::ubi::autoresize(flash, part)?;
    }
    let mut after = plan.lebs;
    for (lnum, data) in plan.writes {
        crate::ubi::write_leb(flash, part, lnum, &data)?;
        let mut leb = data;
        leb.resize(crate::ubi::LEB, 0xFF);
        after.insert(lnum as u32, leb);
    }
    Ok(after)
}

/// Whether the volume on `part` reads `after` (what was planned) and holds
/// `tree` where `managed` says it is the tree's.
fn check(flash: &crate::nand::Nand, part: crate::nand::Partition, after: &BTreeMap<u32, Vec<u8>>, tree: &crate::ubifs::Tree, managed: &dyn Fn(&str, bool) -> bool) -> Result<(), String> {
    if nand_lebs(flash, part.offset, part.size) != *after {
        return Err("the LEBs didn't land".to_string());
    }
    let count = after.keys().next_back().map_or(0, |&l| l as usize + 1);
    let slices: Vec<&[u8]> = (0..count as u32).map(|l| after.get(&l).map_or(&[][..], |v| &v[..])).collect();
    let read = crate::ubifs_read::read(&slices, &|l| after.contains_key(&(l as u32)));
    for (p, e) in tree {
        if !read.get(p).is_some_and(|r| r.mode == e.mode && r.mtime == e.mtime && same_entry(&r.kind, &e.kind)) {
            return Err(format!("{p} didn't read back"));
        }
    }
    let above = |p: &str| tree.range(format!("{p}/")..).next().is_some_and(|(k, _)| k.starts_with(&format!("{p}/")));
    if let Some(p) = read.iter().find(|(p, e)| !tree.contains_key(*p) && !above(p) && managed(p, matches!(e.kind, crate::ubifs::Kind::Dir))).map(|(p, _)| p) {
        return Err(format!("{p} is still there"));
    }
    Ok(())
}

fn same_entry(a: &crate::ubifs::Kind, b: &crate::ubifs::Kind) -> bool {
    use crate::ubifs::Kind;
    match (a, b) {
        (Kind::Dir, Kind::Dir) => true,
        (Kind::File(x), Kind::File(y)) => x == y,
        (Kind::Symlink(x), Kind::Symlink(y)) => x == y,
        (Kind::Special(t, x), Kind::Special(u, y)) => t == u && x == y,
        _ => false,
    }
}

/// `part` built again as a UBI volume holding `tree` as UBIFS (every erase
/// block of the partition written).
fn rebuild(flash: &mut crate::nand::Nand, part: crate::nand::Partition, name: &str, tree: &crate::ubifs::Tree) -> Result<(), String> {
    let lebs = crate::ubi::available_blocks(part.size / crate::nand::BLOCK);
    let volume = crate::ubifs::build(tree, lebs).map_err(|e| format!("{}: {e}", part.name))?;
    crate::ubi::write_volume(flash, part, name, &volume)
}

/// The root file system built again from `tree`, with what is under the
/// /dev mount on the NAND (the nodes init starts with), rcS's copy of /dev
/// and the flight recorder's logs as the NAND has them.
fn rebuild_root(flash: &mut crate::nand::Nand, part: crate::nand::Partition, mut tree: crate::ubifs::Tree) -> Result<(), String> {
    let (root_now, _) = files_in_nand(flash)?;
    let from_nand = |path: &str| DEV_TAR.contains(&path) || path.starts_with("/dev/") || path.starts_with(FR_LOGS);
    for f in root_now.into_iter().filter(|f| from_nand(&f.path)) {
        let kind = match f.mode & S_IFMT {
            S_IFDIR => crate::ubifs::Kind::Dir,
            S_IFREG => crate::ubifs::Kind::File(f.data),
            S_IFLNK => crate::ubifs::Kind::Symlink(String::from_utf8_lossy(&f.data).into_owned()),
            t => crate::ubifs::Kind::Special(t, f.data),
        };
        tree.insert(f.path[1..].to_string(), crate::ubifs::Entry { kind, mode: f.mode & 0o7777, mtime: f.mtime as u64 });
    }
    rebuild(flash, part, "ubi_rfs", &tree)
}

/// Fingerprints of what `write_to_nand` keeps, the root file system's and
/// Bulk's: the app writes a volume again only when its own changed.
pub fn fingerprints(vfs: &Vfs) -> [u32; 2] {
    let mut h = [crc32fast::Hasher::new(), crc32fast::Hasher::new()];
    for (path, node) in &vfs.nodes {
        let h = match path.starts_with("/LF/Bulk/") {
            true => &mut h[1],
            false if saved_node(path, node) && !path.starts_with(FR_LOGS) => &mut h[0],
            false => continue,
        };
        h.update(path.as_bytes());
        h.update(&node.mode.to_le_bytes());
        match &node.data {
            Data::File(d) => h.update(d),
            Data::Link(t) => h.update(t.as_bytes()),
            Data::Dir | Data::Dev(_) => {}
        }
    }
    h.map(|h| h.finalize())
}

/// `write_to_nand` for the volumes whose fingerprint (`fingerprints`)
/// differs from `written`, which then takes the new ones.
pub fn write_changed_to_nand(vfs: &Vfs, flash: &mut crate::nand::Nand, written: &mut [u32; 2]) -> Result<(), String> {
    let now = fingerprints(vfs);
    if now != *written {
        write_volumes(vfs, flash, [now[0] != written[0], now[1] != written[1]])?;
        *written = now;
    }
    Ok(())
}

// ---- cartridges --------------------------------------------------------

/// What the kernel shows with a (Didj) cartridge in, read on Original with
/// Sonic in the slot (2026-11-12): the MTDs the NAND driver and UBI add,
/// and the vfat mount CartManager makes.
const CART_MTD: &str = "mtd10: 04000000 00004000 \"Cartridge\"\nmtd11: 03b5ec00 00003c00 \"ubi_Cartridge\"\n";
const CART_MOUNT: &str = "/dev/mtdblock11 /LF/Cart vfat ro,relatime,fmask=0022,dmask=0022,codepage=cp437,iocharset=iso8859-1,errors=remount-ro 0 0\n";

impl Process {
    fn proc_file(&self, path: &str) -> String {
        match self.vfs.get(path).map(|n| &n.data) {
            Some(Data::File(d)) => String::from_utf8_lossy(d).into_owned(),
            _ => String::new(),
        }
    }

    /// A cartridge put in the slot (`raw`: its dump, as the Didj's), or
    /// taken out (None), on the Explorer: the kernel's view changes (the
    /// lfcart device, cart_hotswap, the MTDs, UBI's ubi2) and, once the
    /// programs run, mdev's cartridge.sh tells CartManager (cnotify's
    /// message on /tmp/cartridge), which attaches and mounts it at
    /// /LF/Cart through the HLE (`mount`). At power-on CartManager finds it
    /// by itself.
    pub fn explorer_cart(&mut self, raw: Option<&[u8]>) -> Result<(), String> {
        let files = match raw {
            Some(r) => Some(crate::cart::files(r).ok_or("no cartridge volume in the dump")?),
            None => None,
        };
        let inserted = files.is_some();
        self.x.cart = files;
        let lfcart = "/sys/bus/lfcart/devices/cartridge";
        let mtd = self.proc_file("/proc/mtd").replace(CART_MTD, "");
        if inserted {
            self.vfs.mkdir_all(lfcart);
            self.vfs.add_file(&format!("{lfcart}/uevent"), Vec::new());
            self.vfs.add_file("/proc/mtd", format!("{mtd}{CART_MTD}").into_bytes());
            for name in ["ubi2", "ubi2_0"] {
                self.vfs.mkdir_all(&format!("/sys/class/ubi/{name}"));
            }
            self.vfs.add_file("/sys/class/ubi/ubi2_0/name", b"ubi_Cartridge\n".to_vec());
            self.vfs.add_file("/sys/class/mtd/mtd10/name", b"Cartridge\n".to_vec());
            self.vfs.add_file("/sys/class/mtd/mtd11/name", b"ubi_Cartridge\n".to_vec());
        } else {
            self.vfs.remove(&format!("{lfcart}/uevent"));
            self.vfs.remove(lfcart);
            self.vfs.add_file("/proc/mtd", mtd.into_bytes());
            for p in ["/sys/class/ubi/ubi2_0/name", "/sys/class/ubi/ubi2_0", "/sys/class/ubi/ubi2", "/sys/class/mtd/mtd10/name", "/sys/class/mtd/mtd10", "/sys/class/mtd/mtd11/name", "/sys/class/mtd/mtd11"] {
                self.vfs.remove(p);
            }
            // The card's files go with it (the kernel's vfat loses its device).
            self.cart_unmount();
        }
        self.vfs.add_file("/sys/devices/platform/lf1000-nand/cart_hotswap", if inserted { b"1\t1\n".to_vec() } else { b"0\t0\n".to_vec() });
        // cnotify (sbin/cnotify): connect to /tmp/cartridge, send 1, 0x40.
        let msg: Vec<u8> = [1u32, 0x40].iter().flat_map(|w| w.to_le_bytes()).collect();
        self.sock_notify("/tmp/cartridge", msg);
        Ok(())
    }

    /// The cartridge's volume at /LF/Cart (mount of the cartridge's MTD).
    pub(super) fn cart_mount(&mut self) -> bool {
        let Some(files) = self.x.cart.clone() else { return false };
        self.vfs.mkdir_all("/LF/Cart");
        for (path, data) in files {
            let full = format!("/LF/Cart/{}", path.trim_matches('/'));
            if path.ends_with('/') {
                self.vfs.mkdir_all(&full);
            } else {
                self.vfs.add_file(&full, data);
            }
        }
        let mounts = self.proc_file("/proc/mounts").replace(CART_MOUNT, "");
        self.vfs.add_file("/proc/mounts", format!("{mounts}{CART_MOUNT}").into_bytes());
        true
    }

    pub(super) fn cart_unmount(&mut self) {
        let under: Vec<String> = self.vfs.list("/LF/Cart").into_iter().map(|(n, _)| format!("/LF/Cart/{n}")).collect();
        for p in under {
            let inner: Vec<String> = self.vfs.nodes.range(format!("{p}/")..).take_while(|(k, _)| k.starts_with(&format!("{p}/"))).map(|(k, _)| k.clone()).collect();
            for k in inner {
                self.vfs.remove(&k);
            }
            self.vfs.remove(&p);
        }
        let mounts = self.proc_file("/proc/mounts").replace(CART_MOUNT, "");
        self.vfs.add_file("/proc/mounts", mounts.into_bytes());
    }

    /// mount: the cartridge's volume onto /LF/Cart; everything else is
    /// there already.
    pub(super) fn x_mount(&mut self, target: u32) -> Result<u32, i32> {
        let t = self.mem().cstr(target).unwrap_or_default();
        if t.trim_end_matches('/') == "/LF/Cart" {
            return if self.cart_mount() { Ok(0) } else { Err(19) }; // ENODEV
        }
        Ok(0)
    }

    pub(super) fn x_umount(&mut self, target: u32) -> Result<u32, i32> {
        let t = self.mem().cstr(target).unwrap_or_default();
        if t.trim_end_matches('/') == "/LF/Cart" {
            self.cart_unmount();
        }
        Ok(0)
    }

    /// /dev/ubi_ctrl: attaching the cartridge's MTD (UBI_IOCATT, as ubi2)
    /// and detaching it (UBI_IOCDET).
    pub(super) fn ubi_ctrl_ioctl(&mut self, cmd: u32, arg: u32) -> Result<u32, i32> {
        match cmd & 0xFFFF {
            0x6F40 => {
                if self.x.cart.is_none() {
                    return Err(19);
                }
                // struct ubi_attach_req: ubi_num, mtd_num, vid_hdr_offset.
                let _ = self.mem().put32(arg, 2);
                Ok(0)
            }
            0x6F41 => Ok(0),
            _ => Err(ENOTTY),
        }
    }
}
