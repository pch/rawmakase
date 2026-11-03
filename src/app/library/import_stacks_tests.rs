//! Stack Camera Bursts when adding photos, in the Library: read, made once
//! as one Undo step, kept to be done while a photo is offline or the read
//! is lost, and never made from a read of other photos.
use super::Library;
use crate::catalog::{Catalog, ImportOptions, NewPhotoDefaults, PhotoId};
use crate::exif::{Mark, Sequence};
use anyhow::Result;
use eframe::egui;
use std::path::{Path, PathBuf};

/// A Library of a folder of `names` added with Stack Camera Bursts on, or
/// off, each photo taken at its time in `times` (by name).
fn added(
    names: &[(&str, &str)],
    stack_bursts: bool,
) -> Result<(tempfile::TempDir, Library, Vec<PhotoId>)> {
    let d = tempfile::tempdir()?;
    let folder = d.path().join("photos");
    std::fs::create_dir(&folder)?;
    for (name, _) in names {
        std::fs::write(folder.join(name), b"fixture")?;
    }
    let path = d.path().join("import.rawmakase");
    let defaults = NewPhotoDefaults::default().with_options(ImportOptions {
        stack_bursts,
        ..Default::default()
    });
    Catalog::create(&path)?.add_folder_with(&folder, &defaults)?;
    let mut library = Library::load(&path, egui::Context::default())?;
    let ids = names
        .iter()
        .map(|(n, time)| {
            let photo = library
                .session
                .photos_mut_for_tests()
                .iter_mut()
                .find(|p| p.filename == *n)
                .unwrap();
            photo.captured = (*time).into();
            photo.id
        })
        .collect();
    Ok((d, library, ids))
}

/// What the test cameras say: a name's first letter is the body, the rest
/// up to the dot its frame ("a1.ARW" frame 1 of body a; "s" a single shot,
/// "c" continuous drive, "n" nothing).
fn camera(path: &Path) -> Option<Sequence> {
    let name = path.file_stem()?.to_str()?;
    let (body, rest) = name.split_at(1);
    let mark = match rest {
        "s" => Mark::Single,
        "n" => return None,
        r if r.starts_with('c') => Mark::Continuous,
        r => Mark::Frame(r.parse().ok()?),
    };
    Some(Sequence {
        camera: body.into(),
        mark,
    })
}

/// Reads the batches to stack to the end, each file by `read`.
fn read_all(library: &mut Library, read: fn(&Path) -> Option<Sequence>) {
    library.start_import_stacks_by(read);
    let start = std::time::Instant::now();
    while library.session.reading_import_stacks() {
        assert!(start.elapsed().as_secs() < 30, "reading never finished");
        library.poll_import_stacks();
        std::thread::yield_now();
    }
    library.poll_import_stacks();
}

fn pending(library: &Library) -> usize {
    library
        .session
        .catalog()
        .pending_burst_stacks()
        .unwrap()
        .len()
}

fn path_of(library: &Library, id: PhotoId) -> PathBuf {
    library.photo(id).unwrap().path.clone()
}

const TIMES: [(&str, &str); 7] = [
    ("a1.ARW", "2024-03-02T10:00:00.000"),
    ("a2.ARW", "2024-03-02T10:00:00.200"),
    ("b1.ARW", "2024-03-02T10:00:00.300"),
    ("a3.ARW", "2024-03-02T10:00:00.400"),
    ("as.ARW", "2024-03-02T10:00:05.000"),
    ("an.ARW", "2024-03-02T10:00:05.100"),
    ("bn.ARW", "2024-03-02T10:00:05.200"),
];

#[test]
fn bursts_of_photos_added_are_stacked_once_in_one_undo_step() -> Result<()> {
    let (_d, mut library, ids) = added(&TIMES, true)?;
    let [a1, a2, b1, a3, ..] = ids[..] else {
        panic!()
    };
    read_all(&mut library, camera);
    library.make_import_stacks();
    // The burst, not the other body's frame, the single shot, nor the
    // photos whose camera says nothing, however close in time.
    assert_eq!(library.session.catalog().stack_of(a1)?, [a1, a2, a3]);
    assert!(library.session.catalog().stack_of(b1)?.is_empty());
    assert_eq!(library.session.catalog().stack_places()?.len(), 3);
    assert_eq!(pending(&library), 0);
    let done = library.take_stack_done();
    assert_eq!(done.len(), 1, "one undo step");
    assert!(
        library.messages.line().contains("1 stack"),
        "{}",
        library.messages.line()
    );
    // What the files said is kept for Auto-Stack.
    assert_eq!(library.session.catalog().sequences()?.len(), ids.len());
    // Undone, it stays undone: nothing is to be done or read again.
    library.restore_stacks(&done[0].before)?;
    read_all(&mut library, |_| panic!("read again"));
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    assert!(library.take_stack_done().is_empty());
    Ok(())
}

#[test]
fn photos_added_without_the_option_are_left_alone() -> Result<()> {
    let (_d, mut library, _) = added(&TIMES, false)?;
    read_all(&mut library, |_| panic!("nothing to read"));
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    Ok(())
}

#[test]
fn continuous_drive_frames_stack_by_the_gap_kept_with_the_batch() -> Result<()> {
    let (_d, mut library, ids) = added(
        &[
            ("ac.ARW", "2024-03-02T10:00:00.000"),
            ("ac2.ARW", "2024-03-02T10:00:01.500"),
            ("ac3.ARW", "2024-03-02T10:00:04.000"),
            ("ac4.ARW", "2024-03-02T10:00:04.500"),
        ],
        true,
    )?;
    read_all(&mut library, camera);
    library.make_import_stacks();
    // Less than two seconds apart; 2.5 s splits.
    assert_eq!(
        library.session.catalog().stack_of(ids[0])?,
        [ids[0], ids[1]]
    );
    assert_eq!(
        library.session.catalog().stack_of(ids[2])?,
        [ids[2], ids[3]]
    );
    Ok(())
}

#[test]
fn a_batch_waits_for_a_photo_offline_and_is_stacked_once_it_is_back() -> Result<()> {
    let (_d, mut library, ids) = added(&TIMES, true)?;
    let a1 = ids[0];
    let gone = path_of(&library, ids[4]);
    std::fs::rename(&gone, gone.with_extension("away"))?;
    read_all(&mut library, camera);
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    assert_eq!(pending(&library), 1, "still to be done");
    assert!(library.take_stack_done().is_empty());
    std::fs::rename(gone.with_extension("away"), &gone)?;
    read_all(&mut library, camera);
    library.make_import_stacks();
    assert_eq!(library.session.catalog().stack_of(a1)?.len(), 3);
    assert_eq!(pending(&library), 0);
    Ok(())
}

#[test]
fn a_lost_read_leaves_the_batch_to_be_done() -> Result<()> {
    let (_d, mut library, ids) = added(&TIMES, true)?;
    read_all(&mut library, |_| panic!("worker lost"));
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    assert_eq!(pending(&library), 1);
    read_all(&mut library, camera);
    library.make_import_stacks();
    assert_eq!(library.session.catalog().stack_of(ids[0])?.len(), 3);
    Ok(())
}

#[test]
fn a_read_of_another_generation_is_read_again_and_waits_behind_auto_stack() -> Result<()> {
    let (_d, mut library, ids) = added(&TIMES, true)?;
    read_all(&mut library, camera);
    // Photos removed and added since may have other ids: read again.
    library.session.bump_generation();
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    assert!(library.session.reading_import_stacks(), "read again");
    // That read is the real cameras'; the test's reads them again.
    library.session.stop_background_reads();
    // Not while Auto-Stack's window is open.
    library.stack_action(super::StackAction::AutoStack, Vec::new());
    read_all(&mut library, camera);
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    library.auto_stack = None;
    library.make_import_stacks();
    assert_eq!(library.session.catalog().stack_of(ids[0])?.len(), 3);
    Ok(())
}

#[test]
fn suspended_reads_drop_what_was_read_and_photos_stacked_meanwhile_stay() -> Result<()> {
    let (_d, mut library, ids) = added(&TIMES, true)?;
    let [a1, a2, _, a3, ..] = ids[..] else {
        panic!()
    };
    read_all(&mut library, camera);
    library.suspend_background_reads();
    library.make_import_stacks();
    assert!(library.session.catalog().stack_places()?.is_empty());
    library.resume_background_reads();
    // Resuming reads again; meanwhile the user stacks two of the burst.
    library.session.stop_background_reads();
    library.select(Some(a2));
    library.stack_action(super::StackAction::Group, vec![a2, a3]);
    library.take_stack_done();
    read_all(&mut library, camera);
    library.make_import_stacks();
    assert_eq!(library.session.catalog().stack_of(a2)?, [a2, a3]);
    // a1 alone is left: no stack of one.
    assert!(library.session.catalog().stack_of(a1)?.is_empty());
    assert_eq!(pending(&library), 0);
    Ok(())
}

#[test]
fn a_photo_the_catalog_has_no_time_for_yet_is_timed_from_its_file() -> Result<()> {
    use crate::exif::{CameraExif, Field, tag};
    use crate::export::exif::{Container, camera_raw};
    let (_d, mut library, ids) = added(&[("a1.CR3", ""), ("a2.CR3", ""), ("a3.CR3", "")], true)?;
    // a3 has no time at all: left alone.
    for (id, time) in ids
        .iter()
        .zip(["2024:03:02 10:00:00", "2024:03:02 10:00:01", ""])
    {
        let mut exif = vec![];
        if !time.is_empty() {
            exif.push(Field::ascii(tag::DATE_TIME_ORIGINAL, time));
        }
        let camera = CameraExif {
            main: vec![Field::ascii(tag::MAKE, "Canon")],
            exif,
            gps: Vec::new(),
        };
        std::fs::write(path_of(&library, *id), camera_raw(Container::Cr3, camera))?;
    }
    assert!(
        library
            .session
            .photos()
            .iter()
            .all(|p| p.captured.is_empty())
    );
    read_all(&mut library, camera);
    library.make_import_stacks();
    assert_eq!(
        library.session.catalog().stack_of(ids[0])?,
        [ids[0], ids[1]]
    );
    assert!(library.session.catalog().stack_of(ids[2])?.is_empty());
    Ok(())
}

#[test]
fn removing_a_folder_while_reading_reads_the_batches_left_again() -> Result<()> {
    let (d, mut library, _) = added(&TIMES, true)?;
    // Another folder, added without the option.
    let other = d.path().join("other");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("x.ARW"), b"fixture")?;
    library.session.catalog_mut_for_tests().add_folder(&other)?;
    library.session.reload()?;
    let folder = library
        .session
        .photos()
        .iter()
        .find(|p| p.filename == "x.ARW")
        .unwrap()
        .folder;
    library.start_import_stacks_by(camera);
    library.remove_folders(&[folder].into(), "other", None)?;
    assert!(library.session.reading_import_stacks(), "read again");
    Ok(())
}

/// Lets [`gated`] reads go on.
static OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// [`camera`], once [`OPEN`].
fn gated(path: &Path) -> Option<Sequence> {
    while !OPEN.load(std::sync::atomic::Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    camera(path)
}

#[test]
fn files_coming_online_while_reading_are_read_once_it_is_done() -> Result<()> {
    let (_d, mut library, _) = added(&TIMES, true)?;
    library.start_import_stacks_by(gated);
    // Files come online meanwhile: asked to read, it reads again once done.
    library.start_import_stacks();
    assert!(!library.session.import_stacks_again(), "not while reading");
    OPEN.store(true, std::sync::atomic::Ordering::Relaxed);
    let start = std::time::Instant::now();
    while library.import_bursts.is_none() {
        assert!(start.elapsed().as_secs() < 30, "reading never finished");
        library.poll_import_stacks();
        std::thread::yield_now();
    }
    assert!(library.session.reading_import_stacks(), "read again");
    Ok(())
}

#[test]
fn a_refused_folder_removal_reads_the_batches_again() -> Result<()> {
    use crate::catalog::ExportScope;
    let (d, mut library, ids) = added(&TIMES, true)?;
    let folder = library.photo(ids[0]).unwrap().folder;
    let export = library.session.catalog().export_catalog(
        &ExportScope::Folders(vec![folder]),
        &d.path().join("out/Copy.rawmakase"),
    )?;
    let removal = export.removal.expect("removable");
    // Changed since it was exported: not removed.
    library
        .session
        .set_ratings(&[(ids[0], 5, 0, String::new())])?;
    library.start_import_stacks_by(camera);
    assert!(
        library
            .remove_folders(&[folder].into(), "photos", Some(&removal))
            .is_err()
    );
    assert!(library.session.reading_import_stacks(), "read again");
    Ok(())
}
