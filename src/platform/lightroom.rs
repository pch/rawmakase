//! Opening Lightroom (not Classic), for it to download originals it was
//! asked to keep on this computer.
use anyhow::{Result, bail};
use std::process::Command;

/// Opens Lightroom, or brings it forward when it is open. macOS only: it is
/// found by its bundle id; elsewhere the user opens it.
pub(crate) fn open() -> Result<()> {
    if cfg!(target_os = "macos") {
        let status = Command::new("open")
            .args(["-b", "com.adobe.lightroomCC"])
            .status()?;
        if !status.success() {
            bail!("Lightroom isn't installed");
        }
        return Ok(());
    }
    bail!("Opening Lightroom isn't supported here")
}
