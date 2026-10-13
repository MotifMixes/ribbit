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
