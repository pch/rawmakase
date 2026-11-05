//! Find on Disk (issue #693) on synthetic files.
use super::super::{Accepted, Applied, SyncPlan};
use super::find_on_disk;
use crate::catalog::locations::Computer;
use crate::catalog::{Catalog, PhotoId, ReadChecksum};
use crate::storage::{ChecksumRead, FileChecksum, Identity};
use crate::{export_settings::ExportOptions, model::recipe::Recipe};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::path::{Path, PathBuf};

/// Distinct synthetic bytes, larger than the quick hash's three blocks.
fn bytes(seed: u8) -> Vec<u8> {
    (0..300_000u32)
        .map(|i| (i as u8).wrapping_mul(seed) ^ seed)
        .collect()
}

/// A catalog of one root, "trip", with folders "Lisbon" (IMG_1001 to
/// IMG_1005) and "Porto" (IMG_2001), every checksum read in full.
struct Fixture {
    dir: tempfile::TempDir,
    root: PathBuf,
    catalog: PathBuf,
    cat: Catalog,
}

fn fixture() -> Result<Fixture> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?.join("trip");
    std::fs::create_dir_all(root.join("Lisbon"))?;
    std::fs::create_dir_all(root.join("Porto"))?;
    for n in 1..=5u8 {
        std::fs::write(root.join(format!("Lisbon/IMG_100{n}.dng")), bytes(n))?;
    }
    std::fs::write(root.join("Porto/IMG_2001.dng"), bytes(20))?;
    let catalog = dir.path().join("Photos.rawmakase");
    let mut cat = Catalog::create(&catalog)?;
    cat.add_folder(&root)?;
    let read: Vec<ReadChecksum> = cat
        .photos()?
        .into_iter()
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
    cat.save_checksums(&read)?;
    Ok(Fixture {
        dir,
        root,
        catalog,
        cat,
    })
}

impl Fixture {
    fn id(&self, name: &str) -> PhotoId {
        self.cat
            .photos()
            .unwrap()
            .into_iter()
            .find(|p| p.filename == name && p.master.is_none())
            .unwrap()
            .id
    }
    fn path_of(&self, id: PhotoId) -> PathBuf {
        let photos = self.cat.photos().unwrap();
        photos.into_iter().find(|p| p.id == id).unwrap().path
    }
    fn lisbon(&self, name: &str) -> PathBuf {
        self.root.join("Lisbon").join(name)
    }
    /// A directory outside every root.
    fn outside(&self) -> PathBuf {
        let dir = self.dir.path().canonicalize().unwrap().join("moved");
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
    /// Moves Lisbon's `name` to `to`, as another volume would: new time.
    fn move_out(&self, name: &str, to: &Path) -> Result<()> {
        let from = self.lisbon(name);
        std::fs::create_dir_all(to.parent().unwrap())?;
        std::fs::copy(&from, to)?;
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(to)?
            .set_modified(later)?;
        std::fs::remove_file(from)?;
        Ok(())
    }
    fn find(&self, id: PhotoId, file: &Path) -> Result<SyncPlan> {
        self.find_with(id, file, &|_| true)
    }
    fn find_with(
        &self,
        id: PhotoId,
        file: &Path,
        online: &dyn Fn(&Path) -> bool,
    ) -> Result<SyncPlan> {
        let inputs = self.cat.find_inputs(id, file)?;
        find_on_disk(&inputs, online, Cancel::NEVER, &|_| {})
    }
    fn apply(&mut self, plan: &SyncPlan, accepted: &Accepted) -> Result<Applied> {
        self.cat
            .apply_folder_sync(plan, accepted, &|_| true, None, &Default::default())
    }
    fn roots(&self) -> usize {
        self.cat.roots().unwrap().len()
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
    fn sql(&self, statement: &str, id: PhotoId) -> Result<()> {
        self.cat.db_for_tests().execute(statement, [id.0])?;
        Ok(())
    }
}

#[test]
fn a_verified_file_outside_every_root_relinks_into_a_new_root() -> Result<()> {
    let mut f = fixture()?;
    let (a, b, c) = (
        f.id("IMG_1001.dng"),
        f.id("IMG_1002.dng"),
        f.id("IMG_1003.dng"),
    );
    f.cat.set_metadata_of(&[(a, 4, 0, String::new())])?;
    let recipe = f.edit(a)?;
    let copy = f.cat.create_virtual_copy(a)?;
    let previous = f.cat.previous_import()?;
    let out = f.outside();
    for name in ["IMG_1001.dng", "IMG_1002.dng", "IMG_1003.dng"] {
        f.move_out(name, &out.join(name))?;
    }
    // Renamed on the way: found by its content.
    std::fs::rename(out.join("IMG_1003.dng"), out.join("renamed.dng"))?;
    let plan = f.find(copy, &out.join("IMG_1001.dng"))?;
    let picked = plan.picked().expect("the file picked");
    assert_eq!((picked.photo, picked.verified), (a, true));
    assert_eq!(picked.new_root(), Some(out.as_path()));
    assert_eq!(plan.moves.len(), 3);
    assert!(plan.moves.iter().all(|m| m.verified && !m.offline));
    assert!(plan.choices.is_empty() && plan.additions.is_empty() && plan.missing.is_empty());
    let accepted = plan.default_accepted();
    assert_eq!(accepted.moves.len(), 3);
    let roots = f.roots();
    let applied = f.apply(&plan, &accepted)?;
    let mut moved = applied.moved.clone();
    moved.sort();
    let mut expected = vec![a, b, c];
    expected.sort();
    assert_eq!(moved, expected);
    assert!(applied.skipped.is_empty() && applied.protected.is_empty());
    assert_eq!(f.roots(), roots + 1);
    assert_eq!(f.path_of(a), out.join("IMG_1001.dng"));
    assert_eq!(f.path_of(copy), out.join("IMG_1001.dng"));
    assert_eq!(f.path_of(c), out.join("renamed.dng"));
    let photo = f.cat.photos()?.into_iter().find(|p| p.id == a).unwrap();
    assert_eq!(photo.rating, 4);
    assert_eq!(
        f.cat
            .load_edit(a, &out.join("IMG_1001.dng"))?
            .unwrap()
            .recipe,
        recipe
    );
    // The checksum is of the file found now.
    let identity: String = f.cat.db_for_tests().query_row(
        "SELECT identity FROM photo_checksums WHERE photo=?",
        [a.0],
        |r| r.get(0),
    )?;
    let identity: Identity = serde_json::from_str(&identity)?;
    assert_eq!(identity, Identity::read(&out.join("IMG_1001.dng"))?);
    // Find on Disk makes no import batch.
    assert_eq!(f.cat.previous_import()?, previous);
    // The other photos of Lisbon are where they were.
    assert_eq!(f.path_of(f.id("IMG_1004.dng")), f.lisbon("IMG_1004.dng"));
    Ok(())
}

#[test]
fn the_new_root_resolves_on_every_computer_and_in_older_releases() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let out = f.outside();
    f.move_out("IMG_1001.dng", &out.join("IMG_1001.dng"))?;
    let plan = f.find(a, &out.join("IMG_1001.dng"))?;
    f.apply(&plan, &plan.default_accepted())?;
    let catalog = f.catalog.clone();
    drop(f.cat);
    let path = |computer: &str| -> Result<PathBuf> {
        let computer = Computer {
            id: computer.into(),
            name: computer.into(),
        };
        let cat = Catalog::open_as(&catalog, &computer)?;
        Ok(cat.photos()?.into_iter().find(|p| p.id == a).unwrap().path)
    };
    assert_eq!(path(&Computer::this().id)?, out.join("IMG_1001.dng"));
    // Where it was added, on a computer that has no location of it.
    assert_eq!(path("another")?, out.join("IMG_1001.dng"));
    // As an older release reads it.
    let legacy = rusqlite::Connection::open(&catalog)?;
    let (original, mapped): (String, Option<String>) = legacy.query_row(
        "SELECT r.original_path, r.mapped_path FROM roots r
         JOIN folders f ON f.root=r.id JOIN photos p ON p.folder=f.id WHERE p.id=?",
        [a.0],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    assert_eq!((PathBuf::from(original), mapped), (out, None));
    Ok(())
}

#[test]
fn a_file_in_a_catalog_folder_relinks_there_without_a_new_root() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let to = f.root.join("Porto/IMG_1001.dng");
    f.move_out("IMG_1001.dng", &to)?;
    let plan = f.find(a, &to)?;
    assert_eq!(plan.picked().unwrap().new_root(), None);
    let roots = f.roots();
    let applied = f.apply(&plan, &plan.default_accepted())?;
    assert_eq!(applied.moved, [a]);
    assert_eq!(f.roots(), roots);
    assert_eq!(f.path_of(a), to);
    Ok(())
}

#[test]
fn a_file_the_checksum_contradicts_is_refused() -> Result<()> {
    let f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let out = f.outside();
    std::fs::remove_file(f.lisbon("IMG_1001.dng"))?;
    // Another photo's bytes, and another size.
    std::fs::write(out.join("IMG_1001.dng"), bytes(9))?;
    std::fs::write(out.join("short.dng"), b"synthetic")?;
    for file in ["IMG_1001.dng", "short.dng"] {
        let refused = f.find(a, &out.join(file)).unwrap_err();
        assert!(
            format!("{refused:#}").contains("a different photo than IMG_1001.dng"),
            "{refused:#}"
        );
    }
    assert_eq!(f.path_of(a), f.lisbon("IMG_1001.dng"));
    Ok(())
}

#[test]
fn a_full_checksum_mismatch_is_refused_and_never_offered() -> Result<()> {
    let f = fixture()?;
    let (a, b) = (f.id("IMG_1001.dng"), f.id("IMG_1002.dng"));
    let out = f.outside();
    // Same size and quick hash, a byte changed outside the sampled blocks.
    let altered = |seed: u8| {
        let mut data = bytes(seed);
        data[100_000] ^= 0xff;
        data
    };
    std::fs::remove_file(f.lisbon("IMG_1001.dng"))?;
    std::fs::write(out.join("IMG_1001.dng"), altered(1))?;
    let quick = |path: &Path| match FileChecksum::read_quick(path, |_| true).unwrap() {
        ChecksumRead::Done(c) => c.quick,
        _ => unreachable!(),
    };
    std::fs::write(out.join("original.dng"), bytes(1))?;
    assert_eq!(
        quick(&out.join("IMG_1001.dng")),
        quick(&out.join("original.dng"))
    );
    std::fs::remove_file(out.join("original.dng"))?;
    assert!(f.find(a, &out.join("IMG_1001.dng")).is_err());
    // Beside a photo picked: the same name and quick hash, not offered.
    f.move_out("IMG_1002.dng", &out.join("picked.dng"))?;
    std::fs::remove_file(f.lisbon("IMG_1003.dng")).ok();
    std::fs::write(out.join("IMG_1001.dng"), altered(1))?;
    let plan = f.find(b, &out.join("picked.dng"))?;
    assert_eq!(plan.moves.len(), 1);
    assert!(plan.choices.is_empty());
    Ok(())
}

#[test]
fn nearby_photos_are_offered_by_how_sure_the_match_is() -> Result<()> {
    let f = fixture()?;
    let ids: Vec<PhotoId> = (1..=5).map(|n| f.id(&format!("IMG_100{n}.dng"))).collect();
    // 1001 picked; 1002 verified; 1003 has no full checksum; 1004 none at
    // all; 1005's file of the same name is another photo.
    f.sql(
        "UPDATE photo_checksums SET full_hash=NULL WHERE photo=?",
        ids[2],
    )?;
    f.sql("DELETE FROM photo_checksums WHERE photo=?", ids[3])?;
    let out = f.outside();
    for n in 1..=4 {
        let name = format!("IMG_100{n}.dng");
        f.move_out(&name, &out.join(&name))?;
    }
    std::fs::rename(out.join("IMG_1003.dng"), out.join("other-name.dng"))?;
    std::fs::remove_file(f.lisbon("IMG_1005.dng"))?;
    std::fs::write(out.join("IMG_1005.dng"), bytes(55))?;
    let plan = f.find(ids[0], &out.join("IMG_1001.dng"))?;
    let of = |id: PhotoId| plan.moves.iter().find(|m| m.photo == id);
    let verified = of(ids[1]).unwrap();
    assert!(verified.verified && !verified.by_name);
    let probable = of(ids[2]).unwrap();
    assert!(!probable.verified && !probable.by_name);
    assert_eq!(probable.to, out.join("other-name.dng"));
    let named = of(ids[3]).unwrap();
    assert!(!named.verified && named.by_name);
    assert!(of(ids[4]).is_none());
    assert!(plan.choices.is_empty());
    // Only the file picked and the verified match start ticked.
    let mut ticked: Vec<PhotoId> = plan
        .default_accepted()
        .moves
        .iter()
        .map(|(id, _)| *id)
        .collect();
    ticked.sort();
    let mut expected = vec![ids[0], ids[1]];
    expected.sort();
    assert_eq!(ticked, expected);
    Ok(())
}

#[test]
fn the_picked_file_without_a_checksum_is_ticked_but_not_verified() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    f.edit(a)?;
    f.sql("DELETE FROM photo_checksums WHERE photo=?", a)?;
    let out = f.outside();
    f.move_out("IMG_1001.dng", &out.join("IMG_1001.dng"))?;
    let plan = f.find(a, &out.join("IMG_1001.dng"))?;
    assert!(!plan.picked().unwrap().verified);
    assert_eq!(plan.default_accepted().moves.len(), 1);
    f.apply(&plan, &plan.default_accepted())?;
    assert_eq!(f.path_of(a), out.join("IMG_1001.dng"));
    // Not verified, but the same photo (`Identity::same_photo`): its edit
    // still applies.
    assert!(f.cat.load_edit(a, &out.join("IMG_1001.dng"))?.is_some());
    Ok(())
}

#[test]
fn an_offline_old_location_takes_only_verified_matches() -> Result<()> {
    let mut f = fixture()?;
    let ids: Vec<PhotoId> = (1..=4).map(|n| f.id(&format!("IMG_100{n}.dng"))).collect();
    f.sql(
        "UPDATE photo_checksums SET full_hash=NULL WHERE photo=?",
        ids[2],
    )?;
    f.sql("DELETE FROM photo_checksums WHERE photo=?", ids[3])?;
    let out = f.outside();
    for n in 1..=4 {
        let name = format!("IMG_100{n}.dng");
        f.move_out(&name, &out.join(&name))?;
    }
    // The old volume is offline; the one picked from is there.
    let trip = f.root.clone();
    let online = move |p: &Path| !p.starts_with(&trip);
    let plan = f.find_with(ids[0], &out.join("IMG_1001.dng"), &online)?;
    assert!(plan.picked().unwrap().offline);
    let offered: Vec<PhotoId> = plan.moves.iter().map(|m| m.photo).collect();
    assert_eq!(offered, [ids[0], ids[1]]);
    assert!(plan.moves.iter().all(|m| m.verified && m.offline));
    assert!(plan.choices.is_empty());
    let applied = f.cat.apply_folder_sync(
        &plan,
        &plan.default_accepted(),
        &online,
        None,
        &Default::default(),
    )?;
    assert_eq!(applied.moved.len(), 2);
    // Without a full checksum the file picked can't be taken.
    let refused = f
        .find_with(ids[2], &out.join("IMG_1003.dng"), &online)
        .unwrap_err();
    assert!(
        format!("{refused:#}").contains("drive isn't connected"),
        "{refused:#}"
    );
    assert!(
        f.find_with(ids[3], &out.join("IMG_1004.dng"), &online)
            .is_err()
    );
    Ok(())
}

#[test]
fn a_directory_holding_roots_conflicting_or_ambiguous_is_refused() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    // Above the root.
    let above = f.root.parent().unwrap().join("IMG_1001.dng");
    f.move_out("IMG_1001.dng", &above)?;
    let refused = f.find(a, &above).unwrap_err();
    assert!(format!("{refused:#}").contains("holds folders of the catalog"));
    // Porto's old directory, after Porto was located elsewhere.
    let porto = f
        .cat
        .folders()?
        .into_iter()
        .find(|x| x.relative == "Porto")
        .unwrap()
        .id;
    let apart = f.dir.path().canonicalize()?.join("porto-elsewhere");
    std::fs::create_dir_all(&apart)?;
    f.cat.relink_folder(porto, &apart)?;
    let stale = f.root.join("Porto/IMG_1001.dng");
    std::fs::rename(&above, &stale)?;
    let refused = f.find(a, &stale).unwrap_err();
    assert!(
        format!("{refused:#}").contains("is linked to"),
        "{refused:#}"
    );
    // Two roots located at the same directory.
    let (x, y) = (f.outside().join("x"), f.outside().join("y"));
    for dir in [&x, &y] {
        std::fs::create_dir_all(dir)?;
        std::fs::write(dir.join("keep.dng"), bytes(40))?;
        f.cat.add_folder(dir)?;
    }
    let y_root = f
        .cat
        .roots()?
        .into_iter()
        .find(|r| Path::new(&r.1) == y)
        .unwrap()
        .0;
    f.cat.relink_root(y_root, &x)?;
    std::fs::rename(&stale, x.join("IMG_1001.dng"))?;
    let refused = f.find(a, &x.join("IMG_1001.dng")).unwrap_err();
    assert!(
        format!("{refused:#}").contains("more than one catalog folder"),
        "{refused:#}"
    );
    assert_eq!(f.path_of(a), f.lisbon("IMG_1001.dng"));
    Ok(())
}

#[test]
fn changes_after_the_scan_skip_the_move_and_leave_no_root() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let previous = f.cat.previous_import()?;
    let roots = f.roots();
    let out = f.outside();
    let to = out.join("IMG_1001.dng");
    f.move_out("IMG_1001.dng", &to)?;
    let skipped = |f: &mut Fixture, plan: &SyncPlan| -> Result<()> {
        let applied = f.apply(plan, &plan.default_accepted())?;
        assert!(applied.moved.is_empty(), "{:?}", applied.skipped);
        assert_eq!(applied.skipped.len(), 1);
        Ok(())
    };
    // The file changed.
    let plan = f.find(a, &to)?;
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(120);
    std::fs::File::options()
        .write(true)
        .open(&to)?
        .set_modified(later)?;
    skipped(&mut f, &plan)?;
    // The photo edited.
    let plan = f.find(a, &to)?;
    f.cat.add_keywords(&[a], &[vec!["Late".into()]])?;
    skipped(&mut f, &plan)?;
    // Its copy made master: the photo scanned is another now.
    let copy = f.cat.create_virtual_copy(a)?;
    let plan = f.find(a, &to)?;
    f.cat.set_copy_as_master(copy)?;
    skipped(&mut f, &plan)?;
    assert_eq!(f.roots(), roots);
    assert_eq!(f.cat.previous_import()?, previous);
    // The photo removed from the catalog with its folder.
    let plan = f.find(copy, &to)?;
    let lisbon = f.path_of(copy).parent().unwrap().to_path_buf();
    let folder = f
        .cat
        .folders()?
        .into_iter()
        .find(|x| x.path == lisbon)
        .unwrap()
        .id;
    f.cat.remove_folders(&[folder])?;
    skipped(&mut f, &plan)?;
    assert_eq!(f.roots(), roots);
    // A root or folder location added below the directory meanwhile.
    let porto = f.id("IMG_2001.dng");
    let to = out.join("IMG_2001.dng");
    std::fs::rename(f.root.join("Porto/IMG_2001.dng"), &to)?;
    let plan = f.find(porto, &to)?;
    std::fs::create_dir_all(out.join("sub"))?;
    std::fs::write(out.join("sub/new.dng"), bytes(50))?;
    f.cat.add_folder(&out.join("sub"))?;
    let roots = f.roots();
    skipped(&mut f, &plan)?;
    assert_eq!(f.roots(), roots);
    Ok(())
}

#[test]
fn a_directory_added_as_a_root_after_the_scan_skips_the_move() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let out = f.outside();
    let to = out.join("IMG_1001.dng");
    f.move_out("IMG_1001.dng", &to)?;
    let plan = f.find(a, &to)?;
    // Add Folder of a sibling holding the directory's photos would add
    // them; adding the directory itself makes it a root.
    f.cat.add_folder(&out)?;
    let roots = f.roots();
    let applied = f.apply(&plan, &plan.default_accepted())?;
    assert!(applied.moved.is_empty());
    assert_eq!(f.roots(), roots);
    assert_eq!(f.path_of(a), f.lisbon("IMG_1001.dng"));
    Ok(())
}

#[test]
fn the_photo_open_in_develop_follows_only_a_verified_match() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let copy = f.cat.create_virtual_copy(a)?;
    f.sql("DELETE FROM photo_checksums WHERE photo=?", a)?;
    let out = f.outside();
    let to = out.join("IMG_1001.dng");
    f.move_out("IMG_1001.dng", &to)?;
    let plan = f.find(a, &to)?;
    let roots = f.roots();
    let applied = f.cat.apply_folder_sync(
        &plan,
        &plan.default_accepted(),
        &|_| true,
        Some(copy),
        &Default::default(),
    )?;
    assert!(applied.moved.is_empty());
    assert_eq!(f.roots(), roots);
    Ok(())
}

#[test]
fn a_find_on_disk_plan_takes_no_additions_or_removals() -> Result<()> {
    let mut f = fixture()?;
    let a = f.id("IMG_1001.dng");
    let to = f.outside().join("IMG_1001.dng");
    f.move_out("IMG_1001.dng", &to)?;
    let plan = f.find(a, &to)?;
    let accepted = Accepted {
        removals: vec![a],
        ..Default::default()
    };
    assert!(f.apply(&plan, &accepted).is_err());
    Ok(())
}

#[test]
fn two_files_for_one_photo_are_the_users_choice() -> Result<()> {
    let mut f = fixture()?;
    let (a, b) = (f.id("IMG_1001.dng"), f.id("IMG_1002.dng"));
    let out = f.outside();
    f.move_out("IMG_1001.dng", &out.join("IMG_1001.dng"))?;
    f.move_out("IMG_1002.dng", &out.join("IMG_1002.dng"))?;
    std::fs::copy(out.join("IMG_1002.dng"), out.join("IMG_1002 copy.dng"))?;
    let plan = f.find(a, &out.join("IMG_1001.dng"))?;
    assert_eq!(plan.moves.len(), 1);
    assert_eq!(plan.choices.len(), 1);
    assert!(plan.choices[0].options.iter().all(|m| m.photo == b));
    // Nothing of a choice is ticked; the one picked applies.
    let accepted = Accepted {
        moves: vec![
            (a, out.join("IMG_1001.dng")),
            (b, out.join("IMG_1002 copy.dng")),
        ],
        ..Default::default()
    };
    assert_eq!(plan.default_accepted().moves.len(), 1);
    f.apply(&plan, &accepted)?;
    assert_eq!(f.path_of(b), out.join("IMG_1002 copy.dng"));
    Ok(())
}
