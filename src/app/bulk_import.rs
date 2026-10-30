//! Imports camera profiles, lens profiles and presets from chosen files or
//! whole folders, leniently: folders are scanned recursively, files already
//! imported are skipped quietly, and what can't be imported is reported
//! instead of failing the rest.
use super::{Editor, worker::Event};
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
    pub(super) fn noun(self, count: usize) -> &'static str {
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
    pub(super) fn filter(self) -> (&'static str, &'static [&'static str]) {
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
pub(super) fn find_files(dir: &Path, extensions: &[&str], skip: &[&str]) -> Vec<PathBuf> {
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
struct Found {
    path: PathBuf,
    /// Its folder under the chosen one, starting with the chosen folder's
    /// name. Presets keep it: a preset without a group is grouped by folder.
    folder: PathBuf,
    /// Chosen by name rather than found in a folder. A chosen file of the
    /// wrong kind is reported; a found one is passed over quietly.
    chosen: bool,
}

fn gather(kind: ImportKind, picks: &[PathBuf]) -> Vec<Found> {
    let skip: &[&str] = match kind {
        ImportKind::Presets => &["Defaults", "GPU"],
        _ => &[],
    };
    let mut found = Vec::new();
    for pick in picks {
        if pick.is_dir() {
            let root = pick.file_name().map(PathBuf::from).unwrap_or_default();
            for path in find_files(pick, kind.extensions(), skip) {
                let under = path
                    .parent()
                    .and_then(|p| p.strip_prefix(pick).ok())
                    .unwrap_or(Path::new(""));
                found.push(Found {
                    folder: root.join(under),
                    path,
                    chosen: false,
                });
            }
        } else {
            found.push(Found {
                folder: pick
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(PathBuf::from)
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
    pub(super) kind: ImportKind,
    pub(super) imported: usize,
    /// Identical to one already imported, or repeated within this import.
    pub(super) already: usize,
    /// Profile looks found in a folder for cameras without a base profile.
    pub(super) without_base: usize,
    /// Files that couldn't be imported, with the reason.
    pub(super) failed: Vec<(PathBuf, String)>,
    /// What became of each `.cube`: its look's name and how faithfully it holds
    /// the cube.
    pub(super) cubes: Vec<(PathBuf, crate::camera_profiles::CubeImport)>,
}
impl Summary {
    fn new(kind: ImportKind) -> Self {
        Self {
            kind,
            imported: 0,
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
        if imported + already + failed + self.without_base == 0 {
            return format!("No {} found there", self.kind.noun(2));
        }
        let mut parts = vec![format!("Imported {imported} {}", self.kind.noun(imported))];
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
    pub(super) fn details(&self, limit: usize) -> String {
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
    pub(super) fn has_details(&self) -> bool {
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
        self.imported + self.already
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
pub(super) struct ImportProgress {
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
    pub(super) fn text(&self) -> String {
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

/// Imports the profiles and LUTs `found` holds for `kind` (camera profiles or LUTs
/// only), the LUTs as expecting colours in `space`.
fn import_camera_profiles(
    found: &[Found],
    progress: &ImportProgress,
    kind: ImportKind,
    space: LutSpace,
) -> Summary {
    let mut summary = Summary::new(kind);
    let destination = crate::storage::data_dir().join("camera-profiles");
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
        } else if bytes.len() > 16_000_000 {
            summary.fail(&file.path, "too large");
            continue;
        } else if let Err(e) = crate::camera_profiles::from_bytes(&bytes) {
            summary.fail(&file.path, format!("{e:#}"));
            continue;
        }
        if check_name(file, &bytes, &destination, &mut names, &mut summary) {
            if xmp { &mut looks } else { &mut dcps }.push(file.path.clone());
        }
    }
    // Every DCP was checked above, so batches only fail on a write error.
    for batch in dcps.chunks(1000) {
        progress.set(format!("Importing profiles… {} done", summary.imported));
        match crate::camera_profiles::import_files(batch, space) {
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
        match crate::camera_profiles::import_files(batch, space) {
            Ok(done) => summary.imported += done.len(),
            Err(_) => {
                for path in batch {
                    match crate::camera_profiles::import_files(std::slice::from_ref(path), space) {
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
        match crate::camera_profiles::import_files(batch, space) {
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
/// whose name is taken by a different preset gets a numbered name.
fn import_presets(
    found: &[Found],
    progress: &ImportProgress,
    root: &Path,
    installed: &[PathBuf],
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
    for (i, file) in found.iter().enumerate() {
        report(progress, i, found.len(), "presets");
        let text = match std::fs::metadata(&file.path) {
            Ok(m) if m.len() >= 8_000_000 => {
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
        let hash = content_hash(text.as_bytes());
        if known.contains(&hash) {
            summary.already += 1;
            continue;
        }
        if is_look(&text) {
            if file.chosen {
                summary.fail(
                    &file.path,
                    "a camera profile; import it under Camera profiles",
                );
            }
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
    summary
}

/// Imports what `picks` (files and folders) hold for `kind`.
fn run(kind: ImportKind, picks: &[PathBuf], progress: &ImportProgress) -> Summary {
    progress.set(format!("Looking for {}…", kind.noun(2)));
    let found = gather(kind, picks);
    match kind {
        // Imported as profiles, LUTs take the sRGB default.
        ImportKind::CameraProfiles => {
            import_camera_profiles(&found, progress, kind, LutSpace::Srgb)
        }
        ImportKind::Luts(space) => import_camera_profiles(&found, progress, kind, space),
        ImportKind::LensProfiles => import_lens_profiles(&found, progress),
        ImportKind::Presets => import_presets(
            &found,
            progress,
            &crate::storage::data_dir().join("xmp-presets"),
            &crate::presets::library_dirs(),
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
    )
}

impl Editor {
    /// Starts importing `picks` in the background; the status line follows it.
    pub(super) fn import(&mut self, kind: ImportKind, picks: Vec<PathBuf>, ctx: &egui::Context) {
        if self.importing.is_some() || picks.is_empty() {
            return;
        }
        let progress = Arc::new(ImportProgress::new(ctx.clone()));
        self.importing = Some(progress.clone());
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        let first = picks[0].clone();
        super::task::spawn(
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
    pub(super) fn imported(&mut self, summary: Summary, ctx: &egui::Context) {
        self.importing = None;
        if summary.imported > 0 {
            match summary.kind {
                ImportKind::CameraProfiles | ImportKind::Luts(_) => {
                    self.profile_library = crate::camera_profiles::library_fingerprint();
                    let before = self.document.edit.recipe().clone();
                    if let Some(m) = &self.document.metadata {
                        let listed = crate::camera_profiles::listed(m);
                        self.document.profiles = super::camera_look::keep_fitted_default(
                            &self.document.profiles,
                            m,
                            listed.profiles,
                        );
                        self.document.profile_errors = listed.errors;
                        self.document.profile_groups = listed.groups;
                        // Importing never changes an edit: the user picks a
                        // profile. A photo without one follows the raw defaults,
                        // which may now resolve to an imported profile.
                        self.refresh_photo_defaults();
                        self.refresh_preset_support();
                    }
                    let current = self.unchanged_current_photo(&before);
                    self.refresh_library_defaults_preserving(current);
                }
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
        std::fs::write(
            pack.join("Look.xmp"),
            PRESET.replace("crs:Exposure", "crs:PresetType=\"Look\" crs:Exposure"),
        )
        .unwrap();
        let root = temp.path().join("library");
        let progress = ImportProgress::default();

        let found = gather(ImportKind::Presets, std::slice::from_ref(&pack));
        let summary = import_presets(&found, &progress, &root, std::slice::from_ref(&root));
        assert_eq!(summary.imported, 2, "{summary:?}");
        assert_eq!(summary.already, 1);
        assert_eq!(summary.failed.len(), 1, "only Broken.xmp is reported");
        assert!(root.join("Imported/Pack/One.xmp").is_file());
        assert!(root.join("Imported/Pack/Warm/Copy.xmp").is_file());
        assert_eq!(
            summary.message(),
            "Imported 2 presets · 1 already imported · 1 skipped"
        );

        // Importing again finds everything installed.
        let again = import_presets(&found, &progress, &root, std::slice::from_ref(&root));
        assert_eq!((again.imported, again.already), (0, 3));

        // A different preset under a taken name gets a numbered one.
        let other = temp.path().join("Other");
        std::fs::create_dir_all(&other).unwrap();
        let chosen = other.join("One.xmp");
        std::fs::write(&chosen, PRESET.replace("0.50", "1.00")).unwrap();
        std::fs::create_dir_all(root.join("Imported/Other")).unwrap();
        std::fs::write(root.join("Imported/Other/One.xmp"), "taken").unwrap();
        let found = gather(ImportKind::Presets, &[chosen]);
        let summary = import_presets(&found, &progress, &root, std::slice::from_ref(&root));
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

    #[test]
    fn chosen_files_of_the_wrong_kind_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let look = temp.path().join("Look.xmp");
        std::fs::write(
            &look,
            PRESET.replace("crs:Exposure", "crs:PresetType=\"Look\" crs:Exposure"),
        )
        .unwrap();
        let found = gather(ImportKind::Presets, &[look]);
        let summary = import_presets(
            &found,
            &ImportProgress::default(),
            &temp.path().join("lib"),
            &[],
        );
        assert_eq!(summary.failed.len(), 1);
        assert!(summary.details(5).starts_with("Look.xmp: a camera profile"));
    }
}
