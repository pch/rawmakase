//! What Adobe's folders hold, as the bundled Lightroom extension
//! ([`LIGHTROOM`]) lists them with `files.list`. The extension knows where
//! Lightroom and Camera Raw keep things and what their layout means; the
//! editor decides what fits a photo or a catalog and imports it with its own
//! importers (the Profile menu, `app::catalog::onboarding`).
//!
//! The editor asks once and keeps the listing, drops it while the extension
//! is off, and asks again on [`AdobeFiles::relist`]. A listing from a
//! session that ended fails, and an answer the editor no longer waits for is
//! dropped.
use super::{Host, Outcome, Pending};
use crate::app::Editor;
use rawmakase_extensions::protocol::manifest::LIGHTROOM;
use rawmakase_extensions::protocol::method::{
    AppRequest, Empty, FileFolder, FileKind, FileOrigin, FilesListed,
};
use rawmakase_extensions::protocol::{ErrorCode, RemoteError};
use serde_json::Value;
use std::path::{Path, PathBuf};

impl Host {
    /// Whether the Lightroom extension is loaded and enabled.
    pub(in crate::app) fn lightroom_enabled(&self) -> bool {
        self.get(LIGHTROOM).is_some_and(super::Extension::enabled)
    }

    /// Asks for a listing, starting the program when it isn't running.
    /// Returns the ticket its [`Outcome::Files`] comes back with.
    pub(in crate::app) fn list_files(&mut self) -> Result<u64, String> {
        let ticket = self.next_ticket;
        self.next_ticket += 1;
        let request = AppRequest::FilesList(Empty {});
        self.send(LIGHTROOM, request, Pending::Files { ticket }, None)?;
        Ok(ticket)
    }
}

/// The program's answer to listing `ticket`, read.
pub(super) fn files_listed(ticket: u64, result: Result<Value, RemoteError>) -> Outcome {
    let result = result.and_then(|v| {
        serde_json::from_value::<FilesListed>(v)
            .map_err(|e| RemoteError::new(ErrorCode::InvalidRequest, e.to_string()))
    });
    Outcome::Files { ticket, result }
}

/// A file of a listing, or a preset folder, as the editor matches it.
pub(in crate::app) struct Entry<'a> {
    pub(in crate::app) folder: &'a FileFolder,
    /// The file, or the preset folder.
    pub(in crate::app) path: PathBuf,
    /// For a profile in Camera Raw's shared folder, the camera it is for.
    pub(in crate::app) camera: Option<&'a str>,
    pub(in crate::app) used: Option<u64>,
}

/// Each file of `listed`, and each preset folder.
pub(in crate::app) fn entries(listed: &FilesListed) -> impl Iterator<Item = Entry<'_>> {
    listed.folders.iter().flat_map(|folder| {
        let preset = folder.files.is_empty().then(|| Entry {
            folder,
            path: folder.path.clone(),
            camera: None,
            used: None,
        });
        let files = folder.files.iter().map(move |file| Entry {
            folder,
            path: folder.path.join(&file.name),
            camera: file.camera.as_deref().or(folder.camera.as_deref()),
            used: file.used,
        });
        preset.into_iter().chain(files)
    })
}

/// The editor's listing of Adobe's folders.
#[derive(Default)]
pub(in crate::app) struct AdobeFiles {
    state: Listing,
    /// Counts the listings taken and dropped, so what was worked out from one
    /// is worked out again.
    generation: u64,
    /// A first launch, which opens setup once a listing finds something;
    /// opening setup or finishing it decides it instead.
    pub(in crate::app) first_launch: bool,
    /// Adobe profiles **Import Adobe profiles for this camera** took this
    /// session, imported or skipped as unreadable: not offered again.
    pub(in crate::app) taken: std::collections::HashSet<PathBuf>,
}

#[derive(Default)]
enum Listing {
    #[default]
    Unlisted,
    Asked(u64),
    Listed(FilesListed),
    /// Why the listing failed, until it is asked for again.
    Failed(String),
}

impl AdobeFiles {
    pub(in crate::app) fn new(first_launch: bool) -> Self {
        Self {
            first_launch,
            ..Self::default()
        }
    }
    pub(in crate::app) fn generation(&self) -> u64 {
        self.generation
    }
    /// The listing taken, without asking for one.
    pub(in crate::app) fn listed(&self) -> Option<&FilesListed> {
        match &self.state {
            Listing::Listed(listed) => Some(listed),
            _ => None,
        }
    }
    /// Whether a listing was asked for and hasn't come.
    pub(in crate::app) fn asking(&self) -> bool {
        matches!(self.state, Listing::Asked(_))
    }
    /// Why the last listing failed.
    pub(in crate::app) fn failure(&self) -> Option<&str> {
        match &self.state {
            Listing::Failed(why) => Some(why),
            _ => None,
        }
    }
    /// Drops the listing, and lists again at the next use.
    pub(in crate::app) fn relist(&mut self) {
        self.state = Listing::Unlisted;
        self.generation += 1;
    }
}

impl Editor {
    /// The listing, asked for when there is none. None while the Lightroom
    /// extension is off (which drops the listing), until it answers, and when
    /// it failed.
    pub(in crate::app) fn adobe_files(&mut self) -> Option<&FilesListed> {
        let files = &mut self.adobe_files;
        if !self.extensions.lightroom_enabled() {
            if !matches!(files.state, Listing::Unlisted) {
                files.relist();
            }
            return None;
        }
        if matches!(files.state, Listing::Unlisted) {
            files.state = match self.extensions.list_files() {
                Ok(ticket) => Listing::Asked(ticket),
                Err(why) => Listing::Failed(why),
            };
        }
        files.listed()
    }

    /// Takes listing `ticket`'s outcome, unless the editor no longer waits
    /// for it. On a first launch, setup opens when Lightroom's are here (a
    /// catalog, presets, or Camera Raw's folders), and first-run setup is done
    /// otherwise.
    pub(in crate::app) fn files_listed(
        &mut self,
        ticket: u64,
        result: Result<FilesListed, RemoteError>,
    ) {
        let files = &mut self.adobe_files;
        if !matches!(files.state, Listing::Asked(t) if t == ticket) {
            return;
        }
        let listed = match result {
            Ok(listed) => listed,
            Err(why) => {
                files.state = Listing::Failed(why.message);
                return;
            }
        };
        files.generation += 1;
        if std::mem::take(&mut files.first_launch) {
            let here = !listed.folders.is_empty()
                || listed.searched.iter().any(|s| s.kind != FileKind::Catalog);
            self.onboarding.visible = here;
            self.session.onboarding_done = !here;
        }
        files.state = Listing::Listed(listed);
    }

    /// Adobe's profiles in the listing for the camera of photo `m` that aren't
    /// imported yet: what **Import Adobe profiles for this camera** imports.
    pub(in crate::app) fn adobe_profiles_for(
        &self,
        m: &crate::camera_data::Metadata,
    ) -> Vec<PathBuf> {
        let library = crate::storage::data_dir().join("camera-profiles");
        let Some(listed) = self.adobe_files.listed() else {
            return Vec::new();
        };
        let mut offered = camera_profiles_for(listed, m, &library);
        offered.retain(|p| !self.adobe_files.taken.contains(p));
        offered
    }
}

/// Of `listed`, Adobe's camera profiles that fit photo `m` and aren't in
/// `library`.
fn camera_profiles_for(
    listed: &FilesListed,
    m: &crate::camera_data::Metadata,
    library: &Path,
) -> Vec<PathBuf> {
    let adobe: Vec<(&str, PathBuf)> = entries(listed)
        .filter(|e| {
            e.folder.kind == FileKind::CameraProfile && e.folder.origin == FileOrigin::Shared
        })
        .filter_map(|e| Some((e.camera?, e.path)))
        .collect();
    let adobe = adobe.iter().map(|(camera, path)| (*camera, path.as_path()));
    crate::camera_profiles::adobe_offered(adobe, m, library)
}

#[cfg(test)]
#[path = "adobe_files_tests.rs"]
pub(in crate::app) mod tests;
