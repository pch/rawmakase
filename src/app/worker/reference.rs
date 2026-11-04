//! Develop's Reference View photo: another catalog photo, developed with its own
//! edit beside the one being edited. Its own worker, so loading it never holds up
//! the photo on screen; like opening a photo, a half-size decode of a RAW comes
//! first for Fit and the full one follows for 100%. A JPEG, PNG or TIFF is ready
//! at full size at once.
use super::{Event, Latest, send};
use crate::{
    app::library::EditSource,
    camera_data,
    decode::{DecodePolicy, FullSize},
    decode_cache::DecodeCache,
    model::recipe::Recipe,
    photo::Opened,
};
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

/// A reference photo to develop.
pub(in crate::app) struct ReferenceJob {
    /// Matches the results to this request.
    pub ticket: u64,
    pub path: PathBuf,
    pub edit: EditSource,
    pub cancel: Arc<AtomicBool>,
    /// The demosaic of the full-size decode and of its decode-cache key.
    pub demosaic: camera_data::Demosaic,
}
/// The reference photo, developed: the half-size decode first, then the full one.
pub(crate) struct ReferenceImage {
    pub image: Arc<camera_data::CameraImage>,
    pub recipe: Recipe,
    pub resolution: Resolution,
}
/// How much of the photo's resolution an image has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// The half-size decode: enough for Fit.
    Half,
    /// Every pixel, for 100%.
    Full,
}

pub(in crate::app) fn reference_loader(
    tx: Sender<Event>,
    ctx: egui::Context,
    cache: DecodeCache,
) -> Latest<ReferenceJob> {
    Latest::new(move |job: ReferenceJob| {
        let ticket = job.ticket;
        let cancel = job.cancel.clone();
        let reply = |result: Result<Box<ReferenceImage>, String>| {
            if !cancel.load(Ordering::Relaxed) {
                send(&tx, &ctx, Event::Reference { ticket, result });
            }
        };
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            develop(&job, &cache, |image| reply(Ok(Box::new(image))))
        }));
        match loaded {
            Ok(Ok(())) => {}
            Ok(Err(e)) => reply(Err(format!("{e:#}"))),
            Err(panic) => reply(Err(format!(
                "Loading failed: {}",
                super::panic_message(&*panic)
            ))),
        }
    })
}

/// Develops the job's photo, handing each stage to `ready`.
fn develop(
    job: &ReferenceJob,
    cache: &DecodeCache,
    mut ready: impl FnMut(ReferenceImage),
) -> anyhow::Result<()> {
    let raw = match crate::photo::open_source(&job.path, Cancel::new(&job.cancel))? {
        Opened::Raw(raw) => raw,
        // A JPEG, PNG or TIFF opens at full size: nothing to cache or refine.
        Opened::Rendered(image) => {
            let recipe = EditSource::recipe(
                Some(&job.edit),
                &image.metadata,
                &job.path,
                Cancel::new(&job.cancel),
            )?;
            if !job.cancel.load(Ordering::Relaxed) {
                ready(ReferenceImage {
                    image: Arc::new(image),
                    recipe,
                    resolution: Resolution::Full,
                });
            }
            return Ok(());
        }
    };
    let recipe = EditSource::recipe(
        Some(&job.edit),
        &raw.metadata,
        &job.path,
        Cancel::new(&job.cancel),
    )?;
    let full_size = FullSize::with_cache(&job.path, job.demosaic, cache.clone());
    if let Some(image) = full_size.cached(&raw.metadata) {
        // Closed before the reference is ready: Windows refuses to rename or
        // replace a file that is still open.
        drop(raw);
        ready(ReferenceImage {
            image: Arc::new(image),
            recipe,
            resolution: Resolution::Full,
        });
        return Ok(());
    }
    let half = raw.develop(camera_data::Decode::Half, Cancel::new(&job.cancel))?;
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    ready(ReferenceImage {
        image: Arc::new(half),
        recipe: recipe.clone(),
        resolution: Resolution::Half,
    });
    let opened = crate::photo::open(&job.path)?;
    let full = Arc::new(full_size.decode(opened, DecodePolicy::Show, Cancel::new(&job.cancel))?);
    if job.cancel.load(Ordering::Relaxed) {
        return Ok(());
    }
    full_size.store(&full, Cancel::new(&job.cancel));
    ready(ReferenceImage {
        image: full,
        recipe,
        resolution: Resolution::Full,
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera_data::Demosaic;

    /// The job's demosaic is decoded, and the cache entry is keyed with it.
    #[test]
    fn the_full_decode_is_cached_under_the_jobs_demosaic() -> anyhow::Result<()> {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let cache = DecodeCache::new(dir.path().to_path_buf(), u64::MAX);
        let key = |demosaic| DecodeCache::key(&chart, demosaic);
        let job = |demosaic| ReferenceJob {
            ticket: 1,
            path: chart.clone(),
            edit: EditSource::Defaults(Default::default()),
            cancel: Default::default(),
            demosaic,
        };
        let develop_stages = |job: &ReferenceJob| -> anyhow::Result<Vec<Resolution>> {
            let mut stages = Vec::new();
            develop(job, &cache, |image| stages.push(image.resolution))?;
            Ok(stages)
        };

        let demosaic = Demosaic::Rawmakase;
        assert!(!cache.contains(&key(demosaic)?));
        assert_eq!(
            develop_stages(&job(demosaic))?,
            [Resolution::Half, Resolution::Full]
        );
        assert!(cache.contains(&key(demosaic)?));

        // The next job with that demosaic opens from the cache.
        assert_eq!(develop_stages(&job(demosaic))?, [Resolution::Full]);
        Ok(())
    }

    /// Once the reference is ready its file can be renamed: Windows refuses
    /// while it is still open.
    #[test]
    fn the_file_is_closed_when_the_reference_is_ready() -> anyhow::Result<()> {
        let chart = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/corpus/charts/synthetic-d65.dng");
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("photo.dng");
        std::fs::copy(chart, &path)?;
        let cache = DecodeCache::new(dir.path().join("cache"), u64::MAX);
        let job = ReferenceJob {
            ticket: 1,
            path: path.clone(),
            edit: EditSource::Defaults(Default::default()),
            cancel: Default::default(),
            demosaic: Demosaic::default(),
        };
        let away = path.with_extension("away");
        // Developed, then opened from the cache.
        for _ in 0..2 {
            let mut renamed = Vec::new();
            develop(&job, &cache, |image| {
                if image.resolution == Resolution::Full {
                    renamed.push(
                        std::fs::rename(&path, &away).and_then(|()| std::fs::rename(&away, &path)),
                    );
                }
            })?;
            assert_eq!(renamed.len(), 1);
            renamed.pop().unwrap()?;
        }
        Ok(())
    }

    /// A JPEG is its full-size image: one stage, with its edit's Embedded
    /// profile, and nothing kept in the decode cache.
    #[test]
    fn a_jpeg_reference_is_ready_at_full_size_at_once() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("photo.jpg");
        image::RgbImage::from_pixel(12, 8, image::Rgb([90, 120, 150])).save(&path)?;
        let cache = DecodeCache::new(dir.path().join("cache"), u64::MAX);
        let job = ReferenceJob {
            ticket: 1,
            path,
            edit: EditSource::Defaults(Default::default()),
            cancel: Default::default(),
            demosaic: Demosaic::default(),
        };
        let mut images = Vec::new();
        develop(&job, &cache, |image| images.push(image))?;
        let [image] = &images[..] else {
            panic!("{} stages", images.len());
        };
        assert_eq!(image.resolution, Resolution::Full);
        assert!(!image.image.fast);
        assert!(
            image
                .recipe
                .profile
                .as_ref()
                .is_some_and(|p| p.is_embedded())
        );
        assert!(!dir.path().join("cache").exists());
        Ok(())
    }
}
