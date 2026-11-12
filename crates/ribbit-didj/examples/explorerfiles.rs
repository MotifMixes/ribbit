//! Lists or extracts files of a Leapster Explorer built from its packages:
//! the root file system (RFS) or Bulk, read back with `ubifs_read`.
//!
//! cargo run --release -p ribbit-didj --example explorerfiles -- <packages folder> <rfs|bulk> [pattern] [out folder]
//!
//! Lists the paths containing `pattern`; with `out folder`, writes them there.
//! An optional save: RIBBIT_EXPLORER_SAVE=<.nandblocks> is applied first.

use ribbit_didj::{explorer, nand, ubi, ubifs};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let dir = std::path::Path::new(&args[1]);
    let mut flash = explorer::nand_from_packages(dir).unwrap_or_else(|e| panic!("{e}"));
    if let Ok(save) = std::env::var("RIBBIT_EXPLORER_SAVE") {
        flash.apply_changes(&std::fs::read(save).expect("the save")).expect("its changes");
    }
    let n = if args[2] == "rfs" { 3 } else { 4 };
    let offset = explorer::PARTITIONS[n].1;
    let end = explorer::PARTITIONS.get(n + 1).map_or(flash.size(), |p| p.1);
    let part = nand::Partition { name: if n == 3 { "RFS" } else { "Bulk" }, offset, size: end - offset };
    let lebs = ubi::lebs(&flash, part, ubi::available_blocks(part.size / nand::BLOCK));
    let data: Vec<Vec<u8>> = lebs.iter().map(|b| b.map_or_else(Vec::new, |b| flash.read(b * nand::BLOCK + ubi::DATA_OFFSET, ubi::LEB))).collect();
    let slices: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    let tree = ribbit_didj::ubifs_read::read(&slices, &|_| true);
    let pattern = args.get(3).map_or("", String::as_str);
    for (path, entry) in tree.iter().filter(|(p, _)| p.contains(pattern)) {
        match &entry.kind {
            ubifs::Kind::File(d) => {
                println!("{path} ({} bytes)", d.len());
                if let Some(out) = args.get(4) {
                    let to = std::path::Path::new(out).join(path);
                    std::fs::create_dir_all(to.parent().unwrap()).unwrap();
                    std::fs::write(to, d).unwrap();
                }
            }
            ubifs::Kind::Symlink(t) => println!("{path} -> {t}"),
            ubifs::Kind::Dir | ubifs::Kind::Special(..) => {}
        }
    }
}
