//! Lightroom's Compare (C): the select and a candidate side by side, to keep
//! the better of two. Left and Right move the candidate along the photos
//! shown, Up makes it the select, Down swaps the two. A click makes a photo
//! active: rating, flag and label keys go to it, and the panels show it.
//! Both are shown at the size of their half, with their edits (see `stage`).
use super::grid::filter_caption;
use super::stage::MARGIN;
use super::{Action, Library};
use crate::app::theme;
use crate::catalog::PhotoId;
use eframe::egui::{self, Rect, Vec2};

/// One of the two photos.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Side {
    #[default]
    Select,
    Candidate,
}
impl Side {
    fn name(self) -> &'static str {
        match self {
            Self::Select => "Select",
            Self::Candidate => "Candidate",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct Compare {
    pub open: bool,
    pub select: Option<PhotoId>,
    /// None when no other photo is shown.
    pub candidate: Option<PhotoId>,
    pub active: Side,
    /// The selection as Compare last set it; one changed since, by undo or
    /// a new virtual copy, is followed.
    synced: Option<super::selection::Selection>,
    /// Undo or redo restored a place: its selection is followed as it is.
    pub restored: bool,
}
impl Compare {
    fn id(&self, side: Side) -> Option<PhotoId> {
        match side {
            Side::Select => self.select,
            Side::Candidate => self.candidate,
        }
    }
}

impl Library {
    #[cfg(test)]
    pub(crate) fn compare_open(&self) -> bool {
        self.compare.open
    }
    /// C: the active photo as the select, beside the next photo selected
    /// with it, or else the next one shown.
    pub(crate) fn open_compare(&mut self) {
        let Some(select) = self
            .selection
            .active
            .or_else(|| self.visible.first().map(|i| self.session.photos[*i].id))
        else {
            return;
        };
        self.loupe.open = false;
        self.loupe.reset();
        self.survey.open = false;
        self.compare.open = true;
        // A place restored while Compare was closed is no news to it.
        self.compare.restored = false;
        self.seed_compare(select);
    }
    /// `select` as the select, beside the next photo selected with it, or
    /// else the next one shown; the select is active.
    fn seed_compare(&mut self, select: PhotoId) {
        let others: Vec<PhotoId> = self
            .selected_ids()
            .into_iter()
            .filter(|id| *id != select)
            .collect();
        let at = self
            .visible
            .iter()
            .position(|i| self.session.photos[*i].id == select);
        let after = |id: &PhotoId| {
            let position = self
                .visible
                .iter()
                .position(|i| self.session.photos[*i].id == *id);
            position > at
        };
        let candidate = others
            .iter()
            .find(|id| after(id))
            .or(others.first())
            .copied()
            .or_else(|| self.next_candidate(select, None, 1))
            .or_else(|| self.next_candidate(select, None, -1));
        self.compare.select = Some(select);
        self.compare.candidate = candidate;
        self.compare.active = Side::Select;
        self.sync_compare_selection();
    }
    /// Esc or G: back to the grid, with both photos selected.
    pub(crate) fn close_compare(&mut self) {
        if self.compare.open {
            self.compare.open = false;
            self.scroll_to_active = true;
        }
    }
    /// The photo `by` steps from `from` (or from the select) among those
    /// shown, passing over the select; None at either end.
    fn next_candidate(&self, select: PhotoId, from: Option<PhotoId>, by: isize) -> Option<PhotoId> {
        let ids: Vec<PhotoId> = self
            .visible
            .iter()
            .map(|i| self.session.photos[*i].id)
            .collect();
        let from = from.filter(|id| ids.contains(id)).unwrap_or(select);
        let mut at = ids.iter().position(|id| *id == from)? as isize;
        loop {
            at += by.signum();
            let id = *ids.get(usize::try_from(at).ok()?)?;
            if id != select {
                return Some(id);
            }
        }
    }
    /// Left or Right: the candidate before or after, which becomes active.
    pub(super) fn step_candidate(&mut self, by: isize) {
        let Some(select) = self.compare.select else {
            return;
        };
        if let Some(next) = self.next_candidate(select, self.compare.candidate, by) {
            self.compare.candidate = Some(next);
            self.compare.active = Side::Candidate;
            self.sync_compare_selection();
        }
    }
    /// Up: the candidate becomes the select, beside the next candidate.
    pub(super) fn make_select(&mut self) {
        let Some(chosen) = self.compare.candidate else {
            return;
        };
        self.compare.select = Some(chosen);
        self.compare.candidate = self
            .next_candidate(chosen, None, 1)
            .or_else(|| self.next_candidate(chosen, None, -1));
        self.compare.active = Side::Candidate;
        self.sync_compare_selection();
    }
    /// Down: the select and candidate change places; the active photo stays.
    pub(super) fn swap_compare(&mut self) {
        if self.compare.candidate.is_none() {
            return;
        }
        let compare = &mut self.compare;
        std::mem::swap(&mut compare.select, &mut compare.candidate);
        compare.active = match compare.active {
            Side::Select => Side::Candidate,
            Side::Candidate => Side::Select,
        };
        self.sync_compare_selection();
    }
    /// A filmstrip click: the select activates its side; any other photo
    /// becomes the candidate, as in Lightroom.
    pub(super) fn compare_pick(&mut self, id: PhotoId) {
        if Some(id) == self.compare.select {
            self.activate(Side::Select);
        } else {
            self.compare.candidate = Some(id);
            self.activate(Side::Candidate);
        }
    }
    fn activate(&mut self, side: Side) {
        if self.compare.id(side).is_some() {
            self.compare.active = side;
            self.sync_compare_selection();
        }
    }
    /// Selects the two photos, with the active one active, so the panels,
    /// the filmstrip and the keys follow Compare.
    pub(super) fn sync_compare_selection(&mut self) {
        let active = self.compare.id(self.compare.active).or(self.compare.select);
        self.selection.selected = [self.compare.select, self.compare.candidate]
            .into_iter()
            .flatten()
            .collect();
        self.selection.active = active;
        self.selection.anchor = active;
        self.compare.synced = Some(self.selection.clone());
    }
    /// A rating, flag or label key in Compare: the active photo only. With
    /// Shift, the candidate moves on.
    pub(crate) fn edit_compared(
        &mut self,
        edit: crate::app::photo_metadata::Edit,
        advance: bool,
    ) -> anyhow::Result<()> {
        let Some(id) = self.compare.id(self.compare.active) else {
            return Ok(());
        };
        let recorded = self.done.len();
        self.edit_metadata(id, edit, false)?;
        // The selection the edit left is Compare's own to reconcile, not
        // another command's to follow.
        self.compare.synced = Some(self.selection.clone());
        // A photo the filter now hides has already given way to the next.
        let hidden = !self.is_shown(id);
        self.keep_compared_shown();
        if advance && !hidden {
            self.step_candidate(1);
        }
        self.sync_compare_selection();
        // Redo returns to the pair the edit left.
        let place = self.place();
        if self.done.len() > recorded
            && let Some(command) = self.done.last_mut()
        {
            command.place_after = place;
        }
        Ok(())
    }
    fn is_shown(&self, id: PhotoId) -> bool {
        self.visible
            .iter()
            .any(|i| self.session.photos[*i].id == id)
    }
    /// Keeps Compare to the selection and the photos shown. A selection
    /// another command changed (undo, a menu, a new virtual copy) is
    /// followed, keeping the select while it is shown. A photo removed, or
    /// hidden by another source or filter, gives way: the candidate to the
    /// next photo shown, the select to the candidate, both to the photos
    /// shown. The active photo stays active wherever it is. Returns the
    /// select; None when nothing is shown.
    pub(super) fn keep_compared_shown(&mut self) -> Option<PhotoId> {
        let active = self.compare.id(self.compare.active);
        // A filter or source that hid the active photo pruned the selection
        // and moved it off that photo; that is reconciled below, keeping its
        // role. Any other change, such as undo restoring a place or a removed
        // copy's master being selected, is followed.
        let hidden = active.is_some_and(|id| self.photo(id).is_some() && !self.is_shown(id));
        let pruned = self
            .compare
            .synced
            .as_ref()
            .is_some_and(|synced| self.selection.selected.is_subset(&synced.selected));
        let restored = std::mem::take(&mut self.compare.restored);
        if self.compare.synced.as_ref() != Some(&self.selection)
            && (restored || !(hidden && pruned))
        {
            self.follow_selection();
        }
        let (active, side) = (self.compare.id(self.compare.active), self.compare.active);
        if self.compare.candidate.is_some_and(|id| !self.is_shown(id)) {
            self.compare.candidate = None;
        }
        if self.compare.select.is_some_and(|id| !self.is_shown(id)) {
            self.compare.select = self.compare.candidate.take();
        }
        if self.compare.select.is_none() {
            self.compare.select = self.visible.first().map(|i| self.session.photos[*i].id);
        }
        let select = self.compare.select?;
        if self.compare.candidate.is_none() {
            self.compare.candidate = self
                .next_candidate(select, None, 1)
                .or_else(|| self.next_candidate(select, None, -1));
        }
        // The active photo where it is now; one hidden passes its role on.
        self.compare.active = if active == Some(select) {
            Side::Select
        } else if active.is_some() && active == self.compare.candidate {
            Side::Candidate
        } else if active.is_some_and(|id| !self.is_shown(id)) && self.compare.id(side).is_some() {
            side
        } else {
            Side::Select
        };
        Some(select)
    }
    /// Takes up a selection another command made: the select stays while
    /// it is shown, beside the active photo or another one selected;
    /// otherwise Compare starts again from the active photo.
    fn follow_selection(&mut self) {
        let Some(active) = self.selection.active.filter(|id| self.is_shown(*id)) else {
            return;
        };
        let Some(select) = self.compare.select.filter(|id| self.is_shown(*id)) else {
            self.seed_compare(active);
            return;
        };
        if active == select {
            let other = self
                .selected_ids()
                .into_iter()
                .find(|id| *id != select && self.is_shown(*id));
            self.compare.candidate = other.or(self.compare.candidate);
            self.compare.active = Side::Select;
        } else {
            self.compare.candidate = Some(active);
            self.compare.active = Side::Candidate;
        }
        self.sync_compare_selection();
    }
    pub(super) fn compare_keys(&mut self, presses: &[super::selection::Press]) {
        use egui::Key;
        for press in presses {
            // B, Cmd+B and Cmd+Shift+B, for the active photo, once per press.
            if press.key == Key::B {
                if !press.repeat {
                    let ids = self.selection.active.into_iter().collect();
                    self.quick_key(press.modifiers, ids)
                }
                continue;
            }
            if press.modifiers.command || press.modifiers.alt {
                continue;
            }
            match press.key {
                Key::ArrowLeft => self.step_candidate(-1),
                Key::ArrowRight => self.step_candidate(1),
                Key::ArrowUp if !press.repeat => self.make_select(),
                Key::ArrowDown if !press.repeat => self.swap_compare(),
                Key::Escape => self.close_compare(),
                Key::E | Key::Enter => self.open_loupe(),
                Key::N if !press.modifiers.any() => self.open_survey(),
                _ => {}
            }
        }
    }
    /// Compare in place of the grid: the two photos, with a toolbar.
    pub(super) fn compare(&mut self, ui: &mut egui::Ui, chrome: bool) -> Action {
        let palette = theme::palette(ui.ctx());
        if self.keep_compared_shown().is_none() {
            self.close_compare();
            return Action::None;
        }
        self.sync_compare_selection();
        // On the photo's backdrop, as the view above it.
        if chrome {
            egui::Panel::bottom("library-compare-toolbar")
                .frame(
                    egui::Frame::new()
                        .fill(palette.photo_backdrop())
                        .inner_margin(egui::Margin::symmetric(10, 4)),
                )
                .show_separator_line(false)
                .show(ui, |ui| self.compare_toolbar(ui));
        }
        let area = ui.available_rect_before_wrap();
        ui.allocate_rect(area, egui::Sense::hover());
        ui.painter().rect_filled(area, 0., palette.photo_backdrop());
        let inner = area.shrink(MARGIN);
        let half = Vec2::new((inner.width() - MARGIN) / 2., inner.height());
        let panes = [
            (Side::Select, Rect::from_min_size(inner.min, half)),
            (
                Side::Candidate,
                Rect::from_min_size(inner.min + Vec2::new(half.x + MARGIN, 0.), half),
            ),
        ];
        for (side, rect) in panes {
            self.compare_pane(ui, side, rect);
        }
        Action::None
    }
    fn compare_toolbar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let button = |ui: &mut egui::Ui, text: &str, hover: &str, enabled: bool| {
            ui.add_enabled(
                enabled,
                egui::Button::new(egui::RichText::new(text).size(11.)).small(),
            )
            .on_hover_text(hover)
            .clicked()
        };
        ui.horizontal(|ui| {
            self.view_buttons(ui);
            ui.add_space(12.);
            let two = self.compare.candidate.is_some();
            if button(ui, "Swap", "Swap the select and the candidate (Down)", two) {
                self.swap_compare();
            }
            if button(ui, "Make Select", "Make the candidate the select (Up)", two) {
                self.make_select();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if button(ui, "Done", "Back to the grid (Esc)", true) {
                    self.close_compare();
                }
                ui.label(filter_caption(
                    &palette,
                    "Left / Right change the candidate",
                ));
            });
        });
    }
    /// One photo of the two, fitted to `rect` with its caption below.
    fn compare_pane(&mut self, ui: &mut egui::Ui, side: Side, rect: Rect) {
        let id = egui::Id::new(("library-compare", side.name()));
        let response = ui.interact(rect, id, egui::Sense::click());
        let photo = self.compare.id(side).and_then(|id| self.photo(id)).cloned();
        let Some(photo) = photo else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No other photo to compare",
                egui::FontId::proportional(12.),
                theme::palette(ui.ctx()).gray(120),
            );
            return;
        };
        if response.double_clicked() {
            self.activate(side);
            self.open_loupe();
            return;
        }
        if response.clicked() {
            self.activate(side);
        }
        let active = self.compare.active == side;
        self.stage_photo(ui, rect, &photo, Some(side.name()), active);
    }
}
