//! Removes what a catalog holds that nothing reads, as it opens: the copy of a
//! Lightroom catalog earlier releases kept inside ours
//! (`sources.original_catalog`), once (our catalog is canonical, and that copy can
//! be as large as the Lightroom catalog itself), and the mask rasters and AI
//! Remove patches nothing names any more (#1112).
use super::db::{Reads, sql};
use super::{Catalog, OpeningProgress};
use anyhow::Result;

/// Set in `meta` once the archived Lightroom catalog has been emptied.
const ARCHIVE_REMOVED: &str = "lightroom_archive_removed";
/// Set in `meta` while the space the archive left is still inside the file:
/// compacting failed, or hasn't run yet, and is tried on the next open.
const COMPACT_PENDING: &str = "compact_pending";
/// Once deleting rasters leaves this many bytes free in the file, or a quarter of
/// it, the file is compacted; less is left as free pages that later writes reuse.
const COMPACT_WHEN_FREE: i64 = 32 << 20;
/// Set in `meta` to when the rasters were last looked through (Unix seconds).
const RASTERS_CHECKED: &str = "rasters_checked_at";
/// Set in `meta` to the rasters nothing named when they were last looked
/// through, joined by commas: the ones still unnamed next time are deleted.
const RASTERS_UNNAMED: &str = "rasters_unnamed";
/// How often the rasters are looked through. Doing it means inflating every
/// History, about half a second for a hundred well-edited photos, so not on every
/// open; the rasters left meanwhile wait at most this long.
const RASTER_CHECK_EVERY: u64 = 7 * 24 * 60 * 60;

impl Catalog {
    /// Empties an archived Lightroom catalog and deletes the rasters nothing
    /// names, then compacts the file once when that freed a lot, so it gives the
    /// space back. Called where a catalog is opened for use (the app's catalog
    /// session, the command line), before anything is edited, not by every
    /// connection: a compaction that failed is tried again on the next of those
    /// opens.
    ///
    /// The schema doesn't change, and an older release reads an empty archive
    /// as nothing to read. Failing here doesn't stop the catalog opening:
    /// the reason is returned, to be shown, and the file is left as it was
    /// (each deletion is one transaction, and `VACUUM` either completes or
    /// leaves it untouched).
    pub fn remove_unused_data(
        &mut self,
        progress: &mut dyn FnMut(OpeningProgress),
    ) -> Option<String> {
        self.empty_archive()
            .and_then(|()| self.delete_unused_rasters())
            .and_then(|()| self.compact_if_pending(progress))
            .err()
            .map(|e| format!("{e:#}"))
    }

    /// Deletes the mask rasters and patches no edit, History, snapshot or
    /// virtual copy has named ([`super::mask_assets::unnamed`]) at two looks
    /// through at least [`RASTER_CHECK_EVERY`] apart. The wait is for what no
    /// row records: another window's Undo back to a deleted spot, say, whose
    /// pixels that window may no longer hold either. A catalog with a table a
    /// later release added is left alone, as its rows may name some.
    fn delete_unused_rasters(&mut self) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        // A time ahead of now (a clock set back) doesn't put the check off.
        let recent = |checked: Option<String>| {
            checked
                .and_then(|at| at.parse::<u64>().ok())
                .is_some_and(|at| at <= now && now - at < RASTER_CHECK_EVERY)
        };
        // Read first without the write lock, so most opens don't take it.
        #[cfg(test)]
        let skip_first_read = faults::RACE_RASTER_CHECK.with(|f| f.get());
        #[cfg(not(test))]
        let skip_first_read = false;
        if !skip_first_read && recent(self.meta(RASTERS_CHECKED)?) {
            return Ok(());
        }
        let any: bool = self
            .db
            .read_one(sql!("SELECT EXISTS(SELECT 1 FROM bitmaps)"), &[])?;
        if !any || self.ensure_known_tables().is_err() {
            return Ok(());
        }
        // Immediate, and read again under the lock: another window opening the
        // catalog at the same time may have looked since, and two looks seconds
        // apart must not count as a week's wait.
        self.db.write_immediate(|w| {
            let checked: Option<String> = w.read_optional(
                sql!("SELECT value FROM meta WHERE key=?"),
                &[&RASTERS_CHECKED],
            )?;
            if recent(checked) {
                return Ok(());
            }
            super::set_meta(w, RASTERS_CHECKED, &now.to_string())?;
            let before: Option<String> = w.read_optional(
                sql!("SELECT value FROM meta WHERE key=?"),
                &[&RASTERS_UNNAMED],
            )?;
            let before: std::collections::HashSet<&str> =
                before.as_deref().unwrap_or("").split(',').collect();
            // A History that can't be read keeps every raster, and waits again.
            let unnamed = super::mask_assets::unnamed(w)?.unwrap_or_default();
            let (gone, waiting): (Vec<String>, Vec<String>) = unnamed
                .into_iter()
                .partition(|h| before.contains(h.as_str()));
            super::set_meta(w, RASTERS_UNNAMED, &waiting.join(","))?;
            if super::mask_assets::delete(w, &gone)? == 0 {
                return Ok(());
            }
            // The free pages include what earlier opens deleted, so small
            // deletions add up to a compaction.
            let file: i64 = w.read_one(
                sql!("SELECT page_count * page_size FROM pragma_page_count(), pragma_page_size()"),
                &[],
            )?;
            let free: i64 = w.read_one(
                sql!(
                    "SELECT freelist_count * page_size FROM pragma_freelist_count(), pragma_page_size()"
                ),
                &[],
            )?;
            if free >= COMPACT_WHEN_FREE || free * 4 >= file {
                super::set_meta(w, COMPACT_PENDING, "1")?;
            }
            Ok(())
        })
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
                sql!("UPDATE sources SET original_catalog=? WHERE length(original_catalog) > 0"),
                &[&Vec::<u8>::new()],
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
        /// Looking through the rasters goes ahead as if its first read of when
        /// they were last looked through came before another window's look.
        pub(in crate::catalog) static RACE_RASTER_CHECK: Cell<bool> = const { Cell::new(false) };
    }
}
