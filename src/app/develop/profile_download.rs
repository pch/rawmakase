//! Community camera profiles from the catalogue (`camera_profiles::catalogue`):
//! the offer under Develop's Profile menu, downloads, and applying one.
//!
//! The catalogue has three owners. The model's `camera_profiles::catalogue` owns
//! the checked-in data (`data/camera-profiles`), its validation, matching by the
//! same camera rule the profile library lists by, verification of downloaded bytes,
//! installation through the no-clobber importer and the generator; it never reaches
//! the network. This module owns the HTTP request, the task and what the offer shows
//! for the open photo: which profiles match, measured once per camera and profile
//! library rather than every frame, when it shows (by default while the photo keeps
//! its raw defaults' profile and no Adobe profile is imported), and the per-camera
//! dismissal the session keeps. `app::develop::panels::inspector::profile_offer` draws the offer under
//! Basic › Profile and only reports clicks.
//!
//! A download is a library operation, independent of the open document: it
//! captures its profile, camera metadata and destination once, and navigation does
//! not cancel it. Its completion calls `bulk_import::list_profiles_after_import`,
//! the refresh a manual camera-profile import runs, without touching a running
//! import's state or status line, so profiles, raw defaults (with the fitted default
//! kept for a photo without colour data), preset support, Library previews (keeping
//! only the unchanged open photo's) and the reference refresh exactly as after a
//! manual import. Only then is the profile applied, and only to the photo it was
//! asked for, if that photo is still open (same load), its file still has the
//! [`Identity`] it was opened with (checked when the download starts and again at
//! completion: an outside replacement or deletion causes no new load), still has the
//! profile it had and no camera-look fit was asked for since: one History step
//! through `change_edit`, as a Profile menu choice is. Otherwise it is only
//! installed.
//!
//! What applies is `Entry::read_installed`: the installed file read once, bounded,
//! and verified as the download was, so a file substituted after installation is
//! never applied or credited. An installed profile's Apply is clicked during
//! Develop's edit frame and changes the profile in place, as the menu does, so the
//! frame records its one step. Measuring keeps each installed profile's verified
//! content, which says whether the photo uses it: sources may share a profile name
//! and camera. Cancellation and publication claim one atomic state, so a published
//! install cannot be reported as cancelled. Task generations reject cancelled,
//! superseded and duplicate completions; a panicking worker reports a failure
//! through the task wrapper, which clears the busy state.
use crate::app::{Editor, task, worker::Event};
use crate::camera_data::Metadata;
use crate::camera_profiles::CameraProfile;
use crate::camera_profiles::catalogue::{Entry, MAX_BYTES, Status, catalogue};
use crate::develop::black_white::TreatmentChoice;
use crate::model::recipe::Recipe;
use crate::storage::Identity;
use anyhow::{Context, Result, ensure};
use eframe::egui;
use rawmakase_cancel::Cancel;
use std::{
    collections::{BTreeSet, HashSet},
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};

#[derive(Default)]
pub(in crate::app) struct Downloads {
    pub(in crate::app) task: task::Task,
    job: Option<Job>,
    progress: Arc<AtomicUsize>,
    /// 0 downloading, 1 publishing, 2 cancelled. Claiming publication and
    /// cancellation are exclusive, so Cancel can never report a published file
    /// as cancelled. Once publication starts the UI waits for its result.
    publication: Arc<AtomicU8>,
    /// The last download's failure, by catalogue profile id, for its Retry.
    failure: Option<(&'static str, String)>,
    /// Cameras whose offer the user dismissed, kept with the session: dismissing
    /// one camera's offer leaves every other camera's.
    dismissed: BTreeSet<String>,
    /// Cameras whose offer was asked for from the Profile menu this session: shown
    /// even where it would stay quiet, dismissed or not.
    requested: HashSet<String>,
    /// The open photo's catalogue profiles and where they are installed, measured
    /// when its camera or the profile library changes rather than every frame.
    measured: Option<Measured>,
}

struct Measured {
    key: (String, String),
    profiles: Vec<Listed>,
}

/// A catalogue profile for the open photo, as measured.
struct Listed {
    entry: &'static Entry,
    status: Status,
    /// The installed file's profile, read and verified as it was measured: what
    /// tells whether the photo uses this one, not its name, which another source's
    /// profile for the camera may share.
    installed: Option<Arc<CameraProfile>>,
}

impl Listed {
    fn measure(entry: &'static Entry, status: Status, m: &Metadata) -> Self {
        let installed = match &status {
            Status::Installed(path) => entry.read_installed(path, m).ok().map(Arc::new),
            _ => None,
        };
        Self {
            entry,
            status,
            installed,
        }
    }
}

/// A running download and the photo it was asked for.
struct Job {
    entry: &'static Entry,
    origin: Origin,
}

/// The open photo when its offer was taken: the load, its file, the profile it
/// had, and the camera-look fits asked for so far. Any change means the user moved
/// on. `file` is the photo's file as opened, checked still there then: `None`, if
/// it couldn't be, and the download only installs.
struct Origin {
    load: u64,
    file: Option<Identity>,
    profile: Option<Arc<CameraProfile>>,
    fits: u64,
}

/// What the offer under the Profile menu shows this frame.
pub(in crate::app) struct Offer {
    pub(in crate::app) profiles: Vec<Offered>,
    /// The running download: its profile, the fraction done, and whether it can
    /// still be cancelled.
    pub(in crate::app) running: Option<(&'static str, f32, bool)>,
}

pub(in crate::app) struct Offered {
    pub(in crate::app) entry: &'static Entry,
    pub(in crate::app) status: Status,
    pub(in crate::app) in_use: bool,
    pub(in crate::app) failure: Option<String>,
}

/// What the user asked of the offer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) enum OfferAction {
    Download(&'static str),
    Apply(&'static str),
    Cancel,
    Dismiss,
}

/// What the measured profiles depend on: the names the photo's camera matches by,
/// and the profile library.
fn measure_key(m: &Metadata, library: &str) -> (String, String) {
    (
        format!(
            "{}\0{}\0{}\0{}",
            m.make,
            m.model,
            m.adobe_names.join("\0"),
            m.is_rendered()
        ),
        library.to_string(),
    )
}

/// The camera an offer is dismissed or asked for by.
fn camera_key(m: &Metadata) -> String {
    format!("{} {}", m.make.trim(), m.model.trim()).to_lowercase()
}

impl Downloads {
    pub(in crate::app) fn cancel(&mut self) {
        if self.task.is_running()
            && self
                .publication
                .compare_exchange(0, 2, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
        {
            self.task.invalidate();
            self.job = None;
        }
    }

    /// The profile being downloaded, if any.
    pub(in crate::app) fn downloading(&self) -> Option<&'static str> {
        let job = self.job.as_ref().filter(|_| self.task.is_running())?;
        Some(job.entry.profile.name.as_str())
    }

    fn cancellable(&self) -> bool {
        self.task.is_running() && self.publication.load(Ordering::SeqCst) == 0
    }

    /// The offer state the session keeps: the cameras whose offer was dismissed.
    pub(in crate::app) fn with_dismissed(dismissed: BTreeSet<String>) -> Self {
        Self {
            dismissed,
            ..Default::default()
        }
    }

    pub(in crate::app) fn dismissed(&self) -> &BTreeSet<String> {
        &self.dismissed
    }

    /// Shows the open camera's offer again, even where it would stay quiet.
    pub(in crate::app) fn request(&mut self, m: &Metadata) {
        // An explicit search also rechecks files changed outside the app.
        self.measured = None;
        self.requested.insert(camera_key(m));
    }

    fn dismiss(&mut self, m: &Metadata) {
        let camera = camera_key(m);
        self.requested.remove(&camera);
        self.dismissed.insert(camera);
    }

    /// The catalogue's profiles for photo `m`, with where each is installed.
    /// Measured once per camera and profile library (`library`, its fingerprint).
    fn measure(&mut self, m: &Metadata, library: &str) -> &[Listed] {
        let key = measure_key(m, library);
        if self.measured.as_ref().is_none_or(|c| c.key != key) {
            let profiles = catalogue()
                .matching(m)
                .into_iter()
                .map(|e| Listed::measure(e, e.status(), m))
                .collect();
            self.measured = Some(Measured { key, profiles });
        }
        &self.measured.as_ref().expect("measured above").profiles
    }
}

impl Editor {
    /// Whether the open photo has catalogue profiles: a RAW of a camera the
    /// catalogue covers. Only then does the Profile menu offer them.
    pub(in crate::app) fn has_catalogue_profiles(&mut self) -> bool {
        let library = self.profile_library.clone();
        match &self.document.metadata {
            Some(m) => !self.profile_download.measure(m, &library).is_empty(),
            None => false,
        }
    }

    /// The offer for the open photo, if it shows this frame. It shows by default
    /// while the photo has the profile its defaults gave it (no profile chosen by
    /// hand), and stays quiet for a rendered file, a camera the catalogue doesn't
    /// cover, a camera whose offer the user dismissed, and where it is no use: the
    /// photo uses one of the profiles, or Adobe's profiles for the camera are
    /// imported, which take precedence. Asking for it from the Profile menu shows
    /// it anyway.
    pub(in crate::app) fn profile_offer(&mut self) -> Option<Offer> {
        let m = self.document.metadata.clone()?;
        let library = self.profile_library.clone();
        let current = self.document.edit.recipe().profile.clone();
        let camera = camera_key(&m);
        let requested = self.profile_download.requested.contains(&camera);
        let dismissed = self.profile_download.dismissed.contains(&camera);
        let adobe = self
            .document
            .profiles
            .iter()
            .any(|p| p.name == "Adobe Standard");
        let chosen_by_hand = !self.has_default_profile();
        let failure = self.profile_download.failure.clone();
        let running = self.profile_download.job.as_ref().map(|job| {
            (
                job.entry.profile.id.as_str(),
                self.profile_download.progress.load(Ordering::Relaxed) as f32
                    / job.entry.profile.size as f32,
                self.profile_download.cancellable(),
            )
        });
        let profiles: Vec<Offered> = self
            .profile_download
            .measure(&m, &library)
            .iter()
            .map(|listed| Offered {
                entry: listed.entry,
                status: listed.status.clone(),
                in_use: current
                    .as_ref()
                    .zip(listed.installed.as_ref())
                    .is_some_and(|(used, installed)| used == installed),
                failure: failure
                    .as_ref()
                    .filter(|(id, _)| *id == listed.entry.profile.id)
                    .map(|(_, why)| why.clone()),
            })
            .collect();
        if profiles.is_empty() {
            return None;
        }
        let downloading = running
            .as_ref()
            .is_some_and(|(id, ..)| profiles.iter().any(|p| p.entry.profile.id == *id));
        let quiet = if requested || downloading {
            false
        } else {
            dismissed || adobe || chosen_by_hand || profiles.iter().any(|p| p.in_use)
        };
        (!quiet).then_some(Offer { profiles, running })
    }

    /// Whether the open photo's profile is the one its raw defaults give it: the
    /// user hasn't chosen another.
    fn has_default_profile(&self) -> bool {
        let current = &self.document.edit.recipe().profile;
        let same = |default: &Option<Arc<CameraProfile>>| match (current, default) {
            (Some(a), Some(b)) => Arc::ptr_eq(a, b) || a == b,
            (None, None) => true,
            _ => false,
        };
        match &self.document.defaults {
            Some(defaults) => same(&defaults.recipe.profile),
            None => self
                .photo_defaults()
                .is_some_and(|defaults| same(&defaults.recipe.profile)),
        }
    }

    /// Does what the offer was asked. Called during Develop's edit frame, which
    /// records an Apply as one History step, as it does a choice in the Profile menu.
    pub(in crate::app) fn profile_offer_action(
        &mut self,
        action: OfferAction,
        ctx: &egui::Context,
    ) {
        let Some(m) = self.document.metadata.clone() else {
            return;
        };
        // What the offer listed for this photo.
        let listed = |measured: &Option<Measured>, id: &str| {
            measured.as_ref().and_then(|c| {
                c.profiles
                    .iter()
                    .find(|l| l.entry.profile.id == id && l.entry.fits(&m))
                    .map(|l| (l.entry, l.status.clone()))
            })
        };
        match action {
            OfferAction::Dismiss => {
                self.profile_download.dismiss(&m);
                let _ = self.save_session();
            }
            OfferAction::Cancel => self.profile_download.cancel(),
            OfferAction::Download(id) => {
                if let Some((entry, _)) = listed(&self.profile_download.measured, id) {
                    self.download_profile(entry, ctx);
                }
            }
            OfferAction::Apply(id) => {
                // Installed already, as measured: its file is checked again now,
                // with no network.
                let Some((entry, status)) = listed(&self.profile_download.measured, id) else {
                    return;
                };
                let verified = match status {
                    Status::Installed(path) => entry.read_installed(&path, &m).ok(),
                    _ => None,
                };
                let Some(profile) = verified else {
                    self.profile_download.measured = None;
                    self.messages
                        .warn(format!("{} is no longer installed", entry.profile.name));
                    return;
                };
                let profile = self.listed_profile(profile);
                let old = choose(self.document.edit.recipe_mut(), profile, &m);
                self.follow_profile_treatment(old.as_deref());
                self.applied(entry);
            }
        }
    }

    pub(in crate::app) fn download_profile(&mut self, entry: &'static Entry, ctx: &egui::Context) {
        self.download_profile_using(entry, ctx, fetch);
    }

    fn download_profile_using(
        &mut self,
        entry: &'static Entry,
        ctx: &egui::Context,
        transport: fn(&Entry, Cancel<'_>, &AtomicUsize) -> Result<Vec<u8>>,
    ) {
        if self.profile_download.task.is_running() {
            return;
        }
        // The import publishes into the same library; its result decides what this
        // one would be.
        if self.importing.is_some() {
            self.messages.warn(format!(
                "{} was not downloaded: wait for the import to finish",
                entry.profile.name
            ));
            return;
        }
        let Some(metadata) = self.document.metadata.clone().filter(|m| entry.fits(m)) else {
            return;
        };
        let destination = crate::storage::data_dir().join("camera-profiles");
        let (id, cancel) = self.profile_download.task.start();
        let progress = Arc::new(AtomicUsize::new(0));
        let publication = Arc::new(AtomicU8::new(0));
        self.profile_download.publication = publication.clone();
        self.profile_download.progress = progress.clone();
        self.profile_download.failure = None;
        // The file as opened, if it still is: read once now, and again only if the
        // download finishes for this photo.
        let file = self.document.file.clone().filter(|opened| {
            self.document
                .path
                .as_deref()
                .and_then(|p| Identity::read(p).ok())
                .as_ref()
                == Some(opened)
        });
        self.profile_download.job = Some(Job {
            entry,
            origin: Origin {
                load: self.document.load_id(),
                file,
                profile: self.document.edit.recipe().profile.clone(),
                fits: self.document.camera_look.id(),
            },
        });
        task::spawn(
            self.tx.clone(),
            ctx.clone(),
            move |tx| {
                let result = (|| {
                    let bytes = transport(entry, Cancel::new(&cancel), &progress)?;
                    entry.verify(&bytes, &metadata)?;
                    ensure!(
                        !cancel.load(Ordering::Relaxed)
                            && publication
                                .compare_exchange(0, 1, Ordering::SeqCst, Ordering::SeqCst)
                                .is_ok(),
                        "Profile download cancelled"
                    );
                    entry.install_into(&bytes, &metadata, &destination)
                })()
                .map_err(|e: anyhow::Error| format!("{e:#}"));
                let _ = tx.send(Event::ProfileDownloaded { id, result });
            },
            move |tx, error| {
                let _ = tx.send(Event::ProfileDownloaded {
                    id,
                    result: Err(error),
                });
            },
        );
    }

    pub(in crate::app) fn profile_downloaded(&mut self, id: u64, result: Result<PathBuf, String>) {
        // A cancellation, retry or duplicate completion cannot finish another job.
        if !self.profile_download.task.is_running() || id != self.profile_download.task.id() {
            return;
        }
        self.profile_download.task.finish(id);
        let Some(job) = self.profile_download.job.take() else {
            return;
        };
        self.profile_download.measured = None;
        let name = &job.entry.profile.name;
        match result {
            Err(why) => {
                self.messages
                    .fail(format!("{name} was not installed: {why}"));
                self.profile_download.failure = Some((&job.entry.profile.id, why));
            }
            Ok(path) => {
                // Decided before the refresh, which may give an untouched photo
                // new defaults: that is not the user moving on.
                let apply = self.still_asked_for(&job);
                // As a manual import refreshes, without its summary or its state:
                // a lens profile import may be running meanwhile.
                self.list_profiles_after_import();
                if apply {
                    self.apply_downloaded(job.entry, &path);
                } else {
                    self.messages.note(format!(
                        "{name} installed: choose it in the Profile menu of a matching photo"
                    ));
                }
            }
        }
    }

    /// Whether the open photo is still the one the download was asked for, as it
    /// was: the same load of the same file, unchanged on disk (an outside program
    /// can replace or delete it with no new load), with the profile it had and no
    /// camera-look fit asked for since.
    fn still_asked_for(&self, job: &Job) -> bool {
        let origin = &job.origin;
        origin.load == self.document.load_id()
            && self.document.edit.recipe().profile == origin.profile
            && self.document.camera_look.id() == origin.fits
            && self
                .document
                .metadata
                .as_ref()
                .is_some_and(|m| job.entry.fits(m))
            && origin.file.as_ref().is_some_and(|file| {
                self.document.file.as_ref() == Some(file)
                    && self
                        .document
                        .path
                        .as_deref()
                        .and_then(|p| Identity::read(p).ok())
                        .as_ref()
                        == Some(file)
            })
    }

    /// Uses the installed download at `path` as the open photo's, outside an edit
    /// frame, as one History step. Its file is read once and verified as the
    /// catalogue's, so a file put in its place since is never applied or credited.
    fn apply_downloaded(&mut self, entry: &Entry, path: &Path) {
        let Some(m) = self.document.metadata.clone() else {
            return;
        };
        let profile = match entry.read_installed(path, &m) {
            Ok(profile) => self.listed_profile(profile),
            Err(_) => {
                self.messages.warn(format!(
                    "{} was installed, but its file changed before it could be used: it was not applied",
                    entry.profile.name
                ));
                return;
            }
        };
        self.change_edit(None, |r| {
            let old = choose(r, profile, &m);
            r.follow_profile_treatment(old.as_deref(), None);
        });
        self.applied(entry);
    }

    /// The listed profile `profile` is, so the menu shows it chosen.
    fn listed_profile(&self, profile: CameraProfile) -> Arc<CameraProfile> {
        self.document
            .profiles
            .iter()
            .find(|p| ***p == profile)
            .cloned()
            .unwrap_or_else(|| Arc::new(profile))
    }

    fn applied(&mut self, entry: &Entry) {
        self.messages.done(format!(
            "{} applied, from {}",
            entry.profile.name, entry.source.name
        ));
    }
}

/// Makes `profile` the photo's at 100%, as choosing it in the Profile menu does:
/// Temp and Tint follow the new profile's white. Returns the old profile, for the
/// Treatment to follow.
fn choose(r: &mut Recipe, profile: Arc<CameraProfile>, m: &Metadata) -> Option<Arc<CameraProfile>> {
    let old = r.profile.replace(profile);
    r.profile_amount = 1.;
    r.profile_changed(m);
    old
}

fn fetch(entry: &Entry, cancel: Cancel<'_>, progress: &AtomicUsize) -> Result<Vec<u8>> {
    ensure!(!cancel.is_cancelled(), "Profile download cancelled");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .max_redirects(0)
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(15)))
        .timeout_recv_body(Some(Duration::from_secs(20)))
        .timeout_global(Some(Duration::from_secs(30)))
        .build()
        .into();
    let mut response = agent
        .get(&entry.url())
        .call()
        .context("Could not download the camera profile")?;
    ensure!(
        response.status() == 200,
        "The profile's server returned {}",
        response.status()
    );
    read_body(
        response.body_mut().as_reader(),
        entry.profile.size,
        cancel,
        progress,
    )
}

/// Read at most the pinned size plus one byte, even if the host sends no length
/// or sends more than it promised. Cancellation is checked between bounded reads.
fn read_body(
    mut reader: impl Read,
    size: u64,
    cancel: Cancel<'_>,
    progress: &AtomicUsize,
) -> Result<Vec<u8>> {
    ensure!(size <= MAX_BYTES, "Profile exceeds the download limit");
    let size = size as usize;
    let mut bytes = Vec::with_capacity(size);
    let mut chunk = [0u8; 16 * 1024];
    loop {
        ensure!(!cancel.is_cancelled(), "Profile download cancelled");
        let limit = chunk.len().min(size + 1 - bytes.len());
        let n = reader
            .read(&mut chunk[..limit])
            .context("Profile download interrupted")?;
        if n == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..n]);
        ensure!(
            bytes.len() <= size,
            "Profile download exceeds its expected size"
        );
        progress.store(bytes.len(), Ordering::Relaxed);
    }
    ensure!(!cancel.is_cancelled(), "Profile download cancelled");
    ensure!(bytes.len() == size, "Profile download is incomplete");
    Ok(bytes)
}

#[cfg(test)]
impl Editor {
    /// Starts downloading the open photo's first catalogue profile with a transport
    /// that delivers nothing and waits until cancelled: for tests of the offer.
    pub(in crate::app) fn download_waiting_for_cancel(&mut self) {
        let m = self.document.metadata.clone().expect("a photo");
        let entry = catalogue().matching(&m)[0];
        let ctx = self.context.clone();
        self.download_profile_using(entry, &ctx, tests::waits);
    }

    /// Measures the open photo's catalogue profiles as installed nowhere, so a test
    /// doesn't depend on this machine's profile library.
    pub(in crate::app) fn assume_catalogue_profiles_available(&mut self) {
        let m = self.document.metadata.clone().expect("a photo");
        let profiles = catalogue()
            .matching(&m)
            .into_iter()
            .map(|e| (e, Status::Available))
            .collect();
        self.assume_catalogue_profiles(profiles);
    }

    /// Measures the open photo's catalogue profiles as `profiles`, as found where
    /// each says: what the offer lists, without this machine's profile library.
    /// An installed one's file is read as measuring reads it.
    pub(in crate::app) fn assume_catalogue_profiles(
        &mut self,
        profiles: Vec<(&'static Entry, Status)>,
    ) {
        let m = self.document.metadata.clone().expect("a photo");
        self.profile_download.measured = Some(Measured {
            key: measure_key(&m, &self.profile_library),
            profiles: profiles
                .into_iter()
                .map(|(e, status)| Listed::measure(e, status, &m))
                .collect(),
        });
    }
}

#[cfg(test)]
#[path = "profile_download_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "profile_download_editor_tests.rs"]
mod editor_tests;
