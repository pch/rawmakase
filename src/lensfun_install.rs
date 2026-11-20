//! Installing, updating and removing Lensfun's lens database (#662), on the user's
//! request only: nothing is bundled, and nothing is fetched at startup. Where it is
//! installed and how it is read is `lens::lensfun::installed`'s.
//!
//! The source is Lensfun's own update channel, as `lensfun-update-data` uses it:
//! `versions.json` gives the newest database's timestamp and the formats offered, and
//! `version_2.tar.bz2` is that database in format 2. Lensfun publishes no checksum, so
//! an archive is accepted only over HTTPS, within size limits (8 MB packed, 32 MB and
//! 200 files unpacked), holding only regular `.xml` files (and an optional
//! `timestamp.txt`) without paths, every one of which parses as a Lensfun database,
//! and dated as `versions.json` says (its newest file's time is the database's
//! timestamp). Its SHA-256 and size go into the manifest.
//!
//! A version is unpacked into a staging folder beside the installed ones, synced, and
//! renamed to its own folder, which is never changed after; then `installed.json` is
//! replaced atomically to name it. A failure, a cancel or a crash at any point leaves
//! the version in use as it was. Older versions stay until the next install, update
//! or removal (so a reader that just read the old manifest can still read its files),
//! and staging folders a crashed install left are removed then. One install, update or
//! removal runs at a time across RAWmakase's instances and the CLI, under a lock file
//! in the folder.
//!
//! The database is CC BY-SA 3.0 (Lensfun contributors); the license text ships in
//! `licenses/` and the app shows it with the source before downloading.
use crate::lens::lensfun::installed::{self, MANIFEST, Manifest};
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

pub const VERSIONS_URL: &str = "https://lensfun.github.io/db/versions.json";
pub const ARCHIVE_URL: &str = crate::lens::lensfun::choice::SOURCE_URL;
/// The database format this release reads.
pub const FORMAT: u64 = 2;
/// The largest archive accepted, downloaded or read from a file. The unpacked
/// total and `versions.json` have limits of their own.
pub const MAX_ARCHIVE: u64 = 8 << 20;
const MAX_VERSIONS: u64 = 64 << 10;
/// About what Lensfun's archive weighs, for messages before downloading.
pub const ABOUT_MEGABYTES: f32 = 0.5;
/// The lock file one install, update or removal holds.
const LOCK: &str = ".install.lock";
const STAGING: &str = ".staging-";
/// How long a version no longer in use is kept for readers, and a staging folder
/// for the install writing it.
const KEEP_OLD: Duration = Duration::from_secs(600);
const KEEP_STAGING: Duration = Duration::from_secs(3600);
/// How long to wait for another instance's install before giving up.
const LOCK_WAIT: Duration = Duration::from_secs(300);

/// How far a download is, and whether to stop.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}
impl Progress {
    fn check(&self) -> Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            bail!(Cancelled);
        }
        Ok(())
    }
}

/// The user cancelled.
#[derive(Debug)]
pub struct Cancelled;
impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Cancelled")
    }
}
impl std::error::Error for Cancelled {}
pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Cancelled>().is_some()
}

/// Where a database comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Lensfun's update channel.
    Lensfun,
    /// An archive already downloaded (`version_2.tar.bz2`), dated by its files.
    File(PathBuf),
}

/// What an install did.
#[derive(Clone, Debug, PartialEq)]
pub enum Installed {
    /// This version is now in use.
    New(Manifest),
    /// It already was.
    Current(Manifest),
}

/// The newest database Lensfun offers: its timestamp, read from `versions.json`
/// (an array whose first item is the timestamp and second the formats offered).
pub fn latest(progress: &Progress) -> Result<i64> {
    let bytes = fetch(VERSIONS_URL, MAX_VERSIONS, progress, false)?;
    versions(&bytes)
}
fn versions(bytes: &[u8]) -> Result<i64> {
    let v: serde_json::Value =
        serde_json::from_slice(bytes).context("Lensfun's versions.json isn't JSON")?;
    let timestamp = v[0]
        .as_i64()
        .context("Lensfun's versions.json has no timestamp")?;
    let formats = v[1]
        .as_array()
        .context("Lensfun's versions.json lists no formats")?;
    ensure!(
        formats.iter().any(|f| f.as_u64() == Some(FORMAT)),
        "Lensfun no longer offers its database in format {FORMAT}"
    );
    ensure!(timestamp > 0, "Lensfun's versions.json has a bad timestamp");
    Ok(timestamp)
}

/// Installs the newest database from `source` into `dir` (the data folder's
/// `lensfun`), unless it is the one installed.
pub fn install(dir: &Path, source: &Source, progress: &Progress) -> Result<Installed> {
    std::fs::create_dir_all(dir).with_context(|| format!("Could not create {}", dir.display()))?;
    let _lock = lock(dir, progress)?;
    clean(dir);
    let (archive, expected, url) = match source {
        Source::Lensfun => {
            let version = latest(progress)?;
            if let Some(m) = current(dir).filter(|m| is_current(m.version, version)) {
                return Ok(Installed::Current(m));
            }
            let archive = fetch(ARCHIVE_URL, MAX_ARCHIVE, progress, true)?;
            (archive, Some(version), ARCHIVE_URL.to_string())
        }
        Source::File(path) => {
            let len = std::fs::metadata(path)
                .with_context(|| format!("Could not read {}", path.display()))?
                .len();
            ensure!(
                len <= MAX_ARCHIVE,
                "{} is too large for a Lensfun database",
                path.display()
            );
            (std::fs::read(path)?, None, path.display().to_string())
        }
    };
    progress.check()?;
    let unpacked = unpack(&archive, progress)?;
    if let Some(expected) = expected {
        ensure!(
            unpacked.version == expected,
            "The archive is dated {}, not {expected} as Lensfun's versions.json says",
            unpacked.version
        );
    }
    if let Some(m) = current(dir).filter(|m| m.version == unpacked.version) {
        return Ok(Installed::Current(m));
    }
    let manifest = Manifest {
        version: unpacked.version,
        url,
        bytes: archive.len() as u64,
        sha256: Sha256::digest(&archive)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        downloaded: crate::time::now_text(),
        lenses: unpacked.db.lenses.len(),
        cameras: unpacked.db.cameras.len(),
    };
    publish(dir, &unpacked, &manifest, progress)?;
    Ok(Installed::New(manifest))
}

/// Whether a version installed is current against Lensfun's newest: the same, or
/// newer (installed from a file); an update never goes back.
fn is_current(installed: i64, latest: i64) -> bool {
    installed >= latest
}

/// Removes the installed database. Edits keep their copies of what they use.
pub fn remove(dir: &Path, progress: &Progress) -> Result<bool> {
    if !dir.exists() {
        return Ok(false);
    }
    let _lock = lock(dir, progress)?;
    progress.check()?;
    let manifest = dir.join(MANIFEST);
    let was = manifest.exists();
    if was {
        std::fs::remove_file(&manifest)
            .with_context(|| format!("Could not remove {}", manifest.display()))?;
        let _ = crate::storage::sync_dir(dir);
    }
    // Every version, now that none is named; a reader that had just read the
    // manifest finds none and reads nothing.
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.is_dir() && version_folder(&entry.file_name().to_string_lossy()).is_some() {
            std::fs::remove_dir_all(&path)
                .with_context(|| format!("Could not remove {}", path.display()))?;
        }
    }
    clean(dir);
    Ok(was)
}

/// The version installed in `dir`, if its folder is there.
pub fn current(dir: &Path) -> Option<Manifest> {
    installed::manifest(dir).filter(|m| dir.join(Manifest::folder(m.version)).is_dir())
}

/// An archive's database, read and checked.
struct Unpacked {
    version: i64,
    files: Vec<(String, Vec<u8>)>,
    db: crate::lens::lensfun::Database,
}

/// The archive's files, within the limits, each one a Lensfun database. Its
/// version is its newest file's time, which Lensfun sets to the database's
/// timestamp; a `timestamp.txt`, where there is one, must agree.
fn unpack(archive: &[u8], progress: &Progress) -> Result<Unpacked> {
    let tar = Limited {
        inner: bzip2_rs::DecoderReader::new(archive),
        left: installed::MAX_BYTES + (1 << 20),
    };
    let mut entries = tar::Archive::new(tar);
    let mut count = 0usize;
    let mut files = Vec::new();
    let mut total = 0u64;
    let mut newest = 0i64;
    let mut stated = None;
    for entry in entries.entries().context("Not a tar archive")? {
        progress.check()?;
        let mut entry = entry.context("A damaged tar archive")?;
        count += 1;
        ensure!(
            count <= installed::MAX_FILES,
            "The archive is larger than a Lensfun database"
        );
        let header = entry.header();
        let path = entry
            .path()
            .context("A file name that isn't valid")?
            .into_owned();
        let name = plain_name(&path).with_context(|| {
            format!("The archive holds {}, not a database file", path.display())
        })?;
        ensure!(
            header.entry_type().is_file(),
            "The archive holds {name}, which isn't a regular file"
        );
        let size = header.size()?;
        total += size;
        ensure!(
            files.len() < installed::MAX_FILES && total <= installed::MAX_BYTES,
            "The archive is larger than a Lensfun database"
        );
        let mtime = i64::try_from(header.mtime()?).context("A file dated past 2262")?;
        let mut bytes = Vec::with_capacity(size as usize);
        entry.read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == size,
            "A file in the archive is cut short"
        );
        if name == "timestamp.txt" {
            stated = Some(
                String::from_utf8_lossy(&bytes)
                    .trim()
                    .parse::<i64>()
                    .context("A timestamp.txt without a timestamp")?,
            );
            continue;
        }
        ensure!(
            name.to_ascii_lowercase().ends_with(".xml"),
            "The archive holds {name}, not a database file"
        );
        newest = newest.max(mtime);
        files.push((name, bytes));
    }
    // The whole compressed stream, read to its end: a cut-short or padded archive
    // fails here rather than counting as whole.
    std::io::copy(&mut entries.into_inner(), &mut std::io::sink())
        .context("The archive is damaged or larger than a Lensfun database")?;
    ensure!(!files.is_empty(), "The archive holds no database files");
    if let Some(stated) = stated {
        ensure!(
            stated == newest,
            "The archive's timestamp.txt says {stated}, its files {newest}"
        );
    }
    ensure!(newest > 0, "The archive's files are undated");
    let texts: Vec<(String, String)> = files
        .iter()
        .map(|(n, b)| {
            String::from_utf8(b.clone())
                .map(|t| (n.clone(), t))
                .with_context(|| format!("{n} isn't UTF-8"))
        })
        .collect::<Result<_>>()?;
    let db = crate::lens::lensfun::Database::from_texts(
        texts.iter().map(|(n, t)| (n.as_str(), t.as_str())),
    )?;
    ensure!(
        !db.lenses.is_empty(),
        "The archive's database has no lenses"
    );
    Ok(Unpacked {
        version: newest,
        files,
        db,
    })
}

/// A file name without folders, "." or "..".
fn plain_name(path: &Path) -> Option<String> {
    let mut parts = path.components();
    let first = parts.next()?;
    // A leading "./", as tar may write.
    let name = match first {
        std::path::Component::CurDir => parts.next()?,
        other => other,
    };
    match (name, parts.next()) {
        (std::path::Component::Normal(n), None) => {
            let n = n.to_str()?.to_string();
            (!n.starts_with('.') && !n.contains(['/', '\\', ':'])).then_some(n)
        }
        _ => None,
    }
}

/// Writes the version's folder through a staging folder, then names it in the
/// manifest. A cancel before the manifest is written leaves the version in use.
fn publish(
    dir: &Path,
    unpacked: &Unpacked,
    manifest: &Manifest,
    progress: &Progress,
) -> Result<()> {
    let target = dir.join(Manifest::folder(unpacked.version));
    if target.is_dir() {
        // Written whole by an earlier install (a folder is only ever renamed into
        // place complete) and never changed since; checked again all the same.
        installed::read_folder(&target).with_context(|| {
            format!(
                "{} is damaged; remove Lensfun profiles and install again",
                target.display()
            )
        })?;
    } else {
        let staging = dir.join(format!(
            "{STAGING}{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let written = (|| -> Result<()> {
            std::fs::create_dir(&staging)?;
            for (name, bytes) in &unpacked.files {
                progress.check()?;
                let path = staging.join(name);
                let mut f = std::fs::File::create_new(&path)
                    .with_context(|| format!("Could not write {}", path.display()))?;
                std::io::Write::write_all(&mut f, bytes)?;
                f.sync_all()?;
            }
            let _ = crate::storage::sync_dir(&staging);
            progress.check()?;
            std::fs::rename(&staging, &target)
                .with_context(|| format!("Could not publish {}", target.display()))?;
            let _ = crate::storage::sync_dir(dir);
            Ok(())
        })();
        if let Err(e) = written {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    // The manifest is written and synced beside its place, and replaces the old one
    // only if no cancel came meanwhile.
    let mut staged =
        tempfile::NamedTempFile::new_in(dir).context("Could not record the installed version")?;
    serde_json::to_writer_pretty(&mut staged, manifest)?;
    staged.as_file().sync_all()?;
    progress.check()?;
    staged
        .persist(dir.join(MANIFEST))
        .map_err(|e| e.error)
        .context("Could not record the installed version")?;
    let _ = crate::storage::sync_dir(dir);
    Ok(())
}

/// A reader that fails, rather than ends, past its limit: a stream cut there would
/// otherwise read as a whole, smaller archive.
struct Limited<R> {
    inner: R,
    left: u64,
}
impl<R: Read> Read for Limited<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if n as u64 > self.left {
            return Err(std::io::Error::other("larger than a Lensfun database"));
        }
        self.left -= n as u64;
        Ok(n)
    }
}

/// A version folder's name: its timestamp.
fn version_folder(name: &str) -> Option<i64> {
    name.parse()
        .ok()
        .filter(|v: &i64| *v > 0 && Manifest::folder(*v) == name)
}

/// Removes, under the lock, versions no longer named that have had time to be read
/// (out of use since the manifest last changed, that long ago), and staging folders
/// older installs left.
fn clean(dir: &Path) {
    let active = installed::manifest(dir).map(|m| m.version);
    // No manifest: nothing is in use. One dated ahead of the clock is new.
    let out_of_use = match std::fs::metadata(dir.join(MANIFEST)).and_then(|m| m.modified()) {
        Err(_) => true,
        Ok(at) => at.elapsed().is_ok_and(|age| age > KEEP_OLD),
    };
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let age = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .unwrap_or_default();
        let old_version = version_folder(&name).is_some_and(|v| Some(v) != active)
            && age > KEEP_OLD
            && out_of_use;
        let stale = name.starts_with(STAGING) && age > KEEP_STAGING;
        if (old_version || stale) && entry.path().is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// The folder's lock, waiting while another instance holds it (noticing a cancel),
/// at most `LOCK_WAIT`. Unlocked when dropped.
fn lock(dir: &Path, progress: &Progress) -> Result<std::fs::File> {
    let file = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK))
        .with_context(|| format!("Could not write in {}", dir.display()))?;
    let start = std::time::Instant::now();
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(std::fs::TryLockError::WouldBlock) => {
                progress.check()?;
                ensure!(
                    start.elapsed() < LOCK_WAIT,
                    "Another RAWmakase is installing Lensfun profiles"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.into()),
        }
    }
}

/// Reads `url` whole, at most `limit` bytes, counting into `progress` when `count`.
fn fetch(url: &str, limit: u64, progress: &Progress, count: bool) -> Result<Vec<u8>> {
    progress.check()?;
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(120)))
        .https_only(true)
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("Could not reach {url}"))?;
    if count && let Some(len) = response.body().content_length() {
        ensure!(len <= limit, "{url} is larger than expected");
        progress.total.store(len, Ordering::Relaxed);
    }
    let mut reader = response.body_mut().as_reader();
    let mut out = Vec::new();
    let mut buffer = vec![0u8; 64 << 10];
    loop {
        progress.check()?;
        let n = reader
            .read(&mut buffer)
            .with_context(|| format!("Downloading {url}"))?;
        if n == 0 {
            break;
        }
        ensure!(
            out.len() as u64 + n as u64 <= limit,
            "{url} is larger than expected"
        );
        out.extend_from_slice(&buffer[..n]);
        if count {
            progress.done.store(out.len() as u64, Ordering::Relaxed);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
