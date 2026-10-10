use super::*;
use crate::catalog::PhotoId;
use crate::metadata::{
    Capture, DEFAULT_LANG, Descriptive, LangAlt, Location, PhotoInfo, TextField, Value,
};
use rusqlite::Connection;

/// A catalog with `n` photos added from a folder, and their ids.
fn catalog(n: usize) -> Result<(tempfile::TempDir, Catalog, Vec<PhotoId>)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for i in 0..n {
        std::fs::write(
            folder.join(format!("image{i}.ARW")),
            format!("synthetic {i}"),
        )?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let ids = cat.photos()?.iter().map(|p| p.id).collect();
    Ok((dir, cat, ids))
}

/// Sets a photo's descriptive overrides as a whole.
fn put(cat: &mut Catalog, id: PhotoId, descriptive: Descriptive) -> Result<()> {
    let mut snapshot = cat.metadata_snapshot(&[id])?.remove(0);
    snapshot.descriptive = descriptive;
    cat.restore_metadata(&[snapshot])
}

fn set(text: &str) -> Option<Value<LangAlt>> {
    Some(Value::Set(LangAlt::new(text)))
}

#[test]
fn editing_the_default_language_keeps_the_others_and_clearing_drops_all() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let id = ids[0];
    assert_eq!(cat.descriptive(id)?, Descriptive::default());
    put(
        &mut cat,
        id,
        Descriptive {
            title: Some(Value::Set(LangAlt(vec![
                (DEFAULT_LANG.into(), "Harbour".into()),
                ("pl-PL".into(), "Port".into()),
            ]))),
            ..Default::default()
        },
    )?;
    cat.set_text(&ids, TextField::Title, "Old harbour")?;
    assert_eq!(
        cat.descriptive(id)?.title,
        Some(Value::Set(LangAlt(vec![
            (DEFAULT_LANG.into(), "Old harbour".into()),
            ("pl-PL".into(), "Port".into()),
        ])))
    );
    cat.set_text(&ids, TextField::Title, "")?;
    assert_eq!(cat.descriptive(id)?.title, Some(Value::Cleared));
    let left: i64 = cat.db_for_tests().query_row(
        "SELECT count(*) FROM photo_text WHERE photo=?",
        [id.0],
        |r| r.get(0),
    )?;
    assert_eq!(left, 0);
    // Typing again sets just the default language.
    cat.set_text(&ids, TextField::Title, "Pier")?;
    assert_eq!(cat.descriptive(id)?.title, set("Pier"));
    Ok(())
}

#[test]
fn languages_keep_their_order_without_a_default() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let langs = LangAlt(vec![
        ("fr".into(), "Bonjour".into()),
        ("en".into(), "Hello".into()),
    ]);
    put(
        &mut cat,
        ids[0],
        Descriptive {
            caption: Some(Value::Set(langs.clone())),
            ..Default::default()
        },
    )?;
    let read = cat.descriptive(ids[0])?.caption;
    assert_eq!(read, Some(Value::Set(langs)));
    let Some(Value::Set(read)) = read else {
        unreachable!()
    };
    assert_eq!(read.default_text(), Some("Bonjour"));
    Ok(())
}

#[test]
fn undoing_a_capture_override_restores_the_sort_key() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    cat.fill_capture_times(&[(ids[0], "2020-01-01T00:00:00.000".into())])?;
    let before = cat.metadata_snapshot(&ids)?;
    cat.set_capture(
        ids[0],
        &Capture {
            captured: "2024-05-01T12:30:15".into(),
            subsec: None,
            offset: None,
        },
    )?;
    cat.restore_metadata(&before)?;
    assert_eq!(cat.photos()?[0].captured, "2020-01-01T00:00:00.000");
    assert_eq!(cat.descriptive(ids[0])?.capture, None);
    Ok(())
}

#[test]
fn undoing_another_field_keeps_a_time_filled_in_since() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let before = cat.metadata_snapshot(&ids)?;
    cat.set_text(&ids, TextField::Title, "Title")?;
    // Read from the file in the background meanwhile.
    cat.fill_capture_times(&[(ids[0], "2020-01-01T00:00:00.000".into())])?;
    cat.restore_metadata(&before)?;
    assert_eq!(cat.photos()?[0].captured, "2020-01-01T00:00:00.000");
    Ok(())
}

#[test]
fn several_keywords_are_added_in_one_go() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(2)?;
    cat.add_keywords(
        &ids,
        &[vec!["Places".into(), "City".into()], vec!["Event".into()]],
    )?;
    for id in &ids {
        assert_eq!(cat.keywords(*id)?.len(), 2);
    }
    // A bad one adds none.
    assert!(
        cat.add_keywords(&ids, &[vec!["New".into()], vec![" ".into()]])
            .is_err()
    );
    assert_eq!(cat.keywords(ids[0])?.len(), 2);
    let made: i64 = cat.db_for_tests().query_row(
        "SELECT count(*) FROM keywords WHERE name='New'",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(made, 0);
    Ok(())
}

#[test]
fn creators_keep_their_order_and_none_clears_the_field() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let names = vec!["Zoë Example".to_string(), "A. Person, Jr.".to_string()];
    cat.set_creators(&ids, &names)?;
    assert_eq!(cat.descriptive(ids[0])?.creator, Some(Value::Set(names)));
    cat.set_creators(&ids, &[" ".into()])?;
    assert_eq!(cat.descriptive(ids[0])?.creator, Some(Value::Cleared));
    Ok(())
}

#[test]
fn batch_edit_then_undo_restores_each_photos_prior_state() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(3)?;
    // No row, set with two languages, cleared.
    put(
        &mut cat,
        ids[1],
        Descriptive {
            caption: Some(Value::Set(LangAlt(vec![
                (DEFAULT_LANG.into(), "Dusk".into()),
                ("de".into(), "Abend".into()),
            ]))),
            ..Default::default()
        },
    )?;
    cat.set_text(&ids[2..], TextField::Caption, "")?;
    let city = cat.keyword_at(&["Places".into(), "City".into()])?;
    cat.add_keyword(&ids[..1], city)?;

    let before = cat.metadata_snapshot(&ids)?;
    cat.set_text(&ids, TextField::Caption, "Same for all")?;
    cat.add_keyword(&ids, city)?;
    cat.clear_location(&ids)?;
    for id in &ids {
        assert_eq!(
            cat.descriptive(*id)?.caption.unwrap(),
            Value::Set(LangAlt(if *id == ids[1] {
                vec![
                    (DEFAULT_LANG.into(), "Same for all".into()),
                    ("de".into(), "Abend".into()),
                ]
            } else {
                vec![(DEFAULT_LANG.into(), "Same for all".into())]
            }))
        );
    }
    cat.restore_metadata(&before)?;
    assert_eq!(cat.metadata_snapshot(&ids)?, before);
    assert_eq!(cat.descriptive(ids[0])?, Descriptive::default());
    assert_eq!(cat.descriptive(ids[2])?.caption, Some(Value::Cleared));
    assert!(cat.keywords(ids[1])?.is_empty());
    Ok(())
}

#[test]
fn virtual_copies_get_their_own_rows_and_lose_only_theirs() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let master = ids[0];
    cat.set_text(&[master], TextField::Copyright, "© Example")?;
    cat.set_capture(
        master,
        &Capture {
            captured: "2024-05-01T12:30:15".into(),
            subsec: Some("120456".into()),
            offset: Some("+02:00".into()),
        },
    )?;
    let located = Descriptive {
        location: Some(Location::At {
            lat: 52.2297,
            lon: 21.0122,
            alt: Some(100.5),
        }),
        ..cat.descriptive(master)?
    };
    put(&mut cat, master, located)?;
    let copy = cat.create_virtual_copy(master)?;
    assert_eq!(cat.descriptive(copy)?, cat.descriptive(master)?);
    // Independent after that, both ways.
    cat.set_text(&[copy], TextField::Copyright, "© Copy")?;
    cat.set_text(&[master], TextField::Title, "Master only")?;
    assert_eq!(cat.descriptive(master)?.copyright, set("© Example"));
    assert_eq!(cat.descriptive(copy)?.title, None);
    // Set Copy as Master moves nothing.
    cat.set_copy_as_master(copy)?;
    assert_eq!(cat.descriptive(copy)?.copyright, set("© Copy"));
    assert_eq!(cat.descriptive(master)?.title, set("Master only"));
    // Removing a copy (the former master now) removes only its rows.
    cat.remove_virtual_copy(master)?;
    for table in super::descriptive::TABLES {
        let left: i64 = cat.db_for_tests().query_row(
            &format!("SELECT count(*) FROM {table} WHERE photo=?"),
            [master.0],
            |r| r.get(0),
        )?;
        assert_eq!(left, 0, "{table}");
    }
    assert_eq!(cat.descriptive(copy)?.copyright, set("© Copy"));
    Ok(())
}

#[test]
fn a_copy_without_a_row_has_none_rather_than_its_masters() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let copy = cat.create_virtual_copy(ids[0])?;
    cat.set_text(&ids, TextField::Caption, "Master caption")?;
    assert_eq!(cat.descriptive(copy)?.caption, None);
    Ok(())
}

#[test]
fn capture_time_keeps_its_digits_and_sorts_the_photo() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    for subsec in ["12", "120456"] {
        let capture = Capture {
            captured: "2019-12-31T23:59:58".into(),
            subsec: Some(subsec.into()),
            offset: None,
        };
        cat.set_capture(ids[0], &capture)?;
        assert_eq!(cat.descriptive(ids[0])?.capture, Some(capture));
        assert_eq!(cat.photos()?[0].captured, "2019-12-31T23:59:58.120");
    }
    Ok(())
}

#[test]
fn refreshing_photo_info_leaves_the_location_alone() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    cat.clear_location(&ids)?;
    cat.fill_photo_info(&[(ids[0], Some(PhotoInfo::default()))])?;
    cat.fill_photo_info(&[(ids[0], None)])?;
    assert_eq!(cat.descriptive(ids[0])?.location, Some(Location::Cleared));
    Ok(())
}

#[test]
fn keywords_are_found_by_path_in_nfc_and_read_with_their_ancestors() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let nfc = "Kraków".to_string();
    let nfd = "Krako\u{301}w".to_string();
    let a = cat.keyword_at(&["Places".into(), "Poland".into(), nfc.clone()])?;
    let b = cat.keyword_at(&["Places".into(), "Poland".into(), nfd])?;
    assert_eq!(a, b);
    // Case is kept, so this is another keyword; commas and "/" are names.
    let c = cat.keyword_at(&["places".into()])?;
    let d = cat.keyword_at(&["Smith, John".into()])?;
    let e = cat.keyword_at(&["AC/DC 🎸".into()])?;
    assert_ne!(c, cat.keyword_at(&["Places".into()])?);
    for k in [a, c, d, e] {
        cat.add_keyword(&ids, k)?;
    }
    let read = cat.keywords(ids[0])?;
    let paths: Vec<Vec<String>> = read.iter().map(|k| k.path.clone()).collect();
    assert_eq!(
        paths,
        [
            vec!["AC/DC 🎸".to_string()],
            vec!["Places".into(), "Poland".into(), nfc],
            vec!["Smith, John".into()],
            vec!["places".into()],
        ]
    );
    // The display string is still one entry per keyword.
    assert!(cat.photos()?[0].keywords.contains("Smith, John"));
    cat.remove_keyword(&ids, d)?;
    assert_eq!(cat.keywords(ids[0])?.len(), 3);
    Ok(())
}

#[test]
fn keyword_lookup_uses_the_first_of_lightroom_duplicates() -> Result<()> {
    let (_dir, mut cat, _) = catalog(0)?;
    cat.db_for_tests().execute_batch(
        "INSERT INTO keywords(id, name, parent) VALUES (7, 'Dup', NULL), (5, 'Dup', NULL);",
    )?;
    assert_eq!(cat.keyword_at(&["Dup".into()])?, 5);
    Ok(())
}

#[test]
fn new_catalogs_are_version_3_and_older_ones_open_as_they_are() -> Result<()> {
    let (dir, cat, _) = catalog(1)?;
    let CatalogLocation::File(path) = cat.location().clone();
    drop(cat);
    let db = Connection::open(&path)?;
    assert_eq!(
        db.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        3
    );
    // A catalog of the first format is read and left at it until it is upgraded.
    db.execute_batch("PRAGMA user_version=1")?;
    drop(db);
    Catalog::open(&path)?;
    assert_eq!(
        Connection::open(&path)?.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0))?,
        1
    );
    drop(dir);
    Ok(())
}

#[test]
fn a_photo_removed_by_an_older_release_takes_its_rows_and_a_reused_id_has_none() -> Result<()> {
    let (_dir, mut cat, ids) = catalog(1)?;
    let copy = cat.create_virtual_copy(ids[0])?;
    cat.set_text(&[copy], TextField::Title, "Copy title")?;
    cat.set_creators(&[copy], &["Someone".into()])?;
    cat.clear_location(&[copy])?;
    let CatalogLocation::File(path) = cat.location().clone();
    drop(cat);
    {
        // 0.1.12's remove_virtual_copy, which knows none of the new tables,
        // then a new copy that reuses the id.
        let db = Connection::open(&path)?;
        db.execute_batch("PRAGMA foreign_keys=ON;")?;
        for table in [
            "local_edits",
            "lightroom_history",
            "photo_keywords",
            "collection_photos",
            "photo_info",
        ] {
            db.execute(&format!("DELETE FROM {table} WHERE photo=?"), [copy.0])?;
        }
        db.execute("DELETE FROM photos WHERE id=?", [copy.0])?;
        db.execute(
            "INSERT INTO photos(id, folder, filename, original_path, master_id)
             SELECT ?, folder, filename, original_path, id FROM photos WHERE id=?",
            [copy.0, ids[0].0],
        )?;
    }
    let cat = Catalog::open(&path)?;
    assert_eq!(cat.descriptive(copy)?, Descriptive::default());
    Ok(())
}
