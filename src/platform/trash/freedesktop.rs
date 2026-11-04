//! The freedesktop.org Trash (Trash specification 1.0), written here so
//! that every step fails closed. A file on the device of the user's data
//! folder goes to the home Trash, `$XDG_DATA_HOME/Trash`; one elsewhere to
//! its mount's `.Trash/$uid` (when `.Trash` is a sticky folder, not a link)
//! or `.Trash-$uid` (made if absent; a folder of this user's, not a link).
//! Its `info/<name>.trashinfo` is written first, exclusively, then the file
//! is renamed into `files/<name>` without replacing anything; a failed
//! rename takes the `.trashinfo` back. Each folder made, the `.trashinfo`
//! and the file in `files/` are on disk (their folders synced) before the
//! receipt: the catalog lets a photo go on it. A Trash folder another user
//! can write to is not used, nor any Trash for a file on an SMB share,
//! where no rename can promise not to replace a file another computer
//! put at its name ([`fsops::rename_exclusive`]). A Trash that can't be used, or one on
//! another device, is [`TrashError::Unsupported`]: nothing is copied across
//! devices, and nothing deleted.
//!
//! Where the Trashes are comes in as [`Places`], so tests point them at
//! temporary folders.
use crate::catalog::file_ops::{TrashError, TrashReceipt, TrashSupport};
use rawmakase_fs::{self as fsops, FsError};
use std::ffi::{OsStr, OsString};
use std::fs::{self, DirBuilder, OpenOptions};
use std::io::{self, Write as _};
use std::os::unix::ffi::OsStrExt as _;
use std::os::unix::fs::{DirBuilderExt as _, MetadataExt as _, OpenOptionsExt as _};
use std::path::{Path, PathBuf};

/// How many names are tried before giving up on a crowded Trash.
const NAMES: u32 = 10_000;

/// The home Trash and the device of the data folder it is in.
pub(super) struct Home {
    trash: PathBuf,
    device: u64,
}

impl Home {
    /// The Trash in `data` (`$XDG_DATA_HOME`), on the device of `data` or
    /// of the nearest folder above it that exists. `None` when none does.
    pub(super) fn under(data: &Path) -> Option<Self> {
        let device = data
            .ancestors()
            .find_map(|dir| fs::metadata(dir).ok())?
            .dev();
        Some(Self {
            trash: data.join("Trash"),
            device,
        })
    }
}

/// This user's Trashes.
pub(super) struct Places<'a> {
    pub(super) home: Option<Home>,
    pub(super) uid: u32,
    /// The top folder of the mount a folder is on, given the folder and the
    /// mount's device.
    pub(super) top: &'a dyn Fn(&Path, u64) -> Option<PathBuf>,
}

/// Puts `path` in its Trash, saying it was deleted at `date`
/// ([`deletion_date`]).
pub(super) fn trash(path: &Path, places: &Places, date: &str) -> Result<TrashReceipt, TrashError> {
    trash_with(path, places, date, &Fs::real())
}

/// Whether `path` could go to its Trash, told by the steps [`trash`] would
/// take without making or writing anything: `Yes` where its Trash is there
/// and usable (or, for the home Trash, could be made), `No` where `trash`
/// would refuse ([`TrashError::Unsupported`]) as things are, and `Unknown`
/// where it can't be told (a Trash at the top of a mount still to be made,
/// whose mount may not let it be private).
pub(super) fn supports(path: &Path, places: &Places) -> TrashSupport {
    let Ok(source) = Source::of(path) else {
        return TrashSupport::Unknown;
    };
    if source.on_share() {
        return TrashSupport::No;
    }
    if let Some(home) = places.home.as_ref().filter(|h| h.device == source.device) {
        // Through a link, as `target` follows it.
        return match fs::metadata(&home.trash) {
            Ok(m) if m.is_dir() && private(&m, places.uid) && m.dev() == source.device => {
                inside(&home.trash, places.uid, source.device)
            }
            Ok(_) => TrashSupport::No,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let made_in = home.trash.ancestors().skip(1).find(|d| d.exists());
                match made_in.map(writable) {
                    Some(Some(true)) => TrashSupport::Yes,
                    Some(Some(false)) => TrashSupport::No,
                    _ => TrashSupport::Unknown,
                }
            }
            Err(_) => TrashSupport::Unknown,
        };
    }
    let Some(top) = (places.top)(&source.folder, source.device) else {
        return TrashSupport::No;
    };
    let shared = top.join(".Trash");
    let sticky = fs::symlink_metadata(&shared).is_ok_and(|m| m.is_dir() && m.mode() & 0o1000 != 0);
    let mine = if sticky {
        own(
            &shared.join(places.uid.to_string()),
            places.uid,
            source.device,
        )
    } else {
        TrashSupport::No
    };
    let own = own(
        &top.join(format!(".Trash-{}", places.uid)),
        places.uid,
        source.device,
    );
    match (mine, own) {
        (TrashSupport::Yes, _) | (_, TrashSupport::Yes) => TrashSupport::Yes,
        (TrashSupport::No, TrashSupport::No) => TrashSupport::No,
        _ => TrashSupport::Unknown,
    }
}

/// What [`own_folder`] would make of `path` as a Trash folder, without
/// making it: usable, refused, or still to be made (where its parent lets
/// it be, `Unknown`: the mount decides whether it would be private).
fn own(path: &Path, uid: u32, device: u64) -> TrashSupport {
    match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() && private(&m, uid) && m.dev() == device => inside(path, uid, device),
        Ok(_) => TrashSupport::No,
        Err(e) if e.kind() == io::ErrorKind::NotFound => match path.parent().and_then(writable) {
            Some(false) => TrashSupport::No,
            _ => TrashSupport::Unknown,
        },
        Err(_) => TrashSupport::Unknown,
    }
}

/// Whether Trash folder `dir`'s `files/` and `info/`, where they are
/// there, are usable ([`own_folder`]), and `files/` on `device`, as
/// [`put`] needs (as the Trash folder itself must be).
fn inside(dir: &Path, uid: u32, device: u64) -> TrashSupport {
    for sub in ["files", "info"] {
        match fs::symlink_metadata(dir.join(sub)) {
            Ok(m) if m.is_dir() && private(&m, uid) && (sub == "info" || m.dev() == device) => {}
            Ok(_) => return TrashSupport::No,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return TrashSupport::Unknown,
        }
    }
    TrashSupport::Yes
}

/// Whether this user may make a folder in `dir`; `None` when that can't be
/// told.
fn writable(dir: &Path) -> Option<bool> {
    use rustix::fs::{Access, access};
    match access(dir, Access::WRITE_OK | Access::EXEC_OK) {
        Ok(()) => Some(true),
        Err(e)
            if [
                rustix::io::Errno::ACCESS,
                rustix::io::Errno::ROFS,
                rustix::io::Errno::PERM,
            ]
            .contains(&e) =>
        {
            Some(false)
        }
        Err(_) => None,
    }
}

type Rename<'a> = &'a dyn Fn(&Path, &Path) -> Result<(), FsError>;
type Sync<'a> = &'a dyn Fn(&Path) -> Result<(), FsError>;

/// The steps that change the disk or make it durable, which tests replace.
pub(super) struct Fs<'a> {
    /// An exclusive rename.
    pub(super) rename: Rename<'a>,
    /// Makes a folder's entries durable.
    pub(super) sync: Sync<'a>,
}

impl Fs<'static> {
    pub(super) fn real() -> Self {
        Self {
            rename: &fsops::rename_exclusive,
            sync: &fsops::sync_dir,
        }
    }
}

fn trash_with(
    path: &Path,
    places: &Places,
    date: &str,
    ops: &Fs,
) -> Result<TrashReceipt, TrashError> {
    let source = Source::of(path)?;
    if source.on_share() {
        return Err(TrashError::Unsupported);
    }
    let target = target(&source, places, ops)?;
    put(&source, &target, places.uid, date, ops)
}

/// The file to trash: its name in its folder, resolved, and its device.
struct Source {
    path: PathBuf,
    name: OsString,
    folder: PathBuf,
    device: u64,
}

impl Source {
    fn of(path: &Path) -> Result<Self, TrashError> {
        let name = path
            .file_name()
            .ok_or_else(|| TrashError::Failed(format!("{} has no name", path.display())))?;
        let folder = match path.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        // The link the path may end in is what goes, not what it points to.
        let device = fs::symlink_metadata(path).map_err(failed)?.dev();
        let folder = fs::canonicalize(folder).map_err(failed)?;
        Ok(Self {
            path: folder.join(name),
            name: name.to_owned(),
            folder,
            device,
        })
    }

    /// Whether it is on an SMB share, whose Trash isn't used: before
    /// anything is made there.
    fn on_share(&self) -> bool {
        fsops::volume_facts(&self.folder).is_ok_and(|f| f.kind == fsops::VolumeKind::Smb)
    }
}

/// A Trash folder (with `files/` and `info/`), and the path its
/// `.trashinfo` names: absolute in the home Trash, relative to the top of
/// the mount in the others.
struct Target {
    dir: PathBuf,
    original: PathBuf,
}

fn target(source: &Source, places: &Places, ops: &Fs) -> Result<Target, TrashError> {
    if let Some(home) = places.home.as_ref().filter(|h| h.device == source.device) {
        // Made if absent, as the specification says. A link to it is the
        // user's choice; the Trash is used where it really is, so the
        // receipt needs no link to find the file. Each folder down to it
        // is made durable in its parent every time, up to the top of its
        // mount: one an earlier attempt made may not have been. `put`
        // checks its device.
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&home.trash)
            .map_err(|_| TrashError::Unsupported)?;
        let dir = fs::canonicalize(&home.trash).map_err(|_| TrashError::Unsupported)?;
        sync_up(&dir, home.device, ops)?;
        let m = fs::metadata(&dir).map_err(|_| TrashError::Unsupported)?;
        if !private(&m, places.uid) {
            return Err(TrashError::Unsupported);
        }
        return Ok(Target {
            dir,
            original: source.path.clone(),
        });
    }
    let top = (places.top)(&source.folder, source.device).ok_or(TrashError::Unsupported)?;
    let dir = top_trash(&top, places.uid, ops)?;
    let original = source
        .path
        .strip_prefix(&top)
        .map_or_else(|_| source.path.clone(), Path::to_path_buf);
    Ok(Target { dir, original })
}

/// `$top/.Trash/$uid` when `.Trash` is a sticky folder and not a link, else
/// `$top/.Trash-$uid`.
fn top_trash(top: &Path, uid: u32, ops: &Fs) -> Result<PathBuf, TrashError> {
    let shared = top.join(".Trash");
    let sticky = fs::symlink_metadata(&shared).is_ok_and(|m| m.is_dir() && m.mode() & 0o1000 != 0);
    if sticky && let Ok(mine) = own_folder(&shared.join(uid.to_string()), uid, ops) {
        // `.Trash` itself, in the top of the mount, every time too.
        sync_parent(&shared, ops)?;
        return Ok(mine);
    }
    own_folder(&top.join(format!(".Trash-{uid}")), uid, ops)
}

/// `path` as a folder of user `uid`'s, made with mode 0700 if absent, and
/// durable in its parent (each time: an earlier attempt may have made it
/// and not got that far): it must be a folder, not a link, theirs, and
/// writable by no one else.
fn own_folder(path: &Path, uid: u32, ops: &Fs) -> Result<PathBuf, TrashError> {
    match DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Err(TrashError::Unsupported),
    }
    let m = fs::symlink_metadata(path).map_err(|_| TrashError::Unsupported)?;
    if !m.is_dir() || !private(&m, uid) {
        return Err(TrashError::Unsupported);
    }
    sync_parent(path, ops)?;
    Ok(path.to_path_buf())
}

/// Whether a Trash folder is user `uid`'s, and no one else may write in it
/// (to take or replace what it holds).
fn private(m: &fs::Metadata, uid: u32) -> bool {
    m.uid() == uid && m.mode() & 0o022 == 0
}

/// Makes `dir` and each folder above it durable in its parent, up to the
/// top of mount `device`.
fn sync_up(dir: &Path, device: u64, ops: &Fs) -> Result<(), TrashError> {
    let on = |d: &Path| fs::metadata(d).is_ok_and(|m| m.dev() == device);
    for d in dir.ancestors().take_while(|d| on(d)) {
        match d.parent() {
            Some(parent) if on(parent) => sync_parent(d, ops)?,
            _ => break,
        }
    }
    Ok(())
}

/// Makes `dir`'s entry in its parent durable.
fn sync_parent(dir: &Path, ops: &Fs) -> Result<(), TrashError> {
    let parent = dir.parent().unwrap_or(Path::new("/"));
    (ops.sync)(parent).map_err(|e| TrashError::Failed(e.to_string()))
}

/// Writes the `.trashinfo`, then renames the file next to it under the
/// first free name.
fn put(
    source: &Source,
    target: &Target,
    uid: u32,
    date: &str,
    ops: &Fs,
) -> Result<TrashReceipt, TrashError> {
    let files = own_folder(&target.dir.join("files"), uid, ops)?;
    let info = own_folder(&target.dir.join("info"), uid, ops)?;
    let same_device = fs::symlink_metadata(&files).is_ok_and(|m| m.dev() == source.device);
    if !same_device {
        return Err(TrashError::Unsupported);
    }
    let text = format!(
        "[Trash Info]\nPath={}\nDeletionDate={date}\n",
        encoded(&target.original)
    );
    for n in 1..=NAMES {
        let name = candidate(&source.name, n);
        let mut info_name = name.clone();
        info_name.push(".trashinfo");
        let info_path = info.join(info_name);
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&info_path)
        {
            Ok(file) => file,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(failed(e)),
        };
        let written = file
            .write_all(text.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        if let Err(e) = written {
            take_back(&info_path)?;
            return Err(failed(e));
        }
        if let Err(e) = (ops.sync)(&info) {
            take_back(&info_path)?;
            return Err(TrashError::Failed(e.to_string()));
        }
        let to = files.join(&name);
        let refused = match (ops.rename)(&source.path, &to) {
            Ok(()) => return durable(source, &to, &files, &info_path, ops),
            // A file in `files/` without its `.trashinfo`: another name.
            Err(FsError::AlreadyExists) => None,
            Err(FsError::CrossDevice | FsError::Unsupported) => Some(TrashError::Unsupported),
            Err(e) => Some(TrashError::Failed(e.to_string())),
        };
        take_back(&info_path)?;
        if let Some(refused) = refused {
            return Err(refused);
        }
    }
    Err(TrashError::Failed(
        "the Trash has too many files of that name".into(),
    ))
}

/// The receipt for the file just renamed to `to`, once `files` is synced;
/// if it can't be, the file is put back and the `.trashinfo` taken back.
fn durable(
    source: &Source,
    to: &Path,
    files: &Path,
    info: &Path,
    ops: &Fs,
) -> Result<TrashReceipt, TrashError> {
    let Err(e) = (ops.sync)(files) else {
        return Ok(TrashReceipt {
            location: to.to_path_buf(),
        });
    };
    match (ops.rename)(to, &source.path) {
        Ok(()) => {
            take_back(info)?;
            Err(TrashError::Failed(format!(
                "the Trash couldn't be made durable: {e}"
            )))
        }
        Err(back) => Err(TrashError::Failed(format!(
            "the Trash couldn't be made durable ({e}), and {} stays in it at {}: {back}",
            source.path.display(),
            to.display()
        ))),
    }
}

/// Removes the `.trashinfo` written for a file that didn't go.
fn take_back(info: &Path) -> Result<(), TrashError> {
    fs::remove_file(info)
        .map_err(|e| TrashError::Failed(format!("{} could not be removed: {e}", info.display())))
}

fn failed(e: io::Error) -> TrashError {
    TrashError::Failed(e.to_string())
}

/// The highest folder above `folder` (resolved) still on `device`: the top
/// of its mount. `None` when `folder` isn't on it.
pub(super) fn mount_top(folder: &Path, device: u64) -> Option<PathBuf> {
    let on = |dir: &Path| fs::metadata(dir).is_ok_and(|m| m.dev() == device);
    if !on(folder) {
        return None;
    }
    let mut top = folder;
    while let Some(parent) = top.parent().filter(|p| on(p)) {
        top = parent;
    }
    Some(top.to_path_buf())
}

/// `seconds` since 1970 as a `.trashinfo`'s DeletionDate,
/// `YYYY-MM-DDThh:mm:ss`. In UTC: the specification asks for local time,
/// which the standard library can't tell, so a file manager may show it
/// off by the time zone.
pub(super) fn deletion_date(seconds: i64) -> String {
    let [y, mo, d, h, mi, s] = crate::time::utc(seconds);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:{s:02}")
}

/// `path`'s bytes with every one but unreserved ones and `/`
/// percent-encoded, as the `.trashinfo` Path key keeps them.
fn encoded(path: &Path) -> String {
    let mut text = String::new();
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || b"-._~/".contains(&b) {
            text.push(char::from(b));
        } else {
            text.push_str(&format!("%{b:02X}"));
        }
    }
    text
}

/// The `n`th name tried for `name` in the Trash: `name`, then `a.2.jpg`,
/// `a.3.jpg`…
fn candidate(name: &OsStr, n: u32) -> OsString {
    if n == 1 {
        return name.to_owned();
    }
    let path = Path::new(name);
    let mut out = path.file_stem().unwrap_or(name).to_owned();
    out.push(format!(".{n}"));
    if let Some(extension) = path.extension() {
        out.push(".");
        out.push(extension);
    }
    out
}

#[cfg(test)]
#[path = "freedesktop_tests.rs"]
mod tests;
