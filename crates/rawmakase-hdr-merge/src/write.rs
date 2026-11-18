//! Publishing a merge: its DNG staged in the destination folder, synced, the
//! sources checked unchanged, then renamed into place without replacing any
//! file. Nothing is published after a cancel or a failure; the staged file is
//! removed.
use crate::{Error, Merged, name};
use rawmakase_cancel::Cancel;
use rawmakase_model::storage::{self, Identity};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Where [`write_dng`] puts the merged DNG.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    /// In `folder`, named `<stem>-HDR.dng` as Lightroom names its merges, or
    /// `<stem>-HDR-2.dng`, `-3`… when that name is taken.
    Beside { folder: PathBuf, stem: String },
    /// Exactly this path, which must not exist.
    Exactly(PathBuf),
}

/// The most names [`Destination::Beside`] tries.
const MOST_NAMES: u32 = 1000;

/// Writes `merged`'s DNG to `destination` and returns its path. Fails with
/// [`Error::SourceChanged`] when a source no longer has the identity the merge
/// captured, and [`Error::Cancelled`] when `cancel` is set before it is in
/// place; either way nothing is published.
pub fn write_dng(
    merged: &Merged,
    destination: &Destination,
    cancel: Cancel<'_>,
) -> Result<PathBuf, Error> {
    let folder = match destination {
        Destination::Beside { folder, .. } => folder.as_path(),
        Destination::Exactly(path) => storage::parent_dir(path),
    };
    let failed = |e: &dyn std::fmt::Display| Error::Write {
        folder: folder.to_path_buf(),
        why: e.to_string(),
    };
    let mut staged = storage::staging_file(folder).map_err(|e| failed(&e))?;
    staged
        .write_all(&merged.dng)
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|e| failed(&e))?;
    for input in &merged.inputs {
        if Identity::read(&input.path).ok().as_ref() != Some(&input.identity) {
            return Err(Error::SourceChanged(name(&input.path)));
        }
    }
    if cancel.is_cancelled() {
        return Err(Error::Cancelled);
    }
    let candidates: Box<dyn Iterator<Item = PathBuf>> = match destination {
        Destination::Beside { folder, stem } => {
            Box::new((1..=MOST_NAMES).map(move |n| folder.join(numbered(stem, n))))
        }
        Destination::Exactly(path) => Box::new(std::iter::once(path.clone())),
    };
    let mut last = None;
    for path in candidates {
        match storage::persist_noclobber(staged, &path) {
            Ok(_) => {
                // The file is in place under its name: a folder that can't be
                // synced leaves only its durability across a crash in doubt,
                // which is no reason to report the merge as not saved.
                let _ = storage::sync_dir(folder);
                return Ok(path);
            }
            Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                staged = e.file;
                last = Some(path);
            }
            // Dropping the staged file removes it.
            Err(e) => return Err(failed(&e.error)),
        }
    }
    Err(Error::Exists(last.unwrap_or_else(|| folder.to_path_buf())))
}

/// The `n`th name (from 1) of a merge of `stem`.
fn numbered(stem: &str, n: u32) -> String {
    match n {
        1 => format!("{stem}-HDR.dng"),
        n => format!("{stem}-HDR-{n}.dng"),
    }
}

impl Destination {
    /// Beside the photo at `path` (the active photo of the bracket), named
    /// after it.
    pub fn beside(path: &Path) -> Self {
        Self::Beside {
            folder: storage::parent_dir(path).to_path_buf(),
            stem: path
                .file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn names_follow_lightroom() {
        assert_eq!(super::numbered("IMG_9814", 1), "IMG_9814-HDR.dng");
        assert_eq!(super::numbered("IMG_9814", 2), "IMG_9814-HDR-2.dng");
    }
}
