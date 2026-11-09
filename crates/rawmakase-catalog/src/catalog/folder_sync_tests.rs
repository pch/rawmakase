use super::digest::{
    BACKGROUND_TABLES, PHOTO_TABLES, USER_COLUMNS, every_family, family, placement, user_state,
};
use crate::catalog::Catalog;
use anyhow::Result;

#[test]
fn family_tables_cover_the_schema() {
    // Every table with a `photo` column or a reference to photos.
    let schema = include_str!("schema.sql");
    let mut tables = Vec::new();
    let mut current = None;
    for line in schema.lines().map(str::trim) {
        if let Some(name) = line.strip_prefix("CREATE TABLE IF NOT EXISTS ") {
            current = name.split_whitespace().next().map(str::to_string);
        } else if line.starts_with(')') {
            current = None;
        } else if let Some(table) = &current
            && (line.starts_with("photo ") || line.contains("REFERENCES photos"))
            && table != "photos"
            && !tables.contains(table)
        {
            tables.push(table.clone());
        }
    }
    let mut listed: Vec<String> = PHOTO_TABLES.iter().map(|(t, ..)| t.to_string()).collect();
    tables.sort();
    listed.sort();
    assert_eq!(listed, tables);
}

#[test]
fn a_familys_digest_changes_with_any_of_its_rows() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.dng"), b"synthetic raw a")?;
    std::fs::write(folder.join("b.dng"), b"synthetic raw b")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let photos = cat.photos()?;
    let (a, b) = (photos[0].id, photos[1].id);
    let digest = |cat: &Catalog, id| family(&cat.db, id).unwrap().unwrap();
    let before = digest(&cat, a);
    assert_eq!(digest(&cat, a), before);
    assert_ne!(digest(&cat, b), before);
    // Another photo's change is not this family's.
    cat.add_keywords(&[b], &[vec!["Elsewhere".into()]])?;
    assert_eq!(digest(&cat, a), before);
    // Its own rows, a copy gained, and the copy's rows all are.
    cat.add_keywords(&[a], &[vec!["Trip".into()]])?;
    let tagged = digest(&cat, a);
    assert_ne!(tagged, before);
    let copy = cat.create_virtual_copy(a)?;
    let copied = digest(&cat, a);
    assert_ne!(copied, tagged);
    cat.set_copy_name(copy, "Renamed")?;
    assert_ne!(digest(&cat, a), copied);
    assert!(family(&cat.db, crate::catalog::PhotoId(9999))?.is_none());
    // Read a table at a time, every family's is the same.
    let every = every_family(&cat.db)?;
    assert_eq!(every.len(), 2);
    for id in [a, b] {
        assert_eq!(every[&id], digest(&cat, id));
    }
    assert!(!every.contains_key(&copy));
    Ok(())
}

#[test]
fn user_state_covers_the_photo_columns() {
    // Every `photos` column but where the photo is and its sort key.
    let schema = include_str!("schema.sql");
    let table = schema
        .split("CREATE TABLE IF NOT EXISTS photos (")
        .nth(1)
        .and_then(|rest| rest.split(");").next())
        .unwrap();
    let mut columns: Vec<&str> = table
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .collect();
    columns.retain(|c| !["folder", "filename", "captured"].contains(c));
    let mut covered = USER_COLUMNS.to_vec();
    columns.sort();
    covered.sort();
    assert_eq!(covered, columns);
    // And the query reads exactly those.
    let source = include_str!("folder_sync/digest.rs");
    let query = source
        .split("\"SELECT id, original_path")
        .nth(1)
        .and_then(|rest| rest.split("FROM photos").next())
        .unwrap();
    let mut read: Vec<&str> = ["id", "original_path"]
        .into_iter()
        .chain(query.split(',').map(str::trim).filter(|c| !c.is_empty()))
        .collect();
    read.sort();
    assert_eq!(read, covered);
    for table in BACKGROUND_TABLES {
        assert!(PHOTO_TABLES.iter().any(|(t, ..)| *t == table));
    }
}

#[test]
fn placement_and_user_state_change_apart() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let folder = dir.path().join("photos");
    std::fs::create_dir(&folder)?;
    std::fs::write(folder.join("a.dng"), b"synthetic raw a")?;
    let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
    cat.add_folder(&folder)?;
    let a = cat.photos()?[0].id;
    let place = |cat: &Catalog| placement(&cat.db, a).unwrap().unwrap();
    let user = |cat: &Catalog| user_state(&cat.db, a).unwrap().unwrap();
    let (p0, u0) = (place(&cat), user(&cat));
    // Background reads change neither.
    cat.fill_capture_times(&[(a, "2024-05-01T12:00:00".into())])?;
    cat.fill_photo_info(&[(a, None)])?;
    assert_eq!((place(&cat), user(&cat)), (p0, u0));
    // What the user does changes the user state, not the placement.
    cat.set_metadata(a, 3, 0, "")?;
    let u1 = user(&cat);
    assert_ne!(u1, u0);
    assert_eq!(place(&cat), p0);
    // A copy changes both: it is a photo of the family somewhere.
    cat.create_virtual_copy(a)?;
    let (p2, u2) = (place(&cat), user(&cat));
    assert_ne!(p2, p0);
    assert_ne!(u2, u1);
    // Where the family is changes the placement only.
    cat.db_for_tests().execute(
        "UPDATE photos SET filename='b.dng' WHERE folder IS NOT NULL",
        [],
    )?;
    assert_ne!(place(&cat), p2);
    assert_eq!(user(&cat), u2);
    assert!(placement(&cat.db, crate::catalog::PhotoId(9999))?.is_none());
    assert!(user_state(&cat.db, crate::catalog::PhotoId(9999))?.is_none());
    Ok(())
}

mod scenarios {
    use super::super::{Accepted, Applied, SyncPlan, SyncScope, scan};
    use crate::catalog::{Catalog, FolderId, PhotoId, ReadChecksum};
    use crate::storage::{ChecksumRead, FileChecksum};
    use crate::{export_settings::ExportOptions, model::recipe::Recipe};
    use anyhow::Result;
    use rawmakase_cancel::Cancel;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    /// A catalog of one root, "trip", with folders "Lisbon" and "Porto".
    struct Fixture {
        _dir: tempfile::TempDir,
        root: PathBuf,
        cat: Catalog,
    }
    /// Distinct synthetic bytes, larger than the quick hash's three blocks.
    fn bytes(seed: u8) -> Vec<u8> {
        (0..300_000u32)
            .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
            .collect()
    }
    fn fixture() -> Result<Fixture> {
        let dir = tempfile::tempdir()?;
        let root = dir.path().canonicalize()?.join("trip");
        std::fs::create_dir_all(root.join("Lisbon"))?;
        std::fs::create_dir_all(root.join("Porto"))?;
        std::fs::write(root.join("Lisbon/IMG_1001.dng"), bytes(1))?;
        std::fs::write(root.join("Lisbon/IMG_1002.dng"), bytes(2))?;
        std::fs::write(root.join("Porto/IMG_2001.dng"), bytes(3))?;
        let mut cat = Catalog::create(&dir.path().join("Photos.rawmakase"))?;
        cat.add_folder(&root)?;
        let mut f = Fixture {
            _dir: dir,
            root,
            cat,
        };
        f.checksum_all()?;
        Ok(f)
    }
    impl Fixture {
        fn checksum_all(&mut self) -> Result<()> {
            let read: Vec<ReadChecksum> = self
                .cat
                .photos()?
                .into_iter()
                .filter(|p| p.master.is_none())
                .filter_map(|p| match FileChecksum::read_full(&p.path, |_| true).ok()? {
                    ChecksumRead::Done(checksum) => Some(ReadChecksum {
                        photo: p.id,
                        folder: p.folder,
                        filename: p.filename.clone(),
                        path: p.path.clone(),
                        checksum,
                    }),
                    _ => None,
                })
                .collect();
            self.cat.save_checksums(&read)?;
            Ok(())
        }
        fn id(&self, name: &str) -> PhotoId {
            self.cat
                .photos()
                .unwrap()
                .into_iter()
                .find(|p| p.filename == name && p.master.is_none())
                .unwrap()
                .id
        }
        fn folder(&self, logical: &str) -> FolderId {
            self.cat
                .folders()
                .unwrap()
                .into_iter()
                .find(|f| f.relative == logical)
                .unwrap()
                .id
        }
        fn path_of(&self, id: PhotoId) -> PathBuf {
            self.cat
                .photos()
                .unwrap()
                .into_iter()
                .find(|p| p.id == id)
                .unwrap()
                .path
        }
        fn plan(&self, scope: SyncScope) -> Result<SyncPlan> {
            self.plan_with(scope, &|_| true)
        }
        fn plan_with(&self, scope: SyncScope, online: &dyn Fn(&Path) -> bool) -> Result<SyncPlan> {
            let inputs = self.cat.sync_inputs(scope)?;
            scan(&inputs, online, Cancel::NEVER, &|_| {})
        }
        fn apply(&mut self, plan: &SyncPlan, accepted: &Accepted) -> Result<Applied> {
            self.cat
                .apply_folder_sync(plan, accepted, &|_| true, None, &Default::default())
        }
        fn edit(&mut self, id: PhotoId) -> Result<Recipe> {
            let recipe = Recipe {
                exposure: 0.5,
                ..Default::default()
            };
            let path = self.path_of(id);
            self.cat.save_edit(
                id,
                &path,
                &crate::storage::Identity::read(&path)?,
                &recipe,
                &ExportOptions::default(),
                crate::catalog::HistoryUpdate::Keep,
            )?;
            Ok(recipe)
        }
    }
    /// Moves a file as copying to another volume does: new file, new time.
    fn copy_then_delete(from: &Path, to: &Path) -> Result<()> {
        std::fs::create_dir_all(to.parent().unwrap())?;
        std::fs::copy(from, to)?;
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(to)?
            .set_modified(later)?;
        std::fs::remove_file(from)?;
        Ok(())
    }

    #[test]
    fn synchronized_photos_keep_the_name_and_size_they_were_added_with() -> Result<()> {
        let mut f = fixture()?;
        let new = f.root.join("Lisbon/IMG_2000.dng");
        std::fs::write(&new, b"synthetic raw, synchronized")?;
        let plan = f.plan(SyncScope::All)?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.added, 1);
        let id = f.id("IMG_2000.dng");
        assert_eq!(
            f.cat.import_facts(id)?,
            Some(("IMG_2000.dng".into(), None, Some(27), false))
        );
        // Renamed outside RAWmakase and found again: still the name it had.
        let old = f.id("IMG_1001.dng");
        let to = f.root.join("Lisbon/keep-01.dng");
        copy_then_delete(&f.root.join("Lisbon/IMG_1001.dng"), &to)?;
        let plan = f.plan(SyncScope::All)?;
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.path_of(old), to);
        assert_eq!(f.cat.import_facts(old)?.unwrap().0, "IMG_1001.dng");
        Ok(())
    }

    #[test]
    fn a_photo_renamed_into_a_new_folder_keeps_its_rating_keywords_and_edit() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        f.cat.set_metadata_of(&[(id, 3, 0, String::new())])?;
        f.cat.add_keywords(&[id], &[vec!["Trip".into()]])?;
        let recipe = f.edit(id)?;
        let to = f.root.join("Lisbon/selects/keep-01.dng");
        copy_then_delete(&f.root.join("Lisbon/IMG_1001.dng"), &to)?;
        let plan = f.plan(SyncScope::Folder(f.folder("Lisbon")))?;
        assert_eq!(plan.moves.len(), 1);
        assert!(plan.moves[0].verified && plan.moves[0].to == to);
        assert!(plan.additions.is_empty() && plan.missing.is_empty());
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!((applied.moved, applied.protected), (vec![id], vec![]));
        assert_eq!(f.path_of(id), to);
        let photo = f.cat.photos()?.into_iter().find(|p| p.id == id).unwrap();
        assert_eq!((photo.rating, photo.keywords.as_str()), (3, "Trip"));
        // The edit opens at the new path, its identity refreshed.
        assert_eq!(f.cat.load_edit(id, &to)?.unwrap().recipe, recipe);
        // A second sync finds nothing to do.
        let again = f.plan(SyncScope::All)?;
        assert!(again.moves.is_empty() && again.additions.is_empty() && again.missing.is_empty());
        Ok(())
    }

    #[test]
    fn a_folder_listed_under_another_spelling_of_its_name_is_not_new() -> Result<()> {
        // As macOS's FAT and exFAT list a folder RAWmakase named composed:
        // decomposed, found by either (#573).
        let mut f = fixture()?;
        std::fs::create_dir(f.root.join("Lisbon/\u{c9}vora"))?;
        std::fs::write(f.root.join("Lisbon/\u{c9}vora/IMG_3001.dng"), bytes(4))?;
        f.cat.add_folder(&f.root)?;
        f.checksum_all()?;
        std::fs::rename(
            f.root.join("Lisbon/\u{c9}vora"),
            f.root.join("Lisbon/E\u{301}vora"),
        )?;
        if !f.root.join("Lisbon/\u{c9}vora/IMG_3001.dng").exists() {
            // A volume that tells the two spellings apart: two folders.
            return Ok(());
        }
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.additions.is_empty(), "{:?}", plan.additions);
        assert!(plan.missing.is_empty() && plan.moves.is_empty());
        Ok(())
    }

    #[test]
    fn a_photo_moved_to_another_catalog_folder_is_found_from_there() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1002.dng");
        let to = f.root.join("Porto/IMG_1002.dng");
        std::fs::rename(f.root.join("Lisbon/IMG_1002.dng"), &to)?;
        // Syncing Porto alone finds the file that left Lisbon.
        let plan = f.plan(SyncScope::Folder(f.folder("Porto")))?;
        assert_eq!(plan.moves.iter().map(|m| m.photo).collect::<Vec<_>>(), [id]);
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.path_of(id), to);
        Ok(())
    }

    #[test]
    fn changed_content_stays_missing_and_the_file_is_new() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        std::fs::remove_file(f.root.join("Lisbon/IMG_1001.dng"))?;
        let mut changed = bytes(1);
        changed[0] ^= 0xff;
        std::fs::write(f.root.join("Lisbon/IMG_1001-edited.dng"), changed)?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.moves.is_empty() && plan.choices.is_empty());
        assert_eq!(
            plan.missing.iter().map(|m| m.photo).collect::<Vec<_>>(),
            [id]
        );
        assert_eq!(plan.additions.len(), 1);
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.added, 1);
        assert_eq!(f.cat.photos()?.len(), 4);
        Ok(())
    }

    #[test]
    fn a_same_size_change_outside_the_sampled_blocks_is_not_a_match() -> Result<()> {
        let f = fixture()?;
        let id = f.id("IMG_1001.dng");
        std::fs::remove_file(f.root.join("Lisbon/IMG_1001.dng"))?;
        let mut changed = bytes(1);
        // Between the start and middle blocks.
        changed[80_000] ^= 0xff;
        std::fs::write(f.root.join("Lisbon/renamed.dng"), changed)?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.moves.is_empty() && plan.choices.is_empty());
        assert_eq!(
            plan.missing.iter().map(|m| m.photo).collect::<Vec<_>>(),
            [id]
        );
        Ok(())
    }

    #[test]
    fn two_copies_of_a_moved_photo_need_a_choice() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        let from = f.root.join("Lisbon/IMG_1001.dng");
        std::fs::copy(&from, f.root.join("Lisbon/a.dng"))?;
        std::fs::rename(&from, f.root.join("Porto/b.dng"))?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.moves.is_empty());
        assert_eq!(plan.choices.len(), 1);
        assert_eq!(plan.choices[0].options.len(), 2);
        // Left as it is unless the user picks.
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.path_of(id), from);
        let pick = plan.choices[0].options[1].clone();
        let accepted = Accepted {
            moves: vec![(pick.photo, pick.to.clone())],
            ..Default::default()
        };
        let plan = f.plan(SyncScope::All)?;
        f.apply(&plan, &accepted)?;
        assert_eq!(f.path_of(id), pick.to);
        Ok(())
    }

    #[test]
    fn a_photo_without_a_checksum_matched_by_its_edit_is_only_probable() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        f.edit(id)?;
        f.cat
            .db_for_tests()
            .execute("DELETE FROM photo_checksums WHERE photo=?", [id.0])?;
        let to = f.root.join("Porto/IMG_1001.dng");
        copy_then_delete(&f.root.join("Lisbon/IMG_1001.dng"), &to)?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.moves.len(), 1);
        assert!(!plan.moves[0].verified);
        assert!(plan.default_accepted().moves.is_empty());
        let accepted = Accepted {
            moves: vec![(id, to.clone())],
            ..Default::default()
        };
        f.apply(&plan, &accepted)?;
        assert_eq!(f.path_of(id), to);
        // Not verified, but the same photo (`Identity::same_photo`): its edit
        // still applies, whatever the copy's modification time.
        assert!(f.cat.load_edit(id, &to)?.is_some());
        Ok(())
    }

    #[test]
    fn an_edit_already_protected_stays_protected_after_a_verified_move() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        f.edit(id)?;
        // The file's bytes changed after the edit was saved; the checksum is
        // of the new version.
        let from = f.root.join("Lisbon/IMG_1001.dng");
        let mut bytes = std::fs::read(&from)?;
        bytes[16] ^= 0xff;
        std::fs::write(&from, bytes)?;
        f.checksum_all()?;
        let to = f.root.join("Lisbon/moved.dng");
        copy_then_delete(&from, &to)?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.moves[0].verified);
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.protected, [id]);
        assert!(f.cat.load_edit(id, &to).is_err());
        Ok(())
    }

    #[test]
    fn virtual_copies_move_with_their_master_and_keep_their_edits() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        let copy = f.cat.create_virtual_copy(id)?;
        let recipe = f.edit(copy)?;
        let to = f.root.join("Porto/IMG_1001.dng");
        std::fs::rename(f.root.join("Lisbon/IMG_1001.dng"), &to)?;
        let plan = f.plan(SyncScope::All)?;
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.path_of(copy), to);
        assert_eq!(f.cat.load_edit(copy, &to)?.unwrap().recipe, recipe);
        Ok(())
    }

    #[test]
    fn a_family_changed_after_the_scan_is_skipped() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        let copy = f.cat.create_virtual_copy(id)?;
        let to = f.root.join("Porto/IMG_1001.dng");
        std::fs::rename(f.root.join("Lisbon/IMG_1001.dng"), &to)?;
        let plan = f.plan(SyncScope::All)?;
        // Set Copy as Master during the review.
        f.cat.set_copy_as_master(copy)?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert!(applied.moved.is_empty());
        assert_eq!(applied.skipped.len(), 1);
        // A keyword added: the family differs from what was reviewed.
        let plan = f.plan(SyncScope::All)?;
        f.cat.add_keywords(&[copy], &[vec!["Late".into()]])?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert!(applied.moved.is_empty());
        // As reviewed, it moves.
        let plan = f.plan(SyncScope::All)?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.moved, [copy]);
        assert_eq!(f.path_of(id), to);
        Ok(())
    }

    #[test]
    fn a_file_changed_or_an_old_path_back_since_the_scan_is_skipped() -> Result<()> {
        let mut f = fixture()?;
        let from = f.root.join("Lisbon/IMG_1001.dng");
        let to = f.root.join("Porto/IMG_1001.dng");
        std::fs::rename(&from, &to)?;
        let plan = f.plan(SyncScope::All)?;
        std::fs::copy(&to, &from)?;
        assert!(f.apply(&plan, &plan.default_accepted())?.moved.is_empty());
        std::fs::remove_file(&from)?;
        let plan = f.plan(SyncScope::All)?;
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
        std::fs::File::options()
            .write(true)
            .open(&to)?
            .set_modified(later)?;
        assert!(f.apply(&plan, &plan.default_accepted())?.moved.is_empty());
        Ok(())
    }

    #[test]
    fn removing_missing_photos_takes_their_copies_and_spares_the_open_one() -> Result<()> {
        let mut f = fixture()?;
        let (a, b) = (f.id("IMG_1001.dng"), f.id("IMG_1002.dng"));
        let copy = f.cat.create_virtual_copy(a)?;
        std::fs::remove_file(f.root.join("Lisbon/IMG_1001.dng"))?;
        std::fs::remove_file(f.root.join("Lisbon/IMG_1002.dng"))?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.missing.len(), 2);
        let accepted = Accepted {
            removals: vec![a, b],
            ..Default::default()
        };
        let applied =
            f.cat
                .apply_folder_sync(&plan, &accepted, &|_| true, Some(b), &Default::default())?;
        let mut removed = applied.removed.clone();
        removed.sort();
        let mut expected = vec![a, copy];
        expected.sort();
        assert_eq!(removed, expected);
        assert_eq!(applied.skipped.len(), 1);
        assert!(f.cat.photos()?.iter().any(|p| p.id == b));
        let left: i64 = f.cat.db_for_tests().query_row(
            "SELECT count(*) FROM photo_checksums WHERE photo=?",
            [a.0],
            |r| r.get(0),
        )?;
        assert_eq!(left, 0);
        Ok(())
    }

    #[test]
    fn nothing_is_missing_where_the_volume_is_offline() -> Result<()> {
        let f = fixture()?;
        std::fs::remove_file(f.root.join("Lisbon/IMG_1001.dng"))?;
        let plan = f.plan_with(SyncScope::All, &|_| false)?;
        assert!(plan.missing.is_empty() && plan.moves.is_empty());
        // A root that isn't there at all: unavailable, never missing.
        std::fs::rename(&f.root, f.root.with_file_name("elsewhere"))?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.missing.is_empty() && plan.gone_folders.is_empty());
        Ok(())
    }

    #[test]
    fn a_folder_located_apart_under_an_offline_root_is_scanned() -> Result<()> {
        let mut f = fixture()?;
        let porto = f.folder("Porto");
        let apart = f.root.with_file_name("porto-elsewhere");
        std::fs::rename(f.root.join("Porto"), &apart)?;
        f.cat.relink_folder(porto, &apart)?;
        // The root itself goes offline.
        std::fs::rename(&f.root, f.root.with_file_name("gone"))?;
        let id = f.id("IMG_2001.dng");
        std::fs::rename(apart.join("IMG_2001.dng"), apart.join("renamed.dng"))?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.moves.iter().map(|m| m.photo).collect::<Vec<_>>(), [id]);
        assert!(plan.missing.is_empty());
        Ok(())
    }

    #[test]
    fn a_renamed_directory_moves_its_photos_and_its_empty_folder_goes() -> Result<()> {
        let mut f = fixture()?;
        let lisbon = f.folder("Lisbon");
        std::fs::rename(f.root.join("Lisbon"), f.root.join("Lisbon 2026"))?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.moves.len(), 2);
        assert_eq!(plan.gone_folders, [lisbon]);
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.folders_removed, [lisbon]);
        let folders: Vec<String> = f.cat.folders()?.into_iter().map(|f| f.relative).collect();
        assert!(folders.contains(&"Lisbon 2026".to_string()));
        assert!(!folders.contains(&"Lisbon".to_string()));
        Ok(())
    }

    #[test]
    fn a_copy_of_a_present_photo_is_offered_unticked() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        std::fs::copy(
            f.root.join("Lisbon/IMG_1001.dng"),
            f.root.join("Lisbon/IMG_1001 copy.dng"),
        )?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.additions.len(), 1);
        assert_eq!(plan.additions[0].copy_of, [id]);
        assert!(f.apply(&plan, &plan.default_accepted())?.added == 0);
        Ok(())
    }

    #[test]
    fn new_files_are_added_with_their_metadata() -> Result<()> {
        let mut f = fixture()?;
        let file = f.root.join("Lisbon/new/IMG_3001.dng");
        std::fs::create_dir_all(file.parent().unwrap())?;
        std::fs::write(&file, bytes(9))?;
        std::fs::write(
            f.root.join("Lisbon/new/IMG_3001.dng.xmp"),
            r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4"/>
            </rdf:RDF></x:xmpmeta>"#,
        )?;
        let plan = f.plan(SyncScope::All)?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.added, 1);
        let added = f.id("IMG_3001.dng");
        let photo = f.cat.photos()?.into_iter().find(|p| p.id == added).unwrap();
        assert_eq!((photo.rating, photo.path), (4, file));
        Ok(())
    }

    /// Issue #941: a root stored under another spelling of its path than
    /// the disk's own, as a Lightroom catalog or an earlier release may have
    /// it. Here that is a symbolic link; on Windows, the path without the
    /// `\\?\` prefix that the disk's own spelling has.
    #[test]
    fn a_root_stored_under_another_spelling_gets_its_new_and_moved_photos() -> Result<()> {
        let mut f = fixture()?;
        #[cfg(unix)]
        let spelled = {
            let link = f._dir.path().join("link");
            std::os::unix::fs::symlink(&f.root, &link)?;
            link
        };
        #[cfg(windows)]
        let spelled = PathBuf::from(
            f.root
                .to_string_lossy()
                .strip_prefix(r"\\?\")
                .expect("a canonical path")
                .to_string(),
        );
        assert_ne!(spelled, f.root);
        f.cat.db_for_tests().execute(
            "UPDATE roots SET original_path=?",
            [spelled.to_string_lossy()],
        )?;
        let moved = f.id("IMG_2001.dng");
        copy_then_delete(
            &f.root.join("Porto/IMG_2001.dng"),
            &f.root.join("Lisbon/IMG_2001.dng"),
        )?;
        std::fs::write(f.root.join("Lisbon/IMG_1003.dng"), bytes(7))?;
        std::fs::create_dir(f.root.join("Lisbon/new"))?;
        std::fs::write(f.root.join("Lisbon/new/IMG_3001.dng"), bytes(8))?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.not_checked.is_empty(), "{:?}", plan.not_checked);
        assert_eq!(plan.additions.len(), 2, "{:?}", plan.additions);
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
        assert_eq!((applied.added, applied.moved), (2, vec![moved]));
        assert_eq!(f.path_of(moved), spelled.join("Lisbon/IMG_2001.dng"));
        assert_eq!(
            f.path_of(f.id("IMG_3001.dng")),
            spelled.join("Lisbon/new/IMG_3001.dng")
        );
        Ok(())
    }

    /// A directory spelled through the root's stored link, below a folder
    /// located apart at the link's target, goes to that folder: the more
    /// specific location wins whichever spelling matched.
    #[cfg(unix)]
    #[test]
    fn a_location_found_only_as_the_disk_spells_it_still_wins() -> Result<()> {
        use crate::catalog::ingest::{Matched, Matcher, Place};
        let mut f = fixture()?;
        let link = f._dir.path().join("link");
        std::os::unix::fs::symlink(&f.root, &link)?;
        std::fs::create_dir_all(f.root.join("Other/new"))?;
        let porto = f.folder("Porto");
        f.cat.relink_folder(porto, &f.root.join("Other"))?;
        f.cat
            .db_for_tests()
            .execute("UPDATE roots SET original_path=?", [link.to_string_lossy()])?;
        let directory = link.join("Other/new");
        let placed = Matcher::new(&f.cat)?.place(&directory, &directory, &[]);
        assert!(
            matches!(&placed, Matched::Place(Place::Folder(_, logical)) if logical == "Porto/new"),
            "{}",
            match placed {
                Matched::Place(Place::Folder(_, l)) => l,
                _ => "not a folder".into(),
            }
        );
        Ok(())
    }

    /// A catalog folder reached through a symbolic link keeps the photos
    /// synchronized into it: the folder is the one the catalog names, not
    /// the one the link points to.
    #[cfg(unix)]
    #[test]
    fn a_new_photo_in_a_folder_behind_a_link_joins_that_folder() -> Result<()> {
        let mut f = fixture()?;
        let old = f.id("IMG_1001.dng");
        std::fs::rename(f.root.join("Lisbon"), f.root.join("Actual"))?;
        std::os::unix::fs::symlink(f.root.join("Actual"), f.root.join("Lisbon"))?;
        std::fs::write(f.root.join("Lisbon/IMG_1003.dng"), bytes(7))?;
        let lisbon = f.folder("Lisbon");
        let plan = f.plan(SyncScope::Folder(lisbon))?;
        assert_eq!(plan.additions.len(), 1, "{:?}", plan.additions);
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert!(applied.skipped.is_empty(), "{:?}", applied.skipped);
        assert_eq!(applied.added, 1);
        let photos = f.cat.photos()?;
        let folder_of = |id| photos.iter().find(|p| p.id == id).unwrap().folder;
        assert_eq!(folder_of(f.id("IMG_1003.dng")), lisbon);
        assert_eq!(folder_of(old), lisbon);
        Ok(())
    }

    #[test]
    fn a_checksum_replaced_after_the_scan_skips_the_move() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        std::fs::rename(
            f.root.join("Lisbon/IMG_1001.dng"),
            f.root.join("Porto/IMG_1001.dng"),
        )?;
        let plan = f.plan(SyncScope::All)?;
        // The row the match was made with is replaced, as a re-created
        // photo's would be.
        f.cat.db_for_tests().execute(
            "UPDATE photo_checksums SET full_hash='other' WHERE photo=?",
            [id.0],
        )?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert!(applied.moved.is_empty());
        assert_eq!(f.path_of(id), f.root.join("Lisbon/IMG_1001.dng"));
        Ok(())
    }

    #[test]
    fn a_master_whose_copy_is_open_is_not_removed() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        let copy = f.cat.create_virtual_copy(id)?;
        std::fs::remove_file(f.root.join("Lisbon/IMG_1001.dng"))?;
        let plan = f.plan(SyncScope::All)?;
        let accepted = Accepted {
            removals: vec![id],
            ..Default::default()
        };
        let applied = f.cat.apply_folder_sync(
            &plan,
            &accepted,
            &|_| true,
            Some(copy),
            &Default::default(),
        )?;
        assert!(applied.removed.is_empty());
        assert!(f.cat.photos()?.iter().any(|p| p.id == copy));
        Ok(())
    }

    #[test]
    fn a_destination_located_elsewhere_after_the_scan_skips_the_move() -> Result<()> {
        let mut f = fixture()?;
        let id = f.id("IMG_1001.dng");
        std::fs::rename(
            f.root.join("Lisbon/IMG_1001.dng"),
            f.root.join("Porto/IMG_1001.dng"),
        )?;
        let plan = f.plan(SyncScope::All)?;
        // Porto found elsewhere, holding other files.
        let elsewhere = f.root.with_file_name("porto-copy");
        std::fs::create_dir_all(&elsewhere)?;
        std::fs::write(elsewhere.join("IMG_1001.dng"), bytes(42))?;
        f.cat.relink_folder(f.folder("Porto"), &elsewhere)?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert!(applied.moved.is_empty());
        assert_eq!(applied.skipped.len(), 1);
        assert!(f.path_of(id).ends_with("Lisbon/IMG_1001.dng"));
        Ok(())
    }

    #[test]
    fn a_present_photos_old_checksum_makes_no_copy() -> Result<()> {
        let f = fixture()?;
        let present = f.root.join("Lisbon/IMG_1001.dng");
        // A file with what IMG_1001 held, while IMG_1001 now holds other
        // bytes of the same size.
        std::fs::copy(&present, f.root.join("Lisbon/old.dng"))?;
        std::fs::write(&present, bytes(77))?;
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(30);
        std::fs::File::options()
            .write(true)
            .open(&present)?
            .set_modified(later)?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.additions.len(), 1);
        assert!(plan.additions[0].copy_of.is_empty());
        Ok(())
    }

    #[test]
    fn a_case_only_rename_on_a_case_insensitive_volume_changes_nothing() -> Result<()> {
        let f = fixture()?;
        let from = f.root.join("Lisbon/IMG_1001.dng");
        if !f.root.join("Lisbon/img_1001.DNG").exists() {
            // A case-sensitive volume: there it is a rename like any other.
            return Ok(());
        }
        let tmp = f.root.join("Lisbon/tmp.dng");
        std::fs::rename(&from, &tmp)?;
        std::fs::rename(&tmp, f.root.join("Lisbon/img_1001.dng"))?;
        let plan = f.plan(SyncScope::All)?;
        assert!(plan.moves.is_empty() && plan.choices.is_empty());
        assert!(plan.additions.is_empty() && plan.missing.is_empty());
        Ok(())
    }

    #[test]
    fn a_new_file_changed_or_gone_after_the_scan_is_not_added() -> Result<()> {
        let mut f = fixture()?;
        let (a, b) = (
            f.root.join("Lisbon/new-a.dng"),
            f.root.join("Lisbon/new-b.dng"),
        );
        std::fs::write(&a, bytes(50))?;
        std::fs::write(&b, bytes(51))?;
        let plan = f.plan(SyncScope::All)?;
        std::fs::write(&a, bytes(52))?;
        std::fs::remove_file(&b)?;
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!((applied.added, applied.skipped.len()), (0, 2));
        Ok(())
    }

    #[test]
    fn an_offline_folder_is_reported_not_checked() -> Result<()> {
        let f = fixture()?;
        let plan = f.plan_with(SyncScope::All, &|_| false)?;
        assert!(!plan.not_checked.is_empty());
        assert!(plan.missing.is_empty());
        Ok(())
    }

    #[test]
    fn empty_folders_go_deepest_first() -> Result<()> {
        let mut f = fixture()?;
        std::fs::create_dir_all(f.root.join("Lisbon/day 2"))?;
        std::fs::write(f.root.join("Lisbon/day 2/IMG_1101.dng"), bytes(60))?;
        f.cat.add_folder(&f.root)?;
        f.checksum_all()?;
        std::fs::rename(f.root.join("Lisbon"), f.root.join("Lisboa"))?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.gone_folders.len(), 2);
        let applied = f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(applied.folders_removed.len(), 2);
        let folders: Vec<String> = f.cat.folders()?.into_iter().map(|f| f.relative).collect();
        assert!(!folders.iter().any(|f| f.starts_with("Lisbon")));
        Ok(())
    }

    #[test]
    fn a_copy_of_a_photo_without_a_checksum_is_still_a_copy() -> Result<()> {
        let f = fixture()?;
        let id = f.id("IMG_1001.dng");
        f.cat
            .db_for_tests()
            .execute_batch("DELETE FROM photo_checksums")?;
        std::fs::copy(
            f.root.join("Lisbon/IMG_1001.dng"),
            f.root.join("Lisbon/IMG_1001 copy.dng"),
        )?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.additions.len(), 1);
        assert_eq!(plan.additions[0].copy_of, [id]);
        Ok(())
    }

    #[test]
    fn a_new_photo_in_a_root_whose_folders_are_all_located_apart_is_found() -> Result<()> {
        let mut f = fixture()?;
        for name in ["Lisbon", "Porto"] {
            let apart = f.root.with_file_name(format!("{name}-apart"));
            std::fs::rename(f.root.join(name), &apart)?;
            f.cat.relink_folder(f.folder(name), &apart)?;
        }
        std::fs::write(f.root.join("IMG_0001.dng"), bytes(80))?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.additions.len(), 1);
        assert_eq!(plan.additions[0].file, f.root.join("IMG_0001.dng"));
        Ok(())
    }

    #[test]
    fn a_scan_stopped_on_its_last_file_gives_no_plan() -> Result<()> {
        let f = fixture()?;
        std::fs::write(f.root.join("Lisbon/new.dng"), bytes(81))?;
        let inputs = f.cat.sync_inputs(SyncScope::All)?;
        let cancel = AtomicBool::new(false);
        let stopped = scan(&inputs, &|_| true, Cancel::new(&cancel), &|_| {
            cancel.store(true, std::sync::atomic::Ordering::Relaxed)
        });
        assert!(stopped.is_err());
        Ok(())
    }

    #[test]
    fn previous_import_is_the_last_add_or_synchronize() -> Result<()> {
        let mut f = fixture()?;
        assert_eq!(f.cat.previous_import()?.len(), 3);
        let file = f.root.join("Porto/IMG_2002.dng");
        std::fs::write(&file, bytes(70))?;
        let plan = f.plan(SyncScope::Root(f.cat.roots()?[0].0))?;
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.cat.previous_import()?, [f.id("IMG_2002.dng")]);
        // Adding the folder again adds nothing: Previous Import is empty.
        f.cat.add_folder(&f.root)?;
        assert!(f.cat.previous_import()?.is_empty());
        // So does a synchronization that only relinks.
        std::fs::write(f.root.join("Porto/IMG_2003.dng"), bytes(71))?;
        let plan = f.plan(SyncScope::All)?;
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.cat.previous_import()?.len(), 1);
        f.checksum_all()?;
        std::fs::rename(
            f.root.join("Porto/IMG_2003.dng"),
            f.root.join("Porto/renamed.dng"),
        )?;
        let plan = f.plan(SyncScope::All)?;
        assert_eq!(plan.moves.len(), 1);
        let accepted = Accepted {
            moves: plan.moves.iter().map(|m| (m.photo, m.to.clone())).collect(),
            ..Default::default()
        };
        f.apply(&plan, &accepted)?;
        assert!(f.cat.previous_import()?.is_empty());
        // And one whose new files all changed before it applied.
        std::fs::write(f.root.join("Porto/IMG_2004.dng"), bytes(72))?;
        let plan = f.plan(SyncScope::All)?;
        f.apply(&plan, &plan.default_accepted())?;
        assert_eq!(f.cat.previous_import()?.len(), 1);
        std::fs::write(f.root.join("Porto/IMG_2005.dng"), bytes(73))?;
        let plan = f.plan(SyncScope::All)?;
        std::fs::remove_file(f.root.join("Porto/IMG_2005.dng"))?;
        f.apply(&plan, &plan.default_accepted())?;
        assert!(f.cat.previous_import()?.is_empty());
        Ok(())
    }
}
