//! The selection models' sessions: Subject's three networks and Sky's one, loaded
//! through the ONNX Runtime that [`rawmakase_inference::runtime`] opens.

use rawmakase_cancel::Cancel;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::{Duration, Instant};

use ort::session::Session as OrtSession;
use ort::session::builder::GraphOptimizationLevel;
use ort::value::TensorRef;
use rawmakase_inference::InferenceError;
use rawmakase_inference::runtime::{
    LoadOptions, Provider, build_session, build_session_from_memory, cancelled, check_files,
    default_threads, failed, guarded, load_runtime_with, view,
};

use crate::auto::{self, SIDE};
use crate::kinds;
use crate::manifest::{ModelSpec, PANOPTIC, SKY, SUBJECT, all_files, sky_files};
use crate::onnx_patch;
use crate::panoptic::{self, Labels};
use crate::process::{self, Coverage, Prompt, RgbImage};

/// Namespace for loading the selection model.
pub struct Subject;

impl Subject {
    /// Opens the runtime (searching [`RUNTIME_ENV`](rawmakase_inference::RUNTIME_ENV)
    /// and the package locations) and the model in folder `dir`.
    pub fn load(dir: &Path) -> Result<Session, InferenceError> {
        Self::load_with(dir, &LoadOptions::default())
    }

    pub fn load_with(dir: &Path, options: &LoadOptions) -> Result<Session, InferenceError> {
        let environment = load_runtime_with(options)?;
        let spec = SUBJECT;
        check_files(dir, &all_files())?;
        let threads = options.threads.unwrap_or_else(default_threads).max(1);
        let provider = options.provider.unwrap_or_default();
        // The runtime API panics on internal misuse; a panic must never reach
        // the app from a background job.
        let (encoder, decoder, panoptic) = catch_unwind(AssertUnwindSafe(|| {
            Ok::<_, InferenceError>((
                build_session(
                    &environment,
                    &dir.join(spec.encoder_file),
                    threads,
                    GraphOptimizationLevel::All,
                    provider,
                )?,
                build_session(
                    &environment,
                    &dir.join(spec.decoder_file),
                    threads,
                    GraphOptimizationLevel::All,
                    provider,
                )?,
                // The half-precision graph breaks the full fusion pass.
                build_session(
                    &environment,
                    &dir.join(PANOPTIC.file.name),
                    threads,
                    GraphOptimizationLevel::Level1,
                    provider,
                )?,
            ))
        }))
        .map_err(|_| {
            InferenceError::Failed("ONNX Runtime panicked while loading the model".into())
        })??;
        Ok(Session {
            encoder,
            decoder,
            panoptic,
            spec,
            provider,
        })
    }
}

/// Namespace for loading the sky model.
pub struct Sky;

impl Sky {
    /// Opens the runtime as [`Subject::load_with`] does and the sky model in folder `dir`.
    pub fn load_with(dir: &Path, options: &LoadOptions) -> Result<SkySession, InferenceError> {
        let environment = load_runtime_with(options)?;
        check_files(dir, &sky_files())?;
        let threads = options.threads.unwrap_or_else(default_threads).max(1);
        let provider = options.provider.unwrap_or_default();
        let path = dir.join(SKY.file.name);
        let model = onnx_patch::read_for_patch(&path, SKY.logits)
            .map_err(|e| {
                InferenceError::ModelInvalid(format!("cannot read {}: {e}", path.display()))
            })
            .and_then(|file| onnx_patch::expose_output(file, SKY.logits))?;
        let network = catch_unwind(AssertUnwindSafe(|| {
            build_session_from_memory(
                &environment,
                &path,
                &model,
                threads,
                GraphOptimizationLevel::All,
                provider,
            )
        }))
        .map_err(|_| {
            InferenceError::Failed("ONNX Runtime panicked while loading the model".into())
        })??;
        Ok(SkySession {
            network,
            provider,
            last: None,
        })
    }
}

/// The loaded sky model. One run at a time; dropping it releases the model's memory.
pub struct SkySession {
    network: OrtSession,
    provider: Provider,
    last: Option<SkyTimings>,
}

/// Where the time of one [`SkySession::select`] went.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SkyTimings {
    /// Preparing the input, running the model and reading its classes into kinds.
    pub model: Duration,
    /// Everything after the model: the sky's edge, [`SkyTimings::modes`] included.
    pub edge: Duration,
    /// The part of [`SkyTimings::edge`] spent weighing and running the edge modes, beyond
    /// the model's answer snapped to the photo.
    pub modes: Duration,
    /// Whether the sparse mode ran (a crown in the photo).
    pub sparse: bool,
    /// Whether the dense mode ran (a lattice the model is sure of).
    pub dense: bool,
}

impl SkySession {
    /// The provider the runtime accepted, as [`Session::provider`].
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Where the time of the last successful [`SkySession::select`] went.
    pub fn last_timings(&self) -> Option<SkyTimings> {
        self.last
    }

    /// The photo's sky as coverage in its frame, all zero when there is none. A few
    /// seconds; cancellable.
    pub fn select(
        &mut self,
        image: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        let started = Instant::now();
        self.last = None;
        cancelled(cancel)?;
        process::check_image(image)?;
        let (width, height) = sky_input_size(image.width, image.height);
        let tensor = process::preprocess_sized(image, width, height, SKY.mean, SKY.std)?;
        cancelled(cancel)?;
        let network = &mut self.network;
        let (probability, kinds) = guarded(cancel, |options| {
            let input = TensorRef::from_array_view(([1usize, 3, height, width], &tensor[..]))
                .map_err(failed)?;
            let outputs = network
                .run_with_options(ort::inputs!["pixel_values" => input], options)
                .map_err(failed)?;
            let get = |name: &str| {
                let output = outputs.get(name).ok_or_else(|| {
                    InferenceError::ModelInvalid(format!("the sky model has no output `{name}`"))
                })?;
                output.try_extract_tensor::<f32>().map_err(failed)
            };
            let (shape, data) = get("sky")?;
            let expected = [1, height as i64, width as i64];
            if **shape != expected {
                return Err(InferenceError::OutputInvalid(format!(
                    "the sky model answered with shape {shape:?}, expected {expected:?}"
                )));
            }
            let probability = data.to_vec();
            let (shape, logits) = get(SKY.logits)?;
            let (classes, grid_width, grid_height) = logits_grid(shape, width, height)?;
            Ok((
                probability,
                kinds::kinds(logits, classes, grid_width, grid_height)?,
            ))
        })?;
        let modelled = Instant::now();
        let (coverage, modes) =
            auto::sky_with_modes(image, &probability, width, height, &kinds, cancel)?;
        self.last = Some(SkyTimings {
            model: modelled - started,
            edge: modelled.elapsed(),
            modes: modes.took,
            sparse: modes.sparse,
            dense: modes.dense,
        });
        Ok(coverage)
    }
}

/// The classes and grid of the sky model's class scores for an input of `width` x
/// `height`: one image, on a grid a quarter of the input's sides.
fn logits_grid(
    shape: &[i64],
    width: usize,
    height: usize,
) -> Result<(usize, usize, usize), InferenceError> {
    let dims: Vec<usize> = shape
        .iter()
        .filter_map(|d| usize::try_from(*d).ok())
        .collect();
    match dims[..] {
        [1, classes, h, w] if dims.len() == shape.len() && h == height / 4 && w == width / 4 => {
            Ok((classes, w, h))
        }
        _ => Err(InferenceError::OutputInvalid(format!(
            "the sky model's class scores have shape {shape:?} for a {width}x{height} input"
        ))),
    }
}

/// What the sky model is fed for a photo of `width` x `height`: scaled to the long edge,
/// its proportions kept, unless that would leave a side too short for the network.
fn sky_input_size(width: usize, height: usize) -> (usize, usize) {
    let scale = SKY.long_edge as f64 / width.max(height) as f64;
    let side = |v: usize| ((v as f64 * scale).round() as usize).max(SKY.min_side);
    (side(width), side(height))
}

/// What the image encoder made of one photo: reused for every click on it. The
/// photo itself is kept for the edge refinement of each result.
pub struct Embedding {
    features: [([usize; 4], Vec<f32>); 3],
    image: RgbImage,
}
impl Embedding {
    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.features
            .iter()
            .map(|(_, v)| v.len() * 4)
            .sum::<usize>()
            + self.image.data.len()
    }
}

/// A loaded model. Not `Sync`: one run at a time per session, which is also what
/// ONNX Runtime's `Run` expects here. Dropping it releases the model's memory.
pub struct Session {
    encoder: OrtSession,
    decoder: OrtSession,
    panoptic: OrtSession,
    spec: ModelSpec,
    provider: Provider,
}

/// What the models made of one photo: reused for every selection on it. Subject is chosen
/// from it without running a model again; clicks need the embedding.
pub struct Analysis {
    embedding: Embedding,
    /// What SAM 2 drew for each person and animal DETR found, together (0..=1, on the
    /// [`SIDE`] grid).
    subject_mask: Vec<f32>,
}
impl Analysis {
    /// The photo's people and animals as coverage in its frame, all zero when there are
    /// none.
    pub fn subject(&self) -> Result<Coverage, InferenceError> {
        auto::subject(&self.embedding.image, &self.subject_mask)
    }
    /// The embedding, for [`Session::segment`].
    pub fn embedding(&self) -> &Embedding {
        &self.embedding
    }
    /// Bytes held.
    pub fn bytes(&self) -> usize {
        self.embedding.bytes() + self.subject_mask.len() * 4
    }
}

/// The decoder's answer to one prompt.
struct Decoded {
    /// Three candidate masks' logits, one after the other.
    logits: Vec<f32>,
    scores: Vec<f32>,
    object: f32,
}

impl Session {
    pub fn spec(&self) -> &ModelSpec {
        &self.spec
    }

    /// The provider the runtime accepted for these sessions. Loading failed
    /// outright when a requested accelerator could not be registered, so this
    /// never names hardware the sessions were not built with.
    pub fn provider(&self) -> Provider {
        self.provider
    }

    /// Runs the image encoder on `image`. This is the expensive step (about a second);
    /// what it returns serves any number of [`Session::segment`] calls on the photo.
    ///
    /// `cancel` is checked before and polled while the network runs (which terminates
    /// the run), so a raised flag never yields an embedding.
    pub fn embed(
        &mut self,
        image: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<Embedding, InferenceError> {
        embed_with(&mut self.encoder, &self.spec, image, cancel)
    }

    /// The mask for `prompt` on an embedded photo, as 8-bit coverage at the photo's
    /// own size (capped at [`process::MAX_COVERAGE_SIDE`]), its edge moved onto the
    /// photo's. All zero when the model finds no object at the prompt.
    pub fn segment(
        &mut self,
        embedding: &Embedding,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, InferenceError> {
        let decoded = self.decode(embedding, prompt, cancel)?;
        let (width, height) = process::output_size(&embedding.image);
        if decoded.object < 0.0 {
            return Ok(Coverage {
                width,
                height,
                data: vec![0; width * height],
            });
        }
        let side = self.spec.mask_size;
        let best = decoded.best();
        let mut coverage = process::postprocess(
            &decoded.logits[best * side * side..(best + 1) * side * side],
            &self.spec,
            width,
            height,
        )?;
        crate::refine::refine(&mut coverage, &embedding.image);
        Ok(coverage)
    }

    /// Everything Subject needs of a photo: the embedding, where DETR finds its people and
    /// animals, and SAM 2's outline of each. About two seconds; cancellable.
    pub fn analyze(
        &mut self,
        image: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<Analysis, InferenceError> {
        // The two networks do not depend on each other: run them together.
        let (embedding, labels) = {
            let (encoder, panoptic, spec) = (&mut self.encoder, &mut self.panoptic, &self.spec);
            std::thread::scope(|scope| {
                let embedding = scope.spawn(move || embed_with(encoder, spec, image, cancel));
                let labels = panoptic_with(panoptic, image, cancel);
                let embedding = embedding.join().unwrap_or_else(|_| {
                    Err(InferenceError::Failed(
                        "the image encoder stopped unexpectedly".into(),
                    ))
                });
                (embedding, labels)
            })
        };
        let (embedding, labels) = (embedding?, labels?);
        let side = self.spec.mask_size;
        debug_assert_eq!(side, SIDE);
        let candidates = |decoded: &Decoded| -> Vec<Vec<bool>> {
            (0..3)
                .map(|k| {
                    decoded.logits[k * side * side..(k + 1) * side * side]
                        .iter()
                        .map(|l| *l > 0.0)
                        .collect()
                })
                .collect()
        };
        // Each person and animal: SAM 2 is asked for it with its box and points inside,
        // and the candidate that agrees best with what DETR found is kept (DETR's own
        // mask where none does).
        let mut subject_mask = vec![0f32; SIDE * SIDE];
        for (k, instance) in labels.instances.iter().enumerate() {
            let others: Vec<&panoptic::Instance> = labels
                .instances
                .iter()
                .enumerate()
                .filter(|(j, _)| *j != k)
                .map(|(_, o)| o)
                .collect();
            let decoded = self.decode(&embedding, &auto::aim(instance, &others), cancel)?;
            let detr: Vec<bool> = instance.mask.iter().map(|v| *v > 0.5).collect();
            let best = candidates(&decoded)
                .into_iter()
                .enumerate()
                .map(|(c, mask)| (auto::iou(&mask, &detr), c))
                .max_by(|a, b| a.0.total_cmp(&b.0));
            let drawn = match best {
                Some((agreement, c)) if agreement >= 0.5 && decoded.object >= 0.0 => decoded.logits
                    [c * side * side..(c + 1) * side * side]
                    .iter()
                    .map(|l| 1.0 / (1.0 + (-l).exp()))
                    .collect::<Vec<f32>>(),
                _ => instance.mask.clone(),
            };
            for (m, d) in subject_mask.iter_mut().zip(&drawn) {
                *m = m.max(*d);
            }
        }
        Ok(Analysis {
            embedding,
            subject_mask,
        })
    }

    fn decode(
        &mut self,
        embedding: &Embedding,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Decoded, InferenceError> {
        cancelled(cancel)?;
        prompt.validate()?;
        let (points, labels, bounds) = prompt.tensors(&self.spec);
        let n = labels.len();
        let boxes = bounds.len() / 4;
        let decoder = &mut self.decoder;
        let (logits, scores, object) = guarded(cancel, |options| {
            let [f0, f1, f2] = &embedding.features;
            let outputs = decoder
                .run_with_options(
                    ort::inputs![
                        "input_points" => TensorRef::from_array_view(([1usize, 1, n, 2], &points[..])).map_err(failed)?,
                        "input_labels" => TensorRef::from_array_view(([1usize, 1, n], &labels[..])).map_err(failed)?,
                        "input_boxes" => TensorRef::from_array_view(([1usize, boxes, 4], &bounds[..])).map_err(failed)?,
                        "image_embeddings.0" => view(f0)?,
                        "image_embeddings.1" => view(f1)?,
                        "image_embeddings.2" => view(f2)?,
                    ],
                    options,
                )
                .map_err(failed)?;
            let get = |name: &str| -> Result<Vec<f32>, InferenceError> {
                let output = outputs.get(name).ok_or_else(|| {
                    InferenceError::ModelInvalid(format!("the decoder has no output `{name}`"))
                })?;
                let (_, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
                Ok(data.to_vec())
            };
            Ok((
                get("pred_masks")?,
                get("iou_scores")?,
                get("object_score_logits")?,
            ))
        })?;
        let side = self.spec.mask_size;
        if scores.len() != 3
            || logits.len() != 3 * side * side
            || object.len() != 1
            || scores.iter().any(|s| !s.is_finite())
            || logits.iter().any(|l| !l.is_finite())
            || !object[0].is_finite()
        {
            return Err(InferenceError::OutputInvalid(format!(
                "the decoder returned {} masks of {} values",
                scores.len(),
                logits.len()
            )));
        }
        Ok(Decoded {
            logits,
            scores,
            object: object[0],
        })
    }
}

impl Decoded {
    /// The candidate the model rates highest.
    fn best(&self) -> usize {
        (0..3)
            .max_by(|a, b| self.scores[*a].total_cmp(&self.scores[*b]))
            .unwrap_or(0)
    }
}

fn embed_with(
    encoder: &mut OrtSession,
    spec: &ModelSpec,
    image: &RgbImage,
    cancel: Cancel<'_>,
) -> Result<Embedding, InferenceError> {
    cancelled(cancel)?;
    let tensor = process::preprocess(image, spec.input_size, spec.mean, spec.std)?;
    cancelled(cancel)?;
    let size = spec.input_size;
    let features = guarded(cancel, |options| {
        let input =
            TensorRef::from_array_view(([1usize, 3, size, size], &tensor[..])).map_err(failed)?;
        let outputs = encoder
            .run_with_options(ort::inputs!["pixel_values" => input], options)
            .map_err(failed)?;
        let mut features = Vec::with_capacity(3);
        for index in 0..3 {
            let name = format!("image_embeddings.{index}");
            let output = outputs.get(name.as_str()).ok_or_else(|| {
                InferenceError::ModelInvalid(format!("the encoder has no output `{name}`"))
            })?;
            let (shape, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
            features.push((shape.iter().copied().collect::<Vec<i64>>(), data.to_vec()));
        }
        Ok(features)
    })?;
    cancelled(cancel)?;
    let mut checked = Vec::with_capacity(3);
    for (shape, data) in features {
        let dims: Option<[usize; 4]> = (shape.len() == 4 && shape[0] == 1)
            .then(|| {
                let mut dims = [0usize; 4];
                for (out, d) in dims.iter_mut().zip(&shape) {
                    *out = usize::try_from(*d).ok()?;
                }
                Some(dims)
            })
            .flatten();
        let count = dims.and_then(|d| d.iter().try_fold(1usize, |n, d| n.checked_mul(*d)));
        match dims {
            Some(dims) if count == Some(data.len()) => checked.push((dims, data)),
            _ => {
                return Err(InferenceError::OutputInvalid(format!(
                    "an encoder output has shape {shape:?}"
                )));
            }
        }
    }
    let features: [([usize; 4], Vec<f32>); 3] = checked
        .try_into()
        .map_err(|_| InferenceError::ModelInvalid("the encoder outputs changed".into()))?;
    Ok(Embedding {
        features,
        image: image.clone(),
    })
}

/// Where the photo's people and animals are, on the [`SIDE`] grid.
fn panoptic_with(
    network: &mut OrtSession,
    image: &RgbImage,
    cancel: Cancel<'_>,
) -> Result<Labels, InferenceError> {
    cancelled(cancel)?;
    process::check_image(image)?;
    let scale = PANOPTIC.long_edge as f64 / image.width.max(image.height) as f64;
    let width = ((image.width as f64 * scale).round() as usize).max(1);
    let height = ((image.height as f64 * scale).round() as usize).max(1);
    let tensor = process::preprocess_sized(image, width, height, PANOPTIC.mean, PANOPTIC.std)?;
    cancelled(cancel)?;
    let (logits, masks, queries, classes, mask_w, mask_h) = guarded(cancel, |options| {
        let input = TensorRef::from_array_view(([1usize, 3, height, width], &tensor[..]))
            .map_err(failed)?;
        // Every pixel is real: the model takes this mask at a fixed size.
        let valid = vec![1i64; 64 * 64];
        let mask = TensorRef::from_array_view(([1usize, 64, 64], &valid[..])).map_err(failed)?;
        let outputs = network
            .run_with_options(
                ort::inputs!["pixel_values" => input, "pixel_mask" => mask],
                options,
            )
            .map_err(failed)?;
        let get = |name: &str| -> Result<(Vec<i64>, Vec<f32>), InferenceError> {
            let output = outputs.get(name).ok_or_else(|| {
                InferenceError::ModelInvalid(format!("the panoptic model has no output `{name}`"))
            })?;
            let (shape, data) = output.try_extract_tensor::<f32>().map_err(failed)?;
            Ok((shape.iter().copied().collect(), data.to_vec()))
        };
        let (logit_shape, logits) = get("logits")?;
        let (mask_shape, masks) = get("pred_masks")?;
        if logit_shape.len() != 3
            || mask_shape.len() != 4
            || logit_shape[0] != 1
            || mask_shape[0] != 1
        {
            return Err(InferenceError::OutputInvalid(format!(
                "the panoptic output shapes are {logit_shape:?} and {mask_shape:?}"
            )));
        }
        let dim = |v: i64| usize::try_from(v).unwrap_or(0);
        Ok((
            logits,
            masks,
            dim(logit_shape[1]),
            dim(logit_shape[2]),
            dim(mask_shape[3]),
            dim(mask_shape[2]),
        ))
    })?;
    panoptic::decode(&PANOPTIC, &logits, &masks, queries, classes, mask_w, mask_h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_sky_class_scores_are_one_image_on_a_quarter_grid() {
        assert_eq!(
            logits_grid(&[1, 150, 170, 256], 1024, 682).unwrap(),
            (150, 256, 170)
        );
        for shape in [
            &[1, 150, 171, 256][..],
            &[2, 150, 170, 256],
            &[150, 170, 256],
            &[1, 150, -170, 256],
            &[1, 150, 170, 256, 1],
        ] {
            assert!(
                matches!(
                    logits_grid(shape, 1024, 682),
                    Err(InferenceError::OutputInvalid(_))
                ),
                "{shape:?}"
            );
        }
    }

    #[test]
    fn a_missing_or_wrong_sized_model_file_is_invalid_before_the_runtime_is_touched() {
        let dir =
            std::env::temp_dir().join(format!("rawmakase-inference-size-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(matches!(
            check_files(&dir, &all_files()),
            Err(InferenceError::ModelInvalid(_))
        ));
        for file in all_files() {
            std::fs::write(dir.join(file.name), b"short").unwrap();
        }
        let err = check_files(&dir, &all_files()).unwrap_err();
        std::fs::remove_dir_all(&dir).ok();
        assert!(matches!(err, InferenceError::ModelInvalid(_)));
    }

    #[test]
    fn the_sky_model_is_fed_the_long_edge_and_never_too_narrow_a_side() {
        assert_eq!(sky_input_size(1280, 853), (1024, 682));
        assert_eq!(sky_input_size(853, 1280), (682, 1024));
        assert_eq!(sky_input_size(300, 200), (1024, 683));
        assert_eq!(sky_input_size(1280, 20), (1024, 64));
    }

    #[test]
    fn subject_needs_nothing_of_the_sky_model_and_sky_nothing_of_subjects() {
        let dir =
            std::env::temp_dir().join(format!("rawmakase-inference-sky-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(check_files(&dir, &sky_files()).is_err());
        for file in sky_files() {
            std::fs::File::create(dir.join(file.name))
                .unwrap()
                .set_len(file.size_bytes)
                .unwrap();
        }
        let sky = check_files(&dir, &sky_files());
        let subject = check_files(&dir, &all_files());
        std::fs::remove_dir_all(&dir).ok();
        assert!(sky.is_ok());
        assert!(subject.is_err());
    }
}
