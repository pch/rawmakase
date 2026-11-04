//! Mutually exclusive foreground operations. Background previews are independent.
#[derive(Default)]
pub(in crate::app) enum Activity {
    #[default]
    Idle,
    ChoosingFile,
    /// A folder change (relink, clear or add) is writing the catalog. It ends as a
    /// dialog does, once the catalog has opened again.
    ChangingFolder,
    /// A batch edit (Sync Settings, Auto Settings or a preset on the selection) is
    /// preparing or writing photos' edits: moving to another photo or catalog waits
    /// for it. It ends when its worker reports back, cancelled or not.
    Syncing(std::sync::Arc<crate::app::library::sync::BatchProgress>),
    /// A file operation (Remove from Catalog, and later moving and trashing
    /// photos) is planned, confirmed or running: from just after the open
    /// edit was saved until its outcome is applied, nothing else writes the
    /// photos' rows (see `file_ops`).
    ChangingFiles,
}
impl Activity {
    pub(crate) fn is_busy(&self) -> bool {
        !matches!(self, Self::Idle)
    }
    pub(crate) fn is_dialog(&self) -> bool {
        matches!(self, Self::ChoosingFile | Self::ChangingFolder)
    }
    /// A folder change is writing the catalog: closing waits for it.
    pub(crate) fn is_changing_folder(&self) -> bool {
        matches!(self, Self::ChangingFolder)
    }
    pub(crate) fn begin_folder_change(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::ChangingFolder;
        true
    }
    pub(crate) fn begin_dialog(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::ChoosingFile;
        true
    }
    pub(crate) fn begin_sync(
        &mut self,
        progress: std::sync::Arc<crate::app::library::sync::BatchProgress>,
    ) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::Syncing(progress);
        true
    }
    pub(crate) fn is_syncing(&self) -> bool {
        matches!(self, Self::Syncing(_))
    }
    /// The running batch's progress and Cancel.
    pub(crate) fn batch(&self) -> Option<&crate::app::library::sync::BatchProgress> {
        match self {
            Self::Syncing(progress) => Some(progress),
            _ => None,
        }
    }
    /// Something else is writing the open photo's or other photos' rows:
    /// autosave, saving before navigation and undo wait for it.
    pub(crate) fn holds_catalog_writes(&self) -> bool {
        matches!(self, Self::Syncing(_) | Self::ChangingFiles)
    }
    pub(crate) fn begin_file_change(&mut self) -> bool {
        if self.is_busy() {
            return false;
        }
        *self = Self::ChangingFiles;
        true
    }
    /// A file operation holds the catalog: closing waits for it.
    pub(crate) fn is_changing_files(&self) -> bool {
        matches!(self, Self::ChangingFiles)
    }
    pub(crate) fn finish_file_change(&mut self) {
        if self.is_changing_files() {
            *self = Self::Idle;
        }
    }
    pub(crate) fn finish_sync(&mut self) {
        if self.is_syncing() {
            *self = Self::Idle;
        }
    }
    pub(crate) fn finish_dialog(&mut self) {
        if self.is_dialog() {
            *self = Self::Idle;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn batch() -> std::sync::Arc<crate::app::library::sync::BatchProgress> {
        std::sync::Arc::new(crate::app::library::sync::BatchProgress::new("Sync", 2))
    }
    #[test]
    fn a_file_dialog_blocks_another_until_it_closes() {
        let mut activity = Activity::default();
        assert!(activity.begin_dialog());
        assert!(!activity.begin_dialog());
        activity.finish_dialog();
        assert!(!activity.is_busy());
    }
    #[test]
    fn a_folder_change_ends_as_a_dialog_does() {
        let mut activity = Activity::default();
        assert!(activity.begin_folder_change());
        assert!(activity.is_changing_folder() && activity.is_dialog());
        assert!(!activity.begin_dialog() && !activity.begin_folder_change());
        activity.finish_dialog();
        assert!(!activity.is_busy());
    }
    #[test]
    fn a_file_change_holds_catalog_writes_until_it_finishes() {
        let mut activity = Activity::default();
        assert!(!activity.holds_catalog_writes());
        assert!(activity.begin_file_change());
        assert!(activity.holds_catalog_writes() && activity.is_busy());
        assert!(!activity.is_dialog() && !activity.is_syncing());
        assert!(
            !activity.begin_file_change()
                && !activity.begin_sync(batch())
                && !activity.begin_dialog()
        );
        // Only its own end ends it.
        activity.finish_dialog();
        activity.finish_sync();
        assert!(activity.is_changing_files());
        activity.finish_file_change();
        assert!(!activity.is_busy());
        assert!(activity.begin_sync(batch()));
        assert!(activity.holds_catalog_writes());
    }
    #[test]
    fn a_sync_keeps_dialogs_and_other_syncs_waiting() {
        let mut activity = Activity::default();
        assert!(activity.begin_sync(batch()));
        assert!(activity.is_busy() && !activity.is_dialog());
        assert!(!activity.begin_dialog() && !activity.begin_sync(batch()));
        activity.finish_dialog();
        assert!(activity.is_syncing());
        activity.finish_sync();
        assert!(!activity.is_busy());
    }
}
