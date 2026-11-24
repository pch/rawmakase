//! Opening a file in a program the user chose: Edit In's external editors.
use anyhow::{Context, Result, ensure};
use std::path::Path;
use std::process::Command;

/// The command that opens `file` in `program`: on macOS an application
/// bundle, opened through `open -a` so a running app takes the file; elsewhere
/// the executable itself, given the file.
pub(crate) fn command(program: &Path, file: &Path) -> Command {
    if cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.arg("-a").arg(program).arg(file);
        command
    } else {
        let mut command = Command::new(program);
        command.arg(file);
        command
    }
}

/// Opens `file` in `program`, without waiting for it. An error when the
/// program is gone or can't be started.
pub(crate) fn open_with(program: &Path, file: &Path) -> Result<()> {
    ensure!(program.exists(), "{} is no longer there", program.display());
    command(program, file)
        .spawn()
        .with_context(|| format!("{} could not be started", program.display()))?;
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
            assert_eq!(command.get_program(), "open");
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

    #[test]
    fn programs_are_named_without_their_extension() {
        assert_eq!(
            program_name(Path::new("/Applications/Adobe Photoshop 2026.app")),
            "Adobe Photoshop 2026"
        );
        assert_eq!(program_name(Path::new("/usr/bin/gimp")), "gimp");
    }
}
