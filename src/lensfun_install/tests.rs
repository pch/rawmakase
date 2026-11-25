use super::*;
use std::sync::Arc;

fn archive(name: &str) -> Source {
    Source::File(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/lensfun")
            .join(name),
    )
}
/// The folder's entries, sorted, without the lock file.
fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n != LOCK)
        .collect();
    names.sort();
    names
}
const OLD: i64 = 1_793_180_205;
const NEW: i64 = 1_800_000_000;

/// A database installs into its own folder, named in the manifest with its archive's
/// size and SHA-256, and reads back; installing it again changes nothing.
#[test]
fn an_archive_installs_into_its_own_version() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Progress::default();
    let Installed::New(m) = install(dir.path(), &archive("good.tar.bz2"), &progress)? else {
        panic!("not installed");
    };
    assert_eq!((m.version, m.lenses, m.cameras), (OLD, 1, 2));
    let packed =
        std::fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/lensfun/good.tar.bz2"))?;
    assert_eq!(m.bytes, packed.len() as u64);
    assert_eq!(m.sha256.len(), 64);
    assert_eq!(listing(dir.path()), ["1793180205", "installed.json"]);
    let loaded = installed::load(dir.path())?;
    assert_eq!(loaded.manifest, m);
    assert_eq!(loaded.db.timestamp, OLD);
    assert_eq!(
        install(dir.path(), &archive("good.tar.bz2"), &progress)?,
        Installed::Current(m.clone())
    );
    // An archive with a timestamp.txt that agrees is the same version.
    assert_eq!(
        install(dir.path(), &archive("stamped.tar.bz2"), &progress)?,
        Installed::Current(m)
    );
    Ok(())
}

/// An update is a new folder, named once it is whole; the earlier version stays for
/// a while, for readers that had just read the manifest, and goes at the next
/// install once old enough.
#[test]
fn an_update_publishes_a_new_version_beside_the_old() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Progress::default();
    install(dir.path(), &archive("good.tar.bz2"), &progress)?;
    let Installed::New(m) = install(dir.path(), &archive("newer.tar.bz2"), &progress)? else {
        panic!("not updated");
    };
    assert_eq!(m.version, NEW);
    assert_eq!(
        listing(dir.path()),
        ["1793180205", "1800000000", "installed.json"]
    );
    assert_eq!(installed::load(dir.path())?.db.timestamp, NEW);
    // Out of use long enough (the manifest naming another version that old), it goes.
    for name in ["1793180205", MANIFEST] {
        set_time(
            &dir.path().join(name),
            std::time::SystemTime::now() - KEEP_OLD * 2,
        )?;
    }
    install(dir.path(), &archive("newer.tar.bz2"), &progress)?;
    assert_eq!(listing(dir.path()), ["1800000000", "installed.json"]);
    Ok(())
}

/// Every archive that isn't a whole, plain Lensfun database is refused, and leaves
/// the installed version in use, with nothing of its own left behind.
#[test]
fn a_bad_archive_leaves_the_installed_version() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Progress::default();
    install(dir.path(), &archive("good.tar.bz2"), &progress)?;
    let manifest = std::fs::read(dir.path().join(MANIFEST))?;
    for (name, why) in [
        ("symlink.tar.bz2", "isn't a regular file"),
        ("traversal.tar.bz2", "not a database file"),
        ("absolute.tar.bz2", "not a database file"),
        ("folder.tar.bz2", "not a database file"),
        ("directory.tar.bz2", "isn't a regular file"),
        ("other-file.tar.bz2", "not a database file"),
        ("not-lensfun.tar.bz2", "broken.xml"),
        ("no-lenses.tar.bz2", "no lenses"),
        ("stamp-differs.tar.bz2", "timestamp.txt"),
        ("too-large.tar.bz2", "larger"),
        ("not-bzip2.tar.bz2", ""),
        ("many-entries.tar.bz2", "larger"),
        ("truncated.tar.bz2", ""),
    ] {
        let error = install(dir.path(), &archive(name), &progress).unwrap_err();
        assert!(format!("{error:#}").contains(why), "{name}: {error:#}");
        assert_eq!(
            std::fs::read(dir.path().join(MANIFEST))?,
            manifest,
            "{name}"
        );
        assert_eq!(
            listing(dir.path()),
            ["1793180205", "installed.json"],
            "{name}"
        );
        assert!(!dir.path().parent().unwrap().join("escaped.xml").exists());
    }
    Ok(())
}

/// A cancel before the manifest is written leaves the version in use.
#[test]
fn a_cancelled_install_publishes_nothing() -> Result<()> {
    let dir = tempfile::tempdir()?;
    install(dir.path(), &archive("good.tar.bz2"), &Progress::default())?;
    let cancelled = Progress::default();
    cancelled.cancel.store(true, Ordering::Relaxed);
    let error = install(dir.path(), &archive("newer.tar.bz2"), &cancelled).unwrap_err();
    assert!(is_cancelled(&error), "{error:#}");
    assert_eq!(current(dir.path()).unwrap().version, OLD);
    assert_eq!(listing(dir.path()), ["1793180205", "installed.json"]);
    Ok(())
}

/// One install at a time: another waits for the lock, and a cancel stops the wait.
#[test]
fn installs_wait_for_each_other() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Arc::new(Progress::default());
    let held = lock(dir.path(), &Progress::default())?;
    let waiting = {
        let (dir, progress) = (dir.path().to_path_buf(), Arc::clone(&progress));
        std::thread::spawn(move || install(&dir, &archive("good.tar.bz2"), &progress))
    };
    std::thread::sleep(Duration::from_millis(300));
    assert!(!waiting.is_finished());
    assert!(current(dir.path()).is_none());
    progress.cancel.store(true, Ordering::Relaxed);
    assert!(is_cancelled(&waiting.join().unwrap().unwrap_err()));
    // Once released, the next install goes ahead.
    drop(held);
    assert!(matches!(
        install(dir.path(), &archive("good.tar.bz2"), &Progress::default())?,
        Installed::New(_)
    ));
    Ok(())
}

/// Removal deletes the manifest first, then every version; a crashed install's
/// staging folder goes once it is old.
#[test]
fn removal_takes_every_version_and_old_staging() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Progress::default();
    assert!(!remove(dir.path(), &progress)?);
    install(dir.path(), &archive("good.tar.bz2"), &progress)?;
    install(dir.path(), &archive("newer.tar.bz2"), &progress)?;
    let staging = dir.path().join(".staging-1-1");
    std::fs::create_dir(&staging)?;
    set_time(&staging, std::time::SystemTime::now() - KEEP_STAGING * 2)?;
    let recent = dir.path().join(".staging-2-2");
    std::fs::create_dir(&recent)?;
    assert!(remove(dir.path(), &progress)?);
    assert_eq!(listing(dir.path()), [".staging-2-2"]);
    assert!(current(dir.path()).is_none());
    assert!(installed::load(dir.path()).is_err());
    Ok(())
}

/// Lensfun's versions.json: the timestamp and the formats offered.
#[test]
fn versions_name_the_timestamp_and_format() {
    assert_eq!(versions(br"[1793180205, [0, 1, 2], []]").unwrap(), OLD);
    assert!(versions(br"[1793180205, [0, 1], []]").is_err());
    assert!(versions(br"{}").is_err());
    assert!(versions(b"not json").is_err());
}

/// Lensfun's own archive, as downloaded, installs: dated as versions.json says, every
/// file read.
#[test]
#[ignore = "Read-only local copy of Lensfun's version_2.tar.bz2; set RAWMAKASE_LENSFUN_ARCHIVE"]
fn lensfun_s_own_archive_installs() -> Result<()> {
    let path = PathBuf::from(std::env::var_os("RAWMAKASE_LENSFUN_ARCHIVE").unwrap());
    let dir = tempfile::tempdir()?;
    let Installed::New(m) = install(dir.path(), &Source::File(path), &Progress::default())? else {
        panic!("not installed");
    };
    eprintln!("{m:?}");
    assert!(m.lenses > 1000 && m.cameras > 1000);
    Ok(())
}

/// A newer version installed than Lensfun offers now (a database installed from a
/// file) is current: an update never goes back.
#[test]
fn an_update_never_goes_back() {
    assert!(is_current(NEW, OLD) && is_current(OLD, OLD) && !is_current(OLD, NEW));
}

/// A removal cancelled before it starts removes nothing.
#[test]
fn a_cancelled_removal_keeps_the_database() -> Result<()> {
    let dir = tempfile::tempdir()?;
    install(dir.path(), &archive("good.tar.bz2"), &Progress::default())?;
    let cancelled = Progress::default();
    cancelled.cancel.store(true, Ordering::Relaxed);
    assert!(is_cancelled(&remove(dir.path(), &cancelled).unwrap_err()));
    assert_eq!(current(dir.path()).unwrap().version, OLD);
    Ok(())
}

/// A version stays for readers until it has been out of use a while, however old its
/// folder: a reader that read the manifest just before an update still finds it.
#[test]
fn a_version_just_replaced_stays_for_its_readers() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Progress::default();
    install(dir.path(), &archive("good.tar.bz2"), &progress)?;
    set_time(
        &dir.path().join("1793180205"),
        std::time::SystemTime::now() - KEEP_OLD * 2,
    )?;
    install(dir.path(), &archive("newer.tar.bz2"), &progress)?;
    install(dir.path(), &archive("newer.tar.bz2"), &progress)?;
    assert_eq!(
        listing(dir.path()),
        ["1793180205", "1800000000", "installed.json"]
    );
    Ok(())
}

/// Sets file or folder `path`'s modification time. Windows needs a handle opened to
/// write attributes, and a folder's opened with backup semantics.
fn set_time(path: &Path, when: std::time::SystemTime) -> Result<()> {
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_WRITE_ATTRIBUTES, FILE_FLAG_BACKUP_SEMANTICS.
        std::fs::OpenOptions::new()
            .access_mode(0x100)
            .custom_flags(0x0200_0000)
            .open(path)?
    };
    #[cfg(not(windows))]
    let file = std::fs::File::open(path)?;
    Ok(file.set_modified(when)?)
}

/// A removal cut short (the manifest gone, its version's folder only partly
/// deleted) leaves a folder that still reads; installing that version again
/// writes it whole rather than naming what was left of it.
#[test]
fn reinstalling_over_a_partly_removed_version_writes_it_whole() -> Result<()> {
    let dir = tempfile::tempdir()?;
    let progress = Progress::default();
    let Installed::New(whole) = install(dir.path(), &archive("good.tar.bz2"), &progress)? else {
        panic!("not installed");
    };
    std::fs::remove_file(dir.path().join(MANIFEST))?;
    std::fs::remove_file(dir.path().join(OLD.to_string()).join("slr-nikon.xml"))?;
    let Installed::New(again) = install(dir.path(), &archive("good.tar.bz2"), &progress)? else {
        panic!("not installed");
    };
    assert_eq!((again.lenses, again.cameras), (whole.lenses, whole.cameras));
    let loaded = installed::load(dir.path())?;
    assert_eq!(
        loaded.db.cameras.len(),
        whole.cameras,
        "a partial folder named"
    );
    Ok(())
}
