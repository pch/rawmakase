//! Exporting several photos at once, each with its own edit, as Lightroom's Export
//! does with more than one photo selected.
//!
//! A batch is taken when Export is pressed: every photo's edit and metadata as the
//! catalog has them then ([`BatchPhoto`]), and no image. [`plan`] decides where
//! each file goes before anything renders; [`run`] then works out, renders and
//! writes one photo at a time, so a batch of any size holds one photo's images.
use super::{Replace, assemble::Values, job};
use crate::export_settings::{Existing, ExportSettings, NameFields};
use crate::{
    edits::{self, EditRecord, Origin, PhotoRecord},
    model::recipe::Recipe,
    photo::Opened,
    raw_defaults::DevelopDefaults,
};
use anyhow::{Context, Result, bail, ensure};
use rawmakase_cancel::Cancel;
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
};

/// One photo of a batch, as it was when Export was pressed.
#[derive(Clone, Debug)]
pub struct BatchPhoto {
    pub source: PathBuf,
    /// How the photo is named in a report: its file name, and a virtual copy's
    /// name after it.
    pub name: String,
    pub edit: Edit,
    pub values: Values,
    /// What its name is made from, as read when Export was chosen: the
    /// Export dialog's Example was made from the same.
    pub naming: NameFields,
    /// Its place among the photos chosen, and how many were chosen, for the
    /// sequence numbers: a photo left out keeps its number. `None` numbers it by
    /// its place in the batch.
    pub place: Option<(usize, usize)>,
}

/// The edit a photo is exported with.
#[derive(Clone, Debug)]
pub enum Edit {
    /// As the catalog stores it, worked out when the photo's turn comes, as
    /// Develop would open it (`crate::edits`).
    Catalog(EditRecord),
    /// The open photo's edit as shown; `unsaved` when saving it failed. `file` is
    /// the photo's file when Export was pressed: a file replaced since is not
    /// exported with an edit made for another.
    Shown {
        recipe: Box<Recipe>,
        unsaved: bool,
        file: Option<crate::storage::Identity>,
    },
}

impl BatchPhoto {
    /// A photo of the batch from its catalog record, named from `naming`.
    pub fn from_record(
        record: PhotoRecord,
        source: PathBuf,
        name: String,
        naming: NameFields,
    ) -> Self {
        Self {
            source,
            name,
            naming,
            place: None,
            edit: Edit::Catalog(record.edit),
            values: Values {
                descriptive: record.descriptive,
                keywords: record
                    .keywords
                    .into_iter()
                    .map(|k| crate::xmp::write::KeywordPath {
                        path: k.path,
                        exported: k.exported,
                    })
                    .collect(),
                rating: record.rating,
                label: record.label,
            },
        }
    }
}

/// How one photo's file is written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Write {
    /// A name nothing had when the batch was planned.
    Create,
    /// Over a file that was there, as asked.
    Overwrite,
    /// Not at all: a file was there, and the answer was Skip.
    Skip,
}

/// Where one photo goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Planned {
    pub target: PathBuf,
    pub write: Write,
    /// What its name has to say, such as a missing capture date.
    pub notes: Vec<String>,
}

/// Where every photo of a batch goes, decided before anything renders.
#[derive(Clone, Debug)]
pub struct Plan {
    /// One for each photo, in order.
    pub entries: Vec<Planned>,
    /// What a file that appears while the batch runs gets: the answer the batch
    /// was given, or Skip when Ask had nothing to ask about.
    pub late: Existing,
    reserved: Reservations,
}

/// Why a batch could not be planned.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unplanned {
    /// "Specific folder" with no folder chosen.
    NoFolder,
    /// Ask what to do: these files exist. Plan again with the answer.
    Conflicts(Vec<PathBuf>),
    /// The settings can't be exported as they are (the reason): a colour space the
    /// format can't hold, or one only a later release knows.
    Settings(String),
}
impl std::fmt::Display for Unplanned {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unplanned::NoFolder => f.write_str("Choose a folder to export to"),
            Unplanned::Conflicts(files) => write!(f, "{} files already exist", files.len()),
            Unplanned::Settings(reason) => f.write_str(reason),
        }
    }
}

/// The names taken in each destination folder, compared without case, so the
/// names chosen work on any volume. Conservative on a case-sensitive one, where it
/// can rename a file that did not need it.
#[derive(Clone, Debug, Default)]
struct Reservations {
    /// Names already in each folder when the batch was planned, folded, with the
    /// files' own names: more than one on a case-sensitive volume.
    on_disk: HashMap<PathBuf, HashMap<String, Vec<OsString>>>,
    /// Names given to the batch's own files.
    batch: HashMap<PathBuf, HashSet<String>>,
}
fn fold(name: &std::ffi::OsStr) -> String {
    name.to_string_lossy().to_lowercase()
}
impl Reservations {
    fn folder(&mut self, dir: &Path) -> &HashMap<String, Vec<OsString>> {
        self.on_disk.entry(dir.to_path_buf()).or_insert_with(|| {
            let mut names: HashMap<String, Vec<OsString>> = HashMap::new();
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                names
                    .entry(fold(&entry.file_name()))
                    .or_default()
                    .push(entry.file_name());
            }
            names
        })
    }
    /// The file already in `path`'s folder under its name, as it is spelled there:
    /// the exact name when there is one, else the name in other case.
    fn existing(&mut self, path: &Path) -> Option<PathBuf> {
        let name = path.file_name().unwrap_or_default();
        let dir = crate::storage::parent_dir(path);
        let found = self.folder(dir).get(&fold(name))?;
        let spelled = found.iter().find(|n| *n == name).or(found.first())?;
        Some(dir.join(spelled))
    }
    fn on_disk(&mut self, path: &Path) -> bool {
        self.existing(path).is_some()
    }
    fn in_batch(&self, path: &Path) -> bool {
        let name = fold(path.file_name().unwrap_or_default());
        self.batch
            .get(crate::storage::parent_dir(path))
            .is_some_and(|names| names.contains(&name))
    }
    fn reserve(&mut self, path: &Path) {
        let name = fold(path.file_name().unwrap_or_default());
        self.batch
            .entry(crate::storage::parent_dir(path).to_path_buf())
            .or_default()
            .insert(name);
    }
    /// A file found while the batch runs: no later photo takes its name.
    fn found(&mut self, path: &Path) {
        let name = path.file_name().unwrap_or_default();
        let dir = crate::storage::parent_dir(path).to_path_buf();
        self.folder(&dir);
        self.on_disk
            .entry(dir)
            .or_default()
            .entry(fold(name))
            .or_default()
            .push(name.to_os_string());
    }
    /// The first of `path`, "name-2", "name-3"… that is neither in the folder nor
    /// the batch's, reserved for the batch.
    fn unique(&mut self, path: &Path) -> PathBuf {
        let stem = path.file_stem().unwrap_or_default().to_string_lossy();
        let extension = path.extension().map(|e| e.to_string_lossy());
        let named = |n: usize| {
            let mut name = OsString::from(if n == 1 {
                stem.to_string()
            } else {
                format!("{stem}-{n}")
            });
            if let Some(extension) = &extension {
                name.push(format!(".{extension}"));
            }
            path.with_file_name(name)
        };
        let free = (1..)
            .map(named)
            .find(|p| !self.in_batch(p) && !self.on_disk(p))
            .expect("an unused name");
        self.reserve(&free);
        free
    }
}

/// Where each of `photos` goes with `settings`. With Ask, files that already exist
/// are returned for the question unless `answer` is given; the answer applies to
/// all of them. A later photo never replaces an earlier one's file: it gets the
/// next unique name, as Use Unique Names does.
pub fn plan(
    photos: &[BatchPhoto],
    settings: &ExportSettings,
    answer: Option<Existing>,
) -> Result<Plan, Unplanned> {
    // Nothing is planned, asked or rendered for settings no photo could export with.
    settings
        .check_output()
        .map_err(|e| Unplanned::Settings(format!("{e:#}")))?;
    // Numbered in the order the photos were chosen, so a photo that fails or is
    // skipped keeps its number and the others theirs.
    let total = photos.len();
    let (targets, notes): (Vec<PathBuf>, Vec<Vec<String>>) = photos
        .iter()
        .enumerate()
        .map(|(index, p)| {
            let (index, total) = p.place.unwrap_or((index, total));
            settings.target_for(
                &p.source,
                &crate::export_settings::NameContext {
                    index,
                    total,
                    fields: &p.naming,
                },
            )
        })
        .collect::<Option<Vec<_>>>()
        .ok_or(Unplanned::NoFolder)?
        .into_iter()
        .unzip();
    let mut reserved = Reservations::default();
    let mut conflicts: Vec<PathBuf> = Vec::new();
    for target in &targets {
        if let Some(existing) = reserved.existing(target)
            && !conflicts.contains(&existing)
        {
            conflicts.push(existing);
        }
    }
    let choice = match (settings.existing, answer) {
        (Existing::Ask, Some(answer)) if answer != Existing::Ask => answer,
        (Existing::Ask, _) if !conflicts.is_empty() => return Err(Unplanned::Conflicts(conflicts)),
        // Nothing to ask about: a file that appears later has no answer, so it is
        // not replaced.
        (Existing::Ask, _) => Existing::Skip,
        (existing, _) => existing,
    };
    let entries = targets
        .into_iter()
        .zip(notes)
        .map(|(target, notes)| {
            if reserved.in_batch(&target) {
                let target = reserved.unique(&target);
                return Planned {
                    target,
                    write: Write::Create,
                    notes,
                };
            }
            if !reserved.on_disk(&target) {
                reserved.reserve(&target);
                return Planned {
                    target,
                    write: Write::Create,
                    notes,
                };
            }
            match choice {
                // The file that is there, as it is spelled: not a second one beside
                // it in other case.
                Existing::Overwrite => {
                    let target = reserved.existing(&target).unwrap_or(target);
                    reserved.reserve(&target);
                    Planned {
                        target,
                        write: Write::Overwrite,
                        notes,
                    }
                }
                Existing::Unique => Planned {
                    target: reserved.unique(&target),
                    write: Write::Create,
                    notes,
                },
                Existing::Skip | Existing::Ask => Planned {
                    target,
                    write: Write::Skip,
                    notes,
                },
            }
        })
        .collect();
    Ok(Plan {
        entries,
        late: choice,
        reserved,
    })
}

/// What happened to one photo.
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// Written to `path`, with what the export has to say besides.
    Exported {
        path: PathBuf,
        notes: Vec<String>,
    },
    Skipped(String),
    Failed(String),
    /// Stopped part-way by Cancel: nothing written.
    Cancelled,
    /// Cancelled before its turn.
    NotStarted,
}

/// A batch ready to run: its photos, where each goes, and the settings it shares.
#[derive(Clone, Debug)]
pub struct Batch {
    pub photos: Vec<BatchPhoto>,
    pub plan: Plan,
    pub settings: ExportSettings,
    /// What a photo without an edit starts from, as when it is opened.
    pub defaults: Arc<DevelopDefaults>,
    /// The watermark chosen in the Export dialog.
    pub watermark: Option<crate::watermark::Watermark>,
    /// The demosaic each photo is decoded with.
    pub demosaic: crate::camera_data::Demosaic,
    /// Where each full-size decode is looked up first.
    pub cache: crate::decode_cache::DecodeCache,
    /// Where AI Denoise sources come from.
    pub ai: crate::ai_denoise::AiService,
}

/// How far a batch is: `done` photos of `total`, and how far the current one is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Progress {
    pub done: usize,
    pub total: usize,
    pub fraction: f32,
}

/// Exports `batch` one photo at a time, reporting progress, and returns each
/// photo's outcome. Cancel stops between stages: a cancel observed before a file
/// is committed discards it; a commit already under way may finish, and that photo
/// counts as exported.
pub fn run(batch: &Batch, cancel: Cancel<'_>, progress: impl Fn(Progress)) -> Vec<Outcome> {
    let total = batch.photos.len();
    // A preset's image or font, read once: every photo gets the same watermark,
    // however long the batch runs. The Simple Copyright Watermark takes each
    // photo's own copyright, so it is made photo by photo.
    let preset = match &batch.watermark {
        Some(w) if w.name != crate::watermark::SIMPLE_COPYRIGHT => {
            Some(w.ready().map_err(|e| format!("{e:#}")))
        }
        _ => None,
    };
    let mut reserved = batch.plan.reserved.clone();
    let mut outcomes = Vec::with_capacity(total);
    for (done, (photo, planned)) in batch.photos.iter().zip(&batch.plan.entries).enumerate() {
        if cancel.is_cancelled() {
            outcomes.push(Outcome::NotStarted);
            continue;
        }
        let report = |fraction| {
            progress(Progress {
                done,
                total,
                fraction,
            })
        };
        report(0.);
        let outcome = match planned.write {
            Write::Skip => Outcome::Skipped("a file with its name already exists".into()),
            // A panic fails its own photo; the others keep their outcomes.
            _ => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &preset {
                Some(Err(reason)) => Outcome::Failed(reason.clone()),
                preset => {
                    let preset = preset.as_ref().and_then(|p| p.as_ref().ok());
                    export(
                        batch,
                        photo,
                        planned,
                        preset,
                        &mut reserved,
                        cancel,
                        &report,
                    )
                }
            }))
            .unwrap_or_else(|_| Outcome::Failed("the export failed unexpectedly".into())),
        };
        outcomes.push(outcome);
    }
    progress(Progress {
        done: total,
        total,
        fraction: 0.,
    });
    outcomes
}

/// Why a photo stopped, kept apart from failures until the end.
enum Stop {
    Cancelled,
    Skipped(String),
    Failed(anyhow::Error),
}
impl From<anyhow::Error> for Stop {
    fn from(e: anyhow::Error) -> Self {
        Stop::Failed(e)
    }
}

fn export(
    batch: &Batch,
    photo: &BatchPhoto,
    planned: &Planned,
    preset: Option<&crate::watermark::Ready>,
    reserved: &mut Reservations,
    cancel: Cancel<'_>,
    progress: &impl Fn(f32),
) -> Outcome {
    let result = (|| -> Result<(PathBuf, Vec<String>), Stop> {
        let (prepared, mut notes) = render(batch, photo, preset, cancel, progress)?;
        notes.extend(planned.notes.iter().cloned());
        if let Some(parent) = planned.target.parent() {
            std::fs::create_dir_all(parent).map_err(anyhow::Error::from)?;
        }
        let staged = super::stage_pixels(
            &planned.target,
            &prepared.rendered,
            &prepared.metadata,
            &prepared.options,
            &prepared.embed,
        )?;
        progress(0.95);
        let (path, note) = commit(
            staged.file,
            photo,
            planned,
            batch.plan.late,
            reserved,
            cancel,
        )?;
        notes.extend(note);
        progress(1.);
        Ok((path, notes))
    })();
    match result {
        Ok((path, notes)) => Outcome::Exported { path, notes },
        Err(Stop::Skipped(reason)) => Outcome::Skipped(reason),
        // Whatever stopped it, a cancel seen is a cancel, not a failure.
        Err(_) if cancel.is_cancelled() => Outcome::Cancelled,
        Err(Stop::Cancelled) => Outcome::Cancelled,
        Err(Stop::Failed(e)) => Outcome::Failed(format!("{e:#}")),
    }
}

/// `photo` rendered with its edit, and what the export has to say about it.
fn render(
    batch: &Batch,
    photo: &BatchPhoto,
    preset: Option<&crate::watermark::Ready>,
    cancel: Cancel<'_>,
    progress: &impl Fn(f32),
) -> Result<(job::Prepared, Vec<String>)> {
    // The file as it is before it is decoded: AI Denoise's result must be of the same.
    let file = crate::storage::Identity::read(&photo.source).ok();
    let opened = crate::photo::open_source(&photo.source, cancel)
        .with_context(|| format!("{} can't be read", photo.source.display()))?;
    let (mut recipe, mut notes) = match &photo.edit {
        Edit::Catalog(record) => {
            let (profiles, _) = crate::camera_profiles::installed(opened.metadata());
            let profiles = rawmakase_decode::camera_look::with_default(
                &photo.source,
                opened.metadata(),
                profiles,
                cancel,
            );
            let resolved = edits::resolve(
                record,
                &photo.source,
                opened.metadata(),
                &profiles,
                &batch.defaults,
            )?;
            let notes = match resolved.origin {
                Origin::Lightroom => resolved
                    .warnings
                    .into_iter()
                    .map(|w| format!("Lightroom edit not fully rendered: {w}"))
                    .collect(),
                _ => resolved.warnings,
            };
            (resolved.recipe, notes)
        }
        Edit::Shown {
            recipe,
            unsaved,
            file,
        } => {
            if let Some(file) = file {
                ensure!(
                    crate::storage::Identity::read(&photo.source)?.same_photo(file),
                    "the photo's file changed since it was shown; open it again to export it"
                );
            }
            (
                (**recipe).clone(),
                unsaved
                    .then(|| "exported using unsaved adjustments".to_string())
                    .into_iter()
                    .collect(),
            )
        }
    };
    ensure!(!cancel.is_cancelled(), "Cancelled");
    let image = Arc::new(match opened {
        Opened::Raw(raw) => {
            job::decode_full(raw, &photo.source, batch.demosaic, &batch.cache, cancel)?
        }
        // A JPEG, PNG or TIFF opens at full size: there is nothing to decode or cache.
        Opened::Rendered(image) => image,
    });
    // Upright's corrections, as Develop works them out once the photo is open.
    if let Some(issue) = crate::develop::upright::complete(&mut recipe, &image)
        && recipe.upright.mode == crate::model::transform::UprightMode::Guided
    {
        notes.push(issue.message().into());
    }
    if preset.is_some() && batch.settings.hdr.is_some() {
        anyhow::bail!("Watermarks are not supported in HDR exports yet");
    }
    let mut prepared = job::prepare(
        &job::Photo {
            image,
            source: photo.source.clone(),
            recipe,
            values: photo.values.clone(),
            watermark: batch.watermark.clone().filter(|_| preset.is_none()),
            demosaic: batch.demosaic,
            cache: batch.cache.clone(),
            ai: batch.ai.clone(),
            file,
        },
        &batch.settings,
        cancel,
        &|p| progress(p * 0.9),
    )?;
    if let Some(preset) = preset {
        let super::Pixels::Sdr(rendered) = &mut prepared.rendered else {
            anyhow::bail!("Watermarks are not supported in HDR exports yet");
        };
        if !preset.apply(rendered) {
            notes.push("the watermark's text has no characters its font can draw".into());
        }
    }
    notes.extend(prepared.notice.clone());
    Ok((prepared, notes))
}

/// Puts a staged export in place, as `planned` says, unless Cancel came first.
/// A file that appeared at a name planned as new gets the batch's answer.
fn commit(
    mut staged: tempfile::NamedTempFile,
    photo: &BatchPhoto,
    planned: &Planned,
    late: Existing,
    reserved: &mut Reservations,
    cancel: Cancel<'_>,
) -> Result<(PathBuf, Option<String>), Stop> {
    // The last moment to stop: dropping the staged file leaves nothing behind.
    if cancel.is_cancelled() {
        return Err(Stop::Cancelled);
    }
    let mut target = planned.target.clone();
    if planned.write == Write::Overwrite {
        // The file as it is spelled now, should it have been renamed meanwhile; the
        // exact name first.
        let target = if exactly(&target) {
            target
        } else {
            spelled_otherwise(&target).unwrap_or(target)
        };
        overwrite(staged, &target, &photo.source)?;
        return Ok((target, None));
    }
    loop {
        // A file under the name in other case is the same name here; an exact-name
        // check alone misses it on a case-sensitive volume.
        // The exact name first, then the name in other case.
        let found = if exactly(&target) {
            Some(target.clone())
        } else {
            spelled_otherwise(&target)
        };
        let appeared = match found {
            Some(existing) => existing,
            None => match crate::storage::persist_noclobber(staged, &target) {
                Ok(_) => {
                    crate::storage::sync_dir(crate::storage::parent_dir(&target))?;
                    return Ok((target, None));
                }
                Err(e) if e.error.kind() == std::io::ErrorKind::AlreadyExists => {
                    staged = e.file;
                    target.clone()
                }
                Err(e) => return Err(Stop::Failed(e.error.into())),
            },
        };
        reserved.found(&appeared);
        match late {
            Existing::Unique => target = reserved.unique(&target),
            Existing::Overwrite => {
                overwrite(staged, &appeared, &photo.source)?;
                return Ok((
                    appeared,
                    Some("replaced a file that appeared during the export".into()),
                ));
            }
            Existing::Skip | Existing::Ask => {
                return Err(Stop::Skipped(
                    "a file with its name appeared during the export".into(),
                ));
            }
        }
    }
}

/// Whether `path`'s folder has a file named exactly as `path` is, case included.
fn exactly(path: &Path) -> bool {
    let (Some(name), dir) = (path.file_name(), crate::storage::parent_dir(path)) else {
        return false;
    };
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| e.file_name() == name)
}

/// The file in `path`'s folder whose name is `path`'s in other case, if any.
fn spelled_otherwise(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let folded = fold(name);
    let dir = crate::storage::parent_dir(path);
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name())
        .find(|other| other != name && fold(other) == folded)
        .map(|other| dir.join(other))
}

/// Replaces the file at `target` with `staged`, never the photo's own file.
fn overwrite(staged: tempfile::NamedTempFile, target: &Path, source: &Path) -> Result<()> {
    if target.exists() && std::fs::canonicalize(target)? == std::fs::canonicalize(source)? {
        bail!("Cannot overwrite source");
    }
    crate::storage::persist(staged, target, Replace::Overwrite)
}

#[cfg(test)]
mod tests;
