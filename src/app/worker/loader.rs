use super::latest::Mailbox;
use super::{Event, Latest, LoadJob, LoadedHeader, Prefetch, TaskKind, send};
use crate::{
    camera_data::{CameraImage, Decode},
    decode::{DecodePolicy, FullSize},
    decode_cache::DecodeCache,
    export_settings::ExportOptions,
    photo::Opened,
    raw::thumbnail,
};
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, atomic::Ordering, mpsc::Sender},
    time::Instant,
};
/// The slow second stage of opening a photo, run on its own worker so the
/// next photo's quick stage never waits behind it.
struct FullJob {
    id: u64,
    path: std::path::PathBuf,
    /// The file as read before its first decode, carried with every image of it.
    file: Option<crate::storage::Identity>,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
    full: FullSize,
}
/// What a failed load says: why Develop refuses the file, or the error.
fn load_error(path: &std::path::Path, e: &anyhow::Error) -> String {
    crate::app::library::Refusal::opening(path, e).map_or_else(|| format!("{e:#}"), |r| r.detail())
}
/// Errs when the file at `path` is no longer `file`, the version read before it
/// was opened: what was read or decoded since may be of another file. Without
/// one there is nothing to check (and nothing is decode-cached).
fn unchanged(path: &Path, file: Option<&crate::storage::Identity>) -> anyhow::Result<()> {
    if let Some(file) = file {
        anyhow::ensure!(
            crate::storage::Identity::read(path).ok().as_ref() == Some(file),
            "The file changed while it was opening; open it again"
        );
    }
    Ok(())
}
/// Runs `load`, turning a panic into an error, so the photo does not stay loading.
fn caught(load: impl FnOnce() -> anyhow::Result<()>) -> anyhow::Result<()> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(load)).unwrap_or_else(|panic| {
        Err(anyhow::anyhow!(
            "Loading failed: {}",
            super::panic_message(&*panic)
        ))
    })
}
/// A JPEG, PNG or TIFF neighbour opened ahead of time. Rendered files are never
/// decode-cached (opening one is its whole decode), so the one prefetched is held
/// here until the next load takes it, whichever photo that load is for.
#[derive(Clone, Default)]
pub(super) struct Ahead(Arc<Mutex<Option<Held>>>);
struct Held {
    path: PathBuf,
    /// The file as it was opened (size, time and content); a file replaced since
    /// opens afresh.
    identity: crate::storage::Identity,
    image: CameraImage,
}
impl Ahead {
    fn held(&self) -> std::sync::MutexGuard<'_, Option<Held>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    /// Opens `path` and holds it, dropping what was held first. A cancelled
    /// prefetch keeps nothing: the photo it was for has been moved past or opened.
    fn prefetch(&self, path: &Path, cancel: Cancel<'_>) {
        *self.held() = None;
        let Ok(identity) = crate::storage::Identity::read(path) else {
            return;
        };
        let Ok(Opened::Rendered(image)) = crate::photo::open_source(path, cancel) else {
            return;
        };
        if !cancel.is_cancelled() {
            *self.held() = Some(Held {
                path: path.into(),
                identity,
                image,
            });
        }
    }
    /// What was prefetched, if it is `path` as `file` (read by the load) names
    /// it and as the file is now. Empties the slot either way, so a prefetch is
    /// never held past the next load.
    fn take(&self, path: &Path, file: Option<&crate::storage::Identity>) -> Option<CameraImage> {
        let held = self.held().take()?;
        (held.path == path
            && file == Some(&held.identity)
            && crate::storage::Identity::read(path).ok() == Some(held.identity))
        .then_some(held.image)
    }
}

/// Gives back the large blocks macOS's allocator keeps of what the loader's
/// stages freed ([`crate::platform::memory::release_freed`]), where the next
/// stage is about to allocate planes of other sizes: a kept block is handed out
/// again only for a request near its own size, so those planes would come on
/// top of it (measurements in `docs/records/preview-performance-measurements.md`).
///
/// It does nothing elsewhere. On Linux, glibc reuses an arena's free space, and
/// its `malloc_trim` takes 1 to 100 ms, which would hold up the image this
/// loader is about to send.
fn release_kept_blocks() {
    #[cfg(target_os = "macos")]
    crate::platform::memory::release_freed();
}

/// Develops a neighbour on two threads, leaving the other cores to rendering the
/// photo on screen: a RAW into the decode cache, a JPEG, PNG or TIFF into `ahead`.
fn prefetcher(cache: DecodeCache, ahead: Ahead) -> Latest<Prefetch> {
    Latest::new(move |job: Prefetch| {
        static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
        let Some(pool) = POOL.get_or_init(|| crate::raw::background_pool(2, "prefetch").ok())
        else {
            return;
        };
        if job.cancel.load(Ordering::Relaxed) {
            return;
        }
        if !crate::photo::source(&job.path).is_raw() {
            // What the open photo's first render freed goes before the neighbour
            // is opened, as before a RAW's decode below.
            release_kept_blocks();
            pool.install(|| ahead.prefetch(&job.path, Cancel::new(&job.cancel)));
            return;
        }
        let full = FullSize::with_cache(&job.path, job.demosaic, cache.clone());
        if full.is_cached() {
            return;
        }
        // The open photo's first full-size render has just ended (the app starts
        // the prefetch after it), and the allocator still holds the planes its
        // noise reduction freed: they go before this decode allocates its own.
        release_kept_blocks();
        let open = || crate::photo::open(&job.path);
        let _ = pool.install(|| full.get(open, DecodePolicy::Prefetch, Cancel::new(&job.cancel)));
    })
}
/// Runs as a photo's full-size stage starts and once it has opened the file;
/// tests stand in a file replaced there. Nothing in the app sets one.
#[derive(Clone, Default)]
struct Between(Option<Hook>);
type Hook = Arc<dyn Fn(&Path, FullStage) + Send + Sync>;
/// Where in the full-size stage [`Between`] runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullStage {
    Start,
    Opened,
}
impl Between {
    fn run(&self, path: &Path, stage: FullStage) {
        if let Some(hook) = &self.0 {
            hook(path, stage);
        }
    }
}
fn full_loader(tx: Sender<Event>, ctx: egui::Context, between: Between) -> Latest<FullJob> {
    Latest::new(move |job: FullJob| {
        let result = caught(|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            between.run(&job.path, FullStage::Start);
            {
                // Checked before and after: the decoder keeps what it read, so a file
                // put back between the two would pass the second check alone.
                unchanged(&job.path, job.file.as_ref())?;
                let raw = crate::photo::open(&job.path)?;
                between.run(&job.path, FullStage::Opened);
                let image = Arc::new(job.full.decode(
                    raw,
                    DecodePolicy::Show,
                    Cancel::new(&job.cancel),
                )?);
                if job.cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                // Opened again: shown and stored only if it is still the version the
                // first stage read.
                unchanged(&job.path, job.file.as_ref())?;
                // The half-size stage and this decode are done with their planes,
                // which the allocator still holds: they go before the first
                // full-size render allocates its own.
                release_kept_blocks();
                send(
                    &tx,
                    &ctx,
                    Event::Ready {
                        id: job.id,
                        full: image.clone(),
                        file: job.file.clone(),
                        status: format!("Developed in {:.2}s", job.started.elapsed().as_secs_f32()),
                    },
                );
                job.full.store(&image, Cancel::new(&job.cancel));
            }
            Ok(())
        });
        if let Err(e) = result
            && !job.cancel.load(Ordering::Relaxed)
        {
            send(
                &tx,
                &ctx,
                Event::Failed {
                    id: job.id,
                    task: TaskKind::Load,
                    error: load_error(&job.path, &e),
                },
            );
        }
    })
}
/// Opens photos for Develop: the quick first stage and the full-size decode, each
/// on a worker of its own, and on a third the neighbour's prefetch, which the
/// app starts ([`Loader::prefetch`]).
pub(crate) struct Loader {
    open: Latest<LoadJob>,
    full: Latest<FullJob>,
    prefetch: Latest<Prefetch>,
}
impl Loader {
    pub(crate) fn new(tx: Sender<Event>, ctx: egui::Context, cache: DecodeCache) -> Self {
        Self::with_parts(tx, ctx, cache, Ahead::default(), Between::default())
    }
    /// A loader whose full-size decodes are kept in `cache`, whose prefetched
    /// rendered files in `ahead`, running `between` before each full-size stage.
    fn with_parts(
        tx: Sender<Event>,
        ctx: egui::Context,
        cache: DecodeCache,
        ahead: Ahead,
        between: Between,
    ) -> Self {
        let prefetch = prefetcher(cache.clone(), ahead.clone());
        let full = full_loader(tx.clone(), ctx.clone(), between);
        let open = opener(tx, ctx, cache, ahead, full.mailbox());
        Self {
            open,
            full,
            prefetch,
        }
    }
    pub(crate) fn submit(&self, job: LoadJob) {
        self.open.submit(job);
    }
    /// Develops a neighbour ahead of time, in place of any prefetch not started
    /// yet. The app asks once the open photo's first full-size render is done
    /// ([`super::NeighbourPrefetch`]): started as the photo's full-size image
    /// arrived, as it used to be, the neighbour's decode ran beside that
    /// render's noise reduction, the two largest bursts of an open at once.
    pub(crate) fn prefetch(&self, job: Prefetch) {
        self.prefetch.submit(job);
    }
    /// Stops all three workers once their current jobs are done.
    pub(in crate::app) fn stop(&mut self) -> [crate::app::task::Stopping; 3] {
        [self.open.stop(), self.full.stop(), self.prefetch.stop()]
    }
}
/// The camera's embedded preview as Develop's placeholder shows it: some cameras
/// embed a full-size JPEG (60 MP on a Sony A7CR), and a screen-sized copy is all
/// the placeholder needs.
fn placeholder(im: image::RgbImage) -> image::RgbImage {
    let edge = im.width().max(im.height());
    if edge <= 2560 {
        return im;
    }
    let k = 2560. / edge as f32;
    image::imageops::resize(
        &im,
        (im.width() as f32 * k) as u32,
        (im.height() as f32 * k) as u32,
        image::imageops::FilterType::Triangle,
    )
}
fn opener(
    tx: Sender<Event>,
    ctx: egui::Context,
    cache: DecodeCache,
    ahead: Ahead,
    full: Mailbox<FullJob>,
) -> Latest<LoadJob> {
    Latest::new(move |job: LoadJob| {
        if let Some(source) = job.from.editable_preview().cloned() {
            // Final as it is: no embedded preview or full-size stage follows it.
            let result = caught(|| open_editable_preview(&job, &source, &tx, &ctx));
            if let Err(e) = result
                && !job.cancel.load(Ordering::Relaxed)
            {
                send(
                    &tx,
                    &ctx,
                    Event::Failed {
                        id: job.id,
                        task: TaskKind::Load,
                        error: format!("{e:#}"),
                    },
                );
            }
            return;
        }
        let result = caught(|| -> anyhow::Result<()> {
            let path = job.path.clone();
            let started = Instant::now();
            // Read before anything is decoded: every image sent below is of this file or,
            // if it is replaced meanwhile, of one that no longer matches (AI Denoise then
            // refuses rather than mixing the two).
            let file = crate::storage::Identity::read(&path).ok();
            let prefetched = ahead.take(&path, file.as_ref());
            let was_ahead = prefetched.is_some();
            // On a network volume the camera's preview is read alone and shown
            // first, rather than once the whole file has arrived.
            let mut embedded_sent = false;
            if prefetched.is_none()
                && crate::storage::is_raw(&path)
                && crate::platform::preview_reads::previews_only(&path)
                && let Ok(im) = crate::raw::largest_thumbnail_from_file(&path)
                // As every image sent: of this file, not one replaced meanwhile.
                && unchanged(&path, file.as_ref()).is_ok()
            {
                send(
                    &tx,
                    &ctx,
                    Event::Embedded {
                        id: job.id,
                        image: placeholder(im),
                    },
                );
                embedded_sent = true;
            }
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let opened = match prefetched {
                Some(image) => Opened::Rendered(image),
                None => crate::photo::open_source(&path, Cancel::new(&job.cancel))?,
            };
            unchanged(&path, file.as_ref())?;
            let metadata = opened.metadata().clone();
            let crate::camera_profiles::Listed {
                profiles,
                errors,
                groups,
            } = crate::camera_profiles::listed(&metadata);
            // A photo without color data starts from its fitted camera look, fitted
            // (or read from the fit cache) before its defaults are installed.
            let profiles = crate::camera_look::with_default(
                &path,
                &metadata,
                profiles,
                Cancel::new(&job.cancel),
            );
            // The raw defaults; the catalog's edit replaces them once the header
            // is installed.
            let recipe = crate::edits::Start::Defaults(job.defaults.clone())
                .resolve(&metadata, &profiles)?
                .recipe;
            send(
                &tx,
                &ctx,
                Event::Header(Box::new(LoadedHeader {
                    id: job.id,
                    path: path.clone(),
                    from: super::OpenedFrom::Original,
                    file: file.clone(),
                    metadata,
                    recipe,
                    export: ExportOptions::default(),
                    status: "Original".into(),
                })),
            );
            send(
                &tx,
                &ctx,
                Event::Profiles {
                    id: job.id,
                    profiles,
                    errors,
                    groups,
                    imported: false,
                },
            );
            let mut raw = match opened {
                Opened::Raw(raw) => raw,
                // A JPEG, PNG or TIFF opens at full size at once: no embedded
                // preview, half-size stage or decode cache to go through.
                Opened::Rendered(image) => {
                    if job.cancel.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    send(
                        &tx,
                        &ctx,
                        Event::Ready {
                            id: job.id,
                            full: Arc::new(image),
                            file,
                            status: if was_ahead {
                                "Opened ahead of time".into()
                            } else {
                                format!("Opened in {:.2}s", started.elapsed().as_secs_f32())
                            },
                        },
                    );
                    return Ok(());
                }
            };
            if !embedded_sent && let Ok(im) = thumbnail(&mut raw) {
                send(
                    &tx,
                    &ctx,
                    Event::Embedded {
                        id: job.id,
                        image: placeholder(im),
                    },
                );
            }
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let t = Instant::now();
            let full_size = FullSize::of_version(file.as_ref(), job.demosaic, cache.clone());
            if let Some(image) = full_size.cached(&raw.metadata) {
                // Closed before the photo is ready: Windows refuses to rename or
                // replace a file that is still open.
                drop(raw);
                send(
                    &tx,
                    &ctx,
                    Event::Ready {
                        id: job.id,
                        full: Arc::new(image),
                        file,
                        status: format!("Opened from cache in {:.2}s", t.elapsed().as_secs_f32()),
                    },
                );
                return Ok(());
            }
            // Lightroom-style two stages: a half-size decode (about 0.2 s) makes
            // the photo editable at once; the full-resolution decode, which can
            // take seconds, then replaces it for 100% views and export.
            let quick = Arc::new(raw.develop(Decode::Half, Cancel::new(&job.cancel))?);
            unchanged(&path, file.as_ref())?;
            send(
                &tx,
                &ctx,
                Event::Ready {
                    id: job.id,
                    full: quick,
                    file: file.clone(),
                    status: "Loading full resolution…".into(),
                },
            );
            full.submit(FullJob {
                id: job.id,
                path,
                file,
                cancel: job.cancel.clone(),
                started: t,
                full: full_size,
            });
            Ok(())
        });
        if let Err(e) = result
            && !job.cancel.load(Ordering::Relaxed)
        {
            send(
                &tx,
                &ctx,
                Event::Failed {
                    id: job.id,
                    task: TaskKind::Load,
                    error: load_error(&job.path, &e),
                },
            );
        }
    })
}

/// Opens `job`'s photo from its editable preview `source`, through `job.copies`:
/// the reduced image with the original's facts, and the default Camera Look the
/// copy keeps. Every image, and the edit, are the photo's as `source.original`
/// names it: the catalog's row says which original the copy is of, and the
/// copy's file is named after that original's identity
/// ([`editable_preview_file_name`](crate::catalog::editable_preview_file_name)),
/// so whatever file is at `source.file` is a copy of it. A copy discarded or
/// rebuilt for another file since the Library chose it is no longer there, and
/// fails to open.
fn open_editable_preview(
    job: &LoadJob,
    source: &super::EditablePreviewSource,
    tx: &Sender<Event>,
    ctx: &egui::Context,
) -> anyhow::Result<()> {
    let started = Instant::now();
    // The copy's own file, read before it is opened, as for an original: what
    // the image is of, under which Develop's thumbnail of it is kept.
    let copy_file = crate::storage::Identity::read(&source.file).ok();
    let opened = job
        .copies
        .open(&source.file, Cancel::new(&job.cancel))
        .map_err(|e| {
            anyhow::anyhow!(
                "Its editable preview can't be read ({e:#}); build it again while the original is online"
            )
        })?;
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    let metadata = opened.image.metadata.clone();
    let crate::camera_profiles::Listed {
        mut profiles,
        errors,
        groups,
    } = crate::camera_profiles::listed(&metadata);
    profiles.extend(opened.default_look.map(Arc::new));
    let recipe = crate::edits::Start::Defaults(job.defaults.clone())
        .resolve(&metadata, &profiles)?
        .recipe;
    send(
        tx,
        ctx,
        Event::Header(Box::new(LoadedHeader {
            id: job.id,
            path: job.path.clone(),
            from: job.from.clone(),
            file: Some(source.original.clone()),
            metadata,
            recipe,
            export: ExportOptions::default(),
            status: super::OpenedFrom::EDITABLE_PREVIEW.into(),
        })),
    );
    send(
        tx,
        ctx,
        Event::Profiles {
            id: job.id,
            profiles,
            errors,
            groups,
            imported: false,
        },
    );
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    send(
        tx,
        ctx,
        Event::Ready {
            id: job.id,
            full: Arc::new(opened.image),
            file: copy_file,
            status: format!(
                "Opened its editable preview in {:.2}s",
                started.elapsed().as_secs_f32()
            ),
        },
    );
    Ok(())
}

#[cfg(test)]
pub(in crate::app) mod tests {
    use super::*;
    use crate::decode_cache::DecodeCache;
    use std::{sync::mpsc, time::Duration};

    /// Loads `path` on a loader whose decode cache is `cache`, returning every
    /// event it sent once its workers have stopped.
    fn load(path: &std::path::Path, cache: &std::path::Path) -> Vec<Event> {
        load_with(path, cache, Ahead::default())
    }
    /// As [`load`], with `ahead` holding what was prefetched.
    fn load_with(path: &std::path::Path, cache: &std::path::Path, ahead: Ahead) -> Vec<Event> {
        load_between(path, cache, ahead, Between::default())
    }
    /// As [`load_with`], running `between` before the full-size stage.
    fn load_between(
        path: &std::path::Path,
        cache: &std::path::Path,
        ahead: Ahead,
        between: Between,
    ) -> Vec<Event> {
        let (tx, rx) = mpsc::channel();
        let mut loader = Loader::with_parts(
            tx,
            egui::Context::default(),
            DecodeCache::new(cache.into(), u64::MAX),
            ahead,
            between,
        );
        loader.submit(LoadJob {
            id: 7,
            path: path.into(),
            from: Default::default(),
            copies: crate::app::worker::editable_preview::decoder(),
            cancel: Default::default(),
            defaults: Arc::new(crate::raw_defaults::DevelopDefaults::with_presets(
                Default::default(),
                |_| None,
            )),
            demosaic: Default::default(),
        });
        let mut events = Vec::new();
        while let Ok(event) = rx.recv_timeout(Duration::from_secs(30)) {
            let done = matches!(&event, Event::Ready { full, .. } if !full.fast)
                || matches!(event, Event::Failed { .. });
            events.push(event);
            if done {
                break;
            }
        }
        let waited = crate::app::task::wait_for(loader.stop().into(), Duration::from_secs(30));
        assert_eq!(waited.detached, 0);
        events.extend(rx.try_iter());
        events
    }

    fn name(event: &Event) -> &'static str {
        match event {
            Event::Header(_) => "Header",
            Event::Profiles { .. } => "Profiles",
            Event::Embedded { .. } => "Embedded",
            Event::Ready { .. } => "Ready",
            Event::Failed { .. } => "Failed",
            _ => "other",
        }
    }

    #[test]
    fn a_jpeg_loads_straight_to_full_size() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("photo.jpg");
        image::RgbImage::from_fn(40, 30, |x, y| {
            image::Rgb([(x * 6) as u8, (y * 8) as u8, 90])
        })
        .save(&path)?;
        let cache = dir.path().join("decoded");

        let events = load(&path, &cache);

        assert_eq!(
            events.iter().map(name).collect::<Vec<_>>(),
            ["Header", "Profiles", "Ready"]
        );
        let Event::Header(header) = &events[0] else {
            unreachable!()
        };
        assert!(header.metadata.is_rendered());
        assert!(
            header
                .recipe
                .profile
                .as_ref()
                .is_some_and(|p| p.is_embedded())
        );
        let Event::Profiles { profiles, .. } = &events[1] else {
            unreachable!()
        };
        assert_eq!(profiles.len(), 1);
        assert!(profiles[0].is_embedded());
        let Event::Ready { full, .. } = &events[2] else {
            unreachable!()
        };
        assert!(!full.fast);
        assert_eq!((full.width, full.height), (40, 30));
        let cached = std::fs::read_dir(&cache).map_or(0, |entries| entries.count());
        assert_eq!(cached, 0, "a rendered file is never decode-cached");
        Ok(())
    }

    /// A HEIC opens as a JPEG does: at full size at once, with its own
    /// profile, and never decode-cached; where this system decodes HEIC
    /// (macOS, and Linux with libheif).
    #[cfg(not(windows))]
    #[test]
    fn a_heic_loads_straight_to_full_size() -> anyhow::Result<()> {
        if let Err(why) = crate::raster::heif_decoder() {
            // CI's Arch job requires one (see raster's tests).
            assert!(
                std::env::var_os("RAWMAKASE_REQUIRE_HEIF").is_none(),
                "{why}"
            );
            eprintln!("HEIC pixels not tested: {why}");
            return Ok(());
        }
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("IMG_0001.HEIC");
        std::fs::copy(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/formats/grid.heic"),
            &path,
        )?;
        let cache = dir.path().join("decoded");

        let events = load(&path, &cache);

        assert_eq!(
            events.iter().map(name).collect::<Vec<_>>(),
            ["Header", "Profiles", "Ready"]
        );
        let Event::Header(header) = &events[0] else {
            unreachable!()
        };
        assert!(header.metadata.is_rendered());
        let Event::Ready { full, .. } = &events[2] else {
            unreachable!()
        };
        assert_eq!((full.width, full.height), (120, 100));
        let cached = std::fs::read_dir(&cache).map_or(0, |entries| entries.count());
        assert_eq!(cached, 0, "a rendered file is never decode-cached");
        Ok(())
    }

    fn ready(events: &[Event]) -> (&CameraImage, &str) {
        match events.last() {
            Some(Event::Ready { full, status, .. }) => (full, status),
            _ => panic!("{:?}", events.iter().map(name).collect::<Vec<_>>()),
        }
    }

    /// A RAW replaced between its half-size and full-size stages: the full-size
    /// decode, of the file now there, is neither shown nor stored as the photo
    /// opened, and the load says why.
    #[test]
    fn a_raw_replaced_before_its_full_size_stage_is_not_shown_or_cached() -> anyhow::Result<()> {
        let charts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts");
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("photo.dng");
        std::fs::copy(charts.join("synthetic-a.dng"), &path)?;
        let opened = crate::storage::Identity::read(&path)?;
        let cache = dir.path().join("decoded");
        let replacement = charts.join("canon-eos-r7-d65.dng");
        let between = Between(Some(Arc::new(move |path: &Path, stage| {
            if stage == FullStage::Start {
                std::fs::copy(&replacement, path).unwrap();
            }
        })));

        let events = load_between(&path, &cache, Ahead::default(), between);

        let Event::Header(header) = &events[0] else {
            panic!("{:?}", events.iter().map(name).collect::<Vec<_>>())
        };
        assert_eq!(header.file.as_ref(), Some(&opened));
        let model = header.metadata.model.clone();
        for event in &events {
            if let Event::Ready { full, file, .. } = event {
                assert!(
                    full.fast,
                    "the full-size image of the file now there was shown"
                );
                assert_eq!(full.metadata.model, model);
                assert_eq!(file.as_ref(), Some(&opened));
            }
        }
        match events.last() {
            Some(Event::Failed {
                task: TaskKind::Load,
                error,
                ..
            }) => assert!(error.contains("changed"), "{error}"),
            _ => panic!("{:?}", events.iter().map(name).collect::<Vec<_>>()),
        }
        let cached = std::fs::read_dir(&cache).map_or(0, |entries| entries.count());
        assert_eq!(cached, 0, "nothing stored for the photo opened");
        Ok(())
    }

    /// Replaced before its full-size stage and put back once that stage has read
    /// the replacement: the file then matches again, but what was read is not it.
    #[test]
    fn a_raw_put_back_while_its_full_size_stage_reads_is_not_shown() -> anyhow::Result<()> {
        let charts = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts");
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("photo.dng");
        let original = charts.join("synthetic-a.dng");
        std::fs::copy(&original, &path)?;
        let modified = std::fs::metadata(&path)?.modified()?;
        let cache = dir.path().join("decoded");
        let replacement = charts.join("canon-eos-r7-d65.dng");
        let between = Between(Some(Arc::new(move |path: &Path, stage| {
            match stage {
                FullStage::Start => std::fs::copy(&replacement, path),
                FullStage::Opened => std::fs::copy(&original, path),
            }
            .unwrap();
            std::fs::File::options()
                .write(true)
                .open(path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        })));

        let events = load_between(&path, &cache, Ahead::default(), between);

        assert!(
            !events
                .iter()
                .any(|e| matches!(e, Event::Ready { full, .. } if !full.fast)),
            "{:?}",
            events.iter().map(name).collect::<Vec<_>>()
        );
        assert!(matches!(
            events.last(),
            Some(Event::Failed {
                task: TaskKind::Load,
                ..
            })
        ));
        let cached = std::fs::read_dir(&cache).map_or(0, |entries| entries.count());
        assert_eq!(cached, 0, "nothing stored for the photo opened");
        Ok(())
    }

    #[test]
    fn a_prefetched_jpeg_opens_without_decoding_again() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([120, 90, 60])).save(&path)?;
        let ahead = Ahead::default();
        ahead.prefetch(&path, Cancel::NEVER);

        let events = load_with(&path, &dir.path().join("decoded"), ahead.clone());

        let (full, status) = ready(&events);
        assert_eq!((full.width, full.height), (40, 30));
        assert!(status.starts_with("Opened ahead of time"), "{status}");
        // The file it was opened from, as for any other load: what its saves, and
        // its thumbnail in the Library, are kept for.
        let Some(Event::Ready { file, .. }) = events.last() else {
            unreachable!()
        };
        assert_eq!(file.as_ref(), Some(&crate::storage::Identity::read(&path)?));
        assert!(
            ahead
                .take(&path, crate::storage::Identity::read(&path).ok().as_ref())
                .is_none(),
            "taken, not held twice"
        );
        Ok(())
    }

    #[test]
    fn a_rendered_file_replaced_after_its_prefetch_opens_as_it_is_now() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([120, 90, 60])).save(&path)?;
        let ahead = Ahead::default();
        ahead.prefetch(&path, Cancel::NEVER);
        image::RgbImage::from_pixel(20, 10, image::Rgb([20, 30, 40])).save(&path)?;

        let events = load_with(&path, &dir.path().join("decoded"), ahead);

        let (full, status) = ready(&events);
        assert_eq!((full.width, full.height), (20, 10));
        assert!(status.starts_with("Opened in"), "{status}");
        Ok(())
    }

    /// Replaced by a file of the same size, its modification time kept (a restore or
    /// sync can do that): the content tells them apart.
    #[test]
    fn a_rendered_file_replaced_with_its_size_and_time_opens_as_it_is_now() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.tif");
        image::RgbImage::from_pixel(40, 30, image::Rgb([200, 0, 0])).save(&path)?;
        let (len, modified) = {
            let m = std::fs::metadata(&path)?;
            (m.len(), m.modified()?)
        };
        let ahead = Ahead::default();
        ahead.prefetch(&path, Cancel::NEVER);
        image::RgbImage::from_pixel(40, 30, image::Rgb([0, 0, 200])).save(&path)?;
        std::fs::File::options()
            .write(true)
            .open(&path)?
            .set_modified(modified)?;
        let m = std::fs::metadata(&path)?;
        assert_eq!(
            (m.len(), m.modified()?),
            (len, modified),
            "same size and time"
        );

        assert!(
            ahead
                .take(&path, crate::storage::Identity::read(&path).ok().as_ref())
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn a_cancelled_prefetch_keeps_nothing() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.jpg");
        image::RgbImage::new(4, 4).save(&path)?;
        let ahead = Ahead::default();

        ahead.prefetch(
            &path,
            Cancel::new(&std::sync::atomic::AtomicBool::new(true)),
        );

        assert!(
            ahead
                .take(&path, crate::storage::Identity::read(&path).ok().as_ref())
                .is_none()
        );
        Ok(())
    }

    fn chart() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/corpus/charts/synthetic-d65.dng")
    }
    /// A loader caching in `cache`, and what tells whether the chart is there.
    fn prefetching_loader(cache: &Path) -> (Loader, FullSize) {
        let cache = DecodeCache::new(cache.into(), u64::MAX);
        let (tx, _rx) = mpsc::channel();
        let loader = Loader::new(tx, egui::Context::default(), cache.clone());
        (
            loader,
            FullSize::with_cache(&chart(), Default::default(), cache),
        )
    }

    #[test]
    fn a_prefetched_raw_is_in_the_decode_cache() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let (mut loader, cached) = prefetching_loader(dir.path());

        loader.prefetch(Prefetch {
            path: chart(),
            cancel: Default::default(),
            demosaic: Default::default(),
        });

        let started = Instant::now();
        while !cached.is_cached() && started.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(cached.is_cached());
        let waited = crate::app::task::wait_for(loader.stop().into(), Duration::from_secs(30));
        assert_eq!(waited.detached, 0);
        Ok(())
    }

    /// A photo moved past before its neighbour's prefetch began decodes nothing.
    #[test]
    fn a_raw_prefetch_cancelled_before_it_starts_stores_nothing() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let (mut loader, cached) = prefetching_loader(dir.path());

        let cancelled = Arc::new(std::sync::atomic::AtomicBool::new(true));
        loader.prefetch(Prefetch {
            path: chart(),
            cancel: cancelled.clone(),
            demosaic: Default::default(),
        });
        // Until the worker has taken the job and is done with it, which drops
        // the job's hold on its flag: a decode would have stored the chart by then.
        let started = Instant::now();
        while Arc::strong_count(&cancelled) > 1 && started.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(Arc::strong_count(&cancelled), 1);
        assert!(!cached.is_cached());
        let waited = crate::app::task::wait_for(loader.stop().into(), Duration::from_secs(30));
        assert_eq!(waited.detached, 0);
        Ok(())
    }

    /// A minimal ICC profile that says its pixels are CMYK.
    pub(in crate::app) fn cmyk_profile() -> Vec<u8> {
        let mut icc = vec![0u8; 132];
        icc[0..4].copy_from_slice(&132u32.to_be_bytes());
        icc[8..12].copy_from_slice(&0x0240_0000u32.to_be_bytes());
        icc[12..16].copy_from_slice(b"prtr");
        icc[16..20].copy_from_slice(b"CMYK");
        icc[20..24].copy_from_slice(b"Lab ");
        icc[36..40].copy_from_slice(b"acsp");
        // D50, the PCS illuminant.
        for (i, v) in [0x0000_F6D6u32, 0x0001_0000, 0x0000_D32D]
            .iter()
            .enumerate()
        {
            icc[68 + 4 * i..72 + 4 * i].copy_from_slice(&v.to_be_bytes());
        }
        icc
    }

    #[test]
    fn a_rendered_file_develop_cannot_handle_fails_the_load() -> anyhow::Result<()> {
        use image::ImageEncoder;
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cmyk.png");
        let mut encoder = image::codecs::png::PngEncoder::new(std::fs::File::create(&path)?);
        encoder.set_icc_profile(cmyk_profile())?;
        encoder.write_image(&[10; 4 * 3], 2, 2, image::ExtendedColorType::Rgb8)?;

        let events = load(&path, &dir.path().join("decoded"));

        match &events[..] {
            [
                Event::Failed {
                    id: 7,
                    task: TaskKind::Load,
                    error,
                },
            ] => {
                assert_eq!(error, "RAWmakase can't edit CMYK images")
            }
            other => panic!("{:?}", other.iter().map(name).collect::<Vec<_>>()),
        }
        Ok(())
    }
}
