//! What the file operation tests share: a catalog over synthetic files in a
//! temporary folder, a Trash that keeps files in a folder of its own, and a
//! disk that fails or "crashes" (panics, as a process ending would leave
//! things) at a chosen call. Never a real catalog or real photos.
use super::disk::{Disk, Real, parent};
use super::*;
use crate::catalog::Catalog;
use crate::fsops::{Entry, FsError};
use crate::storage::Identity;
use anyhow::Result;
use rawmakase_cancel::Cancel;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Distinct synthetic bytes for a file, from its name.
pub(super) fn bytes(name: &str) -> Vec<u8> {
    let seed = name.bytes().fold(7u8, |a, b| a.wrapping_mul(31) ^ b);
    let mut out = name.as_bytes().to_vec();
    out.extend((0..2000u32).map(|i| (i as u8).wrapping_mul(seed) ^ seed));
    out
}

/// A catalog over one root folder, "trip", of synthetic files.
pub(super) struct Fx {
    pub dir: tempfile::TempDir,
    /// The root, canonical.
    pub root: PathBuf,
    pub catalog: PathBuf,
    pub cat: Catalog,
}

/// A catalog of `trip/` holding `files` (paths below it). A name ending in
/// `/` is a folder, given a photo `keep.dng` so the catalog has it.
pub(super) fn fixture(files: &[&str]) -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap().join("trip");
    std::fs::create_dir_all(&root).unwrap();
    for f in files {
        let path = root.join(f);
        if f.ends_with('/') {
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join("keep.dng"), bytes(&format!("{f}keep.dng"))).unwrap();
        } else {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, bytes(f)).unwrap();
        }
    }
    let catalog = dir.path().join("Photos.rawmakase");
    let mut cat = Catalog::create(&catalog).unwrap();
    cat.add_folder(&root).unwrap();
    Fx {
        dir,
        root,
        catalog,
        cat,
    }
}

impl Fx {
    /// The master named `name` (its path below the root).
    pub(super) fn photo(&self, name: &str) -> PhotoId {
        let path = self.root.join(name);
        self.cat
            .photos()
            .unwrap()
            .into_iter()
            .find(|p| p.master.is_none() && p.path == path)
            .unwrap_or_else(|| panic!("no photo {name}"))
            .id
    }
    /// The root, as the catalog has it: the one added at `root`, whatever
    /// others a test adds.
    pub(super) fn root_id(&self) -> crate::catalog::RootId {
        self.cat
            .roots()
            .unwrap()
            .into_iter()
            .find(|(_, original, _)| Path::new(original) == self.root)
            .expect("the fixture's root")
            .0
    }
    /// The root of catalog folder `folder`.
    pub(super) fn root_of(&self, folder: FolderId) -> crate::catalog::RootId {
        self.place(folder).0
    }
    /// The logical path of catalog folder `folder` in its root.
    pub(super) fn logical(&self, folder: FolderId) -> String {
        self.place(folder).1
    }
    fn place(&self, folder: FolderId) -> (crate::catalog::RootId, String) {
        self.cat
            .db_for_tests()
            .query_row(
                "SELECT f.root, p.path FROM folders f JOIN folder_paths p ON p.folder=f.id
                 WHERE f.id=?",
                [folder.0],
                |r| Ok((crate::catalog::RootId(r.get(0)?), r.get(1)?)),
            )
            .unwrap()
    }
    /// The catalog folder at `relative` below the root ("" is the root).
    pub(super) fn folder(&self, relative: &str) -> FolderId {
        let path = if relative.is_empty() {
            self.root.clone()
        } else {
            self.root.join(relative)
        };
        self.cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no folder {relative}"))
            .id
    }
    /// The catalog folder at `path`, of whichever root.
    pub(super) fn folder_at(&self, path: &Path) -> FolderId {
        self.cat
            .folders()
            .unwrap()
            .into_iter()
            .find(|f| f.path == path)
            .unwrap_or_else(|| panic!("no folder {}", path.display()))
            .id
    }
    /// Every file below the folder the fixture is in, by path relative to
    /// it, with its bytes: where every original is.
    pub(super) fn files(&self) -> BTreeMap<String, Vec<u8>> {
        let mut out = BTreeMap::new();
        let base = self.dir.path().canonicalize().unwrap();
        fn walk(dir: &Path, base: &Path, out: &mut BTreeMap<String, Vec<u8>>) {
            for e in std::fs::read_dir(dir).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else {
                    let name = p.strip_prefix(base).unwrap().to_string_lossy().into_owned();
                    // AppleDouble files the test disk keeps beside others
                    // on FAT and exFAT are part of those.
                    let apple_double = p
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("._"));
                    if !name.starts_with("Photos.rawmakase") && !apple_double {
                        out.insert(name.replace('\\', "/"), std::fs::read(&p).unwrap());
                    }
                }
            }
        }
        walk(&base, &base, &mut out);
        out
    }
    /// Where each photo row says its file is, relative to the root.
    pub(super) fn rows(&self) -> BTreeMap<PhotoId, String> {
        self.cat
            .photos()
            .unwrap()
            .into_iter()
            .map(|p| {
                let rel = p.path.strip_prefix(&self.root).unwrap_or(&p.path);
                (p.id, rel.to_string_lossy().replace('\\', "/"))
            })
            .collect()
    }
    /// A photo row `filename` in folder `logical` of the root added at
    /// `root` (a new root when there is none), written as an older release
    /// or another tool might have: no checks, no file read.
    pub(super) fn add_row(&mut self, root: &Path, logical: &str, filename: &str) -> PhotoId {
        use crate::catalog::db::{Reads, sql};
        let original = root.to_str().unwrap().to_string();
        let path = root.join(logical).join(filename);
        let path = path.to_str().unwrap().to_string();
        self.cat
            .db
            .write(|w| {
                let found: Option<crate::catalog::RootId> = w.read_optional(
                    sql!("SELECT id FROM roots WHERE original_path=?"),
                    &[&original],
                )?;
                let root = match found {
                    Some(root) => root,
                    None => w.insert_returning_id(
                        sql!("INSERT INTO roots(original_path) VALUES (?) RETURNING id"),
                        &[&original],
                    )?,
                };
                let folder = crate::catalog::ingest::folder_id(w, root, logical)?;
                w.insert_returning_id(
                    sql!(
                        "INSERT INTO photos(folder, filename, original_path) VALUES (?, ?, ?)
                         RETURNING id"
                    ),
                    &[&folder, &filename, &path],
                )
            })
            .unwrap()
    }
    /// Opens the catalog again, as a restart does.
    pub(super) fn reopen(&mut self) {
        self.cat = Catalog::open(&self.catalog).unwrap();
    }
    pub(super) fn plan(&self, op: FileOp) -> Plan {
        plan(&self.cat, op).unwrap()
    }
    /// Plans `op` with the volumes and folders as `disk` sees them.
    pub(super) fn plan_on(&self, op: FileOp, disk: &dyn Disk) -> Result<Plan, FileOpError> {
        plan::plan_with(&self.cat, op, disk)
    }
    /// Plans, confirms and executes `op` with `trash` on `disk`.
    pub(super) fn run(&mut self, op: FileOp, trash: &dyn Trash, disk: &dyn Disk) -> Outcome {
        let plan = self.plan(op);
        execute::execute_with(&mut self.cat, plan.confirm(), trash, Cancel::NEVER, disk).unwrap()
    }
    /// Recovers on `disk`.
    pub(super) fn recover(&mut self, disk: &dyn Disk) -> Recovery {
        self.recover_with(disk, &NoTrash)
    }
    /// Recovers on `disk`, with `trash`.
    pub(super) fn recover_with(&mut self, disk: &dyn Disk, trash: &dyn Trash) -> Recovery {
        let _lock = lock::acquire(&self.cat).unwrap();
        recover::run(&mut self.cat, disk, trash).unwrap()
    }
    /// Installs trigger `body` (after its name) for the connection's life.
    pub(super) fn fail_journal_with(&self, body: &str) {
        self.cat
            .db_for_tests()
            .execute_batch(&format!("CREATE TEMP TRIGGER injected {body}"))
            .unwrap();
    }
    /// Installs a trigger making `when` fail, as a full disk would: for
    /// the connection's life, so a reopen clears it.
    pub(super) fn fail_journal(&self, when: &str) {
        self.cat
            .db_for_tests()
            .execute_batch(&format!(
                "CREATE TEMP TRIGGER fail_journal {when} BEGIN SELECT RAISE(FAIL, 'disk full'); END;"
            ))
            .unwrap();
    }
}

/// What the test disk writes as a file's AppleDouble file.
const APPLE_DOUBLE: &[u8] = b"AppleDouble";

/// `path` with each name as it is stored, looked for in any normalisation
/// form, as macOS finds names (and Linux, by bytes, doesn't).
pub(super) fn in_any_form(path: &Path) -> PathBuf {
    use unicode_normalization::UnicodeNormalization;
    if path.symlink_metadata().is_ok() {
        return path.to_path_buf();
    }
    let mut at = PathBuf::new();
    for part in path.components() {
        let exact = at.join(part);
        let name = part.as_os_str().to_string_lossy();
        let nfc: String = name.nfc().collect();
        let nfd: String = name.nfd().collect();
        at = [exact.clone(), at.join(nfc), at.join(nfd)]
            .into_iter()
            .find(|p| p.symlink_metadata().is_ok())
            .unwrap_or(exact);
    }
    at
}

/// Whether `path` names its file in the spelling it is stored in, or names
/// nothing: what macOS's FAT and exFAT drivers ask of a rename or removal.
pub(super) fn stored_spelling(path: &Path) -> bool {
    use unicode_normalization::UnicodeNormalization;
    let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
        return true;
    };
    let wanted: String = name.to_string_lossy().nfc().collect();
    match Real.list(dir) {
        Ok(names) => names
            .iter()
            .filter(|n| n.to_string_lossy().nfc().collect::<String>() == wanted)
            .all(|n| n == name),
        Err(_) => true,
    }
}

/// A test's hook on a path.
pub(super) type PathHook = Option<Box<dyn Fn(&Path)>>;

/// A Trash that keeps files in `<fixture>/trash/`, under a unique name.
pub(super) struct FakeTrash {
    pub dir: PathBuf,
    /// It takes a file only by the spelling its name is stored in, as
    /// macOS's Trash may on FAT and exFAT.
    pub stored_only: bool,
    /// Fails from the `n`-th file on (from 1), if set.
    pub fail_from: Option<usize>,
    /// The process ends right after the `n`-th file went (from 1), if set.
    pub crash_after: Option<usize>,
    /// The process ends just before the `n`-th file would go (from 1), if
    /// set.
    pub crash_before: Option<usize>,
    /// Folders whose files and folders it can't take, as macOS's Trash
    /// can't on an SMB share: it answers `Unsupported`, changing nothing.
    pub refuses: Vec<PathBuf>,
    pub calls: RefCell<usize>,
    /// What it says when asked whether it takes a path.
    pub support: TrashSupport,
    /// Runs on the path just before it is moved: what changes in the
    /// moment between the last check and the Trash.
    pub meanwhile: PathHook,
    /// Runs on where it put the path, right after it moved it.
    pub after: PathHook,
    /// Takes a folder's first file (by name) on its own, then fails, as a
    /// Trash failing part way through a folder may.
    pub partial: bool,
}

impl FakeTrash {
    pub(super) fn new(fx: &Fx) -> Self {
        Self::at(&fx.dir.path().canonicalize().unwrap().join("trash"))
    }
    /// A Trash keeping what it takes in folder `dir`.
    pub(super) fn at(dir: &Path) -> Self {
        std::fs::create_dir_all(dir).unwrap();
        Self {
            dir: dir.to_path_buf(),
            fail_from: None,
            crash_after: None,
            crash_before: None,
            stored_only: false,
            refuses: Vec::new(),
            calls: RefCell::new(0),
            support: TrashSupport::Yes,
            meanwhile: None,
            after: None,
            partial: false,
        }
    }
}

impl Trash for FakeTrash {
    fn trash(&self, path: &Path) -> Result<TrashReceipt, TrashError> {
        let n = {
            let mut calls = self.calls.borrow_mut();
            *calls += 1;
            *calls
        };
        if self.crash_before == Some(n) {
            panic!("injected crash before trashing {n}");
        }
        // Its folder found by any spelling, as macOS finds it; the name
        // too, unless the volume takes it only as it is stored.
        let path = &match self.stored_only {
            true => in_any_form(parent(path)).join(path.file_name().unwrap()),
            false => in_any_form(path),
        };
        if self.stored_only && !stored_spelling(path) {
            return Err(TrashError::Failed("not found".into()));
        }
        if self.fail_from.is_some_and(|from| n >= from)
            || self.refuses.iter().any(|d| path.starts_with(d))
        {
            return Err(TrashError::Unsupported);
        }
        if self.partial {
            let mut names: Vec<OsString> = std::fs::read_dir(path)
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .filter(|n| path.join(n).is_file())
                .collect();
            names.sort();
            if let Some(first) = names.first() {
                let to = self.dir.join(format!("{n}-{}", first.to_string_lossy()));
                std::fs::rename(path.join(first), to).unwrap();
            }
            return Err(TrashError::Failed("the Trash stopped part way".into()));
        }
        if let Some(meanwhile) = &self.meanwhile {
            meanwhile(path);
        }
        let location = self.dir.join(format!(
            "{n}-{}",
            path.file_name().unwrap().to_string_lossy()
        ));
        // A volume that can't rename exclusively (exFAT on macOS) moves it
        // plainly: the name in the Trash is this one's alone.
        match crate::fsops::rename_exclusive(path, &location) {
            Err(FsError::Unsupported) if !location.exists() => {
                std::fs::rename(path, &location).map_err(|e| TrashError::Failed(e.to_string()))?
            }
            other => other.map_err(|e| TrashError::Failed(e.to_string()))?,
        }
        // The AppleDouble file the test disk keeps beside it goes with it,
        // as macOS moves it.
        let double =
            path.with_file_name(format!("._{}", path.file_name().unwrap().to_string_lossy()));
        if std::fs::read(&double).is_ok_and(|b| b == APPLE_DOUBLE) {
            std::fs::remove_file(double).unwrap();
        }
        if let Some(after) = &self.after {
            after(&location);
        }
        if self.crash_after == Some(n) {
            panic!("injected crash after trashing {n}");
        }
        Ok(TrashReceipt { location })
    }
    fn supports(&self, _: &Path) -> TrashSupport {
        self.support
    }
}

/// A Trash that is never used.
pub(super) struct NoTrash;
impl Trash for NoTrash {
    fn trash(&self, _: &Path) -> Result<TrashReceipt, TrashError> {
        panic!("nothing should be trashed")
    }
}

/// The calls a test disk wrapping another (as field `$inner`) passes on
/// unchanged, so each needn't spell them out.
macro_rules! passes_on {
    ($inner:tt) => {
        fn volume_facts(
            &self,
            path: &std::path::Path,
        ) -> Result<crate::fsops::VolumeFacts, crate::fsops::FsError> {
            self.$inner.volume_facts(path)
        }
        fn create_file(
            &self,
            path: &std::path::Path,
        ) -> Result<std::fs::File, crate::fsops::FsError> {
            self.$inner.create_file(path)
        }
        fn copy_into(
            &self,
            from: &std::path::Path,
            to: &std::path::Path,
            into: &mut std::fs::File,
            go_on: &mut dyn FnMut(u64) -> bool,
        ) -> Result<crate::fsops::ContentHash, crate::fsops::CopyError> {
            self.$inner.copy_into(from, to, into, go_on)
        }
        fn handle_id(
            &self,
            at: &std::path::Path,
            file: &std::fs::File,
        ) -> Result<crate::fsops::FileId, crate::fsops::FsError> {
            self.$inner.handle_id(at, file)
        }
        fn hash_prefix(
            &self,
            path: &std::path::Path,
            len: u64,
        ) -> Result<crate::fsops::ContentHash, crate::fsops::FsError> {
            self.$inner.hash_prefix(path, len)
        }
        fn created(
            &self,
            path: &std::path::Path,
        ) -> Result<std::time::SystemTime, crate::fsops::FsError> {
            self.$inner.created(path)
        }
        fn sync_file(
            &self,
            at: &std::path::Path,
            file: &std::fs::File,
        ) -> Result<(), crate::fsops::FsError> {
            self.$inner.sync_file(at, file)
        }
        fn copy_extras(
            &self,
            from: &std::path::Path,
            to: &std::path::Path,
            into: Option<&std::fs::File>,
        ) -> Result<(), crate::fsops::FsError> {
            self.$inner.copy_extras(from, to, into)
        }
        fn copy_times(
            &self,
            from: &std::path::Path,
            to: &std::path::Path,
            into: Option<&std::fs::File>,
        ) -> Result<(), crate::fsops::FsError> {
            self.$inner.copy_times(from, to, into)
        }
        fn extras(
            &self,
            path: &std::path::Path,
        ) -> Result<crate::fsops::ContentHash, crate::fsops::FsError> {
            self.$inner.extras(path)
        }
        fn hash(
            &self,
            path: &std::path::Path,
        ) -> Result<crate::fsops::ContentHash, crate::fsops::FsError> {
            self.$inner.hash(path)
        }
        fn remove_file(&self, path: &std::path::Path) -> Result<(), crate::fsops::FsError> {
            self.$inner.remove_file(path)
        }
        fn remove_dir(&self, path: &std::path::Path) -> Result<(), crate::fsops::FsError> {
            self.$inner.remove_dir(path)
        }
    };
}
pub(super) use passes_on;

/// A disk call, as recorded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Call {
    Entry(PathBuf),
    List(PathBuf),
    Identity(PathBuf),
    Rename(PathBuf, PathBuf),
    CreateDir(PathBuf),
    Sync(PathBuf),
    CreateFile(PathBuf),
    Copy(PathBuf, PathBuf),
    /// A chunk of a copy written, with the bytes written so far.
    Chunk(u64),
    SyncFile(PathBuf),
    CopyExtras(PathBuf),
    CopyTimes(PathBuf),
    Hash(PathBuf),
    RemoveFile(PathBuf),
    RemoveDir(PathBuf),
}

/// A test's hook on each disk call.
pub(super) type CallHook = Option<Box<dyn Fn(&Call)>>;

/// Which kind of call a fault is at.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) enum Op {
    List,
    Rename,
    CreateDir,
    Sync,
    CreateFile,
    Copy,
    SyncFile,
    CopyExtras,
    CopyTimes,
    Hash,
    RemoveFile,
    RemoveDir,
}

impl Op {
    /// The steps of a copy to another volume.
    pub(super) const COPYING: [Op; 9] = [
        Op::CreateFile,
        Op::Copy,
        Op::SyncFile,
        Op::CopyExtras,
        Op::CopyTimes,
        Op::Hash,
        Op::Rename,
        Op::Sync,
        Op::RemoveFile,
    ];
}

/// What happens at the faulty call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Fault {
    /// It fails, changing nothing; a copy fails part way, as on a full
    /// drive.
    Fail,
    /// The process ends before it.
    CrashBefore,
    /// The process ends right after it.
    CrashAfter,
    /// The process ends in the middle of a copy, part of it written; before
    /// any other call.
    CrashMid,
}

/// The real disk, recording every call and faulting at the `nth` (from 0)
/// call of a kind, for each fault it has; and, for copies, numbering what
/// is at or below `elsewhere` as if on another volume, and changing what
/// the hooks ask for.
#[derive(Default)]
pub(super) struct Faulty {
    pub faults: Vec<(Op, usize, Fault)>,
    pub calls: RefCell<Vec<Call>>,
    pub seen: RefCell<BTreeMap<String, usize>>,
    /// A folder taken for another volume's, with everything in it.
    pub elsewhere: Option<PathBuf>,
    /// The bytes every volume has free, instead of its own.
    pub space: Option<u64>,
    /// The step, in nanoseconds, that a copy's time is cut to, as a volume
    /// keeping times more coarsely does.
    pub coarse: Option<u64>,
    /// Each copy reads back with a byte changed.
    pub corrupt: bool,
    /// The volume taken for another can't keep the extras (extended
    /// attributes, alternate data streams) of what is copied there, as
    /// Linux's and Windows' FAT and exFAT can't.
    pub keeps_no_extras: bool,
    /// The extras of this file or folder read otherwise from the `n`-th
    /// time they are read (from 0) on, as if the user tagged it then.
    pub retag: Option<(PathBuf, usize)>,
    pub extras_read: RefCell<BTreeMap<PathBuf, usize>>,
    /// The original gets another byte while it is copied.
    pub change_source: bool,
    /// The original is taken away to this path while it is copied.
    pub take_source: Option<PathBuf>,
    /// A file there takes this name just before something is renamed to it.
    pub occupy: Option<PathBuf>,
    /// A file with these bytes takes this name just before a file is made
    /// there; a folder, just before a folder is.
    pub occupy_on_create: Option<(PathBuf, Vec<u8>)>,
    /// Right after the first file is made, another file with these bytes
    /// takes its name (another computer, on a share); where, is kept in
    /// `swapped`.
    pub swap_after_create: Option<Vec<u8>>,
    pub swapped: RefCell<Option<PathBuf>>,
    /// Just before the first call of this kind on a file `elsewhere`,
    /// another file with these bytes, dated 2001, takes that file's name
    /// (another computer, on a share), the file there moved aside; where,
    /// is kept in `swapped`.
    pub swap_before: Option<(Op, Vec<u8>)>,
    /// Once set, `elsewhere`'s volume can't say what it is.
    pub facts_fail: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Set once a copy wrote its first chunk, as Stop pressed then.
    pub stop: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Folders whose volume says its file ids can't be proven, as network
    /// volumes do, and isn't FAT or exFAT: refused.
    pub unstable_ids: Vec<PathBuf>,
    /// Folders, with everything in them, on a volume that says it is exFAT
    /// (local, its file ids not proven): proven by names.
    pub names_only: Vec<PathBuf>,
    /// The `names_only` volume can't rename exclusively, as macOS's exFAT
    /// can't: it says so, and refuses to.
    pub no_exclusive_rename: bool,
    /// The `names_only` volume says it is an SMB share (on the network, not
    /// renaming exclusively) rather than exFAT.
    pub share: bool,
    /// Files the volume says were made long ago (1970).
    pub born_long_ago: Vec<PathBuf>,
    /// The `names_only` volume gets an AppleDouble file (`._name`) beside
    /// each file whose times are set and each folder made there, which
    /// goes with it when it is renamed or removed, as macOS keeps what FAT
    /// and exFAT can't.
    pub apple_double: bool,
    /// The `names_only` volume lists names decomposed (NFD) but renames and
    /// removes a file only by the spelling it is stored in (as it was made),
    /// as macOS's FAT and exFAT drivers do.
    pub lists_decomposed: bool,
    /// `elsewhere` keeps the names it is given decomposed (NFD), as HFS+
    /// does, and finds a file by any normalisation form of its name.
    pub decompose: bool,
    /// Files there, and found by name, that their folder's listing leaves
    /// out, as an SMB share's listing lags a file another computer made.
    pub unlisted: Vec<PathBuf>,
    /// `elsewhere` gives a file a new number once it is written and closed,
    /// as FAT does.
    pub renumber_on_close: bool,
    /// `elsewhere` gives a file a new number when it is renamed.
    pub renumber_on_rename: bool,
    /// The number `elsewhere` gives each real file object, where it gave
    /// it another than its own.
    pub numbers: RefCell<BTreeMap<crate::fsops::FileId, crate::fsops::FileId>>,
    pub renumbered: RefCell<u64>,
    /// Every listing in reverse order, so nothing may rely on the order a
    /// folder lists its names in.
    pub list_reversed: bool,
    /// Folders that can't be listed, as macOS's Trash without Full Disk
    /// Access, or a drive gone offline.
    pub unlistable: RefCell<Vec<PathBuf>>,
    /// Runs on each call as it is made.
    pub watch: CallHook,
    /// Runs on each folder listed, before it is.
    pub on_list: PathHook,
    /// Every volume says it is read-only.
    pub read_only: bool,
    /// Every volume says it is a network share.
    pub network: bool,
}

impl Faulty {
    pub(super) fn at(op: Op, nth: usize, fault: Fault) -> Self {
        Self::with(&[(op, nth, fault)])
    }
    pub(super) fn with(faults: &[(Op, usize, Fault)]) -> Self {
        Self {
            faults: faults.to_vec(),
            ..Self::default()
        }
    }
    /// This disk with `dir` taken for another volume.
    pub(super) fn elsewhere(mut self, dir: &Path) -> Self {
        self.elsewhere = Some(dir.to_path_buf());
        self
    }
    /// How many calls of `op` it saw.
    pub(super) fn count(&self, op: Op) -> usize {
        self.seen
            .borrow()
            .get(&format!("{op:?}"))
            .copied()
            .unwrap_or(0)
    }
    /// The fault at this call of kind `op`, if any, with its number.
    fn fault(&self, op: Op) -> (usize, Option<Fault>) {
        let n = {
            let mut seen = self.seen.borrow_mut();
            let n = seen.entry(format!("{op:?}")).or_default();
            *n += 1;
            *n - 1
        };
        let fault = self
            .faults
            .iter()
            .find(|(at, nth, _)| *at == op && *nth == n)
            .map(|f| f.2);
        (n, fault)
    }
    /// Runs `step`, a call of kind `op`, with the fault if this is the one.
    fn step<T>(&self, op: Op, step: impl FnOnce() -> Result<T, FsError>) -> Result<T, FsError> {
        let (n, fault) = self.fault(op);
        match fault {
            Some(fault) => match fault {
                Fault::Fail => Err(FsError::Io(std::io::Error::other("injected failure"))),
                Fault::CrashBefore | Fault::CrashMid => panic!("injected crash before {op:?} {n}"),
                Fault::CrashAfter => {
                    let _done = step();
                    panic!("injected crash after {op:?} {n}")
                }
            },
            _ => step(),
        }
    }
    /// Whether `path` is on the `names_only` volume, which takes no name
    /// decomposed (`lists_decomposed`), and is one.
    fn refuses_decomposed(&self, path: &Path) -> bool {
        self.lists_decomposed && self.fat(path) && !stored_spelling(&self.in_found(path))
    }
    /// `path`'s AppleDouble file, where the volume keeps one.
    fn apple_double_of(&self, path: &Path) -> Option<PathBuf> {
        let name = path.file_name()?.to_string_lossy();
        (self.apple_double && self.fat(path)).then(|| path.with_file_name(format!("._{name}")))
    }
    /// Removes `path`'s AppleDouble file once `path` is `gone`.
    fn forget_apple_double(&self, path: &Path, gone: bool) {
        if let (true, Some(double)) = (gone, self.apple_double_of(path))
            && double.exists()
        {
            std::fs::remove_file(double).unwrap();
        }
    }
    /// Swaps the file at `path` for another, if `swap_before` asks it
    /// at this call of kind `op`.
    fn swap(&self, op: Op, path: &Path) {
        let Some((at_op, theirs)) = &self.swap_before else {
            return;
        };
        let at = self.found(path);
        if *at_op != op || self.swapped.borrow().is_some() || !self.far(path) || !at.is_file() {
            return;
        }
        // Moved aside rather than removed, as `swap_after_create`.
        std::fs::rename(&at, at.with_extension("aside")).unwrap();
        std::fs::write(&at, theirs).unwrap();
        let dated = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
        std::fs::File::options()
            .write(true)
            .open(&at)
            .unwrap()
            .set_modified(dated)
            .unwrap();
        *self.swapped.borrow_mut() = Some(at);
    }
    /// Whether `path` is on the `names_only` volume.
    fn fat(&self, path: &Path) -> bool {
        self.names_only.iter().any(|d| path.starts_with(d))
    }
    /// Whether `path` is taken for another volume's.
    fn far(&self, path: &Path) -> bool {
        self.elsewhere.as_ref().is_some_and(|d| path.starts_with(d))
    }
    /// `path` as `elsewhere` stores it when it makes a name there.
    fn stored(&self, path: &Path) -> PathBuf {
        use unicode_normalization::UnicodeNormalization;
        let path = self.in_found(path);
        match (self.decompose && self.far(&path), path.file_name()) {
            (true, Some(name)) => {
                let nfd: String = name.to_string_lossy().nfd().collect();
                path.with_file_name(nfd)
            }
            _ => path,
        }
    }
    /// Whether the volume of `path` finds a name in any normalisation
    /// form: `elsewhere` when it decomposes, and the `names_only` volume
    /// when it lists names decomposed, as macOS's drivers do.
    fn any_form(&self, path: &Path) -> bool {
        (self.decompose && self.far(path)) || (self.lists_decomposed && self.fat(path))
    }
    /// The file `path` names, on a volume that finds a name in any
    /// normalisation form (`any_form`): each name as it is stored, whatever
    /// the temporary folder's file system finds (Linux's only by bytes).
    fn found(&self, path: &Path) -> PathBuf {
        match self.any_form(path) {
            true => in_any_form(path),
            false => path.to_path_buf(),
        }
    }
    /// `path` with its folder [`Self::found`] and its own name as given: where
    /// a name is made or looked for as it is spelled.
    fn in_found(&self, path: &Path) -> PathBuf {
        match path.file_name() {
            Some(name) => self.found(parent(path)).join(name),
            None => path.to_path_buf(),
        }
    }
    /// The number `elsewhere` gives real file object `real`.
    fn number(&self, real: crate::fsops::FileId) -> crate::fsops::FileId {
        self.numbers
            .borrow()
            .get(&real)
            .copied()
            .unwrap_or_else(|| real.elsewhere())
    }
    /// Gives the file at `path`, if `elsewhere`, a number never given yet.
    fn renumber(&self, path: &Path) {
        if !self.far(path) {
            return;
        }
        let Ok(real) = Real.entry(path).map(|e| e.id) else {
            return;
        };
        let n = {
            let mut n = self.renumbered.borrow_mut();
            *n += 1;
            u64::MAX - *n
        };
        let fake = real.elsewhere().numbered(n);
        self.numbers.borrow_mut().insert(real, fake);
    }
    /// A remount of `elsewhere`: each of `files` gets a new number, and the
    /// file at `reuse.1` the number the file at `reuse.0` had before.
    pub(super) fn remount(&self, files: &[&Path], reuse: Option<(&Path, &Path)>) {
        let before = reuse.map(|(a, _)| self.number(Real.entry(a).unwrap().id));
        for f in files {
            self.renumber(f);
        }
        if let (Some((_, b)), Some(old)) = (reuse, before) {
            let real = Real.entry(b).unwrap().id;
            self.numbers.borrow_mut().insert(real, old);
        }
    }
    /// Records `call`, and shows it to the watch.
    fn record(&self, call: Call) {
        if let Some(watch) = &self.watch {
            watch(&call);
        }
        self.calls.borrow_mut().push(call);
    }
    pub(super) fn renames(&self) -> Vec<(PathBuf, PathBuf)> {
        self.calls
            .borrow()
            .iter()
            .filter_map(|c| match c {
                Call::Rename(a, b) => Some((a.clone(), b.clone())),
                _ => None,
            })
            .collect()
    }
    /// How many chunks its copies wrote.
    pub(super) fn chunks(&self) -> usize {
        self.calls
            .borrow()
            .iter()
            .filter(|c| matches!(c, Call::Chunk(_)))
            .count()
    }
}

impl Disk for Faulty {
    fn entry(&self, path: &Path) -> Result<Entry, FsError> {
        self.record(Call::Entry(path.into()));
        let mut found = Real.entry(&self.found(path))?;
        if self.far(path) {
            found.id = self.number(found.id);
        }
        Ok(found)
    }
    fn list(&self, dir: &Path) -> Result<Vec<OsString>, FsError> {
        use unicode_normalization::UnicodeNormalization;
        self.record(Call::List(dir.into()));
        if let Some(hook) = &self.on_list {
            hook(dir);
        }
        if self.unlistable.borrow().iter().any(|d| d == dir) {
            return Err(FsError::Io(std::io::Error::other("not permitted")));
        }
        let mut names = self.step(Op::List, || Real.list(&self.found(dir)))?;
        if self.list_reversed {
            names.sort();
            names.reverse();
        }
        names.retain(|n| !self.unlisted.iter().any(|u| *u == dir.join(n)));
        match self.lists_decomposed && self.fat(dir) {
            true => Ok(names
                .into_iter()
                .map(|n| n.to_string_lossy().nfd().collect::<String>().into())
                .collect()),
            false => Ok(names),
        }
    }
    fn identity(&self, path: &Path) -> anyhow::Result<Identity> {
        self.record(Call::Identity(path.into()));
        Real.identity(&self.found(path))
    }
    fn rename(&self, from: &Path, to: &Path) -> Result<(), FsError> {
        self.record(Call::Rename(from.into(), to.into()));
        if self.occupy.as_deref() == Some(to) {
            std::fs::write(to, b"raced").unwrap();
        }
        if self.refuses_decomposed(from) || self.refuses_decomposed(to) {
            return self.step(Op::Rename, || Err(FsError::NotFound));
        }
        let (from, to) = (&self.found(from), &self.stored(to));
        if self.no_exclusive_rename && self.fat(to) {
            return self.step(Op::Rename, || Err(FsError::Unsupported));
        }
        let renamed = self.step(Op::Rename, || Real.rename(from, to));
        if renamed.is_ok()
            && let (Some(a), Some(b)) = (self.apple_double_of(from), self.apple_double_of(to))
            && a.exists()
        {
            std::fs::rename(a, b).unwrap();
        }
        if renamed.is_ok() && self.renumber_on_rename {
            self.renumber(to);
        }
        renamed
    }
    fn create_dir(&self, path: &Path) -> Result<(), FsError> {
        self.record(Call::CreateDir(path.into()));
        if let Some((at, _)) = &self.occupy_on_create
            && at == path
        {
            std::fs::create_dir(at).unwrap();
        }
        let path = &self.in_found(path);
        let made = self.step(Op::CreateDir, || Real.create_dir(path));
        if made.is_ok()
            && let Some(double) = self.apple_double_of(path)
        {
            std::fs::write(double, APPLE_DOUBLE).unwrap();
        }
        made
    }
    fn sync_dir(&self, dir: &Path) -> Result<(), FsError> {
        self.record(Call::Sync(dir.into()));
        // The fault is still injected here. These crashes are panics in this
        // process, not power loss, so the durability flush is not what the
        // matrix proves. `sync_file` is FlushFileBuffers on NTFS; `sync_dir`
        // is an fsync on Unix and only an existence check on Windows.
        // Opening still fails when the folder is missing, as `sync_dir` does.
        self.step(Op::Sync, || opened_without_flush(&self.found(dir)))
    }
    fn canonical(&self, path: &Path) -> Result<PathBuf, FsError> {
        Real.canonical(&self.found(path))
    }
    fn volume_facts(&self, path: &Path) -> Result<crate::fsops::VolumeFacts, FsError> {
        if self.far(path)
            && let Some(fail) = &self.facts_fail
            && fail.load(std::sync::atomic::Ordering::Relaxed)
        {
            return Err(FsError::Io(std::io::Error::other("injected: no facts")));
        }
        let mut facts = Real.volume_facts(&self.found(path))?;
        if let Some(space) = self.space {
            facts.available = space;
        }
        // Proven ids unless a test says otherwise, whatever the temporary
        // folder is on (overlayfs in CI's containers, tmpfs elsewhere): the
        // real detection is `fsops`'s to test.
        facts.stable_ids = !self.unstable_ids.iter().any(|d| d == path);
        if self.fat(path) {
            use crate::fsops::VolumeKind;
            facts.kind = if self.share {
                VolumeKind::Smb
            } else {
                VolumeKind::Exfat
            };
            facts.local = !self.share;
            facts.stable_ids = false;
            facts.exclusive_rename = !self.no_exclusive_rename && !self.share;
        } else if !facts.stable_ids {
            facts.kind = crate::fsops::VolumeKind::Other;
        }
        if self.network {
            facts.kind = crate::fsops::VolumeKind::Other;
            facts.local = false;
            facts.stable_ids = false;
        }
        facts.read_only |= self.read_only;
        Ok(facts)
    }
    fn create_file(&self, path: &Path) -> Result<std::fs::File, FsError> {
        self.record(Call::CreateFile(path.into()));
        if let Some((at, bytes)) = &self.occupy_on_create
            && at == path
        {
            std::fs::write(at, bytes).unwrap();
        }
        let made = self.step(Op::CreateFile, || Real.create_file(&self.stored(path)))?;
        if let Some(theirs) = &self.swap_after_create
            && self.swapped.borrow().is_none()
        {
            // Moved aside rather than removed: Windows keeps a removed
            // file's name while a handle to it is open.
            let at = self.stored(path);
            std::fs::rename(&at, at.with_extension("aside")).unwrap();
            std::fs::write(&at, theirs).unwrap();
            *self.swapped.borrow_mut() = Some(at);
        }
        Ok(made)
    }
    fn copy_into(
        &self,
        from: &Path,
        to: &Path,
        into: &mut std::fs::File,
        go_on: &mut dyn FnMut(u64) -> bool,
    ) -> Result<crate::fsops::ContentHash, crate::fsops::CopyError> {
        use crate::fsops::CopyError;
        self.record(Call::Copy(from.into(), to.into()));
        let (from, to) = (&self.found(from), &self.found(to));
        let (n, fault) = self.fault(Op::Copy);
        match fault {
            Some(Fault::CrashBefore) => panic!("injected crash before copy {n}"),
            // Part of it: the first chunk.
            Some(Fault::Fail) => {
                let _part = Real.copy_into(from, to, into, &mut |_| false);
                return Err(CopyError::Fs(FsError::NoSpace));
            }
            Some(Fault::CrashMid) => {
                let _part = Real.copy_into(from, to, into, &mut |_| false);
                panic!("injected crash in copy {n}")
            }
            Some(Fault::CrashAfter) | None => {}
        }
        let copied = Real.copy_into(from, to, into, &mut |written| {
            self.record(Call::Chunk(written));
            if let Some(stop) = &self.stop {
                stop.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            go_on(written)
        });
        if copied.is_ok() && self.corrupt {
            let mut bytes = std::fs::read(to).unwrap();
            bytes[0] ^= 0xff;
            std::fs::write(to, bytes).unwrap();
        }
        if let (true, Some(away)) = (copied.is_ok(), &self.take_source) {
            std::fs::rename(from, away).unwrap();
        }
        if copied.is_ok() && self.change_source {
            let mut bytes = std::fs::read(from).unwrap();
            bytes.push(b'!');
            std::fs::write(from, bytes).unwrap();
        }
        // Closed after writing.
        if self.renumber_on_close {
            self.renumber(to);
        }
        if fault == Some(Fault::CrashAfter) {
            panic!("injected crash after copy {n}");
        }
        copied
    }
    fn handle_id(&self, at: &Path, file: &std::fs::File) -> Result<crate::fsops::FileId, FsError> {
        let id = Real.handle_id(at, file)?;
        Ok(match self.far(at) {
            true => self.number(id),
            false => id,
        })
    }
    fn sync_file(&self, at: &Path, _file: &std::fs::File) -> Result<(), FsError> {
        self.record(Call::SyncFile(at.into()));
        self.swap(Op::SyncFile, at);
        // Not flushed: these crashes are panics in this process, not power
        // loss (see `sync_dir`).
        self.step(Op::SyncFile, || Ok(()))
    }
    fn copy_extras(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError> {
        self.record(Call::CopyExtras(to.into()));
        self.swap(Op::CopyExtras, to);
        let (from, to) = (&self.found(from), &self.found(to));
        self.step(Op::CopyExtras, || {
            match self.keeps_no_extras && self.far(to) {
                true => Err(FsError::Unsupported),
                false => Real.copy_extras(from, to, into),
            }
        })
    }
    fn extras(&self, path: &Path) -> Result<crate::fsops::ContentHash, FsError> {
        let n = {
            let mut read = self.extras_read.borrow_mut();
            let n = read.entry(path.to_path_buf()).or_default();
            *n += 1;
            *n - 1
        };
        let found = Real.extras(&self.found(path))?;
        match &self.retag {
            Some((at, from)) if at == path && n >= *from => {
                // Another hash than the one read.
                let mut other = found.to_string();
                let first = if other.starts_with('0') { "1" } else { "0" };
                other.replace_range(..1, first);
                Ok(other.parse().unwrap())
            }
            _ => Ok(found),
        }
    }
    fn copy_times(
        &self,
        from: &Path,
        to: &Path,
        into: Option<&std::fs::File>,
    ) -> Result<(), FsError> {
        self.record(Call::CopyTimes(to.into()));
        self.swap(Op::CopyTimes, to);
        let (from, to) = (&self.found(from), &self.found(to));
        if let Some(double) = self.apple_double_of(to) {
            std::fs::write(double, APPLE_DOUBLE).unwrap();
        }
        self.step(Op::CopyTimes, || {
            Real.copy_times(from, to, into)?;
            if let (Some(step), Some(into)) = (self.coarse, into) {
                let at = into.metadata()?.modified()?;
                let ns = at.duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
                let cut = ns - ns % u128::from(step);
                let cut = std::time::UNIX_EPOCH
                    + std::time::Duration::new(
                        (cut / 1_000_000_000) as u64,
                        (cut % 1_000_000_000) as u32,
                    );
                into.set_modified(cut)?;
            }
            Ok(())
        })
    }
    fn hash(&self, path: &Path) -> Result<crate::fsops::ContentHash, FsError> {
        self.record(Call::Hash(path.into()));
        self.step(Op::Hash, || Real.hash(&self.found(path)))
    }
    fn remove_file(&self, path: &Path) -> Result<(), FsError> {
        self.record(Call::RemoveFile(path.into()));
        self.swap(Op::RemoveFile, path);
        if self.refuses_decomposed(path) {
            return self.step(Op::RemoveFile, || Err(FsError::NotFound));
        }
        let at = &self.found(path);
        let removed = self.step(Op::RemoveFile, || Real.remove_file(at));
        self.forget_apple_double(at, removed.is_ok());
        removed
    }
    fn remove_dir(&self, path: &Path) -> Result<(), FsError> {
        self.record(Call::RemoveDir(path.into()));
        let at = &self.found(path);
        let removed = self.step(Op::RemoveDir, || Real.remove_dir(at));
        self.forget_apple_double(at, removed.is_ok());
        removed
    }
    fn hash_prefix(&self, path: &Path, len: u64) -> Result<crate::fsops::ContentHash, FsError> {
        Real.hash_prefix(&self.found(path), len)
    }
    fn created(&self, path: &Path) -> Result<std::time::SystemTime, FsError> {
        if self.born_long_ago.iter().any(|p| p == path) {
            return Ok(std::time::UNIX_EPOCH);
        }
        // FAT and exFAT keep birth times; where the temporary folder's file
        // system doesn't (overlayfs in CI's containers), its time written
        // stands for it.
        let path = self.found(path);
        match Real.created(&path) {
            Err(_) if self.fat(&path) => std::fs::symlink_metadata(&path)
                .and_then(|m| m.modified())
                .map_err(FsError::Io),
            born => born,
        }
    }
}

/// Gives file or folder `path` the extra `name` (an extended attribute, or
/// an alternate data stream on Windows), or says the temporary folder's
/// file system keeps none (overlayfs in some containers), for the test to
/// end there.
pub(super) fn tagged(path: &Path, name: &str, value: &[u8]) -> bool {
    match crate::fsops::set_extra(path, name, value) {
        Ok(()) => true,
        Err(FsError::Unsupported) => {
            eprintln!("{}: no extended attributes here", path.display());
            false
        }
        Err(e) => panic!("{}: {e}", path.display()),
    }
}

/// Whether a real drive, at folder `dir`, keeps extras: any on macOS (FAT
/// and exFAT in AppleDouble files), not FAT and exFAT elsewhere, and on
/// Windows an SMB share only where its server keeps alternate data streams
/// (Samba needs `streams_xattr`), as a file made there to try tells.
pub(super) fn keeps_extras(dir: &Path) -> bool {
    use crate::fsops::{VolumeKind, volume_facts};
    let tried = || {
        tempfile::NamedTempFile::new_in(dir)
            .is_ok_and(|f| crate::fsops::set_extra(f.path(), "tried", b"1").is_ok())
    };
    cfg!(target_vendor = "apple")
        || volume_facts(dir).is_ok_and(|f| match f.kind {
            VolumeKind::Fat | VolumeKind::Exfat => false,
            VolumeKind::Smb if cfg!(windows) => tried(),
            _ => true,
        })
}

/// Opens folder `path` the way the real `sync_dir` does, and does not
/// flush: opens it on Unix, and on Windows only checks that it is a
/// folder, because a folder cannot be opened there.
fn opened_without_flush(path: &Path) -> Result<(), FsError> {
    #[cfg(unix)]
    {
        std::fs::File::open(path)?;
        Ok(())
    }
    #[cfg(not(unix))]
    {
        if !std::fs::metadata(path)?.is_dir() {
            return Err(FsError::NotFound);
        }
        Ok(())
    }
}

/// Runs `f`, which may "crash", and says whether it did.
pub(super) fn crashes(f: impl FnOnce()) -> bool {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).is_err()
}

/// The results of `outcome`'s items, in order.
pub(super) fn results(outcome: &Outcome) -> Vec<ItemResult> {
    outcome.items.iter().map(|i| i.result.clone()).collect()
}

#[test]
fn a_crash_is_a_panic_the_tests_survive() -> Result<()> {
    assert!(crashes(|| panic!("injected")));
    assert!(!crashes(|| {}));
    Ok(())
}
