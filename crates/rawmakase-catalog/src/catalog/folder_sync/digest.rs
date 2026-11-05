//! A photo family's digest: everything the catalog holds for a master and
//! its virtual copies. Synchronize takes it in the snapshot its scan works
//! from and checks it again as it applies, so a family changed meanwhile
//! (by another computer or an older release, which bump no generation) is
//! left alone. File operations split it in two: where the family is
//! ([`placement`]) and what the user made of it ([`user_state`]).
use super::super::PhotoId;
use super::super::db::{Reads, Row, Sql, sql};
use super::super::value::{FromRow, FromValue, ValueError, ValueRef};
use anyhow::Result;
use sha2::{Digest as _, Sha256};
use std::collections::HashMap;

/// Every table holding rows of a photo, with the statements that read one
/// photo's rows and every row keyed by its photo, written out whole so the
/// portable-SQL check reads them.
/// `family_tables_cover_the_schema` checks it against `schema.sql`.
pub(super) const PHOTO_TABLES: [(&str, Sql, Sql); 17] = [
    (
        "photo_keywords",
        sql!("SELECT * FROM photo_keywords WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_keywords"),
    ),
    (
        "collection_photos",
        sql!("SELECT * FROM collection_photos WHERE photo=?"),
        sql!("SELECT photo, * FROM collection_photos"),
    ),
    (
        "lightroom_history",
        sql!("SELECT * FROM lightroom_history WHERE photo=?"),
        sql!("SELECT photo, * FROM lightroom_history"),
    ),
    (
        "local_edits",
        sql!("SELECT * FROM local_edits WHERE photo=?"),
        sql!("SELECT photo, * FROM local_edits"),
    ),
    (
        "develop_history",
        sql!("SELECT * FROM develop_history WHERE photo=?"),
        sql!("SELECT photo, * FROM develop_history"),
    ),
    (
        "develop_snapshots",
        sql!("SELECT * FROM develop_snapshots WHERE photo=?"),
        sql!("SELECT photo, * FROM develop_snapshots"),
    ),
    (
        "photo_info",
        sql!("SELECT * FROM photo_info WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_info"),
    ),
    (
        "photo_checksums",
        sql!("SELECT * FROM photo_checksums WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_checksums"),
    ),
    (
        "photo_sequences",
        sql!("SELECT * FROM photo_sequences WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_sequences"),
    ),
    (
        "photo_import_facts",
        sql!("SELECT * FROM photo_import_facts WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_import_facts"),
    ),
    (
        "photo_fields",
        sql!("SELECT * FROM photo_fields WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_fields"),
    ),
    (
        "photo_text",
        sql!("SELECT * FROM photo_text WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_text"),
    ),
    (
        "photo_creators",
        sql!("SELECT * FROM photo_creators WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_creators"),
    ),
    (
        "photo_capture",
        sql!("SELECT * FROM photo_capture WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_capture"),
    ),
    (
        "photo_location",
        sql!("SELECT * FROM photo_location WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_location"),
    ),
    (
        "photo_imports",
        sql!("SELECT * FROM photo_imports WHERE photo=?"),
        sql!("SELECT photo, * FROM photo_imports"),
    ),
    (
        "stack_photos",
        sql!("SELECT * FROM stack_photos WHERE photo=?"),
        sql!("SELECT photo, * FROM stack_photos"),
    ),
];

/// SHA-256 of a family's photos and their rows.
pub type FamilyDigest = [u8; 32];

/// One column's value, tagged by class and length-prefixed.
struct Stored(Vec<u8>);
impl FromValue for Stored {
    fn from_value(value: ValueRef<'_>) -> Result<Self, ValueError> {
        let (tag, bytes): (u8, Vec<u8>) = match value {
            ValueRef::Null => (0, Vec::new()),
            ValueRef::Integer(i) => (1, i.to_le_bytes().to_vec()),
            ValueRef::Real(f) => (2, f.to_bits().to_le_bytes().to_vec()),
            ValueRef::Text(t) => (3, t.to_vec()),
            ValueRef::Blob(b) => (4, b.to_vec()),
        };
        let mut out = vec![tag];
        out.extend((bytes.len() as u64).to_le_bytes());
        out.extend(bytes);
        Ok(Self(out))
    }
}
/// Columns `from..` of `row`, as bytes.
fn encode(row: &Row<'_>, from: usize) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    for i in from..row.len() {
        let value: Stored = row.get(i)?;
        out.extend(value.0);
    }
    Ok(out)
}
/// A whole row.
struct Encoded(Vec<u8>);
impl FromRow for Encoded {
    fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self(encode(row, 0)?))
    }
}
/// A row after its photo, which leads it.
struct Keyed(PhotoId, Vec<u8>);
impl FromRow for Keyed {
    fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self(row.get(0)?, encode(row, 1)?))
    }
}
/// A photo's row after its id and master.
struct PhotoRow(PhotoId, Option<PhotoId>, Vec<u8>);
impl FromRow for PhotoRow {
    fn from_row(row: &Row<'_>) -> Result<Self> {
        Ok(Self(row.get(0)?, row.get(1)?, encode(row, 2)?))
    }
}

/// One photo's rows: its own, then each table's, in an order of their own.
#[derive(Default)]
struct PhotoRows {
    photo: Vec<u8>,
    tables: Vec<Vec<Vec<u8>>>,
}
impl PhotoRows {
    fn hash(mut self) -> [u8; 32] {
        let mut sha = Sha256::new();
        sha.update((self.photo.len() as u64).to_le_bytes());
        sha.update(&self.photo);
        self.tables.resize(PHOTO_TABLES.len(), Vec::new());
        for ((name, ..), mut rows) in PHOTO_TABLES.iter().zip(self.tables) {
            rows.sort();
            sha.update(name.as_bytes());
            sha.update((rows.len() as u64).to_le_bytes());
            for row in rows {
                sha.update((row.len() as u64).to_le_bytes());
                sha.update(row);
            }
        }
        sha.finalize().into()
    }
}
/// The digest of a family from its members' hashes, by id.
fn combine(mut members: Vec<(PhotoId, [u8; 32])>) -> FamilyDigest {
    members.sort();
    let mut sha = Sha256::new();
    for (id, hash) in members {
        sha.update(id.0.to_le_bytes());
        sha.update(hash);
    }
    sha.finalize().into()
}

/// The tables background reads write (checksums, photo info, sequences, the
/// EXIF capture time of import facts), which [`user_state`] leaves out so a
/// queued read can't change it.
pub(in crate::catalog) const BACKGROUND_TABLES: [&str; 4] = [
    "photo_checksums",
    "photo_info",
    "photo_sequences",
    "photo_import_facts",
];

/// The `photos` columns [`user_state`] covers: all but where the photo is
/// (`folder`, `filename`, which [`placement`] covers) and `captured`, the
/// sort key background reads fill in.
/// `user_state_covers_the_photo_columns` checks it against `schema.sql` and
/// [`user_state`]'s query.
#[cfg(test)]
pub(in crate::catalog) const USER_COLUMNS: [&str; 14] = [
    "id",
    "original_path",
    "rating",
    "flag",
    "label",
    "format",
    "copy_name",
    "master_id",
    "orientation",
    "lightroom_develop",
    "recipe",
    "export_options",
    "identity",
    "edited_at",
];

/// The family of `master`: the master and every row whose master it is.
/// `None` when `master` is no photo.
fn members(db: &impl Reads, master: PhotoId) -> Result<Option<Vec<PhotoId>>> {
    let members: Vec<PhotoId> = db.read(
        sql!("SELECT id FROM photos WHERE id=? OR master_id=? ORDER BY id"),
        &[&master, &master],
    )?;
    Ok(members.contains(&master).then_some(members))
}

/// Where the family of `master` is: each photo's id, master, folder and
/// filename. `None` when `master` is no photo.
pub(in crate::catalog) fn placement(
    db: &impl Reads,
    master: PhotoId,
) -> Result<Option<FamilyDigest>> {
    if members(db, master)?.is_none() {
        return Ok(None);
    }
    let rows: Vec<Encoded> = db.read(
        sql!(
            "SELECT id, master_id, folder, filename FROM photos
             WHERE id=? OR master_id=? ORDER BY id"
        ),
        &[&master, &master],
    )?;
    let mut sha = Sha256::new();
    for row in rows {
        sha.update((row.0.len() as u64).to_le_bytes());
        sha.update(row.0);
    }
    Ok(Some(sha.finalize().into()))
}

/// What the user made of the family of `master`: [`family`]'s rows without
/// where the photos are and without what only background reads write
/// ([`BACKGROUND_TABLES`], `photos.captured`), so edits, History, metadata,
/// keywords and collections changed by another connection or release are
/// seen while a queued background result changes nothing. `None` when
/// `master` is no photo.
pub(in crate::catalog) fn user_state(
    db: &impl Reads,
    master: PhotoId,
) -> Result<Option<FamilyDigest>> {
    let Some(members) = members(db, master)? else {
        return Ok(None);
    };
    let mut hashes = Vec::new();
    for id in members {
        let photo: Vec<Encoded> = db.read(
            sql!(
                "SELECT id, original_path, rating, flag, label, format, copy_name, master_id,
                    orientation, lightroom_develop, recipe, export_options, identity, edited_at
                 FROM photos WHERE id=?"
            ),
            &[&id],
        )?;
        let mut rows = PhotoRows {
            photo: photo.into_iter().flat_map(|r| r.0).collect(),
            tables: Vec::new(),
        };
        for (name, one, _) in &PHOTO_TABLES {
            if BACKGROUND_TABLES.contains(name) {
                rows.tables.push(Vec::new());
                continue;
            }
            let found: Vec<Encoded> = db.read(*one, &[&id])?;
            rows.tables.push(found.into_iter().map(|r| r.0).collect());
        }
        hashes.push((id, rows.hash()));
    }
    Ok(Some(combine(hashes)))
}

/// The digest of the family of `master` as `db` has it now: its photos (the
/// master and every row whose master it is), by id, and for each every row
/// of every table that holds its rows. `None` when `master` is no photo.
pub(super) fn family(db: &impl Reads, master: PhotoId) -> Result<Option<FamilyDigest>> {
    let members: Vec<PhotoId> = db.read(
        sql!("SELECT id FROM photos WHERE id=? OR master_id=?"),
        &[&master, &master],
    )?;
    if !members.contains(&master) {
        return Ok(None);
    }
    let mut hashes = Vec::new();
    for id in members {
        let photo: Vec<Encoded> = db.read(sql!("SELECT * FROM photos WHERE id=?"), &[&id])?;
        let mut rows = PhotoRows {
            photo: photo.into_iter().flat_map(|r| r.0).collect(),
            tables: Vec::new(),
        };
        for (_, one, _) in &PHOTO_TABLES {
            let found: Vec<Encoded> = db.read(*one, &[&id])?;
            rows.tables.push(found.into_iter().map(|r| r.0).collect());
        }
        hashes.push((id, rows.hash()));
    }
    Ok(Some(combine(hashes)))
}

/// Every family's digest, by master, as `db` has them: what [`family`]
/// gives each, read a table at a time.
pub(super) fn every_family(db: &impl Reads) -> Result<HashMap<PhotoId, FamilyDigest>> {
    let photos: Vec<PhotoRow> = db.read(sql!("SELECT id, master_id, * FROM photos"), &[])?;
    let mut rows: HashMap<PhotoId, PhotoRows> = HashMap::new();
    let mut master_of: HashMap<PhotoId, PhotoId> = HashMap::new();
    for PhotoRow(id, master, photo) in photos {
        master_of.insert(id, master.unwrap_or(id));
        rows.insert(
            id,
            PhotoRows {
                photo,
                tables: vec![Vec::new(); PHOTO_TABLES.len()],
            },
        );
    }
    for (i, (_, _, all)) in PHOTO_TABLES.iter().enumerate() {
        let found: Vec<Keyed> = db.read(*all, &[])?;
        for Keyed(photo, row) in found {
            if let Some(r) = rows.get_mut(&photo) {
                r.tables[i].push(row);
            }
        }
    }
    let mut families: HashMap<PhotoId, Vec<(PhotoId, [u8; 32])>> = HashMap::new();
    for (id, photo) in rows {
        families
            .entry(master_of[&id])
            .or_default()
            .push((id, photo.hash()));
    }
    Ok(families
        .into_iter()
        // A copy whose master is gone has no family of its own.
        .filter(|(master, members)| members.iter().any(|(id, _)| id == master))
        .map(|(master, members)| (master, combine(members)))
        .collect())
}
