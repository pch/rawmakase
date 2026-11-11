//! One run of an extension's program: the process unit, the message channel,
//! the app's requests with their deadlines, and ending it.
//!
//! A session has three threads, none of which its owner ever waits on:
//!
//! - the **reader** reads the program's lines. Replies, events and the
//!   program's operation requests go into a bounded queue the owner drains
//!   with [`Session::poll`]; when the queue is full an operation is refused at
//!   once with `channel_full`, so the reader never blocks and keeps reading.
//!   Lifecycle requests ([`SessionOptions::lifecycle`], `adapter.device`) are
//!   applied by the reader itself, outside that queue, so a device's
//!   disconnection is never held up behind its own burst of input.
//! - the **writer** writes what the owner sends, from a bounded queue: when
//!   the program stops reading and the owner's share of the queue fills,
//!   sending fails with `channel_full` instead of blocking the owner.
//! - the **waiter** waits for the program to exit and ends its whole unit
//!   (process group or job object) the moment it does.
//!
//! The owner (the app's UI thread) calls [`Session::poll`] regularly: it hands
//! over what arrived and enforces deadlines. A request past its deadline is
//! cancelled, no longer admitted ([`Session::is_admitted`]), and fails with
//! `deadline` when the program answers it or after the grace period,
//! whichever is first. [`Session::cancel`] settles a request at once: a reply
//! arriving afterwards is dropped, so a request has exactly one outcome.
//!
//! [`Session::stop`] sends `quitting`, closes the channel's sending side and
//! returns a [`Stopping`] whose thread waits for the program to exit, until
//! shortly before the shared absolute deadline, then ends the unit.
//! [`Killer::kill_remaining`] ends a unit not yet ended without blocking, for
//! the app's exit hook. Dropping a session ends its unit.
use crate::process::{self, Unit};
use rawmakase_extension_protocol::framing::{LineError, LineReader};
use rawmakase_extension_protocol::method::HelloReply;
use rawmakase_extension_protocol::{ErrorCode, Message, PROTOCOL, RemoteError};
use serde_json::Value;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// The default deadline of an app request.
pub const REQUEST_DEADLINE: Duration = Duration::from_secs(30);
/// The deadline of `hello`, and of the program opening its channel.
pub const HELLO_DEADLINE: Duration = Duration::from_secs(5);
/// How long a request past its deadline waits for the program to answer its
/// cancellation before it fails.
pub const CANCEL_GRACE: Duration = Duration::from_secs(2);
/// The longest [`Session::stop`] waits for the program to exit by itself.
pub const QUIT_GRACE: Duration = Duration::from_secs(1);
/// What [`Session::stop`] leaves of the shared deadline for the steps after it.
const QUIT_MARGIN: Duration = Duration::from_millis(200);
/// The default size of each queue, in messages.
pub const QUEUE: usize = 64;

/// What to start.
#[derive(Clone, Debug)]
pub struct Launch {
    /// The program's file.
    pub program: PathBuf,
    /// Its arguments.
    pub args: Vec<OsString>,
    /// Its working folder, usually its package's.
    pub dir: PathBuf,
    /// Variables added to the app's environment.
    pub env: Vec<(OsString, OsString)>,
    /// The log its stdout and stderr go to.
    pub log: PathBuf,
}

/// A lifecycle request the reader thread applies itself: given the method and
/// parameters, it returns the reply, or `None` for requests that go to the
/// owner's queue like any other.
pub type Lifecycle = Arc<dyn Fn(&str, &Value) -> Option<Result<Value, RemoteError>> + Send + Sync>;
/// Called from the session's threads when something arrived for
/// [`Session::poll`] (the app asks for a repaint).
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// How a session runs.
#[derive(Clone)]
pub struct SessionOptions {
    /// The capacity of the operation queue and of the sending queue.
    pub queue: usize,
    /// Requests the reader applies itself.
    pub lifecycle: Option<Lifecycle>,
    /// Called when something arrives.
    pub wake: Option<Wake>,
    /// See [`CANCEL_GRACE`].
    pub cancel_grace: Duration,
    /// How long the program has to open its channel (Windows).
    pub connect: Duration,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            queue: QUEUE,
            lifecycle: None,
            wake: None,
            cancel_grace: CANCEL_GRACE,
            connect: HELLO_DEADLINE,
        }
    }
}

/// How the primary process exited.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    /// Its exit code, when it exited.
    pub code: Option<i32>,
    /// The signal that ended it, on Unix.
    pub signal: Option<i32>,
}
impl std::fmt::Display for Exit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.code, self.signal) {
            (Some(code), _) => write!(f, "exit code {code}"),
            (None, Some(signal)) => write!(f, "signal {signal}"),
            (None, None) => f.write_str("an unknown cause"),
        }
    }
}

/// What [`Session::poll`] hands over.
#[derive(Clone, Debug, PartialEq)]
pub enum Update {
    /// The one outcome of an app request: the program's reply, or a failure
    /// (`deadline`, `exited`).
    Reply {
        /// The request.
        id: u64,
        /// Its method.
        method: String,
        /// Its outcome.
        result: Result<Value, RemoteError>,
    },
    /// How far an app request is.
    Progress {
        /// The request.
        id: u64,
        /// The fraction done.
        done: Option<f64>,
        /// What it is doing.
        text: Option<String>,
    },
    /// A request from the program, to answer with [`Session::reply`].
    Request {
        /// The program's id for it.
        id: u64,
        /// Its method.
        method: String,
        /// Its parameters.
        params: Value,
    },
    /// The program cancelled one of its requests.
    Cancel {
        /// The program's id for it.
        id: u64,
    },
    /// An event from the program.
    Event {
        /// Its name.
        name: String,
        /// Its parameters.
        params: Value,
    },
    /// A line that wasn't a message; the session goes on.
    Malformed {
        /// What was wrong.
        reason: String,
    },
    /// The channel closed: the program exited, or closed its end. Comes once,
    /// after every reply that arrived and the failure of every request left.
    Closed,
}

/// What the reader hands the owner.
enum Incoming {
    Message(Message),
    Malformed(String),
}

/// An app request in flight.
struct Pending {
    method: String,
    deadline: Instant,
    /// Past its deadline: cancelled, no longer admitted, failing at
    /// `deadline` (now the end of the grace period) or at the program's reply.
    expired: bool,
}

/// Room in the sending queue beyond the owner's share, for the reader's own
/// answers to the program.
const ANSWER_ROOM: usize = 1024;

/// One line for the writer.
struct Line {
    bytes: Vec<u8>,
    /// Sent by the owner, so it counts against the owner's share.
    owner: bool,
}

/// The sending queue, which [`Session::stop`] closes. The owner may have at
/// most `limit` lines in it; the reader's answers (refusals and lifecycle
/// acknowledgements) have [`ANSWER_ROOM`] more, so a burst of the program's
/// requests is answered even while the owner's share is full. Only a program
/// that stops reading fills it.
struct Outgoing {
    sender: Mutex<Option<SyncSender<Line>>>,
    owner_queued: AtomicUsize,
    limit: usize,
}
impl Outgoing {
    fn send(&self, bytes: Vec<u8>, owner: bool) -> Result<(), RemoteError> {
        let sender = self.sender.lock().unwrap_or_else(|e| e.into_inner());
        let Some(sender) = sender.as_ref() else {
            return Err(RemoteError::new(
                ErrorCode::Exited,
                "the session is stopping",
            ));
        };
        let full = || {
            RemoteError::new(
                ErrorCode::ChannelFull,
                "the extension's program isn't reading its messages",
            )
        };
        if owner && self.owner_queued.fetch_add(1, Ordering::SeqCst) >= self.limit {
            self.owner_queued.fetch_sub(1, Ordering::SeqCst);
            return Err(full());
        }
        sender.try_send(Line { bytes, owner }).map_err(|e| {
            if owner {
                self.owner_queued.fetch_sub(1, Ordering::SeqCst);
            }
            match e {
                TrySendError::Full(_) => full(),
                TrySendError::Disconnected(_) => {
                    RemoteError::new(ErrorCode::Exited, "the extension's program is gone")
                }
            }
        })
    }
    /// Closes the queue: the writer writes what is in it, then ends.
    fn close(&self) {
        self.sender.lock().unwrap_or_else(|e| e.into_inner()).take();
    }
}

/// A running program.
pub struct Session {
    outgoing: Arc<Outgoing>,
    incoming: Receiver<Incoming>,
    reader_done: Arc<AtomicBool>,
    unit: Arc<Unit>,
    pid: u32,
    pending: HashMap<u64, Pending>,
    next_id: u64,
    grace: Duration,
    closed: bool,
    /// [`Session::stop`] took over ending the unit.
    stopping: bool,
    /// Messages the reader dropped because the owner's queue was full.
    dropped: Arc<AtomicU64>,
}

impl Session {
    /// Starts the program with its channel; the first request should be
    /// `hello` ([`check_hello`] reads its reply).
    pub fn spawn(launch: &Launch, options: SessionOptions) -> std::io::Result<Self> {
        let spawned = process::spawn(launch, options.connect)?;
        let process::Transport {
            reader,
            writer,
            close_write,
        } = spawned.transport;
        let queue = options.queue.max(1);
        let (send, to_writer) = sync_channel::<Line>(queue + ANSWER_ROOM);
        let outgoing = Arc::new(Outgoing {
            sender: Mutex::new(Some(send)),
            owner_queued: AtomicUsize::new(0),
            limit: queue,
        });
        let (to_owner, incoming) = sync_channel(queue);
        let reader_done = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicU64::new(0));
        let started = (|| -> std::io::Result<()> {
            std::thread::Builder::new()
                .name("extension writer".into())
                .spawn({
                    let outgoing = outgoing.clone();
                    move || write(writer, &to_writer, &outgoing, close_write)
                })?;
            let reading = Reading {
                to_owner,
                outgoing: outgoing.clone(),
                lifecycle: options.lifecycle.clone(),
                wake: options.wake.clone(),
                done: reader_done.clone(),
                dropped: dropped.clone(),
            };
            std::thread::Builder::new()
                .name("extension reader".into())
                .spawn(move || reading.run(reader))?;
            Ok(())
        })();
        if let Err(e) = started {
            spawned.unit.kill();
            return Err(e);
        }
        Ok(Self {
            outgoing,
            incoming,
            reader_done,
            unit: spawned.unit,
            pid: spawned.pid,
            pending: HashMap::new(),
            next_id: 1,
            grace: options.cancel_grace,
            closed: false,
            stopping: false,
            dropped,
        })
    }

    /// The primary process's id.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Queues one line without blocking.
    fn send(&self, message: &Message) -> Result<(), RemoteError> {
        let line = message.encode().map_err(|_| {
            RemoteError::new(
                ErrorCode::TooLarge,
                "the message is longer than a protocol line",
            )
        })?;
        let mut line = line;
        line.push(b'\n');
        self.outgoing.send(line, true)
    }

    /// Sends a request that must be answered by `deadline`. Fails at once,
    /// without blocking, when the program isn't reading (`channel_full`) or
    /// is gone (`exited`).
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        deadline: Instant,
    ) -> Result<u64, RemoteError> {
        // After `Closed` nothing is answered any more, and poll fails nothing.
        if self.closed {
            return Err(RemoteError::new(
                ErrorCode::Exited,
                "the extension's program exited",
            ));
        }
        let id = self.next_id;
        self.send(&Message::request(id, method, params))?;
        self.next_id += 1;
        self.pending.insert(
            id,
            Pending {
                method: method.to_string(),
                deadline,
                expired: false,
            },
        );
        Ok(id)
    }

    /// Sends an event.
    pub fn event(&self, name: &str, params: Value) -> Result<(), RemoteError> {
        self.send(&Message::event(name, params))
    }

    /// Answers one of the program's requests.
    pub fn reply(&self, id: u64, result: Result<Value, RemoteError>) -> Result<(), RemoteError> {
        self.send(&Message::reply(id, result))
    }

    /// Cancels an app request and settles it as cancelled now: whatever the
    /// program answers later is dropped. Returns whether the request was still
    /// in flight; one already answered (handed over by [`Session::poll`]) or
    /// failed keeps that outcome.
    pub fn cancel(&mut self, id: u64) -> bool {
        if self.pending.remove(&id).is_none() {
            return false;
        }
        let _ = self.send(&Message::Cancel { id });
        true
    }

    /// Whether an app request is still in flight and within its deadline: a
    /// proposal belonging to it may be applied.
    pub fn is_admitted(&self, id: u64) -> bool {
        self.pending.get(&id).is_some_and(|p| !p.expired)
    }

    /// When an admitted app request stops being admitted, for work done on
    /// its behalf elsewhere to check at the moment it commits; `None` once
    /// it isn't admitted.
    pub fn deadline(&self, id: u64) -> Option<Instant> {
        self.pending
            .get(&id)
            .filter(|p| !p.expired)
            .map(|p| p.deadline)
    }

    /// The app requests in flight.
    pub fn in_flight(&self) -> usize {
        self.pending.len()
    }

    /// The earliest deadline among the requests in flight, for scheduling the
    /// next [`Session::poll`].
    pub fn next_deadline(&self) -> Option<Instant> {
        self.pending.values().map(|p| p.deadline).min()
    }

    /// How many of the program's messages were dropped because the owner's
    /// queue was full (replies and events; refused operations are answered).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Hands over what arrived and enforces deadlines as of `now`.
    pub fn poll(&mut self, now: Instant) -> Vec<Update> {
        let mut updates = Vec::new();
        if self.closed {
            return updates;
        }
        // Whether the reader had finished before this drain: then everything
        // it read is in the queue now.
        let finished = self.reader_done.load(Ordering::SeqCst);
        loop {
            match self.incoming.try_recv() {
                Ok(Incoming::Message(message)) => self.take(message, &mut updates),
                Ok(Incoming::Malformed(reason)) => updates.push(Update::Malformed { reason }),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let mut expired = Vec::new();
        for (id, pending) in &mut self.pending {
            if pending.deadline > now {
                continue;
            }
            if pending.expired {
                expired.push(*id);
            } else {
                pending.expired = true;
                pending.deadline = now + self.grace;
                let line = Message::Cancel { id: *id }.encode();
                if let Ok(mut line) = line {
                    line.push(b'\n');
                    let _ = self.outgoing.send(line, true);
                }
            }
        }
        expired.sort_unstable();
        for id in expired {
            let pending = self.pending.remove(&id).expect("listed above");
            updates.push(deadline_failure(id, pending.method));
        }
        if finished {
            let mut left: Vec<_> = self.pending.drain().collect();
            left.sort_unstable_by_key(|(id, _)| *id);
            for (id, pending) in left {
                updates.push(Update::Reply {
                    id,
                    method: pending.method,
                    result: Err(RemoteError::new(
                        ErrorCode::Exited,
                        "the extension's program exited before answering",
                    )),
                });
            }
            self.closed = true;
            updates.push(Update::Closed);
        }
        updates
    }

    fn take(&mut self, message: Message, updates: &mut Vec<Update>) {
        let mut settle = |id: u64, result: Result<Value, RemoteError>| {
            // A reply to a request already settled (cancelled, failed) is
            // dropped.
            let Some(pending) = self.pending.remove(&id) else {
                return;
            };
            updates.push(if pending.expired {
                deadline_failure(id, pending.method)
            } else {
                Update::Reply {
                    id,
                    method: pending.method,
                    result,
                }
            });
        };
        match message {
            Message::Ok { id, v } => settle(id, Ok(v)),
            Message::Err { id, code, message } => settle(id, Err(RemoteError { code, message })),
            Message::Progress { id, done, text } => {
                if self.is_admitted(id) {
                    updates.push(Update::Progress { id, done, text });
                }
            }
            Message::Req { id, m, p } => updates.push(Update::Request {
                id,
                method: m,
                params: p,
            }),
            Message::Cancel { id } => updates.push(Update::Cancel { id }),
            Message::Event { e, p } => updates.push(Update::Event { name: e, params: p }),
        }
    }

    /// Whether the program has exited and its unit was ended.
    pub fn has_ended(&self) -> bool {
        self.unit.ended()
    }

    /// How the program exited, once it has and was reaped.
    pub fn exit(&self) -> Option<Exit> {
        self.unit.exit()
    }

    /// A handle that ends the unit without the session.
    pub fn killer(&self) -> Killer {
        Killer(self.unit.clone())
    }

    /// Ends the unit now, without waiting.
    pub fn kill_remaining(&self) {
        self.unit.kill();
    }

    /// Stops the program by the shared absolute `deadline`: sends `quitting`,
    /// closes the sending side once what is queued is written, and returns
    /// the stopper. Its thread waits for the program to exit at most
    /// `min(1 s, deadline − now − 200 ms)`, then ends the unit; with the
    /// deadline already past, it ends it at once.
    pub fn stop(mut self, deadline: Instant) -> std::io::Result<Stopping> {
        self.stopping = true;
        let _ = self.event("quitting", Value::Null);
        self.outgoing.close();
        let now = Instant::now();
        let until = (now + QUIT_GRACE).min(deadline.checked_sub(QUIT_MARGIN).unwrap_or(now));
        let unit = self.unit.clone();
        let thread = std::thread::Builder::new()
            .name("extension stopper".into())
            .spawn(move || {
                if !unit.wait_ended(until) {
                    unit.kill();
                }
            });
        match thread {
            Ok(thread) => Ok(Stopping {
                thread,
                unit: self.unit.clone(),
            }),
            Err(e) => {
                self.unit.kill();
                Err(e)
            }
        }
    }
}

impl Drop for Session {
    /// A session dropped without [`Session::stop`] (its extension disabled,
    /// its owner gone) ends its unit at once. Nothing is joined.
    fn drop(&mut self) {
        self.outgoing.close();
        if !self.stopping {
            self.unit.kill();
        }
    }
}

fn deadline_failure(id: u64, method: String) -> Update {
    Update::Reply {
        id,
        result: Err(RemoteError::new(
            ErrorCode::Deadline,
            format!("the extension didn't answer {method} in time"),
        )),
        method,
    }
}

/// A stopping program: join it on a stopper thread; it returns by the
/// deadline given to [`Session::stop`].
pub struct Stopping {
    thread: JoinHandle<()>,
    unit: Arc<Unit>,
}
impl Stopping {
    /// Waits for the stop to finish: the program exited or its unit was
    /// ended.
    pub fn join(self) {
        let _ = self.thread.join();
    }
    /// Whether [`Stopping::join`] would return at once.
    pub fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }
    /// A handle that ends the unit without waiting.
    pub fn killer(&self) -> Killer {
        Killer(self.unit.clone())
    }
    /// The stopper thread, for an owner that waits for several stops together
    /// under one deadline; it ends by the deadline given to [`Session::stop`].
    pub fn into_thread(self) -> JoinHandle<()> {
        self.thread
    }
}

/// Ends a unit from anywhere, without blocking.
#[derive(Clone)]
pub struct Killer(Arc<Unit>);
impl Killer {
    /// Ends every process of the unit unless it has ended already: `killpg`
    /// or `TerminateJobObject`, which return at once.
    pub fn kill_remaining(&self) {
        self.0.kill();
    }
    /// Whether the unit has ended.
    pub fn has_ended(&self) -> bool {
        self.0.ended()
    }
}

/// Reads `hello`'s reply, refusing another protocol.
pub fn check_hello(value: Value) -> Result<HelloReply, RemoteError> {
    let reply: HelloReply = serde_json::from_value(value)
        .map_err(|e| RemoteError::new(ErrorCode::InvalidRequest, format!("hello's reply: {e}")))?;
    if reply.protocol != PROTOCOL {
        return Err(RemoteError::new(
            ErrorCode::UnsupportedProtocol,
            format!(
                "the extension speaks protocol {}, RAWmakase {PROTOCOL}",
                reply.protocol
            ),
        ));
    }
    Ok(reply)
}

/// The writer thread: writes queued lines until the queue closes or a write
/// fails, then closes the sending side.
fn write(
    mut out: Box<dyn std::io::Write + Send>,
    queue: &Receiver<Line>,
    outgoing: &Outgoing,
    close: Box<dyn FnOnce() + Send>,
) {
    for line in queue {
        let written = out.write_all(&line.bytes).and_then(|()| out.flush());
        if line.owner {
            outgoing.owner_queued.fetch_sub(1, Ordering::SeqCst);
        }
        if written.is_err() {
            break;
        }
    }
    close();
}

/// What the reader thread holds.
struct Reading {
    to_owner: SyncSender<Incoming>,
    outgoing: Arc<Outgoing>,
    lifecycle: Option<Lifecycle>,
    wake: Option<Wake>,
    done: Arc<AtomicBool>,
    dropped: Arc<AtomicU64>,
}
impl Reading {
    /// Reads until the channel closes, then marks the reader done, so the
    /// next [`Session::poll`] fails what is pending and reports
    /// [`Update::Closed`]. A panic in the app's callbacks (`lifecycle`,
    /// `wake`) stops the reading but is caught here, not in a drop guard: a
    /// guard calling `wake` again while that panic unwinds would abort the
    /// app.
    fn run(self, reader: Box<dyn std::io::Read + Send>) {
        let read = std::panic::AssertUnwindSafe(|| self.read(reader));
        let _ = std::panic::catch_unwind(read);
        self.done.store(true, Ordering::SeqCst);
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.wake()));
    }

    fn read(&self, reader: Box<dyn std::io::Read + Send>) {
        let mut lines = LineReader::new(reader);
        while let Ok(Some(line)) = lines.next_message() {
            match line {
                Ok(Message::Req { id, m, p }) => self.request(id, m, p),
                Ok(message) => {
                    if self.to_owner.try_send(Incoming::Message(message)).is_err() {
                        self.dropped.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(e) => {
                    if let LineError::Decode(d) = &e
                        && let Some(id) = d.request
                    {
                        self.answer(
                            id,
                            Err(RemoteError::new(
                                ErrorCode::InvalidRequest,
                                d.reason.clone(),
                            )),
                        );
                    }
                    let _ = self.to_owner.try_send(Incoming::Malformed(e.to_string()));
                }
            }
            self.wake();
        }
    }

    fn request(&self, id: u64, method: String, params: Value) {
        if let Some(lifecycle) = &self.lifecycle
            && let Some(result) = lifecycle(&method, &params)
        {
            self.answer(id, result);
            return;
        }
        let request = Incoming::Message(Message::Req {
            id,
            m: method,
            p: params,
        });
        if let Err(TrySendError::Full(_)) = self.to_owner.try_send(request) {
            self.answer(
                id,
                Err(RemoteError::new(
                    ErrorCode::ChannelFull,
                    "RAWmakase is still working through this extension's earlier requests",
                )),
            );
        }
    }

    /// Answers the program without blocking; when the sending queue is full
    /// the program isn't reading, so the answer would wait unread anyway.
    fn answer(&self, id: u64, result: Result<Value, RemoteError>) {
        if let Ok(mut line) = Message::reply(id, result).encode() {
            line.push(b'\n');
            let _ = self.outgoing.send(line, false);
        }
    }

    fn wake(&self) {
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}
