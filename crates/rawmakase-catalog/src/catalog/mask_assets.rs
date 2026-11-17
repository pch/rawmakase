//! The mask rasters edits and snapshots refer to: stored with the rows that
//! refer to them, in the `bitmaps` table, and read back through the loader the app
//! installs (see [`rawmakase_model::storage::mask_assets`]).
//!
//! Rasters are immutable and named by content, so nothing here replaces them. A
//! raster an edit, History, snapshot or virtual copy still names stays readable;
//! one nothing names any more (a deleted spot's patch, a regenerated selection, a
//! removed photo's masks) is deleted as the catalog opens
//! ([`Catalog::remove_unused_data`]), so a catalog doesn't keep every patch it
//! ever held.
use super::db::{Db, Reads, Write, sql};
use super::value::row;
use super::{Catalog, CatalogLocation};
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets::{self, AssetError, AssetLoader};
use anyhow::Result;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};

/// Largest stored (compressed) raster read back: the largest raster compresses to less,
/// so a bigger row is damaged and is not read into memory.
pub(super) const MOST_STORED_BYTES: i64 = (mask_assets::MAX_RASTER_BYTES as i64) + (1 << 20);

/// The stored bytes of the raster `id`, unless the row is larger than any raster can be:
/// `Ok(None)` when there is no row, `Err` when it is too large.
fn stored(db: &impl Reads, id: &str) -> Result<Option<Vec<u8>>, String> {
    let length: Option<Option<i64>> = db
        .read_optional(
            sql!("SELECT length(data) FROM bitmaps WHERE hash=?"),
            &[&id],
        )
        .map_err(|e| e.to_string())?;
    match length {
        None => Ok(None),
        Some(Some(n)) if n > MOST_STORED_BYTES => Err(format!("its stored copy is {n} bytes")),
        Some(_) => db
            .read_optional(sql!("SELECT data FROM bitmaps WHERE hash=?"), &[&id])
            .map_err(|e| e.to_string()),
    }
}

/// The rasters one write refers to: the ones to store, and every one that has to exist
/// once it commits.
#[derive(Default)]
pub(super) struct Assets {
    unsaved: Vec<(String, Arc<Vec<u8>>)>,
    required: Vec<String>,
}
impl Assets {
    /// Stores the unsaved rasters and checks that every required one is there, in
    /// the transaction of the rows that refer to them.
    pub(super) fn write(&self, w: &mut Write<'_>) -> Result<()> {
        for (id, blob) in &self.unsaved {
            let inserted = w.execute(
                sql!("INSERT INTO bitmaps(hash, data) VALUES (?, ?) ON CONFLICT DO NOTHING"),
                &[id, blob.as_ref()],
            )?;
            // Already there: it must be what its ID names, or the edit would point at
            // damaged data once the good copy held here is let go. A damaged row is
            // replaced with the good one.
            if inserted == 0 {
                let intact = stored(w, id)
                    .ok()
                    .flatten()
                    .and_then(|bytes| Bitmap::decompress(&bytes).ok())
                    .is_some_and(|b| b.matches_id(id));
                if !intact {
                    w.execute(
                        sql!("UPDATE bitmaps SET data=? WHERE hash=?"),
                        &[blob.as_ref(), id],
                    )?;
                }
            }
        }
        for id in &self.required {
            let found: Option<i64> =
                w.read_optional(sql!("SELECT 1 FROM bitmaps WHERE hash=?"), &[id])?;
            if found.is_none() {
                // Saved once, then deleted as nothing named it any more (by another
                // window that opened the catalog, say), while this process still
                // holds it: stored again rather than the save refused.
                let held = mask_assets::resolve(id)
                    .map_err(|_| AssetError::Missing(id.clone()))
                    .and_then(|raster| {
                        raster
                            .compress()
                            .map_err(|_| AssetError::Missing(id.clone()))
                    })?;
                super::edits::insert_bitmap(w, id, &held)?;
            }
        }
        Ok(())
    }
    /// Notes that the write committed: the rasters can be read again from here.
    pub(super) fn saved(&self) {
        mask_assets::mark_saved(self.required.iter().map(String::as_str));
    }
}

/// Rows read at a time when looking through stored texts, so memory holds a few
/// inflated Histories, never all of them.
const BATCH: i64 = 32;

/// Which of `ids` (raster, profile or calibration ids) appear in a stored text
/// that may name one: edits, Lightroom settings, spots and masks, snapshots and
/// inflated Histories, whatever release wrote them. `None` when a History can't
/// be read. The rows are read a batch at a time, in `w`'s transaction.
pub(super) fn named(w: &Write<'_>, ids: &[String]) -> Result<Option<HashSet<String>>> {
    row! {
        struct Text {
            key: i64,
            text: String,
        }
    }
    row! {
        struct Blob {
            key: i64,
            data: Vec<u8>,
        }
    }
    let mut finder = Finder::new(ids);
    for statement in [
        sql!(
            "SELECT id, recipe FROM photos WHERE id > ? AND recipe IS NOT NULL ORDER BY id LIMIT ?"
        ),
        sql!(
            "SELECT id, lightroom_develop FROM photos
             WHERE id > ? AND lightroom_develop IS NOT NULL ORDER BY id LIMIT ?"
        ),
        sql!("SELECT photo, data FROM local_edits WHERE photo > ? ORDER BY photo LIMIT ?"),
        sql!(
            "SELECT id, recipe FROM develop_snapshots
             WHERE id > ? AND recipe IS NOT NULL ORDER BY id LIMIT ?"
        ),
    ] {
        let mut after = i64::MIN;
        while !finder.done() {
            let rows: Vec<Text> = w.read(statement, &[&after, &BATCH])?;
            let Some(last) = rows.last() else { break };
            after = last.key;
            rows.iter().for_each(|r| finder.scan(&r.text));
        }
    }
    let mut after = i64::MIN;
    while !finder.done() {
        let rows: Vec<Blob> = w.read(
            sql!("SELECT photo, data FROM develop_history WHERE photo > ? ORDER BY photo LIMIT ?"),
            &[&after, &BATCH],
        )?;
        let Some(last) = rows.last() else { break };
        after = last.key;
        for row in &rows {
            match super::develop_history::text(&row.data) {
                Ok(text) => finder.scan(&String::from_utf8_lossy(&text)),
                Err(_) => return Ok(None),
            }
        }
    }
    Ok(Some(finder.found))
}

/// Finds ids in texts. An id of a known shape (32 lowercase hex digits, or 64 after
/// `sha256:`) is looked up by its digits among each text's stretches of hex digits,
/// so it counts as named wherever its digits appear; any other id is searched for
/// as it is.
struct Finder {
    by_digits: std::collections::HashMap<String, String>,
    other: Vec<String>,
    found: HashSet<String>,
}
impl Finder {
    fn new(ids: &[String]) -> Self {
        let mut finder = Self {
            by_digits: Default::default(),
            other: Vec::new(),
            found: HashSet::new(),
        };
        for id in ids {
            let digits = id.strip_prefix("sha256:").unwrap_or(id);
            let shaped = matches!(
                (id.starts_with("sha256:"), digits.len()),
                (true, 64) | (false, 32)
            ) && digits.bytes().all(is_hex);
            if shaped {
                finder.by_digits.insert(digits.to_string(), id.clone());
            } else {
                finder.other.push(id.clone());
            }
        }
        finder
    }
    fn done(&self) -> bool {
        self.by_digits.is_empty() && self.other.is_empty()
    }
    fn scan(&mut self, text: &str) {
        if self.done() {
            return;
        }
        let bytes = text.as_bytes();
        let mut start = 0;
        while start < bytes.len() && !self.by_digits.is_empty() {
            let len = bytes[start..].iter().take_while(|b| is_hex(**b)).count();
            for width in [32, 64] {
                for at in start..(start + len + 1).saturating_sub(width) {
                    if let Some(id) = self.by_digits.remove(&text[at..at + width]) {
                        self.found.insert(id);
                    }
                }
            }
            start += len.max(1);
        }
        let (found, other): (Vec<String>, Vec<String>) = std::mem::take(&mut self.other)
            .into_iter()
            .partition(|id| text.contains(id.as_str()));
        self.found.extend(found);
        self.other = other;
    }
}
fn is_hex(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// The rasters no edit, History or snapshot names ([`named`]): `None` when a
/// History can't be read, so every raster has to be kept. A raster is stored in
/// the transaction of the rows that name it ([`Assets::write`]), so one a save is
/// storing is never seen here unnamed.
pub(super) fn unnamed(w: &Write<'_>) -> Result<Option<Vec<String>>> {
    let hashes: Vec<String> = w.read(sql!("SELECT hash FROM bitmaps"), &[])?;
    if hashes.is_empty() {
        return Ok(Some(hashes));
    }
    Ok(named(w, &hashes)?.map(|named| hashes.into_iter().filter(|h| !named.contains(h)).collect()))
}

/// Deletes the rasters `hashes`, in `w`'s transaction, and returns their stored
/// (compressed) bytes.
pub(super) fn delete(w: &mut Write<'_>, hashes: &[String]) -> Result<i64> {
    let mut bytes = 0;
    for hash in hashes {
        let stored: Option<Option<i64>> = w.read_optional(
            sql!("SELECT length(data) FROM bitmaps WHERE hash=?"),
            &[hash],
        )?;
        w.execute(sql!("DELETE FROM bitmaps WHERE hash=?"), &[hash])?;
        bytes += stored.flatten().unwrap_or(0);
    }
    Ok(bytes)
}

impl Catalog {
    /// The rasters a write that refers to `ids` has to store or find. A catalog of
    /// a format without raster masks is upgraded as it opens, so none reaches one.
    pub(super) fn assets_of<'a>(&self, ids: impl IntoIterator<Item = &'a str>) -> Result<Assets> {
        let mut required: Vec<String> = ids.into_iter().map(str::to_string).collect();
        required.sort();
        required.dedup();
        if required.is_empty() {
            return Ok(Assets::default());
        }
        let unsaved = mask_assets::unsaved(required.iter().map(String::as_str));
        Ok(Assets { unsaved, required })
    }
    /// A reader of this catalog's rasters for the mask asset store. It opens its
    /// own connection the first time one is needed, so it can be called from any
    /// thread and holds nothing meanwhile.
    pub fn mask_asset_loader(&self) -> Arc<dyn AssetLoader> {
        Arc::new(CatalogAssets {
            location: self.location.clone(),
            db: Mutex::new(None),
        })
    }
}

struct CatalogAssets {
    location: CatalogLocation,
    db: Mutex<Option<Db>>,
}
impl AssetLoader for CatalogAssets {
    fn source(&self) -> String {
        let CatalogLocation::File(path) = &self.location;
        // Debug keeps every byte of a path that is not UTF-8, so two such catalogs never
        // share a reader.
        format!("{path:?}")
    }
    fn load(&self, id: &str) -> Result<Option<Bitmap>, AssetError> {
        let corrupt =
            |why: &dyn std::fmt::Display| AssetError::Corrupt(id.to_string(), why.to_string());
        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(Db::open(&self.location).map_err(|e| corrupt(&e))?);
        }
        let db = guard.as_ref().expect("opened above");
        let data = match stored(db, id) {
            Ok(data) => data,
            Err(error) => {
                // A connection that failed is opened again next time.
                *guard = None;
                return Err(corrupt(&error));
            }
        };
        data.map(|bytes| Bitmap::decompress(&bytes).map_err(|e| corrupt(&e)))
            .transpose()
    }
}
