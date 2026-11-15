//! AI Remove's state machine with an extension's retouch generators, end to
//! end: the test-only `dev.rawmakase.remove` extension
//! (`rawmakase_example::remove`) runs as this test binary started again with
//! [`PROGRAM`] set, and answers over shared memory as a third party's program
//! would. Each test starts one program; the store of patches is the
//! process's, so each uses a grey of its own and its patches are its own.
use super::*;
use crate::app::extensions::Host;
use crate::camera_data::Metadata;
use crate::model::retouch::RetouchShape;
use eframe::egui;
use rawmakase_extensions::secrets::{MemoryStore, SecretError, SecretStore};
use rawmakase_extensions::session::Launch;
use rawmakase_extensions::store::Store;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Set in the environment of the program the tests start.
const PROGRAM: &str = "RAWMAKASE_EXAMPLE_REMOVE";
const ID: &str = "dev.rawmakase.remove";
/// What a secret's value looks like in these tests: it must never appear in
/// `extensions.json`, an error or a message.
const SENTINEL: &str = "sk-SENTINEL-0123456789abcdef";

/// The remove extension's program, when this binary is started as it; a
/// test that does nothing otherwise.
#[test]
fn remove_program() {
    if std::env::var_os(PROGRAM).is_none() {
        return;
    }
    let code = i32::from(rawmakase_example::remove::extension().run().is_err());
    std::process::exit(code);
}

fn program() -> Launch {
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::develop::tools::ai_remove::tests::remove_program",
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
    folder: PathBuf,
    data: PathBuf,
    catalog: PathBuf,
    secrets: Arc<MemoryStore>,
}

/// An editor with a decoded 48 by 32 photo of grey `level` in a catalog, as
/// Develop has one, and the remove extension loaded from a developer folder.
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
    let folder = dir.path().join("remove");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join(rawmakase_extensions::protocol::manifest::FILE),
        include_str!("../../../../../extensions/example/remove/rawmakase-extension.toml"),
    )
    .unwrap();
    std::fs::create_dir(folder.join("icons")).unwrap();
    std::fs::write(
        folder.join("icons/fill.svg"),
        include_str!("../../../../../extensions/example/remove/icons/fill.svg"),
    )
    .unwrap();
    let data = dir.path().join("data");
    Store::new(&data).add_developer(&folder).unwrap();
    editor.extensions = Host::start_for_tests(&data, program(), Duration::from_secs(30));
    assert!(
        editor.extensions.get(ID).is_some(),
        "the extension is loaded"
    );
    let secrets = Arc::new(MemoryStore::default());
    editor
        .extensions
        .use_secret_store_for_tests(secrets.clone());
    Fixture {
        _dir: dir,
        editor,
        folder,
        data,
        catalog,
        secrets,
    }
}

fn image(level: f32) -> Arc<CameraImage> {
    Arc::new(CameraImage {
        recovered: Default::default(),
        width: 48,
        height: 32,
        pixels: vec![[level; 3]; 48 * 32],
        metadata: Metadata {
            width: 48,
            height: 32,
            wb: [1.; 3],
            daylight_wb: [1.; 3],
            matrix: [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]],
            ..Default::default()
        },
        fast: false,
        scale_factor: 1.,
        scale_clipped: 0,
        hdr: Default::default(),
    })
}

/// The extension's generator `id`, as the Remove tool offers it.
fn offer(e: &Editor, id: &str) -> Offer {
    let (offer, why) = e
        .extensions
        .retouch_offers()
        .into_iter()
        .find(|(o, _)| o.extension == ID && o.generator == id)
        .unwrap_or_else(|| panic!("no generator {id}"));
    assert_eq!(why, None);
    offer
}

/// A brushed area across the middle of the photo.
fn painted() -> RetouchOp {
    RetouchOp {
        mode: RetouchMode::Generate,
        shape: RetouchShape::Brush {
            points: vec![[0.4, 0.5], [0.6, 0.5]].into(),
            radius: 0.05,
        },
        feather: 0.5,
        opacity: 1.,
        offset: [0.; 2],
        patch: None,
    }
}

fn remove_with(e: &mut Editor, generator: &str) {
    let offer = offer(e, generator);
    e.request_ai_remove_with(offer, None, painted());
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
    e.ai_remove.running().is_none()
}
fn busy_row(e: &Editor) -> bool {
    e.running_work().iter().any(|w| w.key == "ai-remove")
}
fn leased(e: &Editor) -> u64 {
    e.extensions.mask_ledger(ID).unwrap().in_use()
}
fn generated(e: &Editor) -> Vec<RetouchOp> {
    e.document
        .edit
        .recipe()
        .retouch
        .iter()
        .filter(|op| op.is_generated())
        .cloned()
        .collect()
}
/// Waits for `gate` to be running in the program.
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
fn last_step(e: &Editor) -> String {
    let (steps, applied) = e.document.edit.history().steps();
    steps[applied - 1].name.to_string()
}

#[test]
fn ai_remove_asks_once_before_uploading_then_adds_a_generated_spot() {
    let mut f = fixture(0.21);
    let e = &mut f.editor;
    // `fill` uploads: the user is asked first, and No sends nothing.
    remove_with(e, "fill");
    assert!(e.ai_remove.consent.is_some() && settled(e));
    e.answer_ai_remove_consent(false);
    assert!(e.ai_remove.consent.is_none() && settled(e));
    assert!(e.extensions.retouches_sent().is_empty());

    remove_with(e, "fill");
    e.answer_ai_remove_consent(true);
    assert!(e.extensions.uploads_allowed(ID));
    let state = std::fs::read_to_string(f.data.join("extensions.json")).unwrap();
    assert!(state.contains("\"uploads_allowed\": true"), "{state}");
    assert!(e.ai_remove.running().is_some() && busy_row(e));
    pump(e, "the removal", settled);
    assert!(!busy_row(e));
    let ops = generated(e);
    assert_eq!(ops.len(), 1, "{}", e.messages.line());
    let patch = ops[0].patch.clone().unwrap();
    assert_eq!(patch.generator, "dev.rawmakase.remove/fill");
    assert_eq!(patch.model, "example-fill@1");
    assert!(!patch.ai_denoise);
    assert_eq!(ops[0].offset, [0.; 2]);
    // Named after the generator.
    assert_eq!(last_step(e), "Fill");
    // What was asked: the region of the painted area, its coverage and a
    // result buffer, all of the region's size.
    let asked = &e.extensions.retouches_sent()[0];
    assert_eq!(asked.generator, "fill");
    assert_eq!(
        (patch.width, patch.height),
        (asked.image.width, asked.image.height)
    );
    assert_eq!(patch.rect, asked.image.region.rect);
    assert!(asked.validate().is_ok());
    // The patch is camera RGB: the grey the extension painted, inverted, and
    // the photo's own level where nothing was painted.
    let bitmap = crate::storage::mask_assets::resolve_patch(&patch.id).unwrap();
    let pixels = crate::storage::patches::decode(&bitmap).unwrap();
    let rendition = crate::develop::retouch::rendition::Rendition::of(
        &image(0.21).metadata,
        &Recipe::default(),
    )
    .unwrap();
    let grey = rendition.inverse(rawmakase_example::remove::GREY);
    let centre =
        pixels[(patch.height as usize / 2) * patch.width as usize + patch.width as usize / 2];
    assert!((centre[1] - grey[1]).abs() < 1e-2, "{centre:?} {grey:?}");
    let corner = pixels[0];
    assert!((corner[1] - 0.21).abs() < 0.02, "{corner:?}");
    pump(e, "the lease to end", |e| leased(e) == 0);
    // Asked once: the next one goes straight on.
    remove_with(e, "fill");
    assert!(e.ai_remove.consent.is_none() && e.ai_remove.running().is_some());
    pump(e, "the second removal", settled);
    assert_eq!(generated(e).len(), 2);
}

/// What a generator needs beyond the area comes from its panel, drawn in its
/// tool's drawer: what is still being typed there reaches the program before
/// the request, and the answer's summary stays with its spot. A refusal (no
/// text) makes nothing and paints the area again.
#[test]
fn a_generator_s_panel_feeds_its_requests() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.22);
    let e = &mut f.editor;
    e.view.set_tool(Tool::AiRetouch);
    let ask = offer(e, "ask");
    assert_eq!(ask.panel.as_deref(), Some("ask"));
    e.choose_ai_generator(&ask);
    e.extensions.open_panel(ID, "ask").unwrap();
    pump(e, "the panel", |e| {
        e.extensions
            .panel(ID, "ask")
            .is_some_and(|v| !v.controls.is_empty())
    });
    // Nothing typed: refused, and the area painted again to try once more.
    e.add_ai_stroke(vec![[0.4, 0.5], [0.6, 0.5]]);
    e.send_ai_draft();
    assert!(e.ai_remove.draft.is_empty());
    pump(e, "the refusal", settled);
    assert!(generated(e).is_empty());
    let message = e.ai_remove.failure.as_ref().unwrap().message("Ask");
    assert!(message.contains("Say what to put there first"), "{message}");
    assert_eq!(e.ai_remove.draft.len(), 1, "painted again");
    // Typed and not committed (the field still has the keyboard): sent first.
    e.extensions
        .panel_mut(ID, "ask")
        .unwrap()
        .drafts
        .insert("text".into(), serde_json::json!("a wooden bench"));
    e.send_ai_draft();
    pump(e, "the replacement", settled);
    let ops = generated(e);
    assert_eq!(ops.len(), 1, "{}", e.messages.line());
    let patch = ops[0].patch.as_ref().unwrap();
    assert_eq!(patch.summary.as_deref(), Some("a wooden bench"));
    assert_eq!(patch.generator, "dev.rawmakase.remove/ask");
    assert_eq!(last_step(e), "Ask");
    assert!(e.ai_remove.draft.is_empty(), "made: not painted again");
}

/// Each generator is a tool of its own: the one chosen is the one asked,
/// and choosing one that is gone falls back to the first.
#[test]
fn the_generator_chosen_is_the_one_asked() {
    let mut f = fixture(0.23);
    let e = &mut f.editor;
    let offers: Vec<Offer> = e.ai_remove_offers().into_iter().map(|(o, _)| o).collect();
    assert_eq!(offers[0].generator, "fill");
    assert_eq!(e.ai_remove_offer().unwrap().0.generator, "fill");
    let ask = offer(e, "ask");
    e.choose_ai_generator(&ask);
    assert_eq!(e.ai_remove_offer().unwrap().0, ask);
    e.view.retouch.generator = Some("dev.rawmakase.remove/gone".into());
    assert_eq!(e.ai_remove_offer().unwrap().0.generator, "fill");
}

/// Each generator is a rail tool of its own, named and drawn by its
/// extension: its name is the button's tip and the drawer's title, and its
/// SVG the icon. Its button opens the tool on it, and again closes it.
#[test]
fn each_generator_is_a_rail_tool_named_and_drawn_by_its_extension() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.27);
    let e = &mut f.editor;
    let rail: Vec<String> = e.rail_generators().into_iter().map(|o| o.name).collect();
    assert_eq!(rail[..3], ["Fill", "Local Fill", "Gate"]);
    assert!(!e.rail_tools().contains(&Tool::AiRetouch));
    assert_eq!(e.drawer_caption(), None);
    let local = offer(e, "local");
    e.toggle_generator(&local.provenance);
    assert!(e.view.is(Tool::AiRetouch));
    assert_eq!(e.ai_remove_offer().unwrap().0, local);
    assert_eq!(e.drawer_caption().as_deref(), Some("Local Fill"));
    // Another generator's button moves the open tool to it.
    let fill = offer(e, "fill");
    e.toggle_generator(&fill.provenance);
    assert!(e.view.is(Tool::AiRetouch));
    assert_eq!(e.ai_remove_offer().unwrap().0, fill);
    assert_eq!(e.drawer_caption().as_deref(), Some("Fill"));
    e.toggle_generator(&fill.provenance);
    assert_eq!(e.view.tool(), Tool::None);
    assert_eq!(e.drawer_caption(), None);
    // Spot Removal keeps the app's own title.
    e.view.set_tool(Tool::Remove);
    assert_eq!(e.drawer_caption().as_deref(), Some("Spot Removal"));

    // The icon is the package's SVG, with `currentColor` drawn white; a
    // generator without one, or whose file can't be read, gets none (the
    // rail draws a generic one).
    assert_eq!(
        fill.icon.as_ref().and_then(|p| p.canonicalize().ok()),
        f.folder.join("icons/fill.svg").canonicalize().ok()
    );
    assert_eq!(local.icon, None);
    assert_eq!(e.generator_icon(&local), None);
    let uri = e.generator_icon(&fill).expect("the fill icon");
    assert!(uri.ends_with(".svg"), "{uri}");
    let Ok(egui::load::BytesPoll::Ready { bytes, .. }) = e.context.try_load_bytes(&uri) else {
        panic!("{uri} isn't loaded");
    };
    let svg = String::from_utf8(bytes.to_vec()).unwrap();
    assert!(
        svg.contains("stroke=\"#ffffff\"") && !svg.contains("currentColor"),
        "{svg}"
    );
    let mut gone = fill.clone();
    gone.icon = Some(f.folder.join("icons/gone.svg"));
    assert_eq!(e.generator_icon(&gone), None);
}

/// AI Retouch is a rail tool of its own, there only while a generator is
/// offered; it and Spot Removal each show, select and reset only their own
/// spots, all kept in one ordered list.
#[test]
fn ai_retouch_is_its_own_tool_with_its_own_spots() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.24);
    let e = &mut f.editor;
    assert!(!e.rail_generators().is_empty());
    assert!(e.rail_tools().contains(&Tool::Remove));
    e.view.set_tool(Tool::AiRetouch);
    assert!(e.ai_retouching() && e.view.retouching());

    // A Heal spot, then an AI one.
    let mut heal = painted();
    heal.mode = RetouchMode::Heal;
    heal.offset = [0.1, 0.];
    e.change_edit(None, |r| r.add_retouch(heal));
    let local = offer(e, "local");
    e.request_ai_remove_with(local, None, painted());
    pump(e, "the fill", settled);
    assert_eq!(generated(e).len(), 1, "{}", e.messages.line());
    assert_eq!(e.view.retouch.selected, Some(1));
    assert!(e.spot_in_tool(1) && !e.spot_in_tool(0));

    // Opened in the same frame as its keys, before anything drew: Delete,
    // [ ] and the wheel still never reach the AI spot it doesn't show.
    let before = e.document.edit.recipe().retouch[1].clone();
    e.view.set_tool(Tool::Remove);
    let ctx = e.context.clone();
    let frame = e.begin_edit_frame();
    e.step_retouch_size(true, true);
    e.next_source();
    e.delete_spot();
    {
        use crate::app::develop::tools::brush_scroll::{Adjust, MaskBrush, Scroll};
        for adjust in [Adjust::Size, Adjust::Feather] {
            e.scroll_tool_size(Scroll {
                lines: 1.,
                adjust,
                brush: MaskBrush::Current,
            });
        }
    }
    e.finish_edit_frame(frame, &ctx);
    assert_eq!(e.document.edit.recipe().retouch.len(), 2);
    assert_eq!(e.document.edit.recipe().retouch[1], before);
    e.view.set_tool(Tool::AiRetouch);
    e.view.retouch.selected = Some(1);

    // An index past the end isn't the other tool's spot: it stays for its
    // users' bounds checks, as it did, whenever the photo draws.
    e.view.retouch.selected = Some(9);
    e.fit_retouch_selection();
    assert_eq!(e.view.retouch.selected, Some(9));
    e.view.retouch.selected = Some(1);

    // Spot Removal lets go of the AI spot and never takes it.
    e.view.set_tool(Tool::Remove);
    assert!(!e.ai_retouching() && e.view.retouching());
    e.fit_retouch_selection();
    assert_eq!(e.view.retouch.selected, None);
    assert!(e.spot_in_tool(0) && !e.spot_in_tool(1));
    // Its Reset removes its own spots only.
    let ctx = e.context.clone();
    let frame = e.begin_edit_frame();
    e.reset_tool_spots();
    e.finish_edit_frame(frame, &ctx);
    let ops = &e.document.edit.recipe().retouch;
    assert_eq!(ops.len(), 1);
    assert!(ops[0].is_generated());

    // Without a generator the tool leaves the rail, and closes if open.
    e.view.set_tool(Tool::AiRetouch);
    e.set_extension_enabled(ID, false);
    assert!(e.rail_generators().is_empty());
    e.close_tools_gone();
    assert_eq!(e.view.tool(), Tool::None);
}

/// Painting only marks the area: strokes and clicks gather until the user
/// sends them, as one request for one spot that isn't joined across its
/// strokes.
#[test]
fn painting_gathers_an_area_that_goes_only_when_sent() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.26);
    let e = &mut f.editor;
    e.view.set_tool(Tool::AiRetouch);
    let local = offer(e, "local");
    e.choose_ai_generator(&local);
    assert!(e.ai_send_blocked().is_some(), "nothing painted");
    e.add_ai_stroke(vec![[0.3, 0.4], [0.35, 0.4]]);
    e.add_ai_stroke(vec![[0.7, 0.6]]);
    assert_eq!(e.ai_remove.draft.len(), 2);
    assert!(e.extensions.retouches_sent().is_empty() && settled(e));
    assert_eq!(e.ai_send_blocked(), None);
    e.send_ai_draft();
    assert!(e.ai_remove.draft.is_empty());
    pump(e, "the fill", settled);
    let ops = generated(e);
    assert_eq!(ops.len(), 1, "{}", e.messages.line());
    let RetouchShape::Strokes { strokes, radius } = &ops[0].shape else {
        panic!("one spot of two strokes: {:?}", ops[0].shape);
    };
    assert_eq!(strokes.len(), 2);
    assert_eq!(*radius, e.view.retouch.size);
    assert!(e.ai_remove.draft.is_empty(), "made: not painted again");
    // Clear forgets what was painted, and so does another photo.
    e.add_ai_stroke(vec![[0.5, 0.5]]);
    e.clear_ai_draft();
    assert!(e.ai_remove.draft.is_empty());
    e.add_ai_stroke(vec![[0.5, 0.5]]);
    e.ai_remove.clear_document();
    assert!(e.ai_remove.draft.is_empty());
}

/// Painting again while the upload question is open keeps the area asked
/// about: the question stays with what was painted first, never replaced.
#[test]
fn painting_while_asked_keeps_the_area_asked_about() {
    let mut f = fixture(0.21);
    let e = &mut f.editor;
    remove_with(e, "fill");
    let asked = e.ai_remove.consent.clone().unwrap().op;
    let mut other = painted();
    other.shape = RetouchShape::Spot {
        center: [0.2, 0.2],
        radius: 0.03,
    };
    let offer = offer(e, "fill");
    e.request_ai_remove_with(offer, None, other);
    assert_eq!(e.ai_remove.consent.as_ref().unwrap().op, asked);
    assert!(e.messages.line().contains("first"), "{}", e.messages.line());
    assert!(e.extensions.retouches_sent().is_empty() && settled(e));
}

/// A dark photo is sent exposed up, and its answer is inverted through that
/// exposure: where nothing was painted the patch is the photo's own level.
#[test]
fn a_dark_photo_is_inverted_through_the_exposure_it_was_sent_with() {
    let mut f = fixture(0.004);
    let e = &mut f.editor;
    remove_with(e, "fill");
    e.answer_ai_remove_consent(true);
    pump(e, "the removal", settled);
    let ops = generated(e);
    assert_eq!(ops.len(), 1, "{}", e.messages.line());
    let patch = ops[0].patch.clone().unwrap();
    let bitmap = crate::storage::mask_assets::resolve_patch(&patch.id).unwrap();
    let pixels = crate::storage::patches::decode(&bitmap).unwrap();
    let corner = pixels[0];
    assert!((corner[1] / 0.004 - 1.).abs() < 0.05, "{corner:?}");
}

#[test]
fn a_result_for_pixels_that_changed_is_dropped_and_other_edits_survive() {
    let mut f = fixture(0.23);
    let (e, folder) = (&mut f.editor, f.folder.clone());
    // A slider moved while it runs survives it.
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    e.change_edit(Some(crate::app::history::Step::new("Exposure", "")), |r| {
        r.exposure = 0.5
    });
    release(&folder);
    pump(e, "the removal", settled);
    assert_eq!(generated(e).len(), 1, "{}", e.messages.line());
    assert_eq!(e.document.edit.recipe().exposure, 0.5);
    // Named after the generator.
    assert_eq!(last_step(e), "Gate");

    // A spot added meanwhile changes the pixels it was made from: dropped,
    // never asked again by itself.
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    let sent = e.extensions.retouches_sent().len();
    e.change_edit(Some(crate::app::history::Step::new("Spot", "")), |r| {
        r.add_retouch(RetouchOp {
            mode: RetouchMode::Heal,
            offset: [0.1, 0.],
            ..painted()
        })
    });
    release(&folder);
    pump(e, "the removal", settled);
    assert_eq!(generated(e).len(), 1);
    assert_eq!(e.ai_remove.failure, Some(Failure::Failed(STALE.into())));
    assert!(
        e.messages.line().contains("paint again"),
        "{}",
        e.messages.line()
    );
    pump(e, "nothing more", |_| true);
    assert_eq!(e.extensions.retouches_sent().len(), sent);

    // So does noise reduction changing.
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    e.change_edit(Some(crate::app::history::Step::new("Noise", "")), |r| {
        r.noise_luma = 20.
    });
    release(&folder);
    pump(e, "the removal", settled);
    assert_eq!(generated(e).len(), 1);
    pump(e, "the leases to end", |e| leased(e) == 0);
}

#[test]
fn cancel_another_photo_and_quitting_end_the_request() {
    let mut f = fixture(0.25);
    let (e, folder) = (&mut f.editor, f.folder.clone());
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    e.cancel_ai_remove();
    assert!(settled(e) && !busy_row(e));
    // The program is told; its late answer is dropped and its lease ends.
    pump(e, "the cancel to reach the program", |e| leased(e) == 0);
    assert!(generated(e).is_empty());

    remove_with(e, "gate");
    started(e, &folder);
    e.ai_remove.clear_document();
    assert!(settled(e));
    pump(e, "the cancel to reach the program", |e| leased(e) == 0);

    // Quitting: the worker is handed to the shared deadline and busy clears.
    remove_with(e, "gate");
    let stopping = e.ai_remove.stop();
    assert_eq!(stopping.len(), 1);
    assert!(settled(e));
    release(&folder);
    pump(e, "the program to let go", |e| leased(e) == 0);
    assert!(generated(e).is_empty());
}

#[test]
fn the_extension_crashing_or_being_disabled_ends_the_request() {
    let mut f = fixture(0.27);
    let (e, folder) = (&mut f.editor, f.folder.clone());
    remove_with(e, "crash");
    pump(e, "the crash", settled);
    assert!(matches!(e.ai_remove.failure, Some(Failure::Failed(_))));
    assert!(generated(e).is_empty() && !busy_row(e));

    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    e.set_extension_enabled(ID, false);
    pump(e, "the request to be void", settled);
    assert!(
        e.messages.line().contains("stopped"),
        "{}",
        e.messages.line()
    );
    release(&folder);
    pump(e, "nothing more", |_| true);
    assert!(generated(e).is_empty());
    // Disabled, it isn't offered.
    assert!(e.extensions.retouch_offers().is_empty());
}

#[test]
fn a_worker_that_stops_ends_the_request() {
    let mut f = fixture(0.29);
    let e = &mut f.editor;
    remove_with(e, "local");
    let (load, generation) = (e.document.load_id(), e.ai_remove.task.id());
    // What the worker sends in a job's place when it panics or is gone.
    e.ai_remove_done(Done {
        load,
        generation,
        result: Err(stopped()),
    });
    assert!(settled(e) && !busy_row(e));
    assert_eq!(e.ai_remove.failure, Some(stopped()));
    pump(e, "the program to answer", |e| leased(e) == 0);
    assert!(generated(e).is_empty());
}

#[test]
fn failures_say_why_and_a_secret_setting_sets_the_extension_up() {
    let mut f = fixture(0.31);
    let e = &mut f.editor;
    for (generator, failure) in [
        ("refuse", "declined"),
        ("unavailable", "can't be reached"),
        ("unnamed", "model"),
        ("setup", "API key"),
    ] {
        remove_with(e, generator);
        pump(e, generator, settled);
        let message = e.ai_remove.failure.as_ref().unwrap().message("AI Remove");
        assert!(message.contains(failure), "{generator}: {message}");
        assert!(generated(e).is_empty());
    }
    assert!(matches!(e.ai_remove.failure, Some(Failure::NeedsSetup(_))));
    // It names the generator that failed, and keeps the area it failed on,
    // for the card on the photo, until dismissed or asked again.
    let (failed, area) = e.ai_remove.failed.clone().unwrap();
    assert_eq!(failed, offer(e, "setup"));
    assert_eq!(
        area,
        RetouchOp {
            mode: RetouchMode::Generate,
            ..painted()
        }
    );
    let message = e.ai_remove.failure.as_ref().unwrap().message(&failed.name);
    assert!(message.starts_with("Setup needs setting up: "), "{message}");
    // Until its settings change, painting for that extension again sends
    // nothing: it says to set it up.
    let sent = e.extensions.retouches_sent().len();
    e.view.set_tool(crate::app::develop::state::Tool::AiRetouch);
    e.choose_ai_generator(&offer(e, "fill"));
    e.add_ai_stroke(vec![[0.5, 0.5]]);
    let why = e.ai_send_blocked().unwrap();
    assert!(why.starts_with("Set up"), "{why}");
    e.clear_ai_draft();
    remove_with(e, "setup");
    assert!(settled(e));
    assert_eq!(e.extensions.retouches_sent().len(), sent);
    assert!(
        e.messages.line().contains("Set up"),
        "{}",
        e.messages.line()
    );
    // Set up elsewhere (a permission on the service), the user says so by
    // dismissing it; and the extension starting again lets go of it too.
    e.dismiss_ai_failure();
    assert!(e.ai_remove.failure.is_none() && e.ai_remove.failed.is_none());
    assert!(!e.ai_setup_needed(&offer(e, "setup")));
    remove_with(e, "setup");
    pump(e, "the second refusal", settled);
    assert!(e.ai_setup_needed(&offer(e, "setup")));
    e.set_extension_enabled(ID, false);
    e.set_extension_enabled(ID, true);
    assert!(!e.ai_setup_needed(&offer(e, "setup")));
    // A request that fails after the settings changed under it blocks
    // nothing: what it was sent with is gone.
    remove_with(e, "setup");
    e.extensions
        .set_secret(ID, "key", Some("sk-other".into()))
        .unwrap();
    pump(e, "the request to end", settled);
    assert!(!e.ai_setup_needed(&offer(e, "setup")));
    e.extensions.set_secret(ID, "key", None).unwrap();
    pump(e, "the key to be removed", |e| {
        e.extensions.get(ID).unwrap().secret("key") != Some(&secrets_state("sk-other"))
    });
    assert!(matches!(
        retouch_generator::failure(RemoteError::new(
            rawmakase_extensions::protocol::ErrorCode::Refused,
            "no"
        )),
        Failure::Refused(_)
    ));

    // The key, typed on the extension's page: kept in the store, recorded as
    // set, and sent to the program, which sets it up.
    e.extensions
        .set_secret(ID, "key", Some(SENTINEL.into()))
        .unwrap();
    pump(e, "the key to be saved", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&secrets_state(SENTINEL))
    });
    assert_eq!(
        f.secrets
            .get(&rawmakase_extensions::secrets::account(ID, "key"))
            .unwrap()
            .unwrap()
            .expose(),
        SENTINEL
    );
    let state = std::fs::read_to_string(f.data.join("extensions.json")).unwrap();
    assert!(state.contains("\"secrets\""), "{state}");
    assert!(!state.contains("SENTINEL"), "{state}");
    // What the page and every comparison read is empty.
    assert_eq!(
        e.extensions.get(ID).unwrap().settings()["key"],
        serde_json::json!("")
    );
    // The request sent while the key changed may have been set up already.
    let before = generated(e).len();
    remove_with(e, "setup");
    pump(e, "the removal", settled);
    assert_eq!(generated(e).len(), before + 1, "{}", e.messages.line());
    assert!(!e.messages.line().contains("SENTINEL"));
}

fn secrets_state(value: &str) -> crate::app::extensions::secrets::State {
    crate::app::extensions::secrets::State::Set(rawmakase_extensions::secrets::Secret::new(value))
}

#[test]
fn secret_settings_never_reach_extensions_json_or_an_error() {
    let mut f = fixture(0.33);
    let e = &mut f.editor;
    use crate::app::extensions::secrets::{State, Writing};
    // Never through the settings path, whose errors would format the value.
    let error = e
        .extensions
        .set_setting(ID, "key", serde_json::json!(SENTINEL))
        .unwrap_err();
    assert!(!error.contains("SENTINEL"), "{error}");
    // A store that isn't there: the write fails, nothing says Set, and the
    // reason says what to do; a locked one says so.
    for failure in [
        SecretError::Unavailable("no Secret Service".into()),
        SecretError::Locked("the keyring is locked".into()),
    ] {
        f.secrets.unavailable(failure.clone());
        e.extensions
            .set_secret(ID, "key", Some(SENTINEL.into()))
            .unwrap();
        pump(e, "the write to fail", |e| {
            matches!(
                e.extensions.get(ID).unwrap().secret_writes.get("key"),
                Some(Writing::Failed(_))
            )
        });
        let x = e.extensions.get(ID).unwrap();
        assert_eq!(x.secret("key"), Some(&State::Unset));
        let Some(Writing::Failed(why)) = x.secret_writes.get("key") else {
            unreachable!()
        };
        assert!(!why.contains("SENTINEL"), "{why}");
        match failure {
            SecretError::Locked(_) => assert!(why.contains("locked"), "{why}"),
            SecretError::Unavailable(_) => assert!(why.contains("environment"), "{why}"),
        }
        assert!(!e.messages.line().contains("SENTINEL"));
    }
    f.secrets.heal();
    let state = || std::fs::read_to_string(f.data.join("extensions.json")).unwrap_or_default();
    // Set, replaced and cleared are told apart.
    for (value, said) in [
        (Some(SENTINEL), "saved"),
        (Some("sk-SENTINEL-second"), "replaced"),
        (None, "removed"),
    ] {
        e.extensions
            .set_secret(ID, "key", value.map(Into::into))
            .unwrap();
        pump(e, said, |e| {
            e.extensions.get(ID).unwrap().secret_writes.is_empty()
                && e.messages.line().contains(said)
        });
        assert!(!state().contains("SENTINEL"), "{}", state());
    }
    assert_eq!(
        e.extensions.get(ID).unwrap().secret("key"),
        Some(&State::Unset)
    );
    assert!(!state().contains("\"secrets\""), "{}", state());
    assert_eq!(
        f.secrets
            .get(&rawmakase_extensions::secrets::account(ID, "key"))
            .unwrap(),
        None
    );
    // A secret's state never shows its value.
    let debug = format!("{:?}", secrets_state(SENTINEL));
    assert!(!debug.contains("SENTINEL"), "{debug}");
}

#[test]
fn ai_remove_runs_only_on_a_catalog_photo_it_can_change() {
    let mut f = fixture(0.35);
    let e = &mut f.editor;
    assert_eq!(e.ai_remove_unavailable(), None);
    e.change_edit(None, |r| r.hdr.enabled = true);
    assert_eq!(
        e.ai_remove_unavailable().as_deref(),
        Some(crate::develop::retouch::rendition::HDR_REFUSAL)
    );
    remove_with(e, "local");
    assert!(settled(e) && e.extensions.retouches_sent().is_empty());
    e.change_edit(None, |r| r.hdr.enabled = false);
    e.document.catalog_photo = None;
    assert_eq!(
        e.ai_remove_unavailable().as_deref(),
        Some("Add this photo to a catalog to use this tool")
    );
}

/// A Generate spot is fixed where its patch was made: the keys, the wheel,
/// `/` and Mode leave its shape, source and mode alone, and Mode never turns
/// a Heal spot into one.
#[test]
fn a_generated_spot_stays_where_its_patch_was_made() {
    let mut f = fixture(0.37);
    let e = &mut f.editor;
    remove_with(e, "local");
    pump(e, "the removal", settled);
    let i = e.document.edit.recipe().retouch.len() - 1;
    let before = e.document.edit.recipe().retouch[i].clone();
    assert!(before.is_generated());
    e.view.set_tool(crate::app::develop::state::Tool::AiRetouch);
    e.view.retouch.selected = Some(i);
    // As the tool runs them, inside an edit frame.
    let ctx = e.context.clone();
    let in_frame = |e: &mut Editor, f: &dyn Fn(&mut Editor)| {
        let frame = e.begin_edit_frame();
        f(e);
        e.finish_edit_frame(frame, &ctx);
    };
    in_frame(e, &|e| e.step_retouch_size(true, false));
    in_frame(e, &|e| e.next_source());
    in_frame(e, &|e| e.choose_retouch_mode(RetouchMode::Heal));
    in_frame(e, &|e| e.choose_retouch_mode(RetouchMode::Clone));
    let after = &e.document.edit.recipe().retouch[i];
    assert_eq!(
        (after.radius(), after.offset, after.mode),
        (before.radius(), before.offset, RetouchMode::Generate)
    );
    assert_eq!(after.shape, before.shape);
    // Feather still changes.
    in_frame(e, &|e| e.step_retouch_size(true, true));
    assert!(e.document.edit.recipe().retouch[i].feather > before.feather);
    // A Heal spot selected stays Heal, and Spot Removal's mode never becomes
    // Generate: that is AI Retouch's.
    e.change_edit(None, |r| {
        r.add_retouch(RetouchOp {
            mode: RetouchMode::Heal,
            offset: [0.1, 0.],
            patch: None,
            ..painted()
        })
    });
    let heal = e.document.edit.recipe().retouch.len() - 1;
    e.view.set_tool(crate::app::develop::state::Tool::Remove);
    e.view.retouch.selected = Some(heal);
    in_frame(e, &|e| e.choose_retouch_mode(RetouchMode::Generate));
    assert_eq!(
        e.document.edit.recipe().retouch[heal].mode,
        RetouchMode::Heal
    );
    assert_eq!(e.view.retouch.mode, RetouchMode::Clone);
    in_frame(e, &|e| e.choose_retouch_mode(RetouchMode::Clone));
    assert_eq!(
        e.document.edit.recipe().retouch[heal].mode,
        RetouchMode::Clone
    );
}

/// The spot is saved with its patch; read back with a patch nothing stores,
/// the edit is protected rather than shown or saved without it.
#[test]
fn a_saved_spot_keeps_its_patch_and_a_missing_one_protects_the_edit() {
    let mut f = fixture(0.39);
    let e = &mut f.editor;
    remove_with(e, "local");
    pump(e, "the removal", settled);
    let patch = generated(e)[0].patch.clone().unwrap();
    assert!(e.flush(), "{}", e.messages.line());
    let photo = e.document.catalog_photo.unwrap();
    let catalog = crate::catalog::Catalog::open(&f.catalog).unwrap();
    assert!(
        catalog
            .mask_asset_loader()
            .load(&patch.id)
            .unwrap()
            .is_some()
    );
    let missing = format!("sha256:{}", "7f".repeat(32));
    catalog
        .db_for_tests()
        .execute(
            "UPDATE local_edits SET data = replace(data, ?1, ?2) WHERE photo = ?3",
            rusqlite::params![patch.id, missing, photo.0],
        )
        .unwrap();
    drop(catalog);
    let (generation, _) = e.document.start_load();
    let path = e.document.path.clone().unwrap();
    e.tx.send(crate::app::worker::Event::Header(Box::new(
        crate::app::worker::LoadedHeader {
            id: generation,
            file: crate::storage::Identity::read(&path).ok(),
            path,
            metadata: Metadata::default(),
            recipe: Recipe::default(),
            export: Default::default(),
            status: "Original".into(),
        },
    )))
    .unwrap();
    let ctx = e.context.clone();
    e.events(&ctx);
    assert!(e.document.edit.save_state().is_protected());
    assert!(
        e.document.lightroom_notice.contains("generated spot"),
        "{}",
        e.document.lightroom_notice
    );
}

#[test]
fn the_available_list_installs_from_github() {
    for a in crate::app::extensions::AVAILABLE {
        assert!(rawmakase_extensions::protocol::manifest::is_extension_id(
            a.id
        ));
        assert!(!a.id.starts_with("rawmakase."));
        assert!(a.repository.starts_with("https://github.com/"));
    }
    assert!(
        crate::app::extensions::AVAILABLE
            .iter()
            .any(|a| a.id == "dev.pch.ai-remove")
    );
}

/// A result arriving during a slider drag lands between its two halves:
/// undoing the rest of the drag keeps the spot, and undoing the spot keeps
/// the drag's first half, as Auto does.
#[test]
fn a_removal_finishing_mid_drag_is_its_own_history_step() {
    let mut f = fixture(0.41);
    let (e, folder) = (&mut f.editor, f.folder.clone());
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    let start = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 0.5;
    let mid = e.document.edit.recipe().clone();
    e.document
        .edit
        .history_mut()
        .observe(start.clone(), &mid, true);
    release(&folder);
    pump(e, "the removal", settled);
    let with_spot = e.document.edit.recipe().clone();
    assert_eq!(generated(e).len(), 1, "{}", e.messages.line());
    assert_eq!(with_spot.exposure, 0.5);
    e.document.edit.setup_mut().exposure = 0.8;
    let end = e.document.edit.recipe().clone();
    e.document
        .edit
        .history_mut()
        .observe(with_spot.clone(), &end, false);
    e.undo();
    assert_eq!(*e.document.edit.recipe(), with_spot);
    e.undo();
    assert_eq!(*e.document.edit.recipe(), mid);
    e.undo();
    assert_eq!(*e.document.edit.recipe(), start);
}

/// Not on the half-size draft shown while the full size decodes; and a
/// switch of Detail while it runs changes the pixels it was made from.
#[test]
fn drafts_and_a_switched_off_panel_count() {
    let mut f = fixture(0.43);
    let (e, folder) = (&mut f.editor, f.folder.clone());
    let mut draft = (*image(0.43)).clone();
    draft.fast = true;
    e.document.set_image(Arc::new(draft));
    assert!(e.ai_remove_unavailable().is_some());
    remove_with(e, "local");
    assert!(settled(e) && e.extensions.retouches_sent().is_empty());
    e.document.set_image(image(0.43));
    // AI Denoise on waits for its result, unless Detail switched off bypasses it.
    e.change_edit(None, |r| r.ai_denoise.enabled = true);
    assert!(e.ai_remove_unavailable().is_some());
    e.change_edit(None, |r| {
        r.panels.set(
            crate::model::panels::Panel::Detail,
            crate::model::panels::PanelState::Off,
        )
    });
    assert_eq!(e.ai_remove_unavailable(), None);
    e.change_edit(None, |r| {
        r.ai_denoise.enabled = false;
        r.panels.set(
            crate::model::panels::Panel::Detail,
            crate::model::panels::PanelState::On,
        )
    });
    e.change_edit(None, |r| r.noise_luma = 30.);
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    e.change_edit(None, |r| {
        r.panels.set(
            crate::model::panels::Panel::Detail,
            crate::model::panels::PanelState::Off,
        )
    });
    release(&folder);
    pump(e, "the removal", settled);
    assert!(generated(e).is_empty());
    assert_eq!(e.ai_remove.failure, Some(Failure::Failed(STALE.into())));
}

/// A spot being dragged when the result lands keeps its own edit, and its
/// drag can't write over the new spot, now selected.
#[test]
fn a_spot_dragged_when_the_result_lands_keeps_the_new_spot() {
    let mut f = fixture(0.45);
    let (e, folder) = (&mut f.editor, f.folder.clone());
    let heal = RetouchOp {
        mode: RetouchMode::Heal,
        offset: [0.1, 0.],
        ..painted()
    };
    e.change_edit(None, |r| r.add_retouch(heal.clone()));
    hold(&folder);
    remove_with(e, "gate");
    started(e, &folder);
    e.view.retouch.selected = Some(0);
    e.view.retouch.drag_spot_for_tests([0.5, 0.5], heal);
    release(&folder);
    pump(e, "the removal", settled);
    assert_eq!(generated(e).len(), 1);
    assert!(!e.view.retouch.is_dragging());
}

/// A key another instance replaced is read again when extensions are
/// reloaded; a first key is recorded before it is written, and unrecorded
/// when the write fails.
#[test]
fn a_secret_is_read_again_and_recorded_before_its_first_write() {
    let mut f = fixture(0.47);
    let e = &mut f.editor;
    use crate::app::extensions::secrets::State;
    let state = || std::fs::read_to_string(f.data.join("extensions.json")).unwrap_or_default();
    f.secrets
        .unavailable(SecretError::Unavailable("none here".into()));
    e.extensions
        .set_secret(ID, "key", Some(SENTINEL.into()))
        .unwrap();
    assert!(state().contains("\"secrets\""), "{}", state());
    pump(e, "the write to fail", |e| {
        e.extensions
            .get(ID)
            .unwrap()
            .secret_writes
            .values()
            .any(|w| matches!(w, crate::app::extensions::secrets::Writing::Failed(_)))
    });
    assert!(!state().contains("\"secrets\""), "{}", state());
    f.secrets.heal();
    e.extensions
        .set_secret(ID, "key", Some(SENTINEL.into()))
        .unwrap();
    pump(e, "the key to be saved", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&secrets_state(SENTINEL))
    });
    let account = rawmakase_extensions::secrets::account(ID, "key");
    f.secrets
        .set(
            &account,
            &rawmakase_extensions::secrets::Secret::new("sk-SENTINEL-other"),
        )
        .unwrap();
    e.extensions.reload();
    pump(e, "the key to be read again", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&secrets_state("sk-SENTINEL-other"))
    });
    f.secrets.delete(&account).unwrap();
    e.extensions.reload();
    pump(e, "the key to be gone", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&State::Unset)
    });
}

/// A store whose reads wait while its gate is closed, and whose writes fail
/// while `fail` is set: to order the credentials thread's work in a test.
#[derive(Default)]
struct Gated {
    inner: MemoryStore,
    closed: std::sync::Mutex<bool>,
    opened: std::sync::Condvar,
    fail: std::sync::atomic::AtomicBool,
    /// Deletes fail too, while set.
    fail_deletes: std::sync::atomic::AtomicBool,
}
impl Gated {
    fn close(&self) {
        *self.closed.lock().unwrap() = true;
    }
    fn open(&self) {
        *self.closed.lock().unwrap() = false;
        self.opened.notify_all();
    }
}
impl SecretStore for Gated {
    fn get(
        &self,
        account: &str,
    ) -> Result<Option<rawmakase_extensions::secrets::Secret>, SecretError> {
        let mut closed = self.closed.lock().unwrap();
        while *closed {
            closed = self.opened.wait(closed).unwrap();
        }
        drop(closed);
        self.inner.get(account)
    }
    fn set(
        &self,
        account: &str,
        value: &rawmakase_extensions::secrets::Secret,
    ) -> Result<(), SecretError> {
        if self.fail.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SecretError::Unavailable("refused".into()));
        }
        self.inner.set(account, value)
    }
    fn delete(&self, account: &str) -> Result<(), SecretError> {
        if self.fail_deletes.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(SecretError::Unavailable("locked".into()));
        }
        self.inner.delete(account)
    }
}

/// A read still on its way when another instance removes the key never
/// brings it back; a replacement that fails while the key is still being
/// read settles rather than staying Loading.
#[test]
fn stale_reads_never_bring_a_key_back_and_a_failed_write_settles() {
    use crate::app::extensions::secrets::State;
    let mut f = fixture(0.49);
    let e = &mut f.editor;
    let gated = Arc::new(Gated::default());
    e.extensions.use_secret_store_for_tests(gated.clone());
    e.extensions
        .set_secret(ID, "key", Some(SENTINEL.into()))
        .unwrap();
    pump(e, "the key to be saved", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&secrets_state(SENTINEL))
    });
    // A read again, held; then the key is unrecorded elsewhere.
    gated.close();
    e.extensions.reload();
    rawmakase_extensions::state::State::update(&f.data, |s| {
        s.entry(ID).secrets.remove("key");
        Ok(())
    })
    .unwrap();
    e.extensions.reload();
    assert_eq!(
        e.extensions.get(ID).unwrap().secret("key"),
        Some(&State::Unset)
    );
    gated.open();
    pump(e, "the stale read to land", |_| true);
    std::thread::sleep(Duration::from_millis(50));
    pump(e, "nothing more", |_| true);
    assert_eq!(
        e.extensions.get(ID).unwrap().secret("key"),
        Some(&State::Unset)
    );

    // Recorded again with the key kept: being read (held), then a replacement
    // that fails.
    rawmakase_extensions::state::State::update(&f.data, |s| {
        s.entry(ID).secrets.insert("key".into());
        Ok(())
    })
    .unwrap();
    gated.close();
    e.extensions.reload();
    assert_eq!(
        e.extensions.get(ID).unwrap().secret("key"),
        Some(&State::Loading)
    );
    gated.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    e.extensions
        .set_secret(ID, "key", Some("sk-SENTINEL-new".into()))
        .unwrap();
    gated.open();
    pump(e, "the key to settle", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&secrets_state(SENTINEL))
    });
}

/// Uninstalling an extension takes its keys with it: from the system's
/// credential store, off the UI thread, and from `extensions.json`, so none
/// is left behind unrecorded for a later install to miss.
#[test]
fn uninstalling_takes_the_keys_with_it() {
    let mut f = fixture(0.41);
    let e = &mut f.editor;
    e.extensions
        .set_secret(ID, "key", Some(SENTINEL.into()))
        .unwrap();
    pump(e, "the key to be saved", |e| {
        e.extensions.get(ID).unwrap().secret("key") == Some(&secrets_state(SENTINEL))
    });
    let account = rawmakase_extensions::secrets::account(ID, "key");
    assert!(f.secrets.get(&account).unwrap().is_some());
    e.extensions.uninstall(ID).unwrap();
    pump(e, "the key to be deleted", |_| {
        f.secrets.get(&account).unwrap().is_none()
    });
    let state = std::fs::read_to_string(f.data.join("extensions.json")).unwrap();
    assert!(!state.contains("\"secrets\""), "{state}");
    pump(e, "the key to leave the list", |_| {
        Store::new(&f.data).forgotten_keys().unwrap().is_empty()
    });
    // One an earlier run couldn't delete (a locked keyring, a quit first)
    // is deleted when the host starts with a store again.
    let left = rawmakase_extensions::secrets::account("dev.gone.extension", "token");
    f.secrets
        .set(
            &left,
            &rawmakase_extensions::secrets::Secret::new("sk-left"),
        )
        .unwrap();
    rawmakase_extensions::state::State::update(&f.data, |state| {
        state.forget_keys("dev.gone.extension", ["token".to_string()]);
        Ok(())
    })
    .unwrap();
    let e = &mut f.editor;
    e.extensions.use_secret_store_for_tests(f.secrets.clone());
    pump(e, "the left key to be deleted", |_| {
        f.secrets.get(&left).unwrap().is_none()
            && Store::new(&f.data).forgotten_keys().unwrap().is_empty()
    });
}

/// The AI Retouch drawer as drawn, with a generator's panel, a painted area
/// and a selected spot, for looking at while designing it.
#[test]
#[ignore = "writes an image to look at"]
fn render_the_ai_retouch_drawer() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.3);
    // One generator, as the AI Retouch extension has: its panel, no Mode
    // over generators.
    let manifest =
        include_str!("../../../../../extensions/example/remove/rawmakase-extension.toml");
    let header = &manifest[..manifest.find("[[retouch-generator]]").unwrap()];
    let ask = &manifest[manifest
        .find("[[retouch-generator]]\nid = \"ask\"")
        .unwrap()..];
    std::fs::write(
        f.folder
            .join(rawmakase_extensions::protocol::manifest::FILE),
        format!("{header}{ask}"),
    )
    .unwrap();
    let e = &mut f.editor;
    e.extensions.reload_changed_folders();
    e.extensions.reload();
    let ctx = e.context.clone();
    crate::app::install_fonts(&ctx, &fastframe_text::TextRendering::platform_default());
    crate::app::ui::icons::install(&ctx);
    e.module = crate::app::Module::Develop;
    e.view.set_tool(Tool::AiRetouch);
    let ask = offer(e, "ask");
    e.choose_ai_generator(&ask);
    e.extensions.open_panel(ID, "ask").unwrap();
    pump(e, "the panel", |e| {
        e.extensions
            .panel(ID, "ask")
            .is_some_and(|v| !v.controls.is_empty())
    });
    e.add_ai_stroke(vec![[0.4, 0.5], [0.6, 0.5]]);
    let mut textures = Default::default();
    let image = crate::app::ui::ui_snapshot::render(
        &ctx,
        &mut textures,
        egui::Vec2::new(300., 560.),
        2.,
        4,
        |ui| {
            let _labels = crate::app::ui::widgets::LabelColumn::enter(
                ui,
                crate::app::develop::panels::inspector::DRAWER_LABEL_COLUMN,
            );
            let palette = crate::app::ui::theme::palette(ui.ctx());
            ui.painter().rect_filled(
                ui.max_rect(),
                0.,
                palette.tone(crate::app::ui::design::Tone::CARD),
            );
            let frame = e.begin_edit_frame();
            e.tool_drawer(ui);
            e.finish_edit_frame(frame, &ctx);
        },
    );
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/ai-retouch-drawer.png");
    image.save(&out).unwrap();
    // Sent with nothing typed: refused, the area painted again.
    e.send_ai_draft();
    pump(e, "the refusal", settled);
    let image = crate::app::ui::ui_snapshot::render(
        &ctx,
        &mut textures,
        egui::Vec2::new(300., 600.),
        2.,
        4,
        |ui| {
            let _labels = crate::app::ui::widgets::LabelColumn::enter(
                ui,
                crate::app::develop::panels::inspector::DRAWER_LABEL_COLUMN,
            );
            let palette = crate::app::ui::theme::palette(ui.ctx());
            ui.painter().rect_filled(
                ui.max_rect(),
                0.,
                palette.tone(crate::app::ui::design::Tone::CARD),
            );
            let frame = e.begin_edit_frame();
            e.tool_drawer(ui);
            e.finish_edit_frame(frame, &ctx);
        },
    );
    let out = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/ai-retouch-drawer-refused.png");
    image.save(&out).unwrap();
}

/// A key saved and its extension uninstalled before the save finished: the
/// save's completion is the user's write, not the delete, so a delete that
/// fails leaves the key listed for the next start.
#[test]
fn a_save_finishing_never_stands_for_the_delete_after_it() {
    let mut f = fixture(0.42);
    let e = &mut f.editor;
    let store = Arc::new(Gated::default());
    e.extensions.use_secret_store_for_tests(store.clone());
    store
        .fail_deletes
        .store(true, std::sync::atomic::Ordering::SeqCst);
    e.extensions
        .set_secret(ID, "key", Some(SENTINEL.into()))
        .unwrap();
    e.extensions.uninstall(ID).unwrap();
    pump(e, "the delete to fail", |e| {
        e.messages.line().contains("couldn't be removed")
    });
    let left = Store::new(&f.data).forgotten_keys().unwrap();
    assert_eq!(left, [(ID.to_string(), "key".to_string())]);
}

/// A send whose panel values can't reach the program (too large for a
/// message) sends nothing: the request would go with what the program had.
/// The area stays painted.
#[test]
fn a_panel_value_that_can_t_be_delivered_stops_the_send() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.43);
    let e = &mut f.editor;
    e.view.set_tool(Tool::AiRetouch);
    let ask = offer(e, "ask");
    e.choose_ai_generator(&ask);
    e.extensions.open_panel(ID, "ask").unwrap();
    pump(e, "the panel", |e| {
        e.extensions
            .panel(ID, "ask")
            .is_some_and(|v| !v.controls.is_empty())
    });
    e.extensions
        .panel_mut(ID, "ask")
        .unwrap()
        .drafts
        .insert("text".into(), serde_json::json!("x".repeat(2 << 20)));
    e.add_ai_stroke(vec![[0.4, 0.5]]);
    e.send_ai_draft();
    assert!(e.extensions.retouches_sent().is_empty() && settled(e));
    assert_eq!(e.ai_remove.draft.len(), 1);
    assert!(
        e.messages.line().contains("not sent"),
        "{}",
        e.messages.line()
    );
}

/// The extension stopped (disabled) while its request runs: the request is
/// void, and the area is painted again.
#[test]
fn an_extension_stopping_mid_request_paints_the_area_again() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.44);
    let e = &mut f.editor;
    e.view.set_tool(Tool::AiRetouch);
    let gate = offer(e, "gate");
    e.choose_ai_generator(&gate);
    e.add_ai_stroke(vec![[0.4, 0.5], [0.6, 0.5]]);
    e.send_ai_draft();
    assert!(e.ai_remove.draft.is_empty());
    started(e, &f.folder);
    e.set_extension_enabled(ID, false);
    pump(e, "the request to end", settled);
    assert_eq!(e.ai_remove.draft.len(), 1, "painted again");
}

/// Undo takes back the area's strokes, newest first, before it reaches
/// History; Redo paints them again; a new stroke ends what Redo can bring.
#[test]
fn undo_and_redo_take_back_and_repaint_the_area_s_strokes() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.45);
    let e = &mut f.editor;
    e.module = crate::app::Module::Develop;
    e.view.set_tool(Tool::AiRetouch);
    // A step in History, which Undo reaches once the area is empty.
    e.change_edit(Some(crate::app::history::Step::new("Exposure", "")), |r| {
        r.exposure = 0.5
    });
    let (a, b) = (vec![[0.3, 0.3]], vec![[0.6, 0.6], [0.7, 0.6]]);
    e.add_ai_stroke(a.clone());
    e.add_ai_stroke(b.clone());
    e.undo();
    assert_eq!(e.ai_remove.draft, std::slice::from_ref(&a));
    assert_eq!(e.document.edit.recipe().exposure, 0.5, "History untouched");
    e.redo();
    assert_eq!(e.ai_remove.draft, [a.clone(), b.clone()]);
    e.undo();
    e.undo();
    assert!(e.ai_remove.draft.is_empty());
    assert_eq!(e.document.edit.recipe().exposure, 0.5);
    // Redo brings strokes back only until something new is painted.
    e.redo();
    assert_eq!(e.ai_remove.draft, std::slice::from_ref(&a));
    e.add_ai_stroke(b.clone());
    e.redo();
    assert_eq!(e.ai_remove.draft.len(), 2, "nothing left to redo");
    e.undo();
    e.undo();
    // Empty: Undo is History's again, and Redo with it, as Undo went.
    e.undo();
    assert_eq!(e.document.edit.recipe().exposure, 0.);
    e.redo();
    assert_eq!(e.document.edit.recipe().exposure, 0.5);
    assert!(e.ai_remove.draft.is_empty(), "History's Redo, not a stroke");
    e.add_ai_stroke(a);
    e.add_ai_stroke(b);
    // Outside AI Retouch, Undo is History's.
    e.view.set_tool(Tool::Remove);
    e.undo();
    assert_eq!(e.ai_remove.draft.len(), 2);
}

/// Undo and Redo are offered (the toolbar's buttons, the menu's items)
/// whenever they would act on the area's strokes, History empty or not; an
/// Undo History refuses keeps the strokes Redo can bring; and a send ends
/// what Redo can bring, so the area sent comes back whole if nothing is made.
#[test]
fn stroke_undo_is_offered_and_never_loses_an_area() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.46);
    let e = &mut f.editor;
    e.module = crate::app::Module::Develop;
    e.view.set_tool(Tool::AiRetouch);
    assert!(!e.can_undo() && !e.can_redo());
    let (a, b) = (vec![[0.3, 0.3]], vec![[0.6, 0.6]]);
    e.add_ai_stroke(a.clone());
    assert!(e.can_undo() && !e.can_redo());
    e.undo();
    assert!(!e.can_undo() && e.can_redo());
    // History has nothing: this Undo changes nothing, and A can come back.
    e.undo();
    e.redo();
    assert_eq!(e.ai_remove.draft, std::slice::from_ref(&a));
    // A and B painted, B taken back, A sent, then the upload question
    // cancelled: A comes back, and Redo has nothing of B left to repaint
    // over it.
    e.add_ai_stroke(b);
    e.undo();
    let fill = offer(e, "fill");
    e.choose_ai_generator(&fill);
    e.send_ai_draft();
    assert!(e.ai_remove.consent.is_some());
    assert!(!e.can_redo());
    e.redo();
    e.answer_ai_remove_consent(false);
    assert_eq!(e.ai_remove.draft, [a]);
}

/// A stroke painted and taken back while a send is asked about belongs to
/// no area once the area sent comes back: Redo can't add it to that one.
#[test]
fn an_area_coming_back_takes_no_stroke_from_another() {
    use crate::app::develop::state::Tool;
    let mut f = fixture(0.47);
    let e = &mut f.editor;
    e.module = crate::app::Module::Develop;
    e.view.set_tool(Tool::AiRetouch);
    let fill = offer(e, "fill");
    e.choose_ai_generator(&fill);
    let a = vec![[0.3, 0.3]];
    e.add_ai_stroke(a.clone());
    e.send_ai_draft();
    assert!(e.ai_remove.consent.is_some());
    e.add_ai_stroke(vec![[0.7, 0.7]]);
    e.undo();
    e.answer_ai_remove_consent(false);
    assert_eq!(e.ai_remove.draft, std::slice::from_ref(&a));
    assert!(!e.can_redo());
    e.redo();
    assert_eq!(e.ai_remove.draft, [a]);
}
