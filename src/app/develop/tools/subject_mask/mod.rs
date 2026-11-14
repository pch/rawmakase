//! Select Subject, Sky and Background (docs/subject-and-background-masks.md),
//! and every other mask generator an extension offers: a model finds what to
//! mask, and the result becomes a mask that edits like any other. Everything
//! here is the app's side of that: what a request captures, when its result may
//! still be applied, and what the drawer says meanwhile.
//!
//! The generators are extensions' ([`mask_generator`]); Subject, Sky and
//! Background are the bundled `rawmakase.ai-masks` extension's, whose program
//! runs the models. A request has its input rendered and its answer kept on
//! the selection thread (`worker`), so neither does pixel work on the UI
//! thread. Aiming collects clicks and a box with
//! [`viewer_input`](crate::app::extensions::viewer_input). Saved masks, their
//! rasters and their labels are the core's, and render whether or not the
//! extension that made them is there.
mod ui;
mod worker;

pub(in crate::app) use ui::paint_feature_icon;
pub(in crate::app) use worker::{Worker, keep, render_input};

use crate::app::Editor;
use crate::app::extensions::mask_generator::{self, Offer};
use crate::app::extensions::viewer_input::Collector;
use crate::app::task::{Stopping, Task};
use crate::model::masks::{
    BITMAP_SAMPLING, BitmapMask, BitmapSource, FEATURE_SKY, FEATURE_SUBJECT, MAX_COMPONENTS,
    MAX_GROUPS, MaskComponent, MaskGroup, MaskOp, MaskShape,
};
use crate::model::recipe::Recipe;
use rawmakase_extensions::protocol::RemoteError;
use rawmakase_extensions::protocol::message::NeedsSetup;
use rawmakase_extensions::protocol::method::{MaskGenerate, MaskGenerated};
use rawmakase_extensions::protocol::viewer::ViewerInput;
use rawmakase_extensions::shm::Returned;
use serde_json::Value;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::model_install::Pack;

/// The selections that came before extensions, as their saved masks name them:
/// Subject, Sky, and Background, the same coverage as Subject with the
/// component inverted, so adding a brush to it still adds coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Feature {
    Subject,
    Sky,
    Background,
}
/// Which selection a generated component is, from where it came from and whether it
/// is inverted (Background is an inverted subject).
pub(in crate::app) fn feature_of(b: &BitmapMask, invert: bool) -> Feature {
    match b.source.as_ref().map(|s| s.feature.as_str()) {
        Some(FEATURE_SKY) => Feature::Sky,
        _ if invert => Feature::Background,
        _ => Feature::Subject,
    }
}

/// What a finished selection does to the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Target {
    /// A new mask holding the selection.
    NewMask,
    /// A component of the selected mask, combined with `op`.
    Component { mask: usize, op: MaskOp },
    /// Replace the raster of one generated component, keeping everything else.
    Regenerate { mask: usize, component: usize },
}

/// A request, as it was made: the generator it asks, through one of its
/// tiles or as the one a saved raster names, and what its result does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Request {
    pub(in crate::app) generator: Offer,
    pub(in crate::app) target: Target,
}

/// Why a selection did not make a mask; the drawer says so with a way to try again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Cancelled,
    /// The model found nothing; no mask is made.
    NoSubject,
    NoSky,
    /// Nothing at the clicked place.
    NothingThere,
    /// An extension's generator found nothing by itself.
    NothingFound,
    /// The generator has to be set up (its model downloaded) first.
    NeedsSetup(NeedsSetup),
    /// The inference runtime could not be loaded or has no support here.
    RuntimeUnavailable(String),
    Failed(String),
}
impl Failure {
    fn message(&self) -> String {
        match self {
            Self::Cancelled => "Cancelled".into(),
            Self::NoSubject => "No person or animal found".into(),
            Self::NoSky => "No sky found".into(),
            Self::NothingThere => "Nothing selected there; click on the subject itself".into(),
            Self::NothingFound => "Nothing found".into(),
            Self::NeedsSetup(setup) => format!("{} has to be downloaded first", setup.what),
            Self::RuntimeUnavailable(why) => format!("Selection is unavailable here: {why}"),
            Self::Failed(why) => format!("Selection failed: {why}"),
        }
    }
}

/// A raster a selection produced, registered in the mask asset store.
#[derive(Debug)]
pub(crate) struct Generated {
    pub(crate) id: String,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) source: BitmapSource,
}

/// A selection's outcome, tagged with what it was started for.
pub(crate) struct Done {
    /// The photo's load, and the task generation, it was started under.
    pub(crate) load: u64,
    pub(crate) generation: u64,
    pub(crate) result: Result<Generated, Failure>,
}

/// What must still hold when a result arrives for it to be applied.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) struct Guard {
    /// The masks' structure: how many, with which components. Any addition, removal,
    /// reordering or replacement (Undo, a snapshot) changes it; sliders, names and
    /// visibility do not.
    structure: u64,
    /// The spots and red eye the model saw. Different ones select different content.
    content: u64,
    /// For a regeneration, the component's shape when it was asked for.
    expected: Option<MaskShape>,
}
impl Guard {
    fn of(recipe: &Recipe, target: Target) -> Self {
        let expected = match target {
            Target::Regenerate { mask, component } => recipe
                .masks
                .get(mask)
                .and_then(|m| m.components.get(component))
                .map(|c| c.shape.clone()),
            _ => None,
        };
        Self {
            structure: structure(&recipe.masks),
            content: content(recipe),
            expected,
        }
    }
    fn holds(&self, recipe: &Recipe, target: Target) -> bool {
        let same_shape = match target {
            Target::Regenerate { mask, component } => {
                recipe
                    .masks
                    .get(mask)
                    .and_then(|m| m.components.get(component))
                    .map(|c| &c.shape)
                    == self.expected.as_ref()
                    && self.expected.is_some()
            }
            _ => true,
        };
        same_shape && self.structure == structure(&recipe.masks) && self.content == content(recipe)
    }
}
/// The masks' structure, not their values.
fn structure(masks: &[MaskGroup]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    masks.len().hash(&mut h);
    for g in masks {
        g.invert.hash(&mut h);
        g.components.len().hash(&mut h);
        for c in &g.components {
            // Every shape value and how it combines: Undo or a snapshot that puts a
            // different mask of the same kind at the same place is a different mask.
            serde_json::to_string(&(&c.shape, c.op, c.invert))
                .unwrap_or_default()
                .hash(&mut h);
        }
    }
    h.finish()
}
/// The bounding box (fractions of the photo) of a raster's selected pixels.
fn raster_extent(id: &str) -> Option<[f32; 4]> {
    let raster = crate::storage::mask_assets::resolve_coverage(id).ok()?;
    let (w, h) = (raster.width as usize, raster.height as usize);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, v) in raster.data.iter().enumerate() {
        if *v >= 128 {
            let (x, y) = (i % w, i / w);
            x0 = x0.min(x);
            x1 = x1.max(x + 1);
            y0 = y0.min(y);
            y1 = y1.max(y + 1);
        }
    }
    (x1 > x0 && y1 > y0).then(|| {
        [
            x0 as f32 / w as f32,
            y0 as f32 / h as f32,
            x1 as f32 / w as f32,
            y1 as f32 / h as f32,
        ]
    })
}
/// What the model is given besides pixels the settings do not change: spots and red
/// eye, which alter the content to select (and the content AI Remove regenerates
/// from).
pub(in crate::app) fn content(recipe: &Recipe) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_string(&(
        &recipe.retouch,
        &recipe.red_eye,
        recipe.camera_exposure.to_bits(),
        [
            crate::model::panels::Panel::SpotRemoval,
            crate::model::panels::Panel::RedEye,
        ]
        .map(|panel| recipe.panels.state(panel) == crate::model::panels::PanelState::On),
    ))
    .unwrap_or_default()
    .hash(&mut h);
    h.finish()
}

struct Pending {
    request: Request,
    guard: Guard,
    /// From its input rendered until its answer is kept.
    call: Call,
}

/// What a request was started under, and what it asked once sent.
struct Call {
    load: u64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    /// The extension's lifecycle when it started ([`Host::mask_lifecycle`]):
    /// once it has moved (disabled, uninstalled, loaded again, crashed), the
    /// request is void in whatever phase it is.
    ///
    /// [`Host::mask_lifecycle`]: crate::app::extensions::Host::mask_lifecycle
    lifecycle: u64,
    /// The host's ticket for it and what it asked, from when it is sent until
    /// its outcome arrives.
    sent: Option<(u64, MaskGenerate)>,
}

/// What the drawer asks the user before a selection can run.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Prompt {
    /// The catalog has to be upgraded to keep the result.
    Upgrade(Request),
    /// An extension's generator answered `needs_setup`: what it needs, the
    /// guard the request was first made with, which must still hold for it to
    /// go on once set up, and what happens once it is.
    Setup {
        request: Request,
        setup: NeedsSetup,
        guard: Guard,
        then: Resume,
    },
}

/// What a request that waited for its generator's setup does once set up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum Resume {
    /// Runs again, automatic as it was.
    Select,
    /// Goes back to aiming, as it was being aimed.
    Aim,
    /// Nothing: it was being aimed, and the user has left the Mask tool
    /// since. The generator is ready the next time they aim.
    Nothing,
}
impl Prompt {
    fn request(&self) -> &Request {
        match self {
            Self::Upgrade(request) | Self::Setup { request, .. } => request,
        }
    }
}

/// A selection being aimed: the clicks and box so far. The first one makes the mask;
/// each further one refines that same mask until the user is done.
pub(in crate::app) struct Prompting {
    pub(in crate::app) request: Request,
    pub(in crate::app) input: Collector,
    /// The mask and component the first result made (or the one being regenerated).
    applied: Option<(usize, usize)>,
}

#[derive(Default)]
pub(in crate::app) struct Selection {
    /// Waiting for clicks on the photo.
    pub(in crate::app) prompting: Option<Prompting>,
    /// The running selection's generation; a newer request or a new photo supersedes.
    task: Task,
    pending: Option<Pending>,
    /// What the last attempt ended in, for the drawer, until the next.
    failure: Option<(Request, Failure)>,
    prompt: Option<Prompt>,
    worker: worker::Worker,
    /// A catalog upgrade under way, and the request it unblocks.
    upgrading: Option<(u64, Request)>,
    upgrade_task: Task,
    /// Its thread, to wait for when quitting: cut off, it would leave a partial backup.
    upgrade_thread: Option<std::thread::JoinHandle<()>>,
    /// Rasters of results not taken while another selection was running, to let go of
    /// once it has finished unless it took the same one.
    deferred: Vec<String>,
    /// Extensions' requests given up on, by extension and ticket, for the host
    /// to cancel at its next poll.
    withdrawn: Vec<(String, u64)>,
}

impl Selection {
    /// Whether a selection is running.
    pub(in crate::app) fn running(&self) -> Option<&Request> {
        self.pending.as_ref().map(|p| &p.request)
    }
    /// Stops the running selection, whatever it is for (a new photo, Cancel).
    /// A request in flight is withdrawn: its answer can't apply any more, and
    /// the host cancels it at the program.
    pub(in crate::app) fn cancel(&mut self) {
        self.task.invalidate();
        if let Some(Pending {
            request,
            call: Call {
                sent: Some((ticket, _)),
                ..
            },
            ..
        }) = self.pending.take()
        {
            self.withdrawn.push((request.generator.extension, ticket));
        }
    }
    /// The extensions' requests to cancel at the program.
    pub(in crate::app) fn take_withdrawn(&mut self) -> Vec<(String, u64)> {
        std::mem::take(&mut self.withdrawn)
    }
    /// Stops aiming (Done, Escape, a new photo).
    pub(in crate::app) fn end_prompting(&mut self) {
        self.prompting = None;
    }
    /// The photo changed: nothing asked for the previous one applies, an upgrade under
    /// way included; it finishes, but does not go on to select in this photo. Its
    /// thread is still waited for at exit.
    pub(in crate::app) fn clear_document(&mut self) {
        self.cancel();
        self.end_prompting();
        self.failure = None;
        self.prompt = None;
        self.upgrading = None;
        self.upgrade_task.invalidate();
    }
    /// The catalog changed.
    pub(in crate::app) fn clear_catalog(&mut self) {
        self.clear_document();
    }
    /// Threads to wait for when quitting: the selection thread is handed to the
    /// shared deadline, never joined without one. An upgrade's backup copy is
    /// finished within that deadline too, rather than cut off. The programs
    /// that run the models are the extension host's to stop.
    pub(in crate::app) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        self.end_prompting();
        vec![
            self.worker.stop(),
            Stopping::new(self.upgrade_thread.take()),
        ]
    }
}

impl Editor {
    /// Why Subject and Background cannot be made now, for the drawer to say.
    pub(in crate::app) fn selection_unavailable(&self) -> Option<&'static str> {
        if self.library.is_none() || self.document.catalog_photo.is_none() {
            return Some("Add this photo to a catalog to use AI masks");
        }
        if self.document.full().is_none() {
            return Some("Available once the photo has finished developing");
        }
        if self.document.edit.save_state().is_protected() {
            return Some("This edit is protected and cannot be changed here");
        }
        None
    }
    /// Starts a selection, or asks what it needs first: an upgraded catalog,
    /// an extension that can run. What its generator needs set up it says
    /// when asked.
    pub(in crate::app) fn request_selection(&mut self, request: Request) {
        // The frame that shows what a click did comes after it: ask for one.
        self.context.request_repaint();
        // A second click on a tile while its selection runs is not another request.
        if self.selection.pending.is_some() {
            return;
        }
        if let Some(why) = self.selection_unavailable() {
            self.messages.warn(why);
            return;
        }
        let masks = &self.document.edit.recipe().masks;
        let room = match request.target {
            Target::NewMask => masks.len() < MAX_GROUPS,
            Target::Component { mask, .. } => masks
                .get(mask)
                .is_some_and(|m| m.components.len() < MAX_COMPONENTS),
            Target::Regenerate { .. } => true,
        };
        if !room {
            self.messages
                .warn(format!("A photo holds up to {MAX_GROUPS} masks"));
            return;
        }
        let upgraded = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog().supports_raster_masks().ok())
            .unwrap_or(false);
        if !upgraded {
            self.selection.prompt = Some(Prompt::Upgrade(request));
            return;
        }
        if let Err(why) = self.extensions.mask_ready(&request.generator) {
            self.messages.warn(why);
            return;
        }
        // A generator that only aims is aimed with.
        if !request.generator.takes_none() {
            self.begin_aiming(request);
            return;
        }
        self.selection.prompt = None;
        self.selection.failure = None;
        // The models find the subject and the sky by themselves; clicks only refine.
        self.start_selection(request, None);
    }
    /// Whether `generator` can be aimed with now: it takes clicks, and its
    /// extension can run. What its models need it says at the first click.
    fn can_aim(&self, generator: &Offer) -> bool {
        generator.takes_prompt() && self.extensions.mask_ready(generator).is_ok()
    }
    /// Aims at the subject by hand, for what the models do not know (a sign, a car, a
    /// rocket): the clicks make the mask, a new one or the one `request` names.
    pub(in crate::app) fn begin_aiming(&mut self, request: Request) {
        self.context.request_repaint();
        if self.selection_unavailable().is_some() || !self.can_aim(&request.generator) {
            self.request_selection(request);
            return;
        }
        self.selection.failure = None;
        let hint = request.generator.aim_hint();
        self.selection.prompting = Some(Prompting {
            applied: match request.target {
                Target::Regenerate { mask, component } => Some((mask, component)),
                _ => None,
            },
            request,
            input: Collector::default(),
        });
        self.view.set_tool(crate::app::develop::state::Tool::Mask);
        self.messages.note(hint);
    }
    /// Aims at a generated component: clicks on the photo add to it and leave things
    /// out of it, each one refining the same mask. Only a generator that takes
    /// clicks refines; one that can't run now says why.
    pub(in crate::app) fn begin_refining(&mut self, mask: usize, component: usize) {
        self.context.request_repaint();
        let Some(c) = self
            .document
            .edit
            .recipe()
            .masks
            .get(mask)
            .and_then(|m| m.components.get(component))
        else {
            return;
        };
        let MaskShape::Bitmap(b) = &c.shape else {
            return;
        };
        let generator = match self.raster_generator(b, c.invert) {
            Ok(generator) => generator,
            Err(why) => {
                self.messages.warn(why);
                return;
            }
        };
        if !generator.takes_prompt() {
            return;
        }
        let request = Request {
            generator,
            target: Target::Regenerate { mask, component },
        };
        if self.selection_unavailable().is_some() || !self.can_aim(&request.generator) {
            self.request_selection(request);
            return;
        }
        self.selection.failure = None;
        self.selection.prompting = Some(Prompting {
            request,
            input: Collector::default(),
            applied: Some((mask, component)),
        });
        self.view.set_tool(crate::app::develop::state::Tool::Mask);
        self.messages
            .note("Click the part of the photo to add, Alt-click to leave out");
    }
    /// Aiming belongs to the Mask tool: once Develop switches to another
    /// (Crop, a key, a panel's tool), aiming has ended, and going back to Mask
    /// doesn't take up the old clicks again. Runs every pass.
    /// Aiming that waits for its generator's setup ends the same way: the
    /// download goes on, but once it is done aiming doesn't start again over
    /// the tool the user switched to.
    pub(in crate::app) fn end_aiming_off_mask(&mut self) {
        if self.view.tool() == crate::app::develop::state::Tool::Mask {
            return;
        }
        if self.selection.prompting.is_some() {
            self.selection.end_prompting();
        }
        if let Some(Prompt::Setup { then, .. }) = &mut self.selection.prompt
            && *then == Resume::Aim
        {
            *then = Resume::Nothing;
        }
    }
    /// The generator a generated component's raster came from, by the
    /// provenance it saved: Subject, Sky and Background (an inverted Subject)
    /// are the bundled AI masks extension's, as are rasters that saved none,
    /// which only Select Subject made. `Err` says why that one can't run now
    /// (its extension disabled, Failed or missing); the mask renders all the
    /// same.
    pub(in crate::app) fn raster_generator(
        &self,
        b: &BitmapMask,
        invert: bool,
    ) -> Result<Offer, String> {
        let provenance = b
            .source
            .as_ref()
            .map_or(FEATURE_SUBJECT, |s| s.feature.as_str());
        self.extensions.mask_generator(provenance, invert)
    }
    /// Where a mask's raster came from, for the component settings: Subject's
    /// and Sky's as [`source_text`] says, from their saved provenance alone,
    /// another extension's generator by name, whether it is installed or not.
    pub(in crate::app) fn raster_source_text(&self, b: &BitmapMask) -> String {
        match b.source.as_ref().map(|s| s.feature.as_str()) {
            None | Some(FEATURE_SUBJECT | FEATURE_SKY) => source_text(b),
            Some(provenance) => self.extensions.mask_source_text(provenance),
        }
    }
    /// A click on the photo while aiming: a point on the object (or, with `positive`
    /// false, on something to leave out). Starts or refines the selection.
    pub(in crate::app) fn prompt_click(&mut self, at: [f32; 2], positive: bool) {
        self.context.request_repaint();
        if self.selection.pending.is_some() {
            return;
        }
        let masks = &self.document.edit.recipe().masks;
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        // A negative point means nothing without something to be negative about: when
        // refining a mask already made, that is the mask's own extent.
        let applied = p.applied;
        let extent = || {
            let (m, c) = applied?;
            match &masks.get(m)?.components.get(c)?.shape {
                MaskShape::Bitmap(b) => raster_extent(&b.id),
                _ => None,
            }
        };
        if p.input.click(at, positive, extent) {
            self.run_prompt();
        }
    }
    /// A box dragged on the photo while aiming (image space corners).
    pub(in crate::app) fn prompt_box(&mut self, a: [f32; 2], b: [f32; 2]) {
        if self.selection.pending.is_some() {
            return;
        }
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        if p.input.drag(a, b) {
            self.run_prompt();
        }
    }
    /// Forgets the clicks and box, so the next click starts the selection over (the
    /// mask made so far is replaced by it).
    pub(in crate::app) fn clear_prompt(&mut self) {
        if let Some(p) = &mut self.selection.prompting {
            p.input.clear();
        }
    }
    /// Runs the model on the clicks and box so far.
    pub(in crate::app) fn run_prompt(&mut self) {
        let Some(p) = &self.selection.prompting else {
            return;
        };
        let request = Request {
            generator: p.request.generator.clone(),
            target: match p.applied {
                Some((mask, component)) => Target::Regenerate { mask, component },
                None => p.request.target,
            },
        };
        let input = p.input.input();
        self.selection.failure = None;
        self.start_selection(request, Some(input));
    }
    /// Starts `request`, automatic or with the clicks and box of `input`: the
    /// worker thread renders its input, which is then handed back to be sent.
    fn start_selection(&mut self, request: Request, input: Option<ViewerInput>) {
        let Some(image) = self.document.full().cloned() else {
            return;
        };
        let load = self.document.load_id();
        let offer = request.generator.clone();
        let (Some(ledger), Some(lifecycle)) = (
            self.extensions.mask_ledger(&offer.extension),
            self.extensions.mask_lifecycle(&offer.extension),
        ) else {
            self.messages
                .warn(format!("{} isn't installed", offer.extension));
            return;
        };
        self.context.request_repaint();
        let text: String = match &input {
            None => format!("Finding the {}…", request.generator.name().to_lowercase()),
            Some(_) => "Selecting…".into(),
        };
        self.messages.note(text);
        let (generation, cancel) = self.selection.task.start();
        let recipe = self.document.edit.recipe().clone();
        self.selection.pending = Some(Pending {
            guard: Guard::of(&recipe, request.target),
            request,
            call: Call {
                load,
                generation,
                cancel: cancel.clone(),
                lifecycle,
                sent: None,
            },
        });
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let job = mask_generator::Prepare {
            load,
            generation,
            cancel,
            image,
            recipe,
            ledger,
            generator: offer.generator,
            input,
        };
        self.selection.worker.call(
            move || mask_generator::prepare(job),
            move || mask_generator::done(load, generation, Err(stopped())),
            tx,
            ctx,
        );
    }
    /// A request has its input rendered: it is sent, unless it was given up
    /// on meanwhile, when its buffers are let go of.
    pub(in crate::app) fn mask_prepared(&mut self, prepared: mask_generator::Prepared) {
        let (load, generation) = (prepared.load, prepared.generation);
        if load != self.document.load_id() || generation != self.selection.task.id() {
            return self.settle_deferred();
        }
        let Some(Pending {
            request,
            call: Call { sent: None, .. },
            ..
        }) = &self.selection.pending
        else {
            return self.settle_deferred();
        };
        let offer = request.generator.clone();
        if self.void_if_moved() {
            return;
        }
        let ready = match prepared.result {
            Ok(ready) => ready,
            Err(failure) => {
                return self.selection_done(Done {
                    load,
                    generation,
                    result: Err(failure),
                });
            }
        };
        let asked = ready.params.clone();
        match self
            .extensions
            .generate_mask(&offer, ready.params, ready.buffers)
        {
            Ok(ticket) => {
                if let Some(pending) = self.selection.pending.as_mut() {
                    pending.call.sent = Some((ticket, asked));
                }
            }
            Err(why) => self.selection_done(Done {
                load,
                generation,
                result: Err(Failure::Failed(why)),
            }),
        }
    }
    /// The one outcome of request `ticket`: its answer is kept
    /// as a raster on the worker thread, its failure ends the selection. An
    /// outcome nothing waits for any more (cancelled, superseded, another
    /// photo) is dropped, and its buffers with it.
    pub(in crate::app) fn mask_answered(
        &mut self,
        extension: &str,
        ticket: u64,
        result: Result<Value, RemoteError>,
        buffers: Option<Returned>,
    ) {
        let Some(Pending { request, call, .. }) = &mut self.selection.pending else {
            return;
        };
        let offer = &request.generator;
        if offer.extension != extension || call.sent.as_ref().is_none_or(|(t, _)| *t != ticket) {
            return self.settle_deferred();
        }
        let Some((_, asked)) = call.sent.take() else {
            return;
        };
        let (load, generation, cancel) = (call.load, call.generation, call.cancel.clone());
        let provenance = offer.provenance.clone();
        let answer = result.map_err(mask_generator::failure).and_then(|value| {
            let answer: MaskGenerated = serde_json::from_value(value)
                .map_err(|e| Failure::Failed(format!("the extension's answer: {e}")))?;
            answer
                .validate()
                .map_err(|e| Failure::Failed(format!("the extension's answer: {}", e.message)))?;
            Ok(answer)
        });
        let (answer, buffers) = match (answer, buffers) {
            (Ok(answer), Some(buffers)) => (answer, buffers),
            (result, _) => {
                let failure = result.err().unwrap_or_else(|| {
                    Failure::Failed("the extension answered without its coverage".into())
                });
                return self.selection_done(Done {
                    load,
                    generation,
                    result: Err(failure),
                });
            }
        };
        let job = mask_generator::Finish {
            load,
            generation,
            cancel,
            asked,
            answer,
            buffers,
            provenance,
        };
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        self.selection.worker.call(
            move || mask_generator::finish(job),
            move || mask_generator::done(load, generation, Err(stopped())),
            tx,
            ctx,
        );
    }
    /// After the host's outcomes: a request whose extension moved on in its
    /// lifecycle is void in any phase, and one whose outcome
    /// can't come any more (its request is gone from the host) ends now.
    /// Aiming at a generator that can't run any more (disabled, Failed, gone)
    /// ends, and rasters set aside for a selection that ended without a
    /// result are let go of.
    pub(in crate::app) fn check_mask_requests(&mut self) {
        if self.void_if_moved() {
            return;
        }
        if let Some(Pending {
            request,
            call:
                Call {
                    sent: Some((ticket, _)),
                    load,
                    generation,
                    ..
                },
            ..
        }) = &self.selection.pending
            && !self
                .extensions
                .mask_in_flight(&request.generator.extension, *ticket)
        {
            let offer = &request.generator;
            let done = Done {
                load: *load,
                generation: *generation,
                result: Err(Failure::Failed(format!(
                    "{} stopped before it answered",
                    offer.extension
                ))),
            };
            self.selection_done(done);
        }
        if let Some(Prompting { request, .. }) = &self.selection.prompting
            && let Err(why) = self.extensions.mask_ready(&request.generator)
        {
            self.selection.end_prompting();
            self.messages.note(why);
        }
        self.settle_deferred();
    }
    /// Ends the running request at once if its extension has
    /// moved on in its lifecycle since it started: busy clears, the worker's
    /// job is cancelled, a request sent is withdrawn (its buffers stay leased
    /// until the program answers) and nothing is recorded. Returns whether it
    /// did.
    fn void_if_moved(&mut self) -> bool {
        let Some(Pending { request, call, .. }) = &self.selection.pending else {
            return false;
        };
        let offer = &request.generator;
        let now = self.extensions.mask_lifecycle(&offer.extension);
        if now == Some(call.lifecycle) {
            return false;
        }
        let name = offer.extension.clone();
        self.selection.cancel();
        self.messages
            .note(format!("Selection stopped: {name} was stopped"));
        self.settle_deferred();
        true
    }
    /// Lets go of the rasters set aside while a selection ran, once none runs:
    /// each is discarded unless the edit, its History or Before names it.
    fn settle_deferred(&mut self) {
        if self.selection.pending.is_none() {
            for id in std::mem::take(&mut self.selection.deferred) {
                self.discard_raster(&id);
            }
        }
    }
    /// The setup card's Download: asks the generator's extension to set it up.
    pub(in crate::app) fn setup_generator(&mut self, offer: &Offer) {
        self.context.request_repaint();
        if let Err(why) = self
            .extensions
            .setup_mask(&offer.extension, &offer.generator)
        {
            self.messages.fail(why);
        }
    }
    /// A generator's setup finished: the request that asked for it goes on,
    /// automatic or aimed as it was.
    pub(in crate::app) fn mask_setup_done(
        &mut self,
        extension: &str,
        generator: &str,
        result: Result<Value, RemoteError>,
    ) {
        let waiting = match &self.selection.prompt {
            Some(Prompt::Setup {
                request,
                setup,
                guard,
                then,
            }) if request.generator.extension == extension
                && request.generator.generator == generator =>
            {
                Some((request.clone(), setup.what.clone(), guard.clone(), *then))
            }
            _ => None,
        };
        match result {
            Ok(_) => {
                let what = waiting.as_ref().map_or("The model", |(_, what, ..)| what);
                self.messages.done(format!("{what} installed"));
                if let Some((request, _, guard, then)) = waiting {
                    self.selection.prompt = None;
                    // Indices name the mask it was asked for only while the
                    // masks are as they were; a new mask needs no such thing.
                    let target = request.target;
                    if target != Target::NewMask
                        && !guard.holds(self.document.edit.recipe(), target)
                    {
                        self.messages.warn(format!(
                            "The masks changed while setting up; choose where to use {} again",
                            request.generator.name()
                        ));
                    } else {
                        // The tool is checked here too, not only by each
                        // pass's cleanup: a setup finishing in the pass the
                        // user left Mask in is handled before the cleanup.
                        let on_mask = self.view.tool() == crate::app::develop::state::Tool::Mask;
                        match then {
                            Resume::Nothing => {}
                            Resume::Aim if !on_mask => {}
                            Resume::Aim => self.begin_aiming(request),
                            Resume::Select if !request.generator.takes_none() => {
                                self.begin_aiming(request);
                            }
                            Resume::Select => self.request_selection(request),
                        }
                    }
                }
            }
            Err(e) if e.code == rawmakase_extensions::protocol::ErrorCode::Cancelled => {
                self.messages.note("Download cancelled");
            }
            Err(e) => self.messages.fail(format!("Not set up: {}", e.message)),
        }
    }
    pub(in crate::app) fn cancel_selection(&mut self) {
        self.context.request_repaint();
        self.selection.cancel();
        self.settle_deferred();
        self.messages.note("Selection cancelled");
    }
    /// A selection finished: apply it if it is still the one wanted and the edit it
    /// was made for still stands, else drop it.
    pub(in crate::app) fn selection_done(&mut self, done: Done) {
        self.apply_done(done);
        // Results set aside while this one ran: let go of the ones nothing took.
        self.settle_deferred();
    }
    fn apply_done(&mut self, done: Done) {
        let result = done.result;
        if done.load != self.document.load_id() || done.generation != self.selection.task.id() {
            self.discard_generated(result.ok());
            return;
        }
        self.selection.task.finish(done.generation);
        let Some(pending) = self.selection.pending.take() else {
            self.discard_generated(result.ok());
            return;
        };
        let request = pending.request;
        let generated = match result {
            Ok(generated) => generated,
            Err(Failure::Cancelled) => return,
            Err(Failure::NeedsSetup(setup)) => {
                let then = if self.selection.prompting.is_some() {
                    Resume::Aim
                } else {
                    Resume::Select
                };
                self.selection.end_prompting();
                self.selection.prompt = Some(Prompt::Setup {
                    request,
                    setup,
                    guard: pending.guard,
                    then,
                });
                return;
            }
            Err(failure) => {
                self.messages.fail(failure.message());
                self.selection.failure = Some((request, failure));
                return;
            }
        };
        // An extension disabled, uninstalled or loaded again since: its
        // answer is void, even one that came before.
        if self.extensions.mask_lifecycle(&request.generator.extension)
            != Some(pending.call.lifecycle)
        {
            self.discard_generated(Some(generated));
            return;
        }
        if !pending
            .guard
            .holds(self.document.edit.recipe(), request.target)
        {
            self.messages
                .warn("The masks changed while selecting; nothing was added");
            self.discard_generated(Some(generated));
            return;
        }
        // The rasters this would add, with every other the edit and its History refer to.
        let edit = &self.document.edit;
        let history = edit.history().saved(edit.recipe());
        let held = history.mask_asset_ids();
        let ids: Vec<&str> = edit
            .recipe()
            .mask_asset_ids()
            .chain(held.iter().copied())
            .chain([generated.id.as_str()])
            .collect();
        let bytes = crate::storage::mask_assets::decoded_bytes(ids);
        if bytes > crate::storage::mask_assets::EDIT_BYTES {
            self.messages
                .warn("This photo's masks use too much memory for another selection");
            self.discard_generated(Some(generated));
            return;
        }
        let masks = self.document.edit.recipe().masks.len();
        let id = generated.id.clone();
        let shape = MaskShape::Bitmap(BitmapMask {
            id: generated.id,
            width: generated.width,
            height: generated.height,
            sampling: BITMAP_SAMPLING,
            source: Some(generated.source),
        });
        let name = request.generator.name().to_string();
        let invert = request.generator.invert;
        let step = match request.target {
            Target::Regenerate { .. } => format!("Refine {name}"),
            _ => format!("Select {name}"),
        };
        let selected =
            self.change_edit(
                Some(crate::app::history::Step::new(step, "")),
                |r| match request.target {
                    Target::NewMask => {
                        let mut component = MaskComponent::new(shape);
                        component.invert = invert;
                        r.masks.push(MaskGroup {
                            name: name.clone(),
                            components: vec![component],
                            ..Default::default()
                        });
                        Some((r.masks.len() - 1, 0))
                    }
                    Target::Component { mask, op } => {
                        let group = r.masks.get_mut(mask)?;
                        let mut component = MaskComponent::new(shape);
                        component.op = op;
                        component.invert = invert;
                        group.components.push(component);
                        Some((mask, group.components.len() - 1))
                    }
                    Target::Regenerate { mask, component } => {
                        let c = r.masks.get_mut(mask)?.components.get_mut(component)?;
                        c.shape = shape;
                        Some((mask, component))
                    }
                },
            );
        debug_assert!(masks <= MAX_GROUPS);
        if selected.is_none() {
            // The mask it was for is gone: nothing refers to the raster.
            self.discard_raster(&id);
        }
        if let Some((mask, component)) = selected {
            if let Some(p) = &mut self.selection.prompting {
                p.applied = Some((mask, component));
            }
            self.view.masking.select_after_selection(mask, component);
            // The render the edit started was scheduled before the new mask was
            // selected, so it draws no overlay; with neutral sliders nothing else would
            // show the mask until some later change. Render again, now with it.
            self.schedule();
            self.messages.note(format!("{name} selected"));
        }
    }
    /// Forgets the raster of a result the edit did not take, so results the user never
    /// sees (superseded, stale, over budget) do not stay in memory.
    fn discard_generated(&mut self, generated: Option<Generated>) {
        if let Some(generated) = generated {
            self.discard_raster(&generated.id);
        }
    }
    /// Forgets an unsaved raster unless the edit, its History or Before still names it
    /// (the same coverage selected twice has one ID).
    fn discard_raster(&mut self, id: &str) {
        // A selection still running may produce the same coverage, and so the same ID:
        // decided once it has finished.
        if self.selection.pending.is_some() {
            self.selection.deferred.push(id.to_string());
            return;
        }
        let edit = &self.document.edit;
        let named = edit.recipe().mask_asset_ids().any(|i| i == id)
            || edit
                .history()
                .saved(edit.recipe())
                .mask_asset_ids()
                .contains(id)
            || self
                .document
                .before
                .as_ref()
                .is_some_and(|r| r.mask_asset_ids().any(|i| i == id));
        if !named {
            crate::storage::mask_assets::discard_unsaved(id);
        }
    }
    /// AI Denoise's installer finished: the photo shown asks for its result.
    pub(in crate::app) fn model_installed(
        &mut self,
        pack: Pack,
        result: Result<(), crate::model_install::InstallError>,
    ) {
        match (pack, result) {
            (Pack::Denoise, Ok(())) => {
                self.messages.done("AI Denoise model installed");
                self.document.ai = Default::default();
                self.schedule();
            }
            (_, Err(why)) => self.messages.fail(format!("Model not installed: {why}")),
        }
    }
    /// Asks the installer to download AI Denoise's model.
    pub(in crate::app) fn install_model(&mut self, pack: Pack) {
        self.context.request_repaint();
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        self.installer_mut(pack).install(None, tx, ctx);
    }
    /// Upgrades the open catalog on a thread of its own, after the edit is saved.
    pub(in crate::app) fn upgrade_catalog(&mut self, request: Request) {
        self.context.request_repaint();
        let running = self
            .selection
            .upgrade_thread
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        if self.selection.upgrading.is_some() || running {
            self.messages.note("The catalog is being upgraded");
            return;
        }
        if !self.flush() {
            return;
        }
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
        else {
            return;
        };
        let (generation, _) = self.selection.upgrade_task.start();
        self.selection.upgrading = Some((generation, request));
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        let thread = std::thread::Builder::new()
            .name("catalog-upgrade".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(|| {
                    crate::catalog::Catalog::open(&location)
                        .and_then(|mut c| c.upgrade_for_raster_masks())
                        .map_err(|e| format!("{e:#}"))
                })
                .unwrap_or_else(|_| Err("the upgrade stopped unexpectedly".into()));
                let _ = tx.send(crate::app::worker::Event::CatalogUpgraded { generation, result });
                ctx.request_repaint();
            });
        match thread {
            Ok(thread) => self.selection.upgrade_thread = Some(thread),
            Err(e) => {
                self.selection.upgrading = None;
                self.selection.upgrade_task.finish(generation);
                self.messages.fail(format!("Catalog not upgraded: {e}"));
            }
        }
    }
    pub(in crate::app) fn catalog_upgraded(
        &mut self,
        generation: u64,
        result: Result<Option<std::path::PathBuf>, String>,
    ) {
        let Some((current, request)) = self.selection.upgrading.clone() else {
            return;
        };
        if generation != current {
            return;
        }
        self.selection.upgrading = None;
        self.selection.upgrade_task.finish(generation);
        self.selection.upgrade_thread = None;
        match result {
            Ok(backup) => {
                if let Some(backup) = backup {
                    self.messages
                        .done(format!("Catalog upgraded · backup: {}", backup.display()));
                }
                self.selection.prompt = None;
                self.request_selection(request);
            }
            Err(why) => self.messages.fail(format!("Catalog not upgraded: {why}")),
        }
    }
}

/// What a request whose worker job panicked, or whose worker is gone, ends in.
fn stopped() -> Failure {
    Failure::Failed("the selection stopped unexpectedly".into())
}

/// Where a mask's raster came from, for the component settings, from its saved
/// provenance alone: Subject's and Sky's, made by the AI masks extension or by
/// the app before it, are labelled whether or not the extension is there.
pub(in crate::app) fn source_text(b: &BitmapMask) -> String {
    match b.source.as_ref().map(|s| s.feature.as_str()) {
        Some(FEATURE_SUBJECT) => "Found by the subject models".into(),
        Some(FEATURE_SKY) => "Found by the sky detector".into(),
        _ => "A raster mask".into(),
    }
}

#[cfg(test)]
mod extension_tests;
#[cfg(test)]
mod tests;
