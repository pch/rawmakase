//! Each photo's own UUID ([`PhotoUuid`]), in `photo_uuids`: made once for a
//! photo or virtual copy, never changed or given to another, for what an
//! extension keeps or gets back after a session, where [`PhotoId`]s may be
//! reused.
//!
//! Only this crate stores one. Each insert into `photos` gives its photo
//! one in the same transaction ([`give`]); photos a release before it adds
//! get theirs the next time this release opens the catalog ([`prepare`]),
//! and until then reads report them missing. A UUID belongs to its row, as
//! the edit does, so moving file data between rows (Set Copy as Master)
//! leaves it. Two equal UUIDs are not handled: a random one never repeats in
//! practice, and the `UNIQUE` constraint fails the write if it does.
use super::db::{Db, Reads, Write, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::ids::PhotoUuid;
use anyhow::Result;

impl Catalog {
    /// Photo `id`'s UUID; `None` when it is no photo or has none yet (an older
    /// release added it while this catalog was open).
    pub fn photo_uuid(&self, id: PhotoId) -> Result<Option<PhotoUuid>> {
        self.db
            .read_optional(sql!("SELECT uuid FROM photo_uuids WHERE photo=?"), &[&id])
    }
    /// The UUID of each of `ids`, in order and read together, as
    /// [`Catalog::photo_uuid`] gives it.
    pub fn photo_uuids(&self, ids: &[PhotoId]) -> Result<Vec<Option<PhotoUuid>>> {
        self.db.snapshot(|s| {
            ids.iter()
                .map(|id| {
                    s.read_optional(sql!("SELECT uuid FROM photo_uuids WHERE photo=?"), &[id])
                })
                .collect()
        })
    }
    /// The photo whose UUID is `uuid`, for an answer that comes back later:
    /// `None` once it is removed. In a copy of the catalog it is the copy's
    /// photo.
    pub fn photo_by_uuid(&self, uuid: &PhotoUuid) -> Result<Option<PhotoId>> {
        self.db
            .read_optional(sql!("SELECT photo FROM photo_uuids WHERE uuid=?"), &[uuid])
    }
}

/// Gives new photo `photo` a UUID of its own, in `w`'s transaction.
pub(super) fn give(w: &mut Write<'_>, photo: PhotoId) -> Result<()> {
    w.execute(
        sql!("INSERT INTO photo_uuids(photo, uuid) VALUES (?, ?)"),
        &[&photo, &random()?],
    )?;
    Ok(())
}

/// Gives every photo without a UUID one, in `w`'s transaction.
pub(super) fn give_missing(w: &mut Write<'_>) -> Result<()> {
    let missing: Vec<PhotoId> = w.read(
        sql!(
            "SELECT p.id FROM photos p LEFT JOIN photo_uuids u ON u.photo=p.id
             WHERE u.photo IS NULL ORDER BY p.id"
        ),
        &[],
    )?;
    for photo in missing {
        give(w, photo)?;
    }
    Ok(())
}

/// Gives the photos releases before it added their UUIDs, as the catalog
/// opens. Usually every photo has one, and nothing is written.
pub(super) fn prepare(db: &mut Db) -> Result<()> {
    row! {
        struct Counts {
            photos: i64,
            uuids: i64,
        }
    }
    // A row per photo at most (its key, and the cascade that removes it
    // with its photo in every release), so equal counts mean none is
    // missing: much faster than looking for one, and an open is often
    // a background task's.
    let counts: Counts = db.read_one(
        sql!("SELECT (SELECT count(*) FROM photos), (SELECT count(*) FROM photo_uuids)"),
        &[],
    )?;
    if counts.photos == counts.uuids {
        return Ok(());
    }
    // Locked from the start and looked for again: another open may have
    // given them while this one waited.
    db.write_immediate(give_missing)
}

/// A random (version 4) UUID, lowercase and hyphenated.
fn random() -> Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    Ok(format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    ))
}
