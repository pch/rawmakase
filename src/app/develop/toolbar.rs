use crate::app::Editor;
use crate::app::develop::before_after::{Compare, Transfer};
use crate::app::shell::dialogs::FileDialog;
use crate::app::ui::design::{Tone, size, space, text};
use crate::app::ui::icons::Icon;
use crate::app::ui::theme;
use crate::app::ui::widgets::{menu_item, menu_separator, toolbar_action, toolbar_divider};
use crate::model::recipe::Recipe;
use eframe::egui::{self, Stroke, Vec2};

impl Editor {
    pub(in crate::app) fn toolbar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        // The buttons follow the shared log, which also holds Library changes.
        self.sync_undo();
        let (can_undo, can_redo) = (self.undo_log.can_undo(), self.undo_log.can_redo());
        // Under the photo, as the Library's view bar is, on its backdrop with
        // no rule: the canvas's own margin spaces them from the photo.
        egui::Panel::bottom("toolbar")
            .frame(photo_toolbar_frame(&palette))
            .show_separator_line(false)
            .show(ui, |ui| {
                ui.spacing_mut().button_padding = Vec2::new(space::L, space::M);
                ui.spacing_mut().interact_size.y = size::TOOL;
                ui.spacing_mut().item_spacing.x = space::M;
                ui.visuals_mut().button_frame = true;
                ui.visuals_mut().widgets.inactive.bg_fill = palette.tone(Tone::FIELD);
                ui.visuals_mut().widgets.inactive.weak_bg_fill = palette.tone(Tone::FIELD);
                ui.visuals_mut().widgets.inactive.bg_stroke =
                    Stroke::new(1., palette.tone(Tone::OUTLINE));
                ui.visuals_mut().widgets.hovered.bg_fill = palette.tone(Tone::OUTLINE);
                ui.visuals_mut().widgets.hovered.weak_bg_fill = palette.tone(Tone::OUTLINE);
                ui.horizontal(|ui| {
                    if toolbar_action(ui, "", 32., false, can_undo, 1)
                        .on_hover_text("Undo · Ctrl+Z")
                        .clicked()
                    {
                        self.undo();
                    }
                    if toolbar_action(ui, "", 32., false, can_redo, 2)
                        .on_hover_text("Redo · Ctrl+Shift+Z")
                        .clicked()
                    {
                        self.redo();
                    }
                    toolbar_divider(ui);
                    let before_only = self.view.compare == Compare::BeforeOnly;
                    // Short of room, Before is its icon: the zoom slider keeps its
                    // width on the right.
                    let (label, width) = if ui.available_width() >= NARROW_TOOLBAR {
                        ("Before", 70.)
                    } else {
                        ("", 32.)
                    };
                    if toolbar_action(ui, label, width, before_only, true, 3)
                        .on_hover_text("Show Before · Backslash")
                        .clicked()
                    {
                        let view = self.view.compare.toggled(Compare::BeforeOnly);
                        self.set_compare(view);
                    }
                    self.before_after_menu(ui);
                    self.reference_controls(ui);
                    if toolbar_action(ui, "Clipping", 78., self.view.clipping.both_on(), true, 0)
                        .on_hover_text("Show clipped shadows and highlights · J")
                        .clicked()
                    {
                        self.view.clipping.toggle_both();
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // The photo's menu: its settings, presets and export.
                        // The slider never shrinks out of use: room for it comes first
                        // from the menu, which narrows to its icon.
                        let wide_menu = ui.available_width()
                            >= PHOTO_MENU_WIDTH
                                + ZOOM_SLIDER.0
                                + crate::app::view::magnifier::CONTROLS_NARROW_WIDTH
                                + space::L * 2.
                                + space::M * 3.;
                        let settings = photo_menu_button(ui, wide_menu);
                        egui::Popup::menu(&settings).show(|ui| {
                            ui.set_width(270.);
                            ui.spacing_mut().item_spacing.y = 0.;
                            let (cmd, shift) = if cfg!(target_os = "macos") {
                                ("⌘ ", "Shift ")
                            } else {
                                ("Ctrl+", "Shift+")
                            };
                            let copy = format!("{cmd}{shift}C");
                            let paste = format!("{cmd}{shift}V");
                            if menu_item(ui, "Copy Settings…", &copy, true, false) {
                                self.open_copy_dialog(
                                    crate::app::develop::settings_transfer::Transfer::Copy,
                                );
                                ui.close();
                            }
                            if menu_item(
                                ui,
                                "Paste Settings",
                                &paste,
                                self.clipboard.is_some(),
                                false,
                            ) {
                                self.paste_settings();
                                ui.close();
                            }
                            let previous = format!(
                                "{cmd}{}V",
                                if cfg!(target_os = "macos") {
                                    "⌥ "
                                } else {
                                    "Alt+"
                                }
                            );
                            if menu_item(
                                ui,
                                "Paste Settings from Previous",
                                &previous,
                                self.previous_settings.is_some(),
                                false,
                            ) {
                                self.paste_previous();
                                ui.close();
                            }
                            let sync = format!("{cmd}{shift}S");
                            let targets = self.sync_targets().len();
                            if menu_item(
                                ui,
                                "Sync Settings…",
                                &sync,
                                targets > 0 && !self.activity.is_busy(),
                                false,
                            ) {
                                self.open_copy_dialog(
                                    crate::app::develop::settings_transfer::Transfer::Sync,
                                );
                                ui.close();
                            }
                            // Lightroom's Match Total Exposures, for the photos selected
                            // with this one, from their aperture, shutter speed and ISO.
                            let matchable = targets > 0
                                && !self.activity.is_busy()
                                && self
                                    .document
                                    .metadata
                                    .as_ref()
                                    .and_then(crate::app::library::sync::capture_stops)
                                    .is_some();
                            let match_keys = format!(
                                "{cmd}{shift}{}M",
                                if cfg!(target_os = "macos") {
                                    "⌥ "
                                } else {
                                    "Alt+"
                                }
                            );
                            if menu_item(ui, "Match Total Exposures", &match_keys, matchable, false)
                            {
                                self.start_sync(
                                    crate::app::library::sync::BatchChange::MatchTotalExposures,
                                );
                                ui.close();
                            }
                            let reset = format!("{cmd}{shift}R");
                            if menu_item(ui, "Reset All Settings", &reset, true, false) {
                                self.reset_settings();
                                ui.close();
                            }
                            let thumbnail = self.view.compare == Compare::CameraThumbnail;
                            if menu_item(ui, "Camera Thumbnail", "Shift+\\", true, thumbnail) {
                                self.toggle_camera_thumbnail();
                                ui.close();
                            }
                            menu_separator(ui);
                            let new_preset = format!("{cmd}{shift}N");
                            if menu_item(ui, "New Develop Preset…", &new_preset, true, false) {
                                self.open_copy_dialog(
                                    crate::app::develop::settings_transfer::Transfer::NewPreset,
                                );
                                ui.close();
                            }
                            if menu_item(ui, "Save Preset File…", "", true, false) {
                                self.dialog(FileDialog::SavePreset, &ctx);
                                ui.close();
                            }
                            if menu_item(ui, "Load Preset File…", "", true, false) {
                                self.dialog(FileDialog::LoadPreset, &ctx);
                                ui.close();
                            }
                            menu_separator(ui);
                            // The photo open here, its file gone from where
                            // the catalog has it.
                            let offline = self.document.catalog_photo.filter(|_| {
                                self.document.path.as_ref().is_some_and(|p| !p.exists())
                            });
                            if menu_item(ui, "Find on Disk…", "", offline.is_some(), false)
                                && let Some(photo) = offline
                            {
                                self.choose_file_on_disk(photo, &ctx);
                                ui.close();
                            }
                            menu_separator(ui);
                            let (export, previous) = if cfg!(target_os = "macos") {
                                ("⇧⌘E", "⌥⇧⌘E")
                            } else {
                                ("Ctrl+Shift+E", "Ctrl+Alt+Shift+E")
                            };
                            let photo = self.document.full().is_some();
                            if menu_item(ui, "Export…", export, photo, false) {
                                self.open_export_dialog();
                                ui.close();
                            }
                            if menu_item(ui, "Export with Previous", previous, photo, false) {
                                self.export_with_previous();
                                ui.close();
                            }
                            menu_separator(ui);
                            let prefs = if cfg!(target_os = "macos") {
                                "⌘ ,"
                            } else {
                                "Ctrl+,"
                            };
                            // Profiles, display and engine choices apply to every photo.
                            if menu_item(ui, "Preferences…", prefs, true, false) {
                                self.open_preferences(crate::app::preferences::Tab::General);
                                ui.close();
                            }
                        });
                        toolbar_divider(ui);
                        self.zoom_controls(ui, 32.);
                        if self.document.loading() || self.preview.task.is_running() {
                            ui.add_space(space::L);
                            crate::app::ui::widgets::spinner(ui, 14.);
                        }
                    });
                });
            });
    }
    /// Viewing, at the right of a photo's toolbar (Develop's, the Library Loupe's),
    /// laid out right to left: the zoom slider, always there as neither has a
    /// Navigator, and the magnifier. When room runs short the slider narrows to its
    /// least width, then the magnifier's menu shows a chevron without the level.
    /// `height` is the toolbar's buttons'.
    pub(in crate::app) fn zoom_controls(&mut self, ui: &mut egui::Ui, height: f32) {
        let room = ui.available_width() - space::L * 2. - space::M * 2.;
        let full = crate::app::view::magnifier::CONTROLS_WIDTH;
        let (magnifier, width) = if room >= ZOOM_SLIDER.0 + full {
            (full, (room - full).min(ZOOM_SLIDER.1))
        } else {
            (
                crate::app::view::magnifier::CONTROLS_NARROW_WIDTH,
                ZOOM_SLIDER.0,
            )
        };
        let fit = self.fit_level();
        let slider = ui.allocate_ui(Vec2::new(width, height), |ui| {
            crate::app::view::navigator::zoom_slider(ui, self.view.zoom, fit, width, height)
        });
        if let Some(level) = slider.inner {
            self.set_zoom(level);
            // The photo follows the knob at once: an eased zoom would trail behind
            // every move of it.
            self.view.zoom_key = (self.view.zoom.on, self.view.zoom.level);
            self.view.zoom_anim = None;
        }
        self.magnifier_controls(
            ui,
            magnifier == crate::app::view::magnifier::CONTROLS_WIDTH,
            height,
        );
    }
    /// Reference View's button, and while it shows, its layout and lock, as in
    /// Lightroom's toolbar.
    fn reference_controls(&mut self, ui: &mut egui::Ui) {
        use crate::app::develop::before_after::Axis;
        let shown = self.reference_view();
        if toolbar_action(ui, "Reference", 84., shown, true, 0)
            .on_hover_text("Reference View · Shift+R")
            .clicked()
        {
            self.toggle_reference_view();
        }
        if !shown {
            return;
        }
        for (layout, label, width) in [
            (Axis::LeftRight, "Left / Right", 92.),
            (Axis::TopBottom, "Top / Bottom", 98.),
        ] {
            let chosen = self.view.compare == Compare::Reference(layout);
            if toolbar_action(ui, label, width, chosen, true, 0).clicked() {
                self.set_reference_layout(layout);
            }
        }
        let locked = self.reference.locked;
        if toolbar_action(ui, "Lock", 54., locked, self.reference.photo.is_some(), 0)
            .on_hover_text(if locked {
                "The reference photo stays when you leave Develop"
            } else {
                "Keep the reference photo when you leave Develop"
            })
            .clicked()
        {
            self.reference.locked = !locked;
        }
    }
    /// Lightroom's Before/After menu: the views, with their keys, and copying or
    /// swapping settings between the two sides.
    fn before_after_menu(&mut self, ui: &mut egui::Ui) {
        let two_up = self.view.compare.two_up() && self.view.compare.shows_before();
        let button = toolbar_action(ui, "Before / After", 104., two_up, true, 0)
            .on_hover_text("Before/After views · Y, Option+Y, Shift+Y");
        let mac = cfg!(target_os = "macos");
        let alt = if mac { "⌥ " } else { "Alt+" };
        let transfer = if mac { "⌥⇧⌘ " } else { "Ctrl+Alt+Shift+" };
        egui::Popup::menu(&button).show(|ui| {
            ui.set_width(300.);
            ui.spacing_mut().item_spacing.y = 0.;
            for (view, name) in crate::app::develop::before_after::VIEWS {
                let keys = match view {
                    Compare::BeforeOnly => "\\".to_string(),
                    Compare::SideBySide(crate::app::develop::before_after::Axis::LeftRight) => {
                        "Y".into()
                    }
                    Compare::SideBySide(_) => format!("{alt}Y"),
                    Compare::Split(crate::app::develop::before_after::Axis::LeftRight) => {
                        "Shift+Y".into()
                    }
                    _ => String::new(),
                };
                if menu_item(ui, name, &keys, true, self.view.compare == view) {
                    let view = self.view.compare.toggled(view);
                    self.set_compare(view);
                    ui.close();
                }
            }
            menu_separator(ui);
            for (transfer_kind, name, key) in [
                (
                    Transfer::BeforeToAfter,
                    "Copy Before's Settings to After",
                    "→",
                ),
                (
                    Transfer::AfterToBefore,
                    "Copy After's Settings to Before",
                    "←",
                ),
                (Transfer::Swap, "Swap Before and After Settings", "↑"),
            ] {
                let keys = format!("{transfer}{key}");
                if menu_item(ui, name, &keys, self.document.full().is_some(), false) {
                    self.transfer(transfer_kind);
                    ui.close();
                }
            }
        });
    }
    /// Back to the raw defaults, like Lightroom's Reset.
    pub(in crate::app) fn reset_settings(&mut self) {
        self.document
            .edit
            .name_next_step(crate::app::history::Step::new("Reset Settings", ""));
        let Some(defaults) = self.photo_defaults() else {
            self.document.edit.replace(Recipe::default());
            return;
        };
        if let Some(note) = defaults.note {
            self.messages.note(note);
        }
        self.document.edit.replace(defaults.recipe);
    }
}

/// The toolbar's Photo button, a toolbar button like the rest with a chevron
/// for its menu.
/// The frame of a photo view's toolbar, Develop's and the Library Loupe's: on the
/// photo's backdrop with no rule, the canvas's own margin spacing it from the
/// photo.
pub(in crate::app) fn photo_toolbar_frame(palette: &theme::Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(palette.photo_backdrop())
        .inner_margin(egui::Margin {
            left: crate::app::shell::workspace::PHOTO_MARGIN as i8,
            right: crate::app::shell::workspace::PHOTO_MARGIN as i8,
            top: 0,
            bottom: space::L as i8,
        })
}

/// Narrower than this, the toolbar's Before button shows its icon alone.
const NARROW_TOOLBAR: f32 = 1000.;
/// The zoom slider's least and greatest widths in the toolbar.
const ZOOM_SLIDER: (f32, f32) = (176., 220.);
const PHOTO_MENU_WIDTH: f32 = 82.;

/// The photo's menu button: "Photo" and a chevron, or `…` alone when not `wide`.
fn photo_menu_button(ui: &mut egui::Ui, wide: bool) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let width = if wide { PHOTO_MENU_WIDTH } else { 32. };
    let response = toolbar_action(ui, "", width, false, true, 0)
        .on_hover_text("Photo: copy, paste and sync settings, presets and export");
    let color = palette.tone(if response.hovered() {
        Tone::STRONG
    } else {
        Tone::LABEL
    });
    let rect = response.rect;
    if !wide {
        crate::app::ui::icons::paint_at(ui.painter(), Icon::More, rect.center(), 15., color);
        return response;
    }
    ui.painter().text(
        rect.left_center() + Vec2::new(14., 0.),
        egui::Align2::LEFT_CENTER,
        "Photo",
        egui::FontId::proportional(text::LABEL),
        color,
    );
    crate::app::ui::icons::paint_at(
        ui.painter(),
        Icon::ChevronDown,
        rect.right_center() - Vec2::new(16., 0.),
        11.,
        color,
    );
    response
}
