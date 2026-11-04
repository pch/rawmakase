//! What `rawmakase render` starts from (#808 step 1.12, Piotr's decision 1): the
//! raw defaults chosen in Preferences, as the app opens a photo with no edit, and,
//! given a catalog, the photo's edit there.
use rawmakase::catalog::{Catalog, HistoryUpdate};
use rawmakase::export_settings::ExportOptions;
use rawmakase::model::recipe::Recipe;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const CHART: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/corpus/charts/synthetic-d65.dng"
);

/// A data folder whose raw defaults are a preset that brightens by 0.7.
fn data_with_brighter_defaults(dir: &Path) -> PathBuf {
    let data = dir.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let preset = dir.join("Brighter.xmp");
    std::fs::write(
        &preset,
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" crs:PresetType="Normal" crs:Version="15.0" crs:Exposure2012="+0.70"><crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Brighter</rdf:li></rdf:Alt></crs:Name></rdf:Description></rdf:RDF></x:xmpmeta>"#,
    )
    .unwrap();
    let session = serde_json::json!({
        "raw_defaults": {"master": {"kind": "preset", "id": preset, "name": "Brighter"}}
    });
    std::fs::write(data.join("session.json"), session.to_string()).unwrap();
    data
}

/// `render` of `photo`, with `args` before the paths, saving the recipe it used.
fn render(data: &Path, photo: &Path, args: &[&std::ffi::OsStr]) -> (Output, PathBuf) {
    let dir = photo.parent().unwrap();
    let recipe = dir.join(format!(
        "{}-{}.json",
        photo.file_stem().unwrap().to_string_lossy(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let output = dir.join("out.tif");
    let run = Command::new(env!("CARGO_BIN_EXE_rawmakase"))
        .arg("render")
        .args(args)
        .args(["--max-edge", "64", "--overwrite"])
        .arg("--save-recipe")
        .arg(&recipe)
        .arg(photo)
        .arg(&output)
        .env("RAWMAKASE_DATA_DIR", data)
        .output()
        .unwrap();
    (run, recipe)
}

fn exposure(recipe: &Path) -> f32 {
    rawmakase::presets::load_preset(recipe)
        .unwrap()
        .recipe
        .exposure
}

#[test]
fn a_photo_without_an_edit_starts_from_the_raw_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_with_brighter_defaults(dir.path());
    let photo = dir.path().join("chart.dng");
    std::fs::copy(CHART, &photo).unwrap();
    let (run, recipe) = render(&data, &photo, &[]);
    assert!(run.status.success(), "{run:?}");
    assert_eq!(exposure(&recipe), 0.7);
}

#[test]
fn given_a_catalog_a_photo_starts_from_its_catalog_edit() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_with_brighter_defaults(dir.path());
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let (edited, unedited) = (photos.join("edited.dng"), photos.join("unedited.dng"));
    for photo in [&edited, &unedited] {
        std::fs::copy(CHART, photo).unwrap();
    }
    let catalog_path = dir.path().join("photos.rawmakase");
    let mut catalog = Catalog::create(&catalog_path).unwrap();
    catalog.add_folder(&photos).unwrap();
    let id = |path: &Path| {
        let path = path.canonicalize().unwrap();
        catalog
            .photos()
            .unwrap()
            .into_iter()
            .find(|p| p.path.canonicalize().unwrap() == path)
            .unwrap()
            .id
    };
    let edit = Recipe {
        exposure: 0.4,
        ..Default::default()
    };
    let edited_id = id(&edited);
    catalog
        .save_edit(
            edited_id,
            &edited,
            &rawmakase::storage::Identity::read(&edited).unwrap(),
            &edit,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )
        .unwrap();
    // A virtual copy of it, edited otherwise: a path names the master.
    let copy = catalog.create_virtual_copy(edited_id).unwrap();
    let copy_edit = Recipe {
        exposure: -0.4,
        ..Default::default()
    };
    catalog
        .save_edit(
            copy,
            &edited,
            &rawmakase::storage::Identity::read(&edited).unwrap(),
            &copy_edit,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )
        .unwrap();
    drop(catalog);
    let with_catalog = [std::ffi::OsStr::new("--catalog"), catalog_path.as_os_str()];

    // The catalog's edit, not the raw defaults.
    let (run, recipe) = render(&data, &edited, &with_catalog);
    assert!(run.status.success(), "{run:?}");
    assert_eq!(exposure(&recipe), 0.4);
    // A photo with no edit there starts from the raw defaults.
    let (run, recipe) = render(&data, &unedited, &with_catalog);
    assert!(run.status.success(), "{run:?}");
    assert_eq!(exposure(&recipe), 0.7);
    // A photo the catalog doesn't have is an error, not the defaults.
    let stray = dir.path().join("stray.dng");
    std::fs::copy(CHART, &stray).unwrap();
    let (run, _) = render(&data, &stray, &with_catalog);
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("not in the catalog"), "{stderr}");
    // An explicit recipe still wins.
    let explicit = dir.path().join("explicit.json");
    let darker = Recipe {
        exposure: -1.,
        ..Default::default()
    };
    rawmakase::presets::save_preset(
        &explicit,
        &darker,
        &rawmakase::photo::open(Path::new(CHART)).unwrap().metadata,
    )
    .unwrap();
    let (run, recipe) = render(
        &data,
        &edited,
        &[
            with_catalog[0],
            with_catalog[1],
            "--recipe".as_ref(),
            explicit.as_os_str(),
        ],
    );
    assert!(run.status.success(), "{run:?}");
    assert_eq!(exposure(&recipe), -1.);
}

/// A catalog edit's masks made from a selection render: their rasters are read
/// from the catalog, which a fresh command-line process has no other copy of.
#[test]
fn a_catalog_edit_renders_with_its_selection_masks() {
    use rawmakase::model::masks::{
        BITMAP_SAMPLING, BitmapMask, LocalAdjust, MaskComponent, MaskGroup, MaskShape,
    };
    let dir = tempfile::tempdir().unwrap();
    let data = data_with_brighter_defaults(dir.path());
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let photo = photos.join("masked.dng");
    std::fs::copy(CHART, &photo).unwrap();
    let catalog_path = dir.path().join("masks.rawmakase");
    let mut catalog = Catalog::create(&catalog_path).unwrap();
    catalog.add_folder(&photos).unwrap();
    let id = catalog.photos().unwrap()[0].id;
    let raster = rawmakase::storage::bitmaps::Bitmap {
        width: 4,
        height: 2,
        channels: 1,
        depth: 1,
        data: vec![255, 0, 255, 0, 255, 0, 255, 0],
    };
    let raster_id = rawmakase::storage::mask_assets::register(raster).unwrap();
    let edit = Recipe {
        exposure: 0.2,
        masks: vec![MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Bitmap(BitmapMask {
                id: raster_id,
                width: 4,
                height: 2,
                sampling: BITMAP_SAMPLING,
                source: None,
            }))],
            adjust: LocalAdjust {
                exposure: 0.5,
                ..Default::default()
            },
            ..Default::default()
        }],
        ..Default::default()
    };
    catalog
        .save_edit(
            id,
            &photo,
            &rawmakase::storage::Identity::read(&photo).unwrap(),
            &edit,
            &ExportOptions::default(),
            HistoryUpdate::Keep,
        )
        .unwrap();
    drop(catalog);
    // A preset can't hold such masks, so no recipe is saved: the render itself is
    // the check.
    let output = dir.path().join("masked.tif");
    let run = Command::new(env!("CARGO_BIN_EXE_rawmakase"))
        .arg("render")
        .args(["--max-edge", "64", "--catalog"])
        .arg(&catalog_path)
        .arg(&photo)
        .arg(&output)
        .env("RAWMAKASE_DATA_DIR", &data)
        .output()
        .unwrap();
    assert!(run.status.success(), "{run:?}");
    assert!(output.is_file());
}

/// Two folders of a catalog found at the same place make a path name two photos,
/// which may have different edits: an error, not whichever comes first.
#[test]
fn a_path_two_catalog_photos_share_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let data = data_with_brighter_defaults(dir.path());
    let (here, there) = (dir.path().join("here"), dir.path().join("there"));
    for folder in [&here, &there] {
        std::fs::create_dir(folder).unwrap();
        std::fs::copy(CHART, folder.join("chart.dng")).unwrap();
    }
    let catalog_path = dir.path().join("shared.rawmakase");
    let mut catalog = Catalog::create(&catalog_path).unwrap();
    catalog.add_folder(&here).unwrap();
    catalog.add_folder(&there).unwrap();
    let roots = catalog.roots().unwrap();
    catalog.relink_root(roots[1].0, &here).unwrap();
    drop(catalog);
    let (run, _) = render(
        &data,
        &here.join("chart.dng"),
        &["--catalog".as_ref(), catalog_path.as_os_str()],
    );
    assert!(!run.status.success());
    let stderr = String::from_utf8_lossy(&run.stderr);
    assert!(stderr.contains("2 photos in the catalog"), "{stderr}");
}
