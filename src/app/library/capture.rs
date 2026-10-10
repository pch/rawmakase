//! Capture times for photos added from folders. Adding a folder records only
//! file names, so the dates are read from the files afterwards, in the
//! background, a batch at a time. Photos imported from Lightroom already have
//! theirs.
use super::Library;
use crate::catalog::FolderId;
use std::collections::HashMap;

impl Library {
    /// Once it is known which photos are online, filters again and reads the
    /// capture times still missing, including ones that were offline before.
    pub(super) fn availability_known(&mut self) {
        self.filter();
        self.note_missing_folders();
        // An original back online renders where it failed before.
        self.screen.retry_failed();
        self.session.retry_capture_times();
        self.start_capture_times();
        self.start_photo_info();
    }
    /// Says once, after the catalog opens, how many folders with photos have
    /// none of them on this computer, unless something else is being said.
    fn note_missing_folders(&mut self) {
        if std::mem::replace(&mut self.missing_noted, true) || !self.message.is_empty() {
            return;
        }
        // One pass over the photos: whether each folder has one online.
        let mut online: HashMap<FolderId, bool> = HashMap::new();
        for p in &self.session.photos {
            *online.entry(p.folder).or_default() |= self.is_available(&p.path);
        }
        let missing = self
            .session
            .folders
            .iter()
            .filter(|f| online.get(&f.id) == Some(&false))
            .count();
        if missing > 0 {
            self.message = format!(
                "{} found on this computer. Locate them in Preferences › Catalog › \
                 Folder locations, or right-click a folder.",
                super::super::widgets::plural(missing, "folder isn't", "folders aren't")
            );
        }
    }
    pub(super) fn start_capture_times(&mut self) {
        let availability = &self.availability;
        self.session
            .start_capture_times(|path| availability.is_available(path));
    }
    /// Saves the capture times read so far and sorts the photos again.
    pub(super) fn poll_capture_times(&mut self) {
        let polled = self.session.poll_capture_times();
        if let Some(e) = polled.error {
            // Read again after the next online check, which clears the
            // photos tried.
            self.message = format!("Capture times could not be saved: {e}");
        }
        if !polled.saved.is_empty() {
            self.resort_by_capture_time();
        }
        if polled.start_again {
            // Photos added while it ran.
            self.start_capture_times();
        }
    }
    /// Re-sorts after capture times were filled in or read again, as the
    /// catalog orders photos, keeping the selected photo selected and where it
    /// was on screen.
    pub(super) fn resort_by_capture_time(&mut self) {
        self.sort_keys = None;
        self.resort_in_place(|library| {
            library.session.photos.sort_by(|a, b| {
                (&a.captured, &a.filename, a.id).cmp(&(&b.captured, &b.filename, b.id))
            });
        });
    }
}
