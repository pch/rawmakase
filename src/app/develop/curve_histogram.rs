//! The Tone Curve panel's histogram: what the tone curves take in, not what they
//! give out, so a curve being dragged moves over a histogram that stays put.
//!
//! It is measured from the whole photo [`EDGE`] pixels long
//! ([`crate::develop::quality::curve_input_histogram`]), on a latest-only worker of
//! its own, keyed by the photo and the edit with its tone curves left out
//! ([`Key::new`]): moving a curve never renders it again, and another slider (or
//! Levels, which the curves follow) does. It renders only while the panel shows it;
//! a frame without it cancels the render and lets go of the photo ([`CurveHistograms::sweep`]).
//! A result for anything but the latest request is dropped. While the edit's next
//! histogram renders, the last one of the same photo stays; another photo's never
//! shows. Until one of this photo arrives, or when its render fails, the panel
//! shows Develop's own histogram.
use crate::app::worker::Latest;
use crate::camera_data::CameraImage;
use crate::model::panels::{Panel, PanelState};
use crate::model::recipe::Recipe;
use crate::rendered::Histogram;
use eframe::egui;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// The long edge of the photo the histogram is measured on, as the HDR histogram's
/// ([`super::hdr_histogram::EDGE`]): it has no view to share, so window size never
/// renders it again.
pub(in crate::app) const EDGE: u32 = 512;

/// What a histogram was measured for: the decoded photo, by its allocation (held
/// weakly, as [`super::hdr_histogram::Key`] holds it), the edit without its tone
/// curves, and the AI Denoise source it blends in.
#[derive(Clone, Debug)]
pub(in crate::app) struct Key {
    image: Weak<CameraImage>,
    recipe: Recipe,
    ai: Option<crate::develop::AiSource>,
}
impl Key {
    /// The key of `recipe`'s histogram: `recipe` with the Tone Curve panel's curves
    /// at their defaults (its switch's bypass), which change nothing the curves
    /// take in. Levels are not among them, and stay.
    pub(in crate::app) fn new(
        image: &Arc<CameraImage>,
        recipe: &Recipe,
        ai: Option<&crate::develop::AiSource>,
    ) -> Self {
        let mut recipe = recipe.clone();
        recipe.panels.set(Panel::ToneCurve, PanelState::Off);
        let recipe = recipe.as_rendered().into_owned();
        Self {
            image: Arc::downgrade(image),
            recipe,
            ai: ai.cloned(),
        }
    }
    fn same_photo(&self, other: &Self) -> bool {
        Weak::ptr_eq(&self.image, &other.image)
    }
}
impl PartialEq for Key {
    fn eq(&self, other: &Self) -> bool {
        self.same_photo(other) && self.recipe == other.recipe && self.ai == other.ai
    }
}

struct Measured {
    key: Key,
    histogram: Histogram,
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

pub(in crate::app) struct CurveHistograms {
    worker: Latest<Job>,
    done: Arc<Mutex<Option<Outcome>>>,
    requested: Option<Key>,
    attempt: u64,
    cancel: Option<Arc<AtomicBool>>,
    measured: Option<Measured>,
    /// Whether the frame being drawn asked for a histogram ([`Self::sweep`]).
    asked: bool,
}

impl CurveHistograms {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        Self::with_render(ctx, render)
    }
    fn with_render(ctx: egui::Context, render: Render) -> Self {
        let done: Arc<Mutex<Option<Outcome>>> = Arc::default();
        let out = done.clone();
        let worker = Latest::new(move |job: Job| {
            // A panic is this edit's failure: the panel shows Develop's histogram,
            // and the next edit asks again.
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
            asked: false,
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
    fn request(&mut self, key: Key, image: Arc<CameraImage>) {
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
        self.worker.submit(Job {
            key,
            attempt: self.attempt,
            image,
            cancel,
        });
    }
    /// Takes the worker's result, if it is the latest request's. A failure keeps
    /// nothing of it: the panel falls back to Develop's histogram.
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
            Ok(measured) => self.measured = Some(measured),
            Err(e) => {
                log::warn!("The Tone Curve's histogram failed: {e}");
                self.measured = None;
            }
        }
    }
    /// Asks for `key`'s histogram and returns what shows meanwhile: `key`'s own, or
    /// an earlier edit's of the same photo while it renders; `None` before any of
    /// this photo, and after a failure.
    pub(in crate::app) fn shown(
        &mut self,
        key: Key,
        image: Arc<CameraImage>,
    ) -> Option<&Histogram> {
        self.asked = true;
        self.collect();
        // Another photo's is never shown, so it isn't kept either.
        if self
            .measured
            .as_ref()
            .is_some_and(|m| !m.key.same_photo(&key))
        {
            self.measured = None;
        }
        self.request(key, image);
        self.measured.as_ref().map(|m| &m.histogram)
    }
    /// After each frame: when it asked for no histogram (the panel closed, Develop
    /// left, the photo closed or still opening), cancels the render under way and
    /// lets go of what was asked for and measured, and of a result that arrives
    /// late, so no photo or AI Denoise source outlives its use here. Attempts keep
    /// counting, so a late result is never taken for a later request's.
    pub(in crate::app) fn sweep(&mut self) {
        if std::mem::take(&mut self.asked) {
            return;
        }
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
        self.requested = None;
        self.measured = None;
        self.done
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
    }
}

/// The histogram of what the curves take in, of the photo and edit `job` names.
fn render(job: &Job) -> anyhow::Result<Measured> {
    let recipe = job.key.recipe.checked()?;
    let cancel = rawmakase_cancel::Cancel::new(&job.cancel);
    let ai = job.key.ai.as_ref();
    let histogram =
        crate::develop::quality::curve_input_histogram(&job.image, &recipe, ai, EDGE, cancel)?;
    Ok(Measured {
        key: job.key.clone(),
        histogram,
    })
}

impl crate::app::Editor {
    /// The Tone Curve histogram due for the edit shown: its key and the decoded
    /// photo, or `None` while the photo opens.
    pub(in crate::app) fn curve_histogram_due(&self) -> Option<(Key, Arc<CameraImage>)> {
        let image = self.document.full()?;
        let recipe = self.effective_recipe();
        let ai = self.shown_ai_source(image, &recipe.ai_denoise);
        Some((Key::new(image, &recipe, ai.as_ref()), image.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn photo(ctx: &egui::Context) -> Arc<CameraImage> {
        crate::app::testing::editor_with_blue_photo(ctx, Default::default(), false).1
    }

    /// Waits at most ten seconds for the worker's result.
    fn finished(h: &CurveHistograms) {
        let start = std::time::Instant::now();
        while h.done.lock().unwrap().is_none() {
            assert!(start.elapsed().as_secs() < 10, "no curve histogram");
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    fn measured(key: &Key, level: usize) -> Measured {
        let mut histogram = Histogram::EMPTY;
        histogram.bins[0][level] = 1;
        Measured {
            key: key.clone(),
            histogram,
        }
    }

    /// Moving any tone curve leaves the key, so the histogram, as it was; Levels and
    /// the other sliders, which change what the curves take in, do not.
    #[test]
    fn only_the_curves_leave_the_key_as_it_was() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let plain = Recipe::default();
        let key = Key::new(&image, &plain, None);
        let mut curved = plain.clone();
        curved.curve.points = vec![[0., 0.], [0.25, 0.6], [1., 1.]];
        curved.effects.channels[0].points = vec![[0., 0.2], [1., 1.]];
        curved.effects.parametric = [40., -20., 10., -30.];
        curved.effects.splits = [0.2, 0.5, 0.8];
        curved.curve_saturation = 50.;
        assert_eq!(Key::new(&image, &curved, None), key);
        let mut switched = curved;
        switched.panels.set(Panel::ToneCurve, PanelState::Off);
        assert_eq!(Key::new(&image, &switched, None), key);
        let mut levels = plain.clone();
        levels.black_point = 0.1;
        assert_ne!(Key::new(&image, &levels, None), key);
        let mut brighter = plain;
        brighter.exposure = 1.;
        assert_ne!(Key::new(&image, &brighter, None), key);
    }

    /// The histogram counts what the curves take in: a curve that darkens the
    /// photo renders the same histogram, while its output histogram moves.
    #[test]
    fn a_curve_does_not_move_its_input_histogram() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        use rawmakase_cancel::Cancel;
        let histogram = |r: &Recipe| {
            let r = r.checked().unwrap();
            crate::develop::quality::curve_input_histogram(&image, &r, None, EDGE, Cancel::NEVER)
                .unwrap()
        };
        let output = |r: &Recipe| {
            crate::develop::render(&image, &r.checked().unwrap(), EDGE, Cancel::NEVER)
                .unwrap()
                .histogram()
        };
        let plain = Recipe::default();
        let mut dark = plain.clone();
        dark.curve.points = vec![[0., 0.], [1., 0.3]];
        let input = histogram(&plain);
        assert!(input.bins.iter().flatten().any(|n| *n > 0));
        assert_eq!(histogram(&dark).bins, input.bins);
        assert_ne!(output(&dark).bins, output(&plain).bins, "the curve renders");
    }

    /// A request renders once per key; a result of an earlier request is dropped,
    /// the earlier edit's histogram of the same photo stays while the next renders,
    /// and another photo never shows this one's.
    #[test]
    fn a_stale_or_other_photos_histogram_never_shows() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = CurveHistograms::new(ctx.clone());
        let a = Key::new(&image, &Recipe::default(), None);
        assert!(h.shown(a.clone(), image.clone()).is_none());
        finished(&h);
        assert!(h.shown(a.clone(), image.clone()).is_some());
        // Asked again for the same key: nothing renders.
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(h.done.lock().unwrap().is_none());
        let _ = h.stop();

        let mut h = CurveHistograms::new(ctx.clone());
        let _ = h.worker.stop();
        let brighter = Recipe {
            exposure: 1.,
            ..Default::default()
        };
        let b = Key::new(&image, &brighter, None);
        h.shown(a.clone(), image.clone());
        let first = h.cancel.clone().unwrap();
        h.shown(b.clone(), image.clone());
        assert!(
            first.load(Ordering::Relaxed),
            "the earlier render is cancelled"
        );
        // A's late result is not B's.
        *h.done.lock().unwrap() = Some((1, Ok(measured(&a, 1))));
        assert!(h.shown(b.clone(), image.clone()).is_none());
        *h.done.lock().unwrap() = Some((2, Ok(measured(&b, 2))));
        assert_eq!(h.shown(b, image.clone()).unwrap().bins[0][2], 1);
        // Back to A: B's stays meanwhile.
        assert_eq!(h.shown(a, image).unwrap().bins[0][2], 1);
        // A replaced photo is another photo.
        let replaced = photo(&ctx);
        let other = Key::new(&replaced, &Recipe::default(), None);
        assert!(h.shown(other, replaced).is_none());
    }

    /// A frame that asks for no histogram cancels the render and lets go of the
    /// photo's key, result and a late completion; the next ask renders again.
    #[test]
    fn a_frame_without_the_panel_lets_go_of_the_histogram() {
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = CurveHistograms::new(ctx);
        let _ = h.worker.stop();
        let key = Key::new(&image, &Recipe::default(), None);
        h.shown(key.clone(), image.clone());
        *h.done.lock().unwrap() = Some((1, Ok(measured(&key, 1))));
        assert!(h.shown(key.clone(), image.clone()).is_some());
        h.sweep();
        assert!(h.measured.is_some(), "kept while the frame asked for it");
        let cancel = h.cancel.clone().unwrap();
        // A frame without the panel.
        h.sweep();
        assert!(cancel.load(Ordering::Relaxed), "the render is cancelled");
        assert!(h.requested.is_none() && h.measured.is_none());
        // A late completion is dropped by the next sweep, not kept.
        *h.done.lock().unwrap() = Some((1, Ok(measured(&key, 1))));
        h.sweep();
        assert!(h.done.lock().unwrap().is_none());
        // Shown again: asked for anew, nothing of the earlier result.
        assert!(h.shown(key, image).is_none());
        assert!(h.requested.is_some());
    }

    /// A render that fails or panics shows nothing of it, and the worker takes the
    /// next edit.
    #[test]
    fn a_failed_or_panicking_render_falls_back() {
        fn panics(_: &Job) -> anyhow::Result<Measured> {
            panic!("boom")
        }
        let ctx = egui::Context::default();
        let image = photo(&ctx);
        let mut h = CurveHistograms::with_render(ctx, panics);
        let key = Key::new(&image, &Recipe::default(), None);
        assert!(h.shown(key.clone(), image.clone()).is_none());
        finished(&h);
        assert!(h.shown(key, image.clone()).is_none());
        let next = Recipe {
            exposure: 0.5,
            ..Default::default()
        };
        h.shown(Key::new(&image, &next, None), image.clone());
        finished(&h);
        assert!(h.done.lock().unwrap().is_some(), "the worker survived");
        let _ = h.stop();
        // A cancelled render stops with an error.
        let job = Job {
            key: Key::new(&image, &Recipe::default(), None),
            attempt: 1,
            image,
            cancel: Arc::new(AtomicBool::new(true)),
        };
        assert!(render(&job).is_err());
    }
}
