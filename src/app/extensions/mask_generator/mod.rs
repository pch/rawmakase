//! Extensions' mask generators (`[[mask-generator]]`, docs/extensions/manifest.md)
//! from the app's side: the tiles they offer in the Masking drawer, the
//! generator a saved raster names, and their `mask.generate` and `mask.setup`
//! requests.
//!
//! A request goes through the same state machine as Select Subject's
//! ([`crate::app::develop::tools::subject_mask`]): one at a time, its result
//! checked by load, task generation and the masks' [`Guard`] before it becomes
//! one History step. What differs is who answers it:
//!
//! 1. On the selection worker thread, [`prepare`] renders the photo as the
//!    model sees it, writes it into an input buffer leased from the
//!    extension's [`Ledger`], allocates the coverage buffer beside it, and
//!    names the pixels with [`image_key`].
//! 2. On the UI thread the request is sent ([`Host::generate_mask`]) with a
//!    [`GENERATE_DEADLINE`]; its one outcome comes back as
//!    [`Outcome::Mask`](super::Outcome::Mask), whether the program answered,
//!    crashed, hung, was disabled or let the deadline pass.
//! 3. An answer goes back to the worker thread with the buffers it wrote:
//!    [`finish`] copies the coverage out, checks its size, turns all-zero
//!    coverage or `empty` into the matching failure, and keeps the rest as an
//!    unsaved raster whose [`BitmapSource`] names the generator's provenance,
//!    the model and [`input_identity`].
//!
//! Cancelling, a new photo or a stale result leave the request's buffers to
//! the host, which keeps their lease until the program answers
//! (docs/extensions/protocol.md#leases).
//!
//! [`Guard`]: crate::app::develop::tools::subject_mask
use super::{Extension, Host, Pending};
use crate::app::develop::tools::subject_mask::{
    Done, Failure, Feature, Generated, keep, render_input,
};
use crate::app::worker::Event;
use crate::camera_data::CameraImage;
use crate::model::masks::{BitmapSource, FEATURE_SKY, FEATURE_SUBJECT};
use crate::model::recipe::Recipe;
use crate::storage::mask_assets::MAX_SIDE;
use crate::storage::{FNV_OFFSET, fnv1a};
use rawmakase_extensions::protocol::buffer::{Access, Buffer, Meaning, Region, Sample, Space};
use rawmakase_extensions::protocol::manifest::{AI_MASKS, Input, LEGACY_PROVENANCE};
use rawmakase_extensions::protocol::message::NeedsSetup;
use rawmakase_extensions::protocol::method::{
    AppRequest, EmptyMask, MaskGenerate, MaskGenerated, MaskSetup,
};
use rawmakase_extensions::protocol::viewer::ViewerInput;
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use rawmakase_extensions::shm::{Buffers, Ledger, Returned};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How long `mask.generate` may take: a remote worker may take up to three
/// minutes, as Select Subject's does today.
pub(in crate::app) const GENERATE_DEADLINE: Duration = Duration::from_secs(200);
/// How long `mask.setup` may take. Progress doesn't extend it; a download
/// that takes longer is cancelled, and resumes at the next Download.
pub(in crate::app) const SETUP_DEADLINE: Duration = Duration::from_secs(3600);

/// One of an extension's mask generators as a request asks it: through one
/// of its tiles, or as the generator a saved raster names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct Offer {
    pub(in crate::app) extension: String,
    /// The generator's id within its extension.
    pub(in crate::app) generator: String,
    /// The tile's name: the new mask's, and its History step's.
    pub(in crate::app) name: String,
    /// Whether a new component is inverted (a Background-style tile).
    pub(in crate::app) invert: bool,
    /// What its rasters record as their source.
    pub(in crate::app) provenance: String,
    /// It takes `none`: an automatic mask.
    pub(in crate::app) auto: bool,
    /// It takes `prompt`: aiming and refining with clicks and a box.
    pub(in crate::app) prompt: bool,
    /// The manifest's `prompt-hint`.
    pub(in crate::app) hint: Option<String>,
}

impl Offer {
    /// The new mask's name, and its History step's.
    pub(in crate::app) fn name(&self) -> &str {
        &self.name
    }
    /// Which of the selections that came before extensions it is: the
    /// bundled AI masks' generators, the only ones that may record `subject`
    /// or `sky`. The drawer gives them the icons and wording it always had.
    pub(in crate::app) fn feature(&self) -> Option<Feature> {
        match self.provenance.as_str() {
            FEATURE_SKY => Some(Feature::Sky),
            FEATURE_SUBJECT if self.invert => Some(Feature::Background),
            FEATURE_SUBJECT => Some(Feature::Subject),
            _ => None,
        }
    }
    /// Whether it aims with clicks and a box: Sky doesn't.
    pub(in crate::app) fn takes_prompt(&self) -> bool {
        self.prompt
    }
    /// Whether it makes a mask by itself.
    pub(in crate::app) fn takes_none(&self) -> bool {
        self.auto
    }
    /// The line shown while aiming at something new.
    pub(in crate::app) fn aim_hint(&self) -> String {
        self.hint
            .clone()
            .unwrap_or_else(|| "Click it on the photo, or drag a box around it".into())
    }
}

/// A mask generator's setup, while it runs or after it failed.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Setup {
    /// Sent, with the program's last progress.
    Running {
        done: Option<f64>,
        text: Option<String>,
    },
    /// It failed, with why.
    Failed(String),
}

impl Extension {
    /// `generator` offered as `name`, inverted or not.
    fn offer(
        &self,
        generator: &rawmakase_extensions::protocol::manifest::MaskGenerator,
        name: &str,
        invert: bool,
    ) -> Offer {
        Offer {
            extension: self.id().to_string(),
            generator: generator.id.clone(),
            name: name.to_string(),
            invert,
            provenance: generator.provenance(self.id()),
            auto: generator.input.contains(&Input::None),
            prompt: generator.input.contains(&Input::Prompt),
            hint: generator.prompt_hint.clone(),
        }
    }
}

impl Host {
    /// The tiles of the enabled extensions' mask generators, for the Masking
    /// drawer, each with why it can't run now (a Failed extension, a program
    /// missing). Disabled extensions offer none.
    pub(in crate::app) fn mask_tiles(&self) -> Vec<(Offer, Option<String>)> {
        let mut tiles = Vec::new();
        for e in self.extensions.iter().filter(|e| e.enabled()) {
            let why = e.can_run().err();
            for g in &e.manifest.mask_generators {
                for tile in &g.tiles {
                    tiles.push((e.offer(g, &tile.name, tile.invert), why.clone()));
                }
            }
        }
        tiles
    }

    /// The generator whose rasters record `provenance`, for refining or
    /// regenerating one: named as its tile of the same inversion is, else by
    /// its id. `Err` says why it can't run: its extension is disabled, Failed
    /// or not installed at all. Saved masks render either way.
    pub(in crate::app) fn mask_generator(
        &self,
        provenance: &str,
        invert: bool,
    ) -> Result<Offer, String> {
        let found = self.extensions.iter().find_map(|e| {
            let g = e
                .manifest
                .mask_generators
                .iter()
                .find(|g| g.provenance(e.id()) == provenance)?;
            let name = g
                .tiles
                .iter()
                .find(|t| t.invert == invert)
                .or(g.tiles.first())
                .map_or(g.id.as_str(), |t| t.name.as_str());
            Some((e, e.offer(g, name, invert)))
        });
        match found {
            Some((e, offer)) => e.can_run().map(|()| offer),
            None => Err(match provenance.split_once('/') {
                Some((extension, _)) => {
                    format!("Made by {extension}, which isn't installed")
                }
                // Subject's and Sky's, saved before extensions or since.
                None if LEGACY_PROVENANCE.contains(&provenance) => {
                    format!("Made by {AI_MASKS}, which isn't installed")
                }
                None => format!("Made by {provenance}, which isn't installed"),
            }),
        }
    }

    /// Where a raster recording `provenance` came from, for the component
    /// settings: its generator's extension by name, whatever its state, or
    /// that none here has it.
    pub(in crate::app) fn mask_source_text(&self, provenance: &str) -> String {
        for e in &self.extensions {
            if let Some(g) = e
                .manifest
                .mask_generators
                .iter()
                .find(|g| g.provenance(e.id()) == provenance)
            {
                return format!("Made by {} ({})", e.name(), g.id);
            }
        }
        match provenance.split_once('/') {
            Some((extension, _)) => format!("Made by {extension}, which isn't installed"),
            None => format!("Made by {provenance}"),
        }
    }

    /// Whether `offer`'s extension can take a request now, and still has its
    /// generator.
    pub(in crate::app) fn mask_ready(&self, offer: &Offer) -> Result<(), String> {
        let e = self
            .get(&offer.extension)
            .ok_or_else(|| format!("{} isn't installed", offer.extension))?;
        if !e
            .manifest
            .mask_generators
            .iter()
            .any(|g| g.id == offer.generator)
        {
            return Err(format!("{} no longer has {}", e.name(), offer.generator));
        }
        if self.closed {
            return Err("RAWmakase is quitting".into());
        }
        e.can_run()
    }

    /// Where `extension` is in its lifecycle: a request captures it when it
    /// starts, and is void once it differs (the extension was disabled,
    /// uninstalled, loaded again, or its program crashed or hung meanwhile).
    /// `None` once it isn't loaded at all.
    pub(in crate::app) fn mask_lifecycle(&self, extension: &str) -> Option<u64> {
        self.get(extension)
            .map(|e| e.lifecycle.load(std::sync::atomic::Ordering::SeqCst))
    }

    /// What `extension`'s requests' buffers are leased against.
    pub(in crate::app) fn mask_ledger(&self, extension: &str) -> Option<Ledger> {
        self.get(extension).map(|e| e.ledger.clone())
    }

    /// Sends `mask.generate` with `buffers` to `offer`'s extension, starting
    /// its program first when it isn't running. Returns the ticket its
    /// [`Outcome::Mask`](super::Outcome::Mask) comes back with.
    pub(in crate::app) fn generate_mask(
        &mut self,
        offer: &Offer,
        params: MaskGenerate,
        buffers: Buffers,
    ) -> Result<u64, String> {
        self.mask_ready(offer)?;
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        #[cfg(test)]
        self.masks_sent.push(params.clone());
        self.send(
            &offer.extension,
            AppRequest::MaskGenerate(params),
            Pending::Mask { ticket },
            Some(buffers),
        )?;
        Ok(ticket)
    }

    /// Cancels mask request `ticket` of `extension`, if it is still in
    /// flight: the program is told, and a reply it sends after is dropped.
    /// The request's buffers stay leased until that reply.
    pub(in crate::app) fn cancel_mask(&mut self, extension: &str, ticket: u64) {
        let Some(run) = self
            .extensions
            .iter_mut()
            .find(|e| e.id() == extension)
            .and_then(|e| e.run.as_mut())
        else {
            return;
        };
        run.queued.retain(|q| q.pending != Pending::Mask { ticket });
        let sent = run
            .requests
            .iter()
            .find(|(_, p)| **p == Pending::Mask { ticket })
            .map(|(id, _)| *id);
        if let Some(id) = sent {
            run.requests.remove(&id);
            run.session.cancel(id);
        }
    }

    /// What `extension`'s program last said of the mask request it is
    /// answering, while that request waits for its outcome.
    pub(in crate::app) fn mask_progress(&self, extension: &str) -> Option<&str> {
        let (ticket, text) = self.get(extension)?.mask_progress.as_ref()?;
        self.mask_in_flight(extension, *ticket)
            .then_some(text.as_str())
    }

    /// Whether mask request `ticket` of `extension` is still waiting for its
    /// outcome.
    pub(in crate::app) fn mask_in_flight(&self, extension: &str, ticket: u64) -> bool {
        self.get(extension)
            .and_then(|e| e.run.as_ref())
            .is_some_and(|run| {
                run.requests
                    .values()
                    .chain(run.queued.iter().map(|q| &q.pending))
                    .any(|p| *p == Pending::Mask { ticket })
            })
    }

    /// Sends `mask.setup` for `generator` of `extension`: the Download of the
    /// setup card. Its progress shows in [`Host::mask_setup`] until its
    /// [`Outcome::MaskSetup`](super::Outcome::MaskSetup).
    pub(in crate::app) fn setup_mask(
        &mut self,
        extension: &str,
        generator: &str,
    ) -> Result<(), String> {
        if matches!(
            self.mask_setup(extension, generator),
            Some(Setup::Running { .. })
        ) {
            return Ok(());
        }
        let request = AppRequest::MaskSetup(MaskSetup {
            generator: generator.to_string(),
        });
        let pending = Pending::MaskSetup {
            generator: generator.to_string(),
        };
        let result = self.send(extension, request, pending, None);
        if let Some(index) = self.index(extension) {
            let state = match &result {
                Ok(()) => Setup::Running {
                    done: None,
                    text: None,
                },
                Err(why) => Setup::Failed(why.clone()),
            };
            self.extensions[index]
                .setups
                .insert(generator.to_string(), state);
        }
        result
    }

    /// Cancels the setup of `generator` of `extension`.
    pub(in crate::app) fn cancel_mask_setup(&mut self, extension: &str, generator: &str) {
        let Some(index) = self.index(extension) else {
            return;
        };
        let e = &mut self.extensions[index];
        e.setups.remove(generator);
        let Some(run) = e.run.as_mut() else { return };
        let pending = Pending::MaskSetup {
            generator: generator.to_string(),
        };
        run.queued.retain(|q| q.pending != pending);
        let sent = run
            .requests
            .iter()
            .find(|(_, p)| **p == pending)
            .map(|(id, _)| *id);
        if let Some(id) = sent {
            run.requests.remove(&id);
            run.session.cancel(id);
        }
    }

    /// The setup of `generator` of `extension`, while it runs or after it
    /// failed.
    pub(in crate::app) fn mask_setup(&self, extension: &str, generator: &str) -> Option<&Setup> {
        self.get(extension)?.setups.get(generator)
    }

    /// The mask requests sent so far, for tests.
    #[cfg(test)]
    pub(in crate::app) fn masks_sent(&self) -> &[MaskGenerate] {
        &self.masks_sent
    }
}

/// What a request's error means to the drawer: `needs_setup` asks for its
/// setup, `unavailable` and anything else say why, and a cancelled or
/// disabled request says nothing.
pub(in crate::app) fn failure(error: RemoteError) -> Failure {
    match error.code {
        ErrorCode::Cancelled | ErrorCode::Disabled => Failure::Cancelled,
        ErrorCode::NeedsSetup => match NeedsSetup::read(&error.message) {
            Some(setup) => Failure::NeedsSetup(setup),
            // A message this version doesn't read shows as plain text.
            None => Failure::Failed(error.message),
        },
        ErrorCode::Unavailable => Failure::RuntimeUnavailable(error.message),
        _ => Failure::Failed(error.message),
    }
}

/// A request's input to render on the worker thread, as it was asked for.
pub(in crate::app) struct Prepare {
    pub(in crate::app) load: u64,
    pub(in crate::app) generation: u64,
    pub(in crate::app) cancel: Arc<AtomicBool>,
    pub(in crate::app) image: Arc<CameraImage>,
    pub(in crate::app) recipe: Recipe,
    pub(in crate::app) ledger: Ledger,
    pub(in crate::app) generator: String,
    pub(in crate::app) input: Option<ViewerInput>,
}

/// A request's input, rendered into its buffers, or why it couldn't be.
pub(crate) struct Prepared {
    pub(crate) load: u64,
    pub(crate) generation: u64,
    pub(crate) result: Result<Ready, Failure>,
}

/// A request ready to send. Dropping it lets go of its buffers.
pub(crate) struct Ready {
    pub(crate) params: MaskGenerate,
    pub(crate) buffers: Buffers,
}

/// Renders the photo for a request into its buffers: an input of 8-bit sRGB,
/// sealed once written, and a zeroed coverage of the same size. Runs on the
/// selection worker thread.
pub(in crate::app) fn prepare(job: Prepare) -> Event {
    let result = (|| {
        let (width, height, rgb) = render_input(&job.image, &job.recipe, &job.cancel)?;
        let image_key = image_key(width, height, &rgb);
        let refused = |e: RemoteError| Failure::Failed(e.message);
        let mut buffers = Buffers::new(&job.ledger);
        let image = buffers
            .input(rgb.len() as u64, |bytes| bytes.copy_from_slice(&rgb))
            .map_err(refused)?;
        let coverage = buffers
            .output(u64::from(width) * u64::from(height))
            .map_err(refused)?;
        // Buffer pixels per pixel of the photo, along its long side.
        let long = job.image.width.max(job.image.height).max(1);
        let region = Region {
            space: Space::ImageFrame,
            rect: [0., 0., 1., 1.],
            scale: width.max(height) as f32 / long as f32,
            overlap: 0,
        };
        let image = Buffer {
            handle: image,
            bytes: rgb.len() as u64,
            width,
            height,
            stride: width * 3,
            channels: 3,
            sample: Sample::U8,
            meaning: MaskGenerate::IMAGE,
            region,
            access: Access::Read,
        };
        let coverage = Buffer {
            handle: coverage,
            bytes: u64::from(width) * u64::from(height),
            stride: width,
            channels: 1,
            meaning: Meaning::Coverage,
            access: Access::Write,
            ..image
        };
        let params = MaskGenerate {
            generator: job.generator,
            input: job.input,
            image,
            coverage,
            image_key,
        };
        params.validate().map_err(refused)?;
        Ok(Ready { params, buffers })
    })();
    Event::MaskPrepared(Box::new(Prepared {
        load: job.load,
        generation: job.generation,
        result,
    }))
}

/// A request's answer to keep, with what it was asked: the job's captured
/// inputs, and the buffers the program wrote.
pub(in crate::app) struct Finish {
    pub(in crate::app) load: u64,
    pub(in crate::app) generation: u64,
    pub(in crate::app) cancel: Arc<AtomicBool>,
    pub(in crate::app) asked: MaskGenerate,
    pub(in crate::app) answer: MaskGenerated,
    pub(in crate::app) buffers: Returned,
    pub(in crate::app) provenance: String,
}

/// Copies the coverage out of a request's buffers and keeps it as an
/// unsaved raster, on the selection worker thread. The buffers' lease ends
/// when this returns.
pub(in crate::app) fn finish(job: Finish) -> Event {
    let result = (|| {
        let coverage = &job.asked.coverage;
        let (width, height) = (coverage.width, coverage.height);
        let data = copy_out(job.buffers.read(coverage.handle), width, height)?;
        drop(job.buffers);
        if job.cancel.load(Ordering::Relaxed) {
            return Err(Failure::Cancelled);
        }
        let empty = match job.answer.empty {
            Some(EmptyMask::NoSubject) => Failure::NoSubject,
            Some(EmptyMask::NoSky) => Failure::NoSky,
            Some(EmptyMask::NothingThere) => Failure::NothingThere,
            None if job.asked.input.is_some() => Failure::NothingThere,
            None => Failure::NothingFound,
        };
        if job.answer.empty.is_some() {
            return Err(empty);
        }
        let input = input_identity(&job.asked, &job.answer.model_key);
        keep(
            width,
            height,
            data,
            empty,
            BitmapSource {
                feature: job.provenance,
                model: job.answer.model,
                input,
            },
        )
    })();
    done(job.load, job.generation, result)
}

/// A request's outcome for the state machine.
pub(in crate::app) fn done(
    load: u64,
    generation: u64,
    result: Result<Generated, Failure>,
) -> Event {
    Event::Selection(Box::new(Done {
        load,
        generation,
        result,
    }))
}

/// The coverage of `width` by `height` an output holds, rows packed. Refused
/// when it is larger than a raster may be, or the output smaller than the
/// coverage asked for.
pub(in crate::app) fn copy_out(
    output: Option<&[u8]>,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Failure> {
    if !(1..=MAX_SIDE).contains(&width) || !(1..=MAX_SIDE).contains(&height) {
        return Err(Failure::Failed(format!(
            "a {width}x{height} coverage is larger than a mask raster may be"
        )));
    }
    let len = width as usize * height as usize;
    match output {
        Some(bytes) if bytes.len() >= len => Ok(bytes[..len].to_vec()),
        Some(bytes) => Err(Failure::Failed(format!(
            "the coverage is {} bytes, not {len}",
            bytes.len()
        ))),
        None => Err(Failure::Failed("the request's coverage is missing".into())),
    }
}

/// The app's digest of a request's image: its size, its pixels and the
/// version of how they were rendered (`SELECTION_INPUT_VERSION`).
pub(in crate::app) fn image_key(width: u32, height: u32, rgb: &[u8]) -> String {
    let header = [
        width.to_le_bytes(),
        height.to_le_bytes(),
        crate::develop::masks::SELECTION_INPUT_VERSION.to_le_bytes(),
    ]
    .concat();
    let hash = |seed| fnv1a(fnv1a(seed, &header), rgb);
    format!("{:016x}{:016x}", hash(FNV_OFFSET), hash(0x84222325cbf29ce4))
}

/// What a raster records as made from: the image, the model's `model_key`,
/// the generator and the input, so its provenance names exactly what made
/// it. Saved values are only read, never recomputed.
pub(in crate::app) fn input_identity(asked: &MaskGenerate, model_key: &str) -> String {
    let source = format!(
        "{:016x}",
        fnv1a(
            fnv1a(FNV_OFFSET, asked.image_key.as_bytes()),
            model_key.as_bytes()
        )
    );
    let mut text = format!("{source}|{}", asked.generator);
    match &asked.input {
        None => text.push_str("|auto"),
        Some(ViewerInput::Prompt { clicks, bounds }) => {
            text.push_str("|prompt");
            for c in clicks {
                text.push_str(&format!("|{:.4},{:.4},{}", c.x, c.y, c.positive));
            }
            if let Some(b) = bounds {
                text.push_str(&format!("|{:.4?}", [b.x0, b.y0, b.x1, b.y1]));
            }
        }
        // Only `prompt` is collected for mask generators so far.
        Some(other) => text.push_str(&format!("|{}", serde_json::json!(other))),
    }
    format!("{:016x}", fnv1a(FNV_OFFSET, text.as_bytes()))
}

#[cfg(test)]
mod tests;
