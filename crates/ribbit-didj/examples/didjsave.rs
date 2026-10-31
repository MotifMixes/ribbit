//! Lists what a Didj save holds: the partitions its changed blocks are in,
//! and the files on /Didj (the Brio volume) that differ from the image
//! built from the packages.
//!
//! cargo run --release -p ribbit-didj --example didjsave -- <packages folder> <save.nandblocks> [carry]
//!
//! With `carry`: the files `carry::user_files` would carry from the save
//! (one made on another image) onto the image built now.

use ribbit_core::fatfs;
use ribbit_didj::{brio, nand, ubi};

fn walk(volume: &[u8], path: &str, out: &mut Vec<(String, Vec<u8>)>) {
    for e in fatfs::list(volume, path).unwrap_or_default() {
        let p = format!("{path}/{}", e.name);
        if e.is_dir {
            walk(volume, &p, out);
        } else {
            out.push((p.clone(), fatfs::read_file(volume, &p).unwrap_or_default()));
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, save) = (std::path::Path::new(&args[1]), &args[2]);
    let original = ribbit_didj::nand_from_packages(dir, true).unwrap_or_else(|e| panic!("{e}"));
    let changes = if save == "-" { Vec::new() } else { std::fs::read(save).expect("the save") };
    // `volume <file>`: the Brio volume (the save applied; "-" as the save:
    // none) written to <file>, for fsck.vfat.
    if args.get(3).is_some_and(|a| a == "volume") {
        let mut flash = original.clone();
        if !changes.is_empty() {
            flash.apply_changes(&changes).expect("its changes");
        }
        let brio = nand::partition("Brio").unwrap();
        let len = brio::volume_sectors(brio.size / nand::BLOCK) * 512;
        std::fs::write(&args[4], ubi::read_volume(&flash, brio, len)).expect("writing the volume");
        return;
    }
    if args.get(3).is_some_and(|a| a == "carry") {
        for (p, d) in ribbit_didj::carry::user_files(&changes, &original).unwrap_or_else(|e| panic!("{e}")) {
            println!("carry    {p} ({} bytes)", d.len());
        }
        return;
    }
    let mut flash = original.clone();
    flash.apply_changes(&changes).expect("its changes");
    let mut counts = std::collections::BTreeMap::new();
    for p in nand::PARTITIONS {
        for b in p.offset / nand::BLOCK..(p.offset + p.size) / nand::BLOCK {
            if flash.read(b * nand::BLOCK, nand::BLOCK) != original.read(b * nand::BLOCK, nand::BLOCK) {
                *counts.entry(p.name).or_insert(0) += 1;
            }
        }
    }
    println!("changed blocks: {counts:?}");
    let brio = nand::partition("Brio").unwrap();
    let len = brio::volume_sectors(brio.size / nand::BLOCK) * 512;
    let (mut was, mut now) = (Vec::new(), Vec::new());
    walk(&ubi::read_volume(&original, brio, len), "", &mut was);
    walk(&ubi::read_volume(&flash, brio, len), "", &mut now);
    let was: std::collections::HashMap<_, _> = was.into_iter().collect();
    for (p, d) in &now {
        match was.get(p) {
            None => println!("new      {p} ({} bytes)", d.len()),
            Some(o) if o != d => println!("changed  {p} ({} bytes)", d.len()),
            _ => {}
        }
    }
    for p in was.keys().filter(|p| !now.iter().any(|(n, _)| n == *p)) {
        println!("removed  {p}");
    }
}
