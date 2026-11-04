//! The worker's side of the protocol: the engine that runs the models, the
//! few photos' analyses it keeps, and the connection loop that turns frames
//! into engine calls. One model run at a time; a photo is analysed once and
//! its clicks answered from what was kept; every request is checked against
//! the protocol's bounds before the engine sees it, so a malformed or
//! oversized frame is an error reply, never a model run on nonsense or an
//! unbounded allocation.

use rawmakase_cancel::Cancel;
use rawmakase_inference::{
    Analysis, Coverage, InferenceError, LoadOptions, Prompt, RgbImage, SKY, Session, Sky,
    SkySession, Subject,
};
use std::collections::{HashMap, VecDeque};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::protocol::{
    self, Code, MAX_FRAME_BYTES, MAX_IMAGE_BYTES, MAX_LABEL_BYTES, Op, Reply, Request, Response,
    WireCoverage,
};

/// How long a connection may sit without sending anything. Long enough for a
/// client that thinks between clicks; short enough that a stopped one does not
/// hold a thread forever.
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(300);

/// Analyses the worker keeps: the photo being selected on and the one before
/// it.
pub const ENTRIES: usize = 2;
/// Ceiling on what the kept analyses may hold together; the oldest goes first.
pub const MAX_CACHED_BYTES: usize = 512 << 20;

/// What a worker actually runs. `Model` is the real one over the local ONNX
/// sessions; the tests stand in a fake, which is why these three calls are the
/// whole surface.
pub trait Engine: Send {
    /// Runs the models on `photo`, or answers from the analysis it kept.
    fn analyze(&mut self, photo: &RgbImage, cancel: Cancel<'_>) -> Result<Outcome, EngineError>;
    /// The mask for `prompt` on the analysis `key` names.
    fn segment(
        &mut self,
        key: &str,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, EngineError>;
    /// The photo's sky, from its own model.
    fn sky(&mut self, photo: &RgbImage, cancel: Cancel<'_>) -> Result<Coverage, EngineError>;
    /// What a `ping` says about this worker, as the runtime reported it.
    fn describe(&self) -> Info;
}

/// Why the engine could not answer, in the protocol's own terms.
#[derive(Debug, Clone)]
pub enum EngineError {
    /// The caller's cancel flag was raised in time.
    Cancelled,
    /// No analysis under this `key`; the client should analyse again.
    UnknownKey,
    /// The engine tried and failed, or was given something unusable.
    Failed(InferenceError),
}

/// What [`Engine::analyze`] computed.
pub struct Outcome {
    /// The name the photo's clicks are asked under.
    pub key: String,
    /// The photo's people and animals, all zero where there are none.
    pub subject: Coverage,
}

/// What [`Engine::describe`] says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    /// The selection model, as its id, `@`, and the first 16 hex digits of its
    /// weights' SHA-256.
    pub model: String,
    /// The panoptic model's id, which finds people and animals.
    pub panoptic: String,
    /// The sky model, empty when the worker has none.
    pub sky: String,
    /// The execution provider the models run on, as the runtime names it.
    pub provider: String,
}

/// The real engine: one loaded session and the analyses of the photos asked
/// about lately.
pub struct Model {
    session: Session,
    /// The sky model, when the worker was given one.
    sky: Option<SkySession>,
    cache: HashMap<String, Entry>,
    order: VecDeque<String>,
    bytes: usize,
}

struct Entry {
    analysis: Analysis,
    subject: Coverage,
}

impl Model {
    /// Loads the models from `dir` the way the app would, onto `options`'
    /// provider: a requested accelerator that the runtime cannot register ends
    /// here as the error it is.
    pub fn load(dir: &std::path::Path, options: &LoadOptions) -> Result<Self, InferenceError> {
        Ok(Self {
            session: Subject::load_with(dir, options)?,
            sky: None,
            cache: HashMap::new(),
            order: VecDeque::new(),
            bytes: 0,
        })
    }

    /// Loads the sky model from its folder too; without it, Sky requests are refused.
    pub fn with_sky(
        mut self,
        dir: &std::path::Path,
        options: &LoadOptions,
    ) -> Result<Self, InferenceError> {
        self.sky = Some(Sky::load_with(dir, options)?);
        Ok(self)
    }

    /// The provider the loaded sessions run on.
    pub fn provider(&self) -> rawmakase_inference::Provider {
        self.session.provider()
    }
}

impl Engine for Model {
    fn analyze(&mut self, photo: &RgbImage, cancel: Cancel<'_>) -> Result<Outcome, EngineError> {
        let key = protocol::analysis_key(photo);
        if let Some(entry) = self.cache.get(&key) {
            return Ok(Outcome {
                key,
                subject: entry.subject.clone(),
            });
        }
        let analysis = self.session.analyze(photo, cancel).map_err(engine_error)?;
        let subject = analysis.subject().map_err(engine_error)?;
        let held = analysis.bytes() + subject.data.len();
        self.remember(
            key.clone(),
            Entry {
                analysis,
                subject: subject.clone(),
            },
            held,
        );
        Ok(Outcome { key, subject })
    }

    fn segment(
        &mut self,
        key: &str,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, EngineError> {
        let entry = match self.cache.get(key) {
            Some(entry) => entry,
            None => {
                return Err(EngineError::UnknownKey);
            }
        };
        self.session
            .segment(entry.analysis.embedding(), prompt, cancel)
            .map_err(engine_error)
    }

    fn sky(&mut self, photo: &RgbImage, cancel: Cancel<'_>) -> Result<Coverage, EngineError> {
        let Some(sky) = &mut self.sky else {
            return Err(EngineError::Failed(InferenceError::ModelInvalid(
                "this worker has no sky model; start it with --sky-model".into(),
            )));
        };
        sky.select(photo, cancel).map_err(engine_error)
    }

    fn describe(&self) -> Info {
        let spec = self.session.spec();
        Info {
            model: format!("{}@{}", spec.id, &spec.files[1].sha256[..16]),
            panoptic: rawmakase_inference::PANOPTIC.id.to_string(),
            sky: self
                .sky
                .as_ref()
                .map(|_| format!("{}@{}", SKY.id, &SKY.file.sha256[..16]))
                .unwrap_or_default(),
            provider: self.session.provider().name().to_string(),
        }
    }
}

/// The inference errors, in the protocol's terms: a cancel is a cancel, and
/// everything else is what went wrong.
fn engine_error(error: InferenceError) -> EngineError {
    match error {
        InferenceError::Cancelled => EngineError::Cancelled,
        other => EngineError::Failed(other),
    }
}

impl Model {
    /// Keeps one more analysis, letting the oldest go when the bounds say so.
    /// The newest analysis is never evicted to make room for itself.
    fn remember(&mut self, key: String, entry: Entry, held: usize) {
        while self.order.len() + 1 > ENTRIES
            || self.bytes + held > MAX_CACHED_BYTES && !self.order.is_empty()
        {
            let Some(oldest) = self.order.pop_front() else {
                break;
            };
            if oldest == key {
                self.order.push_front(oldest);
                break;
            }
            if let Some(gone) = self.cache.remove(&oldest) {
                self.bytes -= gone.analysis.bytes() + gone.subject.data.len();
            }
        }
        self.bytes += held;
        self.order.push_back(key.clone());
        self.cache.insert(key, entry);
    }
}

/// The connection loop. Holds the engine behind a lock, so one model runs at a
/// time and a second client waits its turn.
pub struct Server<E: Engine> {
    engine: std::sync::Mutex<E>,
    /// The cancel flag of the request running under each id, so a `cancel`
    /// frame from the client's watcher reaches the model run.
    cancels: std::sync::Mutex<Cancels>,
}

/// How many cancels for requests not yet read the worker remembers.
const EARLY_CANCELS: usize = 64;

#[derive(Default)]
struct Cancels {
    running: HashMap<String, Arc<AtomicBool>>,
    /// Cancels that arrived before their request: the client's watcher uses a
    /// connection of its own and can overtake the request it cancels. The
    /// newest few are kept, so that request starts out cancelled.
    early: VecDeque<String>,
}

impl<E: Engine> Server<E> {
    /// A worker answering with `engine`, with nothing running or cancelled.
    pub fn new(engine: E) -> Self {
        Self {
            engine: std::sync::Mutex::new(engine),
            cancels: std::sync::Mutex::new(Cancels::default()),
        }
    }
}

impl<E: Engine + 'static> Server<E> {
    /// Serves forever, one thread per connection.
    pub fn serve(self: Arc<Self>, listener: TcpListener) -> ! {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let server = self.clone();
            std::thread::spawn(move || {
                let _ = server.handle(stream);
            });
        }
        unreachable!("a listener that stops accepting has nowhere to serve")
    }

    /// One connection: requests until the client goes away or breaks the
    /// protocol. The first violation ends it; everything before it stands.
    pub fn handle(&self, mut stream: TcpStream) -> std::io::Result<()> {
        stream.set_nodelay(true).ok();
        stream.set_read_timeout(Some(IDLE_TIMEOUT)).ok();
        stream.set_write_timeout(Some(IDLE_TIMEOUT)).ok();
        loop {
            let Some(frame) = protocol::read_frame(&mut stream, MAX_FRAME_BYTES)? else {
                return Ok(());
            };
            let request: Request = match serde_json::from_slice(&frame) {
                Ok(request) => request,
                Err(e) => {
                    self.refuse(&mut stream, "", Code::Invalid, format!("unreadable: {e}"))?;
                    return Ok(());
                }
            };
            if request.v != protocol::VERSION {
                self.refuse(
                    &mut stream,
                    &request.id,
                    Code::Unsupported,
                    format!(
                        "this worker speaks protocol {}, the request asks for {}",
                        protocol::VERSION,
                        request.v
                    ),
                )?;
                return Ok(());
            }
            if request.id.len() > MAX_LABEL_BYTES {
                self.refuse(&mut stream, "", Code::Invalid, "the id is too long".into())?;
                return Ok(());
            }
            let request_id = request.id.clone();
            let running = Arc::new(AtomicBool::new(false));
            match request.op {
                Op::Ping => {
                    let info = self.engine.lock().expect("engine lock").describe();
                    self.answer(
                        &mut stream,
                        &request_id,
                        Reply::Ping {
                            model: info.model,
                            panoptic: info.panoptic,
                            sky: info.sky,
                            provider: info.provider,
                        },
                    )?;
                }
                Op::Analyze {
                    width,
                    height,
                    pixels,
                } => {
                    let photo = match photo(width, height, pixels) {
                        Ok(photo) => photo,
                        Err(why) => {
                            self.refuse(&mut stream, &request_id, Code::Invalid, why)?;
                            return Ok(());
                        }
                    };
                    if let Err(why) = self.register(&request_id, running.clone()) {
                        self.refuse(&mut stream, &request_id, Code::Invalid, why)?;
                        return Ok(());
                    }
                    let outcome = self
                        .engine
                        .lock()
                        .expect("engine lock")
                        .analyze(&photo, Cancel::new(&running));
                    self.cancels
                        .lock()
                        .expect("cancel registry")
                        .running
                        .remove(&request_id);
                    match outcome {
                        Ok(outcome) => self.answer(
                            &mut stream,
                            &request_id,
                            Reply::Analyze {
                                key: outcome.key,
                                subject: WireCoverage::of(&outcome.subject),
                            },
                        )?,
                        Err(e) => {
                            let stop = matches!(e, EngineError::Cancelled);
                            self.engine_error(&mut stream, &request_id, &e)?;
                            if stop {
                                return Ok(());
                            }
                        }
                    }
                }
                Op::Sky {
                    width,
                    height,
                    pixels,
                } => {
                    let photo = match photo(width, height, pixels) {
                        Ok(photo) => photo,
                        Err(why) => {
                            self.refuse(&mut stream, &request_id, Code::Invalid, why)?;
                            return Ok(());
                        }
                    };
                    if let Err(why) = self.register(&request_id, running.clone()) {
                        self.refuse(&mut stream, &request_id, Code::Invalid, why)?;
                        return Ok(());
                    }
                    let outcome = self
                        .engine
                        .lock()
                        .expect("engine lock")
                        .sky(&photo, Cancel::new(&running));
                    self.cancels
                        .lock()
                        .expect("cancel registry")
                        .running
                        .remove(&request_id);
                    match outcome {
                        Ok(coverage) => self.answer(
                            &mut stream,
                            &request_id,
                            Reply::Sky {
                                coverage: WireCoverage::of(&coverage),
                            },
                        )?,
                        Err(e) => {
                            let stop = matches!(e, EngineError::Cancelled);
                            self.engine_error(&mut stream, &request_id, &e)?;
                            if stop {
                                return Ok(());
                            }
                        }
                    }
                }
                Op::Segment { key, prompt } => {
                    if key.len() > MAX_LABEL_BYTES {
                        self.refuse(
                            &mut stream,
                            &request_id,
                            Code::Invalid,
                            "the key is too long".into(),
                        )?;
                        return Ok(());
                    }
                    let prompt = match prompt.to_prompt() {
                        Ok(prompt) => prompt,
                        Err(why) => {
                            self.refuse(&mut stream, &request_id, Code::Invalid, why)?;
                            return Ok(());
                        }
                    };
                    if let Err(why) = self.register(&request_id, running.clone()) {
                        self.refuse(&mut stream, &request_id, Code::Invalid, why)?;
                        return Ok(());
                    }
                    let coverage = self.engine.lock().expect("engine lock").segment(
                        &key,
                        &prompt,
                        Cancel::new(&running),
                    );
                    self.cancels
                        .lock()
                        .expect("cancel registry")
                        .running
                        .remove(&request_id);
                    match coverage {
                        Ok(coverage) => self.answer(
                            &mut stream,
                            &request_id,
                            Reply::Segment {
                                coverage: WireCoverage::of(&coverage),
                            },
                        )?,
                        Err(e) => {
                            let stop = matches!(e, EngineError::Cancelled);
                            self.engine_error(&mut stream, &request_id, &e)?;
                            if stop {
                                return Ok(());
                            }
                        }
                    }
                }
                Op::Cancel { target } => {
                    // A cancel for a request that is gone is normal: the answer
                    // may already be on its way, and nothing is owed. One for a
                    // request not read yet is kept for when it arrives.
                    if target.len() <= MAX_LABEL_BYTES {
                        let mut cancels = self.cancels.lock().expect("cancel registry");
                        if let Some(flag) = cancels.running.get(&target) {
                            flag.store(true, Ordering::Relaxed);
                        } else {
                            if cancels.early.len() == EARLY_CANCELS {
                                cancels.early.pop_front();
                            }
                            cancels.early.push_back(target);
                        }
                    }
                    self.answer(&mut stream, &request_id, Reply::Ack)?;
                }
            }
        }
    }

    /// Puts a running request's cancel flag in the registry, refusing an id
    /// that is already live: whatever else a duplicate would break, one
    /// request's completion must not be able to unregister another's. A
    /// request whose cancel came first starts out cancelled.
    fn register(&self, id: &str, flag: Arc<AtomicBool>) -> Result<(), String> {
        let mut cancels = self.cancels.lock().expect("cancel registry");
        if cancels.running.contains_key(id) {
            return Err(format!("a request with the id `{id}` is already running"));
        }
        if let Some(at) = cancels.early.iter().position(|early| early == id) {
            cancels.early.remove(at);
            flag.store(true, Ordering::Relaxed);
        }
        cancels.running.insert(id.to_string(), flag);
        Ok(())
    }

    fn answer(&self, stream: &mut TcpStream, id: &str, reply: Reply) -> std::io::Result<()> {
        let payload = serde_json::to_vec(&Response::success(id, reply))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        protocol::write_frame(stream, &payload)
    }

    fn refuse(
        &self,
        stream: &mut TcpStream,
        id: &str,
        code: Code,
        message: String,
    ) -> std::io::Result<()> {
        let payload = serde_json::to_vec(&Response::failure(id, code, message))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        protocol::write_frame(stream, &payload)
    }

    /// The engine's own errors, mapped to what the client can decide on.
    fn engine_error(
        &self,
        stream: &mut TcpStream,
        id: &str,
        error: &EngineError,
    ) -> std::io::Result<()> {
        let (code, message) = match error {
            EngineError::Cancelled => (Code::Cancelled, "cancelled".to_string()),
            EngineError::UnknownKey => (
                Code::UnknownKey,
                "no analysis under this key; ask for the photo again".to_string(),
            ),
            // The input was malformed; the model never should have been asked.
            EngineError::Failed(InferenceError::OutputInvalid(why)) => (Code::Invalid, why.clone()),
            EngineError::Failed(other) => (Code::Failed, other.to_string()),
        };
        self.refuse(stream, id, code, message)
    }
}

/// The photo a request carried, with its dimensions checked against its bytes
/// and the protocol's bounds.
fn photo(width: u32, height: u32, pixels: Vec<u8>) -> Result<RgbImage, String> {
    if width == 0 || height == 0 {
        return Err("the photo has no pixels".into());
    }
    if pixels.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "the photo is {} bytes; the protocol allows {}",
            pixels.len(),
            MAX_IMAGE_BYTES
        ));
    }
    let expected = usize::checked_mul(width as usize * height as usize, 3);
    if expected != Some(pixels.len()) {
        return Err(format!(
            "the photo claims {width} x {height} but holds {} bytes",
            pixels.len()
        ));
    }
    if width as usize > rawmakase_inference::process::MAX_INPUT_SIDE
        || height as usize > rawmakase_inference::process::MAX_INPUT_SIDE
    {
        return Err(format!(
            "the photo's sides must be at most {}",
            rawmakase_inference::process::MAX_INPUT_SIDE
        ));
    }
    Ok(RgbImage {
        width: width as usize,
        height: height as usize,
        data: pixels,
    })
}
