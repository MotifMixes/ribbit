//! Disassembles part of a LeapPad ROM image: `lpdis <file> <file offset> <cpu address> <length>`
//! (hex numbers). The CPU address is where the bytes appear in the 64 KB code space.
use ribbit_leappad::disasm::disasm;

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex number")
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let rom = std::fs::read(&a[1]).expect("read ROM");
    let (off, base, count) = (hex(&a[2]), hex(&a[3]), hex(&a[4]));
    let read = |pc: u16| rom.get(off.wrapping_add((pc as u32).wrapping_sub(base)) as usize).copied().unwrap_or(0xFF);
    let mut pc = base as u16;
    let mut done = 0u32;
    while done < count {
        let (t, len) = disasm(pc, read);
        let bytes: Vec<String> = (0..len).map(|i| format!("{:02X}", read(pc.wrapping_add(i)))).collect();
        println!("{:05X} {pc:04X}: {:<9} {t}", off + done, bytes.join(" "));
        pc = pc.wrapping_add(len);
        done += len as u32;
    }
}
