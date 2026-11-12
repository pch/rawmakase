//! The bundled AI masks extension (`rawmakase.ai-masks`): Select Subject, Sky
//! and Background, made by RAWmakase's selection models on this computer or
//! on a remote worker (docs/subject-and-background-masks.md).
//!
//! Two mask generators (`rawmakase-extension.toml`): `subject`, automatic or
//! aimed with clicks and a box, whose tiles are Subject and Background (an
//! inverted Subject, which the app makes), and `sky`, automatic. Their masks
//! record `subject` and `sky` as their source, as masks did before this
//! extension, so saved masks find their generator again.
//!
//! The app renders the photo as the models see it and keeps the coverage as a
//! raster. This program runs the models ([`engine`]) or asks the remote worker
//! the user configured ([`Remote`]), keeps the last photo's analysis between
//! requests, and installs the models when asked to (`mask.setup`), adopting
//! the app's own install of them from before ([`models`]). The models run on
//! the ONNX Runtime the app bundles ([`runtime`]).
//!
//! Requests run one at a time, so the generators share their state behind a
//! lock no two hold at once.
pub mod engine;
pub mod fake;
mod inference;
mod models;
mod remote;
mod runtime;

pub use engine::{Engine, Onnx};
pub use models::{Install, download};
pub use remote::Remote;

use inference::{Ask, State};
use models::{Models, Pack};
use rawmakase_extension::protocol::method::{CommandRun, EmptyMask, MaskGenerated};
use rawmakase_extension::protocol::viewer::ViewerInput;
use rawmakase_extension::{Context, ErrorCode, Extension, MaskAsk, MaskGenerator, RemoteError};
use rawmakase_model_install::{InstallError, Progress};
use rawmakase_selection::{Point, Prompt as ModelPrompt, RgbImage};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The extension's id.
pub const ID: &str = "rawmakase.ai-masks";
/// The generators' ids.
pub const SUBJECT: &str = "subject";
pub const SKY: &str = "sky";
/// The command that deletes the downloaded models.
pub const REMOVE_MODELS: &str = "remove-models";

/// Which of the selections a request makes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Feature {
    Subject,
    Sky,
}

/// What a request does: finds the photo's subject or sky by itself, or
/// answers clicks.
#[derive(Clone, Debug)]
pub(crate) enum Action {
    Auto(Feature),
    Click(ModelPrompt),
}

/// Why a selection made no coverage, or what an all-zero one means.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    Cancelled,
    /// The model found nothing; no mask is made.
    NoSubject,
    NoSky,
    /// Nothing at the clicked place.
    NothingThere,
    /// The model files are not installed (or were removed meanwhile).
    ModelMissing,
    /// The inference runtime could not be loaded or has no support here.
    RuntimeUnavailable(String),
    Failed(String),
}

/// What the program runs with: the models, how they are installed, and where
/// the app's own files from before are.
pub struct Options {
    /// What runs the models here.
    pub engine: Box<dyn Engine>,
    /// How a pack's files are fetched.
    pub install: Install,
    /// The app's local data folder, which holds `remote-inference.json`, the
    /// models it installed in `models/` and a runtime it didn't come with in
    /// `runtime/`.
    pub local_data: PathBuf,
    /// Where the endpoint of a remote worker comes from.
    pub remote: Remote,
}

impl Default for Options {
    /// The program's: the ONNX models, downloads, and this computer's
    /// folders and configuration.
    fn default() -> Self {
        Self {
            engine: Box::new(Onnx::default()),
            install: Box::new(download),
            local_data: rawmakase_protocol::paths::local_data_dir(),
            remote: Remote::Configured,
        }
    }
}

/// What the generators share.
struct Masks {
    engine: Box<dyn Engine>,
    state: State,
    install: Install,
    local_data: PathBuf,
    remote: Remote,
}

impl Masks {
    /// The packs, in the extension's data folder and the app's from before.
    fn models(&self, cx: &Context) -> Models {
        Models::new(cx.data_dir().join("models"), self.local_data.join("models"))
    }
}

type Shared = Arc<Mutex<Masks>>;

fn lock(masks: &Shared) -> std::sync::MutexGuard<'_, Masks> {
    masks.lock().unwrap_or_else(|e| e.into_inner())
}

/// The extension, ready to run.
pub fn extension(options: Options) -> Extension {
    let masks: Shared = Arc::new(Mutex::new(Masks {
        engine: options.engine,
        state: State::default(),
        install: options.install,
        local_data: options.local_data,
        remote: options.remote,
    }));
    let remove = masks.clone();
    Extension::new()
        .mask_generator(
            SUBJECT,
            Generator {
                id: SUBJECT,
                masks: masks.clone(),
            },
        )
        .mask_generator(SKY, Generator { id: SKY, masks })
        .command(REMOVE_MODELS, move |cx: &Context, _run: CommandRun| {
            remove_models(cx, &remove)
        })
}

/// One of the generators, by its id.
struct Generator {
    id: &'static str,
    masks: Shared,
}

impl MaskGenerator for Generator {
    fn generate(&mut self, cx: &Context, ask: MaskAsk<'_>) -> Result<MaskGenerated, RemoteError> {
        let action = action(self.id, ask.request.input.as_ref())?;
        let pack = match action {
            Action::Auto(Feature::Sky) => Pack::Sky,
            _ => Pack::Subject,
        };
        let rgb = photo(&ask)?;
        let masks = &mut *lock(&self.masks);
        let endpoint = masks.remote.endpoint(&masks.local_data);
        let model = match &endpoint {
            // Over the remote worker, the models don't have to be here at all.
            Some(endpoint) => {
                cx.progress(None, Some(&format!("on {endpoint}")));
                None
            }
            None => Some(
                masks
                    .models(cx)
                    .installed(pack)
                    .ok_or_else(|| pack.needs_setup().error())?,
            ),
        };
        let answer = inference::generate(
            &Ask {
                rgb: &rgb,
                image_key: &ask.request.image_key,
                action: &action,
                model: model.as_deref(),
                endpoint: endpoint.as_deref(),
                cancel: cx.cancel_flag(),
            },
            &mut masks.state,
            masks.engine.as_mut(),
        )
        .map_err(|failure| error(failure, pack))?;
        if cx.is_cancelled() {
            return Err(error(Failure::Cancelled, pack));
        }
        let coverage = &answer.coverage;
        let out = &ask.request.coverage;
        if (coverage.width, coverage.height) != (out.width as usize, out.height as usize) {
            return Err(error(
                Failure::Failed(format!(
                    "the model's coverage is {}x{}, not the photo's {}x{}",
                    coverage.width, coverage.height, out.width, out.height
                )),
                pack,
            ));
        }
        for (y, row) in coverage.data.chunks(coverage.width.max(1)).enumerate() {
            ask.coverage[y * out.stride as usize..][..row.len()].copy_from_slice(row);
        }
        let empty = coverage
            .data
            .iter()
            .all(|v| *v == 0)
            .then_some(match answer.empty {
                Failure::NoSky => EmptyMask::NoSky,
                Failure::NoSubject => EmptyMask::NoSubject,
                _ => EmptyMask::NothingThere,
            });
        Ok(MaskGenerated {
            model: answer.model,
            model_key: answer.model_key,
            empty,
        })
    }

    fn setup(&mut self, cx: &Context, generator: &str) -> Result<(), RemoteError> {
        let pack = match generator {
            SKY => Pack::Sky,
            _ => Pack::Subject,
        };
        let masks = lock(&self.masks);
        let (models, install) = (masks.models(cx), &masks.install);
        let total = pack.needs_setup().bytes.max(1);
        let progress = Progress::default();
        let result = std::thread::scope(|scope| {
            let job = std::thread::Builder::new()
                .name("model-install".into())
                .spawn_scoped(scope, || models.set_up(pack, install, &progress))
                .map_err(|e| InstallError::Failed(format!("could not start the download: {e}")))?;
            let mut said = None;
            while !job.is_finished() {
                if cx.is_cancelled() {
                    progress.cancel();
                }
                let done = progress.fetched();
                if said != Some(done) {
                    said = Some(done);
                    cx.progress(
                        Some(done as f64 / total as f64),
                        Some(&format!("{} of {} MB", done / 1_000_000, total / 1_000_000)),
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            job.join().unwrap_or_else(|_| {
                Err(InstallError::Failed(
                    "the download stopped unexpectedly".into(),
                ))
            })
        });
        match result {
            Ok(()) => Ok(()),
            Err(InstallError::Cancelled) => {
                Err(RemoteError::new(ErrorCode::Cancelled, "Cancelled"))
            }
            Err(why) => Err(RemoteError::new(
                ErrorCode::Other("failed".into()),
                format!("{why}. Download again to carry on from where it stopped."),
            )),
        }
    }
}

/// What `generator` is asked to do with `input`.
fn action(generator: &str, input: Option<&ViewerInput>) -> Result<Action, RemoteError> {
    match (generator, input) {
        (SUBJECT, None) => Ok(Action::Auto(Feature::Subject)),
        (SKY, None) => Ok(Action::Auto(Feature::Sky)),
        (SUBJECT, Some(ViewerInput::Prompt { clicks, bounds })) => Ok(Action::Click(ModelPrompt {
            points: clicks
                .iter()
                .map(|c| Point {
                    x: c.x,
                    y: c.y,
                    positive: c.positive,
                })
                .collect(),
            bounds: bounds.map(|b| [b.x0, b.y0, b.x1, b.y1]),
        })),
        _ => Err(RemoteError::new(
            ErrorCode::InvalidRequest,
            format!("{generator} takes no such input"),
        )),
    }
}

/// The request's image as the models take it: its rows packed.
fn photo(ask: &MaskAsk<'_>) -> Result<RgbImage, RemoteError> {
    let image = &ask.request.image;
    let (width, height) = (image.width as usize, image.height as usize);
    let (row, stride) = (width * 3, image.stride as usize);
    let mut data = Vec::with_capacity(row * height);
    for y in 0..height {
        let pixels = ask.image.get(y * stride..y * stride + row).ok_or_else(|| {
            RemoteError::new(
                ErrorCode::InvalidRequest,
                "the image is shorter than it says",
            )
        })?;
        data.extend_from_slice(pixels);
    }
    Ok(RgbImage {
        width,
        height,
        data,
    })
}

/// What a failure is to the app: `needs_setup` with what to download,
/// `unavailable` with why, `cancelled`, or a plain failure.
fn error(failure: Failure, pack: Pack) -> RemoteError {
    match failure {
        Failure::Cancelled => RemoteError::new(ErrorCode::Cancelled, "Cancelled"),
        Failure::ModelMissing => pack.needs_setup().error(),
        Failure::RuntimeUnavailable(why) => RemoteError::new(ErrorCode::Unavailable, why),
        Failure::Failed(why) => RemoteError::new(ErrorCode::Other("failed".into()), why),
        // Nothing found is an answer, not an error.
        Failure::NoSubject | Failure::NoSky | Failure::NothingThere => {
            RemoteError::new(ErrorCode::Other("failed".into()), "nothing found")
        }
    }
}

/// The `remove-models` command: lets go of the loaded models, then deletes
/// every pack. Saved masks keep working; selecting again asks to download
/// them.
fn remove_models(cx: &Context, masks: &Shared) -> Result<serde_json::Value, RemoteError> {
    let mut masks = lock(masks);
    masks.engine.unload();
    masks.state = State::default();
    let removed = masks
        .models(cx)
        .remove()
        .map_err(|why| RemoteError::new(ErrorCode::Other("failed".into()), why))?;
    Ok(serde_json::json!({ "removed": removed }))
}

#[cfg(test)]
mod tests;
