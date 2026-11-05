//! Which drive a path lives on, for Lightroom-style volume headers: found
//! from the file system's mount table by [`locate`] (off the UI thread, as
//! it can block on a stalled network mount), or guessed from the path alone
//! by [`volume_of`] where nothing could be found.
use std::path::{Component, Path, PathBuf, Prefix};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Volume {
    /// Display name: the drive's name, a drive letter, or the startup disk.
    pub name: String,
    /// Where the volume is mounted, for external and network drives; `None`
    /// for the startup disk, which is always attached.
    pub mount: Option<PathBuf>,
    /// Which disk it is, where the system tells: a local Windows drive's
    /// volume serial number, so another disk given the same drive letter is
    /// another drive (#891). `None` elsewhere, for a network drive, and for
    /// a drive an earlier release kept, which is then known by its mount.
    pub id: Option<String>,
}

impl Volume {
    /// The drive as this release names it: one an earlier release kept
    /// with Windows' `\\?\` prefix in its mount without it, as
    /// [`volume_of`] and [`locate`] now name it. Its name changes only where
    /// its path gave it (`\\?\D:` for the drive at `\\?\D:\`); a label
    /// stays as it is, and so does every drive kept elsewhere than Windows.
    pub(crate) fn plain(self) -> Volume {
        let Some(kept) = self.mount.as_deref() else {
            return self;
        };
        let mount = plain(kept);
        if mount == kept {
            return self;
        }
        // As 0.4.6's `volume_of` named a drive's root from its path.
        let parts: Vec<Component> = kept.components().collect();
        let by_path = match parts[..] {
            [Component::Prefix(prefix), Component::RootDir]
                if matches!(prefix.kind(), Prefix::VerbatimDisk(_)) =>
            {
                prefix.as_os_str().to_string_lossy().trim_end_matches('\\') == self.name
            }
            _ => false,
        };
        Volume {
            name: if by_path {
                volume_of(&mount).name
            } else {
                self.name
            },
            mount: Some(mount),
            id: self.id,
        }
    }
}
/// The volume `path` would be on, from its path alone: /Volumes/NAME on macOS, /media/USER/NAME,
/// /run/media/USER/NAME or /mnt/NAME on Linux, a drive letter or UNC share
/// on Windows, and otherwise the startup disk.
pub(crate) fn volume_of(path: &Path) -> Volume {
    let path = plain(path);
    let parts: Vec<Component> = path.components().collect();
    if let Some(Component::Prefix(prefix)) = parts.first() {
        let mount: PathBuf = parts.iter().take(2).collect();
        let name = match prefix.kind() {
            Prefix::UNC(server, share) | Prefix::VerbatimUNC(server, share) => {
                format!("{}\\{}", server.to_string_lossy(), share.to_string_lossy())
            }
            _ => prefix
                .as_os_str()
                .to_string_lossy()
                .trim_end_matches('\\')
                .to_string(),
        };
        return Volume {
            name,
            mount: Some(mount),
            id: None,
        };
    }
    let names: Vec<String> = parts
        .iter()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(n.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    let under = |depth: usize| -> Option<Volume> {
        let name = names.get(depth)?.clone();
        let mount = std::iter::once(PathBuf::from("/"))
            .chain(names.iter().take(depth + 1).map(PathBuf::from))
            .collect();
        Some(Volume {
            name,
            mount: Some(mount),
            id: None,
        })
    };
    let first = names.first().map(String::as_str);
    let external = match first {
        Some("Volumes") if cfg!(target_os = "macos") => under(1),
        Some("media") => under(2).or_else(|| under(1)),
        Some("run") if names.get(1).is_some_and(|n| n == "media") => under(3),
        Some("mnt") => under(1),
        _ => None,
    };
    external.unwrap_or_else(|| Volume {
        name: startup_name(),
        mount: None,
        id: None,
    })
}
/// `path` without Windows' `\\?\` prefix (`\\?\D:\x` is `D:\x`, and
/// `\\?\UNC\nas\share\x` is `\\nas\share\x`), so a drive or share is
/// one drive however its path is written: a root folder is kept canonical,
/// a folder located in it as it was chosen. Any other path is as it is.
pub(super) fn plain(path: &Path) -> PathBuf {
    let mut parts = path.components();
    let start = match parts.next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::VerbatimDisk(letter) => format!("{}:\\", letter as char),
            Prefix::VerbatimUNC(server, share) => format!(
                "\\\\{}\\{}\\",
                server.to_string_lossy(),
                share.to_string_lossy()
            ),
            _ => return path.to_path_buf(),
        },
        _ => return path.to_path_buf(),
    };
    let mut plain = PathBuf::from(start);
    plain.extend(parts.filter(|c| !matches!(c, Component::RootDir)));
    plain
}
/// Whether the volume holding `path` is mounted, for headless commands with
/// no volume checks of their own. The startup disk always is; another only
/// when its mount point is a directory on a device of its own, as an empty
/// directory left where a disk was mounted is not.
pub fn mounted(path: &Path) -> bool {
    volume_of(path).mount.is_none_or(|mount| is_mounted(&mount))
}
/// Whether a drive is mounted at `mount`: a directory on a device of its
/// own, as an empty directory left where a disk was mounted is not.
pub(crate) fn is_mounted(mount: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let Ok(m) = std::fs::metadata(mount) else {
            return false;
        };
        m.is_dir()
            && mount
                .parent()
                .is_some_and(|p| std::fs::metadata(p).is_ok_and(|p| p.dev() != m.dev()))
    }
    #[cfg(not(unix))]
    {
        mount.is_dir()
    }
}
/// The drive `path` is on, from the mount the file system reports for it:
/// macOS's `statfs` mount point and the volume's name, Linux's longest
/// mount point in /proc/self/mountinfo holding the path (each GVFS share a
/// drive of its own), Windows' `GetVolumePathNameW` and the volume's label
/// and serial number.
/// `None` where `path` isn't there, as an offline drive's folders aren't,
/// or its mount can't be read. Can block on a stalled network mount: call
/// it off the UI thread.
pub(crate) fn locate(path: &Path) -> Option<Volume> {
    let found = mount_of(path)?;
    Some(Volume {
        id: found.id,
        ..drive_at(&found.point, found.label.as_deref(), startup_mounts())
    })
}
/// What the file system reports of the mount holding a path.
struct Mount {
    point: PathBuf,
    label: Option<String>,
    /// See [`Volume::id`].
    id: Option<String>,
}
impl Mount {
    #[cfg_attr(windows, allow(dead_code))]
    fn at(point: PathBuf, label: Option<String>) -> Mount {
        Mount {
            point,
            label,
            id: None,
        }
    }
}
/// The name of the drive mounted at `mount`, from the path alone, for a
/// mount point the caller found: as [`locate`] names it, but without its
/// label, and with "/", a mount holding the home folder (a Linux /home
/// partition) and macOS's system volumes the startup disk.
pub(crate) fn name_of_mount(mount: &Path) -> String {
    // Not on Windows, whose drive letters keep their names (no startup disk).
    let holds_home = !cfg!(windows)
        && std::env::var_os("HOME").is_some_and(|h| Path::new(&h).starts_with(mount));
    let startup = if holds_home { mount } else { Path::new("/") };
    drive_at(mount, None, &[startup.to_path_buf()]).name
}
/// The drive mounted at `point` with volume label `label`: the startup disk
/// for one of its own mounts (macOS's system volumes, Linux's "/" and the
/// home folder's); a drive whose path [`volume_of`] names (/Volumes/NAME,
/// /run/media/USER/NAME, a UNC share) as it always has, and a drive letter
/// as Windows names it, by its label and letter (`Card (F:)`), so two disks
/// given one letter read apart (#891); any other by its label, or the
/// mount point's name without one.
fn drive_at(point: &Path, label: Option<&str>, startup: &[PathBuf]) -> Volume {
    let point = &plain(point);
    let system = cfg!(target_os = "macos") && point.starts_with("/System/Volumes");
    if system || startup.iter().any(|s| s == point) {
        return Volume {
            name: startup_name(),
            mount: None,
            id: None,
        };
    }
    let label = label.filter(|l| !l.is_empty());
    let by_path = volume_of(point);
    if by_path.mount.as_deref() == Some(point) {
        let letter = matches!(
            point.components().next(),
            Some(Component::Prefix(p)) if matches!(p.kind(), Prefix::Disk(_))
        );
        return match label {
            Some(label) if letter => Volume {
                name: format!("{label} ({})", by_path.name),
                ..by_path
            },
            _ => by_path,
        };
    }
    let name = label
        .map(str::to_owned)
        .or_else(|| point.file_name().map(|n| n.to_string_lossy().into_owned()))
        .unwrap_or_else(|| point.display().to_string());
    Volume {
        name,
        mount: Some(point.to_path_buf()),
        id: None,
    }
}
/// The startup disk's mount points: those of "/" and of the home folder
/// (macOS's Data volume, a Linux /home partition). Windows has none: each
/// drive letter is a drive of its own, as it always was.
fn startup_mounts() -> &'static [PathBuf] {
    static MOUNTS: std::sync::OnceLock<Vec<PathBuf>> = std::sync::OnceLock::new();
    MOUNTS.get_or_init(|| {
        if cfg!(windows) {
            return Vec::new();
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        std::iter::once(PathBuf::from("/"))
            .chain(home)
            .filter_map(|p| mount_of(&p).map(|m| m.point))
            .collect()
    })
}
/// macOS: `statfs`'s mount point, named by the volume.
#[cfg(target_os = "macos")]
fn mount_of(path: &Path) -> Option<Mount> {
    use std::os::unix::ffi::OsStrExt as _;
    let fs = rustix::fs::statfs(path).ok()?;
    let point: Vec<u8> = fs
        .f_mntonname
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| *c as u8)
        .collect();
    let point = PathBuf::from(std::ffi::OsStr::from_bytes(&point));
    let label = volume_name(&point);
    Some(Mount::at(point, label))
}
/// The name macOS gives the volume mounted at `point` (`NSURLVolumeNameKey`).
#[cfg(target_os = "macos")]
fn volume_name(point: &Path) -> Option<String> {
    use objc2_foundation::{NSString, NSURL, NSURLVolumeNameKey};
    // Foundation autoreleases what it returns, and the volume check's
    // thread has no pool of its own to drain.
    objc2::rc::autoreleasepool(|_| {
        let url = NSURL::from_directory_path(point)?;
        let mut value = None;
        // SAFETY: NSURLVolumeNameKey is a constant Foundation defines, and
        // its value, an NSString, is checked before use.
        unsafe { url.getResourceValue_forKey_error(&mut value, NSURLVolumeNameKey) }.ok()?;
        let name = value?.downcast::<NSString>().ok()?;
        Some(name.to_string())
    })
}
/// Linux: the longest mount point holding the (resolved) path, named by
/// its name; below GVFS's FUSE mount, the share the path is on.
#[cfg(target_os = "linux")]
fn mount_of(path: &Path) -> Option<Mount> {
    let path = std::fs::canonicalize(path).ok()?;
    let table = parse_mountinfo(&std::fs::read_to_string("/proc/self/mountinfo").ok()?);
    let points: Vec<PathBuf> = table.iter().map(|(p, _)| p.clone()).collect();
    let point = longest_mount(&path, &points)?.clone();
    let gvfs = table
        .iter()
        .any(|(p, fs)| *p == point && fs == "fuse.gvfsd-fuse");
    if gvfs && let Some((share, name)) = gvfs_share(&point, &path) {
        return Some(Mount::at(share, Some(name)));
    }
    Some(Mount::at(point, None))
}
/// Windows: `GetVolumePathNameW`'s volume, named by its label and known
/// by its serial number; a network drive by its path alone, as the server's
/// file system's label and serial say nothing of the share.
#[cfg(windows)]
fn mount_of(path: &Path) -> Option<Mount> {
    use std::os::windows::ffi::{OsStrExt as _, OsStringExt as _};
    use windows_sys::Win32::Storage::FileSystem::{
        GetDriveTypeW, GetVolumeInformationW, GetVolumePathNameW,
    };
    use windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE;
    std::fs::metadata(path).ok()?;
    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    let mut point = vec![0u16; wide.len().max(261)];
    let len = u32::try_from(point.len()).ok()?;
    // SAFETY: `wide` is NUL-terminated and `point` holds `len` characters,
    // at least the path's length as the call needs.
    if unsafe { GetVolumePathNameW(wide.as_ptr(), point.as_mut_ptr(), len) } == 0 {
        return None;
    }
    let end = point.iter().position(|&c| c == 0)?;
    let found = PathBuf::from(std::ffi::OsString::from_wide(&point[..end]));
    // SAFETY: `point` is the NUL-terminated volume path just written.
    if unsafe { GetDriveTypeW(point.as_ptr()) } == DRIVE_REMOTE {
        return Some(Mount {
            point: found,
            label: None,
            id: None,
        });
    }
    let mut label = [0u16; 261];
    let mut serial = 0u32;
    // SAFETY: `point` is the NUL-terminated volume path just written,
    // `label` holds the 261 characters given and `serial` one number; the
    // other outputs are unused.
    let read = unsafe {
        GetVolumeInformationW(
            point.as_ptr(),
            label.as_mut_ptr(),
            label.len() as u32,
            &mut serial,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
        )
    } != 0;
    let label = read
        .then(|| {
            let n = label.iter().position(|&c| c == 0).unwrap_or(label.len());
            String::from_utf16_lossy(&label[..n])
        })
        .filter(|l| !l.is_empty());
    // As `vol` prints it.
    let id = read.then(|| format!("{:04X}-{:04X}", serial >> 16, serial & 0xFFFF));
    Some(Mount {
        point: found,
        label,
        id,
    })
}
/// Elsewhere no mount is read, and the path decides ([`volume_of`]).
#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn mount_of(_path: &Path) -> Option<Mount> {
    None
}
/// The longest of `points` holding `path`, by whole names.
#[cfg_attr(not(any(target_os = "linux", all(test, unix))), allow(dead_code))]
fn longest_mount<'a>(path: &Path, points: &'a [PathBuf]) -> Option<&'a PathBuf> {
    points
        .iter()
        .filter(|p| p.is_absolute() && path.starts_with(p))
        .max_by_key(|p| p.components().count())
}
/// /proc/self/mountinfo's mount points (its fifth field, with `\040` and
/// the other octal escapes read) and each one's file system type.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn parse_mountinfo(text: &str) -> Vec<(PathBuf, String)> {
    text.lines()
        .filter_map(|line| {
            let (fields, rest) = line.split_once(" - ")?;
            let point = fields.split(' ').nth(4)?;
            let kind = rest.split(' ').next()?;
            Some((PathBuf::from(unescape(point)), kind.to_string()))
        })
        .collect()
}
/// A mountinfo field with its octal escapes (`\040` a space) read.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let octal = bytes.get(i + 1..i + 4).and_then(|d| {
            d.iter()
                .all(|c| (b'0'..=b'7').contains(c))
                .then(|| u8::from_str_radix(std::str::from_utf8(d).ok()?, 8).ok())
                .flatten()
        });
        match (bytes[i], octal) {
            (b'\\', Some(byte)) => {
                out.push(byte);
                i += 4;
            }
            (byte, _) => {
                out.push(byte);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}
/// The GVFS share below the FUSE mount `gvfs` holding `path` (a folder
/// such as `smb-share:server=nas,share=photos`), named by its share, else
/// its host or server, else the folder's name.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
fn gvfs_share(gvfs: &Path, path: &Path) -> Option<(PathBuf, String)> {
    let folder = path.strip_prefix(gvfs).ok()?.components().next()?;
    let folder = folder.as_os_str().to_string_lossy().into_owned();
    let (_, keys) = folder.split_once(':').unwrap_or(("", &folder));
    let value = |key: &str| {
        keys.split(',')
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
            .map(str::to_owned)
    };
    let name = value("share")
        .or_else(|| value("host"))
        .or_else(|| value("server"))
        .unwrap_or_else(|| folder.clone());
    Some((gvfs.join(&folder), name))
}
/// The startup disk's name: on macOS the /Volumes entry that links to "/"
/// (usually "Macintosh HD").
fn startup_name() -> String {
    static NAME: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    NAME.get_or_init(|| {
        if cfg!(target_os = "macos")
            && let Ok(entries) = std::fs::read_dir("/Volumes")
        {
            for entry in entries.flatten() {
                if std::fs::read_link(entry.path()).is_ok_and(|t| t == Path::new("/")) {
                    return entry.file_name().to_string_lossy().into_owned();
                }
            }
        }
        if cfg!(target_os = "macos") {
            "This Mac".into()
        } else {
            "This Computer".into()
        }
    })
    .clone()
}
/// Free and total bytes on the volume holding `path`, from `df` on macOS and
/// Linux; `None` where that isn't available.
pub(crate) fn space(path: &Path) -> Option<(u64, u64)> {
    if cfg!(windows) {
        return None;
    }
    let out = std::process::Command::new("df")
        .arg("-Pk")
        .arg(path)
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let fields: Vec<&str> = text.lines().nth(1)?.split_whitespace().collect();
    let total: u64 = fields.get(1)?.parse().ok()?;
    let free: u64 = fields.get(3)?.parse().ok()?;
    Some((free * 1024, total * 1024))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn names_external_and_local_volumes() {
        let usb = volume_of(Path::new("/Volumes/Samsung USB/05 Pacific Highway"));
        let local = volume_of(Path::new("/Users/me/Pictures"));
        let linux = volume_of(Path::new("/run/media/me/CARD/DCIM"));
        if cfg!(target_os = "macos") {
            assert_eq!(usb.name, "Samsung USB");
            assert_eq!(usb.mount, Some(PathBuf::from("/Volumes/Samsung USB")));
        }
        assert_eq!(local.mount, None);
        assert_eq!(linux.name, "CARD");
        assert_eq!(linux.mount, Some(PathBuf::from("/run/media/me/CARD")));
    }
    /// Elsewhere than on Windows a drive kept is read as it was, even one
    /// whose folder's name begins as Windows' `\\?\` prefix does.
    #[test]
    #[cfg(unix)]
    fn a_unix_drive_kept_is_read_as_it_was() {
        let drive = volume_of(Path::new(r"/mnt/\\?\Photos/2026"));
        assert_eq!(drive.name, r"\\?\Photos");
        assert_eq!(drive.clone().plain(), drive);
    }
    /// A drive or share is one drive whether its path is written with
    /// Windows' `\\?\` prefix (a root folder, kept canonical) or without
    /// it (a folder located as it was chosen, #678).
    #[test]
    #[cfg(windows)]
    fn a_windows_drive_is_one_however_its_path_is_written() {
        let drive = volume_of(Path::new(r"D:\Elsewhere\Party"));
        assert_eq!(drive.name, "D:");
        assert_eq!(drive.mount, Some(PathBuf::from(r"D:\")));
        assert_eq!(volume_of(Path::new(r"\\?\D:\Photos\2026")), drive);
        assert_eq!(drive_at(Path::new(r"\\?\D:\"), None, &[]), drive);
        // A labelled drive letter is named as Windows names it (#891).
        let data = Volume {
            name: "Data (D:)".into(),
            ..drive.clone()
        };
        assert_eq!(drive_at(Path::new(r"\\?\D:\"), Some("Data"), &[]), data);
        assert_eq!(drive_at(Path::new(r"D:\"), Some("Data"), &[]), data);
        assert_eq!(drive_at(Path::new(r"D:\"), Some(""), &[]), drive);
        let share = volume_of(Path::new(r"\\nas\photos\Party"));
        assert_eq!(share.name, r"nas\photos");
        assert_eq!(share.mount, Some(PathBuf::from(r"\\nas\photos\")));
        assert_eq!(volume_of(Path::new(r"\\?\UNC\nas\photos\2026")), share);
        assert_eq!(
            drive_at(Path::new(r"\\?\UNC\nas\photos\"), None, &[]),
            share
        );
        // Kept by 0.4.6 as its path named it, it is named as now; the label
        // of a drive mounted in a folder (any text, even this) stays.
        let kept = |name: &str, mount: &str| Volume {
            name: name.into(),
            mount: Some(mount.into()),
            id: None,
        };
        assert_eq!(kept(r"\\?\D:", r"\\?\D:\").plain(), drive);
        assert_eq!(kept(r"nas\photos", r"\\?\UNC\nas\photos\").plain(), share);
        assert_eq!(
            kept(r"\\?\Photos", r"\\?\C:\Mounted\Card\").plain(),
            drive_at(Path::new(r"C:\Mounted\Card\"), Some(r"\\?\Photos"), &[])
        );
    }
}

#[cfg(test)]
mod mount_tests {
    use super::*;

    fn startup() -> Volume {
        Volume {
            name: startup_name(),
            mount: None,
            id: None,
        }
    }

    /// A drive mounted outside /Volumes is its own, named by its label, and
    /// by its mount point's name without one; the startup disk's mounts
    /// (macOS's "/" and the Data volume) are the startup disk.
    #[test]
    fn a_drive_is_named_by_its_mount_and_label() {
        let startup_mounts = [PathBuf::from("/"), PathBuf::from("/System/Volumes/Data")];
        let image = drive_at(
            Path::new("/tmp/rmk590-mnt"),
            Some("RMK590"),
            &startup_mounts,
        );
        assert_eq!(image.name, "RMK590");
        assert_eq!(image.mount, Some(PathBuf::from("/tmp/rmk590-mnt")));
        let unnamed = drive_at(Path::new("/home/me/card"), Some(""), &startup_mounts);
        assert_eq!(unnamed.name, "card");
        assert_eq!(unnamed.mount, Some(PathBuf::from("/home/me/card")));
        assert_eq!(drive_at(Path::new("/"), None, &startup_mounts), startup());
        assert_eq!(
            drive_at(
                Path::new("/System/Volumes/Data"),
                Some("Data"),
                &startup_mounts
            ),
            startup()
        );
        // Linux's removable drives keep the names they had from their paths.
        let card = drive_at(Path::new("/run/media/me/CARD"), None, &startup_mounts);
        assert_eq!(card, volume_of(Path::new("/run/media/me/CARD/DCIM")));
    }

    /// A /Volumes drive keeps the name and mount it has always had, even when
    /// its label differs (a second "Untitled" is mounted as "Untitled 1").
    #[test]
    #[cfg(target_os = "macos")]
    fn a_volumes_drive_keeps_its_folder_name() {
        let usb = drive_at(Path::new("/Volumes/Untitled 1"), Some("Untitled"), &[]);
        assert_eq!(usb, volume_of(Path::new("/Volumes/Untitled 1/DCIM")));
        assert_eq!(usb.name, "Untitled 1");
    }

    /// The mount a path is on is the longest mount point holding it, whole
    /// names only. (Linux's mount table: Windows paths need a drive.)
    #[test]
    #[cfg(unix)]
    fn the_longest_mount_point_holds_a_path() {
        let points = [
            PathBuf::from("/"),
            PathBuf::from("/home"),
            PathBuf::from("/run/media/me/CARD"),
        ];
        let on = |p: &str| longest_mount(Path::new(p), &points).cloned();
        assert_eq!(
            on("/run/media/me/CARD/DCIM"),
            Some(PathBuf::from("/run/media/me/CARD"))
        );
        assert_eq!(
            on("/run/media/me/CARD"),
            Some(PathBuf::from("/run/media/me/CARD"))
        );
        assert_eq!(on("/run/media/me/CARDS/DCIM"), Some(PathBuf::from("/")));
        assert_eq!(on("/home/me/Pictures"), Some(PathBuf::from("/home")));
        assert_eq!(longest_mount(Path::new("relative"), &points), None);
    }

    /// /proc/self/mountinfo's mount points, octal escapes read, with their
    /// file systems.
    #[test]
    fn mountinfo_lists_mount_points() {
        let text = "\
22 1 8:2 / / rw,relatime shared:1 - ext4 /dev/sda2 rw
40 22 8:17 / /run/media/me/SD\\040CARD rw,nosuid - exfat /dev/sdb1 rw
41 22 0:50 / /run/user/1000/gvfs rw - fuse.gvfsd-fuse gvfsd-fuse rw
broken line
";
        assert_eq!(
            parse_mountinfo(text),
            vec![
                (PathBuf::from("/"), "ext4".to_string()),
                (PathBuf::from("/run/media/me/SD CARD"), "exfat".to_string()),
                (
                    PathBuf::from("/run/user/1000/gvfs"),
                    "fuse.gvfsd-fuse".to_string()
                ),
            ]
        );
    }

    /// Each GVFS share is a drive of its own, below the one FUSE mount that
    /// holds them all, named by its share (or host).
    #[test]
    fn a_gvfs_share_is_its_own_drive() {
        let gvfs = Path::new("/run/user/1000/gvfs");
        let share = gvfs.join("smb-share:server=nas.local,share=photos");
        assert_eq!(
            gvfs_share(gvfs, &share.join("2024/a.NEF")),
            Some((share.clone(), "photos".to_string()))
        );
        let sftp = gvfs.join("sftp:host=box");
        assert_eq!(
            gvfs_share(gvfs, &sftp),
            Some((sftp.clone(), "box".to_string()))
        );
        assert_eq!(gvfs_share(gvfs, gvfs), None);
    }

    /// A real drive mounted anywhere (a disk image attached outside
    /// /Volumes, a Linux card) is found as its own, by its mount point and
    /// label, from a folder on it.
    #[test]
    #[ignore = "needs RAWMAKASE_TEST_MOUNT: a drive's mount point, and RAWMAKASE_TEST_MOUNT_NAME"]
    fn a_drive_mounted_anywhere_is_found() {
        let (Some(mount), Some(name)) = (
            std::env::var_os("RAWMAKASE_TEST_MOUNT"),
            std::env::var("RAWMAKASE_TEST_MOUNT_NAME").ok(),
        ) else {
            return;
        };
        // As the system names it: /tmp is /private/tmp on macOS.
        let mount = std::fs::canonicalize(mount).unwrap();
        // Its id is whatever the system gives the drive.
        let named = |v: Volume| (v.name, v.mount);
        let found = (name, Some(mount.clone()));
        assert_eq!(locate(&mount).map(named), Some(found.clone()));
        let dir = tempfile::tempdir_in(&mount).unwrap();
        assert_eq!(locate(dir.path()).map(named), Some(found));
        assert!(is_mounted(&mount));
    }

    /// The live table: the home folder is on the startup disk, and a path
    /// that isn't there has no drive to find.
    #[test]
    fn locates_the_home_folder_on_the_startup_disk() {
        #[cfg(unix)]
        if let Some(home) = std::env::var_os("HOME") {
            assert_eq!(locate(Path::new(&home)), Some(startup()));
        }
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(locate(&dir.path().join("no such folder (590)")), None);
    }
}
