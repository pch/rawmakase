//! The thread that runs selections, one request at a time. Each request renders
//! the model input here, asks [`super::inference`] for the coverage, and keeps a
//! non-empty answer as an unsaved raster. The inference state lives on this
//! thread: one loaded session reused between requests, and the image embedding
//! of the photo last asked about, so a second click on it takes milliseconds
//! instead of a second.
//!
//! Cancelling raises the request's flag, which the runtime notices within a few
//! milliseconds. Quitting hands the thread to the shared shutdown deadline
//! rather than joining it, and the thread keeps its session until it ends, so a
//! native call is never left running on unloaded code.
use super::inference::{self, Ask, State};
use super::{Action, Done, Failure, Generated};
use crate::app::task::Stopping;
use crate::app::worker::Event;
use crate::camera_data::CameraImage;
use crate::model::masks::BitmapSource;
use crate::model::recipe::Recipe;
use crate::storage::bitmaps::Bitmap;
use crate::storage::mask_assets;
use eframe::egui;
use rawmakase_cancel::Cancel;
use rawmakase_inference::RgbImage;
use std::path::PathBuf;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Sender},
};
use std::time::Duration;

/// The longest side of the photo rendered for the model. The network looks at
/// 1024 pixels; more only makes the stored raster larger.
const INPUT_EDGE: u32 = 1280;

/// A selection to make, as the photo and edit were when it was asked for.
pub(super) struct Job {
    pub(super) load: u64,
    pub(super) generation: u64,
    pub(super) cancel: Arc<AtomicBool>,
    pub(super) image: Arc<CameraImage>,
    pub(super) recipe: Recipe,
    /// The model's folder, when the selection runs on this computer.
    pub(super) model: Option<PathBuf>,
    /// The remote worker to ask instead, when one is configured.
    pub(super) endpoint: Option<String>,
    pub(super) action: Action,
}

enum Message {
    Run(Box<Job>, Sender<Event>, egui::Context),
    /// Let go of the model, then say so.
    Unload(Sender<()>),
}

#[derive(Default)]
pub(super) struct Worker {
    messages: Option<Sender<Message>>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// In tests that ask for it, the jobs submitted, kept instead of run: what
    /// the model was asked, with no model or worker present.
    #[cfg(test)]
    pub(super) captured: Option<Vec<Job>>,
}
impl Worker {
    /// Queues `job`; its outcome arrives as an [`Event::Selection`], whatever
    /// happens, so the drawer never waits on a thread that is gone.
    pub(super) fn submit(&mut self, job: Job, tx: Sender<Event>, ctx: egui::Context) {
        #[cfg(test)]
        if let Some(captured) = &mut self.captured {
            captured.push(job);
            return;
        }
        let (load, generation) = (job.load, job.generation);
        let failed = |why: &str| {
            let _ = tx.send(Event::Selection(Box::new(Done {
                load,
                generation,
                result: Err(Failure::Failed(why.into())),
            })));
        };
        if self.thread.as_ref().is_some_and(|t| t.is_finished()) {
            self.messages = None;
            self.thread = None;
        }
        if self.messages.is_none() {
            let (messages, inbox) = mpsc::channel();
            match std::thread::Builder::new()
                .name("subject-select".into())
                .spawn(move || run(inbox))
            {
                Ok(thread) => {
                    self.messages = Some(messages);
                    self.thread = Some(thread);
                }
                Err(e) => return failed(&format!("could not start the selection thread: {e}")),
            }
        }
        let sent = self
            .messages
            .as_ref()
            .is_some_and(|m| m.send(Message::Run(Box::new(job), tx.clone(), ctx)).is_ok());
        if !sent {
            self.messages = None;
            failed("the selection thread stopped");
        }
    }
    /// A closure that makes the thread release the model and waits (briefly) for it
    /// to; for removing the file off the UI thread.
    pub(super) fn unloader(&self) -> impl FnOnce() + Send + 'static + use<> {
        let messages = self.messages.clone();
        move || {
            if let Some(messages) = messages {
                let (ack, done) = mpsc::channel();
                if messages.send(Message::Unload(ack)).is_ok() {
                    // A run that cannot stop leaves the removal waiting only this long.
                    let _ = done.recv_timeout(Duration::from_secs(30));
                }
            }
        }
    }
    /// Closes the mailbox and returns the thread to wait for under the shutdown
    /// deadline.
    pub(super) fn stop(&mut self) -> Stopping {
        self.messages = None;
        Stopping::new(self.thread.take())
    }
}

fn run(inbox: mpsc::Receiver<Message>) {
    let mut state = State::default();
    for message in inbox {
        match message {
            Message::Unload(ack) => {
                state = State::default();
                let _ = ack.send(());
            }
            Message::Run(job, tx, ctx) => {
                let (load, generation) = (job.load, job.generation);
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    select(&job, &mut state)
                }))
                .unwrap_or_else(|_| {
                    // The session may be half-built after a panic.
                    state = State::default();
                    Err(Failure::Failed("the selection stopped unexpectedly".into()))
                });
                let _ = tx.send(Event::Selection(Box::new(Done {
                    load,
                    generation,
                    result,
                })));
                ctx.request_repaint();
            }
        }
    }
}

/// Renders the model input, asks the models, and keeps a non-empty answer as an
/// unsaved raster. All of it runs on the worker thread.
fn select(job: &Job, state: &mut State) -> Result<Generated, Failure> {
    let cancelled = || job.cancel.load(Ordering::Relaxed);
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    let input = crate::develop::masks::selection_input(
        &job.image,
        &job.recipe,
        INPUT_EDGE,
        Cancel::new(&job.cancel),
    )
    .map_err(|e| {
        if cancelled() {
            Failure::Cancelled
        } else {
            Failure::Failed(format!("{e:#}"))
        }
    })?;
    let rgb = RgbImage {
        width: input.width as usize,
        height: input.height as usize,
        data: input.rgb8(),
    };
    let answer = inference::generate(
        &Ask {
            rgb: &rgb,
            action: &job.action,
            model: job.model.as_deref(),
            endpoint: job.endpoint.as_deref(),
            cancel: &job.cancel,
        },
        state,
    )?;
    if cancelled() {
        return Err(Failure::Cancelled);
    }
    // Exactly nothing selected is the only emptiness decided here.
    let coverage = answer.coverage;
    if coverage.data.iter().all(|v| *v == 0) {
        return Err(answer.empty);
    }
    let (width, height) = (coverage.width as u32, coverage.height as u32);
    let id = mask_assets::register(Bitmap {
        width,
        height,
        channels: 1,
        depth: 1,
        data: coverage.data,
    })
    .map_err(|e| Failure::Failed(format!("{e:#}")))?;
    Ok(Generated {
        id,
        width,
        height,
        source: BitmapSource {
            feature: answer.feature.provenance().into(),
            model: answer.model,
            input: answer.input,
        },
    })
}

/// Where selections run, when the user has said they run elsewhere: the
/// endpoint of a `rawmakase-remote-worker`, from `remote-inference.json` in
/// this computer's data folder. Absent — the usual case — means the models run
/// here, as always. See docs/remote-inference.md.
///
/// ```json
/// { "endpoint": "127.0.0.1:8765" }
/// ```
#[derive(Default)]
pub(in crate::app) struct RemoteConfig {
    /// What the file said, and when: read again at most once a second, so the
    /// drawer notices an edit without stat on every frame. Tests set this
    /// directly.
    seen: std::cell::Cell<Option<(std::time::Instant, Option<String>)>>,
}

impl RemoteConfig {
    /// The endpoint to send selections to, if there is one.
    pub(super) fn endpoint(&self) -> Option<String> {
        match self.seen.take() {
            Some((at, endpoint)) if at.elapsed() < Duration::from_secs(1) => {
                self.seen.set(Some((at, endpoint.clone())));
                endpoint
            }
            _ => {
                let endpoint = read_endpoint();
                self.seen
                    .set(Some((std::time::Instant::now(), endpoint.clone())));
                endpoint
            }
        }
    }
    /// Pretends the file said this, for the tests. It is dated an hour ahead,
    /// so a test that stalls past the one-second reread on a loaded runner
    /// still sees it rather than the developer's own file.
    #[cfg(test)]
    pub(super) fn force(&self, endpoint: Option<&str>) {
        self.seen.set(Some((
            std::time::Instant::now() + Duration::from_secs(3600),
            endpoint.map(str::to_string),
        )));
    }
}

/// The endpoint from the configuration file, when it is a usable one.
/// `RAWMAKASE_REMOTE_ENDPOINT` says the same thing for one launch, without
/// touching the file.
fn read_endpoint() -> Option<String> {
    if let Some(endpoint) = std::env::var_os("RAWMAKASE_REMOTE_ENDPOINT")
        .map(|v| v.to_string_lossy().trim().to_string())
        .filter(|text| !text.is_empty())
    {
        return Some(endpoint);
    }
    let path = crate::storage::local_data_dir().join("remote-inference.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value
        .get("endpoint")?
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && !text.chars().any(char::is_whitespace))
        .map(str::to_string)
}
