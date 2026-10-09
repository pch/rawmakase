//! Application preferences, apart from the photo's develop settings. Laid out
//! like Lightroom's Preferences and Catalog Settings: tabs on the left, one
//! fixed-size page on the right so switching tabs never moves the window.
use super::Editor;
use super::dialogs::{CatalogDialog, FileDialog};
use super::widgets::{form_row, modal_frame, plural, pretty_path, primary_button};
use crate::app::theme;
use crate::camera_data::Demosaic;
use crate::catalog::CatalogLocation;
use crate::catalog::preview_cache::{PreviewCache, PreviewKind};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum Tab {
    #[default]
    General,
    Catalog,
    Profiles,
    Performance,
    Display,
    Automation,
}
impl Tab {
    const ALL: [Tab; 6] = [
        Tab::General,
        Tab::Catalog,
        Tab::Profiles,
        Tab::Performance,
        Tab::Display,
        Tab::Automation,
    ];
    fn title(self) -> &'static str {
        match self {
            Tab::General => "General",
            Tab::Catalog => "Catalog",
            Tab::Profiles => "Profiles & Presets",
            Tab::Performance => "Performance",
            Tab::Display => "Display",
            Tab::Automation => "Automation",
        }
    }
}

/// Disk usage shown on the pages, measured when the window opens, when a tab
/// is chosen and after an import or a purge, never every frame.
#[derive(Default)]
struct Usage {
    camera_profiles: usize,
    lens_profiles: usize,
    decode_cache: (usize, u64),
    /// The preview cache's file, and what each kind of preview holds of it.
    previews: u64,
    preview_kinds: Option<crate::catalog::preview_cache::PreviewUsage>,
    catalog: Option<u64>,
    folders: Option<usize>,
}

#[derive(Default)]
pub(super) struct Preferences {
    pub(super) open: bool,
    tab: Tab,
    usage: Usage,
    /// Measured with a file dialog open, so its import is counted on close.
    stale: bool,
    /// The status line when the window opened; only later messages are shown.
    status_at_open: String,
    /// Default Creator and Copyright for photos added from folders, as typed.
    defaults: crate::catalog::MetadataDefaults,
    /// Typed and not saved yet: saved when the field is left, or when the
    /// page or window is.
    defaults_dirty: bool,
    /// Saving failed; tried again after the next edit, not every frame.
    defaults_failed: bool,
    /// Raw Defaults' camera rows.
    pub(super) raw_defaults: super::raw_defaults::Form,
    /// Folder locations, read when the Catalog page first shows.
    pub(super) locations: Option<super::folder_locations::LocationsView>,
}

const WIDTH: f32 = 780.;
const HEIGHT: f32 = 520.;
const SIDEBAR: f32 = 196.;

fn camera_profiles_dir() -> PathBuf {
    crate::storage::data_dir().join("camera-profiles")
}
fn lens_profiles_dir() -> PathBuf {
    crate::storage::data_dir().join("lens-profiles")
}
fn presets_dir() -> PathBuf {
    crate::storage::data_dir().join("xmp-presets")
}
fn decode_cache_dir() -> PathBuf {
    crate::decode_cache::cache_dir().join("decoded")
}

/// Files under `dir` (with one of `extensions`, if any) and their total size.
fn files(dir: &Path, extensions: &[&str]) -> (usize, u64) {
    let mut out = (0, 0);
    let mut stack = vec![dir.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(path);
            } else if extensions.is_empty()
                || path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| extensions.iter().any(|x| e.eq_ignore_ascii_case(x)))
            {
                out.0 += 1;
                out.1 += meta.len();
            }
        }
    }
    out
}
fn bytes(n: u64) -> String {
    const GB: f64 = (1u64 << 30) as f64;
    const MB: f64 = (1u64 << 20) as f64;
    let n = n as f64;
    if n >= GB {
        format!("{:.1} GB", n / GB)
    } else if n >= MB {
        format!("{:.0} MB", n / MB)
    } else if n > 0. {
        format!("{:.0} KB", (n / 1024.).max(1.))
    } else {
        "Empty".into()
    }
}

impl Editor {
    pub(super) fn open_preferences(&mut self, tab: Tab) {
        self.preferences.open = true;
        self.preferences.tab = tab;
        self.preferences.status_at_open = self.status.clone();
        // Edits not saved yet are kept, not replaced by the file's.
        if !self.preferences.defaults_dirty {
            self.preferences.defaults = crate::catalog::MetadataDefaults::load();
        }
        self.measure_usage();
        self.measure_raw_defaults();
    }
    fn measure_usage(&mut self) {
        let catalog = self.library.as_ref().map(|l| &l.session.catalog);
        self.preferences.usage = Usage {
            camera_profiles: files(&camera_profiles_dir(), &["dcp", "xmp"]).0,
            lens_profiles: files(&lens_profiles_dir(), &["lcp"]).0,
            decode_cache: files(&decode_cache_dir(), &["decoded"]),
            previews: std::fs::metadata(PreviewCache::path()).map_or(0, |m| m.len()),
            preview_kinds: PreviewCache::open(&PreviewCache::path())
                .and_then(|cache| cache.usage())
                .ok(),
            catalog: catalog.and_then(|c| {
                let CatalogLocation::File(path) = c.location();
                std::fs::metadata(path).ok().map(|m| m.len())
            }),
            folders: catalog.and_then(|c| c.folders().ok().map(|f| f.len())),
        };
        self.preferences.stale = self.activity.is_dialog();
        self.save_computer_name();
        self.preferences.locations = None;
    }
    /// ⌘, (Ctrl+, elsewhere) opens Preferences, as in Lightroom.
    pub(super) fn preferences_shortcut(&mut self, ctx: &egui::Context) {
        if !self.preferences.open
            && !self.activity.is_busy()
            && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Comma))
        {
            self.open_preferences(Tab::General);
        }
        // Lightroom's Cmd+/: the keyboard shortcuts.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash)) {
            self.view.shortcuts = !self.view.shortcuts;
        }
    }
    /// Saves defaults still being typed, once their page is left.
    fn save_defaults(&mut self) {
        if !self.preferences.defaults_dirty || self.preferences.defaults_failed {
            return;
        }
        match self.preferences.defaults.save() {
            Ok(()) => self.preferences.defaults_dirty = false,
            Err(e) => {
                self.preferences.defaults_failed = true;
                self.status = format!("Metadata defaults not saved: {e:#}");
            }
        }
    }
    pub(super) fn preferences_window(&mut self, ctx: &egui::Context) {
        let palette = theme::palette(ctx);
        let closing = ctx.input(|i| i.viewport().close_requested());
        if !self.preferences.open || self.preferences.tab != Tab::Catalog || closing {
            self.save_defaults();
            self.save_computer_name();
        }
        if !self.preferences.open {
            return;
        }
        if self.preferences.stale && !self.activity.is_dialog() {
            self.measure_usage();
        }
        let response = egui::Modal::new(egui::Id::new("preferences"))
            .backdrop_color(Color32::from_black_alpha(140))
            .frame(modal_frame(&palette))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), Sense::hover());
                let sidebar = egui::Rect::from_min_size(rect.min, Vec2::new(SIDEBAR, HEIGHT));
                ui.painter().rect_filled(
                    sidebar,
                    egui::CornerRadius {
                        nw: 10,
                        sw: 10,
                        ne: 0,
                        se: 0,
                    },
                    palette.gray(27),
                );
                ui.painter().vline(
                    sidebar.right(),
                    sidebar.y_range(),
                    Stroke::new(1., palette.gray(45)),
                );
                let mut side = ui.new_child(
                    egui::UiBuilder::new().max_rect(sidebar.shrink2(Vec2::new(12., 20.))),
                );
                self.preferences_tabs(&mut side);
                let page = egui::Rect::from_min_max(
                    egui::pos2(sidebar.right() + 32., rect.top() + 26.),
                    egui::pos2(rect.right() - 32., rect.bottom() - 72.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(page));
                content.set_clip_rect(page.expand(4.));
                content.spacing_mut().item_spacing = Vec2::new(8., 10.);
                content.spacing_mut().button_padding = Vec2::new(12., 5.);
                content.spacing_mut().interact_size.y = 28.;
                content.label(
                    egui::RichText::new(self.preferences.tab.title())
                        .size(18.)
                        .color(palette.gray(236)),
                );
                content.add_space(10.);
                match self.preferences.tab {
                    // The usage report makes General taller than the window.
                    Tab::General => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| self.general_page(ui));
                    }
                    Tab::Catalog => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| self.catalog_page(ui));
                    }
                    Tab::Profiles => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| self.profiles_page(ui));
                    }
                    Tab::Performance => self.performance_page(&mut content),
                    Tab::Display => self.display_page(&mut content),
                    Tab::Automation => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| self.automation_page(ui));
                    }
                }
                let footer = egui::Rect::from_min_max(
                    egui::pos2(sidebar.right() + 32., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 16.),
                );
                ui.painter().hline(
                    (sidebar.right() + 1.)..=rect.right(),
                    rect.bottom() - 64.,
                    Stroke::new(1., palette.gray(45)),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = Vec2::new(18., 6.);
                let done = primary_button(&mut bar, "Done").clicked();
                // Only what happened while the window was open, e.g. an import.
                if self.status != self.preferences.status_at_open {
                    bar.add_space(16.);
                    bar.add(
                        egui::Label::new(
                            egui::RichText::new(&self.status)
                                .size(12.)
                                .color(palette.gray(160)),
                        )
                        .truncate(),
                    );
                }
                done
            });
        if response.inner || response.should_close() {
            self.preferences.open = false;
        }
    }
    fn preferences_tabs(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.spacing_mut().item_spacing.y = 2.;
        ui.label(
            egui::RichText::new("Preferences")
                .size(11.)
                .color(palette.gray(130)),
        );
        ui.add_space(8.);
        for tab in Tab::ALL {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::click());
            let selected = self.preferences.tab == tab;
            if selected {
                ui.painter().rect_filled(rect, 5., palette.accent());
            } else if response.hovered() {
                ui.painter().rect_filled(rect, 5., palette.gray(40));
            }
            ui.painter().text(
                rect.left_center() + Vec2::new(12., 0.),
                egui::Align2::LEFT_CENTER,
                tab.title(),
                egui::FontId::proportional(13.),
                if selected {
                    palette.on_accent_text(250)
                } else {
                    palette.gray(205)
                },
            );
            if response.clicked() && !selected {
                self.preferences.tab = tab;
                self.measure_usage();
            }
        }
    }

    fn general_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "About");
        form_row(ui, "RAWmakase", |ui| {
            value(ui, crate::updates::config().current_version);
            ui.add_space(8.);
            self.update_status(ui);
        });
        form_row(ui, "LibRaw", |ui| {
            value(ui, &crate::raw::version());
        });
        form_row(ui, "Updates", |ui| self.automatic_updates_checkbox(ui));
        gap(ui);
        group(ui, "Locations");
        let data = crate::storage::data_dir();
        form_row(ui, "App data", |ui| path_value(ui, &data));
        form_row(ui, "", |ui| reveal_button(ui, &data));
        gap(ui);
        group(ui, "Help");
        form_row(ui, "", |ui| {
            if ui.button("Keyboard Shortcuts").clicked() {
                self.view.shortcuts = true;
                self.preferences.open = false;
            }
            if ui.button("Import from Lightroom").clicked() {
                self.preferences.open = false;
                self.open_onboarding();
            }
        });
    }

    fn catalog_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let usage = &self.preferences.usage;
        if let Some(library) = &self.library {
            let location = library.session.catalog.location().clone();
            group(ui, "Current catalog");
            form_row(ui, "Name", |ui| value(ui, &location.name()));
            let CatalogLocation::File(path) = location;
            form_row(ui, "Location", |ui| path_value(ui, &path));
            form_row(ui, "", |ui| reveal_button(ui, &path));
            form_row(ui, "Photos", |ui| {
                value(ui, &library.session.photos.len().to_string());
            });
            form_row(ui, "Folders", |ui| {
                value(ui, &usage.folders.map_or("–".into(), |n| n.to_string()));
            });
            form_row(ui, "Catalog size", |ui| {
                value(ui, &usage.catalog.map_or("–".into(), bytes));
            });
        } else {
            group(ui, "Current catalog");
            form_row(ui, "", |ui| {
                value(ui, "No catalog is open.");
            });
        }
        form_row(ui, "Previews", |ui| {
            value(ui, &bytes(usage.previews));
        });
        let preview_kinds = usage.preview_kinds;
        if let Some(kinds) = preview_kinds {
            form_row(ui, "", |ui| {
                hint(
                    ui,
                    &format!(
                        "Thumbnails {} · Standard {} · 1:1 {}",
                        bytes(kinds.thumbnails),
                        bytes(kinds.standard),
                        bytes(kinds.one_to_one)
                    ),
                );
            });
        }
        let mut size = self.preview_builds.standard_size;
        form_row(ui, "Standard Preview Size", |ui| {
            egui::ComboBox::from_id_salt("standard-preview-size")
                .width(120.)
                .selected_text(format!("{size} pixels"))
                .show_ui(ui, |ui| {
                    for choice in crate::app::preview_build::STANDARD_SIZES {
                        ui.selectable_value(&mut size, choice, format!("{choice} pixels"));
                    }
                });
        });
        if size != self.preview_builds.standard_size {
            self.preview_builds.standard_size = size;
            let _ = self.save_session();
        }
        let mut discard = self.preview_builds.discard_one_to_one_after;
        form_row(ui, "Discard 1:1 Previews", |ui| {
            let name = |days| {
                crate::app::preview_build::DISCARD_CHOICES
                    .iter()
                    .find(|(d, _)| *d == days)
                    .map_or("", |(_, name)| name)
            };
            egui::ComboBox::from_id_salt("discard-one-to-one")
                .width(160.)
                .selected_text(name(discard))
                .show_ui(ui, |ui| {
                    for (days, title) in crate::app::preview_build::DISCARD_CHOICES {
                        ui.selectable_value(&mut discard, days, title);
                    }
                });
        });
        if discard != self.preview_builds.discard_one_to_one_after {
            self.preview_builds.discard_one_to_one_after = discard;
            let _ = self.save_session();
            self.expire_previews();
        }
        let mut clear = None;
        form_row(ui, "", |ui| {
            if ui
                .add_enabled(
                    preview_kinds.is_some_and(|k| k.standard > 0),
                    egui::Button::new("Clear Standard Previews"),
                )
                .clicked()
            {
                clear = Some(PreviewKind::Standard);
            }
            if ui
                .add_enabled(
                    preview_kinds.is_some_and(|k| k.one_to_one > 0),
                    egui::Button::new("Clear 1:1 Previews"),
                )
                .clicked()
            {
                clear = Some(PreviewKind::OneToOne);
            }
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Build previews from a photo's menu. Develop shows Standard previews while a photo opens; 1:1 previews let an offline photo be zoomed to 100% in the Loupe.",
            );
        });
        if let Some(kind) = clear {
            self.clear_previews(kind);
            self.status = match kind {
                PreviewKind::Standard => "Standard previews cleared",
                PreviewKind::OneToOne => "1:1 previews cleared",
            }
            .into();
            self.preferences.usage.preview_kinds = preview_kinds.map(|kinds| match kind {
                PreviewKind::Standard => crate::catalog::preview_cache::PreviewUsage {
                    standard: 0,
                    ..kinds
                },
                PreviewKind::OneToOne => crate::catalog::preview_cache::PreviewUsage {
                    one_to_one: 0,
                    ..kinds
                },
            });
        }
        gap(ui);
        self.folder_locations_block(ui);
        group(ui, "Metadata defaults");
        let defaults = &mut self.preferences.defaults;
        let mut left = false;
        for (label, text) in [
            ("Creator", &mut defaults.creator),
            ("Copyright", &mut defaults.copyright),
        ] {
            form_row(ui, label, |ui| {
                let field = ui.add(egui::TextEdit::singleline(text).desired_width(320.));
                if field.changed() {
                    self.preferences.defaults_dirty = true;
                    self.preferences.defaults_failed = false;
                }
                left |= field.lost_focus();
            });
        }
        form_row(ui, "", |ui| {
            hint(
                ui,
                "For photos added from folders, when neither the file nor its sidecar has one.",
            );
        });
        if left {
            self.save_defaults();
        }
        gap(ui);
        group(ui, "Catalogs");
        form_row(ui, "", |ui| {
            for (kind, label) in [
                (CatalogDialog::Open, "Open…"),
                (CatalogDialog::Create, "New…"),
                (CatalogDialog::ImportLightroom, "Import Lightroom Catalog…"),
            ] {
                if ui.button(label).clicked() {
                    self.catalog_dialog(kind, &ctx);
                }
            }
        });
    }

    fn profiles_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // First, as in Lightroom's Presets preferences.
        self.raw_defaults_block(ui);
        gap(ui);
        group(ui, "Black & white");
        form_row(ui, "", |ui| {
            let mut auto = self.first_conversion == super::treatment::FirstConversion::AutoMix;
            if ui
                .checkbox(
                    &mut auto,
                    "Apply auto mix when first converting to black and white",
                )
                .changed()
            {
                self.first_conversion = if auto {
                    super::treatment::FirstConversion::AutoMix
                } else {
                    super::treatment::FirstConversion::KeepMix
                };
                let _ = self.save_session();
            }
        });
        gap(ui);
        let usage = &self.preferences.usage;
        let (cameras, lenses) = (usage.camera_profiles, usage.lens_profiles);
        let presets = self.presets.library.presets.len();
        let mut chosen = None;
        group(ui, "Camera profiles");
        form_row(ui, "Imported", |ui| {
            value(ui, &plural(cameras, "profile", "profiles"));
        });
        form_row(ui, "", |ui| {
            if ui.button("Import Profiles…").clicked() {
                chosen = Some(FileDialog::CameraProfile);
            }
            reveal_button(ui, &camera_profiles_dir());
        });
        gap(ui);
        group(ui, "Lens profiles");
        form_row(ui, "Imported", |ui| {
            value(ui, &plural(lenses, "profile", "profiles"));
        });
        form_row(ui, "", |ui| {
            if ui.button("Import Lens Profiles…").clicked() {
                chosen = Some(FileDialog::LensProfile);
            }
            reveal_button(ui, &lens_profiles_dir());
        });
        gap(ui);
        group(ui, "Develop presets");
        form_row(ui, "Installed", |ui| {
            value(ui, &plural(presets, "preset", "presets"));
        });
        form_row(ui, "", |ui| {
            if ui.button("Import Preset…").clicked() {
                chosen = Some(FileDialog::ImportXmp);
            }
            reveal_button(ui, &presets_dir());
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "The Setup Assistant finds Lightroom's own profiles and presets on this Mac.",
            );
        });
        if let Some(kind) = chosen {
            self.dialog(kind, &ctx);
            self.preferences.stale = true;
        }
    }

    fn performance_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "Demosaic");
        let current = self.demosaic.effective();
        let mut picked = None;
        for (choice, label, note) in [
            (
                Demosaic::Rawmakase,
                "RAWmakase",
                "Faster, with equal or better detail.",
            ),
            (
                Demosaic::Libraw,
                "LibRaw",
                "AHD for Bayer sensors, Markesteijn for X-Trans.",
            ),
        ] {
            form_row(
                ui,
                if choice == Demosaic::Rawmakase {
                    "Engine"
                } else {
                    ""
                },
                |ui| {
                    if ui.radio(current == choice, label).clicked() && current != choice {
                        picked = Some(choice);
                    }
                    hint(ui, note);
                },
            );
        }
        if let Some(choice) = picked {
            self.demosaic = choice;
            let _ = self.save_session();
            // Takes effect on the next full-size decode, so the open photo is
            // reopened.
            if let Some(path) = self.document.path.clone() {
                let photo = self.document.catalog_photo;
                self.open_raw(path, photo);
            }
        }
        gap(ui);
        group(ui, "Decoded photo cache");
        let (entries, size) = self.preferences.usage.decode_cache;
        let dir = decode_cache_dir();
        form_row(ui, "Location", |ui| path_value(ui, &dir));
        form_row(ui, "Size", |ui| {
            value(
                ui,
                &format!(
                    "{} of 4 GB · {}",
                    bytes(size),
                    plural(entries, "photo", "photos")
                ),
            );
        });
        let mut purge = false;
        form_row(ui, "", |ui| {
            purge = ui
                .add_enabled(entries > 0, egui::Button::new("Purge Cache"))
                .clicked();
            reveal_button(ui, &dir);
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Reopening a photo skips decoding while it is cached. Purging only removes decoded copies, never photos or edits.",
            );
        });
        if purge {
            let removed = std::fs::read_dir(&dir)
                .into_iter()
                .flatten()
                .flatten()
                .filter(|e| e.path().extension().is_some_and(|x| x == "decoded"))
                .filter(|e| std::fs::remove_file(e.path()).is_ok())
                .count();
            self.status = format!(
                "Purged {} from the cache",
                plural(removed, "photo", "photos")
            );
            self.measure_usage();
        }
    }

    fn display_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        group(ui, "Interface");
        let choices = self.themes.choices();
        let name = |file: &Option<String>| match file {
            None => "RAWmakase".to_string(),
            Some(file) => choices.iter().find(|(f, _)| f == file).map_or_else(
                || fastframe_theme::display_name(file).to_string(),
                |(_, n)| n.clone(),
            ),
        };
        let selected = self.themes.selected();
        let mut chosen = selected.clone();
        form_row(ui, "Theme", |ui| {
            egui::ComboBox::from_id_salt("interface-theme")
                .width(220.)
                .selected_text(name(&chosen))
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut chosen, None, "RAWmakase");
                    for (file, label) in &choices {
                        ui.selectable_value(&mut chosen, Some(file.clone()), label);
                    }
                });
            reveal_button(ui, &super::theme::Themes::folder());
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                &self.themes.status().unwrap_or_else(|| {
                    "Palette files in the themes folder are listed here. On Omarchy the desktop's theme is followed until you choose another. The photo's backdrop stays neutral grey.".into()
                }),
            );
        });
        if chosen != selected {
            self.themes.choose(chosen);
            let _ = self.save_session();
        }
        gap(ui);
        group(ui, "Monitor profile");
        let monitor = self.view.monitor.clone();
        form_row(ui, "Profile", |ui| match &monitor {
            Some(path) => {
                value(ui, &path.file_name().unwrap_or_default().to_string_lossy());
            }
            None => {
                value(ui, "sRGB");
            }
        });
        let mut choose = false;
        let mut srgb = false;
        form_row(ui, "", |ui| {
            choose = ui.button("Choose Profile…").clicked();
            srgb = ui
                .add_enabled(monitor.is_some(), egui::Button::new("Use sRGB"))
                .clicked();
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Photos are shown in this profile. Choose your display's calibrated ICC profile, or keep sRGB.",
            );
        });
        if choose {
            self.dialog(FileDialog::MonitorProfile, &ctx);
        }
        if srgb {
            self.view.monitor = None;
            let _ = self.save_session();
            self.schedule();
        }
    }
}

pub(super) fn group(ui: &mut egui::Ui, title: &str) {
    ui.label(
        egui::RichText::new(title)
            .size(12.)
            .strong()
            .color(theme::palette(ui.ctx()).gray(175)),
    );
}
pub(super) fn gap(ui: &mut egui::Ui) {
    ui.add_space(14.);
}
fn value(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(egui::RichText::new(text).color(theme::palette(ui.ctx()).gray(225)))
            .truncate(),
    );
}
fn path_value(ui: &mut egui::Ui, path: &Path) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(pretty_path(path)).color(theme::palette(ui.ctx()).gray(225)),
        )
        .truncate(),
    )
    .on_hover_text(path.display().to_string());
}
pub(super) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(12.)
                .color(theme::palette(ui.ctx()).gray(135)),
        )
        .wrap(),
    );
}
fn reveal_button(ui: &mut egui::Ui, path: &Path) {
    let exists = path.exists();
    if ui
        .add_enabled(exists, egui::Button::new(crate::platform::reveal::LABEL))
        .clicked()
    {
        let _ = crate::platform::reveal::reveal(path);
    }
}
/// The workspace bar's Preferences button: a gear in a 28 px slot.
pub(super) fn gear_button(ui: &mut egui::Ui) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.), Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 4., palette.gray(38));
    }
    let color = palette.gray(if response.hovered() { 235 } else { 175 });
    super::icons::paint_at(
        ui.painter(),
        super::icons::Icon::Settings,
        rect.center(),
        15.,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
