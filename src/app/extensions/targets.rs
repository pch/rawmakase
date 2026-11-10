//! What an extension reads, and the target token that lets a later proposal
//! apply only to what it read.
//!
//! A token names the catalog (its location and structural generation, which
//! a reload, a reopen or a change that may reuse ids moves on) and, per photo,
//! its id, its file's [`Identity`], its catalog fingerprint
//! ([`crate::catalog::Catalog::photo_fingerprint`]) and, when it was read from
//! Develop, the document's load and command revision. It is JSON the program
//! sends back unchanged; a forged one gains nothing, as every part is checked
//! against the editor as it is when the proposal arrives.
//!
//! A proposal applies photo by photo:
//!
//! - a token of a catalog that isn't the open one, or of an earlier
//!   generation of it, is refused whole (`stale_target`);
//! - the photo open in Develop is checked against Develop's live state
//!   whether or not it was open when read: read from Develop, the same load
//!   and command revision; read from the catalog, Develop showing just what
//!   the catalog holds ([`Editor::develop_as_saved`]) with the same
//!   fingerprint. Alone in the proposal, it applies through Develop's
//!   guarded edit path, as one History step;
//! - a photo read from Develop but no longer open applies only if, when it
//!   was read, Develop showed just what the catalog holds: its token says so
//!   (`clean`). Otherwise the extension saw settings the catalog never had,
//!   or a protected edit, and it is refused (`stale_target`);
//! - any other photo applies as a batch applies a change, on a worker
//!   ([`crate::app::library::sync::apply_proposal`]): its file and fingerprint
//!   checked before its file is opened and again inside the one write
//!   transaction, which also checks that the proposal is still admitted. The
//!   program is answered when the worker finishes. While it runs, it is the
//!   running batch: its window shows, with Cancel, and moving to another
//!   photo or catalog waits for it;
//! - with photos of both kinds, the open photo is saved first and goes to
//!   the worker as any other, then opens again with what was written, so the
//!   whole proposal is one Undo, as a Sync that includes the open photo is.
//!
//! A cancel (the command answered, cancelled or past its deadline, the
//! extension disabled, its program gone, quitting, or the batch window's
//! Cancel) before the worker begins its write leaves every photo unchanged;
//! once the write has begun, it is the write's outcome that stands, with its
//! Undo, and the program is answered if it is still there.
use crate::app::Editor;
use crate::app::library::sync::{
    BatchChange, BatchProgress, Decoding, ProposalOutcome, ProposedOutcome, ProposedPhoto,
    ProposedSettings, Synced, sync_target,
};
use crate::app::shell::commands::Param;
use crate::catalog::{CatalogLocation, PhotoId};
use crate::storage::Identity;
use rawmakase_extensions::protocol::method::{Photo, ProposeSettings, Proposed, Refused, Snapshot};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::sync::Arc;
use std::time::Instant;

/// The most photos one read returns, and one proposal changes, so a reply
/// stays well inside a protocol line and a proposal's work stays bounded.
pub(in crate::app) const MAX_PHOTOS: usize = 200;

/// What a snapshot's token holds.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct Token {
    /// The catalog's location.
    catalog: String,
    /// Its structural generation when read.
    generation: u64,
    photos: Vec<TokenPhoto>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
struct TokenPhoto {
    id: i64,
    file: Option<Identity>,
    fingerprint: Option<String>,
    /// Read from Develop: its load and command revision then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    open: Option<(u64, u64)>,
    /// Read from Develop while it showed just what the catalog holds
    /// ([`Editor::develop_as_saved`]): the read is of the catalog row too, and
    /// holds once the photo is left.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    clean: bool,
}

/// What a proposal's worker reports, as [`super::Done::Proposed`].
pub(crate) struct Finished {
    ticket: u64,
    catalog: CatalogLocation,
    name: String,
    /// The photos in the proposal's order, for the reply's.
    order: Vec<i64>,
    /// Refused before the worker started.
    refused: Vec<Refused>,
    /// The photos the worker had, by their index in `ids`.
    ids: Vec<i64>,
    /// None when the worker panicked or the catalog didn't open: then
    /// nothing was written.
    outcome: Result<ProposalOutcome, String>,
}

/// How [`Editor::propose`] went.
pub(super) enum Proposing {
    Done(Proposed),
    /// A worker applies it and reports [`Finished`]; this is its cancel.
    Started(Arc<BatchProgress>),
}

fn stale(message: impl Into<String>) -> RemoteError {
    RemoteError::new(ErrorCode::StaleTarget, message)
}

/// A recipe's settings in the control layer's names and units.
fn settings_of(recipe: &crate::model::recipe::Recipe) -> Map<String, Value> {
    let mut recipe = recipe.clone();
    Param::all()
        .into_iter()
        .map(|(name, param)| (name, Value::from(param.shown(&mut recipe, 0))))
        .collect()
}

impl Editor {
    fn catalog_key(&self) -> Option<(String, u64)> {
        let library = self.library.as_ref()?;
        let crate::catalog::CatalogLocation::File(path) = library.session.catalog().location();
        Some((path.display().to_string(), library.session.generation()))
    }

    /// Whether Develop shows just what the catalog holds for its photo: the
    /// edit isn't protected, nothing is unsaved or held, and the settings are
    /// the saved edit's; with no saved edit, nothing was done in Develop and
    /// the photo shows the settings it opened with, as the catalog row
    /// resolves them.
    fn develop_as_saved(&self) -> bool {
        let edit = &self.document.edit;
        let (Some(id), Some(path), Some(library)) = (
            self.document.catalog_photo,
            self.document.path.as_ref(),
            self.library.as_ref(),
        ) else {
            return false;
        };
        if edit.save_state().is_protected()
            || edit.save_state().needs_save()
            || edit.history().in_gesture()
            || self.document.pending_lightroom.is_some()
        {
            return false;
        }
        match library.session.catalog().load_edit(id, path) {
            Ok(Some(saved)) => saved.recipe == *edit.recipe(),
            Ok(None) => edit.history().steps().0.is_empty(),
            Err(_) => false,
        }
    }

    /// A snapshot of `ids`, in order; ids the catalog doesn't hold are left
    /// out. The photo open in Develop is read as Develop has it.
    pub(super) fn read_photos(&self, ids: &[PhotoId]) -> Result<Snapshot, RemoteError> {
        let (catalog, generation) = self.catalog_key().ok_or_else(|| {
            RemoteError::new(ErrorCode::Other("no_catalog".into()), "No catalog is open")
        })?;
        if ids.len() > MAX_PHOTOS {
            return Err(RemoteError::new(
                ErrorCode::TooLarge,
                format!("A read covers at most {MAX_PHOTOS} photos"),
            ));
        }
        let library = self.library.as_ref().expect("checked above");
        let mut photos = Vec::new();
        let mut token = Token {
            catalog,
            generation,
            photos: Vec::new(),
        };
        for &id in ids {
            let Some(photo) = library.photo(id) else {
                continue;
            };
            let fingerprint = library
                .session
                .catalog()
                .photo_fingerprint(id)
                .map_err(|e| {
                    RemoteError::new(ErrorCode::Other("catalog".into()), format!("{e:#}"))
                })?;
            let develop = self.document.catalog_photo == Some(id) && !self.document.loading();
            let (settings, file, open, clean) = if develop {
                (
                    settings_of(self.document.edit.recipe()),
                    self.document.file.clone(),
                    Some((self.document.load_id(), self.automation.revision)),
                    self.develop_as_saved(),
                )
            } else {
                let saved = library
                    .session
                    .catalog()
                    .load_edit(id, &photo.path)
                    .ok()
                    .flatten()
                    .map(|e| settings_of(&e.recipe))
                    .unwrap_or_default();
                (saved, Identity::read(&photo.path).ok(), None, false)
            };
            photos.push(Photo {
                id: id.0,
                path: photo.path.clone(),
                settings,
            });
            token.photos.push(TokenPhoto {
                id: id.0,
                file,
                fingerprint,
                open,
                clean,
            });
        }
        Ok(Snapshot {
            token: serde_json::to_string(&token).expect("a token is JSON"),
            photos,
        })
    }

    /// The Library's selection.
    pub(super) fn read_selection(&self) -> Result<Snapshot, RemoteError> {
        let ids = self
            .library
            .as_ref()
            .map(|l| l.selected_photos())
            .unwrap_or_default();
        self.read_photos(&ids)
    }

    /// The photo open in Develop, or none.
    pub(super) fn read_open_photo(&self) -> Result<Snapshot, RemoteError> {
        let ids: Vec<PhotoId> = self.document.catalog_photo.into_iter().collect();
        self.read_photos(&ids)
    }

    /// Applies an admitted proposal; `name` names the History step and Undo.
    /// The open photo alone applies at once; photos not open start a worker
    /// that reports [`Finished`] with `ticket`, and whose write is refused
    /// from `deadline` on (the proposal's command no longer admitted).
    pub(super) fn propose(
        &mut self,
        proposal: ProposeSettings,
        name: &str,
        ticket: u64,
        deadline: Option<Instant>,
    ) -> Result<Proposing, RemoteError> {
        let token: Token = serde_json::from_str(&proposal.token)
            .map_err(|_| stale("The token isn't one RAWmakase gave"))?;
        if self.catalog_key() != Some((token.catalog.clone(), token.generation)) {
            return Err(stale(
                "The catalog the photos were read from is closed or was read again",
            ));
        }
        if proposal.changes.len() > MAX_PHOTOS {
            return Err(RemoteError::new(
                ErrorCode::TooLarge,
                format!("A proposal changes at most {MAX_PHOTOS} photos"),
            ));
        }
        let order: Vec<i64> = proposal.changes.iter().map(|c| c.photo).collect();
        let mut refused = Vec::new();
        let mut open = None;
        let mut closed = Vec::new();
        for change in proposal.changes {
            let refuse = |code: ErrorCode| Refused {
                photo: change.photo,
                code,
            };
            let Some(read) = token.photos.iter().find(|p| p.id == change.photo) else {
                refused.push(refuse(ErrorCode::StaleTarget));
                continue;
            };
            let settings = match parse_settings(&change.settings) {
                Ok(settings) => settings,
                Err(code) => {
                    refused.push(refuse(code));
                    continue;
                }
            };
            if self.document.catalog_photo == Some(PhotoId(change.photo)) {
                open = Some((read.clone(), settings));
                continue;
            }
            match self.closed_photo(read, &settings, name) {
                Ok(photo) => closed.push(photo),
                Err(code) => refused.push(refuse(code)),
            }
        }
        if closed.is_empty() {
            let mut proposed = Proposed {
                applied: Vec::new(),
                refused,
            };
            if let Some((read, settings)) = open {
                match self.propose_open(&read, &settings, name) {
                    Ok(()) => proposed.applied.push(read.id),
                    Err(code) => proposed.refused.push(Refused {
                        photo: read.id,
                        code,
                    }),
                }
            }
            sort(&mut proposed, &order);
            return Ok(Proposing::Done(proposed));
        }
        // From here on it is a batch, and the batch holds the catalog's
        // photos while it runs.
        if self.activity.is_busy() {
            refused.extend(closed.iter().map(|p| Refused {
                photo: p.target.id.0,
                code: ErrorCode::Busy,
            }));
            refused.extend(open.map(|(read, _)| Refused {
                photo: read.id,
                code: ErrorCode::Busy,
            }));
            let mut proposed = Proposed {
                applied: Vec::new(),
                refused,
            };
            sort(&mut proposed, &order);
            return Ok(Proposing::Done(proposed));
        }
        if let Some((read, settings)) = open {
            match self.open_for_batch(&read, &settings, name) {
                Ok(photo) => closed.push(photo),
                Err(code) => refused.push(Refused {
                    photo: read.id,
                    code,
                }),
            }
        }
        Ok(Proposing::Started(self.start_proposal(
            ticket, name, order, refused, closed, deadline,
        )))
    }

    /// The photo open in Develop: admitted only while Develop's state is what
    /// the token saw.
    fn open_as_read(&mut self, read: &TokenPhoto) -> Result<(), ErrorCode> {
        if self.document.loading() || self.document.full().is_none() {
            return Err(ErrorCode::Busy);
        }
        let as_catalog = |e: &Self| {
            let Some(library) = e.library.as_ref() else {
                return false;
            };
            let now = library
                .session
                .catalog()
                .photo_fingerprint(PhotoId(read.id))
                .ok()
                .flatten();
            e.develop_as_saved()
                && now.is_some()
                && now == read.fingerprint
                && read.file.is_some()
                && read.file == e.document.file
        };
        let same = match read.open {
            Some((load, revision)) => {
                self.sync_command_revision();
                (load == self.document.load_id() && revision == self.automation.revision)
                    || (read.clean && as_catalog(self))
            }
            None => as_catalog(self),
        };
        if same {
            Ok(())
        } else {
            Err(ErrorCode::StaleTarget)
        }
    }

    /// The photo open in Develop, alone in its proposal: applied through the
    /// guarded edit path as one step.
    fn propose_open(
        &mut self,
        read: &TokenPhoto,
        settings: &[(Param, f32)],
        name: &str,
    ) -> Result<(), ErrorCode> {
        self.open_as_read(read)?;
        self.apply_open_settings(settings, name)
            .map_err(|e| ErrorCode::Other(e.code.into()))
    }

    /// The photo open in Develop, in a proposal with others: saved first,
    /// then applied by the worker as any other photo, so the proposal is one
    /// Undo. Its edit is final only when no analysis or Lightroom edit is
    /// still to land on it, and a protected edit is never written.
    fn open_for_batch(
        &mut self,
        read: &TokenPhoto,
        settings: &[(Param, f32)],
        name: &str,
    ) -> Result<ProposedPhoto, ErrorCode> {
        self.open_as_read(read)?;
        if self.document.edit.save_state().is_protected() {
            return Err(ErrorCode::Other("protected".into()));
        }
        if self.document.pending_lightroom.is_some()
            || self.document.auto.is_running()
            || self.document.straighten.is_running()
        {
            return Err(ErrorCode::Busy);
        }
        if !self.flush() {
            return Err(ErrorCode::Other("not_saved".into()));
        }
        let id = PhotoId(read.id);
        let library = self.library.as_ref().ok_or(ErrorCode::StaleTarget)?;
        let expected = library
            .session
            .catalog()
            .photo_fingerprint(id)
            .ok()
            .flatten()
            .ok_or(ErrorCode::StaleTarget)?;
        let photo = library.photo(id).ok_or(ErrorCode::StaleTarget)?;
        Ok(ProposedPhoto {
            target: sync_target(photo),
            expected,
            identity: self.document.file.clone().ok_or(ErrorCode::StaleTarget)?,
            change: proposed_change(settings, name),
        })
    }

    /// A photo not open in Develop, as the worker gets it; nothing is read
    /// from its file here.
    fn closed_photo(
        &self,
        read: &TokenPhoto,
        settings: &[(Param, f32)],
        name: &str,
    ) -> Result<ProposedPhoto, ErrorCode> {
        let (Some(file), Some(fingerprint)) = (&read.file, &read.fingerprint) else {
            return Err(ErrorCode::StaleTarget);
        };
        // Read from Develop while it showed what the catalog doesn't hold:
        // valid only while that photo stays open.
        if read.open.is_some() && !read.clean {
            return Err(ErrorCode::StaleTarget);
        }
        let library = self.library.as_ref().ok_or(ErrorCode::StaleTarget)?;
        let photo = library
            .photo(PhotoId(read.id))
            .ok_or(ErrorCode::StaleTarget)?;
        Ok(ProposedPhoto {
            target: sync_target(photo),
            expected: fingerprint.clone(),
            identity: file.clone(),
            change: proposed_change(settings, name),
        })
    }

    /// Starts the worker of a proposal, as the running batch.
    fn start_proposal(
        &mut self,
        ticket: u64,
        name: &str,
        order: Vec<i64>,
        refused: Vec<Refused>,
        photos: Vec<ProposedPhoto>,
        deadline: Option<Instant>,
    ) -> Arc<BatchProgress> {
        let progress = Arc::new(BatchProgress::new(name, photos.len()));
        let catalog = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone())
            .expect("a proposal's catalog is open");
        let started = self.activity.begin_sync(progress.clone());
        debug_assert!(started, "checked idle by the caller");
        let defaults = self.raw_defaults.clone();
        let decoding = Decoding {
            demosaic: self.session.demosaic,
            cache: Some(self.decode_cache.clone()),
        };
        let finished = |outcome| Finished {
            ticket,
            catalog: catalog.clone(),
            name: name.to_string(),
            order: order.clone(),
            refused: refused.clone(),
            ids: photos.iter().map(|p| p.target.id.0).collect(),
            outcome,
        };
        let failed = finished(Err(String::new()));
        let mut done = finished(Err(String::new()));
        let work = progress.clone();
        #[cfg(test)]
        let hook = self.proposal_hook.clone();
        crate::app::task::spawn(
            self.tx.clone(),
            self.context.clone(),
            move |tx| {
                #[cfg(test)]
                if let Some(hook) = hook {
                    hook();
                }
                done.outcome = crate::catalog::Catalog::open(&done.catalog)
                    .map_err(|e| format!("{e:#}"))
                    .map(|mut catalog| {
                        crate::app::library::sync::apply_proposal(
                            &mut catalog,
                            &photos,
                            &defaults,
                            &decoding,
                            &work,
                            deadline,
                        )
                    });
                let _ = tx.send(crate::app::worker::Event::Extensions(Box::new(
                    super::Done::Proposed(Box::new(done)),
                )));
            },
            move |tx, why| {
                let mut failed = failed;
                failed.outcome = Err(why);
                let _ = tx.send(crate::app::worker::Event::Extensions(Box::new(
                    super::Done::Proposed(Box::new(failed)),
                )));
            },
        );
        progress
    }

    /// A proposal's worker reported back: the batch ends, what it wrote gets
    /// its previews and one Undo, the open photo opens again if it was
    /// written, and the program is answered.
    pub(super) fn proposal_finished(&mut self, finished: Finished) {
        self.activity.finish_sync();
        let Finished {
            ticket,
            catalog,
            name,
            order,
            refused,
            ids,
            outcome,
        } = finished;
        // Written to a catalog no longer open: its Undo would write this one.
        let open = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location());
        let reply = match outcome {
            _ if open != Some(&catalog) => Err(stale("The catalog was closed")),
            Err(why) => {
                self.messages
                    .fail(format!("{name}: no photo changed: {why}"));
                Err(RemoteError::new(ErrorCode::Other("failed".into()), why))
            }
            Ok(ProposalOutcome {
                admitted: false, ..
            }) => {
                self.messages
                    .note(format!("{name} stopped: no photo changed"));
                Err(RemoteError::new(
                    ErrorCode::Cancelled,
                    "the proposal was cancelled",
                ))
            }
            Ok(ProposalOutcome { photos, .. }) => {
                let mut proposed = Proposed {
                    applied: Vec::new(),
                    refused,
                };
                let mut changed = Vec::new();
                for (index, outcome) in photos {
                    let photo = ids[index];
                    match outcome {
                        ProposedOutcome::Applied(synced) => {
                            proposed.applied.push(photo);
                            changed.extend(synced.map(|s| *s));
                        }
                        ProposedOutcome::Moved => proposed.refused.push(Refused {
                            photo,
                            code: ErrorCode::StaleTarget,
                        }),
                        ProposedOutcome::Failed(why) => {
                            let file = self
                                .library
                                .as_ref()
                                .and_then(|l| l.photo(PhotoId(photo)))
                                .map(|p| p.filename.clone())
                                .unwrap_or_default();
                            self.messages
                                .warn(format!("{name}: {file} not changed: {why}"));
                            proposed.refused.push(Refused {
                                photo,
                                code: ErrorCode::Other("not_saved".into()),
                            });
                        }
                    }
                }
                if !changed.is_empty() {
                    self.record_proposed(&name, changed);
                }
                sort(&mut proposed, &order);
                Ok(serde_json::to_value(proposed).expect("a reply is JSON"))
            }
        };
        self.extensions.finish_proposal(ticket, reply);
    }

    /// Photos a proposal wrote: their previews, the open photo opened again
    /// with what was written, and one Undo for them all, as a Sync's.
    fn record_proposed(&mut self, name: &str, changed: Vec<Synced>) {
        let ids: Vec<PhotoId> = changed.iter().map(|s| s.id).collect();
        if let Some(library) = &mut self.library {
            library.edits_changed(ids.iter().copied());
        }
        self.refresh_previews(&ids);
        self.reopen_synced(&ids);
        self.load_reference();
        self.undo_log
            .push(crate::app::shell::undo::Command::Sync(Box::new(
                crate::app::library::sync::SyncCommand {
                    sequence: crate::edit_session::sequence(),
                    change: BatchChange::Proposed(Box::new(ProposedSettings {
                        name: name.to_string(),
                        settings: Vec::new(),
                    })),
                    edits: changed,
                },
            )));
    }
}

/// One photo's change, as the worker applies it.
fn proposed_change(settings: &[(Param, f32)], name: &str) -> BatchChange {
    BatchChange::Proposed(Box::new(ProposedSettings {
        name: name.to_string(),
        settings: settings.iter().map(|(p, v)| (p.spec(), *v)).collect(),
    }))
}

/// Puts a reply's photos in the proposal's order.
fn sort(proposed: &mut Proposed, order: &[i64]) {
    let at = |photo: i64| order.iter().position(|p| *p == photo);
    proposed.applied.sort_by_key(|p| at(*p));
    proposed.refused.sort_by_key(|r| at(r.photo));
}

/// A proposal's settings as parameters, or the code that refuses them.
fn parse_settings(settings: &Map<String, Value>) -> Result<Vec<(Param, f32)>, ErrorCode> {
    let mut out = Vec::new();
    for (name, value) in settings {
        let param = match Param::parse(name) {
            Some(Param::Band(_)) | None => return Err(ErrorCode::InvalidRequest),
            Some(param) => param,
        };
        let value = value
            .as_f64()
            .filter(|v| v.is_finite())
            .ok_or(ErrorCode::InvalidRequest)?;
        out.push((param, value as f32));
    }
    if out.is_empty() {
        return Err(ErrorCode::InvalidRequest);
    }
    Ok(out)
}
