//! Adding a folder: each new photo is kept with all it brings, or not at all.
use super::*;
use crate::metadata::{LangAlt, Value};
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use std::path::PathBuf;

/// A folder with a RAW edited by an older release, a RAW with an XMP
/// sidecar and a RAW with neither, and the catalog to add it to.
fn folder() -> Result<(tempfile::TempDir, PathBuf, Catalog, Recipe)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("edited.dng"), b"synthetic raw edited")?;
    let edit = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    legacy_sidecar::save(&folder.join("edited.dng"), &edit, &ExportOptions::default())?;
    std::fs::write(folder.join("tagged.dng"), b"synthetic raw tagged")?;
    std::fs::write(
        folder.join("tagged.dng.xmp"),
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
          xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="3"><dc:title><rdf:Alt>
          <rdf:li xml:lang="x-default">From sidecar</rdf:li></rdf:Alt></dc:title>
        </rdf:Description></rdf:RDF></x:xmpmeta>"#,
    )?;
    std::fs::write(folder.join("plain.dng"), b"synthetic raw plain")?;
    let cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    Ok((dir, folder, cat, edit))
}

fn defaults() -> NewPhotoDefaults {
    NewPhotoDefaults::new(
        MetadataDefaults {
            creator: "Default Creator".into(),
            copyright: "© Default".into(),
        },
        crate::exif::read,
        |_| None,
    )
}

/// Everything adding `folder` should have given its photos.
fn assert_complete(cat: &Catalog, edit: &Recipe) -> Result<()> {
    let photos = cat.photos()?;
    assert_eq!(photos.len(), 3);
    let find = |name: &str| photos.iter().find(|p| p.filename == name).unwrap();
    let edited = find("edited.dng");
    assert_eq!(
        cat.load_edit(edited.id, &edited.path)?.unwrap().recipe,
        *edit
    );
    let tagged = find("tagged.dng");
    assert_eq!(
        cat.descriptive(tagged.id)?.title,
        Some(Value::Set(LangAlt::new("From sidecar")))
    );
    assert_eq!(tagged.rating, 3);
    for photo in [edited, tagged, find("plain.dng")] {
        let d = cat.descriptive(photo.id)?;
        assert_eq!(
            d.creator,
            Some(Value::Set(vec!["Default Creator".into()])),
            "{}",
            photo.filename
        );
        assert_eq!(d.copyright, Some(Value::Set(LangAlt::new("© Default"))));
    }
    Ok(())
}

#[test]
fn a_failed_metadata_write_keeps_no_photo_and_a_retry_completes_them() -> Result<()> {
    let (_dir, folder, mut cat, edit) = folder()?;
    cat.fail_metadata_writes()?;
    assert!(cat.import_folder(&folder, &defaults(), &[]).is_err());
    assert!(cat.photos()?.is_empty());
    cat.db_for_tests()
        .execute_batch("DROP TRIGGER temp.fail_metadata")?;
    let added = cat.import_folder(&folder, &defaults(), &[])?;
    assert_eq!(added.added, 3);
    assert_complete(&cat, &edit)
}

#[test]
fn a_failed_edit_write_keeps_no_photo_and_a_retry_completes_them() -> Result<()> {
    let (_dir, folder, mut cat, edit) = folder()?;
    cat.db_for_tests().execute_batch(
        "CREATE TEMP TRIGGER fail_edit BEFORE UPDATE OF recipe ON photos
         BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
    )?;
    assert!(cat.import_folder(&folder, &defaults(), &[]).is_err());
    assert!(cat.photos()?.is_empty());
    cat.db_for_tests()
        .execute_batch("DROP TRIGGER temp.fail_edit")?;
    assert_eq!(cat.import_folder(&folder, &defaults(), &[])?.added, 3);
    assert_complete(&cat, &edit)
}

#[test]
fn a_legacy_edit_that_cant_be_kept_leaves_its_photo_added_without_it() -> Result<()> {
    use crate::model::masks::{BITMAP_SAMPLING, BitmapMask, MaskComponent, MaskGroup, MaskShape};
    let (_dir, folder, mut cat, edit) = folder()?;
    // Its mask's raster is neither saved with it nor in the catalog.
    let raster = crate::storage::bitmaps::Bitmap {
        width: 2,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![43, 3],
    };
    let broken = Recipe {
        masks: vec![MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
                id: raster.hash(),
                width: 2,
                height: 1,
                sampling: BITMAP_SAMPLING,
                source: None,
            }))],
            ..Default::default()
        }],
        ..Default::default()
    };
    let plain = folder.join("plain.dng");
    legacy_sidecar::save(&plain, &broken, &ExportOptions::default())?;
    assert_eq!(cat.import_folder(&folder, &defaults(), &[])?.added, 3);
    let plain = cat
        .photos()?
        .into_iter()
        .find(|p| p.filename == "plain.dng")
        .unwrap();
    assert!(cat.load_edit(plain.id, &plain.path)?.is_none());
    assert_complete(&cat, &edit)
}

#[test]
fn a_legacy_edits_masks_come_with_its_photo() -> Result<()> {
    use crate::model::masks::{BITMAP_SAMPLING, BitmapMask, MaskComponent, MaskGroup, MaskShape};
    use crate::storage::bitmaps::{Bitmap, to_base64};
    let (_dir, folder, mut cat, edit) = folder()?;
    let raster = Bitmap {
        width: 2,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![43, 3],
    };
    let masked = Recipe {
        masks: vec![MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
                id: raster.hash(),
                width: 2,
                height: 1,
                sampling: BITMAP_SAMPLING,
                source: None,
            }))],
            ..Default::default()
        }],
        ..Default::default()
    };
    let plain = folder.join("plain.dng");
    legacy_sidecar::save(&plain, &masked, &ExportOptions::default())?;
    // Its raster saved beside it, as the release that made it did.
    let companion = folder.join("plain.dng.rawmakase-local.json");
    let mut json: serde_json::Value = serde_json::from_slice(&std::fs::read(&companion)?)?;
    json["bitmaps"] = serde_json::json!({ raster.hash(): to_base64(&raster.compress()?) });
    std::fs::write(&companion, serde_json::to_vec(&json)?)?;
    assert_eq!(cat.import_folder(&folder, &defaults(), &[])?.added, 3);
    let plain = cat
        .photos()?
        .into_iter()
        .find(|p| p.filename == "plain.dng")
        .unwrap();
    assert_eq!(
        cat.load_edit(plain.id, &plain.path)?.unwrap().recipe.masks,
        masked.masks
    );
    assert_eq!(cat.bitmap(&raster.hash())?, Some(raster));
    assert_complete(&cat, &edit)
}
