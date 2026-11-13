//! Select Subject, Sky and Background through the bundled AI masks extension
//! (`rawmakase.ai-masks`), whose program is this test binary started again
//! with [`PROGRAM`] set: the real extension, its models a stand-in
//! (`rawmakase_ai_masks::fake`) that answers from the photo's pixels and the
//! clicks, so every case runs with no model, runtime or remote worker
//! present.
//!
//! The state machine's own cases start a request and hand it a result
//! (`begin`, `finish`), and start no program. The pins of what each gesture
//! asks (the request the extension was sent) and what the edit becomes run
//! through the program; each test starts one and runs its cases in turn.
use super::*;
use crate::app::Editor;
use crate::app::extensions::Host;
use crate::app::extensions::viewer_input::Collector;
use crate::camera_data::{CameraImage, Metadata};
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets;
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_extensions::protocol::message::NeedsSetup;
use rawmakase_extensions::protocol::viewer::{Click, MAX_CLICKS};
use rawmakase_extensions::session::Launch;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Set in the environment of the program the tests start.
const PROGRAM: &str = "RAWMAKASE_AI_MASKS_TEST";
/// The program's remote worker, when it has one.
const ENDPOINT: &str = "RAWMAKASE_AI_MASKS_TEST_ENDPOINT";
/// The app's local data folder, as the program sees it.
const LOCAL: &str = "RAWMAKASE_AI_MASKS_TEST_LOCAL";
/// Where the program writes the runtime variables it was started with.
const REPORT: &str = "RAWMAKASE_AI_MASKS_TEST_REPORT";
/// Set when the program runs the real models.
const REAL: &str = "RAWMAKASE_AI_MASKS_TEST_REAL";
const ID: &str = rawmakase_ai_masks::ID;

/// The AI masks extension's program, when this binary is started as it,
/// with the stand-in models and an install that writes their files at once;
/// a test that does nothing otherwise.
#[test]
fn ai_masks_program() {
    if std::env::var_os(PROGRAM).is_none() {
        return;
    }
    let var = |name| std::env::var_os(name).unwrap_or_default();
    if let Some(report) = std::env::var_os(REPORT) {
        let runtime = rawmakase_inference::runtime::HOST_RUNTIME_DIRS;
        let text = format!(
            "{:?}\n{:?}\n",
            std::env::var_os(runtime),
            std::env::var_os(rawmakase_inference::RUNTIME_ENV)
        );
        std::fs::write(report, text).unwrap();
    }
    let engine: Box<dyn rawmakase_ai_masks::Engine> = match std::env::var_os(REAL) {
        Some(_) => Box::new(rawmakase_ai_masks::Onnx::default()),
        None => Box::new(rawmakase_ai_masks::fake::Fake::default()),
    };
    let options = rawmakase_ai_masks::Options {
        engine,
        install: Box::new(rawmakase_ai_masks::fake::install),
        local_data: PathBuf::from(var(LOCAL)),
        remote: rawmakase_ai_masks::Remote::Fixed(
            std::env::var(ENDPOINT).ok().filter(|e| !e.is_empty()),
        ),
    };
    let code = i32::from(rawmakase_ai_masks::extension(options).run().is_err());
    std::process::exit(code);
}

fn program(local: &Path, endpoint: Option<&str>, report: Option<&Path>) -> Launch {
    let mut env = vec![(PROGRAM.into(), "1".into()), (LOCAL.into(), local.into())];
    if let Some(endpoint) = endpoint {
        env.push((ENDPOINT.into(), endpoint.into()));
    }
    if let Some(report) = report {
        env.push((REPORT.into(), report.into()));
    }
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::develop::tools::subject_mask::tests::ai_masks_program",
            "--exact",
            "--nocapture",
            "--test-threads=1",
        ]
        .map(Into::into)
        .to_vec(),
        dir: PathBuf::new(),
        env,
        log: PathBuf::new(),
    }
}

/// An editor with a decoded photo in a catalog, as Develop has one, and the
/// bundled AI masks extension, which starts at its first request.
struct Fixture {
    _dir: tempfile::TempDir,
    editor: Editor,
    photo: PathBuf,
    /// The app's data folder, with `extensions.json`.
    data: PathBuf,
}

/// How a fixture's extension runs.
#[derive(Clone, Copy, Default)]
struct Setup<'a> {
    /// Its models are installed.
    models: bool,
    /// Its remote worker.
    endpoint: Option<&'a str>,
    /// Where the program reports the runtime variables it was started with.
    report: Option<&'a Path>,
}

/// [`fixture_with`] a photo of grey `level` and the models installed.
///
/// The raster store is the process's, shared by the tests running at once,
/// and a raster's id is its content: each test uses a level of its own, so
/// the stand-in models' coverage, and their rasters, are its own too.
fn fixture(level: f32) -> Fixture {
    fixture_with(
        level,
        Setup {
            models: true,
            ..Default::default()
        },
    )
}

fn fixture_with(level: f32, setup: Setup<'_>) -> Fixture {
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
    editor.document.opened_at(photo.clone());
    editor.document.set_image(image(level));
    let data = dir.path().join("data");
    let local = dir.path().join("local");
    if setup.models {
        rawmakase_ai_masks::fake::install_all(&local);
    }
    editor.extensions = Host::with_bundled_for_tests(
        &data,
        ID,
        program(&local, setup.endpoint, setup.report),
        rawmakase_extensions::session::QUEUE,
    );
    assert!(editor.extensions.get(ID).is_some(), "AI masks is bundled");
    Fixture {
        _dir: dir,
        editor,
        photo,
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

/// The tile `name` (Subject, Sky or Background) offers, asking for `target`.
fn tile(e: &Editor, name: &str, target: Target) -> Request {
    let (offer, why) = e
        .extensions
        .mask_tiles()
        .into_iter()
        .find(|(o, _)| o.name == name)
        .unwrap_or_else(|| panic!("no tile {name}"));
    assert_eq!(why, None);
    Request {
        generator: offer,
        target,
    }
}
fn subject(e: &Editor) -> Request {
    tile(e, "Subject", Target::NewMask)
}
fn sky(e: &Editor) -> Request {
    tile(e, "Sky", Target::NewMask)
}
fn background(e: &Editor) -> Request {
    tile(e, "Background", Target::NewMask)
}
const REFINE: Target = Target::Regenerate {
    mask: 0,
    component: 0,
};

/// A raster of the kind a selection registers (distinct per `seed`).
fn generated(seed: u8) -> Generated {
    let id = mask_assets::register(Bitmap {
        width: 4,
        height: 3,
        channels: 1,
        depth: 1,
        data: (0..12).map(|i| seed.wrapping_add(i)).collect(),
    })
    .unwrap();
    Generated {
        id,
        width: 4,
        height: 3,
        source: BitmapSource {
            feature: FEATURE_SUBJECT.into(),
            model: "test@0".into(),
            input: format!("{seed}"),
        },
    }
}
/// Starts a selection as a request would, without sending it.
fn begin(editor: &mut Editor, request: Request) -> u64 {
    let (generation, cancel) = editor.selection.task.start();
    let guard = Guard::of(editor.document.edit.recipe(), request.target);
    let lifecycle = editor
        .extensions
        .mask_lifecycle(&request.generator.extension)
        .unwrap();
    editor.selection.pending = Some(Pending {
        request,
        guard,
        call: Call {
            load: editor.document.load_id(),
            generation,
            cancel,
            lifecycle,
            sent: None,
        },
    });
    generation
}
fn finish(editor: &mut Editor, generation: u64, result: Result<Generated, Failure>) {
    let load = editor.document.load_id();
    editor.selection_done(Done {
        load,
        generation,
        result,
    });
}
/// Clicks and a box, as aiming collects them.
fn collector(clicks: Vec<Click>, bounds: Option<[f32; 4]>) -> Collector {
    let mut input = Collector::default();
    input.clicks = clicks;
    input.bounds = bounds;
    input
}
const fn at(x: f32, y: f32, positive: bool) -> Click {
    Click { x, y, positive }
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
/// What the extension was asked so far: each request's generator and input.
fn asked(e: &Editor) -> Vec<(String, Option<ViewerInput>)> {
    e.extensions
        .masks_sent()
        .iter()
        .map(|m| (m.generator.clone(), m.input.clone()))
        .collect()
}
/// The `n`th request's clicks and box, once it is sent.
fn click(e: &mut Editor, n: usize) -> (Vec<Click>, Option<[f32; 4]>) {
    pump(e, "the request to be sent", |e| asked(e).len() > n);
    match &asked(e)[n] {
        (generator, Some(ViewerInput::Prompt { clicks, bounds })) => {
            assert_eq!(generator, rawmakase_ai_masks::SUBJECT);
            (clicks.clone(), bounds.map(|b| [b.x0, b.y0, b.x1, b.y1]))
        }
        other => panic!("asked {other:?}, not clicks"),
    }
}
/// Waits for the running request's answer and applies it.
fn answered(e: &mut Editor) {
    pump(e, "the answer", settled);
}
fn aim(e: &mut Editor, request: Request) {
    e.begin_aiming(request);
    assert!(e.selection.prompting.is_some(), "{}", e.messages.line());
}
/// The raster of component `component` of mask `mask`.
fn raster(e: &Editor, mask: usize, component: usize) -> BitmapMask {
    match &e.document.edit.recipe().masks[mask].components[component].shape {
        MaskShape::Bitmap(b) => b.clone(),
        other => panic!("not a raster: {other:?}"),
    }
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

#[test]
fn a_selection_becomes_one_named_mask_and_one_history_step() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    e.view.set_tool(crate::app::develop::state::Tool::Mask);
    e.view.masking.overlay = false;
    let g = begin(e, subject(e));
    let renders = e.preview.task.id();
    finish(e, g, Ok(generated(10)));
    // The mask is selected with its overlay, and the photo rendered again after that so
    // the overlay is in it (a neutral mask changes no pixel otherwise).
    assert!(
        e.preview.task.id() >= renders + 2,
        "no render after selecting"
    );
    assert!(matches!(
        e.overlay(),
        crate::app::worker::Overlay::Mask { index: 0, .. }
    ));
    let masks = &e.document.edit.recipe().masks;
    assert_eq!(masks.len(), 1);
    assert_eq!(masks[0].name, "Subject");
    assert!(masks[0].adjust.is_neutral());
    let MaskShape::Bitmap(b) = &masks[0].components[0].shape else {
        panic!("not a raster")
    };
    assert_eq!((b.width, b.height, b.sampling), (4, 3, BITMAP_SAMPLING));
    assert!(!masks[0].components[0].invert);
    assert_eq!(e.document.edit.history().steps().1, 1);
    assert_eq!(
        e.document.edit.history().steps().0[0].name,
        "Select Subject"
    );
    assert_eq!(e.view.masking.selected, Some(0));
    assert!(e.view.masking.overlay);
    assert!(e.document.edit.save_state().needs_save());
    assert!(e.selection.running().is_none());
    // Undo removes it in one step.
    assert!(e.document.edit.undo());
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn background_is_an_inverted_component_not_an_inverted_mask() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, background(e));
    finish(e, g, Ok(generated(20)));
    let mask = &e.document.edit.recipe().masks[0];
    assert_eq!(mask.name, "Background");
    assert!(mask.components[0].invert && !mask.invert);
    assert_eq!(
        e.document.edit.history().steps().0[0].name,
        "Select Background"
    );
}

#[test]
fn a_selection_can_join_a_mask_as_a_component() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
    finish(e, g, Ok(generated(30)));
    let request = tile(
        e,
        "Background",
        Target::Component {
            mask: 0,
            op: MaskOp::Subtract,
        },
    );
    let g = begin(e, request);
    finish(e, g, Ok(generated(31)));
    let mask = &e.document.edit.recipe().masks[0];
    assert_eq!(mask.components.len(), 2);
    assert_eq!(mask.components[1].op, MaskOp::Subtract);
    assert!(mask.components[1].invert);
    assert_eq!(e.view.masking.component, Some(1));
}

#[test]
fn failures_and_cancellation_record_no_edit() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
    finish(e, g, Err(Failure::NoSubject));
    assert!(e.document.edit.recipe().masks.is_empty());
    assert_eq!(e.document.edit.history().steps().1, 0);
    assert!(!e.document.edit.save_state().needs_save());
    assert_eq!(
        e.selection.failure.as_ref().map(|f| &f.1),
        Some(&Failure::NoSubject)
    );
    assert_eq!(e.messages.line(), "No person or animal found");
    assert!(e.selection.running().is_none());

    e.selection.failure = None;
    let g = begin(e, subject(e));
    finish(e, g, Err(Failure::Cancelled));
    assert!(e.selection.failure.is_none() && e.selection.running().is_none());
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn an_obsolete_result_is_dropped_and_cannot_finish_a_newer_request() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let old = begin(e, subject(e));
    let new = begin(e, subject(e));
    assert_ne!(old, new);
    // The first one's success and failure both arrive late. Its raster is kept while
    // the newer selection runs, which could produce the same one.
    let late = generated(40);
    let late_id = late.id.clone();
    finish(e, old, Ok(late));
    finish(e, old, Err(Failure::Failed("late".into())));
    assert!(e.document.edit.recipe().masks.is_empty());
    assert!(mask_assets::is_held(&late_id));
    let held_late = late_id;
    assert!(
        e.selection.running().is_some(),
        "the newer request is still wanted"
    );
    assert!(e.selection.failure.is_none());
    let taken = generated(41);
    let taken_id = taken.id.clone();
    finish(e, new, Ok(taken));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert!(mask_assets::is_held(&taken_id));
    // Once the newer one finished with another raster, the late one is let go of.
    assert!(!mask_assets::is_held(&held_late));
    // Undone, the raster stays: History still names it.
    assert!(e.document.edit.undo());
    e.discard_raster(&taken_id);
    assert!(mask_assets::is_held(&taken_id));
    assert!(e.document.edit.jump(1));

    // A result for another photo is dropped.
    let g = begin(e, subject(e));
    let wrong_load = e.document.load_id() + 5;
    e.selection_done(Done {
        load: wrong_load,
        generation: g,
        result: Ok(generated(42)),
    });
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    // Cancelling stops it being applied even if the worker had finished, and with
    // nothing running its raster is let go of.
    e.selection.cancel();
    let stray = generated(43);
    let stray_id = stray.id.clone();
    finish(e, g, Ok(stray));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert!(
        !mask_assets::is_held(&stray_id),
        "a result nobody took stays in memory"
    );
}

#[test]
fn a_result_is_not_applied_over_masks_that_changed_meanwhile() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
    // The user adds a mask while the model runs.
    e.change_edit(None, |r| {
        r.masks.push(MaskGroup {
            components: vec![MaskComponent::new(MaskShape::Linear {
                from: [0.; 2],
                to: [1.; 2],
            })],
            ..Default::default()
        })
    });
    let rejected = generated(50);
    let rejected_id = rejected.id.clone();
    finish(e, g, Ok(rejected));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert!(!mask_assets::is_held(&rejected_id));
    assert!(matches!(
        e.document.edit.recipe().masks[0].components[0].shape,
        MaskShape::Linear { .. }
    ));
    assert!(e.messages.line().contains("changed"));

    // Sliders, names and visibility are not structure: they survive an accepted result.
    let g = begin(e, subject(e));
    e.change_edit(None, |r| {
        r.exposure = 1.;
        r.masks[0].adjust.exposure = 0.5;
        r.masks[0].name = "Sky".into();
        r.masks[0].hidden = true;
    });
    finish(e, g, Ok(generated(51)));
    let r = e.document.edit.recipe();
    assert_eq!(r.masks.len(), 2);
    assert_eq!(
        (
            r.exposure,
            r.masks[0].adjust.exposure,
            r.masks[0].name.as_str()
        ),
        (1., 0.5, "Sky")
    );
}

#[test]
fn spots_changed_while_selecting_invalidate_the_result() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
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
    finish(e, g, Ok(generated(60)));
    assert!(e.document.edit.recipe().masks.is_empty());
    // Changed and changed back is a different input, not the same one.
    let g = begin(e, subject(e));
    let before = e.document.edit.recipe().retouch.clone();
    e.change_edit(None, |r| r.retouch.clear());
    e.change_edit(None, |r| r.retouch = before);
    finish(e, g, Ok(generated(61)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
}

#[test]
fn regeneration_replaces_only_the_raster() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
    finish(e, g, Ok(generated(70)));
    e.change_edit(None, |r| {
        r.masks[0].adjust.exposure = 0.7;
        r.masks[0].invert = true;
        r.masks[0].components[0].opacity = 0.5;
        r.masks[0].components[0].op = MaskOp::Intersect;
    });
    let old = e.document.edit.recipe().masks[0].components[0]
        .shape
        .clone();
    let regenerate = tile(e, "Subject", REFINE);
    let g = begin(e, regenerate.clone());
    finish(e, g, Ok(generated(71)));
    let m = &e.document.edit.recipe().masks[0];
    assert_ne!(m.components[0].shape, old);
    assert_eq!(
        (
            m.adjust.exposure,
            m.invert,
            m.components[0].opacity,
            m.components[0].op
        ),
        (0.7, true, 0.5, MaskOp::Intersect)
    );
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Refine Subject"
    );

    // A failed regeneration leaves the old result in place.
    let before = e.document.edit.recipe().clone();
    let g = begin(e, regenerate.clone());
    finish(e, g, Err(Failure::Failed("boom".into())));
    assert_eq!(*e.document.edit.recipe(), before);

    // A correction to the shape while it runs cancels the result.
    let g = begin(e, regenerate);
    e.change_edit(None, |r| {
        r.masks[0].components[0].shape = MaskShape::Linear {
            from: [0.; 2],
            to: [1.; 2],
        }
    });
    let edited = e.document.edit.recipe().clone();
    finish(e, g, Ok(generated(72)));
    assert_eq!(*e.document.edit.recipe(), edited);
}

#[test]
fn a_new_photo_forgets_the_selection_that_was_running() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
    e.selection.clear_document();
    assert!(e.selection.running().is_none());
    finish(e, g, Ok(generated(80)));
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn a_selected_mask_survives_saving_and_reopening_without_the_store() {
    let mut f = fixture(0.2);
    let id;
    {
        let e = &mut f.editor;
        let g = begin(e, subject(e));
        let made = generated(90);
        id = made.id.clone();
        finish(e, g, Ok(made));
        assert!(e.flush());
        assert!(!e.document.edit.save_state().needs_save());
    }
    // The raster is in the catalog now; the process forgets it and reads it back.
    let catalog = f.editor.library.as_ref().unwrap().session.catalog();
    let loaded = catalog
        .load_edit(f.editor.document.catalog_photo.unwrap(), &f.photo)
        .unwrap()
        .unwrap();
    assert_eq!(
        loaded.recipe.mask_asset_ids().collect::<Vec<_>>(),
        [id.as_str()]
    );
    assert!(mask_assets::unsaved([id.as_str()]).is_empty());
    let read = catalog.mask_asset_loader().load(&id).unwrap().unwrap();
    assert_eq!(read.data, (0..12).map(|i| 90 + i).collect::<Vec<u8>>());
}

#[test]
fn the_drawer_draws_in_every_state_a_selection_can_be_in() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    draw_masking_panel(e);
    let g = begin(e, subject(e));
    draw_masking_panel(e); // running
    finish(e, g, Ok(generated(100)));
    draw_masking_panel(e); // with a generated component selected
    e.selection.failure = Some((subject(e), Failure::NoSubject));
    draw_masking_panel(e);
    e.selection.failure = None;
    e.selection.prompting = Some(Prompting {
        request: subject(e),
        input: collector(vec![at(0.5, 0.5, true)], Some([0.1, 0.1, 0.9, 0.9])),
        applied: None,
    });
    draw_masking_panel(e);
    e.selection.prompting = None;
    e.selection.prompt = Some(Prompt::Setup {
        request: subject(e),
        setup: NeedsSetup {
            what: "the selection models".into(),
            bytes: 270_000_000,
        },
        guard: Guard::of(e.document.edit.recipe(), Target::NewMask),
        then: Resume::Select,
    });
    draw_masking_panel(e);
    e.library = None;
    draw_masking_panel(e); // disabled with its reason
}

/// The drawer offers Subject, Sky and Background in that order, as it always
/// did, whatever order the extension lists them in, and each asks its own
/// generator: Background is Subject's, inverted.
#[test]
fn the_tiles_are_subject_sky_and_background() {
    let f = fixture(0.2);
    let tiles = f.editor.extensions.mask_tiles();
    let tiles = super::ui::ordered(tiles);
    let named: Vec<_> = tiles
        .iter()
        .map(|(o, _)| (o.name.as_str(), o.generator.as_str(), o.invert, o.feature()))
        .collect();
    assert_eq!(
        named,
        [
            ("Subject", "subject", false, Some(Feature::Subject)),
            ("Sky", "sky", false, Some(Feature::Sky)),
            ("Background", "subject", true, Some(Feature::Background)),
        ]
    );
    // Subject aims; Sky doesn't.
    assert!(tiles[0].0.takes_prompt() && !tiles[1].0.takes_prompt());
    assert_eq!(
        tiles[0].0.aim_hint(),
        "Click the subject on the photo, or drag a box around it"
    );
}

#[test]
fn aiming_starts_with_nothing_and_each_click_refines_the_same_mask() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    e.selection.prompting = Some(Prompting {
        request: subject(e),
        input: collector(Vec::new(), None),
        applied: None,
    });
    // Outside the photo, and a leave-out click before anything is selected, do nothing.
    e.prompt_click([1.5, 0.5], true);
    e.prompt_click([0.5, 0.5], false);
    assert!(e.selection.running().is_none());
    assert!(
        e.selection
            .prompting
            .as_ref()
            .unwrap()
            .input
            .clicks
            .is_empty()
    );
    // A first result makes the mask and remembers it; later ones replace its raster.
    let g = begin(e, subject(e));
    finish(e, g, Ok(generated(110)));
    assert_eq!(
        e.selection.prompting.as_ref().unwrap().applied,
        Some((0, 0))
    );
    let g = begin(e, tile(e, "Subject", REFINE));
    finish(e, g, Ok(generated(111)));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Refine Subject"
    );
    // Done ends aiming and leaves the mask; leaving the photo does too.
    e.selection.end_prompting();
    assert!(e.selection.prompting.is_none() && e.document.edit.recipe().masks.len() == 1);
    // Boxes too small to be a drag are ignored.
    e.selection.prompting = Some(Prompting {
        request: subject(e),
        input: collector(Vec::new(), None),
        applied: None,
    });
    e.prompt_box([0.5, 0.5], [0.501, 0.501]);
    assert!(
        e.selection
            .prompting
            .as_ref()
            .unwrap()
            .input
            .bounds
            .is_none()
    );
    e.selection.clear_document();
    assert!(e.selection.prompting.is_none());
}

#[test]
fn sky_is_a_plain_mask_of_its_own_and_background_is_the_inverted_subject() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, sky(e));
    let mut made = generated(120);
    made.source.feature = FEATURE_SKY.into();
    finish(e, g, Ok(made));
    let m = &e.document.edit.recipe().masks[0];
    assert_eq!(m.name, "Sky");
    assert!(!m.components[0].invert);
    assert_eq!(m.components[0].shape.kind(), "Sky");
    let MaskShape::Bitmap(b) = &m.components[0].shape else {
        panic!()
    };
    assert_eq!(feature_of(b, false), Feature::Sky);
    assert_eq!(feature_of(b, true), Feature::Sky);
    assert_eq!(e.document.edit.history().steps().0[0].name, "Select Sky");
    // A photo without sky says so and adds nothing.
    let g = begin(e, sky(e));
    finish(e, g, Err(Failure::NoSky));
    assert_eq!(e.messages.line(), "No sky found");
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
}

#[test]
fn a_different_mask_of_the_same_kind_in_the_same_place_is_not_the_one_asked_for() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let line = |to: f32| MaskGroup {
        components: vec![MaskComponent::new(MaskShape::Linear {
            from: [0.; 2],
            to: [to; 2],
        })],
        ..Default::default()
    };
    e.change_edit(None, |r| r.masks.push(line(1.)));
    let request = tile(
        e,
        "Subject",
        Target::Component {
            mask: 0,
            op: MaskOp::Add,
        },
    );
    let g = begin(e, request);
    // Undo, a snapshot: another linear gradient now sits where the first one was.
    e.change_edit(None, |r| r.masks[0] = line(0.5));
    finish(e, g, Ok(generated(140)));
    assert_eq!(e.document.edit.recipe().masks[0].components.len(), 1);
}

#[test]
fn a_box_starts_over_without_the_clicks_before_it() {
    let f = fixture(0.2);
    let mut p = Prompting {
        request: subject(&f.editor),
        input: collector(vec![at(0.2, 0.2, true)], None),
        applied: None,
    };
    assert!(p.input.drag([0.3, 0.3], [0.7, 0.8]));
    assert!(p.input.clicks.is_empty());
    assert_eq!(p.input.bounds, Some([0.3, 0.3, 0.7, 0.8]));
}

#[test]
fn a_stale_result_does_not_drop_the_raster_a_running_selection_may_share() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let old = begin(e, subject(e));
    let new = begin(e, subject(e));
    // Both produce the same coverage: one ID. The stale one arrives first.
    finish(e, old, Ok(generated(150)));
    let same = generated(150);
    let id = same.id.clone();
    assert!(mask_assets::is_held(&id));
    finish(e, new, Ok(same));
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert!(mask_assets::resolve(&id).is_ok());
}

#[test]
fn a_camera_exposure_change_while_selecting_invalidates_the_result() {
    let mut f = fixture(0.2);
    let e = &mut f.editor;
    let g = begin(e, subject(e));
    e.change_edit(None, |r| r.camera_exposure += 0.5);
    finish(e, g, Ok(generated(160)));
    assert!(e.document.edit.recipe().masks.is_empty());
}

#[test]
fn a_saved_mask_is_labelled_from_its_provenance_alone() {
    let source = |feature: &str| BitmapMask {
        source: Some(BitmapSource {
            feature: feature.into(),
            model: "m@0".into(),
            input: "i".into(),
        }),
        id: "x".into(),
        width: 4,
        height: 3,
        sampling: BITMAP_SAMPLING,
    };
    let subject = source(FEATURE_SUBJECT);
    let sky = source(FEATURE_SKY);
    let other = source("someone-else/thing");
    assert_eq!(
        [
            feature_of(&subject, false),
            feature_of(&subject, true),
            feature_of(&sky, true),
            feature_of(&other, false),
        ],
        [
            Feature::Subject,
            Feature::Background,
            Feature::Sky,
            Feature::Subject
        ]
    );
    assert_eq!(
        [&subject, &sky, &other].map(source_text),
        [
            "Found by the subject models",
            "Found by the sky detector",
            "A raster mask",
        ]
    );
}

/// A raster saved before extensions, by Select Subject or Sky (and one that
/// saved no provenance, which only Select Subject made), finds the bundled
/// generator again by its provenance: Subject's, named Background when
/// inverted, or Sky's. Refining is offered only by Subject's, which takes
/// clicks, as before.
#[test]
fn rasters_saved_before_extensions_find_the_bundled_generators() {
    let f = fixture(0.2);
    let e = &f.editor;
    let saved = |feature: Option<&str>| BitmapMask {
        source: feature.map(|feature| BitmapSource {
            feature: feature.into(),
            model: "sam2.1-hiera-small@0123456789abcdef".into(),
            input: "0123456789abcdef".into(),
        }),
        id: "x".into(),
        width: 4,
        height: 3,
        sampling: BITMAP_SAMPLING,
    };
    let found = |feature, invert| {
        let offer = e.raster_generator(&saved(feature), invert).unwrap();
        (offer.extension, offer.generator, offer.name, offer.invert)
    };
    let of = |generator: &str, name: &str, invert| {
        (
            ID.to_string(),
            generator.to_string(),
            name.to_string(),
            invert,
        )
    };
    assert_eq!(
        found(Some(FEATURE_SUBJECT), false),
        of("subject", "Subject", false)
    );
    assert_eq!(
        found(Some(FEATURE_SUBJECT), true),
        of("subject", "Background", true)
    );
    assert_eq!(found(Some(FEATURE_SKY), false), of("sky", "Sky", false));
    assert_eq!(found(None, false), of("subject", "Subject", false));
    assert!(
        e.raster_generator(&saved(Some(FEATURE_SUBJECT)), false)
            .unwrap()
            .takes_prompt()
    );
    assert!(
        !e.raster_generator(&saved(Some(FEATURE_SKY)), false)
            .unwrap()
            .takes_prompt()
    );
    // Labelled as before, from the provenance alone.
    assert_eq!(
        e.raster_source_text(&saved(Some(FEATURE_SUBJECT))),
        "Found by the subject models"
    );
    assert_eq!(
        e.raster_source_text(&saved(Some(FEATURE_SKY))),
        "Found by the sky detector"
    );
}

/// What the extension makes records what Select Subject and Sky recorded:
/// `subject` for Subject and Background, `sky` for Sky, the network that
/// made it, and an input identity of the same form. Automatic Subject,
/// Background and Sky each ask their own generator with no input, and the
/// coverage is the models' at the input's size.
#[test]
fn each_tile_asks_its_generator_and_records_what_made_the_mask() {
    let mut f = fixture(0.21);
    let e = &mut f.editor;
    for (n, request) in [subject(e), sky(e), background(e)].into_iter().enumerate() {
        e.request_selection(request.clone());
        answered(e);
        assert_eq!(
            e.document.edit.recipe().masks.len(),
            n + 1,
            "{}",
            e.messages.line()
        );
        let generator = if request.generator.name == "Sky" {
            "sky"
        } else {
            "subject"
        };
        assert_eq!(asked(e)[n], (generator.to_string(), None));
    }
    let masks = &e.document.edit.recipe().masks;
    assert_eq!(
        masks.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
        ["Subject", "Sky", "Background"]
    );
    let sources: Vec<_> = (0..3).map(|m| raster(e, m, 0).source.unwrap()).collect();
    assert_eq!(
        sources
            .iter()
            .map(|s| s.feature.as_str())
            .collect::<Vec<_>>(),
        [FEATURE_SUBJECT, FEATURE_SKY, FEATURE_SUBJECT]
    );
    assert!(sources[0].model.starts_with("sam2.1-hiera-small@"));
    assert!(sources[1].model.starts_with("upernet-convnext-base@"));
    assert_eq!(sources[0].model, sources[2].model);
    for s in &sources {
        assert_eq!(s.input.len(), 16, "{}", s.input);
        assert!(s.input.chars().all(|c| c.is_ascii_hexdigit()));
    }
    // Subject and Background asked the same thing of the same photo.
    assert_eq!(sources[0].input, sources[2].input);
    assert_ne!(sources[0].input, sources[1].input);
    assert!(masks[2].components[0].invert && !masks[0].components[0].invert);
    let subject = raster(e, 0, 0);
    let pixels = mask_assets::resolve(&subject.id).unwrap();
    assert_eq!(
        (pixels.width, pixels.height),
        (subject.width, subject.height)
    );
    // The stand-in's Subject is the left half of the photo.
    let row = &pixels.data[..pixels.width as usize];
    assert!(row[0] > 0 && row[row.len() - 1] == 0, "{row:?}");
}

/// Pins of what each gesture asks the generator (#965): a box, then clicks
/// that each ask for the box with every click so far; every result after the
/// first refines the mask the first made; a new box starts over.
#[test]
fn a_box_and_then_clicks_ask_for_the_box_with_every_click() {
    let mut f = fixture(0.22);
    let e = &mut f.editor;
    aim(e, subject(e));
    e.prompt_box([0.8, 0.9], [0.2, 0.1]);
    let bounds = Some([0.2, 0.1, 0.8, 0.9]);
    assert_eq!(click(e, 0), (vec![], bounds));
    assert_eq!(e.selection.pending.as_ref().unwrap().request, subject(e));
    answered(e);
    e.prompt_click([0.5, 0.5], true);
    assert_eq!(click(e, 1), (vec![at(0.5, 0.5, true)], bounds));
    // Every result after the first refines the mask the first one made.
    assert_eq!(e.selection.pending.as_ref().unwrap().request.target, REFINE);
    answered(e);
    e.prompt_click([0.3, 0.4], false);
    assert_eq!(
        click(e, 2),
        (vec![at(0.5, 0.5, true), at(0.3, 0.4, false)], bounds)
    );
    answered(e);
    assert_eq!(e.document.edit.recipe().masks.len(), 1);
    assert_eq!(e.document.edit.history().steps().1, 3, "one step each");
    // A new box starts over: it is sent without the clicks before it.
    e.prompt_box([0.1, 0.1], [0.4, 0.4]);
    assert_eq!(click(e, 3), (vec![], Some([0.1, 0.1, 0.4, 0.4])));
    answered(e);

    // Clicks and boxes while a selection runs are ignored, not queued.
    e.selection.end_prompting();
    aim(e, tile(e, "Subject", Target::NewMask));
    e.prompt_click([0.5, 0.5], true);
    e.prompt_click([0.6, 0.6], true);
    e.prompt_box([0.1, 0.1], [0.9, 0.9]);
    let p = e.selection.prompting.as_ref().unwrap();
    assert_eq!((p.input.clicks.len(), p.input.bounds), (1, None));
    assert_eq!(click(e, 4).0, [at(0.5, 0.5, true)]);
    answered(e);
    assert_eq!(asked(e).len(), 5, "nothing else was sent");
    // Once it has answered, the next click is taken, and the lost ones are not.
    e.prompt_click([0.7, 0.7], true);
    assert_eq!(click(e, 5).0, [at(0.5, 0.5, true), at(0.7, 0.7, true)]);
    answered(e);

    // Clicks past the limit ask again without the extra point.
    e.selection.prompting.as_mut().unwrap().input =
        collector(vec![at(0.5, 0.5, true); MAX_CLICKS], None);
    e.prompt_click([0.1, 0.1], true);
    let (sent, _) = click(e, 6);
    assert_eq!(sent.len(), MAX_CLICKS);
    assert!(!sent.contains(&at(0.1, 0.1, true)));
    answered(e);

    // A drag under a hundredth of the photo is not a box.
    e.prompt_box([0.5, 0.2], [0.509, 0.8]);
    e.prompt_box([0.2, 0.5], [0.8, 0.509]);
    assert!(settled(e));
    e.prompt_box([0.5, 0.2], [0.52, 0.8]);
    click(e, 7);
    answered(e);
    assert_eq!(asked(e).len(), 8);
}

/// Leaving something out first, when refining a mask already made, sends the
/// mask's extent as the box, however thin.
#[test]
fn leaving_out_first_on_a_thin_mask_sends_its_extent_as_the_box() {
    let mut f = fixture(0.23);
    let e = &mut f.editor;
    // One column of 200 selected: an extent far under the drag minimum.
    let mut data = vec![0; 200];
    data[0] = 255;
    let id = mask_assets::register(Bitmap {
        width: 200,
        height: 1,
        channels: 1,
        depth: 1,
        data,
    })
    .unwrap();
    assert_eq!(super::raster_extent(&id), Some([0.0, 0.0, 0.005, 1.0]));
    let mut made = generated(174);
    made.id = id;
    (made.width, made.height) = (200, 1);
    let g = begin(e, subject(e));
    finish(e, g, Ok(made));
    e.begin_refining(0, 0);
    e.prompt_click([0.002, 0.5], false);
    assert_eq!(
        click(e, 0),
        (vec![at(0.002, 0.5, false)], Some([0.0, 0.0, 0.005, 1.0]))
    );
    assert_eq!(e.selection.pending.as_ref().unwrap().request.target, REFINE);
    answered(e);
}

/// Nothing found, the drawer offers aiming at the subject for Background; that
/// and refining a Background ask Subject's generator, and the result stays
/// inverted.
#[test]
fn background_aimed_or_refined_by_hand_asks_for_the_subject_and_stays_inverted() {
    // An all-black photo: the stand-in finds no subject in it.
    let mut f = fixture(0.);
    let e = &mut f.editor;
    e.request_selection(background(e));
    answered(e);
    assert_eq!(
        e.selection.failure.as_ref().map(|f| &f.1),
        Some(&Failure::NoSubject)
    );
    assert_eq!(e.messages.line(), "No person or animal found");
    assert_eq!(asked(e)[0], ("subject".to_string(), None));
    draw_masking_panel(e);
    // A box, which the stand-in fills whatever the photo.
    aim(e, background(e));
    e.prompt_box([0.1, 0.1], [0.6, 0.6]);
    assert_eq!(click(e, 1), (vec![], Some([0.1, 0.1, 0.6, 0.6])));
    assert_eq!(e.selection.pending.as_ref().unwrap().request, background(e));
    answered(e);
    let c = &e.document.edit.recipe().masks[0].components[0];
    assert!(c.invert, "{}", e.messages.line());
    let MaskShape::Bitmap(b) = &c.shape else {
        panic!()
    };
    assert_eq!(feature_of(b, c.invert), Feature::Background);
    assert_eq!(e.document.edit.recipe().masks[0].name, "Background");
    e.selection.end_prompting();
    // Refining it aims at the subject again, and the result keeps the inversion.
    e.begin_refining(0, 0);
    e.prompt_box([0.2, 0.2], [0.7, 0.7]);
    click(e, 2);
    let pending = e.selection.pending.as_ref().unwrap().request.clone();
    assert_eq!(pending.generator.feature(), Some(Feature::Background));
    assert_eq!(pending.target, REFINE);
    answered(e);
    let m = &e.document.edit.recipe().masks;
    assert_eq!(m.len(), 1);
    assert!(m[0].components[0].invert);
    assert_eq!(
        e.document.edit.history().steps().0.last().unwrap().name,
        "Refine Background"
    );
}

#[test]
fn switching_tools_ends_aiming() {
    let mut f = fixture(0.24);
    let e = &mut f.editor;
    aim(e, subject(e));
    e.prompt_click([0.5, 0.5], true);
    answered(e);
    let ctx = e.context.clone();
    // A pass with the Mask tool keeps it.
    e.background(&ctx);
    assert!(e.selection.prompting.is_some());
    e.view.toggle(crate::app::develop::state::Tool::Crop);
    e.background(&ctx);
    e.view.toggle(crate::app::develop::state::Tool::Mask);
    e.background(&ctx);
    assert!(e.selection.prompting.is_none(), "the old prompt came back");
    // A click on the photo now aims at nothing.
    e.prompt_click([0.6, 0.6], true);
    assert!(settled(e));
    assert_eq!(asked(e).len(), 1);
}

/// Without the models, each selection asks for its own before it runs, as
/// the drawer always did: the download, with what and how much. Download installs them and the selection goes on
/// by itself. Sky's are its own: having Subject's doesn't install them.
/// Aiming without them asks at the first click and goes back to aiming.
#[test]
fn each_selection_asks_for_its_own_models_once_and_then_runs() {
    let mut f = fixture_with(0.25, Setup::default());
    let e = &mut f.editor;
    // The models are the first thing asked for; no download starts.
    let request = subject(e);
    let guard = Guard::of(e.document.edit.recipe(), Target::NewMask);
    e.request_selection(request.clone());
    answered(e);
    fn what(what: &'static str, megabytes: std::ops::Range<u64>) -> impl Fn(&NeedsSetup) -> bool {
        move |setup| setup.what == what && megabytes.contains(&setup.bytes.div_ceil(1_000_000))
    }
    match &e.selection.prompt {
        Some(Prompt::Setup {
            request: r,
            setup,
            guard: g,
            then: Resume::Select,
        }) if *r == request && *g == guard => {
            assert!(what("the selection models", 250..300)(setup), "{setup:?}")
        }
        other => panic!("{other:?}"),
    }
    assert!(e.document.edit.recipe().masks.is_empty());
    draw_masking_panel(e);
    // Download: the selection goes on once they are in.
    e.setup_generator(&request.generator);
    pump(e, "the mask after the download", |e| {
        e.document.edit.recipe().masks.len() == 1 && settled(e)
    });
    assert_eq!(e.selection.prompt, None);
    assert!(
        e.messages.line().contains("selected"),
        "{}",
        e.messages.line()
    );
    // Sky asks for its own.
    e.request_selection(sky(e));
    answered(e);
    match &e.selection.prompt {
        Some(Prompt::Setup { setup, .. }) => {
            assert!(what("the sky model", 200..260)(setup), "{setup:?}")
        }
        other => panic!("{other:?}"),
    }
    e.selection.prompt = None;
    // Subject's in place, aiming needs nothing more.
    aim(e, subject(e));
    e.selection.end_prompting();
    std::fs::remove_dir_all(f._dir.path().join("local/models")).unwrap();
    // Without them, aiming starts, and its first click asks for them, then
    // aims again once they are in.
    let e = &mut f.editor;
    aim(e, subject(e));
    e.prompt_click([0.5, 0.5], true);
    answered(e);
    assert!(e.selection.prompting.is_none());
    assert!(matches!(
        e.selection.prompt,
        Some(Prompt::Setup {
            then: Resume::Aim,
            ..
        })
    ));
    e.setup_generator(&subject(e).generator);
    pump(e, "aiming again", |e| e.selection.prompting.is_some());
}

/// With a remote worker configured, the models don't have to be here:
/// Subject, its clicks and Sky run on the worker, through the extension, and
/// make masks as they do here. One that is gone fails as a missing runtime
/// does, and a cancelled answer applies nothing.
#[test]
fn a_selection_runs_through_the_remote_worker_into_one_mask() {
    let (endpoint, calls) = rawmakase_ai_masks::fake::remote_worker();
    let mut f = fixture_with(
        0.26,
        Setup {
            endpoint: Some(&endpoint),
            ..Default::default()
        },
    );
    let e = &mut f.editor;
    e.request_selection(subject(e));
    answered(e);
    assert_eq!(
        e.document.edit.recipe().masks.len(),
        1,
        "{}",
        e.messages.line()
    );
    assert_eq!(e.selection.prompt, None, "nothing to download");
    let b = raster(e, 0, 0);
    assert_eq!(b.source.as_ref().unwrap().feature, FEATURE_SUBJECT);
    // A sky on the same photo, as a component of the same mask.
    e.request_selection(tile(
        e,
        "Sky",
        Target::Component {
            mask: 0,
            op: MaskOp::Add,
        },
    ));
    answered(e);
    assert_eq!(e.document.edit.recipe().masks[0].components.len(), 2);
    // Clicks refine it from the analysis the worker kept.
    e.begin_refining(0, 0);
    e.prompt_click([0.7, 0.5], true);
    answered(e);
    let calls = calls.lock().unwrap().clone();
    assert_eq!((calls.analyses, calls.skies, calls.segments), (1, 1, 1));
    assert_eq!(e.document.edit.recipe().masks[0].components.len(), 2);
    // A cancelled request applies nothing, and the drawer moves on.
    e.selection.end_prompting();
    let before = e.document.edit.recipe().clone();
    e.request_selection(background(e));
    e.cancel_selection();
    assert_eq!(e.messages.line(), "Selection cancelled");
    pump(e, "the cancel to reach the program", |e| {
        e.extensions.mask_ledger(ID).unwrap().in_use() == 0
    });
    assert_eq!(*e.document.edit.recipe(), before);
    assert!(e.selection.pending.is_none());

    // Nothing listens here.
    let mut f = fixture_with(
        0.27,
        Setup {
            endpoint: Some("127.0.0.1:1"),
            ..Default::default()
        },
    );
    let e = &mut f.editor;
    e.request_selection(subject(e));
    answered(e);
    let (_, failure) = e.selection.failure.clone().unwrap();
    assert!(
        matches!(failure, Failure::RuntimeUnavailable(_)),
        "{failure:?}"
    );
    assert_eq!(e.document.edit.recipe().masks.len(), 0);
}

/// The bundled program is told where the app looks for ONNX Runtime, and
/// inherits the user's RAWMAKASE_ORT_LIB as it is.
#[test]
fn the_program_is_told_the_apps_runtime_folders() {
    let dir = tempfile::tempdir().unwrap();
    let report = dir.path().join("report");
    let mut f = fixture_with(
        0.29,
        Setup {
            models: true,
            report: Some(&report),
            ..Default::default()
        },
    );
    let e = &mut f.editor;
    e.request_selection(subject(e));
    answered(e);
    let text = std::fs::read_to_string(&report).unwrap();
    let mut lines = text.lines();
    let dirs = std::env::join_paths(rawmakase_inference::runtime::runtime_search_dirs()).unwrap();
    assert_eq!(lines.next(), Some(format!("{:?}", Some(dirs)).as_str()));
    assert_eq!(
        lines.next(),
        Some(format!("{:?}", std::env::var_os(rawmakase_inference::RUNTIME_ENV)).as_str())
    );
}

/// A saved Subject, Sky or Background mask with the AI masks extension
/// disabled, its program missing, or no extensions at all: it renders, keeps
/// its label, and what would make it again says why it can't.
#[test]
fn saved_masks_render_and_keep_their_labels_without_the_extension() {
    let mut f = fixture(0.3);
    let e = &mut f.editor;
    e.request_selection(subject(e));
    answered(e);
    e.request_selection(sky(e));
    answered(e);
    assert_eq!(e.document.edit.recipe().masks.len(), 2);
    assert!(e.flush());
    let (subject_raster, sky_raster) = (raster(e, 0, 0), raster(e, 1, 0));
    let render = |e: &Editor, recipe: &Recipe| {
        let image = e.document.full().cloned().unwrap();
        crate::develop::render(&image, &recipe.checked().unwrap(), 400, Cancel::NEVER).unwrap()
    };
    let mut lit = e.document.edit.recipe().clone();
    lit.masks[0].adjust.exposure = 2.;
    lit.masks[1].adjust.exposure = -2.;
    let base = render(e, &Recipe::default());
    let shown = render(e, &lit);
    assert_ne!(base.pixels, shown.pixels);
    let check = |e: &mut Editor, why: &str| {
        assert_eq!(render(e, &lit).pixels, shown.pixels, "{why}");
        assert_eq!(
            e.raster_source_text(&subject_raster),
            "Found by the subject models"
        );
        assert_eq!(
            e.raster_source_text(&sky_raster),
            "Found by the sky detector"
        );
        assert_eq!(e.raster_generator(&subject_raster, false), Err(why.into()));
        assert_eq!(e.raster_generator(&sky_raster, false), Err(why.into()));
        e.begin_refining(0, 0);
        assert!(e.selection.prompting.is_none());
        e.view.masking.select_after_selection(0, 0);
        draw_masking_panel(e);
    };
    e.set_extension_enabled(ID, false);
    assert!(e.extensions.mask_tiles().is_empty());
    check(e, "AI Masks is disabled");
    // Its program missing beside the app: the tiles say why.
    e.extensions = Host::with_search_dirs_for_tests(&f.data.join("other"), Vec::new());
    let missing = format!(
        "{} is not installed beside RAWmakase",
        rawmakase_extensions::bundled::program_name("rawmakase-ai-masks")
    );
    let tiles = e.extensions.mask_tiles();
    assert_eq!(tiles.len(), 3);
    assert!(
        tiles
            .iter()
            .all(|(_, why)| why.as_deref() == Some(&*missing))
    );
    check(e, &missing);
    // No extensions at all.
    e.extensions = Host::inert();
    check(e, "Made by rawmakase.ai-masks, which isn't installed");
}

/// The built `rawmakase-ai-masks`, found beside the test binary's folder as a
/// release finds it beside the app: it starts at the first request, takes the
/// photo through shared memory and answers. It is asked what it refuses before
/// it looks for any model (Sky with clicks), so it reads and moves no model
/// folder of this computer's. Run `cargo build -p rawmakase-ai-masks` first,
/// as `make check` does.
#[test]
fn the_built_program_answers_a_request() {
    let program = crate::app::testing::built_program("rawmakase-ai-masks")
        .expect("rawmakase-ai-masks is missing: build it with cargo build -p rawmakase-ai-masks");
    let target = program.parent().unwrap().to_path_buf();
    let mut f = fixture(0.31);
    let e = &mut f.editor;
    e.extensions = Host::with_search_dirs_for_tests(&f.data, vec![target]);
    let mut aimed = sky(e);
    aimed.generator.prompt = true;
    aim(e, aimed);
    e.prompt_click([0.5, 0.5], true);
    answered(e);
    let (_, failure) = e.selection.failure.clone().expect("an answer");
    assert_eq!(failure, Failure::Failed("sky takes no such input".into()));
    pump(e, "the request's buffers to be let go of", |e| {
        e.extensions.mask_ledger(ID).unwrap().in_use() == 0
    });
    assert!(e.document.edit.recipe().masks.is_empty());
    e.extensions.stop(Instant::now() + Duration::from_secs(2));
}

/// The real models through the extension: a click on the photo's subject
/// becomes a mask that changes the render, and survives a save. The models
/// are linked in as the app's install from before, so their folder is left as
/// it is. With RAWMAKASE_REMOTE_ENDPOINT set, the same selection runs through
/// the remote worker instead.
#[test]
#[cfg(unix)]
#[ignore = "needs RAWMAKASE_TEST_MODEL, RAWMAKASE_ORT_LIB and RAWMAKASE_TEST_RAW; with \
            RAWMAKASE_REMOTE_ENDPOINT the same selection runs through the remote worker"]
fn the_real_model_selects_through_the_extension_and_the_mask_changes_the_render() {
    let model = PathBuf::from(std::env::var("RAWMAKASE_TEST_MODEL").unwrap());
    let raw = PathBuf::from(std::env::var("RAWMAKASE_TEST_RAW").unwrap());
    let endpoint = std::env::var("RAWMAKASE_REMOTE_ENDPOINT")
        .ok()
        .filter(|endpoint| !endpoint.is_empty());
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let image = Arc::new(
        crate::photo::open(&raw)
            .unwrap()
            .develop(
                crate::camera_data::Decode::full(Default::default()),
                Cancel::new(&cancel),
            )
            .unwrap(),
    );
    let mut f = fixture_with(
        0.32,
        Setup {
            endpoint: endpoint.as_deref(),
            ..Default::default()
        },
    );
    let local = f._dir.path().join("local/models");
    std::fs::create_dir_all(&local).unwrap();
    std::os::unix::fs::symlink(
        &model,
        local.join(rawmakase_ai_masks::model_folder(
            rawmakase_ai_masks::SUBJECT,
        )),
    )
    .unwrap();
    let mut launch = program(&f._dir.path().join("local"), endpoint.as_deref(), None);
    launch.env.push((REAL.into(), "1".into()));
    let e = &mut f.editor;
    e.extensions =
        Host::with_bundled_for_tests(&f.data, ID, launch, rawmakase_extensions::session::QUEUE);
    e.document.set_image(image.clone());
    aim(e, subject(e));
    e.prompt_click([0.5, 0.62], true);
    let until = Instant::now() + Duration::from_secs(120);
    let ctx = e.context.clone();
    while !settled(e) {
        e.events(&ctx);
        e.extensions_frame(&ctx);
        assert!(Instant::now() < until, "no result: {}", e.messages.line());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        e.document.edit.recipe().masks.len(),
        1,
        "{}",
        e.messages.line()
    );
    // The mask selects part of the photo: lift it and compare the two regions.
    let mut recipe = e.document.edit.recipe().clone();
    recipe.masks[0].adjust.exposure = 2.;
    let base = crate::develop::render(
        &image,
        &Recipe::default().checked().unwrap(),
        400,
        Cancel::NEVER,
    )
    .unwrap();
    let lit =
        crate::develop::render(&image, &recipe.checked().unwrap(), 400, Cancel::NEVER).unwrap();
    let changed = base
        .pixels
        .iter()
        .zip(&lit.pixels)
        .filter(|(a, b)| (a[1] - b[1]).abs() > 0.02)
        .count();
    let share = changed as f32 / base.pixels.len() as f32;
    assert!((0.01..0.9).contains(&share), "mask covers {share}");
    // And it survives a save.
    assert!(e.flush());
}
