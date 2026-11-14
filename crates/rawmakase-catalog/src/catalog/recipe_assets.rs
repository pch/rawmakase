//! The camera profiles and Lensfun calibrations stored recipes name by content id,
//! each stored once in the `recipe_assets` table (see
//! [`rawmakase_model::storage::recipe_assets`]).
//!
//! They are written in the transaction of the rows that name them, and, like mask
//! rasters, never replaced or collected: a History, snapshot or virtual copy that
//! still names one must stay readable. Export as Catalog leaves out the ones its
//! copy no longer names.
use super::db::{Db, Reads, Write, sql};
use super::{Catalog, CatalogLocation};
use crate::storage::recipe_assets::{
    self, Asset, AssetKind, AssetSource, MAX_ASSET_BYTES, SharedSource,
};
use anyhow::Result;
use std::sync::{Arc, Mutex};

/// Largest stored (compressed) asset read back: the largest asset compresses to
/// less, so a bigger row is damaged and is not read into memory.
const MOST_STORED_BYTES: i64 = MAX_ASSET_BYTES as i64 + (1 << 20);

/// The kind and stored bytes of the asset `id`, unless its row is larger than any
/// asset can be: `Ok(None)` when there is no row.
fn stored(db: &impl Reads, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
    super::value::row! {
        struct Size {
            kind: String,
            length: i64,
        }
    }
    let size: Option<Size> = db
        .read_optional(
            sql!("SELECT kind, length(data) FROM recipe_assets WHERE id=?"),
            &[&id],
        )
        .map_err(|e| e.to_string())?;
    let Some(size) = size else {
        return Ok(None);
    };
    let kind = AssetKind::from_name(&size.kind)
        .ok_or_else(|| format!("its kind {:?} is not one this release reads", size.kind))?;
    if size.length > MOST_STORED_BYTES {
        return Err(format!("its stored copy is {} bytes", size.length));
    }
    let data: Vec<u8> = db
        .read_one(sql!("SELECT data FROM recipe_assets WHERE id=?"), &[&id])
        .map_err(|e| e.to_string())?;
    Ok(Some((kind, data)))
}

/// Reads assets through a connection of the catalog's own, or its transaction: a
/// read inside a write sees the rows that write stored.
pub(super) struct ReadsSource<'a, R: Reads>(pub &'a R);
impl<R: Reads> AssetSource for ReadsSource<'_, R> {
    fn stored(&self, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
        stored(self.0, id)
    }
}

/// Stores `assets` in `w`'s transaction, before the rows that name them. An asset
/// already there must be what its id names, or a row would point at damaged data;
/// a damaged one is replaced with the good one.
pub(super) fn write(w: &mut Write<'_>, assets: &[Asset]) -> Result<()> {
    for asset in assets {
        let data = asset.compressed();
        let inserted = w.execute(
            sql!(
                "INSERT INTO recipe_assets(id, kind, data) VALUES (?, ?, ?) ON CONFLICT DO NOTHING"
            ),
            &[&asset.id, &asset.kind().name(), &data],
        )?;
        if inserted == 0 {
            let intact = stored(w, &asset.id)
                .ok()
                .flatten()
                .is_some_and(|(kind, bytes)| {
                    kind == asset.kind() && Asset::from_stored(&asset.id, kind, &bytes).is_ok()
                });
            if !intact {
                w.execute(
                    sql!("UPDATE recipe_assets SET kind=?, data=? WHERE id=?"),
                    &[&asset.kind().name(), &data, &asset.id],
                )?;
            }
        }
    }
    Ok(())
}

/// Notes that a write of `assets` committed: later reads of them share these values.
pub(super) fn committed(assets: &[Asset]) {
    for asset in assets {
        recipe_assets::remember(asset);
    }
}

impl Catalog {
    /// Where values read from this catalog resolve their references: a reader with a
    /// connection of its own, opened the first time it is needed, so a record read
    /// here still resolves on another thread, and after the catalog is closed.
    pub fn asset_source(&self) -> SharedSource {
        self.assets.clone()
    }
}

/// The reader [`Catalog::asset_source`] gives.
pub(super) fn shared_source(location: &CatalogLocation) -> SharedSource {
    SharedSource::new(Arc::new(CatalogAssets {
        location: location.clone(),
        db: Mutex::new(None),
    }))
}

struct CatalogAssets {
    location: CatalogLocation,
    db: Mutex<Option<Db>>,
}
impl AssetSource for CatalogAssets {
    fn stored(&self, id: &str) -> Result<Option<(AssetKind, Vec<u8>)>, String> {
        let mut guard = self.db.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_none() {
            *guard = Some(Db::open(&self.location).map_err(|e| format!("{e:#}"))?);
        }
        let db = guard.as_ref().expect("opened above");
        let found = stored(db, id);
        if found.is_err() {
            // A connection that failed is opened again next time.
            *guard = None;
        }
        found
    }
}

/// Damages or deletes the `assets` just stored, when a test asks
/// ([`super::format_upgrade::faults`]).
#[cfg(test)]
pub(super) fn damage_for_tests(w: &mut Write<'_>, assets: &[Asset]) -> Result<()> {
    use super::format_upgrade::faults::{DAMAGE_ASSETS, DROP_ASSETS};
    for asset in assets {
        if DAMAGE_ASSETS.with(std::cell::Cell::get) {
            let other = recipe_assets::compress(b"{\"not\": \"this asset\"}");
            w.execute(
                sql!("UPDATE recipe_assets SET data=? WHERE id=?"),
                &[&other, &asset.id],
            )?;
        }
        if DROP_ASSETS.with(std::cell::Cell::get) {
            w.execute(sql!("DELETE FROM recipe_assets WHERE id=?"), &[&asset.id])?;
        }
    }
    Ok(())
}
