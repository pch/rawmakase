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
//!   and command revision; read from the catalog, nothing unsaved and the same
//!   fingerprint. It applies through Develop's guarded edit path, as one
//!   History step;
//! - any other photo applies as a batch applies a change, its file and
//!   fingerprint checked again inside the catalog's write transaction
//!   ([`crate::app::library::sync::apply_if`]).
use crate::app::Editor;
use crate::app::library::sync::{AppliedIf, BatchChange, ProposedSettings, sync_target};
use crate::app::shell::commands::Param;
use crate::catalog::PhotoId;
use crate::storage::Identity;
use rawmakase_extensions::protocol::method::{Photo, ProposeSettings, Proposed, Refused, Snapshot};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// The most photos one read returns, and one proposal changes, so a reply
/// stays well inside a protocol line and a proposal's work on the UI thread
/// stays bounded.
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
            let (settings, file, open) = if develop {
                (
                    settings_of(self.document.edit.recipe()),
                    self.document.file.clone(),
                    Some((self.document.load_id(), self.automation.revision)),
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
                (saved, Identity::read(&photo.path).ok(), None)
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

    /// Applies an admitted proposal, photo by photo; `name` names the History
    /// step and Undo.
    pub(super) fn propose(
        &mut self,
        proposal: ProposeSettings,
        name: &str,
    ) -> Result<Proposed, RemoteError> {
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
        let mut proposed = Proposed::default();
        let mut changed = Vec::new();
        for change in proposal.changes {
            let refuse = |code: ErrorCode| Refused {
                photo: change.photo,
                code,
            };
            let Some(read) = token.photos.iter().find(|p| p.id == change.photo) else {
                proposed.refused.push(refuse(ErrorCode::StaleTarget));
                continue;
            };
            let settings = match parse_settings(&change.settings) {
                Ok(settings) => settings,
                Err(code) => {
                    proposed.refused.push(refuse(code));
                    continue;
                }
            };
            let id = PhotoId(change.photo);
            let result = if self.document.catalog_photo == Some(id) {
                self.propose_open(read, &settings, name)
            } else {
                self.propose_closed(read, &settings, name)
                    .map(|synced| changed.extend(synced))
            };
            match result {
                Ok(()) => proposed.applied.push(change.photo),
                Err(code) => proposed.refused.push(refuse(code)),
            }
        }
        if !changed.is_empty() {
            self.record_proposed(name, changed);
        }
        Ok(proposed)
    }

    /// The photo open in Develop: admitted only while Develop's state is what
    /// the token saw, applied through the guarded edit path as one step.
    fn propose_open(
        &mut self,
        read: &TokenPhoto,
        settings: &[(Param, f32)],
        name: &str,
    ) -> Result<(), ErrorCode> {
        if self.document.loading() || self.document.full().is_none() {
            return Err(ErrorCode::Busy);
        }
        let same = match read.open {
            Some((load, revision)) => {
                self.sync_command_revision();
                load == self.document.load_id() && revision == self.automation.revision
            }
            None => {
                let library = self.library.as_ref().ok_or(ErrorCode::StaleTarget)?;
                let now = library
                    .session
                    .catalog()
                    .photo_fingerprint(PhotoId(read.id))
                    .ok()
                    .flatten();
                !self.document.edit.save_state().needs_save()
                    && now.is_some()
                    && now == read.fingerprint
                    && read.file.is_some()
                    && read.file == self.document.file
            }
        };
        if !same {
            return Err(ErrorCode::StaleTarget);
        }
        self.apply_open_settings(settings, name)
            .map_err(|e| ErrorCode::Other(e.code.into()))
    }

    /// A photo not open: its file and catalog fingerprint checked against the
    /// token, then again inside the write.
    fn propose_closed(
        &mut self,
        read: &TokenPhoto,
        settings: &[(Param, f32)],
        name: &str,
    ) -> Result<Option<crate::app::library::sync::Synced>, ErrorCode> {
        let (Some(file), Some(fingerprint)) = (&read.file, &read.fingerprint) else {
            return Err(ErrorCode::StaleTarget);
        };
        let defaults = self.raw_defaults.clone();
        let decoding = crate::app::library::sync::Decoding {
            demosaic: self.session.demosaic,
            cache: Some(self.decode_cache.clone()),
        };
        let library = self.library.as_mut().ok_or(ErrorCode::StaleTarget)?;
        let photo = library
            .photo(PhotoId(read.id))
            .ok_or(ErrorCode::StaleTarget)?;
        let target = sync_target(photo);
        let change = BatchChange::Proposed(Box::new(ProposedSettings {
            name: name.to_string(),
            settings: settings.iter().map(|(p, v)| (p.spec(), *v)).collect(),
        }));
        match crate::app::library::sync::apply_if(
            &mut library.session,
            &change,
            &target,
            fingerprint,
            file,
            &defaults,
            &decoding,
        ) {
            Ok(AppliedIf::Changed(synced)) => Ok(Some(*synced)),
            Ok(AppliedIf::Unchanged) => Ok(None),
            Ok(AppliedIf::Moved) => Err(ErrorCode::StaleTarget),
            Err(e) => {
                self.messages
                    .warn(format!("{name}: {} not changed: {e:#}", target.name));
                Err(ErrorCode::Other("not_saved".into()))
            }
        }
    }

    /// Photos not open that a proposal changed: their previews, and one Undo
    /// for them all, as a Sync's.
    fn record_proposed(&mut self, name: &str, changed: Vec<crate::app::library::sync::Synced>) {
        let ids: Vec<PhotoId> = changed.iter().map(|s| s.id).collect();
        if let Some(library) = &mut self.library {
            library.edits_changed(ids.iter().copied());
        }
        self.refresh_previews(&ids);
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
