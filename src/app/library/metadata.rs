//! Rating, flag and label changes made through the Library: one photo from
//! Develop and the filmstrip, every selected photo from the grid. Each is one
//! transaction and is handed to the shared undo log once saved.
use super::{Library, Place};
use crate::app::library::photo_metadata::Edit;
use crate::catalog::{Photo, PhotoId};
use anyhow::Result;

/// Rating, flag and label of a photo.
pub(crate) type Metadata = (PhotoId, i32, i32, String);
/// A metadata change made through the Library, for the shared undo log.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct MetadataCommand {
    /// Orders it among Develop steps made in the same frame.
    pub sequence: u64,
    pub before: Vec<Metadata>,
    pub after: Vec<Metadata>,
    pub place_before: Place,
    pub place_after: Place,
    /// What changed, as the status line said it.
    pub summary: String,
}

impl Library {
    /// Sets the rating, flag or label of one photo, as Develop and the
    /// filmstrip do; see `edit_photos`.
    pub(crate) fn edit_metadata(
        &mut self,
        id: PhotoId,
        edit: Edit,
        advance: bool,
    ) -> Result<Option<PhotoId>> {
        self.edit_photos(&[id], edit, advance)
    }
    /// A metadata key in the Grid: Lightroom applies it to every selected
    /// photo.
    pub(crate) fn edit_selection(&mut self, edit: Edit, advance: bool) -> Result<Option<PhotoId>> {
        self.edit_photos(&self.selected_ids(), edit, advance)
    }
    /// Sets the rating, flag or label of `ids` in one transaction. A toggle
    /// takes its value from the active photo, so all of them end up alike.
    /// With `advance` (Shift), a single photo is followed by the next one
    /// shown. A photo the filter now hides leaves the selection; when the
    /// active one goes, the next one shown is selected. Returns the photo
    /// Develop moves to: the next one with `advance`, else the one selected in
    /// place of a hidden active photo.
    pub(in crate::app) fn edit_photos(
        &mut self,
        ids: &[PhotoId],
        edit: Edit,
        advance: bool,
    ) -> Result<Option<PhotoId>> {
        let Some(&first) = ids.first() else {
            return Ok(None);
        };
        let place_before = self.place();
        let lead = self
            .selection
            .active
            .filter(|id| ids.contains(id))
            .unwrap_or(first);
        let p = self
            .photo(lead)
            .ok_or_else(|| anyhow::anyhow!("Unknown photo"))?;
        let edit = edit.resolve(p);
        let changes: Vec<Metadata> = ids
            .iter()
            .filter_map(|id| self.photo(*id))
            .map(|p| {
                let (rating, flag, label) = edit.values(p);
                (p.id, rating, flag, label)
            })
            .collect();
        // Only a change to the active photo moves the selection: one made to
        // another (a filmstrip menu) leaves the photo shown where it is.
        let was_active = self.selection.active == Some(lead);
        let position = self
            .visible
            .iter()
            .position(|i| self.session.photos()[*i].id == lead);
        let following: Vec<_> = position.map_or_else(Vec::new, |at| {
            self.visible[at + 1..]
                .iter()
                .map(|i| self.session.photos()[*i].id)
                .filter(|id| !ids.contains(id))
                .collect()
        });
        let before = ratings_of(changes.iter().filter_map(|(id, ..)| self.photo(*id)));
        self.write_ratings(&changes)?;
        let text: String = match self.photo(lead) {
            Some(p) if changes.len() == 1 => format!(
                "{} · {} stars · {} · {}",
                p.filename,
                p.rating,
                flag_name(p.flag),
                label_name(&p.label)
            ),
            _ => format!("{} photos · {}", changes.len(), summary(&edit, &changes[0])),
        };
        self.messages.note(text);
        self.filter();
        // Sorted by what changed, the photo may have moved: keep it in view.
        if self.filters.sort != super::sort::Sort::CaptureTime {
            self.scroll_to_active = true;
        }
        let next = following.into_iter().find(|next| {
            self.visible
                .iter()
                .any(|i| self.session.photos()[*i].id == *next)
        });
        let still_visible = self
            .visible
            .iter()
            .any(|i| self.session.photos()[*i].id == lead);
        let advance = advance && changes.len() == 1;
        // Without Shift, the photo the filter hid gives way to the one shown.
        let mut moved = None;
        if was_active && (advance || !still_visible) {
            let to = next.or_else(|| {
                if still_visible {
                    Some(lead)
                } else {
                    self.visible.last().map(|i| self.session.photos()[*i].id)
                }
            });
            // Photos still selected and shown stay selected.
            if self.selection.selected.is_empty() || advance {
                self.select(to);
            } else if let Some(to) = to {
                self.make_active(to);
            }
            moved = to.filter(|to| *to != lead);
        }
        // A key that changed nothing (5 on a five-star photo) is no command:
        // it would only hide the real one before it and clear redo.
        if before == changes {
            return Ok(if advance { next } else { moved });
        }
        self.done.push(MetadataCommand {
            sequence: crate::edit_session::sequence(),
            before,
            after: changes,
            place_before,
            place_after: self.place(),
            summary: self.messages.line().to_string(),
        });
        Ok(if advance { next } else { moved })
    }
    /// The metadata changes made since the last call, for the shared undo log.
    pub(in crate::app) fn take_done(&mut self) -> Vec<MetadataCommand> {
        std::mem::take(&mut self.done)
    }
    /// Sets rating, flag and label for undo and redo, in one transaction,
    /// without recording a change of its own.
    pub(in crate::app) fn set_metadata(&mut self, values: &[Metadata]) -> Result<()> {
        // A photo removed since (a virtual copy) is left out.
        let values: Vec<Metadata> = values
            .iter()
            .filter(|(id, ..)| self.photo(*id).is_some())
            .cloned()
            .collect();
        self.write_ratings(&values)?;
        self.filter();
        Ok(())
    }
    /// Sets rating, flag and label in the catalog, then as shown.
    fn write_ratings(&mut self, values: &[Metadata]) -> Result<()> {
        self.session.set_ratings(values)
    }
}
/// Rating, flag and label of each photo given.
pub(super) fn ratings_of<'a>(photos: impl IntoIterator<Item = &'a Photo>) -> Vec<Metadata> {
    photos
        .into_iter()
        .map(|p| (p.id, p.rating, p.flag, p.label.clone()))
        .collect()
}
/// What a batch change set, as the status line says it.
fn summary(edit: &Edit, (_, rating, flag, label): &Metadata) -> String {
    match edit {
        Edit::Rating(_) => format!("{rating} stars"),
        Edit::RatingDelta(_) => "Rating changed".into(),
        Edit::Label(_) | Edit::ToggleLabel(_) => label_name(label).into(),
        _ => flag_name(*flag).into(),
    }
}
fn label_name(label: &str) -> &str {
    if label.is_empty() { "No label" } else { label }
}
fn flag_name(flag: i32) -> &'static str {
    match flag {
        1 => "Pick",
        -1 => "Reject",
        _ => "Unflagged",
    }
}
