//! The freedesktop Trash on folders of a temporary directory: a fake
//! `$XDG_DATA_HOME` and a fake mount top, so nothing reaches a real Trash.
use super::{
    Fs, Home, Places, candidate, deletion_date, encoded, mount_top, supports, trash, trash_with,
};
use crate::catalog::file_ops::{TrashError, TrashSupport};
use rawmakase_fs::FsError;
use std::ffi::OsStr;
use std::fs;
use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};

const DATE: &str = "2026-10-20T12:34:56";

/// This user's id, as the owner of a folder it makes (rustix, which asks
/// the system, is only a dependency where the freedesktop Trash is used).
fn uid() -> u32 {
    let made = tempfile::tempdir().unwrap();
    fs::metadata(made.path()).unwrap().uid()
}

/// A folder of photos under `root`, and one photo in it with `bytes`.
fn photo(root: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let dir = root.join("photos");
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    fs::write(&path, bytes).unwrap();
    path
}

fn device(path: &Path) -> u64 {
    fs::metadata(path).unwrap().dev()
}

/// The home Trash under `root/data`, on the photos' device.
fn at_home(root: &Path) -> Places<'static> {
    Places {
        home: Home::under(&root.join("data")),
        uid: uid(),
        top: &|_, _| None,
    }
}

/// No home Trash on the photos' device, and `top` as their mount's top.
fn on_mount<'a>(root: &Path, top: &'a dyn Fn(&Path, u64) -> Option<PathBuf>) -> Places<'a> {
    Places {
        home: Some(Home {
            trash: root.join("data/Trash"),
            device: device(root) ^ 1,
        }),
        uid: uid(),
        top,
    }
}

fn info(trash: &Path, name: &str) -> String {
    fs::read_to_string(trash.join("info").join(format!("{name}.trashinfo"))).unwrap()
}

/// The home Trash under `root/data`, where it really is: the receipts
/// name it so.
fn home_trash(root: &Path) -> PathBuf {
    canonical(root).join("data/Trash")
}

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

#[test]
fn a_file_on_the_home_device_goes_to_the_home_trash() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a b.jpg", b"pixels");
    let receipt = trash(&path, &at_home(root.path()), DATE).unwrap();

    let home = home_trash(root.path());
    assert_eq!(receipt.location, home.join("files/a b.jpg"));
    assert!(!path.exists());
    assert_eq!(fs::read(&receipt.location).unwrap(), b"pixels");
    let original = canonical(&root.path().join("photos")).join("a b.jpg");
    assert_eq!(
        info(&home, "a b.jpg"),
        format!(
            "[Trash Info]\nPath={}\nDeletionDate={DATE}\n",
            encoded(&original)
        )
    );
    for dir in ["", "files", "info"] {
        let mode = fs::metadata(home.join(dir)).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o700, "{dir}");
    }
}

#[test]
fn paths_are_percent_encoded_but_for_slashes() {
    use std::os::unix::ffi::OsStrExt as _;
    let path = Path::new(OsStr::from_bytes(b"/ph otos/caf\xc3\xa9/100%/a\xff,b.jpg"));
    assert_eq!(encoded(path), "/ph%20otos/caf%C3%A9/100%25/a%FF%2Cb.jpg");
    assert_eq!(encoded(Path::new("A-z_0.9~")), "A-z_0.9~");
}

#[test]
fn deletion_dates_are_iso_8601_without_a_zone() {
    assert_eq!(deletion_date(1_469_686_444), "2016-07-28T06:14:04");
}

#[test]
fn another_name_is_numbered_before_the_extension() {
    assert_eq!(candidate(OsStr::new("a.jpg"), 1), "a.jpg");
    assert_eq!(candidate(OsStr::new("a.jpg"), 2), "a.2.jpg");
    assert_eq!(candidate(OsStr::new("a.tar.gz"), 3), "a.tar.3.gz");
    assert_eq!(candidate(OsStr::new("noext"), 3), "noext.3");
    assert_eq!(candidate(OsStr::new(".hidden"), 2), ".hidden.2");
}

#[test]
fn a_name_the_trash_has_gets_another() {
    let root = tempfile::tempdir().unwrap();
    let places = at_home(root.path());
    let first = photo(root.path(), "a.jpg", b"one");
    let first = trash(&first, &places, DATE).unwrap();
    let second = photo(root.path(), "a.jpg", b"two");
    let second = trash(&second, &places, DATE).unwrap();

    let home = home_trash(root.path());
    assert_eq!(first.location, home.join("files/a.jpg"));
    assert_eq!(second.location, home.join("files/a.2.jpg"));
    assert_eq!(fs::read(&first.location).unwrap(), b"one");
    assert_eq!(fs::read(&second.location).unwrap(), b"two");
    assert!(info(&home, "a.2.jpg").starts_with("[Trash Info]\n"));
}

/// A file in `files/` with no `.trashinfo` (left by another program) keeps
/// its name, and the `.trashinfo` written for it is taken back.
#[test]
fn a_file_without_trashinfo_keeps_its_name() {
    let root = tempfile::tempdir().unwrap();
    let places = at_home(root.path());
    let home = home_trash(root.path());
    // The Trash made by a first file.
    let made = trash(&photo(root.path(), "x.jpg", b"x"), &places, DATE).unwrap();
    assert!(made.location.exists());
    fs::write(home.join("files/a.jpg"), b"orphan").unwrap();

    let path = photo(root.path(), "a.jpg", b"mine");
    let receipt = trash(&path, &places, DATE).unwrap();
    assert_eq!(receipt.location, home.join("files/a.2.jpg"));
    assert_eq!(fs::read(home.join("files/a.jpg")).unwrap(), b"orphan");
    assert!(!home.join("info/a.jpg.trashinfo").exists());
    assert!(home.join("info/a.2.jpg.trashinfo").exists());
}

#[test]
fn a_failed_rename_takes_back_its_trashinfo() {
    for (error, expected) in [
        (
            FsError::PermissionDenied,
            TrashError::Failed("permission denied".into()),
        ),
        (FsError::CrossDevice, TrashError::Unsupported),
        (FsError::Unsupported, TrashError::Unsupported),
    ] {
        let root = tempfile::tempdir().unwrap();
        let path = photo(root.path(), "a.jpg", b"pixels");
        let error = std::cell::RefCell::new(Some(error));
        let rename = |_: &Path, _: &Path| Err(error.borrow_mut().take().unwrap());
        let fs_ops = Fs {
            rename: &rename,
            ..Fs::real()
        };
        let got = trash_with(&path, &at_home(root.path()), DATE, &fs_ops);
        assert_eq!(got, Err(expected));
        assert_eq!(fs::read(&path).unwrap(), b"pixels");
        let info = root.path().join("data/Trash/info");
        assert_eq!(fs::read_dir(info).unwrap().count(), 0);
    }
}

#[test]
fn without_a_trash_on_its_device_a_file_stays() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    let none = |_: &Path, _: u64| None;
    assert_eq!(
        trash(&path, &on_mount(root.path(), &none), DATE),
        Err(TrashError::Unsupported)
    );
    // No home either.
    let places = Places {
        home: None,
        uid: uid(),
        top: &none,
    };
    assert_eq!(trash(&path, &places, DATE), Err(TrashError::Unsupported));
    assert_eq!(fs::read(&path).unwrap(), b"pixels");
    assert!(!root.path().join("data").exists());
}

#[test]
fn elsewhere_it_makes_its_own_trash_at_the_top() {
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path());
    let path = photo(root.path(), "a.jpg", b"pixels");
    let at = |_: &Path, _: u64| Some(top.clone());
    let receipt = trash(&path, &on_mount(root.path(), &at), DATE).unwrap();

    let own = top.join(format!(".Trash-{}", uid()));
    assert_eq!(receipt.location, own.join("files/a.jpg"));
    assert_eq!(fs::read(&receipt.location).unwrap(), b"pixels");
    assert_eq!(
        fs::metadata(&own).unwrap().permissions().mode() & 0o777,
        0o700
    );
    // Relative to the top.
    assert_eq!(
        info(&own, "a.jpg"),
        format!("[Trash Info]\nPath=photos/a.jpg\nDeletionDate={DATE}\n")
    );
    assert!(!root.path().join("data").exists());
}

#[test]
fn a_sticky_shared_trash_is_used() {
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path());
    let shared = top.join(".Trash");
    fs::create_dir(&shared).unwrap();
    fs::set_permissions(&shared, fs::Permissions::from_mode(0o1777)).unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    let at = |_: &Path, _: u64| Some(top.clone());
    let receipt = trash(&path, &on_mount(root.path(), &at), DATE).unwrap();

    let mine = shared.join(uid().to_string());
    assert_eq!(receipt.location, mine.join("files/a.jpg"));
    assert!(!top.join(format!(".Trash-{}", uid())).exists());
}

#[test]
fn a_shared_trash_without_the_sticky_bit_or_as_a_link_is_not() {
    for link in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let top = canonical(root.path());
        let shared = top.join(".Trash");
        if link {
            let real = top.join("elsewhere");
            fs::create_dir(&real).unwrap();
            fs::set_permissions(&real, fs::Permissions::from_mode(0o1777)).unwrap();
            std::os::unix::fs::symlink(&real, &shared).unwrap();
        } else {
            fs::create_dir(&shared).unwrap();
            fs::set_permissions(&shared, fs::Permissions::from_mode(0o777)).unwrap();
        }
        let path = photo(root.path(), "a.jpg", b"pixels");
        let at = |_: &Path, _: u64| Some(top.clone());
        let receipt = trash(&path, &on_mount(root.path(), &at), DATE).unwrap();

        let own = top.join(format!(".Trash-{}", uid()));
        assert_eq!(receipt.location, own.join("files/a.jpg"), "link {link}");
        assert!(!shared.join(uid().to_string()).exists(), "link {link}");
    }
}

/// `.Trash-$uid` as a link, a file, or a folder of someone else (here: a
/// uid that isn't the one that made it).
#[test]
fn an_unusable_own_trash_refuses() {
    for case in ["link", "file", "owner", "files link"] {
        let root = tempfile::tempdir().unwrap();
        let top = canonical(root.path());
        let mut uid = uid();
        let own = |uid: u32| top.join(format!(".Trash-{uid}"));
        match case {
            "link" => {
                fs::create_dir(top.join("elsewhere")).unwrap();
                std::os::unix::fs::symlink(top.join("elsewhere"), own(uid)).unwrap();
            }
            "file" => fs::write(own(uid), b"").unwrap(),
            "owner" => uid += 1,
            _ => {
                fs::create_dir(own(uid)).unwrap();
                fs::create_dir(top.join("elsewhere")).unwrap();
                std::os::unix::fs::symlink(top.join("elsewhere"), own(uid).join("files")).unwrap();
            }
        }
        let path = photo(root.path(), "a.jpg", b"pixels");
        let at = |_: &Path, _: u64| Some(top.clone());
        let mut places = on_mount(root.path(), &at);
        places.uid = uid;
        assert_eq!(
            trash(&path, &places, DATE),
            Err(TrashError::Unsupported),
            "{case}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"pixels", "{case}");
        assert_eq!(
            fs::read_dir(top.join("elsewhere")).map_or(0, |d| d.count()),
            0
        );
    }
}

#[test]
fn a_top_it_cannot_write_refuses() {
    if uid() == 0 {
        return; // root writes anywhere
    }
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path()).join("volume");
    fs::create_dir(&top).unwrap();
    let path = photo(&top, "a.jpg", b"pixels");
    fs::set_permissions(&top, fs::Permissions::from_mode(0o555)).unwrap();
    let at = |_: &Path, _: u64| Some(top.clone());
    let got = trash(&path, &on_mount(root.path(), &at), DATE);
    fs::set_permissions(&top, fs::Permissions::from_mode(0o755)).unwrap();
    assert_eq!(got, Err(TrashError::Unsupported));
    assert_eq!(fs::read(&path).unwrap(), b"pixels");
}

#[test]
fn a_missing_file_fails_and_makes_nothing() {
    let root = tempfile::tempdir().unwrap();
    let got = trash(&root.path().join("gone.jpg"), &at_home(root.path()), DATE);
    assert!(matches!(got, Err(TrashError::Failed(_))), "{got:?}");
    assert!(!root.path().join("data").exists());
}

#[test]
fn the_mount_top_is_the_highest_folder_on_the_device() {
    let root = tempfile::tempdir().unwrap();
    let dir = canonical(root.path());
    let top = mount_top(&dir, device(&dir)).unwrap();
    assert!(dir.starts_with(&top));
    assert_eq!(device(&top), device(&dir));
    if let Some(parent) = top.parent() {
        assert_ne!(device(parent), device(&dir));
    }
    // Not a folder on that device at all.
    assert_eq!(mount_top(&dir, device(&dir) ^ 1), None);
}

/// Mode `mode` on `path`, whatever the umask made it.
fn chmod(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
}

#[test]
fn a_trash_others_can_write_to_refuses() {
    for case in ["own", "files", "info", "home files"] {
        let root = tempfile::tempdir().unwrap();
        let top = canonical(root.path());
        let own = top.join(format!(".Trash-{}", uid()));
        let path = photo(root.path(), "a.jpg", b"pixels");
        let at = |_: &Path, _: u64| Some(top.clone());
        let places = match case {
            "home files" => {
                let files = root.path().join("data/Trash/files");
                fs::create_dir_all(&files).unwrap();
                chmod(&files, 0o777);
                at_home(root.path())
            }
            "own" => {
                fs::create_dir(&own).unwrap();
                chmod(&own, 0o777);
                on_mount(root.path(), &at)
            }
            sub => {
                fs::create_dir_all(own.join(sub)).unwrap();
                chmod(&own.join(sub), 0o777);
                on_mount(root.path(), &at)
            }
        };
        assert_eq!(
            trash(&path, &places, DATE),
            Err(TrashError::Unsupported),
            "{case}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"pixels", "{case}");
    }
}

#[test]
fn the_trash_is_on_disk_before_its_receipt() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    let log = std::cell::RefCell::new(Vec::<(&str, PathBuf)>::new());
    let rename = |from: &Path, to: &Path| {
        log.borrow_mut().push(("rename", from.to_path_buf()));
        rawmakase_fs::rename_exclusive(from, to)
    };
    let sync = |dir: &Path| {
        log.borrow_mut().push(("sync", canonical(dir)));
        Ok(())
    };
    let fs_ops = Fs {
        rename: &rename,
        sync: &sync,
    };
    let receipt = trash_with(&path, &at_home(root.path()), DATE, &fs_ops).unwrap();
    let log = log.into_inner();
    let at = |what: &str, dir: &Path| {
        let dir = canonical(dir);
        log.iter()
            .position(|(w, p)| *w == what && *p == dir)
            .unwrap_or_else(|| panic!("no {what} of {}: {log:?}", dir.display()))
    };
    let renamed = log.iter().position(|(w, _)| *w == "rename").unwrap();
    let home = home_trash(root.path());
    // The folders made, in their parents; the .trashinfo, in its folder:
    // before the file moves. Then the file, in the Trash.
    assert!(at("sync", &root.path().join("data")) < renamed);
    assert!(at("sync", &home) < renamed);
    assert!(at("sync", &home.join("info")) < renamed);
    assert!(at("sync", &home.join("files")) > renamed);
    assert!(receipt.location.is_file());
}

#[test]
fn a_trash_that_cant_be_made_durable_gives_the_file_back() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    let sync = |dir: &Path| {
        if dir.ends_with("files") {
            Err(FsError::Io(std::io::Error::other("disk full")))
        } else {
            Ok(())
        }
    };
    let fs_ops = Fs {
        sync: &sync,
        ..Fs::real()
    };
    let got = trash_with(&path, &at_home(root.path()), DATE, &fs_ops);
    assert!(matches!(got, Err(TrashError::Failed(_))), "{got:?}");
    assert_eq!(fs::read(&path).unwrap(), b"pixels");
    let home = home_trash(root.path());
    assert_eq!(fs::read_dir(home.join("files")).unwrap().count(), 0);
    assert_eq!(fs::read_dir(home.join("info")).unwrap().count(), 0);
}

#[test]
fn a_trash_made_by_an_attempt_that_failed_is_made_durable_by_the_next() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    let data = canonical(&root.path().join("photos"))
        .parent()
        .unwrap()
        .join("data");
    // The first time the data folder is synced, it fails: the Trash in it
    // is made, but its entry may not be on disk.
    let failed = std::cell::Cell::new(false);
    let log = std::cell::RefCell::new(Vec::<PathBuf>::new());
    let sync = |dir: &Path| {
        let dir = canonical(dir);
        if dir == data && !failed.replace(true) {
            return Err(FsError::Io(std::io::Error::other("disk busy")));
        }
        log.borrow_mut().push(dir);
        Ok(())
    };
    let fs_ops = Fs {
        sync: &sync,
        ..Fs::real()
    };
    let first = trash_with(&path, &at_home(root.path()), DATE, &fs_ops);
    assert!(matches!(first, Err(TrashError::Failed(_))), "{first:?}");
    assert_eq!(fs::read(&path).unwrap(), b"pixels");
    assert!(root.path().join("data/Trash").is_dir(), "made, not synced");
    let receipt = trash_with(&path, &at_home(root.path()), DATE, &fs_ops).unwrap();
    assert!(receipt.location.is_file());
    assert!(log.borrow().contains(&data), "{:?}", log.borrow());
}

#[test]
fn a_home_trash_through_a_link_is_made_durable_where_it_really_is() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    // The data folder is a link to a folder elsewhere on the same device.
    let real = root.path().join("elsewhere/real-data");
    fs::create_dir_all(&real).unwrap();
    std::os::unix::fs::symlink(&real, root.path().join("data")).unwrap();
    let log = std::cell::RefCell::new(Vec::<PathBuf>::new());
    let sync = |dir: &Path| {
        log.borrow_mut().push(canonical(dir));
        Ok(())
    };
    let fs_ops = Fs {
        sync: &sync,
        ..Fs::real()
    };
    let receipt = trash_with(&path, &at_home(root.path()), DATE, &fs_ops).unwrap();
    assert!(receipt.location.is_file());
    // The real Trash's own parent, and that one's.
    assert!(
        log.borrow().contains(&canonical(&real)),
        "{:?}",
        log.borrow()
    );
    assert!(
        log.borrow()
            .contains(&canonical(&root.path().join("elsewhere"))),
        "{:?}",
        log.borrow()
    );
}

#[test]
fn a_shared_trash_is_made_durable_in_the_top_of_its_mount() {
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path());
    let shared = top.join(".Trash");
    fs::create_dir(&shared).unwrap();
    chmod(&shared, 0o1777);
    let path = photo(root.path(), "a.jpg", b"pixels");
    let at = |_: &Path, _: u64| Some(top.clone());
    let log = std::cell::RefCell::new(Vec::<PathBuf>::new());
    let sync = |dir: &Path| {
        log.borrow_mut().push(canonical(dir));
        Ok(())
    };
    let fs_ops = Fs {
        sync: &sync,
        ..Fs::real()
    };
    let receipt = trash_with(&path, &on_mount(root.path(), &at), DATE, &fs_ops).unwrap();
    assert!(receipt.location.starts_with(&shared));
    assert!(log.borrow().contains(&top), "{:?}", log.borrow());
    assert!(log.borrow().contains(&shared), "{:?}", log.borrow());
}

#[test]
fn a_home_trash_through_chained_links_hands_back_its_real_place() {
    let root = tempfile::tempdir().unwrap();
    let path = photo(root.path(), "a.jpg", b"pixels");
    // data → intermediate/link → elsewhere/real-data
    let real = root.path().join("elsewhere/real-data");
    fs::create_dir_all(&real).unwrap();
    fs::create_dir(root.path().join("intermediate")).unwrap();
    std::os::unix::fs::symlink(&real, root.path().join("intermediate/link")).unwrap();
    std::os::unix::fs::symlink(
        root.path().join("intermediate/link"),
        root.path().join("data"),
    )
    .unwrap();
    let receipt = trash(&path, &at_home(root.path()), DATE).unwrap();
    // No link is needed to find it: losing one can't lose the file.
    assert_eq!(receipt.location, canonical(&real).join("Trash/files/a.jpg"));
    assert_eq!(fs::read(&receipt.location).unwrap(), b"pixels");
}

/// Everything under `root`, to see that asking made nothing.
fn tree(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut todo = vec![root.to_path_buf()];
    while let Some(dir) = todo.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if fs::symlink_metadata(&path).unwrap().is_dir() {
                todo.push(path.clone());
            }
            found.push(path);
        }
    }
    found.sort();
    found
}

#[test]
fn a_home_trash_says_yes_made_or_not() {
    let root = tempfile::tempdir().unwrap();
    let places = at_home(root.path());
    let path = photo(root.path(), "a.jpg", b"pixels");
    let before = tree(root.path());
    assert_eq!(supports(path.parent().unwrap(), &places), TrashSupport::Yes);
    assert_eq!(supports(&path, &places), TrashSupport::Yes);
    assert_eq!(tree(root.path()), before, "asking makes nothing");

    let made = trash(&photo(root.path(), "b.jpg", b"b"), &places, DATE).unwrap();
    assert!(made.location.exists());
    assert_eq!(supports(&path, &places), TrashSupport::Yes);
}

#[test]
fn elsewhere_a_trash_still_to_make_is_unknown_and_a_made_one_yes() {
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path());
    let path = photo(root.path(), "a.jpg", b"pixels");
    let at = |_: &Path, _: u64| Some(top.clone());
    let places = on_mount(root.path(), &at);
    let before = tree(root.path());
    assert_eq!(supports(&path, &places), TrashSupport::Unknown);
    assert_eq!(tree(root.path()), before, "asking makes nothing");

    let made = trash(&photo(root.path(), "b.jpg", b"b"), &places, DATE).unwrap();
    assert!(made.location.exists());
    assert_eq!(supports(&path, &places), TrashSupport::Yes);
}

#[test]
fn a_sticky_shared_trash_says_yes_once_this_user_has_a_folder_in_it() {
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path());
    let shared = top.join(".Trash");
    fs::create_dir(&shared).unwrap();
    chmod(&shared, 0o1777);
    let path = photo(root.path(), "a.jpg", b"pixels");
    let at = |_: &Path, _: u64| Some(top.clone());
    let places = on_mount(root.path(), &at);
    assert_eq!(supports(&path, &places), TrashSupport::Unknown);
    let mine = shared.join(uid().to_string());
    fs::create_dir(&mine).unwrap();
    chmod(&mine, 0o700);
    assert_eq!(supports(&path, &places), TrashSupport::Yes);
}

/// A sure no where the Trash refuses: no mount top, an own Trash that is a
/// link, a file or a folder holding a link, and Trash folders others can
/// write to. Each answer is checked against the Trash itself.
#[test]
fn it_says_no_where_the_trash_refuses() {
    for case in [
        "no top",
        "link",
        "file",
        "files link",
        "own",
        "files",
        "info",
        "home",
        "home files",
    ] {
        let root = tempfile::tempdir().unwrap();
        let top = canonical(root.path());
        let own = top.join(format!(".Trash-{}", uid()));
        let path = photo(root.path(), "a.jpg", b"pixels");
        let at = |_: &Path, _: u64| Some(top.clone());
        let none = |_: &Path, _: u64| None;
        let places = match case {
            "no top" => on_mount(root.path(), &none),
            "link" => {
                fs::create_dir(top.join("elsewhere")).unwrap();
                std::os::unix::fs::symlink(top.join("elsewhere"), &own).unwrap();
                on_mount(root.path(), &at)
            }
            "file" => {
                fs::write(&own, b"").unwrap();
                on_mount(root.path(), &at)
            }
            "files link" => {
                fs::create_dir(&own).unwrap();
                chmod(&own, 0o700);
                fs::create_dir(top.join("elsewhere")).unwrap();
                std::os::unix::fs::symlink(top.join("elsewhere"), own.join("files")).unwrap();
                on_mount(root.path(), &at)
            }
            "own" => {
                fs::create_dir(&own).unwrap();
                chmod(&own, 0o777);
                on_mount(root.path(), &at)
            }
            "home" | "home files" => {
                let trash = root.path().join("data/Trash");
                let open = if case == "home" {
                    trash
                } else {
                    trash.join("files")
                };
                fs::create_dir_all(&open).unwrap();
                chmod(&open, 0o777);
                at_home(root.path())
            }
            sub => {
                fs::create_dir_all(own.join(sub)).unwrap();
                chmod(&own, 0o700);
                chmod(&own.join(sub), 0o777);
                on_mount(root.path(), &at)
            }
        };
        let before = tree(root.path());
        assert_eq!(supports(&path, &places), TrashSupport::No, "{case}");
        assert_eq!(tree(root.path()), before, "{case}: asking makes nothing");
        assert_eq!(
            trash(&path, &places, DATE),
            Err(TrashError::Unsupported),
            "{case}"
        );
    }
}

#[test]
fn a_top_it_cannot_write_says_no() {
    if uid() == 0 {
        return; // root writes anywhere
    }
    let root = tempfile::tempdir().unwrap();
    let top = canonical(root.path()).join("volume");
    fs::create_dir(&top).unwrap();
    let path = photo(&top, "a.jpg", b"pixels");
    chmod(&top, 0o555);
    let at = |_: &Path, _: u64| Some(top.clone());
    let got = supports(&path, &on_mount(root.path(), &at));
    chmod(&top, 0o755);
    assert_eq!(got, TrashSupport::No);
}

#[test]
fn a_missing_path_is_unknown() {
    let root = tempfile::tempdir().unwrap();
    let got = supports(&root.path().join("gone"), &at_home(root.path()));
    assert_eq!(got, TrashSupport::Unknown);
}
