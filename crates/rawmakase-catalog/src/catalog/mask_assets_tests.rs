//! Raster masks are stored with the edits and snapshots that refer to them.
use super::*;
use crate::model::masks::{BITMAP_SAMPLING, BitmapMask, MaskComponent, MaskGroup, MaskShape};
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets::{self, AssetError};
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use std::path::{Path, PathBuf};

fn catalog() -> Result<(tempfile::TempDir, Catalog, PathBuf, PhotoId)> {
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/charts/synthetic-d65.dng");
    let file = photos.join("a.dng");
    std::fs::copy(chart, &file)?;
    let mut cat = Catalog::create(&dir.path().join("Masks.rawmakase"))?;
    cat.add_folder(&photos)?;
    let id = cat.photos()?[0].id;
    Ok((dir, cat, file, id))
}
/// A recipe whose mask is a new raster, unlike any other test's.
fn masked(seed: u8) -> Recipe {
    let raster = Bitmap {
        width: 4,
        height: 2,
        channels: 1,
        depth: 1,
        data: vec![seed, 0, 255, 7, 8, 9, seed, 1],
    };
    let id = mask_assets::register(raster).unwrap();
    Recipe {
        masks: vec![MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
                id,
                width: 4,
                height: 2,
                sampling: BITMAP_SAMPLING,
                source: None,
            }))],
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn stored(cat: &Catalog, recipe: &Recipe) -> Result<usize> {
    let ids: Vec<&str> = recipe.mask_asset_ids().collect();
    let mut n = 0;
    for id in ids {
        n += cat.bitmap(id)?.is_some() as usize;
    }
    Ok(n)
}
fn save(cat: &mut Catalog, id: PhotoId, file: &Path, recipe: &Recipe) -> Result<()> {
    cat.save_edit(
        id,
        file,
        &crate::storage::Identity::read(file)?,
        recipe,
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )
}

#[test]
fn a_new_catalog_stores_the_rasters_of_a_saved_edit_and_reads_them_back() -> Result<()> {
    let (_dir, mut cat, file, id) = catalog()?;
    let recipe = masked(11);
    save(&mut cat, id, &file, &recipe)?;
    assert_eq!(stored(&cat, &recipe)?, 1);
    // Saved rasters are read again through the loader once the store forgot them.
    let raster_id = recipe.mask_asset_ids().next().unwrap().to_string();
    assert!(mask_assets::unsaved([raster_id.as_str()]).is_empty());
    let loader = cat.mask_asset_loader();
    let read = loader.load(&raster_id).unwrap().unwrap();
    assert_eq!(read.data, vec![11, 0, 255, 7, 8, 9, 11, 1]);
    assert!(read.matches_id(&raster_id));
    assert_eq!(loader.load("sha256:ffff").unwrap(), None);
    // The recipe reads back with its mask.
    let edit = cat.load_edit(id, &file)?.unwrap();
    assert_eq!(edit.recipe.masks, recipe.masks);
    Ok(())
}

#[test]
fn an_edit_naming_a_raster_nothing_stores_is_not_saved() -> Result<()> {
    let (_dir, mut cat, file, id) = catalog()?;
    let mut recipe = masked(12);
    // A raster nobody registered and the catalog lacks.
    let MaskShape::Bitmap(b) = &mut recipe.masks[0].components[0].shape else {
        unreachable!()
    };
    b.id = format!("sha256:{}", "cd".repeat(32));
    let error = save(&mut cat, id, &file, &recipe).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
    assert!(cat.load_edit(id, &file)?.is_none());
    Ok(())
}

#[test]
fn a_snapshot_stores_its_rasters_apart_from_the_edit() -> Result<()> {
    let (_dir, mut cat, _file, id) = catalog()?;
    let recipe = masked(13);
    let snapshot = cat.add_snapshot(id, "Subject", &recipe)?;
    assert_eq!(stored(&cat, &recipe)?, 1);
    let again = masked(14);
    cat.update_snapshot(snapshot, &again)?;
    assert_eq!(stored(&cat, &again)?, 1);
    // Snapshot rasters outlive an edit that no longer refers to them.
    assert_eq!(stored(&cat, &recipe)?, 1);
    Ok(())
}

/// Catalogs of the first two formats are upgraded as they open: the second's
/// rasters stay, and the first stores rasters from then on.
#[test]
fn catalogs_of_the_first_formats_keep_rasters_once_upgraded_on_open() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let recipe = masked(17);
    save(&mut cat, id, &file, &recipe)?;
    cat.db_for_tests().execute_batch("PRAGMA user_version=2")?;
    drop(cat);
    let cat = Catalog::open(&path)?;
    assert_eq!(cat.format_version()?, super::db::NEWEST);
    assert_eq!(stored(&cat, &recipe)?, 1);
    assert_eq!(
        cat.load_edit(id, &file)?.unwrap().recipe.masks,
        recipe.masks
    );

    cat.db_for_tests().execute_batch("PRAGMA user_version=1")?;
    drop(cat);
    let mut cat = Catalog::open(&path)?;
    assert_eq!(cat.format_version()?, super::db::NEWEST);
    let recipe = masked(15);
    save(&mut cat, id, &file, &recipe)?;
    assert_eq!(stored(&cat, &recipe)?, 1);
    Ok(())
}

#[test]
fn a_damaged_row_under_a_new_rasters_id_is_replaced_by_the_good_copy() -> Result<()> {
    let (_dir, mut cat, file, id) = catalog()?;
    let recipe = masked(16);
    let raster_id = recipe.mask_asset_ids().next().unwrap().to_string();
    cat.db_for_tests().execute(
        "INSERT INTO bitmaps(hash, data) VALUES (?1, ?2)",
        rusqlite::params![raster_id, b"not a bitmap".to_vec()],
    )?;
    save(&mut cat, id, &file, &recipe)?;
    let read = cat.bitmap(&raster_id)?.unwrap();
    assert!(read.matches_id(&raster_id));
    Ok(())
}

#[test]
fn a_stored_raster_larger_than_any_raster_is_not_read() -> Result<()> {
    let (_dir, cat, _file, _id) = catalog()?;
    let id = format!("sha256:{}", "ee".repeat(32));
    cat.db_for_tests().execute(
        "INSERT INTO bitmaps(hash, data) VALUES (?1, zeroblob(?2))",
        rusqlite::params![id, super::mask_assets::MOST_STORED_BYTES + 1],
    )?;
    let error = cat.mask_asset_loader().load(&id).unwrap_err();
    assert!(matches!(error, AssetError::Corrupt(..)), "{error:?}");
    Ok(())
}
