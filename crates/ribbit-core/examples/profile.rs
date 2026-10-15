//! Poor man's sampling profiler (Windows, no admin rights needed): a helper
//! thread suspends the emulator thread ~2000 times a second, reads its
//! instruction pointer, and the samples are resolved to (inlined) function
//! names afterwards. Build with debug info:
//!   CARGO_PROFILE_RELEASE_DEBUG=true cargo run --release -p ribbit-core --example profile -- <bios> [steps]

#[cfg(windows)]
fn main() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use windows_sys::Win32::Foundation::{DUPLICATE_SAME_ACCESS, DuplicateHandle, HANDLE};
    use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentThread, ResumeThread, SuspendThread};

    #[repr(C, align(16))]
    struct Ctx(CONTEXT);

    let mut args = std::env::args().skip(1);
    let path = args.next().expect("usage: profile <bios> [steps]");
    let steps: u64 = args.next().map_or(100_000_000, |s| s.parse().expect("steps"));
    let fw = ribbit_memory::Firmware::from_bytes(ribbit_core::read_image(&path).expect("read")).expect("firmware");
    let cart = std::env::var("RIBBIT_CART")
        .ok()
        .map(|c| ribbit_memory::Cartridge::from_bytes(ribbit_core::read_image(&c).expect("read cart")).expect("cartridge"));
    let mut m = ribbit_core::Machine::with_straps(fw, cart, 0x63FF_FFFF);
    // Optional: RIBBIT_CART (cartridge) and RIBBIT_STATE (save state to start from).
    if let Ok(state) = std::env::var("RIBBIT_STATE") {
        m.load_state(&std::fs::read(&state).expect("read state")).expect("load state");
    }

    m.record_trace_tail = std::env::var_os("RIBBIT_NO_TAIL").is_none();
    // RIBBIT_NOSKIP=1: run every idle-loop iteration (profile busy code only).
    m.idle_skip = std::env::var_os("RIBBIT_NOSKIP").is_none();
    let mut handle: HANDLE = std::ptr::null_mut();
    // SAFETY: plain Win32 calls on this process's own pseudo-handles.
    unsafe {
        DuplicateHandle(
            GetCurrentProcess(),
            GetCurrentThread(),
            GetCurrentProcess(),
            &mut handle,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        );
    }
    let handle = handle as usize;
    let stop = Arc::new(AtomicBool::new(false));
    let samples = Arc::new(Mutex::new(Vec::<u64>::new()));
    let sampler = {
        let (stop, samples) = (stop.clone(), samples.clone());
        std::thread::spawn(move || {
            let h = handle as HANDLE;
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(std::time::Duration::from_micros(500));
                // SAFETY: the target thread is ours; it is resumed right away.
                unsafe {
                    if SuspendThread(h) == u32::MAX {
                        continue;
                    }
                    let mut ctx: Ctx = std::mem::zeroed();
                    ctx.0.ContextFlags = 0x0010_0001; // CONTEXT_CONTROL (AMD64)
                    if GetThreadContext(h, &mut ctx.0) != 0 {
                        samples.lock().unwrap().push(ctx.0.Rip);
                    }
                    ResumeThread(h);
                }
            }
        })
    };
    let t = std::time::Instant::now();
    m.run(steps, &ribbit_debug::Breakpoints::default());
    let secs = t.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    sampler.join().unwrap();

    let samples = samples.lock().unwrap();
    if std::env::var_os("PROFILE_RAW").is_some() {
        // Hottest instruction addresses, relative to `main` (for matching
        // against a disassembly of this executable).
        let base = main as *const () as usize as u64;
        let mut counts: std::collections::HashMap<u64, u64> = Default::default();
        for &ip in samples.iter() {
            *counts.entry(ip).or_default() += 1;
        }
        let mut rows: Vec<_> = counts.into_iter().collect();
        rows.sort_by_key(|r| std::cmp::Reverse(r.1));
        println!("main at {base:#x}");
        for (ip, n) in rows.iter().take(80) {
            println!("{:6.2}% main{:+#x}", 100.0 * *n as f64 / samples.len() as f64, *ip as i64 - base as i64);
        }
        return;
    }
    let mut by_fn: std::collections::HashMap<String, u64> = Default::default();
    let mut cache: std::collections::HashMap<u64, String> = Default::default();
    for &ip in samples.iter() {
        let name = cache
            .entry(ip)
            .or_insert_with(|| {
                let mut name = None;
                let by_line = std::env::var_os("PROFILE_LINES").is_some();
                let chain = std::env::var_os("PROFILE_CHAIN").is_some();
                backtrace::resolve(ip as *mut std::ffi::c_void, |s| {
                    if chain {
                        // Every inlined frame, innermost first.
                        let file = s.filename().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().into_owned());
                        let here = format!("{}:{}", file.unwrap_or_default(), s.lineno().unwrap_or(0));
                        name = Some(match name.take() {
                            None => here,
                            Some(n) => format!("{n} < {here}"),
                        });
                        return;
                    }
                    if name.is_none() {
                        name = if by_line {
                            let file =
                                s.filename().and_then(|f| f.file_name()).map(|f| f.to_string_lossy().into_owned());
                            Some(format!("{}:{}", file.unwrap_or_default(), s.lineno().unwrap_or(0)))
                        } else {
                            s.name().map(|n| format!("{n:#}"))
                        };
                    }
                });
                name.unwrap_or_else(|| format!("?{ip:#x}"))
            })
            .clone();
        *by_fn.entry(name).or_default() += 1;
    }
    let mut rows: Vec<_> = by_fn.into_iter().collect();
    rows.sort_by_key(|a| std::cmp::Reverse(a.1));
    println!("{steps} steps in {secs:.2} s ({:.1} M/s); {} samples", steps as f64 / secs / 1e6, samples.len());
    for (name, n) in rows.iter().take(40) {
        println!("{:6.2}%  {name}", 100.0 * *n as f64 / samples.len() as f64);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("profile example is Windows-only");
}
