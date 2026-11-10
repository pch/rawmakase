//! Extensions in the editor, with the example extension's program: this test
//! binary started again with [`PROGRAM`] set, running
//! `rawmakase_example::extension()` as a third party's program would. Each
//! test that needs a program starts one, or one per crash it counts; the
//! token and state tests start none.
use super::{Host, Outcome, RunBy};
use crate::app::{Editor, Module};
use crate::catalog::PhotoId;
use eframe::egui;
use rawmakase_extensions::protocol::method::{ProposeSettings, Proposed, SettingsChange};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use rawmakase_extensions::session::Launch;
use rawmakase_extensions::state::State;
use rawmakase_extensions::store::Store;
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Set in the environment of the program the tests start.
const PROGRAM: &str = "RAWMAKASE_EXAMPLE_EXTENSION";
const ID: &str = "dev.rawmakase.example";

/// The example extension's program, when this binary is started as it; a
/// test that does nothing otherwise.
#[test]
fn example_program() {
    if std::env::var_os(PROGRAM).is_none() {
        return;
    }
    let code = i32::from(rawmakase_example::extension().run().is_err());
    std::process::exit(code);
}

fn example() -> Launch {
    Launch {
        program: std::env::current_exe().expect("the test binary"),
        args: [
            "app::extensions::tests::example_program",
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

/// A small PNG of `level` brightness.
fn picture(path: &Path, level: u8) {
    image::RgbImage::from_fn(32, 24, |x, _| image::Rgb([level, level / 2, (x * 4) as u8]))
        .save(path)
        .unwrap();
}

struct Fixture {
    dir: tempfile::TempDir,
    editor: Editor,
    photos: Vec<(PhotoId, PathBuf)>,
    data: PathBuf,
    /// The example's developer folder: its working folder when it runs.
    folder: PathBuf,
}

/// An editor in the Library of a catalog of `names` (PNGs), with the
/// example extension loaded from a developer folder; requests time out after
/// `deadline`.
fn fixture_with(names: &[&str], deadline: Duration) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let photos_dir = dir.path().join("photos");
    std::fs::create_dir(&photos_dir).unwrap();
    for (i, name) in names.iter().enumerate() {
        picture(&photos_dir.join(name), 40 + 50 * i as u8);
    }
    let catalog = dir.path().join("test.rawmakase");
    crate::catalog::Catalog::create(&catalog)
        .unwrap()
        .add_folder(&photos_dir)
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
    let photos = names
        .iter()
        .map(|name| {
            let p = library
                .session
                .photos()
                .iter()
                .find(|p| p.filename == *name)
                .unwrap();
            (p.id, p.path.clone())
        })
        .collect();
    editor.library = Some(Box::new(library));
    editor.module = Module::Library;
    editor.onboarding.visible = false;
    let folder = dir.path().join("example");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(
        folder.join(rawmakase_extensions::protocol::manifest::FILE),
        include_str!("../../../extensions/example/rawmakase-extension.toml"),
    )
    .unwrap();
    let data = dir.path().join("data");
    Store::new(&data).add_developer(&folder).unwrap();
    editor.extensions = Host::start_for_tests(&data, example(), deadline);
    editor.sync_extension_commands();
    assert!(editor.extensions.get(ID).is_some(), "the example is loaded");
    Fixture {
        dir,
        editor,
        photos,
        data,
        folder,
    }
}

fn fixture(names: &[&str]) -> Fixture {
    fixture_with(names, Duration::from_secs(30))
}

fn select(e: &mut Editor, ids: &[PhotoId]) {
    let library = e.library.as_mut().unwrap();
    library.select(ids.first().copied());
    if let Some(last) = ids.last().filter(|_| ids.len() > 1) {
        library.select_range_to(*last);
    }
    assert_eq!(library.selected_photos().len(), ids.len());
}

/// Polls the host until `done` holds for what came of it, or fails after
/// 20 s; returns every outcome.
fn pump(e: &mut Editor, what: &str, done: impl Fn(&Editor, &[Outcome]) -> bool) -> Vec<Outcome> {
    let until = Instant::now() + Duration::from_secs(20);
    let mut all = Vec::new();
    loop {
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

/// The result of the first command titled `title` among `outcomes`.
fn result<'a>(outcomes: &'a [Outcome], title: &str) -> Option<&'a Result<Value, RemoteError>> {
    outcomes.iter().find_map(|o| match o {
        Outcome::Command {
            title: t, result, ..
        } if t == title => Some(result),
        _ => None,
    })
}

/// Runs command `command` and waits for its result.
fn run(e: &mut Editor, command: &str, title: &str) -> Result<Value, RemoteError> {
    e.run_extension_command(&format!("{ID}/{command}"), RunBy::Button)
        .unwrap();
    let outcomes = pump(e, title, |_, o| result(o, title).is_some());
    result(&outcomes, title).unwrap().clone()
}

/// Whether the program's command runs and says so in the status line: past
/// the point where a cancellation would stop it before it ran.
fn waiting(e: &Editor, _: &[Outcome]) -> bool {
    e.extensions.get(ID).unwrap().status.as_deref() == Some("Waiting")
}

fn exposure(e: &Editor, photo: &(PhotoId, PathBuf)) -> Option<f32> {
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .load_edit(photo.0, &photo.1)
        .unwrap()
        .map(|edit| edit.recipe.exposure)
}

fn proposal(token: &str, photo: PhotoId, exposure: f64) -> ProposeSettings {
    let mut settings = Map::new();
    settings.insert("exposure".into(), json!(exposure));
    ProposeSettings {
        op: 1,
        token: token.to_string(),
        changes: vec![SettingsChange {
            photo: photo.0,
            settings,
        }],
    }
}

fn refused(proposed: &Proposed) -> Vec<(i64, ErrorCode)> {
    proposed
        .refused
        .iter()
        .map(|r| (r.photo, r.code.clone()))
        .collect()
}

#[test]
fn a_command_runs_with_its_settings_its_panel_and_what_it_reads() {
    let mut f = fixture(&["a.png", "b.png"]);
    let e = &mut f.editor;
    assert_eq!(
        run(e, "settings", "Show Settings"),
        Ok(json!({"step": 0.5}))
    );
    // A setting is written to extensions.json and reaches the running program.
    e.extensions.set_setting(ID, "step", json!(1.0)).unwrap();
    let state = State::read(&f.data).unwrap();
    assert_eq!(state.extensions[ID].settings["step"], json!(1.0));
    assert_eq!(
        run(e, "settings", "Show Settings"),
        Ok(json!({"step": 1.0}))
    );
    // Run through the command layer, as `rawmakase control action` does.
    select(e, &[f.photos[1].0]);
    let ctx = e.context.clone();
    let action = crate::app::shell::commands::Command::new(
        crate::app::shell::commands::Operation::Extension(format!("{ID}/read")),
    );
    assert!(e.execute_command(action, &ctx).is_ok());
    let outcomes = pump(e, "the read", |_, o| result(o, "Read the Editor").is_some());
    let read = result(&outcomes, "Read the Editor")
        .unwrap()
        .clone()
        .unwrap();
    assert_eq!(read["selection"], json!([f.photos[1].0.0]));
    assert_eq!(read["open"], json!([]));
    assert!(read["parameters"].as_u64().unwrap() > 10);
    // The command layer lists it among its actions.
    let capabilities = crate::app::shell::commands::Command::new(
        crate::app::shell::commands::Operation::Capabilities,
    );
    let listed = serde_json::to_value(e.execute_command(capabilities, &ctx).unwrap()).unwrap();
    assert!(
        listed["actions"]
            .as_array()
            .unwrap()
            .contains(&json!(format!("{ID}/brighten")))
    );
    // The panel: drawn from the program's controls, a bad value answered with
    // its help.
    e.extensions.open_panel(ID, "note").unwrap();
    pump(e, "the panel", |e, _| {
        e.extensions
            .panel(ID, "note")
            .is_some_and(|p| !p.waiting && !p.controls.is_empty())
    });
    e.extensions
        .panel_changed(ID, "note", "text", json!("far too long a note"))
        .unwrap();
    pump(e, "the refusal", |e, _| {
        e.extensions
            .panel(ID, "note")
            .is_some_and(|p| !p.waiting && p.controls[0].help.is_some())
    });
    e.extensions
        .panel_changed(ID, "note", "text", json!("short"))
        .unwrap();
    pump(e, "the note", |e, _| {
        e.extensions.panel(ID, "note").is_some_and(|p| {
            !p.waiting
                && p.controls[0].help.is_none()
                && matches!(&p.controls[0].kind,
                    rawmakase_extensions::protocol::controls::ControlKind::Text { value, .. } if value == "short")
        })
    });
}

#[test]
fn a_proposal_applies_to_the_photos_still_as_read_and_refuses_the_rest() {
    let mut f = fixture(&["a.png", "b.png", "c.png"]);
    let e = &mut f.editor;
    let ids: Vec<PhotoId> = f.photos.iter().map(|p| p.0).collect();
    select(e, &ids);
    // The target is read as the command starts; then another edit lands on c
    // and b's file is replaced, before the program proposes.
    e.run_extension_command(&format!("{ID}/brighten"), RunBy::Key)
        .unwrap();
    let library = e.library.as_mut().unwrap();
    library
        .edit_metadata(
            ids[2],
            crate::app::library::photo_metadata::Edit::Rating(3),
            false,
        )
        .unwrap();
    library.take_done();
    picture(&f.photos[1].1, 250);
    let outcomes = pump(e, "Brighten", |_, o| result(o, "Brighten").is_some());
    let proposed: Proposed =
        serde_json::from_value(result(&outcomes, "Brighten").unwrap().clone().unwrap()).unwrap();
    assert_eq!(proposed.applied, vec![ids[0].0]);
    assert_eq!(
        refused(&proposed),
        vec![
            (ids[1].0, ErrorCode::StaleTarget),
            (ids[2].0, ErrorCode::StaleTarget)
        ]
    );
    assert_eq!(exposure(e, &f.photos[0]), Some(0.5));
    assert_eq!(exposure(e, &f.photos[1]), None);
    assert_eq!(exposure(e, &f.photos[2]), None);
    // One History step, named after the command.
    let history = e
        .library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .load_history(ids[0])
        .unwrap()
        .unwrap();
    assert_eq!(history.steps.len(), 1);
    assert_eq!(history.steps[0].name, "Brighten");
}

#[test]
fn a_token_is_refused_whole_once_its_catalog_is_closed_switched_or_read_again() {
    let mut f = fixture(&["a.png", "b.png"]);
    let e = &mut f.editor;
    let a = f.photos[0].0;
    let token = e.read_photos(&[a]).unwrap().token;
    // Another catalog open in its place.
    let other = f.dir.path().join("other.rawmakase");
    crate::catalog::Catalog::create(&other)
        .unwrap()
        .add_folder(f.photos[0].1.parent().unwrap())
        .unwrap();
    let first = e.library.take();
    e.library = Some(Box::new(
        crate::app::library::Library::load(&other, e.context.clone()).unwrap(),
    ));
    let switched = e.propose(proposal(&token, a, 1.0), "Test").unwrap_err();
    assert_eq!(switched.code, ErrorCode::StaleTarget);
    // Closed.
    e.library = None;
    assert_eq!(
        e.propose(proposal(&token, a, 1.0), "Test")
            .unwrap_err()
            .code,
        ErrorCode::StaleTarget
    );
    // The photos removed and added again, as the same ids may be: the catalog
    // is read again, and the token's generation is gone.
    e.library = first;
    let token = e.read_photos(&[a]).unwrap().token;
    let library = e.library.as_mut().unwrap();
    let folder = library.session.folders()[0].id;
    let catalog = library.session.catalog_mut_for_tests();
    catalog.remove_folders(&[folder]).unwrap();
    catalog.add_folder(f.photos[0].1.parent().unwrap()).unwrap();
    library.session.reload().unwrap();
    assert_eq!(
        e.propose(proposal(&token, a, 1.0), "Test")
            .unwrap_err()
            .code,
        ErrorCode::StaleTarget
    );
    assert_eq!(exposure(e, &f.photos[0]), None);
}

#[test]
fn a_photo_open_in_develop_is_checked_against_its_live_state() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    let photo = f.photos[0].clone();
    let ctx = e.context.clone();
    // Two reads of its catalog row while it isn't open.
    let first = e.read_photos(&[photo.0]).unwrap().token;
    let second = e.read_photos(&[photo.0]).unwrap().token;
    // Opened with nothing unsaved: the first applies through Develop, as one
    // step.
    open(e, &photo);
    let proposed = e
        .propose(proposal(&first, photo.0, 0.7), "Brighten")
        .unwrap();
    assert_eq!(proposed.applied, vec![photo.0.0]);
    assert!((e.document.edit.recipe().exposure - 0.7).abs() < 1e-6);
    let steps = e.document.edit.history().steps().0;
    assert_eq!(steps.last().unwrap().name, "Brighten");
    // That change isn't saved yet: the second read no longer describes the
    // photo, and is refused.
    assert!(e.document.edit.save_state().needs_save());
    let proposed = e
        .propose(proposal(&second, photo.0, 1.0), "Brighten")
        .unwrap();
    assert_eq!(
        refused(&proposed),
        vec![(photo.0.0, ErrorCode::StaleTarget)]
    );
    // Read from Develop, then edited there: refused.
    let live = e.read_open_photo().unwrap().token;
    crate::app::testing::in_edit_frame(&ctx, e, |e| {
        e.document.edit.recipe_mut().contrast = 0.2;
    });
    let proposed = e
        .propose(proposal(&live, photo.0, 1.0), "Brighten")
        .unwrap();
    assert_eq!(
        refused(&proposed),
        vec![(photo.0.0, ErrorCode::StaleTarget)]
    );
    assert!((e.document.edit.recipe().exposure - 0.7).abs() < 1e-6);
    // Read from Develop and still as read: applied.
    let live = e.read_open_photo().unwrap().token;
    let proposed = e
        .propose(proposal(&live, photo.0, 0.9), "Brighten")
        .unwrap();
    assert_eq!(proposed.applied, vec![photo.0.0]);
    assert!((e.document.edit.recipe().exposure - 0.9).abs() < 1e-6);
}

/// Opens `photo` in Develop and waits until it has loaded.
fn open(e: &mut Editor, photo: &(PhotoId, PathBuf)) {
    let ctx = e.context.clone();
    e.module = Module::Develop;
    e.open_raw(photo.1.clone(), Some(photo.0));
    let until = Instant::now() + Duration::from_secs(30);
    while e.document.loading() || e.document.full().is_none() {
        assert!(Instant::now() < until, "the photo never opened");
        e.events(&ctx);
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn proposals_after_a_command_was_stopped_or_timed_out_change_nothing() {
    let mut f = fixture_with(&["a.png"], Duration::from_millis(400));
    let e = &mut f.editor;
    select(e, &[f.photos[0].0]);
    let late = |o: &[Outcome]| {
        o.iter().find_map(|o| match o {
            Outcome::Message { text, .. } if text.contains("late proposal") => Some(text.clone()),
            _ => None,
        })
    };
    // Stopped by the user.
    e.run_extension_command(&format!("{ID}/propose-late"), RunBy::Button)
        .unwrap();
    pump(e, "the command to run", waiting);
    let stopped = e.extensions.cancel_commands(ID);
    assert!(
        matches!(&stopped[..], [Outcome::Command { result: Err(err), .. }] if err.code == ErrorCode::Cancelled)
    );
    let outcomes = pump(e, "the late proposal", |_, o| late(o).is_some());
    assert!(
        late(&outcomes).unwrap().ends_with("not_admitted"),
        "{outcomes:?}"
    );
    assert_eq!(exposure(e, &f.photos[0]), None);
    // Past its deadline.
    e.run_extension_command(&format!("{ID}/propose-late"), RunBy::Button)
        .unwrap();
    let outcomes = pump(e, "the deadline", |_, o| {
        late(o).is_some() && result(o, "Propose After Cancellation").is_some()
    });
    assert!(
        late(&outcomes).unwrap().ends_with("not_admitted"),
        "{outcomes:?}"
    );
    assert_eq!(
        result(&outcomes, "Propose After Cancellation")
            .unwrap()
            .as_ref()
            .unwrap_err()
            .code,
        ErrorCode::Deadline
    );
    assert_eq!(exposure(e, &f.photos[0]), None);
}

#[test]
fn disabling_an_extension_mid_request_stops_it_and_takes_its_commands_away() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    let id = format!("{ID}/wait");
    e.run_extension_command(&id, RunBy::Button).unwrap();
    pump(e, "the command to run", waiting);
    // The status line says what the program says while it runs.
    let work = e.running_work();
    let entry = work.iter().find(|w| w.key == "extensions").unwrap();
    assert_eq!(entry.text, "Example: Waiting");
    let killer_count = e.extensions.killers.len();
    assert_eq!(killer_count, 1);
    e.set_extension_enabled(ID, false);
    let x = e.extensions.get(ID).unwrap();
    assert!(!x.enabled() && !x.running());
    assert!(!State::read(&f.data).unwrap().developer[ID].enabled);
    assert!(e.keymap.effective().command(&id).is_none());
    assert!(e.running_work().iter().all(|w| w.key != "extensions"));
    assert_eq!(
        e.run_extension_command(&id, RunBy::Key).unwrap_err().code,
        "unknown_action"
    );
    let until = Instant::now() + Duration::from_secs(10);
    while !e.extensions.all_ended() {
        assert!(Instant::now() < until, "the program outlived its extension");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_commands_keys_survive_its_extension_being_turned_off_and_on() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    let id = format!("{ID}/settings");
    let chord = crate::app::shell::keymap::Chord::parse("Ctrl+Alt+S").unwrap();
    e.keymap.set(&id, &[chord]).unwrap();
    assert_eq!(e.keymap.effective().user_chords(&id), Some(&[chord][..]));
    e.set_extension_enabled(ID, false);
    assert_eq!(e.keymap.effective().user_chords(&id), None);
    assert!(e.keymap.unknown().contains(&id));
    e.set_extension_enabled(ID, true);
    assert_eq!(e.keymap.effective().user_chords(&id), Some(&[chord][..]));
    assert!(e.keymap.unknown().is_empty());
}

#[test]
fn an_extensions_key_that_another_command_has_is_a_conflict() {
    use crate::app::shell::keymap::{Chord, Effective, conflicts::clashes};
    let pick = crate::app::shell::commands::registry::bindings::PICK.defaults[0];
    let key = Chord::new(pick.keys[0], pick.mods);
    let dynamic = crate::app::shell::commands::registry::extension_command(
        "dev.test.keys/pick",
        "Pick it",
        Some(key),
    );
    let map = Effective::default().with_dynamic(vec![dynamic]);
    let found = clashes(&map, false);
    assert!(
        found.iter().any(|c| c.conflict
            && [c.a.command, c.b.command].contains(&"pick")
            && [c.a.command, c.b.command].contains(&"dev.test.keys/pick")),
        "no conflict between the extension's command and Pick"
    );
    // Interned once: asked again, the same command.
    assert!(std::ptr::eq(
        dynamic,
        crate::app::shell::commands::registry::extension_command(
            "dev.test.keys/pick",
            "Pick it",
            Some(key)
        )
    ));
}

#[test]
fn the_command_line_and_the_app_change_extensions_json_without_losing_each_other() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    // The command line turns it off while the app changes a setting.
    Store::new(&f.data).set_enabled(ID, false).unwrap();
    e.extensions.set_setting(ID, "step", json!(1.5)).unwrap();
    let state = State::read(&f.data).unwrap();
    assert!(!state.developer[ID].enabled);
    assert_eq!(state.extensions[ID].settings["step"], json!(1.5));
    // The page opening reads it again.
    e.extensions_page_opened();
    assert!(!e.extensions.get(ID).unwrap().enabled());
    // A file this RAWmakase won't write: nothing changes, here or there.
    std::fs::write(f.data.join("extensions.json"), r#"{"version": 99}"#).unwrap();
    assert!(e.extensions.set_setting(ID, "step", json!(0.25)).is_err());
    assert_eq!(e.extensions.get(ID).unwrap().settings()["step"], json!(1.5));
    assert!(e.extensions.set_enabled(ID, true).is_err());
    assert!(!e.extensions.get(ID).unwrap().enabled());
    assert_eq!(
        std::fs::read_to_string(f.data.join("extensions.json")).unwrap(),
        r#"{"version": 99}"#
    );
}

#[test]
fn three_crashes_in_a_row_make_an_extension_failed_until_retry() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    let crashed = |e: &Editor| !e.extensions.get(ID).unwrap().running();
    // Two crashes, then useful work: the count starts again.
    for _ in 0..2 {
        assert_eq!(
            run(e, "crash", "Crash").unwrap_err().code,
            ErrorCode::Exited
        );
        pump(e, "the crash", |e, _| crashed(e));
    }
    assert_eq!(
        run(e, "settings", "Show Settings"),
        Ok(json!({"step": 0.5}))
    );
    for _ in 0..2 {
        run(e, "crash", "Crash").unwrap_err();
        pump(e, "the crash", |e, _| crashed(e));
    }
    assert!(!e.extensions.get(ID).unwrap().failed);
    run(e, "crash", "Crash").unwrap_err();
    pump(e, "the crash", |e, _| crashed(e));
    let x = e.extensions.get(ID).unwrap();
    assert!(x.failed);
    assert!(
        e.run_extension_command(&format!("{ID}/settings"), RunBy::Button)
            .is_err()
    );
    e.extensions.retry(ID);
    assert_eq!(
        run(e, "settings", "Show Settings"),
        Ok(json!({"step": 0.5}))
    );
}

#[test]
fn starts_that_only_shake_hands_before_crashing_count_as_crashes() {
    let mut f = fixture(&["a.png"]);
    std::fs::write(f.folder.join("crash-on-event"), b"").unwrap();
    let e = &mut f.editor;
    for _ in 0..3 {
        // Started by a request that fails, which is no useful work; then an
        // event makes it exit.
        assert_eq!(
            run(e, "refuse", "Refuse").unwrap_err().code,
            ErrorCode::InvalidRequest
        );
        e.extensions.broadcast(
            &rawmakase_extensions::protocol::method::AppEvent::SelectionChanged(Default::default()),
        );
        pump(e, "the crash", |e, _| {
            !e.extensions.get(ID).unwrap().running()
        });
    }
    assert!(e.extensions.get(ID).unwrap().failed);
}

#[test]
fn quitting_with_a_hung_program_ends_it_by_the_deadline() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    e.run_extension_command(&format!("{ID}/hang"), RunBy::Button)
        .unwrap();
    pump(e, "the program to hang", |e, _| {
        e.extensions.get(ID).unwrap().status.as_deref() == Some("Hanging")
    });
    let started = Instant::now();
    e.exit_within(Duration::from_millis(1500));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "{:?}",
        started.elapsed()
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !e.extensions.all_ended() {
        assert!(Instant::now() < until, "the hung program outlived quitting");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn quitting_after_the_deadline_has_passed_still_ends_the_program() {
    let mut f = fixture(&["a.png"]);
    let e = &mut f.editor;
    e.run_extension_command(&format!("{ID}/hang"), RunBy::Button)
        .unwrap();
    pump(e, "the program to hang", |e, _| {
        e.extensions.get(ID).unwrap().status.as_deref() == Some("Hanging")
    });
    let started = Instant::now();
    e.exit_within(Duration::ZERO);
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    let until = Instant::now() + Duration::from_secs(5);
    while !e.extensions.all_ended() {
        assert!(Instant::now() < until, "the program outlived quitting");
        std::thread::sleep(Duration::from_millis(5));
    }
}
