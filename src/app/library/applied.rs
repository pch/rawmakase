//! Which preset each photo's edit was last made with ([`Recipe::applied_preset`]),
//! read from the catalog when the Preset filter or the info panel needs it: the
//! Library's copy, as the sort order's keys are. Reading every photo's means
//! reading every edit's text, so after that only the photos whose edits changed
//! are read again (Sync and batches name them; Develop's are the photos it
//! opened), and a catalog read again is read whole.
//!
//! [`Recipe::applied_preset`]: crate::model::recipe::Recipe::applied_preset
use crate::catalog::{Catalog, PhotoId};
use crate::model::recipe::AppliedPreset;
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(super) struct AppliedPresets {
    /// Every photo's that names one, by the name shown
    /// ([`presets::display_name`](crate::presets::display_name)).
    all: Option<HashMap<PhotoId, String>>,
    /// One photo's, as the info panel shows it.
    one: Option<(PhotoId, Option<AppliedPreset>)>,
    /// Photos opened in Develop since the Library last looked: their edits may
    /// have changed there.
    opened: HashSet<PhotoId>,
    /// The photo open in Develop, which going back to it doesn't open again.
    in_develop: Option<PhotoId>,
}
impl AppliedPresets {
    /// Edits changed, or the catalog was read again: everything is read again.
    /// Photos opened in Develop are still read again on the way back: one may
    /// be open there now.
    pub(super) fn forget(&mut self) {
        self.all = None;
        self.one = None;
    }
    /// Photo `id` was opened in Develop, where its edit may change.
    pub(super) fn note_opened(&mut self, id: PhotoId) {
        self.opened.insert(id);
    }
    /// The photo open in Develop now, if any.
    pub(super) fn set_in_develop(&mut self, id: Option<PhotoId>) {
        self.in_develop = id;
    }
    /// Reads the photos opened in Develop again, and the one open there; whether
    /// there were any.
    pub(super) fn refresh_opened(&mut self, catalog: &Catalog) -> bool {
        let mut opened = std::mem::take(&mut self.opened);
        opened.extend(self.in_develop);
        self.refresh(catalog, opened.iter().copied());
        !opened.is_empty()
    }
    /// Reads `ids`' presets again, after their edits changed.
    pub(super) fn refresh(&mut self, catalog: &Catalog, ids: impl IntoIterator<Item = PhotoId>) {
        for id in ids {
            if self.one.as_ref().is_some_and(|(of, _)| *of == id) {
                self.one = None;
            }
            let Some(all) = &mut self.all else {
                continue;
            };
            match catalog
                .edit_record(id)
                .ok()
                .and_then(|r| r.applied_preset())
            {
                Some(p) => {
                    all.insert(id, crate::presets::display_name(&p.name));
                }
                None => {
                    all.remove(&id);
                }
            }
        }
    }
    /// Every photo's preset that names one. A catalog that can't be read
    /// names none, as for the sort order's keys.
    pub(super) fn all(&mut self, catalog: &Catalog) -> &HashMap<PhotoId, String> {
        self.all.get_or_insert_with(|| {
            catalog
                .applied_presets()
                .unwrap_or_default()
                .into_iter()
                .map(|(id, p)| (id, crate::presets::display_name(&p.name)))
                .collect()
        })
    }
    /// Photo `id`'s, with its Amount, read once until edits change.
    pub(super) fn of(&mut self, catalog: &Catalog, id: PhotoId) -> Option<AppliedPreset> {
        if self.one.as_ref().is_none_or(|(of, _)| *of != id) {
            let preset = catalog
                .edit_record(id)
                .ok()
                .and_then(|r| r.applied_preset());
            self.one = Some((id, preset));
        }
        self.one.as_ref().and_then(|(_, p)| p.clone())
    }
}

/// How the Library's info panel and Develop's Presets panel name an applied
/// preset: its name, then its Amount when it isn't 100%.
pub(in crate::app) fn shown(preset: &AppliedPreset) -> String {
    let name = crate::presets::display_name(&preset.name);
    match preset.amount {
        Some(amount) if (amount - 1.).abs() > 0.005 => {
            format!("{name} ({:.0}%)", amount * 100.)
        }
        _ => name,
    }
}
