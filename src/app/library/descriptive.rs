//! Title, caption, creator, copyright, location and keyword changes made in
//! the Metadata and Keywording panels: to every photo selected in the Grid,
//! else to the active one. Each is one transaction where it can be, and one
//! command for the shared undo log, which puts back each photo's rows as they
//! were, absent ones included.
use super::{Library, Place};
use crate::app::ui::widgets::plural;
use crate::catalog::{MetadataSnapshot, PhotoId};
use crate::metadata::TextField;
use anyhow::{Result, ensure};

/// What one photo's files hold, as Read Metadata from Files reads them.
type FilesRead = (
    Option<crate::catalog::FileMetadata>,
    crate::catalog::SidecarReport,
);

/// Read Metadata from Files while its files are being read.
pub(super) struct Reread {
    /// What each photo's files hold, with the location its camera's EXIF
    /// gives when its XMP has none.
    reader:
        crate::catalog_session::background::Reader<(FilesRead, Option<crate::metadata::Location>)>,
    paths: std::collections::HashMap<PhotoId, std::path::PathBuf>,
    read: Vec<(PhotoId, std::path::PathBuf, crate::catalog::FileMetadata)>,
    /// The locations cameras' EXIF gave, for the photos without one.
    locations: Vec<(PhotoId, crate::metadata::Location)>,
    report: crate::catalog::SidecarReport,
    place: Place,
    /// The session's structural generation it started under: what it read
    /// is dropped once the catalog was read again meanwhile, as a photo's
    /// id may name another by then.
    generation: u64,
}

/// A descriptive metadata change, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct DescriptiveCommand {
    /// Orders it among other changes made in the same frame.
    pub sequence: u64,
    pub before: Vec<MetadataSnapshot>,
    pub after: Vec<MetadataSnapshot>,
    pub place_before: Place,
    pub place_after: Place,
    /// Rating, flag and label, for a change that set them too (Read
    /// Metadata from Files); empty otherwise.
    pub ratings_before: Vec<super::Metadata>,
    pub ratings_after: Vec<super::Metadata>,
    /// What changed, as the status line said it.
    pub summary: String,
}

impl DescriptiveCommand {
    /// A change made now, from `before` to `after`, that sets no ratings.
    fn new(
        before: Vec<MetadataSnapshot>,
        after: Vec<MetadataSnapshot>,
        place_before: Place,
        place_after: Place,
        summary: String,
    ) -> Self {
        Self {
            sequence: crate::edit_session::sequence(),
            before,
            after,
            place_before,
            place_after,
            ratings_before: Vec::new(),
            ratings_after: Vec::new(),
            summary,
        }
    }
    /// The change, setting rating, flag and label too.
    fn with_ratings(self, before: Vec<super::Metadata>, after: Vec<super::Metadata>) -> Self {
        Self {
            ratings_before: before,
            ratings_after: after,
            ..self
        }
    }
}

pub(crate) use crate::catalog_session::DescriptiveEdit;

/// Keywords typed as Lightroom takes them, separated by commas: a child
/// before its parents, "Child < Parent"; or, when an entry has a ">", its
/// parents first, "Parent > Child"; and either way any name may itself be a
/// path, top first, "Parent|Child". Returns their paths, top first.
pub(crate) fn parse_keywords(text: &str) -> Result<Vec<Vec<String>>> {
    text.split(',')
        .filter(|entry| !entry.trim().is_empty())
        .map(parse_keyword)
        .collect()
}

/// One keyword typed, as [`parse_keywords`] takes each: its path, top first.
pub(crate) fn parse_keyword(entry: &str) -> Result<Vec<String>> {
    let levels: Vec<&str> = if entry.contains('>') {
        entry.split('>').collect()
    } else {
        entry.split('<').rev().collect()
    };
    let path: Vec<String> = levels
        .into_iter()
        .flat_map(|level| level.split('|'))
        .map(|name| name.trim().to_string())
        .collect();
    ensure!(
        path.iter().all(|name| !name.is_empty()),
        "A keyword needs a name: \"{}\"",
        entry.trim()
    );
    Ok(path)
}

impl Library {
    /// Makes `edit` to the photos given and hands it to the undo log.
    pub(in crate::app) fn edit_descriptive(
        &mut self,
        ids: &[PhotoId],
        edit: DescriptiveEdit,
    ) -> Result<()> {
        self.edit_descriptive_at(ids, edit, None)
    }
    /// `edit_descriptive`, for an edit that belongs to `place`, where it was
    /// typed, rather than where the Library is now; undo returns there.
    pub(super) fn edit_descriptive_at(
        &mut self,
        ids: &[PhotoId],
        edit: DescriptiveEdit,
        place: Option<Place>,
    ) -> Result<()> {
        let ids: Vec<PhotoId> = ids
            .iter()
            .copied()
            .filter(|id| self.photo(*id).is_some())
            .collect();
        if ids.is_empty() {
            return Ok(());
        }
        let place_before = place.clone().unwrap_or_else(|| self.place());
        let what = match &edit {
            DescriptiveEdit::Text(TextField::Title, _) => "Title",
            DescriptiveEdit::Text(TextField::Caption, _) => "Caption",
            DescriptiveEdit::Text(TextField::Copyright, _) => "Copyright",
            DescriptiveEdit::Creators(_) => "Creator",
            DescriptiveEdit::ClearLocation => "Location cleared",
            DescriptiveEdit::AddKeywords(_) => "Keywords added",
            DescriptiveEdit::AddKeyword(_) => "Keyword added",
            DescriptiveEdit::RemoveKeyword(_) => "Keyword removed",
        };
        let crate::catalog_session::DescriptiveChange {
            before,
            after,
            listed,
        } = self.session.edit_descriptive(&ids, edit)?;
        self.filter();
        if before == after {
            return listed;
        }
        let summary = match ids.len() {
            1 => what.to_string(),
            n => format!("{n} photos · {what}"),
        };
        self.messages.note(summary.clone());
        self.descriptive_done.push(DescriptiveCommand::new(
            before,
            after,
            place_before,
            place.unwrap_or_else(|| self.place()),
            summary,
        ));
        self.fields.reload();
        // Recorded first: a change saved is one undo can reverse, even if
        // what is shown can't be read again.
        listed
    }
    /// Puts photos' descriptive metadata back, with their rating, flag and
    /// label where the command set them, for undo and redo, without
    /// recording a change of its own; `other` is the command's other side
    /// ([`crate::catalog::Catalog::restore_metadata`]).
    pub(in crate::app) fn restore_descriptive(
        &mut self,
        values: &[MetadataSnapshot],
        other: &[MetadataSnapshot],
        ratings: &[super::Metadata],
    ) -> Result<()> {
        // A photo removed since (a virtual copy) is left out.
        let values: Vec<MetadataSnapshot> = values
            .iter()
            .filter(|s| self.photo(s.photo).is_some())
            .cloned()
            .collect();
        // A photo removed since (a virtual copy) is left out of the ratings too.
        let ratings: Vec<super::Metadata> = ratings
            .iter()
            .filter(|(id, ..)| self.photo(*id).is_some())
            .cloned()
            .collect();
        self.session.restore_descriptive(&values, other, &ratings)?;
        self.resort_by_capture_time();
        self.filter();
        self.fields.reload();
        Ok(())
    }
    /// Lightroom's Read Metadata from Files, for the masters among `ids`:
    /// the files are read in the background, then what they have replaces
    /// the catalog's values, edits included (a location the camera's EXIF
    /// gives only where the catalog has none), as one command.
    pub(in crate::app) fn read_metadata_from_files(&mut self, ids: &[PhotoId]) -> Result<()> {
        let wanted: std::collections::HashSet<PhotoId> = ids.iter().copied().collect();
        let photos: Vec<(PhotoId, std::path::PathBuf)> = self
            .session
            .photos()
            .iter()
            .filter(|p| wanted.contains(&p.id) && p.master.is_none())
            .map(|p| (p.id, p.path.clone()))
            .collect();
        if photos.is_empty() {
            self.messages
                .warn("Virtual copies are never read from files");
            return Ok(());
        }
        if self.reread.is_some() {
            self.messages
                .warn("Still reading metadata from files; try again when it's done");
            return Ok(());
        }
        self.reread = Some(Reread {
            paths: photos.iter().cloned().collect(),
            reader: crate::catalog_session::background::Reader::start(
                photos,
                self.wake(),
                |path| {
                    let read = crate::catalog_session::read_file_metadata(path);
                    // The file is opened again only when the camera's counts.
                    let location =
                        crate::catalog_session::camera_location(read.0.as_ref(), &read.1, || {
                            crate::catalog_session::exif_location(path)
                        });
                    (read, location)
                },
            ),
            read: Vec::new(),
            locations: Vec::new(),
            report: Default::default(),
            place: self.place(),
            generation: self.session.generation(),
        });
        Ok(())
    }
    /// Stops Read Metadata from Files, dropping what it read, before a
    /// change that may give photos' ids to others.
    pub(in crate::app) fn cancel_reread(&mut self) {
        if self.reread.take().is_some() {
            self.messages
                .note("Read Metadata from Files stopped; nothing was read");
            self.reread_finished = true;
        }
    }
    /// [`Self::poll_reread`], for the app's tests.
    #[cfg(test)]
    pub(in crate::app) fn poll_reread_for_tests(&mut self) {
        self.poll_reread();
    }
    /// Takes in what the files read so far had, and once all are read,
    /// writes it as one command.
    pub(super) fn poll_reread(&mut self) {
        let Some(reread) = &mut self.reread else {
            return;
        };
        let (batch, done) = reread.reader.poll();
        for (id, ((read, report), location)) in batch {
            if let Some(location) = location {
                reread.locations.push((id, location));
            }
            reread.report.unreadable.extend(report.unreadable);
            reread.report.ignored.extend(report.ignored);
            if let Some(read) = read
                && let Some(path) = reread.paths.get(&id)
            {
                reread.read.push((id, path.clone(), read));
            }
        }
        if !done {
            return;
        }
        let reread = self.reread.take().unwrap();
        if let Err(e) = self.finish_reread(reread) {
            self.messages.fail(format!("Metadata not read: {e:#}"));
        }
        self.reread_finished = true;
    }
    fn finish_reread(&mut self, reread: Reread) -> Result<()> {
        if reread.generation != self.session.generation() {
            self.messages
                .warn("The catalog changed while its files were read; nothing was read");
            return Ok(());
        }
        // Photos removed, or made copies, while their files were read are
        // left out: copies are never read.
        let read: Vec<_> = reread
            .read
            .into_iter()
            .filter(|(id, ..)| self.photo(*id).is_some_and(|p| p.master.is_none()))
            .collect();
        let ids: Vec<PhotoId> = reread
            .paths
            .keys()
            .copied()
            .filter(|id| self.photo(*id).is_some())
            .collect();
        let wanted: std::collections::HashSet<PhotoId> = ids.iter().copied().collect();
        let before = self.session.catalog().metadata_snapshot(&ids)?;
        let ratings_before = self.ratings_by_id(&wanted);
        // A location the catalog has, or that was removed, stays.
        let locations: Vec<_> = reread
            .locations
            .into_iter()
            .filter(|(id, _)| self.photo(*id).is_some_and(|p| p.master.is_none()))
            .collect();
        let mut report = reread.report;
        let written = self.session.apply_file_metadata(&ids, &read, &locations)?;
        report.unreadable.extend(written.unreadable);
        self.resort_by_capture_time();
        self.filter();
        let after = self.session.catalog().metadata_snapshot(&ids)?;
        let ratings_after = self.ratings_by_id(&wanted);
        let n = ids.len();
        let mut summary = format!("Read metadata from {}", plural(n, "file", "files"));
        if let Some(problems) = report.summary() {
            summary.push_str(" · ");
            summary.push_str(&problems);
        }
        if report.summary().is_some() {
            self.messages
                .warn_with_detail(summary.clone(), report.details());
        } else {
            self.messages
                .done_with_detail(summary.clone(), report.details());
        }
        self.fields.reload();
        if before != after || ratings_before != ratings_after {
            self.descriptive_done.push(
                DescriptiveCommand::new(before, after, reread.place.clone(), reread.place, summary)
                    .with_ratings(ratings_before, ratings_after),
            );
        }
        Ok(())
    }
    /// Rating, flag and label of the photos in `wanted`, by id.
    fn ratings_by_id(&self, wanted: &std::collections::HashSet<PhotoId>) -> Vec<super::Metadata> {
        let mut found = super::metadata::ratings_of(
            self.session
                .photos()
                .iter()
                .filter(|p| wanted.contains(&p.id)),
        );
        found.sort_by_key(|m| m.0);
        found
    }
    /// The descriptive changes made since the last call, for the undo log.
    pub(in crate::app) fn take_descriptive_done(&mut self) -> Vec<DescriptiveCommand> {
        std::mem::take(&mut self.descriptive_done)
    }
}
