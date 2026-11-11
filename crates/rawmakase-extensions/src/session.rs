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
//!
//! **Buffers.** [`Session::request_with`] sends a request with [`Buffers`] in
//! shared memory (API 2 programs only), whose handles go with its line. The
//! request owns their lease until its outcome: an `ok` hands the buffers over
//! with [`Update::Reply`], for the app to copy out and drop; an error drops
//! them. A request cancelled or past its deadline settles for the app as any
//! other, but its lease *lingers*, still counted, until the program's reply
//! for it says the program let go of the buffers. Leased ids live in a set the
//! reader shares: it records each reply's outcome there before handing the
//! reply to the bounded queue, which may drop it, so a reply is never lost to
//! the lease. The owner reconciles under that set's lock: a pending request
//! whose reply was dropped is settled from the record, an error as sent and
//! an `ok`, whose value is lost, as `channel_full`.
//!
//! **Hung.** The program is treated as hung, and the session enters
//! [`Update::Hung`] by itself, when a lingering lease passes its grace
//! ([`LEASE_GRACE`]) with no reply, when the program answers a request that
//! carried handles with `invalid_request` (it may hold handles it never
//! adopted), or when it sends the app a handle. The session then kills the
//! unit, fails every request still pending once with `exited`, keeps the
//! outcomes already handed over, refuses new requests, and keeps the leases
//! counted until the unit has ended.
use crate::process::{self, Unit};
use crate::shm::{Buffers, Ledger, Passing, Returned};
use rawmakase_extension_protocol::framing::LineReader;
use rawmakase_extension_protocol::message::Handles;
use rawmakase_extension_protocol::method::HelloReply;
use rawmakase_extension_protocol::{ENV_API, ErrorCode, Message, PROTOCOL, RemoteError};
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
/// How long a cancelled request's buffers wait for the program's reply before
/// the program is treated as hung: longer than [`CANCEL_GRACE`], since a model
/// polls its cancel flag between steps and a remote worker forwards the
/// cancel. Ten seconds without a reply means hung, not slow.
pub const LEASE_GRACE: Duration = Duration::from_secs(10);
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
impl Launch {
    /// Tells the program its manifest's `api` in [`ENV_API`], replacing any
    /// value `env` held, so the SDK knows the protocol's rules before it reads
    /// the first line. Every extension program's launch sets it; a program
    /// started without it serves API 1.
    pub fn set_api(&mut self, api: u32) {
        self.env.retain(|(name, _)| name != ENV_API);
        self.env.push((ENV_API.into(), api.to_string().into()));
    }
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
    /// See [`LEASE_GRACE`].
    pub lease_grace: Duration,
    /// How long the program has to open its channel (Windows).
    pub connect: Duration,
    /// What the extension's buffers count against. The budget is per
    /// extension only when the app keeps one `Ledger` per extension and
    /// passes it to every session of it: a restarted program's session gets
    /// the same ledger, so leases its predecessor still holds (until that
    /// unit has ended) count against the new program's requests. The default,
    /// a new ledger, counts one session alone.
    pub ledger: Ledger,
}
impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            queue: QUEUE,
            lifecycle: None,
            wake: None,
            cancel_grace: CANCEL_GRACE,
            lease_grace: LEASE_GRACE,
            connect: HELLO_DEADLINE,
            ledger: Ledger::new(),
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
        /// A successful request's buffers, with their lease, for reading what
        /// the program wrote; the lease ends when the last clone is dropped.
        buffers: Option<Returned>,
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
    /// The program broke the protocol or held a cancelled request's buffers
    /// past [`LEASE_GRACE`]: the session killed it, failed what was pending
    /// with `exited` and takes no more requests. Comes once; [`Update::Closed`]
    /// follows when the channel closes.
    Hung {
        /// Why.
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
    /// The buffers it carries, and their lease.
    buffers: Option<Buffers>,
}

/// A settled request's lease, waiting for the program's reply.
struct Lingering {
    buffers: Buffers,
    /// Past this with no reply, the program is hung.
    until: Instant,
}

/// What the reader saw a leased request answered with.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Outcome {
    Ok,
    Err(RemoteError),
}
impl Outcome {
    /// Whether the answer is a protocol fault: a request that carried handles
    /// refused as invalid, so the program may hold handles it never adopted.
    fn is_fault(&self) -> bool {
        matches!(self, Self::Err(e) if e.code == ErrorCode::InvalidRequest)
    }
}

/// A leased request's reply as the reader recorded it.
#[derive(Clone, Debug)]
struct Recorded {
    outcome: Outcome,
    /// The full queue dropped the reply itself: only this record reaches the
    /// owner.
    dropped: bool,
}

/// The ids of leased requests not yet reconciled, with the reply the reader
/// recorded for each, shared by the owner and the reader.
type Leased = Arc<Mutex<HashMap<u64, Option<Recorded>>>>;

/// Room in the sending queue beyond the owner's share, for the reader's own
/// answers to the program.
const ANSWER_ROOM: usize = 1024;

/// One line for the writer.
struct Line {
    bytes: Vec<u8>,
    /// Sent by the owner, so it counts against the owner's share.
    owner: bool,
    /// The handles its first byte carries.
    passing: Option<Passing>,
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
    fn send(
        &self,
        bytes: Vec<u8>,
        owner: bool,
        passing: Option<Passing>,
    ) -> Result<(), RemoteError> {
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
        let line = Line {
            bytes,
            owner,
            passing,
        };
        sender.try_send(line).map_err(|e| {
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
    /// The manifest's `api` the program serves.
    api: u32,
    child: crate::shm::Child,
    ledger: Ledger,
    lease_grace: Duration,
    leased: Leased,
    lingering: HashMap<u64, Lingering>,
    /// Leases kept until the unit has ended: the program hung or closed.
    held: Vec<Buffers>,
    /// The reader received a handle from the program.
    fault: Arc<AtomicBool>,
    hung: bool,
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
            fault,
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
        let leased: Leased = Arc::default();
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
                leased: leased.clone(),
            };
            let reader = Watch {
                inner: reader,
                fault: fault.clone(),
                wake: options.wake.clone(),
                told: false,
            };
            std::thread::Builder::new()
                .name("extension reader".into())
                .spawn(move || reading.run(Box::new(reader)))?;
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
            api: api_of(launch),
            child: spawned.child,
            ledger: options.ledger,
            lease_grace: options.lease_grace,
            leased,
            lingering: HashMap::new(),
            held: Vec::new(),
            fault,
            hung: false,
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
        self.outgoing.send(line, true, None)
    }

    /// Refuses a request after the program exited or hung.
    fn check_open(&self) -> Result<(), RemoteError> {
        if self.closed || self.hung {
            return Err(RemoteError::new(
                ErrorCode::Exited,
                "the extension's program exited",
            ));
        }
        Ok(())
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
        self.check_open()?;
        let id = self.next_id;
        self.send(&Message::request(id, method, params))?;
        self.next_id += 1;
        self.pending.insert(
            id,
            Pending {
                method: method.to_string(),
                deadline,
                expired: false,
                buffers: None,
            },
        );
        Ok(id)
    }

    /// New buffers for a request, leased against this session's ledger.
    pub fn buffers(&self) -> Buffers {
        Buffers::new(&self.ledger)
    }

    /// What this session's buffers count against.
    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    /// [`Session::request`] with `buffers`, whose handles go with the line,
    /// in the order they were added: the index each descriptor names. The
    /// request owns their lease until its outcome, and after it while the
    /// program may still hold them (see the module's notes).
    ///
    /// Refused at once with `invalid_request`, and nothing sent, when the
    /// program serves API 1, which takes no handles.
    pub fn request_with(
        &mut self,
        method: &str,
        params: Value,
        buffers: Buffers,
        deadline: Instant,
    ) -> Result<u64, RemoteError> {
        self.check_open()?;
        if self.api < 2 {
            return Err(RemoteError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "the extension's manifest declares api {}, and only api 2 takes buffers",
                    self.api
                ),
            ));
        }
        let id = self.next_id;
        // Windows: the duplicates are in the program from here, and close
        // there again if `passing` drops unsent.
        let passing = buffers.pass(&self.child).map_err(|e| {
            RemoteError::new(
                ErrorCode::Unavailable,
                format!("the buffers' handles couldn't be passed: {e}"),
            )
        })?;
        let handles = Handles {
            handles: passing.count() as u32,
            hv: passing.values(),
        };
        let mut line = Message::request(id, method, params)
            .encode_with(&handles)
            .map_err(|_| {
                RemoteError::new(
                    ErrorCode::TooLarge,
                    "the message is longer than a protocol line",
                )
            })?;
        line.push(b'\n');
        // Leased before it can be answered.
        self.leased_set().insert(id, None);
        if let Err(e) = self.outgoing.send(line, true, Some(passing)) {
            self.leased_set().remove(&id);
            return Err(e);
        }
        self.next_id += 1;
        self.pending.insert(
            id,
            Pending {
                method: method.to_string(),
                deadline,
                expired: false,
                buffers: Some(buffers),
            },
        );
        Ok(id)
    }

    fn leased_set(&self) -> std::sync::MutexGuard<'_, HashMap<u64, Option<Recorded>>> {
        self.leased.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keeps a settled request's buffers until the program answers it.
    fn linger(&mut self, id: u64, buffers: Buffers, now: Instant) {
        let until = now + self.lease_grace;
        self.lingering.insert(id, Lingering { buffers, until });
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
    ///
    /// A request with buffers keeps their lease until the program's reply,
    /// at most [`LEASE_GRACE`]; then the program is hung.
    pub fn cancel(&mut self, id: u64) -> bool {
        let Some(pending) = self.pending.remove(&id) else {
            return false;
        };
        if let Some(buffers) = pending.buffers {
            self.linger(id, buffers, Instant::now());
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

    /// The earliest deadline among the requests in flight and the lingering
    /// leases, for scheduling the next [`Session::poll`].
    pub fn next_deadline(&self) -> Option<Instant> {
        let lingering = self.lingering.values().map(|l| l.until);
        self.pending
            .values()
            .map(|p| p.deadline)
            .chain(lingering)
            .min()
    }

    /// Whether the session hung the program up ([`Update::Hung`]).
    pub fn is_hung(&self) -> bool {
        self.hung
    }

    /// How many of the program's messages were dropped because the owner's
    /// queue was full (replies and events; refused operations are answered).
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Acquire)
    }

    /// Hands over what arrived and enforces deadlines as of `now`.
    pub fn poll(&mut self, now: Instant) -> Vec<Update> {
        let mut updates = Vec::new();
        let mut hang = None;
        if !self.closed {
            // Whether the reader had finished before this drain: then
            // everything it read is in the queue now.
            let finished = self.reader_done.load(Ordering::SeqCst);
            loop {
                match self.incoming.try_recv() {
                    Ok(Incoming::Message(message)) => {
                        self.take(message, &mut updates, &mut hang);
                    }
                    Ok(Incoming::Malformed(reason)) => updates.push(Update::Malformed { reason }),
                    Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
                }
            }
            self.reconcile(&mut updates, &mut hang);
            self.enforce_deadlines(now, &mut updates);
            if finished {
                let mut left: Vec<_> = self.pending.drain().collect();
                left.sort_unstable_by_key(|(id, _)| *id);
                for (id, pending) in left {
                    // Nothing more will answer it: the lease waits for the
                    // unit to end, at most the grace.
                    if let Some(buffers) = pending.buffers {
                        self.linger(id, buffers, now);
                    }
                    updates.push(exited(id, pending.method));
                }
                self.closed = true;
                updates.push(Update::Closed);
            }
        }
        if !self.hung {
            if self.lingering.values().any(|l| l.until <= now) {
                hang.get_or_insert_with(|| {
                    "it didn't answer a cancelled request whose buffers it held".into()
                });
            }
            if self.fault.load(Ordering::SeqCst) {
                hang.get_or_insert_with(|| "it sent the app a handle".into());
            }
            if let Some(reason) = hang {
                self.hang_up(reason, &mut updates);
            }
        }
        // The program is gone: whatever it held is free, and nothing more
        // will answer a leased request.
        if self.unit.ended() && !(self.lingering.is_empty() && self.held.is_empty()) {
            self.lingering.clear();
            self.held.clear();
            let pending = &self.pending;
            self.leased
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .retain(|id, _| pending.contains_key(id));
        }
        updates
    }

    /// Fails expired requests, and cancels those reaching their deadline.
    fn enforce_deadlines(&mut self, now: Instant, updates: &mut Vec<Update>) {
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
                    let _ = self.outgoing.send(line, true, None);
                }
            }
        }
        expired.sort_unstable();
        for id in expired {
            let pending = self.pending.remove(&id).expect("listed above");
            if let Some(buffers) = pending.buffers {
                self.linger(id, buffers, now);
            }
            updates.push(deadline_failure(id, pending.method));
        }
    }

    /// Settles the leases of the replies the reader recorded: a lingering one
    /// is released, and a protocol fault hangs the program up, even when the
    /// reply itself was dropped by a full queue. A pending request whose
    /// reply the queue dropped is settled here, once: an error as it was
    /// sent, an `ok`, whose value is lost, as `channel_full`. So is one
    /// recorded as a fault, whether or not its reply was dropped, so the
    /// hang-up keeps its error. Any other pending request's record waits for
    /// its reply to be drained.
    fn reconcile(&mut self, updates: &mut Vec<Update>, hang: &mut Option<String>) {
        let mut leased = self.leased.lock().unwrap_or_else(|e| e.into_inner());
        let mut settled = Vec::new();
        leased.retain(|id, recorded| {
            let Some(recorded) = recorded else {
                return true;
            };
            if recorded.outcome.is_fault() {
                hang.get_or_insert_with(fault_reason);
            }
            if let Some(lease) = self.lingering.remove(id) {
                if recorded.outcome.is_fault() {
                    self.held.push(lease.buffers);
                }
                return false;
            }
            // A fault is settled with its own error now, dropped or not: the
            // hang-up that follows would otherwise fail it as `exited`. If
            // its reply is drained later, the request is no longer pending.
            if (recorded.dropped || recorded.outcome.is_fault()) && self.pending.contains_key(id) {
                settled.push((*id, recorded.outcome.clone()));
                return false;
            }
            true
        });
        drop(leased);
        settled.sort_unstable_by_key(|(id, _)| *id);
        for (id, outcome) in settled {
            let mut pending = self.pending.remove(&id).expect("pending, checked above");
            let buffers = pending.buffers.take();
            if outcome.is_fault() {
                self.held.extend(buffers);
            } else {
                drop(buffers);
            }
            // A fault keeps its own error, as `take` keeps it for a drained
            // reply: which of the two settles it first doesn't change it.
            updates.push(if pending.expired && !outcome.is_fault() {
                deadline_failure(id, pending.method)
            } else {
                let result = match outcome {
                    Outcome::Ok => Err(RemoteError::new(
                        ErrorCode::ChannelFull,
                        "the extension answered, but its reply was dropped: RAWmakase's queue \
                         for it was full",
                    )),
                    Outcome::Err(e) => Err(e),
                };
                Update::Reply {
                    id,
                    method: pending.method,
                    result,
                    buffers: None,
                }
            });
        }
    }

    /// Enters Hung: kills the unit, fails every pending request once with
    /// `exited`, and keeps every lease until the unit has ended.
    fn hang_up(&mut self, reason: String, updates: &mut Vec<Update>) {
        self.hung = true;
        self.unit.kill();
        let mut left: Vec<_> = self.pending.drain().collect();
        left.sort_unstable_by_key(|(id, _)| *id);
        for (id, pending) in left {
            self.held.extend(pending.buffers);
            updates.push(exited(id, pending.method));
        }
        self.held
            .extend(self.lingering.drain().map(|(_, lease)| lease.buffers));
        self.leased_set().clear();
        updates.push(Update::Hung { reason });
    }

    fn take(&mut self, message: Message, updates: &mut Vec<Update>, hang: &mut Option<String>) {
        let mut settle = |id: u64, result: Result<Value, RemoteError>| {
            let outcome = match &result {
                Ok(_) => Outcome::Ok,
                Err(e) => Outcome::Err(e.clone()),
            };
            let leased = self.leased_set().remove(&id).is_some();
            let fault = leased && outcome.is_fault();
            if fault {
                hang.get_or_insert_with(fault_reason);
            }
            if let Some(lease) = self.lingering.remove(&id) {
                if fault {
                    self.held.push(lease.buffers);
                }
                return;
            }
            // A reply to a request already settled (cancelled, failed) is
            // dropped.
            let Some(mut pending) = self.pending.remove(&id) else {
                return;
            };
            let buffers = pending.buffers.take();
            if fault {
                self.held.extend(buffers);
                updates.push(Update::Reply {
                    id,
                    method: pending.method,
                    result,
                    buffers: None,
                });
                return;
            }
            updates.push(if pending.expired {
                deadline_failure(id, pending.method)
            } else {
                Update::Reply {
                    id,
                    method: pending.method,
                    buffers: buffers.filter(|_| result.is_ok()).map(Returned::new),
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
    ///
    /// Leases still held end when the unit has ended, released by its
    /// termination tracking, not by the stopper: the stop never waits for
    /// them, and a unit that outlives the deadline keeps them counted until
    /// it really ends.
    pub fn stop(mut self, deadline: Instant) -> std::io::Result<Stopping> {
        self.stopping = true;
        let _ = self.event("quitting", Value::Null);
        self.outgoing.close();
        let now = Instant::now();
        let until = (now + QUIT_GRACE).min(deadline.checked_sub(QUIT_MARGIN).unwrap_or(now));
        let unit = self.unit.clone();
        self.keep_leases();
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
    /// its owner gone) ends its unit at once; its leases end once the unit
    /// has. Nothing is joined.
    fn drop(&mut self) {
        self.outgoing.close();
        if !self.stopping {
            self.unit.kill();
        }
        self.keep_leases();
    }
}

impl Session {
    /// Hands every lease still held to the unit, which drops them once it
    /// has ended: until then the program may still map the buffers.
    fn keep_leases(&mut self) {
        let mut leases = std::mem::take(&mut self.held);
        leases.extend(self.lingering.drain().map(|(_, lease)| lease.buffers));
        leases.extend(self.pending.values_mut().filter_map(|p| p.buffers.take()));
        if !leases.is_empty() {
            self.unit.keep_until_ended(Box::new(leases));
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
        buffers: None,
    }
}

fn exited(id: u64, method: String) -> Update {
    Update::Reply {
        id,
        method,
        result: Err(RemoteError::new(
            ErrorCode::Exited,
            "the extension's program exited before answering",
        )),
        buffers: None,
    }
}

fn fault_reason() -> String {
    "it refused a request that carried handles as invalid, and may hold handles it never adopted"
        .into()
}

/// The `api` the launch tells the program ([`Launch::set_api`]): 1 without.
fn api_of(launch: &Launch) -> u32 {
    launch
        .env
        .iter()
        .rev()
        .find(|(name, _)| name == ENV_API)
        .and_then(|(_, value)| value.to_str()?.parse().ok())
        .unwrap_or(1)
}

/// The host's reader, waking the owner when the program first sends a handle:
/// the fault may come with no line to wake it otherwise.
struct Watch {
    inner: Box<dyn std::io::Read + Send>,
    fault: Arc<AtomicBool>,
    wake: Option<Wake>,
    told: bool,
}
impl std::io::Read for Watch {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf);
        if !self.told && self.fault.load(Ordering::SeqCst) {
            self.told = true;
            if let Some(wake) = &self.wake {
                wake();
            }
        }
        read
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

/// The writer thread: writes queued lines, each with its handles, until the
/// queue closes or a write fails, then closes the sending side. Lines left in
/// the queue drop unsent, and their handles with them.
fn write(
    mut out: crate::shm::Writer,
    queue: &Receiver<Line>,
    outgoing: &Outgoing,
    close: Box<dyn FnOnce() + Send>,
) {
    for line in queue {
        let written = out.send(&line.bytes, line.passing);
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
    leased: Leased,
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
        while let Ok(Some(line)) = lines.next_line() {
            let decoded = match line {
                Ok(line) => Message::decode(&line).map(|message| (message, line)),
                Err(e) => {
                    self.malformed(e.to_string());
                    continue;
                }
            };
            match decoded {
                Ok((Message::Req { id, m, p }, line)) => {
                    // A program passes no handles; a request declaring some
                    // is refused, whatever it carried.
                    if Handles::read(&line).map_or(true, |h| h.handles > 0 || h.hv.is_some()) {
                        self.answer(
                            id,
                            Err(RemoteError::new(
                                ErrorCode::InvalidRequest,
                                "a program sends no handles",
                            )),
                        );
                    } else {
                        self.request(id, m, p);
                    }
                }
                Ok((message, _)) => {
                    let id = self.record(&message);
                    if self.to_owner.try_send(Incoming::Message(message)).is_err() {
                        // Marked before it is counted: whoever sees the count
                        // (Acquire in `Session::dropped`) and then reconciles
                        // sees the mark.
                        if let Some(id) = id {
                            self.dropped_reply(id);
                        }
                        self.dropped.fetch_add(1, Ordering::Release);
                    }
                }
                Err(d) => {
                    if let Some(id) = d.request {
                        self.answer(
                            id,
                            Err(RemoteError::new(
                                ErrorCode::InvalidRequest,
                                d.reason.clone(),
                            )),
                        );
                    }
                    self.malformed(d.to_string());
                }
            }
            self.wake();
        }
    }

    fn malformed(&self, reason: String) {
        let _ = self.to_owner.try_send(Incoming::Malformed(reason));
    }

    /// Records a leased request's outcome before the reply goes to the
    /// queue, which may drop it. Returns the id when it recorded one.
    fn record(&self, message: &Message) -> Option<u64> {
        let (id, outcome) = match message {
            Message::Ok { id, .. } => (*id, Outcome::Ok),
            Message::Err { id, code, message } => (
                *id,
                Outcome::Err(RemoteError::new(code.clone(), message.clone())),
            ),
            _ => return None,
        };
        let mut leased = self.leased.lock().unwrap_or_else(|e| e.into_inner());
        let slot = leased.get_mut(&id)?;
        if slot.is_some() {
            return None;
        }
        *slot = Some(Recorded {
            outcome,
            dropped: false,
        });
        Some(id)
    }

    /// The queue dropped a leased request's reply: the owner settles it from
    /// the record.
    fn dropped_reply(&self, id: u64) {
        let mut leased = self.leased.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(Some(recorded)) = leased.get_mut(&id) {
            recorded.dropped = true;
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
            let _ = self.outgoing.send(line, false, None);
        }
    }

    fn wake(&self) {
        if let Some(wake) = &self.wake {
            wake();
        }
    }
}

#[cfg(test)]
mod tests {
    //! Interleavings of the reader and the owner that a real program can't
    //! force: the reader's record is written here directly, as the reader
    //! writes it, against a program that only idles.
    use super::*;

    const IDLE: &str = "RAWMAKASE_SESSION_TEST_IDLE";

    /// The idle program: this test binary run again, which waits to be ended.
    #[test]
    fn idle_program() {
        if std::env::var_os(IDLE).is_some() {
            std::thread::sleep(Duration::from_secs(60));
        }
    }

    fn idle(dir: &std::path::Path) -> Session {
        let launch = Launch {
            program: std::env::current_exe().unwrap(),
            args: ["--exact", "session::tests::idle_program", "--nocapture"]
                .iter()
                .map(Into::into)
                .collect(),
            dir: dir.to_path_buf(),
            env: vec![(IDLE.into(), "1".into())],
            log: dir.join("log.txt"),
        };
        // The idle program never opens a Windows channel; the reader must not
        // give up on it during the test.
        let options = SessionOptions {
            connect: Duration::from_secs(60),
            ..SessionOptions::default()
        };
        Session::spawn(&launch, options).unwrap()
    }

    /// A pending leased request, as `request_with` leaves it.
    fn leased_request(session: &mut Session, id: u64) {
        let mut buffers = session.buffers();
        buffers.output(16).unwrap();
        session.pending.insert(
            id,
            Pending {
                method: "mask.generate".into(),
                deadline: Instant::now() + Duration::from_secs(60),
                expired: false,
                buffers: Some(buffers),
            },
        );
        session.leased_set().insert(id, None);
    }

    fn replies(updates: &[Update], id: u64) -> Vec<Result<Value, RemoteError>> {
        updates
            .iter()
            .filter_map(|u| match u {
                Update::Reply { id: i, result, .. } if *i == id => Some(result.clone()),
                _ => None,
            })
            .collect()
    }

    /// The reader recorded `invalid_request` for a request that carried
    /// handles but hasn't marked the reply dropped yet when the owner
    /// reconciles: the request is settled with its own error before the
    /// hang-up, once, and the reply drained later settles nothing more.
    #[test]
    fn a_recorded_fault_keeps_its_error_through_the_hang_up() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = idle(dir.path());
        leased_request(&mut session, 7);
        let refused = RemoteError::new(ErrorCode::InvalidRequest, "refused");
        session.leased_set().insert(
            7,
            Some(Recorded {
                outcome: Outcome::Err(refused.clone()),
                dropped: false,
            }),
        );
        let updates = session.poll(Instant::now());
        assert_eq!(replies(&updates, 7), [Err(refused.clone())]);
        assert!(updates.iter().any(|u| matches!(u, Update::Hung { .. })));
        assert!(session.ledger().in_use() > 0 || session.has_ended());
        // The reply itself, drained after all.
        let mut more = Vec::new();
        let mut hang = None;
        session.take(Message::reply(7, Err(refused)), &mut more, &mut hang);
        assert!(replies(&more, 7).is_empty());
        let start = Instant::now();
        while session.ledger().in_use() > 0 {
            assert!(start.elapsed() < Duration::from_secs(20));
            more.extend(session.poll(Instant::now()));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(replies(&more, 7).is_empty(), "{more:?}");
    }

    /// The same, for a request past its deadline and in its grace: the
    /// recorded fault keeps its own error, as a drained reply would, rather
    /// than `deadline`; once, then the hang-up, the lease kept until the unit
    /// has ended.
    #[test]
    fn a_recorded_fault_in_deadline_grace_keeps_its_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut session = idle(dir.path());
        leased_request(&mut session, 9);
        {
            let pending = session.pending.get_mut(&9).unwrap();
            pending.expired = true;
            pending.deadline = Instant::now() + Duration::from_secs(60);
        }
        let refused = RemoteError::new(ErrorCode::InvalidRequest, "refused");
        session.leased_set().insert(
            9,
            Some(Recorded {
                outcome: Outcome::Err(refused.clone()),
                dropped: false,
            }),
        );
        let mut seen = session.poll(Instant::now());
        assert_eq!(replies(&seen, 9), [Err(refused.clone())]);
        assert!(seen.iter().any(|u| matches!(u, Update::Hung { .. })));
        assert!(session.ledger().in_use() > 0 || session.has_ended());
        let mut hang = None;
        session.take(Message::reply(9, Err(refused)), &mut seen, &mut hang);
        let start = Instant::now();
        while session.ledger().in_use() > 0 {
            assert!(start.elapsed() < Duration::from_secs(20));
            seen.extend(session.poll(Instant::now()));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(session.has_ended(), "released only once the unit ended");
        assert_eq!(replies(&seen, 9).len(), 1, "{seen:?}");
    }

    /// A leased reply the full queue drops is marked dropped before it is
    /// counted: once the count shows it, reconciling settles it.
    #[test]
    fn a_dropped_reply_is_marked_before_it_is_counted() {
        let leased: Leased = Arc::default();
        leased.lock().unwrap().insert(3, None);
        // Room for nothing: every message is dropped.
        let (to_owner, _incoming) = sync_channel(0);
        let (send, _to_writer) = sync_channel(1);
        let reading = Reading {
            to_owner,
            outgoing: Arc::new(Outgoing {
                sender: Mutex::new(Some(send)),
                owner_queued: AtomicUsize::new(0),
                limit: 1,
            }),
            lifecycle: None,
            wake: None,
            done: Arc::default(),
            dropped: Arc::default(),
            leased: leased.clone(),
        };
        reading.read(Box::new(&b"{\"t\":\"ok\",\"id\":3,\"v\":1}\n"[..]));
        assert_eq!(reading.dropped.load(Ordering::Acquire), 1);
        let recorded = leased.lock().unwrap()[&3].clone().unwrap();
        assert!(recorded.dropped);
        assert_eq!(recorded.outcome, Outcome::Ok);
    }
}
