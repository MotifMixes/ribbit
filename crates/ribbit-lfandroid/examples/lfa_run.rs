//! Runs a LeapFrog Android title's native library as far as Ribbit's stand-ins go: its
//! constructors, then `JNI_OnLoad` (it registers the natives the Java shell calls).
//!
//! `cargo run --release -p ribbit-lfandroid --example lfa_run -- [--lib <system/lib>] <apk>`
//! LFA_LOG=1 prints the game's log as it goes.

use ribbit_lfandroid::runtime::Runtime;
use std::path::PathBuf;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut lib = None;
    if let Some(i) = args.iter().position(|a| a == "--lib") {
        lib = Some(PathBuf::from(args.remove(i + 1)));
        args.remove(i);
    }
    let Some(apk) = args.first() else {
        eprintln!("usage: lfa_run [--lib <system/lib>] <apk>");
        std::process::exit(2);
    };
    let title = ribbit_lfandroid::open_title(apk.as_ref(), lib.as_deref()).unwrap_or_else(|e| panic!("{e}"));
    let Some(loaded) = &title.library else {
        eprintln!("{}: no native library found", title.package);
        std::process::exit(1);
    };
    println!("{} {}: {} constructors", title.package, title.version, loaded.init.len());
    let mut rt = Runtime::new(loaded);
    let report = |rt: &Runtime, what: &str, r: Result<u32, ribbit_lfandroid::runtime::Stop>| {
        match r {
            Ok(v) => println!("{what}: returned {v:#x}"),
            Err(e) => {
                println!("{what}: {e}");
                println!("  pc {:#010x} lr {:#010x} sp {:#010x}, {} instructions", rt.cpu.current, rt.cpu.r[14], rt.cpu.r[13], rt.cpu.instructions);
            }
        }
    };
    let r = rt.run_constructors().map(|()| 0);
    let ok = r.is_ok();
    report(&rt, "constructors", r);
    if ok {
        let r = rt.jni_onload();
        report(&rt, "JNI_OnLoad", r);
    }
    println!("natives registered: {}", rt.natives.len());
    for (n, v) in &rt.natives {
        println!("  {}.{n}{} at {:#010x}", v.class, v.signature, v.addr);
    }
    println!("imports called: {}", rt.calls.len());
    for l in rt.log.iter().rev().take(10).collect::<Vec<_>>().into_iter().rev() {
        println!("  log: {l}");
    }
    if !rt.mem.unmapped.is_empty() {
        println!("unmapped reads: {:x?}", rt.mem.unmapped);
    }
}
