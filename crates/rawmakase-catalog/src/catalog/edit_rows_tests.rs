//! A photo's edit changes only through `edit_rows`, and a virtual copy takes
//! its master's edit exactly as stored.
use super::*;
use crate::edits::Original;
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use std::path::Path;
use std::path::PathBuf;

/// The catalog's statements that write a photo's edit columns or rows.
fn writes_an_edit(statement: &str) -> bool {
    // Words as SQL reads them: any whitespace between them is one space.
    let s = statement
        .to_ascii_lowercase()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let edit_column = ["recipe", "export_options", "identity", "edited_at"]
        .iter()
        .any(|c| s.contains(c));
    let photos = (s.contains("update photos") || s.contains("insert into photos")) && edit_column;
    let rows = ["local_edits", "develop_history"].iter().any(|t| {
        s.contains(&format!("insert into {t}"))
            || s.contains(&format!("update {t}"))
            || s.contains(&format!("delete from {t}"))
    });
    photos || rows
}

#[test]
fn only_edit_rows_writes_a_photos_edit() {
    let offenders: Vec<String> = super::sql_scan::catalog_statements()
        .into_iter()
        .filter(|s| s.file != "edit_rows.rs" && writes_an_edit(&s.text))
        .map(|s| format!("{}: {}", s.file, s.text.trim()))
        .collect();
    assert!(
        offenders.is_empty(),
        "write edits through edit_rows: {offenders:#?}"
    );
}

#[test]
fn the_scan_finds_edit_writes() {
    assert!(writes_an_edit("UPDATE photos SET recipe=? WHERE id=?"));
    assert!(writes_an_edit("UPDATE\n    photos SET recipe=? WHERE id=?"));
    assert!(writes_an_edit("DELETE  FROM\tlocal_edits WHERE photo=?"));
    assert!(writes_an_edit("DELETE FROM local_edits WHERE photo=?"));
    assert!(writes_an_edit(
        "INSERT INTO develop_history(photo, data) VALUES (?, ?)"
    ));
    assert!(!writes_an_edit("UPDATE photos SET rating=? WHERE id=?"));
    assert!(!writes_an_edit(
        "UPDATE develop_snapshots SET recipe=? WHERE id=?"
    ));
    assert!(!writes_an_edit("SELECT recipe FROM photos WHERE id=?"));
}

/// A catalog with one photo carrying a saved edit and a History, its file and id.
fn edited() -> Result<(tempfile::TempDir, Catalog, PathBuf, PhotoId)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let file = photos.join("a.dng");
    std::fs::copy(chart, &file)?;
    let mut cat = Catalog::create(&dir.path().join("Copies.rawmakase"))?;
    cat.add_folder(&photos)?;
    let id = cat.photos()?[0].id;
    let recipe = Recipe {
        exposure: 0.5,
        ..Default::default()
    };
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &recipe,
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    Ok((dir, cat, file, id))
}

/// The edit as stored: the photo's edit columns, its spots and masks and
/// its History bytes.
fn stored(cat: &Catalog, id: PhotoId) -> Result<[Option<Vec<u8>>; 6]> {
    let db = cat.db_for_tests();
    let column = |sql: &str| -> rusqlite::Result<Option<Vec<u8>>> {
        db.query_row(sql, [id.0], |r| {
            Ok(match r.get_ref(0)? {
                rusqlite::types::ValueRef::Text(t) | rusqlite::types::ValueRef::Blob(t) => {
                    Some(t.to_vec())
                }
                _ => None,
            })
        })
        .or_else(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => Ok(None),
            e => Err(e),
        })
    };
    Ok([
        column("SELECT recipe FROM photos WHERE id=?")?,
        column("SELECT export_options FROM photos WHERE id=?")?,
        column("SELECT identity FROM photos WHERE id=?")?,
        column("SELECT edited_at FROM photos WHERE id=?")?,
        column("SELECT data FROM local_edits WHERE photo=?")?,
        column("SELECT data FROM develop_history WHERE photo=?")?,
    ])
}

#[test]
fn a_virtual_copy_takes_its_masters_edit_exactly_as_stored() -> Result<()> {
    let (_dir, mut cat, file, master) = edited()?;
    let db = cat.db_for_tests();
    // Saved long ago, with spots and a History no release can read yet.
    db.execute(
        "UPDATE photos SET edited_at='2020-01-02 03:04:05' WHERE id=?",
        [master.0],
    )?;
    db.execute(
        "INSERT INTO local_edits(photo, data) VALUES (?, '{\"spots\":\"from the future\"}')",
        [master.0],
    )?;
    db.execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, X'00FF00FF')",
        [master.0],
    )?;
    // And the photo is offline.
    std::fs::remove_file(&file)?;
    let copy = cat.create_virtual_copy(master)?;
    let (from, to) = (stored(&cat, master)?, stored(&cat, copy)?);
    assert_eq!(to, from);
    assert_eq!(to[3].as_deref(), Some(&b"2020-01-02 03:04:05"[..]));
    assert_eq!(to[5].as_deref(), Some(&[0, 0xff, 0, 0xff][..]));
    Ok(())
}

#[test]
fn removing_a_virtual_copy_removes_its_edit_and_leaves_its_masters() -> Result<()> {
    let (_dir, mut cat, _file, master) = edited()?;
    cat.db_for_tests().execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, X'01')",
        [master.0],
    )?;
    let copy = cat.create_virtual_copy(master)?;
    let kept = stored(&cat, master)?;
    cat.remove_virtual_copy(copy)?;
    assert_eq!(stored(&cat, copy)?, [None, None, None, None, None, None]);
    assert_eq!(stored(&cat, master)?, kept);
    Ok(())
}

#[test]
fn clearing_an_edit_leaves_nothing_of_it() -> Result<()> {
    let (_dir, mut cat, _file, id) = edited()?;
    cat.db_for_tests().execute(
        "INSERT INTO develop_history(photo, data) VALUES (?, X'01')",
        [id.0],
    )?;
    let source = crate::storage::Identity::read(&_file)?;
    cat.change_edits(&[EditChange::Clear(EditToClear {
        id,
        original: _file.as_path().into(),
        source: &source,
    })])?;
    assert_eq!(stored(&cat, id)?, [None, None, None, None, None, None]);
    Ok(())
}

/// The exposure of photo `id`'s saved edit, read against `original`.
fn exposure(cat: &Catalog, id: PhotoId, original: Original<'_>) -> Result<f32> {
    Ok(cat.load_edit(id, original)?.unwrap().recipe.exposure)
}

fn save_exposure(
    cat: &mut Catalog,
    id: PhotoId,
    original: Original<'_>,
    source: &crate::storage::Identity,
    exposure: f32,
) -> Result<()> {
    let recipe = Recipe {
        exposure,
        ..Default::default()
    };
    cat.save_edit(
        id,
        original,
        source,
        &recipe,
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )
}

fn clear(
    cat: &mut Catalog,
    id: PhotoId,
    original: Original<'_>,
    source: &crate::storage::Identity,
) -> Result<()> {
    cat.change_edits(&[EditChange::Clear(EditToClear {
        id,
        original,
        source,
    })])
}

/// The saving rule (docs/editable-previews.md): an edit is saved or cleared
/// against a recorded identity only while nothing is at the original's path.
#[test]
fn a_recorded_identity_saves_and_clears_only_while_the_original_is_absent() -> Result<()> {
    let (_dir, mut cat, file, id) = edited()?;
    let recorded = crate::storage::Identity::read(&file)?;
    let bytes = std::fs::read(&file)?;
    std::fs::remove_file(&file)?;
    let offline = Original::recorded(&file, &recorded);
    // The file alone, as every caller but Develop's preview passes it: refused.
    assert!(save_exposure(&mut cat, id, file.as_path().into(), &recorded, 1.0).is_err());
    assert!(clear(&mut cat, id, file.as_path().into(), &recorded).is_err());
    assert_eq!(exposure(&cat, id, offline)?, 0.5);
    // With the recorded identity: saved, as the original's edit.
    save_exposure(&mut cat, id, offline, &recorded, 1.0)?;
    assert_eq!(exposure(&cat, id, offline)?, 1.0);
    // The same file back: checked against the file, and the edit is its own.
    std::fs::write(&file, &bytes)?;
    assert_eq!(exposure(&cat, id, file.as_path().into())?, 1.0);
    let back = crate::storage::Identity::read(&file)?;
    save_exposure(&mut cat, id, offline, &back, 1.5)?;
    assert_eq!(exposure(&cat, id, file.as_path().into())?, 1.5);
    // Absent again, a clear with the recorded identity goes through.
    std::fs::remove_file(&file)?;
    clear(&mut cat, id, offline, &recorded)?;
    assert!(cat.load_edit(id, offline)?.is_none());
    Ok(())
}

#[test]
fn a_recorded_identity_never_stands_in_for_another_file() -> Result<()> {
    let (_dir, mut cat, file, id) = edited()?;
    let recorded = crate::storage::Identity::read(&file)?;
    std::fs::remove_file(&file)?;
    std::fs::write(&file, b"another photo")?;
    let other = crate::storage::Identity::read(&file)?;
    let offline = Original::recorded(&file, &recorded);
    for source in [&recorded, &other] {
        assert!(save_exposure(&mut cat, id, offline, source, 1.0).is_err());
        assert!(clear(&mut cat, id, offline, source).is_err());
    }
    // The edit stays, protected, for the photo it was saved for.
    assert_eq!(
        cat.edit_record(id)?
            .saved_for(&recorded)?
            .unwrap()
            .recipe
            .exposure,
        0.5
    );
    Ok(())
}

/// A path that can't be looked at isn't absent: a folder without permission
/// refuses saves and clears, recorded identity or not.
#[cfg(unix)]
#[test]
fn a_recorded_identity_never_stands_in_for_an_unreadable_original() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let (_dir, mut cat, file, id) = edited()?;
    let recorded = crate::storage::Identity::read(&file)?;
    let folder = file.parent().unwrap().to_path_buf();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000))?;
    // Root ignores permissions, so there is nothing to test.
    let locked = std::fs::File::open(&file).is_err();
    let offline = Original::recorded(&file, &recorded);
    let saved = save_exposure(&mut cat, id, offline, &recorded, 1.0);
    let cleared = clear(&mut cat, id, offline, &recorded);
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755))?;
    if locked {
        assert!(saved.is_err());
        assert!(cleared.is_err());
        assert_eq!(exposure(&cat, id, file.as_path().into())?, 0.5);
    }
    Ok(())
}

/// A clear is checked as a save is: a stored edit saved for another file is
/// protected from it too.
#[test]
fn a_clear_never_removes_a_protected_edit() -> Result<()> {
    let (_dir, mut cat, file, id) = edited()?;
    std::fs::write(&file, b"another photo")?;
    let other = crate::storage::Identity::read(&file)?;
    assert!(clear(&mut cat, id, file.as_path().into(), &other).is_err());
    assert!(cat.edit_record(id)?.recipe.is_some());
    Ok(())
}

#[test]
fn a_photo_has_an_edit_once_one_is_saved() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let file = photos.join("a.jpg");
    std::fs::write(&file, b"fixture")?;
    let mut cat = Catalog::create(&dir.path().join("Edits.rawmakase"))?;
    cat.add_folder(&photos)?;
    let id = cat.photos()?[0].id;
    assert!(!cat.has_edit(id)?);
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    assert!(cat.has_edit(id)?);
    Ok(())
}

/// An edit is saved for the file it was made for: a file replaced since gets
/// none of it, even with no saved edit there whose identity would protect it.
#[test]
fn a_save_for_a_file_replaced_since_is_refused() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let file = dir.path().join("a.ARW");
    std::fs::write(&file, b"the photo opened")?;
    let mut cat = Catalog::create(&dir.path().join("Replaced.rawmakase"))?;
    cat.add_folder(dir.path())?;
    let id = cat.photos()?[0].id;
    let opened = crate::storage::Identity::read(&file)?;
    std::fs::write(&file, b"another photo, same name")?;
    let recipe = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let save = |cat: &mut Catalog, source| {
        cat.save_edit(
            id,
            &file,
            source,
            &recipe,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )
    };
    let refused = save(&mut cat, &opened).unwrap_err();
    assert!(refused.to_string().contains("changed"), "{refused}");
    assert!(cat.load_edit(id, &file)?.is_none());
    // The file as it is now takes an edit made for it.
    save(&mut cat, &crate::storage::Identity::read(&file)?)?;
    assert_eq!(cat.load_edit(id, &file)?.unwrap().recipe.exposure, 1.);
    Ok(())
}

/// A file another program saved again (Edit In) keeps its edit once followed:
/// before, opening it protects the edit and a save made for the new version
/// is refused; after, the edit loads and saves for the new version.
#[test]
fn an_edit_follows_its_file_rewritten_by_another_program() -> Result<()> {
    let (_dir, mut cat, file, id) = edited()?;
    let was = crate::storage::Identity::read(&file)?;
    let mut bytes = std::fs::read(&file)?;
    bytes[100] ^= 0xff;
    bytes.extend_from_slice(b"saved again");
    std::fs::write(&file, bytes)?;
    let now = crate::storage::Identity::read(&file)?;
    assert!(!was.same_photo(&now));
    assert!(cat.load_edit(id, &file).is_err());
    let recipe = Recipe {
        exposure: 1.,
        ..Default::default()
    };
    let save = |cat: &mut Catalog| {
        cat.save_edit(
            id,
            &file,
            &now,
            &recipe,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )
    };
    let refused = save(&mut cat).unwrap_err();
    assert!(
        refused.is::<rawmakase_model::error::Changed>(),
        "{refused:#}"
    );
    // Another version than the one the edit was saved for leaves it bound.
    assert!(!cat.follow_rewritten_file(id, &now, &now)?);
    assert!(cat.follow_rewritten_file(id, &was, &now)?);
    assert_eq!(cat.load_edit(id, &file)?.unwrap().recipe.exposure, 0.5);
    save(&mut cat)?;
    assert_eq!(cat.load_edit(id, &file)?.unwrap().recipe.exposure, 1.);
    Ok(())
}
