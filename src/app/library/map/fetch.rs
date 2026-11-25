//! Fetching map tiles off the UI thread, as OpenStreetMap's tile usage policy
//! asks: only the tiles a map shows now, by two workers, with RAWmakase's own
//! User-Agent, kept on disk as long as the server says (a week when it says
//! nothing) and asked for again only if they changed.
//!
//! Workers start with the first tile asked for. A request not asked for again
//! within [`WANTED_FOR`] is dropped unfetched, so a map scrolled out of sight
//! or turned off stops asking. Closing (or dropping) the fetcher ends the
//! workers once their current tile is done; [`Fetcher::close`] hands them to
//! the exit deadline. A tile's request is bounded by [`TIMEOUT`].
use super::tiles::{self, Meta, Template, TileId};
use eframe::egui;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

const WORKERS: usize = 2;
/// How long a tile stays asked for without being asked again.
pub(super) const WANTED_FOR: Duration = Duration::from_millis(750);
const TIMEOUT: Duration = Duration::from_secs(20);
/// The largest tile read; tiles are some tens of kilobytes.
const LARGEST: u64 = 4 << 20;
/// The tiles kept on disk are trimmed to three quarters of this, oldest
/// first, when a fetcher starts above it.
const KEPT_BYTES: u64 = 256 << 20;

/// A tile fetched, or why it could not be.
pub(super) struct Done {
    pub tile: TileId,
    pub template: Arc<Template>,
    pub image: Result<egui::ColorImage, String>,
}

#[derive(Default)]
struct Queue {
    /// Tiles asked for and not taken yet, the first first.
    wanted: Vec<TileId>,
    template: Option<Arc<Template>>,
    asked: Option<Instant>,
    /// Tiles being fetched.
    busy: HashSet<TileId>,
    closed: bool,
}
struct Shared {
    queue: Mutex<Queue>,
    wake: Condvar,
}

/// How the workers reach the server.
#[derive(Clone)]
pub(super) struct Net {
    /// Whether plain HTTP is refused; only tests against a local server allow it.
    pub https_only: bool,
}
impl Default for Net {
    fn default() -> Self {
        Self { https_only: true }
    }
}

pub(super) struct Fetcher {
    shared: Arc<Shared>,
    tx: Sender<Done>,
    rx: Receiver<Done>,
    threads: Vec<JoinHandle<()>>,
    /// The folder every server's tiles are kept under.
    root: PathBuf,
    net: Net,
    ctx: egui::Context,
}
impl Fetcher {
    pub(super) fn new(root: PathBuf, net: Net, ctx: egui::Context) -> Self {
        let (tx, rx) = channel();
        Self {
            shared: Arc::new(Shared {
                queue: Mutex::new(Queue::default()),
                wake: Condvar::new(),
            }),
            tx,
            rx,
            threads: Vec::new(),
            root,
            net,
            ctx,
        }
    }
    /// Asks for `tiles` of `template`'s server, the first first, in place of
    /// what was asked before; tiles being fetched are not asked for twice.
    pub(super) fn want(&mut self, template: &Arc<Template>, tiles: Vec<TileId>) {
        {
            let mut q = lock(&self.shared.queue);
            if q.closed {
                return;
            }
            q.wanted = tiles.into_iter().filter(|t| !q.busy.contains(t)).collect();
            q.template = Some(template.clone());
            q.asked = Some(Instant::now());
        }
        if self.threads.is_empty() {
            self.start();
        }
        self.shared.wake.notify_all();
    }
    /// Tiles fetched since the last call.
    pub(super) fn done(&self) -> Vec<Done> {
        self.rx.try_iter().collect()
    }
    fn start(&mut self) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .https_only(self.net.https_only)
            .max_redirects(3)
            .timeout_connect(Some(Duration::from_secs(10)))
            .timeout_global(Some(TIMEOUT))
            .http_status_as_error(false)
            .user_agent(user_agent())
            .build()
            .into();
        for i in 0..WORKERS {
            let (shared, tx, root, ctx, agent) = (
                self.shared.clone(),
                self.tx.clone(),
                self.root.clone(),
                self.ctx.clone(),
                agent.clone(),
            );
            let spawned = std::thread::Builder::new()
                .name(format!("map-tiles-{i}"))
                .spawn(move || {
                    if i == 0 {
                        trim(&root, KEPT_BYTES);
                    }
                    work(&shared, &tx, &root, &agent, &ctx);
                });
            match spawned {
                Ok(handle) => self.threads.push(handle),
                Err(e) => eprintln!("Could not start fetching map tiles: {e}"),
            }
        }
    }
    /// Ends the workers once their current tile is done, and hands them to the
    /// exit deadline; nothing more is fetched.
    pub(super) fn close(&mut self) -> Vec<crate::app::task::Stopping> {
        self.shut();
        self.threads
            .drain(..)
            .map(|t| crate::app::task::Stopping::new(Some(t)))
            .collect()
    }
    fn shut(&self) {
        let mut q = lock(&self.shared.queue);
        q.closed = true;
        q.wanted.clear();
        drop(q);
        self.shared.wake.notify_all();
    }
}
impl Drop for Fetcher {
    /// A Library closed for another catalog doesn't wait for its workers: they
    /// end after their current tile.
    fn drop(&mut self) {
        self.shut();
    }
}

/// RAWmakase's name and version, and where to find it, as OpenStreetMap asks
/// every application to send.
pub(super) fn user_agent() -> String {
    format!(
        "RAWmakase/{} (+https://github.com/pch/rawmakase)",
        env!("CARGO_PKG_VERSION")
    )
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn work(shared: &Shared, tx: &Sender<Done>, root: &Path, agent: &ureq::Agent, ctx: &egui::Context) {
    loop {
        let (tile, template) = {
            let mut q = lock(&shared.queue);
            loop {
                if q.closed {
                    return;
                }
                if q.asked.is_some_and(|at| at.elapsed() > WANTED_FOR) {
                    q.wanted.clear();
                }
                if let (false, Some(template)) = (q.wanted.is_empty(), q.template.clone()) {
                    let tile = q.wanted.remove(0);
                    q.busy.insert(tile);
                    break (tile, template);
                }
                q = shared
                    .wake
                    .wait_timeout(q, Duration::from_secs(1))
                    .unwrap_or_else(|e| e.into_inner())
                    .0;
            }
        };
        let folder = template.folder(root);
        let image = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load(agent, &template, &folder, tile, SystemTime::now())
        }))
        .unwrap_or_else(|_| Err("fetching the tile stopped unexpectedly".into()));
        lock(&shared.queue).busy.remove(&tile);
        if tx
            .send(Done {
                tile,
                template,
                image,
            })
            .is_err()
        {
            return;
        }
        ctx.request_repaint();
    }
}

/// The tile from disk while it is fresh; otherwise from the server, asking only
/// whether it changed when one is kept. A kept tile is used when the server
/// can't be reached.
fn load(
    agent: &ureq::Agent,
    template: &Template,
    folder: &Path,
    tile: TileId,
    now: SystemTime,
) -> Result<egui::ColorImage, String> {
    let (file, meta_file) = tiles::paths(folder, tile);
    let kept = std::fs::read(&file).ok();
    let meta: Option<Meta> = std::fs::read(&meta_file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let (Some(bytes), Some(meta)) = (&kept, &meta)
        && meta.fresh(now)
        && let Ok(image) = decode(bytes)
    {
        return Ok(image);
    }
    let mut request = agent.get(template.url(tile));
    if kept.is_some() {
        if let Some(etag) = meta.as_ref().and_then(|m| m.etag.as_deref()) {
            request = request.header("If-None-Match", etag);
        }
        if let Some(at) = meta.as_ref().and_then(|m| m.last_modified.as_deref()) {
            request = request.header("If-Modified-Since", at);
        }
    }
    let fetched = request
        .call()
        .map_err(|e| e.to_string())
        .and_then(|mut response| {
            let header = |name: &str| {
                response
                    .headers()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let fresh = Meta::of(
                now,
                header("cache-control").as_deref(),
                header("expires").as_deref(),
                header("etag").as_deref(),
                header("last-modified").as_deref(),
            );
            match response.status().as_u16() {
                200 => {
                    let bytes = response
                        .body_mut()
                        .with_config()
                        .limit(LARGEST)
                        .read_to_vec()
                        .map_err(|e| e.to_string())?;
                    Ok((Some(bytes), fresh))
                }
                304 if kept.is_some() => {
                    // Unchanged: what the server says now, keeping a validator it
                    // left out this time.
                    let old = meta.clone().unwrap_or_default();
                    Ok((
                        None,
                        Meta {
                            etag: fresh.etag.or(old.etag),
                            last_modified: fresh.last_modified.or(old.last_modified),
                            ..fresh
                        },
                    ))
                }
                status => Err(format!("the map server answered {status}")),
            }
        });
    match fetched {
        Ok((Some(bytes), meta)) => {
            let image = decode(&bytes)?;
            keep(&file, &bytes, &meta_file, &meta);
            Ok(image)
        }
        Ok((None, meta)) => {
            let bytes = kept.ok_or("the tile kept is gone")?;
            let image = decode(&bytes)?;
            keep_meta(&meta_file, &meta);
            Ok(image)
        }
        Err(e) => kept.as_deref().map(decode).unwrap_or(Err(e)),
    }
}

fn decode(bytes: &[u8]) -> Result<egui::ColorImage, String> {
    let image = image::load_from_memory(bytes)
        .map_err(|e| format!("the tile is not an image: {e}"))?
        .into_rgba8();
    let size = [image.width() as usize, image.height() as usize];
    Ok(egui::ColorImage::from_rgba_unmultiplied(
        size,
        image.as_raw(),
    ))
}

/// Writes a tile and its facts, each through a temporary file renamed into
/// place, so a tile half written is never read. They are not synced: a tile
/// lost in a crash is fetched again. A tile that can't be kept is still shown.
fn keep(file: &Path, bytes: &[u8], meta_file: &Path, meta: &Meta) {
    if let Some(dir) = file.parent()
        && std::fs::create_dir_all(dir).is_ok()
        && replace(file, bytes).is_ok()
    {
        keep_meta(meta_file, meta);
    }
}
fn keep_meta(meta_file: &Path, meta: &Meta) {
    if let Ok(json) = serde_json::to_vec(meta) {
        let _ = replace(meta_file, &json);
    }
}
fn replace(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut file = tempfile::NamedTempFile::new_in(path.parent().unwrap_or(Path::new(".")))?;
    file.write_all(bytes)?;
    file.persist(path).map(drop).map_err(|e| e.error)
}

/// Deletes the tiles fetched longest ago until three quarters of `limit` are
/// left, when the tiles under `root` take more than `limit` bytes.
pub(super) fn trim(root: &Path, limit: u64) {
    let mut found = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "tile") {
                let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                found.push((used, meta.len(), path));
            }
        }
    }
    let mut total: u64 = found.iter().map(|f| f.1).sum();
    if total <= limit {
        return;
    }
    found.sort();
    for (_, size, path) in found {
        if total <= limit / 4 * 3 {
            break;
        }
        if std::fs::remove_file(&path).is_ok() {
            let _ = std::fs::remove_file(path.with_extension("json"));
            total -= size;
        }
    }
}

#[cfg(test)]
#[path = "fetch_tests.rs"]
mod tests;
