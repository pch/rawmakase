//! Photo sources in the editor, end to end: the test-only
//! `dev.rawmakase.sources` extension's program (this test binary started
//! again with [`PROGRAM`] set) reports the files a test names, in the scopes
//! the host starts and stops.
use super::{Host, Outcome};
use crate::app::{Editor, Module};
use crate::catalog::PhotoId;
use eframe::egui;
use rawmakase_extensions::session::Launch;
use rawmakase_extensions::state::State;
use rawmakase_extensions::store::Store;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

const PROGRAM: &str = "RAWMAKASE_EXAMPLE_SOURCES";
const ID: &str = "dev.rawmakase.sources";

/// The sources extension's program, when this binary is started as it; a
/// test that does nothing otherwise.
#[test]
fn sources_program() {
    if std::env::var_os(PROGRAM).is_none() {
        return;
    }
    let code = i32::from(rawmakase_example::sources::extension().run().is_err());
    std::process::exit(code);
}

fn program() -> Launch {
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::extensions::sources_tests::sources_program",
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

fn picture(path: &Path, level: u8) {
    image::RgbImage::from_fn(32, 24, |x, _| image::Rgb([level, level / 2, (x * 4) as u8]))
        .save(path)
        .unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    editor: Editor,
    /// The catalog's one photo.
    photo: (PhotoId, PathBuf),
    /// The source's folder, not in the catalog.
    captures: PathBuf,
    /// The extension's developer folder: its program's working folder.
    folder: PathBuf,
    data: PathBuf,
    catalog: PathBuf,
}

/// An editor in the Library of a catalog of one PNG, with the sources
/// extension loaded from a developer folder, and an empty folder of
/// captures beside it.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let photos = dir.path().join("photos");
    std::fs::create_dir(&photos).unwrap();
    picture(&photos.join("a.png"), 60);
    let captures = dir.path().join("captures");
    std::fs::create_dir_all(captures.join("day")).unwrap();
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)
        .unwrap()
        .add_folder(&photos)
        .unwrap();
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.raw_defaults = Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
        Default::default(),
        |_| None,
    ));
    let library = crate::app::library::Library::load(&catalog, ctx).unwrap();
    let a = &library.session.photos()[0];
    let photo = (a.id, a.path.clone());
    editor.library = Some(Box::new(library));
    editor.module = Module::Library;
    editor.onboarding.visible = false;
    let folder = dir.path().join("sources");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join(rawmakase_extensions::protocol::manifest::FILE),
        include_str!("../../../extensions/example/sources/rawmakase-extension.toml"),
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
        photo,
        captures,
        folder,
        data,
        catalog,
    }
}

/// Polls the host and the editor's events until `done` holds, or fails
/// after 20 s; every outcome.
fn pump(e: &mut Editor, what: &str, done: impl Fn(&Editor, &[Outcome]) -> bool) -> Vec<Outcome> {
    let until = Instant::now() + Duration::from_secs(20);
    let mut all = Vec::new();
    let ctx = e.context.clone();
    loop {
        e.events(&ctx);
        let (outcomes, _) = e.extensions_poll(Instant::now());
        all.extend(outcomes);
        if done(e, &all) {
            return all;
        }
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}: {all:?}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Whether the program said `text` among `outcomes`.
fn said(outcomes: &[Outcome], text: &str) -> bool {
    let full = format!("Example Sources: {text}");
    outcomes
        .iter()
        .any(|o| matches!(o, Outcome::Message { text, .. } if *text == full))
}

/// Waits for the program to say `text`.
fn hear(e: &mut Editor, text: &str) {
    pump(e, text, |_, o| said(o, text));
}

/// Turns the source on for the open catalog once the program runs.
fn turn_on(e: &mut Editor) {
    pump(e, "the program", |e, _| {
        e.extensions.get(ID).unwrap().running()
    });
    e.extensions.set_source_on(ID, "drop", true).unwrap();
}

/// Asks the program to report `files` from the captures folder (in `scope`
/// when given), without waiting.
fn ask(f: &Fixture, files: &[PathBuf], show: bool, scope: Option<u64>) {
    let _ = std::fs::remove_file(f.folder.join("reported.json"));
    let mut asked = json!({"folder": f.captures, "files": files, "show": show});
    if let Some(scope) = scope {
        asked["scope"] = json!(scope);
    }
    let part = f.folder.join("report.part");
    std::fs::write(&part, asked.to_string()).unwrap();
    std::fs::rename(&part, f.folder.join("report.json")).unwrap();
}

/// What the app answered the program's report, once it has.
fn answered(f: &mut Fixture) -> Value {
    let reported = f.folder.join("reported.json");
    pump(&mut f.editor, "the report's answer", |_, _| {
        reported.exists()
    });
    serde_json::from_str(&std::fs::read_to_string(&reported).unwrap()).unwrap()
}

/// Asks for a report and waits for its answer.
fn report(f: &mut Fixture, files: &[PathBuf], show: bool, scope: Option<u64>) -> Value {
    ask(f, files, show, scope);
    answered(f)
}

/// The photo the catalog has at `path`.
fn photo_at(e: &Editor, path: &Path) -> Option<PhotoId> {
    e.catalog_photo_at(&path.canonicalize().unwrap())
}

#[test]
fn a_source_turned_on_for_the_catalog_adds_what_it_reports_in_its_scope() {
    let mut f = fixture();
    // Off until turned on for this catalog: the program runs, and no scope
    // starts.
    let outcomes = pump(&mut f.editor, "the program", |e, _| {
        e.extensions.get(ID).unwrap().running()
    });
    assert!(
        !outcomes
            .iter()
            .any(|o| matches!(o, Outcome::Message { .. }))
    );
    assert_eq!(f.editor.extensions.source_on(ID, "drop"), Some(false));
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    let state = State::read(&f.data).unwrap();
    assert_eq!(
        state.extensions[ID].sources["drop"],
        [f.catalog.to_string_lossy()]
    );
    let c = f.captures.join("c.png");
    let d = f.captures.join("day/d.png");
    picture(&c, 120);
    picture(&d, 180);
    let missing = f.captures.join("missing.png");
    let outside = f.photo.1.clone();
    let reply = report(
        &mut f,
        &[c.clone(), d.clone(), missing.clone(), outside.clone()],
        false,
        None,
    );
    assert_eq!(reply["added"], json!(2), "{reply}");
    let (c_id, d_id) = (
        photo_at(&f.editor, &c).expect("c is in the catalog"),
        photo_at(&f.editor, &d).expect("d is in the catalog"),
    );
    assert_eq!(
        reply["photos"],
        json!([{"path": c, "photo": c_id.0}, {"path": d, "photo": d_id.0}])
    );
    assert_eq!(
        reply["refused"],
        json!([{"path": missing, "reason": "not_found"},
               {"path": outside, "reason": "outside_folder"}])
    );
    let library = f.editor.library.as_ref().unwrap();
    assert!(library.session.previous_import().contains(&c_id));
    assert!(library.session.previous_import().contains(&d_id));
    assert!(!f.editor.activity.is_busy(), "the catalog is free again");
    // Reported again, as after a reconnection: nothing is added twice.
    let again = report(&mut f, std::slice::from_ref(&c), false, None);
    assert_eq!(again["added"], json!(0));
    assert_eq!(again["photos"], json!([{"path": c, "photo": c_id.0}]));
    assert_eq!(f.editor.library.as_ref().unwrap().session.photos().len(), 3);
}

#[test]
fn a_report_leaves_the_photo_open_in_develop_as_it_was_until_one_asks_to_be_shown() {
    let mut f = fixture();
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    let e = &mut f.editor;
    let ctx = e.context.clone();
    e.module = Module::Develop;
    e.open_raw(f.photo.1.clone(), Some(f.photo.0));
    let until = Instant::now() + Duration::from_secs(30);
    while e.document.loading() || e.document.full().is_none() {
        assert!(Instant::now() < until, "the photo never opened");
        e.events(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
    let old = e.document.edit.replace(crate::model::recipe::Recipe {
        exposure: 0.4,
        ..e.document.edit.recipe().clone()
    });
    e.commit_edit(old, None);
    let c = f.captures.join("c.png");
    picture(&c, 120);
    let reply = report(&mut f, std::slice::from_ref(&c), false, None);
    assert_eq!(reply["added"], json!(1), "{reply}");
    let e = &f.editor;
    assert_eq!(e.document.catalog_photo, Some(f.photo.0), "still open");
    assert!((e.document.edit.recipe().exposure - 0.4).abs() < 1e-6);
    assert!(photo_at(e, &c).is_some());
    // Shown, as a tethered capture is: Develop saves its edit and opens it.
    let d = f.captures.join("d.png");
    picture(&d, 180);
    let reply = report(&mut f, std::slice::from_ref(&d), true, None);
    assert_eq!(reply["added"], json!(1), "{reply}");
    let d_id = photo_at(&f.editor, &d).unwrap();
    pump(&mut f.editor, "d to open", |e, _| {
        e.document.catalog_photo == Some(d_id)
    });
    let library = f.editor.library.as_ref().unwrap();
    let saved = library
        .session
        .catalog()
        .load_edit(f.photo.0, &f.photo.1)
        .unwrap()
        .map(|edit| edit.recipe.exposure);
    assert_eq!(saved, Some(0.4), "the edit left was saved");
}

#[test]
fn a_shown_report_selects_its_photo_in_the_library() {
    let mut f = fixture();
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    let c = f.captures.join("c.png");
    picture(&c, 120);
    report(&mut f, std::slice::from_ref(&c), true, None);
    let c_id = photo_at(&f.editor, &c).unwrap();
    let library = f.editor.library.as_ref().unwrap();
    assert_eq!(library.selected_photos(), [c_id]);
    assert_eq!(f.editor.module, Module::Library);
    // A file the catalog had is listed, but only the one added is shown,
    // wherever it is in the report.
    let d = f.captures.join("d.png");
    picture(&d, 180);
    let reply = report(&mut f, &[d.clone(), c.clone()], true, None);
    assert_eq!(reply["added"], json!(1), "{reply}");
    assert_eq!(reply["photos"].as_array().unwrap().len(), 2, "{reply}");
    let d_id = photo_at(&f.editor, &d).unwrap();
    assert_eq!(f.editor.library.as_ref().unwrap().selected_photos(), [d_id]);
    // A report adding nothing, a source reporting again after it
    // reconnected, leaves the selection alone.
    let reply = report(&mut f, std::slice::from_ref(&c), true, None);
    assert_eq!(reply["added"], json!(0), "{reply}");
    assert_eq!(f.editor.library.as_ref().unwrap().selected_photos(), [d_id]);
}

#[test]
fn a_copy_name_being_typed_is_saved_before_a_report_reads_the_catalog_again() {
    let mut f = fixture();
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    let library = f.editor.library.as_mut().unwrap();
    let copy = library.create_virtual_copy(f.photo.0).unwrap().value;
    library.set_copy_name_draft(copy, "Typed name");
    let c = f.captures.join("c.png");
    picture(&c, 120);
    let reply = report(&mut f, std::slice::from_ref(&c), false, None);
    assert_eq!(reply["added"], json!(1), "{reply}");
    let library = f.editor.library.as_ref().unwrap();
    assert!(photo_at(&f.editor, &c).is_some(), "the new photo is shown");
    assert_eq!(library.photo(copy).unwrap().copy_name, "Typed name");
    assert!(!library.has_drafts());
}

#[test]
fn a_scope_stops_when_its_source_is_turned_off_or_its_catalog_closes() {
    let mut f = fixture();
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    f.editor
        .extensions
        .set_source_on(ID, "drop", false)
        .unwrap();
    hear(&mut f.editor, "stopped 1");
    let c = f.captures.join("c.png");
    picture(&c, 120);
    let reply = report(&mut f, std::slice::from_ref(&c), false, Some(1));
    assert_eq!(reply, json!({"error": "not_admitted"}));
    assert!(photo_at(&f.editor, &c).is_none());
    // On again: a scope of its own, never one used before.
    f.editor.extensions.set_source_on(ID, "drop", true).unwrap();
    hear(&mut f.editor, "started 2 test");
    // The catalog closes: its scope stops, and its reports are refused.
    f.editor.library = None;
    hear(&mut f.editor, "stopped 2");
    let reply = report(&mut f, std::slice::from_ref(&c), false, Some(2));
    assert_eq!(reply, json!({"error": "not_admitted"}));
}

#[test]
fn a_report_while_other_work_holds_the_catalog_is_added_once_it_is_free() {
    let mut f = fixture();
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    assert!(f.editor.activity.begin_dialog());
    let c = f.captures.join("c.png");
    picture(&c, 120);
    ask(&f, std::slice::from_ref(&c), false, None);
    let until = Instant::now() + Duration::from_millis(600);
    pump(&mut f.editor, "a while", |_, _| Instant::now() >= until);
    assert!(!f.folder.join("reported.json").exists(), "it waits");
    assert!(photo_at(&f.editor, &c).is_none());
    f.editor.activity.finish_dialog();
    let reply = answered(&mut f);
    assert_eq!(reply["added"], json!(1), "{reply}");
}

#[test]
fn a_report_withdrawn_before_its_write_adds_nothing() {
    let mut f = fixture();
    turn_on(&mut f.editor);
    hear(&mut f.editor, "started 1 test");
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let held = barrier.clone();
    f.editor.report_hook = Some(Arc::new(move || {
        held.wait();
        held.wait();
    }));
    let c = f.captures.join("c.png");
    picture(&c, 120);
    ask(&f, std::slice::from_ref(&c), false, None);
    pump(&mut f.editor, "the report's worker", |e, _| {
        e.extensions.proposing()
    });
    barrier.wait();
    // Its scope stops while the worker is held, its files read and nothing
    // written yet: the program is answered at once, and the worker, let go,
    // writes nothing.
    f.editor
        .extensions
        .set_source_on(ID, "drop", false)
        .unwrap();
    let reply = answered(&mut f);
    assert_eq!(reply, json!({"error": "not_admitted"}));
    barrier.wait();
    pump(&mut f.editor, "the worker", |e, _| !e.activity.is_busy());
    assert!(photo_at(&f.editor, &c).is_none());
    let catalog =
        crate::catalog::Catalog::open(crate::catalog::CatalogLocation::File(f.catalog.clone()))
            .unwrap();
    assert_eq!(catalog.photos().unwrap().len(), 1, "nothing was written");
}
