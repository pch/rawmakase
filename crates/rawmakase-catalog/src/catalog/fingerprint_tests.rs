//! The photo fingerprint and the conditional edit it guards.
use super::*;
use crate::metadata::TextField;
use crate::{export_settings::ExportOptions, model::recipe::Recipe, storage::Identity};

/// A catalog's photos: their ids and files.
type Photos = Vec<(PhotoId, std::path::PathBuf)>;

fn catalog(n: usize) -> Result<(tempfile::TempDir, Catalog, Photos)> {
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
    let photos = cat
        .photos()?
        .iter()
        .map(|p| (p.id, folder.join(&p.filename)))
        .collect();
    Ok((dir, cat, photos))
}

fn recipe(exposure: f32) -> Recipe {
    Recipe {
        exposure,
        ..Default::default()
    }
}

fn save(cat: &mut Catalog, id: PhotoId, path: &std::path::Path, exposure: f32) -> Result<()> {
    cat.save_edit(
        id,
        path,
        &Identity::read(path)?,
        &recipe(exposure),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )
}

#[test]
fn the_fingerprint_follows_the_edit_and_the_metadata_and_nothing_else() -> Result<()> {
    let (_dir, mut cat, photos) = catalog(2)?;
    let (a, path) = photos[0].clone();
    let (b, _) = photos[1].clone();
    let first = cat.photo_fingerprint(a)?.unwrap();
    assert_eq!(cat.photo_fingerprint(a)?.unwrap(), first, "stable");
    // Another photo's change leaves it alone.
    cat.set_metadata(b, 3, 0, "")?;
    assert_eq!(cat.photo_fingerprint(a)?.unwrap(), first);
    save(&mut cat, a, &path, 1.0)?;
    let edited = cat.photo_fingerprint(a)?.unwrap();
    assert_ne!(edited, first);
    cat.set_metadata(a, 2, 0, "")?;
    let rated = cat.photo_fingerprint(a)?.unwrap();
    assert_ne!(rated, edited);
    cat.set_text(&[a], TextField::Title, "Harbour")?;
    assert_ne!(cat.photo_fingerprint(a)?.unwrap(), rated);
    assert_eq!(cat.photo_fingerprint(PhotoId(9999))?, None);
    Ok(())
}

#[test]
fn a_conditional_edit_is_written_only_while_the_photo_is_as_it_was_read() -> Result<()> {
    let (_dir, mut cat, photos) = catalog(1)?;
    let (id, path) = photos[0].clone();
    let read = cat.photo_fingerprint(id)?.unwrap();
    let identity = Identity::read(&path)?;
    let (r, export) = (recipe(0.5), ExportOptions::default());
    let edit = EditToSave {
        id,
        path: &path,
        source: &identity,
        recipe: &r,
        export: &export,
        history: HistoryUpdate::Keep,
    };
    // Someone else edits it first: the proposal computed from `read` is refused.
    save(&mut cat, id, &path, 2.0)?;
    let yes = || true;
    assert_eq!(
        cat.change_edits_if(&[(&read, EditChange::Save(&edit))], &yes)?,
        Some(vec![Conditional::Moved])
    );
    assert_eq!(cat.load_edit(id, &path)?.unwrap().recipe.exposure, 2.0);
    // Read again, but no longer admitted when the write begins: nothing.
    let now = cat.photo_fingerprint(id)?.unwrap();
    assert_eq!(
        cat.change_edits_if(&[(&now, EditChange::Save(&edit))], &|| false)?,
        None
    );
    assert_eq!(cat.load_edit(id, &path)?.unwrap().recipe.exposure, 2.0);
    // Admitted, it goes through.
    assert_eq!(
        cat.change_edits_if(&[(&now, EditChange::Save(&edit))], &yes)?,
        Some(vec![Conditional::Written])
    );
    assert_eq!(cat.load_edit(id, &path)?.unwrap().recipe.exposure, 0.5);
    Ok(())
}

#[test]
fn a_conditional_edit_of_several_photos_writes_those_still_as_read_in_one_go() -> Result<()> {
    let (_dir, mut cat, photos) = catalog(2)?;
    let (a, a_path) = photos[0].clone();
    let (b, b_path) = photos[1].clone();
    let read = [
        cat.photo_fingerprint(a)?.unwrap(),
        cat.photo_fingerprint(b)?.unwrap(),
    ];
    let identities = [Identity::read(&a_path)?, Identity::read(&b_path)?];
    let (r, export) = (recipe(0.5), ExportOptions::default());
    let edit = |id, path, source| EditToSave {
        id,
        path,
        source,
        recipe: &r,
        export: &export,
        history: HistoryUpdate::Keep,
    };
    let (edit_a, edit_b) = (
        edit(a, &a_path, &identities[0]),
        edit(b, &b_path, &identities[1]),
    );
    save(&mut cat, b, &b_path, 2.0)?;
    assert_eq!(
        cat.change_edits_if(
            &[
                (&read[0], EditChange::Save(&edit_a)),
                (&read[1], EditChange::Save(&edit_b)),
            ],
            &|| true,
        )?,
        Some(vec![Conditional::Written, Conditional::Moved])
    );
    assert_eq!(cat.load_edit(a, &a_path)?.unwrap().recipe.exposure, 0.5);
    assert_eq!(cat.load_edit(b, &b_path)?.unwrap().recipe.exposure, 2.0);
    Ok(())
}

#[test]
fn keywords_spots_and_masks_and_the_history_change_the_fingerprint() -> Result<()> {
    let (_dir, mut cat, photos) = catalog(2)?;
    let (a, path) = photos[0].clone();
    let (b, _) = photos[1].clone();
    save(&mut cat, a, &path, 1.0)?;
    let mut last = cat.photo_fingerprint(a)?.unwrap();
    let mut moved = |cat: &Catalog, what: &str| -> Result<()> {
        let now = cat.photo_fingerprint(a)?.unwrap();
        assert_ne!(now, last, "{what} left the fingerprint as it was");
        last = now;
        Ok(())
    };
    let unmoved = |cat: &Catalog, last: &str| -> Result<()> {
        assert_eq!(
            cat.photo_fingerprint(a)?.unwrap(),
            last,
            "another photo's change"
        );
        Ok(())
    };
    let keyword = cat.keyword_at(&["Harbour".into()])?;
    let before = cat.photo_fingerprint(a)?.unwrap();
    // The same change to another photo leaves it alone.
    cat.add_keyword(&[b], keyword)?;
    unmoved(&cat, &before)?;
    cat.add_keyword(&[a], keyword)?;
    moved(&cat, "a keyword")?;
    let local = "INSERT INTO local_edits(photo, data) VALUES (?, '{\"spots\":[]}')";
    let history = "INSERT INTO develop_history(photo, data) VALUES (?, X'00FF')";
    let before = cat.photo_fingerprint(a)?.unwrap();
    cat.db_for_tests().execute(local, [b.0])?;
    unmoved(&cat, &before)?;
    cat.db_for_tests().execute(local, [a.0])?;
    moved(&cat, "spots and masks")?;
    let before = cat.photo_fingerprint(a)?.unwrap();
    cat.db_for_tests().execute(history, [b.0])?;
    unmoved(&cat, &before)?;
    cat.db_for_tests().execute(history, [a.0])?;
    moved(&cat, "a History")?;
    cat.db_for_tests().execute(
        "UPDATE develop_history SET data=X'00FE' WHERE photo=?",
        [a.0],
    )?;
    moved(&cat, "a History step")?;
    Ok(())
}

/// A photo removed and another taking its id: the id's fingerprint is the
/// new photo's. (The same content under a reused id is told apart by the
/// catalog's structural generation, which a removal moves on.)
#[test]
fn a_reused_id_has_the_new_photos_fingerprint() -> Result<()> {
    let (_dir, mut cat, photos) = catalog(2)?;
    let (a, b) = (photos[0].0, photos[1].0);
    let copy = cat.create_virtual_copy(a)?;
    let first = cat.photo_fingerprint(copy)?.unwrap();
    cat.remove_virtual_copy(copy)?;
    assert_eq!(cat.photo_fingerprint(copy)?, None);
    let again = cat.create_virtual_copy(b)?;
    assert_eq!(again, copy, "the id is reused");
    assert_ne!(cat.photo_fingerprint(again)?.unwrap(), first);
    Ok(())
}

/// A conditional edit whose every photo moved on writes nothing, and doesn't
/// raise the catalog's format for a recipe it never stores.
#[test]
fn a_conditional_edit_that_writes_nothing_leaves_the_format_alone() -> Result<()> {
    let (_dir, mut cat, photos) = catalog(1)?;
    let (id, path) = photos[0].clone();
    let read = cat.photo_fingerprint(id)?.unwrap();
    let identity = Identity::read(&path)?;
    let mut r = recipe(0.5);
    r.hdr.enabled = true;
    let export = ExportOptions::default();
    let edit = EditToSave {
        id,
        path: &path,
        source: &identity,
        recipe: &r,
        export: &export,
        history: HistoryUpdate::Keep,
    };
    save(&mut cat, id, &path, 2.0)?;
    let before = cat.format_version()?;
    assert_eq!(
        cat.change_edits_if(&[(&read, EditChange::Save(&edit))], &|| true)?,
        Some(vec![Conditional::Moved])
    );
    assert_eq!(cat.format_version()?, before);
    // Still as read, it is written and raises the format.
    let now = cat.photo_fingerprint(id)?.unwrap();
    assert_eq!(
        cat.change_edits_if(&[(&now, EditChange::Save(&edit))], &|| true)?,
        Some(vec![Conditional::Written])
    );
    assert!(cat.format_version()? > before);
    Ok(())
}
