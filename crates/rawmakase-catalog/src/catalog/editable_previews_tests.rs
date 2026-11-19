//! Editable previews in the catalog (docs/editable-previews.md): a row per
//! master that virtual copies share, named after the identity it records,
//! which goes with its photo whichever release removes it, follows this
//! release's Set Copy as Master (an older release's promotion leaves the family
//! without one), goes out of date when the original comes back as another file,
//! and travels with its photo in Export as Catalog.
use super::editable_previews::{carry, folder};
use super::*;
use crate::storage::Identity;
use rusqlite::Connection;
use std::path::PathBuf;

/// A catalog of folder `photos` holding `files`, with each photo's id.
struct Fx {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    path: PathBuf,
    cat: Catalog,
    photos: Vec<(PhotoId, PathBuf)>,
}

fn fixture(files: &[&str]) -> Result<Fx> {
    let d = tempfile::tempdir()?;
    let dir = d.path().to_path_buf();
    let photos = dir.join("photos");
    std::fs::create_dir_all(&photos)?;
    for name in files {
        std::fs::write(photos.join(name), name.as_bytes())?;
    }
    let path = dir.join("Photos.rawmakase");
    let mut cat = Catalog::create(&path)?;
    cat.add_folder(&photos)?;
    let photos = files
        .iter()
        .map(|name| {
            let p = cat.photos()?;
            let photo = p.iter().find(|p| p.filename == *name).unwrap();
            Ok((photo.id, photo.path.clone()))
        })
        .collect::<Result<_>>()?;
    Ok(Fx {
        _dir: d,
        dir,
        path,
        cat,
        photos,
    })
}

/// Records a copy of master photo `id`, whose original is at `original`, with
/// a file standing for it under the name the catalog gives it; returns the
/// file.
fn record(cat: &mut Catalog, id: PhotoId, original: &Path) -> Result<PathBuf> {
    let identity = Identity::read(original)?;
    let file = super::editable_preview_file_name(id, &identity);
    let folder = cat.editable_previews_folder();
    std::fs::create_dir_all(&folder)?;
    std::fs::write(folder.join(&file), b"editable preview")?;
    assert_eq!(cat.record_editable_preview(id, &identity)?, None);
    assert_eq!(cat.editable_preview(id)?.unwrap().file, file);
    Ok(folder.join(file))
}

/// Which photos have rows, by id.
fn rows(cat: &Catalog) -> Result<Vec<i64>> {
    let mut statement = cat
        .db_for_tests()
        .prepare("SELECT photo FROM editable_previews ORDER BY photo")?;
    let rows = statement
        .query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    Ok(rows)
}

/// Set Copy as Master as a release that doesn't know the table does it.
fn promote_as_an_older_release(db: &Connection, copy: PhotoId, master: PhotoId) -> Result<()> {
    db.execute(
        "UPDATE photos SET master_id=?1 WHERE master_id=?2 AND id<>?1",
        [copy.0, master.0],
    )?;
    db.execute(
        "UPDATE photos SET master_id=?, copy_name='Copy 1' WHERE id=?",
        [copy.0, master.0],
    )?;
    db.execute(
        "UPDATE photos SET master_id=NULL, copy_name='' WHERE id=?",
        [copy.0],
    )?;
    Ok(())
}

#[test]
fn virtual_copies_open_from_their_masters_copy() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let copy = fx.cat.create_virtual_copy(a)?;
    // Recorded for the master alone: a virtual copy's build is its master's.
    assert!(
        fx.cat
            .record_editable_preview(copy, &Identity::read(&path)?)
            .is_err()
    );
    record(&mut fx.cat, a, &path)?;
    assert_eq!(rows(&fx.cat)?, [a.0]);
    let preview = fx.cat.editable_preview(copy)?.unwrap();
    assert_eq!(fx.cat.editable_preview(a)?.unwrap(), preview);
    assert_eq!(preview.master, a);
    assert_eq!(preview.format, RAWMAKASE_DNG);
    assert_eq!(preview.original, Identity::read(&path)?);
    assert!(!preview.out_of_date);
    assert_eq!(fx.cat.editable_previews()?, [preview]);
    Ok(())
}

#[test]
fn building_again_replaces_the_copy_and_hands_back_the_old_file() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let first = record(&mut fx.cat, a, &path)?;
    fx.cat.mark_editable_preview_out_of_date(a)?;
    let identity = Identity {
        modified_ns: 1,
        ..Identity::read(&path)?
    };
    let file = super::editable_preview_file_name(a, &identity);
    let old = fx.cat.record_editable_preview(a, &identity)?;
    assert_eq!(
        old.map(|f| fx.cat.editable_previews_folder().join(f)),
        Some(first)
    );
    let preview = fx.cat.editable_preview(a)?.unwrap();
    assert_eq!(
        (preview.file.as_str(), preview.out_of_date),
        (file.as_str(), false)
    );
    // The same original again hands nothing back: its name is the file just
    // written.
    assert_eq!(fx.cat.record_editable_preview(a, &identity)?, None);
    Ok(())
}

/// A row written another way than [`Catalog::record_editable_preview`] (a
/// damaged or hostile catalog) that names a file elsewhere names nothing.
#[test]
fn a_name_that_is_not_a_plain_file_name_is_never_followed() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let identity = Identity::read(&path)?;
    for name in [
        "",
        ".",
        "..",
        "../a.dng",
        "sub/a.dng",
        "a\\b.dng",
        ".hidden.dng",
    ] {
        assert!(!super::editable_previews::is_plain_name(name), "{name:?}");
    }
    fx.cat.db_for_tests().execute(
        "INSERT INTO editable_previews(photo, file, format, original, built_at)
         VALUES (?, '../Photos.rawmakase', 'rawmakase-dng-1', ?, '2026-11-17 00:00:00')",
        rusqlite::params![a.0, serde_json::to_string(&identity)?],
    )?;
    let preview = fx.cat.editable_preview(a)?.unwrap();
    let folder = fx.cat.editable_previews_folder();
    assert_eq!(preview.path_in(&folder), None);
    assert_eq!(preview.usable(&folder, |_| Ok(())), None);
    // Nor is it handed back for deletion, whether replaced or discarded.
    assert_eq!(fx.cat.record_editable_preview(a, &identity)?, None);
    fx.cat.db_for_tests().execute(
        "UPDATE editable_previews SET file='../victim.NEF' WHERE photo=?",
        rusqlite::params![a.0],
    )?;
    assert_eq!(
        fx.cat.discard_editable_previews(&[a])?,
        Vec::<String>::new()
    );
    assert!(
        fx.cat.editable_preview(a)?.is_none(),
        "the row is forgotten"
    );
    Ok(())
}

#[test]
fn the_copies_folder_is_beside_the_catalog() {
    let location = CatalogLocation::File(PathBuf::from("shoots").join("Photos.rawmakase"));
    assert_eq!(
        folder(&location),
        PathBuf::from("shoots").join("Photos.rawmakase-editable-previews")
    );
    let original = Identity {
        size: 12,
        modified_ns: 34,
        prefix_hash: "00ff".into(),
    };
    assert_eq!(
        super::editable_preview_file_name(PhotoId(7), &original),
        "7-00ff-12-34.dng"
    );
}

#[test]
fn removing_a_virtual_copy_keeps_its_masters_copy() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let copy = fx.cat.create_virtual_copy(a)?;
    let file = record(&mut fx.cat, a, &path)?;
    fx.cat.remove_virtual_copy(copy)?;
    assert_eq!(rows(&fx.cat)?, [a.0]);
    assert!(fx.cat.editable_preview_orphans()?.is_empty());
    assert!(file.is_file());
    Ok(())
}

#[test]
fn removing_the_folder_removes_the_row_and_leaves_its_file_to_delete() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let file = record(&mut fx.cat, a, &path)?;
    let folder = fx.cat.folders()?[0].id;
    fx.cat.remove_folders(&[folder])?;
    assert!(rows(&fx.cat)?.is_empty());
    assert_eq!(fx.cat.editable_preview_orphans()?, [file]);
    Ok(())
}

/// Set Copy as Master moves the row in its transaction: the family keeps
/// its copy, and removing the former master, now a virtual copy, keeps it.
#[test]
fn set_copy_as_master_moves_the_row() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let copy = fx.cat.create_virtual_copy(a)?;
    let file = record(&mut fx.cat, a, &path)?;
    let before = fx.cat.editable_preview(a)?.unwrap();
    fx.cat.set_copy_as_master(copy)?;
    assert_eq!(rows(&fx.cat)?, [copy.0]);
    assert_eq!(
        fx.cat.editable_preview(a)?.unwrap(),
        EditablePreview {
            master: copy,
            ..before.clone()
        }
    );
    fx.cat.remove_virtual_copy(a)?;
    let after = fx.cat.editable_preview(copy)?.unwrap();
    assert_eq!(after.file, before.file);
    assert!(fx.cat.editable_preview_orphans()?.is_empty());
    assert!(file.is_file());
    Ok(())
}

/// An older release promotes a virtual copy without knowing the table, so
/// the row stays on what is now a virtual copy. Nothing moves it: the family
/// has no copy, offline its photos can't be edited, and a build (online) makes
/// a new one for the new master. The old row and its file go with the former
/// master.
#[test]
fn a_row_an_older_releases_promotion_left_on_a_virtual_copy_is_not_the_familys() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let copy = fx.cat.create_virtual_copy(a)?;
    let old = record(&mut fx.cat, a, &path)?;
    promote_as_an_older_release(fx.cat.db_for_tests(), copy, a)?;
    let reopened = Catalog::open(fx.path.as_path())?;
    assert_eq!(rows(&reopened)?, [a.0], "left where it was");
    assert_eq!(reopened.editable_preview(a)?, None);
    assert_eq!(reopened.editable_preview(copy)?, None);
    drop(reopened);
    let new = record(&mut fx.cat, copy, &path)?;
    assert_ne!(new, old);
    assert_eq!(fx.cat.editable_preview(a)?.unwrap().master, copy);
    assert!(fx.cat.editable_preview_orphans()?.is_empty());
    // Removing the former master takes the old row; its file is then an
    // orphan, for the app to delete.
    fx.cat.remove_virtual_copy(a)?;
    assert_eq!(rows(&fx.cat)?, [copy.0]);
    assert_eq!(fx.cat.editable_preview_orphans()?, [old]);
    Ok(())
}

/// This release's Set Copy as Master over a family an older release promoted
/// takes the stale row's place: the family's copy follows its master.
#[test]
fn set_copy_as_master_replaces_a_stale_row_on_its_new_master() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let copy = fx.cat.create_virtual_copy(a)?;
    let stale = record(&mut fx.cat, a, &path)?;
    promote_as_an_older_release(fx.cat.db_for_tests(), copy, a)?;
    std::fs::write(&path, b"a.NEF, edited elsewhere")?;
    let current = record(&mut fx.cat, copy, &path)?;
    fx.cat.set_copy_as_master(a)?;
    assert_eq!(rows(&fx.cat)?, [a.0]);
    assert_eq!(
        fx.cat.editable_preview(copy)?.unwrap().file,
        current.file_name().unwrap().to_str().unwrap()
    );
    assert_eq!(fx.cat.editable_preview_orphans()?, [stale]);
    Ok(())
}

/// A release that doesn't know the table removes a photo with SQL alone:
/// the cascade drops the row, and the file is left for the next Build.
#[test]
fn removing_a_photo_with_sql_alone_drops_the_row_through_the_cascade() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let (b, b_path) = fx.photos[1].clone();
    let file = record(&mut fx.cat, a, &path)?;
    let kept = record(&mut fx.cat, b, &b_path)?;
    fx.cat
        .db_for_tests()
        .execute("DELETE FROM photos WHERE id=?", [a.0])?;
    assert_eq!(rows(&fx.cat)?, [b.0]);
    assert_eq!(fx.cat.editable_preview_orphans()?, [file]);
    assert!(kept.is_file());
    Ok(())
}

#[test]
fn discarding_forgets_the_copies_and_hands_back_their_files() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let (b, b_path) = fx.photos[1].clone();
    let copy = fx.cat.create_virtual_copy(a)?;
    record(&mut fx.cat, a, &path)?;
    record(&mut fx.cat, b, &b_path)?;
    // A virtual copy discards its master's; a photo without one is skipped.
    let files = fx
        .cat
        .discard_editable_previews(&[copy, copy, PhotoId(999)])?;
    assert_eq!(files.len(), 1);
    assert!(files[0].starts_with(&format!("{}-", a.0)));
    assert_eq!(rows(&fx.cat)?, [b.0]);
    // Hidden files, a write in progress and folders are never orphans.
    let folder = fx.cat.editable_previews_folder();
    std::fs::write(folder.join(".DS_Store"), b"")?;
    std::fs::write(folder.join("rawmakase-a1b2c3.partial"), b"")?;
    std::fs::create_dir(folder.join("sub"))?;
    assert_eq!(fx.cat.editable_preview_orphans()?, [folder.join(&files[0])]);
    Ok(())
}

/// The copy goes out of date when the Library finds its photo at another file
/// than the one it was built from: by relinking its folder or root, or clearing
/// that location, as well as by Develop opening the original (`note_original`).
/// The same file elsewhere keeps it up to date, as a new modification time
/// alone is no change; a location with no file at it leaves it as it is, since
/// the photo may only be offline. Out of date, it is never opened again.
#[test]
fn the_copy_goes_out_of_date_only_when_the_original_comes_back_as_another_file() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    record(&mut fx.cat, a, &path)?;
    let folder = fx.cat.editable_previews_folder();
    let opens = |p: &Path| -> Result<()> { Ok(std::fs::metadata(p).map(|_| ())?) };
    let out_of_date = |cat: &Catalog| cat.editable_preview(a).unwrap().unwrap().out_of_date;
    let photos_folder = fx.cat.folders()?[0].id;
    let root = fx.cat.roots()?[0].0;
    let dir = |name: &str| -> Result<PathBuf> {
        let dir = fx.dir.join(name);
        std::fs::create_dir(&dir)?;
        Ok(dir)
    };
    // The same file in another folder, relinked.
    let elsewhere = dir("elsewhere")?;
    std::fs::copy(&path, elsewhere.join("a.NEF"))?;
    fx.cat.relink_folder(photos_folder, &elsewhere)?;
    assert_eq!(fx.cat.photos()?[0].path, elsewhere.join("a.NEF"));
    assert!(!out_of_date(&fx.cat));
    // A folder without it: nothing to tell.
    fx.cat.relink_folder(photos_folder, &dir("empty")?)?;
    assert!(!out_of_date(&fx.cat));
    // Back where it was added, the same file.
    fx.cat.clear_folder_location(root, "")?;
    assert_eq!(fx.cat.photos()?[0].path, path);
    assert!(!out_of_date(&fx.cat));
    // Another photo under its name, where the root is relinked.
    let other = dir("other")?;
    std::fs::write(other.join("a.NEF"), b"another photo")?;
    fx.cat.relink_root(root, &other)?;
    assert!(out_of_date(&fx.cat));
    let preview = fx.cat.editable_preview(a)?.unwrap();
    assert_eq!(preview.usable(&folder, opens), None);
    // The first file back doesn't bring it back: Build makes a new one.
    fx.cat.clear_folder_location(root, "")?;
    assert!(out_of_date(&fx.cat));
    let preview = fx.cat.note_original(a, &Identity::read(&path)?)?.unwrap();
    assert!(preview.out_of_date);
    Ok(())
}

/// A relink and the copies it puts out of date commit together: when marking
/// one fails, the relink is refused and nothing changes, so trying again
/// checks the copy again. A stored identity that can't be read is no copy of
/// the file found, and is marked rather than refused.
#[test]
fn a_relink_and_the_copies_it_puts_out_of_date_commit_together() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    record(&mut fx.cat, a, &path)?;
    let root = fx.cat.roots()?[0].0;
    let other = fx.dir.join("other");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("a.NEF"), b"another photo")?;
    fx.cat.db_for_tests().execute_batch(
        "CREATE TRIGGER refuse BEFORE UPDATE ON editable_previews
         BEGIN SELECT RAISE(ABORT, 'refused'); END;",
    )?;
    assert!(fx.cat.relink_root(root, &other).is_err());
    assert_eq!(fx.cat.photos()?[0].path, path, "the relink stood");
    assert!(!fx.cat.editable_preview(a)?.unwrap().out_of_date);
    fx.cat.db_for_tests().execute_batch("DROP TRIGGER refuse")?;
    fx.cat.relink_root(root, &other)?;
    assert_eq!(fx.cat.photos()?[0].path, other.join("a.NEF"));
    assert!(fx.cat.editable_preview(a)?.unwrap().out_of_date);
    // A stored identity that can't be read.
    fx.cat.clear_folder_location(root, "")?;
    fx.cat.record_editable_preview(a, &Identity::read(&path)?)?;
    fx.cat.db_for_tests().execute(
        "UPDATE editable_previews SET original='not an identity' WHERE photo=?",
        [a.0],
    )?;
    fx.cat.relink_root(root, &other)?;
    let out_of_date: bool = fx.cat.db_for_tests().query_row(
        "SELECT out_of_date FROM editable_previews WHERE photo=?",
        [a.0],
        |r| r.get(0),
    )?;
    assert!(out_of_date);
    Ok(())
}

/// The originals a relink will find are read before its transaction, and
/// marked in it only where the change put each one at the path read: a copy
/// whose original the change leaves elsewhere is left as it is.
#[test]
fn a_copy_found_elsewhere_than_read_before_the_transaction_is_left() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    record(&mut fx.cat, a, &path)?;
    let root = fx.cat.roots()?[0].0;
    let computer = fx.cat.computer.id.clone();
    let other = fx.dir.join("other");
    std::fs::create_dir(&other)?;
    std::fs::write(other.join("a.NEF"), b"another photo")?;
    // Read as if the root went to `other`; the transaction changes nothing.
    let found = super::editable_previews::read_relocated(&fx.cat.db, &computer, |rows| {
        rows.insert(root, vec![(String::new(), other.clone())]);
    })?;
    assert_eq!(found[&a].0, other.join("a.NEF"));
    fx.cat
        .db
        .write(|w| super::editable_previews::relocated(w, &computer, &found, |_| Ok(())))?;
    assert!(!fx.cat.editable_preview(a)?.unwrap().out_of_date);
    Ok(())
}

/// No file is read while the catalog's write lock is held: an original that
/// takes its time to open (here a pipe nothing writes to yet, as a network
/// drive spinning up) holds no other write back.
#[cfg(unix)]
#[test]
fn a_relink_reads_no_file_under_the_write_lock() -> Result<()> {
    use std::io::Write as _;
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    record(&mut fx.cat, a, &path)?;
    let root = fx.cat.roots()?[0].0;
    let other = fx.dir.join("other");
    std::fs::create_dir(&other)?;
    let pipe = other.join("a.NEF");
    assert!(
        std::process::Command::new("mkfifo")
            .arg(&pipe)
            .status()?
            .success()
    );
    let mut cat = fx.cat;
    let relink = std::thread::spawn(move || {
        let result = cat.relink_root(root, &other);
        (cat, result)
    });
    // Opening the pipe to write waits for the relink to open it to read. A
    // relink that ends without opening it fails the test with its result, and
    // one that never does fails it after a while, rather than hanging it.
    let (opened, opening) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = opened.send(std::fs::OpenOptions::new().write(true).open(&pipe));
    });
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut writer = loop {
        match opening.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(writer) => break writer?,
            Err(_) if relink.is_finished() => {
                let ended = relink
                    .join()
                    .map(|(_, result)| result.map_err(|e| format!("{e:#}")));
                panic!("the relink ended without opening the original: {ended:?}");
            }
            Err(_) => assert!(
                std::time::Instant::now() < until,
                "the relink never opened the original"
            ),
        }
    };
    // While the relink waits on it, another connection writes at once.
    let mut second = Catalog::open(CatalogLocation::File(fx.path.clone()))?;
    let started = std::time::Instant::now();
    second.set_metadata_of(&[(a, 3, 0, String::new())])?;
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    writer.write_all(b"another photo")?;
    drop(writer);
    let (cat, result) = relink.join().unwrap();
    result?;
    assert!(cat.editable_preview(a)?.unwrap().out_of_date);
    Ok(())
}

/// Clearing a location takes the photo back to where it was added; there,
/// another photo puts the copy out of date too.
#[test]
fn clearing_a_location_to_another_file_puts_the_copy_out_of_date() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let root = fx.cat.roots()?[0].0;
    let elsewhere = fx.dir.join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    std::fs::copy(&path, elsewhere.join("a.NEF"))?;
    fx.cat.relink_root(root, &elsewhere)?;
    record(&mut fx.cat, a, &elsewhere.join("a.NEF"))?;
    std::fs::write(&path, b"another photo")?;
    fx.cat.clear_folder_location(root, "")?;
    assert!(fx.cat.editable_preview(a)?.unwrap().out_of_date);
    Ok(())
}

/// The catalog records files and never decodes them: whatever the reader
/// refuses (a record version it doesn't know, a missing or truncated file)
/// makes the copy out of date, and nothing panics.
#[test]
fn a_copy_the_reader_refuses_is_out_of_date() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let file = record(&mut fx.cat, a, &path)?;
    let folder = fx.cat.editable_previews_folder();
    let preview = fx.cat.editable_preview(a)?.unwrap();
    assert_eq!(preview.usable(&folder, |_| Ok(())), Some(file.clone()));
    for refusal in ["unknown record version 2", "truncated"] {
        assert_eq!(
            preview.usable(&folder, |_| Err(anyhow::anyhow!(refusal))),
            None
        );
    }
    std::fs::remove_file(&file)?;
    assert_eq!(
        preview.usable(&folder, |p| Ok(std::fs::metadata(p).map(|_| ())?)),
        None
    );
    Ok(())
}

#[test]
fn export_as_catalog_carries_the_copies_of_the_photos_it_exports_and_only_those() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF", "c.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let (b, b_path) = fx.photos[1].clone();
    let (c, c_path) = fx.photos[2].clone();
    let a_file = record(&mut fx.cat, a, &path)?;
    let b_file = record(&mut fx.cat, b, &b_path)?;
    let c_file = record(&mut fx.cat, c, &c_path)?;
    // A copy whose file is gone is left out, row and all.
    std::fs::remove_file(&c_file)?;
    let destination = fx.dir.join("out").join("Picked.rawmakase");
    let export = fx
        .cat
        .export_catalog(&ExportScope::Photos(vec![a, c]), &destination)?;
    assert_eq!(export.editable_previews, 1);
    let exported = Catalog::open(destination.as_path())?;
    assert_eq!(rows(&exported)?, [a.0]);
    let carried = exported.editable_previews_folder();
    assert_eq!(
        carried,
        fx.dir.join("out/Picked.rawmakase-editable-previews")
    );
    let names: Vec<_> = std::fs::read_dir(&carried)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    assert_eq!(names, [a_file.file_name().unwrap()]);
    assert_eq!(
        std::fs::read(carried.join(names[0].clone()))?,
        std::fs::read(&a_file)?
    );
    // This catalog is as it was.
    // `rows` is in id order, and the fixture's ids follow the folder's listing.
    let mut all = [a.0, b.0, c.0];
    all.sort();
    assert_eq!(rows(&fx.cat)?, all);
    assert!(b_file.is_file());
    Ok(())
}

#[test]
fn export_as_catalog_never_writes_into_a_copies_folder_already_there() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    record(&mut fx.cat, a, &path)?;
    let destination = fx.dir.join("Picked.rawmakase");
    let there = fx.dir.join("Picked.rawmakase-editable-previews");
    std::fs::create_dir(&there)?;
    std::fs::write(there.join("someone's.dng"), b"left")?;
    assert!(
        fx.cat
            .export_catalog(&ExportScope::Photos(vec![a]), &destination)
            .is_err()
    );
    assert!(!destination.exists());
    assert_eq!(std::fs::read_dir(&there)?.count(), 1);
    Ok(())
}

/// A link in the copies folder named as a copy isn't followed: what it points
/// to stays out of the export, and the copy's row is dropped from it as for a
/// file that isn't there.
#[cfg(unix)]
#[test]
fn export_as_catalog_carries_no_link_named_as_a_copy() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let (a, path) = fx.photos[0].clone();
    let file = record(&mut fx.cat, a, &path)?;
    let secret = fx.dir.join("secret.txt");
    std::fs::write(&secret, b"not a copy")?;
    std::fs::remove_file(&file)?;
    std::os::unix::fs::symlink(&secret, &file)?;
    let destination = fx.dir.join("Picked.rawmakase");
    fx.cat
        .export_catalog(&ExportScope::Photos(vec![a]), &destination)?;
    let carried = fx.dir.join("Picked.rawmakase-editable-previews");
    assert!(
        !carried.exists(),
        "{:?}",
        std::fs::read_dir(&carried).map(|d| d.count())
    );
    let exported = Catalog::open(CatalogLocation::File(destination))?;
    assert!(exported.editable_previews()?.is_empty());
    Ok(())
}

#[test]
fn carrying_nothing_makes_no_folder() -> Result<()> {
    let mut fx = fixture(&["a.NEF"])?;
    let to = fx.dir.join("staged");
    assert_eq!(carry(&fx.dir.join("none"), &mut fx.cat, &to)?, 0);
    assert!(!to.exists());
    Ok(())
}
