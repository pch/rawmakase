//! Lightroom's Grid selection: an active photo, the photos selected with it,
//! and the anchor a Shift range starts from.
use super::Library;
use crate::catalog::PhotoId;
use eframe::egui::{self, Key};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq)]
pub(super) struct Selection {
    /// The photo the panels show and the keys move from; always selected.
    pub active: Option<PhotoId>,
    pub selected: HashSet<PhotoId>,
    pub anchor: Option<PhotoId>,
}
impl Selection {
    /// Without photo `id`, which is gone. A selection left with other photos keeps
    /// one of them active (and the anchor), as it is always selected.
    pub(super) fn forget_photo(&mut self, id: PhotoId) {
        self.selected.remove(&id);
        let survivor = self.selected.iter().min().copied();
        if self.active == Some(id) {
            self.active = survivor;
        }
        if self.anchor == Some(id) {
            self.anchor = self.active.or(survivor);
        }
    }
}

/// How a grid cell is drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Mark {
    None,
    Selected,
    /// The active photo, drawn lighter than the rest of the selection.
    Active,
}

/// A grid key: where it moves the active photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum Step {
    By(isize),
    /// Page Up and Page Down: whole rows in the same column, stopping at the
    /// first or last photo.
    Rows(isize),
    Home,
    End,
}

impl Library {
    /// The active photo.
    pub(crate) fn selected(&self) -> Option<PhotoId> {
        self.selection.active
    }
    /// Selects only `id`, or nothing.
    pub(crate) fn select(&mut self, id: Option<PhotoId>) {
        self.selection = Selection {
            active: id,
            selected: id.into_iter().collect(),
            anchor: id,
        };
    }
    /// Makes `id` active and in view: kept in the selection when it is shown,
    /// else selected alone with the filters that hide it cleared.
    pub(crate) fn reveal(&mut self, id: PhotoId) {
        if self
            .visible
            .iter()
            .any(|i| self.session.photos[*i].id == id)
        {
            self.make_active(id);
        } else {
            self.show(id);
        }
    }
    /// Makes `id` active, keeping the selection when it is part of it.
    pub(crate) fn make_active(&mut self, id: PhotoId) {
        if self.selection.selected.contains(&id) {
            self.selection.active = Some(id);
            self.selection.anchor = Some(id);
        } else {
            self.select(Some(id));
        }
    }
    /// The selected photos in display order; the active one alone if somehow
    /// nothing else is selected.
    pub(super) fn selected_ids(&self) -> Vec<PhotoId> {
        let ids: Vec<PhotoId> = self
            .visible
            .iter()
            .map(|i| self.session.photos[*i].id)
            .filter(|id| self.selection.selected.contains(id))
            .collect();
        if ids.is_empty() {
            self.selection.active.into_iter().collect()
        } else {
            ids
        }
    }
    pub(super) fn mark(&self, id: PhotoId) -> Mark {
        if self.selection.active == Some(id) {
            Mark::Active
        } else if self.selection.selected.contains(&id) {
            Mark::Selected
        } else {
            Mark::None
        }
    }
    fn position(&self, id: PhotoId) -> Option<usize> {
        self.visible
            .iter()
            .position(|i| self.session.photos[*i].id == id)
    }
    /// The shown photos from `a` to `b`, either way round.
    fn range(&self, a: PhotoId, b: PhotoId) -> Vec<PhotoId> {
        let (Some(a), Some(b)) = (self.position(a), self.position(b)) else {
            return vec![b];
        };
        self.visible[a.min(b)..=a.max(b)]
            .iter()
            .map(|i| self.session.photos[*i].id)
            .collect()
    }
    /// A click on a thumbnail: Cmd toggles it, Shift selects the range from
    /// the anchor (Cmd+Shift adds the range), and a plain click selects it
    /// alone, unless it is already selected, when it only becomes active.
    pub(super) fn click(&mut self, id: PhotoId, modifiers: egui::Modifiers) {
        let anchor = self.selection.anchor.or(self.selection.active);
        if modifiers.shift
            && let Some(anchor) = anchor
        {
            let range = self.range(anchor, id);
            if !modifiers.command {
                self.selection.selected.clear();
            }
            self.selection.selected.extend(range);
            self.selection.active = Some(id);
        } else if modifiers.command {
            if self.selection.selected.remove(&id) {
                if self.selection.active == Some(id) {
                    self.selection.active = self.nearest_selected(id);
                }
            } else {
                self.selection.selected.insert(id);
                self.selection.active = Some(id);
            }
            self.selection.anchor = self.selection.active;
        } else if self.selection.selected.contains(&id) && self.selection.selected.len() > 1 {
            self.selection.active = Some(id);
            self.selection.anchor = Some(id);
        } else {
            self.select(Some(id));
        }
    }
    /// The selected photo shown nearest after `id`, else before it.
    fn nearest_selected(&self, id: PhotoId) -> Option<PhotoId> {
        let at = self.position(id)?;
        let selected = |i: &usize| {
            self.selection
                .selected
                .contains(&self.session.photos[*i].id)
        };
        self.visible[at + 1..]
            .iter()
            .find(|i| selected(i))
            .or_else(|| self.visible[..at].iter().rev().find(|i| selected(i)))
            .map(|i| self.session.photos[*i].id)
    }
    pub(in crate::app) fn select_all(&mut self) {
        self.selection.selected = self
            .visible
            .iter()
            .map(|i| self.session.photos[*i].id)
            .collect();
        if self.selection.active.is_none() {
            self.selection.active = self.visible.first().map(|i| self.session.photos[*i].id);
        }
        self.selection.anchor = self.selection.active;
    }
    /// Lightroom's `/`: deselects the active photo; the next selected one
    /// becomes active.
    pub(super) fn deselect_active(&mut self) {
        if let Some(id) = self.selection.active {
            let next = self.nearest_selected(id);
            self.selection.selected.remove(&id);
            self.selection.active = next;
            self.selection.anchor = next;
        }
    }
    /// Moves the active photo; with `extend` (Shift) the selection grows from
    /// the anchor, otherwise the new photo is selected alone.
    pub(super) fn step(&mut self, step: Step, extend: bool) {
        let last = self.visible.len().saturating_sub(1);
        let to = match (step, self.selection.active.and_then(|id| self.position(id))) {
            (_, _) if self.visible.is_empty() => return,
            (Step::Home, _) => 0,
            (Step::End, _) => last,
            (Step::By(_) | Step::Rows(_), None) => 0,
            // Up and Down stay put at the first or last row rather than
            // jumping sideways; Left and Right stop at the ends.
            (Step::By(delta), Some(at)) => match at.checked_add_signed(delta) {
                Some(to) if to <= last => to,
                _ if delta.abs() > 1 => at,
                Some(_) => last,
                None => 0,
            },
            (Step::Rows(rows), Some(at)) => {
                let columns = self.grid_columns.max(1) as isize;
                at.checked_add_signed(rows * columns)
                    .map_or(0, |to| to.min(last))
            }
        };
        let id = self.session.photos[self.visible[to]].id;
        if let Step::By(delta) | Step::Rows(delta) = step {
            self.loupe_direction = if delta < 0 { -1 } else { 1 };
        }
        if extend && let Some(anchor) = self.selection.anchor.or(self.selection.active) {
            self.selection.selected = self.range(anchor, id).into_iter().collect();
            self.selection.anchor = Some(anchor);
            self.selection.active = Some(id);
        } else {
            self.select(Some(id));
        }
        self.scroll_to_active = true;
    }
    /// After filtering: drops photos no longer shown from the selection.
    pub(super) fn keep_shown_selected(&mut self) {
        let shown: HashSet<PhotoId> = self
            .visible
            .iter()
            .map(|i| self.session.photos[*i].id)
            .collect();
        self.selection.selected.retain(|id| shown.contains(id));
        // The first photo still selected takes over from a hidden active one.
        if self.selection.active.is_some_and(|id| !shown.contains(&id)) {
            self.selection.active = self
                .visible
                .iter()
                .map(|i| self.session.photos[*i].id)
                .find(|id| self.selection.selected.contains(id));
        }
        if self.selection.anchor.is_some_and(|id| !shown.contains(&id)) {
            self.selection.anchor = self.selection.active;
        }
    }
    /// The Grid's selection keys: Cmd+A, Cmd+D, `/`, arrows, Page Up and Page
    /// Down, Home and End;
    /// in the Loupe, moving and its own zoom.
    pub(in crate::app) fn selection_keys(&mut self, ctx: &egui::Context) {
        // A menu or popup takes the keys first, Escape above all.
        if egui::Popup::is_any_open(ctx) {
            return;
        }
        let presses = presses(ctx);
        // Cmd+L turns the filter bar off and on, in the grid and the Loupe,
        // and from the Search field too.
        for press in &presses {
            let cmd_l =
                press.key == Key::L && press.modifiers.matches_exact(egui::Modifiers::COMMAND);
            if cmd_l && !press.repeat {
                self.toggle_filters()
            }
        }
        if ctx.text_edit_focused() {
            return;
        }
        if self.survey.open {
            self.survey_keys(&presses);
        } else if self.compare.open {
            self.compare_keys(&presses);
        } else if self.loupe.open {
            self.loupe_keys(&presses);
        } else {
            self.grid_keys(&presses);
        }
    }
    fn grid_keys(&mut self, presses: &[Press]) {
        let columns = self.grid_columns.max(1) as isize;
        let page = self.grid_page_rows.max(1) as isize;
        for press in presses {
            let shift = press.modifiers.shift;
            match (press.key, press.modifiers.command) {
                (Key::E | Key::Enter, false) => self.open_loupe(),
                (Key::C, false) if !press.modifiers.any() => self.open_compare(),
                (Key::N, false) if !press.modifiers.any() => self.open_survey(),
                (Key::J, false) if !press.modifiers.any() && !press.repeat => {
                    self.cell_style = self.cell_style.next()
                }
                // The grid applies B to every selected photo, once per press.
                (Key::B, _) if !press.repeat => {
                    self.quick_key(press.modifiers, self.selected_ids())
                }
                // Once per press: a held Delete must not ask again.
                (Key::Delete | Key::Backspace, _) if !press.repeat => {
                    let ids = self.selected_ids();
                    self.delete_press(press.modifiers, ids)
                }
                (Key::F2, false) if !press.repeat && !press.modifiers.any() => {
                    let ids = self.selected_ids();
                    self.request_rename(ids)
                }
                // Lightroom's stacks: Cmd+G groups, Cmd+Shift+G unstacks,
                // S collapses or expands and Shift+S moves to the top.
                (Key::G, true) if !press.repeat && !press.modifiers.alt => {
                    let action = if shift {
                        super::StackAction::Unstack
                    } else {
                        super::StackAction::Group
                    };
                    self.stack_key(action, self.selected_ids())
                }
                (Key::S, false)
                    if !press.repeat && !press.modifiers.alt && !press.modifiers.ctrl =>
                {
                    let action = if shift {
                        super::StackAction::MoveToTop
                    } else {
                        super::StackAction::Toggle
                    };
                    self.stack_key(action, self.selected_ids())
                }
                (Key::A, true) => self.select_all(),
                (Key::D, true) => self.select(None),
                (Key::Slash, false) => self.deselect_active(),
                (Key::ArrowLeft, false) => self.step(Step::By(-1), shift),
                (Key::ArrowRight, false) => self.step(Step::By(1), shift),
                (Key::ArrowUp, false) => self.step(Step::By(-columns), shift),
                (Key::ArrowDown, false) => self.step(Step::By(columns), shift),
                (Key::PageUp, false) => self.step(Step::Rows(-page), shift),
                (Key::PageDown, false) => self.step(Step::Rows(page), shift),
                (Key::Home, false) => self.step(Step::Home, shift),
                (Key::End, false) => self.step(Step::End, shift),
                _ => {}
            }
        }
    }
    /// The Loupe moves one photo at a time; the editor handles its zoom,
    /// as in Develop.
    fn loupe_keys(&mut self, presses: &[Press]) {
        for press in presses {
            match (press.key, press.modifiers.command) {
                (Key::ArrowLeft | Key::ArrowUp, false) => self.step(Step::By(-1), false),
                (Key::ArrowRight | Key::ArrowDown, false) => self.step(Step::By(1), false),
                (Key::Home, false) => self.step(Step::Home, false),
                (Key::End, false) => self.step(Step::End, false),
                (Key::Escape, _) => self.close_loupe(),
                // The Loupe's photo, once per press.
                (Key::Delete | Key::Backspace, _) if !press.repeat => {
                    let ids = self.selection.active.into_iter().collect();
                    self.delete_press(press.modifiers, ids)
                }
                (Key::F2, false) if !press.repeat && !press.modifiers.any() => {
                    let ids = self.selection.active.into_iter().collect();
                    self.request_rename(ids)
                }
                (Key::C, false) if !press.modifiers.any() => self.open_compare(),
                (Key::N, false) if !press.modifiers.any() => self.open_survey(),
                // The Loupe applies B to the photo shown, once per press.
                (Key::B, _) if !press.repeat => {
                    let ids = self.selection.active.into_iter().collect();
                    self.quick_key(press.modifiers, ids)
                }
                // Once per press: a held I must not flicker through them.
                (Key::I, false) if !press.modifiers.any() && !press.repeat => {
                    self.cycle_loupe_info()
                }
                // As in the grid: S collapses or expands the photo's stack
                // and Shift+S moves it to the top.
                (Key::S, false)
                    if !press.repeat && !press.modifiers.alt && !press.modifiers.ctrl =>
                {
                    let action = if press.modifiers.shift {
                        super::StackAction::MoveToTop
                    } else {
                        super::StackAction::Toggle
                    };
                    self.stack_key(action, self.selection.active.into_iter().collect())
                }
                _ => {}
            }
        }
    }
}

/// A key pressed this frame, with the modifiers held for it: a quick Cmd+D
/// can arrive in the same frame as Cmd's release.
pub(super) struct Press {
    pub key: Key,
    pub modifiers: egui::Modifiers,
    pub repeat: bool,
}
fn presses(ctx: &egui::Context) -> Vec<Press> {
    ctx.input(|i| {
        i.events
            .iter()
            .filter_map(|e| match e {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    repeat,
                    ..
                } => Some(Press {
                    key: *key,
                    modifiers: *modifiers,
                    repeat: *repeat,
                }),
                _ => None,
            })
            .collect()
    })
}

#[cfg(test)]
mod forget_tests {
    use super::*;

    #[test]
    fn forgetting_the_active_photo_keeps_a_surviving_one_active() {
        let [a, b, gone] = [PhotoId(1), PhotoId(2), PhotoId(3)];
        let mut s = Selection {
            active: Some(gone),
            selected: [a, b, gone].into(),
            anchor: Some(gone),
        };
        s.forget_photo(gone);
        assert_eq!(s.active, Some(a));
        assert_eq!(s.anchor, Some(a));
        assert_eq!(s.selected, [a, b].into());
        let mut alone = Selection {
            active: Some(gone),
            selected: [gone].into(),
            anchor: Some(gone),
        };
        alone.forget_photo(gone);
        assert_eq!((alone.active, alone.anchor), (None, None));
        assert!(alone.selected.is_empty());
    }
}
