//! Application preferences, apart from the photo's develop settings. Laid out
//! like Lightroom's Preferences and Catalog Settings: tabs on the left, one
//! fixed-size page on the right so switching tabs never moves the window.
use crate::app::Editor;
use crate::app::shell::dialogs::{CatalogDialog, FileDialog};
use crate::app::ui::design::{Tone, button, radius, size, space, text};
use crate::app::ui::icons::{self, Icon};
use crate::app::ui::theme;
use crate::app::ui::widgets::{form_row, modal_frame, plural, pretty_path, primary_button};
use crate::catalog::CatalogLocation;
use crate::catalog::preview_cache::{PreviewCache, PreviewKind};
use eframe::egui::{self, Color32, Sense, Stroke, Vec2};
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Default, PartialEq)]
pub(in crate::app) enum Tab {
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
    fn icon(self) -> Icon {
        match self {
            Tab::General => Icon::Settings,
            Tab::Catalog => Icon::Catalog,
            Tab::Profiles => Icon::Presets,
            Tab::Performance => Icon::Performance,
            Tab::Display => Icon::Monitor,
            Tab::Automation => Icon::Automation,
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
    /// AI Denoise results: entries and bytes.
    ai_cache: (usize, u64),
    /// The preview cache's file, and what each kind of preview holds of it.
    previews: u64,
    preview_kinds: Option<crate::catalog::preview_cache::PreviewUsage>,
    catalog: Option<u64>,
    folders: Option<usize>,
}

#[derive(Default)]
pub(in crate::app) struct Preferences {
    pub(in crate::app) open: bool,
    tab: Tab,
    usage: Usage,
    /// Measured with a file dialog open, so its import is counted on close.
    stale: bool,
    /// Remove was clicked for Lensfun's profiles, waiting for the confirmation.
    lensfun_remove: bool,
    /// The status line when the window opened; only later messages are shown.
    status_at_open: String,
    /// Default Creator and Copyright for photos added from folders, as typed.
    defaults: crate::catalog::MetadataDefaults,
    /// Typed and not saved yet: saved when the field is left, or when the
    /// page or window is.
    defaults_dirty: bool,
    /// Saving failed; tried again after the next edit, not every frame.
    defaults_failed: bool,
    /// What is done with photos added from folders (Stack Camera Bursts),
    /// saved as it is changed.
    import: crate::catalog::ImportOptions,
    /// Raw Defaults' camera rows.
    pub(in crate::app) raw_defaults: crate::app::preferences::raw_defaults::Form,
    /// Folder locations, read when the Catalog page first shows.
    pub(in crate::app) locations: Option<crate::app::catalog::folder_locations::LocationsView>,
}

const WIDTH: f32 = 860.;
const HEIGHT: f32 = 600.;
const SIDEBAR: f32 = 196.;
/// The height of the bar over the page.
const BAR: f32 = 52.;

fn camera_profiles_dir() -> PathBuf {
    crate::storage::data_dir().join("camera-profiles")
}
fn lens_profiles_dir() -> PathBuf {
    crate::storage::data_dir().join("lens-profiles")
}
fn presets_dir() -> PathBuf {
    crate::storage::data_dir().join("xmp-presets")
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
    pub(in crate::app) fn open_preferences(&mut self, tab: Tab) {
        self.preferences.open = true;
        self.preferences.tab = tab;
        self.preferences.status_at_open = self.messages.line().to_string();
        // Edits not saved yet are kept, not replaced by the file's.
        if !self.preferences.defaults_dirty {
            self.preferences.defaults = crate::catalog::MetadataDefaults::load();
        }
        self.preferences.import = crate::catalog::ImportOptions::load();
        self.measure_usage();
        self.measure_raw_defaults();
    }
    fn measure_usage(&mut self) {
        let catalog = self.library.as_ref().map(|l| l.session.catalog());
        self.preferences.usage = Usage {
            camera_profiles: files(&camera_profiles_dir(), &["dcp", "xmp"]).0,
            lens_profiles: files(&lens_profiles_dir(), &["lcp"]).0,
            decode_cache: files(&self.decode_cache.dir(), &["decoded"]),
            ai_cache: files(&self.ai_denoise_cache().dir(), &["aidenoise"]),
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
    pub(in crate::app) fn preferences_shortcut(&mut self, ctx: &egui::Context) {
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
                self.messages
                    .fail(format!("Metadata defaults not saved: {e:#}"));
            }
        }
    }
    pub(in crate::app) fn preferences_window(&mut self, ctx: &egui::Context) {
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
                    palette.tone(Tone::BAR),
                );
                ui.painter().vline(
                    sidebar.right(),
                    sidebar.y_range(),
                    Stroke::new(1., palette.tone(Tone::RAISED)),
                );
                // The first tab sits level with the top bar.
                let mut side = ui.new_child(
                    egui::UiBuilder::new().max_rect(
                        sidebar
                            .with_min_y(sidebar.top() + (BAR - 32.) / 2.)
                            .shrink2(Vec2::new(12., 0.)),
                    ),
                );
                self.preferences_tabs(&mut side);
                let close = self.preferences_bar(ui, rect.with_min_x(sidebar.right()));
                let page = egui::Rect::from_min_max(
                    egui::pos2(sidebar.right() + 32., rect.top() + BAR + 20.),
                    egui::pos2(rect.right() - 32., rect.bottom() - 72.),
                );
                let mut content = ui.new_child(egui::UiBuilder::new().max_rect(page));
                content.set_clip_rect(page.expand(4.));
                content.spacing_mut().item_spacing = Vec2::new(space::M, space::L);
                content.spacing_mut().button_padding = Vec2::new(space::L, space::S);
                content.spacing_mut().interact_size.y = size::FIELD;
                match self.preferences.tab {
                    Tab::General => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| cards(ui, |ui| self.general_page(ui)));
                    }
                    Tab::Catalog => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| cards(ui, |ui| self.catalog_page(ui)));
                    }
                    Tab::Profiles => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| cards(ui, |ui| self.profiles_page(ui)));
                    }
                    // Taller than the window while a chosen folder is missing.
                    Tab::Performance => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| cards(ui, |ui| self.performance_page(ui)));
                    }
                    Tab::Display => cards(&mut content, |ui| self.display_page(ui)),
                    Tab::Automation => {
                        egui::ScrollArea::vertical()
                            .auto_shrink(false)
                            .show(&mut content, |ui| cards(ui, |ui| self.automation_page(ui)));
                    }
                }
                let footer = egui::Rect::from_min_max(
                    egui::pos2(sidebar.right() + 32., rect.bottom() - 56.),
                    egui::pos2(rect.right() - 24., rect.bottom() - 16.),
                );
                ui.painter().hline(
                    (sidebar.right() + 1.)..=rect.right(),
                    rect.bottom() - 64.,
                    Stroke::new(1., palette.tone(Tone::RAISED)),
                );
                let mut bar = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(footer)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                bar.spacing_mut().button_padding = button::DIALOG;
                let done = primary_button(&mut bar, "Done").clicked();
                // Only what happened while the window was open, e.g. an import.
                // The bar is behind this window, so a problem is said here too.
                if self.messages.line() != self.preferences.status_at_open {
                    bar.add_space(space::XL);
                    bar.add(
                        egui::Label::new(
                            egui::RichText::new(self.messages.line())
                                .size(text::LABEL)
                                .color(palette.tone(Tone::SECONDARY)),
                        )
                        .truncate(),
                    );
                }
                done || close
            });
        if response.inner || response.should_close() {
            self.preferences.open = false;
        }
    }
    /// The bar over the page: where you are, "Preferences / Catalog", and a
    /// close button. Returns whether it was clicked.
    fn preferences_bar(&self, ui: &mut egui::Ui, area: egui::Rect) -> bool {
        let palette = theme::palette(ui.ctx());
        let bar = area.with_max_y(area.top() + BAR);
        ui.painter().hline(
            (bar.left() + 1.)..=bar.right(),
            bar.bottom(),
            Stroke::new(1., palette.tone(Tone::RAISED)),
        );
        let font = egui::FontId::proportional(text::LARGE);
        let mut x = bar.left() + 32.;
        for (text, color) in [
            ("Preferences", palette.tone(Tone::MUTED)),
            ("/", palette.tone(Tone::DISABLED)),
            (self.preferences.tab.title(), palette.tone(Tone::STRONG)),
        ] {
            let drawn = ui.painter().text(
                egui::pos2(x, bar.center().y),
                egui::Align2::LEFT_CENTER,
                text,
                font.clone(),
                color,
            );
            x = drawn.right() + 10.;
        }
        let button = egui::Rect::from_center_size(
            egui::pos2(bar.right() - 30., bar.center().y),
            Vec2::splat(26.),
        );
        ui.scope_builder(egui::UiBuilder::new().max_rect(button), |ui| {
            crate::app::shell::shortcuts::icon_button(ui, Icon::Close, "Close · Esc").clicked()
        })
        .inner
    }
    fn preferences_tabs(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        ui.spacing_mut().item_spacing.y = space::XXS;
        for tab in Tab::ALL {
            let (rect, response) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.), Sense::click());
            let selected = self.preferences.tab == tab;
            if selected {
                ui.painter()
                    .rect_filled(rect, radius::CARD, palette.tone(Tone::RAISED));
            } else if response.hovered() {
                ui.painter()
                    .rect_filled(rect, radius::CARD, palette.tone(Tone::PANEL));
            }
            let tint = palette.tone(if selected { Tone::STRONG } else { Tone::LABEL });
            icons::paint_at(
                ui.painter(),
                tab.icon(),
                rect.left_center() + Vec2::new(20., 0.),
                15.,
                if selected { palette.accent() } else { tint },
            );
            ui.painter().text(
                rect.left_center() + Vec2::new(38., 0.),
                egui::Align2::LEFT_CENTER,
                tab.title(),
                egui::FontId::proportional(text::BODY),
                tint,
            );
            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
            response.widget_info(|| {
                egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, tab.title())
            });
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
            ui.add_space(space::M);
            self.update_status(ui);
        });
        form_row(ui, "RAW decoder", |ui| {
            value(ui, &crate::raw::version());
        });
        form_row(ui, "Updates", |ui| self.automatic_updates_checkbox(ui));
        for (name, credit) in third_party_models() {
            form_row(ui, name, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(credit).size(text::SMALL)).wrap());
            });
        }
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
        form_row(ui, "", |ui| {
            if ui.button("Report an Issue…").clicked() {
                self.open_report_issue();
            }
        });
    }

    fn catalog_page(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let usage = &self.preferences.usage;
        let (mut paused, mut pause_changed) = (self.session.checksums_paused, false);
        if let Some(library) = &self.library {
            let location = library.session.catalog().location().clone();
            group(ui, "Current catalog");
            form_row(ui, "Name", |ui| value(ui, &location.name()));
            let CatalogLocation::File(path) = location;
            form_row(ui, "Location", |ui| path_value(ui, &path));
            form_row(ui, "", |ui| reveal_button(ui, &path));
            form_row(ui, "Photos", |ui| {
                value(ui, &library.session.photos().len().to_string());
            });
            form_row(ui, "Folders", |ui| {
                value(ui, &usage.folders.map_or("–".into(), |n| n.to_string()));
            });
            form_row(ui, "Catalog size", |ui| {
                value(ui, &usage.catalog.map_or("–".into(), bytes));
            });
            let (read, masters) = library.session.checksum_count();
            form_row(ui, "Checksums", |ui| {
                value(ui, &format!("{read} of {masters} photos"));
            });
            form_row(ui, "", |ui| {
                pause_changed = ui
                    .checkbox(&mut paused, "Pause reading checksums")
                    .changed();
            });
            form_row(ui, "", |ui| {
                hint(
                    ui,
                    "Read in the background, to find photos moved or renamed outside RAWmakase.",
                );
            });
        } else {
            group(ui, "Current catalog");
            form_row(ui, "", |ui| {
                value(ui, "No catalog is open.");
            });
        }
        if pause_changed {
            self.session.checksums_paused = paused;
            let _ = self.save_session();
        }
        gap(ui);
        group(ui, "Previews");
        form_row(ui, "Size", |ui| {
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
                    for choice in crate::app::library::preview_build::STANDARD_SIZES {
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
                crate::app::library::preview_build::DISCARD_CHOICES
                    .iter()
                    .find(|(d, _)| *d == days)
                    .map_or("", |(_, name)| name)
            };
            egui::ComboBox::from_id_salt("discard-one-to-one")
                .width(160.)
                .selected_text(name(discard))
                .show_ui(ui, |ui| {
                    for (days, title) in crate::app::library::preview_build::DISCARD_CHOICES {
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
            let text: String = match kind {
                PreviewKind::Standard => "Standard previews cleared",
                PreviewKind::OneToOne => "1:1 previews cleared",
            }
            .into();
            self.messages.done(text);
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
        group(ui, "Decoded images");
        let mut cache_size = (self.decode_cache.limit() >> 30) as u32;
        let mut field = None;
        form_row(ui, "Cache size limit", |ui| {
            // A typed size applies once entered, so a cache write while typing
            // 128 never trims to 1 GiB.
            field = Some(
                ui.add(
                    egui::DragValue::new(&mut cache_size)
                        .range(crate::app::preferences::session::DECODED_CACHE_SIZE_RANGE)
                        .suffix(" GiB")
                        .update_while_editing(false),
                ),
            );
        });
        if u64::from(cache_size) << 30 != self.decode_cache.limit() {
            self.decode_cache.set_limit(u64::from(cache_size) << 30);
        }
        // A drag changes the size every frame; the session is written once it ends.
        if field.is_some_and(|r| r.drag_stopped() || (r.changed() && !r.dragged())) {
            let _ = self.save_session();
        }
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Keeps decoded RAWs for faster reopening. Shared by all catalogs; default 4 GiB.",
            );
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "A smaller limit removes older entries on the next cache write.",
            );
        });
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
        group(ui, "Adding photos");
        let import = &mut self.preferences.import;
        form_row(ui, "", |ui| {
            if ui
                .checkbox(&mut import.stack_bursts, "Stack camera bursts")
                .changed()
                && let Err(e) = import.save()
            {
                self.messages.fail(format!("Preferences not saved: {e:#}"));
            }
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "For photos added from folders or by Synchronize Folder: once their files are \
                 read, each burst or bracket their camera numbered or shot in continuous drive \
                 becomes a stack. Photos whose camera says nothing are left alone.",
            );
        });
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
            let mut auto = self.session.first_conversion
                == crate::app::develop::treatment::FirstConversion::AutoMix;
            if ui
                .checkbox(
                    &mut auto,
                    "Apply auto mix when first converting to black and white",
                )
                .changed()
            {
                self.session.first_conversion = if auto {
                    crate::app::develop::treatment::FirstConversion::AutoMix
                } else {
                    crate::app::develop::treatment::FirstConversion::KeepMix
                };
                let _ = self.save_session();
            }
        });
        gap(ui);
        let usage = &self.preferences.usage;
        let (cameras, lenses) = (usage.camera_profiles, usage.lens_profiles);
        let presets = self.presets.library.presets.len();
        let mut chosen = None;
        let mut luts = false;
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
        // On a row of its own: with the folder's button beside it, the row is too
        // wide where that button is "Show in File Manager".
        form_row(ui, "", |ui| {
            if ui.button("Import LUTs…").clicked() {
                luts = true;
            }
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
        self.lensfun_block(ui);
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
        if luts {
            self.open_lut_import();
            self.preferences.stale = true;
        }
    }

    /// Lensfun's community lens profiles: installed or not, checking for and
    /// installing a newer database, removing it, and Lens Corrections' suggestions.
    fn lensfun_block(&mut self, ui: &mut egui::Ui) {
        use crate::app::develop::lensfun::Job;
        group(ui, "Lensfun profiles");
        let installed = self.lensfun.installed();
        let running = self.lensfun.running();
        let mut start = None;
        form_row(ui, "Installed", |ui| match &installed {
            Some(m) => value(
                ui,
                &format!(
                    "Version {} · {}",
                    m.date(),
                    plural(m.lenses, "lens", "lenses")
                ),
            ),
            None => value(ui, "Not installed"),
        });
        if let Some(job) = running {
            form_row(ui, "", |ui| {
                match (job, self.lensfun.progress()) {
                    (Job::Install, Some((done, total))) if total > 0 => {
                        ui.add(
                            egui::ProgressBar::new(done as f32 / total as f32)
                                .desired_width(160.)
                                .text(format!("{:.1} MB", done as f32 / 1e6)),
                        );
                    }
                    _ => {
                        crate::app::ui::widgets::spinner(ui, 14.);
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.lensfun.cancel();
                }
            });
        } else if installed.is_none() {
            form_row(ui, "", |ui| {
                if ui
                    .button(format!(
                        "Download ({:.1} MB)",
                        crate::lensfun_install::ABOUT_MEGABYTES
                    ))
                    .clicked()
                {
                    start = Some(Job::Install);
                }
            });
        } else if self.preferences.lensfun_remove {
            form_row(ui, "", |ui| {
                hint(
                    ui,
                    "Remove Lensfun profiles? Edits that use a Lensfun lens keep their corrections.",
                );
            });
            form_row(ui, "", |ui| {
                if ui.button("Remove").clicked() {
                    start = Some(Job::Remove);
                    self.preferences.lensfun_remove = false;
                }
                if ui.button("Cancel").clicked() {
                    self.preferences.lensfun_remove = false;
                }
            });
        } else {
            form_row(ui, "", |ui| {
                match self.lensfun.newer() {
                    Some(newer) => {
                        let [y, m, d, ..] = crate::time::utc(newer);
                        if ui
                            .button(format!("Update to {y:04}-{m:02}-{d:02}"))
                            .clicked()
                        {
                            start = Some(Job::Install);
                        }
                    }
                    None => {
                        if ui.button("Check for Updates").clicked() {
                            start = Some(Job::Check);
                        }
                    }
                }
                if ui.button("Remove…").clicked() {
                    self.preferences.lensfun_remove = true;
                }
                reveal_button(ui, &self.lensfun.dir());
            });
            if self.lensfun.up_to_date() {
                form_row(ui, "", |ui| hint(ui, "Up to date"));
            }
        }
        for job in [Job::Install, Job::Check, Job::Remove] {
            if let Some(why) = self.lensfun.failure(job) {
                form_row(ui, "", |ui| hint(ui, why));
            }
        }
        form_row(ui, "", |ui| {
            let mut suggest = self.lensfun.suggest;
            if ui
                .checkbox(&mut suggest, "Suggest Lensfun profiles in Lens Corrections")
                .changed()
            {
                self.lensfun.suggest = suggest;
                let _ = self.save_session();
            }
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Community lens profiles, downloaded from Lensfun on request; never applied unless you choose one. Edits keep a copy of the calibration they use.",
            );
        });
        form_row(
            ui,
            "",
            crate::app::develop::panels::inspector::lensfun_terms,
        );
        if let Some(job) = start {
            self.lensfun.start(job, self.tx.clone(), ui.ctx().clone());
        }
    }

    fn performance_page(&mut self, ui: &mut egui::Ui) {
        group(ui, "Decoded photo cache");
        let (entries, size) = self.preferences.usage.decode_cache;
        let dir = self.decode_cache.dir();
        let folder = self.decode_cache.folder();
        form_row(ui, "Location", |ui| path_value(ui, &dir));
        form_row(ui, "Size", |ui| {
            value(
                ui,
                &format!(
                    "{} of {} GiB · {}",
                    bytes(size),
                    self.decode_cache.limit() >> 30,
                    plural(entries, "photo", "photos")
                ),
            );
        });
        let (ai_entries, ai_size) = self.preferences.usage.ai_cache;
        form_row(ui, "AI Denoise", |ui| {
            value(
                ui,
                &format!(
                    "{} of {} GiB · {}",
                    bytes(ai_size),
                    self.ai_denoise_cache().limit() >> 30,
                    plural(ai_entries, "result", "results")
                ),
            );
        });
        let (mut purge, mut choose, mut default) = (false, false, false);
        form_row(ui, "", |ui| {
            choose = ui.button("Choose Folder…").clicked();
            default = ui
                .add_enabled(folder.is_some(), egui::Button::new("Use Default"))
                .clicked();
        });
        form_row(ui, "", |ui| {
            purge = ui
                .add_enabled(entries + ai_entries > 0, egui::Button::new("Purge Cache"))
                .clicked();
            reveal_button(ui, &dir);
        });
        if let Some(folder) = folder.as_deref().filter(|f| !f.is_dir()) {
            form_row(ui, "", |ui| {
                hint(
                    ui,
                    &format!(
                        "{} is not available, so photos are not cached until it is.",
                        pretty_path(folder)
                    ),
                );
            });
        }
        form_row(ui, "", |ui| {
            hint(
                ui,
                "Reopening a photo skips decoding while it is cached. Choose a folder on another drive to save space on this one; moving the cache removes the decoded copies in the old location. Purging only removes decoded copies, never photos or edits.",
            );
        });
        if purge {
            let removed = self.decode_cache.purge();
            let denoised = self.ai_denoise_cache().purge();
            self.messages.done(format!(
                "Purged {} and {} from the cache",
                plural(removed, "photo", "photos"),
                plural(denoised, "AI Denoise result", "AI Denoise results")
            ));
            self.measure_usage();
        }
        if choose {
            self.dialog(FileDialog::DecodeCacheFolder, &ui.ctx().clone());
        }
        if default {
            self.move_decode_cache(None);
        }
        gap(ui);
        group(ui, "Thumbnail cache");
        let thumbnail_budget = crate::catalog::preview_cache::thumbnail_budget();
        let mut budget = thumbnail_budget;
        form_row(ui, "Size limit", |ui| {
            egui::ComboBox::from_id_salt("thumbnail-cache-size")
                .width(120.)
                .selected_text(bytes(budget))
                .show_ui(ui, |ui| {
                    // A size saved by another release, not offered here, stays
                    // until another is chosen.
                    let offered = session::THUMBNAIL_CACHE_SIZES.map(|mib| u64::from(mib) << 20);
                    if !offered.contains(&thumbnail_budget) {
                        ui.selectable_value(&mut budget, thumbnail_budget, bytes(thumbnail_budget));
                    }
                    for choice in offered {
                        ui.selectable_value(&mut budget, choice, bytes(choice));
                    }
                });
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                "The Library's thumbnails are kept up to this size, the least recently shown going first; one shown again is made again from its photo. For a large catalog on a network drive, choose more.",
            );
        });
        if budget != thumbnail_budget {
            crate::catalog::preview_cache::set_thumbnail_budget(budget);
            let _ = self.save_session();
            if budget < thumbnail_budget {
                // Opening prunes to the budget, and the space freed is given
                // back to the disk, off the UI thread: for gigabytes that takes
                // a minute or more. Each step is its own write, so quitting
                // meanwhile loses nothing; what is left is given back by later
                // prunes. Nothing waits for this thread, and a failure leaves the
                // file as large as it was.
                std::thread::spawn(|| {
                    let path = crate::catalog::preview_cache::PreviewCache::path();
                    if let Ok(mut cache) = crate::catalog::preview_cache::PreviewCache::open(&path)
                    {
                        let _ = cache.release_free_space();
                    }
                });
            }
        }
    }
    /// Caches decoded images in `folder`, or the default directory for `None`,
    /// and removes those in the old location: moving is usually meant to free
    /// that drive. A folder that cannot be written keeps the cache where it is.
    pub(in crate::app) fn move_decode_cache(&mut self, folder: Option<PathBuf>) {
        let removed = match self.decode_cache.relocate(folder.as_deref()) {
            Ok(removed) => removed,
            Err(e) => {
                self.messages
                    .fail(format!("The cache can't be moved there: {e}"));
                return;
            }
        };
        // AI Denoise's results follow, in a subfolder of their own.
        if let Err(e) = self.ai_denoise_cache().relocate(folder.as_deref()) {
            self.messages.fail(format!(
                "AI Denoise results stay in {}: {e}",
                self.ai_denoise_cache().dir().display()
            ));
        }
        let saved = self.save_session();
        let moved = match &saved {
            Ok(()) if removed > 0 => format!(
                "Photos are now cached in {}; {} removed from the old location",
                self.decode_cache.dir().display(),
                plural(removed, "decoded photo", "decoded photos")
            ),
            Ok(()) => format!(
                "Photos are now cached in {}",
                self.decode_cache.dir().display()
            ),
            Err(e) => format!("Cache folder not saved: {e:#}"),
        };
        if saved.is_ok() {
            self.messages.done(moved);
        } else {
            self.messages.fail(moved);
        }
        self.measure_usage();
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
        });
        // On its own row: beside the picker it is wider than the page.
        form_row(ui, "", |ui| {
            reveal_button(ui, &crate::app::ui::theme::Themes::folder())
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                &self.themes.status().unwrap_or_else(|| {
                    "Palette files in the themes folder are listed here. On Omarchy the desktop's theme is followed until you choose another.".into()
                }),
            );
        });
        if chosen != selected {
            self.themes.choose(chosen);
            let _ = self.save_session();
        }
        gap(ui);
        group(ui, "Monitor profile");
        use crate::app::preferences::monitor::MonitorProfile;
        let monitor = self.view.monitor.clone();
        form_row(ui, "Profile", |ui| {
            value(
                ui,
                &crate::app::preferences::monitor::describe(
                    &monitor,
                    self.view.system_display.display(),
                ),
            )
        });
        let mut choose = false;
        let mut chosen = None;
        form_row(ui, "", |ui| {
            choose = ui.button("Choose Profile…").clicked();
            for (choice, label) in [
                (MonitorProfile::System, "Use System Profile"),
                (MonitorProfile::Srgb, "Use sRGB"),
            ] {
                if ui
                    .add_enabled(monitor != choice, egui::Button::new(label))
                    .clicked()
                {
                    chosen = Some(choice);
                }
            }
        });
        form_row(ui, "", |ui| {
            hint(
                ui,
                if cfg!(target_os = "macos") {
                    "The system profile lets macOS match colors to the current display. A chosen profile, or sRGB for a display set to sRGB, is applied instead with no matching by macOS."
                } else if cfg!(windows) {
                    "The system profile is the one Windows assigns to the display the window is on, unless Windows manages color itself. Choose another ICC profile, or sRGB for a display set to sRGB."
                } else {
                    "The system profile is not read on Linux yet, so photos are shown as sRGB. Choose your display's calibrated ICC profile."
                },
            );
        });
        if choose {
            self.dialog(FileDialog::MonitorProfile, &ctx);
        }
        if let Some(choice) = chosen {
            self.view.monitor = choice;
            let _ = self.save_session();
            self.schedule();
        }
    }
}

/// The room between a card's edge and what it holds.
const CARD_INSET: f32 = space::XL;
const CARD_PAD: f32 = 10.;

/// A card under way: its background, drawn once its rows are known.
#[derive(Clone, Copy)]
struct OpenCard {
    layer: egui::LayerId,
    clip: egui::Rect,
    background: egui::layers::ShapeIdx,
    x: egui::Rangef,
    top: f32,
}
fn card_id() -> egui::Id {
    egui::Id::new("preferences-open-card")
}

/// Lays out a page as titled cards: each [`group`] starts one, which the next
/// group, a [`gap`] or the page's end closes. The page's rows sit inset in
/// the cards.
fn cards(ui: &mut egui::Ui, page: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin {
            left: CARD_INSET as i8,
            right: CARD_INSET as i8 + 4,
            top: 0,
            bottom: 12,
        })
        .show(ui, |ui| {
            page(ui);
            close_card(ui);
        });
}
/// A section's title, and the card its rows sit on.
pub(in crate::app) fn group(ui: &mut egui::Ui, title: &str) {
    close_card(ui);
    // Over the card's edge, in line with the page's title.
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.), Sense::hover());
    ui.painter().text(
        egui::pos2(rect.left() - CARD_INSET + 2., rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::proportional(text::LABEL),
        theme::palette(ui.ctx()).tone(Tone::SOFT),
    );
    let top = rect.bottom() + 6.;
    let card = OpenCard {
        layer: ui.layer_id(),
        clip: ui.clip_rect(),
        background: ui.painter().add(egui::Shape::Noop),
        x: ui.max_rect().x_range().expand(CARD_INSET),
        top,
    };
    ui.ctx().data_mut(|d| d.insert_temp(card_id(), card));
    ui.add_space(space::S + CARD_PAD - ui.spacing().item_spacing.y);
}
/// The space between sections; it ends the open card.
pub(in crate::app) fn gap(ui: &mut egui::Ui) {
    close_card(ui);
    ui.add_space(space::XL);
}
fn close_card(ui: &mut egui::Ui) {
    let Some(card) = ui.ctx().data_mut(|d| {
        let card = d.get_temp::<OpenCard>(card_id());
        d.remove::<OpenCard>(card_id());
        card
    }) else {
        return;
    };
    let palette = theme::palette(ui.ctx());
    let bottom = ui.cursor().top() - ui.spacing().item_spacing.y + CARD_PAD;
    let rect = egui::Rect::from_x_y_ranges(card.x, card.top..=bottom);
    egui::Painter::new(ui.ctx().clone(), card.layer, card.clip).set(
        card.background,
        egui::epaint::RectShape::new(
            rect,
            8.,
            palette.tone(Tone::CARD),
            Stroke::new(1., palette.tone(Tone::RAISED)),
            egui::StrokeKind::Inside,
        ),
    );
    ui.add_space(CARD_PAD);
}
fn value(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text).color(theme::palette(ui.ctx()).tone(Tone::TEXT)),
        )
        .truncate(),
    );
}
fn path_value(ui: &mut egui::Ui, path: &Path) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(pretty_path(path)).color(theme::palette(ui.ctx()).tone(Tone::TEXT)),
        )
        .truncate(),
    )
    .on_hover_text(path.display().to_string());
}
pub(in crate::app) fn hint(ui: &mut egui::Ui, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .size(text::LABEL)
                .color(theme::palette(ui.ctx()).tone(Tone::MUTED)),
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
pub(in crate::app) fn gear_button(ui: &mut egui::Ui) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(28.), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, radius::CONTROL, palette.tone(Tone::FIELD));
    }
    let color = palette.tone(if response.hovered() {
        Tone::STRONG
    } else {
        Tone::LABEL
    });
    crate::app::ui::icons::paint_at(
        ui.painter(),
        crate::app::ui::icons::Icon::Settings,
        rect.center(),
        15.,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decoded_cache_preference_is_loaded_and_saved_with_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let session = crate::app::preferences::session::Session {
            decoded_cache_size_gib: Some(32),
            ..Default::default()
        };
        let mut e = Editor::with_context(&egui::Context::default(), None, session, None);
        e.session.set_file(Some(path.clone()));
        assert_eq!(e.decode_cache.limit(), 32 << 30);
        e.decode_cache.set_limit(1 << 50);
        e.save_session().unwrap();
        let saved: crate::app::preferences::session::Session =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(saved.decoded_cache_size_gib(), 1_048_576);
    }

    /// Issue 609: decoded images can move to another drive. The old location's
    /// entries go, the choice reaches the workers' cache and survives a restart,
    /// and Use Default moves them back.
    #[test]
    fn the_decoded_cache_moves_to_a_chosen_folder_and_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.session.set_file(Some(path.clone()));
        // Set, not relocated: relocating would purge the user's real cache.
        let old = dir.path().join("old");
        e.decode_cache.set_folder(Some(&old));
        std::fs::create_dir(&old).unwrap();
        std::fs::create_dir(e.decode_cache.dir()).unwrap();
        let entry = e.decode_cache.dir().join("0123456789abcdef.decoded");
        std::fs::write(&entry, "pixels").unwrap();
        let worker = e.decode_cache.clone();
        let chosen = dir.path().join("other drive");
        std::fs::create_dir(&chosen).unwrap();
        e.move_decode_cache(Some(chosen.clone()));
        assert!(!entry.exists(), "{}", e.messages.line());
        assert_eq!(
            worker.dir(),
            chosen.join(crate::decode_cache::CHOSEN_SUBFOLDER)
        );
        let saved: crate::app::preferences::session::Session =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            saved.decoded_cache_folder.as_deref(),
            Some(chosen.as_path())
        );
        let restarted = Editor::with_context(&ctx, None, saved, None);
        assert_eq!(restarted.decode_cache.dir(), worker.dir());

        // Moving back purges only the chosen folder, never the default one.
        e.move_decode_cache(None);
        assert_eq!(worker.folder(), None);
        let saved: crate::app::preferences::session::Session =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved.decoded_cache_folder, None);
    }

    /// A folder that cannot be written keeps the cache, and its entries, where
    /// they are.
    #[test]
    fn an_unwritable_cache_folder_keeps_the_cache_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let mut e = Editor::with_context(
            &egui::Context::default(),
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        let current = dir.path().join("current");
        std::fs::create_dir(&current).unwrap();
        e.decode_cache.set_folder(Some(&current));
        e.move_decode_cache(Some(dir.path().join("missing")));
        assert_eq!(e.decode_cache.folder(), Some(current));
        assert!(
            e.messages.line().contains("can't be moved"),
            "{}",
            e.messages.line()
        );
    }

    #[test]
    fn a_cache_size_applies_once_entered_and_is_saved_once_a_drag_ends() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.json");
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.session.set_file(Some(path.clone()));
        e.onboarding.visible = false;
        e.preferences.open = true;
        e.preferences.tab = Tab::Catalog;
        let frame = |e: &mut Editor, events: Vec<egui::Event>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    Vec2::new(1600., 1000.),
                )),
                events,
                ..Default::default()
            };
            let mut out = ctx.run_ui(input, |ui| e.preferences_window(ui.ctx()));
            out.textures_delta.clear();
            out.shapes
        };
        let saved = || {
            std::fs::read(&path).ok().map(|bytes| {
                serde_json::from_slice::<crate::app::preferences::session::Session>(&bytes)
                    .unwrap()
                    .decoded_cache_size_gib()
            })
        };
        let mut field = None;
        for _ in 0..3 {
            field = frame(&mut e, vec![])
                .into_iter()
                .find_map(|s| match s.shape {
                    egui::Shape::Text(t) if t.galley.text().contains("GiB") => {
                        Some(t.visual_bounding_rect().center())
                    }
                    _ => None,
                });
        }
        let pos = field.expect("the cache size field is shown");
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut e, vec![egui::Event::PointerMoved(pos), button(true)]);
        frame(&mut e, vec![button(false)]);
        frame(&mut e, vec![]);
        // Part way through typing 128, neither 1 GiB nor 12 GiB evicts or is saved.
        for text in ["1", "2"] {
            frame(&mut e, vec![egui::Event::Text(text.into())]);
            assert_eq!(e.decode_cache.limit(), 4 << 30);
            assert!(saved().is_none_or(|size| size == 4));
        }
        frame(&mut e, vec![egui::Event::Text("8".into())]);
        frame(
            &mut e,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        frame(&mut e, vec![]);
        assert_eq!(e.decode_cache.limit(), 128 << 30);
        assert_eq!(saved(), Some(128));
        // A drag applies as it goes and is saved when released.
        frame(&mut e, vec![egui::Event::PointerMoved(pos), button(true)]);
        for x in 1..=10 {
            let to = pos + Vec2::new(x as f32 * 5., 0.);
            frame(&mut e, vec![egui::Event::PointerMoved(to)]);
        }
        let dragged = e.decode_cache.limit() >> 30;
        assert!(dragged > 128);
        assert_eq!(saved(), Some(128));
        frame(
            &mut e,
            vec![egui::Event::PointerButton {
                pos: pos + Vec2::new(50., 0.),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(saved(), Some(dragged as u32));
    }

    /// What a page paints past the right edge it is clipped to: text or
    /// controls cut off by the window.
    fn cut_off(tab: Tab) -> Vec<String> {
        let ctx = egui::Context::default();
        let mut e = Editor::with_context(
            &ctx,
            None,
            crate::app::preferences::session::Session::default(),
            None,
        );
        e.onboarding.visible = false;
        e.preferences.open = true;
        e.preferences.tab = tab;
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                Vec2::new(1600., 1000.),
            )),
            ..Default::default()
        };
        let mut shapes = Vec::new();
        for _ in 0..3 {
            let mut out = ctx.run_ui(input(), |ui| e.preferences_window(ui.ctx()));
            out.textures_delta.clear();
            shapes = out.shapes;
        }
        shapes
            .into_iter()
            .filter(|s| s.shape.visual_bounding_rect().right() > s.clip_rect.right() + 0.5)
            .map(|s| match s.shape {
                egui::Shape::Text(text) => text.galley.text().to_string(),
                shape => format!("{:?}", shape.visual_bounding_rect()),
            })
            .collect()
    }

    #[test]
    fn no_preferences_page_cuts_off_its_contents() {
        for tab in Tab::ALL {
            assert_eq!(cut_off(tab), Vec::<String>::new(), "{}", tab.title());
        }
    }
}

/// The models RAWmakase downloads on request that ask for credit wherever they are used,
/// each with the manifest's credit text: listed in About.
pub(in crate::app) fn third_party_models() -> [(&'static str, &'static str); 1] {
    [("AI Denoise model", rawmakase_inference::RAWNIND.attribution)]
}

#[cfg(test)]
mod third_party_tests {
    #[test]
    fn about_credits_ai_denoises_model_with_the_manifests_text() {
        assert!(
            super::third_party_models()
                .iter()
                .any(|(_, credit)| *credit == rawmakase_inference::RAWNIND.attribution)
        );
    }
}

pub(in crate::app) mod monitor;
pub(in crate::app) mod raw_defaults;
pub(crate) mod session;
