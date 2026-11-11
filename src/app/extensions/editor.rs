//! The editor's side of extensions: polling the host each frame and
//! answering the programs' reads and proposals, running commands, telling
//! programs what changed, and the file dialogs and background work of
//! Preferences > Extensions.
use super::{Answer, Call, Host, Outcome, RunBy};
use crate::app::Editor;
use crate::app::worker::Event;
use crate::catalog::PhotoId;
use rawmakase_extensions::protocol::manifest::AppliesTo;
use rawmakase_extensions::protocol::method::{
    ActionSupport, AppEvent, Empty, Level, PhotoOpened, ProgramRequest,
};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

/// A reply's value.
fn value<T: serde::Serialize>(v: Result<T, RemoteError>) -> Result<Value, RemoteError> {
    v.map(|v| serde_json::to_value(v).expect("a reply is JSON"))
}

/// What a file dialog of Preferences > Extensions chooses for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PathTarget {
    /// A `.rmext` package to install.
    Package,
    /// An unpacked extension's folder, in developer mode.
    Folder,
    /// A `file` or `folder` setting.
    Setting {
        extension: String,
        setting: String,
        folder: bool,
    },
    /// A panel's `file` or `folder` control.
    Control {
        extension: String,
        panel: String,
        control: String,
        folder: bool,
        extensions: Vec<String>,
    },
}

/// Background work of Preferences > Extensions, reported as
/// [`Event::Extensions`].
pub(crate) enum Done {
    /// A file dialog closed, with the path chosen or none.
    Chosen(PathTarget, Option<PathBuf>),
    /// A package was installed, or why not.
    Installed(Result<String, String>),
    /// A repository's newest package was downloaded for the user to review.
    Offered(Result<Box<rawmakase_extensions::github::Offer>, String>),
    /// A package file was read for the user to review.
    Inspected(PathBuf, Result<Box<rawmakase_extensions::Manifest>, String>),
    /// Check for Updates finished.
    Checked(Result<Box<rawmakase_extensions::github::Checked>, String>),
    /// A proposal's worker finished, or panicked.
    Proposed(Box<super::targets::Finished>),
    /// A read's worker finished, or panicked.
    Read(Box<super::targets::ReadDone>),
}

/// What the editor last told programs, to tell them only of changes.
#[derive(Default)]
pub(in crate::app) struct Told {
    selection: Vec<PhotoId>,
    photo: Option<PhotoId>,
    catalog: Option<(PathBuf, u64)>,
    commands: u64,
}

impl Editor {
    /// Starts the extension host on the user's data folder, with the key map
    /// taking up the enabled extensions' commands. Only `Editor::new` calls
    /// it: a test editor's host stays inert.
    pub(in crate::app) fn start_extensions(&mut self, ctx: &eframe::egui::Context) {
        let wake = ctx.clone();
        self.extensions = Host::start(
            &crate::storage::data_dir(),
            std::sync::Arc::new(move || wake.request_repaint()),
        );
        self.sync_extension_commands();
        for error in self.extensions.take_errors() {
            self.messages.warn(format!("Extensions: {error}"));
        }
    }

    /// Makes the key map's extension commands the enabled extensions'.
    pub(in crate::app) fn sync_extension_commands(&mut self) {
        if self.extensions_told.commands != self.extensions.revision() {
            self.extensions_told.commands = self.extensions.revision();
            self.keymap.set_dynamic(self.extensions.commands());
        }
    }

    /// One frame's work of the extension host: what programs sent, their
    /// requests answered, and the editor's changes told to them.
    pub(in crate::app) fn extensions_frame(&mut self, ctx: &eframe::egui::Context) {
        if !self.extensions.active() {
            return;
        }
        let (outcomes, next) = self.extensions_poll(Instant::now());
        self.extension_outcomes(outcomes);
        if let Some(next) = next {
            ctx.request_repaint_after(next.saturating_duration_since(Instant::now()));
        }
    }

    /// The host's poll as of `now`, with the editor answering its programs'
    /// requests; what came of it, and when to poll next.
    pub(in crate::app) fn extensions_poll(
        &mut self,
        now: Instant,
    ) -> (Vec<Outcome>, Option<Instant>) {
        self.tell_extensions();
        for (extension, ticket) in self.selection.take_withdrawn() {
            self.extensions.cancel_mask(&extension, ticket);
        }
        // The host answers through the editor, which then can't reach it: it
        // is taken out for the poll, and an inert one stands in.
        let mut host = std::mem::take(&mut self.extensions);
        let mut outcomes = Vec::new();
        let next = host.poll(now, &mut |call| self.answer_extension(call), &mut outcomes);
        self.extensions = host;
        self.sync_extension_commands();
        self.end_device_gesture();
        (outcomes, next)
    }

    /// Ends the gesture in progress when the device that sent it is gone:
    /// disconnected, removed, its program ended or its extension disabled.
    /// Its next input, from a new connection or start, is a gesture of its
    /// own anyway; this keeps nothing joining the old one meanwhile.
    fn end_device_gesture(&mut self) {
        if let Some(crate::app::shell::commands::Source::Device {
            session,
            device,
            epoch,
        }) = self.automation.turn_source()
            && !self.extensions.device_current(session, device, epoch)
        {
            self.automation.end_turn();
        }
    }

    /// Says what came of commands, and the programs' notifications.
    pub(in crate::app) fn extension_outcomes(&mut self, outcomes: Vec<Outcome>) {
        for outcome in outcomes {
            match outcome {
                Outcome::Message { level, text } => match level {
                    Level::Info => self.messages.note(text),
                    Level::Warning => self.messages.warn(text),
                    Level::Error => self.messages.fail(text),
                },
                Outcome::Command { title, result, .. } => match result {
                    Ok(_) => self.messages.note(format!("{title}: done")),
                    Err(e) if e.code == ErrorCode::Cancelled || e.code == ErrorCode::Disabled => {
                        self.messages.note(format!("{title}: stopped"));
                    }
                    Err(e) => self.messages.fail(format!("{title}: {e}")),
                },
                Outcome::Mask {
                    extension,
                    ticket,
                    result,
                    buffers,
                } => self.mask_answered(&extension, ticket, result, buffers),
                Outcome::MaskSetup {
                    extension,
                    generator,
                    result,
                } => self.mask_setup_done(&extension, &generator, result),
            }
        }
        self.check_mask_requests();
    }

    /// Tells programs what changed since the last frame: the selection, the
    /// photo open in Develop, and the catalog closing or being read again.
    fn tell_extensions(&mut self) {
        let catalog = self.library.as_ref().map(|l| {
            let crate::catalog::CatalogLocation::File(path) = l.session.catalog().location();
            (path.clone(), l.session.generation())
        });
        if catalog != self.extensions_told.catalog {
            if self.extensions_told.catalog.is_some() {
                self.extensions
                    .broadcast(&AppEvent::CatalogClosed(Empty {}));
            }
            self.extensions_told.catalog = catalog;
        }
        let selection = self
            .library
            .as_ref()
            .map(|l| l.selected_photos())
            .unwrap_or_default();
        if selection != self.extensions_told.selection {
            self.extensions_told.selection = selection;
            self.extensions
                .broadcast(&AppEvent::SelectionChanged(Empty {}));
        }
        let photo = self.document.catalog_photo;
        if photo != self.extensions_told.photo {
            self.extensions_told.photo = photo;
            self.extensions
                .broadcast(&AppEvent::PhotoOpened(PhotoOpened {
                    photo: photo.map(|p| p.0),
                }));
        }
    }

    /// Answers a program's read or admitted proposal; a proposal for photos
    /// not open is answered when its worker finishes.
    fn answer_extension(&mut self, call: Call) -> Answer {
        // A read of photos not open opens their files: on a worker, answered
        // when it finishes.
        if let Some(ids) = self.read_ids(&call.request) {
            return match self.begin_read(&ids) {
                Err(err) => Answer::Now(Err(err)),
                Ok(reading) if !reading.needs_worker() => {
                    Answer::Now(value(reading.finish(None, &AtomicBool::new(false))))
                }
                Ok(reading) => {
                    let cancel = Arc::new(AtomicBool::new(false));
                    self.start_read(call.ticket, reading, cancel.clone());
                    Answer::Reading(cancel)
                }
            };
        }
        let Call {
            request,
            title,
            ticket,
            deadline,
            devices,
        } = call;
        match request {
            ProgramRequest::ProposeSettings(proposal) => {
                match self.propose(proposal, &title, ticket, deadline) {
                    Ok(super::targets::Proposing::Done(proposed)) => {
                        Answer::Now(Ok(serde_json::to_value(proposed).expect("a reply is JSON")))
                    }
                    Ok(super::targets::Proposing::Started(cancel)) => Answer::Later(cancel),
                    Err(err) => Answer::Now(Err(err)),
                }
            }
            ProgramRequest::AdapterOperate(operate) => Answer::Now(match devices {
                None => Err(RemoteError::new(
                    ErrorCode::InvalidRequest,
                    "no devices for this request",
                )),
                Some((session, devices)) => self
                    .device_operations(&devices, session, operate, &title)
                    .map(|v| serde_json::to_value(v).expect("a reply is JSON")),
            }),
            request => Answer::Now(self.answer_read(request)),
        }
    }

    /// The photos a read of them asks for, in order; none for any other
    /// request.
    fn read_ids(&self, request: &ProgramRequest) -> Option<Vec<PhotoId>> {
        match request {
            ProgramRequest::ReadSelection(_) => Some(self.selection_ids()),
            ProgramRequest::ReadOpenPhoto(_) => {
                Some(self.document.catalog_photo.into_iter().collect())
            }
            ProgramRequest::ReadPhotos(read) => {
                Some(read.ids.iter().copied().map(PhotoId).collect())
            }
            _ => None,
        }
    }

    /// Answers a program's request for the control layer's capabilities, or
    /// whether it runs an action.
    fn answer_read(&mut self, request: ProgramRequest) -> Result<Value, RemoteError> {
        match request {
            ProgramRequest::ReadCapabilities(_) => {
                let command = crate::app::shell::commands::Command::new(
                    crate::app::shell::commands::Operation::Capabilities,
                );
                let ctx = self.context.clone();
                self.execute_command_from(
                    command,
                    crate::app::shell::commands::Source::Socket,
                    &ctx,
                )
                .map(|outcome| serde_json::to_value(outcome).expect("capabilities are JSON"))
                .map_err(|e| RemoteError::new(ErrorCode::Other(e.code.into()), e.message))
            }
            ProgramRequest::ReadAction(read) => value(Ok(ActionSupport {
                supported: crate::app::shell::automation::action_supported(&read.spec),
            })),
            _ => Err(RemoteError::new(
                ErrorCode::UnknownMethod,
                "the host answers this request itself",
            )),
        }
    }

    /// Runs `id`, an enabled extension's command (`<extension>/<command>`),
    /// on what its manifest says it applies to, read now: photos not open
    /// are read on a worker, and the command is sent once they are
    /// ([`Host::finish_read`]); until then it is running, and Stop or
    /// disabling the extension cancels it.
    pub(in crate::app) fn run_extension_command(
        &mut self,
        id: &str,
        by: RunBy,
    ) -> crate::app::shell::commands::Result<()> {
        use crate::app::shell::commands::Error;
        let unknown = || Error::new("unknown_action", format!("No enabled extension has {id}"));
        if self.keymap.effective().command(id).is_none() {
            return Err(unknown());
        }
        let (extension, command) = id.split_once('/').ok_or_else(unknown)?;
        let applies = self
            .extensions
            .get(extension)
            .and_then(|e| e.manifest.commands.iter().find(|c| c.id == command))
            .map(|c| c.applies_to)
            .ok_or_else(unknown)?;
        let fail = |e: RemoteError| Error::new("not_ready", e.message);
        let ids = match applies {
            AppliesTo::None => None,
            AppliesTo::Photo => {
                if self.document.catalog_photo.is_none() {
                    return Err(Error::new("no_document", "Open a photo in Develop first"));
                }
                Some(self.document.catalog_photo.into_iter().collect())
            }
            AppliesTo::Selection => {
                let ids = self.selection_ids();
                if ids.is_empty() {
                    return Err(Error::new("no_selection", "Select photos first"));
                }
                Some(ids)
            }
        };
        let target = match ids {
            None => None,
            Some(ids) => {
                let reading = self.begin_read(&ids).map_err(fail)?;
                if reading.needs_worker() {
                    let cancel = self
                        .extensions
                        .read_for_command(extension, command, by)
                        .map_err(|why| Error::new("extension", why))?;
                    self.start_read(cancel.0, reading, cancel.1);
                    return Ok(());
                }
                Some(
                    reading
                        .finish(None, &AtomicBool::new(false))
                        .map_err(fail)?,
                )
            }
        };
        self.extensions
            .run_command(extension, command, target, by)
            .map_err(|why| Error::new("extension", why))
    }

    /// Runs the extensions' commands whose keys were pressed this frame.
    pub(in crate::app) fn extension_keys(&mut self, ctx: &eframe::egui::Context) {
        let map = self.keymap.effective();
        let keys = crate::app::shell::keymap::FrameKeys::of(ctx);
        for dynamic in map.dynamic() {
            if keys.take(&dynamic.binding).is_some()
                && let Err(e) = self.run_extension_command(dynamic.command.id, RunBy::Key)
            {
                self.messages
                    .fail(format!("{}: {}", dynamic.command.title, e.message));
            }
        }
    }

    /// Opens a file dialog for `target`, off the UI thread.
    pub(in crate::app) fn extension_dialog(&mut self, target: PathTarget) {
        if !self.activity.begin_dialog() {
            return;
        }
        let ctx = self.context.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            ctx,
            move |tx| {
                let chosen = match &target {
                    PathTarget::Package => rfd::FileDialog::new()
                        .add_filter("RAWmakase extension", &["rmext"])
                        .pick_file(),
                    PathTarget::Folder
                    | PathTarget::Setting { folder: true, .. }
                    | PathTarget::Control { folder: true, .. } => {
                        rfd::FileDialog::new().pick_folder()
                    }
                    PathTarget::Control { extensions, .. } if !extensions.is_empty() => {
                        rfd::FileDialog::new()
                            .add_filter("Files", extensions)
                            .pick_file()
                    }
                    PathTarget::Setting { .. } | PathTarget::Control { .. } => {
                        rfd::FileDialog::new().pick_file()
                    }
                };
                let _ = tx.send(Event::Extensions(Box::new(Done::Chosen(target, chosen))));
            },
            |tx, _| {
                let _ = tx.send(Event::DialogClosed);
            },
        );
    }

    /// Runs `work` for Preferences > Extensions on a thread of its own; a
    /// panic reports as a failure, so the page never stays busy.
    pub(in crate::app) fn extension_work(
        &mut self,
        work: impl FnOnce() -> Done + Send + 'static,
        failed: impl FnOnce(String) -> Done + Send + 'static,
    ) {
        self.extension_page.busy = true;
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                let _ = tx.send(Event::Extensions(Box::new(work())));
            },
            move |tx, why| {
                let _ = tx.send(Event::Extensions(Box::new(failed(why))));
            },
        );
    }

    /// What a dialog or background work of Preferences > Extensions reported.
    pub(in crate::app) fn extension_done(&mut self, done: Done) {
        match done {
            Done::Chosen(target, path) => {
                self.activity.finish_dialog();
                if let Some(path) = path {
                    self.extension_path_chosen(target, path);
                }
            }
            Done::Installed(result) => {
                self.extension_page.busy = false;
                match result {
                    Ok(name) => self.messages.done(format!("Installed {name}")),
                    Err(why) => self.messages.fail(format!("Not installed: {why}")),
                }
                self.extension_page.offer = None;
                self.reload_extensions();
            }
            Done::Offered(result) => {
                self.extension_page.busy = false;
                match result {
                    Ok(offer) => {
                        self.extension_page.offer = Some(super::page::Offered::Github(offer))
                    }
                    Err(why) => self.messages.fail(format!("Not installed: {why}")),
                }
            }
            Done::Inspected(path, result) => {
                self.extension_page.busy = false;
                match result {
                    Ok(manifest) => {
                        self.extension_page.offer =
                            Some(super::page::Offered::File { path, manifest });
                    }
                    Err(why) => self.messages.fail(format!("Not installed: {why}")),
                }
            }
            Done::Proposed(finished) => self.proposal_finished(*finished),
            Done::Read(done) => self.read_finished(*done),
            Done::Checked(result) => {
                self.extension_page.busy = false;
                match result {
                    Ok(checked) => {
                        for (id, e) in &checked.failed {
                            self.messages
                                .warn(format!("{id}: updates not checked: {e}"));
                        }
                        if checked.updates.is_empty() {
                            self.messages.note("Every extension is up to date");
                        }
                        self.extension_page.updates = checked.updates;
                    }
                    Err(why) => self.messages.fail(format!("Updates not checked: {why}")),
                }
            }
        }
    }

    fn extension_path_chosen(&mut self, target: PathTarget, path: PathBuf) {
        match target {
            // Read first and shown for review, as a package from GitHub is;
            // installed once the user confirms.
            PathTarget::Package => {
                let Some(store) = self.extensions.store().cloned() else {
                    return;
                };
                let app = self.extensions.app_version().clone();
                let failed = path.clone();
                self.extension_work(
                    move || {
                        let manifest = store
                            .inspect(&path, &app)
                            .map(|i| Box::new(i.manifest))
                            .map_err(|e| e.to_string());
                        Done::Inspected(path, manifest)
                    },
                    move |why| Done::Inspected(failed, Err(why)),
                );
            }
            PathTarget::Folder => match self.extensions.load_folder(&path) {
                Ok(name) => {
                    self.messages.done(format!("Loaded {name} from its folder"));
                    self.sync_extension_commands();
                }
                Err(why) => self.messages.fail(format!("Not loaded: {why}")),
            },
            PathTarget::Setting {
                extension, setting, ..
            } => {
                let value = Value::String(path.display().to_string());
                if let Err(why) = self.extensions.set_setting(&extension, &setting, value) {
                    self.messages.fail(why);
                }
            }
            PathTarget::Control {
                extension,
                panel,
                control,
                ..
            } => {
                let value = Value::String(path.display().to_string());
                if let Err(why) = self
                    .extensions
                    .panel_changed(&extension, &panel, &control, value)
                {
                    self.messages.fail(why);
                }
            }
        }
    }

    /// Reads `extensions.json` and the installed extensions again.
    pub(in crate::app) fn reload_extensions(&mut self) {
        self.extensions.reload();
        self.sync_extension_commands();
        for error in self.extensions.take_errors() {
            self.messages.warn(format!("Extensions: {error}"));
        }
    }

    /// Turns an extension on or off.
    pub(in crate::app) fn set_extension_enabled(&mut self, id: &str, enabled: bool) {
        match self.extensions.set_enabled(id, enabled) {
            Ok(outcomes) => self.extension_outcomes(outcomes),
            Err(why) => self.messages.fail(why),
        }
        self.sync_extension_commands();
    }

    /// Uninstalls an extension.
    pub(in crate::app) fn uninstall_extension(&mut self, id: &str) {
        match self.extensions.uninstall(id) {
            Ok(outcomes) => {
                self.extension_outcomes(outcomes);
                self.messages.done("Extension uninstalled");
            }
            Err(why) => self.messages.fail(why),
        }
        self.sync_extension_commands();
    }
}
