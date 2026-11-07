//! Batch edits of the selected photos, each one change that one Undo reverses.
//!
//! Sync Settings and Match Total Exposures take one photo's settings onto the other
//! photos selected with it. In Develop the source is the open photo; in the Library it
//! is the active photo of the selection, as Lightroom's most-selected photo. Each
//! target's settings are worked out for its own camera (see `model::settings_groups`).
//!
//! Auto Settings and a Develop preset have no source: they apply to every selected
//! photo, measured on each as Develop measures the open photo.
//!
//! Every batch runs off the UI thread, reports its progress and can be cancelled
//! until it starts saving (see [`BatchProgress`]).
use crate::app::{
    Editor,
    develop::settings_transfer::Settings,
    history::{History, Step},
    worker::Event,
};
use crate::model::recipe::Recipe;
use crate::{
    catalog::{
        Catalog, CatalogLocation, EditChange, EditToSave, HistoryUpdate, PhotoId, SavedHistory,
    },
    edits::{self, Origin},
    export_settings::ExportOptions,
    model::settings_groups::{self, GroupSelection, Source, Target},
    raw_defaults::DevelopDefaults,
};
use anyhow::{Context, Result, ensure};
use rawmakase_cancel::Cancel;
use std::path::PathBuf;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
};

/// A photo settings are synchronized to.
#[derive(Clone, Debug)]
pub(in crate::app) struct SyncTarget {
    pub id: PhotoId,
    pub path: PathBuf,
    pub name: String,
}

/// Where a Sync's settings come from.
#[derive(Clone, Debug)]
pub(in crate::app) enum SyncSource {
    /// The open photo's settings, as Develop shows them.
    Open(Box<Settings>),
    /// A photo not open in Develop: its edit as Develop would open it with, worked
    /// out with the targets'.
    Photo(SyncTarget),
}

/// One photo's edit before and after a Sync.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Synced {
    pub id: PhotoId,
    pub path: PathBuf,
    /// The edit before, or none when the photo had no RAWmakase edit yet.
    pub before: EditBefore,
    pub after: Recipe,
    /// The History saved with `after`, which Redo writes back.
    pub history: SavedHistory,
    /// The History before the Sync, which Undo writes back (empty when it had none).
    pub history_before: SavedHistory,
    /// The file as the Sync read it: Redo refuses a file changed since.
    pub identity: crate::storage::Identity,
}

/// A photo's edit before a Sync.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EditBefore {
    Saved(Box<Recipe>),
    /// No RAWmakase edit: Develop started it from `starting`, its Lightroom edit or
    /// the camera defaults. Undo returns the photo to having none.
    None {
        starting: Box<Recipe>,
    },
}
impl EditBefore {
    fn recipe(&self) -> &Recipe {
        match self {
            EditBefore::Saved(r) | EditBefore::None { starting: r } => r,
        }
    }
}

/// A photo a Sync left as it was, and why.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyncFailure {
    pub name: String,
    pub reason: String,
}

/// A setting a Sync could not apply as asked on one photo (see `Transferred::notes`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyncNote {
    pub name: String,
    pub note: String,
}

/// What a Sync did, in which catalog.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SyncResult {
    pub change: BatchChange,
    pub catalog: CatalogLocation,
    pub synced: Vec<Synced>,
    pub failed: Vec<SyncFailure>,
    pub notes: Vec<SyncNote>,
    /// Cancelled before it saved anything: nothing changed.
    pub cancelled: bool,
}

/// A Sync for the shared undo log: every photo's edit before and after it.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct SyncCommand {
    pub sequence: u64,
    pub change: BatchChange,
    pub edits: Vec<Synced>,
}

/// What a batch change does to each photo it applies to.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) enum BatchChange {
    /// Sync Settings: the open photo's settings in these groups.
    Settings(GroupSelection),
    /// Lightroom's Match Total Exposures: each photo's Exposure set so that its
    /// aperture, shutter speed and ISO end up as bright as the open photo.
    #[default]
    MatchTotalExposures,
    /// Lightroom's Auto Settings, measured on each photo as Develop's Auto measures
    /// the open one.
    AutoSettings,
    /// A Develop preset at 100%, as a click in Develop's Presets panel applies it.
    Preset(Box<BatchPreset>),
}
/// A preset as a batch applies it: read once, when the batch starts.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct BatchPreset {
    pub preset: crate::xmp::Preset,
    /// As the Presets panel shows it.
    pub name: String,
}
impl BatchChange {
    /// The Undo name, and what the status line calls it.
    pub(crate) fn name(&self) -> String {
        match self {
            BatchChange::Settings(_) => "Synchronize Settings".into(),
            BatchChange::MatchTotalExposures => "Match Total Exposures".into(),
            BatchChange::AutoSettings => "Auto Settings".into(),
            BatchChange::Preset(p) => format!("Apply Preset: {}", p.name),
        }
    }
    /// Each photo's History step, named as Develop names the same change.
    fn step(&self) -> Step {
        match self {
            BatchChange::Preset(p) => Step::new("Preset", &p.name),
            _ => Step::new(self.name(), ""),
        }
    }
    /// Whether it takes settings from a source photo onto the others.
    pub(crate) fn has_source(&self) -> bool {
        matches!(
            self,
            BatchChange::Settings(_) | BatchChange::MatchTotalExposures
        )
    }
}

/// How a batch decodes a photo it measures: in full, with the demosaic Develop
/// uses, from the decode cache when it has the photo (which a batch never fills),
/// so each photo is measured on the image Develop measures.
#[derive(Clone, Default)]
pub(in crate::app) struct Decoding {
    pub demosaic: crate::camera_data::Demosaic,
    pub cache: Option<crate::decode_cache::DecodeCache>,
}

/// How far a batch has got, shared by its worker and the window that shows it, and
/// its Cancel. Cancel is taken until the batch starts saving: from then on the
/// outcome is whatever the save gives. Either way the batch ends only when its
/// worker reports back (`Event::Synced`).
#[derive(Debug)]
pub(crate) struct BatchProgress {
    name: String,
    total: usize,
    done: AtomicUsize,
    current: Mutex<String>,
    state: AtomicU8,
    /// Set with a cancel, for decodes and Auto to stop mid-photo.
    stop: AtomicBool,
    started: std::time::Instant,
    /// Cancels once this many photos are prepared, for tests to cancel at a known
    /// point.
    #[cfg(test)]
    cancel_after: Option<usize>,
}
const RUNNING: u8 = 0;
const CANCELLED: u8 = 1;
const SAVING: u8 = 2;
/// Where a batch is, as its window shows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BatchStage {
    Preparing,
    Cancelling,
    Saving,
}
impl BatchProgress {
    pub(crate) fn new(name: impl Into<String>, total: usize) -> Self {
        Self {
            name: name.into(),
            total,
            done: AtomicUsize::new(0),
            current: Mutex::new(String::new()),
            state: AtomicU8::new(RUNNING),
            stop: AtomicBool::new(false),
            started: std::time::Instant::now(),
            #[cfg(test)]
            cancel_after: None,
        }
    }
    #[cfg(test)]
    pub(crate) fn cancelling_after(mut self, prepared: usize) -> Self {
        self.cancel_after = Some(prepared);
        self
    }
    pub(crate) fn name(&self) -> &str {
        &self.name
    }
    /// Photos prepared, of how many, and the one being prepared.
    pub(crate) fn counts(&self) -> (usize, usize, String) {
        let current = self.current.lock().map(|c| c.clone()).unwrap_or_default();
        (self.done.load(Ordering::Relaxed), self.total, current)
    }
    pub(crate) fn stage(&self) -> BatchStage {
        match self.state.load(Ordering::Acquire) {
            CANCELLED => BatchStage::Cancelling,
            SAVING => BatchStage::Saving,
            _ => BatchStage::Preparing,
        }
    }
    pub(crate) fn elapsed(&self) -> std::time::Duration {
        self.started.elapsed()
    }
    /// Asks the batch to stop and discard what it prepared; false once it has
    /// started saving, when it can no longer be.
    pub(crate) fn cancel(&self) -> bool {
        let cancelled = self
            .state
            .compare_exchange(RUNNING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
            .is_ok();
        if cancelled {
            self.stop.store(true, Ordering::Relaxed);
        }
        cancelled || self.state.load(Ordering::Acquire) == CANCELLED
    }
    fn cancelled(&self) -> bool {
        self.state.load(Ordering::Acquire) == CANCELLED
    }
    /// From now on Cancel is refused; false if it came first.
    fn begin_saving(&self) -> bool {
        self.state
            .compare_exchange(RUNNING, SAVING, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    fn preparing(&self, name: &str) {
        if let Ok(mut current) = self.current.lock() {
            current.clear();
            current.push_str(name);
        }
    }
    fn prepared(&self) {
        let _done = self.done.fetch_add(1, Ordering::Relaxed) + 1;
        #[cfg(test)]
        if self.cancel_after == Some(_done) {
            self.cancel();
        }
    }
}

/// Which edits an undo or redo of a Sync writes back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum SyncSide {
    Before,
    After,
}

/// The new edits of `targets`, prepared photo by photo and saved in one transaction.
/// A photo that cannot be prepared (offline, changed since its edit was saved) is
/// reported and left out; if saving fails, nothing is saved. A source whose settings
/// can't be read changes nothing, nor does a batch cancelled before it saves.
/// `source` is the photo a change that has one ([`BatchChange::has_source`]) takes
/// its settings from.
pub(in crate::app) fn synchronize(
    catalog: &mut Catalog,
    source: Option<&SyncSource>,
    change: &BatchChange,
    targets: &[SyncTarget],
    defaults: &DevelopDefaults,
    decoding: &Decoding,
    progress: &BatchProgress,
) -> SyncResult {
    let mut result = SyncResult {
        change: change.clone(),
        catalog: catalog.location().clone(),
        synced: Vec::new(),
        failed: Vec::new(),
        notes: Vec::new(),
        cancelled: false,
    };
    let read;
    let source = match source {
        _ if !change.has_source() => None,
        None => {
            result.failed.push(SyncFailure {
                name: change.name(),
                reason: "there is no photo to take its settings from".into(),
            });
            return result;
        }
        Some(SyncSource::Open(settings)) => Some(settings.as_ref()),
        Some(SyncSource::Photo(photo)) => match photo_settings(catalog, photo, defaults) {
            Ok((settings, warnings)) => {
                result
                    .notes
                    .extend(warnings.into_iter().map(|note| SyncNote {
                        name: photo.name.clone(),
                        note,
                    }));
                read = settings;
                Some(&read)
            }
            Err(e) => {
                result.failed.push(SyncFailure {
                    name: photo.name.clone(),
                    reason: format!("its settings can't be read to sync from: {e:#}"),
                });
                return result;
            }
        },
    };
    // Matching to a photo without aperture, shutter speed and ISO means nothing.
    if *change == BatchChange::MatchTotalExposures
        && source.is_some_and(|source| capture_stops(&source.metadata).is_none())
    {
        result.failed.push(SyncFailure {
            name: "Match Total Exposures".into(),
            reason: "the source photo has no aperture, shutter speed or ISO".into(),
        });
        return result;
    }
    let cancelled = |mut result: SyncResult| {
        result.cancelled = true;
        result.failed.clear();
        result.notes.clear();
        result
    };
    let mut prepared = Vec::new();
    for target in targets {
        if progress.cancelled() {
            return cancelled(result);
        }
        progress.preparing(&target.name);
        match prepare(
            catalog,
            source,
            change,
            target,
            defaults,
            decoding,
            Cancel::new(&progress.stop),
        ) {
            Ok(p) => {
                result.notes.extend(p.notes.iter().map(|note| SyncNote {
                    name: target.name.clone(),
                    note: note.clone(),
                }));
                prepared.extend(p.edit);
            }
            Err(e) => result.failed.push(SyncFailure {
                name: target.name.clone(),
                reason: format!("{e:#}"),
            }),
        }
        progress.prepared();
    }
    // A file replaced since it was read gets nothing: its settings were worked out
    // for the file that was there.
    let (prepared, replaced): (Vec<_>, Vec<_>) = prepared.into_iter().partition(|p| {
        crate::storage::Identity::read(&p.synced.path)
            .is_ok_and(|now| now.same_photo(&p.synced.identity))
    });
    result.failed.extend(replaced.iter().map(|p| SyncFailure {
        name: p.name.clone(),
        reason: "the file changed during the Sync".into(),
    }));
    // Cancel is checked once more right before saving; once saving starts, it is
    // too late to cancel and the outcome is the save's.
    if !progress.begin_saving() {
        return cancelled(result);
    }
    let edits: Vec<EditToSave> = prepared
        .iter()
        .map(|p| EditToSave {
            id: p.synced.id,
            original: crate::edits::Original::file(&p.synced.path),
            source: &p.synced.identity,
            recipe: &p.synced.after,
            export: &p.export,
            history: HistoryUpdate::Replace(&p.synced.history),
        })
        .collect();
    match catalog.save_edits(&edits) {
        Ok(()) => result.synced = prepared.into_iter().map(|p| p.synced).collect(),
        Err(e) => result.failed.extend(prepared.iter().map(|p| SyncFailure {
            name: p.name.clone(),
            reason: format!("{e:#}"),
        })),
    }
    result
}

/// A target's new edit, ready to save.
struct PreparedEdit {
    synced: Synced,
    name: String,
    export: ExportOptions,
}

/// A target's new edit (`None` when the Sync changes nothing on it), and what could
/// not be applied as asked.
struct Prepared {
    edit: Option<PreparedEdit>,
    notes: Vec<String>,
}

/// A target as it was opened, decoded in full the first time a setting measures it.
struct Opened {
    path: PathBuf,
    opened: Option<crate::photo::Opened>,
    image: Option<crate::camera_data::CameraImage>,
}
impl Opened {
    fn image(
        &mut self,
        decoding: &Decoding,
        cancel: Cancel<'_>,
    ) -> Result<&crate::camera_data::CameraImage> {
        if self.image.is_none() {
            let image = match self.opened.take().context("the photo is not open")? {
                crate::photo::Opened::Raw(raw) => match &decoding.cache {
                    Some(cache) => {
                        let full = crate::decode::FullSize::with_cache(
                            &self.path,
                            decoding.demosaic,
                            cache.clone(),
                        );
                        match full.cached(&raw.metadata) {
                            Some(image) => image,
                            None => {
                                full.decode(raw, crate::decode::DecodePolicy::Export, cancel)?
                            }
                        }
                    }
                    None => {
                        raw.develop(crate::camera_data::Decode::full(decoding.demosaic), cancel)?
                    }
                },
                crate::photo::Opened::Rendered(image) => image,
            };
            self.image = Some(image);
        }
        self.image.as_ref().context("the photo is not decoded")
    }
}

fn prepare(
    catalog: &Catalog,
    source: Option<&Settings>,
    change: &BatchChange,
    target: &SyncTarget,
    defaults: &DevelopDefaults,
    decoding: &Decoding,
    cancel: Cancel<'_>,
) -> Result<Prepared> {
    // Read first, so a file replaced while its settings are worked out is noticed.
    let identity = crate::storage::Identity::read(&target.path)?;
    let opened = crate::photo::open_source(&target.path, Cancel::NEVER)
        .with_context(|| format!("{} can't be read", target.name))?;
    let metadata = opened.metadata().clone();
    let mut photo = Opened {
        path: target.path.clone(),
        opened: Some(opened),
        image: None,
    };
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let profiles = crate::camera_look::with_default(&target.path, &metadata, profiles, cancel);
    let record = catalog.edit_record(target.id)?;
    let resolved = edits::resolve(&record, &target.path, &metadata, &profiles, defaults)?;
    let starting_warnings = starting_warnings(resolved.origin, resolved.warnings);
    let export = resolved.export;
    let before = match resolved.origin {
        Origin::Saved => EditBefore::Saved(Box::new(resolved.recipe)),
        _ => EditBefore::None {
            starting: Box::new(resolved.recipe),
        },
    };
    let source = || source.context("there is no photo to take settings from");
    // A change that measures the photo measures it as Develop would: an Upright or
    // Straighten Auto the saved edit still had to analyse is analysed first, as
    // opening the photo does before its Auto can run. Undo still returns `before`.
    let measures = match change {
        BatchChange::AutoSettings => true,
        BatchChange::Preset(p) => p.preset.measures_photo(),
        _ => false,
    };
    let mut start = before.recipe().clone();
    if measures && (start.upright.needs_analysis() || start.straighten_pending()) {
        crate::develop::upright::complete(&mut start, photo.image(decoding, cancel)?);
    }
    let transferred = match change {
        BatchChange::Settings(groups) => {
            let source = source()?;
            settings_groups::transfer(
                Source {
                    recipe: &source.recipe,
                    metadata: &source.metadata,
                },
                before.recipe(),
                groups,
                Target {
                    metadata: &metadata,
                    profiles: &profiles,
                },
            )
        }
        BatchChange::MatchTotalExposures => settings_groups::Transferred {
            recipe: crate::model::recipe::Recipe {
                exposure: matched_exposure(source()?, &metadata).with_context(|| {
                    format!("{} has no aperture, shutter speed or ISO", target.name)
                })?,
                ..before.recipe().clone()
            },
            notes: Vec::new(),
        },
        // Develop's Auto: the same estimate on the same image.
        BatchChange::AutoSettings => settings_groups::Transferred {
            recipe: crate::develop::auto_tone(photo.image(decoding, cancel)?, &start, cancel)?,
            notes: Vec::new(),
        },
        // As a click in Develop applies it; decoded only to measure an Auto in it.
        BatchChange::Preset(p) => {
            let image = if p.preset.measures_photo() {
                Some(photo.image(decoding, cancel)?)
            } else {
                None
            };
            let measures = image.map(crate::develop::Measures);
            let applied = crate::presets::apply_to_photo(
                &p.preset,
                &start,
                &metadata,
                &profiles,
                measures
                    .as_ref()
                    .map(|m| m as &dyn crate::xmp::PhotoMeasures),
            )?;
            let mut notes: Vec<String> = applied
                .substitute
                .map(|used| format!("{} renders with {used}", p.name))
                .into_iter()
                .collect();
            notes.extend(applied.skipped.into_iter().map(|s| format!("skipped: {s}")));
            settings_groups::Transferred {
                recipe: applied.recipe,
                notes,
            }
        }
    };
    // What its starting edit could not bring along is said, as when it is opened.
    let mut notes = starting_warnings;
    notes.extend(transferred.notes);
    let mut after = transferred.recipe;
    // As the same change made in Develop: one that only edits a switched-off panel
    // turns it on. The change is from the photo as analysed: an analysis landing is
    // not an edit.
    crate::model::edit::turn_on_edited_panel(&start, &mut after);
    // Upright's corrections and a Straighten Auto are measured on each photo; the
    // open photo's editor does it on Paste, and here the photo is developed for it.
    // Guided solves this photo's own guides beside that analysis.
    if after.upright.needs_analysis() || after.straighten_pending() {
        let image = photo.image(decoding, cancel)?;
        if let Some(issue) = crate::develop::upright::complete(&mut after, image)
            && after.upright.mode == crate::model::transform::UprightMode::Guided
        {
            notes.push(issue.message().into());
        }
    }
    let before_recipe = before.recipe().clone();
    if after == before_recipe {
        return Ok(Prepared { edit: None, notes });
    }
    // A History this release can't read (a newer one's) is not replaced.
    ensure!(
        catalog.load_history(target.id)?.is_some() || !catalog.has_history(target.id)?,
        "Its History is from a newer RAWmakase"
    );
    let saved = catalog
        .load_history(target.id)?
        .unwrap_or_else(|| SavedHistory {
            origin: before_recipe.clone(),
            steps: Vec::new(),
            applied: 0,
        });
    let history_before = saved.clone();
    let mut history = History::restored(saved, &before_recipe);
    let mut current = before_recipe;
    history.set(&after, &mut current, change.step());
    Ok(Prepared {
        edit: Some(PreparedEdit {
            synced: Synced {
                id: target.id,
                path: target.path.clone(),
                before,
                after,
                history: history.saved(&current),
                history_before,
                identity,
            },
            name: target.name.clone(),
            export,
        }),
        notes,
    })
}

/// The settings of `photo`, a photo not open, as Develop would open it with (see
/// `edits::resolve`), and what its starting edit could not bring along. An edit that
/// is protected (its file changed) or can't be read is an error, never the defaults.
fn photo_settings(
    catalog: &Catalog,
    photo: &SyncTarget,
    defaults: &DevelopDefaults,
) -> Result<(Settings, Vec<String>)> {
    let opened = crate::photo::open_source(&photo.path, Cancel::NEVER)
        .with_context(|| format!("{} can't be read", photo.name))?;
    let metadata = opened.metadata().clone();
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let profiles =
        crate::camera_look::with_default(&photo.path, &metadata, profiles, Cancel::NEVER);
    let record = catalog.edit_record(photo.id)?;
    let resolved = edits::resolve(&record, &photo.path, &metadata, &profiles, defaults)?;
    let warnings = starting_warnings(resolved.origin, resolved.warnings);
    Ok((
        Settings {
            recipe: resolved.recipe,
            metadata,
        },
        warnings,
    ))
}

/// What a photo's starting edit could not bring along, said as when it is opened; a
/// saved edit has none.
fn starting_warnings(origin: Origin, warnings: Vec<String>) -> Vec<String> {
    match origin {
        Origin::Saved => Vec::new(),
        Origin::Lightroom => warnings
            .into_iter()
            .map(|w| format!("Lightroom edit not fully rendered: {w}"))
            .collect(),
        Origin::Defaults => warnings,
    }
}

/// How much light a photo's camera settings let in, in stops from f/1, 1 s, ISO 100:
/// what Match Total Exposures evens out. `None` without all three.
pub(in crate::app) fn capture_stops(m: &crate::camera_data::Metadata) -> Option<f32> {
    (m.aperture > 0. && m.shutter > 0. && m.iso > 0.)
        .then(|| m.shutter.log2() - 2. * m.aperture.log2() + (m.iso / 100.).log2())
}

/// The Exposure that makes a photo shot with `target`'s settings as bright as the
/// source: a photo that let in a stop more light gets a stop less Exposure.
fn matched_exposure(source: &Settings, target: &crate::camera_data::Metadata) -> Option<f32> {
    let difference = capture_stops(&source.metadata)? - capture_stops(target)?;
    let valid = &crate::model::params::ParameterId::Exposure
        .descriptor()
        .valid;
    Some((source.recipe.exposure + difference).clamp(*valid.start(), *valid.end()))
}

/// Writes one side of a Sync back, in one transaction, keeping each photo's History,
/// which notices the change when the photo opens. A photo that had no edit before
/// goes back to having none. `path` gives each photo's current location, which a
/// relink may have changed since.
pub(in crate::app) fn restore(
    session: &mut crate::catalog_session::CatalogSession,
    edits: &[Synced],
    side: SyncSide,
    path: impl Fn(PhotoId) -> Option<PathBuf>,
) -> std::result::Result<(), SyncRestoreError> {
    // A photo removed since can't be restored; the caller drops the command.
    let paths = edits
        .iter()
        .map(|e| path(e.id))
        .collect::<Option<Vec<_>>>()
        .ok_or(SyncRestoreError::PhotoRemoved)?;
    let mut saves = Vec::with_capacity(edits.len());
    for (e, path) in edits.iter().zip(paths) {
        let (recipe, history) = match (side, &e.before) {
            (SyncSide::Before, EditBefore::None { .. }) => continue,
            // Undo brings back the History it had, redo branch included.
            (SyncSide::Before, EditBefore::Saved(r)) => {
                (r.as_ref(), HistoryUpdate::Replace(&e.history_before))
            }
            // Redo brings back the History the Sync saved, which Undo may have cleared.
            (SyncSide::After, _) => (&e.after, HistoryUpdate::Replace(&e.history)),
        };
        let saved = session
            .catalog()
            .load_edit(e.id, &path)
            .map_err(SyncRestoreError::Write)?;
        // A saved edit's identity, which a verified move of its file refreshes,
        // protects the file. Without an edit there is none: compare with the one
        // the Sync read before writing its settings again.
        let now = crate::storage::Identity::read(&path).map_err(SyncRestoreError::Write)?;
        if saved.is_none() && !now.same_photo(&e.identity) {
            return Err(SyncRestoreError::Write(anyhow::anyhow!(
                "{} changed since the Sync",
                path.display()
            )));
        }
        let export = saved.map(|saved| saved.export).unwrap_or_default();
        saves.push((e.id, path, now, recipe, export, history));
    }
    let saves: Vec<EditToSave> = saves
        .iter()
        .map(|(id, path, source, recipe, export, history)| EditToSave {
            id: *id,
            original: crate::edits::Original::file(path),
            source,
            recipe,
            export,
            history: *history,
        })
        .collect();
    let mut changes: Vec<EditChange> = saves.iter().map(EditChange::Save).collect();
    if side == SyncSide::Before {
        changes.extend(
            edits
                .iter()
                .filter(|e| matches!(e.before, EditBefore::None { .. }))
                .map(|e| EditChange::Clear { id: e.id }),
        );
    }
    session
        .change_edits(&changes)
        .map_err(SyncRestoreError::Write)
}

/// Why a Sync could not be undone or redone.
#[derive(Debug)]
pub(in crate::app) enum SyncRestoreError {
    /// One of its photos was removed from the catalog since.
    PhotoRemoved,
    Write(anyhow::Error),
}
impl std::error::Error for SyncRestoreError {}
impl std::fmt::Display for SyncRestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SyncRestoreError::PhotoRemoved => {
                f.write_str("a synchronized photo is no longer in the catalog")
            }
            SyncRestoreError::Write(e) => write!(f, "{e:#}"),
        }
    }
}

impl Editor {
    /// The photo Sync takes its settings from: in Develop the open photo, in the
    /// Library the active one of the selection.
    fn sync_source_id(&self) -> Option<PhotoId> {
        match self.module {
            crate::app::Module::Develop => self.document.catalog_photo,
            crate::app::Module::Library => self.library.as_ref()?.selected(),
        }
    }
    /// The source photo, when its settings can be synchronized now.
    fn sync_ready_source(&self) -> Option<PhotoId> {
        let (library, source) = (self.library.as_ref()?, self.sync_source_id()?);
        // The open photo may be hidden by the filters and still selected.
        if !self.batch_ready() || !library.is_selected(source) {
            return None;
        }
        // A protected edit (its file changed) shows camera defaults, not its settings.
        // Another photo's edit is read with the targets', and refused there.
        if self.document.catalog_photo == Some(source)
            && (self.document.metadata.is_none() || self.document.edit.save_state().is_protected())
        {
            return None;
        }
        Some(source)
    }
    /// The other photos selected with the source photo, which Sync applies to.
    pub(in crate::app) fn sync_targets(&self) -> Vec<SyncTarget> {
        let (Some(library), Some(source)) = (&self.library, self.sync_ready_source()) else {
            return Vec::new();
        };
        let selected = library.selected_photos();
        // Looked up once: a selection can hold the whole catalog.
        let wanted: std::collections::HashSet<PhotoId> = selected.iter().copied().collect();
        let photos: std::collections::HashMap<PhotoId, &crate::catalog::Photo> = library
            .session
            .photos()
            .iter()
            .filter(|p| wanted.contains(&p.id))
            .map(|p| (p.id, p))
            .collect();
        selected
            .into_iter()
            .filter(|id| *id != source)
            .filter_map(|id| photos.get(&id).copied())
            .map(sync_target)
            .collect()
    }
    /// How many photos Sync would apply to, for a control drawn every frame.
    pub(in crate::app) fn sync_target_count(&self) -> usize {
        let (Some(library), Some(source)) = (&self.library, self.sync_ready_source()) else {
            return 0;
        };
        let selected = library.selected_photos();
        selected.len() - usize::from(selected.contains(&source))
    }
    /// The source's settings: the open photo's as Develop has them, or another photo
    /// to read with the targets.
    fn sync_source(&self) -> Option<SyncSource> {
        let id = self.sync_source_id()?;
        if self.document.catalog_photo == Some(id) {
            return self
                .current_settings()
                .map(|s| SyncSource::Open(Box::new(s)));
        }
        let photo = self.library.as_ref()?.photo(id)?;
        Some(SyncSource::Photo(sync_target(photo)))
    }
    /// Synchronizes the source photo's `groups` to the other selected photos, in the
    /// background; the result arrives as [`Event::Synced`].
    pub(in crate::app) fn start_sync(&mut self, change: BatchChange) {
        debug_assert!(change.has_source());
        // Matching to a photo without aperture, shutter speed and ISO means nothing.
        if change == BatchChange::MatchTotalExposures
            && self.sync_source_id() == self.document.catalog_photo
            && self
                .document
                .metadata
                .as_ref()
                .and_then(capture_stops)
                .is_none()
        {
            self.messages
                .warn("Match Total Exposures needs this photo's aperture, shutter speed and ISO");
            return;
        }
        let targets = self.sync_targets();
        if targets.is_empty() || self.activity.is_busy() || !self.flush() {
            return;
        }
        let Some(source) = self.sync_source() else {
            return;
        };
        self.run_batch(change, Some(source), targets);
    }
    /// Whether a batch can change the selected photos now. The open photo's settings
    /// are not final until its Lightroom edit is in, or while an Auto estimate or an
    /// Auto Straighten is still to land on them: the photo opens again after the
    /// batch, so what landed meanwhile would be lost, or saved over what it wrote. Profiles still being imported would resolve
    /// differently photo to photo, and while Read Metadata from Files runs, which
    /// photos have a Lightroom edit can still change.
    fn batch_ready(&self) -> bool {
        let Some(library) = &self.library else {
            return false;
        };
        let open_selected = self
            .document
            .catalog_photo
            .is_some_and(|id| library.is_selected(id));
        !(open_selected
            && (self.document.pending_lightroom.is_some()
                || self.document.auto.is_running()
                || self.document.straighten.is_running()))
            && self.importing.is_none()
            && !library.rereading()
    }
    /// Every selected photo, the active and the open one included, which Auto
    /// Settings and a preset from the Library apply to.
    pub(in crate::app) fn batch_targets(&self) -> Vec<SyncTarget> {
        let Some(library) = self.library.as_ref().filter(|_| self.batch_ready()) else {
            return Vec::new();
        };
        let selected = library.selected_photos();
        // Looked up once: a selection can hold the whole catalog.
        let wanted: std::collections::HashSet<PhotoId> = selected.iter().copied().collect();
        let photos: std::collections::HashMap<PhotoId, &crate::catalog::Photo> = library
            .session
            .photos()
            .iter()
            .filter(|p| wanted.contains(&p.id))
            .map(|p| (p.id, p))
            .collect();
        selected
            .into_iter()
            .filter_map(|id| photos.get(&id).copied())
            .map(sync_target)
            .collect()
    }
    /// How many photos a batch would apply to, for a control drawn every frame.
    pub(in crate::app) fn batch_target_count(&self) -> usize {
        match &self.library {
            Some(library) if self.batch_ready() => library.selected_photos().len(),
            _ => 0,
        }
    }
    /// Applies `change`, which has no source photo (Auto Settings, a preset), to
    /// every selected photo, in the background; the result arrives as
    /// [`Event::Synced`]. The open photo, when selected, is saved first.
    pub(in crate::app) fn start_batch(&mut self, change: BatchChange) {
        debug_assert!(!change.has_source());
        let targets = self.batch_targets();
        if targets.is_empty() || self.activity.is_busy() || !self.flush() {
            return;
        }
        self.run_batch(change, None, targets);
    }
    /// Starts `change` on `targets` on a worker, with everything it reads captured
    /// now: a later change to the raw defaults, demosaic or presets does not reach it.
    fn run_batch(
        &mut self,
        change: BatchChange,
        source: Option<SyncSource>,
        targets: Vec<SyncTarget>,
    ) {
        let Some(library) = &self.library else {
            return;
        };
        let catalog = library.session.catalog().location().clone();
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let defaults = self.raw_defaults.clone();
        let decoding = Decoding {
            demosaic: self.session.demosaic,
            cache: Some(self.decode_cache.clone()),
        };
        let progress = Arc::new(BatchProgress::new(change.name(), targets.len()));
        // Moving to another photo or catalog waits, so neither can see an edit change
        // underneath it.
        if !self.activity.begin_sync(progress.clone()) {
            return;
        }
        self.messages.note(format!(
            "{} on {}…",
            change.name(),
            crate::app::ui::widgets::plural(targets.len(), "photo", "photos")
        ));
        std::thread::spawn(move || {
            // A panic (in Upright's analysis, say) still finishes the batch.
            let result = finished(
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    Catalog::open(&catalog).map(|mut c| {
                        synchronize(
                            &mut c,
                            source.as_ref(),
                            &change,
                            &targets,
                            &defaults,
                            &decoding,
                            &progress,
                        )
                    })
                })),
                &change,
                &catalog,
            );
            let _ = tx.send(Event::Synced(Box::new(result)));
            ctx.request_repaint();
        });
    }
    /// The open photo, when a Sync or its Undo wrote `ids`' edits, opens again with
    /// what was written, so nothing stale is saved over it.
    pub(in crate::app) fn reopen_synced(&mut self, ids: &[PhotoId]) {
        if let Some(open) = self.document.catalog_photo
            && ids.contains(&open)
            && let Some(path) = self.document.path.clone()
        {
            self.document.edit.save_state_mut().saved();
            // Not opened again (its editable preview can't be used now, or what
            // must be saved first could not be): the recipe on screen is the one
            // from before, marked saved, so it is closed rather than left to be
            // saved over what was written. The catalog has the photo's edit.
            if !self.load_raw(path, Some(open)) {
                self.close_removed_document();
            }
        }
    }
    /// A finished Sync: one command for Undo, and the top bar naming what failed.
    pub(in crate::app) fn synced(&mut self, result: SyncResult) {
        self.activity.finish_sync();
        // A result for a catalog no longer open must not reach this one's undo log.
        if self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location())
            != Some(&result.catalog)
        {
            return;
        }
        if result.cancelled {
            self.messages.note(format!(
                "{} cancelled: no photo changed",
                result.change.name()
            ));
            return;
        }
        let done = result.synced.len();
        if let Some(library) = &mut self.library {
            library.edits_changed(result.synced.iter().map(|e| e.id));
        }
        let synced: Vec<_> = result.synced.iter().map(|e| e.id).collect();
        self.refresh_previews(&synced);
        // From the Library, the photo open in Develop can be among them.
        self.reopen_synced(&synced);
        // The reference photo may be among them.
        self.load_reference();
        if done > 0 {
            self.undo_log
                .push(crate::app::shell::undo::Command::Sync(Box::new(
                    SyncCommand {
                        sequence: crate::edit_session::sequence(),
                        change: result.change.clone(),
                        edits: result.synced,
                    },
                )));
        }
        let mut status = format!(
            "{}: {} changed",
            result.change.name(),
            crate::app::ui::widgets::plural(done, "photo", "photos")
        );
        for failure in &result.failed {
            status.push_str(&format!(
                " · {} not changed: {}",
                failure.name, failure.reason
            ));
        }
        for note in &result.notes {
            status.push_str(&format!(" · {}: {}", note.name, note.note));
        }
        if !result.failed.is_empty() && done == 0 {
            self.messages.fail(status);
        } else if !result.failed.is_empty() || !result.notes.is_empty() {
            self.messages.warn(status);
        } else {
            self.messages.done(status);
        }
    }
}

/// What a batch's worker reports: its result, or why it had none (the catalog did
/// not open, or it panicked), which changed nothing.
fn finished(
    outcome: std::thread::Result<Result<SyncResult>>,
    change: &BatchChange,
    catalog: &CatalogLocation,
) -> SyncResult {
    outcome
        .unwrap_or_else(|_| Err(anyhow::anyhow!("the Sync failed unexpectedly")))
        .unwrap_or_else(|e| SyncResult {
            change: change.clone(),
            catalog: catalog.clone(),
            notes: Vec::new(),
            synced: Vec::new(),
            failed: vec![SyncFailure {
                name: "Catalog".into(),
                reason: format!("{e:#}"),
            }],
            cancelled: false,
        })
}

/// A catalog photo as a Sync names and reads it.
fn sync_target(p: &crate::catalog::Photo) -> SyncTarget {
    SyncTarget {
        id: p.id,
        path: p.path.clone(),
        name: format!("{}{}", p.filename, crate::app::library::copy_suffix(p)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rawmakase_cancel::Cancel;
    use std::path::Path;

    /// A catalog of copies of the synthetic chart DNG, and one file that isn't a photo.
    struct Fixture {
        _dir: tempfile::TempDir,
        catalog: Catalog,
        photos: Vec<(PhotoId, PathBuf)>,
    }
    fn catalog() -> Result<Fixture> {
        let d = tempfile::tempdir()?;
        let photos = d.path().join("photos");
        std::fs::create_dir(&photos)?;
        let chart =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng");
        for name in ["a.dng", "b.dng", "c.dng"] {
            std::fs::copy(&chart, photos.join(name))?;
        }
        std::fs::write(photos.join("d.dng"), b"not a photo")?;
        let mut c = Catalog::create(&d.path().join("sync.rawmakase"))?;
        c.add_folder(&photos)?;
        let photos = c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
        Ok(Fixture {
            _dir: d,
            catalog: c,
            photos,
        })
    }
    /// A session on the fixture's catalog, which Undo and Redo restore a Sync
    /// through; the fixture's own connection still sees what it writes.
    fn open_session(dir: &tempfile::TempDir) -> Result<crate::catalog_session::CatalogSession> {
        let location = CatalogLocation::from(&dir.path().join("sync.rawmakase"));
        Ok(crate::catalog_session::CatalogSession::open(&location)?.session)
    }
    fn target(id: PhotoId, path: &Path) -> SyncTarget {
        SyncTarget {
            id,
            path: path.to_path_buf(),
            name: path.file_name().unwrap().to_string_lossy().into(),
        }
    }

    /// A JPEG takes the settings it can from a RAW and keeps its own profile.
    #[test]
    fn sync_saves_a_jpegs_edit_from_a_raws_settings() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let folder = photos[0].1.parent().unwrap().to_path_buf();
        let jpeg = folder.join("e.jpg");
        image::RgbImage::from_pixel(24, 16, image::Rgb([90, 120, 150])).save(&jpeg)?;
        c.add_folder(&folder)?;
        let id = c.photos()?.into_iter().find(|p| p.path == jpeg).unwrap().id;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.exposure = 0.5;
        let source = Settings {
            recipe: source,
            metadata,
        };
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source))),
            &BatchChange::Settings(GroupSelection::default()),
            &[target(id, &jpeg)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        assert_eq!(result.synced.len(), 1);
        let saved = c.load_edit(id, &jpeg)?.unwrap().recipe;
        assert_eq!(saved.exposure, 0.5);
        assert!(saved.profile.is_some_and(|p| p.is_embedded()));
        Ok(())
    }

    /// Issue 594: an angle the Crop panel's Auto measured is measured on each photo
    /// synchronized, not copied as Lightroom copies it.
    #[test]
    fn sync_measures_an_auto_straighten_on_each_photo() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        // Stripes falling 3° to the right, which Auto levels at about −3°.
        let folder = photos[0].1.parent().unwrap().to_path_buf();
        let png = folder.join("e.png");
        let (s, cos) = 3f32.to_radians().sin_cos();
        image::RgbImage::from_fn(600, 400, |x, y| {
            // Supersampled, so edges are smooth rather than level pixel steps.
            let mut sum = 0.;
            for j in 0..4 {
                let x = x as f32 + (j % 2) as f32 * 0.5 - 299.75;
                let y = y as f32 + (j / 2) as f32 * 0.5 - 199.75;
                let across = -s * x + cos * y;
                sum += if (across / 70.).rem_euclid(2.) < 1. {
                    200.
                } else {
                    25.
                };
            }
            image::Rgb([(sum / 4.) as u8; 3])
        })
        .save(&png)?;
        c.add_folder(&folder)?;
        let id = c.photos()?.into_iter().find(|p| p.path == png).unwrap().id;
        let metadata = crate::photo::open_source(&png, Cancel::NEVER)?
            .metadata()
            .clone();
        let mut source = Recipe::with_profiles(&metadata, &[]);
        source.set_auto_straighten(Some(5.));
        let mut crop = GroupSelection::none();
        crop.set(
            settings_groups::SettingGroup::Crop,
            settings_groups::GroupInclusion::Included,
        );
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(Settings {
                recipe: source,
                metadata,
            }))),
            &BatchChange::Settings(crop),
            &[target(id, &png)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let saved = c.load_edit(id, &png)?.unwrap().recipe;
        assert!((saved.straighten + 3.).abs() < 0.3, "{}", saved.straighten);
        assert_eq!(
            saved.auto_straighten(),
            Some(crate::model::transform::AutoStraighten::Measured(
                saved.straighten
            ))
        );
        Ok(())
    }

    #[test]
    fn sync_saves_every_photo_it_can_with_a_history_step_and_undoes_together() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.exposure = 0.5;
        source.effects.clarity = 0.2;
        let source = Settings {
            recipe: source,
            metadata,
        };
        let targets: Vec<_> = photos[1..].iter().map(|(id, p)| target(*id, p)).collect();
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source.clone()))),
            &BatchChange::Settings(GroupSelection::default()),
            &targets,
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        // The file that isn't a photo is reported; the two charts are saved.
        assert_eq!(result.synced.len(), 2);
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert_eq!(result.failed[0].name, "d.dng");
        for (id, path) in &photos[1..3] {
            let saved = c.load_edit(*id, path)?.unwrap().recipe;
            assert_eq!((saved.exposure, saved.effects.clarity), (0.5, 0.2));
            let history = c.load_history(*id)?.unwrap();
            assert_eq!(history.steps.last().unwrap().name, "Synchronize Settings");
        }
        let path = |id: PhotoId| {
            photos
                .iter()
                .find(|(p, _)| *p == id)
                .map(|(_, path)| path.clone())
        };
        // One Undo restores every photo; these had no edit, and have none again.
        let mut session = open_session(&_dir)?;
        restore(&mut session, &result.synced, SyncSide::Before, path)?;
        for (id, path) in &photos[1..3] {
            assert!(c.load_edit(*id, path)?.is_none());
            assert!(c.load_history(*id)?.is_none());
        }
        // Redo brings the edit back with its History step.
        restore(&mut session, &result.synced, SyncSide::After, path)?;
        let history = c.load_history(photos[1].0)?.unwrap();
        assert_eq!(history.steps.last().unwrap().name, "Synchronize Settings");
        // A photo removed since makes the command unusable rather than wrong.
        assert!(matches!(
            restore(&mut session, &result.synced, SyncSide::Before, |_| None),
            Err(SyncRestoreError::PhotoRemoved)
        ));
        assert_eq!(
            c.load_edit(photos[1].0, &photos[1].1)?
                .unwrap()
                .recipe
                .exposure,
            0.5
        );
        // Undo on a photo that had an edit and History brings both back.
        let (id, path0) = (photos[1].0, photos[1].1.clone());
        let mut edited = c.load_edit(id, &path0)?.unwrap().recipe;
        edited.exposure = -0.3;
        let earlier = SavedHistory {
            origin: Recipe::default(),
            steps: vec![crate::catalog::SavedStep {
                name: "Exposure".into(),
                value: "-0.30".into(),
                recipe: edited.clone(),
            }],
            applied: 1,
        };
        c.save_edit(
            id,
            &path0,
            &crate::storage::Identity::read(&path0)?,
            &edited,
            &ExportOptions::default(),
            HistoryUpdate::Replace(&earlier),
        )?;
        let again = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source.clone()))),
            &BatchChange::Settings(GroupSelection::default()),
            &targets[..1],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert_eq!(again.synced.len(), 1);
        restore(&mut session, &again.synced, SyncSide::Before, path)?;
        assert_eq!(c.load_history(id)?, Some(earlier));
        assert_eq!(c.load_edit(id, &path0)?.unwrap().recipe.exposure, -0.3);
        restore(&mut session, &again.synced, SyncSide::After, path)?;
        // Settings the photos already have change nothing.
        let again = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source))),
            &BatchChange::Settings(GroupSelection::default()),
            &targets[..2],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(again.synced.is_empty() && again.failed.is_empty());
        // A file changed while it had no edit is not given the old settings again.
        restore(&mut session, &result.synced, SyncSide::Before, path)?;
        let changed = &photos[1].1;
        let mut bytes = std::fs::read(changed)?;
        bytes.extend_from_slice(b"changed");
        std::fs::write(changed, bytes)?;
        assert!(restore(&mut session, &result.synced, SyncSide::After, path).is_err());
        assert!(c.load_edit(photos[2].0, &photos[2].1)?.is_none());
        Ok(())
    }

    /// A gradient scene, brighter with `level` and tinted with `tint`, as a PNG.
    fn scene(path: &Path, level: f32, tint: [f32; 3]) -> Result<()> {
        image::RgbImage::from_fn(96, 64, |x, y| {
            let v = level * (0.05 + 0.95 * (x as f32 / 95.).powi(2)) * (1. - y as f32 / 200.);
            image::Rgb(std::array::from_fn(|c| {
                (255. * (v * tint[c]).clamp(0., 1.)) as u8
            }))
        })
        .save(path)?;
        Ok(())
    }
    /// Photos `names` as varied scenes, added to the fixture's catalog.
    fn scenes(f: &mut Fixture, names: &[&str]) -> Result<Vec<(PhotoId, PathBuf)>> {
        let folder = f.photos[0].1.parent().unwrap().to_path_buf();
        for (i, name) in names.iter().enumerate() {
            let i = i as f32;
            scene(
                &folder.join(name),
                0.25 + 0.3 * i,
                [1., 0.9 - 0.1 * i, 0.8 + 0.1 * i],
            )?;
        }
        f.catalog.add_folder(&folder)?;
        let photos = f.catalog.photos()?;
        Ok(names
            .iter()
            .map(|name| {
                let p = photos.iter().find(|p| p.filename == *name).unwrap();
                (p.id, p.path.clone())
            })
            .collect())
    }
    fn run(
        c: &mut Catalog,
        change: &BatchChange,
        targets: &[SyncTarget],
        progress: &BatchProgress,
    ) -> SyncResult {
        synchronize(
            c,
            None,
            change,
            targets,
            &DevelopDefaults::default(),
            &Decoding::default(),
            progress,
        )
    }

    /// Auto Settings on a selection measures each photo with Develop's Auto, on the
    /// full image Develop measures, so each gets its own values: one History step
    /// each, one Undo for all, and Redo writes back what was measured.
    #[test]
    fn batch_auto_settings_measures_each_photo() -> Result<()> {
        let mut f = catalog()?;
        let png = scenes(&mut f, &["dark.png", "mid.png", "bright.png"])?;
        // The synthetic DNG too, decoded in full as Develop decodes it.
        let all: Vec<_> = png.iter().chain(&f.photos[..1]).cloned().collect();
        let targets: Vec<_> = all.iter().map(|(id, p)| target(*id, p)).collect();
        let result = run(
            &mut f.catalog,
            &BatchChange::AutoSettings,
            &targets,
            &BatchProgress::new("Auto Settings", targets.len()),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        assert_eq!(result.synced.len(), all.len());
        let defaults = DevelopDefaults::default();
        let mut exposures = Vec::new();
        for (id, path) in &all {
            let opened = crate::photo::open_source(path, Cancel::NEVER)?;
            let metadata = opened.metadata().clone();
            let (profiles, _) = crate::camera_profiles::installed(&metadata);
            let image = match opened {
                crate::photo::Opened::Raw(raw) => raw.develop(
                    crate::camera_data::Decode::full(Default::default()),
                    Cancel::NEVER,
                )?,
                crate::photo::Opened::Rendered(image) => image,
            };
            let starting = defaults.resolve(&metadata, &profiles).recipe;
            let expected = crate::develop::auto_tone(&image, &starting, Cancel::NEVER)?;
            let saved = f.catalog.load_edit(*id, path)?.unwrap().recipe;
            assert_eq!(saved, expected, "{}", path.display());
            let history = f.catalog.load_history(*id)?.unwrap();
            assert_eq!(history.steps.last().unwrap().name, "Auto Settings");
            exposures.push(saved.exposure);
        }
        // Measured on each: the scenes get different values.
        assert!(
            exposures[0] > exposures[1] && exposures[1] > exposures[2],
            "{exposures:?}"
        );
        // Measured again, nothing changes: no photo gets another step.
        let again = run(
            &mut f.catalog,
            &BatchChange::AutoSettings,
            &targets,
            &BatchProgress::new("Auto Settings", targets.len()),
        );
        assert!(again.synced.is_empty() && again.failed.is_empty());
        assert_eq!(f.catalog.load_history(all[0].0)?.unwrap().steps.len(), 1);
        // One Undo: none of them has an edit again. Redo: what was measured.
        let path = |id: PhotoId| all.iter().find(|(p, _)| *p == id).map(|(_, p)| p.clone());
        let mut session = open_session(&f._dir)?;
        restore(&mut session, &result.synced, SyncSide::Before, path)?;
        for (id, path) in &all {
            assert!(f.catalog.load_edit(*id, path)?.is_none());
        }
        restore(&mut session, &result.synced, SyncSide::After, path)?;
        for synced in &result.synced {
            let saved = f.catalog.load_edit(synced.id, &synced.path)?.unwrap();
            assert_eq!(saved.recipe, synced.after);
        }
        Ok(())
    }

    /// A saved edit whose Upright is still to be analysed is analysed before Auto
    /// measures it, as opening it in Develop does before its Auto can run.
    #[test]
    fn batch_auto_measures_after_a_pending_upright_analysis() -> Result<()> {
        let mut f = catalog()?;
        let folder = f.photos[0].1.parent().unwrap().to_path_buf();
        let png = folder.join("stripes.png");
        let (s, c) = 4f32.to_radians().sin_cos();
        image::RgbImage::from_fn(600, 400, |x, y| {
            let (fx, fy) = (x as f32 - 300., y as f32 - 200.);
            let v = if ((-s * fx + c * fy) / 50.).rem_euclid(2.) < 1. {
                40. + 200. * x as f32 / 600.
            } else {
                20.
            };
            image::Rgb([v as u8; 3])
        })
        .save(&png)?;
        f.catalog.add_folder(&folder)?;
        let id = f
            .catalog
            .photos()?
            .into_iter()
            .find(|p| p.path == png)
            .unwrap()
            .id;
        let opened = crate::photo::open_source(&png, Cancel::NEVER)?;
        let image = match opened {
            crate::photo::Opened::Rendered(image) => image,
            crate::photo::Opened::Raw(_) => unreachable!(),
        };
        let mut start = Recipe::with_profiles(&image.metadata, &[]);
        start.upright.mode = crate::model::transform::UprightMode::Guided;
        start.upright.guides = vec![
            crate::model::transform::UprightGuide {
                a: [0.15, 0.1],
                b: [0.3, 0.9],
            },
            crate::model::transform::UprightGuide {
                a: [0.85, 0.1],
                b: [0.7, 0.9],
            },
        ];
        assert!(start.upright.needs_analysis());
        f.catalog.save_edit(
            id,
            &png,
            &crate::storage::Identity::read(&png)?,
            &start,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        let result = run(
            &mut f.catalog,
            &BatchChange::AutoSettings,
            &[target(id, &png)],
            &BatchProgress::new("Auto Settings", 1),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        // Develop: the analysis lands, then Auto measures the photo as levelled.
        let mut analysed = start.clone();
        crate::develop::upright::complete(&mut analysed, &image);
        let expected = crate::develop::auto_tone(&image, &analysed, Cancel::NEVER)?;
        assert_eq!(f.catalog.load_edit(id, &png)?.unwrap().recipe, expected);
        // Undo goes back to the edit as it was saved.
        assert_eq!(
            result.synced[0].before,
            EditBefore::Saved(Box::new(start.clone()))
        );
        // With Transform off and the sliders already Auto's, Develop's Auto changes
        // nothing: the analysis landing is not an edit that turns Transform on.
        use crate::model::panels::{Panel, PanelState};
        let mut settled = start;
        settled.panels.set(Panel::Transform, PanelState::Off);
        let mut analysed = settled.clone();
        crate::develop::upright::complete(&mut analysed, &image);
        let auto = crate::develop::auto_tone(&image, &analysed, Cancel::NEVER)?;
        crate::develop::AutoTone::of(&auto).apply(&mut settled);
        f.catalog.save_edit(
            id,
            &png,
            &crate::storage::Identity::read(&png)?,
            &settled,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        let result = run(
            &mut f.catalog,
            &BatchChange::AutoSettings,
            &[target(id, &png)],
            &BatchProgress::new("Auto Settings", 1),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let saved = f.catalog.load_edit(id, &png)?.unwrap().recipe;
        assert_eq!(saved.panels.state(Panel::Transform), PanelState::Off);
        Ok(())
    }

    /// Photos that can't be measured are reported and left as they were; the rest
    /// are saved.
    #[test]
    fn batch_auto_settings_reports_photos_it_cannot_measure() -> Result<()> {
        let mut f = catalog()?;
        let png = scenes(&mut f, &["a.png"])?;
        let missing = f.photos[1].1.clone();
        std::fs::remove_file(&missing)?;
        let targets = [
            target(png[0].0, &png[0].1),
            target(f.photos[1].0, &missing),
            target(f.photos[3].0, &f.photos[3].1),
        ];
        let result = run(
            &mut f.catalog,
            &BatchChange::AutoSettings,
            &targets,
            &BatchProgress::new("Auto Settings", 3),
        );
        assert_eq!(result.synced.len(), 1);
        let failed: Vec<_> = result.failed.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(failed, ["b.dng", "d.dng"]);
        Ok(())
    }

    /// Cancel discards the whole batch, whenever it comes before saving: before the
    /// first photo, between photos, or after the last one is prepared.
    #[test]
    fn a_cancelled_batch_saves_nothing() -> Result<()> {
        let mut f = catalog()?;
        let png = scenes(&mut f, &["a.png", "b.png", "c.png"])?;
        let targets: Vec<_> = png.iter().map(|(id, p)| target(*id, p)).collect();
        let before = BatchProgress::new("Auto Settings", 3);
        assert!(before.cancel());
        for progress in [
            before,
            BatchProgress::new("Auto Settings", 3).cancelling_after(1),
            BatchProgress::new("Auto Settings", 3).cancelling_after(3),
        ] {
            let result = run(
                &mut f.catalog,
                &BatchChange::AutoSettings,
                &targets,
                &progress,
            );
            assert!(result.cancelled);
            assert!(result.synced.is_empty() && result.failed.is_empty());
            for (id, path) in &png {
                assert!(f.catalog.load_edit(*id, path)?.is_none());
            }
        }
        // Once it saves, it is too late to cancel.
        let saving = BatchProgress::new("Auto Settings", 3);
        assert!(saving.begin_saving());
        assert!(!saving.cancel());
        assert_eq!(saving.stage(), BatchStage::Saving);
        Ok(())
    }

    /// A batch that panics or can't open the catalog still reports back, so the
    /// window stops waiting for it; nothing changed.
    #[test]
    fn a_batch_that_panics_still_finishes() {
        let catalog = CatalogLocation::from(Path::new("/nowhere/c.rawmakase"));
        let panicked = std::panic::catch_unwind(|| -> Result<SyncResult> {
            panic!("in Upright's analysis");
        });
        let result = finished(panicked, &BatchChange::AutoSettings, &catalog);
        assert!(result.synced.is_empty() && !result.cancelled);
        assert_eq!(result.failed[0].reason, "the Sync failed unexpectedly");
        let failed = finished(
            Ok(Err(anyhow::anyhow!("locked"))),
            &BatchChange::AutoSettings,
            &catalog,
        );
        assert_eq!(failed.failed[0].reason, "locked");
    }

    /// A change that takes settings from a source photo changes nothing without one.
    #[test]
    fn a_sync_without_its_source_changes_nothing() -> Result<()> {
        let mut f = catalog()?;
        let targets = [target(f.photos[1].0, &f.photos[1].1)];
        let result = run(
            &mut f.catalog,
            &BatchChange::Settings(GroupSelection::default()),
            &targets,
            &BatchProgress::new("Sync", 1),
        );
        assert!(result.synced.is_empty());
        assert_eq!(result.failed.len(), 1);
        Ok(())
    }

    /// A Sync that only edits a switched-off panel turns it on, as the same change
    /// made in Develop does: the photo shows what was synchronized.
    #[test]
    fn sync_turns_on_a_switched_off_panel_it_edits_as_develop_does() -> Result<()> {
        use crate::model::panels::{Panel, PanelState};
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.sharpening = 0.9;
        source.panels.set(Panel::Detail, PanelState::Off);
        let (id, path) = photos[1].clone();
        let mut before = Recipe::with_profiles(&metadata, &profiles);
        before.panels.set(Panel::Detail, PanelState::Off);
        c.save_edit(
            id,
            &path,
            &crate::storage::Identity::read(&path)?,
            &before,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        let mut sharpening = GroupSelection::none();
        sharpening.set(
            settings_groups::SettingGroup::Sharpening,
            settings_groups::GroupInclusion::Included,
        );
        let source = Settings {
            recipe: source,
            metadata: metadata.clone(),
        };
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source.clone()))),
            &BatchChange::Settings(sharpening.clone()),
            &[target(id, &path)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let saved = c.load_edit(id, &path)?.unwrap().recipe;
        // What Develop makes of the same change.
        let transferred = settings_groups::transfer(
            Source {
                recipe: &source.recipe,
                metadata: &metadata,
            },
            &before,
            &sharpening,
            Target {
                metadata: &metadata,
                profiles: &profiles,
            },
        )
        .recipe;
        let mut develop = crate::edit_session::EditSession::default();
        *develop.setup_mut() = before;
        develop.change(None, |r| *r = transferred);
        assert_eq!(saved.panels.state(Panel::Detail), PanelState::On);
        assert_eq!(saved, *develop.recipe());
        Ok(())
    }

    #[test]
    fn match_total_exposures_evens_out_aperture_shutter_and_iso() -> Result<()> {
        let camera = |aperture: f32, shutter: f32, iso: f32| crate::camera_data::Metadata {
            aperture,
            shutter,
            iso,
            ..Default::default()
        };
        let source = Settings {
            recipe: Recipe {
                exposure: 0.5,
                ..Default::default()
            },
            metadata: camera(4., 1. / 125., 100.),
        };
        // A stop more light (1/60 s): a stop less Exposure. Twice the ISO and a stop
        // smaller aperture cancel out.
        assert!(
            (matched_exposure(&source, &camera(4., 1. / 62.5, 100.)).unwrap() + 0.5).abs() < 1e-4
        );
        assert!(
            (matched_exposure(&source, &camera(5.6568, 1. / 125., 200.)).unwrap() - 0.5).abs()
                < 1e-3
        );
        assert_eq!(
            matched_exposure(&source, &camera(0., 1. / 125., 100.)),
            None
        );
        // On photos without those settings it says so and leaves them alone.
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source))),
            &BatchChange::MatchTotalExposures,
            &[target(photos[1].0, &photos[1].1)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.synced.is_empty());
        assert!(
            result.failed[0].reason.contains("aperture"),
            "{:?}",
            result.failed
        );
        Ok(())
    }

    /// From the Library the source is a photo not open: its saved edit is what the
    /// others take, read with them.
    #[test]
    fn sync_from_a_photo_not_open_takes_its_saved_edit() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let (source_id, source_path) = photos[0].clone();
        let metadata = crate::photo::open(&source_path)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut edit = Recipe::with_profiles(&metadata, &profiles);
        edit.exposure = 0.7;
        c.save_edit(
            source_id,
            &source_path,
            &crate::storage::Identity::read(&source_path)?,
            &edit,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Photo(target(source_id, &source_path))),
            &BatchChange::Settings(GroupSelection::default()),
            &[target(photos[1].0, &photos[1].1)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        assert_eq!(result.synced.len(), 1);
        let saved = c.load_edit(photos[1].0, &photos[1].1)?.unwrap().recipe;
        assert_eq!(saved.exposure, 0.7);
        Ok(())
    }

    /// A source whose edit is protected (its file changed since) gives nothing: the
    /// camera defaults are not its settings.
    #[test]
    fn sync_from_a_protected_photo_changes_nothing() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let (source_id, source_path) = photos[0].clone();
        let edit = Recipe {
            exposure: 0.7,
            ..Default::default()
        };
        c.save_edit(
            source_id,
            &source_path,
            &crate::storage::Identity::read(&source_path)?,
            &edit,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        {
            use std::io::Write;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&source_path)?
                .write_all(b"\0")?;
        }
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Photo(target(source_id, &source_path))),
            &BatchChange::Settings(GroupSelection::default()),
            &[target(photos[1].0, &photos[1].1)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.synced.is_empty());
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert_eq!(result.failed[0].name, "a.dng");
        assert!(
            result.failed[0].reason.contains("protected"),
            "{:?}",
            result.failed
        );
        assert!(c.load_edit(photos[1].0, &photos[1].1)?.is_none());
        Ok(())
    }

    #[test]
    fn a_history_from_a_newer_release_is_not_replaced() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let source = Settings {
            recipe: Recipe {
                exposure: 0.3,
                ..Default::default()
            },
            metadata,
        };
        let (id, path) = (photos[1].0, photos[1].1.clone());
        c.save_edit(
            id,
            &path,
            &crate::storage::Identity::read(&path)?,
            &Recipe::default(),
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )?;
        // What a newer release might store: a format this one can't read.
        let newer = {
            use std::io::Write;
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
            z.write_all(br#"{"version": 99}"#)?;
            z.finish()?
        };
        c.db_for_tests().execute(
            "INSERT INTO develop_history(photo, data) VALUES (?, ?)",
            rusqlite::params![id.0, newer],
        )?;
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source))),
            &BatchChange::Settings(GroupSelection::default()),
            &[target(id, &path)],
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert_eq!(c.load_edit(id, &path)?.unwrap().recipe.exposure, 0.);
        Ok(())
    }

    #[test]
    fn a_lightroom_edit_that_cant_be_read_fails_its_photo_and_notes_are_reported() -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let mut recipe = Recipe::default();
        recipe.upright.mode = crate::model::transform::UprightMode::Guided;
        recipe.exposure = 0.3;
        let source = Settings { recipe, metadata };
        // Settings cut off mid-value.
        c.db_for_tests().execute(
            "UPDATE photos SET lightroom_develop='s = { Exposure2012 = ' WHERE id=?",
            [photos[1].0.0],
        )?;
        let targets = [
            target(photos[1].0, &photos[1].1),
            target(photos[2].0, &photos[2].1),
        ];
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source))),
            &BatchChange::Settings(GroupSelection::default()),
            &targets,
            &DevelopDefaults::default(),
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert_eq!(result.failed.len(), 1, "{:?}", result.failed);
        assert!(result.failed[0].reason.contains("Lightroom edit"));
        assert!(c.load_edit(photos[1].0, &photos[1].1)?.is_none());
        // Guided Upright can't move without this photo's guides: said, not hidden.
        assert_eq!(result.synced.len(), 1);
        assert!(
            result.notes.iter().any(|n| n.note.contains("Guided")),
            "{:?}",
            result.notes
        );
        Ok(())
    }

    #[test]
    fn photos_without_an_edit_start_from_the_raw_defaults_and_lightroom_edits_from_adobe_default()
    -> Result<()> {
        let Fixture {
            _dir,
            catalog: mut c,
            photos,
        } = catalog()?;
        let lightroom_text = "s = { Exposure2012 = 0.25 }";
        c.db_for_tests().execute(
            "UPDATE photos SET lightroom_develop=? WHERE id=?",
            rusqlite::params![lightroom_text, photos[1].0.0],
        )?;
        let metadata = crate::photo::open(&photos[0].1)?.metadata;
        let (profiles, _) = crate::camera_profiles::installed(&metadata);
        let mut source = Recipe::with_profiles(&metadata, &profiles);
        source.effects.clarity = 0.2;
        let source = Settings {
            recipe: source,
            metadata: metadata.clone(),
        };
        let targets = [
            target(photos[1].0, &photos[1].1),
            target(photos[2].0, &photos[2].1),
        ];
        let mut clarity = GroupSelection::none();
        clarity.set(
            settings_groups::SettingGroup::Clarity,
            settings_groups::GroupInclusion::Included,
        );
        let defaults = crate::raw_defaults::brighter_defaults();
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source.clone()))),
            &BatchChange::Settings(clarity),
            &targets,
            &defaults,
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        assert!(result.failed.is_empty(), "{:?}", result.failed);
        let starting =
            |id: PhotoId| match &result.synced.iter().find(|s| s.id == id).unwrap().before {
                EditBefore::None { starting } => (**starting).clone(),
                EditBefore::Saved(_) => panic!("photo {id} had no edit"),
            };
        // The Lightroom edit is relative to Adobe Default, whatever the raw defaults.
        let converted =
            crate::lr_develop::convert_develop(lightroom_text, &metadata, &profiles, None)?.0;
        assert_eq!(starting(photos[1].0), converted);
        assert_eq!(converted.exposure, 0.25);
        // The photo without one starts from the raw defaults.
        let start = starting(photos[2].0);
        assert_eq!(start, defaults.resolve(&metadata, &profiles).recipe);
        assert_eq!(start.exposure, 0.7);
        let saved = c.load_edit(photos[2].0, &photos[2].1)?.unwrap().recipe;
        assert_eq!((saved.exposure, saved.effects.clarity), (0.7, 0.2));

        // A raw default that can't be used is reported as such, not as a Lightroom edit.
        let missing = DevelopDefaults::with_presets(
            crate::raw_defaults::RawDefaults {
                master: crate::raw_defaults::DefaultChoice::Preset {
                    id: "gone".into(),
                    name: "Gone".into(),
                },
                ..Default::default()
            },
            |_| None,
        );
        let result = synchronize(
            &mut c,
            Some(&SyncSource::Open(Box::new(source))),
            &BatchChange::Settings(GroupSelection::default()),
            &[target(photos[0].0, &photos[0].1)],
            &missing,
            &Decoding::default(),
            &BatchProgress::new("Sync", 0),
        );
        let notes: Vec<_> = result.notes.iter().map(|n| n.note.as_str()).collect();
        assert!(
            notes.iter().any(|n| n.contains("‘Gone’ is missing")),
            "{notes:?}"
        );
        assert!(!notes.iter().any(|n| n.contains("Lightroom")), "{notes:?}");
        Ok(())
    }
}

#[cfg(test)]
#[path = "sync_editor_tests.rs"]
mod editor_tests;
