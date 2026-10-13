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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Region {
    Bios(usize),
    Mmio(u32),
    Vram(usize),
    /// Discarded writes past VRAM (see `map::VRAM_OVERRUN_END`).
    VramOverrun,
    Ram(usize),
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
}

pub struct SystemBus {
    bios: Vec<u8>,
    cart: Option<Vec<u8>>,
    ram: Vec<u8>,
    vram: Vec<u8>,
    unmapped: Vec<UnmappedAccess>,
    unmapped_total: u64,
    unmapped_summary: std::collections::BTreeMap<(u32, u8, AccessKind), UnmappedStat>,
    /// Time stamp for diagnostics (the machine sets the CPU cycle count).
    pub now: u64,
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
}

impl SystemBus {
    pub fn new(firmware: &Firmware, cart: Option<&Cartridge>) -> Self {
        Self {
            bios: firmware.data().to_vec(),
            cart: cart.map(|c| c.rom().to_vec()),
            // Power-on SDRAM contents are undefined; zero keeps runs
            // deterministic. HYPOTHESIS: firmware does not depend on them.
            ram: vec![0; RAM_SIZE as usize],
            vram: vec![0; VRAM_SIZE as usize],
            unmapped: Vec::new(),
            unmapped_total: 0,
            unmapped_summary: Default::default(),
            now: 0,
            cart_reads: 0,
            first_cart_read: None,
            cart_read_buckets: vec![0; cart.map_or(0, |c| c.rom().len().div_ceil(0x1_0000))],
            last_cart_read: None,
            write_log: None,
        }
    }

    #[inline(always)]
    pub fn decode(&self, addr: u32) -> Region {
        let in_window = |base: u32, size: u32| addr.wrapping_sub(base) < size;
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
        if in_window(RAM_BASE, RAM_SIZE) {
            return Region::Ram((addr - RAM_BASE) as usize);
        }
        if let Some(cart) = &self.cart
            && in_window(CART_BASE, cart.len() as u32)
        {
            return Region::Cart((addr - CART_BASE) as usize);
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

    /// Restores RAM and VRAM (save states); false if the sizes differ.
    pub fn restore_memory(&mut self, ram: &[u8], vram: &[u8]) -> bool {
        if ram.len() != self.ram.len() || vram.len() != self.vram.len() {
            return false;
        }
        self.ram.copy_from_slice(ram);
        self.vram.copy_from_slice(vram);
        true
    }

    /// RAM/VRAM/ROM byte without side effects (no logging); None elsewhere.
    pub fn peek8(&self, addr: u32) -> Option<u8> {
        match self.decode(addr) {
            Region::Bios(o) => self.bios.get(o).copied(),
            Region::Ram(o) => self.ram.get(o).copied(),
            Region::Vram(o) => self.vram.get(o).copied(),
            Region::Cart(o) => self.cart.as_ref().and_then(|c| c.get(o).copied()),
            _ => None,
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
        let now = self.now;
        self.unmapped_summary
            .entry((addr, width, kind))
            .or_insert(UnmappedStat { count: 0, first_cycle: now })
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
            Region::Cart(o) => {
                if let Some(bytes) = self.cart.as_ref().and_then(|c| c.get(o..o + w)) {
                    let mut buf = [0u8; 4];
                    buf[..w].copy_from_slice(bytes);
                    self.cart_reads += w as u64;
                    self.first_cart_read.get_or_insert((self.now, addr));
                    self.last_cart_read = Some((self.now, addr + width as u32 - 1));
                    for i in 0..w {
                        self.cart_read_buckets[(o + i) >> 16] += 1;
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
                Region::Ram(o) if o + w <= self.ram.len() => return self.ram[o..o + w].copy_from_slice(bytes),
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
                    if let Some(log) = &mut self.write_log {
                        log.push((addr.wrapping_add(i as u32), self.ram[o + i]));
                    }
                    self.ram[o + i] = byte
                }
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
