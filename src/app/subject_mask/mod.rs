//! Select Subject and Select Background (docs/subject-and-background-masks.md): a
//! model finds the photo's salient foreground, and the result becomes a mask that
//! edits like any other. Everything here is the app's side of that: what a request
//! captures, when its result may still be applied, and what the drawer says
//! meanwhile. The model and its runtime are `rawmakase-inference`'s; installing the
//! model is `models`; the thread that runs it is `worker`.
mod models;
mod ui;
mod worker;

pub(super) use ui::paint_feature_icon;

use super::Editor;
use super::task::{Stopping, Task};
use crate::model::masks::{
    BITMAP_SAMPLING, BitmapMask, BitmapSource, FEATURE_SKY, FEATURE_SUBJECT, MAX_COMPONENTS,
    MAX_GROUPS, MaskComponent, MaskGroup, MaskOp, MaskShape,
};
use crate::model::recipe::Recipe;
use rawmakase_inference::{Point, Prompt as ModelPrompt};
use std::hash::{Hash, Hasher};

pub(super) use models::Installer;

/// Which of the two selections a request makes. Background is the same coverage with
/// the component inverted, so adding a brush to it still adds coverage.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Feature {
    Subject,
    Sky,
    Background,
}
impl Feature {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Subject => "Subject",
            Self::Sky => "Sky",
            Self::Background => "Background",
        }
    }
    fn invert(self) -> bool {
        self == Self::Background
    }
    /// What the raster is a selection of, as its provenance says.
    pub(super) fn provenance(self) -> &'static str {
        match self {
            Self::Sky => FEATURE_SKY,
            Self::Subject | Self::Background => FEATURE_SUBJECT,
        }
    }
}
/// Which selection a generated component is, from where it came from and whether it
/// is inverted (Background is an inverted subject).
pub(super) fn feature_of(b: &BitmapMask, invert: bool) -> Feature {
    match b.source.as_ref().map(|s| s.feature.as_str()) {
        Some(FEATURE_SKY) => Feature::Sky,
        _ if invert => Feature::Background,
        _ => Feature::Subject,
    }
}

/// What a job does: finds the photo's subject or sky by itself, or answers clicks.
#[derive(Clone, Debug)]
pub(super) enum Action {
    Auto(Feature),
    Click(ModelPrompt),
}

/// What a finished selection does to the edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Target {
    /// A new mask holding the selection.
    NewMask,
    /// A component of the selected mask, combined with `op`.
    Component { mask: usize, op: MaskOp },
    /// Replace the raster of one generated component, keeping everything else.
    Regenerate { mask: usize, component: usize },
}

/// A request, as it was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Request {
    pub(super) feature: Feature,
    pub(super) target: Target,
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
    /// The model file is not installed (or was removed meanwhile).
    ModelMissing,
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
            Self::ModelMissing => "The selection model is not installed".into(),
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
struct Guard {
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
    fn of(recipe: &Recipe, request: Request) -> Self {
        let expected = match request.target {
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
    fn holds(&self, recipe: &Recipe, request: Request) -> bool {
        let same_shape = match request.target {
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
    let raster = crate::storage::mask_assets::resolve(id).ok()?;
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
/// eye, which alter the content to select.
fn content(recipe: &Recipe) -> u64 {
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
}

/// What the drawer asks the user before a selection can run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Prompt {
    /// The models have to be downloaded.
    Model(Request),
    /// The catalog has to be upgraded to keep the result.
    Upgrade(Request),
}

/// A selection being aimed: the clicks and box so far. The first one makes the mask;
/// each further one refines that same mask until the user is done.
pub(super) struct Prompting {
    pub(super) request: Request,
    pub(super) points: Vec<Point>,
    pub(super) bounds: Option<[f32; 4]>,
    /// The mask and component the first result made (or the one being regenerated).
    applied: Option<(usize, usize)>,
}
impl Prompting {
    /// A box starts over: the clicks so far were about the previous outline.
    fn set_box(&mut self, bounds: [f32; 4]) {
        self.points.clear();
        self.bounds = Some(bounds);
    }
}

#[derive(Default)]
pub(super) struct Selection {
    /// Waiting for clicks on the photo.
    pub(super) prompting: Option<Prompting>,
    /// Where a drag for a box began, in image space.
    pub(super) drag_from: Option<[f32; 2]>,
    /// The running selection's generation; a newer request or a new photo supersedes.
    task: Task,
    pending: Option<Pending>,
    /// What the last attempt ended in, for the drawer, until the next.
    failure: Option<(Request, Failure)>,
    prompt: Option<Prompt>,
    worker: worker::Worker,
    pub(super) models: Installer,
    /// A catalog upgrade under way, and the request it unblocks.
    upgrading: Option<(u64, Request)>,
    upgrade_task: Task,
    /// Its thread, to wait for when quitting: cut off, it would leave a partial backup.
    upgrade_thread: Option<std::thread::JoinHandle<()>>,
    /// Rasters of results not taken while another selection was running, to let go of
    /// once it has finished unless it took the same one.
    deferred: Vec<String>,
}

impl Selection {
    /// Whether a selection is running.
    pub(super) fn running(&self) -> Option<Request> {
        self.pending.as_ref().map(|p| p.request)
    }
    /// Stops the running selection, whatever it is for (a new photo, Cancel).
    pub(super) fn cancel(&mut self) {
        self.task.invalidate();
        self.pending = None;
    }
    /// Stops aiming (Done, Escape, a new photo).
    pub(super) fn end_prompting(&mut self) {
        self.prompting = None;
        self.drag_from = None;
    }
    /// The photo changed: nothing asked for the previous one applies, an upgrade under
    /// way included; it finishes, but does not go on to select in this photo. Its
    /// thread is still waited for at exit.
    pub(super) fn clear_document(&mut self) {
        self.cancel();
        self.end_prompting();
        self.failure = None;
        self.prompt = None;
        self.upgrading = None;
        self.upgrade_task.invalidate();
    }
    /// The catalog changed.
    pub(super) fn clear_catalog(&mut self) {
        self.clear_document();
    }
    /// Workers to wait for when quitting: inference is asked to stop and handed to the
    /// shared deadline, never joined without one. An upgrade's backup copy is finished
    /// within that deadline too, rather than cut off.
    pub(super) fn stop(&mut self) -> Vec<Stopping> {
        self.cancel();
        self.end_prompting();
        let mut stopping = vec![
            self.worker.stop(),
            Stopping::new(self.upgrade_thread.take()),
        ];
        stopping.extend(self.models.stop());
        stopping
    }
}

impl Editor {
    /// Why Subject and Background cannot be made now, for the drawer to say.
    pub(super) fn selection_unavailable(&self) -> Option<&'static str> {
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
    /// Starts a selection, or asks what it needs first: the model, an upgraded
    /// catalog.
    pub(super) fn request_selection(&mut self, request: Request) {
        // The frame that shows what a click did comes after it: ask for one.
        self.context.request_repaint();
        // A second click on a tile while its selection runs is not another request.
        if self.selection.pending.is_some() {
            return;
        }
        if let Some(why) = self.selection_unavailable() {
            self.status = why.into();
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
            self.status = format!("A photo holds up to {MAX_GROUPS} masks");
            return;
        }
        let upgraded = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog.supports_raster_masks().ok())
            .unwrap_or(false);
        if !upgraded {
            self.selection.prompt = Some(Prompt::Upgrade(request));
            return;
        }
        if !self.selection.models.installed() {
            self.selection.prompt = Some(Prompt::Model(request));
            return;
        }
        self.selection.prompt = None;
        self.selection.failure = None;
        // The models find the subject and the sky by themselves; clicks only refine.
        self.start_selection(request, Action::Auto(request.feature));
    }
    /// Aims at the subject by hand, for what the models do not know (a sign, a car, a
    /// rocket): the clicks make the mask, a new one or the one `request` names.
    pub(super) fn begin_aiming(&mut self, request: Request) {
        self.context.request_repaint();
        if self.selection_unavailable().is_some() || !self.selection.models.installed() {
            self.request_selection(request);
            return;
        }
        self.selection.failure = None;
        self.selection.prompting = Some(Prompting {
            request,
            points: Vec::new(),
            bounds: None,
            applied: match request.target {
                Target::Regenerate { mask, component } => Some((mask, component)),
                _ => None,
            },
        });
        self.view.tool = super::state::Tool::Mask;
        self.status = "Click the subject on the photo, or drag a box around it".into();
    }
    /// Aims at a generated component: clicks on the photo add to it and leave things
    /// out of it, each one refining the same mask.
    pub(super) fn begin_refining(&mut self, mask: usize, component: usize) {
        self.context.request_repaint();
        let Some(MaskShape::Bitmap(b)) = self
            .document
            .edit
            .recipe()
            .masks
            .get(mask)
            .and_then(|m| m.components.get(component))
            .map(|c| c.shape.clone())
        else {
            return;
        };
        let invert = self.document.edit.recipe().masks[mask].components[component].invert;
        let request = Request {
            feature: feature_of(&b, invert),
            target: Target::Regenerate { mask, component },
        };
        if self.selection_unavailable().is_some() || !self.selection.models.installed() {
            self.request_selection(request);
            return;
        }
        self.selection.failure = None;
        self.selection.prompting = Some(Prompting {
            request,
            points: Vec::new(),
            bounds: None,
            applied: Some((mask, component)),
        });
        self.view.tool = super::state::Tool::Mask;
        self.status = "Click the part of the photo to add, Alt-click to leave out".into();
    }
    /// A click on the photo while aiming: a point on the object (or, with `positive`
    /// false, on something to leave out). Starts or refines the selection.
    pub(super) fn prompt_click(&mut self, at: [f32; 2], positive: bool) {
        self.context.request_repaint();
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        if self.selection.pending.is_some()
            || !(0. ..=1.).contains(&at[0])
            || !(0. ..=1.).contains(&at[1])
        {
            return;
        }
        // A negative point means nothing without something to be negative about: when
        // refining a mask already made, that is the mask's own extent.
        if !positive && p.points.iter().all(|q| !q.positive) && p.bounds.is_none() {
            let shape = p.applied.and_then(|(m, c)| {
                let masks = &self.document.edit.recipe().masks;
                Some(masks.get(m)?.components.get(c)?.shape.clone())
            });
            let Some(extent) = shape.and_then(|shape| match shape {
                MaskShape::Bitmap(b) => raster_extent(&b.id),
                _ => None,
            }) else {
                return;
            };
            let Some(p) = &mut self.selection.prompting else {
                return;
            };
            p.bounds = Some(extent);
        }
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        if p.points.len() < rawmakase_inference::process::MAX_POINTS {
            p.points.push(Point {
                x: at[0],
                y: at[1],
                positive,
            });
        }
        self.run_prompt();
    }
    /// A box dragged on the photo while aiming (image space corners).
    pub(super) fn prompt_box(&mut self, a: [f32; 2], b: [f32; 2]) {
        let Some(p) = &mut self.selection.prompting else {
            return;
        };
        let clamp = |v: f32| v.clamp(0., 1.);
        let bounds = [
            clamp(a[0].min(b[0])),
            clamp(a[1].min(b[1])),
            clamp(a[0].max(b[0])),
            clamp(a[1].max(b[1])),
        ];
        if self.selection.pending.is_some()
            || bounds[2] - bounds[0] < 0.01
            || bounds[3] - bounds[1] < 0.01
        {
            return;
        }
        p.set_box(bounds);
        self.run_prompt();
    }
    /// Forgets the clicks and box, so the next click starts the selection over (the
    /// mask made so far is replaced by it).
    pub(super) fn clear_prompt(&mut self) {
        if let Some(p) = &mut self.selection.prompting {
            p.points.clear();
            p.bounds = None;
        }
    }
    /// Runs the model on the clicks and box so far.
    pub(super) fn run_prompt(&mut self) {
        let Some(p) = &self.selection.prompting else {
            return;
        };
        let prompt = ModelPrompt {
            points: p.points.clone(),
            bounds: p.bounds,
        };
        let request = Request {
            feature: p.request.feature,
            target: match p.applied {
                Some((mask, component)) => Target::Regenerate { mask, component },
                None => p.request.target,
            },
        };
        self.selection.failure = None;
        self.start_selection(request, Action::Click(prompt));
    }
    fn start_selection(&mut self, request: Request, action: Action) {
        let Some(image) = self.document.full().cloned() else {
            return;
        };
        let Some(model) = self.selection.models.path() else {
            self.selection.end_prompting();
            self.selection.prompt = Some(Prompt::Model(request));
            return;
        };
        self.context.request_repaint();
        self.status = match &action {
            Action::Auto(feature) => format!("Finding the {}…", feature.name().to_lowercase()),
            Action::Click(_) => "Selecting…".into(),
        };
        let (generation, cancel) = self.selection.task.start();
        let recipe = self.document.edit.recipe().clone();
        self.selection.pending = Some(Pending {
            request,
            guard: Guard::of(&recipe, request),
        });
        self.selection.worker.submit(
            worker::Job {
                load: self.load.id(),
                generation,
                cancel,
                image,
                recipe,
                model,
                action,
            },
            self.tx.clone(),
            self.context.clone(),
        );
    }
    pub(super) fn cancel_selection(&mut self) {
        self.context.request_repaint();
        self.selection.cancel();
        self.status = "Selection cancelled".into();
    }
    /// A selection finished: apply it if it is still the one wanted and the edit it
    /// was made for still stands, else drop it.
    pub(super) fn selection_done(&mut self, done: Done) {
        self.apply_done(done);
        // Results set aside while this one ran: let go of the ones nothing took.
        if self.selection.pending.is_none() {
            for id in std::mem::take(&mut self.selection.deferred) {
                self.discard_raster(&id);
            }
        }
    }
    fn apply_done(&mut self, done: Done) {
        let result = done.result;
        if done.load != self.load.id() || done.generation != self.selection.task.id() {
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
            Err(failure) => {
                self.status = failure.message();
                self.selection.failure = Some((request, failure));
                return;
            }
        };
        if !pending.guard.holds(self.document.edit.recipe(), request) {
            self.status = "The masks changed while selecting; nothing was added".into();
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
            self.status = "This photo's masks use too much memory for another selection".into();
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
        let name = match request.target {
            Target::Regenerate { .. } => format!("Refine {}", request.feature.name()),
            _ => format!("Select {}", request.feature.name()),
        };
        let selected = self.change_edit(
            Some(super::history::Step::new(name, "")),
            |r| match request.target {
                Target::NewMask => {
                    let mut component = MaskComponent::new(shape);
                    component.invert = request.feature.invert();
                    r.masks.push(MaskGroup {
                        name: request.feature.name().into(),
                        components: vec![component],
                        ..Default::default()
                    });
                    Some((r.masks.len() - 1, 0))
                }
                Target::Component { mask, op } => {
                    let group = r.masks.get_mut(mask)?;
                    let mut component = MaskComponent::new(shape);
                    component.op = op;
                    component.invert = request.feature.invert();
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
            self.status = format!("{} selected", request.feature.name());
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
    /// The installer finished or was cancelled; a request waiting for the model goes
    /// on.
    pub(super) fn model_installed(&mut self, result: Result<(), String>) {
        match result {
            Ok(()) => {
                self.status = "Selection model installed".into();
                if let Some(Prompt::Model(request)) = self.selection.prompt.take() {
                    self.request_selection(request);
                }
            }
            Err(why) => self.status = format!("Model not installed: {why}"),
        }
    }
    /// Asks the installer to download the models (the setup card's Download).
    pub(super) fn install_model(&mut self) {
        self.context.request_repaint();
        self.selection
            .models
            .install(None, self.tx.clone(), self.context.clone());
    }
    /// Installs the models from a folder holding their files, each checked as a
    /// download is: for tests, which have no network.
    #[cfg(test)]
    pub(super) fn install_model_from(&mut self, folder: std::path::PathBuf) {
        self.selection
            .models
            .install(Some(folder), self.tx.clone(), self.context.clone());
    }
    /// Upgrades the open catalog on a thread of its own, after the edit is saved.
    pub(super) fn upgrade_catalog(&mut self, request: Request) {
        self.context.request_repaint();
        let running = self
            .selection
            .upgrade_thread
            .as_ref()
            .is_some_and(|t| !t.is_finished());
        if self.selection.upgrading.is_some() || running {
            self.status = "The catalog is being upgraded".into();
            return;
        }
        if !self.flush() {
            return;
        }
        let Some(location) = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone())
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
                let _ = tx.send(super::worker::Event::CatalogUpgraded { generation, result });
                ctx.request_repaint();
            });
        match thread {
            Ok(thread) => self.selection.upgrade_thread = Some(thread),
            Err(e) => {
                self.selection.upgrading = None;
                self.selection.upgrade_task.finish(generation);
                self.status = format!("Catalog not upgraded: {e}");
            }
        }
    }
    pub(super) fn catalog_upgraded(
        &mut self,
        generation: u64,
        result: Result<Option<std::path::PathBuf>, String>,
    ) {
        let Some((current, request)) = self.selection.upgrading else {
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
                    self.status = format!("Catalog upgraded · backup: {}", backup.display());
                }
                self.selection.prompt = None;
                self.request_selection(request);
            }
            Err(why) => self.status = format!("Catalog not upgraded: {why}"),
        }
    }
}

/// Where a mask's raster came from, for the component settings.
pub(super) fn source_text(b: &BitmapMask) -> String {
    match b.source.as_ref().map(|s| s.feature.as_str()) {
        Some(FEATURE_SUBJECT) => "Found by the subject models".into(),
        Some(FEATURE_SKY) => "Found by the sky detector".into(),
        _ => "A raster mask".into(),
    }
}

#[cfg(test)]
mod tests;
