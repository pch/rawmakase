//! Collections, collection sets and their photos, as Lightroom keeps them:
//! the one owner of the `collections`, `collection_photos`,
//! `collection_options` and `smart_collection_rules` rows, the keywords'
//! Lightroom synonyms (`keyword_synonyms`, which smart collections' keyword
//! rules match) and whether they are known, the target collection and the
//! Quick Collection.
//!
//! Rows keep Lightroom's `creationId` as their kind, so a catalog changed here
//! still reads in older releases. Those releases take any plain collection
//! named exactly [`QUICK_COLLECTION`] for the Quick Collection, so no row
//! made or renamed here is ever given that name.
//!
//! Collection ids are never reused while the catalog is open or after: a
//! deleted collection's id stays above `meta.collection_id_high`, so Undo can
//! restore it under the same id ([`Catalog::restore_collections`]).
use super::db::{Reads, Write, sql};
use super::value::row;
use super::{Catalog, Collection, CollectionId, CollectionKind, PhotoId};
use crate::catalog::models::QUICK_COLLECTION;
use anyhow::{Context, Result, bail, ensure};
use rawmakase_model::collection_rules::{Group, JSON_FORMAT, Malformed};
use std::collections::{HashMap, HashSet};

/// Lightroom's `creationId` of a collection of photos (the Quick Collection
/// is one).
pub(super) const COLLECTION_KIND: &str = "com.adobe.ag.library.collection";
/// Lightroom's `creationId` of a collection set.
const SET_KIND: &str = "com.adobe.ag.library.group";
/// Lightroom's `creationId` of a smart collection.
pub(super) const SMART_KIND: &str = "com.adobe.ag.library.smart_collection";
/// The `meta` key above which new collection ids are allocated.
const ID_HIGH: &str = "collection_id_high";
/// The `meta` key holding the target collection; missing means never chosen.
const TARGET: &str = "target_collection";
/// The value [`TARGET`] holds when the Quick Collection was chosen.
const TARGET_QUICK: &str = "quick";

/// Where a photo is in a collection's custom order, as stored: RAWmakase
/// writes decimal numbers, Lightroom wrote `---0`, `---1`… or nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Position(String);

impl Position {
    /// The position stored as `text`.
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// The number it orders by: a decimal number, or Lightroom's `---N`.
    /// `None` for text that is neither; such a photo sorts with the photos
    /// that have no position.
    pub fn number(&self) -> Option<f64> {
        let text = self.0.trim();
        let text = text.strip_prefix("---").unwrap_or(text);
        text.parse::<f64>().ok().filter(|n| n.is_finite())
    }
}

/// A photo of a collection and its place in the collection's custom order.
pub type Member = (PhotoId, Option<Position>);

/// What [`Catalog::change_collection`] did: the photos it added and the ones
/// it took out, each with its position, so Undo and Redo put them back where
/// they were.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MembershipChange {
    pub added: Vec<Member>,
    pub removed: Vec<Member>,
}

/// A collection's sort, as Lightroom stores it: `sort` is its `sortType`
/// (`captureTime`, `positionInCollection` for Custom Order…) and `direction`
/// its `sortDirection` (`ascending` or `descending`). Kept as Lightroom's
/// strings, whatever they are.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CollectionOptions {
    pub sort: String,
    pub direction: String,
}

/// Collections removed with [`Catalog::remove_collections`], with everything
/// needed to restore them as they were: rows with their ids, photos with
/// their positions, and options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RemovedCollections {
    /// Parents before their children.
    rows: Vec<StoredCollection>,
    members: Vec<(CollectionId, PhotoId, Option<Position>)>,
    options: Vec<(CollectionId, CollectionOptions)>,
    rules: Vec<(CollectionId, SmartRules)>,
}

#[derive(Clone, Debug, PartialEq)]
struct StoredCollection {
    id: CollectionId,
    name: String,
    parent: Option<CollectionId>,
    kind: String,
}

impl RemovedCollections {
    /// The removed collections and sets, parents before their children.
    pub fn ids(&self) -> impl Iterator<Item = CollectionId> + '_ {
        self.rows.iter().map(|r| r.id)
    }
    /// Takes photo `id` out of what would be restored, for when the photo
    /// leaves the catalog and its id may be given to another photo.
    pub fn forget_photo(&mut self, id: PhotoId) {
        self.members.retain(|m| m.1 != id);
    }
    /// The removed collections and sets as they were listed, parents before
    /// their children.
    pub fn collections(&self) -> Vec<Collection> {
        self.rows
            .iter()
            .map(|r| Collection {
                id: r.id,
                name: r.name.clone(),
                parent: r.parent,
                kind: CollectionKind::from_lightroom(&r.kind, &r.name),
            })
            .collect()
    }
    /// The smart collections' rules that would be restored.
    pub fn smart_rules(&self) -> impl Iterator<Item = (CollectionId, &SmartRules)> + '_ {
        self.rules.iter().map(|(id, rules)| (*id, rules))
    }
    /// The photos that would be restored, with their collections.
    pub fn members(&self) -> impl Iterator<Item = (CollectionId, PhotoId)> + '_ {
        self.members.iter().map(|m| (m.0, m.1))
    }
}

row! {
    struct KindRow {
        name: String,
        parent: Option<CollectionId>,
        kind: String,
    }
}

impl Catalog {
    pub fn collections(&self) -> Result<Vec<Collection>> {
        row! {
            struct CollectionRow {
                id: CollectionId,
                name: String,
                parent: Option<CollectionId>,
                kind: String,
            }
        }
        let rows: Vec<CollectionRow> = self.db.read(
            sql!("SELECT id, name, parent, kind FROM collections ORDER BY name, id"),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| Collection {
                id: r.id,
                kind: CollectionKind::from_lightroom(&r.kind, &r.name),
                name: r.name,
                parent: r.parent,
            })
            .collect())
    }
    /// Lightroom's Quick Collection: the one imported with the catalog, or a
    /// new one made the same way.
    pub fn quick_collection(&mut self) -> Result<CollectionId> {
        let found = self.db.read_optional(
            sql!("SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL"),
            &[&QUICK_COLLECTION, &COLLECTION_KIND],
        )?;
        if let Some(id) = found {
            return Ok(id);
        }
        self.db
            .write(|w| insert(w, QUICK_COLLECTION, None, COLLECTION_KIND))
    }
    /// Lightroom's Create Collection and Create Collection Set: a new
    /// collection or set (`kind`) named `name`, trimmed, at the top or in set
    /// `parent`. Refused when the name is empty, the Quick Collection's, or
    /// another collection's or set's in the same place.
    pub fn create_collection(
        &mut self,
        kind: CollectionKind,
        name: &str,
        parent: Option<CollectionId>,
    ) -> Result<CollectionId> {
        let kind = match kind {
            CollectionKind::Collection => COLLECTION_KIND,
            CollectionKind::Set => SET_KIND,
            CollectionKind::Smart | CollectionKind::System => {
                bail!("Only collections and collection sets can be created")
            }
        };
        self.db.write(|w| {
            if let Some(parent) = parent {
                ensure_set(w, parent)?;
            }
            let name = usable_name(w, name, parent, None)?;
            insert(w, &name, parent, kind)
        })
    }
    /// Lightroom's Create Smart Collection: a smart collection named `name`,
    /// trimmed, at the top or in set `parent`, with `rules`, in one
    /// transaction, by the naming rules of [`Catalog::create_collection`].
    pub fn create_smart_collection(
        &mut self,
        name: &str,
        parent: Option<CollectionId>,
        rules: &SmartRules,
    ) -> Result<CollectionId> {
        self.db.write(|w| {
            if let Some(parent) = parent {
                ensure_set(w, parent)?;
            }
            let name = usable_name(w, name, parent, None)?;
            let id = insert(w, &name, parent, SMART_KIND)?;
            write_rules(w, id, rules)?;
            Ok(id)
        })
    }
    /// What New Catalog adds, as Lightroom's does: its default smart
    /// collections, in a set named "Smart Collections", with Lightroom's own
    /// text for their rules. Not for a catalog imported from Lightroom, which
    /// has Lightroom's.
    pub fn add_default_smart_collections(&mut self) -> Result<()> {
        use rawmakase_model::collection_rules::{LIGHTROOM_DEFAULTS, LIGHTROOM_DEFAULTS_SET};
        self.db.write(|w| {
            let set = insert(
                w,
                &usable_name(w, LIGHTROOM_DEFAULTS_SET, None, None)?,
                None,
                SET_KIND,
            )?;
            for (name, text) in LIGHTROOM_DEFAULTS {
                let id = insert(w, name, Some(set), SMART_KIND)?;
                write_rules(w, id, &SmartRules::from_lightroom(text))?;
            }
            Ok(())
        })
    }
    /// [`Catalog::create_collection`] of a collection holding `photos`, in
    /// one transaction: made with them all, or not at all.
    pub fn create_collection_of(
        &mut self,
        name: &str,
        parent: Option<CollectionId>,
        photos: &[PhotoId],
    ) -> Result<(CollectionId, MembershipChange)> {
        self.db.write(|w| {
            if let Some(parent) = parent {
                ensure_set(w, parent)?;
            }
            let name = usable_name(w, name, parent, None)?;
            let id = insert(w, &name, parent, COLLECTION_KIND)?;
            let change = change_members(w, id, Adding::Last(photos), &[])?;
            Ok((id, change))
        })
    }
    /// Renames a collection or set to `name`, trimmed, by the rules of
    /// [`Catalog::create_collection`]. The Quick Collection keeps its name.
    pub fn rename_collection(&mut self, id: CollectionId, name: &str) -> Result<()> {
        self.db.write(|w| {
            let row = movable(w, id)?;
            let name = usable_name(w, name, row.parent, Some(id))?;
            w.execute(
                sql!("UPDATE collections SET name=? WHERE id=?"),
                &[&name, &id],
            )?;
            Ok(())
        })
    }
    /// Moves a collection or set to the top (`None`) or into set `parent`,
    /// which can't be the set itself or one inside it. Refused when a
    /// collection or set there has its name. The Quick Collection stays where
    /// it is.
    pub fn move_collection(
        &mut self,
        id: CollectionId,
        parent: Option<CollectionId>,
    ) -> Result<()> {
        self.db.write(|w| {
            let row = movable(w, id)?;
            if let Some(parent) = parent {
                ensure_set(w, parent)?;
                let mut above = Some(parent);
                while let Some(set) = above {
                    ensure!(set != id, "A collection set can't go inside itself");
                    above =
                        w.read_one(sql!("SELECT parent FROM collections WHERE id=?"), &[&set])?;
                }
            }
            if row.parent == parent {
                return Ok(());
            }
            usable_name(w, &row.name, parent, Some(id))?;
            w.execute(
                sql!("UPDATE collections SET parent=? WHERE id=?"),
                &[&parent, &id],
            )?;
            Ok(())
        })
    }
    /// Deletes collections and sets, each set with everything inside it.
    /// Photos stay in the catalog. Returns what [`Catalog::restore_collections`]
    /// needs to put them back. The Quick Collection can't be deleted.
    pub fn remove_collections(&mut self, ids: &[CollectionId]) -> Result<RemovedCollections> {
        self.db.write(|w| remove_rows(w, ids))
    }
    /// Puts back collections [`Catalog::remove_collections`] deleted, under
    /// their ids, with their photos (those still in the catalog), positions
    /// and options. Refused, changing nothing, when one of the ids is in use,
    /// its set is gone or its name is now taken there.
    pub fn restore_collections(&mut self, removed: &RemovedCollections) -> Result<()> {
        self.db.write(|w| restore_rows(w, removed))
    }
    /// Lightroom's Save Quick Collection: a top-level collection `name` of
    /// the Quick Collection's photos, which are taken out of it when
    /// `clear`, all in one transaction. Returns the collection and the
    /// photos taken out, with their positions.
    pub fn save_quick_collection(
        &mut self,
        name: &str,
        clear: bool,
    ) -> Result<(CollectionId, MembershipChange)> {
        self.db.write(|w| {
            let quick: CollectionId = w
                .read_optional(
                    sql!("SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL"),
                    &[&QUICK_COLLECTION, &COLLECTION_KIND],
                )?
                .context("There is no Quick Collection")?;
            let photos: Vec<PhotoId> = w.read(
                sql!("SELECT photo FROM collection_photos WHERE collection=? ORDER BY photo"),
                &[&quick],
            )?;
            let name = usable_name(w, name, None, None)?;
            let id = insert(w, &name, None, COLLECTION_KIND)?;
            change_members(w, id, Adding::Last(&photos), &[])?;
            let cleared = if clear {
                change_members(w, quick, Adding::At(&[]), &photos)?
            } else {
                MembershipChange::default()
            };
            Ok((id, cleared))
        })
    }
    /// Undoes [`Catalog::save_quick_collection`] in one transaction: puts
    /// `cleared` back in the Quick Collection at their positions and deletes
    /// collection `id`, returning what Redo puts back.
    pub fn unsave_quick_collection(
        &mut self,
        id: CollectionId,
        quick: CollectionId,
        cleared: &[Member],
    ) -> Result<RemovedCollections> {
        self.db.write(|w| {
            change_members(w, quick, Adding::At(cleared), &[])?;
            remove_rows(w, &[id])
        })
    }
    /// Redoes [`Catalog::save_quick_collection`] in one transaction: puts the
    /// collection back and takes `cleared` out of the Quick Collection again.
    pub fn resave_quick_collection(
        &mut self,
        removed: &RemovedCollections,
        quick: CollectionId,
        cleared: &[PhotoId],
    ) -> Result<MembershipChange> {
        self.db.write(|w| {
            restore_rows(w, removed)?;
            change_members(w, quick, Adding::At(&[]), cleared)
        })
    }
    /// Adds `add` to and takes `remove` out of a collection, in one
    /// transaction: photos added go after the collection's last, in the
    /// order given. Photos already in it, and photos no longer in the
    /// catalog, are left as they are. Only plain collections take photos:
    /// not sets, smart collections or Lightroom's saved creations. Returns
    /// what changed, with positions, for [`Catalog::put_back_members`].
    pub fn change_collection(
        &mut self,
        collection: CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<MembershipChange> {
        self.db
            .write(|w| change_members(w, collection, Adding::Last(add), remove))
    }
    /// [`Catalog::change_collection`] for Undo and Redo: puts `add` back each
    /// at the position it had, none included, and takes `remove` out.
    pub fn put_back_members(
        &mut self,
        collection: CollectionId,
        add: &[Member],
        remove: &[PhotoId],
    ) -> Result<MembershipChange> {
        self.db
            .write(|w| change_members(w, collection, Adding::At(add), remove))
    }
    /// Every collection's photos, by collection.
    pub fn collection_photos(&self) -> Result<HashMap<CollectionId, HashSet<PhotoId>>> {
        row! {
            struct MemberRow {
                collection: CollectionId,
                photo: PhotoId,
            }
        }
        let mut members: HashMap<_, HashSet<_>> = Default::default();
        let rows: Vec<MemberRow> = self
            .db
            .read(sql!("SELECT collection, photo FROM collection_photos"), &[])?;
        for row in rows {
            members.entry(row.collection).or_default().insert(row.photo);
        }
        Ok(members)
    }
    /// A collection's photos with their positions, by photo id.
    pub fn collection_members(&self, id: CollectionId) -> Result<Vec<Member>> {
        row! {
            struct MemberRow {
                photo: PhotoId,
                position: Option<String>,
            }
        }
        let rows: Vec<MemberRow> = self.db.read(
            sql!("SELECT photo, position FROM collection_photos WHERE collection=? ORDER BY photo"),
            &[&id],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| (r.photo, r.position.map(Position)))
            .collect())
    }
    /// Every collection's sort chosen here or imported, by collection.
    pub fn collection_options(&self) -> Result<HashMap<CollectionId, CollectionOptions>> {
        row! {
            struct OptionsRow {
                collection: CollectionId,
                sort: String,
                direction: String,
            }
        }
        let rows: Vec<OptionsRow> = self.db.read(
            sql!("SELECT collection, sort, direction FROM collection_options"),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| {
                let options = CollectionOptions {
                    sort: r.sort,
                    direction: r.direction,
                };
                (r.collection, options)
            })
            .collect())
    }
    /// Keeps a collection's sort. Always written, even when it is the
    /// default, so a missing row means the sort was never chosen.
    pub fn set_collection_options(
        &mut self,
        id: CollectionId,
        options: &CollectionOptions,
    ) -> Result<()> {
        self.db.write(|w| {
            let row: KindRow = w
                .read_optional(
                    sql!("SELECT name, parent, kind FROM collections WHERE id=?"),
                    &[&id],
                )?
                .context("That collection is gone")?;
            ensure!(
                row.kind != SET_KIND,
                "A collection set has no sort of its own"
            );
            write_options(w, id, options)
        })
    }
    /// The collection B adds photos to, as Lightroom's target collection:
    /// `None` for the Quick Collection, which is also what a deleted target
    /// or one that can't take photos reads as.
    pub fn target_collection(&self) -> Result<Option<CollectionId>> {
        let Some(value) = self.meta(TARGET)? else {
            return Ok(None);
        };
        let Ok(id) = value.parse::<i64>().map(CollectionId) else {
            return Ok(None);
        };
        let row: Option<KindRow> = self.db.read_optional(
            sql!("SELECT name, parent, kind FROM collections WHERE id=?"),
            &[&id],
        )?;
        Ok(row
            .filter(|r| r.kind == COLLECTION_KIND && !is_quick(&r.name, r.parent, &r.kind))
            .map(|_| id))
    }
    /// Whether a target collection was ever chosen in this catalog.
    pub fn has_target_collection(&self) -> Result<bool> {
        Ok(self.meta(TARGET)?.is_some())
    }
    /// Makes `id` the target collection; `None` makes it the Quick
    /// Collection. Only a plain collection can be the target.
    pub fn set_target_collection(&mut self, id: Option<CollectionId>) -> Result<()> {
        self.db.write(|w| {
            let value = match id {
                None => TARGET_QUICK.to_string(),
                Some(id) => {
                    let row: KindRow = w
                        .read_optional(
                            sql!("SELECT name, parent, kind FROM collections WHERE id=?"),
                            &[&id],
                        )?
                        .context("That collection is gone")?;
                    ensure!(
                        row.kind == COLLECTION_KIND,
                        "Only a collection can be the target collection"
                    );
                    if is_quick(&row.name, row.parent, &row.kind) {
                        TARGET_QUICK.to_string()
                    } else {
                        id.0.to_string()
                    }
                }
            };
            super::set_meta(w, TARGET, &value)
        })
    }
}

/// A smart collection's rules as stored: [`Group::to_json`]'s form, and the
/// text Lightroom had when they were imported from it, kept verbatim so a
/// rule this release can't read or evaluate is never lost and a later one
/// can read it again.
#[derive(Clone, Debug, PartialEq)]
pub struct SmartRules {
    /// None when the rules could not be read (Lightroom text the parser
    /// refused, or a stored form of a later release).
    json: Option<String>,
    format: i64,
    lightroom: Option<String>,
}
impl SmartRules {
    /// Rules made here: there is no Lightroom text.
    pub fn new(rules: &Group) -> Self {
        Self {
            json: Some(rules.to_json().to_string()),
            format: JSON_FORMAT,
            lightroom: None,
        }
    }
    /// Lightroom's text (`AgLibraryCollectionContent.content`), read when it
    /// can be and kept either way.
    pub fn from_lightroom(text: &str) -> Self {
        Self {
            json: Group::from_lightroom(text)
                .ok()
                .map(|g| g.to_json().to_string()),
            format: JSON_FORMAT,
            lightroom: Some(text.to_string()),
        }
    }
    /// The rules, or why they can't be read.
    pub fn rules(&self) -> std::result::Result<Group, Malformed> {
        if self.format > JSON_FORMAT {
            return Err(Malformed(
                "These rules were made by a later release of RAWmakase".into(),
            ));
        }
        match &self.json {
            Some(json) => serde_json::from_str(json)
                .map_err(|e| Malformed(e.to_string()))
                .and_then(|json| Group::from_json(&json)),
            None => match &self.lightroom {
                Some(text) => Group::from_lightroom(text),
                None => Err(Malformed("The rules are missing".into())),
            },
        }
    }
    /// The Lightroom text they were imported from.
    pub fn lightroom(&self) -> Option<&str> {
        self.lightroom.as_deref()
    }
}

/// Whether the catalog has its keywords' Lightroom synonyms (`meta`
/// `keyword_synonyms`). A catalog imported from Lightroom before this
/// release, or exported from one, may have keywords but not their synonyms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynonymState {
    Complete,
    Missing,
}
/// The `meta` key of [`SynonymState`].
const SYNONYMS: &str = "keyword_synonyms";

impl Catalog {
    /// Every smart collection's rules, by collection.
    pub fn smart_rules(&self) -> Result<HashMap<CollectionId, SmartRules>> {
        row! {
            struct RulesRow {
                collection: CollectionId,
                rules: String,
                format: i64,
                lightroom: Option<String>,
            }
        }
        let rows: Vec<RulesRow> = self.db.read(
            sql!("SELECT collection, rules, format, lightroom FROM smart_collection_rules"),
            &[],
        )?;
        Ok(rows
            .into_iter()
            .map(|r| (r.collection, stored_rules(r.rules, r.format, r.lightroom)))
            .collect())
    }
    /// Keeps smart collection `id`'s rules.
    pub fn set_smart_rules(&mut self, id: CollectionId, rules: &SmartRules) -> Result<()> {
        self.db.write(|w| {
            let kind: Option<String> =
                w.read_optional(sql!("SELECT kind FROM collections WHERE id=?"), &[&id])?;
            match kind {
                Some(kind) if kind == SMART_KIND => write_rules(w, id, rules),
                Some(_) => bail!("Only a smart collection has rules"),
                None => bail!("That collection is gone"),
            }
        })
    }
    /// Takes smart collection `id`'s rules row away.
    pub fn clear_smart_rules(&mut self, id: CollectionId) -> Result<()> {
        self.db.write(|w| {
            w.execute(
                sql!("DELETE FROM smart_collection_rules WHERE collection=?"),
                &[&id],
            )?;
            Ok(())
        })
    }
    /// Every keyword's Lightroom synonyms, by keyword id.
    pub fn keyword_synonyms(&self) -> Result<HashMap<i64, Vec<String>>> {
        row! {
            struct SynonymRow {
                keyword: i64,
                name: String,
            }
        }
        let rows: Vec<SynonymRow> = self.db.read(
            sql!("SELECT keyword, name FROM keyword_synonyms ORDER BY keyword, name"),
            &[],
        )?;
        let mut out: HashMap<i64, Vec<String>> = HashMap::new();
        for r in rows {
            out.entry(r.keyword).or_default().push(r.name);
        }
        Ok(out)
    }
    /// Whether the keywords' Lightroom synonyms are known. A catalog that
    /// never recorded it was made by an earlier release: missing, unless it
    /// has no keyword (see [`Catalog::open`]).
    pub fn synonym_state(&self) -> Result<SynonymState> {
        Ok(match self.meta(SYNONYMS)?.as_deref() {
            Some("complete") => SynonymState::Complete,
            _ => SynonymState::Missing,
        })
    }
    /// Records whether the keywords' Lightroom synonyms are known: complete
    /// once read from a Lightroom catalog, or when the user says there are
    /// none.
    pub fn set_synonym_state(&mut self, state: SynonymState) -> Result<()> {
        self.db.write(|w| write_synonym_state(w, state))
    }
}

/// Records the synonyms' state of a catalog that has none recorded: an
/// earlier release's. Its keywords can't be told to be Lightroom's or not
/// (their ids carry no provenance, and an exported catalog has no
/// `sources`), so any keyword makes it missing.
pub(super) fn record_synonym_state(w: &mut Write<'_>) -> Result<()> {
    let recorded: Option<String> =
        w.read_optional(sql!("SELECT value FROM meta WHERE key=?"), &[&SYNONYMS])?;
    if recorded.is_some() {
        return Ok(());
    }
    let keyword: Option<i64> = w.read_optional(sql!("SELECT 1 FROM keywords LIMIT 1"), &[])?;
    write_synonym_state(
        w,
        if keyword.is_some() {
            SynonymState::Missing
        } else {
            SynonymState::Complete
        },
    )
}

/// Records that every keyword's Lightroom synonyms are known: a catalog
/// imported from Lightroom by this release.
pub(super) fn record_synonyms_complete(w: &mut Write<'_>) -> Result<()> {
    write_synonym_state(w, SynonymState::Complete)
}

/// Makes Lightroom's target collection (`Adobe_variablesTable`'s
/// `targetCollection`: a collection's id, written as a number, or `qc` for
/// the Quick Collection) the imported catalog's. Anything else, or a
/// collection that can't take photos, leaves the Quick Collection.
pub(super) fn import_target(w: &mut Write<'_>, value: &str) -> Result<()> {
    let value = value.trim();
    if value == "qc" {
        return super::set_meta(w, TARGET, TARGET_QUICK);
    }
    let Some(id) = value
        .parse::<f64>()
        .ok()
        .filter(|n| n.fract() == 0. && *n > 0. && *n < 9e15)
        .map(|n| CollectionId(n as i64))
    else {
        return Ok(());
    };
    let row: Option<KindRow> = w.read_optional(
        sql!("SELECT name, parent, kind FROM collections WHERE id=?"),
        &[&id],
    )?;
    match row {
        Some(r) if r.kind == COLLECTION_KIND && !is_quick(&r.name, r.parent, &r.kind) => {
            super::set_meta(w, TARGET, &id.0.to_string())
        }
        Some(r) if is_quick(&r.name, r.parent, &r.kind) => super::set_meta(w, TARGET, TARGET_QUICK),
        _ => Ok(()),
    }
}

fn write_synonym_state(w: &mut Write<'_>, state: SynonymState) -> Result<()> {
    let value = match state {
        SynonymState::Complete => "complete",
        SynonymState::Missing => "missing",
    };
    super::set_meta(w, SYNONYMS, value)
}

fn stored_rules(json: String, format: i64, lightroom: Option<String>) -> SmartRules {
    SmartRules {
        json: (json != "null").then_some(json),
        format,
        lightroom,
    }
}

fn rules_of(w: &Write<'_>, id: CollectionId) -> Result<Option<SmartRules>> {
    row! {
        struct RulesRow {
            rules: String,
            format: i64,
            lightroom: Option<String>,
        }
    }
    let row: Option<RulesRow> = w.read_optional(
        sql!("SELECT rules, format, lightroom FROM smart_collection_rules WHERE collection=?"),
        &[&id],
    )?;
    Ok(row.map(|r| stored_rules(r.rules, r.format, r.lightroom)))
}

pub(super) fn write_rules(w: &mut Write<'_>, id: CollectionId, rules: &SmartRules) -> Result<()> {
    let json = rules.json.as_deref().unwrap_or("null");
    w.execute(
        sql!(
            "INSERT INTO smart_collection_rules(collection, rules, format, lightroom)
             VALUES (?, ?, ?, ?)
             ON CONFLICT(collection) DO UPDATE SET
                 rules=excluded.rules, format=excluded.format, lightroom=excluded.lightroom"
        ),
        &[&id, &json, &rules.format, &rules.lightroom],
    )?;
    Ok(())
}

/// Whether a row is the Quick Collection, by the rule every release reads it by.
fn is_quick(name: &str, parent: Option<CollectionId>, kind: &str) -> bool {
    name == QUICK_COLLECTION && parent.is_none() && kind == COLLECTION_KIND
}

/// Adds a collection row under a new id, above every id used before.
pub(super) fn insert(
    w: &mut Write<'_>,
    name: &str,
    parent: Option<CollectionId>,
    kind: &str,
) -> Result<CollectionId> {
    let used: Option<i64> = w.read_one(sql!("SELECT max(id) FROM collections"), &[])?;
    let high: Option<String> =
        w.read_optional(sql!("SELECT value FROM meta WHERE key=?"), &[&ID_HIGH])?;
    let high = high.and_then(|h| h.parse::<i64>().ok());
    let id = CollectionId(used.max(high).unwrap_or(0) + 1);
    w.execute(
        sql!("INSERT INTO collections(id, name, parent, kind) VALUES (?, ?, ?, ?)"),
        &[&id, &name, &parent, &kind],
    )?;
    raise_high(w, id.0)?;
    Ok(id)
}

/// Keeps ids up to `id` from being given out again.
fn raise_high(w: &mut Write<'_>, id: i64) -> Result<()> {
    let high: Option<String> =
        w.read_optional(sql!("SELECT value FROM meta WHERE key=?"), &[&ID_HIGH])?;
    if high.and_then(|h| h.parse::<i64>().ok()) < Some(id) {
        super::set_meta(w, ID_HIGH, &id.to_string())?;
    }
    Ok(())
}

/// `name` trimmed, if a collection or set (`except` aside) may have it in
/// `parent`: not empty, not the Quick Collection's, and no sibling's.
fn usable_name(
    w: &Write<'_>,
    name: &str,
    parent: Option<CollectionId>,
    except: Option<CollectionId>,
) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "Enter a name");
    ensure!(
        name != QUICK_COLLECTION,
        "“{QUICK_COLLECTION}” is the Quick Collection's name; choose another"
    );
    ensure_unused(w, name, parent, except)?;
    Ok(name.to_string())
}

/// Refuses `name` when a collection or set other than `except` has it in `parent`.
fn ensure_unused(
    w: &Write<'_>,
    name: &str,
    parent: Option<CollectionId>,
    except: Option<CollectionId>,
) -> Result<()> {
    let except = except.unwrap_or(CollectionId(-1));
    let taken: Option<i64> = match parent {
        Some(parent) => w.read_optional(
            sql!("SELECT 1 FROM collections WHERE parent=? AND name=? AND id<>?"),
            &[&parent, &name, &except],
        )?,
        None => w.read_optional(
            sql!("SELECT 1 FROM collections WHERE parent IS NULL AND name=? AND id<>?"),
            &[&name, &except],
        )?,
    };
    ensure!(taken.is_none(), "That name is already in use.");
    Ok(())
}

/// Refuses anything but a collection set.
fn ensure_set(w: &Write<'_>, id: CollectionId) -> Result<()> {
    let kind: Option<String> =
        w.read_optional(sql!("SELECT kind FROM collections WHERE id=?"), &[&id])?;
    match kind {
        Some(kind) if kind == SET_KIND => Ok(()),
        Some(_) => bail!("Only a collection set can hold collections"),
        None => bail!("That collection set is gone"),
    }
}

/// The row of a collection or set that can be renamed or moved: any but
/// the Quick Collection.
fn movable(w: &Write<'_>, id: CollectionId) -> Result<KindRow> {
    let row: KindRow = w
        .read_optional(
            sql!("SELECT name, parent, kind FROM collections WHERE id=?"),
            &[&id],
        )?
        .context("That collection is gone")?;
    ensure!(
        !is_quick(&row.name, row.parent, &row.kind),
        "The Quick Collection can't be renamed or moved"
    );
    Ok(row)
}

/// The highest position number in a collection, or 0.
fn next_position(w: &Write<'_>, collection: CollectionId) -> Result<i64> {
    let positions: Vec<Option<String>> = w.read(
        sql!("SELECT position FROM collection_photos WHERE collection=?"),
        &[&collection],
    )?;
    let highest = positions
        .into_iter()
        .flatten()
        .filter_map(|p| Position(p).number())
        .fold(0.0_f64, f64::max);
    Ok(highest.floor() as i64)
}

fn options_of(w: &Write<'_>, id: CollectionId) -> Result<Option<CollectionOptions>> {
    row! {
        struct OptionsRow {
            sort: String,
            direction: String,
        }
    }
    let row: Option<OptionsRow> = w.read_optional(
        sql!("SELECT sort, direction FROM collection_options WHERE collection=?"),
        &[&id],
    )?;
    Ok(row.map(|r| CollectionOptions {
        sort: r.sort,
        direction: r.direction,
    }))
}

pub(super) fn write_options(
    w: &mut Write<'_>,
    id: CollectionId,
    options: &CollectionOptions,
) -> Result<()> {
    w.execute(
        sql!(
            "INSERT INTO collection_options(collection, sort, direction) VALUES (?, ?, ?)
             ON CONFLICT(collection) DO UPDATE SET sort=excluded.sort,
                 direction=excluded.direction"
        ),
        &[&id, &options.sort, &options.direction],
    )?;
    Ok(())
}

/// Puts virtual copy `copy` of `source` in the collections Lightroom Classic
/// puts a new copy in (measured in 15.6, #998): the Quick Collection when
/// `source` is in it, and `viewing` when that is a plain collection
/// `source` is in; never `source`'s other collections. Lightroom gives the
/// copy no position in the Quick Collection and `---0` in the viewed one;
/// here it goes right after `source` in both, so a custom order keeps them
/// together (Lightroom's own placement there isn't measured).
pub(super) fn copy_memberships(
    w: &mut Write<'_>,
    source: PhotoId,
    copy: PhotoId,
    viewing: Option<CollectionId>,
) -> Result<()> {
    let quick: Option<CollectionId> = w.read_optional(
        sql!("SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL"),
        &[&QUICK_COLLECTION, &COLLECTION_KIND],
    )?;
    let mut joined: Vec<CollectionId> = quick.into_iter().collect();
    if let Some(viewing) = viewing.filter(|v| Some(*v) != quick) {
        let kind: Option<String> =
            w.read_optional(sql!("SELECT kind FROM collections WHERE id=?"), &[&viewing])?;
        if kind.as_deref() == Some(COLLECTION_KIND) {
            joined.push(viewing);
        }
    }
    for collection in joined {
        let source_position: Option<Option<String>> = w.read_optional(
            sql!("SELECT position FROM collection_photos WHERE collection=? AND photo=?"),
            &[&collection, &source],
        )?;
        let Some(source_position) = source_position else {
            continue;
        };
        let position = match source_position.and_then(|p| Position(p).number()) {
            // Halfway to the next photo's, or one after the last.
            Some(at) => {
                let positions: Vec<Option<String>> = w.read(
                    sql!("SELECT position FROM collection_photos WHERE collection=?"),
                    &[&collection],
                )?;
                let next = positions
                    .into_iter()
                    .flatten()
                    .filter_map(|p| Position(p).number())
                    .filter(|n| *n > at)
                    .fold(None, |least: Option<f64>, n| {
                        Some(least.map_or(n, |l| l.min(n)))
                    });
                Some(match next {
                    Some(next) => ((at + next) / 2.0).to_string(),
                    None => (at.floor() as i64 + 1).to_string(),
                })
            }
            // Without a position, it sorts with the others without one,
            // after its source by id.
            None => None,
        };
        w.execute(
            sql!("INSERT INTO collection_photos(collection, photo, position) VALUES (?, ?, ?)"),
            &[&collection, &copy, &position],
        )?;
    }
    Ok(())
}

/// Photos to add to a collection, and where.
enum Adding<'a> {
    /// After the collection's last photo, in this order.
    Last(&'a [PhotoId]),
    /// Each at its own position, or with none.
    At(&'a [Member]),
}

/// Takes `remove` out of `collection`, then adds `add`.
fn change_members(
    w: &mut Write<'_>,
    collection: CollectionId,
    add: Adding<'_>,
    remove: &[PhotoId],
) -> Result<MembershipChange> {
    let row: KindRow = w
        .read_optional(
            sql!("SELECT name, parent, kind FROM collections WHERE id=?"),
            &[&collection],
        )?
        .context("That collection is gone")?;
    ensure!(
        row.kind == COLLECTION_KIND,
        "{} can't take photos",
        match row.kind.as_str() {
            SET_KIND => "A collection set",
            "com.adobe.ag.library.smart_collection" => "A smart collection",
            _ => "This collection",
        }
    );
    let mut change = MembershipChange::default();
    for photo in remove {
        let position: Option<Option<String>> = w.read_optional(
            sql!("SELECT position FROM collection_photos WHERE collection=? AND photo=?"),
            &[&collection, photo],
        )?;
        if let Some(position) = position {
            w.execute(
                sql!("DELETE FROM collection_photos WHERE collection=? AND photo=?"),
                &[&collection, photo],
            )?;
            change.removed.push((*photo, position.map(Position)));
        }
    }
    let members: Vec<Member> = match add {
        Adding::At(members) => members.to_vec(),
        Adding::Last(photos) => {
            let mut next = next_position(w, collection)?;
            photos
                .iter()
                .map(|photo| {
                    next += 1;
                    (*photo, Some(Position(next.to_string())))
                })
                .collect()
        }
    };
    for (photo, position) in members {
        let text = position.as_ref().map(Position::as_str);
        let added = w.execute(
            sql!(
                "INSERT INTO collection_photos(collection, photo, position)
                 SELECT ?, id, ? FROM photos WHERE id=?
                 ON CONFLICT DO NOTHING"
            ),
            &[&collection, &text, &photo],
        )?;
        if added == 1 {
            change.added.push((photo, position));
        }
    }
    Ok(change)
}

/// Keeps every id collections have now from being given out again, before
/// rows are deleted by something Undo can't restore (Export as Catalog's
/// pruning), so the target collection can't come to name another one.
pub(super) fn reserve_ids(w: &mut Write<'_>) -> Result<()> {
    let used: Option<i64> = w.read_one(sql!("SELECT max(id) FROM collections"), &[])?;
    match used {
        Some(used) => raise_high(w, used),
        None => Ok(()),
    }
}

/// [`Catalog::remove_collections`] in a transaction already open.
fn remove_rows(w: &mut Write<'_>, ids: &[CollectionId]) -> Result<RemovedCollections> {
    row! {
        struct TreeRow {
            id: CollectionId,
            name: String,
            parent: Option<CollectionId>,
            kind: String,
        }
    }
    let all: Vec<TreeRow> = w.read(
        sql!("SELECT id, name, parent, kind FROM collections ORDER BY id"),
        &[],
    )?;
    let mut children: HashMap<Option<CollectionId>, Vec<&TreeRow>> = HashMap::new();
    let by_id: HashMap<CollectionId, &TreeRow> = all.iter().map(|r| (r.id, r)).collect();
    for row in &all {
        children.entry(row.parent).or_default().push(row);
    }
    let mut removed = RemovedCollections::default();
    let mut seen = HashSet::new();
    for id in ids {
        let row = by_id.get(id).context("That collection is gone")?;
        ensure!(
            !is_quick(&row.name, row.parent, &row.kind),
            "The Quick Collection can't be deleted"
        );
        // A set and something inside it, both chosen: the set takes it.
        let mut above = row.parent;
        let mut inside_chosen = false;
        while let Some(parent) = above {
            inside_chosen |= ids.contains(&parent);
            above = by_id.get(&parent).and_then(|r| r.parent);
        }
        if inside_chosen || !seen.insert(*id) {
            continue;
        }
        // Parents before their children.
        let mut queue = std::collections::VecDeque::from([*row]);
        while let Some(row) = queue.pop_front() {
            removed.rows.push(StoredCollection {
                id: row.id,
                name: row.name.clone(),
                parent: row.parent,
                kind: row.kind.clone(),
            });
            queue.extend(children.get(&Some(row.id)).into_iter().flatten().copied());
        }
    }
    // Ids stay taken: Undo restores them.
    if let Some(highest) = all.iter().map(|r| r.id.0).max() {
        raise_high(w, highest)?;
    }
    row! {
        struct MemberRow {
            photo: PhotoId,
            position: Option<String>,
        }
    }
    for row in &removed.rows {
        let members: Vec<MemberRow> = w.read(
            sql!(
                "SELECT photo, position FROM collection_photos WHERE collection=?
                     ORDER BY photo"
            ),
            &[&row.id],
        )?;
        removed.members.extend(
            members
                .into_iter()
                .map(|m| (row.id, m.photo, m.position.map(Position))),
        );
        if let Some(options) = options_of(w, row.id)? {
            removed.options.push((row.id, options));
        }
        if let Some(rules) = rules_of(w, row.id)? {
            removed.rules.push((row.id, rules));
        }
    }
    // Children before their parents.
    for row in removed.rows.iter().rev() {
        w.execute(
            sql!("DELETE FROM collection_photos WHERE collection=?"),
            &[&row.id],
        )?;
        w.execute(
            sql!("DELETE FROM collection_options WHERE collection=?"),
            &[&row.id],
        )?;
        w.execute(
            sql!("DELETE FROM smart_collection_rules WHERE collection=?"),
            &[&row.id],
        )?;
        w.execute(sql!("DELETE FROM collections WHERE id=?"), &[&row.id])?;
    }
    Ok(removed)
}

/// [`Catalog::restore_collections`] in a transaction already open.
fn restore_rows(w: &mut Write<'_>, removed: &RemovedCollections) -> Result<()> {
    for row in &removed.rows {
        let taken: Option<i64> =
            w.read_optional(sql!("SELECT 1 FROM collections WHERE id=?"), &[&row.id])?;
        ensure!(taken.is_none(), "That collection is already there");
        if let Some(parent) = row.parent {
            ensure_set(w, parent).context("The collection set it was in is gone")?;
        }
        // Restored as it was, even a set's `quick collection` an
        // imported Lightroom catalog had; a name taken since refuses it.
        ensure_unused(w, &row.name, row.parent, None)?;
        w.execute(
            sql!("INSERT INTO collections(id, name, parent, kind) VALUES (?, ?, ?, ?)"),
            &[&row.id, &row.name, &row.parent, &row.kind],
        )?;
    }
    for (collection, photo, position) in &removed.members {
        let position = position.as_ref().map(Position::as_str);
        w.execute(
            sql!(
                "INSERT INTO collection_photos(collection, photo, position)
                     SELECT ?, id, ? FROM photos WHERE id=?"
            ),
            &[collection, &position, photo],
        )?;
    }
    for (collection, options) in &removed.options {
        write_options(w, *collection, options)?;
    }
    for (collection, rules) in &removed.rules {
        write_rules(w, *collection, rules)?;
    }
    Ok(())
}
