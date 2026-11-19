//! Inspect LeapFrog Android titles (Epic / LeapPad Academy) without running anything.
//!
//!   cargo run -p ribbit-lfandroid --example lfa_inspect -- [--lib <system/lib dir>] <app.apk>...
//!
//! For each APK: package, Brio SDK version, the native game library (from the APK or from
//! the firmware's /system/lib), its CPU attributes, relocations, the imports an HLE must
//! provide grouped by Android library, and the JNI natives the Java shell calls.

use std::collections::BTreeMap;
use std::path::PathBuf;

fn main() {
    let mut args = std::env::args().skip(1);
    let mut lib = None;
    let mut apks = Vec::new();
    let mut verbose = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--lib" => lib = args.next().map(PathBuf::from),
            "-v" => verbose = true,
            _ => apks.push(PathBuf::from(a)),
        }
    }
    if apks.is_empty() {
        eprintln!("usage: lfa_inspect [--lib <system/lib>] [-v] <app.apk>...");
        std::process::exit(2);
    }
    let mut all_imports: BTreeMap<String, usize> = BTreeMap::new();
    let mut loaded = 0;
    for p in &apks {
        let t = match ribbit_lfandroid::open_title(p, lib.as_deref()) {
            Ok(t) => t,
            Err(e) => {
                println!("== {}: {e}", p.display());
                continue;
            }
        };
        println!("== {} {} ({})", t.package, t.version, p.file_name().unwrap_or_default().to_string_lossy());
        println!("   lib_name {:?}, contentType {:?}, Brio SDK {:?}", t.lib_name, t.content_type, t.sdk_version);
        let mut dirs: Vec<_> = t.asset_dirs.iter().collect();
        dirs.sort_by(|a, b| b.1.1.cmp(&a.1.1));
        let top: Vec<String> = dirs.iter().take(5).map(|(k, (n, b))| format!("{k} ({n} files, {} KB)", b / 1024)).collect();
        println!("   assets: {}", top.join(", "));
        let Some(l) = &t.library else {
            println!("   no native library found (lib_name {:?})", t.lib_name);
            continue;
        };
        loaded += 1;
        println!("   library: {} ({} KB mapped at {:#x})", t.library_source.as_deref().unwrap_or("?"), l.image.len() / 1024, l.base);
        if let Some(a) = &l.arm_attributes {
            println!("   cpu: {}", ribbit_lfandroid::elf::describe_attributes(a).join("; "));
        }
        println!("   needs: {}", l.needed.join(" "));
        let r = &l.relocs;
        println!("   relocations: {} relative, {} abs32, {} glob_dat, {} jump_slot, other {:?}, unbound {}", r.relative, r.abs32, r.glob_dat, r.jump_slot, r.other, r.unbound);
        let brio = l.exports.keys().filter(|k| k.contains("8LeapFrog4Brio")).count();
        let mpi: std::collections::BTreeSet<&str> = l.exports.keys().filter_map(|k| {
            let i = k.find("8LeapFrog4Brio")? + 14;
            let rest = &k[i..];
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            let n: usize = digits.parse().ok()?;
            let name = rest.get(digits.len()..digits.len() + n)?;
            name.ends_with("MPI").then_some(name)
        }).collect();
        println!("   exports: {} ({} in LeapFrog::Brio); MPIs: {}", l.exports.len(), brio, mpi.into_iter().collect::<Vec<_>>().join(" "));
        println!("   constructors: {}", l.init.len());
        let mut by_area: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
        for name in l.imports.keys() {
            by_area.entry(ribbit_lfandroid::api_area(name)).or_default().push(name);
            *all_imports.entry(name.clone()).or_default() += 1;
        }
        println!("   imports: {}", l.imports.len());
        for (area, names) in &by_area {
            if verbose || area.starts_with("Open") || area.starts_with("EGL") || area.starts_with("liba") {
                println!("     {area} ({}): {}", names.len(), names.join(" "));
            } else {
                println!("     {area} ({})", names.len());
            }
        }
        println!("   JNI natives ({}): {}", t.natives.len(), t.natives.join(" "));
    }
    if loaded > 1 {
        let mut by_area: BTreeMap<&str, usize> = BTreeMap::new();
        for name in all_imports.keys() {
            *by_area.entry(ribbit_lfandroid::api_area(name)).or_default() += 1;
        }
        println!("== union of imports over {loaded} libraries: {} ({:?})", all_imports.len(), by_area);
    }
}
