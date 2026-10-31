//! Bringing over a Lightroom library: its catalog, camera and lens profiles
//! and presets, offered on a first launch where Lightroom's are found; and the
//! Library's start while it has no photos.
use super::Editor;
use super::bulk_import::{ImportKind, Summary, find_files};
use super::dialogs::{CatalogDialog, FileDialog, FolderAction};
use super::task::Task;
use super::widgets::pretty_path;
use super::worker::Event;
use crate::app::Module;
use crate::app::design::{self, Tone, radius, space, text};
use crate::app::theme;
use crate::catalog::CatalogLocation;
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

/// Camera Raw's shared folder, installed with Lightroom for all users. It
/// holds Adobe's camera profiles and the Adobe looks (Adobe Color…).
fn shared_camera_raw() -> Option<PathBuf> {
    let path = if cfg!(target_os = "macos") {
        PathBuf::from("/Library/Application Support/Adobe/CameraRaw")
    } else if cfg!(windows) {
        PathBuf::from("C:\\ProgramData\\Adobe\\CameraRaw")
    } else {
        return None;
    };
    path.is_dir().then_some(path)
}
/// Camera Raw's per-user folder: your presets and third-party profiles.
fn user_camera_raw() -> Option<PathBuf> {
    let path = if cfg!(target_os = "macos") {
        PathBuf::from(std::env::var_os("HOME")?).join("Library/Application Support/Adobe/CameraRaw")
    } else if cfg!(windows) {
        PathBuf::from(std::env::var_os("APPDATA")?).join("Adobe\\CameraRaw")
    } else {
        return None;
    };
    path.is_dir().then_some(path)
}
/// What the setup view shows and the scan it runs when it opens.
#[derive(Default)]
pub(super) struct Onboarding {
    pub(super) visible: bool,
    scanned: bool,
    scanned_for: Option<CatalogLocation>,
    /// How many times the catalog had saved photo info read from files when
    /// it last scanned; a later save may add cameras.
    info_saves: u64,
    /// The scan under way; it walks Camera Raw's folders, thousands of files,
    /// so it runs off the UI thread and arrives as [`Event::OnboardingScanned`].
    scan: Task,
    found: Found,
    /// The last import of each kind started here, shown in its row.
    results: Vec<Summary>,
    /// The kind of import this view started and is under way.
    importing: Option<ImportKind>,
    /// Why the last catalog open or import failed, until one succeeds.
    pub(super) catalog_error: Option<String>,
    /// The Lightroom catalogs the open catalog was imported from, read when
    /// the view scans.
    imported_from: Vec<PathBuf>,
}
/// What the setup view found on disk, refreshed when it opens.
#[derive(Default)]
pub(crate) struct Found {
    /// The catalog's cameras as Adobe names them ("Sony ILCE-7CR").
    cameras: BTreeSet<String>,
    /// Adobe base and Camera Matching profiles for those cameras, then looks.
    adobe_profiles: Vec<PathBuf>,
    /// Your profiles named for those cameras ("Sony ILCE-7M2 Portra 400 SO.dcp"),
    /// or all of them when no camera is known yet.
    user_profiles: Vec<PathBuf>,
    /// Adobe lens profiles for the catalog's camera makers and common
    /// third-party lens brands, plus your own lens profiles.
    lens_profiles: Vec<PathBuf>,
    /// Models the catalog records that Adobe has no profiles for, to match
    /// your own profiles by.
    models: Vec<String>,
    /// Makers of the catalog's cameras, e.g. "Sony".
    makers: BTreeSet<String>,
    user_presets: Vec<PathBuf>,
    /// Lightroom catalogs where Lightroom keeps them, the latest used first.
    catalogs: Vec<PathBuf>,
}
impl Onboarding {
    pub(super) fn new(visible: bool) -> Self {
        Self {
            visible,
            ..Default::default()
        }
    }
    fn scanning(&self) -> bool {
        self.scan.is_running()
    }
    /// Keeps an import's result for its row.
    pub(super) fn imported(&mut self, summary: Summary) {
        if self.importing == Some(summary.kind) {
            self.importing = None;
        }
        self.results.retain(|r| r.kind != summary.kind);
        self.results.push(summary);
    }
    fn result(&self, kind: ImportKind) -> Option<&Summary> {
        self.results.iter().find(|r| r.kind == kind)
    }
}
/// The folder Lightroom Classic keeps its catalog in unless told otherwise.
fn lightroom_folder() -> Option<PathBuf> {
    let home = if cfg!(windows) {
        std::env::var_os("USERPROFILE")?
    } else {
        std::env::var_os("HOME")?
    };
    Some(PathBuf::from(home).join("Pictures").join("Lightroom"))
}
/// The Lightroom catalogs in Lightroom's folder, the latest used first.
fn lightroom_catalogs() -> Vec<PathBuf> {
    let Some(folder) = lightroom_folder() else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "lrcat") && p.is_file())
        .map(|p| {
            let used = p.metadata().and_then(|m| m.modified());
            (used.unwrap_or(std::time::UNIX_EPOCH), p)
        })
        .collect();
    found.sort_by_key(|(used, _)| std::cmp::Reverse(*used));
    found.into_iter().map(|(_, p)| p).collect()
}
/// Whether this computer has something of Lightroom's to bring over: its
/// catalog or Camera Raw's profiles and presets. A first launch opens the
/// setup view only then.
pub(super) fn lightroom_here() -> bool {
    shared_camera_raw().is_some() || user_camera_raw().is_some() || !lightroom_catalogs().is_empty()
}
/// The full name `model` goes by among `names`, which spell out the maker:
/// Lightroom records "ILCE-7M2" where Adobe's profiles say "Sony ILCE-7M2", but
/// "LEICA M10" in both.
fn full_name<'a>(model: &str, names: &'a [String]) -> Option<&'a String> {
    let model = model.to_lowercase();
    if let Some(name) = names.iter().find(|name| name.to_lowercase() == model) {
        return Some(name);
    }
    // By model alone only when one maker has it: "M10" is a Leica and a Canon.
    let suffix = format!(" {model}");
    let mut makers = names
        .iter()
        .filter(|name| name.to_lowercase().ends_with(&suffix));
    let name = makers.next()?;
    makers.all(|other| other == name).then_some(name)
}
/// Whether any of `names` ends in `model`, as "Canon EOS M10" ends in "M10".
fn shares_model(model: &str, names: &[String]) -> bool {
    let suffix = format!(" {}", model.to_lowercase());
    names.iter().any(|n| n.to_lowercase().ends_with(&suffix))
}
/// The cameras Adobe has profiles for, as its folders and files name them.
fn adobe_cameras(profiles: &std::path::Path) -> Vec<String> {
    let names = |dir: PathBuf| {
        std::fs::read_dir(dir)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    let mut cameras = names(profiles.join("Camera"));
    cameras.extend(
        names(profiles.join("Adobe Standard"))
            .into_iter()
            .filter_map(|n| n.strip_suffix(" Adobe Standard.dcp").map(str::to_string)),
    );
    cameras
}
impl Found {
    /// What there is for `models`, the cameras the catalog records.
    fn scan(models: &[String], cancel: &AtomicBool) -> Self {
        let mut found = Self::default();
        let shared = shared_camera_raw();
        let known = shared
            .as_ref()
            .map(|s| adobe_cameras(&s.join("CameraProfiles")))
            .unwrap_or_default();
        for model in models {
            match full_name(model, &known) {
                Some(name) => {
                    found.cameras.insert(name.clone());
                }
                // Without Adobe's profiles to name the maker, the model is all
                // there is.
                None if shared.is_none() => {
                    found.cameras.insert(model.clone());
                }
                // A camera Adobe lacks may still have profiles of yours; one
                // several makers share ("M10") is left out rather than guessed.
                None if !shares_model(model, &known) => found.models.push(model.clone()),
                None => {}
            }
        }
        found.find_on_disk(cancel);
        found.catalogs = lightroom_catalogs();
        found
    }
    /// Camera Raw's profiles and presets, narrowed to the cameras found.
    /// Stops early once `cancel` is set, as the result is then dropped.
    fn find_on_disk(&mut self, cancel: &AtomicBool) {
        let cancelled = || cancel.load(Ordering::Relaxed);
        if let Some(shared) = shared_camera_raw() {
            let profiles = shared.join("CameraProfiles");
            for camera in &self.cameras {
                let base = profiles
                    .join("Adobe Standard")
                    .join(format!("{camera} Adobe Standard.dcp"));
                if base.is_file() {
                    self.adobe_profiles.push(base);
                }
                self.adobe_profiles.extend(find_files(
                    &profiles.join("Camera").join(camera),
                    &["dcp"],
                    &[],
                ));
            }
            if !self.adobe_profiles.is_empty() {
                // Looks last: they need their base profile in the same import.
                self.adobe_profiles.extend(find_files(
                    &shared.join("Settings/Adobe/Profiles/Adobe Raw"),
                    &["xmp"],
                    &[],
                ));
            }
        }
        if cancelled() {
            return;
        }
        self.makers = self
            .cameras
            .iter()
            .filter_map(|c| c.split_whitespace().next())
            .map(str::to_string)
            .collect();
        if let Some(shared) = shared_camera_raw()
            && !self.makers.is_empty()
            && let Ok(entries) = std::fs::read_dir(shared.join("LensProfiles/1.0"))
        {
            // Adobe groups lens profiles by maker; the camera maker's own
            // lenses plus the usual third-party brands cover most kits
            // without importing thousands of profiles.
            const THIRD_PARTY: [&str; 12] = [
                "Sigma",
                "Tamron",
                "Samyang",
                "Rokinon",
                "Zeiss",
                "Tokina",
                "Viltrox",
                "Voigtlander",
                "Laowa",
                "TTArtisan",
                "7Artisans",
                "Sirui",
            ];
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_lowercase();
                if self.makers.iter().any(|m| m.to_lowercase() == name)
                    || THIRD_PARTY.iter().any(|b| b.to_lowercase() == name)
                {
                    self.lens_profiles
                        .extend(find_files(&entry.path(), &["lcp"], &[]));
                }
            }
        }
        if cancelled() {
            return;
        }
        let user = user_camera_raw();
        if let Some(user) = &user {
            self.lens_profiles
                .extend(find_files(&user.join("LensProfiles"), &["lcp"], &[]));
        }
        // Third-party packs ship a DCP per camera model, often thousands in all;
        // only those for the catalog's cameras are useful. Without a catalog,
        // take them all.
        let cameras: Vec<String> = self.cameras.iter().chain(&self.models).cloned().collect();
        self.user_profiles = user
            .as_ref()
            .map(|d| find_files(&d.join("CameraProfiles"), &["dcp"], &[]))
            .unwrap_or_default()
            .into_iter()
            .filter(|p| {
                let name = p.file_name().unwrap_or_default().to_string_lossy();
                cameras.is_empty() || cameras.iter().any(|c| names_camera(&name, c))
            })
            .collect();
        self.user_presets = user
            .as_ref()
            .map(|d| find_files(&d.join("Settings"), &["xmp"], &["Defaults", "GPU"]))
            .unwrap_or_default();
    }
}

impl Editor {
    pub(super) fn onboarding_ui(&mut self, ui: &mut egui::Ui) {
        // Rescan when opened and whenever a different catalog is loaded.
        let catalog = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone());
        // And once the catalog has read cameras from new photos' files, when
        // the reader is done rather than at each of its saves.
        let read = self.library.as_ref().is_some_and(|l| {
            !l.reading_photo_info() && l.photo_info_saves() != self.onboarding.info_saves
        }) && !self.onboarding.scanning();
        if !self.onboarding.scanned || self.onboarding.scanned_for != catalog || read {
            self.start_onboarding_scan(catalog);
        }
        let ctx = ui.ctx().clone();
        let area = egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(theme::palette(ui.ctx()).tone(Tone::WINDOW)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink(false)
                    .show(ui, |ui| {
                        let width = (ui.available_width() - 48.).min(640.);
                        let margin = ((ui.available_width() - width) / 2.).max(24.);
                        ui.add_space(
                            (ui.available_height() * 0.1).clamp(space::XXXL, space::HUGE * 2.),
                        );
                        ui.horizontal(|ui| {
                            ui.add_space(margin);
                            ui.vertical(|ui| {
                                ui.set_width(width);
                                self.lightroom_view(ui, &ctx);
                            });
                        });
                        ui.add_space(space::HUGE);
                    });
            });
        self.drop_area = Some(area.response.rect);
    }

    /// Lightroom's catalog, profiles and presets, a row each.
    fn lightroom_view(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.spacing_mut().item_spacing = Vec2::new(space::M, 0.);
        text(
            ui,
            "Bring over your Lightroom library",
            text::DISPLAY,
            Tone::STRONG,
        );
        ui.add_space(space::M);
        paragraph(
            ui,
            "Import your catalog, camera and lens profiles and presets. Everything here \
             is optional, and nothing in Lightroom is changed.",
            text::LARGE,
            Tone::SECONDARY,
        );
        ui.add_space(space::XXXL);

        let enabled = !self.activity.is_busy() && self.importing.is_none();
        let scanning = self.onboarding.scanning();
        let mut catalog_action = None;
        let mut chosen = None;
        let rows = [
            self.catalog_row(),
            self.import_row(ImportKind::CameraProfiles, scanning),
            self.import_row(ImportKind::LensProfiles, scanning),
            self.import_row(ImportKind::Presets, scanning),
        ];
        let palette = theme::palette(ui.ctx());
        egui::Frame::new()
            .fill(palette.tone(Tone::CARD))
            .stroke(Stroke::new(1., palette.tone(Tone::FIELD)))
            .corner_radius(radius::DIALOG)
            .inner_margin(design::margin(space::XXL, space::XXS))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (i, row) in rows.iter().enumerate() {
                    if i > 0 {
                        divider(ui);
                    }
                    let action = row_ui(ui, row, enabled);
                    match (i, action) {
                        (_, None) => {}
                        (0, Some(action)) => catalog_action = Some(action),
                        (_, Some(action)) => chosen = Some((row.kind, action)),
                    }
                }
            });

        ui.add_space(space::XXL);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), 32.),
            egui::Layout::right_to_left(egui::Align::Center),
            |ui| {
                if ui
                    .add(
                        egui::Button::new(
                            egui::RichText::new("Continue")
                                .size(text::LARGE)
                                .color(palette.on_accent()),
                        )
                        .fill(palette.accent())
                        .min_size(Vec2::new(112., 32.)),
                    )
                    .clicked()
                {
                    self.finish_onboarding(true);
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    hint(ui, "Come back any time from the catalog menu.");
                });
            },
        );

        match catalog_action {
            Some(RowAction::Import) => {
                let source = self.onboarding.found.catalogs.first().cloned();
                self.import_lightroom_here(source, ctx);
            }
            Some(RowAction::Choose(_)) => self.import_lightroom_here(None, ctx),
            None => {}
        }
        let Some((Some(kind), action)) = chosen else {
            return;
        };
        match action {
            RowAction::Import => {
                let found = &self.onboarding.found;
                let paths = match kind {
                    // Yours first: an Adobe profile of the same name wins.
                    ImportKind::CameraProfiles | ImportKind::Luts(_) => {
                        let mut paths = found.user_profiles.clone();
                        paths.extend(found.adobe_profiles.iter().cloned());
                        paths
                    }
                    ImportKind::LensProfiles => found.lens_profiles.clone(),
                    // The folder rather than its files, to skip Defaults and
                    // keep preset folders.
                    ImportKind::Presets => user_camera_raw()
                        .map(|user| vec![user.join("Settings")])
                        .unwrap_or_default(),
                };
                self.onboarding.importing = Some(kind);
                self.import(kind, paths, ctx);
            }
            RowAction::Choose(Pick::Folder) => {
                self.onboarding.importing = Some(kind);
                self.dialog(FileDialog::ImportFolder(kind), ctx);
            }
            RowAction::Choose(Pick::Files) => {
                self.onboarding.importing = Some(kind);
                self.dialog(
                    match kind {
                        ImportKind::CameraProfiles => FileDialog::CameraProfile,
                        ImportKind::Luts(space) => FileDialog::Luts(space),
                        ImportKind::LensProfiles => FileDialog::LensProfile,
                        ImportKind::Presets => FileDialog::ImportXmp,
                    },
                    ctx,
                );
            }
        }
    }

    /// The Lightroom catalog's row: the one found where Lightroom keeps it,
    /// imported already or not.
    fn catalog_row(&self) -> Row {
        let found = self.onboarding.found.catalogs.first();
        // Imported when the open catalog says it came from that file; an
        // import leaves its catalog open, and the next launch opens it again.
        let same = |a: &std::path::Path, b: &std::path::Path| {
            a == b
                || a.canonicalize()
                    .is_ok_and(|a| b.canonicalize().is_ok_and(|b| a == b))
        };
        let imported = found.filter(|source| {
            self.onboarding
                .imported_from
                .iter()
                .any(|from| same(from, source))
        });
        let mut row = Row {
            kind: None,
            title: "Catalog",
            detail: "Folders, collections, ratings, flags, labels, keywords and edits.",
            mark: Mark::Todo,
            status: String::new(),
            hover: found.map(|p| pretty_path(p)),
            import: None,
            choose: ChooseWith::File,
        };
        if let Some(work) = &self.catalog_work {
            row.mark = Mark::Working;
            row.status = format!("{work} A large catalog takes a few minutes.");
        } else if imported.is_some()
            && let Some(library) = &self.library
        {
            row.mark = Mark::Done;
            row.status = format!(
                "Imported as {}, open now",
                library.session.catalog.location().name()
            );
        } else if let Some(source) = found {
            row.status = format!(
                "Found {}",
                source.file_name().unwrap_or_default().to_string_lossy()
            );
            row.import = Some("Import".into());
        } else {
            row.status = match lightroom_folder() {
                Some(folder) => format!("None in {}", pretty_path(&folder)),
                None => "None found".into(),
            };
        }
        if let Some(error) = &self.onboarding.catalog_error
            && row.mark != Mark::Working
        {
            row.status = format!("Import failed: {error}");
            row.mark = Mark::Failed;
        }
        row
    }

    /// The row of profiles or presets of `kind`: what the scan found, the
    /// import under way, or the last one's result.
    fn import_row(&self, kind: ImportKind, scanning: bool) -> Row {
        let found = &self.onboarding.found;
        let shared = shared_camera_raw();
        let user = user_camera_raw();
        let (title, detail, n, hover) = match kind {
            ImportKind::CameraProfiles | ImportKind::Luts(_) => (
                "Camera profiles",
                "Starting looks such as Adobe Color, for your cameras.",
                found.adobe_profiles.len() + found.user_profiles.len(),
                folders(&[&shared, &user], "CameraProfiles"),
            ),
            ImportKind::LensProfiles => (
                "Lens profiles",
                "Distortion and vignetting corrections for your lenses.",
                found.lens_profiles.len(),
                folders(&[&shared, &user], "LensProfiles"),
            ),
            ImportKind::Presets => (
                "Presets",
                "Your develop presets, in their groups.",
                found.user_presets.len(),
                folders(&[&user], "Settings"),
            ),
        };
        let noun = match kind {
            ImportKind::CameraProfiles | ImportKind::Luts(_) => "profile",
            ImportKind::LensProfiles => "lens profile",
            ImportKind::Presets => "preset",
        };
        let mut row = Row {
            kind: Some(kind),
            title,
            detail,
            mark: Mark::Todo,
            status: String::new(),
            hover,
            import: (n > 0).then(|| format!("Import {}", count(n, noun))),
            choose: ChooseWith::FolderOrFiles,
        };
        let cameras = || found.cameras.iter().cloned().collect::<Vec<_>>().join(", ");
        if self.importing.is_some() && self.onboarding.importing == Some(kind) {
            row.mark = Mark::Working;
            row.status = "Importing…".into();
        } else if let Some(result) = self.onboarding.result(kind) {
            row.mark = if result.imported + result.already > 0 {
                Mark::Done
            } else {
                Mark::Failed
            };
            row.status = result.message();
            if result.has_details() {
                row.hover = Some(result.details(8));
            }
        } else if scanning && n == 0 {
            row.mark = Mark::Working;
            row.status = "Looking…".into();
        } else if row.hover.is_none() {
            row.status = "Lightroom's folders aren't on this computer".into();
        } else if n == 0 && kind == ImportKind::LensProfiles && found.makers.is_empty() {
            // Adobe's thousands are narrowed to the catalog's camera makers.
            row.status = "Add photos first to find the ones for your cameras".into();
        } else if n == 0 {
            row.status = "None found".into();
        } else {
            row.status = match kind {
                ImportKind::CameraProfiles if !found.cameras.is_empty() => format!(
                    "{} Adobe and {} of yours, for {}",
                    found.adobe_profiles.len(),
                    found.user_profiles.len(),
                    cameras()
                ),
                ImportKind::CameraProfiles => format!(
                    "{} of yours found. Add photos to match Adobe's to your cameras.",
                    found.user_profiles.len()
                ),
                ImportKind::LensProfiles if !found.makers.is_empty() => {
                    let makers = found.makers.iter().cloned().collect::<Vec<_>>().join(", ");
                    format!("{n} found for {makers} and third-party lenses")
                }
                _ => format!("{} found", count(n, noun)),
            };
            if kind == ImportKind::Presets && !self.presets.library.presets.is_empty() {
                row.status.push_str(&format!(
                    " · {} in RAWmakase already",
                    self.presets.library.presets.len()
                ));
            }
        }
        row
    }

    /// The Library's view while its catalog has no photos, or no catalog is
    /// open: what to do first, in place of an empty grid.
    pub(super) fn library_start(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);
        let busy = self.activity.is_busy();
        let default = self.default_catalog();
        // Folders added that held no photos still show in the Folders panel.
        let folders = self.library.as_ref().map(|l| l.session.roots.len());
        let (title, detail) = match folders {
            Some(0) => (
                "Add your photos".to_string(),
                "Choose a folder and RAWmakase shows every photo in it and its subfolders. \
                 Photos stay where they are; nothing is copied, moved or changed.",
            ),
            Some(n) => (
                format!("No photos in the {} added", count(n, "folder")),
                "RAWmakase reads RAW, DNG, JPEG, HEIC, PNG, TIFF, WebP, AVIF and JPEG XL \
                 files. Add a folder that holds some, or find the folder again if it has moved.",
            ),
            None => (
                "No catalog is open".to_string(),
                "RAWmakase keeps your folders, ratings and edits in a catalog. Start one \
                 to add photos, or open a catalog you already have.",
            ),
        };
        // The setup view's backdrop, not the grid's black.
        ui.painter()
            .rect_filled(ui.max_rect(), 0., palette.tone(Tone::WINDOW));
        let width = 400_f32.min(ui.available_width() - 64.);
        let height = 380.;
        let top = ((ui.available_height() - height) / 2.).max(24.);
        ui.add_space(top);
        ui.vertical_centered(|ui| {
            ui.set_max_width(width);
            ui.spacing_mut().item_spacing = Vec2::new(space::M, 0.);
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(64.), Sense::hover());
            ui.painter()
                .circle_filled(rect.center(), 32., palette.tone(Tone::PANEL));
            super::icons::paint_at(
                ui.painter(),
                super::icons::Icon::FolderPlus,
                rect.center(),
                28.,
                palette.tone(Tone::SOFT),
            );
            ui.add_space(space::XXL);
            text(ui, &title, text::HERO, Tone::STRONG);
            ui.add_space(space::L);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(detail)
                        .size(text::BODY)
                        .line_height(Some(20.))
                        .color(palette.tone(Tone::SECONDARY)),
                )
                .halign(egui::Align::Center)
                .wrap(),
            );
            ui.add_space(space::XXXL);
            ui.add_enabled_ui(!busy, |ui| {
                if folders.is_some() {
                    if icon_primary(ui, super::icons::Icon::FolderPlus, "Add photo folder…")
                        .clicked()
                    {
                        self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), &ctx);
                    }
                    ui.add_space(space::XL);
                    hint(ui, "Or drag a folder onto this window.");
                } else {
                    if primary(ui, default_catalog_label(&default))
                        .on_hover_text(pretty_path(&default))
                        .clicked()
                    {
                        self.open_default_catalog(&ctx);
                    }
                    ui.add_space(space::M);
                    if secondary(ui, "Open RAWmakase catalog…").clicked() {
                        self.catalog_dialog(CatalogDialog::Open, &ctx);
                    }
                }
                ui.add_space(space::HUGE);
                text(
                    ui,
                    "Coming from Lightroom Classic?",
                    text::LABEL,
                    Tone::SECONDARY,
                );
                ui.add_space(space::L);
                if secondary(ui, "Bring over your Lightroom library…").clicked() {
                    self.open_onboarding();
                }
            });
            if busy {
                ui.add_space(space::XL);
                let status = self
                    .catalog_work
                    .clone()
                    .unwrap_or_else(|| self.messages.line().to_string());
                ui.horizontal(|ui| {
                    let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.), egui::Sense::hover());
                    super::widgets::paint_spinner(ui, rect, Some(palette.tone(Tone::LABEL)));
                    hint(ui, &status);
                });
            }
        });
    }

    /// Hides the view; `done` records that setup is complete so it stays hidden.
    fn finish_onboarding(&mut self, done: bool) {
        self.onboarding.visible = false;
        self.onboarding_done = done;
        self.module = if self.library.is_some() {
            Module::Library
        } else {
            Module::Develop
        };
        let _ = self.save_session();
    }
    /// Scans in the background; until the result arrives the steps say so.
    fn start_onboarding_scan(&mut self, catalog: Option<CatalogLocation>) {
        // The catalog already records each photo's camera; no RAW is opened.
        let models = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog.raw_cameras().ok())
            .unwrap_or_default();
        self.onboarding.info_saves = self.library.as_ref().map_or(0, |l| l.photo_info_saves());
        self.onboarding.imported_from = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog.lightroom_sources().ok())
            .unwrap_or_default();
        let (generation, cancel) = self.onboarding.scan.start();
        self.onboarding.scanned = true;
        self.onboarding.scanned_for = catalog;
        self.onboarding.found = Found::default();
        let tx = self.tx.clone();
        let ctx = self.context.clone();
        std::thread::spawn(move || {
            let found = Found::scan(&models, &cancel);
            if !cancel.load(Ordering::Relaxed) {
                let _ = tx.send(Event::OnboardingScanned {
                    generation,
                    found: Box::new(found),
                });
                ctx.request_repaint();
            }
        });
    }
    /// Takes a scan's result unless a later scan superseded it.
    pub(super) fn onboarding_scanned(&mut self, generation: u64, found: Found) {
        if self.onboarding.scan.is_running() && self.onboarding.scan.id() == generation {
            self.onboarding.scan.finish(generation);
            self.onboarding.found = found;
        }
    }
    pub(super) fn open_onboarding(&mut self) {
        self.onboarding.visible = true;
        self.onboarding.scanned = false;
        self.onboarding.results.clear();
    }
}

/// Where a row stands, shown in its first column.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mark {
    Todo,
    Working,
    Done,
    Failed,
}
/// What a row's "Choose…" asks for.
#[derive(Clone, Copy)]
enum ChooseWith {
    File,
    FolderOrFiles,
}
/// What the user chose in "Choose…".
#[derive(Clone, Copy)]
enum Pick {
    Folder,
    Files,
}
/// A row's button that was clicked.
#[derive(Clone, Copy)]
enum RowAction {
    /// Import what the row found.
    Import,
    Choose(Pick),
}
/// One thing to bring over, as its row shows it.
struct Row {
    /// None for the catalog.
    kind: Option<ImportKind>,
    title: &'static str,
    detail: &'static str,
    mark: Mark,
    status: String,
    /// Where it was looked for, or what failed.
    hover: Option<String>,
    /// The label of the button importing what was found.
    import: Option<String>,
    choose: ChooseWith,
}
/// Draws `row`: its mark on the title's line, its buttons on the right of
/// that line, and its text in the width they leave. Returns the button
/// clicked.
fn row_ui(ui: &mut egui::Ui, row: &Row, enabled: bool) -> Option<RowAction> {
    /// The mark's column, and the gap after it and before the buttons.
    const MARK: f32 = 22.;
    const GAP: f32 = 16.;
    /// The title's line, which the mark and the buttons are centred on.
    const LINE: f32 = 22.;
    const BUTTON: f32 = 28.;
    let palette = theme::palette(ui.ctx());
    let mut action = None;
    ui.add_space(space::XL);
    let top = ui.cursor().min;
    let width = ui.available_width();
    // Buttons first, right-aligned on the title's line: the text gets the rest.
    let line = egui::Rect::from_min_size(
        egui::pos2(top.x, top.y + (LINE - BUTTON) / 2.),
        Vec2::new(width, BUTTON),
    );
    let buttons = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(line)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = space::M;
                ui.add_enabled_ui(enabled && row.mark != Mark::Working, |ui| {
                    let choose = secondary(ui, "Choose…").on_hover_text(match row.choose {
                        ChooseWith::File => "Pick a Lightroom catalog (.lrcat) elsewhere",
                        ChooseWith::FolderOrFiles => {
                            "Import from a folder (and its subfolders) or chosen files"
                        }
                    });
                    match row.choose {
                        ChooseWith::File => {
                            if choose.clicked() {
                                action = Some(RowAction::Choose(Pick::Files));
                            }
                        }
                        ChooseWith::FolderOrFiles => {
                            egui::Popup::menu(&choose).show(|ui| {
                                if ui.button("Folder…").clicked() {
                                    action = Some(RowAction::Choose(Pick::Folder));
                                }
                                if ui.button("Files…").clicked() {
                                    action = Some(RowAction::Choose(Pick::Files));
                                }
                            });
                        }
                    }
                    if let Some(label) = &row.import {
                        // Imported already: again, without the emphasis.
                        let clicked = if row.mark == Mark::Done {
                            secondary(ui, label).clicked()
                        } else {
                            primary(ui, label).clicked()
                        };
                        if clicked {
                            action = Some(RowAction::Import);
                        }
                    }
                });
            },
        )
        .response
        .rect;
    mark(
        ui,
        egui::pos2(top.x + MARK / 2., top.y + LINE / 2.),
        row.mark,
    );
    let left = top.x + MARK + GAP;
    let text_width = (buttons.left() - GAP - left).max(160.);
    let text = ui
        .scope_builder(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_size(
                    egui::pos2(left, top.y),
                    Vec2::new(text_width, f32::INFINITY),
                ))
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.set_width(text_width);
                ui.allocate_ui_with_layout(
                    Vec2::new(text_width, LINE),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| text(ui, row.title, text::TITLE, Tone::STRONG),
                );
                ui.add_space(space::XXS);
                paragraph(ui, row.detail, text::BODY, Tone::SECONDARY);
                ui.add_space(space::S);
                let status = egui::RichText::new(&row.status)
                    .size(text::LABEL)
                    .line_height(Some(17.))
                    .color(match row.mark {
                        Mark::Done => Color32::from_rgb(120, 190, 140),
                        Mark::Failed => Color32::from_rgb(220, 140, 110),
                        _ => palette.tone(Tone::FAINT),
                    });
                let response = ui.add(egui::Label::new(status).wrap());
                if let Some(hover) = &row.hover {
                    response.on_hover_text(hover);
                }
            },
        )
        .response
        .rect;
    let bottom = text.bottom().max(line.bottom());
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(
        top,
        egui::pos2(top.x + width, bottom),
    ));
    ui.add_space(space::XL);
    action
}
/// A row's mark: an empty ring, a spinner, a check or a cross.
fn mark(ui: &mut egui::Ui, center: egui::Pos2, mark: Mark) {
    let palette = theme::palette(ui.ctx());
    let painter = ui.painter();
    match mark {
        Mark::Todo => {
            painter.circle_stroke(
                center,
                9.,
                Stroke::new(1.5, palette.tone(Tone::OUTLINE_STRONG)),
            );
        }
        Mark::Working => {
            let rect = egui::Rect::from_center_size(center, Vec2::splat(16.));
            super::widgets::paint_spinner(ui, rect, Some(palette.tone(Tone::SOFT)));
        }
        Mark::Done => {
            painter.circle_filled(center, 10., Color32::from_rgb(64, 132, 90));
            super::icons::paint_at(
                painter,
                super::icons::Icon::Check,
                center,
                13.,
                Color32::WHITE,
            );
        }
        Mark::Failed => {
            painter.circle_filled(center, 10., Color32::from_rgb(150, 72, 52));
            super::icons::paint_at(
                painter,
                super::icons::Icon::Close,
                center,
                12.,
                Color32::WHITE,
            );
        }
    }
}
/// The folders of `subfolder` under each of `dirs` that exist, one a line,
/// for a row's hover; None when there are none.
fn folders(dirs: &[&Option<PathBuf>], subfolder: &str) -> Option<String> {
    let lines: Vec<String> = dirs
        .iter()
        .filter_map(|dir| dir.as_ref())
        .map(|dir| pretty_path(&dir.join(subfolder)))
        .collect();
    (!lines.is_empty()).then(|| format!("Looks in\n{}", lines.join("\n")))
}
/// The button that opens the default catalog, made on first use.
fn default_catalog_label(path: &std::path::Path) -> &'static str {
    if path.exists() {
        "Open the Photos catalog"
    } else {
        "Start a new catalog"
    }
}
/// "1 photo", "312 photos".
fn count(n: usize, noun: &str) -> String {
    if n == 1 {
        format!("1 {noun}")
    } else {
        format!("{n} {noun}s")
    }
}
/// A hairline across the card, between rows.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), Sense::hover());
    ui.painter()
        .rect_filled(rect, 0., theme::palette(ui.ctx()).tone(Tone::FIELD));
}
/// Wrapped text with room between its lines.
fn paragraph(ui: &mut egui::Ui, value: &str, size: f32, tone: Tone) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(value)
                .size(size)
                .line_height(Some(size * 1.45))
                .color(theme::palette(ui.ctx()).tone(tone)),
        )
        .wrap(),
    );
}
fn text(ui: &mut egui::Ui, value: &str, size: f32, tone: Tone) {
    ui.label(
        egui::RichText::new(value)
            .size(size)
            .color(theme::palette(ui.ctx()).tone(tone)),
    );
}
fn hint(ui: &mut egui::Ui, value: &str) {
    text(ui, value, text::LABEL, Tone::MUTED);
}
/// Whether a profile file named `name` is for `camera`, by maker and model
/// ("Sony ILCE-7M2 Portra 400 SO.dcp") or by model alone.
fn names_camera(name: &str, camera: &str) -> bool {
    let (name, camera) = (name.to_lowercase(), camera.to_lowercase());
    name.starts_with(&format!("{camera} ")) || name.contains(&format!(" {camera} "))
}
fn primary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::Button::new(
            egui::RichText::new(label)
                .size(text::BODY)
                .color(palette.on_accent()),
        )
        .fill(palette.accent())
        .min_size(Vec2::new(0., 28.)),
    )
}
/// The primary button with an icon before its label.
fn icon_primary(ui: &mut egui::Ui, icon: super::icons::Icon, label: &str) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    ui.add(
        egui::Button::image_and_text(
            egui::Image::new(icon.uri())
                .fit_to_exact_size(Vec2::splat(15.))
                .tint(palette.on_accent()),
            egui::RichText::new(label)
                .size(text::BODY)
                .color(palette.on_accent()),
        )
        .fill(palette.accent())
        .min_size(Vec2::new(0., 30.)),
    )
}
fn secondary(ui: &mut egui::Ui, label: &str) -> egui::Response {
    ui.add(
        egui::Button::new(egui::RichText::new(label).size(text::BODY)).min_size(Vec2::new(0., 28.)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opening_the_assistant_scans_in_the_background() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let photos = dir.path().join("photos");
        std::fs::create_dir(&photos)?;
        std::fs::write(photos.join("image.ARW"), b"not a raw")?;
        let catalog = dir.path().join("test.rawmakase");
        let mut c = crate::catalog::Catalog::create(&catalog)?;
        c.add_folder(&photos)?;
        drop(c);
        let ctx = egui::Context::default();
        let library = crate::app::library::Library::load(&catalog, ctx.clone())?;
        let mut editor =
            Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        editor.library = Some(Box::new(library));
        editor.open_onboarding();
        let frame = |editor: &mut Editor| {
            let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                editor.events(ui.ctx());
                editor.onboarding_ui(ui);
            });
            output.textures_delta.clear();
        };
        // The first frame draws the view and leaves the RAWs to the scan.
        frame(&mut editor);
        assert!(editor.onboarding.scanning());
        let started = std::time::Instant::now();
        while editor.onboarding.scanning() {
            assert!(started.elapsed().as_secs() < 30, "the scan never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
            frame(&mut editor);
        }
        let catalog = CatalogLocation::File(catalog);
        assert_eq!(editor.onboarding.scanned_for.as_ref(), Some(&catalog));
        assert!(editor.onboarding.found.cameras.is_empty());

        // A scan superseded by a later one is ignored.
        editor.start_onboarding_scan(Some(catalog.clone()));
        let stale = editor.onboarding.scan.id();
        editor.start_onboarding_scan(Some(catalog));
        let found = Found {
            cameras: BTreeSet::from(["Sony ILCE-7M2".to_string()]),
            ..Default::default()
        };
        editor.onboarding_scanned(stale, found);
        assert!(editor.onboarding.scanning());
        assert!(editor.onboarding.found.cameras.is_empty());
        Ok(())
    }

    #[test]
    fn a_lightroom_catalog_is_imported_only_when_the_open_catalog_came_from_it()
    -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        // The default catalog, named like the Lightroom catalog found.
        let catalog = dir.path().join("Photos.rawmakase");
        crate::catalog::Catalog::create(&catalog)?;
        let source = dir.path().join("Photos.lrcat");
        std::fs::write(&source, b"lightroom")?;
        let ctx = egui::Context::default();
        let mut editor =
            Editor::with_context(&ctx, None, crate::app::session::Session::default(), None);
        editor.library = Some(Box::new(crate::app::library::Library::load(&catalog, ctx)?));
        editor.onboarding.found.catalogs = vec![source.clone()];
        let row = editor.catalog_row();
        assert!(row.mark == Mark::Todo);
        assert_eq!(row.import.as_deref(), Some("Import"));

        editor.onboarding.imported_from = vec![source];
        let row = editor.catalog_row();
        assert!(row.mark == Mark::Done);
        assert_eq!(row.import, None);
        Ok(())
    }

    #[test]
    fn catalog_models_take_adobe_names() {
        let adobe = [
            "Sony ILCE-7M2",
            "Sony ILCA-77M2",
            "LEICA M10",
            "Canon EOS M10",
        ]
        .map(str::to_string);
        let name = |model| full_name(model, &adobe).map(String::as_str);
        assert_eq!(name("ILCE-7M2"), Some("Sony ILCE-7M2"));
        assert_eq!(name("leica m10"), Some("LEICA M10"));
        assert_eq!(name("M10"), None);
        assert_eq!(name("7M2"), None);
        assert_eq!(name("iPhone 8"), None);
        // "M10" is a Leica and a Canon, so it matches none of your profiles either.
        assert!(shares_model("M10", &adobe));
        assert!(!shares_model("iPhone 8", &adobe));
        assert!(names_camera(
            "Sony ILCE-7M2 Portra 400 SO.dcp",
            "Sony ILCE-7M2"
        ));
        assert!(names_camera("Sony ILCE-7M2 Portra 400 SO.dcp", "ILCE-7M2"));
        assert!(!names_camera("Sony ILCE-7M2 Portra 400 SO.dcp", "7M2"));
        assert!(!names_camera("Sony ILCE-7M2R Portra.dcp", "ILCE-7M2"));
    }
}
