//! A rename that keeps its destination's name first, for a file system
//! that can't be asked not to replace one (ntfs-3g, #914).
//!
//! The new name is taken by a placeholder, a small file holding a marker
//! that names both paths, and the file is then renamed onto it, as a plain
//! rename replaces a file (ntfs-3g does so in one step). The placeholder
//! appears whole: its marker is written to a file of its own first, made
//! only where nothing has its name ([`staged`], hidden, named after both
//! paths), which is then linked at the new name, which fails where that
//! name is taken. So no file another program made at the new name is
//! replaced, and one put in the placeholder's place before the rename is
//! kept, the rename refused.
//!
//! The only files ever removed are those this made: a placeholder, by its
//! whole marker, and a staged file, by its name and a marker it holds all
//! or the start of (a write cut short). A photo's file, empty or not, is
//! never taken for either.
//!
//! What can't be ruled out on such a file system, and is accepted: a file
//! another program puts at one of these names by replacing that very file
//! in the moment between checking it and renaming onto it or removing it
//! is lost. Linux can't remove or replace a name only while it is still a
//! given file.
use super::FsError;
use sha2::{Digest as _, Sha256};
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};

const MARK: &[u8] = b"RAWmakase keeps this name for a file it is renaming: ";

/// What a placeholder for renaming `from` to `to` holds.
fn marker(from: &Path, to: &Path) -> Vec<u8> {
    let mut bytes = MARK.to_vec();
    bytes.extend_from_slice(from.as_os_str().as_encoded_bytes());
    bytes.push(b'\n');
    bytes.extend_from_slice(to.as_os_str().as_encoded_bytes());
    bytes
}

/// Where a placeholder for renaming `from` to `to` is written before it is
/// linked at `to`: hidden, in `to`'s folder, named after both paths.
pub(super) fn staged(from: &Path, to: &Path) -> PathBuf {
    let mut sha = Sha256::new();
    sha.update(from.as_os_str().as_encoded_bytes());
    sha.update([0]);
    sha.update(to.as_os_str().as_encoded_bytes());
    let hex: String = sha.finalize()[..8]
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    let name = format!(".rawmakase-reserve-{hex}");
    match to.parent() {
        Some(dir) => dir.join(name),
        None => PathBuf::from(name),
    }
}

/// Renames file `from` to `to`, where nothing has the name, by a
/// placeholder there. `reserved` runs once the placeholder is linked at
/// `to`, before its staged file is removed; an error from it is taken for
/// the rename's, for tests.
pub(super) fn rename(
    from: &Path,
    to: &Path,
    reserved: &mut dyn FnMut() -> Result<(), FsError>,
) -> Result<(), FsError> {
    let stage = staged(from, to);
    let wanted = marker(from, to);
    // What an earlier try of this rename left goes first.
    if placeholder(to, &wanted)? {
        std::fs::remove_file(to)?;
    }
    if !clear_stage(&stage, &wanted)? {
        return Err(FsError::Io(std::io::Error::other(format!(
            "{} is in the way",
            stage.display()
        ))));
    }
    let mut file = super::create_file(&stage)?;
    let written = file.write_all(&wanted);
    drop(file);
    if let Err(e) = written {
        let _ = clear_stage(&stage, &wanted);
        return Err(e.into());
    }
    if let Err(e) = std::fs::hard_link(&stage, to) {
        let _ = clear_stage(&stage, &wanted);
        // A file system without hard links can't take a name this way.
        return Err(match no_links(&e) {
            true => FsError::Unsupported,
            false => e.into(),
        });
    }
    // The staged file is removed before the rename, or the rename doesn't
    // happen, and what it left is settled, here or by its caller.
    let ready = reserved()
        .and_then(|()| match clear_stage(&stage, &wanted)? {
            true => Ok(()),
            false => Err(FsError::Io(std::io::Error::other(format!(
                "{} is in the way",
                stage.display()
            )))),
        })
        .and_then(|()| match placeholder(to, &wanted)? {
            true => Ok(()),
            // Another program replaced the placeholder: its file stays.
            false => Err(FsError::AlreadyExists),
        });
    let renamed = ready.and_then(|()| Ok(std::fs::rename(from, to)?));
    if let Err(e) = renamed {
        if !matches!(e, FsError::AlreadyExists) {
            let _ = settle(from, to);
        }
        return Err(e);
    }
    Ok(())
}

/// Removes what a [`rename`] of `from` to `to` cut short left: its
/// placeholder at `to`, and its staged file. Anything else is left as it
/// is.
pub(super) fn settle(from: &Path, to: &Path) -> Result<(), FsError> {
    let wanted = marker(from, to);
    let _ = clear_stage(&staged(from, to), &wanted)?;
    if placeholder(to, &wanted)? {
        std::fs::remove_file(to)?;
    }
    Ok(())
}

/// Removes staged file `stage` where it holds `wanted`, all of it or its
/// start; whether nothing is there now.
fn clear_stage(stage: &Path, wanted: &[u8]) -> Result<bool, FsError> {
    match held(stage, wanted.len())? {
        None => Ok(true),
        Some(bytes) if wanted.starts_with(&bytes) => {
            std::fs::remove_file(stage)?;
            Ok(true)
        }
        Some(_) => Ok(false),
    }
}

/// Whether `to` is a placeholder holding `wanted`, whole.
fn placeholder(to: &Path, wanted: &[u8]) -> Result<bool, FsError> {
    Ok(held(to, wanted.len())?.is_some_and(|bytes| bytes == wanted))
}

/// What regular file `path` holds, read up to one byte past `most`; `None`
/// where nothing has its name. Anything but a regular file is taken for one
/// holding what no marker does.
fn held(path: &Path, most: usize) -> Result<Option<Vec<u8>>, FsError> {
    let m = match std::fs::symlink_metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !m.is_file() || m.len() > most as u64 {
        return Ok(Some(vec![0; most + 1]));
    }
    let mut bytes = Vec::with_capacity(most);
    std::fs::File::open(path)?
        .take(most as u64 + 1)
        .read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// Whether a failed hard link says the volume has none.
#[cfg(unix)]
fn no_links(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::EPERM | libc::ENOSYS | libc::EOPNOTSUPP)
    )
}

#[cfg(not(unix))]
fn no_links(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::Unsupported
}
