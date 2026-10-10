use super::lightroom::import_lightroom;
use super::*;
use crate::lr_develop::{convert_develop, develop_fields};
use crate::metadata::{LangAlt, PhotoInfo, Value};
use crate::{export_settings::ExportOptions, model::recipe::Recipe, storage::Identity};
use rusqlite::{Connection, params};
fn fixture(path: &Path) -> Result<()> {
    let db = Connection::open(path)?;
    db.execute_batch("CREATE TABLE AgLibraryRootFolder(id_local INTEGER, absolutePath TEXT);
        CREATE TABLE AgLibraryFolder(id_local INTEGER,rootFolder INTEGER,pathFromRoot TEXT);
        CREATE TABLE AgLibraryFile(id_local INTEGER,folder INTEGER,idx_filename TEXT,baseName TEXT,extension TEXT);
        CREATE TABLE Adobe_images(id_local INTEGER,rootFile INTEGER,captureTime TEXT,rating INTEGER,pick INTEGER,colorLabels TEXT,fileFormat TEXT,copyName TEXT,masterImage INTEGER,orientation TEXT);
        CREATE TABLE Adobe_imageDevelopSettings(image INTEGER,text TEXT);
        CREATE TABLE AgLibraryCollection(id_local INTEGER,name TEXT,parent INTEGER,creationId TEXT);
        CREATE TABLE AgLibraryCollectionImage(collection INTEGER,image INTEGER,positionInCollection TEXT);
        CREATE TABLE AgLibraryKeyword(id_local INTEGER,name TEXT,parent INTEGER);
        CREATE TABLE AgLibraryKeywordImage(image INTEGER,tag INTEGER);
        CREATE TABLE Adobe_libraryImageDevelopHistoryStep(image INTEGER,id_local INTEGER,dateCreated REAL,name TEXT,text BLOB);
        CREATE TABLE ProprietaryData(blob BLOB);
        INSERT INTO ProprietaryData VALUES(X'001122FF');
        INSERT INTO AgLibraryRootFolder VALUES(10,'/Volumes/Photos/');
        INSERT INTO AgLibraryFolder VALUES(20,10,'Trip/'),(21,10,'Trip/Day2/');
        INSERT INTO AgLibraryFile VALUES(30,20,'image.ARW','image','ARW');
        INSERT INTO Adobe_images VALUES(40,30,'2021-06-06T10:00:00',4,1,'Red','RAW','',NULL,'AB'),(41,30,'2021-06-06T10:00:00',2,-1,'Blue','RAW','B&W',40,'AB');
        INSERT INTO Adobe_imageDevelopSettings VALUES(40,'s = { Exposure2012 = 1.5 }'),(41,'s = { ConvertToGrayscale = true }');
        INSERT INTO AgLibraryCollection VALUES(50,'Travel',NULL,'com.adobe.ag.library.collection');
        INSERT INTO AgLibraryCollectionImage VALUES(50,40,'a'),(50,41,'b');
        INSERT INTO AgLibraryKeyword VALUES(60,'City',NULL);
        INSERT INTO AgLibraryKeywordImage VALUES(40,60);")?;
    Ok(())
}
#[test]
fn lightroom_metadata_preserves_all_labels_flags_and_unrated_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("metadata.lrcat");
    fixture(&source)?;
    {
        let db = Connection::open(&source)?;
        db.execute(
            "UPDATE Adobe_images SET rating=NULL,pick=NULL,colorLabels=NULL WHERE id_local=40",
            [],
        )?;
        for (i, label) in [
            "Red",
            "Yellow",
            "Green",
            "Blue",
            "Purple",
            "Client approved",
            "Czerwony",
        ]
        .iter()
        .enumerate()
        {
            db.execute("INSERT INTO Adobe_images VALUES(?,30,'2021-06-06T10:00:00',?,?,?,'RAW','Virtual',40,'AB')",
                    params![100 + i as i64, (i % 6) as f64, (i as i32 % 3 - 1) as f64, label])?;
        }
    }
    let original = std::fs::read(&source)?;
    let destination = d.path().join("metadata.rawmakase");
    import_lightroom(&source, &destination)?;
    let mut cat = Catalog::open(&destination)?;
    let photos = cat.photos()?;
    let unrated = photos.iter().find(|p| p.id == PhotoId(40)).unwrap();
    assert_eq!(
        (unrated.rating, unrated.flag, unrated.label.as_str()),
        (0, 0, "")
    );
    for (i, label) in [
        "Red",
        "Yellow",
        "Green",
        "Blue",
        "Purple",
        "Client approved",
        "Czerwony",
    ]
    .iter()
    .enumerate()
    {
        let photo = photos
            .iter()
            .find(|p| p.id == PhotoId(100 + i as i64))
            .unwrap();
        assert_eq!(
            (photo.rating, photo.flag, photo.label.as_str()),
            ((i % 6) as i32, i as i32 % 3 - 1, *label)
        );
    }
    cat.set_metadata(PhotoId(100), 5, 1, "Purple")?;
    assert!(cat.set_metadata(PhotoId(100), 6, 1, "Red").is_err());
    assert!(cat.set_metadata(PhotoId(100), 0, 2, "Red").is_err());
    assert!(cat.set_metadata(PhotoId(9999), 0, 0, "").is_err());
    drop(cat);
    let reopened = Catalog::open(&destination)?;
    let photos = reopened.photos()?;
    let edited = photos.iter().find(|p| p.id == PhotoId(100)).unwrap();
    assert_eq!(
        (edited.rating, edited.flag, edited.label.as_str()),
        (5, 1, "Purple")
    );
    assert_eq!(
        photos.iter().find(|p| p.id == PhotoId(40)).unwrap().rating,
        0
    );
    assert_eq!(
        photos.iter().find(|p| p.id == PhotoId(101)).unwrap().label,
        "Yellow"
    );
    assert_eq!(std::fs::read(&source)?, original);
    Ok(())
}
#[test]
fn a_catalog_too_large_to_archive_imports_without_its_archive() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.lrcat");
    fixture(&source)?;
    let size = std::fs::metadata(&source)?.len();
    let archived = dir.path().join("Archived.rawmakase");
    super::lightroom::import_archiving_up_to(&source, &archived, size)?;
    let archive: Vec<u8> = Catalog::open(&archived)?.db_for_tests().query_row(
        "SELECT original_catalog FROM sources",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(archive, std::fs::read(&source)?);
    // One byte over the limit: imported in full, but with an empty archive.
    let output = dir.path().join("Photos.rawmakase");
    super::lightroom::import_archiving_up_to(&source, &output, size - 1)?;
    let mut cat = Catalog::open(&output)?;
    let (original_size, archive): (i64, Vec<u8>) = cat.db_for_tests().query_row(
        "SELECT original_size, original_catalog FROM sources",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!(original_size as u64, size);
    assert!(archive.is_empty());
    let photos = cat.photos()?;
    assert_eq!(photos.len(), 2);
    assert_eq!(photos[0].rating, 4);
    assert_eq!(photos[0].keywords, "City");
    // Opening it has nothing to backfill from.
    assert_eq!(cat.backfill_lightroom_history()?, 0);
    assert_eq!(cat.backfill_keyword_export()?, 0);
    Ok(())
}
#[test]
fn import_is_lossless_atomic_and_virtual_copies_are_independent() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("source.lrcat");
    fixture(&source)?;
    let bytes = std::fs::read(&source)?;
    let identity = Identity::read(&source)?;
    let output = dir.path().join("Photos.rawmakase");
    import_lightroom(&source, &output)?;
    assert_eq!(identity, Identity::read(&source)?);
    assert_eq!(bytes, std::fs::read(&source)?);
    let mut cat = Catalog::open(&output)?;
    let archive: Vec<u8> =
        cat.db_for_tests()
            .query_row("SELECT original_catalog FROM sources", [], |r| r.get(0))?;
    assert_eq!(archive, bytes);
    let photos = cat.photos()?;
    assert_eq!(photos.len(), 2);
    assert_eq!(photos[0].rating, 4);
    assert_eq!(photos[0].keywords, "City");
    assert_eq!(photos[1].copy_name, "B&W");
    assert_eq!(cat.collection_members(CollectionId(50))?.len(), 2);
    let local = dir.path().join("local");
    std::fs::create_dir(&local)?;
    std::fs::write(local.join("image.ARW"), b"synthetic raw identity")?;
    cat.relink_folder(FolderId(20), &local)?;
    assert_eq!(
        cat.folders()?
            .iter()
            .find(|f| f.id == FolderId(21))
            .unwrap()
            .path,
        local.join("Day2/")
    );
    let p = cat.photos()?[0].path.clone();
    let edit = Recipe {
        exposure: 1.25,
        ..Default::default()
    };
    cat.save_edit(
        PhotoId(40),
        &p,
        &edit,
        &ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    assert_eq!(cat.load_edit(PhotoId(40), &p)?.unwrap().recipe, edit);
    assert!(cat.load_edit(PhotoId(41), &p)?.is_none());
    assert!(!super::legacy_sidecar::sidecar_path(&p).exists());
    cat.set_metadata(PhotoId(41), 5, 1, "Purple")?;
    assert_eq!(cat.photos()?[0].rating, 4);
    assert_eq!(cat.photos()?[1].rating, 5);
    std::fs::write(&p, b"changed raw")?;
    assert!(cat.load_edit(PhotoId(40), &p).is_err());
    assert!(
        cat.save_edit(
            PhotoId(40),
            &p,
            &edit,
            &ExportOptions::default(),
            crate::catalog::HistoryUpdate::Keep
        )
        .is_err()
    );
    let size = output.metadata()?.len();
    assert!(import_lightroom(&source, &output).is_err());
    assert_eq!(size, output.metadata()?.len());
    assert_eq!(bytes, std::fs::read(&source)?);
    Ok(())
}
#[test]
fn bad_imports_leave_no_destination_and_future_catalogs_are_rejected() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("bad.lrcat");
    std::fs::write(&source, b"not sqlite")?;
    let dest = dir.path().join("bad.rawmakase");
    assert!(import_lightroom(&source, &dest).is_err());
    assert!(!dest.exists());
    std::fs::remove_file(&source)?;
    fixture(&source)?;
    std::fs::write(dir.path().join("bad.lrcat-wal"), b"active")?;
    assert!(import_lightroom(&source, &dest).is_err());
    assert!(!dest.exists());
    std::fs::remove_file(dir.path().join("bad.lrcat-wal"))?;
    let db = Connection::open(&source)?;
    db.execute(
        "INSERT INTO Adobe_images(id_local,rootFile) VALUES(99,999)",
        [],
    )?;
    drop(db);
    assert!(import_lightroom(&source, &dest).is_err());
    assert!(!dest.exists());
    let cat = Catalog::create(&dest)?;
    cat.db_for_tests()
        .execute_batch("PRAGMA user_version=999")?;
    drop(cat);
    assert!(Catalog::open(&dest).is_err());
    Ok(())
}
#[test]
fn folder_import_is_idempotent_and_does_not_touch_photos() -> Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("test.RAF"), b"test")?;
    std::fs::write(photos.join("note.txt"), b"ignore")?;
    // macOS AppleDouble metadata written on exFAT/FAT drives.
    std::fs::write(photos.join("._test.RAF"), b"metadata")?;
    let mut cat = Catalog::create(&d.path().join("new.rawmakase"))?;
    assert_eq!(cat.add_folder(&photos)?, 1);
    assert_eq!(cat.add_folder(&photos)?, 0);
    assert_eq!(cat.photos()?.len(), 1);
    assert_eq!(std::fs::read(photos.join("test.RAF"))?, b"test");
    Ok(())
}
#[test]
fn lightroom_table_parser_never_executes_and_reports_unsupported_edits() -> Result<()> {
    let text = r#"s = { Exposure2012 = 1.25, Contrast2012 = 15, ConvertToGrayscale = true, ToneCurvePV2012 = { 0, 12, 255, 255 }, PerspectiveUpright = 1, RetouchInfo = { { x = 0.5, y = 0.4 } }, CameraProfile = "Missing, {profile}" }"#;
    let (r, w) = convert_develop(text, &crate::camera_data::Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 1.25);
    assert!(r.effects.monochrome);
    assert_eq!(r.curve.points[0], [0., 12. / 255.]);
    assert!(w.iter().any(|s| s.contains("Missing")));
    assert!(w.iter().any(|s| s.contains("PerspectiveUpright")));
    // An incomplete spot is reported, not rendered.
    assert!(w.iter().any(|s| s.starts_with("Spot 1")), "{w:?}");
    assert!(r.retouch.is_empty());
    assert!(
        convert_develop(
            "s = { Exposure2012 = os.execute(\"bad\") }",
            &crate::camera_data::Metadata::default(),
            &[],
            None
        )
        .is_err()
    );
    assert!(develop_fields("s = { a = 1, a = 2 }").is_err());
    // With the corrections Lightroom stores, Upright imports.
    let text = r#"s = { PerspectiveUpright = 1, UprightTransformCount = 2, UprightTransform_0 = "1,0,0,0,1,0,0,0,1", UprightTransform_1 = "1.01,0,0,0,1.01,0,0.002,0,1" }"#;
    let (r, w) = convert_develop(text, &crate::camera_data::Metadata::default(), &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(r.upright.corrections[1][6], 0.002);
    Ok(())
}
#[test]
fn lightroom_point_colors_import() -> Result<()> {
    let m = crate::camera_data::Metadata::default();
    let swatch = "0.425300, 0.729800, 0.603400, 0.500000, -0.300000, 0.200000, 0.500000, 0.000000, 0.333333, 0.666667, 1.000000, 0.000000, 0.549800, 0.909800, 1.000000, 0.072700, 0.622700, 0.982700, 1.000000";
    let text = format!(
        r#"s = {{ ColorVariance = {{ 0.4 }}, PointColors = {{ "{swatch}" }}, Exposure2012 = 0.5 }}"#
    );
    let (r, w) = convert_develop(&text, &m, &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(r.point_colors.len(), 1);
    assert_eq!(r.point_colors[0].shift, [0.5, -0.3, 0.2]);
    assert_eq!(r.point_colors[0].variance, 0.4);
    // The same as bare numbers, and Lightroom's empty list.
    let text = format!("s = {{ PointColors = {{ {swatch} }} }}");
    assert_eq!(
        convert_develop(&text, &m, &[], None)?.0.point_colors,
        r.point_colors
            .iter()
            .map(|p| crate::model::point_color::PointColor { variance: 0., ..*p })
            .collect::<Vec<_>>()
    );
    let (r, w) = convert_develop(
        "s = { PointColors = {  }, ColorVariance = {  } }",
        &m,
        &[],
        None,
    )?;
    assert!(w.is_empty(), "{w:?}");
    assert!(r.point_colors.is_empty());
    Ok(())
}
#[test]
fn lightroom_auto_grayscale_mix_imports_like_a_sidecar() -> Result<()> {
    let m = crate::camera_data::Metadata::default();
    // Lightroom stores the mix it resolved, which renders as Camera Raw does.
    let text = r#"s = { ConvertToGrayscale = true, AutoGrayscaleMix = true, GrayMixerRed = -12, GrayMixerBlue = 30 }"#;
    let (r, w) = convert_develop(text, &m, &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert!(r.effects.monochrome);
    assert_eq!(r.effects.gray_mix[0], -12. * 0.01);
    assert_eq!(r.effects.gray_mix[5], 30. * 0.01);
    // Without stored values or the photo, the default mix is kept.
    let text = r#"s = { ConvertToGrayscale = true, AutoGrayscaleMix = true, Exposure2012 = 0.5 }"#;
    let (r, w) = convert_develop(text, &m, &[], None)?;
    assert!(r.effects.monochrome);
    assert_eq!(r.exposure, 0.5);
    assert_eq!(r.effects.gray_mix, Recipe::default().effects.gray_mix);
    assert!(w.is_empty(), "{w:?}");
    // With a monochrome default profile, Auto is still judged with its stored mix.
    let m = crate::camera_data::Metadata {
        model: "Synthetic".into(),
        cam_xyz: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    let mut profile = crate::camera_profiles::CameraProfile::camera_matrix_default(&m)
        .unwrap()
        .with_test_tables();
    profile.name = "Adobe Color".into();
    profile.camera = "Synthetic".into();
    profile.enhanced.as_mut().unwrap().monochrome = true;
    let profiles = [std::sync::Arc::new(profile)];
    let text = r#"s = { AutoGrayscaleMix = true, GrayMixerRed = -12 }"#;
    let (r, w) = convert_develop(text, &m, &profiles, None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(r.effects.gray_mix[0], -12. * 0.01);
    // Without Auto, a malformed channel does not take the others with it.
    for auto in ["", "AutoGrayscaleMix = false, "] {
        let text = format!(
            "s = {{ {auto}ConvertToGrayscale = true, GrayMixerRed = -12, GrayMixerBlue = 300 }}"
        );
        let (r, w) = convert_develop(&text, &crate::camera_data::Metadata::default(), &[], None)?;
        assert_eq!(r.effects.gray_mix[0], -12. * 0.01);
        assert_eq!(w.len(), 1, "{w:?}");
    }
    Ok(())
}
#[test]
fn named_white_balance_keeps_lightroom_temperature_and_tint() -> Result<()> {
    let m = crate::camera_data::Metadata {
        wb: [2., 1., 1.8],
        daylight_wb: [2., 1., 1.8],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    // Lightroom stores the resolved values with the preset's name.
    let text =
        r#"s = { WhiteBalance = "Tungsten", Temperature = 2850, Tint = 0, Exposure2012 = 0.5 }"#;
    let (r, w) = convert_develop(text, &m, &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!((r.temperature, r.tint), (2850., 0.));
    let text = r#"s = { WhiteBalance = "Fluorescent", Temperature = 3900, Tint = 18 }"#;
    let (r, w) = convert_develop(text, &m, &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!((r.temperature, r.tint), (3900., 18.));
    // A name Lightroom does not use is still reported.
    let text = r#"s = { WhiteBalance = "Moonlight", Temperature = 4100, Tint = 5 }"#;
    let (_, w) = convert_develop(text, &m, &[], None)?;
    assert!(w.iter().any(|s| s.contains("Moonlight")), "{w:?}");
    Ok(())
}
#[test]
fn lightroom_panel_switches_import_and_bypass_only_their_panels() -> Result<()> {
    use crate::model::panels::{Panel, PanelState};
    let m = crate::camera_data::Metadata::default();
    // Lightroom stores every switch, on or off, with each edit.
    let all_on: Vec<String> = Panel::ALL
        .iter()
        .flat_map(|p| p.lightroom_keys())
        .map(|key| format!("{key} = true"))
        .collect();
    let text = format!("s = {{ Exposure2012 = 0.5, {} }}", all_on.join(", "));
    let (r, w) = convert_develop(&text, &m, &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert!(r.panels.all_on());
    let text = r#"s = { EnableDetail = false, EnableEffects = false, EnableToneCurve = true, Sharpness = 40, PostCropVignetteAmount = -30, GrainAmount = 20, Exposure2012 = 0.5 }"#;
    let (r, w) = convert_develop(text, &m, &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(r.panels.state(Panel::Detail), PanelState::Off);
    assert_eq!(r.panels.state(Panel::Effects), PanelState::Off);
    assert_eq!(r.panels.state(Panel::ToneCurve), PanelState::On);
    // The settings are kept, and render as if at their defaults.
    assert!((r.sharpening - 40. / 150.).abs() < 1e-6);
    assert!((r.effects.vignette + 0.3).abs() < 1e-6);
    let shown = r.as_rendered();
    assert_eq!(shown.sharpening, 0.);
    assert_eq!(shown.effects.vignette, 0.);
    assert_eq!(shown.effects.grain, 0.);
    assert_eq!(shown.exposure, 0.5);
    Ok(())
}
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn lightroom_edits_fall_back_to_rawmakase_profiles() -> Result<()> {
    use crate::camera_profiles::open;
    let m = crate::camera_data::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let profiles: Vec<_> = [open::standard(&m), open::color(&m)]
        .into_iter()
        .flatten()
        .map(std::sync::Arc::new)
        .collect();
    for (asked, used) in [
        ("Adobe Standard", open::STANDARD),
        ("Adobe Color", open::COLOR),
    ] {
        let text = format!(r#"s = {{ Exposure2012 = 0.5, CameraProfile = "{asked}" }}"#);
        let (r, w) = convert_develop(&text, &m, &profiles, None)?;
        assert_eq!(r.exposure, 0.5);
        assert_eq!(r.profile.as_ref().unwrap().name, used);
        assert!(w.iter().any(|s| s.contains(used)), "{w:?}");
    }
    // Lightroom writes Adobe Color as a look over Adobe Standard.
    let text = r#"s = { Exposure2012 = 0.5, CameraProfile = "Adobe Standard", Look = { Name = "Adobe Color", Amount = 1 } }"#;
    let (r, w) = convert_develop(text, &m, &profiles, None)?;
    assert_eq!(r.exposure, 0.5);
    assert_eq!(r.profile.as_ref().unwrap().name, open::COLOR);
    assert!(w.iter().any(|s| s.contains(open::COLOR)), "{w:?}");
    // Other Adobe looks aren't substituted: the look is reported, and the rest of
    // the edit applies over the base profile's fallback.
    let text = r#"s = { Exposure2012 = 0.5, CameraProfile = "Adobe Standard", Look = { Name = "Adobe Vivid", Amount = 1 } }"#;
    let (r, w) = convert_develop(text, &m, &profiles, None)?;
    assert_eq!(r.exposure, 0.5);
    assert_eq!(r.profile.as_ref().unwrap().name, open::STANDARD);
    assert!(w.iter().any(|s| s.contains("Adobe Vivid")), "{w:?}");
    Ok(())
}
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn profile_amount_imports_for_looks_that_have_one() -> Result<()> {
    use crate::camera_profiles::{CameraProfile, open};
    let m = crate::camera_data::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let profiles: Vec<_> = [
        open::standard(&m),
        open::color(&m),
        Some(CameraProfile::creative_for_test(&m)),
    ]
    .into_iter()
    .flatten()
    .map(std::sync::Arc::new)
    .collect();
    let edit = |look: &str, amount: &str| {
        convert_develop(
            &format!(
                r#"s = {{ Exposure2012 = 0.5, CameraProfile = "Adobe Standard", Look = {{ Name = "{look}", Amount = {amount} }} }}"#
            ),
            &m,
            &profiles,
            None,
        )
    };
    let amount_warning = |w: &[String]| w.iter().any(|s| s.contains("Profile Amount"));
    // A look with an Amount renders at it, 0% included, with nothing to report.
    for (amount, expected) in [("0", 0.), ("0.5", 0.5), ("1.5", 1.5), ("2", 2.)] {
        let (r, w) = edit("Test Creative", amount)?;
        assert_eq!(r.exposure, 0.5);
        assert_eq!(r.profile.as_ref().unwrap().name, "Test Creative");
        assert_eq!(r.profile_amount, expected);
        assert!(!amount_warning(&w), "{amount}: {w:?}");
    }
    // A look without one (Adobe Color here falls back to RAWmakase Color) renders
    // at 100%, as Lightroom does.
    let (r, _) = edit("Adobe Color", "0.5")?;
    assert_eq!(r.profile.as_ref().unwrap().name, open::COLOR);
    assert_eq!(r.profile_amount, 1.);
    // An amount outside 0–200% is reported, and the rest still applies at 100%.
    let (r, w) = edit("Test Creative", "2.5")?;
    assert_eq!(r.exposure, 0.5);
    assert_eq!(r.profile_amount, 1.);
    assert!(amount_warning(&w), "{w:?}");
    Ok(())
}
#[test]
fn lightroom_history_text_decodes_plain_and_compressed() {
    use std::io::Write;
    let text = "s = { Exposure2012 = 0.5 }";
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(text.as_bytes()).unwrap();
    let mut blob = (text.len() as u32).to_be_bytes().to_vec();
    blob.extend(z.finish().unwrap());
    assert_eq!(
        super::lightroom::history::decode_history_text(&blob).as_deref(),
        Some(text)
    );
    assert_eq!(
        super::lightroom::history::decode_history_text(text.as_bytes()).as_deref(),
        Some(text)
    );
}
#[test]
fn lightroom_history_text_is_bounded_by_its_declared_length() {
    use std::io::Write;
    /// A zlib stream over `len` bytes of zeroes: small input, large output.
    fn bomb(declared: u32, len: usize) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(&vec![0u8; len]).unwrap();
        let mut blob = declared.to_be_bytes().to_vec();
        blob.extend(z.finish().unwrap());
        blob
    }
    let decode = super::lightroom::history::decode_history_text;
    // A length past the cap is refused without decompressing anything.
    assert!(decode(&bomb(0xffff_ffff, 64)).is_none());
    // A declared length inside the cap bounds the read, and a stream that expands
    // past it is refused rather than read cut short.
    assert!(decode(&bomb(8, 1 << 20)).is_none());
    assert_eq!(
        decode(&bomb(8, 8)).as_deref(),
        Some("\0".repeat(8).as_str())
    );
}
#[test]
fn process_version_2010_edits_keep_exposure_and_report_the_rest() -> Result<()> {
    let text = r#"s = { ProcessVersion = "5.7", Exposure = 0.75, Contrast = 40, Brightness = 50, Clarity = 0 }"#;
    let (r, w) = convert_develop(text, &crate::camera_data::Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.75);
    assert!(w.iter().any(|s| s.starts_with("Contrast")));
    // Controls at their legacy defaults are not reported.
    assert!(
        !w.iter()
            .any(|s| s.starts_with("Brightness") || s.starts_with("Clarity"))
    );
    // With 2012 keys present the legacy ones are ignored.
    let text = r#"s = { ProcessVersion = "11.0", Exposure = 0.75, Exposure2012 = 0.25 }"#;
    let (r, _) = convert_develop(text, &crate::camera_data::Metadata::default(), &[], None)?;
    assert_eq!(r.exposure, 0.25);
    Ok(())
}
#[test]
fn bitmaps_are_stored_once_by_hash() -> Result<()> {
    let d = tempfile::tempdir()?;
    let mut catalog = Catalog::create(&d.path().join("bitmaps.rawmakase"))?;
    let bitmap = crate::storage::bitmaps::Bitmap {
        width: 4,
        height: 2,
        channels: 1,
        depth: 1,
        data: vec![0, 64, 128, 255, 1, 2, 3, 4],
    };
    let hash = catalog.put_bitmap(&bitmap)?;
    assert_eq!(catalog.put_bitmap(&bitmap)?, hash);
    assert_eq!(catalog.bitmap(&hash)?, Some(bitmap));
    assert_eq!(catalog.bitmap("missing")?, None);
    Ok(())
}
#[test]
fn catalog_keeps_spots_and_masks_out_of_the_recipe_column() -> Result<()> {
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture")?;
    let mut c = Catalog::create(&d.path().join("local.rawmakase"))?;
    c.add_folder(&photos)?;
    let id = c.photos()?[0].id;
    let mut r = Recipe::default();
    r.masks.push(crate::model::masks::MaskGroup {
        components: vec![crate::model::masks::MaskComponent::new(
            crate::model::masks::MaskShape::Radial {
                center: [0.5, 0.5],
                radii: [0.2, 0.1],
                angle: 0.,
                feather: 0.5,
            },
        )],
        adjust: crate::model::masks::LocalAdjust {
            shadows: 0.5,
            ..Default::default()
        },
        ..Default::default()
    });
    let unedited = c.edit_stamp(id)?;
    c.save_edit(
        id,
        &photo,
        &r,
        &ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    // The stamp follows the edit, masks included.
    let edited = c.edit_stamp(id)?;
    assert_ne!(edited, unedited);
    let column: String =
        c.db_for_tests()
            .query_row("SELECT recipe FROM photos WHERE id=?", [id.0], |row| {
                row.get(0)
            })?;
    assert!(!column.contains("masks"));
    assert_eq!(c.load_edit(id, &photo)?.unwrap().recipe, r);
    let (text, _) = c.edit_texts(id)?;
    assert_eq!(serde_json::from_str::<Recipe>(&text.unwrap())?, r);
    c.save_edit(
        id,
        &photo,
        &Recipe::default(),
        &ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    assert!(c.load_edit(id, &photo)?.unwrap().recipe.masks.is_empty());
    assert_ne!(c.edit_stamp(id)?, edited);
    Ok(())
}
#[test]
fn adding_a_folder_imports_sidecar_edits() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    let edited = folder.join("edited.dng");
    let plain = folder.join("plain.dng");
    std::fs::write(&edited, b"synthetic raw identity")?;
    std::fs::write(&plain, b"another synthetic raw")?;
    let edit = Recipe {
        exposure: 0.75,
        ..Default::default()
    };
    super::legacy_sidecar::save(&edited, &edit, &ExportOptions::default())?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    assert_eq!(cat.add_folder(&folder)?, 2);
    let photos = cat.photos()?;
    let find = |name: &str| photos.iter().find(|p| p.path.ends_with(name)).unwrap();
    let (edited_photo, plain_photo) = (find("edited.dng"), find("plain.dng"));
    assert_eq!(
        cat.load_edit(edited_photo.id, &edited_photo.path)?
            .unwrap()
            .recipe,
        edit
    );
    assert!(cat.load_edit(plain_photo.id, &plain_photo.path)?.is_none());
    // The sidecar is left as it was.
    assert!(super::legacy_sidecar::sidecar_path(&edited).exists());
    Ok(())
}
#[test]
fn lightroom_history_is_backfilled_once() -> Result<()> {
    let d = tempfile::tempdir()?;
    let dest = d.path().join("catalog.rawmakase");
    let mut cat = Catalog::create(&dest)?;
    // A stored Lightroom catalog from before history steps were kept.
    let original = d.path().join("original.lrcat");
    Connection::open(&original)?.execute_batch("CREATE TABLE Adobe_images(id_local INTEGER)")?;
    cat.db_for_tests().execute(
        "INSERT INTO sources(path, original_size, original_catalog) VALUES ('x.lrcat', 0, ?)",
        [std::fs::read(&original)?],
    )?;
    assert_eq!(cat.backfill_lightroom_history()?, 0);
    // Reading it again would now fail: it is not read again, on this or a later open.
    cat.db_for_tests()
        .execute("UPDATE sources SET original_catalog=randomblob(4096)", [])?;
    assert_eq!(cat.backfill_lightroom_history()?, 0);
    drop(cat);
    assert_eq!(Catalog::open(&dest)?.backfill_lightroom_history()?, 0);
    Ok(())
}
#[test]
fn virtual_copies_are_created_promoted_renamed_and_removed() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("image.ARW"), b"synthetic raw identity")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let original = cat.photos()?[0].clone();
    let path = original.path.clone();
    cat.set_metadata(original.id, 3, 1, "Red")?;
    let edit = Recipe {
        exposure: 1.25,
        ..Default::default()
    };
    cat.save_edit(
        original.id,
        &path,
        &edit,
        &ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    cat.db_for_tests().execute_batch(&format!(
        "INSERT INTO keywords(id,name) VALUES(1,'City');
         INSERT INTO photo_keywords VALUES({},1);",
        original.id
    ))?;
    // A copy of a copy belongs to the same master.
    let first = cat.create_virtual_copy(original.id)?;
    let second = cat.create_virtual_copy(first)?;
    let photos = cat.photos()?;
    assert_eq!(photos.len(), 3);
    let copy = photos.iter().find(|p| p.id == first).unwrap();
    assert_eq!(copy.path, path);
    assert_eq!(copy.master, Some(original.id));
    assert_eq!(copy.copy_name, "Copy 1");
    assert_eq!((copy.rating, copy.flag, copy.label.as_str()), (3, 1, "Red"));
    assert_eq!(copy.keywords, "City");
    let copy = photos.iter().find(|p| p.id == second).unwrap();
    assert_eq!(
        (copy.master, copy.copy_name.as_str()),
        (Some(original.id), "Copy 2")
    );
    assert_eq!(cat.load_edit(first, &path)?.unwrap().recipe, edit);
    // Each copy keeps its own edit.
    let other = Recipe {
        exposure: -0.5,
        ..Default::default()
    };
    cat.save_edit(
        first,
        &path,
        &other,
        &ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    assert_eq!(cat.load_edit(original.id, &path)?.unwrap().recipe, edit);
    assert_eq!(cat.load_edit(first, &path)?.unwrap().recipe, other);

    assert!(cat.set_copy_name(original.id, "Nope").is_err());
    cat.set_copy_name(second, "  B&W ")?;
    cat.set_copy_as_master(second)?;
    assert!(cat.set_copy_as_master(second).is_err());
    let photos = cat.photos()?;
    let find = |id| photos.iter().find(|p| p.id == id).unwrap();
    assert_eq!(
        (find(second).master, find(second).copy_name.as_str()),
        (None, "")
    );
    assert_eq!(
        (
            find(original.id).master,
            find(original.id).copy_name.as_str()
        ),
        (Some(second), "B&W")
    );
    assert_eq!(find(first).master, Some(second));
    // The next copy takes the first free number.
    let third = cat.create_virtual_copy(second)?;
    assert_eq!(
        cat.photos()?
            .iter()
            .find(|p| p.id == third)
            .unwrap()
            .copy_name,
        "Copy 2"
    );

    assert!(cat.remove_virtual_copy(second).is_err());
    cat.remove_virtual_copy(first)?;
    let photos = cat.photos()?;
    assert_eq!(photos.len(), 3);
    assert!(photos.iter().all(|p| p.id != first));
    assert!(cat.load_edit(original.id, &path)?.is_some());
    Ok(())
}
#[test]
fn collection_kinds_follow_lightroom_creation_ids() {
    use CollectionKind::*;
    for (id, name, kind) in [
        ("com.adobe.ag.library.group", "Trips", Set),
        ("com.adobe.ag.library.collection", "Japan", Collection),
        (
            "com.adobe.ag.library.collection",
            "quick collection",
            System,
        ),
        ("com.adobe.ag.library.smart_collection", "Five Stars", Smart),
        ("com.adobe.ag.print", "Print", Collection),
        ("com.adobe.ag.print.unsaved", "Unsaved Print", System),
    ] {
        assert_eq!(
            CollectionKind::from_lightroom(id, name),
            kind,
            "{id} {name}"
        );
    }
}
#[test]
fn lightroom_import_copies_photo_info_from_apex_values() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("info.lrcat");
    fixture(&source)?;
    {
        let db = Connection::open(&source)?;
        db.execute_batch(
            "ALTER TABLE Adobe_images ADD COLUMN fileWidth;
             ALTER TABLE Adobe_images ADD COLUMN fileHeight;
             UPDATE Adobe_images SET fileWidth=6000, fileHeight=4000;
             UPDATE Adobe_images SET orientation='BC' WHERE id_local=40;
             CREATE TABLE AgHarvestedExifMetadata(image, aperture, shutterSpeed, isoSpeedRating,
                 focalLength, cameraModelRef, lensRef);
             CREATE TABLE AgInternedExifCameraModel(id_local, value);
             CREATE TABLE AgInternedExifLens(id_local, value);
             INSERT INTO AgInternedExifCameraModel VALUES(1,'ILCE-7M2');
             INSERT INTO AgInternedExifLens VALUES(2,'FE 55mm F1.8 ZA');
             INSERT INTO AgHarvestedExifMetadata VALUES(40, 2.0, 4.643856, 1000, 55, 1, 2);",
        )?;
    }
    let output = d.path().join("info.rawmakase");
    import_lightroom(&source, &output)?;
    let cat = Catalog::open(&output)?;
    let info = cat.photo_info(PhotoId(40))?.unwrap();
    assert_eq!(info.camera.as_deref(), Some("ILCE-7M2"));
    assert_eq!(info.lens.as_deref(), Some("FE 55mm F1.8 ZA"));
    assert_eq!(info.aperture_text().as_deref(), Some("f/2"));
    assert_eq!(info.shutter_text().as_deref(), Some("1/25 sec"));
    assert_eq!(info.exposure_text().as_deref(), Some("1/25 sec at f/2"));
    assert_eq!(info.focal_text().as_deref(), Some("55 mm"));
    assert_eq!(info.iso_text().as_deref(), Some("ISO 1000"));
    // A quarter-turned photo shows taller than it is stored.
    assert_eq!(info.dimensions_text().as_deref(), Some("4000 × 6000"));
    // A virtual copy has its master's info.
    assert_eq!(cat.photo_info(PhotoId(41))?, Some(info));
    Ok(())
}
#[test]
fn photo_info_formats_as_lightroom_shows_it() {
    let info = PhotoInfo {
        aperture: Some(1.7959),
        exposure: Some(2.5),
        focal: Some(23.),
        ..Default::default()
    };
    assert_eq!(info.aperture_text().as_deref(), Some("f/1.8"));
    assert_eq!(info.shutter_text().as_deref(), Some("2.5 sec"));
    assert_eq!(info.focal_text().as_deref(), Some("23 mm"));
    assert_eq!(PhotoInfo::default().exposure_text(), None);
    let slow = |t| PhotoInfo {
        exposure: Some(t),
        ..Default::default()
    };
    assert_eq!(slow(0.8).shutter_text().as_deref(), Some("0.8 sec"));
    assert_eq!(slow(0.5).shutter_text().as_deref(), Some("1/2 sec"));
    assert_eq!(slow(1. / 3.).shutter_text().as_deref(), Some("1/3 sec"));
}
#[test]
fn edit_times_come_from_rawmakase_or_else_lightroom_history() -> Result<()> {
    let d = tempfile::tempdir()?;
    let c = Catalog::create(&d.path().join("c.rawmakase"))?;
    c.db_for_tests().execute_batch(
        "PRAGMA foreign_keys = OFF;
         INSERT INTO photos(id, folder, filename, original_path) VALUES
             (1, 1, 'a.RAF', 'a.RAF'), (2, 1, 'b.RAF', 'b.RAF'), (3, 1, 'c.RAF', 'c.RAF');
         UPDATE photos SET edited_at = '2024-05-01 12:00:00' WHERE id = 1;
         INSERT INTO lightroom_history(photo, position, created, text) VALUES
             (2, 0, 0, ''), (2, 1, 86400, '');",
    )?;
    let times = c.edit_times()?;
    assert_eq!(
        times.get(&PhotoId(1)).map(String::as_str),
        Some("2024-05-01 12:00:00")
    );
    // Lightroom's latest step, counted from 2001.
    assert_eq!(
        times.get(&PhotoId(2)).map(String::as_str),
        Some("2001-01-02 00:00:00")
    );
    assert!(!times.contains_key(&PhotoId(3)));
    Ok(())
}

/// A develop-history snapshot as Lightroom stores it: a 4-byte big-endian
/// length and a zlib stream.
fn compressed(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(text.as_bytes()).unwrap();
    let mut blob = (text.len() as u32).to_be_bytes().to_vec();
    blob.extend(z.finish().unwrap());
    blob
}
#[test]
fn imported_develop_history_is_copied_in_date_order_and_decoded() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("history.lrcat");
    fixture(&source)?;
    let db = Connection::open(&source)?;
    // dateCreated deliberately disagrees with id_local throughout, so the copy
    // has to order by date rather than pass the rows on in table order.
    for (image, id, created, name, text) in [
        (
            40,
            1,
            400.,
            "zlib",
            Some(compressed("s = { Exposure2012 = 0.5 }")),
        ),
        (
            40,
            2,
            100.,
            "plain",
            Some(b"s = { Contrast = 25 }".to_vec()),
        ),
        (40, 3, 300., "no text", None),
        (
            41,
            4,
            200.,
            "other photo",
            Some(b"s = { Highlights = 10 }".to_vec()),
        ),
    ] {
        db.execute(
            "INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (?,?,?,?,?)",
            rusqlite::params![image, id, created, name, text],
        )?;
    }
    drop(db);

    let dest = d.path().join("history.rawmakase");
    import_lightroom(&source, &dest)?;
    let cat = Catalog::open(&dest)?;
    let steps = cat.lightroom_history(PhotoId(40))?;
    // Ordered by dateCreated, the step with no text dropped, the zlib snapshot
    // decoded to the develop settings it holds.
    assert_eq!(
        steps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["plain", "zlib"]
    );
    assert_eq!(steps[1].text, "s = { Exposure2012 = 0.5 }");
    assert_eq!(
        cat.lightroom_history(PhotoId(41))?[0].text,
        "s = { Highlights = 10 }"
    );
    Ok(())
}
#[test]
fn reopening_a_catalog_adds_the_tables_a_newer_release_needs() -> Result<()> {
    use crate::storage::bitmaps::Bitmap;
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"schema fixture")?;
    let dest = d.path().join("schema.rawmakase");
    let mut cat = Catalog::create(&dest)?;
    cat.add_folder(&photos)?;
    let id = cat.photos()?[0].id;
    drop(cat);

    // An older catalog, missing tables this release writes. Dropping a table
    // takes its rows with it, so what matters below is that the table comes back
    // with the shape the code expects, not that old rows survive.
    {
        let db = Connection::open(&dest)?;
        db.execute_batch(
            "DROP TABLE local_edits; DROP TABLE bitmaps; DROP TABLE lightroom_history;",
        )?;
    }

    // Opening runs the schema again, which is what has to fill them in.
    let mut cat = Catalog::open(&dest)?;
    let present: Vec<String> = cat
        .db_for_tests()
        .prepare(
            "SELECT name FROM sqlite_master WHERE type='table' AND name IN
         ('local_edits','bitmaps','lightroom_history') ORDER BY name",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert_eq!(present, ["bitmaps", "lightroom_history", "local_edits"]);

    // And they work: a bitmap and an edit both round trip through the new tables.
    let bitmap = Bitmap {
        width: 1,
        height: 1,
        channels: 1,
        depth: 1,
        data: vec![7],
    };
    let hash = cat.put_bitmap(&bitmap)?;
    assert_eq!(cat.bitmap(&hash)?, Some(bitmap));
    cat.save_edit(
        id,
        &photo,
        &Recipe::default(),
        &ExportOptions::default(),
        crate::catalog::HistoryUpdate::Keep,
    )?;
    assert!(cat.load_edit(id, &photo)?.is_some());
    Ok(())
}
#[test]
fn lightroom_import_reads_each_photos_descriptive_metadata() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("descriptive.lrcat");
    fixture(&source)?;
    {
        let db = Connection::open(&source)?;
        db.execute(
            "CREATE TABLE Adobe_AdditionalMetadata(id_local INTEGER, image INTEGER, xmp TEXT)",
            [],
        )?;
        let packet = |title: &str| {
            format!(
                r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
                <rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/"
                  xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:exif="http://ns.adobe.com/exif/1.0/"
                  exif:DateTimeOriginal="2021-06-06T10:00:00.25+01:00" xmp:Rating="1">
                  <dc:title><rdf:Alt><rdf:li xml:lang="x-default">{title}</rdf:li></rdf:Alt></dc:title>
                  <dc:creator><rdf:Seq><rdf:li>Example</rdf:li></rdf:Seq></dc:creator>
                </rdf:Description></rdf:RDF></x:xmpmeta>"#
            )
        };
        db.execute(
            "INSERT INTO Adobe_AdditionalMetadata VALUES(1, 40, ?), (2, 41, ?)",
            params![packet("Master"), packet("Copy")],
        )?;
    }
    let destination = d.path().join("descriptive.rawmakase");
    import_lightroom(&source, &destination)?;
    let cat = Catalog::open(&destination)?;
    let title = |id| -> Result<_> { Ok(cat.descriptive(id)?.title) };
    assert_eq!(
        title(PhotoId(40))?,
        Some(Value::Set(LangAlt::new("Master")))
    );
    assert_eq!(title(PhotoId(41))?, Some(Value::Set(LangAlt::new("Copy"))));
    let master = cat.descriptive(PhotoId(40))?;
    assert_eq!(master.creator, Some(Value::Set(vec!["Example".into()])));
    assert_eq!(master.capture.unwrap().offset.as_deref(), Some("+01:00"));
    // Rating stays Lightroom's own column's.
    let photos = cat.photos()?;
    assert_eq!(
        photos.iter().find(|p| p.id == PhotoId(40)).unwrap().rating,
        4
    );
    Ok(())
}
#[test]
fn lightroom_keyword_export_options_are_imported_and_backfilled() -> Result<()> {
    let d = tempfile::tempdir()?;
    let source = d.path().join("keywords.lrcat");
    fixture(&source)?;
    {
        let db = Connection::open(&source)?;
        db.execute_batch(
            "ALTER TABLE AgLibraryKeyword ADD COLUMN includeOnExport INTEGER NOT NULL DEFAULT 1;
             ALTER TABLE AgLibraryKeyword ADD COLUMN includeParents INTEGER NOT NULL DEFAULT 1;
             UPDATE AgLibraryKeyword SET includeOnExport=0 WHERE id_local=60;
             INSERT INTO AgLibraryKeyword VALUES(61,'Old Town',60,1,1),(62,'Square',61,1,0);
             INSERT INTO AgLibraryKeywordImage VALUES(40,62);",
        )?;
    }
    let destination = d.path().join("keywords.rawmakase");
    import_lightroom(&source, &destination)?;
    let mut cat = Catalog::open(&destination)?;
    let exported = |cat: &Catalog| -> Result<Vec<(Vec<String>, Vec<bool>)>> {
        Ok(cat
            .keywords(PhotoId(40))?
            .into_iter()
            .map(|k| (k.path, k.exported))
            .collect())
    };
    let expected = vec![
        (vec!["City".to_string()], vec![false]),
        (
            vec!["City".to_string(), "Old Town".into(), "Square".into()],
            vec![false, false, true],
        ),
    ];
    assert_eq!(exported(&cat)?, expected);
    // A catalog imported before the options were kept gets them once.
    cat.db_for_tests().execute_batch(
        "DELETE FROM keyword_export;
         DELETE FROM meta WHERE key='lightroom_keyword_export_backfilled';",
    )?;
    assert!(exported(&cat)?.iter().all(|(_, e)| e.iter().all(|x| *x)));
    assert_eq!(cat.backfill_keyword_export()?, 2);
    assert_eq!(cat.backfill_keyword_export()?, 0);
    assert_eq!(exported(&cat)?, expected);
    Ok(())
}
#[test]
fn develop_history_saves_with_the_edit_and_goes_with_the_photo() -> Result<()> {
    use crate::catalog::{HistoryUpdate, SavedHistory, SavedStep};
    use crate::export_settings::ExportOptions;
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    let photo = photos.join("a.RAF");
    std::fs::write(&photo, b"history fixture")?;
    let mut c = Catalog::create(&d.path().join("history.rawmakase"))?;
    c.add_folder(&photos)?;
    let id = c.photos()?[0].id;
    let edited = Recipe {
        exposure: 0.5,
        ..Default::default()
    };
    let history = SavedHistory {
        origin: Recipe::default(),
        steps: vec![SavedStep {
            name: "Exposure".into(),
            value: "+0.50".into(),
            recipe: edited.clone(),
        }],
        applied: 1,
    };
    let export = ExportOptions::default();
    c.save_edit(
        id,
        &photo,
        &edited,
        &export,
        HistoryUpdate::Replace(&history),
    )?;
    assert_eq!(c.load_history(id)?, Some(history.clone()));
    // Saving from outside Develop keeps it.
    c.save_edit(id, &photo, &Recipe::default(), &export, HistoryUpdate::Keep)?;
    assert_eq!(c.load_history(id)?, Some(history.clone()));
    // A History from a newer release is left unread, not misread.
    c.db_for_tests().execute(
        "UPDATE develop_history SET data=? WHERE photo=?",
        rusqlite::params![
            {
                use std::io::Write;
                let mut z = flate2::write::ZlibEncoder::new(Vec::new(), Default::default());
                z.write_all(br#"{"version": 99}"#)?;
                z.finish()?
            },
            id.0
        ],
    )?;
    assert_eq!(c.load_history(id)?, None);
    // Saving with nothing recorded (an export-only change) keeps that History.
    let empty = SavedHistory {
        origin: edited.clone(),
        steps: Vec::new(),
        applied: 0,
    };
    c.save_edit(id, &photo, &edited, &export, HistoryUpdate::of(&empty))?;
    let count = |c: &Catalog| -> Result<i64> {
        Ok(c.db_for_tests()
            .query_row("SELECT COUNT(*) FROM develop_history", [], |r| r.get(0))?)
    };
    assert_eq!(count(&c)?, 1);
    c.save_edit(
        id,
        &photo,
        &edited,
        &export,
        HistoryUpdate::Replace(&history),
    )?;
    // A virtual copy starts with the History of the edit it copies, and takes its
    // own with it when removed.
    let copy = c.create_virtual_copy(id)?;
    assert_eq!(c.load_history(copy)?, Some(history));
    assert_eq!(count(&c)?, 2);
    c.remove_virtual_copy(copy)?;
    assert_eq!(count(&c)?, 1);
    // Removing the photo removes its History.
    c.db_for_tests()
        .execute("DELETE FROM photos WHERE id=?", [id.0])?;
    assert_eq!(count(&c)?, 0);
    Ok(())
}
#[test]
#[allow(clippy::approx_constant)] // Exact camera matrix coefficients, not mathematical constants.
fn develop_history_stores_each_large_setting_once() -> Result<()> {
    use crate::catalog::{SavedHistory, SavedStep};
    let m = crate::camera_data::Metadata {
        make: "Fujifilm".into(),
        model: "X100F".into(),
        wb: [2.0198677, 1., 1.8874172],
        cam_xyz: [
            [1.1434, -0.4948, -0.121],
            [-0.3746, 1.2042, 0.1903],
            [-0.0666, 0.1479, 0.5235],
        ],
        ..Default::default()
    };
    let base = Recipe {
        profile: crate::camera_profiles::open::color(&m).map(std::sync::Arc::new),
        ..Default::default()
    };
    assert!(base.profile.is_some());
    let one = serde_json::to_string(&base)?.len();
    let steps = (1..=100)
        .map(|i| SavedStep {
            name: "Exposure".into(),
            value: String::new(),
            recipe: Recipe {
                exposure: i as f32 / 100.,
                ..base.clone()
            },
        })
        .collect();
    let history = SavedHistory {
        origin: base,
        steps,
        applied: 100,
    };
    let encoded = super::develop_history::encode(&history)?;
    // A hundred steps cost less than two recipes would uncompressed.
    assert!(
        encoded.len() < 2 * one,
        "{} bytes for a {one}-byte recipe",
        encoded.len()
    );
    assert_eq!(super::develop_history::decode(&encoded)?, Some(history));
    Ok(())
}
#[test]
fn profile_corrections_without_the_adobe_profile_use_the_built_in_correction_and_say_so()
-> Result<()> {
    let m = crate::camera_data::Metadata {
        lens_model: "FE 55mm F1.8 ZA".into(),
        lens: Some(crate::optics::LensCorrection {
            source: "Sony built-in".into(),
            default_on: false,
            vignetting: None,
            distortion: None,
            chromatic: None,
        }),
        ..Default::default()
    };
    let (r, w) = convert_develop("s = { LensProfileEnable = 1 }", &m, &[], None)?;
    assert!(r.lens_profile && r.lens_builtin);
    assert!(
        w.iter()
            .any(|s| s.contains("FE 55mm F1.8 ZA") && s.contains("using Sony built-in")),
        "{w:?}"
    );
    // Off leaves Sony's opt-in data off, with nothing to report.
    let (r, w) = convert_develop("s = { LensProfileEnable = 0 }", &m, &[], None)?;
    assert!(!r.lens_profile && !r.lens_builtin);
    assert!(w.is_empty(), "{w:?}");
    // Without any correction to fall back on, it says that too.
    let bare = crate::camera_data::Metadata {
        lens_model: "FE 55mm F1.8 ZA".into(),
        ..Default::default()
    };
    let (_, w) = convert_develop("s = { LensProfileEnable = 1 }", &bare, &[], None)?;
    assert!(w.iter().any(|s| s.contains("no lens correction")), "{w:?}");
    // With Lightroom's Lens Corrections panel switched off, nothing renders and
    // nothing is reported missing.
    let (r, w) = convert_develop(
        "s = { LensProfileEnable = 1, EnableLensCorrections = false }",
        &m,
        &[],
        None,
    )?;
    assert!(w.is_empty(), "{w:?}");
    assert!(!r.resolved(&m).lens_builtin);
    // As the preview and renderer see it: bypassed first, then resolved.
    assert!(!r.as_rendered().resolved(&m).lens_builtin);
    Ok(())
}
#[test]
fn snapshots_are_named_states_kept_per_photo_and_listed_alphabetically() -> Result<()> {
    use crate::catalog::SnapshotSettings;
    let d = tempfile::tempdir()?;
    let photos = d.path().join("photos");
    std::fs::create_dir(&photos)?;
    std::fs::write(photos.join("a.RAF"), b"snapshot fixture")?;
    let mut c = Catalog::create(&d.path().join("snapshots.rawmakase"))?;
    c.add_folder(&photos)?;
    let id = c.photos()?[0].id;
    let warm = Recipe {
        temperature: 7000.,
        ..Default::default()
    };
    let b = c.add_snapshot(
        id,
        "  bright ",
        &Recipe {
            exposure: 1.,
            ..Default::default()
        },
    )?;
    let a = c.add_snapshot(id, "Warm", &warm)?;
    assert!(c.add_snapshot(id, "  ", &warm).is_err());
    let names: Vec<_> = c.snapshots(id)?.into_iter().map(|s| s.name).collect();
    assert_eq!(names, ["bright", "Warm"]);
    c.rename_snapshot(b, "Bright")?;
    c.update_snapshot(a, &Recipe::default())?;
    let snapshots = c.snapshots(id)?;
    assert_eq!(snapshots[0].name, "Bright");
    assert_eq!(
        snapshots[1].settings,
        SnapshotSettings::Recipe(Box::default())
    );
    c.delete_snapshot(b)?;
    assert_eq!(c.snapshots(id)?.len(), 1);
    // A virtual copy has its own; removing a photo removes its snapshots.
    let copy = c.create_virtual_copy(id)?;
    assert!(c.snapshots(copy)?.is_empty());
    c.add_snapshot(copy, "Copy look", &warm)?;
    c.remove_virtual_copy(copy)?;
    let left: i64 =
        c.db_for_tests()
            .query_row("SELECT COUNT(*) FROM develop_snapshots", [], |r| r.get(0))?;
    assert_eq!(left, 1);
    Ok(())
}
#[test]
fn lightroom_snapshots_import_with_their_photo() -> Result<()> {
    use crate::catalog::SnapshotSettings;
    let d = tempfile::tempdir()?;
    let source = d.path().join("snapshots.lrcat");
    fixture(&source)?;
    {
        let db = Connection::open(&source)?;
        db.execute_batch(
            "CREATE TABLE Adobe_libraryImageDevelopSnapshot(id_local INTEGER, image INTEGER, name TEXT, text BLOB);
             INSERT INTO Adobe_libraryImageDevelopSnapshot VALUES
                (1, 40, 'Before crop', 's = { Exposure2012 = 0.5 }'),
                (2, 99, 'Other catalog photo', 's = { Exposure2012 = 1 }');",
        )?;
    }
    let destination = d.path().join("snapshots.rawmakase");
    import_lightroom(&source, &destination)?;
    let mut cat = Catalog::open(&destination)?;
    // Imported here, so opening the catalog has nothing to recover.
    assert_eq!(cat.backfill_lightroom_snapshots()?, 0);
    let snapshots = cat.snapshots(PhotoId(40))?;
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].name, "Before crop");
    assert_eq!(
        snapshots[0].settings,
        SnapshotSettings::Lightroom("s = { Exposure2012 = 0.5 }".into())
    );
    // A catalog imported before snapshots were kept recovers them once.
    cat.db_for_tests()
        .execute("DELETE FROM develop_snapshots", [])?;
    cat.db_for_tests().execute(
        "DELETE FROM meta WHERE key='lightroom_snapshots_backfilled'",
        [],
    )?;
    assert_eq!(cat.backfill_lightroom_snapshots()?, 1);
    assert_eq!(cat.snapshots(PhotoId(40))?.len(), 1);
    assert_eq!(cat.backfill_lightroom_snapshots()?, 0);
    // A copy cut short before its marker was written copies nothing twice.
    cat.db_for_tests().execute(
        "DELETE FROM meta WHERE key='lightroom_snapshots_backfilled'",
        [],
    )?;
    assert_eq!(cat.backfill_lightroom_snapshots()?, 0);
    assert_eq!(cat.snapshots(PhotoId(40))?.len(), 1);
    Ok(())
}
#[test]
fn lightroom_15_controls_at_rest_are_not_reported() -> Result<()> {
    // Lightroom 15 writes these into every Develop record. Glow's own controls do
    // nothing while Glow is 0, and the SDR and HDR values only apply in HDR editing.
    let text = r#"s = { Exposure2012 = 0.5, Glow = 0, GlowRange = 50, GlowSpread = 50, GlowStyle = 0, GlowWarmth = 0, HDREditMode = 0, HDRMaxValue = 2.3, SDRBlend = 0, SDRBrightness = 0, SDRClarity = 0, SDRContrast = 0, SDRHighlights = 0, SDRShadows = 0, SDRWhites = 0, EnableDistractionRemoval = true }"#;
    let (r, w) = convert_develop(text, &crate::camera_data::Metadata::default(), &[], None)?;
    assert!(w.is_empty(), "{w:?}");
    assert_eq!(r.exposure, 0.5);
    // Active, they are still reported.
    for active in ["Glow = 20", "HDREditMode = 1"] {
        let text = format!("s = {{ {active}, GlowRange = 50, HDRMaxValue = 2.3 }}");
        let (_, w) = convert_develop(&text, &crate::camera_data::Metadata::default(), &[], None)?;
        assert!(!w.is_empty(), "{active}");
    }
    Ok(())
}
#[test]
fn raw_cameras_leave_out_cameras_seen_only_in_jpegs() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    for name in ["a.ARW", "b.JPG", "c.jpg"] {
        std::fs::write(folder.join(name), name)?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let info = |camera: &str| {
        Some(PhotoInfo {
            camera: Some(camera.into()),
            ..Default::default()
        })
    };
    let infos: Vec<_> = cat
        .photos()?
        .iter()
        .map(|p| match p.filename.as_str() {
            "a.ARW" => (p.id, info("ILCE-7M2")),
            "b.JPG" => (p.id, info("ILCE-7M2")),
            _ => (p.id, info("iPhone 8")),
        })
        .collect();
    cat.fill_photo_info(&infos)?;
    assert_eq!(cat.cameras()?, ["ILCE-7M2", "iPhone 8"]);
    assert_eq!(cat.raw_cameras()?, ["ILCE-7M2"]);
    Ok(())
}

#[test]
fn removing_folders_leaves_their_files_and_no_rows_behind() -> Result<()> {
    use anyhow::Context;
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(photos.join("trip/day2"))?;
    for file in ["a.ARW", "trip/b.ARW", "trip/day2/c.ARW"] {
        std::fs::write(photos.join(file), format!("synthetic {file}"))?;
    }
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&photos)?;
    let id = |cat: &Catalog, name: &str| -> Result<PhotoId> {
        Ok(cat
            .photos()?
            .into_iter()
            .find(|p| p.filename == name && p.master.is_none())
            .context("photo")?
            .id)
    };
    let b = id(&cat, "b.ARW")?;
    let copy = cat.create_virtual_copy(b)?;
    cat.add_keywords(&[b, copy], &[vec!["City".to_string()]])?;
    let folder = |cat: &Catalog, relative: &str| -> Result<FolderId> {
        Ok(cat
            .folders()?
            .into_iter()
            .find(|f| f.relative.trim_end_matches('/') == relative)
            .context("folder")?
            .id)
    };
    let trip = [folder(&cat, "trip")?, folder(&cat, "trip/day2")?];

    let removed = cat.remove_folders(&trip)?;
    assert!(removed.contains(&b) && removed.contains(&copy));
    assert_eq!(removed.len(), 3, "b, its copy and c");
    let left: Vec<String> = cat.photos()?.into_iter().map(|p| p.filename).collect();
    assert_eq!(left, ["a.ARW"]);
    assert_eq!(cat.folders()?.len(), 1);
    assert_eq!(cat.roots()?.len(), 1, "the root still has a folder");
    for file in ["a.ARW", "trip/b.ARW", "trip/day2/c.ARW"] {
        assert!(photos.join(file).is_file(), "{file} stays on disk");
    }
    // No row anywhere still names a removed photo.
    let db = cat.db_for_tests();
    let tables: Vec<String> = db
        .prepare("SELECT m.name FROM sqlite_master m, pragma_table_info(m.name) c WHERE m.type='table' AND c.name='photo'")?
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    assert!(tables.contains(&"photo_keywords".to_string()));
    for table in &tables {
        let orphans: i64 = db.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE photo NOT IN (SELECT id FROM photos)"),
            [],
            |r| r.get(0),
        )?;
        assert_eq!(orphans, 0, "{table} keeps a removed photo");
    }

    // The root's last folder takes the root with it.
    let rest: Vec<FolderId> = cat.folders()?.into_iter().map(|f| f.id).collect();
    assert_eq!(cat.remove_folders(&rest)?.len(), 1);
    assert!(cat.photos()?.is_empty());
    assert!(cat.roots()?.is_empty());
    // And the folder can be added again.
    assert_eq!(cat.add_folder(&photos)?, 3);
    Ok(())
}

#[test]
fn an_imported_catalog_names_the_lightroom_catalog_it_came_from() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let source = dir.path().join("Lightroom Catalog.lrcat");
    fixture(&source)?;
    let imported = import_lightroom(&source, &dir.path().join("Imported.rawmakase"))?;
    assert_eq!(Catalog::open(&imported)?.lightroom_sources()?, [source]);
    let made = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    assert!(made.lightroom_sources()?.is_empty());
    Ok(())
}

#[test]
fn a_removed_folder_forgets_where_it_was_located() -> Result<()> {
    use anyhow::Context;
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir_all(photos.join("trip"))?;
    std::fs::write(photos.join("a.ARW"), "a")?;
    std::fs::write(photos.join("trip/b.ARW"), "b")?;
    let elsewhere = dir.path().join("external/trip");
    std::fs::create_dir_all(&elsewhere)?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&photos)?;
    let trip = cat
        .folders()?
        .into_iter()
        .find(|f| f.relative.trim_end_matches('/') == "trip")
        .context("trip")?
        .id;
    // Located elsewhere on this computer, then removed; its root stays.
    cat.relink_folder(trip, &elsewhere)?;
    cat.remove_folders(&[trip])?;
    assert_eq!(cat.roots()?.len(), 1);
    // Added again where it is: not refused as being elsewhere.
    let added = cat.import_folder(&photos.join("trip"), &Default::default(), &[])?;
    assert!(added.conflicts.is_empty(), "{:?}", added.conflicts);
    assert_eq!(added.added, 1);
    Ok(())
}
