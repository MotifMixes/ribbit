//! Keeps the emulator on fast cores. On hybrid CPUs (e.g. Intel 12th-14th
//! gen) Windows may run a process in "efficiency mode" on E-cores, where the
//! interpreter runs at less than half speed (measured: 47.7 vs 22.1 M
//! steps/s on an i9-13900K). Opting out of execution-speed throttling asks
//! for P-cores; above-normal priority keeps the emulation thread there.

#[cfg(windows)]
pub fn prefer_performance_cores() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetCurrentThread, PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        PROCESS_POWER_THROTTLING_EXECUTION_SPEED, PROCESS_POWER_THROTTLING_STATE, ProcessPowerThrottling,
        SetProcessInformation, SetThreadPriority, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    let state = PROCESS_POWER_THROTTLING_STATE {
        Version: PROCESS_POWER_THROTTLING_CURRENT_VERSION,
        ControlMask: PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
        // Control bit set, state bit clear: never throttle execution speed.
        StateMask: 0,
    };
    // SAFETY: plain Win32 calls on this process's own pseudo-handles with a
    // correctly sized, initialised structure.
    let (throttle, priority) = unsafe {
        (
            SetProcessInformation(
                GetCurrentProcess(),
                ProcessPowerThrottling,
                (&raw const state).cast(),
                size_of::<PROCESS_POWER_THROTTLING_STATE>() as u32,
            ),
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL),
        )
    };
    tracing::info!(target: "ribbit::perf", power_throttling_off = throttle != 0, above_normal = priority != 0, "scheduling");
}

#[cfg(not(windows))]
pub fn prefer_performance_cores() {}

/// Above-normal priority for the calling thread (the emulation thread),
/// and on hybrid CPUs only the fastest cores. The emulation thread sleeps
/// between short chunks, which the scheduler can read as light work and
/// move to an E-core: there it runs at less than half speed, which is
/// exactly what makes busy scenes stutter. CPU Sets are a soft restriction
/// (Windows may still use other cores if these are all unavailable).
#[cfg(windows)]
pub fn raise_thread_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, SetThreadSelectedCpuSets, THREAD_PRIORITY_ABOVE_NORMAL,
    };
    let fast = fastest_cpu_sets();
    // SAFETY: plain Win32 calls on this thread's pseudo-handle; `fast` is a
    // live slice of CPU Set ids for the duration of the call.
    let (priority, pinned) = unsafe {
        (
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_ABOVE_NORMAL),
            !fast.is_empty() && SetThreadSelectedCpuSets(GetCurrentThread(), fast.as_ptr(), fast.len() as u32) != 0,
        )
    };
    tracing::info!(target: "ribbit::perf", above_normal = priority != 0, fast_cores = fast.len(), pinned, "emulation thread");
}

/// CPU Set ids of the highest efficiency class (the P-cores), or empty if
/// all cores are alike or the query fails.
#[cfg(windows)]
fn fastest_cpu_sets() -> Vec<u32> {
    use windows_sys::Win32::System::SystemInformation::{
        CpuSetInformation, GetSystemCpuSetInformation, SYSTEM_CPU_SET_INFORMATION,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut len = 0u32;
    // SAFETY: a size query (null buffer, zero length), then a fill of a
    // buffer of exactly the returned size, aligned for the structure.
    let buf = unsafe {
        GetSystemCpuSetInformation(std::ptr::null_mut(), 0, &mut len, GetCurrentProcess(), 0);
        if len == 0 {
            return Vec::new();
        }
        let mut buf = vec![0u64; (len as usize).div_ceil(8)];
        if GetSystemCpuSetInformation(buf.as_mut_ptr().cast(), len, &mut len, GetCurrentProcess(), 0) == 0 {
            return Vec::new();
        }
        buf
    };
    let bytes = len as usize;
    let mut sets = Vec::new();
    let mut at = 0;
    while at + size_of::<SYSTEM_CPU_SET_INFORMATION>() <= bytes {
        // SAFETY: inside the filled buffer; records are 8-byte aligned and
        // `Size` gives the stride (records may grow in later Windows versions).
        let info = unsafe { &*(buf.as_ptr().cast::<u8>().add(at) as *const SYSTEM_CPU_SET_INFORMATION) };
        if info.Size == 0 {
            break;
        }
        if info.Type == CpuSetInformation {
            // SAFETY: `CpuSet` is the union's only member for this type.
            let c = unsafe { info.Anonymous.CpuSet };
            sets.push((c.Id, c.EfficiencyClass));
        }
        at += info.Size as usize;
    }
    let (Some(max), Some(min)) =
        (sets.iter().map(|s| s.1).max(), sets.iter().map(|s| s.1).min())
    else {
        return Vec::new();
    };
    if max == min {
        return Vec::new();
    }
    sets.into_iter().filter(|s| s.1 == max).map(|s| s.0).collect()
}

#[cfg(not(windows))]
pub fn raise_thread_priority() {}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn fastest_cpu_sets_are_valid_ids() {
        // Empty on CPUs whose cores are all alike; otherwise distinct ids.
        let mut ids = super::fastest_cpu_sets();
        eprintln!("fast CPU sets: {ids:?}");
        let n = ids.len();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), n);
    }
}
