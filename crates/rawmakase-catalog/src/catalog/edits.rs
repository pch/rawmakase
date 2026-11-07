//! A photo's saved edit: its recipe and export options, the spots and masks
//! kept beside them, and the bitmaps recipes refer to by hash.
use super::db::{Reads, Write, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::edits::{Original, SavedEdit, local_edits};
use crate::storage::Identity;
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use anyhow::{Context, Result};

/// One photo's change for [`Catalog::change_edits`].
pub enum EditChange<'a, 'b> {
    Save(&'b EditToSave<'a>),
    Clear(EditToClear<'a>),
}

/// One photo's edit to clear, checked as a save is: refused when `original` is
/// no longer the photo `source` names, or not the photo its stored edit was
/// saved for, so a clear never removes a protected edit.
#[derive(Clone, Copy)]
pub struct EditToClear<'a> {
    pub id: PhotoId,
    /// The photo's original, as for [`EditToSave::original`].
    pub original: Original<'a>,
    /// The file the clear is meant for: for Sync's Undo, the file the Sync
    /// read before writing its settings.
    pub source: &'a Identity,
}

/// One photo's edit for [`Catalog::save_edits`].
pub struct EditToSave<'a> {
    pub id: PhotoId,
    /// The photo's original: its file, or for a photo opened from its editable
    /// preview while the file is not there, the identity that preview recorded.
    pub original: Original<'a>,
    /// The file the edit was made for, as read when its photo was opened or its
    /// settings read (or recorded by its editable preview): a save to an original
    /// that is no longer the same photo ([`Identity::same_photo`]) is refused, even
    /// with no saved edit there to protect.
    pub source: &'a Identity,
    pub recipe: &'a Recipe,
    pub export: &'a ExportOptions,
    pub history: super::HistoryUpdate<'a>,
}

impl Catalog {
    #[cfg(any(test, feature = "test-support"))]
    /// Stores `bitmap` once and returns the hash that refers to it.
    pub fn put_bitmap(&mut self, bitmap: &crate::storage::bitmaps::Bitmap) -> Result<String> {
        let hash = bitmap.hash();
        let data = bitmap.compress()?;
        self.db.write(|w| insert_bitmap(w, &hash, &data))?;
        Ok(hash)
    }
    #[cfg(test)]
    pub(crate) fn bitmap(&self, hash: &str) -> Result<Option<crate::storage::bitmaps::Bitmap>> {
        let data: Option<Vec<u8>> = self
            .db
            .read_optional(sql!("SELECT data FROM bitmaps WHERE hash=?"), &[&hash])?;
        data.map(|d| crate::storage::bitmaps::Bitmap::decompress(&d))
            .transpose()
    }
    pub fn save_edit<'a>(
        &mut self,
        id: PhotoId,
        original: impl Into<Original<'a>>,
        source: &Identity,
        recipe: &Recipe,
        export: &ExportOptions,
        history: super::HistoryUpdate<'_>,
    ) -> Result<()> {
        self.save_edits(&[EditToSave {
            id,
            original: original.into(),
            source,
            recipe,
            export,
            history,
        }])
    }
    /// Saves several photos' edits in one transaction: all of them, or none when one
    /// fails (as a Sync to many photos is one change).
    pub fn save_edits(&mut self, edits: &[EditToSave<'_>]) -> Result<()> {
        self.change_edits(&edits.iter().map(EditChange::Save).collect::<Vec<_>>())
    }
    /// Saves or clears several photos' edits in one transaction. Clearing returns a
    /// photo to having no RAWmakase edit: no recipe, spots, masks or History.
    pub fn change_edits(&mut self, changes: &[EditChange<'_, '_>]) -> Result<()> {
        // Checked first: reading files and the stored edits takes no lock.
        let checked = changes
            .iter()
            .map(|change| self.check_edit(change))
            .collect::<Result<Vec<_>>>()?;
        // One time for the whole change, as a Sync to many photos is one edit.
        let edited_at = rawmakase_model::time::now_text();
        let write = |w: &mut super::db::Write<'_>| {
            for change in &checked {
                super::edit_rows::write_edit(w, change, &edited_at)?;
            }
            Ok(())
        };
        // Saving a recipe upgrades an earlier catalog with the write, to the format
        // for Lensfun lenses when the edit or any state of its History names one;
        // clearing stores none.
        if changes.iter().any(|c| matches!(c, EditChange::Save(_))) {
            let recipes = changes.iter().flat_map(|c| match c {
                EditChange::Save(e) => {
                    let history = match e.history {
                        super::HistoryUpdate::Replace(h) => Some(h),
                        super::HistoryUpdate::Keep => None,
                    };
                    std::iter::once(e.recipe)
                        .chain(history.map(|h| &h.origin))
                        .chain(
                            history
                                .into_iter()
                                .flat_map(|h| h.steps.iter().map(|s| &s.recipe)),
                        )
                        .collect::<Vec<_>>()
                }
                EditChange::Clear(_) => Vec::new(),
            });
            self.write_recipes(super::format_upgrade::Stores::of(recipes), write)?;
        } else {
            self.db.write(write)?;
        }
        checked.iter().for_each(|change| change.committed());
        Ok(())
    }
    /// The photo's spots and masks, saved apart from its recipe.
    fn local_edits(&self, id: PhotoId) -> Result<crate::model::recipe::LocalEdits> {
        local_edits(local_text(&self.db, id)?.as_deref())
    }
    /// The photo's saved RAWmakase edit, if it has one; an error when it can't be
    /// read or its original ([`Original`]) changed since it was saved.
    pub fn load_edit<'a>(
        &self,
        id: PhotoId,
        original: impl Into<Original<'a>>,
    ) -> Result<Option<SavedEdit>> {
        self.edit_record(id)?.saved(original)
    }
    /// When each edited photo was last edited, as "YYYY-MM-DD HH:MM:SS"
    /// UTC: in RAWmakase, or else in Lightroom, whose history counts seconds
    /// from 2001.
    pub fn edit_times(&self) -> Result<std::collections::HashMap<PhotoId, String>> {
        row! {
            struct Edited {
                id: PhotoId,
                edited: Option<String>,
                created: Option<f64>,
            }
        }
        let rows: Vec<Edited> = self.db.read(
            sql!(
                "SELECT p.id, p.edited_at,
                     (SELECT MAX(h.created) FROM lightroom_history h WHERE h.photo = p.id)
                 FROM photos p
                 WHERE p.edited_at IS NOT NULL
                    OR EXISTS (SELECT 1 FROM lightroom_history h
                               WHERE h.photo = p.id AND h.created IS NOT NULL)"
            ),
            &[],
        )?;
        rows.into_iter()
            .map(|row| {
                let time = row
                    .edited
                    .or_else(|| row.created.and_then(lightroom_time))
                    .context("Lightroom edit time out of range")?;
                Ok((row.id, time))
            })
            .collect()
    }
    /// Changes whenever the photo's edit does: a hash of its recipe, its
    /// spots and masks, and its Lightroom settings. Cheaper than reading
    /// the edit itself, for previews to notice an edit saved elsewhere.
    pub fn edit_stamp(&self, id: PhotoId) -> Result<u64> {
        use std::hash::{Hash, Hasher};
        row! {
            struct Texts {
                recipe: Option<String>,
                lightroom: Option<String>,
                local: Option<String>,
            }
        }
        let texts: Texts = self.db.read_one(
            sql!(
                "SELECT recipe, lightroom_develop,
                 (SELECT data FROM local_edits WHERE photo=photos.id) FROM photos WHERE id=?"
            ),
            &[&id],
        )?;
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        [texts.recipe, texts.lightroom, texts.local].hash(&mut hasher);
        Ok(hasher.finish())
    }
    /// Whether the photo has an edit of its own: a saved recipe or Lightroom
    /// settings. Cheaper than reading either.
    pub fn has_edit(&self, id: PhotoId) -> Result<bool> {
        self.db.read_one(
            sql!(
                "SELECT recipe IS NOT NULL OR COALESCE(lightroom_develop, '') <> '' FROM photos WHERE id=?"
            ),
            &[&id],
        )
    }
    /// The saved RAWmakase recipe (JSON, with its spots and masks) and Lightroom
    /// develop text, if any.
    pub fn edit_texts(&self, id: PhotoId) -> Result<(Option<String>, Option<String>)> {
        row! {
            struct Texts {
                recipe: Option<String>,
                lightroom: Option<String>,
            }
        }
        let texts: Texts = self.db.read_one(
            sql!("SELECT recipe, lightroom_develop FROM photos WHERE id=?"),
            &[&id],
        )?;
        let local = self.local_edits(id)?;
        let recipe = match texts.recipe {
            Some(text) if !local.is_empty() => {
                let recipe: Recipe = serde_json::from_str(&text)?;
                Some(serde_json::to_string(&recipe.with_local(local))?)
            }
            other => other,
        };
        Ok((recipe, texts.lightroom))
    }
}

/// Stores a compressed bitmap under `hash`, in `w`'s transaction, unless
/// one is there already.
pub(super) fn insert_bitmap(w: &mut Write<'_>, hash: &str, data: &[u8]) -> Result<()> {
    w.execute(
        sql!("INSERT INTO bitmaps(hash, data) VALUES (?, ?) ON CONFLICT DO NOTHING"),
        &[&hash, &data],
    )?;
    Ok(())
}

/// The photo's spots and masks as stored, unread.
pub(super) fn local_text(db: &impl Reads, id: PhotoId) -> Result<Option<String>> {
    db.read_optional(sql!("SELECT data FROM local_edits WHERE photo=?"), &[&id])
}

/// A Lightroom history time, seconds since 2001 (fractions allowed), as
/// catalogs store times. Rounds and limits as SQLite's
/// `datetime(created + 978307200, 'unixepoch')` did: to the millisecond,
/// then down to the second, and `None` before 4714 BC or after 9999.
pub(super) fn lightroom_time(created: f64) -> Option<String> {
    /// Unix time of 2001-01-01, Lightroom's epoch.
    const EPOCH: f64 = 978_307_200.;
    /// Milliseconds from the Julian day epoch (4714 BC) to 1970, and to the
    /// end of 9999.
    const UNIX_JD_MS: f64 = 210_866_760_000_000.;
    const MAX_JD_MS: f64 = 464_269_060_800_000.;
    let julian_ms = (created + EPOCH) * 1000. + UNIX_JD_MS;
    if !(0. ..MAX_JD_MS).contains(&julian_ms) {
        return None;
    }
    let unix_ms = (julian_ms + 0.5) as i64 - UNIX_JD_MS as i64;
    Some(rawmakase_model::time::utc_text(unix_ms.div_euclid(1000)))
}
