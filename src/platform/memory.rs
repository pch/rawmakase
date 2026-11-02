//! The computer's memory, for work sized to it.

/// The computer's physical memory in bytes, when the system says.
pub(crate) fn total() -> Option<u64> {
    let bytes = physical()?;
    (bytes > 0).then_some(bytes)
}

#[cfg(target_os = "macos")]
fn physical() -> Option<u64> {
    Some(objc2_foundation::NSProcessInfo::processInfo().physicalMemory())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn physical() -> Option<u64> {
    let info = rustix::system::sysinfo();
    // A count of `mem_unit`-byte units; 0 for bytes on kernels before 2.3.23.
    #[allow(clippy::useless_conversion)]
    let units = u64::from(info.totalram);
    Some(units.saturating_mul(u64::from(info.mem_unit.max(1))))
}

#[cfg(windows)]
fn physical() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
        ..Default::default()
    };
    // SAFETY: `status` is a MEMORYSTATUSEX with its size set, as the call asks.
    let filled = unsafe { GlobalMemoryStatusEx(&mut status) } != 0;
    filled.then_some(status.ullTotalPhys)
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "linux",
    target_os = "android",
    windows
)))]
fn physical() -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    /// Every platform CI runs on says, and no computer has less than 512 MB.
    #[test]
    fn the_computer_s_memory_is_known() {
        let bytes = super::total().expect("memory size");
        assert!(bytes >= 512 << 20, "{bytes}");
    }
}
