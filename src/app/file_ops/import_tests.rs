//! Import Photos in the app (#806): the dialog's choices as the engine's
//! request, the open edit saved first, the catalog let go however the run
//! ends, a stale scan dropped, and the reporter's Fujifilm RAF imported By
//! Date with a template; driven by frames on synthetic files in a temporary
//! folder, the scan's file reads given by the test.
use super::tests::frame;
use crate::app::Editor;
use crate::app::testing::editor_with_catalog;
use crate::catalog::file_ops::{DateFolders, Organize};
use crate::export_settings::NameFields;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Frames until the dialog's scan and plan are in and no run is under way.
fn settle(e: &mut Editor) {
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until {
        let ctx = e.context.clone();
        e.events(&ctx);
        frame(e, Vec::new());
        let busy = e.file_ops.import_running()
            || (e.file_ops.import_open() && e.file_ops.import_scanning());
        if !busy {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("Import Photos never settled");
}

/// Whether copies can be imported into `destination`'s drive: one that
/// proves file ids, which overlayfs in CI's Linux containers doesn't, and
/// is refused as any such drive is. The engine's tests cover that refusal
/// and every step on a test disk; these real-disk tests say so and end
/// there, as the folder Trash's do.
fn copies_proven(destination: &Path) -> bool {
    let proven = rawmakase_fs::volume_facts(destination).is_ok_and(|f| f.stable_ids);
    if !proven {
        eprintln!("{}: file ids not proven here", destination.display());
    }
    proven
}

/// A reader of the scan's files: each one taken on 24 April 2026, a minute
/// apart by its name's order, but `undated` ones.
fn dated_reader(undated: &'static [&'static str]) -> super::import::FileReader {
    Arc::new(move |path: &Path| {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let minute = name.bytes().map(u32::from).sum::<u32>() % 60;
        Ok(NameFields {
            captured: (!undated.contains(&name.as_str()))
                .then(|| format!("2026-04-24T10:{minute:02}:00.000")),
            ..Default::default()
        })
    })
}

/// A card of `names` below `DCIM/100FUJI`, its folder; each file last
/// modified at noon UTC on 1 May 2026, the date an undated one is filed
/// under in any time zone.
fn card(d: &Path, names: &[&str]) -> anyhow::Result<PathBuf> {
    let dcim = d.join("card/DCIM/100FUJI");
    std::fs::create_dir_all(&dcim)?;
    let noon = std::time::UNIX_EPOCH + Duration::from_secs(1_777_636_800);
    for name in names {
        std::fs::write(dcim.join(name), format!("card {name}"))?;
        std::fs::File::options()
            .write(true)
            .open(dcim.join(name))?
            .set_modified(noon)?;
    }
    Ok(d.join("card/DCIM").canonicalize()?)
}

/// An editor on a catalog of `a.RAF`, Import Photos open on a card of
/// `names` with the library folder as destination, scanned.
fn open_dialog(names: &[&str]) -> anyhow::Result<(tempfile::TempDir, Editor, PathBuf, PathBuf)> {
    let (d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let root = d.path().canonicalize()?;
    let source = card(&root, names)?;
    let library = root.join("library");
    std::fs::create_dir(&library)?;
    e.file_ops.import_reader = Some(dated_reader(&["IMG_3.RAF"]));
    e.request_import();
    assert!(e.file_ops.import_open());
    e.file_ops.import_source(&source);
    e.file_ops.import_destination(&library);
    frame(&mut e, Vec::new());
    settle(&mut e);
    Ok((d, e, source, library))
}

#[test]
fn the_dialogs_choices_are_the_engines_request() -> anyhow::Result<()> {
    // `a.RAF` with the catalog's photo's bytes: a suspected duplicate.
    let (d, mut e, source, library) = open_dialog(&["IMG_1.RAF", "IMG_2.RAF", "IMG_3.RAF"])?;
    std::fs::write(source.join("100FUJI/a.RAF"), b"fixture")?;
    // A new source is a new scan.
    e.file_ops.import_source(&source);
    frame(&mut e, Vec::new());
    settle(&mut e);
    let rows = e.file_ops.import_rows();
    let names: Vec<(&str, bool, bool, bool)> = rows
        .iter()
        .map(|(n, checked, dup, dated)| (n.as_str(), *checked, *dup, *dated))
        .collect();
    assert_eq!(
        names,
        [
            ("IMG_1.RAF", true, false, true),
            ("IMG_2.RAF", true, false, true),
            // No date: marked so, and still checked.
            ("IMG_3.RAF", true, false, false),
            // A suspected duplicate is greyed out, not imported.
            ("a.RAF", false, true, true),
        ]
    );
    let settings = e.file_ops.import_settings_mut();
    settings.organize = Organize::ByDate(DateFolders::YearThenMonth);
    settings.rename = true;
    settings
        .naming
        .choose_tokens("{{custom_token}}-{{naming_sequenceNumber_3Digits}}");
    settings.naming.custom_text = "Porto".into();
    settings.naming.start_number = 7;
    settings.into_subfolder = true;
    settings.subfolder = " Trip ".into();
    settings.shoot_name = "Porto 2026".into();
    settings.extensions = crate::catalog::file_ops::ExtensionCase::Lowercase;
    settings.creator = "Ann Lee".into();
    settings.copyright = "© Ann Lee".into();
    settings.keywords = "Porto, Places > Portugal".into();
    e.file_ops.import_check("IMG_2.RAF", false);
    // Checking a greyed-out one takes nothing.
    e.file_ops.import_check("a.RAF", true);
    let request = e.file_ops.import_request().expect("something to import");
    let sources: Vec<String> = request
        .sources
        .iter()
        .map(|s| {
            s.file
                .path
                .file_name()
                .unwrap()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert_eq!(sources, ["IMG_1.RAF", "IMG_3.RAF"]);
    assert_eq!(request.source, source);
    assert_eq!(request.destination, library);
    assert_eq!(request.subfolder, "Trip");
    assert_eq!(
        request.organize,
        Organize::ByDate(DateFolders::YearThenMonth)
    );
    let naming = request.naming.as_ref().expect("Rename Files");
    assert_eq!(naming.start, 7);
    assert_eq!(naming.text, "Porto");
    assert_eq!(request.shoot_name, "Porto 2026");
    assert_eq!(
        request.extensions,
        crate::catalog::file_ops::ExtensionCase::Lowercase
    );
    assert!(request.skip_duplicates);
    assert_eq!(
        request.apply,
        crate::catalog::file_ops::ImportMetadata {
            creator: "Ann Lee".into(),
            copyright: "© Ann Lee".into(),
            keywords: vec![
                vec!["Porto".into()],
                vec!["Places".into(), "Portugal".into()]
            ],
        }
    );
    // Without Don't Import Suspected Duplicates it is imported as checked.
    e.file_ops.import_settings_mut().skip_duplicates = false;
    let request = e.file_ops.import_request().unwrap();
    assert_eq!(request.sources.len(), 3);
    assert!(!request.skip_duplicates);
    // Keywords that don't read plan nothing.
    e.file_ops.import_settings_mut().keywords = "Places > ".into();
    assert!(e.file_ops.import_request().is_none());
    e.file_ops.import_settings_mut().keywords.clear();
    e.file_ops.import_settings_mut().into_subfolder = false;
    let request = e.file_ops.import_request().unwrap();
    assert_eq!(request.subfolder, "", "the name kept, not used");
    // What the scan read is what goes: the capture time with it.
    assert!(request.sources[0].captured.is_some());
    assert_eq!(request.sources[1].captured, None);
    // Keeping names: no template.
    e.file_ops.import_settings_mut().rename = false;
    assert!(e.file_ops.import_request().unwrap().naming.is_none());
    drop(d);
    Ok(())
}

#[test]
fn photos_are_copied_into_date_folders_named_by_the_template() -> anyhow::Result<()> {
    let (_d, mut e, source, library) = open_dialog(&["IMG_1.RAF", "IMG_2.RAF", "IMG_3.RAF"])?;
    if !copies_proven(&library) {
        return Ok(());
    }
    let settings = e.file_ops.import_settings_mut();
    settings.organize = Organize::ByDate(DateFolders::YearThenDate);
    settings.rename = true;
    settings
        .naming
        .choose_tokens("{{custom_token}}-{{naming_sequenceNumber_1Digit}}");
    settings.naming.custom_text = "Trip".into();
    settings.naming.start_number = 7;
    frame(&mut e, Vec::new());
    settle(&mut e);
    let plan = e.file_ops.import_plan().expect("planned")?;
    assert!(plan.can_run(), "{plan:?}");
    e.press_import();
    assert!(e.activity.is_busy(), "the run holds the catalog");
    settle(&mut e);
    assert!(!e.activity.is_busy());
    let dated = library.join("2026/2026-04-24");
    let mut copied: Vec<String> = std::fs::read_dir(&dated)?
        .map(|f| f.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    copied.sort();
    assert_eq!(copied, ["Trip-7.RAF", "Trip-8.RAF"]);
    // No capture time: filed and named by its file's modification date.
    assert!(library.join("2026/2026-05-01/Trip-9.RAF").is_file());
    // The originals stay.
    assert!(source.join("100FUJI/IMG_1.RAF").is_file());
    let shown = e.library.as_ref().unwrap();
    assert_eq!(shown.session.previous_import().len(), 3);
    assert!(
        e.messages.line().contains("Imported 3 photos"),
        "{}",
        e.messages.line()
    );
    // The choices are kept for the next import.
    assert_eq!(e.file_ops.import_settings.naming.start_number, 7);
    assert_eq!(
        e.file_ops.import_settings.destination.as_ref(),
        Some(&library)
    );
    Ok(())
}

#[test]
fn a_failed_save_stops_the_import_and_copies_nothing() -> anyhow::Result<()> {
    let (d, mut e, _, library) = open_dialog(&["IMG_1.RAF"])?;
    if !copies_proven(&library) {
        return Ok(());
    }
    let ids: Vec<_> = e
        .library
        .as_ref()
        .unwrap()
        .session
        .photos()
        .iter()
        .map(|p| p.id)
        .collect();
    e.document.catalog_photo = Some(ids[0]);
    e.document.opened_at(d.path().join("photos/a.RAF"));
    let before = e.document.edit.recipe().clone();
    e.document.edit.setup_mut().exposure = 1.;
    e.commit_edit(before, None);
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch(
            "CREATE TEMP TRIGGER fail_save BEFORE UPDATE OF recipe ON photos
             BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
        )?;
    assert!(e.file_ops.import_plan().expect("planned")?.can_run());
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(e.file_ops.import_open(), "the dialog stays");
    assert!(
        e.messages.line().contains("nothing was imported"),
        "{}",
        e.messages.line()
    );
    assert!(e.document.edit.save_state().needs_save());
    assert_eq!(std::fs::read_dir(&library)?.count(), 0, "nothing copied");
    Ok(())
}

#[test]
fn the_catalog_is_let_go_when_the_run_fails_panics_or_is_stopped() -> anyhow::Result<()> {
    // Stopped before its first photo: nothing is copied.
    let (_d, mut e, _, library) = open_dialog(&["IMG_1.RAF", "IMG_2.RAF"])?;
    if !copies_proven(&library) {
        return Ok(());
    }
    e.file_ops.stop_first = true;
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(!e.file_ops.import_open());
    assert!(
        e.messages.line().contains("2 photos not imported"),
        "{}",
        e.messages.line()
    );
    assert_eq!(std::fs::read_dir(&library)?.count(), 0);

    // Its worker panics.
    let (_d, mut e, _, _) = open_dialog(&["IMG_1.RAF"])?;
    e.file_ops.import_panics = true;
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(
        e.messages.line().contains("Stopped unexpectedly"),
        "{}",
        e.messages.line()
    );

    // It fails: another window holds the catalog's file operations.
    let (d, mut e, _, library) = open_dialog(&["IMG_1.RAF"])?;
    let lock = std::fs::File::create(d.path().join("test.rawmakase.file-operations-lock"))?;
    lock.lock()?;
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(
        e.messages.line().contains("stopped"),
        "{}",
        e.messages.line()
    );
    assert_eq!(std::fs::read_dir(&library)?.count(), 0);
    Ok(())
}

#[test]
fn a_plan_changed_under_the_lock_is_shown_again_and_copies_nothing() -> anyhow::Result<()> {
    let (_d, mut e, _, library) = open_dialog(&["IMG_1.RAF"])?;
    if !copies_proven(&library) {
        return Ok(());
    }
    e.file_ops.import_settings_mut().organize = Organize::IntoOneFolder;
    frame(&mut e, Vec::new());
    settle(&mut e);
    assert!(e.file_ops.import_plan().unwrap()?.can_run());
    // Taken meanwhile, by another program.
    std::fs::write(library.join("IMG_1.RAF"), b"someone else's")?;
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(e.file_ops.import_open() && e.file_ops.import_changed());
    let plan = e.file_ops.import_plan().unwrap()?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("IMG_1-2.RAF").as_path())
    );
    // Confirmed, it runs.
    e.press_import();
    settle(&mut e);
    assert_eq!(
        std::fs::read(library.join("IMG_1-2.RAF"))?,
        b"card IMG_1.RAF"
    );
    assert_eq!(std::fs::read(library.join("IMG_1.RAF"))?, b"someone else's");
    Ok(())
}

#[test]
fn a_scan_of_an_earlier_source_is_dropped() -> anyhow::Result<()> {
    let (d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let root = d.path().canonicalize()?;
    let first = root.join("first");
    let second = root.join("second");
    std::fs::create_dir_all(&first)?;
    std::fs::create_dir_all(&second)?;
    std::fs::write(first.join("OLD.RAF"), b"old")?;
    std::fs::write(second.join("NEW.RAF"), b"new")?;
    // The first source's read waits until the second one is scanned.
    let (release, wait) = std::sync::mpsc::channel::<()>();
    let wait = Arc::new(Mutex::new(wait));
    let reader_first = first.clone();
    e.file_ops.import_reader = Some(Arc::new(move |path: &Path| {
        if path.starts_with(&reader_first) {
            let _ = wait.lock().unwrap().recv_timeout(Duration::from_secs(10));
        }
        Ok(NameFields::default())
    }));
    e.request_import();
    e.file_ops.import_source(&first);
    frame(&mut e, Vec::new());
    let stale = e.file_ops.import_generation();
    e.file_ops.import_source(&second);
    frame(&mut e, Vec::new());
    assert_ne!(e.file_ops.import_generation(), stale);
    settle(&mut e);
    assert_eq!(e.file_ops.import_rows()[0].0, "NEW.RAF");
    // The first scan ends now, its last file read, and answers: dropped.
    release.send(())?;
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        let ctx = e.context.clone();
        e.events(&ctx);
        std::thread::sleep(Duration::from_millis(10));
    }
    let rows = e.file_ops.import_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "NEW.RAF");
    Ok(())
}

#[test]
fn a_file_that_cant_be_read_is_listed_with_why_and_not_imported() -> anyhow::Result<()> {
    let (_d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let root = _d.path().canonicalize()?;
    let source = card(&root, &["IMG_1.RAF", "BAD.RAF"])?;
    e.file_ops.import_reader = Some(Arc::new(|path: &Path| {
        match path.file_name().unwrap() == "BAD.RAF" {
            true => Err("BAD.RAF can't be read".into()),
            false => Ok(NameFields::default()),
        }
    }));
    e.request_import();
    e.file_ops.import_source(&source);
    frame(&mut e, Vec::new());
    settle(&mut e);
    let rows = e.file_ops.import_rows();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "IMG_1.RAF");
    let unreadable = e.file_ops.import_unreadable();
    assert_eq!(unreadable.len(), 1);
    assert!(unreadable[0].0.ends_with("BAD.RAF"));
    assert_eq!(unreadable[0].1, "BAD.RAF can't be read");
    Ok(())
}

/// The reporter's case (#806): a Fujifilm X-T20 RAF from a card, By Date
/// from the capture time the RAF itself gives, named by a template with
/// Original filename and Import #. The file is raw.pixls.us' CC0 sample
/// (`scripts/corpus/pixls.py download`, never committed); without
/// RAWMAKASE_CORPUS, or the file, the test says so and passes.
#[test]
fn the_reporters_fujifilm_raf_is_imported_by_date_with_a_template() -> anyhow::Result<()> {
    let Some(corpus) = std::env::var_os("RAWMAKASE_CORPUS") else {
        eprintln!("skipped: RAWMAKASE_CORPUS isn't set");
        return Ok(());
    };
    let raf = PathBuf::from(corpus)
        .join("raws/pixls/Fujifilm/X-T20/Fujifilm - X-T20 - 14bit 14bit compressed (3:2).RAF");
    if !raf.is_file() {
        eprintln!("skipped: {} isn't there", raf.display());
        return Ok(());
    }
    let (d, mut e, _) = editor_with_catalog(&["a.RAF"])?;
    let root = d.path().canonicalize()?;
    let dcim = root.join("card/DCIM/100_FUJI");
    std::fs::create_dir_all(&dcim)?;
    std::fs::copy(&raf, dcim.join("DSCF0001.RAF"))?;
    let library = root.join("library");
    std::fs::create_dir(&library)?;
    if !copies_proven(&library) {
        return Ok(());
    }
    // The real reader: the capture time is the RAF's own.
    e.request_import();
    e.file_ops.import_source(&root.join("card/DCIM"));
    e.file_ops.import_destination(&library);
    let settings = e.file_ops.import_settings_mut();
    settings.organize = Organize::ByDate(DateFolders::YearThenDate);
    settings.rename = true;
    settings
        .naming
        .choose_tokens("{{naming_importNumber_3Digits}}_{{image_originalName}}");
    frame(&mut e, Vec::new());
    settle(&mut e);
    let rows = e.file_ops.import_rows();
    assert_eq!(rows.len(), 1, "{:?}", e.file_ops.import_unreadable());
    assert!(rows[0].3, "the RAF gives its capture time");
    let captured = e.file_ops.import_request().unwrap().sources[0]
        .captured
        .clone()
        .unwrap();
    let plan = e.file_ops.import_plan().expect("planned")?;
    assert!(plan.can_run(), "{plan:?}");
    let to = plan.photos[0].destination.clone().unwrap();
    let (year, date) = (&captured[0..4], &captured[0..10]);
    assert_eq!(to.parent().unwrap(), library.join(year).join(date));
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    assert!(to.is_file(), "{} not copied", to.display());
    assert_eq!(
        std::fs::metadata(&to)?.len(),
        std::fs::metadata(&raf)?.len()
    );
    let name = to.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.ends_with("DSCF0001.RAF"), "{name}");
    let shown = e.library.as_ref().unwrap();
    assert_eq!(shown.session.previous_import().len(), 1);
    Ok(())
}

#[test]
fn a_run_failing_after_photos_were_added_says_what_it_added() -> anyhow::Result<()> {
    let (_d, mut e, _source, _library) = open_dialog(&["IMG_1.RAF", "IMG_2.RAF"])?;
    if !copies_proven(&_library) {
        return Ok(());
    }
    // The run's end can't be journalled, every photo added before it.
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute_batch(
            "CREATE TRIGGER finish_fails BEFORE UPDATE OF state ON file_operations
             WHEN NEW.state='done' AND OLD.preconditions LIKE '%\"finish\"%'
             BEGIN SELECT RAISE(FAIL, 'disk full'); END",
        )?;
    let plan = e.file_ops.import_plan().expect("planned")?;
    assert!(plan.can_run(), "{plan:?}");
    e.press_import();
    settle(&mut e);
    assert!(!e.activity.is_busy());
    let line = e.messages.line();
    assert!(
        line.contains("Imported 2 photos") && line.contains("stopped"),
        "{line}"
    );
    assert_eq!(
        e.library.as_ref().unwrap().session.previous_import().len(),
        2
    );
    Ok(())
}

#[test]
fn a_photo_found_in_the_catalog_since_the_scan_is_shown_as_a_duplicate() -> anyhow::Result<()> {
    let (_d, mut e, _source, _library) = open_dialog(&["IMG_1.RAF", "IMG_2.RAF"])?;
    if !copies_proven(&_library) {
        return Ok(());
    }
    assert!(
        e.file_ops.import_rows().iter().all(|r| !r.2),
        "none suspected at the scan"
    );
    // Another import added it meanwhile: the catalog knows its name and size.
    let size = "card IMG_1.RAF".len() as i64;
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute(
            "UPDATE photo_import_facts SET original_name='IMG_1.RAF', size=?1",
            [size],
        )?;
    // The next plan finds it, and the dialog shows it so, unchecked.
    e.file_ops.import_settings_mut().organize = Organize::ByDate(DateFolders::Date);
    frame(&mut e, Vec::new());
    settle(&mut e);
    let rows = e.file_ops.import_rows();
    let first = rows.iter().find(|r| r.0 == "IMG_1.RAF").unwrap();
    // (name, checked, suspected, dated)
    assert!(first.2 && !first.1, "{rows:?}");
    Ok(())
}

#[test]
fn a_photo_found_in_the_catalog_as_import_runs_is_shown_as_a_duplicate() -> anyhow::Result<()> {
    let (_d, mut e, _source, library) = open_dialog(&["IMG_1.RAF", "IMG_2.RAF"])?;
    if !copies_proven(&library) {
        return Ok(());
    }
    let plan = e.file_ops.import_plan().expect("planned")?;
    assert!(plan.can_run());
    // Another import adds it after the preview, before Import.
    let size = "card IMG_1.RAF".len() as i64;
    e.library
        .as_ref()
        .unwrap()
        .session
        .catalog()
        .db_for_tests()
        .execute(
            "UPDATE photo_import_facts SET original_name='IMG_1.RAF', size=?1",
            [size],
        )?;
    e.press_import();
    settle(&mut e);
    // Nothing copied; the dialog asks again, the photo marked and unchecked.
    assert!(e.file_ops.import_open());
    assert_eq!(std::fs::read_dir(&library)?.count(), 0);
    let rows = e.file_ops.import_rows();
    let first = rows.iter().find(|r| r.0 == "IMG_1.RAF").unwrap();
    // (name, checked, suspected, dated)
    assert!(first.2 && !first.1, "{rows:?}");
    Ok(())
}

/// A plan in flight when Import is pressed never comes back to the dialog:
/// a plan changed under the lock must leave the dialog planning again.
#[test]
fn the_dialog_plans_again_after_a_run_found_the_names_changed() -> anyhow::Result<()> {
    let (_d, mut e, _, _library) = open_dialog(&["IMG_1.RAF"])?;
    if !copies_proven(&_library) {
        return Ok(());
    }
    let a = e.file_ops.import_plan().unwrap()?;
    assert!(a.can_run());
    // A plan for other choices is in flight.
    e.file_ops.hold_plans = true;
    e.file_ops.import_settings_mut().organize = Organize::IntoOneFolder;
    frame(&mut e, Vec::new());
    // Back to the choices of the plan shown: Import is on again.
    e.file_ops.import_settings_mut().organize = Organize::ByDate(DateFolders::YearThenDate);
    frame(&mut e, Vec::new());
    e.file_ops.hold_plans = false;
    assert!(e.file_ops.import_plan().is_some(), "plan A shown again");
    // The name is taken meanwhile.
    let to = a.photos[0].destination.clone().unwrap();
    std::fs::create_dir_all(to.parent().unwrap())?;
    std::fs::write(&to, b"someone else's")?;
    e.press_import();
    let until = Instant::now() + Duration::from_secs(10);
    while Instant::now() < until && !e.file_ops.import_open() {
        let ctx = e.context.clone();
        e.events(&ctx);
        frame(&mut e, Vec::new());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(e.file_ops.import_open() && e.file_ops.import_changed());
    // Now the user changes a choice: it must be planned.
    e.file_ops.import_settings_mut().organize = Organize::IntoOneFolder;
    let until = Instant::now() + Duration::from_secs(5);
    let mut planned = false;
    while Instant::now() < until {
        let ctx = e.context.clone();
        e.events(&ctx);
        frame(&mut e, Vec::new());
        if e.file_ops.import_plan().is_some() {
            planned = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(planned, "the dialog never plans again");
    Ok(())
}
