//! ELF32 little-endian ARM shared-object loader (Android NDK `armeabi-v7a` .so files).
//!
//! Maps the PT_LOAD segments into one flat image at a chosen base, applies the dynamic
//! relocations Android's linker would (REL only; ARM .so files don't use RELA), and binds
//! every undefined symbol to an address handed out by the caller. Those addresses are where
//! an HLE puts its stand-ins for bionic libc, GLES 1.1, EGL, OpenSL ES, libandroid and liblog.

use std::collections::BTreeMap;

const PT_LOAD: u32 = 1;
const PT_DYNAMIC: u32 = 2;

const R_ARM_ABS32: u32 = 2;
const R_ARM_GLOB_DAT: u32 = 21;
const R_ARM_JUMP_SLOT: u32 = 22;
const R_ARM_RELATIVE: u32 = 23;

#[derive(Clone, Debug)]
pub struct Symbol {
    pub name: String,
    pub value: u32,
    pub size: u32,
    pub kind: u8,
    pub defined: bool,
}

#[derive(Debug, Default)]
pub struct RelocStats {
    pub relative: usize,
    pub abs32: usize,
    pub glob_dat: usize,
    pub jump_slot: usize,
    pub other: BTreeMap<u32, usize>,
    pub unbound: usize,
}

pub struct Loaded {
    /// Load address of virtual address 0 of the file.
    pub base: u32,
    /// Flat image, `image[0]` is at `base`.
    pub image: Vec<u8>,
    pub needed: Vec<String>,
    pub soname: Option<String>,
    pub symbols: Vec<Symbol>,
    /// Imports (undefined symbols that something references) and the address each was bound to.
    pub imports: BTreeMap<String, u32>,
    /// Defined function and object symbols, at their loaded address (bit 0 set for Thumb).
    pub exports: BTreeMap<String, u32>,
    /// DT_INIT then DT_INIT_ARRAY entries, loaded addresses, in call order.
    pub init: Vec<u32>,
    pub relocs: RelocStats,
    /// Raw `.ARM.attributes` (aeabi) bytes, if present.
    pub arm_attributes: Option<Vec<u8>>,
}

fn r16(d: &[u8], o: usize) -> Result<u16, String> {
    d.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(|| format!("read past end at {o:#x}"))
}
fn r32(d: &[u8], o: usize) -> Result<u32, String> {
    d.get(o..o + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(|| format!("read past end at {o:#x}"))
}
fn cstr(d: &[u8], o: usize) -> String {
    let end = d[o.min(d.len())..].iter().position(|&b| b == 0).map_or(d.len(), |p| o + p);
    String::from_utf8_lossy(&d[o.min(d.len())..end]).into_owned()
}

impl Loaded {
    pub fn read32(&self, va: u32) -> Option<u32> {
        let o = va.checked_sub(self.base)? as usize;
        r32(&self.image, o).ok()
    }
    fn write32(&mut self, va: u32, v: u32) -> bool {
        let Some(o) = va.checked_sub(self.base).map(|o| o as usize) else { return false };
        match self.image.get_mut(o..o + 4) {
            Some(s) => {
                s.copy_from_slice(&v.to_le_bytes());
                true
            }
            None => false,
        }
    }
}

/// Loads `file` at `base`. `bind` is called once per distinct undefined symbol name and returns
/// the address to bind it to (None leaves it at 0 and counts it as unbound).
pub fn load(file: &[u8], base: u32, mut bind: impl FnMut(&str) -> Option<u32>) -> Result<Loaded, String> {
    if file.get(0..4) != Some(b"\x7fELF") {
        return Err("not an ELF file".into());
    }
    if file[4] != 1 || file[5] != 1 {
        return Err("not ELF32 little-endian".into());
    }
    if r16(file, 16)? != 3 {
        return Err("not a shared object (ET_DYN)".into());
    }
    if r16(file, 18)? != 40 {
        return Err("not an ARM ELF".into());
    }
    let phoff = r32(file, 28)? as usize;
    let shoff = r32(file, 32)? as usize;
    let phentsize = r16(file, 42)? as usize;
    let phnum = r16(file, 44)? as usize;
    let shentsize = r16(file, 46)? as usize;
    let shnum = r16(file, 48)? as usize;
    let shstrndx = r16(file, 50)? as usize;

    // Segments.
    let mut loads = Vec::new();
    let mut dynamic = None;
    for i in 0..phnum {
        let p = phoff + i * phentsize;
        let (ty, off, vaddr, filesz, memsz) = (r32(file, p)?, r32(file, p + 4)?, r32(file, p + 8)?, r32(file, p + 16)?, r32(file, p + 20)?);
        if ty == PT_LOAD {
            loads.push((off, vaddr, filesz, memsz));
        } else if ty == PT_DYNAMIC {
            dynamic = Some(vaddr);
        }
    }
    let top = loads.iter().map(|&(_, v, _, m)| v as u64 + m as u64).max().ok_or("no PT_LOAD segments")?;
    let size = ((top + 0xFFF) & !0xFFF) as usize;
    let mut image = vec![0u8; size];
    for &(off, vaddr, filesz, _) in &loads {
        let src = file.get(off as usize..off as usize + filesz as usize).ok_or("segment past end of file")?;
        image[vaddr as usize..vaddr as usize + filesz as usize].copy_from_slice(src);
    }

    // Dynamic section (read from the mapped image, by virtual address).
    let dynamic = dynamic.ok_or("no PT_DYNAMIC")? as usize;
    let mut tags: Vec<(i32, u32)> = Vec::new();
    let mut o = dynamic;
    loop {
        let tag = r32(&image, o)? as i32;
        let val = r32(&image, o + 4)?;
        if tag == 0 {
            break;
        }
        tags.push((tag, val));
        o += 8;
    }
    let get = |t: i32| tags.iter().find(|&&(k, _)| k == t).map(|&(_, v)| v);
    let strtab = get(5).ok_or("no DT_STRTAB")? as usize;
    let symtab = get(6).ok_or("no DT_SYMTAB")? as usize;

    // Symbol count: DT_HASH nchain, else the .dynsym section size.
    let nsyms = if let Some(h) = get(4) {
        r32(&image, h as usize + 4)? as usize
    } else {
        let mut n = 0;
        for i in 0..shnum {
            let s = shoff + i * shentsize;
            if r32(file, s + 4)? == 11 {
                n = r32(file, s + 20)? as usize / 16;
            }
        }
        n
    };
    let mut symbols = Vec::with_capacity(nsyms);
    for i in 0..nsyms {
        let s = symtab + i * 16;
        let name = cstr(&image, strtab + r32(&image, s)? as usize);
        let value = r32(&image, s + 4)?;
        let size = r32(&image, s + 8)?;
        let info = image[s + 12];
        let shndx = r16(&image, s + 14)?;
        symbols.push(Symbol { name, value, size, kind: info & 0xF, defined: shndx != 0 });
    }

    let needed = tags.iter().filter(|&&(k, _)| k == 1).map(|&(_, v)| cstr(&image, strtab + v as usize)).collect();
    let soname = get(14).map(|v| cstr(&image, strtab + v as usize));

    let mut l = Loaded {
        base,
        image,
        needed,
        soname,
        symbols,
        imports: BTreeMap::new(),
        exports: BTreeMap::new(),
        init: Vec::new(),
        relocs: RelocStats::default(),
        arm_attributes: None,
    };
    for s in &l.symbols {
        if s.defined && !s.name.is_empty() && (s.kind == 1 || s.kind == 2) {
            l.exports.insert(s.name.clone(), base.wrapping_add(s.value));
        }
    }

    // Relocations: DT_REL and DT_JMPREL (both REL on ARM).
    let mut tables = Vec::new();
    if let (Some(a), Some(n)) = (get(17), get(18)) {
        tables.push((a, n));
    }
    if let (Some(a), Some(n)) = (get(23), get(2)) {
        tables.push((a, n));
    }
    for (addr, bytes) in tables {
        for k in 0..bytes as usize / 8 {
            let r = addr as usize + k * 8;
            let off = r32(&l.image, r)?;
            let info = r32(&l.image, r + 4)?;
            let (ty, si) = (info & 0xFF, (info >> 8) as usize);
            let place = base.wrapping_add(off);
            let sym_addr = |l: &mut Loaded, bind: &mut dyn FnMut(&str) -> Option<u32>| -> Option<u32> {
                let s = l.symbols.get(si)?.clone();
                if s.defined {
                    return Some(base.wrapping_add(s.value));
                }
                if let Some(&a) = l.imports.get(&s.name) {
                    return Some(a);
                }
                let a = bind(&s.name);
                l.imports.insert(s.name.clone(), a.unwrap_or(0));
                a
            };
            match ty {
                R_ARM_RELATIVE => {
                    let a = l.read32(place).unwrap_or(0);
                    l.write32(place, a.wrapping_add(base));
                    l.relocs.relative += 1;
                }
                R_ARM_ABS32 => {
                    let a = l.read32(place).unwrap_or(0);
                    match sym_addr(&mut l, &mut bind) {
                        Some(s) => {
                            l.write32(place, s.wrapping_add(a));
                        }
                        None => l.relocs.unbound += 1,
                    }
                    l.relocs.abs32 += 1;
                }
                R_ARM_GLOB_DAT | R_ARM_JUMP_SLOT => {
                    match sym_addr(&mut l, &mut bind) {
                        Some(s) => {
                            l.write32(place, s);
                        }
                        None => l.relocs.unbound += 1,
                    }
                    if ty == R_ARM_GLOB_DAT { l.relocs.glob_dat += 1 } else { l.relocs.jump_slot += 1 }
                }
                0 => {}
                t => *l.relocs.other.entry(t).or_default() += 1,
            }
        }
    }

    // Constructors, after relocation (init_array entries are relocated pointers).
    if let Some(i) = get(12) {
        l.init.push(base.wrapping_add(i));
    }
    if let (Some(a), Some(n)) = (get(25), get(27)) {
        for k in 0..n / 4 {
            if let Some(f) = l.read32(base.wrapping_add(a + k * 4)) {
                if f != 0 && f != u32::MAX {
                    l.init.push(f);
                }
            }
        }
    }

    // .ARM.attributes from the section table (not loaded, so read from the file).
    if shoff != 0 && shstrndx < shnum {
        let strsec = shoff + shstrndx * shentsize;
        let stroff = r32(file, strsec + 16)? as usize;
        for i in 0..shnum {
            let s = shoff + i * shentsize;
            if cstr(file, stroff + r32(file, s)? as usize) == ".ARM.attributes" {
                let (o, n) = (r32(file, s + 16)? as usize, r32(file, s + 20)? as usize);
                l.arm_attributes = file.get(o..o + n).map(|b| b.to_vec());
            }
        }
    }
    Ok(l)
}

/// Decodes the few aeabi build attributes that decide which CPU features an HLE must provide.
pub fn describe_attributes(a: &[u8]) -> Vec<String> {
    let mut out = Vec::new();
    // Format: 'A', u32 len, "aeabi\0", then subsections: tag 1 (File), u32 len, attributes.
    let Some(p) = a.windows(6).position(|w| w == b"aeabi\0") else { return out };
    let mut i = p + 6;
    if a.get(i) != Some(&1) {
        return out;
    }
    i += 5;
    let uleb = |i: &mut usize| -> u64 {
        let (mut v, mut sh) = (0u64, 0);
        while let Some(&b) = a.get(*i) {
            *i += 1;
            v |= ((b & 0x7F) as u64) << sh;
            sh += 7;
            if b & 0x80 == 0 {
                break;
            }
        }
        v
    };
    while i < a.len() {
        let tag = uleb(&mut i);
        if tag == 4 || tag == 5 || tag == 67 || tag == 65 || tag == 32 {
            let s = cstr(a, i);
            i += s.len() + 1;
            if tag == 5 {
                out.push(format!("CPU name: {s}"));
            }
            continue;
        }
        let v = uleb(&mut i);
        let text = match (tag, v) {
            (6, 10) => "architecture: ARMv7".to_string(),
            (6, v) => format!("architecture code {v}"),
            (7, 0x41) => "profile: A".to_string(),
            (8, 1) => "ARM instructions: yes".to_string(),
            (9, 2) => "Thumb: Thumb-2".to_string(),
            (9, v) => format!("Thumb: {v}"),
            (10, 3) => "FP: VFPv3".to_string(),
            (10, 4) => "FP: VFPv3-D16".to_string(),
            (10, v) => format!("FP code {v}"),
            (12, 1) => "Advanced SIMD (NEON): v1".to_string(),
            (12, v) => format!("Advanced SIMD code {v}"),
            (28, 1) => "float ABI: hard (VFP registers)".to_string(),
            (28, 0) => "float ABI: soft (core registers)".to_string(),
            _ => continue,
        };
        out.push(text);
    }
    out
}
