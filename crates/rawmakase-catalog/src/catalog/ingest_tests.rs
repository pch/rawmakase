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

#[test]
fn a_legacy_edit_is_not_given_to_a_raw_replaced_while_its_folder_is_read() -> Result<()> {
    // Called for the RAW after its legacy sidecar was read: it replaces the file
    // there, as a copy finishing during Add Folder would.
    fn replacing(file: &std::path::Path) -> Option<Vec<u8>> {
        if file.ends_with("edited.ARW") {
            std::fs::write(file, b"synthetic raw, replaced while it was read").unwrap();
        }
        None
    }
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let raw = folder.join("edited.ARW");
    std::fs::write(&raw, b"synthetic raw edited")?;
    let edit = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    legacy_sidecar::save(&raw, &edit, &ExportOptions::default())?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    let defaults = NewPhotoDefaults::new(
        MetadataDefaults {
            creator: String::new(),
            copyright: String::new(),
        },
        crate::exif::read,
        replacing,
    );
    assert_eq!(cat.import_folder(&folder, &defaults, &[])?.added, 1);
    let photo = cat.photos()?.into_iter().next().unwrap();
    // The edit was saved for the file as it was: never the replacement's.
    assert!(!matches!(cat.load_edit(photo.id, &photo.path), Ok(Some(_))));
    Ok(())
}

/// A watched folder with a subfolder, and the catalog to add its files to.
fn watched() -> Result<(tempfile::TempDir, PathBuf, Catalog)> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("captures");
    std::fs::create_dir_all(folder.join("2026-11-14"))?;
    std::fs::write(folder.join("a.dng"), b"synthetic raw a")?;
    std::fs::write(folder.join("2026-11-14/b.dng"), b"synthetic raw b")?;
    std::fs::write(folder.join("notes.txt"), b"not a photo")?;
    std::fs::write(folder.join(".hidden.dng"), b"synthetic raw hidden")?;
    std::fs::write(dir.path().join("outside.dng"), b"synthetic raw outside")?;
    let cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    Ok((dir, folder, cat))
}

/// What adding `files` reported in `folder` did, never withdrawn.
fn report(cat: &mut Catalog, folder: &Path, files: &[PathBuf]) -> Result<FilesAdded> {
    Ok(cat
        .import_files(folder, files, &defaults(), || true)?
        .expect("not withdrawn"))
}

#[test]
fn reported_files_are_added_under_their_folder_and_the_rest_refused() -> Result<()> {
    let (dir, folder, mut cat) = watched()?;
    let a = folder.join("a.dng");
    let b = folder.join("2026-11-14/b.dng");
    let reported = vec![
        a.clone(),
        b.clone(),
        a.clone(),
        folder.join("missing.dng"),
        folder.join("notes.txt"),
        folder.join(".hidden.dng"),
        dir.path().join("outside.dng"),
        folder.join("2026-11-14"),
    ];
    let added = report(&mut cat, &folder, &reported)?;
    assert_eq!(added.added.len(), 2);
    let placed: Vec<&PathBuf> = added.placed.iter().map(|(r, _)| r).collect();
    assert_eq!(placed, [&a, &b], "a file reported twice is placed once");
    let refused: Vec<(&Path, FileRefusal)> = added
        .refused
        .iter()
        .map(|(path, why)| (path.as_path(), *why))
        .collect();
    assert_eq!(
        refused,
        [
            (folder.join("missing.dng").as_path(), FileRefusal::NotFound),
            (folder.join("notes.txt").as_path(), FileRefusal::NotAPhoto),
            (folder.join(".hidden.dng").as_path(), FileRefusal::NotAPhoto),
            (
                dir.path().join("outside.dng").as_path(),
                FileRefusal::OutsideFolder
            ),
            (folder.join("2026-11-14").as_path(), FileRefusal::NotFound),
        ]
    );
    // One root, the watched folder, with the subfolder below it.
    let roots = cat.roots()?;
    assert_eq!(roots.len(), 1);
    let photos = cat.photos()?;
    let mut names: Vec<&str> = photos.iter().map(|p| p.filename.as_str()).collect();
    names.sort();
    assert_eq!(names, ["a.dng", "b.dng"]);
    let batch = cat.previous_import()?;
    assert_eq!(batch.len(), 2, "one import batch");
    Ok(())
}

#[test]
fn a_file_reported_again_adds_nothing_and_keeps_previous_import() -> Result<()> {
    let (_dir, folder, mut cat) = watched()?;
    let a = folder.join("a.dng");
    report(&mut cat, &folder, std::slice::from_ref(&a))?;
    let before = cat.previous_import()?;
    assert_eq!(before.len(), 1);
    let again = report(&mut cat, &folder, std::slice::from_ref(&a))?;
    assert!(again.added.is_empty());
    assert_eq!(again.placed.len(), 1, "it is in the catalog");
    assert_eq!(cat.photos()?.len(), 1);
    assert_eq!(cat.previous_import()?, before, "no empty batch replaces it");
    // A new file joins the folder the first report placed, as one batch.
    let b = folder.join("2026-11-14/b.dng");
    let added = report(&mut cat, &folder, &[b.clone(), a])?;
    assert_eq!(
        added.added,
        [b.canonicalize()?],
        "only the new file is added"
    );
    assert_eq!(added.placed.len(), 2);
    assert_eq!(cat.roots()?.len(), 1);
    assert_eq!(cat.previous_import()?.len(), 1);
    Ok(())
}

#[test]
fn a_watched_folder_inside_a_catalog_folder_joins_it() -> Result<()> {
    let (dir, folder, mut cat) = watched()?;
    // The folder above the watched one is in the catalog already.
    std::fs::write(dir.path().join("outside.dng"), b"synthetic raw outside")?;
    cat.import_folder(dir.path(), &defaults(), &[])?;
    let roots = cat.roots()?;
    assert_eq!(roots.len(), 1);
    std::fs::write(folder.join("c.dng"), b"synthetic raw c")?;
    let added = report(&mut cat, &folder, &[folder.join("c.dng")])?;
    assert_eq!(added.added.len(), 1);
    assert_eq!(cat.roots()?, roots, "no new root");
    Ok(())
}

#[test]
fn a_report_withdrawn_while_its_files_are_read_writes_nothing() -> Result<()> {
    let (_dir, folder, mut cat) = watched()?;
    let a = folder.join("a.dng");
    // Asked once the files are read: a file read later (its sidecar, say)
    // is still read before the answer counts.
    let asked = std::cell::Cell::new(0);
    let withdrawn = cat.import_files(&folder, std::slice::from_ref(&a), &defaults(), || {
        asked.set(asked.get() + 1);
        false
    })?;
    assert!(withdrawn.is_none());
    assert_eq!(asked.get(), 1);
    assert!(cat.photos()?.is_empty());
    assert!(cat.roots()?.is_empty(), "no root either");
    // A report of files the catalog has is asked too.
    report(&mut cat, &folder, std::slice::from_ref(&a))?;
    let again = cat.import_files(&folder, std::slice::from_ref(&a), &defaults(), || false)?;
    assert!(again.is_none());
    Ok(())
}
