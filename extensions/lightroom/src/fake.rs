//! A small Lightroom and Camera Raw install in a folder, for tests here and in
//! the app: Adobe's folders as either platform lays them out, with empty
//! files, since nothing reads them.
use crate::Roots;
use std::path::Path;

/// Which platform's layout [`install`] writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Layout {
    MacOs,
    Windows,
}

/// The shared folder's files, under Camera Raw's folder.
pub const SHARED: &[&str] = &[
    "CameraProfiles/Adobe Standard/Sony ILCE-7M2 Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/Sony ILCE-7M4 Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/LEICA M10 Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/Canon EOS M10 Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/Google Pixel 7 Pro Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/Google Pixel 7 Pro Telephoto Camera Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/Google Pixel 7 Pro Ultrawide Camera Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/Example Z1 Adobe Standard.dcp",
    "CameraProfiles/Adobe Standard/readme.txt",
    "CameraProfiles/Camera/Sony ILCE-7M2/Sony ILCE-7M2 Camera Standard.dcp",
    "CameraProfiles/Camera/Sony ILCE-7M2/Sony ILCE-7M2 Camera Vivid.dcp",
    "CameraProfiles/Camera/Sony ILCE-7M4/Sony ILCE-7M4 Camera Standard.dcp",
    "Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp",
    "Settings/Adobe/Profiles/Adobe Raw/Adobe Portrait.xmp",
    "LensProfiles/1.0/Sony/Sony FE 24-70mm F2.8 GM.lcp",
    "LensProfiles/1.0/Sigma/SIGMA 35mm F1.4 DG DN A.lcp",
    "LensProfiles/1.0/Canon/Canon EF 50mm f1.8 STM.lcp",
    "LensProfiles/1.0/Pentax/smc PENTAX-DA 35mm F2.4.lcp",
];

/// The user's files, under their Camera Raw folder.
pub const USER: &[&str] = &[
    "LensProfiles/1.0/My Lens.lcp",
    "CameraProfiles/Sony ILCE-7M2 Portra 400 SO.dcp",
    "CameraProfiles/Fujifilm X-T5 Velvia.dcp",
    "CameraProfiles/.hidden.dcp",
    "Settings/My Presets/Warm.xmp",
    "Settings/Mine.xmp",
    "Settings/Defaults/Default.xmp",
];

/// The user's files, under Pictures: two catalogs, `Old.lrcat` used before
/// `New.lrcat`, and Lightroom's library with two accounts' synced presets.
pub const PICTURES: &[&str] = &[
    "Lightroom/Old.lrcat",
    "Lightroom/New.lrcat",
    "Lightroom/notes.txt",
    "Lightroom Library.lrlibrary/b67b/cr_settings/Synced.xmp",
    "Lightroom Library.lrlibrary/0a1c/cr_settings/Other.xmp",
    "Lightroom Library.lrlibrary/9f3e/cr_defaults/Default.xmp",
];

/// Writes the install under `root` in `layout`, and returns its roots.
pub fn install(root: &Path, layout: Layout) -> std::io::Result<Roots> {
    let home = root.join("home");
    let roots = match layout {
        Layout::MacOs => Roots::macos(root, Some(home)),
        Layout::Windows => Roots::windows(
            &root.join("ProgramData"),
            Some(home.join("AppData/Roaming")),
            Some(home),
        ),
    };
    let sets = [
        (roots.shared.as_ref(), SHARED),
        (roots.user.as_ref(), USER),
        (roots.pictures.as_ref(), PICTURES),
    ];
    for (dir, files) in sets {
        let dir = dir.expect("both layouts have every root");
        for file in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().expect("a file in a folder"))?;
            std::fs::write(&path, b"")?;
        }
    }
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
    let pictures = roots.pictures.as_ref().expect("both layouts have Pictures");
    std::fs::File::options()
        .write(true)
        .open(pictures.join("Lightroom/Old.lrcat"))?
        .set_modified(old)?;
    Ok(roots)
}
