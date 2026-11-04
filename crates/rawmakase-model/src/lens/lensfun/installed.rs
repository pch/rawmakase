//! The Lensfun database installed on this computer: `lensfun` in the data folder
//! holds one folder per downloaded version, named for its timestamp and never changed
//! once written, and `installed.json`, which names the version in use with where and
//! when it was downloaded and its archive's SHA-256. Installing a version writes its
//! folder, then replaces `installed.json` atomically, so a reader sees the old version
//! or the new one whole. Downloading, checking and publishing are the app's
//! (`lensfun_install`); this reads what is installed.
//!
//! Only editing reads it: matching a photo's lens, choosing one, resolving an edit
//! that names one. Rendering uses the calibration an edit copied.
use super::Database;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The file naming the version in use.
pub const MANIFEST: &str = "installed.json";
/// Most files a version may hold, and their total size.
pub const MAX_FILES: usize = 200;
pub const MAX_BYTES: u64 = 32 << 20;

/// The folder Lensfun's database is installed in.
pub fn dir() -> PathBuf {
    crate::storage::data_dir().join("lensfun")
}

/// What `installed.json` records.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The database's timestamp, as Lensfun's `versions.json` and the archive's
    /// `timestamp.txt` give it; its folder's name.
    pub version: i64,
    /// The archive it came from, its size and SHA-256.
    pub url: String,
    pub bytes: u64,
    pub sha256: String,
    /// When it was downloaded, UTC "YYYY-MM-DD HH:MM:SS".
    pub downloaded: String,
    pub lenses: usize,
    pub cameras: usize,
}
impl Manifest {
    /// The version's folder name.
    pub fn folder(version: i64) -> String {
        format!("{version}")
    }
    /// The version's date, "2026-10-28".
    pub fn date(&self) -> String {
        {
            let [y, m, d, ..] = crate::time::utc(self.version);
            format!("{y:04}-{m:02}-{d:02}")
        }
    }
}

/// The installed version: its manifest and its database.
#[derive(Debug)]
pub struct Installed {
    pub manifest: Manifest,
    pub db: Database,
}

/// The manifest in `dir`, if a version is installed there.
pub fn manifest(dir: &Path) -> Option<Manifest> {
    let text = std::fs::read_to_string(dir.join(MANIFEST)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Reads the version `dir`'s manifest names. An update or removal may replace the
/// manifest and, later, take away the version it named while it is read: then the
/// manifest is read again, a few times.
pub fn load(dir: &Path) -> Result<Installed> {
    let mut tries = 0;
    loop {
        let manifest = manifest(dir).context("Lensfun profiles aren't installed")?;
        let folder = dir.join(Manifest::folder(manifest.version));
        match read_folder(&folder) {
            Ok(mut db) => {
                db.timestamp = manifest.version;
                return Ok(Installed { manifest, db });
            }
            Err(e) => {
                tries += 1;
                if tries >= 3 || self::manifest(dir).as_ref() == Some(&manifest) {
                    return Err(e);
                }
            }
        }
    }
}

/// Reads every `.xml` file of a version folder, within the limits.
pub fn read_folder(folder: &Path) -> Result<Database> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(folder)
        .with_context(|| format!("Could not read {}", folder.display()))?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("xml")))
        .collect();
    files.sort();
    ensure!(
        !files.is_empty(),
        "No Lensfun database files in {}",
        folder.display()
    );
    ensure!(files.len() <= MAX_FILES, "Too many Lensfun database files");
    let mut total = 0u64;
    let mut texts = Vec::with_capacity(files.len());
    for f in &files {
        let len = std::fs::metadata(f)?.len();
        total += len;
        ensure!(total <= MAX_BYTES, "The Lensfun database is too large");
        let name = f
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        texts.push((name, std::fs::read_to_string(f)?));
    }
    Database::from_texts(texts.iter().map(|(n, t)| (n.as_str(), t.as_str())))
}

/// The installed database, read again only when `installed.json` changes.
#[derive(Default)]
pub struct Cache {
    cached: Mutex<Option<Seen>>,
}
type Stamp = (std::time::SystemTime, u64);
/// The manifest's stamp when last read, what it named, and when a read failed.
type Seen = (
    Option<Stamp>,
    Option<Arc<Installed>>,
    Option<std::time::Instant>,
);
/// How long a database that could not be read counts as none before it is read
/// again (an update or removal may have been under way).
const RETRY: std::time::Duration = std::time::Duration::from_secs(30);
fn stamp(dir: &Path) -> Option<Stamp> {
    let m = std::fs::metadata(dir.join(MANIFEST)).ok()?;
    Some((m.modified().ok()?, m.len()))
}
impl Cache {
    pub const fn new() -> Self {
        Self {
            cached: Mutex::new(None),
        }
    }
    /// The database installed in `dir` now, if any; one that can't be read counts as
    /// none.
    pub fn current(&self, dir: &Path) -> Option<Arc<Installed>> {
        let now = stamp(dir);
        if let Some((s, installed, failed)) = self.lock().as_ref()
            && *s == now
            && failed.is_none_or(|at| at.elapsed() < RETRY)
        {
            return installed.clone();
        }
        // Read without the lock, as `lcp::LibraryCache` does.
        let (installed, failed) = match now.map(|_| load(dir)) {
            None => (None, None),
            Some(Ok(i)) => (Some(Arc::new(i)), None),
            Some(Err(e)) => {
                eprintln!("Lensfun profiles: {e:#}");
                (None, Some(std::time::Instant::now()))
            }
        };
        *self.lock() = Some((now, installed.clone(), failed));
        installed
    }
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Seen>> {
        self.cached.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The database editing a photo resolves Lensfun lenses against: the one installed
/// when it is asked for, so an install, update or removal since the photo opened
/// counts; or one given (tests). Copy Settings, Sync, presets and XMP pass
/// [`Handle::Installed`]; rendering never reads it, since an edit keeps its lens's
/// calibration.
#[derive(Clone, Default)]
pub enum Handle {
    #[default]
    Installed,
    Given(Option<Arc<Installed>>),
}
impl Handle {
    pub fn get(&self) -> Option<Arc<Installed>> {
        match self {
            Self::Installed => current(),
            Self::Given(given) => given.clone(),
        }
    }
}
impl std::fmt::Debug for Handle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Installed => f.write_str("installed Lensfun"),
            Self::Given(Some(i)) => write!(f, "Lensfun({})", i.manifest.version),
            Self::Given(None) => f.write_str("no Lensfun"),
        }
    }
}

static INSTALLED: Cache = Cache::new();

/// The database installed in the data folder now, if any.
pub fn current() -> Option<Arc<Installed>> {
    INSTALLED.current(&dir())
}
