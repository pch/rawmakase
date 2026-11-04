//! The app's side of the protocol: a drop-in for the local selection session
//! that asks a remote worker instead. One request, one connection, every wait
//! bounded; the caller's cancel flag is watched while a request is out and
//! forwarded to the worker, which stops the model run it can. Errors are
//! values the app can show, never panics; a worker that is gone, slow or
//! speaking another protocol is a [`ClientError`], and the local session keeps
//! working.

use rawmakase_cancel::Cancel;
use rawmakase_inference::{Coverage, Prompt, RgbImage};
use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use crate::protocol::{
    self, Code, MAX_FRAME_BYTES, MAX_IMAGE_BYTES, Op, Reply, Request, Response, WireCoverage,
};

/// How long a connect may take, and how long the worker may take to answer.
/// A model run takes seconds; the bounds exist so a dead or stopped worker is
/// an error the app can show, not a drawer that waits forever.
#[derive(Debug, Clone)]
pub struct RemoteOptions {
    /// How long connecting to each address the endpoint resolves to may take (it
    /// tries the first `CONNECT_ATTEMPTS` in turn): 5 s by default.
    pub connect_timeout: Duration,
    /// How long the worker may take to answer one request: 180 s by default.
    pub request_timeout: Duration,
}

impl Default for RemoteOptions {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(5),
            request_timeout: Duration::from_secs(180),
        }
    }
}

/// What one photo's analysis is on the worker: the `key` its clicks are asked
/// under, and the automatic selections it already computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteAnalysis {
    /// What the worker calls this photo; `segment` asks under this name.
    pub key: String,
    /// The photo's people and animals, all zero where there are none.
    pub subject: Coverage,
}

/// What a `ping` learned about a worker: the models it loaded and the
/// provider they are running on, as the runtime registered them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInfo {
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

/// Why a request to the worker produced nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The caller's cancel flag was raised in time.
    Cancelled,
    /// The worker could not be reached or hung up.
    Unavailable(String),
    /// The worker no longer holds this photo's analysis; analyse again.
    Expired,
    /// The worker tried and failed, or answered something unusable.
    Failed(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Cancelled => f.write_str("the selection was cancelled"),
            Self::Unavailable(why) => write!(f, "the remote worker is unreachable: {why}"),
            Self::Expired => f.write_str("the remote worker forgot this photo"),
            Self::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// A connection to one worker. Cheap to make, stateless between requests: the
/// worker holds the analyses, this only remembers where it is.
#[derive(Debug)]
pub struct RemoteSession {
    endpoint: String,
    options: RemoteOptions,
    /// Makes this session's request ids everyone else's problem: never.
    nonce: u64,
    ids: AtomicU64,
}

impl RemoteSession {
    /// Talks to the worker at `endpoint` (`host:port`), with the default
    /// bounds.
    pub fn new(endpoint: impl Into<String>) -> Self {
        Self::with_options(endpoint, RemoteOptions::default())
    }

    /// Talks to the worker at `endpoint` (`host:port`), within `options`' bounds.
    pub fn with_options(endpoint: impl Into<String>, options: RemoteOptions) -> Self {
        Self {
            endpoint: endpoint.into(),
            options,
            nonce: {
                use std::hash::{BuildHasher, Hasher};
                std::collections::hash_map::RandomState::new()
                    .build_hasher()
                    .finish()
            },
            ids: AtomicU64::new(1),
        }
    }

    /// Where this session sends its requests.
    pub fn endpoint(&self) -> &str {
        &self.endpoint
    }

    /// Asks the worker what it is. Also the cheapest way to find out it cannot
    /// be reached, before asking it to run a model.
    pub fn ping(&self) -> Result<ServerInfo, ClientError> {
        match self.call(Op::Ping, Cancel::NEVER)? {
            Reply::Ping {
                model,
                panoptic,
                sky,
                provider,
            } => Ok(ServerInfo {
                model,
                panoptic,
                sky,
                provider,
            }),
            _ => Err(self.mismatched()),
        }
    }

    /// Runs the models on `photo`: the worker keeps the embedding and answers
    /// [`RemoteSession::segment`] for it. The automatic selections come back
    /// computed. A photo too large for the protocol is refused before a byte
    /// is sent.
    pub fn analyze(
        &self,
        photo: &RgbImage,
        cancel: Cancel<'_>,
    ) -> Result<RemoteAnalysis, ClientError> {
        let (width, height, pixels) = self.wire_photo(photo)?;
        match self.call(
            Op::Analyze {
                width,
                height,
                pixels,
            },
            cancel,
        )? {
            Reply::Analyze { key, subject } => Ok(RemoteAnalysis {
                key,
                subject: self.coverage(subject)?,
            }),
            _ => Err(self.mismatched()),
        }
    }

    /// The photo's sky, all zero where there is none. Nothing is kept on the worker.
    pub fn sky(&self, photo: &RgbImage, cancel: Cancel<'_>) -> Result<Coverage, ClientError> {
        let (width, height, pixels) = self.wire_photo(photo)?;
        match self.call(
            Op::Sky {
                width,
                height,
                pixels,
            },
            cancel,
        )? {
            Reply::Sky { coverage } => self.coverage(coverage),
            _ => Err(self.mismatched()),
        }
    }

    /// `photo` as a request carries it, or why it cannot.
    fn wire_photo(&self, photo: &RgbImage) -> Result<(u32, u32, Vec<u8>), ClientError> {
        let (width, height) = (u32::try_from(photo.width), u32::try_from(photo.height));
        let (width, height) = match (width, height) {
            (Ok(w), Ok(h)) => (w, h),
            _ => return Err(self.too_large()),
        };
        if photo.data.len() > MAX_IMAGE_BYTES {
            return Err(self.too_large());
        }
        Ok((width, height, photo.data.clone()))
    }

    /// The mask for `prompt` on a photo analysed before.
    pub fn segment(
        &self,
        analysis: &RemoteAnalysis,
        prompt: &Prompt,
        cancel: Cancel<'_>,
    ) -> Result<Coverage, ClientError> {
        match self.call(
            Op::Segment {
                key: analysis.key.clone(),
                prompt: protocol::WirePrompt::of(prompt),
            },
            cancel,
        )? {
            Reply::Segment { coverage } => self.coverage(coverage),
            _ => Err(self.mismatched()),
        }
    }

    /// Sends one request and waits, bounded, for its answer, while a watcher
    /// forwards the cancel flag to the worker on a connection of its own.
    fn call(&self, op: Op, cancel: Cancel<'_>) -> Result<Reply, ClientError> {
        let id = format!(
            "{:016x}{:x}",
            self.nonce,
            self.ids.fetch_add(1, Ordering::Relaxed)
        );
        let request = Request {
            v: protocol::VERSION,
            id: id.clone(),
            op,
        };
        let payload = serde_json::to_vec(&request)
            .map_err(|e| ClientError::Failed(format!("the request could not be sent: {e}")))?;
        let done = Arc::new(AtomicBool::new(false));
        let result = std::thread::scope(|scope| {
            let watcher = {
                let endpoint = self.endpoint.clone();
                let id = id.clone();
                let done = done.clone();
                let connect = self.options.connect_timeout;
                scope.spawn(move || watch_for_cancel(endpoint, id, cancel, done, connect))
            };
            let result = self.ask(&payload, &id, cancel);
            done.store(true, Ordering::Release);
            let _ = watcher.join();
            result
        });
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        result
    }

    fn ask(&self, payload: &[u8], id: &str, cancel: Cancel<'_>) -> Result<Reply, ClientError> {
        // The whole request gets one wall-clock budget, not a fresh timeout
        // per read: a worker that drips bytes never buys itself more time,
        // and a raise of the cancel flag is noticed within READ_POLL, even
        // if the worker never answers at all.
        let deadline = std::time::Instant::now() + self.options.request_timeout;
        let mut stream = self.connect()?;
        let mut frame = Vec::with_capacity(4 + payload.len());
        let len = u32::try_from(payload.len())
            .map_err(|_| ClientError::Failed("the request is too large to send".into()))?;
        frame.extend(len.to_le_bytes());
        frame.extend_from_slice(payload);
        write_bounded(&mut stream, &frame, deadline, cancel)?;
        let Some(frame) = read_bounded(&mut stream, deadline, cancel)? else {
            return Err(ClientError::Failed(
                "the remote worker closed the connection without answering".into(),
            ));
        };
        let response: Response = serde_json::from_slice(&frame).map_err(|e| {
            ClientError::Failed(format!("the remote worker answered nonsense: {e}"))
        })?;
        if response.v != protocol::VERSION {
            return Err(ClientError::Failed(format!(
                "the remote worker speaks protocol {}, this build speaks {}",
                response.v,
                protocol::VERSION
            )));
        }
        if response.id != id {
            return Err(ClientError::Failed(
                "the remote worker answered another request".into(),
            ));
        }
        match response.result() {
            Ok(reply) => Ok(reply),
            Err(error) => Err(match error.code {
                Code::Cancelled => {
                    if cancel.is_cancelled() {
                        ClientError::Cancelled
                    } else {
                        ClientError::Failed(format!(
                            "the remote worker cancelled the request: {}",
                            error.message
                        ))
                    }
                }
                Code::UnknownKey => ClientError::Expired,
                Code::Unsupported | Code::Invalid | Code::Failed => {
                    ClientError::Failed(error.message)
                }
            }),
        }
    }

    fn connect(&self) -> Result<TcpStream, ClientError> {
        let mut addresses = self
            .endpoint
            .to_socket_addrs()
            .map_err(|e| self.unreachable(e))?
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(ClientError::Unavailable(format!(
                "{} is not an address",
                self.endpoint
            )));
        }
        addresses.truncate(CONNECT_ATTEMPTS);
        let mut last = String::from("no address worked");
        for address in addresses.drain(..) {
            match TcpStream::connect_timeout(&address, self.options.connect_timeout) {
                Ok(stream) => {
                    let _ = stream.set_nodelay(true);
                    return Ok(stream);
                }
                Err(e) => last = e.to_string(),
            }
        }
        Err(ClientError::Unavailable(format!(
            "{}: {last}",
            self.endpoint
        )))
    }

    fn coverage(&self, wire: WireCoverage) -> Result<Coverage, ClientError> {
        wire.to_coverage().map_err(ClientError::Failed)
    }

    fn unreachable(&self, error: impl std::fmt::Display) -> ClientError {
        ClientError::Unavailable(format!("{}: {error}", self.endpoint))
    }

    fn too_large(&self) -> ClientError {
        ClientError::Failed(
            "the photo is too large for remote selection; select it on this computer".into(),
        )
    }

    fn mismatched(&self) -> ClientError {
        ClientError::Failed("the remote worker answered with the wrong reply".into())
    }
}

/// How often a waiting read comes up for air: to notice a raise of the cancel
/// flag promptly even when the worker never says anything, each socket read is
/// capped at this, and the loop between reads checks the flag and the
/// request's one wall-clock budget.
const READ_POLL: Duration = Duration::from_millis(250);

/// A connection attempt budget per resolved address, so a hostname naming
/// several unreachable ones cannot spend the whole request just connecting.
const CONNECT_ATTEMPTS: usize = 4;

/// Writes `payload` before `deadline`, checking `cancel` between poll-sized
/// attempts, so a blocked send never outlives the budget and never swallows a
/// cancel.
fn write_bounded(
    stream: &mut TcpStream,
    payload: &[u8],
    deadline: std::time::Instant,
    cancel: Cancel<'_>,
) -> Result<(), ClientError> {
    let mut written = 0;
    while written < payload.len() {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
            return Err(timed_out());
        };
        stream
            .set_write_timeout(Some(remaining.min(READ_POLL)))
            .map_err(|e| ClientError::Unavailable(e.to_string()))?;
        match stream.write(&payload[written..]) {
            Ok(0) => return Err(ClientError::Unavailable("the remote worker hung up".into())),
            Ok(n) => written += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e)
                if matches!(
                    e.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => {
                return Err(ClientError::Unavailable(
                    "the request could not be sent".into(),
                ));
            }
        }
    }
    Ok(())
}

/// Reads one frame, within `deadline` as a whole and `READ_POLL` at a stretch,
/// so a raise of `cancel` is answered even when the worker says nothing.
fn read_bounded(
    stream: &mut TcpStream,
    deadline: std::time::Instant,
    cancel: Cancel<'_>,
) -> Result<Option<Vec<u8>>, ClientError> {
    // The length prefix, one byte at a time if that is what the network
    // hands over; the first read also tells a clean end from a hang-up.
    let mut prefix = [0u8; 4];
    let mut seen = 0;
    while seen < prefix.len() {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        let Some(n) = read_within(stream, &mut prefix[seen..], deadline)? else {
            if seen == 0 {
                return Ok(None);
            }
            return Err(ClientError::Failed(
                "the remote worker closed the connection without answering".into(),
            ));
        };
        seen += n;
    }
    let len = u32::from_le_bytes(prefix) as usize;
    if len == 0 || len > MAX_FRAME_BYTES {
        return Err(ClientError::Failed(format!(
            "the remote worker sent a frame of {len} bytes"
        )));
    }
    let mut frame = vec![0u8; len];
    let mut filled = 0;
    while filled < frame.len() {
        if cancel.is_cancelled() {
            return Err(ClientError::Cancelled);
        }
        match read_within(stream, &mut frame[filled..], deadline)? {
            // The worker went away mid-frame.
            None => {
                return Err(ClientError::Failed(
                    "the remote worker closed the connection without answering".into(),
                ));
            }
            Some(n) => filled += n,
        }
    }
    Ok(Some(frame))
}

/// One [`one_read`] with the socket's read timeout capped at the shorter of
/// the remaining budget and [`READ_POLL`]. The budget having run out is a
/// timeout error, never a clean end. A socket that no longer takes a timeout
/// has been reset by the worker (macOS answers `EINVAL`), so that is a
/// hang-up like the read itself would report.
fn read_within(
    stream: &mut TcpStream,
    buffer: &mut [u8],
    deadline: std::time::Instant,
) -> Result<Option<usize>, ClientError> {
    let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
        return Err(timed_out());
    };
    if stream
        .set_read_timeout(Some(remaining.min(READ_POLL)))
        .is_err()
    {
        return Ok(None);
    }
    one_read(stream, buffer)
}

/// One socket read, mapped to the client's errors. `Ok(None)` is a hang-up or
/// a clean end (a zero-byte read is an end, not a wait); `Ok(Some(0))` is the
/// poll window or a signal passing through, and the caller goes round.
fn one_read(stream: &mut TcpStream, buffer: &mut [u8]) -> Result<Option<usize>, ClientError> {
    match stream.read(buffer) {
        Ok(0) => Ok(None),
        Ok(n) => Ok(Some(n)),
        Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(Some(0)),
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
            ) =>
        {
            Ok(Some(0))
        }
        // A peer that hangs up with our request still unread answers with a
        // reset; that is a hang-up, not a mystery.
        Err(e)
            if matches!(
                e.kind(),
                io::ErrorKind::ConnectionReset | io::ErrorKind::ConnectionAborted
            ) =>
        {
            Ok(None)
        }
        Err(_) => Err(ClientError::Unavailable("the remote worker hung up".into())),
    }
}

fn timed_out() -> ClientError {
    ClientError::Failed("the remote worker did not answer in time".into())
}

/// While a request is out, raises of the caller's flag are forwarded to the
/// worker on a connection of their own, so the model run stops instead of
/// finishing into a dropped socket. A raise the request loop noticed first is
/// forwarded all the same: the loop ending is no sign the worker heard. Best
/// effort and bounded: the send gets a short timeout of its own, so this
/// thread can never hold the request's join past a few seconds.
fn watch_for_cancel(
    endpoint: String,
    target: String,
    cancel: Cancel<'_>,
    done: Arc<AtomicBool>,
    connect_timeout: Duration,
) {
    while !done.load(Ordering::Acquire) && !cancel.is_cancelled() {
        std::thread::sleep(Duration::from_millis(15));
    }
    if !cancel.is_cancelled() {
        return;
    }
    let request = Request {
        v: protocol::VERSION,
        id: format!("cancel-{target}"),
        op: Op::Cancel { target },
    };
    if let Ok(payload) = serde_json::to_vec(&request)
        && let Ok(mut addresses) = endpoint.to_socket_addrs()
        && let Some(address) = addresses.next()
        && let Ok(mut stream) = TcpStream::connect_timeout(&address, connect_timeout)
    {
        let _ = stream.set_nodelay(true);
        let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
        let _ = protocol::write_frame(&mut stream, &payload);
    }
}
