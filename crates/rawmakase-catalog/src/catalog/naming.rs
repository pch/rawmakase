//! What export names photos from: the catalog's part of each photo's
//! `NameFields`, read once when Export is chosen, so the Example and the files
//! named agree whatever changes afterwards.
use super::db::{Reads, sql};
use super::value::row;
use super::{Catalog, PhotoId};
use crate::export_settings::NameFields;
use crate::metadata::{LangAlt, Value};
use anyhow::Result;

impl Catalog {
    /// The name fields of `ids`, in order, read in one transaction. The
    /// camera's make and model are the file's, which the catalog doesn't
    /// keep: they are left unread.
    pub fn name_fields(&self, ids: &[PhotoId]) -> Result<Vec<NameFields>> {
        self.db
            .snapshot(|snapshot| ids.iter().map(|&id| read(snapshot, id)).collect())
    }
}

/// `id`'s name fields, in the snapshot or transaction `db`.
pub(super) fn read(db: &impl Reads, id: PhotoId) -> Result<NameFields> {
    row! {
        struct Photo {
            captured: String,
            copy_name: String,
            rating: i32,
            label: String,
        }
    }
    let photo: Photo = db.read_one(
        sql!("SELECT captured, copy_name, rating, label FROM photos WHERE id = ?"),
        &[&id],
    )?;
    row! {
        struct Keyword {
            name: String,
        }
    }
    let mut keywords: Vec<String> = db
        .read::<Keyword>(
            sql!(
                "SELECT k.name FROM photo_keywords pk JOIN keywords k ON k.id = pk.keyword
                 WHERE pk.photo = ?"
            ),
            &[&id],
        )?
        .into_iter()
        .map(|k| k.name)
        .collect();
    // By name, as Lightroom lists them.
    keywords.sort_by_cached_key(|k| (k.to_lowercase(), k.clone()));
    keywords.dedup();
    // A virtual copy's file is its master's.
    let original_name: Option<String> = db.read_optional(
        sql!(
            "SELECT f.original_name FROM photo_import_facts f
             WHERE f.photo = (SELECT COALESCE(p.master_id, p.id) FROM photos p WHERE p.id = ?)"
        ),
        &[&id],
    )?;
    let d = super::descriptive::read(db, id)?;
    // No row is the file's own (`None`); a cleared one is empty.
    let text = |value: &Option<Value<LangAlt>>| match value {
        Some(Value::Set(langs)) => Some(langs.default_text().unwrap_or_default().to_string()),
        Some(Value::Cleared) => Some(String::new()),
        None => None,
    };
    Ok(NameFields {
        captured: Some(photo.captured).filter(|c| !c.is_empty()),
        copy_name: photo.copy_name,
        original_name,
        info: super::info::read(db, id)?.unwrap_or_default(),
        title: text(&d.title),
        caption: text(&d.caption),
        copyright: text(&d.copyright),
        // As Lightroom names them.
        creator: match &d.creator {
            Some(Value::Set(names)) => Some(names.join(", ")),
            Some(Value::Cleared) => Some(String::new()),
            None => None,
        },
        artist: match &d.creator {
            Some(Value::Set(names)) => Some(names.first().cloned().unwrap_or_default()),
            Some(Value::Cleared) => Some(String::new()),
            None => None,
        },
        keywords: keywords.join(", "),
        rating: photo.rating,
        label: photo.label,
        camera: Default::default(),
        // The app's to say, from the system and the file.
        decimal: Default::default(),
        facts: Default::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::{PhotoInfo, TextField};

    #[test]
    fn a_photos_name_fields_are_the_catalogs() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let folder = dir.path().join("photos");
        std::fs::create_dir(&folder)?;
        for name in ["a.ARW", "b.ARW"] {
            std::fs::write(folder.join(name), b"synthetic")?;
        }
        let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
        cat.add_folder(&folder)?;
        let ids: Vec<PhotoId> = cat.photos()?.iter().map(|p| p.id).collect();
        cat.fill_capture_times(&[(ids[0], "2026-04-24T18:43:24.000".into())])?;
        cat.set_text(&ids[..1], TextField::Title, "Harbour")?;
        cat.set_creators(&ids[..1], &["Ann".into(), "Bo".into()])?;
        let info = PhotoInfo {
            lens: Some("FE 24-70mm".into()),
            iso: Some(400.),
            dimensions: Some((6000, 4000)),
            ..Default::default()
        };
        cat.fill_photo_info(&[(ids[0], Some(info.clone()))])?;
        cat.db_for_tests().execute(
            "UPDATE photos SET rating = 3, label = 'Red' WHERE id = ?",
            rusqlite::params![ids[0].0],
        )?;
        let copy = cat.create_virtual_copy(ids[0])?;
        let fields = cat.name_fields(&[ids[0], ids[1], copy])?;
        assert_eq!(
            fields[0].captured.as_deref(),
            Some("2026-04-24T18:43:24.000")
        );
        assert_eq!(fields[0].info, info);
        assert_eq!(
            (fields[0].title.as_deref(), fields[0].creator.as_deref()),
            (Some("Harbour"), Some("Ann, Bo"))
        );
        assert_eq!(fields[0].artist.as_deref(), Some("Ann"));
        // Keywords by name, case aside, leaf names only, each once.
        cat.add_keywords(
            &ids[..1],
            &[
                vec!["Zebra".into()],
                vec!["Places".into(), "apple".into()],
                vec!["Fruit".into(), "apple".into()],
            ],
        )?;
        assert_eq!(cat.name_fields(&ids[..1])?[0].keywords, "apple, Zebra");
        // No creator row: the file's own Artist; cleared: none.
        assert_eq!(fields[1].artist, None);
        cat.set_creators(&ids[1..2], &[])?;
        assert_eq!(cat.name_fields(&ids[1..2])?[0].artist.as_deref(), Some(""));
        // No row: the file's own; cleared: empty, whatever the file says.
        assert_eq!(fields[0].caption, None);
        cat.set_text(&ids[1..2], TextField::Caption, "")?;
        let cleared = cat.name_fields(&ids[1..2])?;
        assert_eq!(cleared[0].caption.as_deref(), Some(""));
        assert_eq!((fields[0].rating, fields[0].label.as_str()), (3, "Red"));
        assert_eq!(fields[0].camera, Default::default());
        // Nothing known but the name it was added with.
        assert_eq!(
            fields[1],
            NameFields {
                original_name: Some("b.ARW".into()),
                ..NameFields::default()
            }
        );
        // A virtual copy: its own name, its master's info and original name.
        assert_eq!(fields[2].copy_name, "Copy 1");
        assert_eq!(fields[2].info, info);
        assert_eq!(fields[2].original_name.as_deref(), Some("a.ARW"));
        // Renamed, it keeps the name it was added with.
        cat.db_for_tests().execute(
            "UPDATE photos SET filename = 'renamed.ARW' WHERE id = ?",
            rusqlite::params![ids[0].0],
        )?;
        assert_eq!(
            cat.name_fields(&ids[..1])?[0].original_name.as_deref(),
            Some("a.ARW")
        );
        // Added by a release that didn't keep it: not known.
        cat.db_for_tests().execute(
            "DELETE FROM photo_import_facts WHERE photo = ?",
            rusqlite::params![ids[1].0],
        )?;
        assert_eq!(cat.name_fields(&ids[1..2])?[0].original_name, None);
        Ok(())
    }
}
