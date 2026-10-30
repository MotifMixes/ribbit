//! System bus: address decoding for ROM/RAM regions.
//!
//! Peripheral registers (`map::MMIO_BASE`) are not decoded here; the
//! machine in ribbit-core routes them. Accesses that hit nothing are
//! recorded so boot failures show *where* the firmware went off the map.

use crate::cartridge::Cartridge;
use crate::firmware::Firmware;
use crate::map::*;

/// CPU-visible memory interface. Little-endian (the cartridge directory
/// stores 0x8000_0048 as `48 00 00 80`).
pub trait Bus {
    fn read8(&mut self, addr: u32) -> u8;
    fn read16(&mut self, addr: u32) -> u16;
    fn read32(&mut self, addr: u32) -> u32;
    fn write8(&mut self, addr: u32, value: u8);
    fn write16(&mut self, addr: u32, value: u16);
    fn write32(&mut self, addr: u32, value: u32);

    /// The instruction about to execute (cycle count and address), for
    /// buses that time-stamp accesses. Called by the CPU's batched loop
    /// (`Cpu::run_block`) before every instruction.
    #[inline(always)]
    fn at(&mut self, _cycle: u64, _pc: u32) {}

    /// The instruction at `pc` (`len` bytes, decoded) is about to execute:
    /// its fetch, and the accesses until `take_stall`, are its own (memory
    /// timing). Instruction decoding reads come before this and are not timed.
    #[inline(always)]
    fn instruction(&mut self, _pc: u32, _len: u32) {}

    /// As `instruction`, for an instruction on the same 16-byte line as the
    /// end of the one just executed (and not crossing into the next): its
    /// fetch is known to hit that line, so only the timing is armed.
    #[inline(always)]
    fn instruction_same_line(&mut self) {}

    /// An instruction-stream read (decoding): like `read16`, but never timed
    /// as a data access (fetch timing is `instruction`'s).
    #[inline(always)]
    fn fetch16(&mut self, addr: u32) -> u16 {
        self.read16(addr)
    }

    /// Stall cycles the memory system added to the instruction just executed.
    #[inline(always)]
    fn take_stall(&mut self) -> u64 {
        0
    }

    /// True once an access has happened that the machine must handle before
    /// the next instruction (a device register): ends a `Cpu::run_block`.
    #[inline(always)]
    fn block_exit(&self) -> bool {
        false
    }

    /// What a JIT (ribbit-jit) may do itself instead of calling this bus,
    /// with exactly the same effects (`JitView`); None: it calls the bus.
    /// Valid until the bus is next used otherwise than through the view.
    #[inline(always)]
    fn jit_view(&mut self) -> Option<JitView> {
        None
    }
}

/// Plain memory a JIT may read (and, if `writable`, write) itself: at an
/// address in `[base, base + len)`, a bus access of 1, 2 or 4 bytes (at the
/// address with its low bits cleared as the bus clears them) is a
/// little-endian access to `ptr[addr - base..]` plus the data timing below,
/// nothing else (no `at`, no logging). `len` 0: unused.
#[derive(Debug, Clone, Copy)]
pub struct JitRegion {
    pub base: u32,
    pub len: u32,
    pub ptr: *mut u8,
    pub writable: bool,
}

/// The memory system's state, for a JIT (`Bus::jit_view`): what the bus's
/// `instruction`, `instruction_same_line`, `take_stall`, `block_exit` and a
/// region access's data timing do, done in place. The caches are 4-way, 512
/// sets of 16-byte lines; set `(addr >> 4) % 512` holds 4 tags `addr >> 4 |
/// 1 << 31` (valid), data tags also `1 << 30` when dirty. A lookup that
/// hits changes nothing but a data write's dirty bit; a miss is the bus's
/// (`instruction`, or the region access itself through the bus). While an
/// instruction executes, `armed` is true (set by `instruction` or
/// `instruction_same_line`); `take_stall` returns `stall` and clears both.
/// `timing` false: the memory system does no timing (none of the above).
#[derive(Debug, Clone, Copy)]
pub struct JitView {
    pub regions: [JitRegion; 3],
    pub timing: bool,
    pub armed: *mut bool,
    pub stall: *mut u64,
    pub code_tags: *const u32,
    pub data_tags: *mut u32,
    /// `block_exit`'s flag.
    pub exit: *const bool,
    /// The 64 KiB pages of `regions[0]` (RAM), bit n of word n / 64 for the
    /// page at `regions[0].base + n * 64 KiB`, holding code the CPU cached
    /// (`SystemBus::note_code`): a JIT must not write them itself (the bus
    /// watches those writes), whatever `writable` says.
    pub ram_code: [u64; RAM_CODE_WORDS],
}

/// Words of `JitView::ram_code` (64 KiB pages: RAM up to 64 MiB).
pub const RAM_CODE_WORDS: usize = 16;

/// RAM code pages (`SystemBus::note_code`): 1 KiB.
pub const CODE_PAGE_SHIFT: u32 = 10;
/// A page's state: code from it is cached (this flag), and how many times it
/// was written while it was (the low bits).
const CODE_CACHED: u8 = 0x80;
/// Written this many times while its code was cached, a page is volatile:
/// its code is no longer cached (code that keeps changing, like a JIT's).
const CODE_VOLATILE_AFTER: u8 = 3;

impl JitRegion {
    pub const NONE: JitRegion = JitRegion { base: 0, len: 0, ptr: std::ptr::null_mut(), writable: false };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Bios(usize),
    Mmio(u32),
    Vram(usize),
    /// Discarded writes past VRAM (see `map::VRAM_OVERRUN_END`).
    VramOverrun,
    Ram(usize),
    /// Leapster 2 on-chip RAM (`map::SRAM_BASE`).
    Sram(usize),
    Cart(usize),
    Unmapped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AccessKind {
    Read,
    Write,
    RomWrite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmappedAccess {
    pub addr: u32,
    pub width: u8,
    pub kind: AccessKind,
    pub value: u32,
}

/// Number of unmapped accesses kept verbatim; later ones are only counted.
const UNMAPPED_LOG_LIMIT: usize = 4096;

/// Per (address, width, kind) summary of unmapped accesses; never truncated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UnmappedStat {
    pub count: u64,
    /// `SystemBus::now` at the first occurrence.
    pub first_cycle: u64,
    /// `SystemBus::pc` at the first occurrence.
    pub first_pc: u32,
}

pub struct SystemBus {
    bios: Vec<u8>,
    cart: Option<Vec<u8>>,
    ram: Vec<u8>,
    vram: Vec<u8>,
    sram: Vec<u8>,
    unmapped: Vec<UnmappedAccess>,
    unmapped_total: u64,
    unmapped_summary: std::collections::BTreeMap<(u32, u8, AccessKind), UnmappedStat>,
    /// Time stamp for diagnostics (the machine sets the CPU cycle count).
    pub now: u64,
    /// Address of the executing instruction, for diagnostics (set with `now`).
    pub pc: u32,
    /// Byte reads from the cartridge window (fetches included) and the first
    /// one as (cycle, address).
    pub cart_reads: u64,
    pub first_cart_read: Option<(u64, u32)>,
    /// Byte reads per 64 KiB of cartridge, and the last read as (cycle, address).
    pub cart_read_buckets: Vec<u64>,
    pub last_cart_read: Option<(u64, u32)>,
    /// When set, every RAM/VRAM byte written is logged with its old value
    /// (idle-loop detection).
    pub write_log: Option<Vec<(u32, u8)>>,
    /// Per 1 KiB of RAM (`CODE_PAGE_SHIFT`): `CODE_CACHED` and a write count.
    code_pages: Vec<u8>,
    /// 64 KiB pages with a `CODE_CACHED` page (`JitView::ram_code`).
    code_mask: [u64; RAM_CODE_WORDS],
    /// A write hit a page whose code is cached: the CPU's cached forms of RAM
    /// code must be dropped before the next instruction (`take_code_written`).
    pub code_written: bool,
}

impl SystemBus {
    pub fn new(firmware: &Firmware, cart: Option<&Cartridge>) -> Self {
        Self {
            bios: firmware.data().to_vec(),
            cart: cart.map(|c| c.rom().to_vec()),
            // Power-on SDRAM contents are undefined; zero keeps runs
            // deterministic. HYPOTHESIS: firmware does not depend on them.
            ram: vec![0; firmware.ram_size() as usize],
            vram: vec![0; VRAM_SIZE as usize],
            sram: vec![0; firmware.sram_size() as usize],
            unmapped: Vec::new(),
            unmapped_total: 0,
            unmapped_summary: Default::default(),
            now: 0,
            pc: 0,
            cart_reads: 0,
            first_cart_read: None,
            cart_read_buckets: vec![0; cart.map_or(0, |c| c.rom().len().div_ceil(0x1_0000))],
            last_cart_read: None,
            write_log: None,
            code_pages: vec![0; (firmware.ram_size() as usize).div_ceil(1 << CODE_PAGE_SHIFT)],
            code_mask: [0; RAM_CODE_WORDS],
            code_written: false,
        }
    }

    /// The CPU decoded an instruction half-word at `addr` (`Bus::fetch16`):
    /// a RAM page's code may now be cached, so writes to it are watched.
    #[inline(always)]
    pub fn note_code(&mut self, addr: u32) {
        let o = addr.wrapping_sub(RAM_BASE) as usize;
        if let Some(p) = self.code_pages.get_mut(o >> CODE_PAGE_SHIFT)
            && *p & CODE_CACHED == 0
            && *p < CODE_VOLATILE_AFTER
        {
            *p |= CODE_CACHED;
            let big = o >> 16;
            if big < RAM_CODE_WORDS * 64 {
                self.code_mask[big / 64] |= 1 << (big % 64);
            }
        }
    }

    /// A write to RAM at offset `o`, in a page whose code is cached.
    #[cold]
    #[inline(never)]
    fn code_write(&mut self, o: usize) {
        let p = &mut self.code_pages[o >> CODE_PAGE_SHIFT];
        *p = (*p & !CODE_CACHED).saturating_add(1);
        self.code_written = true;
    }

    /// Whether code was written (`code_written`), clearing it: then nothing
    /// is cached any more (the CPU drops its RAM code), so no page is
    /// watched until code is decoded from it again.
    pub fn take_code_written(&mut self) -> bool {
        if !self.code_written {
            return false;
        }
        self.code_written = false;
        self.code_pages.iter_mut().for_each(|p| *p &= !CODE_CACHED);
        self.code_mask = [0; RAM_CODE_WORDS];
        true
    }

    /// The 1 KiB pages of RAM (bit n of word n / 64) whose code is no longer
    /// cached because it kept changing.
    pub fn volatile_code_pages(&self) -> Vec<u64> {
        let mut bits = vec![0u64; self.code_pages.len().div_ceil(64)];
        for (i, p) in self.code_pages.iter().enumerate() {
            if p & !CODE_CACHED >= CODE_VOLATILE_AFTER {
                bits[i / 64] |= 1 << (i % 64);
            }
        }
        bits
    }

    /// Forgets which pages hold code and which changed (memory replaced:
    /// a save state, a restart); the CPU must drop its RAM code as well.
    pub fn forget_code(&mut self) {
        self.code_pages.iter_mut().for_each(|p| *p = 0);
        self.code_mask = [0; RAM_CODE_WORDS];
        self.code_written = false;
    }

    /// `JitView::ram_code`.
    pub fn ram_code_mask(&self) -> [u64; RAM_CODE_WORDS] {
        self.code_mask
    }

    #[inline(always)]
    pub fn decode(&self, addr: u32) -> Region {
        let in_window = |base: u32, size: u32| addr.wrapping_sub(base) < size;
        // The windows are disjoint, so the order only affects speed: RAM
        // (stack, data) first.
        if in_window(RAM_BASE, self.ram.len() as u32) {
            return Region::Ram((addr - RAM_BASE) as usize);
        }
        if in_window(BIOS_BASE, BIOS_WINDOW) || in_window(BIOS_MIRROR, BIOS_WINDOW) {
            // The BIOS window is larger than a 2 MiB image; MAME reads the
            // remainder as zero. HYPOTHESIS: real hardware may mirror.
            let offset = (addr & (BIOS_WINDOW - 1)) as usize;
            return if offset < self.bios.len() { Region::Bios(offset) } else { Region::Unmapped };
        }
        if in_window(MMIO_BASE, MMIO_SIZE) {
            return Region::Mmio(addr - MMIO_BASE);
        }
        if in_window(VRAM_BASE, VRAM_SIZE) {
            return Region::Vram((addr - VRAM_BASE) as usize);
        }
        if in_window(VRAM_BASE + VRAM_SIZE, VRAM_OVERRUN_END - VRAM_BASE - VRAM_SIZE) {
            return Region::VramOverrun;
        }
        if let Some(cart) = &self.cart
            && in_window(CART_BASE, cart.len() as u32)
        {
            return Region::Cart((addr - CART_BASE) as usize);
        }
        if in_window(SRAM_BASE, self.sram.len() as u32) {
            return Region::Sram((addr - SRAM_BASE) as usize);
        }
        Region::Unmapped
    }

    pub fn unmapped_accesses(&self) -> &[UnmappedAccess] {
        &self.unmapped
    }

    pub fn unmapped_total(&self) -> u64 {
        self.unmapped_total
    }

    /// Every distinct unmapped access, with its count and first cycle.
    pub fn unmapped_summary(&self) -> &std::collections::BTreeMap<(u32, u8, AccessKind), UnmappedStat> {
        &self.unmapped_summary
    }

    /// Restores RAM, VRAM and on-chip RAM (save states); false if the sizes differ.
    pub fn restore_memory(&mut self, ram: &[u8], vram: &[u8], sram: &[u8]) -> bool {
        if ram.len() != self.ram.len() || vram.len() != self.vram.len() || sram.len() != self.sram.len() {
            return false;
        }
        self.ram.copy_from_slice(ram);
        self.vram.copy_from_slice(vram);
        self.sram.copy_from_slice(sram);
        self.forget_code();
        true
    }

    pub fn sram(&self) -> &[u8] {
        &self.sram
    }

    /// RAM, VRAM and the BIOS (at its mirror, where code runs) as plain
    /// memory for a JIT (`JitView`): RAM and VRAM writable unless the write
    /// log (idle probes) is on.
    pub fn jit_regions(&mut self) -> [JitRegion; 3] {
        let writable = self.write_log.is_none();
        let bios_len = (self.bios.len() as u32).min(BIOS_WINDOW);
        [
            JitRegion { base: RAM_BASE, len: self.ram.len() as u32, ptr: self.ram.as_mut_ptr(), writable },
            JitRegion { base: VRAM_BASE, len: self.vram.len() as u32, ptr: self.vram.as_mut_ptr(), writable },
            JitRegion { base: BIOS_MIRROR, len: bios_len, ptr: self.bios.as_mut_ptr(), writable: false },
        ]
    }

    /// RAM/VRAM/ROM byte without side effects (no logging); None elsewhere.
    pub fn peek8(&self, addr: u32) -> Option<u8> {
        match self.decode(addr) {
            Region::Bios(o) => self.bios.get(o).copied(),
            Region::Ram(o) => self.ram.get(o).copied(),
            Region::Vram(o) => self.vram.get(o).copied(),
            Region::Sram(o) => self.sram.get(o).copied(),
            Region::Cart(o) => self.cart.as_ref().and_then(|c| c.get(o).copied()),
            _ => None,
        }
    }

    /// Write a byte to RAM, VRAM or on-chip SRAM without side effects;
    /// returns false if the address is outside writable memory.
    pub fn poke8(&mut self, addr: u32, value: u8) -> bool {
        let region = self.decode(addr);
        match region {
            Region::Ram(o) if o < self.ram.len() => {
                if self.code_pages[o >> CODE_PAGE_SHIFT] & CODE_CACHED != 0 {
                    self.code_write(o);
                }
                self.ram[o] = value;
                true
            }
            Region::Vram(o) => self.vram.get_mut(o).map(|b| *b = value).is_some(),
            Region::Sram(o) => self.sram.get_mut(o).map(|b| *b = value).is_some(),
            _ => false,
        }
    }

    pub fn vram(&self) -> &[u8] {
        &self.vram
    }

    pub fn ram(&self) -> &[u8] {
        &self.ram
    }

    fn record(&mut self, addr: u32, width: u8, kind: AccessKind, value: u32) {
        self.unmapped_total += 1;
        let (now, pc) = (self.now, self.pc);
        self.unmapped_summary
            .entry((addr, width, kind))
            .or_insert(UnmappedStat { count: 0, first_cycle: now, first_pc: pc })
            .count += 1;
        if self.unmapped.len() < UNMAPPED_LOG_LIMIT {
            self.unmapped.push(UnmappedAccess { addr, width, kind, value });
        }
        tracing::trace!(target: "ribbit::bus", addr = format_args!("{addr:#010x}"), width, ?kind, value, "unmapped access");
    }

    fn read_byte(&mut self, addr: u32, width: u8) -> Option<u8> {
        match self.decode(addr) {
            Region::Bios(o) => Some(self.bios[o]),
            Region::Vram(o) => Some(self.vram[o]),
            Region::Ram(o) => Some(self.ram[o]),
            Region::Sram(o) => Some(self.sram[o]),
            Region::Cart(o) => {
                self.cart_reads += 1;
                self.first_cart_read.get_or_insert((self.now, addr));
                self.last_cart_read = Some((self.now, addr));
                self.cart_read_buckets[o >> 16] += 1;
                Some(self.cart.as_ref().map_or(0, |c| c[o]))
            }
            Region::Mmio(_) | Region::VramOverrun | Region::Unmapped => {
                self.record(addr, width, AccessKind::Read, 0);
                None
            }
        }
    }

    #[inline(always)]
    fn read_le(&mut self, addr: u32, width: u8) -> u32 {
        // Fast path: the whole access inside one ROM/RAM region (the common
        // case). Identical results to the byte loop below.
        let w = width as usize;
        let fast = match self.decode(addr) {
            Region::Bios(o) => self.bios.get(o..o + w),
            Region::Ram(o) => self.ram.get(o..o + w),
            Region::Vram(o) => self.vram.get(o..o + w),
            Region::Sram(o) => self.sram.get(o..o + w),
            Region::Cart(o) => {
                if let Some(bytes) = self.cart.as_ref().and_then(|c| c.get(o..o + w)) {
                    let mut buf = [0u8; 4];
                    buf[..w].copy_from_slice(bytes);
                    self.cart_reads += w as u64;
                    self.first_cart_read.get_or_insert((self.now, addr));
                    self.last_cart_read = Some((self.now, addr + width as u32 - 1));
                    if (o >> 16) == ((o + w - 1) >> 16) {
                        self.cart_read_buckets[o >> 16] += w as u64;
                    } else {
                        for i in 0..w {
                            self.cart_read_buckets[(o + i) >> 16] += 1;
                        }
                    }
                    return u32::from_le_bytes(buf);
                }
                None
            }
            _ => None,
        };
        if let Some(bytes) = fast {
            let mut buf = [0u8; 4];
            buf[..w].copy_from_slice(bytes);
            return u32::from_le_bytes(buf);
        }
        let mut value = 0u32;
        for i in 0..width as u32 {
            match self.read_byte(addr.wrapping_add(i), width) {
                Some(b) => value |= (b as u32) << (8 * i),
                // Record a multi-byte unmapped access once, not per byte.
                None => return 0,
            }
        }
        value
    }

    #[inline(always)]
    fn write_le(&mut self, addr: u32, width: u8, value: u32) {
        // Fast path: the whole access inside RAM or VRAM and no idle-probe
        // write log. Identical results to the byte loop.
        if self.write_log.is_none() {
            let w = width as usize;
            let bytes = &value.to_le_bytes()[..w];
            match self.decode(addr) {
                Region::Ram(o) if o + w <= self.ram.len() => {
                    // (Aligned accesses never span two code pages.)
                    if self.code_pages[o >> CODE_PAGE_SHIFT] & CODE_CACHED != 0 {
                        self.code_write(o);
                    }
                    return self.ram[o..o + w].copy_from_slice(bytes);
                }
                Region::Vram(o) if o + w <= self.vram.len() => return self.vram[o..o + w].copy_from_slice(bytes),
                _ => {}
            }
        }
        self.write_le_slow(addr, width, value)
    }

    #[inline(never)]
    fn write_le_slow(&mut self, addr: u32, width: u8, value: u32) {
        let region = self.decode(addr);
        for i in 0..width as usize {
            let byte = (value >> (8 * i)) as u8;
            match region {
                Region::Vram(o) if o + i < self.vram.len() => {
                    if let Some(log) = &mut self.write_log {
                        log.push((addr.wrapping_add(i as u32), self.vram[o + i]));
                    }
                    self.vram[o + i] = byte
                }
                Region::Ram(o) if o + i < self.ram.len() => {
                    if self.code_pages[(o + i) >> CODE_PAGE_SHIFT] & CODE_CACHED != 0 {
                        self.code_write(o + i);
                    }
                    if let Some(log) = &mut self.write_log {
                        log.push((addr.wrapping_add(i as u32), self.ram[o + i]));
                    }
                    self.ram[o + i] = byte
                }
                Region::Sram(o) if o + i < self.sram.len() => self.sram[o + i] = byte,
                Region::VramOverrun => {}
                Region::Bios(_) | Region::Cart(_) => {
                    self.record(addr, width, AccessKind::RomWrite, value);
                    return;
                }
                _ => {
                    self.record(addr, width, AccessKind::Write, value);
                    return;
                }
            }
        }
    }
}

// Misaligned accesses ignore the low address bits. MAME reports this was
// tested on a Leapster 2 and that firmware depends on it; unverified on the
// original Leapster (docs/accuracy-matrix.md).
impl Bus for SystemBus {
    #[inline(always)]
    fn read8(&mut self, addr: u32) -> u8 {
        self.read_le(addr, 1) as u8
    }
    #[inline(always)]
    fn read16(&mut self, addr: u32) -> u16 {
        self.read_le(addr & !1, 2) as u16
    }
    #[inline(always)]
    fn read32(&mut self, addr: u32) -> u32 {
        self.read_le(addr & !3, 4)
    }
    #[inline(always)]
    fn write8(&mut self, addr: u32, value: u8) {
        self.write_le(addr, 1, value as u32)
    }
    #[inline(always)]
    fn write16(&mut self, addr: u32, value: u16) {
        self.write_le(addr & !1, 2, value as u32)
    }
    #[inline(always)]
    fn write32(&mut self, addr: u32, value: u32) {
        self.write_le(addr & !3, 4, value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bus_with(bios: Vec<u8>, cart: Option<Vec<u8>>) -> SystemBus {
        let fw = Firmware::from_bytes(bios).unwrap();
        let cart = cart.map(|c| Cartridge::from_bytes(c).unwrap());
        SystemBus::new(&fw, cart.as_ref())
    }

    #[test]
    fn decoded_windows_are_disjoint() {
        // `decode` tests RAM first for speed; that is only equivalent
        // because no two windows overlap.
        let w = [
            (RAM_BASE, RAM_WINDOW),
            (BIOS_BASE, BIOS_WINDOW),
            (BIOS_MIRROR, BIOS_WINDOW),
            (MMIO_BASE, MMIO_SIZE),
            (VRAM_BASE, VRAM_OVERRUN_END - VRAM_BASE),
            (CART_BASE, CART_WINDOW),
        ];
        for (n, &(a, la)) in w.iter().enumerate() {
            for &(b, lb) in &w[n + 1..] {
                assert!(a as u64 + la as u64 <= b as u64 || b as u64 + lb as u64 <= a as u64, "{a:#x} {b:#x}");
            }
        }
    }

    #[test]
    fn bios_is_mirrored_at_0x4000_0000() {
        let mut bios = vec![0u8; 0x1000];
        bios[0x10..0x14].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        let mut bus = bus_with(bios, None);
        assert_eq!(bus.read32(0x0000_0010), 0xDEAD_BEEF);
        assert_eq!(bus.read32(0x4000_0010), 0xDEAD_BEEF);
        assert_eq!(bus.read16(0x4000_0012), 0xDEAD);
        assert_eq!(bus.read8(0x4000_0010), 0xEF);
    }

    #[test]
    fn rom_writes_are_ignored_and_recorded() {
        let mut bus = bus_with(vec![0x11; 0x100], Some(vec![0x22; 0x100]));
        bus.write32(0x4000_0000, 0);
        bus.write16(0x8000_0000, 0);
        assert_eq!(bus.read32(0x4000_0000), 0x1111_1111);
        assert_eq!(bus.read16(0x8000_0000), 0x2222);
        let kinds: Vec<_> = bus.unmapped_accesses().iter().map(|a| a.kind).collect();
        assert_eq!(kinds, [AccessKind::RomWrite, AccessKind::RomWrite]);
    }

    #[test]
    fn ram_and_vram_are_little_endian_read_write() {
        let mut bus = bus_with(vec![0; 4], None);
        bus.write32(RAM_BASE + 0x100, 0x0403_0201);
        assert_eq!(bus.read8(RAM_BASE + 0x100), 0x01);
        assert_eq!(bus.read8(RAM_BASE + 0x103), 0x04);
        assert_eq!(bus.read16(RAM_BASE + 0x102), 0x0403);
        bus.write16(VRAM_BASE + VRAM_SIZE - 2, 0xBEEF);
        assert_eq!(bus.vram()[VRAM_SIZE as usize - 2..], [0xEF, 0xBE]);
        assert_eq!(bus.read32(RAM_BASE + RAM_SIZE - 4), 0);
        assert_eq!(bus.unmapped_total(), 0);
    }

    #[test]
    fn misaligned_accesses_ignore_low_bits() {
        let mut bus = bus_with(vec![0; 4], None);
        bus.write32(RAM_BASE, 0xAABB_CCDD);
        assert_eq!(bus.read32(RAM_BASE + 1), 0xAABB_CCDD);
        assert_eq!(bus.read32(RAM_BASE + 3), 0xAABB_CCDD);
        assert_eq!(bus.read16(RAM_BASE + 1), 0xCCDD);
        bus.write32(RAM_BASE + 6, 0x1122_3344);
        assert_eq!(bus.read32(RAM_BASE + 4), 0x1122_3344);
    }

    #[test]
    fn unmapped_and_out_of_range_accesses_are_recorded() {
        let mut bus = bus_with(vec![0; 0x100], Some(vec![0; 0x100]));
        assert_eq!(bus.read32(0x4000_0100), 0); // past end of BIOS image
        assert_eq!(bus.read32(0x8000_0100), 0); // past end of cart
        assert_eq!(bus.read32(RAM_BASE + RAM_SIZE), 0); // past 2 MiB
        assert_eq!(bus.read32(MMIO_BASE + 0x4c), 0); // peripherals not routed here
        bus.write32(0x1234_5678, 7);
        assert_eq!(bus.unmapped_total(), 5);
        let last = bus.unmapped_accesses().last().unwrap();
        assert_eq!((last.addr, last.kind, last.value), (0x1234_5678, AccessKind::Write, 7));
    }

    #[test]
    fn vram_overrun_writes_are_silently_discarded() {
        let mut bus = bus_with(vec![0; 4], None);
        bus.write32(VRAM_BASE + VRAM_SIZE + 0x40, 0xFFFF_FFFF);
        assert_eq!(bus.unmapped_total(), 0);
    }

    #[test]
    fn decode_regions() {
        let bus = bus_with(vec![0; 0x20_0000], Some(vec![0; 0x80_0000]));
        assert_eq!(bus.decode(0x401F_FFFF), Region::Bios(0x1F_FFFF));
        assert_eq!(bus.decode(0x4020_0000), Region::Unmapped);
        assert_eq!(bus.decode(0x0180_004C), Region::Mmio(0x4C));
        assert_eq!(bus.decode(0x3C1F_FFFF), Region::Ram(0x1F_FFFF));
        assert_eq!(bus.decode(0x807F_FFFF), Region::Cart(0x7F_FFFF));
        assert_eq!(bus.decode(0x8080_0000), Region::Unmapped);
    }
}
