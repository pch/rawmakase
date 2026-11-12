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
use rawmakase_extensions::protocol::viewer::{Bounds, Click, MAX_CLICKS};
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
    /// The data folder, with `extensions.json`.
    data: PathBuf,
}

/// An editor with a decoded photo of grey `level` in a catalog, as Develop has
/// one, and the masks extension loaded from a developer folder.
///
/// The raster store is the process's, shared by the tests running at once,
/// and a raster's id is its content: each test uses a level of its own, so
/// the generators' coverage, and their rasters, are its own too, and one
/// test's discarding a result can't take another's raster away.
fn fixture(level: f32) -> Fixture {
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
    editor.document.set_image(image(level));
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
        data,
    }
}

/// The photo: 8 by 6 pixels of an even grey.
fn image(level: f32) -> Arc<CameraImage> {
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
        pixels: vec![[level; 3]; 48],
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
        generator: offer,
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

/// What the `n`th request sent asked, with its input, once it is sent: after
/// the worker has rendered its input.
fn sent(e: &mut Editor, n: usize) -> Option<ViewerInput> {
    pump(e, "the request to be sent", |e| sent_count(e) > n);
    e.extensions.masks_sent()[n].input.clone()
}
fn sent_count(e: &Editor) -> usize {
    e.extensions.masks_sent().len()
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

fn prompt(clicks: &[Click], bounds: Option<[f32; 4]>) -> Option<ViewerInput> {
    Some(ViewerInput::Prompt {
        clicks: clicks.to_vec(),
        bounds: bounds.map(|[x0, y0, x1, y1]| Bounds { x0, y0, x1, y1 }),
    })
}
const fn at(x: f32, y: f32, positive: bool) -> Click {
    Click { x, y, positive }
}

#[test]
fn an_extensions_tiles_make_masks_as_select_subject_does() {
    let mut f = fixture(0.12);
    let e = &mut f.editor;
    // Beside the bundled AI masks' tiles, which this host can't run.
    let names: Vec<String> = e
        .extensions
        .mask_tiles()
        .into_iter()
        .filter(|(o, _)| o.extension == ID)
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
    let ink = pixels.data[0];
    assert!(ink > 0);
    for (i, v) in pixels.data.iter().enumerate() {
        assert_eq!(*v, if i % w < w.div_ceil(2) { ink } else { 0 }, "pixel {i}");
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
    let Ok(offer) = e.raster_generator(&not_blob, true) else {
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
    // Flat takes no clicks: nothing to refine it with.
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
fn aiming_an_extensions_generator_sends_what_the_gestures_made() {
    let mut f = fixture(0.6);
    let e = &mut f.editor;
    e.begin_aiming(tile(e, "Blob"));
    assert!(e.selection.prompting.is_some());
    assert_eq!(e.messages.line(), "Click the blob, or drag a box around it");
    // A box, then clicks: one prompt carrying the box and every click.
    e.prompt_box([0.8, 0.9], [0.2, 0.1]);
    // Clicks and boxes while it runs are ignored, not queued.
    e.prompt_click([0.5, 0.5], true);
    e.prompt_box([0.1, 0.1], [0.9, 0.9]);
    let bounds = Some([0.2, 0.1, 0.8, 0.9]);
    assert_eq!(sent(e, 0), prompt(&[], bounds));
    pump(e, "the box", settled);
    assert_eq!(sent_count(e), 1);
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    e.prompt_click([0.5, 0.5], true);
    assert_eq!(sent(e, 1), prompt(&[at(0.5, 0.5, true)], bounds));
    // Every result after the first refines the mask the first one made.
    let refine = Target::Regenerate {
        mask: 0,
        component: 0,
    };
    assert_eq!(e.selection.running().unwrap().target, refine);
    pump(e, "the click", settled);
    e.prompt_click([0.3, 0.4], false);
    assert_eq!(
        sent(e, 2),
        prompt(&[at(0.5, 0.5, true), at(0.3, 0.4, false)], bounds)
    );
    pump(e, "the second click", settled);
    let (steps, count) = e.document.edit.history().steps();
    assert_eq!(count, 3);
    assert_eq!(steps[2].name, "Refine Blob");
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    // The cap: asked again without the extra click.
    e.selection.prompting.as_mut().unwrap().input.clicks = vec![at(0.5, 0.5, true); MAX_CLICKS];
    e.prompt_click([0.1, 0.1], true);
    let Some(ViewerInput::Prompt { clicks, .. }) = sent(e, 3) else {
        panic!("a prompt")
    };
    assert_eq!(clicks.len(), MAX_CLICKS);
    assert!(!clicks.contains(&at(0.1, 0.1, true)));
    pump(e, "the capped click", settled);
    // A drag under a hundredth of the photo is not a box; a box starts over.
    e.prompt_box([0.5, 0.2], [0.509, 0.8]);
    assert_eq!(sent_count(e), 4);
    e.prompt_box([0.1, 0.1], [0.4, 0.4]);
    assert_eq!(sent(e, 4), prompt(&[], Some([0.1, 0.1, 0.4, 0.4])));
    pump(e, "the new box", settled);
    e.selection.end_prompting();

    // Leaving out first, on a thin raster, sends its extent as the box.
    let mut data = vec![0; 200];
    data[0] = 200;
    let thin = mask_assets::register(crate::storage::bitmaps::Bitmap {
        width: 200,
        height: 1,
        channels: 1,
        depth: 1,
        data,
    })
    .unwrap();
    let mut b = raster(e, 0, 0);
    (b.id, b.width, b.height) = (thin, 200, 1);
    e.change_edit(None, |r| {
        r.masks[0].components[0].shape = MaskShape::Bitmap(b);
    });
    e.begin_refining(0, 0);
    let Some(p) = &e.selection.prompting else {
        panic!("refining")
    };
    assert_eq!(p.request.generator, tile(e, "Blob").generator);
    e.prompt_click([0.002, 0.5], false);
    assert_eq!(
        sent(e, 5),
        prompt(&[at(0.002, 0.5, false)], Some([0.0, 0.0, 0.005, 1.0]))
    );
    pump(e, "the thin extent", settled);
    e.selection.end_prompting();

    // Aiming with a Background-style tile keeps the component inverted.
    e.begin_aiming(tile(e, "Not Blob"));
    e.prompt_click([0.5, 0.5], true);
    pump(e, "Not Blob", settled);
    let masks = &e.document.edit.recipe().masks;
    assert_eq!((masks.len(), masks[1].name.as_str()), (2, "Not Blob"));
    assert!(masks[1].components[0].invert);
    // Done ends aiming.
    e.selection.end_prompting();
    assert!(e.selection.prompting.is_none());
    pump(e, "the buffers", |e| leased(e) == 0);
}

#[test]
fn an_extensions_failures_and_stale_answers_record_nothing() {
    let mut f = fixture(0.2);
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
    let mut f = fixture(0.28);
    let folder = f.folder.clone();
    let e = &mut f.editor;
    // `needs_setup` shows the card with what it needs.
    let setup = tile(e, "Setup");
    let offer = setup.generator.clone();
    let guard = Guard::of(e.document.edit.recipe(), setup.target);
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
            guard,
            then: Resume::Select,
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

    // Disabling it while it runs and is aimed with: busy clears, nothing is
    // recorded, aiming ends and its tiles go.
    e.begin_aiming(tile(e, "Blob"));
    hold(&folder);
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    e.set_extension_enabled(ID, false);
    assert!(settled(e) && !busy_row(e));
    assert!(e.selection.prompting.is_none());
    assert!(
        e.extensions
            .mask_tiles()
            .iter()
            .all(|(o, _)| o.extension != ID)
    );
    release(&folder);
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    draw_masking_panel(e);
    // Its saved mask keeps its label and still renders; what would refine it
    // says why it can't.
    assert_eq!(e.raster_source_text(&b), "Made by Example Masks (setup)");
    assert_eq!(
        e.raster_generator(&b, false),
        Err("Example Masks is disabled".into())
    );
    e.begin_refining(0, 0);
    assert!(e.selection.prompting.is_none());
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

/// An event of the selection thread's own test jobs, which nothing applies:
/// no photo has this load.
fn nothing() -> crate::app::worker::Event {
    mask_generator::done(u64::MAX, 0, Err(Failure::Cancelled))
}

/// Holds the selection thread, which runs its jobs in order, until the
/// returned sender is dropped: what is queued after waits.
fn hold_worker(e: &mut Editor) -> std::sync::mpsc::Sender<()> {
    let (release, held) = std::sync::mpsc::channel::<()>();
    let (tx, ctx) = (e.tx.clone(), e.context.clone());
    e.selection.worker.call(
        move || {
            let _ = held.recv_timeout(Duration::from_secs(20));
            nothing()
        },
        nothing,
        tx,
        ctx,
    );
    release
}

/// Waits until the selection thread has run every job queued before, then
/// handles what they sent and runs a frame.
fn drain_worker(e: &mut Editor) {
    let (done, ran) = std::sync::mpsc::channel();
    let (tx, ctx) = (e.tx.clone(), e.context.clone());
    e.selection.worker.call(
        move || {
            let _ = done.send(());
            nothing()
        },
        nothing,
        tx,
        ctx.clone(),
    );
    ran.recv_timeout(Duration::from_secs(20))
        .expect("the selection thread ran its jobs");
    e.events(&ctx);
    e.extensions_frame(&ctx);
}

/// Whether the running request has its reply, and its finish is queued.
fn answered(e: &Editor) -> bool {
    matches!(
        &e.selection.pending,
        Some(Pending {
            call: Call { sent: None, .. },
            ..
        })
    )
}

#[test]
fn an_extension_stopped_in_any_phase_of_a_request_voids_it() {
    let mut f = fixture(0.36);
    let (folder, data) = (f.folder.clone(), f.data.clone());
    let e = &mut f.editor;
    // The extension's ledger, by its id, whatever entry it has.
    let ledger = e.extensions.mask_ledger(ID).unwrap();
    // Disabled while its input is being prepared: idle at once, and enabled
    // again before the preparation ends, nothing is sent or applied.
    let release = hold_worker(e);
    e.request_selection(tile(e, "Blob"));
    assert!(!settled(e));
    e.set_extension_enabled(ID, false);
    assert!(settled(e) && !busy_row(e));
    e.set_extension_enabled(ID, true);
    drop(release);
    drain_worker(e);
    assert!(e.extensions.masks_sent().is_empty());
    assert!(e.document.edit.recipe().masks.is_empty() && settled(e));
    assert_eq!(ledger.in_use(), 0);

    // Disabled, uninstalled or loaded again after a successful reply, while
    // its raster is being kept: nothing is recorded.
    let manifest = folder.join(rawmakase_extensions::protocol::manifest::FILE);
    for change in ["disable", "uninstall", "reload"] {
        let sent = e.extensions.masks_sent().len();
        e.request_selection(tile(e, "Blob"));
        pump(e, "the request to be sent", |e| {
            e.extensions.masks_sent().len() > sent
        });
        let release = hold_worker(e);
        pump(e, "the reply", answered);
        match change {
            "disable" => e.set_extension_enabled(ID, false),
            "uninstall" => e.uninstall_extension(ID),
            _ => {
                let text = std::fs::read_to_string(&manifest).unwrap();
                std::fs::write(&manifest, text.replace("for tests.", "for tests!")).unwrap();
                e.reload_extensions();
                // Loaded again behind the editor's back: void at the next frame.
                let ctx = e.context.clone();
                e.extensions_frame(&ctx);
            }
        }
        assert!(settled(e) && !busy_row(e), "{change}");
        drop(release);
        drain_worker(e);
        assert!(e.document.edit.recipe().masks.is_empty(), "{change}");
        assert_eq!(e.document.edit.history().steps().1, 0, "{change}");
        pump(e, "the buffers", |_| ledger.in_use() == 0);
        match change {
            "disable" => e.set_extension_enabled(ID, true),
            "uninstall" => {
                Store::new(&data).add_developer(&folder).unwrap();
                e.reload_extensions();
            }
            _ => {}
        }
        assert!(
            e.extensions.mask_ready(&offer(e, "Blob")).is_ok(),
            "{change}"
        );
    }
    // Loaded again, it still works.
    e.request_selection(tile(e, "Blob"));
    pump(e, "Blob", settled);
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
}

fn offer(e: &Editor, name: &str) -> Offer {
    tile(e, name).generator
}

#[test]
fn a_setup_finished_after_its_mask_moved_doesnt_go_on() {
    let mut f = fixture(0.44);
    let folder = f.folder.clone();
    let e = &mut f.editor;
    e.request_selection(tile(e, "Blob"));
    pump(e, "Blob", settled);
    e.request_selection(tile(e, "Flat"));
    pump(e, "Flat", settled);
    // Asked for mask 0, which then goes: mask 1 takes its index.
    let mut setup = tile(e, "Setup");
    setup.target = Target::Component {
        mask: 0,
        op: MaskOp::Add,
    };
    e.request_selection(setup);
    pump(e, "Setup", settled);
    assert!(matches!(e.selection.prompt, Some(Prompt::Setup { .. })));
    e.setup_generator(&offer(e, "Setup"));
    pump(e, "the setup to start", |e| {
        matches!(
            e.extensions.mask_setup(ID, "setup"),
            Some(mask_generator::Setup::Running { done: Some(_), .. })
        )
    });
    e.change_edit(
        Some(crate::app::history::Step::new("Delete Mask", "")),
        |r| {
            r.masks.remove(0);
        },
    );
    let flat = e.document.edit.recipe().masks[0].clone();
    let sent = e.extensions.masks_sent().len();
    std::fs::write(folder.join("finish-setup"), b"").unwrap();
    pump(e, "the setup to finish", |e| {
        e.extensions.mask_setup(ID, "setup").is_none() && e.selection.prompt.is_none()
    });
    assert!(settled(e));
    assert_eq!(e.extensions.masks_sent().len(), sent, "not asked again");
    assert_eq!(e.document.edit.recipe().masks, [flat]);
    assert_eq!(
        e.messages.line(),
        "The masks changed while setting up; choose where to use Setup again"
    );
}

#[test]
fn rasters_set_aside_are_let_go_of_when_a_selection_ends_without_a_result() {
    let mut f = fixture(0.52);
    let folder = f.folder.clone();
    let e = &mut f.editor;
    // A result from before, its raster registered: while another runs, it is
    // set aside rather than dropped, since the running one may share it.
    let stale = |e: &mut Editor, seed: u8| {
        let id = mask_assets::register(crate::storage::bitmaps::Bitmap {
            width: 3,
            height: 1,
            channels: 1,
            depth: 1,
            data: vec![seed, 52, 52],
        })
        .unwrap();
        let done = Done {
            load: e.document.load_id(),
            generation: e.selection.task.id() - 1,
            result: Ok(Generated {
                id: id.clone(),
                width: 3,
                height: 1,
                source: BitmapSource {
                    feature: "dev.rawmakase.masks/blob".into(),
                    model: "m@1".into(),
                    input: "i".into(),
                },
            }),
        };
        e.selection_done(done);
        assert!(mask_assets::is_held(&id), "set aside");
        id
    };
    hold(&folder);
    // Cancel ends it.
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    let a = stale(e, 1);
    e.cancel_selection();
    assert!(!mask_assets::is_held(&a));
    // So does another photo, at the next frame.
    e.request_selection(tile(e, "Gate"));
    started(e, &folder);
    let b = stale(e, 2);
    e.selection.clear_document();
    let ctx = e.context.clone();
    e.extensions_frame(&ctx);
    assert!(!mask_assets::is_held(&b));
    // And a request given up on before it was sent, at its stale preparation.
    let release = hold_worker(e);
    e.request_selection(tile(e, "Blob"));
    let c = stale(e, 3);
    e.selection.cancel();
    drop(release);
    drain_worker(e);
    assert!(!mask_assets::is_held(&c));
    release_and_settle(e, &folder);
}

/// Lets the gates still waiting answer, and waits for their buffers.
fn release_and_settle(e: &mut Editor, folder: &Path) {
    release(folder);
    pump(e, "the buffers", |e| leased(e) == 0);
}

#[test]
fn switching_tools_while_a_setup_runs_drops_the_aiming_it_would_resume() {
    let mut f = fixture(0.68);
    let folder = f.folder.clone();
    let e = &mut f.editor;
    let mask = |e: &mut Editor| {
        pump(e, "the queue", settled);
        e.document.edit.recipe().masks.clone()
    };
    // A saved mask of a generator whose model isn't set up here.
    let id = mask_assets::register(crate::storage::bitmaps::Bitmap {
        width: 8,
        height: 6,
        channels: 1,
        depth: 1,
        data: [vec![68; 24], vec![0; 24]].concat(),
    })
    .unwrap();
    let shape = MaskShape::Bitmap(BitmapMask {
        id,
        width: 8,
        height: 6,
        sampling: BITMAP_SAMPLING,
        source: Some(BitmapSource {
            feature: "dev.rawmakase.masks/setup".into(),
            model: "example-setup@1".into(),
            input: "saved".into(),
        }),
    });
    e.change_edit(None, |r| {
        r.masks.push(MaskGroup {
            name: "Setup".into(),
            components: vec![MaskComponent::new(shape)],
            ..Default::default()
        });
    });
    let saved = mask(e);
    // Refining it asks for setup at the first click.
    e.begin_refining(0, 0);
    assert!(e.selection.prompting.is_some());
    e.prompt_click([0.5, 0.5], true);
    pump(e, "the click", settled);
    assert!(matches!(
        e.selection.prompt,
        Some(Prompt::Setup {
            then: Resume::Aim,
            ..
        })
    ));
    assert!(e.selection.prompting.is_none());
    e.setup_generator(&offer(e, "Setup"));
    pump(e, "the setup to start", |e| {
        matches!(
            e.extensions.mask_setup(ID, "setup"),
            Some(mask_generator::Setup::Running { done: Some(_), .. })
        )
    });
    // The user goes to Crop while it downloads.
    let tool = crate::app::develop::state::Tool::Crop;
    e.view.toggle(tool);
    let ctx = e.context.clone();
    e.background(&ctx);
    assert!(matches!(
        e.selection.prompt,
        Some(Prompt::Setup {
            then: Resume::Nothing,
            ..
        })
    ));
    // The download finishes, and nothing takes Crop away.
    let sent = e.extensions.masks_sent().len();
    std::fs::write(folder.join("finish-setup"), b"").unwrap();
    pump(e, "the setup to finish", |e| {
        e.extensions.mask_setup(ID, "setup").is_none() && e.selection.prompt.is_none()
    });
    e.background(&ctx);
    assert_eq!(e.view.tool(), tool);
    assert!(e.selection.prompting.is_none());
    assert_eq!(e.extensions.masks_sent().len(), sent, "nothing sent");
    assert_eq!(mask(e), saved);
    assert_eq!(e.messages.line(), "the Example model installed");
    // Set up, it aims at once the next time.
    e.begin_refining(0, 0);
    assert!(e.selection.prompting.is_some());
}

#[test]
fn a_setup_finishing_before_the_pass_that_would_drop_its_aiming_leaves_the_tool() {
    let mut f = fixture(0.76);
    let folder = f.folder.clone();
    let e = &mut f.editor;
    // Aiming with a generator not set up here: the first click asks for it.
    e.begin_aiming(tile(e, "Setup"));
    e.prompt_click([0.5, 0.5], true);
    pump(e, "the click", settled);
    assert!(matches!(
        e.selection.prompt,
        Some(Prompt::Setup {
            then: Resume::Aim,
            ..
        })
    ));
    e.setup_generator(&offer(e, "Setup"));
    pump(e, "the setup to start", |e| {
        matches!(
            e.extensions.mask_setup(ID, "setup"),
            Some(mask_generator::Setup::Running { done: Some(_), .. })
        )
    });
    // The user goes to Crop, and the setup's answer is handled before any
    // pass's cleanup has seen the switch.
    let tool = crate::app::develop::state::Tool::Crop;
    e.view.toggle(tool);
    let sent = e.extensions.masks_sent().len();
    std::fs::write(folder.join("finish-setup"), b"").unwrap();
    pump(e, "the setup to finish", |e| {
        e.extensions.mask_setup(ID, "setup").is_none() && e.selection.prompt.is_none()
    });
    assert_eq!(e.view.tool(), tool);
    assert!(e.selection.prompting.is_none());
    // The pass after changes nothing either.
    let ctx = e.context.clone();
    e.background(&ctx);
    assert_eq!(e.view.tool(), tool);
    assert!(e.selection.prompting.is_none());
    assert_eq!(e.extensions.masks_sent().len(), sent, "nothing sent");
    assert!(e.document.edit.recipe().masks.is_empty());
    assert_eq!(e.messages.line(), "the Example model installed");
}
