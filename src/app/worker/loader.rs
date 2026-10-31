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
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Instant,
};
/// The slow second stage of opening a photo, run on its own worker so the
/// next photo's quick stage never waits behind it.
struct FullJob {
    id: u64,
    path: std::path::PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    started: Instant,
    full: FullSize,
    prefetch: Option<Prefetch>,
}
/// What a failed load says: why Develop refuses the file, or the error.
fn load_error(path: &std::path::Path, e: &anyhow::Error) -> String {
    crate::app::library::Refusal::opening(path, e).map_or_else(|| format!("{e:#}"), |r| r.detail())
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
    fn prefetch(&self, path: &Path, cancel: &AtomicBool) {
        *self.held() = None;
        let Ok(identity) = crate::storage::Identity::read(path) else {
            return;
        };
        let Ok(Opened::Rendered(image)) = crate::photo::open_source(path, cancel) else {
            return;
        };
        if !cancel.load(Ordering::Relaxed) {
            *self.held() = Some(Held {
                path: path.into(),
                identity,
                image,
            });
        }
    }
    /// What was prefetched, if it is `path` as the file is now. Empties the slot
    /// either way, so a prefetch is never held past the next load.
    fn take(&self, path: &Path) -> Option<CameraImage> {
        let held = self.held().take()?;
        (held.path == path && crate::storage::Identity::read(path).ok() == Some(held.identity))
            .then_some(held.image)
    }
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
        if !crate::storage::is_raw(&job.path) {
            pool.install(|| ahead.prefetch(&job.path, &job.cancel));
            return;
        }
        let full = FullSize::with_cache(&job.path, job.demosaic, cache.clone());
        let open = || crate::photo::open(&job.path);
        let _ = pool.install(|| full.get(open, DecodePolicy::Prefetch, &job.cancel));
    })
}
fn full_loader(
    tx: Sender<Event>,
    ctx: egui::Context,
    prefetcher: Mailbox<Prefetch>,
) -> Latest<FullJob> {
    Latest::new(move |mut job: FullJob| {
        let prefetch = job.prefetch.take();
        let result = caught(|| -> anyhow::Result<()> {
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            {
                let raw = crate::photo::open(&job.path)?;
                let image = Arc::new(job.full.decode(raw, DecodePolicy::Show, &job.cancel)?);
                if job.cancel.load(Ordering::Relaxed) {
                    return Ok(());
                }
                send(
                    &tx,
                    &ctx,
                    Event::Ready {
                        id: job.id,
                        full: image.clone(),
                        status: format!("Developed in {:.2}s", job.started.elapsed().as_secs_f32()),
                    },
                );
                job.full.store(&image, &job.cancel);
            }
            // Only now, so decoding the neighbour never slows the photo on screen.
            if let Some(prefetch) = prefetch {
                prefetcher.submit(prefetch);
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
/// Opens photos for Develop: the quick first stage, then the full-size decode and a
/// neighbour's prefetch, each on a worker of its own.
pub(crate) struct Loader {
    open: Latest<LoadJob>,
    full: Latest<FullJob>,
    prefetch: Latest<Prefetch>,
}
impl Loader {
    pub(crate) fn new(tx: Sender<Event>, ctx: egui::Context, cache: DecodeCache) -> Self {
        Self::with_parts(tx, ctx, cache, Ahead::default())
    }
    /// A loader whose full-size decodes are kept in `cache` and whose prefetched
    /// rendered files in `ahead`.
    fn with_parts(tx: Sender<Event>, ctx: egui::Context, cache: DecodeCache, ahead: Ahead) -> Self {
        let prefetch = prefetcher(cache.clone(), ahead.clone());
        let full = full_loader(tx.clone(), ctx.clone(), prefetch.mailbox());
        let open = opener(tx, ctx, cache, ahead, full.mailbox(), prefetch.mailbox());
        Self {
            open,
            full,
            prefetch,
        }
    }
    pub(crate) fn submit(&self, job: LoadJob) {
        self.open.submit(job);
    }
    /// Stops all three workers once their current jobs are done.
    pub(in crate::app) fn stop(&mut self) -> [crate::app::task::Stopping; 3] {
        [self.open.stop(), self.full.stop(), self.prefetch.stop()]
    }
}
fn opener(
    tx: Sender<Event>,
    ctx: egui::Context,
    cache: DecodeCache,
    ahead: Ahead,
    full: Mailbox<FullJob>,
    prefetcher: Mailbox<Prefetch>,
) -> Latest<LoadJob> {
    Latest::new(move |mut job: LoadJob| {
        let prefetch = job.prefetch.take();
        let result = caught(|| -> anyhow::Result<()> {
            let path = job.path.clone();
            let started = Instant::now();
            let prefetched = ahead.take(&path);
            let was_ahead = prefetched.is_some();
            let opened = match prefetched {
                Some(image) => Opened::Rendered(image),
                None => crate::photo::open_source(&path, &job.cancel)?,
            };
            let metadata = opened.metadata().clone();
            let crate::camera_profiles::Listed {
                profiles,
                errors,
                groups,
            } = crate::camera_profiles::listed(&metadata);
            // A photo without colour data starts from its fitted camera look, fitted
            // (or read from the fit cache) before its defaults are installed.
            let profiles =
                crate::camera_look::with_default(&path, &metadata, profiles, &job.cancel);
            // The raw defaults; the catalog's edit replaces them once the header
            // is installed.
            let recipe = job.defaults.resolve(&metadata, &profiles).recipe;
            send(
                &tx,
                &ctx,
                Event::Header(Box::new(LoadedHeader {
                    id: job.id,
                    path: path.clone(),
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
                            status: if was_ahead {
                                "Opened ahead of time".into()
                            } else {
                                format!("Opened in {:.2}s", started.elapsed().as_secs_f32())
                            },
                        },
                    );
                    if let Some(prefetch) = prefetch {
                        prefetcher.submit(prefetch);
                    }
                    return Ok(());
                }
            };
            if let Ok(im) = thumbnail(&mut raw) {
                // Some cameras embed a full-size JPEG (60 MP on a Sony A7CR);
                // a screen-sized copy is all the placeholder needs.
                let edge = im.width().max(im.height());
                let im = if edge > 2560 {
                    let k = 2560. / edge as f32;
                    image::imageops::resize(
                        &im,
                        (im.width() as f32 * k) as u32,
                        (im.height() as f32 * k) as u32,
                        image::imageops::FilterType::Triangle,
                    )
                } else {
                    im
                };
                send(
                    &tx,
                    &ctx,
                    Event::Embedded {
                        id: job.id,
                        image: im,
                    },
                );
            }
            if job.cancel.load(Ordering::Relaxed) {
                return Ok(());
            }
            let t = Instant::now();
            let full_size = FullSize::with_cache(&path, job.demosaic, cache.clone());
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
                        status: format!("Opened from cache in {:.2}s", t.elapsed().as_secs_f32()),
                    },
                );
                if let Some(prefetch) = prefetch {
                    prefetcher.submit(prefetch);
                }
                return Ok(());
            }
            // Lightroom-style two stages: a half-size decode (about 0.2 s) makes
            // the photo editable at once; the full-resolution decode, which can
            // take seconds, then replaces it for 100% views and export.
            let quick = Arc::new(raw.develop(Decode::Half, &job.cancel)?);
            send(
                &tx,
                &ctx,
                Event::Ready {
                    id: job.id,
                    full: quick,
                    status: "Loading full resolution…".into(),
                },
            );
            full.submit(FullJob {
                id: job.id,
                path,
                cancel: job.cancel.clone(),
                started: t,
                full: full_size,
                prefetch,
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
        let (tx, rx) = mpsc::channel();
        let mut loader = Loader::with_parts(
            tx,
            egui::Context::default(),
            DecodeCache::new(cache.into(), u64::MAX),
            ahead,
        );
        loader.submit(LoadJob {
            id: 7,
            path: path.into(),
            cancel: Default::default(),
            prefetch: None,
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
    /// profile, and never decode-cached.
    #[cfg(feature = "software-hevc")]
    #[test]
    fn a_heic_loads_straight_to_full_size() -> anyhow::Result<()> {
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

    #[test]
    fn a_prefetched_jpeg_opens_without_decoding_again() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([120, 90, 60])).save(&path)?;
        let ahead = Ahead::default();
        ahead.prefetch(&path, &AtomicBool::new(false));

        let events = load_with(&path, &dir.path().join("decoded"), ahead.clone());

        let (full, status) = ready(&events);
        assert_eq!((full.width, full.height), (40, 30));
        assert!(status.starts_with("Opened ahead of time"), "{status}");
        assert!(ahead.take(&path).is_none(), "taken, not held twice");
        Ok(())
    }

    #[test]
    fn a_rendered_file_replaced_after_its_prefetch_opens_as_it_is_now() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.jpg");
        image::RgbImage::from_pixel(40, 30, image::Rgb([120, 90, 60])).save(&path)?;
        let ahead = Ahead::default();
        ahead.prefetch(&path, &AtomicBool::new(false));
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
        ahead.prefetch(&path, &AtomicBool::new(false));
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

        assert!(ahead.take(&path).is_none());
        Ok(())
    }

    #[test]
    fn a_cancelled_prefetch_keeps_nothing() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("next.jpg");
        image::RgbImage::new(4, 4).save(&path)?;
        let ahead = Ahead::default();

        ahead.prefetch(&path, &AtomicBool::new(true));

        assert!(ahead.take(&path).is_none());
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
