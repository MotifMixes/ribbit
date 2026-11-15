//! LeapPad 2 Enhanced (new): the LeapPad 2's programs on the HLE.
//! The LeapPad 2 ("Valencia", 2012) runs LeapFrog's LF2000 (Nexell NXP3200,
//! an ARMv7 Cortex-A9) with Linux 2.6.37 and the same userland family as the
//! Leapster Explorer (uClibc 0.9.32, Brio, AppManager), built for ARMv7 with
//! VFP. So the Explorer's HLE runs it, with the ARM core's ARMv7 mode on and
//! what the LF2000 kernel shows instead of the LF1000's: its /sys (board
//! "VALENCIA", family "LPAD", a 480x272 screen), /proc, /dev and uname.
//! Facts and sources: docs/leappad2.md.

use super::Process;
use super::vfs::{Data, S_IFLNK, Vfs};

/// The screen: 480x272 (board.c get_lcd_size: MADRID and VALENCIA
/// LCD_480_272, ILI6480G2 module).
pub const SCREEN: (u32, u32) = (480, 272);

/// The HLE machine's SDRAM. The real one has 128 MB (LF2000 board configs:
/// the kernel's 62 MB from 0x8020_0000, then the linear and block video
/// memory up to 0x8780_0000); 64 MB is what the Explorer's HLE has, and
/// enough while the programs fit (GUESS: to be raised if they don't).
pub const SDRAM: usize = 64 << 20;

/// ARMv7 Cortex-A9 without NEON: swp half thumb fastmult vfp edsp vfpv3
/// vfpv3d16 tls (HWCAP_*).
pub(super) const HWCAP: u32 = 1 | 2 | 4 | 16 | 64 | 128 | 8192 | 16384 | 32768;

/// /dev/input/event0..4 names (EVIOCGNAME). From the LF2000 kernel:
/// gpio-keys (mach-nxp3200/keyboard.c), "touchscreen interface"
/// (lf2000_ts.c), "LF2000 Accelerometer", "LF2000 USB"; GUESS: the order,
/// and the power button's name (the Explorer's).
pub(super) const INPUT_NAMES: [&str; 5] = ["Power Button", "gpio-keys", "touchscreen interface", "LF2000 Accelerometer", "LF2000 USB"];

pub(super) const UNAME: [&str; 4] = ["2.6.37.4", "#1 PREEMPT Wed Aug 14 16:33:33 PDT 2013", "armv7l", "v7l"];

/// /sys/devices/system/board (arch/arm/mach-nxp3200/board.c): what rcS and
/// the libraries ask to tell the consoles apart. system_rev 0x206 is
/// LF2000_BOARD_VALENCIA (board_revisions.h); GUESS: retail units may be
/// the EP/FEP revisions 0x207/0x208 (front camera).
const BOARD_FILES: &[(&str, &str)] = &[
    ("platform", "VALENCIA\n"),
    ("platform_family", "LPAD\n"),
    ("lcd_size", "480x272\n"),
    ("lcd_type", "ILI6480G2\n"),
    ("lcd_mfg", "GiantPlus-1\n"),
    ("lcd_mfg_get", "GiantPlus-1\n"),
    ("system_rev", "0x206\n"),
];

/// /sys/devices/platform files. GUESS: names from the LF2000 kernel source
/// and rcS (lf2000-power, lf2000-aclmtr, lf2000-alive), values as the
/// Explorer's where the meaning is the same; not read on a real LeapPad 2.
const PLATFORM_FILES: &[(&str, &str)] = &[
    ("lf2000-alive/panic", "0\n"),
    ("lf2000-alive/request", "PLAY\n"),
    ("lf2000-alive/power", "COLD\n"),
    ("lf2000-alive/shutdown", "CLEAN\n"),
    ("lf2000-alive/boot_source", "NAND\n"),
    ("lf2000-alive/boot_image", "PLAY\n"),
    ("lf2000-power/adc_constant", "0\n"),
    ("lf2000-power/adc_slope_256", "2012\n"),
    ("lf2000-power/status", "1\n"),
    ("lf2000-power/external", "1\n"),
    ("lf2000-power/voltage", "5973\n"),
    ("lf2000-power/power_source", "NIMH_EXTERNAL\n"),
    ("lf2000-power/shutdown", "0\n"),
    ("lf2000-power/max_battery_mv", "8000\n"),
    ("lf2000-power/low_battery_mv", "4200\n"),
    ("lf2000-power/critical_battery_mv", "2000\n"),
    ("lf2000-aclmtr/calibration", "0 0 0\n"),
    ("lf2000-aclmtr/enable", "0\n"),
    ("lf2000-aclmtr/rate", "50\n"),
    ("lf2000-aclmtr/orient", "0\n"),
    ("lf2000-aclmtr/raw_xyz", "0 0 31\n"),
    ("lf2000-aclmtr/raw_phi", "0\n"),
    ("lf2000-aclmtr/bias", "0 0 0\n"),
    ("lf2000-usbgadget/vbus", "0\n"),
    // The touch screen driver (lf2000_ts.c); BaseUtils::HasTouchScreen and
    // libButtonMPI look here. The HLE's stylus reports screen pixels, so the
    // calibration is the identity (GUESS for the rest: the Explorer's values,
    // setcal's 5-inch defaults).
    ("lf2000-touchscreen/pointercal", "65536 0 0 0 65536 0 65536\n"),
    ("lf2000-touchscreen/pressure_curve", "384 777 806 821 848 1061 1102 1133 1186\n"),
    ("lf2000-touchscreen/report_events", "1\n"),
    ("lf2000-touchscreen/sample_rate_in_hz", "50\n"),
    ("lf2000-touchscreen/max_tnt_down", "50\n"),
    ("lf2000-touchscreen/min_tnt_up", "500\n"),
    ("lf2000-touchscreen/max_delta_tnt", "20\n"),
    ("lf2000-touchscreen/delay_in_us", "30\n"),
    ("lf2000-touchscreen/y_delay_in_us", "15\n"),
    ("lf2000-touchscreen/tnt_delay_in_us", "40\n"),
    ("lf2000-touchscreen/tnt_mode", "0\n"),
    ("lf2000-touchscreen/tails", "0\n"),
    ("lf2000-touchscreen/touch_button", "0\n"),
    // lf2000_ts.c show_min_x etc.: the raw range.
    ("lf2000-touchscreen/min_x", "1\n"),
    ("lf2000-touchscreen/max_x", "1024\n"),
    ("lf2000-touchscreen/min_y", "1\n"),
    ("lf2000-touchscreen/max_y", "1024\n"),
];

/// /proc files. GUESS: the kernel's built-in command line (from kernel.bin),
/// the rest by analogy with the Explorer's.
const PROC_FILES: &[(&str, &str)] = &[
    // GUESS: screen_module as the Explorer's boot loader passes it (setcal
    // greps it for ILI6480G2, the 5-inch screen).
    ("/proc/cmdline", "console=ttyS0,115200n8 root=ubi0_0 rw rootfstype=ubifs ubi.mtd=RFS ramdisk=24576 screen_module=ILI6480G2\n"),
    (
        "/proc/mtd",
        "dev:    size   erasesize  name\nmtd0: 00100000 00100000 \"Boot\"\nmtd1: 00800000 00100000 \"Kernel\"\n\
         mtd2: 08000000 00100000 \"RFS\"\nmtd3: f7700000 00100000 \"Bulk\"\n\
         mtd5: 0007e000 00001000 \"NOR_Boot\"\nmtd6: 00001000 00001000 \"MfgData0\"\nmtd7: 00001000 00001000 \"MfgData1\"\n",
    ),
    (
        "/proc/version",
        "Linux version 2.6.37.4 (valencia3@lf2000-release) (gcc version 4.5.4 20110722 (prerelease) (GCC) ) \
         #1 PREEMPT Wed Aug 14 16:33:33 PDT 2013\n",
    ),
    ("/proc/sys/kernel/version", "#1 PREEMPT Wed Aug 14 16:33:33 PDT 2013\n"),
    ("/proc/sys/kernel/core_pattern", "core\n"),
    ("/proc/stat", "cpu  100 0 100 10000 0 0 0 0 0 0\ncpu0 100 0 100 10000 0 0 0 0 0 0\nintr 0\nctxt 0\nbtime 0\nprocesses 1\nprocs_running 1\nprocs_blocked 0\n"),
    (
        "/proc/cpuinfo",
        "Processor\t: ARMv7 Processor rev 0 (v7l)\nBogoMIPS\t: 530.84\n\
         Features\t: swp half thumb fastmult vfp edsp vfpv3 vfpv3d16 tls \nCPU implementer\t: 0x41\n\
         CPU architecture: 7\nCPU variant\t: 0x2\nCPU part\t: 0xc09\nCPU revision\t: 0\n\n\
         Hardware\t: LF2000\nRevision\t: 0206\nSerial\t\t: 0000000000000000\n",
    ),
    ("/proc/asound/card0/id", "LF2000LFP100\n"),
    ("/proc/asound/cards", " 0 [LF2000LFP100   ]: LFP100 - LF2000-LFP100\n                      LF2000-LFP100 (LFP100)\n"),
    (
        "/proc/mounts",
        "rootfs / rootfs rw 0 0\nubi0:RFS / ubifs rw,relatime 0 0\nmdev /dev tmpfs rw,relatime 0 0\nnone /proc proc rw,relatime 0 0\n\
         sysfs /sys sysfs rw,relatime 0 0\n/dev/ram0 /tmp tmpfs rw,relatime,mode=777 0 0\n\
         none /dev/pts devpts rw,relatime,mode=600 0 0\nubi1:ubi_bulk /LF/Bulk ubifs rw,relatime 0 0\n",
    ),
    (
        "/proc/meminfo",
        "MemTotal:          59272 kB\nMemFree:           30120 kB\nBuffers:               0 kB\nCached:             9596 kB\n\
         SwapCached:            0 kB\nSwapTotal:             0 kB\nSwapFree:              0 kB\n",
    ),
    (
        "/proc/bus/input/devices",
        "I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"Power Button\"\nH: Handlers=event0 \nB: EV=3\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF2000 Keyboard\"\nH: Handlers=event1 \nB: EV=23\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF2000 touchscreen interface\"\nH: Handlers=event2 \nB: EV=b\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF2000 Accelerometer\"\nH: Handlers=event3 \nB: EV=9\n\n\
         I: Bus=0019 Vendor=0001 Product=0001 Version=0001\nN: Name=\"LF2000 USB\"\nH: Handlers=event4 \nB: EV=21\n\n",
    ),
];

/// Whether a firmware package (an .lfp zip) is the LeapPad 2's.
pub fn for_leappad2(data: &[u8]) -> bool {
    let Ok(entries) = ribbit_core::zip::entries(data) else { return false };
    entries.iter().filter(|e| e.name.to_ascii_lowercase().ends_with("meta.inf")).any(|e| {
        ribbit_core::zip::read_entry(data, e).is_ok_and(|m| crate::package::Meta::parse(&String::from_utf8_lossy(&m)).get("Device") == Some("LeapPad2Explorer"))
    })
}

/// The LeapPad 2's file systems out of a folder of its firmware packages:
/// the root file system (`5,...erootfs.ubi` in Firmware-Base), /LF/Bulk with
/// the `.lf2` packages installed as lfpkg would, and what the kernel adds.
/// Packages that don't install to Bulk are skipped (named in the result).
pub fn system_from_packages(dir: &std::path::Path) -> Result<(Vfs, Vec<String>), String> {
    let mut rootfs = None;
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lfp"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if !for_leappad2(&data) {
            continue;
        }
        for entry in ribbit_core::zip::entries(&data).map_err(|e| format!("{}: {e}", path.display()))? {
            let file = entry.name.rsplit('/').next().unwrap_or("");
            if file.ends_with("erootfs.ubi") {
                rootfs = Some(ribbit_core::zip::read_entry(&data, &entry).map_err(|e| format!("{}: {e}", path.display()))?);
            }
        }
    }
    let rootfs = rootfs.ok_or_else(|| format!("no LeapPad 2 firmware (Firmware-Base) in {}", dir.display()))?;
    let mut vfs = Vfs::new();
    super::explorer::add_files(&mut vfs, "", crate::ubifs_read::files_in_ubi(&rootfs).map_err(|e| format!("erootfs.ubi: {e}"))?);
    let mut bulk = crate::ubifs::Tree::new();
    let mut skipped = Vec::new();
    for path in entries.iter().filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("lf2"))) {
        let data = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        if let Err(e) = crate::explorer::install_lf2(&mut bulk, &data) {
            skipped.push(format!("{}: {e}", path.display()));
        }
    }
    super::explorer::add_tree(&mut vfs, "/LF/Bulk", &bulk);
    finish(&mut vfs);
    // OpenGL ES and EGL answered by Ribbit (gles.rs): the VR5 GPU isn't modelled.
    super::gles::patch_libraries(&mut vfs);
    Ok((vfs, skipped))
}

/// /dev, /tmp, /sys and /proc as the LF2000 kernel would show them.
fn finish(vfs: &mut Vfs) {
    for d in ["/tmp", "/dev/pts", "/var/volatile/run", "/var/volatile/log", "/LF/Cart", "/LF/fuse"] {
        vfs.mkdir_all(d);
    }
    for (name, major, minor) in super::explorer::DEV_NODES {
        let path = format!("/dev/{name}");
        vfs.mkdir_all(super::vfs::split(&path).0);
        vfs.insert(&path, super::vfs::S_IFCHR | 0o666, Data::Dev(major << 8 | minor));
    }
    vfs.insert("/dev/input/touchscreen0", S_IFLNK | 0o777, Data::Link("/dev/input/event2".into()));
    for (path, text) in BOARD_FILES {
        vfs.add_file(&format!("/sys/devices/system/board/{path}"), text.as_bytes().to_vec());
    }
    for (path, text) in PLATFORM_FILES {
        vfs.add_file(&format!("/sys/devices/platform/{path}"), text.as_bytes().to_vec());
    }
    for (path, text) in PROC_FILES {
        vfs.add_file(path, text.as_bytes().to_vec());
    }
    for (n, name) in [(0, "Boot"), (1, "Kernel"), (2, "RFS"), (3, "Bulk"), (5, "NOR_Boot"), (6, "MfgData0"), (7, "MfgData1")] {
        vfs.add_file(&format!("/sys/class/mtd/mtd{n}/name"), format!("{name}\n").into_bytes());
    }
    // Factory data: the Explorer's NOR layout and a made-up unit's data
    // (GUESS: where the LeapPad 2 keeps MfgData0/1 is UNKNOWN; its programs
    // look for those names in /proc/mtd as the Explorer's do).
    super::explorer::set_nor(vfs, &crate::nor::image(crate::explorer::SERIAL).data);
    for n in 0..3 {
        vfs.add_file(&format!("/sys/class/graphics/fb{n}/dev"), format!("29:{n}\n").into_bytes());
    }
    // A calibrated unit's touch calibration (setcal reads /flags/pointercal
    // first; without one the first-time setup starts with Screen
    // Calibration). The identity: the HLE's stylus reports screen pixels.
    if vfs.get("/flags/pointercal").is_none() {
        vfs.add_file("/flags/pointercal", b"65536 0 0 0 65536 0 65536\n".to_vec());
    }
    vfs.add_file("/var/dev_nodes_doom", Vec::new());
    vfs.add_file("/tmp/bulk_ready", b"1\n".to_vec());
}

impl Process {
    /// A process for the LeapPad 2's programs: the Explorer's HLE with the
    /// ARM core in ARMv7 mode and a 480x272 screen.
    pub fn new_leappad2(vfs: Vfs) -> Self {
        let mut p = Self::new_explorer_sized(vfs, SCREEN.0, SCREEN.1);
        p.x.leappad2 = true;
        p.cpu.v7 = Some(Box::default());
        p.m.jit.v7 = true;
        p
    }

    /// A cartridge put in (`raw`: its dump) or taken out, on the LeapPad 2.
    /// There the kernel's lfcart makes mdev run /etc/mdev/cartridge.sh,
    /// which sets lf2000-nand/cart_hotswap, mounts /dev/mtdblock7 at
    /// /LF/Cart and runs cnotify: 1 (CARTRIDGE_INSERTED) and, mounted, 3
    /// (READY); on removal 4 (REMOVED), the unmount, 6 (CLEAN). Ribbit does
    /// what the script does: cnotify connects to /tmp/cart_events_socket
    /// and sends {1, state} (8 bytes), and writes /tmp/cart_brio_state
    /// ("state, name"), from its disassembly.
    pub(super) fn leappad2_cart(&mut self, raw: Option<&[u8]>) -> Result<(), String> {
        let files = match raw {
            Some(r) => Some(crate::cart::files(r).ok_or("no cartridge volume in the dump")?),
            None => None,
        };
        let inserted = files.is_some();
        let notify = |p: &mut Self, state: u32, name: &str| {
            p.vfs.add_file("/tmp/cart_brio_state", format!("{state}, {name}").into_bytes());
            let msg: Vec<u8> = [1u32, state].iter().flat_map(|w| w.to_le_bytes()).collect();
            p.sock_notify("/tmp/cart_events_socket", msg);
        };
        const HOTSWAP: &str = "/sys/devices/platform/lf2000-nand/cart_hotswap";
        let mtd = self.proc_file("/proc/mtd").replace(CART_MTD, "");
        if inserted {
            self.x.cart = files;
            self.vfs.add_file(HOTSWAP, b"1\n".to_vec());
            self.vfs.add_file("/proc/mtd", format!("{mtd}{CART_MTD}").into_bytes());
            self.vfs.add_file("/sys/class/mtd/mtd7/name", b"Cartridge\n".to_vec());
            notify(self, 1, "CARTRIDGE_STATE_INSERTED");
            self.cart_mount();
            self.vfs.add_file("/cart_mounted", b"1\n".to_vec());
            notify(self, 3, "CARTRIDGE_STATE_READY");
        } else {
            notify(self, 4, "CARTRIDGE_STATE_REMOVED");
            self.cart_unmount();
            self.x.cart = None;
            self.vfs.remove("/cart_mounted");
            self.vfs.add_file(HOTSWAP, b"0\n".to_vec());
            self.vfs.add_file("/proc/mtd", mtd.into_bytes());
            self.vfs.remove("/sys/class/mtd/mtd7/name");
            notify(self, 6, "CARTRIDGE_STATE_CLEAN");
        }
        Ok(())
    }
}

/// The cartridge's MTD as the LeapPad 2's cartridge.sh names it
/// (/dev/mtdblock7). GUESS: its size line, as the Explorer's 64 MB part.
const CART_MTD: &str = "mtd7: 04000000 00004000 \"Cartridge\"\n";

impl Process {
    /// SysV semaphores (ipc/sem.c semantics, one machine): semget (299),
    /// semctl (300), semop (298) and semtimedop (312, its timeout ignored).
    /// A semop that can't go on waits and tries again, as a sleeping task.
    pub(super) fn sysv_sem(&mut self, n: u32, a: [u32; 6]) -> Result<u32, i32> {
        const ENOENT: i32 = 2;
        const EEXIST: i32 = 17;
        const EINVAL: i32 = 22;
        const EAGAIN: i32 = 11;
        const ERANGE: i32 = 34;
        const IPC_CREAT: u32 = 0o1000;
        const IPC_EXCL: u32 = 0o2000;
        const IPC_NOWAIT: u16 = 0o4000;
        let sems = &mut self.x.sems;
        match n {
            299 => {
                let (key, nsems, flags) = (a[0], a[1] as usize, a[2]);
                if key != 0
                    && let Some((&id, (_, v))) = sems.iter().find(|(_, (k, _))| *k == key)
                {
                    if flags & IPC_CREAT != 0 && flags & IPC_EXCL != 0 {
                        return Err(EEXIST);
                    }
                    if nsems > v.len() {
                        return Err(EINVAL);
                    }
                    return Ok(id);
                }
                if key != 0 && flags & IPC_CREAT == 0 {
                    return Err(ENOENT);
                }
                if nsems == 0 || nsems > 250 {
                    return Err(EINVAL);
                }
                let id = sems.keys().next_back().map_or(0x8000, |k| k + 1);
                sems.insert(id, (key, vec![0; nsems]));
                Ok(id)
            }
            300 => {
                let (id, num, cmd, arg) = (a[0], a[1] as usize, a[2] & 0xFF, a[3]);
                let Some((_, v)) = sems.get_mut(&id) else { return Err(EINVAL) };
                match cmd {
                    0 => {
                        sems.remove(&id);
                        Ok(0)
                    }
                    1 | 2 | 19 | 20 => Ok(0), // IPC_SET, IPC_STAT, SEM_STAT, SEM_INFO: nothing kept
                    11 | 14 | 15 => Ok(0),    // GETPID, GETNCNT, GETZCNT
                    12 => v.get(num).map(|&x| x as u32).ok_or(EINVAL),
                    16 => {
                        if arg as i32 > 32767 || (arg as i32) < 0 {
                            return Err(ERANGE);
                        }
                        *v.get_mut(num).ok_or(EINVAL)? = arg as i32;
                        Ok(0)
                    }
                    13 | 17 => {
                        let len = v.len();
                        if cmd == 13 {
                            let vals: Vec<u8> = v.iter().flat_map(|&x| (x as u16).to_le_bytes()).collect();
                            if !self.mem().write_bytes(arg, &vals) {
                                return Err(super::EFAULT);
                            }
                        } else {
                            let bytes = self.mem().read_bytes(arg, len * 2).ok_or(super::EFAULT)?;
                            let vals: Vec<i32> = bytes.chunks(2).map(|c| u16::from_le_bytes([c[0], c[1]]) as i32).collect();
                            self.x.sems.get_mut(&id).unwrap().1 = vals;
                        }
                        Ok(0)
                    }
                    _ => Err(EINVAL),
                }
            }
            _ => {
                let (id, sops, nsops) = (a[0], a[1], a[2] as usize);
                let bytes = self.mem().read_bytes(sops, nsops * 6).ok_or(super::EFAULT)?;
                let ops: Vec<(usize, i32, u16)> =
                    bytes.chunks(6).map(|c| (u16::from_le_bytes([c[0], c[1]]) as usize, i16::from_le_bytes([c[2], c[3]]) as i32, u16::from_le_bytes([c[4], c[5]]))).collect();
                let Some((_, v)) = self.x.sems.get_mut(&id) else { return Err(EINVAL) };
                let mut new = v.clone();
                for &(num, op, flg) in &ops {
                    let Some(val) = new.get_mut(num) else { return Err(EINVAL) };
                    let blocked = (op < 0 && *val + op < 0) || (op == 0 && *val != 0);
                    if blocked {
                        if flg & IPC_NOWAIT != 0 {
                            return Err(EAGAIN);
                        }
                        self.block = Some(super::task::Wait::Retry(self.clock() + std::time::Duration::from_millis(1)));
                        return Ok(0);
                    }
                    *val += op;
                }
                *v = new;
                Ok(0)
            }
        }
    }
}
