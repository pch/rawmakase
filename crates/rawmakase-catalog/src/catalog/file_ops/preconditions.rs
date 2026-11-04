//! What must still hold of the catalog for an item to go on, captured once
//! when it is planned and checked again before each change: in the
//! transaction that journals it, in the one that updates its rows, and by
//! recovery before anything else. A family changed in between, by another
//! connection, computer or an older release, is reported, never acted on.
use super::super::db::Reads;
use super::super::folder_sync::digest::{FamilyDigest, placement, user_state};
use super::super::locations::{folder_path, logical_location};
use super::super::{FolderId, PhotoId, RootId};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A family digest, kept in the journal as hexadecimal text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::catalog) struct Digest(pub FamilyDigest);

impl Serialize for Digest {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let hex: String = self.0.iter().map(|b| format!("{b:02x}")).collect();
        s.serialize_str(&hex)
    }
}

impl<'de> Deserialize<'de> for Digest {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let hex = String::deserialize(d)?;
        let mut out = [0u8; 32];
        let ok = hex.len() == 64
            && out.iter_mut().enumerate().all(|(i, b)| {
                hex.get(2 * i..2 * i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
                    .map(|v| *b = v)
                    .is_some()
            });
        if ok {
            Ok(Self(out))
        } else {
            Err(serde::de::Error::custom("not a digest"))
        }
    }
}

/// An item's preconditions, as journalled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct Preconditions {
    /// The family's master.
    pub master: PhotoId,
    /// The rows the item changes: the whole family, or one virtual copy.
    pub rows: Vec<PhotoId>,
    /// Where the family is ([`placement`]).
    pub placement: Digest,
    /// What the user made of it ([`user_state`]).
    pub user_state: Digest,
    /// The master's folder, and where it resolved on this computer, for
    /// items that touch its file.
    pub folder: FolderId,
    pub folder_path: Option<PathBuf>,
    /// The folder a move goes to, and where it resolved.
    pub to: Option<(FolderId, PathBuf)>,
    /// A move's destination the catalog had no folder for when planned:
    /// the catalog's folder there is added, or found, as the rows follow.
    #[serde(default)]
    pub into: Option<Arrival>,
    /// How a move to another drive proves its copies and originals (#573).
    #[serde(default, skip_serializing_if = "super::proof::Proofs::is_default")]
    pub proofs: super::proof::Proofs,
}

/// A logical folder of a root a move goes to that the catalog had no folder
/// for, and where it resolved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::catalog) struct Arrival {
    pub root: RootId,
    pub logical: String,
    pub resolved: PathBuf,
    /// Whether the move added the catalog's folder there, once it did.
    #[serde(default)]
    pub added: bool,
}

/// Why an item can't go on as planned.
pub(in crate::catalog) const CHANGED: &str = "it changed in the catalog since it was planned";
pub(in crate::catalog) const RELOCATED: &str =
    "its folder was located elsewhere since it was planned";

impl Preconditions {
    /// The family of `master` as `db` has it now, with no folders: what a
    /// plan records before adding the folders its change needs. `None`
    /// when `master` is no photo.
    pub(super) fn read(
        db: &impl Reads,
        master: PhotoId,
        rows: Vec<PhotoId>,
    ) -> Result<Option<Self>> {
        let (Some(place), Some(user)) = (placement(db, master)?, user_state(db, master)?) else {
            return Ok(None);
        };
        let folder: FolderId = db.read_one(
            super::super::db::sql!("SELECT folder FROM photos WHERE id=?"),
            &[&master],
        )?;
        Ok(Some(Self {
            master,
            rows,
            placement: Digest(place),
            user_state: Digest(user),
            folder,
            folder_path: None,
            to: None,
            into: None,
            proofs: Default::default(),
        }))
    }

    /// Why the item can't go on, read in `db` on `computer`: its family
    /// changed in the catalog, or a folder it moves from or to resolves
    /// elsewhere now. `None` when everything holds.
    pub(super) fn check(&self, db: &impl Reads, computer: &str) -> Result<Option<&'static str>> {
        if placement(db, self.master)? != Some(self.placement.0)
            || user_state(db, self.master)? != Some(self.user_state.0)
        {
            return Ok(Some(CHANGED));
        }
        if let Some(path) = &self.folder_path
            && folder_path(db, computer, self.folder)?.as_ref() != Some(path)
        {
            return Ok(Some(RELOCATED));
        }
        if let Some((to, path)) = &self.to
            && folder_path(db, computer, *to)?.as_ref() != Some(path)
        {
            return Ok(Some(RELOCATED));
        }
        if let (None, Some(into)) = (&self.to, &self.into)
            && logical_location(db, computer, into.root, &into.logical)?.as_ref()
                != Some(&into.resolved)
        {
            return Ok(Some(RELOCATED));
        }
        Ok(None)
    }
}
