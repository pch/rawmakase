//! Disposable Library previews, separate from user catalogs and edit recipes.
//!
//! Two kinds of rows share the file: the Library's 640 px thumbnails
//! (`previews`), and larger previews built on request for selected photos
//! (`sized_previews`, issue #213), each kind with its own budget. Which catalog
//! photos had previews built is kept apart, in `preview_intent`, because pixel
//! rows are keyed by file and shared by virtual copies and catalogs.
use super::{CatalogLocation, PhotoId};
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
const APP_ID: i64 = 0x4f4d5052;
const VERSION: i64 = 1;
// 2: previews keep their aspect ratio (generation 1 forced 360×240).
// 3: the one engine renders every edit differently from earlier releases.
// 4: highlight recovery keeps a lone clipped channel's recorded value, which saved
//    edits of RAWs with clipped light sources render differently.
// 5: a DNG's OpcodeList3 GainMaps apply (drones' lens shading), which saved edits of
//    those DNGs render differently.
// 6: Highlights -100, and the positions between -50 and -100, keep pulling areas more
//    than 7 stops over the photo's key, which saved edits of photos with such areas
//    render differently.
// 7: built-in lens corrections apply as Lightroom applies them (Olympus, OM System
//    and Panasonic distortion and CA, Nikon Z, Sony's lateral CA), and correction
//    the camera always applies ignores the profile amounts while profile corrections
//    are off, which saved edits of those RAWs render differently.
// 8: Coolpix NRW lens tables apply.
// 9: Samsung's built-in distortion and lateral CA apply, which saved edits of SRWs
//    render differently.
// 10: the EOS R5 Mark II's and R6 Mark III's built-in lens correction, which saved
//     edits with Enable Profile Corrections on render with.
// 11: lens corrections are centred on their optical centre and framed by green's
//     samples for the photo's edge (Nikon Z, Olympus, OM System, Samsung, DNG
//     opcodes, Adobe profiles).
// 12: Fujifilm's distortion table is scaled as Adobe's warps of it are, so its photos
//     frame as in Lightroom.
// 13: a half-size decode sharpens and grains at its photo's scale, which edited
//     thumbnails rendered from it show differently.
// 14: a half-size decode reduces luminance and color noise at its photo's scales.
// 15: a DNG's OpcodeList3 FixVignetteRadial applies (iPhones' and DJI drones'
//     vignetting), which saved edits of those DNGs render differently.
// 16: AI Denoise keeps its own color below Amount 100 and corrects dark areas over
//     larger blocks, which saved edits with AI Denoise render differently.
// 17: the Canon EOS Rebel T8i is the EOS 850D, whose color matrix and baseline
//     exposure its photos had none of before.
const GENERATION: i64 = 17;
// 3: a JPEG, PNG or TIFF is converted from its embedded ICC profile.
// 4: the one engine, as `GENERATION` 3.
// 5: an edited JPEG, PNG or TIFF renders from the scene its rendering is of.
// 6: Highlights, as `GENERATION` 6.
const RASTER_GENERATION: i64 = 6;
/// The oldest generation whose RAW thumbnails without a tag, the file's
/// embedded preview (the camera's JPEG, or the JPEG XL Lightroom writes into a
/// DNG), are still made as this build makes them. Such a preview is not an
/// engine render, so [`GENERATION`]'s later bumps leave it alone. 5 is the
/// first generation every build of which extracts Lightroom's JPEG XL
/// previews, which replaced its 256 px JPEG (#525, merged while 4 was
/// current). When extracting or scaling embedded previews changes, bump
/// [`GENERATION`] and set this to the new value.
const EMBEDDED_GENERATION_MIN: i64 = 5;
/// The most edits of one file whose thumbnails are kept, each with every
/// variant it was rendered in ([`PreviewCache::store_tagged`]).
const EDIT_FAMILIES: usize = 8;
/// Thumbnails a prune deletes in one write.
const PRUNE_STEP: usize = 256;
/// The most pages, of 4 KiB, one step of giving space back to the disk gives
/// back in one write ([`PreviewCache::release_step`]).
const RELEASE_STEP: u32 = 256;
/// How long one such step may write for, well within the 250 ms another
/// connection waits for a write. Free pages at the end of the file are given
/// back in microseconds each, but a page in use there has to be moved first,
/// which took about 1 ms a page next to a 4 GiB free list (measured). Rows
/// another connection writes meanwhile land in the free space, so such pages
/// keep turning up: 256 of them made a step of 645 ms, and some writes beside
/// it failed.
const RELEASE_STEP_TIME: Duration = Duration::from_millis(25);
/// The shortest pause after each such write, which is otherwise as long as
/// the write took. SQLite wakes a writer waiting for the lock after 1, 2, 5
/// ... 50, 100 ms; with only a short pause between such writes, some found
/// it taken each time and gave up after their 250 ms (measured).
const RELEASE_PAUSE: Duration = Duration::from_millis(5);
/// The thumbnails' budget when none is set, in bytes.
pub const DEFAULT_THUMBNAIL_BUDGET: u64 = 512 * 1024 * 1024;
/// The thumbnails' budget every connection prunes to, in bytes
/// ([`set_thumbnail_budget`]).
static THUMBNAIL_BUDGET: AtomicU64 = AtomicU64::new(DEFAULT_THUMBNAIL_BUDGET);
/// Sets how many bytes of Library thumbnails the cache keeps, the least
/// recently used going first (Preferences > Performance > Thumbnail cache). It
/// holds for every connection of this process, from its next prune: when it
/// is opened, and every 32 thumbnails stored. Standard and 1:1 previews keep
/// their own budgets.
pub fn set_thumbnail_budget(bytes: u64) {
    THUMBNAIL_BUDGET.store(bytes, Ordering::Relaxed);
}
/// The budget [`set_thumbnail_budget`] set.
pub fn thumbnail_budget() -> u64 {
    THUMBNAIL_BUDGET.load(Ordering::Relaxed)
}
fn budget() -> i64 {
    i64::try_from(thumbnail_budget()).unwrap_or(i64::MAX)
}
// 2: the one engine, as `GENERATION` 3.
// 3: highlight recovery, as `GENERATION` 4.
// 4: OpcodeList3 GainMaps, as `GENERATION` 5.
// 5: Highlights, as `GENERATION` 6.
// 6: built-in lens corrections, as `GENERATION` 7.
// 7: Coolpix NRW lens tables, as `GENERATION` 8.
// 8: Samsung's built-in lens corrections, as `GENERATION` 9.
// 9: Canon's built-in lens correction, as `GENERATION` 10.
// 10: optical centres and framing, as `GENERATION` 11.
// 11: Fujifilm's distortion framing, as `GENERATION` 12.
// 12: a half-size decode's sharpening and grain, as `GENERATION` 13 (Standard
//     previews built from it).
// 13: a half-size decode's noise reduction, as `GENERATION` 14.
// 14: OpcodeList3 FixVignetteRadial, as `GENERATION` 15.
// 15: the EOS Rebel T8i as the EOS 850D, as `GENERATION` 17.
const SIZED_GENERATION: i64 = 15;
const SIZED_QUALITY: u8 = 85;
/// Tables added after version 1, created on open so older builds, which ignore
/// them, keep opening the file.
const SIZED_SCHEMA: &str = "
    CREATE TABLE IF NOT EXISTS sized_previews(source_path TEXT NOT NULL,identity TEXT NOT NULL,kind TEXT NOT NULL,source_size INTEGER NOT NULL,modified_ns TEXT NOT NULL,generation INTEGER NOT NULL,requested_edge INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,jpeg BLOB NOT NULL,last_used INTEGER NOT NULL,PRIMARY KEY(source_path,identity,kind));
    CREATE INDEX IF NOT EXISTS sized_previews_last_used ON sized_previews(kind,last_used);
    CREATE TABLE IF NOT EXISTS preview_intent(catalog TEXT NOT NULL,photo_id INTEGER NOT NULL,source_path TEXT NOT NULL,kind TEXT NOT NULL,PRIMARY KEY(catalog,photo_id,kind));";
/// A preview built on request, larger than a Library thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PreviewKind {
    /// Fits the Standard Preview Size; Develop shows it while a photo opens.
    Standard,
    /// The photo at full resolution.
    OneToOne,
}
impl PreviewKind {
    pub const ALL: [Self; 2] = [Self::Standard, Self::OneToOne];
    fn code(self) -> &'static str {
        match self {
            Self::Standard => "standard",
            Self::OneToOne => "one_to_one",
        }
    }
    /// Payload bytes this kind may keep before its least recently used rows go.
    pub fn budget(self) -> i64 {
        match self {
            Self::Standard => 2 << 30,
            Self::OneToOne => 4 << 30,
        }
    }
}
/// Image bytes held by each kind of row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PreviewUsage {
    pub thumbnails: u64,
    pub standard: u64,
    pub one_to_one: u64,
}
pub use crate::storage::Stamp;
/// What became of a preview offered to [`PreviewCache::store_tagged`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreOutcome {
    Stored,
    /// The file differs from the [`Stamp`] it was rendered from, is gone or
    /// can't be reached: nothing was stored, and nothing is wrong with the
    /// cache.
    Stale,
}
pub struct PreviewCache {
    db: Connection,
    writes: u32,
    /// Built previews stored, counted apart so thumbnails never delay their pruning.
    sized_writes: u32,
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}
fn catalog_key(catalog: &CatalogLocation) -> String {
    match catalog {
        CatalogLocation::File(path) => format!("file:{}", key(path)),
    }
}
fn tagged(path: &Path, tag: &str) -> String {
    if tag.is_empty() {
        key(path)
    } else {
        format!("{}#{tag}", key(path))
    }
}
/// The generation a thumbnail of `path` is made in now.
fn generation(path: &Path) -> i64 {
    if crate::storage::is_raw(path) {
        GENERATION
    } else {
        RASTER_GENERATION
    }
}
/// What a `previews` row is a thumbnail of, as its key and the family stored
/// with it tell: the file, and the edit it was rendered with, if any.
#[derive(Debug, PartialEq, Eq)]
struct Row<'a> {
    /// The file's key ([`key`]).
    path: &'a str,
    /// The edit family ([`PreviewCache::store_tagged`]): `None` for a file's
    /// own thumbnail, or a row whose edit can't be told.
    family: Option<&'a str>,
    /// Rendered with an edit, rather than the file's own thumbnail.
    tagged: bool,
}
/// Reads a row's key and its `prefix_hash` column, which holds the family of
/// an edited thumbnail. Earlier builds left that column empty: their edited
/// rows are told by a key ending in `#edit-` and 16 hex digits, as
/// `EditSource::tag` makes them, then one of the variants any build has
/// written ([`legacy_variant`]). Any other key is a file's own thumbnail,
/// such as that of a file named `x.ARW#edit-…-copy.ARW`; one with a tag of
/// another form, which no build has stored outside tests, is taken for one
/// too.
fn row_kind<'a>(key: &'a str, family: &'a str) -> Row<'a> {
    if !family.is_empty()
        && let Some(at) = key.rfind(&format!("#{family}"))
    {
        return Row {
            path: &key[..at],
            family: Some(&key[at + 1..at + 1 + family.len()]),
            tagged: true,
        };
    }
    const EDIT: &str = "#edit-";
    if let Some(at) = key.rfind(EDIT) {
        let rest = &key[at + EDIT.len()..];
        if let Some((hex, variant)) = rest.split_at_checked(16)
            && hex.bytes().all(|b| b.is_ascii_hexdigit())
            && legacy_variant(variant)
        {
            return Row {
                path: &key[..at],
                family: Some(&key[at + 1..at + EDIT.len() + 16]),
                tagged: true,
            };
        }
    }
    Row {
        path: key,
        family: None,
        tagged: false,
    }
}
/// Whether `variant` follows an edit family in a tag some build wrote: none
/// (Develop's renders before the Library told how they were rendered),
/// `-fit`, `-fit-noai`, or `-fit-ai-` and the AI Denoise model
/// (`ai_denoise::model_tag`: hex digits, `-a` and its version). A file's name
/// has an extension, so a `.` or a path separator never belongs to one.
fn legacy_variant(variant: &str) -> bool {
    matches!(variant, "" | "-fit" | "-fit-noai")
        || variant.strip_prefix("-fit-ai-").is_some_and(|model| {
            !model.is_empty()
                && model
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}
/// Whether a row made in `made` can still be shown. An edited thumbnail, and
/// any of a JPEG, PNG or TIFF, is the engine's render, made again whenever the
/// engine renders differently; a RAW's own thumbnail is its embedded preview
/// (see [`EMBEDDED_GENERATION_MIN`]).
fn fresh(row: &Row<'_>, made: i64) -> bool {
    let path = Path::new(row.path);
    if !row.tagged && crate::storage::is_raw(path) {
        made >= EMBEDDED_GENERATION_MIN
    } else {
        made == generation(path)
    }
}
/// Drops the edited thumbnails of the file keyed `path` beyond those of
/// `family`, just stored, and the [`EDIT_FAMILIES`]` - 1` other edits used
/// most recently (ties by family), in the caller's write transaction.
fn keep_edit_families(db: &Connection, path: &str, family: &str) -> Result<()> {
    // The file's tagged keys sort between `path#` and `path$`, '$' following
    // '#' in UTF-8: a range of the primary key, which a LIKE on a path with
    // '%' or '_' would not be. Other files whose names begin `path#` fall in
    // it too, and are told apart by `row_kind`.
    let rows: Vec<(String, String, i64)> = db
        .prepare(
            "SELECT source_path,prefix_hash,last_used FROM previews WHERE source_path>=?1 AND source_path<?2",
        )?
        .query_map(params![format!("{path}#"), format!("{path}$")], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;
    let mut used: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    let mut members: Vec<(&str, &str)> = Vec::new();
    for (key, stored, last_used) in &rows {
        let row = row_kind(key, stored);
        let Some(other) = row.family.filter(|_| row.path == path) else {
            continue;
        };
        members.push((key, other));
        if other != family {
            let latest = used.entry(other).or_insert(*last_used);
            *latest = (*latest).max(*last_used);
        }
    }
    if used.len() < EDIT_FAMILIES {
        return Ok(());
    }
    let mut ranked: Vec<(&str, i64)> = used.into_iter().collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
    let dropped: std::collections::HashSet<&str> = ranked[EDIT_FAMILIES - 1..]
        .iter()
        .map(|(family, _)| *family)
        .collect();
    let mut delete = db.prepare("DELETE FROM previews WHERE source_path=?1")?;
    for (key, family) in members {
        if dropped.contains(family) {
            delete.execute([key])?;
        }
    }
    Ok(())
}
/// A row a prune read, which it deletes only if no connection has made it
/// again since.
#[derive(Debug)]
struct Doomed {
    key: String,
    generation: i64,
    last_used: i64,
}
/// Whether the file at `path` is no longer the one a row was made from. An
/// offline original is not stale, so its previews stay usable.
fn stale(path: &Path, size: i64, modified: &str) -> bool {
    Stamp::read(path)
        .is_ok_and(|stamp| stamp.size as i64 != size || stamp.modified_ns.to_string() != modified)
}
impl PreviewCache {
    pub fn path() -> PathBuf {
        crate::storage::data_dir().join("previews.sqlite3")
    }
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Connection::open(path)?;
        db.busy_timeout(Duration::from_millis(250))?;
        let app: i64 = db.query_row("PRAGMA application_id", [], |r| r.get(0))?;
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if app == 0 && version == 0 {
            let tables: i64 = db.query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table'",
                [],
                |r| r.get(0),
            )?;
            ensure!(
                tables == 0,
                "Preview cache path contains a different database"
            );
            db.execute_batch(&format!("PRAGMA auto_vacuum=INCREMENTAL; PRAGMA application_id={APP_ID}; PRAGMA user_version={VERSION};
                CREATE TABLE previews(source_path TEXT PRIMARY KEY,source_size INTEGER NOT NULL,modified_ns TEXT NOT NULL,prefix_hash TEXT NOT NULL,generation INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,jpeg BLOB NOT NULL,last_used INTEGER NOT NULL);
                CREATE INDEX previews_last_used ON previews(last_used);"))?;
        } else {
            ensure!(
                app == APP_ID && version == VERSION,
                "Unsupported preview cache format"
            );
        }
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")?;
        db.execute_batch(SIZED_SCHEMA)?;
        let mut cache = Self {
            db,
            writes: 0,
            sized_writes: 0,
        };
        cache.prune(budget())?;
        for kind in PreviewKind::ALL {
            cache.prune_sized(kind, kind.budget())?;
        }
        cache.tidy();
        Ok(cache)
    }
    pub fn load(&self, path: &Path) -> Result<Option<image::RgbImage>> {
        self.load_tagged(path, "")
    }
    /// A preview variant, e.g. rendered with an edit identified by `tag`.
    pub fn load_tagged(&self, path: &Path, tag: &str) -> Result<Option<image::RgbImage>> {
        let key = tagged(path, tag);
        let row = self
            .db
            .query_row(
                "SELECT source_size,modified_ns,generation,jpeg FROM previews WHERE source_path=?",
                [&key],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, Vec<u8>>(3)?,
                    ))
                },
            )
            .optional()?;
        let Some((size, modified, generation, jpeg)) = row else {
            return Ok(None);
        };
        // An offline original keeps its preview, as in Lightroom.
        let stale = stale(path, size, &modified);
        // Maintenance is best effort, as another connection may hold the database, and
        // never changes the answer: a failed delete still misses, a failed touch hits.
        let forget = || {
            let _ = self
                .db
                .execute("DELETE FROM previews WHERE source_path=?", [&key]);
        };
        let row = Row {
            path: &self::key(path),
            family: None,
            tagged: !tag.is_empty(),
        };
        if !fresh(&row, generation) || stale {
            forget();
            return Ok(None);
        }
        let image = match image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
            Ok(im) if im.width() <= 1024 && im.height() <= 1024 => im.to_rgb8(),
            _ => {
                forget();
                return Ok(None);
            }
        };
        let _ = self.db.execute(
            "UPDATE previews SET last_used=? WHERE source_path=?",
            params![now(), &key],
        );
        Ok(Some(image))
    }
    /// Keeps `image` as the file's own thumbnail: a RAW's embedded preview, or
    /// a JPEG, PNG or TIFF as it is.
    pub fn store(&mut self, path: &Path, stamp: &Stamp, image: &image::RgbImage) -> Result<()> {
        ensure!(
            image.width() <= 1024 && image.height() <= 1024,
            "Preview exceeds cache size limit"
        );
        ensure!(
            Stamp::read(path)? == *stamp,
            "Source changed while generating preview"
        );
        self.put(path, "", "", stamp, image)
    }
    /// Keeps `image` as the file's thumbnail rendered with an edit: under
    /// `tag`, a variant of the edit `family` (`EditSource::tag`, of which
    /// `tag` is an extension, such as with or without AI Denoise's result).
    ///
    /// A file keeps the thumbnails of [`EDIT_FAMILIES`] edits, every variant
    /// of each: this one and those used most recently. It is a cap rather
    /// than an LRU: a file with more live edits than that, as virtual copies
    /// and catalogs give it, has the edit used longest ago made again when it
    /// is shown, and while the file is offline it can't be.
    ///
    /// A file changed, removed or unreachable since `stamp` was read is
    /// [`StoreOutcome::Stale`], not an error: nothing is stored, and nothing
    /// is wrong with the cache. The file is compared in the same call that
    /// writes the row, so callers need not check it first. A preview too
    /// large to keep, or a failure to encode or write it, is still an error.
    pub fn store_tagged(
        &mut self,
        path: &Path,
        family: &str,
        tag: &str,
        stamp: &Stamp,
        image: &image::RgbImage,
    ) -> Result<StoreOutcome> {
        debug_assert!(
            !family.is_empty() && tag.starts_with(family),
            "{tag} is not a variant of {family}"
        );
        ensure!(!family.is_empty(), "An edited preview has no edit family");
        ensure!(
            image.width() <= 1024 && image.height() <= 1024,
            "Preview exceeds cache size limit"
        );
        if Stamp::read(path).ok().as_ref() != Some(stamp) {
            return Ok(StoreOutcome::Stale);
        }
        self.put(path, family, tag, stamp, image)?;
        Ok(StoreOutcome::Stored)
    }
    /// Writes `image` under `tag` of `family`, for a file the caller has
    /// checked against `stamp`.
    fn put(
        &mut self,
        path: &Path,
        family: &str,
        tag: &str,
        stamp: &Stamp,
        image: &image::RgbImage,
    ) -> Result<()> {
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 88).encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        // prefix_hash held a hash of the file's first 64 KB, which no build
        // since 0.1.11 reads; it now holds an edited row's family, and is
        // empty for the file's own thumbnail.
        let tx = self
            .db
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute("INSERT INTO previews VALUES(?,?,?,?,?,?,?,?,?) ON CONFLICT(source_path) DO UPDATE SET source_size=excluded.source_size,modified_ns=excluded.modified_ns,prefix_hash=excluded.prefix_hash,generation=excluded.generation,width=excluded.width,height=excluded.height,jpeg=excluded.jpeg,last_used=excluded.last_used",params![tagged(path, tag),stamp.size as i64,stamp.modified_ns.to_string(),family,generation(path),image.width(),image.height(),jpeg,now()])?;
        if !family.is_empty() {
            keep_edit_families(&tx, &key(path), family)?;
        }
        tx.commit()?;
        self.writes += 1;
        if self.writes.is_multiple_of(32) {
            self.prune(budget())?;
            self.tidy();
        }
        Ok(())
    }
    /// A built preview of `path` for the edit `identity`, with the thumbnails'
    /// offline and staleness rules: an offline original still hits, without
    /// being opened; a rewritten one misses and its row goes.
    pub fn load_sized(
        &self,
        path: &Path,
        identity: &str,
        kind: PreviewKind,
    ) -> Result<Option<image::RgbImage>> {
        let source = key(path);
        let row = self
            .db
            .query_row(
                "SELECT source_size,modified_ns,generation,width,height,jpeg FROM sized_previews WHERE source_path=? AND identity=? AND kind=?",
                params![&source, identity, kind.code()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, u32>(3)?,
                        r.get::<_, u32>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                    ))
                },
            )
            .optional()?;
        let Some((size, modified, generation, width, height, jpeg)) = row else {
            return Ok(None);
        };
        // Best effort, as for thumbnails: a failed delete still misses.
        let forget = || {
            let _ = self.db.execute(
                "DELETE FROM sized_previews WHERE source_path=? AND identity=? AND kind=?",
                params![&source, identity, kind.code()],
            );
        };
        if generation != SIZED_GENERATION || stale(path, size, &modified) {
            forget();
            return Ok(None);
        }
        let image = match image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg) {
            Ok(im) if (im.width(), im.height()) == (width, height) => im.to_rgb8(),
            _ => {
                forget();
                return Ok(None);
            }
        };
        let _ = self.db.execute(
            "UPDATE sized_previews SET last_used=? WHERE source_path=? AND identity=? AND kind=?",
            params![now(), &source, identity, kind.code()],
        );
        Ok(Some(image))
    }
    /// Whether a usable preview exists without reading its pixels: the file is
    /// unchanged and the row was built for at least `requested_edge`. The size
    /// it reached is not compared, so a small original or a tight crop that
    /// cannot reach the edge is not rebuilt each time.
    pub fn sized_fresh(
        &self,
        path: &Path,
        identity: &str,
        kind: PreviewKind,
        requested_edge: u32,
    ) -> Result<bool> {
        let row = self
            .db
            .query_row(
                "SELECT source_size,modified_ns,generation,requested_edge FROM sized_previews WHERE source_path=? AND identity=? AND kind=?",
                params![key(path), identity, kind.code()],
                |r| {
                    Ok((
                        r.get::<_, i64>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, i64>(2)?,
                        r.get::<_, u32>(3)?,
                    ))
                },
            )
            .optional()?;
        Ok(row.is_some_and(|(size, modified, generation, edge)| {
            generation == SIZED_GENERATION
                && edge >= requested_edge
                && !stale(path, size, &modified)
        }))
    }
    /// Stores a preview built from the file as it was at `stamp`, refusing it
    /// when the file has changed since.
    pub fn store_sized(
        &mut self,
        path: &Path,
        identity: &str,
        kind: PreviewKind,
        stamp: &Stamp,
        requested_edge: u32,
        image: &image::RgbImage,
    ) -> Result<()> {
        ensure!(
            image.width() > 0 && image.height() > 0,
            "Preview has no pixels"
        );
        ensure!(
            Stamp::read(path)? == *stamp,
            "Source changed while building preview"
        );
        let mut jpeg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, SIZED_QUALITY).encode(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )?;
        self.db.execute(
            "INSERT OR REPLACE INTO sized_previews VALUES(?,?,?,?,?,?,?,?,?,?,?)",
            params![
                key(path),
                identity,
                kind.code(),
                stamp.size as i64,
                stamp.modified_ns.to_string(),
                SIZED_GENERATION,
                requested_edge,
                image.width(),
                image.height(),
                jpeg,
                now()
            ],
        )?;
        self.sized_writes += 1;
        if self.sized_writes.is_multiple_of(8) {
            for kind in PreviewKind::ALL {
                self.prune_sized(kind, kind.budget())?;
            }
            self.tidy();
        }
        Ok(())
    }
    /// Image bytes held by each kind.
    pub fn usage(&self) -> Result<PreviewUsage> {
        let thumbnails: i64 = self.db.query_row(
            "SELECT COALESCE(sum(length(jpeg)),0) FROM previews",
            [],
            |r| r.get(0),
        )?;
        Ok(PreviewUsage {
            thumbnails: thumbnails as u64,
            standard: self.sized_bytes(PreviewKind::Standard)? as u64,
            one_to_one: self.sized_bytes(PreviewKind::OneToOne)? as u64,
        })
    }
    /// Drops every built preview of `kind` for these files, whatever edit they
    /// were built for. The space they leave is given back by
    /// [`Self::release_step`].
    pub fn discard(&mut self, paths: &[PathBuf], kind: PreviewKind) -> Result<()> {
        let tx = self.db.transaction()?;
        for path in paths {
            tx.execute(
                "DELETE FROM sized_previews WHERE source_path=? AND kind=?",
                params![key(path), kind.code()],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
    /// Drops every built preview of `kind`, and every request to keep one.
    /// The space they leave is given back by [`Self::release_step`].
    pub fn clear(&mut self, kind: PreviewKind) -> Result<()> {
        let tx = self.db.transaction()?;
        tx.execute("DELETE FROM sized_previews WHERE kind=?", [kind.code()])?;
        tx.execute("DELETE FROM preview_intent WHERE kind=?", [kind.code()])?;
        tx.commit()?;
        Ok(())
    }
    /// Counts a preview as used now, as when it is asked for again.
    pub fn touch_sized(&self, path: &Path, identity: &str, kind: PreviewKind) -> Result<()> {
        self.db.execute(
            "UPDATE sized_previews SET last_used=? WHERE source_path=? AND identity=? AND kind=?",
            params![now(), key(path), identity, kind.code()],
        )?;
        Ok(())
    }
    /// Lightroom's Automatically Discard 1:1 Previews: drops the previews of
    /// `kind` not shown or built for `unused`, and the requests for files left
    /// without one, so an edit does not build them again. Returns how many went;
    /// the space they leave is given back by [`Self::release_step`].
    pub fn expire(&mut self, kind: PreviewKind, unused: Duration) -> Result<usize> {
        let before = now() - unused.as_secs() as i64;
        let tx = self.db.transaction()?;
        let gone = tx.execute(
            "DELETE FROM sized_previews WHERE kind=?1 AND last_used<?2",
            params![kind.code(), before],
        )?;
        tx.execute(
            "DELETE FROM preview_intent WHERE kind=?1 AND source_path NOT IN (SELECT source_path FROM sized_previews WHERE kind=?1)",
            [kind.code()],
        )?;
        tx.commit()?;
        Ok(gone)
    }
    /// Records that the user asked for `kind` previews of this catalog photo,
    /// so they are rebuilt when its edit changes.
    pub fn record_intent(
        &mut self,
        catalog: &CatalogLocation,
        photo: PhotoId,
        path: &Path,
        kind: PreviewKind,
    ) -> Result<()> {
        self.db.execute(
            "INSERT OR REPLACE INTO preview_intent VALUES(?,?,?,?)",
            params![catalog_key(catalog), photo.0, key(path), kind.code()],
        )?;
        Ok(())
    }
    /// Whether this catalog photo, at this path, has `kind` previews requested.
    /// A different path means the id now names another photo.
    pub fn has_intent(
        &self,
        catalog: &CatalogLocation,
        photo: PhotoId,
        path: &Path,
        kind: PreviewKind,
    ) -> Result<bool> {
        Ok(self
            .db
            .query_row(
                "SELECT 1 FROM preview_intent WHERE catalog=? AND photo_id=? AND source_path=? AND kind=?",
                params![catalog_key(catalog), photo.0, key(path), kind.code()],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
    /// Points `photo`'s requests at its file's new path, after the file was
    /// found moved, so its previews are built from there.
    pub fn move_intent(
        &mut self,
        catalog: &CatalogLocation,
        photo: PhotoId,
        path: &Path,
    ) -> Result<()> {
        self.db.execute(
            "UPDATE preview_intent SET source_path=? WHERE catalog=? AND photo_id=?",
            params![key(path), catalog_key(catalog), photo.0],
        )?;
        Ok(())
    }
    /// Forgets the requests for these catalog photos: of `kind`, or of every
    /// kind when the photos left the catalog, so a later photo that reuses an
    /// id starts without any.
    pub fn forget_intent(
        &mut self,
        catalog: &CatalogLocation,
        photos: &[PhotoId],
        kind: Option<PreviewKind>,
    ) -> Result<()> {
        let catalog = catalog_key(catalog);
        let tx = self.db.transaction()?;
        for photo in photos {
            match kind {
                Some(kind) => tx.execute(
                    "DELETE FROM preview_intent WHERE catalog=? AND photo_id=? AND kind=?",
                    params![catalog, photo.0, kind.code()],
                )?,
                None => tx.execute(
                    "DELETE FROM preview_intent WHERE catalog=? AND photo_id=?",
                    params![catalog, photo.0],
                )?,
            };
        }
        tx.commit()?;
        Ok(())
    }
    fn sized_bytes(&self, kind: PreviewKind) -> Result<i64> {
        Ok(self.db.query_row(
            "SELECT COALESCE(sum(length(jpeg)),0) FROM sized_previews WHERE kind=?",
            [kind.code()],
            |r| r.get(0),
        )?)
    }
    /// Evicts the least recently used rows of `kind` alone.
    fn prune_sized(&mut self, kind: PreviewKind, budget: i64) -> Result<()> {
        while self.sized_bytes(kind)? > budget {
            self.db.execute(
                "DELETE FROM sized_previews WHERE rowid IN (SELECT rowid FROM sized_previews WHERE kind=?1 ORDER BY last_used,source_path LIMIT 8)",
                [kind.code()],
            )?;
        }
        Ok(())
    }
    /// Gives back to the disk the space dropped thumbnails and previews leave
    /// in the file, which pruning releases only a little at a time: for a
    /// smaller thumbnail budget to free the disk space it promises. The file is
    /// shortened up to [`RELEASE_STEP`] pages, or [`RELEASE_STEP_TIME`], at a
    /// time, each its own short write, with a pause after each as long as the
    /// write took, so the connections
    /// making thumbnails meanwhile get their writes in; for gigabytes it takes
    /// a minute or more, on a thread of its own. It ends once a step gives
    /// nothing back, as in a file made without incremental vacuum.
    pub fn release_free_space(&mut self) -> Result<()> {
        while let Some(pause) = self.release_step()? {
            std::thread::sleep(pause);
        }
        Ok(())
    }
    /// One step of [`Self::release_free_space`], for a worker that gives the
    /// space back between other work: up to [`RELEASE_STEP`] pages in one
    /// short write. Returns how long to pause before the next step, so other
    /// connections get their writes in, or `None` once nothing more can be
    /// given back.
    ///
    /// A step only records the shorter file in the WAL; the file itself is
    /// shortened when the WAL is folded back into it. So once nothing more can
    /// be given back, the step folds it in, unless another connection is
    /// reading or writing, rather than leaving that to whichever connection
    /// closes last (and [`Self::close_at_exit`] never does it).
    pub fn release_step(&mut self) -> Result<Option<Duration>> {
        let started = std::time::Instant::now();
        if self.vacuum_step(RELEASE_STEP_TIME)? == 0 {
            // PASSIVE: it neither waits for other connections nor makes them
            // wait; what it leaves is folded in by a later checkpoint. It is a
            // write as long as the WAL, as is the checkpoint SQLite runs in any
            // step's commit once the WAL reaches 1000 pages, so it costs no
            // more than a step can.
            self.db
                .query_row("PRAGMA wal_checkpoint(PASSIVE)", [], |_| Ok(()))?;
            return Ok(None);
        }
        Ok(Some(RELEASE_PAUSE.max(started.elapsed())))
    }
    /// Closes the connection without folding the WAL back into the file,
    /// which the last connection to close otherwise does: a write as long as
    /// the WAL, flushed to the disk, that took up to 1.65 s on a busy Windows
    /// runner (measured). For a worker stopping at exit, which must wait for
    /// no more than the short write it is in. Nothing is lost: the next
    /// connection reads the WAL, and a later checkpoint folds it in.
    pub fn close_at_exit(self) {
        // Best effort: if it can't be set, closing folds the WAL in as before.
        let _ = self.db.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        );
    }
    /// After a prune, gives back one step of the space left in the file. At
    /// the budget most of what a prune frees is filled again by the next rows,
    /// so one step each time is enough. It is best effort, as another
    /// connection may hold the database; a step not taken is taken after a
    /// later prune.
    fn tidy(&mut self) {
        let _ = self.vacuum_step(RELEASE_STEP_TIME);
    }
    #[cfg(test)]
    fn free_pages(&self) -> Result<i64> {
        Ok(self
            .db
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))?)
    }
    /// Gives back up to [`RELEASE_STEP`] pages, stopping after the first
    /// once `within` has passed; how many it gave back. The pragma frees a
    /// page each time it is stepped (run once, as by `execute_batch`, it frees
    /// a single page), and what it freed is committed when it is stopped early.
    fn vacuum_step(&mut self, within: Duration) -> Result<usize> {
        let started = std::time::Instant::now();
        let mut step = self
            .db
            .prepare(&format!("PRAGMA incremental_vacuum({RELEASE_STEP})"))?;
        let mut rows = step.query([])?;
        let mut freed = 0;
        while rows.next()?.is_some() {
            freed += 1;
            if started.elapsed() >= within {
                break;
            }
        }
        Ok(freed)
    }
    /// Drops the least recently used thumbnails until those left fit
    /// `budget` bytes: the cache is read once, newest first, and the rest
    /// deleted [`PRUNE_STEP`] at a time, each its own short write, so a large
    /// cache cut down to a small budget neither takes time growing with the
    /// square of its size nor keeps other connections waiting on one long
    /// transaction.
    fn prune(&mut self, budget: i64) -> Result<()> {
        // The sum alone is cheap; the newest-first order is read only when
        // there is something to drop.
        let bytes: i64 = self.db.query_row(
            "SELECT COALESCE(sum(length(jpeg)),0) FROM previews",
            [],
            |r| r.get(0),
        )?;
        if bytes > budget {
            self.drop_beyond(budget)?;
        }
        Ok(())
    }
    /// Deletes the thumbnails no lookup can show ([`fresh`]), then those
    /// beyond the newest that fit `budget` bytes.
    fn drop_beyond(&mut self, budget: i64) -> Result<()> {
        let doomed = self.beyond(budget)?;
        self.delete_rows(&doomed)
    }
    /// The rows [`Self::drop_beyond`] deletes, those no lookup can show first.
    /// Freshness goes by generation alone, never reading the files, so the
    /// current thumbnails of offline files stay.
    fn beyond(&self, budget: i64) -> Result<Vec<Doomed>> {
        let mut rows = self.db.prepare(
            "SELECT source_path,prefix_hash,generation,last_used,length(jpeg) FROM previews ORDER BY last_used DESC, source_path DESC",
        )?;
        let mut rows = rows.query([])?;
        let (mut unusable, mut older) = (Vec::new(), Vec::new());
        let mut kept = 0i64;
        while let Some(r) = rows.next()? {
            let (key, family): (String, String) = (r.get(0)?, r.get(1)?);
            let doomed = Doomed {
                generation: r.get(2)?,
                last_used: r.get(3)?,
                key,
            };
            if !fresh(&row_kind(&doomed.key, &family), doomed.generation) {
                unusable.push(doomed);
                continue;
            }
            // Once one doesn't fit, none older is kept, as before rows were
            // told apart: a prune keeps the newest, not the most.
            if kept <= budget {
                kept = kept.saturating_add(r.get(4)?);
            }
            if kept > budget {
                older.push(doomed);
            }
        }
        unusable.append(&mut older);
        Ok(unusable)
    }
    /// Deletes `doomed`, [`PRUNE_STEP`] rows to a write, sparing a row made
    /// again since it was read: at a new generation or a later second. One
    /// made again at the same generation within the same second still goes,
    /// as `last_used` is in whole seconds.
    fn delete_rows(&self, doomed: &[Doomed]) -> Result<()> {
        let mut delete = self.db.prepare(
            "DELETE FROM previews WHERE source_path=?1 AND generation=?2 AND last_used=?3",
        )?;
        for step in doomed.chunks(PRUNE_STEP) {
            let tx = self.db.unchecked_transaction()?;
            for row in step {
                delete.execute(params![row.key, row.generation, row.last_used])?;
            }
            tx.commit()?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn persistent_hits_offline_previews_and_source_invalidation() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let db = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&db)?;
        let im = image::RgbImage::from_pixel(24, 16, image::Rgb([120, 70, 40]));
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        drop(cache);
        let cache = PreviewCache::open(&db)?;
        assert_eq!(cache.load(&raw)?.unwrap().dimensions(), (24, 16));
        std::fs::remove_file(&raw)?;
        assert!(cache.load(&raw)?.is_some());
        std::fs::write(&raw, b"replacement raw")?;
        assert!(cache.load(&raw)?.is_none());
        Ok(())
    }
    #[test]
    fn same_size_rewrite_is_stale_by_its_modification_time() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        let modified = std::fs::metadata(&raw)?.modified()?;
        std::fs::write(&raw, b"replaced raw")?;
        std::fs::File::options()
            .write(true)
            .open(&raw)?
            .set_modified(modified + Duration::from_secs(1))?;
        assert!(cache.load(&raw)?.is_none());
        Ok(())
    }
    /// On a network share every read of a photo is a round trip, so a cached
    /// preview must come back without opening its original.
    #[cfg(unix)]
    #[test]
    fn cached_previews_never_open_the_original() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        cache.store_tagged(&raw, "edit-1", "edit-1", &Stamp::read(&raw)?, &im)?;
        std::fs::set_permissions(&raw, std::fs::Permissions::from_mode(0o000))?;
        // Unless running as root, which ignores permissions.
        if std::fs::File::open(&raw).is_err() {
            assert!(cache.load(&raw)?.is_some());
            assert!(cache.load_tagged(&raw, "edit-1")?.is_some());
        }
        Ok(())
    }
    #[test]
    fn jpeg_thumbnails_made_before_colour_management_are_made_again() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        let jpeg = d.path().join("photo.jpg");
        std::fs::write(&raw, b"original raw")?;
        std::fs::write(&jpeg, b"original jpeg")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        for path in [&raw, &jpeg] {
            cache.store(path, &Stamp::read(path)?, &im)?;
        }
        // A JPEG's row as a build that ignored its ICC profile left it.
        cache.db.execute(
            "UPDATE previews SET generation=2 WHERE source_path=?1",
            [jpeg.to_string_lossy()],
        )?;
        assert!(cache.load(&jpeg)?.is_none());
        // A RAW's current row is kept.
        assert!(cache.load(&raw)?.is_some());
        // A RAW's row from before the one engine is made again.
        cache.db.execute("UPDATE previews SET generation=2", [])?;
        assert!(cache.load(&raw)?.is_none());
        cache.store(&jpeg, &Stamp::read(&jpeg)?, &im)?;
        assert!(cache.load(&jpeg)?.is_some());
        Ok(())
    }
    #[test]
    fn edited_previews_are_kept_apart_from_the_embedded_one() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let stamp = Stamp::read(&raw)?;
        let plain = image::RgbImage::from_pixel(24, 16, image::Rgb([120, 70, 40]));
        let edited = image::RgbImage::from_pixel(16, 16, image::Rgb([20, 70, 140]));
        cache.store(&raw, &stamp, &plain)?;
        cache.store_tagged(&raw, "edit-1", "edit-1", &stamp, &edited)?;
        assert_eq!(cache.load(&raw)?.unwrap().dimensions(), (24, 16));
        assert_eq!(
            cache.load_tagged(&raw, "edit-1")?.unwrap().dimensions(),
            (16, 16)
        );
        assert!(cache.load_tagged(&raw, "edit-2")?.is_none());
        Ok(())
    }
    /// An edited preview of a file changed or removed while it rendered is
    /// stale, not a failure, and is not kept; an unchanged file's is.
    #[test]
    fn edited_previews_of_a_file_changed_since_are_stale() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        let rendered = Stamp::read(&raw)?;
        std::fs::write(&raw, b"rewritten while it rendered")?;
        assert_eq!(
            cache.store_tagged(&raw, "edit-1", "edit-1", &rendered, &im)?,
            StoreOutcome::Stale
        );
        assert!(keys(&cache, "")?.is_empty());
        let stamp = Stamp::read(&raw)?;
        std::fs::remove_file(&raw)?;
        assert_eq!(
            cache.store_tagged(&raw, "edit-1", "edit-1", &stamp, &im)?,
            StoreOutcome::Stale
        );
        assert!(keys(&cache, "")?.is_empty());
        std::fs::write(&raw, b"original raw")?;
        let stamp = Stamp::read(&raw)?;
        assert_eq!(
            cache.store_tagged(&raw, "edit-1", "edit-1", &stamp, &im)?,
            StoreOutcome::Stored
        );
        assert!(cache.load_tagged(&raw, "edit-1")?.is_some());
        // The file's own thumbnail still reports a change as an error.
        std::fs::write(&raw, b"rewritten again")?;
        assert!(cache.store(&raw, &stamp, &im).is_err());
        Ok(())
    }
    /// An edited preview of a file whose folder became unreadable while it
    /// rendered is stale too, while the file's own thumbnail and an oversized
    /// preview are still errors.
    #[cfg(unix)]
    #[test]
    fn edited_previews_of_an_unreachable_file_are_stale() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        /// Opens the folder again, so the temporary directory can be removed.
        struct Reopen<'a>(&'a Path);
        impl Drop for Reopen<'_> {
            fn drop(&mut self) {
                let _ = std::fs::set_permissions(self.0, std::fs::Permissions::from_mode(0o755));
            }
        }
        let d = tempfile::tempdir()?;
        let folder = d.path().join("shoot");
        std::fs::create_dir(&folder)?;
        let raw = folder.join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        let stamp = Stamp::read(&raw)?;
        let reopen = Reopen(&folder);
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000))?;
        if Stamp::read(&raw).is_ok() {
            // Root reads it regardless.
            return Ok(());
        }
        assert_eq!(
            cache.store_tagged(&raw, "edit-1", "edit-1", &stamp, &im)?,
            StoreOutcome::Stale
        );
        assert!(keys(&cache, "")?.is_empty());
        assert!(cache.store(&raw, &stamp, &im).is_err());
        assert!(
            cache
                .store_tagged(
                    &raw,
                    "edit-1",
                    "edit-1",
                    &stamp,
                    &image::RgbImage::new(2048, 8)
                )
                .is_err()
        );
        drop(reopen);
        assert!(Stamp::read(&raw).is_ok());
        Ok(())
    }
    #[test]
    fn reads_survive_failed_maintenance_writes() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let db = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&db)?;
        let im = image::RgbImage::from_pixel(24, 16, image::Rgb([120, 70, 40]));
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        // Another connection holds the write lock: no touch or delete can happen.
        let other = Connection::open(&db)?;
        other.execute_batch("BEGIN IMMEDIATE")?;
        assert!(cache.load(&raw)?.is_some());
        std::fs::write(&raw, b"replacement raw")?;
        assert!(cache.load(&raw)?.is_none());
        other.execute_batch("COMMIT")?;
        assert!(cache.load(&raw)?.is_none());
        Ok(())
    }
    #[test]
    fn broken_entries_budget_and_unrelated_database_are_safe() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.RAF");
        std::fs::write(&raw, b"fixture")?;
        let mut cache = PreviewCache::open(&d.path().join("cache.db"))?;
        let im = image::RgbImage::new(8, 8);
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        cache.db.execute("UPDATE previews SET jpeg=X'001122'", [])?;
        assert!(cache.load(&raw)?.is_none());
        cache.store(&raw, &Stamp::read(&raw)?, &im)?;
        cache.prune(0)?;
        assert!(cache.load(&raw)?.is_none());
        let other = d.path().join("other.db");
        let db = Connection::open(&other)?;
        db.execute("CREATE TABLE precious(value TEXT)", [])?;
        drop(db);
        assert!(PreviewCache::open(&other).is_err());
        assert!(
            Connection::open(&other)?
                .prepare("SELECT * FROM precious")
                .is_ok()
        );
        Ok(())
    }
    /// A prune keeps the most recently used thumbnails that fit the budget,
    /// and drops every one used before them, whatever their number.
    #[test]
    fn a_prune_keeps_the_newest_that_fit() -> Result<()> {
        let d = tempfile::tempdir()?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        for n in 0..3000i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',?2,1,1,zeroblob(100),?3)",
                params![format!("{n}.ARW"), GENERATION, n],
            )?;
        }
        cache.prune(100 * 1000 + 50)?;
        let kept: Vec<i64> = cache
            .db
            .prepare("SELECT last_used FROM previews ORDER BY last_used")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        assert_eq!(kept, (2000..3000).collect::<Vec<_>>());
        cache.prune(0)?;
        let left: i64 = cache
            .db
            .query_row("SELECT count(*) FROM previews", [], |r| r.get(0))?;
        assert_eq!(left, 0);
        Ok(())
    }
    /// The space thumbnails dropped for a smaller budget leave in the file is
    /// given back to the disk.
    #[test]
    fn released_space_shortens_the_file() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&path)?;
        for n in 0..500i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',0,1,1,zeroblob(20000),?2)",
                params![format!("{n}.ARW"), n],
            )?;
        }
        cache.db.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        let full = std::fs::metadata(&path)?.len();
        cache.prune(0)?;
        cache.release_free_space()?;
        let free: i64 = cache
            .db
            .query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        assert_eq!(free, 0);
        assert!(std::fs::metadata(&path)?.len() < full / 10);
        Ok(())
    }
    /// Closing at exit leaves the WAL for the next connection instead of
    /// folding it into the file, and loses nothing.
    #[test]
    fn closing_at_exit_leaves_the_wal_for_later() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let wal = d.path().join("previews.sqlite3-wal");
        let cache = PreviewCache::open(&path)?;
        cache.db.execute(
            "INSERT INTO previews VALUES('a.ARW',0,'0','',0,1,1,zeroblob(20000),0)",
            [],
        )?;
        cache.close_at_exit();
        assert!(std::fs::metadata(&wal)?.len() > 0);
        let other = Connection::open(&path)?;
        let rows: i64 = other.query_row("SELECT count(*) FROM previews", [], |r| r.get(0))?;
        assert_eq!(rows, 1);
        // An ordinary close, as the last connection, folds the WAL in.
        drop(other);
        assert!(!wal.exists());
        Ok(())
    }
    /// One step of giving space back frees up to [`RELEASE_STEP`] pages,
    /// not one.
    #[test]
    fn a_release_step_frees_many_pages() -> Result<()> {
        let d = tempfile::tempdir()?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        for n in 0..500i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',0,1,1,zeroblob(20000),?2)",
                params![format!("{n}.ARW"), n],
            )?;
        }
        cache.prune(0)?;
        let before = cache.free_pages()?;
        assert!(before > i64::from(RELEASE_STEP));
        cache.vacuum_step(Duration::MAX)?;
        assert_eq!(before - cache.free_pages()?, i64::from(RELEASE_STEP));
        Ok(())
    }
    /// A step that runs out of time stops after the page it is on, and keeps
    /// what it gave back.
    #[test]
    fn a_release_step_out_of_time_keeps_what_it_gave_back() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&path)?;
        for n in 0..100i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',0,1,1,zeroblob(20000),?2)",
                params![format!("{n}.ARW"), n],
            )?;
        }
        cache.prune(0)?;
        let before = cache.free_pages()?;
        assert_eq!(cache.vacuum_step(Duration::ZERO)?, 1);
        drop(cache);
        let other = Connection::open(&path)?;
        let free: i64 = other.query_row("PRAGMA freelist_count", [], |r| r.get(0))?;
        assert_eq!(before - free, 1);
        Ok(())
    }
    /// Opening, as storing does every 32 thumbnails, gives back one step of
    /// the space left in the file, not one page per prune. A step also ends
    /// on time, so a slow machine may give back fewer than [`RELEASE_STEP`].
    #[test]
    fn opening_gives_back_a_step_of_the_space_left() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let cache = PreviewCache::open(&path)?;
        for n in 0..500i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',0,1,1,zeroblob(20000),?2)",
                params![format!("{n}.ARW"), n],
            )?;
        }
        cache.db.execute("DELETE FROM previews", [])?;
        let before = cache.free_pages()?;
        assert!(before > i64::from(RELEASE_STEP));
        drop(cache);
        let cache = PreviewCache::open(&path)?;
        let freed = before - cache.free_pages()?;
        assert!(freed > 16 && freed <= i64::from(RELEASE_STEP), "{freed}");
        Ok(())
    }
    /// A file whose space can't be given back page by page (made without
    /// incremental vacuum) ends the release instead of trying for ever.
    #[test]
    fn a_release_ends_where_space_cannot_be_given_back() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let mut cache = PreviewCache::open(&path)?;
        cache.db.execute_batch("PRAGMA auto_vacuum=NONE; VACUUM;")?;
        for n in 0..100i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',0,1,1,zeroblob(20000),?2)",
                params![format!("{n}.ARW"), n],
            )?;
        }
        cache.prune(0)?;
        assert!(cache.free_pages()? > 0);
        cache.release_free_space()?;
        Ok(())
    }
    fn photo(d: &tempfile::TempDir) -> Result<(PathBuf, PreviewCache)> {
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        Ok((raw, cache))
    }
    #[test]
    fn sized_previews_coexist_with_thumbnails_and_each_other() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let stamp = Stamp::read(&raw)?;
        cache.store_tagged(
            &raw,
            "edit-1",
            "edit-1",
            &stamp,
            &image::RgbImage::new(16, 8),
        )?;
        let standard = image::RgbImage::from_pixel(64, 32, image::Rgb([200, 100, 50]));
        let one_to_one = image::RgbImage::from_pixel(128, 64, image::Rgb([50, 100, 200]));
        cache.store_sized(
            &raw,
            "edit-1",
            PreviewKind::Standard,
            &stamp,
            2048,
            &standard,
        )?;
        cache.store_sized(
            &raw,
            "edit-1",
            PreviewKind::OneToOne,
            &stamp,
            0,
            &one_to_one,
        )?;
        assert_eq!(
            cache.load_tagged(&raw, "edit-1")?.unwrap().dimensions(),
            (16, 8)
        );
        let loaded = cache
            .load_sized(&raw, "edit-1", PreviewKind::Standard)?
            .unwrap();
        assert_eq!(loaded.dimensions(), (64, 32));
        assert!(loaded.get_pixel(10, 10)[0] > 150);
        assert_eq!(
            cache
                .load_sized(&raw, "edit-1", PreviewKind::OneToOne)?
                .unwrap()
                .dimensions(),
            (128, 64)
        );
        assert!(
            cache
                .load_sized(&raw, "edit-2", PreviewKind::Standard)?
                .is_none()
        );
        let usage = cache.usage()?;
        assert!(usage.thumbnails > 0 && usage.standard > 0 && usage.one_to_one > 0);
        Ok(())
    }
    #[test]
    fn sized_previews_follow_the_file_and_survive_it_going_offline() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let im = image::RgbImage::new(32, 16);
        cache.store_sized(
            &raw,
            "a",
            PreviewKind::Standard,
            &Stamp::read(&raw)?,
            2048,
            &im,
        )?;
        std::fs::remove_file(&raw)?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::Standard)?
                .is_some()
        );
        assert!(cache.sized_fresh(&raw, "a", PreviewKind::Standard, 2048)?);
        std::fs::write(&raw, b"replacement raw")?;
        assert!(!cache.sized_fresh(&raw, "a", PreviewKind::Standard, 2048)?);
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::Standard)?
                .is_none()
        );
        // A file changed while its preview was built is refused.
        let old = Stamp::read(&raw)?;
        std::fs::write(&raw, b"third version of the raw")?;
        assert!(
            cache
                .store_sized(&raw, "a", PreviewKind::Standard, &old, 2048, &im)
                .is_err()
        );
        Ok(())
    }
    #[cfg(unix)]
    #[test]
    fn sized_previews_never_open_the_original() -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let im = image::RgbImage::new(32, 16);
        cache.store_sized(
            &raw,
            "a",
            PreviewKind::Standard,
            &Stamp::read(&raw)?,
            2048,
            &im,
        )?;
        std::fs::set_permissions(&raw, std::fs::Permissions::from_mode(0o000))?;
        if std::fs::File::open(&raw).is_err() {
            assert!(
                cache
                    .load_sized(&raw, "a", PreviewKind::Standard)?
                    .is_some()
            );
            assert!(cache.sized_fresh(&raw, "a", PreviewKind::Standard, 2048)?);
        }
        Ok(())
    }
    #[test]
    fn freshness_compares_the_requested_edge_not_the_size_reached() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        // A tight crop built for 2048 px only reached 900 px.
        let im = image::RgbImage::new(900, 600);
        cache.store_sized(
            &raw,
            "a",
            PreviewKind::Standard,
            &Stamp::read(&raw)?,
            2048,
            &im,
        )?;
        assert!(cache.sized_fresh(&raw, "a", PreviewKind::Standard, 1440)?);
        assert!(cache.sized_fresh(&raw, "a", PreviewKind::Standard, 2048)?);
        assert!(!cache.sized_fresh(&raw, "a", PreviewKind::Standard, 2560)?);
        assert!(!cache.sized_fresh(&raw, "b", PreviewKind::Standard, 1440)?);
        assert!(!cache.sized_fresh(&raw, "a", PreviewKind::OneToOne, 0)?);
        Ok(())
    }
    #[test]
    fn pruning_one_kind_never_evicts_another() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let stamp = Stamp::read(&raw)?;
        let im = image::RgbImage::new(32, 16);
        cache.store(&raw, &stamp, &im)?;
        cache.store_sized(&raw, "a", PreviewKind::Standard, &stamp, 2048, &im)?;
        cache.store_sized(&raw, "b", PreviewKind::Standard, &stamp, 2048, &im)?;
        cache.store_sized(&raw, "a", PreviewKind::OneToOne, &stamp, 0, &im)?;
        cache.prune_sized(PreviewKind::Standard, 0)?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::Standard)?
                .is_none()
        );
        assert!(
            cache
                .load_sized(&raw, "b", PreviewKind::Standard)?
                .is_none()
        );
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::OneToOne)?
                .is_some()
        );
        assert!(cache.load(&raw)?.is_some());
        cache.prune(0)?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::OneToOne)?
                .is_some()
        );
        Ok(())
    }
    #[test]
    fn corrupt_sized_previews_are_discarded() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let im = image::RgbImage::new(32, 16);
        cache.store_sized(
            &raw,
            "a",
            PreviewKind::Standard,
            &Stamp::read(&raw)?,
            2048,
            &im,
        )?;
        cache
            .db
            .execute("UPDATE sized_previews SET jpeg=X'001122'", [])?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::Standard)?
                .is_none()
        );
        assert_eq!(cache.usage()?.standard, 0);
        Ok(())
    }
    #[test]
    fn discarding_and_clearing_are_per_kind() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let stamp = Stamp::read(&raw)?;
        let im = image::RgbImage::new(32, 16);
        let catalog = CatalogLocation::File(d.path().join("a.rawmakase"));
        cache.store_sized(&raw, "a", PreviewKind::Standard, &stamp, 2048, &im)?;
        cache.store_sized(&raw, "b", PreviewKind::Standard, &stamp, 2048, &im)?;
        cache.store_sized(&raw, "a", PreviewKind::OneToOne, &stamp, 0, &im)?;
        cache.record_intent(&catalog, PhotoId(1), &raw, PreviewKind::Standard)?;
        cache.record_intent(&catalog, PhotoId(1), &raw, PreviewKind::OneToOne)?;
        cache.discard(std::slice::from_ref(&raw), PreviewKind::Standard)?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::Standard)?
                .is_none()
        );
        assert!(
            cache
                .load_sized(&raw, "b", PreviewKind::Standard)?
                .is_none()
        );
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::OneToOne)?
                .is_some()
        );
        cache.clear(PreviewKind::OneToOne)?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::OneToOne)?
                .is_none()
        );
        assert!(!cache.has_intent(&catalog, PhotoId(1), &raw, PreviewKind::OneToOne)?);
        assert!(cache.has_intent(&catalog, PhotoId(1), &raw, PreviewKind::Standard)?);
        Ok(())
    }
    #[test]
    fn intent_belongs_to_one_photo_of_one_catalog() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let other_raw = d.path().join("other.ARW");
        let a = CatalogLocation::File(d.path().join("a.rawmakase"));
        let b = CatalogLocation::File(d.path().join("b.rawmakase"));
        let standard = PreviewKind::Standard;
        cache.record_intent(&a, PhotoId(1), &raw, standard)?;
        assert!(cache.has_intent(&a, PhotoId(1), &raw, standard)?);
        // A virtual copy of the same file, the same photo in another catalog,
        // or another kind has none.
        assert!(!cache.has_intent(&a, PhotoId(2), &raw, standard)?);
        assert!(!cache.has_intent(&b, PhotoId(1), &raw, standard)?);
        assert!(!cache.has_intent(&a, PhotoId(1), &raw, PreviewKind::OneToOne)?);
        // An id that now names another file.
        assert!(!cache.has_intent(&a, PhotoId(1), &other_raw, standard)?);
        cache.record_intent(&a, PhotoId(1), &raw, PreviewKind::OneToOne)?;
        cache.record_intent(&a, PhotoId(2), &raw, standard)?;
        cache.forget_intent(&a, &[PhotoId(1)], Some(standard))?;
        assert!(!cache.has_intent(&a, PhotoId(1), &raw, standard)?);
        assert!(cache.has_intent(&a, PhotoId(1), &raw, PreviewKind::OneToOne)?);
        // Removed from the catalog: every kind goes.
        cache.forget_intent(&a, &[PhotoId(1)], None)?;
        assert!(!cache.has_intent(&a, PhotoId(1), &raw, PreviewKind::OneToOne)?);
        assert!(cache.has_intent(&a, PhotoId(2), &raw, standard)?);
        Ok(())
    }
    #[test]
    fn a_version_one_cache_gains_the_new_tables_and_stays_version_one() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let raw = d.path().join("photo.ARW");
        std::fs::write(&raw, b"original raw")?;
        let old = Connection::open(&path)?;
        old.execute_batch(&format!("PRAGMA application_id={APP_ID}; PRAGMA user_version=1;
            CREATE TABLE previews(source_path TEXT PRIMARY KEY,source_size INTEGER NOT NULL,modified_ns TEXT NOT NULL,prefix_hash TEXT NOT NULL,generation INTEGER NOT NULL,width INTEGER NOT NULL,height INTEGER NOT NULL,jpeg BLOB NOT NULL,last_used INTEGER NOT NULL);
            CREATE INDEX previews_last_used ON previews(last_used);"))?;
        drop(old);
        let mut cache = PreviewCache::open(&path)?;
        let im = image::RgbImage::new(32, 16);
        cache.store_sized(
            &raw,
            "a",
            PreviewKind::Standard,
            &Stamp::read(&raw)?,
            2048,
            &im,
        )?;
        let version: i64 = cache
            .db
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        assert_eq!(version, 1);
        drop(cache);
        // Opening again, as every build does, keeps the rows.
        let cache = PreviewCache::open(&path)?;
        assert!(
            cache
                .load_sized(&raw, "a", PreviewKind::Standard)?
                .is_some()
        );
        Ok(())
    }
    #[test]
    fn unused_previews_of_one_kind_expire() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        let stamp = Stamp::read(&raw)?;
        let im = image::RgbImage::new(32, 16);
        cache.store_sized(&raw, "old", PreviewKind::OneToOne, &stamp, 0, &im)?;
        cache.store_sized(&raw, "new", PreviewKind::OneToOne, &stamp, 0, &im)?;
        cache.store_sized(&raw, "old", PreviewKind::Standard, &stamp, 2048, &im)?;
        let long_ago = now() - 40 * 86400;
        cache.db.execute(
            "UPDATE sized_previews SET last_used=? WHERE identity='old'",
            [long_ago],
        )?;
        let month = Duration::from_secs(30 * 86400);
        assert_eq!(cache.expire(PreviewKind::OneToOne, month)?, 1);
        assert!(
            cache
                .load_sized(&raw, "old", PreviewKind::OneToOne)?
                .is_none()
        );
        assert!(
            cache
                .load_sized(&raw, "new", PreviewKind::OneToOne)?
                .is_some()
        );
        assert!(
            cache
                .load_sized(&raw, "old", PreviewKind::Standard)?
                .is_some()
        );
        // The request for a file whose previews of that kind all went goes too.
        let catalog = CatalogLocation::File(d.path().join("c.rawmakase"));
        let other = d.path().join("other.ARW");
        std::fs::write(&other, b"other raw")?;
        cache.record_intent(&catalog, PhotoId(1), &raw, PreviewKind::OneToOne)?;
        cache.record_intent(&catalog, PhotoId(2), &other, PreviewKind::OneToOne)?;
        cache.store_sized(
            &other,
            "x",
            PreviewKind::OneToOne,
            &Stamp::read(&other)?,
            0,
            &im,
        )?;
        cache.db.execute(
            "UPDATE sized_previews SET last_used=? WHERE source_path=?",
            params![long_ago, key(&other)],
        )?;
        cache.expire(PreviewKind::OneToOne, month)?;
        assert!(cache.has_intent(&catalog, PhotoId(1), &raw, PreviewKind::OneToOne)?);
        assert!(!cache.has_intent(&catalog, PhotoId(2), &other, PreviewKind::OneToOne)?);
        // Asked for again, a preview starts its time again.
        cache
            .db
            .execute("UPDATE sized_previews SET last_used=?", [long_ago])?;
        cache.touch_sized(&raw, "new", PreviewKind::OneToOne)?;
        assert_eq!(cache.expire(PreviewKind::OneToOne, month)?, 0);
        Ok(())
    }
    /// An edit family as `EditSource::tag` names one.
    fn family(n: u64) -> String {
        format!("edit-{n:016x}")
    }
    /// Keeps a tiny preview of `path` under `family`'s variant `suffix`.
    fn store_variant(
        cache: &mut PreviewCache,
        path: &Path,
        family: &str,
        suffix: &str,
    ) -> Result<()> {
        let im = image::RgbImage::new(4, 4);
        cache.store_tagged(
            path,
            family,
            &format!("{family}{suffix}"),
            &Stamp::read(path)?,
            &im,
        )?;
        Ok(())
    }
    /// The keys of the rows whose key starts with `prefix`, sorted.
    fn keys(cache: &PreviewCache, prefix: &str) -> Result<Vec<String>> {
        let mut keys: Vec<String> = cache
            .db
            .prepare("SELECT source_path FROM previews")?
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?
            .into_iter()
            .filter(|k| k.starts_with(prefix))
            .collect();
        keys.sort();
        Ok(keys)
    }
    /// The edit families kept for `path`'s file.
    fn families(cache: &PreviewCache, path: &Path) -> Result<Vec<String>> {
        let rows: Vec<(String, String)> = cache
            .db
            .prepare("SELECT source_path,prefix_hash FROM previews")?
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        let path = key(path);
        let mut families: Vec<String> = rows
            .iter()
            .map(|(k, family)| row_kind(k, family))
            .filter(|row| row.path == path)
            .filter_map(|row| row.family.map(str::to_string))
            .collect();
        families.sort();
        families.dedup();
        Ok(families)
    }
    /// A row is told by its stored family, else by a key as earlier builds
    /// made it; a path that merely holds `#edit-` is a file's own row.
    #[test]
    fn rows_are_told_by_family_then_by_key() {
        let f = family(0xabc);
        let own = |path| Row {
            path,
            family: None,
            tagged: false,
        };
        let edited = |path, family| Row {
            path,
            family: Some(family),
            tagged: true,
        };
        assert_eq!(row_kind("/a/x.ARW", ""), own("/a/x.ARW"));
        let fit = format!("/a/x.ARW#{f}-fit-noai");
        assert_eq!(row_kind(&fit, &f), edited("/a/x.ARW", &f));
        assert_eq!(row_kind(&fit, ""), edited("/a/x.ARW", &f));
        let plain = format!("/a/x.ARW#{f}");
        assert_eq!(row_kind(&plain, ""), edited("/a/x.ARW", &f));
        let folder = format!("/a/#{f}/x.ARW");
        assert_eq!(row_kind(&folder, ""), own(&folder));
        let ai = format!("/a/x.ARW#{f}-fit-ai-0123456789abcdef-a2");
        assert_eq!(row_kind(&ai, ""), edited("/a/x.ARW", &f));
        // Files whose names hold a family, and folders.
        let paths = [
            format!("/a/x.ARW#{f}-copy.ARW"),
            format!("/a/x.ARW#{f}.ARW"),
            format!("/a/x.ARW#{f}-fit.ARW"),
            format!("/a/x.ARW#{f}-fit-ai-m/y.ARW"),
            format!("/a/#{f}-fit/y.ARW"),
        ];
        for path in &paths {
            assert_eq!(row_kind(path, ""), own(path));
        }
        assert_eq!(row_kind("/a/x.ARW#edit-1", ""), own("/a/x.ARW#edit-1"));
        assert_eq!(
            row_kind("/a/x.ARW#edit-1", "edit-1"),
            edited("/a/x.ARW", "edit-1")
        );
    }
    /// A RAW's embedded preview is the camera's (or Lightroom's) and not an
    /// engine render, so it stays usable across engine generations from the
    /// first one that extracted Lightroom's JPEG XL previews. Edited
    /// previews and rasters still need their exact generation.
    #[test]
    fn embedded_raw_previews_outlive_engine_generations() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("photo.ARW");
        let dng = d.path().join("photo.dng");
        let jpeg = d.path().join("photo.jpg");
        for path in [&raw, &dng, &jpeg] {
            std::fs::write(path, b"original")?;
        }
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let im = image::RgbImage::new(8, 8);
        for path in [&raw, &dng, &jpeg] {
            cache.store(path, &Stamp::read(path)?, &im)?;
        }
        store_variant(&mut cache, &raw, &family(1), "-fit")?;
        let set = |key: String, generation: i64| {
            cache.db.execute(
                "UPDATE previews SET generation=?2 WHERE source_path=?1",
                params![key, generation],
            )
        };
        set(key(&raw), 12)?;
        set(key(&dng), EMBEDDED_GENERATION_MIN - 1)?;
        set(key(&jpeg), RASTER_GENERATION - 1)?;
        set(tagged(&raw, &format!("{}-fit", family(1))), 12)?;
        assert!(cache.load(&raw)?.is_some(), "an embedded preview at 12");
        // Before JPEG XL: a Lightroom DNG's preview was its 256 px JPEG.
        assert!(cache.load(&dng)?.is_none());
        assert!(cache.load(&jpeg)?.is_none());
        assert!(
            cache
                .load_tagged(&raw, &format!("{}-fit", family(1)))?
                .is_none()
        );
        // Offline, the embedded preview still shows.
        std::fs::remove_file(&raw)?;
        assert!(cache.load(&raw)?.is_some());
        Ok(())
    }
    /// Each saved edit leaves its own previews; a file keeps those of the
    /// eight edits used last, every variant of each.
    #[test]
    fn a_file_keeps_eight_edit_families() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        for n in 0..10 {
            store_variant(&mut cache, &raw, &family(n), "-fit")?;
            store_variant(&mut cache, &raw, &family(n), "-fit-noai")?;
        }
        let kept = families(&cache, &raw)?;
        assert_eq!(kept.len(), 8, "{kept:?}");
        assert!(kept.contains(&family(9)));
        assert_eq!(keys(&cache, &format!("{}#", key(&raw)))?.len(), 16);
        assert_eq!(
            keys(&cache, &format!("{}#{}", key(&raw), family(9)))?.len(),
            2
        );
        Ok(())
    }
    /// The families used least recently go; the one just stored stays, even
    /// with every other used later (clocks, or another connection).
    #[test]
    fn edit_families_go_least_recently_used_first() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        for n in 0..8 {
            store_variant(&mut cache, &raw, &family(n), "-fit")?;
        }
        // Family 0 used last, family 1 longest ago.
        for n in 0..8u64 {
            let used = if n == 0 { 1000 } else { 100 + n as i64 };
            cache.db.execute(
                "UPDATE previews SET last_used=?2 WHERE source_path=?1",
                params![tagged(&raw, &format!("{}-fit", family(n))), used],
            )?;
        }
        store_variant(&mut cache, &raw, &family(8), "-fit")?;
        let kept = families(&cache, &raw)?;
        assert!(!kept.contains(&family(1)), "{kept:?}");
        assert!(kept.contains(&family(0)) && kept.contains(&family(8)));
        // Every row used after the next store.
        cache
            .db
            .execute("UPDATE previews SET last_used=?", [now() + 1000])?;
        store_variant(&mut cache, &raw, &family(9), "-fit")?;
        let kept = families(&cache, &raw)?;
        assert_eq!(kept.len(), 8);
        assert!(kept.contains(&family(9)));
        Ok(())
    }
    /// Only the file's own previews count: not those of files whose names
    /// begin with its name, nor whose names hold LIKE's wildcards.
    #[test]
    fn edit_families_are_counted_per_file() -> Result<()> {
        let d = tempfile::tempdir()?;
        let raw = d.path().join("zdjęcie_%.ARW");
        std::fs::write(&raw, b"original raw")?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let base = key(&raw);
        let neighbours = [
            format!("{base}2"),
            format!("{base}#y"),
            base.replace('_', "x").replace('%', "y"),
            format!("{base}#z.ARW"),
        ];
        // Files of their own, named as an edit of this one would be keyed.
        let lookalikes = [
            format!("{base}#{}-copy.ARW", family(0)),
            format!("{base}#{}-fit.ARW", family(1)),
        ];
        for lookalike in &lookalikes {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',?2,1,1,zeroblob(10),0)",
                params![lookalike, GENERATION],
            )?;
        }
        for neighbour in &neighbours {
            for n in 0..10 {
                cache.db.execute(
                    "INSERT INTO previews VALUES(?1,0,'0',?2,?3,1,1,zeroblob(10),?4)",
                    params![
                        format!("{neighbour}#{}-fit", family(n)),
                        family(n),
                        GENERATION,
                        n as i64
                    ],
                )?;
            }
        }
        for n in 0..10 {
            store_variant(&mut cache, &raw, &family(100 + n), "-fit")?;
        }
        assert_eq!(families(&cache, &raw)?.len(), 8);
        assert_eq!(keys(&cache, &format!("{base}#edit-"))?.len(), 8 + 2);
        for lookalike in &lookalikes {
            assert_eq!(keys(&cache, lookalike)?, vec![lookalike.clone()]);
        }
        for neighbour in &neighbours {
            assert_eq!(
                keys(&cache, &format!("{neighbour}#edit-"))?.len(),
                10,
                "{neighbour}"
            );
        }
        Ok(())
    }
    /// Rows earlier builds left, without their family, count by the family
    /// their key names.
    #[test]
    fn earlier_builds_edit_rows_count_by_their_key() -> Result<()> {
        let d = tempfile::tempdir()?;
        let (raw, mut cache) = photo(&d)?;
        for n in 0..10u64 {
            for suffix in ["-fit", "-fit-noai", ""] {
                cache.db.execute(
                    "INSERT INTO previews VALUES(?1,0,'0','',?2,1,1,zeroblob(10),?3)",
                    params![
                        tagged(&raw, &format!("{}{suffix}", family(n))),
                        GENERATION,
                        n as i64
                    ],
                )?;
            }
        }
        store_variant(&mut cache, &raw, &family(100), "-fit")?;
        let kept = families(&cache, &raw)?;
        assert_eq!(kept.len(), 8, "{kept:?}");
        // The seven used last, with every one of their rows.
        for n in 3..10 {
            assert!(kept.contains(&family(n)), "{kept:?}");
        }
        assert_eq!(keys(&cache, &format!("{}#", key(&raw)))?.len(), 7 * 3 + 1);
        Ok(())
    }
    /// Over the budget, rows no lookup can use go before any that can, even
    /// those used later; a current preview of an offline file is usable.
    #[test]
    fn a_prune_drops_unusable_rows_first() -> Result<()> {
        let d = tempfile::tempdir()?;
        let mut cache = PreviewCache::open(&d.path().join("previews.sqlite3"))?;
        let row = |key: String, family: &str, generation: i64, used: i64| {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0',?2,?3,1,1,zeroblob(100),?4)",
                params![key, family, generation, used],
            )
        };
        // Live, used long ago, of files that aren't there.
        for n in 0..10i64 {
            row(format!("old{n}.ARW"), "", GENERATION, n)?;
            row(
                format!("old{n}.ARW#{}-fit", family(n as u64)),
                &family(n as u64),
                GENERATION,
                n,
            )?;
            row(format!("old{n}.jpg"), "", RASTER_GENERATION, n)?;
        }
        // Unusable, used lately.
        for n in 0..10i64 {
            row(
                format!("new{n}.ARW#{}-fit", family(n as u64)),
                &family(n as u64),
                GENERATION - 1,
                1000 + n,
            )?;
            row(format!("new{n}.ARW#{}-fit", family(99)), "", 3, 1000 + n)?;
            row(format!("new{n}.jpg"), "", RASTER_GENERATION - 1, 1000 + n)?;
            row(
                format!("new{n}.dng"),
                "",
                EMBEDDED_GENERATION_MIN - 1,
                1000 + n,
            )?;
        }
        // Under the budget, nothing goes.
        cache.prune(100 * 70)?;
        assert_eq!(keys(&cache, "")?.len(), 70);
        cache.prune(100 * 30)?;
        let left = keys(&cache, "")?;
        assert_eq!(left.len(), 30, "{left:?}");
        assert!(left.iter().all(|k| k.starts_with("old")), "{left:?}");
        Ok(())
    }
    /// A row another connection makes again between a prune's reading and
    /// its deleting is not deleted.
    #[test]
    fn a_prune_spares_rows_made_again_meanwhile() -> Result<()> {
        let d = tempfile::tempdir()?;
        let path = d.path().join("previews.sqlite3");
        let cache = PreviewCache::open(&path)?;
        for n in 0..4i64 {
            cache.db.execute(
                "INSERT INTO previews VALUES(?1,0,'0','',?2,1,1,zeroblob(100),?3)",
                params![format!("{n}.ARW"), GENERATION - 1, n],
            )?;
        }
        let doomed = cache.beyond(0)?;
        assert_eq!(doomed.len(), 4);
        let other = Connection::open(&path)?;
        other.execute(
            "UPDATE previews SET generation=?1,last_used=?2 WHERE source_path='0.ARW'",
            params![GENERATION, now()],
        )?;
        cache.delete_rows(&doomed)?;
        assert_eq!(keys(&cache, "")?, vec!["0.ARW".to_string()]);
        Ok(())
    }
}
