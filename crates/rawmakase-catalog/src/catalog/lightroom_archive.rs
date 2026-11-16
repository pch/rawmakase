//! Removes the copy of a Lightroom catalog that earlier releases kept inside
//! ours (`sources.original_catalog`), once, as the catalog opens. Our catalog
//! is canonical: nothing reads that copy any more, and it can be as large as
//! the Lightroom catalog itself.
use super::db::{Reads, sql};
use super::{Catalog, OpeningProgress};
use anyhow::Result;

/// Set in `meta` once the archived Lightroom catalog has been emptied.
const ARCHIVE_REMOVED: &str = "lightroom_archive_removed";
/// Set in `meta` while the space the archive left is still inside the file:
/// compacting failed, or hasn't run yet, and is tried on the next open.
const COMPACT_PENDING: &str = "compact_pending";

impl Catalog {
    /// Empties an archived Lightroom catalog, then compacts the file once so
    /// it gives the space back. Called where a catalog is opened for use (the
    /// app's catalog session, the command line), not by every connection: a
    /// compaction that failed is tried again on the next of those opens.
    ///
    /// The schema doesn't change, and an older release reads an empty archive
    /// as nothing to read. Failing here doesn't stop the catalog opening:
    /// the reason is returned, to be shown, and the file is left as it was
    /// (`VACUUM` either completes or leaves it untouched).
    pub fn remove_lightroom_archive(
        &mut self,
        progress: &mut dyn FnMut(OpeningProgress),
    ) -> Option<String> {
        self.empty_archive()
            .and_then(|()| self.compact_if_pending(progress))
            .err()
            .map(|e| format!("{e:#}"))
    }

    fn empty_archive(&mut self) -> Result<()> {
        let archived: i64 = self.db.read_one(
            sql!("SELECT count(*) FROM sources WHERE length(original_catalog) > 0"),
            &[],
        )?;
        if archived == 0 {
            return Ok(());
        }
        self.db.write(|w| {
            w.execute(
                sql!("UPDATE sources SET original_catalog=X'' WHERE length(original_catalog) > 0"),
                &[],
            )?;
            super::set_meta(w, ARCHIVE_REMOVED, "1")?;
            super::set_meta(w, COMPACT_PENDING, "1")
        })
    }

    fn compact_if_pending(&mut self, progress: &mut dyn FnMut(OpeningProgress)) -> Result<()> {
        if self.meta(COMPACT_PENDING)?.is_none() {
            return Ok(());
        }
        progress(OpeningProgress::Compacting);
        #[cfg(test)]
        anyhow::ensure!(
            !faults::FAIL_COMPACTION.with(|f| f.get()),
            "No space left on device"
        );
        self.db.vacuum()?;
        self.db.write(|w| {
            w.execute(sql!("DELETE FROM meta WHERE key=?"), &[&COMPACT_PENDING])?;
            Ok(())
        })
    }
}

#[cfg(test)]
pub(super) mod faults {
    use std::cell::Cell;
    thread_local! {
        /// Compacting fails, as it does without enough free space.
        pub(in crate::catalog) static FAIL_COMPACTION: Cell<bool> = const { Cell::new(false) };
    }
}
