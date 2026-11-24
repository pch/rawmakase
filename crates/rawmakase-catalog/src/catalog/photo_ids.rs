//! New photos' ids: the one owner of `meta.photo_id_high`.
//!
//! A photo's id is never given to another photo, as in Lightroom, because
//! requests that name a photo by id can arrive after it is gone (an
//! extension's proposal, MCP's `photo_ids`) and must not land on a photo that
//! took its id. SQLite alone would give the highest id out again once its photo
//! is removed, so every photo inserted takes an id above both the highest in
//! `photos` and [`ID_HIGH`], and removing a photo raises the mark to the
//! highest id first. The mark only ever goes up. A catalog written before it
//! has none and starts from its highest photo; ids removed before then can't
//! be known. Releases that don't know the mark ignore it and may still reuse an
//! id; taking the larger of the two on each insert keeps that from lowering
//! what this release gives out.
use super::PhotoId;
use super::db::{Reads, Write, sql};
use anyhow::Result;

/// The `meta` key above which new photo ids are allocated.
const ID_HIGH: &str = "photo_id_high";

/// An id for a photo about to be inserted in `w`'s transaction, above every
/// id given before, kept from being given again.
pub(super) fn allocate(w: &mut Write<'_>) -> Result<PhotoId> {
    let used: Option<i64> = w.read_one(sql!("SELECT max(id) FROM photos"), &[])?;
    let id = used.max(high(w)?).unwrap_or(0) + 1;
    super::set_meta(w, ID_HIGH, &id.to_string())?;
    Ok(PhotoId(id))
}

/// Keeps every id photos have now from being given out again: before a
/// photo is removed, and after photos were inserted with ids of their own (a
/// Lightroom import keeps Lightroom's).
pub(super) fn reserve(w: &mut Write<'_>) -> Result<()> {
    let used: Option<i64> = w.read_one(sql!("SELECT max(id) FROM photos"), &[])?;
    if let Some(used) = used
        && high(w)? < Some(used)
    {
        super::set_meta(w, ID_HIGH, &used.to_string())?;
    }
    Ok(())
}

fn high(w: &Write<'_>) -> Result<Option<i64>> {
    let high: Option<String> =
        w.read_optional(sql!("SELECT value FROM meta WHERE key=?"), &[&ID_HIGH])?;
    Ok(high.and_then(|h| h.parse().ok()))
}
