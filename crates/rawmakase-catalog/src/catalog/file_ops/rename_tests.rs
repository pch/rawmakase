//! Renaming photos: what a rename does on disk and in the catalog, which
//! names it refuses (and that one refusal refuses the whole rename), the
//! order a batch numbers photos in, and what a fault at any step leaves.
use super::recover_tests::{before, check};
use super::tests::{Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture, results};
use super::*;
use crate::catalog::{Catalog, HistoryUpdate};
use crate::export_settings::{ExportOptions, NameTemplate, Naming};
use crate::fsops;
use crate::model::recipe::Recipe;
use crate::storage::Identity;
use anyhow::Result;
use rawmakase_cancel::Cancel;

/// Renames each photo (its path below the root) to its stem.
fn rename(fx: &Fx, renames: &[(&str, &str)]) -> FileOp {
    FileOp::RenamePhotos {
        renames: renames
            .iter()
            .map(|(photo, stem)| (fx.photo(photo), stem.to_string()))
            .collect(),
    }
}

fn renamed(name: &str) -> ItemResult {
    ItemResult::Renamed { name: name.into() }
}

/// The reasons of a plan's skipped photos, by photo.
fn reasons(plan: &Plan) -> Vec<(PhotoId, Skip)> {
    let mut out: Vec<_> = plan
        .skipped()
        .iter()
        .map(|s| (s.photo, s.reason.clone()))
        .collect();
    out.sort_by_key(|(p, _)| *p);
    out
}

/// `expected`, in the order `reasons` gives: by photo id, which follows
/// the folder's listing order and so differs between systems.
fn by_id(mut expected: Vec<(PhotoId, Skip)>) -> Vec<(PhotoId, Skip)> {
    expected.sort_by_key(|(p, _)| *p);
    expected
}

fn refused(r: Refusal) -> Skip {
    Skip::Refused(r)
}

/// The names in the root folder, sorted.
fn listing(fx: &Fx) -> Vec<String> {
    let mut names: Vec<String> = fsops::list(&fx.root)
        .unwrap()
        .into_iter()
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Whether the root's volume renames a name to another spelling of
/// itself, and whether the root tells case apart.
fn volume(fx: &Fx) -> (bool, fsops::Folding) {
    let facts = fsops::volume_facts(&fx.root).unwrap();
    let rules = fsops::probe(&fx.root, &fsops::list(&fx.root).unwrap());
    (facts.renames_spelling, rules.case)
}

#[test]
fn a_rename_keeps_the_file_object_its_time_identity_edit_and_checksum() -> Result<()> {
    let mut fx = fixture(&[
        "a.NEF",
        "a.NEF.xmp",
        "a.xmp",
        "a.NEF.rawmakase.json",
        "b.NEF",
    ]);
    let a = fx.photo("a.NEF");
    let copy = fx.cat.create_virtual_copy(a)?;
    let source = fx.root.join("a.NEF");
    fx.cat.save_edit(
        a,
        &source,
        &crate::storage::Identity::read(&source)?,
        &Recipe::default(),
        &ExportOptions::default(),
        HistoryUpdate::Keep,
    )?;
    let read = crate::storage::FileChecksum::read_full(&source, |_| true)?;
    let crate::storage::ChecksumRead::Done(checksum) = read else {
        panic!("not read")
    };
    fx.cat.save_checksums(&[crate::catalog::ReadChecksum {
        photo: a,
        folder: fx.folder(""),
        filename: "a.NEF".into(),
        path: source.clone(),
        checksum,
    }])?;
    let (id, identity) = (fsops::entry(&source)?.id, Identity::read(&source)?);
    let modified = std::fs::metadata(&source)?.modified()?;
    let plan = fx.plan(rename(&fx, &[("a.NEF", "Trip-001")]));
    assert_eq!(plan.summary().files, 1);
    assert_eq!(plan.summary().companions, 3);
    assert_eq!(plan.summary().photos, 2);
    assert_eq!(
        plan.summary().renames,
        [("a.NEF".to_string(), "Trip-001.NEF".to_string())]
    );
    assert!(source.exists(), "planning renames nothing");
    let outcome = fx.run(
        rename(&fx, &[("a.NEF", "Trip-001")]),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [renamed("Trip-001.NEF")]);
    assert_eq!(
        listing(&fx),
        [
            "Trip-001.NEF",
            "Trip-001.NEF.rawmakase.json",
            "Trip-001.NEF.xmp",
            "Trip-001.xmp",
            "b.NEF"
        ]
    );
    let target = fx.root.join("Trip-001.NEF");
    assert_eq!(fsops::entry(&target)?.id, id);
    assert_eq!(Identity::read(&target)?, identity);
    assert_eq!(std::fs::metadata(&target)?.modified()?, modified);
    assert_eq!(std::fs::read(fx.root.join("Trip-001.xmp"))?, bytes("a.xmp"));
    assert_eq!(fx.rows()[&a], "Trip-001.NEF");
    assert_eq!(fx.rows()[&copy], "Trip-001.NEF");
    assert!(
        fx.cat.load_edit(a, &target)?.is_some(),
        "the edit still opens"
    );
    assert_eq!(fx.cat.checksums()?[&a].identity, identity);
    Ok(())
}

/// Linux's NTFS drivers (ntfs3 and `ntfs`) keep file ids and rename
/// exclusively, so photos on an NTFS drive rename as on ext4 (#914).
#[test]
#[ignore = "needs RAWMAKASE_TEST_NTFS_VOLUME: a writable folder on an NTFS volume"]
fn photos_on_an_ntfs_volume_rename() -> Result<()> {
    let Some(volume) = std::env::var_os("RAWMAKASE_TEST_NTFS_VOLUME") else {
        return Ok(());
    };
    // The catalog stays on this computer's disk; only the photos' folder
    // is on the NTFS drive.
    let mut fx = fixture(&[]);
    let scratch = tempfile::tempdir_in(volume)?;
    fx.root = scratch.path().canonicalize()?.join("trip");
    std::fs::create_dir(&fx.root)?;
    for f in ["DSC00001.ARW", "DSC00001.JPG", "DSC00002.ARW"] {
        std::fs::write(fx.root.join(f), bytes(f))?;
    }
    fx.cat.add_folder(&fx.root)?;
    let id = fsops::entry(&fx.root.join("DSC00001.ARW"))?.id;
    let op = rename(
        &fx,
        &[("DSC00001.ARW", "Trip-1"), ("DSC00002.ARW", "Trip-2")],
    );
    let plan = fx.plan(op.clone());
    assert_eq!(reasons(&plan), [], "{:?}", plan.skipped());
    let outcome = fx.run(op, &NoTrash, &disk::Real);
    assert_eq!(
        results(&outcome),
        [renamed("Trip-1.ARW"), renamed("Trip-2.ARW")]
    );
    assert_eq!(listing(&fx), ["DSC00001.JPG", "Trip-1.ARW", "Trip-2.ARW"]);
    assert_eq!(fsops::entry(&fx.root.join("Trip-1.ARW"))?.id, id);
    Ok(())
}

#[test]
fn companions_keep_their_endings_case_and_a_shared_one_stays() -> Result<()> {
    // A.NEF.xmp is A.NEF's and A.NEF.DNG's (its stem sidecar).
    let mut fx = fixture(&[
        "A.NEF",
        "A.NEF.DNG",
        "A.NEF.xmp",
        "c.NEF",
        "c.NEF.XMP",
        "c.XMP",
    ]);
    let op = rename(&fx, &[("A.NEF", "B"), ("c.NEF", "d")]);
    let plan = fx.plan(op.clone());
    assert_eq!(
        plan.summary().shared_companions,
        [fx.root.join("A.NEF.xmp")]
    );
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(outcome.items.len(), 2);
    assert_eq!(
        listing(&fx),
        [
            "A.NEF.DNG",
            "A.NEF.xmp",
            "B.NEF",
            "d.NEF",
            "d.NEF.XMP",
            "d.XMP"
        ]
    );
    Ok(())
}

#[test]
fn a_companion_in_another_case_is_renamed_to_the_photos_name() {
    let name = |photo: &str, new: &str, companion: &str| {
        companions::renamed(photo.as_ref(), new.as_ref(), companion.as_ref())
            .map(|n| n.to_string_lossy().into_owned())
    };
    assert_eq!(
        name("A.NEF", "B.NEF", "a.nef.Xmp").as_deref(),
        Some("B.NEF.Xmp")
    );
    assert_eq!(name("A.NEF", "B.NEF", "a.XMP").as_deref(), Some("B.XMP"));
    assert_eq!(
        name("A.NEF", "B.NEF", "A.NEF.rawmakase-local.json").as_deref(),
        Some("B.NEF.rawmakase-local.json")
    );
    // A JPEG has no stem sidecar; nothing else is its companion.
    assert_eq!(name("A.JPG", "B.JPG", "A.xmp"), None);
    assert_eq!(name("A.NEF", "B.NEF", "C.xmp"), None);
}

#[test]
fn names_a_file_cant_have_refuse_the_whole_rename() {
    let fx = fixture(&["a.NEF", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let long = "x".repeat(252);
    for (stem, problem) in [
        ("", NameProblem::Empty),
        (" ", NameProblem::Empty),
        ("..", NameProblem::Dots),
        ("a/b", NameProblem::Separator),
        ("a\\b", NameProblem::Separator),
        ("a\0b", NameProblem::Control),
        ("a:b", NameProblem::Windows),
        ("x ", NameProblem::Trailing),
        ("CON", NameProblem::Device),
        ("lpt1", NameProblem::Device),
        // With ".NEF", over 255 bytes.
        (&long, NameProblem::Long),
    ] {
        if problem != NameProblem::Long {
            assert_eq!(photo_name_problem(stem), Some(problem), "{stem:?}");
        }
        let plan = fx.plan(FileOp::RenamePhotos {
            renames: vec![(a, stem.into()), (b, "fine".into())],
        });
        assert!(plan.is_empty(), "{stem:?}");
        assert_eq!(
            reasons(&plan),
            by_id(vec![
                (a, refused(Refusal::BadName(problem))),
                (b, Skip::Blocked)
            ]),
            "{stem:?}"
        );
    }
    assert_eq!(photo_name_problem("Trip – ąę 2024"), None);
}

#[test]
fn the_extension_is_kept_and_the_new_name_is_all_stem() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let outcome = fx.run(
        rename(&fx, &[("a.NEF", "b.JPG")]),
        &NoTrash,
        &Faulty::default(),
    );
    assert_eq!(results(&outcome), [renamed("b.JPG.NEF")]);
    assert_eq!(listing(&fx), ["b.JPG.NEF"]);
    Ok(())
}

#[test]
fn a_name_taken_on_disk_or_by_an_offline_photo_refuses_the_whole_rename() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF", "c.NEF"]);
    let (a, b, c) = (fx.photo("a.NEF"), fx.photo("b.NEF"), fx.photo("c.NEF"));
    // A file the catalog doesn't have, and a photo whose file is gone.
    std::fs::write(fx.root.join("x.NEF"), b"not in the catalog")?;
    let root = fx.root.clone();
    let offline = fx.add_row(&root, "", "off.NEF");
    let plan = fx.plan(rename(&fx, &[("a.NEF", "x"), ("b.NEF", "fine")]));
    assert_eq!(
        reasons(&plan),
        by_id(vec![
            (a, refused(Refusal::NameTaken("x.NEF".into()))),
            (b, Skip::Blocked)
        ])
    );
    // The offline photo holds its name, in any case.
    for stem in ["off", "OFF"] {
        let plan = fx.plan(rename(&fx, &[("b.NEF", stem)]));
        assert_eq!(
            reasons(&plan),
            [(b, refused(Refusal::NameTaken(format!("{stem}.NEF"))))]
        );
    }
    // On disk, compared as the folder compares names.
    let plan = fx.plan(rename(&fx, &[("c.NEF", "X")]));
    let (_, case) = volume(&fx);
    if case == fsops::Folding::Exact {
        assert!(plan.skipped().is_empty());
    } else {
        assert_eq!(
            reasons(&plan),
            [(c, refused(Refusal::NameTaken("X.NEF".into())))]
        );
    }
    assert!(fx.rows().contains_key(&offline));
    assert_eq!(std::fs::read(fx.root.join("x.NEF"))?, b"not in the catalog");
    Ok(())
}

#[cfg(target_vendor = "apple")]
#[test]
fn an_nfd_name_on_disk_takes_its_nfc_spelling() -> Result<()> {
    let fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    std::fs::write(fx.root.join("cafe\u{301}.NEF"), b"not in the catalog")?;
    let plan = fx.plan(FileOp::RenamePhotos {
        renames: vec![(a, "caf\u{e9}".into())],
    });
    assert_eq!(
        reasons(&plan),
        [(a, refused(Refusal::NameTaken("caf\u{e9}.NEF".into())))]
    );
    Ok(())
}

#[test]
fn one_name_given_twice_or_one_photo_given_two_names_is_refused() -> Result<()> {
    let fx = fixture(&["a.NEF", "b.NEF", "c.NEF"]);
    let (a, b, c) = (fx.photo("a.NEF"), fx.photo("b.NEF"), fx.photo("c.NEF"));
    let plan = fx.plan(rename(
        &fx,
        &[("a.NEF", "x"), ("b.NEF", "X"), ("c.NEF", "y")],
    ));
    assert!(plan.is_empty());
    assert_eq!(
        reasons(&plan),
        by_id(vec![
            (a, refused(Refusal::NameTwice("x.NEF".into()))),
            (b, refused(Refusal::NameTwice("X.NEF".into()))),
            (c, Skip::Blocked)
        ])
    );
    let plan = fx.plan(rename(&fx, &[("a.NEF", "x"), ("a.NEF", "y")]));
    assert_eq!(reasons(&plan), [(a, refused(Refusal::Twice))]);
    // The same name twice is one rename.
    let plan = fx.plan(rename(&fx, &[("a.NEF", "x"), ("a.NEF", "x")]));
    assert!(plan.skipped().is_empty());
    assert_eq!(plan.steps(), 1);
    Ok(())
}

#[test]
fn chains_and_cycles_are_refused() -> Result<()> {
    let fx = fixture(&["a.NEF", "b.NEF", "c.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    // a → b while b → a.
    let plan = fx.plan(rename(&fx, &[("a.NEF", "b"), ("b.NEF", "a")]));
    assert!(plan.is_empty());
    assert_eq!(
        reasons(&plan),
        by_id(vec![
            (a, refused(Refusal::Chain("b.NEF".into()))),
            (b, refused(Refusal::Chain("a.NEF".into())))
        ])
    );
    // a → b, the name b frees by its own rename.
    let plan = fx.plan(rename(&fx, &[("a.NEF", "B"), ("b.NEF", "d")]));
    assert_eq!(
        reasons(&plan),
        by_id(vec![
            (a, refused(Refusal::Chain("B.NEF".into()))),
            (b, Skip::Blocked)
        ])
    );
    // And through a companion: b's sidecar would be x.xmp, which is there.
    let fx = fixture(&["b.NEF", "b.xmp", "x.xmp"]);
    let b = fx.photo("b.NEF");
    let plan = fx.plan(rename(&fx, &[("b.NEF", "x")]));
    assert_eq!(
        reasons(&plan),
        [(b, refused(Refusal::NameTaken("x.xmp".into())))]
    );
    Ok(())
}

#[test]
fn a_case_only_rename_works_where_the_volume_renames_spelling() -> Result<()> {
    let mut fx = fixture(&["ROUND.NEF", "ROUND.xmp"]);
    let a = fx.photo("ROUND.NEF");
    let id = fsops::entry(&fx.root.join("ROUND.NEF"))?.id;
    let (spelling, case) = volume(&fx);
    let op = rename(&fx, &[("ROUND.NEF", "round")]);
    let plan = fx.plan(op.clone());
    if !spelling && case != fsops::Folding::Exact {
        assert_eq!(reasons(&plan), [(a, refused(Refusal::SpellingOnly))]);
        return Ok(());
    }
    assert!(plan.skipped().is_empty(), "{:?}", plan.skipped());
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [renamed("round.NEF")]);
    assert_eq!(listing(&fx), ["round.NEF", "round.xmp"]);
    assert_eq!(fsops::entry(&fx.root.join("round.NEF"))?.id, id);
    assert_eq!(fx.rows()[&a], "round.NEF");
    Ok(())
}

#[test]
fn an_accents_only_rename_works_where_the_volume_renames_spelling() -> Result<()> {
    // Where the volume doesn't rename a name to another spelling of itself,
    // a folder that folds names is asked nothing it can't do: case-sensitive
    // APFS takes the two spellings for one name, and a folder that folds case
    // (NTFS) is compared caselessly, which folds accents too. Only a folder
    // telling both apart (ext4) renames.
    let mut fx = fixture(&["caf\u{e9}.NEF"]);
    let a = fx.photo("caf\u{e9}.NEF");
    let spelling = volume(&fx).0;
    let rules = fsops::probe(&fx.root, &fsops::list(&fx.root)?);
    let exact = rules.case == fsops::Folding::Exact && rules.normalization == fsops::Folding::Exact;
    let op = FileOp::RenamePhotos {
        renames: vec![(a, "cafe\u{301}".into())],
    };
    if !spelling && !exact {
        assert_eq!(reasons(&fx.plan(op)), [(a, refused(Refusal::SpellingOnly))]);
        assert_eq!(listing(&fx), ["caf\u{e9}.NEF"]);
        return Ok(());
    }
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [renamed("cafe\u{301}.NEF")]);
    assert_eq!(listing(&fx), ["cafe\u{301}.NEF"]);
    assert_eq!(fx.rows()[&a], "cafe\u{301}.NEF");
    Ok(())
}

#[test]
fn a_photo_with_its_name_already_is_left_alone_and_the_rest_renamed() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let op = rename(&fx, &[("a.NEF", "a"), ("b.NEF", "c")]);
    assert_eq!(reasons(&fx.plan(op.clone())), [(a, Skip::SameName)]);
    let outcome = fx.run(op, &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [renamed("c.NEF")]);
    assert_eq!(fx.rows()[&b], "c.NEF");
    Ok(())
}

#[test]
fn a_copy_alone_or_a_missing_file_refuses_the_whole_rename() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "b.NEF", "c.NEF"]);
    let (a, b, c) = (fx.photo("a.NEF"), fx.photo("b.NEF"), fx.photo("c.NEF"));
    let copy = fx.cat.create_virtual_copy(a)?;
    let plan = fx.plan(FileOp::RenamePhotos {
        renames: vec![(copy, "x".into()), (b, "y".into())],
    });
    assert_eq!(
        reasons(&plan),
        by_id(vec![(b, Skip::Blocked), (copy, Skip::CopyOnly)])
    );
    std::fs::remove_file(fx.root.join("c.NEF"))?;
    let plan = fx.plan(rename(&fx, &[("b.NEF", "y"), ("c.NEF", "z")]));
    assert_eq!(
        reasons(&plan),
        by_id(vec![(b, Skip::Blocked), (c, Skip::Missing)])
    );
    // A master given with its copy takes it along.
    let plan = fx.plan(FileOp::RenamePhotos {
        renames: vec![(a, "x".into()), (copy, "ignored".into())],
    });
    assert!(plan.skipped().is_empty());
    assert_eq!(plan.photos().len(), 2);
    Ok(())
}

#[test]
fn a_name_taken_between_plan_and_execute_is_left_alone() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    for raced in ["b.NEF", "b.xmp"] {
        let plan = fx.plan(rename(&fx, &[("a.NEF", "b")]));
        std::fs::write(fx.root.join(raced), b"raced")?;
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        assert!(
            matches!(results(&outcome)[..], [ItemResult::Unchanged(Some(_))]),
            "{raced}: {outcome:?}"
        );
        assert_eq!(std::fs::read(fx.root.join(raced))?, b"raced");
        assert_eq!(std::fs::read(fx.root.join("a.NEF"))?, bytes("a.NEF"));
        assert_eq!(fx.rows()[&a], "a.NEF");
        std::fs::remove_file(fx.root.join(raced))?;
    }
    Ok(())
}

#[test]
fn a_name_taken_just_before_its_rename_is_never_replaced() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    struct Racing<'a>(Faulty, &'a std::path::Path);
    impl disk::Disk for Racing<'_> {
        fn entry(&self, p: &std::path::Path) -> Result<fsops::Entry, fsops::FsError> {
            self.0.entry(p)
        }
        fn list(&self, d: &std::path::Path) -> Result<Vec<std::ffi::OsString>, fsops::FsError> {
            self.0.list(d)
        }
        fn identity(&self, p: &std::path::Path) -> anyhow::Result<Identity> {
            self.0.identity(p)
        }
        fn settle_rename(
            &self,
            a: &std::path::Path,
            b: &std::path::Path,
        ) -> Result<(), fsops::FsError> {
            self.0.settle_rename(a, b)
        }
        fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> Result<(), fsops::FsError> {
            if b == self.1 {
                std::fs::write(self.1, b"raced").unwrap();
            }
            self.0.rename(a, b)
        }
        fn create_dir(&self, p: &std::path::Path) -> Result<(), fsops::FsError> {
            self.0.create_dir(p)
        }
        fn sync_dir(&self, d: &std::path::Path) -> Result<(), fsops::FsError> {
            self.0.sync_dir(d)
        }
        fn canonical(&self, p: &std::path::Path) -> Result<std::path::PathBuf, fsops::FsError> {
            self.0.canonical(p)
        }
        tests::passes_on!(0);
    }
    let raced = fx.root.join("b.xmp");
    let outcome = fx.run(
        rename(&fx, &[("a.NEF", "b")]),
        &NoTrash,
        &Racing(Faulty::default(), &raced),
    );
    assert!(matches!(
        &results(&outcome)[..],
        [ItemResult::Unchanged(Some(_))]
    ));
    assert_eq!(std::fs::read(&raced)?, b"raced");
    assert_eq!(
        std::fs::read(fx.root.join("a.NEF"))?,
        bytes("a.NEF"),
        "put back"
    );
    assert_eq!(std::fs::read(fx.root.join("a.xmp"))?, bytes("a.xmp"));
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

#[test]
fn a_name_the_disk_spells_otherwise_is_journalled_as_the_disk_spells_it() -> Result<()> {
    // A volume that stores names decomposed (as HFS+ does): asked for
    // "café.NEF", it names the file "cafe\u{301}.NEF".
    struct Decomposing(Faulty);
    impl disk::Disk for Decomposing {
        fn entry(&self, p: &std::path::Path) -> Result<fsops::Entry, fsops::FsError> {
            self.0.entry(p)
        }
        fn list(&self, d: &std::path::Path) -> Result<Vec<std::ffi::OsString>, fsops::FsError> {
            self.0.list(d)
        }
        fn identity(&self, p: &std::path::Path) -> anyhow::Result<Identity> {
            self.0.identity(p)
        }
        fn settle_rename(
            &self,
            a: &std::path::Path,
            b: &std::path::Path,
        ) -> Result<(), fsops::FsError> {
            self.0.settle_rename(a, b)
        }
        fn rename(&self, a: &std::path::Path, b: &std::path::Path) -> Result<(), fsops::FsError> {
            use unicode_normalization::UnicodeNormalization;
            let name: String = b.file_name().unwrap().to_string_lossy().nfd().collect();
            self.0.rename(a, &b.with_file_name(name))
        }
        fn create_dir(&self, p: &std::path::Path) -> Result<(), fsops::FsError> {
            self.0.create_dir(p)
        }
        fn sync_dir(&self, d: &std::path::Path) -> Result<(), fsops::FsError> {
            self.0.sync_dir(d)
        }
        fn canonical(&self, p: &std::path::Path) -> Result<std::path::PathBuf, fsops::FsError> {
            self.0.canonical(p)
        }
        tests::passes_on!(0);
    }
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let outcome = fx.run(
        FileOp::RenamePhotos {
            renames: vec![(a, "caf\u{e9}".into())],
        },
        &NoTrash,
        &Decomposing(Faulty::default()),
    );
    assert_eq!(results(&outcome), [renamed("cafe\u{301}.NEF")]);
    assert_eq!(listing(&fx), ["cafe\u{301}.NEF", "cafe\u{301}.xmp"]);
    assert_eq!(fx.rows()[&a], "cafe\u{301}.NEF");
    Ok(())
}

#[test]
fn a_photo_changed_by_another_connection_after_planning_is_left_alone() -> Result<()> {
    let mut fx = fixture(&["a.NEF"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(rename(&fx, &[("a.NEF", "b")]));
    let mut other = Catalog::open(&fx.catalog)?;
    other.set_metadata(a, 3, 0, "")?;
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert_eq!(
        results(&outcome),
        [ItemResult::Unchanged(Some(preconditions::CHANGED.into()))]
    );
    assert_eq!(listing(&fx), ["a.NEF"]);
    Ok(())
}

#[test]
fn a_sidecar_another_file_would_read_is_refused() -> Result<()> {
    // b.DNG would read a's sidecar, renamed b.xmp.
    let fx = fixture(&["a.NEF", "a.xmp", "b.DNG"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(rename(&fx, &[("a.NEF", "b")]));
    assert_eq!(
        reasons(&plan),
        [(a, refused(Refusal::CompanionClash("b.DNG".into())))]
    );
    // The photo would read a sidecar already there as its own.
    let fx = fixture(&["a.NEF", "b.xmp"]);
    let a = fx.photo("a.NEF");
    let plan = fx.plan(rename(&fx, &[("a.NEF", "b")]));
    assert_eq!(
        reasons(&plan),
        [(a, refused(Refusal::CompanionClash("b.xmp".into())))]
    );
    Ok(())
}

fn rename_files() -> Fx {
    fixture(&["a.NEF", "a.xmp", "b.NEF"])
}

fn rename_op(fx: &Fx) -> FileOp {
    rename(fx, &[("a.NEF", "x"), ("b.NEF", "y")])
}

/// Plans `op`, runs it on `disk` (which may crash), restarts and recovers.
fn scenario(fx: &mut Fx, op: FileOp, disk: &Faulty) -> (BatchId, Recovery) {
    scenario_on(fx, op, disk, &disk::Real)
}

/// [`scenario`], recovering on `recover_on`.
fn scenario_on(
    fx: &mut Fx,
    op: FileOp,
    disk: &Faulty,
    recover_on: &dyn disk::Disk,
) -> (BatchId, Recovery) {
    let plan = fx.plan(op);
    let batch = plan.batch().clone();
    crashes(|| {
        drop(
            execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, disk)
                .unwrap(),
        );
    });
    fx.reopen();
    let recovery = fx.recover(recover_on);
    (batch, recovery)
}

#[test]
fn every_fault_at_every_disk_step_of_a_rename_ends_proven() {
    let mut clean = rename_files();
    let counting = Faulty::default();
    let op = rename_op(&clean);
    let _ = clean.run(op, &NoTrash, &counting);
    let steps = [
        (Op::Rename, counting.count(Op::Rename)),
        (Op::Sync, counting.count(Op::Sync)),
    ];
    assert_eq!(steps[0].1, 3);
    // Their folder synced once for each round of renames: the photos, then
    // the sidecar (#692).
    assert_eq!(steps[1].1, 2);
    let mut runs = 0;
    for (at, count) in steps {
        for nth in 0..count {
            for fault in [Fault::Fail, Fault::CrashBefore, Fault::CrashAfter] {
                let mut fx = rename_files();
                let was = before(&fx);
                let disk = Faulty::at(at, nth, fault);
                let op = rename_op(&fx);
                let (batch, recovery) = scenario(&mut fx, op, &disk);
                check(&fx, &was, &recovery, &batch);
                for u in &recovery.unfinished {
                    let back =
                        resolve(&mut fx.cat, u.operation, Resolution::PutBack, &NoTrash).unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{at:?} {nth} {fault:?}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                // Every item is renamed whole, or not at all.
                let names = listing(&fx);
                let a = names.contains(&"a.NEF".to_string()) && names.contains(&"a.xmp".into());
                let x = names.contains(&"x.NEF".to_string()) && names.contains(&"x.xmp".into());
                assert!(a != x, "{at:?} {nth} {fault:?}: {names:?}");
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 3 * (3 + 2));
}

/// A rename by placeholder, as on ntfs-3g (#914), cut short by any fault
/// at any rename, before, once its placeholder is made, or after, ends
/// proven: every file at one name, no placeholder left, recovered or put
/// back, and the photos rename again.
#[test]
fn every_fault_at_every_step_of_a_rename_by_placeholder_ends_proven() {
    let mut runs = 0;
    // A failed rename whose placeholder can't be removed either, then.
    for cleanup_fails in [false, true] {
        for nth in 0..3 {
            for fault in [
                Fault::Fail,
                Fault::CrashBefore,
                Fault::CrashMid,
                Fault::CrashAfter,
            ] {
                if cleanup_fails && fault != Fault::Fail {
                    continue;
                }
                let shown = format!("{nth} {fault:?}, cleanup fails: {cleanup_fails}");
                let mut fx = rename_files();
                let was = before(&fx);
                let disk = Faulty {
                    by_placeholder: true,
                    cleanup_fails,
                    ..Faulty::at(Op::Rename, nth, fault)
                };
                // Recovered where renames are by placeholder.
                let there = Faulty {
                    by_placeholder: true,
                    ..Faulty::default()
                };
                let op = rename_op(&fx);
                let (batch, recovery) = scenario_on(&mut fx, op, &disk, &there);
                check(&fx, &was, &recovery, &batch);
                for u in &recovery.unfinished {
                    let back = recover::resolve_with(
                        &mut fx.cat,
                        u.operation,
                        Resolution::PutBack,
                        &there,
                        &NoTrash,
                    )
                    .unwrap();
                    assert!(
                        matches!(back.result, ItemResult::Unchanged(_)),
                        "{shown}: {back:?}"
                    );
                }
                check(&fx, &was, &Recovery::default(), &batch);
                let names = listing(&fx);
                assert!(
                    names.iter().all(|n| !n.starts_with('.')),
                    "{shown}: {names:?}"
                );
                for n in &names {
                    let links = fsops::entry(&fx.root.join(n)).unwrap().links;
                    assert_eq!(links, 1, "{shown}: {n}");
                }
                let a = names.contains(&"a.NEF".to_string()) && names.contains(&"a.xmp".into());
                let x = names.contains(&"x.NEF".to_string()) && names.contains(&"x.xmp".into());
                assert!(a != x, "{shown}: {names:?}");
                // Whichever names they have, they rename again.
                let p = if a { "a.NEF" } else { "x.NEF" };
                let q = if names.contains(&"b.NEF".to_string()) {
                    "b.NEF"
                } else {
                    "y.NEF"
                };
                let again = rename(&fx, &[(p, "p"), (q, "q")]);
                let outcome = fx.run(again, &NoTrash, &Faulty::default());
                let mut done = results(&outcome);
                done.sort_by_key(|r| format!("{r:?}"));
                assert_eq!(done, [renamed("p.NEF"), renamed("q.NEF")], "{shown}");
                runs += 1;
            }
        }
    }
    assert_eq!(runs, 3 * 4 + 3);
}

/// A rename by placeholder that fails as its drive goes away isn't taken
/// for put back: what it left there can't be told, so it stays for
/// recovery, which finds the file where it was once the drive is back
/// (#914 review).
#[test]
fn a_rename_by_placeholder_failing_with_its_drive_gone_stays_for_recovery() -> Result<()> {
    let mut fx = rename_files();
    let away = fx.dir.path().join("away");
    let disk = Faulty {
        by_placeholder: true,
        gone_on_fail: Some((fx.root.clone(), away.clone())),
        ..Faulty::at(Op::Rename, 0, Fault::Fail)
    };
    let plan = fx.plan(rename(&fx, &[("a.NEF", "x")]));
    let batch = plan.batch().clone();
    let outcome =
        execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk)?;
    assert!(
        matches!(results(&outcome)[..], [ItemResult::NeedsRecovery(_)]),
        "{outcome:?}"
    );
    // Back, with the placeholder.
    std::fs::rename(&away, &fx.root)?;
    assert_eq!(listing(&fx), ["a.NEF", "a.xmp", "b.NEF", "x.NEF"]);
    let there = Faulty {
        by_placeholder: true,
        ..Faulty::default()
    };
    let recovery = fx.recover(&there);
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let items = super::outcome(&fx.cat, &batch)?.items;
    assert!(
        matches!(items[0].result, ItemResult::Unchanged(_)),
        "{items:?}"
    );
    assert_eq!(listing(&fx), ["a.NEF", "a.xmp", "b.NEF"]);
    Ok(())
}

#[test]
fn every_failed_commit_of_a_rename_ends_proven_and_a_restart_finishes_it() -> Result<()> {
    for when in [
        "BEFORE INSERT ON file_operations",
        "BEFORE UPDATE ON file_operation_members WHEN NEW.state = 'moved'",
        "BEFORE UPDATE ON photos",
        "BEFORE UPDATE ON file_operations WHEN NEW.state = 'done'",
    ] {
        let mut fx = rename_files();
        let was = before(&fx);
        fx.fail_journal(when);
        let plan = fx.plan(rename_op(&fx));
        let batch = plan.batch().clone();
        let outcome = execute::execute_with(
            &mut fx.cat,
            plan.confirm(),
            &NoTrash,
            Cancel::NEVER,
            &Faulty::default(),
        )?;
        let first = outcome.items.first().map(|i| i.result.clone());
        match when {
            w if w.contains("INSERT") => assert!(outcome.items.is_empty()),
            w if w.contains("members") => assert!(
                matches!(first, Some(ItemResult::Unchanged(Some(_)))),
                "{first:?}"
            ),
            _ => assert!(
                matches!(first, Some(ItemResult::NeedsRecovery(_))),
                "{first:?}"
            ),
        }
        fx.reopen();
        let recovery = fx.recover(&disk::Real);
        assert!(recovery.unfinished.is_empty(), "{when}: {recovery:?}");
        check(&fx, &was, &recovery, &batch);
    }
    Ok(())
}

/// Renames a.NEF (with a.xmp) to x, leaving the item unfinished with both
/// files renamed: its rows didn't follow.
fn renamed_but_unfinished() -> (Fx, BatchId) {
    let mut fx = rename_files();
    fx.fail_journal("BEFORE UPDATE ON photos");
    let plan = fx.plan(rename(&fx, &[("a.NEF", "x")]));
    let batch = plan.batch().clone();
    let outcome = execute::execute_with(
        &mut fx.cat,
        plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )
    .unwrap();
    assert!(matches!(
        results(&outcome)[..],
        [ItemResult::NeedsRecovery(_)]
    ));
    fx.reopen();
    (fx, batch)
}

#[test]
fn a_rename_whose_rows_did_not_follow_is_completed_by_recovery() -> Result<()> {
    let (mut fx, batch) = renamed_but_unfinished();
    let a = fx.photo("a.NEF");
    let recovery = fx.recover(&disk::Real);
    assert!(recovery.unfinished.is_empty());
    assert_eq!(
        results(&super::outcome(&fx.cat, &batch)?),
        [renamed("x.NEF")]
    );
    assert_eq!(fx.rows()[&a], "x.NEF");
    Ok(())
}

#[test]
fn a_rename_changed_in_the_catalog_before_recovery_is_reported_and_left() -> Result<()> {
    let (mut fx, _) = renamed_but_unfinished();
    let a = fx.photo("a.NEF");
    let mut other = Catalog::open(&fx.catalog)?;
    other.set_metadata(a, 2, 0, "")?;
    let recovery = fx.recover(&disk::Real);
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(recovery.unfinished[0].reason, preconditions::CHANGED);
    assert_eq!(fx.rows()[&a], "a.NEF");
    assert!(fx.root.join("x.NEF").is_file(), "files untouched");
    Ok(())
}

#[test]
fn a_case_only_rename_cut_short_is_decided_by_the_listing() -> Result<()> {
    for fault in [Fault::CrashBefore, Fault::CrashAfter] {
        let mut fx = fixture(&["ROUND.NEF"]);
        if !volume(&fx).0 {
            return Ok(());
        }
        let a = fx.photo("ROUND.NEF");
        let disk = Faulty::at(Op::Rename, 0, fault);
        let op = rename(&fx, &[("ROUND.NEF", "round")]);
        let (batch, recovery) = scenario(&mut fx, op, &disk);
        assert!(recovery.unfinished.is_empty(), "{fault:?}: {recovery:?}");
        let result = results(&super::outcome(&fx.cat, &batch)?);
        match fault {
            Fault::CrashAfter => {
                assert_eq!(result, [renamed("round.NEF")]);
                assert_eq!(listing(&fx), ["round.NEF"]);
                assert_eq!(fx.rows()[&a], "round.NEF");
            }
            _ => {
                assert_eq!(result, [ItemResult::Unchanged(None)]);
                assert_eq!(listing(&fx), ["ROUND.NEF"]);
                assert_eq!(fx.rows()[&a], "ROUND.NEF");
            }
        }
    }
    Ok(())
}

/// A small TIFF taken at `original` ("2021:06:06 10:00:00").
fn dated_tiff(original: &str) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend(b"II");
    out.extend(42u16.to_le_bytes());
    out.extend(8u32.to_le_bytes());
    // The main directory: one entry, the EXIF directory's offset.
    out.extend(1u16.to_le_bytes());
    out.extend(0x8769u16.to_le_bytes());
    out.extend(4u16.to_le_bytes());
    out.extend(1u32.to_le_bytes());
    out.extend(26u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    // The EXIF directory: DateTimeOriginal.
    out.extend(1u16.to_le_bytes());
    out.extend(0x9003u16.to_le_bytes());
    out.extend(2u16.to_le_bytes());
    out.extend(20u32.to_le_bytes());
    out.extend(44u32.to_le_bytes());
    out.extend(0u32.to_le_bytes());
    out.extend(original.as_bytes());
    out.push(0);
    out
}

#[test]
fn a_batch_is_numbered_by_capture_time_then_undated_by_filename() -> Result<()> {
    let fx = fixture(&[
        "z.NEF",
        "m.NEF",
        "late.NEF",
        "early.NEF",
        "tie1.NEF",
        "tie2.NEF",
        "file.tif",
    ]);
    // The catalog's times; a file's own is not read (#681): the catalog's
    // background reads fill it in.
    let set = |name: &str, at: &str| {
        fx.cat
            .db_for_tests()
            .execute(
                "UPDATE photos SET captured=?1 WHERE id=?2",
                rusqlite::params![at, fx.photo(name).0],
            )
            .unwrap();
    };
    set("late.NEF", "2021-06-06T12:00:00.000");
    set("early.NEF", "2021-06-06T08:00:00.000");
    set("tie1.NEF", "2021-06-06T11:00:00.000");
    set("tie2.NEF", "2021-06-06T11:00:00.000");
    std::fs::write(fx.root.join("file.tif"), dated_tiff("2021:06:06 10:00:00"))?;
    let mut cat = fx.cat;
    let given: Vec<PhotoId> = [
        "z.NEF",
        "tie2.NEF",
        "late.NEF",
        "m.NEF",
        "file.tif",
        "tie1.NEF",
        "early.NEF",
    ]
    .iter()
    .map(|n| {
        cat.photos()
            .unwrap()
            .into_iter()
            .find(|p| p.path.ends_with(n))
            .unwrap()
            .id
    })
    .collect();
    let copy = cat.create_virtual_copy(given[0])?;
    let mut with_copy = given;
    with_copy.push(copy);
    let order = rename_order(&cat, &with_copy).map_err(|e| anyhow::anyhow!("{e}"))?;
    let names: Vec<(&str, Option<&str>)> = order
        .photos()
        .iter()
        .map(|o| (o.filename.as_str(), o.captured.as_deref()))
        .collect();
    assert_eq!(
        names,
        [
            ("early.NEF", Some("2021-06-06T08:00:00.000")),
            ("tie2.NEF", Some("2021-06-06T11:00:00.000")),
            ("tie1.NEF", Some("2021-06-06T11:00:00.000")),
            ("late.NEF", Some("2021-06-06T12:00:00.000")),
            ("file.tif", None),
            ("m.NEF", None),
            ("z.NEF", None),
        ]
    );
    Ok(())
}

#[test]
fn a_batch_keeps_the_order_read_when_its_dialog_opened() -> Result<()> {
    let fx = fixture(&["a.NEF", "b.NEF"]);
    let (a, b) = (fx.photo("a.NEF"), fx.photo("b.NEF"));
    let order = rename_order(&fx.cat, &[a, b]).map_err(|e| anyhow::anyhow!("{e}"))?;
    // A capture time read meanwhile (the backfill, another window) would
    // put b first; the preview keeps the order it was given.
    fx.cat.db_for_tests().execute(
        "UPDATE photos SET captured='2021-01-01T00:00:00.000' WHERE id=?1",
        [b.0],
    )?;
    let template = NameTemplate {
        naming: Naming::CustomNameSequence,
        text: "x".into(),
        start: 1,
        padding: 0,
        tokens: None,
    };
    let preview = rename_preview(&fx.cat, &order, &template).map_err(|e| anyhow::anyhow!("{e}"))?;
    let rows: Vec<(PhotoId, &str)> = preview
        .rows
        .iter()
        .map(|r| (r.photo, r.new.as_str()))
        .collect();
    assert_eq!(rows, [(a, "x-1.NEF"), (b, "x-2.NEF")]);
    Ok(())
}

#[test]
fn a_preview_names_the_batch_in_order_and_plans_exactly_that() -> Result<()> {
    let mut fx = fixture(&["b.NEF", "b.xmp", "a.NEF", "c.JPG"]);
    let given = [fx.photo("b.NEF"), fx.photo("a.NEF"), fx.photo("c.JPG")];
    let order = rename_order(&fx.cat, &given).map_err(|e| anyhow::anyhow!("{e}"))?;
    let template = NameTemplate {
        naming: Naming::CustomNameSequence,
        text: "Trip".into(),
        start: 1,
        padding: 3,
        tokens: None,
    };
    let preview = rename_preview(&fx.cat, &order, &template).map_err(|e| anyhow::anyhow!("{e}"))?;
    let rows: Vec<(&str, &str, bool)> = preview
        .rows
        .iter()
        .map(|r| (r.old.as_str(), r.new.as_str(), r.undated))
        .collect();
    assert_eq!(
        rows,
        [
            ("a.NEF", "Trip-001.NEF", true),
            ("b.NEF", "Trip-002.NEF", true),
            ("c.JPG", "Trip-003.JPG", true)
        ]
    );
    assert!(preview.rows.iter().all(|r| r.problem.is_none()));
    let outcome = execute::execute_with(
        &mut fx.cat,
        preview.plan.confirm(),
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    assert_eq!(outcome.items.len(), 3);
    assert_eq!(
        listing(&fx),
        [
            "Trip-001.NEF",
            "Trip-002.NEF",
            "Trip-002.xmp",
            "Trip-003.JPG"
        ]
    );
    // A name taken marks its photo, and blocks the others.
    std::fs::write(fx.root.join("Next-2.NEF"), b"in the way")?;
    let order = rename_order(&fx.cat, &given).map_err(|e| anyhow::anyhow!("{e}"))?;
    let template = NameTemplate {
        naming: Naming::CustomNameSequence,
        text: "Next".into(),
        start: 1,
        padding: 0,
        tokens: None,
    };
    let preview = rename_preview(&fx.cat, &order, &template).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(preview.plan.is_empty());
    let problems: Vec<_> = preview.rows.iter().map(|r| r.problem.clone()).collect();
    assert_eq!(
        problems,
        [
            Some(Skip::Blocked),
            Some(refused(Refusal::NameTaken("Next-2.NEF".into()))),
            Some(Skip::Blocked)
        ]
    );
    // Undated photos named by date say so.
    let template = NameTemplate {
        naming: Naming::DateFilename,
        ..NameTemplate::default()
    };
    let preview = rename_preview(&fx.cat, &order, &template).map_err(|e| anyhow::anyhow!("{e}"))?;
    assert!(
        preview
            .rows
            .iter()
            .all(|r| r.note.is_some() && r.new.starts_with("undated-"))
    );
    Ok(())
}

#[test]
fn a_case_only_rename_leaves_a_companion_already_so_named_where_it_is() -> Result<()> {
    let mut fx = fixture(&["A.NEF", "a.xmp"]);
    let a = fx.photo("A.NEF");
    let (spelling, case) = volume(&fx);
    if !spelling && case != fsops::Folding::Exact {
        return Ok(());
    }
    let xmp = fsops::entry(&fx.root.join("a.xmp"))?.id;
    let outcome = fx.run(rename(&fx, &[("A.NEF", "a")]), &NoTrash, &Faulty::default());
    assert_eq!(results(&outcome), [renamed("a.NEF")]);
    assert_eq!(listing(&fx), ["a.NEF", "a.xmp"]);
    assert_eq!(fsops::entry(&fx.root.join("a.xmp"))?.id, xmp);
    assert_eq!(fx.rows()[&a], "a.NEF");
    Ok(())
}

/// Renaming 1500 photos with their sidecars in one folder: their names are
/// planned as they are typed, so planning reads each folder once and asks
/// its index, never every name again for every photo (#681, where a plan
/// of this took minutes).
#[test]
fn a_large_batch_is_planned_in_time_linear_in_its_photos() -> Result<()> {
    let names: Vec<String> = (0..1500)
        .flat_map(|i| [format!("DSC{i:05}.NEF"), format!("DSC{i:05}.NEF.xmp")])
        .collect();
    let names: Vec<&str> = names.iter().map(String::as_str).collect();
    let fx = fixture(&names);
    let photos: Vec<PhotoId> = fx.cat.photos()?.into_iter().map(|p| p.id).collect();
    let order = rename_order(&fx.cat, &photos).map_err(|e| anyhow::anyhow!("{e}"))?;
    let template = NameTemplate {
        naming: Naming::CustomNameSequence,
        text: "Trip".into(),
        start: 1,
        padding: 4,
        tokens: None,
    };
    let started = std::time::Instant::now();
    let preview = rename_preview(&fx.cat, &order, &template).map_err(|e| anyhow::anyhow!("{e}"))?;
    let took = started.elapsed();
    eprintln!("planned 1500 renames in {took:?}");
    assert_eq!(preview.plan.summary().files, 1500);
    assert_eq!(preview.plan.summary().companions, 1500);
    assert!(preview.rows.iter().all(|r| r.problem.is_none()));
    assert_eq!(preview.rows[1499].new, "Trip-1500.NEF");
    // Minutes when every photo asked every name; well under a second now,
    // with room for a slow machine's debug build.
    assert!(took < std::time::Duration::from_secs(30), "{took:?}");
    Ok(())
}

/// Nothing is renamed on an SMB share, whose rename can't promise not to
/// replace a file another computer just put at the name (macOS's answers
/// so; Linux's CIFS replaced it every time, #573): the photo stays as it
/// was, and nothing is left to recover.
#[test]
fn a_photo_on_an_smb_share_is_not_renamed() -> Result<()> {
    let mut fx = fixture(&["a.NEF", "a.xmp"]);
    let a = fx.photo("a.NEF");
    let disk = Faulty {
        names_only: vec![fx.root.clone()],
        share: true,
        no_exclusive_rename: true,
        ..Faulty::default()
    };
    let plan = fx.plan_on(rename(&fx, &[("a.NEF", "b")]), &disk)?;
    let outcome =
        execute::execute_with(&mut fx.cat, plan.confirm(), &NoTrash, Cancel::NEVER, &disk)?;
    assert!(
        matches!(results(&outcome)[..], [ItemResult::Unchanged(_)]),
        "{outcome:?}"
    );
    assert_eq!(listing(&fx), ["a.NEF", "a.xmp"]);
    assert_eq!(fx.rows()[&a], "a.NEF");
    assert!(fx.recover(&disk).unfinished.is_empty());
    Ok(())
}

/// A batch is named from each photo's own catalog fields and its folder,
/// read with its order: a template naming the title, rating and folder
/// gives each photo its own, whatever order they were chosen in.
#[test]
fn a_batch_is_named_from_each_photos_catalog_fields_and_folder() -> Result<()> {
    let mut fx = fixture(&["day 1/a.NEF", "day 1/b.NEF"]);
    let (a, b) = (fx.photo("day 1/a.NEF"), fx.photo("day 1/b.NEF"));
    fx.cat
        .set_text(&[b], crate::metadata::TextField::Title, "Harbour")?;
    fx.cat.db_for_tests().execute(
        "UPDATE photos SET rating = 4, captured = '2021-01-01T00:00:00.000' WHERE id=?1",
        [b.0],
    )?;
    let copy = fx.cat.create_virtual_copy(a)?;
    // b is dated, so first; a's copy stands for a's file.
    let order = rename_order(&fx.cat, &[copy, b]).map_err(|e| anyhow::anyhow!("{e}"))?;
    let template = NameTemplate {
        tokens: Some(crate::export_settings::TokenTemplate::parse(
            "{{image_folder}}-{{com.adobe.title}}-{{com.adobe.rating.string}}-{{copy_name}}",
        )),
        ..NameTemplate::default()
    };
    let preview = rename_preview(&fx.cat, &order, &template).map_err(|e| anyhow::anyhow!("{e}"))?;
    let rows: Vec<(PhotoId, &str)> = preview
        .rows
        .iter()
        .map(|r| (r.photo, r.new.as_str()))
        .collect();
    assert_eq!(
        rows,
        [(b, "day 1-Harbour-4-.NEF"), (copy, "day 1---Copy 1.NEF")]
    );
    Ok(())
}
