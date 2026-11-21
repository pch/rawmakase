use super::*;
use crate::camera_data::Metadata;
use crate::camera_profiles::{LutSpace, import_into_dir, listed_from};

fn photo() -> Metadata {
    Metadata {
        make: "Test".into(),
        model: "Camera".into(),
        cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
        ..Default::default()
    }
}
fn cube(dir: &Path, name: &str, shift: f32) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let text = super::super::cube_look::tests::cube_text(5, |[r, g, b]| {
        [(r + shift).min(1.), g, (b - shift).max(0.)]
    });
    std::fs::write(&path, text).unwrap();
    path
}
/// A library with two LUT looks from "Film" (Portra, Ektar), one from "Moody"
/// (Dusk), and a camera profile.
fn library() -> (tempfile::TempDir, Library) {
    let temp = tempfile::tempdir().unwrap();
    let film = temp.path().join("Film");
    let moody = temp.path().join("Moody");
    let picks = [
        cube(&film, "Portra.cube", 0.1),
        cube(&film, "Ektar.cube", 0.2),
        cube(&moody, "Dusk.cube", 0.3),
    ];
    let dir = temp.path().join("library");
    import_into_dir(&picks, &dir, LutSpace::Srgb).unwrap();
    std::fs::write(
        dir.join("Base.dcp"),
        super::super::library::tests::dcp("Test Camera", "Base A"),
    )
    .unwrap();
    (temp, Library::at(dir))
}
fn entry<'a>(entries: &'a [Entry], name: &str) -> &'a Entry {
    entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("no {name} in {entries:?}"))
}
/// Where the Profile menu lists the look `name` for the test photo.
fn menu_group(library: &Library, name: &str) -> Option<Option<String>> {
    let listed = listed_from(&[library.dir().to_path_buf()], &photo());
    listed
        .profiles
        .iter()
        .find(|p| p.name == name)
        .map(|p| listed.groups.entry(p).group)
}

/// The listing holds every file whatever its camera: DCPs with their camera, looks
/// with their group and UUID, and files that don't read, so they can be removed.
/// Hidden files and a write's staging files are left out.
#[test]
fn every_library_file_is_listed() {
    let (_temp, library) = library();
    std::fs::write(library.dir().join("notes.txt"), "pack readme").unwrap();
    std::fs::write(library.dir().join(".DS_Store"), "").unwrap();
    std::fs::write(library.dir().join("rawmakase-x.partial"), "").unwrap();
    let entries = library.entries();
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["Base A", "Dusk", "Ektar", "Portra", "notes.txt"]);
    let base = entry(&entries, "Base A");
    assert_eq!(
        (base.kind, base.camera.as_str()),
        (Kind::Camera, "Test Camera")
    );
    let portra = entry(&entries, "Portra");
    assert_eq!((portra.kind, portra.group.as_str()), (Kind::Look, "Film"));
    assert!(portra.editable() && portra.uuid.len() == 32);
    assert_eq!(portra.key(), format!("look:{}", portra.uuid));
    let notes = entry(&entries, "notes.txt");
    assert!(notes.kind == Kind::Unreadable && notes.error.is_some());
    assert!(!base.editable() && !notes.editable());
}

/// Renaming a look rewrites its name alone: its UUID, group, table and file stay,
/// the Profile menu lists it by the new name, and its cube imported again is
/// recognised as already imported, under the new name.
#[test]
fn a_renamed_look_keeps_everything_but_its_name() {
    let (temp, library) = library();
    let before = library.entries();
    let portra = entry(&before, "Portra").clone();
    let text = std::fs::read_to_string(&portra.file).unwrap();
    library
        .rename_look(&portra.file, "  Kodak Portra 400 & co ")
        .unwrap();
    let renamed = std::fs::read_to_string(&portra.file).unwrap();
    assert_eq!(
        renamed.replace("Kodak Portra 400 &amp; co", "Portra"),
        text,
        "only the name changes"
    );
    let after = library.entries();
    let look = entry(&after, "Kodak Portra 400 & co");
    assert_eq!(
        (&look.file, &look.uuid, look.group.as_str()),
        (&portra.file, &portra.uuid, "Film")
    );
    assert_eq!(
        menu_group(&library, "Kodak Portra 400 & co"),
        Some(Some("Film".into()))
    );
    assert_eq!(menu_group(&library, "Portra"), None);
    let again = import_into_dir(
        &[temp.path().join("Film/Portra.cube")],
        library.dir(),
        LutSpace::Srgb,
    )
    .unwrap();
    let report = again[0].cube.as_ref().unwrap();
    assert!(report.already, "{report}");
    assert_eq!(report.name, "Kodak Portra 400 & co");
    assert_eq!(library.entries().len(), after.len());
    // The same name again changes nothing.
    library
        .rename_look(&portra.file, "Kodak Portra 400 & co")
        .unwrap();
}

/// A name another profile has, one RAWmakase keeps, an empty one or one with
/// control characters is refused, as is renaming a camera profile, one of Adobe's
/// looks or a file outside the library; the file stays as it was.
#[test]
fn a_rename_that_would_confuse_profiles_is_refused() {
    let (temp, library) = library();
    let entries = library.entries();
    let portra = entry(&entries, "Portra").file.clone();
    let text = std::fs::read_to_string(&portra).unwrap();
    for name in [
        "Ektar",
        "Base A",
        "RAWmakase Color",
        "Adobe Standard",
        " ",
        "a\u{7}b",
    ] {
        assert!(library.rename_look(&portra, name).is_err(), "{name:?}");
    }
    assert_eq!(std::fs::read_to_string(&portra).unwrap(), text);
    let base = entry(&entries, "Base A").file.clone();
    assert!(library.rename_look(&base, "Mine").is_err());
    let adobe = library.dir().join("Adobe Look.xmp");
    std::fs::write(
        &adobe,
        text.replace(
            r#"crs:Copyright="""#,
            r#"crs:Copyright="© 2018 Adobe Systems, Inc.""#,
        )
        .replace(">Portra<", ">Adobe Vivid<"),
    )
    .unwrap();
    assert!(entry(&library.entries(), "Adobe Vivid").adobe);
    assert!(library.rename_look(&adobe, "Vivid").is_err());
    assert!(library.set_group(&[adobe], "Mine").is_err());
    let outside = temp.path().join("outside.xmp");
    std::fs::write(&outside, &text).unwrap();
    assert!(library.rename_look(&outside, "Elsewhere").is_err());
    assert!(
        library
            .rename_look(&library.dir().join("../outside.xmp"), "Elsewhere")
            .is_err()
    );
}

/// Moving looks to a group rewrites their group, adds one to a look without, and an
/// empty group takes it out, to the menu's top level. A batch with a file that can't
/// be moved writes none of them.
#[test]
fn looks_move_between_groups() {
    let (_temp, library) = library();
    let entries = library.entries();
    let portra = entry(&entries, "Portra").file.clone();
    let dusk = entry(&entries, "Dusk").file.clone();
    assert_eq!(
        library
            .set_group(&[portra.clone(), dusk.clone()], "Favourite films")
            .unwrap(),
        2
    );
    assert_eq!(
        menu_group(&library, "Portra"),
        Some(Some("Favourite films".into()))
    );
    assert_eq!(
        menu_group(&library, "Dusk"),
        Some(Some("Favourite films".into()))
    );
    assert_eq!(menu_group(&library, "Ektar"), Some(Some("Film".into())));
    // Already there: nothing to write.
    assert_eq!(
        library
            .set_group(std::slice::from_ref(&portra), "Favourite films")
            .unwrap(),
        0
    );
    // Out of every group, then into one again: the label is removed and added.
    assert_eq!(
        library.set_group(std::slice::from_ref(&dusk), "").unwrap(),
        1
    );
    assert!(!std::fs::read_to_string(&dusk).unwrap().contains("Group>"));
    assert_eq!(menu_group(&library, "Dusk"), Some(None));
    assert_eq!(
        library
            .set_group(std::slice::from_ref(&dusk), "Moody")
            .unwrap(),
        1
    );
    assert_eq!(menu_group(&library, "Dusk"), Some(Some("Moody".into())));
    LookFile::parse(&std::fs::read_to_string(&dusk).unwrap()).unwrap();
    // A camera profile in the batch: refused before anything is written.
    let base = entry(&entries, "Base A").file.clone();
    assert!(library.set_group(&[dusk, base], "Elsewhere").is_err());
    assert_eq!(menu_group(&library, "Dusk"), Some(Some("Moody".into())));
}

/// Removing goes through the Trash given, file by file, only for files in the
/// library; the rest of a batch goes when one fails.
#[test]
fn files_are_removed_through_the_trash() {
    let (temp, library) = library();
    let entries = library.entries();
    let bin = temp.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let trash = |file: &Path| -> Result<()> {
        std::fs::rename(file, bin.join(file.file_name().unwrap()))?;
        Ok(())
    };
    let outside = temp.path().join("Film/Portra.cube");
    let files = [
        entry(&entries, "Portra").file.clone(),
        outside.clone(),
        entry(&entries, "Dusk").file.clone(),
    ];
    let removal = library.remove(&files, trash);
    assert_eq!(removal.removed, 2);
    assert_eq!(removal.failed.len(), 1);
    assert_eq!(removal.failed[0].0, outside);
    assert!(outside.exists());
    let left: Vec<String> = library.entries().into_iter().map(|e| e.name).collect();
    assert_eq!(left, ["Base A", "Ektar"]);
    assert!(bin.join("Portra.xmp").exists());
}

/// A camera look over a DCP is listed as depending on it while no other DCP of
/// that name for its camera is left; creative looks never are.
#[test]
fn removing_a_base_profile_names_the_looks_it_takes() {
    let (_temp, library) = library();
    let entries = library.entries();
    let text = std::fs::read_to_string(&entry(&entries, "Ektar").file).unwrap();
    let camera_look = library.dir().join("Camera Look.xmp");
    std::fs::write(
        &camera_look,
        text.replace(
            r#"crs:PresetType="Look""#,
            r#"crs:PresetType="Look" crs:CameraProfile="Base A""#,
        )
        .replace(">Ektar<", ">Base A Vivid<"),
    )
    .unwrap();
    let entries = library.entries();
    assert_eq!(
        entry(&entries, "Base A Vivid").base.as_deref(),
        Some("Base A")
    );
    let base = entry(&entries, "Base A").file.clone();
    assert_eq!(
        dependents(&entries, std::slice::from_ref(&base)),
        ["Base A Vivid"]
    );
    // Removed together, nothing is left behind.
    assert!(dependents(&entries, &[base.clone(), camera_look]).is_empty());
    // Another DCP of the name keeps it.
    std::fs::write(
        library.dir().join("Base copy.dcp"),
        super::super::library::tests::dcp("Test Camera", "Base A"),
    )
    .unwrap();
    assert!(dependents(&library.entries(), &[base]).is_empty());
}

/// A write that fails partway through a move says how many looks were moved
/// before it, so the caller knows the library changed.
#[cfg(unix)]
#[test]
fn a_move_that_fails_partway_says_how_far_it_got() {
    use std::os::unix::fs::PermissionsExt;
    let (_temp, library) = library();
    let entries = library.entries();
    let portra = entry(&entries, "Portra").file.clone();
    let locked = library.dir().join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    let dusk = locked.join("Dusk.xmp");
    std::fs::rename(&entry(&entries, "Dusk").file, &dusk).unwrap();
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root writes through a read-only folder (CI's Arch container runs as
    // root), so the failure this test needs can't be made there.
    if std::fs::write(locked.join("probe"), b"").is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        eprintln!("skipped: a read-only folder doesn't stop this user's writes");
        return;
    }
    let result = library.set_group(&[portra, dusk], "Elsewhere");
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    let error = result.unwrap_err();
    let partial = error.downcast_ref::<Partial>().expect("a partial move");
    assert_eq!(partial.done, 1);
    assert_eq!(
        menu_group(&library, "Portra"),
        Some(Some("Elsewhere".into()))
    );
    assert_eq!(menu_group(&library, "Dusk"), Some(Some("Moody".into())));
}
