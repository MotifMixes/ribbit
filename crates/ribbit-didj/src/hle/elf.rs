//! Loads an ELF32 ARM program or shared object into an address space, as
//! the kernel's binfmt_elf does: PT_LOAD segments mapped and copied, the
//! rest of each segment zeroed, the interpreter's name taken from PT_INTERP.

use super::mem::{Space, page_up};

const ET_EXEC: u16 = 2;
const ET_DYN: u16 = 3;
const EM_ARM: u16 = 40;
const PT_LOAD: u32 = 1;
const PT_INTERP: u32 = 3;
const PT_PHDR: u32 = 6;
/// e_flags EF_ARM_EABIMASK.
const EABI_MASK: u32 = 0xFF00_0000;

pub struct Loaded {
    pub entry: u32,
    /// Where the program headers are in memory (AT_PHDR), and how many.
    pub phdr: u32,
    pub phnum: u32,
    /// What was added to the file's addresses (0 for ET_EXEC).
    pub bias: u32,
    /// The end of the highest segment (the program's first brk).
    pub end: u32,
    pub interp: Option<String>,
    /// Built for the ARM EABI (else the old ABI).
    pub eabi: bool,
}

fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(d.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(d.get(at..at + 4)?.try_into().ok()?))
}

/// The program's interpreter (PT_INTERP), read before loading it.
pub fn interp(data: &[u8]) -> Result<Option<String>, String> {
    let bad = || "truncated ELF file".to_string();
    let phoff = u32_at(data, 28).ok_or_else(bad)? as usize;
    let phentsize = u16_at(data, 42).ok_or_else(bad)? as usize;
    for i in 0..u16_at(data, 44).ok_or_else(bad)? as usize {
        let h = phoff + i * phentsize;
        if u32_at(data, h) == Some(PT_INTERP) {
            let (offset, filesz) = (u32_at(data, h + 4).ok_or_else(bad)?, u32_at(data, h + 16).ok_or_else(bad)?);
            let file = data.get(offset as usize..(offset + filesz) as usize).ok_or_else(bad)?;
            return Ok(Some(String::from_utf8_lossy(file).trim_end_matches('\0').to_string()));
        }
    }
    Ok(None)
}

/// Loads `data`; a shared object (ET_DYN) is placed at `dyn_base`.
pub fn load(mem: &mut Space, data: &[u8], dyn_base: u32) -> Result<Loaded, String> {
    if data.get(..4) != Some(b"\x7fELF") || data.get(4) != Some(&1) || data.get(5) != Some(&1) {
        return Err("not a 32-bit little-endian ELF file".into());
    }
    let bad = || "truncated ELF file".to_string();
    let kind = u16_at(data, 16).ok_or_else(bad)?;
    if u16_at(data, 18) != Some(EM_ARM) {
        return Err("not an ARM program".into());
    }
    let bias = match kind {
        ET_EXEC => 0,
        ET_DYN => dyn_base,
        _ => return Err(format!("ELF type {kind} can't be run")),
    };
    let entry = u32_at(data, 24).ok_or_else(bad)?;
    let phoff = u32_at(data, 28).ok_or_else(bad)? as usize;
    let flags = u32_at(data, 36).ok_or_else(bad)?;
    let phentsize = u16_at(data, 42).ok_or_else(bad)? as usize;
    let phnum = u16_at(data, 44).ok_or_else(bad)? as usize;
    let mut out = Loaded {
        entry: entry.wrapping_add(bias),
        phdr: 0,
        phnum: phnum as u32,
        bias,
        end: 0,
        interp: None,
        eabi: flags & EABI_MASK != 0,
    };
    for i in 0..phnum {
        let h = phoff + i * phentsize;
        let field = |n: usize| u32_at(data, h + 4 * n).ok_or_else(bad);
        let (kind, offset, vaddr, filesz, memsz) = (field(0)?, field(1)?, field(2)?, field(4)?, field(5)?);
        let file = data.get(offset as usize..(offset + filesz) as usize).ok_or_else(bad)?;
        match kind {
            PT_LOAD => {
                let at = vaddr.wrapping_add(bias);
                if !mem.map(at, memsz.max(filesz)) {
                    return Err("out of memory".into());
                }
                mem.write_bytes(at, file);
                if offset == 0 && out.phdr == 0 {
                    out.phdr = at + phoff as u32;
                }
                out.end = out.end.max(at + memsz);
            }
            PT_INTERP => out.interp = Some(String::from_utf8_lossy(file).trim_end_matches('\0').to_string()),
            PT_PHDR => out.phdr = vaddr.wrapping_add(bias),
            _ => {}
        }
    }
    if out.end == 0 {
        return Err("no loadable segments".into());
    }
    out.end = page_up(out.end);
    Ok(out)
}
