//! Extensions' retouch generators (`[[retouch-generator]]`,
//! docs/extensions/manifest.md) from the app's side: the generators the
//! Remove tool offers as AI Remove (experimental), whether one can run, the
//! user's consent to send pixels to the service behind it, and its
//! `retouch.generate` requests.
//!
//! A request goes through [`crate::app::develop::tools::ai_remove`]'s state
//! machine, one at a time, as Select Subject's does through `subject_mask`:
//!
//! 1. On the AI Remove worker thread, [`prepare`] captures the rendition
//!    ([`crate::develop::retouch::rendition::Rendition::of`]), renders the
//!    region around the painted area and its coverage into input buffers
//!    leased from the extension's [`Ledger`], allocates the result beside
//!    them and names the pixels with [`image_key`].
//! 2. On the UI thread it is sent ([`Host::generate_retouch`]) with a
//!    [`GENERATE_DEADLINE`]; its one outcome comes back as
//!    [`Outcome::Retouch`](super::Outcome::Retouch), however it ends.
//! 3. An answer goes back to the worker thread with the buffers: [`finish`]
//!    copies the result out, inverts the captured rendition and keeps the
//!    camera RGB as an unsaved patch in the asset store.
//!
//! Cancelling, a new photo or a stale result leave the buffers to the host,
//! which keeps their lease until the program answers.
use super::{Host, Pending};
use crate::app::develop::tools::ai_remove::{Done, Failure, Generated};
use crate::app::worker::Event;
use crate::camera_data::CameraImage;
use crate::develop::retouch::rendition::{self, Rendition};
use crate::model::recipe::Recipe;
use crate::model::retouch::{RetouchOp, RetouchPatch};
use crate::storage::{FNV_OFFSET, fnv1a};
use rawmakase_extensions::protocol::buffer::{Access, Buffer, Meaning, Region, Sample, Space};
use rawmakase_extensions::protocol::method::{AppRequest, RetouchGenerate, RetouchGenerated};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use rawmakase_extensions::shm::{Buffers, Ledger, Returned};
use rawmakase_extensions::state::State;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long `retouch.generate` may take: a hosted model's round trip, with an
/// upload, as `mask.generate` allows.
pub(in crate::app) const GENERATE_DEADLINE: Duration = Duration::from_secs(200);

/// The version of how a request's image and coverage are rendered, part of
/// [`image_key`].
const INPUT_VERSION: u32 = 1;

/// One of an extension's retouch generators, as the Remove tool offers it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Offer {
    pub(in crate::app) extension: String,
    /// The generator's id within its extension.
    pub(in crate::app) generator: String,
    /// Its name in the Remove tool.
    pub(in crate::app) name: String,
    /// It sends what it is given off this computer.
    pub(in crate::app) uploads: bool,
    /// What its patches record as their generator.
    pub(in crate::app) provenance: String,
    /// The extension's name, for what the tool says.
    pub(in crate::app) extension_name: String,
}

impl Host {
    /// The enabled extensions' retouch generators, each with why it can't
    /// run now. Disabled extensions offer none.
    pub(in crate::app) fn retouch_offers(&self) -> Vec<(Offer, Option<String>)> {
        let mut offers = Vec::new();
        for e in self.extensions.iter().filter(|e| e.enabled()) {
            let why = e.can_run().err();
            for g in &e.manifest.retouch_generators {
                offers.push((
                    Offer {
                        extension: e.id().to_string(),
                        generator: g.id.clone(),
                        name: g.name.clone(),
                        uploads: g.uploads,
                        provenance: g.provenance(e.id()),
                        extension_name: e.name().to_string(),
                    },
                    why.clone(),
                ));
            }
        }
        offers
    }

    /// Whether `offer`'s extension can take a request now: installed, still
    /// declaring the generator, able to run, and its secret settings read.
    pub(in crate::app) fn retouch_ready(&self, offer: &Offer) -> Result<(), String> {
        let e = self
            .get(&offer.extension)
            .ok_or_else(|| format!("{} isn't installed", offer.extension))?;
        if !e
            .manifest
            .retouch_generators
            .iter()
            .any(|g| g.id == offer.generator)
        {
            return Err(format!("{} no longer has {}", e.name(), offer.generator));
        }
        if self.closed {
            return Err("RAWmakase is quitting".into());
        }
        e.can_run()?;
        if e.secrets_loading() {
            return Err(format!(
                "{} is still reading its settings from the system's credential store",
                e.name()
            ));
        }
        Ok(())
    }

    /// Whether the user agreed that `extension`'s generators send pixels to
    /// the service behind them.
    pub(in crate::app) fn uploads_allowed(&self, extension: &str) -> bool {
        self.get(extension).is_some_and(|e| e.entry.uploads_allowed)
    }

    /// Records the user's consent to `extension`'s uploads, in
    /// `extensions.json` first.
    pub(in crate::app) fn allow_uploads(&mut self, extension: &str) -> Result<(), String> {
        let store = self
            .store
            .clone()
            .ok_or("Extensions are off in this editor")?;
        let index = self
            .index(extension)
            .ok_or_else(|| format!("No extension {extension}"))?;
        State::update(store.data_dir(), |state| {
            state.entry(extension).uploads_allowed = true;
            Ok(())
        })
        .map_err(|err| format!("{} not changed: {err}", self.extensions[index].name()))?;
        self.extensions[index].entry.uploads_allowed = true;
        Ok(())
    }

    /// Sends `retouch.generate` with `buffers` to `offer`'s extension,
    /// starting its program first when it isn't running. Returns the ticket
    /// its [`Outcome::Retouch`](super::Outcome::Retouch) comes back with.
    pub(in crate::app) fn generate_retouch(
        &mut self,
        offer: &Offer,
        params: RetouchGenerate,
        buffers: Buffers,
    ) -> Result<u64, String> {
        self.retouch_ready(offer)?;
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        #[cfg(test)]
        self.retouches_sent.push(params.clone());
        self.send(
            &offer.extension,
            AppRequest::RetouchGenerate(params),
            Pending::Retouch { ticket },
            Some(buffers),
        )?;
        Ok(ticket)
    }

    /// Cancels retouch request `ticket` of `extension`, if it is still in
    /// flight: the program is told, and a reply it sends after is dropped.
    /// The request's buffers stay leased until that reply.
    pub(in crate::app) fn cancel_retouch(&mut self, extension: &str, ticket: u64) {
        let Some(run) = self
            .extensions
            .iter_mut()
            .find(|e| e.id() == extension)
            .and_then(|e| e.run.as_mut())
        else {
            return;
        };
        run.queued
            .retain(|q| q.pending != Pending::Retouch { ticket });
        let sent = run
            .requests
            .iter()
            .find(|(_, p)| **p == Pending::Retouch { ticket })
            .map(|(id, _)| *id);
        if let Some(id) = sent {
            run.requests.remove(&id);
            run.session.cancel(id);
        }
    }

    /// Whether retouch request `ticket` of `extension` is still waiting for
    /// its outcome.
    pub(in crate::app) fn retouch_in_flight(&self, extension: &str, ticket: u64) -> bool {
        self.get(extension)
            .and_then(|e| e.run.as_ref())
            .is_some_and(|run| {
                run.requests
                    .values()
                    .chain(run.queued.iter().map(|q| &q.pending))
                    .any(|p| *p == Pending::Retouch { ticket })
            })
    }

    /// The retouch requests sent so far, for tests.
    #[cfg(test)]
    pub(in crate::app) fn retouches_sent(&self) -> &[RetouchGenerate] {
        &self.retouches_sent
    }
}

/// What a request's error means to the tool: `needs_setup` asks the user to
/// set the extension up on its page, `unavailable` and `refused` say why,
/// and a cancelled or disabled request says nothing.
pub(in crate::app) fn failure(error: RemoteError) -> Failure {
    match error.code {
        ErrorCode::Cancelled | ErrorCode::Disabled => Failure::Cancelled,
        ErrorCode::NeedsSetup => Failure::NeedsSetup(error.message),
        ErrorCode::Unavailable => Failure::Unavailable(error.message),
        ErrorCode::Refused => Failure::Refused(error.message),
        _ => Failure::Failed(error.message),
    }
}

/// A request's input to render on the worker thread, as it was asked for:
/// everything is captured when the request starts.
pub(in crate::app) struct Prepare {
    pub(in crate::app) load: u64,
    pub(in crate::app) generation: u64,
    pub(in crate::app) cancel: Arc<AtomicBool>,
    pub(in crate::app) image: Arc<CameraImage>,
    pub(in crate::app) recipe: Recipe,
    pub(in crate::app) ai: Option<crate::develop::AiSource>,
    /// The painted shape, with the tool's feather and opacity.
    pub(in crate::app) op: RetouchOp,
    pub(in crate::app) ledger: Ledger,
    pub(in crate::app) generator: String,
}

/// A request's input, rendered into its buffers, or why it couldn't be.
pub(crate) struct Prepared {
    pub(crate) load: u64,
    pub(crate) generation: u64,
    pub(crate) result: Result<Ready, Failure>,
}

/// A request ready to send, with what its answer is inverted through.
/// Dropping it lets go of its buffers.
pub(crate) struct Ready {
    pub(crate) params: RetouchGenerate,
    pub(crate) buffers: Buffers,
    pub(crate) rendition: Rendition,
    pub(crate) region: rendition::Region,
}

/// Renders a request's region and coverage into its buffers: two inputs,
/// sealed once written, and a zeroed result of the image's size. Runs on the
/// AI Remove worker thread.
pub(in crate::app) fn prepare(job: Prepare) -> Event {
    let result = (|| {
        let cancelled = || job.cancel.load(Ordering::Relaxed);
        if cancelled() {
            return Err(Failure::Cancelled);
        }
        let rendition = Rendition::of(&job.image.metadata, &job.recipe)
            .map_err(|e| Failure::Failed(format!("{e:#}")))?;
        let input = rendition::render(
            &job.image,
            &job.recipe,
            job.ai.as_ref(),
            &job.op,
            &rendition,
            rawmakase_cancel::Cancel::new(&job.cancel),
        )
        .map_err(|e| {
            if cancelled() {
                Failure::Cancelled
            } else {
                Failure::Failed(format!("{e:#}"))
            }
        })?;
        let (width, height) = (input.region.width, input.region.height);
        let image_key = image_key(width, height, &input.rgb, &input.coverage);
        let refused = |e: RemoteError| Failure::Failed(e.message);
        let mut buffers = Buffers::new(&job.ledger);
        let rgb_bytes = input.rgb.len() as u64;
        let image = buffers
            .input(rgb_bytes, |bytes| bytes.copy_from_slice(&input.rgb))
            .map_err(refused)?;
        let coverage = buffers
            .input(input.coverage.len() as u64, |bytes| {
                bytes.copy_from_slice(&input.coverage)
            })
            .map_err(refused)?;
        let result = buffers.output(rgb_bytes).map_err(refused)?;
        let region = Region {
            space: Space::ImageFrame,
            rect: input.region.rect,
            scale: input.region.scale,
            overlap: 0,
        };
        let image = Buffer {
            handle: image,
            bytes: rgb_bytes,
            width,
            height,
            stride: width * 3,
            channels: 3,
            sample: Sample::U8,
            meaning: RetouchGenerate::IMAGE,
            region,
            access: Access::Read,
        };
        let coverage = Buffer {
            handle: coverage,
            bytes: input.coverage.len() as u64,
            stride: width,
            channels: 1,
            meaning: Meaning::Coverage,
            ..image
        };
        let result = Buffer {
            handle: result,
            access: Access::Write,
            ..image
        };
        let params = RetouchGenerate {
            generator: job.generator,
            image,
            coverage,
            result,
            image_key,
        };
        params.validate().map_err(refused)?;
        Ok(Ready {
            params,
            buffers,
            rendition,
            region: input.region,
        })
    })();
    Event::RetouchPrepared(Box::new(Prepared {
        load: job.load,
        generation: job.generation,
        result,
    }))
}

/// A request's answer to keep, with what it was asked and captured.
pub(in crate::app) struct Finish {
    pub(in crate::app) load: u64,
    pub(in crate::app) generation: u64,
    pub(in crate::app) cancel: Arc<AtomicBool>,
    pub(in crate::app) asked: RetouchGenerate,
    pub(in crate::app) answer: RetouchGenerated,
    pub(in crate::app) buffers: Returned,
    pub(in crate::app) rendition: Rendition,
    pub(in crate::app) region: rendition::Region,
    pub(in crate::app) provenance: String,
    /// Whether the region was made with AI Denoise.
    pub(in crate::app) ai_denoise: bool,
}

/// Copies the result out of a request's buffers, inverts the rendition and
/// keeps the camera RGB as an unsaved patch, on the worker thread. The
/// buffers' lease ends when this returns.
pub(in crate::app) fn finish(job: Finish) -> Event {
    let result = (|| {
        let out = &job.asked.result;
        let (width, height, stride) = (out.width, out.height, out.stride as usize);
        let bytes = match job.buffers.read(out.handle) {
            Some(bytes) if bytes.len() >= stride * (height as usize - 1) + width as usize * 3 => {
                bytes.to_vec()
            }
            Some(_) => return Err(Failure::Failed("the result is smaller than asked".into())),
            None => return Err(Failure::Failed("the request's result is missing".into())),
        };
        drop(job.buffers);
        if job.cancel.load(Ordering::Relaxed) {
            return Err(Failure::Cancelled);
        }
        let samples = rendition::patch_samples(&job.rendition, &bytes, width, height, stride)
            .map_err(|e| Failure::Failed(format!("{e:#}")))?;
        let bitmap = crate::storage::patches::encode(width, height, &samples)
            .map_err(|e| Failure::Failed(format!("{e:#}")))?;
        let id = crate::storage::mask_assets::register(bitmap)
            .map_err(|e| Failure::Failed(format!("{e:#}")))?;
        Ok(Generated {
            patch: RetouchPatch {
                id,
                width,
                height,
                rect: job.region.rect,
                generator: job.provenance,
                model: job.answer.model,
                ai_denoise: job.ai_denoise,
            },
        })
    })();
    done(job.load, job.generation, result)
}

/// A request's outcome for the state machine.
pub(in crate::app) fn done(
    load: u64,
    generation: u64,
    result: Result<Generated, Failure>,
) -> Event {
    Event::AiRemoved(Box::new(Done {
        load,
        generation,
        result,
    }))
}

/// The app's digest of a request's image and coverage: their size, pixels
/// and the version of how they were rendered.
pub(in crate::app) fn image_key(width: u32, height: u32, rgb: &[u8], coverage: &[u8]) -> String {
    let header = [
        width.to_le_bytes(),
        height.to_le_bytes(),
        INPUT_VERSION.to_le_bytes(),
    ]
    .concat();
    let hash = |seed| fnv1a(fnv1a(fnv1a(seed, &header), rgb), coverage);
    format!("{:016x}{:016x}", hash(FNV_OFFSET), hash(0x84222325cbf29ce4))
}
