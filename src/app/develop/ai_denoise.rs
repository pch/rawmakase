//! AI Denoise in the desktop editor: the open photo's request and result, shown in
//! Develop and (a finished result only) in the Library's Loupe, and the Loupe's
//! loader of finished results. The service itself is [`crate::ai_denoise`].
use crate::ai_denoise::{CHANGED_SINCE_OPENED, Request, State, Update, finished};
use crate::camera_data::CameraImage;
use crate::develop::AiSource;
use rawmakase_cancel::Cancel;
use rawmakase_decode::ai::cache::AiCache;
use rawmakase_decode::ai::denoise::{ModelOutput, assemble};
use std::path::Path;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

/// A finished result the Library's Loupe shows: `key`'s entry in `cache`, assembled
/// over `plain`, the image it was found for.
pub(crate) struct CachedLoad {
    cache: AiCache,
    key: String,
    plain: Arc<CameraImage>,
    /// Set once the photo is left or another load replaces this one.
    cancel: Arc<AtomicBool>,
}

/// The worker that loads the Loupe's finished results, one at a time, the latest
/// first: a load asked for meanwhile replaces the one waiting. It hands each result
/// over as Develop's final result is (`Event::AiDenoise`), and stops between loads at
/// exit; a cancelled load stops between its stages and sends nothing.
pub(in crate::app) fn cached_loader(
    tx: std::sync::mpsc::Sender<crate::app::worker::Event>,
    ctx: eframe::egui::Context,
) -> crate::app::worker::Latest<CachedLoad> {
    crate::app::worker::Latest::new(move |job: CachedLoad| {
        let cancelled = || job.cancel.load(Ordering::Relaxed);
        if cancelled() {
            return;
        }
        let Some(entry) = job.cache.load(&job.key) else {
            return;
        };
        if cancelled() {
            return;
        }
        let output = ModelOutput {
            layout: entry.layout,
            rgb: &entry.rgb,
            done: None,
            clipped: &entry.clipped,
        };
        let source = assemble(&output, &job.plain, Cancel::new(&job.cancel))
            .and_then(|image| AiSource::recover(image, Cancel::new(&job.cancel)));
        if let Ok(source) = source
            && !cancelled()
        {
            let _ = tx.send(crate::app::worker::Event::AiDenoise(Box::new(Update {
                plain: Arc::downgrade(&job.plain),
                state: State::Ready(source),
            })));
            ctx.request_repaint();
        }
    })
}

/// AI Denoise for the open photo's decoded image: the request in flight, the latest
/// result (partial or final) and what the Detail panel says. Replaced with the image, and
/// dropping it tells the service the photo is no longer wanted.
#[derive(Default)]
pub(crate) struct Photo {
    /// The image asked for and whether it is still wanted.
    requested: Option<(std::sync::Weak<CameraImage>, Arc<AtomicBool>)>,
    source: Option<AiSource>,
    /// `source` is the final result, not a partial one.
    complete: bool,
    /// The last request failed: not asked again for this image (a reinstalled model
    /// or a reopened photo asks again).
    failed: bool,
    status: Option<State>,
    /// The image the Library's Loupe looked for a finished result for, found or not:
    /// not looked for again until one is stored.
    looked_up: Option<std::sync::Weak<CameraImage>>,
    /// Cancels the Loupe's load of a finished result, once the photo is left.
    loading: Option<Arc<AtomicBool>>,
}
impl Drop for Photo {
    fn drop(&mut self) {
        self.release();
    }
}
impl Photo {
    /// The latest result.
    pub(crate) fn source(&self) -> Option<&AiSource> {
        self.source.as_ref()
    }
    /// Whether [`source`](Self::source) is the final result, not a partial one.
    pub(crate) fn is_complete(&self) -> bool {
        self.complete
    }
    /// What happened last: running, ready, or why there is none.
    pub(crate) fn status(&self) -> Option<&State> {
        self.status.as_ref()
    }
    /// Tells the service a queued request is no longer wanted. A result already
    /// received is kept, so switching back on shows it at once.
    fn release(&mut self) {
        if let Some((_, wanted)) = self.requested.take() {
            wanted.store(false, Ordering::Relaxed);
        }
        // A load given up before it was shown is looked for again next time.
        if let Some(cancel) = self.loading.take() {
            cancel.store(true, Ordering::Relaxed);
            self.looked_up = None;
        }
    }
    /// Whether `image` should be asked for: not a draft, not asked for already, and
    /// without a final result or a failure. A partial result switched off before the
    /// final one arrived asks again (the job finished and stored: a cache hit).
    fn needs_request(&self, image: &Arc<CameraImage>) -> bool {
        !image.fast && !self.asked_for(image) && !self.complete && !self.failed
    }
    fn asked_for(&self, image: &Arc<CameraImage>) -> bool {
        self.requested
            .as_ref()
            .is_some_and(|(asked, _)| std::ptr::eq(asked.as_ptr(), Arc::as_ptr(image)))
    }
}

impl crate::app::Editor {
    /// Asks for the open photo's AI Denoise result when its edit renders AI Denoise and
    /// the full-size image is decoded; withdraws the request when it no longer does.
    /// `region` is the 100% view (in the render's pixels of `geometry`): its tiles run
    /// first.
    pub(in crate::app) fn request_ai_denoise(
        &mut self,
        image: &Arc<CameraImage>,
        geometry: &crate::develop::Geometry,
        region: Option<[u32; 4]>,
    ) {
        let renders = self.document.edit.recipe().ai_denoise.renders();
        if !renders {
            self.document.ai.release();
            return;
        }
        if !self.document.ai.needs_request(image) {
            return;
        }
        let Some(path) = self.document.path.clone() else {
            return;
        };
        // Without the file's identity from before decoding, nothing proves the result
        // would be of the same file: refused, as a changed file is.
        let Some(file) = self.document.image_file.clone() else {
            self.document.ai.status = Some(State::Failed(CHANGED_SINCE_OPENED.into()));
            return;
        };
        // The Library's Loupe shows a finished result, as its previews do, and never
        // runs the model: without one, the photo shows plain.
        if self.module == crate::app::library::Module::Library {
            self.load_finished_ai(image, &file);
            return;
        }
        let model = self.selection.models.denoise.path();
        if model.is_none() {
            self.document.ai.status = Some(State::Failed(
                "Install the AI Denoise model to use it.".into(),
            ));
            return;
        }
        self.ai_denoise.set_model(model);
        let source_region = region.map(|[x, y, w, h]| {
            let (gw, gh) = (geometry.width.max(1) as f32, geometry.height.max(1) as f32);
            let corners = [(x, y), (x + w, y), (x, y + h), (x + w, y + h)]
                .map(|(u, v)| geometry.source(u as f32 / gw, v as f32 / gh));
            let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for [cx, cy] in corners {
                (x0, y0, x1, y1) = (x0.min(cx), y0.min(cy), x1.max(cx), y1.max(cy));
            }
            let clamp = |v: f32, max: u32| v.clamp(0., max as f32) as u32;
            let (x0, y0) = (clamp(x0, image.width), clamp(y0, image.height));
            let (x1, y1) = (clamp(x1, image.width), clamp(y1, image.height));
            [x0, y0, (x1 - x0).max(1), (y1 - y0).max(1)]
        });
        let wanted = Arc::new(AtomicBool::new(true));
        self.document.ai.requested = Some((Arc::downgrade(image), wanted.clone()));
        self.document.ai.status = Some(State::Queued);
        let (tx, ctx) = (Mutex::new(self.tx.clone()), self.context.clone());
        self.ai_denoise.request(Request {
            path,
            file,
            plain: image.clone(),
            region: source_region,
            wanted,
            sink: Arc::new(move |update| {
                let _ = tx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .send(crate::app::worker::Event::AiDenoise(Box::new(update)));
                ctx.request_repaint();
            }),
        });
    }
    /// For the Library's Loupe: a finished result in the cache for the file `file`
    /// names (of any provider), loaded and assembled over `image` off the UI thread,
    /// and handed over as Develop's final result is. Never runs the model; without a
    /// result the photo stays plain. Looked for once per image, until one is stored.
    fn load_finished_ai(&mut self, image: &Arc<CameraImage>, file: &crate::storage::Identity) {
        let looked = self.document.ai.looked_up.as_ref();
        if looked.is_some_and(|w| std::ptr::eq(w.as_ptr(), Arc::as_ptr(image))) {
            return;
        }
        self.document.ai.looked_up = Some(Arc::downgrade(image));
        let Some(key) = finished(self.ai_denoise_cache(), file) else {
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        if let Some(earlier) = self.document.ai.loading.replace(cancel.clone()) {
            earlier.store(true, Ordering::Relaxed);
        }
        self.ai_loupe.submit(CachedLoad {
            cache: self.ai_denoise_cache().clone(),
            key,
            plain: image.clone(),
            cancel,
        });
    }
    /// A result was stored for the file at `path`: the Library's Loupe showing it
    /// plain looks again.
    pub(in crate::app) fn ai_result_stored(&mut self, path: &Path) {
        if let Some(library) = &mut self.library {
            library.ai_result_stored(path);
        }
        if self.module == crate::app::library::Module::Library
            && self.document.path.as_deref() == Some(path)
            && !self.document.ai.complete
        {
            self.document.ai.looked_up = None;
            self.schedule();
        }
    }
    /// The AI Denoise source a render of `image` with `setting` shows: Develop's latest
    /// result, partial or final; the Library's Loupe only a final one.
    pub(in crate::app) fn shown_ai_source(
        &self,
        image: &Arc<CameraImage>,
        setting: &crate::model::ai_denoise::AiDenoise,
    ) -> Option<crate::develop::AiSource> {
        self.document.ai_source(image, setting).filter(|_| {
            self.module != crate::app::library::Module::Library || self.document.ai.complete
        })
    }
    /// Removes AI Denoise's model: the service lets go of it first, off the UI thread,
    /// and the photo shown asks again (and is told to install it) when next rendered.
    pub(in crate::app) fn remove_ai_denoise_model(&mut self) {
        let service = self.ai_denoise.clone();
        self.selection
            .models
            .of_mut(crate::model_install::Pack::Denoise)
            .remove(
                move || service.unload(),
                self.tx.clone(),
                self.context.clone(),
            );
        self.document.ai = Default::default();
        self.schedule();
    }
    /// The service as exports reach it, with the installed model (or none, when every
    /// photo with AI Denoise on fails with the reason).
    pub(in crate::app) fn ai_service(&self) -> rawmakase_decode::ai::denoise::AiService {
        self.ai_denoise
            .set_model(self.selection.models.denoise.path());
        rawmakase_decode::ai::denoise::AiService(Arc::new(self.ai_denoise.clone()))
    }
    /// The results' disk cache, which follows the decode cache's folder.
    pub(in crate::app) fn ai_denoise_cache(&self) -> &AiCache {
        self.ai_denoise.cache()
    }
    /// An update from the service: kept when it is for the image on screen, which is
    /// rendered again with a new result.
    pub(in crate::app) fn ai_denoise_update(&mut self, update: Update) {
        let current = self
            .document
            .full()
            .is_some_and(|image| std::ptr::eq(update.plain.as_ptr(), Arc::as_ptr(image)));
        if !current {
            return;
        }
        // A final result is not replaced by a partial one: the Loupe's cached load
        // and a Develop run can both answer for one image.
        if matches!(update.state, State::Partial(_)) && self.document.ai.complete {
            return;
        }
        let shown = matches!(update.state, State::Partial(_) | State::Ready(_));
        if let State::Partial(source) | State::Ready(source) = &update.state {
            self.document.ai.source = Some(source.clone());
        }
        match &update.state {
            State::Ready(_) => {
                self.document.ai.complete = true;
                self.document.ai.requested = None;
            }
            State::Failed(_) => {
                self.document.ai.failed = true;
                self.document.ai.requested = None;
            }
            _ => {}
        }
        // A partial result is not shown once the final one will not come: the photo
        // renders plain, as without a result.
        let dropped = matches!(update.state, State::Failed(_))
            && !self.document.ai.complete
            && self.document.ai.source.take().is_some();
        self.document.ai.status = Some(update.state);
        if (shown || dropped) && self.document.edit.recipe().ai_denoise.renders() {
            self.schedule();
        }
    }
}

/// The user's AI Denoise cache as the app keeps it: in the folder `session`'s
/// Preferences chose for decoded images, or the default one.
fn user_cache(session: &crate::app::preferences::session::Session) -> AiCache {
    let cache = AiCache::default();
    cache.set_folder(session.decoded_cache_folder.as_deref());
    cache
}

/// The user's AI Denoise cache, for the command line: where the app keeps it.
pub fn user_ai_denoise_cache() -> AiCache {
    user_cache(&crate::app::preferences::session::load_session())
}

#[cfg(test)]
#[path = "ai_denoise_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "ai_denoise_editor_tests.rs"]
mod editor_tests;
