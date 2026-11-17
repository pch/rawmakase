//! Collections, collection sets and their photos, as Lightroom keeps them.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, Collection, CollectionId, CollectionKind, PhotoId, models};
use anyhow::Result;

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
            sql!("SELECT id, name, parent, kind FROM collections ORDER BY name"),
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
        const KIND: &str = "com.adobe.ag.library.collection";
        let found = self.db.read_optional(
            sql!("SELECT id FROM collections WHERE name=?1 AND kind=?2 AND parent IS NULL"),
            &[&models::QUICK_COLLECTION, &KIND],
        )?;
        if let Some(id) = found {
            return Ok(id);
        }
        self.db.write(|w| {
            w.insert_returning_id(
                sql!(
                    "INSERT INTO collections(name, parent, kind) VALUES (?, NULL, ?) RETURNING id"
                ),
                &[&models::QUICK_COLLECTION, &KIND],
            )
        })
    }
    /// Adds `add` to and removes `remove` from a collection, in one transaction.
    pub fn change_collection(
        &mut self,
        collection: CollectionId,
        add: &[PhotoId],
        remove: &[PhotoId],
    ) -> Result<()> {
        self.db.write(|w| {
            for photo in add {
                w.execute(
                    sql!(
                        "INSERT INTO collection_photos(collection, photo) VALUES (?, ?)
                         ON CONFLICT DO NOTHING"
                    ),
                    &[&collection, photo],
                )?;
            }
            for photo in remove {
                w.execute(
                    sql!("DELETE FROM collection_photos WHERE collection=? AND photo=?"),
                    &[&collection, photo],
                )?;
            }
            Ok(())
        })
    }
    /// Every collection's photos, by collection.
    pub fn collection_photos(
        &self,
    ) -> Result<std::collections::HashMap<CollectionId, std::collections::HashSet<PhotoId>>> {
        row! {
            struct Member {
                collection: CollectionId,
                photo: PhotoId,
            }
        }
        let mut members: std::collections::HashMap<_, std::collections::HashSet<_>> =
            Default::default();
        let rows: Vec<Member> = self
            .db
            .read(sql!("SELECT collection, photo FROM collection_photos"), &[])?;
        for row in rows {
            members.entry(row.collection).or_default().insert(row.photo);
        }
        Ok(members)
    }
    #[cfg(test)]
    pub(crate) fn collection_members(
        &self,
        id: CollectionId,
    ) -> Result<std::collections::HashSet<PhotoId>> {
        let members: Vec<PhotoId> = self.db.read(
            sql!("SELECT photo FROM collection_photos WHERE collection=?"),
            &[&id],
        )?;
        Ok(members.into_iter().collect())
    }
}
