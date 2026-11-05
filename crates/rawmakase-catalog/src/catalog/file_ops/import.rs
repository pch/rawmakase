//! Import Photos (#806): photos copied from a card or any folder into the
//! library, named by a filename template, each copy verified, then added to
//! the catalog as Add Folder adds them. A source is never written, renamed or
//! put in the Trash.
//!
//! The scan ([`scan_file`], [`import_companions`]) captures each source once
//! ([`ImportSource`]); planning ([`plan_import`]) decides every destination
//! folder and name, which the dialog shows; [`execute_import`] plans again
//! under the operation lock and runs only when that plan is the one the user
//! confirmed.
//!
//! A run is journalled as `import_copy` items, in one transaction that also
//! reserves its Import # (`catalog_counters`), before any disk step:
//! - one item per photo, or per **family** of photos that read the same
//!   proven source companion and land in one folder under one stem
//!   (`A.NEF` and `A.DNG` beside one `A.xmp`), whose companion is copied
//!   once; its members are its files, companions first and photo files
//!   last, each copied as a move to another drive copies a file
//!   ([`super::copy::write`]: a hidden partial copy made where nothing has
//!   its name, hashed as read, synced, read back), then given its name by an
//!   exclusive rename and journalled `copied`. Its commit adds all its
//!   photos in one transaction, or none;
//! - one **finish** item, which runs once every photo item is decided: it
//!   queues burst stacking for the run's import batch and ends `done`.
//!
//! Item states are the existing ones: `planned`, then `done`; `no_change`
//! when nothing was written whole; `needs_recovery` (only Keep as Is) when a
//! copy written whole or published is left. A copy written whole is never
//! removed: there is no Put Back for an import. Folders a run makes are
//! journalled `create_folder` items, as New Folder Inside makes them.
//!
//! Add Folder and Synchronize leave out the files an unresolved import of
//! this computer will publish ([`reserved`]), so neither adds a photo
//! half-imported.
use super::super::db::{Reads, Write, sql};
use super::super::folder_sync::locate_folders;
use super::super::ingest::{Choice, ImportFacts, ImportKind, Matched, Matcher, Place};
use super::super::locations::{join, logical_location};
use super::super::sidecar::caseless;
use super::super::{Catalog, ImportOptions, MetadataDefaults, NewPhotoDefaults, PhotoId, RootId};
use super::copy::{self, Partial};
use super::disk::{Disk, Presence, Real, parent, presence, spelled};
use super::folder::{FolderState, NewFolder};
use super::journal::{self, Item, ItemState, Kind, MemberState, NewMember, OperationId, Subject};
use super::plan::Folders;
use super::proof::{self, Proof, Proofs};
use super::{BatchId, FileOpError, Outcome, Trash};
use crate::export_settings::{NameContext, NameFields, NameTemplate};
use crate::fsops::{EntryKind, FileId, FsError};
use crate::storage::Identity;
use anyhow::{Context, Result};
use rawmakase_cancel::Cancel;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// A file as the scan found it: its file object and `Identity` (with its
/// first bytes' fingerprint), read once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScannedFile {
    pub path: PathBuf,
    pub file: FileId,
    pub identity: Identity,
}

/// Reads the file object and `Identity` of the plain file at `path`.
pub fn scan_file(path: &Path) -> Result<ScannedFile> {
    let entry = crate::fsops::entry(path)?;
    anyhow::ensure!(
        entry.kind == EntryKind::File,
        "{} is not a plain file",
        path.display()
    );
    Ok(ScannedFile {
        path: path.to_path_buf(),
        file: entry.id,
        identity: Identity::read(path)?,
    })
}

/// The companions the photo file at `photo` reads in its folder (its XMP
/// sidecars and earlier releases' edit files), whether or not another file
/// there reads them too: a copy takes nothing from its source, so each photo
/// copied gets its own.
pub fn import_companions(photo: &Path) -> Result<Vec<PathBuf>> {
    let dir = parent(photo);
    let name = photo.file_name().context("not a file")?;
    let listing = super::companions::Listing::new(crate::fsops::list(dir)?);
    let same_file = |a: &OsStr, b: &OsStr| match (
        crate::fsops::entry(&dir.join(a)),
        crate::fsops::entry(&dir.join(b)),
    ) {
        (Ok(a), Ok(b)) => a.id == b.id,
        _ => false,
    };
    let found = listing.companions(name, &same_file);
    Ok(found
        .carried
        .into_iter()
        .chain(found.shared)
        .map(|n| dir.join(n))
        .collect())
}

/// One photo file to import, as the scan read it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportSource {
    pub file: ScannedFile,
    /// Its companions, each read as it was.
    pub companions: Vec<ScannedFile>,
    /// The capture time its EXIF (or RAW container) gives, as the catalog
    /// keeps it ("2026-04-24T18:43:24.000"); `None` when it has none.
    pub captured: Option<String>,
    /// Its file's modification time, local, in the same form: what dates a
    /// photo without a capture time, as Lightroom dates it (not its
    /// creation time; measured in Lightroom Classic 2026).
    pub modified: Option<String>,
    /// What a filename template takes from it.
    pub fields: NameFields,
}

impl ImportSource {
    fn name(&self) -> &OsStr {
        self.file.path.file_name().unwrap_or_default()
    }
    /// The date its folders and its name take: its capture time, else its
    /// file's modification time.
    pub fn date(&self) -> Option<&str> {
        self.captured.as_deref().or(self.modified.as_deref())
    }
    /// The bytes it copies, companions included.
    fn bytes(&self) -> u64 {
        self.file.identity.size + self.companions.iter().map(|c| c.identity.size).sum::<u64>()
    }
}

/// Lightroom's Organize: where below the destination (and its Into
/// Subfolder) the photos go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Organize {
    /// Into the folders they are in at the source, from the folder chosen as
    /// the source down, that folder included ("card/DCIM/100CANON").
    ByOriginalFolders,
    /// Into folders named by each photo's date.
    ByDate(DateFolders),
    /// All into one folder.
    IntoOneFolder,
}

/// The folders By date makes, as Lightroom Classic's Date Format offers
/// them, kept by Lightroom's own template (`%Y/%Y-%m-%d`). Each example is
/// 19 November 2026.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum DateFolders {
    /// "2026/11-19"
    #[serde(rename = "%Y/%m-%d")]
    YearThenMonthDay,
    /// "2026/2026-11-19", Lightroom's first choice.
    #[serde(rename = "%Y/%Y-%m-%d")]
    YearThenDate,
    /// "2026/November 19"
    #[serde(rename = "%Y/%B %d")]
    YearThenMonthNameDay,
    /// "2026/11"
    #[serde(rename = "%Y/%m")]
    YearThenMonth,
    /// "2026/Nov"
    #[serde(rename = "%Y/%b")]
    YearThenMonthAbbr,
    /// "2026/November"
    #[serde(rename = "%Y/%B")]
    YearThenMonthName,
    /// "2026/11/19"
    #[serde(rename = "%Y/%m/%d")]
    YearMonthDay,
    /// "2026/November/19"
    #[serde(rename = "%Y/%B/%d")]
    YearMonthNameDay,
    /// "2026/2026-11/2026-11-19"
    #[serde(rename = "%Y/%Y-%m/%Y-%m-%d")]
    YearMonthDate,
    /// "2026-11-19"
    #[serde(rename = "%Y-%m-%d")]
    Date,
    /// "2026 Nov 19"
    #[serde(rename = "%Y %b %d")]
    DateMonthAbbr,
    /// "2026 November 19"
    #[serde(rename = "%Y %B %d")]
    DateMonthName,
    /// "20261119"
    #[serde(rename = "%Y%m%d")]
    DateCompact,
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

impl DateFolders {
    /// Lightroom's Date Format menu, in its order and groups (measured in
    /// Lightroom Classic 2026).
    pub const GROUPS: [&'static [DateFolders]; 4] = [
        &[
            Self::YearThenMonthDay,
            Self::YearThenDate,
            Self::YearThenMonthNameDay,
        ],
        &[
            Self::YearThenMonth,
            Self::YearThenMonthAbbr,
            Self::YearThenMonthName,
        ],
        &[
            Self::YearMonthDay,
            Self::YearMonthNameDay,
            Self::YearMonthDate,
        ],
        &[
            Self::Date,
            Self::DateMonthAbbr,
            Self::DateMonthName,
            Self::DateCompact,
        ],
    ];
    /// Lightroom's template for it, folders split by `/`.
    pub fn template(self) -> &'static str {
        match self {
            Self::YearThenMonthDay => "%Y/%m-%d",
            Self::YearThenDate => "%Y/%Y-%m-%d",
            Self::YearThenMonthNameDay => "%Y/%B %d",
            Self::YearThenMonth => "%Y/%m",
            Self::YearThenMonthAbbr => "%Y/%b",
            Self::YearThenMonthName => "%Y/%B",
            Self::YearMonthDay => "%Y/%m/%d",
            Self::YearMonthNameDay => "%Y/%B/%d",
            Self::YearMonthDate => "%Y/%Y-%m/%Y-%m-%d",
            Self::Date => "%Y-%m-%d",
            Self::DateMonthAbbr => "%Y %b %d",
            Self::DateMonthName => "%Y %B %d",
            Self::DateCompact => "%Y%m%d",
        }
    }
    /// The folders for a photo of `date` (a time as the catalog keeps one,
    /// "2026-11-19T10:00:00.000"); `None` when it holds no date.
    pub fn folders(self, date: &str) -> Option<Vec<String>> {
        let year: u32 = date.get(0..4)?.parse().ok()?;
        let month: usize = date.get(5..7)?.parse().ok()?;
        let day: u32 = date.get(8..10)?.parse().ok()?;
        if year == 0 || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return None;
        }
        let name = MONTHS[month - 1];
        let folder = |part: &str| {
            part.replace("%Y", &format!("{year:04}"))
                .replace("%m", &format!("{month:02}"))
                .replace("%d", &format!("{day:02}"))
                .replace("%B", name)
                .replace("%b", &name[..3])
        };
        Some(self.template().split('/').map(folder).collect())
    }
    /// How the menu shows it: the folders of `date`, as Lightroom shows
    /// today's.
    pub fn label(self, date: &str) -> String {
        self.folders(date)
            .map_or_else(|| self.template().to_string(), |f| f.join("/"))
    }
}

/// Everything an import is planned from, captured once when the user asks
/// for it.
#[derive(Clone, Debug)]
pub struct ImportRequest {
    /// The photos to import, in any order.
    pub sources: Vec<ImportSource>,
    /// The folder chosen as the source, which By original folders recreates
    /// from.
    pub source: PathBuf,
    /// The folder they go into (or below), which must be there.
    pub destination: PathBuf,
    /// Lightroom's Into Subfolder: the folder made in the destination that
    /// the photos are organized in; none when empty.
    pub subfolder: String,
    pub organize: Organize,
    /// The template naming the photos (Custom Text and Start Number in it);
    /// `None` keeps their names.
    pub naming: Option<NameTemplate>,
    /// Shoot Name, for a template's Shoot Name.
    pub shoot_name: String,
    /// How a renamed photo's extension is spelt; names that are kept keep
    /// theirs, as in Lightroom.
    pub extensions: ExtensionCase,
    /// Answers to where the destination is in the catalog, when more than
    /// one place could be it.
    pub choices: Vec<Choice>,
    /// What the photos get as Add Folder's: the Preferences' metadata
    /// defaults and readers, and Stack Camera Bursts.
    pub defaults: NewPhotoDefaults,
    /// Lightroom's Apply During Import, applied over what the files hold.
    pub apply: ImportMetadata,
    /// Lightroom's Don't Import Suspected Duplicates: the suspected
    /// duplicates aren't imported. Without it none is looked for, and every
    /// photo is imported, as in Lightroom.
    pub skip_duplicates: bool,
}

/// Lightroom's Extensions, with Rename Files.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ExtensionCase {
    #[default]
    LeaveAsIs,
    Lowercase,
    Uppercase,
}

impl ExtensionCase {
    fn apply(self, extension: &str) -> String {
        match self {
            Self::LeaveAsIs => extension.to_string(),
            Self::Lowercase => extension.to_lowercase(),
            Self::Uppercase => extension.to_uppercase(),
        }
    }
}

/// Lightroom's Apply During Import: what every photo imported is given over
/// what its files hold (measured in Lightroom Classic 2026). A Creator or
/// Copyright replaces the camera's and the sidecar's; the keywords are
/// added to the file's own. An empty field gives nothing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ImportMetadata {
    pub creator: String,
    pub copyright: String,
    /// Keywords by path, top first.
    pub keywords: Vec<Vec<String>>,
}

impl ImportMetadata {
    pub fn is_empty(&self) -> bool {
        self.creator.trim().is_empty()
            && self.copyright.trim().is_empty()
            && self.keywords.is_empty()
    }
}

/// Why a photo is taken for one already in the catalog (Lightroom's
/// "Suspected duplicate").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Duplicate {
    /// A photo of the catalog had its original name, capture time and size.
    InCatalog,
    /// One had its original name and size; a capture time to compare
    /// wasn't known.
    NameAndSize,
    /// An earlier photo of this import has its name, capture time and size.
    InImport,
}

/// One photo of a plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedPhoto {
    pub source: PathBuf,
    /// Where its file is copied to; `None` when it isn't imported.
    pub destination: Option<PathBuf>,
    /// Where its companions are copied to (none of a family's but its
    /// first's: they share one).
    pub companions: Vec<PathBuf>,
    pub duplicate: Option<Duplicate>,
    /// Why it isn't imported.
    pub skipped: Option<String>,
}

/// What an import will do, for the dialog's preview and confirmation.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPlan {
    /// Its Import #.
    pub number: u32,
    /// Every photo of the request, in the order they are named.
    pub photos: Vec<PlannedPhoto>,
    /// Folders it makes, outermost first.
    pub folders: Vec<PathBuf>,
    /// Why nothing can be imported there.
    pub refused: Option<String>,
    /// The places the destination could be, when one must be chosen.
    pub ambiguous: Option<super::super::ingest::Ambiguity>,
    /// The bytes it copies, and those free there.
    pub bytes: u64,
    pub available: u64,
}

impl ImportPlan {
    /// Whether it can run: nothing refused or to choose, room for it, and a
    /// photo to import.
    pub fn can_run(&self) -> bool {
        self.refused.is_none()
            && self.ambiguous.is_none()
            && self.bytes + super::plan::SPARE <= self.available
            && self.photos.iter().any(|p| p.destination.is_some())
    }
    /// What execution compares with the plan the user confirmed.
    fn key(&self) -> PlanKey<'_> {
        (
            self.number,
            self.photos
                .iter()
                .map(|p| {
                    (
                        p.source.as_path(),
                        p.destination.as_deref(),
                        &p.companions[..],
                    )
                })
                .collect(),
            &self.folders,
        )
    }
}

/// A plan as the user confirmed it: its Import #, each photo's source and
/// where it and its companions go, and the folders it makes.
type PlanKey<'a> = (
    u32,
    Vec<(&'a Path, Option<&'a Path>, &'a [PathBuf])>,
    &'a [PathBuf],
);

/// What a run came to. Once a run is journalled it comes to `Ran` or
/// `Unread`, never an error: an error is a run that began nothing.
#[derive(Debug)]
pub enum ImportOutcome {
    /// The plan made under the lock differs from the one confirmed (another
    /// import took its Import #, a name was taken meanwhile): nothing was
    /// done, and this plan is to be confirmed instead.
    Changed(Box<ImportPlan>),
    Ran {
        batch: BatchId,
        /// The photos added, in order.
        imported: Vec<PhotoId>,
        /// The photos not added, with why.
        skipped: Vec<(PathBuf, String)>,
        outcome: Outcome,
    },
    /// The run was journalled and began, but what it did couldn't be read
    /// back (the catalog failed): some photos may have been added, which
    /// the catalog shows once it can be read; recovery decides the rest.
    Unread { batch: BatchId, error: String },
}

/// Plans `request` in `catalog`, for the dialog; nothing changes.
pub fn plan_import(catalog: &Catalog, request: &ImportRequest) -> Result<ImportPlan, FileOpError> {
    let number = next_number(&catalog.db)?;
    Ok(plan_on(catalog, &Real, request, number)?.view)
}

/// Runs `request` in `catalog`, as `confirmed` (from [`plan_import`])
/// showed it, stopping after the file being copied once `cancel` is set.
/// `trash` is what recovery of other operations it decides first needs.
pub fn execute_import(
    catalog: &mut Catalog,
    request: &ImportRequest,
    confirmed: &ImportPlan,
    trash: &dyn Trash,
    cancel: Cancel<'_>,
) -> Result<ImportOutcome, FileOpError> {
    execute_on(catalog, request, confirmed, trash, cancel, &Real)
}

/// The Import # the next run takes.
fn next_number(db: &impl Reads) -> Result<u32> {
    let last: Option<i64> = db.read_optional(
        sql!("SELECT value FROM catalog_counters WHERE name='import_number'"),
        &[],
    )?;
    Ok(u32::try_from(last.unwrap_or(0) + 1).unwrap_or(u32::MAX))
}

/// Where the destination is in the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Base {
    /// A folder of a root, by logical path.
    Folder(RootId, String),
    /// The destination becomes a root.
    NewRoot,
}

/// The names a destination folder will have as a plan fills it: what is on
/// disk, the catalog's photos there (offline ones too) and the plan's own.
struct Projected {
    names: Vec<OsString>,
    keys: HashSet<String>,
}

impl Projected {
    fn new(names: Vec<OsString>) -> Self {
        let keys = names.iter().map(|n| caseless(n)).collect();
        Self { names, keys }
    }
    /// Whether a photo `name` and its `companions` can arrive: no name of
    /// theirs taken (but the companions of its family, which are there
    /// already for it, read by the family's photo files `family`) or given
    /// twice, nothing there read as the photo's companion, and no file there
    /// but its family reading one of its companions, as a move checks
    /// (`Listing::arrival_conflict`).
    fn free(&self, name: &OsString, companions: &[OsString], family: &[OsString]) -> bool {
        let shared = !family.is_empty();
        let bundle: Vec<String> = std::iter::once(name)
            .chain(companions)
            .map(|n| caseless(n))
            .collect();
        let unique: HashSet<&String> = bundle.iter().collect();
        let taken = |n: &OsString| self.keys.contains(&caseless(n));
        if unique.len() != bundle.len()
            || taken(name)
            || companions.iter().any(|c| taken(c) != shared)
        {
            return false;
        }
        let mut names = self.names.clone();
        names.push(name.clone());
        if !shared {
            names.extend(companions.iter().cloned());
        }
        let theirs: HashSet<String> = family.iter().map(|n| caseless(n)).collect();
        let left_out = |i: usize| names.get(i).is_some_and(|n| theirs.contains(&caseless(n)));
        super::companions::Listing::new(names.clone())
            .arrival_conflict(name, companions, &left_out)
            .is_none()
    }
    fn add<'a>(&mut self, names: impl IntoIterator<Item = &'a OsString>) {
        for n in names {
            if self.keys.insert(caseless(n)) {
                self.names.push(n.clone());
            }
        }
    }
}

/// A photo item as planned.
#[derive(Clone, Debug)]
struct PlannedItem {
    /// The folder below the destination it goes into, by name.
    folders: Vec<String>,
    dir: PathBuf,
    /// The destination folder's file object, when it is there already.
    dir_id: Option<FileId>,
    /// Its files, companions first; with where each goes, the photo files
    /// last.
    members: Vec<(ScannedFile, PathBuf)>,
    /// Its photos: the index of each in `members`, and its source.
    photos: Vec<(usize, usize)>,
    /// The proofs of its source's volume.
    originals: Proof,
}

/// A plan, with what execution needs of it.
struct Planned {
    view: ImportPlan,
    base: Base,
    destination: PathBuf,
    copies: Proof,
    items: Vec<PlannedItem>,
}

/// The suspected duplicate of each of `sources` (Lightroom's rule: the
/// original name, the EXIF capture time and the size), against the photos
/// the catalog has, whether or not their files are online, and the earlier
/// ones of `sources`. A photo whose capture time isn't known on either side
/// is matched by name and size alone.
/// The earlier of two alike in `sources` is the one planning takes first
/// (by capture time, then name), so the photo flagged is the one an import
/// of both leaves out; the result is in `sources`' order.
pub fn suspected_duplicates(
    catalog: &Catalog,
    sources: &[ImportSource],
) -> Result<Vec<Option<Duplicate>>> {
    let known = catalog.db.snapshot(|s| known_signatures(s))?;
    let order = plan_order(sources);
    let ordered: Vec<ImportSource> = order.iter().map(|&i| sources[i].clone()).collect();
    let mut flags = vec![None; sources.len()];
    for (flag, &i) in duplicates(&known, &ordered).into_iter().zip(&order) {
        flags[i] = flag;
    }
    Ok(flags)
}

/// The order an import names `sources` in, as indices: by date (those
/// without one last), then name, as Lightroom numbers an import.
fn plan_order(sources: &[ImportSource]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..sources.len()).collect();
    order.sort_by(|&a, &b| {
        let (a, b) = (&sources[a], &sources[b]);
        (a.date().is_none(), a.date(), a.name()).cmp(&(b.date().is_none(), b.date(), b.name()))
    });
    order
}

/// A photo's duplicate signature: its original name (as `caseless` folds
/// it), its EXIF capture time when known, its size when known.
type Signature = (String, Option<String>, Option<u64>);

fn known_signatures(db: &impl Reads) -> Result<HashMap<String, Vec<Signature>>> {
    super::super::value::row! {
        struct Known {
            name: String,
            captured: Option<String>,
            size: Option<i64>,
        }
    }
    // Photos added before facts were kept: their current name and the size
    // their checksum read.
    let rows: Vec<Known> = db.read(
        sql!(
            "SELECT f.original_name AS name, f.exif_captured AS captured,
                    COALESCE(f.size, c.size) AS size
             FROM photo_import_facts f LEFT JOIN photo_checksums c ON c.photo = f.photo
             UNION ALL
             SELECT p.filename AS name, NULL AS captured, c.size AS size
             FROM photos p LEFT JOIN photo_checksums c ON c.photo = p.id
             WHERE p.master_id IS NULL
               AND NOT EXISTS (SELECT 1 FROM photo_import_facts f WHERE f.photo = p.id)"
        ),
        &[],
    )?;
    let mut known: HashMap<String, Vec<Signature>> = HashMap::new();
    for k in rows {
        let name = caseless(OsStr::new(&k.name));
        let size = k.size.and_then(|s| u64::try_from(s).ok());
        known
            .entry(name.clone())
            .or_default()
            .push((name, k.captured, size));
    }
    Ok(known)
}

fn duplicates(
    known: &HashMap<String, Vec<Signature>>,
    sources: &[ImportSource],
) -> Vec<Option<Duplicate>> {
    let mut seen: HashSet<Signature> = HashSet::new();
    sources
        .iter()
        .map(|s| {
            let name = caseless(s.name());
            let size = s.file.identity.size;
            let signature = (name.clone(), s.captured.clone(), Some(size));
            let found = known.get(&name).and_then(|rows| {
                let sized = rows.iter().filter(|r| r.2 == Some(size));
                let mut by_name = false;
                for r in sized {
                    match (&r.1, &s.captured) {
                        (Some(a), Some(b)) if same_time(a, b) => return Some(Duplicate::InCatalog),
                        (Some(_), Some(_)) => {}
                        _ => by_name = true,
                    }
                }
                by_name.then_some(Duplicate::NameAndSize)
            });
            let again = !seen.insert(signature);
            found.or(again.then_some(Duplicate::InImport))
        })
        .collect()
}

/// Whether two capture times are the same to the second, whatever
/// separates date and time.
fn same_time(a: &str, b: &str) -> bool {
    let key = |t: &str| t.get(..19).map(|t| t.replace('T', " "));
    key(a).is_some() && key(a) == key(b)
}

/// The name of a photo given `stem`, its extension spelt as `case` says.
fn renamed(source: &Path, stem: &str, case: ExtensionCase) -> OsString {
    match source.extension() {
        Some(ext) => format!("{stem}.{}", case.apply(&ext.to_string_lossy())).into(),
        None => stem.into(),
    }
}

/// Why `keywords` (paths, top first) can't be given to a photo: a path or a
/// name in it empty, as the catalog's keyword lookup refuses them.
fn keyword_problem(keywords: &[Vec<String>]) -> Option<String> {
    keywords
        .iter()
        .any(|path| {
            path.is_empty()
                || path
                    .iter()
                    .any(|n| crate::metadata::keyword_name(n).is_empty())
        })
        .then(|| "A keyword to apply has no name.".to_string())
}

/// The folders By original folders puts the photo at `file` in: the folder
/// chosen as the source, `source`, and those below it down to the photo's,
/// as Lightroom recreates them; `None` when it isn't below `source`.
fn original_folders(source: &Path, file: &Path) -> Option<Vec<String>> {
    let below = parent(file).strip_prefix(source).ok()?;
    let top = source.file_name()?.to_str()?.to_string();
    let rest: Option<Vec<String>> = below
        .components()
        .map(|c| c.as_os_str().to_str().map(str::to_string))
        .collect();
    Some(std::iter::once(top).chain(rest?).collect())
}

/// [`plan_import`] on `disk`, with Import # `number`.
fn plan_on(
    catalog: &Catalog,
    disk: &dyn Disk,
    request: &ImportRequest,
    number: u32,
) -> Result<Planned, FileOpError> {
    let mut view = ImportPlan {
        number,
        ..Default::default()
    };
    let refused = |mut view: ImportPlan, why: String| {
        view.refused = Some(why);
        view.photos = request
            .sources
            .iter()
            .map(|s| PlannedPhoto {
                source: s.file.path.clone(),
                destination: None,
                companions: Vec::new(),
                duplicate: None,
                skipped: None,
            })
            .collect();
        Planned {
            view,
            base: Base::NewRoot,
            destination: PathBuf::new(),
            copies: Proof::Ids,
            items: Vec::new(),
        }
    };
    // Keywords the commit couldn't give would fail it on every try, and a
    // failing commit blocks recovery for every file operation after it.
    if let Some(why) = keyword_problem(&request.apply.keywords) {
        return Ok(refused(view, why));
    }
    // The destination: a folder there, on a drive whose copies can be
    // published only where nothing has their names and then proven.
    let destination = match disk.canonical(&request.destination) {
        Ok(d) if disk.entry(&d).is_ok_and(|e| e.kind == EntryKind::Directory) => d,
        _ => {
            return Ok(refused(
                view,
                format!(
                    "{} isn't a folder that is there.",
                    request.destination.display()
                ),
            ));
        }
    };
    let facts = disk
        .volume_facts(&destination)
        .map_err(|e| FileOpError::Destination(e.to_string()))?;
    view.available = facts.available;
    if facts.read_only {
        return Ok(refused(
            view,
            "The destination's drive is read-only.".into(),
        ));
    }
    let copies = match proof::of(&facts) {
        Some(copies) if facts.exclusive_rename => copies,
        _ => {
            return Ok(refused(
                view,
                "Photos can't be imported onto this drive yet: it can't give a copy its name only where no file has it (exFAT on a Mac, network drives). Choose a folder on another drive.".into(),
            ));
        }
    };
    let matcher = Matcher::new(catalog)?;
    let base = match matcher.place(&destination, &destination, &request.choices) {
        Matched::Place(Place::Folder(root, logical)) => Base::Folder(root, logical),
        Matched::Place(Place::New(_)) if matcher.holds_below(&destination) => {
            return Ok(refused(
                view,
                "A folder of the catalog is inside the destination. Choose that folder, or one of the catalog's folders above it.".into(),
            ));
        }
        Matched::Place(Place::New(_)) => Base::NewRoot,
        Matched::Conflict(c) => return Ok(refused(view, c.message())),
        Matched::Ambiguous(options) => {
            view.ambiguous = Some(super::super::ingest::Ambiguity {
                directories: vec![destination.clone()],
                options,
            });
            return Ok(refused(
                view,
                "Choose which folder of the catalog the destination is.".into(),
            ));
        }
    };
    let computer = &catalog.computer.id;
    let located = catalog.db.snapshot(|s| locate_folders(s, computer))?;
    let folders = Folders::index(&located);
    let order = plan_order(&request.sources);
    let known = catalog.db.snapshot(|s| known_signatures(s))?;
    let ordered: Vec<ImportSource> = order.iter().map(|&i| request.sources[i].clone()).collect();
    // Without Don't Import Suspected Duplicates none is looked for.
    let flagged = match request.skip_duplicates {
        true => duplicates(&known, &ordered),
        false => vec![None; ordered.len()],
    };
    let left_out: Vec<bool> = flagged.iter().map(Option::is_some).collect();
    let total = left_out.iter().filter(|l| !**l).count();
    // Names taken in each folder: on disk, by the catalog's photos there
    // (offline ones too), and by this run, as `caseless` folds them.
    let mut taken: HashMap<PathBuf, Projected> = HashMap::new();
    let mut made: Vec<PathBuf> = Vec::new();
    let mut items: Vec<PlannedItem> = Vec::new();
    let mut proofs_of: HashMap<PathBuf, Proof> = HashMap::new();
    // Extensions follow Rename Files alone.
    let case = match request.naming {
        Some(_) => request.extensions,
        None => ExtensionCase::LeaveAsIs,
    };
    let mut index = 0usize;
    for (n, source) in ordered.iter().enumerate() {
        let mut photo = PlannedPhoto {
            source: source.file.path.clone(),
            destination: None,
            companions: Vec::new(),
            duplicate: flagged[n],
            skipped: None,
        };
        if left_out[n] {
            photo.skipped = Some("Suspected duplicate".into());
            view.photos.push(photo);
            continue;
        }
        let organized = match &request.organize {
            Organize::IntoOneFolder => Some(Vec::new()),
            Organize::ByDate(format) => source.date().and_then(|d| format.folders(d)),
            Organize::ByOriginalFolders => original_folders(&request.source, &source.file.path),
        };
        let Some(organized) = organized else {
            photo.skipped = Some(match request.organize {
                Organize::ByDate(_) => "It has no date to file it under.".into(),
                // A drive's root has no name to recreate (what Lightroom
                // does there wasn't measured).
                _ if request.source.file_name().is_none() => {
                    "By original folders needs a folder as the source, not a whole drive.".into()
                }
                _ => "It isn't in the folder imported from.".into(),
            });
            view.photos.push(photo);
            continue;
        };
        let subfolder = request.subfolder.trim();
        let names: Vec<String> = (!subfolder.is_empty())
            .then(|| subfolder.to_string())
            .into_iter()
            .chain(organized)
            .collect();
        if let Some(why) = names.iter().find_map(|n| super::folder_name_problem(n)) {
            photo.skipped = Some(why.into());
            view.photos.push(photo);
            continue;
        }
        let dir = names.iter().fold(destination.clone(), |d, n| d.join(n));
        // Its folders: each there already, or made by the run.
        let mut problem = None;
        let mut at = destination.clone();
        for name in &names {
            at = at.join(name);
            if made.contains(&at) {
                continue;
            }
            match disk.entry(&at) {
                Ok(e) if e.kind == EntryKind::Directory => {
                    // One the catalog may not have yet: adopted, if the
                    // catalog would place it where this run does.
                    let expected = match &base {
                        Base::Folder(root, logical) => {
                            let rest = at.strip_prefix(&destination).unwrap_or(Path::new(""));
                            Some((
                                *root,
                                join(logical, &super::super::locations::logical_from_os(rest)),
                            ))
                        }
                        Base::NewRoot => None,
                    };
                    let placed = matcher.place(&at, &destination, &request.choices);
                    let fits = match (&placed, &expected) {
                        (Matched::Place(Place::Folder(r, l)), Some((root, logical))) => {
                            r == root && l == logical
                        }
                        (Matched::Place(Place::New(_)), None) => !matcher.holds_below(&at),
                        _ => false,
                    };
                    if !fits {
                        problem = Some(format!("The catalog places {} elsewhere.", at.display()));
                        break;
                    }
                }
                Err(FsError::NotFound) => {
                    // A folder the catalog has there, though missing on disk,
                    // is never made over: New Folder Inside refuses it too.
                    if let Base::Folder(root, logical) = &base {
                        let rest = at.strip_prefix(&destination).unwrap_or(Path::new(""));
                        let logical =
                            join(logical, &super::super::locations::logical_from_os(rest));
                        if !super::folder::in_catalog(&catalog.db, computer, *root, &logical, &at)?
                            .is_empty()
                        {
                            problem = Some(format!(
                                "The catalog has a folder named {name} there, though it is missing."
                            ));
                            break;
                        }
                    }
                    made.push(at.clone());
                }
                Ok(_) => {
                    problem = Some(format!("A file named {name} is where its folder goes."));
                    break;
                }
                Err(e) => {
                    problem = Some(e.to_string());
                    break;
                }
            }
        }
        if let Some(why) = problem {
            photo.skipped = Some(why);
            view.photos.push(photo);
            continue;
        }
        let here = match taken.entry(dir.clone()) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let mut names: Vec<OsString> = disk.list(&dir).unwrap_or_default();
                for id in folders.over(&dir) {
                    let filenames: Vec<String> = catalog
                        .db
                        .read(sql!("SELECT filename FROM photos WHERE folder=?"), &[&id])?;
                    names.extend(filenames.into_iter().map(OsString::from));
                }
                e.insert(Projected::new(names))
            }
        };
        // Its new stem, from the template or its own.
        let own = source
            .file
            .path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        let stem = match &request.naming {
            None => own,
            Some(template) => {
                let fields = NameFields {
                    captured: source.date().map(str::to_string),
                    original_name: Some(source.name().to_string_lossy().into_owned()),
                    import_number: Some(number),
                    shoot_name: Some(request.shoot_name.clone()),
                    ..source.fields.clone()
                };
                let at = NameContext {
                    index,
                    total,
                    fields: &fields,
                };
                template.stem(&source.file.path, &at).0
            }
        };
        index += 1;
        // A family: an earlier photo of this folder under the same stem
        // whose companions are this photo's, the same files as scanned.
        let family = items.iter().position(|item| {
            item.dir == dir
                && !source.companions.is_empty()
                && item
                    .photos
                    .iter()
                    .any(|&(m, _)| item.members[m].1.file_stem() == Some(OsStr::new(&stem)))
                && source.companions.iter().all(|c| {
                    item.members.iter().any(|(f, _)| {
                        f.path == c.path && f.identity == c.identity && f.file == c.file
                    })
                })
        });
        let originals = *proofs_of
            .entry(parent(&source.file.path).to_path_buf())
            .or_insert_with(|| {
                disk.volume_facts(parent(&source.file.path))
                    .ok()
                    .as_ref()
                    .and_then(proof::of)
                    .unwrap_or(Proof::Names)
            });
        if let Some(f) = family {
            let name = renamed(&source.file.path, &stem, case);
            // It reads the family's companions, and nothing else there.
            let shared: Vec<OsString> = items[f]
                .members
                .iter()
                .filter(|(m, _)| source.companions.iter().any(|c| c.path == m.path))
                .filter_map(|(_, at)| at.file_name().map(OsStr::to_os_string))
                .collect();
            let family: Vec<OsString> = items[f]
                .photos
                .iter()
                .filter_map(|&(m, _)| items[f].members[m].1.file_name().map(OsStr::to_os_string))
                .collect();
            if here.free(&name, &shared, &family) {
                here.add([&name]);
                let at = dir.join(&name);
                let item = &mut items[f];
                item.members.push((source.file.clone(), at.clone()));
                item.photos.push((item.members.len() - 1, order[n]));
                photo.destination = Some(at);
                view.bytes += source.file.identity.size;
                view.photos.push(photo);
                continue;
            }
        }
        // Its name and its companions', the first stem no file there has:
        // `-2`, `-3`… for the whole bundle; nothing is replaced.
        let mut chosen = None;
        for attempt in 1..10_000u32 {
            let stem = match attempt {
                1 => stem.clone(),
                n => format!("{stem}-{n}"),
            };
            let name = renamed(&source.file.path, &stem, case);
            let companions: Option<Vec<OsString>> = source
                .companions
                .iter()
                .map(|c| {
                    super::companions::renamed(
                        source.name(),
                        &name,
                        c.path.file_name().unwrap_or_default(),
                    )
                })
                .collect();
            let Some(companions) = companions else { break };
            // A name too long, with its extension, stays too long.
            if std::iter::once(&name).chain(&companions).any(|n| {
                n.to_str()
                    .is_none_or(|n| super::names::problem(n).is_some())
            }) {
                break;
            }
            if here.free(&name, &companions, &[]) {
                chosen = Some((name, companions));
                break;
            }
        }
        let Some((name, companions)) = chosen else {
            photo.skipped = Some("It can't be given a name there.".into());
            view.photos.push(photo);
            continue;
        };
        here.add(std::iter::once(&name).chain(&companions));
        let mut members: Vec<(ScannedFile, PathBuf)> = source
            .companions
            .iter()
            .zip(&companions)
            .map(|(c, n)| (c.clone(), dir.join(n)))
            .collect();
        photo.companions = members.iter().map(|(_, at)| at.clone()).collect();
        members.push((source.file.clone(), dir.join(&name)));
        photo.destination = Some(dir.join(&name));
        view.bytes += source.bytes();
        let photo_member = members.len() - 1;
        let dir_id = match made.contains(&dir) {
            true => None,
            false => disk.entry(&dir).ok().map(|e| e.id),
        };
        items.push(PlannedItem {
            folders: names,
            dir_id,
            dir,
            members,
            photos: vec![(photo_member, order[n])],
            originals,
        });
        view.photos.push(photo);
    }
    view.folders = made;
    Ok(Planned {
        view,
        base,
        destination,
        copies,
        items,
    })
}

/// One photo of an import item, as journalled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct ImportedPhoto {
    /// The member that is its file.
    pub member: i64,
    /// Its file's name on the card.
    pub original_name: String,
    /// The capture time its EXIF gives, as scanned.
    pub exif_captured: Option<String>,
    pub size: u64,
}

/// An import item's journalled state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(in crate::catalog) struct ImportState {
    /// Its run's Import #.
    pub number: u32,
    /// What it does.
    pub role: Role,
    /// How its sources (`originals`) and copies are proven.
    pub proofs: Proofs,
    /// What the photos get, as Preferences said when the run started.
    pub metadata: MetadataDefaults,
    pub options: ImportOptions,
    /// What the run applies over what the files hold, as it was asked for.
    #[serde(default)]
    pub apply: ImportMetadata,
    /// The photos it added, once it did.
    #[serde(default)]
    pub committed: Vec<PhotoId>,
}

// Its Preferences are plain JSON values, which equal themselves.
impl Eq for ImportState {}

/// What an import item does.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(in crate::catalog) enum Role {
    /// Copies its members into `dir`, the folder at logical path `logical`
    /// of `root`, then adds its photos.
    Photos {
        root: RootId,
        logical: String,
        dir: PathBuf,
        /// The folder's file object, where the destination's volume keeps
        /// them: from the plan when it was there, else once it is made.
        #[serde(default)]
        dir_id: Option<String>,
        photos: Vec<ImportedPhoto>,
    },
    /// Ends the run, once its photo items are decided: burst stacking
    /// queued for its import batch, and a root it added removed if it was
    /// left with no folder.
    Finish {
        /// The import batch, once its first photo is added.
        batch: Option<i64>,
        /// The root the run added for its destination.
        root: Option<RootId>,
    },
}

/// The paths, as `caseless` folds them, that this computer's unresolved
/// imports publish their copies at: Add Folder and Synchronize leave them
/// out, as they are being imported. Read in their insertion transaction.
pub(in crate::catalog) fn reserved(db: &impl Reads, computer: &str) -> Result<HashSet<String>> {
    let mut paths = HashSet::new();
    for item in journal::unresolved(db, computer)? {
        if item.kind != Kind::ImportCopy {
            continue;
        }
        for m in &item.members {
            if let Some(d) = &m.destination {
                paths.insert(caseless(d.as_os_str()));
            }
        }
    }
    Ok(paths)
}

/// [`execute_import`] on `disk`.
pub(super) fn execute_on(
    catalog: &mut Catalog,
    request: &ImportRequest,
    confirmed: &ImportPlan,
    trash: &dyn Trash,
    cancel: Cancel<'_>,
    disk: &dyn Disk,
) -> Result<ImportOutcome, FileOpError> {
    let _lock = super::lock::acquire(catalog)?;
    super::recover::prune(catalog)?;
    super::recover::run(catalog, disk, trash)?;
    let number = next_number(&catalog.db)?;
    let planned = plan_on(catalog, disk, request, number)?;
    if planned.view.key() != confirmed.key() || !planned.view.can_run() {
        return Ok(ImportOutcome::Changed(Box::new(planned.view)));
    }
    let batch = super::plan::new_batch()?;
    let computer = catalog.computer.id.clone();
    let since = super::plan::now();
    let (metadata, options) = request.defaults.snapshot();
    let state = |role: Role, originals: Proof| ImportState {
        number,
        role,
        proofs: Proofs {
            originals,
            copies: planned.copies,
            direct: false,
            since,
        },
        metadata: metadata.clone(),
        options: options.clone(),
        apply: request.apply.clone(),
        committed: Vec::new(),
    };
    // Journalled in one transaction with the Import # it reserves and the
    // root it adds, before any disk step: a number in a published name is
    // never handed out again.
    let journalled = catalog.db.write_immediate(|w| {
        let reserved = next_number(w)?;
        if reserved != number {
            return Ok(None);
        }
        w.execute(
            sql!(
                "INSERT INTO catalog_counters(name, value) VALUES('import_number', ?)
                 ON CONFLICT(name) DO UPDATE SET value = excluded.value"
            ),
            &[&i64::from(number)],
        )?;
        let (root, new_root) = match &planned.base {
            Base::Folder(root, logical) => ((*root, logical.clone()), None),
            Base::NewRoot => {
                let root: RootId = w.insert_returning_id(
                    sql!("INSERT INTO roots(original_path) VALUES(?) RETURNING id"),
                    &[&journal::text(&planned.destination)?],
                )?;
                ((root, String::new()), Some(root))
            }
        };
        let mut ids = Vec::new();
        for (n, item) in planned.items.iter().enumerate() {
            let logical = item.folders.iter().fold(root.1.clone(), |l, f| join(&l, f));
            let photos = item
                .photos
                .iter()
                .map(|&(m, source)| {
                    let s = &request.sources[source];
                    ImportedPhoto {
                        member: m as i64,
                        original_name: s.name().to_string_lossy().into_owned(),
                        exif_captured: s.captured.clone(),
                        size: s.file.identity.size,
                    }
                })
                .collect();
            let role = Role::Photos {
                root: root.0,
                logical,
                dir: item.dir.clone(),
                dir_id: item.dir_id.map(|id| id.to_string()),
                photos,
            };
            let members: Vec<NewMember> = item
                .members
                .iter()
                .map(|(f, to)| NewMember {
                    source: f.path.clone(),
                    destination: Some(to.clone()),
                    id: f.file,
                    identity: f.identity.clone(),
                })
                .collect();
            let temps: Vec<PathBuf> = item
                .members
                .iter()
                .enumerate()
                .map(|(ordinal, (_, to))| {
                    parent(to).join(super::plan::temp_name(
                        to.file_name().unwrap_or_default(),
                        &batch,
                        n,
                        ordinal,
                    ))
                })
                .collect();
            ids.push(journal::insert_import(
                w,
                (&batch.0, &computer),
                &state(role, item.originals),
                &members,
                &temps,
            )?);
        }
        let finish = journal::insert_import(
            w,
            (&batch.0, &computer),
            &state(
                Role::Finish {
                    batch: None,
                    root: new_root,
                },
                Proof::Ids,
            ),
            &[],
            &[],
        )?;
        Ok(Some((ids, finish, root)))
    })?;
    let Some((ids, finish, (root, base_logical))) = journalled else {
        return Ok(ImportOutcome::Changed(Box::new(planned.view)));
    };
    // Journalled, it is decided by recovery should this read fail.
    let folders = match catalog.db.snapshot(|s| locate_folders(s, &computer)) {
        Ok(located) => Folders::index(&located),
        Err(e) => {
            return Ok(ImportOutcome::Unread {
                batch,
                error: format!("{e:#}"),
            });
        }
    };
    let cx = copy::Cx {
        computer: &computer,
        folders: &folders,
        disk,
        trash,
    };
    let mut stopped = None;
    let mut items = ids.iter().zip(&planned.items);
    // From here every error stops the run, never the call: what is
    // journalled is the outcome, read back below, photos added included.
    for (id, item) in items.by_ref() {
        if cancel.is_cancelled() {
            if let Err(e) = set(catalog, *id, ItemState::NoChange, "Stopped.") {
                stopped = Some(format!("{e:#}"));
            }
            break;
        }
        let run = (|| -> Result<()> {
            let base = (root, base_logical.as_str());
            if let Some(why) = make_folders(
                catalog,
                (&batch.0, &computer),
                disk,
                base,
                &planned.destination,
                item,
                cancel,
            )? {
                return set(catalog, *id, ItemState::NoChange, &why);
            }
            run_item(&cx, catalog, *id, &request.defaults, false, cancel)
        })();
        if let Err(e) = run {
            stopped = Some(format!("{e:#}"));
            break;
        }
    }
    // Those not begun: nothing of them is on disk. Left `planned` when the
    // catalog can't be written, recovery carries them on.
    for (id, _) in items {
        if let Err(e) = set(catalog, *id, ItemState::NoChange, "Stopped.") {
            stopped.get_or_insert(format!("{e:#}"));
            break;
        }
    }
    if stopped.is_none()
        && let Err(e) = finish_run(catalog, finish)
    {
        stopped = Some(format!("{e:#}"));
    }
    let unread = |error: anyhow::Error, batch: BatchId| ImportOutcome::Unread {
        batch,
        error: format!("{error:#}"),
    };
    let outcome = match super::outcome(catalog, &batch) {
        Ok(read) => Outcome { stopped, ..read },
        Err(e) => return Ok(unread(e, batch)),
    };
    let items = match journal::of_batch(&catalog.db, &batch.0) {
        Ok(items) => items,
        Err(e) => return Ok(unread(e, batch)),
    };
    let mut imported = Vec::new();
    let mut skipped: Vec<(PathBuf, String)> = planned
        .view
        .photos
        .iter()
        .filter_map(|p| Some((p.source.clone(), p.skipped.clone()?)))
        .collect();
    for item in items {
        let Subject::Import(state) = &item.subject else {
            continue;
        };
        let Role::Photos { photos, .. } = &state.role else {
            continue;
        };
        imported.extend(&state.committed);
        if item.state != ItemState::Done {
            let why = item
                .error
                .clone()
                .unwrap_or_else(|| "It wasn't finished.".into());
            for p in photos {
                if let Some(m) = item.members.get(p.member as usize) {
                    skipped.push((m.source.clone(), why.clone()));
                }
            }
        }
    }
    Ok(ImportOutcome::Ran {
        batch,
        imported,
        skipped,
        outcome,
    })
}

/// Sets item `id`'s state, with why.
fn set(catalog: &mut Catalog, id: OperationId, state: ItemState, why: &str) -> Result<()> {
    catalog
        .db
        .write(|w| journal::set_state(w, id, state, Some(why)))
}

/// Makes the folders below `destination` that `item` goes into and the disk
/// doesn't have, each as New Folder Inside makes one, in run `batch`. Why
/// not, when one can't be made.
fn make_folders(
    catalog: &mut Catalog,
    batch: (&str, &str),
    disk: &dyn Disk,
    (root, base): (RootId, &str),
    destination: &Path,
    item: &PlannedItem,
    cancel: Cancel<'_>,
) -> Result<Option<String>> {
    if logical_location(&catalog.db, batch.1, root, base)?.as_deref() != Some(destination) {
        return Ok(Some(super::preconditions::RELOCATED.into()));
    }
    let (mut at, mut logical) = (destination.to_path_buf(), base.to_string());
    for name in &item.folders {
        let next = at.join(name);
        if matches!(disk.entry(&next), Err(FsError::NotFound)) {
            let parent_path = logical_location(&catalog.db, batch.1, root, &logical)?
                .context("the destination isn't located here")?;
            let new = NewFolder {
                state: FolderState {
                    root,
                    parent: logical.clone(),
                    parent_path,
                    name: name.clone(),
                    folder: None,
                },
                path: next.clone(),
                rules: super::conflicts::Names(crate::fsops::probe(
                    &at,
                    &disk.list(&at).unwrap_or_default(),
                )),
            };
            if super::folder::create(catalog, batch, disk, &new, cancel)?.is_none() {
                // `create` makes nothing once Stop is asked for.
                return Ok(Some(match cancel.is_cancelled() {
                    true => "Stopped.".into(),
                    false => format!("The folder {name} couldn't be made."),
                }));
            }
        }
        logical = join(&logical, name);
        at = next;
    }
    Ok(None)
}

/// Copies journalled photo item `id`'s members not yet copied, then adds
/// its photos. A member that fails leaves the item: `no_change` when
/// nothing of it was written whole, else `needs_recovery`, its copies kept.
fn run_item(
    cx: &copy::Cx<'_>,
    catalog: &mut Catalog,
    id: OperationId,
    defaults: &NewPhotoDefaults,
    recovering: bool,
    cancel: Cancel<'_>,
) -> Result<()> {
    let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    let Subject::Import(state) = &item.subject else {
        anyhow::bail!("not an import");
    };
    if let Some(why) = destination_moved(catalog, cx, &item)? {
        return leave(cx, catalog, &item, &why);
    }
    let item = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    // The source as scanned, before anything is copied. What a crash left
    // begun is checked as it is carried on (`copy_member`).
    if !item.members.iter().all(|m| {
        m.state != MemberState::Planned || source_proven(cx.disk, m, state.proofs.originals)
    }) {
        return leave(
            cx,
            catalog,
            &item,
            "It changed since it was scanned, or the card is gone.",
        );
    }
    for m in &item.members {
        if m.state == MemberState::Copied {
            continue;
        }
        if let Err(why) = copy_member(cx, catalog, &item, m, state.proofs, cancel) {
            let now = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
            return leave(cx, catalog, &now, &why);
        }
    }
    let now = journal::item(&catalog.db, id)?.context("the journalled item is gone")?;
    commit(cx, catalog, &now, defaults, recovering)
}

/// Whether the source of member `m` is the file scanned, as `proof` says.
fn source_proven(disk: &dyn Disk, m: &journal::Member, proof: Proof) -> bool {
    copy::original_proven(disk, m, proof)
}

/// Why item's destination folder isn't where it was planned: it is located
/// elsewhere now, or isn't a folder there.
fn destination_moved(
    catalog: &mut Catalog,
    cx: &copy::Cx<'_>,
    item: &Item,
) -> Result<Option<String>> {
    let Subject::Import(state) = &item.subject else {
        return Ok(Some("not an import of photos".into()));
    };
    let Role::Photos {
        root,
        logical,
        dir,
        dir_id,
        ..
    } = &state.role
    else {
        return Ok(Some("not an import of photos".into()));
    };
    let now = logical_location(&catalog.db, cx.computer, *root, logical)?;
    if now.as_deref() != Some(dir.as_path()) {
        return Ok(Some(super::preconditions::RELOCATED.into()));
    }
    if !cx
        .disk
        .entry(dir)
        .is_ok_and(|e| e.kind == EntryKind::Directory)
    {
        return Ok(Some(format!("{} isn't there.", dir.display())));
    }
    // The folder planned, not another put at its path since: by its file
    // object where the volume keeps them (on FAT and exFAT, by its path).
    if state.proofs.copies == Proof::Ids {
        let journalled: Option<FileId> = match dir_id {
            Some(id) => Some(id.parse()?),
            // Made by the run: the folder its own item made.
            None => made_by(&catalog.db, &item.batch, dir)?,
        };
        let Some(id) = journalled else {
            return Ok(Some("Its folder wasn't made.".into()));
        };
        if presence(cx.disk, dir, id) != Presence::Here {
            return Ok(Some(format!(
                "{} isn't the folder planned any more.",
                dir.display()
            )));
        }
        if dir_id.is_none() {
            let mut state = state.clone();
            if let Role::Photos { dir_id, .. } = &mut state.role {
                *dir_id = Some(id.to_string());
            }
            catalog
                .db
                .write(|w| journal::set_import(w, item.id, &state))?;
        }
    }
    Ok(None)
}

/// The file object of folder `dir` as run `batch` made it, if it did.
fn made_by(db: &impl Reads, batch: &str, dir: &Path) -> Result<Option<FileId>> {
    let key = caseless(dir.as_os_str());
    Ok(journal::of_batch(db, batch)?
        .iter()
        .filter(|i| i.kind == Kind::CreateFolder)
        .flat_map(|i| i.members.first())
        .find(|m| {
            m.destination
                .as_deref()
                .is_some_and(|d| caseless(d.as_os_str()) == key)
        })
        .and_then(|m| m.id))
}

/// Copies member `m` of `item` (as [`copy::write`] copies a file to another
/// drive), then gives it its name by an exclusive rename and journals it
/// `copied`. Why not, as text.
fn copy_member(
    cx: &copy::Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    m: &journal::Member,
    proofs: Proofs,
    cancel: Cancel<'_>,
) -> Result<(), String> {
    let (Some(to), Some(record), Some(file), Some(identity)) =
        (m.destination.as_deref(), &m.copy, m.id, &m.identity)
    else {
        return Err("It has no destination.".into());
    };
    let failed = |f: copy::Failed| match f {
        copy::Failed::Problem(p) => p.to_string(),
        copy::Failed::Journal(e) => format!("{e:#}"),
    };
    // Begun before (a crash, recovery): what of it is there. Written whole
    // and gone from its partial's name, it may have been given its own
    // before the journal said so.
    // Not journalled yet, an empty file at its partial's name made since the
    // run was planned is its own, as the copy rules take it.
    let whole = match copy::partial(cx.disk, m, &record.temp, proofs) {
        Partial::Gone if record.content.is_some() => {
            match copy::complete_at(cx.disk, to, record, proofs) {
                Some(Some(at))
                    if record
                        .content
                        .is_some_and(|c| cx.disk.hash(&at).is_ok_and(|h| h == c)) =>
                {
                    return catalog
                        .db
                        .write(|w| journal::set_copied(w, item.id, m.ordinal, &at))
                        .map_err(|e| format!("{e:#}"));
                }
                Some(None) => None,
                _ => return Err("A file is where its copy was named.".into()),
            }
        }
        Partial::Gone => None,
        Partial::Ours(at) if record.content.is_some() => Some(at),
        Partial::Ours(at) => {
            cx.disk
                .remove_file(&at)
                .and_then(|()| cx.disk.sync_dir(parent(&at)))
                .map_err(|e| e.to_string())?;
            None
        }
        Partial::Unknown => return Err("A file is where its copy was being made.".into()),
    };
    let (temp, made) = match whole {
        // Written whole before a crash: the rest of its steps (synced, its
        // source's extras and time), which a crash may have cut short, done
        // again while its source is still the file copied, then read back
        // before it is named.
        Some(at) => {
            let content = record.content.expect("written whole");
            if !source_proven(cx.disk, m, proofs.originals) {
                return Err("It changed since it was scanned, or the card is gone.".into());
            }
            finish_partial(cx.disk, &m.source, &at).map_err(|p| p.to_string())?;
            if !cx.disk.hash(&at).is_ok_and(|h| h == content) {
                return Err(format!(
                    "The copy of {} isn't as it was written.",
                    m.source.display()
                ));
            }
            let made = cx.disk.entry(&at).map_err(|e| e.to_string())?.id;
            (at, made)
        }
        None => copy::write(
            cx,
            catalog,
            (item.id, m.ordinal),
            (&m.source, file, identity),
            &record.temp,
            proofs,
            cancel,
        )
        .map_err(failed)?,
    };
    cx.disk.rename(&temp, to).map_err(|e| match e {
        FsError::AlreadyExists => format!(
            "A file named {} is there now.",
            to.file_name().unwrap_or_default().to_string_lossy()
        ),
        e => e.to_string(),
    })?;
    cx.disk.sync_dir(parent(to)).map_err(|e| e.to_string())?;
    let at = match proofs.copies {
        Proof::Ids => spelled(cx.disk, to, made),
        Proof::Names => proof::spelled(cx.disk, to),
    }
    .map_err(|e| e.to_string())?;
    catalog
        .db
        .write(|w| journal::set_copied(w, item.id, m.ordinal, &at))
        .map_err(|e| format!("{e:#}"))
}

/// The steps of a copy after it is written whole ([`copy::write`]'s), for
/// partial copy `at` of `source`: synced, given its source's extras, time
/// and permissions, each through a handle opened on it.
fn finish_partial(disk: &dyn Disk, source: &Path, at: &Path) -> Result<(), copy::Problem> {
    let disk_problem = |e: FsError| copy::Problem::Disk(e.to_string());
    // Read-only (given its source's permissions, or made so by a umask), it
    // is made writable by its owner again, being this run's own: whether
    // its steps were done can't be told from it, so they are done again,
    // and the last gives it its source's permissions back.
    let open = || std::fs::OpenOptions::new().write(true).open(at);
    let into = match open() {
        Ok(into) => into,
        Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
            owner_writable(at).map_err(|e| copy::Problem::Disk(e.to_string()))?;
            open().map_err(|e| copy::Problem::Disk(e.to_string()))?
        }
        Err(e) => return Err(copy::Problem::Disk(e.to_string())),
    };
    disk.sync_file(at, &into).map_err(disk_problem)?;
    let extras = disk.extras(source).map_err(disk_problem)?;
    copy::carry_extras(disk, source, (at, Some(&into)), extras)?;
    disk.copy_times(source, at, Some(&into))
        .map_err(disk_problem)
}

/// Lets the owner of file `at` write it: its owner's write bit on Unix, its
/// read-only attribute cleared on Windows.
fn owner_writable(at: &Path) -> std::io::Result<()> {
    let mut permissions = std::fs::metadata(at)?.permissions();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        permissions.set_mode(permissions.mode() | 0o200);
    }
    #[cfg(not(unix))]
    #[allow(clippy::permissions_set_readonly_false, reason = "Windows' attribute")]
    permissions.set_readonly(false);
    std::fs::set_permissions(at, permissions)
}

/// Leaves `item` uncommitted, for `why`: its partial copies never written
/// whole removed as copies' rules allow; `no_change` when nothing of it is
/// left written whole or named, else `needs_recovery`, kept as it is.
fn leave(cx: &copy::Cx<'_>, catalog: &mut Catalog, item: &Item, why: &str) -> Result<()> {
    let proofs = match &item.subject {
        Subject::Import(s) => s.proofs,
        _ => Proofs::default(),
    };
    let mut kept = false;
    for m in &item.members {
        let Some(record) = &m.copy else { continue };
        match m.state {
            MemberState::Copied => kept = true,
            _ => match copy::partial(cx.disk, m, &record.temp, proofs) {
                // Written whole and gone from its partial's name: named
                // already, perhaps, or that can't be told.
                Partial::Gone if record.content.is_some() => {
                    let named = m
                        .destination
                        .as_deref()
                        .map(|to| copy::complete_at(cx.disk, to, record, proofs));
                    if !matches!(named, Some(Some(None))) {
                        kept = true;
                    }
                }
                Partial::Gone => {}
                Partial::Ours(_) if record.content.is_some() => kept = true,
                Partial::Ours(at) => {
                    let removed = cx
                        .disk
                        .remove_file(&at)
                        .and_then(|()| cx.disk.sync_dir(parent(&at)));
                    if removed.is_err() {
                        kept = true;
                    }
                }
                Partial::Unknown => kept = true,
            },
        }
    }
    let state = match kept {
        true => ItemState::NeedsRecovery,
        false => ItemState::NoChange,
    };
    set(catalog, item.id, state, why)
}

/// Adds journalled `item`'s photos, every member copied: in one transaction,
/// the destination where it was planned, each copy proven at its name, no
/// photo of the catalog there by that name; then each photo as Add Folder
/// adds one (with the run's defaults), its import facts, in the run's
/// import batch (made with the first photo), and the item `done`. A photo
/// of the catalog there already leaves it `needs_recovery`, kept as it is.
fn commit(
    cx: &copy::Cx<'_>,
    catalog: &mut Catalog,
    item: &Item,
    defaults: &NewPhotoDefaults,
    recovering: bool,
) -> Result<()> {
    let Subject::Import(state) = &item.subject else {
        anyhow::bail!("not an import");
    };
    let Role::Photos {
        root,
        logical,
        dir,
        photos,
        ..
    } = &state.role
    else {
        anyhow::bail!("not an import of photos");
    };
    if let Some(why) = destination_moved(catalog, cx, item)? {
        return leave(cx, catalog, item, &why);
    }
    for m in &item.members {
        let (Some(at), Some(record)) = (m.destination.as_deref(), &m.copy) else {
            return leave(cx, catalog, item, "It has no destination.");
        };
        // Its bytes read back again where a crash came between: what was
        // copied may have been changed since. A run's own were just read.
        let here = match state.proofs.copies {
            Proof::Ids => {
                record
                    .id
                    .is_some_and(|id| presence(cx.disk, at, id) == Presence::Here)
                    && (!recovering
                        || record
                            .content
                            .is_some_and(|c| cx.disk.hash(at).is_ok_and(|h| h == c)))
            }
            Proof::Names => record.content.is_some_and(|c| proof::holds(cx.disk, at, c)),
        };
        if m.state != MemberState::Copied || !here {
            return leave(
                cx,
                catalog,
                item,
                "Its copies aren't all where they were made.",
            );
        }
    }
    // No file there now that a photo would read as its companion but those
    // it brought: one put there since it was planned is never read as its.
    let foreign = foreign_companion(cx.disk, item, dir).map_err(|e| e.to_string());
    match foreign {
        Ok(None) => {}
        Ok(Some(name)) => {
            return leave(
                cx,
                catalog,
                item,
                &format!(
                    "{} there would be read as its sidecar; its files were left as they are.",
                    name.to_string_lossy()
                ),
            );
        }
        Err(e) => {
            return leave(
                cx,
                catalog,
                item,
                &format!("Its folder can't be listed: {e}"),
            );
        }
    }
    // What each photo's files hold, read before the transaction as Add
    // Folder reads them, with what it was on the card.
    let files: Vec<(PathBuf, &ImportedPhoto)> = photos
        .iter()
        .filter_map(|p| Some((item.members.get(p.member as usize)?.destination.clone()?, p)))
        .collect();
    let defaults = defaults.with_snapshot(state.metadata.clone(), state.options.clone());
    let mut read = catalog.read_new_photos(files, &defaults);
    for photo in &mut read {
        photo.facts = ImportFacts {
            original_name: photo.place.original_name.clone(),
            size: Some(photo.place.size),
            exif_captured: Some(photo.place.exif_captured.clone()),
        };
    }
    let computer = cx.computer.to_string();
    let batch_of = item.batch.clone();
    let id = item.id;
    let (root, logical, dir) = (*root, logical.clone(), dir.clone());
    let state = state.clone();
    let left = catalog.write_new_photos(|w, writer| {
        // Checked again inside, as Synchronize may have added a file since.
        if logical_location(w, &computer, root, &logical)?.as_deref() != Some(dir.as_path()) {
            return Ok(Some(super::preconditions::RELOCATED.to_string()));
        }
        let arrival = super::preconditions::Arrival {
            root,
            logical: logical.clone(),
            resolved: dir.clone(),
            added: false,
        };
        let folder = match super::folder::arrive(w, &computer, &arrival, &dir)? {
            Ok((folder, _)) => folder,
            Err(why) => return Ok(Some(why)),
        };
        for photo in &read {
            if super::super::ingest::known(w, root, &logical, &photo.file)? {
                return Ok(Some(format!(
                    "A photo of the catalog is named {} there already; its file was left as it is.",
                    photo.file.file_name().unwrap_or_default().to_string_lossy()
                )));
            }
        }
        let batch = import_batch(w, &batch_of)?;
        let mut committed = Vec::new();
        for photo in read {
            let (photo, _) = writer.insert(w, photo, folder, batch)?;
            let apply = &state.apply;
            super::super::descriptive::apply_import(
                w,
                photo,
                &apply.creator,
                &apply.copyright,
                &apply.keywords,
            )?;
            committed.push(photo);
        }
        let state = ImportState { committed, ..state };
        journal::set_import(w, id, &state)?;
        journal::set_done(w, id, None)?;
        Ok(None)
    })?;
    match left {
        None => Ok(()),
        Some(why) => set(catalog, id, ItemState::NeedsRecovery, &why),
    }
}

/// The name of a file in `dir` that one of `item`'s photos would read as
/// its companion, though not one the item brought, or that would read one
/// of them; `None` when there is none. A folder that can't be listed is an
/// error: what is there can't be told.
pub(super) fn foreign_companion(
    disk: &dyn Disk,
    item: &Item,
    dir: &Path,
) -> Result<Option<OsString>, FsError> {
    let Subject::Import(ImportState {
        role: Role::Photos { photos, .. },
        ..
    }) = &item.subject
    else {
        return Ok(None);
    };
    let names = disk.list(dir)?;
    let photo_members: HashSet<i64> = photos.iter().map(|p| p.member).collect();
    let name_of = |m: &journal::Member| {
        m.destination
            .as_deref()
            .and_then(Path::file_name)
            .map(OsStr::to_os_string)
    };
    let carried: Vec<OsString> = item
        .members
        .iter()
        .filter(|m| !photo_members.contains(&m.ordinal))
        .filter_map(name_of)
        .collect();
    let family: HashSet<String> = item
        .members
        .iter()
        .filter(|m| photo_members.contains(&m.ordinal))
        .filter_map(name_of)
        .map(|n| caseless(&n))
        .collect();
    let listing = super::companions::Listing::new(names.clone());
    let left_out = |i: usize| names.get(i).is_some_and(|n| family.contains(&caseless(n)));
    Ok(item
        .members
        .iter()
        .filter(|m| photo_members.contains(&m.ordinal))
        .filter_map(name_of)
        .find_map(|photo| listing.arrival_conflict(&photo, &carried, &left_out)))
}

/// The import batch of run `batch`, as its finish item has it, made with
/// the first photo added, in `w`'s transaction.
fn import_batch(w: &mut Write<'_>, batch: &str) -> Result<i64> {
    let finish = journal::of_batch(w, batch)?
        .into_iter()
        .find(|i| {
            matches!(
                &i.subject,
                Subject::Import(ImportState {
                    role: Role::Finish { .. },
                    ..
                })
            )
        })
        .context("the import's finishing item is gone")?;
    let Subject::Import(mut state) = finish.subject else {
        unreachable!("found as an import");
    };
    let Role::Finish { batch: made, root } = &state.role else {
        unreachable!("found as a finish");
    };
    if let Some(made) = made {
        return Ok(*made);
    }
    let made = super::super::ingest::new_batch(w, ImportKind::Copy)?;
    state.role = Role::Finish {
        batch: Some(made),
        root: *root,
    };
    journal::set_import(w, finish.id, &state)?;
    Ok(made)
}

/// Ends a run, its photo items decided: in one transaction, burst stacking
/// queued for its import batch (with Stack Camera Bursts on), the root it
/// added removed when it was left with no folder, and the item `done`.
/// Its state is read in that transaction: the run's photo items record the
/// import batch in it as they make one.
fn finish_run(catalog: &mut Catalog, id: OperationId) -> Result<()> {
    catalog.db.write_immediate(|w| {
        let item = journal::item(w, id)?.context("the journalled item is gone")?;
        let Subject::Import(ImportState {
            role: Role::Finish { batch, root },
            options,
            ..
        }) = &item.subject
        else {
            anyhow::bail!("not an import's finish");
        };
        if let Some(batch) = batch
            && options.stack_bursts
        {
            w.execute(
                sql!(
                    "INSERT INTO import_burst_stacks(batch, seconds) VALUES(?, ?)
                     ON CONFLICT(batch) DO NOTHING"
                ),
                &[batch, &super::super::ingest::IMPORT_BURST_SECONDS],
            )?;
        }
        if let Some(root) = root {
            let left: Option<super::super::FolderId> =
                w.read_optional(sql!("SELECT id FROM folders WHERE root=?"), &[root])?;
            if left.is_none() {
                w.execute(sql!("DELETE FROM folder_locations WHERE root=?"), &[root])?;
                w.execute(sql!("DELETE FROM roots WHERE id=?"), &[root])?;
            }
        }
        journal::set_done(w, id, None)
    })
}

/// Recovery of import item `item`, the lock held: a photo item left
/// `planned` is carried on (its source still the file scanned) and its
/// photos added, or left; the finish item runs once no photo item of its
/// run is still `planned`. One left `needs_recovery` stays so: it is
/// never carried on.
pub(super) fn decide(cx: &copy::Cx<'_>, catalog: &mut Catalog, item: &Item) -> Result<()> {
    if item.state != ItemState::Planned {
        return Ok(());
    }
    let Subject::Import(state) = &item.subject else {
        anyhow::bail!("not an import");
    };
    match &state.role {
        Role::Finish { .. } => {
            let waiting = journal::of_batch(&catalog.db, &item.batch)?
                .iter()
                .any(|i| {
                    i.id != item.id && i.kind == Kind::ImportCopy && i.state == ItemState::Planned
                });
            match waiting {
                true => Ok(()),
                false => finish_run(catalog, item.id),
            }
        }
        Role::Photos { .. } => {
            let defaults = NewPhotoDefaults::installed()
                .with_snapshot(state.metadata.clone(), state.options.clone());
            run_item(cx, catalog, item.id, &defaults, true, Cancel::NEVER)
        }
    }
}

#[cfg(test)]
pub(super) fn plan_with(
    catalog: &Catalog,
    disk: &dyn Disk,
    request: &ImportRequest,
) -> Result<ImportPlan, FileOpError> {
    let number = next_number(&catalog.db)?;
    Ok(plan_on(catalog, disk, request, number)?.view)
}

#[cfg(test)]
pub(super) fn reserved_paths(catalog: &Catalog) -> Result<HashSet<String>> {
    reserved(&catalog.db, &catalog.computer.id)
}
