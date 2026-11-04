//! Installing the models the app runs (Select Subject's and Sky's, AI Denoise's):
//! downloaded from where the release pins them (or, in tests, copied from a folder).
//! Nothing is published until its size and SHA-256 match the manifest. The app's
//! `Installer` (`app/subject_mask/models.rs`) runs an install on its own thread,
//! only on the user's choice; the AI Denoise service and the selection worker find
//! the installed models and the runtime here.
use rawmakase_inference::ModelFile;
use rawmakase_inference::manifest::{all_files, denoise_files, sky_files};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

/// Why an install stopped. Shown as text, as it always was: a cancel is
/// "Cancelled", a full disk "the disk is full", anything else its reason. A cancel or
/// a full disk stops a download at once; other failures may be retried.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum InstallError {
    Cancelled,
    DiskFull,
    Failed(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("Cancelled"),
            Self::DiskFull => f.write_str("the disk is full"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

impl From<String> for InstallError {
    fn from(why: String) -> Self {
        Self::Failed(why)
    }
}

/// How much of the model has been fetched, and whether to stop.
#[derive(Default)]
pub(crate) struct Progress {
    done: AtomicU64,
    cancel: AtomicBool,
}
impl Progress {
    /// Bytes fetched so far.
    pub(crate) fn fetched(&self) -> u64 {
        self.done.load(Ordering::Relaxed)
    }
    /// Asks the install to stop.
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// One set of model files, installed and removed as a unit: Subject and Background share
/// one, Sky and AI Denoise each have their own, so none obliges anyone to fetch another's.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Pack {
    #[default]
    Subject,
    Sky,
    /// AI Denoise's model, with its `LICENSE` and `NOTICE`: an install without them is
    /// not one.
    Denoise,
}
impl Pack {
    fn files(self) -> Vec<ModelFile> {
        match self {
            Self::Subject => all_files(),
            Self::Sky => sky_files(),
            Self::Denoise => denoise_files(),
        }
    }
    pub(crate) fn bytes(self) -> u64 {
        self.files().iter().map(|f| f.size_bytes).sum()
    }
    /// Where it is kept: under this computer's own data folder, in a folder named for the
    /// model and its contract version, so a replacement model sits beside it.
    pub(crate) fn dir(self) -> PathBuf {
        let (id, version) = match self {
            Self::Subject => (
                rawmakase_inference::SUBJECT.id,
                rawmakase_inference::SUBJECT.version,
            ),
            Self::Sky => (
                rawmakase_inference::SKY.id,
                rawmakase_inference::SKY.version,
            ),
            Self::Denoise => (
                rawmakase_inference::RAWNIND.id,
                rawmakase_inference::RAWNIND.version,
            ),
        };
        crate::storage::local_data_dir()
            .join("models")
            .join(format!("{id}-v{version}"))
    }
    /// Whether every file is in place and verified.
    pub(crate) fn is_installed(self) -> bool {
        is_installed(&self.dir(), self)
    }
    /// Where the pack is installed (whether or not it is).
    pub(crate) fn installed_dir(self) -> PathBuf {
        self.dir()
    }
    /// The megabytes the drawer quotes.
    pub(crate) fn megabytes(self) -> u64 {
        self.bytes().div_ceil(1_000_000)
    }
}

/// Where the app looks for a runtime library it did not come with.
pub(crate) fn runtime_dir() -> PathBuf {
    crate::storage::local_data_dir().join("runtime")
}
/// Installed: every file there with its size, and the receipt the installer wrote
/// after checking each one's SHA-256 says so for exactly these files.
fn is_installed(dir: &Path, pack: Pack) -> bool {
    pack.files().iter().all(|f| has(dir, f))
        && std::fs::read_to_string(dir.join(RECEIPT)).is_ok_and(|r| r == receipt(pack))
}
/// The file recording which files were checked, and against which digests.
const RECEIPT: &str = "verified.txt";
/// The file an install holds locked while it runs.
const LOCK: &str = ".install.lock";
fn receipt(pack: Pack) -> String {
    pack.files()
        .iter()
        .map(|f| format!("{} {} {}\n", f.sha256, f.size_bytes, f.name))
        .collect()
}
/// The SHA-256 of the file at `path`, read in chunks.
fn digest_of(path: &Path) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut sha = Sha256::new();
    let mut buffer = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buffer).ok()?;
        if n == 0 {
            break;
        }
        sha.update(&buffer[..n]);
    }
    Some(sha.finalize().iter().map(|b| format!("{b:02x}")).collect())
}
fn has(dir: &Path, file: &ModelFile) -> bool {
    std::fs::metadata(dir.join(file.name)).is_ok_and(|m| m.is_file() && m.len() == file.size_bytes)
}
/// Fetches or copies each file of the model into a temporary file beside its final
/// place, checks it and publishes it by renaming. `import` is a folder holding the
/// files. A file already in place is kept if its SHA-256 matches, so a retry fetches
/// only what is missing, and a download that stopped part way is kept to be resumed,
/// in this session or a later one; the receipt is written once every file has been
/// checked. One install at a time across running instances: see [`lock`].
pub(crate) fn install(
    pack: Pack,
    import: Option<&Path>,
    progress: &Progress,
) -> Result<(), InstallError> {
    let dir = pack.dir();
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let _lock = lock(&dir, progress)?;
    remove_abandoned(&dir);
    // The receipt goes first: until it is written again, nothing counts as installed.
    let _ = std::fs::remove_file(dir.join(RECEIPT));
    // Bytes of files already there count as done.
    let mut finished = 0u64;
    for file in &pack.files() {
        let kept =
            has(&dir, file) && digest_of(&dir.join(file.name)).as_deref() == Some(file.sha256);
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        if kept {
            finished += file.size_bytes;
            progress.done.store(finished, Ordering::Relaxed);
            continue;
        }
        let part = dir.join(format!(".{}.part", file.name));
        let result = (|| {
            match import {
                Some(folder) => {
                    let mut out = std::fs::File::options()
                        .write(true)
                        .create(true)
                        .truncate(true)
                        .open(&part)
                        .map_err(|e| format!("could not write {}: {e}", part.display()))?;
                    let path = folder.join(file.name);
                    let source = std::fs::File::open(&path)
                        .map_err(|e| format!("could not read {}: {e}", path.display()))?;
                    if copy_checked(source, &mut out, progress, file, finished)? != file.sha256 {
                        return Err(InstallError::Failed(format!(
                            "{} is not the file this release uses",
                            file.name
                        )));
                    }
                    out.sync_all().map_err(|e| e.to_string())?;
                }
                // Checked and synced, or kept to be resumed.
                None => download(&part, progress, file, finished, BODY_BUDGET)?,
            }
            // Cancelled while the last bytes were written out: not published.
            if progress.cancel.load(Ordering::Relaxed) {
                return Err(InstallError::Cancelled);
            }
            let target = dir.join(file.name);
            // Windows will not rename over a file; only a damaged one can be there.
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&part, &target).map_err(|e| InstallError::Failed(e.to_string()))
        })();
        if result.is_err() && import.is_some() {
            let _ = std::fs::remove_file(&part);
        }
        result?;
        finished += file.size_bytes;
        progress.done.store(finished, Ordering::Relaxed);
    }
    if progress.cancel.load(Ordering::Relaxed) {
        return Err(InstallError::Cancelled);
    }
    crate::storage::write_atomic(
        &dir.join(RECEIPT),
        crate::storage::Replace::Overwrite,
        |out| {
            out.write_all(receipt(pack).as_bytes())?;
            Ok(())
        },
    )
    .map_err(|e| format!("could not record the install: {e}"))?;
    let _ = crate::storage::sync_dir(&dir);
    Ok(())
}

/// Locks the folder's lock file for one install, waiting while another window's
/// install holds it, noticing a cancel meanwhile. That install's files are then checked
/// like any others, so this one finishes once they are there. Unlocked when dropped.
fn lock(dir: &Path, progress: &Progress) -> Result<std::fs::File, InstallError> {
    let lock = std::fs::File::options()
        .write(true)
        .create(true)
        .truncate(false)
        .open(dir.join(LOCK))
        .map_err(|e| format!("could not write in {}: {e}", dir.display()))?;
    loop {
        match lock.try_lock() {
            Ok(()) => return Ok(lock),
            Err(std::fs::TryLockError::WouldBlock) => {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(InstallError::Cancelled);
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => {
                return Err(InstallError::Failed(e.to_string()));
            }
        }
    }
}

/// Removes partial files an earlier version's crashed install left, never one a running
/// instance owns: they are named for the process that made them. Today's partial
/// files are kept to be resumed.
fn remove_abandoned(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let ours = format!(".part-{}", std::process::id());
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let old = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if name.starts_with('.') && name.contains(".part-") && !name.ends_with(&ours) && old {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Copies `file` from `source` and gives its SHA-256. Stops at cancellation, and at
/// one byte too many. `before` is the bytes of earlier files, for the shared progress.
fn copy_checked(
    source: impl Read,
    out: &mut impl Write,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<String, InstallError> {
    let mut sha = Sha256::new();
    let mut total = 0u64;
    append(source, out, &mut sha, &mut total, progress, file, before)?;
    if total != file.size_bytes {
        return Err(InstallError::Failed(format!(
            "{} is smaller than expected",
            file.name
        )));
    }
    Ok(hex(sha))
}

/// Copies from `source` until it ends, adding what is written to `sha` and `total` as
/// it goes, so the bytes written before an error count. At most `file`'s size.
fn append(
    mut source: impl Read,
    out: &mut impl Write,
    sha: &mut Sha256,
    total: &mut u64,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<(), InstallError> {
    let mut buffer = vec![0u8; 256 << 10];
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        let n = source.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if *total + n as u64 > file.size_bytes {
            return Err(InstallError::Failed(format!(
                "{} is larger than expected",
                file.name
            )));
        }
        out.write_all(&buffer[..n]).map_err(|e| match e.kind() {
            std::io::ErrorKind::StorageFull => InstallError::DiskFull,
            _ => InstallError::Failed(e.to_string()),
        })?;
        sha.update(&buffer[..n]);
        *total += n as u64;
        progress.done.store(before + *total, Ordering::Relaxed);
    }
    Ok(())
}

fn hex(sha: Sha256) -> String {
    sha.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A download in progress: its file, and how many bytes it holds and their SHA-256.
struct Partial {
    out: std::fs::File,
    len: u64,
    sha: Sha256,
}

impl Partial {
    /// Opens the partial file at `path`, keeping the bytes an earlier attempt left in
    /// it, up to `file`'s size. Reading them back stops at a cancel, keeping them.
    fn open(
        path: &Path,
        file: &ModelFile,
        progress: &Progress,
        before: u64,
    ) -> Result<Self, InstallError> {
        let mut out = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .map_err(|e| format!("could not write {}: {e}", path.display()))?;
        let mut partial = Self {
            out: out.try_clone().map_err(|e| e.to_string())?,
            len: 0,
            sha: Sha256::new(),
        };
        // Too long to be a beginning of this file, or unreadable: start over.
        if out.metadata().is_ok_and(|m| m.len() <= file.size_bytes) {
            let (sha, len) = (&mut partial.sha, &mut partial.len);
            match append(
                &mut out,
                &mut std::io::sink(),
                sha,
                len,
                progress,
                file,
                before,
            ) {
                Ok(()) => {}
                Err(InstallError::Cancelled) => return Err(InstallError::Cancelled),
                Err(_) => partial.len = 0,
            }
        }
        partial.rewind()?;
        Ok(partial)
    }
    /// Cuts the file back to the bytes counted, which are the only ones written in
    /// full, and goes on from there: a write that failed part way may have left more.
    fn rewind(&mut self) -> Result<(), InstallError> {
        use std::io::{Seek, SeekFrom};
        if self.len == 0 {
            self.sha = Sha256::new();
        }
        self.out.set_len(self.len).map_err(|e| e.to_string())?;
        self.out
            .seek(SeekFrom::Start(self.len))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    /// Empties the file to fetch it from the start.
    fn restart(&mut self) -> Result<(), InstallError> {
        self.len = 0;
        self.rewind()
    }
}

/// How long one request may spend receiving its body. The budget is for the whole of
/// it, not each read: it ends a request a minute in, keeping what arrived, so a stalled
/// connection is noticed and the next request carries on from there.
const BODY_BUDGET: Duration = Duration::from_secs(60);

/// Fetches `file` into the partial file at `part`, resuming what it already holds,
/// from RAWmakase's mirror, else from the repository it was copied from; a source
/// whose bytes do not match the checksum is not used. On success the file is
/// complete, checked and synced; otherwise it keeps what arrived.
fn download(
    part: &Path,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
    body: Duration,
) -> Result<(), InstallError> {
    let agent = agent(body);
    let mut partial = Partial::open(part, file, progress, before)?;
    let mut last = String::new();
    let mut sources = [file.url, file.fallback].into_iter().peekable();
    while let Some(&url) = sources.peek() {
        let resumed = partial.len > 0;
        match fetch(&agent, url, &mut partial, progress, file, before) {
            Ok(()) if hex(partial.sha.clone()) == file.sha256 => {
                return partial
                    .out
                    .sync_all()
                    .map_err(|e| InstallError::Failed(e.to_string()));
            }
            Ok(()) => {
                last = format!("the download of {} does not match its checksum", file.name);
                partial.restart()?;
                // The bytes kept from before may be the wrong ones: this source again,
                // from the start, before the next.
                if resumed {
                    continue;
                }
            }
            Err(e @ (InstallError::Cancelled | InstallError::DiskFull)) => return Err(e),
            Err(e) => last = e.to_string(),
        }
        sources.next();
    }
    Err(InstallError::Failed(format!(
        "could not download the model: {last}"
    )))
}

fn agent(body: Duration) -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(20)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(body))
        .timeout_global(Some(body + Duration::from_secs(60)))
        .build()
        .into()
}

/// A tenth of the pause before a source is asked again; short in tests.
const BACKOFF_TICK: Duration = Duration::from_millis(if cfg!(test) { 1 } else { 100 });

/// Fetches the rest of `file` from one source, one request after another; it gives
/// up after three in a row bring it no further than it has been, and at a client error.
/// Further, not more: a host ignoring ranges starts over each time.
fn fetch(
    agent: &ureq::Agent,
    url: &str,
    partial: &mut Partial,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<(), InstallError> {
    let mut last = format!("the download of {} stopped", file.name);
    let mut failures = 0u32;
    let mut furthest = partial.len;
    while partial.len < file.size_bytes {
        if failures == 3 {
            return Err(InstallError::Failed(last));
        }
        if failures > 0 {
            // Back off, a second more each time, noticing a cancel meanwhile.
            for _ in 0..(10 * failures) {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err(InstallError::Cancelled);
                }
                std::thread::sleep(BACKOFF_TICK);
            }
        }
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(InstallError::Cancelled);
        }
        match request(agent, url, partial, progress, file, before) {
            Ok(()) => {}
            Err(Failed::Final(e)) => return Err(e),
            Err(Failed::Retry(e)) => last = e,
        }
        if partial.len > furthest {
            furthest = partial.len;
            failures = 0;
        } else {
            failures += 1;
        }
    }
    Ok(())
}

/// Why a request failed: worth another, or not (a cancel, a full disk, a client error).
enum Failed {
    Retry(String),
    Final(InstallError),
}

impl From<InstallError> for Failed {
    fn from(why: InstallError) -> Self {
        match why {
            InstallError::Failed(why) => Self::Retry(why),
            stop => Self::Final(stop),
        }
    }
}

impl From<String> for Failed {
    fn from(why: String) -> Self {
        InstallError::Failed(why).into()
    }
}

/// One request, for the bytes after those `partial` holds; a host sending the whole
/// file instead is read from the start.
fn request(
    agent: &ureq::Agent,
    url: &str,
    partial: &mut Partial,
    progress: &Progress,
    file: &ModelFile,
    before: u64,
) -> Result<(), Failed> {
    partial.rewind()?;
    let mut get = agent.get(url);
    if partial.len > 0 {
        get = get.header("Range", format!("bytes={}-", partial.len));
    }
    let mut response = match get.call() {
        Ok(response) => response,
        Err(ureq::Error::StatusCode(code)) if (400..500).contains(&code) && code != 429 => {
            return Err(Failed::Final(InstallError::Failed(format!(
                "the host answered {code} for {}",
                file.name
            ))));
        }
        Err(e) => return Err(e.to_string().into()),
    };
    match response.status().as_u16() {
        206 => {
            let rest = response
                .headers()
                .get("content-range")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| {
                    v.starts_with(&format!("bytes {}-", partial.len))
                        && v.ends_with(&format!("/{}", file.size_bytes))
                });
            if !rest {
                partial.restart()?;
                return Err(InstallError::Failed(format!(
                    "the host sent the wrong part of {}",
                    file.name
                ))
                .into());
            }
        }
        200 => partial.restart()?,
        code => {
            return Err(InstallError::Failed(format!(
                "the host answered {code} for {}",
                file.name
            ))
            .into());
        }
    }
    let reader = response.body_mut().as_reader();
    let Partial { out, len, sha } = partial;
    append(reader, out, sha, len, progress, file, before)?;
    Ok(())
}

#[cfg(test)]
mod tests;
