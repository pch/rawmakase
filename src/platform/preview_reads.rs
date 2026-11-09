//! Whether a RAW is shown from its embedded preview read alone, without its
//! sensor data (Library thumbnails), and its photo info read from its header
//! alone (the backfill): on network volumes (SMB, NFS, WebDAV, FUSE; see
//! `rawmakase_fs::VolumeFacts::local`), where reading the whole file to show
//! its preview takes most of the time. On local volumes one read of the whole
//! file is as fast, and stays as it was.
//!
//! `RAWMAKASE_PREVIEW_READS` overrides it: `always`, `never`, or `auto` (the
//! default), e.g. to compare the two on one volume.
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Auto,
    Always,
    Never,
}

fn mode() -> Mode {
    static MODE: OnceLock<Mode> = OnceLock::new();
    *MODE.get_or_init(|| {
        match std::env::var("RAWMAKASE_PREVIEW_READS")
            .unwrap_or_default()
            .to_ascii_lowercase()
            .as_str()
        {
            "always" => Mode::Always,
            "never" => Mode::Never,
            _ => Mode::Auto,
        }
    })
}

/// Whether the RAW at `path` is shown from its embedded preview alone. Asks
/// the file system once per folder; that can wait on a stalled network mount,
/// so call it off the UI thread.
pub(crate) fn previews_only(path: &Path) -> bool {
    match mode() {
        Mode::Always => true,
        Mode::Never => false,
        Mode::Auto => !local(path.parent().unwrap_or(path)),
    }
}

/// Whether `folder` is on a local volume, as last found; a folder whose volume
/// can't be asked counts as local, so it is read as before.
fn local(folder: &Path) -> bool {
    static KNOWN: OnceLock<Mutex<HashMap<PathBuf, bool>>> = OnceLock::new();
    let known = KNOWN.get_or_init(Default::default);
    if let Some(local) = known
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(folder)
    {
        return *local;
    }
    let local = rawmakase_fs::volume_facts(folder).map_or(true, |f| f.local);
    known
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(folder.to_path_buf(), local);
    local
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_local_folder_is_read_as_before() {
        let dir = tempfile::tempdir().unwrap();
        assert!(super::local(dir.path()));
        // Asked once, then known.
        assert!(super::local(dir.path()));
    }
}
