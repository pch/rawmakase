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
    assert_eq!(
        cat.change_edits_if(id, &read, &EditChange::Save(&edit))?,
        Conditional::Moved
    );
    assert_eq!(cat.load_edit(id, &path)?.unwrap().recipe.exposure, 2.0);
    // Read again, it goes through.
    let now = cat.photo_fingerprint(id)?.unwrap();
    assert_eq!(
        cat.change_edits_if(id, &now, &EditChange::Save(&edit))?,
        Conditional::Written
    );
    assert_eq!(cat.load_edit(id, &path)?.unwrap().recipe.exposure, 0.5);
    // A change for another photo than the one checked is a mistake.
    assert!(
        cat.change_edits_if(PhotoId(id.0 + 1), &now, &EditChange::Save(&edit))
            .is_err()
    );
    Ok(())
}
