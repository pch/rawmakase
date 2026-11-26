//! The editor behind the window: one photo, its recipe and the render thread,
//! all on the Rust side. What an interface sends is small (a control's name and
//! value, a curve point added, moved or removed) and what it gets back is the
//! controls' state as JSON and frames as RGBA.
//!
//! Every edit carries a number the interface chose, and the frame it gives
//! carries it back, so the interface knows which edit a frame shows.
#![forbid(unsafe_code)]
use crate::worker::{self, Job, Render, Worker};
use rawmakase_core::{
    CacheLimits, Cancel, Context, Decode, ExportSettings, Image, Inserted, Parameter, Photo,
    Recipe, Renderer, ToneCurve, View,
};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

/// The sliders the editor shows, by the library's names, in the panel's order.
const CONTROLS: [&str; 8] = [
    "temperature",
    "tint",
    "exposure",
    "contrast",
    "highlights",
    "shadows",
    "whites",
    "blacks",
];
/// How long a draft stands, with no newer edit, before its frame is rendered:
/// longer than the gap between a dragged slider's changes (a frame at 60 Hz
/// is 17 ms), so that a drag goes on in drafts.
const QUIET: Duration = Duration::from_millis(50);
/// How many points of the tone curve are sent for drawing it.
const CURVE_SAMPLES: usize = 65;

/// Where the app keeps RAWmakase's data, as values it chose.
pub struct Config {
    pub data_dir: PathBuf,
    pub cache_dir: PathBuf,
    /// What the cache's entries belong to: the app's build.
    pub build_id: String,
}

/// What the editor refuses, besides what the library reports.
#[derive(Debug)]
pub enum Error {
    Library(rawmakase_core::Error),
    NoPhoto,
    UnknownControl(String),
    NoSuchPoint(u32),
    /// Another photo was asked for while this one was opening.
    Superseded,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Library(e) => write!(f, "{e:#}"),
            Self::NoPhoto => f.write_str("No photo is open"),
            Self::UnknownControl(name) => write!(f, "No control is named {name:?}"),
            Self::NoSuchPoint(index) => write!(f, "The curve has no point {index}"),
            Self::Superseded => f.write_str("Another photo was opened meanwhile"),
        }
    }
}

impl std::error::Error for Error {}

impl From<rawmakase_core::Error> for Error {
    fn from(e: rawmakase_core::Error) -> Self {
        Self::Library(e)
    }
}

pub type Result<T> = std::result::Result<T, Error>;

/// What one render is given: the photo's pixels, the edit and the view, as
/// they were when the edit was made.
pub struct Work {
    pub image: Image,
    pub recipe: Recipe,
    pub view: View,
}

/// Rendered pixels on their way to the window.
pub struct Pixels {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// What the editor sends its interface from the render thread.
pub enum Update {
    Frame {
        edit: u32,
        draft: bool,
        pixels: Pixels,
        /// The render, from the recipe to RGBA, in milliseconds.
        render_ms: f64,
    },
    Failed {
        edit: u32,
        message: String,
    },
}

/// One render, as the render thread makes it: the work's view, or its draft,
/// as RGBA. Public so that the same call can be timed without the app.
pub fn render(
    renderer: &mut Renderer,
    work: &Work,
    draft: bool,
    cancel: Cancel<'_>,
) -> rawmakase_core::Result<Pixels> {
    let (image, recipe, view) = (&work.image, &work.recipe, work.view);
    let frame = if draft {
        renderer.render_draft(image, recipe, view, cancel)?
    } else {
        renderer.render(image, recipe, view, cancel)?
    };
    Ok(Pixels {
        width: frame.width(),
        height: frame.height(),
        rgba: frame.rgba8(),
    })
}

/// The library's draft rule decides.
impl Render<Work> for Renderer {
    type Output = Pixels;

    fn draft_due(&self, work: &Work) -> bool {
        Renderer::draft_due(self, &work.image, &work.recipe, work.view).is_some()
    }

    fn render(
        &mut self,
        work: &Work,
        draft: bool,
        cancel: Cancel<'_>,
    ) -> rawmakase_core::Result<Pixels> {
        render(self, work, draft, cancel)
    }
}

struct Document {
    photo: Arc<Photo>,
    image: Image,
    recipe: Recipe,
    /// The recipe the photo opened with, which a control resets to.
    start: Recipe,
}

struct State {
    document: Option<Document>,
    /// The long edge the view shows the photo at; `None` until the interface
    /// says, and nothing renders before.
    max_edge: Option<u32>,
    last_edit: u32,
    /// The flag of the renders of the current photo and view.
    cancel: Arc<AtomicBool>,
    /// Counts the photos asked for, so an older open that finishes late is
    /// dropped.
    opening: u64,
}

impl State {
    /// Stops the renders made for the photo and view that are going away, and
    /// gives the next ones a flag of their own.
    fn retire_renders(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
    }
}

struct Shared {
    ctx: Context,
    state: Mutex<State>,
    worker: Worker<Work>,
}

/// The editor. Cheap to clone; every clone is the same editor.
#[derive(Clone)]
pub struct Editor {
    shared: Arc<Shared>,
}

impl Editor {
    /// An editor that sends its frames to `sink`, on its render thread.
    pub fn new(config: Config, mut sink: impl FnMut(Update) + Send + 'static) -> Result<Self> {
        let ctx = Context::builder()
            .data_dirs([&config.data_dir])
            .cache(&config.cache_dir, CacheLimits::default(), config.build_id)
            .build()?;
        let renderers = ctx.clone();
        let worker = Worker::spawn(
            QUIET,
            move || renderers.renderer(),
            move |event| {
                sink(match event {
                    worker::Event::Shown {
                        edit,
                        draft,
                        output,
                        took,
                    } => Update::Frame {
                        edit,
                        draft,
                        pixels: output,
                        render_ms: took.as_secs_f64() * 1000.,
                    },
                    worker::Event::Failed { edit, message } => Update::Failed { edit, message },
                })
            },
        );
        Ok(Self {
            shared: Arc::new(Shared {
                ctx,
                state: Mutex::new(State {
                    document: None,
                    max_edge: None,
                    last_edit: 0,
                    cancel: Arc::default(),
                    opening: 0,
                }),
                worker,
            }),
        })
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Opens and decodes the photo at `path` and makes it the editor's, as edit
    /// `edit`. Takes as long as the decode: call it off the interface's thread.
    /// The photo already open stays, and keeps rendering, until this one is
    /// ready; if it cannot be opened, nothing changes. Returns the editor's
    /// state, as [`Editor::describe`].
    pub fn open(&self, path: &Path, edit: u32) -> Result<String> {
        let opening = {
            let mut state = self.state();
            state.opening += 1;
            state.opening
        };
        let ctx = &self.shared.ctx;
        let photo = ctx.open(path, Cancel::NEVER)?;
        let image = photo.decode(Decode::Full, Cancel::NEVER)?;
        let recipe = photo.starting_recipe()?;
        let mut state = self.state();
        if state.opening != opening {
            return Err(Error::Superseded);
        }
        state.retire_renders();
        state.document = Some(Document {
            photo: Arc::new(photo),
            image,
            start: recipe.clone(),
            recipe,
        });
        // Edits made while this photo was opening went to the one before it. Its
        // first frame is numbered after them, so it is never taken for an older
        // one.
        let edit = edit.max(state.last_edit.saturating_add(1));
        self.submit(&mut state, edit);
        Ok(describe(&state).to_string())
    }

    /// Renders the current recipe as edit `edit`; the render thread decides
    /// whether a draft comes first.
    fn submit(&self, state: &mut State, edit: u32) {
        state.last_edit = edit;
        let (Some(document), Some(max_edge)) = (&state.document, state.max_edge) else {
            return;
        };
        self.shared.worker.submit(Job {
            edit,
            work: Work {
                image: document.image.clone(),
                recipe: document.recipe.clone(),
                view: View::Fit(max_edge),
            },
            cancel: state.cancel.clone(),
        });
    }

    /// The long edge, in pixels, the view shows the photo at (0 for its full
    /// size). A new size cancels the renders of the old one.
    pub fn set_view(&self, max_edge: u32, edit: u32) {
        let mut state = self.state();
        if state.max_edge == Some(max_edge) {
            return;
        }
        state.max_edge = Some(max_edge);
        state.retire_renders();
        self.submit(&mut state, edit);
    }

    /// Changes the open photo's recipe with `change` and renders it as `edit`.
    fn edit<T>(&self, edit: u32, change: impl FnOnce(&mut Document) -> Result<T>) -> Result<T> {
        let mut state = self.state();
        let document = state.document.as_mut().ok_or(Error::NoPhoto)?;
        let out = change(document)?;
        self.submit(&mut state, edit);
        Ok(out)
    }

    /// Sets the control named `name` to `value`, in the recipe's units. Returns
    /// the control's new state as JSON.
    pub fn set_parameter(&self, name: &str, value: f32, edit: u32) -> Result<String> {
        let id = Parameter::named(name).ok_or_else(|| Error::UnknownControl(name.into()))?;
        self.edit(edit, |document| {
            document.photo.set(&mut document.recipe, id, value)?;
            Ok(json!(document.photo.control(&document.recipe, id)).to_string())
        })
    }

    /// Sets the control named `name` back to what the photo opened with.
    pub fn reset_parameter(&self, name: &str, edit: u32) -> Result<String> {
        let id = Parameter::named(name).ok_or_else(|| Error::UnknownControl(name.into()))?;
        self.edit(edit, |document| {
            let value = document.photo.value(&document.start, id);
            document.photo.set(&mut document.recipe, id, value)?;
            Ok(json!(document.photo.control(&document.recipe, id)).to_string())
        })
    }

    /// Adds a point to the tone curve at `x`, `y` (0 to 1). Returns the curve,
    /// the index of the point to drag (the new one, the one already at that
    /// input, or, when the curve is full, the nearest) and whether it was
    /// added.
    pub fn curve_insert(&self, x: f32, y: f32, edit: u32) -> Result<String> {
        self.edit(edit, |document| {
            let inserted = document.recipe.curve.insert([x, y]);
            Ok(json!({
                "index": inserted.index(),
                "added": matches!(inserted, Inserted::Added(_)),
                "curve": curve(&document.recipe.curve),
            })
            .to_string())
        })
    }

    /// Moves the curve's point `index` to `x`, `y`, kept between its neighbours.
    /// Returns the curve.
    pub fn curve_move(&self, index: u32, x: f32, y: f32, edit: u32) -> Result<String> {
        self.edit(edit, |document| {
            let points = &mut document.recipe.curve;
            if !points.move_point(index as usize, [x, y]) {
                return Err(Error::NoSuchPoint(index));
            }
            Ok(curve(points).to_string())
        })
    }

    /// Removes the curve's point `index`; its ends stay. Returns the curve.
    pub fn curve_remove(&self, index: u32, edit: u32) -> Result<String> {
        self.edit(edit, |document| {
            document.recipe.curve.remove(index as usize);
            Ok(curve(&document.recipe.curve).to_string())
        })
    }

    /// The editor's state as JSON: the photo, its controls, its curve and the
    /// number of the newest edit; no photo, controls or curve while none is open.
    pub fn describe(&self) -> String {
        describe(&self.state()).to_string()
    }

    /// Exports the photo with its recipe as it is now to `out`, in the format
    /// its extension names, replacing a file there: the user chose it in the
    /// save dialog, which asked. Takes as long as a full-size render: call it
    /// off the interface's thread.
    pub fn export(&self, out: &Path) -> Result<Option<String>> {
        let (photo, recipe) = self.document()?;
        let settings = ExportSettings::default();
        Ok(self
            .shared
            .ctx
            .export(&photo, &recipe, &settings, out, true, Cancel::NEVER, |_| {})?)
    }

    /// The open photo and its recipe as it is now.
    fn document(&self) -> Result<(Arc<Photo>, Recipe)> {
        let state = self.state();
        let document = state.document.as_ref().ok_or(Error::NoPhoto)?;
        Ok((document.photo.clone(), document.recipe.clone()))
    }

    /// Saves the recipe as it is now as a RAWmakase preset at `path`.
    pub fn save_recipe(&self, path: &Path) -> Result<()> {
        let (photo, recipe) = self.document()?;
        Ok(photo.save_recipe(&recipe, path)?)
    }
}

fn describe(state: &State) -> Value {
    let Some(document) = &state.document else {
        return json!({ "photo": null, "controls": [], "curve": null });
    };
    let photo = &document.photo;
    let metadata = photo.metadata();
    let (width, height) = document
        .image
        .output_size(&document.recipe)
        .unwrap_or_default();
    let stats = document.image.stats();
    let controls: Vec<Value> = CONTROLS
        .iter()
        .filter_map(|name| Parameter::named(name))
        .map(|id| json!(photo.control(&document.recipe, id)))
        .collect();
    json!({
        "photo": {
            "path": photo.path().to_string_lossy(),
            "name": photo.path().file_name().map(|name| name.to_string_lossy()),
            "camera": format!("{} {}", metadata.make, metadata.model).trim(),
            "width": width,
            "height": height,
            "clippedSamples": stats.clipped_samples,
            "highestLevel": stats.highest_level,
        },
        "controls": controls,
        "curve": curve(&document.recipe.curve),
        "edit": state.last_edit,
    })
}

/// The tone curve for drawing: its points, and `[input, output]` at evenly
/// spaced inputs from 0 to 1, as the library evaluates it.
fn curve(curve: &ToneCurve) -> Value {
    json!({ "points": curve.points, "samples": curve.sampled(CURVE_SAMPLES) })
}
