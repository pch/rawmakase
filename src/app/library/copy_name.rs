//! The Copy Name of a virtual copy, edited in place in the Metadata panel like
//! Lightroom's; saved on Return or when focus leaves, and kept when the
//! selection moves or the save fails.
use super::rows::{ROW, VALUE_GRAY, caption_at, field_rect, font, panel_edit};
use crate::app::ui::theme;
use crate::catalog::{Photo, PhotoId};
use crate::catalog_session::CatalogSession;
use anyhow::Result;
use eframe::egui::{self, Vec2};

#[derive(Default)]
pub(super) struct CopyNames {
    /// The name being typed, for the photo it belongs to.
    pub(super) draft: Option<(PhotoId, String)>,
    /// Saving `draft` failed; it waits for the next commit rather than
    /// being retried, and discarded, as the selection moves.
    pub(super) failed: bool,
}
impl CopyNames {
    /// Forgets the draft, e.g. after the catalog was read again: names may
    /// have changed, and a removed copy's id can be reused.
    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
    /// Whether the draft is a name not saved yet: the field keeps the saved name
    /// as its draft while a copy is shown, which needs no saving.
    pub(super) fn is_unsaved(&self, photos: &[Photo]) -> bool {
        self.draft.as_ref().is_some_and(|(id, text)| {
            photos
                .iter()
                .any(|p| p.id == *id && p.master.is_some() && p.copy_name != text.trim())
        })
    }
    /// Saves a draft still being typed, e.g. when the Library panel goes away
    /// before the field loses focus. On failure the name stays pending, to be
    /// saved again or discarded. Returns whether a photo was renamed.
    pub(super) fn commit(&mut self, session: &mut CatalogSession) -> Result<bool> {
        let Some((id, text)) = &self.draft else {
            return Ok(false);
        };
        let (id, name) = (*id, text.trim().to_string());
        let mut renamed = false;
        if session
            .photos()
            .iter()
            .any(|p| p.id == id && p.master.is_some() && p.copy_name != name)
        {
            let saved = session.rename_copy(id, &name);
            self.failed = saved.is_err();
            saved?;
            renamed = true;
        }
        self.draft = Some((id, name));
        self.failed = false;
        Ok(renamed)
    }
    /// The Copy Name row for `photo`. Returns whether a photo was renamed, or
    /// the error that kept a pending name from being saved.
    pub(super) fn row(
        &mut self,
        ui: &mut egui::Ui,
        photo: &Photo,
        session: &mut CatalogSession,
    ) -> Result<bool> {
        let palette = theme::palette(ui.ctx());
        let (rect, _) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::hover());
        caption_at(ui, rect, "Copy Name");
        let mut outcome = Ok(false);
        if self.draft.as_ref().is_none_or(|(id, _)| *id != photo.id) {
            // Another copy was selected before the field lost focus: keep its
            // name. One that cannot be saved stays pending, and this copy's
            // name is shown but not editable until it is.
            if !self.failed {
                outcome = self.commit(session);
            }
            if !self.failed {
                self.draft = Some((photo.id, photo.copy_name.clone()));
            }
        }
        let field = field_rect(rect);
        let Some((_, text)) = self.draft.as_mut().filter(|(id, _)| *id == photo.id) else {
            ui.painter().text(
                egui::pos2(field.left() + 4., field.center().y),
                egui::Align2::LEFT_CENTER,
                &photo.copy_name,
                font(),
                palette.tone(VALUE_GRAY),
            );
            return outcome;
        };
        let response = ui.put(
            field,
            panel_edit(&palette, egui::TextEdit::singleline(text))
                .vertical_align(egui::Align::Center),
        );
        if response.lost_focus() {
            return Ok(self.commit(session)? || outcome?);
        }
        outcome
    }
}
