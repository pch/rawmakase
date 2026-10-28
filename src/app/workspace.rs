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
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};
use std::time::{Duration, Instant};

impl Editor {
    pub(super) fn workspace_modal(&self) -> bool {
        self.preferences.open
            || self.export_modal()
            || self.modal.is_some()
            || self.not_editable.is_some()
            || self.view.shortcuts
            || self.report_issue.is_open()
            || self.curve_save_open()
            || self.lut_import_open()
            || self.folder_sync.is_reviewing()
            || self.file_ops.dialog_open()
    }

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
        // Before any field takes this frame's keys, or gives them up.
        let typing = ctx.text_edit_focused();
        self.control_commands(&ctx);
        self.events(&ctx);
        self.poll_updates(&ctx);
        self.themes.poll(&ctx);
        // Checksums are read only while nothing heavier reads files.
        let busy = self.checksums_paused
            || self.exporting()
            || self.load.is_running()
            || self.preview_builds.progress().is_some_and(|p| p.running());
        if let Some(library) = &mut self.library {
            library.session.pause_checksums(busy);
            library.publish_shown();
            library.poll_previews(&ctx);
        }
        self.poll_preview_builds();
        // Entering a module draws its chrome: the whole-window mode belongs to
        // the module it was turned on in.
        let loupe = self.library.as_ref().is_some_and(|l| l.loupe_open());
        let entered = self.chrome.follow(self.module, loupe);
        self.chrome_view(entered);
        // Preferences is modal: keys go to it, not to the photo behind.
        #[cfg(target_os = "macos")]
        self.native_menu_actions(&ctx);
        let modal = self.workspace_modal();
        if !modal {
            // Escape leaves the whole-window mode, as it does in Lightroom. It
            // is taken here, before the Library's views and Develop's tools, so
            // they keep it while the chrome is up and the photo is the way out
            // while it is not. A menu open over the view closes first.
            if self.chrome.hidden(self.module)
                && !egui::Popup::is_any_open(&ctx)
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
            {
                let shown = self.chrome.show();
                self.chrome_view(shown);
            }
            self.metadata_shortcuts(&ctx);
            self.workspace_shortcuts(&ctx);
            self.preferences_shortcut(&ctx);
            if !self.onboarding.visible && !self.activity.is_busy() {
                self.panel_keys(&ctx);
            }
        }
        // The sidebar runs the window's height, beside the bar. Whether it
        // shows is read once: its toggle hiding it must not bring the bar's
        // toggle up under the same click.
        let sidebar_shown = self.sidebar_shown();
        let sidebar = if sidebar_shown {
            self.library_left_panel(ui)
        } else {
            crate::app::library::Action::None
        };
        if self.chrome.shown(self.module) {
            self.workspace_bar(ui, sidebar_shown);
        }
        // A file dialog or a running Sync: nothing behind them takes clicks, so no
        // catalog action is chosen only to be dropped.
        if self.activity.is_busy() {
            ui.disable();
        }
        if self.onboarding.visible {
            self.onboarding_ui(ui);
        } else if self.module == Module::Library {
            self.left_develop();
            self.library_workspace(ui, sidebar);
        } else {
            let mut frame = self.begin_develop_frame();
            if !modal {
                self.develop_shortcuts(&ctx);
            }
            // The catalog's sidebar, drawn before the bar, runs the window's
            // height; the filmstrip and toolbar take the rest of its width.
            // Each part may open another photo; what follows edits that one.
            self.follow_edit_frame(&mut frame);
            self.library_action(sidebar, &ctx);
            self.follow_edit_frame(&mut frame);
            if self.panel_shown(WorkspacePanel::Filmstrip) {
                self.status_bar(ui);
                self.filmstrip(ui);
                self.follow_edit_frame(&mut frame);
            }
            // The panels on the right run up to the bar; the toolbar sits
            // over the photo alone.
            self.develop_side_panels(ui);
            self.follow_edit_frame(&mut frame);
            if self.chrome.shown(self.module) {
                self.toolbar(ui);
            }
            self.follow_edit_frame(&mut frame);
            self.develop_photo(ui);
            self.finish_edit_frame(frame, &ctx);
        }
        if self.chrome.hidden(self.module) {
            title_bar_pad(ui);
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
        if let Some((scope, name)) = self.library.as_mut().and_then(|l| l.take_sync_request()) {
            self.start_folder_sync(scope, name);
        }
        if let Some(photo) = self.library.as_mut().and_then(|l| l.take_find_request()) {
            let ctx = self.context.clone();
            self.choose_file_on_disk(photo, &ctx);
        }
        self.poll_folder_sync();
        if let Some(removal) = self.library.as_mut().and_then(|l| l.take_removal_request()) {
            self.modal = Some(super::Modal::RemoveFolder(removal));
        }
        if let Some(ids) = self.library.as_mut().and_then(|l| l.take_read_request()) {
            self.modal = Some(super::Modal::ReadMetadata(ids));
        }
        use super::library::FileRequest;
        match self.library.as_mut().and_then(|l| l.take_file_request()) {
            Some(FileRequest::Remove(ids)) => self.request_photo_removal(ids),
            Some(FileRequest::Delete(ids)) => self.request_photo_delete(ids, false),
            Some(FileRequest::Trash(ids)) => self.request_photo_trash(ids),
            Some(FileRequest::DeleteRejected) => self.request_rejected_delete(),
            Some(FileRequest::Rename(ids)) => self.request_rename(ids),
            Some(FileRequest::ChooseParent { root, from }) => self.choose_parent_for(root, from),
            Some(FileRequest::MoveFolder {
                root,
                from,
                to_root,
                to,
            }) => self.request_folder_move(root, from, to_root, to),
            Some(FileRequest::RenameFolder { root, logical }) => self.rename_folder(root, logical),
            Some(FileRequest::TrashFolder { root, logical }) => {
                self.request_folder_trash(root, logical)
            }
            Some(FileRequest::Move {
                photos,
                root,
                to,
                name,
            }) => self.request_photo_move(photos, root, to, name),
            Some(FileRequest::ChooseFolder(ids)) => self.choose_folder_for(ids),
            Some(FileRequest::NewFolder {
                root,
                parent,
                name,
                photos,
                inside,
            }) => self.name_new_folder(root, parent, name, photos, inside),
            None => {}
        }
        self.poll_file_recovery();
        if let Some(library) = &mut self.library
            && library.take_reread_finished()
        {
            self.status = library.message.clone();
            // Reading metadata can bring in the reference photo's Lightroom edit.
            self.load_reference();
        }
        self.remove_copy_window(&ctx);
        self.remove_folder_window(&ctx);
        self.folder_sync_window(&ctx);
        self.discard_for_find_window(&ctx);
        self.read_metadata_window(&ctx);
        self.file_op_window(&ctx);
        self.not_editable_window(&ctx);
        self.shortcuts_window(&ctx);
        self.report_issue_window(&ctx);
        self.copy_dialog_window(&ctx);
        self.preset_rename_window(&ctx);
        self.curve_save_window(&ctx);
        self.preferences_window(&ctx);
        // Above Preferences, where Change… asks it.
        self.folder_question_window(&ctx);
        // Above Preferences too, which opens it.
        self.lut_import_window(&ctx);
        self.export_windows(&ctx);
        let dialog = modal || self.view.shortcuts || self.report_issue.is_open();
        self.update_notice(&ctx, dialog);
        // Last, after whatever else this frame opened or asked: see its docs.
        self.develop_key_steps(&ctx, typing);
        self.pending_work(&ctx);
        #[cfg(target_os = "macos")]
        self.update_native_menu(&ctx);
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
        let pinned = ctx.data(|d| {
            d.get_temp::<std::collections::BTreeSet<String>>(super::widgets::pinned_sections_id())
        });
        if let Some(pinned) = pinned
            && pinned != self.pinned
        {
            self.pinned = pinned;
            let _ = self.save_session();
        }
        self.remember_maximized(&ctx);
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
    /// Saves the session when the window is maximized or restored. A minimized
    /// window keeps what it was: Windows reports it as not maximized.
    pub(super) fn remember_maximized(&mut self, ctx: &egui::Context) {
        if !KEEPS_MAXIMIZED {
            return;
        }
        let (maximized, now) = ctx.input(|i| {
            let viewport = i.viewport();
            let maximized = viewport
                .maximized
                .filter(|_| viewport.minimized != Some(true));
            (maximized, i.time)
        });
        match self.restoring_maximized {
            RestoreMaximized::Pending if maximized == Some(true) => {
                self.restoring_maximized = RestoreMaximized::Done;
            }
            RestoreMaximized::Pending => {
                // Applied once the first frame has shown the window, which X11
                // window managers wait for. Not to a minimized window, which it
                // would restore.
                if maximized == Some(false) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
                self.restoring_maximized = RestoreMaximized::Asked {
                    until: now + MAXIMIZE_GRACE,
                };
                return;
            }
            RestoreMaximized::Asked { until } => {
                if maximized != Some(true) && now < until {
                    return;
                }
                self.restoring_maximized = RestoreMaximized::Done;
            }
            RestoreMaximized::Done => {}
        }
        if let Some(maximized) = maximized
            && maximized != self.maximized
        {
            self.maximized = maximized;
            let _ = self.save_session();
        }
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
            // F hides the chrome, leaving the photo the window; in the Library
            // it is the grid that takes the window first, and the photo that
            // takes it on the next press (see `chrome`). No modifier at all, so
            // Shift+F is not it; the event's own modifiers, which a quick
            // key-up cannot outrun. Held, it changes once, and the setup view
            // has no chrome to hide.
            let hide = !self.onboarding.visible
                && ctx.input(|i| {
                    i.events.iter().any(|e| {
                        matches!(
                            e,
                            egui::Event::Key {
                                key: egui::Key::F,
                                pressed: true,
                                repeat: false,
                                modifiers,
                                ..
                            } if !modifiers.any()
                        )
                    })
                });
            if hide {
                self.toggle_chrome();
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

    /// F: hides the open module's chrome, or brings it back (see `chrome`).
    /// The Library's info panel goes with the chrome, so a field still being
    /// typed in it is saved first, as hiding the panel does; false, with the
    /// chrome still up, if it could not be. With no photo for the Library's
    /// next step, the chrome comes back.
    pub(super) fn toggle_chrome(&mut self) -> bool {
        if self.chrome.shown(self.module)
            && self.module == Module::Library
            && !self.commit_library_drafts()
        {
            return false;
        }
        let loupe = self.library.as_ref().is_some_and(|l| l.loupe_open());
        let view = self.chrome.toggle(self.module, loupe);
        self.chrome_view(view);
        // No photo to give the window to, as a search matching none leaves
        // the grid: the press brings the chrome back instead.
        if view == super::chrome::View::OpenLoupe
            && !self.library.as_ref().is_some_and(|l| l.loupe_open())
        {
            let shown = self.chrome.show();
            self.chrome_view(shown);
        }
        true
    }
    /// Applies what the whole-window mode did to the Library's view: the photo
    /// takes the window, or the grid takes it back.
    pub(super) fn chrome_view(&mut self, view: super::chrome::View) {
        let Some(library) = &mut self.library else {
            return;
        };
        match view {
            super::chrome::View::Unchanged => {}
            super::chrome::View::OpenLoupe => library.open_loupe(),
            super::chrome::View::CloseLoupe => library.close_loupe(),
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
        let selected = self.library.as_ref().and_then(|l| l.selected_sync_scope());
        let scanning = self.folder_sync.is_scanning();
        if menu_row(
            ui,
            Icon::Folder,
            "Synchronize Folder…",
            selected.is_some() && !scanning,
        )
        .on_hover_text(
            "Find photos added, deleted, moved or renamed outside RAWmakase in the selected folder",
        )
        .clicked()
            && let Some((scope, name)) = selected
        {
            self.start_folder_sync(scope, name);
            ui.close();
        }
        if menu_row(
            ui,
            Icon::Folder,
            "Synchronize All Folders…",
            location.is_some() && !scanning,
        )
        .clicked()
        {
            self.start_folder_sync(
                crate::catalog::folder_sync::SyncScope::All,
                "All Folders".into(),
            );
            ui.close();
        }
        if menu_row(
            ui,
            Icon::Rejected,
            "Delete Rejected Photos…",
            self.library.is_some(),
        )
        .on_hover_text(
            "Move the rejected photos shown here to the Trash, or remove them from the catalog",
        )
        .clicked()
        {
            self.request_rejected_delete();
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
    /// Whether the catalog's sidebar shows: in both modules, not the setup view.
    fn sidebar_shown(&self) -> bool {
        !self.onboarding.visible && self.panel_shown(WorkspacePanel::Left)
    }
    /// The catalog's name, whose menu opens, creates or imports one. In the
    /// sidebar (`width` given) it spans it, captioned "Catalog"; in the bar it
    /// is as wide as the name.
    fn catalog_switcher(&mut self, ui: &mut egui::Ui, width: Option<f32>) {
        let palette = theme::palette(ui.ctx());
        let ctx = ui.ctx().clone();
        let catalog = self
            .library
            .as_ref()
            .map(|library| library.session.catalog.location().name())
            .unwrap_or_else(|| "No catalog".into());
        let name = ui.painter().layout_no_wrap(
            catalog,
            egui::FontId::proportional(13.),
            palette.gray(255),
        );
        let busy = self.activity.is_busy();
        // In the bar, every element is painted in a 28 px slot so all centers line up.
        let size = match width {
            Some(width) => Vec2::new(width, 40.),
            None => Vec2::new(name.size().x.min(320.) + 36., 28.),
        };
        let (rect, response) = ui.allocate_exact_size(
            size,
            if busy {
                egui::Sense::hover()
            } else {
                egui::Sense::click()
            },
        );
        let open = egui::Popup::is_id_open(&ctx, egui::Popup::default_response_id(&response));
        let lit = response.hovered() || open;
        if lit {
            ui.painter().rect_filled(
                rect,
                6.,
                palette.gray(if width.is_some() { 32 } else { 38 }),
            );
        }
        let color = palette.gray(if lit { 240 } else { 215 });
        // In the sidebar, on the columns of its captions and counts.
        let (text_left, chevron_x) = if width.is_some() {
            (rect.left() + 10., rect.right() - 15.5)
        } else {
            (rect.left() + 10., rect.right() - 14.)
        };
        let clip = Rect::from_min_max(
            Pos2::new(text_left, rect.top()),
            Pos2::new(chevron_x - 10., rect.bottom()),
        );
        let name_y = if width.is_some() {
            rect.center().y - name.size().y + 1.
        } else {
            rect.center().y - name.size().y / 2.
        };
        ui.painter()
            .with_clip_rect(clip)
            .galley(Pos2::new(text_left, name_y), name, color);
        if width.is_some() {
            ui.painter().with_clip_rect(clip).text(
                Pos2::new(text_left, rect.center().y + 2.),
                egui::Align2::LEFT_TOP,
                "Catalog",
                egui::FontId::proportional(10.),
                palette.gray(120),
            );
        }
        super::icons::paint_at(
            ui.painter(),
            super::icons::Icon::ChevronDown,
            Pos2::new(chevron_x, rect.center().y),
            11.,
            palette.gray(if lit { 220 } else { 130 }),
        );
        let response = response
            .on_hover_text("Catalog: open, create or import")
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        egui::Popup::menu(&response).show(|ui| self.catalog_menu(ui, &ctx));
    }
    fn shortcuts_button(&mut self, ui: &mut egui::Ui) {
        let hover = format!(
            "Keyboard shortcuts · {}",
            super::shortcuts::keys_text("Cmd+/")
        );
        if super::shortcuts::icon_button(ui, super::icons::Icon::Keyboard, &hover).clicked() {
            self.view.shortcuts = !self.view.shortcuts;
        }
    }
    fn report_issue_button(&mut self, ui: &mut egui::Ui) {
        if super::shortcuts::icon_button(ui, super::icons::Icon::Bug, "Report an issue").clicked() {
            self.open_report_issue();
        }
    }
    fn gear_button(&mut self, ui: &mut egui::Ui) {
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
    }
    /// Lightroom's top panel: progress on the left, module picker on the
    /// right. On macOS it is also the title bar, beside the traffic lights.
    fn workspace_bar(&mut self, ui: &mut egui::Ui, sidebar: bool) {
        let palette = theme::palette(ui.ctx());
        // Its separator, as the side panels' edges are.
        egui::Panel::top("workspace-modes")
            .exact_size(BAR_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(palette.gray(26))
                    .inner_margin(egui::Margin::symmetric(18, 0)),
            )
            .show(ui, |ui| {
                // The bar is the title bar on macOS: its empty space moves the
                // window, and a double click does what System Settings says.
                crate::platform::titlebar::drag(ui, ui.max_rect().expand2(Vec2::new(18., 0.)));
                // Beside the sidebar, which holds the traffic lights, its
                // toggle, the catalog menu, Preferences and the shortcuts.
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.;
                    if !sidebar {
                        ui.add_space(fastframe_titlebar::footprint(ui.ctx(), BAR_HEIGHT).left);
                    }
                    // The panel toggles: none in the setup view, which has no panels.
                    let toggles = !self.onboarding.visible;
                    if toggles && !sidebar {
                        ui.add_enabled_ui(!self.activity.is_busy(), |ui| {
                            self.panel_toggle(ui, WorkspacePanel::Left);
                        });
                        ui.add_space(8.);
                    }
                    // The setup view has no sidebar; its catalog menu stays here.
                    if self.onboarding.visible {
                        self.catalog_switcher(ui, None);
                        ui.add_space(8.);
                    }
                    if self.activity.is_busy() {
                        super::widgets::spinner(ui, 14.);
                    }
                    self.export_progress(ui);
                    self.preview_build_progress(ui);
                    self.folder_sync_progress(ui);
                    self.file_op_progress(ui);
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
                        });
                    });
                });
            });
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
        // On the photo's backdrop with Develop's margin, under the toolbar.
        let outer = ui.available_rect_before_wrap();
        ui.painter()
            .rect_filled(outer, 0., theme::palette(ui.ctx()).photo_backdrop());
        let area = outer.shrink(PHOTO_MARGIN);
        let before = self.view.zoom.on;
        let response = ui
            .scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                self.viewport_ui(ui)
            })
            .inner;
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
        // Use the viewport's response so modals, overlapping windows and
        // disabled UI keep their clicks instead of reaching the photo behind.
        let double = response.double_clicked() || response.triple_clicked();
        if double && let Some(on) = library.loupe_double_click() {
            self.view.zoom.on = on;
        }
    }
    fn library_workspace(&mut self, ui: &mut egui::Ui, sidebar: crate::app::library::Action) {
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
            // Lightroom's Sync Settings, from the active photo to the rest of the
            // selection.
            let sync = ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                    egui::Key::S,
                )
            });
            if sync && !self.sync_targets().is_empty() {
                self.open_copy_dialog(super::settings_transfer::Transfer::Sync);
            }
        }
        // The catalog's sidebar, drawn before the bar, asked for this.
        let mut action = sidebar;
        let filmstrip = self.panel_shown(WorkspacePanel::Filmstrip);
        if filmstrip {
            self.library_status_bar(ui);
        }
        // The filmstrip runs under the right panel, as in Develop. Hidden,
        // the views are still brought up to date.
        if let Some(library) = &mut self.library {
            action.then(if filmstrip {
                library.library_filmstrip(ui)
            } else {
                library.prepare(ui.ctx());
                crate::app::library::Action::None
            });
        }
        let develops = self.library.as_ref().and_then(|l| l.loupe_develops());
        if develops != self.loupe_tried {
            self.loupe_tried = None;
        }
        self.loupe_stored_preview();
        if self.panel_shown(WorkspacePanel::Right) {
            action.then(self.library_right_panel(ui, develops));
        }
        let chrome = self.chrome.shown(self.module);
        let area = egui::CentralPanel::default()
            .frame(egui::Frame::new())
            .show(ui, |ui| match &mut self.library {
                Some(l) if !l.session.photos.is_empty() => {
                    action.then(l.grid(ui, &mut self.view.zoom, chrome));
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
        self.library_action(action, &ctx);
    }
    /// Does what the Library's grid, filmstrip or sidebar asked, unless a
    /// file dialog or Sync is running.
    fn library_action(&mut self, action: crate::app::library::Action, ctx: &egui::Context) {
        if !self.activity.is_busy() {
            match action {
                crate::app::library::Action::Develop(id) => self.develop_catalog_photo(id),
                crate::app::library::Action::RelinkRoot(id) => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::RelinkRoot(id)), ctx)
                }
                crate::app::library::Action::RelinkFolder(root, logical) => self.catalog_dialog(
                    CatalogDialog::Folder(FolderAction::RelinkFolder(root, logical)),
                    ctx,
                ),
                crate::app::library::Action::AddFolder => {
                    self.catalog_dialog(CatalogDialog::Folder(FolderAction::Add), ctx)
                }
                crate::app::library::Action::None => {}
            }
        }
    }
    /// The catalog's sidebar, on the left in both modules: its folders and
    /// collections, with the Library's Export… at the foot.
    fn library_left_panel(&mut self, ui: &mut egui::Ui) -> crate::app::library::Action {
        let palette = theme::palette(ui.ctx());
        let mut action = crate::app::library::Action::None;
        egui::Panel::left("library-sidebar")
            .default_size(260.)
            .min_size(LIBRARY_SIDEBAR_MIN)
            .max_size(500.)
            .frame(
                egui::Frame::new()
                    .fill(palette.gray(22))
                    .inner_margin(egui::Margin::symmetric(8, 0)),
            )
            .show(ui, |ui| {
                let busy = self.activity.is_busy();
                let _side = super::widgets::SectionSide::enter(ui, super::widgets::SectionGroup::LibraryLeft);
                // Preferences and the shortcuts at the foot, with the Library's
                // Export… for the photos selected; Develop's toolbar has its own.
                // First, so Report an Issue stays clickable while the rest of
                // the sidebar is disabled.
                let mut export = false;
                egui::Panel::bottom("library-export")
                    .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 8)))
                    .show_separator_line(false)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 0.;
                            ui.add_enabled_ui(!busy, |ui| {
                                self.gear_button(ui);
                                ui.add_space(4.);
                                self.shortcuts_button(ui);
                            });
                            ui.add_space(4.);
                            self.report_issue_button(ui);
                            if self.library.is_some() && self.module == Module::Library {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if busy {
                                            ui.disable();
                                        }
                                        export = super::widgets::action_button(
                                            ui,
                                            "Export…",
                                            Some(super::icons::Icon::Export),
                                            true,
                                        )
                                        .on_hover_text(if cfg!(target_os = "macos") {
                                            "Export the selected photos · ⇧⌘E"
                                        } else {
                                            "Export the selected photos · Ctrl+Shift+E"
                                        })
                                        .clicked();
                                    },
                                );
                            }
                        });
                    });
                // Drawn before the bar, which disables what follows it.
                if busy {
                    ui.disable();
                }
                // The title bar's part over the sidebar: the traffic lights and
                // the sidebar's toggle, then the catalog menu.
                let (row, _) = ui.allocate_exact_size(
                    Vec2::new(ui.available_width(), BAR_HEIGHT),
                    egui::Sense::hover(),
                );
                crate::platform::titlebar::drag(ui, row.expand2(Vec2::new(8., 0.)));
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(row)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        ui.add_space(10. + fastframe_titlebar::footprint(ui.ctx(), BAR_HEIGHT).left);
                        self.panel_toggle(ui, WorkspacePanel::Left);
                    },
                );
                ui.add_space(4.);
                self.catalog_switcher(ui, Some(ui.available_width()));
                if export {
                    self.open_export_dialog();
                }
                if let Some(library) = &mut self.library {
                    action.then(library.sidebar(ui));
                } else {
                    ui.add_space(12.);
                    // On the catalog name's column.
                    egui::Frame::new()
                        .inner_margin(egui::Margin::symmetric(10, 0))
                        .show(ui, |ui| {
                            ui.label(
                                egui::RichText::new(
                                    "Create a RAWmakase catalog or import a Lightroom catalog from the catalog menu above.",
                                )
                                .size(12.)
                                .color(palette.gray(150)),
                            );
                        });
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
    /// The Library's right panel: the Navigator, which stays at the top, over
    /// the active photo's info and metadata.
    fn library_right_panel(
        &mut self,
        ui: &mut egui::Ui,
        develops: Option<PhotoId>,
    ) -> crate::app::library::Action {
        let mut action = crate::app::library::Action::None;
        // Dark as Develop's right panels and the catalog's sidebar.
        let fill = theme::palette(ui.ctx()).gray(22);
        egui::Panel::right("library-info")
            .default_size(270.)
            .min_size(LIBRARY_INFO_MIN)
            .max_size(420.)
            .frame(egui::Frame::side_top_panel(ui.style()).fill(fill))
            .show(ui, |ui| {
                let _side = super::widgets::SectionSide::enter(
                    ui,
                    super::widgets::SectionGroup::LibraryRight,
                );
                // Sync Settings at the foot, as in Lightroom: the active photo's
                // settings onto the rest of the selection.
                let mut sync = false;
                if self.library.is_some() {
                    let targets = self.sync_target_count();
                    egui::Panel::bottom("library-sync")
                        .frame(egui::Frame::new().inner_margin(egui::Margin::symmetric(0, 8)))
                        .show_separator_line(false)
                        .show(ui, |ui| {
                            let layout = egui::Layout::right_to_left(egui::Align::Center);
                            ui.with_layout(layout, |ui| {
                                sync = super::widgets::action_button(
                                    ui,
                                    "Sync Settings…",
                                    None,
                                    targets > 0,
                                )
                                .on_hover_text(sync_hover(targets))
                                .clicked();
                            });
                        });
                }
                if sync {
                    self.open_copy_dialog(super::settings_transfer::Transfer::Sync);
                }
                // The Navigator controls the zoom: Develop's for a RAW in the
                // Loupe, the same one for other photos. Outside the Loupe it
                // shows the selected photo, and a zoom chosen there opens it.
                let loupe = self.library.as_ref().is_some_and(|l| l.loupe_open());
                let (photo, shown) = match &self.library {
                    _ if develops.is_some() => self.navigator_view(),
                    Some(l) if loupe => l
                        .loupe_navigator()
                        .map_or((None, None), |(photo, shown)| (Some(photo), shown)),
                    Some(l) => (l.selected_preview(), None),
                    None => (None, None),
                };
                let zoom = self.view.zoom;
                let mut change = None;
                let mut navigator = |ui: &mut egui::Ui| {
                    ui.scope(|ui| {
                        change = crate::app::navigator::navigator(ui, photo, Some(zoom), shown);
                    });
                };
                // It scrolls with the info below it, or stays at the top once pinned.
                let pinned = super::widgets::pinned(ui.ctx(), "Navigator");
                if pinned {
                    navigator(ui);
                }
                match &mut self.library {
                    Some(library) => {
                        action.then(library.info_panel(ui, |ui| {
                            if !pinned {
                                navigator(ui);
                            }
                        }));
                    }
                    None if !pinned => navigator(ui),
                    None => {}
                }
                match change {
                    Some(change) if develops.is_some() => self.navigator_change(change),
                    Some(change) => self.library_zoom(change),
                    None => {}
                }
            });
        action
    }
    fn library_status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("library-status")
            .exact_size(STATUS_HEIGHT)
            .frame(status_frame(ui, 8))
            .show(ui, |ui| {
            ui.horizontal_centered(|ui| {
                if let Some(work) = &self.catalog_work {
                    super::widgets::spinner(ui, 11.);
                    ui.small(work);
                } else {
                    let shown = self
                        .library
                        .as_ref()
                        .filter(|l| !l.message.is_empty())
                        .map_or(self.status.as_str(), |l| l.message.as_str());
                    status_text(ui, shown, self.message_detail(shown), None);
                    if let Some(library) = &mut self.library
                        && library.offer_previous_import
                        && ui.small_button("Show").clicked()
                    {
                        library.show_previous_import();
                    }
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
    /// step through the zoom levels, Z toggles Fit and the last zoom. F is not
    /// here: it hides the chrome (see `chrome`), as Lightroom's does.
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
                _ => {}
            }
        }
    }
    pub(super) fn develop_shortcuts(&mut self, ctx: &egui::Context) {
        self.view.masking.expire_rename(ctx.cumulative_pass_nr());
        if !self.activity.is_busy() && !ctx.text_edit_focused() {
            self.zoom_keys(ctx);
            self.develop_delete_key(ctx);
            let (copy, paste, previous, treatment);
            let mut reset = false;
            let mut sync = false;
            let mut match_exposures = false;
            let mut new_preset = false;
            let mut auto = false;
            let mut select_all = false;
            let mut select_none = false;
            let mut export = None;
            let mut transfer = None;
            let mut targeted = None;
            {
                // A copy: `ctx.input` holds the context's lock, and a shortcut that
                // opens a photo uploads a texture, which waits for it forever.
                let i = &ctx.input(|i| i.clone());
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
                // With the modifiers held for the key, which a quick shortcut can
                // release in the same frame.
                let shift_command = |want: egui::Key| {
                    i.events.iter().any(|event| {
                        matches!(event, egui::Event::Key { key, pressed: true, modifiers, .. }
                            if *key == want && modifiers.command && modifiers.shift)
                    })
                };
                copy = shift_command(egui::Key::C);
                paste = shift_command(egui::Key::V);
                // Lightroom's Paste Settings from Previous. Option changes the typed
                // letter on macOS, so match the physical key too.
                previous = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key, physical_key, pressed: true, repeat: false, modifiers }
                        if (*key == egui::Key::V || *physical_key == Some(egui::Key::V))
                            && modifiers.command && modifiers.alt && !modifiers.shift)
                });
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
                // Once per press: holding V does not flip it back and forth. With the
                // key's own modifiers: Shift+Cmd+V released in the same frame pastes.
                treatment = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key: egui::Key::V, pressed: true, repeat: false, modifiers, .. }
                        if !modifiers.any())
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
                // Lightroom's Select All, as in the Library grid: every photo in
                // the filmstrip joins the selection so Sync reaches them all. Read
                // from the event, whose modifiers are those held for the press,
                // not the frame's, which a quick Cmd+A can outrun.
                if i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key: egui::Key::A, pressed: true, modifiers, .. }
                        if modifiers.command)
                }) {
                    select_all = true;
                }
                // Lightroom's Select None, as in the Library grid.
                if i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key: egui::Key::D, pressed: true, modifiers, .. }
                        if modifiers.command)
                }) {
                    select_none = true;
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
                // Shift+R is Reference View, below. C with its own modifiers, as
                // Shift+Cmd+C released in the same frame copies.
                let c = i.events.iter().any(|event| {
                    matches!(event, egui::Event::Key { key: egui::Key::C, pressed: true, modifiers, .. }
                        if !modifiers.command)
                });
                if c || i.key_pressed(egui::Key::R) && !i.modifiers.shift && !i.modifiers.command {
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
                    let view = self
                        .view
                        .compare
                        .toggled(super::before_after::Compare::BeforeOnly);
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
                // egui lets go of a text field on Escape before this runs; a mask's
                // name field still takes that Escape to keep the old name.
                if i.key_pressed(egui::Key::Escape) && !self.view.masking.is_renaming() {
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
                        Some(if i.modifiers.shift {
                            Kind::Radial
                        } else {
                            Kind::Linear
                        })
                    } else if i.key_pressed(egui::Key::J) && i.modifiers.shift {
                        Some(Kind::Color)
                    } else {
                        None
                    };
                    if let Some(kind) = kind {
                        self.create_mask(kind, None);
                    }
                }
            }
            // Select All takes the filmstrip's shown photos, so a following Sync
            // reaches them all; the open photo stays active.
            if select_all {
                let open = self.document.catalog_photo;
                if let Some(library) = &mut self.library {
                    library.develop_select_all(open);
                }
            }
            if select_none && let Some(library) = &mut self.library {
                library.select(None);
            }
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
        egui::Panel::bottom("status")
            .exact_size(STATUS_HEIGHT)
            .frame(status_frame(ui, PHOTO_MARGIN as i8))
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    let display = format!(
                        "Display: {}",
                        super::monitor::describe(
                            &self.view.monitor,
                            self.view.system_display.display()
                        )
                    );
                    status_text(
                        ui,
                        self.document
                            .edit
                            .save_state()
                            .message()
                            .unwrap_or(&self.status),
                        self.message_detail(&self.status),
                        Some(&display),
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

    fn develop_side_panels(&mut self, ui: &mut egui::Ui) {
        // From the window's right edge: the rail, the adjustments or the
        // open tool's drawer, then Presets or History beside the photo.
        let tabs = self.develop_rail_edge(ui);
        if self.panel_shown(WorkspacePanel::Right) && self.develop_adjustments_shown() {
            self.develop_right_panel(ui);
        }
        self.develop_tab_panel(ui, tabs);
    }
    fn develop_photo(&mut self, ui: &mut egui::Ui) {
        // The photo's backdrop reaches the panels around it, leaving the
        // photo a border of its own colour, as wide as the user set it.
        let backdrop = self.develop_backdrop(ui.ctx());
        let area = egui::CentralPanel::default()
            .frame(
                egui::Frame::new()
                    .fill(backdrop)
                    .inner_margin(egui::Margin::same(self.view.border as i8)),
            )
            .show(ui, |ui| {
                // The border opens the backdrop's menu too. Sensed before the
                // viewport, which is above it and keeps the clicks inside it.
                let border = ui.interact(
                    ui.max_rect().expand(self.view.border),
                    ui.id().with("backdrop-border"),
                    egui::Sense::click(),
                );
                self.viewport_ui(ui);
                self.backdrop_menu(&border, &[ui.max_rect()]);
            })
            .response
            .rect;
        self.drop_area = Some(area);
    }
    fn develop_right_panel(&mut self, ui: &mut egui::Ui) {
        // Dark as the catalog's sidebar, around the lighter photo.
        let fill = theme::palette(ui.ctx()).gray(22);
        egui::Panel::right("adjustments")
            .default_size(ADJUSTMENTS_DEFAULT)
            .min_size(ADJUSTMENTS_MIN)
            .max_size(400.)
            .frame(egui::Frame::side_top_panel(ui.style()).fill(fill))
            .show(ui, |ui| {
                let _side = super::widgets::SectionSide::enter(
                    ui,
                    super::widgets::SectionGroup::DevelopRight,
                );
                let enabled = self.document.full().is_some() && !self.view.compare.before_only();
                // The Navigator and Histogram scroll with the panels below
                // them, or stay at the top once pinned.
                let navigator = super::widgets::pinned(ui.ctx(), "Navigator");
                let histogram = super::widgets::pinned(ui.ctx(), "Histogram");
                let top = |editor: &mut Self, ui: &mut egui::Ui, pinned: bool| {
                    if navigator == pinned {
                        ui.scope(|ui| editor.navigator_ui(ui));
                    }
                    if histogram == pinned {
                        super::widgets::section_with(
                            ui,
                            "Histogram",
                            super::widgets::HeaderButton::Pin,
                            |ui| {
                                ui.add_enabled_ui(enabled, |ui| editor.histogram_ui(ui));
                            },
                        );
                    }
                };
                top(self, ui, true);
                if navigator || histogram {
                    ui.add_space(4.);
                }
                if super::inspector::has_drawer(self.view.tool) {
                    // The drawers' labels are short: their controls take the room.
                    let _labels = super::widgets::LabelColumn::enter(
                        ui,
                        super::inspector::DRAWER_LABEL_COLUMN,
                    );
                    egui::ScrollArea::vertical()
                        .id_salt("tool-drawer")
                        .show(ui, |ui| {
                            top(self, ui, false);
                            ui.add_enabled_ui(enabled, |ui| self.tool_drawer(ui));
                        });
                } else {
                    egui::ScrollArea::vertical().show(ui, |ui| {
                        top(self, ui, false);
                        ui.add_enabled_ui(enabled, |ui| self.controls(ui));
                    });
                }
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
        } else if self.activity.is_changing_files() {
            Some(FILE_CHANGE)
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
            // A file operation stops after the photo under way; closing
            // waits for it, or goes anyway, saving nothing while it holds
            // the catalog (the open edit was saved before it began, and the
            // journal leaves what it cut short to recovery).
            self.file_ops.stop();
            let flushed = if !anyway && self.closing_waits_for().is_some() {
                None
            } else if anyway && self.activity.is_changing_files() {
                Some(Flushed::Saved)
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
                    Some(FOLDER_CHANGE | FILE_CHANGE) => Some("Close anyway"),
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
/// egui-winit reads Cmd+C and Cmd+V with any other modifier as the clipboard's
/// Copy and Paste and drops the key, so Shift+Cmd+C (Copy Settings),
/// Shift+Cmd+V (Paste Settings) and Option+Cmd+V (Paste from Previous) never
/// reach the shortcuts. Their keys go back in after the clipboard events, which
/// text fields still take. A Paste with an empty clipboard is not sent at all.
pub(super) fn clipboard_shortcut_keys(ctx: &egui::Context, input: &mut egui::RawInput) {
    let mut modifiers = ctx.input(|i| i.modifiers);
    let mut events = Vec::with_capacity(input.events.len());
    for event in input.events.drain(..) {
        let key = match &event {
            egui::Event::ModifiersChanged(changed) => {
                modifiers = *changed;
                None
            }
            egui::Event::Copy => Some(egui::Key::C),
            egui::Event::Paste(_) => Some(egui::Key::V),
            _ => None,
        };
        events.push(event);
        if let Some(key) = key
            && modifiers.command
            && (modifiers.shift || modifiers.alt)
        {
            events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            });
        }
    }
    input.events = events;
}
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

/// The photo's backdrop around it in Develop and the Library's views.
pub(super) const PHOTO_MARGIN: f32 = 20.;

/// The status line's height, its text centred in it.
const STATUS_HEIGHT: f32 = 26.;
/// The status line's frame: the panels' fill, inset at the sides only, so its
/// text sits midway between the filmstrip and the window's edge; `side` is
/// the inset, Develop's the photo area's margin.
fn status_frame(ui: &egui::Ui, side: i8) -> egui::Frame {
    egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(side, 0))
}

/// What the close guard calls an export or preview a control command started.
const OUTPUT: &str = "the output the control socket asked for";
/// What the close guard calls a folder change.
const FOLDER_CHANGE: &str = "the folder change";
const FILE_CHANGE: &str = "the file operation";

/// The side panels' narrowest widths, no less than their contents need: a
/// panel dragged narrower is painted only that wide but laid out as wide as
/// its contents, leaving a strip of the window's black between. The right
/// panels hold the Navigator, whose zoom levels need 224 points, as do the
/// catalog's sidebar and Develop's Presets.
pub(super) const PRESETS_MIN: f32 = 224.;
pub(super) const ADJUSTMENTS_MIN: f32 = 300.;
/// The adjustments' width until the user drags it; Develop's tab panel folds
/// by itself when the photo would have too little room beside it.
pub(super) const ADJUSTMENTS_DEFAULT: f32 = 330.;
pub(super) const LIBRARY_SIDEBAR_MIN: f32 = 224.;
pub(super) const LIBRARY_INFO_MIN: f32 = 224.;

/// The workspace bar's height; on macOS the traffic lights sit on its centre.
pub(super) const BAR_HEIGHT: f32 = 44.;

/// With the chrome hidden the window has no bar to drag by. This keeps a grab
/// beside the traffic lights, the size of their inset, so the window still
/// moves while almost all of the photo stays its own. Drawn after the photo,
/// whose view takes every click in its area, so the corner is the window's.
fn title_bar_pad(ui: &mut egui::Ui) {
    let corner = ui.max_rect().min;
    let width = fastframe_titlebar::footprint(ui.ctx(), BAR_HEIGHT).left + 24.;
    crate::platform::titlebar::drag(
        ui,
        egui::Rect::from_min_size(corner, egui::Vec2::new(width, BAR_HEIGHT)),
    );
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

/// Whether the window opens maximized when it was left so. Not on macOS, where egui
/// reads the state only at launch (emilk/egui#3494), so a window zoomed back out
/// would still open zoomed.
pub(super) const KEEPS_MAXIMIZED: bool = !cfg!(target_os = "macos");

/// How long a window reopened maximized waits for the window manager to maximize
/// it before its state is saved again: until then it shows unmaximized on X11.
const MAXIMIZE_GRACE: f64 = 2.;

/// Reopening the window maximized, as it was left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) enum RestoreMaximized {
    /// The window manager is to be asked on the first frame.
    Pending,
    /// Asked; the window's state is kept until it is maximized or `until` (egui
    /// time) passes, so the frames before don't save it unmaximized.
    Asked {
        until: f64,
    },
    Done,
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

/// The Library's Sync Settings button's hover, for `targets` photos to sync to.
fn sync_hover(targets: usize) -> String {
    let keys = if cfg!(target_os = "macos") {
        "⇧⌘S"
    } else {
        "Ctrl+Shift+S"
    };
    if targets == 0 {
        return format!("Select the photos to sync with the active photo · {keys}");
    }
    let photos = super::widgets::plural(targets, "other selected photo", "other selected photos");
    format!("Sync the active photo's settings to {photos} · {keys}")
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
