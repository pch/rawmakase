use super::*;
use std::fs;

fn dir() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

#[test]
fn an_exclusive_rename_keeps_the_file_object_and_its_time() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join("b.NEF"));
    fs::write(&a, b"synthetic raw").unwrap();
    let before = entry(&a).unwrap();
    let modified = fs::metadata(&a).unwrap().modified().unwrap();
    rename_exclusive(&a, &b).unwrap();
    assert!(matches!(entry(&a), Err(FsError::NotFound)));
    let after = entry(&b).unwrap();
    assert_eq!(after.id, before.id);
    assert_eq!(after.kind, EntryKind::File);
    assert_eq!(fs::metadata(&b).unwrap().modified().unwrap(), modified);
    assert_eq!(fs::read(&b).unwrap(), b"synthetic raw");
}

#[test]
fn an_exclusive_rename_never_replaces_a_file_and_keeps_both() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join("b.NEF"));
    fs::write(&a, b"first").unwrap();
    fs::write(&b, b"second").unwrap();
    assert!(matches!(
        rename_exclusive(&a, &b),
        Err(FsError::AlreadyExists)
    ));
    assert_eq!(fs::read(&a).unwrap(), b"first");
    assert_eq!(fs::read(&b).unwrap(), b"second");
}

#[test]
fn an_exclusive_rename_moves_a_directory_but_never_onto_one() {
    let d = dir();
    let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
    fs::create_dir(&a).unwrap();
    fs::write(a.join("x.NEF"), b"x").unwrap();
    // A plain rename replaces an empty directory; this one doesn't.
    fs::create_dir(&c).unwrap();
    assert!(matches!(
        rename_exclusive(&a, &c),
        Err(FsError::AlreadyExists)
    ));
    assert!(a.join("x.NEF").is_file());
    rename_exclusive(&a, &b).unwrap();
    assert!(b.join("x.NEF").is_file());
    assert!(c.is_dir());
}

#[test]
fn a_folder_is_made_only_where_nothing_is() {
    let d = dir();
    let (a, file, link) = (d.path().join("a"), d.path().join("f"), d.path().join("l"));
    create_dir(&a).unwrap();
    assert_eq!(entry(&a).unwrap().kind, EntryKind::Directory);
    // Never adopted: a folder, a file, or a link there already.
    fs::write(a.join("x.NEF"), b"x").unwrap();
    assert!(matches!(create_dir(&a), Err(FsError::AlreadyExists)));
    assert!(a.join("x.NEF").is_file());
    fs::write(&file, b"file").unwrap();
    assert!(matches!(create_dir(&file), Err(FsError::AlreadyExists)));
    assert_eq!(fs::read(&file).unwrap(), b"file");
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(d.path().join("nowhere"), &link).unwrap();
        assert!(matches!(create_dir(&link), Err(FsError::AlreadyExists)));
        assert!(!d.path().join("nowhere").exists());
    }
    let _ = link;
    // Its parent must be there; nothing is made on the way.
    assert!(matches!(
        create_dir(&d.path().join("b/c")),
        Err(FsError::NotFound)
    ));
    assert!(!d.path().join("b").exists());
}

#[test]
fn a_missing_source_is_not_found() {
    let d = dir();
    assert!(matches!(
        rename_exclusive(&d.path().join("gone"), &d.path().join("there")),
        Err(FsError::NotFound)
    ));
    assert!(matches!(
        entry(&d.path().join("gone")),
        Err(FsError::NotFound)
    ));
}

#[cfg(target_vendor = "apple")]
#[test]
fn a_case_only_rename_works_on_apfs() {
    let d = dir();
    let (upper, lower) = (d.path().join("ROUND.NEF"), d.path().join("round.NEF"));
    fs::write(&upper, b"r").unwrap();
    let id = entry(&upper).unwrap().id;
    rename_exclusive(&upper, &lower).unwrap();
    assert_eq!(list(d.path()).unwrap(), vec![OsString::from("round.NEF")]);
    assert_eq!(entry(&lower).unwrap().id, id);
}

#[cfg(target_vendor = "apple")]
#[test]
fn a_normalisation_only_rename_works_where_the_volume_renames_spelling() {
    let d = dir();
    let (nfc, nfd) = (
        d.path().join("caf\u{e9}.NEF"),
        d.path().join("cafe\u{301}.NEF"),
    );
    fs::write(&nfc, b"r").unwrap();
    let id = entry(&nfc).unwrap().id;
    if !volume_facts(d.path()).unwrap().renames_spelling {
        // Case-sensitive APFS: the other spelling is the same name, taken.
        assert!(matches!(
            rename_exclusive(&nfc, &nfd),
            Err(FsError::AlreadyExists)
        ));
        assert_eq!(
            list(d.path()).unwrap(),
            vec![OsString::from("caf\u{e9}.NEF")]
        );
        return;
    }
    rename_exclusive(&nfc, &nfd).unwrap();
    assert_eq!(
        list(d.path()).unwrap(),
        vec![OsString::from("cafe\u{301}.NEF")]
    );
    assert_eq!(entry(&nfd).unwrap().id, id);
}

#[test]
fn a_volume_says_whether_a_rename_can_change_only_a_names_spelling() {
    // Temporary folders are on APFS on macOS, which does unless it tells
    // case apart; elsewhere it isn't known, and no volume is taken to.
    let d = dir();
    fs::write(d.path().join("a"), b"").unwrap();
    let caseless = d.path().join("A").exists();
    let facts = volume_facts(d.path()).unwrap();
    assert_eq!(
        facts.renames_spelling,
        cfg!(target_vendor = "apple") && caseless
    );
}

#[test]
fn file_ids_tell_files_apart_and_hard_links_are_counted() {
    let d = dir();
    let (a, b, c) = (d.path().join("a"), d.path().join("b"), d.path().join("c"));
    fs::write(&a, b"same bytes").unwrap();
    fs::write(&b, b"same bytes").unwrap();
    assert_ne!(entry(&a).unwrap().id, entry(&b).unwrap().id);
    assert_eq!(entry(&a).unwrap().links, 1);
    fs::hard_link(&a, &c).unwrap();
    assert_eq!(entry(&c).unwrap().id, entry(&a).unwrap().id);
    assert_eq!(entry(&a).unwrap().links, 2);
    assert_eq!(entry(d.path()).unwrap().kind, EntryKind::Directory);
    assert_eq!(
        entry(&a).unwrap().id.volume(),
        entry(d.path()).unwrap().id.volume()
    );
}

#[test]
fn a_file_id_reads_back_from_its_text() {
    let d = dir();
    let a = d.path().join("a");
    fs::write(&a, b"a").unwrap();
    let id = entry(&a).unwrap().id;
    assert_eq!(id.to_string().parse::<FileId>().unwrap(), id);
    assert!("nonsense".parse::<FileId>().is_err());
    assert!("1:".parse::<FileId>().is_err());
}

#[cfg(unix)]
#[test]
fn an_entry_is_the_link_itself_not_what_it_points_at() {
    let d = dir();
    let (a, link) = (d.path().join("a"), d.path().join("link"));
    fs::write(&a, b"a").unwrap();
    std::os::unix::fs::symlink(&a, &link).unwrap();
    let e = entry(&link).unwrap();
    assert_eq!(e.kind, EntryKind::Symlink);
    assert_ne!(e.id, entry(&a).unwrap().id);
}

#[test]
fn a_listing_has_every_name_exactly() {
    let d = dir();
    for name in ["A.NEF", "a.xmp", "B.dng"] {
        fs::write(d.path().join(name), b"").unwrap();
    }
    let mut names = list(d.path()).unwrap();
    names.sort();
    assert_eq!(names, ["A.NEF", "B.dng", "a.xmp"].map(OsString::from));
    assert!(matches!(
        list(&d.path().join("gone")),
        Err(FsError::NotFound)
    ));
}

#[test]
fn a_folder_is_synced() {
    let d = dir();
    sync_dir(d.path()).unwrap();
    assert!(matches!(
        sync_dir(&d.path().join("gone")),
        Err(FsError::NotFound)
    ));
}

#[test]
fn an_empty_folder_says_nothing_about_its_names() {
    let d = dir();
    let rules = probe(d.path(), &list(d.path()).unwrap());
    assert_eq!(rules.case, Folding::Unknown);
    assert_eq!(rules.normalization, Folding::Unknown);
}

#[test]
fn a_folder_is_probed_through_its_own_names() {
    let d = dir();
    fs::write(d.path().join("Photo.NEF"), b"").unwrap();
    // NFC "é", which a normalising volume also finds as NFD.
    fs::write(d.path().join("caf\u{e9}.NEF"), b"").unwrap();
    let rules = probe(d.path(), &list(d.path()).unwrap());
    // APFS and NTFS as formatted by default fold case; a case-sensitive
    // APFS volume doesn't.
    let caseless = d.path().join("PHOTO.NEF").exists();
    if cfg!(any(target_vendor = "apple", windows)) && caseless {
        assert_eq!(rules.case, Folding::Insensitive);
    } else {
        assert_eq!(rules.case, Folding::Exact);
    }
    if cfg!(target_vendor = "apple") {
        assert_eq!(rules.normalization, Folding::Insensitive);
    }
}

#[test]
fn a_photo_below_its_root_is_contained() {
    let d = dir();
    let root = d.path().join("root");
    fs::create_dir_all(root.join("Trip/Day 1")).unwrap();
    fs::write(root.join("Trip/Day 1/a.NEF"), b"").unwrap();
    let canonical = canonical_below(&root, &root.join("Trip/Day 1/a.NEF")).unwrap();
    // Name by name: a canonical Windows path (`\\?\`) has no `/`.
    assert_eq!(
        canonical,
        fs::canonicalize(&root)
            .unwrap()
            .join("Trip")
            .join("Day 1")
            .join("a.NEF")
    );
    assert_eq!(
        canonical_below(&root, &root).unwrap(),
        fs::canonicalize(&root).unwrap()
    );
    assert!(matches!(
        canonical_below(&root, &d.path().join("elsewhere")),
        Err(Containment::Outside)
    ));
    assert!(matches!(
        canonical_below(&root, &root.join("Trip/../Trip/Day 1/a.NEF")),
        Err(Containment::Outside)
    ));
    assert!(matches!(
        canonical_below(&root, &root.join("Trip/gone.NEF")),
        Err(Containment::Fs(FsError::NotFound))
    ));
}

#[cfg(unix)]
#[test]
fn a_symlink_between_root_and_photo_is_not_contained() {
    let d = dir();
    let root = d.path().join("root");
    let real = d.path().join("real");
    fs::create_dir_all(&root).unwrap();
    fs::create_dir_all(&real).unwrap();
    fs::write(real.join("a.NEF"), b"").unwrap();
    std::os::unix::fs::symlink(&real, root.join("linked")).unwrap();
    std::os::unix::fs::symlink(real.join("a.NEF"), root.join("b.NEF")).unwrap();
    assert!(matches!(
        canonical_below(&root, &root.join("linked/a.NEF")),
        Err(Containment::Symlink(p)) if p.ends_with("linked")
    ));
    assert!(matches!(
        canonical_below(&root, &root.join("b.NEF")),
        Err(Containment::Symlink(p)) if p.ends_with("b.NEF")
    ));
    // A root reached through a link is its canonical self.
    let alias = d.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    assert_eq!(
        canonical_below(&alias, &alias.join("a.NEF")).unwrap(),
        fs::canonicalize(&real).unwrap().join("a.NEF")
    );
}

#[test]
fn a_temporary_folder_is_on_a_local_writable_volume() {
    let d = dir();
    let facts = volume_facts(d.path()).unwrap();
    assert!(facts.local);
    assert!(!facts.read_only);
    assert!(facts.available > 0, "a volume tests write on has room");
    // APFS and NTFS keep file ids; Linux's temporary folder may be tmpfs.
    if cfg!(any(target_vendor = "apple", windows)) {
        assert!(facts.stable_ids, "{facts:?}");
    }
}

#[test]
fn a_file_is_made_empty_and_only_where_nothing_is() {
    let d = dir();
    let (a, b) = (d.path().join("a.part"), d.path().join("b.NEF"));
    create_file(&a).unwrap();
    assert_eq!(fs::read(&a).unwrap(), b"");
    fs::write(&b, b"kept").unwrap();
    assert!(matches!(create_file(&b), Err(FsError::AlreadyExists)));
    assert!(matches!(create_file(&a), Err(FsError::AlreadyExists)));
    assert_eq!(fs::read(&b).unwrap(), b"kept", "never truncated");
    assert!(matches!(
        create_file(&d.path().join("no/c")),
        Err(FsError::NotFound)
    ));
}

/// Bytes over several copy chunks, distinct per position.
fn big() -> Vec<u8> {
    (0..2 * COPY_CHUNK as u32 + 77)
        .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
        .collect()
}

#[test]
fn a_copy_has_every_byte_and_the_hash_of_what_was_read() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".a.part"));
    let bytes = big();
    fs::write(&a, &bytes).unwrap();
    let mut into = create_file(&b).unwrap();
    let mut seen = Vec::new();
    let hash = copy_into(&a, &mut into, &mut |n| {
        seen.push(n);
        true
    })
    .unwrap();
    assert_eq!(fs::read(&b).unwrap(), bytes);
    assert_eq!(hash, hash_file(&a).unwrap());
    assert_eq!(hash, hash_file(&b).unwrap());
    let chunk = COPY_CHUNK as u64;
    assert_eq!(seen, [chunk, 2 * chunk, 2 * chunk + 77]);
    assert_eq!(fs::read(&a).unwrap(), bytes, "the source is untouched");
    // SHA-256 of nothing, as journals keep it, and back.
    fs::write(&a, b"").unwrap();
    let empty = hash_file(&a).unwrap();
    assert_eq!(
        empty.to_string(),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
    assert_eq!(empty.to_string().parse::<ContentHash>().unwrap(), empty);
    assert!("e3b0".parse::<ContentHash>().is_err());
}

#[test]
fn a_copy_stops_between_chunks_when_asked() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".a.part"));
    fs::write(&a, big()).unwrap();
    let mut into = create_file(&b).unwrap();
    let mut calls = 0;
    let stopped = copy_into(&a, &mut into, &mut |_| {
        calls += 1;
        false
    });
    assert!(matches!(stopped, Err(CopyError::Stopped)));
    assert_eq!(calls, 1);
    assert_eq!(fs::metadata(&b).unwrap().len(), COPY_CHUNK as u64);
}

#[test]
fn a_copy_goes_only_into_the_empty_file_it_was_given() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".b.part"));
    fs::write(&a, b"synthetic raw").unwrap();
    // A file with bytes in it already.
    let mut into = create_file(&b).unwrap();
    fs::write(&b, b"x").unwrap();
    assert!(matches!(
        copy_into(&a, &mut into, &mut |_| true),
        Err(CopyError::NotTheFile)
    ));
    assert_eq!(fs::read(&b).unwrap(), b"x");
}

/// What is copied goes into the file made, never into one another program
/// or computer put at its name since (two mounts of an SMB share, where
/// names prove files, showed one's bytes overwritten, #573). Unix only:
/// Windows may keep a removed file's name while a handle to it is open.
#[cfg(unix)]
#[test]
fn a_copy_goes_into_the_file_made_whatever_is_put_at_its_name() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".b.part"));
    fs::write(&a, b"synthetic raw").unwrap();
    let mut into = create_file(&b).unwrap();
    fs::remove_file(&b).unwrap();
    fs::write(&b, b"another's").unwrap();
    let copied = copy_into(&a, &mut into, &mut |_| true);
    assert!(copied.is_ok(), "{copied:?}");
    assert_eq!(fs::read(&b).unwrap(), b"another's");
}

#[test]
fn a_copy_takes_the_sources_time_and_permissions() {
    let d = dir();
    let (a, b) = (d.path().join("a.NEF"), d.path().join(".a.part"));
    fs::write(&a, b"synthetic raw").unwrap();
    let when = std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, 123_456_700);
    fs::File::options()
        .write(true)
        .open(&a)
        .unwrap()
        .set_modified(when)
        .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&a, fs::Permissions::from_mode(0o440)).unwrap();
    }
    let mut into = create_file(&b).unwrap();
    copy_into(&a, &mut into, &mut |_| true).unwrap();
    sync_file(&into).unwrap();
    copy_times(&a, &b, Some(&into)).unwrap();
    // Read-only now, and still synced through the handle it was made by.
    sync_file(&into).unwrap();
    drop(into);
    let (source, copy) = (fs::metadata(&a).unwrap(), fs::metadata(&b).unwrap());
    assert_eq!(copy.modified().unwrap(), source.modified().unwrap());
    assert_eq!(copy.modified().unwrap(), when);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(copy.permissions().mode() & 0o777, 0o440);
    }
    assert_eq!(fs::read(&b).unwrap(), b"synthetic raw");
    // Only an unfinished copy is removed, by its caller's proof.
    remove_file(&b).unwrap();
    assert!(!b.exists() && a.is_file());
}

#[test]
fn errors_say_what_callers_branch_on() {
    use std::io::{Error, ErrorKind};
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::AlreadyExists)),
        FsError::AlreadyExists
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::NotFound)),
        FsError::NotFound
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::PermissionDenied)),
        FsError::PermissionDenied
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::CrossesDevices)),
        FsError::CrossDevice
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::Unsupported)),
        FsError::Unsupported
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::StorageFull)),
        FsError::NoSpace
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::QuotaExceeded)),
        FsError::NoSpace
    ));
    assert!(matches!(
        FsError::from(Error::from(ErrorKind::TimedOut)),
        FsError::Io(_)
    ));
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_FAT_VOLUMES: folders on FAT or exFAT volumes, `:` between"]
fn fat_and_exfat_volumes_are_told_and_have_no_stable_ids() {
    let Some(dirs) = std::env::var_os("RAWMAKASE_TEST_FAT_VOLUMES") else {
        return;
    };
    for dir in std::env::split_paths(&dirs) {
        let facts = volume_facts(&dir).unwrap();
        let shown = format!("{}: {facts:?}", dir.display());
        assert!(!facts.stable_ids, "{shown}");
        assert!(
            matches!(facts.kind, VolumeKind::Fat | VolumeKind::Exfat),
            "{shown}"
        );
        // What a volume says it can't rename exclusively, it can't.
        let d = tempfile::tempdir_in(&dir).unwrap();
        let (a, b) = (d.path().join("a.part"), d.path().join("b.NEF"));
        fs::write(&a, b"a").unwrap();
        match rename_exclusive(&a, &b) {
            Ok(()) => assert!(facts.exclusive_rename, "{shown}"),
            Err(FsError::Unsupported) => assert!(!facts.exclusive_rename, "{shown}"),
            Err(e) => panic!("{shown}: {e}"),
        }
        // A file made has a birth time, near now.
        let c = d.path().join("c.NEF");
        let into = create_file(&c).unwrap();
        let born = created(&c).unwrap();
        let off = born
            .duration_since(std::time::SystemTime::now())
            .unwrap_or_else(|e| e.duration());
        assert!(off < std::time::Duration::from_secs(5), "{shown}: {off:?}");
        // Extras are kept there only on macOS, in AppleDouble files; a
        // folder takes its dates.
        let home = tempfile::tempdir().unwrap();
        let tagged = home.path().join("tagged");
        fs::write(&tagged, b"t").unwrap();
        set_extra(&tagged, "tag", b"Red").unwrap();
        let kept = copy_extras(&tagged, &c, Some(&into));
        if cfg!(target_vendor = "apple") {
            assert!(kept.is_ok(), "{shown}: {kept:?}");
            assert_eq!(extras(&c).unwrap(), extras(&tagged).unwrap(), "{shown}");
        } else {
            assert!(
                matches!(kept, Err(FsError::Unsupported)),
                "{shown}: {kept:?}"
            );
        }
        let folder = d.path().join("folder");
        create_dir(&folder).unwrap();
        copy_times(home.path(), &folder, None).unwrap();
    }
}

/// ntfs-3g's files have their disk's device number; a FUSE file system
/// without a disk (GVFS's shares, sshfs) has an anonymous one, and only
/// files on a disk are renamed by a link and an unlink (#914).
#[cfg(any(target_os = "linux", target_os = "android"))]
#[test]
fn only_fuse_on_a_disk_renames_by_link_and_unlink() {
    use super::unix::on_disk_fuse;
    use rustix::fs::makedev;
    const FUSE: u64 = 0x6573_5546;
    assert!(on_disk_fuse(FUSE, makedev(259, 3)));
    assert!(on_disk_fuse(FUSE, makedev(8, 17)));
    assert!(!on_disk_fuse(FUSE, makedev(0, 52)));
    assert!(!on_disk_fuse(0xef53, makedev(259, 3)));
}

/// The names in folder `dir`, sorted.
fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// A rename by placeholder, as on ntfs-3g (#914): never onto a file,
/// keeping the file object, with nothing else left.
#[test]
fn a_rename_by_placeholder_keeps_the_file_and_leaves_one_name() {
    let d = dir();
    let (a, b, c) = (
        d.path().join("a.ARW"),
        d.path().join("b.ARW"),
        d.path().join("c.ARW"),
    );
    fs::write(&a, b"a").unwrap();
    fs::write(&b, b"b").unwrap();
    let id = entry(&a).unwrap().id;
    assert!(matches!(
        rename_by_placeholder(&a, &b, &mut || Ok(())),
        Err(FsError::AlreadyExists)
    ));
    assert_eq!(names_in(d.path()), ["a.ARW", "b.ARW"]);
    assert_eq!(fs::read(&b).unwrap(), b"b");
    rename_by_placeholder(&a, &c, &mut || Ok(())).unwrap();
    assert_eq!(names_in(d.path()), ["b.ARW", "c.ARW"]);
    let moved = entry(&c).unwrap();
    assert_eq!((moved.id, moved.links), (id, 1));
    assert_eq!(fs::read(&c).unwrap(), b"a");
}

/// A file another program puts in the placeholder's place, empty or not,
/// is kept, and the rename refused; a rename that fails once the
/// placeholder is made, or whose old name went meanwhile, removes it.
#[test]
fn a_rename_by_placeholder_keeps_a_file_put_in_its_place() {
    for put in [&b"put at c"[..], b""] {
        let d = dir();
        let (a, c) = (d.path().join("a.ARW"), d.path().join("c.ARW"));
        fs::write(&a, b"a").unwrap();
        let renamed = rename_by_placeholder(&a, &c, &mut || {
            let other = d.path().join("c.new");
            fs::write(&other, put).unwrap();
            fs::rename(&other, &c).unwrap();
            Ok(())
        });
        assert!(
            matches!(renamed, Err(FsError::AlreadyExists)),
            "{renamed:?}"
        );
        assert_eq!(names_in(d.path()), ["a.ARW", "c.ARW"]);
        assert_eq!(fs::read(&a).unwrap(), b"a");
        assert_eq!(fs::read(&c).unwrap(), put);
    }
    let d = dir();
    let (a, c) = (d.path().join("a.ARW"), d.path().join("c.ARW"));
    fs::write(&a, b"a").unwrap();
    let renamed = rename_by_placeholder(&a, &c, &mut || {
        Err(FsError::Io(std::io::Error::other("injected")))
    });
    assert!(matches!(renamed, Err(FsError::Io(_))), "{renamed:?}");
    assert_eq!(names_in(d.path()), ["a.ARW"]);
    let renamed = rename_by_placeholder(&a, &c, &mut || {
        fs::rename(&a, d.path().join("moved.ARW")).unwrap();
        Ok(())
    });
    assert!(matches!(renamed, Err(FsError::NotFound)), "{renamed:?}");
    assert_eq!(names_in(d.path()), ["moved.ARW"]);
}

/// A staged file that can't be removed stops the rename before it
/// happens, and the placeholder goes; one of another's stays (#914
/// review).
#[test]
fn a_staged_file_left_in_the_way_stops_the_rename() {
    let d = dir();
    let (a, c) = (d.path().join("a.ARW"), d.path().join("c.ARW"));
    fs::write(&a, b"a").unwrap();
    let stage = placeholder_staged(&a, &c);
    let renamed = rename_by_placeholder(&a, &c, &mut || {
        // As if removing it failed: something else is there instead.
        fs::remove_file(&stage).unwrap();
        fs::create_dir(&stage).unwrap();
        Ok(())
    });
    assert!(matches!(renamed, Err(FsError::Io(_))), "{renamed:?}");
    let stage_name = stage.file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(names_in(d.path()), [stage_name, "a.ARW".into()]);
    assert_eq!(fs::read(&a).unwrap(), b"a");
}

/// A placeholder a crash left, and the file it is written to first, whole
/// or cut short, are removed, either way round; the file is where it was.
#[test]
fn what_a_rename_by_placeholder_cut_short_left_is_removed() {
    let d = dir();
    let (a, c) = (d.path().join("a.ARW"), d.path().join("c.ARW"));
    fs::write(&a, b"a").unwrap();
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = rename_by_placeholder(&a, &c, &mut || panic!("crash"));
    }));
    assert!(crashed.is_err());
    let stage = placeholder_staged(&a, &c);
    let staged = stage.file_name().unwrap().to_string_lossy().into_owned();
    let left = [staged.as_str(), "a.ARW", "c.ARW"];
    assert_eq!(names_in(d.path()), left);
    // Settled the other way round, it isn't that rename's.
    settle_placeholder(&c, &a).unwrap();
    assert_eq!(names_in(d.path()), left);
    settle_placeholder(&a, &c).unwrap();
    assert_eq!(names_in(d.path()), ["a.ARW"]);
    assert_eq!(fs::read(&a).unwrap(), b"a");
    // The file it is written to first, as a crash or a full drive leaves
    // it: empty, part written, or whole.
    for left in [&b""[..], b"RAWma", b"RAWmakase keeps this name"] {
        fs::write(&stage, left).unwrap();
        settle_placeholder(&a, &c).unwrap();
        assert_eq!(names_in(d.path()), ["a.ARW"], "{left:?}");
    }
    // Left whole, it doesn't stop the rename.
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = rename_by_placeholder(&a, &c, &mut || panic!("crash"));
    }));
    assert!(crashed.is_err());
    rename_by_placeholder(&a, &c, &mut || Ok(())).unwrap();
    assert_eq!(names_in(d.path()), ["c.ARW"]);
}

/// Only what a rename by placeholder made is ever removed: never a photo's
/// file, empty or not, nor a placeholder for another rename, nor a file of
/// another program's at the hidden name, which stops the rename instead.
#[test]
fn settling_a_rename_by_placeholder_removes_nothing_else() {
    let d = dir();
    let (a, c) = (d.path().join("a.ARW"), d.path().join("c.ARW"));
    for (what, at_a, at_c) in [
        ("an empty file at the new name", &b"a"[..], &b""[..]),
        ("both empty", b"", b""),
        ("a file at the new name", b"a", b"c"),
    ] {
        fs::write(&a, at_a).unwrap();
        fs::write(&c, at_c).unwrap();
        settle_placeholder(&a, &c).unwrap();
        settle_placeholder(&c, &a).unwrap();
        assert_eq!(names_in(d.path()), ["a.ARW", "c.ARW"], "{what}");
        fs::remove_file(&c).unwrap();
    }
    // Put back part way, as a crash leaves it: the photo's empty file at
    // its new name, a placeholder at its old one. Only the placeholder
    // goes, whichever way round it is settled first.
    fs::remove_file(&a).unwrap();
    fs::write(&c, b"").unwrap();
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = rename_by_placeholder(&c, &a, &mut || panic!("crash"));
    }));
    assert!(crashed.is_err());
    settle_placeholder(&a, &c).unwrap();
    settle_placeholder(&c, &a).unwrap();
    assert_eq!(names_in(d.path()), ["c.ARW"]);
    assert_eq!(fs::read(&c).unwrap(), b"");
    // Another rename's placeholder stays.
    let other = d.path().join("other.ARW");
    fs::write(&other, b"o").unwrap();
    let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = rename_by_placeholder(&other, &a, &mut || panic!("crash"));
    }));
    assert!(crashed.is_err());
    settle_placeholder(&c, &a).unwrap();
    assert!(a.exists());
    settle_placeholder(&other, &a).unwrap();
    assert!(!a.exists());
    // A file of its own at the hidden name stays, and the rename is
    // refused.
    let stage = placeholder_staged(&c, &a);
    fs::write(&stage, b"someone else's").unwrap();
    settle_placeholder(&c, &a).unwrap();
    assert!(matches!(
        rename_by_placeholder(&c, &a, &mut || Ok(())),
        Err(FsError::Io(_))
    ));
    assert_eq!(fs::read(&stage).unwrap(), b"someone else's");
    assert_eq!(fs::read(&c).unwrap(), b"");
    assert!(!a.exists());
}

/// Where no rename is by a placeholder (any volume but FUSE on a disk on
/// Linux), [`settle_rename`] leaves even a placeholder as it is.
#[test]
fn settling_a_rename_changes_nothing_on_other_volumes() {
    let d = dir();
    let (a, c) = (d.path().join("a.ARW"), d.path().join("c.ARW"));
    fs::write(&a, b"a").unwrap();
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = rename_by_placeholder(&a, &c, &mut || panic!("crash"));
    }));
    settle_rename(&a, &c).unwrap();
    assert_eq!(names_in(d.path()).len(), 3);
}

/// Linux's NTFS drivers rename photos without replacing a file: ntfs3 and
/// `ntfs` by `RENAME_NOREPLACE`, ntfs-3g (FUSE, which answers `EINVAL` to
/// it) by a placeholder (#914).
#[test]
#[ignore = "needs RAWMAKASE_TEST_NTFS_VOLUMES: folders on NTFS volumes, `:` between"]
fn ntfs_volumes_rename_without_replacing_and_keep_the_file_object() {
    let Some(dirs) = std::env::var_os("RAWMAKASE_TEST_NTFS_VOLUMES") else {
        return;
    };
    for dir in std::env::split_paths(&dirs) {
        let facts = volume_facts(&dir).unwrap();
        let shown = format!("{}: {facts:?}", dir.display());
        let d = tempfile::tempdir_in(&dir).unwrap();
        let (a, b, c) = (
            d.path().join("a.ARW"),
            d.path().join("b.ARW"),
            d.path().join("c.ARW"),
        );
        fs::write(&a, b"a").unwrap();
        fs::write(&b, b"b").unwrap();
        let id = entry(&a).unwrap().id;
        assert!(
            matches!(rename_reserving(&a, &b), Err(FsError::AlreadyExists)),
            "{shown}"
        );
        assert_eq!(names_in(d.path()), ["a.ARW", "b.ARW"], "{shown}");
        assert_eq!(fs::read(&b).unwrap(), b"b", "{shown}");
        rename_reserving(&a, &c).unwrap();
        assert_eq!(names_in(d.path()), ["b.ARW", "c.ARW"], "{shown}");
        let moved = entry(&c).unwrap();
        assert_eq!((moved.id, moved.links), (id, 1), "{shown}");
        // A placeholder a crash left there is removed by recovery's call.
        let crashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = rename_by_placeholder(&b, &a, &mut || panic!("crash"));
        }));
        assert!(crashed.is_err());
        settle_rename(&b, &a).unwrap();
        // Only where renames are by placeholder (ntfs-3g) is it removed,
        // with the file it was staged in.
        let left = names_in(d.path());
        let staged = placeholder_staged(&b, &a);
        let staged = staged.file_name().unwrap().to_string_lossy().into_owned();
        match rename_exclusive(&b, &d.path().join("t.ARW")) {
            Err(FsError::Unsupported) => assert_eq!(left, ["b.ARW", "c.ARW"], "{shown}"),
            done => {
                done.unwrap();
                let all = [staged.as_str(), "a.ARW", "b.ARW", "c.ARW"];
                assert_eq!(left, all, "{shown}");
            }
        }
    }
}

#[test]
#[ignore = "needs RAWMAKASE_TEST_NETWORK_SHARE: a writable folder on an SMB share"]
fn an_smb_share_is_told_and_makes_files_only_where_none_is() {
    let Some(share) = std::env::var_os("RAWMAKASE_TEST_NETWORK_SHARE") else {
        return;
    };
    let d = tempfile::tempdir_in(share).unwrap();
    let facts = volume_facts(d.path()).unwrap();
    let shown = format!("{}: {facts:?}", d.path().display());
    assert_eq!(facts.kind, VolumeKind::Smb, "{shown}");
    assert!(!facts.local && !facts.stable_ids, "{shown}");
    assert!(!facts.exclusive_rename, "{shown}");
    // It says it can't rename exclusively, and changes nothing: macOS's
    // smbfs answers so, and Linux's CIFS, or the server Windows asks, would
    // replace a file another computer just put at the name.
    let (a, b) = (d.path().join("a.NEF"), d.path().join("b.NEF"));
    fs::write(&a, b"a").unwrap();
    assert!(matches!(
        rename_exclusive(&a, &b),
        Err(FsError::Unsupported)
    ));
    assert_eq!(fs::read(&a).unwrap(), b"a");
    assert!(!b.exists());
    // A file is made only where nothing has its name, in any case or form.
    assert!(matches!(create_file(&a), Err(FsError::AlreadyExists)));
    assert!(matches!(
        create_file(&d.path().join("A.nef")),
        Err(FsError::AlreadyExists)
    ));
    assert_eq!(fs::read(&a).unwrap(), b"a");
    let c = d.path().join("caf\u{e9}.NEF");
    create_file(&c).unwrap();
    // macOS's smbfs finds a name by either form; Linux's CIFS and Windows
    // send its bytes, which a server may tell apart (Samba 4.22 and a
    // Synology do).
    if cfg!(target_vendor = "apple") {
        assert!(matches!(
            create_file(&d.path().join("cafe\u{301}.NEF")),
            Err(FsError::AlreadyExists)
        ));
    }
    // With a birth time near now, as the server keeps it.
    let off = created(&c)
        .unwrap()
        .duration_since(std::time::SystemTime::now())
        .unwrap_or_else(|e| e.duration());
    assert!(off < std::time::Duration::from_secs(5), "{shown}: {off:?}");
    // A copy onto it takes each step a move does. A flush through a
    // handle opened only to read is refused there (EACCES on macOS 15).
    let local = dir();
    let source = local.path().join("s.NEF");
    fs::write(&source, b"synthetic raw").unwrap();
    let copy = d.path().join("s.NEF");
    let mut into = create_file(&copy).unwrap();
    set_extra(&source, "com.rawmakase.test", b"synthetic").unwrap();
    let content = copy_into(&source, &mut into, &mut |_| true).unwrap();
    sync_file(&into).unwrap();
    // Its extras, as a move gives them, between the two; on Windows none
    // where the server keeps no alternate data streams (Samba without
    // `streams_xattr`), which a move says it can't keep.
    match copy_extras(&source, &copy, Some(&into)) {
        Ok(()) => assert_eq!(extras(&copy).unwrap(), extras(&source).unwrap()),
        Err(FsError::Unsupported) if cfg!(windows) => {}
        Err(e) => panic!("{shown}: {e}"),
    }
    copy_times(&source, &copy, Some(&into)).unwrap();
    drop(into);
    assert_eq!(hash_file(&copy).unwrap(), content);
    // Its folders sync, though they take no full sync (ENOTSUP).
    sync_dir(d.path()).unwrap();
    // With the source's time, to the 100 ns the server keeps, once its
    // handles close (a flush after the times would lose them then).
    std::thread::sleep(std::time::Duration::from_secs(3));
    hash_file(&copy).unwrap();
    let ns = |p: &std::path::Path| {
        let t = fs::metadata(p).unwrap().modified().unwrap();
        t.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    };
    let (kept, was) = (ns(&copy), ns(&source));
    assert!(kept.abs_diff(was) < 100, "{shown}: {kept} {was}");
    // Not tested here: what another computer puts at a copy's name while it
    // is finished. Linux's CIFS sends an extended attribute by its name
    // whatever handle it is given, and times too once it has looked the
    // name up again: through a second mount, another file put there took
    // both (Linux 7.2 against Samba 4.22, #675).
}

/// What a copy is given after its bytes goes to the file made, through
/// the handle it was made through, never to another file put at its name
/// meanwhile (#675); but for an alternate data stream on Windows, written
/// by path. (Linux's CIFS doesn't keep to the handle: see
/// `an_smb_share_is_told_and_makes_files_only_where_none_is`.)
#[test]
fn a_copy_is_finished_through_its_handle_whatever_is_at_its_name() {
    let d = dir();
    let (source, at) = (d.path().join("a.NEF"), d.path().join("b.NEF"));
    fs::write(&source, b"synthetic raw").unwrap();
    // Tagged first: a stream written on Windows changes the file's time.
    let _ = tagged(&source, "tag", b"Red");
    let when = std::time::UNIX_EPOCH + std::time::Duration::new(1_600_000_000, 0);
    let opened = fs::File::options().write(true).open(&source).unwrap();
    opened.set_modified(when).unwrap();
    drop(opened);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&source, fs::Permissions::from_mode(0o640)).unwrap();
    }
    let mut into = create_file(&at).unwrap();
    copy_into(&source, &mut into, &mut |_| true).unwrap();
    // Another file takes its name.
    let aside = d.path().join("b.aside");
    fs::rename(&at, &aside).unwrap();
    fs::write(&at, b"theirs").unwrap();
    let theirs = extras(&at).unwrap();
    sync_file(&into).unwrap();
    copy_extras(&source, &at, Some(&into)).unwrap();
    copy_times(&source, &at, Some(&into)).unwrap();
    drop(into);
    let modified = |p: &Path| fs::metadata(p).unwrap().modified().unwrap();
    assert_eq!(fs::read(&at).unwrap(), b"theirs");
    assert_ne!(modified(&at), when);
    assert_eq!(modified(&aside), when);
    if !cfg!(windows) {
        assert_eq!(extras(&at).unwrap(), theirs);
        assert_eq!(extras(&aside).unwrap(), extras(&source).unwrap());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&aside), 0o640);
        assert_ne!(mode(&at), 0o640);
    }
}

#[test]
fn a_volume_kind_is_told_from_its_file_system_name() {
    #[cfg(target_vendor = "apple")]
    {
        use super::unix::kind_of;
        assert_eq!(kind_of(b"apfs"), VolumeKind::Apfs);
        assert_eq!(kind_of(b"hfs"), VolumeKind::Hfs);
        assert_eq!(kind_of(b"msdos"), VolumeKind::Fat);
        assert_eq!(kind_of(b"exfat"), VolumeKind::Exfat);
        assert_eq!(kind_of(b"smbfs"), VolumeKind::Smb);
        for other in [&b"macfuse"[..], b"nfs", b"webdav", b"afpfs", b""] {
            assert_eq!(kind_of(other), VolumeKind::Other, "{other:?}");
        }
        // exFAT on macOS answers ENOTSUP to an exclusive rename (measured
        // on macOS 15).
        assert!(!VolumeKind::Exfat.exclusive_rename());
        assert!(VolumeKind::Fat.exclusive_rename());
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use super::unix::kind_of;
        assert_eq!(kind_of(0xef53), VolumeKind::Ext);
        assert_eq!(kind_of(0x5846_5342), VolumeKind::Xfs);
        assert_eq!(kind_of(0x9123_683e), VolumeKind::Btrfs);
        assert_eq!(kind_of(0x4d44), VolumeKind::Fat);
        assert_eq!(kind_of(0x2011_bab0), VolumeKind::Exfat);
        // CIFS, SMB2 and the old SMB.
        for smb in [0xff53_4d42, 0xfe53_4d42, 0x517b] {
            assert_eq!(kind_of(smb), VolumeKind::Smb, "{smb:x}");
        }
        for other in [0x6573_5546, 0x0102_1994, 0x6969] {
            assert_eq!(kind_of(other), VolumeKind::Other, "{other:x}");
        }
    }
    #[cfg(windows)]
    {
        use super::windows::kind_of;
        assert_eq!(kind_of("NTFS"), VolumeKind::Ntfs);
        for fat in ["FAT", "FAT32", "fat32"] {
            assert_eq!(kind_of(fat), VolumeKind::Fat, "{fat}");
        }
        assert_eq!(kind_of("exFAT"), VolumeKind::Exfat);
        for other in ["ReFS", ""] {
            assert_eq!(kind_of(other), VolumeKind::Other, "{other}");
        }
        // A network drive by its protocol, whatever file system its server
        // names (`NTFS`, from Samba and a Synology): SMB, or another (a
        // WebDAV or NFS drive, `WNNC_NET_DAV` and `WNNC_NET_MS_NFS`).
        use super::windows::remote_kind;
        assert_eq!(remote_kind(Some(0x0002_0000)), VolumeKind::Smb);
        for other in [Some(0x002e_0000), Some(0x0042_0000), None] {
            assert_eq!(remote_kind(other), VolumeKind::Other, "{other:?}");
        }
    }
    // Only these keep a file's id while it is on them.
    for kind in [
        VolumeKind::Apfs,
        VolumeKind::Hfs,
        VolumeKind::Ext,
        VolumeKind::Xfs,
        VolumeKind::Btrfs,
        VolumeKind::Ntfs,
    ] {
        assert!(kind.keeps_ids(), "{kind:?}");
    }
    for kind in [
        VolumeKind::Fat,
        VolumeKind::Exfat,
        VolumeKind::Smb,
        VolumeKind::Other,
    ] {
        assert!(!kind.keeps_ids(), "{kind:?}");
    }
    // An SMB share can't promise a rename won't replace a file another
    // computer put at the name: macOS's smbfs answers ENOTSUP (measured on
    // macOS 15), Linux's CIFS checks the name only on this computer, and
    // the servers Windows asked not to replace did (#691).
    assert!(!VolumeKind::Smb.exclusive_rename());
}

#[test]
fn a_file_s_birth_time_and_its_first_bytes_are_read() {
    let d = dir();
    let a = d.path().join("a.NEF");
    let before = std::time::SystemTime::now() - std::time::Duration::from_secs(2);
    fs::write(&a, b"0123456789").unwrap();
    assert!(created(&a).unwrap() >= before);
    fs::write(d.path().join("head"), b"0123").unwrap();
    assert_eq!(
        hash_prefix(&a, 4).unwrap(),
        hash_file(&d.path().join("head")).unwrap()
    );
    // Asked for more than it has, it hashes what it has.
    assert_eq!(hash_prefix(&a, 100).unwrap(), hash_file(&a).unwrap());
}

#[test]
fn only_an_empty_folder_is_removed() {
    let d = dir();
    let empty = d.path().join("empty");
    let full = d.path().join("full");
    fs::create_dir(&empty).unwrap();
    fs::create_dir(&full).unwrap();
    fs::write(full.join("kept.NEF"), b"kept").unwrap();
    fs::write(d.path().join("file.NEF"), b"file").unwrap();
    remove_dir(&empty).unwrap();
    assert!(!empty.exists());
    assert!(remove_dir(&full).is_err());
    assert_eq!(fs::read(full.join("kept.NEF")).unwrap(), b"kept");
    assert!(remove_dir(&d.path().join("file.NEF")).is_err());
    assert!(d.path().join("file.NEF").is_file());
    assert!(matches!(remove_dir(&empty), Err(FsError::NotFound)));
}

/// Gives `path` the extra `name` for a test, or says why the temporary
/// folder's file system can't keep one (overlayfs in some containers).
fn tagged(path: &Path, name: &str, value: &[u8]) -> bool {
    match set_extra(path, name, value) {
        Ok(()) => true,
        Err(FsError::Unsupported) => {
            eprintln!("{}: no extended attributes here", path.display());
            false
        }
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

#[test]
fn a_copy_carries_its_originals_extras_and_tells_them_apart() {
    let d = dir();
    let (a, b, c) = (
        d.path().join("a.NEF"),
        d.path().join(".a.part"),
        d.path().join("c.NEF"),
    );
    fs::write(&a, b"synthetic raw").unwrap();
    fs::write(&c, b"synthetic raw").unwrap();
    let into = create_file(&b).unwrap();
    // None to carry: the same as a file with none.
    copy_extras(&a, &b, Some(&into)).unwrap();
    assert_eq!(extras(&a).unwrap(), extras(&b).unwrap());
    if !tagged(&a, "tag", b"Red") || !tagged(&a, "where", &[0, 1, 2, 255]) {
        return;
    }
    assert_ne!(extras(&a).unwrap(), extras(&b).unwrap());
    copy_extras(&a, &b, Some(&into)).unwrap();
    assert_eq!(extras(&a).unwrap(), extras(&b).unwrap());
    // Another value, or another name, is another set.
    assert!(tagged(&c, "tag", b"Blue") && tagged(&c, "where", &[0, 1, 2, 255]));
    assert_ne!(extras(&a).unwrap(), extras(&c).unwrap());
    assert!(tagged(&c, "tag", b"Red") && tagged(&c, "else", b""));
    assert_ne!(extras(&a).unwrap(), extras(&c).unwrap());
    // The contents are the copy's own.
    assert_eq!(fs::read(&b).unwrap(), b"");
}

#[test]
fn a_folder_takes_its_originals_extras_dates_and_permissions() {
    let d = dir();
    let (a, b) = (d.path().join("trip"), d.path().join(".trip.part"));
    fs::create_dir(&a).unwrap();
    fs::write(a.join("a.NEF"), b"synthetic raw").unwrap();
    create_dir(&b).unwrap();
    let carried = tagged(&a, "tag", b"Green");
    let when = std::time::UNIX_EPOCH + std::time::Duration::new(1_500_000_000, 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&a, fs::Permissions::from_mode(0o750)).unwrap();
    }
    super::os::open_folder(&a)
        .unwrap()
        .set_modified(when)
        .unwrap();
    let when = fs::metadata(&a).unwrap().modified().unwrap();
    if carried {
        copy_extras(&a, &b, None).unwrap();
        assert_eq!(extras(&a).unwrap(), extras(&b).unwrap());
        // Named with a separator after it, the same folder.
        assert_eq!(extras(&a.join("")).unwrap(), extras(&a).unwrap());
    }
    copy_times(&a, &b, None).unwrap();
    let copy = fs::metadata(&b).unwrap();
    assert_eq!(copy.modified().unwrap(), when);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(copy.permissions().mode() & 0o777, 0o750);
    }
    // Nothing of what the original holds.
    assert_eq!(list(&b).unwrap(), Vec::<OsString>::new());
}

#[cfg(target_vendor = "apple")]
#[test]
fn a_resource_fork_and_finder_info_are_carried_and_compared() {
    let d = dir();
    // A custom folder icon: no data fork at all.
    let (icon, copy) = (d.path().join("Icon\r"), d.path().join(".Icon.part"));
    fs::write(&icon, b"").unwrap();
    let fork: Vec<u8> = (0..300_000u32).map(|i| (i * 7 % 251) as u8).collect();
    fs::write(icon.join("..namedfork/rsrc"), &fork).unwrap();
    let mut info = [0u8; 32];
    info[8] = 0x04; // kHasCustomIcon
    set_extra(&icon, "com.apple.FinderInfo", &info).unwrap();
    // Through the handle it was made by, as a move gives them.
    let into = create_file(&copy).unwrap();
    copy_extras(&icon, &copy, Some(&into)).unwrap();
    assert_eq!(fs::read(copy.join("..namedfork/rsrc")).unwrap(), fork);
    assert_eq!(extras(&icon).unwrap(), extras(&copy).unwrap());
    assert_eq!(fs::read(&copy).unwrap(), b"");
    // A fork that differs by a byte is told.
    let mut other = fork;
    other[123_456] ^= 1;
    fs::write(copy.join("..namedfork/rsrc"), &other).unwrap();
    assert_ne!(extras(&icon).unwrap(), extras(&copy).unwrap());
}

#[test]
fn what_the_system_keeps_for_itself_is_neither_carried_nor_compared() {
    #[cfg(target_vendor = "apple")]
    {
        use super::unix::carried;
        for kept in [
            "com.apple.metadata:_kMDItemUserTags",
            "com.apple.FinderInfo",
            "com.apple.ResourceFork",
            "com.apple.quarantine",
            "com.apple.lastuseddate#PS",
            "user.tag",
            // A lower-case `n` too, which Apple's `xattr_flags_from_name`
            // takes for no flags at all (measured on macOS 15).
            "user#Nikon",
        ] {
            assert!(carried(kept.as_ref()), "{kept}");
        }
        // Set by the system for whichever process made the file, and
        // refused or ignored when written.
        for system in [
            "com.apple.provenance",
            "com.apple.macl",
            "com.apple.rootless",
            "com.apple.security.private",
            "com.apple.system.Security",
            "com.example.never#N",
            "com.example.never#PCN",
            "com.example.never#Notes",
        ] {
            assert!(!carried(system.as_ref()), "{system}");
        }
    }
    #[cfg(any(target_os = "linux", target_os = "android"))]
    {
        use super::unix::carried;
        assert!(carried("user.xdg.tags".as_ref()));
        for system in [
            "security.selinux",
            "system.posix_acl_access",
            "trusted.overlay.origin",
        ] {
            assert!(!carried(system.as_ref()), "{system}");
        }
    }
}
