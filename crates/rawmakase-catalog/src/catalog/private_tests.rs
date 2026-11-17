use super::tests::import_lightroom;
use super::*;
use crate::lr_develop::{convert_develop, develop_fields};
use crate::storage::Identity;
use anyhow::Context;
use rusqlite::{Connection, OpenFlags};
use std::path::PathBuf;
#[test]
#[ignore = "Requires private Lightroom catalog; set RAWMAKASE_LRCAT"]
fn supplied_catalog_is_preserved_and_all_images_import() -> Result<()> {
    let source = PathBuf::from(std::env::var("RAWMAKASE_LRCAT")?);
    let before = Identity::read(&source)?;
    let dir = tempfile::tempdir()?;
    let out = dir.path().join("import.rawmakase");
    import_lightroom(&source, &out)?;
    let c = Catalog::open(&out)?;
    let photos = c.photos()?;
    assert_eq!(photos.len(), 8112);
    assert_eq!(c.folders()?.len(), 275);
    assert_eq!(c.collections()?.len(), 13);
    let archive: Vec<u8> =
        c.db_for_tests()
            .query_row("SELECT original_catalog FROM sources", [], |r| r.get(0))?;
    assert!(archive.is_empty());
    assert_eq!(before, Identity::read(&source)?);
    for (id, make, model) in [
        (PhotoId(350644), "Sony", "ILCE-7M2"),
        (PhotoId(1062257), "Fujifilm", "X100F"),
    ] {
        let m = crate::camera_data::Metadata {
            make: make.into(),
            model: model.into(),
            ..Default::default()
        };
        let (profiles, _) = crate::camera_profiles::installed(&m);
        let text = c
            .lightroom_develop(id)?
            .context("Missing representative settings")?;
        let (recipe, warnings) = convert_develop(&text, &m, &profiles, None)?;
        recipe.validate()?;
        println!(
            "{model}: compatible Develop recipe validated; {} reported limitations",
            warnings.len()
        );
    }
    let mut valid = 0;
    let mut unsupported = 0;
    for p in photos {
        if let Some(text) = c.lightroom_develop(p.id)? {
            match develop_fields(&text) {
                Ok(_) => valid += 1,
                Err(_) => unsupported += 1,
            }
        }
    }
    println!(
        "8112 photos; 275 folders; 13 collections; Develop table parser: {valid} readable, {unsupported} preserved opaque"
    );
    assert!(valid > 8000);
    Ok(())
}
/// Every Lightroom spot and mask in a real catalog parses; those without AI convert
/// into valid recipes. Reads a temporary copy, so the catalog is never opened.
#[test]
#[ignore = "Requires private Lightroom catalog; set RAWMAKASE_LRCAT"]
fn supplied_catalog_spots_and_masks_convert() -> Result<()> {
    use crate::xmp::local::{KEYS, Node, convert};
    let source = PathBuf::from(std::env::var("RAWMAKASE_LRCAT")?);
    let dir = tempfile::tempdir()?;
    let copy = dir.path().join("copy.lrcat");
    std::fs::copy(&source, &copy)?;
    let db = Connection::open_with_flags(&copy, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let mut q = db.prepare(
        "SELECT text FROM Adobe_imageDevelopSettings WHERE text LIKE '%RetouchAreas = { {%'
         OR text LIKE '%RetouchInfo = { {%' OR text LIKE '%Corrections = { {%'",
    )?;
    let texts: Vec<String> = q
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let m = crate::camera_data::Metadata {
        width: 6000,
        height: 4000,
        ..Default::default()
    };
    let frame = crate::model::image_frame::ImageFrame::for_metadata(&m);
    let (mut photos, mut spots, mut masks) = (0, 0, 0);
    let mut skipped = std::collections::BTreeMap::<String, usize>::new();
    for text in &texts {
        let fields = develop_fields(text)?;
        let mut local = std::collections::BTreeMap::new();
        for key in KEYS {
            if let Some(value) = fields.get(key) {
                let node = Node::from_lua(value)?;
                if !node.is_empty() {
                    local.insert(key.to_string(), node);
                }
            }
        }
        let edits = convert(&local, frame);
        photos += 1;
        let mut recipe = crate::model::recipe::Recipe::default();
        if let Some(r) = edits.retouch {
            spots += r.len();
            recipe.retouch = r;
        }
        if let Some(g) = edits.masks {
            masks += g.len();
            recipe.masks = g;
        }
        recipe.validate()?;
        for s in edits.skipped {
            // Group by reason, without the mask's name.
            let reason = s.split_once(": ").map_or(s.clone(), |(_, r)| r.to_string());
            *skipped.entry(reason).or_default() += 1;
        }
    }
    println!("{photos} photos with spots or masks: {spots} spots and {masks} masks converted");
    for (reason, n) in &skipped {
        println!("  skipped {n}: {reason}");
    }
    assert!(photos > 0 && spots > 0);
    Ok(())
}
