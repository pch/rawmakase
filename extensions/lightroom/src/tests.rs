use super::*;
use crate::fake::{Layout, install};

/// A file, or a preset folder, with its path relative to the root it is
/// under, named.
type Seen = (FileKind, String, FileOrigin, Option<String>, Option<String>);

/// `path` relative to the root of `roots` it is under, named.
fn relative(roots: &Roots, path: &Path) -> String {
    [
        ("shared", &roots.shared),
        ("user", &roots.user),
        ("pictures", &roots.pictures),
    ]
    .into_iter()
    .find_map(|(name, root)| {
        let relative = path.strip_prefix(root.as_ref()?).ok()?;
        Some(format!(
            "{name}/{}",
            relative.to_string_lossy().replace('\\', "/")
        ))
    })
    .expect("every path is under a root")
}

/// Each file of `listed`, and each preset folder.
fn seen(roots: &Roots, listed: &FilesListed) -> Vec<Seen> {
    let mut seen = Vec::new();
    for f in &listed.folders {
        let entry = |path: PathBuf, camera: Option<String>| {
            (
                f.kind,
                relative(roots, &path),
                f.origin,
                camera,
                f.group.clone(),
            )
        };
        if f.files.is_empty() {
            seen.push(entry(f.path.clone(), f.camera.clone()));
        }
        for file in &f.files {
            let camera = file.camera.clone().or_else(|| f.camera.clone());
            seen.push(entry(f.path.join(&file.name), camera));
        }
    }
    seen
}

#[test]
fn both_layouts_list_what_adobe_s_folders_hold() -> std::io::Result<()> {
    use {FileKind::*, FileOrigin::*};
    let s = |s: &str| Some(s.to_string());
    let expected: Vec<Seen> = [
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Canon EOS M10 Adobe Standard.dcp", Shared, s("Canon EOS M10"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Example Z1 Adobe Standard.dcp", Shared, s("Example Z1"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Google Pixel 7 Pro Adobe Standard.dcp", Shared, s("Google Pixel 7 Pro"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Google Pixel 7 Pro Telephoto Camera Adobe Standard.dcp", Shared, s("Google Pixel 7 Pro Telephoto Camera"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Google Pixel 7 Pro Ultrawide Camera Adobe Standard.dcp", Shared, s("Google Pixel 7 Pro Ultrawide Camera"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/LEICA M10 Adobe Standard.dcp", Shared, s("LEICA M10"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Sony ILCE-7M2 Adobe Standard.dcp", Shared, s("Sony ILCE-7M2"), None),
        (CameraProfile, "shared/CameraProfiles/Adobe Standard/Sony ILCE-7M4 Adobe Standard.dcp", Shared, s("Sony ILCE-7M4"), None),
        (CameraProfile, "shared/CameraProfiles/Camera/Sony ILCE-7M2/Sony ILCE-7M2 Camera Standard.dcp", Shared, s("Sony ILCE-7M2"), None),
        (CameraProfile, "shared/CameraProfiles/Camera/Sony ILCE-7M2/Sony ILCE-7M2 Camera Vivid.dcp", Shared, s("Sony ILCE-7M2"), None),
        (CameraProfile, "shared/CameraProfiles/Camera/Sony ILCE-7M4/Sony ILCE-7M4 Camera Standard.dcp", Shared, s("Sony ILCE-7M4"), None),
        (Look, "shared/Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp", Shared, None, None),
        (Look, "shared/Settings/Adobe/Profiles/Adobe Raw/Adobe Portrait.xmp", Shared, None, None),
        (LensProfile, "shared/LensProfiles/1.0/Canon/Canon EF 50mm f1.8 STM.lcp", Shared, None, s("Canon")),
        (LensProfile, "shared/LensProfiles/1.0/Pentax/smc PENTAX-DA 35mm F2.4.lcp", Shared, None, s("Pentax")),
        (LensProfile, "shared/LensProfiles/1.0/Sigma/SIGMA 35mm F1.4 DG DN A.lcp", Shared, None, s("Sigma")),
        (LensProfile, "shared/LensProfiles/1.0/Sony/Sony FE 24-70mm F2.8 GM.lcp", Shared, None, s("Sony")),
        (LensProfile, "user/LensProfiles/1.0/My Lens.lcp", User, None, None),
        (CameraProfile, "user/CameraProfiles/Fujifilm X-T5 Velvia.dcp", User, None, None),
        (CameraProfile, "user/CameraProfiles/Sony ILCE-7M2 Portra 400 SO.dcp", User, None, None),
        (Presets, "user/Settings", User, None, None),
        (Presets, "pictures/Lightroom Library.lrlibrary/0a1c/cr_settings", User, None, None),
        (Presets, "pictures/Lightroom Library.lrlibrary/b67b/cr_settings", User, None, None),
        (Catalog, "pictures/Lightroom/New.lrcat", User, None, None),
        (Catalog, "pictures/Lightroom/Old.lrcat", User, None, None),
    ]
    .into_iter()
    .map(|(kind, path, origin, camera, group)| (kind, path.to_string(), origin, camera, group))
    .collect();
    for layout in [Layout::MacOs, Layout::Windows] {
        let dir = tempfile::tempdir()?;
        let roots = install(dir.path(), layout)?;
        let listed = list(&roots);
        assert_eq!(seen(&roots, &listed), expected, "{layout:?}");
        let catalogs = listed.folders.iter().find(|f| f.kind == Catalog).unwrap();
        let used: Vec<_> = catalogs.files.iter().map(|f| f.used.unwrap()).collect();
        assert!(used[1] < used[0], "Old.lrcat was used first: {layout:?}");
        let searched: Vec<_> = listed
            .searched
            .iter()
            .map(|s| (s.kind, s.origin, relative(&roots, &s.path)))
            .collect();
        assert_eq!(
            searched,
            [
                (Catalog, User, "pictures/Lightroom".to_string()),
                (CameraProfile, Shared, "shared/CameraProfiles".to_string()),
                (LensProfile, Shared, "shared/LensProfiles".to_string()),
                (CameraProfile, User, "user/CameraProfiles".to_string()),
                (LensProfile, User, "user/LensProfiles".to_string()),
            ],
            "{layout:?}"
        );
    }
    Ok(())
}

#[test]
fn the_layouts_are_adobe_s() {
    let mac = Roots::macos(Path::new("/"), Some("/Users/jane".into()));
    assert_eq!(
        mac.shared.as_deref(),
        Some(Path::new("/Library/Application Support/Adobe/CameraRaw"))
    );
    assert_eq!(
        mac.user.as_deref(),
        Some(Path::new(
            "/Users/jane/Library/Application Support/Adobe/CameraRaw"
        ))
    );
    assert_eq!(
        mac.pictures.as_deref(),
        Some(Path::new("/Users/jane/Pictures"))
    );
    let windows = Roots::windows(
        Path::new("C:/ProgramData"),
        Some("C:/Users/jane/AppData/Roaming".into()),
        Some("C:/Users/jane".into()),
    );
    assert_eq!(
        windows.shared.as_deref(),
        Some(Path::new("C:/ProgramData/Adobe/CameraRaw"))
    );
    assert_eq!(
        windows.user.as_deref(),
        Some(Path::new("C:/Users/jane/AppData/Roaming/Adobe/CameraRaw"))
    );
    assert_eq!(
        windows.pictures.as_deref(),
        Some(Path::new("C:/Users/jane/Pictures"))
    );
    // Missing folders list nothing, and Camera Raw's weren't looked in.
    let missing = list(&Roots::macos(
        Path::new("/nowhere"),
        Some("/nowhere/home".into()),
    ));
    assert!(missing.folders.is_empty());
    assert_eq!(missing.searched.len(), 1, "only Pictures/Lightroom");
}

/// A name that isn't UTF-8 is left out rather than sent changed, and the
/// rest of the listing still goes. Not on macOS, whose file systems refuse
/// to make such a name (which is why Lightroom's folders never hold one).
#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn names_that_aren_t_utf8_are_left_out() -> std::io::Result<()> {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir()?;
    let roots = install(dir.path(), Layout::MacOs)?;
    let bad = |name: &[u8]| std::ffi::OsStr::from_bytes(name).to_owned();
    let pictures = roots.pictures.clone().unwrap();
    std::fs::write(
        pictures.join("Lightroom").join(bad(b"Catalog-\xff.lrcat")),
        b"",
    )?;
    let camera = roots
        .shared
        .clone()
        .unwrap()
        .join("CameraProfiles/Camera")
        .join(bad(b"Bad \xff"));
    std::fs::create_dir_all(&camera)?;
    std::fs::write(camera.join("Bad Camera Standard.dcp"), b"")?;
    let listed = list(&roots);
    let all = seen(&roots, &listed);
    assert!(
        !all.iter()
            .any(|(_, path, ..)| path.contains("Catalog-") || path.contains("Bad"))
    );
    let catalogs = listed
        .folders
        .iter()
        .find(|f| f.kind == FileKind::Catalog)
        .unwrap();
    assert_eq!(catalogs.files.len(), 2);
    serde_json::to_string(&listed).expect("every path is text");

    // A Pictures folder whose own path isn't text: everything else is still
    // listed, and it isn't named as searched.
    let mut roots = roots;
    let elsewhere = dir.path().join(bad(b"Bad-\xff")).join("Pictures");
    std::fs::create_dir_all(elsewhere.join("Lightroom"))?;
    roots.pictures = Some(elsewhere);
    let listed = list(&roots);
    serde_json::to_string(&listed).expect("every path is text");
    assert!(
        listed
            .folders
            .iter()
            .any(|f| f.kind == FileKind::CameraProfile)
    );
    assert!(!listed.searched.iter().any(|s| s.kind == FileKind::Catalog));
    Ok(())
}
