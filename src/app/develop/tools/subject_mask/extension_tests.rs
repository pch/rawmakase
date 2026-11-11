//! The selection state machine with an extension's mask generators, end to
//! end: the test-only `dev.rawmakase.masks` extension
//! (`rawmakase_example::masks`) runs as this test binary started again with
//! [`PROGRAM`] set, and answers over shared memory as a third party's program
//! would. Each test starts one program and runs its cases in turn; only the
//! crash case starts a second.
use super::*;
use crate::app::extensions::Host;
use crate::camera_data::{CameraImage, Metadata};
use crate::storage::mask_assets;
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_extensions::session::Launch;
use rawmakase_extensions::store::Store;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Set in the environment of the program the tests start.
const PROGRAM: &str = "RAWMAKASE_EXAMPLE_MASKS";
const ID: &str = "dev.rawmakase.masks";

/// The masks extension's program, when this binary is started as it; a test
/// that does nothing otherwise.
#[test]
fn masks_program() {
    if std::env::var_os(PROGRAM).is_none() {
        return;
    }
    let code = i32::from(rawmakase_example::masks::extension().run().is_err());
    std::process::exit(code);
}

fn program() -> Launch {
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::develop::tools::subject_mask::extension_tests::masks_program",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
        dir: PathBuf::new(),
        env: vec![(PROGRAM.into(), "1".into())],
        log: PathBuf::new(),
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    editor: Editor,
    /// The extension's developer folder: its program's working folder.
    folder: PathBuf,
}

/// An editor with a decoded photo in a catalog, as Develop has one, and the
/// masks extension loaded from a developer folder.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    let photo = photos.join("image.ARW");
    std::fs::write(&photo, b"identity fixture").unwrap();
    let catalog = dir.path().join("test.rawmakase");
    let mut c = crate::catalog::Catalog::create(&catalog).unwrap();
    c.add_folder(&photos).unwrap();
    drop(c);
    let ctx = egui::Context::default();
    let library = crate::app::library::Library::load(&catalog, ctx.clone()).unwrap();
    let id = library.session.photos()[0].id;
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.library = Some(Box::new(library));
    editor.document.catalog_photo = Some(id);
    editor.document.opened_at(photo);
    editor.document.set_image(image());
    let folder = dir.path().join("masks");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join(rawmakase_extensions::protocol::manifest::FILE),
        include_str!("../../../../../extensions/example/masks/rawmakase-extension.toml"),
    )
    .unwrap();
    let data = dir.path().join("data");
    Store::new(&data).add_developer(&folder).unwrap();
    editor.extensions = Host::start_for_tests(&data, program(), Duration::from_secs(30));
    assert!(
        editor.extensions.get(ID).is_some(),
        "the extension is loaded"
    );
    Fixture {
        _dir: dir,
        editor,
        folder,
    }
}

/// The photo: 8 by 6 pixels of an even grey.
fn image() -> Arc<CameraImage> {
    let metadata = Metadata {
        width: 8,
        height: 6,
        wb: [1.; 3],
        daylight_wb: [1.; 3],
        matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
        ..Default::default()
    };
    Arc::new(CameraImage {
        recovered: Default::default(),
        width: 8,
        height: 6,
        pixels: vec![[0.2; 3]; 48],
        metadata,
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

/// The request tile `name` makes, for a new mask.
fn tile(e: &Editor, name: &str) -> Request {
    let (offer, why) = e
        .extensions
        .mask_tiles()
        .into_iter()
        .find(|(o, _)| o.name == name)
        .unwrap_or_else(|| panic!("no tile {name}"));
    assert_eq!(why, None);
    Request {
        generator: Generator::Extension(offer),
        target: Target::NewMask,
    }
}

/// Runs frames, as the app does, until `done` holds, or fails after 20 s.
fn pump(e: &mut Editor, what: &str, done: impl Fn(&Editor) -> bool) {
    let ctx = e.context.clone();
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        e.events(&ctx);
        e.extensions_frame(&ctx);
        if done(e) {
            return;
        }
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}: {}",
            e.messages.line()
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn settled(e: &Editor) -> bool {
    e.selection.running().is_none()
}

/// The bytes the extension's requests hold in shared memory.
fn leased(e: &Editor) -> u64 {
    e.extensions.mask_ledger(ID).unwrap().in_use()
}

/// Whether the status line shows a selection running.
fn busy_row(e: &Editor) -> bool {
    e.running_work().iter().any(|w| w.key == "selection")
}

/// Waits for `gate` or `late` to be running in the program, then lets it
/// answer when `release` is called.
fn started(e: &mut Editor, folder: &Path) {
    pump(e, "the request to reach the program", |_| {
        folder.join("started").exists()
    });
    std::fs::remove_file(folder.join("started")).unwrap();
}
fn release(folder: &Path) {
    std::fs::write(folder.join("release"), b"").unwrap();
}
fn hold(folder: &Path) {
    let _ = std::fs::remove_file(folder.join("release"));
}

fn raster(e: &Editor, mask: usize, component: usize) -> BitmapMask {
    match &e.document.edit.recipe().masks[mask].components[component].shape {
        MaskShape::Bitmap(b) => b.clone(),
        other => panic!("not a raster: {other:?}"),
    }
}

#[test]
fn an_extensions_tiles_make_masks_as_select_subject_does() {
    let mut f = fixture();
    let e = &mut f.editor;
    let names: Vec<String> = e
        .extensions
        .mask_tiles()
        .into_iter()
        .map(|(o, _)| o.name)
        .collect();
    assert!(names.starts_with(&["Blob".into(), "Not Blob".into(), "Flat".into()]));
    // Automatic: the left half of the photo, as one mask and one History step.
    let blob = tile(e, "Blob");
    e.request_selection(blob.clone());
    assert!(e.selection.running().is_some() && busy_row(e));
    assert!(
        e.running_work()
            .iter()
            .any(|w| w.text == "Finding the blob…")
    );
    pump(e, "Blob", settled);
    assert!(!busy_row(e));
    let masks = &e.document.edit.recipe().masks;
    assert_eq!(masks.len(), 1, "{}", e.messages.line());
    assert_eq!(masks[0].name, "Blob");
    assert!(!masks[0].components[0].invert);
    let b = raster(e, 0, 0);
    let source = b.source.clone().unwrap();
    assert_eq!(
        (source.feature.as_str(), source.model.as_str()),
        ("dev.rawmakase.masks/blob", "example-blob@1")
    );
    let asked = &e.extensions.masks_sent()[0];
    assert_eq!((b.width, b.height), (asked.image.width, asked.image.height));
    assert_eq!(
        source.input,
        mask_generator::input_identity(asked, "example-blob/1")
    );
    let pixels = mask_assets::resolve(&b.id).unwrap();
    let w = b.width as usize;
    for (i, v) in pixels.data.iter().enumerate() {
        assert_eq!(*v, if i % w < w.div_ceil(2) { 255 } else { 0 }, "pixel {i}");
    }
    assert_eq!(e.document.edit.history().steps().1, 1);
    assert_eq!(e.document.edit.history().steps().0[0].name, "Select Blob");
    // Its label and its generator come from its provenance.
    assert_eq!(e.raster_source_text(&b), "Made by Example Masks (blob)");
    assert_eq!(e.raster_generator(&b, false), Ok(blob.generator));
    // What the request held is let go of once it is kept.
    pump(e, "the buffers", |e| leased(e) == 0);

    // A Background-style tile: an inverted component of the same generator.
    e.request_selection(tile(e, "Not Blob"));
    pump(e, "Not Blob", settled);
    let masks = &e.document.edit.recipe().masks;
    assert_eq!((masks.len(), masks[1].name.as_str()), (2, "Not Blob"));
    assert!(masks[1].components[0].invert && !masks[1].invert);
    let not_blob = raster(e, 1, 0);
    let Ok(Generator::Extension(offer)) = e.raster_generator(&not_blob, true) else {
        panic!("its generator")
    };
    assert_eq!(
        (offer.name.as_str(), offer.invert, offer.generator.as_str()),
        ("Not Blob", true, "blob")
    );

    // A generator's mask joins another as a component.
    let mut flat = tile(e, "Flat");
    flat.target = Target::Component {
        mask: 0,
        op: MaskOp::Subtract,
    };
    e.request_selection(flat);
    pump(e, "Flat", settled);
    let c = &e.document.edit.recipe().masks[0].components;
    assert_eq!((c.len(), c[1].op), (2, MaskOp::Subtract));
    // Extensions' generators aren't aimed with yet: nothing to refine with.
    e.begin_refining(0, 1);
    assert!(e.selection.prompting.is_none());

    // Regenerate finds the generator from the raster and keeps the inversion.
    // (Regenerated as it was, the raster would be the same, and no step.)
    let other = raster(e, 0, 1);
    e.change_edit(None, |r| {
        r.masks[1].components[0].shape = MaskShape::Bitmap(other);
    });
    let generator = e.raster_generator(&not_blob, true).unwrap();
    e.request_selection(Request {
        generator,
        target: Target::Regenerate {
            mask: 1,
            component: 0,
        },
    });
    pump(e, "the regeneration", settled);
    assert!(e.document.edit.recipe().masks[1].components[0].invert);
    assert_eq!(raster(e, 1, 0), not_blob);
    let (steps, count) = e.document.edit.history().steps();
    assert_eq!(steps[count - 1].name, "Refine Not Blob");
    assert_eq!(e.document.edit.recipe().masks.len(), 2);
    pump(e, "the buffers", |e| leased(e) == 0);
}

#[test]
fn an_extensions_failures_and_stale_answers_record_nothing() {
    let mut f = fixture();
    let folder = f.folder.clone();
    let e = &mut f.editor;
    for (name, failure) in [
        ("Nothing", Failure::NothingFound),
        ("Absent", Failure::NoSubject),
        (
            "Unavailable",
            Failure::RuntimeUnavailable("no runtime here".into()),
        ),
    ] {
        e.request_selection(tile(e, name));
        pump(e, name, settled);
        assert_eq!(e.selection.failure.as_ref().map(|(_, f)| f), Some(&failure));
        assert!(e.document.edit.recipe().masks.is_empty() && !busy_row(e));
    }
    e.request_selection(tile(e, "Unnamed"));
    pump(e, "Unnamed", settled);
    assert!(
        matches!(&e.selection.failure, Some((_, Failure::Failed(why))) if why.contains("model")),
        "{:?}",
        e.selection.failure
    );
    assert_eq!(e.document.edit.history().steps().1, 0);
    pump(e, "the buffers", |e| leased(e) == 0);

    // A reply that comes after the request was cancelled is dropped, and
    // what it held is let go of then.
    hold(&folder);
    e.request_selection(tile(e, "Late"));
    started(e, &folder);
    assert!(leased(e) > 0);
    e.cancel_selection();
    assert!(settled(e) && !busy_row(e));
    pump(e, "the cancel to be sent", |_| true);
    release(&folder);
    pump(e, "the late reply", |e| leased(e) == 0);
    assert!(e.document.edit.recipe().masks.is_empty());

    // A new photo while it runs: the answer is dropped.
    hold(&folder);
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    e.selection.clear_document();
    assert!(settled(e));
    release(&folder);
    pump(e, "the dropped answer", |e| leased(e) == 0);
    assert!(e.document.edit.recipe().masks.is_empty());

    // Undo while it runs: the masks it was for are gone, so nothing is added.
    e.request_selection(tile(e, "Blob"));
    pump(e, "Blob", settled);
    hold(&folder);
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    assert!(e.document.edit.undo());
    release(&folder);
    pump(e, "the answer after Undo", settled);
    assert!(e.document.edit.recipe().masks.is_empty());
    assert_eq!(
        e.messages.line(),
        "The masks changed while selecting; nothing was added"
    );

    // A spot changed while it runs selects different content.
    hold(&folder);
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    e.change_edit(None, |r| {
        r.retouch.push(crate::model::retouch::RetouchOp {
            mode: Default::default(),
            shape: crate::model::retouch::RetouchShape::Spot {
                center: [0.5, 0.5],
                radius: 0.05,
            },
            feather: 0.5,
            opacity: 1.,
            offset: [0.1, 0.],
        });
    });
    release(&folder);
    pump(e, "the answer after a spot", settled);
    assert!(e.document.edit.recipe().masks.is_empty());
    pump(e, "the buffers", |e| leased(e) == 0);

    // A crash mid-request: busy clears and nothing is recorded; the next
    // request starts the program again.
    let steps = e.document.edit.history().steps().1;
    e.request_selection(tile(e, "Crash"));
    pump(e, "the crash", settled);
    assert!(!busy_row(e));
    assert!(matches!(e.selection.failure, Some((_, Failure::Failed(_)))));
    assert_eq!(e.document.edit.history().steps().1, steps);
    pump(e, "the crash's buffers", |e| leased(e) == 0);
    e.request_selection(tile(e, "Blob"));
    pump(e, "Blob after the crash", settled);
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
}

#[test]
fn a_generator_set_up_then_disabled_keeps_its_saved_masks() {
    let mut f = fixture();
    let folder = f.folder.clone();
    let e = &mut f.editor;
    // `needs_setup` shows the card with what it needs.
    let setup = tile(e, "Setup");
    let Generator::Extension(offer) = setup.generator.clone() else {
        panic!()
    };
    e.request_selection(setup.clone());
    pump(e, "Setup", settled);
    assert_eq!(
        e.selection.prompt,
        Some(Prompt::Setup {
            request: setup,
            setup: NeedsSetup {
                what: "the Example model".into(),
                bytes: 12_500_000,
            },
        })
    );
    assert!(e.document.edit.recipe().masks.is_empty());
    draw_masking_panel(e);
    // Download sends `mask.setup`; its progress shows, and Cancel stops it.
    e.setup_generator(&offer);
    pump(e, "the setup's progress", |e| {
        e.extensions.mask_setup(ID, "setup")
            == Some(&mask_generator::Setup::Running {
                done: Some(0.5),
                text: Some("Halfway".into()),
            })
    });
    draw_masking_panel(e);
    e.extensions.cancel_mask_setup(ID, "setup");
    assert_eq!(e.extensions.mask_setup(ID, "setup"), None);
    // Download again, and once it is done the selection goes on by itself.
    e.setup_generator(&offer);
    pump(e, "the setup to start", |e| {
        matches!(
            e.extensions.mask_setup(ID, "setup"),
            Some(mask_generator::Setup::Running { done: Some(_), .. })
        )
    });
    std::fs::write(folder.join("finish-setup"), b"").unwrap();
    pump(e, "the mask after setup", |e| {
        e.document.edit.recipe().masks.len() == 1 && settled(e)
    });
    assert_eq!(e.selection.prompt, None);
    let b = raster(e, 0, 0);

    // Disabling it while it runs: busy clears, nothing is recorded and its
    // tiles go.
    hold(&folder);
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    e.set_extension_enabled(ID, false);
    assert!(settled(e) && !busy_row(e));
    assert!(e.extensions.mask_tiles().is_empty());
    release(&folder);
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    draw_masking_panel(e);
    // Its saved mask keeps its label and still renders; what would regenerate
    // it says why it can't.
    assert_eq!(e.raster_source_text(&b), "Made by Example Masks (setup)");
    assert_eq!(
        e.raster_generator(&b, false),
        Err("Example Masks is disabled".into())
    );
    let mut recipe = e.document.edit.recipe().clone();
    recipe.masks[0].adjust.exposure = 2.;
    let image = e.document.full().cloned().unwrap();
    let render = |recipe: &Recipe| {
        crate::develop::render(&image, &recipe.checked().unwrap(), 400, Cancel::NEVER).unwrap()
    };
    let (base, lit) = (render(&Recipe::default()), render(&recipe));
    let changed = base
        .pixels
        .iter()
        .zip(&lit.pixels)
        .filter(|(a, b)| (a[1] - b[1]).abs() > 0.02)
        .count();
    assert!(changed > 0 && changed < base.pixels.len(), "{changed}");
    // A missing extension: the label says so.
    assert_eq!(
        e.raster_generator(
            &BitmapMask {
                source: Some(BitmapSource {
                    feature: "dev.someone.gone/people".into(),
                    model: "m@1".into(),
                    input: "i".into(),
                }),
                ..b
            },
            false
        ),
        Err("Made by dev.someone.gone, which isn't installed".into())
    );
}

fn draw_masking_panel(e: &mut Editor) {
    let ctx = e.context.clone();
    let frame = e.begin_edit_frame();
    let mut output = ctx.run_ui(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400., 900.),
            )),
            ..Default::default()
        },
        |ui| e.mask_panel(ui),
    );
    output.textures_delta.clear();
    e.finish_edit_frame(frame, &ctx);
}
