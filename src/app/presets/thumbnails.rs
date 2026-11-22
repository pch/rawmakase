//! The Presets panel's tiles in Grid view: each preset applied to the open photo
//! and rendered small, as a click would apply it (docs/xmp-presets.md#thumbnails).
//!
//! A library may hold a thousand presets, so only the tiles on screen, and a row
//! either side, are ever rendered, and only while nothing else wants the
//! machine: the main view's render is idle and nothing that changes every tile
//! has changed for [`QUIET`] seconds.
//!
//! The panel tells [`Thumbnails`] each frame what the tiles are rendered from
//! ([`Inputs`]) and which tiles it shows ([`Thumbnails::want`]). Any change to
//! the inputs (a new decode, an edit, the profiles, the monitor profile or the
//! preset library) starts a new *generation*: the batch rendering is cancelled,
//! and every tile is due again. A tile of an older generation stays on screen
//! until its replacement lands, so an edit doesn't blank the grid. A tile
//! rendered for another size (the panel resized) is drawn scaled meanwhile.
//!
//! The worker takes one batch at a time ([`Latest`]), with every input captured
//! when it was submitted. Each tile is applied and rendered on its own: a tile
//! that fails or panics reports its error and the rest of the batch goes on. A
//! batch cancelled part way reports nothing for the tile it was rendering, so
//! that tile is asked for again, never taken for a failure.
//!
//! Tiles are kept by preset id, at most [`CACHE_BYTES`] of them, least recently
//! drawn first out, until another photo opens.
use crate::app::worker::Latest;
use crate::camera_data::{CameraImage, Metadata};
use crate::camera_profiles::CameraProfile;
use crate::catalog::PhotoId;
use crate::model::recipe::Recipe;
use eframe::egui;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Weak};

/// How long, in seconds, nothing that renders the tiles again may change before
/// they render: while a slider moves they wait.
pub(in crate::app) const QUIET: f64 = 0.3;
/// The most tile pixels kept, in bytes of RGBA: about 190 tiles 290 pixels square.
pub(in crate::app) const CACHE_BYTES: usize = 64 << 20;

/// What the tiles are rendered from, as the panel sees it this frame.
pub(in crate::app) struct Inputs<'a> {
    /// The document's current decode.
    pub(in crate::app) image: &'a Arc<CameraImage>,
    /// The current edit, which each preset is applied over.
    pub(in crate::app) recipe: &'a Recipe,
    pub(in crate::app) metadata: &'a Metadata,
    pub(in crate::app) profiles: &'a [Arc<CameraProfile>],
    /// The display profile the tiles are converted for, as the main view's.
    pub(in crate::app) monitor: Option<PathBuf>,
    pub(in crate::app) library: &'a Arc<crate::presets::Library>,
    /// The photo, by its file and catalog entry: another photo's tiles are let go.
    pub(in crate::app) photo: (Option<PathBuf>, Option<PhotoId>),
}

/// What a tile shows.
pub(in crate::app) enum Tile<'a> {
    /// Nothing yet.
    Missing,
    /// Its render, possibly of an earlier generation or another size, shown
    /// until its replacement lands.
    Picture(&'a egui::TextureHandle),
    /// Why it could not be rendered.
    Failed(&'a str),
}

/// What the inputs were when the generation started.
struct Seen {
    image: Weak<CameraImage>,
    recipe: Recipe,
    profiles: Vec<Weak<CameraProfile>>,
    monitor: Option<PathBuf>,
    library: Weak<crate::presets::Library>,
}
impl Seen {
    fn of(inputs: &Inputs) -> Self {
        Self {
            image: Arc::downgrade(inputs.image),
            recipe: inputs.recipe.clone(),
            profiles: inputs.profiles.iter().map(Arc::downgrade).collect(),
            monitor: inputs.monitor.clone(),
            library: Arc::downgrade(inputs.library),
        }
    }
    /// Whether `inputs` renders every tile as these did.
    fn same(&self, inputs: &Inputs) -> bool {
        std::ptr::eq(self.image.as_ptr(), Arc::as_ptr(inputs.image))
            && std::ptr::eq(self.library.as_ptr(), Arc::as_ptr(inputs.library))
            && self.profiles.len() == inputs.profiles.len()
            && (self.profiles.iter().zip(inputs.profiles))
                .all(|(a, b)| std::ptr::eq(a.as_ptr(), Arc::as_ptr(b)))
            && self.monitor == inputs.monitor
            && self.recipe == *inputs.recipe
    }
}

enum Content {
    Picture(egui::TextureHandle),
    Failed(String),
}
struct Entry {
    generation: u64,
    /// The side it was rendered at, in pixels.
    edge: u32,
    content: Content,
    /// When it was last drawn, by [`Thumbnails::ticks`].
    used: u64,
}
impl Entry {
    fn bytes(&self) -> usize {
        match &self.content {
            Content::Picture(t) => t.size()[0] * t.size()[1] * 4,
            Content::Failed(_) => 0,
        }
    }
}

/// The batch last submitted, while it may still be rendering.
struct Batch {
    generation: u64,
    edge: u32,
    cancel: Arc<AtomicBool>,
    /// Its tiles not reported yet, in display order.
    outstanding: Vec<String>,
}

/// What the worker renders: a batch of tiles, everything captured once.
struct Job {
    generation: u64,
    edge: u32,
    cancel: Arc<AtomicBool>,
    image: Arc<CameraImage>,
    recipe: Recipe,
    metadata: Metadata,
    profiles: Vec<Arc<CameraProfile>>,
    monitor: Option<PathBuf>,
    library: Arc<crate::presets::Library>,
    /// Indices into `library` and their ids, in display order.
    tiles: Vec<(usize, String)>,
}

/// A tile's result as the worker reports it.
struct Outcome {
    generation: u64,
    edge: u32,
    id: String,
    result: Result<egui::ColorImage, String>,
}

/// Renders one tile; [`Renderer`], or a stand-in in tests.
pub(in crate::app) trait Render: Send + 'static {
    /// `preset` applied over the job's edit and rendered `job.edge` pixels
    /// square, honouring `cancel`.
    fn tile(
        &mut self,
        job: &JobView,
        preset: &crate::xmp::Preset,
        cancel: &AtomicBool,
    ) -> anyhow::Result<egui::ColorImage>;
    /// Lets go of what it keeps: after a panic, whose state may be inconsistent,
    /// and once the tiles are no longer shown.
    fn reset(&mut self);
}

/// What a tile is rendered from: the job's inputs.
pub(in crate::app) struct JobView<'a> {
    pub(in crate::app) image: &'a Arc<CameraImage>,
    pub(in crate::app) recipe: &'a Recipe,
    pub(in crate::app) metadata: &'a Metadata,
    pub(in crate::app) profiles: &'a [Arc<CameraProfile>],
    pub(in crate::app) monitor: Option<&'a std::path::Path>,
    pub(in crate::app) edge: u32,
}

/// What the worker is asked to do.
enum Work {
    Batch(Box<Job>),
    /// Let go of the renderer's reduced copy and caches.
    Release,
}

pub(in crate::app) struct Thumbnails {
    worker: Latest<Work>,
    inbox: Arc<Mutex<Vec<Outcome>>>,
    generation: u64,
    seen: Option<Seen>,
    photo: Option<(Option<PathBuf>, Option<PhotoId>)>,
    /// When anything that renders the tiles again last changed, in the UI's
    /// seconds (`egui::InputState::time`).
    changed: f64,
    /// The tiles' side in pixels this frame.
    edge: u32,
    entries: HashMap<String, Entry>,
    ticks: u64,
    batch: Option<Batch>,
    /// The tiles the frame being drawn shows, in display order.
    wanted: Vec<(usize, String)>,
    /// Whether the frame being drawn showed the grid ([`Self::sweep`]).
    asked: bool,
    /// Whether the worker was asked to let go since the grid last showed.
    released: bool,
}

impl Thumbnails {
    pub(in crate::app) fn new(ctx: egui::Context) -> Self {
        Self::with_render(ctx, Renderer::default())
    }
    pub(in crate::app) fn with_render(ctx: egui::Context, mut render: impl Render) -> Self {
        let inbox: Arc<Mutex<Vec<Outcome>>> = Arc::default();
        let out = inbox.clone();
        let worker = Latest::new(move |work: Work| match work {
            Work::Release => render.reset(),
            Work::Batch(job) => run(&mut render, &job, &out, &ctx),
        });
        Self {
            worker,
            inbox,
            generation: 0,
            seen: None,
            photo: None,
            changed: f64::NEG_INFINITY,
            edge: 0,
            entries: HashMap::new(),
            ticks: 0,
            batch: None,
            wanted: Vec::new(),
            asked: false,
            released: true,
        }
    }

    /// Stops the worker; the batch under way is cancelled and reports nothing.
    pub(in crate::app) fn stop(&mut self) -> crate::app::task::Stopping {
        self.cancel_batch();
        self.worker.stop()
    }

    /// Starts a frame of the grid at `now` (the UI's seconds), with tiles `edge`
    /// pixels square, rendered from `inputs` (`None` while no photo is decoded).
    /// Takes the tiles the worker finished, and starts a new generation when the
    /// inputs changed.
    pub(in crate::app) fn begin(
        &mut self,
        ctx: &egui::Context,
        now: f64,
        edge: u32,
        inputs: Option<&Inputs>,
    ) {
        self.asked = true;
        self.released = false;
        self.wanted.clear();
        self.collect(ctx);
        if edge != self.edge {
            // Tiles of the old size stay shown, scaled; none more are rendered.
            self.edge = edge;
            self.changed = now;
            self.cancel_batch();
        }
        let Some(inputs) = inputs else {
            // No photo decoded (none open, or the next one opening): placeholders,
            // never the last photo's tiles.
            self.cancel_batch();
            self.photo = None;
            self.entries.clear();
            return;
        };
        if self.photo.as_ref() != Some(&inputs.photo) {
            // Another photo's tiles never show.
            self.photo = Some(inputs.photo.clone());
            self.entries.clear();
        }
        if !self.seen.as_ref().is_some_and(|s| s.same(inputs)) {
            self.seen = Some(Seen::of(inputs));
            self.generation += 1;
            self.changed = now;
            self.cancel_batch();
        }
    }

    /// The preset at `index` of the library, `id`, shows this frame.
    pub(in crate::app) fn want(&mut self, index: usize, id: &str) {
        self.wanted.push((index, id.to_owned()));
    }

    /// What preset `id`'s tile shows.
    pub(in crate::app) fn tile(&mut self, id: &str) -> Tile<'_> {
        self.ticks += 1;
        let (generation, ticks) = (self.generation, self.ticks);
        let Some(entry) = self.entries.get_mut(id) else {
            return Tile::Missing;
        };
        entry.used = ticks;
        match &entry.content {
            Content::Picture(texture) => Tile::Picture(texture),
            // A failure of an earlier generation is retried.
            Content::Failed(_) if entry.generation != generation => Tile::Missing,
            Content::Failed(why) => Tile::Failed(why),
        }
    }

    /// Ends a frame of the grid: submits the shown tiles that are due, when the
    /// main view's render is `idle` and nothing has changed for [`QUIET`]
    /// seconds, and wakes the UI when that time comes.
    pub(in crate::app) fn end(
        &mut self,
        ctx: &egui::Context,
        now: f64,
        idle: bool,
        inputs: Option<&Inputs>,
    ) {
        self.trim();
        let Some(inputs) = inputs else {
            return;
        };
        let due: Vec<(usize, String)> = (self.wanted.iter())
            .filter(|(_, id)| self.due(id))
            .cloned()
            .collect();
        if due.is_empty() {
            // Everything shown is here: a batch still rendering others stops.
            self.cancel_batch();
            return;
        }
        let waited = now - self.changed;
        if waited < QUIET {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(QUIET - waited + 0.01));
            return;
        }
        // The main view's render repaints when it lands.
        if !idle {
            return;
        }
        let running = self.batch.as_ref().is_some_and(|b| {
            b.generation == self.generation
                && b.edge == self.edge
                && b.outstanding.iter().eq(due.iter().map(|(_, id)| id))
        });
        if running {
            return;
        }
        self.cancel_batch();
        let cancel = Arc::new(AtomicBool::new(false));
        self.batch = Some(Batch {
            generation: self.generation,
            edge: self.edge,
            cancel: cancel.clone(),
            outstanding: due.iter().map(|(_, id)| id.clone()).collect(),
        });
        self.worker.submit(Work::Batch(Box::new(Job {
            generation: self.generation,
            edge: self.edge,
            cancel,
            image: inputs.image.clone(),
            recipe: inputs.recipe.clone(),
            metadata: inputs.metadata.clone(),
            profiles: inputs.profiles.to_vec(),
            monitor: inputs.monitor.clone(),
            library: inputs.library.clone(),
            tiles: due,
        })));
    }

    /// After each frame: when it showed no grid (List view, the panel hidden,
    /// another tab or module), cancels the batch under way and has the worker let
    /// go of its copy of the photo. The tiles kept stay for when the grid shows
    /// again.
    pub(in crate::app) fn sweep(&mut self) {
        if std::mem::take(&mut self.asked) {
            return;
        }
        self.wanted.clear();
        self.cancel_batch();
        if !std::mem::replace(&mut self.released, true) {
            self.worker.submit(Work::Release);
        }
    }

    /// Shows `tiles` as rendered for `inputs` at `edge`, with the worker stopped
    /// so nothing else lands: for shots.
    #[cfg(test)]
    pub(in crate::app) fn show_for_test(
        &mut self,
        ctx: &egui::Context,
        inputs: &Inputs,
        edge: u32,
        tiles: Vec<(String, Result<egui::ColorImage, String>)>,
    ) {
        let _ = self.stop();
        self.photo = Some(inputs.photo.clone());
        self.seen = Some(Seen::of(inputs));
        self.edge = edge;
        let generation = self.generation;
        self.inbox
            .lock()
            .unwrap()
            .extend(tiles.into_iter().map(|(id, result)| Outcome {
                generation,
                edge,
                id,
                result,
            }));
        self.collect(ctx);
    }

    /// The ids of the tiles the frame drawn last showed, in order.
    #[cfg(test)]
    pub(in crate::app) fn wanted_ids(&self) -> Vec<String> {
        self.wanted.iter().map(|(_, id)| id.clone()).collect()
    }
    #[cfg(test)]
    pub(in crate::app) fn generation(&self) -> u64 {
        self.generation
    }

    /// Whether preset `id`'s tile is to be rendered: none, one of an earlier
    /// generation, or a picture of another size. A failure stands for its
    /// generation.
    fn due(&self, id: &str) -> bool {
        self.entries.get(id).is_none_or(|e| {
            e.generation != self.generation
                || matches!(e.content, Content::Picture(_)) && e.edge != self.edge
        })
    }

    /// Raises the cancel of the batch last submitted and forgets it, so the next
    /// frame that wants tiles submits them again.
    fn cancel_batch(&mut self) {
        if let Some(batch) = self.batch.take() {
            batch.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Takes the worker's tiles of the current generation; earlier ones are let go.
    fn collect(&mut self, ctx: &egui::Context) {
        let done = std::mem::take(
            &mut *self
                .inbox
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for outcome in done {
            if outcome.generation != self.generation {
                continue;
            }
            if let Some(batch) = &mut self.batch
                && batch.generation == outcome.generation
                && batch.edge == outcome.edge
            {
                batch.outstanding.retain(|id| *id != outcome.id);
            }
            let content = match outcome.result {
                Ok(image) => Content::Picture(ctx.load_texture(
                    format!("preset-thumbnail-{}", outcome.id),
                    image,
                    egui::TextureOptions::LINEAR,
                )),
                Err(why) => {
                    log::warn!("Preset thumbnail {} not rendered: {why}", outcome.id);
                    Content::Failed(why)
                }
            };
            self.ticks += 1;
            self.entries.insert(
                outcome.id,
                Entry {
                    generation: outcome.generation,
                    edge: outcome.edge,
                    content,
                    used: self.ticks,
                },
            );
        }
    }

    /// Lets go of the tiles drawn longest ago beyond [`CACHE_BYTES`], never one
    /// this frame shows.
    fn trim(&mut self) {
        let mut bytes: usize = self.entries.values().map(Entry::bytes).sum();
        if bytes <= CACHE_BYTES {
            return;
        }
        let shown: std::collections::HashSet<&str> =
            self.wanted.iter().map(|(_, id)| id.as_str()).collect();
        let mut old: Vec<(u64, String)> = (self.entries.iter())
            .filter(|(id, _)| !shown.contains(id.as_str()))
            .map(|(id, e)| (e.used, id.clone()))
            .collect();
        old.sort_unstable();
        for (_, id) in old {
            if bytes <= CACHE_BYTES {
                break;
            }
            if let Some(e) = self.entries.remove(&id) {
                bytes -= e.bytes();
            }
        }
    }
}

/// Renders `job`'s tiles in order, reporting each to `inbox`, until it is
/// cancelled. A cancelled tile reports nothing.
fn run(render: &mut impl Render, job: &Job, inbox: &Mutex<Vec<Outcome>>, ctx: &egui::Context) {
    let view = JobView {
        image: &job.image,
        recipe: &job.recipe,
        metadata: &job.metadata,
        profiles: &job.profiles,
        monitor: job.monitor.as_deref(),
        edge: job.edge,
    };
    for (index, id) in &job.tiles {
        if job.cancel.load(Ordering::Relaxed) {
            return;
        }
        let result = match job.library.presets.get(*index).filter(|p| p.id == *id) {
            None => Err("The preset is no longer in the library".to_string()),
            Some(preset) => std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                render.tile(&view, preset, &job.cancel)
            }))
            .unwrap_or_else(|panic| {
                render.reset();
                Err(anyhow::anyhow!(
                    "The renderer stopped unexpectedly: {}",
                    crate::app::worker::panic_message(&*panic)
                ))
            })
            .map_err(|e| format!("{e:#}")),
        };
        let mut inbox = inbox
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked after the render: a tile cancelled while it rendered reports
        // nothing. One that finished just before its cancel is still a result of
        // its generation, which the UI keeps.
        if job.cancel.load(Ordering::Relaxed) {
            return;
        }
        inbox.push(Outcome {
            generation: job.generation,
            edge: job.edge,
            id: id.clone(),
            result,
        });
        drop(inbox);
        ctx.request_repaint();
    }
}

/// The tiles' renderer: presets applied on the worker's own thread, at normal
/// priority (their Auto measures the whole photo, uncancellably), and rendered
/// on a low-priority pool of two threads, as the neighbour prefetch decodes.
#[derive(Default)]
pub(in crate::app) struct Renderer {
    thumbnails: Option<crate::develop::thumbnails::ThumbnailRenderer>,
    pool: Option<Option<rayon::ThreadPool>>,
}
impl Render for Renderer {
    fn tile(
        &mut self,
        job: &JobView,
        preset: &crate::xmp::Preset,
        cancel: &AtomicBool,
    ) -> anyhow::Result<egui::ColorImage> {
        use crate::develop::thumbnails::ThumbnailRenderer;
        let measures = crate::develop::Measures(job.image);
        let applied = crate::presets::apply_to_photo(
            preset,
            job.recipe,
            job.metadata,
            job.profiles,
            Some(&measures),
        )?;
        // Auto CA is measured here, at normal priority, before the slow pool.
        ThumbnailRenderer::prime(job.image, &applied.recipe);
        let thumbnails = self.thumbnails.get_or_insert_default();
        let cancel = rawmakase_cancel::Cancel::new(cancel);
        let mut render = || thumbnails.render(job.image, &applied.recipe, job.edge, cancel);
        let pool = self
            .pool
            .get_or_insert_with(|| crate::raw::background_pool(2, "preset-thumbnails").ok());
        let out = match pool {
            Some(pool) => pool.install(render)?,
            None => render()?,
        };
        Ok(square(&out, job.edge, job.monitor))
    }
    fn reset(&mut self) {
        self.thumbnails = None;
    }
}

/// The middle of `out`, at most `edge` pixels square, for `monitor`.
fn square(
    out: &crate::rendered::Rendered,
    edge: u32,
    monitor: Option<&std::path::Path>,
) -> egui::ColorImage {
    let side = edge.min(out.width).min(out.height).max(1);
    let (x0, y0) = ((out.width - side) / 2, (out.height - side) / 2);
    let mut rgb = Vec::with_capacity(side as usize * side as usize * 3);
    for y in y0..y0 + side {
        let row = (y * out.width + x0) as usize;
        for p in &out.pixels[row..row + side as usize] {
            rgb.extend(p.map(crate::rendered::unit_to_u8));
        }
    }
    // As the main view: a failed transform shows the sRGB pixels.
    if let Some(monitor) = monitor {
        let _ = crate::color_management::display_transform(monitor, &mut rgb);
    }
    egui::ColorImage::from_rgb([side as usize, side as usize], &rgb)
}

#[cfg(test)]
mod tests;
