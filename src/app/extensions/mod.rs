//! Extensions inside the editor: which are loaded, their programs' sessions,
//! the requests the app sends them and what they ask back.
//!
//! The host crate ([`rawmakase_extensions`]) knows packages, `extensions.json`
//! and a program's session; this module decides when a program runs and what
//! its requests mean to the editor:
//!
//! - [`Host`] owns every loaded extension: bundled ones (MCP, MIDI), installed
//!   ones and those loaded from a folder in developer mode, with what this
//!   session knows of each (running, crashed, Failed). It is started only by
//!   `Editor::new`; a test editor's is inert and reads nothing.
//! - `with-app` programs start after the first frame, and again after they
//!   end, until they are Failed; `on-demand` ones start at their first
//!   request, and an `on-demand` program idle for [`IDLE_STOP`] is stopped.
//! - A program's control adapters report their devices (`adapter.device`) to
//!   the [`Devices`] table of its start, applied by the session's reader
//!   thread outside the operation queue; their operations
//!   (`adapter.operate`) run through the editor, each only in its device's
//!   current epoch, as gestures of a source the host issued for that start.
//! - [`Host::poll`] runs every frame: it hands over what programs sent,
//!   enforces deadlines through the session, counts crashes and answers the
//!   programs' requests through the editor (see `editor.rs`). A program the
//!   session hangs up ([`Update::Hung`]: it broke the protocol, or held a
//!   cancelled request's buffers too long) counts as a crash, as an
//!   unexpected exit does.
//! - Each extension keeps one [`Ledger`] for its whole time in the host, given
//!   to every session of its program: the shared-memory budget is the
//!   extension's, so a restarted program can't allocate while its
//!   predecessor still holds buffers.
//!
//! A program's request that changes photos (`propose.settings`) belongs to one
//! of the app's `command.run` requests still in flight. [`Running::requests`]
//! is the one per-request state on the UI thread: a request is in it from
//! when it is sent until its reply, its cancellation or its deadline, and a
//! proposal is admitted only while its `op` is there and the session still
//! admits it, checked immediately before the editor applies it.
//!
//! A proposal that changes photos not open in Develop runs on a worker (see
//! `targets.rs`) and is answered when it finishes. Until then it is a
//! [`Deferred`] of the session, holding the work's cancel: the moment its
//! `op` stops being admitted (answered, cancelled, past its deadline, the
//! extension disabled, the program gone, quitting), the cancel is set, and
//! the worker writes nothing it hadn't written. The worker's one write takes
//! the same cancel inside its transaction, so the two race through one state.
//! A program still there is answered `not_admitted` as the cancel is set
//! (or `cancelled` when it cancelled the proposal itself), unless the write
//! had begun: then it is answered with what was written.
//!
//! A `[[source]]` reports files in a scope the host issues
//! ([`Host::set_catalog`], [`Host::sync_sources`]): one per source, while its
//! program is ready, its extension enabled, a catalog open and the source
//! turned on for that catalog (in `extensions.json`). The program hears
//! `source-started` and `source-stopped`; a scope's number is never reused,
//! and a program start gets its scopes again after `hello`. A
//! `source.report` is admitted only in its source's current scope, and is
//! added on a worker (see `sources.rs`) as a [`Deferred`] holding the
//! worker's write gate: the scope ending, the program cancelling it or its
//! session ending closes the gate, and the worker writes nothing unless it
//! has begun.
//!
//! A read of photos not open in Develop opens their files, so it runs on a
//! worker too: a program's read is a [`Deferred`] with no `op`, answered by
//! [`Host::finish_read`] and cancelled with its session or by the program;
//! a command's target is a [`TargetRead`] of its extension, and the command
//! is sent only once it is read. Until then the command is running (the
//! status line and Stop show it), and Stop, disabling, uninstalling or
//! quitting cancels the read and fails the command; its read's report then
//! finds nothing and is dropped.
//!
//! Every persisted change (enable, disable, a setting, an uninstall) goes
//! through [`rawmakase_extensions::state::State::update`] first; when that
//! fails the session's state stays as it was and the failure is reported.
//!
//! Quitting follows docs/shutdown.md: requests are cancelled
//! ([`Host::cancel_requests`]), nothing more is admitted or read
//! ([`Host::close_channels`]), each program is stopped by the shared deadline
//! ([`Host::stop`]), and the exit hook ends every unit left
//! ([`Host::kill_remaining`]).
pub(in crate::app) mod command_input;
mod devices;
mod editor;
pub(in crate::app) mod mask_generator;
#[cfg(test)]
mod midi_tests;
mod page;
mod panels;
mod sources;
#[cfg(test)]
mod sources_tests;
mod targets;
#[cfg(test)]
mod tests;
pub(in crate::app) mod viewer_input;

pub(in crate::app) use devices::Devices;
pub(in crate::app) use editor::Told;
pub(crate) use editor::{Done, PathTarget};
pub(in crate::app) use page::Page;
pub(crate) use sources::ReportDone;

use crate::app::shell::commands::registry::{self, Dynamic};
use rawmakase_extensions::protocol::manifest::{Input, Launch as AdapterLaunch, Start};
use rawmakase_extensions::protocol::method::{
    AppEvent, AppRequest, CommandRun, Hello, Level, PanelChanged, PanelClicked, PanelOpen,
    ProgramEvent, ProgramRequest, SettingChanged, Snapshot, SourceStarted, SourceStopped,
};
use rawmakase_extensions::protocol::viewer::ViewerInput;
use rawmakase_extensions::protocol::{ErrorCode, PROTOCOL, RemoteError, controls::Control};
use rawmakase_extensions::session::{
    self, Killer, Launch, Session, SessionOptions, Update, check_hello,
};
use rawmakase_extensions::shm::{Buffers, Ledger, Returned};
use rawmakase_extensions::state::{Entry, Source, State};
use rawmakase_extensions::store::Store;
use rawmakase_extensions::{Manifest, semver};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// An `on-demand` program with nothing to do for this long is stopped (D10).
pub(in crate::app) const IDLE_STOP: Duration = Duration::from_secs(5 * 60);
/// Running this long without a crash clears the crash count, for `with-app`
/// programs that get few requests.
const CALM: Duration = Duration::from_secs(5 * 60);
/// Crashes in a row after which an extension is Failed until Retry (D12:
/// for this session only).
pub(in crate::app) const CRASHES: u32 = 3;
/// The last errors kept per extension, for its page.
const ERRORS: usize = 5;

/// The bundled extensions' manifests, compiled in; their programs are found
/// beside the app's executable.
const BUNDLED: &[&str] = &[
    include_str!("../../../extensions/mcp/rawmakase-extension.toml"),
    include_str!("../../../extensions/midi/rawmakase-extension.toml"),
    include_str!("../../../extensions/ai-masks/rawmakase-extension.toml"),
];

/// Tells a bundled program the folders the app searches for ONNX Runtime
/// beside its executable, `dirs`, in
/// [`HOST_RUNTIME_DIRS`](rawmakase_inference::runtime::HOST_RUNTIME_DIRS),
/// so the AI masks extension runs on the runtime the app bundles. The user's
/// `RAWMAKASE_ORT_LIB` is inherited as it is, and searched before these.
fn pass_runtime_dirs(launch: &mut Launch, dirs: &[PathBuf]) {
    use rawmakase_inference::runtime::HOST_RUNTIME_DIRS;
    launch.env.retain(|(name, _)| name != HOST_RUNTIME_DIRS);
    if let Ok(joined) = std::env::join_paths(dirs) {
        launch.env.push((HOST_RUNTIME_DIRS.into(), joined));
    }
}

/// Files the core kept in the data folder before an extension took them
/// over, by extension: on the extension's first start the file is copied
/// into its data folder, unless one is there already, and the original
/// stays, so an older RAWmakase still reads it.
const TAKEN_OVER: &[(&str, &str)] = &[("rawmakase.midi", "midi.json")];

/// Where a loaded extension comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum Origin {
    /// It ships with the app and can't be uninstalled.
    Bundled,
    /// Installed under `<data>/extensions/<id>/<version>/`.
    Installed,
    /// An unpacked folder loaded in developer mode.
    Developer,
}

/// Who asked for a command to run, for how its outcome is said.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::app) enum RunBy {
    /// A key.
    Key,
    /// The Run button on its extension's page.
    Button,
    /// A control request (`rawmakase control action <ext>/<cmd>`).
    Control,
}

/// An app request in flight, until its reply, cancellation or deadline.
#[derive(Clone, Debug, PartialEq)]
enum Pending {
    Hello,
    Command {
        command: String,
        by: RunBy,
    },
    Panel {
        panel: String,
    },
    /// `mask.generate`, named by the host's ticket: its id is known only once
    /// it is sent, which may wait for the handshake.
    Mask {
        ticket: u64,
    },
    /// `mask.setup` for one of the extension's generators.
    MaskSetup {
        generator: String,
    },
}

/// A request waiting for the program's handshake.
struct Queued {
    request: AppRequest,
    pending: Pending,
    /// Its buffers, sent with it; their lease is held meanwhile.
    buffers: Option<Buffers>,
}

/// A program's proposal being applied by a worker, until the worker reports
/// back ([`Host::finish_proposal`]), or until its command stops being
/// admitted or the program cancels it ([`Host::withdraw`], which answers it
/// then). Dropping it cancels the work, which then writes nothing: the
/// session ends with its proposals.
/// A program's read of photos not open is one too, until its worker reports
/// back ([`Host::finish_read`]); it belongs to no command.
struct Deferred {
    ticket: u64,
    /// The program's request, answered when the work finishes.
    request: u64,
    /// The `command.run` a proposal belongs to; none for a read or a report.
    op: Option<u64>,
    /// The source and scope a report belongs to; none for anything else.
    scope: Option<(String, u64)>,
    cancel: Work,
}
impl Drop for Deferred {
    fn drop(&mut self) {
        // Once a proposal has started writing, or the work has finished,
        // this changes nothing.
        self.cancel.cancel();
    }
}

/// The cancel of a program's request answered later.
enum Work {
    Proposal(Arc<crate::app::library::sync::BatchProgress>),
    Read(Arc<AtomicBool>),
    Report(Arc<sources::WriteGate>),
}
impl Work {
    /// Stops the work; false once a proposal has begun its write, which it
    /// then finishes.
    fn cancel(&self) -> bool {
        match self {
            Self::Proposal(progress) => progress.cancel(),
            Self::Read(cancel) => {
                cancel.store(true, Ordering::Relaxed);
                true
            }
            Self::Report(gate) => gate.close(),
        }
    }
}

/// A command's target being read on a worker; the command is sent once it
/// is ([`Host::finish_read`]). Dropping it cancels the read.
struct TargetRead {
    ticket: u64,
    command: String,
    by: RunBy,
    /// The viewer input collected for it, sent with its target.
    input: Option<ViewerInput>,
    cancel: Arc<AtomicBool>,
}
impl Drop for TargetRead {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// A program running, or starting.
struct Running {
    session: Session,
    /// Done with `hello`; requests before it wait in `queued`.
    ready: bool,
    queued: Vec<Queued>,
    /// The per-request state: see the module's documentation.
    requests: HashMap<u64, Pending>,
    started: Instant,
    /// Stopped by the host (disabled, idle, quitting): its end is no crash.
    stopping: bool,
    /// Its proposals being applied.
    deferred: Vec<Deferred>,
    /// The `session` number `hello` sent: its devices' gestures carry it.
    number: u64,
    /// Its control adapters' devices.
    devices: std::sync::Arc<Devices>,
    /// Its sources' scopes running now, by source: the scope's number and
    /// the catalog it is for.
    scopes: BTreeMap<String, (u64, String)>,
}

/// One host-drawn panel's controls, as its program last returned them.
#[derive(Clone, Debug, Default)]
pub(in crate::app) struct PanelView {
    pub controls: Vec<Control>,
    /// Why the program's last answer couldn't be drawn, or its error.
    pub error: Option<String>,
    /// Asked for and not answered yet.
    pub waiting: bool,
    /// Asked for once since its extension started.
    pub opened: bool,
    /// Values being edited and not yet sent or answered, by control id.
    pub drafts: HashMap<String, Value>,
}

/// A loaded extension.
pub(in crate::app) struct Extension {
    pub manifest: Manifest,
    pub origin: Origin,
    /// Its folder: the version's, a developer folder, or the app's own for a
    /// bundled one.
    pub dir: PathBuf,
    /// Its program, when it has one that can run.
    program: Option<PathBuf>,
    /// Why it can't run (an incompatible version, a program missing), shown
    /// on its page.
    pub problem: Option<String>,
    /// What `extensions.json` says of it.
    pub entry: Entry,
    run: Option<Running>,
    /// What its requests' buffers count against, in every session of its
    /// program: the host's ledger for its id ([`Host`]'s `ledgers`), so
    /// leases a program that ended still holds until its unit has ended
    /// count against the next, whatever entry it has.
    ledger: Ledger,
    /// Counts the times its program was ended by the host or ended by itself
    /// (disabled, uninstalled, loaded again, crashed, hung), shared by every
    /// entry its id has, as `ledger` is ([`Host`]'s `lifecycles`). Work for
    /// the extension captures it, and is void once it has moved.
    lifecycle: Arc<std::sync::atomic::AtomicU64>,
    crashes: u32,
    /// Crashed [`CRASHES`] times in a row: nothing starts it until Retry.
    pub failed: bool,
    pub errors: VecDeque<String>,
    /// The program's `status` entry.
    pub status: Option<String>,
    pub panels: BTreeMap<String, PanelView>,
    /// Its mask generators' setups running or failed, by generator.
    pub setups: BTreeMap<String, mask_generator::Setup>,
    /// What the program last said of a mask request it is answering, by its
    /// ticket: where it runs (`on <endpoint>`).
    mask_progress: Option<(u64, String)>,
    /// Commands whose target is being read, before they are sent.
    reading: Vec<TargetRead>,
    last_active: Instant,
    /// The manifest's and program's modification times, for developer mode.
    mtimes: (Option<std::time::SystemTime>, Option<std::time::SystemTime>),
}

impl Extension {
    pub(in crate::app) fn id(&self) -> &str {
        &self.manifest.extension.id
    }
    pub(in crate::app) fn name(&self) -> &str {
        &self.manifest.extension.name
    }
    pub(in crate::app) fn enabled(&self) -> bool {
        self.entry.enabled
    }
    /// Whether its program runs now.
    pub(in crate::app) fn running(&self) -> bool {
        self.run.is_some()
    }
    /// Its settings: the manifest's defaults, with the values `extensions.json`
    /// holds where they fit their control.
    pub(in crate::app) fn settings(&self) -> Map<String, Value> {
        self.manifest
            .settings
            .iter()
            .map(|s| {
                let value = self
                    .entry
                    .settings
                    .get(&s.id)
                    .filter(|v| s.control.accepts(v))
                    .cloned()
                    .unwrap_or_else(|| s.default.clone());
                (s.id.clone(), value)
            })
            .collect()
    }
    fn error(&mut self, text: impl Into<String>) {
        if self.errors.len() == ERRORS {
            self.errors.pop_front();
        }
        self.errors.push_back(text.into());
    }
    /// Whether a request may start it, or go to it.
    fn can_run(&self) -> Result<(), String> {
        if !self.entry.enabled {
            return Err(format!("{} is disabled", self.name()));
        }
        if self.failed {
            return Err(format!(
                "{} stopped working; Retry it in Preferences > Extensions",
                self.name()
            ));
        }
        if let Some(problem) = &self.problem {
            return Err(problem.clone());
        }
        if self.program.is_none() {
            return Err(format!("{} has no program", self.name()));
        }
        Ok(())
    }
    /// Whether the program is active: starting, or with a request in flight,
    /// or a command's target is being read for it.
    fn busy(&self) -> bool {
        !self.reading.is_empty()
            || self.run.as_ref().is_some_and(|r| {
                !r.ready || !r.requests.is_empty() || !r.queued.is_empty() || !r.deferred.is_empty()
            })
    }
    fn ready_session(&mut self) -> Option<&mut Running> {
        self.run.as_mut().filter(|r| r.ready && !r.stopping)
    }
}

/// What a poll hands the editor, besides the requests it answered.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::app) enum Outcome {
    /// A command finished, or failed.
    Command {
        extension: String,
        title: String,
        by: RunBy,
        result: Result<Value, RemoteError>,
    },
    /// Something for the top bar.
    Message { level: Level, text: String },
    /// A `mask.generate` request's one outcome: the program's reply with the
    /// buffers it wrote, or its failure (`exited`, `disabled`, `deadline`).
    Mask {
        extension: String,
        ticket: u64,
        result: Result<Value, RemoteError>,
        buffers: Option<Returned>,
    },
    /// A `mask.setup` request's one outcome.
    MaskSetup {
        extension: String,
        generator: String,
        result: Result<Value, RemoteError>,
    },
}

/// A request from a program for the editor to answer: everything but the
/// requests the host answers itself (`notify`, `status`).
pub(in crate::app) struct Call {
    pub request: ProgramRequest,
    /// What a proposal's History step and Undo are named: the command it
    /// belongs to. The extension's name for anything else.
    pub title: String,
    /// Names a proposal answered later, in [`Host::finish_proposal`].
    pub ticket: u64,
    /// When a proposal's command stops being admitted by its deadline.
    pub deadline: Option<Instant>,
    /// For `adapter.operate`: the program start's number and its devices.
    pub devices: Option<(u64, std::sync::Arc<Devices>)>,
}

/// How the editor answers a [`Call`].
pub(in crate::app) enum Answer {
    Now(Result<Value, RemoteError>),
    /// The editor started work that answers it, through
    /// [`Host::finish_proposal`] with the call's ticket; this is its cancel.
    Later(std::sync::Arc<crate::app::library::sync::BatchProgress>),
    /// The editor started a read that answers it, through
    /// [`Host::finish_read`] with the call's ticket; this is its cancel.
    Reading(Arc<AtomicBool>),
    /// The editor started adding a report's files, which answers it through
    /// [`Host::finish_report`] with the call's ticket; this is its gate.
    Adding(Arc<sources::WriteGate>),
}

/// An extension found while loading: its manifest, origin, folder, program
/// and why it can't run.
type Found = (Manifest, Origin, PathBuf, Option<PathBuf>, Option<String>);

/// Every extension loaded in this session, and their programs.
pub(in crate::app) struct Host {
    /// None in a test editor's inert host: nothing is read or started.
    store: Option<Store>,
    app: semver::Version,
    wake: Option<session::Wake>,
    extensions: Vec<Extension>,
    /// The `session` number the next start sends.
    next_number: u64,
    /// The ticket the next [`Call`] gets.
    next_ticket: u64,
    polls: u64,
    /// Quitting: nothing is admitted or read any more.
    closed: bool,
    /// Every unit started, for [`Host::kill_remaining`].
    killers: Vec<Killer>,
    /// Each extension's ledger, by id, for as long as the host runs: whatever
    /// entry an id has (restarted, reloaded, uninstalled and installed again,
    /// or falling back to the bundled one) counts against the same budget as
    /// its predecessors, whose leases may still be held.
    ledgers: HashMap<String, Ledger>,
    /// Each extension's lifecycle count, by id, for as long as the host runs.
    lifecycles: HashMap<String, Arc<std::sync::atomic::AtomicU64>>,
    /// Counts changes to the enabled extensions' commands.
    revision: u64,
    /// For the next Preferences page to report.
    errors: Vec<String>,
    /// Where bundled extensions' programs are looked for.
    search_dirs: Vec<PathBuf>,
    /// How long an app request may take before it is cancelled.
    request_deadline: Duration,
    /// Problems for the top bar, from the next poll.
    notices: Vec<String>,
    /// The catalog open in the editor, which sources' scopes are for.
    catalog: Option<sources::OpenCatalog>,
    /// The number the next source scope gets.
    next_scope: u64,
    /// Tests start every program that isn't bundled as this instead: their
    /// own test binary, running the example extension.
    #[cfg(test)]
    program_override: Option<Launch>,
    /// Every proposal's reply, by ticket, for tests that make proposals
    /// without a program.
    #[cfg(test)]
    pub(in crate::app) finished: Vec<(u64, Result<Value, RemoteError>)>,
    /// Every `mask.generate` sent, for tests of what a gesture asked.
    #[cfg(test)]
    masks_sent: Vec<rawmakase_extensions::protocol::method::MaskGenerate>,
    /// Tests start these bundled extensions, by id, as these programs.
    #[cfg(test)]
    bundled_override: Vec<(String, Launch)>,
    /// The size of each session's queues, smaller in a test.
    #[cfg(test)]
    queue: usize,
}

impl Default for Host {
    fn default() -> Self {
        Self::inert()
    }
}

impl Host {
    /// A host that loads and starts nothing: a test editor's, and the
    /// placeholder while [`Host::poll`] answers through the editor.
    pub(in crate::app) fn inert() -> Self {
        Self {
            store: None,
            app: app_version(),
            wake: None,
            extensions: Vec::new(),
            next_number: 1,
            next_ticket: 1,
            polls: 0,
            closed: false,
            killers: Vec::new(),
            ledgers: HashMap::new(),
            lifecycles: HashMap::new(),
            revision: 0,
            errors: Vec::new(),
            search_dirs: Vec::new(),
            request_deadline: session::REQUEST_DEADLINE,
            notices: Vec::new(),
            catalog: None,
            next_scope: 1,
            #[cfg(test)]
            program_override: None,
            #[cfg(test)]
            finished: Vec::new(),
            #[cfg(test)]
            masks_sent: Vec::new(),
            #[cfg(test)]
            bundled_override: Vec::new(),
            #[cfg(test)]
            queue: session::QUEUE,
        }
    }
    /// A host of `data_dir` whose programs all start as `program` (its
    /// program, arguments and environment), with requests timing out after
    /// `deadline`.
    #[cfg(test)]
    pub(in crate::app) fn start_for_tests(
        data_dir: &Path,
        program: Launch,
        deadline: Duration,
    ) -> Self {
        let mut host = Self {
            store: Some(Store::new(data_dir)),
            wake: None,
            request_deadline: deadline,
            program_override: Some(program),
            ..Self::inert()
        };
        host.reload();
        host
    }
    /// A host of `data_dir` whose bundled extension `id` starts as
    /// `program`, with sessions' queues of `queue` messages; the other
    /// bundled ones aren't found, and nothing else starts.
    #[cfg(test)]
    pub(in crate::app) fn with_bundled_for_tests(
        data_dir: &Path,
        id: &str,
        program: Launch,
        queue: usize,
    ) -> Self {
        let mut host = Self {
            store: Some(Store::new(data_dir)),
            bundled_override: vec![(id.to_string(), program)],
            queue,
            ..Self::inert()
        };
        host.reload();
        host
    }
    /// A host of `data_dir` whose bundled programs are found in
    /// `search_dirs`, as a release finds them beside the app.
    #[cfg(test)]
    pub(in crate::app) fn with_search_dirs_for_tests(
        data_dir: &Path,
        search_dirs: Vec<PathBuf>,
    ) -> Self {
        let mut host = Self {
            store: Some(Store::new(data_dir)),
            search_dirs,
            ..Self::inert()
        };
        host.reload();
        host
    }
    /// The test launch of bundled extension `id`, if it has one.
    #[cfg(test)]
    fn bundled_launch(&self, id: &str) -> Option<&Launch> {
        self.bundled_override
            .iter()
            .find(|(b, _)| b == id)
            .map(|(_, launch)| launch)
    }
    #[cfg(not(test))]
    fn bundled_launch(&self, _: &str) -> Option<&Launch> {
        None
    }
    /// A host in `dir` with the example extension loaded from a folder, whose
    /// program can't start: for drawing its page without running anything.
    #[cfg(test)]
    pub(in crate::app) fn with_example_for_tests(dir: &Path) -> Self {
        let folder = dir.join("example");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join(rawmakase_extensions::protocol::manifest::FILE),
            include_str!("../../../extensions/example/rawmakase-extension.toml"),
        )
        .unwrap();
        let data = dir.join("data");
        Store::new(&data).add_developer(&folder).unwrap();
        let missing = Launch {
            program: dir.join("no-such-program"),
            args: Vec::new(),
            dir: PathBuf::new(),
            env: Vec::new(),
            log: PathBuf::new(),
        };
        Self::start_for_tests(&data, missing, session::REQUEST_DEADLINE)
    }
    /// Whether every program this host started has ended, its whole unit.
    #[cfg(test)]
    pub(in crate::app) fn all_ended(&self) -> bool {
        self.extensions.iter().all(|e| e.run.is_none())
            && self.killers.iter().all(Killer::has_ended)
    }
    /// The host of `data_dir`, with its extensions loaded and none started
    /// yet; `wake` repaints when a program sends something.
    pub(in crate::app) fn start(data_dir: &Path, wake: session::Wake) -> Self {
        let mut host = Self {
            store: Some(Store::new(data_dir)),
            wake: Some(wake),
            search_dirs: rawmakase_extensions::bundled::program_search_dirs(),
            ..Self::inert()
        };
        host.reload();
        // What an earlier run left: staging folders, versions replaced.
        if let Some(store) = &host.store {
            store.clean(&[]);
        }
        host
    }
    /// Whether this host loads extensions at all.
    pub(in crate::app) fn active(&self) -> bool {
        self.store.is_some()
    }
    pub(in crate::app) fn store(&self) -> Option<&Store> {
        self.store.as_ref()
    }
    pub(in crate::app) fn app_version(&self) -> &semver::Version {
        &self.app
    }
    pub(in crate::app) fn extensions(&self) -> &[Extension] {
        &self.extensions
    }
    pub(in crate::app) fn get(&self, id: &str) -> Option<&Extension> {
        self.extensions.iter().find(|e| e.id() == id)
    }
    /// A panel's view, as last drawn.
    pub(in crate::app) fn panel(&self, id: &str, panel: &str) -> Option<&PanelView> {
        self.get(id)?.panels.get(panel)
    }
    pub(in crate::app) fn panel_mut(&mut self, id: &str, panel: &str) -> Option<&mut PanelView> {
        let index = self.index(id)?;
        self.extensions[index].panels.get_mut(panel)
    }
    fn index(&self, id: &str) -> Option<usize> {
        self.extensions.iter().position(|e| e.id() == id)
    }
    pub(in crate::app) fn take_errors(&mut self) -> Vec<String> {
        std::mem::take(&mut self.errors)
    }
    /// Whether `adapter`, as a control request names it
    /// (`<extension>/<adapter>`), is a control adapter of an extension loaded
    /// and enabled here. The control socket asks before running each request
    /// that names one, and again before answering a `wait` it parked, so
    /// disabling an extension refuses even an externally launched adapter's
    /// requests already queued (D8).
    pub(in crate::app) fn adapter_enabled(&self, adapter: &str) -> bool {
        let Some((id, name)) = adapter.split_once('/') else {
            return false;
        };
        self.get(id).is_some_and(|e| {
            e.enabled() && e.manifest.control_adapters.iter().any(|a| a.id == name)
        })
    }

    /// Whether device `device`'s connection `epoch`, of the program start
    /// `session`, is still its current one: false once it disconnected, its
    /// program ended or crashed, or its extension was disabled. The editor
    /// ends a gesture of a device that no longer is.
    pub(in crate::app) fn device_current(&self, session: u64, device: u64, epoch: u64) -> bool {
        self.extensions.iter().any(|e| {
            e.entry.enabled
                && e.run.as_ref().is_some_and(|r| {
                    r.number == session && !r.stopping && r.devices.is_current(device, epoch)
                })
        })
    }

    /// Counts changes to [`Host::commands`].
    pub(in crate::app) fn revision(&self) -> u64 {
        self.revision
    }

    /// The commands of the enabled extensions, as the registry knows them
    /// (`<extension>/<command>`).
    pub(in crate::app) fn commands(&self) -> Vec<&'static Dynamic> {
        let mut out = Vec::new();
        for e in self.extensions.iter().filter(|e| e.entry.enabled) {
            for c in &e.manifest.commands {
                let key = c
                    .key
                    .as_deref()
                    .and_then(|k| crate::app::shell::keymap::Chord::parse(k).ok());
                out.push(registry::extension_command(
                    &format!("{}/{}", e.id(), c.id),
                    &c.title,
                    key,
                ));
            }
        }
        out
    }

    /// Reads `extensions.json` and the installed extensions again, so a change
    /// the CLI made shows: extensions added are loaded, removed ones stopped
    /// and dropped, and a new active version, enabled state or setting is
    /// taken up. A program whose version or folder changed is stopped, to
    /// start again as the new one.
    pub(in crate::app) fn reload(&mut self) {
        let Some(store) = self.store.clone() else {
            return;
        };
        if let Err(why) = keep_midi_for_its_devices(&store) {
            self.errors.push(why);
        }
        let state = match State::read(store.data_dir()) {
            Ok(state) => state,
            Err(e) => {
                self.errors.push(e.to_string());
                return;
            }
        };
        let mut found: Vec<Found> = Vec::new();
        for bundled in
            rawmakase_extensions::bundled::discover(BUNDLED, &self.search_dirs).into_iter()
        {
            match bundled {
                Ok(mut b) => {
                    if let Some(launch) = self.bundled_launch(&b.manifest.extension.id) {
                        b.program = Some(launch.program.clone());
                    }
                    let problem = b.problem();
                    let dir = b
                        .program
                        .as_ref()
                        .and_then(|p| p.parent().map(Path::to_path_buf))
                        .unwrap_or_default();
                    found.push((b.manifest, Origin::Bundled, dir, b.program, problem));
                }
                Err(e) => self.errors.push(e.to_string()),
            }
        }
        for (id, entry) in &state.extensions {
            let Some(version) = &entry.active else {
                continue;
            };
            let dir = store.version_dir(id, version);
            match rawmakase_extensions::read_manifest(&dir) {
                Ok(manifest) => {
                    let problem =
                        rawmakase_extensions::check_compatible(&manifest, &self.app).err();
                    let program = manifest
                        .program
                        .as_ref()
                        .map(|p| rawmakase_extensions::program_file(&dir, p));
                    found.push((
                        manifest,
                        Origin::Installed,
                        dir,
                        program,
                        problem.map(|e| e.to_string()),
                    ));
                }
                Err(e) => self.errors.push(e.to_string()),
            }
        }
        for (id, developer) in &state.developer {
            match rawmakase_extensions::read_manifest(&developer.path) {
                Ok(manifest) if manifest.extension.id == *id => {
                    let program = manifest
                        .program
                        .as_ref()
                        .map(|p| rawmakase_extensions::program_file(&developer.path, p));
                    let problem = program
                        .as_ref()
                        .filter(|p| !p.is_file() && !self.overridden())
                        .map(|p| format!("{} isn't built yet", p.display()));
                    found.push((
                        manifest,
                        Origin::Developer,
                        developer.path.clone(),
                        program,
                        problem,
                    ));
                }
                // Kept, with what is wrong as its problem: its page says so,
                // the folder is still watched, and it loads again once fixed.
                broken => {
                    let why = match broken {
                        Ok(manifest) => format!(
                            "{} now holds {}, not {id}",
                            developer.path.display(),
                            manifest.extension.id
                        ),
                        Err(e) => e.to_string(),
                    };
                    let name = self.get(id).map_or(id.as_str(), Extension::name);
                    found.push((
                        placeholder(id, name),
                        Origin::Developer,
                        developer.path.clone(),
                        None,
                        Some(why),
                    ));
                }
            }
        }
        // Later sources replace earlier ones of the same id: a developer
        // folder over an installed version over the bundled one.
        let mut wanted: Vec<Found> = Vec::new();
        for f in found {
            wanted.retain(|w| w.0.extension.id != f.0.extension.id);
            wanted.push(f);
        }
        let mut changed = false;
        let mut kept = Vec::new();
        for (manifest, origin, dir, program, problem) in wanted {
            let id = manifest.extension.id.clone();
            let mut entry = state
                .extensions
                .get(&id)
                .cloned()
                .unwrap_or_else(|| Entry::new(&id));
            if origin == Origin::Developer {
                entry.enabled = state.developer.get(&id).is_none_or(|d| d.enabled);
            }
            let existing = self
                .extensions
                .iter()
                .position(|e| e.id() == id)
                .map(|i| self.extensions.remove(i));
            let extension = match existing {
                Some(mut e) if e.dir == dir && e.manifest == manifest && e.origin == origin => {
                    if e.entry.enabled != entry.enabled {
                        changed = true;
                        if !entry.enabled {
                            Self::end(&mut e, ErrorCode::Disabled);
                        }
                    }
                    let before = e.settings();
                    e.entry = entry;
                    e.problem = problem;
                    e.program = program;
                    let after = e.settings();
                    for (setting, value) in &after {
                        if before.get(setting) != Some(value) {
                            Self::setting_event(&mut e, setting, value);
                        }
                    }
                    e
                }
                existing => {
                    let ledger = self.ledgers.entry(id.clone()).or_default().clone();
                    let lifecycle = self.lifecycles.entry(id).or_default().clone();
                    if let Some(mut old) = existing {
                        Self::end(&mut old, ErrorCode::Cancelled);
                    }
                    changed = true;
                    let mtimes = mtimes(&dir, program.as_deref());
                    Extension {
                        manifest,
                        origin,
                        dir,
                        program,
                        problem,
                        entry,
                        run: None,
                        ledger,
                        lifecycle,
                        crashes: 0,
                        failed: false,
                        errors: VecDeque::new(),
                        status: None,
                        panels: BTreeMap::new(),
                        setups: BTreeMap::new(),
                        mask_progress: None,
                        reading: Vec::new(),
                        last_active: Instant::now(),
                        mtimes,
                    }
                }
            };
            kept.push(extension);
        }
        // What is left was removed meanwhile.
        for mut gone in std::mem::take(&mut self.extensions) {
            Self::end(&mut gone, ErrorCode::Cancelled);
            changed = true;
        }
        kept.sort_by_key(|e| e.name().to_lowercase());
        self.extensions = kept;
        if changed {
            self.revision += 1;
        }
    }

    #[cfg(test)]
    fn overridden(&self) -> bool {
        self.program_override.is_some()
    }
    #[cfg(not(test))]
    fn overridden(&self) -> bool {
        false
    }

    /// Reloads the developer extensions whose manifest or program changed on
    /// disk since they were loaded; Preferences calls it every second while
    /// its page is open.
    pub(in crate::app) fn reload_changed_folders(&mut self) {
        let changed = self.extensions.iter().any(|e| {
            e.origin == Origin::Developer && mtimes(&e.dir, e.program.as_deref()) != e.mtimes
        });
        if changed {
            for e in &mut self.extensions {
                if e.origin == Origin::Developer && mtimes(&e.dir, e.program.as_deref()) != e.mtimes
                {
                    Self::end(e, ErrorCode::Cancelled);
                    e.mtimes = mtimes(&e.dir, e.program.as_deref());
                    // Taken for a new extension by `reload`.
                    e.dir = PathBuf::new();
                }
            }
            self.reload();
        }
    }

    /// Ends `e`'s program now, if it runs, without counting a crash: its
    /// requests fail with `code` and are cancelled at the program, and its
    /// status and panels clear. Dropping the session ends its unit at once.
    /// Its lifecycle moves on, whether it ran or not, so work started for it
    /// before is void.
    fn end(e: &mut Extension, code: ErrorCode) -> Vec<Outcome> {
        e.lifecycle.fetch_add(1, Ordering::SeqCst);
        let mut outcomes = Self::stop_reading(e, &code);
        if let Some(mut run) = e.run.take() {
            let mut ids: Vec<u64> = run.requests.keys().copied().collect();
            ids.sort_unstable();
            let pending = ids.into_iter().filter_map(|id| {
                run.session.cancel(id);
                run.requests.remove(&id)
            });
            let queued = run.queued.drain(..).map(|q| q.pending);
            for pending in pending.collect::<Vec<_>>().into_iter().chain(queued) {
                outcomes.extend(Self::stopped(e, pending, &code));
            }
            run.stopping = true;
            drop(run);
        }
        e.status = None;
        e.panels.clear();
        outcomes
    }

    /// What the editor is told of `e`'s request `pending` stopped with `code`
    /// before it was answered: commands and mask requests fail.
    fn stopped(e: &mut Extension, pending: Pending, code: &ErrorCode) -> Option<Outcome> {
        let extension = e.manifest.extension.id.clone();
        let error = |what: &str| RemoteError::new(code.clone(), format!("the {what} was stopped"));
        match pending {
            Pending::Command { command, by } => Some(Outcome::Command {
                title: command_title(&e.manifest, &command),
                extension,
                by,
                result: Err(error("command")),
            }),
            Pending::Mask { ticket } => Some(Outcome::Mask {
                extension,
                ticket,
                result: Err(error("mask request")),
                buffers: None,
            }),
            Pending::MaskSetup { generator } => {
                e.setups.remove(&generator);
                Some(Outcome::MaskSetup {
                    extension,
                    generator,
                    result: Err(error("setup")),
                })
            }
            Pending::Hello | Pending::Panel { .. } => None,
        }
    }

    /// Cancels the reads of `e`'s commands' targets: those commands are never
    /// sent, and fail with `code`.
    fn stop_reading(e: &mut Extension, code: &ErrorCode) -> Vec<Outcome> {
        std::mem::take(&mut e.reading)
            .into_iter()
            .map(|read| Outcome::Command {
                extension: e.manifest.extension.id.clone(),
                title: command_title(&e.manifest, &read.command),
                by: read.by,
                result: Err(RemoteError::new(code.clone(), "the command was stopped")),
            })
            .collect()
    }

    fn setting_event(e: &mut Extension, id: &str, value: &Value) {
        if let Some(run) = e.ready_session() {
            let (name, params) = AppEvent::SettingChanged(SettingChanged {
                id: id.to_string(),
                value: value.clone(),
            })
            .into_parts();
            let _ = run.session.event(&name, params);
        }
    }

    /// Starts `e`'s program and sends `hello`. A start that fails counts as a
    /// crash.
    fn spawn(&mut self, index: usize) -> Result<(), String> {
        let number = self.next_number;
        let (store, wake) = (self.store.clone(), self.wake.clone());
        self.extensions[index].can_run()?;
        let store = store.ok_or("Extensions are off in this editor")?;
        let id = self.extensions[index].id().to_string();
        let data_dir = store.extension_data_dir(&id);
        let cache_dir = store.data_dir().join("extension-cache").join(&id);
        for dir in [&data_dir, &cache_dir] {
            if let Err(err) = std::fs::create_dir_all(dir) {
                // A start that fails counts as a crash, so a `with-app`
                // program, started again after each, is Failed in the end.
                let why = format!("{}: {err}", dir.display());
                Self::crashed(&mut self.extensions[index], why.clone());
                return Err(why);
            }
        }
        for (_, file) in TAKEN_OVER.iter().filter(|(owner, _)| *owner == id) {
            if let Err(why) = take_over(store.data_dir(), &data_dir, file) {
                let e = &mut self.extensions[index];
                let why = format!("{}: {why}", e.name());
                e.error(why.clone());
                self.notices.push(why);
            }
        }
        #[cfg(test)]
        let test_launch = match self.extensions[index].origin {
            Origin::Bundled => self.bundled_launch(&id).cloned(),
            _ => self.program_override.clone(),
        };
        let e = &mut self.extensions[index];
        let program = e.program.clone().ok_or("no program")?;
        let mut launch = Launch {
            program,
            args: Vec::new(),
            dir: e.dir.clone(),
            env: Vec::new(),
            log: data_dir.join("log.txt"),
        };
        #[cfg(test)]
        if let Some(program) = test_launch {
            launch.program = program.program;
            launch.args = program.args;
            launch.env = program.env;
        }
        // After the tests' replacement above, which would drop it: the program
        // serves the protocol of the API its manifest declares.
        launch.set_api(e.manifest.extension.api);
        if e.origin == Origin::Bundled {
            pass_runtime_dirs(
                &mut launch,
                &rawmakase_inference::runtime::runtime_search_dirs(),
            );
        }
        let devices = std::sync::Arc::new(Devices::default());
        let adapters: Vec<String> = e
            .manifest
            .control_adapters
            .iter()
            .filter(|a| a.launch == AdapterLaunch::App)
            .map(|a| a.id.clone())
            .collect();
        let lifecycle = (!adapters.is_empty()).then(|| -> session::Lifecycle {
            let devices = devices.clone();
            std::sync::Arc::new(move |method: &str, params: &Value| {
                if !ProgramRequest::is_lifecycle(method) {
                    return None;
                }
                Some(match ProgramRequest::parse(method, params.clone()) {
                    Ok(ProgramRequest::AdapterDevice(report))
                        if adapters.contains(&report.adapter) =>
                    {
                        devices.apply(&report).map(|()| Value::Null)
                    }
                    Ok(ProgramRequest::AdapterDevice(report)) => Err(RemoteError::new(
                        ErrorCode::InvalidRequest,
                        format!(
                            "the manifest declares no control adapter {}",
                            report.adapter
                        ),
                    )),
                    Ok(_) => Err(RemoteError::new(
                        ErrorCode::InvalidRequest,
                        format!("{method} is not a lifecycle request"),
                    )),
                    Err(err) => Err(err),
                })
            })
        });
        let options = SessionOptions {
            wake,
            lifecycle,
            ledger: e.ledger.clone(),
            #[cfg(test)]
            queue: self.queue,
            ..SessionOptions::default()
        };
        let mut session = match Session::spawn(&launch, options) {
            Ok(session) => session,
            Err(err) => {
                let why = format!("{} didn't start: {err}", e.name());
                Self::crashed(e, why.clone());
                return Err(why);
            }
        };
        self.next_number += 1;
        self.killers.push(session.killer());
        let hello = AppRequest::Hello(Hello {
            protocol: PROTOCOL,
            app_version: self.app.to_string(),
            extension: id,
            settings: e.settings(),
            contributions: e.manifest.contributions(),
            data_dir,
            cache_dir,
            session: number,
        });
        let (method, params) = hello.into_parts();
        let now = Instant::now();
        let mut requests = HashMap::new();
        match session.request(&method, params, now + session::HELLO_DEADLINE) {
            Ok(id) => {
                requests.insert(id, Pending::Hello);
            }
            Err(err) => {
                let why = format!("{} didn't start: {err}", e.name());
                Self::crashed(e, why.clone());
                return Err(why);
            }
        }
        e.run = Some(Running {
            session,
            ready: false,
            queued: Vec::new(),
            requests,
            started: now,
            stopping: false,
            deferred: Vec::new(),
            number,
            devices,
            scopes: BTreeMap::new(),
        });
        e.last_active = now;
        Ok(())
    }

    /// Notes an unexpected end, Failed at the [`CRASHES`]th in a row.
    fn crashed(e: &mut Extension, why: String) {
        e.crashes += 1;
        e.error(why);
        e.status = None;
        e.panels.clear();
        if e.crashes >= CRASHES {
            e.failed = true;
            e.error(format!(
                "{} stopped working after {CRASHES} crashes in a row",
                e.name()
            ));
        }
    }

    /// Sends `request` to extension `id`, starting its program first when it
    /// isn't running.
    fn send(
        &mut self,
        id: &str,
        request: AppRequest,
        pending: Pending,
        buffers: Option<Buffers>,
    ) -> Result<(), String> {
        if self.closed {
            return Err("RAWmakase is quitting".into());
        }
        let index = self.index(id).ok_or_else(|| format!("No extension {id}"))?;
        self.extensions[index].can_run()?;
        if self.extensions[index].run.is_none() {
            self.spawn(index)?;
        }
        let e = &mut self.extensions[index];
        e.last_active = Instant::now();
        let run = e.run.as_mut().expect("started above");
        if !run.ready {
            run.queued.push(Queued {
                request,
                pending,
                buffers,
            });
            return Ok(());
        }
        let (method, params) = request.into_parts();
        let deadline = Instant::now()
            + match pending {
                Pending::Mask { .. } => mask_generator::GENERATE_DEADLINE,
                Pending::MaskSetup { .. } => mask_generator::SETUP_DEADLINE,
                _ => self.request_deadline,
            };
        let request = match buffers {
            Some(buffers) => run.session.request_with(&method, params, buffers, deadline),
            None => run.session.request(&method, params, deadline),
        }
        .map_err(|err| err.to_string())?;
        run.requests.insert(request, pending);
        Ok(())
    }

    /// Runs command `command` of extension `id` on `target`, with the viewer
    /// input collected for it: input of the kind its manifest declares, or
    /// none for `none`, checked before it is sent ([`ViewerInput::validate`]).
    pub(in crate::app) fn run_command(
        &mut self,
        id: &str,
        command: &str,
        target: Option<Snapshot>,
        input: Option<ViewerInput>,
        by: RunBy,
    ) -> Result<(), String> {
        let e = self.get(id).ok_or_else(|| format!("No extension {id}"))?;
        let declared = e
            .manifest
            .commands
            .iter()
            .find(|c| c.id == command)
            .ok_or_else(|| format!("{} has no command {command}", e.name()))?
            .input;
        let given = input.as_ref().map_or(Input::None, ViewerInput::kind);
        if given != declared {
            return Err(format!("{command} takes {declared:?} input, not {given:?}"));
        }
        let input = match input {
            Some(input) => {
                input.validate().map_err(|e| e.message)?;
                serde_json::to_value(input).expect("viewer input is JSON")
            }
            None => Value::Null,
        };
        let request = AppRequest::CommandRun(CommandRun {
            command: command.to_string(),
            target,
            input,
        });
        let pending = Pending::Command {
            command: command.to_string(),
            by,
        };
        self.send(id, request, pending, None)
    }

    /// Opens panel `panel` of extension `id`: its program answers with its
    /// controls.
    pub(in crate::app) fn open_panel(&mut self, id: &str, panel: &str) -> Result<(), String> {
        self.panel_request(
            id,
            panel,
            AppRequest::PanelOpen(PanelOpen {
                panel: panel.to_string(),
            }),
        )
    }
    /// The user committed `value` in control `control` of a panel.
    pub(in crate::app) fn panel_changed(
        &mut self,
        id: &str,
        panel: &str,
        control: &str,
        value: Value,
    ) -> Result<(), String> {
        self.panel_request(
            id,
            panel,
            AppRequest::PanelChanged(PanelChanged {
                panel: panel.to_string(),
                control: control.to_string(),
                value,
            }),
        )
    }
    /// The user clicked button `control` of a panel.
    pub(in crate::app) fn panel_clicked(
        &mut self,
        id: &str,
        panel: &str,
        control: &str,
    ) -> Result<(), String> {
        self.panel_request(
            id,
            panel,
            AppRequest::PanelClicked(PanelClicked {
                panel: panel.to_string(),
                control: control.to_string(),
            }),
        )
    }
    fn panel_request(&mut self, id: &str, panel: &str, request: AppRequest) -> Result<(), String> {
        let result = self.send(
            id,
            request,
            Pending::Panel {
                panel: panel.to_string(),
            },
            None,
        );
        if let Some(index) = self.index(id) {
            let view = self.extensions[index]
                .panels
                .entry(panel.to_string())
                .or_default();
            view.opened = true;
            match &result {
                Ok(()) => view.waiting = true,
                Err(why) => view.error = Some(why.clone()),
            }
        }
        result
    }

    /// Turns extension `id` on or off, in `extensions.json` first. Off, it
    /// admits no request, its requests are cancelled, its program is stopped
    /// and its commands and panels go.
    pub(in crate::app) fn set_enabled(
        &mut self,
        id: &str,
        enabled: bool,
    ) -> Result<Vec<Outcome>, String> {
        let store = self
            .store
            .clone()
            .ok_or("Extensions are off in this editor")?;
        let index = self.index(id).ok_or_else(|| format!("No extension {id}"))?;
        // A bundled extension has no install for the store to find.
        let bundled: &[&str] = if self.extensions[index].origin == Origin::Bundled {
            &[id]
        } else {
            &[]
        };
        store
            .set_enabled(id, enabled, bundled)
            .map_err(|e| format!("{} not changed: {e}", self.extensions[index].name()))?;
        let e = &mut self.extensions[index];
        e.entry.enabled = enabled;
        let outcomes = if enabled {
            Vec::new()
        } else {
            Self::end(e, ErrorCode::Disabled)
        };
        self.revision += 1;
        Ok(outcomes)
    }

    /// Stops extension `id`'s commands in flight: each is cancelled at the
    /// program and settled as cancelled now, so a proposal it sends later is
    /// refused.
    pub(in crate::app) fn cancel_commands(&mut self, id: &str) -> Vec<Outcome> {
        let Some(index) = self.index(id) else {
            return Vec::new();
        };
        let e = &mut self.extensions[index];
        let mut outcomes = Self::stop_reading(e, &ErrorCode::Cancelled);
        let Some(run) = &mut e.run else {
            return outcomes;
        };
        let mut ids: Vec<u64> = run
            .requests
            .iter()
            .filter(|(_, p)| matches!(p, Pending::Command { .. }))
            .map(|(id, _)| *id)
            .collect();
        ids.sort_unstable();
        for op in ids {
            run.session.cancel(op);
            if let Some(Pending::Command { command, by }) = run.requests.remove(&op) {
                outcomes.push(Outcome::Command {
                    extension: e.manifest.extension.id.clone(),
                    title: command_title(&e.manifest, &command),
                    by,
                    result: Err(RemoteError::new(ErrorCode::Cancelled, "stopped")),
                });
            }
        }
        Self::sweep(e);
        let Some(run) = &mut e.run else {
            return outcomes;
        };
        let (commands, rest): (Vec<Queued>, Vec<Queued>) = std::mem::take(&mut run.queued)
            .into_iter()
            .partition(|q| matches!(q.pending, Pending::Command { .. }));
        run.queued = rest;
        for queued in commands {
            if let Pending::Command { command, by } = queued.pending {
                outcomes.push(Outcome::Command {
                    extension: e.manifest.extension.id.clone(),
                    title: command_title(&e.manifest, &command),
                    by,
                    result: Err(RemoteError::new(ErrorCode::Cancelled, "stopped")),
                });
            }
        }
        outcomes
    }
    /// Where extension `id` is in its lifecycle: work for it captures this,
    /// and is void once it differs (the extension was disabled, uninstalled,
    /// loaded again, or its program crashed or hung meanwhile). `None` once
    /// it isn't loaded at all.
    pub(in crate::app) fn lifecycle(&self, id: &str) -> Option<u64> {
        self.get(id).map(|e| e.lifecycle.load(Ordering::SeqCst))
    }
    /// Why extension `id` can't take a request now, if it can't.
    pub(in crate::app) fn ready(&self, id: &str) -> Result<(), String> {
        if self.closed {
            return Err("RAWmakase is quitting".into());
        }
        self.get(id)
            .ok_or_else(|| format!("No extension {id}"))?
            .can_run()
    }
    /// Whether extension `id` has a command in flight.
    pub(in crate::app) fn command_running(&self, id: &str) -> bool {
        let Some(e) = self.get(id) else { return false };
        !e.reading.is_empty()
            || e.run.as_ref().is_some_and(|r| {
                r.requests
                    .values()
                    .chain(r.queued.iter().map(|q| &q.pending))
                    .any(|p| matches!(p, Pending::Command { .. }))
            })
    }

    /// The commands of extension `id` sent and not answered, or waiting for
    /// its handshake, for tests.
    #[cfg(test)]
    pub(in crate::app) fn commands_in_flight(&self, id: &str) -> Vec<String> {
        let Some(run) = self.get(id).and_then(|e| e.run.as_ref()) else {
            return Vec::new();
        };
        let mut commands: Vec<String> = run
            .requests
            .values()
            .chain(run.queued.iter().map(|q| &q.pending))
            .filter_map(|p| match p {
                Pending::Command { command, .. } => Some(command.clone()),
                _ => None,
            })
            .collect();
        commands.sort();
        commands
    }

    /// Notes that command `command` of extension `id` waits for its target,
    /// read on a worker, to be sent with `input`; the ticket that read
    /// reports with ([`Host::finish_read`]), and its cancel.
    pub(in crate::app) fn read_for_command(
        &mut self,
        id: &str,
        command: &str,
        input: Option<ViewerInput>,
        by: RunBy,
    ) -> Result<(u64, Arc<AtomicBool>), String> {
        if self.closed {
            return Err("RAWmakase is quitting".into());
        }
        let index = self.index(id).ok_or_else(|| format!("No extension {id}"))?;
        let e = &self.extensions[index];
        if !e.manifest.commands.iter().any(|c| c.id == command) {
            return Err(format!("{} has no command {command}", e.name()));
        }
        e.can_run()?;
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let cancel = Arc::new(AtomicBool::new(false));
        let e = &mut self.extensions[index];
        e.reading.push(TargetRead {
            ticket,
            command: command.to_string(),
            by,
            input,
            cancel: cancel.clone(),
        });
        e.last_active = Instant::now();
        Ok((ticket, cancel))
    }

    /// The read `ticket` finished with `result`: a program's read is
    /// answered, if the session that asked still runs; a command's target
    /// is sent with its command, if that command wasn't stopped meanwhile.
    /// What came of a command that can't be sent.
    pub(in crate::app) fn finish_read(
        &mut self,
        ticket: u64,
        result: Result<Snapshot, RemoteError>,
    ) -> Vec<Outcome> {
        for index in 0..self.extensions.len() {
            let e = &mut self.extensions[index];
            if let Some(run) = e.run.as_mut()
                && let Some(at) = run.deferred.iter().position(|d| d.ticket == ticket)
            {
                let deferred = run.deferred.remove(at);
                let reply = result.map(|s| serde_json::to_value(s).expect("a snapshot is JSON"));
                let _ = run.session.reply(deferred.request, reply);
                return Vec::new();
            }
            let Some(at) = e.reading.iter().position(|r| r.ticket == ticket) else {
                continue;
            };
            let mut read = e.reading.remove(at);
            let id = e.manifest.extension.id.clone();
            let title = command_title(&e.manifest, &read.command);
            let failed = |err: RemoteError| {
                vec![Outcome::Command {
                    extension: id.clone(),
                    title: title.clone(),
                    by: read.by,
                    result: Err(err),
                }]
            };
            return match result {
                Err(err) => failed(err),
                Ok(target) => match self.run_command(
                    &id,
                    &read.command,
                    Some(target),
                    read.input.take(),
                    read.by,
                ) {
                    Ok(()) => Vec::new(),
                    Err(why) => failed(RemoteError::new(ErrorCode::Exited, why)),
                },
            };
        }
        // Its session ended, or its command was stopped: no one is waiting.
        Vec::new()
    }

    /// Whether a command's target is being read, for tests.
    #[cfg(test)]
    pub(in crate::app) fn reading(&self) -> bool {
        self.extensions.iter().any(|e| !e.reading.is_empty())
    }

    /// Clears a Failed extension's crashes, so it may start again.
    pub(in crate::app) fn retry(&mut self, id: &str) {
        if let Some(index) = self.index(id) {
            let e = &mut self.extensions[index];
            e.failed = false;
            e.crashes = 0;
            e.errors.clear();
        }
    }

    /// Uninstalls extension `id` (not a bundled one): its entry, then its
    /// folders. A developer folder is only forgotten.
    pub(in crate::app) fn uninstall(&mut self, id: &str) -> Result<Vec<Outcome>, String> {
        let store = self
            .store
            .clone()
            .ok_or("Extensions are off in this editor")?;
        let index = self.index(id).ok_or_else(|| format!("No extension {id}"))?;
        let name = self.extensions[index].name().to_string();
        match self.extensions[index].origin {
            Origin::Bundled => return Err(format!("{name} comes with RAWmakase")),
            Origin::Installed => store.remove(id).map_err(|e| format!("{name}: {e}"))?,
            Origin::Developer => State::update(store.data_dir(), |state| {
                state.developer.remove(id);
                Ok(())
            })
            .map_err(|e| format!("{name}: {e}"))?,
        }
        let mut e = self.extensions.remove(index);
        let outcomes = Self::end(&mut e, ErrorCode::Cancelled);
        self.revision += 1;
        // Another source of the same id (the bundled one under an installed
        // version) shows again.
        self.reload();
        Ok(outcomes)
    }

    /// Sets setting `setting` of extension `id` to `value`, in
    /// `extensions.json` first, and tells its program.
    pub(in crate::app) fn set_setting(
        &mut self,
        id: &str,
        setting: &str,
        value: Value,
    ) -> Result<(), String> {
        let store = self
            .store
            .clone()
            .ok_or("Extensions are off in this editor")?;
        let index = self.index(id).ok_or_else(|| format!("No extension {id}"))?;
        let e = &self.extensions[index];
        let declared = e
            .manifest
            .settings
            .iter()
            .find(|s| s.id == setting)
            .ok_or_else(|| format!("{} has no setting {setting}", e.name()))?;
        if !declared.control.accepts(&value) {
            return Err(format!("{value} doesn't fit {setting}"));
        }
        State::update(store.data_dir(), |state| {
            state
                .entry(id)
                .settings
                .insert(setting.to_string(), value.clone());
            Ok(())
        })
        .map_err(|err| format!("{} not changed: {err}", e.name()))?;
        let e = &mut self.extensions[index];
        e.entry.settings.insert(setting.to_string(), value.clone());
        Self::setting_event(e, setting, &value);
        Ok(())
    }

    /// Registers a developer folder and loads it.
    pub(in crate::app) fn load_folder(&mut self, folder: &Path) -> Result<String, String> {
        let store = self
            .store
            .clone()
            .ok_or("Extensions are off in this editor")?;
        let manifest = store.add_developer(folder).map_err(|e| e.to_string())?;
        self.reload();
        Ok(manifest.extension.name)
    }

    /// Sends event `event` to every ready program.
    pub(in crate::app) fn broadcast(&mut self, event: &AppEvent) {
        if self.closed {
            return;
        }
        let (name, params) = event.clone().into_parts();
        for e in &mut self.extensions {
            if let Some(run) = e.ready_session() {
                let _ = run.session.event(&name, params.clone());
            }
        }
    }

    /// The catalog open in the editor changed (opened, closed or another
    /// one): sources' scopes follow it.
    pub(in crate::app) fn set_catalog(&mut self, catalog: Option<sources::OpenCatalog>) {
        if self.catalog != catalog {
            self.catalog = catalog;
            self.sync_sources();
        }
    }

    /// Starts and stops sources' scopes so that each ready program has one
    /// for each of its sources turned on for the open catalog, and none
    /// other: a scope of another catalog, or of a source turned off, stops
    /// (reports in it being added are withdrawn), and a new one starts with
    /// a number never used before.
    pub(in crate::app) fn sync_sources(&mut self) {
        if self.closed {
            return;
        }
        let catalog = self.catalog.clone();
        let mut next = self.next_scope;
        for e in &mut self.extensions {
            if e.manifest.sources.is_empty() {
                continue;
            }
            let wanted: Vec<String> = match &catalog {
                Some(c) if e.entry.enabled => e
                    .manifest
                    .sources
                    .iter()
                    .filter(|s| e.entry.source_on(&s.id, &c.key))
                    .map(|s| s.id.clone())
                    .collect(),
                _ => Vec::new(),
            };
            let Some(run) = e.ready_session() else {
                continue;
            };
            let stale: Vec<String> = run
                .scopes
                .iter()
                .filter(|(id, (_, key))| {
                    !wanted.contains(id) || catalog.as_ref().map(|c| &c.key) != Some(key)
                })
                .map(|(id, _)| id.clone())
                .collect();
            let changed = !stale.is_empty();
            for source in stale {
                if let Some((scope, _)) = run.scopes.remove(&source) {
                    let (name, params) =
                        AppEvent::SourceStopped(SourceStopped { source, scope }).into_parts();
                    let _ = run.session.event(&name, params);
                }
            }
            if let Some(c) = &catalog {
                for source in wanted {
                    if run.scopes.contains_key(&source) {
                        continue;
                    }
                    let scope = next;
                    next += 1;
                    run.scopes.insert(source.clone(), (scope, c.key.clone()));
                    let (name, params) = AppEvent::SourceStarted(SourceStarted {
                        source,
                        scope,
                        catalog: c.name.clone(),
                    })
                    .into_parts();
                    let _ = run.session.event(&name, params);
                }
            }
            if changed {
                Self::sweep(e);
            }
        }
        self.next_scope = next;
    }

    /// Whether source `source` of extension `id` adds photos to the open
    /// catalog; none when no catalog is open.
    pub(in crate::app) fn source_on(&self, id: &str, source: &str) -> Option<bool> {
        let catalog = self.catalog.as_ref()?;
        let e = self.get(id)?;
        Some(e.entry.source_on(source, &catalog.key))
    }

    /// Turns source `source` of extension `id` on or off for the open
    /// catalog, in `extensions.json` first; its scope starts or stops.
    pub(in crate::app) fn set_source_on(
        &mut self,
        id: &str,
        source: &str,
        on: bool,
    ) -> Result<(), String> {
        let store = self
            .store
            .clone()
            .ok_or("Extensions are off in this editor")?;
        let catalog = self
            .catalog
            .clone()
            .ok_or("Open a catalog to add photos to")?;
        let index = self.index(id).ok_or_else(|| format!("No extension {id}"))?;
        let e = &self.extensions[index];
        if !e.manifest.sources.iter().any(|s| s.id == source) {
            return Err(format!("{} has no source {source}", e.name()));
        }
        State::update(store.data_dir(), |state| {
            state.entry(id).set_source_on(source, &catalog.key, on);
            Ok(())
        })
        .map_err(|err| format!("{} not changed: {err}", e.name()))?;
        self.extensions[index]
            .entry
            .set_source_on(source, &catalog.key, on);
        self.sync_sources();
        Ok(())
    }

    /// Whether `op`, an app request of extension `id`'s current session,
    /// is a `command.run` still in flight and admitted.
    fn admitted(e: &Extension, op: u64) -> bool {
        e.run.as_ref().is_some_and(|r| {
            !r.stopping
                && matches!(r.requests.get(&op), Some(Pending::Command { .. }))
                && r.session.is_admitted(op)
        })
    }

    /// What arrived from every program, as of `now`: replies settle requests,
    /// the programs' requests are answered (by `answer` for those the editor
    /// answers), deadlines and crashes are enforced, `with-app` programs start
    /// after the first frame and idle `on-demand` ones stop. Returns the next
    /// time to poll, when one is due.
    pub(in crate::app) fn poll(
        &mut self,
        now: Instant,
        answer: &mut dyn FnMut(Call) -> Answer,
        outcomes: &mut Vec<Outcome>,
    ) -> Option<Instant> {
        if self.closed || self.store.is_none() {
            return None;
        }
        self.polls += 1;
        outcomes.extend(self.notices.drain(..).map(|text| Outcome::Message {
            level: Level::Warning,
            text,
        }));
        // After the first frame: the window shows before any program starts.
        // A `with-app` program that ended starts again, until it is Failed.
        if self.polls >= 2 {
            for index in 0..self.extensions.len() {
                let e = &self.extensions[index];
                let with_app = e
                    .manifest
                    .program
                    .as_ref()
                    .is_some_and(|p| p.start == Start::WithApp)
                    && e.manifest.runs_in_app();
                if with_app
                    && e.can_run().is_ok()
                    && e.run.is_none()
                    && let Err(why) = self.spawn(index)
                {
                    self.errors.push(why);
                }
            }
        }
        self.sync_sources();
        let mut next: Option<Instant> = None;
        for index in 0..self.extensions.len() {
            self.poll_one(index, now, answer, outcomes);
            let e = &mut self.extensions[index];
            let Some(run) = &e.run else {
                continue;
            };
            if run.started + CALM <= now && e.crashes > 0 {
                e.crashes = 0;
            }
            let on_demand = e
                .manifest
                .program
                .as_ref()
                .is_none_or(|p| p.start == Start::OnDemand);
            if on_demand && !e.busy() && e.last_active + IDLE_STOP <= now {
                if let Some(run) = e.run.take() {
                    // Asked to exit, and ended by its stopper thread within
                    // `QUIT_GRACE` if it doesn't; nothing waits for it.
                    let _ = run.session.stop(now + session::QUIT_GRACE * 2);
                }
                e.status = None;
                e.panels.clear();
                continue;
            }
            let due = run.session.next_deadline();
            let idle = on_demand.then_some(e.last_active + IDLE_STOP);
            for at in [due, idle].into_iter().flatten() {
                next = Some(next.map_or(at, |n: Instant| n.min(at)));
            }
        }
        // Programs that finished their handshake in this poll get their
        // scopes now, not a frame later.
        self.sync_sources();
        self.killers.retain(|k| !k.has_ended());
        next
    }

    fn poll_one(
        &mut self,
        index: usize,
        now: Instant,
        answer: &mut dyn FnMut(Call) -> Answer,
        outcomes: &mut Vec<Outcome>,
    ) {
        let Some(run) = self.extensions[index].run.as_mut() else {
            return;
        };
        let number = run.number;
        let updates = run.session.poll(now);
        // The session's poll is where a deadline passes.
        Self::sweep(&mut self.extensions[index]);
        for update in updates {
            let e = &mut self.extensions[index];
            // The rest belongs to a run that ended on an earlier update (a
            // failed handshake, `Closed`, `Hung`): it was settled and counted
            // then, once, and nothing more of it applies.
            if e.run.as_ref().map(|r| r.number) != Some(number) {
                break;
            }
            match update {
                Update::Reply {
                    id,
                    result,
                    buffers,
                    ..
                } => {
                    let Some(run) = e.run.as_mut() else { break };
                    let Some(pending) = run.requests.remove(&id) else {
                        continue;
                    };
                    // An answered command admits no more of its proposals.
                    Self::sweep(e);
                    self.settle(index, pending, result, buffers, outcomes);
                }
                Update::Request { id, method, params } => {
                    let answered = match ProgramRequest::parse(&method, params) {
                        Err(err) => Answer::Now(Err(err)),
                        Ok(request) => self.program_request(index, id, request, answer, outcomes),
                    };
                    if let (Answer::Now(result), Some(run)) =
                        (answered, self.extensions[index].run.as_ref())
                    {
                        let _ = run.session.reply(id, result);
                    }
                }
                Update::Event { name, params } => match ProgramEvent::parse(&name, params) {
                    Ok(ProgramEvent::PanelUpdate(update)) => {
                        if e.manifest.panels.iter().any(|p| p.id == update.panel) {
                            let view = e.panels.entry(update.panel).or_default();
                            panels::take(view, update.controls);
                        }
                    }
                    Err(err) => e.error(format!("{}: {err}", e.manifest.extension.name)),
                },
                Update::Malformed { reason } => {
                    let name = e.manifest.extension.name.clone();
                    e.error(format!("{name} sent a line that isn't a message: {reason}"));
                }
                // The program gave up on a request answered later: its work
                // stops, unless it is writing.
                Update::Cancel { id } => {
                    let Some(run) = e.run.as_mut() else { break };
                    if let Some(ticket) = run
                        .deferred
                        .iter()
                        .find(|d| d.request == id)
                        .map(|d| d.ticket)
                    {
                        Self::withdraw(
                            run,
                            ticket,
                            RemoteError::new(ErrorCode::Cancelled, "cancelled by the extension"),
                        );
                    }
                }
                Update::Progress { id, done, text } => {
                    match e.run.as_ref().and_then(|r| r.requests.get(&id)) {
                        Some(Pending::MaskSetup { generator }) => {
                            if let Some(mask_generator::Setup::Running { done: d, text: t }) =
                                e.setups.get_mut(generator)
                            {
                                (*d, *t) = (done, text);
                            }
                        }
                        Some(Pending::Mask { ticket }) => {
                            e.mask_progress = text.map(|text| (*ticket, text));
                        }
                        _ => {}
                    }
                }
                // The same terminal cleanup as `Hung`, whichever comes first:
                // what waits on the program fails with `exited` (commands
                // queued before the handshake, targets being read) and the
                // run goes, its proposals' work cancelled as the session
                // drops. Before, `Closed` left target reads to finish and
                // send their command to a program started again.
                Update::Closed => {
                    let Some(run) = e.run.as_ref() else { break };
                    let crash = (!run.stopping).then(|| {
                        let how = run
                            .session
                            .exit()
                            .map_or_else(|| "its channel closed".to_string(), |x| x.to_string());
                        format!("{} ended unexpectedly ({how})", e.name())
                    });
                    let mut stopped = Self::end(e, ErrorCode::Exited);
                    outcomes.append(&mut stopped);
                    if let Some(why) = crash {
                        Self::crashed(e, why);
                    }
                    break;
                }
                // The session killed the program and failed what it had
                // pending with `exited` (those replies came first). Whatever
                // else waits on it fails too, and it counts as a crash, as an
                // unexpected exit does; the session goes, and with it the busy
                // state. Its leases stay counted until the unit has ended.
                Update::Hung { reason } => {
                    let why = format!("{} stopped responding and was ended: {reason}", e.name());
                    let mut stopped = Self::end(e, ErrorCode::Exited);
                    outcomes.append(&mut stopped);
                    Self::crashed(e, why);
                    break;
                }
            }
        }
    }

    /// The one outcome of an app request.
    fn settle(
        &mut self,
        index: usize,
        pending: Pending,
        result: Result<Value, RemoteError>,
        buffers: Option<Returned>,
        outcomes: &mut Vec<Outcome>,
    ) {
        let e = &mut self.extensions[index];
        match pending {
            Pending::Mask { ticket } => {
                if result.is_ok() {
                    e.crashes = 0;
                }
                outcomes.push(Outcome::Mask {
                    extension: e.manifest.extension.id.clone(),
                    ticket,
                    result,
                    buffers,
                });
            }
            Pending::MaskSetup { generator } => {
                match &result {
                    Err(err) if err.code != ErrorCode::Cancelled => {
                        e.error(format!("Setting up {generator}: {err}"));
                        e.setups.insert(
                            generator.clone(),
                            mask_generator::Setup::Failed(err.message.clone()),
                        );
                    }
                    _ => {
                        e.setups.remove(&generator);
                    }
                }
                outcomes.push(Outcome::MaskSetup {
                    extension: e.manifest.extension.id.clone(),
                    generator,
                    result,
                });
            }
            Pending::Hello => match result.and_then(check_hello) {
                Ok(_) => {
                    let Some(run) = e.run.as_mut() else { return };
                    run.ready = true;
                    let queued = std::mem::take(&mut run.queued);
                    let id = e.manifest.extension.id.clone();
                    for q in queued {
                        if let Err(why) = self.send(&id, q.request, q.pending.clone(), q.buffers) {
                            self.failed_request(index, q.pending, why, outcomes);
                        }
                    }
                }
                Err(err) => {
                    let why = format!("{} didn't start: {err}", e.name());
                    let mut stopped = Self::end(e, ErrorCode::Exited);
                    outcomes.append(&mut stopped);
                    Self::crashed(e, why);
                }
            },
            Pending::Command { command, by } => {
                if result.is_ok() {
                    e.crashes = 0;
                }
                if let Err(err) = &result {
                    e.error(format!("{}: {err}", command_title(&e.manifest, &command)));
                }
                outcomes.push(Outcome::Command {
                    extension: e.manifest.extension.id.clone(),
                    title: command_title(&e.manifest, &command),
                    by,
                    result,
                });
            }
            Pending::Panel { panel } => {
                let view = e.panels.entry(panel).or_default();
                view.waiting = false;
                match result.and_then(|v| {
                    serde_json::from_value::<rawmakase_extensions::protocol::method::Controls>(v)
                        .map_err(|err| RemoteError::new(ErrorCode::InvalidRequest, err.to_string()))
                }) {
                    Ok(controls) => {
                        panels::take(view, controls.controls);
                        e.crashes = 0;
                    }
                    Err(err) => view.error = Some(err.to_string()),
                }
            }
        }
    }

    fn failed_request(
        &mut self,
        index: usize,
        pending: Pending,
        why: String,
        outcomes: &mut Vec<Outcome>,
    ) {
        let e = &mut self.extensions[index];
        match pending {
            Pending::Command { command, by } => outcomes.push(Outcome::Command {
                extension: e.manifest.extension.id.clone(),
                title: command_title(&e.manifest, &command),
                by,
                result: Err(RemoteError::new(ErrorCode::Exited, why)),
            }),
            Pending::Mask { .. } | Pending::MaskSetup { .. } => {
                outcomes.extend(Self::stopped(e, pending, &ErrorCode::Exited));
            }
            Pending::Panel { panel } => {
                let view = e.panels.entry(panel).or_default();
                view.waiting = false;
                view.error = Some(why);
            }
            Pending::Hello => {}
        }
    }

    /// Answers one of a program's requests: `notify` and `status` here, the
    /// rest through `answer`, `propose.settings` only while its `op` is
    /// admitted.
    fn program_request(
        &mut self,
        index: usize,
        id: u64,
        request: ProgramRequest,
        answer: &mut dyn FnMut(Call) -> Answer,
        outcomes: &mut Vec<Outcome>,
    ) -> Answer {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let e = &mut self.extensions[index];
        match request {
            ProgramRequest::Notify(notify) => {
                outcomes.push(Outcome::Message {
                    level: notify.level,
                    text: format!("{}: {}", e.name(), notify.text),
                });
                Answer::Now(Ok(Value::Null))
            }
            ProgramRequest::Status(status) => {
                e.status = status.text.filter(|t| !t.trim().is_empty());
                Answer::Now(Ok(Value::Null))
            }
            ProgramRequest::AdapterOperate(operate) => {
                let declared = e
                    .manifest
                    .control_adapters
                    .iter()
                    .any(|a| a.id == operate.adapter && a.launch == AdapterLaunch::App);
                let run = e.run.as_ref().filter(|r| !r.stopping && e.entry.enabled);
                let Some(run) = run.filter(|_| declared) else {
                    return Answer::Now(Err(RemoteError::new(
                        ErrorCode::Other("adapter_disabled".into()),
                        format!(
                            "{} is not an enabled control adapter of {}",
                            operate.adapter,
                            e.name()
                        ),
                    )));
                };
                let call = Call {
                    request: ProgramRequest::AdapterOperate(operate),
                    title: e.name().to_string(),
                    ticket,
                    deadline: None,
                    devices: Some((run.number, run.devices.clone())),
                };
                let result = answer(call);
                // Useful work: a device's input changed the editor.
                if let Answer::Now(Ok(v)) = &result
                    && v["applied"].as_u64().is_some_and(|n| n > 0)
                {
                    self.extensions[index].crashes = 0;
                }
                result
            }
            // Applied by the session's reader when the extension declares an
            // adapter the app launches; anything else declares none.
            ProgramRequest::AdapterDevice(_) => Answer::Now(Err(RemoteError::new(
                ErrorCode::Other("adapter_disabled".into()),
                "this extension declares no control adapter RAWmakase launches",
            ))),
            ProgramRequest::SourceReport(report) => {
                let Some(source) = e.manifest.sources.iter().find(|s| s.id == report.source) else {
                    return Answer::Now(Err(RemoteError::new(
                        ErrorCode::InvalidRequest,
                        format!("the manifest declares no source {}", report.source),
                    )));
                };
                let title = format!("{}: {}", e.name(), source.name());
                if !Self::in_scope(e, &report.source, report.scope) {
                    return Answer::Now(Err(RemoteError::new(
                        ErrorCode::NotAdmitted,
                        format!("scope {} of {title} is not running", report.scope),
                    )));
                }
                if let Err(err) = report.validate() {
                    return Answer::Now(Err(err));
                }
                let scope = (report.source.clone(), report.scope);
                let answered = answer(Call {
                    request: ProgramRequest::SourceReport(report),
                    title,
                    ticket,
                    deadline: None,
                    devices: None,
                });
                if let Answer::Adding(gate) = &answered {
                    match self.extensions[index].run.as_mut() {
                        Some(run) => run.deferred.push(Deferred {
                            ticket,
                            request: id,
                            op: None,
                            scope: Some(scope),
                            cancel: Work::Report(gate.clone()),
                        }),
                        None => {
                            gate.close();
                        }
                    }
                }
                answered
            }
            ProgramRequest::ProposeSettings(proposal) => {
                if !Self::admitted(e, proposal.op) {
                    return Answer::Now(Err(RemoteError::new(
                        ErrorCode::NotAdmitted,
                        "the command this proposal belongs to is no longer running",
                    )));
                }
                let op = proposal.op;
                let run = e.run.as_ref().expect("admitted");
                let title = match run.requests.get(&op) {
                    Some(Pending::Command { command, .. }) => command_title(&e.manifest, command),
                    _ => e.name().to_string(),
                };
                let deadline = run.session.deadline(op);
                let answered = answer(Call {
                    request: ProgramRequest::ProposeSettings(proposal),
                    title,
                    ticket,
                    deadline,
                    devices: None,
                });
                if let Answer::Later(cancel) = &answered {
                    let e = &mut self.extensions[index];
                    match e.run.as_mut() {
                        Some(run) => run.deferred.push(Deferred {
                            ticket,
                            request: id,
                            op: Some(op),
                            scope: None,
                            cancel: Work::Proposal(cancel.clone()),
                        }),
                        None => {
                            cancel.cancel();
                        }
                    }
                }
                answered
            }
            request => {
                let answered = answer(Call {
                    request,
                    title: e.name().to_string(),
                    ticket,
                    deadline: None,
                    devices: None,
                });
                if let Answer::Reading(cancel) = &answered {
                    match self.extensions[index].run.as_mut() {
                        Some(run) => run.deferred.push(Deferred {
                            ticket,
                            request: id,
                            op: None,
                            scope: None,
                            cancel: Work::Read(cancel.clone()),
                        }),
                        None => cancel.store(true, Ordering::Relaxed),
                    }
                }
                answered
            }
        }
    }

    /// Cancels the work of `e`'s proposals whose command is no longer
    /// admitted, and answers each `not_admitted` now: the program isn't left
    /// waiting for work that will write nothing.
    fn sweep(e: &mut Extension) {
        let Some(run) = e.run.as_ref() else { return };
        let gone: Vec<u64> = run
            .deferred
            .iter()
            .filter(|d| d.op.is_some_and(|op| !Self::admitted(e, op)))
            .map(|d| d.ticket)
            .collect();
        let out_of_scope: Vec<u64> = run
            .deferred
            .iter()
            .filter(|d| {
                d.scope
                    .as_ref()
                    .is_some_and(|(source, scope)| !Self::in_scope(e, source, *scope))
            })
            .map(|d| d.ticket)
            .collect();
        if let Some(run) = e.run.as_mut() {
            for ticket in gone {
                Self::withdraw(
                    run,
                    ticket,
                    RemoteError::new(
                        ErrorCode::NotAdmitted,
                        "the command this proposal belongs to stopped running",
                    ),
                );
            }
            for ticket in out_of_scope {
                Self::withdraw(
                    run,
                    ticket,
                    RemoteError::new(
                        ErrorCode::NotAdmitted,
                        "the scope this report belongs to stopped",
                    ),
                );
            }
        }
    }

    /// Whether `scope` is source `source`'s scope now, in `e`'s current
    /// session, which still admits requests.
    fn in_scope(e: &Extension, source: &str, scope: u64) -> bool {
        e.entry.enabled
            && e.run.as_ref().is_some_and(|r| {
                r.ready && !r.stopping && r.scopes.get(source).is_some_and(|(s, _)| *s == scope)
            })
    }

    /// The report `ticket`'s files were added, or not, with `result`: the
    /// program is answered, if the session that asked still runs and hasn't
    /// been answered already (its scope ended, or it cancelled the report,
    /// before the write began).
    pub(in crate::app) fn finish_report(
        &mut self,
        ticket: u64,
        result: Result<Value, RemoteError>,
    ) {
        for e in &mut self.extensions {
            let Some(run) = e.run.as_mut() else { continue };
            let Some(at) = run.deferred.iter().position(|d| d.ticket == ticket) else {
                continue;
            };
            let deferred = run.deferred.remove(at);
            #[cfg(test)]
            self.finished.push((ticket, result.clone()));
            let _ = run.session.reply(deferred.request, result);
            return;
        }
        #[cfg(test)]
        self.finished.push((ticket, result));
    }

    /// Cancels the work of the proposal `ticket` and answers the program
    /// with `refusal`, unless the work has begun its write: then it is
    /// answered when the work finishes, with what it wrote. The work's own
    /// report later finds nothing to answer.
    fn withdraw(run: &mut Running, ticket: u64, refusal: RemoteError) {
        let Some(at) = run.deferred.iter().position(|d| d.ticket == ticket) else {
            return;
        };
        if run.deferred[at].cancel.cancel() {
            let deferred = run.deferred.remove(at);
            let _ = run.session.reply(deferred.request, Err(refusal));
        }
    }

    /// The work of proposal `ticket` finished with `result`: the program is
    /// answered, if the session that asked still runs. Work that stopped
    /// because its command was no longer admitted answers `not_admitted`.
    pub(in crate::app) fn finish_proposal(
        &mut self,
        ticket: u64,
        result: Result<Value, RemoteError>,
    ) {
        let mut result = Some(result);
        for e in &mut self.extensions {
            let Some(run) = e.run.as_mut() else { continue };
            let Some(at) = run.deferred.iter().position(|d| d.ticket == ticket) else {
                continue;
            };
            let deferred = run.deferred.remove(at);
            // Past its deadline the command isn't admitted, whether or not
            // a poll has noticed yet.
            let op = deferred.op.unwrap_or_default();
            let admitted = Self::admitted(e, op)
                && e.run
                    .as_ref()
                    .and_then(|r| r.session.deadline(op))
                    .is_some_and(|d| Instant::now() < d);
            let result = match result.take().expect("answered once") {
                Err(err) if err.code == ErrorCode::Cancelled && !admitted => Err(RemoteError::new(
                    ErrorCode::NotAdmitted,
                    "the command this proposal belongs to stopped running",
                )),
                result => result,
            };
            #[cfg(test)]
            self.finished.push((ticket, result.clone()));
            if let Some(run) = e.run.as_ref() {
                let _ = run.session.reply(deferred.request, result);
            }
            return;
        }
        // Its session ended, or it was answered when its command stopped
        // being admitted or the program cancelled it: there is no one to
        // answer.
        #[cfg(test)]
        self.finished.push((ticket, result.expect("not answered")));
    }

    /// Whether a program's proposal or read is being worked on, for tests.
    #[cfg(test)]
    pub(in crate::app) fn proposing(&self) -> bool {
        self.extensions
            .iter()
            .any(|e| e.run.as_ref().is_some_and(|r| !r.deferred.is_empty()))
    }

    /// The status line's entries: each program's own, or the command it runs.
    pub(in crate::app) fn status(&self) -> Vec<String> {
        let mut out = Vec::new();
        for e in &self.extensions {
            let running = e.run.as_ref().and_then(|run| {
                run.requests
                    .values()
                    .chain(run.queued.iter().map(|q| &q.pending))
                    .find_map(|p| match p {
                        Pending::Command { command, .. } => Some(command),
                        _ => None,
                    })
            });
            if let Some(text) = e.status.as_ref().filter(|_| e.run.is_some()) {
                out.push(format!("{}: {text}", e.name()));
            } else if let Some(command) = running.or(e.reading.first().map(|r| &r.command)) {
                out.push(format!(
                    "{}: {}…",
                    e.name(),
                    command_title(&e.manifest, command)
                ));
            }
        }
        out
    }

    /// Step 2.1 of quitting: every request in flight is cancelled.
    pub(in crate::app) fn cancel_requests(&mut self) {
        for e in &mut self.extensions {
            if let Some(run) = &mut e.run {
                let ids: Vec<u64> = run.requests.keys().copied().collect();
                for id in ids {
                    run.session.cancel(id);
                }
                run.requests.clear();
                run.queued.clear();
                // Their work is cancelled: nothing more is written.
                run.deferred.clear();
            }
            // Their commands are never sent.
            e.reading.clear();
        }
    }
    /// Step 2.2: nothing more is admitted or read; [`Host::stop`] then sends
    /// `quitting` and closes the channels.
    pub(in crate::app) fn close_channels(&mut self) {
        self.closed = true;
    }
    /// Step 2.3: each program is told it is quitting and stopped by
    /// `deadline`, the absolute deadline the whole exit shares. Its stopper
    /// thread waits for it to exit at most `min(1 s, deadline − 200 ms)`, then
    /// ends its unit, so every [`crate::app::task::Stopping`] returned is
    /// finished by the deadline.
    pub(in crate::app) fn stop(&mut self, deadline: Instant) -> Vec<crate::app::task::Stopping> {
        self.closed = true;
        let mut stopping = Vec::new();
        for e in &mut self.extensions {
            if let Some(run) = e.run.take() {
                // A stop that fails to start its thread has ended the unit
                // itself.
                if let Ok(stopper) = run.session.stop(deadline) {
                    stopping.push(crate::app::task::Stopping::new(Some(stopper.into_thread())));
                }
            }
            e.status = None;
        }
        stopping
    }
    /// After the exit sequence has waited: ends every unit not confirmed
    /// ended, without blocking (`killpg` or `TerminateJobObject`), so helpers
    /// a program left die even when the deadline had passed or a stopper was
    /// detached.
    pub(in crate::app) fn kill_remaining(&mut self) {
        for killer in &self.killers {
            killer.kill_remaining();
        }
    }
}

/// A command's title, by its id in `manifest`.
fn command_title(manifest: &Manifest, command: &str) -> String {
    manifest
        .commands
        .iter()
        .find(|c| c.id == command)
        .map_or_else(|| command.to_string(), |c| c.title.clone())
}

/// What stands for the manifest of a developer folder that holds no valid
/// one: its id and name, and nothing it contributes.
fn placeholder(id: &str, name: &str) -> Manifest {
    Manifest {
        extension: rawmakase_extensions::protocol::manifest::Extension {
            id: id.to_string(),
            name: name.to_string(),
            version: semver::Version::new(0, 0, 0),
            rawmakase: semver::VersionReq::STAR,
            api: rawmakase_extensions::protocol::manifest::API,
            authors: Vec::new(),
            license: None,
            repository: None,
            description: None,
            icon: None,
        },
        access: Default::default(),
        program: None,
        settings: Vec::new(),
        commands: Vec::new(),
        panels: Vec::new(),
        control_adapters: Vec::new(),
        mask_generators: Vec::new(),
        sources: Vec::new(),
    }
}

/// Copies `file` from the data folder `data` into an extension's data folder
/// `into`, unless one is there already or there is none to copy. The copy is
/// written whole before it takes the name, so an interrupted copy leaves no
/// partial file to be taken for the user's.
fn take_over(data: &Path, into: &Path, file: &str) -> Result<(), String> {
    let (from, to) = (data.join(file), into.join(file));
    if std::fs::symlink_metadata(&to).is_ok() {
        return Ok(());
    }
    if let Err(e) = std::fs::symlink_metadata(&from) {
        return match e.kind() {
            std::io::ErrorKind::NotFound => Ok(()),
            _ => Err(format!("{} wasn't read: {e}", from.display())),
        };
    }
    let temp = into.join(format!(".{file}.copy"));
    let copied = std::fs::copy(&from, &temp).and_then(|_| std::fs::rename(&temp, &to));
    copied.map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        format!(
            "{} wasn't copied into {}: {e}; its devices are left there",
            from.display(),
            into.display()
        )
    })
}

/// Enables the MIDI extension, which starts off
/// ([`rawmakase_extensions::bundled::OFF_BY_DEFAULT`]), for someone who
/// already set up MIDI devices: in the core's `midi.json` of 0.4.12 and
/// earlier, or in the extension's own copy. Done once, while
/// `extensions.json` has no entry for MIDI; after that the entry decides,
/// so disabling it sticks. A `midi.json` that doesn't read counts as set up,
/// so a device's owner is never turned off by a file this version misreads.
fn keep_midi_for_its_devices(store: &Store) -> Result<(), String> {
    const MIDI: &str = "rawmakase.midi";
    let has_devices = |path: &Path| match std::fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Value>(&bytes) {
            // The first format, without a version, held one Loupedeck.
            Ok(v) if v.get("version").is_none() => true,
            Ok(v) => v["devices"].as_array().is_none_or(|d| !d.is_empty()),
            Err(_) => true,
        },
        Err(e) => e.kind() != std::io::ErrorKind::NotFound,
    };
    let undecided = |state: &State| !state.extensions.contains_key(MIDI);
    if !State::read(store.data_dir()).is_ok_and(|s| undecided(&s)) {
        return Ok(());
    }
    let in_use = [
        store.data_dir().join("midi.json"),
        store.extension_data_dir(MIDI).join("midi.json"),
    ]
    .iter()
    .any(|p| has_devices(p));
    if !in_use {
        return Ok(());
    }
    State::update(store.data_dir(), |state| {
        if undecided(state) {
            state.entry(MIDI).enabled = true;
        }
        Ok(())
    })
    .map_err(|e| format!("MIDI wasn't kept on for its devices: {e}"))
}

/// The manifest's and the program's modification times.
fn mtimes(
    dir: &Path,
    program: Option<&Path>,
) -> (Option<std::time::SystemTime>, Option<std::time::SystemTime>) {
    let modified = |p: &Path| std::fs::metadata(p).and_then(|m| m.modified()).ok();
    (
        modified(&dir.join(rawmakase_extensions::protocol::manifest::FILE)),
        program.and_then(modified),
    )
}

/// This build's version, as manifests' `rawmakase` ranges are checked against.
pub(in crate::app) fn app_version() -> semver::Version {
    semver::Version::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver")
}

/// The source a loaded extension shows on its page.
pub(in crate::app) fn source_label(e: &Extension) -> &'static str {
    match e.origin {
        Origin::Bundled => "Bundled",
        Origin::Developer => "Developer folder",
        Origin::Installed => e.entry.source().as_ref().map_or("Installed", Source::label),
    }
}
