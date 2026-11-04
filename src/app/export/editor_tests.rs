//! Editor tests of `export`: the whole editor driven through its frames.
use crate::app::*;
use crate::catalog::PhotoId;
use rawmakase_cancel::Cancel;
use std::sync::Arc;

/// A photo exported in a batch, never opened, has exactly the pixels of the same
/// photo exported from Develop with the same settings: a saved edit with a mask and
/// a spot, a Lightroom-only edit with Auto settings left to compute, the raw
/// defaults, Upright Auto and Guided without stored corrections, a virtual copy, and
/// the open photo with adjustments not yet saved.
#[test]
fn a_batch_export_matches_develops_export_pixel_for_pixel() -> anyhow::Result<()> {
    use crate::export::{
        Replace,
        batch::{self, BatchPhoto, Edit},
    };
    use crate::export_settings::{Destination, Existing, ExportSettings, Format};
    let dir = tempfile::tempdir()?;
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos)?;
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    let names = ["a.dng", "b.dng", "c.dng", "d.dng", "e.dng", "f.dng"];
    for name in names {
        std::fs::copy(&chart, photos.join(name))?;
    }
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog)?;
    c.add_folder(&photos)?;
    let mut ids: Vec<(PhotoId, std::path::PathBuf)> =
        c.photos()?.into_iter().map(|p| (p.id, p.path)).collect();
    let metadata = crate::photo::open(&ids[0].1)?.metadata;
    let (profiles, _) = crate::camera_profiles::installed(&metadata);
    let base = Recipe::with_profiles(&metadata, &profiles);
    let save = |c: &mut crate::catalog::Catalog,
                (id, path): &(PhotoId, std::path::PathBuf),
                r: &Recipe| {
        c.save_edit(
            *id,
            path,
            &crate::storage::Identity::read(path)?,
            r,
            &Default::default(),
            crate::catalog::HistoryUpdate::Keep,
        )
    };
    // a: a mask and a spot.
    let mut local = base.clone();
    local.exposure = 0.3;
    local.masks.push(crate::model::masks::MaskGroup {
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
    local.retouch = vec![crate::model::retouch::RetouchOp {
        mode: crate::model::retouch::RetouchMode::Heal,
        shape: crate::model::retouch::RetouchShape::Spot {
            center: [0.3, 0.3],
            radius: 0.03,
        },
        feather: 0.4,
        opacity: 1.,
        offset: [0.1, 0.],
    }];
    save(&mut c, &ids[0], &local)?;
    // b: Lightroom's settings only, with Auto Tone and Auto white balance to compute.
    c.db_for_tests().execute(
        "UPDATE photos SET lightroom_develop='s = { AutoTone = true, WhiteBalance = \"Auto\", Contrast2012 = 20 }' WHERE id=?",
        [ids[1].0.0],
    )?;
    // c: nothing. d: Upright Auto, e: Guided, neither analysed.
    let mut auto = base.clone();
    auto.upright.mode = crate::model::transform::UprightMode::Auto;
    save(&mut c, &ids[3], &auto)?;
    let mut guided = base.clone();
    guided.upright.mode = crate::model::transform::UprightMode::Guided;
    guided.upright.guides = vec![
        crate::model::transform::UprightGuide {
            a: [0.2, 0.1],
            b: [0.25, 0.9],
        },
        crate::model::transform::UprightGuide {
            a: [0.8, 0.1],
            b: [0.75, 0.9],
        },
    ];
    save(&mut c, &ids[4], &guided)?;
    // A virtual copy of f, with an edit of its own.
    let copy = c.create_virtual_copy(ids[5].0)?;
    let mut copied = base;
    copied.contrast = 0.4;
    ids.push((copy, ids[5].1.clone()));
    save(&mut c, &ids[6], &copied)?;
    // Taken before any photo is opened: the batch works each edit out itself.
    let records = c.photo_records(&ids.iter().map(|(id, _)| *id).collect::<Vec<_>>())?;
    drop(c);

    let ctx = egui::Context::default();
    let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    let defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    editor.raw_defaults = defaults.clone();
    editor.library = Some(Box::new(library));
    let settings = |folder: &str| ExportSettings {
        destination: Destination::Folder,
        folder: Some(dir.path().join(folder)),
        format: Format::Tiff,
        resize: true,
        long_edge: 160,
        existing: Existing::Unique,
        ..Default::default()
    };
    let pixels = |path: &std::path::Path| image::open(path).unwrap().into_rgb16().into_raw();
    let mut rendered = Vec::new();
    for (i, ((id, path), record)) in ids.iter().zip(records).enumerate() {
        editor.open_raw(path.clone(), Some(*id));
        // Open once decoded in full and Upright's analysis is in.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
        loop {
            editor.events(&ctx);
            let open = editor.document.full().is_some_and(|im| !im.fast)
                && !editor.document.upright.is_running()
                && !editor.document.edit.recipe().upright.needs_analysis();
            if open {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "photo {i} not opened");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        let last = i == ids.len() - 1;
        if last {
            // Adjusted on screen, not saved.
            editor.document.edit.setup_mut().exposure = -0.4;
        }
        let develop = dir.path().join(format!("develop/{i}.tif"));
        std::fs::create_dir_all(develop.parent().unwrap())?;
        crate::export::job::run(
            editor.export_photo().unwrap(),
            &settings("develop"),
            &develop,
            Replace::NoClobber,
            Cancel::NEVER,
            |_| {},
        )?;
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let mut photo = BatchPhoto::from_record(record, path.clone(), name, Default::default());
        if last {
            photo.edit = Edit::Shown {
                recipe: Box::new(editor.document.edit.recipe().clone()),
                unsaved: true,
                file: Some(crate::storage::Identity::read(path)?),
            };
        }
        let settings = settings(&format!("batch/{i}"));
        let photos = vec![photo];
        let outcomes = batch::run(
            &batch::Batch {
                plan: batch::plan(&photos, &settings, None).unwrap(),
                photos,
                settings,
                defaults: defaults.clone(),
                watermark: None,
                demosaic: Default::default(),
                cache: editor.decode_cache.clone(),
                ai: rawmakase_decode::ai::denoise::AiService::refusing("no AI Denoise in tests"),
            },
            Cancel::NEVER,
            |_| {},
        );
        let batch::Outcome::Exported { path: exported, .. } = &outcomes[0] else {
            panic!("photo {i}: {:?}", outcomes[0]);
        };
        let developed = pixels(&develop);
        assert!(pixels(exported) == developed, "photo {i} differs");
        rendered.push(developed);
    }
    // Every edit shows: none of them rendered as the unedited photo (c).
    for i in [0, 1, 3, 4, 6] {
        assert!(rendered[i] != rendered[2], "photo {i} rendered unedited");
    }
    Ok(())
}
