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

/// What [`delete_unnamed`] did.
pub(super) struct Deleted {
    /// Every raster was kept, as a History couldn't be read.
    pub(super) kept_every: bool,
    /// Stored (compressed) bytes of the rasters deleted.
    pub(super) bytes: i64,
}

/// Every stored text that may name a mask raster, patch, profile or calibration:
/// edits, Lightroom settings, spots and masks, snapshots and inflated Histories,
/// whatever release wrote them. `None` when a History can't be read.
pub(super) fn naming_texts(w: &Write<'_>) -> Result<Option<Vec<String>>> {
    let mut texts: Vec<String> = Vec::new();
    for statement in [
        sql!("SELECT recipe FROM photos WHERE recipe IS NOT NULL"),
        sql!("SELECT lightroom_develop FROM photos WHERE lightroom_develop IS NOT NULL"),
        sql!("SELECT data FROM local_edits"),
        sql!("SELECT recipe FROM develop_snapshots WHERE recipe IS NOT NULL"),
    ] {
        texts.extend(w.read::<String>(statement, &[])?);
    }
    let histories: Vec<Vec<u8>> = w.read(sql!("SELECT data FROM develop_history"), &[])?;
    for data in histories {
        match super::develop_history::text(&data) {
            Ok(text) => texts.push(String::from_utf8_lossy(&text).into_owned()),
            Err(_) => return Ok(None),
        }
    }
    Ok(Some(texts))
}

/// Deletes the rasters no edit, History or snapshot names, in `w`'s transaction.
/// A raster is kept if its id appears anywhere in them, whatever release wrote
/// them (its hex digits are enough); if a History can't be read, every raster is
/// kept. A raster is stored in the transaction of the rows that name it
/// ([`Assets::write`]), so one a save is storing is never seen here unnamed.
pub(super) fn delete_unnamed(w: &mut Write<'_>) -> Result<Deleted> {
    let hashes: Vec<String> = w.read(sql!("SELECT hash FROM bitmaps"), &[])?;
    let mut deleted = Deleted {
        kept_every: false,
        bytes: 0,
    };
    if hashes.is_empty() {
        return Ok(deleted);
    }
    let Some(texts) = naming_texts(w)? else {
        deleted.kept_every = true;
        return Ok(deleted);
    };
    let named = HexRuns::of(&texts);
    for hash in hashes.iter().filter(|h| !named.contains(h)) {
        let bytes: Option<i64> = w.read_one(
            sql!("SELECT length(data) FROM bitmaps WHERE hash=?"),
            &[hash],
        )?;
        w.execute(sql!("DELETE FROM bitmaps WHERE hash=?"), &[hash])?;
        deleted.bytes += bytes.unwrap_or(0);
    }
    Ok(deleted)
}

/// Every stretch of lowercase hex digits in stored texts as long as a raster id's
/// digits (32 for an older id, 64 after `sha256:`), so each raster is looked up
/// once rather than searched for in every text. A longer run gives each of its
/// stretches: an id inside it is as named as one on its own.
struct HexRuns<'a>(HashSet<&'a str>);
impl<'a> HexRuns<'a> {
    fn of(texts: &'a [String]) -> Self {
        let mut found = HashSet::new();
        let is_hex = |b: &u8| b.is_ascii_digit() || (b'a'..=b'f').contains(b);
        for text in texts {
            let bytes = text.as_bytes();
            let mut start = 0;
            while start < bytes.len() {
                let len = bytes[start..].iter().take_while(|b| is_hex(b)).count();
                for width in [32, 64] {
                    for at in start..(start + len + 1).saturating_sub(width) {
                        found.insert(&text[at..at + width]);
                    }
                }
                start += len.max(1);
            }
        }
        Self(found)
    }
    /// Whether `id`'s hex digits appear in a text.
    fn contains(&self, id: &str) -> bool {
        self.0.contains(id.strip_prefix("sha256:").unwrap_or(id))
    }
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
