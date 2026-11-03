//! Installing the selection models: downloaded from where the release pins them (or,
//! in tests, copied from a folder). Nothing starts without the user's choice, and
//! nothing is published until its size and SHA-256 match the manifest.
use super::super::task::Stopping;
use super::super::worker::Event;
use eframe::egui;
use rawmakase_inference::ModelFile;
use rawmakase_inference::manifest::{all_files, denoise_files, sky_files};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc::Sender,
};
use std::time::Duration;

/// How much of the model has been fetched, and whether to stop.
#[derive(Default)]
struct Progress {
    done: AtomicU64,
    cancel: AtomicBool,
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
    fn bytes(self) -> u64 {
        self.files().iter().map(|f| f.size_bytes).sum()
    }
    /// Where it is kept: under this computer's own data folder, in a folder named for the
    /// model and its contract version, so a replacement model sits beside it.
    pub(super) fn dir(self) -> PathBuf {
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
    pub(in crate::app) fn is_installed(self) -> bool {
        is_installed(&self.dir(), self)
    }
    /// Where the pack is installed (whether or not it is).
    pub(in crate::app) fn installed_dir(self) -> PathBuf {
        self.dir()
    }
    /// The megabytes the drawer quotes.
    pub(in crate::app) fn megabytes(self) -> u64 {
        self.bytes().div_ceil(1_000_000)
    }
}

/// The installs of each pack.
pub(crate) struct Models {
    pub(in crate::app) subject: Installer,
    pub(in crate::app) sky: Installer,
    pub(in crate::app) denoise: Installer,
}
impl Default for Models {
    fn default() -> Self {
        Self {
            subject: Installer::default(),
            sky: Installer {
                pack: Pack::Sky,
                ..Installer::default()
            },
            denoise: Installer {
                pack: Pack::Denoise,
                ..Installer::default()
            },
        }
    }
}
impl Models {
    pub(in crate::app) fn of(&self, pack: Pack) -> &Installer {
        match pack {
            Pack::Subject => &self.subject,
            Pack::Sky => &self.sky,
            Pack::Denoise => &self.denoise,
        }
    }
    pub(in crate::app) fn of_mut(&mut self, pack: Pack) -> &mut Installer {
        match pack {
            Pack::Subject => &mut self.subject,
            Pack::Sky => &mut self.sky,
            Pack::Denoise => &mut self.denoise,
        }
    }
}

/// One pack on disk and the install in progress, if any.
#[derive(Default)]
pub(crate) struct Installer {
    pack: Pack,
    /// What the folder held when last looked at, and when: the files on disk decide
    /// whether the models are installed (a copy made, or deleted, by hand counts), not
    /// a note of what this process did.
    seen: std::cell::Cell<Option<(std::time::Instant, bool)>>,
    progress: Option<Arc<Progress>>,
    thread: Option<std::thread::JoinHandle<()>>,
    removing: bool,
    /// Why the last install failed, until the next one starts.
    failed: Option<String>,
}

/// Where the app looks for a runtime library it did not come with.
pub(in crate::app) fn runtime_dir() -> PathBuf {
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

impl Installer {
    pub(in crate::app) fn installed(&self) -> bool {
        if self.removing {
            return false;
        }
        // Looked at again after a second, so the drawer does not stat on every frame.
        match self.seen.get() {
            Some((at, installed)) if at.elapsed() < Duration::from_secs(1) => installed,
            _ => {
                let installed = is_installed(&self.pack.dir(), self.pack);
                self.seen.set(Some((std::time::Instant::now(), installed)));
                installed
            }
        }
    }
    fn forget(&self) {
        self.seen.set(None);
    }
    /// The folder holding the model's files.
    pub(in crate::app) fn path(&self) -> Option<PathBuf> {
        self.installed().then(|| self.pack.dir())
    }
    pub(in crate::app) fn busy(&self) -> bool {
        self.progress.is_some() || self.removing
    }
    /// Bytes fetched so far and the total, while installing.
    pub(in crate::app) fn progress(&self) -> Option<(u64, u64)> {
        self.progress
            .as_ref()
            .map(|p| (p.done.load(Ordering::Relaxed), self.pack.bytes()))
    }
    pub(in crate::app) fn cancel(&mut self) {
        if let Some(p) = &self.progress {
            p.cancel.store(true, Ordering::Relaxed);
        }
    }
    /// Why the last install failed, if it did and was not cancelled.
    pub(in crate::app) fn failure(&self) -> Option<&str> {
        self.failed.as_deref()
    }
    /// Called when the install's event arrives.
    pub(in crate::app) fn finished(&mut self, result: &Result<(), String>) {
        self.progress = None;
        self.thread = None;
        self.failed = result.clone().err().filter(|e| e != "Cancelled");
        self.forget();
    }
    /// Starts fetching the model, or copying `import`; one install at a time.
    pub(in crate::app) fn install(
        &mut self,
        import: Option<PathBuf>,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || self.installed() {
            return;
        }
        let pack = self.pack;
        let progress = Arc::new(Progress::default());
        self.progress = Some(progress.clone());
        self.failed = None;
        let spawned = std::thread::Builder::new()
            .name("model-install".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    install(pack, import.as_deref(), &progress)
                }))
                .unwrap_or_else(|_| Err("the install stopped unexpectedly".into()));
                let _ = tx.send(Event::ModelInstalled(pack, result));
                ctx.request_repaint();
            });
        match spawned {
            Ok(handle) => self.thread = Some(handle),
            Err(e) => {
                self.progress = None;
                eprintln!("Could not start the model install: {e}");
            }
        }
    }
    /// Cancels an install and hands its thread to the shared shutdown deadline.
    pub(in crate::app) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        vec![Stopping::new(self.thread.take())]
    }
    /// Removes the model from disk: the worker lets go of it first, off the UI
    /// thread, and the files go once it has. Saved masks keep their pixels.
    pub(in crate::app) fn remove(
        &mut self,
        unload: impl FnOnce() + Send + 'static,
        tx: Sender<Event>,
        ctx: egui::Context,
    ) {
        if self.busy() || !self.installed() {
            return;
        }
        self.removing = true;
        let pack = self.pack;
        std::thread::spawn(move || {
            unload();
            let result = std::fs::remove_dir_all(pack.dir())
                .or_else(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        Ok(())
                    } else {
                        Err(e)
                    }
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(Event::ModelRemoved(pack, result));
            ctx.request_repaint();
        });
    }
    pub(in crate::app) fn removed(&mut self) {
        self.removing = false;
        self.forget();
    }
}

/// Fetches or copies each file of the model into a temporary file beside its final
/// place, checks it and publishes it by renaming. `import` is a folder holding the
/// files. A file already in place is kept if its SHA-256 matches, so a retry fetches
/// only what is missing, and a download that stopped part way is kept to be resumed,
/// in this session or a later one; the receipt is written once every file has been
/// checked. One install at a time across running instances: see [`lock`].
fn install(pack: Pack, import: Option<&Path>, progress: &Progress) -> Result<(), String> {
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
            return Err("Cancelled".into());
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
                        return Err(format!("{} is not the file this release uses", file.name));
                    }
                    out.sync_all().map_err(|e| e.to_string())?;
                }
                // Checked and synced, or kept to be resumed.
                None => download(&part, progress, file, finished, BODY_BUDGET)?,
            }
            // Cancelled while the last bytes were written out: not published.
            if progress.cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".to_string());
            }
            let target = dir.join(file.name);
            // Windows will not rename over a file; only a damaged one can be there.
            let _ = std::fs::remove_file(&target);
            std::fs::rename(&part, &target).map_err(|e| e.to_string())
        })();
        if result.is_err() && import.is_some() {
            let _ = std::fs::remove_file(&part);
        }
        result?;
        finished += file.size_bytes;
        progress.done.store(finished, Ordering::Relaxed);
    }
    if progress.cancel.load(Ordering::Relaxed) {
        return Err("Cancelled".into());
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
fn lock(dir: &Path, progress: &Progress) -> Result<std::fs::File, String> {
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
                    return Err("Cancelled".into());
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(std::fs::TryLockError::Error(e)) => return Err(e.to_string()),
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
) -> Result<String, String> {
    let mut sha = Sha256::new();
    let mut total = 0u64;
    append(source, out, &mut sha, &mut total, progress, file, before)?;
    if total != file.size_bytes {
        return Err(format!("{} is smaller than expected", file.name));
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
) -> Result<(), String> {
    let mut buffer = vec![0u8; 256 << 10];
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let n = source.read(&mut buffer).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        if *total + n as u64 > file.size_bytes {
            return Err(format!("{} is larger than expected", file.name));
        }
        out.write_all(&buffer[..n]).map_err(|e| match e.kind() {
            std::io::ErrorKind::StorageFull => "the disk is full".to_string(),
            _ => e.to_string(),
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
    ) -> Result<Self, String> {
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
                Err(e) if e == "Cancelled" => return Err(e),
                Err(_) => partial.len = 0,
            }
        }
        partial.rewind()?;
        Ok(partial)
    }
    /// Cuts the file back to the bytes counted, which are the only ones written in
    /// full, and goes on from there: a write that failed part way may have left more.
    fn rewind(&mut self) -> Result<(), String> {
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
    fn restart(&mut self) -> Result<(), String> {
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
) -> Result<(), String> {
    let agent = agent(body);
    let mut partial = Partial::open(part, file, progress, before)?;
    let mut last = String::new();
    let mut sources = [file.url, file.fallback].into_iter().peekable();
    while let Some(&url) = sources.peek() {
        let resumed = partial.len > 0;
        match fetch(&agent, url, &mut partial, progress, file, before) {
            Ok(()) if hex(partial.sha.clone()) == file.sha256 => {
                return partial.out.sync_all().map_err(|e| e.to_string());
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
            Err(e) if e == "Cancelled" || e.contains("disk is full") => return Err(e),
            Err(e) => last = e,
        }
        sources.next();
    }
    Err(format!("could not download the model: {last}"))
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
) -> Result<(), String> {
    let mut last = format!("the download of {} stopped", file.name);
    let mut failures = 0u32;
    let mut furthest = partial.len;
    while partial.len < file.size_bytes {
        if failures == 3 {
            return Err(last);
        }
        if failures > 0 {
            // Back off, a second more each time, noticing a cancel meanwhile.
            for _ in 0..(10 * failures) {
                if progress.cancel.load(Ordering::Relaxed) {
                    return Err("Cancelled".into());
                }
                std::thread::sleep(BACKOFF_TICK);
            }
        }
        if progress.cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
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
    Final(String),
}

impl From<String> for Failed {
    fn from(why: String) -> Self {
        if why == "Cancelled" || why.contains("disk is full") {
            Self::Final(why)
        } else {
            Self::Retry(why)
        }
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
            return Err(Failed::Final(format!(
                "the host answered {code} for {}",
                file.name
            )));
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
                return Err(format!("the host sent the wrong part of {}", file.name).into());
            }
        }
        200 => partial.restart()?,
        code => return Err(format!("the host answered {code} for {}", file.name).into()),
    }
    let reader = response.body_mut().as_reader();
    let Partial { out, len, sha } = partial;
    append(reader, out, sha, len, progress, file, before)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: ModelFile = ModelFile {
        name: "x.onnx",
        size_bytes: 3,
        sha256: "",
        url: "",
        fallback: "",
    };

    #[test]
    fn copying_checks_size_and_digest_and_stops_when_asked() {
        let progress = Progress::default();
        // Too short, then too long.
        assert!(copy_checked(&b"ab"[..], &mut Vec::new(), &progress, &FILE, 0).is_err());
        assert!(copy_checked(&b"abcd"[..], &mut Vec::new(), &progress, &FILE, 0).is_err());
        // The right bytes give their SHA-256, and progress counts earlier files too.
        let digest = copy_checked(&b"abc"[..], &mut Vec::new(), &progress, &FILE, 10).unwrap();
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(progress.done.load(Ordering::Relaxed), 13);
        // Cancelled before the first chunk.
        progress.cancel.store(true, Ordering::Relaxed);
        let error = copy_checked(&b"abc"[..], &mut Vec::new(), &progress, &FILE, 0).unwrap_err();
        assert_eq!(error, "Cancelled");
    }

    /// How the test host answers one request.
    enum Answer {
        /// The whole file, closing after `send` bytes.
        Whole { send: usize },
        /// The whole file, sending `send` bytes and then nothing until the client
        /// hangs up.
        Stall { send: usize },
        /// The rest of the file from where the request's range starts.
        Rest,
    }

    const LEN: usize = 100_000;

    /// The bytes the test host serves.
    fn body() -> Vec<u8> {
        (0..LEN).map(|i| (i * 7 % 251) as u8).collect()
    }

    /// Serves `body()` on a local port, one connection per request, answering each
    /// as told. Gives the file as the manifest describes it, both sources pointing at
    /// the host, and where each request asked to start, sent before it is answered so
    /// it has arrived by the time the client has its answer.
    ///
    /// The host is never joined: if the client stops early, its test fails on what
    /// the host saw instead of waiting in `accept`. Reads time out for the same
    /// reason, and the thread ends with the test process.
    fn host(answers: Vec<Answer>) -> (ModelFile, std::sync::mpsc::Receiver<Option<u64>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/x.onnx", listener.local_addr().unwrap());
        let (asked, requests) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let body = body();
            for answer in answers {
                let Ok((stream, _)) = listener.accept() else {
                    return;
                };
                // A client that hung up mid-answer still gets the next one.
                let _ = answer_one(stream, &body, answer, &asked);
            }
        });
        let file = ModelFile {
            name: "x.onnx",
            size_bytes: LEN as u64,
            sha256: Box::leak(hex(Sha256::new_with_prefix(body())).into_boxed_str()),
            url: Box::leak(url.into_boxed_str()),
            fallback: "",
        };
        (
            ModelFile {
                fallback: file.url,
                ..file
            },
            requests,
        )
    }

    fn answer_one(
        mut stream: std::net::TcpStream,
        body: &[u8],
        answer: Answer,
        asked: &std::sync::mpsc::Sender<Option<u64>>,
    ) -> std::io::Result<()> {
        use std::io::BufRead;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let mut from = None;
        let mut reader = std::io::BufReader::new(stream.try_clone()?);
        loop {
            let mut line = String::new();
            reader.read_line(&mut line)?;
            if line.trim().is_empty() {
                break;
            }
            if let Some(range) = line.to_ascii_lowercase().strip_prefix("range: bytes=") {
                from = range.trim().trim_end_matches('-').parse::<usize>().ok();
            }
        }
        let _ = asked.send(from.map(|f| f as u64));
        let len = body.len();
        match (answer, from) {
            (Answer::Whole { send }, _) => {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n")?;
                stream.write_all(&body[..send])
            }
            (Answer::Stall { send }, _) => {
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {len}\r\n\r\n")?;
                stream.write_all(&body[..send])?;
                // Until the client gives up on the body and closes.
                std::io::copy(&mut reader, &mut std::io::sink()).map(|_| ())
            }
            (Answer::Rest, Some(from)) => {
                write!(
                    stream,
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {from}-{}/{len}\r\n\
                     Content-Length: {}\r\n\r\n",
                    len - 1,
                    len - from
                )?;
                stream.write_all(&body[from..])
            }
            (Answer::Rest, None) => {
                stream.write_all(b"HTTP/1.1 500 No range\r\nContent-Length: 0\r\n\r\n")
            }
        }
    }

    /// The partial file's place, holding `held` from an earlier attempt.
    fn partial(dir: &tempfile::TempDir, held: &[u8]) -> PathBuf {
        let part = dir.path().join(".x.onnx.part");
        std::fs::write(&part, held).unwrap();
        part
    }

    #[test]
    fn a_dropped_download_resumes_where_it_stopped() {
        let (file, requests) = host(vec![Answer::Whole { send: 40_000 }, Answer::Rest]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &[]);
        let progress = Progress::default();
        assert_eq!(download(&part, &progress, &file, 0, BODY_BUDGET), Ok(()));
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(progress.done.load(Ordering::Relaxed), LEN as u64);
        // The second request asked for the rest only.
        assert_eq!(
            requests.try_iter().collect::<Vec<_>>(),
            [None, Some(40_000)]
        );
    }

    /// The 0.2.2 bug: a body longer in coming than its budget started over every
    /// time, so on a slow connection a large file never arrived.
    #[test]
    fn a_download_slower_than_its_budget_keeps_what_arrived() {
        let (file, requests) = host(vec![Answer::Stall { send: 30_000 }, Answer::Rest]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &[]);
        let budget = Duration::from_millis(200);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, budget),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(
            requests.try_iter().collect::<Vec<_>>(),
            [None, Some(30_000)]
        );
    }

    /// A client giving up before it has read what was sent resets the connection;
    /// on a slow machine `download` does that at its budget. The host must still
    /// answer the next request rather than refuse it.
    #[test]
    fn the_host_answers_after_a_client_resets_the_connection() {
        let (file, requests) = host(vec![Answer::Stall { send: 30_000 }, Answer::Rest]);
        let address = file
            .url
            .trim_start_matches("http://")
            .trim_end_matches("/x.onnx");
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream.write_all(b"GET /x.onnx HTTP/1.1\r\n\r\n").unwrap();
        stream.read_exact(&mut [0u8; 4]).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        // Unread bytes left behind: closing resets the connection.
        drop(stream);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &body()[..60_000]);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
            Ok(())
        );
        assert_eq!(
            requests.try_iter().collect::<Vec<_>>(),
            [None, Some(60_000)]
        );
    }

    #[test]
    fn a_download_left_by_an_earlier_session_is_resumed() {
        let (file, requests) = host(vec![Answer::Rest]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &body()[..60_000]);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(requests.try_iter().collect::<Vec<_>>(), [Some(60_000)]);
    }

    #[test]
    fn a_host_ignoring_the_range_and_dropping_gets_no_further_and_is_left() {
        // Each answer starts over, ending at 40 or 60 KB: never past 60 KB, so three
        // requests after reaching it the source is given up, and so is the fallback.
        let answers = (0..10).map(|i| Answer::Whole {
            send: if i % 2 == 0 { 40_000 } else { 60_000 },
        });
        let (mut file, requests) = host(answers.collect());
        file.fallback = host(vec![]).0.url;
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &[]);
        assert!(download(&part, &Progress::default(), &file, 0, BODY_BUDGET).is_err());
        assert_eq!(requests.try_iter().count(), 5);
        // What it got is kept for the next try.
        assert_eq!(std::fs::read(&part).unwrap(), &body()[..40_000]);
    }

    #[test]
    fn a_host_ignoring_the_range_is_read_from_the_start() {
        let (file, requests) = host(vec![Answer::Whole { send: LEN }]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &body()[..60_000]);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(requests.try_iter().collect::<Vec<_>>(), [Some(60_000)]);
    }

    #[test]
    fn wrong_bytes_kept_from_before_are_fetched_again() {
        // The resumed file fails its checksum, and the other source starts over.
        let (file, requests) = host(vec![Answer::Rest, Answer::Whole { send: LEN }]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &[0; 60_000]);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(
            requests.try_iter().collect::<Vec<_>>(),
            [Some(60_000), None]
        );
    }

    #[test]
    fn a_partial_file_longer_than_the_file_is_not_resumed() {
        let (file, requests) = host(vec![Answer::Whole { send: LEN }]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &[0; LEN + 1]);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(requests.try_iter().collect::<Vec<_>>(), [None]);
    }

    #[test]
    fn a_cancelled_download_keeps_what_arrived() {
        let (file, requests) = host(vec![]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &body()[..60_000]);
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            download(&part, &progress, &file, 0, BODY_BUDGET),
            Err("Cancelled".into())
        );
        assert_eq!(std::fs::read(&part).unwrap(), &body()[..60_000]);
        // Nothing is asked of the host once cancelled.
        assert_eq!(requests.try_iter().count(), 0);
    }

    #[test]
    fn reading_kept_bytes_back_stops_at_a_cancel_and_keeps_them() {
        let (file, _) = host(vec![]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &body()[..60_000]);
        let progress = Progress::default();
        progress.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            Partial::open(&part, &file, &progress, 0).err().as_deref(),
            Some("Cancelled")
        );
        assert_eq!(std::fs::read(&part).unwrap(), &body()[..60_000]);
    }

    #[test]
    fn bytes_left_by_a_failed_write_are_cut_off_before_the_next_request() {
        let (file, requests) = host(vec![Answer::Rest]);
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &body()[..60_000]);
        let progress = Progress::default();
        let mut partial = Partial::open(&part, &file, &progress, 0).unwrap();
        // Written but never counted, as by a write that failed part way.
        partial.out.write_all(b"stray").unwrap();
        let agent = agent(BODY_BUDGET);
        assert_eq!(
            fetch(&agent, file.url, &mut partial, &progress, &file, 0),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(requests.try_iter().collect::<Vec<_>>(), [Some(60_000)]);
    }

    #[test]
    fn wrong_bytes_kept_from_before_are_fetched_again_from_the_same_source() {
        // The whole file is there but wrong: no request can show it, so the checksum
        // does, and the mirror is asked again rather than the fallback, which is down.
        let (mut file, requests) = host(vec![Answer::Whole { send: LEN }]);
        file.fallback = host(vec![]).0.url;
        let dir = tempfile::tempdir().unwrap();
        let part = partial(&dir, &[0; LEN]);
        assert_eq!(
            download(&part, &Progress::default(), &file, 0, BODY_BUDGET),
            Ok(())
        );
        assert_eq!(std::fs::read(&part).unwrap(), body());
        assert_eq!(requests.try_iter().collect::<Vec<_>>(), [None]);
    }

    #[test]
    fn an_install_waits_for_another_windows_install() {
        let dir = tempfile::tempdir().unwrap();
        let other = lock(dir.path(), &Progress::default()).unwrap();
        // Waiting, not failing: only a cancel ends the wait.
        let cancelled = Progress::default();
        cancelled.cancel.store(true, Ordering::Relaxed);
        assert_eq!(
            lock(dir.path(), &cancelled).err().as_deref(),
            Some("Cancelled")
        );
        // And once the other install ends, this one goes on.
        let path = dir.path().to_owned();
        let waiting = std::thread::spawn(move || lock(&path, &Progress::default()).is_ok());
        drop(other);
        assert!(waiting.join().unwrap());
    }

    #[test]
    fn a_failed_install_says_why_until_the_next_one() {
        let mut installer = Installer::default();
        installer.finished(&Err("could not download the model: timed out".into()));
        assert_eq!(
            installer.failure(),
            Some("could not download the model: timed out")
        );
        installer.finished(&Err("Cancelled".into()));
        assert_eq!(installer.failure(), None);
    }

    #[test]
    fn each_pack_has_a_folder_named_for_its_model_and_version() {
        assert!(Pack::Subject.dir().ends_with(format!(
            "models/{}-v{}",
            rawmakase_inference::SUBJECT.id,
            rawmakase_inference::SUBJECT.version
        )));
        assert!(Pack::Sky.dir().ends_with(format!(
            "models/{}-v{}",
            rawmakase_inference::SKY.id,
            rawmakase_inference::SKY.version
        )));
        assert!(Pack::Subject.megabytes() > 250 && Pack::Sky.megabytes() > 200);
    }

    /// A folder holding `pack`'s files at their sizes, with its receipt.
    fn fake_install(pack: Pack) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for file in pack.files() {
            let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
            f.set_len(file.size_bytes).unwrap();
        }
        std::fs::write(dir.path().join(RECEIPT), receipt(pack)).unwrap();
        dir
    }

    /// What an install from before Sky had its own model holds: it must keep working after
    /// the update, without fetching anything.
    #[test]
    fn subjects_models_stay_installed_without_the_sky_model() {
        let dir = fake_install(Pack::Subject);
        assert!(is_installed(dir.path(), Pack::Subject));
        assert!(!is_installed(dir.path(), Pack::Sky));
        let receipt_of_the_old_install = [
            "aacf1f7137bb6fffcf6bf166abcfabe28f57a76059254f3fb611c4a64a208119 467440 vision_encoder.onnx",
            "260fd1f0a34e72a3dc79a739e563b4facc0ba75504818b433a1f808e66637456 162476288 vision_encoder.onnx_data",
            "079c59b261f723ff5c6a125e69b0170a957b21c58738c28d2b0394ecd0587d7f 213114 prompt_encoder_mask_decoder.onnx",
            "f9e59a584ab8ced21fa812c211bc01084204db1c9e92a5ef4fb3a49972b4e864 20958208 prompt_encoder_mask_decoder.onnx_data",
            "afd9f02d864302d690356fd4bfcb2feed2397a1190bf46a7306cb430464d734a 86559030 detr-panoptic-fp16.onnx",
            "",
        ]
        .join("\n");
        assert_eq!(receipt(Pack::Subject), receipt_of_the_old_install);
        let sky = fake_install(Pack::Sky);
        assert!(is_installed(sky.path(), Pack::Sky));
        assert!(!is_installed(sky.path(), Pack::Subject));
    }

    #[test]
    fn a_model_is_installed_only_when_every_file_is_there_with_its_size_and_checked() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_installed(dir.path(), Pack::Subject));
        for file in all_files() {
            let f = std::fs::File::create(dir.path().join(file.name)).unwrap();
            f.set_len(file.size_bytes).unwrap();
        }
        // The right sizes are not enough without the installer's receipt.
        assert!(!is_installed(dir.path(), Pack::Subject));
        std::fs::write(dir.path().join(RECEIPT), receipt(Pack::Subject)).unwrap();
        assert!(is_installed(dir.path(), Pack::Subject));
        assert_eq!(
            digest_of(&dir.path().join(all_files()[0].name)).map(|d| d.len()),
            Some(64)
        );
        std::fs::File::create(dir.path().join(all_files()[1].name))
            .unwrap()
            .set_len(5)
            .unwrap();
        assert!(!is_installed(dir.path(), Pack::Subject));
    }

    /// AI Denoise's model is installed only with its licence and credit beside it: a
    /// folder without `LICENSE` or `NOTICE`, receipt or not, is not an install.
    #[test]
    fn the_denoiser_needs_its_licence_and_notice() {
        let dir = tempfile::tempdir().unwrap();
        for file in Pack::Denoise.files() {
            std::fs::File::create(dir.path().join(file.name))
                .unwrap()
                .set_len(file.size_bytes)
                .unwrap();
        }
        std::fs::write(dir.path().join(RECEIPT), receipt(Pack::Denoise)).unwrap();
        assert!(is_installed(dir.path(), Pack::Denoise));
        for name in ["LICENSE", "NOTICE"] {
            let path = dir.path().join(name);
            let kept = std::fs::read(&path).unwrap();
            std::fs::remove_file(&path).unwrap();
            assert!(!is_installed(dir.path(), Pack::Denoise), "{name}");
            std::fs::write(&path, kept).unwrap();
        }
        assert!(receipt(Pack::Denoise).contains(" LICENSE\n"));
        assert!(receipt(Pack::Denoise).contains(" NOTICE\n"));
        // Its own folder: none of the selection models' files.
        assert_ne!(Pack::Denoise.dir(), Pack::Subject.dir());
    }
}
