//! Every write to a photo's edit: its recipe, export options, identity and
//! edit time in `photos`, its spots and masks in `local_edits` and its
//! History in `develop_history` (issue #341). The rules for changing an
//! edit live here, and nowhere else writes those rows; a backend that
//! detected conflicting edits would do it here.
//!
//! Six operations: a checked save or clear ([`write_edit`]), of a photo
//! being added too ([`CheckedNew`]), an opaque copy for virtual copies
//! ([`copy_edit`]), a checked identity refresh for a file found moved
//! ([`refresh_identity`]), removal with a virtual copy ([`delete_edits`]),
//! and the catalog upgrade's migration of every stored edit
//! ([`migrate_edits`]).
use super::db::{Reads, Write, sql};
use super::value::row;
use super::{Catalog, EditChange, HistoryUpdate, PhotoId};
use crate::storage::Identity;
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use anyhow::{Result, ensure};

/// A change to one photo's edit, checked and ready to write. Only
/// [`Catalog::check_edit`] and [`CheckedNew`] make one, so nothing
/// unchecked is written.
pub(super) struct CheckedChange<'a> {
    id: PhotoId,
    save: Option<Checked<'a>>,
}
impl CheckedChange<'_> {
    /// Notes that the change committed: its mask rasters are stored.
    pub(super) fn committed(&self) {
        if let Some(save) = &self.save {
            save.assets.saved();
        }
    }
}

/// A validated edit, as its rows store it.
struct Checked<'a> {
    recipe: String,
    export: String,
    identity: String,
    /// Spots and masks, `None` without any.
    local: Option<String>,
    history: HistoryUpdate<'a>,
    /// The mask rasters the recipe and a replaced History refer to.
    assets: super::mask_assets::Assets,
}

/// The edit of a photo that the same transaction adds, checked as a save is,
/// with no saved edit to replace. Only [`Catalog::check_new_edit`] makes one.
pub(super) struct CheckedNew(Checked<'static>);
impl CheckedNew {
    /// The change that writes it for photo `id`, once the photo is inserted.
    pub(super) fn change(self, id: PhotoId) -> CheckedChange<'static> {
        CheckedChange {
            id,
            save: Some(self.0),
        }
    }
}

impl Catalog {
    /// Checks `change` before it is written: the recipe and export options
    /// are valid, the photo's file is still the one the edit was made for,
    /// and the edit it replaces was saved for that same file.
    pub(super) fn check_edit<'a>(&self, change: &EditChange<'_, 'a>) -> Result<CheckedChange<'a>> {
        let e = match change {
            EditChange::Clear { id } => {
                return Ok(CheckedChange {
                    id: *id,
                    save: None,
                });
            }
            EditChange::Save(e) => e,
        };
        let now = Identity::read(e.path)?;
        ensure!(
            now.same_photo(e.source),
            "{} changed since its edit was made; edit not saved",
            e.path.display()
        );
        // Stored for the file as it is, so a new modification time alone
        // (issue #870) is taken up by the next save.
        let save = self.checked(&now, e.recipe, e.export, e.history)?;
        // Refuse replacing an edit after the underlying source changed.
        let _ = self.load_edit(e.id, e.path)?;
        Ok(CheckedChange {
            id: e.id,
            save: Some(save),
        })
    }
    /// Checks the edit of a photo not yet in the catalog, as `check_edit`
    /// checks a save, for the file `identity` names: the one the edit was
    /// read for, not the file as it is by the time this runs.
    pub(super) fn check_new_edit(
        &self,
        identity: &Identity,
        recipe: &Recipe,
        export: &ExportOptions,
    ) -> Result<CheckedNew> {
        Ok(CheckedNew(self.checked(
            identity,
            recipe,
            export,
            HistoryUpdate::Keep,
        )?))
    }
    /// A valid edit for the file `identity` names, as its rows store it.
    fn checked<'a>(
        &self,
        identity: &Identity,
        recipe: &Recipe,
        export: &ExportOptions,
        history: HistoryUpdate<'a>,
    ) -> Result<Checked<'a>> {
        recipe.validate()?;
        export.validate()?;
        let (saved, local) = recipe.split_local();
        let history_ids = match history {
            HistoryUpdate::Replace(h) => h.mask_asset_ids(),
            HistoryUpdate::Keep => Default::default(),
        };
        let assets = self.assets_of(recipe.mask_asset_ids().chain(history_ids))?;
        Ok(Checked {
            recipe: serde_json::to_string(&saved)?,
            export: serde_json::to_string(export)?,
            identity: serde_json::to_string(identity)?,
            local: (!local.is_empty())
                .then(|| serde_json::to_string(&local))
                .transpose()?,
            history,
            assets,
        })
    }
}

/// Saves or clears one photo's edit, stamping a save with `edited_at`, as
/// catalogs store times ([`rawmakase_model::time::now_text`]); every photo
/// of one change gets the same. Clearing returns the photo to having no
/// RAWmakase edit: no recipe, spots, masks or History.
pub(super) fn write_edit(
    w: &mut Write<'_>,
    change: &CheckedChange<'_>,
    edited_at: &str,
) -> Result<()> {
    let id = &change.id;
    let Some(e) = &change.save else {
        ensure!(
            w.execute(
                sql!(
                    "UPDATE photos SET recipe=NULL,export_options=NULL,identity=NULL,edited_at=NULL
                     WHERE id=?"
                ),
                &[id]
            )? == 1,
            "Unknown photo"
        );
        w.execute(sql!("DELETE FROM local_edits WHERE photo=?"), &[id])?;
        w.execute(sql!("DELETE FROM develop_history WHERE photo=?"), &[id])?;
        return Ok(());
    };
    e.assets.write(w)?;
    ensure!(
        w.execute(
            sql!("UPDATE photos SET recipe=?,export_options=?,identity=?,edited_at=? WHERE id=?"),
            &[&e.recipe, &e.export, &e.identity, &edited_at, id]
        )? == 1,
        "Unknown photo"
    );
    match &e.local {
        None => {
            w.execute(sql!("DELETE FROM local_edits WHERE photo=?"), &[id])?;
        }
        Some(local) => {
            w.execute(
                sql!(
                    "INSERT INTO local_edits(photo, data) VALUES (?, ?)
                     ON CONFLICT(photo) DO UPDATE SET data=excluded.data"
                ),
                &[id, local],
            )?;
        }
    }
    match e.history {
        HistoryUpdate::Keep => {}
        // An empty History stores as none (Sync's Undo on a photo that had none).
        HistoryUpdate::Replace(h) if h.steps.is_empty() => {
            w.execute(sql!("DELETE FROM develop_history WHERE photo=?"), &[id])?;
        }
        HistoryUpdate::Replace(h) => {
            w.execute(
                sql!(
                    "INSERT INTO develop_history(photo, data) VALUES (?, ?)
                     ON CONFLICT(photo) DO UPDATE SET data=excluded.data"
                ),
                &[id, &super::develop_history::encode(h)?],
            )?;
        }
    }
    Ok(())
}

/// Gives the new photo `to` the edit of `from` exactly as stored: not
/// checked, not read from the file and not restamped, so an offline photo
/// can be copied, the copy keeps its master's edit time, and a History
/// this release can't read survives unchanged. `to` has none yet.
pub(super) fn copy_edit(w: &mut Write<'_>, from: PhotoId, to: PhotoId) -> Result<()> {
    ensure!(
        w.execute(
            sql!(
                "UPDATE photos SET (recipe, export_options, identity, edited_at) =
                 (SELECT recipe, export_options, identity, edited_at FROM photos WHERE id=?)
                 WHERE id=?"
            ),
            &[&from, &to]
        )? == 1,
        "Unknown photo"
    );
    w.execute(
        sql!("INSERT INTO local_edits(photo, data) SELECT ?, data FROM local_edits WHERE photo=?"),
        &[&to, &from],
    )?;
    // The copy starts with the History of the edit it copies, then goes its own way.
    w.execute(
        sql!(
            "INSERT INTO develop_history(photo, data)
             SELECT ?, data FROM develop_history WHERE photo=?"
        ),
        &[&to, &from],
    )?;
    Ok(())
}

/// What [`refresh_identity`] did to a photo's edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Refresh {
    /// The photo has no saved edit.
    NoEdit,
    /// Its edit now names the file found.
    Done,
    /// Its edit was saved for another version of the file: it stays
    /// protected, as before the move.
    Protected,
}

/// Gives the saved edit of `photo`, whose file was found moved, the identity
/// of the file found (`now`), only when that edit was saved for the photo
/// `was` names ([`Identity::same_photo`]) and the file found was verified to
/// have that version's content. An edit saved for another photo stays bound
/// to it.
pub(super) fn refresh_identity(
    w: &mut Write<'_>,
    photo: PhotoId,
    was: &Identity,
    now: &Identity,
) -> Result<Refresh> {
    let saved: Option<Option<String>> =
        w.read_optional(sql!("SELECT identity FROM photos WHERE id=?"), &[&photo])?;
    let Some(Some(saved)) = saved else {
        return Ok(Refresh::NoEdit);
    };
    if !serde_json::from_str::<Identity>(&saved).is_ok_and(|saved| saved.same_photo(was)) {
        return Ok(Refresh::Protected);
    }
    w.execute(
        sql!("UPDATE photos SET identity=? WHERE id=?"),
        &[&serde_json::to_string(now)?, &photo],
    )?;
    Ok(Refresh::Done)
}

/// Removes a photo's spots, masks and History, before the photo itself goes.
pub(super) fn delete_edits(w: &mut Write<'_>, photo: PhotoId) -> Result<()> {
    w.execute(sql!("DELETE FROM local_edits WHERE photo=?"), &[&photo])?;
    w.execute(sql!("DELETE FROM develop_history WHERE photo=?"), &[&photo])?;
    Ok(())
}

/// Removes the settings that chose an engine or operator
/// (`saved_format::OBSOLETE_SETTINGS`) from every stored edit and History, in the
/// catalog's upgrade. Everything else stays as stored, edit times included; rows
/// this release cannot read are left as they are.
pub(super) fn migrate_edits(w: &mut Write<'_>) -> Result<()> {
    use crate::model::saved_format::recipe_text_without_obsolete_settings;
    row! {
        struct Edit {
            id: PhotoId,
            recipe: String,
        }
    }
    let edits: Vec<Edit> = w.read(
        sql!("SELECT id, recipe FROM photos WHERE recipe IS NOT NULL"),
        &[],
    )?;
    for edit in edits {
        if let Some(recipe) = recipe_text_without_obsolete_settings(&edit.recipe) {
            w.execute(
                sql!("UPDATE photos SET recipe=? WHERE id=?"),
                &[&recipe, &edit.id],
            )?;
        }
    }
    row! {
        struct History {
            photo: PhotoId,
            data: Vec<u8>,
        }
    }
    let histories: Vec<History> = w.read(sql!("SELECT photo, data FROM develop_history"), &[])?;
    for history in histories {
        if let Some(data) = super::develop_history::without_obsolete_settings(&history.data) {
            w.execute(
                sql!("UPDATE develop_history SET data=? WHERE photo=?"),
                &[&data, &history.photo],
            )?;
        }
    }
    Ok(())
}
