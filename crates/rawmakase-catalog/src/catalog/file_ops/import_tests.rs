//! Import Photos (#806): planning, naming, families, duplicates, Import #,
//! refused drives, Add Folder and Synchronize meanwhile, and whatever a
//! fault or a crash leaves at each step, with recovery after it.
use super::disk::Disk;
use super::import::{self, ImportState, Role};
use super::journal::{self, ItemState, Kind, MemberState, Subject};
use super::tests::{Fault, Faulty, Fx, NoTrash, Op, bytes, crashes, fixture};
use super::*;
use crate::catalog::Catalog;
use crate::catalog::NewPhotoDefaults;
use crate::export_settings::{NameFields, NameTemplate, Naming, TokenTemplate};
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// A card's folder, `card/DCIM/100FUJI`, beside the catalog, holding
/// `files`.
fn card(fx: &Fx, files: &[&str]) -> PathBuf {
    let dir = fx
        .dir
        .path()
        .canonicalize()
        .unwrap()
        .join("card/DCIM/100FUJI");
    std::fs::create_dir_all(&dir).unwrap();
    for f in files {
        std::fs::write(dir.join(f), bytes(f)).unwrap();
    }
    dir
}

/// `path` as the scan reads it, taken at `captured`.
fn source(path: &Path, captured: Option<&str>) -> ImportSource {
    ImportSource {
        file: scan_file(path).unwrap(),
        companions: import_companions(path)
            .unwrap()
            .iter()
            .map(|c| scan_file(c).unwrap())
            .collect(),
        captured: captured.map(Into::into),
        modified: None,
        fields: NameFields::default(),
    }
}

/// An import of `sources` (from the folder of the first) into
/// `destination`, organised as `organize`, keeping names and leaving the
/// suspected duplicates out.
fn request(destination: &Path, sources: Vec<ImportSource>, organize: Organize) -> ImportRequest {
    let source = sources
        .first()
        .map(|s| s.file.path.parent().unwrap().to_path_buf())
        .unwrap_or_default();
    ImportRequest {
        sources,
        source,
        destination: destination.to_path_buf(),
        subfolder: String::new(),
        organize,
        naming: None,
        shoot_name: String::new(),
        extensions: ExtensionCase::LeaveAsIs,
        choices: Vec::new(),
        defaults: NewPhotoDefaults::default(),
        apply: ImportMetadata::default(),
        skip_duplicates: true,
    }
}

fn into_one() -> Organize {
    Organize::IntoOneFolder
}

/// Plans `request` and runs it as planned on `disk`.
fn import_on(fx: &mut Fx, request: &ImportRequest, disk: &dyn Disk) -> ImportOutcome {
    let plan = import::plan_with(&fx.cat, disk, request).unwrap();
    import::execute_on(&mut fx.cat, request, &plan, &NoTrash, Cancel::NEVER, disk).unwrap()
}

fn ran(outcome: ImportOutcome) -> (Vec<PhotoId>, Vec<(PathBuf, String)>) {
    match outcome {
        ImportOutcome::Ran {
            imported, skipped, ..
        } => (imported, skipped),
        ImportOutcome::Changed(plan) => panic!("changed: {plan:?}"),
        ImportOutcome::Unread { error, .. } => panic!("unread: {error}"),
    }
}

/// The import items journalled, by role: photo items, then finish items.
fn items(fx: &Fx) -> (Vec<journal::Item>, Vec<journal::Item>) {
    let all: Vec<journal::Item> = journal::unresolved_anywhere(&fx.cat.db)
        .unwrap()
        .into_iter()
        .chain(
            fx.cat
                .db_for_tests()
                .prepare("SELECT DISTINCT batch FROM file_operations WHERE kind='import_copy'")
                .unwrap()
                .query_map([], |r| r.get::<_, String>(0))
                .unwrap()
                .flat_map(|b| journal::of_batch(&fx.cat.db, &b.unwrap()).unwrap()),
        )
        .filter(|i| i.kind == Kind::ImportCopy)
        .collect();
    let mut seen = HashSet::new();
    let all: Vec<_> = all.into_iter().filter(|i| seen.insert(i.id)).collect();
    all.into_iter().partition(|i| {
        matches!(
            &i.subject,
            Subject::Import(ImportState {
                role: Role::Photos { .. },
                ..
            })
        )
    })
}

fn counter(fx: &Fx) -> Option<i64> {
    fx.cat
        .db_for_tests()
        .query_row(
            "SELECT value FROM catalog_counters WHERE name='import_number'",
            [],
            |r| r.get(0),
        )
        .ok()
}

fn path_of(fx: &Fx, id: PhotoId) -> PathBuf {
    fx.cat
        .photos()
        .unwrap()
        .into_iter()
        .find(|p| p.id == id)
        .unwrap()
        .path
}

#[test]
fn a_card_is_imported_by_date_and_added_as_add_folder_adds() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0002.JPG", "DSCF0003.RAF"]);
    let before = std::fs::read(dir.join("DSCF0001.RAF"))?;
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![
            source(&dir.join("DSCF0002.JPG"), Some("2024-05-02T09:00:00.000")),
            source(&dir.join("DSCF0001.RAF"), Some("2024-05-01T10:00:00.000")),
            // No capture time: dated by its file's modification time, as
            // Lightroom dates it.
            ImportSource {
                modified: Some("2024-06-01T12:00:00.000".into()),
                ..source(&dir.join("DSCF0003.RAF"), None)
            },
        ],
        Organize::ByDate(DateFolders::YearThenDate),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert!(plan.can_run(), "{plan:?}");
    assert_eq!(plan.number, 1);
    // In date order; folders outermost first.
    let to: Vec<PathBuf> = plan
        .photos
        .iter()
        .map(|p| p.destination.clone().unwrap())
        .collect();
    assert_eq!(
        to,
        [
            library.join("2024/2024-05-01/DSCF0001.RAF"),
            library.join("2024/2024-05-02/DSCF0002.JPG"),
            library.join("2024/2024-06-01/DSCF0003.RAF"),
        ]
    );
    assert_eq!(
        plan.folders,
        [
            library.join("2024"),
            library.join("2024/2024-05-01"),
            library.join("2024/2024-05-02"),
            library.join("2024/2024-06-01"),
        ]
    );
    let (imported, skipped) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert!(skipped.is_empty(), "{skipped:?}");
    assert_eq!(imported.len(), 3);
    // The copies, byte for byte; the card as it was.
    assert_eq!(std::fs::read(&to[0])?, before);
    assert_eq!(std::fs::read(dir.join("DSCF0001.RAF"))?, before);
    for (id, path) in imported.iter().zip(&to) {
        assert_eq!(&path_of(&fx, *id), path);
    }
    // What each was on the card, for duplicates and Original filename.
    assert_eq!(
        fx.cat.import_facts(imported[0])?,
        Some((
            "DSCF0001.RAF".into(),
            Some("2024-05-01T10:00:00.000".into()),
            Some(before.len() as i64),
            true
        ))
    );
    assert_eq!(fx.cat.import_facts(imported[2])?.unwrap().1, None);
    // Previous Import, as an import batch of its own.
    let mut previous = fx.cat.previous_import()?;
    previous.sort();
    assert_eq!(previous, imported);
    let kind: String = fx.cat.db_for_tests().query_row(
        "SELECT kind FROM import_batches ORDER BY id DESC LIMIT 1",
        [],
        |r| r.get(0),
    )?;
    assert_eq!(kind, "copy");
    // Every item done; nothing hidden left behind.
    let (photos, finish) = items(&fx);
    assert!(photos.iter().all(|i| i.state == ItemState::Done));
    assert_eq!(finish.len(), 1);
    assert_eq!(finish[0].state, ItemState::Done);
    assert!(!fx.files().keys().any(|k| k.ends_with(".rawmakase-part")));
    assert_eq!(counter(&fx), Some(1));
    Ok(())
}

#[test]
fn names_come_from_the_template_and_a_taken_name_gets_the_next_stem() -> Result<()> {
    let mut fx = fixture(&["imports/Trip-1.RAF"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0001.xmp", "DSCF0002.RAF"]);
    let library = fx.root.join("imports");
    let mut request = request(
        &library,
        vec![
            source(&dir.join("DSCF0001.RAF"), Some("2024-05-01T10:00:00.000")),
            source(&dir.join("DSCF0002.RAF"), Some("2024-05-01T10:00:01.000")),
        ],
        into_one(),
    );
    request.naming = Some(NameTemplate {
        naming: Naming::Filename,
        text: String::new(),
        start: 1,
        padding: 1,
        tokens: Some(TokenTemplate::parse(
            "Trip-{{naming_importNumber_1Digit}}{{image_originalName}}",
        )),
    });
    // Trip-1DSCF0001: free. Then the same with a taken name: the bundle,
    // sidecar and all, takes the next stem.
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("Trip-1DSCF0001.RAF").as_path())
    );
    assert_eq!(
        plan.photos[0].companions,
        [library.join("Trip-1DSCF0001.xmp")]
    );
    std::fs::write(library.join("Trip-1DSCF0001.xmp"), b"someone else's")?;
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("Trip-1DSCF0001-2.RAF").as_path())
    );
    assert_eq!(
        plan.photos[0].companions,
        [library.join("Trip-1DSCF0001-2.xmp")]
    );
    let (imported, _) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert_eq!(imported.len(), 2);
    assert_eq!(
        std::fs::read(library.join("Trip-1DSCF0001.xmp"))?,
        b"someone else's"
    );
    assert_eq!(
        std::fs::read(library.join("Trip-1DSCF0001-2.xmp"))?,
        bytes("DSCF0001.xmp")
    );
    Ok(())
}

#[test]
fn photos_reading_one_sidecar_under_one_stem_are_one_family() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["A.NEF", "A.DNG", "A.xmp"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![
            source(&dir.join("A.NEF"), None),
            source(&dir.join("A.DNG"), None),
        ],
        into_one(),
    );
    let (imported, _) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert_eq!(imported.len(), 2);
    // One item: the sidecar copied once, read by both.
    let (photos, _) = items(&fx);
    assert_eq!(photos.len(), 1);
    assert_eq!(photos[0].members.len(), 3);
    assert_eq!(std::fs::read(library.join("A.xmp"))?, bytes("A.xmp"));
    Ok(())
}

#[test]
fn one_folder_s_sidecar_isn_t_another_s_and_is_suffixed() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let base = fx.dir.path().canonicalize()?.join("card");
    for d in ["one", "two"] {
        std::fs::create_dir_all(base.join(d))?;
        for f in ["A.NEF", "A.xmp"] {
            std::fs::write(base.join(d).join(f), bytes(&format!("{d}{f}")))?;
        }
    }
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![
            source(&base.join("one/A.NEF"), Some("2024-01-01T00:00:00.000")),
            source(&base.join("two/A.NEF"), Some("2024-01-02T00:00:00.000")),
        ],
        into_one(),
    );
    let (imported, _) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert_eq!(imported.len(), 2);
    assert_eq!(std::fs::read(library.join("A.xmp"))?, bytes("oneA.xmp"));
    assert_eq!(std::fs::read(library.join("A-2.xmp"))?, bytes("twoA.xmp"));
    assert_eq!(std::fs::read(library.join("A-2.NEF"))?, bytes("twoA.NEF"));
    Ok(())
}

#[test]
fn a_card_imported_again_is_suspected_duplicates() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0002.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let sources = || {
        vec![
            source(&dir.join("DSCF0001.RAF"), Some("2024-05-01T10:00:00.000")),
            source(&dir.join("DSCF0002.RAF"), None),
        ]
    };
    ran(import_on(
        &mut fx,
        &request(&library, sources(), into_one()),
        &Faulty::default(),
    ));
    // The same files again, wherever the first copies are now.
    assert_eq!(
        suspected_duplicates(&fx.cat, &sources())?,
        [Some(Duplicate::InCatalog), Some(Duplicate::NameAndSize)]
    );
    let mut again = request(&library, sources(), into_one());
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &again)?;
    assert!(plan.photos.iter().all(|p| p.destination.is_none()));
    assert!(!plan.can_run());
    // Without Don't Import Suspected Duplicates none is looked for: both
    // imported, under the next free names.
    again.skip_duplicates = false;
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &again)?;
    assert!(plan.photos.iter().all(|p| p.duplicate.is_none()));
    let (imported, skipped) = ran(import_on(&mut fx, &again, &Faulty::default()));
    assert_eq!(imported.len(), 2);
    assert_eq!(path_of(&fx, imported[0]), library.join("DSCF0001-2.RAF"));
    assert!(skipped.is_empty(), "{skipped:?}");
    // Within one scan, the second of two alike.
    let copy = fx.dir.path().canonicalize()?.join("other/DSCF0009.RAF");
    std::fs::create_dir_all(copy.parent().unwrap())?;
    std::fs::write(&copy, bytes("DSCF0009.RAF"))?;
    std::fs::write(dir.join("DSCF0009.RAF"), bytes("DSCF0009.RAF"))?;
    assert_eq!(
        suspected_duplicates(
            &fx.cat,
            &[source(&copy, None), source(&dir.join("DSCF0009.RAF"), None)]
        )?,
        [None, Some(Duplicate::InImport)]
    );
    Ok(())
}

#[test]
fn a_photo_added_from_a_folder_is_a_duplicate_by_its_facts() -> Result<()> {
    let fx = fixture(&["DSCF0001.RAF"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    // Added by Add Folder: name and size known, its EXIF date not read yet.
    assert_eq!(
        suspected_duplicates(
            &fx.cat,
            &[source(
                &dir.join("DSCF0001.RAF"),
                Some("2024-05-01T10:00:00.000")
            )]
        )?,
        [Some(Duplicate::NameAndSize)]
    );
    // Read, it is matched by its date too.
    let id = fx.photo("DSCF0001.RAF");
    let mut cat = Catalog::open(&fx.catalog)?;
    cat.record_capture_reads(&[(id, Some("2024-05-01T10:00:00.000".into()))], &[])?;
    assert_eq!(
        suspected_duplicates(
            &cat,
            &[source(
                &dir.join("DSCF0001.RAF"),
                Some("2024-05-01T10:00:00.000")
            )]
        )?,
        [Some(Duplicate::InCatalog)]
    );
    assert_eq!(
        suspected_duplicates(
            &cat,
            &[source(
                &dir.join("DSCF0001.RAF"),
                Some("2024-05-01T10:00:09.000")
            )]
        )?,
        [None]
    );
    // An older catalog's photo, without facts: its name and checksum size.
    cat.db_for_tests()
        .execute("DELETE FROM photo_import_facts", [])?;
    assert_eq!(
        suspected_duplicates(&cat, &[source(&dir.join("DSCF0001.RAF"), None)])?,
        [None],
        "no checksum read: no size to match"
    );
    Ok(())
}

#[test]
fn a_destination_that_can_t_name_copies_exclusively_is_refused() -> Result<()> {
    let fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.dir.path().canonicalize()?.join("exfat");
    std::fs::create_dir(&library)?;
    let mut disk = Faulty::default().elsewhere(&library);
    disk.names_only = vec![library.clone()];
    disk.no_exclusive_rename = true;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &disk, &request)?;
    assert!(plan.refused.as_ref().is_some_and(|r| r.contains("exFAT")));
    assert!(!plan.can_run());
    Ok(())
}

#[test]
fn a_new_root_is_added_for_a_destination_the_catalog_has_none_of() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.dir.path().canonicalize()?.join("library");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(
            &dir.join("DSCF0001.RAF"),
            Some("2024-05-01T10:00:00.000"),
        )],
        Organize::ByDate(DateFolders::Date),
    );
    let (imported, _) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert_eq!(
        path_of(&fx, imported[0]),
        library.join("2024-05-01/DSCF0001.RAF")
    );
    let roots: i64 = fx
        .cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM roots", [], |r| r.get(0))?;
    assert_eq!(roots, 2);
    // A run that adds nothing leaves no root behind.
    let empty = fx.dir.path().canonicalize()?.join("empty");
    std::fs::create_dir(&empty)?;
    std::fs::write(dir.join("DSCF0002.RAF"), bytes("DSCF0002.RAF"))?;
    let request = self::request(
        &empty,
        vec![source(&dir.join("DSCF0002.RAF"), None)],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    let disk = Faulty::at(Op::Copy, 0, Fault::Fail);
    let outcome = import::execute_on(&mut fx.cat, &request, &plan, &NoTrash, Cancel::NEVER, &disk)?;
    assert!(ran(outcome).0.is_empty());
    let roots: i64 = fx
        .cat
        .db_for_tests()
        .query_row("SELECT count(*) FROM roots", [], |r| r.get(0))?;
    assert_eq!(roots, 2);
    Ok(())
}

#[test]
fn import_number_is_reserved_once_and_a_changed_plan_is_asked_again() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["A.RAF", "A.xmp", "B.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    // The sidecar copied, the photo's copy failing: Import # 1 was
    // reserved before the first copy, and stays taken.
    let first = request(&library, vec![source(&dir.join("A.RAF"), None)], into_one());
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &first)?;
    let disk = Faulty::at(Op::Copy, 1, Fault::Fail);
    let outcome = import::execute_on(&mut fx.cat, &first, &plan, &NoTrash, Cancel::NEVER, &disk)?;
    let (imported, skipped) = ran(outcome);
    assert!(imported.is_empty());
    assert_eq!(skipped.len(), 1);
    // The sidecar written whole is kept, for the user: Keep as Is only.
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::NeedsRecovery);
    assert_eq!(photos[0].members[0].state, MemberState::Copied);
    assert!(library.join("A.xmp").exists());
    let recovery = fx.recover(&Faulty::default());
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(recovery.unfinished[0].choices, [Resolution::Keep]);
    assert!(matches!(
        recover::resolve_with(
            &mut fx.cat,
            recovery.unfinished[0].operation,
            Resolution::PutBack,
            &Faulty::default(),
            &NoTrash
        ),
        Err(FileOpError::Unsupported(_))
    ));
    recover::resolve_with(
        &mut fx.cat,
        recovery.unfinished[0].operation,
        Resolution::Keep,
        &Faulty::default(),
        &NoTrash,
    )?;
    assert_eq!(counter(&fx), Some(1));
    // A plan made before another run took the next number is shown again.
    let second = request(&library, vec![source(&dir.join("B.RAF"), None)], into_one());
    let stale = import::plan_with(&fx.cat, &Faulty::default(), &second)?;
    assert_eq!(stale.number, 2);
    ran(import_on(&mut fx, &second, &Faulty::default()));
    assert_eq!(counter(&fx), Some(2));
    std::fs::write(dir.join("C.RAF"), bytes("C.RAF"))?;
    let third = request(&library, vec![source(&dir.join("C.RAF"), None)], into_one());
    let stale = ImportPlan {
        number: 2,
        ..import::plan_with(&fx.cat, &Faulty::default(), &third)?
    };
    let ImportOutcome::Changed(now) = import::execute_on(
        &mut fx.cat,
        &third,
        &stale,
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?
    else {
        panic!("ran a plan that changed");
    };
    assert_eq!(now.number, 3);
    assert!(!library.join("C.RAF").exists());
    Ok(())
}

#[test]
fn a_source_changed_since_the_scan_is_skipped() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let file = dir.join("DSCF0001.RAF");
    let request = request(&library, vec![source(&file, None)], into_one());
    // Replaced with as many bytes and the same time, as a FAT card keeps
    // it: its first bytes tell it.
    let modified = std::fs::metadata(&file)?.modified()?;
    let mut other = bytes("DSCF0001.RAF");
    other[0] ^= 0xff;
    std::fs::write(&file, &other)?;
    std::fs::File::options()
        .write(true)
        .open(&file)?
        .set_modified(modified)?;
    let disk = Faulty {
        names_only: vec![dir],
        ..Faulty::default()
    };
    let (imported, skipped) = ran(import_on(&mut fx, &request, &disk));
    assert!(imported.is_empty());
    assert!(
        skipped[0].1.contains("changed since it was scanned"),
        "{skipped:?}"
    );
    assert!(!library.join("DSCF0001.RAF").exists());
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::NoChange);
    Ok(())
}

#[test]
fn add_folder_and_synchronize_leave_out_what_an_import_is_copying() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0002.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![
            source(&dir.join("DSCF0001.RAF"), None),
            source(&dir.join("DSCF0002.RAF"), None),
        ],
        into_one(),
    );
    // The first photo published, the process gone before its commit.
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    let disk = Faulty::at(Op::Rename, 0, Fault::CrashAfter);
    assert!(crashes(|| {
        let _ = import::execute_on(&mut fx.cat, &request, &plan, &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    assert!(library.join("DSCF0001.RAF").exists());
    assert_eq!(import::reserved_paths(&fx.cat)?.len(), 2);
    // Synchronize finds the copy, and leaves it to the import.
    let added = fx
        .cat
        .import_folder(&fx.root, &NewPhotoDefaults::default(), &[])?;
    assert_eq!(added.added, 0);
    assert_eq!(added.importing, [library.join("DSCF0001.RAF")]);
    // A photo source reporting the copy is refused, and nothing is written.
    let before = fx.cat.previous_import()?;
    let refused = fx.cat.import_files(
        &library,
        &[library.join("DSCF0001.RAF")],
        &NewPhotoDefaults::default(),
        || true,
    );
    assert!(
        refused
            .as_ref()
            .is_err_and(|e| e.to_string().contains("being imported")),
        "{refused:?}"
    );
    assert_eq!(fx.cat.previous_import()?, before);
    // Recovery finishes it with the import's own facts.
    let recovery = fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let (photos, finish) = items(&fx);
    assert!(
        photos.iter().all(|i| i.state == ItemState::Done),
        "{photos:?}"
    );
    assert_eq!(finish[0].state, ItemState::Done);
    let id = fx.photo("imports/DSCF0001.RAF");
    assert!(fx.cat.import_facts(id)?.unwrap().3, "the import's facts");
    assert!(fx.cat.previous_import()?.contains(&id));
    Ok(())
}

#[test]
fn a_photo_the_catalog_has_at_the_destination_already_is_left_as_it_is() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    let disk = Faulty::at(Op::Rename, 0, Fault::CrashAfter);
    assert!(crashes(|| {
        let _ = import::execute_on(&mut fx.cat, &request, &plan, &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    // An older release (or another computer) added it meanwhile.
    let root = fx.root.clone();
    let other = fx.add_row(&root, "imports", "DSCF0001.RAF");
    let recovery = fx.recover(&Faulty::default());
    assert_eq!(recovery.unfinished.len(), 1);
    assert_eq!(recovery.unfinished[0].choices, [Resolution::Keep]);
    assert!(library.join("DSCF0001.RAF").exists());
    assert_eq!(
        fx.rows().get(&other).map(String::as_str),
        Some("imports/DSCF0001.RAF")
    );
    // Recovered again, it stays as it is; the run finishes without it.
    let again = fx.recover(&Faulty::default());
    assert_eq!(again.unfinished.len(), 1);
    let (_, finish) = items(&fx);
    assert_eq!(finish[0].state, ItemState::Done);
    Ok(())
}

#[test]
fn a_crash_at_any_copy_step_is_recovered_without_losing_a_copy() -> Result<()> {
    for op in [
        Op::CreateFile,
        Op::Copy,
        Op::SyncFile,
        Op::CopyTimes,
        Op::Hash,
        Op::Rename,
        Op::Sync,
    ] {
        for fault in [Fault::CrashBefore, Fault::CrashAfter] {
            for nth in 0..2 {
                let mut fx = fixture(&["keep.dng"]);
                let dir = card(&fx, &["A.RAF", "A.xmp"]);
                let library = fx.root.join("imports");
                std::fs::create_dir(&library)?;
                let request = request(&library, vec![source(&dir.join("A.RAF"), None)], into_one());
                let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
                let disk = Faulty::at(op, nth, fault);
                let crashed = crashes(|| {
                    let _ = import::execute_on(
                        &mut fx.cat,
                        &request,
                        &plan,
                        &NoTrash,
                        Cancel::NEVER,
                        &disk,
                    );
                });
                let case = format!("{op:?} #{nth} {fault:?}");
                fx.reopen();
                let first = fx.recover(&Faulty::default());
                let second = fx.recover(&Faulty::default());
                let (photos, finish) = items(&fx);
                // The card is never touched; whatever is left is decided
                // and stays decided.
                assert_eq!(std::fs::read(dir.join("A.RAF"))?, bytes("A.RAF"), "{case}");
                assert_eq!(first.unfinished.len(), second.unfinished.len(), "{case}");
                assert!(
                    finish.iter().all(|i| i.state == ItemState::Done),
                    "{case}: {finish:?}"
                );
                match photos[0].state {
                    // Recovery carries a run on from where it stopped.
                    ItemState::Done => {
                        assert_eq!(
                            std::fs::read(library.join("A.RAF"))?,
                            bytes("A.RAF"),
                            "{case}"
                        );
                        assert_eq!(
                            std::fs::read(library.join("A.xmp"))?,
                            bytes("A.xmp"),
                            "{case}"
                        );
                        fx.photo("imports/A.RAF");
                    }
                    ItemState::NoChange => {
                        assert!(!crashed || !library.join("A.RAF").exists(), "{case}")
                    }
                    state => panic!("{case}: {state:?}"),
                }
                // Only partial copies never written whole are ever removed;
                // none is left once decided.
                assert!(
                    !fx.files().keys().any(|k| k.ends_with(".rawmakase-part")),
                    "{case}: {:?}",
                    fx.files().keys().collect::<Vec<_>>()
                );
            }
        }
    }
    Ok(())
}

#[test]
fn a_crash_after_the_last_commit_queues_burst_stacking_on_recovery() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    request.defaults = NewPhotoDefaults::default().with_options(crate::catalog::ImportOptions {
        stack_bursts: true,
        ..Default::default()
    });
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    // The photo committed; the run ends before its finish.
    fx.fail_journal_with(
        "BEFORE UPDATE OF state ON file_operations
         WHEN NEW.state='done' AND OLD.preconditions LIKE '%\"finish\"%'
         BEGIN SELECT RAISE(FAIL, 'gone'); END",
    );
    // The run says so, with the photo it added.
    let outcome = import::execute_on(
        &mut fx.cat,
        &request,
        &plan,
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    )?;
    let ImportOutcome::Ran {
        imported, outcome, ..
    } = outcome
    else {
        panic!("not run");
    };
    assert_eq!(imported.len(), 1);
    assert!(outcome.stopped.is_some());
    fx.reopen();
    assert!(fx.cat.pending_burst_stacks()?.is_empty());
    let (_, finish) = items(&fx);
    assert_eq!(finish[0].state, ItemState::Planned);
    fx.recover(&Faulty::default());
    let pending = fx.cat.pending_burst_stacks()?;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].photos, [fx.photo("imports/DSCF0001.RAF")]);
    Ok(())
}

#[test]
fn a_family_left_half_copied_is_finished_after_older_items_are_pruned() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["A.NEF", "A.DNG", "A.xmp", "B.NEF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![
            source(&dir.join("B.NEF"), Some("2024-01-01T00:00:00.000")),
            source(&dir.join("A.NEF"), Some("2024-01-02T00:00:00.000")),
            source(&dir.join("A.DNG"), Some("2024-01-02T00:00:00.000")),
        ],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    // B done; the family's sidecar and first photo named, then the end.
    let disk = Faulty::at(Op::Rename, 2, Fault::CrashAfter);
    assert!(crashes(|| {
        let _ = import::execute_on(&mut fx.cat, &request, &plan, &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    // B's item, finished, is forgotten as an old one would be.
    fx.cat.db_for_tests().execute(
        "UPDATE file_operations SET created_at='2000-01-01 00:00:00' WHERE state='done'",
        [],
    )?;
    recover::prune(&mut fx.cat)?;
    fx.recover(&Faulty::default());
    let (photos, finish) = items(&fx);
    assert!(
        photos.iter().all(|i| i.state == ItemState::Done),
        "{photos:?}"
    );
    assert_eq!(finish[0].state, ItemState::Done);
    for name in ["A.NEF", "A.DNG", "B.NEF"] {
        fx.photo(&format!("imports/{name}"));
    }
    Ok(())
}

#[test]
fn an_unfinished_import_is_one_older_releases_fail_closed_on() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    let disk = Faulty::at(Op::Copy, 0, Fault::CrashBefore);
    assert!(crashes(|| {
        let _ = import::execute_on(&mut fx.cat, &request, &plan, &NoTrash, Cancel::NEVER, &disk);
    }));
    fx.reopen();
    // The states an older release's unresolved query selects, of a kind it
    // can't read.
    let unresolved: Vec<(String, String)> = fx
        .cat
        .db_for_tests()
        .prepare(
            "SELECT kind, state FROM file_operations
             WHERE state IN ('planned', 'restoring', 'committed', 'needs_recovery')",
        )?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    assert!(!unresolved.is_empty());
    assert!(unresolved.iter().all(|(k, _)| k == "import_copy"));
    assert!("import_copy".parse::<Kind>().is_ok());
    Ok(())
}

#[test]
fn stopping_mid_copy_removes_the_partial_and_begins_nothing_more() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["A.RAF", "B.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![
            source(&dir.join("A.RAF"), Some("2024-01-01T00:00:00.000")),
            source(&dir.join("B.RAF"), Some("2024-01-02T00:00:00.000")),
        ],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    // Stop pressed as the first copy writes its first chunk.
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let disk = Faulty {
        stop: Some(stop.clone()),
        ..Faulty::default()
    };
    let outcome = import::execute_on(
        &mut fx.cat,
        &request,
        &plan,
        &NoTrash,
        Cancel::new(&stop),
        &disk,
    )?;
    let (imported, skipped) = ran(outcome);
    assert!(imported.is_empty());
    assert_eq!(skipped.len(), 2);
    let (photos, finish) = items(&fx);
    assert!(
        photos.iter().all(|i| i.state == ItemState::NoChange),
        "{photos:?}"
    );
    assert_eq!(finish[0].state, ItemState::Done);
    assert!(!fx.files().keys().any(|k| k.starts_with("trip/imports/")));
    // Its number stays taken.
    assert_eq!(counter(&fx), Some(1));
    Ok(())
}

/// Runs `request` as planned on `disk`, which crashes; then reopens.
fn crash_on(fx: &mut Fx, request: &ImportRequest, disk: &Faulty) {
    let plan = import::plan_with(&fx.cat, &Faulty::default(), request).unwrap();
    assert!(crashes(|| {
        let _ = import::execute_on(&mut fx.cat, request, &plan, &NoTrash, Cancel::NEVER, disk);
    }));
    fx.reopen();
}

#[test]
fn a_copy_changed_before_recovery_commits_is_not_added() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    // Every copy named; the catalog's transaction fails.
    fx.fail_journal_with("BEFORE INSERT ON photos BEGIN SELECT RAISE(FAIL, 'disk full'); END");
    let _ = import::execute_on(
        &mut fx.cat,
        &request,
        &plan,
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    );
    fx.reopen();
    // Changed in place since, the same file object.
    let copy = library.join("DSCF0001.RAF");
    let mut bytes = std::fs::read(&copy)?;
    bytes[100] ^= 0xff;
    std::fs::OpenOptions::new()
        .write(true)
        .open(&copy)?
        .write_all_at_start(&bytes)?;
    fx.recover(&Faulty::default());
    assert!(
        fx.cat.photos()?.iter().all(|p| p.path != copy),
        "a changed copy was added"
    );
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::NeedsRecovery);
    Ok(())
}

/// Writes all of `bytes` from the start of a file, keeping its file object.
trait WriteAtStart {
    fn write_all_at_start(self, bytes: &[u8]) -> std::io::Result<()>;
}
impl WriteAtStart for std::fs::File {
    fn write_all_at_start(mut self, bytes: &[u8]) -> std::io::Result<()> {
        use std::io::Write;
        self.write_all(bytes)?;
        self.sync_all()
    }
}

#[test]
fn a_copy_written_whole_before_a_crash_gets_its_time_before_it_is_named() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let file = dir.join("DSCF0001.RAF");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    std::fs::File::options()
        .write(true)
        .open(&file)?
        .set_modified(old)?;
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(&library, vec![source(&file, None)], into_one());
    // Its bytes written and journalled; not synced, nor given its time.
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::SyncFile, 0, Fault::CrashBefore),
    );
    fx.recover(&Faulty::default());
    let copy = library.join("DSCF0001.RAF");
    assert_eq!(std::fs::metadata(&copy)?.modified()?, old);
    fx.photo("imports/DSCF0001.RAF");
    Ok(())
}

#[test]
fn a_copy_named_but_not_journalled_is_never_taken_for_no_change() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    // Named, and the journal can't say so.
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    fx.fail_journal_with(
        "BEFORE UPDATE OF state ON file_operation_members WHEN NEW.state='copied'
         BEGIN SELECT RAISE(FAIL, 'disk full'); END",
    );
    let _ = import::execute_on(
        &mut fx.cat,
        &request,
        &plan,
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    );
    let (photos, _) = items(&fx);
    assert!(library.join("DSCF0001.RAF").exists());
    assert_eq!(photos[0].state, ItemState::NeedsRecovery, "{photos:?}");
    assert_eq!(import::reserved_paths(&fx.cat)?.len(), 1);
    Ok(())
}

#[test]
fn a_copy_named_before_a_crash_is_added_with_the_card_gone() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::Rename, 0, Fault::CrashAfter),
    );
    // The card pulled: the copy is proven by the bytes journalled.
    std::fs::rename(&dir, dir.with_extension("away"))?;
    fx.recover(&Faulty::default());
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::Done, "{photos:?}");
    fx.photo("imports/DSCF0001.RAF");
    Ok(())
}

#[test]
fn a_folder_put_in_the_destination_s_place_is_never_written_to() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::CreateFile, 0, Fault::CrashBefore),
    );
    std::fs::rename(&library, fx.root.join("aside"))?;
    std::fs::create_dir(&library)?;
    fx.recover(&Faulty::default());
    assert_eq!(std::fs::read_dir(&library)?.count(), 0);
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::NoChange);
    Ok(())
}

#[test]
fn bursts_are_stacked_when_recovery_adds_the_first_photo() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    request.defaults = NewPhotoDefaults::default().with_options(crate::catalog::ImportOptions {
        stack_bursts: true,
        ..Default::default()
    });
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::Rename, 0, Fault::CrashAfter),
    );
    fx.recover(&Faulty::default());
    assert_eq!(fx.cat.pending_burst_stacks()?.len(), 1);
    Ok(())
}

#[test]
fn a_sidecar_there_is_never_taken_for_an_imported_photo_s() -> Result<()> {
    let fx = fixture(&["imports/A.xmp"]);
    let dir = card(&fx, &["A.NEF"]);
    let request = request(
        &fx.root.join("imports"),
        vec![source(&dir.join("A.NEF"), None)],
        into_one(),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(fx.root.join("imports/A-2.NEF").as_path())
    );
    Ok(())
}

#[test]
fn a_name_too_long_with_its_extension_is_refused_when_planned() -> Result<()> {
    let fx = fixture(&["imports/keep.dng"]);
    let dir = card(&fx, &["B.NEF"]);
    let mut request = request(
        &fx.root.join("imports"),
        vec![source(&dir.join("B.NEF"), None)],
        into_one(),
    );
    request.naming = Some(NameTemplate {
        naming: Naming::Filename,
        text: "x".repeat(253),
        start: 1,
        padding: 1,
        tokens: Some(TokenTemplate::parse("{{custom_token}}")),
    });
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(plan.photos[0].destination, None, "{plan:?}");
    assert!(!plan.can_run());
    Ok(())
}

#[test]
fn a_catalog_folder_missing_on_disk_is_never_made_over() -> Result<()> {
    let fx = fixture(&["imports/2024-05-01/keep.dng"]);
    std::fs::remove_dir_all(fx.root.join("imports/2024-05-01"))?;
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let request = request(
        &fx.root.join("imports"),
        vec![source(
            &dir.join("DSCF0001.RAF"),
            Some("2024-05-01T10:00:00.000"),
        )],
        Organize::ByDate(DateFolders::Date),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert!(
        plan.photos[0]
            .skipped
            .as_deref()
            .is_some_and(|s| s.contains("missing")),
        "{plan:?}"
    );
    assert!(!plan.can_run());
    Ok(())
}

#[test]
fn a_sidecar_put_there_before_the_commit_is_never_read_as_the_photo_s() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["A.NEF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(&library, vec![source(&dir.join("A.NEF"), None)], into_one());
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    fx.fail_journal_with("BEFORE INSERT ON photos BEGIN SELECT RAISE(FAIL, 'disk full'); END");
    let _ = import::execute_on(
        &mut fx.cat,
        &request,
        &plan,
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    );
    fx.reopen();
    std::fs::write(library.join("A.xmp"), b"someone else's")?;
    fx.recover(&Faulty::default());
    assert!(
        fx.cat
            .photos()?
            .iter()
            .all(|p| p.path != library.join("A.NEF"))
    );
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::NeedsRecovery);
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_read_only_copy_written_whole_before_a_crash_is_named_on_recovery() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let file = dir.join("DSCF0001.RAF");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444))?;
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(&library, vec![source(&file, None)], into_one());
    // Every step but its name done: its permissions too.
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::Rename, 0, Fault::CrashBefore),
    );
    fx.recover(&Faulty::default());
    let (photos, _) = items(&fx);
    assert_eq!(photos[0].state, ItemState::Done, "{photos:?}");
    fx.photo("imports/DSCF0001.RAF");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_partial_made_read_only_before_its_steps_finish_is_finished_on_recovery() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let file = dir.join("DSCF0001.RAF");
    let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_500_000_000);
    std::fs::File::options()
        .write(true)
        .open(&file)?
        .set_modified(old)?;
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(&library, vec![source(&file, None)], into_one());
    // Written whole, not synced nor dated, and read-only, as a umask of
    // 0222 makes it.
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::SyncFile, 0, Fault::CrashBefore),
    );
    let partial = std::fs::read_dir(&library)?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.to_string_lossy().ends_with(".rawmakase-part"))
        .expect("the partial copy");
    std::fs::set_permissions(&partial, std::fs::Permissions::from_mode(0o444))?;
    fx.recover(&Faulty::default());
    let copy = library.join("DSCF0001.RAF");
    assert_eq!(std::fs::metadata(&copy)?.modified()?, old);
    assert_eq!(
        std::fs::metadata(&copy)?.permissions().mode() & 0o777,
        std::fs::metadata(&file)?.permissions().mode() & 0o777
    );
    fx.photo("imports/DSCF0001.RAF");
    Ok(())
}

#[test]
fn a_folder_that_can_t_be_listed_at_the_commit_adds_nothing() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["A.NEF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let request = request(&library, vec![source(&dir.join("A.NEF"), None)], into_one());
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    fx.fail_journal_with("BEFORE INSERT ON photos BEGIN SELECT RAISE(FAIL, 'disk full'); END");
    let _ = import::execute_on(
        &mut fx.cat,
        &request,
        &plan,
        &NoTrash,
        Cancel::NEVER,
        &Faulty::default(),
    );
    fx.reopen();
    let (photos, _) = items(&fx);
    let disk = Faulty::default();
    disk.unlistable.borrow_mut().push(library.clone());
    assert!(import::foreign_companion(&disk, &photos[0], &library).is_err());
    Ok(())
}

#[test]
fn the_duplicate_flagged_is_the_one_an_import_of_both_leaves_out() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let base = fx.dir.path().canonicalize()?.join("card");
    for (d, f) in [("01", "SHOT.RAF"), ("02", "shot.RAF")] {
        std::fs::create_dir_all(base.join(d))?;
        std::fs::write(base.join(d).join(f), bytes("shot"))?;
    }
    let at = Some("2024-05-01T10:00:00.000");
    // As a scan lists them; planning takes SHOT.RAF first.
    let sources = vec![
        source(&base.join("02/shot.RAF"), at),
        source(&base.join("01/SHOT.RAF"), at),
    ];
    assert_eq!(
        suspected_duplicates(&fx.cat, &sources)?,
        [Some(Duplicate::InImport), None]
    );
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    // Without Don't Import Suspected Duplicates both are imported.
    let mut both = request(&library, sources, into_one());
    both.skip_duplicates = false;
    let (imported, skipped) = ran(import_on(&mut fx, &both, &Faulty::default()));
    assert_eq!((imported.len(), skipped.len()), (2, 0), "{skipped:?}");
    Ok(())
}

/// Lightroom Classic's Date Format menu, as it shows 19 November 2026,
/// in its order (measured 2026-11-19).
#[test]
fn date_formats_are_lightroom_s_menu() {
    let shown: Vec<Vec<String>> = DateFolders::GROUPS
        .iter()
        .map(|g| {
            g.iter()
                .map(|f| f.label("2026-11-19T10:00:00.000"))
                .collect()
        })
        .collect();
    assert_eq!(
        shown,
        [
            vec!["2026/11-19", "2026/2026-11-19", "2026/November 19"],
            vec!["2026/11", "2026/Nov", "2026/November"],
            vec!["2026/11/19", "2026/November/19", "2026/2026-11/2026-11-19"],
            vec!["2026-11-19", "2026 Nov 19", "2026 November 19", "20261119"],
        ]
    );
    // Kept by Lightroom's own template.
    assert_eq!(
        serde_json::to_string(&DateFolders::YearMonthDate).unwrap(),
        r#""%Y/%Y-%m/%Y-%m-%d""#
    );
    assert_eq!(
        DateFolders::Date
            .folders("2026-07-07T07:07:07.000")
            .unwrap(),
        ["2026-07-07"]
    );
    assert_eq!(DateFolders::Date.folders("no date"), None);
}

#[test]
fn into_subfolder_comes_before_the_folders_organize_makes() -> Result<()> {
    let fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let card_root = dir.parent().unwrap().parent().unwrap().to_path_buf();
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let at = Some("2026-07-07T07:07:07.000");
    let mut by_date = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), at)],
        Organize::ByDate(DateFolders::YearThenDate),
    );
    by_date.subfolder = " Shoot ".into();
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &by_date)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("Shoot/2026/2026-07-07/DSCF0001.RAF").as_path())
    );
    // By original folders: the folder chosen as the source, and below.
    let mut original = by_date;
    original.organize = Organize::ByOriginalFolders;
    original.source = card_root;
    original.subfolder.clear();
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &original)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("card/DCIM/100FUJI/DSCF0001.RAF").as_path())
    );
    // A photo without a date of any kind isn't filed under one.
    let undated = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        Organize::ByDate(DateFolders::Date),
    );
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &undated)?;
    assert_eq!(plan.photos[0].destination, None);
    assert!(!plan.can_run());
    Ok(())
}

#[test]
fn a_renamed_photo_s_extension_follows_extensions_and_shoot_name_names_it() -> Result<()> {
    let fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0001.xmp"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    // Kept names keep their extension, whatever Extensions says.
    request.extensions = ExtensionCase::Lowercase;
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("DSCF0001.RAF").as_path())
    );
    request.naming = Some(NameTemplate {
        naming: Naming::Filename,
        text: String::new(),
        start: 1,
        padding: 1,
        tokens: Some(TokenTemplate::parse(
            crate::export_settings::IMPORT_BUILTIN[0].tokens,
        )),
    });
    request.shoot_name = "Wedding".into();
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(
        plan.photos[0].destination.as_deref(),
        Some(library.join("Wedding-0001.raf").as_path())
    );
    assert_eq!(
        plan.photos[0].companions,
        [library.join("Wedding-0001.xmp")]
    );
    Ok(())
}

/// A sidecar with a creator, a copyright and a keyword of its own.
const SIDECAR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
<rdf:Description rdf:about="" xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:creator><rdf:Seq><rdf:li>Sidecar Creator</rdf:li></rdf:Seq></dc:creator>
<dc:rights><rdf:Alt><rdf:li xml:lang="x-default">Sidecar Rights</rdf:li></rdf:Alt></dc:rights>
<dc:subject><rdf:Bag><rdf:li>sidecarkw</rdf:li></rdf:Bag></dc:subject>
</rdf:Description></rdf:RDF></x:xmpmeta>"#;

fn applied(fx: &Fx, id: PhotoId) -> (Vec<String>, Option<String>, Vec<String>) {
    let d = fx.cat.descriptive(id).unwrap();
    let creator = match d.creator {
        Some(crate::metadata::Value::Set(names)) => names,
        _ => Vec::new(),
    };
    let copyright = match d.copyright {
        Some(crate::metadata::Value::Set(langs)) => langs.default_text().map(Into::into),
        _ => None,
    };
    let mut keywords: Vec<String> = fx
        .cat
        .keywords(id)
        .unwrap()
        .into_iter()
        .map(|k| k.name)
        .collect();
    keywords.sort();
    (creator, copyright, keywords)
}

fn apply_during_import() -> ImportMetadata {
    ImportMetadata {
        creator: "Preset Creator".into(),
        copyright: "Preset Copyright".into(),
        keywords: vec![vec!["importkw".into()]],
    }
}

/// Lightroom Classic's Apply During Import (measured 2026-11-19): Creator
/// and Copyright replace the sidecar's, keywords are added to its own.
#[test]
fn apply_during_import_replaces_creator_and_copyright_and_adds_keywords() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0002.RAF"]);
    std::fs::write(dir.join("DSCF0001.xmp"), SIDECAR)?;
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut request = request(
        &library,
        vec![
            source(&dir.join("DSCF0001.RAF"), None),
            source(&dir.join("DSCF0002.RAF"), None),
        ],
        into_one(),
    );
    // Without it, the sidecar's own.
    let (plain, _) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert_eq!(
        applied(&fx, plain[0]),
        (
            vec!["Sidecar Creator".into()],
            Some("Sidecar Rights".into()),
            vec!["sidecarkw".into()]
        )
    );
    request.destination = fx.root.join("again");
    std::fs::create_dir(&request.destination)?;
    request.apply = apply_during_import();
    request.skip_duplicates = false;
    let (imported, _) = ran(import_on(&mut fx, &request, &Faulty::default()));
    assert_eq!(
        applied(&fx, imported[0]),
        (
            vec!["Preset Creator".into()],
            Some("Preset Copyright".into()),
            vec!["importkw".into(), "sidecarkw".into()]
        )
    );
    assert_eq!(
        applied(&fx, imported[1]),
        (
            vec!["Preset Creator".into()],
            Some("Preset Copyright".into()),
            vec!["importkw".into()]
        )
    );
    // The copies are the card's, byte for byte: nothing is written in them.
    assert_eq!(
        std::fs::read_to_string(request.destination.join("DSCF0001.xmp"))?,
        SIDECAR
    );
    Ok(())
}

#[test]
fn recovery_applies_what_the_run_was_asked_to() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    request.apply = apply_during_import();
    // Published, the process gone before its commit.
    crash_on(
        &mut fx,
        &request,
        &Faulty::at(Op::Rename, 0, Fault::CrashAfter),
    );
    let recovery = fx.recover(&Faulty::default());
    assert!(recovery.unfinished.is_empty(), "{recovery:?}");
    let id = fx.photo("imports/DSCF0001.RAF");
    assert_eq!(
        applied(&fx, id),
        (
            vec!["Preset Creator".into()],
            Some("Preset Copyright".into()),
            vec!["importkw".into()]
        )
    );
    Ok(())
}

/// A keyword the commit couldn't give is refused when planned: a commit
/// failing on every try would leave its item planned, and recovery, which
/// every file operation runs first, would fail with it.
#[test]
fn keywords_that_can_t_be_given_are_refused_before_anything_is_journalled() -> Result<()> {
    let mut fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF", "DSCF0002.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut bad = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        into_one(),
    );
    for keywords in [vec![vec![]], vec![vec!["Places".into(), "  ".into()]]] {
        bad.apply.keywords = keywords;
        let plan = import::plan_with(&fx.cat, &Faulty::default(), &bad)?;
        assert!(plan.refused.is_some() && !plan.can_run(), "{plan:?}");
        let outcome = import_on(&mut fx, &bad, &Faulty::default());
        assert!(matches!(outcome, ImportOutcome::Changed(_)), "{outcome:?}");
    }
    let (photos, _) = items(&fx);
    assert!(photos.is_empty(), "nothing journalled");
    // The next import, and recovery, go on.
    let good = request(
        &library,
        vec![source(&dir.join("DSCF0002.RAF"), None)],
        into_one(),
    );
    let (imported, _) = ran(import_on(&mut fx, &good, &Faulty::default()));
    assert_eq!(imported.len(), 1);
    assert!(fx.recover(&Faulty::default()).unfinished.is_empty());
    Ok(())
}

#[test]
fn by_original_folders_from_a_drive_s_root_says_why_nothing_goes() -> Result<()> {
    let fx = fixture(&["keep.dng"]);
    let dir = card(&fx, &["DSCF0001.RAF"]);
    let library = fx.root.join("imports");
    std::fs::create_dir(&library)?;
    let mut request = request(
        &library,
        vec![source(&dir.join("DSCF0001.RAF"), None)],
        Organize::ByOriginalFolders,
    );
    request.source = PathBuf::from("/");
    let plan = import::plan_with(&fx.cat, &Faulty::default(), &request)?;
    assert_eq!(
        plan.photos[0].skipped.as_deref(),
        Some("By original folders needs a folder as the source, not a whole drive.")
    );
    Ok(())
}
