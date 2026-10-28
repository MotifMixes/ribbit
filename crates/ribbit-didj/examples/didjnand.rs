//! Builds the Didj's internal NAND from its firmware packages and checks
//! what is in it (the user's own packages; nothing is committed):
//!   cargo run -p ribbit-didj --example didjnand -- <folder of .lfp files> [out.nand]
//! out.nand: every page as 2048 data bytes then 64 spare bytes (nanddump -o).

use ribbit_core::zip;
use ribbit_didj::nand::{self, Nand};
use ribbit_didj::package::{self, Firmware, Meta};
use ribbit_didj::tfs;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dir = args.first().expect("usage: didjnand <folder of .lfp files> [out.nand]");
    let mut paths: Vec<_> = std::fs::read_dir(dir)
        .expect("can't read the folder")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| package::package_id(&p.to_string_lossy()).is_some())
        .collect();
    paths.sort();

    let mut fw = Firmware::default();
    for path in &paths {
        let data = std::fs::read(path).expect("can't read a package");
        let entries = zip::entries(&data).expect("not a zip archive");
        let meta = entries
            .iter()
            .find(|e| e.name.ends_with("/meta.inf"))
            .map(|e| Meta::parse(&String::from_utf8_lossy(&zip::read_entry(&data, e).unwrap())));
        let get = |k| meta.as_ref().and_then(|m| m.get(k)).unwrap_or("?").to_string();
        let mut firmware = Vec::new();
        for e in &entries {
            if fw.add(&e.name, zip::read_entry(&data, e).unwrap()) {
                firmware.push(e.name.rsplit('/').next().unwrap().to_string());
            }
        }
        println!(
            "{}  {} {} ({} files){}",
            package::package_id(&path.to_string_lossy()).unwrap(),
            get("Name"),
            get("Version"),
            entries.len(),
            if firmware.is_empty() { String::new() } else { format!("  firmware: {}", firmware.join(", ")) }
        );
    }

    let flash = Nand::from_firmware(&fw).unwrap_or_else(|e| panic!("{e}"));
    println!();
    for p in nand::PARTITIONS {
        let used = (0..p.size / nand::BLOCK)
            .filter(|b| flash.read(p.offset + b * nand::BLOCK, nand::BLOCK).iter().any(|&x| x != 0xFF))
            .count();
        println!("{:#010x} {:<19} {:>5} blocks, {:>4} written", p.offset, p.name, p.size / nand::BLOCK, used);
    }

    let k0 = nand::partition("Kernel0").unwrap().offset;
    let files = tfs::files(&flash.read(k0, nand::BLOCK)).expect("kernel.bin: no TFS summary");
    println!();
    for f in &files {
        let head = flash.read(k0 + f.offset, 64);
        let note = if u32::from_be_bytes(head[..4].try_into().unwrap()) == 0x2705_1956 {
            let be = |o: usize| u32::from_be_bytes(head[o..o + 4].try_into().unwrap());
            let name = String::from_utf8_lossy(&head[32..64]).trim_end_matches('\0').to_string();
            format!("uImage \"{name}\", {} bytes, load {:#x}, entry {:#x}", be(12), be(16), be(20))
        } else {
            String::new()
        };
        println!("{:<16} {} blocks at +{:#x}  {note}", f.name, f.blocks, f.offset);
    }

    if let Some(out) = args.get(1) {
        let mut raw = Vec::with_capacity(nand::BLOCKS * nand::PAGES_PER_BLOCK * nand::PAGE_RAW);
        for n in 0..nand::BLOCKS * nand::PAGES_PER_BLOCK {
            raw.extend_from_slice(flash.page(n));
        }
        std::fs::write(out, raw).expect("can't write the image");
        println!("\nwrote {out}");
    }
}
