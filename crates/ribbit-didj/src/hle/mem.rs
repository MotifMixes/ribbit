//! A program's address space for the HLE, on the Didj machine's own SDRAM.
//! Ribbit is the kernel: it writes each process's ARM page tables (a
//! level-1 table, coarse level-2 tables, 4 KiB small pages) into a reserved
//! part of SDRAM and takes user pages from the rest, outside the frame
//! buffers, so the JIT and the device models see the same memory the
//! program does. The processor runs with its MMU on (domain 0 manager: no
//! permission checks); an access to an unmapped page aborts, which the run
//! loop stops on, as Linux would send SIGSEGV. Ribbit's own accesses (system
//! call arguments) go through `Space`, which uses the same map.

use crate::machine::Bus;
use ribbit_arm::ArmBus;
use std::collections::HashMap;

pub const PAGE: u32 = 4096;
/// SDRAM the HLE machine has (Enhanced's 64 MB).
pub const SDRAM: usize = 64 << 20;
/// Level-1 tables (16 KiB, 16 KiB-aligned), one per process, from address 0.
const L1_TABLES: u32 = 60;
/// The level-2 table for the top megabyte (just the vector page), shared
/// by every process, and the vector page itself (the user helpers' page).
pub const VECTOR_L2: u32 = 0x000F_0000;
pub const VECTOR_FRAME: u32 = 0x000F_1000;
/// User pages: from 1 MB up, but not the frame buffers (FB_BASE_ADDR
/// 0x0120_0000 to 32 MB: RGB, YUV and OpenGL ES planes,
/// include/asm-arm/arch-lf1000/mlc.h).
const POOL: [(u32, u32); 2] = [(0x0010_0000, 0x0120_0000), (0x0200_0000, SDRAM as u32)];
/// Coarse table entry (domain 0; bit 4 set as the ARM926 wants).
const L1_COARSE: u32 = 0x11;
/// Small page entry, AP 11 for all four subpages, not cached.
const L2_SMALL: u32 = 0xFF2;

pub fn page_up(v: u32) -> u32 {
    v.wrapping_add(PAGE - 1) & !(PAGE - 1)
}

/// The free physical memory, shared by every process.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Frames {
    free: Vec<u32>,
    l1_free: Vec<u32>,
    /// 1 KiB level-2 table slots (four to a frame).
    l2_free: Vec<u32>,
    /// Where user pages come from (`POOL` on the Didj).
    pool: [(u32, u32); 2],
    /// Pages shared by content (read-only file mappings, `map_shared`):
    /// how many maps hold each, and the frames by their contents' CRC.
    #[serde(default)]
    shared: HashMap<u32, u32>,
    #[serde(default)]
    by_crc: HashMap<u32, Vec<u32>>,
}

impl Frames {
    /// Lays out SDRAM: the vector page with the user helpers' version word
    /// at 0xFFFF0FFC, mapped by the shared top-megabyte table.
    pub fn new(bus: &mut Bus, kuser_version: u32) -> Self {
        Self::with_pool(bus, kuser_version, POOL)
    }

    /// `new` with user pages from `pool` (two ranges; the Explorer's frame
    /// buffers are bigger and its SDRAM larger).
    pub fn with_pool(bus: &mut Bus, kuser_version: u32, pool: [(u32, u32); 2]) -> Self {
        let ram = &mut bus.sdram;
        ram[..POOL[0].0 as usize].fill(0);
        put(ram, VECTOR_L2 + (0xF0 << 2), VECTOR_FRAME | L2_SMALL);
        put(ram, VECTOR_FRAME + 0xFFC, kuser_version);
        let free = pool.iter().flat_map(|&(from, to)| (from..to).step_by(PAGE as usize)).rev().collect();
        Self { free, l1_free: (0..L1_TABLES).rev().map(|i| i * 0x4000).collect(), l2_free: Vec::new(), pool, shared: HashMap::new(), by_crc: HashMap::new() }
    }

    pub fn free_pages(&self) -> usize {
        self.free.len()
    }

    /// A map lets go of frame `f`: back to the pool unless it is device
    /// memory or still shared.
    fn put_back(&mut self, f: u32, sdram: &[u8]) {
        if let Some(n) = self.shared.get_mut(&f) {
            *n -= 1;
            if *n > 0 {
                return;
            }
            self.shared.remove(&f);
            let crc = crc32fast::hash(&sdram[f as usize..(f + PAGE) as usize]);
            if let Some(v) = self.by_crc.get_mut(&crc) {
                v.retain(|&x| x != f);
            }
        }
        if pooled(&self.pool, f) {
            self.free.push(f);
        }
    }
}

/// A frame from the pool (not device memory mapped in).
fn pooled(pool: &[(u32, u32); 2], f: u32) -> bool {
    pool.iter().any(|&(from, to)| (from..to).contains(&f))
}

fn put(ram: &mut [u8], pa: u32, v: u32) {
    ram[pa as usize..pa as usize + 4].copy_from_slice(&v.to_le_bytes());
}

/// One process's map. Its tables live in SDRAM; `Space` changes them.
#[derive(Default, serde::Serialize, serde::Deserialize)]
pub struct Mem {
    /// The level-1 table's physical address (TTB).
    pub l1: u32,
    /// Virtual page number → physical frame.
    pages: HashMap<u32, u32>,
    /// Level-2 tables by megabyte.
    l2: HashMap<u32, u32>,
    /// The tables changed since the processor's TLB was last flushed.
    pub dirty: bool,
    /// Pages reserved but not yet given memory (`reserve`): zero pages
    /// made on first touch, as the kernel's demand paging.
    #[serde(default)]
    lazy: std::collections::HashSet<u32>,
    /// It has its tables (not released).
    live: bool,
}

/// A process's map with the memory it lives in, for changing or reading it.
pub struct Space<'a> {
    pub mem: &'a mut Mem,
    pub bus: &'a mut Bus,
    pub frames: &'a mut Frames,
}

impl Space<'_> {
    /// A fresh address space (just the vector page); None if there are
    /// too many processes.
    pub fn fresh(bus: &mut Bus, frames: &mut Frames) -> Option<Mem> {
        let l1 = frames.l1_free.pop()?;
        bus.sdram[l1 as usize..l1 as usize + 0x4000].fill(0);
        put(&mut bus.sdram, l1 + (0xFFF << 2), VECTOR_L2 | L1_COARSE);
        Some(Mem { l1, dirty: true, live: true, ..Default::default() })
    }

    pub fn mapped(&self, addr: u32) -> bool {
        self.mem.pages.contains_key(&(addr / PAGE)) || self.mem.lazy.contains(&(addr / PAGE))
    }

    /// Reserves `addr..addr + len` as zero pages given memory on first
    /// touch (`populate`); pages already there stay.
    pub fn reserve(&mut self, addr: u32, len: u32) {
        let (first, last) = (addr / PAGE, (addr as u64 + len as u64).div_ceil(PAGE as u64).min(0xFFF00) as u32);
        for vpn in first..last {
            if !self.mem.pages.contains_key(&vpn) {
                self.mem.lazy.insert(vpn);
            }
        }
    }

    /// Gives a reserved page at `addr` its memory; false if it isn't
    /// reserved (a real fault) or memory ran out.
    pub fn populate(&mut self, addr: u32) -> bool {
        let vpn = addr / PAGE;
        if !self.mem.lazy.remove(&vpn) {
            return false;
        }
        self.map(vpn * PAGE, PAGE)
    }

    /// Maps the page at `addr` read-only in spirit with `contents`, sharing
    /// a frame with every other map of the same contents (libraries' code
    /// and read-only data, mapped by every process). The program must not
    /// write it: `privatize` first (mprotect adding PROT_WRITE).
    pub fn map_shared(&mut self, addr: u32, contents: &[u8]) -> bool {
        let vpn = addr / PAGE;
        let mut page = [0u8; PAGE as usize];
        page[..contents.len().min(PAGE as usize)].copy_from_slice(&contents[..contents.len().min(PAGE as usize)]);
        if let Some(f) = self.mem.pages.remove(&vpn) {
            self.set_entry(vpn, 0);
            self.frames.put_back(f, &self.bus.sdram);
        }
        self.mem.lazy.remove(&vpn);
        let crc = crc32fast::hash(&page);
        let found = self.frames.by_crc.get(&crc).and_then(|v| v.iter().copied().find(|&f| self.bus.sdram[f as usize..(f + PAGE) as usize] == page));
        let f = match found {
            Some(f) => {
                *self.frames.shared.get_mut(&f).unwrap() += 1;
                f
            }
            None => {
                let Some(f) = self.frames.free.pop() else { return false };
                self.bus.sdram[f as usize..(f + PAGE) as usize].copy_from_slice(&page);
                self.wrote(f, PAGE);
                self.frames.shared.insert(f, 1);
                self.frames.by_crc.entry(crc).or_default().push(f);
                f
            }
        };
        if !self.set_entry(vpn, f | L2_SMALL) {
            self.frames.put_back(f, &self.bus.sdram);
            return false;
        }
        self.mem.pages.insert(vpn, f);
        true
    }

    /// Gives the shared pages in `addr..addr + len` copies of their own
    /// (they are about to be written).
    pub fn privatize(&mut self, addr: u32, len: u32) -> bool {
        let (first, last) = (addr / PAGE, (addr as u64 + len as u64).div_ceil(PAGE as u64).min(0xFFF00) as u32);
        for vpn in first..last {
            let Some(&f) = self.mem.pages.get(&vpn) else { continue };
            if !self.frames.shared.contains_key(&f) {
                continue;
            }
            let Some(to) = self.frames.free.pop() else { return false };
            self.bus.sdram.copy_within(f as usize..(f + PAGE) as usize, to as usize);
            self.wrote(to, PAGE);
            self.set_entry(vpn, to | L2_SMALL);
            self.mem.pages.insert(vpn, to);
            self.frames.put_back(f, &self.bus.sdram);
        }
        true
    }

    /// The physical address of `addr`, giving a reserved page its memory.
    fn phys_mut(&mut self, addr: u32) -> Option<u32> {
        if self.mem.lazy.contains(&(addr / PAGE)) {
            self.populate(addr);
        }
        self.phys(addr)
    }

    fn set_entry(&mut self, vpn: u32, entry: u32) -> bool {
        let mb = vpn >> 8;
        let table = match self.mem.l2.get(&mb) {
            Some(&t) => t,
            None => {
                if entry == 0 {
                    return true;
                }
                if self.frames.l2_free.is_empty() {
                    let Some(f) = self.frames.free.pop() else { return false };
                    self.frames.l2_free.extend((0..4).rev().map(|i| f + i * 0x400));
                }
                let t = self.frames.l2_free.pop().unwrap();
                self.bus.sdram[t as usize..t as usize + 0x400].fill(0);
                put(&mut self.bus.sdram, self.mem.l1 + (mb << 2), t | L1_COARSE);
                self.mem.l2.insert(mb, t);
                t
            }
        };
        put(&mut self.bus.sdram, table + ((vpn & 0xFF) << 2), entry);
        self.mem.dirty = true;
        true
    }

    /// Maps the pages covering `addr..addr + len`, zero-filled; pages
    /// already there keep their contents. False if memory ran out.
    pub fn map(&mut self, addr: u32, len: u32) -> bool {
        let (first, last) = (addr / PAGE, (addr as u64 + len as u64).div_ceil(PAGE as u64).min(0xFFF00) as u32);
        for vpn in first..last {
            if self.mem.pages.contains_key(&vpn) {
                continue;
            }
            let Some(f) = self.frames.free.pop() else { return false };
            self.bus.sdram[f as usize..(f + PAGE) as usize].fill(0);
            self.wrote(f, PAGE);
            if !self.set_entry(vpn, f | L2_SMALL) {
                self.frames.free.push(f);
                return false;
            }
            self.mem.pages.insert(vpn, f);
        }
        true
    }

    /// Maps `addr..addr + len` onto physical memory from `pa` (a device's
    /// mmap: the frame buffers, io_remap_pfn_range). False if out of memory
    /// for the tables.
    pub fn map_phys(&mut self, addr: u32, len: u32, pa: u32) -> bool {
        self.unmap(addr, len);
        for i in 0..page_up(len) / PAGE {
            let (vpn, f) = (addr / PAGE + i, pa + i * PAGE);
            if !self.set_entry(vpn, f | L2_SMALL) {
                return false;
            }
            self.mem.pages.insert(vpn, f);
        }
        true
    }

    pub fn unmap(&mut self, addr: u32, len: u32) {
        let (first, last) = (addr / PAGE, (addr as u64 + len as u64).div_ceil(PAGE as u64).min(0xFFF00) as u32);
        for vpn in first..last {
            self.mem.lazy.remove(&vpn);
            if let Some(f) = self.mem.pages.remove(&vpn) {
                self.set_entry(vpn, 0);
                self.frames.put_back(f, &self.bus.sdram);
            }
        }
    }

    /// Gives back every page and table (the process ended or ran a new
    /// program).
    pub fn release(&mut self) {
        for (_, f) in std::mem::take(&mut self.mem.pages) {
            self.frames.put_back(f, &self.bus.sdram);
        }
        // Level-2 slots go back as slots (their frames stay split).
        self.frames.l2_free.extend(self.mem.l2.drain().map(|(_, t)| t));
        if self.mem.live {
            self.frames.l1_free.push(self.mem.l1);
        }
        *self.mem = Mem::default();
    }

    /// A copy of this address space (fork); None if memory ran out.
    pub fn fork(&mut self) -> Option<Mem> {
        let mut child = Space::fresh(self.bus, self.frames)?;
        let mut pages: Vec<(u32, u32)> = self.mem.pages.iter().map(|(&v, &f)| (v, f)).collect();
        pages.sort_unstable();
        child.lazy = self.mem.lazy.clone();
        let mut s = Space { mem: &mut child, bus: self.bus, frames: self.frames };
        for (vpn, from) in pages {
            // Shared pages stay shared.
            if let Some(n) = s.frames.shared.get_mut(&from) {
                *n += 1;
                if !s.set_entry(vpn, from | L2_SMALL) {
                    s.release();
                    return None;
                }
                s.mem.pages.insert(vpn, from);
                continue;
            }
            // Device memory is shared, not copied.
            if !pooled(&s.frames.pool, from) {
                if !s.map_phys(vpn * PAGE, PAGE, from) {
                    s.release();
                    return None;
                }
                continue;
            }
            if !s.map(vpn * PAGE, PAGE) {
                s.release();
                return None;
            }
            let to = s.mem.pages[&vpn] as usize;
            s.bus.sdram.copy_within(from as usize..(from + PAGE) as usize, to);
        }
        Some(child)
    }

    /// The lowest `len` bytes of unmapped pages at or above `from` and below
    /// `limit`, page-aligned.
    pub fn find_free(&self, from: u32, len: u32, limit: u32) -> Option<u32> {
        let need = page_up(len) / PAGE;
        let (mut start, mut run) = (from / PAGE, 0);
        for p in from / PAGE..limit / PAGE {
            if self.mem.pages.contains_key(&p) || self.mem.lazy.contains(&p) {
                (start, run) = (p + 1, 0);
            } else {
                run += 1;
                if run == need {
                    return Some(start * PAGE);
                }
            }
        }
        None
    }

    /// The physical address of `addr`, if mapped.
    pub fn phys(&self, addr: u32) -> Option<u32> {
        // Device registers mapped in are the Bus's, not memory Rust reads.
        self.mem.pages.get(&(addr / PAGE)).map(|f| f + addr % PAGE).filter(|&pa| (pa as usize) < self.bus.sdram.len())
    }

    /// Tells the JIT about writes to `len` bytes at `pa`.
    fn wrote(&mut self, pa: u32, len: u32) {
        if let Some(w) = self.bus.code_watch() {
            for kb in (pa >> 10)..=((pa + len - 1) >> 10) {
                w.wrote((kb << 10) as usize);
            }
        }
    }

    /// Copies out guest memory; None if any of it is unmapped.
    pub fn read_bytes(&mut self, addr: u32, len: usize) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(len);
        let mut a = addr;
        while out.len() < len {
            let n = (PAGE - a % PAGE).min((len - out.len()) as u32);
            let pa = self.phys_mut(a)? as usize;
            out.extend_from_slice(&self.bus.sdram[pa..pa + n as usize]);
            a = a.wrapping_add(n);
        }
        Some(out)
    }

    /// Copies into guest memory; false (and nothing past the hole written)
    /// if any of it is unmapped.
    pub fn write_bytes(&mut self, addr: u32, data: &[u8]) -> bool {
        let mut a = addr;
        let mut rest = data;
        while !rest.is_empty() {
            let n = (PAGE - a % PAGE).min(rest.len() as u32);
            let Some(pa) = self.phys_mut(a) else { return false };
            self.bus.sdram[pa as usize..(pa + n) as usize].copy_from_slice(&rest[..n as usize]);
            self.wrote(pa, n);
            rest = &rest[n as usize..];
            a = a.wrapping_add(n);
        }
        true
    }

    /// A NUL-terminated string (at most 4 KiB); None if unmapped or too long.
    pub fn cstr(&mut self, addr: u32) -> Option<String> {
        let mut out = Vec::new();
        for i in 0..4096u32 {
            let pa = self.phys_mut(addr.wrapping_add(i))? as usize;
            let b = self.bus.sdram[pa];
            if b == 0 {
                return Some(String::from_utf8_lossy(&out).into_owned());
            }
            out.push(b);
        }
        None
    }

    pub fn put32(&mut self, addr: u32, v: u32) -> bool {
        self.write_bytes(addr, &v.to_le_bytes())
    }

    pub fn get32(&mut self, addr: u32) -> Option<u32> {
        Some(u32::from_le_bytes(self.read_bytes(addr, 4)?.try_into().unwrap()))
    }
}
