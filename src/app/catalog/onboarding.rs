//! Bringing over a Lightroom library: its catalog, camera and lens profiles
//! and presets, offered on a first launch where Lightroom's are found; and the
//! Library's start while it has no photos. What Adobe's folders hold comes
//! from the Lightroom extension's listing (`app::extensions::adobe_files`);
//! this view picks what fits the catalog's cameras and imports it with the
//! app's importers.
use crate::app::Editor;
use crate::app::Module;
use crate::app::extensions::adobe_files::entries;
use crate::app::presets::bulk_import::{ImportKind, Summary, find_files};
use crate::app::shell::dialogs::{CatalogDialog, FileDialog, FolderAction};
use crate::app::task::Task;
use crate::app::ui::design::{self, Tone, radius, space, text};
use crate::app::ui::theme;
use crate::app::ui::widgets::pretty_path;
use crate::app::worker::Event;
use crate::catalog::CatalogLocation;
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use rawmakase_extensions::protocol::method::{FileKind, FileOrigin, FilesListed, Searched};
use std::collections::BTreeSet;
use std::path::PathBuf;

/// What the setup view shows and what it found when it opened.
#[derive(Default)]
pub(in crate::app) struct Onboarding {
    pub(in crate::app) visible: bool,
    scanned: bool,
    scanned_for: Option<CatalogLocation>,
    /// The listing of Adobe's folders it last scanned
    /// ([`AdobeFiles::generation`](crate::app::extensions::adobe_files::AdobeFiles::generation)).
    scanned_listing: u64,
    /// How many times the catalog had saved photo info read from files when
    /// it last scanned; a later save may add cameras.
    info_saves: u64,
    /// The scan under way: counting the presets reads their folders, so it
    /// runs off the UI thread and arrives as [`Event::OnboardingScanned`].
    scan: Task,
    found: Found,
    /// The last import of each kind started here, shown in its row.
    results: Vec<Summary>,
    /// The kind of import this view started and is under way.
    importing: Option<ImportKind>,
    /// Why the last catalog open or import failed, until one succeeds.
    pub(in crate::app) catalog_error: Option<String>,
    /// The Lightroom catalogs the open catalog was imported from, read when
    /// the view scans.
    imported_from: Vec<PathBuf>,
}
/// What the setup view found in Adobe's folders, refreshed when it opens.
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
    /// Makers of the catalog's cameras, e.g. "Sony".
    makers: BTreeSet<String>,
    /// The folders presets are imported from: Camera Raw's, where Lightroom
    /// Classic keeps them, then each Adobe account's in Lightroom's library.
    preset_folders: Vec<PathBuf>,
    /// How many presets they hold.
    user_presets: usize,
    /// Lightroom catalogs where Lightroom keeps them, the latest used first.
    catalogs: Vec<PathBuf>,
    /// Where the Lightroom extension looked, for each row to say.
    searched: Vec<Searched>,
}
impl Onboarding {
    fn scanning(&self) -> bool {
        self.scan.is_running()
    }
    /// Keeps an import's result for its row.
    pub(in crate::app) fn imported(&mut self, summary: Summary) {
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
impl Found {
    /// What `listed` holds for `models`, the cameras the catalog records.
    /// It counts the presets in the preset folders, which reads them: off
    /// the UI thread.
    fn scan(models: &[String], listed: &FilesListed) -> Self {
        let mut found = Self {
            searched: listed.searched.clone(),
            ..Self::default()
        };
        let of = |kind, origin| {
            entries(listed).filter(move |e| e.folder.kind == kind && e.folder.origin == origin)
        };
        let adobe = || of(FileKind::CameraProfile, FileOrigin::Shared);
        let known: Vec<String> = adobe()
            .filter_map(|e| e.camera.map(str::to_string))
            .collect();
        // Models Adobe has no profiles for, to match your own profiles by.
        let mut others = Vec::new();
        for model in models {
            match full_name(model, &known) {
                Some(name) => {
                    found.cameras.insert(name.clone());
                }
                // Without Adobe's profiles to name the maker, the model is all
                // there is.
                None if known.is_empty() => {
                    found.cameras.insert(model.clone());
                }
                // A camera Adobe lacks may still have profiles of yours; one
                // several makers share ("M10") is left out rather than guessed.
                None if !shares_model(model, &known) => others.push(model),
                None => {}
            }
        }
        // Adobe Standard, then Camera Matching, for each camera.
        for camera in &found.cameras {
            let fits = adobe().filter(|e| e.camera == Some(camera.as_str()));
            found.adobe_profiles.extend(fits.map(|e| e.path));
        }
        if !found.adobe_profiles.is_empty() {
            // Looks last: they need their base profile in the same import.
            let looks = of(FileKind::Look, FileOrigin::Shared);
            found.adobe_profiles.extend(looks.map(|e| e.path));
        }
        found.makers = found
            .cameras
            .iter()
            .filter_map(|c| c.split_whitespace().next())
            .map(str::to_string)
            .collect();
        // Adobe groups lens profiles by maker; the camera maker's own lenses
        // plus the usual third-party brands cover most kits without importing
        // thousands of profiles.
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
        let wanted = |group: &str| {
            let group = group.to_lowercase();
            found.makers.iter().any(|m| m.to_lowercase() == group)
                || THIRD_PARTY.iter().any(|b| b.to_lowercase() == group)
        };
        found.lens_profiles = of(FileKind::LensProfile, FileOrigin::Shared)
            .filter(|e| !found.makers.is_empty() && e.folder.group.as_deref().is_some_and(wanted))
            .chain(of(FileKind::LensProfile, FileOrigin::User))
            .map(|e| e.path)
            .collect();
        // Third-party packs ship a DCP per camera model, often thousands in all;
        // only those for the catalog's cameras are useful. Without a catalog,
        // take them all.
        let cameras: Vec<&String> = found.cameras.iter().chain(others).collect();
        found.user_profiles = of(FileKind::CameraProfile, FileOrigin::User)
            .filter(|e| {
                let name = e.path.file_name().unwrap_or_default().to_string_lossy();
                cameras.is_empty() || cameras.iter().any(|c| names_camera(&name, c))
            })
            .map(|e| e.path)
            .collect();
        found.preset_folders = of(FileKind::Presets, FileOrigin::User)
            .map(|e| e.path)
            .collect();
        found.user_presets = found
            .preset_folders
            .iter()
            .map(|d| find_files(d, &["xmp"], &["Defaults", "GPU"]).len())
            .sum();
        let mut catalogs: Vec<_> = of(FileKind::Catalog, FileOrigin::User).collect();
        catalogs.sort_by_key(|e| std::cmp::Reverse(e.used.unwrap_or(0)));
        found.catalogs = catalogs.into_iter().map(|e| e.path).collect();
        found
    }
}

impl Editor {
    pub(in crate::app) fn onboarding_ui(&mut self, ui: &mut egui::Ui) {
        // Rescan when opened and whenever a different catalog is loaded.
        let catalog = self
            .library
            .as_ref()
            .map(|l| l.session.catalog().location().clone());
        // And once the catalog has read cameras from new photos' files, when
        // the reader is done rather than at each of its saves; and for each
        // new listing of Adobe's folders.
        let read = self.library.as_ref().is_some_and(|l| {
            !l.reading_photo_info() && l.photo_info_saves() != self.onboarding.info_saves
        }) && !self.onboarding.scanning();
        self.adobe_files();
        if !self.adobe_files.asking()
            && (!self.onboarding.scanned
                || self.onboarding.scanned_for != catalog
                || self.onboarding.scanned_listing != self.adobe_files.generation()
                || read)
        {
            self.onboarding_scan(catalog);
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
        let mut catalog_action = None;
        let mut chosen = None;
        let rows = self.lightroom_rows();
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
                    match (row.kind, row_ui(ui, row, enabled)) {
                        (_, None) => {}
                        (None, Some(action)) => catalog_action = Some(action),
                        (kind, Some(action)) => chosen = Some((kind, action)),
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

        // Into a new catalog beside the default one, without asking where.
        if let (Some(action), Some(offer)) = (catalog_action, self.lightroom_importer()) {
            let source = match action {
                RowAction::Import => self.onboarding.found.catalogs.first().cloned(),
                RowAction::Choose(_) => None,
            };
            self.import_with(offer, source, Some(self.default_catalog()), ctx);
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
                    ImportKind::Presets => found.preset_folders.clone(),
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

    /// The Lightroom extension's catalog importer, while it is enabled.
    fn lightroom_importer(&self) -> Option<crate::app::extensions::importer::Offer> {
        self.importer_items().into_iter().find(|o| {
            o.extension == rawmakase_extensions::protocol::manifest::LIGHTROOM
                && o.importer == "catalog"
        })
    }

    /// The view's rows: the catalog's, while the Lightroom extension imports
    /// catalogs, then the profiles' and presets'.
    fn lightroom_rows(&self) -> Vec<Row> {
        let catalog = self.lightroom_importer().map(|_| self.catalog_row());
        catalog
            .into_iter()
            .chain([
                self.import_row(ImportKind::CameraProfiles),
                self.import_row(ImportKind::LensProfiles),
                self.import_row(ImportKind::Presets),
            ])
            .collect()
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
        if let Some(work) = self.catalog_importing() {
            row.mark = Mark::Working;
            row.status = format!("{work} A large catalog takes a few minutes.");
        } else if imported.is_some()
            && let Some(library) = &self.library
        {
            row.mark = Mark::Done;
            row.status = format!(
                "Imported as {}, open now",
                library.session.catalog().location().name()
            );
        } else if let Some(source) = found {
            row.status = format!(
                "Found {}",
                source.file_name().unwrap_or_default().to_string_lossy()
            );
            row.import = Some("Import".into());
        } else {
            row.status = match (
                self.adobe_files.failure(),
                self.onboarding.found.looked_in(FileKind::Catalog).first(),
            ) {
                (Some(why), _) => format!("Couldn't look in Lightroom's folders: {why}"),
                (None, Some(folder)) => format!("None in {folder}"),
                (None, None) => "None found".into(),
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
    fn import_row(&self, kind: ImportKind) -> Row {
        let found = &self.onboarding.found;
        let (title, detail, n, hover) = match kind {
            ImportKind::CameraProfiles | ImportKind::Luts(_) => (
                "Camera profiles",
                "Starting looks such as Adobe Color, for your cameras.",
                found.adobe_profiles.len() + found.user_profiles.len(),
                looks_in(found.looked_in(FileKind::CameraProfile)),
            ),
            ImportKind::LensProfiles => (
                "Lens profiles",
                "Distortion and vignetting corrections for your lenses.",
                found.lens_profiles.len(),
                looks_in(found.looked_in(FileKind::LensProfile)),
            ),
            ImportKind::Presets => (
                "Presets",
                "Your develop presets, in their groups.",
                found.user_presets,
                looks_in(
                    found
                        .preset_folders
                        .iter()
                        .map(|d| pretty_path(d))
                        .collect(),
                ),
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
            row.mark = if result.done() > 0 {
                Mark::Done
            } else {
                Mark::Failed
            };
            row.status = result.message();
            if result.has_details() {
                row.hover = Some(result.details(8));
            }
        } else if (self.adobe_files.asking() || self.onboarding.scanning()) && n == 0 {
            row.mark = Mark::Working;
            row.status = "Looking…".into();
        } else if !self.extensions.lightroom_enabled() {
            row.status = "The Lightroom extension, which finds these, is off".into();
        } else if let Some(why) = self.adobe_files.failure() {
            row.status = format!("Couldn't look in Lightroom's folders: {why}");
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
    pub(in crate::app) fn library_start(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let palette = theme::palette(&ctx);
        let busy = self.activity.is_busy();
        let default = self.default_catalog();
        // Folders added that held no photos still show in the Folders panel.
        let folders = self.library.as_ref().map(|l| l.session.roots().len());
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
            crate::app::ui::icons::paint_at(
                ui.painter(),
                crate::app::ui::icons::Icon::FolderPlus,
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
                    if icon_primary(
                        ui,
                        crate::app::ui::icons::Icon::FolderPlus,
                        "Add photo folder…",
                    )
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
                    crate::app::ui::widgets::paint_spinner(
                        ui,
                        rect,
                        Some(palette.tone(Tone::LABEL)),
                    );
                    hint(ui, &status);
                });
            }
        });
    }

    /// Hides the view; `done` records that setup is complete so it stays hidden.
    fn finish_onboarding(&mut self, done: bool) {
        self.onboarding.visible = false;
        self.adobe_files.first_launch = false;
        self.session.onboarding_done = done;
        self.module = if self.library.is_some() {
            Module::Library
        } else {
            Module::Develop
        };
        let _ = self.save_session();
    }
    /// Works out what the listing holds for the open catalog.
    fn onboarding_scan(&mut self, catalog: Option<CatalogLocation>) {
        // The catalog already records each photo's camera; no RAW is opened.
        let models = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog().raw_cameras().ok())
            .unwrap_or_default();
        self.onboarding.info_saves = self.library.as_ref().map_or(0, |l| l.photo_info_saves());
        self.onboarding.imported_from = self
            .library
            .as_ref()
            .and_then(|l| l.session.catalog().lightroom_sources().ok())
            .unwrap_or_default();
        let (generation, cancel) = self.onboarding.scan.start();
        self.onboarding.scanned = true;
        self.onboarding.scanned_for = catalog;
        self.onboarding.scanned_listing = self.adobe_files.generation();
        self.onboarding.found = Found::default();
        let listed = self.adobe_files.listed().cloned().unwrap_or_default();
        let (tx, ctx) = (self.tx.clone(), self.context.clone());
        std::thread::spawn(move || {
            let found = Found::scan(&models, &listed);
            if !cancel.load(std::sync::atomic::Ordering::Relaxed) {
                let found = Box::new(found);
                let _ = tx.send(Event::OnboardingScanned { generation, found });
                ctx.request_repaint();
            }
        });
    }
    /// Takes a scan's result unless a later scan superseded it.
    pub(in crate::app) fn onboarding_scanned(&mut self, generation: u64, found: Found) {
        if self.onboarding.scan.is_running() && self.onboarding.scan.id() == generation {
            self.onboarding.scan.finish(generation);
            self.onboarding.found = found;
        }
    }
    /// Opens the view, which lists Adobe's folders again. A first launch's
    /// listing no longer decides whether it opens.
    pub(in crate::app) fn open_onboarding(&mut self) {
        self.onboarding.visible = true;
        self.onboarding.scanned = false;
        self.onboarding.results.clear();
        self.adobe_files.first_launch = false;
        self.adobe_files.relist();
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
            crate::app::ui::widgets::paint_spinner(ui, rect, Some(palette.tone(Tone::SOFT)));
        }
        Mark::Done => {
            painter.circle_filled(center, 10., Color32::from_rgb(64, 132, 90));
            crate::app::ui::icons::paint_at(
                painter,
                crate::app::ui::icons::Icon::Check,
                center,
                13.,
                Color32::WHITE,
            );
        }
        Mark::Failed => {
            painter.circle_filled(center, 10., Color32::from_rgb(150, 72, 52));
            crate::app::ui::icons::paint_at(
                painter,
                crate::app::ui::icons::Icon::Close,
                center,
                12.,
                Color32::WHITE,
            );
        }
    }
}
impl Found {
    /// The folders the Lightroom extension looked in for `kind`, as shown.
    fn looked_in(&self, kind: FileKind) -> Vec<String> {
        let paths = self.searched.iter().filter(|s| s.kind == kind);
        paths.map(|s| pretty_path(&s.path)).collect()
    }
}
/// A row's hover listing `lines`, the folders it looks in; None without any.
fn looks_in(lines: Vec<String>) -> Option<String> {
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
fn icon_primary(
    ui: &mut egui::Ui,
    icon: crate::app::ui::icons::Icon,
    label: &str,
) -> egui::Response {
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
    use rawmakase_extensions::protocol::manifest::LIGHTROOM;

    /// For the fake install, what setup found before the listing came from
    /// the extension, by the catalog's cameras, relative to the roots: frozen
    /// from that code run on the same install.
    #[test]
    fn setup_finds_what_it_found_on_disk_before() {
        use rawmakase_lightroom::fake::{Layout, install};
        let v = |paths: &[&str]| paths.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let set = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<BTreeSet<_>>();
        let sony = v(&[
            "shared/CameraProfiles/Adobe Standard/Sony ILCE-7M2 Adobe Standard.dcp",
            "shared/CameraProfiles/Camera/Sony ILCE-7M2/Sony ILCE-7M2 Camera Standard.dcp",
            "shared/CameraProfiles/Camera/Sony ILCE-7M2/Sony ILCE-7M2 Camera Vivid.dcp",
            "shared/Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp",
            "shared/Settings/Adobe/Profiles/Adobe Raw/Adobe Portrait.xmp",
        ]);
        let sony_lenses = v(&[
            "shared/LensProfiles/1.0/Sigma/SIGMA 35mm F1.4 DG DN A.lcp",
            "shared/LensProfiles/1.0/Sony/Sony FE 24-70mm F2.8 GM.lcp",
            "user/LensProfiles/1.0/My Lens.lcp",
        ]);
        let portra = "user/CameraProfiles/Sony ILCE-7M2 Portra 400 SO.dcp";
        let velvia = "user/CameraProfiles/Fujifilm X-T5 Velvia.dcp";
        // Cameras, makers, Adobe's profiles, yours and lens profiles.
        type Expected = (
            BTreeSet<String>,
            BTreeSet<String>,
            Vec<String>,
            Vec<String>,
            Vec<String>,
        );
        let cases: [(&[&str], Expected); 4] = [
            (
                &[],
                (
                    set(&[]),
                    set(&[]),
                    v(&[]),
                    v(&[velvia, portra]),
                    v(&["user/LensProfiles/1.0/My Lens.lcp"]),
                ),
            ),
            (
                &["ILCE-7M2"],
                (
                    set(&["Sony ILCE-7M2"]),
                    set(&["Sony"]),
                    sony.clone(),
                    v(&[portra]),
                    sony_lenses.clone(),
                ),
            ),
            (
                &["ILCE-7M2", "M10", "X-T5", "EOS R5"],
                (
                    set(&["Sony ILCE-7M2"]),
                    set(&["Sony"]),
                    sony,
                    v(&[velvia, portra]),
                    sony_lenses,
                ),
            ),
            (
                &["Pixel 7 Pro", "LEICA M10"],
                (
                    set(&["Google Pixel 7 Pro", "LEICA M10"]),
                    set(&["Google", "LEICA"]),
                    v(&[
                        "shared/CameraProfiles/Adobe Standard/Google Pixel 7 Pro Adobe Standard.dcp",
                        "shared/CameraProfiles/Adobe Standard/LEICA M10 Adobe Standard.dcp",
                        "shared/Settings/Adobe/Profiles/Adobe Raw/Adobe Color.xmp",
                        "shared/Settings/Adobe/Profiles/Adobe Raw/Adobe Portrait.xmp",
                    ]),
                    v(&[]),
                    v(&[
                        "shared/LensProfiles/1.0/Sigma/SIGMA 35mm F1.4 DG DN A.lcp",
                        "user/LensProfiles/1.0/My Lens.lcp",
                    ]),
                ),
            ),
        ];
        // Every case finds the same presets and catalogs.
        let preset_folders = v(&[
            "user/Settings",
            "pictures/Lightroom Library.lrlibrary/0a1c/cr_settings",
            "pictures/Lightroom Library.lrlibrary/b67b/cr_settings",
        ]);
        let user_presets = v(&[
            "user/Settings/Mine.xmp",
            "user/Settings/My Presets/Warm.xmp",
            "pictures/Lightroom Library.lrlibrary/0a1c/cr_settings/Other.xmp",
            "pictures/Lightroom Library.lrlibrary/b67b/cr_settings/Synced.xmp",
        ]);
        let catalogs = v(&[
            "pictures/Lightroom/New.lrcat",
            "pictures/Lightroom/Old.lrcat",
        ]);
        for layout in [Layout::MacOs, Layout::Windows] {
            let dir = tempfile::tempdir().unwrap();
            let roots = install(dir.path(), layout).unwrap();
            let listed = rawmakase_lightroom::list(&roots);
            let rel = |paths: &[PathBuf]| -> Vec<String> {
                let under = [
                    ("shared", &roots.shared),
                    ("user", &roots.user),
                    ("pictures", &roots.pictures),
                ];
                paths
                    .iter()
                    .map(|p| {
                        under
                            .iter()
                            .find_map(|(name, root)| {
                                let relative = p.strip_prefix(root.as_ref()?).ok()?;
                                Some(format!(
                                    "{name}/{}",
                                    relative.to_string_lossy().replace('\\', "/")
                                ))
                            })
                            .unwrap()
                    })
                    .collect()
            };
            for (models, expected) in &cases {
                let models: Vec<String> = models.iter().map(|m| m.to_string()).collect();
                let found = Found::scan(&models, &listed);
                let seen = (
                    found.cameras.clone(),
                    found.makers.clone(),
                    rel(&found.adobe_profiles),
                    rel(&found.user_profiles),
                    rel(&found.lens_profiles),
                );
                assert_eq!(&seen, expected, "{layout:?} {models:?}");
                assert_eq!(rel(&found.preset_folders), preset_folders, "{layout:?}");
                assert_eq!(found.user_presets, user_presets.len(), "{layout:?}");
                assert_eq!(rel(&found.catalogs), catalogs, "{layout:?}");
            }
        }
    }

    #[test]
    fn opening_setup_lists_adobe_s_folders_again_and_follows_the_extension() {
        use crate::app::extensions::adobe_files::tests::{host, poll_until};
        let dir = tempfile::tempdir().unwrap();
        rawmakase_lightroom::fake::install(dir.path(), rawmakase_lightroom::fake::Layout::MacOs)
            .unwrap();
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        editor.extensions = host(dir.path());
        // A frame, then frames until the scan is done.
        let frame = |editor: &mut Editor| {
            let started = std::time::Instant::now();
            loop {
                let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
                    editor.events(ui.ctx());
                    editor.onboarding_ui(ui);
                });
                output.textures_delta.clear();
                if !editor.onboarding.scanning() {
                    break;
                }
                assert!(started.elapsed().as_secs() < 30, "the scan never finished");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
        };
        editor.open_onboarding();
        frame(&mut editor);
        assert!(editor.adobe_files.asking());
        let row = editor.import_row(ImportKind::Presets);
        assert_eq!(row.status, "Looking…");
        poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
        frame(&mut editor);
        let catalogs = &editor.onboarding.found.catalogs;
        assert_eq!(catalogs.len(), 2);
        assert!(catalogs[0].ends_with("New.lrcat"), "the latest used first");
        assert_eq!(editor.onboarding.found.preset_folders.len(), 3);

        // Opened again, it lists again.
        let listing = editor.adobe_files.generation();
        editor.open_onboarding();
        frame(&mut editor);
        poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
        frame(&mut editor);
        assert!(editor.onboarding.scanned_listing > listing);
        assert_eq!(
            editor.onboarding.scanned_listing,
            editor.adobe_files.generation()
        );
        // The rows say where it looked.
        let hover = editor.import_row(ImportKind::CameraProfiles).hover.unwrap();
        assert!(hover.starts_with("Looks in\n"), "{hover}");
        assert_eq!(hover.matches("CameraProfiles").count(), 2, "{hover}");

        // Without Camera Raw's folders or a catalog, they say so.
        std::fs::remove_dir_all(dir.path().join("Library")).unwrap();
        std::fs::remove_dir_all(dir.path().join("home")).unwrap();
        editor.open_onboarding();
        frame(&mut editor);
        poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
        frame(&mut editor);
        let row = editor.import_row(ImportKind::LensProfiles);
        assert_eq!(row.status, "Lightroom's folders aren't on this computer");
        let status = editor.catalog_row().status;
        assert!(
            status.starts_with("None in ") && status.ends_with("Lightroom"),
            "{status}"
        );

        assert!(
            editor.lightroom_rows()[0].kind.is_none(),
            "the catalog's row"
        );

        // With the extension off, nothing is offered from Adobe's folders,
        // and no catalog is imported.
        editor.extensions.set_enabled(LIGHTROOM, false).unwrap();
        frame(&mut editor);
        assert!(editor.lightroom_rows().iter().all(|r| r.kind.is_some()));
        assert!(editor.onboarding.found.catalogs.is_empty());
        assert!(editor.onboarding.found.preset_folders.is_empty());
        let row = editor.import_row(ImportKind::Presets);
        assert_eq!(row.import, None);
        assert_eq!(
            row.status,
            "The Lightroom extension, which finds these, is off"
        );
    }

    /// An editor whose Lightroom extension lists a fake install in `dir`.
    fn listing_editor(dir: &std::path::Path) -> Editor {
        rawmakase_lightroom::fake::install(dir, rawmakase_lightroom::fake::Layout::MacOs).unwrap();
        let ctx = egui::Context::default();
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        editor.extensions = crate::app::extensions::adobe_files::tests::host(dir);
        editor
    }

    fn draw(editor: &mut Editor) {
        let ctx = editor.context.clone();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| editor.onboarding_ui(ui));
        output.textures_delta.clear();
    }

    /// Takes the scan's result once it is done.
    fn scanned(editor: &mut Editor) {
        let ctx = editor.context.clone();
        let started = std::time::Instant::now();
        while editor.onboarding.scanning() {
            assert!(started.elapsed().as_secs() < 30, "the scan never finished");
            std::thread::sleep(std::time::Duration::from_millis(5));
            editor.events(&ctx);
        }
    }

    /// Counting the presets reads their folders, which may be large or on a
    /// slow disk: the view draws meanwhile, and takes the result later.
    #[test]
    fn setup_scans_the_listing_off_the_ui_thread() {
        use crate::app::extensions::adobe_files::tests::poll_until;
        let dir = tempfile::tempdir().unwrap();
        let mut editor = listing_editor(dir.path());
        editor.open_onboarding();
        draw(&mut editor);
        poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
        draw(&mut editor);
        assert!(editor.onboarding.scanning());
        assert_eq!(editor.onboarding.found.user_presets, 0);
        scanned(&mut editor);
        assert_eq!(editor.onboarding.found.user_presets, 4);
    }

    /// Setup opened and finished before a first launch's listing arrives:
    /// the listing decides nothing any more.
    #[test]
    fn finishing_setup_settles_a_first_launch() {
        use crate::app::extensions::adobe_files::tests::poll_until;
        let dir = tempfile::tempdir().unwrap();
        let mut editor = listing_editor(dir.path());
        editor.session.onboarding_done = false;
        editor.adobe_files.first_launch = true;
        assert!(editor.adobe_files().is_none());
        editor.open_onboarding();
        editor.finish_onboarding(true);
        poll_until(&mut editor, |e| e.adobe_files().is_some());
        assert!(!editor.onboarding.visible);
        assert!(editor.session.onboarding_done);
    }

    /// A listing that failed isn't an empty one: the rows say why, and
    /// opening setup again lists again.
    #[test]
    fn a_failed_listing_says_so_and_setup_lists_again() {
        use crate::app::extensions::adobe_files::tests::poll_until;
        let dir = tempfile::tempdir().unwrap();
        let mut editor = listing_editor(dir.path());
        editor.open_onboarding();
        draw(&mut editor);
        editor.extensions.kill_program_for_tests(LIGHTROOM);
        poll_until(&mut editor, |e| e.adobe_files.failure().is_some());
        draw(&mut editor);
        scanned(&mut editor);
        let failed = "Couldn't look in Lightroom's folders: ";
        let row = editor.import_row(ImportKind::LensProfiles);
        assert!(row.status.starts_with(failed), "{}", row.status);
        assert!(editor.catalog_row().status.starts_with(failed));
        editor.open_onboarding();
        draw(&mut editor);
        poll_until(&mut editor, |e| e.adobe_files.listed().is_some());
        assert!(editor.adobe_files.failure().is_none());
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
        let mut editor = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
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
