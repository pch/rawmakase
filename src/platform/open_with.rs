//! Opening a file in a program the user chose: Edit In's external editors.
use anyhow::{Context, Result, ensure};
use std::path::Path;
use std::process::Command;

/// The command that opens `file` in `program`: on macOS an application
/// bundle, opened through the system's `open -a` (by its path, not whatever
/// `open` comes first on the search path) so a running app takes the file;
/// elsewhere the executable itself, given the file.
pub(crate) fn command(program: &Path, file: &Path) -> Command {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-a").arg(program).arg(file);
        command
    } else {
        let mut command = Command::new(program);
        command.arg(file);
        command
    }
}

/// Opens `file` in `program`. An error when the program is gone or can't be
/// started, or on macOS when `open` says it could not open the file, which it
/// does within moments: this waits for it, so it runs off the UI thread. An
/// editor started directly runs until it is closed: a thread of its own
/// waits for it then, so it leaves no zombie process behind.
pub(crate) fn open_with(program: &Path, file: &Path) -> Result<()> {
    ensure!(program.exists(), "{} is no longer there", program.display());
    let started = || format!("{} could not be started", program.display());
    if cfg!(target_os = "macos") {
        let output = command(program, file).output().with_context(started)?;
        let said = String::from_utf8_lossy(&output.stderr);
        ensure!(output.status.success(), "{}", said.trim());
    } else {
        let mut child = command(program, file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(started)?;
        std::thread::spawn(move || child.wait());
    }
    Ok(())
}

/// The name a program is shown by: an app bundle's or executable's name
/// without its extension ("Adobe Photoshop 2026" for
/// `Adobe Photoshop 2026.app`, "gimp" for `/usr/bin/gimp`).
pub(crate) fn program_name(program: &Path) -> String {
    program
        .file_stem()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn the_command_opens_the_file_in_the_program() {
        let program = Path::new(if cfg!(windows) {
            r"C:\Apps\Editor.exe"
        } else if cfg!(target_os = "macos") {
            "/Applications/Editor.app"
        } else {
            "/usr/bin/editor"
        });
        let file = Path::new("photo-Edit.tif");
        let command = command(program, file);
        let args: Vec<&OsStr> = command.get_args().collect();
        if cfg!(target_os = "macos") {
            assert_eq!(command.get_program(), "/usr/bin/open");
            assert_eq!(
                args,
                [OsStr::new("-a"), program.as_os_str(), file.as_os_str()]
            );
        } else {
            assert_eq!(command.get_program(), program.as_os_str());
            assert_eq!(args, [file.as_os_str()]);
        }
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_launch() {
        let dir = tempfile::tempdir().unwrap();
        let error = open_with(&dir.path().join("Gone.app"), Path::new("a.tif")).unwrap_err();
        assert!(error.to_string().contains("no longer there"), "{error}");
    }

    /// `open` saying it could not open the file is an error, not a launch: it
    /// checks the file before starting anything.
    #[cfg(target_os = "macos")]
    #[test]
    fn opens_refusal_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let error = open_with(
            Path::new("/System/Applications/TextEdit.app"),
            &dir.path().join("missing.tif"),
        )
        .unwrap_err();
        assert!(error.to_string().contains("does not exist"), "{error}");
    }

    #[test]
    fn programs_are_named_without_their_extension() {
        assert_eq!(
            program_name(Path::new("/Applications/Adobe Photoshop 2026.app")),
            "Adobe Photoshop 2026"
        );
        assert_eq!(program_name(Path::new("/usr/bin/gimp")), "gimp");
    }
}
