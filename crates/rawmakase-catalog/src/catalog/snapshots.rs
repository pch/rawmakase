//! Develop Snapshots: named states of a photo's edit, as Lightroom's Snapshots panel
//! keeps them. Each photo and virtual copy has its own, as in Lightroom's catalog,
//! where snapshots belong to one image. Snapshots imported from Lightroom keep
//! Lightroom's settings text and are converted when applied.
use super::db::{LightroomWrite, Reads, SqliteSql, sql, sqlite_sql};
use super::value::{TextOrBlob, row};
use super::{Catalog, PhotoId};
use crate::model::recipe::Recipe;
use anyhow::{Context, Result, ensure};

/// Copies snapshots from a Lightroom catalog attached as `lr`.
pub(super) const COPY_LIGHTROOM_SNAPSHOTS: SqliteSql = sqlite_sql!(
    "INSERT INTO develop_snapshots(photo, name, recipe, lightroom)
    SELECT image, COALESCE(name, ''), NULL, text
    FROM lr.Adobe_libraryImageDevelopSnapshot
    WHERE text IS NOT NULL AND image IN (SELECT id FROM photos)
      AND NOT EXISTS (SELECT 1 FROM develop_snapshots s
                      WHERE s.photo = image AND s.name = COALESCE(name, '') AND s.lightroom = text)"
);

/// A snapshot, by name.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    pub id: i64,
    pub name: String,
    pub settings: SnapshotSettings,
}

/// What a snapshot holds.
#[derive(Clone, Debug, PartialEq)]
pub enum SnapshotSettings {
    /// Made in RAWmakase.
    Recipe(Box<Recipe>),
    /// Imported from Lightroom: its develop-settings text.
    Lightroom(String),
}

/// Set in `meta` once snapshots have been recovered from the stored Lightroom catalog.
pub(super) const SNAPSHOTS_BACKFILLED: &str = "lightroom_snapshots_backfilled";

impl Catalog {
    /// Catalogs imported before snapshots were kept still hold the original Lightroom
    /// catalog; copy its snapshots once. Returns snapshots added.
    pub fn backfill_lightroom_snapshots(&mut self) -> Result<usize> {
        self.backfill_once(SNAPSHOTS_BACKFILLED, copy_lightroom_snapshots)
    }
    /// The photo's snapshots, alphabetically as Lightroom lists them. A snapshot
    /// that cannot be read (from a newer release) is left out.
    pub fn snapshots(&self, photo: PhotoId) -> Result<Vec<Snapshot>> {
        row! {
            struct Stored {
                id: i64,
                name: String,
                recipe: Option<String>,
                lightroom: TextOrBlob,
            }
        }
        let rows: Vec<Stored> = self.db.read(
            sql!("SELECT id, name, recipe, lightroom FROM develop_snapshots WHERE photo=?"),
            &[&photo],
        )?;
        let mut snapshots: Vec<Snapshot> = rows
            .into_iter()
            .filter_map(
                |Stored {
                     id,
                     name,
                     recipe,
                     lightroom: TextOrBlob(lightroom),
                 }| {
                    let settings = match (recipe, lightroom) {
                        (Some(json), _) => {
                            let recipe: Recipe = serde_json::from_str(&json).ok()?;
                            recipe.validate().ok()?;
                            SnapshotSettings::Recipe(Box::new(recipe))
                        }
                        (None, Some(bytes)) => SnapshotSettings::Lightroom(
                            super::lightroom::history::decode_history_text(&bytes)?,
                        ),
                        (None, None) => return None,
                    };
                    Some(Snapshot { id, name, settings })
                },
            )
            .collect();
        snapshots.sort_by_cached_key(|s| (s.name.to_lowercase(), s.id));
        Ok(snapshots)
    }
    /// Saves `recipe` as a new snapshot of the photo named `name`; returns its id.
    pub fn add_snapshot(&mut self, photo: PhotoId, name: &str, recipe: &Recipe) -> Result<i64> {
        recipe.validate()?;
        self.upgrade_before_storing_recipes()?;
        let assets = self.assets_of(recipe.mask_asset_ids())?;
        let (name, recipe) = (snapshot_name(name)?, serde_json::to_string(recipe)?);
        let id = self.db.write(|w| {
            assets.write(w)?;
            w.insert_returning_id(
                sql!(
                    "INSERT INTO develop_snapshots(photo, name, recipe) VALUES (?, ?, ?)
                     RETURNING id"
                ),
                &[&photo, &name, &recipe],
            )
        })?;
        assets.saved();
        Ok(id)
    }
    /// Lightroom's Update with Current Settings: the snapshot now holds `recipe`.
    pub fn update_snapshot(&mut self, id: i64, recipe: &Recipe) -> Result<()> {
        recipe.validate()?;
        self.upgrade_before_storing_recipes()?;
        let assets = self.assets_of(recipe.mask_asset_ids())?;
        let recipe = serde_json::to_string(recipe)?;
        let n = self.db.write(|w| {
            assets.write(w)?;
            w.execute(
                sql!("UPDATE develop_snapshots SET recipe=?, lightroom=NULL WHERE id=?"),
                &[&recipe, &id],
            )
        })?;
        ensure!(n == 1, "Unknown snapshot");
        assets.saved();
        Ok(())
    }
    pub fn rename_snapshot(&mut self, id: i64, name: &str) -> Result<()> {
        let name = snapshot_name(name)?;
        let n = self.db.write(|w| {
            w.execute(
                sql!("UPDATE develop_snapshots SET name=? WHERE id=?"),
                &[&name, &id],
            )
        })?;
        ensure!(n == 1, "Unknown snapshot");
        Ok(())
    }
    pub fn delete_snapshot(&mut self, id: i64) -> Result<()> {
        self.db
            .write(|w| w.execute(sql!("DELETE FROM develop_snapshots WHERE id=?"), &[&id]))?;
        Ok(())
    }
}

/// Copies snapshots from a Lightroom catalog attached as `lr` that has them.
fn copy_lightroom_snapshots(lr: &mut LightroomWrite<'_>) -> Result<usize> {
    if !lr.has_table("Adobe_libraryImageDevelopSnapshot")? {
        return Ok(0);
    }
    lr.execute_sqlite(COPY_LIGHTROOM_SNAPSHOTS, &[])
}

/// A snapshot's name, trimmed; it may not be empty.
fn snapshot_name(name: &str) -> Result<&str> {
    let name = name.trim();
    (!name.is_empty())
        .then_some(name)
        .context("A snapshot needs a name")
}

/// Removes the settings that chose an engine or operator
/// (`saved_format::OBSOLETE_SETTINGS`) from every stored snapshot recipe, in the
/// catalog's upgrade; ones this release cannot read are left as they are.
pub(super) fn migrate_snapshots(w: &mut super::db::Write<'_>) -> Result<()> {
    use crate::model::saved_format::recipe_text_without_obsolete_settings;
    row! {
        struct Stored {
            id: i64,
            recipe: String,
        }
    }
    let snapshots: Vec<Stored> = w.read(
        sql!("SELECT id, recipe FROM develop_snapshots WHERE recipe IS NOT NULL"),
        &[],
    )?;
    for snapshot in snapshots {
        if let Some(recipe) = recipe_text_without_obsolete_settings(&snapshot.recipe) {
            w.execute(
                sql!("UPDATE develop_snapshots SET recipe=? WHERE id=?"),
                &[&recipe, &snapshot.id],
            )?;
        }
    }
    Ok(())
}
