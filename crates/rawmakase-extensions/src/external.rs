//! Externally launched adapters: a bundled program that something other than
//! the app starts, such as an MCP client starting `rawmakase mcp`. The app
//! never supervises it; it reaches the app through the control socket, naming
//! its adapter on every request, and the app refuses those requests while the
//! extension is disabled.
//!
//! The command its clients run finds the program with [`program`] and runs it
//! with [`run`], with the client's standard streams.
use crate::bundled;
use crate::state::State;
use crate::{Error, Result};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// The program of the bundled extension `manifest` (its compiled-in text),
/// looked for in `dirs`, unless `<data_dir>/extensions.json` disables the
/// extension. Its errors are the user's to read: disabled, an
/// `extensions.json` this version can't read, or a program missing beside
/// the app.
pub fn program(manifest: &str, data_dir: &Path, dirs: &[PathBuf]) -> Result<PathBuf> {
    let found = bundled::discover(&[manifest], dirs)
        .pop()
        .expect("one manifest, one result")?;
    let extension = &found.manifest.extension;
    let state = State::read(data_dir)?;
    if !state.enabled(&extension.id) {
        return Err(Error::new(format!(
            "The {} extension is disabled in Preferences > Extensions",
            extension.name
        )));
    }
    if let Some(problem) = found.problem() {
        return Err(Error::new(problem));
    }
    found
        .program
        .ok_or_else(|| Error::new(format!("{} has no program", extension.name)))
}

/// Runs `program` with `args` and this process's standard streams, and
/// returns its exit code. On Linux and macOS it replaces this process
/// (`exec`), so it returns only an error. On Windows it runs in a job object
/// that kills its processes when the job's last handle closes, so a helper it
/// starts dies with this process, however that ends.
pub fn run(program: &Path, args: &[OsString]) -> std::io::Result<i32> {
    platform::run(program, args)
}

#[cfg(unix)]
mod platform {
    use std::ffi::OsString;
    use std::os::unix::process::CommandExt;
    use std::path::Path;

    pub(super) fn run(program: &Path, args: &[OsString]) -> std::io::Result<i32> {
        Err(std::process::Command::new(program).args(args).exec())
    }
}

#[cfg(windows)]
mod platform {
    use std::ffi::OsString;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;
    use windows_sys::Win32::System::JobObjects::AssignProcessToJobObject;
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    pub(super) fn run(program: &Path, args: &[OsString]) -> std::io::Result<i32> {
        // This process joins the job first, so the program is in it from its
        // creation, as every process it starts is.
        let job = crate::process::kill_on_close_job()?;
        // SAFETY: a valid job handle, and the current process's pseudo-handle.
        if unsafe { AssignProcessToJobObject(job.as_raw_handle(), GetCurrentProcess()) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
        // Closing the job's last handle kills every process in it, this one
        // included, so the handle must outlive this function: otherwise this
        // process would end with the job's status, not the program's exit
        // code, and before it could report an error. It closes when this
        // process ends, killed or not, and ends the program's helpers.
        std::mem::forget(job);
        let status = std::process::Command::new(program).args(args).status()?;
        Ok(status.code().unwrap_or(1))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;

    const MANIFEST: &str = "[extension]\nid = \"rawmakase.mcp\"\nname = \"MCP\"\n\
        version = \"1.0.0\"\nrawmakase = \"*\"\napi = 1\n[program]\npath = \"rawmakase-mcp\"\n\
        [[control-adapter]]\nid = \"mcp\"\nlaunch = \"external\"\n";

    #[test]
    fn the_program_runs_unless_its_extension_is_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let (data, bin) = (dir.path().join("data"), dir.path().join("bin"));
        std::fs::create_dir_all(&bin).unwrap();
        let dirs = [bin.clone()];
        let missing = program(MANIFEST, &data, &dirs).unwrap_err();
        assert!(
            missing.0.contains("not installed beside RAWmakase"),
            "{missing}"
        );
        let installed = bin.join(bundled::program_name("rawmakase-mcp"));
        std::fs::write(&installed, b"").unwrap();
        assert_eq!(program(MANIFEST, &data, &dirs).unwrap(), installed);
        let store = Store::new(&data);
        store
            .set_enabled("rawmakase.mcp", false, &["rawmakase.mcp"])
            .unwrap();
        assert_eq!(
            program(MANIFEST, &data, &dirs).unwrap_err().0,
            "The MCP extension is disabled in Preferences > Extensions"
        );
        store
            .set_enabled("rawmakase.mcp", true, &["rawmakase.mcp"])
            .unwrap();
        assert_eq!(program(MANIFEST, &data, &dirs).unwrap(), installed);
    }

    #[test]
    fn an_extensions_json_it_cant_read_stops_it() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(crate::state::FILE), "{\"version\": 99}").unwrap();
        let error = program(MANIFEST, dir.path(), &[]).unwrap_err();
        assert!(error.0.contains("newer RAWmakase"), "{error}");
    }
}
