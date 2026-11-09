//! Develop's HDR histogram (docs/hdr.md#histogram): the histogram of an HDR edit's
//! master, in Lightroom's layout of an SDR section up to graphics white and stops
//! above it up to the HDR limit.
//!
//! It describes the edit, not the display, so it is measured wherever the edit's
//! master renders, on an SDR display too: from a master of the whole photo
//! [`EDGE`] pixels long, rendered on a latest-only worker of its own, keyed by the
//! photo and the edit like the HDR preview ([`super::hdr_preview`]). The display's
//! headroom is applied only where the histogram is drawn, so a headroom change
//! shows at once and renders nothing. A result for anything but the latest request
//! is dropped. While the edit's next histogram renders, the last one of the same
//! photo stays, as the SDR histogram stays until its next render; another photo's
//! never shows. Where the master is refused the SDR histogram shows, with the
//! reason.
use crate::app::worker::Latest;
use crate::camera_data::CameraImage;
use crate::model::recipe::Recipe;
use crate::rendered::{HdrHistogram, HdrImage};
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The long edge of the master the histogram is measured on. The SDR histogram
/// measures the Fit preview; this one has no view to share, so it is fixed and
/// window size never renders it again.
pub(in crate::app) const EDGE: u32 = 512;

/// What a histogram was measured for: the decoded photo, by its allocation (held
/// weakly, as [`super::hdr_preview::Key`] holds it), and the edit.
#[derive(Clone, Debug)]
pub(in crate::app) struct Key {
    image: Weak<CameraImage>,
    recipe: Recipe,
}
impl Key {
    pub(in crate::app) fn new(image: &Arc<CameraImage>, recipe: &Recipe) -> Self {
        Self {
            image: Arc::downgrade(image),
            recipe: recipe.clone(),
        }
    }
    fn same_photo(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.image, &other.image)
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.same_photo(other) && self.recipe == other.recipe
    }
}

/// A measured master: its histogram, and the master itself, which the RGB readout
/// reads where no HDR preview of the edit shows.
pub(in crate::app) struct Measured {
    key: Key,
    pub(in crate::app) histogram: HdrHistogram,
    pub(in crate::app) master: HdrImage,
}

struct Job {
    key: Key,
    attempt: u64,
    image: Arc<CameraImage>,
    cancel: Arc<AtomicBool>,
}

/// What the worker hands back for an attempt: the measurement, or why it failed.
type Outcome = (u64, Result<Measured, String>);

/// Renders a job's measurement; [`render`], or a stand-in in tests.
type Render = fn(&Job) -> anyhow::Result<Measured>;

/// What the histogram shows for the edit asked for.
pub(in crate::app) enum Shown<'a> {
    /// The edit's own histogram.
    Current(&'a Measured),
    /// An earlier edit's of the same photo, while the edit's renders.
    Earlier(&'a Measured),
    /// Nothing of this photo yet.
    Rendering,
    /// The edit's render failed, for this reason.
    Failed(&'a str),
}

pub(in crate::app) struct HdrHistograms {
    worker: Latest<Job>,
    done: Arc<Mutex<Option<Outcome>>>,
    requested: Option<Key>,
    attempt: u64,
    cancel: Option<Arc<AtomicBool>>,
    measured: Option<Measured>,
    failure: Option<(Key, String)>,
}

impl HdrHistograms {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        Self::with_render(ctx, render)
    }
    fn with_render(ctx: egui::Context, render: Render) -> Self {
        let done: Arc<Mutex<Option<Outcome>>> = Arc::default();
        let out = done.clone();
        let worker = Latest::new(move |job: Job| {
            // A panic is this edit's failure, so the SDR histogram shows with the
            // reason and the next edit asks again.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| render(&job)))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("the renderer stopped unexpectedly")))
                .map_err(|e| format!("{e:#}"));
            *out.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some((job.attempt, result));
            ctx.request_repaint();
        });
        Self {
            worker,
            done,
            requested: None,
            attempt: 0,
            cancel: None,
            measured: None,
            failure: None,
        }
    }
    /// Stops the worker; a render under way is cancelled and dropped.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        if let Some(cancel) = &self.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
        self.worker.stop()
    }
    /// Asks for the histogram of `key`, unless it was asked for already; a render of
    /// the earlier request is cancelled.
    pub(in crate::app) fn request(&mut self, key: Key, image: Arc<CameraImage>) {
        if self.requested.as_ref() == Some(&key) {
            return;
        }
        if let Some(old) = self.cancel.take() {
            old.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.requested = Some(key.clone());
        self.attempt += 1;
        self.cancel = Some(cancel.clone());
        self.failure = None;
        self.worker.submit(Job {
            key,
            attempt: self.attempt,
            image,
            cancel,
        });
    }
    /// Forgets what was asked for and measured: no HDR histogram is due (HDR off, the
    /// master refused, the photo closed or replaced). Attempts keep counting, so a
    /// result still on its way is never taken for a later request's.
    pub(in crate::app) fn release(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.requested = None;
        self.measured = None;
        self.failure = None;
    }
    /// Takes the worker's result, if it is the latest request's.
    fn collect(&mut self) {
        let Some((attempt, result)) = self
            .done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
        else {
            return;
        };
        if attempt != self.attempt || self.requested.is_none() {
            return;
        }
        match result {
            Ok(measured) => {
                self.failure = None;
                self.measured = Some(measured);
            }
            Err(e) => self.failure = self.requested.clone().map(|key| (key, e)),
        }
    }
    /// What shows for `key`, taking a finished render first.
    pub(in crate::app) fn shown(&mut self, key: &Key) -> Shown<'_> {
        self.collect();
        if let Some((_, e)) = self.failure.as_ref().filter(|(k, _)| k == key) {
            return Shown::Failed(e);
        }
        match &self.measured {
            Some(m) if &m.key == key => Shown::Current(m),
            Some(m) if m.key.same_photo(key) => Shown::Earlier(m),
            _ => Shown::Rendering,
        }
    }
    /// The master measured for exactly `key`, without taking a finished render.
    pub(in crate::app) fn current(&self, key: &Key) -> Option<&Measured> {
        self.measured.as_ref().filter(|m| &m.key == key)
    }
}

/// The HDR master of the whole photo `job` names, at [`EDGE`], and its histogram.
fn render(job: &Job) -> anyhow::Result<Measured> {
    let recipe = job.key.recipe.checked()?;
    let cancel = rawmakase_cancel::Cancel::new(&job.cancel);
    let reduced = crate::develop::preview(&job.image, EDGE * 2);
    let master = crate::develop::render_hdr(&reduced, &recipe, None, EDGE, cancel)?;
    Ok(Measured {
        key: job.key.clone(),
        histogram: master.histogram(),
        master,
    })
}

/// What the Develop histogram shows.
pub(in crate::app) enum Source<'a> {
    /// The SDR rendition's histogram; for an HDR edit, with the reason.
    Sdr(Option<String>),
    /// The HDR master's.
    Hdr(&'a Measured),
}

impl crate::app::Editor {
    /// The HDR histogram due for the edit shown, or why there is none.
    pub(in crate::app) fn hdr_histogram_due(
        &self,
    ) -> Result<(Key, Arc<CameraImage>), &'static str> {
        let image = self.document.full().ok_or("the photo is still opening")?;
        let recipe = self.effective_recipe();
        crate::app::develop::viewport::hdr_master_refused(&recipe, image)?;
        Ok((Key::new(image, &recipe), image.clone()))
    }
    /// What Develop's histogram shows, asking for the HDR histogram it is due.
    pub(in crate::app) fn histogram_source(&mut self) -> Source<'_> {
        let due = self.hdr_histogram_due();
        let (key, image) = match due {
            Ok(due) => due,
            Err(why) => {
                self.hdr_histogram.release();
                let hdr = self.effective_recipe().hdr.is_active();
                return Source::Sdr(hdr.then(|| why.to_owned()));
            }
        };
        self.hdr_histogram.request(key.clone(), image);
        match self.hdr_histogram.shown(&key) {
            Shown::Current(m) | Shown::Earlier(m) => Source::Hdr(m),
            Shown::Rendering => Source::Sdr(Some("the HDR histogram is rendering".into())),
            Shown::Failed(e) => Source::Sdr(Some(format!("the HDR histogram failed: {e}"))),
        }
    }
    /// The master measured for the edit as it is now, if there is one.
    pub(in crate::app) fn hdr_histogram_master(&self) -> Option<&HdrImage> {
        let (key, _) = self.hdr_histogram_due().ok()?;
        self.hdr_histogram.current(&key).map(|m| &m.master)
    }
}

/// How the highlight triangle reads an HDR histogram on a display with `headroom`
/// (linear, 1 = none), as Lightroom's: light above white that the display shows,
/// or light it can't (beyond its headroom, or flattened at the HDR limit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum HdrHighlights {
    /// Nothing above graphics white.
    None,
    /// Light above white, all of it within the display's headroom.
    Within,
    /// Light beyond the display's headroom or at the HDR limit.
    Beyond,
}
impl HdrHighlights {
    pub(in crate::app) fn of(h: &HdrHistogram, headroom: f32) -> Self {
        let any = |counts: [u32; 3]| counts.iter().any(|n| *n > 0);
        if any(h.at_limit) || any(h.above(headroom.max(1.).log2())) {
            Self::Beyond
        } else if any(h.above(0.)) {
            Self::Within
        } else {
            Self::None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hdr_recipe() -> Recipe {
        let mut recipe = Recipe::default();
        recipe.hdr.enabled = true;
        recipe
    }

    fn photo(ctx: &egui::Context) -> Arc<CameraImage> {
        crate::app::testing::editor_with_blue_photo(ctx, Default::default(), false).1
    }

    /// Waits at most ten seconds for the worker's result.
    fn finished(h: &HdrHistograms) {
        let start = std::time::Instant::now();
        while h.done.lock().unwrap().is_none() {
            assert!(start.elapsed().as_secs() < 10, "no HDR histogram");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn measured(key: &Key, limit: f32) -> Measured {
        let master = HdrImage {
            width: 1,
            height: 1,
            pixels: vec![[2.; 3]],
            peak: limit.exp2(),
        };
        Measured {
            key: key.clone(),
            histogram: master.histogram(),
            master,
        }
    }

    /// A request measures the master of exactly the photo and edit it names, at
    /// the histogram's size; asking again for the same key renders nothing new.
    #[test]
    fn a_request_measures_the_master_it_names_once() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = HdrHistograms::new(ctx);
        let key = Key::new(&image, &hdr_recipe());
        h.request(key.clone(), image.clone());
        finished(&h);
        let Shown::Current(m) = h.shown(&key) else {
            panic!("the edit's histogram")
        };
        assert!(m.master.width.max(m.master.height) <= EDGE);
        assert_eq!(
            m.histogram.total(),
            m.master.width * m.master.height,
            "every pixel counted"
        );
        h.request(key, image);
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(h.done.lock().unwrap().is_none());
        let _ = h.stop();
    }

    /// A result of an earlier request is dropped, even one for the same key asked
    /// for again; the earlier edit's histogram of the same photo stays while the
    /// edit's renders.
    #[test]
    fn a_stale_completion_is_dropped() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = HdrHistograms::new(ctx);
        let _ = h.worker.stop();
        let a = Key::new(&image, &hdr_recipe());
        let mut brighter = hdr_recipe();
        brighter.exposure = 1.;
        let b = Key::new(&image, &brighter);
        h.request(a.clone(), image.clone());
        let first = h.cancel.clone().unwrap();
        h.request(b.clone(), image.clone());
        assert!(
            first.load(Ordering::Relaxed),
            "the earlier render is cancelled"
        );
        // A's late result is not B's.
        *h.done.lock().unwrap() = Some((1, Ok(measured(&a, 4.))));
        assert!(matches!(h.shown(&b), Shown::Rendering));
        // B's arrives.
        *h.done.lock().unwrap() = Some((2, Ok(measured(&b, 4.))));
        assert!(matches!(h.shown(&b), Shown::Current(_)));
        // Back to A: B's stays meanwhile, and a late B result is dropped.
        h.request(a.clone(), image);
        *h.done.lock().unwrap() = Some((2, Ok(measured(&b, 3.))));
        let Shown::Earlier(m) = h.shown(&a) else {
            panic!("the earlier edit's histogram meanwhile")
        };
        assert_eq!(m.histogram.limit, 4.);
        assert!(h.current(&a).is_none(), "the readout never reads it");
    }

    /// A replaced or deleted photo's histogram never shows for another, and a
    /// released histogram drops its late result.
    #[test]
    fn another_photo_never_shows_this_ones_histogram() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = HdrHistograms::new(ctx.clone());
        let _ = h.worker.stop();
        let key = Key::new(&image, &hdr_recipe());
        h.request(key.clone(), image.clone());
        *h.done.lock().unwrap() = Some((1, Ok(measured(&key, 4.))));
        assert!(matches!(h.shown(&key), Shown::Current(_)));
        // Replaced: the same file decoded again is another photo.
        let replaced = photo(&ctx);
        let other = Key::new(&replaced, &hdr_recipe());
        assert!(matches!(h.shown(&other), Shown::Rendering));
        // Deleted: its key matches no photo opened later.
        drop(image);
        for _ in 0..4 {
            let next = photo(&ctx);
            assert!(matches!(
                h.shown(&Key::new(&next, &hdr_recipe())),
                Shown::Rendering
            ));
        }
        // Released (the document closed), a late result is dropped.
        h.request(other.clone(), replaced);
        let cancel = h.cancel.clone().unwrap();
        h.release();
        assert!(cancel.load(Ordering::Relaxed));
        *h.done.lock().unwrap() = Some((2, Ok(measured(&other, 4.))));
        assert!(matches!(h.shown(&other), Shown::Rendering));
        assert!(h.current(&key).is_none());
    }

    /// A render that fails or panics is that edit's failure, shown with its reason;
    /// the next edit asks again.
    #[test]
    fn a_failed_or_panicking_render_is_the_edits_failure() {
        /// Closed while the test checks that the next edit is still rendering: a
        /// render that panics at once could otherwise finish before the check.
        static OPEN: AtomicBool = AtomicBool::new(true);
        fn panics(_: &Job) -> anyhow::Result<Measured> {
            while !OPEN.load(Ordering::SeqCst) {
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            panic!("boom")
        }
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = HdrHistograms::with_render(ctx, panics);
        let key = Key::new(&image, &hdr_recipe());
        h.request(key.clone(), image.clone());
        finished(&h);
        let Shown::Failed(e) = h.shown(&key) else {
            panic!("a failure")
        };
        assert_eq!(e, "the renderer stopped unexpectedly");
        // The worker survives it and takes the next edit.
        let mut next = hdr_recipe();
        next.exposure = 0.5;
        let next = Key::new(&image, &next);
        OPEN.store(false, Ordering::SeqCst);
        h.request(next.clone(), image.clone());
        assert!(matches!(h.shown(&next), Shown::Rendering));
        OPEN.store(true, Ordering::SeqCst);
        finished(&h);
        assert!(matches!(h.shown(&next), Shown::Failed(_)));
        let _ = h.stop();
        // A cancelled render stops with an error.
        let cancel = Arc::new(AtomicBool::new(true));
        let job = Job {
            key,
            attempt: 1,
            image,
            cancel,
        };
        assert!(render(&job).is_err());
    }

    /// The highlight triangle follows the display's headroom without a new render.
    #[test]
    fn the_highlight_reading_follows_the_headroom() {
        let master = |pixels: Vec<[f32; 3]>| {
            HdrImage {
                width: pixels.len() as u32,
                height: 1,
                pixels,
                peak: 16.,
            }
            .histogram()
        };
        let bright = master(vec![[0.5; 3], [3.; 3]]);
        assert_eq!(HdrHighlights::of(&bright, 1.), HdrHighlights::Beyond);
        assert_eq!(HdrHighlights::of(&bright, 2.), HdrHighlights::Beyond);
        assert_eq!(HdrHighlights::of(&bright, 4.), HdrHighlights::Within);
        let sdr = master(vec![[0.5; 3], [1.; 3]]);
        assert_eq!(HdrHighlights::of(&sdr, 1.), HdrHighlights::None);
        let limited = master(vec![[16.; 3]]);
        assert_eq!(HdrHighlights::of(&limited, 64.), HdrHighlights::Beyond);
    }

    /// An HDR edit's histogram is its master's on any display, keyed by the photo
    /// and the edit alone: a headroom change renders nothing. Where it shows no
    /// HDR preview, the readout reads that master; a refused master falls back to
    /// the SDR histogram with the reason.
    #[test]
    fn an_hdr_edit_has_an_hdr_histogram_on_any_display() {
        use crate::app::develop::readout::Values;
        use crate::app::shell::display::HdrDisplay;
        let ctx = egui::Context::default();
        let (mut e, _) =
            crate::app::testing::editor_with_blue_photo(&ctx, Default::default(), true);
        assert_eq!(e.hdr_histogram_due().err(), Some("HDR is off"));
        assert!(matches!(e.histogram_source(), Source::Sdr(None)));
        e.document.edit.setup_mut().hdr.enabled = true;
        // The default display shows no HDR.
        let (key, _) = e
            .hdr_histogram_due()
            .expect("an HDR histogram on an SDR display");
        e.view.hdr_display = HdrDisplay {
            headroom: 4.,
            potential: 16.,
            float_surface: true,
            ..HdrDisplay::default()
        };
        assert_eq!(
            e.hdr_histogram_due().unwrap().0,
            key,
            "headroom isn't its key"
        );
        e.view.hdr_display = HdrDisplay::default();
        let start = std::time::Instant::now();
        loop {
            match e.histogram_source() {
                Source::Hdr(m) => {
                    assert!(m.histogram.total() > 0);
                    break;
                }
                Source::Sdr(why) => {
                    assert_eq!(why.as_deref(), Some("the HDR histogram is rendering"))
                }
            }
            assert!(start.elapsed().as_secs() < 20, "no HDR histogram");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // The readout reads the master, with no headroom on this display.
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::Vec2::new(64., 48.));
        e.update_readout(Some(egui::Pos2::new(10., 10.)), rect, None);
        assert!(
            matches!(
                e.view.readout.values,
                Some(Values::Hdr { headroom, .. }) if headroom == 1.
            ),
            "{:?}",
            e.view.readout.values
        );
        // Refused in HDR: the SDR histogram, with why, and the master is dropped.
        e.document.edit.setup_mut().highlights = 20.;
        assert!(matches!(
            e.histogram_source(),
            Source::Sdr(Some(why)) if why == "this edit doesn't render in HDR yet"
        ));
        assert!(e.hdr_histogram.measured.is_none());
        e.update_readout(Some(egui::Pos2::new(10., 10.)), rect, None);
        assert!(!matches!(e.view.readout.values, Some(Values::Hdr { .. })));
        let _ = e.hdr_histogram.stop();
    }
}
