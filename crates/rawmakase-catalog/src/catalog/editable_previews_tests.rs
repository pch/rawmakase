//! The catalog's record of editable previews: one row per master, removed with
//! its photo, carried by Export as Catalog, and never pointing at a file it
//! deleted.
use super::*;
use crate::storage::Identity;
use anyhow::Context as _;
use std::path::PathBuf;

/// A catalog `Photos.rawmakase` of folder `photos` holding `a.dng` and
/// `b.dng`, with their ids.
struct Fx {
    _dir: tempfile::TempDir,
    dir: PathBuf,
    cat: Catalog,
    a: PhotoId,
    b: PhotoId,
}
fn fixture() -> Result<Fx> {
    let d = tempfile::tempdir()?;
    let dir = d.path().to_path_buf();
    let folder = dir.join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.dng"), b"synthetic raw a")?;
    std::fs::write(folder.join("b.dng"), b"synthetic raw b")?;
    let mut cat = Catalog::create(&dir.join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let id = |name: &str| -> Result<PhotoId> {
        Ok(cat
            .photos()?
            .into_iter()
            .find(|p| p.filename == name)
            .context("photo")?
            .id)
    };
    let (a, b) = (id("a.dng")?, id("b.dng")?);
    Ok(Fx {
        _dir: d,
        dir,
        cat,
        a,
        b,
    })
}
impl Fx {
    fn original(&self, name: &str) -> Result<Identity> {
        Identity::read(&self.dir.join("photos").join(name))
    }
    /// Writes an editable preview file of `photo` built from `original`, as the
    /// builder does, and describes it as its header would.
    fn build(
        &self,
        photo: PhotoId,
        original: &Identity,
        bytes: &[u8],
    ) -> Result<BuiltEditablePreview> {
        let file = editable_preview_file_name(photo, original);
        let folder = self.cat.editable_preview_folder();
        std::fs::create_dir_all(&folder)?;
        std::fs::write(folder.join(&file), bytes)?;
        Ok(BuiltEditablePreview {
            file,
            format_version: 1,
            original: original.clone(),
            original_size: (6000, 4000),
            size: (2560, 1707),
        })
    }
    /// Where `photo`'s file is, as a build reads it.
    fn source(&self, photo: PhotoId) -> PathBuf {
        let photos = self.cat.photos().unwrap();
        let master = photos.iter().find(|p| p.id == photo).and_then(|p| p.master);
        let id = master.unwrap_or(photo);
        photos
            .into_iter()
            .find(|p| p.id == id)
            .map(|p| p.path)
            .unwrap_or_default()
    }
    fn file(&self, name: &str) -> PathBuf {
        self.cat.editable_preview_folder().join(name)
    }
    fn rows(&self) -> Result<i64> {
        Ok(self.cat.db_for_tests().query_row(
            "SELECT count(*) FROM editable_previews",
            [],
            |r| r.get(0),
        )?)
    }
}

#[test]
fn the_store_is_beside_the_catalog_named_after_it() -> Result<()> {
    let fx = fixture()?;
    assert_eq!(
        fx.cat.editable_preview_folder(),
        fx.dir
            .canonicalize()?
            .join("Photos.rawmakase-editable-previews")
    );
    // Opened through a link, it is the same folder.
    #[cfg(unix)]
    {
        let link = fx.dir.join("link");
        std::fs::create_dir(&link)?;
        std::os::unix::fs::symlink(
            fx.dir.join("Photos.rawmakase"),
            link.join("Photos.rawmakase"),
        )?;
        let linked = Catalog::open(link.join("Photos.rawmakase"))?;
        assert_eq!(
            linked.editable_preview_folder(),
            fx.cat.editable_preview_folder()
        );
    }
    let original = fx.original("a.dng")?;
    assert_eq!(
        editable_preview_file_name(PhotoId(7), &original),
        format!("7-{}.rmep", original.key())
    );
    Ok(())
}

#[test]
fn a_editable_preview_is_recorded_looked_up_listed_and_totalled() -> Result<()> {
    let mut fx = fixture()?;
    assert_eq!(fx.cat.editable_preview(fx.a)?, None);
    let original = fx.original("a.dng")?;
    let built = fx.build(fx.a, &original, &[1; 300])?;
    let recorded = fx
        .cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    let found = fx.cat.editable_preview(fx.a)?.context("recorded")?;
    assert_eq!(found, recorded);
    assert_eq!(found.photo, fx.a);
    assert_eq!(found.path, fx.file(&built.file));
    assert_eq!(found.original, original);
    assert_eq!(found.format_version, 1);
    assert_eq!(found.original_size, (6000, 4000));
    assert_eq!(found.size, (2560, 1707));
    assert_eq!(found.bytes, 300);
    assert_eq!(found.built_at.len(), 19, "{}", found.built_at);
    assert_eq!(fx.cat.editable_preview(fx.b)?, None);
    assert_eq!(
        fx.cat.photos_with_editable_previews()?,
        std::collections::BTreeSet::from([fx.a])
    );
    let b = fx.build(fx.b, &fx.original("b.dng")?, &[2; 200])?;
    fx.cat.record_editable_preview(fx.b, &fx.source(fx.b), &b)?;
    assert_eq!(fx.cat.editable_previews_size()?, 500);
    Ok(())
}

#[test]
fn recording_again_replaces_the_row_and_only_then_deletes_the_old_file() -> Result<()> {
    let mut fx = fixture()?;
    let first = fx.original("a.dng")?;
    let old = fx.build(fx.a, &first, b"old")?;
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &old)?;
    // The original changed and was built again: another name.
    let second = Identity {
        modified_ns: first.modified_ns + 1,
        ..first
    };
    let new = fx.build(fx.a, &second, b"newer")?;
    assert_ne!(old.file, new.file);
    // Built, not recorded yet: the row still names the old file, which stays.
    assert!(fx.file(&old.file).is_file());
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &new)?;
    assert_eq!(fx.rows()?, 1);
    let found = fx.cat.editable_preview(fx.a)?.context("recorded")?;
    assert_eq!(
        (found.path, found.original, found.bytes),
        (fx.file(&new.file), second, 5)
    );
    assert!(!fx.file(&old.file).exists(), "the old file is deleted");
    assert!(fx.file(&new.file).is_file());
    // Built again from the same original: the same name, which stays.
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &new)?;
    assert!(fx.file(&new.file).is_file());
    Ok(())
}

#[test]
fn a_failed_record_leaves_the_row_and_files_as_they_were() -> Result<()> {
    let mut fx = fixture()?;
    let original = fx.original("a.dng")?;
    let old = fx.build(fx.a, &original, b"old")?;
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &old)?;
    let new = fx.build(
        fx.a,
        &Identity {
            size: original.size + 1,
            ..original
        },
        b"new",
    )?;
    fx.cat.db_for_tests().execute_batch(
        "CREATE TEMP TRIGGER fail BEFORE INSERT ON editable_previews
         BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
    )?;
    assert!(
        fx.cat
            .record_editable_preview(fx.a, &fx.source(fx.a), &new)
            .is_err()
    );
    let found = fx.cat.editable_preview(fx.a)?.context("kept")?;
    assert_eq!(found.path, fx.file(&old.file));
    assert!(fx.file(&old.file).is_file(), "the old file stays");
    assert!(
        fx.file(&new.file).is_file(),
        "the new file is the builder's"
    );
    Ok(())
}

#[test]
fn a_record_is_refused_for_a_missing_file_a_bad_name_or_a_photo_gone() -> Result<()> {
    let mut fx = fixture()?;
    let original = fx.original("a.dng")?;
    let mut built = fx.build(fx.a, &original, b"x")?;
    for bad in [
        "",
        ".",
        "..",
        "../1-a.rmep",
        "sub/1-a.rmep",
        "1-a.jpg",
        ".rmep",
        "1\\..\\..\\x.rmep",
        "C:1-a.rmep",
        "1-A.rmep",
    ] {
        built.file = bad.into();
        assert!(
            fx.cat
                .record_editable_preview(fx.a, &fx.source(fx.a), &built)
                .is_err(),
            "{bad}"
        );
    }
    // A name of its own, but never written.
    built.file = editable_preview_file_name(
        fx.a,
        &Identity {
            size: original.size + 99,
            ..original
        },
    );
    assert!(
        fx.cat
            .record_editable_preview(fx.a, &fx.source(fx.a), &built)
            .is_err()
    );
    // Renamed while it was built, or its id given to another photo.
    let built = fx.build(fx.b, &fx.original("b.dng")?, b"x")?;
    let source = fx.source(fx.b);
    fx.cat.db_for_tests().execute(
        "UPDATE photos SET filename='other.dng' WHERE id=?",
        [fx.b.0],
    )?;
    assert!(
        fx.cat
            .record_editable_preview(fx.b, &source, &built)
            .is_err()
    );
    fx.cat
        .db_for_tests()
        .execute("UPDATE photos SET filename='b.dng' WHERE id=?", [fx.b.0])?;
    // Built from another path than the photo's.
    assert!(
        fx.cat
            .record_editable_preview(fx.b, &fx.dir.join("elsewhere/b.dng"), &built)
            .is_err()
    );
    // Removed while it was built.
    fx.cat.remove_folders(&[fx.cat.folders()?[0].id])?;
    assert!(
        fx.cat
            .record_editable_preview(fx.b, &source, &built)
            .is_err()
    );
    assert_eq!(fx.rows()?, 0, "no row names a file it should not");
    Ok(())
}

#[test]
fn virtual_copies_share_their_masters() -> Result<()> {
    let mut fx = fixture()?;
    let copy = fx.cat.create_virtual_copy(fx.a)?;
    let built = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
    // Recorded through the copy, it is the master's.
    fx.cat
        .record_editable_preview(copy, &fx.source(copy), &built)?;
    let found = fx.cat.editable_preview(copy)?.context("the master's")?;
    assert_eq!(found.photo, fx.a);
    assert_eq!(fx.cat.editable_preview(fx.a)?, Some(found));
    assert_eq!(
        fx.cat.photos_with_editable_previews()?,
        std::collections::BTreeSet::from([fx.a, copy])
    );
    // Removing the copy keeps it.
    fx.cat.remove_virtual_copy(copy)?;
    assert!(fx.cat.editable_preview(fx.a)?.is_some());
    assert!(fx.file(&built.file).is_file());
    Ok(())
}

#[test]
fn set_copy_as_master_takes_the_editable_preview_over() -> Result<()> {
    let mut fx = fixture()?;
    let built = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    let copy = fx.cat.create_virtual_copy(fx.a)?;
    fx.cat.set_copy_as_master(copy)?;
    let found = fx.cat.editable_preview(fx.a)?.context("still shared")?;
    assert_eq!(found.photo, copy);
    assert_eq!(found.path, fx.file(&built.file));
    assert_eq!(fx.rows()?, 1);
    Ok(())
}

#[test]
fn a_copy_an_older_release_left_holding_it_passes_it_to_its_master() -> Result<()> {
    let mut fx = fixture()?;
    let built = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    let copy = fx.cat.create_virtual_copy(fx.a)?;
    // Set Copy as Master as a release before editable previews makes it: the
    // row stays with the photo that is now a copy.
    fx.cat.db_for_tests().execute_batch(&format!(
        "UPDATE photos SET master_id={copy} WHERE id={a};
         UPDATE photos SET master_id=NULL, copy_name='' WHERE id={copy};",
        copy = copy.0,
        a = fx.a.0
    ))?;
    assert_eq!(
        fx.cat.editable_preview(copy)?.map(|s| s.path),
        Some(fx.file(&built.file))
    );
    // Removing the old master, now a copy, keeps the family's.
    fx.cat.remove_virtual_copy(fx.a)?;
    let kept = fx.cat.editable_preview(copy)?.context("kept")?;
    assert_eq!(kept.photo, copy);
    assert!(fx.file(&built.file).is_file());
    Ok(())
}

#[test]
fn removing_a_family_an_older_release_rearranged_removes_its_editable_preview() -> Result<()> {
    // Removing a folder deletes each photo of it, copies first or masters
    // first: either way the row goes, and its file after the commit.
    for copies_first in [false, true] {
        let mut fx = fixture()?;
        let built = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
        fx.cat
            .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
        let copy = fx.cat.create_virtual_copy(fx.a)?;
        fx.cat.db_for_tests().execute_batch(&format!(
            "UPDATE photos SET master_id={copy} WHERE id={a};
             UPDATE photos SET master_id=NULL, copy_name='' WHERE id={copy};",
            copy = copy.0,
            a = fx.a.0
        ))?;
        let order = if copies_first {
            [fx.a, copy]
        } else {
            [copy, fx.a]
        };
        fx.cat.db.write(|w| {
            for photo in order {
                super::copies::delete_photo(w, photo)?;
            }
            Ok(())
        })?;
        assert_eq!(fx.rows()?, 0, "copies first: {copies_first}");
        assert!(!fx.file(&built.file).exists());
    }
    Ok(())
}

#[test]
fn discarding_removes_the_rows_and_files_of_a_selection() -> Result<()> {
    let mut fx = fixture()?;
    let a = fx.build(fx.a, &fx.original("a.dng")?, b"a")?;
    let b = fx.build(fx.b, &fx.original("b.dng")?, b"b")?;
    fx.cat.record_editable_preview(fx.a, &fx.source(fx.a), &a)?;
    fx.cat.record_editable_preview(fx.b, &fx.source(fx.b), &b)?;
    let copy = fx.cat.create_virtual_copy(fx.a)?;
    // A copy chosen discards its master's; one without any is left out.
    assert_eq!(fx.cat.discard_editable_previews(&[copy, copy])?, [fx.a]);
    assert_eq!(fx.cat.editable_preview(fx.a)?, None);
    assert!(!fx.file(&a.file).exists());
    assert!(fx.cat.editable_preview(fx.b)?.is_some());
    assert!(fx.file(&b.file).is_file());
    assert_eq!(
        fx.cat.discard_editable_previews(&[fx.a, PhotoId(9999)])?,
        []
    );
    Ok(())
}

#[test]
fn a_failed_discard_keeps_rows_and_files() -> Result<()> {
    let mut fx = fixture()?;
    let a = fx.build(fx.a, &fx.original("a.dng")?, b"a")?;
    fx.cat.record_editable_preview(fx.a, &fx.source(fx.a), &a)?;
    fx.cat.db_for_tests().execute_batch(
        "CREATE TEMP TRIGGER fail BEFORE DELETE ON editable_previews
         BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
    )?;
    assert!(fx.cat.discard_editable_previews(&[fx.a]).is_err());
    assert!(fx.cat.editable_preview(fx.a)?.is_some());
    assert!(fx.file(&a.file).is_file(), "the row still names it");
    Ok(())
}

#[test]
fn removing_folders_removes_their_photos_editable_previews_after_commit() -> Result<()> {
    let mut fx = fixture()?;
    let a = fx.build(fx.a, &fx.original("a.dng")?, b"a")?;
    fx.cat.record_editable_preview(fx.a, &fx.source(fx.a), &a)?;
    let folder = fx.cat.folders()?[0].id;
    // A failure removing the photos removes nothing.
    fx.cat.db_for_tests().execute_batch(
        "CREATE TEMP TRIGGER fail BEFORE DELETE ON folders
         BEGIN SELECT RAISE(FAIL, 'disk full'); END;",
    )?;
    assert!(fx.cat.remove_folders(&[folder]).is_err());
    assert!(fx.cat.editable_preview(fx.a)?.is_some());
    assert!(fx.file(&a.file).is_file());
    fx.cat
        .db_for_tests()
        .execute_batch("DROP TRIGGER temp.fail")?;
    fx.cat.remove_folders(&[folder])?;
    assert_eq!(fx.rows()?, 0);
    assert!(!fx.file(&a.file).exists());
    // The original is a user's file: untouched.
    assert!(fx.dir.join("photos/a.dng").is_file());
    Ok(())
}

#[test]
fn a_changed_original_makes_a_editable_preview_stale() -> Result<()> {
    let mut fx = fixture()?;
    let original = fx.original("a.dng")?;
    let built = fx.build(fx.a, &original, b"x")?;
    let recorded = fx
        .cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    assert!(!recorded.is_stale(&original));
    // Touched, or copied without its time: the same photo.
    let touched = Identity {
        modified_ns: original.modified_ns + 5,
        ..original
    };
    assert!(!recorded.is_stale(&touched));
    // Replaced by another file.
    std::fs::write(fx.dir.join("photos/a.dng"), b"another photo entirely")?;
    assert!(recorded.is_stale(&fx.original("a.dng")?));
    // Nothing is deleted by the check.
    assert!(fx.cat.editable_preview(fx.a)?.is_some());
    assert!(recorded.path.is_file());
    Ok(())
}

#[test]
fn orphans_are_files_no_row_names() -> Result<()> {
    let mut fx = fixture()?;
    let a = fx.build(fx.a, &fx.original("a.dng")?, b"a")?;
    fx.cat.record_editable_preview(fx.a, &fx.source(fx.a), &a)?;
    assert!(fx.cat.editable_preview_orphans()?.is_empty());
    let stray = fx.build(fx.b, &fx.original("b.dng")?, b"b")?;
    std::fs::write(fx.file("partial.tmp"), b"a build in progress")?;
    assert_eq!(fx.cat.editable_preview_orphans()?, [fx.file(&stray.file)]);
    Ok(())
}

#[test]
fn a_catalog_reopens_with_its_editable_previews_and_an_older_one_gains_the_table() -> Result<()> {
    let mut fx = fixture()?;
    let built = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
    let recorded = fx
        .cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    let path = fx.dir.join("Photos.rawmakase");
    let reopened = Catalog::open(&path)?;
    assert_eq!(reopened.editable_preview(fx.a)?, Some(recorded));
    drop(reopened);
    // A catalog from a release before the table: it opens, gains it, and
    // keeps its format version, so that release still opens it.
    let version = || -> Result<i64> {
        let db = rusqlite::Connection::open(&path)?;
        Ok(db.query_row("PRAGMA user_version", [], |r| r.get(0))?)
    };
    let before = version()?;
    rusqlite::Connection::open(&path)?.execute_batch("DROP TABLE editable_previews")?;
    let mut older = Catalog::open(&path)?;
    assert_eq!(older.editable_preview(fx.a)?, None);
    assert_eq!(older.editable_previews_size()?, 0);
    let again = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
    older.record_editable_preview(fx.a, &fx.source(fx.a), &again)?;
    assert_eq!(version()?, before);
    Ok(())
}

// Symlinks need privileges on Windows; the comparison is the same there.
#[cfg(unix)]
#[test]
fn a_build_read_through_a_link_to_the_photos_file_is_recorded() -> Result<()> {
    // The catalog keeps a folder by its real path; a build may have been
    // given another spelling of it, as macOS's /var is /private/var.
    let mut fx = fixture()?;
    let built = fx.build(fx.a, &fx.original("a.dng")?, b"x")?;
    let link = fx.dir.join("linked");
    std::os::unix::fs::symlink(fx.dir.join("photos"), &link)?;
    fx.cat
        .record_editable_preview(fx.a, &link.join("a.dng"), &built)?;
    let recorded = fx.cat.editable_preview(fx.a)?;
    // A link to another photo's file is not this photo's.
    let error = fx
        .cat
        .record_editable_preview(fx.a, &link.join("b.dng"), &built)
        .unwrap_err();
    assert!(format!("{error:#}").contains("changed while"), "{error:#}");
    assert_eq!(fx.cat.editable_preview(fx.a)?, recorded);
    assert_eq!(fx.rows()?, 1);
    Ok(())
}

/// A build the catalog refused (its photo left, moved or was replaced) removes
/// its file through the catalog, which deletes it only while no row names it.
#[test]
fn an_unrecorded_build_is_removed_but_never_a_file_a_row_names() -> Result<()> {
    let mut fx = fixture()?;
    let original = fx.original("a.dng")?;
    let built = fx.build(fx.a, &original, b"recorded")?;
    fx.cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    assert!(!fx.cat.discard_unrecorded_editable_preview(&built.file)?);
    assert!(fx.file(&built.file).is_file());
    assert!(fx.cat.editable_preview(fx.a)?.is_some());
    let stray = fx.build(fx.b, &fx.original("b.dng")?, b"refused")?;
    assert!(fx.cat.discard_unrecorded_editable_preview(&stray.file)?);
    assert!(!fx.file(&stray.file).exists());
    // Nothing there, and a name that isn't an editable preview's.
    assert!(!fx.cat.discard_unrecorded_editable_preview(&stray.file)?);
    assert!(
        fx.cat
            .discard_unrecorded_editable_preview("../Photos.rawmakase")
            .is_err()
    );
    assert!(fx.dir.join("Photos.rawmakase").is_file());
    Ok(())
}

/// An edit with `exposure`, for the offline saves below.
fn exposure(exposure: f32) -> crate::model::recipe::Recipe {
    crate::model::recipe::Recipe {
        exposure,
        ..Default::default()
    }
}
impl Fx {
    /// Saves `photo`'s edit as Develop saves one opened from its editable
    /// preview, built from `recorded`, while its original at `path` may be away.
    fn save_offline(
        &mut self,
        photo: PhotoId,
        path: &std::path::Path,
        recorded: &Identity,
        edit: f32,
    ) -> Result<()> {
        self.cat.save_edit(
            photo,
            crate::edits::Original::recorded(path, recorded),
            recorded,
            &exposure(edit),
            &crate::export_settings::ExportOptions::default(),
            HistoryUpdate::Keep,
        )
    }
}

#[test]
fn an_offline_save_is_checked_against_the_recorded_identity_and_loads_back_online() -> Result<()> {
    let mut fx = fixture()?;
    let path = fx.source(fx.a);
    let original = fx.original("a.dng")?;
    let kept = std::fs::read(&path)?;
    std::fs::remove_file(&path)?;
    // Without anything standing in, the missing file can't be checked.
    let refused = fx.cat.save_edit(
        fx.a,
        &path,
        &original,
        &exposure(0.4),
        &Default::default(),
        HistoryUpdate::Keep,
    );
    assert!(refused.is_err());
    fx.save_offline(fx.a, &path, &original, 0.4)?;
    // Stored as an ordinary identity: the recorded one.
    let stored: Identity =
        serde_json::from_str(&fx.cat.edit_record(fx.a)?.identity.context("an identity")?)?;
    assert_eq!(stored, original);
    // Offline, it loads against the recorded identity, not the missing file.
    assert!(fx.cat.load_edit(fx.a, &path).is_err());
    let back = fx
        .cat
        .load_edit(fx.a, crate::edits::Original::recorded(&path, &original))?
        .context("saved")?;
    assert_eq!(back.recipe.exposure, 0.4);
    // The same file back: the edit loads as any photo's does.
    std::fs::write(&path, &kept)?;
    assert_eq!(
        fx.cat
            .load_edit(fx.a, &path)?
            .context("saved")?
            .recipe
            .exposure,
        0.4
    );
    Ok(())
}

#[test]
fn a_replaced_original_protects_an_edit_saved_offline() -> Result<()> {
    let mut fx = fixture()?;
    let path = fx.source(fx.a);
    let original = fx.original("a.dng")?;
    std::fs::remove_file(&path)?;
    fx.save_offline(fx.a, &path, &original, 0.4)?;
    std::fs::write(&path, b"another photo entirely")?;
    assert!(fx.cat.load_edit(fx.a, &path).is_err());
    // A file that is there is read, even for a photo still open from its
    // editable preview: the replaced one gets none of the edit.
    assert!(fx.save_offline(fx.a, &path, &original, 0.7).is_err());
    let stored: Identity =
        serde_json::from_str(&fx.cat.edit_record(fx.a)?.identity.context("an identity")?)?;
    assert_eq!(stored, original);
    Ok(())
}

#[test]
fn an_offline_save_from_a_preview_of_another_version_is_refused() -> Result<()> {
    let mut fx = fixture()?;
    let path = fx.source(fx.a);
    let original = fx.original("a.dng")?;
    // Edited online, then the file replaced and edited again, then taken offline:
    // a preview still of the first version is not of the edit's photo.
    std::fs::write(&path, b"a later version of a")?;
    let later = fx.original("a.dng")?;
    fx.cat.save_edit(
        fx.a,
        &path,
        &later,
        &exposure(0.2),
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    std::fs::remove_file(&path)?;
    let error = fx
        .save_offline(fx.a, &path, &original, 0.4)
        .expect_err("refused");
    assert!(format!("{error:#}").contains("changed"), "{error:#}");
    assert_eq!(
        fx.cat
            .load_edit(fx.a, crate::edits::Original::recorded(&path, &later))?
            .context("saved")?
            .recipe
            .exposure,
        0.2
    );
    Ok(())
}

#[test]
fn an_unreadable_original_that_is_there_is_never_stood_in_for() -> Result<()> {
    let fx = fixture()?;
    // A folder where the file should be: there, but not a readable file.
    let path = fx.dir.join("photos/folder.dng");
    std::fs::create_dir(&path)?;
    let recorded = fx.original("a.dng")?;
    let original = crate::edits::Original::recorded(&path, &recorded);
    assert!(original.identity().is_err());
    Ok(())
}

#[test]
fn an_offline_preview_is_usable_missing_or_stale() -> Result<()> {
    let mut fx = fixture()?;
    assert_eq!(fx.cat.offline_editable_preview(fx.a)?, OfflinePreview::None);
    let original = fx.original("a.dng")?;
    let built = fx.build(fx.a, &original, b"x")?;
    let recorded = fx
        .cat
        .record_editable_preview(fx.a, &fx.source(fx.a), &built)?;
    assert_eq!(
        fx.cat.offline_editable_preview(fx.a)?,
        OfflinePreview::Usable(recorded.clone())
    );
    // A virtual copy uses its master's.
    let copy = fx.cat.create_virtual_copy(fx.a)?;
    assert!(matches!(
        fx.cat.offline_editable_preview(copy)?,
        OfflinePreview::Usable(_)
    ));
    // An edit saved for another version of the photo makes it stale.
    let path = fx.source(fx.a);
    std::fs::write(&path, b"a later version of a")?;
    let later = fx.original("a.dng")?;
    fx.cat.save_edit(
        fx.a,
        &path,
        &later,
        &exposure(0.2),
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    assert_eq!(
        fx.cat.offline_editable_preview(fx.a)?,
        OfflinePreview::Stale(recorded.clone())
    );
    // Its copy has no edit of its own yet: its preview is still usable for it.
    assert!(matches!(
        fx.cat.offline_editable_preview(copy)?,
        OfflinePreview::Usable(_)
    ));
    // Deleted outside RAWmakase.
    std::fs::remove_file(&recorded.path)?;
    assert_eq!(
        fx.cat.offline_editable_preview(copy)?,
        OfflinePreview::Missing(recorded)
    );
    Ok(())
}

impl Fx {
    /// Clears `photo`'s edit, meant for the file `source` names, against
    /// `original`, as Sync's Undo clears a photo that had no edit.
    fn clear(
        &mut self,
        photo: PhotoId,
        original: crate::edits::Original<'_>,
        source: &Identity,
    ) -> Result<()> {
        self.cat.change_edits(&[EditChange::Clear(EditToClear {
            id: photo,
            original,
            source,
        })])
    }
    fn has_recipe(&self, photo: PhotoId) -> Result<bool> {
        Ok(self.cat.edit_record(photo)?.recipe.is_some())
    }
}

#[test]
fn a_clear_is_checked_against_the_file_as_a_save_is() -> Result<()> {
    let mut fx = fixture()?;
    let path = fx.source(fx.a);
    let original = fx.original("a.dng")?;
    fx.cat.save_edit(
        fx.a,
        &path,
        &original,
        &exposure(0.4),
        &Default::default(),
        HistoryUpdate::Keep,
    )?;
    // Another photo in its place: its edit is protected, never cleared.
    let kept = std::fs::read(&path)?;
    std::fs::write(&path, b"another photo entirely")?;
    let error = fx
        .clear(fx.a, (&path).into(), &original)
        .expect_err("refused");
    assert!(format!("{error:#}").contains("not cleared"), "{error:#}");
    assert!(fx.has_recipe(fx.a)?);
    // Meant for that other photo, it is still refused: the stored edit isn't its.
    let other = fx.original("a.dng")?;
    assert!(fx.clear(fx.a, (&path).into(), &other).is_err());
    assert!(fx.has_recipe(fx.a)?);
    // Offline, with nothing standing in, the missing file can't be checked.
    std::fs::remove_file(&path)?;
    assert!(fx.clear(fx.a, (&path).into(), &original).is_err());
    assert!(fx.has_recipe(fx.a)?);
    // The same file back: cleared.
    std::fs::write(&path, &kept)?;
    fx.clear(fx.a, (&path).into(), &original)?;
    assert!(!fx.has_recipe(fx.a)?);
    Ok(())
}

#[test]
fn an_offline_clear_is_checked_against_the_recorded_identity() -> Result<()> {
    let mut fx = fixture()?;
    let path = fx.source(fx.a);
    let original = fx.original("a.dng")?;
    std::fs::remove_file(&path)?;
    fx.save_offline(fx.a, &path, &original, 0.4)?;
    // A preview of another version of the photo stands in for none of its edit.
    std::fs::write(&path, b"a later version of a")?;
    let later = fx.original("a.dng")?;
    std::fs::remove_file(&path)?;
    let recorded = |identity| crate::edits::Original::recorded(&path, identity);
    assert!(fx.clear(fx.a, recorded(&later), &later).is_err());
    assert!(fx.clear(fx.a, recorded(&later), &original).is_err());
    assert!(fx.has_recipe(fx.a)?);
    fx.clear(fx.a, recorded(&original), &original)?;
    assert!(!fx.has_recipe(fx.a)?);
    Ok(())
}

#[test]
fn a_change_with_one_refused_clear_writes_nothing() -> Result<()> {
    let mut fx = fixture()?;
    let (a, b) = (fx.source(fx.a), fx.source(fx.b));
    let (was_a, was_b) = (fx.original("a.dng")?, fx.original("b.dng")?);
    for (id, path, identity) in [(fx.a, &a, &was_a), (fx.b, &b, &was_b)] {
        fx.cat.save_edit(
            id,
            path,
            identity,
            &exposure(0.4),
            &Default::default(),
            HistoryUpdate::Keep,
        )?;
    }
    std::fs::write(&b, b"another photo entirely")?;
    let save = EditToSave {
        id: fx.a,
        original: (&a).into(),
        source: &was_a,
        recipe: &exposure(0.9),
        export: &Default::default(),
        history: HistoryUpdate::Keep,
    };
    let refused = fx.cat.change_edits(&[
        EditChange::Save(&save),
        EditChange::Clear(EditToClear {
            id: fx.b,
            original: (&b).into(),
            source: &was_b,
        }),
    ]);
    assert!(refused.is_err());
    let exposure_a = fx
        .cat
        .load_edit(fx.a, &a)?
        .context("saved")?
        .recipe
        .exposure;
    assert_eq!(exposure_a, 0.4);
    assert!(fx.has_recipe(fx.b)?);
    Ok(())
}
