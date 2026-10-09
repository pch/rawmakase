use super::Editor;
use super::dialogs::{CatalogDialog, FolderAction};
use super::library::PreviewsRequest;
use super::panels::WorkspacePanel;
use super::state::Tool;
use super::widgets::{TOP_BAR_SEGMENTS, segment_bar};
use super::workflow::Flushed;
use crate::app::Module;
use crate::app::theme;
use crate::catalog::PhotoId;
use eframe::egui::{self, Color32, Vec2};
use std::time::{Duration, Instant};

impl Editor {
    pub(super) fn metadata_shortcuts(&mut self, ctx: &egui::Context) {
        if self.activity.is_busy() {
            return;
        }
        // With a brush tool open, [ and ] size the brush instead of rating the photo.
        let brushing = self.module == Module::Develop && self.tool_has_size();
        // With the Crop tool open, X swaps the crop's orientation instead of rejecting.
        let cropping = self.module == Module::Develop && self.view.is(Tool::Crop);
        let auto_advance = self.auto_advance;
        let shortcut = crate::app::photo_metadata::shortcut(ctx)
            .filter(|(e, _)| {
                !(brushing && matches!(e, crate::app::photo_metadata::Edit::RatingDelta(_)))
                    && !(cropping && matches!(e, crate::app::photo_metadata::Edit::Flag(-1)))
            })
            // Photo > Auto Advance: every key moves on, as Shift does.
            .map(|(edit, shift)| (edit, shift || auto_advance));
        // A spot resize still being grouped is logged before the rating it precedes.
        if shortcut.is_some() {
            self.finish_wheel_gesture();
        }
        if let Some((edit, advance)) = shortcut {
            if self.library.is_some()
                && let Err(error) = self.command_metadata(edit, None, advance)
            {
                self.status = format!("Metadata could not be saved: {}", error.message);
            }
        } else if self.module == Module::Library
            && let Some(library) = &mut self.library
        {
            library.selection_keys(ctx);
        }
    }
    pub(super) fn draw(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.control_commands(&ctx);
        self.events(&ctx);
        self.poll_updates(&ctx);
        self.themes.poll(&ctx);
        if let Some(library) = &mut self.library {
            library.publish_shown();
            library.poll_previews(&ctx);
        }
        self.poll_preview_builds();
        // Preferences is modal: keys go to it, not to the photo behind.
        let modal = self.preferences.open
            || self.export_modal()
            || self.modal.is_some()
            || self.not_editable.is_some()
            || self.view.shortcuts
            || self.curve_save_open();
        if !modal {
            self.metadata_shortcuts(&ctx);
            self.workspace_shortcuts(&ctx);
            self.preferences_shortcut(&ctx);
            if !self.onboarding.visible && !self.activity.is_busy() {
                self.panel_keys(&ctx);
            }
        }
        self.workspace_bar(ui);
        // A file dialog or a running Sync: nothing behind them takes clicks, so no
        // catalog action is chosen only to be dropped.
        if self.activity.is_busy() {
            ui.disable();
        }
        if self.onboarding.visible {
            self.onboarding_ui(ui);
        } else if self.module == Module::Library {
            self.left_develop();
            self.library_workspace(ui);
        } else {
            let mut frame = self.begin_edit_frame();
            if !modal {
                self.develop_shortcuts(&ctx);
            }
            // The Navigator column runs full height; the toolbar sits over
            // the photo and the adjustments only. Each part may open another
            // photo; what follows edits that one.
            self.follow_edit_frame(&mut frame);
            if self.panel_shown(WorkspacePanel::Filmstrip) {
                self.status_bar(ui);
                self.filmstrip(ui);
                self.follow_edit_frame(&mut frame);
            }
            if self.panel_shown(WorkspacePanel::Left) {
                self.develop_left_panel(ui);
                self.follow_edit_frame(&mut frame);
            }
            self.toolbar(ui);
            self.follow_edit_frame(&mut frame);
            self.develop_panels(ui);
            self.finish_edit_frame(frame, &ctx);
        }
        if let Some(request) = self.library.as_mut().and_then(|l| l.take_copy_request()) {
            self.virtual_copy(request);
        }
        if let Some(request) = self
            .library
            .as_mut()
            .and_then(|l| l.take_previews_request())
        {
            match request {
                PreviewsRequest::Build(ids, kind) => self.build_previews_from_menu(&ids, kind),
                PreviewsRequest::Discard(ids, kinds) => self.discard_previews(&ids, &kinds),
            }
        }
        if let Some(removal) = self.library.as_mut().and_then(|l| l.take_removal_request()) {
            self.modal = Some(super::Modal::RemoveFolder(removal));
        }
        if let Some(ids) = self.library.as_mut().and_then(|l| l.take_read_request()) {
            self.modal = Some(super::Modal::ReadMetadata(ids));
        }
        if let Some(library) = &mut self.library
            && library.take_reread_finished()
        {
            self.status = library.message.clone();
            // Reading metadata can bring in the reference photo's Lightroom edit.
            self.load_reference();
        }
        self.remove_copy_window(&ctx);
        self.remove_folder_window(&ctx);
        self.read_metadata_window(&ctx);
        self.not_editable_window(&ctx);
        self.shortcuts_window(&ctx);
        self.copy_dialog_window(&ctx);
        self.preset_rename_window(&ctx);
        self.curve_save_window(&ctx);
        self.preferences_window(&ctx);
        // Above Preferences, where Change… asks it.
        self.folder_question_window(&ctx);
        self.export_windows(&ctx);
        self.update_notice(&ctx, modal || self.view.shortcuts);
        self.pending_work(&ctx);
        super::panels::keep_resize_cursor(&ctx);
        let collapsed = ctx.data(|d| {
            d.get_temp::<std::collections::BTreeSet<String>>(
                super::widgets::collapsed_sections_id(),
            )
        });
        if let Some(collapsed) = collapsed
            && collapsed != self.collapsed
        {
            self.collapsed = collapsed;
            let _ = self.save_session();
        }
        let solo = ctx.data(|d| {
            d.get_temp::<std::collections::BTreeSet<String>>(super::widgets::solo_sections_id())
        });
        if let Some(solo) = solo
            && solo != self.solo
        {
            self.solo = solo;
            let _ = self.save_session();
        }
        self.sync_undo();
        // The layout is kept once a drag (the thumbnail size) or typing (the search)
        // ends, or when the window closes.
        let closing = ctx.input(|i| i.viewport().close_requested());
        let busy = !closing && (ctx.input(|i| i.pointer.any_down()) || ctx.text_edit_focused());
        self.remember_place(if busy {
            LayoutEdit::Changing
        } else {
            LayoutEdit::Settled
        });
    }
    /// Saves the session when the place in the catalog, or a settled layout, changed.
    pub(super) fn remember_place(&mut self, layout: LayoutEdit) {
        let place = self.current_place();
        let shown = self.library.as_ref().map(|l| l.layout());
        let layout_changed = layout == LayoutEdit::Settled
            && shown.as_ref().is_some_and(|l| *l != self.saved_layout);
        if self.library.is_some() && (place != self.saved_place || layout_changed) {
            self.saved_place = place;
            if let Some(shown) = shown {
                self.saved_layout = shown;
            }
            let _ = self.save_session();
        }
    }
    fn workspace_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.activity.is_busy() && !ctx.text_edit_focused() {
            // Cmd+G and Cmd+D are other commands (Stack, Select None).
            let plain = |key| {
                // The modifiers held for that key, not at the end of the frame.
                ctx.input(|i| {
                    i.events.iter().any(|e| {
                        matches!(e, egui::Event::Key { key: k, pressed: true, modifiers, .. }
                            if *k == key && !(modifiers.command || modifiers.ctrl || modifiers.alt))
                    })
                })
            };
            // The Loupe zooms with Develop's keys, whatever the photo; a menu
            // or popup takes the keys first.
            if self.module == Module::Library
                && !egui::Popup::is_any_open(ctx)
                && self.library.as_ref().is_some_and(|l| l.loupe_open())
            {
                self.zoom_keys(ctx);
            }
            // Develop has its own keys; the log is the same.
            if self.module == Module::Library {
                // Consumed, with the modifiers held for the key, so an undo
                // that opens Develop is not run again by Develop's keys.
                use egui::{Key, Modifiers};
                let (undo, redo) = ctx.input_mut(|i| {
                    let undo = i.consume_key(Modifiers::COMMAND, Key::Z);
                    let redo = i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::Z)
                        || (!cfg!(target_os = "macos")
                            && i.consume_key(Modifiers::COMMAND, Key::Y));
                    (undo, redo)
                });
                if undo {
                    self.undo();
                } else if redo {
                    self.redo();
                }
            }
            if plain(egui::Key::G) && self.flush() {
                self.module = Module::Library;
                if let Some(library) = &mut self.library {
                    library.show_grid();
                }
            }
            // E from Develop: the photo in the Library's Loupe.
            if self.module == Module::Develop
                && plain(egui::Key::E)
                && self.flush()
                && let (Some(library), Some(id)) = (&mut self.library, self.document.catalog_photo)
            {
                self.module = Module::Library;
                library.reveal(id);
                library.open_loupe();
            }
            // Lightroom's Create Virtual Copy, in Library and Develop.
            // Only the first key-down: a held key must not make copy after copy.
            let create = ctx.input(|i| {
                i.modifiers.command
                    && i.events.iter().any(|e| {
                        matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::Quote,
                                pressed: true,
                                repeat: false,
                                ..
                            }
                        )
                    })
            });
            if create {
                let id = if self.module == Module::Library {
                    self.library.as_ref().and_then(|l| l.selected())
                } else {
                    self.document.catalog_photo
                };
                if let Some(id) = id {
                    self.virtual_copy(crate::app::library::CopyAction::Create(id));
                }
            }
            if plain(egui::Key::D) {
                if self.module == Module::Library {
                    if let Some(id) = self.library.as_mut().and_then(|l| l.selected_or_first()) {
                        self.develop_catalog_photo(id);
                    }
                } else {
                    self.module = Module::Develop;
                }
            }
        }
    }

    /// The catalog menu under the catalog's name: where it is, adding photos,
    /// other catalogs, Lightroom, and its settings.
    fn catalog_menu(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        use super::icons::Icon;
        let palette = theme::palette(ctx);
        ui.set_width(268.);
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let location = self
            .library
            .as_ref()
            .map(|l| l.session.catalog.location().clone());
        // Which catalog this is and how many photos it holds; where it lives
        // is in its hover.
        ui.add_space(4.);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(12, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let Some(library) = &self.library else {
                    ui.label(
                        egui::RichText::new("No catalog open")
                            .size(13.)
                            .color(palette.gray(200)),
                    );
                    return;
                };
                let location = library.session.catalog.location();
                let crate::catalog::CatalogLocation::File(path) = location;
                ui.label(
                    egui::RichText::new(location.name())
                        .size(13.)
                        .color(palette.gray(235)),
                )
                .on_hover_text(super::widgets::pretty_path(path));
                ui.add_space(2.);
                let photos = match library.session.photos.len() {
                    1 => "1 photo".to_string(),
                    n => format!("{n} photos"),
                };
                ui.label(
                    egui::RichText::new(photos)
                        .size(11.5)
                        .color(palette.gray(130)),
                );
            });
        menu_separator(ui);
        if menu_row(
            ui,
            Icon::FolderPlus,
            "Add Photo Folder…",
            location.is_some(),
        )
        .clicked()
        {
            self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), ctx);
            ui.close();
        }
        menu_separator(ui);
        if menu_row(ui, Icon::Folder, "Open Catalog…", true).clicked() {
            self.catalog_dialog(CatalogDialog::Open, ctx);
            ui.close();
        }
        if menu_row(ui, Icon::Add, "New Catalog…", true).clicked() {
            self.catalog_dialog(CatalogDialog::Create, ctx);
            ui.close();
        }
        menu_separator(ui);
        if menu_row(ui, Icon::Collection, "Import from Lightroom", true)
            .on_hover_text("Its catalog, camera and lens profiles and presets")
            .clicked()
        {
            self.open_onboarding();
            ui.close();
        }
        ui.add_space(4.);
    }
    /// Lightroom's top panel: catalog menu on the left, module picker on the
    /// right. On macOS it is also the title bar, beside the traffic lights.
    fn workspace_bar(&mut self, ui: &mut egui::Ui) {
        let palette = theme::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        egui::Panel::top("workspace-modes")
            .exact_size(BAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(palette.gray(26))
                    .inner_margin(egui::Margin::symmetric(18, 0)),
            )
            .show(ui, |ui| {
                title_bar_drag(ui);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    ui.add_space(fastframe_macos::traffic_light_inset(ui.ctx()));
                    // The panel toggles: none in the setup view, which has no panels.
                    let toggles = !self.onboarding.visible;
                    if toggles {
                        ui.add_enabled_ui(!self.activity.is_busy(), |ui| {
                            self.panel_toggle(ui, WorkspacePanel::Left);
                        });
                        ui.add_space(8.);
                    }
                    let catalog = self
                        .library
                        .as_ref()
                        .map(|library| library.session.catalog.location().name())
                        .unwrap_or_else(|| "No catalog".into());
                    // Every element is painted in a 28 px slot so all centers line up.
                    let name = ui.painter().layout_no_wrap(
                        catalog,
                        egui::FontId::proportional(13.),
                        palette.gray(255),
                    );
                    let busy = self.activity.is_busy();
                    let (rect, response) = ui.allocate_exact_size(
                        Vec2::new(name.size().x.min(320.) + 36., 28.),
                        if busy {
                            egui::Sense::hover()
                        } else {
                            egui::Sense::click()
                        },
                    );
                    let open =
                        egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&response));
                    if response.hovered() || open {
                        ui.painter().rect_filled(rect, 4., palette.gray(38));
                    }
                    let color = palette.gray(if response.hovered() || open { 235 } else { 175 });
                    ui.painter()
                        .with_clip_rect(rect.shrink2(Vec2::new(10., 0.)))
                        .galley(
                            rect.left_center() + Vec2::new(10., -name.size().y / 2.),
                            name,
                            color,
                        );
                    super::icons::paint_at(
                        ui.painter(),
                        super::icons::Icon::ChevronDown,
                        rect.right_center() - Vec2::new(14., 0.),
                        11.,
                        color,
                    );
                    let response = response
                        .on_hover_text("Catalog: open, create or import")
                        .on_hover_cursor(egui::CursorIcon::PointingHand);
                    egui::Popup::menu(&response).show(|ui| self.catalog_menu(ui, &ctx));
                    ui.add_space(8.);
                    if self.activity.is_busy() {
                        ui.spinner();
                    }
                    self.export_progress(ui);
                    self.preview_build_progress(ui);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 0.;
                        ui.add_enabled_ui(!self.activity.is_busy(), |ui| {
                            if toggles {
                                self.panel_toggle(ui, WorkspacePanel::Right);
                                ui.add_space(2.);
                                self.panel_toggle(ui, WorkspacePanel::Filmstrip);
                                ui.add_space(12.);
                            }
                            let setup = self.onboarding.visible;
                            // The setup assistant shows neither module as active.
                            let selected = (!setup).then_some(if self.module == Module::Library {
                                0
                            } else {
                                1
                            });
                            let [library, develop] = segment_bar(
                                ui,
                                ["Library", "Develop"],
                                selected,
                                &TOP_BAR_SEGMENTS,
                            );
                            if develop.on_hover_text("Develop · D").clicked() {
                                self.onboarding.visible = false;
                                if self.module == Module::Library
                                    && let Some(id) = self
                                        .library
                                        .as_mut()
                                        .and_then(|library| library.selected_or_first())
                                {
                                    self.develop_catalog_photo(id);
                                } else {
                                    self.module = Module::Develop;
                                }
                            }
                            if library.on_hover_text("Library · G").clicked() && self.flush() {
                                self.onboarding.visible = false;
                                self.module = Module::Library;
                            }
                            ui.add_space(12.);
                            let hover = format!(
                                "Keyboard shortcuts · {}",
                                super::shortcuts::keys_text("Cmd+/")
                            );
                            if super::shortcuts::icon_button(
                                ui,
                                super::icons::Icon::Keyboard,
                                &hover,
                            )
                            .clicked()
                            {
                                self.view.shortcuts = !self.view.shortcuts;
                            }
                            ui.add_space(8.);
                            let shortcut = if cfg!(target_os = "macos") {
                                "⌘,"
                            } else {
                                "Ctrl+,"
                            };
                            if super::preferences::gear_button(ui)
                                .on_hover_text(format!("Preferences · {shortcut}"))
                                .clicked()
                            {
                                self.open_preferences(super::preferences::Tab::General);
                            }
                        });
                    });
                });
            });
        egui::Panel::top("workspace-modes-rule")
            .exact_size(1.)
            .frame(egui::Frame::new().fill(palette.gray(16)))
            .show(ui, |_| {});
    }

    /// A RAW in the Library's Loupe: loaded as the document, as Develop
    /// does, and drawn by Develop's viewport without its tools, so it zooms
    /// the same way and D shows it in Develop at once.
    fn loupe_viewport(&mut self, ui: &mut egui::Ui, id: PhotoId) {
        // Loaded once each time the Loupe shows the photo: one that failed to
        // open, or whose predecessor failed to save, is tried again the next
        // time, not every frame.
        let failed = self.document.catalog_photo == Some(id)
            && self.document.full().is_none()
            && !self.load.is_running();
        if (self.document.catalog_photo != Some(id) || failed) && self.loupe_tried != Some(id) {
            self.loupe_tried = Some(id);
            let Some(path) = self
                .library
                .as_ref()
                .and_then(|l| l.photo(id))
                .map(|p| p.path.clone())
            else {
                return;
            };
            // Moving on keeps the zoom, so the next photo is compared as it was.
            let zoom = (self.view.zoom.on, self.view.zoom.level, self.view.zoom.pan);
            if !self.load_raw(path, Some(id)) {
                return;
            }
            (self.view.zoom.on, self.view.zoom.level, self.view.zoom.pan) = zoom;
        }
        // Still the previous photo, e.g. it could not be saved: never show it
        // under this one's name.
        if self.document.catalog_photo != Some(id) {
            ui.centered_and_justified(|ui| {
                ui.label(
                    egui::RichText::new(&self.status).color(theme::palette(ui.ctx()).gray(150)),
                );
            });
            return;
        }
        // Develop's tools, Before view, clipping warning and preset preview
        // stay in Develop.
        if self.view.tool != Tool::None
            || self.view.compare.shows_before()
            || self.view.clipping != Default::default()
            || self.presets.preview.is_some()
        {
            self.view.tool = Tool::None;
            self.view.compare = Default::default();
            self.view.clipping.clear();
            self.presets.preview = None;
            self.schedule();
        }
        let area = ui.available_rect_before_wrap();
        let before = self.view.zoom.on;
        self.viewport_ui(ui);
        let Some(library) = &mut self.library else {
            return;
        };
        library.loupe_overlay(ui.painter(), area);
        if self.view.zoom.on != before {
            library.loupe_zoom_toggled(before);
        }
        // As in Lightroom, a double-click goes back to the grid; its first
        // click's zoom is undone.
        // egui counts a click soon after a double-click as a triple one.
        let double = ui.input(|i| {
            let button = egui::PointerButton::Primary;
            (i.pointer.button_double_clicked(button) || i.pointer.button_triple_clicked(button))
                && i.pointer.interact_pos().is_some_and(|p| area.contains(p))
        });
        if double && let Some(on) = library.loupe_double_click() {
            self.view.zoom.on = on;
        }
    }
    fn library_workspace(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // Export… and Export with Previous work in the Library too, on its selection.
        // Behind any dialog they don't; the chord is read from the key press itself.
        if !self.command_modal() && !self.activity.is_busy() && !ctx.text_edit_focused() {
            let export = ctx.input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        repeat: false,
                        modifiers,
                    } if (*key == egui::Key::E || *physical_key == Some(egui::Key::E))
                        && modifiers.command
                        && modifiers.shift =>
                    {
                        Some(modifiers.alt)
                    }
                    _ => None,
                })
            });
            match export {
                Some(true) => self.export_with_previous(),
                Some(false) => self.open_export_dialog(),
                None => {}
            }
        }
        let filmstrip = self.panel_shown(WorkspacePanel::Filmstrip);
        if filmstrip {
            self.library_status_bar(ui);
        }
        let mut action = crate::app::library::Action::None;
        // The filmstrip runs the window's width, under both side panels, as
        // in Develop. Hidden, the views are still brought up to date.
        if let Some(library) = &mut self.library {
            action = if filmstrip {
                library.library_filmstrip(ui)
            } else {
                library.prepare(ui.ctx());
                crate::app::library::Action::None
            };
        }
        let develops = self.library.as_ref().and_then(|l| l.loupe_develops());
        if develops != self.loupe_tried {
            self.loupe_tried = None;
        }
        self.loupe_stored_preview();
        if self.panel_shown(WorkspacePanel::Left) {
            action = action.then(self.library_left_panel(ui, develops));
        }
        if self.panel_shown(WorkspacePanel::Right) {
            action = action.then(self.library_right_panel(ui));
        }
        let area = egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| match &mut self.library {
                Some(l) if !l.session.photos.is_empty() => {
                    action = action.then(l.grid(ui, &mut self.view.zoom));
                    if let Some(id) = l.loupe_develops() {
                        self.loupe_viewport(ui, id);
                    }
                }
                _ => self.library_start(ui),
            });
        let area = area.response.rect;
        self.drop_area = Some(area);
        // A click in the view, drawn after the strip, shows there next frame.
        // A hidden strip has nothing to catch up on.
        if filmstrip && self.library.as_ref().is_some_and(|l| l.filmstrip_behind()) {
            crate::app::library::filmstrip::redraw(
                &ctx,
                "the view changed what the filmstrip shows",
            );
        }
        if !self.activity.is_busy() {
            match action {
                crate::app::library::Action::Develop(id) => self.develop_catalog_photo(id),
                crate::app::library::Action::RelinkRoot(id) => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::RelinkRoot(id)), &ctx)
                }
                crate::app::library::Action::RelinkFolder(id) => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::RelinkFolder(id)), &ctx)
                }
                crate::app::library::Action::AddFolder => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), &ctx)
                }
                crate::app::library::Action::None => {}
            }
        }
    }
    /// The Library's left panel: Export…, the Loupe's Navigator, folders and
    /// collections.
    fn library_left_panel(
        &mut self,
        ui: &mut egui::Ui,
        develops: Option<PhotoId>,
    ) -> crate::app::library::Action {
        let mut action = crate::app::library::Action::None;
        egui::Panel::left("library-sidebar")
            .default_size(260.)
            .min_size(LIBRARY_SIDEBAR_MIN)
            .max_size(500.)
            .show(ui, |ui| {
                let _side = super::widgets::SectionSide::enter(ui, super::widgets::SectionGroup::LibraryLeft);
                // Lightroom's Export… at the foot of the Library's left panel, for the
                // photos selected.
                let mut export = false;
                if self.library.is_some() {
                    egui::Panel::bottom("library-export")
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(8, 8)))
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                export = super::widgets::action_button(
                                    ui,
                                    "Export…",
                                    Some(super::icons::Icon::Export),
                                    super::widgets::ButtonKind::Primary,
                                    true,
                                )
                                .on_hover_text(if cfg!(target_os = "macos") {
                                    "Export the selected photos · ⇧⌘E"
                                } else {
                                    "Export the selected photos · Ctrl+Shift+E"
                                })
                                .clicked();
                            });
                        });
                }
                // The Navigator controls the zoom: Develop's for a RAW in the
                // Loupe, the same one for other photos. Outside the Loupe it
                // shows the selected photo, and a zoom chosen there opens it.
                let loupe = self.library.as_ref().is_some_and(|l| l.loupe_open());
                if develops.is_some() {
                    self.navigator_ui(ui);
                } else {
                    let (photo, shown) = match &self.library {
                        Some(l) if loupe => l
                            .loupe_navigator()
                            .map_or((None, None), |(photo, shown)| (Some(photo), shown)),
                        Some(l) => (l.selected_preview(), None),
                        None => (None, None),
                    };
                    if let Some(change) =
                        crate::app::navigator::navigator(ui, photo, Some(self.view.zoom), shown)
                    {
                        self.library_zoom(change);
                    }
                }
                if export {
                    self.open_export_dialog();
                }
                if let Some(library) = &mut self.library {
                    action = action.then(library.sidebar(ui));
                } else {
                    ui.heading("Library");
                    ui.label("Create an RAWmakase catalog or import a Lightroom catalog from the Catalog menu.");
                }
            });
        action
    }
    /// A zoom chosen in the Library's Navigator: applied to the Loupe, which
    /// opens on the selected photo (or the first shown) if it is not open.
    pub(super) fn library_zoom(&mut self, change: crate::app::navigator::Change) {
        match change {
            crate::app::navigator::Change::Level(level) => self.view.zoom.set(level),
            crate::app::navigator::Change::Inspect(at) => {
                self.view.zoom.pan = at;
                self.view.zoom.on = true;
            }
        }
        if let Some(library) = &mut self.library
            && !library.loupe_open()
        {
            library.open_loupe();
        }
    }
    /// The Library's right panel: the active photo's info and metadata.
    fn library_right_panel(&mut self, ui: &mut egui::Ui) -> crate::app::library::Action {
        let mut action = crate::app::library::Action::None;
        egui::Panel::right("library-info")
            .default_size(270.)
            .min_size(LIBRARY_INFO_MIN)
            .max_size(420.)
            .show(ui, |ui| {
                let _side = super::widgets::SectionSide::enter(
                    ui,
                    super::widgets::SectionGroup::LibraryRight,
                );
                if let Some(library) = &mut self.library {
                    action = action.then(library.info_panel(ui));
                }
            });
        action
    }
    fn library_status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("library-status").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(work) = &self.catalog_work {
                    ui.add(egui::Spinner::new().size(11.));
                    ui.small(work);
                } else {
                    let shown = self
                        .library
                        .as_ref()
                        .filter(|l| !l.message.is_empty())
                        .map_or(self.status.as_str(), |l| l.message.as_str());
                    status_text(ui, shown, self.message_detail(shown), None);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let toggle = ui
                        .checkbox(
                            &mut self.auto_advance,
                            egui::RichText::new("Auto Advance").small(),
                        )
                        .on_hover_text(
                            "Photo > Auto Advance: a rating, flag or label moves on to the next photo",
                        );
                    if toggle.changed() {
                        let _ = self.save_session();
                    }
                    if let Some(library) = &self.library
                        && library.preview_progress_active()
                    {
                        library.preview_progress(ui);
                    }
                });
            });
        });
    }

    /// Develop's zoom keys, shared with the Library's Loupe: Cmd+= and Cmd+-
    /// step through the zoom levels, Z toggles Fit and the last zoom, F fits.
    pub(super) fn zoom_keys(&mut self, ctx: &egui::Context) {
        use egui::Key;
        // Each key with the modifiers held for it, which a quick shortcut
        // can release in the same frame.
        let presses: Vec<(Key, Option<Key>, egui::Modifiers, bool)> = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Key {
                        key,
                        physical_key,
                        pressed: true,
                        repeat,
                        modifiers,
                    } => Some((*key, *physical_key, *modifiers, *repeat)),
                    _ => None,
                })
                .collect()
        });
        for (key, physical, modifiers, repeat) in presses {
            match key {
                // Cmd+Option+0: 1:1. Option changes the typed key on macOS.
                _ if physical == Some(Key::Num0) && modifiers.command && modifiers.alt => {
                    self.set_zoom(1.)
                }
                Key::Plus | Key::Equals if modifiers.command => self.step_zoom(1),
                Key::Minus if modifiers.command => self.step_zoom(-1),
                // Once per press: a held Z must not flicker the zoom.
                Key::Z if !modifiers.any() && !repeat => self.view.zoom.on = !self.view.zoom.on,
                Key::F if !modifiers.any() => self.view.zoom.on = false,
                _ => {}
            }
        }
    }
    pub(super) fn develop_shortcuts(&mut self, ctx: &egui::Context) {
        if !self.activity.is_busy() && !ctx.text_edit_focused() {
            self.zoom_keys(ctx);
            let (mut copy, mut paste, mut reset) = (false, false, false);
            let mut previous = false;
            let mut sync = false;
            let mut match_exposures = false;
            let mut new_preset = false;
            let mut auto = false;
            let mut treatment = false;
            let mut export = None;
            let mut transfer = None;
            let mut targeted = None;
            ctx.input(|i| {
                // Lightroom's Copy After's Settings to Before (←), Copy Before's to
                // After (→) and Swap (↑), with Cmd+Option+Shift.
                let m = i.modifiers;
                if m.command && m.alt && m.shift {
                    use super::before_after::Transfer;
                    transfer = [
                        (egui::Key::ArrowLeft, Transfer::AfterToBefore),
                        (egui::Key::ArrowRight, Transfer::BeforeToAfter),
                        (egui::Key::ArrowUp, Transfer::Swap),
                    ]
                    .into_iter()
                    .find(|(key, _)| i.key_pressed(*key))
                    .map(|(_, t)| t);
                } else if i.key_pressed(egui::Key::ArrowRight) {
                    self.navigate(1);
                } else if i.key_pressed(egui::Key::ArrowLeft) {
                    self.navigate(-1);
                }
                // Y: Before/After left and right, Option+Y top and bottom, Shift+Y
                // split. Option changes the typed letter on macOS, so match the
                // physical key too.
                let y = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, physical_key, pressed: true, repeat: false, .. }
                        if *key == egui::Key::Y || *physical_key == Some(egui::Key::Y))
                });
                if y && !m.command {
                    use super::before_after::{Axis, Compare};
                    let view = if m.alt {
                        Compare::SideBySide(Axis::TopBottom)
                    } else if m.shift {
                        Compare::Split(Axis::LeftRight)
                    } else {
                        Compare::SideBySide(Axis::LeftRight)
                    };
                    self.set_compare(self.view.compare.toggled(view));
                }
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::C) {
                    copy = true;
                }
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::V) {
                    paste = true;
                }
                // Lightroom's Paste Settings from Previous. Option changes the typed
                // letter on macOS, so match the physical key too.
                let v = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, physical_key, pressed: true, repeat: false, .. }
                        if *key == egui::Key::V || *physical_key == Some(egui::Key::V))
                });
                if v && i.modifiers.command && i.modifiers.alt && !i.modifiers.shift {
                    previous = true;
                }
                // Lightroom's Match Total Exposures; Option changes the typed letter
                // on macOS, so match the physical key too.
                let m = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, physical_key, pressed: true, repeat: false, .. }
                        if *key == egui::Key::M || *physical_key == Some(egui::Key::M))
                });
                if m && i.modifiers.command && i.modifiers.shift && i.modifiers.alt {
                    match_exposures = true;
                }
                // Lightroom's New Develop Preset.
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::N) {
                    new_preset = true;
                }
                // Lightroom's Targeted Adjustment Tools: Cmd+Option+Shift and T (Tone
                // Curve), H, S, L (the Color Mixer's Hue, Saturation, Luminance) or G
                // (B&W). Option changes the typed letter on macOS, so match the
                // physical key too.
                if i.modifiers.command && i.modifiers.alt && i.modifiers.shift {
                    use crate::develop::targeted::{HslChannel, Target};
                    for (key, target) in [
                        (egui::Key::T, Target::ToneCurve),
                        (egui::Key::H, Target::Hsl(HslChannel::Hue)),
                        (egui::Key::S, Target::Hsl(HslChannel::Saturation)),
                        (egui::Key::L, Target::Hsl(HslChannel::Luminance)),
                        (egui::Key::G, Target::BlackWhite),
                    ] {
                        let pressed = i.events.iter().any(|event| {
                            matches!(event, egui::Event::Key { key: k, physical_key, pressed: true, repeat: false, .. }
                                if *k == key || *physical_key == Some(key))
                        });
                        if pressed {
                            targeted = Some(target);
                        }
                    }
                }
                // Lightroom's Sync Settings.
                if i.modifiers.command
                    && i.modifiers.shift
                    && !i.modifiers.alt
                    && i.key_pressed(egui::Key::S)
                {
                    sync = true;
                }
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::R) {
                    reset = true;
                }
                // Lightroom's Convert to Black & White.
                // Once per press: holding V does not flip it back and forth.
                treatment = !i.modifiers.any()
                    && i.events.iter().any(|event| {
                        matches!(event, egui::Event::Key { key: egui::Key::V, pressed: true, repeat: false, .. })
                    });
                // Lightroom's Auto Settings.
                if i.modifiers.command && i.modifiers.shift && i.key_pressed(egui::Key::U) {
                    auto = true;
                }
                // Shift+Cmd+E exports, Option+Shift+Cmd+E exports with the previous
                // choices. Option changes the typed letter on macOS, so match the
                // physical key too.
                let e = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, physical_key, pressed: true, repeat: false, .. }
                        if *key == egui::Key::E || *physical_key == Some(egui::Key::E))
                });
                if e && i.modifiers.command && i.modifiers.shift {
                    export = Some(i.modifiers.alt);
                }
                // Cmd+Z / Cmd+Shift+Z on macOS, Ctrl+Z / Ctrl+Shift+Z or Ctrl+Y elsewhere.
                if i.modifiers.command && i.key_pressed(egui::Key::Z) {
                    if i.modifiers.shift {
                        self.redo();
                    } else {
                        self.undo();
                    }
                }
                if !cfg!(target_os = "macos")
                    && i.modifiers.command
                    && !i.modifiers.shift
                    && i.key_pressed(egui::Key::Y)
                {
                    self.redo();
                }
                // Shift+R is Reference View, below.
                if (i.key_pressed(egui::Key::C)
                    || i.key_pressed(egui::Key::R) && !i.modifiers.shift)
                    && !i.modifiers.command
                {
                    self.view.toggle(Tool::Crop);
                }
                if i.key_pressed(egui::Key::R)
                    && i.modifiers.shift
                    && !i.modifiers.command
                    && !i.modifiers.alt
                {
                    self.toggle_reference_view();
                }
                // Lightroom's I: the photo info overlay, Info 1, Info 2 or off.
                // Once per press: a held I must not flicker through them. With the
                // modifiers held for it, which a quick shortcut can release in the
                // same frame.
                let info = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key: egui::Key::I, pressed: true, repeat: false, modifiers, .. } if !modifiers.any())
                });
                if info && let Some(library) = &mut self.library {
                    library.cycle_loupe_info();
                }
                // Shift+J makes a colour range mask, below.
                if i.key_pressed(egui::Key::J) && !i.modifiers.any() {
                    self.view.clipping.toggle_both();
                }
                if i.key_pressed(egui::Key::Backslash) {
                    let view = self.view.compare.toggled(super::before_after::Compare::BeforeOnly);
                    self.set_compare(view);
                }
                if i.key_pressed(egui::Key::Enter)
                    && (self.view.is(Tool::Crop) || self.view.is(Tool::Guided))
                {
                    self.view.tool = Tool::None;
                }
                if i.key_pressed(egui::Key::W) && !i.modifiers.any() {
                    self.view.toggle(Tool::WhiteBalance);
                }
                if i.key_pressed(egui::Key::Q) && !i.modifiers.any() {
                    self.view.toggle(Tool::Remove);
                }
                if i.key_pressed(egui::Key::W) && i.modifiers.shift && !i.modifiers.command {
                    self.view.toggle(Tool::Mask);
                }
                if i.key_pressed(egui::Key::T) && i.modifiers.shift && !i.modifiers.command {
                    self.toggle_guided_tool();
                }
                if i.key_pressed(egui::Key::Escape) {
                    self.view.tool = Tool::None;
                }
                if self.view.is(Tool::Crop) {
                    self.crop_keys(i);
                }
                if self.view.is(Tool::Remove) {
                    self.retouch_keys(i);
                }
                if self.view.is(Tool::RedEye) {
                    self.red_eye_keys(i);
                }
                if self.view.is(Tool::Mask) {
                    self.mask_keys(i);
                }
                if self.view.is(Tool::Guided) {
                    self.guided_keys(i);
                }
                // New masks: K brush, M linear, Shift+M radial, Shift+J colour range.
                if !i.modifiers.command && !i.modifiers.alt {
                    use super::mask_tool::Kind;
                    let kind = if i.key_pressed(egui::Key::K) && !i.modifiers.shift {
                        Some(Kind::Brush)
                    } else if i.key_pressed(egui::Key::M) {
                        Some(if i.modifiers.shift { Kind::Radial } else { Kind::Linear })
                    } else if i.key_pressed(egui::Key::J) && i.modifiers.shift {
                        Some(Kind::Color)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        self.create_mask(kind, None);
                    }
                }
            });
            // As in Lightroom, Shift+Cmd+C opens Copy Settings. Not while a folder is
            // being picked: the folder question that follows would replace it.
            if copy && !self.activity.is_busy() {
                self.open_copy_dialog(super::settings_transfer::Transfer::Copy);
            }
            if match_exposures && !self.sync_targets().is_empty() && !self.activity.is_busy() {
                self.start_sync(super::sync::BatchChange::MatchTotalExposures);
            }
            if new_preset && !self.activity.is_busy() {
                self.open_copy_dialog(super::settings_transfer::Transfer::NewPreset);
            }
            if sync && !self.sync_targets().is_empty() && !self.activity.is_busy() {
                self.open_copy_dialog(super::settings_transfer::Transfer::Sync);
            }
            if reset {
                self.reset_settings();
            }
            if let Some(target) = targeted {
                self.toggle_targeted(target);
            }
            if auto && !self.auto_in_effect() {
                self.start_auto(super::worker::AutoKind::Settings);
            }
            if treatment {
                self.toggle_treatment();
            }
            match export {
                Some(true) => self.export_with_previous(),
                Some(false) => self.open_export_dialog(),
                None => {}
            }
            if paste {
                self.paste_settings();
            }
            if previous {
                self.paste_previous();
            }
            if let Some(transfer) = transfer {
                self.transfer(transfer);
            }
        }
    }

    /// The items a Library summary (sidecars that could not be read) lists
    /// on hover, while `message` is that summary.
    fn message_detail(&self, message: &str) -> Option<&str> {
        self.library
            .as_ref()
            .filter(|l| !l.message.is_empty() && l.message == message)
            .and_then(|l| l.message_detail())
    }
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal(|ui| {
                let display = if self.view.monitor.is_some() {
                    "Display: custom ICC (disable compositor ICC conversion)"
                } else {
                    "Display: sRGB (compositor may manage the monitor)"
                };
                status_text(
                    ui,
                    self.document
                        .edit
                        .save_state()
                        .message()
                        .unwrap_or(&self.status),
                    self.message_detail(&self.status),
                    Some(display),
                );
                if !self.preview.status.is_empty() {
                    ui.separator();
                    ui.small(&self.preview.status);
                }
                self.preview_progress(ui);
                if !self.document.lightroom_notice.is_empty() {
                    ui.separator();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(
                                self.document.lightroom_notice.lines().next().unwrap_or(""),
                            )
                            .small(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&self.document.lightroom_notice);
                }
                if self.document.edit.save_state().is_protected() {
                    ui.separator();
                    ui.colored_label(
                        Color32::YELLOW,
                        egui::RichText::new(
                            "Saved edits protected; editing is temporary. Export or save a preset.",
                        )
                        .small(),
                    );
                }
            });
        });
    }

    /// Library preview progress, right-aligned inside an existing status row
    /// so its appearance never changes the layout.
    fn preview_progress(&self, ui: &mut egui::Ui) {
        if let Some(library) = &self.library
            && library.preview_progress_active()
        {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                library.preview_progress(ui);
            });
        }
    }
    /// The Library's filmstrip, the same panel as in the Library, with the
    /// photo open here highlighted.
    fn filmstrip(&mut self, ui: &mut egui::Ui) {
        if let Some(library) = &mut self.library {
            let current = self.document.catalog_photo;
            let strip = library.filmstrip_panel(ui, current, crate::app::library::Module::Develop);
            if strip.metadata_changed {
                self.status = library.message.clone();
            }
            // Cmd and Shift select, as in the Library, for Sync; otherwise a click
            // and Open in Develop show the photo.
            let modifiers = ui.input(|i| i.modifiers);
            if let Some(crate::app::library::Pick::Show(id)) = strip.pick
                && library.develop_select(id, current, modifiers)
            {
                return;
            }
            if let Some(crate::app::library::Pick::Reference(id)) = strip.pick {
                self.set_reference(id);
                return;
            }
            if let Some(
                crate::app::library::Pick::Show(id) | crate::app::library::Pick::Develop(id),
            ) = strip.pick
                && Some(id) != current
                && !self.activity.is_busy()
            {
                self.develop_catalog_photo(id);
            }
        }
    }

    fn develop_left_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::left("presets")
            .default_size(245.)
            .min_size(PRESETS_MIN)
            .max_size(400.)
            .show(ui, |ui| {
                let _side = super::widgets::SectionSide::enter(
                    ui,
                    super::widgets::SectionGroup::DevelopLeft,
                );
                self.navigator_ui(ui);
                self.presets_ui(ui);
            });
    }
    fn develop_panels(&mut self, ui: &mut egui::Ui) {
        if self.panel_shown(WorkspacePanel::Right) {
            self.develop_right_panel(ui);
        }
        let area = egui::CentralPanel::default()
            .show(ui, |ui| self.viewport_ui(ui))
            .response
            .rect;
        self.drop_area = Some(area);
    }
    fn develop_right_panel(&mut self, ui: &mut egui::Ui) {
        egui::Panel::right("adjustments")
            .default_size(330.)
            .min_size(ADJUSTMENTS_MIN)
            .max_size(400.)
            .show(ui, |ui| {
                let _side = super::widgets::SectionSide::enter(
                    ui,
                    super::widgets::SectionGroup::DevelopRight,
                );
                egui::ScrollArea::vertical().show(ui, |ui| {
                    let enabled =
                        self.document.full().is_some() && !self.view.compare.before_only();
                    ui.add_enabled_ui(enabled, |ui| self.controls(ui));
                });
            });
    }

    /// Whether quitting now would cut off work or could lose an edit: an export, Sync
    /// Settings, a folder change or a control output running, or an edit, Copy Name
    /// or metadata field not saved yet, pending, being saved or failed. A quit the
    /// close guard doesn't see (the Dock, logging out) then closes the window
    /// instead, so the guard saves first and asks if that fails: an earlier save
    /// succeeding says nothing of the next.
    pub(super) fn quitting_would_cut_off_work(&self) -> bool {
        self.closing_waits_for().is_some()
            || self.document.edit.save_state().needs_save()
            || self.library.as_ref().is_some_and(|l| l.has_drafts())
    }
    /// The work closing waits for, as the close guard names it: closing would cut
    /// it off, and nothing can be saved in its place.
    fn closing_waits_for(&self) -> Option<&'static str> {
        if self.exporting() {
            Some("the export")
        } else if self.activity.is_syncing() {
            Some("Sync Settings")
        } else if self.activity.is_changing_folder() {
            Some(FOLDER_CHANGE)
        } else if self.automation.outputs_running() {
            Some(OUTPUT)
        } else {
            None
        }
    }
    pub(super) fn pending_work(&mut self, ctx: &egui::Context) {
        self.autosave(ctx);
        // For a quit the close guard doesn't see (the Dock, logging out).
        crate::platform::quit::set_work_pending(self.quitting_would_cut_off_work());
        if let Some(due) = self.document.edit.save_state().due_in() {
            // Just after it is due, so the frame finds it ready.
            ctx.request_repaint_after(due + Duration::from_millis(10));
        }
        if ctx.input(|i| i.viewport().close_requested()) {
            let until = Instant::now() + super::exit::DEADLINE;
            let anyway = std::mem::take(&mut self.close_anyway);
            let flushed = if !anyway && self.closing_waits_for().is_some() {
                None
            } else {
                Some(self.flush_by(until))
            };
            if flushed == Some(Flushed::Saved) {
                self.quit_by = Some(until);
            } else {
                refuse_close(ctx);
                self.close_confirm = true;
                // Work still running, or a save still in flight, closes once done.
                self.close_after_work = flushed != Some(Flushed::Failed);
            }
        }
        if self.close_after_work && self.closing_waits_for().is_none() && !self.autosave.busy() {
            self.close_confirm = false;
            self.close_after_work = false;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if self.close_confirm {
            let waits_for = self.closing_waits_for();
            egui::Window::new("Work still pending").show(ctx, |ui| {
                ui.label(if let Some(work) = waits_for {
                    format!("Wait for {work} to finish before closing.")
                } else if self.autosave.busy() {
                    "Edits are still being saved: the catalog is not answering. Wait, or \
                     close without saving."
                        .into()
                } else {
                    "Edits could not be saved. Retry or save a preset before closing.".into()
                });
                if ui.button("Keep editing").clicked() {
                    self.close_confirm = false;
                    self.close_after_work = false;
                }
                // What only runs without its own way to stop it: closing anyway
                // must stay possible should it stall on a network share.
                let anyway = match waits_for {
                    Some(OUTPUT) => Some("Cancel it and close"),
                    Some(FOLDER_CHANGE) => Some("Close anyway"),
                    _ => None,
                };
                if let Some(label) = anyway
                    && ui.button(label).clicked()
                {
                    self.automation.cancel_outputs();
                    self.close_anyway = true;
                    self.close_confirm = false;
                    self.close_after_work = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                if waits_for.is_none() && ui.button("Close without saving").clicked() {
                    self.document.edit.save_state_mut().saved();
                    if let Some(library) = &mut self.library {
                        library.discard_drafts();
                    }
                    self.close_confirm = false;
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });
        }
        if ctx.input(|i| !i.raw.hovered_files.is_empty()) && !self.activity.is_busy() {
            let catalog = self
                .library
                .as_ref()
                .map(|l| l.session.catalog.location().name());
            let area = self.drop_area.unwrap_or_else(|| ctx.content_rect());
            drop_overlay(ctx, area, catalog.as_deref());
        }
        let dropped: Vec<std::path::PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|file| file.path().to_path_buf())
                .collect()
        });
        if !dropped.is_empty() {
            self.dropped(dropped);
        }
    }
}

/// What dropping files onto the window does, shown while they are dragged
/// over it: the main `area` becomes the drop zone, drawn over whatever it
/// showed; the side panels stay as they are.
fn drop_overlay(ctx: &egui::Context, area: egui::Rect, catalog: Option<&str>) {
    let palette = theme::palette(ctx);
    let painter = ctx
        .layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("drop-overlay"),
        ))
        .with_clip_rect(area);
    painter.rect_filled(area, 0., palette.gray(24));
    // A dashed outline inset from the area's edges.
    let zone = area.shrink(16.);
    let stroke = egui::Stroke::new(1.5, palette.accent());
    for (from, to) in [
        (zone.left_top(), zone.right_top()),
        (zone.right_top(), zone.right_bottom()),
        (zone.right_bottom(), zone.left_bottom()),
        (zone.left_bottom(), zone.left_top()),
    ] {
        painter.extend(egui::Shape::dashed_line(&[from, to], stroke, 8., 6.));
    }
    let (title, detail) = match catalog {
        Some(name) => (
            format!("Drop to add to {name}"),
            "Folders come with their subfolders. Photos stay where they are.",
        ),
        None => (
            "Drop a catalog to open it".to_string(),
            "RAWmakase catalogs end in .rawmakase.",
        ),
    };
    let c = zone.center();
    painter.circle_filled(
        c - Vec2::new(0., 44.),
        34.,
        palette.accent().gamma_multiply(0.18),
    );
    super::icons::paint_at(
        &painter,
        super::icons::Icon::FolderPlus,
        c - Vec2::new(0., 44.),
        30.,
        palette.accent(),
    );
    painter.text(
        c + Vec2::new(0., 16.),
        egui::Align2::CENTER_CENTER,
        title,
        egui::FontId::proportional(18.),
        palette.gray(240),
    );
    painter.text(
        c + Vec2::new(0., 44.),
        egui::Align2::CENTER_CENTER,
        detail,
        egui::FontId::proportional(13.),
        palette.gray(150),
    );
}

/// A row of a menu: an icon in its column, then the label; the whole row
/// highlights and takes the click.
fn menu_row(
    ui: &mut egui::Ui,
    icon: super::icons::Icon,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let palette = theme::palette(ui.ctx());
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.), sense);
    let hovered = enabled && response.hovered();
    if hovered {
        ui.painter()
            .rect_filled(rect.shrink2(Vec2::new(4., 1.)), 5., palette.gray(50));
    }
    let color = match (enabled, hovered) {
        (false, _) => palette.gray(95),
        (true, true) => palette.gray(245),
        (true, false) => palette.gray(205),
    };
    let y = rect.center().y;
    super::icons::paint_at(
        ui.painter(),
        icon,
        egui::pos2(rect.left() + 20., y),
        15.,
        if enabled {
            palette.gray(160)
        } else {
            palette.gray(80)
        },
    );
    ui.painter().text(
        egui::pos2(rect.left() + 40., y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::FontId::proportional(13.),
        color,
    );
    if enabled {
        response.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        response
    }
}
/// A hairline between a menu's groups.
fn menu_separator(ui: &mut egui::Ui) {
    ui.add_space(4.);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.), egui::Sense::hover());
    ui.painter().rect_filled(
        rect.shrink2(Vec2::new(10., 0.)),
        0.,
        theme::palette(ui.ctx()).gray(48),
    );
    ui.add_space(4.);
}

/// What the close guard calls an export or preview a control command started.
const OUTPUT: &str = "the output the control socket asked for";
/// What the close guard calls a folder change.
const FOLDER_CHANGE: &str = "the folder change";

/// The side panels' narrowest widths, no less than their contents need: a
/// panel dragged narrower is painted only that wide but laid out as wide as
/// its contents, leaving a strip of the window's black between. The left
/// panels hold the Navigator, whose zoom levels need 224 points.
pub(super) const PRESETS_MIN: f32 = 224.;
pub(super) const ADJUSTMENTS_MIN: f32 = 300.;
pub(super) const LIBRARY_SIDEBAR_MIN: f32 = 224.;
pub(super) const LIBRARY_INFO_MIN: f32 = 220.;

/// The workspace bar's height; on macOS the traffic lights sit on its centre.
pub(super) const BAR_HEIGHT: f32 = 44.;

/// The bar's empty space moves the window, and a double click does what
/// System Settings says, as a title bar does. Controls drawn later take their
/// own clicks. Only macOS hides the system title bar.
fn title_bar_drag(ui: &mut egui::Ui) {
    if !cfg!(target_os = "macos") {
        return;
    }
    let bar = ui.max_rect().expand2(Vec2::new(18., 0.));
    let response = ui.interact(
        bar,
        ui.id().with("title-bar"),
        egui::Sense::click_and_drag(),
    );
    let ctx = ui.ctx();
    if response.double_clicked() {
        match fastframe_macos::double_click_action() {
            fastframe_macos::DoubleClick::Minimize => {
                ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
            }
            fastframe_macos::DoubleClick::Zoom => {
                let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
                ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
            }
            // AppKit fills the screen itself, from the drag started below.
            fastframe_macos::DoubleClick::Fill | fastframe_macos::DoubleClick::Nothing => {}
        }
    } else if response.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_pressed()) {
        // AppKit only starts a drag during the original mouse-down.
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}
/// A status line showing `text`, with `detail` (a Library summary's items)
/// or else `hover` on hover.
fn status_text(ui: &mut egui::Ui, text: &str, detail: Option<&str>, hover: Option<&str>) {
    let shown = ui.small(text);
    if let Some(hover) = detail.or(hover) {
        shown.on_hover_text(hover);
    }
}

/// Whether the Library's layout (thumbnail size, search) may still be changing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayoutEdit {
    /// A drag or typing is under way: kept once it ends.
    Changing,
    Settled,
}

/// Keeps the window open for the "Work still pending" question, and shows it: Quit
/// from the app menu (Cmd-Q) reaches a minimized window, where the question would
/// stay hidden until the window was restored. Quit from the Dock reaches it too while
/// work is pending (`platform::quit`).
fn refuse_close(ctx: &egui::Context) {
    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
    if ctx.input(|i| i.viewport().minimized == Some(true)) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
    }
}

#[cfg(test)]
mod tests {
    use super::refuse_close;
    use eframe::egui::{self, ViewportCommand, ViewportId, ViewportInfo};

    fn commands(minimized: bool) -> Vec<ViewportCommand> {
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.viewports.insert(
            ViewportId::ROOT,
            ViewportInfo {
                minimized: Some(minimized),
                ..Default::default()
            },
        );
        let mut output = ctx.run_ui(input, |ui| refuse_close(ui.ctx()));
        output.textures_delta.clear();
        // egui sets the theme on a first frame; only the window commands matter here.
        output.viewport_output[&ViewportId::ROOT]
            .commands
            .iter()
            .filter(|c| !matches!(c, ViewportCommand::SetTheme(_)))
            .cloned()
            .collect()
    }

    #[test]
    fn a_refused_close_shows_a_minimized_window_for_its_question() {
        assert_eq!(
            commands(true),
            [
                ViewportCommand::CancelClose,
                ViewportCommand::Minimized(false),
                ViewportCommand::Focus
            ]
        );
        assert_eq!(commands(false), [ViewportCommand::CancelClose]);
    }

    #[test]
    fn an_unsaved_or_failed_edit_is_work_a_quit_would_cut_off() {
        let ctx = egui::Context::default();
        let mut editor = crate::app::Editor::with_context(
            &ctx,
            None,
            crate::app::session::Session::default(),
            None,
        );
        assert!(!editor.quitting_would_cut_off_work());
        // Not saved yet: its first save may fail, so the close guard saves it.
        editor.document.edit.save_state_mut().mark_changed();
        assert!(editor.quitting_would_cut_off_work());
        // Being saved in the background: the guard waits for the result.
        editor.document.edit.save_state_mut().saving();
        assert!(editor.quitting_would_cut_off_work());
        editor
            .document
            .edit
            .save_state_mut()
            .failed("disk full".into());
        assert!(editor.quitting_would_cut_off_work());
        editor.document.edit.save_state_mut().saved();
        assert!(!editor.quitting_would_cut_off_work());
    }
}
