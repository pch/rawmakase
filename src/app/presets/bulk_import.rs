//! Imports camera profiles, lens profiles and presets from chosen files or
//! whole folders, leniently: folders are scanned recursively, files already
//! imported are skipped quietly, and what can't be imported is reported
//! instead of failing the rest. A preset import also takes the look profiles
//! among its files, as packs ship them beside the presets that use them.
use crate::app::{Editor, worker::Event};
use crate::camera_profiles::LutSpace;
use eframe::egui;
use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ImportKind {
    /// DCPs, look XMPs and `.cube` LUTs, the LUTs at the sRGB default.
    CameraProfiles,
    /// `.cube` LUTs only, expecting colours in this space (Import .cube LUTs).
    Luts(LutSpace),
    LensProfiles,
    Presets,
}
impl ImportKind {
    fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::CameraProfiles => &["dcp", "xmp", "cube"],
            Self::Luts(_) => &["cube"],
            Self::LensProfiles => &["lcp"],
            Self::Presets => &["xmp"],
        }
    }
    pub(in crate::app) fn noun(self, count: usize) -> &'static str {
        match (self, count == 1) {
            (Self::CameraProfiles, true) => "profile",
            (Self::CameraProfiles, false) => "profiles",
            (Self::Luts(_), true) => "LUT",
            (Self::Luts(_), false) => "LUTs",
            (Self::LensProfiles, true) => "lens profile",
            (Self::LensProfiles, false) => "lens profiles",
            (Self::Presets, true) => "preset",
            (Self::Presets, false) => "presets",
        }
    }
    /// Label for the multi-file chooser's filter.
    pub(in crate::app) fn filter(self) -> (&'static str, &'static [&'static str]) {
        match self {
            Self::CameraProfiles => ("Camera profiles and .cube LUTs", &["dcp", "xmp", "cube"]),
            Self::Luts(_) => ("LUTs (.cube)", &["cube"]),
            Self::LensProfiles => ("Adobe lens profiles", &["lcp"]),
            Self::Presets => ("Lightroom presets", &["xmp"]),
        }
    }
}

/// Files with one of `extensions` under `dir`, in sorted order. Skips hidden
/// folders, macOS archive leftovers and, when `skip` names them, other
/// folders such as Camera Raw's Defaults.
pub(in crate::app) fn find_files(dir: &Path, extensions: &[&str], skip: &[&str]) -> Vec<PathBuf> {
    fn walk(dir: &Path, extensions: &[&str], skip: &[&str], depth: usize, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') {
                continue;
            }
            // Follows links, so the depth limit also stops link cycles.
            if path.is_dir() {
                if depth < 12 && name != "__MACOSX" && !skip.contains(&name.as_ref()) {
                    walk(&path, extensions, skip, depth + 1, out);
                }
            } else if path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
            {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, extensions, skip, 0, &mut out);
    out.sort();
    out
}

/// A file to import.
#[derive(Clone)]
struct Found {
    path: PathBuf,
    /// Its folder under the chosen one, starting with the chosen folder's
    /// name. Presets keep it: a preset without a group is grouped by folder.
    folder: PathBuf,
    /// Chosen by name rather than found in a folder. A chosen file of the
    /// wrong kind is reported; a found one is passed over quietly.
    chosen: bool,
}

/// The folder of an Adobe account's presets in Lightroom's library.
pub(in crate::app) const LIGHTROOM_PRESETS: &str = "cr_settings";

/// Where Lightroom's presets in `dir` are imported, if `dir` is an account's
/// [`LIGHTROOM_PRESETS`]: Lightroom (not Classic) shows those without a group
/// as User Presets, and a preset without a group takes its folder's name.
fn lightroom_presets_folder(kind: ImportKind, dir: &Path) -> Option<PathBuf> {
    (kind == ImportKind::Presets && dir.file_name().is_some_and(|n| n == LIGHTROOM_PRESETS))
        .then(|| PathBuf::from("User Presets"))
}

fn gather(kind: ImportKind, picks: &[PathBuf]) -> Vec<Found> {
    let skip: &[&str] = match kind {
        // Camera Raw's and Lightroom's own settings, not presets.
        ImportKind::Presets => &["Defaults", "GPU", "cr_defaults"],
        _ => &[],
    };
    let mut found = Vec::new();
    for pick in picks {
        if pick.is_dir() {
            let root = pick.file_name().map(PathBuf::from).unwrap_or_default();
            for path in find_files(pick, kind.extensions(), skip) {
                let parent = path.parent().unwrap_or(Path::new(""));
                let under = parent.strip_prefix(pick).unwrap_or(Path::new(""));
                found.push(Found {
                    folder: lightroom_presets_folder(kind, parent)
                        .unwrap_or_else(|| root.join(under)),
                    path,
                    chosen: false,
                });
            }
        } else {
            found.push(Found {
                folder: pick
                    .parent()
                    .and_then(|p| {
                        lightroom_presets_folder(kind, p)
                            .or_else(|| p.file_name().map(PathBuf::from))
                    })
                    .unwrap_or_default(),
                path: pick.clone(),
                chosen: true,
            });
        }
    }
    found
}

/// What an import did.
#[derive(Debug)]
pub struct Summary {
    pub(in crate::app) kind: ImportKind,
    pub(in crate::app) imported: usize,
    /// Look profiles a preset import took from among its files.
    pub(in crate::app) profiles: usize,
    /// Identical to one already imported, or repeated within this import.
    pub(in crate::app) already: usize,
    /// Profile looks found in a folder for cameras without a base profile.
    pub(in crate::app) without_base: usize,
    /// Files that couldn't be imported, with the reason.
    pub(in crate::app) failed: Vec<(PathBuf, String)>,
    /// What became of each `.cube`: its look's name and how faithfully it holds
    /// the cube.
    pub(in crate::app) cubes: Vec<(PathBuf, crate::camera_profiles::CubeImport)>,
}
impl Summary {
    pub(in crate::app) fn new(kind: ImportKind) -> Self {
        Self {
            kind,
            imported: 0,
            profiles: 0,
            already: 0,
            without_base: 0,
            failed: Vec::new(),
            cubes: Vec::new(),
        }
    }
    fn fail(&mut self, path: &Path, reason: impl Into<String>) {
        self.failed.push((path.to_path_buf(), reason.into()));
    }
    /// One line: "Imported 40 presets · 3 already imported · 2 skipped".
    pub fn message(&self) -> String {
        let (imported, already, failed) = (self.imported, self.already, self.failed.len());
        if imported + self.profiles + already + failed + self.without_base == 0 {
            return format!("No {} found there", self.kind.noun(2));
        }
        let mut parts = vec![format!("Imported {imported} {}", self.kind.noun(imported))];
        if self.profiles > 0 {
            let noun = ImportKind::CameraProfiles.noun(self.profiles);
            parts.push(format!("{} {noun}", self.profiles));
        }
        // A LUT import names its space, unless it is the sRGB default.
        if let ImportKind::Luts(space) = self.kind
            && space != LutSpace::Srgb
        {
            parts[0] += &format!(" in {space}");
        }
        if already > 0 {
            parts.push(format!("{already} already imported"));
        }
        if self.without_base > 0 {
            parts.push(format!(
                "{} looks skipped for cameras without a profile",
                self.without_base
            ));
        }
        let fresh = || self.cubes.iter().map(|(_, c)| c).filter(|c| !c.already);
        let approximated = fresh()
            .filter(|c| c.fidelity != crate::camera_profiles::Fidelity::Exact)
            .count();
        if approximated > 0 {
            let luts = match (self.kind, approximated) {
                // Imported LUTs, said already.
                (ImportKind::Luts(_), _) => "",
                (_, 1) => " LUT",
                _ => " LUTs",
            };
            parts.push(format!("{approximated}{luts} approximated"));
        }
        let renamed = fresh().filter(|c| c.renamed()).count();
        if renamed > 0 {
            parts.push(format!("{renamed} renamed"));
        }
        if failed > 0 {
            parts.push(format!("{failed} skipped"));
        }
        parts.join(" · ")
    }
    /// The skipped files, then the `.cube` files approximated or renamed, one per
    /// line, at most `limit` of them.
    pub(in crate::app) fn details(&self, limit: usize) -> String {
        let noted = self.cubes.iter().filter(|(_, c)| {
            !c.already && (c.renamed() || c.fidelity != crate::camera_profiles::Fidelity::Exact)
        });
        let all: Vec<String> = self
            .failed
            .iter()
            .map(|(path, reason)| line(path, reason))
            .chain(noted.map(|(path, cube)| line(path, cube)))
            .collect();
        let mut lines: Vec<String> = all.iter().take(limit).cloned().collect();
        if all.len() > limit {
            lines.push(format!("…and {} more", all.len() - limit));
        }
        lines.join("\n")
    }
    /// Whether [`Self::details`] has anything to say.
    pub(in crate::app) fn has_details(&self) -> bool {
        !self.details(1).is_empty()
    }
    /// Every `.cube` and every file that couldn't be imported, one line each.
    pub fn lines(&self) -> Vec<String> {
        self.cubes
            .iter()
            .map(|(path, cube)| line(path, cube))
            .chain(self.failed.iter().map(|(path, reason)| line(path, reason)))
            .collect()
    }
    /// Files that couldn't be imported.
    pub fn failures(&self) -> usize {
        self.failed.len()
    }
    /// Files imported, or found imported already.
    pub fn done(&self) -> usize {
        self.imported + self.profiles + self.already
    }
}
fn line(path: &Path, what: impl std::fmt::Display) -> String {
    format!(
        "{}: {what}",
        path.file_name().unwrap_or_default().to_string_lossy()
    )
}

fn content_hash(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}
/// Camera Raw profile looks are XMP files like presets, marked as looks.
fn is_look(text: &str) -> bool {
    text.contains("PresetType=\"Look\"") || text.contains(">Look</crs:PresetType>")
}
/// What an import is doing, for the status line. Setting it wakes the interface,
/// so the status line follows without polling.
#[derive(Default)]
pub(in crate::app) struct ImportProgress {
    text: Mutex<String>,
    ctx: Option<egui::Context>,
}
impl ImportProgress {
    fn new(ctx: egui::Context) -> Self {
        Self {
            text: Mutex::default(),
            ctx: Some(ctx),
        }
    }
    fn set(&self, text: String) {
        *self
            .text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = text;
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint();
        }
    }
    pub(in crate::app) fn text(&self) -> String {
        self.text
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}
fn report(progress: &ImportProgress, i: usize, total: usize, noun: &str) {
    if i.is_multiple_of(50) {
        progress.set(format!("Checking {noun}… {i} of {total}"));
    }
}

/// Checks a profile against the library folder and this import's earlier
/// files: true to import it; counts it as already imported or fails it.
fn check_name(
    found: &Found,
    bytes: &[u8],
    destination: &Path,
    names: &mut BTreeMap<OsString, u64>,
    summary: &mut Summary,
) -> bool {
    let Some(name) = found.path.file_name() else {
        summary.fail(&found.path, "no file name");
        return false;
    };
    let hash = content_hash(bytes);
    if let Some(&seen) = names.get(name) {
        if seen == hash {
            summary.already += 1;
        } else {
            summary.fail(&found.path, "another file in this import has the same name");
        }
        return false;
    }
    names.insert(name.to_owned(), hash);
    let target = destination.join(name);
    if target.exists() {
        if std::fs::read(&target).is_ok_and(|existing| existing == bytes) {
            summary.already += 1;
        } else {
            summary.fail(
                &found.path,
                "a different one with this name is already imported",
            );
        }
        return false;
    }
    true
}

/// The largest DCP or look XMP the profile library imports
/// (`camera_profiles::import_files`).
const PROFILE_LIMIT: u64 = 16_000_000;

/// The user's profile library, which profile imports write to.
fn profile_library() -> PathBuf {
    crate::storage::data_dir().join("camera-profiles")
}

/// Imports the profiles and LUTs `found` holds for `kind` (camera profiles or LUTs
/// only) into the profile library `destination`, the LUTs as expecting colours in
/// `space`.
fn import_camera_profiles(
    found: &[Found],
    progress: &ImportProgress,
    kind: ImportKind,
    space: LutSpace,
    destination: &Path,
) -> Summary {
    let mut summary = Summary::new(kind);
    let mut names = BTreeMap::new();
    let (mut dcps, mut looks, mut cubes) = (Vec::new(), Vec::new(), Vec::new());
    for (i, file) in found.iter().enumerate() {
        report(progress, i, found.len(), "profiles");
        let Ok(bytes) = std::fs::read(&file.path) else {
            summary.fail(&file.path, "unreadable");
            continue;
        };
        let extension = |e: &str| {
            file.path
                .extension()
                .is_some_and(|x| x.eq_ignore_ascii_case(e))
        };
        let xmp = extension("xmp");
        // A LUT import takes cubes only; a folder's other files are left out when
        // found, and a profile chosen by name is said to be the wrong kind.
        if matches!(kind, ImportKind::Luts(_)) && !extension("cube") {
            summary.fail(&file.path, "not a .cube LUT");
            continue;
        }
        if extension("cube") {
            // Its look's name is the import's to choose, so it is checked by its
            // table rather than its file name (`camera_profiles::import_files`).
            match crate::camera_profiles::read_cube(&bytes) {
                Ok(_) => cubes.push(file.path.clone()),
                Err(e) => summary.fail(&file.path, format!("{e:#}")),
            }
            continue;
        }
        if xmp {
            // Folders of profiles often hold presets too; only looks belong here.
            if !std::str::from_utf8(&bytes).is_ok_and(is_look) {
                if file.chosen {
                    summary.fail(&file.path, "a preset, not a profile");
                }
                continue;
            }
        } else if bytes.len() as u64 > PROFILE_LIMIT {
            summary.fail(&file.path, "too large");
            continue;
        } else if let Err(e) = crate::camera_profiles::from_bytes(&bytes) {
            summary.fail(&file.path, format!("{e:#}"));
            continue;
        }
        if check_name(file, &bytes, destination, &mut names, &mut summary) {
            if xmp { &mut looks } else { &mut dcps }.push(file.path.clone());
        }
    }
    // Every DCP was checked above, so batches only fail on a write error.
    for batch in dcps.chunks(1000) {
        progress.set(format!("Importing profiles… {} done", summary.imported));
        match crate::camera_profiles::import_into_dir(batch, destination, space) {
            Ok(done) => summary.imported += done.len(),
            Err(e) => batch.iter().for_each(|p| summary.fail(p, format!("{e:#}"))),
        }
    }
    let chosen: HashSet<&Path> = found
        .iter()
        .filter(|f| f.chosen)
        .map(|f| f.path.as_path())
        .collect();
    // Looks last, as they need their base profile in the library. One without
    // its base fails the batch, so then import them one by one.
    for batch in looks.chunks(1000) {
        match crate::camera_profiles::import_into_dir(batch, destination, space) {
            Ok(done) => summary.imported += done.len(),
            Err(_) => {
                for path in batch {
                    match crate::camera_profiles::import_into_dir(
                        std::slice::from_ref(path),
                        destination,
                        space,
                    ) {
                        Ok(_) => summary.imported += 1,
                        // Camera Raw's folder of looks covers every camera; only
                        // looks chosen by name are worth a line each.
                        Err(e)
                            if e.downcast_ref::<crate::camera_profiles::MissingBase>()
                                .is_some()
                                && !chosen.contains(path.as_path()) =>
                        {
                            summary.without_base += 1
                        }
                        Err(e) => {
                            let name = path.file_name().unwrap_or_default().to_string_lossy();
                            let reason = format!("{e:#}");
                            let reason =
                                reason.strip_prefix(&format!("{name}: ")).unwrap_or(&reason);
                            summary.fail(path, reason);
                        }
                    }
                }
            }
        }
    }
    // Cubes after the looks, so their names avoid the looks' too. Every one was
    // read above, so a batch fails only on a write error.
    for batch in cubes.chunks(1000) {
        progress.set(format!("Importing LUTs… {} done", summary.imported));
        match crate::camera_profiles::import_into_dir(batch, destination, space) {
            Ok(done) => done.into_iter().for_each(|done| summary.add_cube(done)),
            Err(e) => batch.iter().for_each(|p| summary.fail(p, format!("{e:#}"))),
        }
    }
    summary
}
impl Summary {
    fn add_cube(&mut self, done: crate::camera_profiles::ImportedProfile) {
        let Some(cube) = done.cube else {
            return;
        };
        if cube.already {
            self.already += 1;
        } else {
            self.imported += 1;
        }
        self.cubes.push((done.source, cube));
    }
}

fn import_lens_profiles(found: &[Found], progress: &ImportProgress) -> Summary {
    let mut summary = Summary::new(ImportKind::LensProfiles);
    let destination = crate::storage::data_dir().join("lens-profiles");
    let mut names = BTreeMap::new();
    let mut good = Vec::new();
    for (i, file) in found.iter().enumerate() {
        report(progress, i, found.len(), "lens profiles");
        let Ok(bytes) = std::fs::read(&file.path) else {
            summary.fail(&file.path, "unreadable");
            continue;
        };
        let parsed = std::str::from_utf8(&bytes)
            .map_err(anyhow::Error::from)
            .and_then(crate::optics::lcp::parse);
        if let Err(e) = parsed {
            summary.fail(&file.path, format!("not a lens profile: {e:#}"));
        } else if check_name(file, &bytes, &destination, &mut names, &mut summary) {
            good.push(file.path.clone());
        }
    }
    for batch in good.chunks(4000) {
        progress.set(format!(
            "Importing lens profiles… {} done",
            summary.imported
        ));
        match crate::lens::lcp::import_files(batch) {
            Ok(done) => summary.imported += done.len(),
            Err(e) => batch.iter().for_each(|p| summary.fail(p, format!("{e:#}"))),
        }
    }
    summary
}

/// Copies presets into `root`/Imported, keeping their folders. A preset
/// identical to one anywhere in `installed` counts as already imported; one
/// whose name is taken by a different preset gets a numbered name. Look
/// profiles among the files are imported into the profile library `profiles`,
/// as a profile import would, so the presets that use them work.
fn import_presets(
    found: &[Found],
    progress: &ImportProgress,
    root: &Path,
    installed: &[PathBuf],
    profiles: &Path,
) -> Summary {
    use std::io::Write;
    let mut summary = Summary::new(ImportKind::Presets);
    progress.set("Checking installed presets…".to_string());
    let mut known: HashSet<u64> = installed
        .iter()
        .flat_map(|dir| find_files(dir, &["xmp"], &[]))
        .filter_map(|p| std::fs::read(p).ok())
        .map(|bytes| content_hash(&bytes))
        .collect();
    let mut looks = Vec::new();
    for (i, file) in found.iter().enumerate() {
        report(progress, i, found.len(), "presets");
        // Read up to the profile import's limit, as the file may be a look.
        let text = match std::fs::metadata(&file.path) {
            Ok(m) if m.len() > PROFILE_LIMIT => {
                summary.fail(&file.path, "too large");
                continue;
            }
            _ => match std::fs::read_to_string(&file.path) {
                Ok(text) => text,
                Err(_) => {
                    summary.fail(&file.path, "unreadable");
                    continue;
                }
            },
        };
        // Looks are profiles, never presets, wherever they lie.
        if is_look(&text) {
            looks.push(file.clone());
            continue;
        }
        if text.len() >= 8_000_000 {
            summary.fail(&file.path, "too large");
            continue;
        }
        let hash = content_hash(text.as_bytes());
        if known.contains(&hash) {
            summary.already += 1;
            continue;
        }
        match crate::xmp::parse(&file.path, &text) {
            // Photo sidecars sit next to RAW files; they aren't presets.
            Ok(preset) if preset.photo_settings && !file.chosen => continue,
            Ok(_) => {}
            Err(e) => {
                summary.fail(&file.path, format!("not a preset: {e:#}"));
                continue;
            }
        }
        let dir = root.join("Imported").join(&file.folder);
        let written = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(&dir)?;
            let stem = file.path.file_stem().unwrap_or_default().to_string_lossy();
            let mut target = dir.join(file.path.file_name().unwrap_or_default());
            let mut n = 2;
            while target.exists() {
                target = dir.join(format!("{stem} {n}.xmp"));
                n += 1;
            }
            crate::storage::write_atomic(&target, crate::storage::Replace::NoClobber, |f| {
                Ok(f.write_all(text.as_bytes())?)
            })
        })();
        match written {
            Ok(()) => {
                known.insert(hash);
                summary.imported += 1;
            }
            Err(e) => summary.fail(&file.path, format!("{e:#}")),
        }
    }
    if !looks.is_empty() {
        let kind = ImportKind::CameraProfiles;
        summary.add_profiles(import_camera_profiles(
            &looks,
            progress,
            kind,
            LutSpace::Srgb,
            profiles,
        ));
    }
    summary
}
impl Summary {
    /// Counts in what a profile import of a preset import's looks did.
    fn add_profiles(&mut self, looks: Summary) {
        self.profiles += looks.imported;
        self.already += looks.already;
        self.without_base += looks.without_base;
        self.failed.extend(looks.failed);
    }
}

/// Imports what `picks` (files and folders) hold for `kind`.
fn run(kind: ImportKind, picks: &[PathBuf], progress: &ImportProgress) -> Summary {
    progress.set(format!("Looking for {}…", kind.noun(2)));
    let found = gather(kind, picks);
    match kind {
        // Imported as profiles, LUTs take the sRGB default.
        ImportKind::CameraProfiles => {
            import_camera_profiles(&found, progress, kind, LutSpace::Srgb, &profile_library())
        }
        ImportKind::Luts(space) => {
            import_camera_profiles(&found, progress, kind, space, &profile_library())
        }
        ImportKind::LensProfiles => import_lens_profiles(&found, progress),
        ImportKind::Presets => import_presets(
            &found,
            progress,
            &crate::storage::data_dir().join("xmp-presets"),
            &crate::presets::library_dirs(),
            &profile_library(),
        ),
    }
}

/// The `import-profiles` command: imports `picks` (files and folders) as the app's
/// Import does, waiting for it, with the `.cube` LUTs among them expecting colours in
/// `space`.
pub fn import_profiles(picks: &[PathBuf], space: LutSpace) -> Summary {
    let kind = ImportKind::CameraProfiles;
    import_camera_profiles(
        &gather(kind, picks),
        &ImportProgress::default(),
        kind,
        space,
        &profile_library(),
    )
}

impl Editor {
    /// Starts importing `picks` in the background; the status line follows it.
    pub(in crate::app) fn import(
        &mut self,
        kind: ImportKind,
        picks: Vec<PathBuf>,
        ctx: &egui::Context,
    ) {
        if self.importing.is_some() || picks.is_empty() {
            return;
        }
        // A profile download publishes into the same library: profiles, looks and
        // presets (which bring their looks) wait for it, said so rather than
        // dropped silently. Lens profiles don't touch it.
        if !matches!(kind, ImportKind::LensProfiles)
            && let Some(name) = self.profile_download.downloading()
        {
            self.messages.warn(format!(
                "Not imported: {name} is downloading into the profile library. Import again when it finishes."
            ));
            return;
        }
        let progress = Arc::new(ImportProgress::new(ctx.clone()));
        self.importing = Some(progress.clone());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        let first = picks[0].clone();
        crate::app::task::spawn(
            tx,
            ctx,
            move |tx| {
                let summary = run(kind, &picks, &progress);
                let _ = tx.send(Event::Imported(Box::new(summary)));
            },
            move |tx, error| {
                let summary = Summary {
                    failed: vec![(first, error)],
                    ..Summary::new(kind)
                };
                let _ = tx.send(Event::Imported(Box::new(summary)));
            },
        );
    }
    pub(in crate::app) fn imported(&mut self, summary: Summary, ctx: &egui::Context) {
        self.importing = None;
        // A preset import takes the looks among its files too.
        let profiles = match summary.kind {
            ImportKind::CameraProfiles | ImportKind::Luts(_) => summary.imported,
            ImportKind::Presets => summary.profiles,
            ImportKind::LensProfiles => 0,
        };
        if profiles > 0 {
            self.list_profiles_after_import();
        }
        if summary.imported > 0 {
            match summary.kind {
                ImportKind::CameraProfiles | ImportKind::Luts(_) => {}
                ImportKind::LensProfiles => {
                    // Lens profiles are matched when a photo opens: reopen it.
                    if let Some(path) = self.document.path.clone() {
                        let photo = self.document.catalog_photo;
                        self.open_raw(path, photo);
                    }
                    // And the reference photo, whose lens may now have a profile.
                    self.reload_reference();
                }
                ImportKind::Presets => self.reload_presets(ctx),
            }
        }
        let mut said = summary.message();
        if summary.has_details() {
            said.push_str(": ");
            said.push_str(&summary.details(1));
        }
        if summary.failures() > 0 && summary.done() == 0 {
            self.messages.fail(said);
        } else if summary.failures() > 0 || summary.done() == 0 {
            self.messages.warn(said);
        } else {
            self.messages.done(said);
        }
        self.onboarding.imported(summary);
    }
    /// Lists the open photo's profiles again once an import has added some.
    pub(in crate::app) fn list_profiles_after_import(&mut self) {
        self.profile_library = crate::camera_profiles::library_fingerprint();
        let before = self.document.edit.recipe().clone();
        if let Some(m) = &self.document.metadata {
            let listed = crate::camera_profiles::listed(m);
            self.document.profiles = crate::app::develop::camera_look::keep_fitted_default(
                &self.document.profiles,
                m,
                listed.profiles,
            );
            self.document.profile_errors = listed.errors;
            self.document.profile_groups = listed.groups;
            // Importing never changes an edit: the user picks a profile. A photo
            // without one follows the raw defaults, which may now resolve to an
            // imported profile.
            self.refresh_photo_defaults();
            self.refresh_preset_support();
        }
        let current = self.unchanged_current_photo(&before);
        self.refresh_library_defaults_preserving(current);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRESET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about="" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:Exposure2012="+0.50"/>
 </rdf:RDF>
</x:xmpmeta>"#;

    #[test]
    fn lightroom_s_synced_presets_without_a_group_are_user_presets() {
        let temp = tempfile::tempdir().unwrap();
        // Where Lightroom, not Classic, keeps them: named by UUID in an
        // account's folder of its library.
        let synced = temp
            .path()
            .join("Lightroom Library.lrlibrary/0123abcd/cr_settings");
        std::fs::create_dir_all(&synced).unwrap();
        std::fs::write(synced.join("1f2e.xmp"), PRESET).unwrap();
        let grouped = PRESET.replace(
            "crs:Exposure2012=\"+0.50\"/>",
            "crs:Exposure2012=\"+0.30\">\n   <crs:Group><rdf:Alt><rdf:li xml:lang=\"x-default\">Travel</rdf:li></rdf:Alt></crs:Group>\n  </rdf:Description>",
        );
        std::fs::write(synced.join("3d4c.xmp"), grouped).unwrap();
        // Lightroom's defaults beside them, not presets: passed over quietly.
        let defaults = temp
            .path()
            .join("Lightroom Library.lrlibrary/0123abcd/cr_defaults");
        std::fs::create_dir_all(&defaults).unwrap();
        std::fs::write(defaults.join("RawDefaults.xmp"), "<x:xmpmeta/>").unwrap();
        let library = temp.path().join("Lightroom Library.lrlibrary");
        // The account's folder, the whole library, or the files themselves.
        let picks = [
            vec![synced.clone()],
            vec![library],
            vec![synced.join("1f2e.xmp"), synced.join("3d4c.xmp")],
        ];
        for (i, pick) in picks.iter().enumerate() {
            let root = temp.path().join(format!("library-{i}"));
            let summary = import_presets(
                &gather(ImportKind::Presets, pick),
                &ImportProgress::default(),
                &root,
                std::slice::from_ref(&root),
                &temp.path().join("profiles"),
            );
            assert_eq!(summary.imported, 2, "{pick:?}: {summary:?}");
            assert!(summary.failed.is_empty(), "{pick:?}: {summary:?}");
            let groups: std::collections::BTreeSet<String> = find_files(&root, &["xmp"], &[])
                .iter()
                .map(|p| {
                    let text = std::fs::read_to_string(p).unwrap();
                    crate::xmp::parse(p, &text).unwrap().group
                })
                .collect();
            assert_eq!(
                groups,
                ["Travel".into(), "User Presets".into()].into(),
                "{pick:?}"
            );
        }
    }

    #[test]
    fn folder_import_scans_recursively_keeps_folders_and_skips_duplicates() {
        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("Pack");
        std::fs::create_dir_all(pack.join("Warm/.hidden")).unwrap();
        std::fs::create_dir_all(pack.join("Defaults")).unwrap();
        std::fs::write(pack.join("One.xmp"), PRESET).unwrap();
        std::fs::write(pack.join("Warm/One.xmp"), PRESET.replace("0.50", "0.70")).unwrap();
        // Identical to Warm/One.xmp under another name: only the first,
        // Copy.xmp, is imported.
        std::fs::write(pack.join("Warm/Copy.xmp"), PRESET.replace("0.50", "0.70")).unwrap();
        std::fs::write(pack.join("Warm/.hidden/Two.xmp"), PRESET).unwrap();
        std::fs::write(pack.join("Defaults/Three.xmp"), PRESET).unwrap();
        std::fs::write(pack.join("Broken.xmp"), "<not xmp").unwrap();
        // A look profile goes to the profile library, not among the presets.
        std::fs::write(pack.join("Warm/Look.xmp"), REPRO_LOOK).unwrap();
        let root = temp.path().join("library");
        let profiles = temp.path().join("profiles");
        let progress = ImportProgress::default();
        let import = |found: &[Found]| {
            import_presets(
                found,
                &progress,
                &root,
                std::slice::from_ref(&root),
                &profiles,
            )
        };

        let found = gather(ImportKind::Presets, std::slice::from_ref(&pack));
        let summary = import(&found);
        assert_eq!((summary.imported, summary.profiles), (2, 1), "{summary:?}");
        assert_eq!(summary.already, 1);
        assert_eq!(summary.failed.len(), 1, "only Broken.xmp is reported");
        assert!(root.join("Imported/Pack/One.xmp").is_file());
        assert!(root.join("Imported/Pack/Warm/Copy.xmp").is_file());
        assert!(profiles.join("Look.xmp").is_file());
        assert!(!root.join("Imported/Pack/Warm/Look.xmp").exists());
        assert_eq!(
            summary.message(),
            "Imported 2 presets · 1 profile · 1 already imported · 1 skipped"
        );

        // Importing again finds everything installed.
        let again = import(&found);
        assert_eq!((again.imported, again.profiles, again.already), (0, 0, 4));

        // A different preset under a taken name gets a numbered one.
        let other = temp.path().join("Other");
        std::fs::create_dir_all(&other).unwrap();
        let chosen = other.join("One.xmp");
        std::fs::write(&chosen, PRESET.replace("0.50", "1.00")).unwrap();
        std::fs::create_dir_all(root.join("Imported/Other")).unwrap();
        std::fs::write(root.join("Imported/Other/One.xmp"), "taken").unwrap();
        let found = gather(ImportKind::Presets, &[chosen]);
        let summary = import(&found);
        assert_eq!(summary.imported, 1);
        assert!(root.join("Imported/Other/One 2.xmp").is_file());
    }

    /// Renamed or approximated LUTs are listed even when nothing failed.
    #[test]
    fn renamed_and_approximated_cubes_are_detailed() {
        use crate::camera_profiles::{CubeImport, Fidelity};
        let mut summary = Summary::new(ImportKind::CameraProfiles);
        let cube = |name: &str, fidelity| CubeImport {
            name: name.into(),
            stem: "Portra".into(),
            already: false,
            fidelity,
            space: LutSpace::Srgb,
        };
        summary.imported = 1;
        summary
            .cubes
            .push(("Portra.cube".into(), cube("Portra", Fidelity::Exact)));
        assert!(!summary.has_details());
        summary.imported = 2;
        summary.cubes.push((
            "Cool/Portra.cube".into(),
            cube("Portra (Cool)", Fidelity::Exact),
        ));
        assert!(summary.has_details());
        assert_eq!(summary.message(), "Imported 2 profiles · 1 renamed");
        assert_eq!(
            summary.details(8),
            "Portra.cube: imported as look \"Portra (Cool)\", as another profile is named \"Portra\": exact"
        );
    }

    /// Import .cube LUTs takes only the `.cube` files of a folder that holds profiles
    /// too, as LUT packs ship Lightroom profiles beside their cubes; a profile chosen
    /// by name is reported as the wrong kind, and nothing is written for it.
    #[test]
    fn a_lut_import_takes_only_cubes() {
        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("Pack");
        std::fs::create_dir_all(pack.join("Lightroom")).unwrap();
        for file in [
            "A.cube",
            "Sub.CUBE",
            "Lightroom/A.xmp",
            "Camera.dcp",
            "notes.txt",
        ] {
            std::fs::write(pack.join(file), "").unwrap();
        }
        let kind = ImportKind::Luts(LutSpace::AdobeRgb);
        let names = |found: &[Found]| -> Vec<String> {
            found
                .iter()
                .map(|f| f.path.file_name().unwrap().to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(
            names(&gather(kind, std::slice::from_ref(&pack))),
            ["A.cube", "Sub.CUBE"]
        );
        let chosen = gather(
            kind,
            &[pack.join("Camera.dcp"), pack.join("Lightroom/A.xmp")],
        );
        let summary = import_camera_profiles(
            &chosen,
            &ImportProgress::default(),
            kind,
            LutSpace::AdobeRgb,
            &temp.path().join("library"),
        );
        assert_eq!(summary.imported, 0);
        assert_eq!(
            summary.lines(),
            ["Camera.dcp: not a .cube LUT", "A.xmp: not a .cube LUT"]
        );
        assert_eq!(
            summary.message(),
            "Imported 0 LUTs in Adobe RGB (1998) · 2 skipped"
        );
        assert_eq!(kind.filter(), ("LUTs (.cube)", &["cube"][..]));
    }

    /// The status line names a LUT import's space unless it is sRGB, and a profile
    /// import's line is as it was; the detail lines name the space too.
    #[test]
    fn a_lut_imports_status_names_its_space() {
        use crate::camera_profiles::{CubeImport, Fidelity};
        let cube = |name: &str, space, fidelity| CubeImport {
            name: name.into(),
            stem: "Portra".into(),
            already: false,
            fidelity,
            space,
        };
        let rough = || Fidelity::Approximated {
            max_levels: 1.,
            resampled: Some(65),
            shaper: false,
            clamped: false,
        };
        let summary = |kind, space| {
            let mut summary = Summary::new(kind);
            summary.imported = 2;
            summary.cubes = vec![
                ("Portra.cube".into(), cube("Portra", space, rough())),
                (
                    "Cool/Portra.cube".into(),
                    cube("Portra (Cool)", space, Fidelity::Exact),
                ),
            ];
            summary
        };
        let pro = summary(
            ImportKind::Luts(LutSpace::ProPhotoRgb),
            LutSpace::ProPhotoRgb,
        );
        assert_eq!(
            pro.message(),
            "Imported 2 LUTs in ProPhoto RGB · 1 approximated · 1 renamed"
        );
        assert!(pro.details(8).contains(
            "Portra.cube: imported as look \"Portra (Cool)\" in ProPhoto RGB, as another"
        ));
        let srgb = summary(ImportKind::Luts(LutSpace::Srgb), LutSpace::Srgb);
        assert_eq!(
            srgb.message(),
            "Imported 2 LUTs · 1 approximated · 1 renamed"
        );
        let profiles = summary(ImportKind::CameraProfiles, LutSpace::Srgb);
        assert_eq!(
            profiles.message(),
            "Imported 2 profiles · 1 LUT approximated · 1 renamed"
        );
        assert!(!profiles.details(8).contains(" in "));
    }

    /// A look chosen in a preset import is imported as a profile, so one that
    /// can't be is reported, and nothing is written for it.
    #[test]
    fn chosen_looks_that_cannot_be_imported_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let look = temp.path().join("Look.xmp");
        std::fs::write(
            &look,
            PRESET.replace("crs:Exposure", "crs:PresetType=\"Look\" crs:Exposure"),
        )
        .unwrap();
        let found = gather(ImportKind::Presets, &[look]);
        let (root, profiles) = (temp.path().join("lib"), temp.path().join("profiles"));
        let summary = import_presets(&found, &ImportProgress::default(), &root, &[], &profiles);
        assert_eq!((summary.imported, summary.profiles), (0, 0));
        assert_eq!(summary.failed.len(), 1, "{summary:?}");
        assert!(summary.details(5).starts_with("Look.xmp: "), "{summary:?}");
        assert_eq!(summary.message(), "Imported 0 presets · 1 skipped");
        assert!(!root.exists() && !profiles.join("Look.xmp").exists());
    }

    /// Issue #790's pack: a preset and the look profile it uses, in one folder.
    const REPRO_LOOK: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Look"
   crs:UUID="C0FFEE00000000000000000000000001"
   crs:SupportsAmount="True"
   crs:Version="18.2.1"
   crs:ProcessVersion="15.4"
   crs:RGBTable="DAB0C6C9546227C6B4163C04E7D47C60"
   crs:Table_DAB0C6C9546227C6B4163C04E7D47C60="71000OcImwM!emwO1/)wKOJQvnrwLvUf400}|nDn"
   crs:HasSettings="True">
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Repro Look</rdf:li></rdf:Alt></crs:Name>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Repro Pack</rdf:li></rdf:Alt></crs:Group>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;
    const REPRO_PRESET: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
 <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
  <rdf:Description rdf:about=""
    xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/"
   crs:PresetType="Normal"
   crs:UUID="C0FFEE00000000000000000000000002"
   crs:SupportsAmount="True"
   crs:Version="18.2.1"
   crs:ProcessVersion="15.4"
   crs:Exposure2012="+0.50"
   crs:HasSettings="True">
   <crs:Name><rdf:Alt><rdf:li xml:lang="x-default">Repro Preset</rdf:li></rdf:Alt></crs:Name>
   <crs:Group><rdf:Alt><rdf:li xml:lang="x-default">Repro Pack</rdf:li></rdf:Alt></crs:Group>
   <crs:Look>
    <rdf:Description crs:Name="Repro Look" crs:Amount="1" crs:UUID="C0FFEE00000000000000000000000001"/>
   </crs:Look>
  </rdf:Description>
 </rdf:RDF>
</x:xmpmeta>"#;

    /// Whether the preset imported into `root` applies to a photo with the
    /// profiles in `profiles`, as the Presets panel checks it.
    fn repro_preset_applies(root: &Path, profiles: &Path) -> anyhow::Result<()> {
        let path = root.join("Imported/Repro Pack/Repro Preset.xmp");
        let preset = crate::xmp::parse(&path, &std::fs::read_to_string(&path)?)?;
        let photo = crate::camera_data::Metadata {
            make: "Test".into(),
            model: "Camera".into(),
            cam_xyz: [[0.8, -0.2, -0.1], [-0.3, 1.1, 0.2], [-0.05, 0.15, 0.6]],
            ..Default::default()
        };
        let listed = crate::camera_profiles::listed_from(&[profiles.to_path_buf()], &photo);
        let base = crate::model::recipe::Recipe::with_profiles(&photo, &listed.profiles);
        preset
            .apply(&base, &photo, &listed.profiles, None)
            .map(|_| ())
    }

    /// A preset pack ships its look profiles beside its presets; importing the
    /// pack takes both, as Lightroom's Import Develop Profiles and Presets does,
    /// whether the folder or its files are chosen (#790).
    #[test]
    fn a_preset_import_takes_the_packs_look_profiles_too() {
        for choose_files in [false, true] {
            let temp = tempfile::tempdir().unwrap();
            let pack = temp.path().join("Repro Pack");
            std::fs::create_dir_all(&pack).unwrap();
            std::fs::write(pack.join("Repro Look.xmp"), REPRO_LOOK).unwrap();
            std::fs::write(pack.join("Repro Preset.xmp"), REPRO_PRESET).unwrap();
            let (root, profiles) = (temp.path().join("presets"), temp.path().join("profiles"));
            let picks = if choose_files {
                vec![pack.join("Repro Look.xmp"), pack.join("Repro Preset.xmp")]
            } else {
                vec![pack.clone()]
            };
            let found = gather(ImportKind::Presets, &picks);
            let import = || {
                import_presets(
                    &found,
                    &ImportProgress::default(),
                    &root,
                    std::slice::from_ref(&root),
                    &profiles,
                )
            };

            let summary = import();
            assert_eq!(
                summary.message(),
                "Imported 1 preset · 1 profile",
                "{choose_files}: {summary:?}"
            );
            assert!(summary.failed.is_empty(), "{summary:?}");
            assert_eq!(summary.profiles, 1);
            assert_eq!(summary.done(), 2);
            assert_eq!(
                std::fs::read_to_string(profiles.join("Repro Look.xmp")).unwrap(),
                REPRO_LOOK
            );
            assert!(!root.join("Imported/Repro Pack/Repro Look.xmp").exists());
            repro_preset_applies(&root, &profiles).unwrap();

            let again = import();
            assert_eq!(
                again.message(),
                "Imported 0 presets · 2 already imported",
                "{choose_files}: {again:?}"
            );
            assert!(again.failed.is_empty(), "{again:?}");
            assert_eq!((again.profiles, again.done()), (0, 2));
        }
    }

    /// A look is held to the profile import's size limit, not the presets'
    /// smaller one; a preset that large is still too large.
    #[test]
    fn a_large_look_imports_as_a_profile_would() {
        let temp = tempfile::tempdir().unwrap();
        let pack = temp.path().join("Pack");
        std::fs::create_dir_all(&pack).unwrap();
        let pad = |text: &str| {
            let close = "</x:xmpmeta>";
            let body = text.strip_suffix(close).unwrap();
            let filler = 9_000_000 - body.len() - close.len() - "<!--  -->".len();
            format!("{body}<!-- {} -->{close}", "x".repeat(filler))
        };
        std::fs::write(pack.join("Look.xmp"), pad(REPRO_LOOK)).unwrap();
        std::fs::write(pack.join("Preset.xmp"), pad(REPRO_PRESET)).unwrap();
        let (root, profiles) = (temp.path().join("presets"), temp.path().join("profiles"));
        let found = gather(ImportKind::Presets, std::slice::from_ref(&pack));
        let summary = import_presets(&found, &ImportProgress::default(), &root, &[], &profiles);
        assert_eq!((summary.imported, summary.profiles), (0, 1), "{summary:?}");
        assert_eq!(summary.lines(), ["Preset.xmp: too large"]);
        assert!(profiles.join("Look.xmp").is_file());
    }

    /// Without its look, the pack's preset is the one the issue reports.
    #[test]
    fn the_repro_preset_needs_its_look() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("presets");
        let pack = temp.path().join("Repro Pack");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(pack.join("Repro Preset.xmp"), REPRO_PRESET).unwrap();
        let found = gather(ImportKind::Presets, std::slice::from_ref(&pack));
        let profiles = temp.path().join("profiles");
        let summary = import_presets(&found, &ImportProgress::default(), &root, &[], &profiles);
        assert_eq!(summary.message(), "Imported 1 preset");
        let error = format!("{:#}", repro_preset_applies(&root, &profiles).unwrap_err());
        assert!(
            error.contains("Missing or unsupported enhanced profile ‘Repro Look’"),
            "{error}"
        );
    }
}
