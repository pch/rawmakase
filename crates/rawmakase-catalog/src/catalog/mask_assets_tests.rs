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

/// A recipe with one AI Remove spot whose patch is a new raster.
fn generated(seed: f32) -> Recipe {
    use crate::model::retouch::{RetouchMode, RetouchOp, RetouchPatch, RetouchShape};
    let samples: Vec<f32> = (0..4 * 3 * 3).map(|i| seed + i as f32 * 0.01).collect();
    let patch = crate::storage::patches::encode(4, 3, &samples).unwrap();
    let id = mask_assets::register(patch).unwrap();
    Recipe {
        retouch: vec![RetouchOp {
            mode: RetouchMode::Generate,
            shape: RetouchShape::Brush {
                points: vec![[0.4, 0.4], [0.45, 0.42]].into(),
                radius: 0.02,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.; 2],
            patch: Some(RetouchPatch {
                id,
                width: 4,
                height: 3,
                rect: [0.35, 0.35, 0.5, 0.47],
                generator: "dev.pch.ai-remove/remove".into(),
                model: "bria/fibo-edit-1.5/eraser".into(),
                ai_denoise: false,
                summary: None,
            }),
        }],
        ..Default::default()
    }
}

/// A generated spot's patch is stored with the edit that names it, in a catalog
/// of the version releases before generated spots refuse; it reads back
/// through the loader as the patch it was, in a snapshot and a virtual copy too.
#[test]
fn a_generated_spot_is_stored_with_its_patch() -> Result<()> {
    let (_dir, mut cat, file, id) = catalog()?;
    let recipe = generated(0.1);
    save(&mut cat, id, &file, &recipe)?;
    assert_eq!(cat.format_version()?, super::db::NEWEST);
    assert_eq!(stored(&cat, &recipe)?, 1);
    let patch_id = recipe.mask_asset_ids().next().unwrap().to_string();
    assert!(mask_assets::unsaved([patch_id.as_str()]).is_empty());
    let read = cat.mask_asset_loader().load(&patch_id).unwrap().unwrap();
    assert!(read.matches_id(&patch_id));
    assert_eq!((read.channels, read.depth), (3, 2));
    assert_eq!(
        cat.load_edit(id, &file)?.unwrap().recipe.retouch,
        recipe.retouch
    );
    let copy = cat.create_virtual_copy(id)?;
    assert_eq!(
        cat.load_edit(copy, &file)?.unwrap().recipe.retouch,
        recipe.retouch
    );
    let again = generated(0.2);
    cat.add_snapshot(id, "Removed", &again)?;
    assert_eq!(stored(&cat, &again)?, 1);
    Ok(())
}

/// An edit whose patch nothing holds is refused whole, as one naming a missing
/// mask raster is: the edit is never saved without its pixels.
#[test]
fn an_edit_naming_a_patch_nothing_stores_is_not_saved() -> Result<()> {
    let (_dir, mut cat, file, id) = catalog()?;
    let mut recipe = generated(0.3);
    recipe.retouch[0].patch.as_mut().unwrap().id = format!("sha256:{}", "c7".repeat(32));
    let error = save(&mut cat, id, &file, &recipe).unwrap_err();
    assert!(error.to_string().contains("missing"), "{error}");
    assert!(cat.load_edit(id, &file)?.is_none());
    Ok(())
}

fn has(cat: &Catalog, recipe: &Recipe) -> Result<bool> {
    Ok(stored(cat, recipe)? == recipe.mask_asset_ids().count())
}
/// The catalog at `path` opened for use, as the app and the command line open
/// it, a week after its rasters were last looked through.
fn opened_a_week_later(path: &Path) -> Result<Catalog> {
    let mut cat = Catalog::open(path)?;
    cat.db_for_tests()
        .execute("DELETE FROM meta WHERE key='rasters_checked_at'", [])?;
    assert_eq!(cat.remove_unused_data(&mut |_| {}), None);
    Ok(cat)
}
/// [`opened_a_week_later`] twice, so what nothing names at either look is
/// deleted.
fn reopened(path: &Path) -> Result<Catalog> {
    drop(opened_a_week_later(path)?);
    opened_a_week_later(path)
}

/// #1112: a deleted spot's patch, a replaced generation and a removed photo's
/// masks are deleted as the catalog opens; what the edit names stays.
#[test]
fn opening_deletes_the_rasters_nothing_names_any_more() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let deleted_spot = generated(1.0);
    save(&mut cat, id, &file, &deleted_spot)?;
    save(&mut cat, id, &file, &Recipe::default())?;
    let replaced = generated(1.1);
    save(&mut cat, id, &file, &replaced)?;
    let kept = generated(1.2);
    save(&mut cat, id, &file, &kept)?;
    let copy = cat.create_virtual_copy(id)?;
    let removed_photos = masked(40);
    save(&mut cat, copy, &file, &removed_photos)?;
    cat.remove_virtual_copy(copy)?;
    for recipe in [&deleted_spot, &replaced, &removed_photos] {
        assert!(has(&cat, recipe)?);
    }
    drop(cat);

    let cat = reopened(&path)?;
    for recipe in [&deleted_spot, &replaced, &removed_photos] {
        assert_eq!(stored(&cat, recipe)?, 0);
    }
    assert!(has(&cat, &kept)?);
    assert_eq!(
        cat.load_edit(id, &file)?.unwrap().recipe.retouch,
        kept.retouch
    );
    Ok(())
}

/// A raster named only by a History step, a snapshot or a virtual copy stays, and
/// so does every raster while a History can't be read or a later release's table
/// may name one.
#[test]
fn opening_keeps_the_rasters_a_history_snapshot_or_copy_names() -> Result<()> {
    use super::develop_history::{SavedHistory, SavedStep};
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let in_history = generated(2.0);
    let in_origin = masked(41);
    let history = SavedHistory {
        origin: in_origin.clone(),
        steps: vec![SavedStep {
            name: "AI Remove".into(),
            value: String::new(),
            recipe: in_history.clone(),
        }],
        applied: 0,
    };
    cat.save_edit(
        id,
        &file,
        &crate::storage::Identity::read(&file)?,
        &in_origin,
        &ExportOptions::default(),
        HistoryUpdate::Replace(&history),
    )?;
    let in_snapshot = masked(42);
    cat.add_snapshot(id, "Before", &in_snapshot)?;
    let in_copy = generated(2.1);
    let copy = cat.create_virtual_copy(id)?;
    save(&mut cat, copy, &file, &in_copy)?;
    let unnamed = generated(2.2);
    save(&mut cat, copy, &file, &unnamed)?;
    save(&mut cat, copy, &file, &in_copy)?;
    drop(cat);

    let cat = reopened(&path)?;
    for recipe in [&in_history, &in_origin, &in_snapshot, &in_copy] {
        assert!(has(&cat, recipe)?);
    }
    assert_eq!(stored(&cat, &unnamed)?, 0);

    // A later release's table, and a History this release can't read.
    let orphan = generated(2.3);
    let mut cat = cat;
    save(&mut cat, copy, &file, &orphan)?;
    save(&mut cat, copy, &file, &in_copy)?;
    cat.db_for_tests()
        .execute_batch("CREATE TABLE later_release(x TEXT)")?;
    drop(cat);
    let cat = reopened(&path)?;
    assert!(has(&cat, &orphan)?);
    cat.db_for_tests().execute_batch(
        "DROP TABLE later_release; UPDATE develop_history SET data=x'00ff' WHERE photo=(SELECT min(photo) FROM develop_history)",
    )?;
    drop(cat);
    let cat = reopened(&path)?;
    assert!(has(&cat, &orphan)?);
    Ok(())
}

/// A raster stored in the transaction of the edit that names it is never seen
/// unnamed. One deleted by another window that opened the catalog, while this
/// process still holds it (an Undo back to a deleted spot, say), is stored
/// again when the edit naming it is saved, not refused.
#[test]
fn a_raster_deleted_while_this_process_holds_it_is_stored_again() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let spot = generated(3.0);
    save(&mut cat, id, &file, &spot)?;
    save(&mut cat, id, &file, &Recipe::default())?;
    // Another window opens the catalog meanwhile.
    let other = reopened(&path)?;
    assert_eq!(stored(&other, &spot)?, 0);
    save(&mut cat, id, &file, &spot)?;
    assert!(has(&cat, &spot)?);
    drop((cat, other));
    let cat = reopened(&path)?;
    assert!(has(&cat, &spot)?);
    Ok(())
}

/// Deleting rasters that were a large part of the file compacts it; deleting a
/// few small ones leaves their space for later writes.
#[test]
fn opening_compacts_the_file_when_the_deleted_rasters_were_large() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let mut state = 0x9e37_79b9_u32;
    let noise: Vec<u8> = (0..1024 * 1024)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state as u8
        })
        .collect();
    let large = mask_assets::register(Bitmap {
        width: 1024,
        height: 1024,
        channels: 1,
        depth: 1,
        data: noise,
    })?;
    let mut recipe = masked(43);
    let MaskShape::Bitmap(b) = &mut recipe.masks[0].components[0].shape else {
        unreachable!()
    };
    (b.id, b.width, b.height) = (large, 1024, 1024);
    save(&mut cat, id, &file, &recipe)?;
    save(&mut cat, id, &file, &Recipe::default())?;
    drop(cat);
    let before = std::fs::metadata(&path)?.len();
    let cat = reopened(&path)?;
    assert_eq!(stored(&cat, &recipe)?, 0);
    let after = std::fs::metadata(&path)?.len();
    assert!(after < before / 2, "{before} -> {after}");

    // A small one doesn't.
    let mut cat = cat;
    save(&mut cat, id, &file, &masked(44))?;
    save(&mut cat, id, &file, &Recipe::default())?;
    let pages = |cat: &Catalog| -> Result<i64> {
        Ok(cat
            .db_for_tests()
            .query_row("PRAGMA page_count", [], |r| r.get(0))?)
    };
    let before = pages(&cat)?;
    drop(cat);
    let cat = reopened(&path)?;
    assert_eq!(pages(&cat)?, before);
    Ok(())
}

/// Looking through the rasters inflates every History, so it is done at most
/// once a week, whatever the opens in between; a time ahead of the clock doesn't
/// put it off.
#[test]
fn the_rasters_are_looked_through_at_most_once_a_week() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let week = 7 * 24 * 60 * 60;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    for (checked, deleted) in [
        (None, true),
        (Some(now), false),
        (Some(now - week + 60), false),
        (Some(now - week - 60), true),
        (Some(now + 3600), true),
    ] {
        let spot = generated(4.0);
        save(&mut cat, id, &file, &spot)?;
        save(&mut cat, id, &file, &Recipe::default())?;
        let db = cat.db_for_tests();
        db.execute("DELETE FROM meta WHERE key='rasters_checked_at'", [])?;
        // Unnamed at the look before, too.
        db.execute(
            "INSERT INTO meta(key, value) VALUES ('rasters_unnamed', ?1)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            [spot.mask_asset_ids().next().unwrap()],
        )?;
        if let Some(at) = checked {
            db.execute(
                "INSERT INTO meta(key, value) VALUES ('rasters_checked_at', ?1)",
                [at.to_string()],
            )?;
        }
        drop(cat);
        cat = Catalog::open(&path)?;
        assert_eq!(cat.remove_unused_data(&mut |_| {}), None);
        assert_eq!(stored(&cat, &spot)? == 0, deleted, "checked at {checked:?}");
    }
    Ok(())
}

/// A raster is deleted only when nothing named it at two looks a week apart, so
/// a window that still holds an edit naming it (an Undo back to a deleted spot)
/// has that week to save it. Named again between the looks, it waits anew.
#[test]
fn a_raster_unnamed_at_one_look_waits_for_the_next() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let spot = generated(5.0);
    save(&mut cat, id, &file, &spot)?;
    save(&mut cat, id, &file, &Recipe::default())?;
    drop(cat);
    let mut cat = opened_a_week_later(&path)?;
    assert!(has(&cat, &spot)?);
    // Saved again before the next look, then unnamed again.
    save(&mut cat, id, &file, &spot)?;
    drop(cat);
    let mut cat = opened_a_week_later(&path)?;
    save(&mut cat, id, &file, &Recipe::default())?;
    drop(cat);
    let cat = opened_a_week_later(&path)?;
    assert!(has(&cat, &spot)?);
    drop(cat);
    let cat = opened_a_week_later(&path)?;
    assert_eq!(stored(&cat, &spot)?, 0);
    Ok(())
}

/// An id of a shape this release doesn't know is kept while a text holds it, as
/// is a known id written in a longer stretch of hex digits.
#[test]
fn ids_of_other_shapes_are_kept_while_a_text_holds_them() -> Result<()> {
    let (dir, mut cat, file, id) = catalog()?;
    let path = dir.path().join("Masks.rawmakase");
    let raster = Bitmap {
        width: 1,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![3],
    }
    .compress()?;
    let later = format!("sha512:{}", "ab".repeat(64));
    let inside = "cd".repeat(16);
    let unnamed = format!("sha512:{}", "ef".repeat(64));
    let db = cat.db_for_tests();
    for hash in [&later, &inside, &unnamed] {
        db.execute(
            "INSERT INTO bitmaps(hash, data) VALUES (?1, ?2)",
            rusqlite::params![hash, raster],
        )?;
    }
    save(&mut cat, id, &file, &Recipe::default())?;
    cat.db_for_tests().execute(
        "UPDATE photos SET lightroom_develop=?1 WHERE id=?2",
        rusqlite::params![format!("a later release's {later} and 0{inside}0"), id.0],
    )?;
    drop(cat);
    let cat = reopened(&path)?;
    let count = |hash: &str| -> Result<i64> {
        Ok(cat.db_for_tests().query_row(
            "SELECT count(*) FROM bitmaps WHERE hash=?1",
            [hash],
            |r| r.get(0),
        )?)
    };
    assert_eq!(
        (count(&later)?, count(&inside)?, count(&unnamed)?),
        (1, 1, 0)
    );
    Ok(())
}
