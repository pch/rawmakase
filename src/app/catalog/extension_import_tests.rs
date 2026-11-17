//! Imports from an extension's importer in the editor, end to end: the
//! test-only `dev.rawmakase.importer` extension (`rawmakase_example::importer`)
//! runs as this test binary started again with [`PROGRAM`] set, driven by
//! the JSON source each test writes.
use crate::app::Editor;
use crate::app::extensions::Host;
use crate::app::extensions::importer::Offer;
use crate::catalog::plan_import::PlanImport;
use eframe::egui;
use rawmakase_extensions::session::Launch;
use rawmakase_extensions::store::Store;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

const PROGRAM: &str = "RAWMAKASE_EXAMPLE_IMPORTER";
const ID: &str = "dev.rawmakase.importer";

/// The importer extension's program, when this binary is started as it; a
/// test that does nothing otherwise.
#[test]
fn importer_program() {
    if std::env::var_os(PROGRAM).is_none() {
        return;
    }
    let code = i32::from(rawmakase_example::importer::extension().run().is_err());
    std::process::exit(code);
}

fn program() -> Launch {
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::catalog::extension_import_tests::importer_program",
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
    dir: tempfile::TempDir,
    editor: Editor,
    source: PathBuf,
    destination: PathBuf,
}

/// An editor with a catalog of one photo open, and the importer extension
/// loaded from a developer folder; its imports go to `out/Photos.rawmakase`.
fn fixture() -> Fixture {
    let (dir, mut editor, _) = crate::app::testing::editor_with_catalog(&["a.RAF"]).unwrap();
    let folder = dir.path().join("importer");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join(rawmakase_extensions::protocol::manifest::FILE),
        include_str!("../../../extensions/example/importer/rawmakase-extension.toml"),
    )
    .unwrap();
    let data = dir.path().join("data");
    Store::new(&data).add_developer(&folder).unwrap();
    editor.extensions = Host::start_for_tests(&data, program(), Duration::from_secs(30));
    std::fs::create_dir(dir.path().join("out")).unwrap();
    Fixture {
        source: dir.path().join("source.json"),
        destination: dir.path().join("out").join("Photos.rawmakase"),
        dir,
        editor,
    }
}

impl Fixture {
    fn offer(&self) -> Offer {
        self.editor
            .importer_items()
            .into_iter()
            .find(|o| o.extension == ID)
            .expect("the importer is listed")
    }

    /// Starts an import of `script` as Catalog › Import does once its
    /// dialogs are answered.
    fn start(&mut self, script: &str) {
        std::fs::write(&self.source, script).unwrap();
        let offer = self.offer();
        assert!(self.editor.ready_for_catalog() && self.editor.activity.begin_dialog());
        let job = PlanImport::prepare(&self.source, &self.destination).unwrap();
        self.editor.start_catalog_import(offer, job);
        assert!(self.editor.catalog_importing().is_some());
    }

    /// Starts an import of `script` whose worker, once the program has
    /// answered, waits as it begins to check the plan until the returned
    /// [`Paused`] lets it go.
    fn start_paused(&mut self, script: &str) -> Paused {
        self.start(script);
        let (reached, has_reached) = mpsc::channel();
        let (go, wait) = mpsc::channel::<()>();
        self.editor.catalog_import.as_mut().unwrap().on_progress = Some(Box::new(move |text| {
            if text == "Checking the import plan…" {
                let _ = reached.send(());
                let _ = wait.recv();
            }
        }));
        Paused { has_reached, go }
    }

    /// What is beside the destination.
    fn beside(&self) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(self.dir.path().join("out"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    fn nothing_left(&self) {
        assert_eq!(self.beside(), Vec::<String>::new());
    }

    fn ended(&self) -> bool {
        self.editor.catalog_importing().is_none()
    }

    fn opened(&self) -> bool {
        self.editor.library.as_ref().is_some_and(|l| {
            let crate::catalog::CatalogLocation::File(path) = l.session.catalog().location();
            path == &self.destination
        })
    }

    fn said(&self, text: &str) -> bool {
        self.editor.messages.line().contains(text) || self.editor.messages.has_entry_with(text)
    }
}

/// Runs frames, as the app does, until `done` holds, or fails after 20 s.
fn pump(f: &mut Fixture, what: &str, done: impl Fn(&Fixture) -> bool) {
    let ctx = f.editor.context.clone();
    let until = Instant::now() + Duration::from_secs(20);
    loop {
        frame(&mut f.editor, &ctx);
        if done(f) {
            return;
        }
        assert!(
            Instant::now() < until,
            "timed out waiting for {what}: {}",
            f.editor.messages.line()
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn frame(e: &mut Editor, ctx: &egui::Context) {
    e.events(ctx);
    e.extensions_frame(ctx);
}

/// An import's worker paused (see [`Fixture::start_paused`]).
struct Paused {
    has_reached: mpsc::Receiver<()>,
    go: mpsc::Sender<()>,
}

impl Paused {
    /// Runs frames until the worker is at the pause.
    fn reach(&self, f: &mut Fixture) {
        let ctx = f.editor.context.clone();
        let until = Instant::now() + Duration::from_secs(20);
        while self.has_reached.try_recv().is_err() {
            frame(&mut f.editor, &ctx);
            assert!(Instant::now() < until, "the import never paused");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn go(&self) {
        self.go.send(()).unwrap();
    }
}

fn not_busy(f: &Fixture) {
    assert!(f.ended());
    assert!(!f.editor.activity.is_busy());
    assert!(
        !f.editor
            .running_work()
            .iter()
            .any(|w| w.key == "catalog import")
    );
}

#[test]
fn the_import_menu_lists_enabled_importers_only() {
    let mut f = fixture();
    let items = f.editor.importer_items();
    // The bundled Lightroom extension's program isn't beside the tests: it
    // offers nothing.
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].name, "Example script");
    assert_eq!(items[0].extensions, ["json"]);
    f.editor.set_extension_enabled(ID, false);
    assert_eq!(f.editor.importer_items(), Vec::new());
    f.editor.set_extension_enabled(ID, true);
    assert_eq!(f.editor.importer_items().len(), 1);
}

#[test]
fn an_import_publishes_and_opens_its_catalog() {
    let mut f = fixture();
    f.start(r#"{"photos": 2}"#);
    assert!(
        f.editor
            .running_work()
            .iter()
            .any(|w| w.key == "catalog import")
    );
    pump(&mut f, "the new catalog to open", |f| {
        f.opened() && !f.editor.activity.is_busy()
    });
    not_busy(&f);
    assert_eq!(f.editor.library.as_ref().unwrap().session.photos().len(), 2);
    assert_eq!(f.beside(), ["Photos.rawmakase"]);
}

#[test]
fn a_failed_import_ends_its_busy_state() {
    let mut f = fixture();
    f.start(r#"{"then": "fail"}"#);
    pump(&mut f, "the import to fail", Fixture::ended);
    not_busy(&f);
    assert!(f.said("couldn't be read"), "{}", f.editor.messages.line());
    f.nothing_left();
}

#[test]
fn stopping_an_import_ends_it_and_a_late_answer_is_dropped() {
    let mut f = fixture();
    f.start(r#"{"then": "progress"}"#);
    pump(&mut f, "the program's progress", |f| {
        f.editor.catalog_importing() == Some("Still working")
    });
    let ticket = f.editor.catalog_import_ticket().unwrap();
    f.editor.cancel_catalog_import();
    pump(&mut f, "the import to stop", Fixture::ended);
    not_busy(&f);
    assert!(f.said("import stopped"), "{}", f.editor.messages.line());
    f.nothing_left();
    // An answer for that import, from a session that ended, changes
    // nothing.
    f.editor
        .import_answered(ID, ticket, Ok(serde_json::json!({"photos": 5})));
    assert!(f.ended() && !f.editor.activity.is_busy());
    f.nothing_left();
}

#[test]
fn an_answer_to_another_request_is_dropped() {
    let mut f = fixture();
    f.start(r#"{"then": "progress"}"#);
    let ticket = f.editor.catalog_import_ticket().unwrap();
    f.editor
        .import_answered(ID, ticket + 100, Ok(serde_json::json!({"photos": 5})));
    f.editor
        .import_answered("dev.other", ticket, Ok(serde_json::json!({"photos": 5})));
    assert_eq!(
        f.editor.catalog_importing().map(str::to_string),
        Some("Importing source.json…".to_string())
    );
    f.editor.cancel_catalog_import();
    pump(&mut f, "the import to stop", Fixture::ended);
    f.nothing_left();
}

#[test]
fn a_panic_while_importing_ends_its_busy_state() {
    let mut f = fixture();
    f.start(r#"{"photos": 3}"#);
    f.editor.catalog_import.as_mut().unwrap().on_progress = Some(Box::new(|text| {
        assert_ne!(text, "Importing photos…", "a panic on the worker");
    }));
    pump(&mut f, "the import to fail", Fixture::ended);
    not_busy(&f);
    assert!(
        f.said("Stopped unexpectedly"),
        "{}",
        f.editor.messages.line()
    );
    f.nothing_left();
}

#[test]
fn a_program_crash_before_it_answers_fails_the_import() {
    let mut f = fixture();
    f.start(r#"{"then": "crash"}"#);
    pump(&mut f, "the import to fail", Fixture::ended);
    not_busy(&f);
    assert!(
        f.said("ended before it answered"),
        "{}",
        f.editor.messages.line()
    );
    f.nothing_left();
}

#[test]
fn disabling_then_enabling_the_extension_cancels_the_import() {
    let mut f = fixture();
    f.start(r#"{"then": "progress"}"#);
    pump(&mut f, "the program's progress", |f| {
        f.editor.catalog_importing() == Some("Still working")
    });
    f.editor.set_extension_enabled(ID, false);
    f.editor.set_extension_enabled(ID, true);
    pump(&mut f, "the import to end", Fixture::ended);
    not_busy(&f);
    f.nothing_left();
}

#[test]
fn disabling_the_extension_while_the_plan_is_imported_cancels_the_import() {
    let mut f = fixture();
    let paused = f.start_paused(r#"{"photos": 3}"#);
    paused.reach(&mut f);
    f.editor.set_extension_enabled(ID, false);
    // The frame's extension outcomes, after which the import is checked.
    let ctx = f.editor.context.clone();
    frame(&mut f.editor, &ctx);
    paused.go();
    pump(&mut f, "the import to end", Fixture::ended);
    not_busy(&f);
    assert!(f.said("import stopped"), "{}", f.editor.messages.line());
    f.nothing_left();
}

#[test]
fn the_program_crashing_after_it_answered_changes_nothing() {
    let mut f = fixture();
    let paused = f.start_paused(r#"{"photos": 3}"#);
    paused.reach(&mut f);
    f.editor.extensions.kill_program_for_tests(ID);
    pump(&mut f, "the program to end", |f| {
        !f.editor.extensions.program_running(ID)
    });
    paused.go();
    pump(&mut f, "the new catalog to open", |f| {
        f.opened() && !f.editor.activity.is_busy()
    });
    assert_eq!(f.beside(), ["Photos.rawmakase"]);
}

#[test]
fn the_program_idle_stopped_after_it_answered_changes_nothing() {
    let mut f = fixture();
    let paused = f.start_paused(r#"{"photos": 3}"#);
    paused.reach(&mut f);
    // Past the idle stop: the program, with nothing in flight, is stopped.
    let later = Instant::now() + crate::app::extensions::IDLE_STOP + Duration::from_secs(1);
    let (outcomes, _) = f.editor.extensions_poll(later);
    f.editor.extension_outcomes(outcomes);
    assert!(!f.editor.extensions.program_running(ID));
    paused.go();
    pump(&mut f, "the new catalog to open", |f| {
        f.opened() && !f.editor.activity.is_busy()
    });
    assert_eq!(f.beside(), ["Photos.rawmakase"]);
}

#[test]
fn a_program_that_stops_reporting_progress_fails_the_import() {
    let mut f = fixture();
    f.editor
        .extensions
        .set_import_inactivity(Duration::from_millis(300));
    f.start(r#"{"photos": 1, "then": "silent"}"#);
    pump(&mut f, "the import to fail", Fixture::ended);
    not_busy(&f);
    assert!(
        f.said("stopped reporting progress"),
        "{}",
        f.editor.messages.line()
    );
    f.nothing_left();
}

#[test]
fn steady_progress_keeps_an_import_going_past_the_idle_stop() {
    let mut f = fixture();
    // A program that reports progress every 50 ms until it is cancelled.
    f.start(r#"{"then": "progress"}"#);
    pump(&mut f, "the program's progress", |f| {
        f.editor.catalog_importing() == Some("Still working")
    });
    // The simulated clock moves 40 s on each poll that took progress, and
    // not otherwise: the deadline that progress set is a minute after the
    // poll, so it never passes however the threads are scheduled, and the
    // clock passes the idle stop only through progress.
    let ctx = f.editor.context.clone();
    let start = Instant::now();
    let mut clock = start;
    let until = start + Duration::from_secs(30);
    while clock - start <= crate::app::extensions::IDLE_STOP + Duration::from_secs(60) {
        f.editor.events(&ctx);
        let (outcomes, _) = f.editor.extensions_poll(clock);
        let progressed = outcomes
            .iter()
            .any(|o| matches!(o, crate::app::extensions::Outcome::ImportProgress { .. }));
        f.editor.extension_outcomes(outcomes);
        assert!(!f.ended(), "{}", f.editor.messages.line());
        if progressed {
            clock += Duration::from_secs(40);
        }
        assert!(Instant::now() < until, "{}", f.editor.messages.line());
        std::thread::sleep(Duration::from_millis(10));
    }
    // Still running, its program too, past the idle stop.
    assert!(f.editor.extensions.program_running(ID));
    assert_eq!(f.editor.catalog_importing(), Some("Still working"));
    f.editor.cancel_catalog_import();
    pump(&mut f, "the import to stop", Fixture::ended);
    not_busy(&f);
    f.nothing_left();
}

#[test]
fn quitting_while_the_program_works_cancels_the_import_within_the_grace() {
    let mut f = fixture();
    f.start(r#"{"then": "progress"}"#);
    pump(&mut f, "the program's progress", |f| {
        f.editor.catalog_importing() == Some("Still working")
    });
    let started = Instant::now();
    f.editor.exit_within(Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_secs(4));
    f.nothing_left();
}

#[test]
fn quitting_while_the_plan_is_imported_cancels_the_import_within_the_grace() {
    let mut f = fixture();
    let paused = f.start_paused(r#"{"photos": 3}"#);
    paused.reach(&mut f);
    let go = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        paused.go();
    });
    let started = Instant::now();
    f.editor.exit_within(Duration::from_secs(3));
    assert!(started.elapsed() < Duration::from_secs(4));
    // Waited for: the worker saw the cancel and deleted what it made.
    f.nothing_left();
    go.join().unwrap();
}

#[test]
fn a_failed_save_before_opening_leaves_the_catalog_published_and_says_where() {
    let mut f = fixture();
    let paused = f.start_paused(r#"{"photos": 1}"#);
    paused.reach(&mut f);
    // Meanwhile a photo is edited, and the catalog stops taking edits.
    let (id, path) = {
        let library = f.editor.library.as_ref().unwrap();
        let photo = &library.session.photos()[0];
        (photo.id, photo.path.clone())
    };
    let old = f
        .editor
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .location()
        .clone();
    f.editor.document.catalog_photo = Some(id);
    f.editor.document.opened_at(path);
    f.editor
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch(
            "CREATE TRIGGER no_edits BEFORE UPDATE OF recipe ON photos
             BEGIN SELECT RAISE(ABORT, 'read-only'); END;",
        )
        .unwrap();
    f.editor.document.edit.change(None, |r| r.exposure = 1.);
    paused.go();
    pump(&mut f, "the import to end", Fixture::ended);
    assert_eq!(f.beside(), ["Photos.rawmakase"]);
    assert!(!f.opened());
    assert_eq!(
        f.editor
            .library
            .as_ref()
            .unwrap()
            .session
            .catalog()
            .location(),
        &old
    );
    assert!(f.said("wasn't opened"), "{}", f.editor.messages.line());
    let shown = crate::app::ui::widgets::pretty_path(&f.destination);
    assert!(f.said(&shown), "{}", f.editor.messages.line());
    assert!(!f.editor.activity.is_busy());
}

/// Runs frames until `done` holds, or fails after 60 s.
fn until(editor: &mut Editor, what: &str, done: impl Fn(&Editor) -> bool) {
    let ctx = editor.context.clone();
    let until = Instant::now() + Duration::from_secs(60);
    while !done(editor) {
        frame(editor, &ctx);
        assert!(Instant::now() < until, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// `text` as Lightroom compresses History steps and snapshots.
fn compressed(text: &str) -> Vec<u8> {
    use std::io::Write;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(text.as_bytes()).unwrap();
    let mut blob = (text.len() as u32).to_be_bytes().to_vec();
    blob.extend(z.finish().unwrap());
    blob
}

/// A Lightroom Classic catalog imports with the bundled Lightroom
/// extension's importer, and with the extension off its photo opens with
/// its Lightroom edit as the catalog resolves it (what the renderer gets),
/// its History step applies and copies to Before, and its snapshot applies.
/// The same holds for the catalog as an earlier release imported it, with
/// its History and snapshot text compressed.
#[test]
fn a_lightroom_catalog_imports_and_its_edits_work_with_the_extension_off() -> anyhow::Result<()> {
    use crate::app::develop::panels::history_panel::LightroomStep;
    use crate::catalog::PhotoId;
    use rawmakase_extensions::protocol::manifest::LIGHTROOM;
    let dir = tempfile::tempdir()?;
    let trip = dir.path().join("photos").join("Trip");
    std::fs::create_dir_all(&trip)?;
    let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/corpus/charts/synthetic-d65.dng");
    std::fs::copy(chart, trip.join("image.dng"))?;
    let source = dir.path().join("Lightroom.lrcat");
    rawmakase_lightroom::fake::catalog(&source)?;
    let lr = rusqlite::Connection::open(&source)?;
    let root = format!("{}/", dir.path().join("photos").display());
    lr.execute("UPDATE AgLibraryRootFolder SET absolutePath = ?", [root])?;
    lr.execute_batch(
        "UPDATE AgLibraryFile SET idx_filename = 'image.dng', extension = 'dng';
         CREATE TABLE Adobe_libraryImageDevelopSnapshot(image INTEGER, name TEXT, text BLOB);",
    )?;
    let (step, snapshot) = ("s = { Exposure2012 = 0.75 }", "s = { Exposure2012 = -0.5 }");
    lr.execute(
        "INSERT INTO Adobe_libraryImageDevelopHistoryStep VALUES (40, 1, 100., 'Brighter', ?)",
        [compressed(step)],
    )?;
    lr.execute(
        "INSERT INTO Adobe_libraryImageDevelopSnapshot VALUES (40, 'Darker', ?)",
        [compressed(snapshot)],
    )?;
    drop(lr);

    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.extensions = crate::app::extensions::adobe_files::tests::host(dir.path());
    let offer = editor
        .importer_items()
        .into_iter()
        .find(|o| o.extension == LIGHTROOM)
        .expect("the Lightroom extension's importer");
    assert_eq!(offer.name, "Lightroom Classic catalog");
    let destination = dir.path().join("Photos.rawmakase");
    assert!(editor.ready_for_catalog() && editor.activity.begin_dialog());
    editor.start_catalog_import(offer, PlanImport::prepare(&source, &destination)?);
    until(&mut editor, "the import to open", |e| {
        e.library.is_some() && !e.activity.is_busy()
    });
    assert_eq!(editor.library.as_ref().unwrap().session.photos().len(), 2);
    editor.set_extension_enabled(LIGHTROOM, false);
    assert!(editor.importer_items().is_empty());

    // As an earlier release imported it: History and snapshots compressed.
    let earlier = dir.path().join("Earlier.rawmakase");
    std::fs::copy(&destination, &earlier)?;
    let db = rusqlite::Connection::open(&earlier)?;
    db.execute("UPDATE lightroom_history SET text = ?", [compressed(step)])?;
    db.execute(
        "UPDATE develop_snapshots SET lightroom = ?",
        [compressed(snapshot)],
    )?;
    drop(db);

    for catalog in [destination, earlier] {
        editor.load_catalog(catalog.clone(), &ctx);
        until(&mut editor, "the catalog to open", |e| {
            e.library.as_ref().is_some_and(|l| {
                l.session.catalog().location()
                    == &crate::catalog::CatalogLocation::File(catalog.clone())
            }) && !e.activity.is_busy()
        });
        let id = PhotoId(40);
        let library = editor.library.as_ref().unwrap();
        let path = library
            .session
            .photos()
            .iter()
            .find(|p| p.id == id)
            .unwrap()
            .path
            .clone();
        editor.open_raw(path.clone(), Some(id));
        until(&mut editor, "the photo to open", |e| {
            e.document.full().is_some()
        });
        let record = editor
            .library
            .as_ref()
            .unwrap()
            .session
            .catalog()
            .edit_record(id)?;
        let resolved = crate::edits::resolve(
            &record,
            &path,
            editor.document.metadata.as_ref().unwrap(),
            &editor.document.profiles,
            &editor.raw_defaults,
        )?;
        assert_eq!(resolved.origin, crate::edits::Origin::Lightroom);
        assert_eq!(*editor.document.edit.recipe(), resolved.recipe);
        assert_eq!(editor.document.edit.recipe().exposure, 1.5);

        assert_eq!(editor.document.lightroom_history.len(), 1, "{catalog:?}");
        assert_eq!(editor.document.lightroom_history[0].text, step);
        editor.use_lightroom_step(0, LightroomStep::ToBefore);
        assert_eq!(
            editor.document.before.as_ref().map(|r| r.exposure),
            Some(0.75)
        );
        editor.use_lightroom_step(0, LightroomStep::Apply);
        assert_eq!(editor.document.edit.recipe().exposure, 0.75);
        let snapshots = editor.document.snapshots.list.clone();
        assert_eq!(snapshots.len(), 1, "{catalog:?}");
        editor.apply_snapshot(snapshots[0].id);
        assert_eq!(editor.document.edit.recipe().exposure, -0.5);
    }
    Ok(())
}

/// An importer whose program can't run (not installed beside the app here)
/// isn't offered, in the menu or setup.
#[test]
fn an_importer_whose_program_is_missing_is_not_offered() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = egui::Context::default();
    let mut editor = Editor::with_context(
        &ctx,
        None,
        crate::app::preferences::session::Session::default(),
        None,
    );
    editor.extensions =
        Host::with_search_dirs_for_tests(&dir.path().join("data"), vec![dir.path().join("none")]);
    assert_eq!(editor.importer_items(), Vec::new());
}
