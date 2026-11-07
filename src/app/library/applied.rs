//! Which preset each photo's edit was last made with ([`Recipe::applied_preset`]),
//! read from the catalog when the Preset filter or the info panel needs it: the
//! Library's copy, as the sort order's keys are. Reading every photo's means
//! reading every edit's text, so after that only the photos whose edits changed
//! are read again (Sync and batches name them; Develop's are the photos it
//! opened, and the one open there), and a catalog read again is read whole.
//!
//! A Lightroom edit that records no preset may still look made with one of the
//! user's presets ([`presets::inferred`](crate::presets::inferred)). Finding it
//! means reading and comparing every such edit, seconds for a large library, so
//! for the filter it is done on a thread of its own with its own catalog
//! connection, a batch at a time. Forgetting (edits changed, the catalog read
//! again, another preset library) drops the reader, which stops it, and its
//! results with it.
//!
//! [`Recipe::applied_preset`]: crate::model::recipe::Recipe::applied_preset
use crate::catalog::{Catalog, PhotoId};
use crate::model::recipe::AppliedPreset;
use crate::presets::inferred::Fingerprints;
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, TryRecvError, channel},
    },
};

/// Photos compared between two updates of the filter.
const BATCH: usize = 64;

/// A photo's preset as the Library shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Shown {
    /// The name shown ([`presets::display_name`](crate::presets::display_name)).
    pub name: String,
    /// Found by its settings, not recorded.
    pub inferred: bool,
}

#[derive(Default)]
pub(super) struct AppliedPresets {
    /// The user's presets as compared, and the library they were made from.
    fingerprints: Arc<Fingerprints>,
    of_library: Option<Arc<crate::presets::Library>>,
    /// Every photo's that names or looks like one; the inferred ones arrive
    /// while `reading`.
    all: Option<HashMap<PhotoId, Shown>>,
    reading: Option<Reading>,
    /// One photo's, as the info panel shows it.
    one: Option<(PhotoId, Option<AppliedPreset>)>,
    /// Photos opened in Develop since the Library last looked: their edits may
    /// have changed there.
    opened: HashSet<PhotoId>,
    /// The photo open in Develop, which going back to it doesn't open again.
    in_develop: Option<PhotoId>,
    /// Photos read again since the comparison started: what it read of them
    /// before is stale.
    read_again: HashSet<PhotoId>,
}

/// Comparing edits with the presets on a thread of its own; dropping it stops
/// the thread at its next batch.
struct Reading {
    rx: Receiver<Vec<(PhotoId, AppliedPreset)>>,
    cancel: Arc<AtomicBool>,
}
impl Drop for Reading {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl AppliedPresets {
    /// The catalog was read again, or the presets compared changed: everything is
    /// read again. Photos opened in Develop are still read again on the way
    /// back: one may be open there now.
    pub(super) fn forget(&mut self) {
        self.all = None;
        self.reading = None;
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
    /// Reads `ids`' presets again, after their edits changed: recorded or, for a
    /// Lightroom edit, inferred.
    pub(super) fn refresh(&mut self, catalog: &Catalog, ids: impl IntoIterator<Item = PhotoId>) {
        for id in ids {
            self.was_read_again(id);
            if self.one.as_ref().is_some_and(|(of, _)| *of == id) {
                self.one = None;
            }
            let Some(all) = &mut self.all else {
                continue;
            };
            let preset = catalog
                .edit_record(id)
                .ok()
                .and_then(|r| applied_or_inferred(&r, &self.fingerprints));
            match preset {
                Some(p) => {
                    all.insert(id, shown_as(&p));
                }
                None => {
                    all.remove(&id);
                }
            }
        }
    }
    /// Compares edits with `library`'s presets from now on; whether that
    /// changed what is known.
    pub(super) fn set_presets(&mut self, library: &Arc<crate::presets::Library>) -> bool {
        if self
            .of_library
            .as_ref()
            .is_some_and(|l| Arc::ptr_eq(l, library))
        {
            return false;
        }
        self.fingerprints = Arc::new(Fingerprints::of(&library.presets));
        self.of_library = Some(library.clone());
        self.forget();
        true
    }
    /// Photo `id` was read again: the running comparison's result for it, read
    /// before, is dropped.
    fn was_read_again(&mut self, id: PhotoId) {
        if self.reading.is_some() {
            self.read_again.insert(id);
        }
    }
    /// Every photo's preset that names or looks like one. The recorded ones are
    /// read at once; the inferred ones start being compared in the background
    /// (see [`poll`](Self::poll)). A catalog that can't be read names none, as
    /// for the sort order's keys.
    pub(super) fn all(
        &mut self,
        catalog: &Catalog,
        wake: impl Fn() + Send + 'static,
    ) -> &HashMap<PhotoId, Shown> {
        if self.all.is_none() {
            let recorded: HashMap<PhotoId, Shown> = catalog
                .applied_presets()
                .unwrap_or_default()
                .into_iter()
                .map(|(id, p)| (id, shown_as(&p)))
                .collect();
            if !self.fingerprints.is_empty() {
                let unrecorded: Vec<PhotoId> = catalog
                    .lightroom_xmp_edits()
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|id| !recorded.contains_key(id))
                    .collect();
                if !unrecorded.is_empty() {
                    self.read_again.clear();
                    self.reading = Some(Reading::start(
                        catalog.location().clone(),
                        unrecorded,
                        self.fingerprints.clone(),
                        wake,
                    ));
                }
            }
            self.all = Some(recorded);
        }
        self.all.get_or_insert_default()
    }
    /// Takes in what the background comparison found since the last call;
    /// whether anything was added.
    pub(super) fn poll(&mut self) -> bool {
        let Some(reading) = &self.reading else {
            return false;
        };
        let mut found = Vec::new();
        let done = loop {
            match reading.rx.try_recv() {
                Ok(batch) => found.extend(batch),
                Err(TryRecvError::Empty) => break false,
                Err(TryRecvError::Disconnected) => break true,
            }
        };
        if done {
            self.reading = None;
        }
        let Some(all) = &mut self.all else {
            return false;
        };
        let mut added = false;
        for (id, preset) in found {
            // A photo read again since knows better.
            if !self.read_again.contains(&id) && !all.contains_key(&id) {
                all.insert(id, shown_as(&preset));
                added = true;
            }
        }
        if done {
            self.read_again.clear();
        }
        added
    }
    /// Whether edits are still being compared with the presets.
    pub(super) fn reading(&self) -> bool {
        self.reading.is_some()
    }
    /// Photo `id`'s, with its Amount, read once until edits change: the one its
    /// edit records, else the one its Lightroom edit looks made with.
    pub(super) fn of(&mut self, catalog: &Catalog, id: PhotoId) -> Option<AppliedPreset> {
        if self.one.as_ref().is_none_or(|(of, _)| *of != id) {
            let preset = catalog
                .edit_record(id)
                .ok()
                .and_then(|r| applied_or_inferred(&r, &self.fingerprints));
            self.one = Some((id, preset));
        }
        self.one.as_ref().and_then(|(_, p)| p.clone())
    }
}

impl Reading {
    fn start(
        location: crate::catalog::CatalogLocation,
        photos: Vec<PhotoId>,
        fingerprints: Arc<Fingerprints>,
        wake: impl Fn() + Send + 'static,
    ) -> Self {
        let (tx, rx) = channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancelled = cancel.clone();
        crate::raw::spawn_background(move || {
            // Its own connection, as autosave has: the window's stays free.
            let Ok(catalog) = Catalog::open(&location) else {
                return;
            };
            for batch in photos.chunks(BATCH) {
                if cancelled.load(Ordering::Relaxed) {
                    return;
                }
                let found: Vec<_> = batch
                    .iter()
                    .filter_map(|&id| {
                        let record = catalog.edit_record(id).ok()?;
                        Some((id, applied_or_inferred(&record, &fingerprints)?))
                    })
                    .collect();
                if tx.send(found).is_err() {
                    return;
                }
                wake();
            }
        });
        Self { rx, cancel }
    }
}

/// The preset `record`'s edit records, else, for a Lightroom edit without a
/// RAWmakase one, the preset it looks made with.
fn applied_or_inferred(
    record: &crate::edits::EditRecord,
    fingerprints: &Fingerprints,
) -> Option<AppliedPreset> {
    record.applied_preset().or_else(|| {
        record
            .recipe
            .is_none()
            .then_some(record.lightroom.as_deref())
            .flatten()
            .and_then(|text| fingerprints.infer_text(text))
    })
}

fn shown_as(preset: &AppliedPreset) -> Shown {
    Shown {
        name: crate::presets::display_name(&preset.name),
        inferred: preset.inferred,
    }
}

/// How the Library's info panel and Develop's Presets panel name an applied
/// preset: its name, then its Amount when it isn't 100%; "Looks like" one found
/// by its settings.
pub(in crate::app) fn shown(preset: &AppliedPreset) -> String {
    let name = crate::presets::display_name(&preset.name);
    let name = match preset.amount {
        Some(amount) if (amount - 1.).abs() > 0.005 => {
            format!("{name} ({:.0}%)", amount * 100.)
        }
        _ => name,
    };
    if preset.inferred {
        format!("Looks like {name}")
    } else {
        name
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A batch read before a photo's edit changed doesn't put back what reading
    /// it again found: Sync removed its guess, and the batch would restore it.
    #[test]
    fn a_batch_read_before_an_edit_changed_does_not_restore_its_guess() {
        let (tx, rx) = channel();
        let mut applied = AppliedPresets {
            all: Some(HashMap::new()),
            reading: Some(Reading {
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
            }),
            ..Default::default()
        };
        let guess = |name: &str| AppliedPreset {
            name: name.into(),
            amount: None,
            uuid: String::new(),
            inferred: true,
        };
        applied.was_read_again(PhotoId(1));
        tx.send(vec![
            (PhotoId(1), guess("Old")),
            (PhotoId(2), guess("Warm")),
        ])
        .unwrap();
        assert!(applied.poll());
        let all = applied.all.as_ref().unwrap();
        assert!(!all.contains_key(&PhotoId(1)));
        assert_eq!(all.get(&PhotoId(2)).map(|s| s.name.as_str()), Some("Warm"));
    }
}
