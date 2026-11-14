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
    assert_eq!(last_step(e), "AI Remove");
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
    assert_eq!(last_step(e), "AI Remove");

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
        let message = e.ai_remove.failure.as_ref().unwrap().message();
        assert!(message.contains(failure), "{generator}: {message}");
        assert!(generated(e).is_empty());
    }
    assert!(matches!(e.ai_remove.failure, Some(Failure::NeedsSetup(_))));
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
    remove_with(e, "setup");
    pump(e, "the removal", settled);
    assert_eq!(generated(e).len(), 1, "{}", e.messages.line());
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
        Some("Add this photo to a catalog to use AI Remove")
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
    // A Heal spot selected stays Heal when AI Remove is chosen for new spots.
    e.change_edit(None, |r| {
        r.add_retouch(RetouchOp {
            mode: RetouchMode::Heal,
            offset: [0.1, 0.],
            patch: None,
            ..painted()
        })
    });
    let heal = e.document.edit.recipe().retouch.len() - 1;
    e.view.retouch.selected = Some(heal);
    in_frame(e, &|e| e.choose_retouch_mode(RetouchMode::Generate));
    assert_eq!(
        e.document.edit.recipe().retouch[heal].mode,
        RetouchMode::Heal
    );
    assert_eq!(e.view.retouch.mode, RetouchMode::Generate);
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
        e.document.lightroom_notice.contains("AI Remove"),
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
