//! Opening a catalog for a command-line command: `render --catalog` and the
//! binary's catalog commands, which report an upgrade and its backup on standard
//! error rather than in a window. Nothing here reaches `app`
//! (`scripts/deps-closures.txt`).
use anyhow::Result;
use std::io::Write;
use std::path::Path;

/// Opens the catalog at `path` for a command, upgrading one of an earlier format
/// first and removing an archived Lightroom catalog: their progress, and where
/// the upgrade's backup is, go to `err` (the process's standard error). A failed
/// write of the progress is ignored; one of the notices after it is an error, so
/// the command doesn't go on without them.
pub fn open_catalog(path: &Path, err: &mut dyn Write) -> Result<crate::catalog::Catalog> {
    use crate::catalog::OpeningProgress;
    let mut last = None;
    let mut progress = |progress: OpeningProgress| {
        // A line per tenth of the way.
        let step = match progress {
            OpeningProgress::Upgrading { done, total } => Some(done * 10 / total.max(1)),
            OpeningProgress::Compacting => None,
        };
        if last != Some(step) {
            last = Some(step);
            let _ = writeln!(err, "{progress}");
        }
    };
    let mut catalog = crate::catalog::Catalog::open_reporting(
        path,
        &crate::catalog::locations::Computer::this(),
        &mut progress,
    )?;
    let archive = catalog.remove_lightroom_archive(&mut progress);
    if let Some(error) = archive {
        writeln!(
            err,
            "The catalog couldn't be made smaller: {error}. It is tried again the next time \
             the catalog opens."
        )?;
    }
    if let Some(upgraded) = catalog.upgraded() {
        writeln!(
            err,
            "Upgraded the catalog from format {} for this release. A copy of it as it was is at {}.",
            upgraded.from,
            upgraded.backup.display()
        )?;
        if let Some(earlier) = upgraded.earlier_backups_note() {
            writeln!(err, "{earlier}")?;
        }
        if upgraded.unreadable > 0 {
            writeln!(
                err,
                "{} stored edits could not be read and were left as they were.",
                upgraded.unreadable
            )?;
        }
        if let Some(error) = &upgraded.vacuum_error {
            writeln!(
                err,
                "The disk space it no longer needs could not be given back: {error}"
            )?;
        }
    }
    catalog.remove_earlier_backups();
    Ok(catalog)
}
