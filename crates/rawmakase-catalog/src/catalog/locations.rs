//! Where each computer finds the catalog's folders, so one catalog can move
//! between computers that mount the same photos at different paths (#187).
//!
//! A folder is its root and a logical path (`folder_paths`): names joined by
//! '/', whatever system added it. Each computer keeps its own locations in
//! `folder_locations`, one for a root ('') or for a folder and everything
//! below it; the most specific one wins, and without one a folder is where
//! its root was added. The legacy `roots.mapped_path` and `folder_mappings`
//! are copied into a computer's rows the first time it opens the catalog
//! with this release; after that they are only written, for older releases.
use super::db::{Db, Reads, Write, sql};
use super::value::row;
use super::{Catalog, FolderId, RootId};
use anyhow::{Context, Result, ensure};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A computer that opens catalogs: a random id made once per install, kept
/// in its own data folder, and a name to show.
#[derive(Clone, Debug)]
pub struct Computer {
    pub id: String,
    pub name: String,
}
impl Computer {
    /// This computer.
    pub fn this() -> Computer {
        // Tests never write to the data folder of the computer they run on.
        if cfg!(test) {
            return Computer {
                id: "test".into(),
                name: "Test".into(),
            };
        }
        Self::load_from(&crate::storage::local_data_dir()).unwrap_or_else(|_| {
            // Without a writable data folder, the name is the most stable id left.
            let name = host_name();
            Computer {
                id: format!("host:{name}"),
                name,
            }
        })
    }
    /// The computer whose id is kept in `dir`, made there the first time.
    pub fn load_from(dir: &Path) -> Result<Computer> {
        let file = dir.join(id_file());
        let read = |file: &Path| -> Option<String> {
            let id = std::fs::read_to_string(file).ok()?.trim().to_string();
            (!id.is_empty()).then_some(id)
        };
        let id = match read(&file) {
            Some(id) => id,
            None => {
                std::fs::create_dir_all(dir)?;
                let mut bytes = [0u8; 16];
                getrandom::fill(&mut bytes).map_err(|e| anyhow::anyhow!("{e}"))?;
                let id: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                let temp = tempfile::NamedTempFile::new_in(dir)?;
                std::fs::write(temp.path(), &id)?;
                // Another process may have made one meanwhile: theirs stays.
                match crate::storage::persist_noclobber(temp, &file) {
                    Ok(_) => id,
                    Err(_) => read(&file).context("Unreadable computer id")?,
                }
            }
        };
        Ok(Computer {
            id,
            name: host_name(),
        })
    }
}
/// The name of the file holding the computer id. On Linux the data folder
/// can be in a home directory several computers share (over NFS, or a synced
/// XDG_DATA_HOME), so each machine, as /etc/machine-id (else its host
/// name) tells them apart, keeps its own.
fn id_file() -> String {
    let machine = cfg!(target_os = "linux")
        .then(|| std::fs::read_to_string("/etc/machine-id").ok())
        .flatten()
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()));
    // Without one, the host name still tells machines sharing a home apart.
    let host = || {
        let name: String = host_name()
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        (!name.is_empty()).then_some(name)
    };
    match machine {
        Some(machine) => format!("computer-id-{machine}"),
        None if cfg!(target_os = "linux") => match host() {
            Some(host) => format!("computer-id-host-{host}"),
            None => "computer-id".into(),
        },
        None => "computer-id".into(),
    }
}
/// The computer's name as its system shows it, for labels only.
fn host_name() -> String {
    let run = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    std::env::var("COMPUTERNAME")
        .ok()
        .or_else(|| {
            cfg!(target_os = "macos")
                .then(|| run("scutil", &["--get", "ComputerName"]))
                .flatten()
        })
        .or_else(|| {
            std::fs::read_to_string("/etc/hostname")
                .ok()
                .map(|s| s.trim().to_string())
        })
        .or_else(|| run("hostname", &[]))
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "This computer".into())
}

/// Whether to keep a root's folder locations when the root moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overrides {
    Keep,
    Clear,
}
/// A folder (and its subfolders) located apart from its root on this computer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Override {
    /// Its logical path in the root.
    pub relative: String,
    pub path: PathBuf,
}
/// A root, where it is on this computer and elsewhere.
#[derive(Clone, Debug)]
pub struct RootLocations {
    pub root: RootId,
    /// Where it was added.
    pub original: String,
    /// This computer's location of the root, if it has one.
    pub location: Option<PathBuf>,
    /// Where the root is on this computer: its location, else where it was added.
    pub path: PathBuf,
    /// Folders below it located separately on this computer.
    pub overrides: Vec<Override>,
    /// Other computers' locations: computer name, logical path, path.
    pub elsewhere: Vec<(String, String, PathBuf)>,
}
/// A place on this computer where a root ('') or folder is.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FolderLocation {
    pub root: RootId,
    pub relative: String,
    pub path: PathBuf,
}

/// Folder `folder`'s root and logical path, if it is in the catalog.
fn logical_path(db: &impl Reads, folder: FolderId) -> Result<Option<(RootId, String)>> {
    row! {
        struct Logical {
            root: RootId,
            original: String,
            relative: String,
            logical: Option<String>,
        }
    }
    let f: Option<Logical> = db.read_optional(
        sql!(
            "SELECT f.root, r.original_path, f.relative_path, p.path
             FROM folders f JOIN roots r ON r.id=f.root
             LEFT JOIN folder_paths p ON p.folder=f.id WHERE f.id=?"
        ),
        &[&folder],
    )?;
    Ok(f.map(|f| {
        (
            f.root,
            f.logical
                .unwrap_or_else(|| logical_from_legacy(&f.original, &f.relative)),
        )
    }))
}
/// Where folder `folder` is on `computer`, as [`Catalog::folders`] places
/// it, read in `db`'s transaction: `None` when it isn't in the catalog or
/// can't be there.
pub(super) fn folder_path(
    db: &impl Reads,
    computer: &str,
    folder: FolderId,
) -> Result<Option<PathBuf>> {
    let Some((root, logical)) = logical_path(db, folder)? else {
        return Ok(None);
    };
    logical_location(db, computer, root, &logical)
}
/// Where logical folder `logical` of `root` is on `computer`, read in
/// `db`'s transaction, whether or not the catalog has that folder yet.
pub(super) fn logical_location(
    db: &impl Reads,
    computer: &str,
    root: RootId,
    logical: &str,
) -> Result<Option<PathBuf>> {
    Ok(logical_anchored(db, computer, root, logical)?.map(|(_, path)| path))
}
/// [`logical_location`], with the location it resolves below (see
/// [`resolve_anchored`]).
pub(super) fn logical_anchored(
    db: &impl Reads,
    computer: &str,
    root: RootId,
    logical: &str,
) -> Result<Option<(PathBuf, PathBuf)>> {
    let original: Option<String> =
        db.read_optional(sql!("SELECT original_path FROM roots WHERE id=?"), &[&root])?;
    let Some(original) = original else {
        return Ok(None);
    };
    let rows: Vec<Location> = db.read(
        sql!(
            "SELECT root, relative_path, path FROM folder_locations
             WHERE computer=? AND root=? ORDER BY relative_path"
        ),
        &[&computer, &root],
    )?;
    let rows: Vec<(String, PathBuf)> = rows
        .into_iter()
        .map(|l| (l.relative, PathBuf::from(l.path)))
        .collect();
    Ok(resolve_anchored(&original, &rows, logical, cfg!(windows))
        .filter(|(_, path)| !path.as_os_str().is_empty()))
}
/// Forgets where folder `folder` is on every computer, as it leaves the
/// catalog: a location left behind would refuse the folder added again as
/// being elsewhere. A root's own location goes with the root.
pub(super) fn forget_folder_locations(w: &mut Write<'_>, folder: FolderId) -> Result<()> {
    if let Some((root, logical)) = logical_path(w, folder)?
        && !logical.is_empty()
    {
        w.execute(
            sql!("DELETE FROM folder_locations WHERE root=? AND relative_path=?"),
            &[&root, &logical],
        )?;
    }
    Ok(())
}
/// The names of a logical path.
pub(super) fn names(logical: &str) -> impl Iterator<Item = &str> {
    logical.split('/').filter(|n| !n.is_empty())
}
/// Whether `path` was written on Windows: a drive letter or a UNC share.
fn is_windows_path(path: &str) -> bool {
    let b = path.as_bytes();
    path.starts_with(r"\\")
        || (b.len() >= 3
            && b[0].is_ascii_alphabetic()
            && b[1] == b':'
            && matches!(b[2], b'\\' | b'/'))
}
/// The logical path of a folder an older release stored as `relative` in a
/// root added at `original`: '\' separates names only in a Windows root,
/// since elsewhere it can be part of one.
pub(super) fn logical_from_legacy(original: &str, relative: &str) -> String {
    let windows = is_windows_path(original);
    relative
        .split(|c| c == '/' || (windows && c == '\\'))
        .filter(|n| !n.is_empty() && *n != ".")
        .collect::<Vec<_>>()
        .join("/")
}
/// The logical path of a folder at `relative` below a location on this system.
pub(super) fn logical_from_os(relative: &Path) -> String {
    relative
        .components()
        .filter_map(|c| match c {
            std::path::Component::Normal(name) => Some(name.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}
/// Joins two logical paths.
pub(super) fn join(parent: &str, child: &str) -> String {
    names(parent)
        .chain(names(child))
        .collect::<Vec<_>>()
        .join("/")
}
/// Where logical folder `logical` of a root added at `original` is, given
/// that root's `rows` (logical path, location): below the most specific row
/// containing it, else below `original`. `None` when one of its names can't
/// be a file name on this system (a Unix name with a '\' on Windows): it is
/// unavailable here, never read as more folders.
pub fn resolve_in(
    original: &str,
    rows: &[(String, PathBuf)],
    logical: &str,
    windows: bool,
) -> Option<PathBuf> {
    resolve_anchored(original, rows, logical, windows).map(|(_, path)| path)
}
/// [`resolve_in`], with the location it resolves below: the most specific
/// row containing it, else `original`. A folder whose anchor is there while
/// the folder isn't is missing from it, not offline.
pub(super) fn resolve_anchored(
    original: &str,
    rows: &[(String, PathBuf)],
    logical: &str,
    windows: bool,
) -> Option<(PathBuf, PathBuf)> {
    let parts: Vec<&str> = names(logical).collect();
    if windows
        && parts
            .iter()
            .any(|n| n.contains(['\\', '<', '>', ':', '"', '|', '?', '*']))
    {
        return None;
    }
    let mut best: Option<(usize, &PathBuf)> = None;
    for (relative, path) in rows {
        let row: Vec<&str> = names(relative).collect();
        if row.len() <= parts.len()
            && parts[..row.len()] == row[..]
            && best.is_none_or(|(n, _)| row.len() > n)
        {
            best = Some((row.len(), path));
        }
    }
    let (skip, anchor) = match best {
        Some((n, path)) => (n, path.clone()),
        None => (0, PathBuf::from(original)),
    };
    let mut path = anchor.clone();
    for name in &parts[skip..] {
        path.push(name);
    }
    Some((anchor, path))
}

/// The folders an older release added since, which have no logical path
/// yet, joined with their roots.
macro_rules! unmapped {
    () => {
        "FROM folders f JOIN roots r ON r.id=f.root
         LEFT JOIN folder_paths p ON p.folder=f.id WHERE p.folder IS NULL"
    };
}

/// Readies a catalog for `computer`, the only writing an open does, in one
/// transaction: registers the computer, gives folders an older release added
/// their logical path, and once per computer adopts the legacy mappings.
pub(super) fn prepare(db: &mut Db, computer: &Computer) -> Result<()> {
    // Most opens have nothing to write; a write would wait for other computers.
    if is_adopted(db, computer)? && !has_unmapped(db)? {
        return Ok(());
    }
    db.write_immediate(|w| {
        w.execute(
            sql!("INSERT INTO computers(id, name) VALUES (?, ?) ON CONFLICT DO NOTHING"),
            &[&computer.id, &computer.name],
        )?;
        row! {
            struct Legacy {
                folder: FolderId,
                original: String,
                relative: String,
            }
        }
        let legacy: Vec<Legacy> = w.read(
            sql!(concat!(
                "SELECT f.id, r.original_path, f.relative_path ",
                unmapped!()
            )),
            &[],
        )?;
        for l in legacy {
            w.execute(
                sql!("INSERT INTO folder_paths(folder, path) VALUES (?, ?)"),
                &[&l.folder, &logical_from_legacy(&l.original, &l.relative)],
            )?;
        }
        if !is_adopted(w, computer)? {
            // Everything the legacy mapping says, the root and every folder, so
            // a later change of one folder leaves the others where they were.
            w.execute(
                sql!(
                    "INSERT INTO folder_locations(root, relative_path, computer, path)
                     SELECT id, '', ?, mapped_path FROM roots WHERE mapped_path IS NOT NULL
                     ON CONFLICT DO NOTHING"
                ),
                &[&computer.id],
            )?;
            // A folder's mapping wins over its root's, as in older releases.
            // Folders an older release added twice share a logical path: the
            // last one added wins.
            w.execute(
                sql!(
                    "INSERT INTO folder_locations(root, relative_path, computer, path)
                     SELECT f.root, p.path, ?, m.path FROM folder_mappings m
                     JOIN folders f ON f.id=m.folder JOIN folder_paths p ON p.folder=f.id
                     WHERE f.id=(SELECT MAX(f2.id) FROM folder_mappings m2
                         JOIN folders f2 ON f2.id=m2.folder JOIN folder_paths p2 ON p2.folder=f2.id
                         WHERE f2.root=f.root AND p2.path=p.path)
                     ON CONFLICT(root, relative_path, computer) DO UPDATE SET path=excluded.path"
                ),
                &[&computer.id],
            )?;
            w.execute(
                sql!("UPDATE computers SET adopted_at=? WHERE id=?"),
                &[&rawmakase_model::time::now_text(), &computer.id],
            )?;
        }
        Ok(())
    })
}

/// Whether `computer` has adopted the legacy mappings.
fn is_adopted(db: &impl Reads, computer: &Computer) -> Result<bool> {
    Ok(db
        .read_optional(
            sql!("SELECT adopted_at IS NOT NULL FROM computers WHERE id=?"),
            &[&computer.id],
        )?
        .unwrap_or(false))
}

/// Whether an older release added a folder that has no logical path yet.
fn has_unmapped(db: &impl Reads) -> Result<bool> {
    db.read_one(
        sql!(concat!("SELECT EXISTS(SELECT 1 ", unmapped!(), ")")),
        &[],
    )
}

row! {
    /// One of this computer's locations.
    struct Location {
        root: RootId,
        relative: String,
        path: String,
    }
}

impl Catalog {
    /// This computer's locations, by root: (logical path, location).
    pub(super) fn location_rows(&self) -> Result<HashMap<RootId, Vec<(String, PathBuf)>>> {
        let mut rows: HashMap<RootId, Vec<(String, PathBuf)>> = HashMap::new();
        let locations: Vec<Location> = self.db.read(
            sql!(
                "SELECT root, relative_path, path FROM folder_locations WHERE computer=?
                 ORDER BY root, relative_path"
            ),
            &[&self.computer.id],
        )?;
        for l in locations {
            rows.entry(l.root)
                .or_default()
                .push((l.relative, PathBuf::from(l.path)));
        }
        Ok(rows)
    }
    /// Finds root `id` at `path` on this computer, keeping its folders'
    /// own locations.
    pub fn relink_root(&mut self, id: RootId, path: &Path) -> Result<()> {
        self.set_root_location(id, path, Overrides::Keep)
    }
    /// Finds root `id` at `path` on this computer, keeping or clearing the
    /// locations of folders below it.
    pub fn relink_root_with(
        &mut self,
        id: RootId,
        path: &Path,
        overrides: Overrides,
    ) -> Result<()> {
        self.set_root_location(id, path, overrides)
    }
    fn set_root_location(&mut self, id: RootId, path: &Path, overrides: Overrides) -> Result<()> {
        ensure!(path.is_dir(), "Choose an existing folder");
        let path = path.to_string_lossy();
        let computer = &self.computer.id;
        self.db.write(|w| {
            // Older releases read the last change made anywhere.
            ensure!(
                w.execute(
                    sql!("UPDATE roots SET mapped_path=? WHERE id=?"),
                    &[&path, &id]
                )? == 1,
                "Unknown root"
            );
            w.execute(
                sql!(
                    "INSERT INTO folder_locations(root, relative_path, computer, path) VALUES (?, '', ?, ?)
                     ON CONFLICT(root, relative_path, computer) DO UPDATE SET path=excluded.path"
                ),
                &[&id, computer, &path],
            )?;
            if overrides == Overrides::Clear {
                let below: Vec<String> = w.read(
                    sql!(
                        "SELECT relative_path FROM folder_locations
                         WHERE root=? AND computer=? AND relative_path<>''"
                    ),
                    &[&id, computer],
                )?;
                for relative in below {
                    clear(w, computer, id, &relative)?;
                }
            }
            Ok(())
        })
    }
    #[cfg(any(test, feature = "test-support"))]
    /// Finds folder `id` and its subfolders at `path` on this computer,
    /// keeping the locations of folders below it.
    pub fn relink_folder(&mut self, id: FolderId, path: &Path) -> Result<()> {
        let (root, logical) = logical_path(&self.db, id)?.context("Unknown folder")?;
        self.relink_folder_at(root, &logical, path, Overrides::Keep)
    }
    /// Finds the folder of `root` at logical path `relative` and everything
    /// below it at `path` on this computer, keeping or clearing the locations
    /// of folders below it. The folder needs no catalog folder of its own: a
    /// folder holding only folders is located by its path.
    pub fn relink_folder_at(
        &mut self,
        root: RootId,
        relative: &str,
        path: &Path,
        overrides: Overrides,
    ) -> Result<()> {
        ensure!(path.is_dir(), "Choose an existing folder");
        let computer = &self.computer.id;
        self.db.write(|w| {
            let folders = logical_folders(w, root)?;
            ensure!(
                folders.iter().any(|(_, logical)| within(relative, logical)),
                "Unknown folder"
            );
            if overrides == Overrides::Clear {
                for below in own_rows(w, computer, root)? {
                    if below != relative && within(relative, &below) {
                        clear(w, computer, root, &below)?;
                    }
                }
            }
            w.execute(
                sql!(
                    "INSERT INTO folder_locations(root, relative_path, computer, path) VALUES (?, ?, ?, ?)
                     ON CONFLICT(root, relative_path, computer) DO UPDATE SET path=excluded.path"
                ),
                &[&root, &relative, computer, &path.to_string_lossy()],
            )?;
            // Older releases read a catalog folder's own mapping, for the
            // folders below it too. Folders below a folder with none of its
            // own get none: a mapping there would go stale as folders come
            // and go, and stop them being renamed or moved.
            for (id, _) in folders.iter().filter(|(_, logical)| logical == relative) {
                w.execute(
                    sql!(
                        "INSERT INTO folder_mappings(folder, path) VALUES (?, ?)
                         ON CONFLICT(folder) DO UPDATE SET path=excluded.path"
                    ),
                    &[id, &path.to_string_lossy()],
                )?;
            }
            Ok(())
        })
    }
    /// Forgets this computer's location of root `root` ('') or of its folder
    /// `relative`: it is then found through the nearest location above it,
    /// else where its root was added. Folders below keep their own.
    pub fn clear_folder_location(&mut self, root: RootId, relative: &str) -> Result<()> {
        let computer = &self.computer.id;
        self.db.write(|w| clear(w, computer, root, relative))
    }
    /// The folders of every root located separately on this computer, by
    /// root.
    pub fn overrides(&self) -> Result<HashMap<RootId, Vec<Override>>> {
        Ok(self
            .location_rows()?
            .into_iter()
            .map(|(root, rows)| {
                let overrides = rows
                    .into_iter()
                    .filter(|(relative, _)| !relative.is_empty())
                    .map(|(relative, path)| Override { relative, path })
                    .collect();
                (root, overrides)
            })
            .collect())
    }
    /// The folders below root `root` located separately on this computer.
    pub fn root_overrides(&self, root: RootId) -> Result<Vec<Override>> {
        self.folder_overrides(root, "")
    }
    /// The folders below the folder of `root` at logical path `relative`
    /// (the root for '') located separately on this computer.
    pub fn folder_overrides(&self, root: RootId, relative: &str) -> Result<Vec<Override>> {
        Ok(self
            .location_rows()?
            .remove(&root)
            .unwrap_or_default()
            .into_iter()
            .filter(|(below, _)| below != relative && within(relative, below))
            .map(|(relative, path)| Override { relative, path })
            .collect())
    }
    /// Every root with its locations, for Folder locations.
    pub fn folder_locations(&self) -> Result<Vec<RootLocations>> {
        row! {
            struct Elsewhere {
                root: RootId,
                computer: String,
                relative: String,
                path: String,
            }
        }
        let mut rows = self.location_rows()?;
        let mut elsewhere: HashMap<RootId, Vec<(String, String, PathBuf)>> = HashMap::new();
        let others: Vec<Elsewhere> = self.db.read(
            sql!(
                "SELECT l.root, c.name, l.relative_path, l.path FROM folder_locations l
                 JOIN computers c ON c.id=l.computer WHERE l.computer<>?
                 ORDER BY c.name, l.relative_path"
            ),
            &[&self.computer.id],
        )?;
        for e in others {
            elsewhere.entry(e.root).or_default().push((
                e.computer,
                e.relative,
                PathBuf::from(e.path),
            ));
        }
        Ok(self
            .roots()?
            .into_iter()
            .map(|(root, original, location)| {
                let own = rows.remove(&root).unwrap_or_default();
                let location = location.map(PathBuf::from);
                RootLocations {
                    root,
                    path: location.clone().unwrap_or_else(|| PathBuf::from(&original)),
                    original,
                    location,
                    overrides: own
                        .into_iter()
                        .filter(|(relative, _)| !relative.is_empty())
                        .map(|(relative, path)| Override { relative, path })
                        .collect(),
                    elsewhere: elsewhere.remove(&root).unwrap_or_default(),
                }
            })
            .collect())
    }
    /// Renames this computer; its locations stay.
    pub fn rename_computer(&mut self, name: &str) -> Result<()> {
        let name = name.trim();
        ensure!(!name.is_empty(), "Name this computer");
        let computer = &self.computer.id;
        self.db.write(|w| {
            w.execute(
                sql!("UPDATE computers SET name=? WHERE id=?"),
                &[&name, computer],
            )
        })?;
        self.computer.name = name.into();
        Ok(())
    }
    /// This computer's name as the catalog has it.
    pub fn computer_name(&self) -> Result<String> {
        Ok(self
            .db
            .read_optional(
                sql!("SELECT name FROM computers WHERE id=?"),
                &[&self.computer.id],
            )?
            .unwrap_or_else(|| self.computer.name.clone()))
    }
}
/// Clears `computer`'s location of `relative` in `root` and, in the same
/// transaction, the legacy mapping older releases read for it.
fn clear(w: &mut Write<'_>, computer: &str, root: RootId, relative: &str) -> Result<()> {
    w.execute(
        sql!("DELETE FROM folder_locations WHERE root=? AND relative_path=? AND computer=?"),
        &[&root, &relative, &computer],
    )?;
    if relative.is_empty() {
        w.execute(
            sql!("UPDATE roots SET mapped_path=NULL WHERE id=?"),
            &[&root],
        )?;
    } else {
        w.execute(
            sql!(
                "DELETE FROM folder_mappings WHERE folder IN
                 (SELECT f.id FROM folders f JOIN folder_paths p ON p.folder=f.id
                  WHERE f.root=? AND p.path=?)"
            ),
            &[&root, &relative],
        )?;
    }
    Ok(())
}
/// Whether logical path `logical` is `parent` or a folder below it.
fn within(parent: &str, logical: &str) -> bool {
    let mut below = names(logical);
    names(parent).all(|n| below.next() == Some(n))
}
/// The catalog folders of `root`, with their logical paths.
fn logical_folders(db: &impl Reads, root: RootId) -> Result<Vec<(FolderId, String)>> {
    row! {
        struct Logical {
            id: FolderId,
            logical: String,
        }
    }
    let rows: Vec<Logical> = db.read(
        sql!(
            "SELECT f.id, p.path FROM folders f JOIN folder_paths p ON p.folder=f.id
             WHERE f.root=?"
        ),
        &[&root],
    )?;
    Ok(rows.into_iter().map(|r| (r.id, r.logical)).collect())
}
/// The logical paths `computer` has a location of its own for in `root`.
fn own_rows(db: &impl Reads, computer: &str, root: RootId) -> Result<Vec<String>> {
    db.read(
        sql!("SELECT relative_path FROM folder_locations WHERE root=? AND computer=?"),
        &[&root, &computer],
    )
}

/// The folders of `db`, each with its root and logical path.
fn every_logical(db: &impl Reads) -> Result<Vec<(FolderId, RootId, String)>> {
    let ids: Vec<FolderId> = db.read(sql!("SELECT id FROM folders ORDER BY id"), &[])?;
    let mut folders = Vec::new();
    for id in ids {
        if let Some((root, logical)) = logical_path(db, id)? {
            folders.push((id, root, logical));
        }
    }
    Ok(folders)
}
/// `kept` with every catalog folder above one of them in its root: removing
/// one would take the locations below it with it, which `kept` resolve by.
pub(super) fn with_ancestors(
    db: &impl Reads,
    kept: &std::collections::BTreeSet<FolderId>,
) -> Result<std::collections::BTreeSet<FolderId>> {
    let folders = every_logical(db)?;
    let places: Vec<(RootId, &str)> = folders
        .iter()
        .filter(|(id, ..)| kept.contains(id))
        .map(|(_, root, logical)| (*root, logical.as_str()))
        .collect();
    Ok(folders
        .iter()
        .filter(|(id, root, logical)| {
            kept.contains(id)
                || places
                    .iter()
                    .any(|(r, place)| r == root && within(logical, place))
        })
        .map(|(id, ..)| *id)
        .collect())
}
/// Deletes every location no folder left is at or below, on every computer,
/// and the roots no folder is left in: what is left places only the folders
/// left, and names nothing else. Run once the other folders are removed.
pub(super) fn forget_unused(w: &mut Write<'_>) -> Result<()> {
    row! {
        struct Any {
            root: RootId,
            relative: String,
        }
    }
    let folders = every_logical(w)?;
    let rows: Vec<Any> = w.read(
        sql!("SELECT DISTINCT root, relative_path FROM folder_locations"),
        &[],
    )?;
    for row in rows {
        let used = folders
            .iter()
            .any(|(_, root, logical)| *root == row.root && within(&row.relative, logical));
        if !used {
            w.execute(
                sql!("DELETE FROM folder_locations WHERE root=? AND relative_path=?"),
                &[&row.root, &row.relative],
            )?;
        }
    }
    w.execute(
        sql!(
            "DELETE FROM folder_locations
             WHERE root NOT IN (SELECT root FROM folders)"
        ),
        &[],
    )?;
    w.execute(
        sql!("DELETE FROM roots WHERE id NOT IN (SELECT root FROM folders)"),
        &[],
    )?;
    Ok(())
}
/// Where `folders` are, as the catalog records it for every computer: their
/// rows and logical paths, their roots, and every location in those roots.
/// Equal only while nothing moved or relinked any of them.
pub(super) fn placement_of(db: &impl Reads, folders: &[FolderId]) -> Result<Vec<String>> {
    row! {
        struct Placed {
            root: RootId,
            original: String,
            mapped: Option<String>,
            relative: String,
            logical: Option<String>,
            legacy: Option<String>,
        }
    }
    row! {
        struct Located {
            relative: String,
            computer: String,
            path: String,
        }
    }
    let mut placement = Vec::new();
    let mut roots = std::collections::BTreeSet::new();
    for folder in folders {
        let placed: Option<Placed> = db.read_optional(
            sql!(
                "SELECT f.root, r.original_path, r.mapped_path, f.relative_path, p.path, m.path
                 FROM folders f JOIN roots r ON r.id=f.root
                 LEFT JOIN folder_paths p ON p.folder=f.id
                 LEFT JOIN folder_mappings m ON m.folder=f.id WHERE f.id=?"
            ),
            &[folder],
        )?;
        placement.push(format!(
            "{folder:?} {:?}",
            placed.as_ref().map(|p| (
                p.root,
                &p.original,
                &p.mapped,
                &p.relative,
                &p.logical,
                &p.legacy,
            ))
        ));
        roots.extend(placed.map(|p| p.root));
    }
    for root in roots {
        let rows: Vec<Located> = db.read(
            sql!(
                "SELECT relative_path, computer, path FROM folder_locations
                 WHERE root=? ORDER BY relative_path, computer"
            ),
            &[&root],
        )?;
        for l in rows {
            placement.push(format!("{root:?} {:?}", (l.relative, l.computer, l.path)));
        }
    }
    Ok(placement)
}
