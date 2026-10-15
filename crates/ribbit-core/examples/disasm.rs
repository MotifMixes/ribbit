//! Disassembles code in a cartridge or BIOS image (plain or zipped):
//!   cargo run --release -p ribbit-core --example disasm -- <image> <base> <from> [count]
//! <base> is the image's CPU address (0x80000000 for cartridges, 0x40000000
//! for the BIOS); <from> is the CPU address to start at; addresses are hex.

fn hex(s: &str) -> u32 {
    u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex address")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [image, base, from, rest @ ..] = &args[..] else {
        panic!("usage: disasm <image> <base> <from> [count]");
    };
    let data = ribbit_core::read_image(image).expect("image");
    let (base, mut addr) = (hex(base), hex(from));
    let count: usize = rest.first().map_or(64, |c| c.parse().expect("count"));
    let fetch = |a: u32| {
        let o = a.wrapping_sub(base) as usize;
        data.get(o..o + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]))
    };
    for _ in 0..count {
        let i = ribbit_cpu::decode::decode_with(addr, fetch);
        println!("{addr:08x}: {}", ribbit_cpu::disasm::disassemble(&i));
        addr += i.len.max(2);
    }
}
