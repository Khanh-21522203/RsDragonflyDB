#[cfg(target_os = "linux")]
pub fn pin_to_core(core_id: usize) -> Result<(), std::io::Error> {
    use libc::{cpu_set_t, sched_setaffinity, CPU_SET, CPU_ZERO};

    unsafe {
        let mut cpuset: cpu_set_t = std::mem::zeroed();
        CPU_ZERO(&mut cpuset);
        CPU_SET(core_id, &mut cpuset);

        let result = sched_setaffinity(
            0,  // Current thread
            std::mem::size_of::<cpu_set_t>(),
            &cpuset,
        );

        if result != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }

    Ok(())
}

#[cfg(not(target_os = "linux"))]
pub fn pin_to_core(_core_id: usize) -> Result<(), std::io::Error> {
    // CPU pinning not supported on this platform
    Ok(())
}