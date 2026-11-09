//! The thumbnail workers: each photo's embedded preview, made by several workers side
//! by side, with the disk cache.
use crate::catalog::preview_cache::{PreviewCache, Stamp};
use eframe::egui;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, SyncSender},
    },
    thread::JoinHandle,
};

pub(in crate::app::library) struct PreviewResult {
    pub path: PathBuf,
    pub image: Option<image::RgbImage>,
    pub cache_error: Option<String>,
    /// No longer shown when a worker came to it: not made, and asked for again
    /// once it is shown.
    pub skipped: bool,
}

/// The files the views showed in the last frame, which the thumbnail workers
/// make; `None` until the views have said, when every one asked for is made.
pub(in crate::app::library) type ShownFiles = Arc<Mutex<Option<HashSet<PathBuf>>>>;

/// Whether a worker makes the thumbnail of `path`: shown in the last frame,
/// or nothing said yet.
fn wanted(shown: &ShownFiles, path: &Path) -> bool {
    shown
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .as_ref()
        .is_none_or(|shown| shown.contains(path))
}

/// A preview worker: where its jobs go, where its results come back, and its thread.
pub(in crate::app::library) type Worker<J, R> = (J, Receiver<R>, JoinHandle<()>);

/// The thumbnail workers, which end once their requests or their results
/// are dropped: a few side by side (see [`thumbnail_threads`]), so one slow
/// file or disk doesn't hold up the rest. They run at normal priority, as the
/// one worker did, so the grid fills before edited previews are rendered.
pub(in crate::app::library) fn spawn(
    cache_path: PathBuf,
    ctx: egui::Context,
    shown: ShownFiles,
) -> Worker<SyncSender<PathBuf>, PreviewResult> {
    spawn_with(cache_path, ctx, super::super::thumbnail, shown)
}
/// Thumbnails made side by side: enough to keep a disk and a few cores busy,
/// leaving the rest to the edited previews and Develop.
fn thumbnail_threads() -> usize {
    std::thread::available_parallelism().map_or(2, |n| (n.get() / 2).clamp(2, 4))
}
pub(super) fn spawn_with(
    cache_path: PathBuf,
    ctx: egui::Context,
    thumbnail: fn(&Path) -> anyhow::Result<image::RgbImage>,
    shown: ShownFiles,
) -> Worker<SyncSender<PathBuf>, PreviewResult> {
    let (tx, rx) = mpsc::sync_channel::<PathBuf>(24);
    let (result_tx, result_rx) = mpsc::sync_channel(24);
    let thread = std::thread::spawn(move || {
        let threads = thumbnail_threads();
        let open = || match PreviewCache::open(&cache_path) {
            Ok(cache) => (Some(cache), None),
            Err(error) => (None, Some(error.to_string())),
        };
        // A connection each. The first is opened here, as the one worker's
        // was, and makes the database; the others on their first request, so
        // a worker that is never asked opens nothing and ends at once.
        let first = open();
        let rx = Mutex::new(rx);
        let work = |opened: Option<(Option<PreviewCache>, Option<String>)>| {
            let mut opened = opened;
            loop {
                let Ok(path) = rx
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .recv()
                else {
                    break;
                };
                // Asked for in a folder the grid has left: not made, so the
                // one shown now isn't kept waiting behind it.
                if !wanted(&shown, &path) {
                    let skipped = PreviewResult {
                        path,
                        image: None,
                        cache_error: None,
                        skipped: true,
                    };
                    if result_tx.send(skipped).is_err() {
                        break;
                    }
                    // The UI clears it from what is pending and asks again
                    // once it is shown, which takes a frame.
                    ctx.request_repaint();
                    continue;
                }
                let (cache, open_error) = opened.get_or_insert_with(open);
                let prepared = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    let mut cache_error = open_error.clone();
                    let cached = cache.as_ref().and_then(|cache| match cache.load(&path) {
                        Ok(image) => image,
                        Err(error) => {
                            cache_error = Some(error.to_string());
                            None
                        }
                    });
                    let image = cached.or_else(|| {
                        let stamp = Stamp::read(&path).ok()?;
                        let image = thumbnail(&path).ok()?;
                        if let Some(cache) = cache.as_mut()
                            && let Err(error) = cache.store(&path, &stamp, &image)
                        {
                            cache_error = Some(error.to_string());
                        }
                        Some(image)
                    });
                    (image, cache_error)
                }));
                // After a panic the preview is unavailable, and the cache, which it may
                // have left mid-write, is opened again for the next one.
                let (image, cache_error) = prepared.unwrap_or_else(|_| {
                    opened = Some(open());
                    (None, None)
                });
                if result_tx
                    .send(PreviewResult {
                        path,
                        image,
                        cache_error,
                        skipped: false,
                    })
                    .is_err()
                {
                    break;
                }
                ctx.request_repaint();
            }
        };
        // One works on this thread, so a worker the system can't start
        // leaves the others; returns once every worker has ended.
        let work = &work;
        std::thread::scope(|scope| {
            for _ in 1..threads {
                let _ = std::thread::Builder::new()
                    .name("thumbnail".into())
                    .spawn_scoped(scope, move || work(None));
            }
            work(Some(first));
        });
    });
    (tx, result_rx, thread)
}
